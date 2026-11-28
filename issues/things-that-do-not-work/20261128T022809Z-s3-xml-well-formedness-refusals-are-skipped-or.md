Issue-Source: trace-sweep-divergence
Issue-Trace: s3-xml-well-formedness-refusals-leave-stored-state
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: s3-xml-well-formedness-refusals-leave-stored-state s3w-complete-multipart-part-checksum-value-mismatch-with-matching-etag
Issue-Shape-Id: com.amazonaws.s3#AmazonS3
Issue-Key: s3|PutBucketTagging|4xx->2xx|$
Issue-Key: s3|PutBucketTagging|4xx->2xx|status
Issue-Key: s3|GetBucketTagging|4xx->2xx|$.Error
Issue-Key: s3|GetBucketTagging|4xx->2xx|status
Issue-Key: s3|CompleteMultipartUpload|4xx->2xx|$.Error
Issue-Key: s3|CompleteMultipartUpload|4xx->2xx|status
Issue-Key: s3|GetObject|4xx->2xx|$
Issue-Key: s3|GetObject|4xx->2xx|$headers.content-type
Issue-Key: s3|GetObject|4xx->2xx|status
Issue-Key: s3|CompleteMultipartUpload|2xx->4xx|$.CompleteMultipartUploadResult
Issue-Key: s3|CompleteMultipartUpload|2xx->4xx|$headers.x-amz-server-side-encryption
Issue-Key: s3|CompleteMultipartUpload|2xx->4xx|status
Issue-Key: s3|HeadObject|4xx->2xx|$headers.content-type
Issue-Key: s3|HeadObject|4xx->2xx|status
Issue-Related: 20261128T022809Z-s3-xml-payload-validation-refusals-answer-malformedxml-instead.md
Issue-Commit: 8e0150f548083d70861fe6ec8aea207c225cbc47

# s3 XML well-formedness refusals are skipped or leak the parser error, and the rejected write mutates stored state

> _Note: this issue was identified against the code base at commit `8e0150f548083d70861fe6ec8aea207c225cbc47`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutBucketTagging

## Description
8 net-new causes sharing ONE fault: an XML request payload that is not well-formed is handled in one of two wrong ways — either accepted with the success status (and the state change applied, or silently not applied), or refused with the emulator's raw XML-parser diagnostic under Code 'ValidationException'. AWS refuses any ill-formed document with 400 Code 'MalformedXML' and Message 'The XML you provided was not well-formed or did not validate against our published schema' (fixed text, no variable part) and leaves stored state untouched. 'ValidationException' is not an S3 error code at all. The Smithy model declares errors: [] on PutBucketTagging, PutObjectTagging, GetBucketTagging and CompleteMultipartUpload, so these codes and messages are recorded behavior only.

Cause 1 of 8 — PutBucketTagging accepts an ill-formed Tagging document. AWS: 400 'MalformedXML' with the schema sentence; the bucket's tag set is unchanged (and stays absent if it was never set). Emulator: 204 No Content with an empty body. Why: the caller believes its tag set was applied. Fix: the bucket-tagging payload must be parsed and rejected before the state write. Traces: s3-xml-well-formedness-refusals-leave-stored-state#task#3.

Cause 2 of 8 — GetBucketTagging answers 200 after the write Cause 1 should have refused. AWS: with no tag set the read is refused 404 with Code 'NoSuchTagSet', Message 'The TagSet does not exist' (fixed text) and <BucketName> = the bucket as addressed. Emulator: 200 with no Error element. Why: the read confirms a tag set AWS says does not exist, so a 'tag if untagged' loop never runs. Fix: GetBucketTagging's not-found branch must answer 404 NoSuchTagSet; and follows from Cause 1 for the state. Traces: s3-xml-well-formedness-refusals-leave-stored-state#task#4.

Cause 3 of 8 — PutBucketTagging leaks the parser error for a mismatched closing tag. AWS: 400 'MalformedXML' with the schema sentence. Emulator: 400 with Code 'ValidationException' and the parser's own text ('xml parse error: ill-formed document: expected `</Key>`, but `</Value>` was found'). Why: the code is not an S3 code, so an XML-aware client cannot classify the refusal, and the message exposes the emulator's parser internals. Fix: the XML deserializer's error mapping — every parse failure must surface as MalformedXML with the fixed sentence. Traces: s3-xml-well-formedness-refusals-leave-stored-state#task#7.

Cause 4 of 8 — PutObjectTagging leaks the parser error for an unterminated entity reference. AWS: 400 'MalformedXML' with the schema sentence. Emulator: 400 'ValidationException' with 'xml parse error: ill-formed document: entity or character reference not closed: `;` not found before end of input'. Why/Fix: as Cause 3, reached from the object-tagging call site. Traces: s3-xml-well-formedness-refusals-leave-stored-state#task#9.

Cause 5 of 8 — CompleteMultipartUpload accepts an ill-formed CompleteMultipartUpload document. AWS: 400 'MalformedXML' with the schema sentence; the upload is NOT completed and no object appears under the key. Emulator: 200. Why: the caller believes the object was assembled from the parts it named, when the service never read that list. Fix: the CompleteMultipartUpload payload parse must gate the completion. Traces: s3-xml-well-formedness-refusals-leave-stored-state#task#16, s3w-complete-multipart-part-checksum-value-mismatch-with-matching-etag#task#4.

Cause 6 of 8 — GetObject serves an object the refused completion of Cause 5 should never have produced. AWS: 404 with Code 'NoSuchKey', Message 'The specified key does not exist.' (fixed text) and <Key> = the requested key, served as application/xml. Emulator: 200 with the object bytes and content-type 'binary/octet-stream'. Why: the rejected multipart completion is observable as a real object, so a failed upload looks successful to every later reader. Fix: follows from Cause 5. Traces: s3-xml-well-formedness-refusals-leave-stored-state#task#17.

Cause 7 of 8 — the subsequent valid CompleteMultipartUpload is then refused 404. AWS: 200 with CompleteMultipartUploadResult (Location, Bucket, Key, ETag, the checksum members and ChecksumType) and x-amz-server-side-encryption AES256. Emulator: 404; no result element. Why: the upload consumed by Cause 5's wrongly accepted completion is gone, so the legitimate retry — the exact recovery path a client takes after a MalformedXML refusal — cannot succeed. Fix: follows from Cause 5 — a refused completion must leave the upload open. Traces: s3-xml-well-formedness-refusals-leave-stored-state#task#18.

Cause 8 of 8 — HeadObject answers 200 for a key whose multipart completion AWS refused. AWS: 404 (the object does not exist), served as application/xml. Emulator: 200 with content-type 'binary/octet-stream'. Why/Fix: as Cause 6, on the HEAD path and from a second fixture (a part-checksum mismatch rather than a malformed document), showing the same 'refused completion still materializes the object' behavior. Traces: s3w-complete-multipart-part-checksum-value-mismatch-with-matching-etag#task#5.

## Evidence
Replay of simparity-wire-aws-v1 against the emulator; fixture s3-xml-well-formedness-refusals-leave-stored-state unless noted.
Representative task#3 (PutBucketTagging): status expected 400, got 204; body empty_body — expected {'Error': {'Code': 'MalformedXML', 'Message': 'The XML you provided was not well-formed or did not validate against our published schema', ...}}, got {}.
Other causes:
- task#4 (GetBucketTagging): status expected 404 got 200; $.Error missing — expected Code 'NoSuchTagSet', Message 'The TagSet does not exist', BucketName = the addressed bucket.
- task#7 (PutBucketTagging): $.Error.Code expected 'MalformedXML' got 'ValidationException'; Message expected the schema sentence, got 'xml parse error: ill-formed document: expected `</Key>`, but `</Value>` was found'.
- task#9 (PutObjectTagging): Code expected 'MalformedXML' got 'ValidationException'; Message got 'xml parse error: ill-formed document: entity or character reference not closed: `;` not found before end of input'.
- task#16 (CompleteMultipartUpload): status expected 400 got 200; $.Error missing (expected MalformedXML).
- task#17 (GetObject): status expected 404 got 200; expected {'Error': {'Code': 'NoSuchKey', 'Message': 'The specified key does not exist.', 'Key': 'parts/joined.txt', ...}}, got the object bytes; content-type expected 'application/xml' got 'binary/octet-stream'.
- task#18 (CompleteMultipartUpload): status expected 200 got 404; $.CompleteMultipartUploadResult missing; x-amz-server-side-encryption missing.
- s3w-complete-multipart-part-checksum-value-mismatch-with-matching-etag#task#5 (HeadObject): status expected 404 got 200; content-type expected 'application/xml' got 'binary/octet-stream'.

## Steps to Reproduce
1. Replay fixture s3-xml-well-formedness-refusals-leave-stored-state.
2. task#3 must refuse the ill-formed Tagging document 400 MalformedXML with the fixed schema sentence and leave the bucket untagged; task#4's GetBucketTagging must then be 404 NoSuchTagSet with BucketName.
3. task#7 and task#9 must answer Code MalformedXML with the fixed schema sentence — no 'ValidationException', and no parser diagnostic in the message.
4. task#16 must refuse the ill-formed CompleteMultipartUpload 400 MalformedXML and leave the upload open; task#17's GetObject must then be 404 NoSuchKey as application/xml; task#18's valid retry must complete 200 with CompleteMultipartUploadResult.
5. Replay s3w-complete-multipart-part-checksum-value-mismatch-with-matching-etag: task#4's completion must be refused and task#5's HeadObject must be 404.
