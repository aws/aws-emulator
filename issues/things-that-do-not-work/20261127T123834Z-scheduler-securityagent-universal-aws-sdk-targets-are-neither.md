Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: scheduler__securityagent.securityagent-target-unknown-operation-rejected scheduler__securityagent.securityagent-target-payload-missing-agent-space-id-rejected scheduler__securityagent.one-time-schedule-updates-agent-space-then-deletes-itself scheduler__securityagent.schedule-in-agent-space-group-updates-the-agent-space scheduler__securityagent.agent-space-group-role-rejected-from-another-group
Issue-Key: scheduler__securityagent.securityagent-target-unknown-operation-rejected|_error_of#20|AssertionError|raise AssertionError(f"the call was expected to be refused but returned {out!r}")
Issue-Key: scheduler__securityagent.securityagent-target-payload-missing-agent-space-id-rejected|_refusal#22|AssertionError|raise AssertionError(f"expected a refusal, but the call succeeded: {result!r}")
Issue-Key: scheduler__securityagent.one-time-schedule-updates-agent-space-then-deletes-itself|run#24|AssertionError|assert (seen.get("name"), seen.get("description")) == (new_name, note), (
Issue-Key: scheduler__securityagent.schedule-in-agent-space-group-updates-the-agent-space|run#29|AssertionError|assert current.get("description") == marker, (
Issue-Key: scheduler__securityagent.agent-space-group-role-rejected-from-another-group|_refusal#25|AssertionError|raise AssertionError(f"expected a refusal, but the call succeeded: {result!r}")
Issue-Commit: f73c567c518c4ea32fbc68f3ca5cb3fd81df2cf2

# scheduler__securityagent: universal aws-sdk targets are neither validated at create time nor invoked at delivery time, and the role's trust aws:SourceArn is ignored

> _Note: this issue was identified against the code base at commit `f73c567c518c4ea32fbc68f3ca5cb3fd81df2cf2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateSchedule

## Description
Four net-new causes covering EventBridge Scheduler's universal (aws-sdk) target against Security Agent.

Cause 1 of 4: a universal target naming an operation the target service does not define is accepted.
AWS: CreateSchedule resolves the universal target ARN "arn:<partition>:scheduler:::aws-sdk:<service>:<operation>" against the target service's API and refuses an unknown operation with 400 ValidationException and the message "Invalid RequestJson provided. Reason The api <operation> is not valid for the service aws-sdk:<service>." — the fixed text is "Invalid RequestJson provided. Reason The api ", " is not valid for the service aws-sdk:" and the trailing '.'; the operation name and the service token come from the target ARN in the request. Nothing is stored, so GetSchedule answers ResourceNotFoundException "Schedule <name> does not exist."
Emulator: returns 200 with a ScheduleArn for "arn:aws:scheduler:::aws-sdk:securityagent:renameAgentSpace" and stores the schedule.
Why: a typo in a universal target's operation is accepted and surfaces, if ever, only as an undiagnosable delivery failure.
Fix: the universal-target resolution in the scheduler crate's CreateSchedule handler.
Traces: scheduler__securityagent.securityagent-target-unknown-operation-rejected

Cause 2 of 4: a universal target's Input is not checked against the target operation's request shape, so a payload missing a required member is accepted.
AWS: CreateSchedule validates the Input JSON against the resolved operation's request shape and refuses a payload whose required members are absent with 400 ValidationException "Invalid RequestJson provided. Reason Request payload is missing the following field(s): <MemberName>[, <MemberName>…]." — the fixed text is "Invalid RequestJson provided. Reason Request payload is missing the following field(s): " and the trailing '.'; the member names are the shape's required members the payload does not carry. Note the keys are the SHAPE's member names (AgentSpaceId), not the REST wire's lower-camel spellings (agentSpaceId), so a payload keyed in wire form is the missing-field case.
Emulator: returns 200 with a ScheduleArn and stores the schedule.
Why: a malformed universal-target payload is only discovered at delivery, if at all; the create-time contract the operation documents is unenforced.
Fix: the same universal-target resolution path — validate Input against the resolved request shape.
Traces: scheduler__securityagent.securityagent-target-payload-missing-agent-space-id-rejected

Cause 3 of 4: a universal aws-sdk target is never invoked when the schedule fires — no call reaches the target service and no dead-letter event is produced.
AWS: when an occurrence fires, Scheduler assumes the execution role and makes the named AWS-SDK call with the schedule's Input as the request, so the target resource changes; for a one-time at() schedule with ActionAfterCompletion DELETE the call happens once and the schedule is then gone, and for a rate() schedule it happens again each period. Here the call is securityagent:updateAgentSpace, so the Agent Space's name and description become the values the Input carried.
Emulator: the Agent Space is never touched — after 300 s (one-time schedule) and after 240 s (rate schedule) it still reads its pre-schedule name and description, and nothing is dead-lettered either.
Why: every universal-target schedule is a silent no-op; a client cannot distinguish "delivered" from "never ran", and no dead-letter event records the absence.
Fix: the occurrence-delivery path in the scheduler crate — the universal (aws-sdk) target invoker.
Traces: scheduler__securityagent.one-time-schedule-updates-agent-space-then-deletes-itself, scheduler__securityagent.schedule-in-agent-space-group-updates-the-agent-space

Cause 4 of 4: the execution role's trust aws:SourceArn condition is not evaluated, so a role trusted only for one schedule group is accepted in another.
AWS: Scheduler assumes the execution role with aws:SourceArn = the schedule's own ARN; a role whose trust condition names only group A's schedules is not assumable for a schedule in group B, and CreateSchedule there is refused 400 ValidationException "The execution role you provide must allow AWS EventBridge Scheduler to assume the role." (fixed text). Nothing is stored. Once the trust also names group B, the identical request is accepted.
Emulator: returns 200 with a ScheduleArn for the schedule in the group the trust does not name.
Why: the documented confused-deputy scoping for a Security Agent execution role is inert; a role intended for one Agent Space's group is usable from any group.
Fix: the execution-role assumability check in CreateSchedule must evaluate the trust policy's aws:SourceArn condition against the schedule ARN.
Traces: scheduler__securityagent.agent-space-group-role-rejected-from-another-group

## Evidence
Cause 1 — scheduler__securityagent.securityagent-target-unknown-operation-rejected:
  expected ValidationException "Invalid RequestJson provided. Reason The api renameAgentSpace is not valid for the service aws-sdk:securityagent."
  got      HTTP 200 {"ScheduleArn": "arn:aws:scheduler:<region>:<account>:schedule/default/<name>"}

Cause 2 — scheduler__securityagent.securityagent-target-payload-missing-agent-space-id-rejected:
  Target.Arn = arn:aws:scheduler:::aws-sdk:securityagent:updateAgentSpace, Input = {"agentSpaceId": "<id>", "name": "<name>"} (wire-cased keys)
  expected ValidationException "Invalid RequestJson provided. Reason Request payload is missing the following field(s): AgentSpaceId."
  got      HTTP 200 {"ScheduleArn": "arn:aws:scheduler:<region>:<account>:schedule/default/<name>"}

Cause 3 — scheduler__securityagent.one-time-schedule-updates-agent-space-then-deletes-itself:
  expected the Agent Space to read the name/description the schedule's Input carried
  got      after 300 s it still reads name=<its original name> description='before'
  scheduler__securityagent.schedule-in-agent-space-group-updates-the-agent-space:
  got      after 240 s the Agent Space description is still 'initial description'

Cause 4 — scheduler__securityagent.agent-space-group-role-rejected-from-another-group:
  expected ValidationException "The execution role you provide must allow AWS EventBridge Scheduler to assume the role." for a CreateSchedule in a group the role's trust does not name
  got      HTTP 200 {"ScheduleArn": "arn:aws:scheduler:<region>:<account>:schedule/<other-group>/<name>"}

## Steps to Reproduce
1. Boot the emulator. Create a Security Agent Agent Space and note its id, name and description. Create an IAM role trusted by scheduler.amazonaws.com granting securityagent:UpdateAgentSpace on it.
2. Cause 1: CreateSchedule with Target.Arn = "arn:aws:scheduler:::aws-sdk:securityagent:renameAgentSpace" — expect ValidationException "Invalid RequestJson provided. Reason The api renameAgentSpace is not valid for the service aws-sdk:securityagent.", observe 200.
3. Cause 2: CreateSchedule with Target.Arn = "arn:aws:scheduler:::aws-sdk:securityagent:updateAgentSpace" and Input = {"agentSpaceId": "<id>", "name": "<n>"} — expect ValidationException "… missing the following field(s): AgentSpaceId.", observe 200.
4. Cause 3: CreateSchedule with the same universal target, Input = {"AgentSpaceId": "<id>", "Name": "<new>", "Description": "<note>"}, ScheduleExpression = at(<now+1m>) and ActionAfterCompletion = DELETE. Poll GetAgentSpace — expect the new name/description, observe the originals unchanged after 300 s and no dead-letter event. Repeat with rate(1 minute).
5. Cause 4: give the role a trust Condition scoping aws:SourceArn to group A's schedules, then CreateSchedule in group B with that role — expect the assume-role ValidationException, observe 200.
