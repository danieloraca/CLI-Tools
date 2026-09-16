use super::*;
use crate::{
    scenarios::{GenerateArgs, fixture::generate},
    test_support::{Directory, Server},
};

pub(super) fn fixture() -> Scenario {
    generate(&GenerateArgs {
        request: None,
        name: "verify-test".into(),
        seed: 42,
        contacts: Some(3),
        duplicate_emails: Some(1),
        missing_emails: Some(1),
        custom_field: vec![],
        custom_fields: true,
        restricted_permissions: true,
        output: None,
    })
    .unwrap()
}
fn setup(dir: &Directory, url: &str, scenario: &Scenario) -> (ApplyArgs, State) {
    let session = crate::session::AppSession {
        profile_id: "p-1".into(),
        account_id: "test-account-uuid".into(),
        user_id: "app-user-1".into(),
        account_name: "Development".into(),
        app_description: "Forms".into(),
        redirect_url: String::new(),
    };
    let session_file = dir.path("session.json");
    let app_token_file = dir.path("tokens.json");
    std::fs::write(&session_file, serde_json::to_vec(&session).unwrap()).unwrap();
    std::fs::write(
        &app_token_file,
        serde_json::to_vec(&crate::test_support::app_tokens("app-user-1")).unwrap(),
    )
    .unwrap();
    let args = ApplyArgs {
        fixture: dir.path("fixture.json"),
        profile_id: "p-1".into(),
        app_api_base_url: url.into(),
        app_token_file: Some(app_token_file),
        session_file: Some(session_file),
        state_file: Some(dir.path("run.apply-state.json")),
    };
    let api = Connection {
        app_api_base_url: url.into(),
        app_token_file: args.app_token_file.clone(),
        session_file: args.session_file.clone(),
    }
    .open()
    .unwrap();
    let mut state =
        apply::load_state(args.state_file.as_ref().unwrap(), api.target(), scenario).unwrap();
    for (index, field) in scenario.fields.iter().enumerate() {
        if !matches!(field.kind, FieldType::Name | FieldType::Email) {
            state
                .completed
                .insert(format!("field:{}", field.key), (index + 1).to_string());
        }
    }
    for group in &scenario.groups {
        state
            .completed
            .insert(format!("group:{}", group.key), "9".into());
    }
    for (index, contact) in scenario.contacts.iter().enumerate() {
        for prefix in ["contact", "populated"] {
            state.completed.insert(
                format!("{prefix}:{}", contact.key),
                (101 + index).to_string(),
            );
        }
    }
    std::fs::write(
        args.state_file.as_ref().unwrap(),
        serde_json::to_vec_pretty(&state).unwrap(),
    )
    .unwrap();
    (args, state)
}
fn responses(scenario: &Scenario) -> Vec<(u16, Value)> {
    let fields:Vec<_>=scenario.fields.iter().enumerate().map(|(i,f)|json!({"id":i+1,"type":f.kind.as_str(),"field_type":"contact","label":format!("{}: {}",scenario.name,f.label),"is_sensitive":false,"required":false,"matchable":false})).collect();
    let mut result = vec![
        (200, json!({"fields":fields})),
        (200, json!({"fields":[]})),
        (
            200,
            json!({"group":{"id":9,"name":scenario.groups[0].name,"permissions":[{"id":1,"key":"contacts_view","always":false},{"id":2,"key":"mandatory","always":true}]}}),
        ),
    ];
    for (i, c) in scenario.contacts.iter().enumerate() {
        let values:Vec<_>=scenario.fields.iter().enumerate().filter(|(_,f)|!c.fields[&f.key].is_null()).map(|(i,f)|json!({"contact_field_id":i+1,"field_id":i+1,"field":{"id":i+1,"is_sensitive":false},"value":c.fields[&f.key]})).collect();
        result.push((200, json!({"contact":{"id":101+i,"current_values":values}})));
    }
    result
}
#[test]
fn verifies_resources_typed_values_duplicate_and_missing_emails_without_writes() {
    let scenario = fixture();
    let dir = Directory::new();
    let server = Server::with_identity(responses(&scenario));
    let (args, _) = setup(&dir, &server.url, &scenario);
    let before = std::fs::read(args.state_file.as_ref().unwrap()).unwrap();
    let (api, _, _lock, state) = prepare(&args, &scenario).unwrap();
    let report = verify(&api, &state, &scenario).unwrap();
    assert!(report.ok, "{report:?}");
    assert_eq!(report.contacts_checked, 3);
    assert_eq!(
        report.expected_emails,
        EmailCounts {
            duplicates: 1,
            missing: 1
        }
    );
    assert_eq!(
        report.observed_emails,
        Some(EmailCounts {
            duplicates: 1,
            missing: 1
        })
    );
    assert_eq!(
        std::fs::read(args.state_file.as_ref().unwrap()).unwrap(),
        before
    );
    assert!(server.finish().iter().all(|r| r.line.starts_with("GET ")));
}
#[test]
fn reports_missing_changed_and_restricted_values_without_printing_them() {
    for case in ["missing", "changed", "restricted"] {
        let scenario = fixture();
        let mut replies = responses(&scenario);
        match case {
            "missing" => replies[3] = (404, json!({"message":"missing"})),
            "changed" => {
                replies[3].1["contact"]["current_values"][0]["value"] =
                    json!("secret changed value")
            }
            "restricted" => replies[0].1["fields"][1]["is_sensitive"] = json!(true),
            _ => unreachable!(),
        }
        let server = Server::with_identity(replies);
        let dir = Directory::new();
        let (args, _) = setup(&dir, &server.url, &scenario);
        let (api, _, _lock, state) = prepare(&args, &scenario).unwrap();
        let report = verify(&api, &state, &scenario).unwrap();
        assert!(!report.ok);
        let encoded = serde_json::to_string(&report).unwrap();
        assert!(!encoded.contains("secret changed value"));
        assert!(!encoded.contains("example.test"));
        assert!(report.issues.iter().any(|i| i.kind
            == match case {
                "missing" => "missing",
                "changed" => "mismatch",
                _ => "unverified",
            }));
        if case == "restricted" {
            assert!(report.observed_emails.is_none());
        }
        server.finish();
    }
}
#[test]
fn incomplete_and_uncertain_journals_do_not_pass() {
    let scenario = fixture();
    let server = Server::with_identity(responses(&scenario));
    let dir = Directory::new();
    let (args, mut state) = setup(&dir, &server.url, &scenario);
    state.pending = Some("populated:pending".into());
    state
        .completed
        .remove(&format!("populated:{}", scenario.contacts[0].key));
    let api = Connection {
        app_api_base_url: args.app_api_base_url,
        app_token_file: args.app_token_file,
        session_file: args.session_file,
    }
    .open()
    .unwrap();
    let report = verify(&api, &state, &scenario).unwrap();
    assert!(!report.ok);
    assert!(report.issues.iter().any(|i| i.kind == "uncertain"));
    assert!(report.issues.iter().any(|i| i.kind == "incomplete"));
    server.finish();
}
#[test]
fn edited_fixture_wrong_target_and_lock_fail_before_requests() {
    let scenario = fixture();
    let dir = Directory::new();
    let (mut args, _) = setup(&dir, "http://127.0.0.1:1", &scenario);
    let mut edited = scenario.clone();
    edited.seed += 1;
    assert!(prepare(&args, &edited).is_err());
    args.app_api_base_url = "http://127.0.0.1:2".into();
    assert!(prepare(&args, &scenario).is_err());
    args.app_api_base_url = "http://127.0.0.1:1".into();
    let _lock = apply::lock_state(args.state_file.as_ref().unwrap()).unwrap();
    assert!(prepare(&args, &scenario).is_err());
}
#[test]
fn numeric_comparison_handles_database_decimals_without_integer_rounding() {
    for (expected, actual) in [
        (json!(2026), json!("2026.000")),
        (json!(1.5), json!("1.500")),
        (json!(1000), json!("1e3")),
    ] {
        assert!(equivalent(&expected, &actual, FieldType::Number));
    }
    assert!(!equivalent(
        &json!(9007199254740992u64),
        &json!("9007199254740993"),
        FieldType::Number
    ));
    assert!(!equivalent(&json!(1), &json!("bad"), FieldType::Number));
}

#[test]
fn inaccessible_metadata_and_changed_field_settings_cannot_pass() {
    let scenario = fixture();
    let dir = Directory::new();
    let server = Server::with_identity(vec![(403, json!({"message":"denied"}))]);
    let (args, _) = setup(&dir, &server.url, &scenario);
    let (api, _, _lock, state) = prepare(&args, &scenario).unwrap();
    let report = verify(&api, &state, &scenario).unwrap();
    assert!(!report.ok);
    assert_eq!(report.issues[0].kind, "unverified");
    server.finish();
    let dir = Directory::new();
    let mut replies = responses(&scenario);
    replies[0].1["fields"][2]["matchable"] = json!(true);
    let server = Server::with_identity(replies);
    let (args, _) = setup(&dir, &server.url, &scenario);
    let (api, _, _lock, state) = prepare(&args, &scenario).unwrap();
    let report = verify(&api, &state, &scenario).unwrap();
    assert!(!report.ok);
    assert!(
        report
            .issues
            .iter()
            .any(|i| i.reason.contains("matching settings"))
    );
    server.finish();
}
