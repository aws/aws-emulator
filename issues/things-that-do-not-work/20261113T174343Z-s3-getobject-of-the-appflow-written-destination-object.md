Issue-Source: trace-sweep-divergence
Issue-Trace: appflow-s3-to-s3-flow-run-transfers-records
Issue-Step: task#10
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: appflow-s3-to-s3-flow-run-transfers-records
Issue-Shape-Id: com.amazonaws.appflow#SandstoneConfigurationServiceLambda
Issue-Key: s3|GetObject|2xx->2xx|$headers.x-amz-server-side-encryption-aws-kms-key-id
Issue-Related: 20261109T135237Z-s3-getobject-loses-object-response-headers-user-metadata.md
Issue-Related: 20261109T175957Z-s3-getobject-serves-the-appflow-written-object-with.md
Issue-Commit: 86fef9963b92592df7182a804cee9776b79fbc91

# s3 GetObject of the AppFlow-written destination object diverges in payload byte order, full-object checksum headers and SSE reporting

> _Note: this issue was identified against the code base at commit `86fef9963b92592df7182a804cee9776b79fbc91`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetObject

## Description
Reading back the object an AppFlow S3-to-S3 flow run wrote to its destination bucket answers HTTP 200, but the response body bytes and four response headers diverge from the recorded AWS response. All of it is observed on one replayed step (fixture appflow-s3-to-s3-flow-run-transfers-records, step task#10), which GETs the flow's destination object after a successful run. This cluster is reported UNOWNED by the replay (no open issue carries its replay key, which covers all five diverging paths $, $headers.x-amz-checksum-crc64nvme, $headers.x-amz-checksum-type, $headers.x-amz-server-side-encryption and $headers.x-amz-server-side-encryption-aws-kms-key-id). There are 3 causes in this issue; all three must be closed before the step passes.

Cause 1 of 3: the destination object's JSON-Lines records are re-serialized with their members reordered.
AWS: recorded behavior only. The destination object is JSON Lines - one JSON object per line, newline-terminated - and each line preserves the FIELD ORDER of the source record as it was read from the source object (the recording's lines list each record's members in source order: the id member, then the city member, then the temp member). Smithy (com.amazonaws.s3#GetObject, GET /{Bucket}/{Key+}): output Body -> StreamingBlob @httpPayload, so GetObject streams the stored bytes verbatim; the ordering defect is therefore in the bytes the AppFlow destination writer stored, not in the GetObject serializer.
Emulator: every record is emitted with its keys sorted alphabetically, so the stored (and served) bytes differ from AWS's even though the record set and values are identical. Replay reports `$: value_mismatch` - expected the source-order lines, got the key-sorted lines. The requirement is order PRESERVATION relative to the source record; the recording's particular field names and values are per-fixture data, not the requirement.
Why: a client that reconciles a pipeline end-to-end by comparing bytes or a digest of the destination object against the source (or against a digest it computed itself) sees a mismatch that AWS would not produce; any consumer that is byte- or line-sensitive (a diff, a signature over the object, a fixed-width/ordered parser, a checksum recomputation) disagrees with AWS.
Fix: the AppFlow flow-run destination record writer (the component that serializes each transferred record into the S3 object it stores) must preserve source member order instead of serializing from a key-sorted map.
Traces: appflow-s3-to-s3-flow-run-transfers-records#task#10.

Cause 2 of 3: the object is served with no full-object checksum headers.
AWS: Smithy: GetObject output ChecksumCRC64NVME -> @httpHeader("x-amz-checksum-crc64nvme") and ChecksumType -> @httpHeader("x-amz-checksum-type") (enum ChecksumType: COMPOSITE | FULL_OBJECT); the operation's @httpChecksum trait declares requestValidationModeMember ChecksumMode (request header x-amz-checksum-mode) with responseAlgorithms including CRC64NVME. A single-part write - including one performed by a service integration such as AppFlow - carries S3's default full-object CRC64NVME checksum, so the read reports x-amz-checksum-crc64nvme as the base64 CRC64NVME of the stored bytes together with x-amz-checksum-type: FULL_OBJECT.
Emulator: both headers are absent. Replay reports `$headers.x-amz-checksum-crc64nvme: missing (expected '+VAf27mVm4s=', got None)` and `$headers.x-amz-checksum-type: missing (expected 'FULL_OBJECT', got None)`. The base64 value in the recording is the checksum of that recording's bytes - the requirement is the correctly computed CRC64NVME of whatever bytes are stored, not that literal. Hypothesis: the emulator neither computes nor retains a checksum for service-written objects, so the serializer has nothing to emit.
Why: a client that validates integrity on read (SDK checksum validation driven by x-amz-checksum-mode: ENABLED, or any consumer comparing the header against its own digest) gets no checksum and silently skips verification, so corruption AWS would surface passes unnoticed; code that branches on FULL_OBJECT vs COMPOSITE takes the wrong branch.
Fix: S3 object state must record each write's checksum algorithm, value and checksum type (full-object CRC64NVME by default for a single-part write, including writes made by service integrations), and the GetObject response serializer must emit both headers from it.
Traces: appflow-s3-to-s3-flow-run-transfers-records#task#10.

Cause 3 of 3: the object's server-side-encryption mode is misreported as AES256 and its KMS key is never reported.
AWS: Smithy: GetObject output ServerSideEncryption -> @httpHeader("x-amz-server-side-encryption") (enum ServerSideEncryption, values include AES256 and aws:kms) and SSEKMSKeyId -> @httpHeader("x-amz-server-side-encryption-aws-kms-key-id") (@sensitive). The AppFlow S3 destination writes with KMS server-side encryption, so the recorded read reports x-amz-server-side-encryption: aws:kms together with the KMS key id/ARN that encrypted the object. The requirement is that the object's actual encryption mode is retained and reported, and that an aws:kms object also reports its encrypting key id.
Emulator: reports x-amz-server-side-encryption: AES256 and omits x-amz-server-side-encryption-aws-kms-key-id entirely. Replay reports `$headers.x-amz-server-side-encryption: value_mismatch (expected 'aws:kms', got 'AES256')` and `$headers.x-amz-server-side-encryption-aws-kms-key-id: missing`. The key ARN itself is an environment-specific value and is not the requirement - its presence, and its agreement with the reported encryption mode, are.
Why: a client or compliance control that asserts KMS encryption on pipeline output reads AES256 with no key id and concludes the object is only SSE-S3 encrypted, failing a check that passes on AWS; code that re-encrypts, re-keys, or audits by key id has no key to act on.
Fix: the S3 object-metadata state must retain the write's encryption mode and KMS key id (and the AppFlow destination writer must store the object with the destination's KMS encryption rather than defaulting to SSE-S3), and the GetObject response serializer must emit x-amz-server-side-encryption and x-amz-server-side-encryption-aws-kms-key-id from that state.
Traces: appflow-s3-to-s3-flow-run-transfers-records#task#10.

Not covered elsewhere: open issue 20261109T175957Z-s3-getobject-serves-the-appflow-written-object-with.md describes the same three facets but does NOT carry this cluster's replay key (its recorded Issue-Key lines cover only $, $headers.x-amz-checksum-type and $headers.x-amz-server-side-encryption, not x-amz-checksum-crc64nvme or x-amz-server-side-encryption-aws-kms-key-id), so the replay reports this cluster as unowned; it is re-filed here in full and that issue should be closed together with it. The open issues 20261109T135237Z-s3-getobject-loses-object-response-headers-user-metadata.md, 20261109T165919Z-s3-getobject-drops-the-transfer-written-object-s.md, 20261110T030759Z-s3-getobject-omits-the-crc32-full-object-checksum.md and 20261113T084741Z-s3-getobject-serves-a-datasync-copied-object-without.md concern GetObject header loss on OTHER write paths (Transfer, Firehose, DataSync) and do not cover this fixture step. The sibling ListObjectsV2 divergence on the same fixture (task#9) and the two appflow divergences (task#2, task#8) are reported owned by open issues and are not re-filed.

## Evidence
Replayed step: fixture appflow-s3-to-s3-flow-run-transfers-records, step task#10, service s3, operation GetObject, verdict mismatch, expected_status 200, actual_status 200 (status matches; body and headers diverge). Divergence key: service=s3|GetObject|status=200->200|keys=$,$headers.x-amz-checksum-crc64nvme,$headers.x-amz-checksum-type,$headers.x-amz-server-side-encryption,$headers.x-amz-server-side-encryption-aws-kms-key-id.

Expected (recorded AWS) vs actual (emulator):
- $: value_mismatch. Expected body '{"id":"1","city":"lisbon","temp":"21"}\n{"id":"2","city":"oslo","temp":"4"}\n{"id":"3","city":"lima","temp":"18"}\n'; got '{"city":"lisbon","id":"1","temp":"21"}\n{"city":"oslo","id":"2","temp":"4"}\n{"city":"lima","id":"3","temp":"18"}\n'. Same records and values, members alphabetized instead of kept in source order. The field names/values are fixture data; the invariant is source member order per JSON-Lines record.
- $headers.x-amz-checksum-crc64nvme: missing. Expected the base64 CRC64NVME of the stored bytes ('+VAf27mVm4s=' for this recording's bytes), got None.
- $headers.x-amz-checksum-type: missing. Expected 'FULL_OBJECT' (fixed enum value for a single-part write), got None.
- $headers.x-amz-server-side-encryption: value_mismatch. Expected 'aws:kms' (fixed enum value), got 'AES256'.
- $headers.x-amz-server-side-encryption-aws-kms-key-id: missing. Expected the encrypting KMS key id/ARN (a per-environment value; presence and agreement with the SSE mode are the requirement), got None.

Smithy citations (services/service-com-amazonaws-s3-amazons3/smithy-model): com.amazonaws.s3#GetObject, http GET /{Bucket}/{Key+}, code 200; output Body -> StreamingBlob @httpPayload @streaming; ChecksumCRC64NVME -> header x-amz-checksum-crc64nvme; ChecksumType -> header x-amz-checksum-type (enum COMPOSITE | FULL_OBJECT); ServerSideEncryption -> header x-amz-server-side-encryption (enum incl. AES256, aws:kms); SSEKMSKeyId -> header x-amz-server-side-encryption-aws-kms-key-id (@sensitive); @httpChecksum requestValidationModeMember=ChecksumMode, responseAlgorithms include CRC64NVME; errors NoSuchKey (404), InvalidObjectState (403).

## Steps to Reproduce
1. Create the source and destination buckets and seed the source records as fixture appflow-s3-to-s3-flow-run-transfers-records does.
2. Create the AppFlow S3-source -> S3-destination flow (destination connector properties set a bucket prefix and KMS server-side encryption), call StartFlow and wait for the execution to finish.
3. GET the destination object written by the run (key '<destinationPrefix>/<flowName>/<executionId>'), sending x-amz-checksum-mode: ENABLED. This is step task#10 of fixture appflow-s3-to-s3-flow-run-transfers-records.
4. Compare the body bytes: each JSON-Lines record comes back with its members alphabetized instead of in source order.
5. Inspect the response headers: x-amz-checksum-crc64nvme and x-amz-checksum-type are absent (AWS sends the base64 CRC64NVME and FULL_OBJECT), and x-amz-server-side-encryption is 'AES256' with no x-amz-server-side-encryption-aws-kms-key-id (AWS sends 'aws:kms' plus the key id).
6. Re-replay appflow-s3-to-s3-flow-run-transfers-records#task#10 to confirm all three causes: source-order body bytes, both checksum headers present and correct, and x-amz-server-side-encryption 'aws:kms' with the key id header.
