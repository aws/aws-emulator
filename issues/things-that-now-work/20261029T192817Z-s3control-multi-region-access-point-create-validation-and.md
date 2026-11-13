Issue-Source: trace-sweep-divergence
Issue-Trace: s3c-multi-region-access-point-create-validation-boundary
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: s3c-multi-region-access-point-create-validation-boundary s3control-multi-region-access-point-create-accepted-asynchronously
Issue-Shape-Id: com.amazonaws.s3control#AWSS3ControlServiceV20180820
Issue-Key: s3control|CreateMultiRegionAccessPoint|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Key: s3control|CreateMultiRegionAccessPoint|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: s3control|CreateMultiRegionAccessPoint|4xx->4xx|$.ErrorResponse.HostId
Issue-Key: s3control|CreateMultiRegionAccessPoint|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: s3control|CreateMultiRegionAccessPoint|2xx->2xx|$.CreateMultiRegionAccessPointResult.RequestTokenARN
Issue-Key: s3control|DescribeMultiRegionAccessPointOperation|2xx->2xx|$.DescribeMultiRegionAccessPointOperationResult.AsyncOperation.RequestParameters.CreateMultiRegionAccessPointRequest.PublicAccessBlock
Issue-Key: s3control|DescribeMultiRegionAccessPointOperation|2xx->2xx|$.DescribeMultiRegionAccessPointOperationResult.AsyncOperation.RequestStatus
Issue-Key: s3control|DescribeMultiRegionAccessPointOperation|2xx->2xx|$.DescribeMultiRegionAccessPointOperationResult.AsyncOperation.RequestTokenARN
Issue-Key: s3control|DescribeMultiRegionAccessPointOperation|2xx->2xx|$.DescribeMultiRegionAccessPointOperationResult.AsyncOperation.ResponseDetails.ErrorDetails
Issue-Key: s3control|DescribeMultiRegionAccessPointOperation|2xx->2xx|$.DescribeMultiRegionAccessPointOperationResult.AsyncOperation.ResponseDetails.MultiRegionAccessPointDetails.Regions
Issue-Key: s3control|GetMultiRegionAccessPoint|4xx->2xx|$.ErrorResponse
Issue-Key: s3control|GetMultiRegionAccessPoint|4xx->2xx|status
Issue-Commit: 907b6e75f03592bc638357f3d5c791839011739f

# s3control Multi-Region Access Point create validation and async operation results diverge

> _Note: this issue was identified against the code base at commit `907b6e75f03592bc638357f3d5c791839011739f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateMultiRegionAccessPoint

## Description
5 net-new root causes in the s3control Multi-Region Access Point (MRAP) create path: the name refusal uses the wrong code and message, the vended async request token does not match the recorded token shape, and the async operation the create starts always reports SUCCEEDED — it never fails for a duplicate bucket or a non-existent bucket, never carries ErrorDetails, does not echo the request's PublicAccessBlock, and leaves a resource behind that AWS does not create. Smithy declares no errors for these operations (errors: []); the codes and messages are recorded behavior only.

--- Cause 1 of 5: an invalid MRAP name is refused with the wrong code and message ---
AWS: recorded behavior only for the wire text (Smithy: the MRAP name is constrained by @pattern ^[a-z0-9][-a-z0-9]{1,48}[a-z0-9]$, length 0..50). The recorded refusal is HTTP 400 with Code `InvalidRequest` and Message `The provided Amazon S3 MultiRegionAccessPoint name is invalid` — fixed text, with the offending name NOT interpolated — plus RequestId/HostId.
Emulator: answers 400 with Code `ValidationException` and Message `1 validation error detected: Value '<name>' at 'details.name' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[a-z0-9][-a-z0-9]{1,48}[a-z0-9]$`, and omits RequestId/HostId.
Why: a client branching on `InvalidRequest` does not recognize the refusal, and the message is a generic shape error rather than the service's name rule, so a name-boundary probe reports the wrong cause.
Fix: the CreateMultiRegionAccessPoint name validation's error mapping plus the shared s3control envelope serializer.
Traces: s3c-multi-region-access-point-create-validation-boundary#task#1 (4 steps in cluster).

--- Cause 2 of 5: the async RequestTokenARN does not match the recorded token shape ---
AWS: recorded behavior only — CreateMultiRegionAccessPoint returns a `RequestTokenARN` whose shape the fixture asserts (Smithy: DescribeMultiRegionAccessPointOperation.RequestTokenARN @pattern ^arn:.+$, length 1..1024); the token must be the opaque async-request identity the Describe route accepts.
Emulator: returns `arn:aws:s3:<region>:<account>:async-request/mrap/create/<32 hex chars>`, which fails the recorded shape assertion.
Why: a client that stores the token, or passes it to another API/region that expects AWS's token form, has an identifier AWS would not issue — and the shape assertion is exactly the contract an SDK's waiter relies on.
Fix: the async-request token construction in the s3control MRAP create path.
Traces: s3c-multi-region-access-point-create-validation-boundary#task#5.

--- Cause 3 of 5: a create naming the same bucket twice succeeds instead of failing ---
AWS: recorded behavior only — a create whose Regions list names the same bucket twice is accepted asynchronously and then FAILS: DescribeMultiRegionAccessPointOperation reports `RequestStatus: FAILED` with `ResponseDetails.ErrorDetails` {Code `InvalidRequest`, Resource = the duplicated bucket from the request, Message `Your request includes duplicate buckets. Select a new bucket in a different Region for your Multi-Region Access Point.`} and no per-region success list; the describe response also echoes `RequestParameters.CreateMultiRegionAccessPointRequest.PublicAccessBlock` as submitted.
Emulator: reports `RequestStatus: SUCCEEDED` with a `Regions` list repeating the duplicate region as SUCCEEDED, no ErrorDetails, and no PublicAccessBlock in RequestParameters (the fixture's poll aborts because SUCCEEDED is in its fail_on set).
Why: a client learns a configuration AWS rejects was created, so it proceeds against an MRAP that does not exist on AWS; the asynchronous failure channel (ErrorDetails) is never exercised, and the submitted PublicAccessBlock cannot be read back.
Fix: the MRAP create async-operation evaluation and the DescribeMultiRegionAccessPointOperation projection in the s3control crate — validate the Regions list, record a FAILED operation with ErrorDetails, and echo RequestParameters.
Traces: s3c-multi-region-access-point-create-validation-boundary#task#6.

--- Cause 4 of 5: a create naming a non-existent bucket succeeds instead of failing ---
AWS: recorded behavior only — the async operation ends `RequestStatus: FAILED` with `ResponseDetails.ErrorDetails` {Code `NoSuchBucket`, Resource = the bucket name from the request, Message `The specified bucket does not exist.`}, and echoes the request's PublicAccessBlock.
Emulator: reports `RequestStatus: SUCCEEDED` with a per-region SUCCEEDED entry, no ErrorDetails and no PublicAccessBlock echo.
Why: as cause 3 — bucket existence is never checked, so a client cannot distinguish a real MRAP from one whose backing bucket is missing.
Fix: same async-operation evaluation — resolve each region's bucket and fail the operation with NoSuchBucket when it is absent.
Traces: s3control-multi-region-access-point-create-accepted-asynchronously#task#3.

--- Cause 5 of 5: the MRAP from a failed create is readable afterwards ---
AWS: recorded behavior only — after the create operation fails, GetMultiRegionAccessPoint for that name answers 404 with Code `NoSuchMultiRegionAccessPoint`, Message `The specified Multi-Region Access Point does not exist.`, plus `<MultiRegionAccessPointName>` and `<AccountId>` from the request.
Emulator: answers HTTP 200 and describes the MRAP.
Why: a failed create leaves a phantom resource; a client's create-then-verify flow concludes success, and cleanup logic keyed on 404 never settles.
Fix: the MRAP state — do not publish (or roll back) the MRAP when its create operation fails.
Traces: s3control-multi-region-access-point-create-accepted-asynchronously#task#4.

## Evidence
Replay of wire corpus simparity-wire-aws-v1 against the emulator:
- CreateMultiRegionAccessPoint invalid name: `Code: expected 'InvalidRequest', got 'ValidationException'`; `Message: expected 'The provided Amazon S3 MultiRegionAccessPoint name is invalid', got "1 validation error detected: Value '<name>' at 'details.name' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[a-z0-9][-a-z0-9]{1,48}[a-z0-9]$"`; RequestId/HostId missing — s3c-multi-region-access-point-create-validation-boundary#task#1 (4 affected steps).
- CreateMultiRegionAccessPoint success: `$.CreateMultiRegionAccessPointResult.RequestTokenARN: value_mismatch (expected '<shaped-not-asserted>', got 'arn:aws:s3:us-west-2:<acct>:async-request/mrap/create/<hex>')` — same fixture task#5.
- DescribeMultiRegionAccessPointOperation (duplicate buckets): `AsyncOperation.RequestStatus: value_mismatch (expected 'FAILED', got 'SUCCEEDED')` and `poll: 'SUCCEEDED' is in fail_on after 2 attempt(s)`; `ResponseDetails.ErrorDetails: missing (expected {'Code': 'InvalidRequest', 'Resource': '<bucket>', 'Message': 'Your request includes duplicate buckets. Select a new bucket in a different Region for your Multi-Region Access Point.'})`; `RequestParameters.CreateMultiRegionAccessPointRequest.PublicAccessBlock: missing`; `ResponseDetails.MultiRegionAccessPointDetails.Regions: type_mismatch (expected '', got two SUCCEEDED entries)` — same fixture task#6.
- DescribeMultiRegionAccessPointOperation (absent bucket): `RequestStatus: expected 'FAILED', got 'SUCCEEDED'`; `ErrorDetails: missing (expected {'Code': 'NoSuchBucket', 'Resource': '<bucket>', 'Message': 'The specified bucket does not exist.'})`; PublicAccessBlock missing; Regions carries a SUCCEEDED entry — s3control-multi-region-access-point-create-accepted-asynchronously#task#3.
- GetMultiRegionAccessPoint after the failed create: `status: expected 404, got 200`; expected `{'Error': {'Code': 'NoSuchMultiRegionAccessPoint', 'Message': 'The specified Multi-Region Access Point does not exist.', 'MultiRegionAccessPointName': '<name>', 'AccountId': '<acct>'}, ...}` — same fixture task#4.

## Steps to Reproduce
1. Replay s3c-multi-region-access-point-create-validation-boundary#task#1: POST /v20180820/async-requests/mrap/create with a name containing an upper-case character — the emulator answers ValidationException instead of InvalidRequest / `The provided Amazon S3 MultiRegionAccessPoint name is invalid`.
2. Same fixture task#5/#task#6: create an MRAP whose Regions list names one bucket twice, then poll DescribeMultiRegionAccessPointOperation with the returned RequestTokenARN — the token fails the recorded shape and the operation reports SUCCEEDED with no ErrorDetails and no PublicAccessBlock echo, where the recording reports FAILED with the duplicate-buckets ErrorDetails.
3. Replay s3control-multi-region-access-point-create-accepted-asynchronously#task#3: create an MRAP naming a bucket that does not exist, then describe the operation — SUCCEEDED instead of FAILED with ErrorDetails {NoSuchBucket, 'The specified bucket does not exist.'}.
4. Same fixture task#4: GET /v20180820/mrap/instances/<that name> — the emulator answers 200 instead of 404 NoSuchMultiRegionAccessPoint.
