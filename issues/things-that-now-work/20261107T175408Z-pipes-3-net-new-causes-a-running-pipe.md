Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: pipes.deploy-cloudformation-pipe-routes-message pipes.deploy-cfn-pipe-update-to-a-role-without-source-read-fails-and-rolls-back-to-the-working-pipe pipes.event-bus-target-publishes-record-with-detail-type-and-source pipes.update-pipe-filter-criteria-takes-effect-on-a-running-pipe-and-reverts
Issue-Shape-Id: com.amazonaws.pipes#Pipes
Issue-Key: pipes.deploy-cloudformation-pipe-routes-message|_wait_for_target_message#14|AssertionError|raise AssertionError(f"target never received a message with body {expected_body!r} within {timeout_s}s")
Issue-Key: pipes.deploy-cfn-pipe-update-to-a-role-without-source-read-fails-and-rolls-back-to-the-working-pipe|_await_record#13|AssertionError|raise AssertionError(f"no record with body {body!r} reached the target within {timeout_s}s")
Issue-Key: pipes.event-bus-target-publishes-record-with-detail-type-and-source|_receive_until#20|AssertionError|raise AssertionError(f"no message carrying {marker!r} reached the queue within {timeout_s:.0f}s; saw {seen!r}")
Issue-Key: pipes.update-pipe-filter-criteria-takes-effect-on-a-running-pipe-and-reverts|_receive#36|Timeout|resp = sqs.receive_message(**kwargs)
Issue-Commit: c98f49a3ab045fe250f88b9f8f7e4420d8205746

# pipes: 3 net-new causes — a RUNNING pipe never delivers to an SQS target, never publishes to an EventBridge event-bus target, and UpdatePipe FilterCriteria on a RUNNING pipe never converges

> _Note: this issue was identified against the code base at commit `c98f49a3ab045fe250f88b9f8f7e4420d8205746`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribePipe

## Description
Three distinct root causes in the pipes service's data plane. This issue carries 3 causes; all three must be closed before submitting.

Cause 1 of 3: a pipe in CurrentState RUNNING with an SQS source and an SQS-queue target never delivers the record to the target queue.
AWS: Smithy (services/service-com-amazonaws-pipes-pipes/smithy-model/com-amazonaws-pipes-pipes.json) models CreatePipe Source/Target/RoleArn as required, SourceParameters.SqsQueueParameters and TargetParameters.SqsQueueParameters (PipeTargetSqsQueueParameters), and DescribePipe.CurrentState over the PipeState enum. The runtime itself is not modeled, so recorded behavior only: once DescribePipe reports RUNNING, each message read from the source is sent to the target queue (its body rendered by TargetParameters.InputTemplate when one is set) and deleted from the source. This holds whether the pipe was created by the SDK or by a CloudFormation AWS::Pipes::Pipe resource.
Emulator: the pipe reaches RUNNING but the target queue stays empty for the full window — "target never received a message with body 'cfn-payload-<id>' within 120s" and "no record with body '{\"m\": \"before-<id>\"}' reached the target within 120.0s". The body values shown are per-run markers; the requirement is that the target receives the record the source carried.
Why: an SQS -> Pipe -> SQS fan-out, the most basic pipe shape, silently drops every record while reporting a healthy pipe. CloudFormation stacks that deploy such a pipe reach CREATE_COMPLETE and are then unusable, and any test or workload that relies on a positive control before exercising an update/rollback path can never get past that control.
Fix: the pipes crate's pipe runtime — the SQS-source poller and the SQS-queue target dispatcher, including InputTemplate rendering and deletion of the consumed source message. Open issue 20261023T235727Z-pipes-eventbridge-pipes-has-no-runtime-a-running-pipe.md describes the same missing runtime and should be reconciled with this one rather than duplicated.
Traces: pipes.deploy-cloudformation-pipe-routes-message, pipes.deploy-cfn-pipe-update-to-a-role-without-source-read-fails-and-rolls-back-to-the-working-pipe.

Cause 2 of 3: a pipe whose target is an EventBridge event bus never publishes any event to that bus.
AWS: Smithy models TargetParameters.EventBridgeEventBusParameters (PipeTargetEventBridgeEventBusParameters) with DetailType and Source, and PipeTargetParameters documents the event-bus target. Recorded behavior: for each record consumed from the source, the pipe calls PutEvents on the target bus with Source and DetailType taken from EventBridgeEventBusParameters and the record as Detail, so a rule on that bus matching {"source": [<configured source>]} fires and its target (here an SQS sink queue) receives the event.
Emulator: nothing is ever published; the rule never fires and the sink queue is empty for the whole window — "no message carrying '<marker>' reached the queue within 150s; saw []". The marker is a per-run value.
Why: a client building an SQS -> Pipe -> event bus -> rule chain gets a RUNNING pipe, a matching rule and a correctly policied sink, and zero events. Because nothing is published, the Source/DetailType/Detail shape of the emitted event is also unverifiable.
Fix: the pipes crate's pipe runtime target dispatch — the EventBridge event-bus target branch, which must PutEvents onto the configured bus with EventBridgeEventBusParameters.Source and .DetailType and the source record as the event detail.
Traces: pipes.event-bus-target-publishes-record-with-detail-type-and-source.

Cause 3 of 3: UpdatePipe that re-installs SourceParameters.FilterCriteria on a RUNNING pipe never converges, so the check runs to the harness cap.
AWS: Smithy models UpdatePipe (PUT /v1/pipes/{Name}, required Name and RoleArn) taking UpdatePipeSourceParameters, and DescribePipe echoing SourceParameters.FilterCriteria plus CurrentState over the PipeState enum (UPDATING -> RUNNING). Recorded behavior: after UpdatePipe installs a new filter pattern, DescribePipe returns to CurrentState=RUNNING with the new FilterCriteria.Filters[0].Pattern stored, and the live poller then applies the replaced pattern — records admitted by the new pattern are delivered and records it excludes are not, and reverting the pattern reverses which shape flows.
Emulator: the check makes no progress and is killed by the harness — "Check timed out after 180.0s". It never gets past its first positive control (an admitted record reaching the target under the original filter), so neither the stored-pattern convergence nor the applied-filter swap is ever observed.
Why: a client that reconfigures a live pipe's filter cannot tell whether the new filter is in effect; the pipe neither converges nor reports a failure, and the call hangs the caller's reconfiguration loop instead of settling.
Fix: the pipes crate's UpdatePipe handler and pipe state (store and echo the replaced FilterCriteria and drive CurrentState back to RUNNING) together with the runtime poller that must re-read the pipe's current FilterCriteria on each poll. Open issues 20261031T214016Z-pipes-updatepipe-that-re-installs-filtercriteria-on-a.md and 20261103T175414Z-pipes-updatepipe-re-installing-filtercriteria-on-a-running.md describe this area; this check is not carried by either and is filed here.
Traces: pipes.update-pipe-filter-criteria-takes-effect-on-a-running-pipe-and-reverts.

## Evidence
Cause 1 — expected: with DescribePipe CurrentState=RUNNING, a message sent to the source queue appears on the target queue within the poll window. Actual: the target queue stays empty. Replay output: "AssertionError: target never received a message with body 'cfn-payload-f0cfd90d0e1c' within 120s"; "AssertionError: no record with body '{\"m\": \"before-92db96106f54\"}' reached the target within 120.0s".

Cause 2 — expected: the pipe calls PutEvents on the target bus with Source=<EventBridgeEventBusParameters.Source>, DetailType=<EventBridgeEventBusParameters.DetailType> and the source record as Detail, so the bus rule matching that source forwards it to the sink queue. Actual: no event is ever published; the sink queue stays empty. Replay output: "AssertionError: no message carrying 'bus-dbe9a979a8484a06bd32d3c6551e4304' reached the queue within 150s; saw []".

Cause 3 — expected: UpdatePipe installs the new FilterCriteria, DescribePipe returns CurrentState=RUNNING with the new Filters[0].Pattern, and the poller delivers only the records the new pattern admits (and the revert reverses it). Actual: no convergence and no delivery; the check is killed by the harness — "Error: Check timed out after 180.0s".

## Steps to Reproduce
Cause 1: create a source and a target SQS queue and a role granting sqs:ReceiveMessage/DeleteMessage/GetQueueAttributes on the source and sqs:SendMessage on the target; CreatePipe (SDK or CloudFormation AWS::Pipes::Pipe) with Source=<source arn>, SourceParameters={SqsQueueParameters:{BatchSize:1}}, Target=<target arn>, DesiredState=RUNNING; wait for DescribePipe CurrentState=RUNNING; SendMessage to the source; ReceiveMessage on the target — nothing arrives.

Cause 2: create an SQS source queue, an EventBridge event bus, a rule on that bus with EventPattern {"source":["sim2real.pipes"]} and an SQS sink target (with a queue policy allowing events.amazonaws.com); create a role granting the source reads plus events:PutEvents on the bus; CreatePipe with Target=<bus arn> and TargetParameters={EventBridgeEventBusParameters:{DetailType:"OrderPlaced",Source:"sim2real.pipes"}}, DesiredState=RUNNING; SendMessage to the source; ReceiveMessage on the sink — nothing arrives.

Cause 3: create a pipe with SourceParameters={SqsQueueParameters:{BatchSize:1},FilterCriteria:{Filters:[{Pattern:'{"body":{"kind":["alpha","beta"]}}'}]}} from an SQS source to an SQS target; wait for RUNNING; send a record with kind=alpha and wait for it on the target; then UpdatePipe with the pattern replaced by '{"body":{"kind":["beta","gamma"]}}' and poll DescribePipe for CurrentState=RUNNING with that pattern stored; neither the control delivery nor the convergence happens and the run hangs.
