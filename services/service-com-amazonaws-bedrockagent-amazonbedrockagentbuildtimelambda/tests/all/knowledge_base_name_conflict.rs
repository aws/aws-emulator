// SPDX-License-Identifier: CC0-1.0

//! A knowledge base's NAME as the account's own namespace, end to end over HTTP.
//!
//! RULE: a knowledge base's name is this account's to hold ONCE — a create under a name
//! another knowledge base of the account already has is refused with the modelled
//! `ConflictException` and writes nothing, and so is a rename an update would move onto such
//! a name; a knowledge base on its way out of the store no longer holds its name, so the
//! name is free again once its removal comes due.
//!
//! The rows span what the rule generalises over: the form of the name the collision is on
//! (letters, digits, both separators the modelled `Name` admits, its longest form), the call
//! that collides (a create and an update), the name a knowledge base re-sends for itself
//! (which is no collision), and the status the holder is in (held, and on its way out).
//!
//! Every assertion goes through the router, so it exercises the real wire path.

use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_bedrockagent_amazonbedrockagentbuildtimelambda::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 `Authorization` header scoped to the `bedrock` signing name.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/eu-west-1/bedrock/aws4_request, \
     SignedHeaders=host;x-amz-date, Signature=0123456789abcdef";

/// A role ARN matching the modelled `KnowledgeBaseRoleArn` pattern.
const KB_ROLE: &str =
    "arn:aws:iam::000000000000:role/service-role/AmazonBedrockExecutionRoleForKnowledgeBase";

/// An embedding model ARN naming a model whose published output is an embedding.
const EMBEDDING_MODEL: &str =
    "arn:aws:bedrock:eu-west-1::foundation-model/amazon.titan-embed-text-v2:0";

/// How long the removal of a knowledge base is given to come through, and how often it is
/// read while it does.
///
/// The deadline is deliberately far past the span the store owes the removal: what is
/// asserted is that the removal happens, not how fast the host runs.
const REMOVAL_DEADLINE: Duration = Duration::from_secs(30);
const REMOVAL_POLL_INTERVAL: Duration = Duration::from_millis(50);

/// One HTTP answer from the emulator, parsed.
#[derive(Debug)]
struct Answer {
    status: u16,
    error_type: Option<String>,
    body: Value,
}

impl Answer {
    fn member(&self, path: &[&str]) -> &Value {
        let mut at = &self.body;
        for key in path {
            at = &at[*key];
        }
        at
    }

    fn string(&self, path: &[&str]) -> String {
        self.member(path)
            .as_str()
            .unwrap_or_else(|| panic!("{path:?} is not a string in {:?}", self.body))
            .to_string()
    }
}

/// The emulator under test: one router over one shared engine.
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

    async fn call(&self, method: &str, uri: &str, body: Option<Value>) -> Answer {
        let bytes = body
            .map(|value| Bytes::from(serde_json::to_vec(&value).expect("serialize request body")))
            .unwrap_or_default();
        let request = http::Request::builder()
            .method(method)
            .uri(uri)
            .header("host", "bedrock-agent.eu-west-1.amazonaws.com")
            .header("authorization", AUTHORIZATION)
            .header("content-type", "application/json")
            .body(bytes)
            .expect("build the request");

        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let header_error = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .map(ToString::to_string);
        let raw = response.into_body();
        let body: Value = if raw.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&raw).unwrap_or_else(|error| {
                panic!(
                    "{method} {uri} -> {status} body is not JSON ({error}): {}",
                    String::from_utf8_lossy(&raw)
                )
            })
        };
        let error_type = header_error.or_else(|| {
            body.get("__type")
                .and_then(Value::as_str)
                .map(ToString::to_string)
        });
        Answer {
            status,
            error_type,
            body,
        }
    }

    async fn create(&self, name: &str) -> Answer {
        self.call("PUT", "/knowledgebases/", Some(request_body(name)))
            .await
    }

    async fn created(&self, case: &str, name: &str) -> String {
        let created = self.create(name).await;
        assert_eq!(
            created.status, 202,
            "case {case}: expected CreateKnowledgeBase -> 202, got {created:?}"
        );
        created.string(&["knowledgeBase", "knowledgeBaseId"])
    }

    async fn update(&self, knowledge_base_id: &str, name: &str) -> Answer {
        self.call(
            "PUT",
            &format!("/knowledgebases/{knowledge_base_id}"),
            Some(request_body(name)),
        )
        .await
    }

    async fn get(&self, knowledge_base_id: &str) -> Answer {
        self.call("GET", &format!("/knowledgebases/{knowledge_base_id}"), None)
            .await
    }

    async fn delete(&self, knowledge_base_id: &str) -> Answer {
        self.call(
            "DELETE",
            &format!("/knowledgebases/{knowledge_base_id}"),
            None,
        )
        .await
    }

    /// Read the knowledge base back until this store no longer holds it, and report the
    /// status of the last read.
    ///
    /// The removal is a transition the CLOCK owes, materialised by whichever reader comes
    /// first, so the test waits for THAT condition rather than for a fixed span: it looks
    /// again until the read answers 404 or [`REMOVAL_DEADLINE`] passes.
    async fn poll_until_gone(&self, knowledge_base_id: &str) -> u16 {
        let deadline = Instant::now() + REMOVAL_DEADLINE;
        loop {
            let read = self.get(knowledge_base_id).await;
            if read.status == 404 || Instant::now() >= deadline {
                return read.status;
            }
            tokio::time::sleep(REMOVAL_POLL_INTERVAL).await;
        }
    }

    /// The name the store reports for one knowledge base.
    async fn reported_name(&self, case: &str, knowledge_base_id: &str) -> String {
        let fetched = self.get(knowledge_base_id).await;
        assert_eq!(
            fetched.status, 200,
            "case {case}: expected GetKnowledgeBase -> 200, got {fetched:?}"
        );
        fetched.string(&["knowledgeBase", "name"])
    }
}

/// A `CreateKnowledgeBase`/`UpdateKnowledgeBase` body for a self-managed vector knowledge
/// base under `name`. Every member but the name is the same across the rows, so the name is
/// the only thing a collision can be about.
fn request_body(name: &str) -> Value {
    json!({
        "name": name,
        "roleArn": KB_ROLE,
        "knowledgeBaseConfiguration": {
            "type": "VECTOR",
            "vectorKnowledgeBaseConfiguration": { "embeddingModelArn": EMBEDDING_MODEL },
        },
        "storageConfiguration": {
            "type": "OPENSEARCH_SERVERLESS",
            "opensearchServerlessConfiguration": {
                "collectionArn": "arn:aws:aoss:eu-west-1:000000000000:collection/name-conflict",
                "vectorIndexName": "name-conflict-index",
                "fieldMapping": {
                    "vectorField": "bedrock-knowledge-base-default-vector",
                    "textField": "AMAZON_BEDROCK_TEXT_CHUNK",
                    "metadataField": "AMAZON_BEDROCK_METADATA",
                },
            },
        },
    })
}

/// Assert one refusal: the modelled `ConflictException`, naming the operation and the name
/// it was asked for.
fn assert_conflict(case: &str, operation: &str, name: &str, refused: &Answer) {
    assert_eq!(
        refused.status, 409,
        "case {case}: expected {operation} -> 409, got {refused:?}"
    );
    assert_eq!(
        refused.error_type.as_deref(),
        Some("ConflictException"),
        "case {case}: expected the modelled ConflictException, got {refused:?}"
    );
    let message = refused.string(&["message"]);
    assert!(
        message.contains(operation) && message.contains(name),
        "case {case}: expected the refusal to name the operation and the name it was asked \
         for, got {message:?}"
    );
}

/// A create under a name this account's knowledge bases already hold is refused and writes
/// nothing, whatever form the name takes.
#[tokio::test]
async fn a_create_under_a_held_name_is_refused_and_writes_nothing() {
    let emulator = Emulator::new();

    // Rows: the form of the modelled `Name` (`^([0-9a-zA-Z][_-]?){1,100}$`) the collision is
    // on — letters, digits, each separator it admits, a single character and its longest
    // form.
    let rows: Vec<(&str, String)> = vec![
        ("a plain name", "conflict-kb".to_string()),
        ("a name with digits", "conflict9kb9".to_string()),
        ("a name with an underscore", "conflict_kb".to_string()),
        ("a single-character name", "c".to_string()),
        ("a name at the pattern's longest form", "c".repeat(100)),
    ];

    for (case, name) in &rows {
        let held = emulator.created(case, name).await;

        let refused = emulator.create(name).await;
        assert_conflict(case, "CreateKnowledgeBase", name, &refused);

        // Nothing was written: the knowledge base holding the name is the only one under it,
        // and it is exactly as it was.
        let reported = emulator.reported_name(case, &held).await;
        assert_eq!(
            &reported, name,
            "case {case}: expected the holder to keep its name, got {reported:?}"
        );
        let listed = emulator
            .call("POST", "/knowledgebases/", Some(json!({})))
            .await;
        let under_the_name = listed
            .member(&["knowledgeBaseSummaries"])
            .as_array()
            .unwrap_or_else(|| panic!("case {case}: expected a listing, got {listed:?}"))
            .iter()
            .filter(|summary| summary["name"].as_str() == Some(name.as_str()))
            .count();
        assert_eq!(
            under_the_name, 1,
            "case {case}: expected exactly one knowledge base under {name:?}, got {listed:?}"
        );
    }
}

/// An update may rename a knowledge base, but not onto a name another one holds — and
/// re-sending a knowledge base's own name is no collision.
#[tokio::test]
async fn an_update_may_not_rename_a_knowledge_base_onto_a_held_name() {
    let emulator = Emulator::new();
    let held = emulator.created("the holder", "rename-kb-held").await;
    let renamed = emulator.created("the renamed", "rename-kb-other").await;

    // Re-sending its own name is the no-op rename it is, and so is a name nothing holds.
    for (case, name) in [
        ("its own name", "rename-kb-other"),
        ("a free name", "rename-kb-free"),
        ("another free name", "rename_kb_free_2"),
    ] {
        let updated = emulator.update(&renamed, name).await;
        assert_eq!(
            updated.status, 202,
            "case {case}: expected UpdateKnowledgeBase -> 202, got {updated:?}"
        );
        let reported = emulator.reported_name(case, &renamed).await;
        assert_eq!(
            reported, name,
            "case {case}: expected the update to leave the name it asked for, got {reported:?}"
        );
    }

    // The name the other knowledge base holds is refused, and neither record changed.
    let refused = emulator.update(&renamed, "rename-kb-held").await;
    assert_conflict(
        "a rename onto a held name",
        "UpdateKnowledgeBase",
        "rename-kb-held",
        &refused,
    );
    assert_eq!(
        emulator
            .reported_name("a rename onto a held name", &renamed)
            .await,
        "rename_kb_free_2",
        "case a rename onto a held name: expected the refused update to write nothing"
    );
    assert_eq!(
        emulator
            .reported_name("a rename onto a held name", &held)
            .await,
        "rename-kb-held",
        "case a rename onto a held name: expected the holder to be untouched"
    );
}

/// A knowledge base a delete has been accepted for is on its way out of the store, so it no
/// longer holds its name: the create that waited for it is served once the removal the clock
/// owes comes due.
#[tokio::test]
async fn a_deleted_knowledge_base_releases_its_name() {
    let emulator = Emulator::new();
    let name = "released-kb";
    let first = emulator.created("the first holder", name).await;

    let accepted = emulator.delete(&first).await;
    assert_eq!(
        accepted.status, 202,
        "case the accepted delete: expected DeleteKnowledgeBase -> 202, got {accepted:?}"
    );

    // The name is free as soon as the delete is accepted — the knowledge base is on its way
    // out — so the create under it is served and mints its own knowledge base.
    let second = emulator.created("the second holder", name).await;
    assert_ne!(
        second, first,
        "case the second holder: expected a knowledge base of its own, got {second:?}"
    );

    // Once the removal the clock owes comes due, the first is gone and the second still
    // holds the name. The wait is for the CONDITION — the knowledge base no longer being
    // held — under a deadline generous enough for any host.
    let gone = emulator.poll_until_gone(&first).await;
    assert_eq!(
        gone, 404,
        "case after the removal came due: expected the first knowledge base to be gone, got \
         {gone}"
    );
    assert_eq!(
        emulator
            .reported_name("after the removal came due", &second)
            .await,
        name,
        "case after the removal came due: expected the second holder to keep the name"
    );
}
