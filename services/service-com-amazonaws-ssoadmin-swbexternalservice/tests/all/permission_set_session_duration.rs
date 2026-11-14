// SPDX-License-Identifier: CC0-1.0

//! The WINDOW a permission set's `SessionDuration` has to fall in, end to end over the
//! `awsJson1_1` wire.
//!
//! THE RULE: a permission set's session duration is a span between ONE HOUR and TWELVE
//! HOURS (`state::MIN_SESSION_DURATION`, `state::MAX_SESSION_DURATION`, the range IAM
//! Identity Center publishes for a permission set), both ends inclusive — so a write naming
//! a span outside that window, or a value naming no fixed span at all (a negative duration,
//! or a calendar designator whose length depends on the calendar), is refused with the
//! modelled `ValidationException` (HTTP 400,
//! `smithy_query(query="operation",operation="CreatePermissionSet,UpdatePermissionSet")`
//! lists it on both) and changes nothing: the create mints no permission set and the update
//! leaves the stored record exactly as it was. A value INSIDE the window is stored and read
//! back verbatim, whichever of the ISO-8601 forms the model's `Duration` pattern admits
//! names it.
//!
//! The model cannot express the window — `Duration`
//! (`smithy_query(query="shape",shape="Duration")`) is `length 1..100` plus the FORM pattern
//! `^(-?)P(?=\d|T\d)(?:(\d+)Y)?(?:(\d+)M)?(?:(\d+)([DW]))?(?:T(?:(\d+)H)?(?:(\d+)M)?(?:(\d+(?:\.\d+)?)S)?)?$`,
//! with no range at all — so every row below passes the boundary validator and reaches the
//! handler.
//!
//! The rows span what the rule generalises over: the UNIT the span is named in (hours,
//! minutes, seconds, a fractional second, a zero day component, combinations), each
//! published bound EXACTLY and one second past it, spans far outside on both sides, the
//! calendar and week designators the pattern admits, and the signed form. The two bounds and
//! the rows one second outside them are built FROM the window constants, so they cannot
//! drift from the window the service enforces. The recorded case (`PT24H`) is one row.
//!
//! Every call goes through `MultiServiceRouter::dispatch`, the one path a socket-served
//! request takes; the harness is the one `ssoadmin_permission_sets.rs` owns.
//! `permission_set_update.rs` owns which members an update writes, and
//! `ssoadmin_permission_sets.rs` the create/read round trip itself.

use serde_json::{json, Value};
use service_com_amazonaws_ssoadmin_swbexternalservice::state;
use smithy_server_runtime::MultiServiceRouter;

use crate::ssoadmin_permission_sets::{assert_refused, call, organization_router};

/// One row: a `SessionDuration` and whether the window admits the span it names.
struct DurationCase {
    label: String,
    value: String,
    served: bool,
}

/// The span `seconds` long, named in seconds — the form every bound row is built in, so a
/// row sits exactly where the window constant puts it.
fn seconds_duration(seconds: u64) -> String {
    format!("PT{seconds}S")
}

/// The window's two bounds, in seconds.
fn window_bounds() -> (u64, u64) {
    (
        state::MIN_SESSION_DURATION.as_secs(),
        state::MAX_SESSION_DURATION.as_secs(),
    )
}

/// The rows the rule is asserted over.
fn duration_cases() -> Vec<DurationCase> {
    let (minimum, maximum) = window_bounds();
    let mut cases: Vec<DurationCase> = vec![
        // The window's two bounds, named in the units a caller writes them in.
        ("the lower bound in hours", "PT1H", true),
        ("the upper bound in hours", "PT12H", true),
        ("the lower bound in minutes", "PT60M", true),
        ("the upper bound in minutes", "PT720M", true),
        // Inside the window, across the forms the pattern admits.
        ("hours and minutes", "PT4H30M", true),
        ("every time component", "PT2H15M30S", true),
        ("a fractional second", "PT1H0.5S", true),
        ("a zero day component before the hours", "P0DT6H", true),
        ("minutes and seconds adding up past the hour", "PT59M60S", true),
        ("a zero-padded digit run", "PT003600S", true),
        // Outside the window: too long.
        ("a full day in hours, the reported case", "PT24H", false),
        ("a bare day", "P1D", false),
        ("a week, which the pattern's W designator admits", "P1W", false),
        ("one hour past the upper bound", "PT13H", false),
        ("a hundred hours", "PT100H", false),
        ("a calendar month", "P1M", false),
        ("a calendar year", "P1Y", false),
        ("every designator at once", "P1Y2M3DT4H5M6S", false),
        // Outside the window: too short, zero, or negative.
        ("half the lower bound", "PT30M", false),
        ("a zero-length span", "PT0S", false),
        ("a zero-length span in days", "P0D", false),
        ("the signed form the pattern admits", "-PT1H", false),
        ("a negative span inside the window's magnitude", "-PT6H", false),
    ]
    .into_iter()
    .map(|(label, value, served)| DurationCase {
        label: label.to_string(),
        value: value.to_string(),
        served,
    })
    .collect();

    // Each published bound exactly, and one second past it, built from the window itself.
    cases.extend([
        DurationCase {
            label: "the lower bound exactly, in seconds".to_string(),
            value: seconds_duration(minimum),
            served: true,
        },
        DurationCase {
            label: "the upper bound exactly, in seconds".to_string(),
            value: seconds_duration(maximum),
            served: true,
        },
        DurationCase {
            label: "one second below the lower bound".to_string(),
            value: seconds_duration(minimum - 1),
            served: false,
        },
        DurationCase {
            label: "one second above the upper bound".to_string(),
            value: seconds_duration(maximum + 1),
            served: false,
        },
    ]);
    cases
}

/// The `Message` an error body carries, whichever member the renderer put it in.
fn message_of(body: &Value) -> String {
    body["Message"]
        .as_str()
        .or_else(|| body["message"].as_str())
        .unwrap_or_default()
        .to_string()
}

/// Assert `body` is the window refusal, naming the value the request sent and the window it
/// was judged against.
fn assert_window_refusal(case: &str, what: &str, value: &str, status: u16, body: &Value) {
    assert_refused(case, what, "ValidationException", status, body);
    let message = message_of(body);
    let (minimum, maximum) = window_bounds();
    assert!(
        message.contains(value),
        "case {case:?}: {what} must name the duration it refused, got {message:?}"
    );
    assert!(
        message.contains(&format!("{} hour", minimum / 3_600)),
        "case {case:?}: {what} must state the window's lower bound, got {message:?}"
    );
    assert!(
        message.contains(&format!("{} hours", maximum / 3_600)),
        "case {case:?}: {what} must state the window's upper bound, got {message:?}"
    );
    assert_eq!(
        message,
        state::session_duration_window_message(value),
        "case {case:?}: {what} must refuse through the one shared wording, got {message:?}"
    );
}

/// The `PermissionSet` `DescribePermissionSet` reports for `permission_set_arn`.
async fn describe(
    router: &MultiServiceRouter,
    case: &str,
    instance_arn: &str,
    permission_set_arn: &str,
) -> Value {
    let (status, body) = call(
        router,
        "DescribePermissionSet",
        &json!({"InstanceArn": instance_arn, "PermissionSetArn": permission_set_arn}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribePermissionSet expected 200, got {status} {body}"
    );
    body["PermissionSet"].clone()
}

/// How many permission sets the instance holds, read the way a client reads it.
async fn permission_set_count(router: &MultiServiceRouter, case: &str, instance_arn: &str) -> usize {
    let (status, body) = call(
        router,
        "ListPermissionSets",
        &json!({"InstanceArn": instance_arn}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListPermissionSets expected 200, got {status} {body}"
    );
    body["PermissionSets"].as_array().map(Vec::len).unwrap_or(0)
}

/// THE RULE on `CreatePermissionSet`: a span inside the window is stored and read back, one
/// outside it is refused and mints nothing.
#[tokio::test]
async fn create_stores_only_a_session_duration_inside_the_window() {
    let (router, instance_arn) = organization_router();

    for (index, row) in duration_cases().into_iter().enumerate() {
        let case = format!("{} ({})", row.label, row.value);
        let name = format!("dur{index}");
        let before = permission_set_count(&router, &case, &instance_arn).await;
        let (status, answer) = call(
            &router,
            "CreatePermissionSet",
            &json!({
                "InstanceArn": instance_arn,
                "Name": name,
                "SessionDuration": row.value,
            }),
        )
        .await;

        if row.served {
            assert_eq!(
                status, 200,
                "case {case:?}: CreatePermissionSet expected 200, got {status} {answer}"
            );
            assert_eq!(
                answer["PermissionSet"]["SessionDuration"].as_str(),
                Some(row.value.as_str()),
                "case {case:?}: the create must answer with the duration it stored, got {answer}"
            );
            // The ROUND TRIP: the stored record is what the reader answers from.
            let arn = answer["PermissionSet"]["PermissionSetArn"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: the create minted no ARN: {answer}"))
                .to_string();
            let described = describe(&router, &case, &instance_arn, &arn).await;
            assert_eq!(
                described["SessionDuration"].as_str(),
                Some(row.value.as_str()),
                "case {case:?}: the describe must report the stored duration, got {described}"
            );
            assert_eq!(
                permission_set_count(&router, &case, &instance_arn).await,
                before + 1,
                "case {case:?}: a served create must mint exactly one permission set"
            );
        } else {
            assert_window_refusal(&case, "CreatePermissionSet", &row.value, status, &answer);
            assert_eq!(
                permission_set_count(&router, &case, &instance_arn).await,
                before,
                "case {case:?}: a refused create must mint no permission set"
            );
        }
    }
}

/// THE SAME RULE on `UpdatePermissionSet`, the other write that takes a `SessionDuration`: a
/// span inside the window is written and read back, one outside it is refused and leaves the
/// stored duration alone.
#[tokio::test]
async fn update_writes_only_a_session_duration_inside_the_window() {
    let (router, instance_arn) = organization_router();
    // The duration the record starts from, so a refused update has something to be shown to
    // have left alone.
    let stored_at_create = "PT2H";
    let (status, created) = call(
        &router,
        "CreatePermissionSet",
        &json!({
            "InstanceArn": instance_arn,
            "Name": "UpdatedSet",
            "SessionDuration": stored_at_create,
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "the seeding CreatePermissionSet expected 200, got {status} {created}"
    );
    let permission_set_arn = created["PermissionSet"]["PermissionSetArn"]
        .as_str()
        .unwrap_or_else(|| panic!("the create minted no ARN: {created}"))
        .to_string();
    let mut last_stored = stored_at_create.to_string();

    for row in duration_cases() {
        let case = format!("{} ({})", row.label, row.value);
        let (status, answer) = call(
            &router,
            "UpdatePermissionSet",
            &json!({
                "InstanceArn": instance_arn,
                "PermissionSetArn": permission_set_arn,
                "SessionDuration": row.value,
            }),
        )
        .await;

        if row.served {
            assert_eq!(
                status, 200,
                "case {case:?}: UpdatePermissionSet expected 200, got {status} {answer}"
            );
            last_stored = row.value.clone();
        } else {
            assert_window_refusal(&case, "UpdatePermissionSet", &row.value, status, &answer);
        }

        let described = describe(&router, &case, &instance_arn, &permission_set_arn).await;
        assert_eq!(
            described["SessionDuration"].as_str(),
            Some(last_stored.as_str()),
            "case {case:?}: the stored duration must be {last_stored:?}, got {described}"
        );
    }
}
