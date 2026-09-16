use crate::{auth::TokenSet, session::AppSession};
use anyhow::{Context, Result, ensure};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use reqwest::{blocking::RequestBuilder, header::HeaderMap};
use serde_json::Value;

/// A local consistency check, not signature verification: Gecko verifies the JWT, and its
/// account response header must also match before we use data or write anything.
pub fn validate_token_profile(tokens: &TokenSet, session: &AppSession) -> Result<()> {
    let parts = tokens.access_token.split('.').collect::<Vec<_>>();
    ensure!(
        parts.len() == 3 && parts.iter().all(|part| !part.is_empty()),
        "saved app token is not a JWT; select the profile again to renew app tokens"
    );
    let bytes = URL_SAFE_NO_PAD
        .decode(parts[1])
        .context("invalid app token claims")?;
    let claims: Value = serde_json::from_slice(&bytes).context("invalid app token claims")?;
    let app = claims.get("apps").and_then(|apps| {
        ["engage", "form", "manage"]
            .iter()
            .find_map(|key| apps.get(key))
    });
    let profile = app
        .and_then(|app| app.get("profile"))
        .filter(|value| !value.is_null())
        .or_else(|| claims.get("profile"))
        .context("app token has no profile; select the profile again")?;
    let profile = identity_value(profile).context("app token has an invalid profile claim")?;
    ensure!(
        profile == session.user_id,
        "app token profile {} does not match saved app user {}; run `cli_tools profiles` to select the profile again",
        profile,
        session.user_id
    );
    let account = app
        .and_then(|app| app.get("account"))
        .filter(|value| !value.is_null())
        .or_else(|| claims.get("account"))
        .and_then(identity_value)
        .context("app token has no valid account claim")?;
    ensure!(
        account == session.account_id,
        "app token account {} does not match saved account {}; select the profile again",
        account,
        session.account_id
    );
    Ok(())
}

#[derive(Debug, Clone)]
pub struct ApiIdentity {
    pub account_id: String,
    pub user_id: String,
}

/// Resolve the public account/user IDs to the numeric IDs used in Gecko's headers.
/// The request must be a bearer-authenticated GET to the app API's /auth/check.
pub fn confirm_session(request: RequestBuilder, session: &AppSession) -> Result<ApiIdentity> {
    let response = request
        .header("Accept", "application/json")
        .query(&[
            ("account_rfields", "uuid,routing_id"),
            ("user_rfields", "id,auth_id"),
        ])
        .send()
        .context("app identity request failed")?;
    let status = response.status();
    let headers = response.headers().clone();
    let payload: Value = response
        .json()
        .with_context(|| format!("app identity API returned non-JSON with status {status}"))?;
    ensure!(
        status.is_success(),
        "app identity API returned HTTP {status}; select the profile again if its token has expired"
    );
    let account = &payload["token"]["account"];
    let user = &payload["token"]["user"];
    ensure!(
        identity_value(&account["uuid"]).as_deref() == Some(session.account_id.as_str()),
        "Gecko authenticated account does not match the selected account; refusing to use this API target"
    );
    ensure!(
        identity_value(&user["auth_id"]).as_deref() == Some(session.user_id.as_str()),
        "Gecko authenticated user does not match the selected app user; select the profile again"
    );
    let identity = ApiIdentity {
        account_id: numeric_id(&account["routing_id"])
            .context("Gecko account has no routing ID")?,
        user_id: numeric_id(&user["id"]).context("Gecko user has no numeric ID")?,
    };
    validate_account(&headers, &identity.account_id)?;
    Ok(identity)
}

fn identity_value(value: &Value) -> Option<String> {
    match value {
        Value::String(value) if !value.trim().is_empty() => Some(value.clone()),
        Value::Number(value) => Some(value.to_string()),
        _ => None,
    }
}

fn numeric_id(value: &Value) -> Option<String> {
    let id = identity_value(value)?;
    (id.parse::<u64>().ok()? > 0).then_some(id)
}

pub fn validate_account(headers: &HeaderMap, expected_account: &str) -> Result<()> {
    let actual = headers
        .get("Gecko-Account")
        .and_then(|header| header.to_str().ok())
        .context("Gecko response did not confirm its account; refusing to use this API target")?;
    ensure!(
        actual == expected_account,
        "Gecko authenticated account {} does not match selected account {}; refusing to use this API target",
        actual,
        expected_account
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn check(mut claims: Value, expected: &str) -> Result<()> {
        if claims.get("account").is_none() {
            claims["account"] = json!("test-account-uuid");
        }
        let mut tokens = crate::test_support::app_tokens("unused");
        tokens.access_token = format!(
            "e30.{}.test-signature",
            URL_SAFE_NO_PAD.encode(claims.to_string())
        );
        validate_token_profile(
            &tokens,
            &AppSession {
                profile_id: "auth-profile-uuid".into(),
                account_id: "test-account-uuid".into(),
                account_name: "Development".into(),
                user_id: expected.into(),
                app_description: "Forms".into(),
                redirect_url: String::new(),
            },
        )
    }

    #[test]
    fn matches_geckos_app_claim_precedence_and_fallbacks() {
        for (claims, expected) in [
            (json!({"profile": "root"}), "root"),
            (json!({"profile": 123}), "123"),
            (
                json!({"profile": "root", "apps": {"form": {"profile": "form"}, "engage": {"profile": "engage"}, "manage": {"profile": "manage"}}}),
                "engage",
            ),
            (
                json!({"apps": {"form": {"profile": "form"}, "manage": {"profile": "manage"}}}),
                "form",
            ),
            (json!({"apps": {"manage": {"profile": "manage"}}}), "manage"),
            (
                json!({"profile": "root", "apps": {"engage": {"account": "test-account-uuid"}, "form": {"profile": "form"}}}),
                "root",
            ),
        ] {
            check(claims, expected).unwrap();
        }
        assert!(check(json!({"profile": "other"}), "selected").is_err());
        let null_app =
            json!({"profile": "root", "apps": {"engage": null, "form": {"profile": "form"}}});
        check(null_app.clone(), "root").unwrap();
        assert!(check(null_app, "form").is_err());
        assert!(check(json!({}), "selected").is_err());
        assert!(check(json!({"profile": false}), "selected").is_err());
    }

    #[test]
    fn requires_the_server_to_confirm_the_selected_account() {
        let mut headers = HeaderMap::new();
        assert!(validate_account(&headers, "281").is_err());
        headers.insert("Gecko-Account", "999".parse().unwrap());
        assert!(validate_account(&headers, "281").is_err());
        headers.insert("Gecko-Account", "281".parse().unwrap());
        validate_account(&headers, "281").unwrap();
    }

    #[test]
    fn checks_the_effective_account_as_well_as_the_external_profile() {
        check(json!({"profile": "app-user"}), "app-user").unwrap();
        assert!(
            check(
                json!({"profile": "app-user", "account": "other-account"}),
                "app-user"
            )
            .is_err()
        );
        assert!(
            check(
                json!({"profile": "app-user", "apps": {"engage": {"account": "other-account"}}}),
                "app-user"
            )
            .is_err()
        );
        check(json!({"profile": "app-user", "account": "root-account", "apps": {"engage": {"account": "test-account-uuid"}}}), "app-user").unwrap();
        // Auth ProfileId cannot stand in for the app's ExternalId.
        assert!(check(json!({"profile": "auth-profile-uuid"}), "app-user").is_err());
    }

    #[test]
    fn resolves_public_ids_to_the_authenticated_numeric_ids() {
        let server =
            crate::test_support::Server::new(vec![(200, crate::test_support::auth_identity())]);
        let identity = confirm_session(
            reqwest::blocking::Client::new().get(format!("{}/auth/check", server.url)),
            &test_session(),
        )
        .unwrap();
        assert_eq!(identity.account_id, "281");
        assert_eq!(identity.user_id, "2260");
        assert_eq!(server.finish().len(), 1);
    }

    #[test]
    fn rejects_inconsistent_server_identity_mappings() {
        for (field, value) in [
            ("/token/account/uuid", json!("other-account")),
            ("/token/user/auth_id", json!("other-user")),
            ("/token/account/routing_id", json!(999)),
            ("/token/account/routing_id", Value::Null),
            ("/token/user/id", json!(0)),
        ] {
            let mut payload = crate::test_support::auth_identity();
            *payload.pointer_mut(field).unwrap() = value;
            let server = crate::test_support::Server::new(vec![(200, payload)]);
            assert!(
                confirm_session(
                    reqwest::blocking::Client::new().get(format!("{}/auth/check", server.url)),
                    &test_session(),
                )
                .is_err(),
                "{field}"
            );
            assert_eq!(server.finish().len(), 1);
        }
    }

    fn test_session() -> AppSession {
        AppSession {
            profile_id: "auth-profile-uuid".into(),
            account_id: "test-account-uuid".into(),
            user_id: "app-user-1".into(),
            account_name: "Development".into(),
            app_description: "Forms".into(),
            redirect_url: String::new(),
        }
    }
}
