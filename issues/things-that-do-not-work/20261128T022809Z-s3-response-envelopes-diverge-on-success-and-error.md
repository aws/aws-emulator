Issue-Source: trace-sweep-divergence
Issue-Trace: s3-aws-chunked-put-object-with-an-unsigned-payload-and-a-trailing-checksum
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: s3-aws-chunked-put-object-with-an-unsigned-payload-and-a-trailing-checksum s3-abort-multipart-upload-request-payer-header-values s3-delete-marker-version-subresource-writes-refused s3-sse-c-list-parts-on-a-customer-key-multipart-upload s3-upload-part-copy-source-needs-bucket-slash-key s3-upload-part-number-ten-thousand-boundary s3-b2b-summary-inline-tagging-header-mistakes s3-object-tagging-acl-and-torrent s3-unsigned-and-presigned-query-reads-of-existing-and-missing-objects-and-buckets
Issue-Shape-Id: com.amazonaws.s3#AmazonS3
Issue-Key: s3|PutObject|2xx->2xx|$headers.content-type
Issue-Key: s3|ListMultipartUploads|2xx->2xx|$.ListMultipartUploadsResult.NextKeyMarker
Issue-Key: s3|ListMultipartUploads|2xx->2xx|$.ListMultipartUploadsResult.NextUploadIdMarker
Issue-Key: s3|CreateMultipartUpload|2xx->2xx|$headers.x-amz-version-id
Issue-Key: s3|UploadPartCopy|2xx->2xx|$headers.x-amz-version-id
Issue-Key: s3|CompleteMultipartUpload|2xx->2xx|$.CompleteMultipartUploadResult.Location
Issue-Key: s3|GetObjectTagging|4xx->4xx|$.Error.Key
Issue-Key: s3|ListObjectsV2|4xx->4xx|$.Error
Issue-Key: s3|ListObjectsV2|4xx->4xx|$headers.content-type
Issue-Commit: 8e0150f548083d70861fe6ec8aea207c225cbc47

# s3 response envelopes diverge on success and error paths: stray content-type, phantom pagination markers, missing version-id, wrong Location and error members

> _Note: this issue was identified against the code base at commit `8e0150f548083d70861fe6ec8aea207c225cbc47`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutObject

## Description
7 net-new causes sharing ONE fault: the response envelope the emulator builds does not match the one AWS serves, on paths where the status and the substantive result are otherwise right. Headers that AWS omits are emitted, headers AWS sets are absent, a pagination marker is populated on a non-truncated page, an object URL is built in the wrong form, and two error bodies carry the wrong members or the wrong media type. Each is a wire-contract divergence that breaks clients parsing the response rather than the operation's effect.

Cause 1 of 7 — PutObject sets a content-type on a successful write. AWS: a 200 PutObject response has NO body and therefore no content-type header. Recorded behavior only (the Smithy PutObject output binds no content-type member). Emulator: content-type 'application/xml' is present on the 200. Why: a client that treats a content-type as 'there is a document here' tries to parse an empty body and errors out of a successful upload; an aws-chunked upload with a trailing checksum is exactly where that matters. Fix: the PutObject success serializer must not set content-type when it writes no body. Traces: s3-aws-chunked-put-object-with-an-unsigned-payload-and-a-trailing-checksum#task#2 (and #task#5, #task#8).

Cause 2 of 7 — ListMultipartUploads populates NextKeyMarker/NextUploadIdMarker on a page that is not truncated. AWS: when IsTruncated is false the next-marker elements are empty — they are the continuation cursor and there is nothing to continue. Recorded behavior only. Emulator: both carry the last listed upload's key and upload id. Why: a paginating client sees a non-empty cursor, issues another ListMultipartUploads with it and loops forever or re-processes the last page. Fix: the ListMultipartUploads serializer must emit the next-marker elements only when the result is truncated. Traces: s3-abort-multipart-upload-request-payer-header-values#task#8.

Cause 3 of 7 — CreateMultipartUpload omits x-amz-version-id on a versioned bucket. AWS: the response carries x-amz-version-id for the version the upload will produce. Recorded behavior only. Emulator: the header is absent. Why: a client that records the version id at initiate time to address or roll back that version has nothing to record. Fix: the CreateMultipartUpload response serializer on the versioned-bucket path. Traces: s3-delete-marker-version-subresource-writes-refused#task#9.

Cause 4 of 7 — UploadPartCopy omits x-amz-version-id. AWS: the response carries it. Recorded behavior only. Emulator: absent. Why/Fix: as Cause 3, at a second call site on the copy path. Traces: s3-delete-marker-version-subresource-writes-refused#task#12.

Cause 5 of 7 — CompleteMultipartUpload builds Location in the wrong form. AWS: Smithy CompleteMultipartUploadOutput.Location is the object's URL in VIRTUAL-HOSTED form — the bucket as a host-name label, the service/region suffix, and the object key PERCENT-ENCODED (so a '/' inside the key appears as %2F). Emulator: a path-style URL — bucket as the first path segment — with the key's slashes left literal. The host and port themselves are environment-specific and are not the requirement; the virtual-hosted shape and the percent-encoded key are. Why: a client that hands Location straight to an HTTP fetch addresses a different resource once a key contains a '/' or any reserved character. Fix: the CompleteMultipartUpload Location builder — virtual-hosted layout and percent-encode the key. Traces: s3-sse-c-list-parts-on-a-customer-key-multipart-upload#task#7, s3-upload-part-copy-source-needs-bucket-slash-key#task#8, s3-upload-part-number-ten-thousand-boundary#task#6.

Cause 6 of 7 — GetObjectTagging's 404 reports a <Key> that omits the bucket qualifier. AWS: on this route the error envelope's <Key> is the bucket-qualified key as addressed ('<bucket>/<key>'), not the bare object key. Recorded behavior only. Emulator: the bare key. Why: in a cross-bucket sweep the refusal does not say which bucket it came from, so the caller cannot attribute it. Fix: the object-tagging route's error mapper, which must use the addressed resource rather than the parsed key. Traces: s3-b2b-summary-inline-tagging-header-mistakes#task#6, s3-object-tagging-acl-and-torrent#task#13.

Cause 7 of 7 — ListObjectsV2 on a nonexistent bucket serves a JSON error body with no Error element. AWS: 404 with an XML <Error> carrying Code 'NoSuchBucket', Message 'The specified bucket does not exist' (fixed text) and <BucketName> = the bucket as addressed, served as content-type 'application/xml' — every S3 error body is XML. Emulator: the status is right but the body has no Error element and the content-type is 'application/x-amz-json-1.1'. Why: an S3 client parses the body as XML, gets nothing, and reports an unknown failure instead of 'bucket does not exist'. Fix: the error serializer bound to this route is the JSON one; S3 must always serialize refusals as XML with the error's context members. Traces: s3-unsigned-and-presigned-query-reads-of-existing-and-missing-objects-and-buckets#task#6.

Related open issue (does NOT cover Cause 6 — the replay reports this cluster unowned): 20261102T111455Z-s3-refusal-envelopes-omit-aws-contextual-error-members.md covers the case where Key is ABSENT; here it is present with the wrong value.

## Evidence
Replay of simparity-wire-aws-v1 against the emulator; the operation's effect and status matched in every cause.
Representative s3-aws-chunked-put-object-with-an-unsigned-payload-and-a-trailing-checksum#task#2 (PutObject, 200/200): $headers.content-type unexpected_present (expected None, got 'application/xml').
Other causes:
- s3-abort-multipart-upload-request-payer-header-values#task#8 (ListMultipartUploads, 200/200): $.ListMultipartUploadsResult.NextKeyMarker expected '' got the last upload's key; NextUploadIdMarker expected '' got the last upload id.
- s3-delete-marker-version-subresource-writes-refused#task#9 (CreateMultipartUpload, 200/200): $headers.x-amz-version-id missing.
- s3-delete-marker-version-subresource-writes-refused#task#12 (UploadPartCopy, 200/200): $headers.x-amz-version-id missing.
- s3-sse-c-list-parts-on-a-customer-key-multipart-upload#task#7 (CompleteMultipartUpload, 200/200): $.CompleteMultipartUploadResult.Location — the recording is a virtual-hosted URL with the key percent-encoded ('.../vault%2Fassembled.txt'), the emulator returns a path-style URL with the key's '/' literal.
- s3-b2b-summary-inline-tagging-header-mistakes#task#6 (GetObjectTagging, 404/404): $.Error.Key value_mismatch — expected the bucket-qualified key, got the bare key 'reports/summary.json'.
- s3-unsigned-and-presigned-query-reads-of-existing-and-missing-objects-and-buckets#task#6 (ListObjectsV2, 404/404): $.Error missing — expected Code 'NoSuchBucket', Message 'The specified bucket does not exist', BucketName = the addressed bucket; $headers.content-type expected 'application/xml' got 'application/x-amz-json-1.1'.

## Steps to Reproduce
1. Replay s3-aws-chunked-put-object-with-an-unsigned-payload-and-a-trailing-checksum: task#2/#5/#8 must answer 200 with no content-type header.
2. Replay s3-abort-multipart-upload-request-payer-header-values: task#8's ListMultipartUploads must leave NextKeyMarker/NextUploadIdMarker empty while IsTruncated is false.
3. Replay s3-delete-marker-version-subresource-writes-refused: task#9 and task#12 must carry x-amz-version-id.
4. Replay s3-sse-c-list-parts-on-a-customer-key-multipart-upload (task#7), s3-upload-part-copy-source-needs-bucket-slash-key (task#8) and s3-upload-part-number-ten-thousand-boundary (task#6): CompleteMultipartUploadResult.Location must be the virtual-hosted URL form with the key percent-encoded (ignore the environment's host and port).
5. Replay s3-b2b-summary-inline-tagging-header-mistakes (task#6): the 404 Error.Key must be the bucket-qualified key.
6. Replay s3-unsigned-and-presigned-query-reads-of-existing-and-missing-objects-and-buckets (task#6): the 404 must be an XML <Error> with Code NoSuchBucket, the fixed message and BucketName, served as application/xml.
