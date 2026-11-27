Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iam__secretsmanager__sts.batch-get-secret-value-reports-the-session-tag-mismatched-secret-as-an-error-entry iam__secretsmanager__sts.create-secret-tagged-with-another-session-tag-value-rejected iam__secretsmanager__sts.session-tag-abac-read-denied-on-a-secret-tagged-for-another-team iam__secretsmanager__sts.session-tag-decides-which-secret-an-assumed-role-reads-denied
Issue-Key: iam__secretsmanager__sts.batch-get-secret-value-reports-the-session-tag-mismatched-secret-as-an-error-entry|_batch_once_granted#15|Timeout|time.sleep(min(2 ** attempt, 8))
Issue-Key: secrets-manager|CreateSecret|AccessDeniedException
Issue-Key: iam__secretsmanager__sts.session-tag-abac-read-denied-on-a-secret-tagged-for-another-team|_read_when_granted#12|Timeout|time.sleep(min(2 ** attempt, 10)) # IAM propagation exposes nothing to poll
Issue-Key: secrets-manager|GetSecretValue|AccessDeniedException
Issue-Commit: 7c686d71091806786cdd6101a8e36a3d7eadfcb8

# secretsmanager authorization ignores the session's principal tags — session-tag ABAC policies (aws:PrincipalTag, and aws:ResourceTag compared to ${aws:PrincipalTag/...}) deny every call they should allow

> _Note: this issue was identified against the code base at commit `7c686d71091806786cdd6101a8e36a3d7eadfcb8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetSecretValue

## Description
Cause 1 of 1: the authorization decision for secretsmanager operations does not see the principal tags an AssumeRole session carries, so any statement whose Condition references `aws:PrincipalTag/<key>` — directly, or as the policy variable `${aws:PrincipalTag/<key>}` on the right-hand side of an `aws:ResourceTag/<key>` comparison — never matches and the call is refused.

AWS: recorded behavior only for the condition evaluation (the secretsmanager model declares no authorization error on these operations; the refusal is IAM's standard AccessDeniedException). A role assumed with `Tags=[{Key:<k>,Value:<v>}]` (trust policy allowing `sts:AssumeRole` and `sts:TagSession`) carries `aws:PrincipalTag/<k>` = <v> for the life of the session. Therefore:
  - a statement allowing `secretsmanager:GetSecretValue` on `*` under `StringEquals {"aws:ResourceTag/<k>": "${aws:PrincipalTag/<k>}"}` SERVES the secret tagged <k>=<v> and REFUSES one tagged <k>=<other> — with AccessDeniedException whose message is "User: " + the session ARN + " is not authorized to perform: secretsmanager:GetSecretValue on resource: " + the secret ARN + " because no identity-based policy allows the secretsmanager:GetSecretValue action";
  - BatchGetSecretValue over both secrets answers 200, returning the matching secret's value in `SecretValues` and the non-matching one as a per-secret entry in `Errors` with ErrorCode `AccessDeniedException` and that same message — the batch is not refused as a whole;
  - `secretsmanager:CreateSecret` conditioned on the session's tag CREATES the secret tagged with the session's own tag value and refuses one tagged for another value.
In every message the session ARN, the secret ARN/name and the tag values come from the request and from state; the sentence around them is fixed text.
Emulator: every one of those calls is refused. CreateSecret for the session's OWN tag value answers AccessDeniedException 'User: arn:aws:sts::<account>:assumed-role/<role>/<session> is not authorized to perform: secretsmanager:CreateSecret on resource: <secret name> because no identity-based policy allows the secretsmanager:CreateSecret action', and GetSecretValue for the secret whose tag MATCHES the session's tag is refused with the equivalent message for secretsmanager:GetSecretValue. The two batch/read checks never reach an assertion at all: their bounded retry loops keep re-issuing the call while it answers AccessDeniedException and are cut off at the 60 s budget ('Check timed out after 60.0s'), which is the same refusal surfaced as a timeout.
Why: tag-based access control through session tags — the documented way to let one role serve many tenants — is unusable against Secrets Manager. The allowed path is refused, so a client is told its policy denies a secret it owns; the refusal is also indistinguishable from the genuine cross-tenant denial the policy exists to produce, so the negative class proves nothing. Clients that retry on AccessDenied while a session settles never exit the loop.
Fix: the authorization step the secretsmanager operations share — populate the request context with the calling session's principal tags and resolve `${aws:PrincipalTag/<key>}` policy variables before comparing condition keys (including `aws:ResourceTag/<key>` against the secret's tags); BatchGetSecretValue must additionally report a per-secret denial as an `Errors` entry rather than failing the whole batch.
Traces: iam__secretsmanager__sts.session-tag-decides-which-secret-an-assumed-role-reads-denied, iam__secretsmanager__sts.create-secret-tagged-with-another-session-tag-value-rejected, iam__secretsmanager__sts.batch-get-secret-value-reports-the-session-tag-mismatched-secret-as-an-error-entry, iam__secretsmanager__sts.session-tag-abac-read-denied-on-a-secret-tagged-for-another-team.

## Evidence
Expected (AWS): the matching secret is served (and, for the batch, returned in SecretValues while the other appears in Errors with ErrorCode AccessDeniedException); only the NON-matching secret is refused.

Actual (emulator), verbatim from the replay:
- 'botocore.exceptions.ClientError: An error occurred (AccessDeniedException) when calling the GetSecretValue operation: User: arn:aws:sts::<account>:assumed-role/<role>/east-reader is not authorized to perform: secretsmanager:GetSecretValue on resource: arn:aws:secretsmanager:<region>:<account>:secret:<name>-XXXXXX because no identity-based policy allows the secretsmanager:GetSecretValue action' — raised for the secret whose tag MATCHES the session tag.
- 'botocore.exceptions.ClientError: An error occurred (AccessDeniedException) when calling the CreateSecret operation: User: arn:aws:sts::<account>:assumed-role/<role>/unit-writer is not authorized to perform: secretsmanager:CreateSecret on resource: <secret name> because no identity-based policy allows the secretsmanager:CreateSecret action' — raised for the positive control, a secret tagged with the session's OWN tag value.
- 'Error: Check timed out after 60.0s' for the two remaining checks, whose retry loops only exit on a served answer.

Account ids, role/session names, secret names and tag values are per-run harness values; the requirement is that the matching-tag call is SERVED and only the mismatching one carries the AccessDeniedException above.

## Steps to Reproduce
1. Create a role whose trust policy allows sts:AssumeRole and sts:TagSession for the account root, and attach an inline policy: Allow secretsmanager:BatchGetSecretValue on '*', plus Allow secretsmanager:GetSecretValue on '*' with Condition StringEquals {"aws:ResourceTag/squad": "${aws:PrincipalTag/squad}"}.
2. Create two secrets, tagged squad=red and squad=blue.
3. AssumeRole with Tags=[{Key:squad,Value:red}] and, as that session, call GetSecretValue on the red secret — expect it served; observe AccessDeniedException.
4. Call BatchGetSecretValue over both ARNs — expect 200 with the red secret in SecretValues and the blue one in Errors (ErrorCode AccessDeniedException); observe the call refused so the batch never returns.
5. Repeat with a CreateSecret policy conditioned on the session tag: creating a secret tagged with the session's own value must succeed; observe AccessDeniedException.
