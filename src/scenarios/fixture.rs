use super::GenerateArgs;
use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Scenario {
    pub schema_version: u32,
    pub name: String,
    pub seed: u64,
    pub fields: Vec<Field>,
    pub contacts: Vec<Contact>,
    pub groups: Vec<Group>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Field {
    pub key: String,
    pub label: String,
    #[serde(rename = "type")]
    pub kind: FieldType,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum FieldType {
    Name,
    Email,
    Text,
    Number,
    Textarea,
}

impl FieldType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Name => "name",
            Self::Email => "email",
            Self::Text => "text",
            Self::Number => "number",
            Self::Textarea => "textarea",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Contact {
    pub key: String,
    pub fields: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Group {
    pub key: String,
    pub name: String,
    pub permissions: Vec<String>,
}

impl Scenario {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.schema_version == 1,
            "unsupported scenario schema_version {}; expected 1",
            self.schema_version
        );
        ensure!(
            !self.name.is_empty()
                && self.name.len() <= 24
                && self
                    .name
                    .bytes()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-'),
            "scenario name must contain 1–24 lowercase letters, digits or hyphens"
        );
        ensure!(
            (1..=10_000).contains(&self.contacts.len()),
            "contacts must be between 1 and 10000"
        );
        ensure!(
            (2..=22).contains(&self.fields.len()),
            "a scenario needs name and email fields and at most 20 custom fields"
        );
        let mut keys = BTreeSet::new();
        for field in &self.fields {
            ensure!(
                valid_key(&field.key) && keys.insert(field.key.as_str()),
                "invalid or duplicate field key: {}",
                field.key
            );
            ensure!(
                !field.label.trim().is_empty() && field.label.chars().count() <= 100,
                "field label must contain 1–100 characters"
            );
            ensure!(
                match field.kind {
                    FieldType::Name => field.key == "name",
                    FieldType::Email => field.key == "email",
                    _ => field.key != "name" && field.key != "email",
                },
                "name and email are reserved field keys and types"
            );
        }
        ensure!(
            keys.contains("name") && keys.contains("email"),
            "name and email fields are required"
        );
        let mut contact_keys = BTreeSet::new();
        for contact in &self.contacts {
            ensure!(
                valid_key(&contact.key) && contact_keys.insert(&contact.key),
                "invalid or duplicate contact key: {}",
                contact.key
            );
            ensure!(
                contact
                    .fields
                    .keys()
                    .map(String::as_str)
                    .collect::<BTreeSet<_>>()
                    == keys,
                "contact {} must include exactly the declared fields (use null for missing values)",
                contact.key
            );
            for field in &self.fields {
                let value = &contact.fields[&field.key];
                let valid = match field.kind {
                    FieldType::Name => value.as_object().is_some_and(|name| {
                        name.len() == 2
                            && ["first_name", "last_name"].iter().all(|key| {
                                name.get(*key).and_then(Value::as_str).is_some_and(|s| {
                                    !s.trim().is_empty() && s.chars().count() <= 100
                                })
                            })
                    }),
                    FieldType::Email => {
                        value.is_null()
                            || value.as_str().is_some_and(|s| {
                                s.len() <= 254
                                    && s.split_once('@').is_some_and(|(local, domain)| {
                                        !local.is_empty()
                                            && !local.contains(char::is_whitespace)
                                            && domain == "example.test"
                                    })
                            })
                    }
                    FieldType::Number => {
                        value.is_null()
                            || value
                                .as_i64()
                                .is_some_and(|n| (-2_147_483_647..=2_147_483_647).contains(&n))
                    }
                    FieldType::Text => {
                        value.is_null() || value.as_str().is_some_and(|s| s.chars().count() <= 1500)
                    }
                    FieldType::Textarea => {
                        value.is_null()
                            || value.as_str().is_some_and(|s| s.chars().count() <= 65535)
                    }
                };
                ensure!(
                    valid,
                    "invalid {} value for contact {}, field {} (emails must use example.test)",
                    field.kind.as_str(),
                    contact.key,
                    field.key
                );
                if matches!(
                    field.kind,
                    FieldType::Number | FieldType::Text | FieldType::Textarea
                ) {
                    ensure!(
                        value != 0
                            && !value
                                .as_str()
                                .is_some_and(|value| matches!(value.trim(), "" | "0")),
                        "contact {}, field {} contains a value Gecko contact writes silently ignore (0, \"0\" or an empty string); use null for a missing value",
                        contact.key,
                        field.key
                    );
                }
            }
        }
        ensure!(
            self.groups.len() <= 1,
            "at most one restricted group is supported"
        );
        for group in &self.groups {
            ensure!(
                valid_key(&group.key) && (2..=40).contains(&group.name.chars().count()),
                "invalid group key or name"
            );
            ensure!(
                group.permissions == ["contacts_view"],
                "restricted groups must grant only contacts_view; Gecko also adds mandatory permissions"
            );
        }
        Ok(())
    }
}

fn valid_key(key: &str) -> bool {
    !key.is_empty()
        && key.len() <= 32
        && key.as_bytes()[0].is_ascii_lowercase()
        && key
            .bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'_')
}

#[derive(Default)]
struct Request {
    contacts: Option<usize>,
    duplicates: Option<usize>,
    missing: Option<usize>,
    custom_fields: bool,
    restricted: bool,
}

// A deliberately small grammar: unknown clauses fail, so requests never silently lose requirements.
fn parse_request(request: &str) -> Result<Request> {
    let request = request.trim().trim_end_matches('.').to_lowercase();
    let request = request
        .strip_prefix("create a profile with ")
        .or_else(|| request.strip_prefix("create a scenario with "))
        .unwrap_or(&request);
    let (count, rest) = request
        .split_once(" contacts")
        .context("request must start with 'create a profile with N contacts' or 'N contacts'")?;
    let count = count
        .parse::<usize>()
        .context("invalid contact count in request")?;
    ensure!(
        (1..=10_000).contains(&count),
        "contacts must be between 1 and 10000"
    );
    let mut result = Request {
        contacts: Some(count),
        ..Request::default()
    };
    let clauses = rest.replace(" and ", ",");
    for clause in clauses
        .trim_start_matches(" with ")
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        match clause {
            "duplicate emails" => result.duplicates = Some((count / 10).max(1)),
            "missing emails" => result.missing = Some((count / 20).max(1)),
            "custom fields" => result.custom_fields = true,
            "restricted permissions" => result.restricted = true,
            _ => {
                if let Some(n) = clause.strip_suffix(" duplicate emails") {
                    result.duplicates = Some(n.parse().context("invalid duplicate email count")?);
                } else if let Some(n) = clause.strip_suffix(" missing emails") {
                    result.missing = Some(n.parse().context("invalid missing email count")?);
                } else {
                    bail!(
                        "unsupported request clause: {clause:?}; supported: duplicate emails, missing emails, custom fields, restricted permissions (use flags for explicit counts and field types)"
                    );
                }
            }
        }
    }
    Ok(result)
}

pub fn generate(args: &GenerateArgs) -> Result<Scenario> {
    let request = args
        .request
        .as_deref()
        .map(parse_request)
        .transpose()?
        .unwrap_or_default();
    let count = args.contacts.or(request.contacts).unwrap_or(100);
    let duplicates = args.duplicate_emails.or(request.duplicates).unwrap_or(0);
    let missing = args.missing_emails.or(request.missing).unwrap_or(0);
    ensure!(
        (1..=10_000).contains(&count),
        "contacts must be between 1 and 10000"
    );
    ensure!(
        missing <= count,
        "missing emails cannot exceed the contact count"
    );
    let with_email = count - missing;
    ensure!(
        duplicates == 0 || duplicates < with_email,
        "duplicate emails require at least one unique, non-missing email"
    );
    let unique = with_email - duplicates;
    let mut fields = vec![
        Field {
            key: "name".into(),
            label: "Full name".into(),
            kind: FieldType::Name,
        },
        Field {
            key: "email".into(),
            label: "Email address".into(),
            kind: FieldType::Email,
        },
    ];
    let mut custom = Vec::new();
    if args.custom_fields || request.custom_fields {
        custom.extend([
            "course:text".to_owned(),
            "cohort:number".to_owned(),
            "access_notes:textarea".to_owned(),
        ]);
    }
    custom.extend(args.custom_field.iter().cloned());
    ensure!(custom.len() <= 20, "at most 20 custom fields are supported");
    for spec in custom {
        let (key, kind) = spec
            .split_once(':')
            .context("custom fields must use KEY:TYPE, e.g. course:text")?;
        let kind = match kind {
            "text" => FieldType::Text,
            "number" => FieldType::Number,
            "textarea" => FieldType::Textarea,
            _ => bail!("unsupported custom field type {kind:?}; choose text, number or textarea"),
        };
        ensure!(
            valid_key(key) && !fields.iter().any(|field| field.key == key),
            "invalid, reserved or duplicate field key: {key}"
        );
        fields.push(Field {
            key: key.into(),
            label: key.replace('_', " "),
            kind,
        });
    }
    let mut random = Random(args.seed);
    let names = [
        ("Amara", "Okafor"),
        ("Daniel", "O'Neill"),
        ("Sofía", "García"),
        ("Wei", "Zhang"),
        ("Maya", "Patel"),
        ("Zoë", "Martin"),
        ("Noor", "Al-Hassan"),
        ("Renée", "Dubois"),
        ("Jürgen", "Müller"),
        ("Ava", "Smith-Jones"),
        ("李", "明"),
        ("Alex", "Taylor"),
    ];
    let courses = [
        "Computer Science",
        "History",
        "Biomedical Sciences",
        "Fine Art",
        "Mathematics",
        "Law",
    ];
    let contacts = (0..count)
        .map(|index| {
            let (first, last) = names[random.index(names.len())];
            let mut values = BTreeMap::new();
            values.insert(
                "name".into(),
                json!({"first_name": first, "last_name": last}),
            );
            values.insert(
                "email".into(),
                if index >= with_email {
                    Value::Null
                } else {
                    let email_index = if index < unique {
                        index
                    } else {
                        (index - unique) % unique
                    };
                    json!(format!(
                        "{}.{}.{:05}@example.test",
                        args.name,
                        args.seed,
                        email_index + 1
                    ))
                },
            );
            for field in fields.iter().skip(2) {
                let value = match field.kind {
                    FieldType::Text => json!(courses[random.index(courses.len())]),
                    FieldType::Number => json!(2024 + random.index(6)),
                    FieldType::Textarea => json!(format!(
                        "Development contact {:05}\nPrefers {}. Accessibility notes: {}.",
                        index + 1,
                        ["email", "a campus visit", "online appointments"][random.index(3)],
                        [
                            "step-free access",
                            "large-print materials",
                            "a quiet room",
                            "none requested"
                        ][random.index(4)]
                    )),
                    _ => unreachable!(),
                };
                values.insert(field.key.clone(), value);
            }
            Contact {
                key: format!("contact_{:05}", index + 1),
                fields: values,
            }
        })
        .collect();
    let groups = if args.restricted_permissions || request.restricted {
        vec![Group {
            key: "read_only".into(),
            name: format!("{} Read only", args.name),
            permissions: vec!["contacts_view".into()],
        }]
    } else {
        Vec::new()
    };
    let scenario = Scenario {
        schema_version: 1,
        name: args.name.clone(),
        seed: args.seed,
        fields,
        contacts,
        groups,
    };
    scenario.validate()?;
    Ok(scenario)
}

// SplitMix64, fixed here as part of fixture schema v1; independent of library/platform versions.
struct Random(u64);
impl Random {
    fn index(&mut self, length: usize) -> usize {
        self.0 = self.0.wrapping_add(0x9e3779b97f4a7c15);
        let mut value = self.0;
        value = (value ^ (value >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
        value = (value ^ (value >> 27)).wrapping_mul(0x94d049bb133111eb);
        ((value ^ (value >> 31)) % length as u64) as usize
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;
    #[derive(Parser)]
    struct TestCli {
        #[command(flatten)]
        args: GenerateArgs,
    }
    fn args(values: &[&str]) -> GenerateArgs {
        TestCli::try_parse_from(std::iter::once("generate").chain(values.iter().copied()))
            .unwrap()
            .args
    }

    #[test]
    fn request_produces_repeatable_typed_edge_cases() {
        let options = args(&[
            "create a profile with 500 contacts, duplicate emails, custom fields, and restricted permissions.",
        ]);
        let fixture = generate(&options).unwrap();
        assert_eq!(
            serde_json::to_vec(&fixture).unwrap(),
            serde_json::to_vec(&generate(&options).unwrap()).unwrap()
        );
        assert_eq!(fixture.contacts.len(), 500);
        assert_eq!(
            fixture
                .contacts
                .iter()
                .map(|c| c.fields["email"].as_str().unwrap())
                .collect::<BTreeSet<_>>()
                .len(),
            450
        );
        assert_eq!(fixture.fields.len(), 5);
        assert!(fixture.contacts[0].fields["cohort"].is_number());
        assert!(
            fixture.contacts[0].fields["access_notes"]
                .as_str()
                .unwrap()
                .contains('\n')
        );
        assert_eq!(fixture.groups[0].permissions, ["contacts_view"]);
    }

    #[test]
    fn explicit_counts_and_missing_values_are_exact() {
        let fixture = generate(&args(&[
            "--contacts",
            "12",
            "--duplicate-emails",
            "3",
            "--missing-emails",
            "2",
        ]))
        .unwrap();
        let emails = fixture
            .contacts
            .iter()
            .filter_map(|c| c.fields["email"].as_str())
            .collect::<Vec<_>>();
        assert_eq!(emails.len(), 10);
        assert_eq!(emails.iter().collect::<BTreeSet<_>>().len(), 7);
        assert_ne!(
            fixture,
            generate(&args(&["--contacts", "12", "--seed", "43"])).unwrap()
        );
    }

    #[test]
    fn invalid_requests_and_combinations_fail() {
        for input in [
            vec!["500 contacts, unicorns"],
            vec!["--contacts", "0"],
            vec!["--contacts", "10001"],
            vec!["--contacts", "2", "--duplicate-emails", "2"],
            vec!["--contacts", "2", "--missing-emails", "3"],
            vec![
                "--contacts",
                "2",
                "--missing-emails",
                "2",
                "--duplicate-emails",
                "1",
            ],
            vec!["--custom-field", "email:text"],
            vec!["--custom-field", "course:magic"],
            vec!["--custom-field", "x:text", "--custom-field", "x:number"],
            vec!["--name", "invalid name"],
        ] {
            assert!(generate(&args(&input)).is_err(), "{input:?}");
        }
    }

    #[test]
    fn validates_edited_fixtures_before_applying() {
        let fixture = generate(&args(&["--contacts", "1", "--restricted-permissions"])).unwrap();
        let mut edited = fixture.clone();
        edited.schema_version = 2;
        assert!(edited.validate().is_err());
        let mut edited = fixture.clone();
        edited.contacts[0]
            .fields
            .insert("email".into(), json!("real@example.com"));
        assert!(edited.validate().is_err());
        let mut edited = fixture.clone();
        edited.contacts[0]
            .fields
            .insert("unknown".into(), json!(true));
        assert!(edited.validate().is_err());
        let mut edited = fixture;
        edited.groups[0].permissions.push("contacts_delete".into());
        assert!(edited.validate().is_err());
    }

    #[test]
    fn all_missing_emails_and_explicit_request_overrides_work() {
        let fixture = generate(&args(&[
            "5 contacts, 2 duplicate emails, 1 missing emails",
            "--duplicate-emails",
            "0",
            "--missing-emails",
            "5",
        ]))
        .unwrap();
        assert!(fixture.contacts.iter().all(|c| c.fields["email"].is_null()));
    }

    #[test]
    fn schema_v1_generation_matches_the_checked_in_example() {
        let fixture = generate(&args(&[
            "--name",
            "restricted-contacts",
            "--seed",
            "42",
            "--contacts",
            "6",
            "--duplicate-emails",
            "2",
            "--missing-emails",
            "1",
            "--custom-fields",
            "--restricted-permissions",
        ]))
        .unwrap();
        let expected = include_str!("../../examples/scenarios/restricted-contacts.json");
        assert_eq!(
            format!("{}\n", serde_json::to_string_pretty(&fixture).unwrap()),
            expected
        );
    }

    #[test]
    fn rejects_values_that_gecko_silently_ignores() {
        let original = generate(&args(&[
            "--contacts",
            "1",
            "--custom-field",
            "score:number",
            "--custom-field",
            "course:text",
            "--custom-field",
            "notes:textarea",
        ]))
        .unwrap();
        for (field, value) in [
            ("score", json!(0)),
            ("course", json!("0")),
            ("course", json!("")),
            ("course", json!(" 0 ")),
            ("course", json!("   ")),
            ("notes", json!("0")),
            ("notes", json!("")),
            ("notes", json!("\n 0\t")),
            ("notes", json!("\n\t")),
        ] {
            let mut edited = original.clone();
            edited.contacts[0].fields.insert(field.into(), value);
            assert!(
                edited
                    .validate()
                    .unwrap_err()
                    .to_string()
                    .contains("silently ignore")
            );
            edited.contacts[0].fields.insert(field.into(), Value::Null);
            edited.validate().unwrap();
        }
    }
}
