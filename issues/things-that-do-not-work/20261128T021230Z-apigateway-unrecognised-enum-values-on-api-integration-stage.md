Issue-Source: trace-sweep-divergence
Issue-Trace: apigatewayv2-create-api-stores-ip-address-type-outside-enum-as-ipv4
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: apigatewayv2-create-api-stores-ip-address-type-outside-enum-as-ipv4 apigatewayv2-create-integration-passthrough-behavior-outside-enum apigatewayv2-websocket-integration-unrecognised-connection-type-stored-as-internet apigatewayv2-create-stage-route-logging-level-outside-enum apigatewayv2-websocket-stage-unrecognised-logging-level-stored-as-off http-stage-logging-level-refused-when-legal-dropped-when-unrecognised apigatewayv2-http-api-route-unrecognised-authorization-type-stored-as-none websocket-route-unrecognised-authorization-stored-as-none-jwt-refused apigatewayv2-http-api-update-integration-type-outside-enum apigatewayv2-update-integration-type-outside-enum
Issue-Shape-Id: com.amazonaws.apigatewayv2#ApiGatewayV2
Issue-Key: apigateway|CreateApi|2xx->4xx|$.apiEndpoint
Issue-Key: apigateway|CreateApi|2xx->4xx|$.apiId
Issue-Key: apigateway|CreateApi|2xx->4xx|$.apiKeySelectionExpression
Issue-Key: apigateway|CreateApi|2xx->4xx|$.createdDate
Issue-Key: apigateway|CreateApi|2xx->4xx|$.disableExecuteApiEndpoint
Issue-Key: apigateway|CreateApi|2xx->4xx|$.ipAddressType
Issue-Key: apigateway|CreateApi|2xx->4xx|$.name
Issue-Key: apigateway|CreateApi|2xx->4xx|$.protocolType
Issue-Key: apigateway|CreateApi|2xx->4xx|$.routeSelectionExpression
Issue-Key: apigateway|CreateApi|2xx->4xx|$.tags
Issue-Key: apigateway|CreateApi|2xx->4xx|status
Issue-Key: apigateway|UpdateApi|2xx->4xx|$.apiEndpoint
Issue-Key: apigateway|UpdateApi|2xx->4xx|$.apiId
Issue-Key: apigateway|UpdateApi|2xx->4xx|$.apiKeySelectionExpression
Issue-Key: apigateway|UpdateApi|2xx->4xx|$.createdDate
Issue-Key: apigateway|UpdateApi|2xx->4xx|$.disableExecuteApiEndpoint
Issue-Key: apigateway|UpdateApi|2xx->4xx|$.ipAddressType
Issue-Key: apigateway|UpdateApi|2xx->4xx|$.name
Issue-Key: apigateway|UpdateApi|2xx->4xx|$.protocolType
Issue-Key: apigateway|UpdateApi|2xx->4xx|$.routeSelectionExpression
Issue-Key: apigateway|UpdateApi|2xx->4xx|$.tags
Issue-Key: apigateway|UpdateApi|2xx->4xx|status
Issue-Key: apigateway|CreateIntegration|2xx->4xx|$.connectionType
Issue-Key: apigateway|CreateIntegration|2xx->4xx|$.integrationId
Issue-Key: apigateway|CreateIntegration|2xx->4xx|$.integrationResponseSelectionExpression
Issue-Key: apigateway|CreateIntegration|2xx->4xx|$.integrationType
Issue-Key: apigateway|CreateIntegration|2xx->4xx|$.passthroughBehavior
Issue-Key: apigateway|CreateIntegration|2xx->4xx|$.payloadFormatVersion
Issue-Key: apigateway|CreateIntegration|2xx->4xx|$.timeoutInMillis
Issue-Key: apigateway|CreateIntegration|2xx->4xx|status
Issue-Key: apigateway|GetIntegrations|2xx->2xx|$.items[*]
Issue-Key: apigateway|CreateStage|2xx->4xx|$.createdDate
Issue-Key: apigateway|CreateStage|2xx->4xx|$.defaultRouteSettings
Issue-Key: apigateway|CreateStage|2xx->4xx|$.lastUpdatedDate
Issue-Key: apigateway|CreateStage|2xx->4xx|$.routeSettings
Issue-Key: apigateway|CreateStage|2xx->4xx|$.stageName
Issue-Key: apigateway|CreateStage|2xx->4xx|$.stageVariables
Issue-Key: apigateway|CreateStage|2xx->4xx|$.tags
Issue-Key: apigateway|CreateStage|2xx->4xx|status
Issue-Key: apigateway|GetStage|2xx->4xx|$.createdDate
Issue-Key: apigateway|GetStage|2xx->4xx|$.defaultRouteSettings
Issue-Key: apigateway|GetStage|2xx->4xx|$.lastUpdatedDate
Issue-Key: apigateway|GetStage|2xx->4xx|$.routeSettings
Issue-Key: apigateway|GetStage|2xx->4xx|$.stageName
Issue-Key: apigateway|GetStage|2xx->4xx|$.stageVariables
Issue-Key: apigateway|GetStage|2xx->4xx|$.tags
Issue-Key: apigateway|GetStage|2xx->4xx|status
Issue-Key: apigateway|CreateRoute|2xx->4xx|$.apiKeyRequired
Issue-Key: apigateway|CreateRoute|2xx->4xx|$.authorizationType
Issue-Key: apigateway|CreateRoute|2xx->4xx|$.routeId
Issue-Key: apigateway|CreateRoute|2xx->4xx|$.routeKey
Issue-Key: apigateway|CreateRoute|2xx->4xx|status
Issue-Key: apigateway|UpdateIntegration|2xx->4xx|$.connectionType
Issue-Key: apigateway|UpdateIntegration|2xx->4xx|$.integrationId
Issue-Key: apigateway|UpdateIntegration|2xx->4xx|$.integrationMethod
Issue-Key: apigateway|UpdateIntegration|2xx->4xx|$.integrationType
Issue-Key: apigateway|UpdateIntegration|2xx->4xx|$.integrationUri
Issue-Key: apigateway|UpdateIntegration|2xx->4xx|$.payloadFormatVersion
Issue-Key: apigateway|UpdateIntegration|2xx->4xx|$.timeoutInMillis
Issue-Key: apigateway|UpdateIntegration|2xx->4xx|status
Issue-Key: apigateway|UpdateIntegration|2xx->4xx|$.integrationResponseSelectionExpression
Issue-Key: apigateway|UpdateIntegration|2xx->4xx|$.passthroughBehavior
Issue-Commit: c05681c6a7a1f98eaadd6408209ad7859bc7b6dc

# apigateway unrecognised enum values on Api, Integration, Stage and Route are rejected 400 instead of coerced to the service default

> _Note: this issue was identified against the code base at commit `c05681c6a7a1f98eaadd6408209ad7859bc7b6dc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateApi

## Description
9 causes. The ApiGateway V2 control plane in this emulator applies strict Smithy enum validation to members whose recorded AWS behaviour is to ACCEPT an out-of-enum string and silently coerce it to the member's service default. Every one of these requests is answered 400 BadRequestException with a generic '1 validation error detected: ... failed to satisfy constraint: Member must satisfy enum value set: [...]' sentence, so the resource is never created or updated and every downstream read of it fails too.

Cause 1 of 9: CreateApi.IpAddressType outside the enum.
AWS: Smithy: CreateApi.IpAddressType -> enum IpAddressType values [ipv4, dualstack]; CreateApi http {POST /v2/apis, code 201}; modeled errors BadRequestException(400)/ConflictException(409)/NotFoundException(404)/TooManyRequestsException(429). Recorded behavior only for the coercion: a CreateApi carrying an ipAddressType string that is not a member of IpAddressType is answered 201 with the API created and ipAddressType reported as the default 'ipv4', plus apiId, name, protocolType, routeSelectionExpression, apiKeySelectionExpression, apiEndpoint, createdDate, tags and disableExecuteApiEndpoint=false.
Emulator: answers 400 and emits no body members at all (apiId/name/protocolType/ipAddressType/routeSelectionExpression/apiKeySelectionExpression/apiEndpoint/createdDate/tags/disableExecuteApiEndpoint all absent).
Why: a client that forwards a newer or mis-cased IP address type (an SDK built against a later model, or a CloudFormation template) cannot create an API at all, where against AWS it gets a working ipv4 API.
Fix: CreateApi's input validation in the apigatewayv2 operation handler — IpAddressType must not be wire-validated against the enum; unknown values fall back to the ipv4 default before the Api is stored.
Traces: apigatewayv2-create-api-stores-ip-address-type-outside-enum-as-ipv4#task#1.

Cause 2 of 9: UpdateApi.IpAddressType outside the enum.
AWS: Smithy: UpdateApi.IpAddressType -> enum IpAddressType; UpdateApi http {PATCH /v2/apis/{ApiId}}. Recorded behavior only: the update is accepted and the stored ipAddressType is reported back (the recording shows 'dualstack' persisted and echoed with the full Api view).
Emulator: answers 400 with an empty body; none of apiId/name/protocolType/ipAddressType/apiEndpoint/createdDate/tags is returned.
Why: the IP address type of an existing API can never be changed through this emulator once an unrecognised spelling is involved, and the caller sees a validation refusal where AWS reports success.
Fix: UpdateApi's input validation in the apigatewayv2 operation handler — same enum-coercion rule as CreateApi.
Traces: apigatewayv2-create-api-stores-ip-address-type-outside-enum-as-ipv4#task#4.

Cause 3 of 9: CreateIntegration.PassthroughBehavior / ConnectionType outside the enum.
AWS: Smithy: CreateIntegration.PassthroughBehavior -> enum PassthroughBehavior values [WHEN_NO_MATCH, NEVER, WHEN_NO_TEMPLATES]; CreateIntegration.ConnectionType -> enum ConnectionType values [INTERNET, VPC_LINK]; http {POST /v2/apis/{ApiId}/integrations, code 201}. Recorded behavior only: an unrecognised passthroughBehavior (and an unrecognised connectionType) is accepted and coerced to the default, and the 201 reports integrationId, integrationType MOCK, connectionType INTERNET, passthroughBehavior WHEN_NO_MATCH, timeoutInMillis 29000, integrationResponseSelectionExpression '${integration.response.statuscode}' and payloadFormatVersion '1.0'.
Emulator: answers 400 with no body; the integration is not created.
Why: no integration can be created when any enum-typed member carries a value the emulator's model does not list, so the whole WebSocket/HTTP integration setup aborts.
Fix: CreateIntegration's input validation in the apigatewayv2 operation handler — coerce unknown PassthroughBehavior to WHEN_NO_MATCH and unknown ConnectionType to INTERNET rather than refusing.
Traces: apigatewayv2-create-integration-passthrough-behavior-outside-enum#task#2, apigatewayv2-websocket-integration-unrecognised-connection-type-stored-as-internet#task#2.

Cause 4 of 9: GetIntegrations reports the wrong coerced PassthroughBehavior and emits empty collection members AWS omits.
AWS: Smithy: GetIntegrations item members passthroughBehavior/requestParameters/requestTemplates/responseParameters. Recorded behavior only: the stored integration reads back with passthroughBehavior WHEN_NO_MATCH (the default an unrecognised value coerces to) and WITHOUT requestParameters, requestTemplates or responseParameters, because those maps were never set.
Emulator: reports passthroughBehavior 'NEVER' and additionally serializes 'requestParameters': {}, 'requestTemplates': {} and 'responseParameters': {} as empty maps.
Why: a client reading back its integration sees a different passthrough mode than it will get on AWS, and code that tests member presence to decide whether a mapping was configured mis-detects three unset maps as configured-but-empty.
Fix: the integration state's default for PassthroughBehavior, and the GetIntegrations serializer, which must omit unset maps rather than emitting {}.
Traces: apigatewayv2-create-integration-passthrough-behavior-outside-enum#task#4.

Cause 5 of 9: CreateStage RouteSettings/DefaultRouteSettings LoggingLevel outside the enum.
AWS: Smithy: RouteSettings.LoggingLevel -> enum LoggingLevel values [ERROR, INFO, OFF]; CreateStage http {POST /v2/apis/{ApiId}/stages, code 201}. Recorded behavior only: an unrecognised loggingLevel is accepted and dropped to the default, and the 201 reports stageName, defaultRouteSettings {detailedMetricsEnabled:false, loggingLevel:'OFF', dataTraceEnabled:false}, routeSettings, stageVariables, createdDate, lastUpdatedDate and tags.
Emulator: answers 400 with no body; the stage is not created.
Why: a stage cannot be created at all when a logging level is spelled in a form the emulator does not recognise, so no deployment or invoke can follow.
Fix: CreateStage's input validation in the apigatewayv2 operation handler — coerce an unknown LoggingLevel to OFF instead of refusing the stage.
Traces: apigatewayv2-create-stage-route-logging-level-outside-enum#task#4, apigatewayv2-websocket-stage-unrecognised-logging-level-stored-as-off#task#2, http-stage-logging-level-refused-when-legal-dropped-when-unrecognised#task#2.

Cause 6 of 9: GetStage on the stage cause 5 refused answers 404 instead of 200.
AWS: Smithy: GetStage http {GET /v2/apis/{ApiId}/stages/{StageName}}, modeled NotFoundException(404). Recorded behavior: because AWS created the stage, the read is 200 with stageName, defaultRouteSettings (loggingLevel 'OFF'), routeSettings, stageVariables, createdDate, lastUpdatedDate and tags.
Emulator: answers 404 and returns none of those members, because cause 5 never stored the stage.
Why: the read-back leg of every stage lifecycle that uses a non-canonical logging level reports the stage as nonexistent.
Fix: follows from cause 5 (CreateStage validation); no separate GetStage change expected, but this step is the observable confirmation.
Traces: apigatewayv2-create-stage-route-logging-level-outside-enum#task#5, apigatewayv2-websocket-stage-unrecognised-logging-level-stored-as-off#task#3, http-stage-logging-level-refused-when-legal-dropped-when-unrecognised#task#3.

Cause 7 of 9: CreateRoute.AuthorizationType outside the enum.
AWS: Smithy: CreateRoute.AuthorizationType -> enum AuthorizationType values [NONE, AWS_IAM, CUSTOM, JWT]; CreateRoute http {POST /v2/apis/{ApiId}/routes, code 201}. Recorded behavior only: an unrecognised authorizationType is accepted and stored as NONE, and the 201 reports routeId, routeKey, authorizationType 'NONE' and apiKeyRequired false.
Emulator: answers 400 with no body; the route is not created.
Why: routes carrying an authorization type the emulator does not list cannot be created, so the API has no routes and nothing can be invoked.
Fix: CreateRoute's input validation in the apigatewayv2 operation handler — coerce an unknown AuthorizationType to NONE.
Traces: apigatewayv2-http-api-route-unrecognised-authorization-type-stored-as-none#task#2, websocket-route-unrecognised-authorization-stored-as-none-jwt-refused#task#2.

Cause 8 of 9: UpdateIntegration.IntegrationType outside the enum on an HTTP API.
AWS: Smithy: UpdateIntegration.IntegrationType -> enum IntegrationType values [AWS, HTTP, MOCK, HTTP_PROXY, AWS_PROXY]; UpdateIntegration http {PATCH /v2/apis/{ApiId}/integrations/{IntegrationId}, code 200}. Recorded behavior only: the unrecognised value is ignored, the existing integrationType is retained, and the 200 reports integrationId, integrationType 'HTTP_PROXY', integrationMethod, integrationUri, connectionType INTERNET, timeoutInMillis and payloadFormatVersion.
Emulator: answers 400 with no body; the integration is left unchanged and the caller sees a refusal.
Why: any PATCH that carries a stale or newer integration type string fails outright instead of being a no-op on that member, so unrelated members in the same patch (uri, method, timeout) are never applied.
Fix: UpdateIntegration's input validation in the apigatewayv2 operation handler — an unknown IntegrationType must not fail the patch.
Traces: apigatewayv2-http-api-update-integration-type-outside-enum#task#3, apigatewayv2-http-api-update-integration-type-outside-enum#task#5.

Cause 9 of 9: UpdateIntegration.IntegrationType outside the enum on a WebSocket MOCK integration.
AWS: same Smithy facts as cause 8. Recorded behavior only: the 200 retains integrationType 'MOCK' and reports connectionType INTERNET, passthroughBehavior WHEN_NO_MATCH, timeoutInMillis, integrationResponseSelectionExpression '${integration.response.statuscode}' and payloadFormatVersion '1.0'.
Emulator: answers 400 with no body.
Why: same as cause 8, on the WebSocket protocol path, confirming the refusal is protocol-independent.
Fix: UpdateIntegration's input validation in the apigatewayv2 operation handler.
Traces: apigatewayv2-update-integration-type-outside-enum#task#3.

## Evidence
Cause 1 — expected 201 with $.ipAddressType='ipv4'; actual 400, all response members absent. Cause 2 — expected 201 with $.ipAddressType='dualstack' and the full Api view; actual 400, empty body. Cause 3 — expected 201 with $.integrationType='MOCK', $.connectionType='INTERNET', $.passthroughBehavior='WHEN_NO_MATCH', $.timeoutInMillis=29000, $.payloadFormatVersion='1.0'; actual 400, empty body. Cause 4 — expected items[0].passthroughBehavior='WHEN_NO_MATCH' and no requestParameters/requestTemplates/responseParameters; actual passthroughBehavior='NEVER' plus requestParameters={}, requestTemplates={}, responseParameters={}. Cause 5 — expected 201 with $.defaultRouteSettings={detailedMetricsEnabled:false, loggingLevel:'OFF', dataTraceEnabled:false}; actual 400, empty body. Cause 6 — expected 200 with the stage view; actual 404. Cause 7 — expected 201 with $.authorizationType='NONE', $.apiKeyRequired=false; actual 400, empty body. Cause 8 — expected 200 with $.integrationType='HTTP_PROXY'; actual 400, empty body. Cause 9 — expected 200 with $.integrationType='MOCK', $.passthroughBehavior='WHEN_NO_MATCH'; actual 400, empty body. In every case the emulator's refusal body is the generic Smithy sentence "1 validation error detected: Value '<v>' at '<member>' failed to satisfy constraint: Member must satisfy enum value set: [...]".

## Steps to Reproduce
1. Replay fixture apigatewayv2-create-api-stores-ip-address-type-outside-enum-as-ipv4 step task#1 (CreateApi with an ipAddressType outside [ipv4, dualstack]): expect 201 with ipAddressType 'ipv4'.
2. Same fixture step task#4 (UpdateApi with the same member).
3. Replay apigatewayv2-create-integration-passthrough-behavior-outside-enum steps task#2 and task#4.
4. Replay apigatewayv2-create-stage-route-logging-level-outside-enum steps task#4 and task#5.
5. Replay apigatewayv2-http-api-route-unrecognised-authorization-type-stored-as-none step task#2.
6. Replay apigatewayv2-http-api-update-integration-type-outside-enum step task#3 and apigatewayv2-update-integration-type-outside-enum step task#3.
Each returns 400 with the generic enum-constraint sentence instead of the recorded 2xx.
