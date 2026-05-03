use crate::api::normalize_base_url;
use crate::auth::TokenSet;
use crate::session::AppSession;
use anyhow::{Context, Result, anyhow};
use chrono::{DateTime, Datelike, Local, NaiveDateTime, TimeZone};
use reqwest::{blocking::Client, header::HeaderMap};
use serde_json::Value;

const CONTACT_RFIELDS: &str = "field_1,field_2,field_3,field_4,field_5,field_6";
const LABEL_RFIELDS: &str = "color,name";
const HEADER_PREFIX: &str = concat!("Ge", "cko");

#[derive(Debug, Clone)]
pub struct ContactService {
    base_url: String,
    http: Client,
}

impl ContactService {
    pub fn new(base_url: impl Into<String>) -> Result<Self> {
        let base_url = normalize_base_url(base_url.into())?;
        let http = Client::builder()
            .user_agent("cli_tools/0.1.0")
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
            .header(format!("{HEADER_PREFIX}-Account"), &session.account_id)
            .header(format!("{HEADER_PREFIX}-User"), &session.user_id)
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
        let headers = response.headers().clone();
        let payload: Value = response.json().with_context(|| {
            format!("contacts API returned non-JSON response with status {status}")
        })?;

        if !status.is_success() {
            return Err(
                api_error(&payload).unwrap_or_else(|| anyhow!("contacts API returned {status}"))
            );
        }

        let pagination = ContactsPagination::from_response(&headers, &payload, page, per_page);
        parse_contacts_page_with_pagination(payload, pagination)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContactsPage {
    pub contacts: Vec<ContactRow>,
    pub pagination: ContactsPagination,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContactsPagination {
    pub page: u32,
    pub per_page: u32,
    pub total_results: Option<u64>,
    pub total_pages: Option<u32>,
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

#[cfg(test)]
pub fn parse_contacts_page(payload: Value, page: u32, per_page: u32) -> Result<ContactsPage> {
    parse_contacts_page_with_pagination(
        payload,
        ContactsPagination {
            page,
            per_page,
            total_results: None,
            total_pages: None,
        },
    )
}

pub fn parse_contacts_page_with_pagination(
    payload: Value,
    pagination: ContactsPagination,
) -> Result<ContactsPage> {
    let contacts = contact_items(&payload)
        .context("contacts API response did not include a contacts array")?
        .iter()
        .filter_map(Value::as_object)
        .map(ContactRow::from_object)
        .collect();

    Ok(ContactsPage {
        contacts,
        pagination,
    })
}

pub fn render_contacts_page(page: &ContactsPage) -> String {
    if page.contacts.is_empty() {
        return format!("Contacts page {}: no contacts found.", page.pagination.page);
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

impl ContactsPagination {
    fn from_response(
        headers: &HeaderMap,
        payload: &Value,
        fallback_page: u32,
        fallback_per_page: u32,
    ) -> Self {
        let mut pagination = Self::from_headers(headers, fallback_page, fallback_per_page);
        pagination.apply_body_fallback(payload);

        if pagination.total_pages.is_none() {
            pagination.total_pages = pagination
                .total_results
                .map(|total| total.div_ceil(u64::from(pagination.per_page)) as u32);
        }

        pagination
    }

    fn from_headers(headers: &HeaderMap, fallback_page: u32, fallback_per_page: u32) -> Self {
        Self {
            page: header_u32(headers, &["current_page", "current-page", "page"])
                .unwrap_or(fallback_page),
            per_page: header_u32(headers, &["per_page", "per-page", "perpage"])
                .unwrap_or(fallback_per_page),
            total_results: header_u64(
                headers,
                &[
                    "total_results",
                    "total-results",
                    "total_items",
                    "total-items",
                    "total",
                ],
            ),
            total_pages: header_u32(
                headers,
                &["total_pages", "total-pages", "last_page", "last-page"],
            ),
        }
    }

    fn apply_body_fallback(&mut self, payload: &Value) {
        let sources = [
            Some(payload),
            payload.get("pagination"),
            payload.get("meta"),
            payload.get("meta").and_then(|meta| meta.get("pagination")),
        ];

        for source in sources.into_iter().flatten() {
            self.page =
                value_u32(source, &["current_page", "currentPage", "page"]).unwrap_or(self.page);
            self.per_page = value_u32(source, &["per_page", "perPage"]).unwrap_or(self.per_page);
            self.total_results = self.total_results.or_else(|| {
                value_u64(
                    source,
                    &[
                        "total_results",
                        "totalResults",
                        "total_items",
                        "totalItems",
                        "total",
                    ],
                )
            });
            self.total_pages = self.total_pages.or_else(|| {
                value_u32(
                    source,
                    &["total_pages", "totalPages", "last_page", "lastPage"],
                )
            });
        }
    }

    pub fn can_go_previous(self) -> bool {
        self.page > 1
    }

    pub fn can_go_next(self) -> bool {
        self.total_pages
            .map(|total_pages| self.page < total_pages)
            .unwrap_or(true)
    }
}

fn header_u32(headers: &HeaderMap, names: &[&str]) -> Option<u32> {
    header_u64(headers, names).and_then(|value| u32::try_from(value).ok())
}

fn header_u64(headers: &HeaderMap, names: &[&str]) -> Option<u64> {
    names.iter().find_map(|name| {
        headers
            .get(*name)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse().ok())
    })
}

fn value_u32(value: &Value, keys: &[&str]) -> Option<u32> {
    value_u64(value, keys).and_then(|value| u32::try_from(value).ok())
}

fn value_u64(value: &Value, keys: &[&str]) -> Option<u64> {
    keys.iter()
        .filter_map(|key| value.get(*key))
        .find_map(|value| match value {
            Value::Number(value) => value.as_u64(),
            Value::String(value) => value.parse().ok(),
            _ => None,
        })
}

impl ContactRow {
    fn from_object(object: &serde_json::Map<String, Value>) -> Self {
        Self {
            id: first_string(object, &["id", "contact_id", "ContactId"]).unwrap_or_default(),
            full_name: first_string(object, &["full_name", "field_1", "name"]).unwrap_or_default(),
            email: first_string(object, &["email", "field_2"]).unwrap_or_default(),
            last_chat_message: first_string(object, &["last_chat_message", "field_3"])
                .map(|value| format_datetime(&value))
                .unwrap_or_default(),
            created_at: first_string(object, &["created_at", "field_4"])
                .map(|value| format_datetime(&value))
                .unwrap_or_default(),
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

fn format_datetime(value: &str) -> String {
    parse_datetime(value)
        .map(|datetime| {
            format!(
                "{} {} at {}",
                ordinal_day(datetime.day()),
                datetime.format("%b %Y"),
                datetime.format("%H:%M")
            )
        })
        .unwrap_or_else(|| value.to_string())
}

fn parse_datetime(value: &str) -> Option<NaiveDateTime> {
    if let Some(datetime) = parse_epoch_datetime(value) {
        return Some(datetime);
    }

    if let Ok(datetime) = DateTime::parse_from_rfc3339(value) {
        return Some(datetime.naive_local());
    }

    for format in [
        "%Y-%m-%d %H:%M:%S",
        "%Y-%m-%d %H:%M",
        "%Y-%m-%dT%H:%M:%S",
        "%Y-%m-%dT%H:%M",
    ] {
        if let Ok(datetime) = NaiveDateTime::parse_from_str(value, format) {
            return Some(datetime);
        }
    }

    None
}

fn parse_epoch_datetime(value: &str) -> Option<NaiveDateTime> {
    let raw = value.trim();
    if raw.is_empty() || !raw.chars().all(|character| character.is_ascii_digit()) {
        return None;
    }

    let epoch = raw.parse::<i64>().ok()?;
    let seconds = match raw.len() {
        10 => epoch,
        13 => epoch / 1_000,
        _ => return None,
    };

    Local
        .timestamp_opt(seconds, 0)
        .single()
        .map(|value| value.naive_local())
}

fn ordinal_day(day: u32) -> String {
    let suffix = match day % 100 {
        11..=13 => "th",
        _ => match day % 10 {
            1 => "st",
            2 => "nd",
            3 => "rd",
            _ => "th",
        },
    };

    format!("{day}{suffix}")
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
    use reqwest::header::HeaderValue;
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
                        "field_3": "2026-04-30T11:56:00Z",
                        "field_4": "2026-04-30T11:44:00Z",
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
        assert_eq!(page.contacts[0].created_at, "30th Apr 2026 at 11:44");
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

        assert_eq!(page.pagination.page, 2);
        assert_eq!(page.contacts[0].full_name, "Roger Rabbit");
        assert_eq!(page.contacts[0].created_at, "27th Apr 2026 at 14:11");
    }

    #[test]
    fn renders_contacts_table() {
        let rendered = render_contacts_page(&ContactsPage {
            pagination: ContactsPagination {
                page: 1,
                per_page: 15,
                total_results: Some(1),
                total_pages: Some(1),
            },
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

    #[test]
    fn extracts_pagination_from_response_headers() {
        let mut headers = HeaderMap::new();
        headers.insert("current_page", HeaderValue::from_static("3"));
        headers.insert("per_page", HeaderValue::from_static("15"));
        headers.insert("total_results", HeaderValue::from_static("214451"));

        let pagination = ContactsPagination::from_response(&headers, &json!({ "data": [] }), 1, 30);

        assert_eq!(
            pagination,
            ContactsPagination {
                page: 3,
                per_page: 15,
                total_results: Some(214451),
                total_pages: Some(14297),
            }
        );
    }

    #[test]
    fn extracts_pagination_from_body_metadata() {
        let pagination = ContactsPagination::from_response(
            &HeaderMap::new(),
            &json!({
                "data": [],
                "pagination": {
                    "currentPage": "2",
                    "perPage": "30",
                    "totalResults": "214451",
                    "totalPages": "7149"
                }
            }),
            1,
            15,
        );

        assert_eq!(
            pagination,
            ContactsPagination {
                page: 2,
                per_page: 30,
                total_results: Some(214451),
                total_pages: Some(7149),
            }
        );
    }

    #[test]
    fn extracts_laravel_style_root_pagination() {
        let pagination = ContactsPagination::from_response(
            &HeaderMap::new(),
            &json!({
                "current_page": 3,
                "data": [],
                "from": 31,
                "last_page": 14297,
                "per_page": 15,
                "to": 45,
                "total": 214451
            }),
            1,
            30,
        );

        assert_eq!(
            pagination,
            ContactsPagination {
                page: 3,
                per_page: 15,
                total_results: Some(214451),
                total_pages: Some(14297),
            }
        );
    }

    #[test]
    fn derives_total_pages_from_body_total_results() {
        let pagination = ContactsPagination::from_response(
            &HeaderMap::new(),
            &json!({
                "data": [],
                "meta": {
                    "total_items": 31
                }
            }),
            1,
            15,
        );

        assert_eq!(pagination.total_results, Some(31));
        assert_eq!(pagination.total_pages, Some(3));
    }

    #[test]
    fn formats_datetime_values_with_ordinal_days() {
        assert_eq!(
            format_datetime("2026-04-01T09:05:00Z"),
            "1st Apr 2026 at 09:05"
        );
        assert_eq!(
            format_datetime("2026-04-02 10:15:00"),
            "2nd Apr 2026 at 10:15"
        );
        assert_eq!(
            format_datetime("2026-04-13T11:58:00Z"),
            "13th Apr 2026 at 11:58"
        );
    }

    #[test]
    fn formats_epoch_timestamps() {
        assert_eq!(format_datetime("1777546721"), "30th Apr 2026 at 11:58");
        assert_eq!(format_datetime("1777546721000"), "30th Apr 2026 at 11:58");
    }

    #[test]
    fn leaves_unknown_datetime_values_unchanged() {
        assert_eq!(
            format_datetime("30th Apr 2026 at 11:58"),
            "30th Apr 2026 at 11:58"
        );
    }
}
