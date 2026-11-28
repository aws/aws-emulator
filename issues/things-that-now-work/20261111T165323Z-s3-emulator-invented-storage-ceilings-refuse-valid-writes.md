Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: s3.bucket-holding-more-than-sixty-four-mib-reads-back-every-object s3.list-objects-v2-default-page-holds-one-thousand-keys-of-a-larger-bucket s3.list-object-versions-pages-one-key-written-more-than-one-thousand-times s3.objects-totalling-over-128-mib-across-three-buckets-all-read-back s3.upload-part-of-twenty-mib-completes-and-reads-back-exactly
Issue-Shape-Id: com.amazonaws.s3#AmazonS3
Issue-Key: s3|PutObject|ValidationException
Issue-Key: s3|UploadPart|413
Issue-Commit: f0a035afe3a6d4bc3a732ff8fc28ae95e972cd65

# s3: emulator-invented storage ceilings refuse valid writes — 64 MiB/1000-object per-bucket caps, a 128 MiB account-wide cap, and a 413 on a 20 MiB UploadPart (4 causes)

> _Note: this issue was identified against the code base at commit `f0a035afe3a6d4bc3a732ff8fc28ae95e972cd65`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Resolution (three of the four causes)

- Causes 1, 2 and 3 — FIXED: the object store keeps no per-bucket object-count ceiling, no
  per-bucket byte ceiling and no account-wide byte ceiling at all, so a write within the
  payload size S3 accepts is served whatever the bucket, the key or the account already
  holds; a versioned key's versions are charged against nothing; and the one bound that is
  left — the whole-store memory guard that keeps this process bounded, raised to 250,000
  stored versions and 1 GiB of retained bytes, orders of magnitude above the traffic the API
  admits — is answered, if it is ever met, with S3's own `ServiceUnavailable` (HTTP 503,
  "Service is unable to handle request.") rather than the `ValidationException` that is not
  in S3's vocabulary. Four of this report's five checks pass.
  `tests/all/object_store_capacity_is_not_a_quota.rs` pins the rule end to end: a table of
  object counts and sizes filling ONE bucket, 1,001 distinct keys served and then paged
  through `ListObjectsV2`, 1,001 versions of ONE key served and paged through
  `ListObjectVersions`, and nine 15 MiB objects across three buckets read back byte-exact.
- Cause 4 (a 20 MiB `UploadPart` refused with a bare HTTP 413) — STILL OPEN, and NOT a
  handler defect: the refusal is `MAX_PRODUCTION_BODY_BYTES = 16 * 1024 * 1024` in
  `vendor/server/src/lib.rs`, applied by `read_production_body` on the production serve path
  before any service sees the request, so no service crate can admit such a body or word the
  refusal in an S3 `<Error>` envelope. The same core cap, and the check
  `s3.upload-part-of-twenty-mib-completes-and-reads-back-exactly` with it, is owned by
  `20261118T090000Z-s3-2-causes-left-open-a-32-mib-putobject-and-the-v1-metadata-table-sentence.md`
  (whose cause 1 is the same cap refusing a 32 MiB `PutObject`, and which already names this
  report's cause 4 as the same blocker). The core change it needs: raise that cap, or make it
  configurable per service, so a service's own documented limits are what bound a write.

## Operation
PutObject

## Description
Four net-new root causes in the S3 object-store write path. All four are the emulator imposing capacity limits that AWS S3 does not have, so ordinary data-plane writes are refused instead of served. They are bundled because they share one service (s3) and one component family (the object store's capacity accounting plus the request-body size limit in front of it), but they are FOUR separate causes and each must be closed before this issue is submitted.

--- Cause 1 of 4: a 64 MiB per-bucket byte ceiling refuses PutObject ---
AWS: S3 places no limit on how many bytes a bucket holds — a bucket's storage is unlimited, a single object may be up to 5 TiB, and a single-part PutObject may carry up to 5 GiB. Smithy (com-amazonaws-s3-amazons3): PutObject.Body is a StreamingBlob @httpPayload with no length constraint; PutObject.ContentLength is a plain `long` with no @range; PutObject's modeled errors are exactly EncryptionTypeMismatch (400), InvalidRequest (400), InvalidWriteOffset (400) and TooManyParts (400) — there is no capacity/quota error on the operation, and S3 is a REST-XML service whose refusals are <Error><Code>…</Code></Error> envelopes, so `ValidationException` is not an S3 error code at all. Writing five 13 MiB objects (65 MiB) into one bucket must answer HTTP 200 on every PutObject, with ETag equal to the body's MD5.
Emulator: the fifth PutObject is refused with error code `ValidationException` and the emulator-authored message `storing "blob/4.bin" would leave the bucket "sim2real-bigbkt-d23ef92d2f79" holding 68157895 bytes of objects, past the 67108864 bytes this emulator keeps per bucket`. Fixed text of that template: `storing "` … `" would leave the bucket "` … `" holding ` … ` bytes of objects, past the 67108864 bytes this emulator keeps per bucket`; the variable parts are the request's Key, the request's Bucket and the running byte total from emulator state — the quoted key, bucket name and byte counts are one run's values, not part of the requirement. The requirement is that no such refusal exists: the write is served 200.
Why: a client that stores more than 64 MiB in one bucket — any realistic upload set, log drop, dataset or asset bundle — gets a 400 on a call AWS accepts, with an error code (`ValidationException`) that is not in S3's vocabulary, so SDK error handling cannot even classify it. Workloads that fan out over object storage cannot be exercised at all.
Fix: the S3 object store's per-bucket capacity accounting and the PutObject handler's pre-store check that consults it — the ceiling must be removed (or raised beyond anything a check can reach) rather than re-worded, and PutObject must not be able to emit a non-S3 `ValidationException`.
Traces: s3.bucket-holding-more-than-sixty-four-mib-reads-back-every-object.

--- Cause 2 of 4: a 1000-object per-bucket ceiling refuses PutObject, and object versions count against it ---
AWS: a bucket may hold any number of objects; the 1,000 figure in S3 is the DEFAULT PAGE SIZE of a listing, not a storage limit. Smithy: ListObjectsV2 is @paginated (inputToken ContinuationToken, outputToken NextContinuationToken, pageSize MaxKeys) and returns IsTruncated/KeyCount/NextContinuationToken precisely so that a bucket larger than one page can be walked; the same holds for ListObjectVersions over many versions of one key. So 1,001 PutObject calls to distinct keys, and 1,001 overwrites of ONE key in a versioning-enabled bucket (which create 1,001 versions, not 1,001 objects), must all answer 200.
Emulator: the 1,001st write is refused with code `ValidationException` and the message `the bucket "sim2real-lpage-c358a9ea47ce" already holds the maximum of 1000 objects this emulator keeps per bucket; delete an object before writing "obj/1000"` (fixed text: `the bucket "` … `" already holds the maximum of 1000 objects this emulator keeps per bucket; delete an object before writing "` … `"`; variable parts: the request's Bucket and Key). The same refusal fires for repeated writes to a SINGLE key on a versioned bucket (`… before writing "hot/key.txt"`), so the counter charges object VERSIONS against a per-bucket object count — on AWS a versioned key's version count is unbounded.
Why: the whole pagination contract of the S3 list surface is unreachable — no client can observe a truncated first page, a continuation token, or version pagination, because the bucket can never exceed one page. A client writing a hot key in a versioned bucket is also refused after 1,000 revisions.
Fix: the S3 object store's per-bucket object-count accounting and the PutObject handler's pre-store check; additionally the counter must not treat non-current versions of one key as separate objects against any ceiling.
Traces: s3.list-objects-v2-default-page-holds-one-thousand-keys-of-a-larger-bucket, s3.list-object-versions-pages-one-key-written-more-than-one-thousand-times.

--- Cause 3 of 4: an emulator-wide 128 MiB ceiling across every bucket refuses PutObject ---
AWS: there is no account-wide or service-wide stored-byte quota on S3 object storage, and no error is modeled for one (see the PutObject error list in Cause 1). Three buckets each holding three 15 MiB objects (135 MiB in total) must all be written and read back.
Emulator: a PutObject that would take the total past 128 MiB is refused with code `ValidationException` and the message `storing "part-c.bin" would leave this emulator holding 141558579 bytes of objects across every bucket, past the 134217728 bytes it keeps in total` (fixed text: `storing "` … `" would leave this emulator holding ` … ` bytes of objects across every bucket, past the 134217728 bytes it keeps in total`; variable parts: the request's Key and the running global total). This is distinct from Cause 1: it is a GLOBAL counter, so it also fails writes that are individually small and spread over several buckets, and raising the per-bucket ceiling alone will not fix it.
Why: cross-bucket workloads (copy/replicate/export pipelines, multi-bucket estates) hit a 400 that depends on what OTHER buckets hold, making every such call order-dependent and non-reproducible for a client.
Fix: the S3 object store's account/service-wide byte accounting and the PutObject handler's pre-store check that consults it.
Traces: s3.objects-totalling-over-128-mib-across-three-buckets-all-read-back.

--- Cause 4 of 4: UploadPart of a 20 MiB part is refused HTTP 413 Request Entity Too Large ---
AWS: a multipart part may be 5 MiB to 5 GiB (only the last part may be smaller), so a single 20 MiB part is well inside the allowed range and UploadPart answers HTTP 200 with ETag equal to the part's MD5. Smithy: UploadPart declares NO errors at all, and UploadPart.Body is a StreamingBlob @httpPayload with no length constraint — there is no modeled refusal for part size, and S3 never answers an UploadPart with a bare HTTP 413.
Emulator: UploadPart with a 20 MiB body is refused with HTTP 413 and the reason phrase `Request Entity Too Large` as the whole response, with no S3 <Error> envelope — botocore surfaces it as error code `413`, message `Request Entity Too Large`, because there is no Code/Message to parse. This is a request-body size limit in front of the handler, not the storage accounting of Causes 1–3.
Why: multipart upload — the only way to write large objects — is unusable above the emulator's body limit, and because the refusal carries no S3 error envelope a client cannot retry or classify it; SDK retry/error logic sees an unmodeled transport-level status.
Fix: the S3 request-body size limit applied before the UploadPart handler (it must admit at least S3's 5 GiB part maximum, or stream the body rather than bound it), and any refusal on that path must be a REST-XML S3 <Error> envelope, never a bare 413 reason phrase.
Traces: s3.upload-part-of-twenty-mib-completes-and-reads-back-exactly.

No open issue covers these: the suite report marks every other S3 failure as OWNED by an existing issue and leaves exactly these five checks unattributed. The nearest open S3 issues — 20261111T040849Z-s3-seven-s3-fidelity-causes-configuration-limit-and-validation.md (configuration-limit and validation message wording) and 20261110T221513Z-s3-error-envelopes-diverge-wrong-code-status-missing.md (error envelope fidelity) — are about per-configuration limits and envelope shape on OTHER operations and do not touch the object-store capacity ceilings or the UploadPart body limit.

## Evidence
All five grounding checks are [error] verdicts: the emulator refused a write that AWS serves, so the check never reached its assertions.

Cause 1 — s3.bucket-holding-more-than-sixty-four-mib-reads-back-every-object (5 x 13 MiB single-part PutObject into one bucket):
  expected: every PutObject -> HTTP 200, ETag == MD5(body); ListObjectsV2 lists 5 objects of 13631488 bytes; GetObject returns each body byte-for-byte.
  actual:   botocore.exceptions.ClientError: An error occurred (ValidationException) when calling the PutObject operation: storing "blob/4.bin" would leave the bucket "sim2real-bigbkt-d23ef92d2f79" holding 68157895 bytes of objects, past the 67108864 bytes this emulator keeps per bucket

Cause 2 — s3.list-objects-v2-default-page-holds-one-thousand-keys-of-a-larger-bucket (1,001 distinct keys):
  expected: all 1,001 PutObject -> 200; ListObjectsV2 without MaxKeys -> MaxKeys 1000, KeyCount 1000, IsTruncated true, NextContinuationToken returning the 1,001st key.
  actual:   botocore.exceptions.ClientError: An error occurred (ValidationException) when calling the PutObject operation: the bucket "sim2real-lpage-c358a9ea47ce" already holds the maximum of 1000 objects this emulator keeps per bucket; delete an object before writing "obj/1000"
Cause 2 — s3.list-object-versions-pages-one-key-written-more-than-one-thousand-times (1,001 overwrites of ONE key on a versioned bucket):
  expected: 1,001 distinct VersionIds; ListObjectVersions pages them newest-first 1,000 at a time.
  actual:   botocore.exceptions.ClientError: An error occurred (ValidationException) when calling the PutObject operation: the bucket "sim2real-hotkey-03f1c23f517f" already holds the maximum of 1000 objects this emulator keeps per bucket; delete an object before writing "hot/key.txt"  (i.e. versions of one key are charged as separate objects)

Cause 3 — s3.objects-totalling-over-128-mib-across-three-buckets-all-read-back (3 buckets x 3 x 15 MiB):
  expected: every PutObject -> 200; every GetObject returns 15728640 bytes whose MD5 matches what was written.
  actual:   botocore.exceptions.ClientError: An error occurred (ValidationException) when calling the PutObject operation: storing "part-c.bin" would leave this emulator holding 141558579 bytes of objects across every bucket, past the 134217728 bytes it keeps in total

Cause 4 — s3.upload-part-of-twenty-mib-completes-and-reads-back-exactly (one 20 MiB part):
  expected: UploadPart -> HTTP 200, ETag == MD5(part); CompleteMultipartUpload -> MD5(part-MD5)-1; GetObject returns the 20971520 bytes.
  actual:   botocore.exceptions.ClientError: An error occurred (413) when calling the UploadPart operation: Request Entity Too Large  (bare HTTP 413, no S3 <Error><Code> envelope)

Model evidence (services/service-com-amazonaws-s3-amazons3/smithy-model/com-amazonaws-s3-amazons3.json): PutObject errors = EncryptionTypeMismatch | InvalidRequest | InvalidWriteOffset | TooManyParts (no capacity error, no ValidationException); PutObject.Body / UploadPart.Body are unconstrained streaming blobs; UploadPart declares no errors; ListObjectsV2 is @paginated on MaxKeys/ContinuationToken. Capacity accounting appears to live with the S3 object store state (services/service-com-amazonaws-s3-amazons3/src/state.rs).

## Steps to Reproduce
Against the emulator's S3 endpoint, with any credentials the emulator accepts:

Cause 1: CreateBucket B; PutObject 13 MiB bodies to keys blob/0.bin .. blob/4.bin. The 5th PutObject (crossing 67108864 bytes in B) returns 400 ValidationException instead of 200. Expected: all five 200 with ETag == MD5(body).

Cause 2a: CreateBucket B; PutObject 1,001 distinct keys obj/0000 .. obj/1000. The 1,001st returns 400 ValidationException instead of 200; then ListObjectsV2 without MaxKeys can never report MaxKeys=1000 / KeyCount=1000 / IsTruncated=true with a usable NextContinuationToken.
Cause 2b: CreateBucket B; PutBucketVersioning Status=Enabled; PutObject the SAME key hot/key.txt 1,001 times. The 1,001st write is refused with the per-bucket 1000-OBJECT message even though the bucket holds one key. Expected: 1,001 distinct VersionIds, ListObjectVersions paging 1,000 newest-first then the oldest on page 2.

Cause 3: CreateBucket B1, B2, B3; PutObject three 15 MiB objects into each. The write that crosses 134217728 total bytes across all buckets returns 400 ValidationException naming the global total, even though no single bucket is near 64 MiB. Expected: all nine 200, all nine readable byte-exact.

Cause 4: CreateBucket B; CreateMultipartUpload key twenty-mib-part.bin; UploadPart PartNumber=1 with a 20 MiB body. The emulator answers HTTP 413 Request Entity Too Large with no XML error body. Expected: 200 with ETag == MD5(part), then CompleteMultipartUpload -> ETag MD5(part-MD5)-1 and GetObject returning the 20971520 bytes.

Replay to confirm: the five check ids in trace_ids.
