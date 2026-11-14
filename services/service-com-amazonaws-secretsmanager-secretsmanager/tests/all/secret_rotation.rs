// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for Secrets Manager rotation: `RotateSecret` against
//! `DescribeSecret`, `ListSecrets`, `GetSecretValue` and `PutSecretValue`,
//! driven over the wire through the multi-service router.
//!
//! Every test is a TABLE over the class the rule generalises: how the rotation
//! schedule is expressed (days, a `rate()` interval, a `cron()` expression) and
//! at which published bound, the form of the `SecretId`, whether the caller
//! supplied the version id, and whether the rotation starts immediately. One
//! recorded request is one row.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

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
pub(crate) const LAMBDA_ARN: &str = "arn:aws:lambda:us-west-2:123456789012:function:RotateMySecret";

/// Seconds in an hour and in a day, for the intervals a schedule names.
pub(crate) const HOUR: f64 = 3_600.0;
/// See [`HOUR`].
pub(crate) const DAY: f64 = 86_400.0;

/// How far a reported `NextRotationDate` may sit from the instant the test
/// computed for it: the emulator reads its own clock while serving the call, so
/// the two differ by the time the call took.
pub(crate) const CLOCK_SLACK_SECONDS: f64 = 120.0;

/// A fresh emulator holding only this service, over its own engine.
pub(crate) fn emulator() -> MultiServiceRouter {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.register_service(EmulatedService::new(engine));
    router
}

/// Send one `awsJson1_1` request and return its status and JSON body.
pub(crate) async fn call(
    router: &MultiServiceRouter,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
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
pub(crate) async fn ok(
    router: &MultiServiceRouter,
    operation: &str,
    body: Value,
    case: &str,
) -> Value {
    let (status, parsed) = call(router, operation, &body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 from {operation}, got {status} with body {parsed}"
    );
    parsed
}

/// Send one request that must be refused with a 400, and return its wire error
/// code.
async fn err(router: &MultiServiceRouter, operation: &str, body: Value, case: &str) -> String {
    let (status, parsed) = call(router, operation, &body).await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected a 400 client error from {operation}, got {status} with body \
         {parsed}"
    );
    error_code(&parsed, case)
}

/// The `__type` discriminator of an `awsJson1_1` error body, without namespace.
fn error_code(parsed: &Value, case: &str) -> String {
    let raw = parsed
        .get("__type")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: no __type in error body {parsed}"));
    raw.split('#').next_back().unwrap_or(raw).to_string()
}

/// A required string member of a response body.
fn text(parsed: &Value, member: &str, case: &str) -> String {
    parsed
        .get(member)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: no {member} in {parsed}"))
        .to_string()
}

/// A required timestamp member of a response body, in seconds since the epoch.
pub(crate) fn instant(parsed: &Value, member: &str, case: &str) -> f64 {
    parsed
        .get(member)
        .and_then(Value::as_f64)
        .unwrap_or_else(|| panic!("case {case:?}: no {member} in {parsed}"))
}

/// Seconds since the epoch, for comparing a reported date with the clock.
pub(crate) fn epoch_seconds(at: SystemTime) -> f64 {
    at.duration_since(UNIX_EPOCH)
        .expect("the test clock is after the epoch")
        .as_secs_f64()
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

/// The version ids a `VersionIdsToStages` map reports as carrying `stage`,
/// sorted so the comparison does not depend on map order.
fn ids_with_stage(parsed: &Value, member: &str, stage: &str) -> Vec<String> {
    let mut ids: Vec<String> = parsed
        .get(member)
        .and_then(Value::as_object)
        .map(|map| {
            map.iter()
                .filter(|(_, stages)| {
                    stages
                        .as_array()
                        .is_some_and(|stages| stages.iter().any(|held| held == stage))
                })
                .map(|(id, _)| id.clone())
                .collect()
        })
        .unwrap_or_default();
    ids.sort();
    ids
}

/// Create a secret holding `value`, and return its ARN and the id of the
/// version `AWSCURRENT` names.
pub(crate) async fn create_secret(
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
pub(crate) async fn describe(router: &MultiServiceRouter, secret_id: &str, case: &str) -> Value {
    ok(router, "DescribeSecret", json!({"SecretId": secret_id}), case).await
}

/// The ARN without the trailing hyphen and six random characters — the partial
/// ARN form a `SecretId` is also accepted in.
fn partial_arn(arn: &str) -> String {
    arn[..arn.len() - 7].to_string()
}

/// `RotateSecret` persists the schedule the call names and reports the instant
/// that schedule next makes a rotation due.
///
/// The rows differ along how the interval is expressed and how long it is,
/// including each published bound of the documented range: `AutomaticallyAfterDays`
/// is modelled with a range of 1..1000, and a `ScheduleExpression` rate "can
/// rotate a secret as often as every four hours"
/// (`RotationRulesType$AutomaticallyAfterDays` / `$ScheduleExpression`, this
/// service's Smithy model).
#[tokio::test]
async fn rotation_rules_fix_the_next_rotation_date() {
    // (case, the RotationRules the call carries, the interval they name)
    let rows: &[(&str, Value, f64)] = &[
        (
            "AutomaticallyAfterDays at the documented minimum of 1",
            json!({"AutomaticallyAfterDays": 1}),
            DAY,
        ),
        (
            "AutomaticallyAfterDays at a middling 30",
            json!({"AutomaticallyAfterDays": 30}),
            30.0 * DAY,
        ),
        (
            "AutomaticallyAfterDays at the documented maximum of 1000",
            json!({"AutomaticallyAfterDays": 1000}),
            1000.0 * DAY,
        ),
        (
            "a rate in hours at the documented four-hour floor",
            json!({"ScheduleExpression": "rate(4 hours)"}),
            4.0 * HOUR,
        ),
        (
            "a rate in hours",
            json!({"ScheduleExpression": "rate(12 hours)"}),
            12.0 * HOUR,
        ),
        (
            "a rate in days, with a rotation window",
            json!({"ScheduleExpression": "rate(10 days)", "Duration": "3h"}),
            10.0 * DAY,
        ),
        (
            "a rate in days at the documented thousand-day maximum",
            json!({"ScheduleExpression": "rate(1000 days)"}),
            1000.0 * DAY,
        ),
    ];

    let router = emulator();
    for (index, (case, rules, interval)) in rows.iter().enumerate() {
        let name = format!("scheduled-{index}");
        let (arn, current) = create_secret(&router, &name, "the-current-value", case).await;

        let before = epoch_seconds(SystemTime::now());
        let rotated = ok(
            &router,
            "RotateSecret",
            json!({
                "SecretId": name,
                "RotationLambdaARN": LAMBDA_ARN,
                "RotationRules": rules,
                // False so this row asserts the SCHEDULE alone: nothing is
                // staged, which the staging tests below cover instead.
                "RotateImmediately": false,
            }),
            case,
        )
        .await;
        assert_eq!(
            text(&rotated, "ARN", case),
            arn,
            "case {case:?}: expected RotateSecret to report the secret's ARN {arn}, got {rotated}"
        );
        assert_eq!(
            text(&rotated, "Name", case),
            name,
            "case {case:?}: expected RotateSecret to report the secret's name {name}, got {rotated}"
        );
        let version_id = text(&rotated, "VersionId", case);
        // `SecretVersionIdType` is modelled with a length of 32..64.
        assert!(
            (32..=64).contains(&version_id.chars().count()),
            "case {case:?}: expected a VersionId of 32..64 characters, got {version_id:?}"
        );
        assert_ne!(
            version_id, current,
            "case {case:?}: expected a version id other than the current one, got {version_id:?}"
        );

        let described = describe(&router, &name, case).await;
        assert_eq!(
            described.get("RotationEnabled"),
            Some(&json!(true)),
            "case {case:?}: expected RotationEnabled true after RotateSecret, got {described}"
        );
        assert_eq!(
            text(&described, "RotationLambdaARN", case),
            LAMBDA_ARN,
            "case {case:?}: expected the rotation function to be persisted, got {described}"
        );
        assert_eq!(
            described.get("RotationRules"),
            Some(rules),
            "case {case:?}: expected the rules to round-trip, got {described}"
        );
        let next = instant(&described, "NextRotationDate", case);
        let expected = before + interval;
        assert!(
            (next - expected).abs() <= CLOCK_SLACK_SECONDS,
            "case {case:?}: expected NextRotationDate near {expected} ({interval} seconds after \
             the call), got {next}"
        );
        // Nothing rotated: no rotation function ran, so the service has no
        // completed rotation to date.
        assert_eq!(
            described.get("LastRotatedDate"),
            None,
            "case {case:?}: expected no LastRotatedDate before a rotation completes, got \
             {described}"
        );
        assert_eq!(
            stages_of(&described, "VersionIdsToStages", &current),
            vec!["AWSCURRENT".to_string()],
            "case {case:?}: expected the stored version to keep AWSCURRENT, got {described}"
        );

        // The same projection through ListSecrets, which reports the same
        // rotation configuration for the secret.
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
            entry.get("RotationRules"),
            Some(rules),
            "case {case:?}: expected ListSecrets to report the same rules, got {entry}"
        );
        assert_eq!(
            text(entry, "RotationLambdaARN", case),
            LAMBDA_ARN,
            "case {case:?}: expected ListSecrets to report the rotation function, got {entry}"
        );
    }
}

/// A `cron()` schedule is accepted in the six-field form the service documents
/// (minutes, hours, day-of-month, month, day-of-week, year), round-trips
/// unchanged, and enables rotation.
///
/// It names occurrences on the UTC calendar rather than an interval, so the next
/// rotation it makes due is the next occurrence it names — ahead of the call that
/// configured it. WHICH occurrence, for each field a cron expression restricts,
/// is the table in `secret_rotation_schedule`.
#[tokio::test]
async fn cron_schedules_round_trip_and_schedule_their_next_occurrence() {
    // (case, the expression, the longest span its calendar leaves between two
    // occurrences — so the reported instant is the next one, not a later one)
    let rows: &[(&str, &str, f64)] = &[
        ("every day at 08:00", "cron(0 8 * * ? *)", DAY),
        ("every day at 22:30", "cron(30 22 * * ? *)", DAY),
        (
            "the first of every month at midnight",
            "cron(0 0 1 * ? *)",
            31.0 * DAY,
        ),
        (
            "the fifteenth of every month at 06:45",
            "cron(45 6 15 * ? *)",
            31.0 * DAY,
        ),
        (
            "a rotation window on a cron schedule",
            "cron(0 8 1 * ? *)",
            31.0 * DAY,
        ),
    ];

    let router = emulator();
    for (index, (case, expression, longest_gap)) in rows.iter().enumerate() {
        let name = format!("cron-scheduled-{index}");
        create_secret(&router, &name, "the-current-value", case).await;

        let before = epoch_seconds(SystemTime::now());
        ok(
            &router,
            "RotateSecret",
            json!({
                "SecretId": name,
                "RotationLambdaARN": LAMBDA_ARN,
                "RotationRules": {"ScheduleExpression": expression},
                "RotateImmediately": false,
            }),
            case,
        )
        .await;

        let described = describe(&router, &name, case).await;
        assert_eq!(
            described.get("RotationRules"),
            Some(&json!({"ScheduleExpression": expression})),
            "case {case:?}: expected the cron rules to round-trip, got {described}"
        );
        assert_eq!(
            described.get("RotationEnabled"),
            Some(&json!(true)),
            "case {case:?}: expected rotation to be enabled, got {described}"
        );
        let next = instant(&described, "NextRotationDate", case);
        assert!(
            next > before && next - before <= longest_gap + CLOCK_SLACK_SECONDS,
            "case {case:?}: expected the next occurrence of {expression} within \
             {longest_gap} seconds of the call at {before}, got {next}"
        );
    }
}

/// Rotation rules outside what the service's own rules allow are refused, and
/// nothing is configured.
#[tokio::test]
async fn rotation_rules_the_service_does_not_allow_are_refused() {
    // (case, the rules the call carries)
    let rows: &[(&str, Value)] = &[
        (
            "both halves of the schedule named at once",
            json!({"AutomaticallyAfterDays": 10, "ScheduleExpression": "rate(10 days)"}),
            // "you can set the rotation schedule in RotationRules with
            // AutomaticallyAfterDays or ScheduleExpression, but not both"
        ),
        (
            "a rate below the documented four-hour floor",
            json!({"ScheduleExpression": "rate(2 hours)"}),
        ),
        (
            "a rate one day past the documented thousand-day maximum",
            json!({"ScheduleExpression": "rate(1001 days)"}),
        ),
        (
            "a rate in a unit the service does not express intervals in",
            json!({"ScheduleExpression": "rate(10 fortnights)"}),
        ),
        (
            "a rate with no unit at all",
            json!({"ScheduleExpression": "rate(10)"}),
        ),
        (
            "an expression that is neither a rate nor a cron",
            json!({"ScheduleExpression": "every 10 days"}),
        ),
        (
            "a cron expression one field short of the six documented",
            json!({"ScheduleExpression": "cron(0 8 1 * ?)"}),
        ),
        (
            "a cron expression one field past the six documented",
            json!({"ScheduleExpression": "cron(0 8 1 * ? * *)"}),
        ),
        (
            "a cron expression with no fields at all",
            json!({"ScheduleExpression": "cron()"}),
        ),
    ];

    let router = emulator();
    let name = "unschedulable";
    create_secret(&router, name, "the-current-value", "setup").await;

    for (case, rules) in rows {
        let code = err(
            &router,
            "RotateSecret",
            json!({
                "SecretId": name,
                "RotationLambdaARN": LAMBDA_ARN,
                "RotationRules": rules,
            }),
            case,
        )
        .await;
        assert_eq!(
            code, "InvalidParameterException",
            "case {case:?}: expected InvalidParameterException for rules {rules}, got {code}"
        );
        let described = describe(&router, name, case).await;
        assert_eq!(
            described.get("RotationEnabled"),
            Some(&json!(false)),
            "case {case:?}: expected a refused call to configure nothing, got {described}"
        );
    }

    // The modelled range refuses the same interval expressed in days outside
    // 1..1000 at the boundary, before the handler runs
    // (`AutomaticallyRotateAfterDaysType`, range 1..1000).
    for (case, days) in [("one below the modelled minimum", 0), ("one past the modelled maximum", 1001)] {
        let (status, parsed) = call(
            &router,
            "RotateSecret",
            &json!({
                "SecretId": name,
                "RotationLambdaARN": LAMBDA_ARN,
                "RotationRules": {"AutomaticallyAfterDays": days},
            }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected AutomaticallyAfterDays {days} to be refused, got {status} \
             with body {parsed}"
        );
        let code = error_code(&parsed, case);
        assert!(
            matches!(code.as_str(), "ValidationException" | "InvalidParameterException"),
            "case {case:?}: expected a constraint refusal for AutomaticallyAfterDays {days}, got \
             {code}"
        );
    }
}

/// A started rotation stakes `AWSPENDING` on the version id it reports, leaves
/// the current value exactly where it was, and is the rotation a later
/// `RotateSecret` call is refused for while it has not finished.
///
/// The rows differ along the form of the `SecretId` and along who chose the new
/// version's id — the caller through `ClientRequestToken`, or the service.
#[tokio::test]
async fn a_started_rotation_stages_awspending_and_leaves_awscurrent_alone() {
    /// How a row names the secret it rotates.
    #[derive(Debug, Clone, Copy)]
    enum Id {
        Name,
        FullArn,
        PartialArn,
    }

    // (case, how the secret is named, the ClientRequestToken the call carries)
    let rows: &[(&str, Id, Option<&str>)] = &[
        ("named by name, version id minted by the service", Id::Name, None),
        (
            "named by full ARN, version id chosen by the caller",
            Id::FullArn,
            Some("11111111-1111-4111-8111-111111111111"),
        ),
        (
            "named by partial ARN, version id minted by the service",
            Id::PartialArn,
            None,
        ),
        (
            "named by name, version id chosen by the caller at the modelled 32-character minimum",
            Id::Name,
            Some("01234567890123456789012345678901"),
        ),
    ];

    let router = emulator();
    for (index, (case, id, token)) in rows.iter().enumerate() {
        let name = format!("rotating-{index}");
        let (arn, current) = create_secret(&router, &name, "the-current-value", case).await;
        let secret_id = match id {
            Id::Name => name.clone(),
            Id::FullArn => arn.clone(),
            Id::PartialArn => partial_arn(&arn),
        };

        let mut request = json!({"SecretId": secret_id, "RotationLambdaARN": LAMBDA_ARN});
        if let Some(token) = token {
            request["ClientRequestToken"] = json!(token);
        }
        let rotated = ok(&router, "RotateSecret", request.clone(), case).await;
        let pending = text(&rotated, "VersionId", case);
        if let Some(token) = token {
            assert_eq!(
                pending, *token,
                "case {case:?}: expected the caller's ClientRequestToken as the new version id, \
                 got {pending:?}"
            );
        }

        // The staging labels: AWSPENDING on the version the rotation will
        // create, AWSCURRENT still on the stored one.
        let described = describe(&router, &secret_id, case).await;
        assert_eq!(
            stages_of(&described, "VersionIdsToStages", &pending),
            vec!["AWSPENDING".to_string()],
            "case {case:?}: expected AWSPENDING on the rotation's version {pending}, got \
             {described}"
        );
        assert_eq!(
            stages_of(&described, "VersionIdsToStages", &current),
            vec!["AWSCURRENT".to_string()],
            "case {case:?}: expected AWSCURRENT to stay on version {current}, got {described}"
        );
        assert_eq!(
            described.get("LastRotatedDate"),
            None,
            "case {case:?}: expected no LastRotatedDate while the rotation has not finished, got \
             {described}"
        );

        // A reader during the rotation still gets the current value.
        let read = ok(&router, "GetSecretValue", json!({"SecretId": &secret_id}), case).await;
        assert_eq!(
            text(&read, "SecretString", case),
            "the-current-value",
            "case {case:?}: expected the current value to be unchanged, got {read}"
        );
        assert_eq!(
            text(&read, "VersionId", case),
            current,
            "case {case:?}: expected AWSCURRENT to resolve to version {current}, got {read}"
        );

        // The staged id names no version yet: "During rotation, Secrets Manager
        // creates an AWSPENDING version ID before creating the new secret
        // version. To check if a secret version exists, call GetSecretValue."
        let code = err(
            &router,
            "GetSecretValue",
            json!({"SecretId": &secret_id, "VersionStage": "AWSPENDING"}),
            case,
        )
        .await;
        assert_eq!(
            code, "ResourceNotFoundException",
            "case {case:?}: expected no readable AWSPENDING version yet, got {code}"
        );

        // "If the AWSPENDING staging label is present but not attached to the
        // same version as AWSCURRENT, then any later invocation of RotateSecret
        // assumes that a previous rotation request is still in progress and
        // returns an error."
        let code = err(&router, "RotateSecret", request.clone(), case).await;
        assert_eq!(
            code, "InvalidRequestException",
            "case {case:?}: expected a second rotation to be refused while one is in progress, \
             got {code}"
        );

        // The rotation function's own createSecret step, which puts the new
        // value under the staged id: the label moves onto the version that now
        // exists, and AWSCURRENT is still untouched.
        ok(
            &router,
            "PutSecretValue",
            json!({
                "SecretId": &secret_id,
                "ClientRequestToken": &pending,
                "SecretString": "the-next-value",
                "VersionStages": ["AWSPENDING"],
            }),
            case,
        )
        .await;
        let staged = ok(
            &router,
            "GetSecretValue",
            json!({"SecretId": &secret_id, "VersionStage": "AWSPENDING"}),
            case,
        )
        .await;
        assert_eq!(
            text(&staged, "SecretString", case),
            "the-next-value",
            "case {case:?}: expected the staged version to hold the new value, got {staged}"
        );
        assert_eq!(
            text(&staged, "VersionId", case),
            pending,
            "case {case:?}: expected the staged version to be {pending}, got {staged}"
        );
        let read = ok(&router, "GetSecretValue", json!({"SecretId": &secret_id}), case).await;
        assert_eq!(
            text(&read, "SecretString", case),
            "the-current-value",
            "case {case:?}: expected the current value to survive the staged one, got {read}"
        );
    }
}

/// `RotateImmediately` decides whether the call starts a rotation or only
/// schedules one: "The default for RotateImmediately is true. If you don't
/// specify this value, Secrets Manager rotates the secret immediately. If you
/// set RotateImmediately to false, Secrets Manager tests the rotation
/// configuration by running the testSecret step of the Lambda rotation function.
/// This test creates an AWSPENDING version of the secret and then removes it"
/// (`RotateSecretRequest$RotateImmediately`, this service's Smithy model) — a
/// test that leaves no version staged.
#[tokio::test]
async fn rotate_immediately_decides_whether_a_rotation_is_started() {
    // (case, the RotateImmediately the call carries, whether it stages a version)
    let rows: &[(&str, Option<bool>, bool)] = &[
        ("absent, which the service reads as true", None, true),
        ("true", Some(true), true),
        ("false", Some(false), false),
    ];

    let router = emulator();
    for (index, (case, rotate_immediately, stages)) in rows.iter().enumerate() {
        let name = format!("immediacy-{index}");
        let (_, current) = create_secret(&router, &name, "the-current-value", case).await;

        let mut request = json!({
            "SecretId": name,
            "RotationLambdaARN": LAMBDA_ARN,
            "RotationRules": {"AutomaticallyAfterDays": 7},
        });
        if let Some(rotate_immediately) = rotate_immediately {
            request["RotateImmediately"] = json!(rotate_immediately);
        }
        let rotated = ok(&router, "RotateSecret", request.clone(), case).await;
        let version_id = text(&rotated, "VersionId", case);

        let described = describe(&router, &name, case).await;
        let staged = stages_of(&described, "VersionIdsToStages", &version_id);
        if *stages {
            assert_eq!(
                staged,
                vec!["AWSPENDING".to_string()],
                "case {case:?}: expected the rotation to stage AWSPENDING, got {described}"
            );
        } else {
            assert!(
                staged.is_empty(),
                "case {case:?}: expected no version staged, got {staged:?} in {described}"
            );
        }
        assert_eq!(
            stages_of(&described, "VersionIdsToStages", &current),
            vec!["AWSCURRENT".to_string()],
            "case {case:?}: expected AWSCURRENT to stay on version {current}, got {described}"
        );
        // Either way the schedule is configured, so rotation is enabled.
        assert_eq!(
            described.get("RotationEnabled"),
            Some(&json!(true)),
            "case {case:?}: expected rotation to be enabled, got {described}"
        );

        // With nothing staged there is no rotation in progress, so the next
        // call is accepted rather than refused.
        let (status, parsed) = call(&router, "RotateSecret", &request).await;
        let expected = if *stages { 400 } else { 200 };
        assert_eq!(
            status, expected,
            "case {case:?}: expected a repeat call to answer {expected}, got {status} with body \
             {parsed}"
        );
    }
}

/// A `RotateSecret` call that names no configuration rotates with "the values
/// already stored in the secret", and one that names part of it leaves the rest
/// alone (`RotateSecret`, this service's Smithy model).
#[tokio::test]
async fn rotation_configuration_a_call_omits_keeps_its_stored_value() {
    let router = emulator();
    let case = "a call naming only new rules";
    let name = "reconfigured";
    create_secret(&router, name, "the-current-value", case).await;

    // Configure the function and a schedule, without starting a rotation, so the
    // calls below are not refused as "a rotation is still in progress".
    ok(
        &router,
        "RotateSecret",
        json!({
            "SecretId": name,
            "RotationLambdaARN": LAMBDA_ARN,
            "RotationRules": {"AutomaticallyAfterDays": 7},
            "RotateImmediately": false,
        }),
        case,
    )
    .await;

    // A second call naming only the rules keeps the stored function, and the
    // next due instant follows the new schedule.
    let before = epoch_seconds(SystemTime::now());
    ok(
        &router,
        "RotateSecret",
        json!({
            "SecretId": name,
            "RotationRules": {"ScheduleExpression": "rate(6 hours)"},
            "RotateImmediately": false,
        }),
        case,
    )
    .await;
    let described = describe(&router, name, case).await;
    assert_eq!(
        text(&described, "RotationLambdaARN", case),
        LAMBDA_ARN,
        "case {case:?}: expected the stored rotation function to survive, got {described}"
    );
    assert_eq!(
        described.get("RotationRules"),
        Some(&json!({"ScheduleExpression": "rate(6 hours)"})),
        "case {case:?}: expected the new rules to replace the stored ones, got {described}"
    );
    let next = instant(&described, "NextRotationDate", case);
    assert!(
        (next - (before + 6.0 * HOUR)).abs() <= CLOCK_SLACK_SECONDS,
        "case {case:?}: expected NextRotationDate six hours out, got {next}"
    );

    // A third call naming nothing at all rotates with what is stored, so the
    // schedule is unchanged and the rotation is started.
    let case = "a call naming no configuration at all";
    let rotated = ok(&router, "RotateSecret", json!({"SecretId": name}), case).await;
    let described = describe(&router, name, case).await;
    assert_eq!(
        described.get("RotationRules"),
        Some(&json!({"ScheduleExpression": "rate(6 hours)"})),
        "case {case:?}: expected the stored rules to survive, got {described}"
    );
    assert_eq!(
        stages_of(
            &described,
            "VersionIdsToStages",
            &text(&rotated, "VersionId", case)
        ),
        vec!["AWSPENDING".to_string()],
        "case {case:?}: expected the rotation to be started, got {described}"
    );
}

/// A managed external secret rotates through a partner role and metadata rather
/// than a Lambda function: "The metadata needed to successfully rotate a managed
/// external secret" and "the ARN of the role that allows Secrets Manager to
/// rotate a secret held by a third-party partner"
/// (`RotateSecretRequest$ExternalSecretRotationMetadata` / `$…RoleArn`, this
/// service's Smithy model).
#[tokio::test]
async fn external_rotation_metadata_and_role_round_trip() {
    // (case, the metadata items the call carries)
    let rows: &[(&str, Value)] = &[
        ("one metadata item", json!([{"Key": "tenant", "Value": "acme"}])),
        (
            "several metadata items",
            json!([
                {"Key": "tenant", "Value": "acme"},
                {"Key": "endpoint", "Value": "https://partner.example/rotate"},
            ]),
        ),
        ("no metadata at all", json!([])),
    ];
    let role_arn = "arn:aws:iam::123456789012:role/SecretsManagerExternalRotation";

    let router = emulator();
    for (index, (case, metadata)) in rows.iter().enumerate() {
        let name = format!("external-{index}");
        create_secret(&router, &name, "the-current-value", case).await;

        ok(
            &router,
            "RotateSecret",
            json!({
                "SecretId": name,
                "ExternalSecretRotationRoleArn": role_arn,
                "ExternalSecretRotationMetadata": metadata,
                "RotateImmediately": false,
            }),
            case,
        )
        .await;

        let described = describe(&router, &name, case).await;
        assert_eq!(
            text(&described, "ExternalSecretRotationRoleArn", case),
            role_arn,
            "case {case:?}: expected the rotation role to be persisted, got {described}"
        );
        let reported = described.get("ExternalSecretRotationMetadata");
        let expected = metadata.as_array().filter(|items| !items.is_empty());
        match expected {
            // `ExternalSecretRotationMetadataType` carries the items as given.
            Some(_) => assert_eq!(
                reported,
                Some(metadata),
                "case {case:?}: expected the metadata to round-trip, got {described}"
            ),
            None => assert!(
                reported.is_none() || reported == Some(&json!([])),
                "case {case:?}: expected no metadata to be reported, got {described}"
            ),
        }
        assert_eq!(
            described.get("RotationLambdaARN"),
            None,
            "case {case:?}: expected no rotation function for a managed external secret, got \
             {described}"
        );
    }
}

/// `AWSPENDING` names at most one version of a secret at a time, whichever id
/// the value that claims it is put under.
///
/// The rows differ along the id the rotation function uses for the new version:
/// the one the rotation staked, or another one it chose itself.
#[tokio::test]
async fn awspending_names_one_version_at_a_time() {
    // (case, whether the put reuses the staked id)
    let rows: &[(&str, bool)] = &[
        ("the value put under the id the rotation staked", true),
        ("the value put under an id of the put's own", false),
    ];

    let router = emulator();
    for (index, (case, reuse_staked_id)) in rows.iter().enumerate() {
        let name = format!("one-pending-{index}");
        let (_, current) = create_secret(&router, &name, "the-current-value", case).await;
        let rotated = ok(
            &router,
            "RotateSecret",
            json!({"SecretId": name, "RotationLambdaARN": LAMBDA_ARN}),
            case,
        )
        .await;
        let staked = text(&rotated, "VersionId", case);
        let put_under = if *reuse_staked_id {
            staked.clone()
        } else {
            format!("2222222222222222222222222222222{index}")
        };

        ok(
            &router,
            "PutSecretValue",
            json!({
                "SecretId": name,
                "ClientRequestToken": &put_under,
                "SecretString": "the-next-value",
                "VersionStages": ["AWSPENDING"],
            }),
            case,
        )
        .await;

        let described = describe(&router, &name, case).await;
        assert_eq!(
            ids_with_stage(&described, "VersionIdsToStages", "AWSPENDING"),
            vec![put_under.clone()],
            "case {case:?}: expected AWSPENDING on version {put_under} alone, got {described}"
        );
        assert_eq!(
            ids_with_stage(&described, "VersionIdsToStages", "AWSCURRENT"),
            vec![current.clone()],
            "case {case:?}: expected AWSCURRENT to stay on version {current}, got {described}"
        );
    }

    // The other ordering: the label is already on a version when the rotation
    // starts. "When rotation is successful, the AWSPENDING staging label might be
    // attached to the same version as the AWSCURRENT version" — from that shape a
    // fresh rotation is allowed, and staking its new version id has to leave the
    // label naming one version.
    let case = "a rotation started where AWSPENDING sits on the AWSCURRENT version";
    let name = "pending-on-current";
    create_secret(&router, name, "the-first-value", case).await;
    let finished = ok(
        &router,
        "PutSecretValue",
        json!({
            "SecretId": name,
            "SecretString": "the-rotated-value",
            "VersionStages": ["AWSCURRENT", "AWSPENDING"],
        }),
        case,
    )
    .await;
    let finished = text(&finished, "VersionId", case);
    let described = describe(&router, name, case).await;
    assert_eq!(
        ids_with_stage(&described, "VersionIdsToStages", "AWSPENDING"),
        vec![finished.clone()],
        "case {case:?}: expected the setup to leave AWSPENDING on {finished}, got {described}"
    );
    let rotated = ok(
        &router,
        "RotateSecret",
        json!({"SecretId": name, "RotationLambdaARN": LAMBDA_ARN}),
        case,
    )
    .await;
    let staked = text(&rotated, "VersionId", case);
    let described = describe(&router, name, case).await;
    assert_eq!(
        ids_with_stage(&described, "VersionIdsToStages", "AWSPENDING"),
        vec![staked.clone()],
        "case {case:?}: expected AWSPENDING to move to the staked version {staked} alone, got \
         {described}"
    );
    assert_eq!(
        ids_with_stage(&described, "VersionIdsToStages", "AWSCURRENT"),
        vec![finished.clone()],
        "case {case:?}: expected AWSCURRENT to stay on version {finished}, got {described}"
    );
}

/// The requests `RotateSecret` refuses, each with the error the model names for
/// it.
#[tokio::test]
async fn rotate_secret_refusals() {
    let router = emulator();

    // "Secrets Manager can't find the resource that you asked for."
    let case = "a secret that does not exist";
    let code = err(
        &router,
        "RotateSecret",
        json!({"SecretId": "no-such-secret", "RotationLambdaARN": LAMBDA_ARN}),
        case,
    )
    .await;
    assert_eq!(
        code, "ResourceNotFoundException",
        "case {case:?}: expected ResourceNotFoundException, got {code}"
    );

    // "You tried to enable rotation on a secret that doesn't already have a
    // Lambda function ARN configured and you didn't include such an ARN as a
    // parameter in this call."
    create_secret(
        &router,
        "unconfigured",
        "the-current-value",
        "a secret with no rotation function",
    )
    .await;
    for (case, request) in [
        (
            "no rotation function named and none stored",
            json!({"SecretId": "unconfigured"}),
        ),
        (
            "an empty rotation function ARN, which names no function",
            json!({"SecretId": "unconfigured", "RotationLambdaARN": ""}),
        ),
    ] {
        let code = err(&router, "RotateSecret", request, case).await;
        assert_eq!(
            code, "InvalidRequestException",
            "case {case:?}: expected InvalidRequestException, got {code}"
        );
        let described = describe(&router, "unconfigured", case).await;
        assert_eq!(
            described.get("RotationEnabled"),
            Some(&json!(false)),
            "case {case:?}: expected the refused call to enable nothing, got {described}"
        );
    }

    // "The secret is scheduled for deletion."
    let case = "a secret scheduled for deletion";
    create_secret(&router, "doomed", "the-current-value", case).await;
    ok(
        &router,
        "DeleteSecret",
        json!({"SecretId": "doomed", "RecoveryWindowInDays": 7}),
        case,
    )
    .await;
    let code = err(
        &router,
        "RotateSecret",
        json!({"SecretId": "doomed", "RotationLambdaARN": LAMBDA_ARN}),
        case,
    )
    .await;
    assert_eq!(
        code, "InvalidRequestException",
        "case {case:?}: expected InvalidRequestException for a deleted secret, got {code}"
    );

    // `ClientRequestToken` is the id of the NEW version, so a token that already
    // identifies a stored version of the secret cannot be it — whichever version
    // it names.
    for (case, stage) in [
        ("a token that is already the current version's id", "AWSCURRENT"),
        (
            "a token that is already a deprecated version's id",
            "AWSPREVIOUS",
        ),
    ] {
        let name = "token-collision";
        if stage == "AWSCURRENT" {
            create_secret(&router, name, "the-first-value", case).await;
            // A second value demotes the first to AWSPREVIOUS, so both rows have
            // a stored version to collide with.
            ok(
                &router,
                "PutSecretValue",
                json!({"SecretId": name, "SecretString": "the-second-value"}),
                case,
            )
            .await;
        }
        let existing = ok(
            &router,
            "GetSecretValue",
            json!({"SecretId": name, "VersionStage": stage}),
            case,
        )
        .await;
        let existing = text(&existing, "VersionId", case);
        let code = err(
            &router,
            "RotateSecret",
            json!({
                "SecretId": name,
                "RotationLambdaARN": LAMBDA_ARN,
                "ClientRequestToken": &existing,
            }),
            case,
        )
        .await;
        assert_eq!(
            code, "InvalidRequestException",
            "case {case:?}: expected InvalidRequestException for the existing version id \
             {existing}, got {code}"
        );
        let described = describe(&router, name, case).await;
        assert!(
            ids_with_stage(&described, "VersionIdsToStages", "AWSPENDING").is_empty(),
            "case {case:?}: expected the refused call to stage nothing, got {described}"
        );
        assert_eq!(
            described.get("RotationEnabled"),
            Some(&json!(false)),
            "case {case:?}: expected the refused call to enable nothing, got {described}"
        );
    }
}
