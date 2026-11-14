// SPDX-License-Identifier: CC0-1.0

//! `GenerateCredentialReport`, end to end over the awsQuery wire.
//!
//! Every test drives a `MultiServiceRouter` holding this service over one shared
//! `InMemoryEngine`, exactly as the production router does, so what is asserted is the
//! HTTP behaviour (status, awsQuery error code, XML members) a client on the socket sees.
//! The awsQuery harness is the one `tests/all/user_lifecycle.rs` owns and the other
//! suites in this crate share.
//!
//! The rules being asserted:
//!
//! * the action is ROUTED — a bound operation of this service answers it rather than the
//!   dispatcher's `InvalidAction`;
//! * what a call reports is the state of the account's ONE credential-report generation,
//!   read off that record and the clock: a call with none on record starts one
//!   (`STARTED`), a call while it runs reports `INPROGRESS`, the call that first finds it
//!   due reports `COMPLETE`, a request for another report inside the published
//!   once-every-four-hours rate generates none and reports that same `COMPLETE`, and from
//!   the end of that window a new generation starts;
//! * a caller whose identity-based policies do not allow the action is refused, and a
//!   refused call starts nothing.
//!
//! Each suite is a TABLE whose rows differ along what those rules generalise over: where
//! the clock stands relative to the generation's due instant and to the published rate —
//! including the instant on each side of both — which identity drives the call, and what
//! that caller is permitted.

use aws_smithy_types::DateTime;
use service_com_amazonaws_iam_awsidentitymanagementv20100508::ops::types::REPORT_STATE_TYPE_VALUES;
use service_com_amazonaws_iam_awsidentitymanagementv20100508::state;
use smithy_server_runtime::MultiServiceRouter;

use crate::identity_policy_enforcement::{principal, statement};
use crate::user_lifecycle::{
    account_id, assert_access_denied_message, call_signed_by, element, elements, error_code,
    router_with_engine,
};

/// The three values `ReportStateType` declares, named here so a row reads as the state it
/// is about. Source: `ops::types::REPORT_STATE_TYPE_VALUES`, the model's own value set.
const STARTED: &str = "STARTED";
const INPROGRESS: &str = "INPROGRESS";
const COMPLETE: &str = "COMPLETE";

/// One `GenerateCredentialReport` call, presented as signed by `akid` when a row names
/// one. The operation takes no input, so the call carries no form parameter beyond the
/// action.
///
/// `pub(crate)` because the suite about READING the report
/// (`tests/all/credential_report_reads.rs`) starts its generations through this same call.
pub(crate) async fn generate(router: &MultiServiceRouter, akid: Option<&str>) -> (u16, String) {
    call_signed_by(router, akid, "GenerateCredentialReport", &[]).await
}

/// Move the account's credential-report generation `seconds` further into the PAST, so a
/// row can stand the clock where it needs it without sleeping: the record holds the
/// instant the generation was asked for and the instant it is due, and every state a call
/// reports is read off those two.
///
/// `pub(crate)` because the suite about READING the report stands the clock the same way:
/// one rewind, so the two suites cannot disagree about where the clock is.
pub(crate) fn rewind(engine: &engine::InMemoryEngine, seconds: i64) {
    let store = state::service_state(engine).expect("the IAM store");
    let mut held = store.lock();
    let generation = held
        .credential_report
        .as_mut()
        .expect("a credential report generation to rewind");
    generation.requested = DateTime::from_secs(generation.requested.secs() - seconds);
    generation.completes = DateTime::from_secs(generation.completes.secs() - seconds);
}

/// Whether the account holds a credential-report generation at all: what a refused call
/// must leave untouched.
fn holds_a_generation(engine: &engine::InMemoryEngine) -> bool {
    let store = state::service_state(engine).expect("the IAM store");
    let held = store.lock().credential_report.is_some();
    held
}

/// Assert the call answered 200 with `state` and a non-empty description, and that the
/// state is one the model declares.
fn assert_state(case: &str, status: u16, body: &str, expected: &str) {
    assert_eq!(
        status, 200,
        "case {case:?}: expected the generation to be served, got {status}: {body}"
    );
    let reported = element(body, "State");
    assert_eq!(
        reported, expected,
        "case {case:?}: expected the state {expected}, got {reported:?}: {body}"
    );
    assert!(
        REPORT_STATE_TYPE_VALUES.contains(&reported.as_str()),
        "case {case:?}: expected a state ReportStateType declares, got {reported:?}"
    );
    assert!(
        !element(body, "Description").is_empty(),
        "case {case:?}: expected the response to describe the report, got {body}"
    );
}

/// RULE: the action is a bound operation of this service, so a call reaches a handler and
/// is answered with the operation's own response rather than with the dispatcher's
/// `InvalidAction` for an action the service does not have.
#[tokio::test]
async fn the_action_is_routed_rather_than_answered_as_an_unknown_action() {
    let (router, _engine) = router_with_engine();
    let (status, body) = generate(&router, None).await;
    assert!(
        !body.contains("<Code>"),
        "expected the action to be routed rather than answered with an error document, \
         got {body}"
    );
    assert_state("the first call of an account", status, &body, STARTED);
    assert_eq!(
        element(&body, "Description"),
        "No report exists. Starting a new report generation task",
        "expected the first call to describe the task it started, got {:?}",
        element(&body, "Description")
    );
}

/// RULE: what a call reports is the state of the account's one credential-report
/// generation, read off that record and the clock — so the rows are where the clock
/// stands: before the generation is due, at it, and on each side of the published rate
/// that governs generating another report. Every value `ReportStateType` declares is
/// reached.
#[tokio::test]
async fn the_clock_and_the_held_generation_are_what_a_call_reports() {
    let (router, engine) = router_with_engine();

    // No generation on record: one is started.
    let (status, body) = generate(&router, None).await;
    assert_state("no generation on record", status, &body, STARTED);
    assert!(
        holds_a_generation(&engine),
        "the started generation must be recorded"
    );

    // Still running, because the span is stored rather than slept for: a caller that calls
    // again straight away sees the generation in flight, and sees it however many times it
    // asks.
    for attempt in 0..2 {
        let case = format!("a generation still running, attempt {attempt}");
        let (status, body) = generate(&router, None).await;
        assert_state(&case, status, &body, INPROGRESS);
    }

    // One second short of due: still running. The span is the emulator's own
    // (`state::CREDENTIAL_REPORT_GENERATION_SECONDS`), so the row is written against it
    // rather than against a literal.
    rewind(&engine, state::CREDENTIAL_REPORT_GENERATION_SECONDS - 1);
    let (status, body) = generate(&router, None).await;
    assert_state("one second short of due", status, &body, INPROGRESS);

    // Due: the completion the clock already made due is reported.
    rewind(&engine, 1);
    let (status, body) = generate(&router, None).await;
    assert_state("the generation has become due", status, &body, COMPLETE);

    // The account now HAS a report, so a further call is asking for a new one — which the
    // published rate does not generate yet: the call is served and reports the generation
    // the account holds, twice over and whatever the caller.
    for attempt in 0..2 {
        let case = format!("a report asked for again inside the rate, attempt {attempt}");
        let (status, body) = generate(&router, None).await;
        assert_state(&case, status, &body, COMPLETE);
    }

    // One second short of the rate's end: still the held generation.
    rewind(
        &engine,
        state::CREDENTIAL_REPORT_REGENERATION_SECONDS - state::CREDENTIAL_REPORT_GENERATION_SECONDS
            - 1,
    );
    let (status, body) = generate(&router, None).await;
    assert_state("one second short of the rate's end", status, &body, COMPLETE);

    // From the rate's end: a new generation starts, so the account can get a fresh report.
    rewind(&engine, 1);
    let (status, body) = generate(&router, None).await;
    assert_state("the rate now allows another report", status, &body, STARTED);
    let (status, body) = generate(&router, None).await;
    assert_state("the new generation is running", status, &body, INPROGRESS);
}

/// RULE: the state a call reports is a fact about the account's generation and the clock,
/// never about who asked — so two different callers polling one generation see the same
/// state, and the completion is reported to whichever call first finds it due.
#[tokio::test]
async fn the_reported_state_follows_the_generation_rather_than_the_caller() {
    let (router, engine) = router_with_engine();
    let grant = vec![(
        "generate".to_string(),
        statement("Allow", "iam:GenerateCredentialReport", "*"),
    )];
    let first = principal(&router, "first poller", "report-reader-one", &grant).await;
    let second = principal(&router, "second poller", "report-reader-two", &grant).await;

    let (status, body) = generate(&router, Some(&first)).await;
    assert_state("the first caller starts the generation", status, &body, STARTED);
    let (status, body) = generate(&router, Some(&second)).await;
    assert_state("another caller polls the same generation", status, &body, INPROGRESS);

    rewind(&engine, state::CREDENTIAL_REPORT_GENERATION_SECONDS);
    let (status, body) = generate(&router, Some(&second)).await;
    assert_state("the other caller finds it due", status, &body, COMPLETE);
    let (status, body) = generate(&router, Some(&first)).await;
    assert_state(
        "the caller that started it reads the same completion",
        status,
        &body,
        COMPLETE,
    );
}

/// RULE: IAM decides the call against the caller's identity-based policies before it
/// records anything, so a caller no policy allows the action for is refused with HTTP 403
/// `AccessDenied` naming the principal, the action and the account-wide resource — and the
/// refused call starts no generation.
#[tokio::test]
async fn a_caller_whose_policies_do_not_allow_the_call_is_refused() {
    // Rows: a caller whose only grant is for ANOTHER action, and one explicitly denied this
    // action by a second statement.
    let rows = vec![
        (
            "no allow at all",
            vec![(
                "unrelated".to_string(),
                statement("Allow", "iam:ListUsers", "*"),
            )],
            "because no identity-based policy allows the iam:GenerateCredentialReport \
             action"
                .to_string(),
        ),
        (
            "an explicit deny over the allow",
            vec![
                (
                    "allow".to_string(),
                    statement("Allow", "iam:GenerateCredentialReport", "*"),
                ),
                (
                    "deny".to_string(),
                    statement("Deny", "iam:GenerateCredentialReport", "*"),
                ),
            ],
            "with an explicit deny in an identity-based policy".to_string(),
        ),
    ];

    for (row, (case, policies, reason)) in rows.into_iter().enumerate() {
        let (router, engine) = router_with_engine();
        let user_name = format!("reporter{row}");
        let access_key_id = principal(&router, case, &user_name, &policies).await;
        let (status, body) = generate(&router, Some(&access_key_id)).await;
        assert_eq!(
            status, 403,
            "case {case:?}: expected 403, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "AccessDenied",
            "case {case:?}: expected AccessDenied, got {:?}: {body}",
            error_code(&body)
        );
        assert_access_denied_message(
            case,
            &element(&body, "Message"),
            &format!("arn:aws:iam::{}:user/{user_name}", account_id()),
            "iam:GenerateCredentialReport",
            state::authorization::ACCOUNT_WIDE_RESOURCE,
            &reason,
        );
        assert!(
            !holds_a_generation(&engine),
            "case {case:?}: a refused call must start no generation"
        );
    }

    // The same call under a statement that names its action is served, so the gate decides
    // it from the policies rather than refusing everything.
    let (router, engine) = router_with_engine();
    let access_key_id = principal(
        &router,
        "a granted caller",
        "reporter-granted",
        &[(
            "generate".to_string(),
            statement("Allow", "iam:GenerateCredentialReport", "*"),
        )],
    )
    .await;
    let (status, body) = generate(&router, Some(&access_key_id)).await;
    assert_state("a granted caller", status, &body, STARTED);
    assert!(
        holds_a_generation(&engine),
        "a served call must record the generation it started"
    );
}

/// RULE: the response carries exactly the two members the output shape declares — one
/// `State` and one `Description` — so a client reading either finds a single value.
#[tokio::test]
async fn the_response_carries_one_state_and_one_description() {
    let (router, _engine) = router_with_engine();
    let (status, body) = generate(&router, None).await;
    assert_eq!(status, 200, "expected 200, got {status}: {body}");
    assert_eq!(
        elements(&body, "State").len(),
        1,
        "expected exactly one <State>, got {:?}: {body}",
        elements(&body, "State")
    );
    assert_eq!(
        elements(&body, "Description").len(),
        1,
        "expected exactly one <Description>, got {:?}: {body}",
        elements(&body, "Description")
    );
}
