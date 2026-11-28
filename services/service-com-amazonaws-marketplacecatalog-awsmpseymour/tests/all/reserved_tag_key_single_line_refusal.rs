// SPDX-License-Identifier: CC0-1.0

//! The refusal of a reserved tag key is a SINGLE-LINE sentence, end to end over HTTP.
//!
//! RULE: every sentence this service builds around a submitted value is carried by a
//! member whose shape declares `@pattern("^(.)+$")` beside `@length(1, 2048)`
//! (`ExceptionMessageContent` on `ValidationException$Message` —
//! `smithy_query(query="shape", shape="ExceptionMessageContent")`), and `.` matches no line
//! terminator — so the value is quoted back with every terminator ESCAPED rather than
//! spliced in raw, whichever member carried it.
//!
//! A tag key is the submitted value where that matters most: `Tag$Key`'s own `@pattern`
//! (`^([\p{L}\p{Z}\p{N}_.:/=+\-@]*)$`, `smithy_query(query="shape", shape="Tag")`) admits
//! the whole Unicode `Z` category, whose `Zl` and `Zp` members (U+2028 LINE SEPARATOR,
//! U+2029 PARAGRAPH SEPARATOR) ARE line terminators. So the request boundary accepts such a
//! key, the handler judges it against the reserved namespace, and the sentence it builds has
//! to carry the key escaped.
//!
//! The rows span what the rule generalises over: each separator the key syntax admits, where
//! it sits in the key, how many of them the key carries, the length of the key up to the
//! modelled maximum, the casing of the reserved prefix, and EVERY request member of this
//! crate that carries tag keys — `TagResource$Tags`, `UntagResource$TagKeys`,
//! `StartChangeSet$ChangeSetTags`, a change's `StartChangeSet$ChangeSet$EntityTags` and
//! `StartAssessment$Tags`. The control test holds the other side: a `\p{Z}` separator that
//! is NOT a line terminator is quoted exactly as sent.
//!
//! Every assertion goes through the router, so each case exercises routing,
//! deserialization, the boundary validator, the handler and serialization.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_marketplacecatalog_awsmpseymour::state::{
    entity_arn, is_reserved_tag_key, RESERVED_TAG_KEY_PREFIX,
};
use service_com_amazonaws_marketplacecatalog_awsmpseymour::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The catalog every operation's `Catalog` member documents as its fixed value.
const CATALOG: &str = "AWSMarketplace";

/// The region the test's requests are signed for, and the one the ARNs they name carry.
const REGION: &str = "eu-west-1";

/// A SigV4 credential scope naming this service's signing name (`aws-marketplace`), which
/// is what routes a request here.
const SIGNED_FOR_EU_WEST_1: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/eu-west-1/aws-marketplace/aws4_request, \
     SignedHeaders=host, Signature=abc";

/// The restJson1 document media type: `application/json`, for answers and refusals alike.
const JSON: &str = "application/json";

/// How long a message may be: the `@length(max)` of `ExceptionMessageContent`
/// (`smithy_query(query="shape", shape="ExceptionMessageContent")`), written out here rather
/// than read from the crate so a wrong implementation cannot pass by quoting its own bound.
const MESSAGE_MAX: usize = 2048;

/// How long a tag key may be: the `@length(max)` of `TagKey`
/// (`smithy_query(query="shape", shape="TagKey")`), spelled out for the same reason.
const TAG_KEY_MAX: usize = 128;

/// A framework this catalog assesses against, as `FrameworkId`'s own doc gives the
/// unversioned form (`smithy_query(query="doc", shape="StartAssessmentRequest",
/// member="FrameworkId")` — "For example, `AMISecurity`").
const FRAMEWORK: &str = "AMISecurity";

/// The line terminators `Tag$Key`'s `@pattern` ADMITS: the members of the Unicode `Z`
/// category that terminate a line, which is exactly `Zl` and `Zp`. Every other line
/// terminator (a line feed, a carriage return, a next line) is a `Cc` character the key
/// syntax excludes, so it never reaches the handler that builds this sentence.
///
/// `(the name of the case, the character, the escape a message shows it as)`.
const KEY_SEPARATORS_THAT_END_A_LINE: &[(&str, char, &str)] = &[
    ("a line separator (U+2028, category Zl)", '\u{2028}', r"\u2028"),
    (
        "a paragraph separator (U+2029, category Zp)",
        '\u{2029}',
        r"\u2029",
    ),
];

/// The other side of the key syntax's `\p{Z}`: separators that are NOT line terminators, so
/// a message carries them exactly as sent. One from each of the places `Zs` draws from — the
/// ASCII space, a Latin-1 space and a CJK space.
const KEY_SEPARATORS_THAT_DO_NOT: &[(&str, char)] = &[
    ("a space (U+0020, category Zs)", ' '),
    ("a no-break space (U+00A0, category Zs)", '\u{a0}'),
    ("an ideographic space (U+3000, category Zs)", '\u{3000}'),
];

/// One emulator: a router carrying only this service, over one shared engine.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// What the emulator answered: the HTTP status, the JSON body and the media type.
struct Answer {
    status: u16,
    body: Value,
    content_type: String,
}

impl Answer {
    /// The wire error code (`__type`), or the empty string on a success.
    fn error_code(&self) -> &str {
        let raw = self.body["__type"].as_str().unwrap_or_default();
        raw.rsplit('#').next().unwrap_or(raw)
    }

    /// The `Message` of a refusal, under either casing a protocol spells it.
    fn message(&self) -> String {
        let reported = self
            .body
            .pointer("/Message")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if reported.is_empty() {
            return self
                .body
                .pointer("/message")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
        }
        reported.to_string()
    }
}

async fn call(router: &MultiServiceRouter, method: &str, uri: &str, body: Value) -> Answer {
    let payload = body.to_string();
    let request = ::http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", "catalog.marketplace.eu-west-1.amazonaws.com")
        .header("authorization", SIGNED_FOR_EU_WEST_1)
        .header("content-type", JSON)
        .body(Bytes::from(payload))
        .expect("the test request is well formed");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|error| {
            panic!(
                "{method} {uri} answered a body that is not JSON ({error}): {:?}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    Answer {
        status,
        body,
        content_type,
    }
}

/// One request carrying `key` at a member that accepts tag keys, and the member the refusal
/// has to be stated against.
struct Case {
    /// The member and operation this row is about.
    case: &'static str,
    /// The request member the fault names, which is what the sentence opens with.
    member: &'static str,
    method: &'static str,
    uri: &'static str,
    body: Value,
}

/// Every request member of this crate that carries tag keys, each carrying `key`.
///
/// The ARN the two tagging operations name is the one this service itself mints for an
/// entity ([`entity_arn`]), so each request is one the handler reads the member of rather
/// than one it refuses for its ARN.
fn cases(key: &str) -> Vec<Case> {
    let arn = entity_arn(REGION, "AmiProduct", "ent-abc123");
    vec![
        Case {
            case: "TagResource: the key alone in Tags",
            member: "Tags",
            method: "POST",
            uri: "/TagResource",
            body: json!({"ResourceArn": arn, "Tags": [{"Key": key, "Value": "v"}]}),
        },
        Case {
            case: "TagResource: the key last of two in Tags",
            member: "Tags",
            method: "POST",
            uri: "/TagResource",
            body: json!({
                "ResourceArn": arn,
                "Tags": [{"Key": "env", "Value": "test"}, {"Key": key, "Value": "v"}],
            }),
        },
        Case {
            case: "UntagResource: the key in TagKeys",
            member: "TagKeys",
            method: "POST",
            uri: "/UntagResource",
            body: json!({"ResourceArn": arn, "TagKeys": ["env", key]}),
        },
        Case {
            case: "StartChangeSet: the key in ChangeSetTags",
            member: "ChangeSetTags",
            method: "POST",
            uri: "/StartChangeSet",
            body: json!({
                "Catalog": CATALOG,
                "ChangeSet": [{
                    "ChangeType": "CreateProduct",
                    "Entity": {"Type": "AmiProduct@1.0"},
                    "DetailsDocument": {"ProductTitle": "a single line"},
                }],
                "ChangeSetTags": [{"Key": key, "Value": "v"}],
            }),
        },
        Case {
            case: "StartChangeSet: the key in a change's EntityTags",
            member: "ChangeSet.EntityTags",
            method: "POST",
            uri: "/StartChangeSet",
            body: json!({
                "Catalog": CATALOG,
                "ChangeSet": [{
                    "ChangeType": "CreateProduct",
                    "Entity": {"Type": "AmiProduct@1.0"},
                    "DetailsDocument": {"ProductTitle": "a single line"},
                    "EntityTags": [{"Key": key, "Value": "v"}],
                }],
            }),
        },
        Case {
            case: "StartAssessment: the key in the Tags map",
            member: "Tags",
            method: "POST",
            uri: "/StartAssessment",
            body: json!({
                "Catalog": CATALOG,
                "FrameworkId": FRAMEWORK,
                "EntityId": "ent-abc123",
                "Tags": {key: "v"},
            }),
        },
    ]
}

/// The reserved keys a row carries `separator` in, each inside the namespace the crate
/// publishes and each inside `TagKey`'s modelled length: the separator straight after the
/// prefix, at the end of the key, twice over, in a key as long as the member allows, and
/// under a differently cased prefix.
fn keys_carrying(separator: char) -> Vec<(String, String)> {
    let prefix = RESERVED_TAG_KEY_PREFIX;
    let upper = prefix.to_uppercase();
    let filler = "a".repeat(TAG_KEY_MAX - prefix.len() - 1);
    vec![
        (
            "the separator straight after the prefix".to_string(),
            format!("{prefix}{separator}createdBy"),
        ),
        (
            "the separator at the end of the key".to_string(),
            format!("{prefix}createdBy{separator}"),
        ),
        (
            "the separator twice over".to_string(),
            format!("{prefix}a{separator}b{separator}c"),
        ),
        (
            format!("a key of the modelled maximum {TAG_KEY_MAX} characters"),
            format!("{prefix}{filler}{separator}"),
        ),
        (
            "the prefix in capitals".to_string(),
            format!("{upper}{separator}CreatedBy"),
        ),
    ]
}

/// `key` as a single-line message carries it: every line terminator the key syntax admits
/// written as the escape that stands for it, every other character as sent.
fn escaped(key: &str) -> String {
    let mut single = String::with_capacity(key.len());
    for c in key.chars() {
        match KEY_SEPARATORS_THAT_END_A_LINE
            .iter()
            .find(|(_, terminator, _)| *terminator == c)
        {
            Some((_, _, escape)) => single.push_str(escape),
            None => single.push(c),
        }
    }
    single
}

/// Assert `message` is a sentence `ExceptionMessageContent` may carry: not empty, one line,
/// and inside the length it declares.
fn assert_single_line(case: &str, message: &str) {
    assert!(
        !message.is_empty(),
        "case {case:?}: the refusal must state a message, got {message:?}"
    );
    let carried = message
        .chars()
        .find(|c| matches!(c, '\n' | '\r' | '\u{85}' | '\u{2028}' | '\u{2029}'));
    assert!(
        carried.is_none(),
        "case {case:?}: a message member admits no line terminator, got {carried:?} in \
         {message:?}"
    );
    assert!(
        message.chars().count() <= MESSAGE_MAX,
        "case {case:?}: a message is at most {MESSAGE_MAX} characters, got {} in {message:?}",
        message.chars().count()
    );
}

/// RULE: a reserved-key refusal quoting a key that carries a line terminator the key syntax
/// admits shows that terminator ESCAPED, and the sentence stays one line inside the length
/// its message member declares — at every member of this service that accepts tag keys.
#[tokio::test]
async fn a_reserved_key_carrying_a_line_separator_is_refused_in_one_line() {
    let router = emulator();

    for (separator, character, escape) in KEY_SEPARATORS_THAT_END_A_LINE {
        for (shape, key) in keys_carrying(*character) {
            assert!(
                is_reserved_tag_key(&key),
                "case {shape:?} with {separator}: {key:?} must lie inside the reserved namespace"
            );
            assert!(
                key.chars().count() <= TAG_KEY_MAX,
                "case {shape:?} with {separator}: {key:?} is longer than the modelled \
                 {TAG_KEY_MAX} characters, got {}",
                key.chars().count()
            );

            for row in cases(&key) {
                let case = format!("{} ({shape}, {separator})", row.case);
                let answer = call(&router, row.method, row.uri, row.body).await;
                assert_eq!(
                    (
                        answer.status,
                        answer.error_code(),
                        answer.content_type.as_str()
                    ),
                    (422, "ValidationException", JSON),
                    "case {case:?}: a reserved tag key must be refused 422 ValidationException \
                     as application/json, got {} {:?} with {:?}",
                    answer.status,
                    answer.error_code(),
                    answer.body
                );
                let message = answer.message();
                assert_single_line(&case, &message);

                let opening = format!("[{}: tag key '", row.member);
                assert!(
                    message.starts_with(&opening),
                    "case {case:?}: the sentence must open with {opening:?}, got {message:?}"
                );
                let quoted = escaped(&key);
                assert!(
                    message.contains(&format!("tag key '{quoted}' begins with ")),
                    "case {case:?}: the key must be quoted with the separator escaped as \
                     {escape:?}, got {message:?}"
                );
                assert!(
                    message.contains(RESERVED_TAG_KEY_PREFIX),
                    "case {case:?}: the refusal must name the reserved prefix \
                     {RESERVED_TAG_KEY_PREFIX:?}, got {message:?}"
                );
            }
        }
    }
}

/// The same rule's CONTROL: a `\p{Z}` separator that is NOT a line terminator is part of the
/// key as sent, so the refusal quotes it verbatim — the escaping above is about the
/// terminator and not about the separator class the key syntax admits.
#[tokio::test]
async fn a_reserved_key_carrying_a_space_is_quoted_as_sent() {
    let router = emulator();

    for (separator, character) in KEY_SEPARATORS_THAT_DO_NOT {
        let key = format!("{RESERVED_TAG_KEY_PREFIX}a{character}b");
        for row in cases(&key) {
            let case = format!("{} ({separator})", row.case);
            let answer = call(&router, row.method, row.uri, row.body).await;
            assert_eq!(
                (answer.status, answer.error_code()),
                (422, "ValidationException"),
                "case {case:?}: a reserved tag key must be refused 422 ValidationException, \
                 got {} {:?} with {:?}",
                answer.status,
                answer.error_code(),
                answer.body
            );
            let message = answer.message();
            assert_single_line(&case, &message);
            assert!(
                message.contains(&format!("tag key '{key}' begins with ")),
                "case {case:?}: the key must be quoted as sent, got {message:?}"
            );
        }
    }
}
