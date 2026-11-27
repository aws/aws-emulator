// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `UpdateLaunchConfigurationTemplate` and the read that
//! reports what it left behind (`DescribeLaunchConfigurationTemplates`).
//!
//! Every test drives the service the way the router does — one buffered
//! `http::Request` per call through [`MultiServiceRouter::dispatch`] — so what is
//! asserted is the wire behaviour (status, error code, JSON body), not a handler's Rust
//! return value. The harness (the router, `call`, `Answer`) is the one
//! `replication_configuration_template_lifecycle` builds.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one recorded
//! request:
//!
//! 1. A read of a launch configuration template reports every setting as it stands after
//!    the most recent accepted mutation of that template: an update replaces each setting
//!    it supplies, leaves every setting it does not supply exactly as it was, and reports
//!    the stored record — so the answer and a following read agree, member for member.
//! 2. A setting the create never supplied is absent until an update supplies it, and
//!    stays absent otherwise: the service invents no value for a member nobody set.
//! 3. An id the store does not hold is the modelled `ResourceNotFoundException` (404)
//!    naming the resource type and the id the request asked for, and writes nothing —
//!    neither to the addressed template nor to any other.
//! 4. A setting carrying a value outside its modelled enumeration is refused as a request
//!    body this service cannot read, and the stored template keeps every value it had.

use serde_json::{json, Map, Value};
use service_com_amazonaws_drs_elasticdisasterrecoveryservice::ops::types::{
    LAUNCH_DISPOSITION_VALUES, RECOVERY_MODE_VALUES,
    TARGET_INSTANCE_TYPE_RIGHT_SIZING_METHOD_VALUES,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::replication_configuration_template_lifecycle::{call, drs, items, Answer};

/// The account id every generated emulator presents (`engine::EMULATOR_ACCOUNT_ID`).
const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// The code the model declares for a template this service does not hold
/// (`smithy_query(query="operation",operation="UpdateLaunchConfigurationTemplate")` lists
/// `ResourceNotFoundException`, HTTP 404), and the service's name for the resource type.
const NOT_FOUND_CODE: &str = "ResourceNotFoundException";
const RESOURCE_TYPE: &str = "Launch Configuration Template";

/// The error code and the whole message this service answers a request body that does not
/// satisfy its model with — written out as the literals a client reads.
const BAD_REQUEST_CODE: &str = "BadRequestException";
const INVALID_REQUEST_BODY: &str = "Invalid request body";

/// The settings of a launch configuration template an update can carry, as the model's
/// `UpdateLaunchConfigurationTemplateRequest` names them
/// (`smithy_query(query="operation",operation="UpdateLaunchConfigurationTemplate")`): every
/// input member beside the id the request addresses the template by.
pub(crate) const SETTINGS: [&str; 9] = [
    "launchDisposition",
    "targetInstanceTypeRightSizingMethod",
    "recoveryMode",
    "copyPrivateIp",
    "copyTags",
    "licensing",
    "exportBucketArn",
    "postLaunchEnabled",
    "launchIntoSourceInstance",
];

/// Create a template from `body`, asserting the modelled 201, and answer the created
/// template as the service reported it.
pub(crate) async fn create_template(router: &MultiServiceRouter, body: &Value) -> Value {
    let answer = call(router, "CreateLaunchConfigurationTemplate", body).await;
    assert_eq!(
        answer.status, 201,
        "creating {body}: expected 201, got {answer:?}"
    );
    reported(&answer)
}

/// The `launchConfigurationTemplate` member of a create's or an update's answer.
pub(crate) fn reported(answer: &Answer) -> Value {
    answer
        .member("launchConfigurationTemplate")
        .cloned()
        .unwrap_or_else(|| panic!("expected a reported template, got {answer:?}"))
}

/// A string member of a JSON object.
pub(crate) fn text(value: &Value, name: &str) -> String {
    value
        .get(name)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// `UpdateLaunchConfigurationTemplate` for `id` with `patch`'s settings.
pub(crate) async fn update(router: &MultiServiceRouter, id: &str, patch: &Value) -> Answer {
    let mut body = Map::new();
    body.insert(
        "launchConfigurationTemplateID".to_string(),
        Value::String(id.to_string()),
    );
    for (member, value) in patch.as_object().expect("a JSON object patch") {
        body.insert(member.clone(), value.clone());
    }
    call(
        router,
        "UpdateLaunchConfigurationTemplate",
        &Value::Object(body),
    )
    .await
}

/// The template `id` is reported as, through the read that reports templates.
pub(crate) async fn read_back(router: &MultiServiceRouter, id: &str) -> Value {
    let answer = call(
        router,
        "DescribeLaunchConfigurationTemplates",
        &json!({"launchConfigurationTemplateIDs": [id]}),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "describing {id}: expected 200, got {answer:?}"
    );
    items(&answer)
        .first()
        .cloned()
        .unwrap_or_else(|| panic!("expected {id} to be reported, got {answer:?}"))
}

/// `stored` with every member of `patch` overlaid — what the rule says the template
/// reports after the update, computed from the rule rather than listed per row.
fn overlaid(stored: &Value, patch: &Value) -> Value {
    let mut expected = stored
        .as_object()
        .expect("the stored template is a JSON object")
        .clone();
    for (member, value) in patch.as_object().expect("a JSON object patch") {
        expected.insert(member.clone(), value.clone());
    }
    Value::Object(expected)
}

/// A create body carrying every setting, so each row below can be shown to replace the
/// settings it names and only those.
fn every_setting_set() -> Value {
    json!({
        "tags": {"env": "prod"},
        "launchDisposition": LAUNCH_DISPOSITION_VALUES[0],
        "targetInstanceTypeRightSizingMethod": TARGET_INSTANCE_TYPE_RIGHT_SIZING_METHOD_VALUES[0],
        "recoveryMode": RECOVERY_MODE_VALUES[0],
        "copyPrivateIp": true,
        "copyTags": true,
        "licensing": {"osByol": true},
        "exportBucketArn": format!("arn:aws:s3:::drs-exports-{ACCOUNT}"),
        "postLaunchEnabled": true,
        "launchIntoSourceInstance": true,
    })
}

/// The update bodies rule 1 spans: the rows differ along which settings they supply and
/// which value each carries. Every modelled enumeration is covered at EVERY one of its
/// values, from the value sets the model generated
/// (`crate::…::ops::types::{LAUNCH_DISPOSITION_VALUES,
/// TARGET_INSTANCE_TYPE_RIGHT_SIZING_METHOD_VALUES, RECOVERY_MODE_VALUES}`), so no row is
/// hard-coded from a recording and each enumeration is driven to the value it already
/// carries as well as to the others.
fn update_rows() -> Vec<(String, Value)> {
    let mut rows: Vec<(String, Value)> = vec![
        ("no setting at all".to_string(), json!({})),
        (
            "every boolean cleared".to_string(),
            json!({
                "copyPrivateIp": false,
                "copyTags": false,
                "postLaunchEnabled": false,
                "launchIntoSourceInstance": false,
            }),
        ),
        (
            "one boolean cleared, the others untouched".to_string(),
            json!({"copyPrivateIp": false}),
        ),
        (
            "licensing".to_string(),
            json!({"licensing": {"osByol": false}}),
        ),
        (
            "an export bucket".to_string(),
            json!({"exportBucketArn": format!("arn:aws:s3:::drs-exports-replaced-{ACCOUNT}")}),
        ),
    ];
    for value in LAUNCH_DISPOSITION_VALUES {
        rows.push((
            format!("launchDisposition {value}"),
            json!({"launchDisposition": value}),
        ));
    }
    for value in TARGET_INSTANCE_TYPE_RIGHT_SIZING_METHOD_VALUES {
        rows.push((
            format!("targetInstanceTypeRightSizingMethod {value}"),
            json!({"targetInstanceTypeRightSizingMethod": value}),
        ));
    }
    for value in RECOVERY_MODE_VALUES {
        rows.push((format!("recoveryMode {value}"), json!({"recoveryMode": value})));
    }
    rows.push((
        "every setting at once".to_string(),
        json!({
            "launchDisposition": LAUNCH_DISPOSITION_VALUES[LAUNCH_DISPOSITION_VALUES.len() - 1],
            "targetInstanceTypeRightSizingMethod":
                TARGET_INSTANCE_TYPE_RIGHT_SIZING_METHOD_VALUES
                    [TARGET_INSTANCE_TYPE_RIGHT_SIZING_METHOD_VALUES.len() - 1],
            "recoveryMode": RECOVERY_MODE_VALUES[RECOVERY_MODE_VALUES.len() - 1],
            "copyPrivateIp": false,
            "copyTags": false,
            "licensing": {"osByol": false},
            "exportBucketArn": format!("arn:aws:s3:::drs-exports-all-{ACCOUNT}"),
            "postLaunchEnabled": false,
            "launchIntoSourceInstance": false,
        }),
    ));
    rows
}

// ---------------------------------------------------------------------------
// Rule 1 — an update replaces what it supplies and the read reports it
// ---------------------------------------------------------------------------

/// An update replaces every setting it supplies, leaves every other setting as it was,
/// and the read reports the stored record: the answer and the following read agree member
/// for member, and repeating the same update leaves the same template.
#[tokio::test]
async fn an_update_replaces_the_settings_it_supplies_and_the_read_reports_them() {
    let router = drs();
    for (case, patch) in update_rows() {
        let created = create_template(&router, &every_setting_set()).await;
        let id = text(&created, "launchConfigurationTemplateID");
        let expected = overlaid(&created, &patch);

        let answer = update(&router, &id, &patch).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected 200, got {answer:?}"
        );
        assert_eq!(
            reported(&answer),
            expected,
            "case {case:?}: expected the update to report {expected}, got {answer:?}"
        );

        let read = read_back(&router, &id).await;
        assert_eq!(
            read, expected,
            "case {case:?}: expected the read to report {expected}, got {read}"
        );

        // The id and the ARN are the service's own: an update cannot move the template.
        assert_eq!(
            (text(&read, "launchConfigurationTemplateID"), text(&read, "arn")),
            (id.clone(), text(&created, "arn")),
            "case {case:?}: the id and ARN must survive the update, got {read}"
        );

        // The same update applied again leaves the same template, so what a read reports
        // is a function of the settings in force and not of how often they were written.
        let again = update(&router, &id, &patch).await;
        assert_eq!(
            reported(&again),
            expected,
            "case {case:?}: a repeated update must leave {expected}, got {again:?}"
        );
        assert_eq!(
            read_back(&router, &id).await,
            expected,
            "case {case:?}: a repeated update must leave {expected} to be read back"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 2 — a setting nobody set stays absent
// ---------------------------------------------------------------------------

/// A setting the create never supplied is absent until an update supplies it: an update
/// naming ONE setting on a template created with none leaves exactly that setting
/// reported, and every other setting still absent. The rows are each settable member of
/// the operation on its own.
#[tokio::test]
async fn a_setting_nobody_supplied_stays_absent_until_an_update_supplies_it() {
    // One value per setting, each inside the member's modelled domain.
    let values: Vec<(&str, Value)> = vec![
        (
            "launchDisposition",
            json!(LAUNCH_DISPOSITION_VALUES[LAUNCH_DISPOSITION_VALUES.len() - 1]),
        ),
        (
            "targetInstanceTypeRightSizingMethod",
            json!(
                TARGET_INSTANCE_TYPE_RIGHT_SIZING_METHOD_VALUES
                    [TARGET_INSTANCE_TYPE_RIGHT_SIZING_METHOD_VALUES.len() - 1]
            ),
        ),
        (
            "recoveryMode",
            json!(RECOVERY_MODE_VALUES[RECOVERY_MODE_VALUES.len() - 1]),
        ),
        ("copyPrivateIp", json!(true)),
        ("copyTags", json!(false)),
        ("licensing", json!({"osByol": true})),
        (
            "exportBucketArn",
            json!(format!("arn:aws:s3:::drs-exports-{ACCOUNT}")),
        ),
        ("postLaunchEnabled", json!(true)),
        ("launchIntoSourceInstance", json!(false)),
    ];
    assert_eq!(
        values.len(),
        SETTINGS.len(),
        "every settable member of the operation must have a row, got {values:?}"
    );

    let router = drs();
    for (member, value) in &values {
        let created = create_template(&router, &json!({})).await;
        let id = text(&created, "launchConfigurationTemplateID");
        for setting in SETTINGS {
            assert!(
                created.get(setting).is_none(),
                "case {member:?}: a create that supplied no setting must report none, \
                 got {setting} in {created}"
            );
        }

        let mut patch = Map::new();
        patch.insert((*member).to_string(), value.clone());
        let answer = update(&router, &id, &Value::Object(patch)).await;
        assert_eq!(
            answer.status, 200,
            "case {member:?}: expected 200, got {answer:?}"
        );

        let read = read_back(&router, &id).await;
        assert_eq!(
            read.get(*member),
            Some(value),
            "case {member:?}: expected {value} to be reported, got {read}"
        );
        for setting in SETTINGS.iter().filter(|setting| *setting != member) {
            assert!(
                read.get(setting).is_none(),
                "case {member:?}: {setting} was never supplied, so it must stay absent, \
                 got {read}"
            );
        }
    }
}

/// An update carries no `tags` member at all, so the tag set a template was created with
/// is the set it still carries after any update. The rows differ along the set the create
/// supplied and along what the update changes.
#[tokio::test]
async fn an_update_leaves_the_tag_set_the_create_supplied() {
    let rows: Vec<(&str, Value, Value)> = vec![
        ("no tag set", json!({}), json!({"copyTags": true})),
        ("one tag", json!({"env": "prod"}), json!({"copyPrivateIp": true})),
        (
            "several tags",
            json!({"env": "prod", "team": "dr"}),
            json!({"recoveryMode": RECOVERY_MODE_VALUES[0]}),
        ),
    ];

    let router = drs();
    for (case, tags, patch) in &rows {
        let created = create_template(&router, &json!({"tags": tags.clone()})).await;
        let id = text(&created, "launchConfigurationTemplateID");

        let answer = update(&router, &id, patch).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected 200, got {answer:?}"
        );
        assert_eq!(
            reported(&answer).get("tags"),
            Some(tags),
            "case {case:?}: the update must report {tags}, got {answer:?}"
        );
        assert_eq!(
            read_back(&router, &id).await.get("tags"),
            Some(tags),
            "case {case:?}: the template must still carry {tags}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 3 — an id the store does not hold
// ---------------------------------------------------------------------------

/// An update of an id the store does not hold is the modelled `ResourceNotFoundException`,
/// naming the resource type and the id the request asked for, and writes nothing. The rows
/// span the ids such a request can name: one nothing minted, one of another id shape inside
/// the modelled pattern, and one the store held and has given up.
#[tokio::test]
async fn an_update_of_an_unknown_id_is_not_found_and_writes_nothing() {
    let router = drs();
    let held = create_template(&router, &every_setting_set()).await;
    let held_id = text(&held, "launchConfigurationTemplateID");

    let gone = create_template(&router, &json!({})).await;
    let gone_id = text(&gone, "launchConfigurationTemplateID");
    let deleted = call(
        &router,
        "DeleteLaunchConfigurationTemplate",
        &json!({"launchConfigurationTemplateID": gone_id}),
    )
    .await;
    assert_eq!(
        deleted.status, 204,
        "deleting {gone_id}: expected 204, got {deleted:?}"
    );

    // Each id matches the modelled `^lct-[0-9a-zA-Z]{17}$`.
    let rows: Vec<(&str, String)> = vec![
        ("an id nothing minted", "lct-00000000000000000".to_string()),
        (
            "an id nothing minted, mixed case",
            "lct-aBcDeFgHiJkLmNoP0".to_string(),
        ),
        ("an id the store has given up", gone_id.clone()),
    ];

    for (case, id) in &rows {
        let answer = update(
            &router,
            id,
            &json!({"launchDisposition": LAUNCH_DISPOSITION_VALUES[0]}),
        )
        .await;
        assert_eq!(
            answer.status, 404,
            "case {case:?}: expected 404 for {id}, got {answer:?}"
        );
        assert_eq!(
            answer.error_code.as_deref(),
            Some(NOT_FOUND_CODE),
            "case {case:?}: expected {NOT_FOUND_CODE}, got {answer:?}"
        );
        assert_eq!(
            text(&answer.body, "resourceType"),
            RESOURCE_TYPE,
            "case {case:?}: wrong resourceType in {answer:?}"
        );
        assert_eq!(
            text(&answer.body, "resourceId"),
            *id,
            "case {case:?}: resourceId must be the id the request named, got {answer:?}"
        );
        assert_eq!(
            text(&answer.body, "message"),
            format!("{RESOURCE_TYPE} with Id: '{id}' was not found"),
            "case {case:?}: wrong message in {answer:?}"
        );
        assert!(
            answer.member("launchConfigurationTemplate").is_none(),
            "case {case:?}: a refusal must report no template, got {answer:?}"
        );
    }

    // No refusal wrote anything: the template that IS held still reports what the create
    // stored for it.
    assert_eq!(
        read_back(&router, &held_id).await,
        held,
        "the held template must be untouched by every refused update"
    );
}

// ---------------------------------------------------------------------------
// Rule 4 — a value outside a modelled enumeration
// ---------------------------------------------------------------------------

/// A setting carrying a value its modelled enumeration does not list is refused as a
/// request body this service cannot read, and the stored template keeps every value it
/// had. The rows span the operation's enumerated members and the ways a value can fall
/// outside the set: an unknown token, the wrong case of a listed value, a value another of
/// the operation's enumerations lists, and the empty string.
#[tokio::test]
async fn an_update_value_outside_a_modelled_enumeration_is_refused() {
    let router = drs();
    let created = create_template(&router, &every_setting_set()).await;
    let id = text(&created, "launchConfigurationTemplateID");

    // (member, the values its enumeration lists, a value another member's does)
    let members: [(&str, &[&str], &str); 3] = [
        (
            "launchDisposition",
            LAUNCH_DISPOSITION_VALUES,
            RECOVERY_MODE_VALUES[0],
        ),
        (
            "targetInstanceTypeRightSizingMethod",
            TARGET_INSTANCE_TYPE_RIGHT_SIZING_METHOD_VALUES,
            LAUNCH_DISPOSITION_VALUES[0],
        ),
        (
            "recoveryMode",
            RECOVERY_MODE_VALUES,
            TARGET_INSTANCE_TYPE_RIGHT_SIZING_METHOD_VALUES[0],
        ),
    ];

    for (member, listed, foreign) in members {
        let lowercased = listed[0].to_ascii_lowercase();
        let outside = [
            ("an unknown token", "BOGUS_VALUE"),
            ("the wrong case of a listed value", lowercased.as_str()),
            ("a value of another enumeration", foreign),
            ("the empty string", ""),
        ];
        for (why, value) in outside {
            // Only a value this enumeration does not list can be refused.
            if listed.contains(&value) {
                continue;
            }
            let case = format!("{member} = {value:?} ({why})");
            let mut patch = Map::new();
            patch.insert(member.to_string(), Value::String(value.to_string()));
            let answer = update(&router, &id, &Value::Object(patch)).await;
            assert_eq!(
                answer.status, 400,
                "case {case:?}: expected 400, got {answer:?}"
            );
            assert_eq!(
                answer.error_code.as_deref(),
                Some(BAD_REQUEST_CODE),
                "case {case:?}: expected {BAD_REQUEST_CODE}, got {answer:?}"
            );
            assert_eq!(
                text(&answer.body, "message"),
                INVALID_REQUEST_BODY,
                "case {case:?}: wrong message in {answer:?}"
            );
            assert_eq!(
                read_back(&router, &id).await,
                created,
                "case {case:?}: a refused update must leave the template as it was"
            );
        }
    }
}
