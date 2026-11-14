Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2__iam__imagebuilder.ibp-e2e-ec2-image-builder-pipeline-lifecycle
Issue-Key: ec2__iam__imagebuilder|ListImages|NotImplemented
Issue-Commit: 7eb3f68345fb0e1e7b2dcd6e02146b3b6c63cb69

# Implement imagebuilder ListImages: the operation answers 501 NotImplemented, so no Image Builder image inventory can be listed

> _Note: this issue was identified against the code base at commit `7eb3f68345fb0e1e7b2dcd6e02146b3b6c63cb69`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListImages

## Description
One net-new root cause in the Image Builder image-inventory surface.

Cause 1 of 1: imagebuilder ListImages is an unimplemented scaffold stub and refuses every call.

AWS: Smithy (services/service-com-amazonaws-imagebuilder-imagebuilder/smithy-model/com-amazonaws-imagebuilder-imagebuilder.json) — ListImages is bound to POST /ListImages with success status 200. Input: `owner` (Ownership enum: AWSMarketplace|Amazon|Self|Shared|ThirdParty), `filters` (FilterList, 1..10 — name, osVersion, platform, type, version), `byName` (boolean, default false), `maxResults` (1..25), `nextToken` (1..65535), `includeDeprecated`. Output: `requestId`, `imageVersionList` (ImageVersionList) and `nextToken`; the operation is paginated (inputToken/outputToken nextToken, items imageVersionList, pageSize maxResults). Its modeled errors are CallRateLimitExceededException (429), ClientException (400), ForbiddenException (403), InvalidPaginationTokenException (400), InvalidRequestException (400), ServiceException (500) and ServiceUnavailableException (503) — a well-formed list request is answered 200 with the account's image semantic versions, and an empty inventory is an empty/absent `imageVersionList`, never an error.

Emulator: the handler is the generated scaffold. services/service-com-amazonaws-imagebuilder-imagebuilder/src/ops/list_images.rs still carries the `//! SCAFFOLDED, NOT IMPLEMENTED` marker (line 4) and its `handle` body ends in `Err(crate::scaffolded::Scaffolded::not_implemented("ListImages"))` (line 196), so every call — including the well-formed, unfiltered one the suite makes — is refused with HTTP 501, error code `NotImplemented` and the message "operation ListImages is scaffolded but not implemented in this emulator". `NotImplemented` is not a code this operation models at all.

Why: ListImages is the only way a client enumerates the images in an account, so it is on the normal path of every Image Builder workflow: polling a started pipeline execution for the image version it produced, resolving a recipe's latest image, and the CLI/console image list. A client gets a 501 with a non-AWS error code, which no AWS SDK retry or error-handling path recognises — the SDK surfaces it as an unmodelled service failure rather than as a throttle, a validation error or an empty result — so the whole image-build lifecycle is unreachable rather than merely degraded.

Fix: services/service-com-amazonaws-imagebuilder-imagebuilder/src/ops/list_images.rs — implement `ListImages::handle` against the service's stored images (honour `owner`, `filters`, `byName`, `includeDeprecated`, and `maxResults`/`nextToken` pagination; return `imageVersionList` plus `requestId`), and delete the SCAFFOLDED marker line at the top of the file.

Note: separate open issues already cover the imagebuilder GetImage stub (20261102T131027Z-imagebuilder-getimage-stub.md) and the ListImagePipelineImages stub (20261103T041241Z-imagebuilder-listimagepipelineimages-stub.md). Neither names ListImages, and no open issue does.

Traces: ec2__iam__imagebuilder.ibp-e2e-ec2-image-builder-pipeline-lifecycle

## Evidence
Check ec2__iam__imagebuilder.ibp-e2e-ec2-image-builder-pipeline-lifecycle [error], phase 2/6 `pre_invoke`.

Expected (AWS): imagebuilder:ListImages answers HTTP 200 with `requestId` and `imageVersionList` (possibly empty), per the modeled POST /ListImages binding with success status 200.

Actual (emulator): HTTP 501, Error.Code "NotImplemented":
  _PhaseError: [phase 2/6 pre_invoke] ClientError: An error occurred (NotImplemented) when calling the ListImages operation: operation ListImages is scaffolded but not implemented in this emulator

Handler state in the checkout:
  services/service-com-amazonaws-imagebuilder-imagebuilder/src/ops/list_images.rs:4   `//! SCAFFOLDED, NOT IMPLEMENTED: this operation answers HTTP 501 NotImplemented`
  services/service-com-amazonaws-imagebuilder-imagebuilder/src/ops/list_images.rs:196 `Err(crate::scaffolded::Scaffolded::not_implemented("ListImages"))`

The refusal happens before any Image Builder assertion in the check runs, so the whole pipeline lifecycle (CreateComponent -> CreateInfrastructureConfiguration -> CreateDistributionConfiguration -> CreateImageRecipe -> CreateImagePipeline -> StartImagePipelineExecution -> GetImage/GetImagePipeline) is never exercised.

## Steps to Reproduce
1. Call imagebuilder:ListImages with no arguments (or with owner="Self").
2. EXPECT HTTP 200 with `requestId` and an `imageVersionList` member (empty when the account owns no images). OBSERVED HTTP 501 with Error.Code "NotImplemented" and message "operation ListImages is scaffolded but not implemented in this emulator".
3. Same result with any well-formed variant: owner="Amazon", byName=true, maxResults=25, or filters=[{"name": "platform", "values": ["Linux"]}] — the stub refuses before input is considered.
4. Full path: create an IAM role + instance profile for Image Builder, then imagebuilder:CreateComponent, CreateInfrastructureConfiguration, CreateDistributionConfiguration, CreateImageRecipe, CreateImagePipeline, StartImagePipelineExecution, and poll the produced image with ListImages — the poll is refused 501 immediately.
