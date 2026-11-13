// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for how `DeleteS3TableIntegration` refuses an `Arn` that
//! addresses nothing.
//!
//! Every case drives the real wire path — the production `MultiServiceRouter`,
//! restJson1 serialization, the modelled-constraint walk and the handlers — through
//! the ONE harness this test crate has (`telemetry_rules`), so what is asserted is
//! what a signed AWS SDK request observes.
//!
//! The rule under test: a request whose `Arn` satisfies the modelled `ResourceArn`
//! constraints but resolves to no stored integration is refused as a NOT-FOUND — HTTP
//! 404, `ResourceNotFoundException`, the `x-amzn-errortype` header naming that error
//! type — while the modelled 400 `ValidationException` is kept for an `Arn` outside
//! `ResourceArn`'s own length and pattern. The two are different classes of failure:
//! a well-formed handle that addresses nothing is retried and branched on as a
//! missing resource, a malformed one as a client bug.
//!
//! The sentence the refusal carries is written out here rather than read from the
//! service's own constants, so a change to it is a failure in this file. Its two
//! variable parts come from the request: the integration identifier the ARN's
//! resource portion carries, and the account the request authenticates as.
//!
//! [`crate::s3_table_integration_listing`] asserts the same refusal retires nothing;
//! what is asserted here is its status, its error type and its wording.

use serde_json::{json, Value};
use service_com_amazonaws_observabilityadmin_observabilityadmin::state::s3_table_integration_arn;

use crate::telemetry_rules::{emulator, request, Emulator, ACCOUNT};

/// The region most requests below sign for.
const HOME_REGION: &str = "us-west-2";

/// A second region, so "resolves only within the caller's own region" can be shown to
/// be per-region rather than store-wide.
const OTHER_REGION: &str = "eu-west-1";

/// A role ARN every create below carries; it satisfies `ResourceArn`'s own modelled
/// `^arn:aws…` pattern.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/telemetry-s3-tables";

/// Everything an `arn:aws:observabilityadmin:<region>:<account>:s3tableintegration/`
/// ARN carries before the integration's own id, written out so a case can state the
/// exact ARN LENGTH the model bounds (`ResourceArn`, len 1..1011) rather than inferring
/// it.
fn integration_arn_prefix(region: &str) -> String {
    format!("arn:aws:observabilityadmin:{region}:{ACCOUNT}:s3tableintegration/")
}

/// Status, `x-amzn-errortype` header and parsed body of one call. The harness's own
/// `call` folds the header into the body; this keeps them apart, so the header can be
/// asserted on its own.
async fn call_with_headers(
    emulator: &Emulator,
    region: &str,
    uri: &str,
    body: &Value,
) -> (u16, Option<String>, Value) {
    let response = emulator.router.dispatch(request(region, uri, body)).await;
    let status = response.status().as_u16();
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let raw = response.into_body();
    let parsed: Value = if raw.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&raw).unwrap_or(Value::Null)
    };
    (status, error_type, parsed)
}

async fn delete(
    emulator: &Emulator,
    region: &str,
    arn: &str,
) -> (u16, Option<String>, Value) {
    call_with_headers(
        emulator,
        region,
        "/DeleteS3TableIntegration",
        &json!({ "Arn": arn }),
    )
    .await
}

async fn create(emulator: &Emulator, region: &str) -> String {
    let (status, _, created) = call_with_headers(
        emulator,
        region,
        "/CreateS3TableIntegration",
        &json!({ "Encryption": { "SseAlgorithm": "AES256" }, "RoleArn": ROLE_ARN }),
    )
    .await;
    assert_eq!(status, 200, "expected the setup create to answer 200, got {status} {created:?}");
    member(&created, "Arn")
}

/// One string member of a response, absent members read as a stand-in that can never
/// equal an expected value.
fn member(body: &Value, name: &str) -> String {
    body.get(name)
        .and_then(Value::as_str)
        .unwrap_or("<absent>")
        .to_string()
}

/// The error type the `x-amzn-errortype` header named, without the namespace a
/// restJson1 error type may carry.
fn header_error_type(error_type: &Option<String>) -> String {
    error_type
        .as_deref()
        .unwrap_or("<no x-amzn-errortype>")
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string()
}

/// A well-formed `Arn` that addresses no stored integration is a 404
/// `ResourceNotFoundException` naming the integration by the id the ARN carries and
/// the account that asked — whatever field of the ARN failed to resolve.
#[tokio::test]
async fn a_delete_of_an_unknown_integration_is_a_not_found_naming_the_requester() {
    let emulator = emulator();
    let home = create(&emulator, HOME_REGION).await;
    let elsewhere = create(&emulator, OTHER_REGION).await;
    let home_id = home.rsplit('/').next().unwrap_or_default().to_string();

    // The longest `Arn` the model admits (`ResourceArn` len 1..1011) that still names
    // an integration of this endpoint: the published upper bound, served rather than
    // refused, with the whole id in the sentence.
    let longest_id = "e".repeat(1011 - integration_arn_prefix(HOME_REGION).len());

    // The rows differ along what the rule generalises over: which field of the ARN
    // fails to resolve (region, account, partition, resource type, id), the region the
    // request is signed for, the LENGTH of the id (one character, 32, and the one that
    // takes the ARN to its modelled maximum), and the FORM of the resource portion
    // (type and id separated by `/`, separated by `:`, nested segments, and a bare
    // identifier with no resource type at all) — the id a refusal names is the
    // trailing segment of that portion either way.
    let rows: Vec<(&str, String, &str, String)> = vec![
        (
            "an id nothing was created under",
            s3_table_integration_arn(HOME_REGION, &"f".repeat(32)),
            HOME_REGION,
            "f".repeat(32),
        ),
        (
            "an id of one character",
            s3_table_integration_arn(HOME_REGION, "z"),
            HOME_REGION,
            "z".to_string(),
        ),
        (
            "an id that takes the ARN to its modelled 1011-character maximum",
            s3_table_integration_arn(HOME_REGION, &longest_id),
            HOME_REGION,
            longest_id.clone(),
        ),
        (
            "another region's integration",
            elsewhere.clone(),
            HOME_REGION,
            elsewhere
                .rsplit('/')
                .next()
                .unwrap_or_default()
                .to_string(),
        ),
        (
            "this endpoint's integration from another region",
            home.clone(),
            OTHER_REGION,
            home_id.clone(),
        ),
        (
            "an ARN naming this id in another region",
            s3_table_integration_arn(OTHER_REGION, &home_id),
            HOME_REGION,
            home_id.clone(),
        ),
        (
            "an ARN of another account",
            format!(
                "arn:aws:observabilityadmin:{HOME_REGION}:000000000001:s3tableintegration/\
                 {home_id}"
            ),
            HOME_REGION,
            home_id.clone(),
        ),
        (
            "an ARN of another partition",
            format!(
                "arn:aws-us-gov:observabilityadmin:{HOME_REGION}:{ACCOUNT}:s3tableintegration/\
                 {home_id}"
            ),
            HOME_REGION,
            home_id.clone(),
        ),
        (
            "an ARN of another resource type",
            format!("arn:aws:observabilityadmin:{HOME_REGION}:{ACCOUNT}:telemetry-rule/{home_id}"),
            HOME_REGION,
            home_id.clone(),
        ),
        (
            "a resource portion separated by a colon",
            format!(
                "arn:aws:observabilityadmin:{HOME_REGION}:{ACCOUNT}:s3tableintegration:{home_id}"
            ),
            HOME_REGION,
            home_id.clone(),
        ),
        (
            "a resource portion of nested segments",
            format!(
                "arn:aws:observabilityadmin:{HOME_REGION}:{ACCOUNT}:s3tableintegration/account/\
                 {home_id}"
            ),
            HOME_REGION,
            home_id.clone(),
        ),
        (
            "a bare resource portion carrying no resource type",
            format!("arn:aws:observabilityadmin:{HOME_REGION}:{ACCOUNT}:{home_id}"),
            HOME_REGION,
            home_id.clone(),
        ),
    ];

    for (case, arn, region, expected_id) in rows {
        let (status, error_type, response) = delete(&emulator, region, &arn).await;
        assert_eq!(
            status, 404,
            "case {case:?} ({arn}): expected 404, got {status} {response:?}"
        );
        assert_eq!(
            header_error_type(&error_type),
            "ResourceNotFoundException",
            "case {case:?}: expected the x-amzn-errortype header to name \
             ResourceNotFoundException, got {error_type:?}"
        );
        let expected_message =
            format!("Integration {expected_id} from requesterAccountId {ACCOUNT} not found");
        let message = member(&response, "Message");
        assert_eq!(
            message, expected_message,
            "case {case:?}: expected Message {expected_message:?}, got {message:?}"
        );
        // Only the two parts the service interpolates vary: the region the request was
        // signed for and the whole requested ARN stay out of the sentence.
        assert!(
            !message.contains(region) && !message.contains(&arn),
            "case {case:?}: expected neither {region:?} nor the requested ARN in the sentence, \
             got {message:?}"
        );
        assert_eq!(
            member(&response, "ResourceType"),
            "Integration",
            "case {case:?}: expected ResourceType \"Integration\", got {:?}",
            member(&response, "ResourceType")
        );
        assert_eq!(
            member(&response, "ResourceId"),
            arn,
            "case {case:?}: expected the requested identifier in ResourceId, got {:?}",
            member(&response, "ResourceId")
        );
    }

    // The integration the setup created is untouched by every refusal above: it still
    // deletes, and the SECOND delete of it is the same not-found refusal, which is what
    // makes an idempotent teardown work.
    let (status, _, deleted) = delete(&emulator, HOME_REGION, &home).await;
    assert_eq!(
        status, 204,
        "case {:?}: expected the first delete to answer 204, got {status} {deleted:?}",
        "delete twice"
    );
    let (status, error_type, response) = delete(&emulator, HOME_REGION, &home).await;
    assert_eq!(
        status, 404,
        "case {:?}: expected the second delete to answer 404, got {status} {response:?}",
        "delete twice"
    );
    assert_eq!(
        header_error_type(&error_type),
        "ResourceNotFoundException",
        "case {:?}: expected ResourceNotFoundException, got {error_type:?}",
        "delete twice"
    );
    let expected_message =
        format!("Integration {home_id} from requesterAccountId {ACCOUNT} not found");
    assert_eq!(
        member(&response, "Message"),
        expected_message,
        "case {:?}: expected Message {expected_message:?}, got {:?}",
        "delete twice",
        member(&response, "Message")
    );
}

/// An `Arn` outside `ResourceArn`'s own modelled length and pattern stays the modelled
/// 400 `ValidationException`: a malformed handle is a different failure from one that
/// is well formed and addresses nothing.
#[tokio::test]
async fn a_delete_of_a_malformed_arn_is_still_a_validation_failure() {
    let emulator = emulator();

    // `ResourceArn` is modelled len 1..1011, pattern
    // `^arn:aws([a-z0-9\-]+)?:([a-zA-Z0-9\-]+):([a-z0-9\-]+)?:([0-9]{12})?:(.+)$`
    // (smithy_query(query="operation",operation="DeleteS3TableIntegration")): each row
    // is outside one of those, including one character past the published upper bound.
    let past_the_maximum = format!(
        "{}{}",
        integration_arn_prefix(HOME_REGION),
        "g".repeat(1012 - integration_arn_prefix(HOME_REGION).len())
    );
    let rows: Vec<(&str, String)> = vec![
        ("empty, one character below the lower bound", String::new()),
        (
            "1012 characters, one past the upper bound",
            past_the_maximum,
        ),
        ("text that is no ARN at all", "not-an-arn".to_string()),
        (
            "an ARN with an empty resource portion, which the pattern excludes",
            format!("arn:aws:observabilityadmin:{HOME_REGION}:{ACCOUNT}:"),
        ),
        (
            "an ARN of another cloud, which the pattern excludes",
            format!("arn:gcp:observabilityadmin:{HOME_REGION}:{ACCOUNT}:s3tableintegration/x"),
        ),
        (
            "an account segment that is not twelve digits, which the pattern excludes",
            format!("arn:aws:observabilityadmin:{HOME_REGION}:123:s3tableintegration/x"),
        ),
    ];

    for (case, arn) in rows {
        let (status, error_type, response) = delete(&emulator, HOME_REGION, &arn).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} {response:?}"
        );
        assert_eq!(
            header_error_type(&error_type),
            "ValidationException",
            "case {case:?}: expected the x-amzn-errortype header to name \
             ValidationException, got {error_type:?}"
        );
    }
}
