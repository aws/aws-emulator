Issue-Source: trace-sweep-divergence
Issue-Trace: function-event-invoke-config-qualifier-error-paths
Issue-Step: task#6
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: function-event-invoke-config-qualifier-error-paths lambda-concurrency-qualified-and-missing-function-errors lambda-listing-qualified-identifier-and-missing-function-errors lambda-lls-layer-reference-forms-name-arn-and-version-arn lambda-lls-s3-source-publish-mistakes-then-valid-publish lambda-x2x-xlsx-layer-attach-mistakes-and-size-ceiling list-function-url-configs-error-paths lambda-create-function-ephemeral-storage-above-maximum-rejected lambda-create-function-imageconfig-on-zip-package-rejected
Issue-Shape-Id: com.amazonaws.lambda#AWSGirApiService
Issue-Key: lambda|ListFunctionEventInvokeConfigs|4xx->4xx|$.message
Issue-Key: lambda|ListFunctionEventInvokeConfigs|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: lambda|ListFunctionEventInvokeConfigs|4xx->4xx|status
Issue-Key: lambda|GetFunctionConcurrency|4xx->2xx|$.Type
Issue-Key: lambda|GetFunctionConcurrency|4xx->2xx|$.message
Issue-Key: lambda|GetFunctionConcurrency|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: lambda|GetFunctionConcurrency|4xx->2xx|status
Issue-Key: lambda|ListAliases|4xx->4xx|$.message
Issue-Key: lambda|ListAliases|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: lambda|ListAliases|4xx->4xx|status
Issue-Key: lambda|ListVersionsByFunction|4xx->4xx|$.Type
Issue-Key: lambda|ListVersionsByFunction|4xx->4xx|$.message
Issue-Key: lambda|ListVersionsByFunction|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: lambda|ListVersionsByFunction|4xx->4xx|status
Issue-Key: lambda|GetLayerVersion|4xx->2xx|$.Content.Location
Issue-Key: lambda|PublishLayerVersion|4xx->2xx|$.Content.Location
Issue-Key: lambda|ListFunctionUrlConfigs|4xx->2xx|$.Type
Issue-Key: lambda|ListFunctionUrlConfigs|4xx->2xx|$.message
Issue-Key: lambda|ListFunctionUrlConfigs|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: lambda|ListFunctionUrlConfigs|4xx->2xx|status
Issue-Key: lambda|GetFunction|4xx->2xx|$.Code.Location
Issue-Related: 20261102T083644Z-lambda-reads-are-not-region-scoped-a-function.md
Issue-Related: 20261103T165236Z-lambda-accepts-foreign-region-and-qualified-resource-identifiers.md
Issue-Related: 20261103T165236Z-lambda-layer-publish-and-list-operations-skip-validation.md
Issue-Commit: 8d55b39ab66a28bf4265bc0ae130d6078a9d6cb6

# Lambda accepts requests AWS refuses with 400: qualified identifiers, bad markers, foreign-region layer names and oversize create params

> _Note: this issue was identified against the code base at commit `8d55b39ab66a28bf4265bc0ae130d6078a9d6cb6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListFunctionEventInvokeConfigs

## Description
Shared fault: input validation that AWS performs BEFORE it looks anything up is missing. The emulator goes straight to its state, so a request AWS refuses 400 InvalidParameterValueException is instead answered 200/201 (it succeeds) or 404 ResourceNotFoundException (it is refused for the wrong reason, with the wrong status and code). Eight net-new clusters.

AWS: in every case below the input is malformed or unsupported for that operation, so Lambda answers 400 with the modelled InvalidParameterValueException (Smithy: InvalidParameterValueException {Type, message}, httpError 400; it is a modelled error of every operation named here) and a fixed message text whose only variable parts come from the request. The emulator instead resolves the identifier.
Why (all causes): a client's error handling inverts. A 404 ResourceNotFoundException tells the caller "create it first" when the real fault is a malformed parameter it must stop sending; a 200/201 tells the caller the request was valid, so an invalid configuration is silently persisted and the caller's own validation tests pass against the emulator and fail against AWS.
Fix (all causes): each operation's request-validation stage, ahead of state lookup — reject the qualified/invalid identifier, the opaque-marker parse failure, the foreign-region layer name, the unreadable S3 content and the out-of-range EphemeralStorage with InvalidParameterValueException and AWS's message, and do not mutate state.

Cause 1 of 8 — ListFunctionEventInvokeConfigs with a version/alias in the function name.
AWS: refuses 400, message fixed text `The function name must not include a version number or alias.` (no variable parts). Smithy: ListFunctionEventInvokeConfigs input FunctionName targets NamespacedFunctionName, whose pattern admits a trailing `:qualifier`, so the operation-level rule — not the pattern — is what rejects it; errors include InvalidParameterValueException (400).
Emulator: 404 with `x-amzn-errortype: ResourceNotFoundException` and no `message` member.
Traces: function-event-invoke-config-qualifier-error-paths#task#6.

Cause 2 of 8 — GetFunctionConcurrency on a qualified function ARN.
AWS: refuses 400 with Type `User` and the fixed message `This operation is permitted on Lambda functions only. Aliases and versions do not support this operation. Please specify either a function name or an unqualified function ARN.` Smithy: GetFunctionConcurrency GET /2019-09-30/functions/{FunctionName}/concurrency, errors InvalidParameterValueException (400) / ResourceNotFoundException (404).
Emulator: 200 with the concurrency body, no error at all.
Traces: lambda-concurrency-qualified-and-missing-function-errors#task#5.

Cause 3 of 8 — ListAliases with a qualifier in the function name.
AWS: refuses 400, fixed message `Should not specify a qualifier in list alias request.`
Emulator: 404 `ResourceNotFoundException`, no message member.
Traces: lambda-listing-qualified-identifier-and-missing-function-errors#task#5.

Cause 4 of 8 — ListVersionsByFunction with a version number in the function name.
AWS: refuses 400, fixed message `Should not specify a version number in list version request.`
Emulator: 404 `ResourceNotFoundException`, and it adds a `Type: User` member the recording does not have on this path.
Traces: lambda-listing-qualified-identifier-and-missing-function-errors#task#8.

Cause 5 of 8 — GetLayerVersion with a layer ARN from another region.
AWS: refuses 400 with Type `User` and the message `Invalid Layer name: <the LayerName as sent>` — fixed prefix `Invalid Layer name: ` plus the request's own value. Smithy: GetLayerVersion LayerName pattern only admits a layer ARN in the caller's partition/region form or a bare name, so a cross-region ARN is an invalid layer name.
Emulator: 200, resolving it to the same-region layer and returning the layer version body.
Traces: lambda-lls-layer-reference-forms-name-arn-and-version-arn#task#6.

Cause 6 of 8 — PublishLayerVersion whose Content names an S3 bucket that does not exist.
AWS: fetches the object first and refuses 400 with Type `User` and the relayed message `Error occurred while GetObject. S3 Error Code: NoSuchBucket. S3 Error Message: The specified bucket does not exist` (fixed text; the S3 code and S3 message are the relayed values). No layer version is minted. Smithy: PublishLayerVersion errors InvalidParameterValueException (400), success code 201.
Emulator: 201, minting a layer version and a Content.Location for content it never read.
Traces: lambda-lls-s3-source-publish-mistakes-then-valid-publish#task#5.

Cause 7 of 8 — ListFunctionUrlConfigs with an unparseable Marker.
AWS: refuses 400 with Type `User` and the fixed message `Invalid marker value from user.` Smithy: ListFunctionUrlConfigs Marker is the @paginated inputToken; errors include InvalidParameterValueException (400).
Emulator: 200 with a normal page, silently ignoring the marker.
Traces: list-function-url-configs-error-paths#task#6.

Cause 8 of 8 — a function whose CreateFunction AWS refused is nevertheless readable.
AWS: CreateFunction with EphemeralStorage.Size above the 10240 MB maximum is refused, so no function exists and the follow-up GetFunction answers 404 ResourceNotFoundException with Type `User` and Message `Function not found: <function ARN>` (fixed prefix plus the resolved ARN).
Emulator: the create was accepted, so GetFunction answers 200 with a full Configuration and a Code.Location (itself on an AWS host the emulator does not serve). The accepted create is already filed as 20261103T165236Z-lambda-createfunction-skips-package-and-code-signing-validation.md; that issue does NOT carry this GetFunction cluster's replay key, so the state leak it leaves behind is filed here.
Traces: lambda-create-function-ephemeral-storage-above-maximum-rejected#task#5.

## Evidence
Per-cluster replay detail (expected -> actual):
1. function-event-invoke-config-qualifier-error-paths#task#6 ListFunctionEventInvokeConfigs: `status: expected 400, got 404`; `$.message: missing (expected 'The function name must not include a version number or alias.')`; `$headers.x-amzn-errortype: shape_mismatch (got 'ResourceNotFoundException')`.
2. lambda-concurrency-qualified-and-missing-function-errors#task#5 GetFunctionConcurrency: `status: expected 400, got 200`; `$.Type: missing (expected 'User')`; `$.message: missing (expected 'This operation is permitted on Lambda functions only. Aliases and versions do not support this operation. Please specify either a function name or an unqualified function ARN.')`.
3. lambda-listing-qualified-identifier-and-missing-function-errors#task#5 ListAliases: `status: expected 400, got 404`; `$.message: missing (expected 'Should not specify a qualifier in list alias request.')`; errortype `ResourceNotFoundException`.
4. lambda-listing-qualified-identifier-and-missing-function-errors#task#8 ListVersionsByFunction: `status: expected 400, got 404`; `$.message: missing (expected 'Should not specify a version number in list version request.')`; `$.Type: type_mismatch (expected None, got 'User')`.
5. lambda-lls-layer-reference-forms-name-arn-and-version-arn#task#6 GetLayerVersion: `status: expected 400, got 200`; `$.message: missing (expected 'Invalid Layer name: arn:aws:lambda:eu-north-1:000000000000:layer:<name>')`; the 200 body even carries `$.Content.Location: host_not_own`.
6. lambda-lls-s3-source-publish-mistakes-then-valid-publish#task#5 PublishLayerVersion: `status: expected 400, got 201`; `$.message: missing (expected 'Error occurred while GetObject. S3 Error Code: NoSuchBucket. S3 Error Message: The specified bucket does not exist')`.
7. list-function-url-configs-error-paths#task#6 ListFunctionUrlConfigs: `status: expected 400, got 200`; `$.message: missing (expected 'Invalid marker value from user.')`.
8. lambda-create-function-ephemeral-storage-above-maximum-rejected#task#5 GetFunction: `status: expected 404, got 200`; `$.Message: missing (expected 'Function not found: arn:aws:lambda:us-west-2:000000000000:function:<name>')`; `$.Type: missing (expected 'User')`.
Smithy: InvalidParameterValueException {Type, message} httpError 400 is a modelled error of ListFunctionEventInvokeConfigs, GetFunctionConcurrency, ListAliases, ListVersionsByFunction, GetLayerVersion, PublishLayerVersion, ListFunctionUrlConfigs and GetFunction.

## Steps to Reproduce
Replay the corpus; each cause is one step of the fixtures named in its Traces line. By hand, against the emulator:
1. ListFunctionEventInvokeConfigs with FunctionName="<fn>:1" — expect 400 InvalidParameterValueException, observe 404 ResourceNotFoundException.
2. GetFunctionConcurrency with an ARN ending in a version/alias — expect 400, observe 200.
3. ListAliases with FunctionName="<fn>:prod" — expect 400, observe 404.
4. ListVersionsByFunction with FunctionName="<fn>:1" — expect 400, observe 404.
5. GetLayerVersion with LayerName="arn:aws:lambda:eu-north-1:<acct>:layer:<layer>", VersionNumber=1 — expect 400 "Invalid Layer name: ...", observe 200.
6. PublishLayerVersion with Content.S3Bucket naming a bucket that does not exist — expect 400 relaying NoSuchBucket, observe 201 and ListLayerVersions showing the minted version.
7. ListFunctionUrlConfigs with Marker="not-a-marker" — expect 400 "Invalid marker value from user.", observe 200.
8. CreateFunction with EphemeralStorage.Size=20480, then GetFunction on that name — expect 404 Function not found, observe 200.
