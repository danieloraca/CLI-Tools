use crate::api::normalize_base_url;
use crate::auth::TokenSet;
use crate::session::AppSession;
use anyhow::{Context, Result};
use chrono::{DateTime, Datelike, NaiveDateTime, TimeZone, Utc};
use reqwest::header::HeaderMap;
use serde::Serialize;
use serde_json::Value;

const CONTACT_RFIELDS: &str =
    "id,full_name,email,created_at,field_1,field_2,field_3,field_4,field_5,field_6";
const LABEL_RFIELDS: &str = "color,name";

#[derive(Debug, Clone)]
pub struct ContactService {
    base_url: String,
    query: crate::query::ContactQuery,
}

impl ContactService {
    pub fn new(base_url: impl Into<String>) -> Result<Self> {
        let base_url = normalize_base_url(base_url.into())?;
        Ok(Self {
            base_url,
            query: Default::default(),
        })
    }

    pub fn with_query(mut self, query: crate::query::ContactQuery) -> Result<Self> {
        query.validate()?;
        self.query = query;
        Ok(self)
    }

    pub fn list_contacts(
        &self,
        tokens: &TokenSet,
        session: &AppSession,
        page: u32,
        per_page: u32,
    ) -> Result<ContactsPage> {
        let api = crate::gecko::GeckoApi::new(&self.base_url, tokens, session)?;
        let fields = api.collection("fields", &[("field_type", "contact".into())])?;
        let (payload, pagination) = self.read_page(&api, &fields, page, per_page, false)?;
        parse_page(payload, pagination, &fields)
    }

    pub fn read_page(
        &self,
        api: &crate::gecko::GeckoApi,
        fields: &[Value],
        page: u32,
        per_page: u32,
        custom: bool,
    ) -> Result<(Value, ContactsPagination)> {
        self.query.validate()?;
        anyhow::ensure!(
            page > 0 && (1..=500).contains(&per_page),
            "page must be positive and per-page must be 1..500"
        );
        self.query.validate_fields(fields)?;
        if let Some(id) = self.query.saved_filter {
            let filter = api.request(reqwest::Method::GET, &format!("filters/{id}"), &[], None)?;
            anyhow::ensure!(
                crate::gecko::resource_id(filter.get("filter").unwrap_or(&filter))?
                    == id.to_string(),
                "saved filter response has an unexpected ID"
            );
        }
        let mut query = vec![
            ("contact_rfields", CONTACT_RFIELDS.to_string()),
            ("label_rfields", LABEL_RFIELDS.to_string()),
            ("per_page", per_page.to_string()),
            ("page", page.to_string()),
            (
                "include",
                if custom {
                    "labels,current_values:1000,current_values.field"
                } else {
                    "labels"
                }
                .to_string(),
            ),
        ];
        query.extend(self.query.parameters());
        let conditions = self.query.conditions();
        let body = serde_json::json!({"conditions": conditions, "requirement": "ALL"});
        let (method, endpoint, body) = if conditions.is_empty() {
            (reqwest::Method::GET, "contacts", None)
        } else {
            (reqwest::Method::POST, "contacts/search", Some(&body))
        };
        let (headers, payload) = api.response(method, endpoint, &query, body)?;

        let pagination = ContactsPagination::from_response(&headers, &payload, page, per_page);
        Ok((payload, pagination))
    }

    pub fn contact_detail(
        &self,
        tokens: &TokenSet,
        session: &AppSession,
        contact_id: &str,
    ) -> Result<ContactDetail> {
        crate::gecko::resource_id(&serde_json::json!({"id":contact_id}))?;
        let api = crate::gecko::GeckoApi::new(&self.base_url, tokens, session)?;
        let payload = api.request(
            reqwest::Method::GET,
            &format!("contacts/{contact_id}"),
            &[
                ("contact_rfields", "id".to_string()),
                (
                    "include",
                    "current_values:1000,current_values.field".to_string(),
                ),
            ],
            None,
        )?;

        parse_contact_detail(payload)
    }
}

pub fn parse_page(
    payload: Value,
    pagination: ContactsPagination,
    fields: &[Value],
) -> Result<ContactsPage> {
    parse_contacts_page_with_pagination(payload, pagination, &ContactColumns::from_metadata(fields))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ContactsPage {
    pub contacts: Vec<ContactRow>,
    pub pagination: ContactsPagination,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ContactsPagination {
    pub page: u32,
    pub per_page: u32,
    pub total_results: Option<u64>,
    pub total_pages: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ContactRow {
    pub id: String,
    pub full_name: String,
    pub email: String,
    pub last_chat_message: String,
    pub created_at: String,
    pub phone: String,
    pub labels: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ContactDetail {
    pub id: String,
    pub fields: Vec<ContactFieldValue>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ContactFieldValue {
    pub label: String,
    pub value: String,
}

#[derive(Default)]
struct ContactColumns {
    phone: Option<String>,
    last_chat_message: Option<String>,
    mask_name: bool,
    mask_email: bool,
    mask_created: bool,
    mask_phone: bool,
    mask_chat: bool,
}

impl ContactColumns {
    fn from_metadata(fields: &[Value]) -> Self {
        let column = |kind: &str| {
            fields
                .iter()
                .filter(|field| field["type"] == kind)
                .filter_map(|field| {
                    value_u32(field, &["contact_list_view"]).map(|position| (position, field))
                })
                .filter(|(position, _)| (1..=6).contains(position))
                .min_by_key(|(position, _)| *position)
        };
        // Without metadata we cannot establish that a stable property is safe to display.
        let sensitive = |kind: &str| {
            let mut matching = fields
                .iter()
                .filter(|field| field["type"] == kind)
                .peekable();
            matching.peek().is_none() || matching.any(|field| truthy(field.get("is_sensitive")))
        };
        let phone = column("tel");
        let chat = column("last_chat_message");
        Self {
            phone: phone.map(|(position, _)| format!("field_{position}")),
            last_chat_message: chat.map(|(position, _)| format!("field_{position}")),
            mask_name: sensitive("name"),
            mask_email: sensitive("email"),
            mask_created: sensitive("contact_created"),
            mask_phone: phone.is_none_or(|(_, field)| truthy(field.get("is_sensitive"))),
            mask_chat: chat.is_none_or(|(_, field)| truthy(field.get("is_sensitive"))),
        }
    }
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
        &ContactColumns::default(),
    )
}

fn parse_contacts_page_with_pagination(
    payload: Value,
    pagination: ContactsPagination,
    columns: &ContactColumns,
) -> Result<ContactsPage> {
    let contacts = contact_items(&payload)
        .context("contacts API response did not include a contacts array")?
        .iter()
        .filter_map(Value::as_object)
        .map(|object| ContactRow::from_object(object, columns))
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

pub fn parse_contact_detail(payload: Value) -> Result<ContactDetail> {
    let contact = payload
        .get("contact")
        .or_else(|| payload.get("data"))
        .or(Some(&payload))
        .and_then(Value::as_object)
        .context("contact detail response did not include a contact object")?;

    let id = first_string(contact, &["id", "contact_id", "ContactId"]).unwrap_or_default();
    let values = contact
        .get("current_values")
        .and_then(Value::as_array)
        .context("contact detail response did not include current values")?;

    let fields = values
        .iter()
        .filter_map(Value::as_object)
        .filter_map(ContactFieldValue::from_object)
        .collect();

    Ok(ContactDetail { id, fields })
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
    fn from_object(object: &serde_json::Map<String, Value>, columns: &ContactColumns) -> Self {
        let mut contact = Self {
            id: first_string(object, &["id", "contact_id", "ContactId"]).unwrap_or_default(),
            full_name: first_string(object, &["full_name", "name"]).unwrap_or_default(),
            email: first_string(object, &["email"]).unwrap_or_default(),
            last_chat_message: columns
                .last_chat_message
                .as_deref()
                .and_then(|column| first_string(object, &[column]))
                .or_else(|| first_string(object, &["last_chat_message"]))
                .map(|value| format_datetime(&value))
                .unwrap_or_default(),
            created_at: first_string(object, &["created_at"])
                .map(|value| format_datetime(&value))
                .unwrap_or_default(),
            phone: columns
                .phone
                .as_deref()
                .and_then(|column| first_string(object, &[column]))
                .or_else(|| first_string(object, &["telephone", "phone"]))
                .unwrap_or_default(),
            labels: parse_labels(object.get("labels")),
        };
        for (value, sensitive) in [
            (&mut contact.full_name, columns.mask_name),
            (&mut contact.email, columns.mask_email),
            (&mut contact.created_at, columns.mask_created),
            (&mut contact.phone, columns.mask_phone),
            (&mut contact.last_chat_message, columns.mask_chat),
        ] {
            if sensitive && !value.is_empty() {
                *value = "************".into();
            }
        }
        contact
    }
}

impl ContactFieldValue {
    fn from_object(object: &serde_json::Map<String, Value>) -> Option<Self> {
        let field = object.get("field").and_then(Value::as_object)?;
        let label = first_string(field, &["label", "Label", "tag", "type"])?;
        let data_type = first_string(field, &["data_type", "dataType"]).unwrap_or_default();
        let sensitive = truthy(field.get("is_sensitive"));
        let value = if sensitive {
            masked_value(object).unwrap_or_else(|| "************".to_string())
        } else {
            current_value(object, &data_type).unwrap_or_else(|| "-".to_string())
        };

        Some(Self { label, value })
    }
}

fn current_value(object: &serde_json::Map<String, Value>, data_type: &str) -> Option<String> {
    ["safe_db", "safe", "value", "varchar", "text", "numeric"]
        .iter()
        .filter_map(|key| object.get(*key))
        .find_map(|value| field_display_value(value, data_type))
        .filter(|value| !value.trim().is_empty())
}

fn masked_value(object: &serde_json::Map<String, Value>) -> Option<String> {
    current_value(object, "").map(|value| {
        if value == "-" {
            value
        } else {
            "*".repeat(value.chars().count().clamp(8, 12))
        }
    })
}

fn field_display_value(value: &Value, data_type: &str) -> Option<String> {
    match value {
        Value::Null => None,
        Value::String(value) => Some(format_field_string(value, data_type)),
        Value::Number(value) => {
            let raw = value.to_string();
            if data_type == "timestamp" {
                Some(format_datetime(&raw))
            } else {
                Some(raw)
            }
        }
        Value::Bool(value) => Some(value.to_string()),
        Value::Object(object) => {
            let first = first_string(object, &["first_name", "firstName"]);
            let last = first_string(object, &["last_name", "lastName"]);
            match (first, last) {
                (Some(first), Some(last)) => Some(format!("{first} {last}")),
                (Some(first), None) => Some(first),
                (None, Some(last)) => Some(last),
                (None, None) => Some(Value::Object(object.clone()).to_string()),
            }
        }
        Value::Array(values) => {
            let values = values
                .iter()
                .filter_map(|value| field_display_value(value, data_type))
                .collect::<Vec<_>>();
            (!values.is_empty()).then(|| values.join(", "))
        }
    }
}

fn format_field_string(value: &str, data_type: &str) -> String {
    if data_type == "timestamp" {
        format_datetime(value)
    } else {
        value.to_string()
    }
}

pub fn truthy(value: Option<&Value>) -> bool {
    match value {
        Some(Value::Bool(value)) => *value,
        Some(Value::Number(value)) => value.as_u64().unwrap_or(0) > 0,
        Some(Value::String(value)) => matches!(value.as_str(), "1" | "true" | "yes"),
        _ => false,
    }
}

pub fn contact_items(payload: &Value) -> Option<&Vec<Value>> {
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

pub fn render_table(headers: &[&str], rows: &[Vec<String>]) -> String {
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
        return Some(datetime.naive_utc());
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

    Utc.timestamp_opt(seconds, 0)
        .single()
        .map(|value| value.naive_utc())
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
                        "full_name": "Peter Tester",
                        "email": "peter@example.com",
                        "last_chat_message": "2026-04-30T11:56:00Z",
                        "created_at": "2026-04-30T11:44:00Z",
                        "phone": "+4412345",
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
    fn parses_contact_detail_current_values() {
        let detail = parse_contact_detail(json!({
            "contact": {
                "id": 444621,
                "current_values": [
                    {
                        "safe": 1777546721,
                        "field": {
                            "label": "Contact created",
                            "data_type": "timestamp",
                            "is_sensitive": null
                        }
                    },
                    {
                        "safe_db": "peter+otpagain@example.com",
                        "field": {
                            "label": "Email address (s)",
                            "data_type": "string",
                            "is_sensitive": 1
                        }
                    },
                    {
                        "value": { "first_name": "peter", "last_name": "nother" },
                        "field": {
                            "label": "Full Name (s)",
                            "data_type": "json"
                        }
                    }
                ]
            }
        }))
        .unwrap();

        assert_eq!(detail.id, "444621");
        assert_eq!(detail.fields[0].label, "Contact created");
        assert_eq!(detail.fields[0].value, "30th Apr 2026 at 10:58");
        assert_eq!(detail.fields[1].label, "Email address (s)");
        assert_eq!(detail.fields[1].value, "************");
        assert_eq!(detail.fields[2].value, "peter nother");
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
        assert_eq!(format_datetime("1777546721"), "30th Apr 2026 at 10:58");
        assert_eq!(format_datetime("1777546721000"), "30th Apr 2026 at 10:58");
    }

    #[test]
    fn leaves_unknown_datetime_values_unchanged() {
        assert_eq!(
            format_datetime("30th Apr 2026 at 11:58"),
            "30th Apr 2026 at 11:58"
        );
    }

    #[test]
    fn equivalent_timestamps_display_the_same_utc_instant() {
        for value in [
            "1777546721",
            "1777546721000",
            "2026-04-30T10:58:41Z",
            "2026-04-30T11:58:41+01:00",
            "2026-04-30T05:58:41-05:00",
            "2026-04-30 10:58:41",
        ] {
            assert_eq!(format_datetime(value), "30th Apr 2026 at 10:58", "{value}");
        }
    }

    fn test_session() -> AppSession {
        AppSession {
            profile_id: "p-1".into(),
            account_id: "test-account-uuid".into(),
            user_id: "app-user-1".into(),
            account_name: "Development".into(),
            app_description: "Forms".into(),
            redirect_url: String::new(),
        }
    }

    #[test]
    fn queries_are_retained_across_pages_and_json_remains_masked() {
        use clap::Parser;
        #[derive(Parser)]
        struct Q {
            #[command(flatten)]
            query: crate::query::ContactQuery,
        }
        let query = Q::parse_from([
            "test",
            "--search",
            "test@example.test",
            "--where",
            "17:empty",
            "--label-id",
            "9",
            "--sort",
            "created-desc",
        ])
        .query;
        let mut responses = Vec::new();
        for id in [101, 102] {
            responses.extend([
                (200, crate::test_support::auth_identity()),
                (200, json!({"fields":[{"id":17,"type":"email","is_sensitive":true}]})),
                (200, json!({"fields":[]})),
                (200, json!({"contacts":[{"id":id,"email":"secret@example.test","full_name":"Secret Name"}]})),
            ]);
        }
        let server = crate::test_support::Server::new(responses);
        let service = ContactService::new(&server.url)
            .unwrap()
            .with_query(query)
            .unwrap();
        for page in [1, 2] {
            let result = service
                .list_contacts(
                    &crate::test_support::app_tokens("app-user-1"),
                    &test_session(),
                    page,
                    15,
                )
                .unwrap();
            let encoded = serde_json::to_string(&result).unwrap();
            assert!(!encoded.contains("secret@example.test"));
            assert!(!encoded.contains("Secret Name"));
            assert_eq!(
                serde_json::from_str::<Value>(&encoded).unwrap()["pagination"]["page"],
                page
            );
        }
        let requests = server.finish();
        assert_eq!(requests[3].body, requests[7].body);
        assert_eq!(requests[3].body["requirement"], "ALL");
        assert_eq!(
            requests[3].body["conditions"][1],
            json!({"model":"label","type":"=","value":[9]})
        );
        assert!(requests[3].line.starts_with("POST /contacts/search?"));
        assert!(
            requests[3]
                .line
                .contains("contact_keyword=test%40example.test")
        );
        assert!(requests[7].line.contains("page=2"));
        assert!(
            requests[7]
                .line
                .contains("order_by=created_at%7CDESC%2Cid%7CASC")
        );
    }

    #[test]
    fn loads_stable_contact_columns_and_resolves_configured_phone_and_chat_fields() {
        let server = crate::test_support::Server::with_identity(vec![
            (
                200,
                json!({"fields": [
                    {"id": 1, "type": "email", "contact_list_view": 1}, {"id": 2, "type": "name", "contact_list_view": 2},
                    {"id": 3, "type": "tel", "contact_list_view": "3"}, {"id": 4, "type": "text", "contact_list_view": 4},
                    {"id": 5, "type": "last_chat_message", "contact_list_view": 5}, {"id": 6, "type": "contact_created", "contact_list_view": null},
                ]}),
            ),
            (200, json!({"data": []})),
            (
                200,
                json!({"contacts": [{"id": 7, "full_name": "Peter Tester", "email": "peter@example.test", "created_at": "2026-04-30T10:58:41Z",
                "field_1": "peter@example.test", "field_2": "Peter Tester", "field_3": "+4412345", "field_4": "History", "field_5": "2026-05-01T11:00:00+01:00"}]}),
            ),
        ]);
        let service = ContactService::new(&server.url).unwrap();
        let page = service
            .list_contacts(
                &crate::test_support::app_tokens("app-user-1"),
                &test_session(),
                1,
                15,
            )
            .unwrap();
        assert_eq!(page.contacts[0].full_name, "Peter Tester");
        assert_eq!(page.contacts[0].email, "peter@example.test");
        assert_eq!(page.contacts[0].phone, "+4412345");
        assert_eq!(page.contacts[0].created_at, "30th Apr 2026 at 10:58");
        assert_eq!(page.contacts[0].last_chat_message, "1st May 2026 at 10:00");
        let all_requests = server.finish();
        assert!(all_requests[0].line.starts_with("GET /auth/check?"));
        let requests = &all_requests[1..];
        assert!(requests[0].line.starts_with("GET /fields?"));
        assert!(requests[1].line.contains("page=2"));
        let request_url = reqwest::Url::parse(&format!(
            "http://example.test{}",
            requests[2].line.split_whitespace().nth(1).unwrap()
        ))
        .unwrap();
        let rfields = request_url
            .query_pairs()
            .find(|(key, _)| key == "contact_rfields")
            .unwrap()
            .1;
        for stable in ["id", "full_name", "email", "created_at"] {
            assert!(rfields.split(',').any(|field| field == stable));
        }
    }

    #[test]
    fn unconfigured_list_fields_are_never_guessed() {
        let page = parse_contacts_page(json!([{"id": 7, "field_1": "Programme", "field_2": "2026", "field_3": "History", "field_4": "1234", "field_5": "Notes"}]), 1, 15).unwrap();
        let contact = &page.contacts[0];
        assert!(
            contact.full_name.is_empty()
                && contact.email.is_empty()
                && contact.phone.is_empty()
                && contact.created_at.is_empty()
                && contact.last_chat_message.is_empty()
        );
    }

    #[test]
    fn contact_reads_reject_mismatched_profiles_and_accounts() {
        let service = ContactService::new("http://127.0.0.1:1").unwrap();
        let error = service
            .list_contacts(
                &crate::test_support::app_tokens("other"),
                &test_session(),
                1,
                15,
            )
            .unwrap_err();
        assert!(error.to_string().contains("does not match saved app user"));
        for account in [None, Some("999")] {
            let server = crate::test_support::Server::with_account(
                vec![(200, crate::test_support::auth_identity())],
                account,
            );
            let service = ContactService::new(&server.url).unwrap();
            assert!(
                service
                    .contact_detail(
                        &crate::test_support::app_tokens("app-user-1"),
                        &test_session(),
                        "7"
                    )
                    .is_err()
            );
            assert_eq!(server.finish().len(), 1);
        }
    }

    #[test]
    fn opens_contact_detail_with_the_resolved_account_and_user_headers() {
        let server = crate::test_support::Server::with_identity(vec![(
            200,
            json!({"contact": {"id": 7, "current_values": []}}),
        )]);
        let detail = ContactService::new(&server.url)
            .unwrap()
            .contact_detail(
                &crate::test_support::app_tokens("app-user-1"),
                &test_session(),
                "7",
            )
            .unwrap();
        assert_eq!(detail.id, "7");
        let requests = server.finish();
        assert_eq!(requests.len(), 2);
        assert!(requests[0].line.starts_with("GET /auth/check?"));
        assert!(requests[1].line.starts_with("GET /contacts/7?"));
        let headers = requests[1].headers.to_lowercase();
        assert!(headers.contains("gecko-account: 281"));
        assert!(headers.contains("gecko-user: 2260"));
    }

    #[test]
    fn masks_sensitive_fields_even_when_omitted_from_the_configured_list() {
        let server = crate::test_support::Server::with_identity(vec![
            (
                200,
                json!({"data": [
                    {"id": 1, "type": "tel", "contact_list_view": 1, "is_sensitive": true},
                    {"id": 2, "type": "last_chat_message", "contact_list_view": 2, "is_sensitive": 1},
                ]}),
            ),
            (
                200,
                json!({"data": [
                    {"id": 3, "type": "name", "contact_list_view": null, "is_sensitive": true},
                    {"id": 4, "type": "email", "contact_list_view": null, "is_sensitive": "1"},
                    {"id": 5, "type": "contact_created", "contact_list_view": null, "is_sensitive": 1},
                ]}),
            ),
            (200, json!({"data": []})),
            (
                200,
                json!({"contacts": [{"id": 7, "full_name": "Private Name", "email": "private@example.test", "created_at": "2026-04-30T10:58:41Z", "field_1": "+4412345", "field_2": "2026-05-01T10:00:00Z"}]}),
            ),
        ]);
        let page = ContactService::new(&server.url)
            .unwrap()
            .list_contacts(
                &crate::test_support::app_tokens("app-user-1"),
                &test_session(),
                1,
                15,
            )
            .unwrap();
        let contact = &page.contacts[0];
        for value in [
            &contact.full_name,
            &contact.email,
            &contact.created_at,
            &contact.phone,
            &contact.last_chat_message,
        ] {
            assert_eq!(value, "************");
        }
        let table = render_contacts_page(&page);
        assert!(
            !table.contains("Private Name")
                && !table.contains("private@example.test")
                && !table.contains("+4412345")
        );
        let all_requests = server.finish();
        assert!(all_requests[0].line.starts_with("GET /auth/check?"));
        let requests = &all_requests[1..];
        assert_eq!(requests.len(), 4);
        assert!(requests[1].line.contains("page=2"));
    }

    #[test]
    fn missing_privacy_metadata_masks_stable_properties() {
        let columns = ContactColumns::from_metadata(&[]);
        let row = ContactRow::from_object(json!({"id": 7, "full_name": "Private Name", "email": "private@example.test", "created_at": "2026-04-30T10:58:41Z"}).as_object().unwrap(), &columns);
        assert_eq!(row.full_name, "************");
        assert_eq!(row.email, "************");
        assert_eq!(row.created_at, "************");
    }
}
