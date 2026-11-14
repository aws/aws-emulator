Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: secretsmanager.deep-secrets-and-params-force-deleted-secret-old-arn-and-version-ids-rejected-after-name-reuse secretsmanager.list-secrets-invalid-filter-key-value-and-page-size-rejected secretsmanager.rotate-secret-duration-without-a-schedule-or-frequency-rejected
Issue-Shape-Id: com.amazonaws.secretsmanager#secretsmanager
Issue-Key: secretsmanager.deep-secrets-and-params-force-deleted-secret-old-arn-and-version-ids-rejected-after-name-reuse|run#19|AssertionError|assert new_arn != old_arn, (
Issue-Key: secretsmanager.list-secrets-invalid-filter-key-value-and-page-size-rejected|run#13|AssertionError|assert message == (
Issue-Key: secretsmanager.rotate-secret-duration-without-a-schedule-or-frequency-rejected|run#10|AssertionError|assert message == _DURATION_MESSAGE, f"unexpected refusal message: {message!r}"
Issue-Commit: 2526e2ab83e8e410d7ed9e76e35f7ef152f7b443

# secretsmanager: 3 net-new causes — a force-deleted secret re-created under the same name keeps the OLD ARN, the ListSecrets filter-value pattern refusal is mis-pathed/mis-worded, and the Duration-without-schedule RotateSecret refusal is emulator-worded

> _Note: this issue was identified against the code base at commit `2526e2ab83e8e410d7ed9e76e35f7ef152f7b443`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateSecret

## Description
Three independent Secrets Manager defects found by the behavioral replay. All three are in the secretsmanager service; none is covered by an open issue (the nearest ones are named below and are about different causes).

Cause 1 of 3: a secret re-created under the name of a force-deleted secret is given the OLD secret's ARN.
AWS: Smithy models CreateSecret output ARN as SecretARNType (string, length 20-2048); the model does not state the ARN format, so this is recorded behavior only. AWS mints a per-secret random 6-character suffix on the ARN (arn:aws:secretsmanager:<region>:<account>:secret:<name>-<6 random chars>), so a NEW secret created after ForceDeleteWithoutRecovery of a same-named secret gets a DIFFERENT ARN, and the old ARN names nothing: GetSecretValue(old ARN), DescribeSecret(old ARN) and RestoreSecret(old ARN) are then refused 400 ResourceNotFoundException with message "Secrets Manager can't find the specified secret." (fixed text, no variable part), and GetSecretValue(name, VersionId=<an old version id>) is refused 400 ResourceNotFoundException with "Secrets Manager can't find the specified secret value for VersionId: <VersionId>" — fixed text up to the colon, the VersionId taken verbatim from the request.
Emulator: after delete_secret(RecoveryWindowInDays=7) followed by delete_secret(ForceDeleteWithoutRecovery=true), CreateSecret with the same Name returns the SAME ARN the destroyed secret had (the replay asserts new ARN != old ARN and gets the old one back, e.g. ...:secret:<name>-<suffix> identical to the first create). The suffix is evidently derived from, or cached against, the secret name rather than minted fresh per secret.
Why: every handle to the destroyed secret silently resolves to the new one. A client that still holds the old ARN (in a stack template, an IAM policy resource, a cached config) reads the NEW material instead of being told ResourceNotFoundException, and RestoreSecret on a destroyed ARN can appear to work. Identity reuse also breaks IAM scoping, because a policy written for the destroyed secret's ARN now grants access to a different secret.
Fix: the CreateSecret handler's ARN minting and the secret-name/ARN index in the service state — mint a fresh random suffix per secret and retire the destroyed secret's ARN so it resolves to nothing.
Traces: secretsmanager.deep-secrets-and-params-force-deleted-secret-old-arn-and-version-ids-rejected-after-name-reuse.

Cause 2 of 3: the ListSecrets filter-value pattern refusal names the wrong request path, omits the list form of the value, and drops the composite constraint list.
AWS: Smithy: ListSecrets.Filters -> FiltersListType (0-10) of Filter; Filter.Values -> FilterValuesStringList (1-10) of FilterValueStringType with @length min 0 max 512 and @pattern ^\!?[a-zA-Z0-9 :_@\/\+\=\.\-\!]*$ ; error ValidationException (serialization/validation envelope). For a value containing '*', AWS refuses with 400 ValidationException and the message "1 validation error detected: Value '[<value>]' at 'filters.1.member.values' failed to satisfy constraint: Member must satisfy constraint: [Member must have length less than or equal to 512, Member must have length greater than or equal to 0, Member must satisfy regular expression pattern: ^\!?[a-zA-Z0-9 :_@\/\+\=\.\-\!]*$]". Fixed text: "1 validation error detected: Value ", " at ", " failed to satisfy constraint: Member must satisfy constraint: [...]" and the three constraint clauses with the modeled bounds (512, 0) and the modeled anchored pattern; variable parts: the offending value(s), rendered as a bracketed list because the failing member is the LIST, and the 1-based index in the 'filters.<n>.member.values' path.
Emulator: answers ValidationException (code is right) with "1 validation error detected: Value 'sim2real-behav-lsbad-e34e72a44c02*' at 'filters.1.member.values.1.member' failed to satisfy constraint: Member must satisfy regular expression pattern: \!?[a-zA-Z0-9 :_@\/\+\=\.\-\!]*" — it reports the element path ('...values.1.member') instead of the list path ('...values'), the bare value instead of the '[value]' list rendering, only the regex clause instead of the composite "Member must satisfy constraint: [length<=512, length>=0, pattern]" list, and an UNANCHORED pattern (missing the leading ^ and trailing $ of the modeled pattern). The sibling refusals in the same check (unknown filter key enum message, MaxResults=101, unknown SortBy) match AWS.
Why: clients and test suites that assert on, parse or display AWS's validation text cannot match the refusal; the unanchored pattern also misstates the constraint being enforced, so a user who corrects their input against the quoted pattern can still be refused.
Fix: the ListSecrets input validation and the constraint-violation message serializer — report the failing LIST member path, render the value in list form, and emit the full composite constraint list with the modeled anchored pattern.
Traces: secretsmanager.list-secrets-invalid-filter-key-value-and-page-size-rejected. (Open issue 20261105T153227Z-secretsmanager-listsecrets-description-all-filter-matching-never-converges.md is about `description`/`all` filter MATCHING, not this validation message, and does not cover it.)

Cause 3 of 3: RotateSecret's "Duration with no schedule" refusal message is emulator-authored.
AWS: Smithy: RotateSecret.RotationRules -> RotationRulesType with Duration (@pattern ^[0-9]+h$, documented as "The window starts according to the ScheduleExpression") plus ScheduleExpression and AutomaticallyAfterDays; error InvalidParameterException (400). RotateSecret with RotationRules.Duration and neither ScheduleExpression nor AutomaticallyAfterDays is refused 400 InvalidParameterException with the fixed message "You must provide a schedule expression or rotation frequency when using the duration option." (no variable part). This cross-field check runs BEFORE the missing-rotation-Lambda check, which answers 400 InvalidRequestException "No Lambda rotation function ARN is associated with this secret."
Emulator: answers InvalidParameterException (code is right) with "You can't specify a rotation Duration without a rotation schedule." — emulator-authored wording for this error class.
Why: a client that branches on the message (or a test asserting AWS's text) cannot distinguish this rule from the other RotationRules rejections, and the emulator's wording names only "a rotation schedule" even though AutomaticallyAfterDays (a rotation frequency) also satisfies the rule, so it misdirects the fix.
Fix: the RotateSecret handler's RotationRules cross-field validation message text.
Traces: secretsmanager.rotate-secret-duration-without-a-schedule-or-frequency-rejected. (Open issues 20261113T030808Z-cloudformation-secretsmanager-2-net-new-causes-the-conflicting.md — conflicting ScheduleExpression+AutomaticallyAfterDays wording — and 20261029T224105Z-secretsmanager-secrets-manager-refusal-identity-and-wording-diverge-on.md cover different RotateSecret refusals and do not cover this one.)

## Evidence
Cause 1 — check secretsmanager.deep-secrets-and-params-force-deleted-secret-old-arn-and-version-ids-rejected-after-name-reuse:
  AssertionError: a secret re-created after a force delete must get a NEW ARN, got the old one 'arn:aws:secretsmanager:us-west-2:000000000000:secret:sim2real-deep-sm-d3e125c3f051-870HyC'
  Expected: CreateSecret(Name=<reused name>) returns an ARN whose random suffix differs from the destroyed secret's; GetSecretValue/DescribeSecret/RestoreSecret on the old ARN -> 400 ResourceNotFoundException "Secrets Manager can't find the specified secret."; GetSecretValue(SecretId=<name>, VersionId=<old version>) -> 400 ResourceNotFoundException "Secrets Manager can't find the specified secret value for VersionId: <VersionId>".
  Actual: the re-created secret is handed the destroyed secret's ARN verbatim (same -<suffix>), so the old ARN is a live handle on the new secret. (The ARN/account/region/suffix values above are from one run and are not part of the requirement; the requirement is suffix freshness and retirement of the old ARN.)

Cause 2 — check secretsmanager.list-secrets-invalid-filter-key-value-and-page-size-rejected, call ListSecrets(Filters=[{Key: 'name', Values: ['<name>*']}]):
  Expected 400 ValidationException, Message: "1 validation error detected: Value '[<name>*]' at 'filters.1.member.values' failed to satisfy constraint: Member must satisfy constraint: [Member must have length less than or equal to 512, Member must have length greater than or equal to 0, Member must satisfy regular expression pattern: ^\!?[a-zA-Z0-9 :_@\/\+\=\.\-\!]*$]"
  Actual 400 ValidationException, Message: "1 validation error detected: Value 'sim2real-behav-lsbad-e34e72a44c02*' at 'filters.1.member.values.1.member' failed to satisfy constraint: Member must satisfy regular expression pattern: \!?[a-zA-Z0-9 :_@\/\+\=\.\-\!]*"
  (the secret name is a per-run value substituted into the template; the path, the '[...]' rendering, the three constraint clauses and the anchored pattern are the fixed parts.)

Cause 3 — check secretsmanager.rotate-secret-duration-without-a-schedule-or-frequency-rejected, call RotateSecret(SecretId=<arn>, RotationRules={'Duration': '2h'}):
  Expected 400 InvalidParameterException, Message: "You must provide a schedule expression or rotation frequency when using the duration option."
  Actual 400 InvalidParameterException, Message: "You can't specify a rotation Duration without a rotation schedule."
  (The control in the same check — RotationRules={'Duration':'2h','AutomaticallyAfterDays':30} -> 400 InvalidRequestException "No Lambda rotation function ARN is associated with this secret." — passed, so only the cross-field message text diverges.)

## Steps to Reproduce
Cause 1:
1. CreateSecret(Name=N, SecretString="original-material") -> note ARN_old and VersionId v1; PutSecretValue(SecretId=ARN_old, SecretString="second-material") -> v2.
2. DeleteSecret(SecretId=ARN_old, RecoveryWindowInDays=7); confirm CreateSecret(Name=N) is refused InvalidRequestException (name reserved).
3. DeleteSecret(SecretId=ARN_old, ForceDeleteWithoutRecovery=true).
4. CreateSecret(Name=N, SecretString="reborn-material") (retry while the name is still reserved) -> observe the returned ARN equals ARN_old instead of a freshly suffixed ARN.
5. GetSecretValue/DescribeSecret/RestoreSecret(SecretId=ARN_old) and GetSecretValue(SecretId=N, VersionId=v1|v2) -> expect ResourceNotFoundException; observe the old ARN still resolves.

Cause 2:
1. CreateSecret(Name=N, SecretString="x").
2. ListSecrets(Filters=[{"Key":"name","Values":[N + "*"]}]) -> compare Error.Message with AWS's composite-constraint text above.

Cause 3:
1. CreateSecret(Name=N, SecretString="v1") with no RotationLambdaARN.
2. RotateSecret(SecretId=<ARN>, RotationRules={"Duration":"2h"}) -> compare Error.Message with "You must provide a schedule expression or rotation frequency when using the duration option."
3. RotateSecret(SecretId=<ARN>, RotationRules={"Duration":"2h","AutomaticallyAfterDays":30}) -> still InvalidRequestException "No Lambda rotation function ARN is associated with this secret." (control).
