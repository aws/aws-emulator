Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: secretsmanager.create-secret-non-partner-type-rejected secretsmanager.create-secret-string-limit-counts-utf8-bytes-rejected secretsmanager.cron-hour-step-from-midnight-places-next-rotation-on-the-eight-hour-grid secretsmanager.cron-last-day-of-month-schedules-the-next-rotation secretsmanager.cron-nth-weekday-schedules-the-next-rotation secretsmanager.replication-changes-advance-last-changed-date
Issue-Shape-Id: com.amazonaws.secretsmanager#secretsmanager
Issue-Key: secretsmanager.create-secret-non-partner-type-rejected|run#7|AssertionError|assert "Type" not in described, (
Issue-Key: secretsmanager.create-secret-string-limit-counts-utf8-bytes-rejected|run#12|AssertionError|raise AssertionError(
Issue-Key: secrets-manager|RotateSecret|InvalidParameterException
Issue-Key: secretsmanager.cron-last-day-of-month-schedules-the-next-rotation|run#8|Timeout|desc = sm.describe_secret(SecretId=secret["ARN"])
Issue-Key: secretsmanager.cron-nth-weekday-schedules-the-next-rotation|run#8|Timeout|desc = sm.describe_secret(SecretId=secret["ARN"])
Issue-Key: secretsmanager.replication-changes-advance-last-changed-date|run#14|AssertionError|assert after_add > created, (
Issue-Commit: c1d149b2bb5e60d0ae77bd59c6a4638e6165c0e6

# secretsmanager: 4 net-new causes — an empty CreateSecret Type is stored and echoed, the SecretString ceiling counts characters not UTF-8 bytes, cron() rotation schedules with /step, L and #n are refused or never produce NextRotationDate, and replication changes do not advance LastChangedDate

> _Note: this issue was identified against the code base at commit `c1d149b2bb5e60d0ae77bd59c6a4638e6165c0e6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateSecret

## Description
4 net-new causes in Secrets Manager's state and limit semantics, on operations the emulator serves. Unlike the refusal-classification issue filed alongside this one, these are not message-wording divergences: two of them let a request through that AWS refuses or stores differently, and two make a modelled output member unusable. A fixer must close all four before submitting.

Related open issue that does NOT carry these checks' keys and does NOT cover them: 20261127T124058Z-secretsmanager-semantic-value-validation-is-absent-so-schema.md (schema-legal but invalid rotation tokens, replica regions and schedule expressions ACCEPTED). Cause 3 below is the opposite direction — a VALID cron() extension refused, and two valid ones accepted but never scheduled — so it is a distinct defect in the same component.

Cause 1 of 4: CreateSecret stores an empty Type and DescribeSecret echoes it.
AWS: Smithy CreateSecret.Type -> MedeaTypeType @length {min:0,max:256}; DescribeSecret output also carries Type -> MedeaTypeType. CreateSecret with Type="" creates an ORDINARY, untyped secret: DescribeSecret for it omits the Type member entirely (an empty Type is not a type), and GetSecretValue serves the SecretString unchanged. The same operation refuses a 256-character Type that names no partner with InvalidParameterException and the fixed message "Invalid type.", and refuses a 257-character Type with ValidationException "1 validation error detected: Value '<first 64 characters of the submitted type> ...' at 'type' failed to satisfy constraint: Member must have length less than or equal to 256"; neither refusal leaves a secret behind, so DescribeSecret on those names answers ResourceNotFoundException "Secrets Manager can't find the specified secret." (Recorded behaviour for the "Invalid type." wording and for the omission of an empty Type; the model only gives the length bound.)
Emulator: the secret is created, but DescribeSecret reports Type: "" — the member is present with an empty string. The check fails at that first assertion, so the two Type refusals downstream are untested here and must be verified by the same replay.
Why: a client that tests presence of Type to decide whether a secret is a partner-managed type (the SDK idiom, since the member is optional) sees every untyped secret as typed, with an empty type name, and branches wrongly.
Fix: CreateSecret's handler/state — treat an empty Type as absent rather than storing it — and DescribeSecret's serializer, which must omit Type when the secret has none.
Traces: secretsmanager.create-secret-non-partner-type-rejected.

Cause 2 of 4: the SecretString size ceiling is counted in characters instead of UTF-8 bytes.
AWS: Smithy CreateSecret.SecretString -> SecretStringType @length {min:1,max:65536} @sensitive; CreateSecret models LimitExceededException (400). The ceiling counts UTF-8 BYTES, not characters: a value of 32769 two-byte characters is 65538 bytes and is refused 400 with error code LimitExceededException and message "The secure secret must be between 1 and 65536 bytes in size." — fixed text apart from the limit, which is the modelled 65536. A value of 32768 two-byte characters (exactly 65536 bytes) is stored and read back byte-identical, and so is a 32769-character value whose UTF-8 encoding is exactly 65536 bytes.
Emulator: the 32769-character / 65538-byte value is ACCEPTED — create_secret returns a secret — so the limit is being applied to the character count.
Why: the limit is silently 2x too permissive for non-ASCII payloads; a client that fits its payload to the emulator stores secrets real AWS refuses, and the failure surfaces only in production. It also means no LimitExceededException is ever raised on this path, so retry/backoff logic keyed on it is never exercised.
Fix: CreateSecret's (and the same check in UpdateSecret/PutSecretValue's) validation of SecretString — measure the UTF-8 encoded byte length, and raise LimitExceededException with that message when it exceeds 65536.
Traces: secretsmanager.create-secret-string-limit-counts-utf8-bytes-rejected.

Cause 3 of 4: RotateSecret's cron() evaluator does not support the EventBridge cron extensions, so /step is refused outright and L / #n never yield a NextRotationDate.
AWS: Smithy RotateSecret.RotationRules -> RotationRulesType.ScheduleExpression -> ScheduleExpressionType @length {1,256} @pattern "^[0-9A-Za-z\\(\\)#\\?\\*\\-\\/, ]+$" — the pattern admits '#', '/', '?' and '*' precisely because Secrets Manager accepts EventBridge-style cron() fields; the member doc says a cron() expression defines the rotation schedule and that Secrets Manager rotates within a rotation window computed from it. DescribeSecret output carries NextRotationDate -> NextRotationDateType (timestamp). AWS therefore (a) ACCEPTS cron(0 /8 ? * SAT *), round-trips it in RotationRules.ScheduleExpression, and reports a NextRotationDate on a Saturday in a one-hour window opening at 00:00, 08:00 or 16:00 UTC; (b) accepts cron(0 17 L * ? *) and reports a NextRotationDate on a month's last day at 23:59:59 UTC; (c) accepts cron(0 0 ? * SUN#1 *) and reports a NextRotationDate on a Sunday within the first seven days of a month. In every case RotationEnabled is true. (Recorded behaviour for the window placement; the model supplies the accepted grammar.)
Emulator: (a) RotateSecret REFUSES cron(0 /8 ? * SAT *) with 400 InvalidParameterException "ScheduleExpression must be a rate() or cron() expression, for example rate(10 days) or cron(0 8 1 * ? *)." — a step value in the hours field is rejected even though the modelled pattern admits '/'. (b) and (c) are accepted and RotationEnabled is true, but DescribeSecret never grows a NextRotationDate: both checks poll for 60 s and time out, so the member is simply never populated for an L or #n schedule.
Why: a client cannot set an every-N-hours rotation at all (the call fails), and for last-day-of-month or nth-weekday schedules it can never learn when the next rotation is due — compliance reporting and any "rotate before X" scheduling built on NextRotationDate have no value to read. The refusal message also misrepresents a valid expression as malformed.
Fix: RotateSecret's RotationRules validation and the next-rotation computation behind DescribeSecret — accept the EventBridge cron() extensions the modelled pattern admits ('/' steps, 'L' last-day, '#n' nth-weekday) and compute NextRotationDate from the parsed expression.
Traces: secretsmanager.cron-hour-step-from-midnight-places-next-rotation-on-the-eight-hour-grid, secretsmanager.cron-last-day-of-month-schedules-the-next-rotation, secretsmanager.cron-nth-weekday-schedules-the-next-rotation.

Cause 4 of 4: adding or removing a replica Region does not advance the primary's LastChangedDate.
AWS: Smithy ReplicateSecretToRegions (SecretId, AddReplicaRegions @required) and RemoveRegionsFromReplication both modify the secret; DescribeSecret output LastChangedDate -> LastChangedDateType is documented as "the last date and time that this secret was modified in any way" (API_DescribeSecret). So each replication change advances LastChangedDate strictly past its previous value.
Emulator: after ReplicateSecretToRegions the primary's LastChangedDate is byte-identical to the value DescribeSecret reported right after CreateSecret — the timestamp is not touched at all (the check sleeps 2 s between the two reads, so this is not clock granularity; only the fact that it did not advance is the finding, never the particular timestamp).
Why: LastChangedDate is the field clients and drift detectors poll to notice that a secret changed; a replication change is invisible to them, so caches and audit trails never refresh and "has anything changed since T?" answers no.
Fix: the ReplicateSecretToRegions and RemoveRegionsFromReplication handlers / the secret's state — stamp LastChangedDate on every replication mutation, as the other mutating operations do.
Traces: secretsmanager.replication-changes-advance-last-changed-date.

## Evidence
Behavioural suite simparity-behavioral-aws-v1, service heading '### secretsmanager' (220/236 passed). Verdicts verbatim from the failing list:

Cause 1 — secretsmanager.create-secret-non-partner-type-rejected [mismatch]
  AssertionError: an empty Type should leave the secret untyped; DescribeSecret reports ''
  (assertion: "Type" not in describe_secret(SecretId=<created ARN>) after create_secret(..., Type=""))

Cause 2 — secretsmanager.create-secret-string-limit-counts-utf8-bytes-rejected [mismatch]
  AssertionError: 32769 characters (65538 UTF-8 bytes) were accepted
  expected refusal: ('LimitExceededException', 'The secure secret must be between 1 and 65536 bytes in size.')
  Positive controls in the same check pass: 32768 two-byte characters (exactly 65536 bytes) store and read back intact, as does a 32769-character value encoding to 65536 bytes.

Cause 3 — secretsmanager.cron-hour-step-from-midnight-places-next-rotation-on-the-eight-hour-grid [error]
  botocore.errorfactory.InvalidParameterException: An error occurred (InvalidParameterException) when calling the RotateSecret operation: ScheduleExpression must be a rate() or cron() expression, for example rate(10 days) or cron(0 8 1 * ? *).
  request: RotateSecret(RotationRules={'ScheduleExpression': 'cron(0 /8 ? * SAT *)'}, RotateImmediately=False)
Cause 3 — secretsmanager.cron-last-day-of-month-schedules-the-next-rotation [mismatch]
  Error: Check timed out after 60.0s — the check polls DescribeSecret every 5 s for up to 60 s after RotateSecret(RotationRules={'ScheduleExpression': 'cron(0 17 L * ? *)'}); RotationEnabled is True throughout but NextRotationDate never appears.
Cause 3 — secretsmanager.cron-nth-weekday-schedules-the-next-rotation [mismatch]
  Error: Check timed out after 60.0s — same loop after RotateSecret(RotationRules={'ScheduleExpression': 'cron(0 0 ? * SUN#1 *)'}).

Cause 4 — secretsmanager.replication-changes-advance-last-changed-date [mismatch]
  AssertionError: adding a replica Region left LastChangedDate at <the same timestamp DescribeSecret reported at create> (created <identical timestamp>) — the two reads are separated by a 2 s sleep and a ReplicateSecretToRegions call that reached InSync.

Model citations (smithy_query on services/service-com-amazonaws-secretsmanager-secretsmanager/smithy-model/com-amazonaws-secretsmanager-secretsmanager.json): CreateSecret.Type -> MedeaTypeType @length{0,256} and DescribeSecret output Type -> MedeaTypeType; CreateSecret.SecretString -> SecretStringType @length{1,65536} @sensitive with LimitExceededException modelled at httpError 400; RotationRulesType.ScheduleExpression -> ScheduleExpressionType @length{1,256} @pattern ^[0-9A-Za-z\(\)#\?\*\-\/, ]+$ (the pattern admits '#', '/' and '?'); DescribeSecret output NextRotationDate -> NextRotationDateType and LastChangedDate -> LastChangedDateType; ReplicateSecretToRegions(SecretId @required, AddReplicaRegions @required @length{min:1}).

## Steps to Reproduce
1. git_clone the repo and boot the emulator built from that checkout.
2. Replay the simparity-behavioral-aws-v1 suite and look under the '### secretsmanager' heading.
3. Cause 1: replay secretsmanager.create-secret-non-partner-type-rejected — CreateSecret with Type="", then DescribeSecret: the Type member must be absent. Then CreateSecret with a 256-character Type (expect InvalidParameterException / 'Invalid type.') and with a 257-character one (expect the ValidationException length message), and confirm neither name is describable afterwards.
4. Cause 2: replay secretsmanager.create-secret-string-limit-counts-utf8-bytes-rejected — CreateSecret with 32769 two-byte characters (65538 UTF-8 bytes, sent as raw UTF-8); expect LimitExceededException / 'The secure secret must be between 1 and 65536 bytes in size.' and no secret left behind. Confirm 32768 two-byte characters (65536 bytes) still store and read back intact.
5. Cause 3: replay secretsmanager.cron-hour-step-from-midnight-places-next-rotation-on-the-eight-hour-grid — RotateSecret with ScheduleExpression 'cron(0 /8 ? * SAT *)' must be ACCEPTED, round-trip in RotationRules, and DescribeSecret must report a NextRotationDate on a Saturday in a one-hour window opening at 00:00, 08:00 or 16:00 UTC.
6. Cause 3: replay secretsmanager.cron-last-day-of-month-schedules-the-next-rotation ('cron(0 17 L * ? *)' -> NextRotationDate on a month's last day at 23:59:59 UTC) and secretsmanager.cron-nth-weekday-schedules-the-next-rotation ('cron(0 0 ? * SUN#1 *)' -> NextRotationDate on a Sunday in the first seven days).
7. Cause 4: replay secretsmanager.replication-changes-advance-last-changed-date — read LastChangedDate, ReplicateSecretToRegions to a second Region, wait for InSync, read again: it must be strictly later; then RemoveRegionsFromReplication and confirm it advances again.
