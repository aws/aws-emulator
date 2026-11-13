// SPDX-License-Identifier: CC0-1.0

//! A Kinesis data stream's TAGS, end to end over the wire: `AddTagsToStream` ->
//! `ListTagsForStream`.
//!
//! RULE: `AddTagsToStream` ASSIGNS every key-value pair its request carries to the stored
//! stream it addresses — a key the stream already carries takes the new value, every tag
//! the request does not name stays as it was — and `ListTagsForStream` reports that STORED
//! set, ordered by key, one page at a time: a page of at most the `Limit` the request names
//! starting after the `ExclusiveStartTagKey` it names, with `HasMoreTags` true exactly when
//! the stream carries a tag past that page. So whatever an assignment wrote is what a read
//! reports, in one order every read agrees on, and re-assigning what is already there
//! changes nothing. Either operation resolves its stream by `StreamName`, by `StreamARN` or
//! by both, inside the region the request was signed for; a request naming a stream this
//! account and region do not hold is `ResourceNotFoundException`, one naming no stream at
//! all (or two different streams) is `InvalidArgumentException`, and an assignment that
//! would leave the stream carrying more tags than one stream may carry is
//! `LimitExceededException`. A stream's tag set exists for as long as the stream does, so
//! an assignment is served in every lifecycle status the stream has.
//!
//! The recorded flow this was derived from tags ONE stream with ONE pair
//! (`add_tags_to_stream(StreamName=…, Tags={…})`) and is one row of the tables below. The
//! rows differ along what the rule generalises over: how many pairs an assignment carries,
//! whether its keys are new or already present, the form and length of a key and a value
//! (both at their modelled bounds), the order the keys arrive in, how many assignments the
//! stream takes, the identifier the request resolves the stream by, and the page size and
//! cursor a read asks for.
//!
//! Every case drives the real awsJson1_1 request path (`POST /` with `X-Amz-Target:
//! Kinesis_20131202.<Operation>` through `MultiServiceRouter::dispatch`), so the statuses,
//! bodies and modelled errors asserted here are the ones a client sees.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Map, Value};
use service_com_amazonaws_kinesis_kinesis_20131202::state::{
    stream_not_found_message, ACTIVATION_DELAY, MAX_LIST_TAGS_FOR_STREAM_LIMIT, MAX_STREAM_TAGS,
    STREAM_ACTIVE, STREAM_CREATING, STREAM_DELETING, stream_identifier_mismatch_message,
    STREAM_IDENTIFIER_REQUIRED_MESSAGE,
};
use service_com_amazonaws_kinesis_kinesis_20131202::state::ComAmazonawsKinesisKinesis20131202State;
use service_com_amazonaws_kinesis_kinesis_20131202::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region most cases sign for.
pub(crate) const REGION: &str = "us-east-1";

/// The `X-Amz-Target` prefix of this service (`OPERATIONS.md`, "Wire protocol").
const TARGET_PREFIX: &str = "Kinesis_20131202";

/// The `@length` maximum of `Tag$Key`, in characters
/// (`smithy_query(query="shape",shape="Tag")` -> `Key` len 1..128).
const TAG_KEY_MAX_LENGTH: usize = 128;

/// The `@length` maximum of `Tag$Value` (same call -> `Value` len 0..256).
const TAG_VALUE_MAX_LENGTH: usize = 256;

/// Characters `Tag$Key` and `Tag$Value` document as valid beyond letters and digits:
/// "Valid characters: Unicode letters, digits, white space, _ . / = + - % @"
/// (`smithy_query(query="shape",shape="Tag")`, both members). The model carries no
/// `@pattern` for either, so this doc sentence is the catalog.
const TAG_PUNCTUATION: &str = "_ ./=+-%@";

/// How long a case waits before treating a freshly created stream as ready: past the
/// instant the create armed the `CREATING` -> `ACTIVE` transition for.
pub(crate) fn past_activation() -> Duration {
    ACTIVATION_DELAY + Duration::from_millis(400)
}

/// A router carrying exactly this service, over one shared engine: the same assembly
/// `core/router` performs, with one service mounted.
pub(crate) struct Emulator {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

impl Emulator {
    pub(crate) fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // The signature itself is not under test here, so requests are unauthenticated.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        Self { router, engine }
    }

    /// This service's store inside the engine the router was assembled over — the very
    /// store every handler of it reads and writes.
    ///
    /// A case that asserts what an operation RECORDED beside what it reports (the rolling
    /// window a quota is counted over, say) reads it here, and one that needs a stream in a
    /// state reaching it over the wire would cost a long wait sets it up here.
    pub(crate) fn store(&self) -> engine::ServiceState<ComAmazonawsKinesisKinesis20131202State> {
        self.engine
            .state::<ComAmazonawsKinesisKinesis20131202State>()
    }

    /// Call `operation` with `body`, signed for `region` so the service resolves the
    /// request's region the way it does in production (the SigV4 credential scope).
    pub(crate) async fn call_in(&self, region: &str, operation: &str, body: &Value) -> (u16, Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", format!("{TARGET_PREFIX}.{operation}"))
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/kinesis/\
                     aws4_request, SignedHeaders=host;x-amz-target, Signature=deadbeef"
                ),
            )
            .body(Bytes::from(body.to_string()))
            .expect("build the request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed: Value = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
        (status, parsed)
    }

    /// [`Self::call_in`] in the default test region.
    pub(crate) async fn call(&self, operation: &str, body: &Value) -> (u16, Value) {
        self.call_in(REGION, operation, body).await
    }

    /// Call `operation` in `region`, asserting a 200 and answering the body.
    pub(crate) async fn ok_in(&self, case: &str, region: &str, operation: &str, body: &Value) -> Value {
        let (status, response) = self.call_in(region, operation, body).await;
        assert_eq!(
            status, 200,
            "case {case}: expected {operation} to answer 200, got {status} with body \
             {response}"
        );
        response
    }

    /// [`Self::ok_in`] in the default test region.
    pub(crate) async fn ok(&self, case: &str, operation: &str, body: &Value) -> Value {
        self.ok_in(case, REGION, operation, body).await
    }
}

/// The `__type` an awsJson1_1 error body carries, reduced to the error's shape name.
pub(crate) fn error_code(response: &Value) -> String {
    response
        .get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        .rsplit(['#', '/'])
        .next()
        .unwrap_or("<no __type>")
        .to_string()
}

/// The `message` an awsJson1_1 error body carries, under either casing the protocols use.
pub(crate) fn error_message(response: &Value) -> String {
    response
        .get("message")
        .or_else(|| response.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// How long a case keeps LOOKING for a state the service owes it before giving up. Generous
/// on purpose: what a case waits for is a CONDITION the emulator reaches on its own clock,
/// never a duration the case measures, so a slow or loaded host only makes it look again.
pub(crate) const POLL_DEADLINE: Duration = Duration::from_secs(60);

/// How long a case waits between two looks while polling for such a condition.
pub(crate) const BETWEEN_LOOKS: Duration = Duration::from_millis(25);

/// Look again until `holds` answers true, and fail once [`POLL_DEADLINE`] has passed.
///
/// THE ONE poll loop of this crate's suites, whichever harness a case drives: what a case
/// waits for is a CONDITION the emulator reaches on its own clock — a stream that reports
/// `ACTIVE`, a channel that has settled, a name a delete has released — never a duration the
/// case measures, so a slow or loaded host only makes it look again. `what` names the
/// condition in the failure.
pub(crate) async fn poll_until<F, Fut>(case: &str, what: &str, holds: F)
where
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    let deadline = std::time::Instant::now() + POLL_DEADLINE;
    loop {
        if holds().await {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "case {case}: {what} never became true inside the deadline"
        );
        tokio::time::sleep(BETWEEN_LOOKS).await;
    }
}

/// Look at the stream named `name` in `region` until it reports `ACTIVE` — the state its
/// create owes it — through [`poll_until`].
pub(crate) async fn poll_until_active_in(emulator: &Emulator, case: &str, region: &str, name: &str) {
    poll_until(
        case,
        &format!("the stream {name} reports {STREAM_ACTIVE}"),
        || async {
            let (status, response) = emulator
                .call_in(region, "DescribeStreamSummary", &json!({ "StreamName": name }))
                .await;
            status == 200
                && response["StreamDescriptionSummary"]["StreamStatus"].as_str()
                    == Some(STREAM_ACTIVE)
        },
    )
    .await;
}

/// [`poll_until_active_in`] in the default test region.
pub(crate) async fn poll_until_active(emulator: &Emulator, case: &str, name: &str) {
    poll_until_active_in(emulator, case, REGION, name).await;
}

/// Look until this account and region hold no stream under `name` any more — the removal a
/// delete owes — through [`poll_until`].
pub(crate) async fn poll_until_gone(emulator: &Emulator, case: &str, name: &str) {
    poll_until(case, &format!("the stream {name} is released"), || async {
        let (status, response) = emulator
            .call("DescribeStreamSummary", &json!({ "StreamName": name }))
            .await;
        status == 400 && error_code(&response) == "ResourceNotFoundException"
    })
    .await;
}

/// The ARN a stream named `name` in `region` is addressed by.
pub(crate) fn stream_arn(region: &str, name: &str) -> String {
    format!(
        "arn:aws:kinesis:{region}:{}:stream/{name}",
        engine::EMULATOR_ACCOUNT_ID
    )
}

/// A `TagMap` body member built from pairs.
pub(crate) fn tag_map(tags: &[(String, String)]) -> Value {
    Value::Object(
        tags.iter()
            .map(|(key, value)| (key.clone(), Value::String(value.clone())))
            .collect::<Map<String, Value>>(),
    )
}

/// The `Tags` a `ListTagsForStream` response carries, as pairs in the order the response
/// listed them.
pub(crate) fn reported_tags(case: &str, response: &Value) -> Vec<(String, String)> {
    let tags = response
        .get("Tags")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case}: ListTagsForStream must carry Tags, got {response}"));
    tags.iter()
        .map(|tag| {
            let key = tag
                .get("Key")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("case {case}: a Tag must carry a Key, got {tag}"));
            // `Tag$Value` is optional, so a tag that carries none reads as an empty value.
            let value = tag.get("Value").and_then(Value::as_str).unwrap_or_default();
            (key.to_string(), value.to_string())
        })
        .collect()
}

/// Whether the response says the stream carries tags past the page it answered with.
fn has_more_tags(case: &str, response: &Value) -> bool {
    response
        .get("HasMoreTags")
        .and_then(Value::as_bool)
        .unwrap_or_else(|| {
            panic!("case {case}: ListTagsForStream must carry HasMoreTags, got {response}")
        })
}

/// How a request addresses the stream it operates on — the identifiers the rule says both
/// operations accept.
#[derive(Debug, Clone, Copy)]
enum Address {
    /// `StreamName` only.
    Name,
    /// `StreamARN` only, which the docs recommend.
    Arn,
    /// Both, naming one stream.
    Both,
}

impl Address {
    /// The identifier members this addressing adds to a request body.
    fn members(self, name: &str) -> Value {
        let arn = stream_arn(REGION, name);
        match self {
            Self::Name => json!({ "StreamName": name }),
            Self::Arn => json!({ "StreamARN": arn }),
            Self::Both => json!({ "StreamName": name, "StreamARN": arn }),
        }
    }
}

/// One row of the assignment table: what the stream was created with, what successive
/// `AddTagsToStream` calls assign to it, and the tag set it must then report.
struct TagCase {
    label: &'static str,
    /// The `Tags` the create carries, empty on a row whose create names none.
    created_with: Vec<(String, String)>,
    /// One `AddTagsToStream` request each, served in order.
    assignments: Vec<Vec<(String, String)>>,
    /// How those requests, and the read that follows, address the stream.
    address: Address,
    /// The tags the stream must report, given here UNSORTED so the key order the rule
    /// promises is what the assertion checks.
    expected: Vec<(String, String)>,
}

/// A pair, spelled short.
pub(crate) fn pair(key: &str, value: &str) -> (String, String) {
    (key.to_string(), value.to_string())
}

/// `count` distinct pairs, keyed so lexicographic key order is also numeric order.
pub(crate) fn pairs(count: usize) -> Vec<(String, String)> {
    (0..count)
        .map(|index| (format!("key-{index:02}"), format!("value-{index:02}")))
        .collect()
}

/// The table the assignment/read rule is checked over. The rows differ along how many
/// pairs an assignment carries, whether its keys are new or already present, the form and
/// length of a key and a value, the order the keys arrive in, how many assignments the
/// stream takes, and the identifier the requests resolve the stream by.
fn tag_cases() -> Vec<TagCase> {
    vec![
        // The recorded shape: one pair onto a stream created with no tags at all.
        TagCase {
            label: "one-pair-onto-an-untagged-stream",
            created_with: Vec::new(),
            assignments: vec![vec![pair("Environment", "production")]],
            address: Address::Name,
            expected: vec![pair("Environment", "production")],
        },
        // Several pairs at once, arriving in an order that is not key order.
        TagCase {
            label: "three-pairs-at-once-out-of-key-order",
            created_with: Vec::new(),
            assignments: vec![vec![
                pair("owner", "data-platform"),
                pair("Environment", "staging"),
                pair("costCenter", "4711"),
            ]],
            address: Address::Arn,
            expected: vec![
                pair("owner", "data-platform"),
                pair("costCenter", "4711"),
                pair("Environment", "staging"),
            ],
        },
        // A key the stream already carries takes the new value, and the tag the assignment
        // does not name stays as it was.
        TagCase {
            label: "overwrites-one-key-and-keeps-the-rest",
            created_with: vec![pair("Environment", "staging"), pair("owner", "team-a")],
            assignments: vec![vec![pair("Environment", "production")]],
            address: Address::Both,
            expected: vec![pair("owner", "team-a"), pair("Environment", "production")],
        },
        // Successive assignments accumulate: three calls, three keys, in no key order.
        TagCase {
            label: "three-successive-assignments-accumulate",
            created_with: Vec::new(),
            assignments: vec![
                vec![pair("zeta", "3")],
                vec![pair("alpha", "1")],
                vec![pair("mu", "2")],
            ],
            address: Address::Name,
            expected: vec![pair("mu", "2"), pair("zeta", "3"), pair("alpha", "1")],
        },
        // Re-assigning exactly what is already there changes nothing (and stays inside the
        // per-stream ceiling, since an assignment overwrites rather than adds).
        TagCase {
            label: "re-assigning-the-same-pairs-is-idempotent",
            created_with: vec![pair("owner", "team-b")],
            assignments: vec![
                vec![pair("owner", "team-b"), pair("tier", "gold")],
                vec![pair("owner", "team-b"), pair("tier", "gold")],
            ],
            address: Address::Name,
            expected: vec![pair("owner", "team-b"), pair("tier", "gold")],
        },
        // `Tag$Value` is modelled `len 0..256`, so the empty value is a value.
        TagCase {
            label: "a-pair-whose-value-is-empty",
            created_with: Vec::new(),
            assignments: vec![vec![pair("marker", ""), pair("other", "kept")]],
            address: Address::Name,
            expected: vec![pair("other", "kept"), pair("marker", "")],
        },
        // Both members at their modelled `@length` maxima, and the documented punctuation
        // the issue's own pairs never exercise.
        TagCase {
            label: "key-and-value-at-their-length-maxima-and-documented-punctuation",
            created_with: Vec::new(),
            assignments: vec![vec![
                pair(
                    &"k".repeat(TAG_KEY_MAX_LENGTH),
                    &"v".repeat(TAG_VALUE_MAX_LENGTH),
                ),
                pair(TAG_PUNCTUATION, TAG_PUNCTUATION),
                // The `@length` minimum of `Tag$Key` is one character.
                pair("x", "1"),
            ]],
            address: Address::Arn,
            expected: vec![
                pair("x", "1"),
                pair(TAG_PUNCTUATION, TAG_PUNCTUATION),
                pair(
                    &"k".repeat(TAG_KEY_MAX_LENGTH),
                    &"v".repeat(TAG_VALUE_MAX_LENGTH),
                ),
            ],
        },
        // The whole ceiling in one call: the most tags one stream may carry.
        TagCase {
            label: "the-maximum-number-of-tags-in-one-assignment",
            created_with: Vec::new(),
            assignments: vec![pairs(MAX_STREAM_TAGS)],
            address: Address::Name,
            expected: pairs(MAX_STREAM_TAGS),
        },
        // The ceiling reached across two calls, the second overwriting part of the first.
        TagCase {
            label: "the-maximum-number-of-tags-across-two-assignments",
            created_with: pairs(10),
            assignments: vec![
                pairs(MAX_STREAM_TAGS / 2),
                pairs(MAX_STREAM_TAGS).split_off(MAX_STREAM_TAGS / 2),
            ],
            address: Address::Both,
            expected: pairs(MAX_STREAM_TAGS),
        },
    ]
}

/// The pairs `expected` holds, in the key order every read must report them in.
pub(crate) fn in_key_order(expected: &[(String, String)]) -> Vec<(String, String)> {
    let mut ordered = expected.to_vec();
    ordered.sort_by(|left, right| left.0.cmp(&right.0));
    ordered
}

/// Create the case's stream and serve its assignments, then read the whole tag list back.
#[tokio::test]
async fn assigned_tags_are_reported_back_in_key_order() {
    let emulator = Emulator::new();
    let cases = tag_cases();

    // Every stream is created first, so the cases share ONE wait for the transition the
    // creates armed rather than one each.
    for case in &cases {
        let mut body = json!({ "StreamName": case.label, "ShardCount": 1 });
        if !case.created_with.is_empty() {
            body["Tags"] = tag_map(&case.created_with);
        }
        emulator.ok(case.label, "CreateStream", &body).await;
    }
    tokio::time::sleep(past_activation()).await;

    for case in &cases {
        for (index, assignment) in case.assignments.iter().enumerate() {
            let mut body = case.address.members(case.label);
            body["Tags"] = tag_map(assignment);
            let answered = emulator
                .ok(case.label, "AddTagsToStream", &body)
                .await;
            assert_eq!(
                answered,
                json!({}),
                "case {}: AddTagsToStream #{index} models no output members, got {answered}",
                case.label
            );
        }

        let response = emulator
            .ok(
                case.label,
                "ListTagsForStream",
                &case.address.members(case.label),
            )
            .await;
        let reported = reported_tags(case.label, &response);
        assert_eq!(
            reported,
            in_key_order(&case.expected),
            "case {}: the stream must report every assigned tag, in key order, got {:?}",
            case.label,
            reported
        );
        assert!(
            !has_more_tags(case.label, &response),
            "case {}: a read of the whole tag list carries no more tags, got {response}",
            case.label
        );

        // A read reports the stored set; it does not change it.
        let again = emulator
            .ok(
                case.label,
                "ListTagsForStream",
                &case.address.members(case.label),
            )
            .await;
        assert_eq!(
            reported_tags(case.label, &again),
            reported,
            "case {}: a second read reports the same tags, got {again}",
            case.label
        );
    }
}

/// One page of a tag list: the `Limit` a request names and what it must answer with.
struct PageCase {
    label: &'static str,
    /// The `Limit` the request names, absent on a row that names none.
    limit: Option<i64>,
    /// How many tags the stream carries.
    stored: usize,
    /// How many tags the page must hold.
    expected_page: usize,
}

/// Paging is checked over the published bounds of `Limit` (`range 1..50`), a value inside
/// them, and a request naming none — against a stream carrying fewer, exactly as many, and
/// more tags than the page holds.
fn page_cases() -> Vec<PageCase> {
    vec![
        PageCase {
            label: "limit-at-its-published-minimum",
            limit: Some(1),
            stored: 7,
            expected_page: 1,
        },
        PageCase {
            label: "limit-inside-its-published-bounds",
            limit: Some(3),
            stored: 7,
            expected_page: 3,
        },
        PageCase {
            label: "limit-equal-to-the-stored-count",
            limit: Some(7),
            stored: 7,
            expected_page: 7,
        },
        PageCase {
            label: "limit-past-the-stored-count",
            limit: Some(20),
            stored: 7,
            expected_page: 7,
        },
        PageCase {
            label: "limit-at-its-published-maximum",
            limit: Some(i64::from(MAX_LIST_TAGS_FOR_STREAM_LIMIT)),
            stored: MAX_STREAM_TAGS,
            expected_page: MAX_STREAM_TAGS,
        },
        PageCase {
            label: "no-limit-at-all",
            limit: None,
            stored: 7,
            expected_page: 7,
        },
    ]
}

#[tokio::test]
async fn a_tag_list_pages_by_limit_and_exclusive_start_tag_key() {
    let emulator = Emulator::new();
    let cases = page_cases();

    for case in &cases {
        emulator
            .ok(
                case.label,
                "CreateStream",
                &json!({ "StreamName": case.label, "ShardCount": 1 }),
            )
            .await;
    }
    tokio::time::sleep(past_activation()).await;

    for case in &cases {
        let stored = pairs(case.stored);
        emulator
            .ok(
                case.label,
                "AddTagsToStream",
                &json!({ "StreamName": case.label, "Tags": tag_map(&stored) }),
            )
            .await;

        // Walk the whole list one page at a time, following the cursor the way the output
        // member documents: `ExclusiveStartTagKey` set to the key of the last tag returned.
        let mut walked: Vec<(String, String)> = Vec::new();
        let mut cursor: Option<String> = None;
        let mut pages = 0usize;
        loop {
            let mut body = json!({ "StreamName": case.label });
            if let Some(limit) = case.limit {
                body["Limit"] = json!(limit);
            }
            if let Some(cursor) = &cursor {
                body["ExclusiveStartTagKey"] = json!(cursor);
            }
            let response = emulator.ok(case.label, "ListTagsForStream", &body).await;
            let page = reported_tags(case.label, &response);
            assert!(
                page.len() <= case.expected_page,
                "case {}: a page holds at most {} tags, got {} of them",
                case.label,
                case.expected_page,
                page.len()
            );
            if pages == 0 {
                assert_eq!(
                    page.len(),
                    case.expected_page,
                    "case {}: the first page holds {} tags, got {page:?}",
                    case.label,
                    case.expected_page
                );
                assert_eq!(
                    has_more_tags(case.label, &response),
                    case.stored > case.expected_page,
                    "case {}: HasMoreTags says whether a tag is left past the page, got \
                     {response}",
                    case.label
                );
            }
            cursor = page.last().map(|(key, _)| key.clone());
            walked.extend(page);
            pages += 1;
            if !has_more_tags(case.label, &response) {
                break;
            }
            assert!(
                pages <= case.stored + 1,
                "case {}: the walk must terminate, it took {pages} pages",
                case.label
            );
        }

        assert_eq!(
            walked,
            in_key_order(&stored),
            "case {}: the pages concatenate to the whole tag list in key order, got \
             {walked:?}",
            case.label
        );

        // The cursor is EXCLUSIVE: a read starting after the last key is an empty page.
        let past_the_end = emulator
            .ok(
                case.label,
                "ListTagsForStream",
                &json!({
                    "StreamName": case.label,
                    "ExclusiveStartTagKey": walked.last().map(|(key, _)| key.clone()),
                }),
            )
            .await;
        assert!(
            reported_tags(case.label, &past_the_end).is_empty()
                && !has_more_tags(case.label, &past_the_end),
            "case {}: a cursor at the last key leaves nothing to report, got {past_the_end}",
            case.label
        );
    }
}

#[tokio::test]
async fn a_limit_outside_its_published_range_is_refused() {
    let emulator = Emulator::new();
    let name = "limit-range";
    emulator
        .ok(
            name,
            "CreateStream",
            &json!({ "StreamName": name, "ShardCount": 1 }),
        )
        .await;
    tokio::time::sleep(past_activation()).await;

    // `Limit` is modelled `range 1..50`, so each published bound is served and a value one
    // past either is refused.
    for limit in [1, i64::from(MAX_LIST_TAGS_FOR_STREAM_LIMIT)] {
        emulator
            .ok(
                "limit-inside-the-range",
                "ListTagsForStream",
                &json!({ "StreamName": name, "Limit": limit }),
            )
            .await;
    }
    for limit in [0, i64::from(MAX_LIST_TAGS_FOR_STREAM_LIMIT) + 1] {
        let (status, response) = emulator
            .call(
                "ListTagsForStream",
                &json!({ "StreamName": name, "Limit": limit }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case limit-{limit}: a Limit outside the modelled range is refused, got \
             {status} with body {response}"
        );
    }
}

#[tokio::test]
async fn an_assignment_past_the_tag_ceiling_is_refused_and_changes_nothing() {
    let emulator = Emulator::new();
    let name = "tag-ceiling";
    let stored = pairs(MAX_STREAM_TAGS);
    emulator
        .ok(
            name,
            "CreateStream",
            &json!({ "StreamName": name, "ShardCount": 1 }),
        )
        .await;
    tokio::time::sleep(past_activation()).await;
    emulator
        .ok(
            name,
            "AddTagsToStream",
            &json!({ "StreamName": name, "Tags": tag_map(&stored) }),
        )
        .await;

    // One key the stream does not carry yet would take it past the ceiling...
    let (status, response) = emulator
        .call(
            "AddTagsToStream",
            &json!({ "StreamName": name, "Tags": { "one-too-many": "x" } }),
        )
        .await;
    assert_eq!(
        (status, error_code(&response).as_str()),
        (400, "LimitExceededException"),
        "case one-past-the-ceiling: an assignment past the tag ceiling is refused, got \
         {status} with body {response}"
    );

    // ...while re-assigning a key it already carries stays inside it, because an
    // assignment overwrites rather than adds.
    emulator
        .ok(
            "at-the-ceiling",
            "AddTagsToStream",
            &json!({ "StreamName": name, "Tags": { "key-00": "replaced" } }),
        )
        .await;

    let response = emulator
        .ok(
            "ceiling-read-back",
            "ListTagsForStream",
            &json!({ "StreamName": name }),
        )
        .await;
    let mut expected = in_key_order(&stored);
    expected[0] = pair("key-00", "replaced");
    assert_eq!(
        reported_tags("ceiling-read-back", &response),
        expected,
        "case ceiling-read-back: the refused assignment left the stored tags alone and the \
         overwrite took effect, got {response}"
    );
}

/// One row of the lifecycle table: the status a stream is put into before it is tagged,
/// and how it is put there.
struct LifecycleCase {
    /// The case's label, which is also its stream's name.
    label: &'static str,
    /// Whether the case waits past the instant the create armed the activation for.
    wait_for_activation: bool,
    /// Whether the case accepts a `DeleteStream` for the stream first, which leaves it in
    /// `DELETING` until the removal that delete owes falls due.
    delete_first: bool,
    /// The `StreamStatus` the stream is in when the assignment lands, which the case
    /// asserts through `DescribeStreamSummary` rather than assuming.
    expected_status: &'static str,
}

/// A stream's TAG SET is addressed in every lifecycle status the stream has.
///
/// RULE: a tag operation addresses the stored tag set of a stream this account and region
/// hold, and that set exists for as long as the stream does — so an assignment is served
/// whatever `StreamStatus` the stream is in, exactly as the `RemoveTagsFromStream` that
/// unassigns from the same set is. The status gate belongs to the operations that act on
/// the stream's SHARDS and to the ones that change its lifecycle, not to its metadata.
///
/// The rows span the statuses `StreamStatus` reaches here: a stream still inside the
/// transition its create armed (`CREATING`), one past it (`ACTIVE`), and one a delete has
/// been accepted for (`DELETING`).
#[tokio::test]
async fn an_assignment_is_served_in_every_lifecycle_status() {
    let cases = [
        LifecycleCase {
            label: "creating",
            wait_for_activation: false,
            delete_first: false,
            expected_status: STREAM_CREATING,
        },
        LifecycleCase {
            label: "active",
            wait_for_activation: true,
            delete_first: false,
            expected_status: STREAM_ACTIVE,
        },
        LifecycleCase {
            label: "deleting",
            wait_for_activation: true,
            delete_first: true,
            expected_status: STREAM_DELETING,
        },
    ];

    for case in &cases {
        let emulator = Emulator::new();
        emulator
            .ok(
                case.label,
                "CreateStream",
                &json!({ "StreamName": case.label, "ShardCount": 1 }),
            )
            .await;
        if case.wait_for_activation {
            tokio::time::sleep(past_activation()).await;
        }
        if case.delete_first {
            emulator
                .ok(
                    case.label,
                    "DeleteStream",
                    &json!({ "StreamName": case.label }),
                )
                .await;
        }

        let summary = emulator
            .ok(
                case.label,
                "DescribeStreamSummary",
                &json!({ "StreamName": case.label }),
            )
            .await;
        let reported_status = summary
            .get("StreamDescriptionSummary")
            .and_then(|summary| summary.get("StreamStatus"))
            .and_then(Value::as_str)
            .unwrap_or("<no StreamStatus>");
        assert_eq!(
            reported_status, case.expected_status,
            "case {}: the stream is in {} when the assignment lands, got {summary}",
            case.label, case.expected_status
        );

        let (status, response) = emulator
            .call(
                "AddTagsToStream",
                &json!({ "StreamName": case.label, "Tags": { "owner": "team-a" } }),
            )
            .await;
        assert_eq!(
            (status, &response),
            (200, &json!({})),
            "case {}: an assignment to a stream in {} is served, got {status} with body \
             {response}",
            case.label,
            case.expected_status
        );

        // What the assignment wrote is what a read of the stored set reports.
        let after = emulator
            .ok(
                case.label,
                "ListTagsForStream",
                &json!({ "StreamName": case.label }),
            )
            .await;
        assert_eq!(
            reported_tags(case.label, &after),
            vec![pair("owner", "team-a")],
            "case {}: the assignment served in {} is reported, got {after}",
            case.label,
            case.expected_status
        );
    }
}

/// One row of the refusal table: a request that names no stream of this account and region,
/// and the modelled error it must answer with.
struct RefusalCase {
    label: &'static str,
    /// The identifier members the request carries.
    identifiers: Value,
    expected_code: &'static str,
    expected_message: String,
}

#[tokio::test]
async fn a_request_naming_no_stream_of_this_account_and_region_is_refused() {
    let emulator = Emulator::new();
    let name = "refusals";
    let other = "refusals-other";
    for stream in [name, other] {
        emulator
            .ok(
                stream,
                "CreateStream",
                &json!({ "StreamName": stream, "ShardCount": 1 }),
            )
            .await;
    }
    // The same name in another region is another stream, and only the region that created
    // one resolves it.
    let elsewhere = "refusals-elsewhere";
    emulator
        .ok_in(
            elsewhere,
            "eu-west-1",
            "CreateStream",
            &json!({ "StreamName": elsewhere, "ShardCount": 1 }),
        )
        .await;
    tokio::time::sleep(past_activation()).await;

    let cases = vec![
        RefusalCase {
            label: "no-identifier-at-all",
            identifiers: json!({}),
            expected_code: "InvalidArgumentException",
            expected_message: STREAM_IDENTIFIER_REQUIRED_MESSAGE.to_string(),
        },
        RefusalCase {
            label: "only-the-reserved-stream-id",
            // `StreamId` is documented "Not Implemented. Reserved for future use", so it
            // names no stream.
            identifiers: json!({ "StreamId": "abcdefghijklmnopqrst-001" }),
            expected_code: "InvalidArgumentException",
            expected_message: STREAM_IDENTIFIER_REQUIRED_MESSAGE.to_string(),
        },
        RefusalCase {
            label: "name-and-arn-naming-different-streams",
            identifiers: json!({ "StreamName": name, "StreamARN": stream_arn(REGION, other) }),
            expected_code: "InvalidArgumentException",
            expected_message: stream_identifier_mismatch_message(
                name,
                &stream_arn(REGION, other),
            ),
        },
        RefusalCase {
            label: "a-name-no-stream-holds",
            identifiers: json!({ "StreamName": "no-such-stream" }),
            expected_code: "ResourceNotFoundException",
            expected_message: stream_not_found_message("no-such-stream"),
        },
        RefusalCase {
            label: "an-arn-no-stream-holds",
            identifiers: json!({ "StreamARN": stream_arn(REGION, "no-such-stream") }),
            expected_code: "ResourceNotFoundException",
            expected_message: stream_not_found_message(&stream_arn(REGION, "no-such-stream")),
        },
        RefusalCase {
            label: "a-stream-another-region-holds",
            identifiers: json!({ "StreamName": elsewhere }),
            expected_code: "ResourceNotFoundException",
            expected_message: stream_not_found_message(elsewhere),
        },
    ];

    // The rule binds BOTH operations that address a stream by this pair.
    for case in &cases {
        for operation in ["AddTagsToStream", "ListTagsForStream"] {
            let mut body = case.identifiers.clone();
            if operation == "AddTagsToStream" {
                body["Tags"] = json!({ "owner": "team-a" });
            }
            let (status, response) = emulator.call(operation, &body).await;
            assert_eq!(
                (status, error_code(&response).as_str()),
                (400, case.expected_code),
                "case {} ({operation}): expected the modelled {}, got {status} with body \
                 {response}",
                case.label,
                case.expected_code
            );
            assert_eq!(
                error_message(&response),
                case.expected_message,
                "case {} ({operation}): the refusal names what was at fault, got {response}",
                case.label
            );
        }
    }

    // Nothing above persisted a tag anywhere.
    for (region, stream) in [(REGION, name), (REGION, other), ("eu-west-1", elsewhere)] {
        let response = emulator
            .ok_in(
                "refusals-persisted-nothing",
                region,
                "ListTagsForStream",
                &json!({ "StreamName": stream }),
            )
            .await;
        assert!(
            reported_tags("refusals-persisted-nothing", &response).is_empty(),
            "case refusals-persisted-nothing: {stream} in {region} must carry no tag, got \
             {response}"
        );
    }
}
