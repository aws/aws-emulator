Issue-Source: trace-sweep-divergence
Issue-Trace: apigateway-stage-method-settings-duplicate-spelling
Issue-Step: task#8
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: apigateway-stage-method-settings-duplicate-spelling apigateway-stage-method-settings-path-grammar apigw-patch-deployment-stage-invalid-path apigateway-deploy-and-export-api-definition apigateway-import-configure-deploy-export-sdk apigateway-import-rest-api-deploy-export-sdk apigateway-sdk-type-catalog-and-generation apigw-patch-usageplan-usage-invalid-path
Issue-Shape-Id: com.amazonaws.apigateway#BackplaneControlService
Issue-Key: apigateway|UpdateStage|4xx->4xx|$.message
Issue-Key: apigateway|GetExport|2xx->2xx|$
Issue-Key: apigateway|GetSdk|4xx->4xx|$.message
Issue-Key: apigateway|GetSdk|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: apigateway|GetSdk|4xx->4xx|status
Issue-Key: apigateway|UpdateUsage|4xx->4xx|$.message
Issue-Commit: fa605e3beb5560d80c09dff54081711248679782

# API Gateway REST responses use emulator-authored wording, validation order and export server URL (5 causes)

> _Note: this issue was identified against the code base at commit `fa605e3beb5560d80c09dff54081711248679782`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateStage

## Description
Five net-new root causes, all apigateway, all the same fault: the emulator answers with wording, a refusal precedence, or a document body it authored itself instead of the one AWS's BackplaneControlService produces. Every one of them is an emulator defect, not a per-run value difference. Each cause must be closed on its own.

=== Cause 1 of 5: UpdateStage rejects a method-setting patch path whose resource path carries unescaped slashes, instead of applying AWS's duplicate-method-settings refusal ===
AWS: Smithy (com.amazonaws.apigateway#BackplaneControlService): UpdateStage = PATCH /restapis/{restApiId}/stages/{stageName}, input patchOperations: ListOfPatchOperation, output Stage {methodSettings: MapOfMethodSettings, ...}, errors include BadRequestException (errorKind client, httpError 400). A method-setting patch path is '/{resourcePath}/{httpMethod}/<setting>', and AWS accepts the resource path spelled with its slashes left unescaped as well as escaped as '~1' — both spellings are legal grammar and denote the same methodSettings key '<resourcePath>/<httpMethod>'. When one request contains two patch operations that denote the SAME resource path + HTTP method under different spellings, AWS refuses the whole update with 400 BadRequestException whose message is: the fixed text "Update Stage failed due to different method settings with the same resource path and http method. Please remove one of the duplicate method settings for method paths: ", then the duplicate method paths exactly as the request spelled them, comma-separated, then the fixed text " for accountId: ", the caller's account id, the fixed text " and restApiId: ", then the restApiId. The only variable parts are the request's own method-path spellings and the account id / restApiId taken from the caller and the request. (Precedence and sentence: recorded behavior; the 400/BadRequestException classification is modeled.)
Emulator: the request is refused 400 with a message the emulator authored about its own path grammar: "Invalid method setting path: //pets/GET/throttling/burstLimit. Must be one of: [/deploymentId, /description, ... /tracingEnabled]" — i.e. it treats the unescaped-slash spelling as ungrammatical and never reaches the duplicate-settings check, so the duplicate-method-settings sentence is never produced.
Why: a client that submits a contradictory method-settings patch is told its patch PATH is unsupported rather than that it contains duplicate method settings. It will retry with a different escaping (which AWS also refuses, for the real reason) and can never discover the actual conflict; tooling that matches on AWS's duplicate-settings sentence to surface a template error sees an unrelated grammar complaint instead. The emulator's enumerated 'Must be one of' list is also not text AWS ever emits, so a client keying on it is coupled to the emulator.
Fix: UpdateStage's patch-path validation — accept '/{resourcePath}/{httpMethod}/<setting>' with the resource path's slashes unescaped as well as '~1'-escaped, then run the duplicate-method-settings guard and emit AWS's sentence above. Note the open issue 20261107T204751Z-apigateway-api-gateway-stage-method-settings-duplicate-resource-path.md covers the case where that same request is ACCEPTED 200; it does not cover this cluster, where a 400 is returned with the wrong message, so this cause is net-new.
Traces: apigateway-stage-method-settings-duplicate-spelling#task#8 (also apigateway-stage-method-settings-path-grammar#task#13 and #task#15, apigw-patch-deployment-stage-invalid-path#task#9 and #task#10).

=== Cause 2 of 5: GetSdk's missing-required-parameter refusal uses emulator wording instead of AWS's 'Required SDK properties are missing' sentence ===
AWS: Smithy: GetSdk = GET /restapis/{restApiId}/stages/{stageName}/sdks/{sdkType}, input parameters: MapOfStringToString (httpQueryParams), errors include BadRequestException (httpError 400). When the requested sdkType needs properties the request did not supply, AWS refuses with 400 BadRequestException whose message is the fixed text "Required SDK properties are missing: [", then the missing property names as supplied by the SDK type's requirement set, comma-separated, then "]". The only variable part is the list of missing property names; the sdkType is NOT named in the message.
Emulator: it answers 400 with its own sentence, "A parameter named service.name is required for sdkType of ruby" — one property per sentence, phrased around the sdkType and never using AWS's 'Required SDK properties are missing: [...]' form.
Why: the message is the only machine-readable description of WHICH properties are missing. A client or CI step that parses AWS's bracketed list to prompt for the missing properties extracts nothing from the emulator's sentence, and a client that must report several missing properties at once gets only one.
Fix: GetSdk's parameter validation and its error serializer — collect every missing required SDK property and render AWS's bracketed-list sentence.
Traces: apigateway-import-configure-deploy-export-sdk#task#13 (also apigateway-import-rest-api-deploy-export-sdk#task#6).

=== Cause 3 of 5: GetSdk validates SDK parameters before the stage exists, returning 400 BadRequestException where AWS returns 404 NotFoundException ===
AWS: Smithy: GetSdk binds restApiId, stageName and sdkType as httpLabels and models NotFoundException (errorKind client, httpError 404) alongside BadRequestException (400). AWS resolves the path's resources first: when the named stage does not exist the call fails 404 NotFoundException with the message "Invalid stage identifier specified" (fixed text; no variable part), regardless of whether the request's SDK parameters would also have been rejected. Resource existence precedes request-parameter validation.
Emulator: the call is answered 400 with x-amzn-errortype BadRequestException and the parameter message "A parameter named service.name is required for sdkType of ruby" — the stage is never looked up, so a request naming a nonexistent stage is reported as a malformed request.
Why: 404 vs 400 is the difference a client uses to decide whether to retry with a fixed request or to create/deploy the missing stage. An SDK's error-type switch (NotFoundException vs BadRequestException) takes the wrong branch, so provisioning code that treats NotFoundException as 'stage not deployed yet, wait and retry' instead aborts as a caller error.
Fix: GetSdk's handler — resolve restApiId and stageName against state and raise 404 NotFoundException 'Invalid stage identifier specified' before validating the sdkType parameter set.
Traces: apigateway-sdk-type-catalog-and-generation#task#8.

=== Cause 4 of 5: UpdateUsage's invalid-patch-path refusal appends an emulator-authored supported-path list instead of AWS's closing sentence ===
AWS: Smithy: UpdateUsage = PATCH /usageplans/{usagePlanId}/keys/{keyId}/usage, input patchOperations: ListOfPatchOperation, errors include BadRequestException (httpError 400). A patch operation naming a path the resource does not support is refused 400 BadRequestException with the message: the fixed text "Invalid patch path  '" (note the two spaces AWS emits after 'path'), the offending path exactly as the request spelled it, the fixed text "' specified for op '", the op exactly as the request spelled it, then the fixed text "'. Please choose supported operations". The only variable parts are the path and the op, both taken from the request; AWS does not enumerate the supported paths.
Emulator: it emits the same prefix but closes with its own enumeration — "' specified for op 'add'. Must be one of: [/remaining]" — replacing AWS's "Please choose supported operations" tail.
Why: the tail is fixed text, so clients and test assertions match on it. A client matching AWS's sentence does not recognise the emulator's refusal, and one that learns to parse the emulator's bracketed list is coupled to text AWS never sends and will break against the real service.
Fix: UpdateUsage's patch-path validation error serializer — emit AWS's fixed tail and drop the supported-path enumeration (this is the same tail defect as the UpdateStage/other patch-path validators, so fix it where the shared patch-path error is rendered).
Traces: apigw-patch-usageplan-usage-invalid-path#task#13.

=== Cause 5 of 5: GetExport emits a loopback, path-prefixed servers[].url in the exported OpenAPI document instead of the execute-api endpoint form ===
AWS: Smithy: GetExport = GET /restapis/{restApiId}/stages/{stageName}/exports/{exportType}, output body: Blob (httpPayload) with contentType. For exportType 'oas30' the document's single servers entry is the API's invoke endpoint rendered as a template: url "https://<restApiId>.execute-api.<region>.amazonaws.com/{basePath}" with variables.basePath.default set to the exported stage name. The restApiId, region and stage name are values from the request and state; the host grammar '<restApiId>.execute-api.<region>.amazonaws.com' and the '/{basePath}' template suffix are fixed structure. (Recorded behavior for the exact rendering; the model supplies only that the body is the export document.)
Emulator: the exported document instead carries url "https://127.0.0.1:<port>/_aws/execute-api/<restApiId>/{basePath}" — a loopback host with the restApiId moved into an emulator-private '/_aws/execute-api/' path prefix. The divergence is the URL's STRUCTURE, not merely which host or port appears: AWS encodes the API id in the hostname and uses no path prefix.
Why: the exported OpenAPI/Swagger document is an artifact clients hand to codegen, API portals and import tooling. A generated client built from the emulator's export targets '/_aws/execute-api/<id>/...', a path that does not exist on AWS, so the artifact is not portable and any consumer that parses servers[].url to recover the API id or region finds neither. Round-tripping the export back through ImportRestApi also carries the wrong basePath shape.
Fix: GetExport's export-document serializer — render servers[].url from the restApiId, the region and the stage using the execute-api host form, independently of the address the emulator is being reached on.
Traces: apigateway-deploy-and-export-api-definition#task#6.

## Evidence
Replay of trace set simparity-wire-aws-v1 (200 fixtures, 1943 steps, 86 mismatched) against the emulator built from this checkout. All five clusters report stub_path="" and owned_by=[] — no unimplemented operation blocks them and no open issue carries their replay key.

Cause 1 — fixture apigateway-stage-method-settings-duplicate-spelling, step task#8, UpdateStage, expected 400 / actual 400:
  $.message: value_mismatch
    expected: 'Update Stage failed due to different method settings with the same resource path and http method. Please remove one of the duplicate method settings for method paths: ~1pets/GET, /pets/GET for accountId: <accountId> and restApiId: <restApiId>'
    actual:   'Invalid method setting path: //pets/GET/throttling/burstLimit. Must be one of: [/deploymentId, /description, /cacheClusterEnabled, /cacheClusterSize, /clientCertificateId, /accessLogSettings, /{resourcePath}/{httpMethod}/metrics/enabled, ... /tracingEnabled]'
  Fixed text of the expected message: everything except the two method-path spellings (verbatim from the request's patchOperations), the accountId (caller) and the restApiId (request/state). The recorded '~1pets/GET, /pets/GET', account id and rest api id are this run's fill-ins, not the requirement.
  Also affected: apigateway-stage-method-settings-path-grammar#task#13, #task#15; apigw-patch-deployment-stage-invalid-path#task#9, #task#10.

Cause 2 — fixture apigateway-import-configure-deploy-export-sdk, step task#13, GetSdk, expected 400 / actual 400:
  $.message: value_mismatch (expected 'Required SDK properties are missing: [service.name]', got 'A parameter named service.name is required for sdkType of ruby')
  Fixed text: "Required SDK properties are missing: [" ... "]"; variable part: the missing property names ('service.name' here).
  Also affected: apigateway-import-rest-api-deploy-export-sdk#task#6.

Cause 3 — fixture apigateway-sdk-type-catalog-and-generation, step task#8, GetSdk, expected 404 / actual 400:
  status: expected 404, got 400
  $.message: value_mismatch (expected 'Invalid stage identifier specified', got 'A parameter named service.name is required for sdkType of ruby')
  $headers.x-amzn-errortype: shape_mismatch (expected NotFoundException-shaped, got 'BadRequestException')
  'Invalid stage identifier specified' is entirely fixed text.

Cause 4 — fixture apigw-patch-usageplan-usage-invalid-path, step task#13, UpdateUsage, expected 400 / actual 400:
  $.message: value_mismatch
    expected: "Invalid patch path  '/sim2realNoSuchField' specified for op 'add'. Please choose supported operations"
    actual:   "Invalid patch path  '/sim2realNoSuchField' specified for op 'add'. Must be one of: [/remaining]"
  Fixed text: "Invalid patch path  '", "' specified for op '", "'. Please choose supported operations". Variable parts: the path and the op, both from the request.

Cause 5 — fixture apigateway-deploy-and-export-api-definition, step task#6, GetExport, expected 200 / actual 200, body value_mismatch confined to servers[0].url:
    expected: 'https://<restApiId>.execute-api.<region>.amazonaws.com/{basePath}'   (recorded: https://jho0tn34ri.execute-api.us-west-2.amazonaws.com/{basePath})
    actual:   'https://127.0.0.1:<port>/_aws/execute-api/<restApiId>/{basePath}'    (recorded: https://127.0.0.1:37417/_aws/execute-api/jho0tn34ri/{basePath})
  Every other line of the document (openapi version, info.title, variables.basePath.default, paths, components, x-amazon-apigateway-security-policy) matched. The requirement is the host/path grammar, not the recorded id, region or port.

## Steps to Reproduce
0. Build and boot the emulator from this checkout; replay trace set simparity-wire-aws-v1.

Cause 1: CreateRestApi, create resource /pets with a GET method, CreateDeployment with a stage. Call UpdateStage with two patch operations for the same resource path + verb spelled differently (one '~1pets~1GET/...', one '/pets/GET/...'). The emulator answers 400 'Invalid method setting path: //pets/GET/throttling/burstLimit. Must be one of: [...]'; AWS answers 400 with the duplicate-method-settings sentence naming both spellings, the accountId and the restApiId. Replay apigateway-stage-method-settings-duplicate-spelling#task#8.

Cause 2: Import/create a REST API, deploy a stage, then GET /restapis/{id}/stages/{stage}/sdks/ruby WITHOUT the required service.name parameter. The emulator answers 400 'A parameter named service.name is required for sdkType of ruby'; AWS answers 400 'Required SDK properties are missing: [service.name]'. Replay apigateway-import-configure-deploy-export-sdk#task#13.

Cause 3: GET /restapis/{existingId}/stages/{nonexistentStage}/sdks/ruby with no parameters. The emulator answers 400 BadRequestException with the SDK-parameter message; AWS answers 404 NotFoundException 'Invalid stage identifier specified'. Replay apigateway-sdk-type-catalog-and-generation#task#8.

Cause 4: Create a usage plan and an API key, associate them, then PATCH /usageplans/{id}/keys/{keyId}/usage with patchOperations [{op: add, path: /sim2realNoSuchField, value: 1}]. The emulator closes the message with "Must be one of: [/remaining]"; AWS closes it with "Please choose supported operations". Replay apigw-patch-usageplan-usage-invalid-path#task#13.

Cause 5: Create a REST API with a GET /pets mock method, CreateDeployment to stage 'prod', then GET /restapis/{id}/stages/prod/exports/oas30. Inspect servers[0].url in the returned document: the emulator emits a loopback host with an '/_aws/execute-api/{restApiId}' path prefix; AWS emits 'https://{restApiId}.execute-api.{region}.amazonaws.com/{basePath}'. Replay apigateway-deploy-and-export-api-definition#task#6.
