// SPDX-License-Identifier: CC0-1.0

//! DescribeFlowSourceThumbnail, end to end over HTTP.
//!
//! The rule under test: the operation REPORTS what this service holds for the flow its
//! `{FlowArn}` label resolves to in the store — so it answers `NotFoundException` for a
//! label the store does not hold, and for a flow it holds that satisfies the report's
//! preconditions (running, with thumbnails enabled) it answers that STORED flow's ARN plus
//! one `thumbnailMessages` entry per STORED source. A flow that fails either precondition is
//! refused instead, which is the subject of
//! `tests/all/flow_source_report_preconditions.rs`; the fixtures here are therefore created
//! with thumbnails enabled and started before they are read. Nothing in this emulator
//! ingests media, so no frame has ever been received: `thumbnail`, `timecode` and
//! `timestamp` are absent for every flow rather than carrying a synthesised image.
//!
//! Each rule is asserted over a TABLE whose rows differ along what it generalises over —
//! which OTHER monitoring settings the configuration carries beside the thumbnail state, how
//! many sources the flow holds and what they are named, and which protocol they use —
//! rather than over one recorded request. Every case drives the real request path (routing,
//! restJson1 deserialization, constraint validation, handler, serialization) through the
//! harness [`crate::flow_lifecycle::Emulator`], so what these tests assert is what a
//! MediaConnect client sees on the wire.

use serde_json::{json, Value};

use crate::flow_lifecycle::{Emulator, REGION};
use crate::flow_source_metadata::{assert_messages_are_worded, coded, create, start_and_settle};
use crate::flow_source_report_preconditions::assert_thumbnail_refusal;

/// The `MessageDetail$Code` this operation reports, written out here rather than read from
/// the crate so that a change to the handler's constants cannot quietly move what these
/// tests assert: `NO_THUMBNAIL_AVAILABLE` is the code for a source of a reported-on flow
/// nothing has arrived on.
const CODE_NO_THUMBNAIL_AVAILABLE: &str = "NO_THUMBNAIL_AVAILABLE";

/// The status a started flow settles into (`smithy_query(query="shape",shape="Status")`).
const STATUS_ACTIVE: &str = "ACTIVE";

/// Call DescribeFlowSourceThumbnail through its modelled HTTP binding
/// (`smithy_query(query="operation",operation="DescribeFlowSourceThumbnail")`: GET
/// `/v1/flows/{FlowArn}/source-thumbnail`).
///
/// `pub(crate)` because the precondition suite drives the same binding.
pub(crate) async fn source_thumbnail(
    emulator: &Emulator,
    flow_arn: &str,
) -> (u16, Option<String>, Value) {
    emulator
        .send("GET", &format!("/v1/flows/{flow_arn}/source-thumbnail"), None)
        .await
}

/// Assert that no image, timecode or generation instant is reported: nothing is ingested, so
/// no frame has ever been decoded for any flow.
///
/// `pub(crate)` because the precondition suite asserts the same of every served read.
pub(crate) fn assert_no_image_is_reported(case: &str, details: &Value) {
    for (member, value) in [
        ("thumbnail", &details["thumbnail"]),
        ("timecode", &details["timecode"]),
        ("timestamp", &details["timestamp"]),
    ] {
        assert!(
            value.is_null(),
            "case {case:?}: no {member} is reported for a flow nothing has been ingested \
             on, got {value}"
        );
    }
}

/// One row of the reporting table: a CreateFlow body for a flow the report is served for.
struct ReportCase {
    case: &'static str,
    /// The flow as the create request states it. Every row enables thumbnails, because the
    /// read is gated on them (see `tests/all/flow_source_report_preconditions.rs`), and
    /// every row's flow is started before it is read for the same reason.
    flow: Value,
}

/// The rows span what the served report generalises over, with the two preconditions held
/// fixed at the values that admit the read: which OTHER monitoring settings the
/// configuration carries beside `ThumbnailState` ENABLED
/// (`smithy_query(query="shape",shape="MonitoringConfig")`: `AudioMonitoringSettings`,
/// `ContentQualityAnalysisState`, `VideoMonitoringSettings`), how many sources the flow
/// holds (one, and the two a flow under active failover may hold), how they are named, and
/// which protocol they use — including an entitled source, which has no ingest endpoint of
/// its own.
fn report_cases() -> Vec<ReportCase> {
    vec![
        ReportCase {
            case: "one rtp source, thumbnails ENABLED",
            flow: json!({
                "name": "thumbnail-enabled-running",
                "sourceMonitoringConfig": { "thumbnailState": "ENABLED" },
                "source": { "name": "src", "protocol": "rtp", "whitelistCidr": "10.21.0.0/16" }
            }),
        },
        ReportCase {
            case: "one srt-listener source named unlike the flow",
            flow: json!({
                "name": "thumbnail-enabled-srt",
                "sourceMonitoringConfig": { "thumbnailState": "ENABLED" },
                "source": {
                    "name": "primary-feed",
                    "protocol": "srt-listener",
                    "whitelistCidr": "192.0.2.0/24",
                    "minLatency": 2000
                }
            }),
        },
        ReportCase {
            case: "a configuration that also tunes content quality analysis",
            flow: json!({
                "name": "thumbnail-with-quality-analysis",
                "sourceMonitoringConfig": {
                    "thumbnailState": "ENABLED",
                    "contentQualityAnalysisState": "ENABLED"
                },
                "source": { "name": "src", "protocol": "rtp", "whitelistCidr": "10.23.0.0/16" }
            }),
        },
        ReportCase {
            case: "a configuration that also carries audio and video monitoring settings",
            flow: json!({
                "name": "thumbnail-other-settings",
                "sourceMonitoringConfig": {
                    "thumbnailState": "ENABLED",
                    "audioMonitoringSettings": [
                        { "silentAudio": { "state": "ENABLED", "thresholdSeconds": 12 } }
                    ],
                    "videoMonitoringSettings": [
                        { "blackFrames": { "state": "ENABLED", "thresholdSeconds": 4 } }
                    ]
                },
                "source": { "name": "src", "protocol": "rtp", "whitelistCidr": "10.24.0.0/16" }
            }),
        },
        ReportCase {
            case: "two sources under active failover",
            flow: json!({
                "name": "thumbnail-two-sources",
                "sourceMonitoringConfig": { "thumbnailState": "ENABLED" },
                "sources": [
                    { "name": "feed-a", "protocol": "rtp", "whitelistCidr": "10.26.0.0/16" },
                    { "name": "feed-b", "protocol": "rist", "whitelistCidr": "10.27.0.0/16" }
                ],
                "sourceFailoverConfig": { "state": "ENABLED", "failoverMode": "MERGE" }
            }),
        },
        ReportCase {
            case: "an entitled source, which has no ingest endpoint of its own",
            flow: json!({
                "name": "thumbnail-entitled",
                "sourceMonitoringConfig": { "thumbnailState": "ENABLED" },
                "source": {
                    "name": "entitled-feed",
                    "entitlementArn": format!(
                        "arn:aws:mediaconnect:{REGION}:444455556666:entitlement:1-0000beef:ent"
                    )
                }
            }),
        },
    ]
}

/// DescribeFlowSourceThumbnail reports the STORED flow: its ARN, one coded message per
/// stored source naming that source, and no image, timecode or generation instant for any
/// flow — because nothing has been ingested.
#[tokio::test]
async fn source_thumbnail_reports_the_stored_flow_and_its_sources() {
    let emulator = Emulator::new();

    for ReportCase { case, flow } in report_cases() {
        let (flow_arn, source_names) = create(&emulator, case, flow).await;
        assert!(
            !source_names.is_empty(),
            "case {case:?}: the fixture flow must hold at least one source"
        );
        start_and_settle(&emulator, case, &flow_arn).await;

        let (status, error, body) = source_thumbnail(&emulator, &flow_arn).await;
        assert_eq!(
            (status, error.as_deref()),
            (200, None),
            "case {case:?}: expected 200 with no error, got {status} {error:?}: {body}"
        );
        let details = &body["thumbnailDetails"];
        assert_eq!(
            details["flowArn"].as_str(),
            Some(flow_arn.as_str()),
            "case {case:?}: the details name the flow the call was performed on, got {body}"
        );

        // One message per stored source, in the flow's own source order, each naming its
        // source and carrying the code a running flow nothing has arrived on implies.
        let expected: Vec<(String, String)> = source_names
            .iter()
            .map(|name| (CODE_NO_THUMBNAIL_AVAILABLE.to_string(), name.clone()))
            .collect();
        assert_eq!(
            coded(&details["thumbnailMessages"]),
            expected,
            "case {case:?}: expected {expected:?}, got {}",
            details["thumbnailMessages"]
        );
        assert_messages_are_worded(case, &details["thumbnailMessages"]);
        assert_no_image_is_reported(case, details);
    }
}

/// Whether the read is served follows the flow's STORED source monitoring configuration, not
/// the request that created it: on one RUNNING flow, turning thumbnails off through UpdateFlow
/// makes the same read refused, turning them back on makes it served again, and a
/// configuration that replaces the thumbnail state with other monitoring settings leaves
/// thumbnails off.
#[tokio::test]
async fn the_read_follows_the_stored_monitoring_configuration() {
    let emulator = Emulator::new();
    let case = "a flow whose monitoring configuration is updated";
    let (flow_arn, source_names) = create(
        &emulator,
        case,
        json!({
            "name": "thumbnail-updated",
            "sourceMonitoringConfig": { "thumbnailState": "ENABLED" },
            "source": { "name": "src", "protocol": "rtp", "whitelistCidr": "10.30.0.0/16" }
        }),
    )
    .await;
    start_and_settle(&emulator, case, &flow_arn).await;

    /// One step: the `sourceMonitoringConfig` UpdateFlow is given (or `None` to read the
    /// flow as created), and whether the read must be served afterwards.
    struct Step {
        case: &'static str,
        config: Option<Value>,
        served: bool,
    }

    let steps = [
        Step {
            case: "as created, with thumbnails enabled",
            config: None,
            served: true,
        },
        Step {
            case: "thumbnails turned off",
            config: Some(json!({ "thumbnailState": "DISABLED" })),
            served: false,
        },
        Step {
            case: "thumbnails turned back on",
            config: Some(json!({ "thumbnailState": "ENABLED" })),
            served: true,
        },
        Step {
            case: "the configuration replaced by one that never mentions thumbnails",
            config: Some(json!({ "contentQualityAnalysisState": "ENABLED" })),
            served: false,
        },
        Step {
            case: "thumbnails turned on again",
            config: Some(json!({ "thumbnailState": "ENABLED" })),
            served: true,
        },
        Step {
            case: "thumbnails turned off again",
            config: Some(json!({ "thumbnailState": "DISABLED" })),
            served: false,
        },
    ];

    for Step {
        case,
        config,
        served,
    } in steps
    {
        if let Some(config) = config {
            let (status, error, body) = emulator
                .send(
                    "PUT",
                    &format!("/v1/flows/{flow_arn}"),
                    Some(json!({ "sourceMonitoringConfig": config })),
                )
                .await;
            assert_eq!(
                (status, error.as_deref()),
                (202, None),
                "case {case:?}: UpdateFlow expected 202, got {status} {error:?}: {body}"
            );
        }

        let (status, error, body) = source_thumbnail(&emulator, &flow_arn).await;
        if !served {
            // Only the thumbnail precondition is unmet here: the flow is still running.
            assert_thumbnail_refusal(case, status, error.as_deref(), &body, false, true);
            continue;
        }
        assert_eq!(
            (status, error.as_deref()),
            (200, None),
            "case {case:?}: expected 200 with no error, got {status} {error:?}: {body}"
        );
        let expected: Vec<(String, String)> = source_names
            .iter()
            .map(|name| (CODE_NO_THUMBNAIL_AVAILABLE.to_string(), name.clone()))
            .collect();
        assert_eq!(
            coded(&body["thumbnailDetails"]["thumbnailMessages"]),
            expected,
            "case {case:?}: expected {expected:?}, got {}",
            body["thumbnailDetails"]["thumbnailMessages"]
        );
        assert_no_image_is_reported(case, &body["thumbnailDetails"]);
    }
}

/// The read REPORTS: calling it twice answers the same body, and it moves neither the flow's
/// status nor its monitoring configuration, so a later caller sees exactly what an earlier
/// one did.
#[tokio::test]
async fn source_thumbnail_is_a_report_that_changes_nothing() {
    let emulator = Emulator::new();
    let case = "repeated reads of one flow";
    let (flow_arn, _) = create(
        &emulator,
        case,
        json!({
            "name": "thumbnail-stable",
            "sourceMonitoringConfig": { "thumbnailState": "ENABLED" },
            "source": { "name": "src", "protocol": "rtp", "whitelistCidr": "10.31.0.0/16" }
        }),
    )
    .await;
    start_and_settle(&emulator, case, &flow_arn).await;

    let (_, _, before) = emulator.describe_flow(&flow_arn).await;
    let (_, _, first) = source_thumbnail(&emulator, &flow_arn).await;
    let (_, _, second) = source_thumbnail(&emulator, &flow_arn).await;
    assert_eq!(
        first, second,
        "case {case:?}: two reads of an unchanged flow answer the same body, got {first} \
         then {second}"
    );

    let (status, error, after) = emulator.describe_flow(&flow_arn).await;
    assert_eq!(
        (status, error.as_deref()),
        (200, None),
        "case {case:?}: DescribeFlow must answer 200, got {status} {error:?}: {after}"
    );
    assert_eq!(
        after["flow"]["status"].as_str(),
        Some(STATUS_ACTIVE),
        "case {case:?}: reading the thumbnail did not move the flow's status, got {}",
        after["flow"]["status"]
    );
    assert_eq!(
        before, after,
        "case {case:?}: reading the thumbnail changed nothing about the flow, got {before} \
         then {after}"
    );
}

/// The `{FlowArn}` label is resolved against the STORE once it satisfies the constraint the
/// model states for it: a label of the modelled form that the store does not hold is refused
/// 404 `NotFoundException` whose message is the label the request named, while a label
/// outside that form is refused by the modelled constraint itself (400 `BadRequestException`)
/// before any lookup.
///
/// The not-found answer comes before the report's own preconditions: a label that resolves to
/// no flow has no status and no configuration to judge, so these rows name flows that were
/// never started and are still answered 404.
///
/// `DescribeFlowSourceThumbnailRequest$FlowArn` declares no pattern of its own
/// (`smithy_query(query="operation",operation="DescribeFlowSourceThumbnail")`), so the narrow
/// `FlowArn` shape pattern is the only one the model states for it.
#[tokio::test]
async fn an_unheld_flow_label_is_refused_not_found() {
    let emulator = Emulator::new();

    // A flow that existed and does not any more resolves to nothing, like one that never
    // did.
    let (deleted, _) = create(
        &emulator,
        "deleted flow",
        json!({
            "name": "thumbnail-doomed",
            "source": { "name": "src", "protocol": "rtp", "whitelistCidr": "10.32.0.0/16" }
        }),
    )
    .await;
    let (status, code, body) = emulator
        .send("DELETE", &format!("/v1/flows/{deleted}"), None)
        .await;
    assert_eq!(
        (status, code.as_deref()),
        (202, None),
        "deleting the fixture flow expected 202, got {status} {code:?}: {body}"
    );

    // Labels of the modelled form that address no flow of this store: one deleted, one
    // never minted, one of another account, one of another Region.
    let unheld = [
        ("a flow that was deleted", deleted),
        (
            "a flow ARN of the modelled form that names no flow",
            format!("arn:aws:mediaconnect:{REGION}:111122223333:flow:1-0000dead:never-made"),
        ),
        (
            "a flow ARN of another account",
            format!("arn:aws:mediaconnect:{REGION}:999988887777:flow:1-0000beef:elsewhere"),
        ),
        (
            "a flow ARN of another Region",
            "arn:aws:mediaconnect:eu-west-1:111122223333:flow:1-0000beef:elsewhere".to_string(),
        ),
    ];
    for (case, label) in unheld {
        let (status, code, body) = source_thumbnail(&emulator, &label).await;
        assert_eq!(
            (status, code.as_deref()),
            (404, Some("NotFoundException")),
            "case {case:?}: expected 404 NotFoundException, got {status} {code:?}: {body}"
        );
        assert_eq!(
            body["message"].as_str(),
            Some(
                service_com_amazonaws_mediaconnect_mediaconnect::state::not_found_message(&label)
                    .as_str()
            ),
            "case {case:?}: the message is the label the request named in this service's \
             envelope, got {body}"
        );
    }

    // Labels outside the modelled form, one broken segment at a time: the modelled
    // constraint refuses them before the store is consulted.
    let malformed = [
        ("not an ARN at all", "not-a-flow".to_string()),
        (
            "an ARN of another resource type",
            format!("arn:aws:mediaconnect:{REGION}:111122223333:source:1-0000dead:src"),
        ),
        (
            "an ARN of another service",
            format!("arn:aws:medialive:{REGION}:111122223333:flow:1-0000dead:f"),
        ),
        (
            "an account segment that is not twelve digits",
            format!("arn:aws:mediaconnect:{REGION}:11112222333:flow:1-0000dead:f"),
        ),
        (
            "a flow name outside the pattern's character class",
            format!("arn:aws:mediaconnect:{REGION}:111122223333:flow:1-0000dead:f.1"),
        ),
    ];
    for (case, label) in malformed {
        let (status, code, body) = source_thumbnail(&emulator, &label).await;
        assert_eq!(
            (status, code.as_deref()),
            (400, Some("BadRequestException")),
            "case {case:?}: expected 400 BadRequestException, got {status} {code:?}: {body}"
        );
        let message = body["message"].as_str().unwrap_or_default();
        assert!(
            message.contains("flowArn"),
            "case {case:?}: the refusal names the member it judged, got {message:?}"
        );
    }
}
