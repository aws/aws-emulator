Issue-Source: trace-sweep-divergence
Issue-Trace: lam-err-code-signing-warn-then-enforce
Issue-Step: task#6
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: lam-err-code-signing-warn-then-enforce lambda-code-signing-config-enforcement lambda-codesigning-config-crud-and-list lambda-function-codesigning-attach-detach-lifecycle lambda-function-codesigning-attach-error-paths
Issue-Shape-Id: com.amazonaws.lambda#AWSGirApiService
Issue-Key: lambda|UpdateFunctionCode|4xx->2xx|$.Message
Issue-Key: lambda|UpdateFunctionCode|4xx->2xx|$.Type
Issue-Key: lambda|UpdateFunctionCode|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: lambda|UpdateFunctionCode|4xx->2xx|status
Issue-Key: lambda|CreateFunction|4xx->2xx|$.Message
Issue-Key: lambda|CreateFunction|4xx->2xx|$.Type
Issue-Key: lambda|DeleteFunction|4xx->2xx|status
Issue-Key: lambda|GetCodeSigningConfig|2xx->2xx|$.CodeSigningConfig.Description
Issue-Key: lambda|UpdateCodeSigningConfig|2xx->2xx|$.CodeSigningConfig.Description
Issue-Key: lambda|ListCodeSigningConfigs|2xx->2xx|$.CodeSigningConfigs[*]
Issue-Key: lambda|GetFunctionCodeSigningConfig|2xx->2xx|$.FunctionName
Issue-Related: 20261029T233239Z-lambda-does-not-enforce-the-timeout-and-memorysize.md
Issue-Commit: 6fcb2038be76f91f904ab31cfab5a8d60c9030f7

# Lambda code signing is never enforced and code-signing config payloads omit Description

> _Note: this issue was identified against the code base at commit `6fcb2038be76f91f904ab31cfab5a8d60c9030f7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateFunctionCode

## Description
Shared fault: a code-signing configuration whose policy is Enforce does not block an unsigned/untrusted deployment, and the code-signing-config serializer omits members the recorded responses carry. Seven net-new clusters.

AWS: Smithy binds CodeVerificationFailedException (httpError 400, members {Type, Message}) to both CreateFunction and UpdateFunctionCode: "The code signature failed one or more of the validation checks ... and the code signing policy is set to ENFORCE. Lambda blocks the deployment." The recorded refusal Message is `Lambda cannot deploy the function. The function or layer might be signed using a signature that the client is not configured to accept. Check the provided signature for <resource>.` — the two sentences are fixed text and `<resource>` is the function ARN (UpdateFunctionCode) or the function name (CreateFunction) from the request. Smithy CodeSigningConfig declares `Description` (@length 0..256); the recordings return it on GetCodeSigningConfig/UpdateCodeSigningConfig (empty string when unset) and as a member of each ListCodeSigningConfigs entry. GetFunctionCodeSigningConfigResponse: the recorded responses carry only `CodeSigningConfigArn` — recorded behavior only, and worth noting the on-disk model marks FunctionName required while AWS does not return it.

Emulator: the Enforce policy is ignored — UpdateFunctionCode answers 200 and CreateFunction answers 201 — so the deployment that AWS blocks lands; `CodeSigningConfig.Description` is omitted whenever unset; GetFunctionCodeSigningConfig adds a `FunctionName` member.

Why: code signing is a deployment control — a client that attaches an Enforce config and expects untrusted code to be refused gets it deployed instead, and the fixture's teardown shows the divergence persisting (a function AWS never created exists in the emulator).

Fix: CreateFunction's and UpdateFunctionCode's validation must consult the function's attached code-signing config and raise CodeVerificationFailedException (400) with AWS's Message when the policy is Enforce; the code-signing-config serializer must render Description and GetFunctionCodeSigningConfig must return only the recorded member set.

Cause 1 of 7: UpdateFunctionCode — fixture lam-err-code-signing-warn-then-enforce step task#6; expected 400 CodeVerificationFailedException naming the function ARN, emulator answered 200 and updated the code.
Cause 2 of 7: CreateFunction — fixture lam-err-code-signing-warn-then-enforce step task#7; expected 400 CodeVerificationFailedException naming the function name, emulator answered 201 and created the function.
Cause 3 of 7: DeleteFunction — fixture lam-err-code-signing-warn-then-enforce step teardown#10; expected 404 (AWS never created the function), emulator answered 204 — the consequence of cause 2 persisting in state.
Cause 4 of 7: GetCodeSigningConfig, 200 — fixture lambda-code-signing-config-enforcement step task#4; `CodeSigningConfig.Description` omitted.
Cause 5 of 7: UpdateCodeSigningConfig, 200 — fixture lambda-code-signing-config-enforcement step task#5; same member omitted.
Cause 6 of 7: ListCodeSigningConfigs, 200 — fixture lambda-code-signing-config-enforcement step task#9; the entry omits the `Description` member the recording renders.
Cause 7 of 7: GetFunctionCodeSigningConfig, 200 — fixture lambda-code-signing-config-enforcement step task#13; emulator adds a `FunctionName` member the recorded response does not contain.

## Evidence
Cluster details: task#6 `status: expected 400, got 200` with `$.Message: missing (expected 'Lambda cannot deploy the function. ... Check the provided signature for arn:aws:lambda:...:function:<name>.')`; task#7 `status: expected 400, got 201` with the same Message naming the function name; teardown#10 `status: expected 404, got 204`; task#4/task#5 `$.CodeSigningConfig.Description: missing (expected '', got None)`; task#9 `$.CodeSigningConfigs[0]: value_mismatch` where only the `Description` member differs; task#13 `$.FunctionName: type_mismatch (expected None, got '<function name>')`. Smithy: CodeVerificationFailedException httpError 400 bound to CreateFunction and UpdateFunctionCode; CodeSigningConfig.Description.

## Steps to Reproduce
Replay the corpus. Fixture lam-err-code-signing-warn-then-enforce step task#6: attach an Enforce code-signing config and call UpdateFunctionCode with an unsigned zip — expect 400 CodeVerificationFailedException, observe 200. Fixture lambda-code-signing-config-enforcement step task#4: GetCodeSigningConfig and look for Description.
