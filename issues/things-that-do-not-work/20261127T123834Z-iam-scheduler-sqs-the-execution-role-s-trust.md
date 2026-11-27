Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iam__scheduler__sqs.trust-source-arn-wildcard-under-string-equals-rejected iam__scheduler__sqs.update-schedule-role-with-literal-wildcard-source-arn-rejected iam__scheduler__sqs.trust-scoped-to-default-group-named-group-schedule-rejected iam__scheduler__sqs.trust-deny-on-source-arn-overrides-allow-rejected iam__scheduler__sqs.create-schedule-kms-key-arn-longer-than-2048-characters-rejected
Issue-Key: iam__scheduler__sqs.trust-source-arn-wildcard-under-string-equals-rejected|_expect_refusal#26|AssertionError|raise AssertionError(f"the call was expected to be refused but returned {got!r}")
Issue-Key: iam__scheduler__sqs.update-schedule-role-with-literal-wildcard-source-arn-rejected|_rejection#21|AssertionError|raise AssertionError(f"expected the request to be refused; it returned {result!r}")
Issue-Key: iam__scheduler__sqs.trust-scoped-to-default-group-named-group-schedule-rejected|_refused#26|AssertionError|raise AssertionError(f"expected a refusal, got a success: {answer!r}")
Issue-Key: iam__scheduler__sqs|CreateSchedule|ValidationException
Issue-Commit: f73c567c518c4ea32fbc68f3ca5cb3fd81df2cf2

# iam__scheduler__sqs: the execution role's trust-policy conditions are not evaluated — a literal-wildcard StringEquals aws:SourceArn matches, a SourceArn scoped to another group is ignored, a Deny statement blocks the allowed group, and KmsKeyArn is resolved instead of validated

> _Note: this issue was identified against the code base at commit `f73c567c518c4ea32fbc68f3ca5cb3fd81df2cf2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateSchedule

## Description
Three net-new causes around what EventBridge Scheduler checks on CreateSchedule/UpdateSchedule before it stores a schedule.

Cause 1 of 3: the aws:SourceArn condition in the execution role's trust policy is not evaluated — a literal '*' under StringEquals is treated as a wildcard, and a SourceArn naming a different schedule group is ignored.
AWS: Scheduler assumes the execution role with aws:SourceArn set to the SCHEDULE's own ARN (and aws:SourceAccount set to the account), and the trust policy's condition operator is applied literally: StringEquals compares the value character for character, so a value containing '*' matches nothing and the role is not assumable; only ArnLike (or StringLike) treats '*' as a wildcard. When the role cannot be assumed for that schedule, CreateSchedule/UpdateSchedule is refused with 400 ValidationException and the fixed message "The execution role you provide must allow AWS EventBridge Scheduler to assume the role." Nothing is stored: GetSchedule answers ResourceNotFoundException "Schedule <name> does not exist.", the refused schedule never appears in ListSchedules, and on UpdateSchedule the stored RoleArn, ScheduleExpression, Target.Input and LastModificationDate are all unchanged. Reference: cross-service confused-deputy prevention for EventBridge Scheduler.
Emulator: accepts every one of these — it returns 200 and stores the schedule for a role whose trust names a wildcard schedule-group ARN under StringEquals (on CreateSchedule and on UpdateSchedule), and for a role whose trust names only the default group when the schedule is created in a named group.
Why: the confused-deputy guard a customer writes into the trust policy is silently inert, so a role scoped to one group or one schedule is usable everywhere, and the difference between StringEquals and ArnLike — the whole point of the documented pattern — has no observable effect.
Fix: the execution-role assumability check in the scheduler crate's CreateSchedule/UpdateSchedule handlers must evaluate the trust policy's Condition block against aws:SourceArn (the schedule ARN) and aws:SourceAccount with operator-correct matching.
Traces: iam__scheduler__sqs.trust-source-arn-wildcard-under-string-equals-rejected, iam__scheduler__sqs.update-schedule-role-with-literal-wildcard-source-arn-rejected, iam__scheduler__sqs.trust-scoped-to-default-group-named-group-schedule-rejected

Cause 2 of 3: an explicit Deny statement in the trust policy makes the role unusable everywhere, not only where it is denied.
AWS: IAM policy evaluation is Deny-wins per request — a trust policy with an account-wide Allow plus a Deny conditioned on one group's aws:SourceArn leaves the role assumable for a schedule in ANY OTHER group (so CreateSchedule there returns 200 and the schedule delivers), and unassumable only for a schedule in the denied group, where CreateSchedule is refused 400 ValidationException "The execution role you provide must allow AWS EventBridge Scheduler to assume the role."
Emulator: refuses the create in the ALLOWED group with that same ValidationException, so the role is treated as unusable as soon as the trust policy contains any Deny.
Why: the opposite polarity of the correct answer — a legal schedule cannot be created, and the per-request Deny the customer wrote is applied to every request.
Fix: the trust-policy evaluation reached from the same assumability check — evaluate Allow and Deny statements per request against the request's own condition keys instead of rejecting the whole policy.
Traces: iam__scheduler__sqs.trust-deny-on-source-arn-overrides-allow-rejected

Cause 3 of 3: CreateSchedule resolves KmsKeyArn against the key store and refuses an unresolvable key, so the modelled length bound is never reached.
AWS: KmsKeyArn is validated as a field only. Smithy: scheduler#KmsKeyArn @length min 1 max 2048, @pattern ^arn:aws(-[a-z]+)?:kms:[a-z0-9\-]+:\d{12}:(key|alias)\/[0-9a-zA-Z-_]*$. A 2048-character key ARN that matches the pattern is ACCEPTED and stored, and GetSchedule returns it unchanged; 2049 characters is refused 400 ValidationException "1 validation error detected: Value '<the submitted arn>' at 'kmsKeyArn' failed to satisfy constraint: Member must have length less than or equal to 2048" — fixed text with the submitted value and the bound 2048 as its variable parts.
Emulator: refuses the 2048-character ARN with 400 ValidationException "Unable to access the KMS key <the submitted arn>", so the schedule is never created and the length bound is never exercised.
Why: a schedule AWS accepts cannot be created, and the one refusal a client can rely on (the length bound) is replaced by a key-access message AWS does not emit here.
Fix: the KmsKeyArn handling in the CreateSchedule handler — apply the modelled length/pattern validation and do not require the key to resolve.
Traces: iam__scheduler__sqs.create-schedule-kms-key-arn-longer-than-2048-characters-rejected

## Evidence
Cause 1 — iam__scheduler__sqs.trust-source-arn-wildcard-under-string-equals-rejected:
  role trust: Allow sts:AssumeRole to scheduler.amazonaws.com, Condition {"StringEquals": {"aws:SourceArn": "arn:aws:scheduler:<region>:<account>:schedule-group/<stem>-*"}}
  expected ValidationException "The execution role you provide must allow AWS EventBridge Scheduler to assume the role."
  got      HTTP 200 {"ScheduleArn": "arn:aws:scheduler:<region>:<account>:schedule/<group>/<name>"}
  (the ArnLike twin on the same value is the accepted control, and it was accepted — so only the operator distinction is lost)
  iam__scheduler__sqs.update-schedule-role-with-literal-wildcard-source-arn-rejected: same, via UpdateSchedule — expected the refusal and an unchanged stored role; got HTTP 200 and the role swapped.
  iam__scheduler__sqs.trust-scoped-to-default-group-named-group-schedule-rejected: trust names only the default group's ARN under StringEquals; a CreateSchedule in a NAMED group was expected to be refused with the same ValidationException; got HTTP 200 {"ScheduleArn": "…/schedule/<named-group>/<name>"}.

Cause 2 — iam__scheduler__sqs.trust-deny-on-source-arn-overrides-allow-rejected:
  role trust: Allow sts:AssumeRole to scheduler.amazonaws.com account-wide, plus Deny conditioned on aws:SourceArn = <shut group ARN>
  expected HTTP 200 for a CreateSchedule in the OPEN group (and the refusal only in the shut group)
  got      ValidationException "The execution role you provide must allow AWS EventBridge Scheduler to assume the role." on the OPEN group's create, raised out of the check

Cause 3 — iam__scheduler__sqs.create-schedule-kms-key-arn-longer-than-2048-characters-rejected:
  expected HTTP 200 for KmsKeyArn = "arn:aws:kms:<region>:<account>:key/" + 'a'-padding to exactly 2048 characters, then GetSchedule echoing it; and ValidationException "1 validation error detected: Value '<arn>' at 'kmsKeyArn' failed to satisfy constraint: Member must have length less than or equal to 2048" at 2049
  got      ValidationException "Unable to access the KMS key arn:aws:kms:<region>:<account>:key/aaaa…a" already at 2048 characters

## Steps to Reproduce
1. Boot the emulator. Create an SQS queue and note its ARN.
2. Cause 1: create an IAM role whose trust is {Allow, Principal scheduler.amazonaws.com, sts:AssumeRole, Condition {StringEquals: {aws:SourceAccount: <account>, aws:SourceArn: "arn:aws:scheduler:<region>:<account>:schedule-group/<stem>-*"}}} and grant it sqs:SendMessage. Create schedule group "<stem>-a". CreateSchedule in that group with Target.RoleArn = that role — expect ValidationException "The execution role you provide must allow AWS EventBridge Scheduler to assume the role.", observe 200. Repeat the same swap via UpdateSchedule. Separately, scope a role's trust aws:SourceArn to the DEFAULT group's ARN under StringEquals and CreateSchedule in a named group — expect the same refusal, observe 200.
3. Cause 2: create a role whose trust holds an account-wide Allow for scheduler.amazonaws.com plus a Deny statement conditioned on aws:SourceArn = <group B's ARN>. CreateSchedule in group A — expect 200, observe the assume-role ValidationException.
4. Cause 3: CreateSchedule with KmsKeyArn = "arn:aws:kms:<region>:<account>:key/" padded with 'a' to exactly 2048 characters — expect 200 and GetSchedule returning the same 2048 characters, observe ValidationException "Unable to access the KMS key …". Then try 2049 characters and expect the kmsKeyArn length message.
