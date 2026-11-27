Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iam__mq__scheduler.mq-read-only-universal-targets-rejected-while-create-tags-delivers iam__mq__scheduler.update-schedule-to-read-only-mq-target-rejected-keeps-original-target
Issue-Key: iam__mq__scheduler.mq-read-only-universal-targets-rejected-while-create-tags-delivers|_refusal#13|AssertionError|raise AssertionError(f"expected a refusal, the call succeeded: {result!r}")
Issue-Key: iam__mq__scheduler.update-schedule-to-read-only-mq-target-rejected-keeps-original-target|_refused#14|AssertionError|raise AssertionError(f"the call was expected to be refused but returned {outcome!r}")
Issue-Commit: e47284ab37991d11b5f4fdcc5d2b5ad1491b8a6a

# scheduler accepts a universal target naming a read-only Amazon MQ API on CreateSchedule and UpdateSchedule instead of refusing it with ValidationException

> _Note: this issue was identified against the code base at commit `e47284ab37991d11b5f4fdcc5d2b5ad1491b8a6a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateSchedule

## Description
Cause 1 of 1: EventBridge Scheduler's universal-target validation of the invoked AWS SDK action is missing, so a schedule whose Target.Arn is `arn:aws:scheduler:::aws-sdk:<service>:<action>` for a READ-ONLY action is created (and an existing schedule is re-pointed at one) instead of being refused.

AWS: Smithy (services/service-com-amazonaws-scheduler-awschronosservice) models CreateSchedule as POST /schedules/{Name} and UpdateSchedule as PUT /schedules/{Name}, both with required Target and the modeled error ValidationException (client, httpError 400). The model does not enumerate which universal-target actions are invocable; AWS documents that the universal target can only invoke mutating API actions and that read-only actions (describe*/list*/get*) are not supported (https://docs.aws.amazon.com/scheduler/latest/UserGuide/managing-targets-universal.html). Recorded behavior: a CreateSchedule/UpdateSchedule whose Target.Arn names a read-only action is refused with HTTP 400 ValidationException and the message `Invalid RequestJson provided. Reason <action> API is not supported.` — the fixed text is `Invalid RequestJson provided. Reason ` + `<the action segment taken from the request's Target.Arn, in the service's own camelCase spelling>` + ` API is not supported.`. The action names in the failing checks (`describeConfiguration`, `listTags`, `listConfigurations`) are values from the request, not part of the template; the rule is that any read-only action of the targeted service is refused this way, while a mutating one (e.g. `mq:createTags`) is accepted. Because the request is refused, no schedule resource is created: a subsequent GetSchedule of that name must answer ResourceNotFoundException, and for UpdateSchedule the stored schedule must be left completely untouched (Target.Arn, Target.Input and ScheduleExpression unchanged, and it must still fire its original target when due).

Emulator: answers HTTP 200 with a ScheduleArn for both entry points. For CreateSchedule with Target.Arn `arn:aws:scheduler:::aws-sdk:mq:describeConfiguration` (and again with `...:mq:listTags`) it returns 200 `{"ScheduleArn": "arn:aws:scheduler:<region>:<account>:schedule/default/<name>"}` and the schedule becomes readable; no ValidationException is produced and no message/status divergence is even attempted. For UpdateSchedule re-pointing a working `mq:createTags` schedule at `mq:listConfigurations` it likewise returns 200 with the schedule's ARN, so the stored target is replaced with one AWS would never store.

Why: a client that relies on the service to reject an unsupported universal target gets a schedule the emulator happily persists but AWS refuses outright — so code calibrated against the emulator deploys schedules that fail to create on AWS, and the negative path (catch ValidationException, surface `... API is not supported.` to the user, know the schedule was not created) is untestable. Worse for UpdateSchedule: the emulator destroys the caller's working target on a call AWS treats as a no-op refusal, so the configuration a client believes is still in place (original Arn/Input/expression, firing when due) has silently been overwritten.

Fix: the universal-target validation shared by the CreateSchedule and UpdateSchedule handlers — classify the `aws-sdk:<service>:<action>` action named in Target.Arn and reject the read-only ones with ValidationException (400) carrying the message template above, before any schedule state is written, so a refused CreateSchedule leaves no schedule for GetSchedule to find and a refused UpdateSchedule leaves the stored schedule byte-for-byte unchanged.

Traces: the two failing behavioral checks listed under `### iam__mq__scheduler` (see trace_ids). The companion scheduler issues already open (20261121T130922Z-scheduler-2-net-new-behavioral-causes-the-dead.md, 20261111T160016Z-scheduler-an-sns-target-occurrence-whose-target.md) cover dead-letter delivery and SNS-target permission handling, not universal-target action support; neither covers this.

## Evidence
Check iam__mq__scheduler.mq-read-only-universal-targets-rejected-while-create-tags-delivers:
  CreateSchedule with Target.Arn 'arn:aws:scheduler:::aws-sdk:mq:describeConfiguration', Input {"ConfigurationId": "<cfg id>"}
  expected: ClientError ('ValidationException', 'Invalid RequestJson provided. Reason describeConfiguration API is not supported.')
  actual:   HTTP 200, {'ScheduleArn': 'arn:aws:scheduler:us-west-2:000000000000:schedule/default/sim2real-xsvc-sch-ro-<suffix>'}
  harness: AssertionError: expected a refusal, the call succeeded: {'ResponseMetadata': {'HTTPStatusCode': 200, ...}, 'ScheduleArn': ...}
  (the same check then expects GetSchedule of that name to answer ResourceNotFoundException; the second refused action in the same check is 'arn:aws:scheduler:::aws-sdk:mq:listTags' with the message 'Invalid RequestJson provided. Reason listTags API is not supported.')

Check iam__mq__scheduler.update-schedule-to-read-only-mq-target-rejected-keeps-original-target:
  UpdateSchedule on an existing mq:createTags schedule, re-pointed at Target.Arn 'arn:aws:scheduler:::aws-sdk:mq:listConfigurations'
  expected: ClientError ('ValidationException', 'Invalid RequestJson provided. Reason listConfigurations API is not supported.'), the stored schedule unchanged (Target.Arn still '...aws-sdk:mq:createTags', Target.Input still the original ResourceArn/Tags body, ScheduleExpression unchanged) and still firing mq:createTags when due
  actual:   HTTP 200, {'ScheduleArn': 'arn:aws:scheduler:us-west-2:000000000000:schedule/default/sim2real-xsvc-upd-sch-<suffix>'}
  harness: AssertionError: the call was expected to be refused but returned {'ResponseMetadata': {'HTTPStatusCode': 200, ...}, 'ScheduleArn': ...}

Fixed vs variable text in the refusal message: fixed = 'Invalid RequestJson provided. Reason ' and ' API is not supported.'; variable = the action segment of the request's Target.Arn. The region, account id, schedule name suffixes and configuration ids above are per-run values, not requirements.

## Steps to Reproduce
1. Clone the repo and boot the emulator built from that checkout; replay the behavioral suite (simparity-behavioral-aws-v1).
2. CreateSchedule path: mq.CreateConfiguration(EngineType=ACTIVEMQ, Name=<any>) to get a configuration id/ARN; iam.CreateRole with a scheduler.amazonaws.com trust policy and an inline policy allowing mq:CreateTags on that ARN.
3. scheduler.CreateSchedule(Name=<new>, ScheduleExpression='at(<~45s ahead>)', FlexibleTimeWindow={Mode: OFF}, Target={Arn: 'arn:aws:scheduler:::aws-sdk:mq:describeConfiguration', RoleArn: <role>, Input: '{"ConfigurationId": "<cfg id>"}'}). Expect 400 ValidationException 'Invalid RequestJson provided. Reason describeConfiguration API is not supported.'; the emulator returns 200 with a ScheduleArn. Repeat with action 'listTags' (Input {"ResourceArn": <cfg arn>}).
4. scheduler.GetSchedule(Name=<that name>) — expect ResourceNotFoundException because the create was refused; against the emulator the schedule exists.
5. UpdateSchedule path: create a schedule whose Target.Arn is 'arn:aws:scheduler:::aws-sdk:mq:createTags' with Input {"ResourceArn": <cfg arn>, "Tags": {...}}, read it back with GetSchedule, then call scheduler.UpdateSchedule(Name=<same>, Target={Arn: 'arn:aws:scheduler:::aws-sdk:mq:listConfigurations', RoleArn: <role>, Input: '{"MaxResults": 5}'}, same expression/window). Expect 400 ValidationException 'Invalid RequestJson provided. Reason listConfigurations API is not supported.'; the emulator returns 200.
6. GetSchedule again and compare Target.Arn, Target.Input and ScheduleExpression with the pre-update values, and wait for the occurrence: on AWS the original mq:createTags target survives and tags the configuration; against the emulator the target has been replaced.
