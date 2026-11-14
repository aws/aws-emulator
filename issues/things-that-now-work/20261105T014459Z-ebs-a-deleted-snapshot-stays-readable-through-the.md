Issue-Source: trace-sweep-divergence
Issue-Trace: ebs-snapshot-lifecycle-and-block-listing
Issue-Step: teardown#7
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ebs-snapshot-lifecycle-and-block-listing
Issue-Shape-Id: com.amazonaws.ebs#Ebs
Issue-Key: ebs|ListSnapshotBlocks|4xx->2xx|$
Issue-Key: ebs|ListSnapshotBlocks|4xx->2xx|$.Message
Issue-Key: ebs|ListSnapshotBlocks|4xx->2xx|$.Reason
Issue-Key: ebs|ListSnapshotBlocks|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: ebs|ListSnapshotBlocks|4xx->2xx|status
Issue-Commit: a5885c3be3f986c1a94ab18540ac47db2748e09d

# ebs: a deleted snapshot stays readable through the EBS direct API

> _Note: this issue was identified against the code base at commit `a5885c3be3f986c1a94ab18540ac47db2748e09d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListSnapshotBlocks

## Description
1 cause.

Cause 1 of 1: after the snapshot created through the EBS direct API (StartSnapshot/PutSnapshotBlock/CompleteSnapshot) is deleted, ListSnapshotBlocks on that snapshot id still answers HTTP 200 with the snapshot's block listing and metadata instead of the modeled not-found error. Fixture ebs-snapshot-lifecycle-and-block-listing, step teardown#7.

AWS: Smithy (services/service-com-amazonaws-ebs-ebs/smithy-model/com-amazonaws-ebs-ebs.json) binds ListSnapshotBlocks to GET /snapshots/{SnapshotId}/blocks and declares ResourceNotFoundException with @httpError 404 ("The specified resource does not exist."), whose members are Message (ErrorMessage, len 0..256) and Reason (ResourceNotFoundExceptionReason, values SNAPSHOT_NOT_FOUND | GRANT_NOT_FOUND | DEPENDENCY_RESOURCE_NOT_FOUND | IMAGE_NOT_FOUND). A snapshot id that the account no longer holds — including one that WAS held and has since been deleted — is not a resource the EBS direct API can list, so AWS answers 404 ResourceNotFoundException with Reason "SNAPSHOT_NOT_FOUND" and the x-amzn-errortype header naming ResourceNotFoundException. The recorded response's message is "The snapshot 'snap-<hex>' does not exist." — its FIXED text is "The snapshot '" ... "' does not exist." and its only VARIABLE part is the snapshot id taken verbatim from the request's SnapshotId path label (the recorded snap-3c17d841bdec69a1c is one run's value and is not part of the requirement). Deletion is eventually consistent on the read path, which is why the fixture polls rather than asserting on a single response.

Emulator: the poll never converges. Every attempt answers HTTP 200 with {"Blocks": [], "ExpiryTime": ..., "VolumeSize": 1, "BlockSize": 524288} — an existing, readable, empty snapshot — and the replay reports "poll: status 200 not in [404] after 13 attempt(s)", capped at 60s. $.Message and $.Reason are absent, and no x-amzn-errortype header is emitted. The ListSnapshotBlocks handler itself does construct the right refusal (it returns ResourceNotFoundException with Reason SNAPSHOT_NOT_FOUND and the "The snapshot '<id>' does not exist." message whenever the id resolves to no readable snapshot), so the wrong answer comes from STATE, not from the serializer: the snapshot record minted by the EBS direct-API path is never removed, so the lookup keeps succeeding. The EBS snapshot store exposes no deletion path that a snapshot delete reaches; related, the deletion request in this fixture's teardown is itself refused 501 by the ec2 DeleteSnapshot scaffold stub, which is filed separately as 20261023T060504Z-ec2-deletesnapshot-stub.md and is NOT this cause — implementing that stub alone still leaves the EBS-side record intact, exactly as 20261030T204611Z-ebs-direct-api-snapshots-are-invisible-to-ec2.md reports for the read direction.

Why: a client that deletes a snapshot and then reads it gets a successful listing of a snapshot that no longer exists. An empty Blocks array on a 200 is the wire shape AWS uses for a snapshot that exists and holds no block, so a caller cannot distinguish "deleted" from "present but empty": cleanup verification loops never terminate (or terminate claiming the snapshot survived), block-token refresh logic keeps re-reading a dead snapshot, and error handling written against ResourceNotFoundException/SNAPSHOT_NOT_FOUND never runs. Any code that treats a 404 on the read path as the completion signal for a delete hangs until its own timeout.

Fix: the ebs service's snapshot state — it needs a deletion path that removes (or tombstones out of the readable set) a snapshot record minted by the direct-API write path, reachable from a snapshot delete, so the existing not-found branch in the ListSnapshotBlocks handler is taken. The read handlers' refusal construction is already correct and should not change.

Traces: ebs-snapshot-lifecycle-and-block-listing#teardown#7 (ListSnapshotBlocks, expected 404, got 200). Handler whose not-found branch is never reached: services/service-com-amazonaws-ebs-ebs/src/ops/list_snapshot_blocks.rs lines 195-220 (readability filter and the not_found/snapshot_not_found_message refusal) and lines 297-304 (the ResourceNotFoundException constructor with Reason SNAPSHOT_NOT_FOUND).

## Evidence
Fixture ebs-snapshot-lifecycle-and-block-listing, step teardown#7, operation ListSnapshotBlocks, verdict mismatch.

Expected (recorded AWS): HTTP 404, body {"Message": "The snapshot 'snap-<hex>' does not exist.", "Reason": "SNAPSHOT_NOT_FOUND"}, header x-amzn-errortype present (shaped, not asserted on value).

Actual (emulator): HTTP 200, body {"Blocks": [], "ExpiryTime": <float epoch>, "VolumeSize": 1, "BlockSize": 524288}. Replay detail: "status: expected 404, got 200"; "$.Message: missing"; "$.Reason: missing"; "$headers.x-amzn-errortype: missing"; "$: poll: status 200 not in [404] after 13 attempt(s)" / "poll capped at 60s".

Smithy evidence: ListSnapshotBlocks errors include ResourceNotFoundException (errorKind client, httpError 404); ResourceNotFoundException.Reason targets enum ResourceNotFoundExceptionReason, which contains SNAPSHOT_NOT_FOUND.

## Steps to Reproduce
1. Create a snapshot through the EBS direct API: StartSnapshot, PutSnapshotBlock for at least one block, CompleteSnapshot, and wait until the snapshot is readable.
2. Confirm ListSnapshotBlocks on that snapshot id answers 200 with the written blocks.
3. Delete that snapshot.
4. Poll GET /snapshots/{SnapshotId}/blocks (ListSnapshotBlocks) for the same snapshot id.
5. Expected: the poll converges on HTTP 404 ResourceNotFoundException with Reason "SNAPSHOT_NOT_FOUND", Message "The snapshot '<the requested snapshot id>' does not exist." and an x-amzn-errortype response header. Observed: every attempt answers HTTP 200 with an empty Blocks array plus ExpiryTime/VolumeSize/BlockSize, and the poll exhausts its 60s cap.

Replay to confirm: replay fixture ebs-snapshot-lifecycle-and-block-listing and check step teardown#7.
