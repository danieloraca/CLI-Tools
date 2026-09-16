use super::*;
use crate::scenarios::{GenerateArgs, fixture::generate};
use crate::test_support::{Directory, Server};

fn scenario(contacts: usize) -> Scenario {
    generate(&GenerateArgs {
        request: None,
        name: "test".into(),
        seed: 42,
        contacts: Some(contacts),
        duplicate_emails: Some(contacts - 1),
        missing_emails: None,
        custom_field: vec![],
        custom_fields: false,
        restricted_permissions: false,
        output: None,
    })
    .unwrap()
}

fn fields() -> Value {
    json!({"fields": [
        {"id": 17, "type": "name", "field_type": "contact", "required": true},
        {"id": 28, "type": "email", "field_type": "contact", "required": false},
    ]})
}

fn args(dir: &Directory, url: &str) -> ApplyArgs {
    let selected = session::AppSession {
        account_id: "281".into(),
        user_id: "2260".into(),
        profile_id: "p-1".into(),
        account_name: "Development".into(),
        app_description: "Forms".into(),
        redirect_url: "https://example.test".into(),
    };
    let tokens = crate::test_support::app_tokens("p-1");
    let session_file = dir.path("session.json");
    let app_token_file = dir.path("tokens.json");
    fs::write(&session_file, serde_json::to_vec(&selected).unwrap()).unwrap();
    fs::write(&app_token_file, serde_json::to_vec(&tokens).unwrap()).unwrap();
    ApplyArgs {
        fixture: dir.path("fixture.json"),
        profile_id: "p-1".into(),
        app_api_base_url: url.into(),
        app_token_file: Some(app_token_file),
        session_file: Some(session_file),
        state_file: Some(dir.path("state.json")),
    }
}

#[test]
fn applies_real_payloads_with_duplicate_preservation_and_noop_reruns() {
    let dir = Directory::new();
    let server = Server::new(vec![
        (200, fields()),
        (200, json!({"fields": []})),
        (201, json!({"field": {"id": 91}})),
        (201, json!({"group": {"id": 12}})),
        (201, json!({"contact": {"id": 101}})),
        (200, json!({"contact": {"id": 101}})),
        (201, json!({"contact": {"id": 102}})),
        (200, json!({"contact": {"id": 102}})),
    ]);
    let args = args(&dir, &server.url);
    let mut scenario = scenario(2);
    scenario.fields.push(crate::scenarios::fixture::Field {
        key: "cohort".into(),
        label: "Cohort".into(),
        kind: FieldType::Number,
    });
    for contact in &mut scenario.contacts {
        contact.fields.insert("cohort".into(), json!(2026));
    }
    scenario.groups.push(crate::scenarios::fixture::Group {
        key: "read_only".into(),
        name: "Test Read only".into(),
        permissions: vec!["contacts_view".into()],
    });
    // Field ordering in edited JSON does not change field mapping.
    scenario.fields.swap(0, 2);
    scenario.validate().unwrap();
    run(&args, scenario.clone()).unwrap();
    let requests = server.finish();
    assert_eq!(requests.len(), 8);
    assert!(requests[0].line.contains("field_type=contact"));
    assert!(requests[1].line.contains("page=2"));
    assert_eq!(requests[2].line, "POST /fields HTTP/1.1");
    assert_eq!(requests[2].body["matchable"], false);
    assert_eq!(requests[3].body["permissions"], json!(["contacts_view"]));
    assert_eq!(requests[4].line, "POST /contacts HTTP/1.1");
    assert_eq!(requests[5].line, "POST /contacts/101 HTTP/1.1");
    assert_ne!(
        requests[4].body["fields"]["field28"],
        requests[6].body["fields"]["field28"]
    );
    assert_eq!(
        requests[5].body["fields"]["field28"],
        requests[7].body["fields"]["field28"]
    );
    assert_eq!(requests[5].body["fields"]["field91"], 2026);
    assert_eq!(
        requests[5].body["fields"]["field17"],
        scenario.contacts[0].fields["name"]
    );
    let headers = requests[4].headers.to_lowercase();
    assert!(headers.contains("gecko-account: 281"));
    assert!(headers.contains("gecko-user: 2260"));
    assert!(
        headers.contains(
            &format!(
                "authorization: bearer {}",
                crate::test_support::app_tokens("p-1").access_token
            )
            .to_lowercase()
        )
    );
    let state_bytes = fs::read(args.state_file.as_ref().unwrap()).unwrap();
    assert!(!String::from_utf8_lossy(&state_bytes).contains("test-signature"));
    // The server is closed: this succeeds only if it performs no more network requests.
    run(&args, scenario).unwrap();
    assert_eq!(
        fs::read(args.state_file.as_ref().unwrap()).unwrap(),
        state_bytes
    );
}

#[test]
fn ambiguous_write_keeps_prior_success_and_blocks_retry() {
    let dir = Directory::new();
    let server = Server::new(vec![
        (200, fields()),
        (200, json!({"fields": []})),
        (201, json!({"contact": {"id": 101}})),
        (500, json!({"message": "write failed"})),
    ]);
    let args = args(&dir, &server.url);
    let scenario = scenario(1);
    let error = run(&args, scenario.clone()).unwrap_err();
    assert!(format!("{error:#}").contains("500"));
    assert_eq!(server.finish().len(), 4);
    let state: State =
        serde_json::from_slice(&fs::read(args.state_file.as_ref().unwrap()).unwrap()).unwrap();
    assert_eq!(state.completed["contact:contact_00001"], "101");
    assert_eq!(state.pending.as_deref(), Some("populated:contact_00001"));
    assert!(
        run(&args, scenario.clone())
            .unwrap_err()
            .to_string()
            .contains("uncertain outcome")
    );
    let mut changed = scenario;
    changed.seed = 43;
    assert!(
        run(&args, changed)
            .unwrap_err()
            .to_string()
            .contains("different fixture or target")
    );
}

#[test]
fn missing_email_is_never_created_with_a_temporary_email() {
    let dir = Directory::new();
    let server = Server::new(vec![
        (200, fields()),
        (200, json!({"fields": []})),
        (201, json!({"contact": {"id": 101}})),
        (200, json!({"contact": {"id": 101}})),
    ]);
    let args = args(&dir, &server.url);
    let mut scenario = scenario(1);
    scenario.contacts[0]
        .fields
        .insert("email".into(), Value::Null);
    run(&args, scenario).unwrap();
    let requests = server.finish();
    assert!(requests[2].body["fields"].get("field28").is_none());
    assert!(requests[3].body["fields"]["field28"].is_null());
}

#[test]
fn required_field_preflight_fails_before_any_writes() {
    let dir = Directory::new();
    let mut field_data = fields();
    field_data["fields"][1]["required"] = json!(1);
    let server = Server::new(vec![(200, field_data), (200, json!({"fields": []}))]);
    let args = args(&dir, &server.url);
    let mut scenario = scenario(1);
    scenario.contacts[0]
        .fields
        .insert("email".into(), Value::Null);
    assert!(format!("{:#}", run(&args, scenario).unwrap_err()).contains("profile requires email"));
    assert_eq!(server.finish().len(), 2);
    assert!(!args.state_file.unwrap().exists());
}

#[test]
fn success_without_an_id_remains_pending() {
    let dir = Directory::new();
    let server = Server::new(vec![
        (200, fields()),
        (200, json!({"fields": []})),
        (201, json!({"contact": {}})),
    ]);
    let args = args(&dir, &server.url);
    assert!(
        format!("{:#}", run(&args, scenario(1)).unwrap_err())
            .contains("without a usable resource ID")
    );
    assert_eq!(server.finish().len(), 3);
    let state: State =
        serde_json::from_slice(&fs::read(args.state_file.unwrap()).unwrap()).unwrap();
    assert_eq!(state.pending.as_deref(), Some("contact:contact_00001"));
}

#[test]
fn profile_mismatch_and_concurrent_apply_fail_before_requests() {
    let dir = Directory::new();
    let mut args = args(&dir, "http://127.0.0.1:1");
    args.profile_id = "p-2".into();
    assert!(
        run(&args, scenario(1))
            .unwrap_err()
            .to_string()
            .contains("does not match")
    );
    args.profile_id = "p-1".into();
    let _lock = lock_state(args.state_file.as_ref().unwrap()).unwrap();
    assert!(
        run(&args, scenario(1))
            .unwrap_err()
            .to_string()
            .contains("another process")
    );
}

#[test]
fn resuming_after_create_updates_the_existing_contact() {
    let dir = Directory::new();
    let server = Server::new(vec![
        (200, fields()),
        (200, json!({"fields": []})),
        (200, json!({"contact": {"id": 101}})),
    ]);
    let args = args(&dir, &server.url);
    let scenario = scenario(1);
    let mut state = load_state(
        args.state_file.as_ref().unwrap(),
        Target {
            base_url: server.url.clone(),
            account_id: "281".into(),
            profile_id: "p-1".into(),
        },
        &scenario,
    )
    .unwrap();
    state
        .completed
        .insert("contact:contact_00001".into(), "101".into());
    save_state(args.state_file.as_ref().unwrap(), &state).unwrap();
    run(&args, scenario).unwrap();
    let requests = server.finish();
    assert_eq!(requests[2].line, "POST /contacts/101 HTTP/1.1");
}

#[test]
fn an_unexpected_update_id_is_not_checkpointed_as_complete() {
    let dir = Directory::new();
    let server = Server::new(vec![
        (200, fields()),
        (200, json!({"fields": []})),
        (201, json!({"contact": {"id": 101}})),
        (200, json!({"contact": {"id": 999}})),
    ]);
    let args = args(&dir, &server.url);
    assert!(format!("{:#}", run(&args, scenario(1)).unwrap_err()).contains("unexpected ID"));
    assert_eq!(server.finish().len(), 4);
    let state: State =
        serde_json::from_slice(&fs::read(args.state_file.unwrap()).unwrap()).unwrap();
    assert_eq!(state.pending.as_deref(), Some("populated:contact_00001"));
    assert!(!state.completed.contains_key("populated:contact_00001"));
}

#[test]
fn repeated_field_pages_fail_before_creating_resources() {
    let dir = Directory::new();
    let server = Server::new(vec![(200, fields()), (200, fields())]);
    let args = args(&dir, &server.url);
    assert!(format!("{:#}", run(&args, scenario(1)).unwrap_err()).contains("pagination repeated"));
    assert_eq!(server.finish().len(), 2);
    assert!(!args.state_file.unwrap().exists());
}

#[test]
fn unsupported_zero_values_fail_before_any_api_call() {
    let directory = Directory::new();
    let args = args(&directory, "http://127.0.0.1:1");
    let mut fixture = scenario(1);
    fixture.fields.push(crate::scenarios::fixture::Field {
        key: "score".into(),
        label: "Score".into(),
        kind: FieldType::Number,
    });
    fixture.contacts[0].fields.insert("score".into(), json!(0));
    assert!(
        run(&args, fixture)
            .unwrap_err()
            .to_string()
            .contains("silently ignore")
    );
    assert!(!args.state_file.unwrap().exists());
}

#[test]
fn account_identity_must_be_confirmed_before_any_write() {
    for account in [None, Some("999")] {
        let dir = Directory::new();
        let server = Server::with_account(vec![(200, fields())], account);
        let args = args(&dir, &server.url);
        let error = format!("{:#}", run(&args, scenario(1)).unwrap_err());
        assert!(error.contains("account"), "{error}");
        let requests = server.finish();
        assert_eq!(requests.len(), 1);
        assert!(requests[0].line.starts_with("GET /fields?"));
        assert!(!args.state_file.unwrap().exists());
    }
}

#[test]
fn mismatched_app_tokens_are_rejected_before_requesting_fields() {
    let dir = Directory::new();
    let args = args(&dir, "http://127.0.0.1:1");
    auth::persist_tokens(
        &crate::test_support::app_tokens("p-2"),
        args.app_token_file.as_deref(),
    )
    .unwrap();
    let error = run(&args, scenario(1)).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("app token profile p-2 does not match saved profile p-1")
    );
    assert!(!args.state_file.unwrap().exists());
}
