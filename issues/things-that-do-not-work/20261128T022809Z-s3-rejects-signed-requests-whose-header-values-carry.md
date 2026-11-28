Issue-Source: trace-sweep-divergence
Issue-Trace: s3-object-header-values-sent-as-iso-8859-1-bytes-or-with-an-interior-tab
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: s3-object-header-values-sent-as-iso-8859-1-bytes-or-with-an-interior-tab
Issue-Shape-Id: com.amazonaws.s3#AmazonS3
Issue-Key: s3|PutObject|2xx->4xx|$headers.content-type
Issue-Key: s3|PutObject|2xx->4xx|$headers.etag
Issue-Key: s3|PutObject|2xx->4xx|$headers.x-amz-checksum-crc32
Issue-Key: s3|PutObject|2xx->4xx|$headers.x-amz-checksum-type
Issue-Key: s3|PutObject|2xx->4xx|$headers.x-amz-server-side-encryption
Issue-Key: s3|PutObject|2xx->4xx|status
Issue-Key: s3|HeadObject|2xx->4xx|$headers.accept-ranges
Issue-Key: s3|HeadObject|2xx->4xx|$headers.content-disposition
Issue-Key: s3|HeadObject|2xx->4xx|$headers.content-type
Issue-Key: s3|HeadObject|2xx->4xx|$headers.etag
Issue-Key: s3|HeadObject|2xx->4xx|$headers.last-modified
Issue-Key: s3|HeadObject|2xx->4xx|$headers.x-amz-meta-note
Issue-Key: s3|HeadObject|2xx->4xx|$headers.x-amz-server-side-encryption
Issue-Key: s3|HeadObject|2xx->4xx|status
Issue-Key: s3|PutObject|2xx->4xx|$headers.x-amz-checksum-crc64nvme
Issue-Key: s3|GetObject|2xx->4xx|$
Issue-Key: s3|GetObject|2xx->4xx|$headers.accept-ranges
Issue-Key: s3|GetObject|2xx->4xx|$headers.content-type
Issue-Key: s3|GetObject|2xx->4xx|$headers.etag
Issue-Key: s3|GetObject|2xx->4xx|$headers.last-modified
Issue-Key: s3|GetObject|2xx->4xx|$headers.x-amz-checksum-type
Issue-Key: s3|GetObject|2xx->4xx|$headers.x-amz-meta-note
Issue-Key: s3|GetObject|2xx->4xx|$headers.x-amz-server-side-encryption
Issue-Key: s3|GetObject|2xx->4xx|status
Issue-Related: 20261121T075817Z-s3-the-s3files-mount-write-through-never-materialises.md
Issue-Commit: 8e0150f548083d70861fe6ec8aea207c225cbc47

# s3 rejects signed requests whose header values carry non-ASCII or tab bytes, answering 403 InvalidAccessKeyId instead of serving them

> _Note: this issue was identified against the code base at commit `8e0150f548083d70861fe6ec8aea207c225cbc47`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutObject

## Description
5 net-new causes sharing ONE fault: a correctly signed request whose header VALUES contain bytes outside printable US-ASCII (an ISO-8859-1 encoded Content-Disposition filename, an RFC 2047 encoded-word in x-amz-meta-*) or an interior horizontal tab is refused 403 with Code 'InvalidAccessKeyId'. AWS canonicalizes such values for SigV4 (trimming only leading/trailing whitespace and collapsing interior runs of SPACE, leaving other bytes as transmitted) and the signature verifies, so the request succeeds and the value is stored and echoed back on the read. The Smithy model carries no constraint on these header members (PutObject's ContentDisposition and Metadata are plain strings), so the behavior is recorded behavior only; the refusal's code and message are AWS's fixed text for a key that is genuinely unknown, which is not the case here.

Cause 1 of 5 — PutObject with an ISO-8859-1 Content-Disposition value and an RFC 2047 x-amz-meta-* value. AWS: 200, with x-amz-server-side-encryption AES256, an ETag, and the checksum headers (x-amz-checksum-crc32 / x-amz-checksum-crc64nvme plus x-amz-checksum-type FULL_OBJECT) for the algorithm the caller requested; no content-type is set on the success response. Emulator: 403 with Code 'InvalidAccessKeyId' and Message 'The AWS Access Key Id you provided does not exist in our records.', a content-type of application/xml, and none of the success headers. Why: the credential is valid — the same credential succeeds on the other steps of the same fixture — so the diagnosis is actively misleading: a client rotates or re-fetches credentials instead of inspecting the header it sent. Any upload whose metadata is not plain ASCII becomes impossible. Fix: the SigV4 canonical-request builder's header-value normalization — it must not reject or alter non-ASCII bytes, and must collapse only interior SPACE runs. Traces: s3-object-header-values-sent-as-iso-8859-1-bytes-or-with-an-interior-tab#task#2 (and #task#4).

Cause 2 of 5 — the HeadObject that reads back the object of Cause 1. AWS: 200 with last-modified, ETag, x-amz-server-side-encryption AES256, accept-ranges 'bytes', content-type 'text/plain', and the user metadata echoed byte-for-byte as sent (the RFC 2047 encoded-word in x-amz-meta-note, and the Content-Disposition with its ISO-8859-1 filename). Emulator: 404, content-type application/xml, none of those headers. Why: the object is absent because Cause 1 never stored it, and a client cannot verify metadata round-tripping. Fix: follows from Cause 1 for the state; separately, HeadObject must echo stored metadata values verbatim rather than re-encoding them. Traces: s3-object-header-values-sent-as-iso-8859-1-bytes-or-with-an-interior-tab#task#3.

Cause 3 of 5 — a HeadObject whose stored x-amz-meta-* value contains an interior tab. AWS: 200 and the metadata value is echoed with the tab preserved. Emulator: 404 with the XML error content-type. Why: tab-bearing metadata is a legal value that becomes unstorable. Fix: as Cause 2. Traces: s3-object-header-values-sent-as-iso-8859-1-bytes-or-with-an-interior-tab#task#5.

Cause 4 of 5 — a second PutObject in the same fixture, refused the same way. AWS: 200 with AES256, ETag, x-amz-checksum-crc64nvme and x-amz-checksum-type FULL_OBJECT, no content-type on success. Emulator: 403 InvalidAccessKeyId with content-type application/xml. Why/Fix: as Cause 1 — recorded separately because it exercises a different checksum algorithm on the write path. Traces: s3-object-header-values-sent-as-iso-8859-1-bytes-or-with-an-interior-tab#task#6.

Cause 5 of 5 — the GetObject that reads the object of Cause 4. AWS: 200 serving the object bytes with accept-ranges, last-modified, ETag, the crc64nvme checksum headers, AES256, the echoed x-amz-meta-note and content-type 'text/plain'. Emulator: 404 with Code 'NoSuchKey', Message 'The specified key does not exist.' and <Key> = the requested key, content-type application/xml. Why: the NoSuchKey refusal is itself correctly shaped, but it is wrong — the key should exist; the whole non-ASCII-metadata round trip is unverifiable. Fix: follows from Cause 1. Traces: s3-object-header-values-sent-as-iso-8859-1-bytes-or-with-an-interior-tab#task#7.

## Evidence
Replay of simparity-wire-aws-v1 against the emulator; fixture s3-object-header-values-sent-as-iso-8859-1-bytes-or-with-an-interior-tab.
Representative task#2 (PutObject): status expected 200, got 403; $headers.x-amz-server-side-encryption missing (expected 'AES256'); $headers.etag missing; $headers.x-amz-checksum-crc32 missing; $headers.x-amz-checksum-type missing (expected 'FULL_OBJECT'); $headers.content-type unexpected_present (expected None, got 'application/xml'). The 403 body carries Code 'InvalidAccessKeyId' / 'The AWS Access Key Id you provided does not exist in our records.' while the same credential succeeds on other steps of the same fixture.
Other causes:
- task#3 (HeadObject): status expected 200 got 404; last-modified, etag, x-amz-server-side-encryption, x-amz-meta-note (expected '=?UTF-8?Q?caf=C3=A9_cr=C3=A8me?='), content-disposition (expected 'attachment; filename="résumé.txt"') and accept-ranges all missing; content-type expected 'text/plain' got 'application/xml'.
- task#5 (HeadObject): status expected 200 got 404; x-amz-meta-note missing (expected the value with its interior tab, 'left\tright').
- task#6 (PutObject): status expected 200 got 403; x-amz-checksum-crc64nvme and x-amz-checksum-type missing; content-type unexpectedly 'application/xml'.
- task#7 (GetObject): status expected 200 got 404 with body {'Error': {'Code': 'NoSuchKey', 'Message': 'The specified key does not exist.', 'Key': 'presigned.txt', ...}} instead of the object bytes.

## Steps to Reproduce
1. Replay fixture s3-object-header-values-sent-as-iso-8859-1-bytes-or-with-an-interior-tab.
2. At task#2 and task#6 the signed PutObject must be answered 200 with the SSE, ETag and checksum headers from the recording and no content-type — not 403 InvalidAccessKeyId.
3. At task#3, task#5 and task#7 the HeadObject/GetObject must be 200 and must echo x-amz-meta-note and content-disposition byte-for-byte as sent, including the RFC 2047 encoded-word, the ISO-8859-1 filename bytes and the interior tab.
4. Confirm the credential itself is not at fault by observing the other steps of the same fixture succeed with it; the divergence must be reproduced by varying only the header value bytes.
