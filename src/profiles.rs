use crate::api::ApiClient;
use crate::auth::TokenSet;
use crate::session::AppSession;
use crate::tui::run_profile_selector_tui;
use anyhow::{Context, Result, bail};
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::HashMap;

#[derive(Debug, Clone)]
pub struct ProfileSelectionOptions {
    pub client: String,
    pub app: Option<String>,
    pub profile_id: Option<String>,
    pub auto_select_single: bool,
    pub print_redirect_url: bool,
}

impl Default for ProfileSelectionOptions {
    fn default() -> Self {
        Self {
            client: "web".to_string(),
            app: None,
            profile_id: None,
            auto_select_single: false,
            print_redirect_url: true,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ProfileService {
    api: ApiClient,
}

impl ProfileService {
    pub fn new(base_url: impl Into<String>) -> Result<Self> {
        Ok(Self {
            api: ApiClient::new(base_url)?,
        })
    }

    pub fn saturate(
        &self,
        tokens: &TokenSet,
        client: &str,
        app: Option<&str>,
    ) -> Result<SaturateResponse> {
        self.api.get(
            "/user/saturate",
            &[("client", Some(client)), ("app", app)],
            Some(&tokens.id_token),
        )
    }

    pub fn redirect_url(
        &self,
        tokens: &TokenSet,
        profile_id: Value,
        client: &str,
        app: Option<&str>,
    ) -> Result<RedirectResponse> {
        self.api.post(
            "/user/redirect",
            &json!({
                "client": client,
                "app": app,
                "ProfileId": profile_id,
            }),
            Some(&tokens.id_token),
        )
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct SaturateResponse {
    #[serde(default)]
    pub accounts: Vec<Account>,
    #[serde(default)]
    pub apps: Vec<App>,
    #[serde(default)]
    pub profiles: Vec<Profile>,
    #[serde(default)]
    pub user: Value,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Account {
    #[serde(rename = "AccountId")]
    pub account_id: Value,
    #[serde(rename = "Name")]
    pub name: String,
    #[serde(rename = "IsClosed", default)]
    pub is_closed: bool,
    #[serde(flatten)]
    pub extra: HashMap<String, Value>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct App {
    #[serde(rename = "Id")]
    pub id: Value,
    #[serde(rename = "Description", default)]
    pub description: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Profile {
    #[serde(rename = "ProfileId")]
    pub profile_id: Value,
    #[serde(rename = "AccountId")]
    pub account_id: Value,
    #[serde(rename = "AppId")]
    pub app_id: Value,
    #[serde(default)]
    pub unavailable: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct RedirectResponse {
    pub redirect_url: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProfileChoice {
    pub profile_id: Value,
    pub account_id: Value,
    pub account_header: String,
    pub account_name: String,
    pub app_description: String,
    pub is_closed: bool,
}

pub fn select_profile_after_login(
    base_url: &str,
    tokens: &TokenSet,
    options: ProfileSelectionOptions,
) -> Result<AppSession> {
    let service = ProfileService::new(base_url)?;
    let data = service.saturate(tokens, &options.client, options.app.as_deref())?;
    let choices = build_profile_choices(&data)?;

    if choices.is_empty() {
        bail!("No Profiles for your account found");
    }

    let selected = select_choice(&choices, &options)?;
    let redirect = service.redirect_url(
        tokens,
        selected.profile_id.clone(),
        &options.client,
        options.app.as_deref(),
    )?;

    if options.print_redirect_url {
        println!("Redirect URL: {}", redirect.redirect_url);
    }

    Ok(AppSession {
        profile_id: display_id(&selected.profile_id),
        account_id: selected.account_header.clone(),
        account_name: selected.account_name.clone(),
        app_description: selected.app_description.clone(),
        user_id: user_id(&data)?,
        redirect_url: redirect.redirect_url,
    })
}

pub fn build_profile_choices(data: &SaturateResponse) -> Result<Vec<ProfileChoice>> {
    let accounts = data
        .accounts
        .iter()
        .map(|account| (id_key(&account.account_id), account))
        .collect::<HashMap<_, _>>();
    let apps = data
        .apps
        .iter()
        .map(|app| (id_key(&app.id), app))
        .collect::<HashMap<_, _>>();

    let mut open = Vec::new();
    let mut closed = Vec::new();

    for profile in data.profiles.iter().filter(|profile| !profile.unavailable) {
        let account = accounts
            .get(&id_key(&profile.account_id))
            .with_context(|| format!("missing account {}", display_id(&profile.account_id)))?;
        let app = apps
            .get(&id_key(&profile.app_id))
            .with_context(|| format!("missing app {}", display_id(&profile.app_id)))?;
        let choice = ProfileChoice {
            profile_id: profile.profile_id.clone(),
            account_id: profile.account_id.clone(),
            account_header: account_header(account),
            account_name: account.name.clone(),
            app_description: app.description.clone(),
            is_closed: account.is_closed,
        };

        if account.is_closed {
            closed.push(choice);
        } else {
            open.push(choice);
        }
    }

    open.extend(closed);
    Ok(open)
}

#[cfg(test)]
pub fn render_profile_choices(choices: &[ProfileChoice]) -> String {
    let mut output = String::from("Profiles:\n");
    let mut open_index = 1;
    let mut wrote_closed_header = false;

    for choice in choices {
        if choice.is_closed {
            if !wrote_closed_header {
                output.push_str("\nClosed accounts:\n");
                wrote_closed_header = true;
            }
            output.push_str(&format!(
                "  - {} - {}\n",
                choice.account_name, choice.app_description
            ));
        } else {
            output.push_str(&format!(
                "  {open_index}. {} - {}\n",
                choice.account_name, choice.app_description
            ));
            open_index += 1;
        }
    }

    output.trim_end().to_string()
}

fn select_choice<'a>(
    choices: &'a [ProfileChoice],
    options: &ProfileSelectionOptions,
) -> Result<&'a ProfileChoice> {
    let open_choices = choices
        .iter()
        .filter(|choice| !choice.is_closed)
        .collect::<Vec<_>>();

    if open_choices.is_empty() {
        bail!("no selectable open profiles found");
    }

    if let Some(profile_id) = options.profile_id.as_deref() {
        return open_choices
            .into_iter()
            .find(|choice| id_matches(&choice.profile_id, profile_id))
            .with_context(|| format!("profile {profile_id} was not found or is not selectable"));
    }

    if options.auto_select_single && open_choices.len() == 1 {
        return Ok(open_choices[0]);
    }

    let index = run_profile_selector_tui(&open_choices)?;
    Ok(open_choices[index])
}

fn id_matches(value: &Value, expected: &str) -> bool {
    match value {
        Value::String(value) => value == expected,
        _ => value.to_string() == expected,
    }
}

fn id_key(value: &Value) -> String {
    match value {
        Value::String(value) => value.clone(),
        _ => value.to_string(),
    }
}

fn display_id(value: &Value) -> String {
    match value {
        Value::String(value) => value.clone(),
        _ => value.to_string(),
    }
}

fn account_header(account: &Account) -> String {
    ["routing_id", "RoutingId", "routingId"]
        .iter()
        .filter_map(|key| account.extra.get(*key))
        .find_map(display_header_value)
        .unwrap_or_else(|| display_id(&account.account_id))
}

fn user_id(data: &SaturateResponse) -> Result<String> {
    let user = data
        .user
        .as_object()
        .context("profile data did not include a user object")?;

    ["id", "Id", "UserId", "user_id"]
        .iter()
        .filter_map(|key| user.get(*key))
        .find_map(display_header_value)
        .context("profile data did not include a user id")
}

fn display_header_value(value: &Value) -> Option<String> {
    match value {
        Value::String(value) if !value.trim().is_empty() => Some(value.clone()),
        Value::Number(value) => Some(value.to_string()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_saturate_response() -> SaturateResponse {
        serde_json::from_value(json!({
            "accounts": [
                { "AccountId": 1, "Name": "Open Account", "IsClosed": false, "routing_id": 281 },
                { "AccountId": 2, "Name": "Closed Account", "IsClosed": true }
            ],
            "apps": [
                { "Id": "forms", "Description": "Forms, Events, Call Centre, Email & Text Campaigns", "Icon": "/forms.svg" },
                { "Id": "chat", "Description": "Chat & Chatbot", "Icon": "/chat.svg" }
            ],
            "profiles": [
                { "ProfileId": "p-unavailable", "AccountId": 1, "AppId": "chat", "unavailable": true },
                { "ProfileId": "p-open", "AccountId": 1, "AppId": "forms" },
                { "ProfileId": "p-closed", "AccountId": 2, "AppId": "chat" }
            ],
            "user": { "id": 2260 }
        }))
        .unwrap()
    }

    #[test]
    fn builds_profile_choices_in_open_then_closed_order() {
        let choices = build_profile_choices(&sample_saturate_response()).unwrap();

        assert_eq!(choices.len(), 2);
        assert_eq!(choices[0].account_name, "Open Account");
        assert_eq!(choices[0].account_header, "281");
        assert_eq!(
            choices[0].app_description,
            "Forms, Events, Call Centre, Email & Text Campaigns"
        );
        assert!(!choices[0].is_closed);
        assert_eq!(choices[1].account_name, "Closed Account");
        assert!(choices[1].is_closed);
    }

    #[test]
    fn renders_open_profiles_as_numbered_and_closed_profiles_unselectable() {
        let choices = build_profile_choices(&sample_saturate_response()).unwrap();
        let rendered = render_profile_choices(&choices);

        assert!(
            rendered
                .contains("1. Open Account - Forms, Events, Call Centre, Email & Text Campaigns")
        );
        assert!(rendered.contains("Closed accounts:"));
        assert!(rendered.contains("- Closed Account - Chat & Chatbot"));
    }

    #[test]
    fn can_select_by_profile_id_without_prompting() {
        let choices = build_profile_choices(&sample_saturate_response()).unwrap();
        let selected = select_choice(
            &choices,
            &ProfileSelectionOptions {
                profile_id: Some("p-open".to_string()),
                ..ProfileSelectionOptions::default()
            },
        )
        .unwrap();

        assert_eq!(selected.account_name, "Open Account");
    }

    #[test]
    fn closed_profiles_cannot_be_selected_by_profile_id() {
        let choices = build_profile_choices(&sample_saturate_response()).unwrap();
        let selected = select_choice(
            &choices,
            &ProfileSelectionOptions {
                profile_id: Some("p-closed".to_string()),
                ..ProfileSelectionOptions::default()
            },
        );

        assert!(selected.is_err());
    }

    #[test]
    fn extracts_user_id_from_profile_data() {
        let user_id = user_id(&sample_saturate_response()).unwrap();

        assert_eq!(user_id, "2260");
    }
}
