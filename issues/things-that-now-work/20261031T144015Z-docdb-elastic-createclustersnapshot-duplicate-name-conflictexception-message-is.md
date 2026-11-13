Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: docdb-elastic.create-cluster-snapshot-duplicate-name-conflict-rejected
Issue-Shape-Id: com.amazonaws.docdbelastic#ChimeraDbLionfishServiceLambda
Issue-Key: docdb-elastic.create-cluster-snapshot-duplicate-name-conflict-rejected|run#13|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Commit: ea226a61fd4fc055bb456ff32cf72ea13892c65d

# DocDB Elastic CreateClusterSnapshot duplicate-name ConflictException message is interpolated prose instead of AWS's fixed sentence

> _Note: this issue was identified against the code base at commit `ea226a61fd4fc055bb456ff32cf72ea13892c65d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateClusterSnapshot

## Description
One cause.

Cause 1 of 1: the ConflictException raised when CreateClusterSnapshot is called with a snapshotName that already exists carries emulator-authored, name-interpolated prose rather than the fixed sentence AWS returns for that error class. Failing check: docdb-elastic.create-cluster-snapshot-duplicate-name-conflict-rejected.

AWS: a duplicate snapshotName is refused with ConflictException, HTTP 409 (Smithy: CreateClusterSnapshot errors include ConflictException, httpError 409; its modeled doc is only "There was an access conflict", and the model states nothing about message text, so the message is recorded behavior only). The recording shows the body's `message` is the FIXED text `A snapshot already exists with given name` — the whole sentence is fixed text, with NO variable part: AWS does not interpolate the requested snapshotName, the conflicting snapshot's ARN, or a resource kind into it.

Emulator: the status and the error code are right (409 / ConflictException), but the message is `A cluster snapshot named <snapshotName> already exists` — a different sentence built by interpolating the request's snapshotName. Replay recorded `'A cluster snapshot named sim2real-docdbe-dupsnap-04d9bc676c75 already exists'` where `A snapshot already exists with given name` was owed (the name there is a per-run value filling the emulator's own template, not part of any requirement).

Why: a client that keys retry, idempotency or user-facing reporting on the conflict message — the common pattern for a service whose ConflictException is modeled with only a free-text `message` and no discriminating member — sees an unknown sentence and cannot tell a duplicate-name conflict from any other 409 this operation can raise. Matching or logging the AWS sentence fails, and the leaked caller-supplied name changes the message per request so it cannot be matched at all.

Note: open issue 20261029T172712Z-docdb-elastic-resourcenotfoundexception-bodies-invent-interpolated-prose-and.md reports the same interpolated-prose habit for ResourceNotFoundException; it does not cover ConflictException on CreateClusterSnapshot, which is this issue.

Fix: belongs in the CreateClusterSnapshot handler's duplicate-name refusal and the shared conflict-body helper it calls — the message for the duplicate-snapshot-name conflict class must be the fixed AWS sentence with no interpolation of the request's snapshotName or of the existing snapshot's ARN.

Traces: docdb-elastic.create-cluster-snapshot-duplicate-name-conflict-rejected. Code sites: services/service-com-amazonaws-docdbelastic-chimeradblionfishservicelambda/src/ops/create_cluster_snapshot.rs lines 196-208 (the `format!("A cluster snapshot named {} already exists", input.snapshot_name)` passed to `state::conflict`), and the `state::conflict` helper in services/service-com-amazonaws-docdbelastic-chimeradblionfishservicelambda/src/state.rs.

## Evidence
Check: docdb-elastic.create-cluster-snapshot-duplicate-name-conflict-rejected [mismatch]

Assertion from the replay report:
  File "check_docdb_elastic_create_cluster_snapshot_duplicate_name_conflict_rejected.py", line 39, in run
    assert message == expected, f"expected message {expected!r}, got {message!r}"
  AssertionError: expected message 'A snapshot already exists with given name', got 'A cluster snapshot named sim2real-docdbe-dupsnap-04d9bc676c75 already exists'

Expected (AWS):
  HTTP 409, Error.Code = "ConflictException", Error.Message = "A snapshot already exists with given name"  (entire sentence is fixed text; no variable parts)

Actual (emulator):
  HTTP 409, Error.Code = "ConflictException" (correct), Error.Message = "A cluster snapshot named <snapshotName-from-request> already exists"  (wrong sentence; the only variable part is the emulator's own interpolation of the request's snapshotName)

The error code assertion passed and the effect assertions were never reached, so the divergence is confined to the message text. The snapshot name shown above is a per-run generated value and is not itself a requirement.

Smithy (services/service-com-amazonaws-docdbelastic-chimeradblionfishservicelambda/smithy-model/com-amazonaws-docdbelastic-chimeradblionfishservicelambda.json): CreateClusterSnapshot input snapshotName @length min 1 max 63, required; errors include ConflictException with httpError 409; ConflictException doc is "There was an access conflict" and the model prescribes no message text (recorded behavior only).

## Steps to Reproduce
1. CreateCluster with a unique clusterName (authType PLAIN_TEXT, shardCapacity 2, shardCount 1) and wait for status ACTIVE.
2. CreateClusterSnapshot(clusterArn=<that cluster>, snapshotName=<unique name N>) — succeeds.
3. CreateClusterSnapshot(clusterArn=<that cluster>, snapshotName=N) again, with the SAME name.
4. Observe the raised error: the code is ConflictException at HTTP 409 as expected, but Error.Message is "A cluster snapshot named N already exists" instead of AWS's fixed "A snapshot already exists with given name".
