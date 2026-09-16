use crate::{auth::TokenSet, session::AppSession};
use anyhow::{Context, Result, ensure};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use reqwest::header::HeaderMap;
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
    let profile = match profile {
        Value::String(profile) => profile.clone(),
        Value::Number(profile) => profile.to_string(),
        _ => anyhow::bail!("app token has an invalid profile claim"),
    };
    ensure!(
        profile == session.profile_id,
        "app token profile {} does not match saved profile {}; select the profile again or use its matching app token file",
        profile,
        session.profile_id
    );
    Ok(())
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

    fn check(claims: Value, expected: &str) -> Result<()> {
        let mut tokens = crate::test_support::app_tokens("unused");
        tokens.access_token = format!(
            "e30.{}.test-signature",
            URL_SAFE_NO_PAD.encode(claims.to_string())
        );
        validate_token_profile(
            &tokens,
            &AppSession {
                profile_id: expected.into(),
                account_id: "281".into(),
                account_name: "Development".into(),
                user_id: "2260".into(),
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
                json!({"profile": "root", "apps": {"engage": {"account": "account"}, "form": {"profile": "form"}}}),
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
}
