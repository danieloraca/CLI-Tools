use super::{
    apply::{self, ApplyArgs, State},
    fixture::{FieldType, Scenario},
};
use crate::{
    batch::single,
    catalog::Connection,
    gecko::{ApiError, GeckoApi, resource_id},
};
use anyhow::{Result, ensure};
use reqwest::Method;
use serde::Serialize;
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::File,
    path::PathBuf,
};

#[derive(Debug, Serialize)]
pub(super) struct Report {
    target: crate::gecko::Target,
    fixture_sha256: String,
    pub ok: bool,
    pub contacts_checked: usize,
    pub resources_checked: usize,
    pub issues: Vec<Issue>,
    pub expected_emails: EmailCounts,
    pub observed_emails: Option<EmailCounts>,
}
#[derive(Debug, Serialize)]
pub(super) struct Issue {
    resource: String,
    field: Option<String>,
    kind: &'static str,
    reason: String,
}
#[derive(Debug, Serialize, PartialEq)]
pub(super) struct EmailCounts {
    pub duplicates: usize,
    pub missing: usize,
}
impl Report {
    fn issue(&mut self, resource: &str, field: Option<&str>, kind: &'static str, reason: &str) {
        self.issues.push(Issue {
            resource: resource.into(),
            field: field.map(String::from),
            kind,
            reason: reason.into(),
        });
    }
}
pub fn run(args: &ApplyArgs, scenario: &Scenario) -> Result<()> {
    let (api, _, _lock, state) = prepare(args, scenario)?;
    let report = verify(&api, &state, scenario)?;
    println!("{}", serde_json::to_string_pretty(&report)?);
    ensure!(report.ok, "scenario verification failed; see JSON issues");
    Ok(())
}

pub(super) fn prepare(
    args: &ApplyArgs,
    scenario: &Scenario,
) -> Result<(GeckoApi, PathBuf, File, State)> {
    scenario.validate()?;
    let api = Connection {
        app_api_base_url: args.app_api_base_url.clone(),
        app_token_file: args.app_token_file.clone(),
        session_file: args.session_file.clone(),
    }
    .open()?;
    ensure!(
        api.target().profile_id == args.profile_id,
        "requested profile does not match saved session"
    );
    let path = args
        .state_file
        .clone()
        .unwrap_or_else(|| args.fixture.with_extension("apply-state.json"));
    let lock = apply::lock_state(&path)?;
    ensure!(path.is_file(), "matching apply journal does not exist");
    let state = apply::load_state(&path, api.target(), scenario)?;
    validate_checkpoints(&state, scenario)?;
    Ok((api, path, lock, state))
}
pub(super) fn validate_checkpoints(state: &State, scenario: &Scenario) -> Result<()> {
    let mut allowed = BTreeSet::new();
    for field in &scenario.fields {
        if !matches!(field.kind, FieldType::Name | FieldType::Email) {
            allowed.insert(format!("field:{}", field.key));
        }
    }
    for group in &scenario.groups {
        allowed.insert(format!("group:{}", group.key));
    }
    for contact in &scenario.contacts {
        allowed.insert(format!("contact:{}", contact.key));
        allowed.insert(format!("populated:{}", contact.key));
    }
    for key in state.creation.keys() {
        ensure!(
            state.completed.contains_key(key) && !key.starts_with("populated:"),
            "creation evidence has no matching creation checkpoint"
        );
    }
    let mut contacts = BTreeSet::new();
    for (key, id) in &state.completed {
        ensure!(
            allowed.contains(key),
            "journal contains an unexpected checkpoint {key}"
        );
        resource_id(&json!({"id":id}))?;
        if key.starts_with("contact:") {
            ensure!(contacts.insert(id), "journal repeats a created contact ID");
        }
    }
    Ok(())
}

pub(super) fn read_resource(
    api: &GeckoApi,
    endpoint: &str,
    singular: &str,
    id: &str,
    query: &[(&str, String)],
) -> Result<Option<Value>> {
    resource_id(&json!({"id":id}))?;
    let payload = match api.request(Method::GET, &format!("{endpoint}/{id}"), query, None) {
        Ok(payload) => payload,
        Err(error)
            if error
                .downcast_ref::<ApiError>()
                .is_some_and(|e| e.status == 404) =>
        {
            return Ok(None);
        }
        Err(error) => return Err(error),
    };
    let object = single(&payload, singular)?;
    ensure!(
        resource_id(object)? == id,
        "resource response has an unexpected ID"
    );
    Ok(Some(object.clone()))
}

pub(super) fn verify(api: &GeckoApi, state: &State, scenario: &Scenario) -> Result<Report> {
    let expected: Vec<_> = scenario
        .contacts
        .iter()
        .map(|c| c.fields["email"].clone())
        .collect();
    let mut report = Report {
        target: state.target.clone(),
        fixture_sha256: state.fixture_sha256.clone(),
        ok: false,
        contacts_checked: 0,
        resources_checked: 0,
        issues: vec![],
        expected_emails: email_counts(&expected),
        observed_emails: None,
    };
    if state.lifecycle != apply::Lifecycle::Active {
        report.issue(
            "run",
            None,
            "incomplete",
            "cleanup has started for this scenario",
        );
        return Ok(report);
    }
    if let Some(pending) = &state.pending {
        report.issue(
            pending,
            None,
            "uncertain",
            "apply operation has an uncertain outcome",
        );
    }
    let fields = match api.fields() {
        Ok(fields) => fields,
        Err(_) => {
            report.issue(
                "fields",
                None,
                "unverified",
                "contact field metadata could not be read with current access",
            );
            return Ok(report);
        }
    };
    let mut mapped = BTreeMap::new();
    for field in &scenario.fields {
        let key = format!("field:{}", field.key);
        let candidate = if matches!(field.kind, FieldType::Name | FieldType::Email) {
            let candidates: Vec<_> = fields
                .iter()
                .filter(|f| f["type"] == field.kind.as_str())
                .collect();
            if candidates.len() == 1 {
                Some(candidates[0])
            } else {
                None
            }
        } else {
            state.completed.get(&key).and_then(|id| {
                fields
                    .iter()
                    .find(|f| resource_id(f).ok().as_ref() == Some(id))
            })
        };
        let Some(metadata) = candidate else {
            report.issue(
                &key,
                None,
                "unverified",
                "field metadata or creation checkpoint is missing/ambiguous",
            );
            continue;
        };
        let id = resource_id(metadata)?;
        report.resources_checked += 1;
        if metadata["type"] != field.kind.as_str() {
            report.issue(&key, None, "mismatch", "field type differs");
            continue;
        }
        if !matches!(field.kind, FieldType::Name | FieldType::Email)
            && metadata["label"] != format!("{}: {}", scenario.name, field.label)
        {
            report.issue(&key, None, "mismatch", "custom field label differs");
        }
        if !matches!(field.kind, FieldType::Name | FieldType::Email) {
            if metadata.get("required").is_none() || metadata.get("matchable").is_none() {
                report.issue(
                    &key,
                    None,
                    "unverified",
                    "custom field creation settings are unavailable",
                );
            } else if crate::contacts::truthy(metadata.get("required"))
                || crate::contacts::truthy(metadata.get("matchable"))
            {
                report.issue(
                    &key,
                    None,
                    "mismatch",
                    "custom field required/matching settings differ",
                );
            }
        }
        mapped.insert(field.key.clone(), (id, metadata));
    }
    for group in &scenario.groups {
        let key = format!("group:{}", group.key);
        let Some(id) = state.completed.get(&key) else {
            report.issue(&key, None, "incomplete", "group creation is not recorded");
            continue;
        };
        let resource = read_resource(
            api,
            "groups",
            "group",
            id,
            &[
                ("include", "permissions:1000".into()),
                ("permission_rfields", "id,key,always".into()),
            ],
        );
        match resource {
            Ok(Some(resource)) => {
                report.resources_checked += 1;
                if resource["name"] != group.name {
                    report.issue(&key, None, "mismatch", "group name differs");
                }
                if let Some(permissions) = resource["permissions"]
                    .as_array()
                    .filter(|p| p.len() < 1000)
                {
                    let mut actual = BTreeSet::new();
                    let mut unexpected = false;
                    let mut unavailable = false;
                    for p in permissions {
                        if let Some(key) = p["key"].as_str() {
                            actual.insert(key);
                            unexpected |= !group.permissions.iter().any(|g| g == key)
                                && !crate::contacts::truthy(p.get("always"));
                        } else {
                            unavailable = true;
                        }
                    }
                    if unavailable {
                        report.issue(&key, None, "unverified", "permission keys unavailable");
                    } else if unexpected
                        || group
                            .permissions
                            .iter()
                            .any(|p| !actual.contains(p.as_str()))
                    {
                        report.issue(
                            &key,
                            None,
                            "mismatch",
                            "group permissions differ (mandatory Gecko permissions are allowed)",
                        );
                    }
                } else {
                    report.issue(
                        &key,
                        None,
                        "unverified",
                        "complete group permissions are unavailable",
                    );
                }
            }
            Ok(None) => report.issue(&key, None, "missing", "recorded group is missing"),
            Err(_) => report.issue(
                &key,
                None,
                "unverified",
                "group could not be read with current access",
            ),
        }
    }
    let mut emails = Vec::new();
    for contact in &scenario.contacts {
        let key = format!("contact:{}", contact.key);
        let Some(id) = state.completed.get(&key) else {
            report.issue(&key, None, "incomplete", "contact creation is not recorded");
            continue;
        };
        if state.completed.get(&format!("populated:{}", contact.key)) != Some(id) {
            report.issue(
                &key,
                None,
                "incomplete",
                "contact population is not confirmed",
            );
        }
        let actual = match read_resource(
            api,
            "contacts",
            "contact",
            id,
            &[
                ("contact_rfields", "id".into()),
                ("include", "current_values:1000,current_values.field".into()),
            ],
        ) {
            Ok(Some(actual)) => actual,
            Ok(None) => {
                report.issue(&key, None, "missing", "recorded contact is missing");
                continue;
            }
            Err(_) => {
                report.issue(
                    &key,
                    None,
                    "unverified",
                    "contact could not be read with current access",
                );
                continue;
            }
        };
        report.contacts_checked += 1;
        for field in &scenario.fields {
            let Some((field_id, metadata)) = mapped.get(&field.key) else {
                continue;
            };
            let value = match crate::export::custom_value(&actual, field_id, metadata) {
                Ok(value) => value,
                Err(_) => {
                    report.issue(
                        &key,
                        Some(&field.key),
                        "unverified",
                        "complete typed values are unavailable",
                    );
                    continue;
                }
            };
            if crate::contacts::truthy(metadata.get("is_sensitive")) || value == "************" {
                report.issue(
                    &key,
                    Some(&field.key),
                    "unverified",
                    "value is restricted or masked",
                );
                continue;
            }
            if field.kind == FieldType::Email {
                emails.push(value.clone());
            }
            if !equivalent(&contact.fields[&field.key], &value, field.kind) {
                report.issue(
                    &key,
                    Some(&field.key),
                    "mismatch",
                    "value differs (values omitted)",
                );
            }
        }
    }
    if emails.len() == scenario.contacts.len() {
        let observed = email_counts(&emails);
        if observed != report.expected_emails {
            report.issue(
                "contacts",
                Some("email"),
                "mismatch",
                "duplicate/missing email counts differ",
            );
        }
        report.observed_emails = Some(observed);
    }
    report.ok = report.issues.is_empty();
    Ok(report)
}
fn email_counts(values: &[Value]) -> EmailCounts {
    let mut seen = BTreeSet::new();
    let mut result = EmailCounts {
        duplicates: 0,
        missing: 0,
    };
    for value in values {
        if value.is_null() {
            result.missing += 1;
        } else if !seen.insert(value.to_string()) {
            result.duplicates += 1;
        }
    }
    result
}
fn equivalent(expected: &Value, actual: &Value, kind: FieldType) -> bool {
    if kind == FieldType::Number && !expected.is_null() && !actual.is_null() {
        let expected = decimal(&expected.to_string());
        let raw = actual
            .as_str()
            .map(String::from)
            .unwrap_or_else(|| actual.to_string());
        return expected.is_some() && expected == decimal(&raw);
    }
    expected == actual
}
// Exact decimal comparison accommodates database numeric strings without rounding large integers.
fn decimal(input: &str) -> Option<(bool, String, i32)> {
    let mut s = input.trim();
    let negative = s.starts_with('-');
    if s.starts_with(['-', '+']) {
        s = &s[1..];
    }
    let mut exponent = 0;
    if let Some(pos) = s.find(['e', 'E']) {
        exponent = s[pos + 1..].parse::<i32>().ok()?;
        s = &s[..pos];
    }
    let (whole, fraction) = s.split_once('.').unwrap_or((s, ""));
    if whole.is_empty()
        || !whole
            .bytes()
            .chain(fraction.bytes())
            .all(|c| c.is_ascii_digit())
    {
        return None;
    }
    let mut digits = format!("{whole}{fraction}")
        .trim_start_matches('0')
        .to_string();
    exponent = exponent.checked_sub(i32::try_from(fraction.len()).ok()?)?;
    if digits.is_empty() {
        return Some((false, "0".into(), 0));
    }
    while digits.ends_with('0') {
        digits.pop();
        exponent = exponent.checked_add(1)?;
    }
    Some((negative, digits, exponent))
}

#[cfg(test)]
mod tests;
