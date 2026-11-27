// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of the REPEATED request.
//!
//! The rule under test: a MediaConvert control-plane request whose effect is already
//! present in the store is answered with that operation's own success — a `Create*` whose
//! `Name` is already stored answers its 201 with the resource that is ALREADY there,
//! unchanged, and a delete or a cancel whose target is already in the state it asks for
//! answers its empty 202 — so a provisioning or teardown path that runs twice is never
//! handed a 4xx. Only a target the request can never reach stays refused: a name a
//! BUILT-IN resource holds (which no `Create*` minted, and which no caller may delete),
//! a queue that still owes work, and a job that left `SUBMITTED` for a status other than
//! `CANCELED`. The READ paths are untouched: a `Get*` of a resource that is not stored is
//! still the modelled 404.
//!
//! Every case drives the real restJson1 wire path through the router (the harness is the
//! one `job_lifecycle` builds), so what is asserted is the bytes a client would see. Two
//! assertions read the store directly, where the listing operation that would show it
//! over the wire (`ListQueues`, `ListPresets`) is still a 501 stub; they say so.

use serde_json::{json, Value};
use service_com_amazonaws_mediaconvert_mediaconvert::ops::types::JobTemplateSettings;
use service_com_amazonaws_mediaconvert_mediaconvert::state::{
    default_acceleration_settings, JobTemplateRecord, DEFAULT_PRIORITY, DEFAULT_QUEUE_NAME,
    DEFAULT_STATUS_UPDATE_INTERVAL, MAX_JOB_TEMPLATES, TYPE_CUSTOM, TYPE_SYSTEM,
};
use service_com_amazonaws_mediaconvert_mediaconvert::ComAmazonawsMediaconvertMediaconvertState;

use super::job_lifecycle::{
    settings as job_settings, Emulator, Reply, PAST_THE_SETTLE, ROLE, SHORT_SETTLE,
};
// One `JobTemplateSettings` fixture serves every job-template suite, so a model change to
// the shape is chased through one file.
use super::job_template_crud::settings as template_settings_with;

/// A structurally real `JobTemplateSettings` with `groups` output groups and one input
/// template.
fn template_settings(groups: usize) -> Value {
    template_settings_with(1, groups)
}

/// A `PresetSettings` carrying the named container, which is the one member every preset
/// must describe.
fn preset_settings(container: &str) -> Value {
    json!({ "containerSettings": { "container": container } })
}

/// One row of the create table: the collection the resource lives in, the output member
/// its operation answers with, the first create and the repeats that follow it.
struct CreateCase {
    label: &'static str,
    /// The path segment `smithy_query(query="operation",operation="CreateQueue,
    /// CreateJobTemplate,CreatePreset")` reports for the collection: `POST
    /// /2017-08-29/<collection>`, and `GET /2017-08-29/<collection>/<name>` to read back.
    collection: &'static str,
    /// The single output member of that create (`queue`, `jobTemplate`, `preset`).
    member: &'static str,
    name: &'static str,
    first: Value,
    /// Repeats of the create under the SAME name. The rows differ in how the repeat's
    /// body relates to the first one, so what comes back cannot depend on the bodies
    /// matching.
    repeats: Vec<(&'static str, Value)>,
}

/// The table: one row per named resource a `Create*` mints, each repeated with an
/// identical body, with a body that sets a member the first did not, and with a body that
/// contradicts the first.
fn create_cases() -> Vec<CreateCase> {
    vec![
        CreateCase {
            label: "queue with only the required Name",
            collection: "queues",
            member: "/queue",
            name: "idempotent-minimal",
            first: json!({ "name": "idempotent-minimal" }),
            repeats: vec![
                ("identical body", json!({ "name": "idempotent-minimal" })),
                (
                    "a description the first create did not carry",
                    json!({ "name": "idempotent-minimal", "description": "second attempt" }),
                ),
                (
                    "a contradicting status",
                    json!({ "name": "idempotent-minimal", "status": "PAUSED" }),
                ),
            ],
        },
        CreateCase {
            label: "queue with every optional member set",
            collection: "queues",
            member: "/queue",
            name: "idempotent-full",
            first: json!({
                "name": "idempotent-full",
                "description": "probe queue for the lifecycle test",
                "status": "PAUSED",
                "pricingPlan": "ON_DEMAND",
                "concurrentJobs": 20,
                "maximumConcurrentFeeds": 2,
                "tags": { "owner": "probe" }
            }),
            repeats: vec![
                (
                    "identical body",
                    json!({
                        "name": "idempotent-full",
                        "description": "probe queue for the lifecycle test",
                        "status": "PAUSED",
                        "pricingPlan": "ON_DEMAND",
                        "concurrentJobs": 20,
                        "maximumConcurrentFeeds": 2,
                        "tags": { "owner": "probe" }
                    }),
                ),
                ("only the Name", json!({ "name": "idempotent-full" })),
            ],
        },
        CreateCase {
            label: "job template",
            collection: "jobTemplates",
            member: "/jobTemplate",
            name: "idempotent-template",
            first: json!({
                "name": "idempotent-template",
                "description": "probe job template",
                "settings": template_settings(1)
            }),
            repeats: vec![
                (
                    "identical body",
                    json!({
                        "name": "idempotent-template",
                        "description": "probe job template",
                        "settings": template_settings(1)
                    }),
                ),
                (
                    "different settings and description",
                    json!({
                        "name": "idempotent-template",
                        "description": "a second deploy",
                        "statusUpdateInterval": "SECONDS_120",
                        "settings": template_settings(3)
                    }),
                ),
            ],
        },
        CreateCase {
            label: "preset",
            collection: "presets",
            member: "/preset",
            name: "idempotent-preset",
            first: json!({
                "name": "idempotent-preset",
                "description": "probe preset",
                "category": "probe-category",
                "settings": preset_settings("MP4")
            }),
            repeats: vec![
                (
                    "identical body",
                    json!({
                        "name": "idempotent-preset",
                        "description": "probe preset",
                        "category": "probe-category",
                        "settings": preset_settings("MP4")
                    }),
                ),
                (
                    "a different container",
                    json!({
                        "name": "idempotent-preset",
                        "settings": preset_settings("WEBM")
                    }),
                ),
            ],
        },
    ]
}

/// `POST /2017-08-29/<collection>`.
async fn create(emulator: &Emulator, collection: &str, body: Value) -> Reply {
    emulator
        .send("POST", &format!("/2017-08-29/{collection}"), Some(body))
        .await
}

/// `GET /2017-08-29/<collection>/<name>`.
async fn read(emulator: &Emulator, collection: &str, name: &str) -> Reply {
    emulator
        .send("GET", &format!("/2017-08-29/{collection}/{name}"), None)
        .await
}

/// `DELETE /2017-08-29/<collection>/<name>`.
async fn delete(emulator: &Emulator, collection: &str, name: &str) -> Reply {
    emulator
        .send("DELETE", &format!("/2017-08-29/{collection}/{name}"), None)
        .await
}

/// How many queues, job templates and presets the store holds.
///
/// Read from the store because `ListQueues` and `ListPresets` are still 501 stubs, so
/// there is no wire path that reports the queue or preset count yet.
fn stored_counts(emulator: &Emulator) -> (usize, usize, usize) {
    let state = emulator
        .engine
        .state::<ComAmazonawsMediaconvertMediaconvertState>();
    let state = state.lock();
    (
        state.queues.len(),
        state.job_templates.len(),
        state.presets.len(),
    )
}

/// The name of a BUILT-IN (`SYSTEM`) job template, read from the STORE so no case here
/// hard-codes the built-in catalogue.
fn a_system_template_name(emulator: &Emulator) -> String {
    emulator
        .engine
        .state::<ComAmazonawsMediaconvertMediaconvertState>()
        .lock()
        .job_templates
        .values()
        .find(|record| record.template_type == TYPE_SYSTEM)
        .map(|record| record.name.clone())
        .unwrap_or_default()
}

/// The name of a BUILT-IN (`SYSTEM`) preset, read from the STORE so no case here
/// hard-codes the built-in catalogue.
fn a_system_preset_name(emulator: &Emulator) -> String {
    emulator
        .engine
        .state::<ComAmazonawsMediaconvertMediaconvertState>()
        .lock()
        .presets
        .values()
        .find(|record| record.preset_type == TYPE_SYSTEM)
        .map(|record| record.name.clone())
        .unwrap_or_default()
}

/// A repeated `Create*` under a name that is already stored answers the operation's own
/// success status with the resource that is ALREADY there, byte for byte, whatever the
/// repeat's body says — and changes nothing, so the reader keeps answering what the FIRST
/// create persisted and the store does not grow.
#[tokio::test]
async fn a_repeated_create_returns_the_stored_resource_unchanged() {
    for case in create_cases() {
        let emulator = Emulator::new();
        let before = stored_counts(&emulator);

        let first = create(&emulator, case.collection, case.first.clone()).await;
        assert_eq!(
            first.status, 201,
            "case {:?}: the first create must succeed, got {} with body {}",
            case.label, first.status, first.body
        );
        let stored = first.at(case.member).clone();
        assert!(
            stored.is_object(),
            "case {:?}: the create must answer with its {} member, got {}",
            case.label,
            case.member,
            first.body
        );

        for (repeat_label, body) in &case.repeats {
            let again = create(&emulator, case.collection, body.clone()).await;
            assert_eq!(
                again.status, 201,
                "case {:?} / {repeat_label:?}: a repeated create must succeed, got {} \
                 with body {}",
                case.label, again.status, again.body
            );
            assert_eq!(
                again.error_type, None,
                "case {:?} / {repeat_label:?}: a repeated create is not an error, got {:?}",
                case.label, again.error_type
            );
            assert_eq!(
                again.at(case.member),
                &stored,
                "case {:?} / {repeat_label:?}: expected the stored resource {stored}, got {}",
                case.label,
                again.body
            );

            // Round trip: the reader answers the record the FIRST create persisted, so
            // the repeat edited nothing.
            let fetched = read(&emulator, case.collection, case.name).await;
            assert_eq!(
                fetched.status, 200,
                "case {:?} / {repeat_label:?}: expected the read to succeed, got {} \
                 with body {}",
                case.label, fetched.status, fetched.body
            );
            assert_eq!(
                fetched.at(case.member),
                &stored,
                "case {:?} / {repeat_label:?}: the read must answer {stored}, got {}",
                case.label,
                fetched.body
            );
        }

        // Exactly ONE record was added, however many creates were sent.
        let after = stored_counts(&emulator);
        let added = (
            after.0 - before.0,
            after.1 - before.1,
            after.2 - before.2,
        );
        let expected = match case.collection {
            "queues" => (1, 0, 0),
            "jobTemplates" => (0, 1, 0),
            _ => (0, 0, 1),
        };
        assert_eq!(
            added, expected,
            "case {:?}: expected {expected:?} record(s) added by {} create(s), got {added:?}",
            case.label,
            case.repeats.len() + 1
        );
    }
}

/// A name a BUILT-IN resource holds is the one name a create is refused for: no `Create*`
/// ever minted it, so there is no earlier create for the repeat to be idempotent with,
/// and the built-in resource is left exactly as it was.
#[tokio::test]
async fn creating_over_a_built_in_resource_is_a_conflict() {
    let emulator = Emulator::new();

    // The built-in resources the store is seeded with, read from the state module and the
    // store itself so this cannot drift from what is actually there.
    let queue = DEFAULT_QUEUE_NAME;
    let template = a_system_template_name(&emulator);

    let cases: Vec<(&str, &str, &str, Value)> = vec![
        (
            "the built-in queue",
            "queues",
            "/queue",
            json!({ "name": queue }),
        ),
        (
            "a built-in job template",
            "jobTemplates",
            "/jobTemplate",
            json!({ "name": template, "settings": template_settings(1) }),
        ),
    ];

    for (label, collection, member, body) in cases {
        let name = body["name"].as_str().unwrap_or_default().to_string();
        let before = read(&emulator, collection, &name).await;
        assert_eq!(
            before.status, 200,
            "case {label:?}: the built-in resource must be readable first, got {} with body {}",
            before.status, before.body
        );

        for attempt in 1..=2 {
            let refused = create(&emulator, collection, body.clone()).await;
            assert_eq!(
                refused.status, 409,
                "case {label:?} attempt {attempt}: expected HTTP 409, got {} with body {}",
                refused.status, refused.body
            );
            assert_eq!(
                refused.error_type.as_deref(),
                Some("ConflictException"),
                "case {label:?} attempt {attempt}: expected ConflictException, got {:?}",
                refused.error_type
            );
            // The wording names the REQUEST MEMBER the name came from, not the value.
            assert_eq!(
                refused.string("/message"),
                service_com_amazonaws_mediaconvert_mediaconvert::state::system_resource_name_message(
                    "name"
                ),
                "case {label:?} attempt {attempt}: the refusal must name the request member, \
                 got {}",
                refused.body
            );
        }

        let after = read(&emulator, collection, &name).await;
        assert_eq!(
            after.at(member),
            before.at(member),
            "case {label:?}: the refused creates must leave the built-in resource alone, \
             got {} after {}",
            after.body,
            before.body
        );
    }
}

/// The idempotent repeat stores nothing, so it is answered even when the store is FULL:
/// at capacity a create under a stored name still returns that resource, while a create
/// under a new name is the modelled quota refusal.
#[tokio::test]
async fn a_repeated_create_succeeds_even_at_the_store_capacity() {
    // The capacity the state module publishes, so this test cannot drift from it.
    let capacity = MAX_JOB_TEMPLATES;
    let emulator = Emulator::new();

    let created = create(
        &emulator,
        "jobTemplates",
        json!({ "name": "at-capacity", "settings": template_settings(1) }),
    )
    .await;
    assert_eq!(
        created.status, 201,
        "the fixture template must be created, got {} with body {}",
        created.status, created.body
    );

    // Seeded directly: a thousand creates over HTTP would say the same thing far more
    // slowly. Every seeded record is a CUSTOM template, as a caller's own would be.
    {
        let state = emulator
            .engine
            .state::<ComAmazonawsMediaconvertMediaconvertState>();
        let mut state = state.lock();
        let seeded_at = ::std::time::SystemTime::now();
        let mut index = 0;
        while state.job_templates.len() < capacity {
            let name = format!("bulk-{index:05}");
            state.job_templates.insert(
                name.clone(),
                JobTemplateRecord {
                    name,
                    category: None,
                    description: None,
                    queue: None,
                    priority: DEFAULT_PRIORITY,
                    settings: JobTemplateSettings::default(),
                    status_update_interval: DEFAULT_STATUS_UPDATE_INTERVAL.to_string(),
                    acceleration_settings: default_acceleration_settings(),
                    elemental_inference_settings: None,
                    hop_destinations: Vec::new(),
                    template_type: TYPE_CUSTOM.to_string(),
                    created_at: seeded_at,
                    last_updated: seeded_at,
                    tags: ::std::collections::BTreeMap::new(),
                },
            );
            index += 1;
        }
    }

    for attempt in 1..=2 {
        let again = create(
            &emulator,
            "jobTemplates",
            json!({ "name": "at-capacity", "settings": template_settings(2) }),
        )
        .await;
        assert_eq!(
            again.status, 201,
            "case attempt {attempt}: a repeat at capacity adds nothing, so it must \
             succeed, got {} with body {}",
            again.status, again.body
        );
        assert_eq!(
            again.template(),
            created.template(),
            "case attempt {attempt}: expected the stored template {}, got {}",
            created.template(),
            again.template()
        );
    }

    let refused = create(
        &emulator,
        "jobTemplates",
        json!({ "name": "one-too-many", "settings": template_settings(1) }),
    )
    .await;
    assert_eq!(
        refused.status, 402,
        "case a new name at capacity: expected HTTP 402, got {} with body {}",
        refused.status, refused.body
    );
}

/// One row of the delete table: the collection, and the name to delete twice.
struct DeleteCase {
    label: &'static str,
    collection: &'static str,
    /// The name to create first, when the row deletes a resource that exists.
    create: Option<Value>,
    name: &'static str,
}

/// A delete is IDEMPOTENT on the name it addresses: deleting a stored resource, deleting
/// it again, and deleting a name that was never stored all answer the operation's empty
/// 202 — while the READ of that name is the modelled 404 either way.
#[tokio::test]
async fn deleting_a_resource_that_is_already_gone_is_a_no_op_success() {
    let cases: Vec<DeleteCase> = vec![
        DeleteCase {
            label: "a queue that was created",
            collection: "queues",
            create: Some(json!({ "name": "teardown-queue" })),
            name: "teardown-queue",
        },
        DeleteCase {
            label: "a queue name nothing ever created",
            collection: "queues",
            create: None,
            name: "never-a-queue",
        },
        DeleteCase {
            label: "a queue name differing from a stored one only in case",
            collection: "queues",
            create: Some(json!({ "name": "teardown-queue" })),
            name: "TEARDOWN-QUEUE",
        },
        DeleteCase {
            label: "a job template that was created",
            collection: "jobTemplates",
            create: Some(json!({
                "name": "teardown-template",
                "settings": template_settings(1)
            })),
            name: "teardown-template",
        },
        DeleteCase {
            label: "a job template name nothing ever created",
            collection: "jobTemplates",
            create: None,
            name: "never-a-template",
        },
        DeleteCase {
            label: "a preset that was created",
            collection: "presets",
            create: Some(json!({
                "name": "teardown-preset",
                "settings": preset_settings("MP4")
            })),
            name: "teardown-preset",
        },
        DeleteCase {
            label: "a preset name nothing ever created",
            collection: "presets",
            create: None,
            name: "never-a-preset",
        },
        DeleteCase {
            label: "a preset name differing from a stored one only in case",
            collection: "presets",
            create: Some(json!({
                "name": "teardown-preset",
                "settings": preset_settings("MP4")
            })),
            name: "TEARDOWN-PRESET",
        },
    ];

    for case in cases {
        let emulator = Emulator::new();
        if let Some(body) = case.create.clone() {
            let created = create(&emulator, case.collection, body).await;
            assert_eq!(
                created.status, 201,
                "case {:?}: the fixture must be created, got {} with body {}",
                case.label, created.status, created.body
            );
        }

        // Three deletes: the name is gone after the first one at the latest, and every
        // one of them answers the same empty 202.
        for attempt in 1..=3 {
            let deleted = delete(&emulator, case.collection, case.name).await;
            assert_eq!(
                deleted.status, 202,
                "case {:?} attempt {attempt}: expected HTTP 202, got {} with body {}",
                case.label, deleted.status, deleted.body
            );
            assert_eq!(
                deleted.body,
                json!({}),
                "case {:?} attempt {attempt}: the delete models no output members, got {}",
                case.label, deleted.body
            );

            // The delete is idempotent; the READ is not.
            let gone = read(&emulator, case.collection, case.name).await;
            assert_eq!(
                gone.status, 404,
                "case {:?} attempt {attempt}: expected the read to be 404, got {} with body {}",
                case.label, gone.status, gone.body
            );
            assert_eq!(
                gone.error_type.as_deref(),
                Some("NotFoundException"),
                "case {:?} attempt {attempt}: expected NotFoundException, got {:?}",
                case.label,
                gone.error_type
            );
        }
    }
}

/// A BUILT-IN resource is the one target a delete is refused for: it is not the caller's
/// to delete, however many times the delete is sent, and it is still there afterwards.
#[tokio::test]
async fn deleting_a_built_in_resource_stays_refused() {
    let emulator = Emulator::new();
    let cases: Vec<(&str, &str, String)> = vec![
        ("the built-in queue", "queues", DEFAULT_QUEUE_NAME.to_string()),
        (
            "a built-in job template",
            "jobTemplates",
            a_system_template_name(&emulator),
        ),
        (
            "a built-in preset",
            "presets",
            a_system_preset_name(&emulator),
        ),
    ];

    for (label, collection, name) in cases {
        let name = name.as_str();
        for attempt in 1..=2 {
            let refused = delete(&emulator, collection, name).await;
            assert_eq!(
                refused.status, 403,
                "case {label:?} attempt {attempt}: expected HTTP 403, got {} with body {}",
                refused.status, refused.body
            );
            assert_eq!(
                refused.error_type.as_deref(),
                Some("ForbiddenException"),
                "case {label:?} attempt {attempt}: expected ForbiddenException, got {:?}",
                refused.error_type
            );
        }
        let still_there = read(&emulator, collection, name).await;
        assert_eq!(
            still_there.status, 200,
            "case {label:?}: the built-in resource must survive, got {} with body {}",
            still_there.status, still_there.body
        );
    }
}

/// `CancelJob` on a job that is ALREADY `CANCELED` answers the same empty 202 and leaves
/// the job exactly as it was — the repeat is a no-op, not a conflict — however many times
/// it is sent and whichever queue the job was submitted to.
#[tokio::test]
async fn cancelling_an_already_canceled_job_is_a_no_op_success() {
    let emulator = Emulator::new();
    let queue = emulator.create_queue(json!({ "name": "cancel-queue" })).await;
    assert_eq!(
        queue.status, 201,
        "the fixture queue must be created, got {} with body {}",
        queue.status, queue.body
    );

    // The rows differ in the queue the job was submitted to and in how many repeats
    // follow the first cancel, so the success is not keyed to one shape of the sequence.
    for (label, queue_name, repeats) in [
        ("the built-in queue, one repeat", "Default", 1),
        ("a custom queue, two repeats", "cancel-queue", 2),
        ("a custom queue, five repeats", "cancel-queue", 5),
    ] {
        let created = emulator
            .create_job(json!({
                "role": ROLE,
                "queue": queue_name,
                "settings": job_settings("s3://example-bucket/in/a.mp4", "_a")
            }))
            .await;
        assert_eq!(
            created.status, 201,
            "case {label:?}: the job must be created, got {} with body {}",
            created.status, created.body
        );
        let id = created.string("/job/id");

        let cancelled = emulator.cancel_job(&id).await;
        assert_eq!(
            cancelled.status, 202,
            "case {label:?}: the first cancel must succeed, got {} with body {}",
            cancelled.status, cancelled.body
        );
        let after_first = emulator.get_job(&id).await;
        assert_eq!(
            after_first.string("/job/status"),
            "CANCELED",
            "case {label:?}: the job must be CANCELED, got {}",
            after_first.body
        );

        for attempt in 1..=repeats {
            let again = emulator.cancel_job(&id).await;
            assert_eq!(
                again.status, 202,
                "case {label:?} repeat {attempt}: expected HTTP 202, got {} with body {}",
                again.status, again.body
            );
            assert_eq!(
                again.body,
                json!({}),
                "case {label:?} repeat {attempt}: CancelJob models no output members, got {}",
                again.body
            );
            let now = emulator.get_job(&id).await;
            assert_eq!(
                now.at("/job"),
                after_first.at("/job"),
                "case {label:?} repeat {attempt}: the repeat must change nothing, got {} \
                 after {}",
                now.body,
                after_first.body
            );
        }
    }
}

/// The repeat is a success only where the job is already in the state the cancel asks
/// for: a job that left `SUBMITTED` for a status other than `CANCELED` still conflicts,
/// and an id nothing stored is still the modelled 404.
#[tokio::test]
async fn cancelling_a_job_that_cannot_be_canceled_is_still_refused() {
    // A settle span short enough that the job leaves SUBMITTED during the case.
    let emulator = Emulator::with_settle_span(SHORT_SETTLE);
    let created = emulator
        .create_job(json!({
            "role": ROLE,
            "settings": job_settings("s3://example-bucket/in/a.mp4", "_a")
        }))
        .await;
    assert_eq!(
        created.status, 201,
        "the job must be created, got {} with body {}",
        created.status, created.body
    );
    let id = created.string("/job/id");

    ::tokio::time::sleep(PAST_THE_SETTLE).await;
    let settled = emulator.get_job(&id).await;
    assert_eq!(
        settled.string("/job/status"),
        "ERROR",
        "the job must have left SUBMITTED, got {}",
        settled.body
    );

    for attempt in 1..=2 {
        let refused = emulator.cancel_job(&id).await;
        assert_eq!(
            refused.status, 409,
            "case attempt {attempt}: a job that is not CANCELED and not SUBMITTED must \
             conflict, got {} with body {}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_type.as_deref(),
            Some("ConflictException"),
            "case attempt {attempt}: expected ConflictException, got {:?}",
            refused.error_type
        );
    }

    for missing in ["nothing-here", "1700000000000-zzzzzz"] {
        let refused = emulator.cancel_job(missing).await;
        assert_eq!(
            refused.status, 404,
            "case {missing:?}: expected HTTP 404, got {} with body {}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_type.as_deref(),
            Some("NotFoundException"),
            "case {missing:?}: expected NotFoundException, got {:?}",
            refused.error_type
        );
    }
}
