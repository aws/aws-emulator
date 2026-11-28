Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudfront__events.cloudfront-distribution-inprogress-event-filtered-by-pattern cloudfront__events.eventbridge-rule-role-arn-wrong-trust-execution-not-started
Issue-Key: cloudfront__events.cloudfront-distribution-inprogress-event-filtered-by-pattern|_drain_queue#9|Timeout|resp = sqs.receive_message(
Issue-Key: cloudfront__events.eventbridge-rule-role-arn-wrong-trust-execution-not-started|_wait_for_execution_with_marker#29|Timeout|time.sleep(3)
Issue-Commit: d9773982178bffc206ea9531196a8841a020d5e3

# cloudfront__events: 2 net-new causes — a pattern-filtered CloudFront state-change event is never delivered to its SQS target, and a target RoleArn's trust policy is never exercised for a Step Functions target

> _Note: this issue was identified against the code base at commit `d9773982178bffc206ea9531196a8841a020d5e3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutEvents

## Description
Two net-new root causes on the EventBridge target-invocation path exercised by the CloudFront eventing checks. Both checks consume their entire replay budget without ever observing the target effect AWS produces, so neither the positive delivery nor the negative (withheld / not-started) leg is reachable. Related but NON-covering open issues: 20261024T182555Z-cloudfront-events-a-matched-eventbridge-rule-never-produces.md (same family, three other checks), 20261102T091831Z-cloudfront-events-a-matched-rule-never-invokes-its.md (Lambda target), 20261121T170347Z-cloudfront-events-2-net-new-causes-an-absent.md (CreateInvalidation refusal + IAM trust readback). None of them carries the two replay keys below.

Cause 1 of 2: a rule whose EventPattern filters on a nested detail member never delivers the matching entry to its SQS target.
AWS: Smithy (eventbridge) — PutRule.EventPattern, PutTargets.Targets[].Arn (SQS queue ARN), PutEvents.Entries (PutEventsRequestEntryList @length min 1 max 10). For a rule on the default bus with EventPattern {"source":[<source>],"detail-type":["CloudFront Distribution State Change"],"detail":{"state":["Deployed"]}} and one SQS target whose queue policy allows sqs:SendMessage to events.amazonaws.com conditioned on aws:SourceArn = the rule ARN, a single PutEvents call carrying two entries of that source/detail-type — one with detail.state="InProgress", one with detail.state="Deployed" — is accepted with FailedEntryCount 0, and EventBridge delivers exactly the Deployed entry to the queue as a full event envelope; the InProgress entry matches no rule and is delivered nowhere. Recorded behavior only for the delivery latency: the match is evaluated per entry, not per request.
Emulator: the check reaches no verdict inside its budget — it is cut off at the 60.0 s cap ("Check timed out after 60.0s") while draining the target queue, with the positive-control assertion ("the Deployed event was never delivered") never satisfied. No envelope for the Deployed entry is observed on the SQS target.
Why: a client that routes service state-change events through a nested-detail pattern to a queue sees an accepted PutEvents (FailedEntryCount 0) and an empty queue. Nothing distinguishes "filtered out" from "never delivered", so a consumer blocks forever and the pattern's precision cannot be verified at all.
Fix: the EventBridge rule-matching and target-invocation path for SQS targets — the dispatcher that evaluates each PutEvents entry against enabled rules on the bus and enqueues the envelope to the target queue, plus the nested-detail matcher it calls.
Traces: cloudfront__events.cloudfront-distribution-inprogress-event-filtered-by-pattern

Cause 2 of 2: a rule target's RoleArn trust policy is not enforced (or honoured) for a Step Functions state-machine target, so neither the denied leg nor the permitted leg produces its effect.
AWS: Smithy (eventbridge) — PutTargets.Targets[].RoleArn, Targets[].Arn (a states state-machine ARN). When the target RoleArn's AssumeRolePolicyDocument trusts only lambda.amazonaws.com, EventBridge cannot assume it, starts NO execution of the state machine, and the matching event produces no execution for the lifetime of that configuration. When the same target is re-pointed at a role whose trust policy allows sts:AssumeRole for Principal Service events.amazonaws.com and which carries states:StartExecution on the state-machine ARN, the same rule starts an execution whose input is the event envelope and which reaches status SUCCEEDED for a Pass-only definition.
Emulator: the check reaches no verdict inside its budget — cut off at the 60.0 s cap ("Check timed out after 60.0s"). The bounded-absence wait for the untrusted role and the positive control with the correctly-trusted role never both complete; no execution carrying the event marker in its input is observed on the state machine.
Why: a client cannot tell a correctly-denied invocation from a never-attempted one. Granting the right trust buys no delivery, and removing it signals no failure, so IAM mistakes in a rule target are invisible in both directions and a pipeline looks broken when it is correctly configured.
Fix: the EventBridge target-invocation path for Step Functions targets — the component that assumes Targets[].RoleArn (checking the role's trust policy for events.amazonaws.com) and calls StartExecution with the event envelope as input, and the state-machine execution driver that must take a Pass-only definition to SUCCEEDED.
Traces: cloudfront__events.eventbridge-rule-role-arn-wrong-trust-execution-not-started

## Evidence
Both checks are reported as [mismatch] with detail "Error: Check timed out after 60.0s".

Cause 1 — cloudfront__events.cloudfront-distribution-inprogress-event-filtered-by-pattern:
  Expected: put_events(Entries=[{state:"InProgress",distributionId:<marker-a>},{state:"Deployed",distributionId:<marker-b>}]) returns FailedEntryCount 0; draining the SQS target yields at least one body containing <marker-b> and no body containing <marker-a>.
  Actual: no message containing either marker is observed; the check is cut off at its 60.0 s cap and its positive-control assertion ("positive control failed: the Deployed event was never delivered") is never satisfied.

Cause 2 — cloudfront__events.eventbridge-rule-role-arn-wrong-trust-execution-not-started:
  Expected: with Targets=[{Id:"sfn-target",Arn:<stateMachineArn>,RoleArn:<role trusting lambda.amazonaws.com>}], list_executions/describe_execution show NO execution whose input detail.marker equals the published marker; after PutTargets re-points the same Id at a role trusting events.amazonaws.com with states:StartExecution on <stateMachineArn>, a matching event starts an execution whose describe_execution status becomes SUCCEEDED.
  Actual: no execution carrying any published marker is ever observed; the check is cut off at its 60.0 s cap, so the positive control ("positive control with a correctly-trusted role must start an execution") never fires.

The marker strings, role names, state-machine names and queue names above are per-run generated values; only the shape of the assertions is the requirement.

## Steps to Reproduce
Cause 1:
1. events.put_rule(Name=R, EventPattern={"source":[S],"detail-type":["CloudFront Distribution State Change"],"detail":{"state":["Deployed"]}}, State="ENABLED").
2. sqs.create_queue(Q); read QueueArn; sqs.set_queue_attributes(Policy allowing sqs:SendMessage to Service events.amazonaws.com with Condition ArnEquals aws:SourceArn = the rule ARN).
3. events.put_targets(Rule=R, Targets=[{Id:"sqs-target",Arn:<queue arn>}]).
4. events.put_events with two entries of source S and detail-type "CloudFront Distribution State Change": one Detail {"state":"InProgress","distributionId":A}, one Detail {"state":"Deployed","distributionId":B}. Assert FailedEntryCount == 0.
5. Receive from Q: expect a body containing B and never one containing A. Observed: nothing arrives.

Cause 2:
1. iam.create_role(SM_ROLE, trust states.amazonaws.com); sfn.create_state_machine(definition {StartAt:"Echo",States:{Echo:{Type:"Pass",End:true}}}, roleArn=SM_ROLE).
2. iam.create_role(BAD, trust lambda.amazonaws.com only) + put_role_policy allowing states:StartExecution on the state-machine ARN.
3. events.put_rule(Name=R, EventPattern={"source":[S],"detail-type":["CloudFront Distribution Event (Simulated)"],"detail":{"eventName":["CreateInvalidation"]}}); events.put_targets(Rule=R, Targets=[{Id:"sfn-target",Arn:<sm arn>,RoleArn:<BAD arn>}]).
4. events.put_events a matching entry with a unique detail.marker; poll sfn.list_executions/describe_execution — expect NO execution with that marker.
5. iam.create_role(GOOD, trust events.amazonaws.com) + same inline policy; events.put_targets same Id with RoleArn=<GOOD arn>; publish a fresh matching entry; expect an execution with that marker reaching SUCCEEDED. Observed: no execution ever appears and the check is cut off at its 60 s cap.
