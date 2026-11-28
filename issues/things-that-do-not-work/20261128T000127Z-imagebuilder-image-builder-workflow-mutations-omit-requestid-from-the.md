Issue-Source: trace-sweep-divergence
Issue-Trace: imagebuilder-workflow-build-versions-accumulate-per-semantic-version
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: imagebuilder-workflow-build-versions-accumulate-per-semantic-version imagebuilder-workflow-build-versions-list-each-build-and-drop-a-deleted-one imagebuilder-workflow-build-versions-list-the-created-build-and-drop-it-on-delete
Issue-Shape-Id: com.amazonaws.imagebuilder#imagebuilder
Issue-Key: imagebuilder|CreateWorkflow|2xx->2xx|$.requestId
Issue-Key: imagebuilder|DeleteWorkflow|2xx->2xx|$.requestId
Issue-Commit: 7e7f1782eb72d186b4a02ad6793c94a317471370

# Image Builder workflow mutations omit requestId from the response body

> _Note: this issue was identified against the code base at commit `7e7f1782eb72d186b4a02ad6793c94a317471370`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateWorkflow

## Description
Two causes, one shared fault: the imagebuilder workflow mutation serializers drop the top-level `requestId` member that Image Builder returns on every successful workflow response, so a client that reads it from a 200 body gets nothing.

Cause 1 of 2: CreateWorkflow (PUT /CreateWorkflow) 200 response omits `requestId`.
  AWS: recorded behavior only — the on-disk Smithy model (CreateWorkflowResponse: clientToken, workflowBuildVersionArn, latestVersionReferences) does not model `requestId`, but the recording shows the real service returning a top-level `requestId` string on the 200 body alongside `workflowBuildVersionArn`. The recorded value itself is not asserted (`<shaped-not-asserted>`): the requirement is that the member is PRESENT and is a non-empty request-id string generated per call, not any particular literal (the recorded id is a per-run value and is not part of the requirement).
  Emulator: returns 200 with the expected `workflowBuildVersionArn` but no `requestId` key at all ($.requestId: missing, expected '<shaped-not-asserted>', got None).
  Why: a client or SDK that logs/correlates Image Builder calls by the body's `requestId`, or a test that asserts the field exists, sees a null/absent field on every successful CreateWorkflow and loses the correlation handle AWS always supplies.
  Fix: the CreateWorkflow output serializer — populate the response's `requestId` with the per-request id the handler already has.
  Traces: imagebuilder-workflow-build-versions-accumulate-per-semantic-version#task#1 (also task#2).

Cause 2 of 2: DeleteWorkflow (DELETE /DeleteWorkflow) 200 response omits `requestId`.
  AWS: recorded behavior only — DeleteWorkflowResponse in the model carries only `workflowBuildVersionArn`, yet the recording shows the service returning a top-level `requestId` as well. Again presence/shape is the requirement, not the recorded literal.
  Emulator: 200 with `workflowBuildVersionArn` only ($.requestId: missing, expected '<shaped-not-asserted>', got None).
  Why: same correlation loss on the delete path; a client that asserts response shape parity against AWS sees a differently shaped success body.
  Fix: the DeleteWorkflow output serializer — same missing field as Cause 1; both likely share the response-builder for workflow ops.
  Traces: imagebuilder-workflow-build-versions-accumulate-per-semantic-version#task#5 (also #task#8, imagebuilder-workflow-build-versions-list-each-build-and-drop-a-deleted-one#task#7, imagebuilder-workflow-build-versions-list-the-created-build-and-drop-it-on-delete#task#4).

No open issue covers this: the existing imagebuilder workflow issue 20261127T125601Z-imagebuilder-image-builder-create-time-validation-gaps-createworkflow-accepts is about accepting invalid step documents, not about the response body's requestId member.

## Evidence
Replayed against the emulator at the bound data listener:

Cause 1 — CreateWorkflow, fixture imagebuilder-workflow-build-versions-accumulate-per-semantic-version step task#1 (cluster also covers task#2):
  expected status 200 / actual status 200
  $.requestId: missing (expected '<shaped-not-asserted>', got None)

Cause 2 — DeleteWorkflow, same fixture step task#5 (cluster also covers task#8 and two sibling fixtures, 4 steps total):
  expected status 200 / actual status 200
  $.requestId: missing (expected '<shaped-not-asserted>', got None)

Model evidence: smithy_query operation=CreateWorkflow/DeleteWorkflow shows no requestId member on either output shape, while ListComponentsResponse and ListWorkflowStepExecutionsResponse DO model `requestId` (NonEmptyString, length 1..1024) — i.e. the field is the service's standard response member and the recordings confirm it on the workflow mutations too.

## Steps to Reproduce
1. PUT /CreateWorkflow with a valid name, semanticVersion 1.0.0, type test, a clientToken and inline data. Observe the 200 body: it contains workflowBuildVersionArn but no requestId.
2. DELETE /DeleteWorkflow?workflowBuildVersionArn=<the arn returned above>. Observe the 200 body: workflowBuildVersionArn only, no requestId.
3. Replay fixture imagebuilder-workflow-build-versions-accumulate-per-semantic-version (steps task#1, task#2, task#5, task#8) to see both causes.
