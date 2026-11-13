// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of the SPELLING half of the one decode-or-refuse rule every
//! paged reader of this service shares.
//!
//! Every case drives the real restJson1 wire path — the router picks the service from the
//! request's SigV4 scope, the framework deserializes and validates, the handler runs, and
//! the response is serialized — so what is asserted is the bytes a client would see.
//!
//! The rule under test, which every row below is one case of: a `nextToken` names a page
//! only when it is byte for byte the token this service minted for that page, so any OTHER
//! spelling of the very same ordering key — a sign or a zero pad on the instant, upper-case
//! hex, an id carrying no characters — is a token the service never issued and is refused
//! with its `BadRequestException` rather than served as a page whose earlier records are
//! silently hidden.
//!
//! The rows span the listings whose records the account comes up with, so each case can
//! re-spell a key the service ITSELF issued a moment earlier (the first page's `nextToken`),
//! and they span both halves of the key and the hex around it. Nothing here seeds the store:
//! the Job engine version catalogue and the built-in presets and job templates are the sets
//! the account has from the start, and what a page CONTAINS is the business of
//! `tests/all/shared_paging.rs`.

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::Value;
use service_com_amazonaws_mediaconvert_mediaconvert::state::encode_cursor;
use service_com_amazonaws_mediaconvert_mediaconvert::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region every case signs for.
const REGION: &str = "us-west-2";

/// The one sentence this service refuses a `nextToken` it did not issue with.
const REFUSAL: &str = "nextToken is not a token this service issued.";

/// An id of the shape a key's second half carries: a stored record's name, which always
/// carries characters.
const PROBE_ID: &str = "next-token-spelling-probe";

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

    /// The refusal sentence this reply carries, if it carries one.
    fn message(&self) -> Option<&str> {
        self.body.pointer("/message").and_then(Value::as_str)
    }
}

/// One emulator: a single shared engine behind the multi-service router, exactly as the
/// production router assembles it.
struct Emulator {
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
        Self { router }
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

/// One listing a case can take a token this service issued from, without seeding anything:
/// the set it reports is one the account holds from the start.
///
/// The bindings and members come from the model (`smithy_query(query="digest")`):
/// `GET /2017-08-29/versions`, `GET /2017-08-29/presets` and
/// `GET /2017-08-29/jobTemplates`; the `listBy=SYSTEM` rows select the built-in records,
/// which is the set those two listings hold before a caller creates anything.
struct Listing {
    /// How a failing assertion names this listing.
    label: &'static str,
    /// The operation's HTTP URI.
    path: &'static str,
    /// The query the listing is driven with, apart from the page size and the token.
    query: &'static str,
    /// The output member the page's records come back in.
    items: &'static str,
    /// The member of a record that identifies it — the second half of the key the listing
    /// orders and resumes on.
    id_member: &'static str,
}

/// Every listing whose set the account comes up with.
const LISTINGS: [Listing; 3] = [
    Listing {
        label: "ListVersions",
        path: "/2017-08-29/versions",
        query: "",
        items: "versions",
        id_member: "version",
    },
    Listing {
        label: "ListPresets listBy=SYSTEM",
        path: "/2017-08-29/presets",
        query: "listBy=SYSTEM",
        items: "presets",
        id_member: "name",
    },
    Listing {
        label: "ListJobTemplates listBy=SYSTEM",
        path: "/2017-08-29/jobTemplates",
        query: "listBy=SYSTEM",
        items: "jobTemplates",
        id_member: "name",
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

    /// One page of this listing, at `page_size` and resuming from `token`.
    async fn page(
        &self,
        emulator: &Emulator,
        page_size: Option<usize>,
        token: Option<&str>,
    ) -> Reply {
        let mut parts: Vec<String> = Vec::new();
        if !self.query.is_empty() {
            parts.push(self.query.to_string());
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
}

/// The two halves of the ordering key inside a token this service issued.
fn key_inside(token: &str) -> (String, String) {
    let plain = String::from_utf8(::hex::decode(token).expect("an issued token is hex"))
        .expect("an issued token is the hex of text");
    let (nanos, id) = plain
        .split_once(':')
        .expect("an issued token carries <instant>:<id>");
    (nanos.to_string(), id.to_string())
}

/// Every way of spelling ONE ordering key that this service's minting site never writes:
/// the instant it writes is a plain unsigned decimal, the id it writes always carries
/// characters, and the hex it writes is lower case — so each row changes exactly one of
/// those while naming the very same key.
fn other_spellings_of(nanos: &str, id: &str) -> Vec<(String, String)> {
    let canonical = format!("{nanos}:{id}");
    vec![
        (
            "a plus sign before the instant".to_string(),
            ::hex::encode(format!("+{nanos}:{id}")),
        ),
        (
            "one leading zero on the instant".to_string(),
            ::hex::encode(format!("0{nanos}:{id}")),
        ),
        (
            "eight leading zeros on the instant".to_string(),
            ::hex::encode(format!("00000000{nanos}:{id}")),
        ),
        (
            "a space before the instant".to_string(),
            ::hex::encode(format!(" {nanos}:{id}")),
        ),
        (
            "no id at all".to_string(),
            ::hex::encode(format!("{nanos}:")),
        ),
        (
            "upper-case hex of the key".to_string(),
            ::hex::encode(&canonical).to_uppercase(),
        ),
    ]
}

/// A spelling of an ordering key this service never writes names no page, whichever
/// listing reads it: HTTP 400 and the one refusal sentence, and NOT a page — even when the
/// key itself is the one the listing's own first page just issued, which is exactly the
/// case where accepting it would serve a page with that key's record and every record
/// before it silently missing.
#[tokio::test]
async fn another_spelling_of_an_issued_key_is_refused() {
    let emulator = Emulator::new();

    for listing in &LISTINGS {
        let whole = listing.page(&emulator, None, None).await;
        assert_eq!(
            whole.status, 200,
            "case {}: expected HTTP 200 for the whole set, got {} with body {}",
            listing.label, whole.status, whole.body
        );
        let all_ids = listing.ids(&whole);
        assert!(
            all_ids.len() >= 2,
            "case {}: expected a set of at least two records so a fabricated anchor could \
             hide one, got {all_ids:?}",
            listing.label
        );

        let first = listing.page(&emulator, Some(1), None).await;
        let issued = first.next_token().unwrap_or_else(|| {
            panic!(
                "case {}: a set of {} records has a further page after one, so a nextToken \
                 was due, got body {}",
                listing.label,
                all_ids.len(),
                first.body
            )
        });
        let (nanos, id) = key_inside(&issued);

        for (label, token) in other_spellings_of(&nanos, &id) {
            let reply = listing.page(&emulator, None, Some(&token)).await;
            let case = format!("{} {label}", listing.label);
            assert_eq!(
                reply.status, 400,
                "case {case}: expected HTTP 400 for a token this service never issued, got \
                 {} with body {}",
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
                reply.message(),
                Some(REFUSAL),
                "case {case}: expected the one refusal sentence, got {}",
                reply.body
            );
            assert!(
                listing.ids(&reply).is_empty(),
                "case {case}: a refused token reports no page at all, got {:?}",
                listing.ids(&reply)
            );
        }

        // The token the listing DID issue still resumes where it says: the tail of the set
        // after the one record the first page reported.
        let resumed = listing.page(&emulator, None, Some(&issued)).await;
        assert_eq!(
            resumed.status, 200,
            "case {}: expected HTTP 200 for the token it issued, got {} with body {}",
            listing.label, resumed.status, resumed.body
        );
        assert_eq!(
            listing.ids(&resumed),
            all_ids[1..].to_vec(),
            "case {}: expected the issued token to resume at {:?}, got {:?}",
            listing.label,
            &all_ids[1..],
            listing.ids(&resumed)
        );
    }
}

/// The instant half of a key, spelled the one way the minting site writes it and the ways
/// it never does. A plain unsigned decimal is the service's own spelling, at any magnitude
/// up to the top of the `u128` count of nanoseconds a cursor carries; every other rendering
/// of the same number is a token the service never issued.
///
/// `(how a failure names the row, the instant as the token spells it, whether this
/// service issues it)`.
const INSTANT_SPELLINGS: [(&str, &str, bool); 10] = [
    ("the zero instant", "0", true),
    ("one nanosecond past the epoch", "1", true),
    ("a present-day instant", "1762000000000000000", true),
    // The top of the range a `u128` count of nanoseconds can hold, which is the type the
    // cursor's instant half decodes at.
    (
        "the largest instant the count can hold",
        "340282366920938463463374607431768211455",
        true,
    ),
    // One past that top: the number no longer fits, so it was already refused.
    (
        "one digit past the largest instant the count can hold",
        "3402823669209384634633746074317682114550",
        false,
    ),
    ("a plus-signed instant", "+1762000000000000000", false),
    ("a minus-signed instant", "-1762000000000000000", false),
    ("a zero-padded instant", "0001762000000000000000", false),
    ("a zero-padded zero instant", "00", false),
    ("a plus-signed zero instant", "+0", false),
];

/// Only the spelling the minting site writes is read as an instant: the same number spelled
/// another way is refused, and the plain spelling is accepted at every magnitude — checked
/// against the listing that serves the account's version catalogue.
#[tokio::test]
async fn only_the_minted_spelling_of_an_instant_names_a_page() {
    let emulator = Emulator::new();
    let listing = &LISTINGS[0];

    for (label, instant, issued_by_this_service) in INSTANT_SPELLINGS {
        let token = ::hex::encode(format!("{instant}:{PROBE_ID}"));
        let reply = listing.page(&emulator, None, Some(&token)).await;
        let case = format!("{} {label}", listing.label);
        if issued_by_this_service {
            assert_eq!(
                reply.status, 200,
                "case {case}: the plain decimal is the spelling this service writes, so \
                 expected HTTP 200, got {} with body {}",
                reply.status, reply.body
            );
            assert_eq!(
                reply.message(),
                None,
                "case {case}: expected a page rather than a refusal, got {}",
                reply.body
            );
        } else {
            assert_eq!(
                reply.status, 400,
                "case {case}: expected HTTP 400, got {} with body {}",
                reply.status, reply.body
            );
            assert_eq!(
                reply.message(),
                Some(REFUSAL),
                "case {case}: expected the one refusal sentence, got {}",
                reply.body
            );
        }
    }
}

/// The key a cursor carries round-trips: the token the minting site writes for an instant
/// and an id decodes back to that page position and is accepted, whatever the magnitude of
/// the instant — the counterpart of the refusals above, so the tightening cannot have
/// closed the door on the service's own tokens.
#[tokio::test]
async fn a_token_this_service_minted_round_trips_for_every_instant() {
    let emulator = Emulator::new();
    let listing = &LISTINGS[0];

    // One row per kind of instant `encode_cursor` is handed: the epoch itself, a moment
    // ago, now, and an instant far in the future — each the whole key, id included.
    let minted = [
        ("the epoch", UNIX_EPOCH),
        (
            "a day ago",
            SystemTime::now() - Duration::from_secs(60 * 60 * 24),
        ),
        ("now", SystemTime::now()),
        (
            "a year from now",
            SystemTime::now() + Duration::from_secs(60 * 60 * 24 * 365),
        ),
    ];

    for (label, at) in minted {
        let token = encode_cursor(at, PROBE_ID);
        let reply = listing.page(&emulator, None, Some(&token)).await;
        let case = format!("{} {label}", listing.label);
        assert_eq!(
            reply.status, 200,
            "case {case}: a token this service minted names a page, so expected HTTP 200, \
             got {} with body {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.message(),
            None,
            "case {case}: expected a page rather than a refusal, got {}",
            reply.body
        );
    }
}
