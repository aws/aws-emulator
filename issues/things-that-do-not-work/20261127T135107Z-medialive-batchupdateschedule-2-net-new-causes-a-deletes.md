Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: medialive.delete-schedule-empties-the-schedule-and-absent-deletes-are-no-ops medialive.schedule-batch-reusing-a-scheduled-action-name-is-rejected-whole
Issue-Shape-Id: com.amazonaws.medialive#MediaLive
Issue-Key: medialive|BatchUpdateSchedule|UnprocessableEntityException
Issue-Key: medialive.schedule-batch-reusing-a-scheduled-action-name-is-rejected-whole|run#38|AssertionError|assert message == want, f"reusing a scheduled name was refused with message {message!r}, expected {want!r}"
Issue-Commit: 692629b6d1ae5ae78e012290c3136f29b80245ac

# medialive BatchUpdateSchedule: 2 net-new causes — a Deletes naming an absent action is refused instead of being a no-op, and duplicate-name refusals carry a generic message instead of AWS's per-action text

> _Note: this issue was identified against the code base at commit `692629b6d1ae5ae78e012290c3136f29b80245ac`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
BatchUpdateSchedule

## Description
Two distinct defects in the MediaLive BatchUpdateSchedule path, both surfacing the same placeholder refusal message.

Cause 1 of 2: a batch whose Deletes names a schedule action that is not in the schedule is refused outright, instead of being accepted as a no-op that still lands the batch's Creates.
AWS: Smithy: BatchUpdateSchedule PUT /prod/channels/{ChannelId}/schedule, input Deletes -> BatchScheduleActionDeleteRequest.ActionNames (@required list of strings), output Deletes -> BatchScheduleActionDeleteResult.ScheduleActions ("List of actions that have been deleted from the schedule"); modeled errors include UnprocessableEntityException (422) but the model states no constraint requiring a deleted name to exist. Recorded behavior only: AWS returns HTTP 200 for a batch of Creates=[one fresh action] + Deletes={ActionNames:[a name not in the schedule]}, with deletes.scheduleActions empty (nothing was removed) and creates.scheduleActions listing the created action; DescribeSchedule afterwards shows the pre-existing actions plus the newly created one. Deleting an absent action name is idempotent/no-op, and it does not poison the sibling creates in the same batch.
Emulator: the call is refused with HTTP 422 UnprocessableEntityException and message "1 schedule action problem(s) in this request" (botocore: "An error occurred (UnprocessableEntityException) when calling the BatchUpdateSchedule operation: 1 schedule action problem(s) in this request"). The batch's Creates therefore never land either. Hypothesis: the emulator validates every name in Deletes against the current schedule and counts a missing one as a "schedule action problem"; the count of 1 matches the single absent delete name in the failing batch (the two preceding create-only batches were accepted).
Why: a client that prunes a schedule defensively — issuing deletes for names it is not sure are still scheduled, which AWS treats as no-ops — gets a 422 and loses the creates bundled in the same batch. Reconciliation loops and "delete-then-recreate" schedule updates fail against the emulator while they succeed on AWS, and the schedule is left in the pre-batch state with no indication which element was at fault.
Fix: the BatchUpdateSchedule handler's validation of the Deletes list in the medialive service crate: an ActionName absent from the channel's schedule must not be an error; it is dropped from the result's deletes.scheduleActions while the rest of the batch (creates and present deletes) is applied, and the operation answers 200.
Traces: medialive.delete-schedule-empties-the-schedule-and-absent-deletes-are-no-ops.

Cause 2 of 2: when Creates reuses the name of an action already in the schedule, the batch is correctly refused with UnprocessableEntityException, but the error message is a generic placeholder instead of AWS's per-action text.
AWS: Smithy: UnprocessableEntityException (httpError 422) carries Message (jsonName "message") and ValidationErrors (list of ValidationError{ElementPath, ErrorMessage}). Recorded behavior only for the message text: AWS refuses a batch that creates a name already present with code UnprocessableEntityException and Message of the form "actionName: <name> error: Cannot create actionName=<name> in schedule because it already exists". Fixed text: "actionName: ", " error: Cannot create actionName=", " in schedule because it already exists". Variable parts: <name>, taken twice from the offending ScheduleActions[].ActionName in the request (the recording's "keep-1000000" is one run's channel-derived value filling that template, not part of it). The refusal is whole-batch: no action from the refused batch is applied.
Emulator: same code (UnprocessableEntityException, 422) but Message is "1 schedule action problem(s) in this request" — a count, with no action name and no statement of the rule violated.
Why: the error code alone does not tell a client which of the actions in a multi-action batch was rejected or why. Clients and operators that parse or log the message to identify and rename the colliding action (the only place AWS reports the action name for this failure) cannot recover the name, and tests asserting AWS's documented refusal text fail.
Fix: the BatchUpdateSchedule handler's validation/error construction in the medialive service crate — build the UnprocessableEntityException message per offending action from the request's ActionName using AWS's wording above (and populate ValidationErrors rather than emitting a bare count).
Traces: medialive.schedule-batch-reusing-a-scheduled-action-name-is-rejected-whole.

## Evidence
Cause 1 — check medialive.delete-schedule-empties-the-schedule-and-absent-deletes-are-no-ops:
  Request (after two actions "early"/"late" were seeded successfully):
    BatchUpdateSchedule(ChannelId=<id>,
      Creates={ScheduleActions:[{ActionName:"extra-<id>", ScheduleActionSettings:{StaticImageDeactivateSettings:{}}, ScheduleActionStartSettings:{FixedModeScheduleActionStartSettings:{Time:<t3>}}}]},
      Deletes={ActionNames:["ghost-<id>"]})   # "ghost-<id>" is not in the schedule
  Expected (AWS): HTTP 200; deletes.scheduleActions == [] ; creates.scheduleActions == ["extra-<id>"]; DescribeSchedule == {early, late, extra}.
  Actual (emulator): HTTP 422 UnprocessableEntityException, Message "1 schedule action problem(s) in this request"
    botocore.errorfactory.UnprocessableEntityException: An error occurred (UnprocessableEntityException) when calling the BatchUpdateSchedule operation: 1 schedule action problem(s) in this request
  The check aborts at this call, so the created action and the subsequent DeleteSchedule assertions never run.

Cause 2 — check medialive.schedule-batch-reusing-a-scheduled-action-name-is-rejected-whole:
  Request (after "keep-<id>" was created successfully):
    BatchUpdateSchedule(ChannelId=<id>, Creates={ScheduleActions:[{ActionName:"add-<id>",...},{ActionName:"keep-<id>",...}]})
  Expected (AWS): code UnprocessableEntityException, Message "actionName: keep-<id> error: Cannot create actionName=keep-<id> in schedule because it already exists"
  Actual (emulator): code UnprocessableEntityException (correct), Message "1 schedule action problem(s) in this request"
    AssertionError: reusing a scheduled name was refused with message '1 schedule action problem(s) in this request', expected 'actionName: keep-1000000 error: Cannot create actionName=keep-1000000 in schedule because it already exists'
  (keep-1000000 is that run's channel-id-derived name; the requirement is the template above with the request's ActionName.)

## Steps to Reproduce
1. Create an S3 bucket, an IAM role trusted by medialive.amazonaws.com, a MediaLive MP4_FILE input and a SINGLE_PIPELINE channel referencing them; wait until DescribeChannel reports State=IDLE.
2. Cause 2: BatchUpdateSchedule(ChannelId, Creates={ScheduleActions:[{ActionName:"keep-X", ScheduleActionSettings:{StaticImageDeactivateSettings:{}}, ScheduleActionStartSettings:{FixedModeScheduleActionStartSettings:{Time:<future t0>}}}]}) -> 200. Then BatchUpdateSchedule with Creates=[{ActionName:"add-X",...t1},{ActionName:"keep-X",...t2}] and read the error Message: the emulator answers "1 schedule action problem(s) in this request" instead of "actionName: keep-X error: Cannot create actionName=keep-X in schedule because it already exists".
3. Cause 1: with "keep-X"/"early"/"late" in the schedule, call BatchUpdateSchedule(ChannelId, Creates={ScheduleActions:[{ActionName:"extra-X",...t3}]}, Deletes={ActionNames:["ghost-X"]}) where "ghost-X" was never scheduled. The emulator refuses with 422 UnprocessableEntityException "1 schedule action problem(s) in this request"; AWS answers 200 with deletes.scheduleActions=[] and creates.scheduleActions=["extra-X"], and DescribeSchedule then lists the new action alongside the existing ones.
4. Replay: medialive.delete-schedule-empties-the-schedule-and-absent-deletes-are-no-ops and medialive.schedule-batch-reusing-a-scheduled-action-name-is-rejected-whole.
