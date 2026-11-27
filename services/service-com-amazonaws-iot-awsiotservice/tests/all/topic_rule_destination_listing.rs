// SPDX-License-Identifier: CC0-1.0

//! `ListTopicRuleDestinations` end to end over the real `MultiServiceRouter`, with the IoT
//! control plane and the IoT data plane mounted on one engine.
//!
//! The rule under test, in one sentence: the listing reports ONE PAGE of every topic rule
//! destination the account holds, in ARN order, each entry projected from the same persisted
//! record `GetTopicRuleDestination` reads, with an opaque `nextToken` present exactly when
//! the listing is truncated — "Lists all the topic rule destinations in your Amazon Web
//! Services account", `maxResults` "The maximum number of results to return at one time" and
//! `nextToken` "The token to use to get the next set of results, or null if there are no
//! additional results".
//!
//! The tables span the class rather than one recorded request:
//!
//! * the WHOLE-LISTING table varies how many destinations the account holds (none, one, a
//!   handful) and asserts the order, the completeness and the projection of every entry
//!   against `GetTopicRuleDestination`, member for member;
//! * the PAGING table varies the page size against the number of destinations held — a page
//!   of one, a page smaller than the listing, a page exactly its size, a page larger than it
//!   and the modelled maximum — and walks the pages a client walks, asserting they
//!   concatenate to the whole listing with no row repeated or skipped;
//! * the STATUS table drives the destinations through the lifecycle (`IN_PROGRESS`, the rest
//!   at `DISABLED` the clock owes after a confirmation, and `ENABLED`) and asserts the
//!   listing reports the status each one CURRENTLY holds;
//! * the refusal table varies the way a continuation token can fail to be one of this
//!   listing's, and the page sizes outside the modelled range, including each published
//!   bound and one past it.
//!
//! Nothing is mocked and no socket is opened: a destination's confirmation challenge is
//! delivered to an endpoint this emulator serves itself (a retained `Publish` of the IoT data
//! plane), and the token that confirms it is read back out of that endpoint's own
//! `GetRetainedMessage`.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_iot_awsiotservice::state::topic_rule_destinations::{
    CONFIRMATION_SENT_REASON, DESTINATION_CONFIRMATION_MESSAGE_TYPE, DESTINATION_DISABLED,
    DESTINATION_ENABLED, DESTINATION_IN_PROGRESS,
};
use service_com_amazonaws_iot_awsiotservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region every call below is signed for.
const REGION: &str = "us-east-1";

/// `InvalidRequestException` as this service NAMES it in `x-amzn-errortype`: the shape name,
/// a `:`, and the service's API doc namespace.
const QUALIFIED_INVALID_REQUEST: &str =
    "InvalidRequestException:http://iot.amazonaws.com/doc/2015-10-02/";

/// The one sentence this listing refuses a continuation token of another listing with.
const NOT_A_TOKEN_OF_THIS_LISTING: &str = "nextToken is not a continuation token of this listing";

/// How long a condition another task brings about is polled for before a case fails: far
/// longer than anything this emulator takes, so a loaded machine does not fail a case.
const DEADLINE: Duration = Duration::from_secs(20);

/// How often that condition is looked at again.
const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// `TopicRuleDestinationMaxResults`'s published bounds (`range 1..1000`), from
/// `smithy_query(query="operation",operation="ListTopicRuleDestinations")`.
const MAX_RESULTS_BOUNDS: (i64, i64) = (1, 1_000);

// ---------------------------------------------------------------------------
// The emulator
// ---------------------------------------------------------------------------

/// One HTTP answer, reduced to what a client branches on.
#[derive(Debug)]
struct Reply {
    status: u16,
    error_code: Option<String>,
    body: Value,
}

impl Reply {
    /// The error message a modelled failure carries.
    fn message(&self) -> String {
        self.body
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }

    /// The `arn` of the `topicRuleDestination` a create or a read reports.
    fn arn(&self) -> String {
        self.body["topicRuleDestination"]["arn"]
            .as_str()
            .unwrap_or_default()
            .to_string()
    }

    /// The `status` of the `topicRuleDestination` a create or a read reports.
    fn status_value(&self) -> String {
        self.body["topicRuleDestination"]["status"]
            .as_str()
            .unwrap_or_default()
            .to_string()
    }

    /// The `destinationSummaries` a listing reports, or `None` when the answer carries no
    /// such member at all.
    fn summaries(&self) -> Option<&Vec<Value>> {
        self.body.get("destinationSummaries")?.as_array()
    }

    /// The ARNs a listing reports, in the order it reported them.
    fn listed_arns(&self) -> Vec<String> {
        self.summaries()
            .map(|summaries| {
                summaries
                    .iter()
                    .map(|summary| summary["arn"].as_str().unwrap_or_default().to_string())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The `nextToken` a listing reports, or `None` when there are no additional results.
    fn next_token(&self) -> Option<String> {
        self.body
            .get("nextToken")
            .and_then(Value::as_str)
            .map(str::to_string)
    }
}

/// The emulator under test: the IoT control plane and the IoT data plane on ONE engine, so a
/// destination's confirmation challenge is delivered to an endpoint this emulator serves.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        router.register_service(
            service_com_amazonaws_iotdataplane_iotmoonrakerservice::EmulatedService::new(engine),
        );
        Self { router }
    }

    /// Dispatch one request signed for `signing_name`, answered with whatever the service
    /// said.
    async fn send(&self, method: &str, uri: &str, signing_name: &str, body: Vec<u8>) -> Reply {
        let request = http::Request::builder()
            .method(method)
            .uri(uri)
            .header("host", format!("{signing_name}.{REGION}.amazonaws.com"))
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDTESTIOT/20260101/{REGION}/{signing_name}/\
                     aws4_request, SignedHeaders=host, Signature=0000"
                ),
            )
            .header("content-type", "application/json")
            .body(bytes::Bytes::from(body))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let error_code = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let bytes = response.into_body();
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        Reply {
            status,
            error_code,
            body,
        }
    }

    /// One IoT control-plane call (restJson1 over the `iot` signing name).
    async fn iot(&self, method: &str, path: &str, body: Option<&Value>) -> Reply {
        let payload = body.map_or_else(Vec::new, |value| {
            serde_json::to_vec(value).expect("a request body serializes")
        });
        self.send(
            method,
            &format!("https://iot.{REGION}.amazonaws.com{path}"),
            "iot",
            payload,
        )
        .await
    }

    /// `CreateTopicRuleDestination` of an HTTP destination for `confirmation_url`, asserted
    /// to have been created.
    async fn create_destination(&self, case: &str, confirmation_url: &str) -> String {
        let created = self
            .iot(
                "POST",
                "/destinations",
                Some(&json!({
                    "destinationConfiguration": {
                        "httpUrlConfiguration": { "confirmationUrl": confirmation_url },
                    },
                })),
            )
            .await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: CreateTopicRuleDestination({confirmation_url}) answered {} {:?}",
            created.status, created.body
        );
        created.arn()
    }

    /// `GetTopicRuleDestination`: `GET /destinations/{arn+}`.
    async fn get_destination(&self, arn: &str) -> Reply {
        self.iot("GET", &format!("/destinations/{arn}"), None).await
    }

    /// `ConfirmTopicRuleDestination`: `GET /confirmdestination/{confirmationToken+}`.
    async fn confirm_destination(&self, token: &str) -> Reply {
        self.iot("GET", &format!("/confirmdestination/{token}"), None)
            .await
    }

    /// `UpdateTopicRuleDestination`: `PATCH /destinations`.
    async fn update_destination(&self, arn: &str, status: &str) -> Reply {
        self.iot(
            "PATCH",
            "/destinations",
            Some(&json!({ "arn": arn, "status": status })),
        )
        .await
    }

    /// `DeleteTopicRuleDestination`: `DELETE /destinations/{arn+}`.
    async fn delete_destination(&self, arn: &str) -> Reply {
        self.iot("DELETE", &format!("/destinations/{arn}"), None)
            .await
    }

    /// `ListTopicRuleDestinations`: `GET /destinations` with `query` appended verbatim.
    async fn list_destinations(&self, query: &str) -> Reply {
        self.iot("GET", &format!("/destinations{query}"), None).await
    }

    /// What the endpoint topic `topic` last received, read through the data plane's own
    /// `GetRetainedMessage`, or `None` while nothing has arrived there.
    async fn received(&self, topic: &str) -> Option<Vec<u8>> {
        let reply = self
            .send(
                "GET",
                &format!("https://iotdata.{REGION}.amazonaws.com/retainedMessage/{topic}"),
                "iotdata",
                Vec::new(),
            )
            .await;
        if reply.status != 200 {
            return None;
        }
        let encoded = reply.body["payload"].as_str()?;
        aws_smithy_types::base64::decode(encoded).ok()
    }

    /// The confirmation token that arrived at the endpoint topic `topic`, polled for until it
    /// is there or a generous deadline passes.
    ///
    /// The challenge goes out AFTER the create that asked for it has answered, so the token
    /// is waited for by looking at the endpoint again rather than by sleeping a fixed time.
    async fn token_within(&self, case: &str, topic: &str) -> String {
        let deadline = SystemTime::now() + DEADLINE;
        loop {
            if let Some(received) = self.received(topic).await {
                let document: Value = serde_json::from_slice(&received).unwrap_or(Value::Null);
                let is_challenge = document["messageType"].as_str()
                    == Some(DESTINATION_CONFIRMATION_MESSAGE_TYPE);
                if let Some(token) = document["confirmationToken"]
                    .as_str()
                    .filter(|_| is_challenge)
                {
                    return token.to_string();
                }
            }
            assert!(
                SystemTime::now() < deadline,
                "case {case:?}: no confirmation challenge arrived at the endpoint topic \
                 {topic:?} within {DEADLINE:?}"
            );
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    }

    /// The destination's status, read again until it is `expected` or a generous deadline
    /// passes; answered with the last status read either way.
    async fn status_within(&self, case: &str, arn: &str, expected: &str) -> String {
        let deadline = SystemTime::now() + DEADLINE;
        loop {
            let read = self.get_destination(arn).await;
            assert_eq!(
                read.status, 200,
                "case {case:?}: GetTopicRuleDestination answered {} {:?}",
                read.status, read.body
            );
            let status = read.status_value();
            if status == expected || SystemTime::now() >= deadline {
                return status;
            }
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    }

    /// The destination's `statusReason`, read again until it is `expected` or a generous
    /// deadline passes; answered with the last reason read either way.
    async fn reason_within(&self, case: &str, arn: &str, expected: &str) -> Option<String> {
        let deadline = SystemTime::now() + DEADLINE;
        loop {
            let read = self.get_destination(arn).await;
            assert_eq!(
                read.status, 200,
                "case {case:?}: GetTopicRuleDestination answered {} {:?}",
                read.status, read.body
            );
            let reason = read.body["topicRuleDestination"]["statusReason"]
                .as_str()
                .map(str::to_string);
            if reason.as_deref() == Some(expected) || SystemTime::now() >= deadline {
                return reason;
            }
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    }

    /// Wait until every destination in `arns` has had its confirmation challenge DISPATCHED.
    ///
    /// The challenge goes out off the response path of the create that asked for it, and its
    /// dispatch is what moves the destination to the phase of the unconfirmed state its
    /// `statusReason` then states, so a case that compares one read with another waits for
    /// that phase first rather than racing it. The endpoint these cases configure is one this
    /// emulator serves and answers 200, so the phase reached is the message having been taken.
    async fn await_challenges(&self, case: &str, arns: &[String]) {
        for arn in arns {
            let reason = self.reason_within(case, arn, CONFIRMATION_SENT_REASON).await;
            assert_eq!(
                reason.as_deref(),
                Some(CONFIRMATION_SENT_REASON),
                "case {case:?}: expected {arn} to report its challenge as taken by the \
                 endpoint, got {reason:?}"
            );
        }
    }

    /// Drive the destination at `arn`, whose endpoint is the topic `topic`, all the way to
    /// `DISABLED`: the token its endpoint received comes back through
    /// `ConfirmTopicRuleDestination`, and the move out of `IN_PROGRESS` the clock then owes is
    /// polled for.
    async fn confirm_to_disabled(&self, case: &str, arn: &str, topic: &str) {
        let token = self.token_within(case, topic).await;
        let confirmed = self.confirm_destination(&token).await;
        assert_eq!(
            confirmed.status, 200,
            "case {case:?}: ConfirmTopicRuleDestination answered {} {:?}",
            confirmed.status, confirmed.body
        );
        let status = self.status_within(case, arn, DESTINATION_DISABLED).await;
        assert_eq!(
            status, DESTINATION_DISABLED,
            "case {case:?}: expected a confirmed destination to rest at \
             {DESTINATION_DISABLED}, got {status:?}"
        );
    }
}

/// The URL of one endpoint this emulator serves itself: a retained `Publish` of the IoT data
/// plane, which records what arrived at it.
fn endpoint_url(topic: &str) -> String {
    format!("https://iotdata.{REGION}.amazonaws.com/topics/{topic}?qos=1&retain=true")
}

/// The summary a listing reported for `arn`, or `None` when it reported none.
fn summary_of<'a>(listed: &'a Reply, arn: &str) -> Option<&'a Value> {
    listed
        .summaries()?
        .iter()
        .find(|summary| summary["arn"].as_str() == Some(arn))
}

/// Assert the summary a listing reports for one destination is the SAME record
/// `GetTopicRuleDestination` reads, member for member.
///
/// `TopicRuleDestinationSummary` and `TopicRuleDestination` name the HTTP member differently
/// (`httpUrlSummary` against `httpUrlProperties`, from
/// `smithy_query(query="shape",shape="TopicRuleDestinationSummary")`), and carry the same
/// `arn`, `status`, `statusReason`, `createdAt` and `lastUpdatedAt`.
fn assert_summary_matches_read(case: &str, summary: &Value, read: &Value) {
    for member in ["arn", "status", "createdAt", "lastUpdatedAt", "statusReason"] {
        assert_eq!(
            summary.get(member),
            read.get(member),
            "case {case:?}: expected the listed {member:?} to be the record \
             GetTopicRuleDestination reads, got {:?} against {:?}",
            summary.get(member),
            read.get(member)
        );
    }
    assert_eq!(
        summary["httpUrlSummary"]["confirmationUrl"], read["httpUrlProperties"]["confirmationUrl"],
        "case {case:?}: expected the listed confirmation URL to be the one the record holds, \
         got {:?} against {:?}",
        summary["httpUrlSummary"], read["httpUrlProperties"]
    );
}

// ---------------------------------------------------------------------------
// The whole listing
// ---------------------------------------------------------------------------

/// RULE: the listing reports EVERY destination the account holds, in ARN order, each entry
/// projected from the record `GetTopicRuleDestination` reads — and no `nextToken` when there
/// is nothing more to report.
///
/// The rows differ along how many destinations the account holds, including none at all.
#[tokio::test]
async fn the_listing_reports_every_destination_the_account_holds_in_arn_order() {
    // (case, how many destinations the account holds)
    let rows: [(&str, usize); 4] = [
        ("an account holding no destination", 0),
        ("an account holding one destination", 1),
        ("an account holding two destinations", 2),
        ("an account holding a handful of destinations", 5),
    ];

    for (case, held) in rows {
        let emulator = Emulator::new();
        let mut created = Vec::new();
        for index in 0..held {
            created.push(
                emulator
                    .create_destination(case, &endpoint_url(&format!("listing-{index}")))
                    .await,
            );
        }
        emulator.await_challenges(case, &created).await;

        let listed = emulator.list_destinations("").await;
        assert_eq!(
            listed.status, 200,
            "case {case:?}: ListTopicRuleDestinations answered {} {:?}",
            listed.status, listed.body
        );
        assert!(
            listed.summaries().is_some(),
            "case {case:?}: expected destinationSummaries to be reported even when the \
             account holds none, got {:?}",
            listed.body
        );

        let mut expected = created.clone();
        expected.sort();
        assert_eq!(
            listed.listed_arns(),
            expected,
            "case {case:?}: expected every created destination reported once in ARN order, \
             got {:?}",
            listed.listed_arns()
        );
        assert_eq!(
            listed.next_token(),
            None,
            "case {case:?}: expected no nextToken when nothing more is to be reported, got \
             {:?}",
            listed.next_token()
        );

        for arn in &created {
            let read = emulator.get_destination(arn).await;
            assert_eq!(
                read.status, 200,
                "case {case:?}: GetTopicRuleDestination({arn}) answered {} {:?}",
                read.status, read.body
            );
            let summary = summary_of(&listed, arn)
                .unwrap_or_else(|| panic!("case {case:?}: {arn} was not listed: {:?}", listed.body));
            assert_summary_matches_read(case, summary, &read.body["topicRuleDestination"]);
        }
    }
}

/// RULE: a reporting call changes nothing a later caller sees — two listings of an unchanged
/// account report the same destinations in the same order.
#[tokio::test]
async fn listing_twice_reports_the_same_destinations() {
    let case = "an unchanged account listed twice";
    let emulator = Emulator::new();
    let mut created = Vec::new();
    for index in 0..3 {
        created.push(
            emulator
                .create_destination(case, &endpoint_url(&format!("stable-{index}")))
                .await,
        );
    }
    emulator.await_challenges(case, &created).await;

    let first = emulator.list_destinations("").await;
    let second = emulator.list_destinations("").await;
    assert_eq!(
        first.listed_arns(),
        second.listed_arns(),
        "case {case:?}: expected a listing to change nothing, got {:?} then {:?}",
        first.listed_arns(),
        second.listed_arns()
    );
    assert_eq!(
        first.summaries(),
        second.summaries(),
        "case {case:?}: expected the same records reported twice, got {:?} then {:?}",
        first.summaries(),
        second.summaries()
    );
}

// ---------------------------------------------------------------------------
// Paging
// ---------------------------------------------------------------------------

/// Walk the whole listing one page at a time, as a client walks it: every ARN each page
/// reported, in order, with each page asserted to be no longer than `max_results` and to
/// carry a `nextToken` only while more remains.
async fn walk_pages(case: &str, emulator: &Emulator, max_results: i64) -> Vec<String> {
    let mut walked: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    // A page carries at least one row, so the walk cannot take more steps than the store's
    // ceiling (`MAX_TOPIC_RULE_DESTINATIONS`) allows rows.
    for _ in 0..=1_000 {
        let query = match token.as_deref() {
            Some(token) => format!("?maxResults={max_results}&nextToken={token}"),
            None => format!("?maxResults={max_results}"),
        };
        let page = emulator.list_destinations(&query).await;
        assert_eq!(
            page.status, 200,
            "case {case:?}: ListTopicRuleDestinations({query}) answered {} {:?}",
            page.status, page.body
        );
        let arns = page.listed_arns();
        assert!(
            arns.len() as i64 <= max_results,
            "case {case:?}: expected at most {max_results} entries on a page, got {}",
            arns.len()
        );
        walked.extend(arns);
        match page.next_token() {
            Some(next) => token = Some(next),
            None => return walked,
        }
    }
    panic!("case {case:?}: the paged walk did not terminate");
}

/// RULE: a page carries at most `maxResults` entries and its `nextToken` resumes the listing
/// after it, so the pages of one listing concatenate to the whole listing with no entry
/// repeated or skipped.
///
/// The rows differ along the page size against the number of destinations held: a page of
/// one, a page smaller than the listing, a page exactly its size, a page larger than it, and
/// the modelled maximum.
#[tokio::test]
async fn the_pages_of_one_listing_concatenate_to_the_whole_listing() {
    // (case, how many destinations the account holds, the page size asked for)
    let rows: [(&str, usize, i64); 6] = [
        ("a page of one over five destinations", 5, 1),
        ("a page smaller than the listing", 5, 2),
        ("a page exactly the size of the listing", 5, 5),
        ("a page larger than the listing", 5, 8),
        ("the modelled maximum page size", 5, MAX_RESULTS_BOUNDS.1),
        ("a page of one over a single destination", 1, 1),
    ];

    for (case, held, max_results) in rows {
        let emulator = Emulator::new();
        let mut created = Vec::new();
        for index in 0..held {
            created.push(
                emulator
                    .create_destination(case, &endpoint_url(&format!("paged-{index}")))
                    .await,
            );
        }
        created.sort();

        let walked = walk_pages(case, &emulator, max_results).await;
        assert_eq!(
            walked, created,
            "case {case:?}: expected the pages to concatenate to the whole listing in ARN \
             order, got {walked:?}"
        );
    }
}

/// RULE: the `nextToken` of a truncated page is present, opaque and resumes AFTER the page it
/// closed — so the entry that page ended on is not reported again.
#[tokio::test]
async fn a_trailing_token_resumes_after_the_page_it_closed() {
    let case = "a page of two over three destinations";
    let emulator = Emulator::new();
    let mut created = Vec::new();
    for index in 0..3 {
        created.push(
            emulator
                .create_destination(case, &endpoint_url(&format!("resume-{index}")))
                .await,
        );
    }
    created.sort();

    let first = emulator.list_destinations("?maxResults=2").await;
    assert_eq!(
        first.listed_arns(),
        created[..2].to_vec(),
        "case {case:?}: expected the first page to be the first two destinations, got {:?}",
        first.listed_arns()
    );
    let token = first.next_token().unwrap_or_else(|| {
        panic!(
            "case {case:?}: expected a truncated page to carry a nextToken, got {:?}",
            first.body
        )
    });
    assert!(
        !token.contains(&created[1]),
        "case {case:?}: expected an opaque continuation token, got {token:?}"
    );

    let second = emulator
        .list_destinations(&format!("?maxResults=2&nextToken={token}"))
        .await;
    assert_eq!(
        second.listed_arns(),
        created[2..].to_vec(),
        "case {case:?}: expected the second page to resume after the first, got {:?}",
        second.listed_arns()
    );
    assert_eq!(
        second.next_token(),
        None,
        "case {case:?}: expected the last page to carry no nextToken, got {:?}",
        second.next_token()
    );
}

// ---------------------------------------------------------------------------
// The status each destination currently holds
// ---------------------------------------------------------------------------

/// RULE: the listing reports the status each destination CURRENTLY holds, including the move
/// out of `IN_PROGRESS` the clock owes a confirmed destination — the same status
/// `GetTopicRuleDestination` reports for it.
///
/// The rows differ along how far through the lifecycle each destination has been driven:
/// created and unconfirmed, confirmed and resting, and enabled by its owner
/// (`TopicRuleDestinationStatus`).
#[tokio::test]
async fn the_listing_reports_the_status_each_destination_currently_holds() {
    let case = "destinations at every status of the lifecycle";
    let emulator = Emulator::new();

    let unconfirmed = emulator
        .create_destination(case, &endpoint_url("status-in-progress"))
        .await;
    let resting = emulator
        .create_destination(case, &endpoint_url("status-disabled"))
        .await;
    let enabled = emulator
        .create_destination(case, &endpoint_url("status-enabled"))
        .await;

    emulator
        .await_challenges(case, std::slice::from_ref(&unconfirmed))
        .await;
    emulator
        .confirm_to_disabled(case, &resting, "status-disabled")
        .await;
    emulator
        .confirm_to_disabled(case, &enabled, "status-enabled")
        .await;
    let updated = emulator
        .update_destination(&enabled, DESTINATION_ENABLED)
        .await;
    assert_eq!(
        updated.status, 200,
        "case {case:?}: UpdateTopicRuleDestination answered {} {:?}",
        updated.status, updated.body
    );

    let listed = emulator.list_destinations("").await;
    for (arn, expected) in [
        (&unconfirmed, DESTINATION_IN_PROGRESS),
        (&resting, DESTINATION_DISABLED),
        (&enabled, DESTINATION_ENABLED),
    ] {
        let summary = summary_of(&listed, arn)
            .unwrap_or_else(|| panic!("case {case:?}: {arn} was not listed: {:?}", listed.body));
        assert_eq!(
            summary["status"],
            json!(expected),
            "case {case:?}: expected {arn} to be listed {expected}, got {summary:?}"
        );
        let read = emulator.get_destination(arn).await;
        assert_summary_matches_read(case, summary, &read.body["topicRuleDestination"]);
    }
}

/// RULE: the listing is the store, so a destination `DeleteTopicRuleDestination` removed is
/// no longer one of the account's destinations.
#[tokio::test]
async fn a_deleted_destination_is_no_longer_listed() {
    let case = "one of three destinations deleted";
    let emulator = Emulator::new();
    let mut created = Vec::new();
    for index in 0..3 {
        created.push(
            emulator
                .create_destination(case, &endpoint_url(&format!("deleted-{index}")))
                .await,
        );
    }
    created.sort();

    let removed = created.remove(1);
    let deleted = emulator.delete_destination(&removed).await;
    assert_eq!(
        deleted.status, 200,
        "case {case:?}: DeleteTopicRuleDestination answered {} {:?}",
        deleted.status, deleted.body
    );

    let listed = emulator.list_destinations("").await;
    assert_eq!(
        listed.listed_arns(),
        created,
        "case {case:?}: expected the deleted destination to be gone from the listing, got {:?}",
        listed.listed_arns()
    );
}

// ---------------------------------------------------------------------------
// Refusals
// ---------------------------------------------------------------------------

/// RULE: a `nextToken` that is not one THIS listing issued is refused
/// `InvalidRequestException` (HTTP 400, "The request is not valid") and lists nothing.
///
/// The rows differ along the way a token can fail to be one of this listing's: a bare word,
/// an odd-length encoding, an encoding of bytes carrying no resume point, an encoding that
/// names another listing, and the empty token.
#[tokio::test]
async fn a_next_token_of_another_listing_is_refused() {
    let emulator = Emulator::new();
    emulator
        .create_destination("a seeded account", &endpoint_url("refused-token"))
        .await;

    // (case, the token as the request sends it)
    let rows: [(&str, &str); 6] = [
        ("a bare word", "not-a-token"),
        ("an odd-length hex encoding", "abc"),
        // The hex of `topicRules\u{1f}rule-one`: a resume point of ANOTHER of this
        // service's listings.
        (
            "a token naming another listing",
            "746f70696352756c65731f72756c652d6f6e65",
        ),
        // The hex of `arn:aws`: bytes that decode, with no listing scope in front of them.
        ("an encoding carrying no listing scope", "61726e3a617773"),
        ("the empty token", ""),
        ("a token of unprintable bytes", "00ff00ff"),
    ];

    for (case, token) in rows {
        let refused = emulator
            .list_destinations(&format!("?nextToken={token}"))
            .await;
        assert_eq!(
            (refused.status, refused.error_code.as_deref()),
            (400, Some(QUALIFIED_INVALID_REQUEST)),
            "case {case:?}: expected 400 InvalidRequestException, got {} {:?} with {:?}",
            refused.status,
            refused.error_code,
            refused.body
        );
        assert_eq!(
            refused.message(),
            NOT_A_TOKEN_OF_THIS_LISTING,
            "case {case:?}: expected the refusal to say the token is not this listing's, got \
             {:?}",
            refused.message()
        );
        assert!(
            refused.summaries().is_none(),
            "case {case:?}: a refused request lists nothing, got {:?}",
            refused.body
        );
    }
}

/// RULE: `maxResults` outside its published range (`range 1..1000`) is refused at the
/// boundary and lists nothing, while each published bound is served.
#[tokio::test]
async fn a_page_size_outside_the_modelled_range_is_refused() {
    let emulator = Emulator::new();
    emulator
        .create_destination("a seeded account", &endpoint_url("refused-page-size"))
        .await;

    let (low, high) = MAX_RESULTS_BOUNDS;
    // (case, the page size asked for, the constraint the refusal names) — each published
    // bound, and one past each of them.
    let refused_rows: [(&str, i64, String); 3] = [
        (
            "one below the published minimum",
            low - 1,
            format!("Member must have value greater than or equal to {low}"),
        ),
        (
            "far below the published minimum",
            -1,
            format!("Member must have value greater than or equal to {low}"),
        ),
        (
            "one past the published maximum",
            high + 1,
            format!("Member must have value less than or equal to {high}"),
        ),
    ];
    for (case, max_results, constraint) in refused_rows {
        let refused = emulator
            .list_destinations(&format!("?maxResults={max_results}"))
            .await;
        assert_eq!(
            (refused.status, refused.error_code.as_deref()),
            (400, Some(QUALIFIED_INVALID_REQUEST)),
            "case {case:?}: expected a page size outside the modelled range to be refused \
             400 InvalidRequestException, got {} {:?} with {:?}",
            refused.status,
            refused.error_code,
            refused.body
        );
        assert_eq!(
            refused.message(),
            format!(
                "1 validation error detected: Value at 'maxResults' failed to satisfy \
                 constraint: {constraint}"
            ),
            "case {case:?}: expected the refusal to name the constraint maxResults failed, \
             got {:?}",
            refused.message()
        );
        assert!(
            refused.summaries().is_none(),
            "case {case:?}: a refused request lists nothing, got {:?}",
            refused.body
        );
    }

    for (case, max_results) in [
        ("the published minimum", low),
        ("the published maximum", high),
    ] {
        let listed = emulator
            .list_destinations(&format!("?maxResults={max_results}"))
            .await;
        assert_eq!(
            listed.status, 200,
            "case {case:?}: expected a page size at the published bound to be served, got {} \
             {:?}",
            listed.status, listed.body
        );
        assert_eq!(
            listed.listed_arns().len(),
            1,
            "case {case:?}: expected the account's one destination reported, got {:?}",
            listed.body
        );
    }
}
