Issue-Source: trace-sweep-divergence
Issue-Trace: s3-get-object-if-range-validators-and-extreme-byte-ranges
Issue-Step: task#5
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: s3-get-object-if-range-validators-and-extreme-byte-ranges s3-copy-range-header-refusals s3-range-header-outside-object-reads s3-object-key-relative-path-segments
Issue-Shape-Id: com.amazonaws.s3#AmazonS3
Issue-Key: s3|GetObject|2xx->2xx|$
Issue-Key: s3|GetObject|2xx->2xx|$headers.x-amz-checksum-crc32
Issue-Key: s3|GetObject|2xx->2xx|$headers.x-amz-checksum-type
Issue-Key: s3|GetObject|2xx->2xx|status
Issue-Key: s3|CopyObject|4xx->2xx|$.Error
Issue-Key: s3|CopyObject|4xx->2xx|status
Issue-Key: s3|UploadPartCopy|2xx->4xx|$.CopyPartResult
Issue-Key: s3|UploadPartCopy|2xx->4xx|$headers.x-amz-server-side-encryption
Issue-Key: s3|UploadPartCopy|2xx->4xx|status
Issue-Key: s3|ListParts|2xx->2xx|$.ListPartsResult.Part
Issue-Key: s3|ListObjectsV2|2xx->2xx|$.ListBucketResult.Contents[*]
Issue-Key: s3|GetObjectAttributes|4xx->2xx|$.Error
Issue-Key: s3|GetObjectAttributes|4xx->2xx|status
Issue-Key: s3|CopyObject|2xx->4xx|$.CopyObjectResult
Issue-Key: s3|CopyObject|2xx->4xx|$headers.x-amz-server-side-encryption
Issue-Key: s3|CopyObject|2xx->4xx|status
Issue-Key: s3|ListObjectsV2|2xx->2xx|$.ListBucketResult.CommonPrefixes
Issue-Related: 20261127T075544Z-s3-bucket-object-index-is-inconsistent-for-a.md
Issue-Related: 20261128T022809Z-s3-rejects-every-signed-request-that-carries-an.md
Issue-Related: 20261128T022809Z-s3-rejects-signed-requests-whose-header-values-carry.md
Issue-Commit: 8e0150f548083d70861fe6ec8aea207c225cbc47

# s3 Range, If-Range and copy-source resolution diverge, so ranged reads and copies return or refuse the wrong thing

> _Note: this issue was identified against the code base at commit `8e0150f548083d70861fe6ec8aea207c225cbc47`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetObject

## Description
10 net-new causes sharing ONE fault: the byte-range and copy-source header family is mis-evaluated. A Range that AWS ignores is honoured as a partial read, a degenerate range AWS ignores is refused 416, a Range on CopyObject that AWS refuses outright is honoured, x-amz-copy-source-range on UploadPartCopy that AWS honours is refused, partNumber on GetObjectAttributes that AWS refuses is accepted, and a copy source whose key contains relative path segments is not resolved. The Smithy model declares the members (GetObject.Range, UploadPartCopy.CopySourceRange, ObjectKey with @length min 1) but no error shapes for these refusals, so the codes and messages below are recorded behavior only; each quoted message is the fixed text of its error class and its variable parts come from the request or from state.

Cause 1 of 10 — GetObject honours a Range whose If-Range validator does not match. AWS: when If-Range is present and its validator (ETag or last-modified) does NOT match the current object, the Range is ignored and the service answers 200 with the WHOLE object plus its checksum headers (x-amz-checksum-* and x-amz-checksum-type FULL_OBJECT). Emulator: 206 with only the ranged slice and no checksum headers. Why: a resumable downloader relies on 'validator stale ⇒ full body' to restart cleanly; getting 206 it stitches a slice onto a stale prefix and corrupts the file. Fix: the GetObject range evaluation — the If-Range validator must gate whether the Range applies at all. Traces: s3-get-object-if-range-validators-and-extreme-byte-ranges#task#5 (and #task#7, #task#8).

Cause 2 of 10 — GetObject refuses a degenerate suffix range with 416. AWS: a suffix range requesting zero bytes ('bytes=-0') is unsatisfiable as a range and is IGNORED, so the service answers 200 with the whole object, last-modified, ETag, the checksum headers, AES256, accept-ranges 'bytes' and the object's own content-type. Emulator: 416 with Code 'InvalidRange', Message 'The requested range is not satisfiable', <RangeRequested> = the range as sent and <ActualObjectSize> = the object size from state, content-type application/xml. Why: the refusal turns a read that AWS satisfies into a hard error. Fix: the Range parser — a zero-length or otherwise non-applicable range must fall through to the unranged read rather than entering the 416 branch. Traces: s3-get-object-if-range-validators-and-extreme-byte-ranges#task#9.

Cause 3 of 10 — GetObject answers 206 where AWS answers 200 for another extreme range form. AWS: 200 with the full body. Emulator: 206. Why/Fix: as Cause 1 — the same evaluation, recorded separately for a different range form. Traces: s3-get-object-if-range-validators-and-extreme-byte-ranges#task#12.

Cause 4 of 10 — CopyObject accepts a Range header. AWS: CopyObject has no ranged form; a Range header is refused 400 with Code 'InvalidArgument', Message 'RANGE is not supported in Copy!' (fixed text, exclamation included), <ArgumentName> 'Range' and <ArgumentValue> = the range as sent, and no object is created. Emulator: 200 — the copy succeeds. Why: a caller believing it ranged-copied gets a full-object copy under the destination key, so the destination silently holds the wrong bytes. Fix: the CopyObject handler must reject Range before performing the copy. Traces: s3-copy-range-header-refusals#task#4 (and #task#5).

Cause 5 of 10 — UploadPartCopy refuses a valid x-amz-copy-source-range. AWS: 200 with a CopyPartResult (LastModified, ETag) and x-amz-server-side-encryption AES256 — x-amz-copy-source-range is exactly where a byte range IS supported. Emulator: 400; no CopyPartResult. Why: the ranged-copy form of multipart upload is unusable, and together with Cause 4 the two Range-bearing copy operations have swapped verdicts. Fix: the UploadPartCopy handler's copy-source-range parsing. Traces: s3-copy-range-header-refusals#task#7.

Cause 6 of 10 — ListParts omits the parts written before the failing one. AWS: the result lists every uploaded part with its PartNumber, LastModified, ETag and Size. Emulator: a single part is returned instead of the two uploaded. Why: CompleteMultipartUpload is driven from this list, so the upload cannot be completed. Fix: follows from Cause 5 for the missing part; separately the ListParts serializer must emit a list, not a lone element, when more than one part exists. Traces: s3-copy-range-header-refusals#task#9.

Cause 7 of 10 — ListObjectsV2 reports the objects the refused copies of Cause 4 created. AWS: KeyCount and Contents cover only the objects that were actually written. Emulator: KeyCount is higher and Contents holds the extra keys produced by copies AWS refused. Why: a reconciliation or cleanup sweep acts on objects that should not exist. Fix: follows from Cause 4 — a refused copy must not write. Traces: s3-copy-range-header-refusals#task#10.

Cause 8 of 10 — GetObjectAttributes accepts a partNumber parameter. AWS: this operation takes no partNumber; it is refused 400 with Code 'InvalidArgument', Message 'This operation does not accept partNumber' (fixed text) and ArgumentName/ArgumentValue both 'partNumber'. Emulator: 200. Why: a client probing per-part attributes gets whole-object attributes and silently mis-reports per-part sizes. Fix: the GetObjectAttributes query-parameter validation. Traces: s3-range-header-outside-object-reads#task#7.

Cause 9 of 10 — CopyObject cannot resolve a source key containing relative path segments. AWS: an object key is an opaque byte string — segments such as '..' are literal characters, not path navigation — so a copy whose x-amz-copy-source names a key containing them resolves to that exact key and answers 200 with CopyObjectResult (LastModified, ETag, the checksum members) and x-amz-server-side-encryption AES256. Emulator: 404 — the source is not found. Why: any key with a '..' or '.' segment becomes uncopyable, and the 404 wrongly implies the object is absent. Fix: the copy-source parser must not normalize or collapse path segments in the key portion. Traces: s3-object-key-relative-path-segments#task#8.

Cause 10 of 10 — ListObjectsV2 reports an empty bucket for keys containing relative path segments. AWS: KeyCount counts those keys and Contents lists them, and with a delimiter they roll up into the expected CommonPrefixes. Emulator: KeyCount 0 with Contents and CommonPrefixes absent. Why: objects that were stored are invisible to listing, so they can never be enumerated or deleted. Fix: the bucket object index must key on the raw object key without path normalization. Traces: s3-object-key-relative-path-segments#task#11.

## Evidence
Replay of simparity-wire-aws-v1 against the emulator.
Representative s3-get-object-if-range-validators-and-extreme-byte-ranges#task#5 (GetObject): status expected 200, got 206; body expected the whole object, got only the leading slice; $headers.x-amz-checksum-crc32 and $headers.x-amz-checksum-type missing.
Other causes:
- #task#9: status expected 200 got 416, body replaced by {'Error': {'Code': 'InvalidRange', 'Message': 'The requested range is not satisfiable', 'RangeRequested': 'bytes=-0', 'ActualObjectSize': '36', ...}}; last-modified, etag, checksum, SSE and accept-ranges headers missing; content-type expected 'text/plain' got 'application/xml'.
- #task#12: status expected 200 got 206.
- s3-copy-range-header-refusals#task#4 (CopyObject): status expected 400 got 200; $.Error missing — expected Code 'InvalidArgument', Message 'RANGE is not supported in Copy!', ArgumentName 'Range', ArgumentValue the sent range.
- #task#7 (UploadPartCopy): status expected 200 got 400; $.CopyPartResult missing; x-amz-server-side-encryption missing.
- #task#9 (ListParts): $.ListPartsResult.Part type_mismatch — two parts expected, one returned.
- #task#10 (ListObjectsV2): KeyCount expected '2' got '4'; Contents hold extra keys created by the copies that should have been refused.
- s3-range-header-outside-object-reads#task#7 (GetObjectAttributes): status expected 400 got 200; $.Error missing — expected Code 'InvalidArgument', Message 'This operation does not accept partNumber', ArgumentName/ArgumentValue 'partNumber'.
- s3-object-key-relative-path-segments#task#8 (CopyObject): status expected 200 got 404; $.CopyObjectResult and x-amz-server-side-encryption missing.
- #task#11 (ListObjectsV2): KeyCount expected '2' got '0'; Contents and CommonPrefixes missing.

## Steps to Reproduce
1. Replay s3-get-object-if-range-validators-and-extreme-byte-ranges: task#5/#7/#8 must be 200 with the whole body and the checksum headers when the If-Range validator does not match; task#9's zero-length suffix range must be ignored and answered 200, not 416 InvalidRange; task#12 must be 200.
2. Replay s3-copy-range-header-refusals: task#4/#5 must refuse the Range on CopyObject 400 InvalidArgument 'RANGE is not supported in Copy!' and write nothing; task#7's UploadPartCopy with x-amz-copy-source-range must be 200 with CopyPartResult; task#9's ListParts must list both parts; task#10's ListObjectsV2 must show only the two objects actually written.
3. Replay s3-range-header-outside-object-reads: task#7's GetObjectAttributes with partNumber must be refused 400.
4. Replay s3-object-key-relative-path-segments: task#8's CopyObject must resolve the literal key containing '..' segments and answer 200; task#11's ListObjectsV2 must count and list those keys and roll them up into CommonPrefixes.
