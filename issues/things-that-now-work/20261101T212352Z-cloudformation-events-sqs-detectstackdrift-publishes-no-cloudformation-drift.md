Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__events__sqs.xsvc-cloudformation-2-drift-detection-status
Issue-Key: cloudformation__events__sqs.xsvc-cloudformation-2-drift-detection-status|_receive#19|Timeout|return out
Issue-Commit: 446e5d40b4aa72232e72e50ee85b856a1859ebd6

# cloudformation__events__sqs: DetectStackDrift publishes no "CloudFormation Drift Detection Status Change" event to EventBridge (1 cause)

> _Note: this issue was identified against the code base at commit `446e5d40b4aa72232e72e50ee85b856a1859ebd6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DetectStackDrift

## Description
One root cause of service `cloudformation__events__sqs`, grounded in the failing behavioral check in trace_ids. It was not marked ALREADY FILED or OWNED by the replay report. The open issue 20261029T182228Z-cloudformation-events-sqs-cloudformation-publishes-no-stack-resource-status-change-or.md covers CloudFormation's stack/resource status-change events; the drift-detection event family graded here is a different event type and is not carried by it.

=== Cause 1 of 1: a drift detection emits no event on the default EventBridge bus ===
AWS: not in the Smithy model — DetectStackDrift's modeled contract (services/service-com-amazonaws-cloudformation-cloudformation/smithy-model/com-amazonaws-cloudformation-cloudformation.json; input StackName + LogicalResourceIds, output StackDriftDetectionId, no modeled errors) says nothing about eventing, so this is recorded behavior only. Every DetectStackDrift emits TWO events to the default bus with `source` `aws.cloudformation` and `detail-type` `CloudFormation Drift Detection Status Change`, both joined to the call by `detail.stack-id` (the stack ARN) and `detail.stack-drift-detection-id` (the id the call returned):
  * on the detection starting: `detail.status-details.detection-status` = `DETECTION_IN_PROGRESS`, `detail.status-details.stack-drift-status` = `""` (empty — no verdict yet) and `detail.drift-detection-details.drifted-stack-resource-count` = `-1`;
  * on it concluding: `detection-status` = `DETECTION_COMPLETE` with `stack-drift-status` = the verdict (`IN_SYNC` or `DRIFTED`) and `drifted-stack-resource-count` = the real count (0 when IN_SYNC, 1 for a single out-of-band-modified resource).
The envelope carries the standard EventBridge fields (version, id, detail-type, source, account, time, region, resources). Because the events go through the bus, a rule whose pattern filters `detail.status-details.stack-drift-status: ["DRIFTED"]` must receive the DRIFTED detection's events and must NOT receive an IN_SYNC detection's events. The two events can share the same second-granularity `time` and arrive in any order.
Emulator: no such event is ever published. `src/ops/detect_stack_drift.rs` is implemented — it mints the detection id, records a StoredDriftDetection with DETECTION_IN_PROGRESS, performs the per-resource comparison through `crate::state::drift::detect` and stores a PendingDriftOutcome that becomes DETECTION_COMPLETE on the clock — but it contains no EventBridge publish on either transition, and nothing on the conclusion path publishes one either. The check creates both rules BEFORE the stack (so they are effective for the whole run), creates the stack, runs a detection, and then drains the control queue for the IN_PROGRESS + COMPLETE pair; nothing arrives, so it makes no further progress and is killed at the harness ceiling: `Check timed out after 60.0s`.
Why: drift detection is asynchronous and the event is the only push notification of its conclusion. A client relying on AWS's behavior wires an EventBridge rule to react to drift (alarm, auto-remediate, or gate a deploy) and never fires against the emulator; an operator who filters on `stack-drift-status: DRIFTED` sees permanent silence and reads it as "no stack has drifted". The IN_PROGRESS sentinel values (`stack-drift-status` `""` and count `-1`) are part of the contract: a consumer that treats them as a verdict/count must be able to distinguish the two events.
Fix: the drift-detection path in the cloudformation service — `src/ops/detect_stack_drift.rs` on accepting the call, and whatever settles `state::PendingDriftOutcome` into DETECTION_COMPLETE/DETECTION_FAILED — must publish a `CloudFormation Drift Detection Status Change` event to the default EventBridge bus with the detail fields above, so bus rules evaluate it. (DetectStackResourceDrift's and stack-level drift events belong to the same eventing seam.)
Traces: cloudformation__events__sqs.xsvc-cloudformation-2-drift-detection-status.

## Evidence
cloudformation__events__sqs.xsvc-cloudformation-2-drift-detection-status [mismatch]
  expected: after CreateStack of a one-AWS::SQS::Queue stack and DetectStackDrift on it, two events with source "aws.cloudformation" and detail-type "CloudFormation Drift Detection Status Change" reach the control rule's queue within 90s — one with detail.status-details.detection-status "DETECTION_IN_PROGRESS", detail.status-details.stack-drift-status "" and detail.drift-detection-details.drifted-stack-resource-count -1, and one with "DETECTION_COMPLETE", stack-drift-status "IN_SYNC" and count 0; both carrying detail.stack-id = the stack ARN and detail.stack-drift-detection-id = the id DetectStackDrift returned. The claim rule (pattern detail.status-details.stack-drift-status ["DRIFTED"]) must stay silent for that detection and then receive the DETECTION_COMPLETE event after sqs.SetQueueAttributes changes VisibilityTimeout out of band.
  actual: no event is delivered to either queue; the check spends its whole window draining the control queue and the harness reports `Check timed out after 60.0s`.
  code evidence: services/service-com-amazonaws-cloudformation-cloudformation/src/ops/detect_stack_drift.rs lines 152-258 implement the detection and record its in-progress and pending-complete states, with no EventBridge publish on either.

## Steps to Reproduce
Boot the emulator and replay cloudformation__events__sqs.xsvc-cloudformation-2-drift-detection-status from suite simparity-behavioral-aws-v1.

By hand:
1. sqs.CreateQueue and allow events.amazonaws.com to SendMessage to it; events.PutRule with EventPattern {"source":["aws.cloudformation"],"detail-type":["CloudFormation Drift Detection Status Change"],"detail":{"stack-id":[{"prefix":"arn:aws:cloudformation:<region>:<account>:stack/<stack name>/"}]}} and events.PutTargets pointing at the queue. Add a second rule/queue whose pattern additionally requires {"detail":{"status-details":{"stack-drift-status":["DRIFTED"]}}}.
2. cloudformation.CreateStack with {"Resources":{"Subject":{"Type":"AWS::SQS::Queue","Properties":{"VisibilityTimeout":30}}}} and wait for CREATE_COMPLETE.
3. cloudformation.DetectStackDrift(StackName=<stack id>) and poll DescribeStackDriftDetectionStatus to DETECTION_COMPLETE.
4. sqs.ReceiveMessage on the first queue — against AWS the DETECTION_IN_PROGRESS and DETECTION_COMPLETE events are there; against the emulator the queue stays empty.
5. sqs.SetQueueAttributes on the stack's deployed queue (VisibilityTimeout 45), DetectStackDrift again, and poll the DRIFTED-only queue — it also stays empty.
