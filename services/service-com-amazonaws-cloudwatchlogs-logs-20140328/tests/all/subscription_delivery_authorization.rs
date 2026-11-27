// SPDX-License-Identifier: CC0-1.0

//! What a subscription filter's DELIVERY ROLE decides when nothing can be learned about it,
//! end to end over the wire with the Kinesis emulator mounted beside this service on one
//! engine — the assembly a client gets when IAM is not mounted at all.
//!
//! RULE: CloudWatch Logs writes to a subscription destination under the IAM role the filter's
//! configuration names, and withholds that write only where the permissions in force on that
//! role are ANSWERED not to allow the destination service's write action. Where no answer can
//! be had — there is no IAM to ask, the value names no role of this account, or the records
//! cannot be read — nothing is withheld: the health check the put makes and the envelopes
//! later batches owe are written exactly as they were before any role was consulted. A
//! delivery role is routinely provisioned out of band, so its absence from a store is evidence
//! about that store and not about the role's permissions.
//!
//! The rows differ along what that half of the rule generalises over: the FORM of the role the
//! filter names (this account's role under the default path, one under a path, another
//! account's role) and where the role comes from at all (the filter's own `roleArn`, or the
//! `roleArn` of the `PutDestination` logical destination a filter names instead, which carries
//! none of its own).
//!
//! The other half — what a role IAM DOES answer about decides — needs IAM mounted beside this
//! service, so it lives in `core/router/tests/all/logs_subscription_delivery_role.rs`.
//!
//! Every case drives the real wire path and reads the destination where a consumer reads it,
//! through the harness `subscription_fan_out.rs` owns.

use serde_json::json;

use crate::harness::base_ms;
use crate::subscription_fan_out::{assert_envelope, stream_arn, Emulator, BY_LOG_STREAM};

/// The line every case writes, which an empty filter pattern selects.
const LINE: &str = "ERROR disk full";

/// The filter every case configures.
const FILTER: &str = "to-kinesis";

/// The log stream every case writes to.
const LOG_STREAM: &str = "instance-one";

/// The IAM role ARN of this account that `role` names, under `path` (`""` for the default
/// path). An IAM ARN carries no Region.
fn role_arn_of(path: &str, role: &str) -> String {
    engine::emulator_arn("iam", "", &format!("role/{path}{role}"))
}

/// One row: where the delivery role comes from and the form it takes.
struct Row {
    case: &'static str,
    /// The row's own resource suffix, so each row has its own group, stream and destination.
    name: &'static str,
    /// The `roleArn` the filter is configured with, or `None` for a row that names a logical
    /// destination (whose own `roleArn` governs the write, so the filter carries none).
    filter_role_arn: Option<String>,
    /// The `roleArn` the logical destination is created with, for such a row.
    destination_role_arn: Option<String>,
}

/// The rows: every form of delivery role, none of which can be decided here.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a role of this account under the default path",
            name: "default-path",
            filter_role_arn: Some(role_arn_of("", "CWLtoKinesisRole")),
            destination_role_arn: None,
        },
        Row {
            case: "a role of this account under a path",
            name: "under-a-path",
            filter_role_arn: Some(role_arn_of("service-role/", "CWLtoKinesisRole")),
            destination_role_arn: None,
        },
        Row {
            case: "another account's role, which this account's IAM is not asked about",
            name: "another-account",
            filter_role_arn: Some("arn:aws:iam::123456789012:role/CWLtoKinesisRole".to_string()),
            destination_role_arn: None,
        },
        Row {
            case: "a logical destination's own role, the filter carrying none",
            name: "logical-destination",
            filter_role_arn: None,
            destination_role_arn: Some(role_arn_of("", "CWLtoKinesisRole")),
        },
    ]
}

/// Nothing is withheld where nothing about the delivery role can be answered: every form of
/// role delivers, and the put's health check reaches the destination too.
#[tokio::test]
async fn an_undecidable_delivery_role_withholds_nothing() {
    let emulator = Emulator::new();
    let rows = rows();
    for row in &rows {
        emulator
            .create_stream(row.case, &format!("stream-{}", row.name))
            .await;
    }
    // Every stream is WAITED FOR until it reports itself usable, rather than for a span of
    // the clock: the status it leaves `CREATING` for is one the clock owes, and the wait ends
    // as soon as the stream says so.
    for row in &rows {
        emulator
            .await_active_stream(row.case, &format!("stream-{}", row.name))
            .await;
    }

    for row in &rows {
        let case = row.case;
        let stream = format!("stream-{}", row.name);
        let group = format!("/aws/emulator/{}", row.name);
        emulator
            .logs(case, "CreateLogGroup", &json!({"logGroupName": group}))
            .await;
        emulator
            .logs(
                case,
                "CreateLogStream",
                &json!({"logGroupName": group, "logStreamName": LOG_STREAM}),
            )
            .await;

        // The destination the filter names: the stream itself, or a logical destination over
        // it created with its own delivery role. A logical destination also needs the access
        // policy that lets this account subscribe to it ("the destination owner must call
        // PutDestinationPolicy after PutDestination").
        let destination_arn = match &row.destination_role_arn {
            None => stream_arn(&stream),
            Some(role_arn) => {
                let created = emulator
                    .logs(
                        case,
                        "PutDestination",
                        &json!({
                            "destinationName": row.name,
                            "targetArn": stream_arn(&stream),
                            "roleArn": role_arn,
                        }),
                    )
                    .await;
                let arn = created["destination"]["arn"]
                    .as_str()
                    .unwrap_or_else(|| {
                        panic!("case {case}: PutDestination reports the arn, got {created}")
                    })
                    .to_string();
                emulator
                    .logs(
                        case,
                        "PutDestinationPolicy",
                        &json!({
                            "destinationName": row.name,
                            "accessPolicy": json!({
                                "Version": "2012-10-17",
                                "Statement": [{
                                    "Effect": "Allow",
                                    "Principal": {
                                        "AWS": format!(
                                            "arn:aws:iam::{}:root",
                                            engine::emulator_account_id()
                                        ),
                                    },
                                    "Action": "logs:PutSubscriptionFilter",
                                    "Resource": arn,
                                }],
                            })
                            .to_string(),
                        }),
                    )
                    .await;
                arn
            }
        };

        let mut put = json!({
            "logGroupName": group,
            "filterName": FILTER,
            "filterPattern": "",
            "destinationArn": destination_arn,
            "distribution": BY_LOG_STREAM,
        });
        if let Some(role_arn) = &row.filter_role_arn {
            put["roleArn"] = json!(role_arn);
        }
        emulator.logs(case, "PutSubscriptionFilter", &put).await;

        let timestamp = base_ms();
        emulator
            .logs(
                case,
                "PutLogEvents",
                &json!({
                    "logGroupName": group,
                    "logStreamName": LOG_STREAM,
                    "logEvents": [{"timestamp": timestamp, "message": LINE}],
                }),
            )
            .await;

        let records = emulator.delivered(case, &stream).await;
        assert_eq!(
            records.len(),
            1,
            "case {case}: an undecidable role withholds nothing, so the line is delivered, \
             got {records:?}"
        );
        let (key, envelope) = &records[0];
        assert_envelope(
            case,
            envelope,
            &group,
            LOG_STREAM,
            FILTER,
            &[LINE],
            &[timestamp],
        );
        assert_eq!(
            key, LOG_STREAM,
            "case {case}: log data grouped by log stream is keyed on the log stream, got {key}"
        );
        let checks = emulator.health_checks(case, &stream).await;
        // The put's health check, and — for a row whose filter names a logical destination —
        // the test message `PutDestination` wrote while it validated that destination.
        let expected = 1 + usize::from(row.destination_role_arn.is_some());
        assert_eq!(
            checks.len(),
            expected,
            "case {case}: the put's health check is written under the same role, got {checks:?}"
        );

        // The batch is ingested whatever the delivery did.
        let page = emulator
            .logs(
                case,
                "GetLogEvents",
                &json!({"logGroupName": group, "logStreamName": LOG_STREAM}),
            )
            .await;
        let messages: Vec<String> = page["events"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|event| {
                event["message"]
                    .as_str()
                    .unwrap_or_else(|| {
                        panic!("case {case}: a stored event carries a message, got {event}")
                    })
                    .to_string()
            })
            .collect();
        assert_eq!(
            messages,
            vec![LINE.to_string()],
            "case {case}: the ingested line stays readable, got {messages:?}"
        );
    }
}
