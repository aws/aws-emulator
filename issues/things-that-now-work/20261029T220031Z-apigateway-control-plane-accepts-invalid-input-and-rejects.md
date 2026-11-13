Issue-Source: trace-sweep-divergence
Issue-Trace: apigatewayv2-cors-configuration-validation-errors
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: apigatewayv2-cors-configuration-validation-errors apigatewayv2-http-api-route-config-cleanup apigatewayv2-jwt-authorizer-validation-errors apigatewayv2-model-template-by-schema-type apigatewayv2-portal-create-validation-error apigatewayv2-portal-invalid-product-reference apigatewayv2-portal-product-sharing-policy-roundtrip apigatewayv2-product-rest-endpoint-page-error-paths apigatewayv2-product-rest-endpoint-page-errors apigatewayv2-route-request-parameter-lifecycle apigatewayv2-vpc-link-extra-error-paths
Issue-Shape-Id: com.amazonaws.apigatewayv2#ApiGatewayV2
Issue-Key: apigateway|CreateApi|4xx->2xx|$.message
Issue-Key: apigateway|CreateApi|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: apigateway|CreateApi|4xx->2xx|status
Issue-Key: apigateway|CreateApi|2xx->4xx|$.apiEndpoint
Issue-Key: apigateway|CreateApi|2xx->4xx|$.apiId
Issue-Key: apigateway|CreateApi|2xx->4xx|$.apiKeySelectionExpression
Issue-Key: apigateway|CreateApi|2xx->4xx|$.corsConfiguration
Issue-Key: apigateway|CreateApi|2xx->4xx|$.createdDate
Issue-Key: apigateway|CreateApi|2xx->4xx|$.disableExecuteApiEndpoint
Issue-Key: apigateway|CreateApi|2xx->4xx|$.ipAddressType
Issue-Key: apigateway|CreateApi|2xx->4xx|$.name
Issue-Key: apigateway|CreateApi|2xx->4xx|$.protocolType
Issue-Key: apigateway|CreateApi|2xx->4xx|$.routeSelectionExpression
Issue-Key: apigateway|CreateApi|2xx->4xx|status
Issue-Key: apigateway|CreateRoute|2xx->4xx|status
Issue-Key: apigateway|CreateAuthorizer|4xx->2xx|$.message
Issue-Key: apigateway|CreateAuthorizer|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: apigateway|CreateAuthorizer|4xx->2xx|status
Issue-Key: apigateway|GetModelTemplate|4xx->2xx|$.message
Issue-Key: apigateway|GetModelTemplate|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: apigateway|GetModelTemplate|4xx->2xx|status
Issue-Key: apigateway|CreatePortal|4xx->2xx|$.message
Issue-Key: apigateway|CreatePortal|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: apigateway|CreatePortal|4xx->2xx|status
Issue-Key: apigateway|PutPortalProductSharingPolicy|4xx->2xx|$
Issue-Key: apigateway|PutPortalProductSharingPolicy|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: apigateway|PutPortalProductSharingPolicy|4xx->2xx|status
Issue-Key: apigateway|CreateProductRestEndpointPage|4xx->2xx|$.message
Issue-Key: apigateway|CreateProductRestEndpointPage|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: apigateway|CreateProductRestEndpointPage|4xx->2xx|status
Issue-Key: apigateway|CreateRoute|2xx->4xx|$.apiKeyRequired
Issue-Key: apigateway|CreateRoute|2xx->4xx|$.authorizationType
Issue-Key: apigateway|CreateRoute|2xx->4xx|$.requestParameters
Issue-Key: apigateway|CreateRoute|2xx->4xx|$.routeId
Issue-Key: apigateway|CreateRoute|2xx->4xx|$.routeKey
Issue-Key: apigateway|CreateRoute|2xx->4xx|$.target
Issue-Key: apigateway|CreateVpcLink|4xx->2xx|$.message
Issue-Key: apigateway|CreateVpcLink|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: apigateway|CreateVpcLink|4xx->2xx|status
Issue-Commit: 89c27cbcf326b5c1a1abaeca904759cc614d9072

# apigateway control plane accepts invalid input and rejects valid input

> _Note: this issue was identified against the code base at commit `89c27cbcf326b5c1a1abaeca904759cc614d9072`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateApi

## Description
Ten request-validation paths diverge: seven where the emulator accepts input AWS refuses (so a bad resource is created and the client gets a 2xx), and three where the emulator refuses input AWS accepts (so a valid request fails). Each cause names the required status, error type and message.

Cause 1 of 10: CreateApi with a malformed CORS allowOrigins entry.
AWS: 400 with an x-amzn-errortype error header and the message `Invalid format for origin <origin>` - fixed text `Invalid format for origin ` plus the offending origin string from corsConfiguration.allowOrigins.
Emulator: 201 - the api is created with the malformed origin.
Why: a client is told its CORS configuration is valid and only discovers at the data plane that the origin is unusable.
Fix: CreateApi's corsConfiguration validation.
Traces: apigatewayv2-cors-configuration-validation-errors#task#2.

Cause 2 of 10: CreateApi with corsConfiguration.maxAge above the modeled bound.
AWS: 201 - the api is created and the response echoes `corsConfiguration.maxAge` unchanged. Smithy: Cors.MaxAge -> IntegerWithLengthBetweenMinus1And86400 @range min -1 max 86400; the recorded service does NOT enforce that upper bound.
Emulator: 400 - a valid create is refused.
Why: a client that legitimately sets a larger maxAge cannot create its api at all; this is an over-strict validation, the mirror of cause 1.
Fix: CreateApi's corsConfiguration.maxAge check - do not reject above 86400.
Traces: apigatewayv2-cors-configuration-validation-errors#task#4.

Cause 3 of 10: CreateRoute refused during fixture setup.
AWS: 201 with the route body.
Emulator: 400 - a route create AWS accepts is refused.
Why: the whole downstream workflow (route config, stage route settings) cannot be set up.
Fix: the CreateRoute request validation - it rejects a route shape AWS accepts.
Traces: apigatewayv2-http-api-route-config-cleanup#setup#3.

Cause 4 of 10: CreateAuthorizer (JWT) with an issuer that has no OpenID discovery document.
AWS: 400 with an x-amzn-errortype error header and a message of the form `Caught exception when connecting to <issuer>/.well-known/openid-configuration for issuer <issuer>. Please try again later. Error: Invalid issuer: <issuer>. Issuer must have a valid discovery endpoint ended with '/.well-known/openid-configuration'` - the fixed text is everything but the issuer URL, which is taken from jwtConfiguration.issuer in the request.
Emulator: 201 - the authorizer is created with an unreachable issuer.
Why: an unusable JWT authorizer is reported healthy; the failure surfaces only when a request is authorized.
Fix: CreateAuthorizer's jwtConfiguration.issuer validation.
Traces: apigatewayv2-jwt-authorizer-validation-errors#task#7.

Cause 5 of 10: GetModelTemplate for a model whose schema type has no mapping template.
AWS: 400 with an x-amzn-errortype error header and the message `Unsupported data type for mapping template: <type>` - fixed text `Unsupported data type for mapping template: ` plus the JSON Schema type from the model's schema.
Emulator: 200 with a template body.
Why: the client receives a template AWS would never generate and wires it into an integration.
Fix: GetModelTemplate's schema-type gate.
Traces: apigatewayv2-model-template-by-schema-type#task#8.

Cause 6 of 10: CreatePortal referencing a portal-product ARN that is not well formed.
AWS: 400 with x-amzn-errortype `BadRequestException` and the message `Invalid ARN. ARN is not well formed <arn>` - fixed text `Invalid ARN. ARN is not well formed ` plus the rejected ARN from includedPortalProductArns.
Emulator: 201 - the portal is created referencing a product that cannot exist.
Why: a dangling product reference is accepted and the portal renders nothing.
Fix: CreatePortal's includedPortalProductArns ARN validation.
Traces: apigatewayv2-portal-create-validation-error#task#1.

Cause 7 of 10: PutPortalProductSharingPolicy with an invalid policy document.
AWS: 400 with x-amzn-errortype `BadRequestException` and the message `The specified policy was invalid` - the whole sentence is fixed text.
Emulator: 200 with an empty body - the invalid policy is stored.
Why: the client believes a sharing policy is in force when the document could never be evaluated.
Fix: PutPortalProductSharingPolicy's policy-document validation.
Traces: apigatewayv2-portal-product-sharing-policy-roundtrip#task#3.

Cause 8 of 10: CreateProductRestEndpointPage with a restEndpointIdentifier naming a non-existent stage/restApiId.
AWS: 400 with x-amzn-errortype `BadRequestException` and the message `Invalid restEndpointIdentifier stage or restApiId` - the whole sentence is fixed text.
Emulator: 201 - the page is created pointing at nothing.
Why: a portal page is published for an endpoint that does not exist.
Fix: CreateProductRestEndpointPage's restEndpointIdentifier resolution.
Traces: apigatewayv2-product-rest-endpoint-page-error-paths#task#2.

Cause 9 of 10: CreateRoute with requestParameters.
AWS: 201 with the route body echoing `requestParameters` (e.g. a `route.request.querystring.<name>` entry with `required: true`).
Emulator: 400 - a valid create is refused.
Why: route request-parameter configuration is unusable; the over-strict refusal blocks a supported feature.
Fix: CreateRoute's requestParameters validation (accept the `route.request.<location>.<name>` grammar).
Traces: apigatewayv2-route-request-parameter-lifecycle#task#3.

Cause 10 of 10: CreateVpcLink with a subnet id that does not exist.
AWS: 400 with x-amzn-errortype `BadRequestException` and a message of the form `The subnet ID '<subnetId>' does not exist (Service: AmazonEC2; Status Code: 400; Error Code: InvalidSubnetID.NotFound; Request ID: <requestId>; Proxy: null)` - the fixed text is the sentence and the bracketed Service/Status Code/Error Code/Proxy fields; the subnet id comes from the request and the request id is a per-run value.
Emulator: 201 - the VPC link is created over a non-existent subnet.
Why: an unusable VPC link settles successfully and only fails at integration time.
Fix: CreateVpcLink's subnetIds resolution against the EC2 subnet state.
Traces: apigatewayv2-vpc-link-extra-error-paths#task#1.

## Evidence
Replay verdict `mismatch`. Over-permissive clusters report `status: expected 400, got 201|200` with `$.message: missing (expected '<AWS message>', got None)` and `$headers.x-amzn-errortype: missing`. Over-strict clusters report `status: expected 201|200, got 400` with every modeled output member `missing`, e.g. CreateApi maxAge 86401 expected 201 echoing `corsConfiguration {'allowOrigins': [...], 'maxAge': 86401}`, and CreateRoute expected 201 echoing `requestParameters {'route.request.querystring.petId': {'required': True}}`.

## Steps to Reproduce
Replay the corpus. Cause 1: POST /v2/apis with corsConfiguration.allowOrigins = ["not-a-url"] (fixture apigatewayv2-cors-configuration-validation-errors step task#2) - the emulator answers 201. Cause 2: the same fixture step task#4 with maxAge 86401 - the emulator answers 400 where AWS answers 201. Cause 10: POST /v2/vpclinks with a non-existent subnet id (apigatewayv2-vpc-link-extra-error-paths#task#1) - 201 instead of 400.
