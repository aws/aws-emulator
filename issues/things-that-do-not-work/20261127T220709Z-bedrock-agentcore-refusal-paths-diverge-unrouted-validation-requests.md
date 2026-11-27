Issue-Source: trace-sweep-divergence
Issue-Trace: bedrock-agentcore-agent-runtime-command-unknown-runtime
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: bedrock-agentcore-agent-runtime-command-unknown-runtime bedrock-agentcore-browser-session-get-malformed-id-validation bedrock-agentcore-code-interpreter-get-session-id-validation bedrock-agentcore-list-recommendations-query-validation-errors list-browser-sessions-request-validation bedrock-agentcore-search-registry-records-request-errors list-payment-sessions-request-validation workload-token-for-jwt-unknown-workload
Issue-Shape-Id: com.amazonaws.bedrockagentcore#AmazonBedrockAgentCore
Issue-Key: bedrock-agentcore|InvokeAgentRuntimeCommand|4xx->4xx|$.message
Issue-Key: bedrock-agentcore|InvokeAgentRuntimeCommand|4xx->4xx|$headers.content-type
Issue-Key: bedrock-agentcore|InvokeAgentRuntimeCommand|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: bedrock-agentcore|GetBrowserSession|4xx->4xx|$.message
Issue-Key: bedrock-agentcore|GetBrowserSession|4xx->4xx|$headers.content-type
Issue-Key: bedrock-agentcore|GetBrowserSession|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: bedrock-agentcore|GetBrowserSession|4xx->4xx|status
Issue-Key: bedrock-agentcore|GetCodeInterpreterSession|4xx->4xx|$.message
Issue-Key: bedrock-agentcore|GetCodeInterpreterSession|4xx->4xx|$headers.content-type
Issue-Key: bedrock-agentcore|GetCodeInterpreterSession|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: bedrock-agentcore|GetCodeInterpreterSession|4xx->4xx|status
Issue-Key: bedrock-agentcore|ListRecommendations|4xx->4xx|$.message
Issue-Key: bedrock-agentcore|ListBrowserSessions|4xx->4xx|$.message
Issue-Key: bedrock-agentcore|SearchRegistryRecords|4xx->4xx|$.message
Issue-Key: bedrock-agentcore|ListPaymentSessions|4xx->4xx|$.message
Issue-Key: bedrock-agentcore|GetWorkloadAccessTokenForJWT|4xx->4xx|$.message
Issue-Key: bedrock-agentcore|GetWorkloadAccessTokenForJWT|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: bedrock-agentcore|GetWorkloadAccessTokenForJWT|4xx->4xx|status
Issue-Commit: d1bf6e73c350197b177d1f9613fd03fd8fa1db6f

# bedrock-agentcore refusal paths diverge: unrouted validation requests, wrong auth status and reworded ValidationException messages

> _Note: this issue was identified against the code base at commit `d1bf6e73c350197b177d1f9613fd03fd8fa1db6f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
InvokeAgentRuntimeCommand

## Description
This bundle carries 8 net-new causes, all in the bedrock-agentcore data-plane REFUSAL path: three request shapes are not routed at all and come back as a generic unknown-service 404, one authorization refusal takes the wrong status and error shape, and four ValidationException messages are re-worded by the emulator. Each cause must be closed before this issue is submitted.

Cause 1 of 8: InvokeAgentRuntimeCommand is not routed.
AWS: Smithy: InvokeAgentRuntimeCommand POST /runtimes/{agentRuntimeArn}/commands, errors ValidationException(400)/ResourceNotFoundException(404)/AccessDeniedException(403)/RuntimeClientError(424). For a runtime ARN with no endpoint for the requested qualifier the recorded response is 404 with content-type application/json and the message "No endpoint or agent found with qualifier '<qualifier>' for agent '<agentRuntimeArn>'" - fixed text "No endpoint or agent found with qualifier" / "for agent", with the qualifier (DEFAULT when none is supplied) and the full agent-runtime ARN taken from the request.
Emulator: 404 with x-amzn-errortype: UnknownService, content-type application/x-amz-json-1.1 and message "no emulated service is registered for 'bedrock-agentcore'" - i.e. the URI is not bound to the service at all, so the request never reaches a handler.
Why: a client cannot distinguish "this agent runtime/qualifier does not exist" from "this whole service is absent"; the modeled ResourceNotFoundException is never raised, the error payload is in the wrong protocol framing (awsJson rather than restJson), and no command invocation is reachable.
Fix: the restJson route table for bedrock-agentcore (bind POST /runtimes/{agentRuntimeArn}/commands) plus the InvokeAgentRuntimeCommand handler's endpoint/qualifier resolution. Open issue 20261127T140224Z-bedrock-agentcore-6-more-net-new-causes-invokeagentruntimecommand.md names this operation as unrouted but carries no replay key for this fixture step, so this cause is still unexercised by any replay.
Traces: bedrock-agentcore-agent-runtime-command-unknown-runtime#task#1 (cluster also covers task#2).

Cause 2 of 8: GetBrowserSession with no sessionId is unrouted instead of refused.
AWS: Smithy: GetBrowserSession GET /browsers/{browserIdentifier}/sessions/get, sessionId is @required and bound to the httpQuery "sessionId" (pattern ^[0-9a-zA-Z]{1,40}$). With the query parameter absent AWS refuses 400 ValidationException with the message "1 validation error detected: Value at 'sessionId' failed to satisfy constraint: Member must not be null" - fixed text "1 validation error detected: Value at '" + member path + "' failed to satisfy constraint: Member must not be null", the member path being the missing input member.
Emulator: 404, x-amzn-errortype: UnknownService, content-type application/x-amz-json-1.1, message "no emulated service is registered for 'bedrock-agentcore'".
Why: a missing required query member is reported as a missing service, so SDK clients retry/fail over instead of surfacing a ValidationException the caller can correct; the response is also mis-framed as awsJson.
Fix: GetBrowserSession's route matching must not require the query member to be present, and its input validation must emit the modeled ValidationException.
Traces: bedrock-agentcore-browser-session-get-malformed-id-validation#task#3.

Cause 3 of 8: GetCodeInterpreterSession with no sessionId is unrouted instead of refused.
AWS: Smithy: GetCodeInterpreterSession GET /code-interpreters/{codeInterpreterIdentifier}/sessions/get, sessionId @required, httpQuery "sessionId". Missing member -> 400 ValidationException "1 validation error detected: Value at 'sessionId' failed to satisfy constraint: Member must not be null" (same template as cause 2).
Emulator: 404 UnknownService, content-type application/x-amz-json-1.1, message "no emulated service is registered for 'bedrock-agentcore'".
Why: same as cause 2 - the null-required-member refusal is unreachable for the code-interpreter session surface.
Fix: GetCodeInterpreterSession route matching + input validation.
Traces: bedrock-agentcore-code-interpreter-get-session-id-validation#task#6.

Cause 4 of 8: ListRecommendations enum-constraint message lists the enum set in model-declaration order.
AWS: Smithy: ListRecommendations.statusFilter targets RecommendationStatus (enum). For an unmodeled value AWS refuses 400 ValidationException with "1 validation error detected: Value at 'statusFilter' failed to satisfy constraint: Member must satisfy enum value set: [IN_PROGRESS, DELETING, COMPLETED, PENDING, FAILED]" - fixed text through "Member must satisfy enum value set: ", then the service's own enumeration order, which the recording shows is NOT the model's declaration order (recorded behavior only for the ordering).
Emulator: same status/code but the set is rendered "[PENDING, IN_PROGRESS, COMPLETED, FAILED, DELETING]", i.e. the order the enum is declared in the model.
Why: callers and test suites that assert on the refusal message (and humans diffing it) see a message that never matches AWS; the enum ordering is a fixed property of the service's message, not a per-run value.
Fix: the enum-constraint message formatter used by ListRecommendations' input validation must render the service's recorded value order for RecommendationStatus.
Traces: bedrock-agentcore-list-recommendations-query-validation-errors#task#3.

Cause 5 of 8: ListBrowserSessions enum-constraint message has the same ordering fault.
AWS: Smithy: ListBrowserSessions.status targets BrowserSessionStatus. Recorded refusal: 400 ValidationException "1 validation error detected: Value at 'status' failed to satisfy constraint: Member must satisfy enum value set: [TERMINATED, READY]".
Emulator: "...enum value set: [READY, TERMINATED]" (model declaration order).
Why: as cause 4 - the refusal text diverges for every bad status filter.
Fix: same formatter/ordering source, applied to BrowserSessionStatus in ListBrowserSessions' validation.
Traces: list-browser-sessions-request-validation#task#5.

Cause 6 of 8: SearchRegistryRecords length-constraint message injects a synthetic "Value '<n> items'" clause.
AWS: Smithy: SearchRegistryRecords.registryIds is @required with @length(min:1, max:1). For a two-element list AWS refuses 400 ValidationException "1 validation error detected: Value at 'registryIds' failed to satisfy constraint: Member must have length less than or equal to 1" - for a collection member the value is NOT echoed; fixed text is "1 validation error detected: Value at '<member>' failed to satisfy constraint: Member must have length less than or equal to <max>", with member path and the modeled max as the only variables.
Emulator: "1 validation error detected: Value '2 items' at 'registryIds' failed to satisfy constraint: Member must have length less than or equal to 1" - it renders a "Value '<count> items'" fragment AWS never emits.
Why: the message no longer matches AWS's template for collection length violations, so message-asserting clients break; the extra fragment also leaks a rendering of the request body.
Fix: the length-constraint message formatter in SearchRegistryRecords' input validation - omit the value clause for list/collection members.
Traces: bedrock-agentcore-search-registry-records-request-errors#task#2.

Cause 7 of 8: ListPaymentSessions replaces AWS's paymentManagerArn sentence with a generic regex-pattern message.
AWS: Smithy: ListPaymentSessions.paymentManagerArn is @required, @length(66,2048), with the payment-manager ARN @pattern. For a missing/ill-formed value the recorded refusal is 400 ValidationException with the message "paymentManagerArn is missing or does not match the required pattern" - a fixed sentence naming the member, with no regex and no 'N validation error(s) detected' prefix.
Emulator: 400 ValidationException whose message is the generic member-path form "1 validation error detected: Value at 'paymentManagerArn' failed to satisfy constraint: Member must satisfy regular expression pattern: arn:(aws|aws-[a-z0-9-]+):bedrock-agentcore:...", i.e. it dumps the modeled pattern.
Why: this operation uses a bespoke refusal sentence; emitting the generic constraint message (and exposing the raw regex) diverges for both the missing and the malformed case, which a client cannot match.
Fix: ListPaymentSessions' input validation must emit the operation's own fixed sentence for paymentManagerArn rather than the generic pattern-constraint formatter.
Traces: list-payment-sessions-request-validation#task#1 (cluster also covers task#2).

Cause 8 of 8: GetWorkloadAccessTokenForJWT refuses a cross-account workload identity as an expired token.
AWS: Smithy: GetWorkloadAccessTokenForJWT POST /identities/GetWorkloadAccessTokenForJWT, errors include AccessDeniedException(403) and UnauthorizedException(401). For a workload identity that is not the caller account's, AWS refuses 403 with the fixed message "Workload Identity does not belong to caller account" (no variable parts).
Emulator: 401 with x-amzn-errortype: UnauthorizedException and message "Token has expired".
Why: the status, the error shape and the message all diverge, so a client is told to refresh its JWT (a retryable, client-fixable condition) when the real condition is a permanent cross-account authorization failure - retry loops instead of a surfaced error.
Fix: GetWorkloadAccessTokenForJWT's handler - evaluate workload-identity ownership before token expiry, and raise AccessDeniedException(403) with AWS's sentence.
Traces: workload-token-for-jwt-unknown-workload#task#2 (cluster also covers task#6).

## Evidence
Cause 1 - bedrock-agentcore-agent-runtime-command-unknown-runtime#task#1 (InvokeAgentRuntimeCommand): expected 404 message "No endpoint or agent found with qualifier 'DEFAULT' for agent '<runtime ARN>'", content-type application/json; actual 404 message "no emulated service is registered for 'bedrock-agentcore'", content-type application/x-amz-json-1.1, x-amzn-errortype UnknownService.
Cause 2 - bedrock-agentcore-browser-session-get-malformed-id-validation#task#3 (GetBrowserSession): expected 400 "1 validation error detected: Value at 'sessionId' failed to satisfy constraint: Member must not be null"; actual 404 "no emulated service is registered for 'bedrock-agentcore'" (UnknownService, x-amz-json-1.1).
Cause 3 - bedrock-agentcore-code-interpreter-get-session-id-validation#task#6 (GetCodeInterpreterSession): expected 400 "1 validation error detected: Value at 'sessionId' failed to satisfy constraint: Member must not be null"; actual 404 UnknownService "no emulated service is registered for 'bedrock-agentcore'".
Cause 4 - bedrock-agentcore-list-recommendations-query-validation-errors#task#3 (ListRecommendations): expected "...enum value set: [IN_PROGRESS, DELETING, COMPLETED, PENDING, FAILED]"; actual "...enum value set: [PENDING, IN_PROGRESS, COMPLETED, FAILED, DELETING]".
Cause 5 - list-browser-sessions-request-validation#task#5 (ListBrowserSessions): expected "...enum value set: [TERMINATED, READY]"; actual "...enum value set: [READY, TERMINATED]".
Cause 6 - bedrock-agentcore-search-registry-records-request-errors#task#2 (SearchRegistryRecords): expected "1 validation error detected: Value at 'registryIds' failed to satisfy constraint: Member must have length less than or equal to 1"; actual "1 validation error detected: Value '2 items' at 'registryIds' failed to satisfy constraint: Member must have length less than or equal to 1".
Cause 7 - list-payment-sessions-request-validation#task#1 (ListPaymentSessions): expected "paymentManagerArn is missing or does not match the required pattern"; actual "1 validation error detected: Value at 'paymentManagerArn' failed to satisfy constraint: Member must satisfy regular expression pattern: arn:(aws|aws-[a-z0-9-]+):bedrock-agentcore:[a-z0-9-]+:[0-9]{12}:payment-manager/[a-z0-9]([a-z0-9-]{0,47}[a-z0-9])?-[a-z0-9]{10}".
Cause 8 - workload-token-for-jwt-unknown-workload#task#2 (GetWorkloadAccessTokenForJWT): expected 403 "Workload Identity does not belong to caller account"; actual 401 UnauthorizedException "Token has expired".

## Steps to Reproduce
Replay the simparity-wire-aws-v1 wire corpus against the emulator and inspect these steps:
1. bedrock-agentcore-agent-runtime-command-unknown-runtime, step task#1 - POST /runtimes/{agentRuntimeArn}/commands for a runtime ARN with no DEFAULT endpoint.
2. bedrock-agentcore-browser-session-get-malformed-id-validation, step task#3 - GET /browsers/{browserIdentifier}/sessions/get with the sessionId query parameter omitted.
3. bedrock-agentcore-code-interpreter-get-session-id-validation, step task#6 - GET /code-interpreters/{codeInterpreterIdentifier}/sessions/get with sessionId omitted.
4. bedrock-agentcore-list-recommendations-query-validation-errors, step task#3 - GET /recommendations?status=<not an enum value>.
5. list-browser-sessions-request-validation, step task#5 - POST /browsers/{browserIdentifier}/sessions/list with status=<not an enum value>.
6. bedrock-agentcore-search-registry-records-request-errors, step task#2 - POST /registry-records/search with two registryIds.
7. list-payment-sessions-request-validation, step task#1 - POST /payments/listPaymentSessions with a missing/ill-formed paymentManagerArn.
8. workload-token-for-jwt-unknown-workload, step task#2 - POST /identities/GetWorkloadAccessTokenForJWT for a workload identity owned by another account.
Compare each response's HTTP status, x-amzn-errortype, content-type and message against the recorded response.
