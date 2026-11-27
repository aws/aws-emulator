Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudtrail__dynamodb__events__sqs.xsvc-dynamodb-3-create-table-reaches-only-its-rule-not-the-delete-table-near-miss
Issue-Key: cloudtrail__dynamodb__events__sqs.xsvc-dynamodb-3-create-table-reaches-only-its-rule-not-the-delete-table-near-miss|_collect#66|Timeout|msgs = sqs.receive_message(
Issue-Commit: b4d546d4232932d32ca317b0739ae1583d9e6982

# cloudtrail/dynamodb/events/sqs: DynamoDB control-plane calls produce no 'AWS API Call via CloudTrail' event on the default bus, so a rule keyed on CreateTable never fires

> _Note: this issue was identified against the code base at commit `b4d546d4232932d32ca317b0739ae1583d9e6982`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateTable

## Description
One net-new root cause, grounded in one failing check the suite lists under `cloudtrail__dynamodb__events__sqs`.

Cause 1 of 1: with a live multi-region trail logging management events, a DynamoDB control-plane call produces no CloudTrail→EventBridge management-event delivery, so a rule keyed on eventName/requestParameters never receives it. Failing check: cloudtrail__dynamodb__events__sqs.xsvc-dynamodb-3-create-table-reaches-only-its-rule-not-the-delete-table-near-miss.

AWS: recorded behavior only — the CloudTrail→EventBridge fan-out is not expressed in any Smithy model (documented at https://docs.aws.amazon.com/awscloudtrail/latest/userguide/cloudtrail-aws-service-specific-topics.html). DynamoDB emits no native EventBridge events, so the only path from a DynamoDB control-plane call to the default bus is CloudTrail's management-event fan-out: once a trail with management events enabled is logging, each recorded API call is published to the default bus of the calling region with source `aws.dynamodb`, detail-type the fixed string `AWS API Call via CloudTrail`, and a detail object that is the CloudTrail record — `eventSource` = the fixed token `dynamodb.amazonaws.com`, `eventName` = the called operation's name (`CreateTable`, `DeleteTable`, ...), plus `requestParameters` echoing the call (here `tableName` = the table from the request), `awsRegion`, `userIdentity` and `eventID` from state. A rule whose pattern pins detail.eventName and detail.requestParameters.tableName therefore receives the call it names, and a sibling rule pinning a different eventName receives nothing.

Emulator: after CreateTrail/StartLogging on a bucket whose policy admits cloudtrail.amazonaws.com and a rule on {source: aws.dynamodb, detail-type: 'AWS API Call via CloudTrail', detail.eventName: [CreateTable]}, the DynamoDB control-plane call is accepted but no envelope ever reaches the rule's SQS target; the check never gets past waiting for the first delivery and is cut off (mismatch, 'Check timed out after 60.0s'). Nothing arrives on the near-miss rule either, so the negative half of the assertion is vacuous rather than proved.

Why: CloudTrail management-event fan-out is the only way to react to control-plane activity of services that publish no native events (DynamoDB, IAM and the rest). Against the emulator every such rule is silently dead: audit/compliance automation, table-lifecycle reactions and near-miss routing tests can never be exercised, and a client cannot distinguish 'no such call happened' from 'the fan-out does not exist'.

Fix: the CloudTrail recording path and its EventBridge publication — a trail that is logging management events must emit each recorded call onto the default bus as an `AWS API Call via CloudTrail` envelope with the calling service's `aws.<service>` source and the CloudTrail record (eventSource, eventName, requestParameters, awsRegion) as detail; DynamoDB's control-plane handlers must feed that recorder. Context: open issues 20261029T173328Z-cloudtrail-records-and-delivers-no-audit-record-for.md and 20261125T063152Z-cloudtrail-events-iam-sqs-no-aws-api-call.md record the same missing fan-out for other services' checks; neither carries this check's replay key, so the DynamoDB arm is filed here.

Traces: cloudtrail__dynamodb__events__sqs.xsvc-dynamodb-3-create-table-reaches-only-its-rule-not-the-delete-table-near-miss (timed out waiting for the first CloudTrail-sourced delivery).

## Evidence
cloudtrail__dynamodb__events__sqs.xsvc-dynamodb-3-create-table-reaches-only-its-rule-not-the-delete-table-near-miss:
  mismatch — 'Check timed out after 60.0s'

The check creates a logging trail (S3 bucket + bucket policy admitting cloudtrail.amazonaws.com GetBucketAcl/PutObject), two SQS-backed EventBridge rules, and then performs the DynamoDB control-plane calls.
Expected on the CreateTable-scoped queue: one envelope with source 'aws.dynamodb', detail-type 'AWS API Call via CloudTrail', detail.eventSource 'dynamodb.amazonaws.com', detail.eventName 'CreateTable' and detail.requestParameters.tableName equal to the table named in the request; and nothing at all on the DeleteTable-scoped near-miss rule (and the converse for the DeleteTable call).
Actual: no CloudTrail-sourced envelope is delivered to either queue, so the check cannot reach any of its assertions and is cut off at its budget.

'AWS API Call via CloudTrail' and 'dynamodb.amazonaws.com' are fixed text; the table name, region, account and event ids are values from the request and the emulator's state.

## Steps to Reproduce
1. CreateBucket and PutBucketPolicy allowing cloudtrail.amazonaws.com s3:GetBucketAcl on the bucket and s3:PutObject under AWSLogs/<account>/*; CreateTrail (management events, multi-region) and StartLogging.
2. CreateQueue twice with policies allowing events.amazonaws.com; PutRule twice and PutTargets: rule A on {"source": ["aws.dynamodb"], "detail-type": ["AWS API Call via CloudTrail"], "detail": {"eventSource": ["dynamodb.amazonaws.com"], "eventName": ["CreateTable"], "requestParameters": {"tableName": [<table>]}}}, rule B the same but eventName DeleteTable.
3. CreateTable the named table and wait for it to become ACTIVE; then ReceiveMessage both queues.
   Expected: rule A's queue receives one 'AWS API Call via CloudTrail' envelope naming eventName CreateTable and that tableName; rule B's queue receives nothing. DeleteTable then delivers to B only.
   Emulator: neither queue ever receives a CloudTrail-sourced envelope.
