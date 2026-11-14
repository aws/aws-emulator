Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events__glue.event-trigger-fires-only-after-its-batch-size-is-reached
Issue-Key: events__glue.event-trigger-fires-only-after-its-batch-size-is-reached|_warm_up#29|AssertionError|assert time.monotonic() < deadline, (
Issue-Commit: 1b9f432602d5f7155b187927ba6fddf5156de930

# events__glue: PutTargets now accepts a Glue workflow target but the EventBridge->Glue-workflow ingress still never produces a workflow run, so an EVENT trigger's EventBatchingCondition is unobservable

> _Note: this issue was identified against the code base at commit `1b9f432602d5f7155b187927ba6fddf5156de930`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutEvents

## Description
One root cause. Cause count: 1.

=== Cause 1 of 1: an EventBridge rule whose target is a Glue workflow ARN never turns a matched delivery into a Glue workflow run ===
Failing check ids for this cause: events__glue.event-trigger-fires-only-after-its-batch-size-is-reached.

AWS: a Glue workflow whose only starting trigger has Type EVENT is driven from EventBridge. events:PutRule + events:PutTargets accept a target whose Arn is arn:aws:glue:<region>:<account>:workflow/<name> together with a RoleArn that grants glue:notifyEvent on that workflow, answering FailedEntryCount 0; every event the rule matches is then delivered to that workflow and counted against the trigger's EventBatchingCondition. Smithy (com.amazonaws.glue#EventBatchingCondition): BatchSize is @required with @range 1..100 — "Number of events that must be received from Amazon EventBridge before EventBridge event trigger fires"; BatchWindow is @range 1..900 — "Window of time in seconds after which EventBridge event trigger fires. Window starts when first event is received"; the structure doc: "Batch condition that must be met (specified number of events received or batch time window expired) before EventBridge event trigger fires." So a trigger with BatchSize=1 must have exactly one workflow run visible through glue:GetWorkflowRuns after its FIRST matched delivery, and a BatchSize=3/BatchWindow=900 twin on the same rule must stay at ZERO runs after deliveries 1 and 2 and reach EXACTLY ONE after delivery 3. Delivery LATENCY is recorded behavior only (the check calibrates its absence window from the unbatched twin's measured latency); no fixed timing is a requirement.

Emulator: the control-plane halves now work — glue CreateTrigger accepts the EVENT trigger and GetTrigger reports Type EVENT, State CREATED and the submitted EventBatchingCondition, events:PutRule is accepted, and events:PutTargets now answers FailedEntryCount 0 for the Glue workflow target (the check's 120 s PutTargets retry completes, which it did not in the earlier recording). The DELIVERY half is still inert: the check then re-emits a matching event to the warm-up rule every 10 s for its whole budget and glue:GetWorkflowRuns for that rule's workflow never reports a run — it fails with AssertionError "no warm-up delivery to '<workflow>' produced a workflow run within 600s". No error is surfaced anywhere: PutEvents answers FailedEntryCount 0, the rule exists, the target exists, the trigger is CREATED, and nothing ever happens. Hypothesis: EventBridge target dispatch has no handler for a glue workflow target ARN (the glue:notifyEvent ingress), so matched events are accepted and dropped and the Glue trigger's batch counter is never incremented.

Why: the documented — and only — way an event starts a Glue workflow is an EVENT trigger behind an EventBridge rule, and the only purpose of EventBatchingCondition is to coalesce N deliveries into one run. A client that builds exactly the documented ingress gets a success from every call it makes and then no workflow run at all, with no way to distinguish "the batch has not filled yet" from "deliveries are discarded". Every downstream observable (the run's WorkflowRunId, the job the trigger's Actions start, the Glue Workflow/Job State Change events) is unreachable, and an emulator that later starts one run per delivery while ignoring BatchSize would be indistinguishable from a correct one because the whole path is ungraded today.

Related open issue, which does NOT cover this failure: 20261026T234618Z-events-glue-with-glue-createtrigger-now-implemented-the.md carries this check's id but was keyed to an earlier, different failure (the check was killed by the harness at 60 s while still inside the PutTargets-acceptance retry, with the first half of the cause — whether PutTargets even accepts a Glue workflow target — left unresolved). That arm no longer reproduces: PutTargets is now accepted and the check runs on to its 600 s delivery poll, so the surviving defect is purely the delivery/batch-accounting path. A fixer who closes that issue on the PutTargets arm alone will re-replay this check and still find it failing.

Fix: the EventBridge target-dispatch component for glue workflow target ARNs (invoke the glue:notifyEvent ingress with the target's RoleArn), plus the Glue trigger/workflow state that accounts for deliveries — count each delivery against the trigger's EventBatchingCondition and start exactly one workflow run when BatchSize is reached or BatchWindow expires, recording it so GetWorkflowRuns reports it with the workflow's Name and a non-empty WorkflowRunId.

Traces: events__glue.event-trigger-fires-only-after-its-batch-size-is-reached.

## Evidence
Check events__glue.event-trigger-fires-only-after-its-batch-size-is-reached [mismatch]

Actual (emulator), from the replay output:
  File "check_events__glue_event_trigger_fires_only_after_its_batch_size_is_reached.py", line 293, in run
    _warm_up(ctx, ev, glue, f"{stem}-warm-rule", warm, warm_arn, events_role, stem)
  File "...", line 205, in _warm_up
    assert time.monotonic() < deadline
  AssertionError: no warm-up delivery to '<workflow-name>' produced a workflow run within 600s
(the workflow name in that message is a per-run generated value, not a requirement; the fixed text is "no warm-up delivery to ... produced a workflow run within <budget>s", which is the harness's own wording, not an AWS message.)

What the emulator demonstrably DOES answer on the way there (so the stall is narrowed to delivery):
- glue:CreateTrigger(Type='EVENT', WorkflowName=W, Actions=[{JobName:J}], EventBatchingCondition={BatchSize:N[,BatchWindow:900]}) -> accepted; glue:GetTrigger reports Type 'EVENT', State 'CREATED' and the submitted EventBatchingCondition.
- events:PutRule(Name=r, State='ENABLED', EventPattern={"source":[S],"detail-type":[DT]}) -> accepted.
- events:PutTargets(Rule=r, Targets=[{Id, Arn:'arn:aws:glue:<region>:<account>:workflow/<W>', RoleArn:<role granting glue:notifyEvent>}]) -> FailedEntryCount 0 within the check's 120 s retry.
- events:PutEvents(Entries=[{Source:S, DetailType:DT, Detail:...}]) -> FailedEntryCount 0, re-emitted every 10 s.

Expected (AWS, per com.amazonaws.glue#EventBatchingCondition and the Glue "Starting a workflow with an EventBridge event" docs):
- after the FIRST matched delivery to a workflow whose EVENT trigger has BatchSize=1: glue:GetWorkflowRuns(Name=W) reports exactly one run, carrying Name W and a non-empty WorkflowRunId.
- for the BatchSize=3 / BatchWindow=900 twin on the same rule, role and deliveries: GetWorkflowRuns(Name=W1) reports ZERO runs after deliveries 1 and 2 (400 EntityNotFoundException for a never-run workflow is the same zero) and EXACTLY one run after delivery 3, well inside the 900 s window.
Emulator: zero runs forever, for both arms. Generated workflow/trigger/job/role names, the account id and the 600 s harness budget are per-run values, not requirements.

## Steps to Reproduce
1. iam.create_role trusting glue.amazonaws.com; glue.create_job(Name=J, Command={'Name':'pythonshell',...}, MaxCapacity=0.0625).
2. glue.create_workflow(Name=W1); glue.create_workflow(Name=W2).
3. glue.create_trigger(Name=W1+'-evt', Type='EVENT', WorkflowName=W1, Actions=[{'JobName': J}], EventBatchingCondition={'BatchSize': 3, 'BatchWindow': 900}) and glue.create_trigger(Name=W2+'-evt', Type='EVENT', WorkflowName=W2, Actions=[{'JobName': J}], EventBatchingCondition={'BatchSize': 1}); glue.get_trigger on each -> Type 'EVENT', State 'CREATED', EventBatchingCondition as submitted (this already works).
4. iam.create_role trusting events.amazonaws.com with an inline grant of glue:notifyEvent on both workflow ARNs.
5. events.put_rule(Name=r, State='ENABLED', EventPattern='{"source":["<S>"],"detail-type":["<DT>"]}'); events.put_targets(Rule=r, Targets=[{'Id':'batched','Arn':'arn:aws:glue:<region>:<acct>:workflow/'+W1,'RoleArn':<events role>}, {'Id':'twin','Arn':'arn:aws:glue:<region>:<acct>:workflow/'+W2,'RoleArn':<events role>}]) -> FailedEntryCount 0 (already works).
6. events.put_events(Entries=[{'Source':'<S>','DetailType':'<DT>','Detail':'{}'}]) -> FailedEntryCount 0. Poll glue.get_workflow_runs(Name=W2) (the BatchSize=1 twin).
   -> AWS: one run appears. Emulator: no run ever appears; polling for 600 s yields nothing. THIS IS THE DEFECT.
7. (Must pass after the fix) assert glue.get_workflow_runs(Name=W1) reports ZERO runs across deliveries 1 and 2 for a window well under BatchWindow=900 s, then put a third matching event and assert it reports EXACTLY one run whose Name is W1 and which carries a non-empty WorkflowRunId.
8. Re-replay events__glue.event-trigger-fires-only-after-its-batch-size-is-reached to confirm.
