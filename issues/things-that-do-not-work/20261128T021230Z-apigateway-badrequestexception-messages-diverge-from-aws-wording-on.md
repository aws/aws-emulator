Issue-Source: trace-sweep-divergence
Issue-Trace: apigatewayv2-http-api-authorizer-type-outside-enum-rejected
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: apigatewayv2-http-api-authorizer-type-outside-enum-rejected apigatewayv2-websocket-authorizer-type-outside-enum-rejected apigatewayv2-http-api-integration-content-handling-strategy-outside-enum http-stage-logging-level-refused-when-legal-dropped-when-unrecognised websocket-route-unrecognised-authorization-stored-as-none-jwt-refused
Issue-Shape-Id: com.amazonaws.apigatewayv2#ApiGatewayV2
Issue-Key: apigateway|CreateAuthorizer|4xx->4xx|$.message
Issue-Key: apigateway|CreateIntegration|4xx->4xx|$.message
Issue-Key: apigateway|CreateStage|4xx->4xx|$.message
Issue-Key: apigateway|CreateRoute|4xx->4xx|$.message
Issue-Commit: c05681c6a7a1f98eaadd6408209ad7859bc7b6dc

# apigateway BadRequestException messages diverge from AWS wording on four control-plane validations

> _Note: this issue was identified against the code base at commit `c05681c6a7a1f98eaadd6408209ad7859bc7b6dc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateAuthorizer

## Description
4 causes. These four requests are correctly refused with HTTP 400, but the emulator substitutes its own sentence for the message AWS returns. The error message is part of the contract: clients and CloudFormation/CDK error handling match on these strings.

Cause 1 of 4: CreateAuthorizer with an authorizerType AWS rejects for the API's protocol.
AWS: Smithy: CreateAuthorizer.AuthorizerType -> enum AuthorizerType values [REQUEST, JWT]; modeled BadRequestException httpError 400. Recorded behavior: status 400 with the protocol-scoped message, fixed text in full — 'Invalid authorizer type. Only JWT authorizer type is supported on HTTP protocol Apis.' This message has NO variable parts; AWS names the one type the protocol supports, it does not echo the submitted value.
Emulator: status 400 (correct) but message "1 validation error detected: Value 'TOKEN' at 'authorizerType' failed to satisfy constraint: Member must satisfy enum value set: [REQUEST, JWT]" — a generic Smithy enum sentence that echoes the submitted value and names the model's enum set instead of the protocol rule.
Why: a client cannot tell from the emulator's message that the refusal is protocol-scoped (JWT-only on HTTP APIs) rather than a plain unknown-enum rejection, and any handler matching AWS's sentence does not fire.
Fix: CreateAuthorizer's validation in the apigatewayv2 operation handler — the protocol check must run before generic enum validation and raise BadRequestException with AWS's fixed sentence.
Traces: apigatewayv2-http-api-authorizer-type-outside-enum-rejected#task#2, apigatewayv2-http-api-authorizer-type-outside-enum-rejected#task#3, apigatewayv2-websocket-authorizer-type-outside-enum-rejected#task#2, apigatewayv2-websocket-authorizer-type-outside-enum-rejected#task#3.

Cause 2 of 4: CreateIntegration with contentHandlingStrategy on an HTTP-protocol API.
AWS: Smithy: CreateIntegration.ContentHandlingStrategy -> enum ContentHandlingStrategy; modeled BadRequestException httpError 400. Recorded behavior: status 400, message fixed text in full — 'Currently, a ContentHandlingStrategy is not supported for APIs with a protocol type of HTTP'. No variable parts.
Emulator: status 400 with 'Currently, ContentHandlingStrategy is not permitted for APIs with a protocol type of HTTP' — the emulator's own paraphrase (missing the article 'a', and 'not permitted' in place of 'not supported').
Why: the sentence differs from AWS's by two tokens, so an exact-match assertion or a localized client error map keyed on AWS's wording fails.
Fix: CreateIntegration's validation in the apigatewayv2 operation handler — use AWS's fixed sentence verbatim.
Traces: apigatewayv2-http-api-integration-content-handling-strategy-outside-enum#task#3.

Cause 3 of 4: CreateStage with a RouteSettings loggingLevel on an HTTP-protocol API.
AWS: Smithy: RouteSettings.LoggingLevel doc states 'Supported only for WebSocket APIs'; CreateStage modeled BadRequestException httpError 400. Recorded behavior: status 400, message fixed text in full — 'Execution logs are not supported on protocolType HTTP'. The only variable part is the protocol type name taken from the API being staged.
Emulator: status 400 with 'Currently, RouteSettings LoggingLevel is not permitted for APIs with a protocol type of HTTP' — an entirely different sentence that names the input member rather than the feature (execution logs).
Why: the message does not match AWS at all, so clients that classify stage-creation failures by message text misroute this one.
Fix: CreateStage's validation in the apigatewayv2 operation handler — emit AWS's 'Execution logs are not supported on protocolType <protocolType>' sentence.
Traces: http-stage-logging-level-refused-when-legal-dropped-when-unrecognised#task#4.

Cause 4 of 4: CreateRoute with authorizationType JWT on a WebSocket API.
AWS: Smithy: CreateRoute.AuthorizationType -> enum AuthorizationType values [NONE, AWS_IAM, CUSTOM, JWT], whose doc records that JWT is an HTTP-API value; modeled BadRequestException httpError 400. Recorded behavior: status 400, message fixed text in full — 'Currently, JWT authorization type is restricted to APIs with a protocol type of HTTP'. No variable parts.
Emulator: status 400 with 'Invalid authorizer ID specified. Setting the authorization type to JWT requires a valid JWT authorizer.' — it refuses for the wrong reason (a missing authorizer id) rather than the protocol restriction.
Why: the client is told to supply a JWT authorizer id, which on a WebSocket API can never succeed; AWS's message tells it the protocol itself forbids JWT. The emulator's diagnosis sends the caller down a dead end.
Fix: CreateRoute's validation in the apigatewayv2 operation handler — check the API's protocolType against the AuthorizationType before validating AuthorizerId, and emit AWS's fixed sentence.
Traces: websocket-route-unrecognised-authorization-stored-as-none-jwt-refused#task#4.

## Evidence
Cause 1 — $.message expected 'Invalid authorizer type. Only JWT authorizer type is supported on HTTP protocol Apis.', got "1 validation error detected: Value 'TOKEN' at 'authorizerType' failed to satisfy constraint: Member must satisfy enum value set: [REQUEST, JWT]" (status 400 both sides, 4 affected steps). Cause 2 — expected 'Currently, a ContentHandlingStrategy is not supported for APIs with a protocol type of HTTP', got 'Currently, ContentHandlingStrategy is not permitted for APIs with a protocol type of HTTP'. Cause 3 — expected 'Execution logs are not supported on protocolType HTTP', got 'Currently, RouteSettings LoggingLevel is not permitted for APIs with a protocol type of HTTP'. Cause 4 — expected 'Currently, JWT authorization type is restricted to APIs with a protocol type of HTTP', got 'Invalid authorizer ID specified. Setting the authorization type to JWT requires a valid JWT authorizer.'

## Steps to Reproduce
1. Replay apigatewayv2-http-api-authorizer-type-outside-enum-rejected step task#2 (CreateAuthorizer, authorizerType not supported for HTTP) and compare $.message.
2. Replay apigatewayv2-http-api-integration-content-handling-strategy-outside-enum step task#3 (CreateIntegration with contentHandlingStrategy on an HTTP API).
3. Replay http-stage-logging-level-refused-when-legal-dropped-when-unrecognised step task#4 (CreateStage with a legal routeSettings loggingLevel on an HTTP API).
4. Replay websocket-route-unrecognised-authorization-stored-as-none-jwt-refused step task#4 (CreateRoute with authorizationType JWT on a WebSocket API).
All four answer 400 with a message other than the recorded one.
