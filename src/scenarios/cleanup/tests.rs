use super::super::apply::CreationEvidence;
use super::*;
use crate::{
    scenarios::{GenerateArgs, fixture::generate},
    test_support::{Directory, Server},
};
fn fixture(count: usize, shared: bool) -> Scenario {
    generate(&GenerateArgs {
        request: None,
        name: "cleanup-test".into(),
        seed: 42,
        contacts: Some(count),
        duplicate_emails: None,
        missing_emails: None,
        custom_field: if shared {
            vec!["note:text".into()]
        } else {
            vec![]
        },
        custom_fields: false,
        restricted_permissions: shared,
        output: None,
    })
    .unwrap()
}
fn contact(id: u64) -> Value {
    json!({"contact":{"id":id,"created_at":1000,"uuid":format!("contact-{id}")}})
}
fn group(scenario: &Scenario) -> Value {
    json!({"group":{"id":9,"created_at":1000,"type":"custom","name":scenario.groups[0].name,"user_count":0,"permissions":[{"id":1,"key":"contacts_view","always":false}]}})
}
fn field() -> Value {
    json!({"field":{"id":91,"created_at":1000,"type":"text","field_type":"contact"}})
}
fn setup(dir: &Directory, url: &str, scenario: &Scenario, execute: bool) -> CleanupArgs {
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
    fs::write(&session_file, serde_json::to_vec(&session).unwrap()).unwrap();
    fs::write(
        &app_token_file,
        serde_json::to_vec(&crate::test_support::app_tokens("app-user-1")).unwrap(),
    )
    .unwrap();
    let args = CleanupArgs {
        run: ApplyArgs {
            fixture: dir.path("fixture.json"),
            profile_id: "p-1".into(),
            app_api_base_url: url.into(),
            session_file: Some(session_file),
            app_token_file: Some(app_token_file),
            state_file: Some(dir.path("run.apply-state.json")),
        },
        execute,
    };
    let target = Target {
        base_url: url.into(),
        account_id: session.account_id,
        profile_id: session.profile_id,
    };
    let mut state =
        apply::load_state(args.run.state_file.as_ref().unwrap(), target, scenario).unwrap();
    for (i, c) in scenario.contacts.iter().enumerate() {
        let id = (101 + i).to_string();
        let key = format!("contact:{}", c.key);
        state.completed.insert(key.clone(), id.clone());
        state.completed.insert(format!("populated:{}", c.key), id);
        state.creation.insert(
            key,
            CreationEvidence::from_resource(&contact((101 + i) as u64)["contact"]).unwrap(),
        );
    }
    for g in &scenario.groups {
        let key = format!("group:{}", g.key);
        state.completed.insert(key.clone(), "9".into());
        state.creation.insert(
            key,
            CreationEvidence::from_resource(&group(scenario)["group"]).unwrap(),
        );
    }
    for f in &scenario.fields {
        if !matches!(
            f.kind,
            super::super::fixture::FieldType::Name | super::super::fixture::FieldType::Email
        ) {
            let key = format!("field:{}", f.key);
            state.completed.insert(key.clone(), "91".into());
            state.creation.insert(
                key,
                CreationEvidence::from_resource(&field()["field"]).unwrap(),
            );
        }
    }
    apply::save_state(args.run.state_file.as_ref().unwrap(), &state).unwrap();
    args
}
fn applied(args: &CleanupArgs) -> State {
    serde_json::from_slice(&fs::read(args.run.state_file.as_ref().unwrap()).unwrap()).unwrap()
}
fn save_applied(args: &CleanupArgs, state: &State) {
    apply::save_state(args.run.state_file.as_ref().unwrap(), state).unwrap();
}
#[test]
fn preview_has_no_mutations_or_journal_changes() {
    let scenario = fixture(1, false);
    let server = Server::with_identity(vec![(200, contact(101))]);
    let dir = Directory::new();
    let args = setup(&dir, &server.url, &scenario, false);
    let path = args.run.state_file.as_ref().unwrap();
    let before = fs::read(path).unwrap();
    let report = execute(&args, &scenario).unwrap();
    assert_eq!(report["resources"][0]["action"], "delete");
    assert_eq!(fs::read(path).unwrap(), before);
    assert!(!cleanup_path(path).exists());
    assert!(server.finish().iter().all(|r| r.line.starts_with("GET ")));
}
#[test]
fn deletes_only_recorded_contacts_and_closes_the_run_without_erasing_creation_evidence() {
    let scenario = fixture(1, false);
    let server = Server::with_identity(vec![
        (200, contact(101)),
        (200, contact(101)),
        (204, Value::Null),
        (404, json!({"message":"missing"})),
    ]);
    let dir = Directory::new();
    let args = setup(&dir, &server.url, &scenario, true);
    let before = applied(&args);
    let report = execute(&args, &scenario).unwrap();
    assert_eq!(report["removed"], 1);
    assert_eq!(report["lifecycle"], "cleaned");
    let after = applied(&args);
    assert_eq!(before.completed, after.completed);
    assert_eq!(before.creation, after.creation);
    let requests = server.finish();
    assert_eq!(
        requests
            .iter()
            .filter(|r| r.line.starts_with("DELETE "))
            .map(|r| r.line.as_str())
            .collect::<Vec<_>>(),
        vec!["DELETE /contacts/101 HTTP/1.1"]
    );
    assert_eq!(execute(&args, &scenario).unwrap()["removed"], 1);
    assert!(
        apply::run(&args.run, scenario.clone())
            .unwrap_err()
            .to_string()
            .contains("cleanup has started")
    );
    assert!(
        apply::recorded_contacts(args.run.state_file.as_ref().unwrap(), &after.target).is_err()
    );
}
#[test]
fn dependency_order_deletes_contacts_then_unused_groups_and_retains_shared_fields() {
    let scenario = fixture(1, true);
    let g = group(&scenario);
    let server = Server::with_identity(vec![
        (200, contact(101)),
        (200, g.clone()),
        (200, json!({"data":[]})),
        (200, field()),
        (200, contact(101)),
        (204, Value::Null),
        (404, json!({"message":"gone"})),
        (200, g),
        (200, json!({"data":[]})),
        (204, Value::Null),
        (404, json!({"message":"gone"})),
        (200, field()),
    ]);
    let dir = Directory::new();
    let args = setup(&dir, &server.url, &scenario, true);
    let result = execute(&args, &scenario).unwrap();
    assert_eq!(result["removed"], 2);
    assert_eq!(result["retained"], 1);
    assert_eq!(result["resources"][2]["action"], "retain");
    assert!(
        result["resources"][2]["reason"]
            .as_str()
            .unwrap()
            .contains("dependency check")
    );
    let requests = server.finish();
    assert_eq!(
        requests
            .iter()
            .filter(|r| r.line.starts_with("DELETE "))
            .map(|r| r.line.as_str())
            .collect::<Vec<_>>(),
        vec!["DELETE /contacts/101 HTTP/1.1", "DELETE /groups/9 HTTP/1.1"]
    );
    assert!(
        requests
            .iter()
            .any(|r| r.line.contains("contact-field-groups?perPage=100"))
    );
}
#[test]
fn missing_resources_are_recorded_without_delete_requests() {
    let scenario = fixture(1, false);
    let server = Server::with_identity(vec![
        (404, json!({"message":"gone"})),
        (404, json!({"message":"gone"})),
    ]);
    let dir = Directory::new();
    let args = setup(&dir, &server.url, &scenario, true);
    assert_eq!(execute(&args, &scenario).unwrap()["removed"], 1);
    assert!(server.finish().iter().all(|r| r.line.starts_with("GET ")));
}
#[test]
fn legacy_journals_and_reused_ids_are_retained() {
    for legacy in [true, false] {
        let scenario = fixture(1, false);
        let mut response = contact(101);
        if !legacy {
            response["contact"]["uuid"] = json!("replacement-contact");
        }
        let server = Server::with_identity(vec![(200, response)]);
        let dir = Directory::new();
        let args = setup(&dir, &server.url, &scenario, false);
        if legacy {
            let mut state = applied(&args);
            state.creation.clear();
            save_applied(&args, &state);
        }
        let result = execute(&args, &scenario).unwrap();
        assert_eq!(result["retained"], 1);
        assert_eq!(result["resources"][0]["action"], "retain");
        assert!(server.finish().iter().all(|r| r.line.starts_with("GET ")));
    }
}
#[test]
fn groups_with_users_or_access_rules_are_retained() {
    for users in [true, false] {
        let scenario = fixture(1, true);
        let mut g = group(&scenario);
        if users {
            g["group"]["user_count"] = json!(1);
        }
        let mut responses = vec![(200, g)];
        if !users {
            responses.extend([
                (200, json!({"data":[{"id":"rule-1","userGroupIds":["9"]}]})),
                (200, json!({"data":[]})),
            ]);
        }
        let server = Server::with_identity(responses);
        let dir = Directory::new();
        let args = setup(&dir, &server.url, &scenario, false);
        let mut state = applied(&args);
        state.completed.retain(|k, _| k.starts_with("group:"));
        state.creation.retain(|k, _| k.starts_with("group:"));
        save_applied(&args, &state);
        let result = execute(&args, &scenario).unwrap();
        assert_eq!(result["retained"], 1);
        assert!(server.finish().iter().all(|r| r.line.starts_with("GET ")));
    }
}
#[test]
fn failed_deletion_preserves_prior_success_and_reconciles_only_confirmed_absence() {
    for absent in [true, false] {
        let scenario = fixture(2, false);
        let mut responses = vec![
            (200, contact(101)),
            (200, contact(102)),
            (200, contact(101)),
            (204, Value::Null),
            (404, json!({"message":"gone"})),
            (200, contact(102)),
            (500, json!({"message":"lost outcome"})),
            (200, crate::test_support::auth_identity()),
        ];
        responses.push(if absent {
            (404, json!({"message":"gone"}))
        } else {
            (200, contact(102))
        });
        let server = Server::with_identity(responses);
        let dir = Directory::new();
        let args = setup(&dir, &server.url, &scenario, true);
        assert!(execute(&args, &scenario).is_err());
        let state = applied(&args);
        let sidecar = cleanup_path(args.run.state_file.as_ref().unwrap());
        let progress = load(&sidecar, &state).unwrap();
        assert_eq!(progress.removed.len(), 1);
        assert_eq!(
            progress.pending,
            Some(format!("contact:{}", scenario.contacts[1].key))
        );
        let result = execute(&args, &scenario);
        if absent {
            assert_eq!(result.unwrap()["removed"], 2);
        } else {
            assert!(
                result
                    .unwrap_err()
                    .to_string()
                    .contains("remains uncertain")
            );
        }
        let requests = server.finish();
        assert_eq!(
            requests
                .iter()
                .filter(|r| r.line == "DELETE /contacts/102 HTTP/1.1")
                .count(),
            1
        );
    }
}
#[test]
fn wrong_fixture_target_pending_apply_and_concurrent_lifecycle_fail_before_requests() {
    let scenario = fixture(1, false);
    let dir = Directory::new();
    let mut args = setup(&dir, "http://127.0.0.1:1", &scenario, true);
    let mut edited = scenario.clone();
    edited.seed += 1;
    assert!(execute(&args, &edited).is_err());
    args.run.app_api_base_url = "http://127.0.0.1:2".into();
    assert!(execute(&args, &scenario).is_err());
    args.run.app_api_base_url = "http://127.0.0.1:1".into();
    let _lock = apply::lock_state(args.run.state_file.as_ref().unwrap()).unwrap();
    assert!(execute(&args, &scenario).is_err());
    drop(_lock);
    let mut state = applied(&args);
    state.pending = Some("unknown".into());
    save_applied(&args, &state);
    assert!(
        execute(&args, &scenario)
            .unwrap_err()
            .to_string()
            .contains("uncertain operation")
    );
    assert!(!cleanup_path(args.run.state_file.as_ref().unwrap()).exists());
}
#[test]
fn changed_creation_checkpoints_cannot_reuse_a_cleanup_journal() {
    let scenario = fixture(1, false);
    let dir = Directory::new();
    let args = setup(&dir, "http://127.0.0.1:1", &scenario, true);
    let mut state = applied(&args);
    let sidecar = cleanup_path(args.run.state_file.as_ref().unwrap());
    save(&sidecar, &load(&sidecar, &state).unwrap()).unwrap();
    state.completed.insert(
        format!("contact:{}", scenario.contacts[0].key),
        "999".into(),
    );
    state.completed.insert(
        format!("populated:{}", scenario.contacts[0].key),
        "999".into(),
    );
    save_applied(&args, &state);
    assert!(
        execute(&args, &scenario)
            .unwrap_err()
            .to_string()
            .contains("changed creation evidence")
    );
}
