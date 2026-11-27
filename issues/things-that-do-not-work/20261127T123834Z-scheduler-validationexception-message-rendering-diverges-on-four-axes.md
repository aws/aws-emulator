Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: scheduler.create-schedule-ecs-placement-strategy-over-max-entries-rejected scheduler.create-schedule-ecs-task-tags-count-bounds-rejected scheduler.create-schedule-group-tags-beyond-per-resource-limit-rejected scheduler.ecs-placement-constraints-over-ten-rejected scheduler.create-schedule-ecs-awsvpc-security-groups-count-bounds-rejected scheduler.untag-resource-key-over-maximum-length-rejected scheduler.untag-resource-tag-key-over-128-chars-rejected scheduler.ecs-target-security-group-entry-length-bounds-rejected scheduler.create-schedule-ecs-launch-type-outside-enum-rejected scheduler.create-schedule-ecs-placement-constraint-unknown-type-rejected scheduler.create-schedule-flexible-window-mode-outside-enum-rejected scheduler.create-schedule-client-token-outside-length-bounds-rejected
Issue-Shape-Id: com.amazonaws.scheduler#AWSChronosService
Issue-Key: scheduler.create-schedule-ecs-placement-strategy-over-max-entries-rejected|run#13|AssertionError|assert got_over == want_over, f"six strategies: expected {want_over!r}, got {got_over!r}"
Issue-Key: scheduler.create-schedule-ecs-task-tags-count-bounds-rejected|run#13|AssertionError|assert got == want, f"51 task tags gave {got!r}, expected {want!r}"
Issue-Key: scheduler.create-schedule-group-tags-beyond-per-resource-limit-rejected|run#6|AssertionError|assert refusal == ("ValidationException", expected), (
Issue-Key: scheduler.ecs-placement-constraints-over-ten-rejected|run#17|AssertionError|assert got == want, f"eleven placement constraints were answered {got!r}, not {want!r}"
Issue-Key: scheduler.create-schedule-ecs-awsvpc-security-groups-count-bounds-rejected|run#8|AssertionError|assert got == want, f"{count} security groups gave {got!r}, expected {want!r}"
Issue-Key: scheduler.untag-resource-key-over-maximum-length-rejected|run#9|AssertionError|assert got == (
Issue-Key: scheduler.untag-resource-tag-key-over-128-chars-rejected|run#12|AssertionError|assert (code, message) == ("ValidationException", expected), (
Issue-Key: scheduler.ecs-target-security-group-entry-length-bounds-rejected|run#20|AssertionError|assert got == want, f"an {label} security group entry answered {got!r}, expected {want!r}"
Issue-Key: scheduler.create-schedule-ecs-launch-type-outside-enum-rejected|run#16|AssertionError|assert got == expected, f"expected {expected!r}, got {got!r}"
Issue-Key: scheduler.create-schedule-ecs-placement-constraint-unknown-type-rejected|run#11|AssertionError|assert refused == want, f"placement constraint type 'spread' gave {refused!r}, expected {want!r}"
Issue-Key: scheduler.create-schedule-flexible-window-mode-outside-enum-rejected|run#9|AssertionError|assert got == want, f"Mode {bad!r}: expected {want!r}, got {got!r}"
Issue-Key: scheduler.create-schedule-client-token-outside-length-bounds-rejected|run#18|AssertionError|assert got == want, f"an empty ClientToken was answered {got!r}, expected {want!r}"
Issue-Commit: f73c567c518c4ea32fbc68f3ca5cb3fd81df2cf2

# scheduler: ValidationException message rendering diverges on four axes — collection values printed as "<N> items", element violations reported at the member path, enum value sets in model order, and multi-error ordering

> _Note: this issue was identified against the code base at commit `f73c567c518c4ea32fbc68f3ca5cb3fd81df2cf2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateSchedule

## Description
The emulator's validation-message renderer for EventBridge Scheduler produces text that differs from AWS's for four distinct reasons. Status (400) and error code (ValidationException) are correct in every case; only the Message diverges. Four net-new causes.

Cause 1 of 4: a constraint violated on a LIST ITSELF renders the offending value as "'<N> items'" instead of the full bracketed rendering of the submitted list.
AWS: the message template is "1 validation error detected: Value '<rendered value>' at '<member path>' failed to satisfy constraint: <constraint sentence>". For a list member, <rendered value> is the list rendered as '[' + the entries joined by ', ' + ']', each entry rendered in the shape's own Java-ish form — a structure as "Name(member=value, member=value)" (e.g. "PlacementStrategy(type=spread, field=memory)", "PlacementConstraint(type=memberOf, expression=...)"), a map entry as "{key=value}", a string entry as the bare string; an empty list renders as '[]'. Smithy: scheduler#PlacementStrategies @length max 5; scheduler#PlacementConstraints @length max 10; scheduler#Tags (ECS task tags) @length min 0 max 50; scheduler#TagList @length min 0 max 200; scheduler#SecurityGroups @length min 1 max 5. The constraint sentence is "Member must have length less than or equal to <max>" / "... greater than or equal to <min>".
Emulator: prints the item COUNT in place of the value — "Value '6 items' at 'target.ecsParameters.placementStrategy' ...", "Value '51 items' at 'target.ecsParameters.tags' ...", "Value '201 items' at 'tags' ...", "Value '11 items' at 'target.ecsParameters.placementConstraints' ...", and "Value '0 items' at 'target.ecsParameters.networkConfiguration.awsvpcConfiguration.securityGroups' ..." where AWS prints "Value '[]'".
Why: a client that parses or asserts on the refusal message (or shows it to an operator) cannot recover which entries were submitted, and any equality test against AWS's wording fails.
Fix: the shared validation-message renderer in the scheduler crate — the collection-value formatter that feeds the "Value '...'" slot.
Traces: scheduler.create-schedule-ecs-placement-strategy-over-max-entries-rejected, scheduler.create-schedule-ecs-task-tags-count-bounds-rejected, scheduler.create-schedule-group-tags-beyond-per-resource-limit-rejected, scheduler.ecs-placement-constraints-over-ten-rejected, scheduler.create-schedule-ecs-awsvpc-security-groups-count-bounds-rejected

Cause 2 of 4: a constraint violated by a list ELEMENT is reported at the element's own path with that one constraint, where AWS reports it at the LIST's path with the element shape's whole constraint set.
AWS: when a list member's target constraint is violated, the message names the list member path and the constraint sentence is "Member must satisfy constraint: [<sentence>, <sentence>]" listing the element shape's constraints, with the value slot holding the rendered LIST. Smithy: scheduler#SecurityGroups member SecurityGroup @length min 1 max 1000; scheduler#Subnets member Subnet @length min 1 max 1000; scheduler#TagKeyList member TagKey @length min 1 max 128. So an over-long tag key in UntagResource yields "1 validation error detected: Value '[<key>, <key>]' at 'tagKeys' failed to satisfy constraint: Member must satisfy constraint: [Member must have length less than or equal to 128, Member must have length greater than or equal to 1]" — the fixed text is everything but the rendered list, which comes from the request.
Emulator: reports the element path and a single sentence — "Value '<key>' at 'tagKeys.2.member' failed to satisfy constraint: Member must have length less than or equal to 128", and "Value '' at 'target.ecsParameters.networkConfiguration.awsvpcConfiguration.securityGroups.1.member' failed to satisfy constraint: Member must have length greater than or equal to 1".
Why: the member path the emulator invents ('tagKeys.2.member') is not a path AWS ever emits, and the constraint set is truncated to the one bound that happened to trip, so clients keying on the path or the constraint list diverge.
Fix: the same validation-message renderer — the path/constraint-set composition for list-element violations.
Traces: scheduler.untag-resource-key-over-maximum-length-rejected, scheduler.untag-resource-tag-key-over-128-chars-rejected, scheduler.ecs-target-security-group-entry-length-bounds-rejected

Cause 3 of 4: an enum-constraint refusal lists the enum value set in the Smithy model's declaration order instead of the order AWS prints.
AWS: the sentence is "Member must satisfy enum value set: [<v>, <v>, ...]" and the order AWS emits is not the model's: target.ecsParameters.launchType prints [FARGATE, EXTERNAL, EC2]; target.ecsParameters.placementConstraints.N.member.type prints [memberOf, distinctInstance]; flexibleTimeWindow.mode prints [FLEXIBLE, OFF]. The fixed text is the sentence and the value list; the rejected value in the "Value '...'" slot comes from the request.
Emulator: prints the model's declaration order — Smithy scheduler#LaunchType values are [EC2, FARGATE, EXTERNAL], scheduler#PlacementConstraintType [distinctInstance, memberOf], scheduler#FlexibleTimeWindowMode [OFF, FLEXIBLE] — and those are exactly the orders served.
Why: the refusal message is a requirement; a client or test comparing it to AWS's sees a different value set for every enum member.
Fix: the enum-constraint arm of the validation-message renderer needs the per-enum emitted order, not the model order.
Traces: scheduler.create-schedule-ecs-launch-type-outside-enum-rejected, scheduler.create-schedule-ecs-placement-constraint-unknown-type-rejected, scheduler.create-schedule-flexible-window-mode-outside-enum-rejected

Cause 4 of 4: when one member violates both a pattern and a length bound, the two errors are emitted in the wrong order inside the "N validation errors detected" message.
AWS: for an empty ClientToken the message is "2 validation errors detected: Value '' at 'clientToken' failed to satisfy constraint: Member must satisfy regular expression pattern: [a-zA-Z0-9-_]+; Value '' at 'clientToken' failed to satisfy constraint: Member must have length greater than or equal to 1" — the pattern error first, the length error second, joined by "; ". Smithy: scheduler#ClientToken @length min 1 max 64, @pattern ^[a-zA-Z0-9-_]+$.
Emulator: emits the length error first and the pattern error second, same wording otherwise.
Why: the message is a requirement; ordering is part of it, and a client asserting on the whole Message diverges.
Fix: the multi-error accumulation order in the scheduler crate's validation pass.
Traces: scheduler.create-schedule-client-token-outside-length-bounds-rejected

## Evidence
Cause 1 — scheduler.create-schedule-ecs-placement-strategy-over-max-entries-rejected:
  expected ValidationException "1 validation error detected: Value '[PlacementStrategy(type=spread, field=attribute:ecs.availability-zone), PlacementStrategy(type=binpack, field=memory), ...]' at 'target.ecsParameters.placementStrategy' failed to satisfy constraint: Member must have length less than or equal to 5"
  got      ValidationException "1 validation error detected: Value '6 items' at 'target.ecsParameters.placementStrategy' failed to satisfy constraint: Member must have length less than or equal to 5"
  likewise '51 items' vs the rendered [{key00=value00}, ...] list (ECS task tags), '201 items' vs the rendered [Tag(key=..., value=...), ...] list (CreateScheduleGroup tags), '11 items' vs the rendered PlacementConstraint(...) list, and '0 items' vs '[]' (awsvpc securityGroups).

Cause 2 — scheduler.untag-resource-key-over-maximum-length-rejected:
  expected ValidationException "1 validation error detected: Value '[tier, kkk…k]' at 'tagKeys' failed to satisfy constraint: Member must satisfy constraint: [Member must have length less than or equal to 128, Member must have length greater than or equal to 1]"
  got      ValidationException "1 validation error detected: Value 'kkk…k' at 'tagKeys.2.member' failed to satisfy constraint: Member must have length less than or equal to 128"
  scheduler.ecs-target-security-group-entry-length-bounds-rejected:
  expected "… Value '[]' at '…awsvpcConfiguration.securityGroups' failed to satisfy constraint: Member must satisfy constraint: [Member must have length less than or equal to 1000, Member must have length greater than or equal to 1]"
  got      "… Value '' at '…awsvpcConfiguration.securityGroups.1.member' failed to satisfy constraint: Member must have length greater than or equal to 1"

Cause 3 — scheduler.create-schedule-ecs-launch-type-outside-enum-rejected:
  expected "… Value 'FARGATE_SPOT' at 'target.ecsParameters.launchType' failed to satisfy constraint: Member must satisfy enum value set: [FARGATE, EXTERNAL, EC2]"
  got      "… enum value set: [EC2, FARGATE, EXTERNAL]"
  placementConstraints type: expected "[memberOf, distinctInstance]", got "[distinctInstance, memberOf]"
  flexibleTimeWindow.mode: expected "[FLEXIBLE, OFF]", got "[OFF, FLEXIBLE]"

Cause 4 — scheduler.create-schedule-client-token-outside-length-bounds-rejected:
  expected "2 validation errors detected: Value '' at 'clientToken' failed to satisfy constraint: Member must satisfy regular expression pattern: [a-zA-Z0-9-_]+; Value '' at 'clientToken' failed to satisfy constraint: Member must have length greater than or equal to 1"
  got      the same two clauses with the length clause first and the pattern clause second.

## Steps to Reproduce
1. Boot the emulator and point a botocore scheduler client at it with parameter_validation=False (so the service, not the SDK, refuses).
2. Cause 1: CreateSchedule with Target.EcsParameters.PlacementStrategy holding 6 entries; CreateSchedule with Target.EcsParameters.Tags holding 51 entries; CreateScheduleGroup with 201 Tags; CreateSchedule with 11 PlacementConstraints; CreateSchedule with NetworkConfiguration.awsvpcConfiguration.SecurityGroups = []. Read Error.Message.
3. Cause 2: UntagResource on a schedule-group ARN with TagKeys=["tier", "k"*129]; CreateSchedule with awsvpcConfiguration.SecurityGroups=[""]. Read Error.Message.
4. Cause 3: CreateSchedule with EcsParameters.LaunchType="FARGATE_SPOT"; with PlacementConstraints=[{"type":"spread"}]; with FlexibleTimeWindow={"Mode":"off"}. Read Error.Message.
5. Cause 4: CreateSchedule with ClientToken="". Read Error.Message.
