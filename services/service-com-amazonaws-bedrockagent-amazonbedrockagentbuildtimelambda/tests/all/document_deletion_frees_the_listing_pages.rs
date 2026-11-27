// SPDX-License-Identifier: CC0-1.0

//! How `ListKnowledgeBaseDocuments` PAGES a data source some of whose documents have been
//! deleted, end to end over HTTP.
//!
//! One rule is pinned here: the collection reports a document being deleted under the
//! statuses of that deletion and nothing else, and once the removal has settled it stops
//! enumerating the identity for good — so a client polling the collection until the
//! identifier leaves it converges, and from then on the removed document occupies no row
//! and no page of the listing: walking the pages yields exactly the documents the data
//! source still holds, once each and in the listing's own order. The deletion's outcome
//! stays reportable through a targeted `GetKnowledgeBaseDocuments` of the identifier.
//!
//! Every assertion goes through the router, so it exercises the real wire path (route
//! matching, model constraint validation, serialization) rather than calling a handler
//! directly.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_bedrockagent_amazonbedrockagentbuildtimelambda::state::MAX_DOCUMENTS_PER_PAGE;
use service_com_amazonaws_bedrockagent_amazonbedrockagentbuildtimelambda::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 `Authorization` header scoped to the `bedrock` signing name: the routing key
/// this service claims on.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/eu-west-1/bedrock/aws4_request, \
     SignedHeaders=host;x-amz-date, Signature=0123456789abcdef";

/// A role ARN matching the modelled `KnowledgeBaseRoleArn` pattern.
const KB_ROLE: &str =
    "arn:aws:iam::000000000000:role/service-role/AmazonBedrockExecutionRoleForKnowledgeBase";

/// An embedding model ARN matching the modelled `BedrockEmbeddingModelArn`.
const EMBEDDING_MODEL: &str =
    "arn:aws:bedrock:eu-west-1::foundation-model/amazon.titan-embed-text-v2:0";

/// The vector bucket and index the knowledge base here binds to.
const VECTOR_BUCKET: &str = "arn:aws:s3vectors:::bucket/deletion-paging-vectors";
const VECTOR_INDEX: &str = "deletion-paging-index";

/// The statuses a document of this service reports while a deletion carries it out of the
/// data source, and the terminal status of that deletion.
///
/// Source: the `status` member of the modelled `KnowledgeBaseDocumentDetail`
/// (`smithy_query(query="doc",shape="KnowledgeBaseDocumentDetail",member="status")`), which
/// documents `DELETING` as "You submitted the delete job containing the document",
/// `DELETE_IN_PROGRESS` as "The document is being deleted" and `NOT_FOUND` as "The document
/// wasn't found".
const DELETION_STATUSES: [&str; 3] = ["DELETING", "DELETE_IN_PROGRESS", "NOT_FOUND"];
const NOT_FOUND: &str = "NOT_FOUND";

/// The terminal status of an ingestion, which the same member documents as the document
/// that was successfully indexed.
const INDEXED: &str = "INDEXED";

/// How long a poll keeps looking for a transition the CLOCK owes before giving up.
///
/// Deliberately far past every span the store owes a document, because what is asserted is
/// that the removal settles at all, never how fast — so a loaded host converges here
/// instead of failing.
const POLL_CAP: Duration = Duration::from_secs(120);

/// How often a poll looks again while it waits for the condition.
const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// The modelled maximum number of documents one request may carry, which both
/// `IngestKnowledgeBaseDocuments` (`documents` @length 1..10) and
/// `DeleteKnowledgeBaseDocuments` (`documentIdentifiers` @length 1..10) declare.
const MAX_DOCUMENTS_PER_REQUEST: usize = 10;

/// One HTTP answer from the emulator, parsed.
#[derive(Debug)]
struct Answer {
    status: u16,
    body: Value,
}

impl Answer {
    /// The response's `documentDetails`, which every document operation of this service
    /// answers with.
    fn details(&self) -> &Vec<Value> {
        self.body["documentDetails"]
            .as_array()
            .unwrap_or_else(|| panic!("documentDetails is not a list in {:?}", self.body))
    }

    /// The identifiers the answer reports details for, in the order reported.
    fn identifiers(&self) -> Vec<Value> {
        self.details()
            .iter()
            .map(|detail| detail["identifier"].clone())
            .collect()
    }

    /// The `nextToken` the page handed out, when it handed one out.
    fn next_token(&self) -> Option<String> {
        self.body["nextToken"].as_str().map(ToString::to_string)
    }
}

/// Whether `details` reports `identifier` with the status `status`.
fn reports(details: &[Value], identifier: &Value, status: &str) -> bool {
    details.iter().any(|detail| {
        &detail["identifier"] == identifier && detail["status"].as_str() == Some(status)
    })
}

/// The status `details` reports for `identifier`, or `None` when it reports no detail for
/// it at all.
fn status_of(details: &[Value], identifier: &Value) -> Option<String> {
    details
        .iter()
        .find(|detail| &detail["identifier"] == identifier)
        .and_then(|detail| detail["status"].as_str())
        .map(ToString::to_string)
}

/// The emulator under test: one router over one shared engine, exactly as the production
/// router mounts this service.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        // A self-managed vector knowledge base binds to an S3 Vectors index, which
        // `CreateKnowledgeBase` resolves in the service that owns it — so the index exists
        // first, as a caller creates it first against AWS.
        crate::vector_store_fixture::seed_index(
            &engine,
            VECTOR_BUCKET,
            VECTOR_INDEX,
            crate::vector_store_fixture::EMBEDDING_DIMENSION,
        );
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(engine));
        Self { router }
    }

    async fn call(&self, method: &str, uri: &str, body: Value) -> Answer {
        let bytes = Bytes::from(serde_json::to_vec(&body).expect("serialize request body"));
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

    /// Create a knowledge base and answer its minted `knowledgeBaseId`.
    async fn knowledge_base(&self, name: &str) -> String {
        let created = self
            .call(
                "PUT",
                "/knowledgebases/",
                json!({
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
                            "indexName": VECTOR_INDEX,
                        },
                    },
                }),
            )
            .await;
        assert_eq!(
            created.status, 202,
            "case setup: expected CreateKnowledgeBase -> 202, got {created:?}"
        );
        created.body["knowledgeBase"]["knowledgeBaseId"]
            .as_str()
            .expect("a minted knowledgeBaseId")
            .to_string()
    }

    /// Attach a CUSTOM data source — the kind documents are pushed into directly — and
    /// answer its minted `dataSourceId`.
    async fn data_source(&self, knowledge_base_id: &str, name: &str) -> String {
        let created = self
            .call(
                "PUT",
                &format!("/knowledgebases/{knowledge_base_id}/datasources/"),
                json!({
                    "name": name,
                    "dataSourceConfiguration": { "type": "CUSTOM" },
                }),
            )
            .await;
        assert_eq!(
            created.status, 200,
            "case setup: expected CreateDataSource -> 200, got {created:?}"
        );
        created.body["dataSource"]["dataSourceId"]
            .as_str()
            .expect("a minted dataSourceId")
            .to_string()
    }

    /// `IngestKnowledgeBaseDocuments`, in batches of the modelled maximum per request, so a
    /// data source can be filled with more documents than one request may carry.
    async fn ingest_all(&self, knowledge_base_id: &str, data_source_id: &str, ids: &[String]) {
        for batch in ids.chunks(MAX_DOCUMENTS_PER_REQUEST) {
            let documents: Vec<Value> = batch.iter().map(|id| custom_document(id)).collect();
            let ingested = self
                .call(
                    "PUT",
                    &format!(
                        "/knowledgebases/{knowledge_base_id}/datasources/\
                         {data_source_id}/documents"
                    ),
                    json!({ "documents": documents }),
                )
                .await;
            assert_eq!(
                ingested.status, 202,
                "case setup: expected IngestKnowledgeBaseDocuments -> 202, got {ingested:?}"
            );
        }
    }

    /// `DeleteKnowledgeBaseDocuments`, in batches of the modelled maximum per request.
    async fn delete_all(
        &self,
        knowledge_base_id: &str,
        data_source_id: &str,
        identifiers: &[Value],
    ) {
        for batch in identifiers.chunks(MAX_DOCUMENTS_PER_REQUEST) {
            let accepted = self
                .call(
                    "POST",
                    &format!(
                        "/knowledgebases/{knowledge_base_id}/datasources/\
                         {data_source_id}/documents/deleteDocuments"
                    ),
                    json!({ "documentIdentifiers": batch }),
                )
                .await;
            assert_eq!(
                accepted.status, 202,
                "case setup: expected DeleteKnowledgeBaseDocuments -> 202, got {accepted:?}"
            );
        }
    }

    /// The details `GetKnowledgeBaseDocuments` reports for these identifiers, read in
    /// batches of the modelled maximum per request.
    async fn get_documents(
        &self,
        knowledge_base_id: &str,
        data_source_id: &str,
        identifiers: &[Value],
    ) -> Vec<Value> {
        let mut details: Vec<Value> = Vec::new();
        for batch in identifiers.chunks(MAX_DOCUMENTS_PER_REQUEST) {
            let fetched = self
                .call(
                    "POST",
                    &format!(
                        "/knowledgebases/{knowledge_base_id}/datasources/{data_source_id}\
                         /documents/getDocuments"
                    ),
                    json!({ "documentIdentifiers": batch }),
                )
                .await;
            assert_eq!(
                fetched.status, 200,
                "expected GetKnowledgeBaseDocuments -> 200, got {fetched:?}"
            );
            details.extend(fetched.details().clone());
        }
        details
    }

    /// `ListKnowledgeBaseDocuments`, with or without the paging members.
    async fn list(
        &self,
        knowledge_base_id: &str,
        data_source_id: &str,
        max_results: Option<i64>,
        next_token: Option<&str>,
    ) -> Answer {
        let mut body = json!({});
        if let Some(max_results) = max_results {
            body["maxResults"] = json!(max_results);
        }
        if let Some(next_token) = next_token {
            body["nextToken"] = json!(next_token);
        }
        self.call(
            "POST",
            &format!("/knowledgebases/{knowledge_base_id}/datasources/{data_source_id}/documents"),
            body,
        )
        .await
    }
}

/// A custom document whose content is defined inline as text.
///
/// Source: the `DocumentContent` structure of this service's Smithy model
/// (`smithy_query(query="shape",shape="DocumentContent")`): the `dataSourceType` names
/// which sibling member carries the document's own content.
fn custom_document(id: &str) -> Value {
    json!({
        "content": {
            "dataSourceType": "CUSTOM",
            "custom": {
                "customDocumentIdentifier": { "id": id },
                "sourceType": "IN_LINE",
                "inlineContent": {
                    "type": "TEXT",
                    "textContent": { "data": format!("the text of {id}") },
                },
            },
        },
    })
}

/// The identifier the store holds a custom document under.
fn custom_identifier(id: &str) -> Value {
    json!({ "dataSourceType": "CUSTOM", "custom": { "id": id } })
}

/// The identifiers of these ids in the order the listing reports them: a custom document's
/// own id breaks the tie within its kind, so a page resumes exactly after the one before
/// it.
fn in_document_order(ids: &[String]) -> Vec<Value> {
    let mut ordered = ids.to_vec();
    ordered.sort();
    ordered.iter().map(|id| custom_identifier(id)).collect()
}

/// One row of the table: a data source of its own, the documents it took, the ones a
/// delete named and the `maxResults` its pages are cut at.
struct Row {
    case: &'static str,
    data_source_id: String,
    doomed: Vec<Value>,
    surviving: Vec<Value>,
    max_results: Option<i64>,
    /// Every status the COLLECTION reported for a deleted document while the removal was
    /// settling.
    statuses_seen: BTreeSet<String>,
}

/// Walk every page of the listing with `max_results` and answer the identifiers reported,
/// in the order the pages reported them.
async fn walk(
    emulator: &Emulator,
    knowledge_base_id: &str,
    row: &Row,
    expected_pages: usize,
    page_cap: usize,
) -> Vec<Value> {
    let case = row.case;
    let mut walked: Vec<Value> = Vec::new();
    let mut token: Option<String> = None;
    let mut pages = 0;
    loop {
        let page = emulator
            .list(
                knowledge_base_id,
                &row.data_source_id,
                row.max_results,
                token.as_deref(),
            )
            .await;
        assert_eq!(
            page.status, 200,
            "case {case}: expected ListKnowledgeBaseDocuments -> 200, got {page:?}"
        );
        let reported = page.identifiers();
        assert!(
            reported.len() <= page_cap,
            "case {case}: a page carries at most {page_cap} documents, got {reported:?}"
        );
        walked.extend(reported.clone());
        pages += 1;
        assert!(
            pages <= expected_pages + 1,
            "case {case}: paging must terminate after the documents the data source holds, \
             walked {walked:?}"
        );
        match page.next_token() {
            None => break,
            Some(next) => {
                assert_eq!(
                    reported.len(),
                    page_cap,
                    "case {case}: a page that carries a nextToken is a full page, \
                     got {reported:?}"
                );
                token = Some(next);
            }
        }
    }
    assert_eq!(
        pages, expected_pages,
        "case {case}: expected {expected_pages} page(s) of {page_cap}, walked {pages}"
    );
    walked
}

/// RULE: the collection reports a document being deleted under the statuses of that
/// deletion and stops enumerating the identity once the removal has settled — so a client
/// polling the collection until the identifier leaves it converges — and from then on the
/// removed document occupies no row and no page of the listing: the pages carry exactly the
/// documents the data source still holds, once each and in document order, while the
/// deletion's outcome stays reportable through the targeted read.
///
/// The rows differ along what the rule generalises over: how many documents the data source
/// took, how many of them a delete removed (none, one, a whole page's worth, all but one,
/// every one of them) and the `maxResults` the pages are cut at — including the lower bound
/// of the modelled range (1..1000), a value above the document count and the member
/// omitted. The documents a delete does not name are the control in every row.
#[tokio::test]
async fn deleted_documents_stop_occupying_the_listing_pages() {
    let emulator = Emulator::new();
    let knowledge_base_id = emulator.knowledge_base("deletion-paging-kb").await;

    // (case, how many documents are ingested, how many of them are deleted, maxResults)
    let table: Vec<(&'static str, usize, usize, Option<i64>)> = vec![
        ("nothing deleted, maxResults omitted", 4, 0, None),
        ("one of twelve deleted, pages of five", 12, 1, Some(5)),
        ("five of twelve deleted, pages of five", 12, 5, Some(5)),
        ("eleven of twelve deleted, pages of one", 12, 11, Some(1)),
        ("every document deleted, maxResults omitted", 3, 3, None),
        (
            "three of five deleted, maxResults above the count",
            5,
            3,
            Some(50),
        ),
    ];

    // Every row is set up first, so the removals of all of them settle over one span rather
    // than one span each.
    let mut rows: Vec<Row> = Vec::new();
    for (index, (case, held, deleted, max_results)) in table.into_iter().enumerate() {
        let data_source_id = emulator
            .data_source(&knowledge_base_id, &format!("deletion-paging-{index}"))
            .await;
        // Ids whose ingestion order is not their document order, so the order the pages
        // report is the listing's own rather than the order the documents arrived in.
        let ids: Vec<String> = (0..held)
            .map(|position| format!("paged-{index}-{:02}", (held - position) * 7))
            .collect();
        emulator
            .ingest_all(&knowledge_base_id, &data_source_id, &ids)
            .await;
        let doomed: Vec<Value> = in_document_order(&ids).into_iter().take(deleted).collect();
        let surviving: Vec<Value> = in_document_order(&ids).into_iter().skip(deleted).collect();
        if !doomed.is_empty() {
            emulator
                .delete_all(&knowledge_base_id, &data_source_id, &doomed)
                .await;
        }
        rows.push(Row {
            case,
            data_source_id,
            doomed,
            surviving,
            max_results,
            statuses_seen: BTreeSet::new(),
        });
    }

    // The clock carries each deletion to its end and the removal then settles out of the
    // collection. The collection is watched the whole way: the only statuses it ever
    // reports for a document being deleted are that deletion's own.
    let deadline = Instant::now() + POLL_CAP;
    loop {
        let mut all_settled = true;
        for row in &mut rows {
            let listed = emulator
                .list(&knowledge_base_id, &row.data_source_id, None, None)
                .await;
            assert_eq!(
                listed.status, 200,
                "case {}: expected ListKnowledgeBaseDocuments -> 200, got {listed:?}",
                row.case
            );
            let details = listed.details().clone();
            for identifier in &row.doomed {
                if let Some(status) = status_of(&details, identifier) {
                    row.statuses_seen.insert(status);
                    all_settled = false;
                }
            }
            // The surviving documents reach the terminal status of their own ingestion on
            // the same clock, and the row is read only once both have happened.
            if row
                .surviving
                .iter()
                .any(|identifier| !reports(&details, identifier, INDEXED))
            {
                all_settled = false;
            }
        }
        if all_settled || Instant::now() >= deadline {
            break;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }

    for row in &rows {
        let case = row.case;
        assert_eq!(
            row.statuses_seen.is_empty(),
            row.doomed.is_empty(),
            "case {case}: expected the collection to report the deletion of the {} named \
             document(s) while it was settling, saw {:?}",
            row.doomed.len(),
            row.statuses_seen
        );
        assert!(
            row.statuses_seen
                .iter()
                .all(|status| DELETION_STATUSES.contains(&status.as_str())),
            "case {case}: the collection reported {:?} for a document being deleted; \
             expected only the statuses of that deletion {DELETION_STATUSES:?}",
            row.statuses_seen
        );

        let page_cap = row
            .max_results
            .map_or(MAX_DOCUMENTS_PER_PAGE, |requested| {
                usize::try_from(requested).unwrap_or(MAX_DOCUMENTS_PER_PAGE)
            })
            .min(MAX_DOCUMENTS_PER_PAGE);
        // A data source holding nothing reports one empty page: there is nothing to resume
        // after.
        let expected_pages = row.surviving.len().div_ceil(page_cap).max(1);
        let walked = walk(&emulator, &knowledge_base_id, row, expected_pages, page_cap).await;
        assert_eq!(
            walked,
            row.surviving,
            "case {case}: walking the pages must yield the {} document(s) the data source \
             holds once each, in document order, got {walked:?}",
            row.surviving.len()
        );
        for identifier in &row.doomed {
            assert!(
                !walked.contains(identifier),
                "case {case}: a document whose removal has settled occupies no page of the \
                 listing, got {walked:?}"
            );
        }

        // The deletion's outcome stays reportable through the targeted read, and each
        // surviving document keeps the terminal status of its own ingestion.
        let details = emulator
            .get_documents(&knowledge_base_id, &row.data_source_id, &row.doomed)
            .await;
        for identifier in &row.doomed {
            assert!(
                reports(&details, identifier, NOT_FOUND),
                "case {case}: expected the targeted read to report {NOT_FOUND} for the \
                 removed document {identifier:?}, got {details:?}"
            );
        }
        let details = emulator
            .get_documents(&knowledge_base_id, &row.data_source_id, &row.surviving)
            .await;
        for identifier in &row.surviving {
            assert!(
                reports(&details, identifier, INDEXED),
                "case {case}: expected the targeted read to report {INDEXED} for the \
                 surviving document {identifier:?}, got {details:?}"
            );
        }
    }
}
