Issue-Source: trace-sweep-divergence
Issue-Trace: codeartifact-repository-error-paths
Issue-Step: task#7
Issue-Severity: LOW
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: codeartifact-repository-error-paths
Issue-Shape-Id: com.amazonaws.codeartifact#CodeArtifactControlPlaneService
Issue-Key: codeartifact|AssociateExternalConnection|4xx->4xx|$.message
Issue-Commit: ac9e642ebb6d7cd67632ea3e08d8c560fc4b2f43

# CodeArtifact AssociateExternalConnection unknown-connection 404 message does not match the recorded ResourceNotFoundException text

> _Note: this issue was identified against the code base at commit `ac9e642ebb6d7cd67632ea3e08d8c560fc4b2f43`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AssociateExternalConnection

## Description
Cause 1 of 1: AssociateExternalConnection, asked to attach an external connection name that is not a real CodeArtifact upstream, returns the right status and error type but the wrong error message text.

AWS: Smithy (com-amazonaws-codeartifact-codeartifactcontrolplaneservice): AssociateExternalConnection POST /v1/repository/external-connection, required httpQuery members domain / repository / external-connection, modeled errors include ResourceNotFoundException (httpError 404) and ValidationException (httpError 400). The model does not specify any message wording, so for the text this is recorded behavior only: the fixture step records a 404 whose `message` member is captured in shaped (non-literal) form, i.e. AWS does return a ResourceNotFoundException body carrying a `message`, and the emulator's literal sentence does not conform to the recorded shape. The requirement is therefore the error class as AWS words it for "this external connection does not exist": a fixed not-found sentence naming the external-connection resource, whose only variable part is the external connection name echoed from the request's `external-connection` query parameter (here a value of the form `public:<name>`); nothing else in the message is request- or run-derived. Hypothesis: CodeArtifact words its not-found messages with the service's `<Resource> not found. <Resource> = <value>` template rather than single-quoting the value — the same corpus records DescribePackage as `Package not found in repository. Package = {format=generic, name=<name>}` — so the expected text is the external-connection analogue of that template, not a re-authored sentence.

Emulator: answers 404 (status matches) with message "External connection 'public:does-not-exist' not found." — its own phrasing, single-quoting the connection name instead of using the recorded message shape. Replay verdict: mismatch on `$.message`, `value_mismatch (expected '<shaped-not-asserted>', got "External connection 'public:does-not-exist' not found.")`; expected_status 404, actual_status 404, so only the message text diverges.

Why: a client that keys off the error message — log scrapers, CLI/SDK retry or remediation logic, and tests that assert on the not-found sentence for an unsupported upstream — sees text AWS never emits, so the same failure reads differently against the emulator than against AWS. The status and error code are right, so the breakage is silent: the caller believes it is handling the real AWS error path.

Fix: the error-construction path of the codeartifact AssociateExternalConnection handler (the branch that rejects an unknown/unsupported external connection name) must emit the ResourceNotFoundException message in AWS's wording for that error class, built from the request's `external-connection` value, rather than a locally invented sentence. The same wording should be shared with DisassociateExternalConnection, which has the identical error set.

Related open issues do NOT cover this: 20261113T223143Z-codeartifact-external-connections-ingest-nothing-the-npm-and, 20261029T095746Z-codeartifact-npm-external-connection-ingests-nothing and 20261027T083906Z-codeartifact-reopens-codeartifact-pypi-repository-endpoint-still-ingests-nothing are about the data plane not resolving upstream packages for an ASSOCIATED connection, not about the control-plane error message for a NON-EXISTENT one; 20261103T055241Z-codeartifact-describepackage-not-found-message-includes-a-namespace is the DescribePackage message. This cluster carried no `owned_by`, so no filed issue holds its replay key.

Traces: codeartifact-repository-error-paths#task#7.

## Evidence
Fixture codeartifact-repository-error-paths, step task#7, operation AssociateExternalConnection, service codeartifact.
Expected (recorded AWS): HTTP 404, ResourceNotFoundException, `message` recorded in shaped form ('<shaped-not-asserted>').
Actual (emulator): HTTP 404 with $.message = "External connection 'public:does-not-exist' not found."
Replay detail: "$.message: value_mismatch (expected '<shaped-not-asserted>', got \"External connection 'public:does-not-exist' not found.\")"; divergence_key service=codeartifact|AssociateExternalConnection|status=404->404|keys=$.message; verdict mismatch; affected_count 1; stub_path empty; owned_by empty.
Message anatomy: fixed text = the not-found sentence and its resource label; variable = the external connection name taken from the request's `external-connection` query parameter (`public:does-not-exist` in this recording, not part of the requirement).

## Steps to Reproduce
1. Clone the repo and boot the emulator built from it.
2. Replay the wire corpus (trace set simparity-wire-aws-v1); fixture codeartifact-repository-error-paths, step task#7.
3. The step creates a domain and repository, then calls AssociateExternalConnection with domain/repository set to the fixture's resources and external-connection set to a connection name that does not exist (e.g. `public:does-not-exist`).
4. Observe the emulator replies 404 with message "External connection 'public:does-not-exist' not found.", which does not match the recorded ResourceNotFoundException message for that request; the status 404 itself is correct.
5. Re-replay codeartifact-repository-error-paths#task#7 after the fix to confirm the message matches.
