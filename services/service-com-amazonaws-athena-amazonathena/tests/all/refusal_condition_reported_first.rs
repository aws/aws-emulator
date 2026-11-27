// SPDX-License-Identifier: CC0-1.0

//! WHICH failure condition a refusal reports, driven end to end over the same router the
//! emulator boots (awsJson1_1, `x-amz-target: AmazonAthena.<Operation>`).
//!
//! Three operations answer the right status and the right error shape for more than one
//! condition, and what a client branches on is the condition itself — the sentence of a
//! refusal, or the per-item code and message of a partial answer. The rules under test:
//!
//! * `PutCapacityAssignmentConfiguration` resolves the workgroups its `CapacityAssignments`
//!   name BEFORE the capacity reservation it is addressed to, so a put carrying a workgroup
//!   this account does not hold reports that condition whatever the state of the named
//!   reservation, and only a put whose every assigned workgroup resolves can report the
//!   reservation as missing.
//! * `GetResourceDashboard` checks the FORM of its `ResourceARN` before it resolves
//!   anything: a value out of which no session reference can be read at all — one that is not
//!   a complete ARN (six colon-separated fields, the first literally `arn`, and the
//!   partition, the vendor, the Region, the account and the resource all non-empty), or one
//!   whose resource field does not spell out a session — is an input-shape failure, worded
//!   apart from a value that does name a session and resolves none.
//! * `BatchGetPreparedStatement` answers a name the workgroup holds no statement under inside
//!   `UnprocessedPreparedStatementNames`, under the class a missing statement name carries and
//!   with the reason NAMING that statement — the only place a partial answer reports which of
//!   the submitted names failed.
//!
//! Each rule is asserted over a table whose rows differ along what it generalises over: which
//! condition is wrong and which is right, where an unknown name sits, the shape of the name,
//! and which field an ARN leaves out.

use serde_json::{json, Value};
use service_com_amazonaws_athena_amazonathena::state::{
    capacity_reservation_not_found, prepared_statement_name_not_found, DEFAULT_WORK_GROUP,
    ERROR_CODE_INVALID_INPUT, ERROR_CODE_STATEMENT_NAME_NOT_FOUND, PYSPARK_ENGINE_VERSION,
    RESOURCE_ARN_NOT_COMPLETE_OR_INVALID, RESOURCE_ARN_NOT_FOUND, WORK_GROUPS_NOT_FOUND,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{assert_refusal, call, create_work_group, router};

/// The amount of provisioned capacity every reservation below asks for: a positive multiple
/// of the block size, so no row says anything about the requested amount by accident.
const SOME_TARGET_DPUS: i32 = 8;

/// The Region every request of this suite is signed for, which is the Region the harness's
/// SigV4 credential scope names.
const REGION: &str = "us-east-1";

/// The one canonical account id this emulator presents, which is the account every ARN below
/// names.
fn account() -> &'static str {
    ::engine::emulator_account_id()
}

// AWS's own wording for each refusal this suite drives, written out HERE as the wire carries
// it. The assertions below compare the wire against these literals and then pin the service's
// own constant or helper to them, so a rewording on either side fails a test instead of
// passing against itself.

/// The whole sentence a put naming a workgroup the account does not hold is refused with.
const WIRE_WORK_GROUPS_NOT_FOUND: &str = "One or more WorkGroups are not found.";

/// The whole sentence a `ResourceARN` that spells out no session ARN is refused with — with
/// no terminating period.
const WIRE_RESOURCE_ARN_NOT_COMPLETE_OR_INVALID: &str = "ResourceARN is not complete or invalid";

/// The whole sentence an ARN that names a session this account holds none of is refused with.
const WIRE_RESOURCE_ARN_NOT_FOUND: &str = "ResourceARN is not found";

/// The reason a batch read's unprocessed entry carries for `name`: the fixed text
/// `PreparedStatement ` and ` was not found` around the name the request submitted, with no
/// terminating period.
fn wire_prepared_statement_not_found(name: &str) -> String {
    format!("PreparedStatement {name} was not found")
}

// ---------------------------------------------------------------------------
// PutCapacityAssignmentConfiguration: the assignment is resolved before the reservation.
// ---------------------------------------------------------------------------

/// What a put is expected to answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PutOutcome {
    /// Accepted, and the configuration it carried is what a later read reports.
    Accepted,
    /// Refused because an assignment names a workgroup this account does not hold.
    WorkGroupsMissing,
    /// Refused because this account holds no reservation under the name the put addressed.
    ReservationMissing,
}

/// One row of the put table.
struct PutCase {
    /// The row's name, repeated in every assertion.
    case: &'static str,
    /// Whether the account holds the reservation the put addresses.
    reservation_held: bool,
    /// The assignments the put carries: one list of workgroup names per assignment, each
    /// name paired with whether the account holds that workgroup.
    assignments: Vec<Vec<(&'static str, bool)>>,
    /// What the put must answer.
    outcome: PutOutcome,
}

/// Create the capacity reservation `name`.
async fn create_reservation(router: &MultiServiceRouter, name: &str) {
    let (status, body) = call(
        router,
        "CreateCapacityReservation",
        &json!({ "Name": name, "TargetDpus": SOME_TARGET_DPUS }),
    )
    .await;
    assert_eq!(
        status, 200,
        "the reservation {name} must be created, got {status} with {body:?}"
    );
}

/// Put `assignments` as the capacity assignment configuration of `reservation`.
async fn put_assignments(
    router: &MultiServiceRouter,
    reservation: &str,
    assignments: &Value,
) -> (u16, Value) {
    call(
        router,
        "PutCapacityAssignmentConfiguration",
        &json!({
            "CapacityReservationName": reservation,
            "CapacityAssignments": assignments,
        }),
    )
    .await
}

/// RULE: the workgroups a put assigns are resolved before the reservation it is addressed to,
/// so the condition a client is told is the assignment's whenever one of its names does not
/// resolve.
#[tokio::test]
async fn a_put_reports_an_unresolvable_assignment_before_a_missing_reservation() {
    // The rows differ along which of the two conditions is wrong, how many names an
    // assignment carries, where the unknown name sits, and whether the configuration names
    // any workgroup at all.
    let rows = vec![
        PutCase {
            case: "neither the reservation nor the assigned workgroup exists",
            reservation_held: false,
            assignments: vec![vec![("absent-group", false)]],
            outcome: PutOutcome::WorkGroupsMissing,
        },
        PutCase {
            case: "the reservation is absent and one name of several is unknown",
            reservation_held: false,
            assignments: vec![vec![(DEFAULT_WORK_GROUP, true), ("absent-group", false)]],
            outcome: PutOutcome::WorkGroupsMissing,
        },
        PutCase {
            case: "the reservation is absent and the unknown name is in a later assignment",
            reservation_held: false,
            assignments: vec![
                vec![(DEFAULT_WORK_GROUP, true)],
                vec![("absent-group", false)],
            ],
            outcome: PutOutcome::WorkGroupsMissing,
        },
        PutCase {
            case: "the reservation is held and one assigned workgroup is unknown",
            reservation_held: true,
            assignments: vec![vec![("held-group", true), ("absent-group", false)]],
            outcome: PutOutcome::WorkGroupsMissing,
        },
        PutCase {
            case: "the reservation is absent and every assigned workgroup resolves",
            reservation_held: false,
            assignments: vec![vec![("held-group", true)]],
            outcome: PutOutcome::ReservationMissing,
        },
        PutCase {
            case: "the reservation is absent and the assignment names no workgroup at all",
            reservation_held: false,
            assignments: vec![vec![]],
            outcome: PutOutcome::ReservationMissing,
        },
        PutCase {
            case: "both the reservation and every assigned workgroup resolve",
            reservation_held: true,
            assignments: vec![vec![("held-group", true), (DEFAULT_WORK_GROUP, true)]],
            outcome: PutOutcome::Accepted,
        },
    ];

    for row in rows {
        let case = row.case;
        let router = router();
        let reservation = "assignment-order";
        if row.reservation_held {
            create_reservation(&router, reservation).await;
        }
        for (work_group, held) in row.assignments.iter().flatten() {
            // The account's default workgroup is held without any create of its own.
            if *held && *work_group != DEFAULT_WORK_GROUP {
                create_work_group(&router, work_group).await;
            }
        }
        let carried = Value::Array(
            row.assignments
                .iter()
                .map(|names| {
                    let names: Vec<&str> = names.iter().map(|(name, _)| *name).collect();
                    json!({ "WorkGroupNames": names })
                })
                .collect(),
        );

        let (status, body) = put_assignments(&router, reservation, &carried).await;
        match row.outcome {
            PutOutcome::Accepted => {
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected the put to be accepted, got {status} with {body:?}"
                );
                // Read back through the operation that projects the stored field, so the
                // accepted put is proved by what a later caller sees.
                let (status, read) = call(
                    &router,
                    "GetCapacityAssignmentConfiguration",
                    &json!({ "CapacityReservationName": reservation }),
                )
                .await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: the read must answer 200, got {status} with {read:?}"
                );
                let assigned: Vec<Vec<String>> = read["CapacityAssignmentConfiguration"]
                    ["CapacityAssignments"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default()
                    .iter()
                    .map(|assignment| {
                        assignment["WorkGroupNames"]
                            .as_array()
                            .cloned()
                            .unwrap_or_default()
                            .iter()
                            .filter_map(|name| name.as_str().map(str::to_string))
                            .collect()
                    })
                    .collect();
                let expected: Vec<Vec<String>> = row
                    .assignments
                    .iter()
                    .map(|names| {
                        names
                            .iter()
                            .map(|(name, _)| (*name).to_string())
                            .collect::<Vec<String>>()
                    })
                    .collect();
                assert_eq!(
                    assigned, expected,
                    "case {case:?}: expected the configuration {expected:?} to be stored, got \
                     {assigned:?}"
                );
            }
            PutOutcome::WorkGroupsMissing => {
                assert_refusal(
                    case,
                    status,
                    &body,
                    ERROR_CODE_INVALID_INPUT,
                    WIRE_WORK_GROUPS_NOT_FOUND,
                );
                assert_eq!(
                    WORK_GROUPS_NOT_FOUND, WIRE_WORK_GROUPS_NOT_FOUND,
                    "case {case:?}: the service's sentence for this condition must stay \
                     AWS's, got {WORK_GROUPS_NOT_FOUND:?}"
                );
            }
            PutOutcome::ReservationMissing => assert_refusal(
                case,
                status,
                &body,
                ERROR_CODE_INVALID_INPUT,
                &capacity_reservation_not_found(reservation),
            ),
        }
    }
}

// ---------------------------------------------------------------------------
// GetResourceDashboard: the ARN's form before the session lookup.
// ---------------------------------------------------------------------------

/// The execution role a Spark workgroup accesses user resources through, inside the modelled
/// `RoleArn` pattern.
const EXECUTION_ROLE: &str = "arn:aws:iam::000000000000:role/AthenaSparkExecution";

/// Where the Spark workgroup below writes its results.
const OUTPUT_LOCATION: &str = "s3://athena-spark-results/sessions/";

/// Create a Spark enabled workgroup, which is the workgroup Athena documents sessions run in.
async fn given_spark_work_group(router: &MultiServiceRouter, name: &str) {
    let (status, created) = call(
        router,
        "CreateWorkGroup",
        &json!({
            "Name": name,
            "Configuration": {
                "EngineVersion": { "SelectedEngineVersion": PYSPARK_ENGINE_VERSION },
                "ExecutionRole": EXECUTION_ROLE,
                "ResultConfiguration": { "OutputLocation": OUTPUT_LOCATION }
            }
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "the Spark workgroup {name} must be created, got {status} with {created:?}"
    );
}

/// Start one session in `work_group` and hand back its `SessionId`.
async fn given_session(router: &MultiServiceRouter, work_group: &str) -> String {
    let (status, started) = call(
        router,
        "StartSession",
        &json!({
            "WorkGroup": work_group,
            "EngineConfiguration": {
                "CoordinatorDpuSize": 1,
                "MaxConcurrentDpus": 4,
                "DefaultExecutorDpuSize": 1
            }
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "StartSession in {work_group} must be accepted, got {status} with {started:?}"
    );
    started["SessionId"]
        .as_str()
        .unwrap_or_else(|| panic!("StartSession must answer with a SessionId, got {started:?}"))
        .to_string()
}

/// RULE: a `ResourceARN` out of which no session reference can be read is refused for its
/// FORM, before anything is resolved, and told apart from one that names a session this
/// account holds none of.
#[tokio::test]
async fn an_incomplete_resource_arn_is_refused_for_its_form_not_as_a_missing_resource() {
    let router = router();
    const SPARK_WORK_GROUP: &str = "spark-arn-forms";
    given_spark_work_group(&router, SPARK_WORK_GROUP).await;
    // A real session exists throughout, so each refusal below is about the value the request
    // carried rather than about an account holding no sessions at all.
    let session_id = given_session(&router, SPARK_WORK_GROUP).await;
    let account = account();
    let held = format!("arn:aws:athena:{REGION}:{account}:session/{session_id}");

    // Rows differ along WHICH part of the form is missing: the separators, the leading
    // literal, each field the form requires to be filled in, and the resource field's own
    // segments — the type that says a session is named, the id, and nothing past it. Every
    // value is inside the modelled `AmazonResourceName` (1 to 1011 characters), so the
    // request reaches the handler rather than being refused at the boundary.
    let incomplete = vec![
        ("no ARN at all", "not-an-arn".to_string()),
        ("the leading literal alone", "arn".to_string()),
        ("the partition field only", "arn:aws".to_string()),
        ("the vendor field only", "arn:aws:athena".to_string()),
        (
            "no account and no resource field",
            format!("arn:aws:athena:{REGION}"),
        ),
        (
            "no resource field",
            format!("arn:aws:athena:{REGION}:{account}"),
        ),
        (
            "a leading literal that is not arn",
            format!("urn:aws:athena:{REGION}:{account}:session/{session_id}"),
        ),
        (
            "an empty partition field",
            format!("arn::athena:{REGION}:{account}:session/{session_id}"),
        ),
        (
            "an empty vendor field",
            format!("arn:aws::{REGION}:{account}:session/{session_id}"),
        ),
        (
            "an empty Region field",
            format!("arn:aws:athena::{account}:session/{session_id}"),
        ),
        (
            "an empty account field",
            format!("arn:aws:athena:{REGION}::session/{session_id}"),
        ),
        (
            "an empty resource field",
            format!("arn:aws:athena:{REGION}:{account}:"),
        ),
        ("the session id alone", session_id.clone()),
        (
            "the resource type alone, with no session id",
            format!("arn:aws:athena:{REGION}:{account}:session"),
        ),
        (
            "an empty session id",
            format!("arn:aws:athena:{REGION}:{account}:session/"),
        ),
        (
            "a resource type that is not a session",
            format!("arn:aws:athena:{REGION}:{account}:datacatalog/{session_id}"),
        ),
        (
            "a workgroup, which is not a session",
            format!("arn:aws:athena:{REGION}:{account}:workgroup/{SPARK_WORK_GROUP}"),
        ),
        (
            "a segment past the session id",
            format!("arn:aws:athena:{REGION}:{account}:session/{session_id}/live"),
        ),
        (
            "an empty workgroup segment before the session",
            format!("arn:aws:athena:{REGION}:{account}:workgroup//session/{session_id}"),
        ),
    ];

    for (case, resource_arn) in incomplete {
        let (status, body) = call(
            &router,
            "GetResourceDashboard",
            &json!({ "ResourceARN": resource_arn }),
        )
        .await;
        assert_refusal(
            case,
            status,
            &body,
            ERROR_CODE_INVALID_INPUT,
            WIRE_RESOURCE_ARN_NOT_COMPLETE_OR_INVALID,
        );
    }
    assert_eq!(
        RESOURCE_ARN_NOT_COMPLETE_OR_INVALID, WIRE_RESOURCE_ARN_NOT_COMPLETE_OR_INVALID,
        "the service's sentence for a value that spells out no session ARN must stay AWS's, \
         got {RESOURCE_ARN_NOT_COMPLETE_OR_INVALID:?}"
    );

    // A value that DOES spell out the ARN of a session and still resolves none is the other
    // condition, and reads the other way: the rows differ along what makes it resolve nothing.
    let unresolvable = vec![
        (
            "another vendor's session ARN",
            format!("arn:aws:glue:{REGION}:{account}:session/{session_id}"),
        ),
        (
            "another account's session ARN",
            format!("arn:aws:athena:{REGION}:999999999999:session/{session_id}"),
        ),
        (
            "another Region's session ARN",
            format!("arn:aws:athena:eu-west-1:{account}:session/{session_id}"),
        ),
        (
            "a session id nothing was started under",
            format!("arn:aws:athena:{REGION}:{account}:session/{session_id}-no"),
        ),
        (
            "a session scoped to a workgroup it does not belong to",
            format!(
                "arn:aws:athena:{REGION}:{account}:workgroup/absent/session/{session_id}"
            ),
        ),
    ];

    for (case, resource_arn) in unresolvable {
        let (status, body) = call(
            &router,
            "GetResourceDashboard",
            &json!({ "ResourceARN": resource_arn }),
        )
        .await;
        assert_refusal(
            case,
            status,
            &body,
            ERROR_CODE_INVALID_INPUT,
            WIRE_RESOURCE_ARN_NOT_FOUND,
        );
    }
    assert_eq!(
        RESOURCE_ARN_NOT_FOUND, WIRE_RESOURCE_ARN_NOT_FOUND,
        "the service's sentence for an ARN that resolves no session must stay AWS's, got \
         {RESOURCE_ARN_NOT_FOUND:?}"
    );

    // And the ARN that IS complete and DOES resolve still answers, so the check above refuses
    // the form rather than the operation.
    let (status, answered) = call(
        &router,
        "GetResourceDashboard",
        &json!({ "ResourceARN": held }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"the session's own ARN\": expected the dashboard, got {status} with {answered:?}"
    );
    assert!(
        answered["Url"]
            .as_str()
            .is_some_and(|url| url.contains(session_id.as_str())),
        "case \"the session's own ARN\": expected this session's interface, got {answered:?}"
    );
}

// ---------------------------------------------------------------------------
// BatchGetPreparedStatement: the per-item class and the reason naming the statement.
// ---------------------------------------------------------------------------

/// A `StatementName` of `length` characters, inside both the modelled pattern
/// (`^[a-zA-Z_][a-zA-Z0-9_@:]{1,256}$`, which admits 2 to 257 characters) and the modelled
/// length (`StatementName`, 1 to 256), so the longest name the model admits is 256: a leading
/// letter followed by characters of the pattern's own set.
///
/// `length` is therefore at least 2; anything shorter satisfies neither.
fn statement_name_of_length(length: usize) -> String {
    assert!(
        length >= 2,
        "a StatementName is at least 2 characters, asked for {length}"
    );
    let mut name = String::from("s");
    name.extend("tatement_name_0@:".chars().cycle().take(length - 1));
    name
}

/// Create the prepared statement `name` in `work_group`.
async fn create_statement(router: &MultiServiceRouter, work_group: &str, name: &str) {
    let (status, created) = call(
        router,
        "CreatePreparedStatement",
        &json!({
            "WorkGroup": work_group,
            "StatementName": name,
            "QueryStatement": "SELECT 1",
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "the prepared statement {name} must be created, got {status} with {created:?}"
    );
}

/// RULE: a batch read reports a name the workgroup holds no statement under as an unprocessed
/// name carrying the class of that miss and the reason that NAMES the statement asked for.
#[tokio::test]
async fn a_batch_reads_unprocessed_entry_names_the_statement_it_could_not_return() {
    let router = router();
    const WORK_GROUP: &str = "batch-entry-reasons";
    const OTHER_WORK_GROUP: &str = "batch-entry-reasons-other";
    create_work_group(&router, WORK_GROUP).await;
    create_work_group(&router, OTHER_WORK_GROUP).await;
    const HELD: &str = "held_statement";
    create_statement(&router, WORK_GROUP, HELD).await;
    // One statement of the SAME name in another workgroup, so a row can ask for a name this
    // workgroup holds nothing under while the account holds a statement of that name.
    const ELSEWHERE: &str = "statement_elsewhere";
    create_statement(&router, OTHER_WORK_GROUP, ELSEWHERE).await;

    let shortest = statement_name_of_length(2);
    let longest = statement_name_of_length(256);
    // The rows differ along the shape of the name the workgroup holds nothing under, where it
    // sits among names it does hold, and how many of them there are.
    let rows: Vec<(&str, Vec<String>, Vec<String>)> = vec![
        (
            "one name the workgroup holds nothing under",
            vec!["absent_statement".to_string()],
            vec!["absent_statement".to_string()],
        ),
        (
            "a name differing from a held one by its last character",
            vec![format!("{HELD}_")],
            vec![format!("{HELD}_")],
        ),
        (
            "the shortest name the model admits",
            vec![shortest.clone()],
            vec![shortest.clone()],
        ),
        (
            "the longest name the model admits",
            vec![longest.clone()],
            vec![longest.clone()],
        ),
        (
            "a name another workgroup holds a statement under",
            vec![ELSEWHERE.to_string()],
            vec![ELSEWHERE.to_string()],
        ),
        (
            "two unheld names around a held one",
            vec![
                "absent_first".to_string(),
                HELD.to_string(),
                "absent_second".to_string(),
            ],
            vec!["absent_first".to_string(), "absent_second".to_string()],
        ),
        (
            "every name held",
            vec![HELD.to_string()],
            vec![],
        ),
    ];

    for (case, asked, expected_unprocessed) in rows {
        let (status, body) = call(
            &router,
            "BatchGetPreparedStatement",
            &json!({ "WorkGroup": WORK_GROUP, "PreparedStatementNames": asked }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: a batch read answers 200 whatever it could not resolve, got \
             {status} with {body:?}"
        );
        let unprocessed = body["UnprocessedPreparedStatementNames"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        let reported: Vec<String> = unprocessed
            .iter()
            .map(|entry| {
                entry["StatementName"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string()
            })
            .collect();
        assert_eq!(
            reported, expected_unprocessed,
            "case {case:?}: expected the unheld names {expected_unprocessed:?} reported in \
             request order, got {unprocessed:?}"
        );
        for entry in &unprocessed {
            let name = entry["StatementName"].as_str().unwrap_or_default();
            assert_eq!(
                entry["ErrorCode"].as_str(),
                Some(ERROR_CODE_STATEMENT_NAME_NOT_FOUND),
                "case {case:?}: expected the class {ERROR_CODE_STATEMENT_NAME_NOT_FOUND:?}, \
                 got {entry:?}"
            );
            assert_eq!(
                entry["ErrorMessage"].as_str(),
                Some(wire_prepared_statement_not_found(name).as_str()),
                "case {case:?}: expected the reason naming {name:?}, got {entry:?}"
            );
            assert_eq!(
                prepared_statement_name_not_found(name),
                wire_prepared_statement_not_found(name),
                "case {case:?}: the service's reason for this condition must stay AWS's, got \
                 {:?}",
                prepared_statement_name_not_found(name)
            );
        }
    }
}
