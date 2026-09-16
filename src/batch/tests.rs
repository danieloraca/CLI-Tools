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

fn membership_read(endpoint: &str, membership: Option<Value>) -> Vec<(u16, Value)> {
    let mut responses = vec![
        (200, json!({"contact":{"id":101}})),
        (
            200,
            json!({endpoint:membership.clone().into_iter().collect::<Vec<_>>()}),
        ),
    ];
    if membership.is_some() {
        responses.push((200, json!({endpoint:[]})));
    }
    responses
}
#[test]
fn organisation_memberships_use_exact_destinations_and_skip_existing_links() {
    for existing in [true, false] {
        let dir = Directory::new();
        let member = json!({"id":45,"contact_id":101,"organisation_id":3});
        let before = if existing { Some(member.clone()) } else { None };
        let mut responses = vec![(200, json!({"organisation":{"id":3}}))];
        responses.extend(membership_read("enrolments", before.clone()));
        responses.extend(membership_read("enrolments", before));
        if !existing {
            responses.push((200, json!({"enrolment":{"id":45}})));
            responses.extend(membership_read("enrolments", Some(member)));
        }
        let server = Server::with_identity(responses);
        let args = selection(&dir, &server.url, vec![101], true);
        let op = Operation::Organisation { id: 3 };
        let result = execute(&args, op.clone()).unwrap();
        assert_eq!(result["results"]["101"]["membership_id"], "45");
        let requests = server.finish();
        let writes: Vec<_> = requests
            .iter()
            .filter(|r| r.line.starts_with("POST "))
            .collect();
        if existing {
            assert!(writes.is_empty());
        } else {
            assert_eq!(
                writes[0].line,
                "POST /organisations/3/add_contact/101 HTTP/1.1"
            );
        }
        assert!(
            requests
                .iter()
                .any(|r| r.line.contains("contact_id=101&organisation_id=3"))
        );
        assert_eq!(execute(&args, op).unwrap()["completed"], 1);
    }
}
#[test]
fn event_registration_records_the_actual_waitlist_outcome() {
    let dir = Directory::new();
    let mut responses = vec![(200, json!({"event":{"id":8,"type":10}}))];
    responses.extend(membership_read("attendances", None));
    responses.extend(membership_read("attendances", None));
    responses.push((200, json!({"attendance":{"id":66,"status":50}})));
    responses.extend(membership_read(
        "attendances",
        Some(json!({"id":66,"contact_id":101,"event_id":8,"status":50})),
    ));
    let server = Server::with_identity(responses);
    let args = selection(&dir, &server.url, vec![101], true);
    let result = execute(
        &args,
        Operation::Event {
            id: 8,
            status: AttendanceStatus::Registered,
        },
    )
    .unwrap();
    assert_eq!(result["results"]["101"]["status"], 50);
    assert_eq!(result["results"]["101"]["status_title"], "Waitlisted");
    assert_eq!(result["results"]["101"]["matches_requested"], false);
    let requests = server.finish();
    let write = requests
        .iter()
        .find(|r| r.line.starts_with("POST "))
        .unwrap();
    assert_eq!(write.line, "POST /contacts/101/attend HTTP/1.1");
    assert_eq!(write.body, json!({"event_id":8,"status":10}));
    assert_eq!(state(&args).completed["101"]["status"], 50);
}
#[test]
fn event_preview_and_existing_attendance_do_not_write() {
    for execute_flag in [false, true] {
        let dir = Directory::new();
        let membership = json!({"id":66,"contact_id":101,"event_id":8,"status":30});
        let mut responses = vec![(200, json!({"event":{"id":8,"type":10}}))];
        responses.extend(membership_read("attendances", Some(membership.clone())));
        if execute_flag {
            responses.extend(membership_read("attendances", Some(membership)));
        }
        let server = Server::with_identity(responses);
        let args = selection(&dir, &server.url, vec![101], execute_flag);
        let result = execute(
            &args,
            Operation::Event {
                id: 8,
                status: AttendanceStatus::Registered,
            },
        )
        .unwrap();
        assert_eq!(result["preview"][0]["change_required"], false);
        assert_eq!(result["preview"][0]["current"]["status"], 30);
        assert!(server.finish().iter().all(|r| r.line.starts_with("GET ")));
    }
}
#[test]
fn foreign_memberships_and_failed_registration_never_claim_completion() {
    let dir = Directory::new();
    let mut responses = vec![(200, json!({"event":{"id":8,"type":10}}))];
    responses.extend(membership_read(
        "attendances",
        Some(json!({"id":66,"contact_id":999,"event_id":8,"status":10})),
    ));
    let server = Server::with_identity(responses);
    let args = selection(&dir, &server.url, vec![101], true);
    assert!(
        execute(
            &args,
            Operation::Event {
                id: 8,
                status: AttendanceStatus::Registered
            }
        )
        .is_err()
    );
    assert!(server.finish().iter().all(|r| r.line.starts_with("GET ")));
    let dir = Directory::new();
    let mut responses = vec![(200, json!({"event":{"id":8,"type":10}}))];
    responses.extend(membership_read("attendances", None));
    responses.extend(membership_read("attendances", None));
    responses.push((500, json!({"message":"failed"})));
    let server = Server::with_identity(responses);
    let args = selection(&dir, &server.url, vec![101], true);
    let op = Operation::Event {
        id: 8,
        status: AttendanceStatus::Registered,
    };
    assert!(execute(&args, op.clone()).is_err());
    server.finish();
    assert_eq!(state(&args).pending.as_deref(), Some("101"));
    assert!(state(&args).completed.is_empty());
    assert!(execute(&args, op).is_err());
}

#[test]
fn session_containers_are_rejected_before_preview_or_registration() {
    for kind in [json!(20), Value::Null, json!(999)] {
        let dir = Directory::new();
        let server = Server::with_identity(vec![(200, json!({"event":{"id":8,"type":kind}}))]);
        let args = selection(&dir, &server.url, vec![101], true);
        assert!(
            execute(
                &args,
                Operation::Event {
                    id: 8,
                    status: AttendanceStatus::Registered
                }
            )
            .is_err()
        );
        assert!(server.finish().iter().all(|r| r.line.starts_with("GET ")));
        assert!(!args.journal.exists());
    }
}
