Issue-Source: trace-sweep-divergence
Issue-Trace: datasync-s3-to-s3-task-execution-copies-object
Issue-Step: task#12
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: datasync-s3-to-s3-task-execution-copies-object
Issue-Shape-Id: com.amazonaws.datasync#FmrsService
Issue-Key: s3|GetObject|2xx->2xx|$headers.content-type
Issue-Related: 20261113T084741Z-s3-getobject-serves-a-datasync-copied-object-without.md
Issue-Commit: 1e017f94eab8e7d21388033943594495371e1b36

# S3 GetObject serves a DataSync-copied object with a fallback content type and no user metadata

> _Note: this issue was identified against the code base at commit `1e017f94eab8e7d21388033943594495371e1b36`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetObject

## Description
One root cause, grounded in one wire divergence cluster (the only cluster the replay reports as unowned).

=== Cause 1 of 1: an object materialized by a DataSync S3->S3 transfer is stored with no content type and no user-metadata map, so GetObject answers 200 but emits the generic fallback Content-Type and no x-amz-meta-* header ===

AWS: Smithy (services/service-com-amazonaws-s3-amazons3/smithy-model/com-amazonaws-s3-amazons3.json, GetObject) binds GET /{Bucket}/{Key+} with success code 200, the object bytes on the httpPayload streaming member Body, and among its response headers ContentType -> httpHeader "Content-Type" and Metadata -> httpPrefixHeaders "x-amz-meta-" (a map). A GET of an object therefore re-emits the Content-Type that was recorded when the object was stored -- not a fallback chosen by the server -- and re-emits every user-metadata entry stored with the object as an x-amz-meta-<name> header. For the DataSync S3->S3 destination object this is recorded behavior only as far as the DataSync model goes (no DataSync shape describes metadata preservation): the recording shows that after a terminal task execution a GET of the transferred key returns 200 with the source object's content type (text/plain for a text object) and with the source object's x-amz-meta-* entries present (the recording carries an x-amz-meta-last-modified entry whose value is not asserted). The requirement is that both are derived from the stored object -- the content type is whatever the source object was stored with, and each user-metadata name stored on the source appears on the destination; one run's particular metadata value and key name only fill that template.

Emulator: the GET answers 200 with the correct body, but the object behind it has no metadata record. Content-Type comes back as the generic fallback 'application/octet-stream' instead of the stored 'text/plain', and no x-amz-meta-* header is emitted at all (expected x-amz-meta-last-modified present, got absent). Status matches (200 vs 200); only the response headers diverge. Note that the CRC64NVME checksum header that an earlier sweep saw missing at this step is no longer part of this cluster -- the remaining divergence is exactly the content type and the user metadata.

Why: a client that consumes the destination of a transfer reads the object's metadata, not only its bytes. Content-Type application/octet-stream makes an HTTP client or browser download a text object instead of rendering it and makes content-sniffing/dispatch pipelines mis-route it; a dropped x-amz-meta-* entry silently loses application state the producer attached to the object, so a consumer keyed off user metadata sees it as unset rather than as preserved across the copy. Code that asserts a DataSync copy is faithful cannot pass even though the bytes arrived.

Fix: the write path DataSync's transfer phase uses to materialize destination objects into S3 object state, plus the S3 object-state record it writes -- the transfer must carry the source object's content type and its user-metadata map onto the destination object so that S3's GetObject serializer has values to bind to Content-Type and to the x-amz-meta- prefix headers. GetObject's own serializer needs no change beyond emitting prefix headers for metadata that is present.

Related but not covering this: open issue 20261113T084741Z-s3-getobject-serves-a-datasync-copied-object-without.md describes the same fixture step with a wider cause set (content type + user metadata + a missing x-amz-checksum-crc64nvme). The replay report lists this cluster as unowned (owned_by empty) and the checksum header no longer diverges, so this issue records the narrowed, still-open remainder. Open issue 20261110T181940Z-datasync-describetaskexecution-reports-a-no-op-transfer-zero.md owns the control-plane view of the same transfer (task#11) and says nothing about these response headers.

Traces: datasync-s3-to-s3-task-execution-copies-object#task#12

## Evidence
Fixture datasync-s3-to-s3-task-execution-copies-object, step task#12, service s3, operation GetObject, verdict mismatch, owned_by [] (unowned), stub_path empty. Expected status 200, actual status 200.

  $headers.content-type: value_mismatch (expected 'text/plain', got 'application/octet-stream')
  $headers.x-amz-meta-last-modified: missing (expected present, value '<shaped-not-asserted>', got None)

Fixed vs variable: the header NAMES (Content-Type, x-amz-meta-<name>) are fixed requirements; the content-type value and the metadata name/value come from the stored source object, not from this recording -- 'text/plain' and 'last-modified' are this run's filling of that template.

Model references (smithy_query, GetObject): http {method GET, uri /{Bucket}/{Key+}?x-id=GetObject, code 200}; output Body = httpPayload streaming blob; ContentType -> httpHeader 'Content-Type'; Metadata -> httpPrefixHeaders 'x-amz-meta-' (map); errors = [InvalidObjectState(403), NoSuchKey(404)].

## Steps to Reproduce
Replay fixture datasync-s3-to-s3-task-execution-copies-object and inspect step task#12.

Manually: PutObject a small object into a source bucket with Content-Type text/plain and at least one user-metadata entry (e.g. x-amz-meta-last-modified); create an empty destination bucket; CreateLocationS3 for both locations; CreateTask over them; StartTaskExecution and poll DescribeTaskExecution until the execution is terminal; then GetObject the transferred key from the destination bucket. Observe 200 with the correct body but Content-Type application/octet-stream and no x-amz-meta-* response header, instead of the source object's content type and its preserved user metadata.
