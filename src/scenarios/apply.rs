use super::fixture::{FieldType, Scenario};
use crate::gecko::{GeckoApi, Target, resource_id};
use crate::{auth, session};
use anyhow::{Context, Result, ensure};
use clap::Args;
use reqwest::Method;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Args)]
pub struct ApplyArgs {
    /// Scenario JSON fixture.
    pub fixture: PathBuf,
    /// Must match the profile in the saved session; use a development profile.
    #[arg(long)]
    pub profile_id: String,
    #[arg(long, env = "CLI_TOOLS_APP_API_URL", hide_env_values = true)]
    pub app_api_base_url: String,
    #[arg(long)]
    pub app_token_file: Option<PathBuf>,
    #[arg(long)]
    pub session_file: Option<PathBuf>,
    /// Progress journal. Defaults to FIXTURE.apply-state.json; keep it for reruns.
    #[arg(long)]
    pub state_file: Option<PathBuf>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct State {
    pub(super) version: u32,
    pub(super) target: Target,
    pub(super) fixture_sha256: String,
    pub(super) run_id: String,
    /// Keys are field:KEY, group:KEY, contact:KEY and populated:KEY.
    pub(super) completed: BTreeMap<String, String>,
    /// Written before each mutation. An unknown outcome must never be retried automatically.
    pub(super) pending: Option<String>,
    #[serde(default)]
    pub(super) creation: BTreeMap<String, CreationEvidence>,
    #[serde(default)]
    pub(super) lifecycle: Lifecycle,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(super) enum Lifecycle {
    #[default]
    Active,
    CleanupStarted,
    Cleaned,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct CreationEvidence {
    pub created_at: u64,
    pub uuid: Option<String>,
}
impl CreationEvidence {
    pub fn from_resource(resource: &Value) -> Option<Self> {
        let value = &resource["created_at"];
        let created_at = value
            .as_u64()
            .or_else(|| value.as_str().and_then(|v| v.parse().ok()))
            .filter(|n| *n > 0)?;
        let uuid = resource["uuid"]
            .as_str()
            .filter(|s| !s.is_empty())
            .map(String::from);
        Some(Self { created_at, uuid })
    }
    pub fn matches(&self, resource: &Value) -> bool {
        Self::from_resource(resource).is_some_and(|actual| {
            actual.created_at == self.created_at
                && self
                    .uuid
                    .as_ref()
                    .is_none_or(|id| actual.uuid.as_ref() == Some(id))
        })
    }
}

pub fn run(args: &ApplyArgs, scenario: Scenario) -> Result<()> {
    scenario.validate()?;
    let session_path = args
        .session_file
        .clone()
        .or_else(session::default_session_file)
        .context("no session file path available")?;
    let selected = session::load_session(&session_path)?;
    ensure!(
        selected.profile_id == args.profile_id,
        "requested profile {} does not match saved profile {}; select it with the profiles command first",
        args.profile_id,
        selected.profile_id
    );
    let token_path = args
        .app_token_file
        .clone()
        .or_else(auth::default_app_token_file)
        .context("no app token file path available")?;
    let tokens = auth::load_tokens(&token_path)?;
    crate::app_identity::validate_token_profile(&tokens, &selected)?;
    let api = GeckoApi::new(&args.app_api_base_url, &tokens, &selected)?;
    let target = Target {
        base_url: api.base_url.clone(),
        account_id: selected.account_id.clone(),
        profile_id: selected.profile_id.clone(),
    };
    let path = args
        .state_file
        .clone()
        .unwrap_or_else(|| args.fixture.with_extension("apply-state.json"));
    let _lock = lock_state(&path)?;
    let mut state = load_state(&path, target, &scenario)?;
    ensure!(
        state.lifecycle == Lifecycle::Active,
        "cleanup has started for this run; use a new journal for a new run"
    );
    ensure!(
        state.pending.is_none(),
        "operation {:?} has an uncertain outcome; inspect Gecko and reconcile {} before retrying (see README recovery instructions)",
        state.pending,
        path.display()
    );
    execute(&api, &path, &mut state, &scenario)
        .with_context(|| format!("scenario stopped; progress is saved in {}", path.display()))?;
    eprintln!(
        "Applied {} contacts and {} groups to {} (profile {}). State: {}",
        scenario.contacts.len(),
        scenario.groups.len(),
        selected.account_name,
        selected.profile_id,
        path.display()
    );
    Ok(())
}

pub(super) fn load_state(path: &Path, target: Target, scenario: &Scenario) -> Result<State> {
    let fixture_sha256 = format!("{:x}", Sha256::digest(serde_json::to_vec(scenario)?));
    match fs::read(path) {
        Ok(data) => {
            let state: State = serde_json::from_slice(&data)
                .context("invalid apply state; do not discard it to retry a partial run")?;
            ensure!(
                state.version == 1
                    && state.target == target
                    && state.fixture_sha256 == fixture_sha256,
                "apply state belongs to a different fixture or target; keep it and use a separate state file for a new run"
            );
            Ok(state)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(State {
            version: 1,
            target,
            fixture_sha256,
            run_id: format!(
                "{:x}-{:x}",
                SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos(),
                std::process::id()
            ),
            completed: BTreeMap::new(),
            pending: None,
            creation: BTreeMap::new(),
            lifecycle: Lifecycle::Active,
        }),
        Err(error) => Err(error).context("cannot read apply state"),
    }
}

fn execute(api: &GeckoApi, path: &Path, state: &mut State, scenario: &Scenario) -> Result<()> {
    if scenario.contacts.iter().all(|c| {
        state
            .completed
            .contains_key(&format!("populated:{}", c.key))
    }) && scenario
        .groups
        .iter()
        .all(|g| state.completed.contains_key(&format!("group:{}", g.key)))
    {
        return Ok(());
    }
    let fields = api.fields()?;
    let mut identifiers = BTreeMap::new();
    // Resolve standard fields by type, never by the contact list's display column numbers.
    for field in scenario
        .fields
        .iter()
        .filter(|field| matches!(field.kind, FieldType::Name | FieldType::Email))
    {
        let candidates = fields
            .iter()
            .filter(|item| item["type"] == field.kind.as_str())
            .collect::<Vec<_>>();
        ensure!(
            candidates.len() == 1,
            "expected exactly one {} contact field, found {}; configure an unambiguous development profile first",
            field.kind.as_str(),
            candidates.len()
        );
        identifiers.insert(
            field.key.clone(),
            format!("field{}", resource_id(candidates[0])?),
        );
    }
    // Gecko requires existing mandatory fields on every new contact.
    for field in &fields {
        if truthy(&field["required"]) {
            let identifier = format!("field{}", resource_id(field)?);
            let logical_key = identifiers
                .iter()
                .find(|(_, id)| **id == identifier)
                .map(|(key, _)| key);
            let key = logical_key.with_context(|| format!("profile has an unsupported required field {:?}; use a development profile with only name/email required", field["label"]))?;
            ensure!(
                scenario.contacts.iter().all(|c| !c.fields[key].is_null()),
                "profile requires {key}, but the fixture contains missing values"
            );
        }
    }
    save_state(path, state)?;
    for field in scenario
        .fields
        .iter()
        .filter(|field| !matches!(field.kind, FieldType::Name | FieldType::Email))
    {
        let id = mutation(
            api,
            path,
            state,
            &format!("field:{}", field.key),
            "fields",
            "field",
            &json!({
                "field_type": "contact", "type": field.kind.as_str(), "label": format!("{}: {}", scenario.name, field.label),
                "tag": format!("cli_{}_{}", scenario.name.replace('-', "_"), field.key), "required": false, "matchable": false,
            }),
        )?;
        identifiers.insert(field.key.clone(), format!("field{id}"));
    }
    for group in &scenario.groups {
        mutation(
            api,
            path,
            state,
            &format!("group:{}", group.key),
            "groups",
            "group",
            &json!({
                "name": group.name, "permissions": group.permissions,
            }),
        )?;
    }
    let name_id = &identifiers["name"];
    let email_id = &identifiers["email"];
    for (index, contact) in scenario.contacts.iter().enumerate() {
        let values = contact
            .fields
            .iter()
            .map(|(key, value)| (identifiers[key].clone(), value.clone()))
            .collect::<BTreeMap<_, _>>();
        let mut initial = values.clone();
        // /contacts merges matching values on CREATE. Unique temporary identities followed by
        // an UPDATE by ID preserve intentional duplicates without changing account matching rules.
        initial.insert(name_id.clone(), json!({"first_name": "Scenario", "last_name": format!("{}-{}", state.run_id, index + 1)}));
        if !contact.fields["email"].is_null() {
            initial.insert(
                email_id.clone(),
                json!(format!("cli-{}-{}@example.test", state.run_id, index + 1)),
            );
        } else {
            // Null on update does not erase an existing Gecko value. Never create it in this case.
            initial.remove(email_id);
        }
        let id = mutation(
            api,
            path,
            state,
            &format!("contact:{}", contact.key),
            "contacts",
            "contact",
            &json!({"fields": initial}),
        )?;
        let contact_ids = state
            .completed
            .iter()
            .filter(|(key, _)| key.starts_with("contact:"))
            .map(|(_, id)| id)
            .collect::<Vec<_>>();
        ensure!(
            contact_ids.iter().collect::<BTreeSet<_>>().len() == contact_ids.len(),
            "Gecko returned the same ID for multiple contacts; stop and inspect account matching rules"
        );
        let updated = mutation(
            api,
            path,
            state,
            &format!("populated:{}", contact.key),
            &format!("contacts/{id}"),
            "contact",
            &json!({"fields": values}),
        )?;
        ensure!(updated == id, "contact update returned an unexpected ID");
        if (index + 1) % 25 == 0 || index + 1 == scenario.contacts.len() {
            eprintln!("Contacts: {}/{}", index + 1, scenario.contacts.len());
        }
    }
    Ok(())
}

fn mutation(
    api: &GeckoApi,
    path: &Path,
    state: &mut State,
    key: &str,
    endpoint: &str,
    singular: &str,
    body: &Value,
) -> Result<String> {
    if let Some(id) = state.completed.get(key) {
        return Ok(id.clone());
    }
    state.pending = Some(key.into());
    save_state(path, state)?;
    // No automatic retries, including 4xx/5xx: a server can have committed a write before failing.
    let response = api
        .request(Method::POST, endpoint, &[], Some(body))
        .with_context(|| {
            format!("{key} failed; its outcome is uncertain and will not be retried automatically")
        })?;
    let resource = response
        .get(singular)
        .or_else(|| response.get("data"))
        .unwrap_or(&response);
    let resource = if let Some(items) = resource.as_array() {
        ensure!(
            items.len() == 1,
            "{key} returned an unexpected number of resources"
        );
        &items[0]
    } else {
        resource
    };
    let id = resource_id(resource).with_context(|| {
        format!("{key} succeeded without a usable resource ID; inspect the server before retrying")
    })?;
    if let Some(expected) = endpoint.strip_prefix("contacts/") {
        ensure!(
            id == expected,
            "contact update returned an unexpected ID; inspect the server before retrying"
        );
    }
    if !key.starts_with("populated:")
        && let Some(evidence) = CreationEvidence::from_resource(resource)
    {
        state.creation.insert(key.into(), evidence);
    }
    state.completed.insert(key.into(), id.clone());
    state.pending = None;
    save_state(path, state)?;
    Ok(id)
}

fn truthy(value: &Value) -> bool {
    value == true || value == 1 || value == "1"
}

pub(crate) fn lock_state(path: &Path) -> Result<File> {
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(suffix_path(path, ".lock"))
        .context("cannot open apply state lock; ensure the parent directory exists")?;
    lock.try_lock()
        .context("another process is using this apply state")?;
    Ok(lock)
}

fn suffix_path(path: &Path, suffix: &str) -> PathBuf {
    let mut value = path.as_os_str().to_owned();
    value.push(suffix);
    PathBuf::from(value)
}

pub(super) fn save_state(path: &Path, state: &State) -> Result<()> {
    let temporary = suffix_path(path, ".tmp");
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&temporary).with_context(|| {
        format!(
            "cannot create state checkpoint {}; inspect any leftover checkpoint before removing it",
            temporary.display()
        )
    })?;
    serde_json::to_writer_pretty(&mut file, state)?;
    file.write_all(b"\n")?;
    file.sync_all()?;
    fs::rename(&temporary, path).context("cannot commit apply state checkpoint")?;
    #[cfg(unix)]
    File::open(
        path.parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new(".")),
    )?
    .sync_all()?;
    Ok(())
}

#[cfg(test)]
mod tests;

pub(crate) fn recorded_contacts(path: &Path, target: &Target) -> Result<Vec<String>> {
    let state: State =
        serde_json::from_slice(&fs::read(path)?).context("invalid scenario journal")?;
    ensure!(
        state.version == 1 && &state.target == target,
        "scenario journal belongs to another target"
    );
    ensure!(
        state.pending.is_none(),
        "scenario journal has an uncertain operation; reconcile it first"
    );
    ensure!(
        state.lifecycle == Lifecycle::Active,
        "cleanup has started for this scenario; it cannot be used as a batch source"
    );
    let mut ids = BTreeSet::new();
    for (key, id) in &state.completed {
        if let Some(key) = key.strip_prefix("contact:") {
            ensure!(
                state.completed.get(&format!("populated:{key}")) == Some(id),
                "scenario contact {key} is not fully populated"
            );
            ensure!(
                id.parse::<u64>().is_ok_and(|id| id > 0),
                "invalid recorded contact ID"
            );
            ensure!(
                ids.insert(id.clone()),
                "scenario journal repeats a created contact ID"
            );
        }
        if let Some(key) = key.strip_prefix("populated:") {
            ensure!(
                state.completed.get(&format!("contact:{key}")) == Some(id),
                "scenario journal has an orphan population checkpoint"
            );
        }
    }
    ensure!(
        !ids.is_empty(),
        "scenario journal contains no completed contacts"
    );
    Ok(ids.into_iter().collect())
}
