Issue-Source: trace-sweep-divergence
Issue-Trace: emr-run-job-flow-step-writes-s3-output
Issue-Step: task#24
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: emr-run-job-flow-step-writes-s3-output
Issue-Shape-Id: com.amazonaws.emr#ElasticMapReduce
Issue-Key: s3|GetObject|2xx->2xx|$headers.x-amz-checksum-crc64nvme
Issue-Related: 20261113T184807Z-s3-getobject-serves-a-datasync-copied-object-with.md
Issue-Commit: dcecf1852a9f86e80d5d906dc10b7044742585db

# s3 GetObject serves an EMR step's S3 output object with a fallback content type and no CRC64NVME checksum header

> _Note: this issue was identified against the code base at commit `dcecf1852a9f86e80d5d906dc10b7044742585db`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetObject

## Description
One root cause, grounded in one wire divergence cluster (the only unowned cluster in this replay).

=== Cause 1 of 1: an object materialized by an EMR step's S3 write is stored without its content type or full-object checksum, so GetObject answers 200 but reports Content-Type 'application/octet-stream' and omits x-amz-checksum-crc64nvme ===

AWS: Smithy (services/service-com-amazonaws-s3-amazons3/smithy-model/com-amazonaws-s3-amazons3.json, GetObject) binds GET /{Bucket}/{Key+} with success code 200, the object bytes on the httpPayload streaming member Body, and among its response headers ContentType -> httpHeader 'Content-Type' and ChecksumCRC64NVME -> httpHeader 'x-amz-checksum-crc64nvme', with httpChecksum.responseAlgorithms including CRC64NVME (requestValidationModeMember ChecksumMode). A GET therefore reports the Content-Type that S3 recorded for the object when it was stored, and the object's full-object checksum header for an object stored with a CRC64NVME. For an object written with no explicit Content-Type (which is how a Hadoop/Spark step's output part files are written), S3's recorded content type is 'binary/octet-stream' — that is S3's own default for an upload that supplies no Content-Type, not the generic HTTP fallback 'application/octet-stream' — and S3 computes and stores a CRC64NVME full-object checksum for the upload, which GetObject emits on x-amz-checksum-crc64nvme. Here 'binary/octet-stream' is fixed text: it is S3's default value, not a value from this run. The checksum header's VALUE is derived from the stored object's bytes (the recording shows 'exstQHcXHaI=' for its bytes); the requirement is that the header is present and computed from the stored object, never that it equals one run's literal.

Emulator: the GET answers 200 with the object present, but the stored object carries no content type of its own and no computed checksum: Content-Type comes back as the generic fallback 'application/octet-stream' instead of S3's default 'binary/octet-stream', and x-amz-checksum-crc64nvme is absent entirely even though a CRC64NVME was computable from the stored bytes. Status, body and every other asserted header match.

Why: a client that reads a job's output out of S3 keys off these headers. The content type S3 reports for a default upload is 'binary/octet-stream', so code or a test that round-trips an object and asserts its stored content type sees a value S3 never reports, and content-type-driven routing/sniffing pipelines mis-classify the object. The missing x-amz-checksum-crc64nvme means a reader that requests ChecksumMode ENABLED to verify integrity of a step's output has nothing to verify against, so the verification either errors or silently degrades to no integrity check — exactly the guarantee the newer S3 default-checksum behaviour is relied on for.

Fix: S3's object state record and the write path that stores an object with no explicit Content-Type (it must record S3's default 'binary/octet-stream' rather than leaving the field unset for the response serializer to fall back on), plus the same write path's checksum computation so a CRC64NVME is stored for the upload; GetObject's response serializer then has values to bind to Content-Type and x-amz-checksum-crc64nvme and needs no change beyond emitting headers for metadata that is present. The EMR step execution path that materializes the output object must go through that same S3 write path.

Related but not covering this: open issue 20261113T084741Z-s3-getobject-serves-a-datasync-copied-object-without.md and 20261113T184807Z-s3-getobject-serves-a-datasync-copied-object-with.md describe the same family of symptom for an object materialized by a DataSync transfer (their expected content type is the SOURCE object's text/plain, preserved by the copy), and 20261113T174343Z-s3-getobject-of-the-appflow-written-destination-object.md covers an AppFlow-written destination. None covers an object written by an EMR step, none covers S3's default 'binary/octet-stream' content type for an upload that supplies no Content-Type, and the replay report lists this cluster as unowned.

Traces: emr-run-job-flow-step-writes-s3-output#task#24

## Evidence
Fixture emr-run-job-flow-step-writes-s3-output, step task#24, operation GetObject, service s3, verdict mismatch. Expected status 200, actual status 200 (status and body match; response headers diverge). divergence_key: service=s3|GetObject|status=200->200|keys=$headers.content-type,$headers.x-amz-checksum-crc64nvme

  $headers.content-type: value_mismatch (expected 'binary/octet-stream', got 'application/octet-stream')
  $headers.x-amz-checksum-crc64nvme: missing (expected a CRC64NVME value — the recording shows 'exstQHcXHaI=' for its bytes; the requirement is the header, computed from the stored object)

Model references: GetObject http {method: GET, uri: /{Bucket}/{Key+}?x-id=GetObject, code: 200}; Body = httpPayload streaming blob; ContentType -> httpHeader Content-Type; ChecksumCRC64NVME -> httpHeader x-amz-checksum-crc64nvme; ChecksumType -> httpHeader x-amz-checksum-type; httpChecksum {requestValidationModeMember: ChecksumMode, responseAlgorithms: [CRC64NVME, CRC32, CRC32C, SHA256, SHA1, SHA512, MD5, XXHASH64, XXHASH3, XXHASH128]}; errors = [InvalidObjectState(403), NoSuchKey(404)].

## Steps to Reproduce
Replay fixture emr-run-job-flow-step-writes-s3-output and inspect step task#24.

Manually: create an S3 bucket for job output; RunJobFlow a cluster whose single step writes output to an s3:// prefix in that bucket (no explicit Content-Type on the write); poll DescribeStep/ListSteps until the step is terminal; then GetObject the written output key. Observe 200 with the correct bytes but 'Content-Type: application/octet-stream' and no 'x-amz-checksum-crc64nvme' response header, instead of S3's default 'binary/octet-stream' and the object's stored CRC64NVME full-object checksum (also observable by re-issuing the GET with x-amz-checksum-mode: ENABLED).
