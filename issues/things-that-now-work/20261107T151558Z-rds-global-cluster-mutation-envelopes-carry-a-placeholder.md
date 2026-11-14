Issue-Source: trace-sweep-divergence
Issue-Trace: rds-global-cluster-mutation-envelopes
Issue-Step: task#5
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: rds-global-cluster-mutation-envelopes
Issue-Shape-Id: com.amazonaws.rds#AmazonRDSv19
Issue-Key: rds|ModifyGlobalCluster|2xx->2xx|$.ModifyGlobalClusterResponse.ModifyGlobalClusterResult.GlobalCluster.GlobalClusterResourceId
Issue-Key: rds|DeleteGlobalCluster|2xx->2xx|$.DeleteGlobalClusterResponse.DeleteGlobalClusterResult.GlobalCluster.GlobalClusterResourceId
Issue-Key: rds|DeleteGlobalCluster|2xx->2xx|$.DeleteGlobalClusterResponse.DeleteGlobalClusterResult.GlobalCluster.Status
Issue-Commit: f1d789f71099019e66841b6c76416b94c6fc417d

# rds global cluster mutation envelopes carry a placeholder resource id and the wrong delete status

> _Note: this issue was identified against the code base at commit `f1d789f71099019e66841b6c76416b94c6fc417d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ModifyGlobalCluster

## Description
Three net-new root causes in the GlobalCluster envelope the mutation operations return. A fixer must close ALL THREE before submitting.

Cause 1 of 3: ModifyGlobalCluster's returned GlobalCluster.GlobalClusterResourceId is an emulator placeholder, not a minted partition-unique identifier.
AWS: Smithy: ModifyGlobalCluster output is ModifyGlobalClusterResult { GlobalCluster }; GlobalCluster$GlobalClusterResourceId is documented as "The Amazon Web Services partition-unique, immutable identifier for the global database cluster. This identifier is found in Amazon Web Services CloudTrail log entries whenever the Amazon Web Services KMS key for the DB cluster is accessed." The model states no pattern, so the concrete format is recorded behavior only: the recording asserts the member's SHAPE (the harness masks the literal as '<shaped-not-asserted>' because the value is per-resource random) — AWS mints a `cluster-` prefix followed by an opaque alphanumeric suffix that is unique per global database cluster and stable across subsequent reads and mutations of that same cluster.
Emulator: returns the fixed sequence placeholder 'cluster-00000000000000000000000001' — a zero-padded decimal counter. It fails the shape assertion and, being counter-derived, is the same value the emulator hands out for other global clusters in other fixtures, so it is neither partition-unique nor resource-identifying.
Why: a client that stores GlobalClusterResourceId to correlate the global cluster with CloudTrail KMS-access entries, or that uses it as the immutable key for a cluster whose GlobalClusterIdentifier was renamed by this very call, cannot tell two global clusters apart and joins the wrong records.
Fix: global-cluster state — mint the resource id once, per global cluster, when the global cluster is created and carry it through the Modify/Delete/Describe envelopes; the serializer must emit the stored id rather than a counter rendering.
Traces: rds-global-cluster-mutation-envelopes#task#5 and #task#7 (same cluster).

Cause 2 of 3: DeleteGlobalCluster's returned GlobalCluster.GlobalClusterResourceId carries the same placeholder.
AWS: Smithy: DeleteGlobalCluster output is DeleteGlobalClusterResult { GlobalCluster }, carrying the same GlobalCluster shape and therefore the same partition-unique immutable GlobalClusterResourceId; recorded behavior shows a shape-asserted (per-resource opaque) value.
Emulator: 'cluster-00000000000000000000000001' again.
Why: the delete response is the last chance a client has to record the deleted global cluster's immutable id for audit correlation; a counter placeholder makes that record useless and indistinguishable between clusters.
Fix: same global-cluster state/serializer path as cause 1, verified on the DeleteGlobalCluster response envelope.
Traces: rds-global-cluster-mutation-envelopes#task#10.

Cause 3 of 3: DeleteGlobalCluster reports Status 'deleting' in its response envelope, where AWS reports the status the global cluster held when the delete was accepted.
AWS: Smithy models GlobalCluster$Status as a plain String with no documented value list, so this is recorded behavior only: the recording of a DeleteGlobalCluster that succeeds on an available global cluster returns HTTP 200 with GlobalCluster.Status 'available' — AWS echoes the cluster's state as of the accepted request and does not transition the echoed copy.
Emulator: returns 200 with GlobalCluster.Status 'deleting'.
Why: a client that reads Status straight off the delete response to decide whether the global cluster was in a deletable state (or that asserts the pre-delete state for a reconciliation log) sees a transition AWS does not report there, and a state machine keyed on that field takes a branch that never happens against real RDS.
Fix: the DeleteGlobalCluster handler / its response serializer — serialize the global cluster snapshot as it was at acceptance instead of the post-mutation record it writes into state.
Traces: rds-global-cluster-mutation-envelopes#task#10.

Related open issues that do NOT cover these: 20261029T214632Z-rds-omits-vpc-subnet-and-availability-zone-attributes.md owns the same GlobalClusterResourceId divergence on the CreateGlobalCluster response, and 20261106T224204Z-rds-not-found-and-state-guard-refusal-messages.md owns the DeleteGlobalCluster deletion-protection sentence; the replay reports the Modify/Delete steps above as unowned. 20261104T053605Z-rds-modifyglobalcluster-stub.md is stale here: ModifyGlobalCluster now answers 200 on the wire.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout.

Cause 1 — fixture rds-global-cluster-mutation-envelopes, step task#5 (cluster also covers #task#7), ModifyGlobalCluster, status 200 -> 200:
  $.ModifyGlobalClusterResponse.ModifyGlobalClusterResult.GlobalCluster.GlobalClusterResourceId: value_mismatch
    expected: '<shaped-not-asserted>' (per-resource opaque `cluster-...` id; the literal is masked because it is a generated value)
    actual:   'cluster-00000000000000000000000001'

Causes 2 and 3 — fixture rds-global-cluster-mutation-envelopes, step task#10, DeleteGlobalCluster, status 200 -> 200:
  $.DeleteGlobalClusterResponse.DeleteGlobalClusterResult.GlobalCluster.GlobalClusterResourceId: value_mismatch
    expected: '<shaped-not-asserted>', actual 'cluster-00000000000000000000000001'
  $.DeleteGlobalClusterResponse.DeleteGlobalClusterResult.GlobalCluster.Status: value_mismatch
    expected: 'available', actual 'deleting'

The same 'cluster-00000000000000000000000001' literal is served for the CreateGlobalCluster envelope in fixtures rds-already-exists-conflict-class, rds-global-cluster-mutation-envelopes and rds-identifier-validation-and-lowercasing, which is what shows the value to be a shared counter rather than a per-resource id.

Smithy (services/service-com-amazonaws-rds-amazonrdsv19): ModifyGlobalCluster -> ModifyGlobalClusterResult{GlobalCluster}; DeleteGlobalCluster -> DeleteGlobalClusterResult{GlobalCluster}; GlobalCluster$GlobalClusterResourceId documented as the partition-unique, immutable identifier for the global database cluster.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1.
2. Causes 1: replay fixture rds-global-cluster-mutation-envelopes step task#5 (and task#7) — CreateGlobalCluster then ModifyGlobalCluster on it; inspect ModifyGlobalClusterResult.GlobalCluster.GlobalClusterResourceId. It must be the opaque per-cluster id minted at create time, identical across reads of that cluster and different from any other global cluster's, not a zero-padded counter.
3. Causes 2 and 3: replay the same fixture step task#10 — DeleteGlobalCluster on an available global cluster. Expect 200 with GlobalCluster.GlobalClusterResourceId equal to that same minted id and GlobalCluster.Status 'available' (the state at acceptance), not 'deleting'.
4. Create two global clusters in one run and compare their GlobalClusterResourceId values to confirm uniqueness.
