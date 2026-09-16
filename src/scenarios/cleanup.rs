use super::{
    apply::{self, ApplyArgs, Lifecycle, State},
    fixture::Scenario,
    verify,
};
use crate::gecko::{GeckoApi, Target, resource_id};
use anyhow::{Context, Result, ensure};
use clap::Args;
use reqwest::Method;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
};

#[derive(Debug, Args)]
pub struct CleanupArgs {
    #[command(flatten)]
    pub run: ApplyArgs,
    /// Delete only recorded resources whose ownership and use checks pass.
    #[arg(long)]
    pub execute: bool,
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CleanupState {
    version: u32,
    target: Target,
    fixture_sha256: String,
    run_id: String,
    creation_sha256: String,
    removed: BTreeSet<String>,
    pending: Option<String>,
}
#[derive(Debug, Clone, Serialize)]
struct Resource {
    key: String,
    id: String,
    kind: &'static str,
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
enum Action {
    Delete,
    Retain,
    AlreadyMissing,
    AlreadyDeleted,
}
#[derive(Debug, Clone, Serialize)]
struct Decision {
    resource: Resource,
    action: Action,
    reason: String,
}
fn decision(resource: &Resource, action: Action, reason: &str) -> Decision {
    Decision {
        resource: resource.clone(),
        action,
        reason: reason.into(),
    }
}

pub fn run(args: &CleanupArgs, scenario: &Scenario) -> Result<()> {
    let result = execute(args, scenario)?;
    println!("{}", serde_json::to_string_pretty(&result)?);
    Ok(())
}
fn execute(args: &CleanupArgs, scenario: &Scenario) -> Result<Value> {
    // prepare acquires the existing apply lock before reading either lifecycle journal.
    let (api, path, _lock, mut applied) = verify::prepare(&args.run, scenario)?;
    ensure!(
        applied.pending.is_none(),
        "apply has an uncertain operation; reconcile it before cleanup"
    );
    let resources = resources(&applied, scenario);
    let cleanup_path = cleanup_path(&path);
    let mut cleanup = load(&cleanup_path, &applied)?;
    ensure!(
        cleanup
            .removed
            .iter()
            .all(|k| resources.iter().any(|r| &r.key == k)),
        "cleanup journal contains an unexpected removed resource"
    );
    // A lost DELETE response may be reconciled by an authenticated absence read, never by retrying.
    if let Some(key) = cleanup.pending.clone() {
        let resource = resources
            .iter()
            .find(|r| r.key == key)
            .context("cleanup pending resource is not recorded")?;
        ensure!(
            read(&api, resource)?.is_none(),
            "delete outcome for {key} remains uncertain; resource still exists; inspect and reconcile before retrying"
        );
        cleanup.removed.insert(key);
        cleanup.pending = None;
    }
    let mut preview = Vec::new();
    for resource in &resources {
        preview.push(assess(&api, &applied, &cleanup, scenario, resource)?);
    }
    if !args.execute {
        return Ok(report(&applied, &cleanup, false, &preview));
    }
    applied.lifecycle = Lifecycle::CleanupStarted;
    apply::save_state(&path, &applied)?;
    save(&cleanup_path, &cleanup)?;
    let mut results = Vec::new();
    for resource in &resources {
        // Revalidate identity/use immediately before deletion; do not reuse the preview decision.
        let result = assess(&api, &applied, &cleanup, scenario, resource)?;
        match result.action {
            Action::Delete => {
                cleanup.pending = Some(resource.key.clone());
                save(&cleanup_path, &cleanup)?;
                api.request(
                    Method::DELETE,
                    &format!("{}/{}", resource.kind, resource.id),
                    &[],
                    None,
                )
                .with_context(|| {
                    format!(
                        "delete outcome for {} is uncertain; progress in {}",
                        resource.key,
                        cleanup_path.display()
                    )
                })?;
                ensure!(
                    read(&api, resource)?.is_none(),
                    "{} remains visible after deletion; outcome stays pending",
                    resource.key
                );
                cleanup.removed.insert(resource.key.clone());
                cleanup.pending = None;
                save(&cleanup_path, &cleanup)?;
            }
            Action::AlreadyMissing => {
                cleanup.removed.insert(resource.key.clone());
                save(&cleanup_path, &cleanup)?;
            }
            Action::AlreadyDeleted | Action::Retain => {}
        }
        results.push(result);
    }
    if cleanup.removed.len() == resources.len() {
        applied.lifecycle = Lifecycle::Cleaned;
        apply::save_state(&path, &applied)?;
    }
    Ok(report(&applied, &cleanup, true, &results))
}
fn report(
    applied: &State,
    cleanup: &CleanupState,
    executed: bool,
    decisions: &[Decision],
) -> Value {
    json!({"executed":executed,"target":applied.target,"lifecycle":applied.lifecycle,"removed":cleanup.removed.len(),"retained":decisions.iter().filter(|d|matches!(d.action,Action::Retain)).count(),"pending":cleanup.pending,"resources":decisions})
}
fn resources(state: &State, scenario: &Scenario) -> Vec<Resource> {
    let mut result = Vec::new();
    for (prefix, kind, keys) in [
        (
            "contact",
            "contacts",
            scenario
                .contacts
                .iter()
                .map(|r| r.key.as_str())
                .collect::<Vec<_>>(),
        ),
        (
            "group",
            "groups",
            scenario.groups.iter().map(|r| r.key.as_str()).collect(),
        ),
        (
            "field",
            "fields",
            scenario.fields.iter().map(|r| r.key.as_str()).collect(),
        ),
    ] {
        for key in keys {
            let key = format!("{prefix}:{key}");
            if let Some(id) = state.completed.get(&key) {
                result.push(Resource {
                    key,
                    id: id.clone(),
                    kind,
                });
            }
        }
    }
    result
}
fn cleanup_path(path: &Path) -> PathBuf {
    let mut p = path.as_os_str().to_os_string();
    p.push(".cleanup.json");
    PathBuf::from(p)
}
fn load(path: &Path, applied: &State) -> Result<CleanupState> {
    let digest = format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&(
            &applied.completed,
            &applied.creation
        ))?)
    );
    match fs::read(path) {
        Ok(data) => {
            let state: CleanupState = serde_json::from_slice(&data)
                .context("invalid cleanup journal; do not discard it to retry")?;
            ensure!(
                state.version == 1
                    && state.target == applied.target
                    && state.fixture_sha256 == applied.fixture_sha256
                    && state.run_id == applied.run_id
                    && state.creation_sha256 == digest,
                "cleanup journal belongs to another run or changed creation evidence"
            );
            Ok(state)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(CleanupState {
            version: 1,
            target: applied.target.clone(),
            fixture_sha256: applied.fixture_sha256.clone(),
            run_id: applied.run_id.clone(),
            creation_sha256: digest,
            removed: BTreeSet::new(),
            pending: None,
        }),
        Err(e) => Err(e).context("cannot read cleanup journal"),
    }
}
fn save(path: &Path, state: &CleanupState) -> Result<()> {
    crate::storage::write_private_atomic(path, &serde_json::to_vec_pretty(state)?)
}
fn read(api: &GeckoApi, resource: &Resource) -> Result<Option<Value>> {
    let (singular, query) = match resource.kind {
        "contacts" => (
            "contact",
            vec![("contact_rfields", "id,uuid,created_at".into())],
        ),
        "groups" => (
            "group",
            vec![
                ("include", "permissions:1000".into()),
                ("permission_rfields", "id,key,always".into()),
            ],
        ),
        _ => ("field", vec![]),
    };
    verify::read_resource(api, resource.kind, singular, &resource.id, &query)
}
fn assess(
    api: &GeckoApi,
    state: &State,
    cleanup: &CleanupState,
    scenario: &Scenario,
    resource: &Resource,
) -> Result<Decision> {
    if cleanup.removed.contains(&resource.key) {
        return Ok(decision(
            resource,
            Action::AlreadyDeleted,
            "confirmed by this cleanup journal",
        ));
    }
    let Some(object) = read(api, resource)? else {
        return Ok(decision(
            resource,
            Action::AlreadyMissing,
            "authenticated read confirms absence",
        ));
    };
    let Some(evidence) = state.creation.get(&resource.key) else {
        return Ok(decision(
            resource,
            Action::Retain,
            "journal lacks creation evidence; inspect this older run manually",
        ));
    };
    if !evidence.matches(&object) {
        return Ok(decision(
            resource,
            Action::Retain,
            "creation identity differs; resource may have been replaced or reused",
        ));
    }
    if resource.kind == "contacts" {
        if evidence.uuid.is_none() {
            return Ok(decision(
                resource,
                Action::Retain,
                "contact creation UUID is unavailable",
            ));
        }
        return Ok(decision(
            resource,
            Action::Delete,
            "recorded contact ID, creation time and UUID match",
        ));
    }
    if resource.kind == "fields" {
        return Ok(decision(
            resource,
            Action::Retain,
            "field deletion can detach form/integration references; a complete dependency check is unavailable",
        ));
    }
    let group = scenario
        .groups
        .iter()
        .find(|g| resource.key == format!("group:{}", g.key))
        .context("group not in fixture")?;
    if object["type"] != "custom" || object["name"] != group.name {
        return Ok(decision(
            resource,
            Action::Retain,
            "group type or name changed",
        ));
    }
    let users = object["user_count"]
        .as_u64()
        .or_else(|| object["user_count"].as_str().and_then(|s| s.parse().ok()));
    if users != Some(0) {
        return Ok(decision(
            resource,
            Action::Retain,
            "group has users or its user count is unavailable",
        ));
    }
    let Some(permissions) = object["permissions"].as_array().filter(|p| p.len() < 1000) else {
        return Ok(decision(
            resource,
            Action::Retain,
            "group permissions cannot be verified",
        ));
    };
    let mut actual = BTreeSet::new();
    for permission in permissions {
        let Some(key) = permission["key"].as_str() else {
            return Ok(decision(
                resource,
                Action::Retain,
                "group permissions cannot be verified",
            ));
        };
        actual.insert(key);
        if !group.permissions.iter().any(|p| p == key)
            && !crate::contacts::truthy(permission.get("always"))
        {
            return Ok(decision(
                resource,
                Action::Retain,
                "group permissions changed",
            ));
        }
    }
    if group
        .permissions
        .iter()
        .any(|p| !actual.contains(p.as_str()))
    {
        return Ok(decision(
            resource,
            Action::Retain,
            "group permissions changed",
        ));
    }
    let field_groups = match api.collection("contact-field-groups", &[("perPage", "100".into())]) {
        Ok(groups) => groups,
        Err(_) => {
            return Ok(decision(
                resource,
                Action::Retain,
                "contact-field-group references cannot be inspected",
            ));
        }
    };
    for group in field_groups {
        let Some(ids) = group["userGroupIds"].as_array() else {
            return Ok(decision(
                resource,
                Action::Retain,
                "contact-field-group assignments unavailable",
            ));
        };
        for id in ids {
            if resource_id(&json!({"id":id}))? == resource.id {
                return Ok(decision(
                    resource,
                    Action::Retain,
                    "group is used by a contact-field access rule",
                ));
            }
        }
    }
    Ok(decision(
        resource,
        Action::Delete,
        "created custom group is unchanged, has no users and no contact-field access rule",
    ))
}

#[cfg(test)]
mod tests;
