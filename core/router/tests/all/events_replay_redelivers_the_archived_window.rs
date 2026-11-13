// SPDX-License-Identifier: CC0-1.0

//! An EventBridge replay redelivers the archived events its window selects to the
//! destination bus's rule targets, over the WHOLE mounted service set.
//!
//! The RULE under test: **a `StartReplay` over an archive re-publishes onto
//! `Destination.Arn` exactly those archived events whose OWN event time lies in
//! `[EventStartTime, EventEndTime]`, routes each of them by the destination bus's rules
//! (confined to `Destination.FilterArns` when the destination carries them), stamps each
//! re-published envelope with `replay-name` equal to the `ReplayName` while leaving the
//! archived `detail`, `source` and `detail-type` unchanged, and drives the replay to
//! `COMPLETED`, which `DescribeReplay` reports.** A live delivery carries no `replay-name`.
//!
//! This runs the path a client runs it over: EventBridge archives, selects and re-publishes,
//! SQS holds what arrived, and both are reached through the WHOLE emulator — the same
//! `build_router` service set the binary serves — so the redelivery is claimed and routed by
//! whichever mounted model a client's `events`-scoped call reaches.
//!
//! The rows differ along where the window falls relative to the archived events' own times
//! (around the newest pair only, around all of them, ending ON an event's own time, and one
//! second past it), so what each row asserts is WHICH events the window selects — the
//! recorded case (two events in, one out) is one row, and the row whose window selects
//! nothing reads the queue all the same. Every row also carries a SECOND rule of the same
//! bus with the identical pattern that the replay's `FilterArns` do not name, whose queue
//! must hold no redelivery and must still take live events afterwards: the negative half of
//! the same rule.
//!
//! What only this suite can reach is the MOUNT. The window table, the `FilterArns` scoping
//! and the `replay-name` stamp are also pinned inside the owning crate by
//! `services/service-com-amazonaws-eventbridge-awsevents/tests/all/archive_replay.rs`, which
//! mounts EventBridge and Amazon SQS as a hand-picked pair; the table here is kept to the
//! rows that span the selection rule, and what it adds is that the whole redelivery runs over
//! the service set a client's own `events`-scoped call reaches, where several mounted models
//! resolve to the `events` key.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The Region every request below is signed for.
const REGION: &str = "us-west-2";

/// The `source` and `detail-type` every event below carries, and the rule's pattern matches
/// on.
const SOURCE: &str = "com.demo.replay";
const DETAIL_TYPE: &str = "Replayable";

/// The service principal EventBridge delivers to an SQS target as, which is what a queue's
/// resource policy admits ("for Amazon SQS queues, EventBridge relies on resource-based
/// policies").
const EVENTS_PRINCIPAL: &str = "events.amazonaws.com";

/// The envelope field a REPLAYED event carries and a live delivery does not. Spelled out
/// here rather than imported from the service crate, so a wrong constant in the
/// implementation cannot make this suite pass.
const REPLAY_NAME: &str = "replay-name";

/// How long a row waits for a condition the clock owes — a delivery to arrive, a replay to
/// settle — before it gives up: far above the span either takes, and driven off the
/// condition rather than off a fixed sleep, so a slow machine delays a row instead of
/// breaking it.
const DEADLINE: Duration = Duration::from_secs(60);

/// One poll inside [`DEADLINE`].
const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// The whole emulator over one engine, exactly as `core/router/src/lib.rs` assembles it for
/// the binary.
fn router() -> MultiServiceRouter {
    let engine = aws_emulator_router::new_engine();
    let mut router = aws_emulator_router::build_router(Arc::clone(&engine));
    router.set_request_authentication(false);
    router
}

/// A SigV4 `Authorization` whose credential scope names `service`, which is what routes the
/// request. The signature is never checked: the router above runs with request
/// authentication off.
fn authorization(service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{service}/aws4_request, \
         SignedHeaders=host, Signature=0000"
    )
}

/// The status and parsed body of one response.
fn answered(response: &http::Response<Bytes>) -> (u16, Value) {
    let status = response.status().as_u16();
    let parsed = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// One `X-Amz-Target` RPC call against the service whose signing name is `signing_name`.
async fn call(
    router: &MultiServiceRouter,
    signing_name: &str,
    target_prefix: &str,
    json_version: &str,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("{signing_name}.{REGION}.amazonaws.com"))
        .header("authorization", authorization(signing_name))
        .header("content-type", format!("application/x-amz-json-{json_version}"))
        .header("x-amz-target", format!("{target_prefix}.{operation}"))
        .body(Bytes::from(body.to_string()))
        .expect("the test request is well formed");
    answered(&router.dispatch(request).await)
}

/// One call that must answer 200.
async fn ok(
    router: &MultiServiceRouter,
    case: &str,
    signing_name: &str,
    target_prefix: &str,
    json_version: &str,
    operation: &str,
    body: &Value,
) -> Value {
    let (status, parsed) =
        call(router, signing_name, target_prefix, json_version, operation, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from {signing_name} {operation}, got {status} with \
         {parsed}"
    );
    parsed
}

/// One EventBridge call (`awsJson1_1`, `AWSEvents.<Operation>`).
async fn events(router: &MultiServiceRouter, case: &str, operation: &str, body: &Value) -> Value {
    ok(router, case, "events", "AWSEvents", "1.1", operation, body).await
}

/// One Amazon SQS call (`awsJson1_0`, `AmazonSQS.<Operation>`).
async fn sqs(router: &MultiServiceRouter, case: &str, operation: &str, body: &Value) -> Value {
    ok(router, case, "sqs", "AmazonSQS", "1.0", operation, body).await
}

/// A string member of a response, or a panic naming the case and what came back.
fn member(case: &str, body: &Value, pointer: &str) -> String {
    body.pointer(pointer)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: expected {pointer} in {body}"))
        .to_string()
}

/// An SQS queue called `name`, created through SQS itself; answers its url and the ARN a
/// rule names it by.
async fn queue(router: &MultiServiceRouter, case: &str, name: &str) -> (String, String) {
    let created = sqs(router, case, "CreateQueue", &json!({"QueueName": name})).await;
    let url = member(case, &created, "/QueueUrl");
    let attributes = sqs(
        router,
        case,
        "GetQueueAttributes",
        &json!({"QueueUrl": &url, "AttributeNames": ["QueueArn"]}),
    )
    .await;
    let arn = member(case, &attributes, "/Attributes/QueueArn");
    (url, arn)
}

/// Let EventBridge send to the queue at `url` on behalf of the rule `rule_arn`, the way a
/// client admits a rule to its queue: a resource policy naming EventBridge's service
/// principal, scoped to that rule.
async fn admit_events(
    router: &MultiServiceRouter,
    case: &str,
    url: &str,
    queue_arn: &str,
    rule_arn: &str,
) {
    let policy = json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Sid": "AllowEventBridge",
            "Effect": "Allow",
            "Principal": {"Service": EVENTS_PRINCIPAL},
            "Action": "sqs:SendMessage",
            "Resource": queue_arn,
            "Condition": {"ArnEquals": {"aws:SourceArn": rule_arn}},
        }],
    })
    .to_string();
    sqs(
        router,
        case,
        "SetQueueAttributes",
        &json!({"QueueUrl": url, "Attributes": {"Policy": policy}}),
    )
    .await;
}

/// Every envelope now waiting on the queue at `url`.
async fn drain(router: &MultiServiceRouter, case: &str, url: &str) -> Vec<Value> {
    let received = sqs(
        router,
        case,
        "ReceiveMessage",
        &json!({"QueueUrl": url, "MaxNumberOfMessages": 10}),
    )
    .await;
    let messages = received["Messages"].as_array().cloned().unwrap_or_default();
    for message in &messages {
        let handle = member(case, message, "/ReceiptHandle");
        sqs(
            router,
            case,
            "DeleteMessage",
            &json!({"QueueUrl": url, "ReceiptHandle": handle}),
        )
        .await;
    }
    messages
        .iter()
        .map(|message| {
            let raw = message["Body"].as_str().unwrap_or_default();
            serde_json::from_str::<Value>(raw).unwrap_or_else(|error| {
                panic!("case {case:?}: a delivered body is not JSON ({error}): {raw:?}")
            })
        })
        .collect()
}

/// Collect envelopes off the queue at `url` until `wanted` of them have arrived or
/// [`DEADLINE`] passes, and then ONCE MORE; answers everything that did arrive.
///
/// The queue is read at least twice however small `wanted` is — a `wanted` of none reads
/// it twice and nothing else — so an envelope that must NOT be there is read rather than
/// assumed absent, and an extra one arriving after the count was reached is part of what
/// the caller compares instead of being left on the queue.
async fn collect(router: &MultiServiceRouter, case: &str, url: &str, wanted: usize) -> Vec<Value> {
    let started = std::time::Instant::now();
    let mut arrived: Vec<Value> = Vec::new();
    loop {
        arrived.extend(drain(router, case, url).await);
        if arrived.len() >= wanted || started.elapsed() >= DEADLINE {
            break;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    arrived.extend(drain(router, case, url).await);
    arrived
}

/// Epoch seconds now, which is what an `awsJson1_1` time stamp carries.
fn epoch_now() -> i64 {
    aws_smithy_types::DateTime::from(std::time::SystemTime::now()).secs()
}

/// Publish one event per entry of `details` onto `bus`, each carrying its own `Time`.
async fn publish(
    router: &MultiServiceRouter,
    case: &str,
    bus: &str,
    entries: &[(i64, &str)],
) -> Value {
    let entries: Vec<Value> = entries
        .iter()
        .map(|(time, nonce)| {
            json!({
                "EventBusName": bus,
                "Source": SOURCE,
                "DetailType": DETAIL_TYPE,
                "Detail": json!({"nonce": nonce}).to_string(),
                "Time": time,
            })
        })
        .collect();
    let published = events(router, case, "PutEvents", &json!({"Entries": entries})).await;
    assert_eq!(
        published["FailedEntryCount"],
        json!(0),
        "case {case:?}: every published entry must be accepted, got {published}"
    );
    published
}

/// The `detail.nonce` of each envelope, sorted: nothing here depends on the order a standard
/// queue hands messages over in, only on WHICH events arrived.
fn nonces(envelopes: &[Value]) -> Vec<String> {
    let mut found: Vec<String> = envelopes
        .iter()
        .map(|envelope| {
            envelope["detail"]["nonce"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect();
    found.sort();
    found
}

/// The replay `name`, once `DescribeReplay` reports it in a terminal state.
async fn settled_replay(router: &MultiServiceRouter, case: &str, name: &str) -> Value {
    let started = std::time::Instant::now();
    loop {
        let described = events(router, case, "DescribeReplay", &json!({"ReplayName": name})).await;
        let state = described["State"].as_str().unwrap_or_default().to_string();
        if matches!(state.as_str(), "COMPLETED" | "CANCELLED" | "FAILED") {
            return described;
        }
        assert!(
            started.elapsed() < DEADLINE,
            "case {case:?}: expected the replay to settle, it is still {state:?} up to the \
             deadline: {described}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// One ENABLED rule called `name` on `bus` matching every event published below, with one
/// SQS queue as its only target; answers the rule's ARN and that queue's url. The queue
/// admits EventBridge on behalf of this rule alone, the way a client admits a rule to its
/// queue.
async fn given_rule(
    router: &MultiServiceRouter,
    case: &str,
    bus: &str,
    name: &str,
) -> (String, String) {
    let (url, queue_arn) = queue(router, case, name).await;
    let put = events(
        router,
        case,
        "PutRule",
        &json!({
            "Name": name,
            "EventBusName": bus,
            "EventPattern": json!({"source": [SOURCE]}).to_string(),
            "State": "ENABLED",
        }),
    )
    .await;
    let rule_arn = member(case, &put, "/RuleArn");
    admit_events(router, case, &url, &queue_arn, &rule_arn).await;
    let targeted = events(
        router,
        case,
        "PutTargets",
        &json!({
            "Rule": name,
            "EventBusName": bus,
            "Targets": [{"Id": "t1", "Arn": &queue_arn}],
        }),
    )
    .await;
    assert_eq!(
        targeted["FailedEntryCount"],
        json!(0),
        "case {case:?}: the queue target of {name} must be accepted, got {targeted}"
    );
    (rule_arn, url)
}

/// How far before now each archived event's own time lies, in seconds.
const OLDEST: i64 = -7_200;
const OLDER: i64 = -3_600;
const NEWER: i64 = -3_590;

/// One row: the case, the replay window as offsets from now, and the nonces the rule's queue
/// must receive from the replay.
struct Row {
    case: &'static str,
    window: (i64, i64),
    expected: &'static [&'static str],
}

/// RULE: a replay re-publishes exactly the archived events whose own time lies in its
/// window, stamped with its `ReplayName`, to the rules its `FilterArns` name.
#[tokio::test]
async fn a_replay_redelivers_only_the_archived_events_inside_its_window() {
    let rows = [
        Row {
            // The recorded case: a band around the two newest events, which leaves the
            // oldest one out.
            case: "a ten minute band around the two newest events",
            window: (OLDER - 300, NEWER + 300),
            expected: &["in-a", "in-b"],
        },
        Row {
            case: "a window around every archived event",
            window: (OLDEST - 300, NEWER + 300),
            expected: &["in-a", "in-b", "out"],
        },
        Row {
            // "Only events that occurred between the EventStartTime and EventEndTime are
            // replayed": an event AT a bound occurred between them.
            case: "a window ending on the oldest event's own time",
            window: (OLDEST - 300, OLDEST),
            expected: &["out"],
        },
        Row {
            // One second past that bound selects nothing at all.
            case: "a window ending one second before the oldest event",
            window: (OLDEST - 300, OLDEST - 1),
            expected: &[],
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        let router = router();
        let anchor = epoch_now();

        // The bus and TWO rules carrying the identical pattern, each with its own queue: the
        // replay below names only the first in `Destination.FilterArns`, so the second is the
        // negative half of the same rule — "a list of ARNs for rules to replay events to".
        let bus = format!("replay-bus-{index}");
        let created = events(&router, case, "CreateEventBus", &json!({"Name": &bus})).await;
        let bus_arn = member(case, &created, "/EventBusArn");
        let (rule_arn, url) =
            given_rule(&router, case, &bus, &format!("replay-named-{index}")).await;
        let (_, unnamed_url) =
            given_rule(&router, case, &bus, &format!("replay-unnamed-{index}")).await;

        // The live path is settled BEFORE the archive exists, the way the recorded client
        // settles it: one warm-up event has to reach BOTH queues.
        publish(&router, case, &bus, &[(anchor, "warm-up")]).await;
        for (which, queue_url) in [("named", &url), ("unnamed", &unnamed_url)] {
            let warmed = collect(&router, case, queue_url, 1).await;
            assert_eq!(
                nonces(&warmed),
                vec!["warm-up".to_string()],
                "case {case:?}: the {which} rule's delivery path has to be alive before the \
                 archive exists, got {warmed:?}"
            );
        }

        // The archive of the whole bus: "if you do not specify a pattern to filter events
        // sent to the archive, all events are sent to the archive".
        let archive = format!("replay-archive-{index}");
        let archived = events(
            &router,
            case,
            "CreateArchive",
            &json!({"ArchiveName": &archive, "EventSourceArn": &bus_arn}),
        )
        .await;
        let archive_arn = member(case, &archived, "/ArchiveArn");
        let described = events(
            &router,
            case,
            "DescribeArchive",
            &json!({"ArchiveName": &archive}),
        )
        .await;
        assert_eq!(
            described["State"],
            json!("ENABLED"),
            "case {case:?}: the archive has to be capturing before any event is published, got \
             {described}"
        );

        // The three events the rows select from: two at the newest pair's times, one older.
        let entries = [
            (anchor + OLDER, "in-a"),
            (anchor + NEWER, "in-b"),
            (anchor + OLDEST, "out"),
        ];
        publish(&router, case, &bus, &entries).await;

        // The LIVE copies, which carry no replay name, are taken off both queues first, so
        // what the row reads afterwards is what the REPLAY delivered.
        for (which, queue_url) in [("named", &url), ("unnamed", &unnamed_url)] {
            let live = collect(&router, case, queue_url, entries.len()).await;
            assert_eq!(
                nonces(&live),
                vec![
                    "in-a".to_string(),
                    "in-b".to_string(),
                    "out".to_string()
                ],
                "case {case:?}: every published event reaches the {which} rule's queue live, \
                 got {live:?}"
            );
            for envelope in &live {
                assert!(
                    envelope.get(REPLAY_NAME).is_none(),
                    "case {case:?}: a LIVE delivery carries no {REPLAY_NAME}, got {envelope}"
                );
            }
        }

        // What the archive holds, read back the way a client confirms ingestion.
        let described = events(
            &router,
            case,
            "DescribeArchive",
            &json!({"ArchiveName": &archive}),
        )
        .await;
        assert_eq!(
            described["EventCount"],
            json!(entries.len()),
            "case {case:?}: the archive has to hold the published events, got {described}"
        );
        assert!(
            described["SizeBytes"].as_i64().unwrap_or_default() > 0,
            "case {case:?}: an archive holding events holds bytes, got {described}"
        );

        // The replay itself, confined to the one rule of the destination bus.
        let replay = format!("replay-{index}");
        let started = events(
            &router,
            case,
            "StartReplay",
            &json!({
                "ReplayName": &replay,
                "EventSourceArn": &archive_arn,
                "EventStartTime": anchor + row.window.0,
                "EventEndTime": anchor + row.window.1,
                "Destination": {"Arn": &bus_arn, "FilterArns": [&rule_arn]},
            }),
        )
        .await;
        assert!(
            started["ReplayArn"]
                .as_str()
                .unwrap_or_default()
                .ends_with(&format!(":replay/{replay}")),
            "case {case:?}: StartReplay answers the replay's own ARN, got {started}"
        );

        let settled = settled_replay(&router, case, &replay).await;
        assert_eq!(
            settled["State"],
            json!("COMPLETED"),
            "case {case:?}: a replay nothing cancelled settles COMPLETED, got {settled}"
        );
        assert_eq!(
            settled["Destination"]["FilterArns"],
            json!([&rule_arn]),
            "case {case:?}: DescribeReplay echoes the destination it was started with, got \
             {settled}"
        );

        // What the replay redelivered: exactly the events its window selected, each carrying
        // the replay's name and the archived detail, source and detail type unchanged. The
        // queue is read even where the window selects nothing, and once more after the
        // expected count has arrived, so an event the window excludes is read rather than
        // assumed absent (see `collect`).
        let redelivered = collect(&router, case, &url, row.expected.len()).await;
        assert_eq!(
            nonces(&redelivered),
            row.expected
                .iter()
                .map(|nonce| (*nonce).to_string())
                .collect::<Vec<_>>(),
            "case {case:?}: expected the window to redeliver {:?}, got {redelivered:?}",
            row.expected
        );
        for envelope in &redelivered {
            assert_eq!(
                envelope[REPLAY_NAME],
                json!(&replay),
                "case {case:?}: a redelivered envelope carries the replay's name, got {envelope}"
            );
            assert_eq!(
                envelope["source"],
                json!(SOURCE),
                "case {case:?}: a redelivered envelope carries the archived source, got {envelope}"
            );
            assert_eq!(
                envelope["detail-type"],
                json!(DETAIL_TYPE),
                "case {case:?}: a redelivered envelope carries the archived detail type, got \
                 {envelope}"
            );
        }

        // The rule the filters do NOT name receives nothing from the replay, although its
        // pattern matches the same events: the read is anchored on the replay having settled
        // and on the named rule's redeliveries having arrived, so an empty queue is evidence
        // of the confinement rather than of having looked too early.
        let unnamed = collect(&router, case, &unnamed_url, 0).await;
        assert!(
            unnamed.is_empty(),
            "case {case:?}: a rule the FilterArns do not name receives nothing from the \
             replay, got {unnamed:?}"
        );

        // And it is still live: a fresh publication reaches it, so its silence above was the
        // replay's confinement and not a dead delivery path.
        publish(&router, case, &bus, &[(anchor, "after")]).await;
        let after = collect(&router, case, &unnamed_url, 1).await;
        assert_eq!(
            nonces(&after),
            vec!["after".to_string()],
            "case {case:?}: the rule outside the filters still delivers live events, got \
             {after:?}"
        );
    }
}
