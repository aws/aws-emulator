Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway.import-documentation-parts-bulk-atomic apigateway.canary-deployment-traffic-shift-and-promotion
Issue-Shape-Id: com.amazonaws.apigateway#BackplaneControlService
Issue-Key: apigateway.import-documentation-parts-bulk-atomic|run#21|AssertionError|assert blocked_message == (
Issue-Key: apigateway.canary-deployment-traffic-shift-and-promotion|_poll_http#20|Timeout|time.sleep(interval_s)
Issue-Commit: 11bf1cc5011aa260df7304f2661cde6d9105badf

# apigateway: 2 net-new causes — ImportDocumentationParts' failOnWarnings rejection lists only one of the three import warnings, and a 100% canary deployment never shifts traffic or promotes

> _Note: this issue was identified against the code base at commit `11bf1cc5011aa260df7304f2661cde6d9105badf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ImportDocumentationParts

## Description
Two net-new root causes in the apigateway service. Both must be closed before this issue is submitted.

=== Cause 1 of 2: ImportDocumentationParts(failOnWarnings=true) rejects with an incomplete warning list ===
Failing check: apigateway.import-documentation-parts-bulk-atomic

AWS: Smithy: apigateway ImportDocumentationParts — PUT /restapis/{restApiId}/documentation/parts, input {restApiId (httpLabel, required), mode (httpQuery "mode", PutMode), failOnWarnings (httpQuery "failonwarnings", boolean, default false), body (httpPayload, required)}, output DocumentationPartIds{ids, warnings}, errors BadRequestException(400), ConflictException(409), LimitExceededException(429), NotFoundException(404), TooManyRequestsException(429), UnauthorizedException(401). With failOnWarnings=true AWS refuses the import with BadRequestException (HTTP 400) and the Message is the fixed header line "Warnings found during import:" followed by one tab-indented line PER WARNING the import produced, newline-separated. For a document whose only defect is a response schema "$ref" to a non-existent component schema plus one explicit MODEL documentation part, AWS emits THREE warning lines: the unresolvable model reference is reported once per place it is resolved (twice), and the method's media type is then reported as unusable:
  Warnings found during import:\n\tReference to model '<ModelName>' not found. Ignoring.\n\tReference to model '<ModelName>' not found. Ignoring.\n\tInvalid format for model <mediaType> for method <HTTP-VERB>
Fixed text: "Warnings found during import:", "Reference to model '", "' not found. Ignoring.", "Invalid format for model ", " for method ". Variable parts taken from the request document: the model/component name ('DoesNotExist' in the trace), the media type (application/json) and the HTTP verb (GET). Nothing new must be committed: GetDocumentationParts must hold exactly what it held before the refused call. With failOnWarnings=false the same import returns 200 with those warnings in `warnings` and commits the MODEL part.

Emulator: returns BadRequestException (correct code) but the Message carries only ONE of the three lines: "Warnings found during import:\n\tReference to model 'DoesNotExist' not found. Ignoring." — the second resolution of the same reference and the "Invalid format for model application/json for method GET" line are both missing. So the emulator's import pipeline resolves the $ref in one place only and never validates the method's media-type model binding.

Why: a client that reads the refusal to find out what to fix is told about one defect and not the media-type binding, so an operator fixing only what the message names re-submits and is refused again. A test or CI gate that asserts the documented warning set (a very common shape for "import must be clean") sees a different message and cannot distinguish "fewer warnings" from "a different document".

Fix: the ImportDocumentationParts handler's warning collection and its BadRequestException message serializer — the warning list must be produced per resolution site (emitting the model-reference warning for every place the unresolvable $ref is dereferenced) and must include the per-method media-type model validation; the message is then the fixed header plus one tab-indented line per collected warning. The non-blocking path (failOnWarnings=false) must surface the identical list in `warnings`.

=== Cause 2 of 2: a 100% canary deployment never serves canary traffic, and promotion never converges ===
Failing check: apigateway.canary-deployment-traffic-shift-and-promotion

AWS: Smithy: CreateDeployment (POST /restapis/{restApiId}/deployments, 201) accepts stageName + canarySettings (DeploymentCanarySettings{percentTraffic, stageVariableOverrides, useStageCache}); the Stage read back carries canarySettings (CanarySettings{deploymentId, percentTraffic, stageVariableOverrides, useStageCache}) and UpdateStage (PATCH /restapis/{restApiId}/stages/{stageName}) applies patchOperations against /canarySettings/percentTraffic, /canarySettings/deploymentId and /deploymentId. Recorded behavior for the data plane: attaching a canary must NOT move the stage's own deploymentId; with percentTraffic=100.0 every request is served by the canary deployment's snapshot with the canary's stageVariableOverrides applied; with percentTraffic=0.0 every request falls back to the stage's own deployment with the stage's own variables (the canary override must not leak into base traffic); a copy patch from /canarySettings/deploymentId to /deploymentId followed by a remove of /canarySettings promotes the canary snapshot and leaves the stage's own variables in force.

Emulator: the check never completes — replay reports `Check timed out after 60.0s`. Each of the check's four data-plane waits allows 120 s, so the cap is reached inside one of them: the stage never serves the awaited (snapshot, release) pair. Hypothesis: the stage data plane serves only the stage's own deployment snapshot and ignores canarySettings entirely (neither the canary snapshot at 100% nor the stageVariableOverrides are ever applied), so the very first canary wait polls to its deadline.

Why: canary deployments are the supported way to shift REST API traffic. A client that attaches a 100% canary and verifies the new snapshot before promoting gets served the OLD snapshot forever, so a deploy pipeline either hangs on its verification gate or — if it does not verify — promotes a snapshot it never actually exercised. The 0% leg matters the same way in reverse: a canary override leaking into base traffic silently changes production behavior.

Fix: the execute-api stage data plane's deployment selection — it must honour the stage's canarySettings (percentTraffic as the split, canarySettings.deploymentId as the snapshot for canary-routed requests, stageVariableOverrides layered over the stage variables for those requests only) — plus the UpdateStage patch handling for copy from /canarySettings/deploymentId and remove of /canarySettings.

Note: several neighbouring canary checks are already owned (20261109T152222Z-apigateway-createdeployment-createstage-preconditions-and-stage-name-wording.md covers canary-deployment-without-stage-name-rejected / create-deployment-canary-*; 20261109T152222Z-apigateway-stage-throttling-is-never-enforced-deleted-api.md covers canary-fifty-percent-split-serves-both-versions-within-tolerance). Those are refusal-path and split-tolerance causes and do NOT carry this check's replay key; the traffic-shift/promotion path above is net-new. Cause 1 is likewise distinct from 20261107T204751Z-apigateway-api-gateway-documentation-parts-method-location-parts-dropped.md, which covers documentation-part listings/filters/merge ids, not the failOnWarnings refusal message.

## Evidence
Cause 1 — apigateway.import-documentation-parts-bulk-atomic:

  File "check_apigateway_import_documentation_parts_bulk_atomic.py", line 120, in run
  assert blocked_message == (
  AssertionError: expected the unresolvable model reference to be named in the rejection, got "Warnings found during import:\n\tReference to model 'DoesNotExist' not found. Ignoring."

Expected Message (BadRequestException, HTTP 400):
  "Warnings found during import:\n"
  "\tReference to model 'DoesNotExist' not found. Ignoring.\n"
  "\tReference to model 'DoesNotExist' not found. Ignoring.\n"
  "\tInvalid format for model application/json for method GET"
Actual Message:
  "Warnings found during import:\n\tReference to model 'DoesNotExist' not found. Ignoring."

Fixed text vs values: fixed — "Warnings found during import:", "Reference to model '", "' not found. Ignoring.", "Invalid format for model ", " for method ". From the request document — the component schema name (DoesNotExist), the media type (application/json), the HTTP verb (GET). The error CODE (BadRequestException) and status (400) already match; only the warning list diverges.

Cause 2 — apigateway.canary-deployment-traffic-shift-and-promotion:

  #### [mismatch] apigateway.canary-deployment-traffic-shift-and-promotion
  Error: Check timed out after 60.0s

The check's awaited states, in order (each with its own 120 s poll):
  1. GET /release serves {"snapshot":"v1","release":"base"}            (base deployment)
  2. after CreateDeployment(stageName=<stage>, canarySettings={percentTraffic:100.0, stageVariableOverrides:{release:"canary"}, useStageCache:false}): GET /release serves {"snapshot":"v2","release":"canary"}
  3. after UpdateStage replace /canarySettings/percentTraffic = "0.0": GET /release serves {"snapshot":"v1","release":"base"} again
  4. after UpdateStage copy /canarySettings/deploymentId -> /deploymentId and remove /canarySettings: GET /release serves {"snapshot":"v2","release":"base"}
It also asserts GetStage keeps the stage's own deploymentId while the canary is attached, and that canarySettings round-trips deploymentId / percentTraffic=100.0 / stageVariableOverrides.release="canary".

Actual: none of the awaited bodies was observed inside the harness cap; the check was killed at 60.0s with no leg completing. (The api id, stage name and deployment ids are per-run values; the requirement is the snapshot/variable pairing at each step.)

## Steps to Reproduce
Cause 1:
1. CreateRestApi (REGIONAL).
2. ImportDocumentationParts(restApiId, mode="merge", body=<OpenAPI 3.0 doc with 3 explicit x-amazon-apigateway-documentation parts>) — expect >= 3 ids and GetDocumentationParts to reflect them.
3. ImportDocumentationParts(restApiId, mode="merge", failOnWarnings=true, body=<doc whose GET 200 response content application/json schema is {"$ref": "#/components/schemas/DoesNotExist"} and which declares one MODEL documentation part>).
4. Expect BadRequestException (400) whose Message is the 4-line text above (header + 2 model-reference lines + the Invalid-format line). Observed: a 2-line Message with one model-reference line.
5. Also assert GetDocumentationParts is unchanged by the refused call, then repeat with failOnWarnings=false and expect 200 with the same warnings in `warnings` and the MODEL part committed.

Cause 2:
1. CreateRestApi (REGIONAL); create a /release child resource with a MOCK GET whose 200 integration response template emits {"snapshot":"v1","release":"$util.escapeJavaScript($stageVariables.release)"}.
2. CreateDeployment; CreateStage(stageName=<s>, deploymentId=<base>, variables={release:"base"}). GET the stage URL until it serves snapshot v1 / release base.
3. PutIntegrationResponse rewriting the template to snapshot "v2"; CreateDeployment(stageName=<s>, canarySettings={percentTraffic:100.0, stageVariableOverrides:{release:"canary"}, useStageCache:false}).
4. GetStage: deploymentId must still be <base>; canarySettings must carry the new deployment id, percentTraffic 100.0 and the override.
5. GET the stage URL: expect snapshot v2 / release canary. Then UpdateStage replace /canarySettings/percentTraffic = "0.0" and expect snapshot v1 / release base. Then UpdateStage copy /canarySettings/deploymentId -> /deploymentId + remove /canarySettings and expect snapshot v2 / release base.

Observed: the check is killed by the 60 s harness cap before any of these data-plane states is observed.
