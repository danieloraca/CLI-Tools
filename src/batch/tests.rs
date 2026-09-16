use super::*;
use crate::test_support::{Directory, Server};

fn selection(dir: &Directory, url: &str, ids: Vec<u64>, execute: bool) -> Selection {
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
    Selection {
        connection: Connection {
            app_api_base_url: url.into(),
            app_token_file: Some(app_token_file),
            session_file: Some(session_file),
        },
        profile_id: "p-1".into(),
        contact_id: ids,
        scenario_state: None,
        journal: dir.path("operation.batch-state.json"),
        execute,
    }
}
fn contact(id: u64, with_label: bool) -> Value {
    json!({"contact":{"id":id,"labels":if with_label{vec![json!({"id":9}),json!({"id":5})]}else{vec![json!({"id":5})]}}})
}
fn consent_contact(granted: bool) -> Value {
    json!({"contact":{"id":101,"consent_data":[{"consent":7,"granted":granted},{"consent":8,"granted":true}]}})
}
fn state(selection: &Selection) -> State {
    serde_json::from_slice(&fs::read(&selection.journal).unwrap()).unwrap()
}
#[test]
fn preview_is_exact_private_and_has_no_mutations() {
    let dir = Directory::new();
    let server = Server::with_identity(vec![
        (200, json!({"label":{"id":9}})),
        (200, contact(101, false)),
        (200, contact(102, true)),
    ]);
    let args = selection(&dir, &server.url, vec![102, 101, 101], false);
    let result = execute(&args, Operation::LabelAdd { id: 9 }).unwrap();
    assert_eq!(result["contact_ids"], json!(["101", "102"]));
    assert_eq!(result["preview"][0]["change_required"], true);
    assert_eq!(result["preview"][1]["change_required"], false);
    assert!(server.finish().iter().all(|r| r.line.starts_with("GET ")));
    assert!(state(&args).completed.is_empty());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(&args.journal).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
}
#[test]
fn label_additions_use_only_the_targeted_delta_and_reruns_make_no_requests() {
    let dir = Directory::new();
    let before = contact(101, false);
    let after = contact(101, true);
    let server = Server::with_identity(vec![
        (200, json!({"label":{"id":9}})),
        (200, before.clone()),
        (200, before),
        (200, json!({"queue_id":null})),
        (200, after),
    ]);
    let args = selection(&dir, &server.url, vec![101], true);
    let op = Operation::LabelAdd { id: 9 };
    assert_eq!(execute(&args, op.clone()).unwrap()["completed"], 1);
    let requests = server.finish();
    assert_eq!(requests[4].line, "POST /contacts/mass_action HTTP/1.1");
    assert_eq!(
        requests[4].body,
        json!({"conditions":{"contact_ids":["101"]},"actions":[{"type":"assign_contact_label","to":[9]}]})
    );
    assert!(state(&args).pending.is_none());
    assert_eq!(execute(&args, op).unwrap()["completed"], 1);
}
#[test]
fn consent_changes_preserve_other_reasons_through_add_remove_actions() {
    for grant in [true, false] {
        let dir = Directory::new();
        let before = consent_contact(!grant);
        let after = consent_contact(grant);
        let server = Server::with_identity(vec![
            (200, json!({"consent":{"id":7}})),
            (200, before.clone()),
            (200, before),
            (200, json!({"queue_id":null})),
            (200, after),
        ]);
        let args = selection(&dir, &server.url, vec![101], true);
        execute(&args, Operation::Consent { id: 7, grant }).unwrap();
        assert_eq!(
            server.finish()[4].body["actions"],
            json!([{"type":"manage_consent","operator":if grant{"add"}else{"remove"},"to":[7]}])
        );
    }
}
#[test]
fn foreign_destination_or_missing_contacts_stop_before_any_mutation() {
    for responses in [
        vec![(200, json!({"label":{"id":99}}))],
        vec![
            (200, json!({"label":{"id":9}})),
            (404, json!({"message":"missing"})),
        ],
    ] {
        let dir = Directory::new();
        let server = Server::with_identity(responses);
        let args = selection(&dir, &server.url, vec![101], true);
        assert!(execute(&args, Operation::LabelAdd { id: 9 }).is_err());
        assert!(server.finish().iter().all(|r| r.line.starts_with("GET ")));
        assert!(!args.journal.exists());
    }
}
#[test]
fn ambiguous_mutation_retains_prior_success_and_blocks_retry() {
    let dir = Directory::new();
    let server = Server::with_identity(vec![
        (200, json!({"label":{"id":9}})),
        (200, contact(101, false)),
        (200, contact(102, false)),
        (200, contact(101, false)),
        (200, json!({"queue_id":null})),
        (200, contact(101, true)),
        (200, contact(102, false)),
        (500, json!({"message":"committed or not?"})),
    ]);
    let args = selection(&dir, &server.url, vec![101, 102], true);
    let op = Operation::LabelAdd { id: 9 };
    assert!(execute(&args, op.clone()).is_err());
    server.finish();
    assert!(state(&args).completed.contains_key("101"));
    assert_eq!(state(&args).pending.as_deref(), Some("102"));
    assert!(
        execute(&args, op)
            .unwrap_err()
            .to_string()
            .contains("uncertain")
    );
}
#[test]
fn journal_binding_and_lock_prevent_wrong_or_concurrent_operations() {
    let dir = Directory::new();
    let args = selection(&dir, "http://127.0.0.1:1", vec![101], true);
    let lock = crate::scenarios::lock_state(&args.journal).unwrap();
    assert!(
        execute(&args, Operation::LabelAdd { id: 9 })
            .unwrap_err()
            .to_string()
            .contains("another process")
    );
    drop(lock);
    let api = args.connection.open().unwrap();
    let recorded = State {
        version: 1,
        target: api.target(),
        operation: Operation::LabelAdd { id: 9 },
        contact_ids: vec!["101".into()],
        completed: BTreeMap::new(),
        pending: None,
    };
    save(&args, &recorded).unwrap();
    assert!(
        execute(&args, Operation::LabelAdd { id: 10 })
            .unwrap_err()
            .to_string()
            .contains("another target")
    );
}
#[test]
fn scenario_sources_require_matching_target_and_complete_contacts() {
    let dir = Directory::new();
    let mut args = selection(&dir, "http://127.0.0.1:1", vec![], false);
    let source = dir.path("source.apply-state.json");
    args.scenario_state = Some(source.clone());
    let target = args.connection.open().unwrap().target();
    let mut journal = json!({"version":1,"target":target,"fixture_sha256":"abc","run_id":"run-1","completed":{"contact:a":"101","populated:a":"101"},"pending":null});
    fs::write(&source, journal.to_string()).unwrap();
    assert_eq!(
        crate::scenarios::recorded_contacts(&source, &target).unwrap(),
        vec!["101"]
    );
    let source_lock = crate::scenarios::lock_state(&source).unwrap();
    assert!(
        execute(&args, Operation::LabelAdd { id: 9 })
            .unwrap_err()
            .to_string()
            .contains("another process")
    );
    drop(source_lock);
    journal["completed"]["populated:a"] = json!("102");
    fs::write(&source, journal.to_string()).unwrap();
    assert!(crate::scenarios::recorded_contacts(&source, &target).is_err());
    journal["target"]["profile_id"] = json!("foreign");
    fs::write(&source, journal.to_string()).unwrap();
    assert!(crate::scenarios::recorded_contacts(&source, &target).is_err());
}

#[test]
fn queued_or_unconfirmed_actions_keep_pending_instead_of_claiming_success() {
    for queued in [true, false] {
        let dir = Directory::new();
        let mut responses = vec![
            (200, json!({"label":{"id":9}})),
            (200, contact(101, false)),
            (200, contact(101, false)),
            (
                200,
                json!({"queue_id":if queued{json!(123)}else{Value::Null}}),
            ),
        ];
        if !queued {
            responses.push((200, contact(101, false)));
        }
        let server = Server::with_identity(responses);
        let args = selection(&dir, &server.url, vec![101], true);
        assert!(execute(&args, Operation::LabelAdd { id: 9 }).is_err());
        server.finish();
        assert_eq!(state(&args).pending.as_deref(), Some("101"));
        assert!(state(&args).completed.is_empty());
    }
}
