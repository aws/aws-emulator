Issue-Source: trace-sweep-divergence
Issue-Trace: appflow-s3-to-s3-flow-run-transfers-records
Issue-Step: task#8
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: appflow-s3-to-s3-flow-run-transfers-records
Issue-Shape-Id: com.amazonaws.appflow#SandstoneConfigurationServiceLambda
Issue-Key: appflow|DescribeFlowExecutionRecords|2xx->2xx|$.flowExecutions[*].executionStartTime
Issue-Key: appflow|DescribeFlowExecutionRecords|2xx->2xx|$.flowExecutions[*].lastUpdatedAt
Issue-Key: appflow|DescribeFlowExecutionRecords|2xx->2xx|$.flowExecutions[*].lastUpdatedTime
Issue-Key: appflow|DescribeFlowExecutionRecords|2xx->2xx|$.flowExecutions[*].startedAt
Issue-Commit: fa605e3beb5560d80c09dff54081711248679782

# AppFlow DescribeFlowExecutionRecords serializes flow-run timestamps with sub-millisecond precision

> _Note: this issue was identified against the code base at commit `fa605e3beb5560d80c09dff54081711248679782`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeFlowExecutionRecords

## Description
One cause.

Cause 1 of 1: every timestamp member of an ExecutionRecord returned by DescribeFlowExecutionRecords is serialized with sub-millisecond resolution, so the value's SHAPE diverges from what AWS puts on the wire (fixture appflow-s3-to-s3-flow-run-transfers-records, step task#8).

AWS: AppFlow (sdkId Appflow, apiVersion 2020-08-23) is a restJson1 service. Smithy: DescribeFlowExecutionRecords -> DescribeFlowExecutionRecordsResponse.flowExecutions (FlowExecutionList of ExecutionRecord); ExecutionRecord.startedAt / lastUpdatedAt / dataPullStartTime / dataPullEndTime all target com.amazonaws.appflow#Date, a plain `timestamp` carrying NO @timestampFormat trait, so the restJson1 default applies and each one is a JSON number of epoch seconds. The model declares no `executionStartTime` and no `lastUpdatedTime` member; the recording shows AppFlow reporting those two spellings beside the modeled ones, so they are recorded behavior only, and they carry the same instants and therefore the same wire format requirement. The fixture does not assert the instant itself (it is a per-run clock value and is recorded as '<shaped-not-asserted>'); it asserts the SHAPE of the serialized number, and that shape is what diverges. Hypothesis: AppFlow's recorded epoch-second numbers carry at most millisecond resolution (three fractional digits, trailing zeros trimmed), which is the resolution the service's own instants are kept at.

Emulator: the step returns HTTP 200 with a well-formed body, but all four timestamp members of flowExecutions[0] fail the shape assertion: executionStartTime and startedAt serialize as 1794607132.7745607 and lastUpdatedAt and lastUpdatedTime as 1794607133.2754664 — seven fractional digits, i.e. 100-nanosecond resolution taken straight from the host clock. crate::state::to_date_time is documented as deliberately preserving the stored SystemTime's subsecond nanoseconds, and ExecutionRun::to_execution_record feeds every one of these members (plus dataPullStartTime/dataPullEndTime, and Flow::to_description/last_run_execution_details) through it, so the whole family of AppFlow timestamps is emitted at a precision AWS never emits.

Why: a client that round-trips or compares AppFlow timestamps sees a value whose precision AWS does not produce. Anything that re-serializes a flow-run record, keys or dedupes runs by their reported startedAt/lastUpdatedAt string, or compares a DescribeFlowExecutionRecords instant against the same instant reported by another AppFlow call or by a recorded baseline, gets a value that will not match; a consumer that parses epoch seconds into a millisecond-resolution instant type silently rounds, so the emulator and the real service disagree on the same run. The divergence is in the wire format of the value, not in which instant it is, so it reproduces on every run regardless of the clock.

Fix: the shared timestamp converter in the AppFlow state layer that turns a stored SystemTime into the modeled Date (crate::state::to_date_time), so that every ExecutionRecord member built by ExecutionRun::to_execution_record — the two modeled instants startedAt/lastUpdatedAt and the two recorded aliases executionStartTime/lastUpdatedTime, plus dataPullStartTime/dataPullEndTime — is emitted at the resolution AppFlow reports, rather than at the host clock's native resolution. Because the same helper serves DescribeFlow and ListFlows, the fix belongs in that one converter and not in the DescribeFlowExecutionRecords handler.

No open issue covers this step: the replay reports it unowned, and the open AppFlow issues concern DescribeConnector's connector configuration, error-message wording, flow deactivation/deletion semantics and missing-resource lookups.

Traces: appflow-s3-to-s3-flow-run-transfers-records#task#8 (DescribeFlowExecutionRecords, expected 200, actual 200, four timestamp members shape-mismatched).

## Evidence
Fixture appflow-s3-to-s3-flow-run-transfers-records, step task#8, operation DescribeFlowExecutionRecords, verdict mismatch, expected_status 200, actual_status 200.

Expected (recorded AppFlow response, value not asserted, shape asserted):
  $.flowExecutions[0].executionStartTime: <shaped-not-asserted>
  $.flowExecutions[0].startedAt:          <shaped-not-asserted>
  $.flowExecutions[0].lastUpdatedAt:      <shaped-not-asserted>
  $.flowExecutions[0].lastUpdatedTime:    <shaped-not-asserted>

Actual (emulator):
  $.flowExecutions[0].executionStartTime: 1794607132.7745607
  $.flowExecutions[0].startedAt:          1794607132.7745607
  $.flowExecutions[0].lastUpdatedAt:      1794607133.2754664
  $.flowExecutions[0].lastUpdatedTime:     1794607133.2754664

All four are reported as value_mismatch against the shape assertion; the seconds themselves are a per-run clock value and are not the finding. Seven fractional digits are emitted where AppFlow's recorded epoch-second numbers carry at most three.

Model evidence (smithy_query on services/service-com-amazonaws-appflow-sandstoneconfigurationservicelambda/smithy-model/com-amazonaws-appflow-sandstoneconfigurationservicelambda.json):
  - service_info: protocols [restJson1], sdkId Appflow, version 2020-08-23
  - shape Date: {"type": "timestamp"} with no timestampFormat trait -> restJson1 default epoch-seconds
  - shape ExecutionRecord: members startedAt, lastUpdatedAt, dataPullStartTime, dataPullEndTime all target Date; executionStartTime and lastUpdatedTime are NOT modeled (recorded behavior only).

Code evidence: services/service-com-amazonaws-appflow-sandstoneconfigurationservicelambda/src/state.rs:847-854 (to_date_time, documented as keeping the instant's subsecond nanoseconds) and src/state.rs:297-315 (ExecutionRun::to_execution_record, which routes startedAt, lastUpdatedAt, dataPullStartTime, dataPullEndTime, executionStartTime and lastUpdatedTime through it).

## Steps to Reproduce
1. Boot the emulator from this checkout and replay fixture appflow-s3-to-s3-flow-run-transfers-records.
2. The fixture creates an AppFlow S3->S3 flow, calls StartFlow, and at step task#8 calls DescribeFlowExecutionRecords with the flow's name.
3. Read the 200 response body and inspect flowExecutions[0]: startedAt, lastUpdatedAt, executionStartTime and lastUpdatedTime are JSON numbers with seven fractional digits (e.g. 1794607132.7745607), i.e. sub-millisecond resolution.
4. Compare against the recorded AppFlow response for the same step, whose epoch-second numbers carry at most millisecond resolution; the replay reports all four members as shape mismatches while the status stays 200.
