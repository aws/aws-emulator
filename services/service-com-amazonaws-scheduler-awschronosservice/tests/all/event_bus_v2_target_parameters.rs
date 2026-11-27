// SPDX-License-Identifier: CC0-1.0

//! The parameter blocks a templated EventBridge target states for the event bus v2 API —
//! `Target.EventBridgePutEventsV2Parameters` and `Target.EventBridgePutRawEventsParameters`
//! — end to end over the real `MultiServiceRouter`.
//!
//! RULE: a member this service's model declares is part of every operation that carries its
//! shape, so a schedule's target is persisted and reported member-for-member as written,
//! and every constraint such a member declares is the service's requirement — including a
//! `@pattern` the request boundary cannot compile, which the service applies itself when
//! the schedule is WRITTEN, at the path of whichever member carries the value. Both blocks
//! that identify a published event (`EventBridgeParameters`,
//! `EventBridgePutEventsV2Parameters`) declare the same REQUIRED `Source` and `DetailType`
//! for the same `PutEvents` call, so what a target states in either is what the invocation
//! carries.
//!
//! The rows span the class rather than one case: they differ along WHICH parameter block
//! the target states, along the values it carries (a source and detail type, a metadata map
//! of one pair and of several), and — for the constraint — along which branch of the
//! expression the value satisfies and which member carries it.
//!
//! Nothing here sleeps for a span: the one test that waits for an invocation waits for the
//! CONDITION it is about — the report on the queue — looked at again until it holds or a
//! generous deadline passes, and no assertion reads the wall clock.

use serde_json::{json, Map, Value};

use crate::dead_letter_delivery::{body_document, messages_once, PATIENCE};
use crate::schedule_invocation::{
    at_expression_body, emulator, given_execution_role_for, given_queue, schedule_read, scheduler,
    seconds_from_now, REGION,
};
use crate::wire_harness::{account_id, error_code, error_message};

/// How far ahead of the clock the schedules that must NOT be invoked state their one
/// occurrence: far enough that no clock tick reaches it while the table is written.
const FAR_OFF: u64 = 3_600;

/// How far ahead the one schedule that IS invoked states its occurrence: room for the
/// store, the role and the queue to be written before the instant passes, since a one-time
/// schedule written after its instant names no occurrence at all.
const OCCURRENCE_IN: u64 = 6;

/// The member a target states the v2 `PutEvents` parameters at.
const V2_MEMBER: &str = "EventBridgePutEventsV2Parameters";

/// The member a target states the `PutEvents` parameters that were modelled before it at.
const V1_MEMBER: &str = "EventBridgeParameters";

/// The member a target states the raw-events parameters at.
const RAW_MEMBER: &str = "EventBridgePutRawEventsParameters";

/// The members of the `PutEvents` request a bus target's FAILED event is reported as:
/// that API takes its events as a list of entries, each naming the bus, the source, the
/// detail type and the detail (`smithy_query` query="operation" operation="PutEvents" on
/// Amazon EventBridge's model: `Entries` of `PutEventsRequestEntry{EventBusName, Source,
/// DetailType, Detail}`).
const ENTRIES_MEMBER: &str = "Entries";
/// See [`ENTRIES_MEMBER`].
const EVENT_BUS_NAME_MEMBER: &str = "EventBusName";
/// See [`ENTRIES_MEMBER`].
const SOURCE_MEMBER: &str = "Source";
/// See [`ENTRIES_MEMBER`].
const DETAIL_TYPE_MEMBER: &str = "DetailType";
/// See [`ENTRIES_MEMBER`].
const DETAIL_MEMBER: &str = "Detail";

/// The ARN of an event bus of the account's own, which is what a templated EventBridge
/// target names. The bus itself is reached only when an occurrence is performed, so the
/// rows that only WRITE a schedule name one without creating it.
fn bus_arn(name: &str) -> String {
    format!("arn:aws:events:{REGION}:{}:event-bus/{name}", account_id())
}

/// A target naming the bus `arn`, acting as `role`, carrying `input`, and stating `block`
/// at the member `parameters` names. The member is a value rather than a literal, so one
/// table can state every block a target may carry.
fn bus_target(arn: &str, role: &str, input: &str, parameters: &str, block: &Value) -> Value {
    let mut target = Map::new();
    target.insert("Arn".to_string(), json!(arn));
    target.insert("RoleArn".to_string(), json!(role));
    target.insert("Input".to_string(), json!(input));
    target.insert(parameters.to_string(), block.clone());
    Value::Object(target)
}

/// A one-time schedule whose target is `target`, stated `seconds` ahead of the clock.
fn schedule_body(target: Value, seconds: u64) -> Value {
    json!({
        "ScheduleExpression": format!("at({})", at_expression_body(seconds_from_now(seconds))),
        "ScheduleExpressionTimezone": "UTC",
        "FlexibleTimeWindow": { "Mode": "OFF" },
        "State": "ENABLED",
        "Target": target,
    })
}

/// The path a clause names a `Source` at: the walk spells a path as the dotted lowerCamel
/// member names from the root of the request.
fn source_path(parameters: &str) -> &'static str {
    match parameters {
        V2_MEMBER => "target.eventBridgePutEventsV2Parameters.source",
        _ => "target.eventBridgeParameters.source",
    }
}

/// Every parameter block a templated EventBridge target may state is persisted as written
/// and reported back member-for-member by the service's own reader.
///
/// The rows differ along which block the target states and along the values inside it: the
/// source/detail-type pair the v2 `PutEvents` block carries, a metadata map of one pair and
/// one of several, and the block that was modelled before them.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn each_eventbridge_parameter_block_round_trips_through_the_reader() {
    let case = "a templated EventBridge target's parameter block";
    let emulator = emulator();
    let role = given_execution_role_for(&emulator, case, "bus-v2-round-trip-role", &[]).await;
    let arn = bus_arn("round-trip-bus");
    // (case, the member the target states the block at, the block)
    let rows: [(&str, &str, Value); 5] = [
        (
            "the v2 PutEvents block",
            V2_MEMBER,
            json!({ "Source": "orders.nightly", "DetailType": "Nightly Roll-Up" }),
        ),
        (
            "the v2 PutEvents block with a reference-path source",
            V2_MEMBER,
            json!({ "Source": "$.detail.source", "DetailType": "Re-sourced" }),
        ),
        (
            "the raw-events block carrying one metadata pair",
            RAW_MEMBER,
            json!({ "Metadata": { "tenant": "acme" } }),
        ),
        (
            "the raw-events block carrying several metadata pairs",
            RAW_MEMBER,
            json!({ "Metadata": { "tenant": "acme", "tier": "gold", "run.id": "7" } }),
        ),
        (
            "the PutEvents block modelled before them",
            V1_MEMBER,
            json!({ "Source": "returns.intake", "DetailType": "Return Received" }),
        ),
    ];

    for (index, (row, parameters, block)) in rows.iter().enumerate() {
        let name = format!("bus-v2-round-trip-{index}");
        let input = json!({ "probe": row }).to_string();
        let target = bus_target(&arn, &role, &input, parameters, block);
        let (status, created) = scheduler(
            &emulator,
            "POST",
            &format!("/schedules/{name}"),
            Some(schedule_body(target, FAR_OFF)),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {row:?}: expected the target accepted, got {status} {created}"
        );

        let (status, read) = schedule_read(&emulator, "default", &name).await;
        assert_eq!(
            status, 200,
            "case {row:?}: expected the schedule read back, got {status} {read}"
        );
        assert_eq!(
            &read["Target"][parameters],
            block,
            "case {row:?}: expected {parameters} reported as {block}, got {}",
            read["Target"]
        );
    }
}

/// A `Source` outside the expression its member declares is refused when the schedule is
/// WRITTEN — by `CreateSchedule` and by `UpdateSchedule` alike — whichever parameter block
/// carries it, and the clause names THAT member's own path. Every value the expression
/// admits is accepted in either block.
///
/// The rows differ along the block carrying the member and along which part of the
/// expression the value walks: its opening character class, the namespace its look-ahead
/// excludes, and the near misses of that namespace, which it does NOT exclude.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_source_outside_the_expression_is_refused_in_either_parameter_block() {
    // The expression this service PUBLISHES for the member: the modelled `@pattern` of
    // `Source` — the shape `EventBridgeParameters$Source` and
    // `EventBridgePutEventsV2Parameters$Source` both target
    // (`smithy_query(query="shape", shape="com.amazonaws.scheduler#Source")`) — without
    // the whole-value anchors the model writes it with, which is how this service quotes a
    // pattern.
    const PUBLISHED_SOURCE_PATTERN: &str =
        r"(?=[/\.\-_A-Za-z0-9]+)((?!aws\.).*)|(\$(\.[\w_-]+(\[(\d+|\*)\])*)*)";
    /// The source every standing schedule below is written with, and the one a refused
    /// update must leave it carrying.
    const STANDING_SOURCE: &str = "orders.standing";
    let case = "a source held to the expression its member declares";
    let emulator = emulator();
    let role = given_execution_role_for(&emulator, case, "bus-v2-source-role", &[]).await;
    let arn = bus_arn("source-pattern-bus");
    let input = json!({ "probe": "pattern" }).to_string();
    let sourced = |parameters: &str, source: &str| {
        bus_target(
            &arn,
            &role,
            &input,
            parameters,
            &json!({ "Source": source, "DetailType": "Pattern Probe" }),
        )
    };

    // One standing schedule per block, so a refused update is read back against the source
    // the schedule was written with.
    let standing = |parameters: &str| format!("bus-v2-standing-{parameters}");
    for parameters in [V1_MEMBER, V2_MEMBER] {
        let (status, created) = scheduler(
            &emulator,
            "POST",
            &format!("/schedules/{}", standing(parameters)),
            Some(schedule_body(
                sourced(parameters, STANDING_SOURCE),
                FAR_OFF,
            )),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the standing schedule of {parameters} written, got \
             {status} {created}"
        );
    }

    // (case, the member carrying the block, the Source, whether the expression admits it)
    let rows: [(&str, &str, &str, bool); 7] = [
        (
            "a custom namespace in the v2 block",
            V2_MEMBER,
            "orders.nightly",
            true,
        ),
        (
            "a path-like namespace in the v2 block",
            V2_MEMBER,
            "/orders/nightly",
            true,
        ),
        // The look-ahead is anchored at the start, so this is not the reserved prefix.
        (
            "the reserved letters without the dot, in the v2 block",
            V2_MEMBER,
            "aws",
            true,
        ),
        (
            "the reserved namespace in the v2 block",
            V2_MEMBER,
            "aws.scheduler",
            false,
        ),
        (
            "a value opening outside the character class, in the v2 block",
            V2_MEMBER,
            " orders",
            false,
        ),
        // The constraint is the MEMBER's, so the block modelled before the v2 one is held
        // to it exactly as the v2 one is.
        (
            "the reserved namespace in the block modelled before it",
            V1_MEMBER,
            "aws.events",
            false,
        ),
        (
            "a custom namespace in the block modelled before it",
            V1_MEMBER,
            "returns.intake",
            true,
        ),
    ];

    for (index, (row, parameters, source, admitted)) in rows.iter().enumerate() {
        let name = format!("bus-v2-source-{index}");
        let (status, body) = scheduler(
            &emulator,
            "POST",
            &format!("/schedules/{name}"),
            Some(schedule_body(sourced(parameters, source), FAR_OFF)),
        )
        .await;
        if *admitted {
            assert_eq!(
                status, 200,
                "case {row:?}: expected the source {source:?} accepted, got {status} {body}"
            );
            continue;
        }
        let expected = format!(
            "1 validation error detected: Value '{source}' at '{}' failed to satisfy \
             constraint: Member must satisfy regular expression pattern: \
             {PUBLISHED_SOURCE_PATTERN}",
            source_path(parameters)
        );
        assert_eq!(
            (status, error_code(&body).as_str(), error_message(&body)),
            (400, "ValidationException", expected.clone()),
            "case {row:?}: expected the constraint clause for {source:?}, got {status} {body}"
        );
        // Nothing was stored for a refused write.
        let (status, read) = schedule_read(&emulator, "default", &name).await;
        assert_eq!(
            status, 404,
            "case {row:?}: expected no schedule stored, got {status} {read}"
        );

        // The same constraint on the UPDATE path, and nothing of the standing schedule is
        // changed by the refusal.
        let held = standing(parameters);
        let (status, refused) = scheduler(
            &emulator,
            "PUT",
            &format!("/schedules/{held}"),
            Some(schedule_body(sourced(parameters, source), FAR_OFF)),
        )
        .await;
        assert_eq!(
            (status, error_code(&refused).as_str(), error_message(&refused)),
            (400, "ValidationException", expected.clone()),
            "case {row:?}: expected the update refused with the same clause, got {status} \
             {refused}"
        );
        let (status, read) = schedule_read(&emulator, "default", &held).await;
        assert_eq!(
            (status, read["Target"][parameters]["Source"].as_str()),
            (200, Some(STANDING_SOURCE)),
            "case {row:?}: expected the stored source unchanged, got {status} {read}"
        );
    }
}

/// What a target states in the v2 `PutEvents` block is what the invocation carries: an
/// occurrence the execution role may not publish is reported to the schedule's dead-letter
/// queue as the `PutEvents` request it attempted, whose one entry names the bus and carries
/// the `Source` and `DetailType` that block states.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_v2_block_identifies_the_event_the_invocation_publishes() {
    let case = "a role granted nothing on the bus";
    let emulator = emulator();
    let arn = bus_arn("v2-dead-letter-bus");
    let (dead_letter_url, dead_letter_arn) = given_queue(&emulator, case, "bus-v2-dlq").await;
    // The role may write to the dead-letter queue and nothing else, so the publication is
    // withheld and its report is still delivered.
    let role = given_execution_role_for(
        &emulator,
        case,
        "bus-v2-dead-letter-role",
        &[dead_letter_arn.as_str()],
    )
    .await;
    let source = "returns.intake";
    let detail_type = "Return Received";
    let input = json!({ "probe": "withheld" }).to_string();
    let mut target = bus_target(
        &arn,
        &role,
        &input,
        V2_MEMBER,
        &json!({ "Source": source, "DetailType": detail_type }),
    );
    // A refusal of the request is the invocation's last attempt in any case; the policy is
    // stated so the report is of an invocation that retried nothing.
    target["RetryPolicy"] = json!({ "MaximumRetryAttempts": 0 });
    target["DeadLetterConfig"] = json!({ "Arn": dead_letter_arn });
    let (status, created) = scheduler(
        &emulator,
        "POST",
        "/schedules/bus-v2-dead-letter",
        Some(schedule_body(target, OCCURRENCE_IN)),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateSchedule answered {status} {created}"
    );

    let reported = messages_once(&emulator, &dead_letter_url, 1, PATIENCE).await;
    let message = reported
        .first()
        .unwrap_or_else(|| panic!("case {case:?}: nothing was dead-lettered"));
    let document = body_document(message);
    let expected = json!({
        ENTRIES_MEMBER: [{
            EVENT_BUS_NAME_MEMBER: arn,
            SOURCE_MEMBER: source,
            DETAIL_TYPE_MEMBER: detail_type,
            DETAIL_MEMBER: input,
        }],
    });
    assert_eq!(
        document, expected,
        "case {case:?}: expected the dead-lettered request {expected}, got {message}"
    );
}
