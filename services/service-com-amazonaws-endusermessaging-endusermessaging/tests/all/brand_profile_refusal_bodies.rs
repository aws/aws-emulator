// SPDX-License-Identifier: CC0-1.0

//! The bodies this service puts on a brand-profile refusal, over the wire.
//!
//! Every assertion goes through `MultiServiceRouter::dispatch`, so it reads the
//! rendered restJson1 error body a real SDK client would parse.
//!
//! The rules under test, each over a SET of inputs rather than one recorded request:
//!
//! * a `ResourceNotFoundException` body carries this error class's fixed template
//!   `Resource not found - ResourceType="<type>" ResourceId="<id>"`, where `<type>`
//!   is the resource's hyphenated type and `<id>` is the identifier the request
//!   gave, verbatim — whichever operation refuses and whatever form the identifier
//!   took;
//! * a `ConflictException` body carries the fixed template
//!   `Conflict occurred - Reason="<REASON>" ResourceType="<type>"`, with the reason
//!   token naming the state that caused the conflict and no request-supplied value
//!   anywhere in the sentence;
//! * both report `resourceType` in that same hyphenated vocabulary, and
//!   `resourceId` names the resource.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_endusermessaging_endusermessaging::EmulatedService;

/// This service's sigv4 signing name, from the model (`OPERATIONS.md`, "Wire protocol").
const SIGNING_NAME: &str = "end-user-messaging";

/// The account id every generated emulator presents, as `engine` defines it.
const ACCOUNT_ID: &str = engine::EMULATOR_ACCOUNT_ID;

/// The region every request here is signed for.
const REGION: &str = "us-east-1";

/// The hyphenated resource type a brand-profile refusal reports, as the service
/// exports it for its handlers.
const BRAND_PROFILE_TYPE: &str =
    service_com_amazonaws_endusermessaging_endusermessaging::state::BRAND_PROFILE_RESOURCE_TYPE;

/// One response, reduced to what the assertions read.
#[derive(Debug)]
struct Reply {
    status: u16,
    /// The wire error code (`x-amzn-errortype`, else the body's `__type`), with any
    /// `namespace#` prefix stripped. `None` on success.
    error: Option<String>,
    body: Value,
}

impl Reply {
    fn string(&self, name: &str) -> String {
        self.body[name]
            .as_str()
            .unwrap_or_else(|| panic!("member {name} is not a string in {}", self.body))
            .to_string()
    }
}

/// One emulator over one shared engine: every call sees the same store.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(engine));
        Self { router }
    }

    async fn call(&self, method: &str, path: &str, body: Option<Value>) -> Reply {
        let authorization = format!(
            "AWS4-HMAC-SHA256 Credential=AKIDTESTONLY/20260101/{REGION}/{SIGNING_NAME}/aws4_request, \
             SignedHeaders=host, Signature=00"
        );
        let payload = body
            .map(|value| Bytes::from(serde_json::to_vec(&value).expect("a serializable body")))
            .unwrap_or_default();
        let request = http::Request::builder()
            .method(method)
            .uri(format!(
                "https://{SIGNING_NAME}.{REGION}.amazonaws.com{path}"
            ))
            .header("authorization", authorization)
            .header("content-type", "application/json")
            .body(payload)
            .expect("a well-formed test request");

        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let header_error = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let bytes = response.into_body();
        let body: Value = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        let error = if status < 400 {
            None
        } else {
            header_error
                .or_else(|| body["__type"].as_str().map(str::to_string))
                .map(|code| {
                    code.rsplit('#')
                        .next()
                        .unwrap_or(code.as_str())
                        .to_string()
                })
        };
        Reply {
            status,
            error,
            body,
        }
    }

    async fn create(&self, body: Value) -> Reply {
        self.call("POST", "/v1/brand-profiles", Some(body)).await
    }

    async fn delete(&self, id_or_arn: &str) -> Reply {
        self.call("DELETE", &format!("/v1/brand-profiles/{id_or_arn}"), None)
            .await
    }

    async fn update(&self, id_or_arn: &str, body: Value) -> Reply {
        self.call("PUT", &format!("/v1/brand-profiles/{id_or_arn}"), Some(body))
            .await
    }
}

/// The identifiers a `BrandProfileIdOrArn` accepts that name nothing: a minted-shaped
/// id, a short bare id, one carrying each punctuation the shape's
/// `^[A-Za-z0-9_:/-]+$` pattern allows, the maximum length the shape's
/// `@length(1..256)` permits, and a well-formed ARN for a profile that was never
/// created. Each row is reported back verbatim.
fn unknown_identifiers() -> Vec<String> {
    vec![
        "bp-00000000000000000".to_string(),
        "x".to_string(),
        "A_b-c:d/e".to_string(),
        "z".repeat(256),
        format!("arn:aws:{SIGNING_NAME}:{REGION}:{ACCOUNT_ID}:brand-profile/bp-11111111111111111"),
    ]
}

/// Every brand-profile operation that resolves a `BrandProfileIdOrArn`, with the
/// request body it needs.
fn not_found_calls() -> Vec<(&'static str, Option<Value>)> {
    vec![
        ("GET", None),
        ("DELETE", None),
        ("PUT", Some(json!({ "brandProfileName": "renamed" }))),
    ]
}

#[tokio::test]
async fn a_not_found_body_is_the_templated_sentence_with_the_requested_identifier() {
    let em = Emulator::new();

    for identifier in unknown_identifiers() {
        for (method, body) in not_found_calls() {
            let case = format!("{method} identifier={identifier:?}");
            let reply = em
                .call(
                    method,
                    &format!("/v1/brand-profiles/{identifier}"),
                    body.clone(),
                )
                .await;

            assert_eq!(
                reply.status, 404,
                "case {case}: expected 404, got {} / {:?}",
                reply.status, reply.body
            );
            assert_eq!(
                reply.error.as_deref(),
                Some("ResourceNotFoundException"),
                "case {case}: unexpected error code, got {:?}",
                reply.error
            );
            let expected = format!(
                "Resource not found - ResourceType=\"{BRAND_PROFILE_TYPE}\" ResourceId=\"{identifier}\""
            );
            assert_eq!(
                reply.string("message"),
                expected,
                "case {case}: expected message {expected:?}, got {:?}",
                reply.body
            );
            assert_eq!(
                reply.string("resourceType"),
                BRAND_PROFILE_TYPE,
                "case {case}: expected resourceType {BRAND_PROFILE_TYPE:?}, got {:?}",
                reply.body
            );
            assert_eq!(
                reply.string("resourceId"),
                identifier,
                "case {case}: resourceId must be the requested identifier, got {:?}",
                reply.body
            );
        }
    }
}

#[tokio::test]
async fn the_reported_resource_type_is_the_arn_resource_vocabulary() {
    // The type token is the resource segment of the resource's own ARN, so a
    // refusal and the ARN of a live profile spell the resource the same way.
    let em = Emulator::new();
    let created = em
        .create(json!({ "brandProfileName": "vocabulary" }))
        .await;
    let arn = created.string("brandProfileArn");
    let id = created.string("brandProfileId");
    assert_eq!(
        arn,
        format!("arn:aws:{SIGNING_NAME}:{REGION}:{ACCOUNT_ID}:{BRAND_PROFILE_TYPE}/{id}"),
        "the ARN must name the resource with the same token a refusal reports, got {arn:?}"
    );
}

#[tokio::test]
async fn a_deletion_protection_conflict_body_is_the_templated_sentence_with_no_request_value() {
    let em = Emulator::new();

    // Rows differ in how protection came to be on and in the identifier form the
    // refused delete names: a create with it set, a create followed by an update
    // that sets it, and the same profile addressed by its ARN. The refusal body is
    // the same fixed sentence in every one.
    let cases: Vec<(&str, bool, bool)> = vec![
        ("set at create, deleted by id", true, false),
        ("set by update, deleted by id", false, false),
        ("set at create, deleted by ARN", true, true),
    ];

    for (case, protect_at_create, delete_by_arn) in cases {
        let created = em
            .create(json!({
                "brandProfileName": "protected",
                "deletionProtectionEnabled": protect_at_create,
            }))
            .await;
        assert_eq!(
            created.status, 200,
            "case {case}: the create must succeed, got {} / {:?}",
            created.status, created.body
        );
        let id = created.string("brandProfileId");
        let arn = created.string("brandProfileArn");
        if !protect_at_create {
            let updated = em
                .update(&id, json!({ "deletionProtectionEnabled": true }))
                .await;
            assert_eq!(
                updated.status, 200,
                "case {case}: switching protection on must succeed, got {} / {:?}",
                updated.status, updated.body
            );
        }

        let reference = if delete_by_arn { arn.clone() } else { id.clone() };
        let refused = em.delete(&reference).await;
        assert_eq!(
            refused.status, 409,
            "case {case}: expected 409, got {} / {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error.as_deref(),
            Some("ConflictException"),
            "case {case}: unexpected error code, got {:?}",
            refused.error
        );
        let expected = format!(
            "Conflict occurred - Reason=\"DELETION_PROTECTION_ENABLED\" ResourceType=\"{BRAND_PROFILE_TYPE}\""
        );
        let message = refused.string("message");
        assert_eq!(
            message, expected,
            "case {case}: expected message {expected:?}, got {:?}",
            refused.body
        );
        assert!(
            !message.contains(&id) && !message.contains(&arn),
            "case {case}: the conflict sentence carries no request-supplied value, got {message:?}"
        );
        assert_eq!(
            refused.string("resourceType"),
            BRAND_PROFILE_TYPE,
            "case {case}: expected resourceType {BRAND_PROFILE_TYPE:?}, got {:?}",
            refused.body
        );
        assert_eq!(
            refused.string("resourceId"),
            id,
            "case {case}: resourceId must name the conflicting profile, got {:?}",
            refused.body
        );

        // Leave the store as it was found, so one row cannot shadow the next.
        assert_eq!(
            em.update(&id, json!({ "deletionProtectionEnabled": false }))
                .await
                .status,
            200,
            "case {case}: switching protection off must succeed"
        );
        assert_eq!(
            em.delete(&id).await.status,
            200,
            "case {case}: tearing the profile down must succeed"
        );
    }
}
