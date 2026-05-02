use anyhow::{Context, Result, anyhow, bail};
use reqwest::blocking::{Client, RequestBuilder};
use serde::de::DeserializeOwned;
use serde_json::Value;

pub const DEFAULT_BASE_URL: &str = "https://account-api-stage.geckoengage.com";

#[derive(Debug, Clone)]
pub struct ApiClient {
    base_url: String,
    http: Client,
}

impl ApiClient {
    pub fn new(base_url: impl Into<String>) -> Result<Self> {
        let base_url = normalize_base_url(base_url.into())?;
        let http = Client::builder()
            .user_agent("gecko-cli/0.1.0")
            .build()
            .context("failed to build HTTP client")?;

        Ok(Self { base_url, http })
    }

    pub fn get<T>(
        &self,
        endpoint: &str,
        query: &[(&str, Option<&str>)],
        bearer_token: Option<&str>,
    ) -> Result<T>
    where
        T: DeserializeOwned,
    {
        let mut request = self
            .http
            .get(self.url(endpoint)?)
            .header("Accept", "application/json");

        for (key, value) in query {
            if let Some(value) = value {
                request = request.query(&[(key, value)]);
            }
        }

        self.send_json(request, bearer_token)
    }

    pub fn post<T>(&self, endpoint: &str, body: &Value, bearer_token: Option<&str>) -> Result<T>
    where
        T: DeserializeOwned,
    {
        let request = self
            .http
            .post(self.url(endpoint)?)
            .header("Accept", "application/json")
            .json(body);

        self.send_json(request, bearer_token)
    }

    fn send_json<T>(&self, request: RequestBuilder, bearer_token: Option<&str>) -> Result<T>
    where
        T: DeserializeOwned,
    {
        let request = if let Some(token) = bearer_token {
            request.bearer_auth(token)
        } else {
            request
        };

        let response = request.send().context("account API request failed")?;
        let status = response.status();
        let payload: Value = response.json().with_context(|| {
            format!("account API returned non-JSON response with status {status}")
        })?;

        if !status.is_success() {
            return Err(
                api_error(&payload).unwrap_or_else(|| anyhow!("account API returned {status}"))
            );
        }

        serde_json::from_value(payload).context("account API returned an unexpected response")
    }

    fn url(&self, endpoint: &str) -> Result<String> {
        if !endpoint.starts_with('/') {
            bail!("API endpoint must start with /");
        }

        Ok(format!("{}{}", self.base_url, endpoint))
    }
}

fn api_error(payload: &Value) -> Option<anyhow::Error> {
    payload
        .get("Error")
        .or_else(|| payload.get("Message"))
        .and_then(Value::as_str)
        .map(|message| anyhow!(message.to_string()))
}

pub fn normalize_base_url(mut value: String) -> Result<String> {
    if value.trim().is_empty() {
        value = DEFAULT_BASE_URL.to_string();
    }

    let normalized = value.trim().trim_end_matches('/').to_string();
    if !(normalized.starts_with("http://") || normalized.starts_with("https://")) {
        bail!("base URL must start with http:// or https://");
    }

    Ok(normalized)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_base_urls() {
        assert_eq!(
            normalize_base_url("https://account-api-stage.geckoengage.com/".to_string()).unwrap(),
            "https://account-api-stage.geckoengage.com"
        );
    }

    #[test]
    fn rejects_base_urls_without_scheme() {
        assert!(normalize_base_url("account-api-stage.geckoengage.com".to_string()).is_err());
    }
}
