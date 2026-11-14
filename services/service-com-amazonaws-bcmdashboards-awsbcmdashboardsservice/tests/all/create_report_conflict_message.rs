// SPDX-License-Identifier: CC0-1.0

//! The WORDING of the `ConflictException` a `CreateScheduledReport` `clientToken`
//! conflict is refused with, end to end over the wire.
//!
//! The rule under test: the refusal a duplicate idempotency token is answered with is
//! worded from the state of the create that claimed the token, and that wording is fixed
//! text a client matches on — the creation-in-progress branch, which a call overlapping a
//! create that has not stored its report yet gets, carries NO variable part at all, and
//! the already-created branch, which every call presenting a token whose create has stored
//! its report gets, carries one variable part: the ARN of that report. The caller's token
//! is a value the client chose as its idempotency key and never appears in either message,
//! so neither branch's text varies with the request.
//!
//! The table spans what the rule generalises over: the token itself. Its rows walk
//! `ClientToken`'s constraints (length 1..128, pattern `^[\u0021-\u007E]+$`, source:
//! `smithy_query(query="shape",shape="ClientToken")`) at both published length bounds,
//! over the punctuation the pattern admits, over a token shaped like an ARN (so a
//! message echoing the token instead of the stored ARN is caught) and over a token
//! spelled like the fixed wording itself (so a message interpolating the token is caught
//! even when it reads plausibly). Every row is asserted against the two texts
//! character for character.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_bcmdashboards_awsbcmdashboardsservice::state::service_state;

use super::dashboard_lifecycle::{call, emulator, error_code, ok_call};
use super::scheduled_report_lifecycle::{source_dashboard, EXECUTION_ROLE_ARN};

/// The whole message a call overlapping a create that has not stored its report yet is
/// refused with. Fixed text: there is no variable part, and the "retry after some time"
/// instruction is what tells a client this conflict is the transient one.
pub(crate) const IN_PROGRESS_MESSAGE: &str = "A scheduled report creation is already in progress \
     for the provided clientToken. Please retry after some time.";

/// The message every call presenting a token whose create has stored its report is refused
/// with: fixed text around the one value a caller can act on, the ARN of the report the
/// token already created.
pub(crate) fn already_exists_message(arn: &str) -> String {
    format!(
        "A scheduled report with the provided clientToken already exists (ARN: {arn}). \
         Use a different clientToken to create a new scheduled report, or use the \
         existing report's ARN to update it."
    )
}

/// One row: a `clientToken` a client may legitimately choose.
struct TokenCase {
    label: &'static str,
    token: String,
}

fn cases() -> Vec<TokenCase> {
    vec![
        TokenCase {
            label: "the token an SDK generates",
            token: "sdk-generated-token-7b31".to_string(),
        },
        TokenCase {
            // `ClientToken`'s shortest admitted value, and `!` is the first code point
            // its pattern admits (\u0021).
            label: "one character, the shortest admitted token",
            token: "!".to_string(),
        },
        TokenCase {
            // The published upper length bound, 128.
            label: "128 characters, the longest admitted token",
            token: "t".repeat(128),
        },
        TokenCase {
            // The printable range the pattern admits, excluding nothing but the space
            // it stops short of.
            label: "punctuation across the admitted printable range",
            token: "~!#$%&'()*+,-./:;<=>?@[\\]^_`{|}\"".to_string(),
        },
        TokenCase {
            label: "a token shaped like a scheduled-report ARN",
            token: "arn:aws:bcm-dashboards::123456789012:scheduled-report/not-a-report".to_string(),
        },
        TokenCase {
            label: "a token spelled like the refusal's own wording",
            token: "the-provided-clientToken".to_string(),
        },
    ]
}

/// The create request every row presents, under its own token.
fn create_request(token: &str, dashboard_arn: &str, widget_id: &str, name: &str) -> Value {
    json!({
        "clientToken": token,
        "scheduledReport": {
            "name": name,
            "dashboardArn": dashboard_arn,
            "scheduledReportExecutionRoleArn": EXECUTION_ROLE_ARN,
            "scheduleConfig": {"scheduleExpression": "cron(0 9 1 * ? *)"},
            "widgetIds": [widget_id]
        }
    })
}

/// The conflict body of a refused create: asserts the code and status, and hands back the
/// message.
fn conflict_message(case: &str, branch: &str, status: u16, body: &Value) -> String {
    assert_eq!(
        status, 409,
        "case {case:?} ({branch}): a duplicate clientToken is refused with 409, got {status}: {body}"
    );
    assert!(
        error_code(body).ends_with("ConflictException"),
        "case {case:?} ({branch}): expected ConflictException, got {body}"
    );
    body["message"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?} ({branch}): the refusal carries a message: {body}"))
        .to_string()
}

/// A dashboard to report on, on a fresh store of its own.
async fn source(name: &str) -> (MultiServiceRouter, String, String) {
    let (router, _engine) = emulator();
    let (dashboard_arn, widget_id) = source_dashboard(&router, name).await;
    (router, dashboard_arn, widget_id)
}

/// The already-created branch, over the table: the refusal every call presenting a token
/// whose create stored its report gets is the fixed text carrying only the ARN that create
/// minted — not the token, however the token is spelled, and whether the request repeats
/// that create or differs from it.
#[tokio::test]
async fn a_reused_client_token_is_refused_in_aws_s_own_words() {
    for case in cases() {
        let (router, dashboard_arn, widget_id) = source("ConflictMessageSource").await;
        let request = create_request(&case.token, &dashboard_arn, &widget_id, "Conflict Rollup");
        let created = ok_call(&router, "CreateScheduledReport", &request).await;
        let arn = created["arn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {:?}: the create returns an arn: {created}", case.label))
            .to_string();

        let different = create_request(
            &case.token,
            &dashboard_arn,
            &widget_id,
            "Another Conflict Rollup",
        );
        let expected = already_exists_message(&arn);
        for (branch, presented) in [
            ("the identical request", &request),
            ("a different request", &different),
        ] {
            let (status, body) = call(&router, "CreateScheduledReport", presented).await;
            let message = conflict_message(case.label, branch, status, &body);
            assert_eq!(
                message, expected,
                "case {:?} ({branch}): expected {expected:?}, got {message:?}",
                case.label
            );
            assert!(
                !message.contains(&case.token),
                "case {:?} ({branch}): the presented clientToken must not appear in the refusal, got {message:?}",
                case.label
            );
        }
    }
}

/// The creation-in-progress branch, over the same table: a call that overlaps a create
/// which has claimed the token and not stored its report yet is refused with text that
/// has no variable part at all, so every row's refusal is the same bytes.
#[tokio::test]
async fn the_in_progress_refusal_is_the_same_fixed_text_for_every_token() {
    let mut messages: Vec<(String, String)> = Vec::new();
    for case in cases() {
        let (router, engine) = emulator();
        let (dashboard_arn, widget_id) = source_dashboard(&router, "InProgressMessageSource").await;

        // A create has claimed this row's token and has not stored its report yet, which
        // is the state the call below overlaps.
        {
            let state = service_state(&*engine).expect("the dashboard store");
            let claimed = state
                .lock()
                .reserve_scheduled_report_creation(case.token.clone());
            assert!(
                claimed.is_none(),
                "case {:?} (overlapping the create): the token must be free before the claim",
                case.label
            );
        }

        let request = create_request(&case.token, &dashboard_arn, &widget_id, "Conflict Rollup");
        let (status, body) = call(&router, "CreateScheduledReport", &request).await;
        let message = conflict_message(case.label, "overlapping the create", status, &body);
        assert_eq!(
            message, IN_PROGRESS_MESSAGE,
            "case {:?} (overlapping the create): expected the fixed in-progress text {IN_PROGRESS_MESSAGE:?}, got {message:?}",
            case.label
        );
        assert!(
            !message.contains(&case.token),
            "case {:?} (overlapping the create): the presented clientToken must not appear in the refusal, got {message:?}",
            case.label
        );
        messages.push((case.label.to_string(), message));
    }

    // Fixed text means fixed: every row's in-progress refusal is the same bytes.
    let (first_label, first) = messages[0].clone();
    for (label, message) in &messages {
        assert_eq!(
            message, &first,
            "case {label:?} (overlapping the create): the in-progress message has no variable part, so every row's must match case {first_label:?}'s {first:?}, got {message:?}"
        );
    }
}
