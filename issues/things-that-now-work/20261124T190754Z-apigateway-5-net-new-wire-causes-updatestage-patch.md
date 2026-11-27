Issue-Source: trace-sweep-divergence
Issue-Trace: apigw-patch-deployment-stage-invalid-path
Issue-Step: task#11
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: apigw-patch-deployment-stage-invalid-path apigateway-stage-method-settings-path-grammar apigateway-import-export-sdk-error-paths apigateway-invalid-input-error-paths apigw-envelope-api-keys-position-walk apigateway-deploy-and-export-api-definition
Issue-Shape-Id: com.amazonaws.apigateway#BackplaneControlService
Issue-Key: apigateway|UpdateStage|4xx->2xx|$.message
Issue-Key: apigateway|UpdateStage|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: apigateway|UpdateStage|4xx->2xx|status
Issue-Key: apigateway|UpdateStage|4xx->4xx|$.message
Issue-Key: apigateway|ImportRestApi|4xx->4xx|$.message
Issue-Key: apigateway|GetApiKeys|2xx->2xx|$.position
Issue-Key: apigateway|GetExport|2xx->2xx|$
Issue-Commit: ebf3a28cf5dce567e4c0f686d2c323b3753a6406

# apigateway: 5 net-new wire causes — UpdateStage patch-path validation and four response bodies diverge from API Gateway

> _Note: this issue was identified against the code base at commit `ebf3a28cf5dce567e4c0f686d2c323b3753a6406`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateStage, ImportRestApi, GetApiKeys, GetExport

## Description
5 net-new root causes in the apigateway (REST API Gateway) emulator, from the replay of wire trace set simparity-wire-aws-v1. These are the only 5 divergence clusters of this service the replay marks with neither a `stub_path` nor an `owned_by` open issue. ALL FIVE MUST BE CLOSED before this issue is submitted.

Cause 1 of 5: UpdateStage ACCEPTS a `remove` patch on a stage member API Gateway supports no `remove` for, answering 200 with the member cleared, instead of refusing the request 400 BadRequestException.
AWS: Smithy (services/service-com-amazonaws-apigateway-backplanecontrolservice/smithy-model/com-amazonaws-apigateway-backplanecontrolservice.json) models UpdateStage as PATCH /restapis/{restApiId}/stages/{stageName}, success 200 (output Stage), input member `patchOperations` (ListOfPatchOperation), errors BadRequestException (HTTP 400), ConflictException (409), LimitExceededException (429), NotFoundException (404), TooManyRequestsException (429), UnauthorizedException (401). The model does not enumerate which (op, path) pairs a stage supports, so the refusal itself is recorded behavior only: a stage takes `remove` ONLY for a per-method settings path, so a `remove` naming any other pointer of the stage is refused with HTTP 400, error code `BadRequestException` (also announced in the `x-amzn-errortype` response header) and the message "Invalid patch path  '<path>' specified for op '<op>'. Must be one of: /{resourcePath}/{httpMethod}". In that message the fixed text is "Invalid patch path ", the DOUBLE space before the quoted path, "' specified for op '", "'. Must be one of: " and the single template `/{resourcePath}/{httpMethod}`; the only variable parts are the `path` and the `op` taken verbatim from the request's patch operation. The stage must be left unchanged.
Emulator: the request succeeds — HTTP 200 with a Stage body, no `message`, no `x-amzn-errortype` header — because the stage's `remove` dispatch treats every OPTIONAL member of the Stage shape as a removable part (src/ops/update_stage.rs: `apply_stage_patch`'s remove arms; only a pointer that is no member of the shape falls through to `remove_method_setting`, lines 1144-1160). So `remove /description` (and the other optional single-valued members) is applied rather than refused, and the refusal sentence quoted above is never produced for any op.
Why: a client that sends an unsupported `remove` gets a success and believes the stage member was legitimately cleared, where AWS rejects the whole patch list and leaves the stage untouched. Infrastructure tooling and CI assertions that branch on the 400/BadRequestException (the signal that a patch grammar is wrong) see no error at all, and emulator-validated patch sets then fail against real API Gateway. The divergence is also a state divergence, not only a message one: the emulator's stage loses the member AWS keeps.
Fix: UpdateStage's patch-path/op validation in the apigateway crate — the `remove` dispatch of src/ops/update_stage.rs (`apply_stage_patch` / `remove_method_setting`) must restrict `remove` to the per-method-settings path grammar and raise BadRequestException with the "Invalid patch path  '<path>' specified for op '<op>'. Must be one of: /{resourcePath}/{httpMethod}" sentence for every other pointer, before any member of the copied record is edited.
Traces: apigw-patch-deployment-stage-invalid-path#task#11.

Cause 2 of 5: the pointer ENUMERATION in UpdateStage's invalid-method-setting-path refusal is the emulator's own path table, not the list API Gateway prints.
AWS: same operation/model as Cause 1; the message text is recorded behavior only. A `replace` naming a path that is no supported stage pointer is refused 400 BadRequestException with "Invalid method setting path: <path>. Must be one of: [<enumeration>]". The fixed text is "Invalid method setting path: ", ". Must be one of: [" and the enumeration; the one variable part is `<path>`, copied from the request. The enumeration AWS prints is, in order: /deploymentId, /description, /cacheClusterEnabled, /cacheClusterSize, /clientCertificateId, /accessLogSettings, /accessLogSettings/destinationArn, /accessLogSettings/format, /{resourcePath}/{httpMethod}/metrics/enabled, /{resourcePath}/{httpMethod}/logging/dataTrace, /{resourcePath}/{httpMethod}/logging/loglevel, /{resourcePath}/{httpMethod}/throttling/burstLimit/{resourcePath}/{httpMethod}/throttling/rateLimit/{resourcePath}/{httpMethod}/caching/ttlInSeconds, /{resourcePath}/{httpMethod}/caching/enabled, /{resourcePath}/{httpMethod}/caching/dataEncrypted, /{resourcePath}/{httpMethod}/caching/requireAuthorizationForCacheControl, /{resourcePath}/{httpMethod}/caching/unauthorizedCacheControlHeaderStrategy, /*/*/metrics/enabled, /*/*/logging/dataTrace, /*/*/logging/loglevel, /*/*/throttling/burstLimit /*/*/throttling/rateLimit /*/*/caching/ttlInSeconds, /*/*/caching/enabled, /*/*/caching/dataEncrypted, /*/*/caching/requireAuthorizationForCacheControl, /*/*/caching/unauthorizedCacheControlHeaderStrategy, /variables/{variable_name}, /tracingEnabled. Note that this is the service's own fixed text and reproduces its quirks exactly: the two `accessLogSettings` sub-pointers ARE listed, the `/*/*` wildcard family IS listed, the burstLimit/rateLimit/ttlInSeconds entries of each family run TOGETHER with no comma-space separator, and neither /documentationVersion nor any /canarySettings pointer appears.
Emulator: the sentence frame and the 400/BadRequestException are right, but the enumeration is the crate's own table STAGE_PATCH_PATHS (src/ops/update_stage.rs lines 413-437, used as `InvalidPathRefusal::MethodSettingPath(STAGE_PATCH_PATHS)` at lines 443-444): it omits /accessLogSettings/destinationArn and /accessLogSettings/format, omits the whole /*/* wildcard family, separates the throttling/caching entries with ", " where AWS runs them together, and adds /documentationVersion, /canarySettings/percentTraffic, /canarySettings/deploymentId, /canarySettings/stageVariableOverrides/{variable_name} and /canarySettings/useStageCache, which AWS's list does not carry.
Why: the enumeration is the only machine-readable statement of the supported patch grammar a client gets from this refusal. A generator, linter or operator reading it is told the `/*/*` stage-wide method settings are unsupported (they are supported) and that /canarySettings/* pointers may be patched through this grammar (AWS's own refusal does not list them), so a patch set tuned against the emulator's message is wrong against AWS in both directions.
Fix: the fixed refusal text in the apigateway crate — the STAGE_PATCH_PATHS constant in src/ops/update_stage.rs must become API Gateway's published enumeration verbatim (including its run-together segments) rather than a mirror of the handler's dispatch arms; the dispatch table and the printed list are separate things.
Traces: apigateway-stage-method-settings-path-grammar#task#13 (cluster also covers task#15 of that fixture and apigw-patch-deployment-stage-invalid-path#task#9, #task#10).

Cause 3 of 5: ImportRestApi collapses "this document declares no OAS/Swagger version" into the unparseable-body refusal, so the wrong sentence is served for a document that parsed.
AWS: Smithy models ImportRestApi as POST /restapis?mode=import, success 201, required @httpPayload `body` (Blob), error BadRequestException at HTTP 400. The message text is recorded behavior only. API Gateway reads an imported body in distinct steps and words each refusal differently: a body no reader could parse at all is refused "Invalid OAS input"; a body that PARSED but declares neither root version member (`openapi` from 3.0 on, `swagger` before it) is refused with the wholly fixed sentence "Unable to determine OAS/Swagger version" — it names no member, carries no position and no parser complaint, and has no variable part; a document whose format IS settled but which is not a valid definition of that format is refused "Invalid OpenAPI input.". The code is 400 / `BadRequestException` in every case and no RestApi is created.
Emulator: the 400 and the BadRequestException code are right, but the message is "Invalid OAS input" where AWS answers "Unable to determine OAS/Swagger version". The crate already defines both sentences — src/ops/put_rest_api.rs line 428 `UNPARSEABLE_DEFINITION = "Invalid OAS input"` and line 450 `UNDETERMINED_VERSION = "Unable to determine OAS/Swagger version"`, whose own doc comment states that version detection happens FIRST and that a body declaring neither member earns the version sentence — so the defect is the ORDER/selection in the shared reader `parse_definition` (reached from src/ops/import_rest_api.rs): the parse failure is reported before the version member is looked for. PutRestApi shares the reader and therefore serves the same wrong sentence for the same class of body.
Why: the three sentences are what let a client tell "these bytes are not a document" from "nothing here says what this is" from "this is not a valid document of the format it claims". All three share status 400 and code BadRequestException, so an importer, CLI wrapper or CI assertion that branches on the message to tell a caller which of the three to fix is misdirected to "the file is corrupt" when the real problem is a missing `openapi`/`swagger` member.
Why this is not an existing issue: open issue 20261124T102233Z-apigateway-2-net-new-causes-importrestapi-refuses-a.md and 20261122T081303Z-apigateway-importrestapi-s-rejection-of-a-malformed-openapi.md cover the "Invalid OpenAPI input." sentence for an INVALID definition; neither covers the version-undetermined class or the reader's step order, and this cluster carries no `owned_by`.
Fix: the step order of the shared definition reader in the apigateway crate — `parse_definition` in src/ops/put_rest_api.rs must select UNDETERMINED_VERSION for a body that declares neither version member (including one that is no readable document and writes neither) and reserve UNPARSEABLE_DEFINITION for a body that did not parse yet DID write one of the two members' format.
Traces: apigateway-import-export-sdk-error-paths#task#4 (cluster also covers apigateway-invalid-input-error-paths#task#4).

Cause 4 of 5: GetApiKeys publishes an emulator-minted opaque continuation token as `position`, which is not the shape API Gateway's pagination position has.
AWS: Smithy models GetApiKeys as GET /apikeys with @paginated {inputToken: position, outputToken: position, items, pageSize: limit}; `position` is documented only as "The current pagination position in the paged result set", so its shape is recorded behavior only: API Gateway's position for this collection is an API-Gateway resource identifier — the short (≈10-character) alphanumeric id of the last ApiKey on the page — and is fed back verbatim in the `position` query string to fetch the next page. Hypothesis: AWS derives the cursor from the last item's id rather than from server-side paging state, which is why the recorded token is id-shaped.
Emulator: `position` is a 64-character lowercase hex digest minted by the store's own paging helper (src/ops/get_api_keys.rs lines 188-216, `crate::state::minted_page_of(&store.page_token_key(), ...)`, reported at line 246 as `page.next_position`), deliberately opaque so that "a caller pages with it and reads no api key's identity out of it". The replayed step asserts the recorded shape of `$.position` and the digest does not satisfy it.
Why: the position is part of this listing's wire contract and clients treat it as an API Gateway id: SDK paginators, scripts that resume a walk from a stored cursor, and code that compares the position against the last item's id (or logs/validates it as an id) break or mis-validate. A 64-hex digest also leaks that paging is state-dependent on the emulator process, so a cursor minted in one run is meaningless in another, where an AWS position is just an id.
Fix: GetApiKeys' paging/serialization in the apigateway crate — the cursor minted in src/ops/get_api_keys.rs (and the shared `minted_page_of`/page-token helper it calls in src/state.rs) must publish an id-shaped position derived from the last reported key of the page, and accept that form back on input.
Traces: apigw-envelope-api-keys-position-walk#task#3.

Cause 5 of 5: the OpenAPI document GetExport returns advertises the emulator's own loopback authority and an `/_aws/execute-api/<api-id>` path prefix in `servers[0].url`, instead of the execute-api server URL template API Gateway writes.
AWS: Smithy models GetExport as GET /restapis/{restApiId}/stages/{stageName}/exports/{exportType}, success 200, output `body` (@httpPayload Blob) plus contentType/contentDisposition headers. The exported document's content is recorded behavior only: for a REST API with the default execute-api endpoint, the exported OpenAPI 3 document carries exactly one server whose `url` is the template "https://{restApiId}.execute-api.{region}.amazonaws.com/{basePath}" with a `basePath` variable whose default is the exported stage name — i.e. the api id is the FIRST DNS LABEL of an execute-api host and the stage is carried by the templated `basePath` variable, with no extra path prefix. (The host's region and the api id are per-environment values; the required thing is the URL's shape: `{api-id}` as the leading label of the execute-api host and `/{basePath}` as the only path.)
Emulator: the exported document instead carries `url: "https://<loopback-host>:<port>/_aws/execute-api/<api-id>/{basePath}"` — the api id demoted from the host's first label into a path segment behind an `/_aws/execute-api` prefix. Everything else in the exported document (info, paths, components, x-amazon-apigateway-security-policy, the `basePath` variable and its default) matches.
Why: the exported definition is consumed as a client artifact — it is fed to SDK/client generators, API-contract tests and documentation pipelines. Those emit clients whose base URL has the api id in the path and an emulator-internal `_aws` prefix, so the generated client cannot be pointed at AWS (or at any other host) by substituting host and region, and a diff of an exported definition against a checked-in AWS export fails on the servers block.
Why this is not an existing issue: open issue 20261115T090135Z-apigateway-apiendpoint-is-published-as-a-loopback-host.md reports the same wrong URL form on the `apiEndpoint` the control plane publishes; this cluster is a different site — the serializer that writes the exported OpenAPI document — and carries no `owned_by`, so it will not be exercised by that issue's replay.
Fix: GetExport's exported-document serializer in the apigateway crate (src/ops/get_export.rs and whatever endpoint-URL helper it shares with the apiEndpoint publisher): the exported `servers[0].url` must be rendered as the execute-api server template with the api id as the host's first label and `/{basePath}` as the whole path, independently of the address the emulator is reached on.
Traces: apigateway-deploy-and-export-api-definition#task#6.

## Evidence
Replay of wire trace set simparity-wire-aws-v1 against the emulator built from this checkout: 206 fixtures replayed, 2023 steps, 1999 passed (98.8%), 24 mismatched in 12 clusters, 0 blocked, 0 errored, emulator_up=true. Of the 12 clusters, 7 carry an `owned_by` open issue and are excluded; the 5 below are the net-new ones.

Cause 1 — fixture apigw-patch-deployment-stage-invalid-path, step task#11, operation UpdateStage [mismatch]
  Expected (recorded AWS): status 400; $.message = "Invalid patch path  '/description' specified for op 'remove'. Must be one of: /{resourcePath}/{httpMethod}" (double space after "path" is AWS's own fixed text); response header x-amzn-errortype present.
  Actual (emulator): status 200; $.message missing (None); x-amzn-errortype missing (None) — i.e. the remove was applied and the stage answered as a success.
  Emulator source: src/ops/update_stage.rs — remove arms of `apply_stage_patch`; `remove_method_setting` (lines 1144-1160) is only reached for a pointer that is no member of the Stage shape.

Cause 2 — fixture apigateway-stage-method-settings-path-grammar, step task#13, operation UpdateStage [mismatch] (affected_count 4: also task#15 of the same fixture and apigw-patch-deployment-stage-invalid-path task#9 and task#10)
  Both sides answer 400 BadRequestException; only $.message diverges.
  Expected (recorded AWS): "Invalid method setting path: <path>. Must be one of: [/deploymentId, /description, /cacheClusterEnabled, /cacheClusterSize, /clientCertificateId, /accessLogSettings, /accessLogSettings/destinationArn, /accessLogSettings/format, /{resourcePath}/{httpMethod}/metrics/enabled, /{resourcePath}/{httpMethod}/logging/dataTrace, /{resourcePath}/{httpMethod}/logging/loglevel, /{resourcePath}/{httpMethod}/throttling/burstLimit/{resourcePath}/{httpMethod}/throttling/rateLimit/{resourcePath}/{httpMethod}/caching/ttlInSeconds, /{resourcePath}/{httpMethod}/caching/enabled, /{resourcePath}/{httpMethod}/caching/dataEncrypted, /{resourcePath}/{httpMethod}/caching/requireAuthorizationForCacheControl, /{resourcePath}/{httpMethod}/caching/unauthorizedCacheControlHeaderStrategy, /*/*/metrics/enabled, /*/*/logging/dataTrace, /*/*/logging/loglevel, /*/*/throttling/burstLimit /*/*/throttling/rateLimit /*/*/caching/ttlInSeconds, /*/*/caching/enabled, /*/*/caching/dataEncrypted, /*/*/caching/requireAuthorizationForCacheControl, /*/*/caching/unauthorizedCacheControlHeaderStrategy, /variables/{variable_name}, /tracingEnabled]" (the <path> echoed in the replayed step was the request's own patch path).
  Actual (emulator): the same frame, but the list drops /accessLogSettings/destinationArn, /accessLogSettings/format and every /*/* entry, inserts ", " where AWS runs burstLimit/rateLimit/ttlInSeconds together, and appends /documentationVersion and the /canarySettings/{percentTraffic,deploymentId,stageVariableOverrides/{variable_name},useStageCache} pointers.
  Emulator source: STAGE_PATCH_PATHS, src/ops/update_stage.rs lines 413-437; bound as the refusal text at lines 443-444 and used at line 1417.

Cause 3 — fixture apigateway-import-export-sdk-error-paths, step task#4, operation ImportRestApi [mismatch] (affected_count 2: also apigateway-invalid-input-error-paths#task#4)
  Both sides answer 400 BadRequestException; only $.message diverges.
  Expected (recorded AWS): "Unable to determine OAS/Swagger version" (wholly fixed text, no variable part).
  Actual (emulator): "Invalid OAS input".
  Emulator source: src/ops/put_rest_api.rs line 428 (UNPARSEABLE_DEFINITION, the sentence served) and line 450 (UNDETERMINED_VERSION, the sentence AWS serves, defined but not selected); the reader is shared with ImportRestApi via src/ops/import_rest_api.rs.

Cause 4 — fixture apigw-envelope-api-keys-position-walk, step task#3, operation GetApiKeys [mismatch]
  Both sides answer 200; only $.position diverges.
  Expected (recorded AWS): a position of the recorded shape (the step asserts the shape, not a literal cursor value — an API Gateway id, not a digest).
  Actual (emulator): "b8f10bf34cb5c65e2b2864267ad3cfb76a8224b6b33688e8abc1ddf038637156" — 64 lowercase hex characters. (That exact digest is a per-run value and is not the requirement; its SHAPE is.)
  Emulator source: src/ops/get_api_keys.rs lines 188-216 (`crate::state::minted_page_of(&store.page_token_key(), &crate::state::listing_scope(...), ...)`) and line 246 (`position: page.next_position.clone()`), whose comment states the cursor is deliberately "this store's own continuation token rather than one of those ids".

Cause 5 — fixture apigateway-deploy-and-export-api-definition, step task#6, operation GetExport [mismatch]
  Both sides answer 200; the exported YAML body diverges in exactly one line, servers[0].url.
  Expected (recorded AWS): servers:\n- url: "https://<api-id>.execute-api.<region>.amazonaws.com/{basePath}" with variables.basePath.default = the exported stage name.
  Actual (emulator): servers:\n- url: "https://<loopback-host>:<port>/_aws/execute-api/<api-id>/{basePath}" (same basePath variable and default).
  Everything else in the document (openapi version, info.title/version, paths, components, x-amazon-apigateway-security-policy) matched. The specific host, port, api id and region of the run are per-environment values; the divergence is the URL's shape — api id as a path segment behind /_aws/execute-api instead of as the execute-api host's first label.

## Steps to Reproduce
Common setup: point a boto3 `apigateway` client at the emulator's data listener.

Cause 1 (UpdateStage unsupported `remove` accepted):
1. CreateRestApi; CreateResource; PutMethod GET authorizationType=NONE; PutIntegration type=MOCK; CreateDeployment; CreateStage(stageName=<stage>, description="x").
2. UpdateStage(restApiId=<api>, stageName=<stage>, patchOperations=[{"op":"remove","path":"/description"}]).
3. Observed: HTTP 200 with a Stage body and the description cleared. Expected: HTTP 400, code BadRequestException, header x-amzn-errortype set, message "Invalid patch path  '/description' specified for op 'remove'. Must be one of: /{resourcePath}/{httpMethod}", and GetStage still reporting the original description.
4. Full scenario: replay fixture apigw-patch-deployment-stage-invalid-path step task#11.

Cause 2 (invalid-method-setting-path enumeration):
1. With the stage from Cause 1, UpdateStage with patchOperations=[{"op":"replace","path":"/~1pets~1GET/throttling/rateLimit","value":"100"}] (an escaped spelling that is no supported pointer).
2. Observed: 400 BadRequestException whose "Must be one of: [...]" list omits /accessLogSettings/destinationArn, /accessLogSettings/format and the /*/* family and adds /documentationVersion and /canarySettings/*. Expected: the enumeration quoted under Evidence, verbatim, including AWS's run-together burstLimit/rateLimit/ttlInSeconds segments.
3. Full scenario: replay fixture apigateway-stage-method-settings-path-grammar steps task#13 and task#15, and apigw-patch-deployment-stage-invalid-path steps task#9 and task#10.

Cause 3 (ImportRestApi version undetermined):
1. ImportRestApi(body=b'{"info": {"title": "no-version", "version": "1.0"}, "paths": {}}') — a document that PARSES and declares neither `openapi` nor `swagger`.
2. Observed: 400 BadRequestException, message "Invalid OAS input". Expected: 400 BadRequestException, message "Unable to determine OAS/Swagger version".
3. Confirm GetRestApis gained no new id, and that a truly unparseable body (e.g. b'{"swagger": "2.0", "info": {"title": "broken" ') still gets its own sentence.
4. Repeat through PutRestApi(mode=overwrite) with the same body — it shares the reader and must change with it.
5. Full scenario: replay fixtures apigateway-import-export-sdk-error-paths step task#4 and apigateway-invalid-input-error-paths step task#4.

Cause 4 (GetApiKeys position shape):
1. CreateApiKey several times (more than one page's worth).
2. GetApiKeys(limit=1) and read `position`.
3. Observed: a 64-character lowercase hex digest. Expected: an API-Gateway-id-shaped position (the short alphanumeric id of the page's last ApiKey), which GetApiKeys(position=<that value>) continues the walk from.
4. Full scenario: replay fixture apigw-envelope-api-keys-position-walk step task#3.

Cause 5 (GetExport servers URL):
1. Create a REST API with a GET method on /pets backed by a MOCK integration, CreateDeployment, CreateStage(prod).
2. GetExport(restApiId=<api>, stageName="prod", exportType="oas30", accepts="application/yaml") and read the body's servers block.
3. Observed: url "https://<loopback-host>:<port>/_aws/execute-api/<api-id>/{basePath}". Expected: url "https://<api-id>.execute-api.<region>.amazonaws.com/{basePath}" with variables.basePath.default = "prod".
4. Full scenario: replay fixture apigateway-deploy-and-export-api-definition step task#6.
