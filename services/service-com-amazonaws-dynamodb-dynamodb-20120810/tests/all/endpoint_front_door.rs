// SPDX-License-Identifier: CC0-1.0

//! The endpoint's own HTTP surface: what this service answers a request it names no
//! operation for, and what every response reports about itself.
//!
//! RULES, each asserted over a SET of inputs rather than over one recorded request:
//!
//! 1. MEDIA TYPE. The endpoint serves its API under the JSON media types it publishes alone
//!    (`application/json` and its own `application/x-amz-json-1.0`, with any
//!    `;`-parameters ignored). A request announcing any other media type — or none at all —
//!    is not routed to an operation: it is a route the endpoint binds nothing for, answered
//!    HTTP 404 whose whole body is the bare `<UnknownOperationException/>` element — not the
//!    400 an operation answers a request it received and found invalid.
//! 2. ANNOUNCED MEDIA TYPE. A response announces the media type the REQUEST named the API
//!    under, so a client that asked in `application/json` is answered in
//!    `application/json`, whichever layer answered it.
//! 3. THE ROOT'S METHOD SURFACE. The API is served on POST of the root. The root read with
//!    GET answers a plain-text health report naming the regional endpoint the request
//!    addressed — `healthy: <endpoint> ` — and the root reached with any other method, or
//!    any other path with any method, is a route the endpoint binds nothing for (404, as
//!    rule 1). A request target in the absolute form a client given an endpoint sends is
//!    read by its path, exactly as an origin-form target is.
//! 4. AN UNKNOWN TARGET. An `x-amz-target` naming no operation this endpoint binds —
//!    including one that does not parse as `Service.Operation`, and a request carrying none
//!    at all — is refused HTTP 400 with `__type`
//!    `com.amazon.coral.service#UnknownOperationException` and NO `message` member. There
//!    is no notion of an operation that is modelled but unserved on this surface, so no 5xx
//!    and no emulator prose.
//! 5. THE BODY CHECKSUM. Every response that IS a document of this API — a success, a
//!    modelled error, a refusal ahead of the handler, the front door's own refusal —
//!    reports `x-amz-crc32`, the CRC-32 of its own body as an unsigned decimal integer,
//!    which is what an SDK validates each DynamoDB response against.
//!
//! The rows span what the rules generalise over: the media type announced (served,
//! unserved, parameterised, absent), the method, path and target FORM the endpoint is
//! reached with, and which layer answered.

use bytes::Bytes;
use serde_json::Value;
use smithy_server_runtime::MultiServiceRouter;

use crate::emulator::{emulator, REGION};

/// The media type the `awsJson1_0` protocol this service speaks stamps on its responses.
const PROTOCOL_MEDIA_TYPE: &str = "application/x-amz-json-1.0";

/// The plain JSON media type the same endpoint also serves its API under.
const JSON_MEDIA_TYPE: &str = "application/json";

/// The whole body a route the endpoint binds nothing for is answered with: the bare element
/// naming the fault, with the one newline that closes it. Written out here rather than read
/// from the framework, so the assertion does not merely compare the implementation with
/// itself.
const UNROUTED_BODY: &str = "<UnknownOperationException/>\n";

/// The class the front door refuses a target it cannot bind with, in the namespace the
/// front door itself declares.
const UNKNOWN_OPERATION_TYPE: &str = "com.amazon.coral.service#UnknownOperationException";

/// The host a probe addresses, which is the endpoint the health report is about.
const HOST: &str = "dynamodb.us-east-1.amazonaws.com";

/// The whole `X-Amz-Target` of a call this service serves.
const LIST_TABLES: &str = "DynamoDB_20120810.ListTables";

/// One request a row sends: everything the front door reads, and nothing else.
#[derive(Clone, Copy)]
struct Probe<'a> {
    method: &'a str,
    uri: &'a str,
    host: &'a str,
    region: &'a str,
    content_type: Option<&'a str>,
    target: Option<&'a str>,
    body: &'a str,
}

/// The call every row varies from: the POST an SDK sends for a `ListTables`.
const API_CALL: Probe<'static> = Probe {
    method: "POST",
    uri: "/",
    host: HOST,
    region: REGION,
    content_type: Some(PROTOCOL_MEDIA_TYPE),
    target: Some(LIST_TABLES),
    body: "{}",
};

/// One answer, as a client reads it.
struct Answer {
    status: u16,
    content_type: Option<String>,
    checksum: Option<String>,
    body: String,
}

impl Answer {
    /// The answer's body as a JSON document, or `Null` when it is not one.
    fn document(&self) -> Value {
        serde_json::from_str(&self.body).unwrap_or(Value::Null)
    }
}

/// Send one probe and read back the whole answer.
async fn send(router: &MultiServiceRouter, probe: Probe<'_>) -> Answer {
    let mut request = http::Request::builder()
        .method(probe.method)
        .uri(probe.uri)
        .header("host", probe.host)
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTEST/20260101/{}/dynamodb/aws4_request, \
                 SignedHeaders=host, Signature=0000",
                probe.region
            ),
        );
    if let Some(media_type) = probe.content_type {
        request = request.header("content-type", media_type);
    }
    if let Some(target) = probe.target {
        request = request.header("x-amz-target", target);
    }
    let request = request
        .body(Bytes::from(probe.body.to_string()))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let read = |name: &str| {
        response
            .headers()
            .get(name)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string)
    };
    let status = response.status().as_u16();
    let content_type = read("content-type");
    let checksum = read("x-amz-crc32");
    Answer {
        status,
        content_type,
        checksum,
        body: String::from_utf8_lossy(&response.into_body()).to_string(),
    }
}

/// The CRC-32 of `body` as the unsigned decimal integer the header carries.
fn crc32_of(body: &str) -> String {
    let mut digest = crc32fast::Hasher::new();
    digest.update(body.as_bytes());
    digest.finalize().to_string()
}

/// Assert `answer` is the 404 a route the endpoint binds nothing for is answered with.
fn assert_unroutable(case: &str, answer: &Answer) {
    assert_eq!(
        answer.status, 404,
        "case {case:?}: expected HTTP 404 for a route the endpoint binds nothing for, got \
         {} with {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.body, UNROUTED_BODY,
        "case {case:?}: expected the bare fault element as the whole body, got {:?}",
        answer.body
    );
    assert_eq!(
        answer.content_type, None,
        "case {case:?}: a missing route announces no media type, got {:?}",
        answer.content_type
    );
}

/// RULES 1, 2 and 5 over the media types a client can announce: which ones the API is
/// served under, what each answer announces, and the checksum every served answer reports.
#[tokio::test]
async fn the_endpoint_serves_its_api_under_the_json_media_types_and_answers_in_the_one_asked() {
    let router = emulator();

    // (case, the media type announced, the media type the answer announces — `None` for a
    //  request the endpoint routes nothing for)
    let rows: [(&str, Option<&str>, Option<&str>); 12] = [
        (
            "the protocol's own media type",
            Some(PROTOCOL_MEDIA_TYPE),
            Some(PROTOCOL_MEDIA_TYPE),
        ),
        (
            "the plain JSON media type",
            Some(JSON_MEDIA_TYPE),
            Some(JSON_MEDIA_TYPE),
        ),
        (
            "a parameterised JSON media type",
            Some("application/json; charset=utf-8"),
            Some(JSON_MEDIA_TYPE),
        ),
        (
            "the same, upper-cased, as a media type is compared case-insensitively",
            Some("Application/JSON"),
            Some(JSON_MEDIA_TYPE),
        ),
        (
            "another version of the awsJson family, a protocol this endpoint does not speak",
            Some("application/x-amz-json-1.1"),
            None,
        ),
        ("no media type announced at all", None, None),
        ("an empty media type", Some(""), None),
        ("plain text", Some("text/plain"), None),
        ("XML", Some("application/xml"), None),
        ("an octet stream", Some("application/octet-stream"), None),
        ("HTML, with a parameter", Some("text/html; charset=utf-8"), None),
        ("a wildcard, which names no media type", Some("*/*"), None),
    ];

    for (case, announced, expected) in rows {
        let answer = send(
            &router,
            Probe {
                content_type: announced,
                ..API_CALL
            },
        )
        .await;
        let Some(expected) = expected else {
            assert_unroutable(case, &answer);
            assert_eq!(
                answer.checksum, None,
                "case {case:?}: a missing route is no document of this API and reports no \
                 checksum, got {:?}",
                answer.checksum
            );
            continue;
        };
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected the call served with HTTP 200, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.content_type.as_deref(),
            Some(expected),
            "case {case:?}: expected the answer announced as {expected:?}, got {:?}",
            answer.content_type
        );
        assert_eq!(
            answer.document()["TableNames"],
            serde_json::json!([]),
            "case {case:?}: expected the modelled ListTables result, got {:?}",
            answer.body
        );
        assert_eq!(
            answer.checksum.as_deref(),
            Some(crc32_of(&answer.body).as_str()),
            "case {case:?}: expected the CRC-32 {} of the body it answered, got {:?}",
            crc32_of(&answer.body),
            answer.checksum
        );
    }
}

/// RULE 3: the root's method surface — one method at the root reports the endpoint's
/// health, the API is served on one other, and every remaining method or path there is a
/// route the endpoint binds nothing for. The rows cover both forms of a request target,
/// because a client given an endpoint sends the absolute form.
#[tokio::test]
async fn the_endpoint_root_reports_its_health_on_one_method_and_routes_nothing_on_the_rest() {
    let router = emulator();

    /// What the endpoint answers one row with: its health report, the operation the target
    /// names, or the 404 of a route it binds nothing for.
    #[derive(Clone, Copy, PartialEq, Eq, Debug)]
    enum Answers {
        Health,
        TheApi,
        Nothing,
    }

    // (case, method, request target, what the endpoint answers)
    let rows: [(&str, &str, &str, Answers); 13] = [
        ("the API's own method", "POST", "/", Answers::TheApi),
        (
            "the API in the absolute form a client given an endpoint sends",
            "POST",
            "http://dynamodb.us-east-1.amazonaws.com/",
            Answers::TheApi,
        ),
        ("the root, read for its health", "GET", "/", Answers::Health),
        (
            "the root, read for its health in the absolute form",
            "GET",
            "http://dynamodb.us-east-1.amazonaws.com/",
            Answers::Health,
        ),
        (
            "the root, read with a query string",
            "GET",
            "/?probe=1",
            Answers::Health,
        ),
        ("a head of the root", "HEAD", "/", Answers::Nothing),
        ("a write to the root", "PUT", "/", Answers::Nothing),
        ("a delete of the root", "DELETE", "/", Answers::Nothing),
        ("a patch of the root", "PATCH", "/", Answers::Nothing),
        ("an options probe of the root", "OPTIONS", "/", Answers::Nothing),
        (
            "a path the endpoint binds nothing for",
            "GET",
            "/ping",
            Answers::Nothing,
        ),
        (
            "the API's own method at a path the endpoint binds nothing for",
            "POST",
            "/ping",
            Answers::Nothing,
        ),
        (
            "the same, in the absolute form",
            "POST",
            "http://dynamodb.us-east-1.amazonaws.com/ping",
            Answers::Nothing,
        ),
    ];

    for (case, method, uri, expected) in rows {
        let answer = send(
            &router,
            Probe {
                method,
                uri,
                ..API_CALL
            },
        )
        .await;
        match expected {
            Answers::Health => {
                assert_eq!(
                    answer.status, 200,
                    "case {case:?}: expected the health report with HTTP 200, got {} with {:?}",
                    answer.status, answer.body
                );
                assert_eq!(
                    answer.content_type.as_deref(),
                    Some("text/plain"),
                    "case {case:?}: expected the health report announced as plain text, got {:?}",
                    answer.content_type
                );
                assert_eq!(
                    answer.body,
                    format!("healthy: dynamodb.{REGION}.amazonaws.com "),
                    "case {case:?}: expected the health report of the endpoint addressed, got \
                     {:?}",
                    answer.body
                );
            }
            Answers::TheApi => assert_eq!(
                answer.status, 200,
                "case {case:?}: expected the call served with HTTP 200, got {} with {:?}",
                answer.status, answer.body
            ),
            Answers::Nothing => assert_unroutable(case, &answer),
        }
    }

    // The endpoint a report names is the one the request addressed, not a constant: a probe
    // of each regional endpoint reads back the endpoint it is talking to. The region is
    // resolved the way every other request of this service resolves it, so a probe that
    // addresses the endpoint through a loopback address still names the regional endpoint
    // its credential scope addresses.
    // (case, the region the request addresses, the host it reached the emulator at)
    let rows: [(&str, &str, &str); 4] = [
        (
            "a probe of the region's own endpoint",
            "us-east-1",
            "dynamodb.us-east-1.amazonaws.com",
        ),
        (
            "a probe of another region's endpoint",
            "eu-west-1",
            "dynamodb.eu-west-1.amazonaws.com",
        ),
        (
            "a probe of a third region's endpoint",
            "ap-southeast-2",
            "dynamodb.ap-southeast-2.amazonaws.com",
        ),
        (
            "a probe that reached this emulator at a loopback address",
            "eu-central-1",
            "127.0.0.1:9297",
        ),
    ];
    for (case, region, host) in rows {
        let answer = send(
            &router,
            Probe {
                method: "GET",
                host,
                region,
                ..API_CALL
            },
        )
        .await;
        assert_eq!(
            answer.body,
            format!("healthy: dynamodb.{region}.amazonaws.com "),
            "case {case:?}: expected the endpoint addressed named in the report, got {:?}",
            answer.body
        );
    }
}

/// RULES 4 and 5: a target naming no operation this endpoint binds is refused in the front
/// door's own vocabulary, with no sentence and no server-fault status.
#[tokio::test]
async fn a_target_naming_no_bound_operation_is_refused_as_an_unknown_operation() {
    let router = emulator();

    // (case, the whole X-Amz-Target of the request — `None` for a request carrying none)
    let rows: [(&str, Option<&str>); 6] = [
        ("a target that does not parse as Service.Operation", Some("garbage-target")),
        (
            "a well-formed target naming an operation that does not exist",
            Some("DynamoDB_20120810.NoSuchOperation"),
        ),
        (
            "a well-formed target naming an operation of another API",
            Some("DynamoDB_20120810.CreateFunction"),
        ),
        ("a target naming no operation at all", Some("DynamoDB_20120810.")),
        ("an empty target", Some("")),
        ("no target at all", None),
    ];

    for (case, target) in rows {
        let answer = send(&router, Probe { target, ..API_CALL }).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected HTTP 400 for a target this endpoint binds nothing for, \
             got {} with {:?}",
            answer.status, answer.body
        );
        let document = answer.document();
        assert_eq!(
            document["__type"],
            serde_json::json!(UNKNOWN_OPERATION_TYPE),
            "case {case:?}: expected the front door's own class, got {:?}",
            answer.body
        );
        assert!(
            document.get("message").is_none() && document.get("Message").is_none(),
            "case {case:?}: this class states no sentence, got {:?}",
            answer.body
        );
        assert_eq!(
            answer.content_type.as_deref(),
            Some(PROTOCOL_MEDIA_TYPE),
            "case {case:?}: expected the refusal announced in the media type the request \
             named, got {:?}",
            answer.content_type
        );
        assert_eq!(
            answer.checksum.as_deref(),
            Some(crc32_of(&answer.body).as_str()),
            "case {case:?}: expected the CRC-32 {} of the body it answered, got {:?}",
            crc32_of(&answer.body),
            answer.checksum
        );
    }

    // The refusal is announced in the media type the REQUEST named, exactly as an
    // operation's own answer is: which layer answered does not change that.
    for announced in [PROTOCOL_MEDIA_TYPE, JSON_MEDIA_TYPE] {
        let answer = send(
            &router,
            Probe {
                target: Some("DynamoDB_20120810.NoSuchOperation"),
                content_type: Some(announced),
                ..API_CALL
            },
        )
        .await;
        assert_eq!(
            (answer.status, answer.content_type.as_deref()),
            (400, Some(announced)),
            "case a refusal asked for in {announced:?}: expected it announced in that media \
             type, got {} {:?}",
            answer.status,
            answer.content_type
        );
    }
}

/// RULE 5 across the LAYERS that can answer a document of this API: a handler's success, a
/// handler's modelled error, the constraint walk ahead of the handler, and the front door's
/// own refusal.
#[tokio::test]
async fn every_document_this_api_answers_reports_the_checksum_of_its_own_body() {
    let router = emulator();

    // (case, the whole X-Amz-Target, the request body, the status it is answered with)
    let rows: [(&str, &str, &str, u16); 4] = [
        ("a handler's success", LIST_TABLES, "{}", 200),
        (
            "a handler's modelled error",
            "DynamoDB_20120810.DescribeTable",
            r#"{"TableName":"NoSuchTable"}"#,
            400,
        ),
        (
            "a refusal from the constraint walk ahead of the handler",
            "DynamoDB_20120810.Scan",
            r#"{"TableName":"NoSuchTable","Limit":0}"#,
            400,
        ),
        (
            "the front door's refusal of an unknown target",
            "DynamoDB_20120810.NoSuchOperation",
            "{}",
            400,
        ),
    ];

    for (case, target, body, expected) in rows {
        let answer = send(
            &router,
            Probe {
                target: Some(target),
                body,
                ..API_CALL
            },
        )
        .await;
        assert_eq!(
            answer.status, expected,
            "case {case:?}: expected HTTP {expected}, got {} with {:?}",
            answer.status, answer.body
        );
        let expected_checksum = crc32_of(&answer.body);
        assert_eq!(
            answer.checksum.as_deref(),
            Some(expected_checksum.as_str()),
            "case {case:?}: expected the CRC-32 {expected_checksum} of the body it answered, \
             got {:?}",
            answer.checksum
        );
        assert!(
            answer
                .checksum
                .as_deref()
                .is_some_and(|reported| reported.parse::<u32>().is_ok()),
            "case {case:?}: the checksum is reported as an unsigned decimal integer, got {:?}",
            answer.checksum
        );
    }
}
