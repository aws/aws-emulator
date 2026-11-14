Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: s3.select-object-content-compressed-input-json-lines s3.select-object-content-permission-denied s3.select-object-content-progress-and-stats-events-ordering s3.s3-csv-sse-kms-etag-is-not-the-body-md5-while-sse-s3-etag-is s3.s3-csv-sse-kms-object-reports-one-aws-managed-key-and-no-bucket-key s3.s3-sse-kms-object-is-unreadable-while-its-cmk-is-disabled-and-readable-again-after s3.s3unicode-list-objects-url-encoding-is-form-encoded-not-rfc3986 s3.s3unicode-carriage-return-in-a-key-survives-the-list-xml-byte-exact s3.deploy-s3-1-bucket-update-versioning-policy-rename s3.deploy-s3-1-rename-onto-owned-bucket-rejected s3.bucket-intelligent-tiering-archive-tier-minimum-days-rejected s3.s3-metrics-cfg-and-requires-two-predicates-and-rejects-a-lone-tag s3.s3-metrics-cfg-put-rejected-when-the-document-id-differs-from-the-parameter s3.update-object-encryption-error-paths s3.object-put-get-delete s3.get-object-torrent-unsupported-rejected s3.s3-website-hosting-put-rejected-for-an-index-suffix-containing-a-slash s3.sseinv-kms-master-key-id-with-an-aes256-default-rejected
Issue-Shape-Id: com.amazonaws.s3#AmazonS3
Issue-Key: s3.select-object-content-compressed-input-json-lines|run#17|AssertionError|assert plain_code == "MethodNotAllowed", (
Issue-Key: s3.select-object-content-permission-denied|run#25|AssertionError|assert code == "AccessDenied", (
Issue-Key: s3|SelectObjectContent|NotImplemented
Issue-Key: s3.s3-csv-sse-kms-etag-is-not-the-body-md5-while-sse-s3-etag-is|run#11|AssertionError|assert md5 not in (first, second), (
Issue-Key: s3.s3-csv-sse-kms-object-reports-one-aws-managed-key-and-no-bucket-key|run#12|AssertionError|assert key_arn and key_arn.startswith(prefix) and len(key_arn) > len(prefix), (
Issue-Key: s3.s3-sse-kms-object-is-unreadable-while-its-cmk-is-disabled-and-readable-again-after|run#21|Timeout|time.sleep(_POLL_S)
Issue-Key: s3.s3unicode-list-objects-url-encoding-is-form-encoded-not-rfc3986|run#15|AssertionError|assert by_recovered[f"{prefix}a b.txt"] == f"{prefix}a+b.txt", (
Issue-Key: s3.s3unicode-carriage-return-in-a-key-survives-the-list-xml-byte-exact|run#17|AssertionError|assert encoded == [f"{prefix}a%0Db.txt"], (
Issue-Key: s3.deploy-s3-1-bucket-update-versioning-policy-rename|_wait_stack#23|AssertionError|raise AssertionError(
Issue-Key: s3.deploy-s3-1-rename-onto-owned-bucket-rejected|_wait_stack#20|AssertionError|raise AssertionError(
Issue-Key: s3.bucket-intelligent-tiering-archive-tier-minimum-days-rejected|run#5|AssertionError|assert code == "InvalidAccessTier", (
Issue-Key: s3.s3-metrics-cfg-and-requires-two-predicates-and-rejects-a-lone-tag|run#5|AssertionError|assert code == "MalformedXML", (
Issue-Key: s3.s3-metrics-cfg-put-rejected-when-the-document-id-differs-from-the-parameter|run#5|AssertionError|assert code == "MalformedXML", (
Issue-Key: s3|UpdateObjectEncryption|ValidationException
Issue-Key: s3.object-put-get-delete|_assert_error#18|AssertionError|assert err.get("Code") in codes, f"{what}: expected code in {codes}, got {err.get('Code')!r}"
Issue-Key: s3.get-object-torrent-unsupported-rejected|run#9|AssertionError|assert code == "MethodNotAllowed", f"expected MethodNotAllowed, got {code!r}"
Issue-Key: s3.s3-website-hosting-put-rejected-for-an-index-suffix-containing-a-slash|run#9|AssertionError|assert message == _SUFFIX_MSG, (
Issue-Key: s3.sseinv-kms-master-key-id-with-an-aes256-default-rejected|run#8|AssertionError|assert message == _KEY_WITH_AES_MSG, (
Issue-Commit: 271fde8cc1a42675cd9f2105c7c7bd008dd933ab

# S3 remaining divergences: 8 root causes — SelectObjectContent routing, SSE-KMS ETag/key reporting, list URL encoding, CFN bucket updates, tiering/metrics validation, UpdateObjectEncryption union, GetObjectTorrent, refusal messages

> _Note: this issue was identified against the code base at commit `271fde8cc1a42675cd9f2105c7c7bd008dd933ab`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SelectObjectContent

## Description
Eight distinct net-new root causes remaining on S3 after the three sibling issues. Each is numbered with its own failing check ids; all eight must be closed before this issue is submitted.

Cause 1 of 8: SelectObjectContent is not routed, so neither its retirement refusal nor its authorization refusal can be observed.
AWS: Smithy s3#SelectObjectContent is bound to POST /{Bucket}/{Key+}?select&select-type=2 and streams a SelectObjectContentEventStream. For accounts new to the feature AWS refuses it MethodNotAllowed/405; for a caller with no s3:GetObject it is refused AccessDenied/403.
Emulator: the route is missing entirely — `An error occurred (NotImplemented) when calling the SelectObjectContent operation: operation POST /<bucket>/<key> is not implemented in this emulator` — and the two refusal checks see code 'NotImplemented' where MethodNotAllowed and AccessDenied are required.
Why: a client that handles the documented 405 (and falls back to a full GET) instead sees NotImplemented and treats the emulator as broken rather than as the retired-feature case.
Fix: the SelectObjectContent route and handler (including its authorization and retirement refusals).
Traces: s3.select-object-content-compressed-input-json-lines, s3.select-object-content-permission-denied, s3.select-object-content-progress-and-stats-events-ordering

Cause 2 of 8: SSE-KMS objects report an MD5 ETag and no SSEKMSKeyId, and a disabled CMK still serves the object.
AWS: Smithy s3#GetObject/PutObject output declare SSEKMSKeyId (x-amz-server-side-encryption-aws-kms-key-id) and BucketKeyEnabled. For an SSE-KMS object AWS reports the key ARN and an ETag that is NOT the body MD5 (only SSE-S3/plaintext objects have the MD5 ETag); a GET while the CMK is disabled is refused (KMS.DisabledException surfaced as a 400/KMS.*).
Emulator: `under SSE-KMS the ETag must not be the body MD5 26ff05446eefa691a7ebd8e454c790a0; got '26ff05446eefa691a7ebd8e454c790a0'`; `SSEKMSKeyId must be a key ARN in this account and region; got None`; the disabled-CMK check hangs to the 60 s cap.
Why: a client cannot tell which key encrypted an object, and the MD5 ETag reveals that nothing was actually encrypted. Related open issue 20261024T102105Z-kms-s3-s3-never-resolves-or-validates-ssekmskeyid-against-kms.md covers the kms__s3 checks; these s3.* checks are not carried by it.
Fix: the SSE-KMS branch of the PutObject/GetObject/HeadObject handlers — key resolution, ETag derivation and response headers.
Traces: s3.s3-csv-sse-kms-etag-is-not-the-body-md5-while-sse-s3-etag-is, s3.s3-csv-sse-kms-object-reports-one-aws-managed-key-and-no-bucket-key, s3.s3-sse-kms-object-is-unreadable-while-its-cmk-is-disabled-and-readable-again-after

Cause 3 of 8: EncodingType=url in list responses uses RFC-3986 percent-encoding instead of S3's form encoding.
AWS: with EncodingType=url, S3 form-encodes keys in ListObjects/ListObjectsV2/ListObjectVersions XML — a space becomes '+', and a CR becomes %0D.
Emulator: `a space in a key was not encoded as '+': the stored key 'enc/a b.txt' came back as 'enc%2Fa%20b.txt'`; `EncodingType=url did not report the key's CR as %0D: got ['cr%2Fa%0Db.txt']`.
Why: a client that decodes with S3's rule reconstructs the wrong key, so a listed key cannot be fetched.
Fix: the list-objects XML serializer's key encoder.
Traces: s3.s3unicode-list-objects-url-encoding-is-form-encoded-not-rfc3986, s3.s3unicode-carriage-return-in-a-key-survives-the-list-xml-byte-exact

Cause 4 of 8: the CloudFormation AWS::S3::Bucket provider cannot update a bucket in place and cannot empty a bucket on rollback.
AWS: a stack update that changes a bucket's VersioningConfiguration or policy updates the existing bucket and reaches UPDATE_COMPLETE; a rename update that collides with an owned bucket fails and reaches UPDATE_ROLLBACK_COMPLETE.
Emulator: the stack reaches UPDATE_FAILED with `NotImplemented: this emulator does not change the properties of an existing AWS::S3::Bucket in place; the resource was left as it is`, and the rollback leg reaches UPDATE_FAILED with `BucketNotEmpty: The bucket you tried to delete is not empty` instead of UPDATE_ROLLBACK_COMPLETE.
Why: no infrastructure-as-code change to an existing bucket can be exercised, and a failed update leaves the stack wedged rather than rolled back.
Fix: the CloudFormation AWS::S3::Bucket resource provider's update and rollback/delete paths.
Traces: s3.deploy-s3-1-bucket-update-versioning-policy-rename, s3.deploy-s3-1-rename-onto-owned-bucket-rejected

Cause 5 of 8: Intelligent-Tiering and metrics-configuration validation answers NotImplemented instead of the modeled refusal.
AWS: an ARCHIVE_ACCESS tiering with fewer than the minimum days is refused InvalidAccessTier; a metrics-configuration And filter carrying a single predicate is refused MalformedXML; a metrics document whose Id differs from the ?id= query parameter is refused MalformedXML.
Emulator: all three answer 'NotImplemented', so the invalid document is neither stored-and-reported nor refused with the code the client handles.
Why: a configuration AWS would never create is accepted/unreported, and the client sees NotImplemented where a 400 with a code is required.
Fix: validation in the PutBucketIntelligentTieringConfiguration and PutBucketMetricsConfiguration handlers.
Traces: s3.bucket-intelligent-tiering-archive-tier-minimum-days-rejected, s3.s3-metrics-cfg-and-requires-two-predicates-and-rejects-a-lone-tag, s3.s3-metrics-cfg-put-rejected-when-the-document-id-differs-from-the-parameter

Cause 6 of 8: UpdateObjectEncryption rejects a well-formed union, and CopyObject from a deleted key answers NotImplemented.
AWS: the ObjectEncryption union is deserialized from the request body and a well-formed request is accepted; CopyObject whose source key has been deleted is refused NoSuchKey/404.
Emulator: `An error occurred (ValidationException) when calling the UpdateObjectEncryption operation: union 'ObjectEncryption' had no member set` for a request that does set a member, and `CopyObject from deleted key: expected code in ('NoSuchKey',), got 'NotImplemented'`. (The CopyObject stub itself has an issue filed naming services/service-com-amazonaws-s3-amazons3/src/ops/copy_object.rs; this check was not marked as carried by it, and the required behavior here is the NoSuchKey refusal.)
Why: the union deserializer defect makes the operation unusable, and the missing NoSuchKey means a client cannot distinguish a deleted source from a server fault.
Fix: the UpdateObjectEncryption input deserializer (union member binding) and the CopyObject handler's source-resolution path.
Traces: s3.update-object-encryption-error-paths, s3.object-put-get-delete

Cause 7 of 8: GetObjectTorrent answers NotImplemented instead of MethodNotAllowed.
AWS: BitTorrent delivery is retired; Smithy s3#GetObjectTorrent is still bound to GET /{Bucket}/{Key+}?torrent, and AWS refuses it MethodNotAllowed/405.
Emulator: `expected MethodNotAllowed, got 'NotImplemented'`.
Why: the retired-feature signal a client branches on is replaced by a generic 501.
Fix: the GetObjectTorrent handler's refusal.
Traces: s3.get-object-torrent-unsupported-rejected

Cause 8 of 8: two InvalidArgument refusals carry an emulator-authored message where AWS's message is a fixed documented constant.
AWS: PutBucketWebsite with an IndexDocument.Suffix containing a slash is refused InvalidArgument with the documented constant message 'The IndexDocument Suffix is not well formed'; PutBucketEncryption with a KMSMasterKeyID and a non-KMS default algorithm is refused InvalidArgument with the documented constant 'a KMSMasterKeyID is not applicable if the default sse algorithm is not aws:kms or aws:kms:dsse'. The code and status (InvalidArgument/400) already match; the rule producing the message is a fixed AWS constant, not a recorded accident.
Emulator: the message is the emulator's own validator text, e.g. `IndexDocument.Suffix must not be empty and must not include a slash character, got "docs/index.html"` and `ApplyServerSideEncryptionByDefault.KMSMasterKeyID is allowed if and only if SSEAlgorithm is one of ["aws:kms", "aws:kms:dsse"], got Some("AES256")`.
Why: callers and test suites that match on AWS's documented message text do not match.
Fix: the message constants in the PutBucketWebsite and PutBucketEncryption validators.
Traces: s3.s3-website-hosting-put-rejected-for-an-index-suffix-containing-a-slash, s3.sseinv-kms-master-key-id-with-an-aes256-default-rejected

## Evidence
Cause 1: `An error occurred (NotImplemented) when calling the SelectObjectContent operation: operation POST /sim2real-s3select-<id>/ordering.csv is not implemented in this emulator`; `expected AWS to refuse SelectObjectContent with MethodNotAllowed (discontinued for accounts new to the feature), got 'NotImplemented'`; `a caller whose identity policy grants no s3:GetObject must be refused SelectObjectContent with AccessDenied; got code='NotImplemented'`.
Cause 2: `under SSE-KMS the ETag must not be the body MD5 26ff05446eefa691a7ebd8e454c790a0; got '26ff05446eefa691a7ebd8e454c790a0', '26ff05446eefa691a7ebd8e454c790a0'`; `SSEKMSKeyId must be a key ARN in this account and region; got None`; `Error: Check timed out after 60.0s` for the disabled-CMK check.
Cause 3: `a space in a key was not encoded as '+': the stored key 'enc/a b.txt' came back as 'enc%2Fa%20b.txt'`; `EncodingType=url did not report the key's CR as %0D: got ['cr%2Fa%0Db.txt']`.
Cause 4: `stack sim2real-s3d1-<id> reached 'UPDATE_FAILED', expected 'UPDATE_COMPLETE'; failures: [(..., 'UPDATE_FAILED', 'The following resource(s) failed to update: [Bucket]. NotImplemented: this emulator does not change the properties of an existing AWS::S3::Bucket in place; the resource was left as it is.')]`; `stack sim2real-s3d1n-<id> reached 'UPDATE_FAILED', expected 'UPDATE_ROLLBACK_COMPLETE'; failures: [(..., 'The following resource(s) failed to delete: [Bucket]. BucketNotEmpty: The bucket you tried to delete is not empty')]`.
Cause 5: `expected InvalidAccessTier for an ARCHIVE_ACCESS tiering of 30 days, got 'NotImplemented'`; `an And filter carrying a single predicate must be rejected MalformedXML; got 'NotImplemented'`; `a metrics document id differing from the ?id= parameter must be rejected MalformedXML ...; got 'NotImplemented'`.
Cause 6: `An error occurred (ValidationException) when calling the UpdateObjectEncryption operation: union 'ObjectEncryption' had no member set`; `CopyObject from deleted key: expected code in ('NoSuchKey',), got 'NotImplemented'`.
Cause 7: `AssertionError: expected MethodNotAllowed, got 'NotImplemented'`.
Cause 8: `the InvalidArgument refusal must carry the exact message 'The IndexDocument Suffix is not well formed'; got 'IndexDocument.Suffix must not be empty and must not include a slash character, got "docs/index.html"'`; `the InvalidArgument refusal must carry the exact message 'a KMSMasterKeyID is not applicable if the default sse algorithm is not aws:kms or aws:kms:dsse'; got 'ApplyServerSideEncryptionByDefault.KMSMasterKeyID is allowed if and only if SSEAlgorithm is one of ["aws:kms", "aws:kms:dsse"], got Some("AES256")'`.

## Steps to Reproduce
Boot the emulator and replay the behavioral suite, re-running the check ids listed under each cause's Traces line (all ids are in trace_ids). Manual repros: (1) SelectObjectContent against any CSV object; (2) PutObject with ServerSideEncryption=aws:kms then HeadObject and compare ETag to the body MD5 and read SSEKMSKeyId; (3) PutObject key 'enc/a b.txt' then ListObjectsV2 with EncodingType='url'; (4) deploy an AWS::S3::Bucket stack, then update its VersioningConfiguration; (5) PutBucketIntelligentTieringConfiguration with ARCHIVE_ACCESS Days=30, and PutBucketMetricsConfiguration with a single-predicate And filter; (6) UpdateObjectEncryption with one union member set, and CopyObject from a deleted source key; (7) GetObjectTorrent on any key; (8) PutBucketWebsite with IndexDocument.Suffix='docs/index.html', and PutBucketEncryption with AES256 + KMSMasterKeyID.
