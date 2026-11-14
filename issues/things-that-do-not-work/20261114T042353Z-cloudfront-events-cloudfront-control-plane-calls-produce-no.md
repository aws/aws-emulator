Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudfront__events.xsvc-cloudfront-1-read-only-calls-not-delivered-to-enabled-rule
Issue-Key: cloudfront__events.xsvc-cloudfront-1-read-only-calls-not-delivered-to-enabled-rule|_await_envelope#27|Timeout|time.sleep(_POLL_INTERVAL_S)
Issue-Commit: 7446e1137d863a6c5f0117bdbe1b0c65f0c299cf

# cloudfront__events: CloudFront control-plane calls produce no CloudTrail-derived EventBridge events, so no envelope ever reaches a rule's SQS target and the read-only-suppression check cannot progress

> _Note: this issue was identified against the code base at commit `7446e1137d863a6c5f0117bdbe1b0c65f0c299cf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutRule

## Description
Cause 1 of 1 — CloudFront API activity is never published as "AWS API Call via CloudTrail" events onto the EventBridge default bus, so neither the positive leg (a mutating CloudFront call delivered to a rule) nor the barrier record a routing negative depends on ever arrives, and the check makes no progress at all.

AWS: CloudFront is a global service whose management (control-plane) API calls are recorded by CloudTrail in us-east-1, and CloudTrail-derived records are published to that region's default event bus as events with source "aws.cloudfront" and detail-type "AWS API Call via CloudTrail", whose detail carries eventName, requestID and readOnly. EventBridge then filters them by the rule's State. Smithy (eventbridge): PutRuleRequest.State targets enum com.amazonaws.eventbridge#RuleState with values ENABLED | DISABLED | ENABLED_WITH_ALL_CLOUDTRAIL_MANAGEMENT_EVENTS, documented as: DISABLED — "EventBridge does not match any events against the rule"; ENABLED — "EventBridge matches events against the rule, except for Amazon Web Services management events delivered through CloudTrail"; ENABLED_WITH_ALL_CLOUDTRAIL_MANAGEMENT_EVENTS — "The rule is enabled for all events, including Amazon Web Services management events delivered through CloudTrail". So AWS's required behavior is three-way: a read (GetDistribution, ListDistributions, readOnly true) must NOT reach a rule in state ENABLED but MUST reach a rule in state ENABLED_WITH_ALL_CLOUDTRAIL_MANAGEMENT_EVENTS; a write (TagResource, readOnly false) must reach both; and a rule whose pattern names detail-type "AWS Service Event via CloudTrail" must match none of them. Each matching rule must deliver the envelope to its SQS target when the queue policy admits events.amazonaws.com for that rule ARN.

Emulator: no envelope of any kind reaches any of the three SQS sinks. The check first proves the channel live by repeatedly issuing UpdateOriginAccessControl on a barrier OAC and waiting for that record to reach every sink; that record never arrives, so the check stays in its delivery poll and is cut off by the replay runner ("Check timed out after 60.0s"). The setup calls it depends on (CreateDistributionWithTags, CreateOriginAccessControl/UpdateOriginAccessControl, CreateTrail/StartLogging with IsLogging true, PutRule, PutTargets, CreateQueue/SetQueueAttributes) all succeed, so the emulator reports a fully wired pipeline and then delivers nothing. Hypothesis: the CloudFront emulator emits no management events at all, so EventBridge has nothing to ingest and the State distinction above is never exercised; the fixer should confirm by re-replaying the check after the publisher exists.

Why: a client cannot use EventBridge over CloudFront activity for audit, drift detection or automation — no event ever arrives on any rule. Worse, the absence that AWS guarantees (reads suppressed on an ENABLED rule) is indistinguishable from total non-delivery, so a client that tests its own filtering against this emulator sees a green result while the real pipeline would behave differently, and a rule created with State ENABLED_WITH_ALL_CLOUDTRAIL_MANAGEMENT_EVENTS silently yields nothing.

Fix: the CloudFront management-event publisher in services/service-com-amazonaws-cloudfront-cloudfront2020-05-31 (the component that must emit source "aws.cloudfront", detail-type "AWS API Call via CloudTrail" records with eventName/requestID/readOnly for control-plane calls, read and write alike) and the EventBridge default-bus ingestion plus rule-State filtering and SQS target dispatch in services/service-com-amazonaws-eventbridge-awsevents.

Related but NOT covering this failure: open issue 20261025T002357Z-cloudfront-events-cloudfront-read-only-calls-emit-no-events-and.md names this check in its trace list but its replay keys bind to a different failure site in a different corpus, so the suite does not report this check as owned by it; 20261024T182555Z-cloudfront-events-a-matched-eventbridge-rule-never-produces.md and 20261107T042814Z-cloudfront-events-the-in-lambda-aws-sdk-has.md cover matched-rule delivery and in-Lambda SDK gaps, not CloudFront management-event publication or the ENABLED vs ENABLED_WITH_ALL_CLOUDTRAIL_MANAGEMENT_EVENTS distinction.

Traces: cloudfront__events.xsvc-cloudfront-1-read-only-calls-not-delivered-to-enabled-rule.

## Evidence
Suite report: [mismatch] cloudfront__events.xsvc-cloudfront-1-read-only-calls-not-delivered-to-enabled-rule — "Error: Check timed out after 60.0s". The check never reaches any of its behavioral assertions; it is still inside the readiness poll that drains the three SQS sinks.

Expected (AWS): after CreateTrail/StartLogging (IsLogging true) in us-east-1 with IncludeGlobalServiceEvents true, each CloudFront control-plane call produces one default-bus envelope with source "aws.cloudfront", detail-type "AWS API Call via CloudTrail" and detail.eventName equal to the call name (UpdateOriginAccessControl for the barrier, GetDistribution/ListDistributions for the reads with detail.readOnly true, TagResource for the write with detail.readOnly false) and detail.requestID equal to the caller's ResponseMetadata.RequestId. The ENABLED rule's queue receives only the write; the ENABLED_WITH_ALL_CLOUDTRAIL_MANAGEMENT_EVENTS rule's queue receives the two reads and the write; the "AWS Service Event via CloudTrail" rule's queue receives none of them.

Actual (emulator): zero messages on all three queues, including for the barrier UpdateOriginAccessControl record that is re-issued every 30 s, so the check cannot even establish its positive control and ends at the runner cap. Every preceding call returns 2xx: CreateDistributionWithTags, PutRule (State ENABLED and State ENABLED_WITH_ALL_CLOUDTRAIL_MANAGEMENT_EVENTS), PutTargets, CreateQueue/GetQueueAttributes/SetQueueAttributes, CreateTrail, StartLogging (GetTrailStatus.IsLogging true), CreateOriginAccessControl, GetOriginAccessControl, UpdateOriginAccessControl, GetDistribution, ListDistributions, TagResource. No error code or non-2xx status is returned anywhere — the divergence is silent non-delivery, not a refusal.

## Steps to Reproduce
All calls in us-east-1 (CloudFront's CloudTrail region).
1. CreateQueue(q) three times; GetQueueAttributes for each QueueArn.
2. PutRule(Name=rNarrow, State="ENABLED", EventPattern={"source":["aws.cloudfront"],"detail-type":["AWS API Call via CloudTrail"],"detail":{"eventName":["GetDistribution","ListDistributions","TagResource"]}}); PutRule(Name=rWide, same pattern, State="ENABLED_WITH_ALL_CLOUDTRAIL_MANAGEMENT_EVENTS"); PutRule(Name=rMiss, same eventName list but detail-type ["AWS Service Event via CloudTrail"], State="ENABLED"). Add, per sink, a barrier rule on detail.eventName ["UpdateOriginAccessControl"] in State ENABLED.
3. PutTargets(Rule=<each>, Targets=[{Id:"sink", Arn:<that sink's queue ARN>}]); SetQueueAttributes(Policy allowing Principal Service events.amazonaws.com sqs:SendMessage with Condition ArnEquals aws:SourceArn = each rule ARN).
4. CreateBucket + bucket policy for cloudtrail.amazonaws.com; CreateTrail(IsMultiRegionTrail=false, IncludeGlobalServiceEvents=true); StartLogging; GetTrailStatus -> IsLogging true.
5. CreateOriginAccessControl, then UpdateOriginAccessControl on it every 30 s and drain all three queues -> no envelope with detail.eventName "UpdateOriginAccessControl" ever arrives (AWS: it reaches every sink, since each has a barrier rule on it). The pipeline is proven dead here.
6. For completeness: GetDistribution(Id=d), ListDistributions(MaxItems="1"), TagResource(Resource=<distribution ARN>) -> AWS delivers the TagResource record to rNarrow's queue and all three records to rWide's queue, with detail.requestID matching each call's RequestId and detail.readOnly true for the reads / false for the write; the emulator delivers nothing to any queue.
