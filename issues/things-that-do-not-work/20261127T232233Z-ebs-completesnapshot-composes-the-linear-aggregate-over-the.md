Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ebs.complete-snapshot-linear-sha256-aggregate-seals-snapshot
Issue-Shape-Id: com.amazonaws.ebs#Ebs
Issue-Key: ebs|CompleteSnapshot|ValidationException
Issue-Commit: 5cb989982ca164b028438556d2b2a85d4de8b473

# ebs CompleteSnapshot composes the LINEAR aggregate over the per-block Base64 TEXT instead of the raw SHA256 digests, so a correctly built completion is refused and the snapshot never seals

> _Note: this issue was identified against the code base at commit `5cb989982ca164b028438556d2b2a85d4de8b473`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CompleteSnapshot

## Description
REOPENS 20261119T114443Z-ebs-completesnapshot-parameter-ignored.md: its fix left the call refused.

Cause 1 of 1: the aggregate `x-amz-Checksum` a producer computes the way the real service computes it is rejected by the emulator, so the ordinary seal-a-snapshot path cannot complete at all when the optional aggregate is supplied.

AWS: Smithy `CompleteSnapshot` (POST /snapshots/completion/{SnapshotId}, @http code 202) takes `Checksum` (`x-amz-Checksum`, @length 0..64, @pattern ^[A-Za-z0-9+/=]+$), `ChecksumAlgorithm` (enum [SHA256]) and `ChecksumAggregationMethod` (enum [LINEAR]); the modeled doc on `CompleteSnapshotRequest$Checksum` says to "arrange the checksums for each written block in ascending order of their block index, concatenate them to form a single string, and then generate the checksum on the entire string using the SHA256 algorithm". The recording pins what that concatenation actually is: the real service accepts base64(SHA256( concat of the RAW 32-byte SHA256 digests of each written block, ascending by BlockIndex )) and answers 202 with Status pending/completed, after which the snapshot settles `completed` and every block reads back. The written blocks in the recording were put out of index order (7 then 2) and the accepted aggregate is still the ascending-index one, so ordering is by block index, not by write order; the concatenation unit is the raw digest, not its Base64 text.

Emulator: `CompleteSnapshot` recomputes the aggregate by concatenating the stored per-block checksums as their BASE64 TEXT and hashing that ASCII string, then refuses any request whose declared value differs. The replay gets HTTP 400 ValidationException "The value passed in as x-amz-Checksum does not match the computed checksum. Computed checksum: <emulator's base64-text aggregate> expected checksum: <the client's raw-digest aggregate>" (both values are per-run data, not part of the requirement) — and because the request is refused, the snapshot stays `pending`, so nothing is sealed, no block becomes listable and the whole happy path stops there. The ordering (ascending by BlockIndex) is already right; only the unit being concatenated is wrong.

Why: a client that builds the aggregate exactly as AWS documents and accepts it can never seal a snapshot through the emulator. Every downstream step — DescribeSnapshots reaching `completed`, ListSnapshotBlocks becoming visible, GetSnapshotBlock serving bytes — is unreachable, and the caller is told its correctly computed checksum is wrong. The same mis-composition also refuses the aggregate supplied by any other check that passes one.

Fix: the aggregate computation in the snapshot state (`Snapshot::written_blocks_linear_checksum`), consumed by the `CompleteSnapshot` handler's checksum comparison — it must hash the concatenation of the raw per-block SHA256 digests (base64-decoding each stored per-block checksum, or storing the digest bytes) in ascending BlockIndex order, rather than the concatenation of their Base64 renderings.

Traces: behavioral check ebs.complete-snapshot-linear-sha256-aggregate-seals-snapshot. Code citations: services/service-com-amazonaws-ebs-ebs/src/state.rs:395-401 (`written_blocks_linear_checksum` pushes `entry.checksum`, the Base64 text, into one String and hashes it); services/service-com-amazonaws-ebs-ebs/src/ops/complete_snapshot.rs:258-275 (the comparison that refuses); services/service-com-amazonaws-ebs-ebs/src/state.rs:959-964 (`checksum_mismatch_message`).

## Evidence
Expected (real AWS, from the recording): StartSnapshot(VolumeSize=1) -> PutSnapshotBlock at indexes 7 and 2 (524288 bytes each, per-block Checksum = base64(sha256(block)), ChecksumAlgorithm=SHA256) -> CompleteSnapshot(ChangedBlocksCount=2, ChecksumAlgorithm=SHA256, ChecksumAggregationMethod=LINEAR, Checksum = base64(sha256( sha256(block[2]).digest() + sha256(block[7]).digest() ))) answers 202 with Status in {pending, completed}; DescribeSnapshots then settles the snapshot to `completed`, ListSnapshotBlocks lists exactly indexes [2, 7] and each GetSnapshotBlock returns the written bytes with the written per-block checksum.

Actual (emulator): the CompleteSnapshot call is refused HTTP 400 ValidationException, message "The value passed in as x-amz-Checksum does not match the computed checksum. Computed checksum: jbJEmXpsl7qQP2DfPNCe93O+sSTYhU0YBYilPZCKz1k= expected checksum: eZViIO4Lq0Sp1yIkj7VmLbZjveP6EcsQ4VN8+Cs/JHw=" (the two base64 values are this run's data). The snapshot therefore never leaves `pending` and no block is ever listable. The divergence is the composition: the emulator hashes the concatenated Base64 TEXTS of the per-block checksums (state.rs:395-401), AWS hashes the concatenated raw 32-byte digests.

## Steps to Reproduce
1. POST /snapshots with {"VolumeSize": 1} and note SnapshotId.
2. PUT /snapshots/{SnapshotId}/blocks/7 and then /blocks/2, each with 524288 random bytes, x-amz-Data-Length: 524288, x-amz-Checksum-Algorithm: SHA256 and x-amz-Checksum = base64(sha256(payload)). Both are accepted and echo the same checksum.
3. Compute A = base64(sha256( sha256(block2).digest() || sha256(block7).digest() )) — raw digests, ascending block index.
4. POST /snapshots/completion/{SnapshotId} with x-amz-ChangedBlocksCount: 2, x-amz-Checksum-Algorithm: SHA256, x-amz-Checksum-Aggregation-Method: LINEAR, x-amz-Checksum: A.
5. AWS answers 202 and the snapshot settles `completed`. The emulator answers 400 ValidationException "The value passed in as x-amz-Checksum does not match the computed checksum. ..." and leaves the snapshot `pending`.
6. Repeat step 4 with A' = base64(sha256( base64(sha256(block2)) || base64(sha256(block7)) )) — the Base64-text composition — and the emulator accepts it, confirming the unit it concatenates.
