Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events__rds__sqs.xsvc-rds-1-near-miss-rules-not-delivered-after-matching-rule-receives
Issue-Key: events__rds__sqs.xsvc-rds-1-near-miss-rules-not-delivered-after-matching-rule-receives|_receive#23|Timeout|msgs = sqs.receive_message(QueueUrl=url, MaxNumberOfMessages=10,
Issue-Commit: d42870bc3a3955aaff0594ef70435ba8ae8c0650

# events__rds__sqs: the RDS near-miss EventBridge routing scenario never reaches a verdict — the check is killed at the 60 s budget

> _Note: this issue was identified against the code base at commit `d42870bc3a3955aaff0594ef70435ba8ae8c0650`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RebootDBInstance

## Description
Cause 1 of 1: the end-to-end RDS -> EventBridge -> SQS near-miss routing scenario does not converge inside the replay budget, so the check reaches no assertion at all (failing check: events__rds__sqs.xsvc-rds-1-near-miss-rules-not-delivered-after-matching-rule-receives).

AWS: the check drives a scenario every step of which AWS completes promptly. Smithy (services/service-com-amazonaws-rds-amazonrdsv19/smithy-model/com-amazonaws-rds-amazonrdsv19.json): CreateDBInstance/DescribeDBInstances (paginated by Marker, items DBInstances) must converge on DBInstance$DBInstanceStatus == "available"; RebootDBInstance (input DBInstanceIdentifier required, errors DBInstanceNotFoundFault 404 / InvalidDBInstanceStateFault 400 / KMSKeyNotAccessibleFault 400) documents "Rebooting a DB instance restarts the database engine service ... the DB instance status is set to rebooting", after which the instance returns to available. Amazon RDS additionally publishes that change onto the account's DEFAULT event bus with source "aws.rds", detail-type "RDS DB Instance Event" and detail.EventID "RDS-EVENT-0006" ("DB instance restarted", category availability), detail.SourceArn being the rebooted instance's ARN — recorded behavior only for the envelope shape, which is what the check's rule patterns pin (source, detail-type, detail.EventID, detail.SourceArn). An EventBridge rule whose pattern matches must deliver that envelope to its SQS target; the three near-miss rules (wrong EventID, wrong SourceArn, wrong detail-type) must deliver nothing, and after PutRule repoints them at the matching pattern a second reboot must reach all three queues.

Emulator: the check produces no verdict at all. The only observation the replay gives is `Error: Check timed out after 60.0s` — no HTTP status, no error code and no message, because the scenario never gets as far as an assertion. The chain it has to walk before its first assertion is: DescribeDBEngineVersions(Engine=postgres, DefaultOnly) -> DescribeOrderableDBInstanceOptions (paginated) -> EC2 CreateVpc/CreateSubnet x2 -> CreateDBSubnetGroup -> CreateDBInstance -> DescribeDBInstances polled until "available" -> RebootDBInstance -> ReceiveMessage on the matching rule's queue until the post-reboot RDS-EVENT-0006 arrives. One of those steps does not settle inside the budget, so the near-miss and repair phases are never exercised. The RDS side of the publication path is present in the checkout (see Traces) but the scenario as a whole never completes, so the emulator never demonstrates it.

Why: a client cannot use this capability at all. Nothing in the run tells it whether the matching rule received, whether a near-miss rule leaked, or whether repairing a pattern starts delivery — the absence of a delivery is indistinguishable from the scenario never having run. Automation built on "reboot the instance, react to RDS-EVENT-0006 on the bus" has no way to be validated against the emulator.

Fix: the convergence of the operations this scenario polls, not the near-miss assertions themselves. Audit, in order, the step that fails to settle: RDS CreateDBInstance/DescribeDBInstances status progression to "available" (the pending-status clock in services/service-com-amazonaws-rds-amazonrdsv19/src/state.rs), RebootDBInstance's return to "available" (services/service-com-amazonaws-rds-amazonrdsv19/src/ops/reboot_db_instance.rs), the handover of the recorded event onto the default bus (services/service-com-amazonaws-rds-amazonrdsv19/src/state/bus_events.rs), EventBridge's matching of that envelope against the rule patterns, and SQS ReceiveMessage long-poll behavior on an empty queue (the check spends three 20 s long polls plus a two-round drain on empty queues, so a receive path that does not return as soon as the queue is known empty consumes the whole budget on its own). Related but NOT the same cause, and not covering this check: open issues 20261110T191704Z-rds-six-control-plane-lifecycle-flows-never-reach.md and 20261111T084329Z-rds-ten-more-control-plane-and-data-plane.md (other RDS flows that never reach a terminal status) and 20261111T033241Z-events-rds-scheduler-an-at-schedule-whose-aws.md (a scheduler target's dead-letter record).

Traces: events__rds__sqs.xsvc-rds-1-near-miss-rules-not-delivered-after-matching-rule-receives. Code cited above: services/service-com-amazonaws-rds-amazonrdsv19/src/ops/reboot_db_instance.rs lines 233-252 (status -> "rebooting", pending "available", record_event DB_INSTANCE_RESTARTED), services/service-com-amazonaws-rds-amazonrdsv19/src/state/events.rs lines 124-130 (RDS-EVENT-0006 / "DB instance restarted" / availability), services/service-com-amazonaws-rds-amazonrdsv19/src/state/bus_events.rs lines 46-76 and 158-195 (source aws.rds, detail-type "RDS DB Instance Event", publication onto the default bus, best-effort).

## Evidence
Expected (AWS): the check runs to completion. The exactly-matching rule's queue receives an envelope with source "aws.rds", detail-type "RDS DB Instance Event", detail.EventID "RDS-EVENT-0006" and detail.SourceArn equal to the rebooted instance's ARN, dated at/after the reboot; the three near-miss queues then report ApproximateNumberOfMessages "0" and ApproximateNumberOfMessagesNotVisible "0"; after PutRule repoints each near-miss rule at the matching pattern, a second RebootDBInstance delivers the same envelope to all three of those queues.

Actual (emulator): no verdict is produced. The replay reports, verbatim and as the entire detail:

  [mismatch] events__rds__sqs.xsvc-rds-1-near-miss-rules-not-delivered-after-matching-rule-receives
  Error: Check timed out after 60.0s

No HTTP status, error code or error message was returned for any step — the scenario is cut off before its first assertion, so neither the positive control nor the near-miss absences nor the repair phase is ever observed. (The 60 s figure is this run's budget, not a requirement; the requirement is that the scenario converge.)

## Steps to Reproduce
1. Boot the emulator and replay the behavioral suite (run_behavioral_suite).
2. Look under the `### events__rds__sqs` heading of the failing-checks report for `xsvc-rds-1-near-miss-rules-not-delivered-after-matching-rule-receives`.
3. Equivalently, by hand against the emulator in one region: rds DescribeDBEngineVersions(Engine=postgres, DefaultOnly=true); rds DescribeOrderableDBInstanceOptions(Engine=postgres, EngineVersion=<that version>, DBInstanceClass=db.t4g.micro) and pick two AZs it offers that ec2 DescribeAvailabilityZones reports available; ec2 CreateVpc(10.71.0.0/16) + two CreateSubnet calls; rds CreateDBSubnetGroup over them; rds CreateDBInstance(DBInstanceIdentifier=<id>, Engine=postgres, DBInstanceClass=db.t4g.micro, AllocatedStorage=20, DBSubnetGroupName=<group>); poll rds DescribeDBInstances until DBInstanceStatus == "available".
4. events PutRule with pattern {"source":["aws.rds"],"detail-type":["RDS DB Instance Event"],"detail":{"EventID":["RDS-EVENT-0006"],"SourceArn":["arn:aws:rds:<region>:<account>:db:<id>"]}}, State=ENABLED; create an SQS queue, set a Policy allowing events.amazonaws.com sqs:SendMessage with ArnEquals aws:SourceArn = the rule ARN, and events PutTargets it.
5. rds RebootDBInstance(DBInstanceIdentifier=<id>), then sqs ReceiveMessage on that queue: observe how long it takes (if ever) for an envelope with detail.EventID "RDS-EVENT-0006" and detail.Date at/after the reboot to arrive.
6. Repeat with three near-miss rules (EventID RDS-EVENT-0087; SourceArn of a nonexistent instance; detail-type "RDS DB Snapshot Event") pointed at their own queues, confirm those queues stay empty, then PutRule each to the matching pattern and reboot again — each must then receive. Time the whole sequence: it never reaches step 6 inside the replay budget.
