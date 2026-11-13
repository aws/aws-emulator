// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of the ONE decode-or-refuse rule every reader of a
//! `nextToken` in this service shares.
//!
//! Every case drives the real restJson1 wire path — the router picks the service from the
//! request's SigV4 scope, the framework deserializes and validates, the handler runs, and
//! the response is serialized — so what is asserted is the bytes a client would see.
//!
//! The rule under test, which every row below is one case of: a request carrying no
//! `nextToken` starts at the beginning of the set, one carrying a token this service
//! issued resumes after it, and one carrying a token this service did not issue is a
//! `BadRequestException` (HTTP 400) stating the single refusal — the same status, the same
//! error shape and the same phrase from whichever operation reads the token, which the one
//! operation that issues no token at all closes by naming the token it refused.
//!
//! The rows span every operation that reads a `nextToken`, the two places the wire carries
//! one (a query-string member and a JSON body member), and the ways a token can fail to be
//! one this service issued, so a change to the shared rule cannot land in one reader and
//! miss another. Nothing here seeds the store: what a page CONTAINS is the business of
//! `tests/all/shared_paging.rs` and the per-operation listing suites.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_mediaconvert_mediaconvert::state::encode_cursor;
use service_com_amazonaws_mediaconvert_mediaconvert::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region every case signs for.
const REGION: &str = "us-west-2";

/// The one phrase this service words the refusal with, pinned as a literal here so a
/// change to it has to be deliberate.
const REFUSAL_PHRASE: &str = "nextToken is not a token this service issued";

/// Where on the wire an operation's `NextToken` member lives.
#[derive(Debug, Clone, Copy)]
enum TokenIn {
    /// Bound to the query string (`httpQuery: nextToken`).
    Query,
    /// A member of the JSON request body (`jsonName: nextToken`).
    Body,
}

/// One operation that reads a `nextToken`, and how to hand it one.
///
/// The bindings are the model's
/// (`smithy_query(query="digest")`): `GET /2017-08-29/jobs`, `GET /2017-08-29/search`,
/// `GET /2017-08-29/queues`, `GET /2017-08-29/presets`, `GET /2017-08-29/jobTemplates`,
/// `GET /2017-08-29/versions`, `POST /2017-08-29/jobsQueries` and
/// `POST /2017-08-29/endpoints`.
#[derive(Debug, Clone, Copy)]
struct Reader {
    /// How a failing assertion names this operation.
    label: &'static str,
    method: &'static str,
    path: &'static str,
    token_in: TokenIn,
    /// The status this operation answers a request it accepts with — 201 for
    /// `StartJobsQuery`, which creates a query, and 200 for the rest.
    success_status: u16,
    /// Whether this operation ever ISSUES a token. `DescribeEndpoints` reports an account's
    /// one endpoint per region, so its first page is its only page and it issues none —
    /// which makes every token handed to it one it did not issue.
    issues_tokens: bool,
}

/// Every operation of this service whose input carries a `nextToken`.
const READERS: [Reader; 8] = [
    Reader {
        label: "ListJobs",
        method: "GET",
        path: "/2017-08-29/jobs",
        token_in: TokenIn::Query,
        success_status: 200,
        issues_tokens: true,
    },
    Reader {
        label: "SearchJobs",
        method: "GET",
        path: "/2017-08-29/search",
        token_in: TokenIn::Query,
        success_status: 200,
        issues_tokens: true,
    },
    Reader {
        label: "ListQueues",
        method: "GET",
        path: "/2017-08-29/queues",
        token_in: TokenIn::Query,
        success_status: 200,
        issues_tokens: true,
    },
    Reader {
        label: "ListPresets",
        method: "GET",
        path: "/2017-08-29/presets",
        token_in: TokenIn::Query,
        success_status: 200,
        issues_tokens: true,
    },
    Reader {
        label: "ListJobTemplates",
        method: "GET",
        path: "/2017-08-29/jobTemplates",
        token_in: TokenIn::Query,
        success_status: 200,
        issues_tokens: true,
    },
    Reader {
        label: "ListVersions",
        method: "GET",
        path: "/2017-08-29/versions",
        token_in: TokenIn::Query,
        success_status: 200,
        issues_tokens: true,
    },
    Reader {
        label: "StartJobsQuery",
        method: "POST",
        path: "/2017-08-29/jobsQueries",
        token_in: TokenIn::Body,
        success_status: 201,
        issues_tokens: true,
    },
    Reader {
        label: "DescribeEndpoints",
        method: "POST",
        path: "/2017-08-29/endpoints",
        token_in: TokenIn::Body,
        success_status: 200,
        issues_tokens: false,
    },
];

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

    async fn send(&self, method: &str, uri: &str, body: Value) -> Reply {
        let payload = match body {
            Value::Null => ::bytes::Bytes::new(),
            value => ::bytes::Bytes::from(value.to_string()),
        };
        let request = ::http::Request::builder()
            .method(method)
            .uri(uri)
            .header("host", format!("mediaconvert.{REGION}.amazonaws.com"))
            .header("authorization", authorization(REGION))
            .header("content-type", "application/json")
            .body(payload)
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

impl Reader {
    /// The whole sentence this operation refuses `token` with: the one phrase, closed by
    /// the token itself for the operation that issues none, since there what is wrong is
    /// that a token arrived at all.
    fn expected_refusal(&self, token: &str) -> String {
        if self.issues_tokens {
            format!("{REFUSAL_PHRASE}.")
        } else {
            format!("{REFUSAL_PHRASE}: {token:?}.")
        }
    }

    /// Call this operation with `token` as its `nextToken`, or with none at all.
    async fn call(&self, emulator: &Emulator, token: Option<&str>) -> Reply {
        match (self.token_in, token) {
            (TokenIn::Query, Some(token)) => {
                emulator
                    .send(
                        self.method,
                        &format!("{}?nextToken={token}", self.path),
                        Value::Null,
                    )
                    .await
            }
            (TokenIn::Query, None) => emulator.send(self.method, self.path, Value::Null).await,
            (TokenIn::Body, Some(token)) => {
                emulator
                    .send(self.method, self.path, json!({ "nextToken": token }))
                    .await
            }
            (TokenIn::Body, None) => emulator.send(self.method, self.path, json!({})).await,
        }
    }
}

/// Tokens this service did not issue: a token is the hex of `<epoch nanoseconds>:<id>`, so
/// each row breaks one part of that — the hex, its length, its case, the colon, the number
/// before it, the id after it — plus the degenerate empty one.
///
/// The last four rows are the ones that name a WELL-FORMED key the service never spelled
/// that way: the instant it mints is a plain unsigned decimal and the hex is lower case, so
/// a sign, a zero pad or an upper-case rendering is no token of its own, and an id carrying
/// no characters names no record, since no door of this service mints one under such a
/// name.
fn tokens_this_service_did_not_issue() -> Vec<(&'static str, String)> {
    let instant = 1_762_000_000_000_000_000_u128;
    vec![
        ("empty", String::new()),
        ("not hex at all", "zzzz".to_string()),
        ("odd-length hex", "abc".to_string()),
        ("hex of a string with no colon", ::hex::encode("no-colon")),
        (
            "hex of a non-numeric instant",
            ::hex::encode("not-a-number:some-record"),
        ),
        (
            "hex of an instant past what a count of nanoseconds can be",
            ::hex::encode(format!("{}0:some-record", u128::MAX)),
        ),
        (
            "hex of a plus-signed instant",
            ::hex::encode(format!("+{instant}:some-record")),
        ),
        (
            "hex of a zero-padded instant",
            ::hex::encode(format!("000000{instant}:some-record")),
        ),
        (
            "hex of a key with no id",
            ::hex::encode(format!("{instant}:")),
        ),
        (
            "upper-case hex of a key",
            ::hex::encode(format!("{instant}:some-record")).to_uppercase(),
        ),
    ]
}

/// A token of the format this service issues: the hex of the ordering key of a record
/// created a moment ago, as [`encode_cursor`] produces for the last record of a page.
fn a_token_this_service_issues() -> String {
    encode_cursor(
        SystemTime::now() - Duration::from_secs(1),
        "shared-next-token-probe",
    )
}

/// A `nextToken` this service did not issue is refused the same way by every operation
/// that reads one: HTTP 400, `BadRequestException`, and the one refusal phrase — whatever
/// is wrong with the token and wherever on the wire it arrived.
#[tokio::test]
async fn every_reader_refuses_a_next_token_this_service_did_not_issue() {
    let emulator = Emulator::new();

    for reader in &READERS {
        for (label, token) in tokens_this_service_did_not_issue() {
            let reply = reader.call(&emulator, Some(&token)).await;
            let case = format!("{} {label}", reader.label);
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
                Some(reader.expected_refusal(&token).as_str()),
                "case {case}: expected the one refusal sentence, got {}",
                reply.body
            );
        }
    }
}

/// The other two outcomes of the same rule: a request carrying no `nextToken` is accepted,
/// and so is one carrying a token of the format this service issues — except by the one
/// operation that issues no token at all, which refuses even a well-formed one with the
/// same phrase.
#[tokio::test]
async fn every_reader_accepts_a_next_token_this_service_issues() {
    let emulator = Emulator::new();
    let issued = a_token_this_service_issues();

    for reader in &READERS {
        let without = reader.call(&emulator, None).await;
        assert_eq!(
            without.status,
            reader.success_status,
            "case {} no nextToken: expected HTTP {}, got {} with body {}",
            reader.label,
            reader.success_status,
            without.status,
            without.body
        );

        let reply = reader.call(&emulator, Some(&issued)).await;
        if reader.issues_tokens {
            assert_eq!(
                reply.status,
                reader.success_status,
                "case {} issued nextToken: expected HTTP {}, got {} with body {}",
                reader.label,
                reader.success_status,
                reply.status,
                reply.body
            );
        } else {
            assert_eq!(
                reply.status, 400,
                "case {} issued nextToken: an operation that issues no token refuses every \
                 one, expected HTTP 400, got {} with body {}",
                reader.label, reply.status, reply.body
            );
            assert_eq!(
                reply.body.pointer("/message").and_then(Value::as_str),
                Some(reader.expected_refusal(&issued).as_str()),
                "case {} issued nextToken: expected the one refusal sentence, got {}",
                reader.label,
                reply.body
            );
        }
    }
}
