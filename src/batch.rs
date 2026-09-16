use crate::{
    catalog::Connection,
    gecko::{GeckoApi, Target, resource_id},
};
use anyhow::{Context, Result, ensure};
use clap::{Args, Subcommand, ValueEnum};
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
#[derive(Debug, Args)]
pub struct OrganisationArgs {
    #[command(flatten)]
    pub selection: Selection,
    #[arg(long, value_parser=clap::value_parser!(u64).range(1..))]
    pub organisation_id: u64,
}
#[derive(Debug, Args)]
pub struct EventArgs {
    #[command(flatten)]
    pub selection: Selection,
    #[arg(long, value_parser=clap::value_parser!(u64).range(1..))]
    pub event_id: u64,
    /// Status for a new/reactivated attendance. Existing active attendance is retained.
    #[arg(long, value_enum, default_value = "registered")]
    pub status: AttendanceStatus,
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, ValueEnum)]
#[serde(rename_all = "snake_case")]
pub enum AttendanceStatus {
    Registered,
    Invited,
    Attended,
    Waitlisted,
}
impl AttendanceStatus {
    fn code(self) -> u64 {
        match self {
            Self::Registered => 10,
            Self::Invited => 20,
            Self::Attended => 30,
            Self::Waitlisted => 50,
        }
    }
}
#[derive(Debug, Subcommand)]
pub enum BatchCommand {
    /// Add contacts to an existing organisation, preserving existing memberships.
    OrganisationAdd(OrganisationArgs),
    /// Add contacts to an existing event and report actual attendance status.
    EventAdd(EventArgs),
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
    Organisation { id: u64 },
    Event { id: u64, status: AttendanceStatus },
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
        BatchCommand::OrganisationAdd(a) => (
            a.selection,
            Operation::Organisation {
                id: a.organisation_id,
            },
        ),
        BatchCommand::EventAdd(a) => (
            a.selection,
            Operation::Event {
                id: a.event_id,
                status: a.status,
            },
        ),
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
        preview.push(json!({"contact_id":id,"change_required":!operation.satisfied(&contact)?,"current":operation.outcome(&contact)?}));
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
        let mut current = operation.read_contact(&api, id)?;
        if !operation.satisfied(&current)? {
            state.pending = Some(id.clone());
            save(selection, &state)?;
            let created_id = operation.apply(&api, id).with_context(|| {
                format!("contact {id} outcome is uncertain; pending journal entry retained")
            })?;
            current = operation.read_contact(&api, id)?;
            if let Some(created_id) = created_id {
                ensure!(
                    resource_id(&current["membership"])? == created_id,
                    "membership readback differs from the mutation response; outcome remains pending"
                );
            }
            ensure!(
                operation.satisfied(&current)?,
                "contact {id} did not confirm the requested result; outcome remains pending"
            );
        }
        state
            .completed
            .insert(id.clone(), operation.outcome(&current)?);
        state.pending = None;
        save(selection, &state)?;
    }
    Ok(report(&state, true, &preview))
}
fn save(selection: &Selection, state: &State) -> Result<()> {
    crate::storage::write_private_atomic(&selection.journal, &serde_json::to_vec_pretty(state)?)
}
fn report(state: &State, executed: bool, preview: &[Value]) -> Value {
    json!({"executed":executed,"target":state.target,"operation":state.operation,"contact_ids":state.contact_ids,"completed":state.completed.len(),"results":state.completed,"pending":state.pending,"preview":preview})
}

impl Operation {
    fn validate_destination(&self, api: &GeckoApi) -> Result<()> {
        let (endpoint, singular, id) = match self {
            Self::Organisation { id } => ("organisations", "organisation", id),
            Self::Event { id, .. } => ("events", "event", id),
            Self::LabelAdd { id } => ("labels", "label", id),
            Self::Consent { id, .. } => ("consents", "consent", id),
        };
        let payload = api.request(Method::GET, &format!("{endpoint}/{id}"), &[], None)?;
        let object = single(&payload, singular)?;
        ensure!(
            resource_id(object)? == id.to_string(),
            "destination response has an unexpected ID"
        );
        if matches!(self, Self::Event { .. }) {
            let kind = object["type"]
                .as_u64()
                .or_else(|| object["type"].as_str().and_then(|s| s.parse().ok()))
                .context("event type is missing; cannot confirm registration destination")?;
            ensure!(
                kind != 20,
                "select a session-time ID, not a session container; use events list to inspect type and parent_id"
            );
            ensure!(
                matches!(kind, 10 | 30 | 90 | 100),
                "unsupported event type {kind}; cannot confirm registration destination"
            );
        }
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
        let mut contact = contact.clone();
        if let Some((endpoint, foreign_key, destination)) = self.membership_query() {
            let memberships = api.collection(
                endpoint,
                &[
                    ("contact_id", id.into()),
                    (foreign_key, destination.to_string()),
                ],
            )?;
            ensure!(
                memberships.len() <= 1,
                "multiple memberships returned for one contact/destination"
            );
            for membership in &memberships {
                ensure!(
                    resource_id(&json!({"id":membership["contact_id"]}))? == id
                        && resource_id(&json!({"id":membership[foreign_key]}))?
                            == destination.to_string(),
                    "membership response contains a foreign contact or destination"
                );
            }
            contact["membership"] = memberships.into_iter().next().unwrap_or(Value::Null);
        }
        Ok(contact)
    }
    fn membership_query(&self) -> Option<(&'static str, &'static str, u64)> {
        match self {
            Self::Organisation { id } => Some(("enrolments", "organisation_id", *id)),
            Self::Event { id, .. } => Some(("attendances", "event_id", *id)),
            _ => None,
        }
    }
    fn outcome(&self, contact: &Value) -> Result<Value> {
        let membership = &contact["membership"];
        match self {
            Self::Organisation { .. } if !membership.is_null() => {
                Ok(json!({"member":true,"membership_id":resource_id(membership)?}))
            }
            Self::Event { status, .. } if !membership.is_null() => {
                let actual = attendance_status(membership)?;
                Ok(
                    json!({"member":self.satisfied(contact)?,"membership_id":resource_id(membership)?,"status":actual,"status_title":status_title(actual),"requested_status":status.code(),"matches_requested":actual==status.code()}),
                )
            }
            Self::Organisation { .. } | Self::Event { .. } => Ok(json!({"member":false})),
            _ => Ok(json!({"verified":self.satisfied(contact)?})),
        }
    }
    fn satisfied(&self, contact: &Value) -> Result<bool> {
        match self {
            Self::Organisation { .. } => Ok(!contact["membership"].is_null()),
            Self::Event { .. } => {
                let membership = &contact["membership"];
                Ok(!membership.is_null()
                    && matches!(attendance_status(membership)?, 10 | 15 | 20 | 30 | 40 | 50))
            }
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
    fn apply(&self, api: &GeckoApi, contact_id: &str) -> Result<Option<String>> {
        if let Self::Organisation { id } = self {
            let payload = api.request(
                Method::POST,
                &format!("organisations/{id}/add_contact/{contact_id}"),
                &[],
                None,
            )?;
            return Ok(Some(resource_id(single(&payload, "enrolment")?)?));
        }
        if let Self::Event { id, status } = self {
            let payload = api.request(
                Method::POST,
                &format!("contacts/{contact_id}/attend"),
                &[],
                Some(&json!({"event_id":id,"status":status.code()})),
            )?;
            return Ok(Some(resource_id(single(&payload, "attendance")?)?));
        }
        let action = match self {
            Self::Organisation { .. } | Self::Event { .. } => unreachable!(),
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
        Ok(None)
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

fn attendance_status(membership: &Value) -> Result<u64> {
    let status = membership["status"]
        .as_u64()
        .or_else(|| membership["status"].as_str().and_then(|s| s.parse().ok()))
        .context("attendance status is missing")?;
    ensure!(
        matches!(status, 10 | 15 | 20 | 30 | 40 | 50 | 80 | 90 | 100),
        "unknown attendance status {status}"
    );
    Ok(status)
}
fn status_title(status: u64) -> &'static str {
    match status {
        10 => "Registered",
        15 => "Payment Pending",
        20 => "Invited",
        30 => "Attended",
        40 => "Engaged",
        50 => "Waitlisted",
        80 => "Removed",
        90 => "Cancelled",
        100 => "Did Not Attend",
        _ => "Unknown",
    }
}
