Issue-Source: trace-sweep-divergence
Issue-Trace: quicksight-group-lifecycle-and-duplicate-rejected
Issue-Step: task#5
Issue-Severity: LOW
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: quicksight-group-lifecycle-and-duplicate-rejected quicksight-iam-policy-assignment-lifecycle
Issue-Shape-Id: com.amazonaws.quicksight#QuickSight_20180401
Issue-Key: quicksight|CreateGroup|4xx->4xx|$.RequestId
Issue-Key: quicksight|DescribeGroup|4xx->4xx|$.RequestId
Issue-Key: quicksight|DescribeIAMPolicyAssignment|4xx->4xx|$.RequestId
Issue-Commit: 74429669c22f4248895c195473d5b207144eaced

# QuickSight error bodies omit the modeled RequestId member AWS returns

> _Note: this issue was identified against the code base at commit `74429669c22f4248895c195473d5b207144eaced`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateGroup

## Description
The emulator's QuickSight error serializer writes the error payload WITHOUT the modeled `RequestId` member. AWS QuickSight's client-error shapes carry `RequestId` in the JSON body (the member is modeled on every QuickSight exception shape and documented as "The Amazon Web Services request ID for this request."), and these recordings assert the member is PRESENT (the corpus shapes it but does not assert its value, so the divergence is the member's absence, never its per-run value). On all three steps the HTTP status, the x-amzn-errortype header and the `Message` text MATCH the recording: the only divergence is the missing member. This issue bundles 3 causes - one shared serializer fault observed on three operations and two error classes - and ALL THREE must be closed and re-replayed.

Cause 1 of 3: CreateGroup on a group name that already exists - 409 ResourceExistsException.
AWS: Smithy (quicksight-20180401): CreateGroup (POST /accounts/{AwsAccountId}/namespaces/{Namespace}/groups) models ResourceExistsException (errorKind client, httpError 409); shape members are Message, ResourceType and RequestId, and RequestId's doc is "The Amazon Web Services request ID for this request.". Recorded behavior: the 409 body carries the duplicate-group message - its fixed text is the ResourceExistsException wording for a group that already exists, with the group name and namespace taken from the request - AND a top-level `RequestId` member holding that request's id (a per-run value, which is why the corpus shapes it rather than asserting it).
Emulator: answers 409 with the correct error type and message, but the JSON body has NO `RequestId` member at all (replay: "$.RequestId: missing (expected '<shaped-not-asserted>', got None)").
Why: a client that reads the request id out of a QuickSight error body for support correlation or logging gets nothing from the emulator and a value from AWS, and any SDK/contract test that diffs the error payload's member set against AWS fails on an error that is otherwise exactly right.
Fix: the quicksight error-response serializer - the shared error-payload builder that populates modeled exception members before writing the body. It must set the modeled `RequestId` member from the request-id allocator for every QuickSight exception shape that models it, not per operation.
Traces: quicksight-group-lifecycle-and-duplicate-rejected#task#5.

Cause 2 of 3: DescribeGroup on a group that is not present - 404 ResourceNotFoundException.
AWS: Smithy: DescribeGroup (GET /accounts/{AwsAccountId}/namespaces/{Namespace}/groups/{GroupName}) models ResourceNotFoundException (errorKind client, httpError 404); shape members Message, ResourceType, RequestId. Recorded behavior: the 404 body carries the group not-found message (fixed ResourceNotFoundException text plus the group name/namespace taken from the request) and a `RequestId` member.
Emulator: correct 404, error type and message; `RequestId` member absent (replay: "$.RequestId: missing (expected '<shaped-not-asserted>', got None)").
Why: same as Cause 1 - the error body is shape-divergent from AWS on the not-found path, the most commonly asserted QuickSight error class.
Fix: the same quicksight error-response serializer; this cause shows the omission is not specific to the 409 path.
Traces: quicksight-group-lifecycle-and-duplicate-rejected#task#7.

Cause 3 of 3: DescribeIAMPolicyAssignment on an assignment that is not present - 404 ResourceNotFoundException.
AWS: Smithy: DescribeIAMPolicyAssignment (GET /accounts/{AwsAccountId}/namespaces/{Namespace}/iam-policy-assignments/{AssignmentName}) models ResourceNotFoundException (errorKind client, httpError 404). Recorded behavior: the 404 body carries the assignment not-found message (fixed text plus the assignment name/namespace from the request) and a `RequestId` member.
Emulator: correct 404, error type and message; `RequestId` member absent (replay: "$.RequestId: missing (expected '<shaped-not-asserted>', got None)").
Why: same as above, on a third operation - confirming the fault is serializer-wide, so a per-handler patch would leave the remaining QuickSight error classes diverging.
Fix: the same quicksight error-response serializer.
Traces: quicksight-iam-policy-assignment-lifecycle#task#7.

Related open issues that do NOT cover these three clusters: 20261120T210750Z-quicksight-createdatasource-400-and-describefolder-404-bodies-emit-a.md and 20261112T051305Z-quicksight-error-payloads-carry-a-synthetic-requestid-member.md describe the OPPOSITE direction (a synthetic RequestId emitted where their recordings assert the member absent) and their replay keys are bound to the CreateDataSource 400, DescribeFolder 404, DescribeDataSource 404 and CreateFolder 409 clusters only. None of them carries the CreateGroup 409, DescribeGroup 404 or DescribeIAMPolicyAssignment 404 keys filed here. Taken together, the correct behavior is per-error-class as RECORDED - these three classes include the member - so a fix must make the serializer honor the recorded member set rather than flipping a global flag in either direction.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator booted from this checkout (emulator_up true, /__admin/health answered 200, data listener accepting). 49 steps replayed: 39 passed, 10 mismatched, 5 divergence clusters; 2 of those clusters are owned by an existing open issue and are not filed here.

Cause 1 - fixture `quicksight-group-lifecycle-and-duplicate-rejected`, step `task#5`, operation CreateGroup, verdict mismatch, expected_status 409 / actual_status 409:
  $.RequestId: missing (expected '<shaped-not-asserted>', got None)
No other key diverged (status, $headers.x-amzn-errortype and $.Message all matched).

Cause 2 - fixture `quicksight-group-lifecycle-and-duplicate-rejected`, step `task#7`, operation DescribeGroup, verdict mismatch, expected_status 404 / actual_status 404:
  $.RequestId: missing (expected '<shaped-not-asserted>', got None)
No other key diverged.

Cause 3 - fixture `quicksight-iam-policy-assignment-lifecycle`, step `task#7`, operation DescribeIAMPolicyAssignment, verdict mismatch, expected_status 404 / actual_status 404:
  $.RequestId: missing (expected '<shaped-not-asserted>', got None)
No other key diverged.

Expected (AWS, per recording + Smithy): error body = {"Message": "<that error class's template filled from the request and the emulator's state>", "RequestId": "<this request's id>"} (plus ResourceType where the service sets it).
Actual (emulator): the same status, x-amzn-errortype and Message, but no `RequestId` member in the body.

Model references used: ResourceExistsException - errorKind client, httpError 409, members {Message, ResourceType, RequestId}; ResourceNotFoundException - errorKind client, httpError 404, members {Message, ResourceType, RequestId}; RequestId documented "The Amazon Web Services request ID for this request."; CreateGroup errors include ResourceExistsException, DescribeGroup and DescribeIAMPolicyAssignment errors include ResourceNotFoundException. The divergence is the member's PRESENCE only - the corpus shapes the id without asserting its value, so it reproduces on every run regardless of the generated id.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1.
2. Cause 1: replay fixture `quicksight-group-lifecycle-and-duplicate-rejected` step `task#5` - POST /accounts/<account>/namespaces/<namespace>/groups re-creating a group that already exists. The emulator answers 409 ResourceExistsException with the right message; diff the JSON body member set against the recording and observe the missing top-level `RequestId`.
3. Cause 2: replay the same fixture step `task#7` - GET /accounts/<account>/namespaces/<namespace>/groups/<a group that is not present>. 404 ResourceNotFoundException, correct message, no `RequestId` member.
4. Cause 3: replay fixture `quicksight-iam-policy-assignment-lifecycle` step `task#7` - GET /accounts/<account>/namespaces/<namespace>/iam-policy-assignments/<an assignment that is not present>. 404 ResourceNotFoundException, correct message, no `RequestId` member.
5. Compare only the body MEMBER SET; the id's value is not asserted, so the divergence reproduces on every run.
6. Re-replay ALL THREE steps after the serializer fix - closing one does not exercise the other two.
