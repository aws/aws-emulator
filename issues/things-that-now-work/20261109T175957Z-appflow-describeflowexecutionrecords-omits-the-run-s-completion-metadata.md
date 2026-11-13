Issue-Source: trace-sweep-divergence
Issue-Trace: appflow-s3-to-s3-flow-run-transfers-records
Issue-Step: task#8
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: appflow-s3-to-s3-flow-run-transfers-records
Issue-Shape-Id: com.amazonaws.appflow#SandstoneConfigurationServiceLambda
Issue-Key: appflow|DescribeFlowExecutionRecords|2xx->2xx|$.flowExecutions[*].dataPullEndTime
Issue-Key: appflow|DescribeFlowExecutionRecords|2xx->2xx|$.flowExecutions[*].dataPullStartTime
Issue-Key: appflow|DescribeFlowExecutionRecords|2xx->2xx|$.flowExecutions[*].executionMessage
Issue-Key: appflow|DescribeFlowExecutionRecords|2xx->2xx|$.flowExecutions[*].executionMetadata
Issue-Key: appflow|DescribeFlowExecutionRecords|2xx->2xx|$.flowExecutions[*].executionResult.maxPageSize
Issue-Key: appflow|DescribeFlowExecutionRecords|2xx->2xx|$.flowExecutions[*].executionStartTime
Issue-Key: appflow|DescribeFlowExecutionRecords|2xx->2xx|$.flowExecutions[*].lastUpdatedAt
Issue-Key: appflow|DescribeFlowExecutionRecords|2xx->2xx|$.flowExecutions[*].lastUpdatedTime
Issue-Key: appflow|DescribeFlowExecutionRecords|2xx->2xx|$.flowExecutions[*].startedAt
Issue-Commit: db165e9afd311b39a9697deff319fb6ff63a6914

# AppFlow DescribeFlowExecutionRecords omits the run's completion metadata and invents data-pull timestamps

> _Note: this issue was identified against the code base at commit `db165e9afd311b39a9697deff319fb6ff63a6914`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeFlowExecutionRecords

## Description
Cause 1 of 1: after a flow run finishes, the ExecutionRecord the emulator projects for DescribeFlowExecutionRecords is missing every field that reports what the run actually did, and populates two timestamps AWS leaves unset for this flow shape.

AWS: Smithy (com.amazonaws.appflow#DescribeFlowExecutionRecords, POST /describe-flow-execution-records, output DescribeFlowExecutionRecordsResponse.flowExecutions -> FlowExecutionList -> ExecutionRecord) models each record as executionId, executionStatus (enum InProgress|Successful|Error|CancelStarted|Canceled), executionResult (ExecutionResult{errorInfo, bytesProcessed, bytesWritten, recordsProcessed, numParallelProcesses, maxPageSize — all Long}), startedAt, lastUpdatedAt, dataPullStartTime, dataPullEndTime (all com.amazonaws.appflow#Date timestamps) and metadataCatalogDetails. So ExecutionResult.maxPageSize is a modeled Long and belongs in the serialized executionResult of a completed run; the recording carries it as 0.

Beyond the public model the recorded AWS response for a completed run carries members the model does not declare (recorded behavior only, taken from the recorded 200 response): a record-level executionMessage whose fixed text for a successful run is "Successfully ran the flow" (no variable parts); an executionMetadata block with bytesProcessed, bytesWritten, maxPageSize, numFailures, numFilteredRecords, numParallelProcesses, numPutFailures, reason, recordsProcessed and terminateFlow, whose values are the run's own accounting (the recorded run reports bytesWritten 112, recordsProcessed 3, numFailures/numPutFailures/numFilteredRecords 0, numParallelProcesses 1, maxPageSize 0, reason null, terminateFlow false — those are that run's values, not the requirement; the requirement is that the block is present and filled from the run's real counters); and executionStartTime / lastUpdatedTime as shaped timestamp siblings of startedAt / lastUpdatedAt. The recording also shows dataPullStartTime and dataPullEndTime ABSENT for this S3-source → S3-destination run: AWS only reports a data-pull window when the connector performed a data pull (recorded behavior only).

Emulator: for the same completed run the replayed record has executionMessage = absent, executionMetadata = absent, executionResult.maxPageSize = absent, executionStartTime = absent and lastUpdatedTime = absent, while dataPullStartTime and dataPullEndTime ARE populated (epoch-seconds values, e.g. 1794247039.771/1794247039.772) where AWS reports neither. startedAt and lastUpdatedAt are additionally emitted as bare epoch-seconds floats that do not match the shape the recording asserts for those members. Status is 200 in both cases — only the record body diverges.

Why: a client that polls DescribeFlowExecutionRecords to learn the outcome of a run gets a record with no human-readable outcome (executionMessage) and no transfer accounting (executionMetadata / executionResult.maxPageSize), so success-reporting, record-count reconciliation and failure triage all read as empty on a run that in fact moved records. Conversely, code that treats a present dataPullStartTime/dataPullEndTime as proof that the connector pulled data from a source concludes a data pull happened on a flow where AWS reports none, and code reading executionStartTime/lastUpdatedTime (the names the console and several SDK samples use) finds nothing at all.

Fix: the appflow service's DescribeFlowExecutionRecords handler and the execution-record projection/serializer it reads from — the flow-run state must retain the finished run's accounting (message, metadata counters, maxPageSize, start/last-updated instants) and the projection must emit executionMessage, executionMetadata, executionResult.maxPageSize and the executionStartTime/lastUpdatedTime aliases, serialize startedAt/lastUpdatedAt in AppFlow's timestamp representation, and leave dataPullStartTime/dataPullEndTime unset unless the run actually performed a data pull. Related but non-covering open issues: 20261105T081532Z-appflow-read-paths-still-misreport-the-flow-principal.md and 20261108T013452Z-appflow-describeconnector-returns-an-unfaithful-connector-configuration.md are about other appflow read paths and do not carry this step's replay key.

Traces: appflow-s3-to-s3-flow-run-transfers-records#task#8 (DescribeFlowExecutionRecords after the S3→S3 flow run completes).

## Evidence
Fixture appflow-s3-to-s3-flow-run-transfers-records, step task#8, DescribeFlowExecutionRecords, expected 200 / actual 200, body diverges:
  $.flowExecutions[0].executionMessage: missing (expected 'Successfully ran the flow', got None)
  $.flowExecutions[0].executionMetadata: missing (expected {bytesProcessed, bytesWritten: 112, maxPageSize: 0, numFailures: 0, numFilteredRecords: 0, numParallelProcesses: 1, numPutFailures: 0, reason: None, recordsProcessed: 3, terminateFlow: False}, got None)
  $.flowExecutions[0].executionResult.maxPageSize: missing (expected 0, got None)
  $.flowExecutions[0].executionStartTime: missing (expected shaped timestamp, got None)
  $.flowExecutions[0].lastUpdatedTime: missing (expected shaped timestamp, got None)
  $.flowExecutions[0].dataPullStartTime: type_mismatch (expected None, got 1794247039.7711403)
  $.flowExecutions[0].dataPullEndTime: type_mismatch (expected None, got 1794247039.7719414)
  $.flowExecutions[0].startedAt: value_mismatch (expected shaped timestamp, got 1794247039.7711403)
  $.flowExecutions[0].lastUpdatedAt: value_mismatch (expected shaped timestamp, got 1794247040.272028)
The numeric counters and instants above are that run's values; the requirement is the presence and sourcing of the members, not those literals. Smithy citations: ExecutionRecord{executionId, executionStatus, executionResult, startedAt, lastUpdatedAt, dataPullStartTime, dataPullEndTime, metadataCatalogDetails}; ExecutionResult.maxPageSize -> Long; errors InternalServerException(500)/ResourceNotFoundException(404)/ValidationException(400).

## Steps to Reproduce
1. Create an AppFlow flow with an S3 source and an S3 destination (fixture appflow-s3-to-s3-flow-run-transfers-records) and StartFlow it so the run completes.
2. Call DescribeFlowExecutionRecords (POST /describe-flow-execution-records) with that flowName.
3. Inspect flowExecutions[0]: executionMessage, executionMetadata, executionResult.maxPageSize, executionStartTime and lastUpdatedTime are absent, while dataPullStartTime/dataPullEndTime are populated. AWS returns the first five and omits the last two for an S3→S3 run.
4. Replay step task#8 of fixture appflow-s3-to-s3-flow-run-transfers-records to confirm.
