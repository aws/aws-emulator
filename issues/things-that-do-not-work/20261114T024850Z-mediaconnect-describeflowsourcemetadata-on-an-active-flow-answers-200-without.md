Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: mediaconnect.describe-flow-source-metadata-not-yet-available
Issue-Shape-Id: com.amazonaws.mediaconnect#MediaConnect
Issue-Key: mediaconnect.describe-flow-source-metadata-not-yet-available|run#16|AssertionError|assert "TransportMediaInfo" in metadata, (
Issue-Commit: c035be45a7182c86e523045b713949a5a5fe5fd5

# DescribeFlowSourceMetadata on an ACTIVE flow answers 200 without the modelled TransportMediaInfo member (and without Timestamp)

> _Note: this issue was identified against the code base at commit `c035be45a7182c86e523045b713949a5a5fe5fd5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeFlowSourceMetadata

## Description
One net-new root cause survives this behavioral replay of the MediaConnect emulator. 12 of the 13 mediaconnect failures are marked OWNED by an open issue by the replay report and are skipped; this is the only one the report leaves unclaimed.

NOTE ON OVERLAP (the replay report does NOT mark this check as covered, so it is filed here): open issue 20261029T202226Z-mediaconnect-deletebridge-erases-the-bridge-instead-of-retiring.md describes this same residue inside its 'Cause 2 of 4', but it is keyed on that check's OLD failure frame (`mediaconnect.describe-flow-source-metadata-not-yet-available|_client_error#4|AssertionError`, i.e. the STANDBY call succeeding with 200). That half is now correct — the STANDBY call is refused HTTP 400 BadRequestException with exactly 'Bad request: The flow must be in ACTIVE state to perform this action', and the check now gets all the way past it. The failure has moved to a DIFFERENT frame (run#44, the ACTIVE-flow assertion), so the report no longer reports it as owned and the check is unclaimed. A fixer closing this issue should re-check that file: the ACTIVE half of its Cause 2 is the same defect and the two should close together.

=== Cause 1 of 1: an ACTIVE flow's source metadata is reported with TransportMediaInfo absent ===
AWS: Smithy model services/service-com-amazonaws-mediaconnect-mediaconnect/smithy-model/com-amazonaws-mediaconnect-mediaconnect.json — DescribeFlowSourceMetadata, GET /v1/flows/{FlowArn}/source-metadata, success 200, output DescribeFlowSourceMetadataResponse {FlowArn (flowArn), Messages (messages), Timestamp (timestamp, date-time), TransportMediaInfo (transportMediaInfo), NdiInfo (ndiInfo)}; TransportMediaInfo is 'Information about the flow's transport media' and is the structure {Programs (required), TsId}; the operation's doc says it 'is used to view information about the flow's source transport stream and programs'. Modelled errors include BadRequestException (400). Recorded behavior: the call is gated on the flow's own Status and NOT on real ingest — on a STANDBY flow it is refused 400 BadRequestException with the fixed text 'Bad request: The flow must be in ACTIVE state to perform this action' (no request- or state-derived values in that message), and the IDENTICAL call then succeeds HTTP 200 WITH a TransportMediaInfo member once the flow reaches ACTIVE, with no media ever sent (the recording for that account shows the structure present with an empty Programs list; the per-run TsId/program/PID/codec values are not part of the requirement, only that the member is served for an ACTIVE flow).
Emulator: the STANDBY refusal now matches (status, code and message all correct — that half of the check passes). For the ACTIVE flow the handler answers HTTP 200 but omits the member entirely: the response body carries only FlowArn and Messages (botocore sees keys ['FlowArn', 'Messages', 'ResponseMetadata']). services/service-com-amazonaws-mediaconnect-mediaconnect/src/ops/describe_flow_source_metadata.rs constructs its output with `transport_media_info: None` and `timestamp: None` unconditionally, by deliberate design: the module doc argues the member is 'ABSENT rather than carrying a synthesised stream' because this emulator ingests no media, and substitutes an emulator-authored Messages entry (`MessageDetail.Code = "NO_SOURCE_METADATA"`, message 'The flow is <status>, and no content has been received on this source, so its source metadata is not available yet.') built by `source_metadata_messages`. Timestamp ('the timestamp of the most recent change in metadata for this flow's source') is suppressed by the same reasoning. The result is that the ACTIVE and non-ACTIVE answers are structurally indistinguishable on the one member that distinguishes them in AWS.
Why: a client that does what the operation is for — start a flow, wait for ACTIVE, then read the source transport stream's TsId and programs — finds the member missing on a 200 response and raises KeyError (botocore) / dereferences a nil structure (typed SDKs) where AWS always supplies it. Because the member is absent rather than empty, a caller cannot tell 'the flow is ACTIVE and the stream has no programs yet' from 'this response is a different shape than the API models', so it has no way to poll for program metadata at all; the only signal left is free-form Messages[].Code prose that is this emulator's own vocabulary and that AWS does not use this way. Monitoring and failover tooling keyed on TransportMediaInfo/Timestamp sees a permanently empty surface with no error to branch on.
Fix: the DescribeFlowSourceMetadata handler's projection — for a flow whose stored status is ACTIVE, project a TransportMediaInfo from the flow's source state (with Programs empty while nothing has been demultiplexed, which is what the recording shows AWS returning) and supply the accompanying Timestamp, instead of hard-coding both to None; the emulator-authored NO_SOURCE_METADATA Messages entry may stay alongside it but must not be the substitute for the member.
Traces: mediaconnect.describe-flow-source-metadata-not-yet-available. Code: services/service-com-amazonaws-mediaconnect-mediaconnect/src/ops/describe_flow_source_metadata.rs (`handle`, lines 252-263 — `transport_media_info: None`, `timestamp: None`; `source_metadata_messages`, lines 288-301; the design note in the module docs, lines 16-41).

## Evidence
Replay report: mediaconnect 15/28 passed (54%), 13 failed (8 mismatch, 5 error). 12 of the 13 are marked 'OWNED by ... already filed; skip it.' The one surviving failure:

[mismatch] mediaconnect.describe-flow-source-metadata-not-yet-available
  sequence: create_flow(Name=..., Source={'Name':'source1','Protocol':'rtp','WhitelistCidr':'203.0.113.0/24'}) -> describe_flow_source_metadata (STANDBY) -> describe_flow -> start_flow -> poll describe_flow until Flow.Status=='ACTIVE' -> describe_flow_source_metadata again, with NO media sent.
  passing prefix: the STANDBY call is refused HTTP 400 BadRequestException with Message exactly 'Bad request: The flow must be in ACTIVE state to perform this action'; DescribeFlow reports the flow STANDBY; StartFlow drives it to ACTIVE.
  expected: the identical DescribeFlowSourceMetadata call on the ACTIVE flow answers HTTP 200 with a 'TransportMediaInfo' member.
  actual:
    File "check_mediaconnect_describe_flow_source_metadata_not_yet_available.py", line 44, in run
      assert "TransportMediaInfo" in metadata, (
    AssertionError: expected TransportMediaInfo once ACTIVE, got keys=['FlowArn', 'Messages', 'ResponseMetadata']

So the status (200) and FlowArn are correct; the divergence is the ABSENCE of the modelled TransportMediaInfo member (and of Timestamp) on a successful response for an ACTIVE flow. No error code or message is involved in this half of the check — nothing was refused.

Source of the omission — services/service-com-amazonaws-mediaconnect-mediaconnect/src/ops/describe_flow_source_metadata.rs:
  252  Ok(DescribeFlowSourceMetadataOutput {
  255      flow_arn: Some(flow.flow_arn.clone()),
  256      messages: Some(source_metadata_messages(flow)),
  260      timestamp: None,
  261      transport_media_info: None,
  262      ndi_info: ndi_source_metadata_info(flow),
  263  })
with the module doc (lines 16-41) stating the member is intentionally absent because 'this emulator ingests no media'. The status gate above it (`crate::state::flow_source_metadata_refusal`, lines 243-250) is the part that now matches AWS.

Smithy evidence (model_path services/service-com-amazonaws-mediaconnect-mediaconnect/smithy-model/com-amazonaws-mediaconnect-mediaconnect.json):
  DescribeFlowSourceMetadata output = {FlowArn: String(flowArn), Messages: __listOfMessageDetail(messages), Timestamp: Timestamp(timestamp,date-time), TransportMediaInfo: TransportMediaInfo(transportMediaInfo), NdiInfo: NdiSourceMetadataInfo(ndiInfo)}; http GET /v1/flows/{FlowArn}/source-metadata, code 200.
  TransportMediaInfo = structure {Programs: __listOfTransportStreamProgram (required), TsId: Integer} — 'The metadata of the transport stream in the current flow's source.'

## Steps to Reproduce
1. Boot the emulator and point a botocore 'mediaconnect' client at its endpoint.
2. f = mc.create_flow(Name='f1', Source={'Name':'source1','Protocol':'rtp','WhitelistCidr':'203.0.113.0/24'})['Flow']['FlowArn']  (the flow is STANDBY).
3. mc.describe_flow_source_metadata(FlowArn=f) -> correctly refused HTTP 400 BadRequestException 'Bad request: The flow must be in ACTIVE state to perform this action'.
4. mc.start_flow(FlowArn=f); poll mc.describe_flow(FlowArn=f)['Flow']['Status'] until it is 'ACTIVE'. Send no media.
5. resp = mc.describe_flow_source_metadata(FlowArn=f); print(sorted(resp)) -> ['FlowArn', 'Messages', 'ResponseMetadata']. 'TransportMediaInfo' is absent (so is 'Timestamp'), where AWS answers 200 with the TransportMediaInfo structure present; resp['Messages'][0]['Code'] is the emulator's own 'NO_SOURCE_METADATA'.
6. Equivalently: run run_behavioral_suite and observe mediaconnect.describe-flow-source-metadata-not-yet-available failing at the 'expected TransportMediaInfo once ACTIVE' assertion.
