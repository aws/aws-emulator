Issue-Source: trace-sweep-divergence
Issue-Trace: cleanrooms-protected-query-returns-joined-row
Issue-Step: task#22
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cleanrooms-protected-query-returns-joined-row
Issue-Shape-Id: com.amazonaws.cleanrooms#AWSBastionControlPlaneServiceLambda
Issue-Key: s3|ListObjectsV2|2xx->2xx|$.ListBucketResult.Contents
Issue-Key: s3|ListObjectsV2|2xx->2xx|$.ListBucketResult.KeyCount
Issue-Key: s3|DeleteBucket|2xx->4xx|status
Issue-Commit: a298555d8b7939ba1a31a08b252b3b97a8469fe1

# s3 bucket object index is inconsistent for a service-written result object: ListObjectsV2 reports an empty bucket while DeleteBucket refuses it as non-empty

> _Note: this issue was identified against the code base at commit `a298555d8b7939ba1a31a08b252b3b97a8469fe1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListObjectsV2

## Description
2 net-new causes in one s3 fault: an object that was written into a bucket by a service-side writer (here the Clean Rooms protected-query result output) is never registered in the bucket's listing index, yet it IS still counted by the bucket's emptiness test. The two replayed symptoms are the two sides of that single divergence between the listing view of a bucket and the object-存在 view of the same bucket.

Cause 1 of 2 — ListObjectsV2 omits an object that exists in the bucket (fixture cleanrooms-protected-query-returns-joined-row, step task#22).
AWS: ListObjectsV2 (Smithy: GET /{Bucket}?list-type=2, success code 200, output shape ListObjectsV2Output with xmlName ListBucketResult; members KeyCount:Integer and Contents:ObjectList, xmlFlattened) returns one <Contents> element per key in the bucket that matches the request's Prefix/Delimiter/StartAfter, and KeyCount is the number of keys returned in that response (Contents entries plus CommonPrefixes). Every object durably stored in the bucket is listable, regardless of which principal or which service-side writer created it; S3 provides read-after-write consistency for a new object, so a PUT that has been acknowledged is visible to the next ListObjectsV2. Each listed entry carries Key, LastModified, ETag, Size, StorageClass (STANDARD when no storage class was requested) and, when the object was stored with a full-object checksum, ChecksumAlgorithm/ChecksumType.
Emulator: for the same bucket the emulator answers 200 with KeyCount '0' and NO Contents element at all, where the recorded AWS response is KeyCount '1' with a single Contents entry (ChecksumAlgorithm CRC64NVME, ChecksumType FULL_OBJECT, Size 22, StorageClass STANDARD; Key/LastModified/ETag are per-run values and are not themselves the requirement).
Why: a client that enumerates a bucket to find the output of an asynchronous, service-written job sees an empty bucket and concludes the job produced nothing. Every poll-until-the-object-appears loop never terminates, and any consumer that drives GetObject off a listing never reads the result that was in fact written. The bucket's apparent and real contents disagree, which is not a state AWS can be in.
Fix: the ListObjectsV2 handler's read of the bucket's key index in the s3 service's object state — the service-side write path stores the object bytes without inserting the key into the index the listing walks (or inserts it under a container the listing does not scan). Registration must happen on the same path for every writer, not only for client PutObject/CopyObject. The listed entry must then be projected with Size, StorageClass and the stored full-object checksum fields.

Cause 2 of 2 — DeleteBucket is refused 409 on a bucket AWS deletes with 204 (fixture cleanrooms-protected-query-returns-joined-row, step teardown#36).
AWS: DeleteBucket (Smithy: DELETE /{Bucket}, success code 204, empty output, no modeled operation errors) answers 204 No Content with an empty body when the bucket is deletable. A bucket that still holds objects or incomplete multipart uploads is refused 409 Conflict with error code BucketNotEmpty and the message "The bucket you tried to delete is not empty" — fixed text, with the offending bucket named in the response's <BucketName> element rather than interpolated into the sentence. That refusal is correct only while the bucket really does hold keys.
Emulator: the emulator answers 409 for this teardown delete where the recorded AWS response is 204 No Content. The same bucket had just been reported by the emulator's own ListObjectsV2 as KeyCount 0, so the emulator simultaneously claims the bucket is empty and refuses to delete it as non-empty. Hypothesis: the residue the emptiness test trips on is exactly the unindexed service-written object from Cause 1 (or a tombstone the teardown's object delete could not address, because the key was not listable and so was never deleted).
Why: teardown is unrunnable. A client that correctly drains a bucket — list, delete every returned key, then delete the bucket — is told the bucket is empty, deletes nothing, and is then refused 409 forever, so scripts, CloudFormation stack deletes and test fixtures leak buckets and exit non-zero on a sequence AWS completes.
Fix: the DeleteBucket handler's emptiness predicate in the s3 service's bucket state must consult the SAME key index ListObjectsV2 serves, so the two can never disagree; once Cause 1 registers service-written keys in that index, the listing exposes the key, the teardown delete removes it and DeleteBucket returns 204 with an empty body. A bucket that genuinely still holds keys must keep answering 409 BucketNotEmpty with AWS's fixed message text and the bucket name carried in <BucketName>.

Related but not the same as the open issue "s3 ListObjectsV2 misreports the MediaConvert output object: key loses the input basename and ChecksumAlgorithm is wrong" (20261125T223657Z): there the object IS listed with wrong fields, whereas here it is not listed at all and additionally blocks DeleteBucket. Neither divergence was reported as owned by an open issue or blocked by an unimplemented operation.

## Evidence
Replay of trace set simparity-wire-aws-v1, fixture cleanrooms-protected-query-returns-joined-row (both steps verdict=mismatch, stub_path empty, owned_by empty).

Cause 1 — step task#22, ListObjectsV2, expected_status 200 / actual_status 200, body mismatch:
  $.ListBucketResult.KeyCount: value_mismatch (expected '1', got '0')
  $.ListBucketResult.Contents: missing (expected {'Key': '<shaped-not-asserted>', 'LastModified': '<shaped-not-asserted>', 'ETag': '<shaped-not-asserted>', 'ChecksumAlgorithm': 'CRC64NVME', 'ChecksumType': 'FULL_OBJECT', 'Size': '22', 'StorageClass': 'STANDARD'}, got None)
  divergence_key: service=s3|ListObjectsV2|status=200->200|keys=$.ListBucketResult.Contents,$.ListBucketResult.KeyCount
  Expected: 200 with one Contents entry and KeyCount 1. Actual: 200 with KeyCount 0 and no Contents element.

Cause 2 — step teardown#36, DeleteBucket, expected_status 204 / actual_status 409:
  detail: status: expected 204, got 409
  divergence_key: service=s3|DeleteBucket|status=204->409|keys=status
  Expected: 204 No Content, empty body. Actual: 409 Conflict (the bucket-not-empty refusal class), on the same bucket the emulator's own ListObjectsV2 reported as KeyCount 0.

Cross-check: the two expectations are mutually inconsistent inside the emulator — one response says the bucket has no keys, the other refuses to delete it because it does. In AWS both hold together: the object is listed, the teardown deletes it, the bucket delete returns 204.

Smithy citations (services/service-com-amazonaws-s3-amazons3/smithy-model/com-amazonaws-s3-amazons3.json): ListObjectsV2 http {GET /{Bucket}?list-type=2, code 200}, output ListObjectsV2Output xmlName=ListBucketResult, members KeyCount (integer) and Contents (ObjectList, xmlFlattened), error NoSuchBucket(404); DeleteBucket http {DELETE /{Bucket}, code 204}, output Unit, no modeled errors.

## Steps to Reproduce
1. Clone the repo and boot the emulator built from it.
2. Replay the wire corpus: replay_wire_corpus() over trace set simparity-wire-aws-v1.
3. Cause 1: fixture cleanrooms-protected-query-returns-joined-row, step task#22 — after the protected query has written its single result object into the result bucket, issue GET /{Bucket}?list-type=2. Observe 200 with KeyCount 0 and no Contents element, instead of KeyCount 1 and one Contents entry (Size 22, StorageClass STANDARD, ChecksumAlgorithm CRC64NVME, ChecksumType FULL_OBJECT).
4. Cause 2: same fixture, step teardown#36 — drain the bucket using the keys the listing returned (none), then issue DELETE /{Bucket}. Observe 409 instead of 204 No Content, i.e. the bucket is refused as non-empty immediately after being reported empty.
5. After the fix, re-replay cleanrooms-protected-query-returns-joined-row#task#22 and cleanrooms-protected-query-returns-joined-row#teardown#36; both must pass.
