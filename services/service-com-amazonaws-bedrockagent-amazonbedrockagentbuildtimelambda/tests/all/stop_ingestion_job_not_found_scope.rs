// SPDX-License-Identifier: CC0-1.0

//! Which resource `StopIngestionJob` reports its `ResourceNotFoundException`
//! against, end to end over HTTP.
//!
//! RULE: the three ids `StopIngestionJob` is addressed by
//! (`smithy_query(query="operation",operation="StopIngestionJob")`:
//! `/knowledgebases/{knowledgeBaseId}/datasources/{dataSourceId}/ingestionjobs/{ingestionJobId}/stop`)
//! are ONE selector of the job the call acts on — a job is held under the
//! knowledge base and data source it ran for — so a request that selects no job
//! is refused as the ADDRESSED JOB not being found, whichever part of the triple
//! is the part that resolves to nothing. The sentence is the one the wording suite
//! pins, "IngestionJob with id <ingestionJobId> is not found.", whose only
//! variable part is the `ingestionJobId` the request carried; the enclosing
//! knowledge base and data source are no part of it.
//!
//! The reads of this plane resolve their scope first and name it instead (the
//! `nested_read_not_found_scope` suite), so this is the rule for the operation
//! that acts ON the job rather than for every operation of the triple.
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
const VECTOR_BUCKET: &str = "arn:aws:s3vectors:::bucket/stop-scope-vectors";
const INDEX: &str = "stop-scope-index";

/// The missing-resource error of this service and the status its model binds to
/// it (`smithy_query(query="operation",operation="StopIngestionJob")`).
const NOT_FOUND: (&str, u16) = ("ResourceNotFoundException", 404);

/// Identifiers of the modelled `Id` shape
/// (`smithy_query(query="shape",shape="Id")`: `^[0-9a-zA-Z]{10}$`, which all three
/// path members target) that no record of this suite carries: digits only, the
/// mixed form, and a word-shaped one.
const ABSENT_IDS: [&str; 3] = ["0123456789", "aZ09aZ09aZ", "NoSuchIdAA"];

/// The sentence this service words for a resource it cannot resolve.
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
    fn string(&self, path: &[&str]) -> String {
        let mut at = &self.body;
        for key in path {
            at = &at[*key];
        }
        at.as_str()
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
}

/// One row: the triple a `StopIngestionJob` request carries, and the job id its
/// refusal has to name.
struct Row {
    case: String,
    knowledge_base_id: String,
    data_source_id: String,
    ingestion_job_id: String,
}

/// RULE: a `StopIngestionJob` whose triple selects no job is refused as the
/// addressed JOB not being found, whichever part of the triple resolves to
/// nothing.
#[tokio::test]
async fn a_stop_names_the_addressed_job_whatever_part_of_its_triple_resolves_to_none() {
    let emulator = Emulator::new();
    let knowledge_base_id = emulator.knowledge_base("stop-scope-kb").await;
    let other_knowledge_base_id = emulator.knowledge_base("stop-scope-other-kb").await;
    let data_source_id = emulator
        .data_source(&knowledge_base_id, "stop-scope-source")
        .await;
    let other_data_source_id = emulator
        .data_source(&other_knowledge_base_id, "stop-scope-other-source")
        .await;
    let other_job = emulator
        .start_ingestion_job(&other_knowledge_base_id, &other_data_source_id)
        .await;

    // The rows differ along WHICH part of the triple resolves to nothing — the
    // knowledge base, the data source, the job, and every pair of them at once —
    // and along the form of the absent id, which is the sentence's one variable
    // part.
    let mut rows: Vec<Row> = Vec::new();
    for absent in ABSENT_IDS {
        rows.push(Row {
            case: format!("a knowledge base nothing holds ({absent}) over a stored pair"),
            knowledge_base_id: absent.to_string(),
            data_source_id: data_source_id.clone(),
            ingestion_job_id: ABSENT_IDS[0].to_string(),
        });
        rows.push(Row {
            case: format!("a data source nothing holds ({absent}) under a stored knowledge base"),
            knowledge_base_id: knowledge_base_id.clone(),
            data_source_id: absent.to_string(),
            ingestion_job_id: ABSENT_IDS[1].to_string(),
        });
        rows.push(Row {
            case: format!("a job nothing holds ({absent}) under a stored pair"),
            knowledge_base_id: knowledge_base_id.clone(),
            data_source_id: data_source_id.clone(),
            ingestion_job_id: absent.to_string(),
        });
        rows.push(Row {
            case: format!("neither the knowledge base nor the data source held ({absent})"),
            knowledge_base_id: absent.to_string(),
            data_source_id: absent.to_string(),
            ingestion_job_id: ABSENT_IDS[2].to_string(),
        });
        rows.push(Row {
            case: format!("no part of the triple held ({absent})"),
            knowledge_base_id: absent.to_string(),
            data_source_id: absent.to_string(),
            ingestion_job_id: absent.to_string(),
        });
    }
    // A job that DOES exist, addressed through a knowledge base nothing holds and
    // through another knowledge base's data source: the triple selects no job, so
    // the refusal is still about the job the request named.
    rows.push(Row {
        case: "a stored job under a knowledge base nothing holds".to_string(),
        knowledge_base_id: ABSENT_IDS[0].to_string(),
        data_source_id: other_data_source_id.clone(),
        ingestion_job_id: other_job.clone(),
    });
    rows.push(Row {
        case: "a stored job addressed through another knowledge base's pair".to_string(),
        knowledge_base_id: knowledge_base_id.clone(),
        data_source_id: other_data_source_id.clone(),
        ingestion_job_id: other_job.clone(),
    });

    let (code, status) = NOT_FOUND;
    for row in &rows {
        let case = &row.case;
        let answer = emulator
            .stop_ingestion_job(
                &row.knowledge_base_id,
                &row.data_source_id,
                &row.ingestion_job_id,
            )
            .await;
        assert_eq!(
            answer.status, status,
            "case {case:?}: expected HTTP {status}, got {answer:?}"
        );
        assert_eq!(
            answer.error_type.as_deref(),
            Some(code),
            "case {case:?}: expected {code}, got {answer:?}"
        );
        let expected = not_found_sentence("IngestionJob", &row.ingestion_job_id);
        assert_eq!(
            answer.message(),
            expected,
            "case {case:?}: expected the refusal worded {expected:?}, got {:?}",
            answer.message()
        );
        // The enclosing resources are no part of the sentence, so neither of
        // their ids appears in it.
        for (member, id) in [
            ("knowledgeBaseId", &row.knowledge_base_id),
            ("dataSourceId", &row.data_source_id),
        ] {
            if id == &row.ingestion_job_id {
                continue;
            }
            assert!(
                !answer.message().contains(id.as_str()),
                "case {case:?}: the refusal must not name the enclosing {member} {id:?}, \
                 got {:?}",
                answer.message()
            );
        }
    }

    // The job the OTHER pair holds is still stoppable through its own triple, so
    // the refusals above are about resolution and not about having broken the
    // operation: the stop is accepted and the job reads back as stopping.
    let stopped = emulator
        .stop_ingestion_job(&other_knowledge_base_id, &other_data_source_id, &other_job)
        .await;
    assert_eq!(
        stopped.status, 202,
        "the stored job must still be stoppable through its own triple, got {stopped:?}"
    );
    assert_eq!(
        stopped.string(&["ingestionJob", "ingestionJobId"]),
        other_job,
        "the stop must answer with the job it stopped, got {:?}",
        stopped.body
    );
    let read_back = emulator
        .get_ingestion_job(&other_knowledge_base_id, &other_data_source_id, &other_job)
        .await;
    assert_eq!(
        read_back.status, 200,
        "the stopped job must still be readable, got {read_back:?}"
    );
    assert_eq!(
        read_back.string(&["ingestionJob", "status"]),
        stopped.string(&["ingestionJob", "status"]),
        "the stop must be what the reader reports, got {:?}",
        read_back.body
    );
}
