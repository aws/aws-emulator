Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway.access-log-format-cap-counts-characters-not-utf8-bytes apigateway.access-log-format-cap-counts-supplementary-characters-as-two apigateway.access-log-format-over-three-thousand-characters-rejected apigateway.access-log-format-with-line-break-rejected apigateway.import-rest-api-definition-past-six-mib-rejected apigateway.put-rest-api-definition-past-six-mib-rejected apigateway.request-template-twice-the-300-kib-limit-rejected apigateway.put-integration-oversized-request-template-rejected-keeps-previous apigateway.create-deployment-canary-on-missing-stage-rejected
Issue-Shape-Id: com.amazonaws.apigateway#BackplaneControlService
Issue-Key: api-gateway|UpdateStage|BadRequestException
Issue-Key: apigateway.access-log-format-with-line-break-rejected|_error_of#13|AssertionError|raise AssertionError(f"the format update was expected to fail, got {res!r}")
Issue-Key: api-gateway|ImportRestApi|BadRequestException
Issue-Key: api-gateway|PutRestApi|BadRequestException
Issue-Key: api-gateway|PutIntegration|BadRequestException
Issue-Key: apigateway.put-integration-oversized-request-template-rejected-keeps-previous|run#12|AssertionError|assert got == ("BadRequestException", _TOO_LARGE), f"oversized template answered {got!r}"
Issue-Key: apigateway.create-deployment-canary-on-missing-stage-rejected|run#8|AssertionError|assert message == expected, f"a canary on a missing stage gave message {message!r}"
Issue-Commit: 9bfc3dcca28986b1e030db2b6d5b17f08a507141

# apigateway: 5 net-new control-plane validation causes — accessLogSettings/format, ImportRestApi/PutRestApi body and requestTemplates size caps are far below AWS's, plus two wrong refusal messages

> _Note: this issue was identified against the code base at commit `9bfc3dcca28986b1e030db2b6d5b17f08a507141`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateStage

## Description
Five independent control-plane validation causes in the API Gateway REST control plane. Four of them refuse requests AWS accepts (the emulator's limits are an order of magnitude smaller than AWS's), and all five answer with the emulator's own sentence instead of AWS's message text.

Cause 1 of 5: UpdateStage caps /accessLogSettings/format at 512 characters instead of 3000.
AWS: Smithy (com-amazonaws-apigateway-backplanecontrolservice) models UpdateStage.patchOperations as ListOfPatchOperation and Stage.accessLogSettings as AccessLogSettings with NO @length on `format`; the limit is API Gateway's documented stage quota of 3000 characters for the access log format, counted in CHARACTERS (not UTF-8 bytes, and a supplementary character counts as the two UTF-16 units it is made of). A format at the cap is stored and read back byte-identical by GetStage; one character past it is refused 400 BadRequestException with the fixed text `Access log format must be less than or equal to 3000 characters` (the number is the limit, the only variable part), and the previously stored format is left untouched.
Emulator: UpdateStage refuses a 3000-character format with 400 BadRequestException and the message `/accessLogSettings/format may be at most 512 characters, got 3000` (and `... got 1510` for the supplementary-character form) — a 512-character cap and a JSON-pointer-style sentence that is not AWS's wording.
Why: a client that configures any realistic access log format (AWS's own sample formats are well over 512 characters) cannot enable access logging at all, and a client that matches on the refusal message never sees AWS's sentence.
Fix: the UpdateStage handler's patch validation for /accessLogSettings/format — raise the cap to 3000 and count characters, and emit AWS's message.
Traces: apigateway.access-log-format-cap-counts-characters-not-utf8-bytes, apigateway.access-log-format-cap-counts-supplementary-characters-as-two, apigateway.access-log-format-over-three-thousand-characters-rejected

Cause 2 of 5: an access log format containing a line break is accepted.
AWS: recorded behavior only — the model puts no @pattern on AccessLogSettings.format. AWS refuses a format whose text contains an embedded newline with 400 BadRequestException and leaves the stage's stored format unchanged.
Emulator: UpdateStage returns 200 and GetStage reports `accessLogSettings.format` = `$context.requestId $context.httpMethod\n$context.status`, i.e. the newline-bearing format was stored.
Why: a client that accidentally embeds a newline gets a stage that AWS would never have accepted, so the divergence is only discovered against real AWS.
Fix: the UpdateStage handler's /accessLogSettings/format validation (reject control characters / line breaks).
Traces: apigateway.access-log-format-with-line-break-rejected

Cause 3 of 5: ImportRestApi and PutRestApi cap the body at 1048576 bytes instead of 6 MiB.
AWS: Smithy models ImportRestApi.body and PutRestApi.body as a required @httpPayload Blob with NO @length; the limit is API Gateway's documented 6 MiB (6291456 bytes) API-definition import size. A definition exactly at 6 MiB is imported/overwritten successfully (GetRestApi reports the document's title as the API name and the document's resource tree is built); one byte past it is refused 400 BadRequestException with the fixed text `Body length exceeds maximum import size.` and no API is created and no existing API is modified.
Emulator: both operations refuse a 6291456-byte body with 400 BadRequestException `body may be at most 1048576 bytes, got 6291456` — a 1 MiB cap and the emulator's own sentence.
Why: every real-world OpenAPI import larger than 1 MiB is rejected, so importing or overwriting a non-trivial API definition is impossible; and the refusal message is not the one AWS words.
Fix: the ImportRestApi and PutRestApi handlers' body-size validation and its message.
Traces: apigateway.import-rest-api-definition-past-six-mib-rejected, apigateway.put-rest-api-definition-past-six-mib-rejected

Cause 4 of 5: PutIntegration caps a requestTemplates value at 32768 characters instead of 300 KB, and its too-large message is the emulator's own.
AWS: Smithy models PutIntegration.requestTemplates as MapOfStringToString with NO @length; the limit is API Gateway's documented mapping-template quota of 300 KB per template. A 100 KB template is stored (GetIntegration returns it byte-identical) and served by the deployed stage; one byte past 300 KB is refused 400 BadRequestException with the fixed text `The resource being saved is too large. Consider reducing the number of modeled parameters, the number of response mappings, or perhaps the size of your VTL templates if used.` (entirely fixed — no request values appear in it), and the integration keeps its previous requestTemplates.
Emulator: PutIntegration refuses a 65536-character template with `a requestTemplates value may be at most 32768 characters, got 65536`, and refuses the 307201-character one with `a requestTemplates value may be at most 32768 characters, got 307201` — the cap is ~1/9 of AWS's and the message is the emulator's own sentence naming the limit and the observed length.
Why: any non-trivial VTL mapping template (above 32 KB) cannot be saved, and a client matching AWS's too-large sentence never sees it.
Fix: the PutIntegration handler's requestTemplates validation and its message.
Traces: apigateway.request-template-twice-the-300-kib-limit-rejected, apigateway.put-integration-oversized-request-template-rejected-keeps-previous

Cause 5 of 5: CreateDeployment with canarySettings naming a stage that does not exist is refused with the wrong message.
AWS: Smithy models CreateDeployment.canarySettings (DeploymentCanarySettings) and BadRequestException @httpError 400. A canary release deployment whose stageName names a stage that does not exist yet is refused 400 BadRequestException with `Invalid deployment content specified.Stage <stageName> must already be created before making a canary release deployment` — the fixed text is `Invalid deployment content specified.Stage ` + ` must already be created before making a canary release deployment`, and the only variable part is the requested stageName. (`Invalid stage identifier specified` is AWS's message for the NotFoundException of a later GetStage on that stage, not for this refusal.) The refused call creates neither the stage nor a deployment.
Emulator: CreateDeployment answers 400 BadRequestException with `Invalid stage identifier specified` — the NotFoundException wording, with no mention of the canary rule and no stage name.
Why: a client cannot tell from the refusal that the stage must be created before a canary release; it reads as a plain missing-stage error and the obvious retry (re-specifying the stage) fails the same way.
Fix: the CreateDeployment handler's canarySettings validation branch (its message).
Traces: apigateway.create-deployment-canary-on-missing-stage-rejected

## Evidence
Cause 1 — expected ('BadRequestException', 'Access log format must be less than or equal to 3000 characters') for a 3001-character format and acceptance at 3000; got BadRequestException on UpdateStage: '/accessLogSettings/format may be at most 512 characters, got 3000' (and '... got 1510').
Cause 2 — expected a refusal of a format containing '\n'; got HTTP 200 and GetStage accessLogSettings.format == '$context.requestId $context.httpMethod\n$context.status'.
Cause 3 — expected a 6291456-byte import/put to succeed and 6291457 to be refused ('BadRequestException', 'Body length exceeds maximum import size.'); got BadRequestException on ImportRestApi and PutRestApi: 'body may be at most 1048576 bytes, got 6291456'.
Cause 4 — expected PutIntegration to store a 102400-character template and refuse 307201 with ('BadRequestException', 'The resource being saved is too large. Consider reducing the number of modeled parameters, the number of response mappings, or perhaps the size of your VTL templates if used.'); got BadRequestException 'a requestTemplates value may be at most 32768 characters, got 65536' / '... got 307201'.
Cause 5 — expected message 'Invalid deployment content specified.Stage next must already be created before making a canary release deployment'; got 'Invalid stage identifier specified'.

## Steps to Reproduce
1. CreateRestApi (REGIONAL), PutMethod + MOCK PutIntegration on the root, CreateDeployment with a stageName.
2. UpdateStage patchOperations=[{op:replace, path:/accessLogSettings/format, value:<3000-character format>}] -> observe the 512-character refusal (Cause 1); repeat with a format containing '\n' -> observe acceptance (Cause 2).
3. ImportRestApi / PutRestApi with a 6 MiB (6291456-byte) OpenAPI document -> observe the 1 MiB refusal (Cause 3).
4. PutIntegration with requestTemplates={'application/json': <102400-character template>} -> observe the 32768-character refusal (Cause 4).
5. CreateDeployment(restApiId, stageName='next', canarySettings={percentTraffic:25.0}) against an API with no 'next' stage -> observe the message (Cause 5).
