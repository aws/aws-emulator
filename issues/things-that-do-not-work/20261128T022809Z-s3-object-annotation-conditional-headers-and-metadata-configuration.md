Issue-Source: trace-sweep-divergence
Issue-Trace: object-annotation-if-match-wildcard-rejected-and-bare-etag-accepted
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: object-annotation-if-match-wildcard-rejected-and-bare-etag-accepted s3-metadata-create-annotation-state-outside-enum-refused
Issue-Shape-Id: com.amazonaws.s3#AmazonS3
Issue-Key: s3|PutObjectAnnotation|4xx->4xx|$.Error.ArgumentName
Issue-Key: s3|PutObjectAnnotation|4xx->4xx|$.Error.Code
Issue-Key: s3|PutObjectAnnotation|4xx->4xx|$.Error.Message
Issue-Key: s3|PutObjectAnnotation|4xx->4xx|status
Issue-Key: s3|PutObjectAnnotation|2xx->4xx|$headers.etag
Issue-Key: s3|PutObjectAnnotation|2xx->4xx|$headers.x-amz-checksum-crc32
Issue-Key: s3|PutObjectAnnotation|2xx->4xx|$headers.x-amz-checksum-type
Issue-Key: s3|PutObjectAnnotation|2xx->4xx|$headers.x-amz-server-side-encryption
Issue-Key: s3|PutObjectAnnotation|2xx->4xx|status
Issue-Key: s3|GetObjectAnnotation|2xx->4xx|status
Issue-Key: s3|DeleteObjectAnnotation|4xx->4xx|$.Error.ArgumentName
Issue-Key: s3|DeleteObjectAnnotation|4xx->4xx|$.Error.Code
Issue-Key: s3|DeleteObjectAnnotation|4xx->4xx|$.Error.Message
Issue-Key: s3|DeleteObjectAnnotation|4xx->4xx|status
Issue-Key: s3|DeleteObjectAnnotation|2xx->4xx|status
Issue-Key: s3|CreateBucketMetadataConfiguration|4xx->2xx|$
Issue-Key: s3|CreateBucketMetadataConfiguration|4xx->2xx|status
Issue-Key: s3|GetBucketMetadataConfiguration|4xx->2xx|$.Error
Issue-Key: s3|GetBucketMetadataConfiguration|4xx->2xx|status
Issue-Commit: 8e0150f548083d70861fe6ec8aea207c225cbc47

# s3 object-annotation conditional headers and metadata-configuration required fields are not evaluated

> _Note: this issue was identified against the code base at commit `8e0150f548083d70861fe6ec8aea207c225cbc47`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutObjectAnnotation

## Description
7 net-new causes sharing ONE fault: the S3 annotation/metadata surface does not implement its own request validation. The annotation operations treat x-amz-object-if-match as a generic HTTP precondition instead of the operation-specific argument it is (Smithy PutObjectAnnotation.ObjectIfMatch / DeleteObjectAnnotation.ObjectIfMatch, httpHeader 'x-amz-object-if-match'), so a wildcard is answered 412 instead of 400 and a legitimate bare ETag is answered 412 instead of succeeding; and CreateBucketMetadataConfiguration does not enforce the required-field rule that accompanies an ENABLED annotation table. The downstream reads then diverge because the state was never written (or was written when it should not have been). PutObjectAnnotation models InvalidRequest/400 and NoSuchKey/404 and DeleteObjectAnnotation models NoSuchKey/404; neither models a precondition error, and CreateBucketMetadataConfiguration declares errors: [] — so the codes/messages below are recorded behavior only, with each quoted message being the fixed text of its error class and its variable parts coming from the request.

Cause 1 of 7 — PutObjectAnnotation answers 412 for a wildcard x-amz-object-if-match. AWS: this operation does not accept a wildcard in that header; it is refused 400 with Code 'InvalidArgument', Message "We don't accept the provided value of Object-If-Match header for this API" (fixed text) and <ArgumentName> = 'x-amz-object-if-match'. Emulator: 412 with Code 'PreconditionFailed' and 'At least one of the pre-conditions you specified did not hold', no ArgumentName. Why: 412 tells a client its view of the object is stale and to re-read and retry; 400 tells it the request form is illegal. The emulator sends the client into an unbounded retry loop over a request that can never succeed. Fix: the PutObjectAnnotation handler must validate ObjectIfMatch as an argument before any precondition evaluation. Traces: object-annotation-if-match-wildcard-rejected-and-bare-etag-accepted#task#3.

Cause 2 of 7 — PutObjectAnnotation refuses a matching bare (unquoted) ETag in x-amz-object-if-match. AWS: a bare ETag matching the object's current ETag is accepted and the annotation is written — 200 with the ETag, the requested checksum headers (x-amz-checksum-crc32 and x-amz-checksum-type FULL_OBJECT) and x-amz-server-side-encryption AES256. Emulator: 412 PreconditionFailed; none of those headers. Why: the conditional-write form the API documents is unusable, so no annotation can be written safely. Fix: the ObjectIfMatch comparison must accept both the quoted and the bare ETag spelling. Traces: object-annotation-if-match-wildcard-rejected-and-bare-etag-accepted#task#5.

Cause 3 of 7 — GetObjectAnnotation answers 404 for the annotation Cause 2 should have written. AWS: 200 serving the annotation. Emulator: 404. Why: the read of a conditionally written annotation fails, so the whole write/read round trip is unverifiable. Fix: follows from Cause 2. Traces: object-annotation-if-match-wildcard-rejected-and-bare-etag-accepted#task#6 (and #task#8).

Cause 4 of 7 — DeleteObjectAnnotation answers 412 for a wildcard x-amz-object-if-match. AWS: 400 'InvalidArgument' with the same fixed sentence and ArgumentName 'x-amz-object-if-match'. Emulator: 412 'PreconditionFailed'. Why/Fix: as Cause 1, on the delete path — the same header validated at a second call site. Traces: object-annotation-if-match-wildcard-rejected-and-bare-etag-accepted#task#7.

Cause 5 of 7 — DeleteObjectAnnotation answers 404 for an annotation that should exist. AWS: 204 No Content (Smithy http code 204) once the annotation is removed. Emulator: 404. Why: the delete cannot be confirmed; a cleanup loop reports a failure. Fix: follows from Cause 2 for the state. Traces: object-annotation-if-match-wildcard-rejected-and-bare-etag-accepted#task#9.

Cause 6 of 7 — CreateBucketMetadataConfiguration accepts an ENABLED AnnotationTableConfiguration with no Role. AWS: refused 400 with Code 'InvalidRequest' and Message 'When AnnotationTableConfiguration is set to ENABLED, Role must be specified. Specify a valid Role ARN, and then try again.' (fixed text), and nothing is created. Emulator: 200 with an empty body; the configuration is created. Why: a configuration that cannot work is created, and the caller never learns a role is needed. Fix: the CreateBucketMetadataConfiguration validator must enforce the ENABLED⇒Role conditional-requirement before the state write. Traces: s3-metadata-create-annotation-state-outside-enum-refused#task#3.

Cause 7 of 7 — GetBucketMetadataConfiguration serves the configuration Cause 6 should have rejected. AWS: with no metadata configuration the read is refused 404 with Code 'MetadataConfigurationNotFound', Message 'The metadata configuration was not found' (fixed text) and <BucketName> = the bucket as addressed. Emulator: 200 with no Error element. Why: the read confirms a configuration AWS says does not exist, so an idempotent create-if-absent flow never creates it. Fix: GetBucketMetadataConfiguration's not-found branch; and follows from Cause 6 for the state. Traces: s3-metadata-create-annotation-state-outside-enum-refused#task#4.

## Evidence
Replay of simparity-wire-aws-v1 against the emulator.
Representative object-annotation-if-match-wildcard-rejected-and-bare-etag-accepted#task#3 (PutObjectAnnotation): status expected 400, got 412; $.Error.Code expected 'InvalidArgument' got 'PreconditionFailed'; $.Error.Message expected "We don't accept the provided value of Object-If-Match header for this API" got 'At least one of the pre-conditions you specified did not hold'; $.Error.ArgumentName missing (expected 'x-amz-object-if-match').
Other causes:
- #task#5 (PutObjectAnnotation, bare matching ETag): status expected 200 got 412; $headers.etag, x-amz-checksum-crc32, x-amz-checksum-type ('FULL_OBJECT') and x-amz-server-side-encryption ('AES256') all missing.
- #task#6 and #task#8 (GetObjectAnnotation): status expected 200 got 404.
- #task#7 (DeleteObjectAnnotation, wildcard): status expected 400 got 412 with the same Code/Message/ArgumentName divergence as task#3.
- #task#9 (DeleteObjectAnnotation): status expected 204 got 404.
- s3-metadata-create-annotation-state-outside-enum-refused#task#3 (CreateBucketMetadataConfiguration): status expected 400 got 200; body empty_body — expected Code 'InvalidRequest' with 'When AnnotationTableConfiguration is set to ENABLED, Role must be specified. Specify a valid Role ARN, and then try again.'
- #task#4 (GetBucketMetadataConfiguration): status expected 404 got 200; $.Error missing — expected Code 'MetadataConfigurationNotFound', Message 'The metadata configuration was not found', BucketName = the addressed bucket.

## Steps to Reproduce
1. Replay fixture object-annotation-if-match-wildcard-rejected-and-bare-etag-accepted.
2. task#3 and task#7 must refuse the wildcard x-amz-object-if-match 400 InvalidArgument with the recorded sentence and the ArgumentName member — not 412 PreconditionFailed.
3. task#5 must accept the bare (unquoted) matching ETag and answer 200 with the ETag, checksum and SSE headers; task#6/#task#8 must then read the annotation 200 and task#9 must delete it 204.
4. Replay s3-metadata-create-annotation-state-outside-enum-refused: task#3 must refuse the ENABLED annotation table with no Role 400 InvalidRequest and create nothing; task#4 must then be 404 MetadataConfigurationNotFound with BucketName.
