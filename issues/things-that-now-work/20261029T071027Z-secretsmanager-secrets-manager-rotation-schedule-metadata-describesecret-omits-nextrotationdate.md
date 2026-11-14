Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: secretsmanager.rotation-schedule-expression-with-duration-places-next-rotation-in-the-window secretsmanager.rotation-rules-sets-next-rotation-date-window
Issue-Shape-Id: com.amazonaws.secretsmanager#secretsmanager
Issue-Key: secretsmanager.rotation-schedule-expression-with-duration-places-next-rotation-in-the-window|run#26|TimeoutError|raise TimeoutError("DescribeSecret showing rotation enabled: the run budget ended first; not graded")
Issue-Key: secretsmanager.rotation-rules-sets-next-rotation-date-window|_poll_until_rotated#24|TimeoutError|raise TimeoutError("rotation completing: the run budget ended first; not graded")
Issue-Commit: 8fe1b88606c77dfa59d771b1a110885b6dfaed56

# Secrets Manager rotation schedule metadata: DescribeSecret omits NextRotationDate for every cron() ScheduleExpression, and never recomputes it from a completed rotation (2 causes)

> _Note: this issue was identified against the code base at commit `8fe1b88606c77dfa59d771b1a110885b6dfaed56`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RotateSecret

## Description
Two distinct root causes in how this service derives DescribeSecret's NextRotationDate from the rotation rules RotateSecret stores. Both must be closed before submitting.

Cause 1 of 2: a cron() ScheduleExpression produces NO NextRotationDate at all, and Duration is stored but never used.
AWS: Smithy RotationRulesType$ScheduleExpression -- "A cron() or rate() expression that defines the schedule for rotating your secret. Secrets Manager rotation schedules use UTC time zone. Secrets Manager rotates your secret any time during a rotation window"; RotationRulesType$Duration (length 2..3, pattern ^[0-9]+h$) -- "The length of the rotation window in hours, for example 3h for a three hour window... The window starts according to the ScheduleExpression." DescribeSecret models NextRotationDate (timestamp), and RotationRulesType$AutomaticallyAfterDays states "In DescribeSecret and ListSecrets, this value is calculated from the rotation schedule after every successful rotation." So after RotateSecret with ScheduleExpression="cron(0 16 ? * SAT *)", Duration="3h", RotateImmediately=false, DescribeSecret reports RotationEnabled=true, round-trips both rules, and reports NextRotationDate at the next cron occurrence -- a Saturday, inside the window the cron start plus Duration define (16:00-19:00 UTC for this expression) -- and no LastRotatedDate, with AWSCURRENT left on the pre-existing version.
Emulator: the schedule parser answers "no interval" for EVERY cron() expression (it only counts the six fields, then returns none), and next_rotation_date is only ever assigned as call_instant + interval. With no interval there is no instant, so NextRotationDate is omitted permanently; Duration is round-tripped but never placed. DescribeSecret therefore never satisfies RotationEnabled=true AND NextRotationDate present, no matter how long a client polls -- the replay's bounded poll never converges and the check ends in a TimeoutError without ever reaching its window assertions. RotateSecret itself returns 200 and does enable rotation, so the divergence is confined to the derived next-rotation instant.
Why: cron() is the production-standard way to schedule Secrets Manager rotation. A client that reads DescribeSecret["NextRotationDate"] to display, alert on, or gate compliance against the next rotation gets a missing key (KeyError / nil) for every cron-scheduled secret, and a client that computes its own window from Duration sees that field have no effect on anything the service reports.
Fix: the next-rotation computation in this service's state -- the schedule-expression parser and the rotation-configuration path that stamps next_rotation_date must evaluate a cron() occurrence (UTC) and place NextRotationDate inside the ScheduleExpression+Duration window, instead of returning "no interval" and leaving the field unset; DescribeSecret's projection then carries it.
Traces: secretsmanager.rotation-schedule-expression-with-duration-places-next-rotation-in-the-window.

Cause 2 of 2: NextRotationDate is stamped once at the RotateSecret call instant and never recomputed, so it cannot track the last rotation.
AWS: Smithy RotationRulesType$AutomaticallyAfterDays -- "The number of days between rotations of the secret... If you use this field to set the rotation schedule, Secrets Manager calculates the next rotation date based on the previous rotation... In DescribeSecret and ListSecrets, this value is calculated from the rotation schedule after every successful rotation. Manually updating the secret value by calling PutSecretValue or UpdateSecret is considered a valid rotation." So with AutomaticallyAfterDays=30, once a rotation completes DescribeSecret reports LastRotatedDate and a NextRotationDate within a day of LastRotatedDate + 30 days, and a later PutSecretValue/UpdateSecret re-bases it again.
Emulator: next_rotation_date is assigned in exactly one place -- the rotation-configuration path, as the RotateSecret call instant plus the interval -- and cleared when rotation is cancelled. Nothing recomputes it when a rotation completes, nor on PutSecretValue/UpdateSecret, so it is anchored to the configure call forever rather than to the previous rotation. Compounding it, LastRotatedDate is never set because the rotation function is never executed, so this check's poll for "AWSCURRENT moved and LastRotatedDate present" never converges and it ends in a TimeoutError before grading the window. That rotation-never-runs half is the root cause already carried by open issues 20261028T011946Z-secretsmanager-rotatesecret-with-rotateimmediately-true-never-executes-the.md, 20261028T061500Z-secretsmanager-rotatesecret-rotateimmediately-true-never-runs-the-rotation.md and 20261027T052624Z-dynamodb-lambda-s3-secretsmanager-secretsmanager-rotatesecret-never-drives-a-rotation-so-awspending.md -- none of which carries this check's replay key, and none of which covers the re-anchoring of NextRotationDate described here.
Why: a client that reads LastRotatedDate and NextRotationDate together to prove rotation compliance ("rotated within N days, next rotation due at T") gets an inconsistent pair: a next-rotation instant measured from when rotation was configured, never from the rotation that actually happened, and it never advances as the secret rotates.
Fix: the rotation bookkeeping in this service's state -- recompute next_rotation_date from last_rotated_date (and from a manual PutSecretValue/UpdateSecret write) on every successful rotation, not only when rotation is configured.
Traces: secretsmanager.rotation-rules-sets-next-rotation-date-window.

## Evidence
Cause 1 -- Expected (AWS): after RotateSecret(SecretId=<arn>, RotationLambdaARN=<fn>, RotationRules={"ScheduleExpression":"cron(0 16 ? * SAT *)","Duration":"3h"}, RotateImmediately=False) -> 200, then DescribeSecret -> RotationEnabled=true, RotationLambdaARN=<fn>, RotationRules.ScheduleExpression="cron(0 16 ? * SAT *)", RotationRules.Duration="3h", NextRotationDate present, falling on a Saturday with 16:00 UTC <= NextRotationDate <= 19:00 UTC and no later than the call instant + 7 days, and no LastRotatedDate.
Actual (emulator): RotateSecret -> 200 and rotation is enabled, but DescribeSecret never carries NextRotationDate -- the check's bounded poll for `RotationEnabled is True and "NextRotationDate" in d` never becomes true and the check terminates with `TimeoutError: DescribeSecret showing rotation enabled: the run budget ended first; not graded`. The emulator's own schedule parser is documented in-tree as reporting NextRotationDate "for the interval forms and OMIT[ting] it for a cron() schedule", i.e. no amount of waiting makes the field appear.

Cause 2 -- Expected (AWS): after RotateSecret(RotationRules={"AutomaticallyAfterDays":30}, RotateImmediately=True) and the rotation completing -> DescribeSecret.LastRotatedDate present, AWSCURRENT on a new version, and abs(NextRotationDate - (LastRotatedDate + 30 days)) < 86400 s.
Actual (emulator): AWSCURRENT never moves and LastRotatedDate is never set (the rotation function is never executed), so the poll for a completed rotation never converges and the check terminates with `TimeoutError: rotation completing: the run budget ended first; not graded`. Independently of that, NextRotationDate is only ever computed as the RotateSecret call instant + 30 days and is never re-derived from a rotation, so the LastRotatedDate+30d relation could not hold even once rotation runs.

Source of both: services/service-com-amazonaws-secretsmanager-secretsmanager/src/state.rs -- `parse_schedule_expression` (returns no interval for any cron(), ~lines 278-312), `parse_rotation_rules` (~lines 249-268), `configure_rotation` (the single assignment of next_rotation_date, ~lines 1781-1799) and `cancel_rotation` (clears it, ~line 1841); services/service-com-amazonaws-secretsmanager-secretsmanager/src/ops/rotate_secret.rs (~lines 313-354).

Note on both verdicts: each check ended in a harness TimeoutError worded "not graded" because the runner's overall budget expired during the poll. The underlying emulator behavior is nonetheless deterministic and verifiable without any timing -- a cron() schedule can never yield a NextRotationDate, and next_rotation_date has exactly one assignment site -- so these are filed as emulator defects, not as runner flakiness.

## Steps to Reproduce
Cause 1 (no timing involved):
1. CreateSecret(Name=<unique>, SecretString="sim2real-not-a-real-secret").
2. Create an IAM role trusted by lambda.amazonaws.com, create a python3.12 Lambda rotation function with it, and Lambda AddPermission(Principal="secretsmanager.amazonaws.com", Action="lambda:InvokeFunction", SourceArn=<secret arn>).
3. RotateSecret(SecretId=<secret arn>, RotationLambdaARN=<fn arn>, RotationRules={"ScheduleExpression": "cron(0 16 ? * SAT *)", "Duration": "3h"}, RotateImmediately=False) -> 200.
4. DescribeSecret(SecretId=<secret arn>): RotationEnabled is true and the rules round-trip, but there is no NextRotationDate key -- and repeating the DescribeSecret indefinitely never produces one. AWS reports NextRotationDate on the next Saturday inside 16:00-19:00 UTC.

Cause 2:
1. CreateSecret(Name=<unique>, SecretString="rotation-seed"), then set up the rotation role/function/permission as above.
2. RotateSecret(SecretId=<secret arn>, RotationLambdaARN=<fn arn>, RotationRules={"AutomaticallyAfterDays": 30}, RotateImmediately=True) -> 200.
3. DescribeSecret immediately: NextRotationDate == the RotateSecret call instant + 30 days.
4. Poll DescribeSecret: AWSCURRENT never moves off the original version and LastRotatedDate never appears, so NextRotationDate is never re-derived. AWS, after the rotation completes, reports LastRotatedDate and NextRotationDate within one day of LastRotatedDate + 30 days.
5. Also check the manual-rotation re-base: PutSecretValue(SecretId=<secret arn>, SecretString="next") does not move NextRotationDate either, though AWS treats that write as a valid rotation for schedule purposes.
