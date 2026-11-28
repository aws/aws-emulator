Issue-Source: trace-sweep-divergence
Issue-Trace: s3files-nfs-mount-write-lands-in-bucket
Issue-Step: task#30
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: s3files-nfs-mount-write-lands-in-bucket
Issue-Shape-Id: com.amazonaws.s3files#S3Files
Issue-Key: s3|GetObject|2xx->4xx|$headers.x-amz-meta-file-atime
Issue-Key: s3|GetObject|2xx->4xx|$headers.x-amz-meta-file-btime
Issue-Key: s3|GetObject|2xx->4xx|$headers.x-amz-meta-file-group
Issue-Key: s3|GetObject|2xx->4xx|$headers.x-amz-meta-file-mtime
Issue-Key: s3|GetObject|2xx->4xx|$headers.x-amz-meta-file-owner
Issue-Key: s3|GetObject|2xx->4xx|$headers.x-amz-meta-file-permissions
Issue-Key: s3|GetObject|2xx->4xx|$headers.x-amz-meta-fs-id
Issue-Key: s3|GetObject|2xx->4xx|$headers.x-amz-meta-user-agent
Issue-Related: 20261102T111455Z-s3-scalar-parameter-refusals-leak-smithy-coercion-wording.md
Issue-Related: 20261109T183045Z-s3-getobject-answers-404-nosuchkey-for-the-emr.md
Issue-Commit: 109411407d03b40f255761f7a04c49ae72d75d8c

# s3 GetObject 404 NoSuchKey for an object written through an s3files mount: write-through to the bucket never lands

> _Note: this issue was identified against the code base at commit `109411407d03b40f255761f7a04c49ae72d75d8c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetObject

## Description
Cause 1 of 1: a file written into a mounted s3files file system never becomes an object in the backing S3 bucket — s3.GetObject for that key answers HTTP 404 NoSuchKey indefinitely where AWS answers 200 with the file's bytes and the s3files-written object metadata (fixture s3files-nfs-mount-write-lands-in-bucket, step task#30).
AWS: Smithy (services/service-com-amazonaws-s3-amazons3/smithy-model/com-amazonaws-s3-amazons3.json) models GetObject GET /{Bucket}/{Key+} with success status 200, output Body (@httpPayload, streaming), AcceptRanges (accept-ranges), LastModified (Last-Modified), ETag, ContentType (Content-Type), VersionId (x-amz-version-id), ServerSideEncryption (x-amz-server-side-encryption), ChecksumCRC64NVME (x-amz-checksum-crc64nvme), ChecksumType (x-amz-checksum-type) and Metadata (@httpPrefixHeaders "x-amz-meta-"); client errors NoSuchKey (404) and InvalidObjectState (403). Recorded behavior: after a client mounts an s3files file system and writes a file under the file system's prefix, that file is readable through the S3 API at the corresponding key — GetObject returns 200, the body is exactly the bytes written, `accept-ranges: bytes`, a Last-Modified/ETag/x-amz-version-id for the new object, `x-amz-server-side-encryption: AES256`, a CRC64NVME checksum with `x-amz-checksum-type: FULL_OBJECT`, a Content-Type derived from the file (text/plain for the recorded .txt), and the POSIX/file-system metadata s3files stamps onto the object as user metadata: x-amz-meta-file-owner, x-amz-meta-file-group, x-amz-meta-file-permissions, x-amz-meta-file-btime/atime/mtime, x-amz-meta-fs-id (the writing file system's id with its generation/inode suffix) and x-amz-meta-user-agent: aws-s3-files. The recorded checksum value, version id, timestamps and the specific fs-id are per-run or payload-derived and are not themselves the requirement; the 200, the body equality, and the presence of that header set with the fixed values noted (accept-ranges: bytes, x-amz-checksum-type: FULL_OBJECT, x-amz-server-side-encryption: AES256, x-amz-meta-user-agent: aws-s3-files) are.
Emulator: GetObject answers HTTP 404 with an XML error body `{Error: {Code: 'NoSuchKey', Message: 'The specified key does not exist.', Key: <the requested key>, RequestId: ..., HostId: ...}}` and `content-type: application/xml`; the replay polled for 180 s (13 attempts) and the key never appeared, and none of the expected response headers above were present. Hypothesis: there is no s3files-to-S3 write-through path at all, so nothing the file system receives is ever materialised as an object; in this fixture the write itself is additionally blocked upstream because ssm.SendCommand (step task#28 of the same fixture) refuses the command that performs the mount and write with HTTP 400 — that SSM refusal is filed separately against the `ssm` service.
Why: the whole point of s3files is that file-system writes are S3 objects. A client that writes through the mount and then reads the object with the S3 API — the canonical round-trip, and how every data-landing check, ETL hand-off and lifecycle rule observes the data — gets a hard 404 forever, so it concludes the data was lost. Even where the object did exist, a consumer that reads the POSIX attributes or `x-amz-meta-fs-id` back off the object (permission-preserving restores, provenance checks) finds no metadata to read.
Fix: the s3files data-plane write-through into S3 object state, plus the object-metadata stamping on objects it creates (the s3files file-system write path and the S3 object store it must write into, so s3.GetObject serves the key with the s3files metadata headers and checksum/SSE/accept-ranges projection). Fixing the ssm SendCommand refusal is a prerequisite for this step to even attempt the write.
Traces: s3files-nfs-mount-write-lands-in-bucket#task#30.

## Evidence
Replay of trace set simparity-wire-aws-v1, fixture s3files-nfs-mount-write-lands-in-bucket, step task#30, GetObject [mismatch, status 200 -> 404]:
  status: expected 200, got 404
  $: poll capped at 180s / poll: status 404 not in [200] after 13 attempt(s) — expected the written payload as the body, got {'Error': {'Code': 'NoSuchKey', 'Message': 'The specified key does not exist.', 'Key': '<the requested key>', 'RequestId': ..., 'HostId': ...}}
  $headers.accept-ranges: missing (expected 'bytes')
  $headers.last-modified: missing (expected a timestamp)
  $headers.etag: missing (expected an etag)
  $headers.x-amz-version-id: missing (expected a version id)
  $headers.x-amz-server-side-encryption: missing (expected 'AES256')
  $headers.x-amz-checksum-crc64nvme: missing (expected the object's CRC64NVME, payload-derived)
  $headers.x-amz-checksum-type: missing (expected 'FULL_OBJECT')
  $headers.x-amz-meta-file-owner: missing (expected '0')
  $headers.x-amz-meta-file-group: missing (expected '0')
  $headers.x-amz-meta-file-permissions: missing (expected '0100644')
  $headers.x-amz-meta-file-btime / -atime / -mtime: missing (expected timestamps)
  $headers.x-amz-meta-fs-id: missing (expected '<writing file system id>:<generation>:<inode>')
  $headers.x-amz-meta-user-agent: missing (expected 'aws-s3-files')
  $headers.content-type: value_mismatch (expected 'text/plain', got 'application/xml' — the error body's type)

Upstream in the same fixture: step task#28 ssm.SendCommand (the command that mounts the file system and writes the file) is refused HTTP 400, filed separately against the `ssm` service; steps task#23/task#24 show the s3files control-plane projection defects, filed separately against `s3files`.

Per-run values in the recording (object version id, etag, checksum bytes, timestamps, the concrete fs-id and the key name) are not requirements; the 200 + body equality + the header set, with the fixed values accept-ranges 'bytes', x-amz-checksum-type 'FULL_OBJECT', x-amz-server-side-encryption 'AES256' and x-amz-meta-user-agent 'aws-s3-files', are.

Model: smithy_query operation GetObject on services/service-com-amazonaws-s3-amazons3/smithy-model/com-amazonaws-s3-amazons3.json (output header bindings, Metadata @httpPrefixHeaders 'x-amz-meta-', errors NoSuchKey 404 / InvalidObjectState 403, httpChecksum responseAlgorithms incl. CRC64NVME).

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus (trace set simparity-wire-aws-v1), fixture s3files-nfs-mount-write-lands-in-bucket.
2. The fixture creates a bucket, creates an s3files file system over it, brings up a managed node, and (via ssm.SendCommand, step task#28) mounts the file system and writes a small text file into the mount, reads it back and unmounts.
3. Step task#30: poll s3.GetObject(Bucket=<the backing bucket>, Key=<the file's key under the file system prefix>).
4. Observe HTTP 404 with XML `NoSuchKey` / 'The specified key does not exist.' on every attempt for 180 s, content-type application/xml, and none of the expected response headers. AWS returns 200 with the written bytes, accept-ranges/etag/last-modified/version-id, x-amz-server-side-encryption AES256, a CRC64NVME checksum with x-amz-checksum-type FULL_OBJECT, Content-Type text/plain, and the x-amz-meta-file-*/x-amz-meta-fs-id/x-amz-meta-user-agent metadata s3files stamps on objects it writes.
5. Re-replay this step after the write-through path lands (and after the ssm SendCommand refusal is fixed, which this step depends on).
