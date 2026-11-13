// SPDX-License-Identifier: CC0-1.0

//! The preconditions a flow's SOURCE REPORTS are gated on, end to end over HTTP.
//!
//! THE RULE UNDER TEST. DescribeFlowSourceMetadata reports the transport stream a flow is
//! RECEIVING and DescribeFlowSourceThumbnail a still decoded out of it, so MediaConnect
//! answers neither for a flow that is not in a state where it could be receiving anything,
//! and answers the thumbnail read only for a flow thumbnails are turned on for. Such a
//! request is refused with the modelled `BadRequestException` (HTTP 400, error type
//! `BadRequestException`) carrying AWS's fixed message for that error class, NOTHING is
//! projected, and nothing about the flow moves; the same call is served once the flow
//! satisfies the precondition.
//!
//! The messages are written out below exactly as AWS words them, rather than read from the
//! crate, so a change to the handler cannot quietly move what these tests assert. Their only
//! variable parts are the status and state names the model declares, and the thumbnail
//! refusal reports one sentence per precondition the flow fails — so a flow failing both is
//! refused once, with both.
//!
//! The rows differ along what the rule generalises over: which status the flow is in, which
//! value of the modelled `ThumbnailState` enum its stored source monitoring configuration
//! names (and whether it names one, or carries a configuration at all), how many sources the
//! flow holds, which protocol they use, and whether its source is entitled rather than
//! owned. Every case drives the real request path through the harness
//! [`crate::flow_lifecycle::Emulator`], so what these tests assert is what a MediaConnect
//! client sees on the wire.

use serde_json::{json, Value};

use crate::flow_lifecycle::{Emulator, REGION};
use crate::flow_source_metadata::{create, source_metadata, start_and_settle};
use crate::flow_source_thumbnail::{assert_no_image_is_reported, source_thumbnail};

/// AWS's whole `BadRequestException$Message` for a source-metadata read of a flow that is
/// not `ACTIVE`: fixed text for this error class, interpolating nothing from the request.
///
/// `pub(crate)` because `tests/all/flow_source_metadata.rs` asserts the same refusal while
/// driving the flow through its lifecycle.
pub(crate) const METADATA_REFUSAL: &str =
    "Bad request: The flow must be in ACTIVE state to perform this action";

/// The envelope AWS reports a failed source-thumbnail validation under.
const VALIDATION_FAILED: &str = "Validation failed:";

/// The sentence AWS adds for a flow whose status does not admit a thumbnail read.
const THUMBNAIL_STATUS_LINE: &str =
    "To perform this action, the flow must be in ACTIVE or UPDATING state.";

/// The sentence AWS adds for a flow whose stored source monitoring configuration does not
/// have thumbnails enabled.
const THUMBNAIL_DISABLED_LINE: &str = "This action can only be performed on a flow with \
                                       ThumbnailState in SourceMonitoringConfiguration set \
                                       to ENABLED.";

/// AWS's whole `BadRequestException$Message` for a source-thumbnail read, given which
/// preconditions the flow fails: the envelope line, then one sentence per unmet precondition
/// in the order the operation checks them.
fn thumbnail_refusal(status_unmet: bool, thumbnail_unmet: bool) -> String {
    let mut lines = vec![VALIDATION_FAILED];
    if status_unmet {
        lines.push(THUMBNAIL_STATUS_LINE);
    }
    if thumbnail_unmet {
        lines.push(THUMBNAIL_DISABLED_LINE);
    }
    lines.join("\n")
}

/// Assert one answer is the modelled source-thumbnail refusal, worded as AWS words it for
/// the preconditions `status_unmet` / `thumbnail_unmet` say the flow failed, and that nothing
/// was projected with it.
///
/// `pub(crate)` because `tests/all/flow_source_thumbnail.rs` asserts the same refusal when
/// the stored configuration turns thumbnails off under it.
pub(crate) fn assert_thumbnail_refusal(
    case: &str,
    status: u16,
    code: Option<&str>,
    body: &Value,
    status_unmet: bool,
    thumbnail_unmet: bool,
) {
    assert_eq!(
        (status, code),
        (400, Some("BadRequestException")),
        "case {case:?}: expected 400 BadRequestException, got {status} {code:?}: {body}"
    );
    let expected = thumbnail_refusal(status_unmet, thumbnail_unmet);
    assert_eq!(
        body["message"].as_str(),
        Some(expected.as_str()),
        "case {case:?}: expected the refusal {expected:?}, got {body}"
    );
    assert!(
        body["thumbnailDetails"].is_null(),
        "case {case:?}: a refused read projects no thumbnail details, got {body}"
    );
}

/// Assert one answer is the modelled source-metadata refusal, and that nothing was projected
/// with it.
fn assert_metadata_refusal(case: &str, status: u16, code: Option<&str>, body: &Value) {
    assert_eq!(
        (status, code),
        (400, Some("BadRequestException")),
        "case {case:?}: expected 400 BadRequestException, got {status} {code:?}: {body}"
    );
    assert_eq!(
        body["message"].as_str(),
        Some(METADATA_REFUSAL),
        "case {case:?}: expected the refusal {METADATA_REFUSAL:?}, got {body}"
    );
    for member in ["flowArn", "messages", "ndiInfo", "transportMediaInfo", "timestamp"] {
        assert!(
            body[member].is_null(),
            "case {case:?}: a refused read projects no {member}, got {body}"
        );
    }
}

/// The flows the metadata rows report on: they differ in how many sources they hold, how
/// those are named, which protocol they use, whether the source is entitled rather than
/// owned, whether NDI is part of the flow, and whether the flow has a source monitoring
/// configuration at all — none of which the metadata precondition is about, which is the
/// point: the only thing that decides the answer is the flow's status.
fn metadata_flows() -> Vec<(&'static str, Value)> {
    vec![
        (
            "one rtp source",
            json!({
                "name": "gate-metadata-one-source",
                "source": { "name": "src", "protocol": "rtp", "whitelistCidr": "10.40.0.0/16" }
            }),
        ),
        (
            "two sources under active failover",
            json!({
                "name": "gate-metadata-two-sources",
                "sources": [
                    { "name": "feed-a", "protocol": "rtp", "whitelistCidr": "10.41.0.0/16" },
                    { "name": "feed-b", "protocol": "rist", "whitelistCidr": "10.42.0.0/16" }
                ],
                "sourceFailoverConfig": { "state": "ENABLED", "failoverMode": "MERGE" }
            }),
        ),
        (
            "an entitled source, which has no ingest endpoint of its own",
            json!({
                "name": "gate-metadata-entitled",
                "source": {
                    "name": "entitled-feed",
                    "entitlementArn": format!(
                        "arn:aws:mediaconnect:{REGION}:444455556666:entitlement:1-0000beef:ent"
                    )
                }
            }),
        ),
        (
            "an NDI flow whose monitoring configuration enables thumbnails",
            json!({
                "name": "gate-metadata-ndi",
                "flowSize": "LARGE",
                "ndiConfig": { "ndiState": "ENABLED", "machineName": "ndi-machine" },
                "sourceMonitoringConfig": { "thumbnailState": "ENABLED" },
                "source": {
                    "name": "ndi-feed",
                    "protocol": "ndi-speed-hq",
                    "whitelistCidr": "10.43.0.0/16"
                }
            }),
        ),
    ]
}

/// DescribeFlowSourceMetadata is refused while the flow is not running, whatever the flow
/// looks like, and is served for the SAME flow once the clock has settled the start — so the
/// status is what decides it, and the refusal is not a flow this emulator simply cannot
/// report on.
#[tokio::test]
async fn source_metadata_is_refused_until_the_flow_is_active() {
    let emulator = Emulator::new();

    for (case, flow) in metadata_flows() {
        let (flow_arn, source_names) = create(&emulator, case, flow).await;

        // As created the flow waits in STANDBY: the read is refused, and the refusal leaves
        // the flow exactly as it was.
        let (_, _, before) = emulator.describe_flow(&flow_arn).await;
        let (status, code, body) = source_metadata(&emulator, &flow_arn).await;
        assert_metadata_refusal(case, status, code.as_deref(), &body);
        let (_, _, after) = emulator.describe_flow(&flow_arn).await;
        assert_eq!(
            before, after,
            "case {case:?}: a refused read changed nothing about the flow, got {before} \
             then {after}"
        );

        // Started and settled: the same call on the same flow is served.
        start_and_settle(&emulator, case, &flow_arn).await;
        let (status, code, served) = source_metadata(&emulator, &flow_arn).await;
        assert_eq!(
            (status, code.as_deref()),
            (200, None),
            "case {case:?}: a running flow is served, got {status} {code:?}: {served}"
        );
        assert_eq!(
            served["flowArn"].as_str(),
            Some(flow_arn.as_str()),
            "case {case:?}: the served report names the flow, got {served}"
        );
        assert_eq!(
            served["messages"].as_array().map(Vec::len),
            Some(source_names.len()),
            "case {case:?}: one message per stored source ({source_names:?}), got {}",
            served["messages"]
        );
    }
}

/// One row of the thumbnail table: the `sourceMonitoringConfig` the flow is created with,
/// whether the flow is started before the read, and which preconditions the rule says that
/// leaves unmet.
struct ThumbnailCase {
    case: &'static str,
    /// The `sourceMonitoringConfig` member of the create request, or `None` for a flow that
    /// carries no source monitoring configuration at all.
    config: Option<Value>,
    /// Whether the flow is started (and the clock waited out) before the read.
    running: bool,
    /// Whether the status precondition is unmet, and whether the thumbnail-state one is:
    /// `(false, false)` is the one combination the read is served for.
    unmet: (bool, bool),
}

/// Both preconditions, crossed: each value of the modelled `ThumbnailState` enum
/// (`smithy_query(query="shape",shape="ThumbnailState")`: ENABLED | DISABLED) plus the two
/// ways a flow states nothing about thumbnails (a configuration that never mentions them,
/// and no configuration at all), each read on a flow that is not running and on the same kind
/// of flow once it is. `UPDATING`, the other status the message names, is not reachable
/// through this service's own lifecycle calls, so no row drives it.
fn thumbnail_cases() -> Vec<ThumbnailCase> {
    vec![
        ThumbnailCase {
            case: "standby, thumbnails ENABLED",
            config: Some(json!({ "thumbnailState": "ENABLED" })),
            running: false,
            unmet: (true, false),
        },
        ThumbnailCase {
            case: "running, thumbnails ENABLED",
            config: Some(json!({ "thumbnailState": "ENABLED" })),
            running: true,
            unmet: (false, false),
        },
        ThumbnailCase {
            case: "standby, thumbnails DISABLED",
            config: Some(json!({ "thumbnailState": "DISABLED" })),
            running: false,
            unmet: (true, true),
        },
        ThumbnailCase {
            case: "running, thumbnails DISABLED",
            config: Some(json!({ "thumbnailState": "DISABLED" })),
            running: true,
            unmet: (false, true),
        },
        ThumbnailCase {
            case: "standby, a configuration that never mentions thumbnails",
            config: Some(json!({ "contentQualityAnalysisState": "ENABLED" })),
            running: false,
            unmet: (true, true),
        },
        ThumbnailCase {
            case: "running, a configuration that never mentions thumbnails",
            config: Some(json!({
                "audioMonitoringSettings": [
                    { "silentAudio": { "state": "ENABLED", "thresholdSeconds": 12 } }
                ]
            })),
            running: true,
            unmet: (false, true),
        },
        ThumbnailCase {
            case: "standby, no source monitoring configuration at all",
            config: None,
            running: false,
            unmet: (true, true),
        },
        ThumbnailCase {
            case: "running, no source monitoring configuration at all",
            config: None,
            running: true,
            unmet: (false, true),
        },
    ]
}

/// DescribeFlowSourceThumbnail is served exactly for a flow that is running AND has
/// thumbnails enabled; every other combination is refused with AWS's message naming the
/// preconditions that flow fails — both of them where both are unmet — and projects nothing.
#[tokio::test]
async fn source_thumbnail_is_refused_unless_the_flow_runs_with_thumbnails_enabled() {
    let emulator = Emulator::new();

    for (index, row) in thumbnail_cases().into_iter().enumerate() {
        let ThumbnailCase {
            case,
            config,
            running,
            unmet: (status_unmet, thumbnail_unmet),
        } = row;
        let mut request = json!({
            "name": format!("gate-thumbnail-{index}"),
            "sources": [
                { "name": "feed-a", "protocol": "rtp", "whitelistCidr": "10.44.0.0/16" },
                { "name": "feed-b", "protocol": "rtp", "whitelistCidr": "10.45.0.0/16" }
            ],
            "sourceFailoverConfig": { "state": "ENABLED" }
        });
        if let Some(config) = config {
            request["sourceMonitoringConfig"] = config;
        }
        let (flow_arn, source_names) = create(&emulator, case, request).await;
        if running {
            start_and_settle(&emulator, case, &flow_arn).await;
        }

        let (_, _, before) = emulator.describe_flow(&flow_arn).await;
        let (status, code, body) = source_thumbnail(&emulator, &flow_arn).await;
        if status_unmet || thumbnail_unmet {
            assert_thumbnail_refusal(
                case,
                status,
                code.as_deref(),
                &body,
                status_unmet,
                thumbnail_unmet,
            );
        } else {
            assert_eq!(
                (status, code.as_deref()),
                (200, None),
                "case {case:?}: both preconditions are met, so the read is served, got \
                 {status} {code:?}: {body}"
            );
            let details = &body["thumbnailDetails"];
            assert_eq!(
                details["flowArn"].as_str(),
                Some(flow_arn.as_str()),
                "case {case:?}: the served details name the flow, got {body}"
            );
            assert_eq!(
                details["thumbnailMessages"].as_array().map(Vec::len),
                Some(source_names.len()),
                "case {case:?}: one message per stored source ({source_names:?}), got {}",
                details["thumbnailMessages"]
            );
            assert_no_image_is_reported(case, details);
        }

        // Whichever way it answered, the read reported: the flow is unchanged.
        let (_, _, after) = emulator.describe_flow(&flow_arn).await;
        assert_eq!(
            before, after,
            "case {case:?}: the read changed nothing about the flow, got {before} then \
             {after}"
        );
    }
}
