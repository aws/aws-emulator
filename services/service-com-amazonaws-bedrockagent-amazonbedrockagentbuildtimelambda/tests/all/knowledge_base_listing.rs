// SPDX-License-Identifier: CC0-1.0

//! `ListKnowledgeBases` end to end over HTTP: the listing is a PROJECTION of the
//! knowledge bases this account's store holds, never a value of its own.
//!
//! The rule this suite pins, over tables rather than one example:
//!
//! * every stored knowledge base is reported exactly once, in a stable order,
//!   as the modelled `KnowledgeBaseSummary` — each member carrying the same
//!   stored value a read of that knowledge base reports;
//! * `maxResults` shortens the page and the `nextToken` it hands out resumes
//!   exactly after it, so the pages together are the whole listing once, and a
//!   page that reported everything left hands out no token;
//! * a `nextToken` this service never issued, and a `maxResults` outside the
//!   modelled range, are bad requests;
//! * the listing follows the store through the transitions the CLOCK owes: a
//!   knowledge base is reported `CREATING` and then `ACTIVE`, reports `DELETING`
//!   while its accepted removal is still pending, and leaves the listing once
//!   that removal comes due — and listing changes nothing for a later caller.
//!
//! Every assertion here goes through the router, so it exercises the real wire
//! path (route matching, model constraint validation, serialization) rather than
//! calling a handler directly.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_bedrockagent_amazonbedrockagentbuildtimelambda::state::{
    DELETION_SPAN, KNOWLEDGE_BASE_ACTIVATION_SPAN,
};
use service_com_amazonaws_bedrockagent_amazonbedrockagentbuildtimelambda::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 `Authorization` header scoped to the `bedrock` signing name: the
/// routing key this service claims on.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/eu-west-1/bedrock/aws4_request, \
     SignedHeaders=host;x-amz-date, Signature=0123456789abcdef";

/// The region of [`AUTHORIZATION`]'s credential scope.
const REGION: &str = "eu-west-1";

/// The account id every emulated ARN carries (`engine::EMULATOR_ACCOUNT_ID`).
const ACCOUNT: &str = "000000000000";

/// A role ARN matching the modelled `KnowledgeBaseRoleArn` pattern.
const KB_ROLE: &str =
    "arn:aws:iam::000000000000:role/service-role/AmazonBedrockExecutionRoleForKnowledgeBase";

/// An embedding model ARN matching the modelled `BedrockEmbeddingModelArn`
/// pattern, naming a model whose published output is an embedding.
const EMBEDDING_MODEL: &str =
    "arn:aws:bedrock:eu-west-1::foundation-model/amazon.titan-embed-text-v2:0";

/// Every member of the modelled `KnowledgeBaseSummary`, and the only members a
/// summary may carry.
///
/// Source: this service's Smithy model
/// (`smithy_query(query="shape",shape="KnowledgeBaseSummary")`).
const SUMMARY_MEMBERS: [&str; 5] = [
    "knowledgeBaseId",
    "name",
    "description",
    "status",
    "updatedAt",
];

/// How long a test waits past a span the clock owes before asserting the
/// transition it owed has been materialised.
const MARGIN: Duration = Duration::from_millis(200);

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
}

/// The emulator under test: one router over one shared engine, exactly as the
/// production router mounts this service.
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

    async fn create_knowledge_base(&self, request: Value) -> Answer {
        self.call("PUT", "/knowledgebases/", Some(request)).await
    }

    /// Create one knowledge base and report the id it was minted, failing the
    /// test if the create was not served.
    async fn knowledge_base(&self, case: &str, request: Value) -> String {
        let created = self.create_knowledge_base(request).await;
        assert_eq!(
            created.status, 202,
            "case {case}: expected CreateKnowledgeBase -> 202, got {created:?}"
        );
        created.string(&["knowledgeBase", "knowledgeBaseId"])
    }

    async fn get_knowledge_base(&self, knowledge_base_id: &str) -> Answer {
        self.call("GET", &format!("/knowledgebases/{knowledge_base_id}"), None)
            .await
    }

    async fn delete_knowledge_base(&self, knowledge_base_id: &str) -> Answer {
        self.call(
            "DELETE",
            &format!("/knowledgebases/{knowledge_base_id}"),
            None,
        )
        .await
    }

    async fn list(&self, max_results: Option<i64>, next_token: Option<&str>) -> Answer {
        let mut body = json!({});
        if let Some(max_results) = max_results {
            body["maxResults"] = json!(max_results);
        }
        if let Some(next_token) = next_token {
            body["nextToken"] = json!(next_token);
        }
        self.call("POST", "/knowledgebases/", Some(body)).await
    }

    /// One served `ListKnowledgeBases` page: its summaries and the token it
    /// hands out.
    async fn page(
        &self,
        case: &str,
        max_results: Option<i64>,
        next_token: Option<&str>,
    ) -> (Vec<Value>, Option<String>) {
        let listed = self.list(max_results, next_token).await;
        assert_eq!(
            listed.status, 200,
            "case {case}: expected ListKnowledgeBases -> 200, got {listed:?}"
        );
        let summaries = listed
            .member(&["knowledgeBaseSummaries"])
            .as_array()
            .unwrap_or_else(|| {
                panic!(
                    "case {case}: knowledgeBaseSummaries is a required list, got {:?}",
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

/// The `knowledgeBaseId` of one summary.
fn summary_id(case: &str, summary: &Value) -> String {
    summary["knowledgeBaseId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: a summary carries a knowledgeBaseId, {summary:?}"))
        .to_string()
}

/// An OpenSearch Serverless vector store, with every member its modelled shape
/// requires: a store that needs no index seeded in another service.
fn opensearch_storage(index: &str) -> Value {
    json!({
        "type": "OPENSEARCH_SERVERLESS",
        "opensearchServerlessConfiguration": {
            "collectionArn": format!("arn:aws:aoss:{REGION}:{ACCOUNT}:collection/listing"),
            "vectorIndexName": index,
            "fieldMapping": {
                "vectorField": "bedrock-knowledge-base-default-vector",
                "textField": "AMAZON_BEDROCK_TEXT_CHUNK",
                "metadataField": "AMAZON_BEDROCK_METADATA",
            },
        },
    })
}

/// A `CreateKnowledgeBase` body for a self-managed vector knowledge base, with
/// the description the row carries (if any).
fn create_request(name: &str, description: Option<&str>) -> Value {
    let mut request = json!({
        "name": name,
        "roleArn": KB_ROLE,
        "knowledgeBaseConfiguration": {
            "type": "VECTOR",
            "vectorKnowledgeBaseConfiguration": { "embeddingModelArn": EMBEDDING_MODEL },
        },
        "storageConfiguration": opensearch_storage("bedrock-knowledge-base-default-index"),
    });
    if let Some(description) = description {
        request["description"] = json!(description);
    }
    request
}

/// Create `count` knowledge bases and report their ids in the order they were
/// created.
async fn seed(emulator: &Emulator, prefix: &str, count: usize) -> Vec<String> {
    let mut ids = Vec::with_capacity(count);
    for position in 0..count {
        ids.push(
            emulator
                .knowledge_base(
                    &format!("{prefix} #{position}"),
                    create_request(&format!("{prefix}-{position}"), None),
                )
                .await,
        );
    }
    ids
}

/// Walk the whole listing `page_size` at a time and report the ids the pages
/// carried, asserting the paging invariants on every page.
async fn walk(emulator: &Emulator, case: &str, page_size: Option<i64>) -> Vec<String> {
    let mut seen: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    // One page per stored knowledge base is the most a terminating walk needs;
    // past that the token is not advancing and the test must fail rather than
    // loop.
    for _ in 0..64 {
        let (summaries, next) = emulator.page(case, page_size, token.as_deref()).await;
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

/// The listing reports a summary of every stored knowledge base, once each, in
/// creation order, with every member carrying the value a read of that knowledge
/// base reports — and nothing else.
#[tokio::test]
async fn the_listing_reports_a_summary_of_every_stored_knowledge_base() {
    let emulator = Emulator::new();

    // An account that holds nothing has nothing to report and nowhere to
    // resume.
    let (empty, token) = emulator.page("the empty account", None, None).await;
    assert!(
        empty.is_empty() && token.is_none(),
        "case the empty account: expected no summary and no nextToken, got {empty:?}/{token:?}"
    );

    // Rows differ along what the rule generalises over: whether the optional
    // `description` is carried, and the form of the modelled `Name`
    // (`^([0-9a-zA-Z][_-]?){1,100}$`) — letters, digits, both separators the
    // pattern admits, and the pattern's longest form.
    let rows: Vec<(&str, String, Option<&str>)> = vec![
        ("a plain name, no description", "listing-kb".to_string(), None),
        (
            "a name with digits and a description",
            "listing_kb_2".to_string(),
            Some("the second knowledge base of the listing suite"),
        ),
        (
            "a single-character name",
            "k".to_string(),
            Some("a one-character name is a whole name"),
        ),
        (
            "a name at the pattern's longest form",
            "a".repeat(100),
            None,
        ),
        (
            "a name whose both admitted separators appear",
            "listing-kb_again".to_string(),
            Some("Name admits one separator after each alphanumeric"),
        ),
    ];

    let mut created: Vec<(&str, String, String, Option<&str>)> = Vec::new();
    for (case, name, description) in &rows {
        let knowledge_base_id = emulator
            .knowledge_base(case, create_request(name, *description))
            .await;
        created.push((case, knowledge_base_id, name.clone(), *description));
    }

    let (summaries, token) = emulator.page("the whole listing", None, None).await;
    let listed: Vec<String> = summaries
        .iter()
        .map(|summary| summary_id("the whole listing", summary))
        .collect();
    let expected: Vec<String> = created.iter().map(|(_, id, _, _)| id.clone()).collect();
    assert_eq!(
        listed, expected,
        "case the whole listing: expected exactly the stored knowledge bases, in creation \
         order, got {listed:?}"
    );
    assert!(
        token.is_none(),
        "case the whole listing: a page that reported everything hands out no token, got \
         {token:?}"
    );

    // Each summary is the same stored record a read of that knowledge base
    // reports, projected onto the modelled summary members — and what the row
    // supplied is what both of them carry.
    for ((case, knowledge_base_id, name, description), summary) in
        created.iter().zip(summaries.iter())
    {
        let fetched = emulator.get_knowledge_base(knowledge_base_id).await;
        assert_eq!(
            fetched.status, 200,
            "case {case}: expected GetKnowledgeBase -> 200, got {fetched:?}"
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
                fetched.member(&["knowledgeBase", member]),
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
            "case {case}: a summary carries only the modelled KnowledgeBaseSummary members, \
             got {extra:?} as well"
        );
    }
}

/// `maxResults` shortens the page and the token resumes exactly after it: the
/// pages together are the whole listing once, whatever the page size — over a
/// table of page sizes against a table of stored counts.
#[tokio::test]
async fn max_results_pages_the_listing_without_losing_or_repeating_one() {
    // Rows: the stored count spans an empty account, one knowledge base and
    // several; the page sizes span one, a divisor of the count, a size the count
    // does not divide by, exactly the count, one more than the count, and each
    // published bound of the modelled range (1..1000).
    for count in [0_usize, 1, 7] {
        let emulator = Emulator::new();
        let stored = seed(&emulator, "paging-kb", count).await;

        for page_size in [None, Some(1), Some(2), Some(3), Some(7), Some(8), Some(1000)] {
            let case = format!("{count} stored, maxResults={page_size:?}");
            let walked = walk(&emulator, &case, page_size).await;
            assert_eq!(
                walked, stored,
                "case {case}: the pages together are the whole listing once, in creation \
                 order, got {walked:?}"
            );
        }

        // The pages are cut from one order, so the first page of any size is the
        // head of the whole listing.
        for page_size in [1, 2, 7, 1000] {
            let case = format!("{count} stored, the first page of {page_size}");
            let (summaries, _) = emulator.page(&case, Some(page_size), None).await;
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

/// A `nextToken` this service never issued names no position in the listing, and
/// a `maxResults` outside the modelled range is refused at the boundary — and
/// neither changes what the listing reports.
#[tokio::test]
async fn the_listing_refuses_a_foreign_token_and_a_max_results_out_of_range() {
    let emulator = Emulator::new();
    let stored = seed(&emulator, "refusal-kb", 3).await;

    // Rows: strings no page of this listing handed out. A position names the
    // knowledge base a page ended at, so nothing else names one.
    for (case, token) in [
        ("a token from somewhere else", "not-a-pagination-token"),
        ("a bare identifier", "aZ09aZ09aZ"),
        ("a bare position with no knowledge base", "1700000000000000000-"),
        ("a knowledge base with no position", "-aZ09aZ09aZ"),
        ("a position that is not a whole number", "1.5-aZ09aZ09aZ"),
        ("a hexadecimal position", "0x10-aZ09aZ09aZ"),
        ("a signed position", "+1-aZ09aZ09aZ"),
        ("a zero-padded position", "01-aZ09aZ09aZ"),
        (
            "an identifier no knowledge base could be minted",
            "1700000000000000000-aZ09:aZ09a",
        ),
        (
            "an identifier shorter than a minted one",
            "1700000000000000000-aZ09aZ09",
        ),
    ] {
        let refused = emulator.list(None, Some(token)).await;
        assert_eq!(
            refused.status, 400,
            "case {case} ({token}): expected ListKnowledgeBases -> 400, got {refused:?}"
        );
        assert_eq!(
            refused.error_type.as_deref(),
            Some("ValidationException"),
            "case {case} ({token}): expected the modelled ValidationException, got {:?}",
            refused.error_type
        );
    }

    // `nextToken` is modelled `length 1..2048, pattern ^\S*$` and `maxResults`
    // `range 1..1000`: one past a published bound is refused before the handler
    // runs.
    let long_token = "a".repeat(2049);
    for (case, max_results, token) in [
        ("one below the maxResults lower bound", Some(0), None),
        ("one above the maxResults upper bound", Some(1001), None),
        ("a negative maxResults", Some(-1), None),
        ("an empty nextToken", None, Some("")),
        ("a nextToken carrying whitespace", None, Some("1 2")),
        ("a nextToken past its length bound", None, Some(long_token.as_str())),
    ] {
        let refused = emulator.list(max_results, token).await;
        assert_eq!(
            refused.status, 400,
            "case {case} (maxResults={max_results:?}, nextToken={token:?}): expected \
             ListKnowledgeBases -> 400, got {refused:?}"
        );
    }

    // Every refusal above left the stored knowledge bases exactly as they were.
    let walked = walk(&emulator, "after the refusals", None).await;
    assert_eq!(
        walked, stored,
        "case after the refusals: a refused listing changes nothing, got {walked:?}"
    );
}

/// The listing follows the store through the transitions the CLOCK owes: it
/// reports `CREATING` and then `ACTIVE`, reports `DELETING` while an accepted
/// removal is still pending, and drops the knowledge base once that removal
/// comes due — while listing itself changes nothing for a later caller.
#[tokio::test]
async fn the_listing_follows_the_store_through_activation_and_deletion() {
    let emulator = Emulator::new();

    // The transient status, asserted on a listing taken straight after ONE
    // create. The window the create happened in and the window the listing
    // happened in are both measured, so the assertion is decided by the same
    // clock the handler reads: while the whole listing provably fell inside the
    // activation span it must still report CREATING, once it provably fell past
    // it the clock owed ACTIVE, and only a round trip that straddles the span
    // admits either — so the assertion cannot race it.
    let create_began = std::time::Instant::now();
    let first = emulator
        .knowledge_base("the first create", create_request("clock-kb-0", None))
        .await;
    let create_ended = std::time::Instant::now();
    let listing_began = std::time::Instant::now();
    let (summaries, _) = emulator.page("immediately after the create", None, None).await;
    let listing_ended = std::time::Instant::now();
    let owed: &[&str] = if listing_ended < create_began + KNOWLEDGE_BASE_ACTIVATION_SPAN {
        &["CREATING"]
    } else if listing_began > create_ended + KNOWLEDGE_BASE_ACTIVATION_SPAN {
        &["ACTIVE"]
    } else {
        &["CREATING", "ACTIVE"]
    };
    assert_eq!(
        summaries.len(),
        1,
        "case immediately after the create: expected the one knowledge base just created, \
         got {summaries:?}"
    );
    let status = summaries[0]["status"].as_str().unwrap_or_default();
    assert!(
        owed.contains(&status),
        "case immediately after the create: the clock owes this knowledge base one of \
         {owed:?}, got {status:?} in {:?}",
        summaries[0]
    );

    let mut stored = vec![first];
    for position in 1..3 {
        stored.push(
            emulator
                .knowledge_base(
                    &format!("the create #{position}"),
                    create_request(&format!("clock-kb-{position}"), None),
                )
                .await,
        );
    }

    // Past the span the transition is owed whether or not anybody called, and
    // the listing reports it exactly as a read of the knowledge base does.
    tokio::time::sleep(KNOWLEDGE_BASE_ACTIVATION_SPAN + MARGIN).await;
    let (summaries, _) = emulator.page("after the activation span", None, None).await;
    for summary in &summaries {
        assert_eq!(
            summary["status"].as_str(),
            Some("ACTIVE"),
            "case after the activation span: the clock owes the knowledge base ACTIVE, got \
             {summary:?}"
        );
    }

    // Listing is a read: it changes nothing a later caller sees.
    let (again, _) = emulator.page("a repeated listing", None, None).await;
    assert_eq!(
        again, summaries,
        "case a repeated listing: a read must not change what a later caller sees, got \
         {again:?}"
    );

    // An accepted delete puts the removal on the clock; until it comes due the
    // knowledge base is still held, and the listing reports it as DELETING.
    let deleting = &stored[1];
    let accepted = emulator.delete_knowledge_base(deleting).await;
    assert_eq!(
        accepted.status, 202,
        "case the accepted delete: expected DeleteKnowledgeBase -> 202, got {accepted:?}"
    );
    let (summaries, _) = emulator.page("while the removal is pending", None, None).await;
    let pending: Vec<(String, String)> = summaries
        .iter()
        .map(|summary| {
            (
                summary_id("while the removal is pending", summary),
                summary["status"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect();
    let expected: Vec<(String, String)> = stored
        .iter()
        .map(|knowledge_base_id| {
            let status = if knowledge_base_id == deleting {
                "DELETING"
            } else {
                "ACTIVE"
            };
            (knowledge_base_id.clone(), status.to_string())
        })
        .collect();
    assert_eq!(
        pending, expected,
        "case while the removal is pending: the listing reports every held knowledge base, \
         the deleted one as DELETING, got {pending:?}"
    );

    // Once the removal comes due the knowledge base is gone from the listing,
    // and the ones around it are untouched.
    tokio::time::sleep(DELETION_SPAN + MARGIN).await;
    let walked = walk(&emulator, "after the removal came due", None).await;
    let survivors: Vec<String> = stored
        .iter()
        .filter(|knowledge_base_id| *knowledge_base_id != deleting)
        .cloned()
        .collect();
    assert_eq!(
        walked, survivors,
        "case after the removal came due: a removed knowledge base is in no listing, got \
         {walked:?}"
    );
}
