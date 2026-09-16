use crate::{auth, session};
use anyhow::{Context, Result, bail, ensure};
use reqwest::{Method, Url, blocking::Client, redirect::Policy};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::BTreeSet, time::Duration};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Target {
    pub base_url: String,
    pub account_id: String,
    pub profile_id: String,
}

pub struct GeckoApi {
    pub base_url: String,
    http: Client,
    session: session::AppSession,
    identity: std::sync::OnceLock<crate::app_identity::ApiIdentity>,
}

impl GeckoApi {
    pub fn target(&self) -> Target {
        Target {
            base_url: self.base_url.clone(),
            account_id: self.session.account_id.clone(),
            profile_id: self.session.profile_id.clone(),
        }
    }

    pub fn new(
        base_url: &str,
        tokens: &auth::TokenSet,
        selected: &session::AppSession,
    ) -> Result<Self> {
        crate::app_identity::validate_token_profile(tokens, selected)?;
        let base_url = crate::api::normalize_base_url(base_url.to_string())?;
        let url = Url::parse(&base_url).context("invalid app API URL")?;
        ensure!(
            url.host_str().is_some()
                && url.username().is_empty()
                && url.password().is_none()
                && url.query().is_none()
                && url.fragment().is_none(),
            "app API URL must have a host and no credentials, query or fragment"
        );
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert("Accept", "application/json".parse()?);
        let mut bearer: reqwest::header::HeaderValue = format!("Bearer {}", tokens.access_token)
            .parse()
            .context("invalid app token")?;
        bearer.set_sensitive(true);
        headers.insert(reqwest::header::AUTHORIZATION, bearer);
        let http = Client::builder()
            .user_agent("cli_tools/0.1.0")
            .default_headers(headers)
            .redirect(Policy::none())
            .timeout(Duration::from_secs(30))
            .build()?;
        Ok(Self {
            base_url,
            http,
            session: selected.clone(),
            identity: std::sync::OnceLock::new(),
        })
    }

    pub fn request(
        &self,
        method: Method,
        endpoint: &str,
        query: &[(&str, String)],
        body: Option<&Value>,
    ) -> Result<Value> {
        Ok(self.response(method, endpoint, query, body)?.1)
    }

    pub fn response(
        &self,
        method: Method,
        endpoint: &str,
        query: &[(&str, String)],
        body: Option<&Value>,
    ) -> Result<(reqwest::header::HeaderMap, Value)> {
        if self.identity.get().is_none() {
            let identity = crate::app_identity::confirm_session(
                self.http.get(format!("{}/auth/check", self.base_url)),
                &self.session,
            )?;
            let _ = self.identity.set(identity);
        }
        let identity = self.identity.get().context("missing app API identity")?;
        let mut request = self
            .http
            .request(method, format!("{}/{endpoint}", self.base_url))
            .header("Gecko-Account", &identity.account_id)
            .header("Gecko-User", &identity.user_id)
            .query(query);
        if let Some(body) = body {
            request = request.json(body);
        }
        let response = request.send().context("Gecko API request failed")?;
        let status = response.status();
        let headers = response.headers().clone();
        if status.is_success() {
            crate::app_identity::validate_account(response.headers(), &identity.account_id)?;
        }
        let payload: Value = response
            .json()
            .with_context(|| format!("Gecko API returned non-JSON with HTTP {status}"))?;
        if !status.is_success() {
            let message = ["error", "message", "Error", "Message"]
                .iter()
                .find_map(|key| payload.get(key).and_then(Value::as_str))
                .unwrap_or("request rejected");
            bail!("Gecko API returned HTTP {status}: {message}");
        }
        Ok((headers, payload))
    }

    pub fn fields(&self) -> Result<Vec<Value>> {
        let fields = self.collection("fields", &[("field_type", "contact".into())])?;
        for field in &fields {
            ensure!(
                field["field_type"] == "contact",
                "API returned a non-contact field"
            );
        }
        Ok(fields)
    }
    pub fn collection(&self, endpoint: &str, query: &[(&str, String)]) -> Result<Vec<Value>> {
        let mut result = Vec::new();
        let mut seen = BTreeSet::new();
        for page in 1..=1000 {
            let mut query = query.to_vec();
            query.extend([("per_page", "100".into()), ("page", page.to_string())]);
            let payload = self.request(Method::GET, endpoint, &query, None)?;
            let items = payload
                .get(endpoint)
                .or_else(|| payload.get("data"))
                .unwrap_or(&payload)
                .as_array()
                .context("collection response must contain an array")?;
            if items.is_empty() {
                return Ok(result);
            }
            for item in items {
                ensure!(
                    seen.insert(resource_id(item)?),
                    "{endpoint} pagination repeated an ID"
                );
                result.push(item.clone());
            }
        }
        bail!("{endpoint} pagination exceeded 1000 pages")
    }
}

pub fn resource_id(resource: &Value) -> Result<String> {
    let id = match &resource["id"] {
        Value::String(value) => value.clone(),
        Value::Number(value) if value.as_u64().is_some() => value.to_string(),
        _ => bail!("API resource has no usable id"),
    };
    ensure!(
        !id.is_empty() && id != "0" && id.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-'),
        "API resource has an invalid id"
    );
    Ok(id)
}
