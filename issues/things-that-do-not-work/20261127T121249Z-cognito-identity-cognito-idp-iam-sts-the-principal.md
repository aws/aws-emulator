Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cognito-identity__cognito-idp__iam__sts.principal-tag-from-changed-user-attribute-denied-until-restored
Issue-Key: cognito-identity__cognito-idp__iam__sts.principal-tag-from-changed-user-attribute-denied-until-restored|_retrying#33|Timeout|time.sleep(min(2 ** attempt, 10))
Issue-Commit: 7c686d71091806786cdd6101a8e36a3d7eadfcb8

# cognito-identity__cognito-idp__iam__sts: the principal-tag-from-a-user-attribute ABAC scenario reaches no verdict — a Cognito-vended session's aws:PrincipalTag condition never becomes satisfiable inside the client budget

> _Note: this issue was identified against the code base at commit `7c686d71091806786cdd6101a8e36a3d7eadfcb8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetCredentialsForIdentity

## Description
Cause 1 of 1: credentials vended by GetCredentialsForIdentity for an identity pool with a principal-tag attribute map never carry the mapped tag in a form the IAM condition evaluator accepts, so the scenario makes no observable progress and is cut off at the 60 s client budget with no verdict.

AWS: recorded behavior only for the end-to-end contract. SetPrincipalTagAttributeMap({<tag>: 'custom:<attr>'}) on an identity pool's provider makes each sign-in's vended session carry `aws:PrincipalTag/<tag>` = that user's current attribute value. A role policy conditioned on `StringEquals {"aws:PrincipalTag/<tag>": "<allowed>"}` then (a) SERVES `cognito-idp:DescribeUserPool` to a session minted while the attribute equals the allowed value, (b) REFUSES it for a session minted after the attribute changed, with AccessDeniedException whose message is "User: " + that session's ARN + " is not authorized to perform: cognito-idp:DescribeUserPool on resource: " + the pool ARN + " because no identity-based policy allows the cognito-idp:DescribeUserPool action", (c) leaves the EARLIER session still able to read (tags are fixed at vend time), and (d) serves it again once the attribute is restored.
Emulator: the check exhausts its whole 60 s budget and reports no assertion at all ('Check timed out after 60.0s'). The only waits it enters are bounded retries (90 s each) on InvalidIdentityPoolConfigurationException from GetCredentialsForIdentity and on AccessDeniedException from the tag-gated DescribeUserPool — both longer than the budget, so a persistently refused or persistently unconfigured leg consumes the run. Hypothesis: the vended session does not present the mapped principal tag, so the tag-gated read stays AccessDenied and the retry never exits.
Why: none of the four legs of attribute-based access control through Cognito is observable. A client cannot confirm that a user attribute governs what the vended credentials may do, cannot see the denial after the attribute changes, and cannot see that credentials vended earlier keep their tag — and because the run produces no verdict rather than a wrong answer, the failure is silent: the scenario looks merely slow.
Fix: the cognito-identity GetCredentialsForIdentity path and the session it mints — the principal-tag attribute map must be resolved from the ID token's claims onto the vended session's principal tags, so IAM's `aws:PrincipalTag/<key>` condition can be evaluated against them; and the first leg must settle (serve or refuse) rather than leaving the caller in a retry.
Traces: cognito-identity__cognito-idp__iam__sts.principal-tag-from-changed-user-attribute-denied-until-restored.

## Evidence
Expected (AWS): the four-leg trajectory above, ending with a restored-attribute session reading the pool again.

Actual (emulator), verbatim from the replay: 'Error: Check timed out after 60.0s' — no assertion of the check ran, so neither the allow leg nor the AccessDeniedException leg produced a value. The check's own bounded retries wrap GetCredentialsForIdentity (retrying InvalidIdentityPoolConfigurationException for up to 90 s) and the tag-gated cognito-idp:DescribeUserPool (retrying AccessDeniedException for up to 90 s); either one, if never satisfied, consumes the whole budget.

Pool ids, user names, role names and the per-run tag values are harness values and are not part of the requirement.

## Steps to Reproduce
1. Create a user pool with a mutable custom attribute, a client with ADMIN_USER_PASSWORD_AUTH, and a user whose custom:<attr> is the allowed value.
2. Create an identity pool bound to that user-pool provider, a role trusted by cognito-identity.amazonaws.com for sts:AssumeRoleWithWebIdentity + sts:TagSession, and a role policy allowing cognito-idp:DescribeUserPool on the pool ARN under StringEquals aws:PrincipalTag/<tag> = <allowed>.
3. SetIdentityPoolRoles(authenticated=<role>) and SetPrincipalTagAttributeMap({<tag>: 'custom:<attr>'}).
4. Sign in, GetId, GetCredentialsForIdentity, and call cognito-idp DescribeUserPool with the vended credentials — expect it served; observe the call never becomes authorized and the run is cut off at 60 s with no verdict.
5. Change the attribute, sign in again, vend again and repeat the read — expect AccessDeniedException naming cognito-idp:DescribeUserPool and the pool ARN; then restore the attribute and expect the read served again.
