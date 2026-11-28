Issue-Source: trace-sweep-divergence
Issue-Trace: start-suite-run-empty-primary-device-rejected
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: start-suite-run-empty-primary-device-rejected stop-suite-run-id-outside-length-bound-not-found stop-suite-run-unknown-run-id-not-found
Issue-Shape-Id: com.amazonaws.iotdeviceadvisor#IotSenateService
Issue-Key: iotdeviceadvisor|StartSuiteRun|4xx->4xx|$.message
Issue-Key: iotdeviceadvisor|StopSuiteRun|4xx->4xx|$.message
Issue-Key: iotdeviceadvisor|StopSuiteRun|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: iotdeviceadvisor|StopSuiteRun|4xx->4xx|status
Issue-Key: iotdeviceadvisor|GetSuiteRun|4xx->4xx|$.message
Issue-Commit: 244eae870d957920c579d40eef12d136da6f5bab

# iotdeviceadvisor suite-run error statuses and messages diverge from AWS Device Advisor

> _Note: this issue was identified against the code base at commit `244eae870d957920c579d40eef12d136da6f5bab`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StopSuiteRun

## Description
The emulator's iotdeviceadvisor suite-run operations answer client errors with their own invented wording, and in one case with the wrong error class and HTTP status entirely. Four distinct causes, all in this service. BUNDLE OF 4 CAUSES — all four must be fixed before this issue is closed.

Cause 1 of 4: StartSuiteRun rejects an empty primaryDevice with non-AWS wording.
AWS: Smithy model com.amazonaws.iotdeviceadvisor — StartSuiteRun input member suiteRunConfiguration (required) targets SuiteRunConfiguration, whose member primaryDevice (required) targets DeviceUnderTest; DeviceUnderTest doc: 'A thing ARN, certificate ARN or device role ARN is required', primaryDevice doc: 'Sets the primary device for the test suite run. This requires a thing ARN or a certificate ARN.' The model does not state the message text, so for the text this is recorded behavior only: AWS answers HTTP 400 with the message 'Either thingArn or certificateArn needs to be provided for primaryDevice for regular MQTT protocol'. That whole string is FIXED text for this error class — it interpolates nothing from the request, so there are no variable parts to fill.
Emulator: HTTP 400 (status correct) but the message is 'suiteRunConfiguration.primaryDevice requires a thing ARN or a certificate ARN' — a self-authored sentence naming the input path rather than AWS's sentence.
Why: a client that branches on, logs, or asserts against the Device Advisor error message (the only thing distinguishing this validation failure from any other ValidationException, since both share status 400 and error code) sees text that never appears in AWS, so message-matching retry/diagnostic logic silently stops matching.
Fix: StartSuiteRun's input validation for suiteRunConfiguration.primaryDevice — emit AWS's fixed sentence instead of the locally-worded one.
Traces: start-suite-run-empty-primary-device-rejected#task#3.

Cause 2 of 4: StopSuiteRun enforces suiteRunId's @length minimum as a ValidationException instead of treating an under-length id as a missing resource.
AWS: Smithy model — StopSuiteRun input suiteRunId is an @httpLabel targeting UUID with @length min 12, max 36; the operation models both ValidationException (400) and ResourceNotFoundException (404). The recording shows that for a well-formed but short path label AWS does NOT raise the length constraint: with an 11-character suiteRunId it answers HTTP 404 ResourceNotFoundException with message 'Unable to find resource with SuiteDefinitionId: <suiteDefinitionId> and SuiteRunId: <suiteRunId>'. Fixed text: 'Unable to find resource with SuiteDefinitionId: ' and ' and SuiteRunId: '; variable parts: both ids taken verbatim from the request path, including the under-length one. Hypothesis: suiteRunId is resolved as an opaque lookup key, so a value outside the modeled length simply fails to resolve rather than failing constraint validation.
Emulator: HTTP 400 with x-amzn-errortype 'ValidationException' and message "1 validation error detected: Value '<suiteRunId>' at 'suiteRunId' failed to satisfy constraint: Member must have length greater than or equal to 12" — the wrong status, the wrong error type, and a message for an error class AWS never returns here.
Why: a client that catches ResourceNotFoundException to treat an unknown/expired suite run as absent instead gets an unhandled ValidationException and a 400, so a not-found path is misreported as a malformed request and any 404-based cleanup or idempotency branch never runs.
Fix: StopSuiteRun's handler/validation — stop pre-validating the suiteRunId @length lower bound on this path label and let the state lookup produce ResourceNotFoundException.
Traces: stop-suite-run-id-outside-length-bound-not-found#task#3 (and #task#5).

Cause 3 of 4: StopSuiteRun's ResourceNotFoundException message omits the suite definition id and mis-cases the member name.
AWS: recorded behavior only for the text (the model types ResourceNotFoundException's single `message` member, httpError 404, but states no wording). AWS answers 404 with 'Unable to find resource with SuiteDefinitionId: <suiteDefinitionId> and SuiteRunId: <suiteRunId>'. Fixed text: 'Unable to find resource with SuiteDefinitionId: ' and ' and SuiteRunId: '; variable parts: the suiteDefinitionId and suiteRunId from the request path. Note the capitalized, type-style names SuiteDefinitionId/SuiteRunId, not the lowerCamel wire member names.
Emulator: status 404 is correct, but the message is 'Unable to find resource with suiteRunId: <suiteRunId>' — it drops the SuiteDefinitionId clause entirely and lowercases the run-id name.
Why: the message is the only place the response says WHICH pair of ids failed to resolve; dropping the definition id makes the error ambiguous for a client stopping runs across several suite definitions, and the casing difference breaks any parse of AWS's message shape.
Fix: StopSuiteRun's not-found error construction/serializer — render both ids using AWS's two-clause template and capitalization.
Traces: stop-suite-run-id-outside-length-bound-not-found#task#4 (and stop-suite-run-unknown-run-id-not-found#task#3).

Cause 4 of 4: GetSuiteRun's ResourceNotFoundException message mis-cases the member name.
AWS: recorded behavior only for the text; GetSuiteRun models ResourceNotFoundException (404). AWS answers 404 with 'Unable to find resource with SuiteRunId: <suiteRunId>'. Fixed text: 'Unable to find resource with SuiteRunId: '; variable part: the suiteRunId from the request path. Unlike StopSuiteRun, this form names only the run id.
Emulator: status 404 is correct, message is 'Unable to find resource with suiteRunId: <suiteRunId>' — lowerCamel 'suiteRunId' where AWS writes 'SuiteRunId'.
Why: clients and test harnesses that assert on the AWS message text for a missing suite run stop matching, and the divergence shows the emulator shares one locally-worded not-found template across operations instead of each operation's AWS wording.
Fix: GetSuiteRun's not-found error construction/serializer — use AWS's single-clause template with the capitalized name.
Traces: stop-suite-run-unknown-run-id-not-found#task#4.

Not covered by any existing open iotdeviceadvisor issue: the open issues 'iotdeviceadvisor StartSuiteRun returns a port-qualified, non-served endpoint...' and 'iotdeviceadvisor: 2 net-new causes — GetEndpoint now answers a bare 127.0.0.1:<port>...' concern the Endpoint value returned on SUCCESS, not these four client-error responses, and none of these replay steps was reported as owned by an open issue.

## Evidence
Replay of trace set simparity-wire-aws-v1 (7 fixtures, 37 steps, 31 passed, 6 mismatched, score 83.8%); all 4 divergence clusters are iotdeviceadvisor, none carried stub_path or owned_by.

Cause 1 — start-suite-run-empty-primary-device-rejected#task#3, StartSuiteRun, status 400 -> 400:
  expected $.message: 'Either thingArn or certificateArn needs to be provided for primaryDevice for regular MQTT protocol'
  actual   $.message: 'suiteRunConfiguration.primaryDevice requires a thing ARN or a certificate ARN'

Cause 2 — stop-suite-run-id-outside-length-bound-not-found#task#3 (also #task#5), StopSuiteRun:
  expected status 404, actual 400
  expected $.message: 'Unable to find resource with SuiteDefinitionId: <definition-id> and SuiteRunId: <run-id>' (ResourceNotFoundException)
  actual   $.message: "1 validation error detected: Value '<run-id>' at 'suiteRunId' failed to satisfy constraint: Member must have length greater than or equal to 12"
  expected $headers.x-amzn-errortype: <shaped-not-asserted>, actual 'ValidationException'

Cause 3 — stop-suite-run-id-outside-length-bound-not-found#task#4 (also stop-suite-run-unknown-run-id-not-found#task#3), StopSuiteRun, status 404 -> 404:
  expected $.message: 'Unable to find resource with SuiteDefinitionId: <definition-id> and SuiteRunId: <run-id>'
  actual   $.message: 'Unable to find resource with suiteRunId: <run-id>'

Cause 4 — stop-suite-run-unknown-run-id-not-found#task#4, GetSuiteRun, status 404 -> 404:
  expected $.message: 'Unable to find resource with SuiteRunId: <run-id>'
  actual   $.message: 'Unable to find resource with suiteRunId: <run-id>'

Model evidence (smithy_query on services/service-com-amazonaws-iotdeviceadvisor-iotsenateservice/smithy-model/com-amazonaws-iotdeviceadvisor-iotsenateservice.json): StopSuiteRun/GetSuiteRun suiteDefinitionId+suiteRunId are @httpLabel UUID @length(12,36), errors {ValidationException 400, ResourceNotFoundException 404, InternalServerException 500}; StartSuiteRun errors {ConflictException 400, ValidationException 400, InternalServerException 500}; SuiteRunConfiguration.primaryDevice required, DeviceUnderTest doc 'A thing ARN, certificate ARN or device role ARN is required'.

## Steps to Reproduce
1. git clone the repo and boot the emulator from that checkout.
2. Replay the simparity-wire-aws-v1 wire corpus (replay_wire_corpus).
3. Cause 1: fixture start-suite-run-empty-primary-device-rejected, step task#3 — StartSuiteRun with suiteRunConfiguration.primaryDevice present but carrying neither thingArn nor certificateArn; compare $.message against AWS's fixed sentence.
4. Cause 2: fixture stop-suite-run-id-outside-length-bound-not-found, step task#3 (repeats at task#5) — StopSuiteRun on an existing suiteDefinitionId with a suiteRunId shorter than UUID's @length min of 12; expect 404 ResourceNotFoundException, observe 400 ValidationException.
5. Cause 3: same fixture, step task#4 (repeats as stop-suite-run-unknown-run-id-not-found#task#3) — StopSuiteRun with a 12+ character but unknown suiteRunId; expect the two-clause 'SuiteDefinitionId: ... and SuiteRunId: ...' message.
6. Cause 4: fixture stop-suite-run-unknown-run-id-not-found, step task#4 — GetSuiteRun with an unknown suiteRunId; expect 'Unable to find resource with SuiteRunId: <run-id>'.
