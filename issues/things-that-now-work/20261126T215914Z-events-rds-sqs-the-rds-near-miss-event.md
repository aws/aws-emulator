Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events__rds__sqs.xsvc-rds-1-near-miss-rules-not-delivered-after-matching-rule-receives
Issue-Key: events__rds__sqs.xsvc-rds-1-near-miss-rules-not-delivered-after-matching-rule-receives|_receive#23|Timeout|msgs = sqs.receive_message(QueueUrl=url, MaxNumberOfMessages=10,
Issue-Commit: 043cf7bbddd41f9e9269741e32e8c49c8ecdcab2

# events__rds__sqs: the RDS near-miss event-pattern scenario reaches no verdict — the 60 s budget is consumed before the matching rule ever receives RDS-EVENT-0006

> _Note: this issue was identified against the code base at commit `043cf7bbddd41f9e9269741e32e8c49c8ecdcab2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RebootDBInstance

## Description
Cause 1 of 1: the RDS DB-instance lifecycle / RDS->EventBridge notification span never settles inside a client-sized poll, so the whole negative-delivery (near-miss pattern) scenario is killed before it reaches a single assertion.

AWS:      RebootDBInstance is modelled to return the DBInstance envelope and to refuse only with DBInstanceNotFoundFault (404, awsQueryError code DBInstanceNotFound), InvalidDBInstanceStateFault (400, InvalidDBInstanceState) or KMSKeyNotAccessibleFault (400) — Smithy: rds RebootDBInstance -> RebootDBInstanceResult{DBInstance}. On AWS a CreateDBInstance settles to DescribeDBInstances[].DBInstanceStatus == "available", and a reboot of an available instance publishes one RDS event on the DEFAULT EventBridge bus with source "aws.rds", detail-type "RDS DB Instance Event" and detail {EventID: "RDS-EVENT-0006", SourceArn: <the db ARN>, Date: <event time>}; an EventBridge rule whose pattern matches those three fields exactly receives it, and rules that differ in exactly one of EventID, SourceArn or detail-type receive nothing. The event body itself is recorded behavior only (the Smithy model says nothing about the EventBridge envelope).

Emulator: the check never produces a verdict. The replay reports only "Error: Check timed out after 60.0s" — the entire harness budget is consumed and no assertion in the check body is reached. The two places the body can block both have budgets far larger than the cap: _wait_available() polls DescribeDBInstances every 2 s for up to 1200 s waiting for DBInstanceStatus == "available", and _await_post() polls ReceiveMessage for up to 600 s waiting for the first RDS-EVENT-0006 dated at/after the reboot's server Date. Whichever of those two never completes eats the whole 60 s cap, so the emulator's answer is "no progress" rather than a wrong value. Hypothesis: either the created DB instance never transitions to DBInstanceStatus "available" (so the check never gets as far as the reboot), or RebootDBInstance succeeds but no "RDS DB Instance Event" / RDS-EVENT-0006 is ever put on the default event bus, so the exactly-matching rule's queue stays empty.

Why:      a client that relies on AWS's behavior gets two things wrong. First, a caller that creates a DB instance and polls for "available" with a normal client timeout never sees the instance become usable. Second, no consumer can ever observe an RDS DB Instance Event: every EventBridge rule built on source "aws.rds" / detail-type "RDS DB Instance Event" routes nothing, and because the positive control never fires, no negative-delivery claim about RDS event patterns (wrong EventID, wrong SourceArn, wrong detail-type) can be attributed to the pattern at all.

Fix:      the rds DB-instance lifecycle state machine that owns the create/reboot status transitions reported by DescribeDBInstances, and the rds -> EventBridge notification publication path that must emit RDS-EVENT-0006 on the default bus when RebootDBInstance completes. Both asynchronous spans must settle inside a client-sized poll rather than an AWS-wall-clock one.

Traces:   events__rds__sqs.xsvc-rds-1-near-miss-rules-not-delivered-after-matching-rule-receives (the only failing check in this service group; it is neither ALREADY FILED nor OWNED by an open issue). Related but NOT covering: several open rds issues record endpoint/port/catalog divergences (e.g. 20261123T220118Z-rds-3-net-new-causes-rebootdbinstance-publishes-in.md covers RebootDBInstance publishing in-sync status and an OS-assigned port) — none of them covers the RDS -> EventBridge event publication or this check's non-termination.

## Evidence
Expected (AWS): create_db_instance -> describe_db_instances reports DBInstanceStatus "available"; reboot_db_instance returns 200 with the DBInstance envelope; within the delivery budget the exactly-matching rule's SQS queue receives one envelope with source "aws.rds", detail-type "RDS DB Instance Event", detail.EventID "RDS-EVENT-0006", detail.SourceArn equal to arn:<partition>:rds:<region>:<account>:db:<db-id> and detail.Date at/after the reboot; the three near-miss rules' queues report ApproximateNumberOfMessages "0" and ApproximateNumberOfMessagesNotVisible "0"; after each near-miss rule is repaired to the matching pattern, a second reboot delivers to those same queues.

Actual (emulator): no assertion is reached. Replay verdict for events__rds__sqs.xsvc-rds-1-near-miss-rules-not-delivered-after-matching-rule-receives is [mismatch] with detail "Error: Check timed out after 60.0s". The 60 s harness cap expires inside one of the two in-body polls (_wait_available, 1200 s budget, polling DescribeDBInstances for DBInstanceStatus == "available"; or _await_post, 600 s budget, polling ReceiveMessage for the first post-reboot RDS-EVENT-0006), so no status value and no delivered envelope are ever observed. No HTTP status or error code diverges here — every call the check makes before the stall is accepted; the divergence is that the state transition and/or the event delivery never happens.

## Steps to Reproduce
1. sts GetCallerIdentity to get <account>; derive db_arn = arn:aws:rds:<region>:<account>:db:<db-id>.
2. events PutRule R_match with EventPattern {"source":["aws.rds"],"detail-type":["RDS DB Instance Event"],"detail":{"EventID":["RDS-EVENT-0006"],"SourceArn":[db_arn]}}, State ENABLED; create an SQS queue, set a queue Policy allowing events.amazonaws.com sqs:SendMessage scoped to the rule ARN, and PutTargets the queue ARN.
3. Repeat step 2 for three near-miss rules that differ from R_match in exactly one field: EventID "RDS-EVENT-0087"; SourceArn <db_arn>-absent; detail-type "RDS DB Snapshot Event".
4. Create a VPC with two subnets in two AZs that offer postgres db.t4g.micro, an rds DBSubnetGroup over them, then rds CreateDBInstance <db-id> (postgres, db.t4g.micro, AllocatedStorage 20, MultiAZ false, BackupRetentionPeriod 0).
5. Poll rds DescribeDBInstances <db-id> every 2 s for DBInstanceStatus == "available". Observe whether it ever reports "available".
6. Call rds RebootDBInstance <db-id>, note the response's HTTP Date header as the cutoff, then poll ReceiveMessage on R_match's queue for an envelope whose detail.EventID is "RDS-EVENT-0006" and whose detail.Date is at/after the cutoff. Observe that none arrives.
7. Both polls together exhaust the 60 s replay budget, so the check is killed with "Check timed out after 60.0s" and none of its near-miss/repair assertions is evaluated.
