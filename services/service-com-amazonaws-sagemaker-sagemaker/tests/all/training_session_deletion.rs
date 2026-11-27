// SPDX-License-Identifier: CC0-1.0

//! End-to-end wire tests for deleting a SageMaker training session:
//! `DeleteTrainingSession` and the readers that must stop resolving the name it released.
//!
//! RULE: a delete releases the resource — every later read of the name reports the
//! modelled not-found, and a second delete of the same name reports it too — and it is
//! refused, with nothing released, while the resource's documented precondition is unmet.
//! Here that precondition is `DeleteTrainingSession`'s own ("Deletes a training session.
//! You must stop a running session before deleting it. Use the StopTrainingSession
//! operation to stop the session first" —
//! `smithy_query(query="operation",operation="DeleteTrainingSession")`), so a session the
//! caller has not stopped is refused as a constraint violation naming the status it is in,
//! and a session that has reached a terminal status is released.
//!
//! Every case goes over the wire (`POST /` with `X-Amz-Target: SageMaker.<Op>`,
//! `awsJson1_1`) through the same `MultiServiceRouter` the production router uses.
//!
//! The rows span what the rule generalises over rather than one recorded call: the status
//! the delete is asked in (still provisioning, running, quiescing, stopped), the name's
//! shape, a name nothing created, and the sessions standing beside the one deleted.

use serde_json::{json, Value};

// The crate's ONE wire harness, plus the training-session helpers its own suites define.
use super::inference_deployment_lifecycle::{error_code, message, Emulator, ACCOUNT};
use super::training_session_lifecycle::{
    create_request, field, poll_until_status, STATUS_CREATED, STATUS_RUNNING,
};

/// The `TrainingSessionStatus` members a stopped session passes through.
const STATUS_STOPPING: &str = "Stopping";
/// See [`STATUS_STOPPING`].
const STATUS_STOPPED: &str = "Stopped";

/// The ARN a create mints for `name` in `us-east-1`, which the refusal below quotes.
fn session_arn(name: &str) -> String {
    format!("arn:aws:sagemaker:us-east-1:{ACCOUNT}:training-session/{name}")
}

/// Delete `name` and answer what the call reported.
async fn delete(emulator: &Emulator, name: &str) -> (u16, Value) {
    emulator
        .call(
            "DeleteTrainingSession",
            &json!({ "TrainingSessionName": name }),
        )
        .await
}

/// What `DescribeTrainingSession` answers for `name`.
async fn describe_raw(emulator: &Emulator, name: &str) -> (u16, Value) {
    emulator
        .call(
            "DescribeTrainingSession",
            &json!({ "TrainingSessionName": name }),
        )
        .await
}

/// RULE: a session the caller has not stopped is NOT released — the delete is refused as a
/// constraint violation naming the session and the status it is in, and the session reads
/// back exactly as it did.
#[tokio::test]
async fn deleting_a_session_that_was_not_stopped_is_refused() {
    let emulator = Emulator::new();

    // (case, the session's name, whether to let the clock move it to `Running` first, the
    // status the refusal must name). Both non-terminal states a session reaches on its way
    // up are rows, because the precondition is about what is still running.
    let rows: Vec<(&str, &str, bool, &str)> = vec![
        ("still-provisioning", "provisioning", false, STATUS_CREATED),
        ("running", "running-session", true, STATUS_RUNNING),
    ];

    for (case, name, let_it_run, status) in rows {
        emulator
            .ok("CreateTrainingSession", &create_request(name, json!({})))
            .await;
        let before = if let_it_run {
            poll_until_status(&emulator, name, STATUS_RUNNING).await
        } else {
            let (_, parsed) = describe_raw(&emulator, name).await;
            field(&parsed, "TrainingSessionDetail")
        };
        assert_eq!(
            field(&before, "TrainingSessionStatus"),
            json!(status),
            "case {case}: expected the session to be deleted from {status}, got {before}"
        );

        let (http, parsed) = delete(&emulator, name).await;
        assert_eq!(
            (http, error_code(&parsed).as_str()),
            (400, "ValidationException"),
            "case {case}: a session that was not stopped must be refused, got {http} with \
             {parsed}"
        );
        assert_eq!(
            message(&parsed),
            format!(
                "Unable to delete training session {} because it is in {status} status. You \
                 must stop a running session before deleting it.",
                session_arn(name)
            ),
            "case {case}: the refusal must name the session and the status it is in, got \
             {parsed}"
        );

        // Nothing was released: the session still reads back, in the state it was in.
        let (http, parsed) = describe_raw(&emulator, name).await;
        assert_eq!(
            http, 200,
            "case {case}: a refused delete must release nothing, got {http} with {parsed}"
        );
        assert_eq!(
            field(&field(&parsed, "TrainingSessionDetail"), "TrainingSessionStatus"),
            json!(status),
            "case {case}: a refused delete must not move the session, got {parsed}"
        );
    }
}

/// RULE: a session that has reached a terminal status is RELEASED — the delete answers its
/// empty modelled output, every later read of the name reports the modelled not-found, and
/// the sessions beside it are untouched.
#[tokio::test]
async fn deleting_a_stopped_session_releases_the_name() {
    let emulator = Emulator::new();

    // The names span what the rule generalises over: a single character and a hyphenated
    // name, both inside `TrainingSessionName`'s `^[a-zA-Z0-9](-*[a-zA-Z0-9]){0,62}$`.
    let deleted = ["d", "deleted-session"];
    let kept = "kept-session";
    for name in deleted.iter().copied().chain([kept]) {
        emulator
            .ok("CreateTrainingSession", &create_request(name, json!({})))
            .await;
    }

    for name in deleted {
        emulator
            .ok(
                "StopTrainingSession",
                &json!({ "TrainingSessionName": name }),
            )
            .await;
        // The stop passes through its transient status before the terminal one, and only
        // the terminal one admits the delete: the wait is for that CONDITION.
        let stopping = {
            let (_, parsed) = describe_raw(&emulator, name).await;
            field(
                &field(&parsed, "TrainingSessionDetail"),
                "TrainingSessionStatus",
            )
        };
        assert!(
            stopping == json!(STATUS_STOPPING) || stopping == json!(STATUS_STOPPED),
            "case {name}: a stopped session quiesces through {STATUS_STOPPING}, got \
             {stopping}"
        );
        poll_until_status(&emulator, name, STATUS_STOPPED).await;

        let (http, parsed) = delete(&emulator, name).await;
        assert_eq!(
            http, 200,
            "case {name}: a stopped session must be released, got {http} with {parsed}"
        );
        assert_eq!(
            parsed.as_object().map(serde_json::Map::len).unwrap_or(0),
            0,
            "case {name}: DeleteTrainingSession models no output member, got {parsed}"
        );

        // The release is observable: the name resolves to nothing, and deleting it again
        // reports that too rather than a second success.
        let (http, parsed) = describe_raw(&emulator, name).await;
        assert_eq!(
            (http, error_code(&parsed).as_str()),
            (400, "ResourceNotFound"),
            "case {name}: a released session must stop resolving, got {http} with {parsed}"
        );
        let (http, parsed) = delete(&emulator, name).await;
        assert_eq!(
            (http, error_code(&parsed).as_str(), message(&parsed)),
            (
                400,
                "ResourceNotFound",
                format!("Could not find training session \"{name}\".")
            ),
            "case {name}: a second delete must report the name it cannot find, got {http} \
             with {parsed}"
        );
    }

    // The session beside them is untouched: a delete releases the session it names and no
    // other.
    let (http, parsed) = describe_raw(&emulator, kept).await;
    assert_eq!(
        http, 200,
        "case {kept}: deleting another session must leave this one, got {http} with {parsed}"
    );
}

/// RULE: a delete of a name no create stored is the modelled `ResourceNotFound` naming the
/// identifier the request carried, and it brings no session into being.
#[tokio::test]
async fn deleting_an_unknown_training_session_is_refused_with_resource_not_found() {
    let emulator = Emulator::new();
    emulator
        .ok("CreateTrainingSession", &create_request("present", json!({})))
        .await;

    // Rows: a name never created, one differing from a stored name only in case, one
    // differing by a single character, and the longest legal name.
    let longest = format!("z{}", "y".repeat(62));
    let rows: Vec<(&str, String)> = vec![
        ("never-created", "absent".to_string()),
        ("case-differs-from-a-stored-name", "Present".to_string()),
        ("one-character-off", "presen".to_string()),
        ("longest-legal-name", longest),
    ];

    for (case, name) in &rows {
        let (http, parsed) = delete(&emulator, name).await;
        assert_eq!(
            (http, error_code(&parsed).as_str(), message(&parsed)),
            (
                400,
                "ResourceNotFound",
                format!("Could not find training session \"{name}\".")
            ),
            "case {case}: expected the modelled not-found quoting the name, got {http} with \
             {parsed}"
        );
        let (http, parsed) = describe_raw(&emulator, name).await;
        assert_eq!(
            (http, error_code(&parsed).as_str()),
            (400, "ResourceNotFound"),
            "case {case}: a refused delete must write nothing, got {http} with {parsed}"
        );
    }

    // The session the account does hold is untouched by every refusal above.
    let (http, _) = describe_raw(&emulator, "present").await;
    assert_eq!(
        http, 200,
        "a refused delete must not release another session"
    );
}

/// RULE: a request outside the operation's modelled constraints is refused as a constraint
/// violation — never as a not-implemented operation — and reaches no store.
#[tokio::test]
async fn delete_constraint_violations_are_refused_as_validation_errors() {
    let emulator = Emulator::new();

    // `TrainingSessionName` is `len 1..63, pattern ^[a-zA-Z0-9](-*[a-zA-Z0-9]){0,62}$`, so
    // the rows carry a name one character past the published maximum, the empty name one
    // short of the minimum, a name whose characters the pattern does not admit, and a
    // request missing the required member altogether.
    let too_long = "a".repeat(64);
    let rows: Vec<(&str, Value)> = vec![
        (
            "one-past-the-published-maximum-length",
            json!({ "TrainingSessionName": too_long }),
        ),
        (
            "one-short-of-the-published-minimum-length",
            json!({ "TrainingSessionName": "" }),
        ),
        (
            "characters-outside-the-pattern",
            json!({ "TrainingSessionName": "not a session name" }),
        ),
        ("missing-the-required-name", json!({})),
    ];

    for (case, body) in &rows {
        let (http, parsed) = emulator.call("DeleteTrainingSession", body).await;
        assert_eq!(
            (http, error_code(&parsed).as_str()),
            (400, "ValidationException"),
            "case {case}: the delete must be refused as a constraint violation, got {http} \
             with {parsed}"
        );
    }
}
