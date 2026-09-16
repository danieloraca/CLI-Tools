use anyhow::{Result, ensure};
use chrono::{DateTime, NaiveDate};
use clap::{Args, ValueEnum};
use serde_json::{Value, json};

#[derive(Debug, Clone, Default, Args)]
pub struct ContactQuery {
    /// Search names, emails and other searchable contact values on Gecko.
    #[arg(long)]
    pub search: Option<String>,
    /// Require every specified label. Repeat to combine label IDs.
    #[arg(long, value_parser = clap::value_parser!(u64).range(1..))]
    pub label_id: Vec<u64>,
    /// FIELD_ID:OP:VALUE; OP is eq, ne, contains, gt, lt, empty or not-empty.
    #[arg(long = "where", value_parser = parse_condition)]
    pub conditions: Vec<FieldCondition>,
    /// Saved filter ID, discoverable with `filters list`. Can combine with --search.
    #[arg(long, conflicts_with_all = ["label_id", "conditions", "created_after", "created_before"], value_parser = clap::value_parser!(u64).range(1..))]
    pub saved_filter: Option<u64>,
    /// Inclusive creation time (RFC3339 or YYYY-MM-DD, midnight UTC).
    #[arg(long, value_parser = parse_date)]
    pub created_after: Option<i64>,
    /// Exclusive creation time (RFC3339 or YYYY-MM-DD, midnight UTC).
    #[arg(long, value_parser = parse_date)]
    pub created_before: Option<i64>,
    #[arg(long, value_enum, default_value = "id-asc")]
    pub sort: Sort,
}

#[derive(Debug, Clone, Copy, Default, ValueEnum)]
pub enum Sort {
    #[default]
    IdAsc,
    IdDesc,
    NameAsc,
    EmailAsc,
    CreatedAsc,
    CreatedDesc,
    UpdatedDesc,
}

#[derive(Debug, Clone)]
pub struct FieldCondition {
    pub id: u64,
    operator: &'static str,
    value: Value,
}

fn parse_condition(input: &str) -> Result<FieldCondition, String> {
    let mut parts = input.splitn(3, ':');
    let id = parts
        .next()
        .unwrap_or_default()
        .parse::<u64>()
        .map_err(|_| "field ID must be a positive integer")?;
    if id == 0 {
        return Err("field ID must be positive".into());
    }
    let operator = match parts.next() {
        Some("eq") => "=",
        Some("ne") => "!=",
        Some("contains") => "%",
        Some("gt") => ">",
        Some("lt") => "<",
        Some("empty") => "--",
        Some("not-empty") => "!--",
        _ => {
            return Err(
                "expected FIELD_ID:OP[:VALUE]; OP: eq, ne, contains, gt, lt, empty, not-empty"
                    .into(),
            );
        }
    };
    let raw = parts.next();
    let value = if operator == "--" || operator == "!--" {
        if raw.is_some() {
            return Err("empty/not-empty do not take a value".into());
        }
        Value::Null
    } else {
        let raw = raw
            .filter(|v| !v.is_empty())
            .ok_or("comparison requires a value")?;
        let mut value: Value = serde_json::from_str(raw).unwrap_or_else(|_| json!(raw));
        if value.is_null() || value.is_array() || value.is_object() || value.is_boolean() {
            return Err("comparison value must be a string or number".into());
        }
        if matches!(operator, "=" | "!=")
            && (value.as_f64() == Some(0.0) || matches!(value.as_str(), Some("0" | "")))
        {
            return Err("Gecko treats equality with zero/empty text as missing; use empty/not-empty or another comparison".into());
        }
        // PHP's equality and LIKE helpers omit predicates for JSON floats; decimal text is supported.
        if value.is_f64() {
            value = json!(value.to_string());
        }
        value
    };
    Ok(FieldCondition {
        id,
        operator,
        value,
    })
}

fn parse_date(input: &str) -> Result<i64, String> {
    DateTime::parse_from_rfc3339(input)
        .map(|d| d.timestamp())
        .or_else(|_| {
            NaiveDate::parse_from_str(input, "%Y-%m-%d")
                .map(|d| d.and_hms_opt(0, 0, 0).unwrap().and_utc().timestamp())
        })
        .map_err(|_| "use RFC3339 or YYYY-MM-DD".into())
}

impl ContactQuery {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.search.as_ref().is_none_or(|s| !s.trim().is_empty()),
            "search cannot be blank"
        );
        if let (Some(after), Some(before)) = (self.created_after, self.created_before) {
            ensure!(after < before, "created-after must precede created-before");
        }
        ensure!(
            self.saved_filter.is_none()
                || (self.conditions.is_empty()
                    && self.label_id.is_empty()
                    && self.created_after.is_none()
                    && self.created_before.is_none()),
            "saved-filter cannot combine with explicit conditions; --search is supported"
        );
        Ok(())
    }
    pub fn validate_fields(&self, fields: &[Value]) -> Result<()> {
        for c in &self.conditions {
            ensure!(
                fields.iter().any(|f| f["id"].as_u64() == Some(c.id)
                    || f["id"].as_str() == Some(&c.id.to_string())),
                "unknown contact field ID {}",
                c.id
            );
        }
        Ok(())
    }
    pub fn parameters(&self) -> Vec<(&'static str, String)> {
        let order = match self.sort {
            Sort::IdAsc => "id|ASC",
            Sort::IdDesc => "id|DESC",
            Sort::NameAsc => "full_name|ASC,id|ASC",
            Sort::EmailAsc => "email|ASC,id|ASC",
            Sort::CreatedAsc => "created_at|ASC,id|ASC",
            Sort::CreatedDesc => "created_at|DESC,id|ASC",
            Sort::UpdatedDesc => "updated_at|DESC,id|ASC",
        };
        let mut result = vec![("order_by", order.to_string())];
        if let Some(s) = &self.search {
            result.push(("contact_keyword", s.clone()));
        }
        if let Some(id) = self.saved_filter {
            result.push(("filter_id", id.to_string()));
        }
        result
    }
    pub fn conditions(&self) -> Vec<Value> {
        let mut result: Vec<_> = self.conditions.iter().map(|c| json!({"model":"contact_field", "contact_field_id": c.id, "type":c.operator, "value":c.value})).collect();
        // Gecko drops inaccessible IDs inside a multi-label condition. Separate AND
        // conditions ensure one missing label makes the whole search match nothing.
        for id in &self.label_id {
            result.push(json!({"model":"label","type":"=","value":[id]}));
        }
        for (value, op) in [(self.created_after, ">="), (self.created_before, "<")] {
            if let Some(value) = value {
                result.push(json!({"model":"contact_dates","property":"created_at","type":op,"value":value}));
            }
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;
    #[derive(Parser)]
    struct TestCli {
        #[command(flatten)]
        query: ContactQuery,
    }
    #[test]
    fn validates_query_and_composes_all_conditions() {
        let q = TestCli::try_parse_from([
            "test",
            "--where",
            "17:contains:a:b",
            "--where",
            "28:empty",
            "--label-id",
            "9",
            "--created-after",
            "2026-01-01",
            "--search",
            "Jane",
        ])
        .unwrap()
        .query;
        q.validate().unwrap();
        assert_eq!(
            q.conditions()[0],
            json!({"model":"contact_field","contact_field_id":17,"type":"%","value":"a:b"})
        );
        assert_eq!(q.conditions()[1]["type"], "--");
        assert_eq!(q.conditions().len(), 4);
        assert!(q.parameters().contains(&("contact_keyword", "Jane".into())));
        assert!(q.validate_fields(&[]).is_err());
    }
    #[test]
    fn preserves_decimal_equality_and_rejects_backend_falsy_or_boolean_semantics() {
        assert_eq!(parse_condition("17:eq:1.5").unwrap().value, "1.5");
        assert_eq!(parse_condition("17:contains:1.5").unwrap().value, "1.5");
        for c in [
            "17:eq:true",
            "17:eq:false",
            "17:eq:0",
            "17:ne:0",
            "17:eq:-0.0",
            "17:eq:\"0\"",
        ] {
            assert!(parse_condition(c).is_err(), "{c}");
        }
        assert_eq!(parse_condition("17:gt:0").unwrap().value, 0);
        let q = TestCli::parse_from(["test", "--label-id", "8", "--label-id", "999"]).query;
        assert_eq!(
            q.conditions(),
            vec![
                json!({"model":"label","type":"=","value":[8]}),
                json!({"model":"label","type":"=","value":[999]})
            ]
        );
    }

    #[test]
    fn rejects_ambiguous_or_invalid_queries() {
        for flags in [
            vec!["--where", "0:eq:x"],
            vec!["--where", "3:eq:null"],
            vec!["--where", "3:empty:x"],
            vec!["--saved-filter", "3", "--label-id", "2"],
            vec!["--created-after", "not-a-date"],
        ] {
            assert!(TestCli::try_parse_from(std::iter::once("test").chain(flags)).is_err());
        }
        let q = TestCli::try_parse_from([
            "test",
            "--created-after",
            "2026-02-01",
            "--created-before",
            "2026-01-01",
        ])
        .unwrap()
        .query;
        assert!(q.validate().is_err());
    }
}
