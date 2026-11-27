Issue-Source: trace-sweep-divergence
Issue-Trace: inspector2-code-security-scan-configuration-unknown-project-association
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: inspector2-code-security-scan-configuration-unknown-project-association
Issue-Shape-Id: com.amazonaws.inspector2#Inspector2
Issue-Key: inspector2|CreateCodeSecurityScanConfiguration|4xx->2xx|$.message
Issue-Key: inspector2|CreateCodeSecurityScanConfiguration|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: inspector2|CreateCodeSecurityScanConfiguration|4xx->2xx|status
Issue-Commit: eef1405ac0806ef267777bba6aefe2b3b0754197

# inspector2 CreateCodeSecurityScanConfiguration accepts a configuration with neither scan mode enabled instead of ValidationException

> _Note: this issue was identified against the code base at commit `eef1405ac0806ef267777bba6aefe2b3b0754197`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateCodeSecurityScanConfiguration

## Description
Single cause (Cause 1 of 1): CreateCodeSecurityScanConfiguration does not enforce the cross-member requirement that a CodeSecurityScanConfiguration enable at least one scan mode, so a request whose `configuration` carries neither `periodicScanConfiguration` nor `continuousIntegrationScanConfiguration` is created successfully instead of refused.

AWS: Smithy (services/service-com-amazonaws-inspector2-inspector2/smithy-model/com-amazonaws-inspector2-inspector2.json) models CreateCodeSecurityScanConfiguration with required input members `name`, `level` and `configuration`; `CodeSecurityScanConfiguration` has a required `ruleSetCategories` and two OPTIONAL members, `periodicScanConfiguration` and `continuousIntegrationScanConfiguration`. The model therefore cannot express the requirement that one of the two be present, and the recorded response is the authority for it: AWS rejects a configuration with neither with HTTP 400 and the modeled client error `ValidationException` (errorKind client, httpError 400; required members `message` and `reason`), surfaced on the wire via the `x-amzn-errortype` header. The error message is the fixed text `Either periodic scanning or continuous integration scanning must be enabled` in its entirety — it interpolates nothing from the request or from state, so there are no variable parts to substitute. Recorded behavior only for the exact wording and for which combination triggers it; everything about status and error class is also what the model prescribes for a malformed request.

Emulator: the replayed step got HTTP 200 with a success body. The response carries no `message` field (expected `Either periodic scanning or continuous integration scanning must be enabled`, got None) and no `x-amzn-errortype` header at all, i.e. the emulator created and persisted the scan configuration rather than refusing it.

Why: a client that relies on AWS's behavior treats the 200 as proof that a usable scan configuration now exists, and gets back a scanConfigurationArn for a configuration that can never scan anything — no periodic schedule and no CI trigger. Error-handling paths that catch ValidationException to re-prompt for a scan mode, and tests that assert the refusal, never fire; infrastructure tooling records the configuration as healthy. The divergence is silent and only shows up later as scans that never run.

Fix: the operation's input validation for CreateCodeSecurityScanConfiguration — the handler must apply the cross-member check on the `configuration` member after the modeled structural/constraint validation and before any state mutation, returning HTTP 400 with error type `ValidationException`, the message above, and a populated `reason`, leaving no configuration persisted. Nothing should be written to the scan-configuration state on the rejection path.

Traces: fixture inspector2-code-security-scan-configuration-unknown-project-association, step task#1 (CreateCodeSecurityScanConfiguration, expected 400, got 200).

Related but not covering this: open issue 20261125T182239Z-inspector2-getcodesecurityscanconfiguration-reports-a-synthesized-periodic-frequencyexpression-the.md concerns GetCodeSecurityScanConfiguration synthesizing a frequencyExpression on read, a different operation and a different fault.

## Evidence
Replay of fixture inspector2-code-security-scan-configuration-unknown-project-association, step task#1, operation CreateCodeSecurityScanConfiguration (verdict: mismatch).

Expected (recorded AWS response):
  HTTP 400
  headers: x-amzn-errortype present (ValidationException, per the modeled client error with httpError 400)
  body: $.message == "Either periodic scanning or continuous integration scanning must be enabled"

Actual (emulator):
  HTTP 200
  $.message: missing (None)
  $headers.x-amzn-errortype: missing (None)

Divergence key: service=inspector2|CreateCodeSecurityScanConfiguration|status=400->200|keys=$.message,$headers.x-amzn-errortype,status

Message anatomy: the whole string is fixed text; there are no request- or state-derived substitutions in it.

## Steps to Reproduce
1. Boot the emulator from this checkout and point an inspector2 client at it.
2. Call CreateCodeSecurityScanConfiguration (POST /codesecurity/scan-configuration/create) with a valid `name`, a valid `level`, and a `configuration` that sets `ruleSetCategories` but OMITS both `periodicScanConfiguration` and `continuousIntegrationScanConfiguration`.
3. Observe HTTP 200 and a returned scanConfigurationArn.
4. Expected: HTTP 400, x-amzn-errortype: ValidationException, body message "Either periodic scanning or continuous integration scanning must be enabled", and no configuration created.

Equivalently: replay fixture inspector2-code-security-scan-configuration-unknown-project-association and inspect step task#1.
