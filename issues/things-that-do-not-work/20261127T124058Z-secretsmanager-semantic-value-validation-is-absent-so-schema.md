Issue-Source: trace-sweep-divergence
Issue-Trace: secretsmanager-rotate-secret-cron-both-day-fields
Issue-Step: task#5
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: secretsmanager-rotate-secret-cron-both-day-fields secretsmanager-rotation-schedule-expression-grammar-rejected secretsmanager-put-secret-value-rotation-token-length-bounds secretsmanager-remove-regions-from-replication-list-and-region-length-limits
Issue-Shape-Id: com.amazonaws.secretsmanager#secretsmanager
Issue-Key: secretsmanager|RotateSecret|4xx->2xx|$.Message
Issue-Key: secretsmanager|RotateSecret|4xx->2xx|$.__type
Issue-Key: secretsmanager|RotateSecret|4xx->2xx|status
Issue-Key: secretsmanager|PutSecretValue|4xx->2xx|$.Message
Issue-Key: secretsmanager|PutSecretValue|4xx->2xx|$.__type
Issue-Key: secretsmanager|PutSecretValue|4xx->2xx|status
Issue-Key: secretsmanager|DescribeSecret|2xx->2xx|$.VersionIdsToStages.12121212-3434-4565-8787-909090909090
Issue-Key: secretsmanager|DescribeSecret|2xx->2xx|$.VersionIdsToStages.b4b4b4b4-c5c5-4d6d-8e7e-f8f8f8f8f8f8
Issue-Key: secretsmanager|DescribeSecret|2xx->2xx|$.VersionIdsToStages.f6f6f6f6-0707-4818-9292-a3a3a3a3a3a3[*]
Issue-Key: secretsmanager|RemoveRegionsFromReplication|4xx->2xx|$.Message
Issue-Key: secretsmanager|RemoveRegionsFromReplication|4xx->2xx|$.__type
Issue-Key: secretsmanager|RemoveRegionsFromReplication|4xx->2xx|status
Issue-Key: secretsmanager|DescribeSecret|2xx->2xx|$.NextRotationDate
Issue-Key: secretsmanager|DescribeSecret|2xx->2xx|$.RotationEnabled
Issue-Key: secretsmanager|DescribeSecret|2xx->2xx|$.RotationLambdaARN
Issue-Key: secretsmanager|DescribeSecret|2xx->2xx|$.RotationRules.AutomaticallyAfterDays
Issue-Key: secretsmanager|DescribeSecret|2xx->2xx|$.RotationRules.ScheduleExpression
Issue-Commit: 34735640b265050797646a7031263995ba2fdc83

# secretsmanager: semantic value validation is absent, so schema-legal but invalid rotation tokens, replica regions and schedule expressions are accepted and mutate state

> _Note: this issue was identified against the code base at commit `34735640b265050797646a7031263995ba2fdc83`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RotateSecret

## Description
7 net-new causes sharing ONE fault: the emulator enforces only the Smithy schema traits (@length, @pattern, @range) on these members and performs NO semantic validation of the value itself. A value that satisfies the regex/length but is not a legal rotation token, AWS Region or Secrets Manager schedule expression is therefore accepted with HTTP 200 and written to state, where AWS refuses it with 400 InvalidParameterException and leaves state untouched. Four of the causes are the read-side consequences: DescribeSecret then reports rotation/version state that the refused write created, or omits a derived field.

Cause 1 of 7: RotateSecret accepts a cron() expression that sets BOTH day-of-month and day-of-week.
AWS: Smithy RotationRulesType.ScheduleExpression -> ScheduleExpressionType @length{1,256} @pattern "^[0-9A-Za-z\\(\\)#\\?\\*\\-\\/, ]+$"; the member doc defines the legal forms as cron() and rate() schedule expressions. A cron() expression in which neither the day-of-month nor the day-of-week field is "?" is not a legal Secrets Manager schedule, and RotateSecret models InvalidParameterException at httpError 400. Recorded refusal: HTTP 400, $.__type "InvalidParameterException", $.Message "Invalid schedule expression." — entirely fixed text, no variable part. The secret's rotation configuration is NOT changed.
Emulator: HTTP 200 with neither $.__type nor $.Message; the schedule is accepted.
Why: a client that mis-writes a cron schedule believes rotation is configured on a cadence that AWS would never have accepted; the error never surfaces at configuration time, which is the only time the client can react.
Fix: RotateSecret's RotationRules validation — parse and validate the cron()/rate() expression's semantics before mutating state.
Traces: secretsmanager-rotate-secret-cron-both-day-fields#task#5 (cluster also covers secretsmanager-rotation-schedule-expression-grammar-rejected#task#9).

Cause 2 of 7: because of cause 1, DescribeSecret reports rotation as configured.
AWS: since RotateSecret refused, the follow-up DescribeSecret carries NO $.RotationEnabled, NO $.RotationLambdaARN and NO $.NextRotationDate (Smithy DescribeSecretResponse models RotationEnabled -> RotationEnabledType with no default, RotationLambdaARN and NextRotationDate, all absent when rotation was never configured).
Emulator: all three are present — RotationEnabled true, RotationLambdaARN set to the rotation function, and NextRotationDate populated.
Why: the read-side half of cause 1. A client polling DescribeSecret to confirm whether rotation took effect gets the opposite answer from AWS, and any assertion of the form "rotation must still be off after a refused RotateSecret" inverts.
Fix: follows from cause 1 — RotateSecret must validate before it writes rotation state.
Traces: secretsmanager-rotate-secret-cron-both-day-fields#task#6.

Cause 3 of 7: PutSecretValue accepts a malformed RotationToken.
AWS: Smithy PutSecretValue.RotationToken -> RotationTokenType @length{36,256} @pattern "^[a-zA-Z0-9\\-]+$" @sensitive; PutSecretValue models InvalidParameterException at 400. A token that satisfies length and character class but is not a well-formed rotation token is refused: HTTP 400, $.__type "InvalidParameterException", $.Message "Invalid rotation token value format." — fixed text, no variable part. No new secret version is minted.
Emulator: HTTP 200 with no $.__type / $.Message; the put succeeds and a version is minted.
Why: the rotation token is how a rotation function proves it owns the in-flight rotation. Accepting a malformed one lets a caller write a version AWS would have refused, so the emulator cannot be used to test the rotation-token guard at all.
Fix: PutSecretValue's input validation — validate the RotationToken's format beyond the schema traits, before minting a version.
Traces: secretsmanager-put-secret-value-rotation-token-length-bounds#task#4.

Cause 4 of 7: because of cause 3, DescribeSecret's VersionIdsToStages carries an extra version and the staging labels are shifted.
AWS: Smithy DescribeSecretResponse.VersionIdsToStages -> SecretVersionsToStagesMapType. With the malformed-token put refused, AWS keeps the version that already held AWSCURRENT holding AWSCURRENT, and the version it displaced earlier holding AWSPREVIOUS; no version exists for the refused call.
Emulator: the version minted by the refused put holds AWSCURRENT, the version that should hold AWSCURRENT is demoted to AWSPREVIOUS, and the version that should hold AWSPREVIOUS is absent from the map entirely.
Why: AWSCURRENT is the label GetSecretValue resolves by default, so the emulator serves a different secret value than AWS would, and the AWSPREVIOUS rollback target is lost. This is the state-corruption consequence of cause 3.
Fix: follows from cause 3; also the version-staging logic must only re-label versions for an accepted put.
Traces: secretsmanager-put-secret-value-rotation-token-length-bounds#task#6.

Cause 5 of 7: RemoveRegionsFromReplication accepts a region code that is not an AWS Region.
AWS: Smithy RemoveRegionsFromReplication.RemoveReplicaRegions -> RemoveReplicaRegionListType @required @length{min:1}, member RegionType @length{1,128} @pattern "^([a-z]+-)+\\d+$"; the operation models InvalidParameterException at 400. A value that MATCHES that pattern but names no real AWS Region is refused: HTTP 400, $.__type "InvalidParameterException", $.Message "Invalid replica region." — fixed text, no variable part.
Emulator: HTTP 200 with no $.__type / $.Message.
Why: the replication-removal call silently "succeeds" against a region that does not exist, so a client with a typo'd region believes a replica was removed and never reconciles.
Fix: RemoveRegionsFromReplication's validation — check each region against the known AWS Region set, not just RegionType's pattern.
Traces: secretsmanager-remove-regions-from-replication-list-and-region-length-limits#task#5.

Cause 6 of 7: DescribeSecret omits the RotationRules.AutomaticallyAfterDays derived from the schedule expression.
AWS: Smithy RotationRulesType.AutomaticallyAfterDays -> AutomaticallyRotateAfterDaysType @range{1,1000}, and its own doc states: "In DescribeSecret and ListSecrets, this value is calculated from the rotation schedule after every successful rotation." So once a rotation schedule is set via ScheduleExpression, DescribeSecret must report AutomaticallyAfterDays derived from that schedule's interval in days (the value is derived from the schedule, not echoed from the request).
Emulator: $.RotationRules.AutomaticallyAfterDays is absent.
Why: a client that reads AutomaticallyAfterDays to check its rotation cadence against a compliance policy — the use the model's own doc names — gets null and cannot evaluate the secret.
Fix: DescribeSecret's RotationRules projection must compute AutomaticallyAfterDays from the stored ScheduleExpression.
Traces: secretsmanager-rotation-schedule-expression-grammar-rejected#task#8. Related open issue 20261112T222923Z-cloudformation-iam-lambda-secretsmanager-2-net-new-causes-104be0 reports the same omission for a rate() schedule reached through CloudFormation; it does not carry this replay key, so this cause is filed here.

Cause 7 of 7: DescribeSecret serves a STALE RotationRules.ScheduleExpression after a later accepted RotateSecret, and still omits AutomaticallyAfterDays.
AWS: DescribeSecret reports the ScheduleExpression of the most recently ACCEPTED RotateSecret call, together with the derived AutomaticallyAfterDays.
Emulator: $.RotationRules.ScheduleExpression still reports the earlier schedule rather than the last accepted one, and AutomaticallyAfterDays is again absent.
Why: a client that reconfigures a rotation schedule and then reads it back to confirm sees the previous schedule, so the reconfiguration looks like a no-op (and, combined with cause 1, a REFUSED schedule and an ACCEPTED one are indistinguishable on read).
Fix: RotateSecret's state write must replace the stored RotationRules, and DescribeSecret must project the stored value plus the derived AutomaticallyAfterDays (cause 6).
Traces: secretsmanager-rotation-schedule-expression-grammar-rejected#task#10.

No open issue carries these replay keys. The open RotateSecret items (20261028T061500Z-secretsmanager-rotatesecret-rotateimmediately-true-never-runs-the-rotation, 20261117T115022Z-iam-lambda-logs-secretsmanager-2-causes-rotatesecret-never, 20261027T052624Z-dynamodb-lambda-s3-secretsmanager-secretsmanager-rotatesecret-never-drives-a-rotation-so-awspending) are about rotation never EXECUTING; these causes are about input validation and the DescribeSecret projection, a different component.

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator up; 45/484 steps mismatched). Verbatim from the divergence clusters:

Cause 1 — RotateSecret, expected 400 got 200:
  status: expected 400, got 200
  $.__type: missing (expected 'InvalidParameterException', got None)
  $.Message: missing (expected 'Invalid schedule expression.', got None)

Cause 2 — DescribeSecret, 200 -> 200:
  $.RotationEnabled: unexpected_present (expected None, got True)
  $.RotationLambdaARN: unexpected_present (expected None, got a rotation function ARN)
  $.NextRotationDate: unexpected_present (expected None, got a timestamp)

Cause 3 — PutSecretValue, expected 400 got 200:
  status: expected 400, got 200
  $.__type: missing (expected 'InvalidParameterException', got None)
  $.Message: missing (expected 'Invalid rotation token value format.', got None)

Cause 4 — DescribeSecret, 200 -> 200:
  $.VersionIdsToStages.<version-that-should-hold-AWSPREVIOUS>: missing (expected ['AWSPREVIOUS'], got None)
  $.VersionIdsToStages.<version-that-should-hold-AWSCURRENT>[0]: expected 'AWSCURRENT', got 'AWSPREVIOUS'
  $.VersionIdsToStages.<version minted by the refused put>: extra (expected None, got ['AWSCURRENT'])

Cause 5 — RemoveRegionsFromReplication, expected 400 got 200:
  status: expected 400, got 200
  $.__type: missing (expected 'InvalidParameterException', got None)
  $.Message: missing (expected 'Invalid replica region.', got None)

Cause 6 — DescribeSecret, 200 -> 200:
  $.RotationRules.AutomaticallyAfterDays: missing (expected a shaped value, got None)

Cause 7 — DescribeSecret, 200 -> 200:
  $.RotationRules.AutomaticallyAfterDays: missing (expected a shaped value, got None)
  $.RotationRules.ScheduleExpression: expected 'cron(0 18 * * ? *)' (the last accepted schedule), got 'cron(0 * * * ? *)' (the earlier one)

Model citations (smithy_query, services/service-com-amazonaws-secretsmanager-secretsmanager/smithy-model/com-amazonaws-secretsmanager-secretsmanager.json): RotationRulesType.ScheduleExpression -> ScheduleExpressionType @length{1,256} @pattern ^[0-9A-Za-z\(\)#\?\*\-\/, ]+$ with cron()/rate() semantics documented on the member; RotationRulesType.AutomaticallyAfterDays -> AutomaticallyRotateAfterDaysType @range{1,1000}, doc: "In DescribeSecret and ListSecrets, this value is calculated from the rotation schedule after every successful rotation"; PutSecretValue.RotationToken -> RotationTokenType @length{36,256} @pattern ^[a-zA-Z0-9\-]+$ @sensitive; RemoveReplicaRegionListType member RegionType @length{1,128} @pattern ^([a-z]+-)+\d+$; RotateSecret, PutSecretValue and RemoveRegionsFromReplication all model InvalidParameterException (client, httpError 400); DescribeSecretResponse models RotationEnabled, RotationLambdaARN, NextRotationDate, RotationRules and VersionIdsToStages -> SecretVersionsToStagesMapType.

## Steps to Reproduce
1. git clone the repo; boot the emulator built from that checkout.
2. Replay the simparity-wire-aws-v1 corpus.
3. Cause 1: secretsmanager-rotate-secret-cron-both-day-fields#task#5 — RotateSecret with RotationRules.ScheduleExpression a cron() in which neither day-of-month nor day-of-week is "?"; expect 400 / InvalidParameterException / "Invalid schedule expression.".
4. Cause 2: then task#6 of the same fixture — DescribeSecret must report no RotationEnabled, no RotationLambdaARN and no NextRotationDate.
5. Cause 3: secretsmanager-put-secret-value-rotation-token-length-bounds#task#4 — PutSecretValue with a RotationToken that satisfies @length{36,256}/@pattern but is not a valid rotation token; expect 400 / InvalidParameterException / "Invalid rotation token value format.".
6. Cause 4: then task#6 of the same fixture — DescribeSecret's VersionIdsToStages must show no version for the refused put, AWSCURRENT unchanged, and the displaced version still at AWSPREVIOUS.
7. Cause 5: secretsmanager-remove-regions-from-replication-list-and-region-length-limits#task#5 — RemoveRegionsFromReplication naming a pattern-legal but non-existent region; expect 400 / InvalidParameterException / "Invalid replica region.".
8. Causes 6 and 7: secretsmanager-rotation-schedule-expression-grammar-rejected#task#8 and #task#10 — after setting a rotation schedule, DescribeSecret must report RotationRules.AutomaticallyAfterDays derived from that schedule, and ScheduleExpression must be the LAST accepted schedule.
