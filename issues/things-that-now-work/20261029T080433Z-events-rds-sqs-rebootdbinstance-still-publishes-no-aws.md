Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events__rds__sqs.xsvc-rds-1-reboot-emits-db-instance-restarted-event-to-sqs events__rds__sqs.xsvc-rds-1-input-transformer-renders-db-and-message-from-rds-event
Issue-Key: events__rds__sqs.xsvc-rds-1-reboot-emits-db-instance-restarted-event-to-sqs|run#22|TimeoutError|raise TimeoutError(f"a post-reboot {EVENT_ID} on the queue: the run budget ended first; not graded")
Issue-Key: events__rds__sqs.xsvc-rds-1-input-transformer-renders-db-and-message-from-rds-event|run#28|TimeoutError|raise TimeoutError(f"a post-reboot {EVENT_ID} on the raw target: the run budget ended first; not graded")
Issue-Commit: 9aa9958003004f44d89dbc31abda0f8624986a4f

# events__rds__sqs: RebootDBInstance still publishes no aws.rds DB Instance Event, so the raw SQS target and the InputTransformer sibling on the same rule both receive nothing (1 cause)

> _Note: this issue was identified against the code base at commit `9aa9958003004f44d89dbc31abda0f8624986a4f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RebootDBInstance

## Description
One root cause, surfaced by the two surviving `events__rds__sqs` failures of this replay (the third check of this family, xsvc-rds-1-near-miss-rules-not-delivered-after-matching-rule-receives, is marked OWNED and is skipped).

Cause 1 of 1: an accepted RebootDBInstance emits no DB instance event, so no rule on the default EventBridge bus ever delivers one and DescribeEvents never lists the restart. Failing checks: events__rds__sqs.xsvc-rds-1-reboot-emits-db-instance-restarted-event-to-sqs, events__rds__sqs.xsvc-rds-1-input-transformer-renders-db-and-message-from-rds-event.

AWS: every RDS state-machine transition publishes a DB Instance Event on the account's DEFAULT EventBridge bus. For a reboot the envelope carries source `aws.rds`, detail-type `RDS DB Instance Event`, the caller's account and region, resources `[arn:<partition>:rds:<region>:<account>:db:<db-id>]`, and a detail with EventID `RDS-EVENT-0006`, Message `DB instance restarted`, EventCategories including `availability`, SourceType `DB_INSTANCE`, SourceIdentifier `<db-id>`, SourceArn `<the instance ARN>` and Date. A rule on the default bus whose pattern matches that event delivers it to EACH of its targets independently: a raw SQS target gets the whole envelope, and a sibling target with an InputTransformer gets only the rendered template (InputPathsMap {db: $.detail.SourceIdentifier, msg: $.detail.Message} with InputTemplate '"<db> :: <msg>"' renders the quoted string `"<db-id> :: DB instance restarted"` and nothing else). The same transition is readable from the RDS control plane: Smithy (com-amazonaws-rds-amazonrdsv19) DescribeEvents takes SourceIdentifier + SourceType (enum `SourceType` includes `db-instance`) + StartTime and returns `Event` records with SourceIdentifier, SourceType, Message, EventCategories, Date and SourceArn — the same restart, with the same Date as the bus event. Recorded behavior only for the EventID/Message literals `RDS-EVENT-0006` / `DB instance restarted`, which the checks pin from real AWS; the db id, ARN, account, region and Date are values taken from the request and the emulator's own state.

Emulator: PutRule/PutTargets are accepted (FailedEntryCount 0, ListTargetsByRule shows both the raw and the transformed target), CreateDBInstance settles to `available` and RebootDBInstance is accepted — and then nothing is ever published. Both checks sit in their ReceiveMessage delivery loop (DELIVERY_BUDGET_S=600, polling for a detail.EventID == 'RDS-EVENT-0006' dated at/after the reboot) until the harness budget expires: `TimeoutError: a post-reboot RDS-EVENT-0006 on the queue: the run budget ended first; not graded` and `TimeoutError: a post-reboot RDS-EVENT-0006 on the raw target: the run budget ended first; not graded`. Because the raw sibling never receives the event, the InputTransformer rendering cannot even be reached, so the transformer leg is unobservable as well.

Why: every event-driven RDS integration is dead. A client observes the instance change state through the API but receives no notification — an EventBridge rule, an InputTransformer target, an SQS/SNS/Lambda consumer of aws.rds events and anything reading the same stream through DescribeEvents all see nothing, which is exactly the signal RDS automation (failover alerting, restart auditing, remediation) is built on.

Fix: the RDS state-transition path (the RebootDBInstance handler and the create/modify/delete transitions that emit the other RDS-EVENT-xxxx events) must publish an `aws.rds` / `RDS DB Instance Event` envelope onto the default EventBridge bus of the request's account and region with the detail members above, and record the same event in the store DescribeEvents projects; the EventBridge delivery path must then fan it out to EVERY target of a matching rule, rendering an InputTransformer target's template.

Related open issue that does NOT cover these two failures as filed: 20261024T030130Z-events-rds-sqs-rds-publishes-no-db-instance-events-to-the.md names the same root cause but its replay keys were recorded for the earlier failure mode (`Check timed out after 60.0s` inside `_receive`); in this run both checks fail on a different key (the run-budget TimeoutError raised from the delivery loop), so the suite no longer marks them OWNED by it. They should be closed together with it: re-replaying the two check ids in trace_ids is what confirms the fix.

Traces: events__rds__sqs.xsvc-rds-1-reboot-emits-db-instance-restarted-event-to-sqs, events__rds__sqs.xsvc-rds-1-input-transformer-renders-db-and-message-from-rds-event.

## Evidence
Expected (real AWS): within seconds of an accepted RebootDBInstance, the rule's raw SQS target receives a message whose body is the envelope {source: 'aws.rds', detail-type: 'RDS DB Instance Event', account: <caller account>, region: <request region>, resources: ['arn:<partition>:rds:<region>:<account>:db:<db-id>'], detail: {EventID: 'RDS-EVENT-0006', Message: 'DB instance restarted', EventCategories: [... 'availability'], SourceType: 'DB_INSTANCE', SourceIdentifier: '<db-id>', SourceArn: '<instance arn>', Date: <ts>}}, the InputTransformer sibling target receives exactly the body '"<db-id> :: DB instance restarted"' and nothing else, and DescribeEvents(SourceIdentifier='<db-id>', SourceType='db-instance', StartTime=<reboot>) lists the same event with the same Date, EventCategories containing 'availability', SourceArn '<instance arn>' and SourceType 'db-instance'.

Actual (emulator): no message is ever delivered to either queue and DescribeEvents is never reached. Replay output, verbatim:

  #### [error] events__rds__sqs.xsvc-rds-1-reboot-emits-db-instance-restarted-event-to-sqs
  TimeoutError: a post-reboot RDS-EVENT-0006 on the queue: the run budget ended first; not graded

  #### [error] events__rds__sqs.xsvc-rds-1-input-transformer-renders-db-and-message-from-rds-event
  TimeoutError: a post-reboot RDS-EVENT-0006 on the raw target: the run budget ended first; not graded

The assertions the checks would have reached are 'no RDS-EVENT-0006 dated after the reboot (<cutoff>) reached the queue within <N>s; received []' and 'the raw target received no post-reboot RDS-EVENT-0006 within <N>s, so the transformed queue cannot be graded'. The fixed text of the graded strings is 'RDS-EVENT-0006', 'DB instance restarted', 'aws.rds', 'RDS DB Instance Event', 'availability', 'DB_INSTANCE' and the ' :: ' separator of the template; the db id, instance ARN, account, region and Date are per-run values taken from the request and the emulator's state.

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. sqs: CreateQueue twice and set each queue's policy to allow events.amazonaws.com sqs:SendMessage from the rule ARN.
3. events: PutRule on the default bus with EventPattern {"source":["aws.rds"],"detail-type":["RDS DB Instance Event"],"detail":{"EventID":["RDS-EVENT-0006"],"SourceArn":["arn:aws:rds:<region>:<account>:db:<db-id>"]}}; PutTargets with target 'q' = the first queue (raw) and target 'xf' = the second queue with InputTransformer {InputPathsMap: {db: "$.detail.SourceIdentifier", msg: "$.detail.Message"}, InputTemplate: '"<db> :: <msg>"'}.
4. rds: CreateDBInstance <db-id> (mysql, db.t4g.micro), wait for DBInstanceStatus 'available', then RebootDBInstance(DBInstanceIdentifier=<db-id>).
5. sqs: ReceiveMessage on both queues in a loop. Observe: neither queue ever receives a message — the raw envelope never arrives and the transformed body '"<db-id> :: DB instance restarted"' never arrives.
6. rds: DescribeEvents(SourceIdentifier=<db-id>, SourceType='db-instance', StartTime=<the reboot instant>). Observe: the restart event is not listed.
