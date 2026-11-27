Issue-Source: trace-sweep-divergence
Issue-Trace: appintegrations-data-integration-get-roundtrip
Issue-Step: task#4
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: appintegrations-data-integration-get-roundtrip
Issue-Shape-Id: com.amazonaws.appintegrations#AmazonAppIntegrationService
Issue-Key: app-integrations|GetDataIntegration|2xx->2xx|$.IntegrationType
Issue-Commit: af007de286377294faf8721f963c7a04ad7dcdf8

# GetDataIntegration response omits IntegrationType for DataIntegrations created by CreateDataIntegration

> _Note: this issue was identified against the code base at commit `af007de286377294faf8721f963c7a04ad7dcdf8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetDataIntegration

## Description
One cause.

Cause 1 of 1: GetDataIntegration omits the IntegrationType member from its 200 response body.

AWS: recorded behavior only — the on-disk Smithy model for this service (services/service-com-amazonaws-appintegrations-amazonappintegrationservice/smithy-model/com-amazonaws-appintegrations-amazonappintegrationservice.json) models GetDataIntegration as GET /dataIntegrations/{Identifier} -> 200 with output shape GetDataIntegrationResponse carrying Arn, Id, Name, Description, KmsKey, SourceURI, ScheduleConfiguration, Tags, FileConfiguration and ObjectConfiguration; it does not declare an IntegrationType member, so the model says nothing about this field. The recording shows the real service returning, on a GET of a DataIntegration that was created earlier in the same fixture, a 200 whose JSON body additionally contains the member IntegrationType with the value 'IMPORT'. The recorded value is the integration kind the resource was created as, derived from the create request/stored resource, not a literal constant of this one run; the requirement is that the member is present on every successful GetDataIntegration and reports the stored integration type of the identified DataIntegration.

Emulator: the emulator answers the same request with HTTP 200 and the correct remaining members (every other asserted key on the step matched), but the response body has no IntegrationType key at all — replay reports '$.IntegrationType: missing (expected \'IMPORT\', got None)'. Status and error code are not in question here: expected 200, actual 200; only the body is short one member.

Why: a client that reads the integration kind back from GetDataIntegration — to branch on an import-style versus other-style integration, to round-trip a resource description, or to render the resource in a console/IaC refresh — gets a null/absent field instead of the kind. In a typed SDK the member deserializes as None/null, so code that switches on it takes the wrong branch or throws on an unexpected null; an IaC refresh sees the attribute drift from the value it created the resource with. Because the GET otherwise succeeds, the failure is silent: nothing in the status or error signals that part of the resource was dropped.

Fix: the GetDataIntegration read path of the appintegrations service — the operation's output serializer must emit IntegrationType, and the DataIntegration state record must carry the integration type captured at CreateDataIntegration time so the getter has a value to emit (if the create handler never persists it, that is the same defect's other half). Do not hardcode 'IMPORT': emit the kind stored for the resource named by the Identifier path label.

Traces: appintegrations-data-integration-get-roundtrip#task#4 (GetDataIntegration, verdict mismatch, divergence key service=app-integrations|GetDataIntegration|status=200->200|keys=$.IntegrationType). No other fixture step in the corpus exercises this member.

Note on existing issues: the two open appintegrations issues (AWS::AppIntegrations::Application/EventIntegration and AWS::AppIntegrations::DataIntegration not being registered CloudFormation resource types) are about CloudFormation resource-type registration and do not cover this response-shape gap in the native GetDataIntegration API.

## Evidence
Fixture appintegrations-data-integration-get-roundtrip, step task#4, operation GetDataIntegration (GET /dataIntegrations/{Identifier}).

Expected (recorded AWS response): HTTP 200, JSON body including "IntegrationType": "IMPORT" alongside the DataIntegration's Arn/Id/Name/KmsKey/SourceURI/etc.
Actual (emulator): HTTP 200, JSON body identical on every asserted key except that "IntegrationType" is absent entirely.

Replay detail, verbatim: "$.IntegrationType: missing (expected 'IMPORT', got None)".
Verdict: mismatch. expected_status 200, actual_status 200. affected_count 1.
Smithy reference consulted: GetDataIntegrationResponse members (Arn, Id, Name, Description, KmsKey, SourceURI, ScheduleConfiguration, Tags, FileConfiguration, ObjectConfiguration) — IntegrationType is not modeled on disk, so the recording is the only authority for it.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay fixture appintegrations-data-integration-get-roundtrip.
2. The fixture creates a DataIntegration (CreateDataIntegration, POST /dataIntegrations) and then, at step task#4, issues GetDataIntegration (GET /dataIntegrations/{Identifier}) for it.
3. Inspect the 200 response body of step task#4: the key IntegrationType is absent, where the recorded AWS response carries the resource's integration type (recorded as "IMPORT").
4. Equivalent by hand: POST /dataIntegrations to create a DataIntegration, then GET /dataIntegrations/<returned Id or Name> and diff the response keys against the recorded response.
