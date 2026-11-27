// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of the ONE paging path every listing of this service shares.
//!
//! Every case drives the real restJson1 wire path — the router picks the service from the
//! request's SigV4 scope, the framework deserializes and validates, the handler runs, and
//! the response is serialized — so what is asserted is the bytes a client would see.
//!
//! The rule under test, which every row below is one case of: a paged listing reports its
//! whole selected set, ordered on the key the request names and in the direction it names,
//! in pages of at most `maxResults`, each page carrying an opaque `nextToken` that resumes
//! immediately after its last item and carrying none when the page is the last — so the
//! same set comes back in the same order whatever page size the caller walks it with, and
//! a token this service never issued is refused.
//!
//! The rows span the listings that reach that path from different operations — job
//! templates, presets, queues and the Job engine version catalogue — the page sizes 1
//! through the default 20, both `Order` directions and both `listBy` keys, so a change to
//! the ordering, the anchor comparison, the page-size fallback or the token condition
//! cannot land in one listing and miss another.
//!
//! The account's own job templates, presets and queues are seeded straight into the store
//! at the creation instants each case needs, as the ordering suites beside this one do; the
//! write side's own coverage lives in `tests/all/job_template_crud.rs`,
//! `tests/all/preset_lifecycle.rs` and `tests/all/queue_update.rs`. The version catalogue is
//! the one the account comes up with. Nothing else here reaches past HTTP.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::Value;
use service_com_amazonaws_mediaconvert_mediaconvert::ops::types::{
    ContainerSettings, JobTemplateSettings, PresetSettings,
};
use service_com_amazonaws_mediaconvert_mediaconvert::state::{
    default_acceleration_settings, JobTemplateRecord, PresetRecord, QueueRecord,
    DEFAULT_CONCURRENT_JOBS, DEFAULT_MAXIMUM_CONCURRENT_FEEDS, DEFAULT_PRIORITY,
    DEFAULT_STATUS_UPDATE_INTERVAL, PRICING_PLAN_ON_DEMAND, QUEUE_STATUS_ACTIVE,
    QUEUE_TYPE_CUSTOM, TYPE_CUSTOM,
};
use service_com_amazonaws_mediaconvert_mediaconvert::{
    ComAmazonawsMediaconvertMediaconvertState, EmulatedService,
};
use smithy_server_runtime::MultiServiceRouter;

/// The region every case signs for.
const REGION: &str = "us-west-2";

/// The page size a listing uses when the caller gives no `maxResults`, which is also the
/// top of the model's own `__integerMin1Max20` range on every `MaxResults` member here.
const DEFAULT_PAGE_SIZE: usize = 20;

/// A SigV4 `Authorization` header in this service's scope. It is what the router reads to
/// pick the service, and what a handler reads the ARN region from.
fn authorization(region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 \
         Credential=AKIDTESTTESTTESTTEST/20260101/{region}/mediaconvert/aws4_request, \
         SignedHeaders=host;x-amz-date, Signature=deadbeefdeadbeefdeadbeefdeadbeefdeadbeef"
    )
}

/// One HTTP reply, decoded far enough to assert on.
struct Reply {
    status: u16,
    /// `x-amzn-errortype`, the header restJson1 resolves an error shape from.
    error_type: Option<String>,
    /// The JSON body, or `Value::Null` for an empty one.
    body: Value,
}

impl Reply {
    /// The `nextToken`, or `None` when the response carries none.
    fn next_token(&self) -> Option<String> {
        self.body
            .pointer("/nextToken")
            .and_then(Value::as_str)
            .map(str::to_string)
    }
}

/// One emulator: a single shared engine behind the multi-service router, exactly as the
/// production router assembles it.
struct Emulator {
    engine: Arc<engine::InMemoryEngine>,
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        // The Job engine version catalogue is behind the account's version selection
        // capability, which is granted out of band: this account is granted it so the
        // `ListVersions` rows below are about paging and not about the capability
        // (`tests/all/fixed_refusal_sentences.rs` owns the refusal an account without it
        // earns).
        engine
            .state::<ComAmazonawsMediaconvertMediaconvertState>()
            .lock()
            .set_version_selection_access(true);
        let mut router = MultiServiceRouter::new();
        // The signature itself is the transport's business, not this suite's; the
        // Authorization header is still present because routing and the ARN region read
        // the credential scope out of it.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        Self { engine, router }
    }

    async fn get(&self, uri: &str) -> Reply {
        let request = ::http::Request::builder()
            .method("GET")
            .uri(uri)
            .header("host", format!("mediaconvert.{REGION}.amazonaws.com"))
            .header("authorization", authorization(REGION))
            .header("content-type", "application/json")
            .body(::bytes::Bytes::new())
            .expect("build the request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let error_type = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let raw = response.body().clone();
        let body = if raw.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&raw).unwrap_or(Value::Null)
        };
        Reply {
            status,
            error_type,
            body,
        }
    }
}

/// One of the listings under test: where it is bound and how its page reports the records
/// it returned.
///
/// The bindings come from the model
/// (`smithy_query(query="digest")`): `GET /2017-08-29/jobTemplates`,
/// `GET /2017-08-29/presets`, `GET /2017-08-29/queues` and `GET /2017-08-29/versions`.
struct Listing {
    /// How a failing assertion names this listing.
    label: &'static str,
    /// The operation's HTTP URI.
    path: &'static str,
    /// The output member the page's records come back in.
    items: &'static str,
    /// The member of a record that identifies it — the second half of the key the listing
    /// orders and resumes on.
    id_member: &'static str,
    /// Whether the operation models `listBy` and `order`. `ListVersions` models neither, so
    /// its one ordering is not the caller's to choose.
    ordered_by_request: bool,
}

/// Every listing of this service that pages a stored set.
const LISTINGS: [Listing; 4] = [
    Listing {
        label: "ListJobTemplates",
        path: "/2017-08-29/jobTemplates",
        items: "jobTemplates",
        id_member: "name",
        ordered_by_request: true,
    },
    Listing {
        label: "ListPresets",
        path: "/2017-08-29/presets",
        items: "presets",
        id_member: "name",
        ordered_by_request: true,
    },
    Listing {
        label: "ListQueues",
        path: "/2017-08-29/queues",
        items: "queues",
        id_member: "name",
        ordered_by_request: true,
    },
    Listing {
        label: "ListVersions",
        path: "/2017-08-29/versions",
        items: "versions",
        id_member: "version",
        ordered_by_request: false,
    },
];

impl Listing {
    /// The ids of the records one reply reported, in the order they came back.
    fn ids(&self, reply: &Reply) -> Vec<String> {
        reply
            .body
            .pointer(&format!("/{}", self.items))
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .map(|item| {
                        item.pointer(&format!("/{}", self.id_member))
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string()
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// One page of this listing: `query` with `page_size` and any `token` appended.
    async fn page(
        &self,
        emulator: &Emulator,
        query: &str,
        page_size: Option<usize>,
        token: Option<&str>,
    ) -> Reply {
        let mut parts: Vec<String> = Vec::new();
        if !query.is_empty() {
            parts.push(query.to_string());
        }
        if let Some(size) = page_size {
            parts.push(format!("maxResults={size}"));
        }
        if let Some(token) = token {
            parts.push(format!("nextToken={token}"));
        }
        let uri = if parts.is_empty() {
            self.path.to_string()
        } else {
            format!("{}?{}", self.path, parts.join("&"))
        };
        emulator.get(&uri).await
    }

    /// Walk this listing to its end at `page_size`, asserting the per-page part of the rule
    /// as it goes, and return the ids in the order the walk saw them.
    ///
    /// The walk is bounded: a token that did not advance, or more pages than the set could
    /// possibly hold, fails the case instead of looping.
    async fn walk(&self, emulator: &Emulator, query: &str, page_size: Option<usize>) -> Vec<String> {
        let case = format!("{} {query:?} maxResults={page_size:?}", self.label);
        let expected_size = page_size.unwrap_or(DEFAULT_PAGE_SIZE);
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        for page_number in 1..=64 {
            let reply = self.page(emulator, query, page_size, token.as_deref()).await;
            assert_eq!(
                reply.status, 200,
                "case {case}: page {page_number}: expected HTTP 200, got {} with body {}",
                reply.status, reply.body
            );
            let ids = self.ids(&reply);
            assert!(
                ids.len() <= expected_size,
                "case {case}: page {page_number}: expected at most {expected_size} records, got {} ({ids:?})",
                ids.len()
            );
            assert!(
                !ids.is_empty(),
                "case {case}: page {page_number}: a page of a non-empty set reports records, got {ids:?}"
            );
            for id in &ids {
                assert!(
                    !seen.contains(id),
                    "case {case}: page {page_number}: {id:?} was already returned on an earlier page, after {seen:?}"
                );
            }
            seen.extend(ids.iter().cloned());
            match reply.next_token() {
                None => return seen,
                Some(next) => {
                    assert_eq!(
                        ids.len(),
                        expected_size,
                        "case {case}: page {page_number}: a page short of {expected_size} records is the last one, so no nextToken was due, got {next:?}"
                    );
                    assert_ne!(
                        Some(next.as_str()),
                        token.as_deref(),
                        "case {case}: page {page_number}: the nextToken did not advance past {token:?}"
                    );
                    token = Some(next);
                }
            }
        }
        panic!("case {case}: the walk did not reach a last page within 64 pages, saw {seen:?}");
    }
}

/// An instant `seconds_ago` before now, for seeding a creation order that is deliberately
/// not the alphabetical one.
fn ago(seconds_ago: u64) -> SystemTime {
    SystemTime::now() - Duration::from_secs(seconds_ago)
}

/// Put one of the account's own ([`TYPE_CUSTOM`]) job templates in the store.
fn seed_job_template(emulator: &Emulator, name: &str, created_at: SystemTime) {
    emulator
        .engine
        .state::<ComAmazonawsMediaconvertMediaconvertState>()
        .lock()
        .job_templates
        .insert(
            name.to_string(),
            JobTemplateRecord {
                name: name.to_string(),
                category: None,
                description: Some(format!("{name} description")),
                queue: None,
                priority: DEFAULT_PRIORITY,
                settings: JobTemplateSettings::default(),
                status_update_interval: DEFAULT_STATUS_UPDATE_INTERVAL.to_string(),
                acceleration_settings: default_acceleration_settings(),
                elemental_inference_settings: None,
                hop_destinations: Vec::new(),
                template_type: TYPE_CUSTOM.to_string(),
                created_at,
                last_updated: created_at,
                tags: BTreeMap::new(),
            },
        );
}

/// Put one of the account's own ([`TYPE_CUSTOM`]) presets in the store.
fn seed_preset(emulator: &Emulator, name: &str, created_at: SystemTime) {
    emulator
        .engine
        .state::<ComAmazonawsMediaconvertMediaconvertState>()
        .lock()
        .presets
        .insert(
            name.to_string(),
            PresetRecord {
                name: name.to_string(),
                category: None,
                description: Some(format!("{name} description")),
                // A stored preset names the container of the output it describes
                // (`ContainerSettings$Container`), as one created over HTTP has to.
                settings: PresetSettings {
                    container_settings: Some(ContainerSettings {
                        container: Some("MP4".to_string()),
                        ..ContainerSettings::default()
                    }),
                    ..PresetSettings::default()
                },
                preset_type: TYPE_CUSTOM.to_string(),
                created_at,
                last_updated: created_at,
                tags: BTreeMap::new(),
            },
        );
}

/// Put one of the account's own queues in the store.
fn seed_queue(emulator: &Emulator, name: &str, created_at: SystemTime) {
    emulator
        .engine
        .state::<ComAmazonawsMediaconvertMediaconvertState>()
        .lock()
        .queues
        .insert(
            name.to_string(),
            QueueRecord {
                name: name.to_string(),
                description: Some(format!("{name} description")),
                pricing_plan: PRICING_PLAN_ON_DEMAND.to_string(),
                status: QUEUE_STATUS_ACTIVE.to_string(),
                queue_type: QUEUE_TYPE_CUSTOM.to_string(),
                concurrent_jobs: DEFAULT_CONCURRENT_JOBS,
                maximum_concurrent_feeds: DEFAULT_MAXIMUM_CONCURRENT_FEEDS,
                reservation_plan: None,
                created_at,
                last_updated: created_at,
                tags: BTreeMap::new(),
            },
        );
}

/// The names the account's own records are seeded under in every case below: five of each
/// kind, whose alphabetical order and creation order are deliberately different, so a walk
/// can tell the two keys apart and needs several pages at the smaller page sizes.
const SEEDED: [(&str, u64); 5] = [
    ("zeta-shared-paging", 500),
    ("alpha-shared-paging", 400),
    ("mid-shared-paging", 300),
    ("delta-shared-paging", 200),
    ("omega-shared-paging", 100),
];

/// Seed the account's own job templates, presets and queues the cases list.
fn seed_all(emulator: &Emulator) {
    for (name, seconds_ago) in SEEDED {
        seed_job_template(emulator, name, ago(seconds_ago));
        seed_preset(emulator, name, ago(seconds_ago));
        seed_queue(emulator, name, ago(seconds_ago));
    }
}

/// The same set comes back in the same order whatever page size the caller walks it with:
/// every listing, at every page size from one record to the default twenty, against the
/// whole-set page.
#[tokio::test]
async fn every_listing_reports_the_same_ordered_set_at_every_page_size() {
    let emulator = Emulator::new();
    seed_all(&emulator);

    for listing in &LISTINGS {
        // The default page size holds every record of each set seeded here, so this walk is
        // the whole set in one page and the rest of the row is compared against it.
        let whole = listing.walk(&emulator, "", None).await;
        // Each row's set must need SEVERAL pages at the smaller sizes walked below, so a
        // resumption that reported the wrong record cannot pass unnoticed: the four
        // listings here hold the four catalogued Job engine versions at the least.
        assert!(
            whole.len() >= 4,
            "case {}: expected a set of at least four records to page, got {whole:?}",
            listing.label
        );
        for page_size in [1_usize, 2, 3, 4, 5, DEFAULT_PAGE_SIZE] {
            let walked = listing.walk(&emulator, "", Some(page_size)).await;
            assert_eq!(
                walked, whole,
                "case {} maxResults={page_size}: expected the walk to report {whole:?}, got {walked:?}",
                listing.label
            );
        }
    }
}

/// A listing runs UP the key it is ordered on unless the caller asks for `DESCENDING`, in
/// which case it reports exactly the reverse — on either `listBy` key, and across page
/// boundaries, since the resumption anchor is what has to run the same way.
#[tokio::test]
async fn a_descending_walk_is_the_reverse_of_the_ascending_one() {
    let emulator = Emulator::new();
    seed_all(&emulator);

    for listing in &LISTINGS {
        // `ListVersions` models neither `listBy` nor `order`, so the direction is not the
        // caller's to choose there and the listing has no row in this case.
        if !listing.ordered_by_request {
            continue;
        }
        for key in ["listBy=NAME", "listBy=CREATION_DATE"] {
            let ascending = listing.walk(&emulator, key, Some(2)).await;
            let defaulted = listing
                .walk(&emulator, &format!("{key}&order=ASCENDING"), Some(2))
                .await;
            assert_eq!(
                defaulted, ascending,
                "case {} {key:?}: a listing with no order runs up its key, expected {ascending:?}, got {defaulted:?}",
                listing.label
            );
            let descending = listing
                .walk(&emulator, &format!("{key}&order=DESCENDING"), Some(2))
                .await;
            let mut reversed = ascending.clone();
            reversed.reverse();
            assert_eq!(
                descending, reversed,
                "case {} {key:?}: expected the DESCENDING walk to be {reversed:?}, got {descending:?}",
                listing.label
            );
        }
    }
}

/// Ordering on `NAME` is the byte order of the names, whichever listing reports them.
#[tokio::test]
async fn ordering_on_name_is_the_byte_order_of_the_names() {
    let emulator = Emulator::new();
    seed_all(&emulator);

    for listing in &LISTINGS {
        if !listing.ordered_by_request {
            continue;
        }
        let walked = listing.walk(&emulator, "listBy=NAME", Some(2)).await;
        let mut sorted = walked.clone();
        sorted.sort();
        assert_eq!(
            walked, sorted,
            "case {}: expected the NAME walk in byte order {sorted:?}, got {walked:?}",
            listing.label
        );
    }
}

/// A `nextToken` this service never issued is a `BadRequestException` (HTTP 400) from every
/// listing, whatever is wrong with it — and the store is not consulted for one.
#[tokio::test]
async fn every_listing_refuses_a_next_token_it_did_not_issue() {
    let emulator = Emulator::new();
    seed_all(&emulator);

    // A token is the hex of `<epoch nanoseconds>:<id>`, so each row below breaks one part
    // of that: the hex, the colon, and the number before it.
    let tokens = [
        ("not hex at all", "zzzz".to_string()),
        ("odd-length hex", "abc".to_string()),
        ("hex of a string with no colon", ::hex::encode("no-colon")),
        (
            "hex of a non-numeric instant",
            ::hex::encode("not-a-number:alpha-shared-paging"),
        ),
    ];

    for listing in &LISTINGS {
        for (label, token) in &tokens {
            let reply = listing.page(&emulator, "", None, Some(token)).await;
            let case = format!("{} {label}", listing.label);
            assert_eq!(
                reply.status, 400,
                "case {case}: expected HTTP 400, got {} with body {}",
                reply.status, reply.body
            );
            assert!(
                reply
                    .error_type
                    .as_deref()
                    .is_some_and(|value| value.contains("BadRequestException")),
                "case {case}: expected a BadRequestException error type, got {:?}",
                reply.error_type
            );
            assert_eq!(
                reply.body.pointer("/message").and_then(Value::as_str),
                Some("nextToken is not a token this service issued."),
                "case {case}: expected the shared refusal message, got {}",
                reply.body
            );
        }
    }
}

/// The token a page carries resumes immediately after that page's last record: the
/// remainder of the walk is exactly the records the first page did not report, with none
/// repeated and none skipped.
#[tokio::test]
async fn a_page_token_resumes_immediately_after_the_page_that_issued_it() {
    let emulator = Emulator::new();
    seed_all(&emulator);

    for listing in &LISTINGS {
        let whole = listing.walk(&emulator, "", None).await;
        for page_size in [1_usize, 2, 3] {
            let first = listing.page(&emulator, "", Some(page_size), None).await;
            let case = format!("{} maxResults={page_size}", listing.label);
            assert_eq!(
                listing.ids(&first),
                whole[..page_size.min(whole.len())].to_vec(),
                "case {case}: expected the first page to be the head of {whole:?}, got {:?}",
                listing.ids(&first)
            );
            let token = first
                .next_token()
                .unwrap_or_else(|| panic!("case {case}: a set of {} records has a further page after {page_size}, so a nextToken was due", whole.len()));
            let resumed = listing
                .page(&emulator, "", Some(DEFAULT_PAGE_SIZE), Some(&token))
                .await;
            assert_eq!(
                listing.ids(&resumed),
                whole[page_size.min(whole.len())..].to_vec(),
                "case {case}: expected the resumed page to be the tail of {whole:?}, got {:?}",
                listing.ids(&resumed)
            );
        }
    }
}
