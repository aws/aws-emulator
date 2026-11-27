// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of the job-template write path: `CreateJobTemplate` persists
//! a template under its name, `GetJobTemplate` projects the stored record back,
//! `UpdateJobTemplate` changes the members it names and `DeleteJobTemplate` removes it.
//!
//! Every case drives the real restJson1 wire path — the router picks the service from the
//! request's SigV4 scope, the framework deserializes and validates, the handler runs, and
//! the response is serialized — so what is asserted is the bytes a client would see,
//! never an internal call. The listing side is `tests/all/job_templates.rs`.

use serde_json::{json, Value};
use service_com_amazonaws_mediaconvert_mediaconvert::ops::types::JobTemplateSettings;
use service_com_amazonaws_mediaconvert_mediaconvert::state::{
    default_acceleration_settings, JobTemplateRecord, DEFAULT_PRIORITY,
    DEFAULT_STATUS_UPDATE_INTERVAL, TYPE_CUSTOM,
};
use service_com_amazonaws_mediaconvert_mediaconvert::ComAmazonawsMediaconvertMediaconvertState;

// The router, the SigV4 scope, the reply decoding, the ARN helper and the request helpers
// are the crate's ONE test harness, exported by `job_lifecycle`: this suite drives the
// same wire path through it rather than keeping a second copy of it.
use super::job_lifecycle::{
    arn, arn_in, elemental_inference_forms, feed_arn, malformed_feed_arns, queue_arn,
    settings as job_settings, Emulator, ROLE,
};

/// A queue the fixture creates, so a case can point a template at a real queue.
const FIXTURE_QUEUE: &str = "templates-queue";

/// A structurally real `JobTemplateSettings` with `inputs` input templates and `groups`
/// output groups. A job template's `Inputs` are `InputTemplate`s, which carry no file
/// input — the file comes from the job — so the input is described by its timecode
/// source.
///
/// `pub(crate)` because the suites about repeated creates build their templates with it
/// too, so one fixture serves every job-template suite.
pub(crate) fn settings(inputs: usize, groups: usize) -> Value {
    let inputs: Vec<Value> = (0..inputs)
        .map(|_| json!({ "timecodeSource": "ZEROBASED" }))
        .collect();
    let output_groups: Vec<Value> = (0..groups)
        .map(|index| {
            json!({
                "name": format!("File Group {index}"),
                // Each output states the container it is written in: `Output$Preset` is
                // "either Preset or Container settings", so one of the two must be there.
                "outputs": [{
                    "nameModifier": format!("_{index}"),
                    "containerSettings": { "container": "MP4" }
                }],
                "outputGroupSettings": {
                    "type": "FILE_GROUP_SETTINGS",
                    "fileGroupSettings": { "destination": format!("s3://example-bucket/out{index}/") }
                }
            })
        })
        .collect();
    json!({ "inputs": inputs, "outputGroups": output_groups })
}

/// How many entries a JSON array member holds, or `usize::MAX` when it is not an array —
/// a value an assertion prints rather than a panic.
fn count(value: &Value) -> usize {
    value.as_array().map_or(usize::MAX, Vec::len)
}

/// Assert every member of `expected` against `view`, where a `null` expectation asserts
/// the member is absent (or null) in the view.
fn assert_members(case: &str, view: &Value, expected: &Value) {
    let members = expected
        .as_object()
        .map(|members| members.iter().collect::<Vec<_>>())
        .unwrap_or_default();
    for (member, want) in members {
        let got = view.pointer(&format!("/{member}")).unwrap_or(&Value::Null);
        assert_eq!(
            got, want,
            "case {case:?}: expected {member} to be {want}, got {got} in {view}"
        );
    }
}

/// One row of the round-trip table: a `CreateJobTemplate` request, and the members the
/// stored template must carry afterwards.
struct Row {
    case: &'static str,
    request: Value,
    /// `jobTemplate` members whose value is asserted exactly; `null` asserts absence.
    expected: Value,
    /// The number of `Settings.Inputs` and `Settings.OutputGroups` entries expected.
    inputs: usize,
    groups: usize,
}

/// `CreateJobTemplate` stores the template the caller described under its name, and
/// `GetJobTemplate` answers with that stored record — every member the caller set, the
/// `CUSTOM` type, the resource's ARN and its timestamps.
///
/// The table spans the class: which optional members are present at all, the counts
/// inside `Settings`, the form a queue is referenced in (name or ARN), a list-valued
/// member, and an enum-valued one.
#[tokio::test]
async fn a_created_job_template_reads_back_through_get() {
    let emulator = Emulator::new();
    let queue = emulator
        .create_queue(json!({ "name": FIXTURE_QUEUE }))
        .await;
    assert_eq!(
        queue.status, 201,
        "the fixture queue must be created: got {} with body {}",
        queue.status, queue.body
    );

    let rows = vec![
        Row {
            case: "only the required members",
            request: json!({ "name": "minimal", "settings": settings(1, 1) }),
            expected: json!({
                "name": "minimal",
                "category": null,
                "description": null,
                "queue": null,
                // The three members the service applies when the request omits them.
                "priority": 0,
                "statusUpdateInterval": "SECONDS_60",
                "accelerationSettings": { "mode": "DISABLED" },
                "hopDestinations": [],
            }),
            inputs: 1,
            groups: 1,
        },
        Row {
            case: "a category and a description",
            request: json!({
                "name": "described",
                "category": "nightly",
                "description": "the nightly ladder",
                "settings": settings(1, 1),
            }),
            expected: json!({
                "name": "described",
                "category": "nightly",
                "description": "the nightly ladder",
                "queue": null,
            }),
            inputs: 1,
            groups: 1,
        },
        Row {
            case: "priority at the lower bound",
            request: json!({ "name": "lowest", "priority": -50, "settings": settings(1, 1) }),
            expected: json!({ "name": "lowest", "priority": -50 }),
            inputs: 1,
            groups: 1,
        },
        Row {
            case: "priority at the upper bound",
            request: json!({ "name": "highest", "priority": 50, "settings": settings(1, 1) }),
            expected: json!({ "name": "highest", "priority": 50 }),
            inputs: 1,
            groups: 1,
        },
        Row {
            case: "the one input a template may carry and three output groups",
            request: json!({ "name": "wide", "settings": settings(1, 3) }),
            expected: json!({ "name": "wide" }),
            inputs: 1,
            groups: 3,
        },
        Row {
            case: "a queue referenced by name",
            request: json!({
                "name": "queued-by-name",
                "queue": FIXTURE_QUEUE,
                "settings": settings(1, 1),
            }),
            expected: json!({ "name": "queued-by-name", "queue": queue_arn(FIXTURE_QUEUE) }),
            inputs: 1,
            groups: 1,
        },
        Row {
            case: "the same queue referenced by ARN",
            request: json!({
                "name": "queued-by-arn",
                "queue": arn(&format!("queues/{FIXTURE_QUEUE}")),
                "settings": settings(1, 1),
            }),
            expected: json!({ "name": "queued-by-arn", "queue": queue_arn(FIXTURE_QUEUE) }),
            inputs: 1,
            groups: 1,
        },
        Row {
            case: "an enum member and an acceleration mode",
            request: json!({
                "name": "accelerated",
                // `StatusUpdateInterval` and `AccelerationMode` values, from
                // smithy_query(query="shape",shape="StatusUpdateInterval,AccelerationMode").
                "statusUpdateInterval": "SECONDS_120",
                "accelerationSettings": { "mode": "PREFERRED" },
                "settings": settings(1, 1),
            }),
            expected: json!({
                "name": "accelerated",
                "statusUpdateInterval": "SECONDS_120",
                "accelerationSettings": { "mode": "PREFERRED" },
            }),
            inputs: 1,
            groups: 1,
        },
        Row {
            case: "two hop destinations, one naming its queue by ARN",
            request: json!({
                "name": "hopping",
                "hopDestinations": [
                    { "queue": arn(&format!("queues/{FIXTURE_QUEUE}")), "priority": 10, "waitMinutes": 15 },
                    { "priority": -10, "waitMinutes": 4320 },
                ],
                "settings": settings(1, 1),
            }),
            expected: json!({
                "name": "hopping",
                "hopDestinations": [
                    { "queue": queue_arn(FIXTURE_QUEUE), "priority": 10, "waitMinutes": 15 },
                    { "priority": -10, "waitMinutes": 4320 },
                ],
            }),
            inputs: 1,
            groups: 1,
        },
        Row {
            case: "a hop destination naming its queue by name",
            request: json!({
                "name": "hopping-by-name",
                "hopDestinations": [{ "queue": FIXTURE_QUEUE, "waitMinutes": 1 }],
                "settings": settings(1, 1),
            }),
            // The same destination, named the other way, reads back in the same form.
            expected: json!({
                "name": "hopping-by-name",
                "hopDestinations": [{ "queue": queue_arn(FIXTURE_QUEUE), "waitMinutes": 1 }],
            }),
            inputs: 1,
            groups: 1,
        },
    ];

    for row in &rows {
        let Row {
            case,
            request,
            expected,
            inputs,
            groups,
        } = row;

        let created = emulator.create_job_template(request.clone()).await;
        assert_eq!(
            created.status, 201,
            "case {case:?}: expected HTTP 201, got {} with body {}",
            created.status, created.body
        );

        let name = expected
            .pointer("/name")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let fetched = emulator.get_job_template(&name).await;
        assert_eq!(
            fetched.status, 200,
            "case {case:?}: expected the template to read back with HTTP 200, got {} with body {}",
            fetched.status, fetched.body
        );

        // The reader projects the stored record, so it answers exactly what the create
        // returned — not a second, separately computed view.
        assert_eq!(
            fetched.template(),
            created.template(),
            "case {case:?}: GetJobTemplate must answer what CreateJobTemplate persisted, \
             got {} after {}",
            fetched.template(),
            created.template()
        );

        for view in [created.template(), fetched.template()] {
            assert_members(case, view, expected);
            assert_eq!(
                view.pointer("/type").and_then(Value::as_str),
                Some("CUSTOM"),
                "case {case:?}: a template a caller created is CUSTOM, got {view}"
            );
            assert_eq!(
                view.pointer("/arn").and_then(Value::as_str),
                Some(arn(&format!("jobTemplates/{name}")).as_str()),
                "case {case:?}: expected the job-template ARN, got {view}"
            );
            let created_at = view.pointer("/createdAt").cloned().unwrap_or(Value::Null);
            let last_updated = view.pointer("/lastUpdated").cloned().unwrap_or(Value::Null);
            assert!(
                created_at.as_i64().is_some(),
                "case {case:?}: expected createdAt in epoch seconds, got {created_at}"
            );
            assert_eq!(
                created_at, last_updated,
                "case {case:?}: a template nothing has updated was last updated when it \
                 was created, got createdAt {created_at} and lastUpdated {last_updated}"
            );
            let got_inputs = count(view.pointer("/settings/inputs").unwrap_or(&Value::Null));
            assert_eq!(
                got_inputs, *inputs,
                "case {case:?}: expected {inputs} inputs, got {got_inputs} in {view}"
            );
            let got_groups = count(view.pointer("/settings/outputGroups").unwrap_or(&Value::Null));
            assert_eq!(
                got_groups, *groups,
                "case {case:?}: expected {groups} output groups, got {got_groups} in {view}"
            );
        }
    }

    // Reading never changes what a later caller sees: every template still reads back
    // identically after the whole table has been read once.
    for row in &rows {
        let name = row
            .expected
            .pointer("/name")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let first = emulator.get_job_template(name).await;
        let second = emulator.get_job_template(name).await;
        assert_eq!(
            first.template(),
            second.template(),
            "case {:?}: repeated reads must answer the same record, got {} then {}",
            row.case,
            first.template(),
            second.template()
        );
    }
}

/// `Priority` is `__integerMinNegative50Max50` — "range -50..50"
/// (`smithy_query(query="operation",operation="CreateJobTemplate")`). Every value inside
/// the range is stored and reads back; a value past either published bound is refused
/// with the modelled `BadRequestException` and persists nothing.
#[tokio::test]
async fn priority_is_stored_inside_its_published_range_and_refused_outside_it() {
    let emulator = Emulator::new();

    for priority in [-50_i64, -1, 0, 1, 50] {
        let name = format!("priority{priority}");
        let created = emulator
            .create_job_template(json!({
                "name": name,
                "priority": priority,
                "settings": settings(1, 1),
            }))
            .await;
        assert_eq!(
            created.status, 201,
            "case priority {priority}: expected HTTP 201 inside the range, got {} with body {}",
            created.status, created.body
        );
        let fetched = emulator.get_job_template(&name).await;
        assert_eq!(
            fetched.at("/jobTemplate/priority").as_i64(),
            Some(priority),
            "case priority {priority}: expected the stored priority, got {}",
            fetched.body
        );
    }

    for priority in [-51_i64, 51, i64::from(i32::MIN), i64::from(i32::MAX)] {
        let name = format!("outside{priority}");
        let refused = emulator
            .create_job_template(json!({
                "name": name,
                "priority": priority,
                "settings": settings(1, 1),
            }))
            .await;
        assert_eq!(
            refused.status, 400,
            "case priority {priority}: expected HTTP 400 outside the range, got {} with body {}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_type.as_deref(),
            Some("BadRequestException"),
            "case priority {priority}: expected BadRequestException, got {:?}",
            refused.error_type
        );
        let fetched = emulator.get_job_template(&name).await;
        assert_eq!(
            fetched.status, 404,
            "case priority {priority}: a refused create must persist nothing, got {} with body {}",
            fetched.status, fetched.body
        );
    }
}

/// A second create under a name already in use is IDEMPOTENT: the same 201 with the
/// template that is already stored, whatever the second request's body asks for, and the
/// stored template is left as the first create wrote it.
#[tokio::test]
async fn a_name_already_in_use_returns_the_stored_template() {
    let emulator = Emulator::new();
    let first = emulator
        .create_job_template(json!({
            "name": "taken",
            "description": "the first one",
            "settings": settings(1, 1),
        }))
        .await;
    assert_eq!(
        first.status, 201,
        "the first create must succeed: got {} with body {}",
        first.status, first.body
    );

    for (case, request) in [
        (
            "the same description",
            json!({ "name": "taken", "description": "the first one", "settings": settings(1, 1) }),
        ),
        (
            "a different description",
            json!({ "name": "taken", "description": "the second one", "settings": settings(1, 2) }),
        ),
    ] {
        let again = emulator.create_job_template(request).await;
        assert_eq!(
            again.status, 201,
            "case {case:?}: expected HTTP 201 for a name in use, got {} with body {}",
            again.status, again.body
        );
        assert_eq!(
            again.template(),
            first.template(),
            "case {case:?}: the repeat must return the STORED template, got {} after {}",
            again.template(),
            first.template()
        );
    }

    let fetched = emulator.get_job_template("taken").await;
    assert_eq!(
        fetched.template(),
        first.template(),
        "a repeated create must leave the stored template untouched, got {} after {}",
        fetched.template(),
        first.template()
    );
}

/// A queue a template names is a reference to a queue resource, in either of the forms a
/// queue reference takes: an unknown one is the modelled `NotFoundException` and persists
/// nothing, and the same request succeeds once the queue exists.
#[tokio::test]
async fn a_template_naming_an_unknown_queue_is_not_found() {
    let emulator = Emulator::new();

    let cases = [
        ("an unknown queue by name", "no-such-queue".to_string()),
        (
            "an unknown queue by ARN",
            arn("queues/no-such-queue-either"),
        ),
    ];

    for (index, (case, reference)) in cases.iter().enumerate() {
        let name = format!("pointing-{index}");
        let refused = emulator
            .create_job_template(json!({
                "name": name,
                "queue": reference,
                "settings": settings(1, 1),
            }))
            .await;
        assert_eq!(
            refused.status, 404,
            "case {case:?}: expected HTTP 404 for an unknown queue, got {} with body {}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_type.as_deref(),
            Some("NotFoundException"),
            "case {case:?}: expected NotFoundException, got {:?}",
            refused.error_type
        );
        let fetched = emulator.get_job_template(&name).await;
        assert_eq!(
            fetched.status, 404,
            "case {case:?}: a refused create must persist nothing, got {} with body {}",
            fetched.status, fetched.body
        );
    }

    // The built-in `Default` queue exists without anybody creating it, so a template may
    // name it in either form straight away.
    for (index, reference) in ["Default".to_string(), arn("queues/Default")]
        .into_iter()
        .enumerate()
    {
        let name = format!("on-default-{index}");
        let created = emulator
            .create_job_template(json!({
                "name": name,
                "queue": reference,
                "settings": settings(1, 1),
            }))
            .await;
        assert_eq!(
            created.status, 201,
            "case {reference:?}: expected HTTP 201 for the built-in queue, got {} with body {}",
            created.status, created.body
        );
        assert_eq!(
            created.at("/jobTemplate/queue").as_str(),
            Some(queue_arn("Default").as_str()),
            "case {reference:?}: expected the ARN of the resolved queue, got {}",
            created.body
        );
    }
}

/// A template nobody created does not exist: `GetJobTemplate` answers the modelled 404
/// rather than an empty template.
#[tokio::test]
async fn getting_a_template_that_was_never_created_is_not_found() {
    let emulator = Emulator::new();
    let created = emulator
        .create_job_template(json!({ "name": "present", "settings": settings(1, 1) }))
        .await;
    assert_eq!(
        created.status, 201,
        "the fixture template must be created: got {} with body {}",
        created.status, created.body
    );

    for case in ["absent", "present-not", "Default", "PRESENT"] {
        let fetched = emulator.get_job_template(case).await;
        assert_eq!(
            fetched.status, 404,
            "case {case:?}: expected HTTP 404, got {} with body {}",
            fetched.status, fetched.body
        );
        assert_eq!(
            fetched.error_type.as_deref(),
            Some("NotFoundException"),
            "case {case:?}: expected NotFoundException, got {:?}",
            fetched.error_type
        );
    }
}

/// A job template with every optional member set, so an update case can show that the
/// members it does NOT name keep the value this request stored.
fn populated_request(name: &str) -> Value {
    json!({
        "name": name,
        "category": "original-category",
        "description": "the original description",
        "priority": 5,
        "statusUpdateInterval": "SECONDS_60",
        "accelerationSettings": { "mode": "DISABLED" },
        "hopDestinations": [{ "priority": 1, "waitMinutes": 10 }],
        "settings": settings(1, 1),
    })
}

/// The view members [`populated_request`] stores, before any update.
fn populated_view(name: &str) -> Value {
    json!({
        "name": name,
        "category": "original-category",
        "description": "the original description",
        "queue": null,
        "priority": 5,
        "statusUpdateInterval": "SECONDS_60",
        "accelerationSettings": { "mode": "DISABLED" },
        "hopDestinations": [{ "priority": 1, "waitMinutes": 10 }],
    })
}

/// One row of the update table: the members the update names, and the members the stored
/// template must carry afterwards.
struct UpdateRow {
    case: &'static str,
    update: Value,
    /// The view members whose value the update changes; everything else must keep the
    /// value [`populated_request`] stored.
    changed: Value,
    inputs: usize,
    groups: usize,
}

/// Each member of `UpdateJobTemplate` is "the new <member> for the job template, if you
/// are changing it": a member the caller sends replaces the stored one, a member it
/// leaves out keeps its stored value, and the change is what every later read answers.
///
/// The table spans the class: each member that can change, by type (string, integer at a
/// published bound, enum, structure, list, the whole `Settings`), a list emptied rather
/// than replaced, a queue referenced by ARN, and an update that names nothing at all.
#[tokio::test]
async fn an_update_replaces_only_the_members_it_names_and_reflects_in_get() {
    let emulator = Emulator::new();
    let queue = emulator
        .create_queue(json!({ "name": FIXTURE_QUEUE }))
        .await;
    assert_eq!(
        queue.status, 201,
        "the fixture queue must be created: got {} with body {}",
        queue.status, queue.body
    );

    let rows = vec![
        UpdateRow {
            case: "a new category",
            update: json!({ "category": "nightly" }),
            changed: json!({ "category": "nightly" }),
            inputs: 1,
            groups: 1,
        },
        UpdateRow {
            case: "a new description",
            update: json!({ "description": "the new description" }),
            changed: json!({ "description": "the new description" }),
            inputs: 1,
            groups: 1,
        },
        UpdateRow {
            case: "a new priority at the lower bound",
            update: json!({ "priority": -50 }),
            changed: json!({ "priority": -50 }),
            inputs: 1,
            groups: 1,
        },
        UpdateRow {
            case: "a new queue referenced by ARN",
            update: json!({ "queue": arn(&format!("queues/{FIXTURE_QUEUE}")) }),
            changed: json!({ "queue": queue_arn(FIXTURE_QUEUE) }),
            inputs: 1,
            groups: 1,
        },
        UpdateRow {
            case: "the same new queue referenced by name",
            update: json!({ "queue": FIXTURE_QUEUE }),
            changed: json!({ "queue": queue_arn(FIXTURE_QUEUE) }),
            inputs: 1,
            groups: 1,
        },
        UpdateRow {
            case: "a new status update interval",
            update: json!({ "statusUpdateInterval": "SECONDS_600" }),
            changed: json!({ "statusUpdateInterval": "SECONDS_600" }),
            inputs: 1,
            groups: 1,
        },
        UpdateRow {
            case: "new acceleration settings",
            update: json!({ "accelerationSettings": { "mode": "ENABLED" } }),
            changed: json!({ "accelerationSettings": { "mode": "ENABLED" } }),
            inputs: 1,
            groups: 1,
        },
        UpdateRow {
            case: "new hop destinations",
            update: json!({ "hopDestinations": [{ "priority": -5, "waitMinutes": 2 }] }),
            changed: json!({ "hopDestinations": [{ "priority": -5, "waitMinutes": 2 }] }),
            inputs: 1,
            groups: 1,
        },
        UpdateRow {
            case: "a new hop destination naming its queue by name",
            update: json!({ "hopDestinations": [{ "queue": FIXTURE_QUEUE, "waitMinutes": 2 }] }),
            changed: json!({
                "hopDestinations": [{ "queue": queue_arn(FIXTURE_QUEUE), "waitMinutes": 2 }],
            }),
            inputs: 1,
            groups: 1,
        },
        UpdateRow {
            case: "hop destinations emptied",
            update: json!({ "hopDestinations": [] }),
            changed: json!({ "hopDestinations": [] }),
            inputs: 1,
            groups: 1,
        },
        UpdateRow {
            case: "wholly new settings",
            update: json!({ "settings": settings(1, 3) }),
            changed: json!({}),
            inputs: 1,
            groups: 3,
        },
        UpdateRow {
            case: "two members at once",
            update: json!({ "category": "both", "priority": 50 }),
            changed: json!({ "category": "both", "priority": 50 }),
            inputs: 1,
            groups: 1,
        },
        UpdateRow {
            case: "no member at all",
            update: json!({}),
            changed: json!({}),
            inputs: 1,
            groups: 1,
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let UpdateRow {
            case,
            update,
            changed,
            inputs,
            groups,
        } = row;
        let name = format!("updated-{index}");

        let created = emulator.create_job_template(populated_request(&name)).await;
        assert_eq!(
            created.status, 201,
            "case {case:?}: the fixture template must be created, got {} with body {}",
            created.status, created.body
        );

        let updated = emulator.update_job_template(&name, update.clone()).await;
        assert_eq!(
            updated.status, 200,
            "case {case:?}: expected HTTP 200 from the update, got {} with body {}",
            updated.status, updated.body
        );

        // What the stored template must look like: the untouched members, overridden by
        // the ones this row changes.
        let mut expected = populated_view(&name);
        if let (Some(target), Some(source)) = (expected.as_object_mut(), changed.as_object()) {
            for (member, value) in source {
                target.insert(member.clone(), value.clone());
            }
        }

        let fetched = emulator.get_job_template(&name).await;
        assert_eq!(
            fetched.status, 200,
            "case {case:?}: expected the updated template to read back, got {} with body {}",
            fetched.status, fetched.body
        );
        assert_eq!(
            fetched.template(),
            updated.template(),
            "case {case:?}: GetJobTemplate must answer what UpdateJobTemplate persisted, \
             got {} after {}",
            fetched.template(),
            updated.template()
        );

        for view in [updated.template(), fetched.template()] {
            assert_members(case, view, &expected);
            let got_inputs = count(view.pointer("/settings/inputs").unwrap_or(&Value::Null));
            assert_eq!(
                got_inputs, *inputs,
                "case {case:?}: expected {inputs} inputs after the update, got {got_inputs} \
                 in {view}"
            );
            let got_groups = count(view.pointer("/settings/outputGroups").unwrap_or(&Value::Null));
            assert_eq!(
                got_groups, *groups,
                "case {case:?}: expected {groups} output groups after the update, got \
                 {got_groups} in {view}"
            );
            // An update never re-creates the resource, and it is now more recently
            // updated than created.
            assert_eq!(
                view.pointer("/createdAt"),
                created.template().pointer("/createdAt"),
                "case {case:?}: an update must not move createdAt, got {view}"
            );
            let created_at = view.pointer("/createdAt").and_then(Value::as_i64);
            let last_updated = view.pointer("/lastUpdated").and_then(Value::as_i64);
            assert!(
                created_at.is_some() && last_updated >= created_at,
                "case {case:?}: expected lastUpdated at or after createdAt, got \
                 {last_updated:?} and {created_at:?}"
            );
        }
    }
}

/// An update is a modification of an existing template, so a name nothing created and a
/// queue nothing created are both the modelled 404 — and the refused update leaves the
/// stored template exactly as it was.
#[tokio::test]
async fn an_update_that_cannot_be_applied_is_not_found_and_changes_nothing() {
    let emulator = Emulator::new();
    let created = emulator
        .create_job_template(populated_request("present"))
        .await;
    assert_eq!(
        created.status, 201,
        "the fixture template must be created: got {} with body {}",
        created.status, created.body
    );

    let cases = [
        (
            "a template nobody created",
            "absent".to_string(),
            json!({ "description": "whatever" }),
        ),
        (
            "an unknown queue by name",
            "present".to_string(),
            json!({ "queue": "no-such-queue" }),
        ),
        (
            "an unknown queue by ARN",
            "present".to_string(),
            json!({ "queue": arn("queues/no-such-queue-either") }),
        ),
    ];

    for (case, name, update) in cases {
        let refused = emulator.update_job_template(&name, update).await;
        assert_eq!(
            refused.status, 404,
            "case {case:?}: expected HTTP 404, got {} with body {}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_type.as_deref(),
            Some("NotFoundException"),
            "case {case:?}: expected NotFoundException, got {:?}",
            refused.error_type
        );
        let fetched = emulator.get_job_template("present").await;
        assert_eq!(
            fetched.template(),
            created.template(),
            "case {case:?}: a refused update must leave the stored template untouched, \
             got {} after {}",
            fetched.template(),
            created.template()
        );
    }
}

/// "Permanently delete a job template you have created": the deleted template is the one
/// named and only that one, a later read of it is the modelled 404, and deleting a
/// template that is not stored is the delete's own empty 202 — the delete is idempotent
/// on the name even though the read is not.
///
/// The table spans the class by the deleted template's position among the ones created —
/// the first, a middle and the last name — so a delete is shown to remove one record,
/// not a range.
#[tokio::test]
async fn deleting_a_job_template_removes_only_that_template() {
    let names = ["alpha", "middle", "omega"];

    for target in names {
        let emulator = Emulator::new();
        for name in names {
            let created = emulator
                .create_job_template(json!({ "name": name, "settings": settings(1, 1) }))
                .await;
            assert_eq!(
                created.status, 201,
                "case {target:?}: the fixture template {name:?} must be created, got {} \
                 with body {}",
                created.status, created.body
            );
        }

        let deleted = emulator.delete_job_template(target).await;
        assert_eq!(
            deleted.status, 202,
            "case {target:?}: expected HTTP 202 from the delete, got {} with body {}",
            deleted.status, deleted.body
        );

        for name in names {
            let fetched = emulator.get_job_template(name).await;
            let expected = if name == target { 404 } else { 200 };
            assert_eq!(
                fetched.status, expected,
                "case {target:?}: expected HTTP {expected} reading {name:?} after the \
                 delete, got {} with body {}",
                fetched.status, fetched.body
            );
        }

        // Deleting it again has nothing to delete, which is the same empty 202: the
        // delete is idempotent on the name, as `DeleteQueue`'s is.
        let again = emulator.delete_job_template(target).await;
        assert_eq!(
            again.status, 202,
            "case {target:?}: expected HTTP 202 deleting it twice, got {} with body {}",
            again.status, again.body
        );
        // And the read of that name is still the modelled 404.
        let still_gone = emulator.get_job_template(target).await;
        assert_eq!(
            still_gone.status, 404,
            "case {target:?}: the name must stay unreadable, got {} with body {}",
            still_gone.status, still_gone.body
        );

        // And the name is free again, so a create under it succeeds.
        let recreated = emulator
            .create_job_template(json!({ "name": target, "settings": settings(1, 2) }))
            .await;
        assert_eq!(
            recreated.status, 201,
            "case {target:?}: a deleted name must be free again, got {} with body {}",
            recreated.status, recreated.body
        );
    }
}

/// A queue member the caller SENT is a value they meant: an empty or blank one is the
/// modelled `BadRequestException`, on create and on update, and neither stores nor
/// changes anything — a caller is never left unable to tell "queue set" from "queue
/// ignored".
#[tokio::test]
async fn an_empty_queue_reference_is_a_bad_request() {
    let emulator = Emulator::new();
    let created = emulator
        .create_job_template(json!({
            "name": "has-a-queue",
            "queue": "Default",
            "settings": settings(1, 1),
        }))
        .await;
    assert_eq!(
        created.status, 201,
        "the fixture template must be created: got {} with body {}",
        created.status, created.body
    );

    for (index, blank) in ["", " ", "\t", "   "].into_iter().enumerate() {
        let name = format!("blank-queue-{index}");
        let refused = emulator
            .create_job_template(json!({
                "name": name,
                "queue": blank,
                "settings": settings(1, 1),
            }))
            .await;
        assert_eq!(
            refused.status, 400,
            "case create with queue {blank:?}: expected HTTP 400, got {} with body {}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_type.as_deref(),
            Some("BadRequestException"),
            "case create with queue {blank:?}: expected BadRequestException, got {:?}",
            refused.error_type
        );
        let fetched = emulator.get_job_template(&name).await;
        assert_eq!(
            fetched.status, 404,
            "case create with queue {blank:?}: a refused create must persist nothing, got \
             {} with body {}",
            fetched.status, fetched.body
        );

        let updated = emulator
            .update_job_template("has-a-queue", json!({ "queue": blank }))
            .await;
        assert_eq!(
            updated.status, 400,
            "case update with queue {blank:?}: expected HTTP 400, got {} with body {}",
            updated.status, updated.body
        );
        assert_eq!(
            updated.error_type.as_deref(),
            Some("BadRequestException"),
            "case update with queue {blank:?}: expected BadRequestException, got {:?}",
            updated.error_type
        );
        let after = emulator.get_job_template("has-a-queue").await;
        assert_eq!(
            after.template(),
            created.template(),
            "case update with queue {blank:?}: a refused update must change nothing, got \
             {} after {}",
            after.template(),
            created.template()
        );
    }
}

/// "System or built-in job templates can't be modified or deleted by the user"
/// (`JobTemplate$Type`): every built-in template refuses an update and a delete with the
/// modelled `ForbiddenException`, and still reads back unchanged afterwards.
///
/// The names come from `ListJobTemplates?listBy=SYSTEM`, so the case spans whatever
/// built-in catalogue the store holds rather than hard-coding an entry of it.
#[tokio::test]
async fn a_built_in_template_cannot_be_updated_or_deleted() {
    let emulator = Emulator::new();
    let listed = emulator.list_job_templates("listBy=SYSTEM").await;
    assert_eq!(
        listed.status, 200,
        "the built-in templates must list: got {} with body {}",
        listed.status, listed.body
    );
    let names: Vec<String> = listed
        .at("/jobTemplates")
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.pointer("/name").and_then(Value::as_str))
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    assert!(
        !names.is_empty(),
        "expected at least one built-in template to test, got body {}",
        listed.body
    );

    for name in names {
        let before = emulator.get_job_template(&name).await;
        assert_eq!(
            before.at("/jobTemplate/type").as_str(),
            Some("SYSTEM"),
            "case {name:?}: expected a built-in template, got {}",
            before.body
        );

        let updated = emulator
            .update_job_template(&name, json!({ "description": "mine now" }))
            .await;
        assert_eq!(
            updated.status, 403,
            "case {name:?}: expected HTTP 403 from updating a built-in template, got {} \
             with body {}",
            updated.status, updated.body
        );
        assert_eq!(
            updated.error_type.as_deref(),
            Some("ForbiddenException"),
            "case {name:?}: expected ForbiddenException from the update, got {:?}",
            updated.error_type
        );

        let deleted = emulator.delete_job_template(&name).await;
        assert_eq!(
            deleted.status, 403,
            "case {name:?}: expected HTTP 403 from deleting a built-in template, got {} \
             with body {}",
            deleted.status, deleted.body
        );
        assert_eq!(
            deleted.error_type.as_deref(),
            Some("ForbiddenException"),
            "case {name:?}: expected ForbiddenException from the delete, got {:?}",
            deleted.error_type
        );

        let after = emulator.get_job_template(&name).await;
        assert_eq!(
            after.template(),
            before.template(),
            "case {name:?}: a refused update and delete must leave the built-in template \
             untouched, got {} after {}",
            after.template(),
            before.template()
        );
    }
}

/// MediaConvert reports a reference to a resource the account does not have by naming the
/// REQUEST MEMBER the reference arrived on, never the value: "The resource you specified
/// in &lt;member&gt; was not found." (the wording AWS itself answers, as the recorded
/// `mediaconvert.job-template-update-reflects-in-get` unit asserts for `name`).
///
/// The table spans the class: every member a missing reference can arrive on in this
/// service — the `name` label of a job template and of a queue, the `id` label of a job,
/// and the `queue` body member of a job and of a job template, on create and on update.
#[tokio::test]
async fn a_missing_resource_is_reported_by_the_member_that_named_it() {
    let emulator = Emulator::new();
    let template = emulator
        .create_job_template(json!({ "name": "present", "settings": settings(1, 1) }))
        .await;
    assert_eq!(
        template.status, 201,
        "the fixture template must be created: got {} with body {}",
        template.status, template.body
    );

    let unknown_queue = arn("queues/no-such-queue");
    let cases: Vec<(&str, &str, String, Option<Value>, &str)> = vec![
        (
            "GetJobTemplate for a template nobody created",
            "GET",
            "/2017-08-29/jobTemplates/absent".to_string(),
            None,
            "name",
        ),
        (
            "UpdateJobTemplate for a template nobody created",
            "PUT",
            "/2017-08-29/jobTemplates/absent".to_string(),
            Some(json!({ "description": "whatever" })),
            "name",
        ),
        (
            "CreateJobTemplate pointing at a queue nobody created",
            "POST",
            "/2017-08-29/jobTemplates".to_string(),
            Some(json!({
                "name": "pointing",
                "queue": unknown_queue,
                "settings": settings(1, 1),
            })),
            "queue",
        ),
        (
            "UpdateJobTemplate pointing at a queue nobody created",
            "PUT",
            "/2017-08-29/jobTemplates/present".to_string(),
            Some(json!({ "queue": unknown_queue })),
            "queue",
        ),
        (
            "CreateJob on a queue nobody created",
            "POST",
            "/2017-08-29/jobs".to_string(),
            Some(json!({
                "role": ROLE,
                "queue": unknown_queue,
                "settings": job_settings("s3://example-bucket/in/a.mp4", "_1"),
            })),
            "queue",
        ),
        (
            "GetJob for a job nobody created",
            "GET",
            "/2017-08-29/jobs/1700000000000-abcdef".to_string(),
            None,
            "id",
        ),
        (
            "CancelJob for a job nobody created",
            "DELETE",
            "/2017-08-29/jobs/1700000000000-abcdef".to_string(),
            None,
            "id",
        ),
        (
            "GetQueue for a queue nobody created",
            "GET",
            "/2017-08-29/queues/absent".to_string(),
            None,
            "name",
        ),
    ];

    for (case, method, uri, body, member) in cases {
        let refused = emulator.send(method, &uri, body).await;
        assert_eq!(
            refused.status, 404,
            "case {case:?}: expected HTTP 404, got {} with body {}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_type.as_deref(),
            Some("NotFoundException"),
            "case {case:?}: expected NotFoundException, got {:?}",
            refused.error_type
        );
        let expected = format!("The resource you specified in {member} was not found.");
        assert_eq!(
            refused.at("/message").as_str(),
            Some(expected.as_str()),
            "case {case:?}: expected the message to name the {member} member as \
             {expected:?}, got {}",
            refused.body
        );
    }
}

/// Caller-driven growth is bounded: once the store holds [`MAX_JOB_TEMPLATES`] templates,
/// `CreateJobTemplate` answers the modelled quota error instead of growing further, and
/// nothing is stored.
///
/// The store is seeded to capacity directly — a thousand creates over HTTP would say the
/// same thing a thousand times more slowly — and the refusal itself is asserted over the
/// wire, for several names, with the templates already there still readable.
#[tokio::test]
async fn creating_past_the_store_capacity_is_a_quota_refusal() {
    // The capacity the state module publishes, so this test cannot drift from it.
    let capacity = service_com_amazonaws_mediaconvert_mediaconvert::state::MAX_JOB_TEMPLATES;
    let emulator = Emulator::new();

    let seeded_at = ::std::time::SystemTime::now();
    {
        let state = emulator
            .engine
            .state::<ComAmazonawsMediaconvertMediaconvertState>();
        let mut state = state.lock();
        // The built-in system templates already occupy their slots, so seed up to the
        // cap, never past it.
        let mut index = 0;
        while state.job_templates.len() < capacity {
            let name = format!("bulk-{index:05}");
            state.job_templates.insert(
                name.clone(),
                JobTemplateRecord {
                    name,
                    category: Some("bulk".to_string()),
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

    for name in ["one-too-many", "and-another"] {
        let refused = emulator
            .create_job_template(json!({ "name": name, "settings": settings(1, 1) }))
            .await;
        assert_eq!(
            refused.status, 402,
            "case {name:?}: expected HTTP 402 at capacity, got {} with body {}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_type.as_deref(),
            Some("ServiceQuotaExceededException"),
            "case {name:?}: expected ServiceQuotaExceededException, got {:?}",
            refused.error_type
        );
        let fetched = emulator.get_job_template(name).await;
        assert_eq!(
            fetched.status, 404,
            "case {name:?}: a refused create must persist nothing, got {} with body {}",
            fetched.status, fetched.body
        );
    }

    // A template already in the store still reads back, so the refusal did not break it.
    let existing = emulator.get_job_template("bulk-00000").await;
    assert_eq!(
        existing.status, 200,
        "an existing template must still read back at capacity, got {} with body {}",
        existing.status, existing.body
    );
}

/// A job template's ARN names the region of the request that reads it, as every ARN this
/// service builds does — not the region of whichever caller created it.
#[tokio::test]
async fn a_job_template_arn_names_the_region_of_the_request_that_reads_it() {
    let emulator = Emulator::new();
    let created = emulator
        .send_in(
            "eu-central-1",
            "POST",
            "/2017-08-29/jobTemplates",
            Some(json!({ "name": "regional", "settings": settings(1, 1) })),
        )
        .await;
    assert_eq!(
        created.status, 201,
        "the fixture template must be created: got {} with body {}",
        created.status, created.body
    );

    for region in ["eu-central-1", "us-west-2", "ap-southeast-2"] {
        let fetched = emulator
            .send_in(region, "GET", "/2017-08-29/jobTemplates/regional", None)
            .await;
        assert_eq!(
            fetched.at("/jobTemplate/arn").as_str(),
            Some(arn_in(region, "jobTemplates/regional").as_str()),
            "case {region:?}: expected the template ARN in the reading region, got {}",
            fetched.body
        );
    }
}

/// `JobTemplate$ElementalInferenceSettings` is a member the service applies no value of
/// its own for, so it follows the write rules every other optional member of a template
/// follows: `CreateJobTemplate` stores the settings the request states and
/// `GetJobTemplate` answers exactly those, a create that states none leaves the member
/// absent, an update that names it REPLACES the stored value, and an update that does not
/// name it leaves the stored value as it was.
///
/// The table spans the forms the member takes ([`super::job_lifecycle::
/// elemental_inference_forms`]), each row updated to the row after it so the replacement
/// is between two different stated values rather than onto the same one.
#[tokio::test]
async fn a_templates_elemental_inference_settings_are_stored_read_back_and_replaced() {
    let emulator = Emulator::new();
    let forms = elemental_inference_forms();

    for (index, (case, stated)) in forms.iter().enumerate() {
        let name = format!("inference-{index}");
        let mut request = json!({ "name": name, "settings": settings(1, 1) });
        if !stated.is_null() {
            request["elementalInferenceSettings"] = stated.clone();
        }

        let created = emulator.create_job_template(request).await;
        assert_eq!(
            created.status, 201,
            "case {case:?}: expected HTTP 201, got {} with body {}",
            created.status, created.body
        );
        assert_eq!(
            created.at("/jobTemplate/elementalInferenceSettings"),
            stated,
            "case {case:?}: expected the stated settings {stated} back, got {}",
            created.body
        );

        let fetched = emulator.get_job_template(&name).await;
        assert_eq!(
            fetched.status, 200,
            "case {case:?}: expected the template to read back, got {} with body {}",
            fetched.status, fetched.body
        );
        assert_eq!(
            fetched.at("/jobTemplate/elementalInferenceSettings"),
            stated,
            "case {case:?}: GetJobTemplate must answer what CreateJobTemplate persisted, \
             expected {stated}, got {}",
            fetched.body
        );

        // An update that names another member leaves these settings exactly as stored...
        let touched = emulator
            .update_job_template(&name, json!({ "description": "untouched" }))
            .await;
        assert_eq!(
            touched.status, 200,
            "case {case:?}: expected HTTP 200 from the update, got {} with body {}",
            touched.status, touched.body
        );
        assert_eq!(
            touched.at("/jobTemplate/elementalInferenceSettings"),
            stated,
            "case {case:?}: an update that does not name the member must keep {stated}, \
             got {}",
            touched.body
        );

        // ... and an update that DOES name it replaces the stored value with the next
        // row's, which every later read then answers.
        let (replacement_case, replacement) = &forms[(index + 1) % forms.len()];
        if replacement.is_null() {
            continue;
        }
        let updated = emulator
            .update_job_template(
                &name,
                json!({ "elementalInferenceSettings": replacement.clone() }),
            )
            .await;
        assert_eq!(
            updated.status, 200,
            "case {case:?} -> {replacement_case:?}: expected HTTP 200, got {} with body {}",
            updated.status, updated.body
        );
        let reread = emulator.get_job_template(&name).await;
        for view in [updated.template(), reread.template()] {
            assert_eq!(
                view.pointer("/elementalInferenceSettings").unwrap_or(&Value::Null),
                replacement,
                "case {case:?} -> {replacement_case:?}: the update must replace the stored \
                 settings with {replacement}, got {view}"
            );
        }
    }
}

/// A feed ARN outside the pattern `ElementalInferenceFeedConfigurationSource$FeedArn`
/// publishes is the modelled `BadRequestException` on both job-template write doors, and
/// the refused call neither stores a template nor changes the stored one.
#[tokio::test]
async fn a_template_feed_arn_outside_its_published_pattern_is_a_bad_request() {
    let emulator = Emulator::new();
    let good = json!({
        "feedConfigurationSource": { "feedArn": feed_arn("abcdefghij123456789") }
    });
    let standing = emulator
        .create_job_template(json!({
            "name": "standing",
            "elementalInferenceSettings": good.clone(),
            "settings": settings(1, 1),
        }))
        .await;
    assert_eq!(
        standing.status, 201,
        "a feed ARN inside the published pattern must be accepted, got {} with body {}",
        standing.status, standing.body
    );

    for (case, arn) in malformed_feed_arns() {
        let settings_member = json!({ "feedConfigurationSource": { "feedArn": arn } });
        let created = emulator
            .create_job_template(json!({
                "name": "refused",
                "elementalInferenceSettings": settings_member.clone(),
                "settings": settings(1, 1),
            }))
            .await;
        assert_eq!(
            created.status, 400,
            "case {case:?}: expected HTTP 400 from the create, got {} with body {}",
            created.status, created.body
        );
        assert_eq!(
            created.error_type.as_deref(),
            Some("BadRequestException"),
            "case {case:?}: expected BadRequestException, got {:?} with body {}",
            created.error_type,
            created.body
        );
        let absent = emulator.get_job_template("refused").await;
        assert_eq!(
            absent.status, 404,
            "case {case:?}: a refused create must store no template, got {} with body {}",
            absent.status, absent.body
        );

        let updated = emulator
            .update_job_template(
                "standing",
                json!({ "elementalInferenceSettings": settings_member }),
            )
            .await;
        assert_eq!(
            updated.status, 400,
            "case {case:?}: expected HTTP 400 from the update, got {} with body {}",
            updated.status, updated.body
        );
        let unchanged = emulator.get_job_template("standing").await;
        assert_eq!(
            unchanged.at("/jobTemplate/elementalInferenceSettings"),
            &good,
            "case {case:?}: a refused update must leave the stored settings as they were, \
             got {}",
            unchanged.body
        );
    }
}

