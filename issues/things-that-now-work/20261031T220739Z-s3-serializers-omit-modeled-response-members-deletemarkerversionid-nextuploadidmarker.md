Issue-Source: trace-sweep-divergence
Issue-Trace: s3-bucket-lifecycle-and-discovery
Issue-Step: teardown#13
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: s3-bucket-lifecycle-and-discovery s3-multipart-upload-lifecycle s3w-delete-if-match-size-general-purpose
Issue-Shape-Id: com.amazonaws.s3#AmazonS3
Issue-Key: s3|DeleteObjects|2xx->2xx|$.DeleteResult.Deleted[*]
Issue-Key: s3|ListMultipartUploads|2xx->2xx|$.ListMultipartUploadsResult.NextUploadIdMarker
Issue-Key: s3|DeleteObject|5xx->5xx|$.Error.Header
Issue-Commit: d7a2f366149cfae4c7c153e56172220f0cdfc550

# S3 serializers omit modeled response members: DeleteMarkerVersionId, NextUploadIdMarker, Error Header

> _Note: this issue was identified against the code base at commit `d7a2f366149cfae4c7c153e56172220f0cdfc550`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteObjects

## Description
Three net-new root causes, each a response whose status is right but whose XML drops a member AWS populates. A fixer must close all three.

Cause 1 of 3: DeleteObjects omits DeleteMarkerVersionId from a Deleted entry.
AWS: when a multi-object delete removes a key in a versioning-enabled bucket and the delete creates a delete marker, the <Deleted> entry carries DeleteMarker=true AND DeleteMarkerVersionId — the version id of the marker the DELETE created. Smithy: DeleteObjectsOutput.Deleted -> DeletedObjects (xmlFlattened) of DeletedObject {Key, VersionId, DeleteMarker, DeleteMarkerVersionId}, documented as "The version ID of the delete marker created as a result of the DELETE operation"; output xmlName DeleteResult, http code 200.
Emulator: emits the Deleted entry with Key, VersionId and DeleteMarker only — DeleteMarkerVersionId is absent (replay shows the expected entry with DeleteMarkerVersionId vs the actual entry without it).
Why: a caller that must later remove the marker it just created (the standard "empty a versioned bucket" loop, or an undo path) has no id to delete and must re-list versions to find it; a caller reconciling markers it created cannot correlate them at all.
Fix: the DeleteObjects response serializer / the multi-object-delete handler, which must report the created marker's version id on each Deleted entry.
Traces: s3-bucket-lifecycle-and-discovery#teardown#13

Cause 2 of 3: ListMultipartUploads omits NextUploadIdMarker on a truncated listing.
AWS: when IsTruncated is true, the result carries both NextKeyMarker and NextUploadIdMarker; the pair is what the caller passes back as key-marker / upload-id-marker to fetch the next page, and NextUploadIdMarker is required to disambiguate several uploads of the SAME key. Smithy: ListMultipartUploadsOutput.NextUploadIdMarker (target NextUploadIdMarker), output xmlName ListMultipartUploadsResult, input UploadIdMarker is httpQuery "upload-id-marker".
Emulator: the truncated listing comes back with no NextUploadIdMarker element (replay: "$.ListMultipartUploadsResult.NextUploadIdMarker: missing").
Why: a paginating client (every SDK paginator) has no upload-id-marker to send, so when a key has multiple in-flight uploads the next page restarts at the same key and the pagination either loops forever or silently skips uploads.
Fix: the ListMultipartUploads handler's pagination/serialization, which must emit NextUploadIdMarker (the upload id of the last upload on the page) whenever IsTruncated is true.
Traces: s3-multipart-upload-lifecycle#task#7

Cause 3 of 3: the NotImplemented refusal for an unsupported header omits the Header member.
AWS: DeleteObject with x-amz-if-match-size against a general-purpose bucket is refused with HTTP 501 and a flat <Error> envelope whose Code/Message are AWS's NotImplemented wording AND whose Header member names the header that is not supported. Fixed text: the Code and message of the NotImplemented class (status and code match today, so only the member is at issue); variable part: Header is the offending header name taken from the request — here x-amz-if-match-size. Smithy: DeleteObject.IfMatchSize is httpHeader "x-amz-if-match-size" (long) and DeleteObject models no errors, so the 501 envelope's shape is recorded behavior.
Emulator: answers 501 with the Code/Message but no Header element (replay: "$.Error.Header: missing (expected 'x-amz-if-match-size', got None)").
Why: NotImplemented is emitted for any one of several unsupported headers; without the Header member the caller cannot tell WHICH header to drop and cannot retry correctly, and a client that reads Error/Header gets a null.
Fix: the DeleteObject unsupported-header path's error serializer, which must add the Header member carrying the rejected header's name.
Traces: s3w-delete-if-match-size-general-purpose#task#3, s3w-delete-if-match-size-general-purpose#task#4

## Evidence
Cause 1 — s3-bucket-lifecycle-and-discovery#teardown#13, DeleteObjects, status 200 both sides: expected $.DeleteResult.Deleted[1] = {'Key': ..., 'VersionId': ..., 'DeleteMarker': 'true', 'DeleteMarkerVersionId': ...}; actual {'Key': ..., 'VersionId': ..., 'DeleteMarker': 'true'} — DeleteMarkerVersionId absent. divergence_key service=s3|DeleteObjects|status=200->200|keys=$.DeleteResult.Deleted[1].
Cause 2 — s3-multipart-upload-lifecycle#task#7, ListMultipartUploads, status 200 both sides: $.ListMultipartUploadsResult.NextUploadIdMarker missing (expected the last returned upload's id, got None). divergence_key service=s3|ListMultipartUploads|status=200->200|keys=$.ListMultipartUploadsResult.NextUploadIdMarker.
Cause 3 — s3w-delete-if-match-size-general-purpose#task#3 (also task#4), DeleteObject, status 501 both sides: $.Error.Header missing (expected 'x-amz-if-match-size', got None). divergence_key service=s3|DeleteObject|status=501->501|keys=$.Error.Header.
Smithy (com.amazonaws.s3#AmazonS3): DeletedObject members {Key, VersionId, DeleteMarker, DeleteMarkerVersionId}; ListMultipartUploadsOutput members include NextKeyMarker and NextUploadIdMarker; DeleteObject.IfMatchSize is httpHeader x-amz-if-match-size.

## Steps to Reproduce
Replay the wire corpus (replay_wire_corpus) and inspect these steps:
1. s3-bucket-lifecycle-and-discovery step teardown#13 — on a versioning-enabled bucket, POST /{Bucket}?delete with a Delete payload naming a live key. Expect each <Deleted> entry that created a marker to carry DeleteMarker=true and DeleteMarkerVersionId; the emulator omits DeleteMarkerVersionId.
2. s3-multipart-upload-lifecycle step task#7 — GET /{Bucket}?uploads with max-uploads small enough that IsTruncated=true. Expect both NextKeyMarker and NextUploadIdMarker; the emulator omits NextUploadIdMarker.
3. s3w-delete-if-match-size-general-purpose step task#3 (and task#4) — DELETE /{Bucket}/{Key} with the x-amz-if-match-size header on a general-purpose bucket. Expect HTTP 501 with an <Error> envelope whose Header member is 'x-amz-if-match-size'; the emulator returns the 501 without the Header member.
