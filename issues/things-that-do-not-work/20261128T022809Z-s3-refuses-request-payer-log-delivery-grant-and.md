Issue-Source: trace-sweep-divergence
Issue-Trace: s3-abort-multipart-upload-request-payer-header-values
Issue-Step: task#5
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: s3-abort-multipart-upload-request-payer-header-values s3-bucket-logging-target-grant-permission-outside-enum s3-request-payer-requester-from-the-owner-on-a-bucket-without-requester-pays s3-request-payer-requester-from-the-owner-on-a-requester-pays-bucket s3-restore-object-tier-outside-enum-refused
Issue-Shape-Id: com.amazonaws.s3#AmazonS3
Issue-Key: s3|AbortMultipartUpload|2xx->4xx|$
Issue-Key: s3|AbortMultipartUpload|2xx->4xx|status
Issue-Key: s3|ListMultipartUploads|2xx->2xx|$.ListMultipartUploadsResult.Upload
Issue-Key: s3|PutBucketLogging|2xx->4xx|status
Issue-Key: s3|GetBucketLogging|2xx->2xx|$.BucketLoggingStatus
Issue-Key: s3|HeadObject|2xx->5xx|$headers.accept-ranges
Issue-Key: s3|HeadObject|2xx->5xx|$headers.content-type
Issue-Key: s3|HeadObject|2xx->5xx|$headers.etag
Issue-Key: s3|HeadObject|2xx->5xx|$headers.last-modified
Issue-Key: s3|HeadObject|2xx->5xx|$headers.x-amz-server-side-encryption
Issue-Key: s3|HeadObject|2xx->5xx|status
Issue-Key: s3|RestoreObject|4xx->4xx|status
Issue-Related: 20261121T075817Z-s3-the-s3files-mount-write-through-never-materialises.md
Issue-Related: 20261128T022809Z-s3-rejects-signed-requests-whose-header-values-carry.md
Issue-Commit: 8e0150f548083d70861fe6ec8aea207c225cbc47

# s3 refuses request-payer, log-delivery grant and restore-tier requests that AWS serves, and one read path is still unimplemented

> _Note: this issue was identified against the code base at commit `8e0150f548083d70861fe6ec8aea207c225cbc47`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AbortMultipartUpload

## Description
7 net-new causes sharing ONE fault: requests AWS accepts are refused, because the emulator applies a stricter or simply absent rule on the x-amz-request-payer header, on log-delivery ACL grants and on a restore refusal — and one object read path answers 501 Not Implemented. In each case the refusal (or the 501) also leaves the fixture's later steps observing the wrong state. These operations declare few or no error shapes in Smithy (AbortMultipartUpload: NoSuchUpload/404; PutBucketLogging: errors: []; RestoreObject: ObjectAlreadyInActiveTierError/403), so the behaviors below are recorded behavior only.

Cause 1 of 7 — AbortMultipartUpload refuses an x-amz-request-payer value outside the enum. AWS: Smithy com.amazonaws.s3#RequestPayer has the single value 'requester', and its documentation says bucket owners need not specify the parameter at all — a misspelled value from the bucket owner does not block the abort, which answers 204 No Content with an empty body. Recorded behavior only. Emulator: 400 with Code 'InvalidArgument' and the Smithy constraint sentence "Value '<v>' at 'requestPayer' failed to satisfy constraint: Member must satisfy enum value set: [requester]"; the upload is not aborted. Why: an abort is the cleanup path — refusing it leaks the multipart upload and its parts, and a client cannot release storage. Fix: the AbortMultipartUpload handler's request-payer handling must not refuse the request on this header for the bucket owner. Traces: s3-abort-multipart-upload-request-payer-header-values#task#5.

Cause 2 of 7 — ListMultipartUploads still lists the upload Cause 1 should have aborted. AWS: the aborted upload is gone, so only the remaining upload is listed, each entry carrying Key, UploadId, Initiator, Owner, StorageClass and Initiated. Emulator: both uploads are listed. Why: the leak of Cause 1 is directly observable and a cleanup sweep never converges. Fix: follows from Cause 1. Traces: s3-abort-multipart-upload-request-payer-header-values#task#6.

Cause 3 of 7 — PutBucketLogging refuses a TargetGrant to the S3 log-delivery group. AWS: a TargetGrants Grant whose Grantee is the URI http://acs.amazonaws.com/groups/s3/LogDelivery with Permission READ_ACP is a legal log-delivery grant (Smithy com.amazonaws.s3#Permission includes READ_ACP) and the configuration is accepted — 200 with an empty body. Emulator: 400; nothing stored. Why: server access logging cannot be configured at all, because the grant every logging setup needs is rejected. Fix: the PutBucketLogging TargetGrants validation — the group-URI grantee form and the READ_ACP permission must both be accepted. Traces: s3-bucket-logging-target-grant-permission-outside-enum#task#6.

Cause 4 of 7 — GetBucketLogging returns an empty BucketLoggingStatus. AWS: the read returns BucketLoggingStatus/LoggingEnabled with TargetBucket, TargetPrefix and the TargetGrants exactly as submitted. Emulator: BucketLoggingStatus is an empty element. Why: the logging configuration is unreadable, so a client cannot verify or reconcile it. Fix: follows from Cause 3 for the state; the GetBucketLogging serializer must emit the stored LoggingEnabled subtree. Traces: s3-bucket-logging-target-grant-permission-outside-enum#task#7.

Cause 5 of 7 — HeadObject answers 501 Not Implemented on the request-payer path. AWS: 200 with last-modified, ETag, x-amz-server-side-encryption AES256, accept-ranges 'bytes' and the object's content-type. Emulator: 501 with an application/xml body and none of those headers. Why: 501 is not a verdict AWS ever gives here; a HEAD with x-amz-request-payer: requester is the ordinary requester-pays read form, and the whole requester-pays read surface is unusable. Fix: implement the request-payer branch of the HeadObject handler. Traces: s3-request-payer-requester-from-the-owner-on-a-bucket-without-requester-pays#task#3, s3-request-payer-requester-from-the-owner-on-a-requester-pays-bucket#task#5.

Cause 6 of 7 — GetObject answers 404 NoSuchKey for an object the fixture wrote. AWS: 200 serving the object bytes with accept-ranges, last-modified, ETag, x-amz-checksum-crc64nvme, x-amz-checksum-type FULL_OBJECT, AES256 and content-type 'binary/octet-stream'. Emulator: 404 with Code 'NoSuchKey', Message 'The specified key does not exist.' and <Key> = the requested key. Why: the object is missing because the earlier request-payer step never completed, so a requester-pays read reports a phantom missing object. Fix: follows from Cause 5. Traces: s3-request-payer-requester-from-the-owner-on-a-bucket-without-requester-pays#task#11, s3-request-payer-requester-from-the-owner-on-a-requester-pays-bucket#task#13.

Cause 7 of 7 — RestoreObject answers 400 where AWS answers 403. AWS: a restore of an object already in the active tier is refused 403 — Smithy RestoreObject models ObjectAlreadyInActiveTierError with httpError 403 — so the status class is 'this object's state forbids it', not 'your request is malformed'. Emulator: 400. Why: a client branches on the status class to decide between repairing the request and accepting the object as already restored; 400 sends it down the repair path forever. Fix: the RestoreObject handler must raise the modeled 403 error for an already-active object rather than a 400. Traces: s3-restore-object-tier-outside-enum-refused#task#5.

## Evidence
Replay of simparity-wire-aws-v1 against the emulator.
Representative s3-abort-multipart-upload-request-payer-header-values#task#5 (AbortMultipartUpload): status expected 204, got 400; body expected {}, got {'Error': {'Code': 'InvalidArgument', 'Message': "Value 'requestor' at 'requestPayer' failed to satisfy constraint: Member must satisfy enum value set: [requester]", ...}}.
Other causes:
- #task#6 (ListMultipartUploads, 200/200): $.ListMultipartUploadsResult.Upload — the recording holds only the second upload, the emulator returns both because the abort was refused.
- s3-bucket-logging-target-grant-permission-outside-enum#task#6 (PutBucketLogging): status expected 200, got 400.
- #task#7 (GetBucketLogging, 200/200): $.BucketLoggingStatus type_mismatch — expected LoggingEnabled with TargetBucket, TargetPrefix and the LogDelivery/READ_ACP TargetGrant, got ''.
- s3-request-payer-requester-from-the-owner-on-a-bucket-without-requester-pays#task#3 (HeadObject): status expected 200, got 501; last-modified, etag, x-amz-server-side-encryption ('AES256') and accept-ranges missing; content-type expected 'text/plain' got 'application/xml'.
- #task#11 (GetObject): status expected 200, got 404 with {'Error': {'Code': 'NoSuchKey', 'Message': 'The specified key does not exist.', 'Key': 'ledger/batch.txt', ...}}; accept-ranges, last-modified, etag, x-amz-checksum-crc64nvme, x-amz-checksum-type and SSE headers missing; content-type expected 'binary/octet-stream' got 'application/xml'.
- s3-restore-object-tier-outside-enum-refused#task#5 (RestoreObject): status expected 403, got 400.

## Steps to Reproduce
1. Replay s3-abort-multipart-upload-request-payer-header-values: task#5's AbortMultipartUpload with a misspelled x-amz-request-payer from the bucket owner must be 204 with an empty body and must actually abort the upload; task#6's ListMultipartUploads must then list only the remaining upload.
2. Replay s3-bucket-logging-target-grant-permission-outside-enum: task#6 must accept the TargetGrant to http://acs.amazonaws.com/groups/s3/LogDelivery with Permission READ_ACP (200); task#7's GetBucketLogging must return the stored LoggingEnabled subtree including that grant.
3. Replay s3-request-payer-requester-from-the-owner-on-a-bucket-without-requester-pays and ...-on-a-requester-pays-bucket: the HeadObject at task#3/#task#5 must be 200 with its recorded headers — no 501 — and the later GetObject (task#11/#task#13) must serve the object bytes.
4. Replay s3-restore-object-tier-outside-enum-refused: task#5 must be refused 403, not 400.
