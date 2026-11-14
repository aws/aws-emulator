// SPDX-License-Identifier: CC0-1.0

//! `ListDataSources` end to end over HTTP: the listing is a PROJECTION of the data sources
//! one knowledge base's store holds, never a value of its own.
//!
//! The rule this suite pins, over tables rather than one example:
//!
//! * every data source of the knowledge base is reported exactly once, in a stable order,
//!   as the modelled `DataSourceSummary` — each member carrying the same stored value a read
//!   of that data source reports — and no other knowledge base's data source is;
//! * `maxResults` shortens the page and the `nextToken` it hands out resumes exactly after
//!   it, so the pages together are the whole listing once, and a page that reported
//!   everything left hands out no token;
//! * a `knowledgeBaseId` this store holds no knowledge base for has nothing to enumerate, a
//!   `nextToken` this service never issued names no position, and a `maxResults` outside the
//!   modelled range is refused at the boundary;
//! * the listing follows the store through the transitions the CLOCK owes, and listing
//!   changes nothing a later caller sees.
//!
//! Every assertion here goes through the router, so it exercises the real wire path (route
//! matching, model constraint validation, serialization) rather than calling a handler
//! directly.

use std::sync::Arc;

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

/// Every member of the modelled `DataSourceSummary`, and the only members a summary may
/// carry.
///
/// Source: this service's Smithy model
/// (`smithy_query(query="shape",shape="DataSourceSummary")`).
const SUMMARY_MEMBERS: [&str; 6] = [
    "knowledgeBaseId",
    "dataSourceId",
    "name",
    "status",
    "description",
    "updatedAt",
];

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

    /// Create one knowledge base and report its minted id.
    async fn knowledge_base(&self, case: &str, name: &str) -> String {
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
                            "embeddingModelArn": EMBEDDING_MODEL
                        },
                    },
                    "storageConfiguration": {
                        "type": "OPENSEARCH_SERVERLESS",
                        "opensearchServerlessConfiguration": {
                            "collectionArn":
                                "arn:aws:aoss:eu-west-1:000000000000:collection/listing",
                            "vectorIndexName": "listing-index",
                            "fieldMapping": {
                                "vectorField": "bedrock-knowledge-base-default-vector",
                                "textField": "AMAZON_BEDROCK_TEXT_CHUNK",
                                "metadataField": "AMAZON_BEDROCK_METADATA",
                            },
                        },
                    },
                })),
            )
            .await;
        assert_eq!(
            created.status, 202,
            "case {case}: expected CreateKnowledgeBase -> 202, got {created:?}"
        );
        created.string(&["knowledgeBase", "knowledgeBaseId"])
    }

    /// Create one data source of a knowledge base and report its minted id.
    async fn data_source(
        &self,
        case: &str,
        knowledge_base_id: &str,
        name: &str,
        description: Option<&str>,
    ) -> String {
        let mut request = json!({
            "name": name,
            "dataSourceConfiguration": { "type": "CUSTOM" },
        });
        if let Some(description) = description {
            request["description"] = json!(description);
        }
        let created = self
            .call(
                "PUT",
                &format!("/knowledgebases/{knowledge_base_id}/datasources/"),
                Some(request),
            )
            .await;
        assert_eq!(
            created.status, 200,
            "case {case}: expected CreateDataSource -> 200, got {created:?}"
        );
        created.string(&["dataSource", "dataSourceId"])
    }

    async fn get_data_source(&self, knowledge_base_id: &str, data_source_id: &str) -> Answer {
        self.call(
            "GET",
            &format!("/knowledgebases/{knowledge_base_id}/datasources/{data_source_id}"),
            None,
        )
        .await
    }

    async fn list(
        &self,
        knowledge_base_id: &str,
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
            &format!("/knowledgebases/{knowledge_base_id}/datasources/"),
            Some(body),
        )
        .await
    }

    /// One served page: its summaries and the token it hands out.
    async fn page(
        &self,
        case: &str,
        knowledge_base_id: &str,
        max_results: Option<i64>,
        next_token: Option<&str>,
    ) -> (Vec<Value>, Option<String>) {
        let listed = self.list(knowledge_base_id, max_results, next_token).await;
        assert_eq!(
            listed.status, 200,
            "case {case}: expected ListDataSources -> 200, got {listed:?}"
        );
        let summaries = listed
            .member(&["dataSourceSummaries"])
            .as_array()
            .unwrap_or_else(|| {
                panic!(
                    "case {case}: dataSourceSummaries is a required list, got {:?}",
                    listed.body
                )
            })
            .clone();
        let token = listed
            .member(&["nextToken"])
            .as_str()
            .map(ToString::to_string);
        (summaries, token)
    }
}

/// The `dataSourceId` of one summary.
fn summary_id(case: &str, summary: &Value) -> String {
    summary["dataSourceId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: a summary carries a dataSourceId, {summary:?}"))
        .to_string()
}

/// Walk the whole listing `page_size` at a time and report the ids the pages carried,
/// asserting the paging invariants on every page.
async fn walk(
    emulator: &Emulator,
    case: &str,
    knowledge_base_id: &str,
    page_size: Option<i64>,
) -> Vec<String> {
    let mut seen: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    // One page per stored data source is the most a terminating walk needs; past that the
    // token is not advancing and the test must fail rather than loop.
    for _ in 0..64 {
        let (summaries, next) = emulator
            .page(case, knowledge_base_id, page_size, token.as_deref())
            .await;
        if let Some(page_size) = page_size {
            let page_size = usize::try_from(page_size).unwrap_or(usize::MAX);
            assert!(
                summaries.len() <= page_size,
                "case {case}: a page carries at most the maxResults asked for ({page_size}), \
                 got {} summaries",
                summaries.len()
            );
            assert!(
                next.is_none() || summaries.len() == page_size,
                "case {case}: a page that hands out a token is a full page, got {} summaries",
                summaries.len()
            );
        }
        seen.extend(summaries.iter().map(|summary| summary_id(case, summary)));
        token = next;
        if token.is_none() {
            return seen;
        }
    }
    panic!("case {case}: the walk did not terminate after 64 pages, saw {seen:?}");
}

/// The listing reports a summary of every data source of the knowledge base, once each, in
/// creation order, with every member carrying the value a read of that data source reports —
/// and nothing of another knowledge base's.
#[tokio::test]
async fn the_listing_reports_a_summary_of_every_data_source_of_the_knowledge_base() {
    let emulator = Emulator::new();
    let knowledge_base_id = emulator.knowledge_base("the listed store", "listing-ds-kb").await;
    let other_id = emulator.knowledge_base("the other store", "listing-ds-other").await;

    // A knowledge base that holds no data source has nothing to report and nowhere to
    // resume.
    let (empty, token) = emulator
        .page("a knowledge base with no data source", &knowledge_base_id, None, None)
        .await;
    assert!(
        empty.is_empty() && token.is_none(),
        "case a knowledge base with no data source: expected no summary and no nextToken, got \
         {empty:?}/{token:?}"
    );

    // Rows differ along what the rule generalises over: whether the optional `description`
    // is carried, and the form of the modelled `Name` (`^([0-9a-zA-Z][_-]?){1,100}$`) —
    // letters, digits, both separators the pattern admits, and its longest form.
    let rows: Vec<(&str, String, Option<&str>)> = vec![
        ("a plain name, no description", "listing-ds".to_string(), None),
        (
            "a name with digits and a description",
            "listing_ds_2".to_string(),
            Some("the second data source of the listing suite"),
        ),
        (
            "a single-character name",
            "d".to_string(),
            Some("a one-character name is a whole name"),
        ),
        ("a name at the pattern's longest form", "a".repeat(100), None),
    ];

    let mut created: Vec<(&str, String, String, Option<&str>)> = Vec::new();
    for (case, name, description) in &rows {
        let data_source_id = emulator
            .data_source(case, &knowledge_base_id, name, *description)
            .await;
        created.push((case, data_source_id, name.clone(), *description));
    }
    // One data source of ANOTHER knowledge base, which this listing must never report.
    let foreign = emulator
        .data_source("the other store's data source", &other_id, "listing-ds-foreign", None)
        .await;

    let (summaries, token) = emulator
        .page("the whole listing", &knowledge_base_id, None, None)
        .await;
    let listed: Vec<String> = summaries
        .iter()
        .map(|summary| summary_id("the whole listing", summary))
        .collect();
    let expected: Vec<String> = created.iter().map(|(_, id, _, _)| id.clone()).collect();
    assert_eq!(
        listed, expected,
        "case the whole listing: expected exactly this knowledge base's data sources, in \
         creation order, got {listed:?}"
    );
    assert!(
        !listed.contains(&foreign),
        "case the whole listing: another knowledge base's data source is in no listing of \
         this one, got {listed:?}"
    );
    assert!(
        token.is_none(),
        "case the whole listing: a page that reported everything hands out no token, got \
         {token:?}"
    );

    // Each summary is the same stored record a read of that data source reports, projected
    // onto the modelled summary members.
    for ((case, data_source_id, name, description), summary) in created.iter().zip(summaries.iter())
    {
        let fetched = emulator
            .get_data_source(&knowledge_base_id, data_source_id)
            .await;
        assert_eq!(
            fetched.status, 200,
            "case {case}: expected GetDataSource -> 200, got {fetched:?}"
        );
        assert_eq!(
            summary["name"].as_str(),
            Some(name.as_str()),
            "case {case}: the summary reports the name the create supplied, got {:?}",
            summary["name"]
        );
        assert_eq!(
            summary["description"].as_str(),
            *description,
            "case {case}: the summary reports the description the create supplied, got {:?}",
            summary["description"]
        );
        for member in SUMMARY_MEMBERS {
            assert_eq!(
                &summary[member],
                fetched.member(&["dataSource", member]),
                "case {case}: the summary's {member} must be the stored value the read \
                 reports, got {:?}",
                summary[member]
            );
        }
        let extra: Vec<&String> = summary
            .as_object()
            .unwrap_or_else(|| panic!("case {case}: a summary is a structure, got {summary:?}"))
            .keys()
            .filter(|member| !SUMMARY_MEMBERS.contains(&member.as_str()))
            .collect();
        assert!(
            extra.is_empty(),
            "case {case}: a summary carries only the modelled DataSourceSummary members, got \
             {extra:?} as well"
        );
    }

    // Listing is a read: it changes nothing a later caller sees.
    let (again, _) = emulator
        .page("a repeated listing", &knowledge_base_id, None, None)
        .await;
    assert_eq!(
        again, summaries,
        "case a repeated listing: a read must not change what a later caller sees, got \
         {again:?}"
    );
}

/// `maxResults` shortens the page and the token resumes exactly after it: the pages together
/// are the whole listing once, whatever the page size — over a table of page sizes against a
/// table of stored counts.
#[tokio::test]
async fn max_results_pages_the_listing_without_losing_or_repeating_one() {
    // Rows: the stored count spans a knowledge base with none, with one and with several;
    // the page sizes span one, a divisor of the count, a size the count does not divide by,
    // exactly the count, one more, and each published bound of the modelled range (1..1000).
    for count in [0_usize, 1, 7] {
        let emulator = Emulator::new();
        let knowledge_base_id = emulator
            .knowledge_base("the paged store", &format!("paging-ds-kb-{count}"))
            .await;
        let mut stored = Vec::with_capacity(count);
        for position in 0..count {
            stored.push(
                emulator
                    .data_source(
                        &format!("the data source #{position}"),
                        &knowledge_base_id,
                        &format!("paging-ds-{position}"),
                        None,
                    )
                    .await,
            );
        }

        for page_size in [None, Some(1), Some(2), Some(3), Some(7), Some(8), Some(1000)] {
            let case = format!("{count} stored, maxResults={page_size:?}");
            let walked = walk(&emulator, &case, &knowledge_base_id, page_size).await;
            assert_eq!(
                walked, stored,
                "case {case}: the pages together are the whole listing once, in creation \
                 order, got {walked:?}"
            );
        }

        // The pages are cut from one order, so the first page of any size is the head of the
        // whole listing.
        for page_size in [1, 2, 7, 1000] {
            let case = format!("{count} stored, the first page of {page_size}");
            let (summaries, _) = emulator
                .page(&case, &knowledge_base_id, Some(page_size), None)
                .await;
            let head: Vec<String> = summaries
                .iter()
                .map(|summary| summary_id(&case, summary))
                .collect();
            let wanted: Vec<String> = stored
                .iter()
                .take(usize::try_from(page_size).unwrap_or(usize::MAX))
                .cloned()
                .collect();
            assert_eq!(
                head, wanted,
                "case {case}: the first page is the head of the listing, got {head:?}"
            );
        }
    }
}

/// An unknown `knowledgeBaseId` has nothing to enumerate, a `nextToken` this service never
/// issued names no position, and a `maxResults` outside the modelled range is refused — and
/// none of them changes what the listing reports.
#[tokio::test]
async fn the_listing_refuses_an_unknown_knowledge_base_a_foreign_token_and_a_bad_page_size() {
    let emulator = Emulator::new();
    let knowledge_base_id = emulator.knowledge_base("the refusing store", "refusal-ds-kb").await;
    let stored = vec![
        emulator
            .data_source("the first", &knowledge_base_id, "refusal-ds-0", None)
            .await,
        emulator
            .data_source("the second", &knowledge_base_id, "refusal-ds-1", None)
            .await,
    ];

    // A knowledge base this store does not hold: the missing resource, not an empty page.
    // Rows: an id of the minted form nothing holds, and the forms a well-formed id takes.
    for (case, unknown) in [
        ("an id nothing was minted for", "aZ09aZ09aZ"),
        ("an all-digit id", "0123456789"),
        ("an all-letter id", "abcdefghij"),
    ] {
        let refused = emulator.list(unknown, None, None).await;
        assert_eq!(
            refused.status, 404,
            "case {case} ({unknown}): expected ListDataSources -> 404, got {refused:?}"
        );
        assert_eq!(
            refused.error_type.as_deref(),
            Some("ResourceNotFoundException"),
            "case {case} ({unknown}): expected the modelled ResourceNotFoundException, got \
             {refused:?}"
        );
    }

    // Rows: strings no page of this listing handed out. A position names the data source a
    // page ended at, so nothing else names one.
    for (case, token) in [
        ("a token from somewhere else", "not-a-pagination-token"),
        ("a bare identifier", "aZ09aZ09aZ"),
        ("a bare position with no data source", "1700000000000000000-"),
        ("a data source with no position", "-aZ09aZ09aZ"),
        ("a position that is not a whole number", "1.5-aZ09aZ09aZ"),
        ("a hexadecimal position", "0x10-aZ09aZ09aZ"),
        ("a signed position", "+1-aZ09aZ09aZ"),
        ("a zero-padded position", "01-aZ09aZ09aZ"),
        (
            "an identifier no data source could be minted",
            "1700000000000000000-aZ09:aZ09a",
        ),
        (
            "an identifier shorter than a minted one",
            "1700000000000000000-aZ09aZ09",
        ),
    ] {
        let refused = emulator.list(&knowledge_base_id, None, Some(token)).await;
        assert_eq!(
            refused.status, 400,
            "case {case} ({token}): expected ListDataSources -> 400, got {refused:?}"
        );
        assert_eq!(
            refused.error_type.as_deref(),
            Some("ValidationException"),
            "case {case} ({token}): expected the modelled ValidationException, got {:?}",
            refused.error_type
        );
    }

    // `nextToken` is modelled `length 1..2048, pattern ^\S*$` and `maxResults`
    // `range 1..1000`: one past a published bound is refused before the handler runs.
    let long_token = "a".repeat(2049);
    for (case, max_results, token) in [
        ("one below the maxResults lower bound", Some(0), None),
        ("one above the maxResults upper bound", Some(1001), None),
        ("a negative maxResults", Some(-1), None),
        ("an empty nextToken", None, Some("")),
        ("a nextToken carrying whitespace", None, Some("1 2")),
        (
            "a nextToken past its length bound",
            None,
            Some(long_token.as_str()),
        ),
    ] {
        let refused = emulator
            .list(&knowledge_base_id, max_results, token)
            .await;
        assert_eq!(
            refused.status, 400,
            "case {case} (maxResults={max_results:?}, nextToken={token:?}): expected \
             ListDataSources -> 400, got {refused:?}"
        );
    }

    // Every refusal above left the stored data sources exactly as they were.
    let walked = walk(&emulator, "after the refusals", &knowledge_base_id, None).await;
    assert_eq!(
        walked, stored,
        "case after the refusals: a refused listing changes nothing, got {walked:?}"
    );
}
