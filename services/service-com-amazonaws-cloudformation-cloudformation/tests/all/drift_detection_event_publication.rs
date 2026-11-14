// SPDX-License-Identifier: CC0-1.0

//! What a drift detection serves while it is PUBLISHING its status changes onto the
//! account's default EventBridge bus — asked of a router that mounts no bus at all.
//!
//! The rule under test: **publishing a drift detection's status changes is best effort, so
//! nothing about serving the detection depends on anything consuming them.** Every status a
//! drift detection reaches is published onto the account's default event bus
//! (`state::drift_events`, observed end to end in the router crate's
//! `cloudformation_drift_detection_status_change_events.rs`, where EventBridge and the queue
//! a rule delivers to are mounted beside this service); here the same code path runs with
//! NOWHERE to publish to, which is what an emulator carrying no EventBridge is. The
//! detection must still be accepted with an id of its own, must still conclude
//! `DETECTION_COMPLETE` on the clock, and must still report the verdict and the drifted
//! count the comparison really found — the very values the published events carry.
//!
//! The rows differ along what the rule generalises over: how many resources the stack
//! declares, how many of them were changed outside the stack, and whether the detection
//! covers all of them or the `LogicalResourceIds` filter narrows it to one — so the verdict
//! and the count vary (IN_SYNC/0, DRIFTED/1 of 1, DRIFTED/1 of 2, IN_SYNC/0 under a filter)
//! rather than one recorded outcome being re-asserted.

use std::time::{Duration, Instant};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{call, member, router, xml_text};
use crate::update_rollback_recovery::ssm;
use service_com_amazonaws_cloudformation_cloudformation::state::{
    drift_detection_status, DRIFT_STATUS_DRIFTED, DRIFT_STATUS_IN_SYNC,
};

/// How long a poll waits for something the clock owes, and how often it looks: generous,
/// since what it waits for is due in milliseconds and the host may be busy.
const BUDGET: Duration = Duration::from_secs(30);
const LOOK_INTERVAL: Duration = Duration::from_millis(10);

/// One `AWS::SSM::Parameter` declaration, of the one resource type this crate's test router
/// mounts the owning service for and whose configuration that service reports back.
fn parameter(name: &str, value: &str) -> Value {
    json!({
        "Type": "AWS::SSM::Parameter",
        "Properties": { "Name": name, "Type": "String", "Value": value },
    })
}

/// Deploy `template` as the stack `name` and poll it to a terminal status, which must be
/// `CREATE_COMPLETE`.
async fn deployed(router: &MultiServiceRouter, case: &str, name: &str, template: &Value) {
    let (status, body) = call(
        router,
        "CreateStack",
        &[
            member("StackName", name),
            member("TemplateBody", &template.to_string()),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateStack answered {status} with {body}"
    );
    let deadline = Instant::now() + BUDGET;
    loop {
        let (status, body) = call(router, "DescribeStacks", &[member("StackName", name)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeStacks answered {status} with {body}"
        );
        let reported = xml_text(&body, "StackStatus").unwrap_or_default();
        if !reported.ends_with("_IN_PROGRESS") {
            assert_eq!(
                reported, "CREATE_COMPLETE",
                "case {case:?}: expected the create to complete, got {body}"
            );
            return;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: the stack was still {reported} at the deadline"
        );
        tokio::time::sleep(LOOK_INTERVAL).await;
    }
}

/// Start a detection on `stack`, optionally narrowed to some logical ids, and answer the
/// `StackDriftDetectionId` it was accepted under.
async fn detect(
    router: &MultiServiceRouter,
    case: &str,
    stack: &str,
    logical_ids: &[&str],
) -> String {
    let mut members = vec![member("StackName", stack)];
    for (index, logical_id) in logical_ids.iter().enumerate() {
        members.push(member(
            &format!("LogicalResourceIds.member.{}", index + 1),
            logical_id,
        ));
    }
    let (status, body) = call(router, "DetectStackDrift", &members).await;
    assert_eq!(
        status, 200,
        "case {case:?}: DetectStackDrift answered {status} with {body}"
    );
    xml_text(&body, "StackDriftDetectionId")
        .unwrap_or_else(|| panic!("case {case:?}: expected a detection id, got {body}"))
}

/// Poll `DescribeStackDriftDetectionStatus` until the detection concludes, and answer the
/// `(StackId, DetectionStatus, StackDriftStatus, DriftedStackResourceCount)` it reports.
async fn concluded(
    router: &MultiServiceRouter,
    case: &str,
    detection_id: &str,
) -> (String, String, String, String) {
    let deadline = Instant::now() + BUDGET;
    loop {
        let (status, body) = call(
            router,
            "DescribeStackDriftDetectionStatus",
            &[member("StackDriftDetectionId", detection_id)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeStackDriftDetectionStatus answered {status} with {body}"
        );
        let detection_status = xml_text(&body, "DetectionStatus").unwrap_or_default();
        if detection_status != drift_detection_status::DETECTION_IN_PROGRESS {
            return (
                xml_text(&body, "StackId").unwrap_or_default(),
                detection_status,
                xml_text(&body, "StackDriftStatus").unwrap_or_default(),
                xml_text(&body, "DriftedStackResourceCount").unwrap_or_default(),
            );
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: the detection was still in progress at the deadline: {body}"
        );
        tokio::time::sleep(LOOK_INTERVAL).await;
    }
}

/// Write another value into the parameter `name`, outside the stack that deployed it.
async fn changed_out_of_band(router: &MultiServiceRouter, case: &str, name: &str, value: &str) {
    let (status, body) = ssm(
        router,
        "PutParameter",
        &json!({ "Name": name, "Value": value, "Type": "String", "Overwrite": true }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: PutParameter({name}) answered {status} with {body}"
    );
}

/// One row: a stack, what was changed outside it, what the detection covers, and the result
/// the detection must therefore report.
struct Row {
    case: &'static str,
    stack: &'static str,
    template: Value,
    /// The parameter names changed outside the stack, with the value written into each.
    changed: Vec<(&'static str, &'static str)>,
    /// The `LogicalResourceIds` filter, empty for a detection over the whole stack.
    covers: Vec<&'static str>,
    verdict: &'static str,
    drifted: &'static str,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "one parameter, nothing changed outside the stack",
            stack: "drift-events-local-in-sync",
            template: json!({
                "Resources": { "First": parameter("/drift-events-local/in-sync", "declared") },
            }),
            changed: Vec::new(),
            covers: Vec::new(),
            verdict: DRIFT_STATUS_IN_SYNC,
            drifted: "0",
        },
        Row {
            case: "one parameter, changed outside the stack",
            stack: "drift-events-local-one",
            template: json!({
                "Resources": { "First": parameter("/drift-events-local/one", "declared") },
            }),
            changed: vec![("/drift-events-local/one", "changed-by-hand")],
            covers: Vec::new(),
            verdict: DRIFT_STATUS_DRIFTED,
            drifted: "1",
        },
        Row {
            case: "two parameters, one of them changed outside the stack",
            stack: "drift-events-local-two",
            template: json!({
                "Resources": {
                    "First": parameter("/drift-events-local/two-first", "declared"),
                    "Second": parameter("/drift-events-local/two-second", "declared"),
                },
            }),
            changed: vec![("/drift-events-local/two-first", "changed-by-hand")],
            covers: Vec::new(),
            verdict: DRIFT_STATUS_DRIFTED,
            drifted: "1",
        },
        Row {
            case: "a detection narrowed to the resource nobody changed",
            stack: "drift-events-local-filtered",
            template: json!({
                "Resources": {
                    "First": parameter("/drift-events-local/filtered-first", "declared"),
                    "Second": parameter("/drift-events-local/filtered-second", "declared"),
                },
            }),
            changed: vec![("/drift-events-local/filtered-first", "changed-by-hand")],
            covers: vec!["Second"],
            verdict: DRIFT_STATUS_IN_SYNC,
            drifted: "0",
        },
    ]
}

/// A detection is accepted, concludes on the clock and reports what the comparison found,
/// with nothing mounted to receive the events it publishes.
#[tokio::test]
async fn a_detection_is_served_whether_or_not_anything_consumes_its_events() {
    for row in rows() {
        let case = row.case;
        // A router carrying this service and the ones that OWN the deployed resources —
        // and no event bus at all, which is what this suite is asking about.
        let router = router();
        deployed(&router, case, row.stack, &row.template).await;
        for (name, value) in &row.changed {
            changed_out_of_band(&router, case, name, value).await;
        }

        let detection_id = detect(&router, case, row.stack, &row.covers).await;
        assert!(
            !detection_id.is_empty(),
            "case {case:?}: the detection must be accepted under an id of its own"
        );

        let (stack_id, status, verdict, drifted) = concluded(&router, case, &detection_id).await;
        assert_eq!(
            (status.as_str(), verdict.as_str(), drifted.as_str()),
            (
                drift_detection_status::DETECTION_COMPLETE,
                row.verdict,
                row.drifted
            ),
            "case {case:?}: expected the detection to conclude complete with {:?} and {:?}, got \
             {status:?}, {verdict:?} and {drifted:?}",
            row.verdict,
            row.drifted
        );
        assert!(
            stack_id.contains(&format!(":stack/{}/", row.stack)),
            "case {case:?}: the detection must report the stack it ran against, got {stack_id:?}"
        );
    }
}

/// Two detections of one stack are two operations: each is accepted under its own id and
/// reports its own result, so the second sees the change the first ran before.
#[tokio::test]
async fn each_detection_of_one_stack_reports_its_own_result() {
    let case = "two detections of one stack, the second after an out-of-band change";
    let router = router();
    let stack = "drift-events-local-twice";
    let template = json!({
        "Resources": { "First": parameter("/drift-events-local/twice", "declared") },
    });
    deployed(&router, case, stack, &template).await;

    let first = detect(&router, case, stack, &[]).await;
    let (_, status, verdict, drifted) = concluded(&router, case, &first).await;
    assert_eq!(
        (status.as_str(), verdict.as_str(), drifted.as_str()),
        (
            drift_detection_status::DETECTION_COMPLETE,
            DRIFT_STATUS_IN_SYNC,
            "0"
        ),
        "case {case:?}: the first detection must find the stack in sync, got {status:?}, \
         {verdict:?} and {drifted:?}"
    );

    changed_out_of_band(&router, case, "/drift-events-local/twice", "changed-by-hand").await;
    let second = detect(&router, case, stack, &[]).await;
    assert_ne!(
        second, first,
        "case {case:?}: each detection is its own operation, so it carries its own id"
    );
    let (_, status, verdict, drifted) = concluded(&router, case, &second).await;
    assert_eq!(
        (status.as_str(), verdict.as_str(), drifted.as_str()),
        (
            drift_detection_status::DETECTION_COMPLETE,
            DRIFT_STATUS_DRIFTED,
            "1"
        ),
        "case {case:?}: the second detection must find the changed resource, got {status:?}, \
         {verdict:?} and {drifted:?}"
    );

    // The first detection's own record is untouched by the second: each reports the check
    // that ran.
    let (_, status, verdict, drifted) = concluded(&router, case, &first).await;
    assert_eq!(
        (status.as_str(), verdict.as_str(), drifted.as_str()),
        (
            drift_detection_status::DETECTION_COMPLETE,
            DRIFT_STATUS_IN_SYNC,
            "0"
        ),
        "case {case:?}: the first detection's result must stand, got {status:?}, {verdict:?} and \
         {drifted:?}"
    );
}
