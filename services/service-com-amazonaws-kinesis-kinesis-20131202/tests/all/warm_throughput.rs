// SPDX-License-Identifier: CC0-1.0

//! A Kinesis data stream's WARM THROUGHPUT, end to end over the wire: `CreateStream` ->
//! `UpdateStreamWarmThroughput` -> `DescribeStreamSummary`.
//!
//! RULE: `UpdateStreamWarmThroughput` STORES the `WarmThroughputMiBps` its request names on
//! the on-demand stream its `StreamName` / `StreamARN` pair addresses — replacing whatever
//! that stream carried, the value its create named or the throughput its own open shards
//! provide — answers that stored configuration, and leaves the stream `UPDATING` until the
//! transition back to `ACTIVE` falls due. From then on the stored target is what every later
//! read of the stream reports, upwards to scale ahead of traffic and downwards to release
//! capacity again, and re-applying the same value changes nothing.
//!
//! The refusals are the ones that rule implies and the operation models: an account that
//! holds no minimum throughput billing commitment is refused with `ValidationException`
//! before its stream is even resolved; a stream whose
//! capacity mode does not admit a warm-throughput configuration is `ValidationException`, and
//! such a stream reports no warm throughput at all; a stream that is not `ACTIVE` — including
//! one whose previous update is still in flight — is `ResourceInUseException`; a target past
//! the published ceiling is `LimitExceededException` and one below the modelled floor is a
//! `ValidationException` of that member; a request naming no stream, or two different streams,
//! is `InvalidArgumentException`; and an identifier this account and region hold no stream
//! under is `ResourceNotFoundException`. Every refusal leaves the stored target exactly as it
//! was.
//!
//! The recorded flows this was derived from scale ONE on-demand stream and read it back, and
//! refuse one provisioned stream and one out-of-range target, which are three rows of the
//! tables below. The rows differ along what the rule generalises over: the target the update
//! names (both published bounds and values between them), the direction of the change (up
//! from the throughput the shards provide, down to release capacity, and the same value
//! twice), whether the create had already named a target, the identifier the request
//! addresses the stream by (name, ARN, both), and the capacity mode and lifecycle status the
//! stream is in when the call arrives.
//!
//! Every case drives the real awsJson1_1 request path (`POST /` with `X-Amz-Target:
//! Kinesis_20131202.<Operation>` through `MultiServiceRouter::dispatch`), so the statuses,
//! bodies and modelled errors asserted here are the ones a client sees.
//!
//! The wire harness and the ARN builder are [`crate::stream_tags`]', reused rather than
//! copied.

use serde_json::{json, Value};
use service_com_amazonaws_kinesis_kinesis_20131202::state::{
    stream_not_found_message, stream_not_active_message, stream_provisioned_message,
    warm_throughput_over_ceiling_message, COMMITMENT_ENABLED, MAX_WARM_THROUGHPUT_MI_BPS,
    STREAM_ACTIVE, STREAM_CREATING, STREAM_DELETING, stream_identifier_mismatch_message,
    STREAM_IDENTIFIER_REQUIRED_MESSAGE, STREAM_MODE_ON_DEMAND, STREAM_MODE_PROVISIONED,
    STREAM_UPDATING,
};

use crate::stream_tags::{
    error_code, error_message, poll_until, poll_until_active, stream_arn, Emulator, REGION,
};

/// The published bounds of `WarmThroughputMiBps`, in MiB/s: the modelled floor
/// (`smithy_query(query="operation",operation="UpdateStreamWarmThroughput")` ->
/// `WarmThroughputMiBps` range 0..) and the documented ceiling ("By default, you cannot do the
/// following: Scale to more than 10 GiBps for an on-demand stream").
const WARM_THROUGHPUT_BOUNDS: (i64, i64) = (0, MAX_WARM_THROUGHPUT_MI_BPS as i64);

/// Which identifier a row addresses its stream by. "When invoking this API, you must use
/// either the StreamARN or the StreamName parameter, or both"
/// (`UpdateStreamWarmThroughput`).
#[derive(Debug, Clone, Copy)]
enum Address {
    Name,
    Arn,
    Both,
}

/// One row of the update table: the stream the updates land on, and the targets they name in
/// turn.
struct Row {
    label: &'static str,
    /// `WarmThroughputMiBps` the create names, `None` on a row whose create names none.
    created_with: Option<i64>,
    /// The identifier the updates address the stream by.
    address: Address,
    /// The targets the updates name, applied in order.
    targets: Vec<i64>,
}

/// The table the update rule is checked over.
fn rows() -> Vec<Row> {
    vec![
        // The recorded shape: an on-demand stream created without a target, scaled up ahead
        // of traffic and read back.
        Row {
            label: "warm-throughput-up-from-what-the-shards-provide",
            created_with: None,
            address: Address::Name,
            targets: vec![64],
        },
        // Down from a target the create named: "To release excess capacity, call the API
        // again and set the warm throughput to the same or a lower value."
        Row {
            label: "warm-throughput-released-to-a-lower-target",
            created_with: Some(256),
            address: Address::Arn,
            targets: vec![128],
        },
        // The published ceiling, addressed by both identifiers at once.
        Row {
            label: "warm-throughput-at-the-published-ceiling",
            created_with: None,
            address: Address::Both,
            targets: vec![WARM_THROUGHPUT_BOUNDS.1],
        },
        // The modelled floor, and the same value a second time: re-applying what is already
        // stored changes nothing.
        Row {
            label: "warm-throughput-down-to-the-modelled-floor",
            created_with: Some(8),
            address: Address::Name,
            targets: vec![WARM_THROUGHPUT_BOUNDS.0, WARM_THROUGHPUT_BOUNDS.0],
        },
        // Several changes in a row, up and then down again, on one stream.
        Row {
            label: "warm-throughput-up-then-down-again",
            created_with: Some(32),
            address: Address::Arn,
            targets: vec![1024, 512, 1024],
        },
    ]
}

/// Hold this account's minimum throughput billing commitment, which is the ACCOUNT-level
/// precondition every case below needs: "This operation is only supported for data streams
/// with the on-demand capacity mode in accounts that have MinimumThroughputBillingCommitment
/// enabled" (`UpdateStreamWarmThroughput`). The refusal an account holding none is served is
/// pinned by [`crate::refusal_message_templates`].
pub(crate) async fn commit_the_account(emulator: &Emulator) {
    emulator
        .ok(
            "setup",
            "UpdateAccountSettings",
            &json!({
                "MinimumThroughputBillingCommitment": { "Status": COMMITMENT_ENABLED },
            }),
        )
        .await;
}

/// The `CreateStream` body a row provisions its on-demand stream with.
fn create_body(row: &Row) -> Value {
    let mut body = json!({
        "StreamName": row.label,
        "StreamModeDetails": { "StreamMode": STREAM_MODE_ON_DEMAND },
    });
    if let Some(created_with) = row.created_with {
        body["WarmThroughputMiBps"] = json!(created_with);
    }
    body
}

/// The `UpdateStreamWarmThroughput` body addressing `name` the way `address` says, naming
/// `target`.
fn update_body(address: Address, name: &str, target: i64) -> Value {
    let mut body = json!({ "WarmThroughputMiBps": target });
    match address {
        Address::Name => {
            body["StreamName"] = json!(name);
        }
        Address::Arn => {
            body["StreamARN"] = json!(stream_arn(REGION, name));
        }
        Address::Both => {
            body["StreamName"] = json!(name);
            body["StreamARN"] = json!(stream_arn(REGION, name));
        }
    }
    body
}

/// The `StreamDescriptionSummary` `DescribeStreamSummary` reports for the stream named `name`.
async fn summary(emulator: &Emulator, case: &str, name: &str) -> Value {
    emulator
        .ok(
            case,
            "DescribeStreamSummary",
            &json!({ "StreamName": name }),
        )
        .await["StreamDescriptionSummary"]
        .clone()
}

/// The `WarmThroughput$TargetMiBps` a described stream reports, `None` on a stream that
/// carries no warm throughput configuration at all: "This is only present for On-Demand
/// Kinesis Data Streams" (`WarmThroughputObject`).
fn reported_warm_throughput(summary: &Value) -> Option<i64> {
    summary
        .get("WarmThroughput")
        .and_then(|throughput| throughput.get("TargetMiBps"))
        .and_then(Value::as_i64)
}

/// The `WarmThroughput$TargetMiBps` an on-demand stream reports, which it always carries.
fn reported_target(case: &str, summary: &Value) -> i64 {
    reported_warm_throughput(summary).unwrap_or_else(|| {
        panic!("case {case}: an on-demand stream must report its warm throughput, got {summary}")
    })
}

/// The `StreamStatus` a described stream reports.
fn reported_status(case: &str, summary: &Value) -> String {
    summary
        .get("StreamStatus")
        .and_then(Value::as_str)
        .unwrap_or_else(|| {
            panic!("case {case}: a described stream must report its status, got {summary}")
        })
        .to_string()
}

/// Whatever an update stored is what the call answers and what every later read reports, for
/// every target the member admits, in both directions and however many updates the stream
/// takes.
#[tokio::test]
async fn an_update_stores_the_target_every_later_read_reports() {
    let emulator = Emulator::new();
    commit_the_account(&emulator).await;
    for row in rows() {
        emulator
            .ok(row.label, "CreateStream", &create_body(&row))
            .await;
    }
    for row in rows() {
        poll_until_active(&emulator, row.label, row.label).await;
    }

    for row in rows() {
        let case = row.label;
        let arn = stream_arn(REGION, case);

        // What the create named is what the stream carries before the first update, so every
        // value asserted after one is the one the UPDATE stored. A stream whose create named
        // no target reports the throughput its own open shards provide, which is a value, not
        // an absence.
        let before = reported_target(case, &summary(&emulator, case, case).await);
        if let Some(created_with) = row.created_with {
            assert_eq!(
                before, created_with,
                "case {case}: before the update the stream reports the target its create \
                 named ({created_with}), got {before}"
            );
        }

        for target in &row.targets {
            let answered = emulator
                .ok(
                    case,
                    "UpdateStreamWarmThroughput",
                    &update_body(row.address, case, *target),
                )
                .await;
            assert_eq!(
                answered,
                json!({
                    "StreamName": case,
                    "StreamARN": arn,
                    "WarmThroughput": { "TargetMiBps": target, "CurrentMiBps": target },
                }),
                "case {case}: an update to {target} answers the stream it updated and the \
                 configuration it stored, got {answered}"
            );

            // "Upon receiving the request, Kinesis Data Streams returns immediately and sets
            // the status of the stream to UPDATING."
            let described = summary(&emulator, case, case).await;
            let status = reported_status(case, &described);
            assert_eq!(
                status, STREAM_UPDATING,
                "case {case}: the stream is {STREAM_UPDATING} while the scaling to {target} \
                 is in flight, got {status}"
            );
            let read = reported_target(case, &described);
            assert_eq!(
                read, *target,
                "case {case}: the stream reports the target {target} it is scaling towards, \
                 got {read}"
            );

            // "After the update is complete, Kinesis Data Streams sets the status of the
            // stream back to ACTIVE", and the stored target is what the read still reports.
            // The CONDITION the emulator reaches on its own clock is what is waited for.
            poll_until_active(&emulator, case, case).await;
            let described = summary(&emulator, case, case).await;
            let status = reported_status(case, &described);
            assert_eq!(
                status, STREAM_ACTIVE,
                "case {case}: the stream is {STREAM_ACTIVE} once the scaling to {target} has \
                 fallen due, got {status}"
            );
            let read = reported_target(case, &described);
            assert_eq!(
                read, *target,
                "case {case}: expected the stored target {target}, got {read}"
            );
        }
    }
}

/// The update is scoped to the ONE stream it names: another on-demand stream of the same
/// account and region keeps the target it had.
#[tokio::test]
async fn an_update_changes_only_the_stream_it_names() {
    let emulator = Emulator::new();
    commit_the_account(&emulator).await;
    let case = "warm-throughput-scoped";
    let updated = "warm-throughput-scoped-updated";
    let untouched = "warm-throughput-scoped-untouched";
    for name in [updated, untouched] {
        emulator
            .ok(
                case,
                "CreateStream",
                &json!({
                    "StreamName": name,
                    "StreamModeDetails": { "StreamMode": STREAM_MODE_ON_DEMAND },
                    "WarmThroughputMiBps": 16,
                }),
            )
            .await;
    }
    for name in [updated, untouched] {
        poll_until_active(&emulator, case, name).await;
    }

    emulator
        .ok(
            case,
            "UpdateStreamWarmThroughput",
            &update_body(Address::Arn, updated, 2048),
        )
        .await;
    poll_until_active(&emulator, case, updated).await;

    let read = reported_target(case, &summary(&emulator, case, updated).await);
    assert_eq!(
        read, 2048,
        "case {case}: the stream the update named carries the new target, got {read}"
    );
    let read = reported_target(case, &summary(&emulator, case, untouched).await);
    assert_eq!(
        read, 16,
        "case {case}: the stream the update did not name keeps the target its create named, \
         got {read}"
    );
}

/// "This operation is only supported for data streams with the on-demand capacity mode...
/// Provisioned capacity mode streams do not support warm throughput configuration": a
/// provisioned stream is refused, whether it was created in that mode or taken to it.
#[tokio::test]
async fn an_update_of_a_provisioned_stream_is_refused() {
    let emulator = Emulator::new();
    commit_the_account(&emulator).await;

    // (case, the stream's name, whether a mode change takes it to the provisioned mode)
    let cases = [
        ("created provisioned", "warm-throughput-provisioned", false),
        ("taken to provisioned", "warm-throughput-made-provisioned", true),
    ];

    for (case, name, change_mode) in cases {
        let mut body = json!({ "StreamName": name });
        if change_mode {
            body["StreamModeDetails"] = json!({ "StreamMode": STREAM_MODE_ON_DEMAND });
        } else {
            // "For the data streams with a provisioned mode, you must specify the number of
            // shards for the data stream" (`CreateStream`).
            body["ShardCount"] = json!(2);
        }
        emulator.ok(case, "CreateStream", &body).await;
        poll_until_active(&emulator, case, name).await;

        if change_mode {
            emulator
                .ok(
                    case,
                    "UpdateStreamMode",
                    &json!({
                        "StreamARN": stream_arn(REGION, name),
                        "StreamModeDetails": { "StreamMode": STREAM_MODE_PROVISIONED },
                    }),
                )
                .await;
            // The CONDITION the mode change owes this stream: back to ACTIVE, carrying the
            // capacity mode the change named.
            poll_until(
                case,
                &format!("the stream {name} reports {STREAM_MODE_PROVISIONED} and {STREAM_ACTIVE}"),
                || async {
                    let described = summary(&emulator, case, name).await;
                    described["StreamStatus"].as_str() == Some(STREAM_ACTIVE)
                        && described["StreamModeDetails"]["StreamMode"].as_str()
                            == Some(STREAM_MODE_PROVISIONED)
                },
            )
            .await;
        }

        // "Represents the warm throughput configuration on the stream. This is only present
        // for On-Demand Kinesis Data Streams" (`WarmThroughputObject`), so a provisioned
        // stream carries none before the refused call...
        let before = reported_warm_throughput(&summary(&emulator, case, name).await);
        assert_eq!(
            before, None,
            "case {case}: a stream in the {STREAM_MODE_PROVISIONED} mode carries no warm \
             throughput configuration, got {before:?}"
        );
        let (status, response) = emulator
            .call(
                "UpdateStreamWarmThroughput",
                &update_body(Address::Arn, name, 512),
            )
            .await;
        assert_eq!(
            (status, error_code(&response).as_str()),
            (400, "ValidationException"),
            "case {case}: a stream in the {STREAM_MODE_PROVISIONED} mode supports no warm \
             throughput configuration, got {status} with body {response}"
        );
        assert_eq!(
            error_message(&response),
            stream_provisioned_message(name),
            "case {case}: expected the sentence this service states for a capacity mode an \
             operation does not serve, got {response}"
        );

        // ...and none after it either: a refused update takes no effect on the stream.
        let after = reported_warm_throughput(&summary(&emulator, case, name).await);
        assert_eq!(
            after, before,
            "case {case}: a refused update stores nothing, expected {before:?}, got {after:?}"
        );
    }
}

/// "For successful operation, the resource must be in the ACTIVE state"
/// (`ResourceInUseException`): a stream still inside the transition its create armed, one a
/// delete has been accepted for, and one whose previous scaling is still in flight are not
/// available for the change.
#[tokio::test]
async fn an_update_of_a_stream_that_is_not_active_is_refused() {
    // (the stream's name, wait past the activation, delete first, scale first, the status the
    // update lands in)
    let cases = [
        (
            "warm-throughput-creating",
            false,
            false,
            false,
            STREAM_CREATING,
        ),
        ("warm-throughput-deleting", true, true, false, STREAM_DELETING),
        ("warm-throughput-updating", true, false, true, STREAM_UPDATING),
    ];

    for (name, wait_for_activation, delete_first, scale_first, expected_status) in cases {
        let emulator = Emulator::new();
        commit_the_account(&emulator).await;
        emulator
            .ok(
                name,
                "CreateStream",
                &json!({
                    "StreamName": name,
                    "StreamModeDetails": { "StreamMode": STREAM_MODE_ON_DEMAND },
                    "WarmThroughputMiBps": 16,
                }),
            )
            .await;
        if wait_for_activation {
            poll_until_active(&emulator, name, name).await;
        }
        if delete_first {
            emulator
                .ok(name, "DeleteStream", &json!({ "StreamName": name }))
                .await;
        }
        if scale_first {
            emulator
                .ok(
                    name,
                    "UpdateStreamWarmThroughput",
                    &update_body(Address::Name, name, 64),
                )
                .await;
        }

        // A stream a delete has been accepted for answers no read, so the status every other
        // case asserts off the read is asserted there only.
        if !delete_first {
            let status_now = reported_status(name, &summary(&emulator, name, name).await);
            assert_eq!(
                status_now, expected_status,
                "case {name}: the stream is in {expected_status} when the update lands, got \
                 {status_now}"
            );
        }

        let (status, response) = emulator
            .call(
                "UpdateStreamWarmThroughput",
                &update_body(Address::Arn, name, 128),
            )
            .await;
        assert_eq!(
            (status, error_code(&response).as_str()),
            (400, "ResourceInUseException"),
            "case {name}: an update of a {expected_status} stream is refused, got {status} \
             with body {response}"
        );
        assert_eq!(
            error_message(&response),
            stream_not_active_message(name, expected_status),
            "case {name}: expected the not-active sentence this service states, got {response}"
        );

        // A target no stream may carry is refused for what it ASKS FOR, whatever state the
        // stream it names is in, so the published ceiling is stated ahead of this status.
        let (status, response) = emulator
            .call(
                "UpdateStreamWarmThroughput",
                &update_body(Address::Arn, name, WARM_THROUGHPUT_BOUNDS.1 + 1),
            )
            .await;
        assert_eq!(
            (status, error_code(&response).as_str()),
            (400, "LimitExceededException"),
            "case {name}: a target past the published ceiling is refused for its value even on \
             a {expected_status} stream, got {status} with body {response}"
        );

        // The refused update stored nothing: the stream still carries what it had, which is
        // the target the create named, or the one the scaling of this case stored.
        if !delete_first {
            poll_until_active(&emulator, name, name).await;
            let read = reported_target(name, &summary(&emulator, name, name).await);
            let expected = if scale_first { 64 } else { 16 };
            assert_eq!(
                read, expected,
                "case {name}: a refused update leaves the stored target at {expected}, got \
                 {read}"
            );
        }
    }
}

/// Each published bound is served and a target past either of them is refused: the modelled
/// `range 0..` of `WarmThroughputMiBps` below, and "By default, you cannot do the following:
/// Scale to more than 10 GiBps for an on-demand stream" above.
#[tokio::test]
async fn a_target_outside_the_published_range_is_refused() {
    let emulator = Emulator::new();
    commit_the_account(&emulator).await;
    let name = "warm-throughput-range";
    emulator
        .ok(
            name,
            "CreateStream",
            &json!({
                "StreamName": name,
                "StreamModeDetails": { "StreamMode": STREAM_MODE_ON_DEMAND },
            }),
        )
        .await;
    poll_until_active(&emulator, name, name).await;

    // Each published bound is accepted...
    for accepted in [WARM_THROUGHPUT_BOUNDS.0, WARM_THROUGHPUT_BOUNDS.1] {
        emulator
            .ok(
                name,
                "UpdateStreamWarmThroughput",
                &update_body(Address::Name, name, accepted),
            )
            .await;
        poll_until_active(&emulator, name, name).await;
        let read = reported_target(name, &summary(&emulator, name, name).await);
        assert_eq!(
            read, accepted,
            "case {accepted}: a target on a published bound is stored, got {read}"
        );
    }
    let stored = WARM_THROUGHPUT_BOUNDS.1;

    // ...and one past either of them is not. A target above the documented default limit asks
    // for more throughput than the account is allowed; one below the modelled floor fails that
    // member's `@range` the way the boundary validator states it.
    let refused: Vec<(&str, i64, &str, Option<String>)> = vec![
        (
            "one past the documented ceiling",
            WARM_THROUGHPUT_BOUNDS.1 + 1,
            "LimitExceededException",
            Some(warm_throughput_over_ceiling_message(
                i32::try_from(WARM_THROUGHPUT_BOUNDS.1 + 1).unwrap_or(i32::MAX),
            )),
        ),
        (
            "twice the documented ceiling",
            2 * WARM_THROUGHPUT_BOUNDS.1,
            "LimitExceededException",
            Some(warm_throughput_over_ceiling_message(
                i32::try_from(2 * WARM_THROUGHPUT_BOUNDS.1).unwrap_or(i32::MAX),
            )),
        ),
        (
            "far past the documented ceiling",
            1_000_000,
            "LimitExceededException",
            Some(warm_throughput_over_ceiling_message(1_000_000)),
        ),
        (
            "one below the modelled floor",
            WARM_THROUGHPUT_BOUNDS.0 - 1,
            "ValidationException",
            None,
        ),
    ];
    for (case, target, expected_code, expected_message) in &refused {
        let (status, response) = emulator
            .call(
                "UpdateStreamWarmThroughput",
                &update_body(Address::Name, name, *target),
            )
            .await;
        assert_eq!(
            (status, error_code(&response).as_str()),
            (400, *expected_code),
            "case {case}: a target of {target} is outside the published range, got {status} \
             with body {response}"
        );
        if let Some(expected_message) = expected_message {
            assert_eq!(
                &error_message(&response),
                expected_message,
                "case {case}: expected the sentence this service states for a target past the \
                 ceiling, got {response}"
            );
        }
    }

    // A request that names no target at all names nothing to store either.
    let (status, response) = emulator
        .call(
            "UpdateStreamWarmThroughput",
            &json!({ "StreamName": name }),
        )
        .await;
    assert_eq!(
        status, 400,
        "case no member: the required member must be named, got {status} with body {response}"
    );

    let read = reported_target(name, &summary(&emulator, name, name).await);
    assert_eq!(
        read, stored,
        "case after the refusals: a refused update leaves the stored target at {stored}, got \
         {read}"
    );
}

/// One row of the identifier-refusal table: the identifiers the request names, and the
/// modelled error and sentence they are refused with.
struct Refusal {
    label: &'static str,
    /// `StreamName`, when the request names one.
    name: Option<String>,
    /// `StreamARN`, when the request names one.
    arn: Option<String>,
    /// The modelled error's shape name.
    code: &'static str,
    /// The sentence it carries.
    message: String,
}

/// "You must use either the StreamARN or the StreamName parameter, or both": a request naming
/// neither names no stream, a pair naming two different streams names no one stream, and an
/// identifier this account and region hold no stream under names nothing to update.
#[tokio::test]
async fn a_request_naming_no_one_stream_of_this_account_and_region_is_refused() {
    let emulator = Emulator::new();
    commit_the_account(&emulator).await;
    let first = "warm-throughput-refusals-first";
    let second = "warm-throughput-refusals-second";
    for name in [first, second] {
        emulator
            .ok(
                name,
                "CreateStream",
                &json!({
                    "StreamName": name,
                    "StreamModeDetails": { "StreamMode": STREAM_MODE_ON_DEMAND },
                    "WarmThroughputMiBps": 16,
                }),
            )
            .await;
    }
    for name in [first, second] {
        poll_until_active(&emulator, name, name).await;
    }

    let absent = "warm-throughput-never-created";
    let other_account = format!("arn:aws:kinesis:{REGION}:123456789012:stream/{first}");
    let cases = vec![
        Refusal {
            label: "neither identifier",
            name: None,
            arn: None,
            code: "InvalidArgumentException",
            message: STREAM_IDENTIFIER_REQUIRED_MESSAGE.to_string(),
        },
        Refusal {
            label: "two different streams",
            name: Some(first.to_string()),
            arn: Some(stream_arn(REGION, second)),
            code: "InvalidArgumentException",
            message: stream_identifier_mismatch_message(first, &stream_arn(REGION, second)),
        },
        Refusal {
            label: "a name nothing created",
            name: Some(absent.to_string()),
            arn: None,
            code: "ResourceNotFoundException",
            message: stream_not_found_message(absent),
        },
        Refusal {
            label: "an ARN nothing created",
            name: None,
            arn: Some(stream_arn(REGION, absent)),
            code: "ResourceNotFoundException",
            message: stream_not_found_message(&stream_arn(REGION, absent)),
        },
        Refusal {
            label: "a stream of another region",
            name: None,
            arn: Some(stream_arn("eu-west-1", first)),
            code: "ResourceNotFoundException",
            message: stream_not_found_message(&stream_arn("eu-west-1", first)),
        },
        // `StreamARN` is modelled `^arn:aws.*:kinesis:.*:\d{12}:stream/\S+$`, so an ARN
        // naming another account's stream is well formed and names no stream of this one.
        Refusal {
            label: "a stream of another account",
            name: None,
            arn: Some(other_account.clone()),
            code: "ResourceNotFoundException",
            message: stream_not_found_message(&other_account),
        },
    ];

    for case in &cases {
        let label = case.label;
        let mut body = json!({ "WarmThroughputMiBps": 512 });
        if let Some(name) = &case.name {
            body["StreamName"] = json!(name);
        }
        if let Some(arn) = &case.arn {
            body["StreamARN"] = json!(arn);
        }
        let (status, response) = emulator.call("UpdateStreamWarmThroughput", &body).await;
        assert_eq!(
            (status, error_code(&response).as_str()),
            (400, case.code),
            "case {label}: expected the modelled refusal, got {status} with body {response}"
        );
        assert_eq!(
            error_message(&response),
            case.message,
            "case {label}: expected the sentence this service states, got {response}"
        );
    }

    // None of the refusals touched a stream this account does hold.
    for name in [first, second] {
        let read = reported_target(name, &summary(&emulator, name, name).await);
        assert_eq!(
            read, 16,
            "case after the refusals: {name} keeps the target its create named, got {read}"
        );
    }
}
