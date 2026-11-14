Issue-Source: trace-sweep-divergence
Issue-Trace: redshift-cluster-lifecycle-op-preconditions
Issue-Step: task#6
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: redshift-cluster-lifecycle-op-preconditions redshift-snapshot-op-preconditions-and-batch-results redshift-event-subscription-validation-and-event-catalog
Issue-Shape-Id: com.amazonaws.redshift#RedshiftServiceVersion20121201
Issue-Key: redshift|ResizeCluster|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: redshift|RotateEncryptionKey|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: redshift|ModifyClusterSnapshotSchedule|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: redshift|RestoreTableFromClusterSnapshot|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: redshift|DescribeNodeConfigurationOptions|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: redshift|CreateEventSubscription|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Commit: 029c732db8092362c2d6a7aa89bcc4c3ae279d73

# Redshift not-found refusals carry emulator-authored messages instead of AWS's per-operation wording

> _Note: this issue was identified against the code base at commit `029c732db8092362c2d6a7aa89bcc4c3ae279d73`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ResizeCluster

## Description
The emulator emits one house style of not-found message per resource kind (`Cluster <id> not found.`, `ClusterSnapshot not found: <id>`, `An Amazon SNS topic with the specified Amazon Resource Name (ARN) <arn> does not exist.`). AWS's wording is per-OPERATION, not per-resource: the same ClusterNotFound fault is worded differently by ResizeCluster, RotateEncryptionKey and ModifyClusterSnapshotSchedule, and the snapshot and SNS-topic refusals have their own sentences too. Status and error code match in every cause below; only the message diverges. This issue bundles 6 causes; all 6 must be closed.

Cause 1 of 6: ResizeCluster on a cluster that does not exist.
AWS: Smithy: ResizeCluster(ClusterIdentifier required), modeled error ClusterNotFoundFault (httpError 404, awsQueryError code ClusterNotFound). Recorded message template: `Cluster not found: <ClusterIdentifier>` — fixed text `Cluster not found: ` plus the request's ClusterIdentifier.
Emulator: HTTP 404, code ClusterNotFound, message `Cluster <ClusterIdentifier> not found.` (the emulator's own template).
Why: a client or operator tool that parses AWS's `Cluster not found: <id>` form (prefix match, or splitting on `: ` to recover the identifier) extracts nothing, and golden-output tests of the CLI error line fail.
Fix: ResizeCluster's error construction in the redshift handler (use the operation's AWS template rather than the shared cluster-not-found string).
Traces: redshift-cluster-lifecycle-op-preconditions#task#6.

Cause 2 of 6: RotateEncryptionKey on a cluster that does not exist.
AWS: Smithy: RotateEncryptionKey(ClusterIdentifier required), modeled error ClusterNotFoundFault (404, code ClusterNotFound). Recorded template: `Cluster not found: <ClusterIdentifier>`.
Emulator: `Cluster <ClusterIdentifier> not found.`
Why: as cause 1, on the key-rotation path.
Fix: RotateEncryptionKey's error construction in the redshift handler.
Traces: redshift-cluster-lifecycle-op-preconditions#task#9.

Cause 3 of 6: ModifyClusterSnapshotSchedule on a cluster that does not exist.
AWS: Smithy: ModifyClusterSnapshotSchedule(ClusterIdentifier required), modeled error ClusterNotFoundFault (404, code ClusterNotFound). Recorded template: `Cluster '<ClusterIdentifier>' not found.` — fixed text with the identifier SINGLE-QUOTED, which is this operation's own form.
Emulator: `Cluster <ClusterIdentifier> not found.` (unquoted).
Why: shows that the emulator has a single global template; a client matching the quoted form fails, and the divergence is invisible to anyone who only checked one operation.
Fix: ModifyClusterSnapshotSchedule's error construction in the redshift handler.
Traces: redshift-cluster-lifecycle-op-preconditions#task#12.

Cause 4 of 6: RestoreTableFromClusterSnapshot naming a cluster that does not exist.
AWS: Smithy: RestoreTableFromClusterSnapshot(ClusterIdentifier, SnapshotIdentifier, SourceDatabaseName, SourceTableName, NewTableName required), modeled errors ClusterNotFoundFault (404, ClusterNotFound) and ClusterSnapshotNotFoundFault (404, ClusterSnapshotNotFound). Recorded template for the cluster refusal: `Cluster not found: <ClusterIdentifier>`.
Emulator: `Cluster <ClusterIdentifier> not found.`
Why: as cause 1, on the table-restore path.
Fix: RestoreTableFromClusterSnapshot's error construction in the redshift handler.
Traces: redshift-snapshot-op-preconditions-and-batch-results#task#8.

Cause 5 of 6: DescribeNodeConfigurationOptions naming a snapshot that does not exist.
AWS: Smithy: DescribeNodeConfigurationOptions(ActionType required; SnapshotIdentifier optional), modeled error ClusterSnapshotNotFoundFault (httpError 404, awsQueryError code ClusterSnapshotNotFound). Recorded template: `Snapshot not found: <SnapshotIdentifier>` — fixed text `Snapshot not found: ` plus the request's snapshot identifier.
Emulator: HTTP 404, correct code, message `ClusterSnapshot not found: <SnapshotIdentifier>` — it uses the modeled shape name (`ClusterSnapshot`) as the noun instead of AWS's `Snapshot`.
Why: a client matching the AWS sentence fails; the emulator's noun is an internal shape name AWS never puts on the wire for this operation.
Fix: DescribeNodeConfigurationOptions' snapshot-not-found error construction in the redshift handler.
Traces: redshift-snapshot-op-preconditions-and-batch-results#task#9.

Cause 6 of 6: CreateEventSubscription whose SnsTopicArn names no SNS topic.
AWS: Smithy: CreateEventSubscription(SubscriptionName, SnsTopicArn required), modeled error SNSTopicArnNotFoundFault (httpError 404, awsQueryError code SNSTopicArnNotFound). Recorded template: `Failed to create Subscription. Topic: <SnsTopicArn> not found.` — fixed text `Failed to create Subscription. Topic: ` + the request's SnsTopicArn + ` not found.`
Emulator: HTTP 404 with the right code but the message `An Amazon SNS topic with the specified Amazon Resource Name (ARN) <SnsTopicArn> does not exist.`, which is the emulator's own sentence (it reads like model documentation, not AWS's refusal).
Why: a client that keys off AWS's `Failed to create Subscription.` prefix to distinguish subscription-creation failures from later topic problems never matches.
Fix: CreateEventSubscription's SNS-topic validation error in the redshift handler.
Traces: redshift-event-subscription-validation-and-event-catalog#task#1.

Related but NOT covering these: 20261108T233629Z-redshift-cluster-identifier-refusals-diverge-no-identifier-syntax.md and 20261109T041550Z-redshift-failoverprimarycompute-clusternotfound-wording-enablelogging-cloudwatch-creates-no.md cover ClusterNotFound wording for DescribeLoggingStatus/FailoverPrimaryCompute only; 20261025T194435Z-redshift-createeventsubscription-accepts-an-snstopicarn-naming-a-nonexistent.md is about acceptance (the emulator now refuses, only the wording is wrong) and 20261114T021829Z-redshift-5-net-new-behavioral-causes-failoverprimarycompute-clusternotfound.md is a behavioral finding. None of them carries the replay keys listed here — the replay reports all 6 clusters unowned.

## Evidence
Replay of trace set simparity-wire-aws-v1; all 6 clusters reported with empty owned_by and empty stub_path. Status and Error.Code matched in every case; only $.ErrorResponse.Error.Message diverged:

1. redshift-cluster-lifecycle-op-preconditions#task#6 ResizeCluster — 404 vs 404; expected 'Cluster not found: sim2real-probe-cluster-missing', got 'Cluster sim2real-probe-cluster-missing not found.'
2. redshift-cluster-lifecycle-op-preconditions#task#9 RotateEncryptionKey — expected 'Cluster not found: sim2real-probe-cluster-missing', got 'Cluster sim2real-probe-cluster-missing not found.'
3. redshift-cluster-lifecycle-op-preconditions#task#12 ModifyClusterSnapshotSchedule — expected "Cluster 'sim2real-probe-cluster-missing' not found.", got 'Cluster sim2real-probe-cluster-missing not found.'
4. redshift-snapshot-op-preconditions-and-batch-results#task#8 RestoreTableFromClusterSnapshot — expected 'Cluster not found: sim2real-probe-cluster-missing', got 'Cluster sim2real-probe-cluster-missing not found.'
5. redshift-snapshot-op-preconditions-and-batch-results#task#9 DescribeNodeConfigurationOptions — expected 'Snapshot not found: sim2real-probe-snap-missing', got 'ClusterSnapshot not found: sim2real-probe-snap-missing'
6. redshift-event-subscription-validation-and-event-catalog#task#1 CreateEventSubscription — 404 vs 404; expected 'Failed to create Subscription. Topic: arn:aws:sns:us-west-2:000000000000:sim2real-probe-topic-missing not found.', got 'An Amazon SNS topic with the specified Amazon Resource Name (ARN) arn:aws:sns:us-west-2:000000000000:sim2real-probe-topic-missing does not exist.'

Fixed vs variable text: the fixed parts are `Cluster not found: `, `Cluster '<id>' not found.`, `Snapshot not found: `, and `Failed to create Subscription. Topic: ... not found.`; the identifiers/ARNs above (sim2real-probe-cluster-missing, sim2real-probe-snap-missing, the SNS ARN) are values taken from the request and are not themselves requirements.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus for trace set simparity-wire-aws-v1.
2. Causes 1-3: with no cluster of that name in state, call ResizeCluster, RotateEncryptionKey and ModifyClusterSnapshotSchedule with ClusterIdentifier=<missing cluster> (fixture redshift-cluster-lifecycle-op-preconditions, steps task#6, task#9, task#12). Each returns 404/ClusterNotFound with 'Cluster <id> not found.' instead of AWS's per-operation wording ('Cluster not found: <id>', and the single-quoted form for ModifyClusterSnapshotSchedule).
3. Causes 4-5: call RestoreTableFromClusterSnapshot with a missing ClusterIdentifier and DescribeNodeConfigurationOptions with a missing SnapshotIdentifier (fixture redshift-snapshot-op-preconditions-and-batch-results, steps task#8 and task#9).
4. Cause 6: call CreateEventSubscription with SnsTopicArn pointing at a topic that does not exist (fixture redshift-event-subscription-validation-and-event-catalog#task#1).
5. Re-replay those 6 fixture steps after the fix; all must pass.
