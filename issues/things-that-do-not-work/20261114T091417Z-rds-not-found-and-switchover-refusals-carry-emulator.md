Issue-Source: trace-sweep-divergence
Issue-Trace: rds-db-instance-core-lifecycle-and-features
Issue-Step: task#9
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: rds-db-instance-core-lifecycle-and-features rds-db-instance-target-error-paths rds-read-replica-and-instance-promotion rds-db-cluster-state-roles-and-capacity-operations rds-global-cluster-lifecycle
Issue-Shape-Id: com.amazonaws.rds#AmazonRDSv19
Issue-Key: rds|PromoteReadReplica|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: rds|RebootDBCluster|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: rds|FailoverGlobalCluster|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Commit: 029c732db8092362c2d6a7aa89bcc4c3ae279d73

# RDS not-found and switchover refusals carry emulator-authored messages instead of AWS's wording (3 causes)

> _Note: this issue was identified against the code base at commit `029c732db8092362c2d6a7aa89bcc4c3ae279d73`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PromoteReadReplica

## Description
Three RDS refusal paths return the correct HTTP status and the correct error code but an error MESSAGE the emulator wrote itself rather than the one AWS words for that error class. In each case the message's fixed text is a requirement and only the embedded identifier/ARN is a value taken from the request. There are THREE distinct causes in this bundle; each must be closed separately.

=== Cause 1 of 3: PromoteReadReplica's DBInstanceNotFound message uses the emulator's generic not-found template ===
AWS: Smithy: PromoteReadReplica models error DBInstanceNotFoundFault, errorKind client, httpError 404, awsQueryError code 'DBInstanceNotFound' httpResponseCode 404. AWS's message for this error class is 'DB Instance not found: <DBInstanceIdentifier>' — fixed text 'DB Instance not found: ' followed by the DBInstanceIdentifier taken verbatim from the request; nothing else varies.
Emulator: status 404 and the code are right, but the message is 'DBInstance <DBInstanceIdentifier> not found.' — a generic '<Type> <id> not found.' template with the resource type spelled 'DBInstance' (no space), the identifier placed mid-string, and a trailing period AWS does not emit.
Why: the message is part of the error contract. Clients and SDK-adjacent tooling that string-match the message to distinguish a genuinely absent instance from other 404 conditions, and log scrapers and test assertions written against AWS, do not match. The same generic template is used for three operations' instance-not-found path in this corpus, so the divergence is systematic rather than one handler's typo.
Fix: the DBInstanceNotFoundFault message construction shared by the instance-targeting handlers (PromoteReadReplica and the other instance operations that resolve an identifier), in the RDS error/validation layer rather than in one operation.
Traces: rds-db-instance-core-lifecycle-and-features#task#9 (representative), rds-db-instance-target-error-paths#task#6, rds-read-replica-and-instance-promotion#task#4 — 3 steps in the cluster.

=== Cause 2 of 3: RebootDBCluster's DBClusterNotFoundFault message is the generic template, not AWS's source-cluster wording ===
AWS: Smithy: RebootDBCluster models DBClusterNotFoundFault, errorKind client, httpError 404, awsQueryError code 'DBClusterNotFoundFault' httpResponseCode 404. For this operation AWS words the refusal 'The source cluster could not be found or cannot be accessed: <DBClusterIdentifier>' — fixed text 'The source cluster could not be found or cannot be accessed: ' plus the DBClusterIdentifier from the request. Note AWS deliberately does not distinguish 'absent' from 'inaccessible' in this message.
Emulator: status 404 and code are right; the message is 'DBCluster <DBClusterIdentifier> not found.' — the same generic '<Type> <id> not found.' template as cause 1, so it asserts the cluster is absent and loses the 'or cannot be accessed' wording entirely.
Why: a client that distinguishes an unreachable/cross-account cluster from a missing one by this message mis-classifies the failure, and anything asserting AWS's text fails. Because the emulator reuses one template for every resource type, correcting the instance message (cause 1) will not correct this one.
Fix: the RebootDBCluster handler's DBClusterNotFoundFault path — it needs this operation's own message template, not the shared generic one.
Traces: rds-db-cluster-state-roles-and-capacity-operations#task#6.

=== Cause 3 of 3: FailoverGlobalCluster ignores the Switchover flag when wording its DBClusterNotFoundFault message ===
AWS: Smithy: FailoverGlobalCluster input has GlobalClusterIdentifier (@required) and TargetDbClusterIdentifier (@required) plus AllowDataLoss and Switchover (BooleanOptional); it models DBClusterNotFoundFault, errorKind client, httpError 404, awsQueryError code 'DBClusterNotFoundFault'. When the target DB cluster does not exist AWS answers 404 with "A DB cluster with ARN <TargetDbClusterIdentifier ARN> doesn't exist. Choose another <mode> target and try again." — fixed text with two variable parts: the target cluster's ARN taken from the request, and the mode word, which is 'switchover' when the request asked for a switchover and 'failover' otherwise.
Emulator: status 404, code and ARN are all correct, but the mode word is hard-coded: for a switchover request it answers "... Choose another failover target and try again." where AWS answers "... Choose another switchover target and try again." The emulator is therefore not reading the request's Switchover flag when it builds the message.
Why: switchover and failover are different operations with different data-loss semantics; a refusal that names the wrong one misdirects the operator (and any automation keyed on the message) to retry the wrong kind of target. It is also evidence the Switchover flag is not threaded into this handler's refusal path at all, which is worth checking for the success path too.
Fix: the FailoverGlobalCluster handler's target-resolution/refusal path — select the mode word from the request's Switchover member.
Traces: rds-global-cluster-lifecycle#task#6.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator (emulator_up=true). In all three causes the expected and actual HTTP status agree (404 -> 404) and the error code matches; only $.ErrorResponse.Error.Message diverges.

Cause 1 — rds-db-instance-core-lifecycle-and-features#task#9, PromoteReadReplica, 404 -> 404:
  $.ErrorResponse.Error.Message: expected 'DB Instance not found: <DBInstanceIdentifier>'; got 'DBInstance <DBInstanceIdentifier> not found.'
  Fixed text: 'DB Instance not found: '. Variable part: the DBInstanceIdentifier from the request (the recorded run used a generated probe identifier; that generated name is not part of the requirement).
  affected_count = 3: rds-db-instance-core-lifecycle-and-features#task#9, rds-db-instance-target-error-paths#task#6, rds-read-replica-and-instance-promotion#task#4.

Cause 2 — rds-db-cluster-state-roles-and-capacity-operations#task#6, RebootDBCluster, 404 -> 404:
  $.ErrorResponse.Error.Message: expected 'The source cluster could not be found or cannot be accessed: <DBClusterIdentifier>'; got 'DBCluster <DBClusterIdentifier> not found.'
  Fixed text: 'The source cluster could not be found or cannot be accessed: '. Variable part: the DBClusterIdentifier from the request (a generated probe identifier in the recording).

Cause 3 — rds-global-cluster-lifecycle#task#6, FailoverGlobalCluster, 404 -> 404:
  $.ErrorResponse.Error.Message: expected "A DB cluster with ARN arn:aws:rds:us-east-1:000000000000:cluster:fake doesn't exist. Choose another switchover target and try again."; got "A DB cluster with ARN arn:aws:rds:us-east-1:000000000000:cluster:fake doesn't exist. Choose another failover target and try again."
  Fixed text: "A DB cluster with ARN " ... " doesn't exist. Choose another " ... " target and try again." Variable parts: the target cluster ARN from the request, and the mode word ('switchover' when the request sets Switchover, otherwise 'failover'). The emulator gets the ARN right and the mode word wrong, which is the whole divergence.

## Steps to Reproduce
1. git clone the repo and build/boot the emulator from this checkout.
2. Replay trace set simparity-wire-aws-v1.
3. Cause 1: replay fixture rds-db-instance-core-lifecycle-and-features step task#9 — call PromoteReadReplica with a DBInstanceIdentifier that does not exist. Assert 404, error code DBInstanceNotFound, and message exactly 'DB Instance not found: <that identifier>'. Repeat for rds-db-instance-target-error-paths#task#6 and rds-read-replica-and-instance-promotion#task#4.
4. Cause 2: replay fixture rds-db-cluster-state-roles-and-capacity-operations step task#6 — call RebootDBCluster with a DBClusterIdentifier that does not exist. Assert 404, code DBClusterNotFoundFault, and message exactly 'The source cluster could not be found or cannot be accessed: <that identifier>'.
5. Cause 3: replay fixture rds-global-cluster-lifecycle step task#6 — call FailoverGlobalCluster with Switchover set and a TargetDbClusterIdentifier ARN that does not exist. Assert 404, code DBClusterNotFoundFault, and that the message says 'Choose another switchover target and try again.'; then issue the same call WITHOUT Switchover and assert the message says 'Choose another failover target and try again.'
6. All three must pass before the issue is closed.
