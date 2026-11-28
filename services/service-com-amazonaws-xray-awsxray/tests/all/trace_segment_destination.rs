// SPDX-License-Identifier: CC0-1.0

//! The trace segment destination (Transaction Search) configuration, end to end
//! over the wire.
//!
//! Every case drives the same front door the production router does (the `Xray`
//! harness in [`super::xray_wire`]), so the assertions below are about HTTP
//! responses, not internal state. The cases about a destination this service
//! DELIVERS to mount CloudWatch Logs beside it on one engine, so the grants read
//! are the ones that service's own `PutResourcePolicy` wrote.
//!
//! The rules under test:
//!
//! * `GetTraceSegmentDestination` reports the destination the account last
//!   configured, and the account default (`XRay`) until something configures
//!   one; its input models no members, so nothing in its answer can come from
//!   the request;
//! * a configured destination is `PENDING` until the switch becomes effective
//!   and `ACTIVE` from then on, and reading it neither starts nor skips that
//!   move: repeated reads at the same instant answer the same thing;
//! * a request naming the destination the account is ALREADY on applies no
//!   switch, so it is refused with the modelled `InvalidRequestException` and
//!   changes nothing — an account singleton that is already there has no
//!   transition for the caller to wait on;
//! * a switch to `CloudWatchLogs` is refused with `AccessDeniedException` while
//!   no CloudWatch Logs resource policy lets X-Ray put the account's spans into
//!   the log group they go to, and the destination is left as it was; with such
//!   a policy in force the identical call is accepted;
//! * a destination outside the model's `TraceSegmentDestination` value set, an
//!   absent destination, a `CloudWatchLogsConfiguration` beside a destination
//!   other than `CloudWatchLogs`, and a `SpansLogGroupKmsKeyId` that is not a
//!   KMS key ARN are each refused at HTTP 400 — a value outside the MODELLED
//!   enum by the request layer as `ValidationException`, a rule this service
//!   enforces itself by the handler as the modelled `InvalidRequestException` —
//!   and a refused call changes nothing.

use std::time::{Duration, Instant};

use serde_json::{json, Value};

use service_com_amazonaws_cloudwatchlogs_logs_20140328::state as logs;
use service_com_amazonaws_xray_awsxray::ops::types::ADMITTED_TRACE_SEGMENT_DESTINATIONS;
use service_com_amazonaws_xray_awsxray::state::destination_switch::{
    destination_already_set_message, spans_delivery_ungranted_message, SPANS_DELIVERY_ACTION,
    XRAY_SERVICE_PRINCIPAL,
};
use service_com_amazonaws_xray_awsxray::state::span_delivery::SPANS_LOG_GROUP;
use service_com_amazonaws_xray_awsxray::state::{
    CLOUD_WATCH_LOGS_DESTINATION, DEFAULT_TRACE_SEGMENT_DESTINATION, DESTINATION_STATUS_ACTIVE,
    DESTINATION_STATUS_PENDING,
};

use super::xray_wire::{Answer, Xray, DEFAULT_REGION};

/// How long a switch is waited FOR before a case gives up: generous on purpose,
/// because what each wait below is bounded by is the CONDITION, not the clock.
const SETTLE_DEADLINE: Duration = Duration::from_secs(30);

/// How often the reported destination is looked at again while waiting.
const LOOK_AGAIN: Duration = Duration::from_millis(25);

/// A valid KMS key ARN, the only form `SpansLogGroupKmsKeyId` accepts per that
/// member's documentation ("Key IDs and alias ARNs are not supported").
const KEY_ARN: &str = "arn:aws:kms:us-east-1:123456789012:key/1234abcd-12ab-34cd-56ef-1234567890ab";

/// The name the account-scoped resource policy below is stored under; nothing of
/// the rule depends on it, since a policy is found by what its statements say.
const POLICY_NAME: &str = "XRayTransactionSearchAccess";

/// A member of an answer's body as a string, or `None`.
fn member<'a>(answer: &'a Answer, name: &str) -> Option<&'a str> {
    answer.get(name).as_str()
}

/// `(Destination, Status)`: the pair both operations here answer with.
fn destination_and_status(answer: &Answer) -> (Option<&str>, Option<&str>) {
    (member(answer, "Destination"), member(answer, "Status"))
}

async fn get_destination(xray: &Xray) -> Answer {
    xray.call("/GetTraceSegmentDestination", &json!({})).await
}

async fn update_destination(xray: &Xray, body: &Value) -> Answer {
    xray.call("/UpdateTraceSegmentDestination", body).await
}

/// One emulator with CloudWatch Logs mounted beside X-Ray on ONE engine, so the
/// resource policies this service reads are the ones CloudWatch Logs' own API
/// wrote ([`Xray::with_cloud_watch_logs`]).
fn emulator() -> Xray {
    Xray::with_cloud_watch_logs()
}

/// The ARN of the log group X-Ray delivers spans to, as CloudWatch Logs itself
/// renders it ([`logs::log_group_arn`]): the wildcard form when `streams` is set
/// (`LogGroup$arn`, which names the group's streams), else the plain one
/// (`LogGroup$logGroupArn`).
fn spans_log_group_arn(streams: bool) -> String {
    logs::log_group_arn(DEFAULT_REGION, SPANS_LOG_GROUP, streams)
}

/// A CloudWatch Logs resource policy whose one statement allows `principal` to
/// take `action` on `resource`.
fn allow_policy(principal: &str, action: &str, resource: &str) -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Sid": "SpansDelivery",
            "Effect": "Allow",
            "Principal": { "Service": principal },
            "Action": action,
            "Resource": resource,
        }],
    })
}

/// Put `document` as the account's resource policy through CloudWatch Logs' OWN
/// `PutResourcePolicy`, which is the API the Transaction Search setup flow uses:
/// the spans log group is vended by X-Ray, so the grant is written before that
/// group exists and is therefore account-scoped.
async fn put_logs_policy(xray: &Xray, case: &str, document: &Value) {
    let put = xray
        .logs_call(
            "PutResourcePolicy",
            &json!({ "policyName": POLICY_NAME, "policyDocument": document.to_string() }),
        )
        .await;
    assert_eq!(
        put.status, 200,
        "case {case:?}: putting the resource policy must succeed, got {put:?}"
    );
}

/// Grant X-Ray the delivery a `CloudWatchLogs` destination needs, the way AWS's
/// own Transaction Search policy does: the service principal,
/// `logs:PutLogEvents` and the spans log group ([`Xray::grant_spans_delivery`]).
async fn grant_spans_delivery(xray: &Xray, case: &str) {
    xray.grant_spans_delivery(case).await;
}

/// Read the reported destination until the switch the clock owes it is in
/// effect, and answer what was read then.
///
/// The move to `ACTIVE` falls due at a fixed instant after the call that asked
/// for it, so the condition is waited FOR rather than for a duration: the
/// deadline is generous and the poll looks again until the status is the terminal
/// one.
async fn settled(xray: &Xray, case: &str, destination: &str) -> Answer {
    let deadline = Instant::now() + SETTLE_DEADLINE;
    loop {
        let read = get_destination(xray).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: reading the destination must succeed, got {read:?}"
        );
        if destination_and_status(&read) == (Some(destination), Some(DESTINATION_STATUS_ACTIVE)) {
            return read;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: the switch to {destination:?} never took effect, last read {read:?}"
        );
        tokio::time::sleep(LOOK_AGAIN).await;
    }
}

/// Configure `destination` and leave it in effect, granting the delivery it needs
/// first when it is the one this service delivers to.
async fn settled_destination(xray: &Xray, case: &str, destination: &str) {
    if destination == CLOUD_WATCH_LOGS_DESTINATION {
        grant_spans_delivery(xray, case).await;
    }
    let updated = update_destination(xray, &json!({ "Destination": destination })).await;
    assert_eq!(
        updated.status, 200,
        "case {case:?}: seeding destination {destination:?} must succeed, got {updated:?}"
    );
    settled(xray, case, destination).await;
}

// =============================================================================
// 1. The default destination is reported until something configures one
// =============================================================================

#[tokio::test]
async fn the_account_default_destination_is_reported_and_reading_it_changes_nothing() {
    let xray = Xray::new();

    // Three reads in a row: a reporting operation must answer the same thing
    // each time, and the first read must not configure anything.
    for attempt in 1..=3 {
        let read = get_destination(&xray).await;
        assert_eq!(
            (read.status, destination_and_status(&read)),
            (
                200,
                (
                    Some(DEFAULT_TRACE_SEGMENT_DESTINATION),
                    Some(DESTINATION_STATUS_ACTIVE)
                )
            ),
            "case read #{attempt} of a never-configured account: expected 200 \
             {DEFAULT_TRACE_SEGMENT_DESTINATION}/{DESTINATION_STATUS_ACTIVE}, got {read:?}"
        );
    }

    // The default is one of the modelled values, not an invention.
    assert!(
        ADMITTED_TRACE_SEGMENT_DESTINATIONS.contains(&DEFAULT_TRACE_SEGMENT_DESTINATION),
        "case default value: {DEFAULT_TRACE_SEGMENT_DESTINATION:?} must be one of \
         {ADMITTED_TRACE_SEGMENT_DESTINATIONS:?}"
    );
}

// =============================================================================
// 2. A switch is PENDING until the clock makes it ACTIVE, and Get reports it
// =============================================================================

/// One destination switch: from the destination in effect, to the one asked
/// for. The rows span every ordered pair of distinct values the destination type
/// admits ([`ADMITTED_TRACE_SEGMENT_DESTINATIONS`]), so each value is exercised
/// as both the source and the target of a switch.
fn switch_rows() -> Vec<(String, String)> {
    let mut rows = Vec::new();
    for from in ADMITTED_TRACE_SEGMENT_DESTINATIONS {
        for to in ADMITTED_TRACE_SEGMENT_DESTINATIONS {
            if from != to {
                rows.push(((*from).to_string(), (*to).to_string()));
            }
        }
    }
    rows
}

#[tokio::test]
async fn a_destination_switch_is_pending_then_the_clock_makes_it_active() {
    for (from, to) in switch_rows() {
        let case = format!("{from} -> {to}");
        let xray = emulator();
        grant_spans_delivery(&xray, &case).await;
        if from != DEFAULT_TRACE_SEGMENT_DESTINATION {
            settled_destination(&xray, &case, &from).await;
        }
        let before = get_destination(&xray).await;
        assert_eq!(
            destination_and_status(&before),
            (Some(from.as_str()), Some(DESTINATION_STATUS_ACTIVE)),
            "case {case:?}: the starting destination must be in effect, got {before:?}"
        );

        let updated = update_destination(&xray, &json!({ "Destination": to.clone() })).await;
        assert_eq!(
            (updated.status, destination_and_status(&updated)),
            (200, (Some(to.as_str()), Some(DESTINATION_STATUS_PENDING))),
            "case {case:?}: an accepted switch answers the new destination as \
             {DESTINATION_STATUS_PENDING}, got {updated:?}"
        );

        // The transient state is observable: a caller reading straight back
        // sees PENDING, twice, because a read does not settle the move early.
        for attempt in 1..=2 {
            let pending = get_destination(&xray).await;
            assert_eq!(
                (pending.status, destination_and_status(&pending)),
                (200, (Some(to.as_str()), Some(DESTINATION_STATUS_PENDING))),
                "case {case:?} read #{attempt} before the switch is due: expected 200 \
                 {to}/{DESTINATION_STATUS_PENDING}, got {pending:?}"
            );
        }

        // And once the clock owes the move, every later read reports it in
        // effect: the earlier reads neither accelerated nor froze it.
        settled(&xray, &case, &to).await;
        let active = get_destination(&xray).await;
        assert_eq!(
            (active.status, destination_and_status(&active)),
            (200, (Some(to.as_str()), Some(DESTINATION_STATUS_ACTIVE))),
            "case {case:?}: a read after the switch is due: expected 200 \
             {to}/{DESTINATION_STATUS_ACTIVE}, got {active:?}"
        );
    }
}

// =============================================================================
// 3. Naming the destination already in effect is refused, and changes nothing
// =============================================================================

#[tokio::test]
async fn a_request_naming_the_destination_already_in_effect_is_refused() {
    // Rows span what the rule generalises over: WHICH destination the account is
    // already on (every value the destination type admits), whether the switch to
    // it has taken effect or is still being applied, and whether the no-op
    // request carries a configuration beside the destination.
    for destination in ADMITTED_TRACE_SEGMENT_DESTINATIONS {
        for settled_first in [true, false] {
            if !settled_first && *destination == DEFAULT_TRACE_SEGMENT_DESTINATION {
                // The default is in effect from the start: there is no switch to
                // it that could still be pending.
                continue;
            }
            let case = format!(
                "{destination} re-requested while it is {}",
                if settled_first { "in effect" } else { "pending" }
            );
            let xray = emulator();
            grant_spans_delivery(&xray, &case).await;
            if *destination != DEFAULT_TRACE_SEGMENT_DESTINATION {
                if settled_first {
                    settled_destination(&xray, &case, destination).await;
                } else {
                    let accepted =
                        update_destination(&xray, &json!({ "Destination": destination })).await;
                    assert_eq!(
                        (accepted.status, destination_and_status(&accepted)),
                        (
                            200,
                            (Some(*destination), Some(DESTINATION_STATUS_PENDING))
                        ),
                        "case {case:?}: the first switch must be accepted, got {accepted:?}"
                    );
                }
            }
            let expected_status = if settled_first {
                DESTINATION_STATUS_ACTIVE
            } else {
                DESTINATION_STATUS_PENDING
            };

            let refused = update_destination(&xray, &json!({ "Destination": destination })).await;
            assert_eq!(
                (refused.status, refused.error_type.as_deref()),
                (400, Some("InvalidRequestException")),
                "case {case:?}: expected 400 InvalidRequestException, got {refused:?}"
            );
            let expected_message = destination_already_set_message(destination);
            assert_eq!(
                refused.message(),
                Some(expected_message.as_str()),
                "case {case:?}: expected the message {expected_message:?}, got {:?}",
                refused.message()
            );

            // The refusal changed nothing: neither the destination nor the state
            // of the switch that was already under way.
            let read = get_destination(&xray).await;
            assert_eq!(
                (read.status, destination_and_status(&read)),
                (200, (Some(*destination), Some(expected_status))),
                "case {case:?}: a refused request must change nothing, got {read:?}"
            );
        }
    }
}

// =============================================================================
// 4. CloudWatchLogs is accepted only where a resource policy admits the spans
// =============================================================================

/// One row of the grant table: the resource policy in force (none, or one
/// document), and whether the switch to `CloudWatchLogs` may be accepted with it.
struct Grant {
    case: &'static str,
    document: Option<Value>,
    accepted: bool,
}

/// The rows differ along what the rule generalises over: whether a policy exists
/// at all, which principal it names, which action, which resource (the group, its
/// streams, another group), whether the grant is wildcarded, whether it is
/// qualified by the condition keys AWS's own Transaction Search policy states,
/// and whether an explicit `Deny` stands beside the grant.
fn grant_rows() -> Vec<Grant> {
    let spans_streams = spans_log_group_arn(true);
    let spans_group = spans_log_group_arn(false);
    let other_group = logs::log_group_arn(DEFAULT_REGION, "some-other-group", true);
    vec![
        Grant {
            case: "no resource policy at all",
            document: None,
            accepted: false,
        },
        Grant {
            case: "a policy granting another service principal",
            document: Some(allow_policy(
                "logs.amazonaws.com",
                SPANS_DELIVERY_ACTION,
                &spans_streams,
            )),
            accepted: false,
        },
        Grant {
            case: "a policy granting another action",
            document: Some(allow_policy(
                XRAY_SERVICE_PRINCIPAL,
                "logs:CreateLogStream",
                &spans_streams,
            )),
            accepted: false,
        },
        Grant {
            case: "a policy naming another log group",
            document: Some(allow_policy(
                XRAY_SERVICE_PRINCIPAL,
                SPANS_DELIVERY_ACTION,
                &other_group,
            )),
            accepted: false,
        },
        Grant {
            case: "a grant on the spans log group's streams",
            document: Some(allow_policy(
                XRAY_SERVICE_PRINCIPAL,
                SPANS_DELIVERY_ACTION,
                &spans_streams,
            )),
            accepted: true,
        },
        Grant {
            case: "a grant on the spans log group itself",
            document: Some(allow_policy(
                XRAY_SERVICE_PRINCIPAL,
                SPANS_DELIVERY_ACTION,
                &spans_group,
            )),
            accepted: true,
        },
        Grant {
            case: "a grant whose action is wildcarded",
            document: Some(allow_policy(
                XRAY_SERVICE_PRINCIPAL,
                "logs:*",
                &spans_streams,
            )),
            accepted: true,
        },
        Grant {
            case: "a grant qualified by the conditions AWS's own policy states",
            document: Some(json!({
                "Version": "2012-10-17",
                "Statement": [{
                    "Sid": "TransactionSearchXRayAccess",
                    "Effect": "Allow",
                    "Principal": { "Service": XRAY_SERVICE_PRINCIPAL },
                    "Action": SPANS_DELIVERY_ACTION,
                    "Resource": [spans_streams.clone()],
                    "Condition": {
                        "StringEquals": {
                            "aws:SourceAccount": emulator_server::emulator_account_id(),
                        },
                        "ArnLike": {
                            "aws:SourceArn": format!(
                                "arn:aws:xray:{DEFAULT_REGION}:{}:*",
                                emulator_server::emulator_account_id()
                            ),
                        },
                    },
                }],
            })),
            accepted: true,
        },
        Grant {
            case: "an explicit deny beside the grant",
            document: Some(json!({
                "Version": "2012-10-17",
                "Statement": [
                    {
                        "Sid": "SpansDelivery",
                        "Effect": "Allow",
                        "Principal": { "Service": XRAY_SERVICE_PRINCIPAL },
                        "Action": SPANS_DELIVERY_ACTION,
                        "Resource": spans_streams.clone(),
                    },
                    {
                        "Sid": "NoSpansDelivery",
                        "Effect": "Deny",
                        "Principal": { "Service": XRAY_SERVICE_PRINCIPAL },
                        "Action": SPANS_DELIVERY_ACTION,
                        "Resource": spans_streams.clone(),
                    },
                ],
            })),
            accepted: false,
        },
    ]
}

#[tokio::test]
async fn a_switch_to_cloud_watch_logs_needs_a_policy_admitting_the_spans() {
    for row in grant_rows() {
        let case = row.case;
        let xray = emulator();
        if let Some(document) = &row.document {
            put_logs_policy(&xray, case, document).await;
        }

        let answered = update_destination(
            &xray,
            &json!({ "Destination": CLOUD_WATCH_LOGS_DESTINATION }),
        )
        .await;
        if row.accepted {
            assert_eq!(
                (answered.status, destination_and_status(&answered)),
                (
                    200,
                    (
                        Some(CLOUD_WATCH_LOGS_DESTINATION),
                        Some(DESTINATION_STATUS_PENDING)
                    )
                ),
                "case {case:?}: the switch must be accepted, got {answered:?}"
            );
            settled(&xray, case, CLOUD_WATCH_LOGS_DESTINATION).await;
            continue;
        }

        assert_eq!(
            (answered.status, answered.error_type.as_deref()),
            (403, Some("AccessDeniedException")),
            "case {case:?}: expected 403 AccessDeniedException, got {answered:?}"
        );
        let expected_message = spans_delivery_ungranted_message();
        assert_eq!(
            answered.message(),
            Some(expected_message.as_str()),
            "case {case:?}: expected the message {expected_message:?}, got {:?}",
            answered.message()
        );

        // The destination is left exactly as it was, so a client that follows
        // the refusal is not sending spans to a path nothing reaches.
        let read = get_destination(&xray).await;
        assert_eq!(
            (read.status, destination_and_status(&read)),
            (
                200,
                (
                    Some(DEFAULT_TRACE_SEGMENT_DESTINATION),
                    Some(DESTINATION_STATUS_ACTIVE)
                )
            ),
            "case {case:?}: a refused switch must change nothing, got {read:?}"
        );
    }
}

#[tokio::test]
async fn the_identical_switch_is_accepted_once_the_policy_is_in_place() {
    // The one difference between the two calls is the policy: the request, the
    // account and the destination are the same.
    let case = "the identical call before and after the policy";
    let xray = emulator();
    let body = json!({ "Destination": CLOUD_WATCH_LOGS_DESTINATION });

    let refused = update_destination(&xray, &body).await;
    assert_eq!(
        (refused.status, refused.error_type.as_deref()),
        (403, Some("AccessDeniedException")),
        "case {case:?}: expected the first call refused, got {refused:?}"
    );

    grant_spans_delivery(&xray, case).await;

    let accepted = update_destination(&xray, &body).await;
    assert_eq!(
        (accepted.status, destination_and_status(&accepted)),
        (
            200,
            (
                Some(CLOUD_WATCH_LOGS_DESTINATION),
                Some(DESTINATION_STATUS_PENDING)
            )
        ),
        "case {case:?}: expected the second call accepted, got {accepted:?}"
    );
    settled(&xray, case, CLOUD_WATCH_LOGS_DESTINATION).await;
}

// =============================================================================
// 5. A CloudWatchLogsConfiguration is accepted with the CloudWatchLogs
//    destination, and the destination still round-trips through Get
// =============================================================================

#[tokio::test]
async fn a_cloud_watch_logs_configuration_round_trips_with_its_destination() {
    // Rows differ in whether a configuration is supplied at all, and in the
    // key ARN supplied with it.
    let rows: Vec<(&str, Value)> = vec![
        (
            "no CloudWatchLogsConfiguration",
            json!({"Destination": "CloudWatchLogs"}),
        ),
        (
            "an empty CloudWatchLogsConfiguration",
            json!({"Destination": "CloudWatchLogs", "CloudWatchLogsConfiguration": {}}),
        ),
        (
            "a customer-managed key ARN",
            json!({
                "Destination": "CloudWatchLogs",
                "CloudWatchLogsConfiguration": {"SpansLogGroupKmsKeyId": KEY_ARN},
            }),
        ),
        (
            "a key ARN in another partition and region",
            json!({
                "Destination": "CloudWatchLogs",
                "CloudWatchLogsConfiguration": {
                    "SpansLogGroupKmsKeyId":
                        "arn:aws-us-gov:kms:us-gov-west-1:210987654321:key/abcd-ef01",
                },
            }),
        ),
    ];

    for (case, body) in rows {
        let xray = emulator();
        grant_spans_delivery(&xray, case).await;
        let updated = update_destination(&xray, &body).await;
        assert_eq!(
            (updated.status, destination_and_status(&updated)),
            (
                200,
                (Some("CloudWatchLogs"), Some(DESTINATION_STATUS_PENDING))
            ),
            "case {case:?}: expected 200 CloudWatchLogs/{DESTINATION_STATUS_PENDING}, \
             got {updated:?}"
        );

        let read = settled(&xray, case, "CloudWatchLogs").await;
        assert_eq!(
            (read.status, destination_and_status(&read)),
            (
                200,
                (Some("CloudWatchLogs"), Some(DESTINATION_STATUS_ACTIVE))
            ),
            "case {case:?}: the configured destination must read back as \
             {DESTINATION_STATUS_ACTIVE}, got {read:?}"
        );
    }
}

// =============================================================================
// 6. The stored CloudWatchLogsConfiguration belongs to its destination
// =============================================================================

/// No modelled output returns `SpansLogGroupKmsKeyId`, so this case reads the
/// stored record directly: the supplied key is kept while the service points at
/// `CloudWatchLogs` and dropped when it points elsewhere.
#[tokio::test]
async fn the_stored_spans_log_group_key_follows_the_cloud_watch_logs_destination() {
    use service_com_amazonaws_xray_awsxray::state::ComAmazonawsXrayAwsxrayState;

    let mut store = ComAmazonawsXrayAwsxrayState::default();
    let now = std::time::Instant::now();

    // Rows: a key supplied with CloudWatchLogs is stored; the same destination
    // re-configured without one keeps it; pointing elsewhere drops it.
    let stored =
        store.set_trace_segment_destination(CLOUD_WATCH_LOGS_DESTINATION, Some(KEY_ARN), now);
    assert_eq!(
        stored.spans_log_group_kms_key_id.as_deref(),
        Some(KEY_ARN),
        "case a key supplied with {CLOUD_WATCH_LOGS_DESTINATION}: expected it stored, \
         got {stored:?}"
    );

    let kept = store.set_trace_segment_destination(CLOUD_WATCH_LOGS_DESTINATION, None, now);
    assert_eq!(
        kept.spans_log_group_kms_key_id.as_deref(),
        Some(KEY_ARN),
        "case {CLOUD_WATCH_LOGS_DESTINATION} re-configured without a key: expected the \
         stored key kept, got {kept:?}"
    );

    let dropped = store.set_trace_segment_destination(DEFAULT_TRACE_SEGMENT_DESTINATION, None, now);
    assert_eq!(
        dropped.spans_log_group_kms_key_id, None,
        "case switched to {DEFAULT_TRACE_SEGMENT_DESTINATION}: the configuration applies \
         only to {CLOUD_WATCH_LOGS_DESTINATION}, so expected no stored key, got {dropped:?}"
    );
    assert_eq!(
        dropped.status(),
        DESTINATION_STATUS_PENDING,
        "case switched to {DEFAULT_TRACE_SEGMENT_DESTINATION}: a real switch is \
         {DESTINATION_STATUS_PENDING}, got {dropped:?}"
    );
}

// =============================================================================
// 7. An invalid configuration request is refused, and changes nothing
// =============================================================================

#[tokio::test]
async fn an_invalid_destination_request_is_refused_and_stores_nothing() {
    // Rows span the ways the request can be invalid: no destination at all, a
    // value outside the model's enum (unknown, miscased, empty), a
    // configuration beside the wrong destination, and every
    // `SpansLogGroupKmsKeyId` form the member's documentation excludes.
    //
    // The third column is the code that class of refusal carries: a value
    // outside the MODELLED enum is refused by the request layer, which answers
    // the `ValidationException` envelope, while a rule this service enforces
    // itself is refused by the handler as `InvalidRequestException`.
    let rows: Vec<(&str, Value, &str)> = vec![
        ("Destination absent", json!({}), "InvalidRequestException"),
        (
            "Destination absent but a configuration supplied",
            json!({"CloudWatchLogsConfiguration": {"SpansLogGroupKmsKeyId": KEY_ARN}}),
            "InvalidRequestException",
        ),
        (
            "Destination empty",
            json!({"Destination": ""}),
            "ValidationException",
        ),
        (
            "Destination outside the model's value set",
            json!({"Destination": "S3"}),
            "ValidationException",
        ),
        (
            "Destination miscased",
            json!({"Destination": "cloudwatchlogs"}),
            "ValidationException",
        ),
        (
            "Destination with surrounding whitespace",
            json!({"Destination": " CloudWatchLogs "}),
            "ValidationException",
        ),
        (
            "a configuration beside the XRay destination",
            json!({
                "Destination": "XRay",
                "CloudWatchLogsConfiguration": {"SpansLogGroupKmsKeyId": KEY_ARN},
            }),
            "InvalidRequestException",
        ),
        (
            "SpansLogGroupKmsKeyId as a bare key id",
            json!({
                "Destination": "CloudWatchLogs",
                "CloudWatchLogsConfiguration": {
                    "SpansLogGroupKmsKeyId": "1234abcd-12ab-34cd-56ef-1234567890ab",
                },
            }),
            "InvalidRequestException",
        ),
        (
            "SpansLogGroupKmsKeyId as an alias name",
            json!({
                "Destination": "CloudWatchLogs",
                "CloudWatchLogsConfiguration": {"SpansLogGroupKmsKeyId": "alias/spans"},
            }),
            "InvalidRequestException",
        ),
        (
            "SpansLogGroupKmsKeyId as an alias ARN",
            json!({
                "Destination": "CloudWatchLogs",
                "CloudWatchLogsConfiguration": {
                    "SpansLogGroupKmsKeyId": "arn:aws:kms:us-east-1:123456789012:alias/spans",
                },
            }),
            "InvalidRequestException",
        ),
        (
            "SpansLogGroupKmsKeyId naming another service's resource",
            json!({
                "Destination": "CloudWatchLogs",
                "CloudWatchLogsConfiguration": {
                    "SpansLogGroupKmsKeyId": "arn:aws:logs:us-east-1:123456789012:key/1234abcd",
                },
            }),
            "InvalidRequestException",
        ),
    ];

    for (case, body, code) in rows {
        // Start from a destination that is NOT the default, so a refusal that
        // wrote anything — including a reset to the default — would show.
        let xray = emulator();
        settled_destination(&xray, case, CLOUD_WATCH_LOGS_DESTINATION).await;

        let refused = update_destination(&xray, &body).await;
        assert_eq!(
            (refused.status, refused.error_type.as_deref()),
            (400, Some(code)),
            "case {case:?}: expected 400 {code}, got {refused:?}"
        );
        // A framework refusal renders `message`; a modelled error shape renders
        // its member's own casing (`Message`).
        assert!(
            refused.message().is_some_and(|message| !message.is_empty()),
            "case {case:?}: a refusal must say what was wrong, got {refused:?}"
        );

        let read = get_destination(&xray).await;
        assert_eq!(
            (read.status, destination_and_status(&read)),
            (
                200,
                (Some("CloudWatchLogs"), Some(DESTINATION_STATUS_ACTIVE))
            ),
            "case {case:?}: a refused request must change nothing, got {read:?}"
        );
    }
}
