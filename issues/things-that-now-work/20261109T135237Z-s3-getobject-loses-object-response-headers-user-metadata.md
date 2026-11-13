Issue-Source: trace-sweep-divergence
Issue-Trace: databrew-recipe-job-writes-transformed-csv
Issue-Step: task#12
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: databrew-recipe-job-writes-transformed-csv
Issue-Shape-Id: com.amazonaws.databrew#AWSGlueDataBrew
Issue-Key: s3|GetObject|2xx->2xx|$headers.content-type
Issue-Key: s3|GetObject|2xx->2xx|$headers.x-amz-checksum-crc64nvme
Issue-Key: s3|GetObject|2xx->2xx|$headers.x-amz-meta-job_name
Issue-Key: s3|GetObject|2xx->2xx|$headers.x-amz-meta-job_run_id
Issue-Commit: 00353ccf65811f65ad7f73dbcefa75ea121211d3

# S3 GetObject loses object response headers: user metadata, the default Content-Type and the full-object checksum

> _Note: this issue was identified against the code base at commit `00353ccf65811f65ad7f73dbcefa75ea121211d3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetObject

## Description
Three net-new root causes in the S3 GetObject response headers for an object written with user metadata and no explicit content type (here, the output object a DataBrew recipe job writes). A fixer must close all three. The status matches (200) and the body is served; the object's header-bound metadata is what diverges.

Cause 1 of 3: GetObject returns no x-amz-meta-* user metadata.
AWS: Smithy: GetObjectOutput member Metadata targets com.amazonaws.s3#Metadata and is bound with @httpPrefixHeaders("x-amz-meta-"), so every user-metadata key stored with the object is echoed back on GetObject (and HeadObject) as x-amz-meta-<key>: <value>. The key suffixes and values are whatever the writer stored — in this recording the DataBrew job writer's job_name and job_run_id entries; the requirement is that stored metadata is returned, not these particular keys.
Emulator: both x-amz-meta-job_name and x-amz-meta-job_run_id are absent from the 200 response (expected x-amz-meta-job_name = the writing job's name and x-amz-meta-job_run_id = the writing run's id; the recorded literal 'sim2real-probe-databrew-job-6880360d' is this run's generated job name, not part of the contract).
Why: user metadata is the documented way to attach provenance to an object; a client that reads back which job/run produced an output file, or any application that round-trips metadata through GET -> PUT, loses it entirely and cannot distinguish "no metadata" from "metadata dropped". Note: if the metadata is never stored in the first place, the omission is in the DataBrew recipe-job output writer rather than the GetObject serializer — both ends must be closed so that metadata set at write time is served at read time.
Fix: the S3 GetObject/HeadObject response-header serializer (the httpPrefixHeaders projection of the stored object's metadata map) and the object state that must persist it on write.
Traces: databrew-recipe-job-writes-transformed-csv#task#12.

Cause 2 of 3: the default Content-Type for an object stored without one is wrong.
AWS: recorded behavior only (the model types GetObjectOutput.ContentType as a plain string bound to the Content-Type header and states no default). The recording shows that when an object is stored with no caller-supplied content type, S3 serves it back as 'binary/octet-stream' — S3's own default media type, a fixed string with no variable parts.
Emulator: Content-Type: 'application/octet-stream'.
Why: a client that branches on the served media type, or that compares the GetObject/HeadObject content type against what S3 is known to default to (for example to detect that an upload failed to set a type), reads a value AWS never serves for such an object; proxies and browsers also treat the two strings as distinct.
Fix: the S3 object-write path's default content type and the GetObject/HeadObject header serializer that emits it.
Traces: databrew-recipe-job-writes-transformed-csv#task#12.

Cause 3 of 3: the object's full-object checksum header is not returned.
AWS: Smithy: GetObject carries @httpChecksum with requestValidationModeMember ChecksumMode and responseAlgorithms [CRC64NVME, CRC32, CRC32C, SHA256, SHA1, SHA512, MD5, XXHASH64, XXHASH3, XXHASH128]; GetObjectOutput member ChecksumCRC64NVME is bound to the x-amz-checksum-crc64nvme header. When the request enables checksum validation, S3 returns the header for the algorithm the object was stored with — CRC64NVME is the default full-object algorithm for an upload that names none. The header value is the object's own base64 checksum computed over its bytes (the recorded 'YGSAGf4mKZQ=' is this object's content checksum, not a constant).
Emulator: x-amz-checksum-crc64nvme is absent from the 200 response.
Why: a client that verifies object integrity on download reads the checksum header for the stored algorithm; with it absent the download cannot be verified at all, and the client cannot tell an unchecksummed object from a dropped header. This is the same default-algorithm gap the listing surface shows (ListObjectsV2 reports ChecksumAlgorithm CRC32 where AWS reports CRC64NVME); the open issue 20261030T053639Z-s3-listobjects-and-listobjectsv2-omit-prefix-marker-and.md carries only the ListObjects/ListObjectsV2 body keys and does not carry this GetObject header key, so it does not cover this cause.
Fix: the S3 object-write path's default checksum algorithm (CRC64NVME when the caller names none) and the GetObject/HeadObject checksum-header serializer.
Traces: databrew-recipe-job-writes-transformed-csv#task#12.

## Evidence
Replay verdict 'mismatch', expected_status 200, actual_status 200 (status matches; the response headers diverge).
Fixture databrew-recipe-job-writes-transformed-csv, step task#12, operation GetObject, service s3, cluster size 1.
detail:
  $headers.x-amz-meta-job_name: missing (expected 'sim2real-probe-databrew-job-6880360d', got None)
  $headers.x-amz-meta-job_run_id: missing (expected '<shaped-not-asserted>', got None)
  $headers.content-type: value_mismatch (expected 'binary/octet-stream', got 'application/octet-stream')
  $headers.x-amz-checksum-crc64nvme: missing (expected 'YGSAGf4mKZQ=', got None)
divergence_key: service=s3|GetObject|status=200->200|keys=$headers.content-type,$headers.x-amz-checksum-crc64nvme,$headers.x-amz-meta-job_name,$headers.x-amz-meta-job_run_id
Model evidence: smithy_query operation=GetObject on services/service-com-amazonaws-s3-amazons3/smithy-model/com-amazonaws-s3-amazons3.json — GetObjectOutput.Metadata @httpPrefixHeaders 'x-amz-meta-', GetObjectOutput.ContentType -> Content-Type, GetObjectOutput.ChecksumCRC64NVME -> x-amz-checksum-crc64nvme, httpChecksum.responseAlgorithms leading with CRC64NVME.
Related-but-not-covering open issue: 20261030T053639Z-s3-listobjects-and-listobjectsv2-omit-prefix-marker-and.md (listing-side checksum members only).

## Steps to Reproduce
1. Replay the wire corpus and re-run the binding in Issue-Trace/Issue-Step: databrew-recipe-job-writes-transformed-csv#task#12.
2. By hand: PUT an object with no Content-Type and no x-amz-sdk-checksum-algorithm, supplying two x-amz-meta-<key> headers (equivalently, let a DataBrew recipe job write its output object, which stores job_name/job_run_id metadata).
3. GET that object with x-amz-checksum-mode: ENABLED and inspect the 200 response headers: both x-amz-meta-<key> headers must come back with the stored values, Content-Type must be 'binary/octet-stream', and x-amz-checksum-crc64nvme must carry the object's base64 CRC64NVME checksum.
