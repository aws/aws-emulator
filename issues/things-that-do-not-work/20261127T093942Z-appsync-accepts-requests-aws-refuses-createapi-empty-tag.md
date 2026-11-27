Issue-Source: trace-sweep-divergence
Issue-Trace: appsync-create-api-empty-tag-key
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: appsync-create-api-empty-tag-key appsync-function-sync-config-conflict-handler-enum-rejection appsync-update-function-max-batch-size-bounds
Issue-Shape-Id: com.amazonaws.appsync#AWSDeepdishControlPlaneService
Issue-Key: appsync|CreateApi|4xx->2xx|$.message
Issue-Key: appsync|CreateApi|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: appsync|CreateApi|4xx->2xx|status
Issue-Key: appsync|CreateFunction|4xx->2xx|$.message
Issue-Key: appsync|CreateFunction|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: appsync|CreateFunction|4xx->2xx|status
Issue-Key: appsync|ListFunctions|2xx->2xx|$.functions[*]
Issue-Key: appsync|UpdateFunction|4xx->2xx|$.message
Issue-Key: appsync|UpdateFunction|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: appsync|UpdateFunction|4xx->2xx|status
Issue-Key: appsync|GetFunction|2xx->2xx|$.functionConfiguration.dataSourceName
Issue-Key: appsync|GetFunction|2xx->2xx|$.functionConfiguration.maxBatchSize
Issue-Commit: 5d024fed191bbfdb41e9accab576b1e1cc16d383

# AppSync accepts requests AWS refuses: CreateApi empty tag key, Create/UpdateFunction syncConfig and maxBatchSize guards missing

> _Note: this issue was identified against the code base at commit `5d024fed191bbfdb41e9accab576b1e1cc16d383`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateApi

## Description
Three AppSync request guards are absent from the emulator, so calls AWS refuses with 400 BadRequestException are answered 200 and the refused state is committed; two further steps then observe the committed state. 5 causes in this issue; all five must be closed before submitting. The open issue 20261029T174359Z-appsync-control-plane-error-semantics-not-found-bad.md covers AppSync refusal CLASSES and wording for other operations, but it carries none of the replay keys below (it is a behavioral-check issue over a different set), so none of these causes is covered by it.

=== Cause 1 of 5: CreateApi accepts a zero-length tag key ===
AWS: Smithy (appsync) CreateApi — POST /v2/apis, input tags: TagMap @length 1-50 whose key TagKey is @length 1-128 @pattern ^(?!aws:)[ a-zA-Z+-=._:/]+$; modeled errors BadRequestException|400, ConcurrentModificationException|409, InternalFailureException|500, ServiceQuotaExceededException|402, UnauthorizedException|401. Recorded behavior for a request carrying a tag whose key is the empty string is 400 with the message "1 validation error detected: Value '{<key>=<value>}' at 'addedTags' failed to satisfy constraint: Map keys must satisfy constraint: [Member must have length less than or equal to 128, Member must have length greater than or equal to 1]". Fixed text: the whole frame, the member name 'addedTags' (AWS names the tag map 'addedTags' in this message, not 'tags') and the bracketed constraint list in that order; the only variable part is the rejected map entry rendered as {key=value} from the request.
Emulator: the request succeeds — 200 with an Api document and no message/x-amzn-errortype at all. The empty key is stored.
Why: a client relying on AWS rejecting an unusable tag key gets a resource created with a tag it can never address; the refusal that AWS guarantees for every tagging surface silently disappears, and tag-driven tooling diverges from the real account.
Fix: the tag-map validation on the CreateApi handler's input path (it must run before the Api is created and raise the operation's modeled BadRequestException).
Traces: appsync-create-api-empty-tag-key#task#1.

=== Cause 2 of 5: CreateFunction accepts syncConfig on a data source that does not support it ===
AWS: Smithy (appsync) CreateFunction — POST /v1/apis/{apiId}/functions, input syncConfig: SyncConfig, modeled BadRequestException|400, ConcurrentModificationException|409, InternalFailureException|500, NotFoundException|404, UnauthorizedException|401. Recorded behavior when syncConfig is supplied for a function whose dataSourceName does not name an AMAZON_DYNAMODB data source is 400 with the fixed message 'SyncConfig can only be specified for a resolver attached to a DynamoDB data source.' — entirely fixed text, no request values.
Emulator: 200 with a functionConfiguration, and the function is persisted (its syncConfig echoed back as conflictHandler/conflictDetection NONE).
Why: conflict-detection configuration is only meaningful on DynamoDB-backed resolvers; a client that creates one against another data-source type believes versioned conflict resolution is active when nothing enforces it, and the same template applied to a real account fails at create time instead.
Fix: the data-source-type cross-check in the CreateFunction handler's validation (resolve dataSourceName, then refuse syncConfig unless its type is AMAZON_DYNAMODB).
Traces: appsync-function-sync-config-conflict-handler-enum-rejection#task#4.

=== Cause 3 of 5: the function wrongly created by Cause 2 is then listed by ListFunctions ===
AWS: ListFunctions returns only functions that were actually created; because AWS refuses the create in Cause 2, the recorded response for the same api has no entry for that function (functions[0] absent).
Emulator: ListFunctions returns an extra element — the function whose create AWS refused, with its name, dataSourceName, mapping templates, syncConfig and maxBatchSize.
Why: the state divergence is observable beyond the refused call, so any client enumerating an api's pipeline functions sees a function that does not exist in AWS; this is the state consequence of Cause 2 and closes with it, but it is replayed as a separate step and must be re-run.
Fix: no separate fix beyond Cause 2 — the create must not commit; this step is its state assertion.
Traces: appsync-function-sync-config-conflict-handler-enum-rejection#task#5.

=== Cause 4 of 5: UpdateFunction accepts maxBatchSize for a data source that does not support batching ===
AWS: Smithy (appsync) UpdateFunction — POST /v1/apis/{apiId}/functions/{functionId}, input maxBatchSize: MaxBatchSize @range 0-2000 (default 0), modeled BadRequestException|400 / NotFoundException|404. Recorded behavior when a non-zero maxBatchSize is set on a function whose data source is not a batching-capable (AWS_LAMBDA) data source is 400 with the fixed message 'maxBatchSize is not supported for this data source.' — entirely fixed text, no request values.
Emulator: 200, no message and no x-amzn-errortype; the update is applied.
Why: maxBatchSize controls Lambda batch invocation; accepting it elsewhere lets a client believe batching is configured, and the request that works against the emulator is refused by AWS.
Fix: the data-source-capability check in the UpdateFunction handler's validation (and the same check on CreateFunction), gating maxBatchSize > 0 on the resolved data source's type.
Traces: appsync-update-function-max-batch-size-bounds#task#12.

=== Cause 5 of 5: GetFunction reports the state left behind by Cause 4 ===
AWS: because the update in Cause 4 is refused, the recorded GetFunction still reports the function's previous dataSourceName and its previously accepted maxBatchSize.
Emulator: GetFunction reports the dataSourceName and maxBatchSize from the refused update instead of the pre-update values.
Why: the refused mutation is durably visible, so a client that reads back after a failed update sees a configuration AWS never accepted. This is the state consequence of Cause 4 and closes with it, but it is replayed as a separate step.
Fix: no separate fix beyond Cause 4 — the refused update must leave the stored function unchanged.
Traces: appsync-update-function-max-batch-size-bounds#task#13.

## Evidence
Cause 1 — appsync-create-api-empty-tag-key#task#1 CreateApi: status expected 400, got 200; $.message expected "1 validation error detected: Value '{=empty-key}' at 'addedTags' failed to satisfy constraint: Map keys must satisfy constraint: [Member must have length less than or equal to 128, Member must have length greater than or equal to 1]", got None; $headers.x-amzn-errortype expected present, got None.
Cause 2 — appsync-function-sync-config-conflict-handler-enum-rejection#task#4 CreateFunction: status expected 400, got 200; $.message expected 'SyncConfig can only be specified for a resolver attached to a DynamoDB data source.', got None.
Cause 3 — appsync-function-sync-config-conflict-handler-enum-rejection#task#5 ListFunctions: $.functions[0] extra (expected None, got the function created by Cause 2, e.g. name sim2real_probe_fn_*, syncConfig {conflictHandler NONE, conflictDetection NONE}, maxBatchSize 0).
Cause 4 — appsync-update-function-max-batch-size-bounds#task#12 UpdateFunction: status expected 400, got 200; $.message expected 'maxBatchSize is not supported for this data source.', got None.
Cause 5 — appsync-update-function-max-batch-size-bounds#task#13 GetFunction: $.functionConfiguration.dataSourceName expected the pre-update lambda data source, got the data source named by the refused update; $.functionConfiguration.maxBatchSize expected the pre-update value, got the value from the refused update (2000).

## Steps to Reproduce
Cause 1: CreateApi(name=<valid>, eventConfig=<valid>, tags={'': 'empty-key'}) -> must be 400 BadRequestException with the 'addedTags' map-key constraint message, and no Api may be created.
Cause 2/3: CreateApi/CreateGraphqlApi, CreateDataSource(type != AMAZON_DYNAMODB), then CreateFunction(apiId, name, dataSourceName=<that data source>, syncConfig={'conflictHandler':'NONE','conflictDetection':'NONE'}) -> must be 400 BadRequestException 'SyncConfig can only be specified for a resolver attached to a DynamoDB data source.'; then ListFunctions(apiId) -> must be empty.
Cause 4/5: create a function on a non-batching data source, then UpdateFunction(apiId, functionId, dataSourceName=<non-lambda data source>, maxBatchSize=2000) -> must be 400 BadRequestException 'maxBatchSize is not supported for this data source.'; then GetFunction(apiId, functionId) -> must still report the pre-update dataSourceName and maxBatchSize.
