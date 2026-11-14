// SPDX-License-Identifier: CC0-1.0

//! The STAGING LADDER a rotation climbs, and what a reader sees while it climbs: Secrets
//! Manager and Lambda on ONE engine, the shape `core/router/src/lib.rs` mounts them in.
//!
//! The rules these tables assert:
//!
//! * **`AWSPENDING` is staked before `AWSCURRENT` moves, and it is staked on ONE version for
//!   the whole rotation.** "During rotation, Secrets Manager creates an AWSPENDING version ID
//!   before creating the new secret version" and `AWSPENDING` "indicates the version of the
//!   secret that contains new secret information that will become the next current version
//!   when rotation finishes" (`DescribeSecretResponse$VersionIdsToStages`, this service's
//!   Smithy model) — so at every step Secrets Manager invokes the rotation function for
//!   (`createSecret`, `setSecret`, `testSecret`, `finishSecret`), the version the rotation is
//!   creating already carries `AWSPENDING` and does not yet carry `AWSCURRENT`, and the
//!   `ClientRequestToken` the event names that version by is the same one at each of the four
//!   steps. Only the last step moves `AWSCURRENT` onto it, after which "the AWSPENDING staging
//!   label might be attached to the same version as the AWSCURRENT version, or it might not be
//!   attached to any version" (`RotateSecret`, this service's Smithy model) — never to a
//!   version other than the current one, which is the state that reads as a rotation still in
//!   progress.
//! * **A reader never sees a half-written current credential.** A staging label "is attached
//!   to only one version at a time" and a version's value is written whole by the call that
//!   creates it, so `GetSecretValue` for `AWSCURRENT` is served the complete value of whichever
//!   version holds the label at that moment: the pre-rotation one until `finishSecret` moves
//!   the label, the rotated one after. Each read is therefore a complete, usable credential,
//!   and the revisions a reader walks through never go backwards.
//!
//! Each step's view of the ladder is recorded BY THE ROTATION FUNCTION, through this service's
//! own operations, as the step's own tag: the token the event carried and the staging labels
//! the secret reported on it at that moment. That is what makes the ladder observable without
//! racing the rotation — the observation is taken inside the step, by the only party that knows
//! the step is running.
//!
//! The rows span what the rules generalise over rather than one recorded request: whether the
//! rotation's token is the caller's or one this service minted, how large the credential being
//! rotated is (a torn read of a long value is what a short one cannot show), and how many
//! rotations the secret has been through — a secret rotated again climbs the same ladder under
//! a token of its own.
//!
//! Every wait below looks for the STATE the rotation reaches rather than for a span to pass,
//! and the ceiling on the looking is derived from the bounds Lambda publishes for one
//! invocation, so it bounds the suite only so it cannot hang.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use base64::Engine as _;
use serde_json::{json, Value};
use service_com_amazonaws_lambda_awsgirapiservice::state::execution::{
    ABANDON_GRACE, HOST_WAIT_CEILING, INIT_PHASE_LIMIT, PERMIT_WAIT,
};
use service_com_amazonaws_lambda_awsgirapiservice::state::STATE_ACTIVE;
use service_com_amazonaws_secretsmanager_secretsmanager::state::rotation::ROTATION_STEPS;
use smithy_server_runtime::MultiServiceRouter;

use crate::secret_client_request_token::fresh_token;
use crate::secret_rotation_driver::package;

/// The Region every request in this suite signs for.
const REGION: &str = "us-west-2";

/// The service principal Secrets Manager invokes a rotation function as.
const SERVICE_PRINCIPAL: &str = "secretsmanager.amazonaws.com";

/// The IAM action an invocation of a function performs.
const INVOKE_ACTION: &str = "lambda:InvokeFunction";

/// A well-formed execution role ARN for the rotation function.
const ROLE: &str = "arn:aws:iam::000000000000:role/service-role/rotation-exec";

/// The handler entry point the deployment package below declares.
const HANDLER: &str = "lambda_function.lambda_handler";

/// The staging label the version a secret currently serves carries.
const CURRENT_STAGE: &str = "AWSCURRENT";

/// The staging label a rotation stakes on the version it is creating.
const PENDING_STAGE: &str = "AWSPENDING";

/// The user the credential documents in this suite name, which every complete value carries.
const USER: &str = "app";

/// The tag key prefix the rotation function records each step's view of the ladder under; the
/// step's own name follows it. The function's source writes the same literal.
const LADDER_TAG_PREFIX: &str = "ladder-";

/// The alphabet the rotation function draws each revision's password from, one letter per
/// revision. The function's source writes the same literal.
const ALPHABET: &str = "abcdefghijklmnopqrstuvwxyz";

/// How often this suite looks again while waiting for a state, and for a read of `AWSCURRENT`
/// to interleave with the rotation's own writes.
const LOOK_INTERVAL: Duration = Duration::from_millis(10);

/// The longest this suite waits for a rotation to reach the state it settles in.
///
/// DERIVED rather than picked: a rotation finishes only once Secrets Manager has invoked the
/// rotation function once per documented step, and Lambda may spend its own published bounds on
/// each of those invocations — the wait for capacity to run in, the initialization of the
/// execution environment, the longest the run itself is waited for and the grace past that
/// (`service_com_amazonaws_lambda_awsgirapiservice::state::execution`). A ceiling below that
/// worth of work is a bet on how fast the host runs; this one is above it.
const ROTATION_WAIT: Duration = PERMIT_WAIT
    .saturating_add(INIT_PHASE_LIMIT)
    .saturating_add(HOST_WAIT_CEILING)
    .saturating_add(ABANDON_GRACE)
    .saturating_mul(ROTATION_STEPS.len() as u32);

/// Secrets Manager and Lambda over ONE shared engine.
fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_secretsmanager_secretsmanager::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_lambda_awsgirapiservice::EmulatedService::new(engine),
    );
    router
}

/// A SigV4 `Authorization` whose credential scope names `service`, which is what routes the
/// request. The signature is never checked here.
fn authorization(service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=emulator-test-key/20260101/{REGION}/{service}/aws4_request, \
         SignedHeaders=host, Signature=not-verified"
    )
}

/// One Secrets Manager call (`awsJson1_1`), as its status and parsed body.
async fn sm(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("secretsmanager.{REGION}.amazonaws.com"))
        .header("authorization", authorization("secretsmanager"))
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("secretsmanager.{operation}"))
        .body(bytes::Bytes::from(body.to_string()))
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed: Value = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// The same, requiring the 200 the operation answers.
async fn sm_ok(router: &MultiServiceRouter, case: &str, operation: &str, body: &Value) -> Value {
    let (status, parsed) = sm(router, operation, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from {operation}, got {status} with {parsed}"
    );
    parsed
}

/// One Lambda `restJson1` call, as its status and parsed body.
async fn lambda(router: &MultiServiceRouter, method: &str, uri: &str, body: &Value) -> (u16, Value) {
    let payload = if body.is_null() {
        bytes::Bytes::new()
    } else {
        bytes::Bytes::from(body.to_string())
    };
    let request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("lambda.{REGION}.amazonaws.com"))
        .header("authorization", authorization("lambda"))
        .header("content-type", "application/json")
        .body(payload)
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed: Value = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// The rotation function this suite runs: the four steps AWS invokes a rotation function with,
/// each one driven through this service's own operations, and — before it does the step's own
/// work — the step's view of the ladder recorded as a tag: the token the event carried and the
/// staging labels the secret reports on that version right then.
///
/// The value it stages is a whole credential document: the same user, the next revision, and a
/// password of one repeated letter as long as the one it replaces. A torn read is therefore not
/// something a reader has to guess at — it is a document that does not parse, or one whose
/// password is the wrong length or not of one letter.
const ROTATION_FUNCTION: &str = r#"
import json

import boto3

LADDER_TAG_PREFIX = "ladder-"
ALPHABET = "abcdefghijklmnopqrstuvwxyz"


def lambda_handler(event, context):
    arn = event["SecretId"]
    token = event["ClientRequestToken"]
    step = event["Step"]
    client = boto3.client("secretsmanager")
    metadata = client.describe_secret(SecretId=arn)
    versions = metadata["VersionIdsToStages"]
    client.tag_resource(
        SecretId=arn,
        Tags=[{
            "Key": LADDER_TAG_PREFIX + step,
            "Value": json.dumps({"token": token, "stages": sorted(versions.get(token, []))}),
        }],
    )
    if step == "createSecret":
        current = client.get_secret_value(SecretId=arn, VersionStage="AWSCURRENT")
        document = json.loads(current["SecretString"])
        revision = int(document["revision"]) + 1
        letter = ALPHABET[revision % len(ALPHABET)]
        client.put_secret_value(
            SecretId=arn,
            ClientRequestToken=token,
            SecretString=json.dumps({
                "user": document["user"],
                "revision": revision,
                "password": letter * len(document["password"]),
            }),
            VersionStages=["AWSPENDING"],
        )
    elif step == "setSecret":
        pass
    elif step == "testSecret":
        json.loads(client.get_secret_value(SecretId=arn, VersionId=token)["SecretString"])
    elif step == "finishSecret":
        previous = None
        for version, stages in versions.items():
            if "AWSCURRENT" in stages:
                previous = version
        client.update_secret_version_stage(
            SecretId=arn,
            VersionStage="AWSCURRENT",
            MoveToVersionId=token,
            RemoveFromVersionId=previous,
        )
    else:
        raise ValueError("Invalid step parameter %s" % step)
    return {"step": step}
"#;

/// Create the rotation function `name`, grant this service the invocation a rotation cannot
/// start without, wait for the function to report itself invocable, and answer its ARN.
async fn create_function(router: &MultiServiceRouter, case: &str, name: &str) -> String {
    let (status, created) = lambda(
        router,
        "POST",
        "/2015-03-31/functions",
        &json!({
            "FunctionName": name,
            "Role": ROLE,
            "Runtime": "python3.12",
            "Handler": HANDLER,
            "Timeout": 10,
            "Code": {
                "ZipFile": base64::engine::general_purpose::STANDARD
                    .encode(package(ROTATION_FUNCTION)),
            },
        }),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected HTTP 201 from CreateFunction, got {status} with {created}"
    );
    let (status, granted) = lambda(
        router,
        "POST",
        &format!("/2015-03-31/functions/{name}/policy"),
        &json!({
            "StatementId": "rotation-invoke",
            "Action": INVOKE_ACTION,
            "Principal": SERVICE_PRINCIPAL,
        }),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected HTTP 201 from AddPermission, got {status} with {granted}"
    );
    // "The State [...] field in the response from GetFunctionConfiguration indicate[s] when the
    // function is ready to invoke": the CONDITION, rather than the provisioning window.
    let deadline = Instant::now() + ROTATION_WAIT;
    loop {
        let (status, configuration) = lambda(
            router,
            "GET",
            &format!("/2015-03-31/functions/{name}/configuration"),
            &Value::Null,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from GetFunctionConfiguration, got {status} with \
             {configuration}"
        );
        if configuration["State"].as_str() == Some(STATE_ACTIVE) {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: the function {name:?} never reported {STATE_ACTIVE}, got \
             {configuration}"
        );
        tokio::time::sleep(LOOK_INTERVAL).await;
    }
    created["FunctionArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a FunctionArn, got {created}"))
        .to_string()
}

/// The credential document revision `revision` of a secret whose password is
/// `password_length` characters long, as the rotation function writes it.
fn credential(revision: u64, password_length: usize) -> String {
    let letter = ALPHABET
        .chars()
        .nth(usize::try_from(revision % 26).unwrap_or_default())
        .expect("the alphabet holds 26 letters");
    json!({
        "user": USER,
        "revision": revision,
        "password": letter.to_string().repeat(password_length),
    })
    .to_string()
}

/// The revision of the complete credential document `value`, or why it is not one.
///
/// Complete means every part of the document is there and consistent: it parses, it names the
/// user it was created for, it carries a revision, and its password is as long as the secret's
/// passwords are and made of one repeated letter. A half-written value fails one of those.
fn credential_revision(value: &str, password_length: usize) -> Result<u64, String> {
    let parsed: Value = serde_json::from_str(value)
        .map_err(|error| format!("it does not parse as a JSON document ({error})"))?;
    if parsed["user"].as_str() != Some(USER) {
        return Err(format!("its user is {}", parsed["user"]));
    }
    let revision = parsed["revision"]
        .as_u64()
        .ok_or_else(|| format!("its revision is {}", parsed["revision"]))?;
    let password = parsed["password"]
        .as_str()
        .ok_or_else(|| format!("its password is {}", parsed["password"]))?;
    if password.chars().count() != password_length {
        return Err(format!(
            "its password is {} characters, not {password_length}",
            password.chars().count()
        ));
    }
    let letter = password
        .chars()
        .next()
        .ok_or_else(|| "its password is empty".to_string())?;
    if !password.chars().all(|held| held == letter) {
        return Err("its password is not one repeated letter".to_string());
    }
    Ok(revision)
}

/// What `DescribeSecret` reports for `secret_id`.
async fn described(router: &MultiServiceRouter, case: &str, secret_id: &str) -> Value {
    sm_ok(router, case, "DescribeSecret", &json!({"SecretId": secret_id})).await
}

/// The versions a `DescribeSecret` answer reports the staging label `stage` on.
fn staged_versions(described: &Value, stage: &str) -> Vec<String> {
    described["VersionIdsToStages"]
        .as_object()
        .map(|versions| {
            versions
                .iter()
                .filter(|(_, labels)| {
                    labels
                        .as_array()
                        .is_some_and(|labels| labels.iter().any(|label| label == stage))
                })
                .map(|(version, _)| version.clone())
                .collect()
        })
        .unwrap_or_default()
}

/// The value of the tag `key` in a `DescribeSecret` answer, when it carries one.
fn tag_value(described: &Value, key: &str) -> Option<String> {
    described["Tags"].as_array()?.iter().find_map(|tag| {
        (tag["Key"].as_str() == Some(key))
            .then(|| tag["Value"].as_str().unwrap_or_default().to_string())
    })
}

/// Wait until the rotation of `secret_id` onto the version `staked` has FINISHED —
/// `AWSCURRENT` on that version and `LastRotatedDate` stamped, which is what this service
/// writes once the last step has returned — and answer what `DescribeSecret` reported last.
async fn rotation_finished(
    router: &MultiServiceRouter,
    case: &str,
    secret_id: &str,
    staked: &str,
) -> Value {
    let deadline = Instant::now() + ROTATION_WAIT;
    loop {
        let described = described(router, case, secret_id).await;
        let current = staged_versions(&described, CURRENT_STAGE);
        if current == vec![staked.to_string()] && described["LastRotatedDate"].is_number() {
            return described;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: the rotation onto {staked:?} never finished, got {described}"
        );
        tokio::time::sleep(LOOK_INTERVAL).await;
    }
}

/// Read `AWSCURRENT` over and over until `finished` says the rotation has settled, and answer
/// every value that was served, in the order it was served.
///
/// Each read goes out while the rotation's own writes are landing — the steps run on a task of
/// their own, so the reads interleave with them — and each one is required to be SERVED: a
/// reader of the current value is never refused because a rotation is in flight.
async fn reads_during_rotation(
    router: &MultiServiceRouter,
    case: &str,
    secret_id: &str,
    finished: &AtomicBool,
) -> Vec<String> {
    let deadline = Instant::now() + ROTATION_WAIT;
    let mut served = Vec::new();
    loop {
        let (status, parsed) = sm(
            router,
            "GetSecretValue",
            &json!({"SecretId": secret_id, "VersionStage": CURRENT_STAGE}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected a reader of {CURRENT_STAGE} to be served during the \
             rotation, got {status} with {parsed}"
        );
        served.push(
            parsed["SecretString"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case {case:?}: expected a SecretString, got {parsed}")
                })
                .to_string(),
        );
        if finished.load(Ordering::SeqCst) || Instant::now() >= deadline {
            return served;
        }
        tokio::time::sleep(LOOK_INTERVAL).await;
    }
}

/// Rotate `secret_id` once and assert the whole ladder of that rotation.
///
/// `revision_before` is the revision the secret's current value holds going in, and
/// `password_length` how long its passwords are; `token` is the `ClientRequestToken` the caller
/// names, or `None` for a rotation whose token this service mints. Answers the version id the
/// rotation staked.
async fn climbs_the_ladder(
    router: &MultiServiceRouter,
    case: &str,
    secret_id: &str,
    function_arn: &str,
    revision_before: u64,
    password_length: usize,
    token: Option<String>,
) -> String {
    let mut request = json!({"SecretId": secret_id, "RotationLambdaARN": function_arn});
    if let Some(token) = &token {
        request["ClientRequestToken"] = json!(token);
    }
    let started = sm_ok(router, case, "RotateSecret", &request).await;
    let staked = started["VersionId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a VersionId, got {started}"))
        .to_string();
    if let Some(token) = &token {
        assert_eq!(
            &staked, token,
            "case {case:?}: expected the rotation to stake the token the call named, got \
             {staked:?}"
        );
    }

    // The rotation runs on a task of its own, so these two go out together: the look for the
    // state it settles in, and a reader of `AWSCURRENT` that keeps reading until it has.
    let settled = AtomicBool::new(false);
    let (described, served) = tokio::join!(
        async {
            let described = rotation_finished(router, case, secret_id, &staked).await;
            settled.store(true, Ordering::SeqCst);
            described
        },
        reads_during_rotation(router, case, secret_id, &settled),
    );

    // RULE: every read of `AWSCURRENT` during the rotation was served a COMPLETE credential —
    // the pre-rotation one or the rotated one, never a half-written document — and a reader
    // that has seen the rotated value never sees the old one again.
    assert!(
        !served.is_empty(),
        "case {case:?}: expected the reader to have read {CURRENT_STAGE} at least once"
    );
    let mut highest = revision_before;
    for (index, value) in served.iter().enumerate() {
        let revision = credential_revision(value, password_length).unwrap_or_else(|why| {
            panic!(
                "case {case:?}: read {index} of {CURRENT_STAGE} was not a complete credential: \
                 {why}"
            )
        });
        assert!(
            revision == revision_before || revision == revision_before + 1,
            "case {case:?}: read {index} of {CURRENT_STAGE} served revision {revision}, \
             expected {revision_before} or {}",
            revision_before + 1
        );
        assert!(
            revision >= highest,
            "case {case:?}: read {index} of {CURRENT_STAGE} served revision {revision} after \
             revision {highest}"
        );
        highest = revision;
    }

    // RULE: at every one of the four steps the version the rotation is creating already held
    // `AWSPENDING`, did not yet hold `AWSCURRENT`, and was named by the SAME token — which is
    // the view the function itself recorded from inside each step.
    for step in ROTATION_STEPS {
        let recorded = tag_value(&described, &format!("{LADDER_TAG_PREFIX}{step}"))
            .unwrap_or_else(|| {
                panic!(
                    "case {case:?}: the rotation function recorded no view of the ladder at the \
                     {step:?} step, got {described}"
                )
            });
        let recorded: Value = serde_json::from_str(&recorded).unwrap_or_else(|error| {
            panic!("case {case:?}: the {step:?} step recorded {recorded:?} ({error})")
        });
        assert_eq!(
            recorded["token"].as_str(),
            Some(staked.as_str()),
            "case {case:?}: expected the {step:?} step to carry the token {staked:?}, got \
             {recorded}"
        );
        let stages: Vec<&str> = recorded["stages"]
            .as_array()
            .map(|held| held.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default();
        assert_eq!(
            stages,
            vec![PENDING_STAGE],
            "case {case:?}: expected the version the rotation creates to hold only \
             {PENDING_STAGE} at the {step:?} step, got {stages:?}"
        );
    }

    // RULE: the last step is where `AWSCURRENT` moves. It now names the version the rotation
    // created, which serves the rotated credential whole, and `AWSPENDING` is left on no
    // version other than that one.
    assert_eq!(
        staged_versions(&described, CURRENT_STAGE),
        vec![staked.clone()],
        "case {case:?}: expected {CURRENT_STAGE} on the rotated version {staked:?}, got \
         {described}"
    );
    let pending = staged_versions(&described, PENDING_STAGE);
    assert!(
        pending.iter().all(|version| version == &staked),
        "case {case:?}: expected {PENDING_STAGE} on no version other than the current one once \
         the rotation finished, got {pending:?} in {described}"
    );
    // The other reader of the ladder agrees with it: "an array of staging labels that are
    // currently associated with this version of the secret"
    // (`SecretVersionsListEntry$VersionStages`, this service's Smithy model) names the rotated
    // version as the current one too.
    let listed = sm_ok(
        router,
        case,
        "ListSecretVersionIds",
        &json!({"SecretId": secret_id}),
    )
    .await;
    let entry = listed["Versions"]
        .as_array()
        .unwrap_or(&Vec::new())
        .iter()
        .find(|version| version["VersionId"].as_str() == Some(staked.as_str()))
        .cloned()
        .unwrap_or_else(|| {
            panic!("case {case:?}: expected {staked:?} among the listed versions, got {listed}")
        });
    let listed_stages: Vec<&str> = entry["VersionStages"]
        .as_array()
        .map(|held| held.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    assert!(
        listed_stages.contains(&CURRENT_STAGE),
        "case {case:?}: expected the listed version {staked:?} to carry {CURRENT_STAGE}, got \
         {listed_stages:?}"
    );

    let read = sm_ok(
        router,
        case,
        "GetSecretValue",
        &json!({"SecretId": secret_id, "VersionStage": CURRENT_STAGE}),
    )
    .await;
    let value = read["SecretString"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a SecretString, got {read}"));
    // Compared as DOCUMENTS rather than as text: the rotation function serialized this one,
    // and how a writer spaces a JSON document is no part of the credential it holds.
    let rotated: Value = serde_json::from_str(value).unwrap_or_else(|error| {
        panic!(
            "case {case:?}: the rotated value of {} characters does not parse ({error})",
            value.chars().count()
        )
    });
    let expected: Value = serde_json::from_str(&credential(revision_before + 1, password_length))
        .expect("the expected credential is a JSON document");
    assert_eq!(
        rotated["user"], expected["user"],
        "case {case:?}: expected the rotated credential to name the same user, got {}",
        rotated["user"]
    );
    assert_eq!(
        rotated["revision"], expected["revision"],
        "case {case:?}: expected the rotated credential to carry revision {}, got {}",
        expected["revision"], rotated["revision"]
    );
    assert_eq!(
        rotated["password"], expected["password"],
        "case {case:?}: expected the rotated credential's password to be {password_length} \
         characters of the letter revision {} draws, got {} characters starting {:?}",
        revision_before + 1,
        rotated["password"].as_str().unwrap_or_default().chars().count(),
        rotated["password"]
            .as_str()
            .unwrap_or_default()
            .chars()
            .take(4)
            .collect::<String>()
    );
    staked
}

/// One row of [`a_rotation_stages_awspending_at_every_step_before_awscurrent_moves`].
struct LadderRow {
    case: &'static str,
    /// Whether the caller names the `ClientRequestToken` the rotation runs under, or this
    /// service mints it.
    caller_named_token: bool,
    /// How long the password in the credential being rotated is.
    password_length: usize,
}

fn ladder_rows() -> Vec<LadderRow> {
    vec![
        LadderRow {
            case: "a caller-named rotation token over a short credential",
            caller_named_token: true,
            password_length: 16,
        },
        LadderRow {
            case: "a rotation token this service minted, over a long credential",
            caller_named_token: false,
            password_length: 4096,
        },
    ]
}

/// RULE: at every step of a rotation the version being created carries `AWSPENDING` and not
/// `AWSCURRENT`, under one token throughout; only the last step moves `AWSCURRENT`; and every
/// read of `AWSCURRENT` while that happens is served a complete credential.
///
/// The rows share one rotation function — an execution environment is built for one function
/// and serves the invocations after it — and each rotates a secret of its own.
#[tokio::test]
async fn a_rotation_stages_awspending_at_every_step_before_awscurrent_moves() {
    let router = router();
    let shared = "the rotation function the ladder rows share";
    let function_arn = create_function(&router, shared, "ladder-rotator").await;
    for (index, row) in ladder_rows().iter().enumerate() {
        let case = row.case;
        let name = format!("ladder-secret-{index}");
        sm_ok(
            &router,
            case,
            "CreateSecret",
            &json!({
                "Name": &name,
                "ClientRequestToken": fresh_token(),
                "SecretString": credential(0, row.password_length),
            }),
        )
        .await;
        let token = row.caller_named_token.then(fresh_token);
        climbs_the_ladder(
            &router,
            case,
            &name,
            &function_arn,
            0,
            row.password_length,
            token,
        )
        .await;
    }
}

/// RULE: the same ladder, a rotation later. A secret rotated again stakes `AWSPENDING` on a
/// version of its own, carries that token through all four steps, and moves `AWSCURRENT` only
/// at the last — so the rule holds of a rotation that is not the secret's first, and the two
/// rotations are told apart by the tokens they ran under.
#[tokio::test]
async fn a_second_rotation_climbs_the_same_ladder_under_its_own_token() {
    let case = "a secret rotated a second time";
    let router = router();
    let function_arn = create_function(&router, case, "second-rotation-rotator").await;
    let name = "twice-rotated-secret";
    let password_length = 32;
    sm_ok(
        &router,
        case,
        "CreateSecret",
        &json!({
            "Name": name,
            "ClientRequestToken": fresh_token(),
            "SecretString": credential(0, password_length),
        }),
    )
    .await;

    let first = climbs_the_ladder(
        &router,
        case,
        name,
        &function_arn,
        0,
        password_length,
        Some(fresh_token()),
    )
    .await;
    let second = climbs_the_ladder(
        &router,
        case,
        name,
        &function_arn,
        1,
        password_length,
        None,
    )
    .await;
    assert_ne!(
        first, second,
        "case {case:?}: expected the second rotation to stake a version of its own, got \
         {second:?} twice"
    );
}
