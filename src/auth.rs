use crate::api::{ApiClient, DEFAULT_BASE_URL};
use crate::prompt::{prompt, prompt_secret};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use std::fs;
use std::path::{Path, PathBuf};

pub const CHALLENGE_USER_PASSWORD: &str = "USER_PASSWORD";
pub const CHALLENGE_SELECT_MFA_TYPE: &str = "SELECT_MFA_TYPE";
pub const CHALLENGE_SMS_MFA: &str = "SMS_MFA";
pub const CHALLENGE_SOFTWARE_TOKEN_MFA: &str = "SOFTWARE_TOKEN_MFA";
pub const CHALLENGE_NEW_PASSWORD_REQUIRED: &str = "NEW_PASSWORD_REQUIRED";
pub const CHALLENGE_SSO: &str = "SSO";

#[derive(Debug, Clone)]
pub struct LoginOptions {
    pub email: Option<String>,
    pub password: Option<String>,
    pub mfa_code: Option<String>,
    pub mfa_method: Option<String>,
    pub base_url: String,
    pub token_file: Option<PathBuf>,
    pub print_tokens: bool,
}

impl Default for LoginOptions {
    fn default() -> Self {
        Self {
            email: None,
            password: None,
            mfa_code: None,
            mfa_method: None,
            base_url: DEFAULT_BASE_URL.to_string(),
            token_file: default_token_file(),
            print_tokens: false,
        }
    }
}

#[derive(Debug, Clone)]
pub struct AuthClient {
    api: ApiClient,
}

impl AuthClient {
    pub fn new(base_url: impl Into<String>) -> Result<Self> {
        Ok(Self {
            api: ApiClient::new(base_url)?,
        })
    }

    pub fn initiate(&self, username: &str) -> Result<AuthResponse> {
        self.api.post(
            "/auth/initiate",
            &json!({ "Username": username.trim() }),
            None,
        )
    }

    pub fn respond(
        &self,
        challenge_name: &str,
        challenge_value: Value,
        extra: Option<Value>,
    ) -> Result<AuthResponse> {
        let mut body = Map::new();
        body.insert(
            "ChallengeName".to_string(),
            Value::String(challenge_name.to_string()),
        );
        body.insert("ChallengeValue".to_string(), challenge_value);

        if let Some(Value::Object(extra)) = extra {
            for (key, value) in extra {
                body.insert(key, value);
            }
        }

        self.api.post("/auth/respond", &Value::Object(body), None)
    }

    pub fn claim_code(&self, code: &str) -> Result<TokenSet> {
        let response: AuthResponse =
            self.api
                .get("/tokens/claim", &[("code", Some(code))], None)?;
        response.require_tokens()
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct AuthResponse {
    #[serde(default)]
    pub challenge_name: Option<String>,
    #[serde(default)]
    pub challenge_value: Option<Value>,
    #[serde(default)]
    pub session: Option<String>,
    #[serde(default)]
    pub message: Option<String>,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub advanced_security: Option<Value>,
    #[serde(default)]
    pub access_token: Option<String>,
    #[serde(default)]
    pub id_token: Option<String>,
    #[serde(default)]
    pub refresh_token: Option<String>,
    #[serde(default)]
    pub expires_in: Option<u64>,
    #[serde(default)]
    pub token_type: Option<String>,
}

impl AuthResponse {
    fn require_tokens(&self) -> Result<TokenSet> {
        Ok(TokenSet {
            access_token: self
                .access_token
                .clone()
                .context("login response did not include AccessToken")?,
            id_token: self
                .id_token
                .clone()
                .context("login response did not include IdToken")?,
            refresh_token: self
                .refresh_token
                .clone()
                .context("login response did not include RefreshToken")?,
            expires_in: self.expires_in,
            token_type: self.token_type.clone(),
        })
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct TokenSet {
    pub access_token: String,
    pub id_token: String,
    pub refresh_token: String,
    pub expires_in: Option<u64>,
    pub token_type: Option<String>,
}

pub fn login(options: LoginOptions) -> Result<TokenSet> {
    let client = AuthClient::new(&options.base_url)?;
    let email = options
        .email
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .map(Ok)
        .unwrap_or_else(|| prompt("Email address: "))?;

    let mut response = client.initiate(&email)?;

    loop {
        if let Some(error) = response.error.as_deref() {
            bail!("{error}");
        }

        match response.challenge_name.as_deref() {
            None => {
                let tokens = response.require_tokens()?;
                persist_tokens(&tokens, options.token_file.as_deref())?;

                if options.print_tokens {
                    println!("{}", serde_json::to_string_pretty(&tokens)?);
                } else {
                    println!("Logged in as {email}");
                }

                return Ok(tokens);
            }
            Some(CHALLENGE_USER_PASSWORD) => {
                if response.advanced_security.is_some() {
                    eprintln!(
                        "The web login requested Cognito advanced-security data. The CLI cannot produce the browser fingerprint yet; attempting password login without it."
                    );
                }

                if let Some(message) = response.message.as_deref() {
                    println!("{message}");
                }

                let password = options
                    .password
                    .clone()
                    .filter(|value| !value.is_empty())
                    .map(Ok)
                    .unwrap_or_else(|| prompt_secret("Password: "))?;

                response = client.respond(
                    CHALLENGE_USER_PASSWORD,
                    json!({
                        "Username": email,
                        "Password": password,
                    }),
                    None,
                )?;
            }
            Some(CHALLENGE_SELECT_MFA_TYPE) => {
                let session = response
                    .session
                    .clone()
                    .context("MFA selection challenge did not include Session")?;
                let username =
                    challenge_string(&response, "USER_ID_FOR_SRP").unwrap_or_else(|| email.clone());
                let mfa_method = choose_mfa_method(&response, options.mfa_method.as_deref())?;

                response = client.respond(
                    CHALLENGE_SELECT_MFA_TYPE,
                    json!({
                        "MfaMethod": mfa_method,
                        "Session": session,
                        "Username": username,
                    }),
                    None,
                )?;
            }
            Some(CHALLENGE_SOFTWARE_TOKEN_MFA) | Some(CHALLENGE_SMS_MFA) => {
                let challenge = response.challenge_name.clone().unwrap();
                let session = response
                    .session
                    .clone()
                    .context("MFA challenge did not include Session")?;
                let username =
                    challenge_string(&response, "USER_ID_FOR_SRP").unwrap_or_else(|| email.clone());

                let label = if challenge == CHALLENGE_SMS_MFA {
                    if let Some(destination) =
                        challenge_string(&response, "CODE_DELIVERY_DESTINATION")
                    {
                        format!("SMS MFA code sent to {destination}: ")
                    } else {
                        "SMS MFA code: ".to_string()
                    }
                } else {
                    "Software MFA code: ".to_string()
                };

                let mfa_code = options
                    .mfa_code
                    .clone()
                    .filter(|value| !value.is_empty())
                    .map(Ok)
                    .unwrap_or_else(|| prompt(&label))?;

                response = client.respond(
                    &challenge,
                    json!({
                        "MfaCode": mfa_code,
                        "Session": session,
                        "Username": username,
                    }),
                    None,
                )?;
            }
            Some(CHALLENGE_NEW_PASSWORD_REQUIRED) => {
                bail!(
                    "this account requires a new password; that challenge is not implemented in the CLI yet"
                );
            }
            Some(CHALLENGE_SSO) => {
                let url = challenge_string(&response, "URL")
                    .unwrap_or_else(|| "<missing SSO URL>".to_string());
                bail!("this account uses SSO; open {url} in a browser");
            }
            Some(other) => bail!("unsupported auth challenge: {other}"),
        }
    }
}

pub fn claim_app_tokens(base_url: &str, redirect_url: &str) -> Result<TokenSet> {
    let code = claim_code_from_redirect_url(redirect_url)?;
    AuthClient::new(base_url)?.claim_code(&code)
}

fn choose_mfa_method(response: &AuthResponse, configured: Option<&str>) -> Result<String> {
    if let Some(method) = configured.filter(|value| !value.is_empty()) {
        return Ok(method.to_string());
    }

    let options = challenge_string(response, "MFAS_CAN_CHOOSE")
        .and_then(|raw| serde_json::from_str::<Vec<String>>(&raw).ok())
        .unwrap_or_default();

    if options.len() == 1 {
        return Ok(options[0].clone());
    }

    if options
        .iter()
        .any(|value| value == CHALLENGE_SOFTWARE_TOKEN_MFA)
    {
        return Ok(CHALLENGE_SOFTWARE_TOKEN_MFA.to_string());
    }

    if options.iter().any(|value| value == CHALLENGE_SMS_MFA) {
        return Ok(CHALLENGE_SMS_MFA.to_string());
    }

    prompt("MFA method: ")
}

fn challenge_string(response: &AuthResponse, key: &str) -> Option<String> {
    match response.challenge_value.as_ref()? {
        Value::Object(map) => map
            .get(key)
            .and_then(Value::as_str)
            .map(ToString::to_string),
        _ => None,
    }
}

fn claim_code_from_redirect_url(redirect_url: &str) -> Result<String> {
    if let Ok(url) = reqwest::Url::parse(redirect_url) {
        if let Some((_, code)) = url.query_pairs().find(|(key, _)| key == "code") {
            return Ok(code.into_owned());
        }
    }

    let query = redirect_url
        .split_once('?')
        .map(|(_, query)| query)
        .unwrap_or(redirect_url);

    query
        .split('&')
        .filter_map(|part| part.split_once('='))
        .find(|(key, _)| *key == "code")
        .map(|(_, value)| value.to_string())
        .filter(|value| !value.trim().is_empty())
        .context("redirect URL did not include a claim code")
}

pub fn persist_tokens(tokens: &TokenSet, token_file: Option<&Path>) -> Result<()> {
    let Some(token_file) = token_file else {
        return Ok(());
    };

    if let Some(parent) = token_file.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }

    let data = serde_json::to_vec_pretty(tokens)?;
    fs::write(token_file, data)
        .with_context(|| format!("failed to write tokens to {}", token_file.display()))?;

    Ok(())
}

pub fn load_tokens(token_file: &Path) -> Result<TokenSet> {
    let data = fs::read(token_file)
        .with_context(|| format!("failed to read tokens from {}", token_file.display()))?;
    serde_json::from_slice(&data)
        .with_context(|| format!("failed to parse tokens from {}", token_file.display()))
}

pub fn default_token_file() -> Option<PathBuf> {
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"))?;
    Some(PathBuf::from(home).join(".config/gecko/tokens.json"))
}

pub fn default_app_token_file() -> Option<PathBuf> {
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"))?;
    Some(PathBuf::from(home).join(".config/gecko/app_tokens.json"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_challenge_values() {
        let response = AuthResponse {
            challenge_name: Some(CHALLENGE_SOFTWARE_TOKEN_MFA.to_string()),
            challenge_value: Some(json!({ "USER_ID_FOR_SRP": "abc" })),
            session: Some("session".to_string()),
            message: None,
            error: None,
            advanced_security: None,
            access_token: None,
            id_token: None,
            refresh_token: None,
            expires_in: None,
            token_type: None,
        };

        assert_eq!(
            challenge_string(&response, "USER_ID_FOR_SRP"),
            Some("abc".to_string())
        );
    }

    #[test]
    fn extracts_claim_code_from_redirect_url() {
        let code = claim_code_from_redirect_url("https://app-stage.geckointernal.com/?code=abc123")
            .unwrap();

        assert_eq!(code, "abc123");
    }

    #[test]
    fn requires_complete_tokens() {
        let response = AuthResponse {
            challenge_name: None,
            challenge_value: None,
            session: None,
            message: None,
            error: None,
            advanced_security: None,
            access_token: Some("access".to_string()),
            id_token: Some("id".to_string()),
            refresh_token: Some("refresh".to_string()),
            expires_in: Some(3600),
            token_type: Some("Bearer".to_string()),
        };

        let tokens = response.require_tokens().unwrap();
        assert_eq!(tokens.access_token, "access");
        assert_eq!(tokens.id_token, "id");
        assert_eq!(tokens.refresh_token, "refresh");
    }
}
