Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iotdeviceadvisor.start-suite-run-unknown-definition-version-rejected
Issue-Shape-Id: com.amazonaws.iotdeviceadvisor#IotSenateService
Issue-Key: iotdeviceadvisor.start-suite-run-unknown-definition-version-rejected|run#13|AssertionError|assert (code, message) == (
Issue-Commit: 4866adc0993f8fa67cc0ae020b08b6d4f21694d6

# iotdeviceadvisor StartSuiteRun on an unknown suiteDefinitionVersion answers ValidationException instead of ResourceNotFoundException with AWS's SuiteDefinitionId wording

> _Note: this issue was identified against the code base at commit `4866adc0993f8fa67cc0ae020b08b6d4f21694d6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartSuiteRun

## Description
BUNDLE OF 1 CAUSE — one net-new root cause; it must be fixed before this issue is closed.

Cause 1 of 1: StartSuiteRun pinned to a suite-definition version that was never created is refused with the wrong error class and a self-authored message.

AWS: Smithy model com.amazonaws.iotdeviceadvisor#IotSenateService — StartSuiteRun is POST /suiteDefinitions/{suiteDefinitionId}/suiteRuns; input member suiteDefinitionId is an @httpLabel targeting UUID @length(12,36) and suiteDefinitionVersion targets SuiteDefinitionVersion @length(2,255). The modeled error set is {ConflictException 400, ValidationException 400, InternalServerException 500} — the model does NOT model ResourceNotFoundException on this operation and states nothing about this case, so for the class, status and text this is recorded behavior only: when the named suiteDefinitionId exists but the requested suiteDefinitionVersion does not, AWS refuses with error code ResourceNotFoundException (HTTP 404) and message 'Unable to find resource with SuiteDefinitionId: <suiteDefinitionId>'. Fixed text: 'Unable to find resource with SuiteDefinitionId: '; the single variable part is the suiteDefinitionId taken verbatim from the request path. Note that AWS keys the not-found on the DEFINITION id in capitalized, type-style casing and does not mention the version at all, and that the refusal records no suite run (a following ListSuiteRuns for that definition is empty). The same request naming the definition's existing latestVersion ('v1' for a freshly created definition) is accepted.

Emulator: the refusal carries error code 'ValidationException' with message 'Unable to find resource with suiteDefinitionVersion: v7' — the wrong error class (and therefore HTTP 400 where AWS answers 404), and a locally-worded message that names the lowerCamel wire member suiteDefinitionVersion and echoes the requested version string instead of naming SuiteDefinitionId with the definition id from the path. The variable part is also the wrong field: the version, not the definition id.

Why: a client that catches ResourceNotFoundException to treat a pinned suite-definition version as absent — to fall back to latestVersion, to re-publish the version, or to drive an idempotent create-then-start flow — instead receives an unhandled ValidationException with a 400. A 400 ValidationException reads as a malformed request, so retry/fallback logic keyed on not-found never runs and callers surface a bad-input error for a perfectly well-formed request. Message-matching diagnostics also stop matching, since neither the fixed text ('SuiteDefinitionId:') nor the interpolated value (the definition id) appears.

Fix: StartSuiteRun's handler and its suite-definition-version resolution in the iotdeviceadvisor service — resolve the (suiteDefinitionId, suiteDefinitionVersion) pair against state and raise the not-found error class (404 ResourceNotFoundException) rather than input-validation's ValidationException, and build the message from AWS's single-clause SuiteDefinitionId template with the path's definition id. This is the same shared, locally-worded not-found template already reported for the suite-run operations in the open issue '20261128T001905Z-iotdeviceadvisor-suite-run-error-statuses-and-messages-diverge.md'; that issue covers StartSuiteRun's EMPTY-primaryDevice message and StopSuiteRun/GetSuiteRun not-found messages, and does NOT cover StartSuiteRun's unknown-suiteDefinitionVersion refusal, which is this cause. The open issues about the endpoint value ('...port-qualified, non-served endpoint...' and '...2 net-new causes — GetEndpoint...') concern successful responses, not this refusal.

Traces: behavioral check iotdeviceadvisor.start-suite-run-unknown-definition-version-rejected.

## Evidence
Behavioral suite simparity-behavioral-aws-v1, 19/24 passed. Failing check iotdeviceadvisor.start-suite-run-unknown-definition-version-rejected (verdict: mismatch), not marked ALREADY FILED and not marked OWNED by any open issue.

Assertion from the replay:
  check_iotdeviceadvisor_start_suite_run_unknown_definition_version_rejected.py, line 50:
    assert (code, message) == ("ResourceNotFoundException", f"Unable to find resource with SuiteDefinitionId: {suite_id}")
  AssertionError: starting a run of version 'v7' that was never created gave ('ValidationException', 'Unable to find resource with suiteDefinitionVersion: v7')

Expected (AWS, recorded): error code ResourceNotFoundException, HTTP 404, message 'Unable to find resource with SuiteDefinitionId: <suiteDefinitionId>' where <suiteDefinitionId> is the id in the request path.
Actual (emulator): error code ValidationException (HTTP 400), message 'Unable to find resource with suiteDefinitionVersion: <requested-version>'.

Model evidence (smithy_query on services/service-com-amazonaws-iotdeviceadvisor-iotsenateservice/smithy-model/com-amazonaws-iotdeviceadvisor-iotsenateservice.json): StartSuiteRun http POST /suiteDefinitions/{suiteDefinitionId}/suiteRuns, code 200; input suiteDefinitionId @httpLabel UUID @length(12,36), suiteDefinitionVersion @length(2,255); errors {ConflictException 400, ValidationException 400, InternalServerException 500} — ResourceNotFoundException is unmodeled here, so the 404 class and the message wording are recorded behavior only.

The check's surrounding assertions (ListSuiteRuns empty after the refusal, then a successful start of the existing 'v1' and its GetSuiteRun/ListSuiteRuns/StopSuiteRun round trip) were never reached, because the run aborted on the error-class/message assertion.

## Steps to Reproduce
1. git clone the repo and boot the emulator from that checkout.
2. Run the behavioral suite (run_behavioral_suite) and select check iotdeviceadvisor.start-suite-run-unknown-definition-version-rejected.
3. CreateSuiteDefinition with a suiteDefinitionConfiguration carrying a rootGroup with one MQTT_Connect case, a devicePermissionRoleArn and one device; keep the returned suiteDefinitionId.
4. GetSuiteDefinition(suiteDefinitionId) and confirm latestVersion == 'v1'.
5. Call StartSuiteRun(suiteDefinitionId=<that id>, suiteDefinitionVersion='v7' (any version never created), suiteRunConfiguration={'primaryDevice': <the device>}).
6. Observe the refusal: expected error code ResourceNotFoundException with HTTP 404 and message 'Unable to find resource with SuiteDefinitionId: <suiteDefinitionId>'; the emulator answers ValidationException (HTTP 400) with 'Unable to find resource with suiteDefinitionVersion: v7'.
7. Then confirm the rest of the check's expectations once the class/message are fixed: ListSuiteRuns(suiteDefinitionId) is empty after the refusal, and StartSuiteRun with suiteDefinitionVersion='v1' is accepted and round-trips through GetSuiteRun/ListSuiteRuns/StopSuiteRun to STOPPED.
