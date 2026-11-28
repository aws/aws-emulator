Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__s3.create-function-unknown-s3-object-storage-mode-rejected lambda__s3.publish-layer-s3-object-version-over-1024-chars-rejected lambda__s3.update-function-code-s3-bucket-trailing-dot-rejected lambda__s3.function-urlopen-put-with-altered-presigned-signature-rejected lambda__s3.lambda-vendored-boto3-puts-and-gets-s3-objects-by-default-endpoint lambda__s3.s3-object-created-failure-reaches-on-failure-bucket-and-handled-object-leaves-no-record
Issue-Key: lambda__s3.create-function-unknown-s3-object-storage-mode-rejected|run#19|AssertionError|assert got == want, f"S3ObjectStorageMode 'MOVE' gave {got!r}, expected {want!r}"
Issue-Key: lambda__s3.publish-layer-s3-object-version-over-1024-chars-rejected|run#20|AssertionError|assert edge == ("InvalidParameterValueException",
Issue-Key: lambda__s3.update-function-code-s3-bucket-trailing-dot-rejected|run#21|AssertionError|assert (code, msg) == ("ValidationException", expected), (
Issue-Key: lambda__s3.function-urlopen-put-with-altered-presigned-signature-rejected|_invoke_with_iam_retry#8|AssertionError|assert resp.get("FunctionError") is None, (
Issue-Key: lambda__s3.lambda-vendored-boto3-puts-and-gets-s3-objects-by-default-endpoint|_invoke#14|AssertionError|assert "FunctionError" not in response, (
Issue-Key: lambda__s3.s3-object-created-failure-reaches-on-failure-bucket-and-handled-object-leaves-no-record|_poll_keys#42|AssertionError|raise AssertionError(f"no object under {prefix!r} after {cap_s:.0f}s: {what}")
Issue-Commit: 35f7de95e926ecd075d4f0c5efe73729faf5e0c1

# lambda/s3: S3-sourced code errors, the enum value set, the Python runtime's missing stdlib extension modules, the vendored boto3 import, and the async on-failure destination for S3 events (5 causes)

> _Note: this issue was identified against the code base at commit `35f7de95e926ecd075d4f0c5efe73729faf5e0c1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateFunctionCode

## Description
Five net-new root causes where Lambda meets S3.

Cause 1 of 5: the S3ObjectStorageMode enum value set is printed in the wrong order in the CreateFunction refusal.
AWS: an off-enum `Code.S3ObjectStorageMode` answers 400 ValidationException `1 validation error detected: Value '<value>' at 'code.s3ObjectStorageMode' failed to satisfy constraint: Member must satisfy enum value set: [REFERENCE, COPY]` — the fixed sentence with `<value>` from the request and the service's own value ordering. Smithy: S3ObjectStorageMode is an enum with values COPY and REFERENCE.
Emulator: emits `[COPY, REFERENCE]` (the model declaration order).
Why: a client that pins or diffs the refusal message gets a different string than AWS returns.
Fix: the enum value-list rendering used by the CreateFunction validator for Code.S3ObjectStorageMode.
Traces: lambda__s3.create-function-unknown-s3-object-storage-mode-rejected.

Cause 2 of 5: an invalid S3 identifier is reported as a missing object instead of an invalid argument.
AWS: an over-long S3ObjectVersion and a bucket name with a trailing dot are refused with the S3 condition AWS actually hits — for the version id, InvalidParameterValueException `Error occurred while GetObjectVersion. S3 Error Code: InvalidArgument. S3 Error Message: Invalid version id specified`; the trailing-dot bucket is likewise refused as a malformed bucket name rather than as a bucket that does not exist.
Emulator: forwards its own S3's not-found conditions — `S3 Error Code: NoSuchVersion. S3 Error Message: The specified version does not exist.` and `S3 Error Code: NoSuchBucket. S3 Error Message: The specified bucket does not exist`.
Why: the client is told its artifact is missing when its identifier is malformed, so it re-uploads instead of correcting the reference, and CI loops on a condition that will never clear.
Fix: validate S3Bucket / S3ObjectVersion and map the S3 error class in the UpdateFunctionCode and PublishLayerVersion handlers before the object fetch.
Traces: lambda__s3.publish-layer-s3-object-version-over-1024-chars-rejected, lambda__s3.update-function-code-s3-bucket-trailing-dot-rejected.

Cause 3 of 5: the Python runtime has no `pyexpat`, so `xml.etree.ElementTree` cannot parse.
AWS: the managed python runtime ships the full standard library including the `pyexpat` extension, so a handler parsing an S3 XML error body with `ET.fromstring` works.
Emulator: the handler raises `ImportError: No module named expat; use SimpleXMLTreeBuilder instead` and the invoke answers `Unhandled`, so the behavior under test (that an altered presigned PUT is refused) is never reached.
Why: every handler that parses XML — any client reading an S3 or SQS/SNS error body with the standard library — crashes on the emulator and works on AWS.
Fix: the python runtime image/layer in the invoke sandbox must include the stdlib C extension modules (`pyexpat`, and the sibling gaps the suite reports elsewhere such as `_ctypes` and `_bz2`).
Traces: lambda__s3.function-urlopen-put-with-altered-presigned-signature-rejected.

Cause 4 of 5: a vendored boto3 in the deployment package cannot be imported because `http.client` is incomplete.
AWS: a function that bundles its own boto3 imports it successfully and reaches S3 at the default endpoint.
Emulator: the import fails with `Unable to import module 'index': cannot import name 'IncompleteRead' from 'http.client' (unknown location)` and ErrorType Runtime.ImportModuleError.
Why: the single most common packaging pattern — pinning an SDK version in the zip — does not work at all, and the error points at the runtime's own `http.client` rather than at the user's package.
Fix: the `http.client` module the python runtime sandbox provides must expose the names the stdlib defines (`IncompleteRead` among them).
Traces: lambda__s3.lambda-vendored-boto3-puts-and-gets-s3-objects-by-default-endpoint.

Cause 5 of 5: an asynchronous invocation that fails writes no record to its OnFailure destination.
AWS: when an S3 ObjectCreated notification invokes a function asynchronously and the handler fails past its retries, Lambda writes an invocation record under the configured OnFailure destination bucket at the `aws/lambda/async/<function-name>/` prefix; a successfully handled object leaves no record.
Emulator: no object ever appears under that prefix — the check waits 420s and finds none.
Why: a client that relies on the OnFailure destination as its dead-letter path for event-driven work loses every failed event with no trace, and its failure dashboards stay empty while work is being dropped.
Fix: the OnFailure destination write in the asynchronous invocation path (the event-invoke destination plumbing), for S3-notification-sourced invocations.
Traces: lambda__s3.s3-object-created-failure-reaches-on-failure-bucket-and-handled-object-leaves-no-record.

## Evidence
Cause 1: `S3ObjectStorageMode 'MOVE' gave (... enum value set: [COPY, REFERENCE]), expected (... enum value set: [REFERENCE, COPY])`.
Cause 2: `a 1024-character S3ObjectVersion naming no version gave ('InvalidParameterValueException', 'Error occurred while GetObjectVersion. S3 Error Code: NoSuchVersion. S3 Error Message: The specified version does not exist.')`; `an S3Bucket ending in a dot answered 'InvalidParameterValueException' 'Error occurred while GetObject. S3 Error Code: NoSuchBucket. S3 Error Message: The specified bucket does not exist'`.
Cause 3: `the function raised: 'Unhandled' {'errorMessage': 'No module named expat; use SimpleXMLTreeBuilder instead', 'errorType': 'ImportError', ... File "<frozen xml.etree.ElementTree>", line 1365, in XML}`.
Cause 4: `raised 'Unhandled': {'errorMessage': "Unable to import module 'index': cannot import name 'IncompleteRead' from 'http.client' (unknown location)", 'errorType': 'Runtime.ImportModuleError'}`.
Cause 5: `no object under 'aws/lambda/async/<function-name>/' after 420s: no invocation record for the failing object`.

## Steps to Reproduce
Replay the 6 check ids in trace_ids. (1) `aws lambda create-function --code S3Bucket=b,S3Key=k,S3ObjectStorageMode=MOVE` and compare the enum list. (2) `aws lambda update-function-code --s3-bucket 'name.' ...` and `aws lambda publish-layer-version --content S3ObjectVersion=<1024 chars>` and compare the S3 error code/message. (3) Deploy a python handler that calls `xml.etree.ElementTree.fromstring` on an S3 error body and invoke it. (4) Bundle boto3 into the deployment zip and invoke the handler that imports it. (5) Configure an OnFailure destination bucket on an async function wired to an S3 ObjectCreated notification, upload an object the handler fails on, and list `aws/lambda/async/<function>/`.
