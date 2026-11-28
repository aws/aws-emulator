// SPDX-License-Identifier: CC0-1.0

//! The NAME of the resource-based policy a Secrets Manager secret carries: which name every
//! policy call addresses, and what `ListResourcePolicies` reports.
//!
//! The RULE pinned here: **a secret carries one resource policy, named `default` — "If you
//! don't specify this field, Secrets Manager uses `default`" — so every policy call addresses
//! that name unless the caller states another, a WRITE that names another is
//! `InvalidParameterException` ("You can only attach the `default` policy; if you specify any
//! other name, the operation returns an `InvalidParameterException`",
//! `PutResourcePolicyRequest$PolicyName`, the Secrets Manager Smithy model; the same for
//! `DeleteResourcePolicyRequest$PolicyName`), a READ that names another is
//! `ResourceNotFoundException` ("If you specify a name other than `default` and no policy with
//! that name exists, the operation returns a `ResourceNotFoundException`",
//! `GetResourcePolicyRequest$PolicyName`), each answer reports the name it addressed, and
//! `ListResourcePolicies` reports that name exactly while a document is attached.**
//!
//! This suite lives here, beside the other suites of the assembled emulator, because the
//! owning crate still roots its integration tests at a hand-declared `tests/all.rs`, which a
//! fix may not edit; the router's `tests/all/` picks a new suite up with nothing to declare.
//! Every call goes through a router with the owning service mounted on one shared engine,
//! exactly as the production router mounts it, so each assertion exercises the real wire path.
//!
//! The rows differ along what the rule generalises over: the policy name the call carries
//! (every form `ResourcePolicyNameType` admits — one character, the documented 64-character
//! maximum, the charset's hyphen and underscore, and a spelling that differs from `default`
//! only in case), which of the three policy calls carries it, the form of the `SecretId`, and
//! the page size and cursor a listing asks for.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_secretsmanager_secretsmanager::state::{
    encode_next_token, DEFAULT_RESOURCE_POLICY_NAME, INVALID_NEXT_TOKEN_MESSAGE,
    MARKED_FOR_DELETION_MESSAGE, ONLY_DEFAULT_RESOURCE_POLICY_MESSAGE,
    RESOURCE_POLICY_NOT_FOUND_MESSAGE, SECRET_NOT_FOUND_MESSAGE,
};
use smithy_server_runtime::MultiServiceRouter;

/// The Region every request below is signed for.
const REGION: &str = "us-west-2";

/// The documented maximum length of a policy name (`ResourcePolicyNameType`, len 1..64, the
/// Secrets Manager Smithy model), which one row sits exactly on.
const MAX_POLICY_NAME_LEN: usize = 64;

/// A fresh emulator: one router with Secrets Manager mounted over one shared engine.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_secretsmanager_secretsmanager::EmulatedService::new(engine),
    );
    router
}

/// One awsJson1_1 Secrets Manager call, with whatever status it answered.
async fn call(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("secretsmanager.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("secretsmanager.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/secretsmanager/\
                 aws4_request, SignedHeaders=host, Signature=not-verified"
            ),
        )
        .body(Bytes::from(body.to_string()))
        .expect("a well-formed test request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// One call that must succeed, answered as its body.
async fn ok(router: &MultiServiceRouter, operation: &str, body: Value, case: &str) -> Value {
    let (status, parsed) = call(router, operation, &body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 from {operation}, got {status} with body {parsed}"
    );
    parsed
}

/// One call that must be refused, answered as `(error code, message)`.
async fn refusal(
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
    let message = parsed
        .get("Message")
        .or_else(|| parsed.get("message"))
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: no Message in error body {parsed}"))
        .to_string();
    (
        raw.split('#').next_back().unwrap_or(raw).to_string(),
        message,
    )
}

/// A required string member of a response body.
fn text(parsed: &Value, member: &str, case: &str) -> String {
    parsed
        .get(member)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: no {member} in {parsed}"))
        .to_string()
}

/// A string member of a JSON object, or `None` when it is absent.
fn member(parsed: &Value, name: &str) -> Option<String> {
    parsed.get(name).and_then(Value::as_str).map(str::to_string)
}

/// A `ClientRequestToken` padded to the modelled 32-character minimum of
/// `ClientRequestTokenType`, which every write that stakes a version has to carry.
fn version_token(label: &str) -> String {
    format!("{label:0<32}")
}

/// Create a secret holding a value, and answer its ARN.
async fn create(router: &MultiServiceRouter, name: &str, case: &str) -> String {
    let created = ok(
        router,
        "CreateSecret",
        json!({
            "Name": name,
            "SecretString": "s3cr3t",
            "ClientRequestToken": version_token("created"),
        }),
        case,
    )
    .await;
    text(&created, "ARN", case)
}

/// A policy document granting one concrete principal one action, rendered through
/// `serde_json` so it is a document the service has to parse rather than one it can match on.
fn policy_document() -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": {"AWS": format!("arn:aws:iam::{}:root", ::engine::emulator_account_id())},
            "Action": "secretsmanager:GetSecretValue",
            "Resource": "*",
        }],
    })
    .to_string()
}

/// The `PolicyName` values of a `ListResourcePolicies` answer, in the order it reports them.
fn listed_names(listed: &Value, case: &str) -> Vec<String> {
    listed
        .get("ResourcePolicies")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: no ResourcePolicies in {listed}"))
        .iter()
        .map(|entry| text(entry, "PolicyName", case))
        .collect()
}

/// The names `ResourcePolicyNameType` admits that are NOT the one policy a secret carries:
/// every one of them is inside the modelled length bound of 1..64 and its charset
/// `^[0-9A-Za-z\-\_]+$`, so each reaches the handler and is answered by the rule rather than
/// by the boundary validator.
fn other_names() -> Vec<String> {
    vec![
        // A spelling that differs from `default` in case alone.
        "Default".to_string(),
        "custom".to_string(),
        "my-policy_1".to_string(),
        // One character, the modelled minimum.
        "a".to_string(),
        // The modelled maximum.
        "n".repeat(MAX_POLICY_NAME_LEN),
    ]
}

/// How a row names the secret in its `SecretId`.
#[derive(Clone, Copy, Debug)]
enum IdForm {
    Name,
    FullArn,
    PartialArn,
}

/// The `SecretId` a row sends.
fn secret_id(form: IdForm, name: &str, arn: &str) -> String {
    match form {
        IdForm::Name => name.to_string(),
        IdForm::FullArn => arn.to_string(),
        // The partial ARN is the full ARN without the trailing hyphen and the six random
        // characters the service appends to the secret's name.
        IdForm::PartialArn => arn[..arn.len().saturating_sub(7)].to_string(),
    }
}

#[tokio::test]
async fn every_policy_call_addresses_the_default_policy_and_reports_the_name_it_addressed() {
    // The rows run over the form of the `SecretId` and over whether the call states the
    // default name or leaves it out: the rule is about WHICH policy is addressed, not about
    // how the secret is named, so both spellings of the addressed policy behave alike.
    for form in [IdForm::Name, IdForm::FullArn, IdForm::PartialArn] {
        for stated in [false, true] {
            let case = format!("{form:?}, PolicyName stated: {stated}");
            let router = emulator();
            let name = "policy-names";
            let arn = create(&router, name, &case).await;
            let id = secret_id(form, name, &arn);
            let with_name = |body: Value| {
                let mut body = body;
                if stated {
                    body.as_object_mut()
                        .expect("a request body is an object")
                        .insert(
                            "PolicyName".to_string(),
                            json!(DEFAULT_RESOURCE_POLICY_NAME),
                        );
                }
                body
            };

            let attached = ok(
                &router,
                "PutResourcePolicy",
                with_name(json!({"SecretId": id.clone(), "ResourcePolicy": policy_document()})),
                &case,
            )
            .await;
            assert_eq!(
                (
                    member(&attached, "ARN"),
                    member(&attached, "Name"),
                    member(&attached, "PolicyName")
                ),
                (
                    Some(arn.clone()),
                    Some(name.to_string()),
                    Some(DEFAULT_RESOURCE_POLICY_NAME.to_string())
                ),
                "case {case:?}: expected the attach to report the secret and the name it \
                 addressed, got {attached}"
            );

            let read = ok(
                &router,
                "GetResourcePolicy",
                with_name(json!({"SecretId": id.clone()})),
                &case,
            )
            .await;
            assert_eq!(
                member(&read, "PolicyName"),
                Some(DEFAULT_RESOURCE_POLICY_NAME.to_string()),
                "case {case:?}: expected the read to report the name it addressed, got {read}"
            );
            assert!(
                member(&read, "ResourcePolicy").is_some(),
                "case {case:?}: expected the read to answer with the attached document, got \
                 {read}"
            );

            let deleted = ok(
                &router,
                "DeleteResourcePolicy",
                with_name(json!({"SecretId": id.clone()})),
                &case,
            )
            .await;
            assert_eq!(
                member(&deleted, "PolicyName"),
                Some(DEFAULT_RESOURCE_POLICY_NAME.to_string()),
                "case {case:?}: expected the delete to report the name it addressed, got \
                 {deleted}"
            );

            // The document is gone, so the read answers the secret with no policy and names
            // no policy either — "Secrets Manager only returns fields that have a value in
            // the response" (`DescribeSecret`, the Secrets Manager Smithy model).
            let read = ok(
                &router,
                "GetResourcePolicy",
                with_name(json!({"SecretId": id})),
                &case,
            )
            .await;
            assert_eq!(
                (
                    member(&read, "ResourcePolicy"),
                    member(&read, "PolicyName")
                ),
                (None, None),
                "case {case:?}: expected a secret carrying no policy to report neither the \
                 document nor a name, got {read}"
            );
        }
    }
}

#[tokio::test]
async fn a_policy_name_that_is_not_the_default_is_refused_by_the_call_that_names_it() {
    let case = "the names no secret carries";
    let router = emulator();
    let name = "other-policy-names";
    create(&router, name, case).await;
    ok(
        &router,
        "PutResourcePolicy",
        json!({"SecretId": name, "ResourcePolicy": policy_document()}),
        case,
    )
    .await;

    for policy_name in other_names() {
        // A WRITE of any other name is a parameter refusal: only the default policy can be
        // attached or deleted.
        for operation in ["PutResourcePolicy", "DeleteResourcePolicy"] {
            let row = format!("{operation} naming {policy_name:?}");
            let mut body = json!({"SecretId": name, "PolicyName": policy_name.clone()});
            if operation == "PutResourcePolicy" {
                body.as_object_mut()
                    .expect("a request body is an object")
                    .insert("ResourcePolicy".to_string(), json!(policy_document()));
            }
            let (code, message) = refusal(&router, operation, body, &row).await;
            assert_eq!(
                (code.as_str(), message.as_str()),
                (
                    "InvalidParameterException",
                    ONLY_DEFAULT_RESOURCE_POLICY_MESSAGE
                ),
                "case {row:?}: expected the parameter refusal, got {code} {message:?}"
            );
        }

        // A READ of any other name is a not-found: no policy of that name exists.
        let row = format!("GetResourcePolicy naming {policy_name:?}");
        let (code, message) = refusal(
            &router,
            "GetResourcePolicy",
            json!({"SecretId": name, "PolicyName": policy_name.clone()}),
            &row,
        )
        .await;
        assert_eq!(
            (code.as_str(), message.as_str()),
            (
                "ResourceNotFoundException",
                RESOURCE_POLICY_NOT_FOUND_MESSAGE
            ),
            "case {row:?}: expected the not-found refusal, got {code} {message:?}"
        );
    }

    // None of those refusals touched the one policy the secret carries.
    let read = ok(
        &router,
        "GetResourcePolicy",
        json!({"SecretId": name}),
        case,
    )
    .await;
    assert!(
        member(&read, "ResourcePolicy").is_some(),
        "case {case:?}: expected the attached document to be untouched, got {read}"
    );
    let listed = ok(
        &router,
        "ListResourcePolicies",
        json!({"SecretId": name}),
        case,
    )
    .await;
    assert_eq!(
        listed_names(&listed, case),
        vec![DEFAULT_RESOURCE_POLICY_NAME.to_string()],
        "case {case:?}: expected the one attached name to be listed, got {listed}"
    );
}

#[tokio::test]
async fn the_listed_policy_names_follow_the_document_the_secret_carries() {
    // The rows run over the form of the `SecretId` and over the page size asked for: the
    // listing reports the names attached, whatever the caller asks the page in.
    for form in [IdForm::Name, IdForm::FullArn, IdForm::PartialArn] {
        let case = format!("listed by {form:?}");
        let router = emulator();
        let name = "listed-policies";
        let arn = create(&router, name, &case).await;
        let id = secret_id(form, name, &arn);

        // A secret carrying no policy lists none: `ResourcePolicySummaryListType` is
        // modelled with a minimum length of 0, and there is no page to continue from.
        for page in [json!({}), json!({"MaxResults": 1}), json!({"MaxResults": 100})] {
            let row = format!("{case}, no policy attached, page {page}");
            let mut body = page.clone();
            body.as_object_mut()
                .expect("a request body is an object")
                .insert("SecretId".to_string(), json!(id.clone()));
            let listed = ok(&router, "ListResourcePolicies", body, &row).await;
            assert_eq!(
                listed_names(&listed, &row),
                Vec::<String>::new(),
                "case {row:?}: expected no names for a secret carrying no policy, got {listed}"
            );
            assert!(
                listed.get("NextToken").is_none(),
                "case {row:?}: expected no cursor where there is no further page, got {listed}"
            );
        }

        ok(
            &router,
            "PutResourcePolicy",
            json!({"SecretId": id.clone(), "ResourcePolicy": policy_document()}),
            &case,
        )
        .await;
        for page in [json!({}), json!({"MaxResults": 1}), json!({"MaxResults": 100})] {
            let row = format!("{case}, one policy attached, page {page}");
            let mut body = page.clone();
            body.as_object_mut()
                .expect("a request body is an object")
                .insert("SecretId".to_string(), json!(id.clone()));
            let listed = ok(&router, "ListResourcePolicies", body, &row).await;
            assert_eq!(
                listed_names(&listed, &row),
                vec![DEFAULT_RESOURCE_POLICY_NAME.to_string()],
                "case {row:?}: expected the attached name, got {listed}"
            );
            assert!(
                listed.get("NextToken").is_none(),
                "case {row:?}: the whole listing fits one page, so no cursor is reported, got \
                 {listed}"
            );
        }

        // Detaching the document takes its name out of the listing.
        ok(
            &router,
            "DeleteResourcePolicy",
            json!({"SecretId": id.clone()}),
            &case,
        )
        .await;
        let listed = ok(
            &router,
            "ListResourcePolicies",
            json!({"SecretId": id}),
            &case,
        )
        .await;
        assert_eq!(
            listed_names(&listed, &case),
            Vec::<String>::new(),
            "case {case:?}: expected the detached name to be gone, got {listed}"
        );
    }
}

#[tokio::test]
async fn the_listing_is_refused_for_a_secret_or_a_cursor_it_cannot_answer_for() {
    let case = "the listings this service cannot answer";
    let router = emulator();
    let name = "refused-listings";
    create(&router, name, case).await;
    ok(
        &router,
        "PutResourcePolicy",
        json!({"SecretId": name, "ResourcePolicy": policy_document()}),
        case,
    )
    .await;

    // The cursors no page of this listing can resume from: not the encoding this service
    // mints, and the encoding of a name this listing does not hold. `NextTokenType` is
    // modelled with a length bound of 1..4096, so every row stays inside it.
    let tokens = vec![
        "not-a-token".to_string(),
        "zzzz".to_string(),
        encode_next_token("custom"),
        "f".repeat(4_000),
    ];
    for token in tokens {
        let row = format!("NextToken {:.32?}", token);
        let (code, message) = refusal(
            &router,
            "ListResourcePolicies",
            json!({"SecretId": name, "NextToken": token.clone()}),
            &row,
        )
        .await;
        assert_eq!(
            (code.as_str(), message.as_str()),
            ("InvalidNextTokenException", INVALID_NEXT_TOKEN_MESSAGE),
            "case {row:?}: expected the invalid-cursor refusal, got {code} {message:?}"
        );
        assert!(
            !message.contains(token.as_str()),
            "case {row:?}: expected no part of the request in the message, got {message:?}"
        );
    }

    // A secret this Region holds none of is not found, and one inside its recovery window
    // still exists, so the listing is refused by its state.
    let (code, message) = refusal(
        &router,
        "ListResourcePolicies",
        json!({"SecretId": "never-created"}),
        case,
    )
    .await;
    assert_eq!(
        (code.as_str(), message.as_str()),
        ("ResourceNotFoundException", SECRET_NOT_FOUND_MESSAGE),
        "case {case:?}: expected the not-found refusal, got {code} {message:?}"
    );
    ok(&router, "DeleteSecret", json!({"SecretId": name}), case).await;
    let (code, message) = refusal(
        &router,
        "ListResourcePolicies",
        json!({"SecretId": name}),
        case,
    )
    .await;
    assert_eq!(
        (code.as_str(), message.as_str()),
        ("InvalidRequestException", MARKED_FOR_DELETION_MESSAGE),
        "case {case:?}: expected the deletion-state refusal, got {code} {message:?}"
    );
}
