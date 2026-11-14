// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `CancelRotateSecret`, driven over the wire through the
//! multi-service router and read back through `DescribeSecret`, `ListSecrets`,
//! `GetSecretValue` and `RotateSecret`.
//!
//! The rules under test:
//!
//! * `CancelRotateSecret` turns automatic rotation off for the secret a
//!   `SecretId` resolves to — rotation is reported disabled and the schedule
//!   stops making a rotation due — while the rotation configuration the secret
//!   stores is left in place, so a later `RotateSecret` that names none of it
//!   rotates with the values already stored;
//! * a rotation still in progress is cancelled and its `AWSPENDING` version id
//!   is reported back, and only then: a call with no rotation in progress
//!   reports no version id. The staging labels are left as the cancelled
//!   rotation left them;
//! * the call reports the secret's ARN and name and succeeds whether or not
//!   rotation was ever configured, so repeating it answers the same thing;
//! * a `SecretId` that names no secret is `ResourceNotFoundException`, and a
//!   secret inside its recovery window — which still exists — is
//!   `InvalidRequestException`.
//!
//! Every test is a TABLE over the class its rule generalises: how the rotation
//! schedule is expressed (days, a `rate()` interval, a `cron()` expression),
//! whether the rotation was started immediately, the form of the `SecretId`
//! (name, full ARN, partial ARN), and whether rotation was ever configured.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_secretsmanager_secretsmanager::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 header naming `secretsmanager` in `us-west-2`; the router routes on
/// the credential scope and the emulator verifies signatures only when an
/// authenticator is registered, which this harness does not do.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-west-2/secretsmanager/aws4_request, \
     SignedHeaders=host;x-amz-date, Signature=deadbeef";

/// The ARN of a Lambda rotation function, as `RotationLambdaARN` carries it.
const LAMBDA_ARN: &str = "arn:aws:lambda:us-west-2:123456789012:function:RotateMySecret";

/// The one sentence `ResourceNotFoundException` carries for a secret this
/// service does not hold ("Secrets Manager can't find the resource that you
/// asked for.", `ResourceNotFoundException`, this service's Smithy model).
const SECRET_NOT_FOUND: &str = "Secrets Manager can't find the specified secret.";

/// The sentence an operation a secret's state forbids carries ("The secret is
/// scheduled for deletion.", `InvalidRequestException`, this service's Smithy
/// model).
const MARKED_FOR_DELETION: &str =
    "You can't perform this operation on the secret because it was marked for deletion.";

/// How one row names the secret in its cancel: the secret's name and its full
/// ARN go in, the `SecretId` the call carries comes out.
type SecretIdForm = fn(&str, &str) -> String;

/// How rotation was configured for one row, and whether a rotation was started.
struct Setup {
    /// The `RotationRules` the `RotateSecret` call carries.
    rules: Value,
    /// Whether that call started a rotation straight away, which is what leaves
    /// a rotation in progress for `CancelRotateSecret` to cancel.
    rotate_immediately: bool,
    /// Whether the schedule makes a next rotation due, which every schedule the
    /// service reads does: an interval names one a span past the call, and a
    /// `cron()` expression the next occurrence it names on the UTC calendar.
    schedules_next: bool,
}

/// A fresh emulator holding only this service, over its own engine.
fn emulator() -> MultiServiceRouter {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.register_service(EmulatedService::new(engine));
    router
}

/// Send one `awsJson1_1` request and return its status and JSON body.
async fn call(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("secretsmanager.{operation}"))
        .header("authorization", AUTHORIZATION)
        .body(bytes::Bytes::from(
            // A write that stores a value creates a version the caller has to
            // name; see `secret_client_request_token::with_staked_version`.
            serde_json::to_vec(&crate::secret_client_request_token::with_staked_version(
                operation, body,
            ))
            .expect("the request body is serializable"),
        ))
        .expect("a valid request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// Send one request that must succeed, and return its body.
async fn ok(router: &MultiServiceRouter, operation: &str, body: Value, case: &str) -> Value {
    let (status, parsed) = call(router, operation, &body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 from {operation}, got {status} with body {parsed}"
    );
    parsed
}

/// Send one request that must be refused with a 400, and return its wire error
/// code and message.
async fn err(
    router: &MultiServiceRouter,
    operation: &str,
    body: Value,
    case: &str,
) -> (String, String) {
    let (status, parsed) = call(router, operation, &body).await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected a 400 client error from {operation}, got {status} with body \
         {parsed}"
    );
    let raw = parsed
        .get("__type")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: no __type in error body {parsed}"));
    let code = raw.split('#').next_back().unwrap_or(raw).to_string();
    let message = parsed
        .get("message")
        .or_else(|| parsed.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: no message in error body {parsed}"))
        .to_string();
    (code, message)
}

/// A required string member of a response body.
fn text(parsed: &Value, member: &str, case: &str) -> String {
    parsed
        .get(member)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: no {member} in {parsed}"))
        .to_string()
}

/// The staging labels a `VersionIdsToStages` map reports for one version id, or
/// an empty list when the map does not carry that id at all.
fn stages_of(parsed: &Value, member: &str, version_id: &str) -> Vec<String> {
    parsed
        .get(member)
        .and_then(Value::as_object)
        .and_then(|map| map.get(version_id))
        .and_then(Value::as_array)
        .map(|stages| {
            stages
                .iter()
                .map(|stage| stage.as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// Create a secret holding `value`, and return its ARN and the id of the
/// version `AWSCURRENT` names.
async fn create_secret(
    router: &MultiServiceRouter,
    name: &str,
    value: &str,
    case: &str,
) -> (String, String) {
    let created = ok(
        router,
        "CreateSecret",
        json!({"Name": name, "SecretString": value}),
        case,
    )
    .await;
    (
        text(&created, "ARN", case),
        text(&created, "VersionId", case),
    )
}

/// `DescribeSecret` for one secret.
async fn describe(router: &MultiServiceRouter, secret_id: &str, case: &str) -> Value {
    ok(router, "DescribeSecret", json!({"SecretId": secret_id}), case).await
}

/// The ARN without the trailing hyphen and six random characters — the partial
/// ARN form a `SecretId` is also accepted in.
fn partial_arn(arn: &str) -> String {
    arn[..arn.len() - 7].to_string()
}

/// `CancelRotateSecret` reports the secret it names, turns rotation off and
/// clears the next due instant, keeps the rotation configuration, and reports
/// the `AWSPENDING` version id exactly when it cancelled a rotation still in
/// progress.
///
/// The rows differ along how the schedule is expressed and at which bound,
/// whether the rotation was started immediately, and the form of the `SecretId`
/// the cancel names.
#[tokio::test]
async fn cancel_turns_rotation_off_and_keeps_the_configuration() {
    // (case, how rotation was configured, how the cancel names the secret)
    let rows: &[(&str, Setup, SecretIdForm)] = &[
        (
            "a days schedule at the documented minimum, rotation in progress, cancelled by name",
            Setup {
                rules: json!({"AutomaticallyAfterDays": 1}),
                rotate_immediately: true,
                schedules_next: true,
            },
            |name, _arn| name.to_string(),
        ),
        (
            "a rate in hours at the documented four-hour floor, rotation in progress, cancelled \
             by full ARN",
            Setup {
                rules: json!({"ScheduleExpression": "rate(4 hours)"}),
                rotate_immediately: true,
                schedules_next: true,
            },
            |_name, arn| arn.to_string(),
        ),
        (
            "a days schedule at the documented maximum, nothing staged, cancelled by partial ARN",
            Setup {
                rules: json!({"AutomaticallyAfterDays": 1000}),
                rotate_immediately: false,
                schedules_next: true,
            },
            |_name, arn| partial_arn(arn),
        ),
        (
            "a cron schedule, nothing staged, cancelled by name",
            Setup {
                rules: json!({"ScheduleExpression": "cron(0 8 1 * ? *)"}),
                rotate_immediately: false,
                schedules_next: true,
            },
            |name, _arn| name.to_string(),
        ),
        (
            "a rate in days with a rotation window, rotation in progress, cancelled by name",
            Setup {
                rules: json!({"ScheduleExpression": "rate(10 days)", "Duration": "3h"}),
                rotate_immediately: true,
                schedules_next: true,
            },
            |name, _arn| name.to_string(),
        ),
    ];

    let router = emulator();
    for (index, (case, setup, secret_id_of)) in rows.iter().enumerate() {
        let name = format!("cancelled-{index}");
        let value = format!("the-current-value-{index}");
        let (arn, current) = create_secret(&router, &name, &value, case).await;

        let rotated = ok(
            &router,
            "RotateSecret",
            json!({
                "SecretId": name,
                "RotationLambdaARN": LAMBDA_ARN,
                "RotationRules": setup.rules,
                "RotateImmediately": setup.rotate_immediately,
            }),
            case,
        )
        .await;
        let rotation_version = text(&rotated, "VersionId", case);

        // Rotation is on, and an interval schedule has fixed the next due
        // instant, BEFORE the cancel — so what the cancel changes is observable.
        let described = describe(&router, &name, case).await;
        assert_eq!(
            described.get("RotationEnabled"),
            Some(&json!(true)),
            "case {case:?}: expected RotationEnabled true after RotateSecret, got {described}"
        );
        assert_eq!(
            described.get("NextRotationDate").is_some(),
            setup.schedules_next,
            "case {case:?}: expected NextRotationDate present={} after RotateSecret, got \
             {described}",
            setup.schedules_next
        );

        let secret_id = secret_id_of(&name, &arn);
        let cancelled = ok(
            &router,
            "CancelRotateSecret",
            json!({"SecretId": secret_id}),
            case,
        )
        .await;
        assert_eq!(
            text(&cancelled, "ARN", case),
            arn,
            "case {case:?}: expected CancelRotateSecret to report the secret's ARN {arn}, got \
             {cancelled}"
        );
        assert_eq!(
            text(&cancelled, "Name", case),
            name,
            "case {case:?}: expected CancelRotateSecret to report the secret's name {name}, got \
             {cancelled}"
        );
        if setup.rotate_immediately {
            // The version the cancelled rotation was creating, which the caller
            // needs in order to clean it up.
            assert_eq!(
                cancelled.get("VersionId"),
                Some(&json!(rotation_version)),
                "case {case:?}: expected the cancelled rotation's AWSPENDING version \
                 {rotation_version:?}, got {cancelled}"
            );
        } else {
            assert_eq!(
                cancelled.get("VersionId"),
                None,
                "case {case:?}: expected no VersionId when no rotation was in progress, got \
                 {cancelled}"
            );
        }

        // Rotation is off and nothing is due, while the configuration the
        // secret stores is untouched.
        let described = describe(&router, &secret_id, case).await;
        assert_eq!(
            described.get("RotationEnabled"),
            Some(&json!(false)),
            "case {case:?}: expected RotationEnabled false after the cancel, got {described}"
        );
        assert_eq!(
            described.get("NextRotationDate"),
            None,
            "case {case:?}: expected no NextRotationDate once rotation is off, got {described}"
        );
        assert_eq!(
            text(&described, "RotationLambdaARN", case),
            LAMBDA_ARN,
            "case {case:?}: expected the rotation function to survive the cancel, got {described}"
        );
        assert_eq!(
            described.get("RotationRules"),
            Some(&setup.rules),
            "case {case:?}: expected the rotation rules to survive the cancel, got {described}"
        );
        assert_eq!(
            described.get("LastRotatedDate"),
            None,
            "case {case:?}: expected no LastRotatedDate for a rotation that never completed, got \
             {described}"
        );

        // The staging labels are left as the cancelled rotation left them: the
        // current value is still current and still readable, and a cancelled
        // in-progress rotation still holds AWSPENDING on the version it was
        // creating.
        assert_eq!(
            stages_of(&described, "VersionIdsToStages", &current),
            vec!["AWSCURRENT".to_string()],
            "case {case:?}: expected the stored version to keep AWSCURRENT, got {described}"
        );
        let expected_pending: Vec<String> = if setup.rotate_immediately {
            vec!["AWSPENDING".to_string()]
        } else {
            Vec::new()
        };
        assert_eq!(
            stages_of(&described, "VersionIdsToStages", &rotation_version),
            expected_pending,
            "case {case:?}: expected the cancelled rotation's labels to be left alone, got \
             {described}"
        );
        let read = ok(
            &router,
            "GetSecretValue",
            json!({"SecretId": secret_id}),
            case,
        )
        .await;
        assert_eq!(
            text(&read, "SecretString", case),
            value,
            "case {case:?}: expected the current value to still be readable, got {read}"
        );

        // ListSecrets projects the same rotation state.
        let listed = ok(
            &router,
            "ListSecrets",
            json!({"Filters": [{"Key": "name", "Values": [name]}]}),
            case,
        )
        .await;
        let entry = listed
            .get("SecretList")
            .and_then(Value::as_array)
            .and_then(|list| list.first())
            .unwrap_or_else(|| panic!("case {case:?}: no SecretList entry in {listed}"));
        assert_eq!(
            entry.get("RotationEnabled"),
            Some(&json!(false)),
            "case {case:?}: expected ListSecrets to report rotation off too, got {entry}"
        );

        // Cancelling again answers exactly the same: nothing about the call
        // depends on rotation having been on.
        let again = ok(
            &router,
            "CancelRotateSecret",
            json!({"SecretId": secret_id}),
            case,
        )
        .await;
        assert_eq!(
            again, cancelled,
            "case {case:?}: expected a repeated cancel to answer the same, got {again} after \
             {cancelled}"
        );
    }
}

/// A secret rotation was never configured for has no schedule to turn off, so
/// the cancel reports the secret and no version id, and leaves rotation off.
///
/// The rows differ along the form of the `SecretId` and the kind of value the
/// secret holds.
#[tokio::test]
async fn cancel_succeeds_when_rotation_was_never_configured() {
    // (case, how the cancel names the secret)
    let rows: &[(&str, SecretIdForm)] = &[
        ("by name", |name, _arn| name.to_string()),
        ("by full ARN", |_name, arn| arn.to_string()),
        ("by partial ARN", |_name, arn| partial_arn(arn)),
    ];

    let router = emulator();
    for (index, (case, secret_id_of)) in rows.iter().enumerate() {
        let name = format!("never-rotated-{index}");
        let (arn, current) = create_secret(&router, &name, "a-plain-value", case).await;
        let secret_id = secret_id_of(&name, &arn);

        let cancelled = ok(
            &router,
            "CancelRotateSecret",
            json!({"SecretId": secret_id}),
            case,
        )
        .await;
        assert_eq!(
            text(&cancelled, "ARN", case),
            arn,
            "case {case:?}: expected the secret's ARN {arn}, got {cancelled}"
        );
        assert_eq!(
            text(&cancelled, "Name", case),
            name,
            "case {case:?}: expected the secret's name {name}, got {cancelled}"
        );
        assert_eq!(
            cancelled.get("VersionId"),
            None,
            "case {case:?}: expected no VersionId when no rotation was in progress, got {cancelled}"
        );

        let described = describe(&router, &name, case).await;
        assert_eq!(
            described.get("RotationEnabled"),
            Some(&json!(false)),
            "case {case:?}: expected rotation to stay off, got {described}"
        );
        assert_eq!(
            described.get("RotationLambdaARN"),
            None,
            "case {case:?}: expected the cancel to configure no rotation function, got {described}"
        );
        assert_eq!(
            stages_of(&described, "VersionIdsToStages", &current),
            vec!["AWSCURRENT".to_string()],
            "case {case:?}: expected the stored version to be untouched, got {described}"
        );
    }
}

/// Because the cancel keeps the rotation configuration, `RotateSecret` turns
/// rotation back on from the values already stored, naming none of them.
///
/// The rows differ along how the kept schedule is expressed.
#[tokio::test]
async fn rotation_restarts_from_the_kept_configuration() {
    // (case, the RotationRules the first RotateSecret stored)
    let rows: &[(&str, Value)] = &[
        ("a days schedule", json!({"AutomaticallyAfterDays": 14})),
        (
            "a rate in hours",
            json!({"ScheduleExpression": "rate(6 hours)"}),
        ),
        (
            "a rate in days at the documented thousand-day maximum",
            json!({"ScheduleExpression": "rate(1000 days)"}),
        ),
    ];

    let router = emulator();
    for (index, (case, rules)) in rows.iter().enumerate() {
        let name = format!("restarted-{index}");
        create_secret(&router, &name, "the-current-value", case).await;

        // `RotateImmediately` false so nothing is staged and the restart below
        // is not refused for a rotation still in progress.
        ok(
            &router,
            "RotateSecret",
            json!({
                "SecretId": name,
                "RotationLambdaARN": LAMBDA_ARN,
                "RotationRules": rules,
                "RotateImmediately": false,
            }),
            case,
        )
        .await;
        ok(
            &router,
            "CancelRotateSecret",
            json!({"SecretId": name}),
            case,
        )
        .await;

        // The restart names no rotation function and no rules at all.
        ok(
            &router,
            "RotateSecret",
            json!({"SecretId": name, "RotateImmediately": false}),
            case,
        )
        .await;
        let described = describe(&router, &name, case).await;
        assert_eq!(
            described.get("RotationEnabled"),
            Some(&json!(true)),
            "case {case:?}: expected rotation back on after a bare RotateSecret, got {described}"
        );
        assert_eq!(
            described.get("RotationRules"),
            Some(rules),
            "case {case:?}: expected the kept rules to be rotated with, got {described}"
        );
        assert!(
            described.get("NextRotationDate").is_some(),
            "case {case:?}: expected the kept schedule to make a rotation due again, got \
             {described}"
        );
    }
}

/// `CancelRotateSecret` refuses by the state of what the `SecretId` resolves to:
/// a secret this service does not hold is `ResourceNotFoundException`, and one
/// inside its recovery window — which still exists — is
/// `InvalidRequestException`.
///
/// The rows differ along the form of the id and why it is refused.
#[tokio::test]
async fn cancel_refuses_an_unknown_or_deleted_secret() {
    let router = emulator();
    let (arn, _current) = create_secret(&router, "to-be-deleted", "a-value", "setup").await;
    // A recovery window, so the secret stays marked for deletion rather than
    // being purged while the table runs.
    ok(
        &router,
        "DeleteSecret",
        json!({"SecretId": "to-be-deleted", "RecoveryWindowInDays": 30}),
        "setup",
    )
    .await;
    let (live_arn, _) = create_secret(&router, "still-live", "a-value", "setup").await;

    // (case, the SecretId the cancel names, the expected code and message)
    let rows: &[(&str, String, &str, &str)] = &[
        (
            "a name no secret has",
            "never-created".to_string(),
            "ResourceNotFoundException",
            SECRET_NOT_FOUND,
        ),
        (
            "a full ARN no secret has",
            format!("{live_arn}x"),
            "ResourceNotFoundException",
            SECRET_NOT_FOUND,
        ),
        (
            "a secret inside its recovery window, by name",
            "to-be-deleted".to_string(),
            "InvalidRequestException",
            MARKED_FOR_DELETION,
        ),
        (
            "a secret inside its recovery window, by full ARN",
            arn.clone(),
            "InvalidRequestException",
            MARKED_FOR_DELETION,
        ),
        (
            "a secret inside its recovery window, by partial ARN",
            partial_arn(&arn),
            "InvalidRequestException",
            MARKED_FOR_DELETION,
        ),
    ];

    for (case, secret_id, expected_code, expected_message) in rows {
        let (code, message) = err(
            &router,
            "CancelRotateSecret",
            json!({"SecretId": secret_id}),
            case,
        )
        .await;
        assert_eq!(
            &code, expected_code,
            "case {case:?}: expected {expected_code}, got {code:?} with message {message:?}"
        );
        assert_eq!(
            &message, expected_message,
            "case {case:?}: expected the message {expected_message:?}, got {message:?}"
        );
    }

    // The live secret is untouched by any of those refusals: its own cancel
    // still succeeds.
    let cancelled = ok(
        &router,
        "CancelRotateSecret",
        json!({"SecretId": "still-live"}),
        "the live secret",
    )
    .await;
    assert_eq!(
        text(&cancelled, "ARN", "the live secret"),
        live_arn,
        "expected the live secret's own cancel to still answer, got {cancelled}"
    );
}
