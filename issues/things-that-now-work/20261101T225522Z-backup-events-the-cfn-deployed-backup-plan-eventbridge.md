Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: backup__events.cfn-deploy-backup-plan-with-eventbridge-rule-target
Issue-Key: backup__events.cfn-deploy-backup-plan-with-eventbridge-rule-target|_poll_until#17|Timeout|last = probe()
Issue-Commit: 82a9e97b43fb811c245308aabae5044dc3bd73ae

# backup__events: the CFN-deployed Backup plan + EventBridge rule -> SNS -> SQS pipeline never converges, so the check exhausts its budget with no 'Backup Job State Change' event delivered

> _Note: this issue was identified against the code base at commit `82a9e97b43fb811c245308aabae5044dc3bd73ae`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartBackupJob

## Description
One root cause behind the one net-new failing check of the `backup__events` group in this replay (every other failure in the group is marked ALREADY FILED for a scaffold stub, or OWNED by an existing issue, and is not re-filed here).

Note for the fixer: the check id below also appears in the trace list of the open issue 20261029T175130Z-backup-events-no-backup-copy-restore-recovery-point.md (its cause 1), but this replay did NOT mark the check as OWNED — its replay key moved from a step-level poll timeout to a check-level budget exhaustion — so it is filed here to carry the current replay key. Closing it means making THIS check converge; it is the same emission gap that issue describes, and 20261024T031359Z-backup-events-aws-backup-job-lifecycle-and-eventing-diverge-jobs.md and 20261027T143015Z-backup-events-the-cli-driven-backup-plan-eventbridge-rule-target.md observe through other paths.

=== Cause 1 of 1: a Backup job driven from a CloudFormation-provisioned vault/plan publishes no state-change event onto the EventBridge default bus, so a rule -> SNS -> SQS pipeline declared in the same stack delivers nothing and the whole flow hangs ===
AWS: recorded behavior only for the event envelope (it is not in either service's Smithy model). After an on-demand backup job on a DynamoDB table reaches the terminal state COMPLETED, AWS Backup publishes an event on the default event bus whose `source` is `aws.backup` and whose `detail-type` is `Backup Job State Change`, with a `detail` carrying at least `backupJobId`, `state`, `resourceArn` and `backupVaultName`. Those values are the same ones the control plane reports: Smithy DescribeBackupJob returns `BackupJobId` and `State` (target shape `BackupJobState`, an enum) and `ResourceArn`/`BackupVaultName`, and the event's detail mirrors them. An ENABLED rule whose pattern is {"source":["aws.backup"],"detail-type":["Backup Job State Change"]} delivers that envelope UNCHANGED to each of its targets — here an SNS topic that a subscribed SQS queue receives with RawMessageDelivery — so sqs.ReceiveMessage yields a body whose `source`, `detail-type`, `detail.backupJobId` and `detail.state` match the job just completed. The stack resources the pipeline is built from (AWS::Backup::BackupVault, AWS::DynamoDB::Table, AWS::IAM::Role, AWS::SNS::Topic + Subscription + TopicPolicy, AWS::SQS::Queue + QueuePolicy, AWS::Events::Rule with inline Targets) all exist and are usable once the stack reports CREATE_COMPLETE.
Emulator: no event is ever published for a backup job state transition. The backup job lifecycle writes vault, job and recovery-point state only and makes no emission onto the EventBridge default bus, so the check's receive loop never sees a message and the check is reported as `Error: Check timed out after 60.0s` — it never reaches any of its four assertions on the delivered envelope. (The check's own budget allows 300 s for the stack, 120 s for StartBackupJob and 900 s for the job, so the exhaustion is the wait for something that never arrives, not slow progress. Hypothesis, since the replay output carries no step: the wait that never completes is the SQS receive for the COMPLETED event, the same wait that times out in the sibling CFN check cfn-deploy-backup-plan-with-eventbridge-notification.)
Why: a client that provisions its Backup notification wiring from a template and reacts to job completion — notify-on-completion, a queue consumer, a fan-out, a remediation — observes absolutely nothing, with no error distinguishing 'not delivered' from 'not yet', and hangs forever. Worse, a suppression or negative-permission test built on the same bus passes vacuously against an emulator that delivers nothing at all.
Fix: the backup job lifecycle's event-emission path in the backup crate's StartBackupJob handler / job state machine (publish a state-change event onto the EventBridge default bus on every job state transition, with AWS's `source`, `detail-type` and `detail` members), together with the EventBridge rule-matching and target-dispatch path that must deliver one matched event to every target of a rule, including an SNS topic that forwards to its SQS subscription. If the CloudFormation provisioning of any resource in this template is what stalls instead, that resource's provisioning in the CFN resource layer is the second place to look — the stack must reach CREATE_COMPLETE with Outputs VaultName/TableArn/RoleArn/TopicArn/QueueUrl resolvable.
Traces: backup__events.cfn-deploy-backup-plan-with-eventbridge-rule-target.

## Evidence
Suite: backup__events 8/50 (16%), 42 failed; exactly one of the listed failures is net-new (all others are ALREADY FILED stub clusters or OWNED by an open issue).

[mismatch] backup__events.cfn-deploy-backup-plan-with-eventbridge-rule-target
  Error: Check timed out after 60.0s

What the check requires and what it got:
  - cloudformation.CreateStack of the template (AWS::Backup::BackupVault, AWS::DynamoDB::Table, AWS::IAM::Role trusting backup.amazonaws.com, AWS::SNS::Topic + AWS::SNS::Subscription(protocol=sqs, RawMessageDelivery=true) + AWS::SNS::TopicPolicy, AWS::SQS::Queue + AWS::SQS::QueuePolicy, AWS::Events::Rule(EventPattern {"source":["aws.backup"],"detail-type":["Backup Job State Change"]}, Targets=[{Id:AlarmTopicTarget, Arn:<topic>}])) then backup.StartBackupJob on the table ARN with the stack's role, then DescribeBackupJob polled to a terminal state.
  - Expected after the job is COMPLETED: sqs.ReceiveMessage on the stack's queue yields a body with source == 'aws.backup', detail-type == 'Backup Job State Change', detail.backupJobId == the started BackupJobId, detail.state == 'COMPLETED'.
  - Actual: no message is ever received; the check is killed at its 60 s budget before any assertion runs. Nothing is published onto the default bus by the backup job lifecycle, so the rule has nothing to match and the topic nothing to forward.

The ids, names, ARNs and the generated stack/vault/queue names in a single run are per-run values and are not part of the requirement; the requirement is that one envelope with the fixed `source`/`detail-type` text above and the `detail` members taken from the job's own state is delivered to each target of the matching rule.

## Steps to Reproduce
1. cloudformation.CreateStack with the template above (Parameters VaultName/TableName/TopicName/QueueName/RoleName/RuleName; Capabilities CAPABILITY_IAM, CAPABILITY_NAMED_IAM) and poll describe_stacks until CREATE_COMPLETE; read Outputs VaultName, TableArn, RoleArn, TopicArn, QueueUrl.
2. backup.StartBackupJob(BackupVaultName=<VaultName>, ResourceArn=<TableArn>, IamRoleArn=<RoleArn>) and record BackupJobId.
3. Poll backup.DescribeBackupJob(BackupJobId=...) until State is terminal; it reaches COMPLETED.
4. sqs.ReceiveMessage(QueueUrl=<QueueUrl>, MaxNumberOfMessages=10, WaitTimeSeconds=5) in a loop -> AWS delivers one envelope with source 'aws.backup', detail-type 'Backup Job State Change' and detail.backupJobId/detail.state matching step 2/3; the emulator delivers nothing and the loop never terminates (the harness kills the check at 60 s).
5. Same result with the rule targeting the SQS queue directly instead of through SNS, confirming the gap is the absent emission rather than the SNS hop.
