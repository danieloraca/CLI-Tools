use crate::api::normalize_base_url;
use crate::auth::TokenSet;
use crate::session::AppSession;
use anyhow::{Context, Result, anyhow};
use reqwest::blocking::Client;
use serde_json::Value;

pub const DEFAULT_APP_API_URL: &str = "https://api-stage.geckoform.com";
const CONTACT_RFIELDS: &str = "field_1,field_2,field_3,field_4,field_5,field_6";
const LABEL_RFIELDS: &str = "color,name";

#[derive(Debug, Clone)]
pub struct ContactService {
    base_url: String,
    http: Client,
}

impl ContactService {
    pub fn new(base_url: impl Into<String>) -> Result<Self> {
        let base_url = normalize_base_url(base_url.into())?;
        let http = Client::builder()
            .user_agent("gecko-cli/0.1.0")
            .build()
            .context("failed to build HTTP client")?;

        Ok(Self { base_url, http })
    }

    pub fn list_contacts(
        &self,
        tokens: &TokenSet,
        session: &AppSession,
        page: u32,
        per_page: u32,
    ) -> Result<ContactsPage> {
        let response = self
            .http
            .get(format!("{}/contacts", self.base_url))
            .header("Accept", "application/json")
            .header("Gecko-Account", &session.account_id)
            .header("Gecko-User", &session.user_id)
            .bearer_auth(&tokens.access_token)
            .query(&[
                ("contact_rfields", CONTACT_RFIELDS.to_string()),
                ("label_rfields", LABEL_RFIELDS.to_string()),
                ("per_page", per_page.to_string()),
                ("page", page.to_string()),
                ("include", "labels".to_string()),
            ])
            .send()
            .context("contacts API request failed")?;

        let status = response.status();
        let payload: Value = response.json().with_context(|| {
            format!("contacts API returned non-JSON response with status {status}")
        })?;

        if !status.is_success() {
            return Err(
                api_error(&payload).unwrap_or_else(|| anyhow!("contacts API returned {status}"))
            );
        }

        parse_contacts_page(payload, page, per_page)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContactsPage {
    pub contacts: Vec<ContactRow>,
    pub page: u32,
    pub per_page: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContactRow {
    pub id: String,
    pub full_name: String,
    pub email: String,
    pub last_chat_message: String,
    pub created_at: String,
    pub phone: String,
    pub labels: Vec<String>,
}

pub fn parse_contacts_page(payload: Value, page: u32, per_page: u32) -> Result<ContactsPage> {
    let contacts = contact_items(&payload)
        .context("contacts API response did not include a contacts array")?
        .iter()
        .filter_map(Value::as_object)
        .map(ContactRow::from_object)
        .collect();

    Ok(ContactsPage {
        contacts,
        page,
        per_page,
    })
}

pub fn render_contacts_page(page: &ContactsPage) -> String {
    if page.contacts.is_empty() {
        return format!("Contacts page {}: no contacts found.", page.page);
    }

    let rows = page
        .contacts
        .iter()
        .map(|contact| {
            vec![
                contact.id.clone(),
                contact.full_name.clone(),
                contact.email.clone(),
                contact.last_chat_message.clone(),
                contact.created_at.clone(),
                contact.phone.clone(),
                contact.labels.join(", "),
            ]
        })
        .collect::<Vec<_>>();

    render_table(
        &[
            "ID",
            "Full name",
            "Email",
            "Last chat",
            "Created",
            "Phone",
            "Labels",
        ],
        &rows,
    )
}

impl ContactRow {
    fn from_object(object: &serde_json::Map<String, Value>) -> Self {
        Self {
            id: first_string(object, &["id", "contact_id", "ContactId"]).unwrap_or_default(),
            full_name: first_string(object, &["full_name", "field_1", "name"]).unwrap_or_default(),
            email: first_string(object, &["email", "field_2"]).unwrap_or_default(),
            last_chat_message: first_string(object, &["last_chat_message", "field_3"])
                .unwrap_or_default(),
            created_at: first_string(object, &["created_at", "field_4"]).unwrap_or_default(),
            phone: first_string(object, &["telephone", "phone", "field_5", "field_6"])
                .unwrap_or_default(),
            labels: parse_labels(object.get("labels")),
        }
    }
}

fn contact_items(payload: &Value) -> Option<&Vec<Value>> {
    if let Value::Array(items) = payload {
        return Some(items);
    }

    let object = payload.as_object()?;
    for key in ["data", "contacts", "items", "results"] {
        if let Some(Value::Array(items)) = object.get(key) {
            return Some(items);
        }
    }

    None
}

fn first_string(object: &serde_json::Map<String, Value>, keys: &[&str]) -> Option<String> {
    keys.iter()
        .filter_map(|key| object.get(*key))
        .find_map(display_value)
        .filter(|value| !value.trim().is_empty())
}

fn display_value(value: &Value) -> Option<String> {
    match value {
        Value::String(value) => Some(value.clone()),
        Value::Number(value) => Some(value.to_string()),
        Value::Bool(value) => Some(value.to_string()),
        _ => None,
    }
}

fn parse_labels(value: Option<&Value>) -> Vec<String> {
    let Some(Value::Array(labels)) = value else {
        return Vec::new();
    };

    labels
        .iter()
        .filter_map(|label| label.as_object())
        .filter_map(|label| first_string(label, &["name", "Name"]))
        .collect()
}

fn render_table(headers: &[&str], rows: &[Vec<String>]) -> String {
    let widths = headers
        .iter()
        .enumerate()
        .map(|(index, header)| {
            rows.iter()
                .filter_map(|row| row.get(index))
                .map(|value| value.chars().count().min(40))
                .chain(std::iter::once(header.chars().count()))
                .max()
                .unwrap_or(0)
        })
        .collect::<Vec<_>>();

    let mut output = String::new();
    output.push_str(&render_table_row(headers, &widths));
    output.push('\n');
    output.push_str(
        &widths
            .iter()
            .map(|width| "-".repeat(*width))
            .collect::<Vec<_>>()
            .join("  "),
    );

    for row in rows {
        output.push('\n');
        let cells = row
            .iter()
            .map(|value| truncate(value, 40))
            .collect::<Vec<_>>();
        output.push_str(&render_table_row(&cells, &widths));
    }

    output
}

fn render_table_row<T: AsRef<str>>(values: &[T], widths: &[usize]) -> String {
    values
        .iter()
        .enumerate()
        .map(|(index, value)| format!("{:<width$}", value.as_ref(), width = widths[index]))
        .collect::<Vec<_>>()
        .join("  ")
}

fn truncate(value: &str, max: usize) -> String {
    if value.chars().count() <= max {
        return value.to_string();
    }

    let mut truncated = value
        .chars()
        .take(max.saturating_sub(3))
        .collect::<String>();
    truncated.push_str("...");
    truncated
}

fn api_error(payload: &Value) -> Option<anyhow::Error> {
    payload
        .get("Error")
        .or_else(|| payload.get("Message"))
        .or_else(|| payload.get("message"))
        .and_then(Value::as_str)
        .map(|message| anyhow!(message.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_contacts_from_data_payload() {
        let page = parse_contacts_page(
            json!({
                "data": [
                    {
                        "id": 7,
                        "field_1": "Peter Tester",
                        "field_2": "peter@example.com",
                        "field_3": "30th Apr 2026 at 11:56",
                        "field_4": "30th Apr 2026 at 11:44",
                        "field_5": "+4412345",
                        "labels": [{ "name": "bigbang" }]
                    }
                ]
            }),
            1,
            15,
        )
        .unwrap();

        assert_eq!(page.contacts.len(), 1);
        assert_eq!(page.contacts[0].id, "7");
        assert_eq!(page.contacts[0].full_name, "Peter Tester");
        assert_eq!(page.contacts[0].email, "peter@example.com");
        assert_eq!(page.contacts[0].last_chat_message, "30th Apr 2026 at 11:56");
        assert_eq!(page.contacts[0].phone, "+4412345");
        assert_eq!(page.contacts[0].labels, vec!["bigbang"]);
    }

    #[test]
    fn parses_contacts_from_raw_array_payload() {
        let page = parse_contacts_page(
            json!([
                {
                    "id": "c-1",
                    "full_name": "Roger Rabbit",
                    "email": "roger@example.com",
                    "created_at": "27th Apr 2026 at 14:11"
                }
            ]),
            2,
            15,
        )
        .unwrap();

        assert_eq!(page.page, 2);
        assert_eq!(page.contacts[0].full_name, "Roger Rabbit");
        assert_eq!(page.contacts[0].created_at, "27th Apr 2026 at 14:11");
    }

    #[test]
    fn renders_contacts_table() {
        let rendered = render_contacts_page(&ContactsPage {
            page: 1,
            per_page: 15,
            contacts: vec![ContactRow {
                id: "1".to_string(),
                full_name: "Steven Murphy".to_string(),
                email: "steven@example.com".to_string(),
                last_chat_message: "27th Apr 2026 at 14:10".to_string(),
                created_at: "27th Apr 2026 at 14:10".to_string(),
                phone: String::new(),
                labels: vec!["bigbang".to_string()],
            }],
        });

        assert!(rendered.contains("Full name"));
        assert!(rendered.contains("Last chat"));
        assert!(rendered.contains("Steven Murphy"));
        assert!(rendered.contains("bigbang"));
    }
}
