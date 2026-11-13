// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the sentence this service refuses with when an ARN a
//! request supplied names no stored configuration manager.
//!
//! Every case drives the real wire path — the production `MultiServiceRouter`,
//! restJson1 serialization, the modelled-constraint walk and the handlers — so what is
//! asserted is what a signed AWS SDK request observes.
//!
//! The rule under test: an operation that resolves a configuration-manager ARN its
//! request carried and finds none stored under it answers the modelled
//! `ResourceNotFoundException` (HTTP 404) whose `Message` is the service's not-found
//! sentence — the fixed text `Quick setup manager with arn: ` and ` could not be found.`
//! around the ARN THE REQUEST carried, echoed verbatim. Nothing else about the sentence
//! varies: no account id, Region or uuid of any one run is part of it, so each row below
//! supplies a different ARN and expects that same ARN back inside the same fixed text.
//!
//! The operations here are every one that resolves such an ARN and states that sentence,
//! however the request carried it: `ListConfigurations` takes it as a body member, and the
//! manager's own `/configurationManager/{ManagerArn}` and
//! `/configurationDefinition/{ManagerArn}/{Id}` carry one as a path LABEL. Where the ARN
//! sat is no part of the answer: each operation resolves the ARN it was given in its own
//! handler, so every row below earns the same modelled shape, status and sentence. The tag
//! plane's three operations resolve the same resource but state fixed sentences of their
//! own, which is what the rows at the end require of them
//! (`tests/all/target_authorization.rs` holds that rule over every route).

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_ssmquicksetup_quicksetup::state::QUICK_SETUP_TYPE_CATALOG;
use smithy_server_runtime::MultiServiceRouter;

/// The region every request below signs for.
const REGION: &str = "us-west-2";

/// One emulator with an empty store, mounted exactly as the router mounts it.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_ssmquicksetup_quicksetup::EmulatedService::new(engine),
    );
    router
}

/// A request signed for `ssm-quicksetup` in [`REGION`], the signal the router routes on.
fn request(method: &str, uri: &str, body: Option<&Value>) -> http::Request<bytes::Bytes> {
    let mut builder = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("ssm-quicksetup.{REGION}.amazonaws.com"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDQUICKSETUP/20260101/{REGION}/ssm-quicksetup/aws4_request, \
                 SignedHeaders=host, Signature=0000"
            ),
        );
    let payload = match body {
        Some(value) => {
            builder = builder.header("content-type", "application/json");
            bytes::Bytes::from(serde_json::to_vec(value).expect("serialize the request body"))
        }
        None => bytes::Bytes::new(),
    };
    builder.body(payload).expect("a well-formed test request")
}

/// Status and parsed body of one call. An empty body reads as `Value::Null`.
async fn call(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<&Value>,
) -> (u16, Value) {
    let response = router.dispatch(request(method, uri, body)).await;
    let status = response.status().as_u16();
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let raw = response.into_body();
    let mut parsed: Value = if raw.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&raw)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&raw).to_string()))
    };
    // restJson1 carries the error shape in `__type`, and the framework also sets the
    // `x-amzn-errortype` header; fold the header in so one accessor reads either.
    if let (Some(error_type), Value::Object(map)) = (error_type, &mut parsed) {
        map.entry("__type")
            .or_insert_with(|| Value::String(error_type));
    }
    (status, parsed)
}

/// The modelled error shape a failed call reported.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        // An error `__type` may be namespaced (`com.amazonaws.ssmquicksetup#Foo`).
        .rsplit('#')
        .next()
        .unwrap_or("<no __type>")
        .to_string()
}

/// The `Message` a refusal carried, read through either casing restJson1 may render an
/// error structure's single message member under.
fn error_message(body: &Value) -> String {
    ["Message", "message"]
        .iter()
        .filter_map(|key| body.get(*key))
        .filter_map(Value::as_str)
        .next()
        .unwrap_or("<no Message>")
        .to_string()
}

/// Percent-encode a value for use as one `@httpLabel` path segment, which is what an
/// SDK does: a non-greedy label may not carry a raw `/`.
fn label(value: &str) -> String {
    value
        .chars()
        .map(|c| match c {
            '/' => "%2F".to_string(),
            ' ' => "%20".to_string(),
            other => other.to_string(),
        })
        .collect()
}

/// The sentence the service must answer with for `arn` — the requirement, written out
/// here rather than read from the crate so the test pins the text itself.
fn expected_message(arn: &str) -> String {
    format!("Quick setup manager with arn: {arn} could not be found.")
}

/// Every operation that reaches this service's own configuration-manager lookup with an
/// ARN the REQUEST carried in its BODY, by the name used in each assertion below.
const MANAGER_ARN_OPERATIONS: &[&str] = &["ListConfigurations"];

/// Every operation whose route carries the manager's ARN as a path LABEL and states the
/// manager not-found sentence: the manager's own three, and the configuration-definition
/// route labelled with its parent manager's ARN (`smithy_query(query="digest")`). Each
/// resolves that label in its own handler, so an ARN naming no manager earns this same
/// sentence there.
const MANAGER_LABEL_OPERATIONS: &[&str] = &[
    "GetConfigurationManager",
    "DeleteConfigurationManager",
    "UpdateConfigurationManager",
    "UpdateConfigurationDefinition",
];

/// The tag plane's three operations, which resolve the same manager by ARN but state a
/// fixed sentence of their own for an absent one: one for the read, one shared by the two
/// mutations. Nothing of the request is rendered in either, which is what the rows below
/// require.
const TAG_PLANE_OPERATIONS: &[(&str, &str)] = &[
    (
        "ListTagsForResource",
        "Configuration with requested id does not exist.",
    ),
    ("TagResource", "Resource arn not found."),
    ("UntagResource", "Resource arn not found."),
];

/// Drive one of [`MANAGER_ARN_OPERATIONS`] against `arn`, on the route and with the
/// payload the model binds for it (`OPERATIONS.md`).
async fn call_naming(
    router: &MultiServiceRouter,
    operation: &str,
    arn: &str,
    definition_id: &str,
) -> (u16, Value) {
    let tags = format!("/tags/{}", label(arn));
    match operation {
        "GetConfigurationManager" => {
            call(
                router,
                "GET",
                &format!("/configurationManager/{}", label(arn)),
                None,
            )
            .await
        }
        "DeleteConfigurationManager" => {
            call(
                router,
                "DELETE",
                &format!("/configurationManager/{}", label(arn)),
                None,
            )
            .await
        }
        "UpdateConfigurationManager" => {
            call(
                router,
                "PUT",
                &format!("/configurationManager/{}", label(arn)),
                Some(&json!({ "Name": "renamed" })),
            )
            .await
        }
        "ListTagsForResource" => call(router, "GET", &tags, None).await,
        "TagResource" => {
            call(
                router,
                "PUT",
                &tags,
                Some(&json!({ "Tags": { "Env": "prod" } })),
            )
            .await
        }
        "UntagResource" => call(router, "DELETE", &format!("{tags}?tagKeys=Env"), None).await,
        "UpdateConfigurationDefinition" => {
            call(
                router,
                "PUT",
                &format!(
                    "/configurationDefinition/{}/{}",
                    label(arn),
                    label(definition_id)
                ),
                Some(&json!({ "TypeVersion": "LATEST" })),
            )
            .await
        }
        "ListConfigurations" => {
            call(
                router,
                "POST",
                "/listConfigurations",
                Some(&json!({ "ManagerArn": arn })),
            )
            .await
        }
        other => panic!("no route is wired for operation {other:?}"),
    }
}

/// Assert one refusal: the modelled 404, the modelled error shape, and the sentence
/// built from the ARN this very request supplied.
fn assert_not_found(case: &str, arn: &str, status: u16, answer: &Value) {
    assert_eq!(
        status, 404,
        "case {case:?}: expected the modelled 404, got {status} / {answer:?}"
    );
    assert_eq!(
        error_code(answer),
        "ResourceNotFoundException",
        "case {case:?}: expected ResourceNotFoundException, got {answer:?}"
    );
    let actual = error_message(answer);
    assert_eq!(
        actual,
        expected_message(arn),
        "case {case:?}: expected {:?}, got {actual:?}",
        expected_message(arn)
    );
}

/// Assert one tag-plane refusal: the modelled 404 and shape, the fixed sentence that
/// operation states, and nothing of the ARN the request carried.
fn assert_fixed_not_found(case: &str, arn: &str, expected: &str, status: u16, answer: &Value) {
    assert_eq!(
        status, 404,
        "case {case:?}: expected the modelled 404, got {status} / {answer:?}"
    );
    assert_eq!(
        error_code(answer),
        "ResourceNotFoundException",
        "case {case:?}: expected ResourceNotFoundException, got {answer:?}"
    );
    let actual = error_message(answer);
    assert_eq!(
        actual, expected,
        "case {case:?}: expected {expected:?}, got {actual:?}"
    );
    assert!(
        !actual.contains(arn),
        "case {case:?}: a wholly fixed sentence renders nothing of the request, got \
         {actual:?}"
    );
}

/// Create one configuration manager, answering its ARN.
async fn create_manager(router: &MultiServiceRouter, case: &str) -> String {
    // `Type` must name a configuration type this service publishes; no case here is
    // about the type, so every create uses the catalog's first entry. Its parameters
    // name somewhere to deploy to and the Region to deploy into, and provide every
    // parameter the table documented for that type marks `(Required)`, which every
    // accepted definition must.
    let type_ = QUICK_SETUP_TYPE_CATALOG[0].type_;
    let body = json!({
        "Name": "not-found-message",
        "ConfigurationDefinitions": [{
            "Type": type_,
            "Parameters": super::type_parameter_fixtures::with_required_parameters(
                type_,
                json!({
                    "TargetOrganizationalUnits": "ou-1",
                    "TargetRegions": "us-east-1",
                }),
            ),
        }],
    });
    let (status, answer) = call(router, "POST", "/configurationManager", Some(&body)).await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateConfigurationManager must succeed, got {status} / {answer:?}"
    );
    answer
        .get("ManagerArn")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: expected a ManagerArn, got {answer:?}"))
        .to_string()
}

/// Well-formed ARNs no create minted, each satisfying the modelled `ManagerArn` pattern
/// `^arn:aws:ssm-quicksetup:([^:]+):(\d{12}):configuration-manager/<uuid>$`
/// (`smithy_query(query="shape",shape="GetConfigurationManagerInput")`). The rows differ
/// in Region, account id and the uuid — including its hex case, which that pattern
/// admits — because none of the three is part of the sentence: each must come back
/// exactly as it was sent.
const UNKNOWN_MANAGER_ARNS: &[(&str, &str)] = &[
    (
        "the signing Region, the all-zero account",
        "arn:aws:ssm-quicksetup:us-west-2:000000000000:configuration-manager/11111111-2222-3333-4444-555555555555",
    ),
    (
        "another Region and another account",
        "arn:aws:ssm-quicksetup:eu-central-1:123456789012:configuration-manager/0f0e0d0c-0b0a-0908-0706-050403020100",
    ),
    (
        "an upper-case hex uuid",
        "arn:aws:ssm-quicksetup:ap-southeast-4:987654321098:configuration-manager/ABCDEF01-2345-6789-ABCD-EF0123456789",
    ),
    (
        "a mixed-case hex uuid in a third Region",
        "arn:aws:ssm-quicksetup:sa-east-1:111122223333:configuration-manager/aBcDeF01-2345-6789-aBcD-eF0123456789",
    ),
];

/// A definition id for the rows that name one: the manager is looked for first, so the id
/// only has to satisfy the modelled `^[a-z0-9-]{1,20}$`.
const SOME_DEFINITION_ID: &str = "cfg-1";

/// Every operation that resolves a manager ARN its request carried refuses an ARN naming
/// no stored manager with the one not-found sentence, carrying the ARN the request
/// supplied — whether that ARN arrived in the body or as a route's path label — while the
/// tag plane states its own fixed sentence for the same resource.
#[tokio::test]
async fn an_arn_naming_no_manager_is_refused_with_the_not_found_sentence() {
    let router = emulator();
    // A manager that exists, so each refusal below is about the ARN that was sent
    // rather than about an empty store.
    let live = create_manager(&router, "a live manager").await;

    for (arn_case, arn) in UNKNOWN_MANAGER_ARNS {
        for operation in MANAGER_ARN_OPERATIONS.iter().chain(MANAGER_LABEL_OPERATIONS) {
            let case = format!("{operation} on {arn_case}");
            let (status, answer) = call_naming(&router, operation, arn, SOME_DEFINITION_ID).await;
            assert_not_found(&case, arn, status, &answer);
        }
        for (operation, expected) in TAG_PLANE_OPERATIONS {
            let case = format!("{operation} on {arn_case}");
            let (status, answer) = call_naming(&router, operation, arn, SOME_DEFINITION_ID).await;
            assert_fixed_not_found(&case, arn, expected, status, &answer);
        }
    }

    // The live manager is still readable, so the refusals above were about the ARNs
    // they named and nothing else.
    let (status, answer) = call(
        &router,
        "GET",
        &format!("/configurationManager/{}", label(&live)),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "the live manager must still be readable, got {status} / {answer:?}"
    );
}

/// The same sentence for a manager that DID exist for this caller and was then deleted:
/// the ARN in the message is the one the request supplied — the very ARN the successful
/// read before the delete echoed — and the operation answers 200 before the teardown
/// comes due, so only the not-found path is under test here.
#[tokio::test]
async fn a_deleted_manager_s_own_arn_is_refused_with_the_not_found_sentence() {
    let router = emulator();
    let doomed = create_manager(&router, "a manager about to be deleted").await;

    // Positive control: the identical call while the manager is live succeeds and echoes
    // the ARN, and hands back the id of the definition the create assigned.
    let (status, answer) = call(
        &router,
        "GET",
        &format!("/configurationManager/{}", label(&doomed)),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "the live manager must be readable, got {status} / {answer:?}"
    );
    assert_eq!(
        answer.get("ManagerArn").and_then(Value::as_str),
        Some(doomed.as_str()),
        "the live read must echo the ARN, got {answer:?}"
    );
    let definition_id = answer["ConfigurationDefinitions"]
        .as_array()
        .and_then(|definitions| definitions.first())
        .and_then(|definition| definition.get("Id"))
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("expected a definition with an Id, got {answer:?}"))
        .to_string();

    let (status, answer) = call(
        &router,
        "DELETE",
        &format!("/configurationManager/{}", label(&doomed)),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "DeleteConfigurationManager must be accepted, got {status} / {answer:?}"
    );
    // The delete starts an asynchronous teardown, so the manager is gone from the store
    // only once that teardown comes due: wait for that condition, not for a duration
    // (`tests/all/target_authorization.rs` owns the one wait).
    super::target_authorization::poll_until_absent(
        &router,
        "a manager about to be deleted",
        &doomed,
    )
    .await;

    for operation in MANAGER_ARN_OPERATIONS.iter().chain(MANAGER_LABEL_OPERATIONS) {
        let case = format!("{operation} on the ARN of a deleted manager");
        let (status, answer) = call_naming(&router, operation, &doomed, &definition_id).await;
        assert_not_found(&case, &doomed, status, &answer);
    }
    for (operation, expected) in TAG_PLANE_OPERATIONS {
        let case = format!("{operation} on the ARN of a deleted manager");
        let (status, answer) = call_naming(&router, operation, &doomed, &definition_id).await;
        assert_fixed_not_found(&case, &doomed, expected, status, &answer);
    }
}
