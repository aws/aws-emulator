Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: scheduler.schedule-flexible-time-window-invocation-within-bounds scheduler.server-side-flexible-time-window-jitter-within-bounds scheduler.server-side-rate-expression-cadence-enforced scheduler.events-scheduler-semantics-same-schedule-name-in-two-groups-is-isolated-and-survives-the-sibling-group-deletion
Issue-Shape-Id: com.amazonaws.scheduler#AWSChronosService
Issue-Key: scheduler.schedule-flexible-time-window-invocation-within-bounds|run#17|Timeout|r = sqs.receive_message(
Issue-Key: scheduler.server-side-flexible-time-window-jitter-within-bounds|_wait_for_message_sent_timestamp#14|Timeout|resp = sqs.receive_message(
Issue-Key: scheduler.server-side-rate-expression-cadence-enforced|_collect_sent_timestamps#7|Timeout|resp = sqs.receive_message(
Issue-Key: scheduler.events-scheduler-semantics-same-schedule-name-in-two-groups-is-isolated-and-survives-the-sibling-group-deletion|_drain_into#20|Timeout|resp = sqs.receive_message(QueueUrl=queue_url, MaxNumberOfMessages=10, WaitTimeSeconds=5)
Issue-Commit: a1aff8283ff7a4c3e09b44355afd91a57915fcd2

# scheduler: 3 unowned behavioral causes — FLEXIBLE time-window bound, server-paced rate() cadence, and group-scoped firing/cascade are never observable because no occurrence is ever delivered

> _Note: this issue was identified against the code base at commit `a1aff8283ff7a4c3e09b44355afd91a57915fcd2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateSchedule

## Description
This issue carries 3 root causes, all in the `scheduler` service. Every one of them is observed as the schedule's target never receiving anything: CreateSchedule returns 200, GetSchedule reports State=ENABLED with the submitted Target and FlexibleTimeWindow, and then the delivery poll loop in each check runs to the harness limit (`Check timed out after 60.0s`).

RELATED, NOT COVERING: the open issue 20261023T191008Z-scheduler-eventbridge-scheduler-never-fires-a-schedule-no-invocation.md describes the same missing occurrence/dispatch component, but its Issue-Trace-Ids / Issue-Key set does NOT carry any of the four check ids below, and the three requirements enumerated here (the FLEXIBLE window BOUND, server-PACED cadence, and (group,name)-keyed firing and cascade) are not stated as acceptance conditions there. Whoever fixes the dispatch engine must additionally satisfy the three causes below and re-replay exactly the ids in trace_ids; otherwise these four checks stay red after that issue is closed. Also related: 20261025T034039Z-scheduler-a-cloudformation-deployed-aws-scheduler-schedule-never-fires.md (CFN-deployed schedules) — neither issue carries these keys.

=== Cause 1 of 3: FlexibleTimeWindow Mode=FLEXIBLE never produces an invocation, so the window bound [nominal, nominal + MaximumWindowInMinutes] is never honoured ===
Failing checks: scheduler.schedule-flexible-time-window-invocation-within-bounds, scheduler.server-side-flexible-time-window-jitter-within-bounds
AWS: Smithy (com.amazonaws.scheduler) — `FlexibleTimeWindow` "Allows you to configure a time window during which EventBridge Scheduler invokes the schedule"; member `Mode` -> `FlexibleTimeWindowMode` (required) "Determines whether the schedule is invoked within a flexible time window"; member `MaximumWindowInMinutes` -> integer `@range min 1 max 1440` "The maximum time window during which a schedule can be invoked". `CreateSchedule.ScheduleExpression` is required, `@length 1..256`, and accepts `at(yyyy-mm-ddThh:mm:ss)`. Therefore a one-time `at(T)` schedule with Mode=FLEXIBLE and MaximumWindowInMinutes=N must be invoked exactly once at a server-chosen instant inside [T, T+N minutes] — never before T and never after the window closes. The checks observe that instant out-of-band, from the SQS `SentTimestamp` message attribute of the delivered message, with a few seconds of boundary slack.
Emulator: no invocation is ever produced for a FLEXIBLE schedule. GetSchedule faithfully echoes `FlexibleTimeWindow.Mode=FLEXIBLE` and `MaximumWindowInMinutes`, but the target queue stays empty for the whole poll window and both checks end in `Check timed out after 60.0s` inside the ReceiveMessage loop. The window bound is consequently untestable and unenforced in either direction.
Why: a client that uses FLEXIBLE mode to spread load gets a schedule that reads back as configured and never runs. Worse, when a dispatcher is added without the bound, a fix can regress to firing before the nominal instant or long after the window closes and still look healthy — the two directions of this bound are exactly what these checks grade.
Fix: the occurrence/dispatch component of the scheduler crate (services/service-com-amazonaws-scheduler-awschronosservice/src/, armed/disarmed by src/ops/create_schedule.rs, update_schedule.rs, delete_schedule.rs). It must compute the nominal instant from ScheduleExpression + ScheduleExpressionTimezone and, when `FlexibleTimeWindow.Mode == FLEXIBLE`, pick the actual invocation instant inside [nominal, nominal + MaximumWindowInMinutes] — clamped at BOTH ends — rather than ignoring the window.
Traces: scheduler.schedule-flexible-time-window-invocation-within-bounds (at(now+90s), Mode=FLEXIBLE, MaximumWindowInMinutes=1, asserts SentTimestamp in [nominal-5s, close+30s]); scheduler.server-side-flexible-time-window-jitter-within-bounds (at(now+1min), MaximumWindowInMinutes=2, 15s slack at each boundary).

=== Cause 2 of 3: rate() cadence is not paced by the server — a recurring schedule never yields a second occurrence, so inter-invocation spacing is never enforced ===
Failing check: scheduler.server-side-rate-expression-cadence-enforced
AWS: Smithy (com.amazonaws.scheduler) — `CreateSchedule.ScheduleExpression` (required, `@length 1..256`) accepts a rate expression `rate(value unit)`. A `rate(2 minutes)` schedule in State=ENABLED with FlexibleTimeWindow Mode=OFF fires repeatedly, and consecutive occurrences are spaced by the SERVER at the configured cadence — approximately 120s apart — independently of how often or how rarely the client polls the target. The check asserts the gap between the SQS `SentTimestamp` of two consecutive deliveries is in [80s, 220s]; a near-zero gap would mean the client's polling, not the server's clock, is pacing invocations.
Emulator: zero occurrences are delivered for a rate(2 minutes) schedule. `_collect_sent_timestamps` is given a 330s budget and collects nothing; the check dies at the harness limit with `Check timed out after 60.0s`. Neither the cadence nor the server-paced property is implemented.
Why: recurring schedules are the primary use of this service. A client that drives periodic work off rate()/cron() sees a healthy control plane and no work at all. Additionally, an implementation that emits occurrences on client polling (or all at once) would satisfy a naive "did anything arrive" test while being wrong; the spacing assertion is the requirement.
Fix: the same occurrence/dispatch component in services/service-com-amazonaws-scheduler-awschronosservice/src/ — specifically rate/cron occurrence computation driven by the server clock, emitting each occurrence at its own due instant, not on demand and not coalesced.
Traces: scheduler.server-side-rate-expression-cadence-enforced (rate(2 minutes), FlexibleTimeWindow OFF, SQS target, 330s collection budget for 2 messages, asserts 80s <= gap <= 220s).

=== Cause 3 of 3: schedule-group scoping of FIRING and of the delete cascade is unverifiable — identically-named schedules in two groups never deliver, so a group deletion cannot be shown to spare its sibling ===
Failing check: scheduler.events-scheduler-semantics-same-schedule-name-in-two-groups-is-isolated-and-survives-the-sibling-group-deletion
AWS: a schedule's identity is the (group, name) pair, which the model makes explicit: `GetScheduleOutput.Arn` carries `@pattern ^arn:aws(-[a-z]+)?:scheduler:[a-z0-9\-]+:\d{12}:schedule\/[0-9a-zA-Z-_.]+\/[0-9a-zA-Z-_.]+$` — group segment then name segment — and `GetSchedule`/`ListSchedules` both take an optional `GroupName` (httpQuery `groupName` / `ScheduleGroup`). So the same schedule name may exist in group A and group B at once; each is listed only under its own group, each fires with its OWN Target.Input, and `DeleteScheduleGroup` (modelled errors ConflictException 409, ResourceNotFoundException 404, ValidationException 400) cascades only to the schedules of THAT group — group B's identically-named schedule must survive and keep firing after group A is gone. GetSchedule against the default group for a name that exists only in A and B must answer `ResourceNotFoundException` (HTTP 404).
Emulator: the store does key schedules by `(group name, schedule name)` (services/service-com-amazonaws-scheduler-awschronosservice/src/state.rs: `pub schedules: BTreeMap<(String, String), ScheduleRecord>`), so the control-plane half is plausible — but the check cannot reach its subject. Its first positive control is "both same-named schedules deliver their own marker payload before the deletion" (180s budget), and since nothing ever fires, the check never gets past it and ends in `Check timed out after 60.0s`. The post-deletion claim — that an occurrence of B's schedule SCHEDULED AFTER group A's removal is still delivered — is therefore entirely unexercised, which is the exact failure mode (a cascade keyed by name alone taking the sibling with it) this check exists to catch.
Why: a client that uses schedule groups as namespaces, with the same logical schedule name per environment or tenant, gets no firing at all today, and after a dispatcher lands has no guarantee that tearing down one group leaves another group's identically-named schedule running. Deleting a group would silently stop unrelated tenants' schedules while the control plane still reports them ENABLED.
Fix: the occurrence/dispatch component must key its armed timers by (group, name) — the same key `src/state.rs` already uses — and `src/ops/delete_schedule_group.rs` (together with `state.rs`'s `settle`, which materialises the DELETING window) must disarm only the timers belonging to the deleted group. Input rendering must stay per-schedule so each same-named schedule delivers its own Target.Input.
Traces: scheduler.events-scheduler-semantics-same-schedule-name-in-two-groups-is-isolated-and-survives-the-sibling-group-deletion (one name created in groups A and B over one SQS queue with distinct marker payloads, rate(1 minute), FlexibleTimeWindow OFF; asserts distinct ARNs, per-group ListSchedules, ResourceNotFoundException from the default group, both markers delivered, then DeleteScheduleGroup(A) and a B occurrence timestamped after A's removal).

## Evidence
Suite: simparity-behavioral-ga-prod-v1, scheduler 45/108 passed (42%), 63 failed. The four checks below are the only scheduler failures the report marks neither "ALREADY FILED" nor "OWNED by" — the other 59 are owned by existing issues.

Cause 1 — FLEXIBLE window bound
  [mismatch] scheduler.schedule-flexible-time-window-invocation-within-bounds — Error: Check timed out after 60.0s
  [mismatch] scheduler.server-side-flexible-time-window-jitter-within-bounds — Error: Check timed out after 60.0s
  Expected: one invocation whose SQS SentTimestamp t satisfies nominal <= t <= nominal + MaximumWindowInMinutes (boundary slack 5s/15s before, 30s/15s after). GetSchedule already echoes Mode=FLEXIBLE and MaximumWindowInMinutes correctly.
  Actual: the target queue never receives any message; the ReceiveMessage poll loop (WaitTimeSeconds 15-20) runs until the harness kills the check at 60s. The check's own assertion text on exhaustion is "target never received the flexible-window invocation" / "no invocation observed within <timeout>s".

Cause 2 — server-paced rate() cadence
  [mismatch] scheduler.server-side-rate-expression-cadence-enforced — Error: Check timed out after 60.0s
  Expected: >= 2 deliveries from a rate(2 minutes) / FlexibleTimeWindow OFF schedule within 330s, with (sent[1]-sent[0])/1000 in [80.0, 220.0] seconds.
  Actual: 0 deliveries. `_collect_sent_timestamps(count=2, timeout_s=330)` accumulates nothing; harness timeout at 60s. The check's exhaustion message is "expected 2 SQS deliveries from the rate(2 minutes) schedule <name> within 330s, got 0: []".

Cause 3 — group-scoped firing and cascade
  [mismatch] scheduler.events-scheduler-semantics-same-schedule-name-in-two-groups-is-isolated-and-survives-the-sibling-group-deletion — Error: Check timed out after 60.0s
  Expected: both same-named schedules (group A marker, group B marker, rate(1 minute)) deliver their own payload; then DeleteScheduleGroup(A) -> GetScheduleGroup(A) answers ResourceNotFoundException (404); GetSchedule(Name, GroupName=B) still reads Arn=<B's arn> State=ENABLED; and B delivers at least one occurrence timestamped after A's removal.
  Actual: the check never leaves its first positive-control loop ("both same-named schedules must deliver their own payload before the deletion: A=0 B=0 within 180s") because nothing fires; harness timeout at 60s. The post-cascade survival claim is never exercised.

Source corroboration: services/service-com-amazonaws-scheduler-awschronosservice/src/state.rs is documented as "Engine state" holding only `groups`, `schedules: BTreeMap<(String, String), ScheduleRecord>`, the DELETING settle window and paging bounds — there is no timer, no occurrence computation, no flexible-window jitter selection and no target dispatcher anywhere in the crate, which is consistent with all three causes above.

## Steps to Reproduce
Common setup (all three causes): create an SQS queue and read its QueueArn; create an IAM role trusting scheduler.amazonaws.com with an inline sqs:SendMessage policy on that ARN.

Cause 1 (FLEXIBLE window bound):
1. nominal = now + 90s; expr = at(<nominal, %Y-%m-%dT%H:%M:%S>).
2. scheduler.create_schedule(Name=n, ScheduleExpression=expr, ScheduleExpressionTimezone='UTC', FlexibleTimeWindow={'Mode':'FLEXIBLE','MaximumWindowInMinutes':1}, State='ENABLED', Target={'Arn':queue_arn,'RoleArn':role_arn,'Input':marker}).
3. scheduler.get_schedule(Name=n) -> FlexibleTimeWindow.Mode == 'FLEXIBLE', MaximumWindowInMinutes == 1 (this part already passes).
4. Poll sqs.receive_message(QueueUrl=q, AttributeNames=['SentTimestamp'], WaitTimeSeconds=20) past nominal+1min. On AWS exactly one message arrives with SentTimestamp inside [nominal, nominal+60s]. On the emulator the queue stays empty indefinitely.
5. Repeat with MaximumWindowInMinutes=2 and a 1-minute lead (server-side variant): same result.

Cause 2 (server-paced rate cadence):
1. scheduler.create_schedule(Name=n, ScheduleExpression='rate(2 minutes)', FlexibleTimeWindow={'Mode':'OFF'}, State='ENABLED', Target={'Arn':queue_arn,'RoleArn':role_arn}).
2. Poll sqs.receive_message(QueueUrl=q, MaxNumberOfMessages=10, AttributeNames=['SentTimestamp'], WaitTimeSeconds=15) for up to 330s, collecting distinct MessageIds.
3. On AWS two messages arrive about 120s apart (gap in [80s,220s]). On the emulator zero messages arrive.

Cause 3 (group-scoped firing and cascade):
1. scheduler.create_schedule_group(Name=A) and (Name=B).
2. For the SAME schedule name n, create it in A with Input {'marker':'grp-a','t':'<aws.scheduler.scheduled-time>'} and in B with marker 'grp-b', both ScheduleExpression='rate(1 minute)', FlexibleTimeWindow={'Mode':'OFF'}, same queue target.
3. get_schedule(Name=n, GroupName=A).Arn != get_schedule(Name=n, GroupName=B).Arn; list_schedules(GroupName=A) == [n]; list_schedules(GroupName=B) == [n]; get_schedule(Name=n) (default group) -> ResourceNotFoundException 404.
4. Drain the queue: on AWS both the grp-a and the grp-b markers arrive within ~2 minutes. On the emulator nothing arrives, so the run stops here.
5. (unreachable on the emulator) delete_schedule_group(Name=A); wait until get_schedule_group(Name=A) -> ResourceNotFoundException; note that instant T. get_schedule(Name=n, GroupName=B) must still read State=ENABLED with B's ARN, and the queue must still receive a grp-b message whose rendered <aws.scheduler.scheduled-time> is later than T.
