Issue-Source: trace-sweep-divergence
Issue-Trace: s3-origin-header-on-signed-requests-to-a-bucket-without-cors
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: s3-origin-header-on-signed-requests-to-a-bucket-without-cors
Issue-Shape-Id: com.amazonaws.s3#AmazonS3
Issue-Key: s3|GetObject|2xx->4xx|$headers.x-amz-checksum-crc32
Issue-Key: s3|ListObjectsV2|2xx->4xx|$.ListBucketResult
Issue-Key: s3|ListObjectsV2|2xx->4xx|status
Issue-Key: s3|GetObjectTagging|2xx->4xx|$.Tagging
Issue-Key: s3|GetObjectTagging|2xx->4xx|status
Issue-Key: s3|CreateMultipartUpload|2xx->4xx|$.InitiateMultipartUploadResult
Issue-Key: s3|CreateMultipartUpload|2xx->4xx|$headers.x-amz-server-side-encryption
Issue-Key: s3|CreateMultipartUpload|2xx->4xx|status
Issue-Key: s3|GetBucketLocation|2xx->4xx|$.LocationConstraint
Issue-Key: s3|GetBucketLocation|2xx->4xx|status
Issue-Key: s3|GetObject|4xx->4xx|$.Error.Code
Issue-Key: s3|GetObject|4xx->4xx|$.Error.Key
Issue-Key: s3|GetObject|4xx->4xx|$.Error.Message
Issue-Key: s3|GetObject|4xx->4xx|status
Issue-Related: 20261121T075817Z-s3-the-s3files-mount-write-through-never-materialises.md
Issue-Related: 20261128T022809Z-s3-rejects-signed-requests-whose-header-values-carry.md
Issue-Commit: 8e0150f548083d70861fe6ec8aea207c225cbc47

# s3 rejects every signed request that carries an Origin header with 403 InvalidAccessKeyId

> _Note: this issue was identified against the code base at commit `8e0150f548083d70861fe6ec8aea207c225cbc47`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutObject

## Description
9 net-new causes sharing ONE fault: a correctly signed request that carries an Origin header is refused 403 with Code 'InvalidAccessKeyId' and Message 'The AWS Access Key Id you provided does not exist in our records.' AWS treats Origin as an ordinary request header: on a bucket with no CORS configuration it simply omits the Access-Control-* response headers and otherwise serves the request normally, whatever the operation. Origin is not part of the SigV4 signed-header set unless the client signs it, so its presence can never change the credential verdict. The Smithy model says nothing about Origin on any S3 operation, so this is recorded behavior only; the quoted refusal text is AWS's fixed message for a genuinely unknown access key, which is not the case here. Every operation on the bucket is affected, so the fault is in the shared request-authentication path rather than in any one handler.

Cause 1 of 9 — PutObject with an Origin header. AWS: 200 with x-amz-server-side-encryption AES256, an ETag and the requested checksum headers (x-amz-checksum-crc32 plus x-amz-checksum-type FULL_OBJECT). Emulator: 403 InvalidAccessKeyId; none of those headers. Why: a browser-origin upload through a signed URL is impossible and the diagnosis blames the credential, so a client rotates keys instead of dropping the header. Fix: the SigV4 verification path — Origin must be excluded from whatever header set the emulator folds into the credential check (and the CORS evaluation must run after authentication, not as a precondition of it). Traces: s3-origin-header-on-signed-requests-to-a-bucket-without-cors#task#2.

Cause 2 of 9 — GetObject with an Origin header. AWS: 200 serving the object body, with last-modified, ETag, the crc32 checksum headers, AES256, accept-ranges 'bytes' and content-type 'text/plain'. Emulator: 403 InvalidAccessKeyId, content-type application/xml. Why/Fix: as Cause 1, on the object read path. Traces: #task#3.

Cause 3 of 9 — HeadObject with an Origin header. AWS: 200 with last-modified, ETag, AES256, accept-ranges 'bytes' and content-type 'text/plain'. Emulator: 403, content-type application/xml. Why/Fix: as Cause 1. Traces: #task#4.

Cause 4 of 9 — ListObjectsV2 with an Origin header. AWS: 200 with the ListBucketResult listing for the requested prefix. Emulator: 403; no ListBucketResult. Why/Fix: as Cause 1, on a bucket-scoped read. Traces: #task#5.

Cause 5 of 9 — GetObjectTagging with an Origin header. AWS: 200 with the Tagging element (an empty TagSet when the object carries no tags). Emulator: 403. Why/Fix: as Cause 1, on a sub-resource read. Traces: #task#6.

Cause 6 of 9 — CreateMultipartUpload with an Origin header. AWS: 200 with InitiateMultipartUploadResult (Bucket, Key, UploadId) and x-amz-server-side-encryption AES256. Emulator: 403. Why: a browser-driven multipart upload cannot even be initiated. Fix: as Cause 1. Traces: #task#7.

Cause 7 of 9 — GetObject of the completed multipart object with an Origin header. AWS: 200 serving the assembled bytes with accept-ranges, last-modified, ETag, x-amz-checksum-crc64nvme, x-amz-checksum-type FULL_OBJECT, AES256 and content-type 'binary/octet-stream'. Emulator: 403 InvalidAccessKeyId, content-type application/xml. Why/Fix: as Cause 2; recorded separately because it exercises the assembled-object read. Traces: #task#10.

Cause 8 of 9 — GetBucketLocation with an Origin header. AWS: 200 with LocationConstraint naming the bucket's region. Emulator: 403; LocationConstraint absent. Why/Fix: as Cause 1, on a bucket sub-resource. Traces: #task#11.

Cause 9 of 9 — a GetObject for a key that does not exist, with an Origin header. AWS: 404 with Code 'NoSuchKey', Message 'The specified key does not exist.' (fixed text) and <Key> = the requested key. Emulator: 403 with Code 'InvalidAccessKeyId' and the unknown-key message, no Key member. Why: the Origin header masks the real not-found verdict, so a client's 'does this object exist?' probe answers 'your credentials are bad'. Fix: as Cause 1 — once authentication stops consuming Origin, the handler's own NoSuchKey refusal is reached. Traces: #task#12.

Related open issue (does NOT cover these clusters — the replay reports them unowned): 20261126T074828Z-s3-a-presigned-get-head-that-carries-an.md covers a presigned GET/HEAD whose Origin is ALLOWED by a configured CORS policy; here the bucket has NO CORS configuration and the whole signed-request surface, not just GET/HEAD, is refused.

## Evidence
Replay of simparity-wire-aws-v1 against the emulator; fixture s3-origin-header-on-signed-requests-to-a-bucket-without-cors. Every step that carries an Origin header is answered 403 with body {'Error': {'Code': 'InvalidAccessKeyId', 'Message': 'The AWS Access Key Id you provided does not exist in our records.', ...}}.
Representative task#2 (PutObject): status expected 200, got 403; $headers.x-amz-server-side-encryption missing (expected 'AES256'); etag missing; x-amz-checksum-crc32 missing; x-amz-checksum-type missing (expected 'FULL_OBJECT').
Other causes: task#3 GetObject expected 200, got 403 (body replaced by the InvalidAccessKeyId envelope, content-type expected 'text/plain' got 'application/xml'); task#4 HeadObject 200→403; task#5 ListObjectsV2 200→403 ($.ListBucketResult missing); task#6 GetObjectTagging 200→403 ($.Tagging missing); task#7 CreateMultipartUpload 200→403 ($.InitiateMultipartUploadResult missing, x-amz-server-side-encryption missing); task#10 GetObject 200→403 (content-type expected 'binary/octet-stream'); task#11 GetBucketLocation 200→403 ($.LocationConstraint missing, expected the bucket's region); task#12 GetObject expected 404 got 403 — $.Error.Code expected 'NoSuchKey' got 'InvalidAccessKeyId', $.Error.Message expected 'The specified key does not exist.', $.Error.Key missing (expected the requested key).

## Steps to Reproduce
1. Replay fixture s3-origin-header-on-signed-requests-to-a-bucket-without-cors against a bucket with no CORS configuration.
2. Every step that sets an Origin header (task#2..task#12) must be answered exactly as it would be without the header: the writes and reads 200 with their recorded headers and bodies, and task#12's missing key 404 NoSuchKey with the Key member.
3. No step may answer 403 InvalidAccessKeyId. Confirm by replaying the same requests with the Origin header removed: the credential is identical, so only the presence of Origin changes the verdict.
4. Because the bucket has no CORS configuration, no Access-Control-* response header may be emitted either.
