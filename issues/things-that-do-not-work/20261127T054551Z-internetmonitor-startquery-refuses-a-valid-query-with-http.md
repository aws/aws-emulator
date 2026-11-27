Issue-Source: trace-sweep-divergence
Issue-Trace: internetmonitor-query-workflow
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: internetmonitor-query-workflow
Issue-Shape-Id: com.amazonaws.internetmonitor#InternetMonitor20210603
Issue-Key: internetmonitor|StartQuery|2xx->4xx|status
Issue-Commit: 7bfef4555d8ff23b4aee8bc4649f9354551130b5

# internetmonitor: StartQuery refuses a valid query with HTTP 400 instead of minting a QueryId, blocking the whole query interface

> _Note: this issue was identified against the code base at commit `7bfef4555d8ff23b4aee8bc4649f9354551130b5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartQuery

## Description
Cause 1 of 1: StartQuery answers an input-validation failure (HTTP 400) for a request real AWS accepts, so no QueryId is ever minted and every downstream query-interface call in the workflow has nothing to address.

AWS: Smithy model services/service-com-amazonaws-internetmonitor-internetmonitor20210603/smithy-model/com-amazonaws-internetmonitor-internetmonitor20210603.json, operation StartQuery — POST /v20210603/Monitors/{MonitorName}/Queries, success code 200, output StartQueryOutput with the required member QueryId ("The internally-generated identifier of a specific query"). Required input: MonitorName (@httpLabel, ResourceName, @length 1..255, @pattern ^[a-zA-Z0-9_.-]+$), StartTime and EndTime (Timestamp, @timestampFormat date-time), QueryType (enum MEASUREMENTS | OVERALL_TRAFFIC_SUGGESTIONS | OVERALL_TRAFFIC_SUGGESTIONS_DETAILS | ROUTING_SUGGESTIONS | TOP_LOCATIONS | TOP_LOCATION_DETAILS); optional FilterParameters and LinkedAccountId (@length 12..12). The operation models exactly five errors: AccessDeniedException (403), InternalServerException (500), LimitExceededException (403), ThrottlingException (429), ValidationException (400) — it does NOT model BadRequestException. The model documents no precondition on the monitor beyond naming it: a query is started for a monitor, and a query that selects no data is reported through the query's own status/results, not refused at start. The recorded step shows AWS answering 200 for this request.

Emulator: the step answers HTTP 400 instead of 200 (replay detail "status: expected 200, got 400"), and the same refusal recurs on the workflow's second StartQuery, so the refusal is systematic for this request shape rather than a one-off. The handler (services/service-com-amazonaws-internetmonitor-internetmonitor20210603/src/ops/start_query.rs) gates three fabricated 400 refusals ahead of minting the id, each of which returns before store.record_query: (a) monitor.resources.is_empty() -> BadRequestException, message "Invalid request body" (crate::state::INVALID_REQUEST_BODY_MESSAGE); (b) EndTime < StartTime -> BadRequestException, same message; (c) a MonitorName not held -> ValidationException with message "resourceID is invalid: monitor/<MonitorName>" (crate::state::invalid_resource_id_message over monitor_resource_path). Hypothesis: branch (a) is the one firing — the monitor the workflow queries carries no Resources, and (a) is the only gate that depends on the monitor's content rather than on the request, so it refuses every query type for such a monitor.

On the message: for the BadRequestException branches the entire string "Invalid request body" is fixed text and carries no request or state value; for the ValidationException branch the fixed text is "resourceID is invalid: monitor/" and the trailing monitor name is the MonitorName taken verbatim from the request path. BadRequestException is additionally outside StartQuery's modeled error set — it is widened in through #[operation_error(..., extra_errors = ["BadRequestException"])] at start_query.rs — so a client branching on StartQuery's declared errors sees an error code the operation cannot return on AWS and falls through to its unmodeled-error path.

Why: a client cannot start a query at all. StartQuery is the entry point of the whole query interface, so the 400 denies the caller the required QueryId and GetQueryStatus, GetQueryResults and StopQuery all become unreachable (the replay reports 4 further steps blocked behind this one). A caller that handles AWS's modeled errors sees neither a retryable condition nor a fixable request: the request is valid, and no change to it clears a precondition the service does not actually impose. Code that distinguishes "query started, no data" from "query refused" takes the wrong branch, and an SDK sees an error code StartQuery does not declare.

Fix: StartQuery's handler and its validation — src/ops/start_query.rs. Remove the preconditions AWS does not impose so the operation mints a QueryId and records the QUEUED query for any monitor the account holds and any modeled QueryType, and let an empty result surface through the query's status and results instead. Keep refusals only for conditions AWS states, and answer them with an error the operation models (ValidationException, 400) rather than the widened-in BadRequestException.

Traces: internetmonitor-query-workflow#task#2 (representative) and internetmonitor-query-workflow#task#5 — both StartQuery, both expected 200, both got 400.

## Evidence
Fixture internetmonitor-query-workflow, step task#2 (cluster also covers task#5), operation StartQuery, verdict mismatch.
Expected: HTTP 200 with StartQueryOutput carrying the required QueryId member, as the recording shows AWS answering.
Actual:   HTTP 400. Replay detail: "status: expected 200, got 400"; divergence_key service=internetmonitor|StartQuery|status=200->400|keys=status; affected_count 2.
The emulator's 400 bodies for this operation are, from src/ops/start_query.rs: error code BadRequestException with message "Invalid request body" (resources-empty gate and EndTime<StartTime gate), or error code ValidationException with message "resourceID is invalid: monitor/<MonitorName>" (unknown monitor gate). BadRequestException is not in StartQuery's modeled error set (AccessDeniedException 403, InternalServerException 500, LimitExceededException 403, ThrottlingException 429, ValidationException 400); it is widened in via extra_errors at start_query.rs.
Consequence recorded by the same replay: 4 steps blocked, because no QueryId was produced for the GetQueryStatus / GetQueryResults / StopQuery steps that follow.
No open issue covers this: the only internetmonitor issue on file is "CloudFormation rejects AWS::InternetMonitor::Monitor as an unrecognized resource type" (20261109T113647Z-cloudformation-internetmonitor-...), which is a CloudFormation resource-type gap, and the three open StartQuery issues are all for the logs and athena services.

## Steps to Reproduce
1. Clone the repo and boot the emulator built from it.
2. Replay the wire corpus: replay_wire_corpus(). The summary reports 1 divergence and 4 blocked steps.
3. Fixture internetmonitor-query-workflow, step task#2 (and task#5): POST /v20210603/Monitors/{MonitorName}/Queries against a monitor the workflow created earlier in the same fixture, with a valid StartTime/EndTime window (StartTime <= EndTime, RFC 3339 date-time strings) and a modeled QueryType.
4. Observe HTTP 400 with an input-validation error body instead of HTTP 200 with a QueryId.
5. Equivalent by hand: CreateMonitor with a MonitorName and no Resources list, then StartQuery on that monitor with QueryType=MEASUREMENTS and any StartTime <= EndTime — the emulator answers 400 BadRequestException "Invalid request body"; AWS answers 200 with a QueryId.
6. The follow-on GetQueryStatus / GetQueryResults / StopQuery steps then have no QueryId to address and are reported blocked.
