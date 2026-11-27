Issue-Severity: MEDIUM
Issue-Gap-Class: parameter-ignored
Issue-Commit: 20b40f42cd78b95666b130bae2475dd4b1dad646

# CompleteSnapshot drops Checksum/ChecksumAggregationMethod, on a premise its own modeled doc refutes

_Filed: 2026-11-19T11:44:42Z_
_Operation: CompleteSnapshot | Class: parameter-ignored_

> _Note: this issue was identified against the code base at commit `20b40f42cd78b95666b130bae2475dd4b1dad646`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CompleteSnapshot` discards the request's aggregate `Checksum` and `ChecksumAggregationMethod` with an explicit `let _ = (...)`: the values are neither verified nor stored, so a completion carrying an aggregate checksum that does not correspond to the blocks the snapshot holds is sealed exactly as one carrying the right checksum, and nothing a later read reports distinguishes them.

The handler's stated reason for declining is that "the published prose does not pin the byte-level composition (whether the per-block checksums are concatenated as their Base64 text or as their raw digests)". That premise is contradicted by the modeled documentation this same file carries 160 lines above, which pins the composition exactly: the per-block checksums, in ascending block-index order, concatenated to form a single STRING, with SHA256 taken over the entire string. The crate also already holds the one helper the computation needs (`crate::state::checksum_base64_sha256`), which the sibling `PutSnapshotBlock` uses to verify the per-block checksum of every block it persists — so this is a check the crate can make with what it already has, declined on a reason the model text does not support.

## Evidence

The modeled contract, as the handler file itself renders it — services/service-com-amazonaws-ebs-ebs/src/ops/complete_snapshot.rs:58-62:
"//! * `Checksum` (Checksum, len 0..64, pattern ^[A-Za-z0-9+/=]+$) — An aggregated Base-64
//!   SHA256 checksum based on the checksums of each written block. To generate the aggregated
//!   checksum using the linear aggregation method, arrange the checksums for each written
//!   block in ascending order of their block index, concatenate them to form a single string,
//!   and then generate the checksum on the entire string using the SHA256 algorithm."
(identical to `smithy_query(query="doc",shape="com.amazonaws.ebs#CompleteSnapshotRequest",member="Checksum")`: "To generate the aggregated checksum using the linear aggregation method, arrange the checksums for each written block in ascending order of their block index, concatenate them to form a single string, and then generate the checksum on the entire string using the SHA256 algorithm.")

The code that drops both members, and the premise it gives — services/service-com-amazonaws-ebs-ebs/src/ops/complete_snapshot.rs:223-234:
"        // DECLINED, deliberately: the optional aggregate `Checksum` /
        // `ChecksumAggregationMethod` headers are NOT verified here. Their values are
        // constrained by the model (`LINEAR`) and the boundary validator already refuses
        // anything else, but the rule that turns the per-block checksums into the
        // aggregate is not in the model and the published prose does not pin the
        // byte-level composition (whether the per-block checksums are concatenated as
        // their Base64 text or as their raw digests).
        // Refusing on a guessed composition would reject requests a real client builds
        // correctly, which is worse than not checking; so the snapshot is sealed on the
        // evidence this service does have — the blocks it actually stored, checksum-
        // verified one by one as `PutSnapshotBlock` persisted them.
        let _ = (&input.checksum, &input.checksum_aggregation_method);"

The sibling that DOES verify a documented checksum with the helper this crate already has — services/service-com-amazonaws-ebs-ebs/src/ops/put_snapshot_block.rs:252-257:
"        let checksum = checksum_base64_sha256(&input.block_data);
        if checksum != input.checksum {
            return Err(crate::scaffolded::Scaffolded::invalid_signature(
                checksum_mismatch_message(&checksum, &input.checksum),
            ));
        }"
and the helper itself, services/service-com-amazonaws-ebs-ebs/src/state.rs:727-730: "/// Base64 SHA256 of `data`, the form `Checksum` is documented to carry.
pub fn checksum_base64_sha256(data: &[u8]) -> String {".

Test coverage: no test in services/service-com-amazonaws-ebs-ebs/tests/ sends a `x-amz-Checksum` to `CompleteSnapshot` at all — tests/all/payload_checksum_integrity.rs covers only `PutSnapshotBlock`'s per-block header, and tests/all/changed_blocks_count_decides_the_terminal_status.rs drives completion with `x-amz-ChangedBlocksCount` alone — so nothing pins the dropping of these two members.

## Steps to Reproduce

1. POST /snapshots with {"VolumeSize": 1}; PUT two blocks (indexes 0 and 1) with their correct per-block x-amz-Checksum values, each accepted 201.
2. POST /snapshots/completion/{snapshot} with x-amz-ChangedBlocksCount: 2, x-amz-Checksum-Algorithm: SHA256, x-amz-Checksum-Aggregation-Method: LINEAR and an x-amz-Checksum of a well-formed but WRONG aggregate (e.g. the Base64 SHA256 of the empty string).
3. Observe HTTP 202 and, on a repeat call, Status "completed": complete_snapshot.rs:234 discarded the header, so the mis-declared aggregate has no effect and is not recorded anywhere on the snapshot.
4. Repeat with the aggregate computed as the handler's own doc header describes (concatenate the two blocks' Base64 checksums in ascending index order, SHA256 the string, Base64 the digest) — the answer is byte-identical, so a caller cannot tell the two completions apart.
