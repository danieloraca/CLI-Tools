use super::fixture::{FieldType, Scenario};
use crate::{auth, session};
use anyhow::{Context, Result, bail, ensure};
use clap::Args;
use reqwest::{Method, Url, blocking::Client, redirect::Policy};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[derive(Debug, Args)]
pub struct ApplyArgs {
    /// Generated JSON fixture to apply.
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Target {
    base_url: String,
    account_id: String,
    profile_id: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct State {
    version: u32,
    target: Target,
    fixture_sha256: String,
    run_id: String,
    /// Keys are field:KEY, group:KEY, contact:KEY and populated:KEY.
    completed: BTreeMap<String, String>,
    /// Written before each mutation. An unknown outcome must never be retried automatically.
    pending: Option<String>,
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

fn load_state(path: &Path, target: Target, scenario: &Scenario) -> Result<State> {
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
    state.completed.insert(key.into(), id.clone());
    state.pending = None;
    save_state(path, state)?;
    Ok(id)
}

fn resource_id(resource: &Value) -> Result<String> {
    let id = match &resource["id"] {
        Value::String(value) => value.clone(),
        Value::Number(value) if value.as_u64().is_some() => value.to_string(),
        _ => bail!("API resource has no usable id"),
    };
    ensure!(
        !id.is_empty() && id != "0" && id.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-'),
        "API resource has an invalid id"
    );
    Ok(id)
}

fn truthy(value: &Value) -> bool {
    value == true || value == 1 || value == "1"
}

struct GeckoApi {
    base_url: String,
    http: Client,
    account_id: String,
}

impl GeckoApi {
    fn new(
        base_url: &str,
        tokens: &auth::TokenSet,
        selected: &session::AppSession,
    ) -> Result<Self> {
        let base_url = crate::api::normalize_base_url(base_url.to_string())?;
        let url = Url::parse(&base_url).context("invalid app API URL")?;
        ensure!(
            url.host_str().is_some()
                && url.username().is_empty()
                && url.password().is_none()
                && url.query().is_none()
                && url.fragment().is_none(),
            "app API URL must have a host and no credentials, query or fragment"
        );
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert("Accept", "application/json".parse()?);
        headers.insert(
            "Gecko-Account",
            selected.account_id.parse().context("invalid account id")?,
        );
        headers.insert(
            "Gecko-User",
            selected.user_id.parse().context("invalid user id")?,
        );
        let mut bearer: reqwest::header::HeaderValue = format!("Bearer {}", tokens.access_token)
            .parse()
            .context("invalid app token")?;
        bearer.set_sensitive(true);
        headers.insert(reqwest::header::AUTHORIZATION, bearer);
        let http = Client::builder()
            .user_agent("cli_tools/0.1.0")
            .default_headers(headers)
            .redirect(Policy::none())
            .timeout(Duration::from_secs(30))
            .build()?;
        Ok(Self {
            base_url,
            http,
            account_id: selected.account_id.clone(),
        })
    }

    fn request(
        &self,
        method: Method,
        endpoint: &str,
        query: &[(&str, String)],
        body: Option<&Value>,
    ) -> Result<Value> {
        let mut request = self
            .http
            .request(method, format!("{}/{endpoint}", self.base_url))
            .query(query);
        if let Some(body) = body {
            request = request.json(body);
        }
        let response = request.send().context("Gecko API request failed")?;
        let status = response.status();
        if status.is_success() {
            crate::app_identity::validate_account(response.headers(), &self.account_id)?;
        }
        let payload: Value = response
            .json()
            .with_context(|| format!("Gecko API returned non-JSON with HTTP {status}"))?;
        if !status.is_success() {
            let message = ["error", "message", "Error", "Message"]
                .iter()
                .find_map(|key| payload.get(key).and_then(Value::as_str))
                .unwrap_or("request rejected");
            bail!("Gecko API returned HTTP {status}: {message}");
        }
        Ok(payload)
    }

    fn fields(&self) -> Result<Vec<Value>> {
        let mut fields = Vec::new();
        let mut seen = BTreeSet::new();
        // Continue to an empty page, even if the server silently caps per_page below 100.
        for page in 1..=1000 {
            let response = self.request(
                Method::GET,
                "fields",
                &[
                    ("field_type", "contact".into()),
                    ("per_page", "100".into()),
                    ("page", page.to_string()),
                ],
                None,
            )?;
            let items = response
                .get("fields")
                .or_else(|| response.get("data"))
                .unwrap_or(&response)
                .as_array()
                .context("fields response must contain an array")?;
            if items.is_empty() {
                return Ok(fields);
            }
            for item in items {
                ensure!(
                    seen.insert(resource_id(item)?),
                    "field pagination repeated an ID; cannot reliably inspect this profile"
                );
                ensure!(
                    item["field_type"] == "contact",
                    "API returned a non-contact field"
                );
                fields.push(item.clone());
            }
        }
        bail!("field pagination exceeded 1000 pages")
    }
}

fn lock_state(path: &Path) -> Result<File> {
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

fn save_state(path: &Path, state: &State) -> Result<()> {
    let temporary = suffix_path(path, ".tmp");
    let mut file = OpenOptions::new().write(true).create_new(true).open(&temporary)
        .with_context(|| format!("cannot create state checkpoint {}; inspect any leftover checkpoint before removing it", temporary.display()))?;
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
