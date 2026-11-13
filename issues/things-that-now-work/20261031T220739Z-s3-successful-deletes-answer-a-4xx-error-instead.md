Issue-Source: trace-sweep-divergence
Issue-Trace: s3-multipart-upload-lifecycle
Issue-Step: task#13
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: s3-multipart-upload-lifecycle s3w-object-lock-governance-delete s3w-object-lock-legal-hold-delete s3w-object-validation-error-envelopes
Issue-Shape-Id: com.amazonaws.s3#AmazonS3
Issue-Key: s3|AbortMultipartUpload|2xx->4xx|status
Issue-Key: s3|DeleteObject|2xx->4xx|status
Issue-Key: s3|DeleteBucket|2xx->4xx|status
Issue-Commit: d7a2f366149cfae4c7c153e56172220f0cdfc550

# S3 successful deletes answer a 4xx error instead of 204 No Content

> _Note: this issue was identified against the code base at commit `d7a2f366149cfae4c7c153e56172220f0cdfc550`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteObject

## Description
Three net-new root causes, all of them a delete that AWS completes with its modeled success status (HTTP 204, empty body) but the emulator refuses with a 4xx. A fixer must close all three.

Cause 1 of 3: AbortMultipartUpload is not idempotent.
AWS: AbortMultipartUpload answers its modeled success status for an upload id that is no longer present (already completed, already aborted, or never created) — the operation is a no-op cleanup call. Smithy: AbortMultipartUpload http {method DELETE, uri /{Bucket}/{Key+}?x-id=AbortMultipartUpload, code 204}; AbortMultipartUploadOutput carries no body members, and NoSuchUpload (httpError 404) is the only modeled error. The recorded response for this step is 204 with no body.
Emulator: answers 404 for the same call, i.e. it raises NoSuchUpload where AWS returns 204 (replay: "status: expected 204, got 404").
Why: the standard cleanup pattern — abort unconditionally in a finally block or after CompleteMultipartUpload — now raises, so every teardown path that aborts an upload it may already have completed fails, and callers must special-case an error AWS does not return.
Fix: the AbortMultipartUpload handler, which must treat an absent upload id as a successful no-op and answer 204. (The behavioral issue 20261029T211510Z-s3-refusal-fidelity-abortmultipartupload-is-not-a-no.md describes the same idempotency gap from the behavioral suite; it does not carry this fixture's replay key, so this wire step is still unexercised by it.)
Traces: s3-multipart-upload-lifecycle#task#13

Cause 2 of 3: DeleteObject of a key/version that is already gone answers 404 instead of 204.
AWS: DeleteObject is idempotent — deleting a key (or a version id) that does not exist succeeds. Smithy: DeleteObject http {method DELETE, uri /{Bucket}/{Key+}?x-id=DeleteObject, code 204}, DeleteObjectOutput binds only response headers (x-amz-delete-marker, x-amz-version-id, x-amz-request-charged) and the operation models NO errors at all, so there is no 404 for it to return. The recorded responses for these teardown steps are 204 with no body.
Emulator: answers 404 (replay: "status: expected 204, got 404") for the object-lock teardown deletes.
Why: teardown and retry logic that deletes a key it cannot be sure still exists — the normal way to empty a bucket or to retry a timed-out delete — fails against the emulator, and a client that trusts AWS's "delete never 404s" contract surfaces a spurious error to its caller.
Fix: the DeleteObject handler's state lookup, which must not convert a missing key or missing version into a 404 but return 204 (setting x-amz-delete-marker / x-amz-version-id when a marker is created).
Traces: s3w-object-lock-governance-delete#teardown#10, s3w-object-lock-legal-hold-delete#teardown#12

Cause 3 of 3: DeleteBucket refuses an emptied bucket with 409.
AWS: DeleteBucket on a bucket whose objects, versions and delete markers have been removed succeeds. Smithy: DeleteBucket http {method DELETE, uri /{Bucket}, code 204}, output Unit, no modeled errors. The recorded response for this teardown step is 204 with no body, so AWS considered the bucket empty after the preceding teardown deletes.
Emulator: answers 409 (replay: "status: expected 204, got 409"), i.e. it still regards the bucket as non-empty (BucketNotEmpty, "The bucket you tried to delete is not empty"; the fixed text is AWS's wording for that class and the BucketName member is the bucket from the request). Hypothesis: residual version or delete-marker rows left behind by the deletes in the same teardown (see Cause 2) keep the emptiness check true.
Why: a client cannot delete a bucket it has just emptied, so every create-use-destroy lifecycle leaks a bucket and the account's bucket count grows without bound; the refusal is also unrecoverable, because repeating the object deletes does not change the emulator's view.
Fix: the DeleteBucket handler's emptiness predicate and the object/version state it reads, which must agree with what the delete paths removed.
Traces: s3w-object-validation-error-envelopes#teardown#17

## Evidence
Cause 1 — s3-multipart-upload-lifecycle#task#13, AbortMultipartUpload: expected status 204 (recorded, empty body), actual status 404. divergence_key service=s3|AbortMultipartUpload|status=204->404|keys=status.
Cause 2 — s3w-object-lock-governance-delete#teardown#10 (and s3w-object-lock-legal-hold-delete#teardown#12), DeleteObject: expected status 204, actual status 404. divergence_key service=s3|DeleteObject|status=204->404|keys=status.
Cause 3 — s3w-object-validation-error-envelopes#teardown#17, DeleteBucket: expected status 204, actual status 409. divergence_key service=s3|DeleteBucket|status=204->409|keys=status.
Smithy (com.amazonaws.s3#AmazonS3): AbortMultipartUpload code 204 / only error NoSuchUpload(404); DeleteObject code 204 / errors: []; DeleteBucket code 204 / output Unit / errors: [].

## Steps to Reproduce
Replay the wire corpus (replay_wire_corpus) and inspect these steps:
1. s3-multipart-upload-lifecycle step task#13 — CreateMultipartUpload then CompleteMultipartUpload (or abort) the upload, then call AbortMultipartUpload with that same uploadId. Expect HTTP 204 with an empty body; the emulator returns 404.
2. s3w-object-lock-governance-delete step teardown#10 (same at s3w-object-lock-legal-hold-delete teardown#12) — DELETE the object key/version during teardown after it has already been removed. Expect HTTP 204; the emulator returns 404.
3. s3w-object-validation-error-envelopes step teardown#17 — after the teardown deletes every object/version in the bucket, DELETE /{Bucket}. Expect HTTP 204; the emulator returns 409.
