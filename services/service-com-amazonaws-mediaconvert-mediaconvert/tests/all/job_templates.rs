// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of `ListJobTemplates`.
//!
//! Every case drives the real restJson1 wire path — the router picks the service from the
//! request's SigV4 scope, the framework deserializes and validates, the handler runs, and
//! the response is serialized — so what is asserted is the bytes a client would see.
//!
//! The rule under test, which every case below is one row of: `ListJobTemplates` reports
//! the stored job templates of the kind `listBy` selects (`SYSTEM` the built-in ones,
//! anything else the account's own), narrowed to `category` when the caller names one,
//! ordered on the key `listBy` names in the direction `order` names, in pages of at most
//! `maxResults` with an opaque `nextToken` that resumes immediately after the last item of
//! the page just returned.
//!
//! The account's own templates are seeded straight into the store by [`seed_custom`] in
//! the cases that are about listing alone; the write side's own coverage lives in
//! `tests/all/job_template_crud.rs`. Nothing else here reaches past HTTP.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use service_com_amazonaws_mediaconvert_mediaconvert::ops::types::JobTemplateSettings;
use service_com_amazonaws_mediaconvert_mediaconvert::state::{
    default_acceleration_settings, JobTemplateRecord, DEFAULT_PRIORITY,
    DEFAULT_STATUS_UPDATE_INTERVAL, TYPE_CUSTOM, TYPE_SYSTEM,
};
use service_com_amazonaws_mediaconvert_mediaconvert::{
    ComAmazonawsMediaconvertMediaconvertState, EmulatedService,
};
use smithy_server_runtime::MultiServiceRouter;

/// The region every case signs for unless it names another.
const REGION: &str = "us-west-2";

/// A SigV4 `Authorization` header in this service's scope, signed for `region`. It is
/// what the router reads to pick the service, and what the handler reads the ARN region
/// from.
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
    /// The `name` of every template in `jobTemplates`, in the order they were returned.
    fn names(&self) -> Vec<String> {
        self.body
            .pointer("/jobTemplates")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .map(|item| {
                        item.pointer("/name")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string()
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

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
        let mut router = MultiServiceRouter::new();
        // The signature itself is the transport's business, not this suite's; the
        // Authorization header is still present because routing and the ARN region read
        // the credential scope out of it.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        Self { engine, router }
    }

    /// The names of the stored templates of one `Type`, alphabetically — read from the
    /// store so no case hard-codes the built-in catalogue.
    fn stored_names_of_type(&self, template_type: &str) -> Vec<String> {
        self.engine
            .state::<ComAmazonawsMediaconvertMediaconvertState>()
            .lock()
            .job_templates
            .values()
            .filter(|record| record.template_type == template_type)
            .map(|record| record.name.clone())
            .collect()
    }

    /// `(name, created_at epoch seconds, last_updated epoch seconds)` for every stored
    /// template, so a case can show a read left the store alone.
    fn store_snapshot(&self) -> Vec<(String, u64, u64)> {
        self.engine
            .state::<ComAmazonawsMediaconvertMediaconvertState>()
            .lock()
            .job_templates
            .values()
            .map(|record| {
                (
                    record.name.clone(),
                    epoch_secs(record.created_at),
                    epoch_secs(record.last_updated),
                )
            })
            .collect()
    }

    async fn list(&self, query: &str) -> Reply {
        self.list_in(REGION, query).await
    }

    async fn list_in(&self, region: &str, query: &str) -> Reply {
        let uri = if query.is_empty() {
            "/2017-08-29/jobTemplates".to_string()
        } else {
            format!("/2017-08-29/jobTemplates?{query}")
        };
        let request = ::http::Request::builder()
            .method("GET")
            .uri(uri)
            .header("host", format!("mediaconvert.{region}.amazonaws.com"))
            .header("authorization", authorization(region))
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

/// Epoch seconds of an instant, the unit `CreatedAt`/`LastUpdated` are modelled in
/// (`__timestampUnix`, `epoch-seconds`).
fn epoch_secs(at: SystemTime) -> u64 {
    at.duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs())
}

/// Put one of the account's own ([`TYPE_CUSTOM`]) templates in the store.
///
/// The cases in this suite are about the LISTING, so they seed the records whose listing
/// they assert; `CreateJobTemplate`'s own coverage lives in
/// `tests/all/job_template_crud.rs`.
fn seed_custom(emulator: &Emulator, name: &str, category: Option<&str>, created_at: SystemTime) {
    emulator
        .engine
        .state::<ComAmazonawsMediaconvertMediaconvertState>()
        .lock()
        .job_templates
        .insert(
            name.to_string(),
            JobTemplateRecord {
                name: name.to_string(),
                category: category.map(str::to_string),
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

/// An instant `seconds_ago` before now, for seeding a creation order that is deliberately
/// not the alphabetical one.
fn ago(seconds_ago: u64) -> SystemTime {
    SystemTime::now() - Duration::from_secs(seconds_ago)
}

/// The three of the account's own templates every ordering case lists: their alphabetical
/// order and their creation order are different, so a case can tell the two keys apart.
fn seed_three(emulator: &Emulator) {
    seed_custom(emulator, "zeta-weekly", Some("promos"), ago(300));
    seed_custom(emulator, "alpha-nightly", Some("promos"), ago(200));
    seed_custom(emulator, "mid-quarterly", Some("ott-hls"), ago(100));
}

/// `listBy` picks both the set and the key, and `order` picks the direction: the full
/// matrix of the three `JobTemplateListBy` values against the two `Order` values and the
/// absent default of each.
#[tokio::test]
async fn list_by_and_order_span_the_orderings_the_model_declares() {
    let emulator = Emulator::new();
    seed_three(&emulator);

    let by_name = vec![
        "alpha-nightly".to_string(),
        "mid-quarterly".to_string(),
        "zeta-weekly".to_string(),
    ];
    let by_creation = vec![
        "zeta-weekly".to_string(),
        "alpha-nightly".to_string(),
        "mid-quarterly".to_string(),
    ];
    let mut name_descending = by_name.clone();
    name_descending.reverse();
    let mut creation_descending = by_creation.clone();
    creation_descending.reverse();
    // Read out of the store, not written down here: the built-in set is the emulator's
    // data, and a case asserts only that a SYSTEM listing is exactly it, in order.
    let mut system_ascending = emulator.stored_names_of_type(TYPE_SYSTEM);
    system_ascending.sort();
    let mut system_descending = system_ascending.clone();
    system_descending.reverse();

    let cases: Vec<(&str, &Vec<String>)> = vec![
        // No `listBy`: "If you don't specify, the service will list them by name", and
        // no `order` runs up that key.
        ("", &by_name),
        ("listBy=NAME", &by_name),
        ("listBy=NAME&order=ASCENDING", &by_name),
        ("listBy=NAME&order=DESCENDING", &name_descending),
        ("listBy=CREATION_DATE", &by_creation),
        ("listBy=CREATION_DATE&order=ASCENDING", &by_creation),
        ("listBy=CREATION_DATE&order=DESCENDING", &creation_descending),
        // `SYSTEM` selects the built-in templates — the ones `JobTemplate$Type` says a
        // user "can't modify or delete" — and none of the account's own.
        ("listBy=SYSTEM", &system_ascending),
        ("listBy=SYSTEM&order=ASCENDING", &system_ascending),
        ("listBy=SYSTEM&order=DESCENDING", &system_descending),
    ];

    for (query, expected) in cases {
        let reply = emulator.list(query).await;
        assert_eq!(
            reply.status, 200,
            "case {query:?}: expected HTTP 200, got {} with body {}",
            reply.status, reply.body
        );
        assert_eq!(
            &reply.names(),
            expected,
            "case {query:?}: expected {expected:?}, got {:?}",
            reply.names()
        );
        assert_eq!(
            reply.next_token(),
            None,
            "case {query:?}: one page holds every template, so no nextToken was due, got {:?}",
            reply.next_token()
        );
    }
}

/// Every template a listing reports carries the type of the set it came from, so the two
/// sets are disjoint however they are listed.
#[tokio::test]
async fn a_listing_reports_only_the_type_its_list_by_selects() {
    let emulator = Emulator::new();
    seed_three(&emulator);

    let cases = [
        ("", TYPE_CUSTOM),
        ("listBy=NAME", TYPE_CUSTOM),
        ("listBy=CREATION_DATE", TYPE_CUSTOM),
        ("listBy=SYSTEM", TYPE_SYSTEM),
    ];

    for (query, expected_type) in cases {
        let reply = emulator.list(query).await;
        let items = reply
            .body
            .pointer("/jobTemplates")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        assert!(
            !items.is_empty(),
            "case {query:?}: expected a non-empty listing, got body {}",
            reply.body
        );
        for item in &items {
            assert_eq!(
                item.pointer("/type").and_then(Value::as_str),
                Some(expected_type),
                "case {query:?}: expected type {expected_type:?}, got {item}"
            );
        }
    }
}

/// `category` limits the listing to the templates stored in that category: one the caller
/// named, one no template is in, and the absent case that limits nothing.
#[tokio::test]
async fn category_limits_the_listing_to_that_category() {
    let emulator = Emulator::new();
    seed_three(&emulator);
    // A template with no category at all: it belongs to no category a caller can name.
    seed_custom(&emulator, "uncategorised", None, ago(50));

    let cases: Vec<(&str, Vec<&str>)> = vec![
        (
            "category=promos",
            vec!["alpha-nightly", "zeta-weekly"],
        ),
        ("category=ott-hls", vec!["mid-quarterly"]),
        ("category=promos&order=DESCENDING", vec!["zeta-weekly", "alpha-nightly"]),
        // Category matching is exact: neither a different case nor a prefix matches.
        ("category=Promos", vec![]),
        ("category=promo", vec![]),
        ("category=no-such-category", vec![]),
        (
            "",
            vec![
                "alpha-nightly",
                "mid-quarterly",
                "uncategorised",
                "zeta-weekly",
            ],
        ),
    ];

    for (query, expected) in cases {
        let reply = emulator.list(query).await;
        assert_eq!(
            reply.status, 200,
            "case {query:?}: expected HTTP 200, got {} with body {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.names(),
            expected,
            "case {query:?}: expected {expected:?}, got {:?}",
            reply.names()
        );
    }
}

/// Paging walks the ordering exactly once: concatenating the pages reproduces the whole
/// listing, no page is longer than asked for, and the last page carries no token.
///
/// The table spans the page sizes across the `maxResults` range (its published bounds, 1
/// and 20, and sizes in between) and both directions, over more templates than one page
/// holds.
#[tokio::test]
async fn paging_walks_every_template_exactly_once() {
    let emulator = Emulator::new();
    // 25: more than the 20 a single page holds, so the default page size is exercised too.
    let total = 25_usize;
    for index in 0..total {
        // Zero-padded so alphabetical order and creation order agree, and the expectation
        // below is just the sorted list.
        seed_custom(
            &emulator,
            &format!("tmpl-{index:02}"),
            Some("paging"),
            ago(1_000 - index as u64),
        );
    }
    let ascending: Vec<String> = (0..total).map(|index| format!("tmpl-{index:02}")).collect();
    let mut descending = ascending.clone();
    descending.reverse();

    for list_by in ["NAME", "CREATION_DATE"] {
        for order in ["ASCENDING", "DESCENDING"] {
            let expected = if order == "ASCENDING" {
                &ascending
            } else {
                &descending
            };
            for page_size in [1_usize, 2, 7, 20] {
                let case = format!("listBy={list_by}&order={order}&maxResults={page_size}");
                let mut seen: Vec<String> = Vec::new();
                let mut token: Option<String> = None;
                let mut pages = 0_usize;
                loop {
                    let mut query = case.clone();
                    if let Some(token) = &token {
                        query.push_str(&format!("&nextToken={token}"));
                    }
                    let reply = emulator.list(&query).await;
                    assert_eq!(
                        reply.status, 200,
                        "case {case}: page {pages} expected HTTP 200, got {} with body {}",
                        reply.status, reply.body
                    );
                    let names = reply.names();
                    assert!(
                        names.len() <= page_size,
                        "case {case}: page {pages} expected at most {page_size} items, got {:?}",
                        names
                    );
                    seen.extend(names);
                    pages += 1;
                    token = reply.next_token();
                    if token.is_none() {
                        break;
                    }
                    assert!(
                        pages <= total + 1,
                        "case {case}: paging did not terminate, saw {:?}",
                        seen
                    );
                }
                assert_eq!(
                    &seen, expected,
                    "case {case}: expected the whole listing in order, got {seen:?}"
                );
                let expected_pages = total.div_ceil(page_size);
                assert_eq!(
                    pages, expected_pages,
                    "case {case}: expected {expected_pages} pages, got {pages}"
                );
            }
        }
    }

    // No `maxResults`: "up to twenty of your job templates", and a token for the rest.
    let first = emulator.list("category=paging").await;
    assert_eq!(
        first.names().len(),
        20,
        "case default page size: expected 20 items, got {:?}",
        first.names()
    );
    let token = first
        .next_token()
        .unwrap_or_else(|| panic!("case default page size: a 25-template listing owes a token"));
    let second = emulator
        .list(&format!("category=paging&nextToken={token}"))
        .await;
    assert_eq!(
        second.names(),
        ascending[20..],
        "case default page size: expected the remaining 5 templates, got {:?}",
        second.names()
    );
    assert_eq!(
        second.next_token(),
        None,
        "case default page size: the second page ends the listing, got {:?}",
        second.next_token()
    );
}

/// `maxResults` carries the model's own range, 1..20: each published bound is accepted and
/// one step past either is refused before the handler runs.
#[tokio::test]
async fn max_results_is_held_to_the_range_the_model_declares() {
    let emulator = Emulator::new();
    seed_three(&emulator);

    for accepted in ["maxResults=1", "maxResults=20"] {
        let reply = emulator.list(accepted).await;
        assert_eq!(
            reply.status, 200,
            "case {accepted:?}: inside the range, expected HTTP 200, got {} with body {}",
            reply.status, reply.body
        );
    }
    for refused in ["maxResults=0", "maxResults=21"] {
        let reply = emulator.list(refused).await;
        assert_eq!(
            reply.status, 400,
            "case {refused:?}: outside the range, expected HTTP 400, got {} with body {}",
            reply.status, reply.body
        );
    }
}

/// A `nextToken` this service never issued is a bad request, not a panic and not a silent
/// first page. The table spans the ways a token can be wrong.
#[tokio::test]
async fn a_next_token_this_service_did_not_issue_is_a_bad_request() {
    let emulator = Emulator::new();
    seed_three(&emulator);

    let cases = [
        // Not hex at all.
        "not-a-token",
        // Hex of a string with no `:` separator.
        &::hex::encode("alpha-nightly"),
        // Hex of a separated pair whose first half is not an instant.
        &::hex::encode("soon:alpha-nightly"),
        // Hex of an empty cursor.
        &::hex::encode(""),
    ];

    for token in cases {
        let reply = emulator.list(&format!("nextToken={token}")).await;
        assert_eq!(
            reply.status, 400,
            "case {token:?}: expected HTTP 400, got {} with body {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_type.as_deref(),
            Some("BadRequestException"),
            "case {token:?}: expected BadRequestException, got {:?}",
            reply.error_type
        );
    }
}

/// Every member of a listed template is projected from the stored record, and its ARN
/// names the region of the request that read it.
#[tokio::test]
async fn each_listed_template_is_projected_from_the_stored_record() {
    let emulator = Emulator::new();
    let created_at = ago(120);
    seed_custom(&emulator, "alpha-nightly", Some("promos"), created_at);

    for region in [REGION, "eu-central-1", "ap-southeast-2"] {
        let reply = emulator.list_in(region, "listBy=NAME").await;
        let item = reply
            .body
            .pointer("/jobTemplates/0")
            .cloned()
            .unwrap_or(Value::Null);
        let expected = json!({
            "name": "alpha-nightly",
            "category": "promos",
            "description": "alpha-nightly description",
            // Built the way the emulator's own ARN helper builds it, so the expectation
            // is not a second hand-written format.
            "arn": engine::emulator_arn("mediaconvert", region, "jobTemplates/alpha-nightly"),
            "createdAt": epoch_secs(created_at),
            "lastUpdated": epoch_secs(created_at),
            "type": TYPE_CUSTOM,
            "hopDestinations": [],
            "settings": {},
            // The members the service applies to every template, which the seeded record
            // carries exactly as a created one does.
            "priority": DEFAULT_PRIORITY,
            "statusUpdateInterval": DEFAULT_STATUS_UPDATE_INTERVAL,
            "accelerationSettings": { "mode": default_acceleration_settings().mode },
        });
        assert_eq!(
            item, expected,
            "case region {region:?}: expected {expected}, got {item}"
        );
    }

    // A built-in template is projected the same way, under its own type.
    let system = emulator.list("listBy=SYSTEM").await;
    let first = system
        .body
        .pointer("/jobTemplates/0")
        .cloned()
        .unwrap_or(Value::Null);
    let name = first
        .pointer("/name")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    assert!(
        emulator.stored_names_of_type(TYPE_SYSTEM).contains(&name),
        "case built-in template: {name:?} is not one of the stored built-ins {:?}",
        emulator.stored_names_of_type(TYPE_SYSTEM)
    );
    assert_eq!(
        first.pointer("/arn").and_then(Value::as_str),
        Some(engine::emulator_arn("mediaconvert", REGION, &format!("jobTemplates/{name}")).as_str()),
        "case built-in template: expected the stored name's ARN, got {first}"
    );
}

/// Listing is a read: it changes nothing a later caller sees, however often and with
/// whatever filters it is called.
#[tokio::test]
async fn listing_changes_nothing_a_later_caller_sees() {
    let emulator = Emulator::new();
    seed_three(&emulator);
    let before = emulator.store_snapshot();

    let queries = [
        "",
        "listBy=NAME",
        "listBy=CREATION_DATE&order=DESCENDING",
        "listBy=SYSTEM",
        "category=promos&maxResults=1",
    ];
    let mut first_bodies = Vec::new();
    for query in queries {
        first_bodies.push(emulator.list(query).await.body);
    }
    for (query, first) in queries.iter().zip(first_bodies) {
        let again = emulator.list(query).await.body;
        assert_eq!(
            again, first,
            "case {query:?}: a repeated listing must answer the same, got {again} then {first}"
        );
    }

    let after = emulator.store_snapshot();
    assert_eq!(
        after, before,
        "the store must be untouched by reads: expected {before:?}, got {after:?}"
    );
}
