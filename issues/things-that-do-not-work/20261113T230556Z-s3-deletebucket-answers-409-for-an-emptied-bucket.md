Issue-Source: trace-sweep-divergence
Issue-Trace: athena-kfi-traj-athena-reads-the-iceberg-table
Issue-Step: teardown#19
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: athena-kfi-traj-athena-reads-the-iceberg-table
Issue-Shape-Id: com.amazonaws.athena#AmazonAthena
Issue-Key: s3|DeleteBucket|2xx->4xx|status
Issue-Commit: a4d278f1279dca95bd1b1693fbff7a33c3289259

# s3 DeleteBucket answers 409 for an emptied bucket that AWS deletes with 204

> _Note: this issue was identified against the code base at commit `a4d278f1279dca95bd1b1693fbff7a33c3289259`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteBucket

## Description
Cause 1 of 1: DeleteBucket refuses a bucket whose contents were removed earlier in the same recorded teardown, answering HTTP 409 where AWS answers 204.
AWS: recorded behavior only — the fixture's teardown deletes the bucket's objects (including the Athena/Iceberg result and table objects written under the query-results and table prefixes) and then calls DeleteBucket on it; the recorded AWS response is HTTP 204 No Content with an empty body. AWS only refuses a DeleteBucket with 409 Conflict when the bucket still holds objects (or noncurrent versions / incomplete multipart uploads); here it held none, so the delete succeeded.
Emulator: the same DeleteBucket answers HTTP 409 — status expected 204, got 409 — so the bucket is never removed and the teardown cannot complete. Hypothesis: the emulator still counts residual server-written objects (the Athena query-result files, Iceberg metadata/data objects, or incomplete multipart uploads) that are not reachable through the ListObjectsV2/DeleteObject(s) path the teardown used, so its emptiness test fails for a bucket that is empty from the client's point of view.
Why: a client that empties a bucket and then deletes it — the documented and universal teardown sequence, and what every CDK/CloudFormation/CLI cleanup does — gets a 409 it cannot clear: nothing it can list remains to delete, so the retry loop never converges and the bucket leaks. It also makes any suite that provisions and tears down an Athena results bucket unable to reach a clean state, masking later failures.
Fix: the DeleteBucket handler's emptiness check and the bucket/object state it consults — objects written by server-side paths (Athena results, Iceberg table writes) must be enumerated and removed by the same object listing/delete path a client uses, so that a bucket a client has emptied passes the check and DeleteBucket returns 204.
Traces: athena-kfi-traj-athena-reads-the-iceberg-table#teardown#19.

## Evidence
Replay of trace set simparity-wire-aws-v1 (score 67.0%), divergence cluster 32 of 42: verdict=mismatch, service=s3, operation=DeleteBucket, fixture athena-kfi-traj-athena-reads-the-iceberg-table, step teardown#19, affected_count 1, stub_path empty, owned_by empty (no open issue carries this replay key; the open s3 issues cover storage ceilings, listing responses, GetObject metadata and DeleteObjects versioned batches, none of them DeleteBucket).

detail: status: expected 204, got 409
expected_status 204, actual_status 409, no body keys asserted.

## Steps to Reproduce
1. git_clone() and boot the emulator built from the checkout.
2. Replay fixture athena-kfi-traj-athena-reads-the-iceberg-table end to end, so the Athena/Iceberg phases write their query results and table objects into the probe bucket.
3. Let the teardown run: it removes the bucket's objects and then issues DeleteBucket on that bucket (step teardown#19).
4. Observe DeleteBucket answers HTTP 409 instead of HTTP 204, and the bucket still exists.
