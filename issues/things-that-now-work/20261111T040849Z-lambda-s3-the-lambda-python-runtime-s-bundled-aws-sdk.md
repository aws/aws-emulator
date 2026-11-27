Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__s3.function-sdk-bucket-policy-round-trips lambda__s3.function-sdk-bucket-tagging-round-trips lambda__s3.function-sdk-bulk-delete-reports-every-key lambda__s3.function-sdk-creates-heads-lists-and-deletes-a-bucket lambda__s3.function-sdk-delete-bucket-refused-while-objects-remain lambda__s3.function-sdk-get-bucket-location-reports-the-region lambda__s3.function-sdk-get-object-attributes-reports-size-and-etag lambda__s3.function-sdk-list-objects-v1-walks-with-marker lambda__s3.function-sdk-multipart-upload-round-trip lambda__s3.function-sdk-object-tagging-round-trips lambda__s3.function-sdk-versioning-keeps-every-version lambda__s3.function-sdk-paginator-walks-every-page lambda__s3.function-sdk-resource-bucket-objects-filter-by-prefix lambda__s3.function-sdk-upload-fileobj-and-download-fileobj-round-trip lambda__s3.function-sdk-head-object-missing-key-is-a-plain-404 lambda__s3.function-sdk-last-modified-is-a-timezone-aware-datetime lambda__s3.function-handler-http-client-fetches-a-presigned-url
Issue-Key: lambda__s3.function-sdk-bucket-policy-round-trips|_invoke_granted#6|AssertionError|assert err is None, f"the function failed: {err!r} {out!r}"
Issue-Key: lambda__s3.function-sdk-bucket-tagging-round-trips|_invoke_granted#6|AssertionError|assert err is None, f"the function failed: {err!r} {out!r}"
Issue-Key: lambda__s3.function-sdk-bulk-delete-reports-every-key|_invoke_granted#9|AssertionError|assert err is None, f"the function failed: {err!r} {out!r}"
Issue-Key: lambda__s3.function-sdk-creates-heads-lists-and-deletes-a-bucket|_invoke_granted#7|AssertionError|assert err is None, f"the function failed: {err!r} {out!r}"
Issue-Key: lambda__s3.function-sdk-delete-bucket-refused-while-objects-remain|_invoke_granted#7|AssertionError|assert err is None, f"the function failed: {err!r} {out!r}"
Issue-Key: lambda__s3.function-sdk-get-bucket-location-reports-the-region|_invoke_granted#5|AssertionError|assert err is None, f"the function failed: {err!r} {out!r}"
Issue-Key: lambda__s3.function-sdk-get-object-attributes-reports-size-and-etag|_invoke_granted#8|AssertionError|assert err is None, f"the function failed: {err!r} {out!r}"
Issue-Key: lambda__s3.function-sdk-list-objects-v1-walks-with-marker|_invoke_granted#7|AssertionError|assert err is None, f"the function failed: {err!r} {out!r}"
Issue-Key: lambda__s3.function-sdk-multipart-upload-round-trip|_invoke_granted#6|AssertionError|assert err is None, f"the function failed: {err!r} {out!r}"
Issue-Key: lambda__s3.function-sdk-object-tagging-round-trips|_invoke_granted#8|AssertionError|assert err is None, f"the function failed: {err!r} {out!r}"
Issue-Key: lambda__s3.function-sdk-versioning-keeps-every-version|_invoke_granted#6|AssertionError|assert err is None, f"the function failed: {err!r} {out!r}"
Issue-Key: lambda__s3.function-sdk-paginator-walks-every-page|_invoke_granted#8|AssertionError|assert err is None, f"the function failed: {err!r} {out!r}"
Issue-Key: lambda__s3.function-sdk-resource-bucket-objects-filter-by-prefix|_invoke_granted#7|AssertionError|assert err is None, f"the function failed: {err!r} {out!r}"
Issue-Key: lambda__s3.function-sdk-upload-fileobj-and-download-fileobj-round-trip|_invoke_granted#7|AssertionError|assert err is None, f"the function failed: {err!r} {out!r}"
Issue-Key: lambda__s3.function-sdk-head-object-missing-key-is-a-plain-404|run#10|AssertionError|assert out["absent"] == {"refused": {"class": "ClientError", "code": "404",
Issue-Key: lambda__s3.function-sdk-last-modified-is-a-timezone-aware-datetime|run#8|AssertionError|assert got["type"] == "datetime", f"{where} LastModified is a {got['type']!r}"
Issue-Key: lambda__s3.function-handler-http-client-fetches-a-presigned-url|run#9|AssertionError|assert "error" not in out, f"http.client.HTTPSConnection GET failed in the function: {out!r}"
Issue-Commit: c0ced5d18fa4eb624de2042a998eca41dbf9ea10

# The Lambda Python runtime's bundled AWS SDK is a stub: most S3 operations, paginators, resources, transfers, error types and timestamp types diverge

> _Note: this issue was identified against the code base at commit `c0ced5d18fa4eb624de2042a998eca41dbf9ea10`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
Seven net-new root causes in the Python (python3.12) Lambda runtime the emulator serves to functions. Each is a property of the SDK/runtime inside the function, not of the S3 service itself.

Cause 1 of 7: the bundled SDK binds only six S3 operations; every other S3 call raises SdkCallNotMade instead of being sent.
AWS: recorded behavior only for the runtime contents — a Lambda python3.12 function's bundled boto3 binds the full S3 API, so PutBucketPolicy, PutBucketTagging, DeleteObjects, ListBuckets, DeleteBucket, GetBucketLocation, GetObjectAttributes, ListObjects, CreateMultipartUpload/UploadPart/CompleteMultipartUpload, PutObjectTagging and PutBucketVersioning all reach S3 and answer as the API models them.
Emulator: the handler faults with errorType 'SdkCallNotMade' and errorMessage 'the AWS SDK this runtime supplies binds no <Operation> call on Amazon S3; the operations it binds are GetObject, HeadObject, PutObject, CopyObject, DeleteObject, ListObjectsV2'.
Why: any function doing more than single-object I/O cannot run; the fault is reported as the caller's code failing, not as a missing runtime.
Fix: the Lambda Python runtime's bundled SDK client — bind the S3 operations the service implements rather than a six-operation allowlist.
Traces: function-sdk-bucket-policy-round-trips, function-sdk-bucket-tagging-round-trips, function-sdk-bulk-delete-reports-every-key, function-sdk-creates-heads-lists-and-deletes-a-bucket, function-sdk-delete-bucket-refused-while-objects-remain, function-sdk-get-bucket-location-reports-the-region, function-sdk-get-object-attributes-reports-size-and-etag, function-sdk-list-objects-v1-walks-with-marker, function-sdk-multipart-upload-round-trip, function-sdk-object-tagging-round-trips, function-sdk-versioning-keeps-every-version.

Cause 2 of 7: the bundled SDK has no paginator.
AWS: boto3 in the runtime supports client.get_paginator('list_objects_v2').paginate(...), walking every page.
Emulator: errorType 'SdkCallNotMade', errorMessage 'the AWS SDK this runtime supplies has no paginator; call list_objects_v2 in a loop over its own continuation member instead'.
Why: the standard listing idiom raises inside the function.
Fix: the bundled SDK client — provide get_paginator for the paginated operations.
Traces: function-sdk-paginator-walks-every-page.

Cause 3 of 7: the bundled SDK offers no S3 resource interface.
AWS: boto3.resource('s3').Bucket(name).objects.filter(Prefix=...) works in the runtime.
Emulator: errorType 'SdkCallNotMade', errorMessage "the AWS SDK this runtime supplies has resource classes for dynamodb only; build a client with boto3.client('s3') instead".
Why: resource-style code (common in handlers) cannot run.
Fix: the bundled SDK's resource layer.
Traces: function-sdk-resource-bucket-objects-filter-by-prefix.

Cause 4 of 7: the bundled SDK has no transfer methods (upload_fileobj/download_fileobj) and the stub's method signature raises a TypeError.
AWS: s3.upload_fileobj(fileobj, bucket, key) and download_fileobj round-trip the bytes.
Emulator: errorType 'TypeError', errorMessage 'Client.__getattr__.<locals>.method() takes 0 positional arguments but 3 were given' — the generic attribute shim answers for a method it does not implement.
Why: the fault is a confusing TypeError inside the SDK rather than a round-trip or a clear unsupported-operation error.
Fix: the bundled SDK client — implement the transfer helpers (or make unimplemented attributes fail as a refusal naming the call).
Traces: function-sdk-upload-fileobj-and-download-fileobj-round-trip.

Cause 5 of 7: a refused call inside the function does not raise botocore's ClientError.
AWS: HeadObject on a missing key (with ListBucket granted) raises botocore.exceptions.ClientError, so type(exc).__name__ == 'ClientError', with Error.Code '404', Error.Message 'Not Found' and HTTPStatusCode 404.
Emulator: the raised exception's class name is '404' (the code/message/status are right): the handler reports {'refused': {'class': '404', 'code': '404', 'message': 'Not Found', 'status': 404}}.
Why: handler code that catches ClientError (the documented way) does not catch the refusal at all, so a missing key crashes the function instead of being handled.
Fix: the bundled SDK's error construction — raise the SDK's ClientError type for a modelled/HTTP refusal.
Traces: function-sdk-head-object-missing-key-is-a-plain-404.

Cause 6 of 7: timestamps come back as numbers, not datetimes.
AWS: boto3 parses LastModified into a timezone-aware datetime.datetime.
Emulator: 'head LastModified is a 'int''.
Why: any handler doing date arithmetic or .isoformat() on LastModified raises.
Fix: the bundled SDK's response parser — deserialise timestamp members to timezone-aware datetimes.
Traces: function-sdk-last-modified-is-a-timezone-aware-datetime.

Cause 7 of 7: a function cannot fetch a presigned S3 URL over plain HTTP(S) from inside the runtime — the emulator's own host is refused as an address it does not serve.
AWS: an http.client.HTTPSConnection GET of a presigned GetObject URL from inside a function returns the object bytes.
Emulator: the handler gets {'error': 'gaierror', 'message': 'https://<emulator host:port>/<bucket>/<key>?AWSAccessKeyId=...&Signature=...&Expires=... addresses no service this emulator serves: sqs, events, ..., s3, ...'} — the runtime's network layer rejects the very host it is reached on.
Why: presigned-URL fetching from a function (a very common pattern) always fails, and the failure is a name-resolution error rather than an HTTP response.
Fix: the runtime's egress/name-resolution layer — route a plain HTTP(S) request to the emulator's own host/port at the address it announces.
Traces: function-handler-http-client-fetches-a-presigned-url.

Related open issues 20261105T111951Z-lambda-s3-the-lambda-python-runtime-has-no.md and 20261105T094842Z-lambda-s3-sqs-the-python-lambda-runtime-provides.md describe this runtime family but do not carry these checks' replay keys.

## Evidence
All under lambda__s3 [mismatch]:
- function-sdk-bucket-policy-round-trips: errorType 'SdkCallNotMade', 'the AWS SDK this runtime supplies binds no PutBucketPolicy call on Amazon S3; the operations it binds are GetObject, HeadObject, PutObject, CopyObject, DeleteObject, ListObjectsV2' (identical message with PutBucketTagging, DeleteObjects, ListBuckets, DeleteBucket, GetBucketLocation, GetObjectAttributes, ListObjects, CreateMultipartUpload, PutObjectTagging, PutBucketVersioning in the other ten checks)
- function-sdk-paginator-walks-every-page: 'the AWS SDK this runtime supplies has no paginator; call list_objects_v2 in a loop over its own continuation member instead'
- function-sdk-resource-bucket-objects-filter-by-prefix: "the AWS SDK this runtime supplies has resource classes for dynamodb only; build a client with boto3.client('s3') instead"
- function-sdk-upload-fileobj-and-download-fileobj-round-trip: TypeError 'Client.__getattr__.<locals>.method() takes 0 positional arguments but 3 were given'
- function-sdk-head-object-missing-key-is-a-plain-404: answered {'refused': {'class': '404', 'code': '404', 'message': 'Not Found', 'status': 404}}; expected class 'ClientError' with code '404', message 'Not Found', status 404
- function-sdk-last-modified-is-a-timezone-aware-datetime: 'head LastModified is a 'int''
- function-handler-http-client-fetches-a-presigned-url: {'error': 'gaierror', 'message': '<presigned url> addresses no service this emulator serves: ...'}

## Steps to Reproduce
1. CreateFunction with runtime python3.12 and a handler that uses boto3 for S3.
2. Call any S3 operation outside {GetObject, HeadObject, PutObject, CopyObject, DeleteObject, ListObjectsV2} — the handler faults with SdkCallNotMade.
3. Call client.get_paginator('list_objects_v2') — SdkCallNotMade; boto3.resource('s3') — SdkCallNotMade; s3.upload_fileobj(fileobj, bucket, key) — TypeError.
4. HeadObject a missing key inside the handler and print type(exc).__name__ — '404', not 'ClientError'.
5. Print type(head['LastModified']) — int, not datetime.
6. Generate a presigned GetObject URL, pass it to the handler, and fetch it with http.client — gaierror naming the emulator's own host.
