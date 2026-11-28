// SPDX-License-Identifier: CC0-1.0

//! How this service WORDS the `ResourceNotFoundException` it answers for a
//! resource it addresses by that resource's own identifier, end to end over HTTP.
//!
//! RULE: the refusal is the sentence "<ResourceType> with id <id> is not found.",
//! whose only variable parts are the type of the resource the refusal is reported
//! against — spelled as the model spells the shape — and the identifier the
//! request carried for THAT resource. The identifiers of the resources enclosing
//! it are no part of the sentence, however many of them the request had to carry
//! to address it.
//!
//! Every assertion here goes through the router, so it exercises the real wire
//! path (route matching, model constraint validation, serialization) rather than
//! calling a handler directly.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_bedrockagent_amazonbedrockagentbuildtimelambda::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 `Authorization` header scoped to the `bedrock` signing name: the
/// routing key this service claims on.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/eu-west-1/bedrock/aws4_request, \
     SignedHeaders=host;x-amz-date, Signature=0123456789abcdef";

/// A role ARN matching the modelled `KnowledgeBaseRoleArn` pattern.
const KB_ROLE: &str =
    "arn:aws:iam::000000000000:role/service-role/AmazonBedrockExecutionRoleForKnowledgeBase";

/// An embedding model ARN matching the modelled `BedrockEmbeddingModelArn`.
const EMBEDDING_MODEL: &str =
    "arn:aws:bedrock:eu-west-1::foundation-model/amazon.titan-embed-text-v2:0";

/// The vector store the knowledge bases of this suite bind to.
const VECTOR_BUCKET: &str = "arn:aws:s3vectors:::bucket/wording-demo-vectors";
const INDEX: &str = "wording-demo-index";

/// The missing-resource error of this service and the status its model binds to
/// it (`smithy_query(query="operation",operation="StopIngestionJob")`).
const NOT_FOUND: (&str, u16) = ("ResourceNotFoundException", 404);

/// Identifiers of the modelled `Id` shape (`^[0-9a-zA-Z]{10}$`) that no record of
/// this suite carries, so each is a resource the store cannot resolve.
const ABSENT_IDS: [&str; 3] = ["0123456789", "aZ09aZ09aZ", "NoSuchIdAA"];

/// The sentence this service words for a resource it addresses by that resource's
/// own id and cannot resolve.
fn not_found_sentence(resource_type: &str, resource_id: &str) -> String {
    format!("{resource_type} with id {resource_id} is not found.")
}

/// One HTTP answer from the emulator, parsed.
#[derive(Debug)]
struct Answer {
    status: u16,
    /// The restJson1 error shape name, from `x-amzn-errortype` or the body's
    /// `__type`; `None` on a success.
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

    /// The refusal's `message` member, or the empty string on an answer carrying
    /// none.
    fn message(&self) -> String {
        self.body["message"].as_str().unwrap_or_default().to_string()
    }
}

/// The emulator under test: one router over one shared engine, exactly as the
/// production router mounts this service.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        // The knowledge bases here bind to an S3 Vectors index, which
        // `CreateKnowledgeBase` resolves in the service that owns it — so the
        // index is created first, as a caller creates it first against AWS.
        crate::vector_store_fixture::seed_index(
            &engine,
            VECTOR_BUCKET,
            INDEX,
            crate::vector_store_fixture::EMBEDDING_DIMENSION,
        );
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

    /// Create a knowledge base bound to [`INDEX`] and answer its minted id.
    async fn knowledge_base(&self, name: &str) -> String {
        let created = self
            .call(
                "PUT",
                "/knowledgebases/",
                Some(json!({
                    "name": name,
                    "roleArn": KB_ROLE,
                    "knowledgeBaseConfiguration": {
                        "type": "VECTOR",
                        "vectorKnowledgeBaseConfiguration": {
                            "embeddingModelArn": EMBEDDING_MODEL,
                        },
                    },
                    "storageConfiguration": {
                        "type": "S3_VECTORS",
                        "s3VectorsConfiguration": {
                            "vectorBucketArn": VECTOR_BUCKET,
                            "indexName": INDEX,
                        },
                    },
                })),
            )
            .await;
        assert_eq!(
            created.status, 202,
            "case setup: expected CreateKnowledgeBase -> 202, got {created:?}"
        );
        created.string(&["knowledgeBase", "knowledgeBaseId"])
    }

    /// Attach a CUSTOM data source to `knowledge_base_id` and answer its minted
    /// id: the kind whose documents are pushed in, so nothing else has to exist
    /// for a job over it.
    async fn data_source(&self, knowledge_base_id: &str, name: &str) -> String {
        let created = self
            .call(
                "PUT",
                &format!("/knowledgebases/{knowledge_base_id}/datasources/"),
                Some(json!({
                    "name": name,
                    "dataSourceConfiguration": { "type": "CUSTOM" },
                })),
            )
            .await;
        assert_eq!(
            created.status, 200,
            "case setup: expected CreateDataSource -> 200, got {created:?}"
        );
        created.string(&["dataSource", "dataSourceId"])
    }

    /// Start an ingestion job over the pair and answer its minted id.
    async fn start_ingestion_job(&self, knowledge_base_id: &str, data_source_id: &str) -> String {
        let started = self
            .call(
                "PUT",
                &format!(
                    "/knowledgebases/{knowledge_base_id}/datasources/{data_source_id}\
                     /ingestionjobs/"
                ),
                Some(json!({})),
            )
            .await;
        assert_eq!(
            started.status, 202,
            "case setup: expected StartIngestionJob -> 202, got {started:?}"
        );
        started.string(&["ingestionJob", "ingestionJobId"])
    }

    async fn get_ingestion_job(
        &self,
        knowledge_base_id: &str,
        data_source_id: &str,
        ingestion_job_id: &str,
    ) -> Answer {
        self.call(
            "GET",
            &format!(
                "/knowledgebases/{knowledge_base_id}/datasources/{data_source_id}\
                 /ingestionjobs/{ingestion_job_id}"
            ),
            None,
        )
        .await
    }

    async fn stop_ingestion_job(
        &self,
        knowledge_base_id: &str,
        data_source_id: &str,
        ingestion_job_id: &str,
    ) -> Answer {
        self.call(
            "POST",
            &format!(
                "/knowledgebases/{knowledge_base_id}/datasources/{data_source_id}\
                 /ingestionjobs/{ingestion_job_id}/stop"
            ),
            None,
        )
        .await
    }

    async fn get_knowledge_base(&self, knowledge_base_id: &str) -> Answer {
        self.call("GET", &format!("/knowledgebases/{knowledge_base_id}"), None)
            .await
    }

    async fn get_data_source(&self, knowledge_base_id: &str, data_source_id: &str) -> Answer {
        self.call(
            "GET",
            &format!("/knowledgebases/{knowledge_base_id}/datasources/{data_source_id}"),
            None,
        )
        .await
    }
}

/// Assert `answer` is this service's missing-resource refusal worded for
/// `resource_type` and `resource_id`, and that it names nothing else.
fn assert_not_found(case: &str, resource_type: &str, resource_id: &str, answer: &Answer) {
    let (code, status) = NOT_FOUND;
    assert_eq!(
        answer.status, status,
        "case {case:?}: expected HTTP {status}, got {answer:?}"
    );
    assert_eq!(
        answer.error_type.as_deref(),
        Some(code),
        "case {case:?}: expected {code}, got {answer:?}"
    );
    let expected = not_found_sentence(resource_type, resource_id);
    assert_eq!(
        answer.message(),
        expected,
        "case {case:?}: expected the refusal worded {expected:?}, got {:?}",
        answer.message()
    );
}

/// RULE: an ingestion-job refusal names the JOB that could not be resolved — its
/// type and the `ingestionJobId` the request path carried — and neither the
/// knowledge base nor the data source whose ids the path also carried.
#[tokio::test]
async fn an_ingestion_job_refusal_names_only_the_job_it_could_not_resolve() {
    let emulator = Emulator::new();
    let knowledge_base_id = emulator.knowledge_base("wording-ingestion-kb").await;
    let first_source = emulator
        .data_source(&knowledge_base_id, "wording-source-1")
        .await;
    let second_source = emulator
        .data_source(&knowledge_base_id, "wording-source-2")
        .await;
    let stored_job = emulator
        .start_ingestion_job(&knowledge_base_id, &first_source)
        .await;

    // One row per way the job id can resolve to nothing: an id nothing holds (in
    // each of the forms the modelled `Id` admits), an id differing from the
    // stored one in its last character only, and the stored id addressed through
    // ANOTHER data source of the same knowledge base.
    let mut nearly = stored_job.clone();
    nearly.pop();
    nearly.push(if stored_job.ends_with('X') { 'Y' } else { 'X' });
    let mut rows: Vec<(String, &str, String)> = ABSENT_IDS
        .iter()
        .map(|absent| {
            (
                format!("a job id nothing holds ({absent})"),
                first_source.as_str(),
                (*absent).to_string(),
            )
        })
        .collect();
    rows.push((
        "an id differing from the stored one in its last character".to_string(),
        first_source.as_str(),
        nearly,
    ));
    rows.push((
        "the stored id addressed through another data source".to_string(),
        second_source.as_str(),
        stored_job.clone(),
    ));

    for (row, data_source_id, requested_job) in rows {
        // Both operations that address one job by the triple word the same
        // refusal, so each row is driven through both.
        for operation in ["GetIngestionJob", "StopIngestionJob"] {
            let case = format!("{operation} with {row}");
            let answer = match operation {
                "GetIngestionJob" => {
                    emulator
                        .get_ingestion_job(&knowledge_base_id, data_source_id, &requested_job)
                        .await
                }
                _ => {
                    emulator
                        .stop_ingestion_job(&knowledge_base_id, data_source_id, &requested_job)
                        .await
                }
            };
            assert_not_found(&case, "IngestionJob", &requested_job, &answer);
            // The enclosing resources are no part of the sentence.
            for (member, id) in [
                ("knowledgeBaseId", knowledge_base_id.as_str()),
                ("dataSourceId", data_source_id),
            ] {
                assert!(
                    !answer.message().contains(id),
                    "case {case:?}: the refusal must not name the enclosing {member} {id:?}, \
                     got {:?}",
                    answer.message()
                );
            }
        }

        // The job that IS stored is still addressable through its own data
        // source, so the refusals above are about resolution, not about having
        // broken the reader.
        let stored = emulator
            .get_ingestion_job(&knowledge_base_id, &first_source, &stored_job)
            .await;
        assert_eq!(
            stored.status, 200,
            "case {row:?}: the stored job must still be readable, got {stored:?}"
        );
    }
}

/// RULE: the refusal names the TYPE of the resource that could not be resolved,
/// as the model spells the shape, and that resource's own id — for a read, the
/// part of the address its own lookup failed on; for `StopIngestionJob`, whose
/// triple is one selector of the job it acts on, the addressed job whichever part
/// of the triple resolves to nothing (pinned over a table by the sibling
/// `stop_ingestion_job_not_found_scope` suite).
#[tokio::test]
async fn a_refusal_names_the_type_of_the_resource_it_could_not_resolve() {
    let emulator = Emulator::new();
    let knowledge_base_id = emulator.knowledge_base("wording-type-kb").await;
    let data_source_id = emulator
        .data_source(&knowledge_base_id, "wording-type-source")
        .await;
    let ingestion_job_id = emulator
        .start_ingestion_job(&knowledge_base_id, &data_source_id)
        .await;
    let absent = ABSENT_IDS[0];

    // The rows differ along which resource of the address is the one that cannot
    // be resolved, and so along the TYPE the refusal has to name: the knowledge
    // base, the data source within it, and the job within that pair — each read
    // through the operation whose path addresses it.
    assert_not_found(
        "a knowledge base nothing holds",
        "KnowledgeBase",
        absent,
        &emulator.get_knowledge_base(absent).await,
    );
    assert_not_found(
        "a data source the knowledge base does not hold",
        "DataSource",
        absent,
        &emulator.get_data_source(&knowledge_base_id, absent).await,
    );

    // `StopIngestionJob` addresses the job itself, so each row of the triple that
    // resolves to nothing — the knowledge base, the data source, the job id — is
    // reported as the addressed JOB not being found, with the id the path carried
    // for it.
    for (case, requested_knowledge_base, requested_data_source, requested_job) in [
        (
            "a knowledge base nothing holds",
            absent,
            data_source_id.as_str(),
            ingestion_job_id.as_str(),
        ),
        (
            "a data source the knowledge base does not hold",
            knowledge_base_id.as_str(),
            absent,
            ingestion_job_id.as_str(),
        ),
        (
            "a job the data source does not hold",
            knowledge_base_id.as_str(),
            data_source_id.as_str(),
            absent,
        ),
    ] {
        let answer = emulator
            .stop_ingestion_job(requested_knowledge_base, requested_data_source, requested_job)
            .await;
        assert_not_found(
            &format!("StopIngestionJob with {case}"),
            "IngestionJob",
            requested_job,
            &answer,
        );
    }
}

/// RULE applied to the agent plane: an agent operation addressed by an `agentId`
/// the store cannot resolve answers the same sentence, naming the `Agent` type and
/// the id the path carried.
#[tokio::test]
async fn an_agent_refusal_names_the_agent_it_could_not_resolve() {
    let emulator = crate::agent_lifecycle::Emulator::new();

    // The rows differ along the operation that addresses the agent (a read, a
    // delete, the work a prepare starts) and along the form of the id, which is
    // the one variable part of the sentence.
    for absent in ABSENT_IDS {
        for (operation, method, body) in [
            ("GetAgent", "GET", None),
            ("DeleteAgent", "DELETE", None),
            ("PrepareAgent", "POST", Some(json!({}))),
        ] {
            let case = format!("{operation} on the agent id {absent}");
            let answer = emulator
                .call(method, &format!("/agents/{absent}/"), body)
                .await;
            let (code, status) = NOT_FOUND;
            assert_eq!(
                answer.status, status,
                "case {case:?}: expected HTTP {status}, got {answer:?}"
            );
            assert_eq!(
                answer.error_type.as_deref(),
                Some(code),
                "case {case:?}: expected {code}, got {answer:?}"
            );
            let expected = not_found_sentence("Agent", absent);
            assert_eq!(
                answer.string(&["message"]),
                expected,
                "case {case:?}: expected the refusal worded {expected:?}, got {:?}",
                answer.body
            );
        }
    }
}
