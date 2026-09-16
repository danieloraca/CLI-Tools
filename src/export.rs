use crate::{
    auth::TokenSet,
    contacts::{ContactService, ContactsPagination},
    gecko::{GeckoApi, resource_id},
    query::ContactQuery,
    session::AppSession,
};
use anyhow::{Context, Result, bail, ensure};
use clap::Args;
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::Write,
    path::PathBuf,
};

#[derive(Debug, Default, Args)]
pub struct ExportArgs {
    /// Print CSV. Missing values are empty cells; arrays/objects use JSON inside the cell.
    #[arg(long, conflicts_with_all = ["json", "plain"])]
    pub csv: bool,
    /// Comma-separated output columns. Custom fields use field:ID, e.g. id,email,field:28.
    #[arg(long, value_delimiter = ',')]
    pub columns: Vec<String>,
    /// Export every page, starting at page 1. Rejects a different --page.
    #[arg(long)]
    pub all: bool,
    /// Create a new private output file only after the complete export succeeds.
    #[arg(long)]
    pub output: Option<PathBuf>,
    /// Stop with an error if an export exceeds this row limit.
    #[arg(long, default_value_t = 100000, value_parser = clap::value_parser!(u32).range(1..=1000000))]
    pub max_rows: u32,
}
impl ExportArgs {
    pub fn active(&self) -> bool {
        self.csv || !self.columns.is_empty() || self.all || self.output.is_some()
    }
}

#[derive(Debug)]
pub struct Export {
    columns: Vec<String>,
    rows: Vec<BTreeMap<String, Value>>,
    pagination: ContactsPagination,
    all: bool,
}

pub fn collect(
    api: &GeckoApi,
    service: &ContactService,
    fields: &[Value],
    args: &ExportArgs,
    page: u32,
    per_page: u32,
) -> Result<Export> {
    ensure!(
        !args.all || page == 1,
        "--all starts at page 1; omit --page or set it to 1"
    );
    let columns = if args.columns.is_empty() {
        vec!["id", "full_name", "email", "phone", "created_at", "labels"]
            .into_iter()
            .map(String::from)
            .collect()
    } else {
        args.columns.clone()
    };
    let mut custom = BTreeMap::new();
    let mut unique = BTreeSet::new();
    for name in &columns {
        ensure!(unique.insert(name), "duplicate output column {name}");
        if let Some(id) = name.strip_prefix("field:") {
            let id: u64 = id
                .parse()
                .ok()
                .filter(|id| *id > 0)
                .context("custom columns use field:POSITIVE_ID")?;
            let metadata = fields
                .iter()
                .find(|f| resource_id(f).ok().as_deref() == Some(&id.to_string()))
                .with_context(|| format!("unknown contact field {id}"))?;
            custom.insert(name.clone(), (id.to_string(), metadata));
        } else if matches!(name.as_str(), "phone" | "last_chat_message") {
            let kind = if name == "phone" {
                "tel"
            } else {
                "last_chat_message"
            };
            let candidates: Vec<_> = fields.iter().filter(|f| f["type"] == kind).collect();
            let chosen = match candidates.as_slice() {
                [] => None,
                [field] => Some(*field),
                _ => {
                    let configured: Vec<_> = candidates
                        .iter()
                        .filter(|f| {
                            f["contact_list_view"]
                                .as_u64()
                                .or_else(|| {
                                    f["contact_list_view"].as_str().and_then(|s| s.parse().ok())
                                })
                                .is_some_and(|n| (1..=6).contains(&n))
                        })
                        .collect();
                    ensure!(
                        configured.len() == 1,
                        "ambiguous {name} fields; select an explicit field:ID column"
                    );
                    Some(**configured.first().unwrap())
                }
            };
            if let Some(field) = chosen {
                custom.insert(name.clone(), (resource_id(field)?, field));
            }
        } else {
            ensure!(
                [
                    "id",
                    "full_name",
                    "email",
                    "phone",
                    "created_at",
                    "last_chat_message",
                    "labels"
                ]
                .contains(&name.as_str()),
                "unknown output column {name}"
            );
        }
    }
    let mut rows = Vec::new();
    let mut seen = BTreeSet::new();
    let mut first_pagination = None;
    for current_page in page..page.saturating_add(10000) {
        let (payload, pagination) =
            service.read_page(api, fields, current_page, per_page, !custom.is_empty())?;
        ensure!(
            pagination.page == current_page,
            "API returned a different page; export incomplete"
        );
        let first = *first_pagination.get_or_insert(pagination);
        if args.all {
            ensure!(
                first.total_results == pagination.total_results,
                "result count changed during export; retry when the data is stable"
            );
        }
        let items = crate::contacts::contact_items(&payload)
            .context("contacts response omitted its array")?;
        let safe_page = crate::contacts::parse_page(payload.clone(), pagination, fields)?;
        ensure!(
            items.len() == safe_page.contacts.len(),
            "contacts response contains malformed rows"
        );
        for (item, safe) in items.iter().zip(safe_page.contacts) {
            let id = resource_id(item)?;
            ensure!(
                seen.insert(id),
                "contact pagination repeated an ID; export incomplete"
            );
            ensure!(
                rows.len() < args.max_rows as usize,
                "export exceeded --max-rows {}; raise the limit or narrow the query",
                args.max_rows
            );
            let safe = serde_json::to_value(safe)?;
            let mut row = BTreeMap::new();
            for column in &columns {
                let value = if let Some((id, metadata)) = custom.get(column) {
                    custom_value(item, id, metadata)?
                } else if matches!(column.as_str(), "phone" | "last_chat_message") {
                    Value::Null
                } else {
                    safe[column].clone()
                };
                row.insert(column.clone(), value);
            }
            rows.push(row);
        }
        if !args.all || items.is_empty() {
            if args.all
                && let Some(total) = first.total_results
            {
                ensure!(
                    rows.len() as u64 == total,
                    "export row count does not match Gecko's total; export incomplete"
                );
            }
            return Ok(Export {
                columns,
                rows,
                pagination: first,
                all: args.all,
            });
        }
    }
    bail!("contact export exceeded 10000 pages; export incomplete")
}

pub fn custom_value(contact: &Value, field_id: &str, metadata: &Value) -> Result<Value> {
    let values = contact
        .get("current_values")
        .and_then(Value::as_array)
        .context("current_values relation is missing; cannot establish custom values")?;
    ensure!(
        values.len() < 1000,
        "current_values reached its limit; custom values may be incomplete"
    );
    let mut found = None;
    for value in values {
        let id = value
            .get("contact_field_id")
            .filter(|v| !v.is_null() && **v != 0)
            .or_else(|| {
                value
                    .get("field")
                    .and_then(|f| f.get("contact_field_id"))
                    .filter(|v| !v.is_null() && **v != 0)
            })
            .or_else(|| value.get("field_id"))
            .or_else(|| value.get("field").and_then(|f| f.get("id")));
        if id
            .and_then(|v| resource_id(&json!({"id":v})).ok())
            .as_deref()
            != Some(field_id)
        {
            continue;
        }
        ensure!(
            found.is_none(),
            "multiple current values for field {field_id}; cannot choose reliably"
        );
        let sensitive = crate::contacts::truthy(metadata.get("is_sensitive"))
            || crate::contacts::truthy(value.get("field").and_then(|f| f.get("is_sensitive")));
        found = Some(if sensitive {
            json!("************")
        } else {
            value
                .get("value")
                .cloned()
                .context("current value omitted its typed value; cannot export reliably")?
        });
    }
    Ok(found.unwrap_or(Value::Null))
}

fn text_value(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::String(s) => s.clone(),
        _ => value.to_string(),
    }
}
fn csv_cell(value: &str) -> String {
    format!("\"{}\"", value.replace('"', "\"\""))
}
impl Export {
    fn encode(&self, csv: bool, plain: bool) -> Result<Vec<u8>> {
        if csv {
            let mut data = self
                .columns
                .iter()
                .map(|c| csv_cell(c))
                .collect::<Vec<_>>()
                .join(",");
            data.push_str("\r\n");
            for row in &self.rows {
                data.push_str(
                    &self
                        .columns
                        .iter()
                        .map(|c| csv_cell(&text_value(&row[c])))
                        .collect::<Vec<_>>()
                        .join(","),
                );
                data.push_str("\r\n");
            }
            Ok(data.into_bytes())
        } else if plain {
            let headers: Vec<_> = self.columns.iter().map(String::as_str).collect();
            let rows: Vec<_> = self
                .rows
                .iter()
                .map(|row| self.columns.iter().map(|c| text_value(&row[c])).collect())
                .collect();
            Ok(format!("{}\n", crate::contacts::render_table(&headers, &rows)).into_bytes())
        } else {
            let mut data = serde_json::to_vec_pretty(
                &json!({"contacts": self.rows, "pagination":self.pagination, "all_pages":self.all, "exported":self.rows.len()}),
            )?;
            data.push(b'\n');
            Ok(data)
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub fn run(
    base_url: &str,
    tokens: &TokenSet,
    session: &AppSession,
    query: ContactQuery,
    args: &ExportArgs,
    page: u32,
    per_page: u32,
    plain: bool,
) -> Result<()> {
    let api = GeckoApi::new(base_url, tokens, session)?;
    let service = ContactService::new(base_url)?.with_query(query)?;
    let fields = api.collection("fields", &[("field_type", "contact".into())])?;
    let export = collect(&api, &service, &fields, args, page, per_page)?;
    let data = export.encode(args.csv, plain)?;
    if let Some(path) = &args.output {
        crate::storage::create_private_atomic(path, &data)?;
    } else {
        std::io::stdout().lock().write_all(&data)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{Directory, Server};
    fn selected() -> AppSession {
        AppSession {
            profile_id: "p-1".into(),
            account_id: "test-account-uuid".into(),
            user_id: "app-user-1".into(),
            account_name: "Development".into(),
            app_description: "Forms".into(),
            redirect_url: String::new(),
        }
    }
    fn fields() -> Vec<Value> {
        vec![
            json!({"id":17,"type":"email","is_sensitive":true}),
            json!({"id":93,"type":"number","is_sensitive":false}),
        ]
    }
    fn row(id: u64) -> Value {
        json!({"id":id,"email":"secret@example.test","current_values":[{"field_id":777,"contact_field_id":93,"value":0,"field":{"id":777,"is_sensitive":false}}]})
    }
    fn options(all: bool) -> ExportArgs {
        ExportArgs {
            all,
            columns: vec!["id".into(), "email".into(), "field:93".into()],
            max_rows: 100,
            ..Default::default()
        }
    }
    #[test]
    fn custom_ids_preserve_typed_values_nulls_and_mask_sensitive_values() {
        let item = row(101);
        assert_eq!(custom_value(&item, "93", &fields()[1]).unwrap(), 0);
        assert_eq!(
            custom_value(&item, "94", &fields()[1]).unwrap(),
            Value::Null
        );
        assert_eq!(
            custom_value(&item, "93", &json!({"is_sensitive":true})).unwrap(),
            "************"
        );
        for value in [
            json!(false),
            json!(["a", "b"]),
            json!({"first_name":"Jo"}),
            Value::Null,
        ] {
            let mut item = item.clone();
            item["current_values"][0]["value"] = value.clone();
            assert_eq!(custom_value(&item, "93", &fields()[1]).unwrap(), value);
        }
        assert!(custom_value(&json!({}), "93", &fields()[1]).is_err());
        let mut duplicate = item.clone();
        duplicate["current_values"]
            .as_array_mut()
            .unwrap()
            .push(item["current_values"][0].clone());
        assert!(custom_value(&duplicate, "93", &fields()[1]).is_err());
    }
    #[test]
    fn aliases_export_real_values_outside_the_six_list_columns() {
        let metadata = vec![
            json!({"id":67,"type":"tel","contact_list_view":null}),
            json!({"id":68,"type":"last_chat_message","contact_list_view":null}),
        ];
        let server = Server::with_identity(vec![(
            200,
            json!({"contacts":[{"id":1,"current_values":[{"field_id":67,"value":"+441234","field":{"is_sensitive":false}},{"field_id":68,"value":1777546721,"field":{"is_sensitive":false}}]}]}),
        )]);
        let api = GeckoApi::new(
            &server.url,
            &crate::test_support::app_tokens("app-user-1"),
            &selected(),
        )
        .unwrap();
        let args = ExportArgs {
            columns: vec!["phone".into(), "last_chat_message".into()],
            max_rows: 10,
            ..Default::default()
        };
        let service = ContactService::new(&server.url).unwrap();
        let export = collect(&api, &service, &metadata, &args, 1, 15).unwrap();
        assert_eq!(export.rows[0]["phone"], "+441234");
        assert_eq!(export.rows[0]["last_chat_message"], 1777546721u64);
        let mut ambiguous = metadata.clone();
        ambiguous.push(json!({"id":69,"type":"tel"}));
        assert!(
            collect(&api, &service, &ambiguous, &args, 1, 15)
                .unwrap_err()
                .to_string()
                .contains("ambiguous phone")
        );
        assert!(server.finish()[1].line.contains("current_values%3A1000"));
    }

    #[test]
    fn all_pages_survive_server_page_caps_and_keep_json_private() {
        let server = Server::with_identity(vec![
            (200, json!({"contacts":[row(1)]})),
            (200, json!({"contacts":[row(2)]})),
            (200, json!({"contacts":[]})),
        ]);
        let api = GeckoApi::new(
            &server.url,
            &crate::test_support::app_tokens("app-user-1"),
            &selected(),
        )
        .unwrap();
        let service = ContactService::new(&server.url).unwrap();
        let export = collect(&api, &service, &fields(), &options(true), 1, 100).unwrap();
        let data = export.encode(false, false).unwrap();
        let json: Value = serde_json::from_slice(&data).unwrap();
        assert_eq!(json["exported"], 2);
        assert_eq!(json["contacts"][0]["field:93"], 0);
        assert!(
            !String::from_utf8(data)
                .unwrap()
                .contains("secret@example.test")
        );
        let requests = server.finish();
        assert!(requests[1].line.contains("current_values%3A1000"));
        assert!(requests[3].line.contains("page=3"));
    }
    #[test]
    fn repeated_pages_and_midstream_errors_are_not_successful_exports() {
        for response in [
            (200, json!({"contacts":[row(1)]})),
            (500, json!({"message":"failed"})),
        ] {
            let server = Server::with_identity(vec![(200, json!({"contacts":[row(1)]})), response]);
            let api = GeckoApi::new(
                &server.url,
                &crate::test_support::app_tokens("app-user-1"),
                &selected(),
            )
            .unwrap();
            assert!(
                collect(
                    &api,
                    &ContactService::new(&server.url).unwrap(),
                    &fields(),
                    &options(true),
                    1,
                    100
                )
                .is_err()
            );
            assert_eq!(server.finish().len(), 3);
        }
        let directory = Directory::new();
        let output = directory.path("export.csv");
        let server = Server::with_identity(vec![
            (200, json!({"fields":fields()})),
            (200, json!({"fields":[]})),
            (200, json!({"contacts":[row(1)]})),
            (500, json!({"message":"failed"})),
        ]);
        let mut args = options(true);
        args.output = Some(output.clone());
        assert!(
            run(
                &server.url,
                &crate::test_support::app_tokens("app-user-1"),
                &selected(),
                Default::default(),
                &args,
                1,
                100,
                false
            )
            .is_err()
        );
        assert!(!output.exists());
        server.finish();
    }
    #[test]
    fn csv_escapes_without_losing_quotes_newlines_or_typed_values() {
        let export = Export {
            columns: vec!["notes".into(), "number".into(), "missing".into()],
            rows: vec![BTreeMap::from([
                ("notes".into(), json!("a,\"b\"\nc")),
                ("number".into(), json!(0)),
                ("missing".into(), Value::Null),
            ])],
            pagination: ContactsPagination {
                page: 1,
                per_page: 1,
                total_results: None,
                total_pages: None,
            },
            all: false,
        };
        assert_eq!(
            String::from_utf8(export.encode(true, false).unwrap()).unwrap(),
            "\"notes\",\"number\",\"missing\"\r\n\"a,\"\"b\"\"\nc\",\"0\",\"\"\r\n"
        );
    }
    #[test]
    fn publishes_complete_private_files_without_overwriting() {
        let directory = Directory::new();
        let path = directory.path("export.json");
        crate::storage::create_private_atomic(&path, b"complete").unwrap();
        assert!(crate::storage::create_private_atomic(&path, b"replacement").is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"complete");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        assert_eq!(std::fs::read_dir(directory.path("")).unwrap().count(), 1);
    }
}
