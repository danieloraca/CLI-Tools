use crate::{
    catalog::Connection,
    gecko::{GeckoApi, Target, resource_id},
};
use anyhow::{Context, Result, ensure};
use clap::{Args, Subcommand};
use reqwest::Method;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::PathBuf,
};

#[derive(Debug, Args)]
pub struct Selection {
    #[command(flatten)]
    pub connection: Connection,
    /// Must match the selected development profile.
    #[arg(long)]
    pub profile_id: String,
    /// Explicit contact ID. Repeat for multiple contacts (maximum 10,000).
    #[arg(long, value_parser=clap::value_parser!(u64).range(1..), required_unless_present="scenario_state", conflicts_with="scenario_state")]
    pub contact_id: Vec<u64>,
    /// Use completed contacts recorded in this scenario apply journal.
    #[arg(long)]
    pub scenario_state: Option<PathBuf>,
    /// Private progress journal, bound to this operation and exact contact IDs.
    #[arg(long)]
    pub journal: PathBuf,
    /// Execute the previewed changes. Without this flag, only preview.
    #[arg(long)]
    pub execute: bool,
}
#[derive(Debug, Args)]
pub struct LabelArgs {
    #[command(flatten)]
    pub selection: Selection,
    #[arg(long, value_parser=clap::value_parser!(u64).range(1..))]
    pub label_id: u64,
}
#[derive(Debug, Args)]
pub struct ConsentArgs {
    #[command(flatten)]
    pub selection: Selection,
    #[arg(long, value_parser=clap::value_parser!(u64).range(1..))]
    pub consent_id: u64,
}
#[derive(Debug, Subcommand)]
pub enum BatchCommand {
    /// Add an existing label to selected contacts.
    LabelAdd(LabelArgs),
    /// Grant one existing consent reason, preserving all others.
    ConsentGrant(ConsentArgs),
    /// Revoke one existing consent reason, preserving all others.
    ConsentRevoke(ConsentArgs),
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Operation {
    LabelAdd { id: u64 },
    Consent { id: u64, grant: bool },
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct State {
    version: u32,
    target: Target,
    operation: Operation,
    contact_ids: Vec<String>,
    completed: BTreeMap<String, Value>,
    pending: Option<String>,
}

pub fn run(command: BatchCommand) -> Result<()> {
    let (selection, operation) = match command {
        BatchCommand::LabelAdd(a) => (a.selection, Operation::LabelAdd { id: a.label_id }),
        BatchCommand::ConsentGrant(a) => (
            a.selection,
            Operation::Consent {
                id: a.consent_id,
                grant: true,
            },
        ),
        BatchCommand::ConsentRevoke(a) => (
            a.selection,
            Operation::Consent {
                id: a.consent_id,
                grant: false,
            },
        ),
    };
    let result = execute(&selection, operation)?;
    println!("{}", serde_json::to_string_pretty(&result)?);
    Ok(())
}

fn execute(selection: &Selection, operation: Operation) -> Result<Value> {
    let api = selection.connection.open()?;
    ensure!(
        api.target().profile_id == selection.profile_id,
        "requested profile does not match saved session; select it first"
    );
    // Hold a scenario source lock until batch completion so cleanup cannot race these writes.
    let _source_lock = selection
        .scenario_state
        .as_ref()
        .map(|p| crate::scenarios::lock_state(p))
        .transpose()?;
    let _lock = crate::scenarios::lock_state(&selection.journal)?;
    let mut contact_ids = if let Some(path) = &selection.scenario_state {
        crate::scenarios::recorded_contacts(path, &api.target())?
    } else {
        selection
            .contact_id
            .iter()
            .map(ToString::to_string)
            .collect()
    };
    contact_ids.sort();
    contact_ids.dedup();
    ensure!(
        !contact_ids.is_empty() && contact_ids.len() <= 10000,
        "select 1..10000 explicit or recorded contacts"
    );
    let mut state = match fs::read(&selection.journal) {
        Ok(data) => {
            let state: State = serde_json::from_slice(&data)
                .context("invalid batch journal; do not discard it to retry")?;
            ensure!(
                state.version == 1
                    && state.target == api.target()
                    && state.operation == operation
                    && state.contact_ids == contact_ids,
                "journal belongs to another target, operation or contact selection; use its original command or a separate journal"
            );
            ensure!(
                state.completed.keys().all(|id| contact_ids.contains(id)),
                "journal contains an unexpected completed contact ID"
            );
            state
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => State {
            version: 1,
            target: api.target(),
            operation: operation.clone(),
            contact_ids: contact_ids.clone(),
            completed: BTreeMap::new(),
            pending: None,
        },
        Err(e) => return Err(e).context("cannot read batch journal"),
    };
    ensure!(
        state.pending.is_none(),
        "contact {:?} has an uncertain outcome; inspect Gecko and reconcile this journal before retrying",
        state.pending
    );
    if state.completed.len() == contact_ids.len() {
        return Ok(report(&state, selection.execute, &[]));
    }
    operation.validate_destination(&api)?;
    // Validate every remaining contact before any write, then re-read at its execution boundary.
    let mut preview = Vec::new();
    for id in contact_ids
        .iter()
        .filter(|id| !state.completed.contains_key(*id))
    {
        let contact = operation.read_contact(&api, id)?;
        preview.push(json!({"contact_id":id,"change_required":!operation.satisfied(&contact)?}));
    }
    save(selection, &state)?;
    if !selection.execute {
        return Ok(report(&state, false, &preview));
    }
    eprintln!(
        "Executing {} for {} explicit contacts; journal {}",
        serde_json::to_string(&operation)?,
        contact_ids.len(),
        selection.journal.display()
    );
    for id in &contact_ids {
        if state.completed.contains_key(id) {
            continue;
        }
        let current = operation.read_contact(&api, id)?;
        if !operation.satisfied(&current)? {
            state.pending = Some(id.clone());
            save(selection, &state)?;
            operation.apply(&api, id).with_context(|| {
                format!("contact {id} outcome is uncertain; pending journal entry retained")
            })?;
            let after = operation.read_contact(&api, id)?;
            ensure!(
                operation.satisfied(&after)?,
                "contact {id} did not confirm the requested result; outcome remains pending"
            );
        }
        state.completed.insert(id.clone(), json!({"verified":true}));
        state.pending = None;
        save(selection, &state)?;
    }
    Ok(report(&state, true, &preview))
}
fn save(selection: &Selection, state: &State) -> Result<()> {
    crate::storage::write_private_atomic(&selection.journal, &serde_json::to_vec_pretty(state)?)
}
fn report(state: &State, executed: bool, preview: &[Value]) -> Value {
    json!({"executed":executed,"target":state.target,"operation":state.operation,"contact_ids":state.contact_ids,"completed":state.completed.len(),"pending":state.pending,"preview":preview})
}

impl Operation {
    fn validate_destination(&self, api: &GeckoApi) -> Result<()> {
        let (endpoint, singular, id) = match self {
            Self::LabelAdd { id } => ("labels", "label", id),
            Self::Consent { id, .. } => ("consents", "consent", id),
        };
        let payload = api.request(Method::GET, &format!("{endpoint}/{id}"), &[], None)?;
        let object = single(&payload, singular)?;
        ensure!(
            resource_id(object)? == id.to_string(),
            "destination response has an unexpected ID"
        );
        Ok(())
    }
    fn read_contact(&self, api: &GeckoApi, id: &str) -> Result<Value> {
        let payload = api.request(
            Method::GET,
            &format!("contacts/{id}"),
            &[
                ("contact_rfields", "id,consent_data".into()),
                ("include", "labels:1000".into()),
                ("label_rfields", "id".into()),
            ],
            None,
        )?;
        let contact = single(&payload, "contact")?;
        ensure!(
            resource_id(contact)? == id,
            "contact response has an unexpected ID"
        );
        Ok(contact.clone())
    }
    fn satisfied(&self, contact: &Value) -> Result<bool> {
        match self {
            Self::LabelAdd { id } => {
                let labels = contact["labels"]
                    .as_array()
                    .context("contact labels are not visible")?;
                ensure!(
                    labels.len() < 1000,
                    "labels reached relation limit; cannot verify completeness"
                );
                let ids: BTreeSet<_> = labels.iter().map(resource_id).collect::<Result<_>>()?;
                Ok(ids.contains(&id.to_string()))
            }
            Self::Consent { id, grant } => {
                let data = contact["consent_data"]
                    .as_array()
                    .context("contact consent data is not visible")?;
                let entries: Vec<_> = data
                    .iter()
                    .filter(|entry| {
                        resource_id(&json!({"id":entry["consent"]})).ok().as_deref()
                            == Some(&id.to_string())
                    })
                    .collect();
                ensure!(
                    entries.len() == 1,
                    "contact does not expose exactly one entry for consent {id}"
                );
                let value = &entries[0]["granted"];
                ensure!(
                    value.is_boolean() || value.is_null(),
                    "invalid consent grant value"
                );
                Ok(value.as_bool().unwrap_or(false) == *grant)
            }
        }
    }
    fn apply(&self, api: &GeckoApi, contact_id: &str) -> Result<()> {
        let action = match self {
            Self::LabelAdd { id } => {
                json!({"type":"assign_contact_label","to":[id]})
            }
            Self::Consent { id, grant } => {
                json!({"type":"manage_consent","operator":if *grant {"add"} else {"remove"},"to":[id]})
            }
        };
        let payload = api.request(
            Method::POST,
            "contacts/mass_action",
            &[],
            Some(&json!({"conditions":{"contact_ids":[contact_id]},"actions":[action]})),
        )?;
        ensure!(
            payload.get("queue_id").is_none_or(Value::is_null),
            "Gecko queued this action; reconcile pending contact after it finishes"
        );
        Ok(())
    }
}

pub fn single<'a>(payload: &'a Value, singular: &str) -> Result<&'a Value> {
    let resource = payload
        .get(singular)
        .or_else(|| payload.get("data"))
        .unwrap_or(payload);
    if let Some(items) = resource.as_array() {
        ensure!(items.len() == 1, "expected one {singular}");
        Ok(&items[0])
    } else {
        ensure!(resource.is_object(), "expected a {singular} object");
        Ok(resource)
    }
}

#[cfg(test)]
mod tests;
