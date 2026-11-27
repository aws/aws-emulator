Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iot.fleet-metric-invalid-aggregation-rejected iot.fleet-metric-query-string-preserved iot.telemetry-fleet-metric-invalid-aggregation-rejected
Issue-Shape-Id: com.amazonaws.iot#AWSIotService
Issue-Key: iot.fleet-metric-invalid-aggregation-rejected|run#11|AssertionError|assert not_found_message == expected_not_found, (
Issue-Key: iot.fleet-metric-query-string-preserved|run#16|AssertionError|assert message == (
Issue-Key: iot.telemetry-fleet-metric-invalid-aggregation-rejected|run#15|AssertionError|assert not_found_code_message == f"An error occurred (ResourceNotFoundException) when calling the DescribeFleetMetric operation: FleetMetric {ctx.account}:{bad_
Issue-Commit: 8faad1de300cdf3d9bfb0403786f5996fdab75da

# REOPENS: iot DescribeFleetMetric's ResourceNotFoundException Message is still only the tail — the leading "An error occurred (ResourceNotFoundException) when calling the DescribeFleetMetric operation: " segment is absent

> _Note: this issue was identified against the code base at commit `8faad1de300cdf3d9bfb0403786f5996fdab75da`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeFleetMetric

## Description
REOPENS 20261114T082236Z-iot-refusal-message-fidelity-7-causes-addthingtothinggroup-createstream.md: its fix left the call refused. That issue is marked fixed and its Cause 7 carried exactly the three replay keys listed below, but all three checks still fail on the same comparison against the same operation's refusal, so the cause it claimed to close is still open. This is not described as a new defect.

ONE cause must be closed before submitting.

Cause 1 of 1: DescribeFleetMetric for a fleet metric the account does not hold is refused with the right HTTP status and the right error code, but the error body's Message member carries only the TAIL of the text AWS returns for this error class — the whole leading segment is missing.

AWS: Smithy (services/service-com-amazonaws-iot-awsiotservice/smithy-model/com-amazonaws-iot-awsiotservice.json): DescribeFleetMetric = GET /fleet-metric/{metricName}, success code 200; metricName is @required @httpLabel targeting FleetMetricName (@length 1..128, pattern ^[a-zA-Z0-9_\-\.]+$); modeled errors include ResourceNotFoundException with @httpError 404. The model says nothing about the message text, so for the TEXT this is recorded behavior only: the refusal is HTTP 404, error code ResourceNotFoundException, and Message == "An error occurred (ResourceNotFoundException) when calling the DescribeFleetMetric operation: FleetMetric " + <account-id> + ":" + <metricName> + " not found.". Fixed text of that template: "An error occurred (ResourceNotFoundException) when calling the DescribeFleetMetric operation: FleetMetric ", the ":" separator, and the trailing " not found.". Variable parts, taken from the request and the emulator's state: the caller's account id and the metricName from the {metricName} URI label. Three independently-authored behavioral checks assert this same template, and the open WIRE issue 20261124T230853Z-iot-describefleetmetric-not-found-message-omits-the-leading.md settles the question the fixed issue's Cause 7 left open: it observes the same divergence directly on the recorded HTTP response body's `message` member with no SDK in the path (4 steps across 2 fixtures), so the leading segment belongs to the serialized Message member and is not merely botocore's str(ClientError) rendering. The 404 status and the ResourceNotFoundException code already match and are NOT part of this divergence.

Emulator: answers 404 / ResourceNotFoundException with Message == "FleetMetric <account-id>:<metricName> not found." — exactly the tail of the recorded text, with the entire leading "An error occurred (ResourceNotFoundException) when calling the DescribeFleetMetric operation: " segment absent. The account id and the metric name agree character-for-character on both sides in all three checks, so the divergence is the handler's message template and not a per-run value.

Why: a client that branches on, logs, or asserts against this refusal's message never matches the text AWS returns, even though the status and the code are right. A caller that string-matches the message to tell a missing fleet metric apart from another 404 on this path, and any parity harness keyed to AWS's sentence for this error class, sees different text than production. Because the previous fix was closed without the text changing, callers that were expected to work after it still do not.

Fix: the DescribeFleetMetric handler's not-found refusal path in the iot crate and the message it formats there (the registry-style "<Type> <account>:<name> not found." formatter it currently shares must keep its text for the other iot operations that legitimately use it — e.g. DescribeThingGroup — so this must be a template fix local to this one refusal path). Status code, error code and body shape are already correct and must not change.

Traces: iot.fleet-metric-invalid-aggregation-rejected, iot.fleet-metric-query-string-preserved, iot.telemetry-fleet-metric-invalid-aggregation-rejected.

## Evidence
Behavioral suite simparity-behavioral-aws-v1 replayed against the emulator built from this checkout; bootstrap_signal=false, total 284/351 passed (80.9%). Heading '### iot': 223/257 (87%), 34 failed — these three are the only iot failures neither OWNED by an open issue nor blocked by an unimplemented scaffold stub.

In all three the error CODE was ResourceNotFoundException as expected and the preceding legs passed; only the Message member diverges.

[mismatch] iot.fleet-metric-invalid-aggregation-rejected
  File "check_iot_fleet_metric_invalid_aggregation_rejected.py", line 49, in run
  assert not_found_message == expected_not_found
  AssertionError: expected message 'An error occurred (ResourceNotFoundException) when calling the DescribeFleetMetric operation: FleetMetric <account>:sim2real-fleetmetric-bad-<id> not found.', got 'FleetMetric <account>:sim2real-fleetmetric-bad-<id> not found.'
  Passed earlier in the same check: CreateFleetMetric with aggregationType {'name':'Statistics','values':['average']} over the String field thingName was refused InvalidRequestException with the exact message "Invalid values [average] for Statistics aggregation on String field. Supported values are [count]".

[mismatch] iot.fleet-metric-query-string-preserved
  File "check_iot_fleet_metric_query_string_preserved.py", line 60, in run
  assert message == ("An error occurred (ResourceNotFoundException) when calling the DescribeFleetMetric operation: FleetMetric {account}:{metric} not found.")
  AssertionError: FleetMetric <account>:sim2realfm<id> not found.
  Passed earlier in the same check: the complex queryString ('thingTypeName:"sim2real-sensor" AND (attributes.firmware:"2.1.0" OR attributes.firmware:"2.2.0") AND NOT thingName:"excluded-*"') round-tripped character-for-character through DescribeFleetMetric, along with aggregationField and aggregationType. The check dies on the post-delete DescribeFleetMetric message, before its UpdateFleetMetric leg (which expects "FleetMetric <metricName> doesn't exist") is ever reached.

[mismatch] iot.telemetry-fleet-metric-invalid-aggregation-rejected
  File "check_iot_telemetry_fleet_metric_invalid_aggregation_rejected.py", line 63, in run
  AssertionError: expected the exact ResourceNotFoundException message, got 'FleetMetric <account>:sim2real-badmetric<id> not found.'
  Passed earlier in the same check: UpdateIndexingConfiguration with a String customField, DescribeIndex reaching ACTIVE, and CreateFleetMetric's InvalidRequestException with the exact "Invalid values [average] for Statistics aggregation on String field. Supported values are [count]".

The account ids and metric names above are per-run values filling the message template and are identical on both sides of every comparison; the requirement is the fixed text plus the caller's account id and the request's metricName.

Corroborating wire evidence (open issue 20261124T230853Z-iot-describefleetmetric-not-found-message-omits-the-leading.md): cluster iot|DescribeFleetMetric|404->404|$.message, verbatim detail — "$.message: value_mismatch (expected 'An error occurred (ResourceNotFoundException) when calling the DescribeFleetMetric operation: FleetMetric <account>:<metricName> not found.', got 'FleetMetric <account>:<metricName> not found.')" across iot-fleet-indexing-error-paths#task#1, #task#4 and iot-fleet-metric-lifecycle#task#4, #task#7.

Previously filed and still unresolved for the same text: 20261114T082236Z-...-refusal-message-fidelity (status fixed, Cause 7, same three replay keys) and 20261114T165150Z-iot-4-net-new-causes-createdomainconfiguration-accepts-a.md Cause 4 (four other checks' keys).

## Steps to Reproduce
1. git_clone(); run_behavioral_suite().
2. Boot the emulator built from this checkout and point a botocore iot client at it.
3. UpdateIndexingConfiguration(thingIndexingConfiguration={'thingIndexingMode':'REGISTRY'}) and poll DescribeIndex(indexName='AWS_Things') until indexStatus == 'ACTIVE'.
4. Either (a) CreateFleetMetric(metricName=M, queryString='thingName:*', aggregationType={'name':'Statistics','values':['average']}, period=300, aggregationField='registry.version', indexName='AWS_Things') then DeleteFleetMetric(metricName=M), or (b) pick a metricName M that was never created (e.g. one whose CreateFleetMetric was refused InvalidRequestException).
5. Call DescribeFleetMetric(metricName=M) and read the raw HTTP response: status and x-amzn-errortype, then the body's `message` member (botocore: e.response['Error']['Code'] and e.response['Error']['Message'] — not str(e), which adds its own prefix).
   Expect: 404, ResourceNotFoundException, message == "An error occurred (ResourceNotFoundException) when calling the DescribeFleetMetric operation: FleetMetric <account>:<M> not found."
   Observe today: 404, ResourceNotFoundException, message == "FleetMetric <account>:<M> not found." — the leading segment is missing.
6. Confirm the fix by re-replaying the three checks in trace_ids; the status, the error code and the body shape must be unchanged, and DescribeThingGroup's "ThingGroup <account>:<name> not found." message must not change.
