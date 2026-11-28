// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the rules this endpoint answers an `@idempotencyToken` reuse, a
//! content create, a tag removal, a recommendation acknowledgement and a `@pattern`
//! violation by, driven over HTTP through the same router the production emulator
//! mounts.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one request:
//!
//! * an idempotency token identifies ONE request, so a create carrying a token a
//!   previous create of the same operation and scope used answers with the resource that
//!   call made only when the request is the SAME, and is refused with the modelled
//!   `ConflictException` (HTTP 409) when any idempotency-relevant member differs. The
//!   table covers every `@idempotencyToken` create this crate implements and, per
//!   create, one row per member a request can differ in;
//! * `CreateContent` finalizes the asset an upload received: an `uploadId` this service
//!   did not issue — never minted, another knowledge base's, one a content already
//!   consumed, one whose URL expired — is refused with the modelled
//!   `ValidationException` (HTTP 400), while a good one persists a content every later
//!   `GetContent` and `ListContents` answers out of, whose asset URL is FOLLOWABLE;
//! * `UntagResource` removes exactly the named keys from the one tag map its resource's
//!   create persisted, for every resource family the tagging surface addresses, leaves
//!   every other key alone, and refuses an ARN that names no resource of this account
//!   with the 4xx not-found answer the recording carries;
//! * `NotifyRecommendationsReceived` acknowledges the ids a session's feed holds —
//!   removing them from it — and reports the ids it does not hold as per-entry `errors`
//!   inside the same 200, while a label this account holds no assistant (or the
//!   assistant no session) under is the modelled `ResourceNotFoundException` (404);
//! * a constraint-violation message states a member's `@pattern` as the service
//!   PUBLISHES it: the whole-value anchors a model wraps the expression in are not part
//!   of the expression, so an expression carrying the pair is quoted without it while
//!   one anchored at a single end is quoted verbatim.
//!
//! Only two things are read through the state seam rather than over the wire: the
//! recommendations a session holds (the service makes one available out of band, so no
//! implemented operation publishes one) and an upload's expiry (backdated so the
//! "the URL is no longer usable" branch is reachable without waiting out a minute of
//! wall clock). Everything else goes over the wire, so a response that is not computed
//! from persisted state cannot pass.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_qconnect_wisdomservice::ops::types::RecommendationData;
use service_com_amazonaws_qconnect_wisdomservice::{state, EmulatedService};
use smithy_server_runtime::MultiServiceRouter;

/// The region every modelled request in this suite is signed for.
const REGION: &str = "eu-central-1";

/// The endpoint host those requests address, and so the host the URLs this suite
/// follows are minted on.
const ENDPOINT_HOST: &str = "wisdom.eu-central-1.amazonaws.com";

/// A UUID no create ever mints, for the not-found assertions. The all-zero id is NOT
/// one: it names the system-managed session every assistant is provisioned with.
const ABSENT_UUID: &str = "ffffffff-ffff-ffff-ffff-ffffffffffff";

/// One media type `StartContentUpload`'s modelled `ContentType` pattern admits
/// (`smithy_query(query="operation",operation="StartContentUpload")`).
const UPLOAD_CONTENT_TYPE: &str = "text/plain";

/// The `@pattern` of `CreateQuickResponse$contentType`
/// (`smithy_query(query="operation",operation="CreateQuickResponse")`), and the
/// expression the service publishes for it: the model's whole-value pair is the wrapping
/// rather than part of the expression.
const QUICK_RESPONSE_TYPE_PATTERN: &str =
    r"^(application/x\.quickresponse;format=(plain|markdown))$";
const QUICK_RESPONSE_TYPE_PUBLISHED: &str = r"(application/x\.quickresponse;format=(plain|markdown))";

/// The `@pattern` of `Name` — `CreateAssistant$name`'s target
/// (`smithy_query(query="operation",operation="CreateAssistant")`) — which is anchored at
/// its START only, so the expression the service publishes is the model's text verbatim.
const NAME_PATTERN: &str = r"^[a-zA-Z0-9\s_.,-]+";

/// The `@pattern` of `StartContentUpload$contentType`, whose pair wraps a top-level
/// alternation, and the expression the service publishes for it.
const UPLOAD_TYPE_PATTERN: &str = r"^(text/(plain|html|csv))|(application/(pdf|vnd\.openxmlformats-officedocument\.wordprocessingml\.document))|(application/x\.wisdom-json;source=(salesforce|servicenow|zendesk))$";
const UPLOAD_TYPE_PUBLISHED: &str = r"(text/(plain|html|csv))|(application/(pdf|vnd\.openxmlformats-officedocument\.wordprocessingml\.document))|(application/x\.wisdom-json;source=(salesforce|servicenow|zendesk))";

/// One wire answer, reduced to what a client branches on.
#[derive(Debug)]
pub(crate) struct Reply {
    pub(crate) status: u16,
    pub(crate) error_type: String,
    pub(crate) body: Value,
}

impl Reply {
    pub(crate) fn string(&self, pointer: &str) -> String {
        self.body
            .pointer(pointer)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }

    pub(crate) fn array(&self, pointer: &str) -> Vec<Value> {
        self.body
            .pointer(pointer)
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
    }

    /// The error code a client branches on: the `x-amzn-errortype` header, falling back
    /// to the body's `__type` for a protocol that carries it only there.
    pub(crate) fn code(&self) -> String {
        if self.error_type.is_empty() {
            self.string("/__type")
        } else {
            self.error_type.clone()
        }
    }

    pub(crate) fn message(&self) -> String {
        self.string("/message")
    }
}

/// A `UuidOrArn` label as it travels in a URL: an ARN's slashes are percent-encoded.
fn encode_label(label: &str) -> String {
    label.replace('/', "%2F")
}

/// An emulator with just this service mounted on a fresh engine, driven in-process
/// through the production router.
pub(crate) struct Emulator {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

impl Emulator {
    pub(crate) fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // Signatures are not what these tests are about; the router still routes by the
        // SigV4 credential scope below, which is also where an ARN's region comes from.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        Self { router, engine }
    }

    pub(crate) async fn send(&self, method: &str, uri: &str, body: Option<Value>) -> Reply {
        let authorization = format!(
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/wisdom/aws4_request, \
             SignedHeaders=host, Signature=not-verified-in-this-suite"
        );
        let builder = http::Request::builder()
            .method(method)
            .uri(format!("https://{ENDPOINT_HOST}{uri}"))
            .header("host", ENDPOINT_HOST)
            .header("authorization", authorization);
        let request = match &body {
            Some(value) => builder
                .header("content-type", "application/json")
                .body(Bytes::from(
                    serde_json::to_vec(value).expect("serialize body"),
                ))
                .expect("build request"),
            None => builder.body(Bytes::new()).expect("build request"),
        };
        self.dispatch(request).await
    }

    /// Follow a URL this service handed out exactly as a client does: `method` against
    /// the absolute URL, with NO credential of any kind.
    async fn follow(&self, method: &str, url: &str, headers: &[(&str, &str)], body: &[u8]) -> Reply {
        let mut builder = http::Request::builder().method(method).uri(url);
        for (name, value) in headers {
            builder = builder.header(*name, *value);
        }
        let request = builder
            .body(Bytes::copy_from_slice(body))
            .expect("build request");
        self.dispatch(request).await
    }

    async fn dispatch(&self, request: http::Request<Bytes>) -> Reply {
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let error_type = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string();
        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string();
        let bytes = response.into_body();
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or_else(|_| {
                // A non-JSON body is an asset the plane served: keep the bytes and the
                // media type it carried, so the assertions can compare both.
                json!({
                    "assetBody": String::from_utf8_lossy(&bytes),
                    "assetContentType": content_type,
                })
            })
        };
        Reply {
            status,
            error_type,
            body,
        }
    }

    pub(crate) async fn assistant(&self, name: &str) -> (String, String) {
        let reply = self
            .send(
                "POST",
                "/assistants",
                Some(json!({ "name": name, "type": "AGENT" })),
            )
            .await;
        assert_eq!(
            reply.status, 200,
            "case \"assistant {name}\": expected HTTP 200, got {} with {}",
            reply.status, reply.body
        );
        (
            reply.string("/assistant/assistantId"),
            reply.string("/assistant/assistantArn"),
        )
    }

    async fn knowledge_base(&self, name: &str, kind: &str) -> (String, String) {
        let reply = self
            .send(
                "POST",
                "/knowledgeBases",
                Some(json!({ "name": name, "knowledgeBaseType": kind })),
            )
            .await;
        assert_eq!(
            reply.status, 200,
            "case \"knowledge base {name}\": expected HTTP 200, got {} with {}",
            reply.status, reply.body
        );
        (
            reply.string("/knowledgeBase/knowledgeBaseId"),
            reply.string("/knowledgeBase/knowledgeBaseArn"),
        )
    }

    pub(crate) async fn session(&self, assistant_id: &str, name: &str) -> (String, String) {
        let reply = self
            .send(
                "POST",
                &format!("/assistants/{assistant_id}/sessions"),
                Some(json!({ "name": name })),
            )
            .await;
        assert_eq!(
            reply.status, 200,
            "case \"session {name}\": expected HTTP 200, got {} with {}",
            reply.status, reply.body
        );
        (
            reply.string("/session/sessionId"),
            reply.string("/session/sessionArn"),
        )
    }

    /// Start an upload under `knowledge_base_id` and write `asset` through the URL it
    /// hands out, so the `uploadId` names bytes this store holds.
    async fn written_upload(&self, knowledge_base_id: &str, asset: &[u8]) -> String {
        let reply = self
            .send(
                "POST",
                &format!("/knowledgeBases/{knowledge_base_id}/upload"),
                Some(json!({ "contentType": UPLOAD_CONTENT_TYPE })),
            )
            .await;
        assert_eq!(
            reply.status, 200,
            "case \"upload under {knowledge_base_id}\": expected HTTP 200, got {} with {}",
            reply.status, reply.body
        );
        let upload_id = reply.string("/uploadId");
        let url = reply.string("/url");
        let put = self
            .follow(
                "PUT",
                &url,
                &[("content-type", UPLOAD_CONTENT_TYPE)],
                asset,
            )
            .await;
        assert_eq!(
            put.status, 200,
            "case \"PUT the asset of {upload_id}\": expected HTTP 200, got {} with {}",
            put.status, put.body
        );
        upload_id
    }

    /// Tag one resource through the modelled tagging route, so the tags under test are
    /// the ones the service persisted.
    async fn tag(&self, resource_arn: &str, tags: Value) {
        let reply = self
            .send(
                "POST",
                &format!("/tags/{}", encode_label(resource_arn)),
                Some(json!({ "tags": tags })),
            )
            .await;
        assert_eq!(
            reply.status, 204,
            "case \"tag {resource_arn}\": expected the no-content HTTP 204, got {} with {}",
            reply.status, reply.body
        );
    }

    /// The tags the service reports for one ARN, read back through
    /// `ListTagsForResource`.
    async fn tags_of(&self, resource_arn: &str) -> Value {
        let reply = self
            .send("GET", &format!("/tags/{}", encode_label(resource_arn)), None)
            .await;
        assert_eq!(
            reply.status, 200,
            "case \"read the tags of {resource_arn}\": expected HTTP 200, got {} with {}",
            reply.status, reply.body
        );
        reply.body.pointer("/tags").cloned().unwrap_or(Value::Null)
    }

    /// Put one recommendation on a session's feed through the state seam — the service
    /// makes a recommendation available out of band, and no implemented operation of this
    /// model publishes one.
    pub(crate) fn publish(&self, assistant_id: &str, session_id: &str, recommendation_id: &str) {
        let store = state::service_state(&*self.engine).expect("the service state");
        let mut store = store.lock();
        let session = store
            .assistants
            .get_mut(assistant_id)
            .and_then(|assistant| assistant.sessions.get_mut(session_id))
            .expect("the session just created");
        session.recommendations.publish(
            RecommendationData {
                recommendation_id: recommendation_id.to_string(),
                relevance_score: 1.0,
                r#type: Some("KNOWLEDGE_CONTENT".to_string()),
                ..RecommendationData::default()
            },
            None,
        );
    }

    /// Backdate one upload's expiry, so the "the URL is no longer usable" branch is
    /// reachable without waiting out a minute of wall clock.
    fn expire_upload(&self, upload_id: &str) {
        let store = state::service_state(&*self.engine).expect("the service state");
        let mut store = store.lock();
        let upload = store
            .content_upload_mut(upload_id)
            .expect("the upload just started");
        upload.url_expiry = SystemTime::now() - Duration::from_secs(1);
    }
}

/// One row of the idempotency table: a create, the request a token was first used for,
/// and a request that differs from it in exactly one member.
struct ChangedRequest {
    case: &'static str,
    body: Value,
}

/// RULE: an idempotency token identifies ONE request. A create carrying a token a
/// previous create of the same operation and scope used answers with the resource that
/// call made when the request is the same, and is refused with the modelled
/// `ConflictException` (409) when any idempotency-relevant member differs — asserted for
/// every `@idempotencyToken` create this crate implements, one row per member the second
/// request can differ in.
#[tokio::test]
async fn a_reused_client_token_replays_the_same_request_and_conflicts_on_a_changed_one() {
    let emulator = Emulator::new();
    let (assistant_id, _) = emulator.assistant("idempotency-parent").await;
    let (knowledge_base_id, _) = emulator
        .knowledge_base("idempotency-parent-kb", "CUSTOM")
        .await;
    let (associated_id, _) = emulator
        .knowledge_base("idempotency-associated-kb", "CUSTOM")
        .await;
    let (other_associated_id, _) = emulator
        .knowledge_base("idempotency-other-associated-kb", "CUSTOM")
        .await;
    let upload_id = emulator.written_upload(&knowledge_base_id, b"asset").await;
    let other_upload_id = emulator
        .written_upload(&knowledge_base_id, b"other asset")
        .await;

    // Each create: where it posts, the first request, the id member its answer names,
    // and the requests that differ from the first in exactly one member.
    let table: Vec<(&str, String, Value, &str, Vec<ChangedRequest>)> = vec![
        (
            "CreateAssistant",
            "/assistants".to_string(),
            json!({
                "name": "idempotent-assistant",
                "type": "AGENT",
                "description": "the first request",
                "tags": { "team": "support", "stage": "one" },
            }),
            "/assistant/assistantId",
            vec![
                ChangedRequest {
                    case: "CreateAssistant with another name",
                    body: json!({ "name": "idempotent-assistant-b", "type": "AGENT",
                                  "description": "the first request",
                                  "tags": { "team": "support", "stage": "one" } }),
                },
                ChangedRequest {
                    case: "CreateAssistant with another description",
                    body: json!({ "name": "idempotent-assistant", "type": "AGENT",
                                  "description": "a second request",
                                  "tags": { "team": "support", "stage": "one" } }),
                },
                ChangedRequest {
                    case: "CreateAssistant with no description",
                    body: json!({ "name": "idempotent-assistant", "type": "AGENT",
                                  "tags": { "team": "support", "stage": "one" } }),
                },
                ChangedRequest {
                    case: "CreateAssistant with another tag value",
                    body: json!({ "name": "idempotent-assistant", "type": "AGENT",
                                  "description": "the first request",
                                  "tags": { "team": "support", "stage": "two" } }),
                },
                ChangedRequest {
                    case: "CreateAssistant with one tag fewer",
                    body: json!({ "name": "idempotent-assistant", "type": "AGENT",
                                  "description": "the first request",
                                  "tags": { "team": "support" } }),
                },
            ],
        ),
        (
            "CreateKnowledgeBase",
            "/knowledgeBases".to_string(),
            json!({
                "name": "idempotent-kb",
                "knowledgeBaseType": "CUSTOM",
                "description": "the first request",
                "tags": { "team": "support" },
            }),
            "/knowledgeBase/knowledgeBaseId",
            vec![
                ChangedRequest {
                    case: "CreateKnowledgeBase with another name",
                    body: json!({ "name": "idempotent-kb-b", "knowledgeBaseType": "CUSTOM",
                                  "description": "the first request",
                                  "tags": { "team": "support" } }),
                },
                ChangedRequest {
                    case: "CreateKnowledgeBase with another type",
                    body: json!({ "name": "idempotent-kb", "knowledgeBaseType": "QUICK_RESPONSES",
                                  "description": "the first request",
                                  "tags": { "team": "support" } }),
                },
                ChangedRequest {
                    case: "CreateKnowledgeBase with another tag key",
                    body: json!({ "name": "idempotent-kb", "knowledgeBaseType": "CUSTOM",
                                  "description": "the first request",
                                  "tags": { "squad": "support" } }),
                },
            ],
        ),
        (
            "CreateSession",
            format!("/assistants/{assistant_id}/sessions"),
            json!({
                "name": "idempotent-session",
                "description": "the first request",
                "tags": { "team": "support" },
            }),
            "/session/sessionId",
            vec![
                ChangedRequest {
                    case: "CreateSession with another name",
                    body: json!({ "name": "idempotent-session-b",
                                  "description": "the first request",
                                  "tags": { "team": "support" } }),
                },
                ChangedRequest {
                    case: "CreateSession with no tags",
                    body: json!({ "name": "idempotent-session",
                                  "description": "the first request" }),
                },
            ],
        ),
        (
            "CreateAssistantAssociation",
            format!("/assistants/{assistant_id}/associations"),
            json!({
                "associationType": "KNOWLEDGE_BASE",
                "association": { "knowledgeBaseId": associated_id },
                "tags": { "team": "support" },
            }),
            "/assistantAssociation/assistantAssociationId",
            vec![
                ChangedRequest {
                    case: "CreateAssistantAssociation with another knowledge base",
                    body: json!({ "associationType": "KNOWLEDGE_BASE",
                                  "association": { "knowledgeBaseId": other_associated_id },
                                  "tags": { "team": "support" } }),
                },
                ChangedRequest {
                    case: "CreateAssistantAssociation with no tags",
                    body: json!({ "associationType": "KNOWLEDGE_BASE",
                                  "association": { "knowledgeBaseId": associated_id } }),
                },
            ],
        ),
        (
            "CreateContent",
            format!("/knowledgeBases/{knowledge_base_id}/contents"),
            json!({
                "name": "idempotent-content",
                "title": "the first request",
                "uploadId": upload_id,
                "metadata": { "source": "one" },
                "tags": { "team": "support" },
            }),
            "/content/contentId",
            vec![
                ChangedRequest {
                    case: "CreateContent with another name",
                    body: json!({ "name": "idempotent-content-b", "title": "the first request",
                                  "uploadId": upload_id, "metadata": { "source": "one" },
                                  "tags": { "team": "support" } }),
                },
                ChangedRequest {
                    case: "CreateContent with another title",
                    body: json!({ "name": "idempotent-content", "title": "a second request",
                                  "uploadId": upload_id, "metadata": { "source": "one" },
                                  "tags": { "team": "support" } }),
                },
                ChangedRequest {
                    case: "CreateContent with another uploadId",
                    body: json!({ "name": "idempotent-content", "title": "the first request",
                                  "uploadId": other_upload_id, "metadata": { "source": "one" },
                                  "tags": { "team": "support" } }),
                },
                ChangedRequest {
                    case: "CreateContent with another metadata value",
                    body: json!({ "name": "idempotent-content", "title": "the first request",
                                  "uploadId": upload_id, "metadata": { "source": "two" },
                                  "tags": { "team": "support" } }),
                },
            ],
        ),
    ];

    for (operation, path, first, id_pointer, changed) in table {
        let token = format!("token-for-{operation}");
        let mut request = first.clone();
        request["clientToken"] = json!(token);

        let created = emulator.send("POST", &path, Some(request.clone())).await;
        assert_eq!(
            created.status, 200,
            "case {operation:?}: the first create must succeed, got {} with {}",
            created.status, created.body
        );
        let created_id = created.string(id_pointer);
        assert!(
            !created_id.is_empty(),
            "case {operation:?}: the create must name the resource it made, got {}",
            created.body
        );

        // The identical request is a RETRY: the same resource, not a second one.
        let replayed = emulator.send("POST", &path, Some(request.clone())).await;
        assert_eq!(
            replayed.status, 200,
            "case {operation:?}: an identical replay must succeed, got {} with {}",
            replayed.status, replayed.body
        );
        assert_eq!(
            replayed.string(id_pointer),
            created_id,
            "case {operation:?}: an identical replay must answer with the first resource, got {}",
            replayed.body
        );

        for ChangedRequest { case, body } in changed {
            let mut conflicting = body.clone();
            conflicting["clientToken"] = json!(token);
            let refused = emulator.send("POST", &path, Some(conflicting)).await;
            assert_eq!(
                refused.status, 409,
                "case {case:?}: expected HTTP 409 for a token reused with a changed request, \
                 got {} with {}",
                refused.status, refused.body
            );
            assert_eq!(
                refused.code(),
                "ConflictException",
                "case {case:?}: expected the modelled ConflictException, got {:?} with {}",
                refused.code(),
                refused.body
            );
            // The refusal must leave the FIRST resource as it was: a replay of the
            // original request still answers with it.
            let after = emulator.send("POST", &path, Some(request.clone())).await;
            assert_eq!(
                after.string(id_pointer),
                created_id,
                "case {case:?}: the refusal must leave the first resource untouched, got {}",
                after.body
            );
        }
    }
}

/// RULE: a `clientToken` is scoped to the create it was given to and, for a nested
/// resource, to its parent — so the same token on another operation, or under another
/// parent, is another create rather than a replay or a conflict.
#[tokio::test]
async fn a_client_token_is_scoped_to_its_create_and_parent() {
    let emulator = Emulator::new();
    let token = "one-token-two-scopes";
    let (first_assistant, _) = emulator.assistant("scope-parent-one").await;
    let (second_assistant, _) = emulator.assistant("scope-parent-two").await;

    let under_first = emulator
        .send(
            "POST",
            &format!("/assistants/{first_assistant}/sessions"),
            Some(json!({ "name": "scoped-session", "clientToken": token })),
        )
        .await;
    let under_second = emulator
        .send(
            "POST",
            &format!("/assistants/{second_assistant}/sessions"),
            Some(json!({ "name": "scoped-session", "clientToken": token })),
        )
        .await;
    assert_eq!(
        under_second.status, 200,
        "case \"the same token under another assistant\": expected HTTP 200, got {} with {}",
        under_second.status, under_second.body
    );
    assert_ne!(
        under_first.string("/session/sessionId"),
        under_second.string("/session/sessionId"),
        "case \"the same token under another assistant\": each parent must get its own session, \
         got {} and {}",
        under_first.body,
        under_second.body
    );

    // The same token on a DIFFERENT create of the same account is a different create
    // too, even with a name that matches nothing it made before.
    let assistant = emulator
        .send(
            "POST",
            "/assistants",
            Some(json!({ "name": "scoped-assistant", "type": "AGENT", "clientToken": token })),
        )
        .await;
    assert_eq!(
        assistant.status, 200,
        "case \"the same token on another operation\": expected HTTP 200, got {} with {}",
        assistant.status, assistant.body
    );
}

/// RULE: `CreateContent` finalizes the asset an upload received, so an `uploadId` this
/// service did not issue for that knowledge base — or no longer holds — names no asset
/// and is refused with the modelled `ValidationException` (HTTP 400).
#[tokio::test]
async fn create_content_refuses_an_upload_id_this_service_does_not_hold() {
    let emulator = Emulator::new();
    let (knowledge_base_id, _) = emulator.knowledge_base("content-kb", "CUSTOM").await;
    let (other_kb_id, _) = emulator.knowledge_base("content-other-kb", "CUSTOM").await;

    let foreign_upload = emulator.written_upload(&other_kb_id, b"foreign").await;
    let consumed_upload = emulator.written_upload(&knowledge_base_id, b"consumed").await;
    let created = emulator
        .send(
            "POST",
            &format!("/knowledgeBases/{knowledge_base_id}/contents"),
            Some(json!({ "name": "consumes-the-upload", "uploadId": consumed_upload })),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "case \"consume an upload\": expected HTTP 200, got {} with {}",
        created.status, created.body
    );
    let expired_upload = emulator.written_upload(&knowledge_base_id, b"expired").await;
    emulator.expire_upload(&expired_upload);

    let table = [
        ("an uploadId this service never issued", format!("bogus-upload-{ABSENT_UUID}")),
        ("an uploadId of another knowledge base", foreign_upload),
        ("an uploadId a content already consumed", consumed_upload),
        ("an uploadId whose URL has expired", expired_upload),
    ];

    for (case, upload_id) in table {
        let refused = emulator
            .send(
                "POST",
                &format!("/knowledgeBases/{knowledge_base_id}/contents"),
                Some(json!({ "name": format!("content-for-{case}"), "uploadId": upload_id })),
            )
            .await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected HTTP 400, got {} with {}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.code(),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {:?} with {}",
            refused.code(),
            refused.body
        );
        assert_eq!(
            refused.message(),
            "Invalid uploadId",
            "case {case:?}: unexpected message in {}",
            refused.body
        );
    }

    // A knowledge base this account does not hold is the modelled not-found, which is a
    // different answer from an upload it does not hold.
    let unknown_parent = emulator
        .send(
            "POST",
            &format!("/knowledgeBases/{ABSENT_UUID}/contents"),
            Some(json!({ "name": "content-of-nothing", "uploadId": "any-upload" })),
        )
        .await;
    assert_eq!(
        unknown_parent.status, 404,
        "case \"an unknown knowledge base\": expected HTTP 404, got {} with {}",
        unknown_parent.status, unknown_parent.body
    );
    assert_eq!(
        unknown_parent.code(),
        "ResourceNotFoundException",
        "case \"an unknown knowledge base\": expected ResourceNotFoundException, got {:?} with {}",
        unknown_parent.code(),
        unknown_parent.body
    );
}

/// RULE: a content a create persisted is the record every later read answers out of —
/// `GetContent` and `ListContents` report it, in either label form, and its asset URL is
/// FOLLOWABLE: a `GET` of it answers the bytes the upload received with the media type
/// the upload was started for.
#[tokio::test]
async fn a_created_content_round_trips_and_its_asset_url_is_followable() {
    let emulator = Emulator::new();
    let (knowledge_base_id, knowledge_base_arn) =
        emulator.knowledge_base("round-trip-kb", "CUSTOM").await;

    // The table spans what a create may leave out: the title (which then equals the
    // name), the metadata and the tags.
    let table: [(&str, &str, Value); 3] = [
        (
            "a content with every member",
            "full-content",
            json!({ "title": "A full content", "metadata": { "source": "crm" },
                    "tags": { "team": "support" } }),
        ),
        ("a content with no title", "untitled-content", json!({})),
        (
            "a content with an overridden link-out URI",
            "linked-content",
            json!({ "overrideLinkOutUri": "https://example.test/articles/1" }),
        ),
    ];

    for (case, name, extra) in table {
        let asset = format!("the asset of {name}").into_bytes();
        let upload_id = emulator.written_upload(&knowledge_base_id, &asset).await;
        let mut body = json!({ "name": name, "uploadId": upload_id });
        for (key, value) in extra.as_object().expect("the extra members are an object") {
            body[key] = value.clone();
        }
        let created = emulator
            .send(
                "POST",
                &format!("/knowledgeBases/{knowledge_base_id}/contents"),
                Some(body),
            )
            .await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected HTTP 200, got {} with {}",
            created.status, created.body
        );
        let content_id = created.string("/content/contentId");
        let content_arn = created.string("/content/contentArn");
        assert_eq!(
            content_arn,
            format!(
                "arn:aws:wisdom:{REGION}:{}:content/{knowledge_base_id}/{content_id}",
                engine::EMULATOR_ACCOUNT_ID
            ),
            "case {case:?}: the content ARN must name its knowledge base and its own id, got {}",
            created.body
        );
        assert_eq!(
            created.string("/content/knowledgeBaseArn"),
            knowledge_base_arn,
            "case {case:?}: the content must report its knowledge base's own ARN, got {}",
            created.body
        );
        // The service answers the create with the ingestion in progress, so that is the
        // status the create reports.
        assert_eq!(
            created.string("/content/status"),
            "CREATE_IN_PROGRESS",
            "case {case:?}: expected the transient create status, got {}",
            created.body
        );
        assert_eq!(
            created.string("/content/contentType"),
            UPLOAD_CONTENT_TYPE,
            "case {case:?}: the content must report the media type its upload was started \
             for, got {}",
            created.body
        );
        assert_eq!(
            created.string("/content/title"),
            extra
                .pointer("/title")
                .and_then(Value::as_str)
                .unwrap_or(name),
            "case {case:?}: a content with no title takes its name as its title, got {}",
            created.body
        );
        assert!(
            !created.string("/content/revisionId").is_empty(),
            "case {case:?}: the content must name its revision, got {}",
            created.body
        );

        // Read back through the modelled read, in both label forms.
        for (label_case, label) in [
            ("by id", content_id.clone()),
            ("by ARN", content_arn.clone()),
        ] {
            let read = emulator
                .send(
                    "GET",
                    &format!(
                        "/knowledgeBases/{knowledge_base_id}/contents/{}",
                        encode_label(&label)
                    ),
                    None,
                )
                .await;
            assert_eq!(
                read.status, 200,
                "case \"{case} read {label_case}\": expected HTTP 200, got {} with {}",
                read.status, read.body
            );
            assert_eq!(
                read.string("/content/contentId"),
                content_id,
                "case \"{case} read {label_case}\": expected the created content, got {}",
                read.body
            );
            assert_eq!(
                read.string("/content/url"),
                created.string("/content/url"),
                "case \"{case} read {label_case}\": the read must report the stored asset URL, \
                 got {}",
                read.body
            );
        }

        // The asset URL is followable and answers the bytes the upload received. It is a
        // URL this emulator SIGNED for the content's own path — see
        // `super::content_upload::assert_signed_plane_url`.
        let url = created.string("/content/url");
        super::content_upload::assert_signed_plane_url(
            case,
            &url,
            &format!("/knowledgeBases/{knowledge_base_id}/contents/{content_id}/asset"),
            state::DEFAULT_PRESIGNED_URL_TIME_TO_LIVE_MINUTES * 60,
            REGION,
        );
        let followed = emulator.follow("GET", &url, &[], b"").await;
        assert_eq!(
            followed.status, 200,
            "case {case:?}: the asset URL must be followable, got {} with {}",
            followed.status, followed.body
        );
        assert_eq!(
            followed.string("/assetBody"),
            String::from_utf8_lossy(&asset),
            "case {case:?}: the asset must be the bytes the upload received, got {}",
            followed.body
        );
        assert_eq!(
            followed.string("/assetContentType"),
            UPLOAD_CONTENT_TYPE,
            "case {case:?}: the asset must carry the upload's media type, got {}",
            followed.body
        );
        // Only a GET reads it.
        let written = emulator.follow("PUT", &url, &[], b"overwrite").await;
        assert_eq!(
            written.status, 405,
            "case {case:?}: the asset URL is read by a GET alone, got {} with {}",
            written.status, written.body
        );
    }

    // Every content is listed, in `contentId` order, and `maxResults` bounds the page.
    let listed = emulator
        .send(
            "GET",
            &format!("/knowledgeBases/{knowledge_base_id}/contents"),
            None,
        )
        .await;
    assert_eq!(
        listed.status, 200,
        "case \"list the contents\": expected HTTP 200, got {} with {}",
        listed.status, listed.body
    );
    let summaries = listed.array("/contentSummaries");
    assert_eq!(
        summaries.len(),
        3,
        "case \"list the contents\": expected the three created contents, got {}",
        listed.body
    );
    let mut ids: Vec<String> = summaries
        .iter()
        .map(|summary| summary["contentId"].as_str().unwrap_or_default().to_string())
        .collect();
    let sorted = {
        let mut sorted = ids.clone();
        sorted.sort();
        sorted
    };
    assert_eq!(
        ids, sorted,
        "case \"list the contents\": the listing must be ordered by contentId, got {ids:?}"
    );
    let first_page = emulator
        .send(
            "GET",
            &format!("/knowledgeBases/{knowledge_base_id}/contents?maxResults=2"),
            None,
        )
        .await;
    assert_eq!(
        first_page.array("/contentSummaries").len(),
        2,
        "case \"the first page of two\": expected two summaries, got {}",
        first_page.body
    );
    let next_token = first_page.string("/nextToken");
    assert!(
        !next_token.is_empty(),
        "case \"the first page of two\": a page that does not end the listing must carry a \
         nextToken, got {}",
        first_page.body
    );
    let second_page = emulator
        .send(
            "GET",
            &format!(
                "/knowledgeBases/{knowledge_base_id}/contents?maxResults=2&nextToken={next_token}"
            ),
            None,
        )
        .await;
    assert_eq!(
        second_page.array("/contentSummaries").len(),
        1,
        "case \"the second page of two\": expected the remaining summary, got {}",
        second_page.body
    );
    ids.sort();

    // A content of ANOTHER knowledge base is not this one's, which is the same
    // not-found as an id nothing was created under.
    let (other_kb_id, _) = emulator.knowledge_base("round-trip-other-kb", "CUSTOM").await;
    for (case, path) in [
        (
            "a content id of another knowledge base",
            format!("/knowledgeBases/{other_kb_id}/contents/{}", ids[0]),
        ),
        (
            "a content id nothing was created under",
            format!("/knowledgeBases/{knowledge_base_id}/contents/{ABSENT_UUID}"),
        ),
    ] {
        let read = emulator.send("GET", &path, None).await;
        assert_eq!(
            read.status, 404,
            "case {case:?}: expected HTTP 404, got {} with {}",
            read.status, read.body
        );
        assert_eq!(
            read.code(),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {:?} with {}",
            read.code(),
            read.body
        );
    }
}

/// RULE: the status a resource the service reports in a non-terminal status moves to is
/// owed by the CLOCK: a content is `CREATE_IN_PROGRESS` while the ingestion the create
/// started is due, and `ACTIVE` once that span has passed — without anything having to
/// call in between.
#[tokio::test]
async fn a_created_content_becomes_active_when_the_clock_makes_it_due() {
    let emulator = Emulator::new();
    let (knowledge_base_id, _) = emulator.knowledge_base("activation-kb", "CUSTOM").await;
    let upload_id = emulator.written_upload(&knowledge_base_id, b"asset").await;
    let created = emulator
        .send(
            "POST",
            &format!("/knowledgeBases/{knowledge_base_id}/contents"),
            Some(json!({ "name": "activating-content", "uploadId": upload_id })),
        )
        .await;
    let content_id = created.string("/content/contentId");
    assert_eq!(
        created.string("/content/status"),
        "CREATE_IN_PROGRESS",
        "case \"straight after the create\": expected the transient status, got {}",
        created.body
    );

    tokio::time::sleep(state::CONTENT_ACTIVATION_DELAY + Duration::from_millis(50)).await;

    let read = emulator
        .send(
            "GET",
            &format!("/knowledgeBases/{knowledge_base_id}/contents/{content_id}"),
            None,
        )
        .await;
    assert_eq!(
        read.string("/content/status"),
        "ACTIVE",
        "case \"after the activation span\": expected ACTIVE, got {}",
        read.body
    );
    let listed = emulator
        .send(
            "GET",
            &format!("/knowledgeBases/{knowledge_base_id}/contents"),
            None,
        )
        .await;
    assert_eq!(
        listed.array("/contentSummaries")[0]["status"]
            .as_str()
            .unwrap_or_default(),
        "ACTIVE",
        "case \"after the activation span, in the listing\": expected ACTIVE, got {}",
        listed.body
    );
}

/// RULE: `UntagResource` removes exactly the named keys from the one tag map its
/// resource's create persisted — for every resource family the tagging surface addresses
/// — leaves every other key alone, and refuses an ARN that names no resource of this
/// account with the 4xx not-found answer the recording carries.
#[tokio::test]
async fn untag_resource_removes_the_named_keys_and_refuses_an_unknown_arn() {
    let emulator = Emulator::new();
    let (assistant_id, assistant_arn) = emulator.assistant("untag-assistant").await;
    let (knowledge_base_id, knowledge_base_arn) =
        emulator.knowledge_base("untag-kb", "CUSTOM").await;
    let (_, session_arn) = emulator.session(&assistant_id, "untag-session").await;
    let (associated_id, _) = emulator.knowledge_base("untag-associated-kb", "CUSTOM").await;
    let association = emulator
        .send(
            "POST",
            &format!("/assistants/{assistant_id}/associations"),
            Some(json!({
                "associationType": "KNOWLEDGE_BASE",
                "association": { "knowledgeBaseId": associated_id },
            })),
        )
        .await;
    let association_arn = association.string("/assistantAssociation/assistantAssociationArn");
    let upload_id = emulator.written_upload(&knowledge_base_id, b"asset").await;
    let content = emulator
        .send(
            "POST",
            &format!("/knowledgeBases/{knowledge_base_id}/contents"),
            Some(json!({ "name": "untag-content", "uploadId": upload_id })),
        )
        .await;
    let content_arn = content.string("/content/contentArn");

    // Every family the tagging surface addresses, each tagged through the modelled
    // route so the map under test is the one the service persisted.
    let table = [
        ("an assistant", assistant_arn.clone()),
        ("a knowledge base", knowledge_base_arn.clone()),
        ("a session", session_arn.clone()),
        ("an assistant association", association_arn.clone()),
        ("a content", content_arn.clone()),
    ];

    for (case, resource_arn) in &table {
        emulator
            .tag(
                resource_arn,
                json!({ "team": "support", "stage": "prod", "owner": "wisdom" }),
            )
            .await;

        // One key removed, the rest untouched — and an unknown key alongside it is not
        // an error: the request asks for a state the resource is already in.
        let removed = emulator
            .send(
                "DELETE",
                &format!(
                    "/tags/{}?tagKeys=stage&tagKeys=absent",
                    encode_label(resource_arn)
                ),
                None,
            )
            .await;
        assert_eq!(
            removed.status, 204,
            "case \"untag {case}\": expected the no-content HTTP 204, got {} with {}",
            removed.status, removed.body
        );
        let tags = emulator.tags_of(resource_arn).await;
        assert_eq!(
            tags,
            json!({ "team": "support", "owner": "wisdom" }),
            "case \"untag {case}\": only the named keys may go, got {tags}"
        );

        // Removing every remaining key leaves the resource with an empty tag map, not
        // with a resource that stopped existing.
        let cleared = emulator
            .send(
                "DELETE",
                &format!(
                    "/tags/{}?tagKeys=team&tagKeys=owner",
                    encode_label(resource_arn)
                ),
                None,
            )
            .await;
        assert_eq!(
            cleared.status, 204,
            "case \"untag {case} entirely\": expected the no-content HTTP 204, got {} with {}",
            cleared.status, cleared.body
        );
        let tags = emulator.tags_of(resource_arn).await;
        assert_eq!(
            tags,
            json!({}),
            "case \"untag {case} entirely\": expected an empty tag map, got {tags}"
        );
    }

    // An ARN that matches the modelled pattern but names no resource of this account:
    // the 4xx not-found answer the recording carries, for every family.
    for (case, resource_arn) in &table {
        let unknown = replace_last_id(resource_arn, ABSENT_UUID);
        let refused = emulator
            .send(
                "DELETE",
                &format!("/tags/{}?tagKeys=team", encode_label(&unknown)),
                None,
            )
            .await;
        assert_eq!(
            refused.status, 404,
            "case \"untag an unknown ARN of {case}\": expected HTTP 404, got {} with {}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.code(),
            "NotFoundException",
            "case \"untag an unknown ARN of {case}\": unexpected error code {:?} in {}",
            refused.code(),
            refused.body
        );
        assert_eq!(
            refused.message(),
            "Resource was not found",
            "case \"untag an unknown ARN of {case}\": unexpected message in {}",
            refused.body
        );
    }
}

/// The ARN that names another resource of the same family: the last id segment replaced.
fn replace_last_id(resource_arn: &str, id: &str) -> String {
    match resource_arn.rsplit_once('/') {
        Some((prefix, _)) => format!("{prefix}/{id}"),
        None => resource_arn.to_string(),
    }
}

/// RULE: `NotifyRecommendationsReceived` acknowledges the ids the session's feed holds —
/// removing them from it, which is what the acknowledgement DOES — and reports every id
/// it does not hold as a per-entry error inside the same successful answer; a label this
/// account holds no assistant under, or that assistant holds no session under, is the
/// modelled `ResourceNotFoundException` (404).
#[tokio::test]
async fn notify_recommendations_received_acknowledges_what_the_session_holds() {
    let emulator = Emulator::new();
    let (assistant_id, _) = emulator.assistant("notify-assistant").await;
    let (session_id, session_arn) = emulator.session(&assistant_id, "notify-session").await;
    let available: Vec<String> = (0..3)
        .map(|index| format!("1111111{index}-1111-1111-1111-111111111111"))
        .collect();
    for recommendation_id in &available {
        emulator.publish(&assistant_id, &session_id, recommendation_id);
    }

    // A mix of ids the feed holds and ids it does not: each is answered in its own list,
    // in the order the request named them.
    let unknown = format!("2222222a-2222-2222-2222-{}", "2".repeat(12));
    let notified = emulator
        .send(
            "POST",
            &format!(
                "/assistants/{assistant_id}/sessions/{session_id}/recommendations/notify"
            ),
            Some(json!({ "recommendationIds": [available[0], unknown, available[1]] })),
        )
        .await;
    assert_eq!(
        notified.status, 200,
        "case \"a mix of known and unknown ids\": expected HTTP 200, got {} with {}",
        notified.status, notified.body
    );
    assert_eq!(
        notified.array("/recommendationIds"),
        vec![json!(available[0]), json!(available[1])],
        "case \"a mix of known and unknown ids\": expected the acknowledged ids, got {}",
        notified.body
    );
    let errors = notified.array("/errors");
    assert_eq!(
        errors.len(),
        1,
        "case \"a mix of known and unknown ids\": expected one per-entry error, got {}",
        notified.body
    );
    assert_eq!(
        errors[0]["recommendationId"].as_str().unwrap_or_default(),
        unknown,
        "case \"a mix of known and unknown ids\": the entry must name the id it is about, got {}",
        notified.body
    );
    assert!(
        !errors[0]["message"].as_str().unwrap_or_default().is_empty(),
        "case \"a mix of known and unknown ids\": the entry must carry a message, got {}",
        notified.body
    );

    // The acknowledged ids are GONE from the feed, and the one nobody acknowledged is
    // still available.
    let remaining = emulator
        .send(
            "GET",
            &format!(
                "/assistants/{assistant_id}/sessions/{session_id}/recommendations?waitTimeSeconds=0"
            ),
            None,
        )
        .await;
    let served: Vec<String> = remaining
        .array("/recommendations")
        .iter()
        .map(|recommendation| {
            recommendation["recommendationId"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect();
    assert_eq!(
        served,
        vec![available[2].clone()],
        "case \"after the acknowledgement\": only the unacknowledged recommendation may remain, \
         got {}",
        remaining.body
    );

    // Acknowledging nothing is a successful answer with nothing in either list, and an
    // id already acknowledged is one the feed no longer holds. The session is addressed
    // by ARN here, the other label form the model's `UuidOrArn` admits.
    let session_label = encode_label(&session_arn);
    let empty = emulator
        .send(
            "POST",
            &format!(
                "/assistants/{assistant_id}/sessions/{session_label}/recommendations/notify"
            ),
            Some(json!({ "recommendationIds": [] })),
        )
        .await;
    assert_eq!(
        empty.status, 200,
        "case \"an empty acknowledgement\": expected HTTP 200, got {} with {}",
        empty.status, empty.body
    );
    assert!(
        empty.array("/recommendationIds").is_empty() && empty.array("/errors").is_empty(),
        "case \"an empty acknowledgement\": expected both lists empty, got {}",
        empty.body
    );
    let again = emulator
        .send(
            "POST",
            &format!(
                "/assistants/{assistant_id}/sessions/{session_id}/recommendations/notify"
            ),
            Some(json!({ "recommendationIds": [available[0]] })),
        )
        .await;
    assert_eq!(
        again.array("/errors").len(),
        1,
        "case \"an id already acknowledged\": expected it to be reported as an error, got {}",
        again.body
    );

    // The hard refusals: an assistant this account does not hold, and a session that
    // assistant does not hold.
    let table = [
        (
            "an assistant this account does not hold",
            format!("/assistants/{ABSENT_UUID}/sessions/{session_id}/recommendations/notify"),
            "Assistant does not exist.",
        ),
        (
            "a session the assistant does not hold",
            format!("/assistants/{assistant_id}/sessions/{ABSENT_UUID}/recommendations/notify"),
            "Session does not exist.",
        ),
    ];
    for (case, path, message) in table {
        let refused = emulator
            .send("POST", &path, Some(json!({ "recommendationIds": [] })))
            .await;
        assert_eq!(
            refused.status, 404,
            "case {case:?}: expected HTTP 404, got {} with {}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.code(),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {:?} with {}",
            refused.code(),
            refused.body
        );
        assert_eq!(
            refused.message(),
            message,
            "case {case:?}: unexpected message in {}",
            refused.body
        );
    }
}

/// RULE: a constraint-violation message states a member's `@pattern` as the service
/// PUBLISHES it. A model expresses a whole-value match by wrapping the service's
/// expression in `^`/`$`, so an expression carrying that pair is quoted without it, while
/// one anchored at a single end is quoted verbatim — for every `@pattern`-constrained
/// member of the service, not only the one a recording exercised.
#[tokio::test]
async fn a_pattern_violation_states_the_published_expression() {
    let emulator = Emulator::new();
    let (knowledge_base_id, _) = emulator.knowledge_base("pattern-kb", "QUICK_RESPONSES").await;
    let (custom_kb_id, _) = emulator.knowledge_base("pattern-custom-kb", "CUSTOM").await;

    // Each row: the request, the member the walk names, the value it echoes, and the
    // expression the service publishes for that member's pattern.
    let table: [(&str, &str, String, Value, &str, &str, &str); 3] = [
        (
            "CreateQuickResponse with a contentType outside its pattern",
            "POST",
            format!("/knowledgeBases/{knowledge_base_id}/quickResponses"),
            json!({
                "name": "a quick response",
                "content": { "content": "irrelevant" },
                "contentType": "application/json",
            }),
            "contentType",
            "application/json",
            QUICK_RESPONSE_TYPE_PUBLISHED,
        ),
        (
            "StartContentUpload with a contentType outside its pattern",
            "POST",
            format!("/knowledgeBases/{custom_kb_id}/upload"),
            json!({ "contentType": "application/json" }),
            "contentType",
            "application/json",
            UPLOAD_TYPE_PUBLISHED,
        ),
        (
            "CreateAssistant with a name outside its pattern",
            "POST",
            "/assistants".to_string(),
            json!({ "name": "!not a name", "type": "AGENT" }),
            "name",
            "!not a name",
            // Anchored at its START only, so the published expression is the model's
            // text verbatim: a single anchor is part of the expression.
            NAME_PATTERN,
        ),
    ];

    for (case, method, path, body, member, value, published) in table {
        let refused = emulator.send(method, &path, Some(body)).await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected HTTP 400, got {} with {}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.code(),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {:?} with {}",
            refused.code(),
            refused.body
        );
        assert_eq!(
            refused.message(),
            format!(
                "1 validation error detected: Value '{value}' at '{member}' failed to satisfy \
                 constraint: Member must satisfy regular expression pattern: {published}"
            ),
            "case {case:?}: unexpected message in {}",
            refused.body
        );
    }

    // The model's own text still carries the anchors: it is the RENDERING that states
    // the published expression, not the check, which runs the pattern as written.
    assert!(
        QUICK_RESPONSE_TYPE_PATTERN.starts_with('^')
            && QUICK_RESPONSE_TYPE_PATTERN.ends_with('$')
            && UPLOAD_TYPE_PATTERN.starts_with('^')
            && UPLOAD_TYPE_PATTERN.ends_with('$'),
        "the model texts this suite quotes must be the anchored ones"
    );

    // Two violations in one request: both clauses are stated, each with its own
    // published expression, under the count the envelope opens with. The second
    // violation is the `knowledgeBaseId` path label's own pattern, so the rule is seen to
    // hold for a member that travels in the URI as well as one in the body.
    let two = emulator
        .send(
            "POST",
            "/knowledgeBases/not-a-knowledge-base-label/quickResponses",
            Some(json!({
                "name": "a quick response",
                "content": { "content": "irrelevant" },
                "contentType": "application/json",
            })),
        )
        .await;
    assert_eq!(
        two.status, 400,
        "case \"two violations in one request\": expected HTTP 400, got {} with {}",
        two.status, two.body
    );
    assert!(
        two.message().starts_with("2 validation errors detected: "),
        "case \"two violations in one request\": the envelope must state the count, got {}",
        two.body
    );
    assert!(
        two.message().contains(&format!(
            "Member must satisfy regular expression pattern: {QUICK_RESPONSE_TYPE_PUBLISHED}"
        )),
        "case \"two violations in one request\": the pattern clause must state the published \
         expression, got {}",
        two.body
    );
    assert!(
        !two.message().contains(QUICK_RESPONSE_TYPE_PATTERN),
        "case \"two violations in one request\": no clause may state the anchored model text, \
         got {}",
        two.body
    );
}
