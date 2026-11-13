// SPDX-License-Identifier: CC0-1.0

//! DescribeFlowSourceMetadata, end to end over HTTP.
//!
//! The rule under test: the operation REPORTS the source metadata this service holds for
//! the flow its `{FlowArn}` label resolves to in the store — so it answers
//! `NotFoundException` for a label the store does not hold, and for a RUNNING flow it holds
//! it answers the stored flow's ARN plus one `messages` entry per STORED source, with
//! `ndiInfo` exactly for a flow NDI is associated with. A flow that is not running is
//! refused instead, which is the subject of
//! `tests/all/flow_source_report_preconditions.rs`; the fixtures here are therefore started
//! before they are read. Nothing in this emulator ingests media, so no transport stream has
//! ever been observed: `transportMediaInfo` and `timestamp` are absent for every flow, and
//! the NDI block reports an empty discovery rather than an invented sender.
//!
//! Each rule is asserted over a TABLE whose rows differ along what it generalises over —
//! how many sources the flow holds, what they are named, which protocol they use, whether
//! NDI is enabled by `ndiConfig` or by a source's `ndiSourceSettings`, and which status
//! the flow is in — rather than over one recorded request. Every case drives the real
//! request path (routing, restJson1 deserialization, constraint validation, handler,
//! serialization) through the harness [`crate::flow_lifecycle::Emulator`], so what these
//! tests assert is what a MediaConnect client sees on the wire.

use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::flow_lifecycle::{Emulator, REGION};
use crate::flow_source_report_preconditions::METADATA_REFUSAL;

/// The `MessageDetail$Code` values this operation reports, written out here rather than
/// read from the crate so that a change to the handler's constants cannot quietly move
/// what these tests assert.
///
/// `NO_SOURCE_METADATA` is the code for a source of a running flow nothing has arrived on,
/// and `NO_NDI_SENDER_DISCOVERED` the code of the NDI block's own message.
const CODE_NO_SOURCE_METADATA: &str = "NO_SOURCE_METADATA";
/// See [`CODE_NO_SOURCE_METADATA`].
const CODE_NO_NDI_SENDER_DISCOVERED: &str = "NO_NDI_SENDER_DISCOVERED";

/// The status a created flow waits in, and the one a started flow settles into
/// (`smithy_query(query="shape",shape="Status")`).
const STATUS_STANDBY: &str = "STANDBY";
/// See [`STATUS_STANDBY`].
const STATUS_ACTIVE: &str = "ACTIVE";

/// Call DescribeFlowSourceMetadata through its modelled HTTP binding
/// (`smithy_query(query="operation",operation="DescribeFlowSourceMetadata")`: GET
/// `/v1/flows/{FlowArn}/source-metadata`).
///
/// `pub(crate)` because the precondition suite drives the same binding.
pub(crate) async fn source_metadata(
    emulator: &Emulator,
    flow_arn: &str,
) -> (u16, Option<String>, Value) {
    emulator
        .send("GET", &format!("/v1/flows/{flow_arn}/source-metadata"), None)
        .await
}

/// How long a poll loop below may run before it gives up. Generous: every transition it
/// waits for is due one transition span after the call that started it, and this only bounds
/// a loop that would otherwise never end.
pub(crate) const POLL_DEADLINE: Duration = Duration::from_secs(120);

/// How long a poll loop waits before looking again.
pub(crate) const BETWEEN_READS: Duration = Duration::from_millis(25);

/// Start the flow and wait for the CONDITION the source reports are gated on: the clock has
/// settled it into `ACTIVE`. Polls DescribeFlow rather than sleeping a span, so the fixture
/// is ready whatever the host's speed.
///
/// `pub(crate)` because every suite that reads a flow's source reports needs a running one.
pub(crate) async fn start_and_settle(emulator: &Emulator, case: &str, flow_arn: &str) {
    let (status, code, body) = emulator
        .send("POST", &format!("/v1/flows/start/{flow_arn}"), None)
        .await;
    assert_eq!(
        (status, code.as_deref()),
        (202, None),
        "case {case:?}: StartFlow expected 202, got {status} {code:?}: {body}"
    );
    settled_status(emulator, case, flow_arn, STATUS_ACTIVE).await;
}

/// Poll DescribeFlow until it reports `expected` as the flow's status, or [`POLL_DEADLINE`]
/// passes — the condition, not a span of time.
pub(crate) async fn settled_status(
    emulator: &Emulator,
    case: &str,
    flow_arn: &str,
    expected: &str,
) {
    let started = Instant::now();
    loop {
        let (status, code, described) = emulator.describe_flow(flow_arn).await;
        assert_eq!(
            (status, code.as_deref()),
            (200, None),
            "case {case:?}: DescribeFlow must answer 200, got {status} {code:?}: {described}"
        );
        if described["flow"]["status"].as_str() == Some(expected) {
            return;
        }
        assert!(
            started.elapsed() < POLL_DEADLINE,
            "case {case:?}: the clock must settle the flow into {expected}, last read \
             {described}"
        );
        tokio::time::sleep(BETWEEN_READS).await;
    }
}

/// Create a flow and return `(FlowArn, the source names DescribeFlow reports for it)`.
///
/// `pub(crate)` because the source-thumbnail suite reports on the same fixture flows
/// (`tests/all/flow_source_thumbnail.rs`), so there is one way to set one up.
pub(crate) async fn create(emulator: &Emulator, case: &str, body: Value) -> (String, Vec<String>) {
    let (status, code, created) = emulator.create_flow(body).await;
    assert_eq!(
        (status, code.as_deref()),
        (201, None),
        "case {case:?}: the fixture flow must be created, got {status} {code:?}: {created}"
    );
    let flow_arn = created["flow"]["flowArn"]
        .as_str()
        .expect("a created flow carries its ARN")
        .to_string();
    let (status, code, described) = emulator.describe_flow(&flow_arn).await;
    assert_eq!(
        (status, code.as_deref()),
        (200, None),
        "case {case:?}: DescribeFlow must answer 200, got {status} {code:?}: {described}"
    );
    let names = described["flow"]["sources"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|source| source["name"].as_str().unwrap_or_default().to_string())
        .collect();
    (flow_arn, names)
}

/// The `(code, resourceName)` pairs of a `messages` list, in the order it reports them.
///
/// `pub(crate)`: `ThumbnailDetails$ThumbnailMessages` is the same `__listOfMessageDetail`,
/// read the same way by `tests/all/flow_source_thumbnail.rs`.
pub(crate) fn coded(messages: &Value) -> Vec<(String, String)> {
    messages
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|message| {
            (
                message["code"].as_str().unwrap_or_default().to_string(),
                message["resourceName"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
            )
        })
        .collect()
}

/// Assert that every message of `messages` carries a non-empty `message` string, so the
/// list is a status report rather than bare codes. `pub(crate)` for the same reason as
/// [`coded`].
pub(crate) fn assert_messages_are_worded(case: &str, messages: &Value) {
    for message in messages.as_array().cloned().unwrap_or_default() {
        assert!(
            message["message"]
                .as_str()
                .is_some_and(|text| !text.trim().is_empty()),
            "case {case:?}: every message carries wording, got {message}"
        );
    }
}

/// One row of the reporting table: a CreateFlow body, and what the rule says
/// DescribeFlowSourceMetadata must report for the flow it stores.
struct ReportCase {
    case: &'static str,
    /// The flow as the create request states it.
    flow: Value,
    /// Whether NDI is associated with the flow, so `ndiInfo` is part of its source.
    ndi: bool,
}

/// The rows span what the rule generalises over: how many sources the flow holds (one, and
/// the two a flow under active failover may hold), how they are named, which protocol they
/// use, and every way NDI becomes part of a flow's source — `NdiConfig$NdiState` `ENABLED`
/// on the flow, and each of the two markers of an NDI source the crate reads
/// (`crate::state::uses_ndi_transport`): the NDI protocol of the modelled `Protocol` enum
/// (`smithy_query(query="shape",shape="Protocol")`: `ndi-speed-hq`) and
/// `SetSourceRequest$NdiSourceSettings`, each on its own and both together. The negative
/// rows are a flow whose `ndiConfig` names NDI `DISABLED` and flows that mention NDI
/// nowhere.
fn report_cases() -> Vec<ReportCase> {
    vec![
        ReportCase {
            case: "one rtp source",
            flow: json!({
                "name": "metadata-one-source",
                "source": { "name": "src", "protocol": "rtp", "whitelistCidr": "10.0.0.0/16" }
            }),
            ndi: false,
        },
        ReportCase {
            case: "one srt-listener source named unlike the flow",
            flow: json!({
                "name": "metadata-srt",
                "source": {
                    "name": "primary-feed",
                    "protocol": "srt-listener",
                    "whitelistCidr": "192.0.2.0/24",
                    "minLatency": 2000
                }
            }),
            ndi: false,
        },
        ReportCase {
            case: "two sources under active failover",
            flow: json!({
                "name": "metadata-two-sources",
                "sources": [
                    { "name": "feed-a", "protocol": "rtp", "whitelistCidr": "10.1.0.0/16" },
                    { "name": "feed-b", "protocol": "rist", "whitelistCidr": "10.2.0.0/16" }
                ],
                "sourceFailoverConfig": { "state": "ENABLED", "failoverMode": "MERGE" }
            }),
            ndi: false,
        },
        ReportCase {
            case: "an entitled source, which has no ingest endpoint of its own",
            flow: json!({
                "name": "metadata-entitled",
                "source": {
                    "name": "entitled-feed",
                    "entitlementArn": format!(
                        "arn:aws:mediaconnect:{REGION}:444455556666:entitlement:1-0000beef:ent"
                    )
                }
            }),
            ndi: false,
        },
        ReportCase {
            case: "a flow with NDI enabled by its ndiConfig",
            flow: json!({
                "name": "metadata-ndi-config",
                "flowSize": "LARGE",
                "ndiConfig": { "ndiState": "ENABLED", "machineName": "ndi-machine" },
                "source": { "name": "src", "protocol": "rtp", "whitelistCidr": "10.3.0.0/16" }
            }),
            ndi: true,
        },
        ReportCase {
            case: "a flow whose ndiConfig names NDI DISABLED",
            flow: json!({
                "name": "metadata-ndi-off",
                "ndiConfig": { "ndiState": "DISABLED" },
                "source": { "name": "src", "protocol": "rtp", "whitelistCidr": "10.4.0.0/16" }
            }),
            ndi: false,
        },
        ReportCase {
            case: "a flow whose source is an NDI source by protocol and settings",
            flow: json!({
                "name": "metadata-ndi-source",
                "flowSize": "LARGE",
                "source": {
                    "name": "ndi-feed",
                    "protocol": "ndi-speed-hq",
                    "whitelistCidr": "10.5.0.0/16",
                    "ndiSourceSettings": { "sourceName": "STUDIO (CAM 1)" }
                }
            }),
            ndi: true,
        },
        ReportCase {
            case: "a flow whose source is an NDI source by protocol alone",
            flow: json!({
                "name": "metadata-ndi-protocol-only",
                "flowSize": "LARGE",
                "source": {
                    "name": "ndi-feed",
                    "protocol": "ndi-speed-hq",
                    "whitelistCidr": "10.10.0.0/16"
                }
            }),
            ndi: true,
        },
        ReportCase {
            case: "a flow whose source is an NDI source by its ndiSourceSettings alone",
            flow: json!({
                "name": "metadata-ndi-settings-only",
                "flowSize": "LARGE",
                "source": {
                    "name": "ndi-feed",
                    "protocol": "rtp",
                    "whitelistCidr": "10.11.0.0/16",
                    "ndiSourceSettings": { "sourceName": "STUDIO (CAM 2)" }
                }
            }),
            ndi: true,
        },
    ]
}

/// DescribeFlowSourceMetadata reports the STORED flow: its ARN, one coded message per
/// stored source naming that source, `ndiInfo` exactly for a flow NDI is associated with,
/// and no transport media or metadata timestamp for any flow — because nothing has been
/// ingested.
///
/// Every fixture is STARTED first: the report is gated on the flow running (see
/// `tests/all/flow_source_report_preconditions.rs`), so a running flow is what this rule is
/// about.
#[tokio::test]
async fn source_metadata_reports_the_stored_flow_and_its_sources() {
    let emulator = Emulator::new();

    for ReportCase { case, flow, ndi } in report_cases() {
        let (flow_arn, source_names) = create(&emulator, case, flow).await;
        assert!(
            !source_names.is_empty(),
            "case {case:?}: the fixture flow must hold at least one source"
        );
        start_and_settle(&emulator, case, &flow_arn).await;

        let (status, code, body) = source_metadata(&emulator, &flow_arn).await;
        assert_eq!(
            (status, code.as_deref()),
            (200, None),
            "case {case:?}: expected 200 with no error, got {status} {code:?}: {body}"
        );
        assert_eq!(
            body["flowArn"].as_str(),
            Some(flow_arn.as_str()),
            "case {case:?}: the response names the flow it was performed on, got {body}"
        );

        // One message per stored source, in the flow's own source order, each naming its
        // source and coded for a running flow nothing has arrived on.
        let expected: Vec<(String, String)> = source_names
            .iter()
            .map(|name| (CODE_NO_SOURCE_METADATA.to_string(), name.clone()))
            .collect();
        assert_eq!(
            coded(&body["messages"]),
            expected,
            "case {case:?}: expected {expected:?}, got {}",
            body["messages"]
        );
        assert_messages_are_worded(case, &body["messages"]);

        // Nothing is ingested, so there is no transport stream to describe and no instant
        // at which the source's metadata last changed.
        assert!(
            body["transportMediaInfo"].is_null(),
            "case {case:?}: no transport media info is reported for a flow nothing has \
             been ingested on, got {}",
            body["transportMediaInfo"]
        );
        assert!(
            body["timestamp"].is_null(),
            "case {case:?}: no metadata timestamp is reported for a flow with no source \
             metadata, got {}",
            body["timestamp"]
        );

        if ndi {
            let ndi_info = &body["ndiInfo"];
            assert!(
                ndi_info.is_object(),
                "case {case:?}: a flow NDI is associated with reports an ndiInfo block, \
                 got {body}"
            );
            assert_eq!(
                ndi_info["discoveredSources"].as_array().map(Vec::len),
                Some(0),
                "case {case:?}: no NDI sender can be discovered, got {}",
                ndi_info["discoveredSources"]
            );
            assert_eq!(
                ndi_info["mediaInfo"]["streams"].as_array().map(Vec::len),
                Some(0),
                "case {case:?}: no NDI media stream has been observed, got {}",
                ndi_info["mediaInfo"]
            );
            assert!(
                ndi_info["activeSource"].is_null(),
                "case {case:?}: no NDI sender is active, got {}",
                ndi_info["activeSource"]
            );
            assert_eq!(
                coded(&ndi_info["messages"])
                    .into_iter()
                    .map(|(code, _)| code)
                    .collect::<Vec<String>>(),
                vec![CODE_NO_NDI_SENDER_DISCOVERED.to_string()],
                "case {case:?}: the NDI block says why it is empty, got {}",
                ndi_info["messages"]
            );
            assert_messages_are_worded(case, &ndi_info["messages"]);
        } else {
            assert!(
                body["ndiInfo"].is_null(),
                "case {case:?}: NDI is no part of this flow's source, so no ndiInfo is \
                 reported, got {}",
                body["ndiInfo"]
            );
        }
    }
}

/// The read REPORTS: calling it twice answers the same body, and it moves neither the
/// flow's status nor its sources, so a later caller sees exactly what an earlier one did.
#[tokio::test]
async fn source_metadata_is_a_report_that_changes_nothing() {
    let emulator = Emulator::new();
    let case = "repeated reads of one flow";
    let (flow_arn, source_names) = create(
        &emulator,
        case,
        json!({
            "name": "metadata-stable",
            "sources": [
                { "name": "feed-a", "protocol": "rtp", "whitelistCidr": "10.6.0.0/16" },
                { "name": "feed-b", "protocol": "rtp", "whitelistCidr": "10.7.0.0/16" }
            ],
            "sourceFailoverConfig": { "state": "ENABLED" }
        }),
    )
    .await;
    start_and_settle(&emulator, case, &flow_arn).await;

    let (_, _, first) = source_metadata(&emulator, &flow_arn).await;
    let (_, _, second) = source_metadata(&emulator, &flow_arn).await;
    assert_eq!(
        first, second,
        "case {case:?}: two reads of an unchanged flow answer the same body, got {first} \
         then {second}"
    );

    let (status, code, described) = emulator.describe_flow(&flow_arn).await;
    assert_eq!(
        (status, code.as_deref()),
        (200, None),
        "case {case:?}: DescribeFlow must answer 200, got {status} {code:?}: {described}"
    );
    assert_eq!(
        described["flow"]["status"].as_str(),
        Some(STATUS_ACTIVE),
        "case {case:?}: reading the metadata did not move the flow's status, got {}",
        described["flow"]["status"]
    );
    let names: Vec<String> = described["flow"]["sources"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|source| source["name"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(
        names, source_names,
        "case {case:?}: reading the metadata did not change the flow's sources, got \
         {names:?}"
    );
}

/// The read is served EXACTLY while the flow is running: refused as created, refused for
/// every instant of the start that has not settled yet, served once the flow is `ACTIVE`,
/// and refused again once the stop has taken it out of `ACTIVE` — each time with the status
/// and the code that go with the state the flow is in.
///
/// A step inside a transition is not raced: the loops below poll until the answer FLIPS and
/// assert that every answer before the flip is the one the earlier state requires, so what
/// is pinned is the pairing of state with answer rather than the test's luck with the clock.
#[tokio::test]
async fn the_read_is_served_exactly_while_the_flow_is_running() {
    let emulator = Emulator::new();
    let case = "a flow driven through its lifecycle";
    let (flow_arn, source_names) = create(
        &emulator,
        case,
        json!({
            "name": "metadata-lifecycle",
            "source": { "name": "src", "protocol": "rtp", "whitelistCidr": "10.8.0.0/16" }
        }),
    )
    .await;

    // As created the flow waits in STANDBY, which is not a status this report is served in.
    let (status, code, body) = source_metadata(&emulator, &flow_arn).await;
    assert_refusal("a created flow waits in STANDBY", status, code.as_deref(), &body);
    let (_, _, described) = emulator.describe_flow(&flow_arn).await;
    assert_eq!(
        described["flow"]["status"].as_str(),
        Some(STATUS_STANDBY),
        "case {case:?}: a created flow waits in {STATUS_STANDBY}, got {}",
        described["flow"]["status"]
    );

    // Started: STARTING until the clock settles it ACTIVE. Every answer before the flow is
    // ACTIVE is the refusal, and the first served answer is the running report.
    let (status, code, body) = emulator
        .send("POST", &format!("/v1/flows/start/{flow_arn}"), None)
        .await;
    assert_eq!(
        (status, code.as_deref()),
        (202, None),
        "case {case:?}: StartFlow expected 202, got {status} {code:?}: {body}"
    );
    let served = poll_until_served(&emulator, "a started flow", &flow_arn).await;
    let expected: Vec<(String, String)> = source_names
        .iter()
        .map(|name| (CODE_NO_SOURCE_METADATA.to_string(), name.clone()))
        .collect();
    assert_eq!(
        coded(&served["messages"]),
        expected,
        "case {case:?}: a running flow reports one running message per source, expected \
         {expected:?}, got {}",
        served["messages"]
    );
    assert!(
        served["messages"][0]["message"]
            .as_str()
            .is_some_and(|wording| wording.contains(STATUS_ACTIVE)),
        "case {case:?}: the wording names the status the flow is in, got {}",
        served["messages"][0]["message"]
    );
    // Never any media: a running flow of this emulator receives nothing either.
    assert!(
        served["transportMediaInfo"].is_null() && served["timestamp"].is_null(),
        "case {case:?}: no transport media info and no metadata timestamp, got {served}"
    );

    // Stopped: STOPPING until the clock settles it STANDBY. Every answer before it leaves
    // ACTIVE is still the served report, and from then on it is the refusal.
    let (status, code, body) = emulator
        .send("POST", &format!("/v1/flows/stop/{flow_arn}"), None)
        .await;
    assert_eq!(
        (status, code.as_deref()),
        (202, None),
        "case {case:?}: StopFlow expected 202, got {status} {code:?}: {body}"
    );
    poll_until_refused(&emulator, "a stopped flow", &flow_arn).await;
    settled_status(&emulator, case, &flow_arn, STATUS_STANDBY).await;
}

/// Poll the read until it is SERVED, asserting that every answer before that is the modelled
/// refusal — so the loop waits for the condition and still pins what a flow that is not
/// running yet is answered with. Returns the first served body.
async fn poll_until_served(emulator: &Emulator, case: &str, flow_arn: &str) -> Value {
    let started = Instant::now();
    loop {
        let (status, code, body) = source_metadata(emulator, flow_arn).await;
        if status == 200 {
            assert!(
                code.is_none(),
                "case {case:?}: a served read carries no error type, got {code:?}: {body}"
            );
            return body;
        }
        assert_refusal(case, status, code.as_deref(), &body);
        assert!(
            started.elapsed() < POLL_DEADLINE,
            "case {case:?}: the read must be served once the flow is running, last answer \
             {status} {code:?}: {body}"
        );
        tokio::time::sleep(BETWEEN_READS).await;
    }
}

/// Poll the read until it is REFUSED, asserting that every answer before that is a served
/// 200 — the mirror of [`poll_until_served`].
async fn poll_until_refused(emulator: &Emulator, case: &str, flow_arn: &str) {
    let started = Instant::now();
    loop {
        let (status, code, body) = source_metadata(emulator, flow_arn).await;
        if status != 200 {
            assert_refusal(case, status, code.as_deref(), &body);
            return;
        }
        assert!(
            started.elapsed() < POLL_DEADLINE,
            "case {case:?}: the read must be refused once the flow stops running, last \
             answer {status} {code:?}: {body}"
        );
        tokio::time::sleep(BETWEEN_READS).await;
    }
}

/// Assert one answer is the modelled refusal this report is gated by, worded as AWS words
/// it ([`METADATA_REFUSAL`]).
fn assert_refusal(case: &str, status: u16, code: Option<&str>, body: &Value) {
    assert_eq!(
        (status, code),
        (400, Some("BadRequestException")),
        "case {case:?}: expected 400 BadRequestException, got {status} {code:?}: {body}"
    );
    assert_eq!(
        body["message"].as_str(),
        Some(METADATA_REFUSAL),
        "case {case:?}: the refusal carries AWS's message for this class, got {body}"
    );
}

/// The `{FlowArn}` label is resolved against the STORE once it satisfies the constraint
/// the model states for it: a label of the modelled form that the store does not hold is
/// refused 404 `NotFoundException` whose message is the label the request named, while a
/// label outside that form is refused by the modelled constraint itself (400
/// `BadRequestException`) before any lookup.
///
/// `DescribeFlowSourceMetadataRequest$FlowArn` declares no pattern of its own
/// (`smithy_query(query="operation",operation="DescribeFlowSourceMetadata")`), so the
/// narrow `FlowArn` shape pattern is the only one the model states for it — unlike
/// `DescribeFlowRequest$FlowArn`, whose own looser pattern is why that label reaches its
/// handler unjudged (see the note on `DescribeFlowInput` in the crate's `ops::types`).
#[tokio::test]
async fn an_unheld_flow_label_is_refused_not_found() {
    let emulator = Emulator::new();

    // A flow that existed and does not any more resolves to nothing, like one that never
    // did.
    let (deleted, _) = create(
        &emulator,
        "deleted flow",
        json!({
            "name": "metadata-doomed",
            "source": { "name": "src", "protocol": "rtp", "whitelistCidr": "10.9.0.0/16" }
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
        let (status, code, body) = source_metadata(&emulator, &label).await;
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
        let (status, code, body) = source_metadata(&emulator, &label).await;
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
