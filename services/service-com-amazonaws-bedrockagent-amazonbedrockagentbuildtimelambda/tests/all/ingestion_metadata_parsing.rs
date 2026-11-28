// SPDX-License-Identifier: CC0-1.0

//! What an ingestion job does with a source document whose sibling METADATA
//! object cannot be parsed, end to end over HTTP.
//!
//! A source document's metadata travels beside it, in the object named
//! `<key>.metadata.json`, and that object has to be the JSON object a metadata
//! file is ("a metadata file must be a JSON object" in the Amazon Bedrock
//! knowledge-base documentation). The rule this suite pins is that a document
//! whose metadata is not is one the knowledge base does NOT take in, while the
//! JOB itself still finishes:
//!
//! * `IngestionJobStatistics` counts such a document under
//!   `numberOfDocumentsFailed` — "the number of source documents that failed to
//!   be ingested" — and not under `numberOfNewDocumentsIndexed`, while
//!   `numberOfDocumentsScanned` still counts it and
//!   `numberOfMetadataDocumentsScanned` still counts its metadata file
//!   (`smithy_query(query="shape",shape="IngestionJobStatistics")`);
//! * the job's `failureReasons` names the refused document's `s3://` URI and no
//!   document it indexed;
//! * the refused document becomes no document of the data source, so
//!   `ListKnowledgeBaseDocuments` does not report it and the knowledge base's
//!   bound vector store holds no vector for it;
//! * the job's terminal status is still `COMPLETE`: it is the DOCUMENT that
//!   failed, not the run;
//! * a document the data source ALREADY held when its metadata broke stays held
//!   — the source still offers it, so nothing deleted it — while the job that
//!   found the broken metadata still counts it failed.
//!
//! The rows differ along what the rule generalises over: how many of the
//! documents carry a metadata object at all, how many of those are the object a
//! metadata file is, and in which way one that is not is wrong (truncated, not
//! JSON at all, JSON that is not an object, empty).
//!
//! Every assertion here goes through the router, so it exercises the real wire
//! path (route matching, model constraint validation, serialization) rather than
//! calling a handler directly, and every wait is a poll for the CONDITION — the
//! job's terminal status — under a deadline generous enough for any host.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_bedrockagent_amazonbedrockagentbuildtimelambda::EmulatedService;
use service_com_amazonaws_s3_amazons3::state::{service_state as s3_state, Object};
use smithy_server_runtime::MultiServiceRouter;

use crate::vector_store_fixture::{index_vector_keys, seed_index, EMBEDDING_DIMENSION};

/// A SigV4 `Authorization` header scoped to the `bedrock` signing name: the
/// routing key this service claims on.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/eu-west-1/bedrock/aws4_request, \
     SignedHeaders=host;x-amz-date, Signature=0123456789abcdef";

/// The region the data source's bucket is held in.
const REGION: &str = "eu-west-1";

/// A role ARN matching the modelled `KnowledgeBaseRoleArn` pattern.
const KB_ROLE: &str =
    "arn:aws:iam::000000000000:role/service-role/AmazonBedrockExecutionRoleForKnowledgeBase";

/// An embedding model ARN matching the modelled `BedrockEmbeddingModelArn`.
const EMBEDDING_MODEL: &str =
    "arn:aws:bedrock:eu-west-1::foundation-model/amazon.titan-embed-text-v2:0";

/// The vector bucket the knowledge bases here bind to.
const VECTOR_BUCKET: &str = "arn:aws:s3vectors:::bucket/metadata-parse-vectors";

/// The index of [`VECTOR_BUCKET`] the ingested vectors are written into.
const INDEX: &str = "metadata-parse-index";

/// The S3 bucket every data source here is crawled from.
const SOURCE_BUCKET: &str = "metadata-parse-source-documents";

/// The prefix the data sources here include, so each case's objects are its own.
const PREFIX: &str = "notes/";

/// The suffix that makes an object the metadata file of the document beside it,
/// as the Amazon Bedrock knowledge-base documentation names it
/// (`<source file name>.metadata.json`).
const METADATA_SUFFIX: &str = ".metadata.json";

/// How long a poll waits for the clock to complete a started job: many times the
/// spans the service owes it, so a loaded machine still converges, and bounded
/// so a hung poll fails this test rather than hanging the suite.
const COMPLETION_POLL_CAP: Duration = Duration::from_secs(30);

/// How long one poll attempt waits before reading the job again.
const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// Every member of the modelled `IngestionJobStatistics`, so "the counters this
/// job reports" is a complete reading rather than the few an assertion happens
/// to name.
const STATISTICS_MEMBERS: [&str; 8] = [
    "numberOfDocumentsScanned",
    "numberOfMetadataDocumentsScanned",
    "numberOfNewDocumentsIndexed",
    "numberOfModifiedDocumentsIndexed",
    "numberOfMetadataDocumentsModified",
    "numberOfDocumentsDeleted",
    "numberOfDocumentsFailed",
    "numberOfDocumentsSkipped",
];

/// One HTTP answer from the emulator, parsed.
#[derive(Debug)]
struct Answer {
    status: u16,
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

    /// Every counter of the job's statistics, as a client reads them.
    fn statistics(&self) -> BTreeMap<&'static str, i64> {
        STATISTICS_MEMBERS
            .into_iter()
            .map(|member| {
                let value = self
                    .member(&["ingestionJob", "statistics", member])
                    .as_i64()
                    .unwrap_or_else(|| {
                        panic!("statistics.{member} is not a number in {:?}", self.body)
                    });
                (member, value)
            })
            .collect()
    }

    /// The reasons the job reports, or an empty list when it reports none.
    fn failure_reasons(&self) -> Vec<String> {
        self.member(&["ingestionJob", "failureReasons"])
            .as_array()
            .map(|reasons| {
                reasons
                    .iter()
                    .map(|reason| reason.as_str().unwrap_or_default().to_string())
                    .collect()
            })
            .unwrap_or_default()
    }
}

/// The counters a run that scanned `scanned` documents beside
/// `metadata_documents` metadata files, indexed `new` of them and refused
/// `failed` must report, with everything else at the modelled zero.
fn expected_statistics(
    scanned: i64,
    metadata_documents: i64,
    new: i64,
    failed: i64,
) -> BTreeMap<&'static str, i64> {
    let mut expected: BTreeMap<&'static str, i64> =
        STATISTICS_MEMBERS.into_iter().map(|member| (member, 0)).collect();
    expected.insert("numberOfDocumentsScanned", scanned);
    expected.insert("numberOfMetadataDocumentsScanned", metadata_documents);
    expected.insert("numberOfNewDocumentsIndexed", new);
    expected.insert("numberOfDocumentsFailed", failed);
    expected
}

/// The emulator under test: one router over one shared engine, exactly as the
/// production router mounts this service.
struct Emulator {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        // The vector store the knowledge base binds to, and the bucket its data
        // sources are crawled from: both set up before anything names them, as a
        // caller sets them up against AWS.
        seed_index(&engine, VECTOR_BUCKET, INDEX, EMBEDDING_DIMENSION);
        {
            let state = s3_state(&*engine).expect("the object store on this engine");
            state.lock().create_bucket(SOURCE_BUCKET, REGION);
        }
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        Self { router, engine }
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
        Answer { status, body }
    }

    /// Store `body` at `key` of the source bucket, through Amazon S3's own store
    /// — the one map its `PutObject` writes and its `GetObject` reads.
    fn hold_object(&self, key: &str, body: &str) {
        let state = s3_state(&*self.engine).expect("the object store on this engine");
        let mut store = state.lock();
        let written = store.put_object(
            SOURCE_BUCKET,
            key,
            Object::stored(body.as_bytes().to_vec(), Some("text/plain".to_string())),
        );
        assert!(
            written.is_some(),
            "the harness must be able to store the object at {key:?}"
        );
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

    /// Attach an S3 data source over `prefix` of the source bucket and answer
    /// its minted id.
    async fn data_source(&self, knowledge_base_id: &str, name: &str, prefix: &str) -> String {
        let created = self
            .call(
                "PUT",
                &format!("/knowledgebases/{knowledge_base_id}/datasources/"),
                Some(json!({
                    "name": name,
                    "dataSourceConfiguration": {
                        "type": "S3",
                        "s3Configuration": {
                            "bucketArn": format!("arn:aws:s3:::{SOURCE_BUCKET}"),
                            "inclusionPrefixes": [prefix],
                        },
                    },
                    "dataDeletionPolicy": "RETAIN",
                })),
            )
            .await;
        assert_eq!(
            created.status, 200,
            "case setup: expected CreateDataSource -> 200, got {created:?}"
        );
        created.string(&["dataSource", "dataSourceId"])
    }

    /// Start an ingestion job and POLL `GetIngestionJob` until the clock has
    /// completed it, answering the completed job.
    async fn run_ingestion_job(&self, knowledge_base_id: &str, data_source_id: &str) -> Answer {
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
        let ingestion_job_id = started.string(&["ingestionJob", "ingestionJobId"]);
        let deadline = Instant::now() + COMPLETION_POLL_CAP;
        loop {
            let read = self
                .call(
                    "GET",
                    &format!(
                        "/knowledgebases/{knowledge_base_id}/datasources/{data_source_id}\
                         /ingestionjobs/{ingestion_job_id}"
                    ),
                    None,
                )
                .await;
            assert_eq!(
                read.status, 200,
                "case setup: expected GetIngestionJob -> 200, got {read:?}"
            );
            if read.string(&["ingestionJob", "status"]) == "COMPLETE" {
                return read;
            }
            assert!(
                Instant::now() < deadline,
                "case setup: the clock must complete job {ingestion_job_id}, last read {read:?}"
            );
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    }

    /// The source URIs `ListKnowledgeBaseDocuments` reports for the data source,
    /// in URI order.
    async fn document_uris(&self, knowledge_base_id: &str, data_source_id: &str) -> Vec<String> {
        let listed = self
            .call(
                "POST",
                &format!(
                    "/knowledgebases/{knowledge_base_id}/datasources/{data_source_id}/documents"
                ),
                Some(json!({})),
            )
            .await;
        assert_eq!(
            listed.status, 200,
            "case setup: expected ListKnowledgeBaseDocuments -> 200, got {listed:?}"
        );
        let mut uris: Vec<String> = listed
            .member(&["documentDetails"])
            .as_array()
            .unwrap_or_else(|| panic!("documentDetails is a list in {:?}", listed.body))
            .iter()
            .map(|detail| {
                detail["identifier"]["s3"]["uri"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string()
            })
            .collect();
        uris.sort();
        uris
    }

    /// The keys of the vectors the bound index holds.
    fn index_keys(&self) -> Vec<String> {
        index_vector_keys(&self.engine, VECTOR_BUCKET, INDEX)
    }
}

/// The `s3://` URI of one source document, as a knowledge-base document and the
/// vector written for it name it.
fn uri(key: &str) -> String {
    format!("s3://{SOURCE_BUCKET}/{key}")
}

/// A well-formed metadata file.
const VALID_METADATA: &str = r#"{"metadataAttributes": {"tier": "gold"}}"#;

/// One row of the metadata table: the documents of the case's prefix, each with
/// the body of the metadata object beside it (`None` for a document with no
/// metadata file at all).
struct MetadataRow {
    case: &'static str,
    /// `(document name, metadata body)` per source document.
    documents: Vec<(&'static str, Option<&'static str>)>,
    /// The documents the job must refuse, by name.
    refused: Vec<&'static str>,
}

/// A document whose sibling metadata object is not parseable JSON is counted
/// FAILED rather than newly indexed, named in the job's `failureReasons`, and
/// left out of the data source's documents and of the bound index — while the
/// job itself completes.
#[tokio::test]
async fn a_document_with_unparseable_metadata_is_failed_and_not_indexed() {
    let rows = [
        MetadataRow {
            case: "no metadata file at all",
            documents: vec![("alpha.txt", None), ("beta.txt", None)],
            refused: Vec::new(),
        },
        MetadataRow {
            case: "a well-formed metadata file",
            documents: vec![("alpha.txt", Some(VALID_METADATA)), ("beta.txt", None)],
            refused: Vec::new(),
        },
        MetadataRow {
            case: "one truncated metadata file beside two well-formed documents",
            documents: vec![
                ("alpha.txt", None),
                ("beta.txt", None),
                ("gamma.txt", Some(r#"{"metadataAttributes": {"#)),
            ],
            refused: vec!["gamma.txt"],
        },
        MetadataRow {
            case: "a metadata file that is not JSON at all",
            documents: vec![
                ("alpha.txt", Some(VALID_METADATA)),
                ("beta.txt", Some("tier = gold")),
            ],
            refused: vec!["beta.txt"],
        },
        MetadataRow {
            case: "a metadata file that is JSON but not an object",
            documents: vec![
                ("alpha.txt", Some(r#"[{"tier": "gold"}]"#)),
                ("beta.txt", None),
            ],
            refused: vec!["alpha.txt"],
        },
        MetadataRow {
            case: "an empty metadata file",
            documents: vec![("alpha.txt", None), ("beta.txt", Some(""))],
            refused: vec!["beta.txt"],
        },
        MetadataRow {
            case: "every document's metadata broken",
            documents: vec![
                ("alpha.txt", Some("[")),
                ("beta.txt", Some("}{")),
            ],
            refused: vec!["alpha.txt", "beta.txt"],
        },
    ];

    for (position, row) in rows.iter().enumerate() {
        let emulator = Emulator::new();
        let prefix = format!("{PREFIX}{position}/");
        for (name, metadata) in &row.documents {
            emulator.hold_object(
                &format!("{prefix}{name}"),
                &format!("the text of {name} in case {position}"),
            );
            if let Some(metadata) = metadata {
                emulator.hold_object(&format!("{prefix}{name}{METADATA_SUFFIX}"), metadata);
            }
        }
        let knowledge_base_id = emulator
            .knowledge_base(&format!("metadata-parse-kb-{position}"))
            .await;
        let data_source_id = emulator
            .data_source(
                &knowledge_base_id,
                &format!("metadata-parse-ds-{position}"),
                &prefix,
            )
            .await;

        let job = emulator
            .run_ingestion_job(&knowledge_base_id, &data_source_id)
            .await;

        let scanned = i64::try_from(row.documents.len()).expect("a small count");
        let metadata_files = i64::try_from(
            row.documents
                .iter()
                .filter(|(_, metadata)| metadata.is_some())
                .count(),
        )
        .expect("a small count");
        let failed = i64::try_from(row.refused.len()).expect("a small count");
        let expected = expected_statistics(scanned, metadata_files, scanned - failed, failed);
        let measured = job.statistics();
        assert_eq!(
            measured, expected,
            "case {}: expected the job to report {expected:?}, got {measured:?}",
            row.case
        );

        // The reasons name the refused documents and nothing else: one reason
        // per failed document, each naming that document's own URI and no URI
        // of a document the job indexed.
        let reasons = job.failure_reasons();
        assert_eq!(
            reasons.len(),
            row.refused.len(),
            "case {}: expected one failure reason per refused document, got {reasons:?}",
            row.case
        );
        for name in &row.refused {
            let named = uri(&format!("{prefix}{name}"));
            assert!(
                reasons.iter().any(|reason| reason.contains(&named)),
                "case {}: expected a failure reason naming {named}, got {reasons:?}",
                row.case
            );
        }
        for (name, _) in row.documents.iter().filter(|(name, _)| {
            !row.refused.contains(name)
        }) {
            let indexed = uri(&format!("{prefix}{name}"));
            assert!(
                !reasons.iter().any(|reason| reason.contains(&indexed)),
                "case {}: expected no failure reason naming the indexed {indexed}, \
                 got {reasons:?}",
                row.case
            );
        }

        // The refused documents are documents of nothing: neither the data
        // source's listing nor the bound vector store holds them.
        let mut expected_uris: Vec<String> = row
            .documents
            .iter()
            .filter(|(name, _)| !row.refused.contains(name))
            .map(|(name, _)| uri(&format!("{prefix}{name}")))
            .collect();
        expected_uris.sort();
        assert_eq!(
            emulator
                .document_uris(&knowledge_base_id, &data_source_id)
                .await,
            expected_uris,
            "case {}: expected only the documents the job indexed to be the data source's own",
            row.case
        );
        assert_eq!(
            emulator.index_keys(),
            expected_uris,
            "case {}: expected the bound index to hold a vector for exactly the indexed \
             documents",
            row.case
        );
    }
}

/// Repairing the metadata file is all it takes: the next job indexes exactly the
/// document that had failed, as NEW, fails none, and the data source then holds
/// every document of its source.
#[tokio::test]
async fn repairing_the_metadata_file_lets_the_next_job_index_the_document() {
    let emulator = Emulator::new();
    let prefix = format!("{PREFIX}repair/");
    for name in ["alpha.txt", "beta.txt", "gamma.txt"] {
        emulator.hold_object(&format!("{prefix}{name}"), &format!("the text of {name}"));
    }
    // The third document's metadata is truncated, so the first job refuses it.
    emulator.hold_object(
        &format!("{prefix}gamma.txt{METADATA_SUFFIX}"),
        r#"{"metadataAttributes": {"#,
    );
    let knowledge_base_id = emulator.knowledge_base("metadata-repair-kb").await;
    let data_source_id = emulator
        .data_source(&knowledge_base_id, "metadata-repair-ds", &prefix)
        .await;

    let first = emulator
        .run_ingestion_job(&knowledge_base_id, &data_source_id)
        .await;
    let expected_first = expected_statistics(3, 1, 2, 1);
    assert_eq!(
        first.statistics(),
        expected_first,
        "case the first run: expected {expected_first:?}, got {:?}",
        first.statistics()
    );

    // The metadata file is replaced with valid JSON — nothing else changes.
    emulator.hold_object(
        &format!("{prefix}gamma.txt{METADATA_SUFFIX}"),
        VALID_METADATA,
    );

    let second = emulator
        .run_ingestion_job(&knowledge_base_id, &data_source_id)
        .await;
    let expected_second = expected_statistics(3, 1, 1, 0);
    assert_eq!(
        second.statistics(),
        expected_second,
        "case the repaired run: expected exactly the repaired document indexed as new \
         ({expected_second:?}), got {:?}",
        second.statistics()
    );
    assert!(
        second.failure_reasons().is_empty(),
        "case the repaired run: expected no failure reason, got {:?}",
        second.failure_reasons()
    );

    let mut expected_uris: Vec<String> = ["alpha.txt", "beta.txt", "gamma.txt"]
        .into_iter()
        .map(|name| uri(&format!("{prefix}{name}")))
        .collect();
    expected_uris.sort();
    assert_eq!(
        emulator
            .document_uris(&knowledge_base_id, &data_source_id)
            .await,
        expected_uris,
        "case the repaired run: expected the data source to hold all three documents"
    );
    assert_eq!(
        emulator.index_keys(),
        expected_uris,
        "case the repaired run: expected the bound index to hold a vector for all three \
         documents"
    );
}

/// A document the data source ALREADY holds, whose metadata file breaks after it
/// was indexed, is counted failed by the job that found the broken metadata —
/// and stays held, because the source still offers it and nothing deleted it.
#[tokio::test]
async fn a_held_document_whose_metadata_breaks_later_stays_held_and_is_counted_failed() {
    let emulator = Emulator::new();
    let prefix = format!("{PREFIX}breaks-later/");
    for name in ["alpha.txt", "beta.txt"] {
        emulator.hold_object(&format!("{prefix}{name}"), &format!("the text of {name}"));
    }
    emulator.hold_object(&format!("{prefix}beta.txt{METADATA_SUFFIX}"), VALID_METADATA);
    let knowledge_base_id = emulator.knowledge_base("metadata-breaks-later-kb").await;
    let data_source_id = emulator
        .data_source(&knowledge_base_id, "metadata-breaks-later-ds", &prefix)
        .await;

    let first = emulator
        .run_ingestion_job(&knowledge_base_id, &data_source_id)
        .await;
    let expected_first = expected_statistics(2, 1, 2, 0);
    assert_eq!(
        first.statistics(),
        expected_first,
        "case the first run: expected both documents indexed ({expected_first:?}), got {:?}",
        first.statistics()
    );

    // The metadata file of a document the data source now HOLDS is replaced with
    // a body that is not the JSON object a metadata file is.
    emulator.hold_object(
        &format!("{prefix}beta.txt{METADATA_SUFFIX}"),
        r#"{"metadataAttributes": {"#,
    );

    let second = emulator
        .run_ingestion_job(&knowledge_base_id, &data_source_id)
        .await;
    let expected_second = expected_statistics(2, 1, 0, 1);
    assert_eq!(
        second.statistics(),
        expected_second,
        "case the broken-later run: expected the held document counted failed and nothing \
         indexed ({expected_second:?}), got {:?}",
        second.statistics()
    );
    let named = uri(&format!("{prefix}beta.txt"));
    assert_eq!(
        second.failure_reasons().len(),
        1,
        "case the broken-later run: expected one failure reason, got {:?}",
        second.failure_reasons()
    );
    assert!(
        second
            .failure_reasons()
            .iter()
            .any(|reason| reason.contains(&named)),
        "case the broken-later run: expected a failure reason naming {named}, got {:?}",
        second.failure_reasons()
    );

    // Both documents are still the data source's own, and both still have a
    // vector: this run took one of them no further, which is not a removal of
    // what the first run did.
    let mut expected_uris: Vec<String> = ["alpha.txt", "beta.txt"]
        .into_iter()
        .map(|name| uri(&format!("{prefix}{name}")))
        .collect();
    expected_uris.sort();
    assert_eq!(
        emulator
            .document_uris(&knowledge_base_id, &data_source_id)
            .await,
        expected_uris,
        "case the broken-later run: expected the data source to still hold both documents"
    );
    assert_eq!(
        emulator.index_keys(),
        expected_uris,
        "case the broken-later run: expected the bound index to still hold both vectors"
    );
}
