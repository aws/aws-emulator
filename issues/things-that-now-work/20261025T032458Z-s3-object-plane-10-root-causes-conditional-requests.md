Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: s3.conc-s3-conditional-writes-if-match-on-absent-key-rejected-and-nothing-created s3.conc-s3-conditional-writes-optimistic-counter-loses-no-increment s3.conc-s3-conditional-writes-read-after-conditional-overwrite-every-read-path-is-current s3.put-object-if-match-conditional s3.s3-delete-object-with-a-stale-if-match-etag-is-refused-and-the-object-survives s3.s3-if-none-match-star-put-is-atomic-across-a-delete-marker s3.s3-refused-conditional-gets-create-no-version-and-leave-the-object-intact s3.range-read-open-end s3.if-modified-since s3.deep-s3-immutability-compliance-lock-extends-and-overwrites-but-weakening-rejected s3.deep-s3-immutability-governance-bypass-without-permission-denied s3.deep-s3-immutability-legal-hold-blocks-bypass-independent-of-retention-rejected s3.delete-object-under-legal-hold-rejected s3.delete-object-under-retention-rejected s3.object-retention-governance-blocks-delete s3.object-retention-governance-bypass-delete s3.object-retention-until-date-enforces-window s3.s3-object-lock-compliance-cannot-be-bypassed-while-governance-can s3.object-lock-requires-versioning-rejected s3.get-object-invalid-object-state-glacier s3.restore-object-server-side-tier-transition s3.restore-object-temporary-availability-window s3.s3-itc-restore-rejected-for-an-object-in-the-frequent-access-tier s3.s3-ssec-and-endpoint-enforcement-ssec-multipart-upload-round-trips-only-with-the-customer-key s3.s3-ssec-and-endpoint-enforcement-ssec-upload-part-copy-rekeys-an-ssec-source-into-a-multipart-object s3.s3-ssec-and-endpoint-enforcement-website-endpoint-serving-an-ssec-object-rejected s3.sseinv-new-bucket-default-is-sse-s3-with-bucket-key-off-and-sse-c-blocked s3.create-session-no-such-bucket s3.list-directory-buckets-rejects-regular-bucket-ops s3.put-object-encryption-type-mismatch-rejected s3.rename-object-idempotency-token-mismatch s3.rename-object-source-if-none-match-precondition-rejected s3.s3-express-append-produces-the-exact-concatenation-of-three-offset-writes s3.s3x-s3express-session-and-listing-inflight-mpu-prefix-listed s3.s3x-s3express-session-and-listing-readonly-session-put-denied s3.s3x-s3express-session-and-listing-readwrite-session-round-trip s3.mbes-plain-delete-hides-keys-but-keeps-version-history s3.s3-delete-marker-hides-the-object-while-the-version-still-returns-its-bytes s3.s3-delete-object-tagging-clears-only-the-addressed-version-tag-set
Issue-Shape-Id: com.amazonaws.s3#AmazonS3
Issue-Key: s3.conc-s3-conditional-writes-if-match-on-absent-key-rejected-and-nothing-created|run#9|AssertionError|assert code == "NoSuchKey", f"expected NoSuchKey for If-Match on an absent key, got {code!r}"
Issue-Key: s3|PutObject|NotImplemented
Issue-Key: s3.put-object-if-match-conditional|run#10|AssertionError|assert stale_code == "PreconditionFailed", f"expected PreconditionFailed for a stale If-Match, got {stale_code!r}"
Issue-Key: s3.s3-delete-object-with-a-stale-if-match-etag-is-refused-and-the-object-survives|run#12|AssertionError|assert code == "PreconditionFailed", (
Issue-Key: s3.s3-refused-conditional-gets-create-no-version-and-leave-the-object-intact|run#15|AssertionError|assert nm_status == 304, (
Issue-Key: s3|GetObject|NotImplemented
Issue-Key: s3|GetObject|ValidationException
Issue-Key: s3.deep-s3-immutability-compliance-lock-extends-and-overwrites-but-weakening-rejected|run#4|KeyError|v1 = s3.put_object(Bucket=bucket, Key=key, Body=_V1)["VersionId"]
Issue-Key: s3.deep-s3-immutability-governance-bypass-without-permission-denied|run#4|KeyError|version = s3.put_object(Bucket=bucket, Key=_KEY, Body=_BODY)["VersionId"]
Issue-Key: s3.deep-s3-immutability-legal-hold-blocks-bypass-independent-of-retention-rejected|run#4|KeyError|held = s3.put_object(Bucket=bucket, Key="held.bin", Body=_HELD_BODY)["VersionId"]
Issue-Key: s3.delete-object-under-legal-hold-rejected|run#8|KeyError|version_id = s3.put_object(Bucket=bucket, Key=key, Body=b"protected-content")["VersionId"]
Issue-Key: s3.delete-object-under-retention-rejected|run#8|KeyError|version_id = s3.put_object(Bucket=bucket, Key=key, Body=b"retained-content")["VersionId"]
Issue-Key: s3.object-retention-governance-blocks-delete|run#4|KeyError|control_version = s3.put_object(Bucket=name, Key=control_key, Body=b"control")["VersionId"]
Issue-Key: s3.object-retention-governance-bypass-delete|run#4|KeyError|control_version = s3.put_object(Bucket=bucket, Key=control_key, Body=b"unretained")["VersionId"]
Issue-Key: s3.object-retention-until-date-enforces-window|run#5|KeyError|version_id = put_resp["VersionId"]
Issue-Key: s3.s3-object-lock-compliance-cannot-be-bypassed-while-governance-can|run#7|AssertionError|assert s3.get_bucket_versioning(Bucket=bucket).get("Status") == "Enabled", (
Issue-Key: s3.object-lock-requires-versioning-rejected|run#6|AssertionError|assert code == "InvalidBucketState", f"expected InvalidBucketState on an unversioned bucket, got {code!r}"
Issue-Key: s3.get-object-invalid-object-state-glacier|_client_error#10|AssertionError|raise AssertionError(f"expected a ClientError, but the call succeeded and returned {result!r}")
Issue-Key: s3.restore-object-server-side-tier-transition|_error#12|AssertionError|raise AssertionError(f"expected a ClientError but the call succeeded and returned {result!r}")
Issue-Key: s3.restore-object-temporary-availability-window|_error#6|AssertionError|raise AssertionError(f"expected a ClientError but the call succeeded and returned {result!r}")
Issue-Key: s3.s3-itc-restore-rejected-for-an-object-in-the-frequent-access-tier|run#7|AssertionError|assert code == "InvalidObjectState", (
Issue-Key: s3|CreateMultipartUpload|NotImplemented
Issue-Key: s3.sseinv-new-bucket-default-is-sse-s3-with-bucket-key-off-and-sse-c-blocked|run#7|AssertionError|assert blocked == ["SSE-C"], f"a new bucket blocks SSE-C by default, got {blocked!r}"
Issue-Key: s3.create-session-no-such-bucket|_create_directory_bucket#4|AssertionError|raise AssertionError(f"could not create an S3 Express directory bucket in {ctx.region}: {last_err}")
Issue-Key: s3.list-directory-buckets-rejects-regular-bucket-ops|_create_directory_bucket#3|AssertionError|raise AssertionError(
Issue-Key: s3.put-object-encryption-type-mismatch-rejected|_create_directory_bucket#3|AssertionError|assert bucket is not None, (
Issue-Key: s3.rename-object-idempotency-token-mismatch|run#9|AssertionError|raise AssertionError(
Issue-Key: s3.rename-object-source-if-none-match-precondition-rejected|_create_directory_bucket#1|AssertionError|raise AssertionError(f"S3 Express directory buckets unavailable in this environment: {last_err}")
Issue-Key: s3.s3-express-append-produces-the-exact-concatenation-of-three-offset-writes|run#8|AssertionError|assert bucket is not None, (
Issue-Key: s3.s3x-s3express-session-and-listing-inflight-mpu-prefix-listed|_directory_bucket#2|AssertionError|raise AssertionError(f"could not create a directory bucket in any of {azs!r}: {last}")
Issue-Key: s3.s3x-s3express-session-and-listing-readonly-session-put-denied|_directory_bucket#2|AssertionError|raise AssertionError(f"could not create a directory bucket in any of {azs!r}: {last}")
Issue-Key: s3.s3x-s3express-session-and-listing-readwrite-session-round-trip|_directory_bucket#2|AssertionError|raise AssertionError(f"could not create a directory bucket in any of {azs!r}: {last}")
Issue-Key: s3|GetObject|NoSuchKey
Issue-Key: s3.s3-delete-marker-hides-the-object-while-the-version-still-returns-its-bytes|run#22|AssertionError|assert head_meta["HTTPHeaders"].get("x-amz-delete-marker") == "true", (
Issue-Key: s3|GetObjectTagging|NotImplemented
Issue-Commit: 271fde8cc1a42675cd9f2105c7c7bd008dd933ab

# S3 object plane: 10 root causes — conditional requests, Range, HTTP-date headers, missing VersionId, object lock, archived state, SSE-C, S3 Express, delete markers, versioned tagging

> _Note: this issue was identified against the code base at commit `271fde8cc1a42675cd9f2105c7c7bd008dd933ab`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutObject

## Description
Ten distinct net-new root causes in the S3 object data plane. Each is numbered with its own failing check ids; all ten must be closed before this issue is submitted.

Cause 1 of 10: conditional request variants are scaffold stubs instead of precondition evaluation.
AWS: Smithy s3#PutObject input declares IfMatch (@httpHeader If-Match) and IfNoneMatch (@httpHeader If-None-Match); DeleteObject/GetObject declare the same conditional headers. AWS evaluates them and answers 200 on success, 412 PreconditionFailed on a stale ETag, 404 NoSuchKey for If-Match against an absent key, and 304 Not Modified for a conditional GET that matches. Recorded behavior confirms these codes.
Emulator: every conditional path answers 501 NotImplemented — `operation PutObject with If-Match is scaffolded but not implemented in this emulator`, `operation PutObject with If-None-Match is scaffolded but not implemented in this emulator`; the conditional-GET leg answers 501 where 304 is required, and the If-Match-on-absent-key leg yields code 'NotImplemented' instead of NoSuchKey; a stale If-Match on DeleteObject yields 'NotImplemented' instead of PreconditionFailed.
Why: optimistic-concurrency clients (read ETag, write If-Match) cannot write at all, and an if-none-match=* create-if-absent guard is unusable, so no caller can implement a compare-and-swap against this emulator.
Fix: the PutObject / DeleteObject / GetObject handlers and their precondition evaluation.
Traces: s3.conc-s3-conditional-writes-if-match-on-absent-key-rejected-and-nothing-created, s3.conc-s3-conditional-writes-optimistic-counter-loses-no-increment, s3.conc-s3-conditional-writes-read-after-conditional-overwrite-every-read-path-is-current, s3.put-object-if-match-conditional, s3.s3-delete-object-with-a-stale-if-match-etag-is-refused-and-the-object-survives, s3.s3-if-none-match-star-put-is-atomic-across-a-delete-marker, s3.s3-refused-conditional-gets-create-no-version-and-leave-the-object-intact

Cause 2 of 10: GetObject with a Range header is a scaffold stub.
AWS: Smithy s3#GetObject input declares Range (@httpHeader Range); output declares ContentRange (@httpHeader Content-Range) and AcceptRanges. A satisfiable range answers 206 with Content-Range.
Emulator: `An error occurred (NotImplemented) when calling the GetObject operation: operation GetObject with Range is scaffolded but not implemented in this emulator`.
Why: every ranged/resumable download and multipart-aware reader fails.
Fix: the GetObject handler's range handling and its serializer.
Traces: s3.range-read-open-end

Cause 3 of 10: HTTP-date conditional headers are parsed as RFC-3339, so If-Modified-Since is refused.
AWS: Smithy s3#GetObject declares IfModifiedSince / IfUnmodifiedSince as timestamps bound to the If-Modified-Since / If-Unmodified-Since headers, which are HTTP-dates (RFC 7231 IMF-fixdate).
Emulator: `An error occurred (ValidationException) when calling the GetObject operation: invalid timestamp \`Wed, 01 Jan 2020 00:00:00 GMT\`: invalid date-time: Smithy does not support timezone offsets in RFC-3339 date times` — the header is run through an RFC-3339 parser.
Why: any SDK or browser that sends a cache-validating conditional GET is answered a 400 instead of 200/304.
Fix: the timestamp deserializer for http-date-bound header members on the GetObject/HeadObject input path.
Traces: s3.if-modified-since

Cause 4 of 10: writes to a versioning-enabled bucket return no version id.
AWS: Smithy s3#PutObject output declares VersionId bound to the x-amz-version-id response header; AWS populates it for every write to a versioning-enabled bucket (and for DeleteObject's delete marker).
Emulator: the header is absent — the replayed clients fail with `KeyError: 'VersionId'` immediately after a successful PutObject on a versioned bucket.
Why: a client cannot address the version it just created, so version-scoped retention, legal hold, governance bypass and version-scoped delete are all unreachable even when those features exist.
Fix: the PutObject/DeleteObject handlers' response serialization and the bucket versioning state that assigns version ids.
Traces: s3.deep-s3-immutability-compliance-lock-extends-and-overwrites-but-weakening-rejected, s3.deep-s3-immutability-governance-bypass-without-permission-denied, s3.deep-s3-immutability-legal-hold-blocks-bypass-independent-of-retention-rejected, s3.delete-object-under-legal-hold-rejected, s3.delete-object-under-retention-rejected, s3.object-retention-governance-blocks-delete, s3.object-retention-governance-bypass-delete, s3.object-retention-until-date-enforces-window

Cause 5 of 10: CreateBucket with ObjectLockEnabledForBucket does not implicitly enable versioning, and object-lock ops on an unversioned bucket are not refused.
AWS: creating a bucket with x-amz-bucket-object-lock-enabled: true turns versioning on implicitly; object-lock configuration on a bucket without versioning is refused InvalidBucketState/409.
Emulator: `ObjectLockEnabledForBucket did not implicitly enable versioning`, and the unversioned-bucket refusal is 'NotImplemented' rather than InvalidBucketState.
Why: WORM/compliance setups built the documented way come up without versioning, so retention silently protects nothing.
Fix: the CreateBucket handler's object-lock branch and the bucket-level object-lock validation.
Traces: s3.s3-object-lock-compliance-cannot-be-bypassed-while-governance-can, s3.object-lock-requires-versioning-rejected

Cause 6 of 10: archived objects are served instead of being refused, and restore never changes state.
AWS: Smithy s3#GetObject declares error InvalidObjectState (client, httpError 403); s3#RestoreObject declares ObjectAlreadyInActiveTierError (client, httpError 403). A GET of a GLACIER/DEEP_ARCHIVE object is refused InvalidObjectState until a restore completes.
Emulator: GetObject of an object stored with x-amz-storage-class: GLACIER answers 200 with the body (`expected a ClientError, but the call succeeded ... 'StorageClass': 'GLACIER'`), restore of an already-active object is 'NotImplemented' instead of InvalidObjectState, and restoring a frequent-access Intelligent-Tiering object is 'NotImplemented' instead of InvalidObjectState.
Why: archive-tier workflows cannot be tested at all — the state machine that distinguishes archived, restoring and restored never exists, so a client never sees the 403 it is written to handle.
Fix: the object state/storage-class model and the GetObject and RestoreObject handlers.
Traces: s3.get-object-invalid-object-state-glacier, s3.restore-object-server-side-tier-transition, s3.restore-object-temporary-availability-window, s3.s3-itc-restore-rejected-for-an-object-in-the-frequent-access-tier

Cause 7 of 10: SSE-C (customer-provided keys) is a scaffold stub on every write path, and a new bucket does not block SSE-C.
AWS: Smithy s3#PutObject, CreateMultipartUpload, UploadPartCopy and GetObject all declare SSECustomerAlgorithm / SSECustomerKey / SSECustomerKeyMD5 as request headers and echo SSECustomerAlgorithm / SSECustomerKeyMD5 on output.
Emulator: `operation PutObject with x-amz-server-side-encryption-customer-algorithm is scaffolded but not implemented in this emulator` and `operation CreateMultipartUpload with SSECustomerAlgorithm is scaffolded but not implemented in this emulator`; separately a freshly created bucket reports nothing where SSE-C must be refused (`a new bucket blocks SSE-C by default, got None`).
Why: client-managed-key uploads are impossible, and the policy posture that AWS applies to a new bucket is not reported.
Fix: the PutObject / CreateMultipartUpload / UploadPartCopy handlers' SSE-C branch and the bucket default-encryption state.
Traces: s3.s3-ssec-and-endpoint-enforcement-ssec-multipart-upload-round-trips-only-with-the-customer-key, s3.s3-ssec-and-endpoint-enforcement-ssec-upload-part-copy-rekeys-an-ssec-source-into-a-multipart-object, s3.s3-ssec-and-endpoint-enforcement-website-endpoint-serving-an-ssec-object-rejected, s3.sseinv-new-bucket-default-is-sse-s3-with-bucket-key-off-and-sse-c-blocked

Cause 8 of 10: S3 Express One Zone directory buckets do not exist — CreateSession answers 404.
AWS: Smithy s3#CreateSession is bound to GET /{Bucket}?session, returns required Credentials, and declares exactly one error, NoSuchBucket (client, httpError 404) for a bucket that is absent. For a directory bucket that exists it answers 200 with session credentials.
Emulator: every availability zone refuses — `An error occurred (404) when calling the CreateSession operation: Not Found` across usw2-az1/az2/az3 — so no directory bucket can be created or addressed, and CreateSession's own NoSuchBucket contract cannot be observed either.
Why: the whole S3 Express surface (zonal buckets, session auth, append writes, RenameObject) is unreachable; a client gets a bare 404 with no S3 error code.
Fix: the CreateSession handler and directory-bucket (S3 Express) routing/state.
Traces: s3.create-session-no-such-bucket, s3.list-directory-buckets-rejects-regular-bucket-ops, s3.put-object-encryption-type-mismatch-rejected, s3.rename-object-idempotency-token-mismatch, s3.rename-object-source-if-none-match-precondition-rejected, s3.s3-express-append-produces-the-exact-concatenation-of-three-offset-writes, s3.s3x-s3express-session-and-listing-inflight-mpu-prefix-listed, s3.s3x-s3express-session-and-listing-readonly-session-put-denied, s3.s3x-s3express-session-and-listing-readwrite-session-round-trip

Cause 9 of 10: the 404 for a key hidden by a delete marker omits x-amz-delete-marker.
AWS: Smithy s3#GetObject output declares DeleteMarker bound to the x-amz-delete-marker response header; AWS sets it true on the 404 for a key whose current version is a delete marker.
Emulator: the 404 carries only content-type, x-amz-request-id, x-amz-id-2, content-length and date — `the 404 for a key hidden by a delete marker must carry x-amz-delete-marker: true; the response headers were {...}`.
Why: a client cannot distinguish 'deleted (recoverable, prior versions exist)' from 'never existed', which is the only signal that drives undelete.
Fix: the GetObject/HeadObject not-found path serializer.
Traces: s3.mbes-plain-delete-hides-keys-but-keeps-version-history, s3.s3-delete-marker-hides-the-object-while-the-version-still-returns-its-bytes

Cause 10 of 10: GetObjectTagging with a versionId is a scaffold stub.
AWS: s3#GetObjectTagging declares VersionId (@httpQuery versionId); tag sets are per version.
Emulator: `operation GetObjectTagging with versionId is scaffolded but not implemented in this emulator`.
Why: version-scoped tagging cannot be read back, so a per-version tag change cannot be verified.
Fix: the GetObjectTagging handler's versionId branch.
Traces: s3.s3-delete-object-tagging-clears-only-the-addressed-version-tag-set

## Evidence
Cause 1: `botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the PutObject operation (reached max retries: 0): operation PutObject with If-Match is scaffolded but not implemented in this emulator`; `AssertionError: expected NoSuchKey for If-Match on an absent key, got 'NotImplemented'`; `AssertionError: expected PreconditionFailed for a stale If-Match, got 'NotImplemented'`; `AssertionError: expected HTTP 304 Not Modified for an If-None-Match matching the current ETag, got 501`; `operation PutObject with If-None-Match is scaffolded but not implemented in this emulator`; `AssertionError: expected PreconditionFailed for a stale IfMatch on a delete, got 'NotImplemented'`.
Cause 2: `An error occurred (NotImplemented) when calling the GetObject operation: operation GetObject with Range is scaffolded but not implemented in this emulator`. Expected 206 + Content-Range.
Cause 3: `An error occurred (ValidationException) when calling the GetObject operation: invalid timestamp \`Wed, 01 Jan 2020 00:00:00 GMT\`: invalid date-time: Smithy does not support timezone offsets in RFC-3339 date times`. Expected 200/304.
Cause 4: `KeyError: 'VersionId'` on the PutObject response for a versioning-enabled bucket, in 8 separate checks. Expected: x-amz-version-id present.
Cause 5: `AssertionError: ObjectLockEnabledForBucket did not implicitly enable versioning`; `AssertionError: expected InvalidBucketState on an unversioned bucket, got 'NotImplemented'`.
Cause 6: `AssertionError: expected a ClientError but the call succeeded and returned {... 'x-amz-storage-class': 'GLACIER' ...}` (expected InvalidObjectState/403); `AssertionError: expected InvalidObjectState restoring an already-active object, got ('NotImplemented', ...)`; `AssertionError: restoring a non-archived Intelligent-Tiering object must be InvalidObjectState; got 'NotImplemented'`.
Cause 7: `operation PutObject with x-amz-server-side-encryption-customer-algorithm is scaffolded but not implemented in this emulator`; `operation CreateMultipartUpload with SSECustomerAlgorithm is scaffolded but not implemented in this emulator`; `AssertionError: a new bucket blocks SSE-C by default, got None`.
Cause 8: `AssertionError: could not create a directory bucket in any of ['usw2-az1','usw2-az3','usw2-az4']: An error occurred (404) when calling the CreateSession operation: Not Found` (and the same 404 across usw2-az1/az2/az3 in five more checks). Expected 200 with Credentials, or NoSuchBucket/404 with an S3 error code.
Cause 9: `AssertionError: the 404 for a key hidden by a delete marker must carry x-amz-delete-marker: true; the response headers were {'content-type': 'application/xml', 'x-amz-request-id': ..., 'content-length': '229', 'date': ...}`; `AssertionError: the 404 must flag the delete marker in x-amz-delete-marker; headers carry None`.
Cause 10: `operation GetObjectTagging with versionId is scaffolded but not implemented in this emulator`.

## Steps to Reproduce
Boot the emulator and replay the behavioral suite, then re-run exactly the check ids listed under each cause's Traces line (all ids are also in trace_ids). Minimal manual repros: (1) PutObject an object, then PutObject the same key with IfMatch=<etag>; (2) GetObject with Range='bytes=5-'; (3) GetObject with IfModifiedSince set from an HTTP-date; (4) PutBucketVersioning Enabled then PutObject and read x-amz-version-id; (5) CreateBucket with ObjectLockEnabledForBucket=True then GetBucketVersioning; (6) PutObject with StorageClass=GLACIER then GetObject; (7) PutObject with SSECustomerAlgorithm=AES256; (8) CreateSession on any directory bucket name; (9) DeleteObject a key on a versioned bucket then GetObject it and inspect response headers; (10) GetObjectTagging with VersionId.
