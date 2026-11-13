Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: datasync.already-synced-destination-transfers-no-bytes-but-verifies
Issue-Shape-Id: com.amazonaws.datasync#FmrsService
Issue-Key: datasync.already-synced-destination-transfers-no-bytes-but-verifies|run#21|AssertionError|assert execution["FilesSkipped"] == len(_OBJECTS), (
Issue-Commit: cd708b7b38e0e4ba503d0604c83e990f017a36a0

# DataSync DescribeTaskExecution reports FilesSkipped=0 for a CHANGED-mode execution onto an already-identical destination

> _Note: this issue was identified against the code base at commit `cd708b7b38e0e4ba503d0604c83e990f017a36a0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeTaskExecution

## Description
One net-new root cause, grounded in one failing behavioral check of simparity-behavioral-aws-v1 (datasync 9/12).

RELATED, NOT COVERING: open issue 20261029T172420Z-datasync-3-net-new-causes-describeagent-is-a.md carries this check in its Issue-Trace-Ids, but its replay key for it is the assertion `execution["FilesVerified"] == len(_OBJECTS)` (run#15). That part now passes: the execution reaches Status=SUCCESS with Result.VerifyStatus=SUCCESS, FilesVerified=3, FilesTransferred=0, BytesTransferred=0, BytesWritten=0, EstimatedFilesToTransfer=0, EstimatedBytesToTransfer=0 and FilesDeleted=0, and the destination objects are untouched. The check now fails one assertion further on, at `execution["FilesSkipped"] == len(_OBJECTS)` — a different replay key, which is why the suite does not list it as OWNED. The skip half of that cause is still open and is what this issue is about. Open issue 20261110T181940Z-datasync-describetaskexecution-reports-a-no-op-transfer-zero.md is a wire finding about the whole counter block being left at defaults and about Excludes/Includes/Options serialization; it does not mention FilesSkipped and does not cover the CHANGED-mode skip accounting.

=== Cause 1 of 1: files that TransferMode=CHANGED decides not to move are not counted as skipped ===

AWS: Smithy (services/service-com-amazonaws-datasync-fmrsservice/smithy-model/com-amazonaws-datasync-fmrsservice.json, DescribeTaskExecution) models FilesSkipped as `long` with @default 0, documented as "The number of files, objects, and directories that DataSync skips during your transfer." Skipping is not restricted to one Options member: a TransferMode=CHANGED execution "copies only the data and metadata that differs between the source and destination location", so a source object the destination already holds with identical content is skipped, and real AWS accounts for it in FilesSkipped. For an execution over a source whose every object already exists byte-identically at the destination, AWS reports Status=SUCCESS, Result.VerifyStatus=SUCCESS, FilesSkipped = the number of source objects, FilesVerified = the same number, and FilesTransferred = BytesTransferred = BytesWritten = EstimatedFilesToTransfer = EstimatedBytesToTransfer = FilesDeleted = 0. The requirement is the partition of the source file count: every source file is accounted for as either transferred or skipped, never neither. (The suite's 3 objects / 850 bytes are that template's values, not the requirement.)

Emulator: the execution ends Status=SUCCESS with every other counter correct, but FilesSkipped stays at the modeled default 0 — "AssertionError: FilesSkipped was 0, expected all 3 to be skipped". The transfer engine counts as skipped ONLY the objects OverwriteMode=NEVER protects: `selected()` already removes the destination-identical objects from the candidate set when TransferMode is not ALL, and `partition_by_overwrite_mode()` then partitions only what survived that filter, so the CHANGED-mode skips are dropped before anything can count them and `counters.files_skipped = skipped.len()` assigns 0.

Why: a client cannot tell the normal steady-state DataSync run (source fully in sync: nothing moved, everything skipped) from a run whose source was empty or whose prepare phase enumerated nothing — both report 0 transferred and 0 skipped. Re-run/idempotency assertions, per-run reconciliation that checks transferred + skipped equals the source inventory, and dashboards that chart FilesSkipped against FilesTransferred all read zero for a run AWS reports as 3 files skipped.

Fix: the task-execution transfer phase and its counter state — services/service-com-amazonaws-datasync-fmrsservice/src/state/transfer.rs: the set of admitted source objects that the CHANGED TransferMode excluded because the destination already matches must be carried into the skip count alongside the OverwriteMode-protected ones, so FilesSkipped covers every admitted source object the phase did not write, before DescribeTaskExecution serializes ExecutionCounters (src/state.rs).

Traces: datasync.already-synced-destination-transfers-no-bytes-but-verifies

## Evidence
Suite: simparity-behavioral-aws-v1. datasync 9/12 (75%), 3 failed; of those, one is marked ALREADY FILED (unimplemented ListTagsForResource) and one is marked OWNED by 20261029T172420Z-datasync-3-net-new-causes-describeagent-is-a.md. This is the one survivor.

[mismatch] datasync.already-synced-destination-transfers-no-bytes-but-verifies
  Setup: a source bucket and a destination bucket seeded by S3 PutObject (not by DataSync) with the SAME three keys and the SAME bytes (alpha.txt 100B, beta.txt 250B, logs/gamma.log 500B); an IAM role granting both buckets; CreateLocationS3 with Subdirectory="/" for each; CreateTask with Options={VerifyMode: POINT_IN_TIME_CONSISTENT, LogLevel: OFF, TransferMode: CHANGED}; StartTaskExecution polled to a terminal status.
  Expected: Status=SUCCESS, Result.VerifyStatus=SUCCESS, FilesVerified=3, FilesSkipped=3, FilesTransferred=0, BytesTransferred=0, BytesWritten=0, EstimatedFilesToTransfer=0, EstimatedBytesToTransfer=0, FilesDeleted=0, destination objects unchanged (same Size/ETag/LastModified).
  Actual: every one of those holds EXCEPT FilesSkipped, which is 0. Failure at check line 73: `assert execution["FilesSkipped"] == len(_OBJECTS)` -> "AssertionError: FilesSkipped was 0, expected all 3 to be skipped". HTTP status 200 on every call; no error code or message is involved, so the divergence is purely the counter value.

Source of the divergence (citations, not a patch): services/service-com-amazonaws-datasync-fmrsservice/src/state/transfer.rs — `selected()` (line ~682) filters out, for a non-ALL TransferMode, every admitted source object whose destination counterpart `matches()` it; `partition_by_overwrite_mode()` (line ~705) partitions only that already-reduced set and returns as `skipped` only what OverwriteMode=NEVER protects; `transfer()` then does `counters.files_skipped = skipped.len() as i64` (line ~459), which is 0 whenever the skip was decided by CHANGED rather than by OverwriteMode.

Model references: DescribeTaskExecution output FilesSkipped -> long, @default 0, doc "The number of files, objects, and directories that DataSync skips during your transfer."; FilesVerified/FilesTransferred/BytesTransferred/BytesWritten/EstimatedFilesToTransfer/EstimatedBytesToTransfer/FilesDeleted -> long, @default 0; Result -> TaskExecutionResultDetail with PrepareStatus/TransferStatus/VerifyStatus of enum PhaseStatus; Options.TransferMode enum [CHANGED, ALL].

## Steps to Reproduce
Boot the emulator and replay datasync.already-synced-destination-transfers-no-bytes-but-verifies.

Manually: create a source and a destination S3 bucket; with S3 PutObject write the SAME keys and the SAME bytes into both (e.g. alpha.txt 100B, beta.txt 250B, logs/gamma.log 500B) so the destination is seeded by S3 and not by DataSync; create an IAM role trusted by datasync.amazonaws.com granting read/write on both buckets; CreateLocationS3 for each bucket with Subdirectory="/" and S3Config.BucketAccessRoleArn set to that role; CreateTask over the two locations with Options={VerifyMode: POINT_IN_TIME_CONSISTENT, LogLevel: OFF, TransferMode: CHANGED}; wait for the task to be AVAILABLE; StartTaskExecution and poll DescribeTaskExecution to Status=SUCCESS.

Observe FilesSkipped=0 where AWS reports FilesSkipped = the number of source objects (3 here), while FilesVerified=3, FilesTransferred=0, BytesTransferred=0, BytesWritten=0, EstimatedFilesToTransfer=0, EstimatedBytesToTransfer=0 and FilesDeleted=0 are already correct and the destination objects are untouched. Contrast with a task whose source is empty: it produces the identical all-zero skip/transfer accounting, which is the ambiguity this creates.
