Issue-Source: trace-sweep-divergence
Issue-Trace: s3-mpu-listing-pagination-scoped-by-bucket
Issue-Step: task#6
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: s3-mpu-listing-pagination-scoped-by-bucket
Issue-Shape-Id: com.amazonaws.s3#AmazonS3
Issue-Key: s3|ListMultipartUploads|2xx->2xx|$.ListMultipartUploadsResult.KeyMarker
Issue-Key: s3|ListMultipartUploads|2xx->2xx|$.ListMultipartUploadsResult.UploadIdMarker
Issue-Key: s3|ListMultipartUploads|2xx->2xx|$.ListMultipartUploadsResult.Upload[Key="upload-a.bin"].Initiator.DisplayName
Issue-Key: s3|ListMultipartUploads|2xx->2xx|$.ListMultipartUploadsResult.Upload[Key="upload-a.bin"].Initiator.ID
Issue-Key: s3|ListMultipartUploads|2xx->2xx|$.ListMultipartUploadsResult.Upload[Key="upload-b.bin"].Initiator.DisplayName
Issue-Key: s3|ListMultipartUploads|2xx->2xx|$.ListMultipartUploadsResult.Upload[Key="upload-b.bin"].Initiator.ID
Issue-Key: s3|ListMultipartUploads|2xx->2xx|$.ListMultipartUploadsResult.NextKeyMarker
Issue-Key: s3|ListMultipartUploads|2xx->2xx|$.ListMultipartUploadsResult.Upload[Key="upload-c.bin"].Initiator.DisplayName
Issue-Key: s3|ListMultipartUploads|2xx->2xx|$.ListMultipartUploadsResult.Upload[Key="upload-c.bin"].Initiator.ID
Issue-Key: s3|ListMultipartUploads|2xx->2xx|$.ListMultipartUploadsResult.Upload[Key="upload-d.bin"].Initiator.DisplayName
Issue-Key: s3|ListMultipartUploads|2xx->2xx|$.ListMultipartUploadsResult.Upload[Key="upload-d.bin"].Initiator.ID
Issue-Related: 20261031T220739Z-s3-serializers-omit-modeled-response-members-deletemarkerversionid-nextuploadidmarker.md
Issue-Commit: f0b2a1a4859373de2ddca21e0cf7a4e6a954ae11

# s3 ListMultipartUploads omits its marker elements and the upload Initiator identity, so the listing cannot be paged

> _Note: this issue was identified against the code base at commit `f0b2a1a4859373de2ddca21e0cf7a4e6a954ae11`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListMultipartUploads

## Description
ListMultipartUploads serializes an incomplete ListMultipartUploadsResult: the marker elements are dropped (both the echoed request markers and the continuation markers) and each Upload's Initiator is emitted without a DisplayName.

Cause 1 of 2 — the echoed KeyMarker / UploadIdMarker elements are omitted on an unpaged listing, and Initiator carries no DisplayName.
AWS: Smithy: ListMultipartUploadsOutput (xmlName ListMultipartUploadsResult) declares KeyMarker and UploadIdMarker as output members; S3 always serializes them, empty, when the request supplied no marker — the recorded response contains both as empty elements. Smithy: MultipartUpload$Initiator targets the Initiator structure, whose members are ID ("If the principal is an Amazon Web Services account, it provides the Canonical User ID. If the principal is an IAM User, it provides a user ARN value.") and DisplayName; S3 serializes both for a general purpose bucket. The values are derived from the caller that initiated each upload, not fixed text.
Emulator: KeyMarker and UploadIdMarker are absent from the result, and every Upload's Initiator contains an ID only — DisplayName is never serialized. The emitted Initiator.ID is also not shaped like the recorded AWS value for the initiating principal.
Why: a client that reads response.KeyMarker/UploadIdMarker to seed its own paging state gets null where AWS gives the empty string, and strict XML/SDK consumers that expect the elements to be present see a shape they do not model. A client that displays or audits who started an in-flight upload gets no Initiator.DisplayName at all.
Fix: the ListMultipartUploads response serializer (always emit KeyMarker and UploadIdMarker; serialize the full Initiator structure) and the multipart-upload state that records the initiating principal's id and display name.
Traces: s3-mpu-listing-pagination-scoped-by-bucket#task#6.

Cause 2 of 2 — the continuation markers NextKeyMarker / NextUploadIdMarker are omitted from a TRUNCATED listing.
AWS: Smithy: ListMultipartUploadsOutput members NextKeyMarker and NextUploadIdMarker. When the listing is truncated (IsTruncated true, e.g. because max-uploads is smaller than the number of in-flight uploads), S3 returns both: NextKeyMarker is the key of the last upload returned and NextUploadIdMarker its upload id, and they are the values the client must pass back as key-marker and upload-id-marker to fetch the next page. Both values come from the service's own listing state; the recorded key and upload id are examples of the template, not requirements.
Emulator: returns the truncated page with neither NextKeyMarker nor NextUploadIdMarker (and, as in cause 1, Initiator.DisplayName missing on every Upload).
Why: this breaks pagination outright. A client — including every AWS SDK paginator for ListMultipartUploads — reads NextKeyMarker/NextUploadIdMarker to issue the follow-up request; with both absent it has no cursor, so it either stops after the first page and silently misses the remaining in-flight uploads, or loops re-requesting the same page.
Fix: the ListMultipartUploads handler/serializer — compute and emit NextKeyMarker and NextUploadIdMarker whenever the result is truncated.
Traces: s3-mpu-listing-pagination-scoped-by-bucket#task#7.

## Evidence
Fixture s3-mpu-listing-pagination-scoped-by-bucket, replayed against the emulator (service s3), ListMultipartUploads, status 200 -> 200:

- task#6: $.ListMultipartUploadsResult.KeyMarker: missing (expected '', got None); $.ListMultipartUploadsResult.UploadIdMarker: missing (expected '', got None); for each Upload (upload-a.bin, upload-b.bin) Initiator.DisplayName: missing (expected a shaped value, got None) and Initiator.ID value_mismatch against the recorded shaped value (emulator emitted a 64-hex canonical-id-like string for every upload).
- task#7 (truncated page): $.ListMultipartUploadsResult.NextKeyMarker: missing (expected the last returned key, e.g. 'upload-d.bin', got None); $.ListMultipartUploadsResult.NextUploadIdMarker: missing (expected that upload's id, got None); Initiator.DisplayName missing and Initiator.ID mismatched for all four Uploads (upload-a.bin .. upload-d.bin).

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus.
2. Fixture s3-mpu-listing-pagination-scoped-by-bucket steps task#6 and task#7 diverge as above.
3. By hand: create a bucket, initiate several multipart uploads, then GET /<bucket>?uploads — the result has no KeyMarker/UploadIdMarker elements and no Initiator/DisplayName. Repeat with &max-uploads=<n> smaller than the number of uploads: IsTruncated is reported but neither NextKeyMarker nor NextUploadIdMarker is present, so there is no cursor for the next page.
