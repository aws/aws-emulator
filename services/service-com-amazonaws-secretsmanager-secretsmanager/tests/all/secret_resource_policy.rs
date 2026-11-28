// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the resource-based policy attached to a secret:
//! `PutResourcePolicy`, `GetResourcePolicy` and `DeleteResourcePolicy`, driven
//! over the wire through the multi-service router.
//!
//! The rules under test:
//!
//! * a secret carries at most one resource policy document: `GetResourcePolicy`
//!   reports the attached document, in the form this service reports a policy in
//!   (`tests/all/secret_resource_policy_reported_form.rs` pins that form), a
//!   second `PutResourcePolicy` replaces it, `DeleteResourcePolicy`
//!   removes it, and a secret carrying none reports no document rather than an
//!   error;
//! * every one of the three resolves a `SecretId` the same way — by name, by
//!   full ARN or by partial ARN — and reports the secret's ARN and name;
//! * a `ResourcePolicy` that is not an IAM policy document (JSON object whose
//!   `Statement` is a statement, or a non-empty list of statements, each with an
//!   `Effect` of `Allow` or `Deny`) is refused with
//!   `MalformedPolicyDocumentException`, carrying the one sentence the service
//!   says about that condition, and attaches nothing;
//! * a request that blocks public policies is refused with
//!   `PublicPolicyException` exactly when the document ALLOWS a wildcard
//!   principal; without that flag the same document attaches;
//! * a `SecretId` that names nothing is `ResourceNotFoundException`, and a
//!   secret inside its recovery window still exists, so these operations refuse
//!   it with `InvalidRequestException`;
//! * the policy a call addresses is the one named `default`, which is the only
//!   name a secret carries: a write of another name is refused as a parameter and
//!   a read of another name as a resource that does not exist, and every answer
//!   reports the name it addressed.
//!
//! Every test is a TABLE over the class its rule generalises: the shape, size
//! and charset of the policy document, the form of the `SecretId`, the
//! statement's effect and principal form, the operation under test.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_secretsmanager_secretsmanager::state::{
    DEFAULT_RESOURCE_POLICY_NAME, ONLY_DEFAULT_RESOURCE_POLICY_MESSAGE,
    RESOURCE_POLICY_NOT_FOUND_MESSAGE,
};
use service_com_amazonaws_secretsmanager_secretsmanager::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 header naming `secretsmanager` in `us-west-2`; the router routes on
/// the credential scope and the emulator verifies signatures only when an
/// authenticator is registered, which this harness does not do.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-west-2/secretsmanager/aws4_request, \
     SignedHeaders=host;x-amz-date, Signature=deadbeef";

/// The documented upper bound on a policy document
/// (`NonEmptyResourcePolicyType`, len 1..20480, this service's Smithy model).
const MAX_POLICY_LEN: usize = 20_480;

/// The one sentence Secrets Manager says about a resource policy it cannot read
/// as a policy document — the `MalformedPolicyDocumentException` message, and
/// the message `ValidateResourcePolicy` reports for its syntax check.
const SYNTAX_ERROR_MESSAGE: &str = "This resource policy contains a syntax error.";

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

/// Send one request that must fail with a 400, and return its wire error code
/// and the message it carries.
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
    let code = raw.split('#').next_back().unwrap_or(raw).to_string();
    let message = parsed
        .get("message")
        .or_else(|| parsed.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: no message in error body {parsed}"))
        .to_string();
    (code, message)
}

/// Send one request that must fail with a 400, and return its wire error code.
async fn err_code(
    router: &MultiServiceRouter,
    operation: &str,
    body: Value,
    case: &str,
) -> String {
    refusal(router, operation, body, case).await.0
}

/// Create a secret and return its ARN.
async fn create(router: &MultiServiceRouter, name: &str, case: &str) -> String {
    let created = ok(
        router,
        "CreateSecret",
        json!({"Name": name, "SecretString": "s3cret"}),
        case,
    )
    .await;
    created
        .get("ARN")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: no ARN in {created}"))
        .to_string()
}

/// The `ResourcePolicy` member of a `GetResourcePolicy` response, absent when
/// the secret carries no document.
fn policy_of(response: &Value) -> Option<&str> {
    response.get("ResourcePolicy").and_then(Value::as_str)
}

/// A policy document as this service REPORTS it: an attached policy is rendered into the
/// canonical layout every emulated service that attaches an IAM policy document reports,
/// rather than echoed back as the caller's bytes. WHAT that layout is is pinned against
/// text written out by hand in `tests/all/secret_resource_policy_reported_form.rs`; here
/// it is only the reference point a round trip is measured from.
fn reported_policy(submitted: &str) -> String {
    ::service_common::iam_policy::canonical_document(submitted)
}

/// A policy document granting one concrete principal one action, rendered
/// through `serde_json` so it is a document the service has to parse rather
/// than one it can match on.
fn policy_for(principal: &str) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": {"AWS": principal},
            "Action": "secretsmanager:GetSecretValue",
            "Resource": "*"
        }]
    })
    .to_string()
}

/// A valid policy document just under the documented length bound: enough
/// statements, each with its own `Sid`, to fill it.
fn large_policy() -> String {
    let mut statements = Vec::new();
    let mut rendered = String::new();
    for index in 0.. {
        statements.push(json!({
            "Sid": format!("statement{index:04}"),
            "Effect": "Allow",
            "Principal": {"AWS": format!("arn:aws:iam::123456789012:role/reader{index:04}")},
            "Action": "secretsmanager:GetSecretValue",
            "Resource": "*"
        }));
        let candidate = json!({"Version": "2012-10-17", "Statement": statements}).to_string();
        if candidate.len() > MAX_POLICY_LEN {
            break;
        }
        rendered = candidate;
    }
    rendered
}

/// A document attached to a secret comes back from `GetResourcePolicy`
/// whatever its shape, size or charset — and a second attach replaces it.
#[tokio::test]
async fn attached_policy_round_trips() {
    // The rows differ along the document's form (one statement object vs a
    // list), its size (a single statement vs one filling the documented bound),
    // its charset (non-ASCII in a `Sid`), its effect, and the insignificant
    // whitespace a caller may have left in it — none of which the service may
    // reinterpret.
    let documents: Vec<(&str, String)> = vec![
        ("one-allow-statement", policy_for("arn:aws:iam::123456789012:root")),
        (
            "statement-object-not-list",
            // The `Statement` element as ONE statement rather than a list, which
            // is the form this row varies. Its `Action` names one action of this
            // service rather than every action, because the document a secret
            // carries is EVALUATED on each later call: a `Deny` of `*` to every
            // principal would refuse the reads below, which is the lockout such
            // a document really is rather than anything about its form.
            json!({
                "Version": "2012-10-17",
                "Statement": {
                    "Effect": "Deny",
                    "Principal": "*",
                    "Action": "secretsmanager:GetSecretValue",
                    "Resource": "*"
                }
            })
            .to_string(),
        ),
        (
            "deny-with-condition",
            json!({
                "Version": "2012-10-17",
                "Statement": [{
                    "Effect": "Deny",
                    "Principal": "*",
                    "Action": "secretsmanager:GetSecretValue",
                    "Resource": "*",
                    "Condition": {
                        "StringNotEquals": {
                            "aws:PrincipalArn": "arn:aws:iam::123456789012:role/reader"
                        }
                    }
                }]
            })
            .to_string(),
        ),
        (
            "non-ascii-sid-and-pretty-printed",
            "{\n  \"Version\": \"2012-10-17\",\n  \"Statement\": [\n    {\n      \"Sid\": \
             \"läsare-☃\",\n      \"Effect\": \"Allow\",\n      \"Principal\": {\"AWS\": \
             \"arn:aws:iam::123456789012:role/läsare\"},\n      \"Action\": \
             \"secretsmanager:GetSecretValue\",\n      \"Resource\": \"*\"\n    }\n  ]\n}"
                .to_string(),
        ),
        ("fills-the-documented-length-bound", large_policy()),
    ];

    for (case, document) in documents {
        let router = emulator();
        let arn = create(&router, &format!("policy-{case}"), case).await;
        let name = format!("policy-{case}");

        let attached = ok(
            &router,
            "PutResourcePolicy",
            json!({"SecretId": &name, "ResourcePolicy": &document}),
            case,
        )
        .await;
        assert_eq!(
            attached.get("ARN").and_then(Value::as_str),
            Some(arn.as_str()),
            "case {case:?}: expected PutResourcePolicy to report ARN {arn:?}, got {attached}"
        );
        assert_eq!(
            attached.get("Name").and_then(Value::as_str),
            Some(name.as_str()),
            "case {case:?}: expected PutResourcePolicy to report Name {name:?}, got {attached}"
        );

        let read = ok(
            &router,
            "GetResourcePolicy",
            json!({"SecretId": &name}),
            case,
        )
        .await;
        assert_eq!(
            policy_of(&read),
            Some(reported_policy(&document).as_str()),
            "case {case:?}: expected the attached document back, got {:?}",
            policy_of(&read)
        );
        assert_eq!(
            read.get("ARN").and_then(Value::as_str),
            Some(arn.as_str()),
            "case {case:?}: expected GetResourcePolicy to report ARN {arn:?}, got {read}"
        );

        // One secret carries at most one policy: attaching another replaces it.
        let replacement = policy_for("arn:aws:iam::210987654321:role/replacement");
        ok(
            &router,
            "PutResourcePolicy",
            json!({"SecretId": &name, "ResourcePolicy": &replacement}),
            case,
        )
        .await;
        let reread = ok(
            &router,
            "GetResourcePolicy",
            json!({"SecretId": &name}),
            case,
        )
        .await;
        assert_eq!(
            policy_of(&reread),
            Some(reported_policy(&replacement).as_str()),
            "case {case:?}: expected the replacement document, got {:?}",
            policy_of(&reread)
        );
    }
}

/// Every one of the three operations resolves a `SecretId` in each of its
/// documented forms, and a read never changes what the next caller sees.
#[tokio::test]
async fn secret_id_forms_all_resolve_the_same_secret() {
    let router = emulator();
    let name = "policy-secret-id-forms";
    let arn = create(&router, name, "setup").await;
    // "we recommend that you specify a complete ARN rather than a partial ARN"
    // (`GetResourcePolicyRequest$SecretId`): the partial ARN is the full one
    // without the trailing hyphen and six random characters.
    let partial = arn
        .rsplit_once('-')
        .map(|(head, _)| head.to_string())
        .expect("a full secret ARN ends in a hyphen and six characters");
    let document = policy_for("arn:aws:iam::123456789012:root");
    ok(
        &router,
        "PutResourcePolicy",
        json!({"SecretId": name, "ResourcePolicy": &document}),
        "setup",
    )
    .await;

    for (case, secret_id) in [
        ("by-name", name.to_string()),
        ("by-full-arn", arn.clone()),
        ("by-partial-arn", partial),
    ] {
        // Read twice: a report must not change what a later caller sees.
        for attempt in 0..2 {
            let read = ok(
                &router,
                "GetResourcePolicy",
                json!({"SecretId": &secret_id}),
                case,
            )
            .await;
            assert_eq!(
                policy_of(&read),
                Some(reported_policy(&document).as_str()),
                "case {case:?} (read {attempt}): expected the attached document, got {:?}",
                policy_of(&read)
            );
            assert_eq!(
                read.get("Name").and_then(Value::as_str),
                Some(name),
                "case {case:?}: expected Name {name:?}, got {read}"
            );
        }
    }

    // And the writer resolves the same forms.
    for (case, secret_id) in [("delete-by-full-arn", arn), ("delete-by-name", name.into())] {
        let deleted = ok(
            &router,
            "DeleteResourcePolicy",
            json!({"SecretId": &secret_id}),
            case,
        )
        .await;
        assert_eq!(
            deleted.get("Name").and_then(Value::as_str),
            Some(name),
            "case {case:?}: expected Name {name:?}, got {deleted}"
        );
    }
}

/// A secret that carries no document reports none, and detaching nothing is not
/// an error: `DeleteResourcePolicy` succeeds however often it is called.
#[tokio::test]
async fn absent_policy_is_reported_and_deleted_without_error() {
    let router = emulator();
    let name = "policy-absent";
    create(&router, name, "setup").await;

    // The sequence under test: the counts of delete calls before and after an
    // attach, which is the class "how many times a caller detaches nothing".
    let read = ok(
        &router,
        "GetResourcePolicy",
        json!({"SecretId": name}),
        "never-attached",
    )
    .await;
    assert_eq!(
        policy_of(&read),
        None,
        "case \"never-attached\": expected no ResourcePolicy member, got {:?}",
        policy_of(&read)
    );

    for attempt in 0..3 {
        let case = format!("delete-noop-{attempt}");
        let deleted = ok(
            &router,
            "DeleteResourcePolicy",
            json!({"SecretId": name}),
            &case,
        )
        .await;
        assert_eq!(
            deleted.get("Name").and_then(Value::as_str),
            Some(name),
            "case {case:?}: expected Name {name:?}, got {deleted}"
        );
        let read = ok(
            &router,
            "GetResourcePolicy",
            json!({"SecretId": name}),
            &case,
        )
        .await;
        assert_eq!(
            policy_of(&read),
            None,
            "case {case:?}: expected no ResourcePolicy member, got {:?}",
            policy_of(&read)
        );
    }

    // Attach, then detach: the document is gone and the secret itself remains.
    let document = policy_for("arn:aws:iam::123456789012:root");
    ok(
        &router,
        "PutResourcePolicy",
        json!({"SecretId": name, "ResourcePolicy": &document}),
        "attach-then-detach",
    )
    .await;
    ok(
        &router,
        "DeleteResourcePolicy",
        json!({"SecretId": name}),
        "attach-then-detach",
    )
    .await;
    let read = ok(
        &router,
        "GetResourcePolicy",
        json!({"SecretId": name}),
        "attach-then-detach",
    )
    .await;
    assert_eq!(
        policy_of(&read),
        None,
        "case \"attach-then-detach\": expected no ResourcePolicy member, got {:?}",
        policy_of(&read)
    );
    let described = ok(
        &router,
        "DescribeSecret",
        json!({"SecretId": name}),
        "attach-then-detach",
    )
    .await;
    assert_eq!(
        described.get("Name").and_then(Value::as_str),
        Some(name),
        "case \"attach-then-detach\": the secret itself must survive, got {described}"
    );
}

/// A `ResourcePolicy` that is not a policy document is refused with
/// `MalformedPolicyDocumentException`, and attaches nothing.
#[tokio::test]
async fn a_document_that_is_not_a_policy_is_refused() {
    // The rows span the ways a document can fail the policy-document grammar:
    // not JSON at all, JSON that is not an object, an object with no
    // `Statement`, a `Statement` that is neither a statement nor a list of
    // them, an empty list, and a statement with no usable `Effect`.
    let malformed: Vec<(&str, String)> = vec![
        ("not-json", "this is not a policy".to_string()),
        ("truncated-json", "{\"Statement\": [".to_string()),
        ("json-array", "[]".to_string()),
        ("json-string", "\"Statement\"".to_string()),
        ("json-number", "17".to_string()),
        ("object-without-statement", json!({"Version": "2012-10-17"}).to_string()),
        (
            "statement-is-a-string",
            json!({"Statement": "Allow everything"}).to_string(),
        ),
        ("statement-list-empty", json!({"Statement": []}).to_string()),
        (
            "statement-list-of-strings",
            json!({"Statement": ["Allow"]}).to_string(),
        ),
        (
            "statement-without-effect",
            json!({"Statement": [{"Action": "secretsmanager:GetSecretValue"}]}).to_string(),
        ),
        (
            "effect-is-neither-allow-nor-deny",
            json!({"Statement": [{"Effect": "Maybe", "Principal": "*"}]}).to_string(),
        ),
    ];

    let router = emulator();
    let name = "policy-malformed";
    create(&router, name, "setup").await;
    let good = policy_for("arn:aws:iam::123456789012:root");
    ok(
        &router,
        "PutResourcePolicy",
        json!({"SecretId": name, "ResourcePolicy": &good}),
        "setup",
    )
    .await;

    for (case, document) in malformed {
        let (code, message) = refusal(
            &router,
            "PutResourcePolicy",
            json!({"SecretId": name, "ResourcePolicy": &document}),
            case,
        )
        .await;
        assert_eq!(
            code, "MalformedPolicyDocumentException",
            "case {case:?}: expected MalformedPolicyDocumentException, got {code}"
        );
        // One fixed sentence for the condition, whatever the document got wrong
        // and whatever it contained: the refusal names no part of it. It is the
        // sentence `ValidateResourcePolicy` reports for the same check, so the
        // pre-flight guard and the refusal read alike.
        assert_eq!(
            message, SYNTAX_ERROR_MESSAGE,
            "case {case:?}: expected the refusal message {SYNTAX_ERROR_MESSAGE:?}, got \
             {message:?}"
        );
        // The refused document replaced nothing.
        let read = ok(
            &router,
            "GetResourcePolicy",
            json!({"SecretId": name}),
            case,
        )
        .await;
        assert_eq!(
            policy_of(&read),
            Some(reported_policy(&good).as_str()),
            "case {case:?}: a refused document must leave the attached one in place, got {:?}",
            policy_of(&read)
        );
    }
}

/// `BlockPublicPolicy` refuses exactly those documents that ALLOW a wildcard
/// principal; without it the same document attaches.
#[tokio::test]
async fn block_public_policy_refuses_only_a_wildcard_allow() {
    // The rows span the forms a principal takes — the bare wildcard string, a
    // principal-type map, a list of identifiers — crossed with the effect,
    // which is what decides whether the wildcard grants broad access.
    let documents: Vec<(&str, String, bool)> = vec![
        (
            "allow-bare-wildcard-principal",
            json!({
                "Version": "2012-10-17",
                "Statement": [{
                    "Effect": "Allow",
                    "Principal": "*",
                    "Action": "secretsmanager:GetSecretValue",
                    "Resource": "*"
                }]
            })
            .to_string(),
            true,
        ),
        (
            "allow-wildcard-under-principal-type",
            json!({
                "Version": "2012-10-17",
                "Statement": [{
                    "Effect": "Allow",
                    "Principal": {"AWS": "*"},
                    "Action": "secretsmanager:GetSecretValue",
                    "Resource": "*"
                }]
            })
            .to_string(),
            true,
        ),
        (
            "allow-wildcard-in-a-principal-list",
            json!({
                "Version": "2012-10-17",
                "Statement": [{
                    "Effect": "Allow",
                    "Principal": {"AWS": ["arn:aws:iam::123456789012:root", "*"]},
                    "Action": "secretsmanager:GetSecretValue",
                    "Resource": "*"
                }]
            })
            .to_string(),
            true,
        ),
        (
            "wildcard-allow-among-several-statements",
            json!({
                "Version": "2012-10-17",
                "Statement": [
                    {
                        "Effect": "Deny",
                        "Principal": {"AWS": "arn:aws:iam::123456789012:root"},
                        "Action": "secretsmanager:DeleteSecret",
                        "Resource": "*"
                    },
                    {
                        "Effect": "Allow",
                        "Principal": "*",
                        "Action": "secretsmanager:GetSecretValue",
                        "Resource": "*"
                    }
                ]
            })
            .to_string(),
            true,
        ),
        (
            "allow-one-concrete-principal",
            policy_for("arn:aws:iam::123456789012:role/reader"),
            false,
        ),
        (
            "deny-the-wildcard-principal",
            json!({
                "Version": "2012-10-17",
                "Statement": [{
                    "Effect": "Deny",
                    "Principal": "*",
                    "Action": "secretsmanager:GetSecretValue",
                    "Resource": "*"
                }]
            })
            .to_string(),
            false,
        ),
        (
            "allow-a-service-principal",
            json!({
                "Version": "2012-10-17",
                "Statement": [{
                    "Effect": "Allow",
                    "Principal": {"Service": "lambda.amazonaws.com"},
                    "Action": "secretsmanager:GetSecretValue",
                    "Resource": "*"
                }]
            })
            .to_string(),
            false,
        ),
        (
            "wildcard-only-in-action-and-resource",
            json!({
                "Version": "2012-10-17",
                "Statement": [{
                    "Effect": "Allow",
                    "Principal": {"AWS": "arn:aws:iam::123456789012:root"},
                    "Action": "*",
                    "Resource": "*"
                }]
            })
            .to_string(),
            false,
        ),
    ];

    for (case, document, public) in documents {
        let router = emulator();
        let name = format!("policy-public-{case}");
        create(&router, &name, case).await;

        let blocked_case = format!("{case}/BlockPublicPolicy=true");
        if public {
            let code = err_code(
                &router,
                "PutResourcePolicy",
                json!({
                    "SecretId": &name,
                    "ResourcePolicy": &document,
                    "BlockPublicPolicy": true
                }),
                &blocked_case,
            )
            .await;
            assert_eq!(
                code, "PublicPolicyException",
                "case {blocked_case:?}: expected PublicPolicyException, got {code}"
            );
            let read = ok(
                &router,
                "GetResourcePolicy",
                json!({"SecretId": &name}),
                &blocked_case,
            )
            .await;
            assert_eq!(
                policy_of(&read),
                None,
                "case {blocked_case:?}: a refused put must attach nothing, got {:?}",
                policy_of(&read)
            );
        } else {
            ok(
                &router,
                "PutResourcePolicy",
                json!({
                    "SecretId": &name,
                    "ResourcePolicy": &document,
                    "BlockPublicPolicy": true
                }),
                &blocked_case,
            )
            .await;
            let read = ok(
                &router,
                "GetResourcePolicy",
                json!({"SecretId": &name}),
                &blocked_case,
            )
            .await;
            assert_eq!(
                policy_of(&read),
                Some(reported_policy(&document).as_str()),
                "case {blocked_case:?}: a policy granting no broad access must attach, got {:?}",
                policy_of(&read)
            );
        }

        // "By default, public policies aren't blocked": both the explicit false
        // and the absent member attach the very same document.
        for (flag_case, body) in [
            (
                format!("{case}/BlockPublicPolicy=false"),
                json!({
                    "SecretId": &name,
                    "ResourcePolicy": &document,
                    "BlockPublicPolicy": false
                }),
            ),
            (
                format!("{case}/BlockPublicPolicy absent"),
                json!({"SecretId": &name, "ResourcePolicy": &document}),
            ),
        ] {
            ok(&router, "PutResourcePolicy", body, &flag_case).await;
            let read = ok(
                &router,
                "GetResourcePolicy",
                json!({"SecretId": &name}),
                &flag_case,
            )
            .await;
            assert_eq!(
                policy_of(&read),
                Some(reported_policy(&document).as_str()),
                "case {flag_case:?}: expected the document to attach, got {:?}",
                policy_of(&read)
            );
        }
    }
}

/// A `SecretId` that names nothing is `ResourceNotFoundException`, and a secret
/// inside its recovery window is refused with `InvalidRequestException` — in
/// every one of the three operations.
#[tokio::test]
async fn unknown_and_deleted_secrets_are_refused() {
    let document = policy_for("arn:aws:iam::123456789012:root");
    // The rows span the operation under test and, for the missing secret, the
    // form of the `SecretId` that names nothing.
    let operations: Vec<(&str, Value)> = vec![
        ("GetResourcePolicy", json!({})),
        (
            "PutResourcePolicy",
            json!({"ResourcePolicy": document.clone()}),
        ),
        ("DeleteResourcePolicy", json!({})),
    ];

    for (operation, extra) in &operations {
        for secret_id in [
            "no-such-secret",
            "arn:aws:secretsmanager:us-west-2:000000000000:secret:no-such-secret-aBcDeF",
        ] {
            let case = format!("{operation}/missing/{secret_id}");
            let mut body = extra.clone();
            body["SecretId"] = json!(secret_id);
            let code = err_code(&emulator(), operation, body, &case).await;
            assert_eq!(
                code, "ResourceNotFoundException",
                "case {case:?}: expected ResourceNotFoundException, got {code}"
            );
        }

        let case = format!("{operation}/scheduled-for-deletion");
        let router = emulator();
        let name = format!("policy-deleted-{operation}");
        create(&router, &name, &case).await;
        ok(
            &router,
            "PutResourcePolicy",
            json!({"SecretId": &name, "ResourcePolicy": &document}),
            &case,
        )
        .await;
        ok(
            &router,
            "DeleteSecret",
            json!({"SecretId": &name, "RecoveryWindowInDays": 7}),
            &case,
        )
        .await;

        let mut body = extra.clone();
        body["SecretId"] = json!(&name);
        let code = err_code(&router, operation, body, &case).await;
        assert_eq!(
            code, "InvalidRequestException",
            "case {case:?}: a secret inside its recovery window still exists, so it must be \
             refused by its state; got {code}"
        );
    }
}

/// A secret carries ONE resource policy, named `default`: every call addresses
/// that name unless the caller states another, a write of another name is
/// `InvalidParameterException` and a read of another name is
/// `ResourceNotFoundException` — and each answer reports the name it addressed.
///
/// "If you don't specify this field, Secrets Manager uses `default`. You can only
/// attach the `default` policy; if you specify any other name, the operation
/// returns an `InvalidParameterException`."
/// (`PutResourcePolicyRequest$PolicyName`, this service's Smithy model; the same
/// for `DeleteResourcePolicyRequest$PolicyName`.) "If you specify a name other
/// than `default` and no policy with that name exists, the operation returns a
/// `ResourceNotFoundException`." (`GetResourcePolicyRequest$PolicyName`.)
#[tokio::test]
async fn only_the_default_resource_policy_is_addressed_by_every_policy_call() {
    let document = policy_for("arn:aws:iam::123456789012:root");
    // The names `ResourcePolicyNameType` admits that are not the one policy a
    // secret carries: each is inside the modelled length bound of 1..64 and its
    // charset `^[0-9A-Za-z\-\_]+$`, so each reaches the handler and is answered by
    // the rule rather than by the boundary validator. The rows run over that
    // charset, both published length bounds, and a spelling that differs from
    // `default` in case alone.
    let other_names: Vec<String> = vec![
        "Default".to_string(),
        "custom".to_string(),
        "my-policy_1".to_string(),
        "a".to_string(),
        "n".repeat(64),
    ];
    let router = emulator();
    let name = "policy-names";
    create(&router, name, "setup").await;
    ok(
        &router,
        "PutResourcePolicy",
        json!({"SecretId": name, "ResourcePolicy": &document}),
        "setup",
    )
    .await;

    // The default name is addressed whether or not the call states it, and every
    // answer names the policy it addressed.
    for stated in [false, true] {
        let case = format!("PolicyName stated: {stated}");
        for (operation, extra) in [
            ("PutResourcePolicy", json!({"ResourcePolicy": &document})),
            ("GetResourcePolicy", json!({})),
        ] {
            let mut body = extra.clone();
            body["SecretId"] = json!(name);
            if stated {
                body["PolicyName"] = json!(DEFAULT_RESOURCE_POLICY_NAME);
            }
            let answered = ok(&router, operation, body, &case).await;
            assert_eq!(
                answered.get("PolicyName").and_then(Value::as_str),
                Some(DEFAULT_RESOURCE_POLICY_NAME),
                "case {case:?}: expected {operation} to report the name it addressed, got \
                 {answered}"
            );
        }
    }

    for policy_name in &other_names {
        for operation in ["PutResourcePolicy", "DeleteResourcePolicy"] {
            let case = format!("{operation} naming {policy_name:?}");
            let mut body = json!({"SecretId": name, "PolicyName": policy_name});
            if operation == "PutResourcePolicy" {
                body["ResourcePolicy"] = json!(&document);
            }
            let (code, message) = refusal(&router, operation, body, &case).await;
            assert_eq!(
                (code.as_str(), message.as_str()),
                (
                    "InvalidParameterException",
                    ONLY_DEFAULT_RESOURCE_POLICY_MESSAGE
                ),
                "case {case:?}: expected the parameter refusal, got {code} {message:?}"
            );
        }

        let case = format!("GetResourcePolicy naming {policy_name:?}");
        let (code, message) = refusal(
            &router,
            "GetResourcePolicy",
            json!({"SecretId": name, "PolicyName": policy_name}),
            &case,
        )
        .await;
        assert_eq!(
            (code.as_str(), message.as_str()),
            (
                "ResourceNotFoundException",
                RESOURCE_POLICY_NOT_FOUND_MESSAGE
            ),
            "case {case:?}: expected the not-found refusal, got {code} {message:?}"
        );
    }

    // None of those refusals touched the one policy the secret carries, and the
    // delete that addresses it does: a read then reports neither the document nor
    // a name.
    let read = ok(
        &router,
        "GetResourcePolicy",
        json!({"SecretId": name}),
        "the policy the refusals left alone",
    )
    .await;
    assert_eq!(
        policy_of(&read),
        Some(reported_policy(&document).as_str()),
        "expected the attached document to be untouched, got {read}"
    );
    ok(
        &router,
        "DeleteResourcePolicy",
        json!({"SecretId": name, "PolicyName": DEFAULT_RESOURCE_POLICY_NAME}),
        "the delete that addresses the default policy",
    )
    .await;
    let read = ok(
        &router,
        "GetResourcePolicy",
        json!({"SecretId": name}),
        "the secret after the delete",
    )
    .await;
    assert_eq!(
        (policy_of(&read), read.get("PolicyName")),
        (None, None),
        "expected a secret carrying no policy to report neither the document nor a name, got \
         {read}"
    );
}
