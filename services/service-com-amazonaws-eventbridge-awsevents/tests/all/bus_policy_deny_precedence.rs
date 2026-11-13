// SPDX-License-Identifier: CC0-1.0

//! An explicit `Deny` in an event bus's resource policy refuses a client's
//! `PutEvents` as a WHOLE REQUEST, whatever else that policy grants, end to end
//! over the wire through the real `MultiServiceRouter` with SQS mounted beside
//! EventBridge.
//!
//! The RULE asserted here: publishing is the action `events:PutEvents` on the bus
//! each entry names, and the bus's own resource policy is evaluated in IAM's order
//! once per REQUEST, before any entry is published — so an explicit `Deny` about
//! any entry of the request refuses the whole request with `AccessDeniedException`,
//! whatever `Allow` the same document also states and whichever bus of the request
//! the denied entry names, and nothing of the request is published.
//!
//! What the rows span, beyond the single-statement documents and two-entry batches
//! `bus_policy_publication_authorization` covers:
//!
//! * **precedence inside ONE document** — an `Allow` and a `Deny` that both apply,
//!   in either written order, each scoped or unscoped, against the same document
//!   carrying the `Allow` alone;
//! * **the batch** — the denied entry at the first, a middle and the last position
//!   of batches from one entry up to `Entries`' modelled maximum of ten, each
//!   beside the identical batch carrying no denied entry at all;
//! * **the bus** — a request whose entries name TWO buses, only one of which
//!   carries the deny, in either order, beside the controls that show each bus's
//!   own delivery path live.
//!
//! A twin control runs on every row: a rule of the bus whose target is a queue, so
//! what the request published is read where a client reads it. An accepted request
//! puts one copy per entry on that queue; a refused one puts none.
//!
//! The harness (the router with EventBridge, SQS, IAM and CloudWatch on one engine)
//! is the one `target_dispatch` owns.

use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::target_dispatch::{drain, emulator, events, given_queue, Emulator, REGION};

/// The bus the rows publish onto, a second bus a request also names, and a third
/// named only as a statement's `Resource` — no entry ever goes near that one.
const BUS: &str = "deny-precedence-bus";
const SECOND_BUS: &str = "undenied-bus";
const UNUSED_BUS: &str = "named-only-by-a-statement";

/// The rule each bus carries, whose single target is that bus's control queue.
const RULE: &str = "receives";
const TARGET_ID: &str = "t1";

/// The two sources every batch below is built from: the one a statement's
/// condition names, and one no statement names.
const DENIED_SOURCE: &str = "com.demo.tenant-denied";
const ALLOWED_SOURCE: &str = "com.demo.tenant-allowed";

const DETAIL_TYPE: &str = "Order Created";

/// The action publishing an event performs, as a statement names it, and another
/// action of the same service a statement can name instead.
const PUT_EVENTS_ACTION: &str = "events:PutEvents";
const OTHER_EVENTS_ACTION: &str = "events:PutRule";

/// The `Sid` of each of the two statements a document below carries.
const ALLOW_SID: &str = "AllowPublishers";
const DENY_SID: &str = "DenyOneSource";

/// How long a delivery is waited FOR, by looking again rather than by sleeping a
/// fixed time, and how often it is looked for. Generous: nothing here measures how
/// fast the host runs.
const READ_BUDGET: Duration = Duration::from_secs(30);
const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// The ARN of the bus `name` in the Region every call here is signed for, as a
/// statement's `Resource` names it and as a refusal names the resource.
fn bus_arn(name: &str) -> String {
    format!(
        "arn:aws:events:{REGION}:{}:event-bus/{name}",
        engine::emulator_account_id()
    )
}

/// The identity every request below is signed as — the one principal this
/// emulator's account holds, which `sts:GetCallerIdentity` reports for it and which
/// an authorization refusal therefore names.
fn caller_arn() -> String {
    ::service_common::caller_principal::ambient_user_arn()
}

/// The sentence AWS refuses a publication its resource policy explicitly denies
/// with: fixed text around the caller's ARN and the ARN of the bus the denied entry
/// named.
fn explicit_deny_message(bus: &str) -> String {
    format!(
        "User: {} is not authorized to perform: {PUT_EVENTS_ACTION} on resource: {} \
         with an explicit deny in a resource-based policy",
        caller_arn(),
        bus_arn(bus)
    )
}

/// A clause scoping a statement to the event's `Source`, over the condition key
/// this service publishes for it.
fn on_source(value: &str) -> Value {
    json!({ "StringEquals": { "events:source": value } })
}

/// One statement, as the rows write one.
fn statement(
    sid: &str,
    effect: &str,
    action: &str,
    resource: &str,
    condition: Option<Value>,
) -> Value {
    let mut body = json!({
        "Sid": sid,
        "Effect": effect,
        "Principal": "*",
        "Action": action,
        "Resource": bus_arn(resource),
    });
    if let Some(condition) = condition {
        body["Condition"] = condition;
    }
    body
}

/// An `Allow` of the publication onto [`BUS`] for every principal, scoped by
/// `condition`.
fn allow(condition: Option<Value>) -> Value {
    statement(ALLOW_SID, "Allow", PUT_EVENTS_ACTION, BUS, condition)
}

/// A `Deny` of the publication onto [`BUS`] for every principal, scoped by
/// `condition`.
fn deny(condition: Option<Value>) -> Value {
    statement(DENY_SID, "Deny", PUT_EVENTS_ACTION, BUS, condition)
}

/// The whole policy document carrying `statements`, in the order written.
fn document(statements: &[Value]) -> String {
    json!({ "Version": "2012-10-17", "Statement": statements }).to_string()
}

/// Create the bus `name`.
async fn given_bus(emulator: &Emulator, case: &str, name: &str) {
    let (status, body) = events(emulator, "CreateEventBus", &json!({ "Name": name })).await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateEventBus {name} answered {status} {body}"
    );
}

/// A rule on the bus `bus` matching both sources published below, delivering to the
/// queue `target_arn` names.
async fn given_rule(emulator: &Emulator, case: &str, bus: &str, target_arn: &str) {
    let pattern = json!({ "source": [DENIED_SOURCE, ALLOWED_SOURCE] }).to_string();
    let (status, body) = events(
        emulator,
        "PutRule",
        &json!({
            "Name": RULE,
            "EventBusName": bus,
            "EventPattern": pattern,
            "State": "ENABLED",
        }),
    )
    .await;
    assert_eq!(status, 200, "case {case:?}: PutRule answered {status} {body}");
    let (status, body) = events(
        emulator,
        "PutTargets",
        &json!({
            "Rule": RULE,
            "EventBusName": bus,
            "Targets": [{ "Id": TARGET_ID, "Arn": target_arn }],
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: PutTargets answered {status} {body}"
    );
    assert_eq!(
        body["FailedEntryCount"],
        json!(0),
        "case {case:?}: PutTargets reported a failed entry: {body}"
    );
}

/// A bus with its own rule and control queue; answers the queue's url.
async fn given_bus_with_queue(emulator: &Emulator, case: &str, bus: &str, queue: &str) -> String {
    given_bus(emulator, case, bus).await;
    let (url, arn) = given_queue(emulator, queue).await;
    given_rule(emulator, case, bus, &arn).await;
    url
}

/// Install `statements` on the bus `bus` as one whole policy document, REPLACING
/// whatever it held, and read it back through `DescribeEventBus` so what a
/// publication is decided against is what a client can see on the bus.
async fn given_policy(emulator: &Emulator, case: &str, bus: &str, statements: &[Value]) {
    let (status, body) = events(
        emulator,
        "PutPermission",
        &json!({ "EventBusName": bus, "Policy": document(statements) }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: PutPermission answered {status} {body}"
    );
    let (status, body) = events(emulator, "DescribeEventBus", &json!({ "Name": bus })).await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeEventBus answered {status} {body}"
    );
    let reported = body["Policy"].as_str().unwrap_or_default().to_string();
    for installed in statements {
        let sid = installed["Sid"].as_str().unwrap_or_default();
        assert!(
            reported.contains(sid),
            "case {case:?}: expected the statement {sid:?} on the bus, got {reported:?}"
        );
    }
}

/// Publish `entries` in one request, each entry `(bus, source)` with an index of
/// its own in its `Detail` so a delivered copy names the entry it came from.
async fn put_events(emulator: &Emulator, entries: &[(&str, &str)]) -> (u16, Value) {
    let elements: Vec<Value> = entries
        .iter()
        .enumerate()
        .map(|(at, (bus, source))| {
            json!({
                "EventBusName": bus,
                "Source": source,
                "DetailType": DETAIL_TYPE,
                "Detail": json!({ "at": at }).to_string(),
            })
        })
        .collect();
    events(emulator, "PutEvents", &json!({ "Entries": elements })).await
}

/// Everything the queue at `url` holds once it holds `want` messages, so a delivery
/// still in flight is read again rather than read as missing; gives up after
/// [`READ_BUDGET`] so a delivery that never arrives fails an assertion rather than
/// hanging.
async fn drain_until(emulator: &Emulator, url: &str, want: usize) -> Vec<Value> {
    let deadline = Instant::now() + READ_BUDGET;
    let mut held = Vec::new();
    loop {
        held.extend(drain(emulator, url).await);
        if held.len() >= want || Instant::now() >= deadline {
            return held;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// The wire error code of a failure body (`__type` on awsJson), without its
/// namespace; the empty string on a success.
fn error_code(body: &Value) -> &str {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .rsplit('#')
        .next()
        .unwrap_or_default()
}

/// The `message` a failure body carries.
fn message(body: &Value) -> &str {
    body.get("message")
        .or_else(|| body.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or_default()
}

/// Assert that `body` is the refusal an explicit deny on the bus `bus` earns.
fn assert_refused(case: &str, bus: &str, status: u16, body: &Value) {
    assert_eq!(
        status, 400,
        "case {case:?}: expected the request to be refused, got {status} {body}"
    );
    let code = error_code(body);
    assert_eq!(
        code, "AccessDeniedException",
        "case {case:?}: expected the code AccessDeniedException, got {code:?} in {body}"
    );
    let expected = explicit_deny_message(bus);
    let reported = message(body);
    assert_eq!(
        reported, expected,
        "case {case:?}: expected the explicit-deny sentence {expected:?}, got {reported:?}"
    );
}

/// Assert that `body` accepted every one of `count` entries.
fn assert_accepted(case: &str, count: usize, status: u16, body: &Value) {
    assert_eq!(
        status, 200,
        "case {case:?}: expected the request to be accepted, got {status} {body}"
    );
    assert_eq!(
        body["FailedEntryCount"],
        json!(0),
        "case {case:?}: expected no failed entry, got {body}"
    );
    let ids = body["Entries"].as_array().cloned().unwrap_or_default();
    assert_eq!(
        ids.len(),
        count,
        "case {case:?}: expected one result per entry, got {body}"
    );
    for (at, entry) in ids.iter().enumerate() {
        assert!(
            entry["EventId"].as_str().is_some_and(|id| !id.is_empty()),
            "case {case:?}: expected an EventId on entry {at}, got {entry}"
        );
    }
}

/// One precedence row: the statements the bus carries, the sources the request
/// publishes onto that bus, and whether the request is refused.
struct Row {
    case: &'static str,
    statements: Vec<Value>,
    sources: Vec<&'static str>,
    refused: bool,
}

fn precedence_rows() -> Vec<Row> {
    vec![
        Row {
            case: "an Allow on every principal written before a Deny scoped to the published \
                   source",
            statements: vec![allow(None), deny(Some(on_source(DENIED_SOURCE)))],
            sources: vec![DENIED_SOURCE],
            refused: true,
        },
        Row {
            case: "those same two statements with the Deny written first",
            statements: vec![deny(Some(on_source(DENIED_SOURCE))), allow(None)],
            sources: vec![DENIED_SOURCE],
            refused: true,
        },
        Row {
            case: "an Allow scoped to the very source the Deny names",
            statements: vec![
                allow(Some(on_source(DENIED_SOURCE))),
                deny(Some(on_source(DENIED_SOURCE))),
            ],
            sources: vec![DENIED_SOURCE],
            refused: true,
        },
        Row {
            case: "an unconditional Deny beside an Allow scoped to the published source",
            statements: vec![allow(Some(on_source(DENIED_SOURCE))), deny(None)],
            sources: vec![DENIED_SOURCE],
            refused: true,
        },
        Row {
            case: "a Deny scoped to the published source, in a batch mixing it with another \
                   source",
            statements: vec![allow(None), deny(Some(on_source(DENIED_SOURCE)))],
            sources: vec![ALLOWED_SOURCE, DENIED_SOURCE],
            refused: true,
        },
        Row {
            case: "the Allow on every principal with no Deny beside it",
            statements: vec![allow(None)],
            sources: vec![DENIED_SOURCE],
            refused: false,
        },
        Row {
            case: "a Deny scoped to the other source, beside an Allow on every principal",
            statements: vec![allow(None), deny(Some(on_source(ALLOWED_SOURCE)))],
            sources: vec![DENIED_SOURCE],
            refused: false,
        },
        Row {
            case: "a Deny over another action of the service, beside an Allow",
            statements: vec![
                allow(None),
                statement(DENY_SID, "Deny", OTHER_EVENTS_ACTION, BUS, None),
            ],
            sources: vec![DENIED_SOURCE],
            refused: false,
        },
        Row {
            case: "a Deny naming another bus as its resource, beside an Allow",
            statements: vec![
                allow(None),
                statement(DENY_SID, "Deny", PUT_EVENTS_ACTION, UNUSED_BUS, None),
            ],
            sources: vec![DENIED_SOURCE],
            refused: false,
        },
    ]
}

/// An explicit `Deny` outranks every `Allow` of the same document, whichever order
/// the two are written in and whichever of them carries the narrower condition —
/// and a document whose `Deny` is about something else refuses nothing.
#[tokio::test]
async fn an_explicit_deny_outranks_every_allow_of_the_same_document() {
    let emulator = emulator();
    let url = given_bus_with_queue(&emulator, "the precedence rows", BUS, "precedence").await;

    for row in precedence_rows() {
        let case = row.case;
        given_policy(&emulator, case, BUS, &row.statements).await;
        let entries: Vec<(&str, &str)> =
            row.sources.iter().map(|source| (BUS, *source)).collect();

        let (status, body) = put_events(&emulator, &entries).await;

        let expected = if row.refused {
            assert_refused(case, BUS, status, &body);
            0
        } else {
            assert_accepted(case, entries.len(), status, &body);
            entries.len()
        };

        // What the request actually published, read where a client reads it. The
        // expected-empty case is read ONCE rather than waited for: there is no
        // delivery to wait for, and looking again could only pick up one from a
        // preceding row.
        let delivered = if expected == 0 {
            drain(&emulator, &url).await
        } else {
            drain_until(&emulator, &url, expected).await
        };
        assert_eq!(
            delivered.len(),
            expected,
            "case {case:?}: expected {expected} published event(s) on the rule's queue, \
             got {delivered:?}"
        );
    }
}

/// One batch row: how many entries the request carries and which of them names the
/// denied source.
struct BatchRow {
    case: &'static str,
    entries: usize,
    denied_at: usize,
}

/// The batch shapes: the denied entry first, in the middle and last, over batch
/// sizes from one entry up to `Entries`' modelled maximum (`smithy_query`
/// `PutEvents`: `Entries` has length 1..10).
fn batch_rows() -> Vec<BatchRow> {
    vec![
        BatchRow {
            case: "one entry, which is the denied one",
            entries: 1,
            denied_at: 0,
        },
        BatchRow {
            case: "two entries, the denied one first",
            entries: 2,
            denied_at: 0,
        },
        BatchRow {
            case: "two entries, the denied one last",
            entries: 2,
            denied_at: 1,
        },
        BatchRow {
            case: "five entries, the denied one first",
            entries: 5,
            denied_at: 0,
        },
        BatchRow {
            case: "five entries, the denied one in the middle",
            entries: 5,
            denied_at: 2,
        },
        BatchRow {
            case: "five entries, the denied one last",
            entries: 5,
            denied_at: 4,
        },
        BatchRow {
            case: "the modelled maximum of ten entries, the denied one first",
            entries: 10,
            denied_at: 0,
        },
        BatchRow {
            case: "the modelled maximum of ten entries, the denied one in the middle",
            entries: 10,
            denied_at: 5,
        },
        BatchRow {
            case: "the modelled maximum of ten entries, the denied one last",
            entries: 10,
            denied_at: 9,
        },
    ]
}

/// The refusal is about the REQUEST, so it does not depend on how many entries the
/// batch carries or where in it the denied entry sits: the whole request is refused
/// and NOT ONE entry is published — while the identical batch carrying no denied
/// entry is accepted whole and every entry of it arrives.
#[tokio::test]
async fn a_deny_refuses_the_whole_batch_whatever_its_size_and_the_denied_entrys_position() {
    let emulator = emulator();
    let url = given_bus_with_queue(&emulator, "the batch rows", BUS, "batched").await;
    given_policy(
        &emulator,
        "the batch rows",
        BUS,
        &[deny(Some(on_source(DENIED_SOURCE)))],
    )
    .await;

    for row in batch_rows() {
        let case = row.case;
        let mut entries: Vec<(&str, &str)> = vec![(BUS, ALLOWED_SOURCE); row.entries];
        entries[row.denied_at] = (BUS, DENIED_SOURCE);

        let (status, body) = put_events(&emulator, &entries).await;
        assert_refused(case, BUS, status, &body);
        let suppressed = drain(&emulator, &url).await;
        assert!(
            suppressed.is_empty(),
            "case {case:?}: a refused request publishes nothing, got {suppressed:?}"
        );

        // The positive control: the identical batch with the denied entry replaced
        // by an allowed one is accepted whole, so the absence above is the policy's
        // doing and not the batch's size.
        let control: Vec<(&str, &str)> = vec![(BUS, ALLOWED_SOURCE); row.entries];
        let (status, body) = put_events(&emulator, &control).await;
        assert_accepted(case, control.len(), status, &body);
        let delivered = drain_until(&emulator, &url, control.len()).await;
        assert_eq!(
            delivered.len(),
            control.len(),
            "case {case:?}: expected every entry of the control batch on the rule's queue, \
             got {delivered:?}"
        );
    }
}

/// The decision is taken for the request, so a batch whose entries name TWO buses
/// is refused in its entirety when ONE of those buses denies its entry — the other
/// bus's entry is not published either, and the refusal names the bus whose policy
/// denied.
#[tokio::test]
async fn a_deny_on_one_bus_refuses_the_entries_of_every_other_bus_in_the_same_request() {
    let case = "two buses in one request, one of them denying";
    let emulator = emulator();
    let denied_url = given_bus_with_queue(&emulator, case, BUS, "denying-bus-queue").await;
    let second_url = given_bus_with_queue(&emulator, case, SECOND_BUS, "second-bus-queue").await;
    given_policy(&emulator, case, BUS, &[deny(Some(on_source(DENIED_SOURCE)))]).await;

    // The denied bus names the source its own policy denies; the second bus names
    // the same source and carries no policy at all.
    for (at, entries) in [
        vec![(BUS, DENIED_SOURCE), (SECOND_BUS, DENIED_SOURCE)],
        vec![(SECOND_BUS, DENIED_SOURCE), (BUS, DENIED_SOURCE)],
    ]
    .into_iter()
    .enumerate()
    {
        let case = &format!("{case}, the denied entry at position {at}");
        let (status, body) = put_events(&emulator, &entries).await;
        assert_refused(case, BUS, status, &body);
        for (bus, url) in [(BUS, &denied_url), (SECOND_BUS, &second_url)] {
            let suppressed = drain(&emulator, url).await;
            assert!(
                suppressed.is_empty(),
                "case {case:?}: the refused request published nothing onto {bus}, \
                 got {suppressed:?}"
            );
        }
    }

    // The controls: each bus's own delivery path is live, so the absence above is
    // the policy's doing. The same source onto the bus that carries no policy is
    // accepted and arrives, and so is the other source onto the denying bus.
    for (bus, source, url) in [
        (SECOND_BUS, DENIED_SOURCE, &second_url),
        (BUS, ALLOWED_SOURCE, &denied_url),
    ] {
        let case = &format!("{case}, the control onto {bus}");
        let (status, body) = put_events(&emulator, &[(bus, source)]).await;
        assert_accepted(case, 1, status, &body);
        let delivered = drain_until(&emulator, url, 1).await;
        assert_eq!(
            delivered.len(),
            1,
            "case {case:?}: expected the control event on the rule's queue, got {delivered:?}"
        );
    }
}
