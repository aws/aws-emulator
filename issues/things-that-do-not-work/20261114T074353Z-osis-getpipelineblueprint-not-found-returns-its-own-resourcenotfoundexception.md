Issue-Source: trace-sweep-divergence
Issue-Trace: osis-pipeline-blueprint-discovery
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: osis-pipeline-blueprint-discovery
Issue-Shape-Id: com.amazonaws.osis#AmazonOpenSearchIngestionService
Issue-Key: osis|GetPipelineBlueprint|4xx->4xx|$.message
Issue-Commit: 7659f92acb00f1f48679164adf57090a6ffbf2ed

# osis GetPipelineBlueprint not-found returns its own ResourceNotFoundException message instead of AWS's wording

> _Note: this issue was identified against the code base at commit `7659f92acb00f1f48679164adf57090a6ffbf2ed`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetPipelineBlueprint

## Description
Replay of the OSIS wire corpus found ONE net-new divergence not covered by any open osis issue: GetPipelineBlueprint's blueprint-not-found error message. (The existing bundle 20261029T174701Z-osis-control-plane-responses-diverge-from-aws-across.md covers ListPipelineBlueprints' catalog contents, CreatePipeline validation, Start/StopPipeline not-found wording and ListPipelines; it does NOT mention GetPipelineBlueprint. 20261109T184855Z-osis-createpipeline-accepts-an-unassumable-sts-role-arn.md covers only CreatePipeline. Neither covers this cause.)

--- Cause 1 of 1: GetPipelineBlueprint on an unknown blueprint name uses a generic not-found message ---
AWS: Smithy GetPipelineBlueprint (GET /2022-01-01/osis/getPipelineBlueprint/{BlueprintName}, http code 200) takes BlueprintName as a required @httpLabel and models ResourceNotFoundException with errorKind=client and httpError 404, whose single member is `message` (shape doc: "You attempted to access or delete a resource that does not exist."). The model does not fix the message text, so this is recorded behavior only: the recording shows AWS answers 404 with message of the form `Blueprint: <BlueprintName> does not exists`. Fixed text is `Blueprint: ` and the trailing ` does not exists` (AWS's own wording, including the non-grammatical "does not exists" — that is the service's literal text, not a transcription slip); the ONLY variable part is <BlueprintName>, echoed verbatim from the request's BlueprintName path label. Note this wording is specific to the blueprint lookup and differs from the per-operation pipeline not-found wordings AWS uses for StartPipeline/StopPipeline, so one shared not-found string cannot be correct for all of them.
Emulator: returns the right status (404) but message `The blueprint <BlueprintName> could not be found` — a generic template of the emulator's own with different fixed text, a different word order and a leading article, where AWS prefixes the resource kind with a colon (`Blueprint: ...`).
Why: the error message is part of the modeled error contract. A client, SDK retry/classification wrapper, log scraper or test suite that recognises an unknown blueprint by matching AWS's `Blueprint: ... does not exists` text never matches against the emulator, so blueprint-discovery flows (resolve a name from ListPipelineBlueprints, then fetch its body before CreatePipeline) cannot distinguish "this blueprint name does not exist" from any other 404 and surface the wrong diagnostic to the user.
Fix: the GetPipelineBlueprint handler's blueprint-lookup miss path in the osis service — raise ResourceNotFoundException whose `message` is built as `Blueprint: ` + the requested BlueprintName + ` does not exists`, rather than reusing the service's generic "could not be found" not-found template. The status (404) and the error shape are already right; only the message construction changes.
Traces: osis-pipeline-blueprint-discovery#task#2.

## Evidence
replay_wire_corpus (trace_set simparity-wire-aws-v1): emulator_up=true, bootstrap_signal=false, steps_total=22, steps_passed=10, steps_mismatched=12, divergence_count=4, score_pct=45.5. Of the 4 clusters, 3 carry owned_by (already-filed osis issues) and are skipped; this one has stub_path="" and owned_by=[].

Cluster: fixture_id=osis-pipeline-blueprint-discovery, step_key=task#2, service=osis, operation=GetPipelineBlueprint, verdict=mismatch, expected_status=404, actual_status=404, affected_count=1.
divergence_key: service=osis|GetPipelineBlueprint|status=404->404|keys=$.message
detail: "$.message: value_mismatch (expected 'Blueprint: nonexistent-osis-blueprint-xyz does not exists', got 'The blueprint nonexistent-osis-blueprint-xyz could not be found')".

Expected vs actual, with the recording's values factored out: expected message template `Blueprint: {BlueprintName} does not exists`; actual message template `The blueprint {BlueprintName} could not be found`. Both echo the same requested name (the recorded instance used BlueprintName 'nonexistent-osis-blueprint-xyz', which is a per-fixture value and not part of the requirement). HTTP status matches at 404, so the divergence is exclusively the message's fixed text.

Model grounding (services/service-com-amazonaws-osis-amazonopensearchingestionservice/smithy-model/com-amazonaws-osis-amazonopensearchingestionservice.json): GetPipelineBlueprint input {BlueprintName: String, required, httpLabel; Format: BlueprintFormat, httpQuery 'format', pattern ^(YAML|JSON)$}, output {Blueprint: PipelineBlueprint, Format: String}, errors AccessDeniedException(403), DisabledOperationException(409), InternalException(500), ResourceNotFoundException(404), ValidationException(400); ResourceNotFoundException is a client error with httpError 404 and member `message`: ErrorMessage.

## Steps to Reproduce
Boot the emulator from this checkout and replay the osis fixtures of trace set simparity-wire-aws-v1 (replay_wire_corpus).

Cause 1: issue GET /2022-01-01/osis/getPipelineBlueprint/{BlueprintName} with a BlueprintName that is not in the blueprint catalog (fixture osis-pipeline-blueprint-discovery step task#2 uses 'nonexistent-osis-blueprint-xyz'). Expect HTTP 404, ResourceNotFoundException, and body message `Blueprint: <BlueprintName> does not exists` with the requested name echoed verbatim. The emulator returns 404 with `The blueprint <BlueprintName> could not be found`.

Re-replay fixture osis-pipeline-blueprint-discovery step task#2 to confirm the cause is closed.
