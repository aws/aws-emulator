Issue-Source: trace-sweep-divergence
Issue-Trace: s3-copy-object-annotation-directive-values
Issue-Step: task#4
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: s3-copy-object-annotation-directive-values s3-copy-object-legal-hold-status-outside-enum s3-copy-object-lock-mode-outside-the-vocabulary s3-copy-object-server-side-encryption-header-outside-enum list-object-annotations-page-size-must-be-a-positive-integer object-annotation-delete-name-refusals s3-create-multipart-upload-checksum-type-outside-the-enum-rejected s3-create-session-mode-outside-enum s3-list-directory-buckets-continuation-token-length-bounds s3-list-object-versions-optional-attributes-outside-enum s3-tag-character-set-refusals s3-upload-part-number-ten-thousand-boundary
Issue-Shape-Id: com.amazonaws.s3#AmazonS3
Issue-Key: s3|CopyObject|4xx->4xx|$.Error.ArgumentName
Issue-Key: s3|CopyObject|4xx->4xx|$.Error.ArgumentValue
Issue-Key: s3|CopyObject|4xx->4xx|$.Error.Message
Issue-Key: s3|ListObjectAnnotations|4xx->4xx|$.Error.ArgumentName
Issue-Key: s3|ListObjectAnnotations|4xx->4xx|$.Error.ArgumentValue
Issue-Key: s3|ListObjectAnnotations|4xx->4xx|$.Error.Code
Issue-Key: s3|ListObjectAnnotations|4xx->4xx|$.Error.Message
Issue-Key: s3|DeleteObjectAnnotation|4xx->4xx|$.Error.MaxSizeAllowed
Issue-Key: s3|DeleteObjectAnnotation|4xx->4xx|$.Error.Size
Issue-Key: s3|CreateMultipartUpload|4xx->4xx|$.Error.Code
Issue-Key: s3|CreateMultipartUpload|4xx->4xx|$.Error.Message
Issue-Key: s3|CreateSession|4xx->4xx|$.Error.Code
Issue-Key: s3|CreateSession|4xx->4xx|$.Error.Message
Issue-Key: s3|ListDirectoryBuckets|4xx->4xx|$.Error.Code
Issue-Key: s3|ListDirectoryBuckets|4xx->4xx|$.Error.Message
Issue-Key: s3|ListObjectVersions|4xx->4xx|$.Error.ArgumentName
Issue-Key: s3|ListObjectVersions|4xx->4xx|$.Error.ArgumentValue
Issue-Key: s3|ListObjectVersions|4xx->4xx|$.Error.Message
Issue-Key: s3|PutObject|4xx->4xx|$.Error.Code
Issue-Key: s3|PutObject|4xx->4xx|$.Error.Message
Issue-Key: s3|PutObject|4xx->4xx|$.Error.TagValue
Issue-Key: s3|CreateMultipartUpload|4xx->4xx|$.Error.TagKey
Issue-Key: s3|CreateMultipartUpload|4xx->4xx|$.Error.TagValue
Issue-Key: s3|UploadPart|4xx->4xx|$.Error.ArgumentName
Issue-Key: s3|UploadPart|4xx->4xx|$.Error.ArgumentValue
Issue-Related: 20261102T111455Z-s3-refusal-envelopes-omit-aws-contextual-error-members.md
Issue-Commit: 8e0150f548083d70861fe6ec8aea207c225cbc47

# s3 header and query-parameter refusals use the generic constraint-violation envelope instead of S3's own code, message and argument members

> _Note: this issue was identified against the code base at commit `8e0150f548083d70861fe6ec8aea207c225cbc47`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CopyObject

## Description
10 net-new causes sharing ONE fault: when a value arrives in an HTTP header or a query parameter and fails validation, the emulator answers the generic framework envelope — either a Smithy-style constraint sentence ("Value '<v>' at '<member>' failed to satisfy constraint: Member must satisfy enum value set: [...]") or a raw Rust parse/ARN error — instead of the error Code, the fixed S3 message and the <ArgumentName>/<ArgumentValue> (or TagKey/TagValue, Size/MaxSizeAllowed) members that AWS puts in the refusal. The status is already correct in every cause. The S3 Smithy model declares the enums and ranges but no error shapes for these refusals, so the Code/Message pairings are recorded behavior only; each quoted message is the fixed text of its error class and its variable parts come from the request.

Cause 1 of 10 — CopyObject, a copy-directive/enum header outside its vocabulary. AWS: refused 400 with the per-header sentence, e.g. for x-amz-object-annotation-directive 'The annotation directive is not valid. Valid values are COPY and EXCLUDE.' (fixed text; the valid-value list is the enum, not a request value), plus <ArgumentName> = the header name as sent ('x-amz-object-annotation-directive') and <ArgumentValue> = the rejected value as sent. The same shape applies to x-amz-object-lock-legal-hold-status, x-amz-object-lock-mode and x-amz-server-side-encryption. Recorded behavior only. Emulator: 400 whose Message is the Smithy constraint sentence naming the internal member path ('annotationDirective'), with no ArgumentName/ArgumentValue. Why: the CLI and SDKs surface 'which argument' from those members and the sentence leaks the emulator's internal member names, so a caller cannot map the refusal back to the header it sent. Fix: the CopyObject header-validation path and the shared InvalidArgument constructor it calls. Traces: s3-copy-object-annotation-directive-values#task#4, s3-copy-object-legal-hold-status-outside-enum#task#5, s3-copy-object-lock-mode-outside-the-vocabulary#task#3, s3-copy-object-server-side-encryption-header-outside-enum#task#4 (and #task#5).

Cause 2 of 10 — ListObjectAnnotations, a non-integer max-annotation-results. AWS: Smithy ListObjectAnnotations.MaxAnnotationResults is httpQuery 'max-annotation-results', integer, @range min 1 max 1000. A value that is not an integer is refused 400 with Code 'InvalidArgument' and Message 'Provided max-annotation-results not an integer or within integer range' (fixed text), plus ArgumentName = 'max-annotation-results' and ArgumentValue = the value as sent. Emulator: 400 with Code 'ValidationException' and the Rust parse error 'invalid i32 `<value>`: invalid digit found in string', no argument members. Why: ValidationException is not an S3 error code at all — an XML-aware client cannot classify it, and the message exposes the implementation language. Fix: the query-parameter deserializer for this operation must map a parse failure to the operation's InvalidArgument. Traces: list-object-annotations-page-size-must-be-a-positive-integer#task#5.

Cause 3 of 10 — DeleteObjectAnnotation, an annotation name over the length limit. AWS: the refusal carries <Size> = the computed length of the submitted annotation name and <MaxSizeAllowed> = the service limit (512). Both are templated: Size comes from the request, MaxSizeAllowed is the fixed limit. Recorded behavior only. Emulator: the status and code match but both members are absent. Why: a client cannot compute how much to trim without the limit. Fix: the annotation-name length check's error serializer. Traces: object-annotation-delete-name-refusals#task#5.

Cause 4 of 10 — CreateMultipartUpload, x-amz-checksum-type outside the enum. AWS: Smithy ChecksumType is a closed enum [COMPOSITE, FULL_OBJECT]; a value outside it is refused 400 with Code 'InvalidRequest' and Message 'Value for x-amz-checksum-type header is invalid.' (fixed text naming the header). Recorded behavior only. Emulator: 400 with Code 'InvalidArgument' and the Smithy constraint sentence naming 'checksumType'. Why: the code is wrong (InvalidRequest, not InvalidArgument), so a client branching on the code mis-classifies the refusal. Fix: the CreateMultipartUpload checksum-header validation. Traces: s3-create-multipart-upload-checksum-type-outside-the-enum-rejected#task#2.

Cause 5 of 10 — CreateSession, x-amz-create-session-mode outside the enum. AWS: Smithy SessionMode is [ReadOnly, ReadWrite]; a value outside it is refused 400 with Code 'InvalidRequest' and Message 'Invalid header: x-amz-create-session-mode' (fixed text naming the header). Recorded behavior only. Emulator: 400 'InvalidArgument' with the Smithy constraint sentence naming 'sessionMode'. Why/Fix: as Cause 4, in the CreateSession header validation. Traces: s3-create-session-mode-outside-enum#task#3.

Cause 6 of 10 — ListDirectoryBuckets, a continuation token outside its length bounds. AWS: refused 400 with Code 'InvalidRequest' and Message 'Invalid Token' (fixed text). Recorded behavior only. Emulator: 400 'InvalidArgument' with 'The continuation token provided is incorrect'. Why: a paginating client retries on 'incorrect token' but must surface InvalidRequest/'Invalid Token'; the wrong code breaks that branch. Fix: the ListDirectoryBuckets continuation-token validation. Traces: s3-list-directory-buckets-continuation-token-length-bounds#task#4 (and #task#5).

Cause 7 of 10 — ListObjectVersions, an x-amz-optional-object-attributes value outside the enum. AWS: Smithy OptionalObjectAttributes is the single-value enum [RestoreStatus]; a value outside it is refused 400 with Message 'Invalid attribute name specified.' (fixed text), plus ArgumentName = 'x-amz-optional-object-attributes' and ArgumentValue = the rejected value as sent. Recorded behavior only. Emulator: the Smithy constraint sentence naming 'optionalObjectAttributes.1.member', no argument members. Why: the message names an internal list index instead of the header the caller set. Fix: the ListObjectVersions header validation and the shared InvalidArgument constructor. Traces: s3-list-object-versions-optional-attributes-outside-enum#task#6 (and #task#7).

Cause 8 of 10 — PutObject, an x-amz-tagging value containing a character outside the tag charset. AWS: refused 400 with Code 'InvalidTag', Message 'The TagValue you have provided is invalid' (fixed text), plus <TagKey> = the offending tag's key and <TagValue> = its value, both as sent. Recorded behavior only. Emulator: 400 'InvalidArgument' with "The header 'x-amz-tagging' shall be encoded as UTF-8 then URLEncoded URL query parameters without tag name duplicates." and no TagKey/TagValue. Why: the emulator reports a *transport-encoding* problem for a *charset* problem, so a client re-encodes a header that was already correctly encoded. Fix: the inline-tagging header parser must separate 'cannot be decoded' from 'decoded fine but a tag value is illegal'. Traces: s3-tag-character-set-refusals#task#8.

Cause 9 of 10 — CreateMultipartUpload, the same x-amz-tagging charset refusal. AWS: 400 'InvalidTag' / 'The TagValue you have provided is invalid' + TagKey/TagValue from the request. Emulator: 400 'InvalidArgument' with the URL-encoding sentence. Why/Fix: as Cause 8, on the multipart-initiate path (the same parser, reached from a second call site). Traces: s3-tag-character-set-refusals#task#16.

Cause 10 of 10 — UploadPart, a partNumber above the 10000 ceiling. AWS: refused 400 carrying <ArgumentName> = 'partNumber' and <ArgumentValue> = the number as sent. Recorded behavior only. Emulator: status and code match, both members absent. Why: a parallel uploader cannot report which part index it got wrong. Fix: the UploadPart part-number range check's error serializer. Traces: s3-upload-part-number-ten-thousand-boundary#task#3.

Related open issue (does NOT cover these clusters — the replay reports them unowned): 20261102T111455Z-s3-refusal-envelopes-omit-aws-contextual-error-members.md covers the ArgumentName/ArgumentValue gap on a different set of operations and steps.

## Evidence
Replay of simparity-wire-aws-v1 against the emulator; the HTTP status matched in all ten causes.
Representative s3-copy-object-annotation-directive-values#task#4 (CopyObject, expected 400 / actual 400): $.Error.Message value_mismatch (expected 'The annotation directive is not valid. Valid values are COPY and EXCLUDE.', got "Value 'REPLACE' at 'annotationDirective' failed to satisfy constraint: Member must satisfy enum value set: [COPY, EXCLUDE]"); $.Error.ArgumentName missing (expected 'x-amz-object-annotation-directive'); $.Error.ArgumentValue missing (expected 'REPLACE').
Other causes, same shape:
- list-object-annotations-page-size-must-be-a-positive-integer#task#5: Code expected 'InvalidArgument' got 'ValidationException'; Message expected 'Provided max-annotation-results not an integer or within integer range' got 'invalid i32 `ten`: invalid digit found in string'; ArgumentName/ArgumentValue missing.
- object-annotation-delete-name-refusals#task#5: $.Error.Size missing (expected '513'), $.Error.MaxSizeAllowed missing (expected '512').
- s3-create-multipart-upload-checksum-type-outside-the-enum-rejected#task#2: Code expected 'InvalidRequest' got 'InvalidArgument'; Message expected 'Value for x-amz-checksum-type header is invalid.'
- s3-create-session-mode-outside-enum#task#3: Code expected 'InvalidRequest' got 'InvalidArgument'; Message expected 'Invalid header: x-amz-create-session-mode'.
- s3-list-directory-buckets-continuation-token-length-bounds#task#4: Code expected 'InvalidRequest' got 'InvalidArgument'; Message expected 'Invalid Token' got 'The continuation token provided is incorrect'.
- s3-list-object-versions-optional-attributes-outside-enum#task#6: Message expected 'Invalid attribute name specified.'; ArgumentName expected 'x-amz-optional-object-attributes', ArgumentValue expected the sent value — both missing.
- s3-tag-character-set-refusals#task#8 (PutObject) and #task#16 (CreateMultipartUpload): Code expected 'InvalidTag' got 'InvalidArgument'; Message expected 'The TagValue you have provided is invalid'; TagKey/TagValue missing.
- s3-upload-part-number-ten-thousand-boundary#task#3: ArgumentName missing (expected 'partNumber'), ArgumentValue missing (expected '10001').

## Steps to Reproduce
1. Replay fixture s3-copy-object-annotation-directive-values and compare step task#4's body against the recording: status 400 matches, but Error.Message must be the S3 sentence and the body must carry ArgumentName/ArgumentValue naming the header and value sent.
2. Repeat for each fixture#step in trace_ids and for the sibling steps named under each cause's Traces.
3. Assert for each refusal: the recorded Error.Code, the recorded fixed Message text, and the error class's context members (ArgumentName/ArgumentValue, TagKey/TagValue, Size/MaxSizeAllowed) populated from the request. No response may carry a Smithy constraint sentence, an internal member path, or a Rust parse/ARN error string.
