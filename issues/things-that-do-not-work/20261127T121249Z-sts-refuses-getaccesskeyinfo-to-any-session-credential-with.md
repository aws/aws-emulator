Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sts.get-federation-token-session-calls-to-sts-operations sts.get-session-token-session-calls-to-iam-and-sts-operations
Issue-Shape-Id: com.amazonaws.sts#AWSSecurityTokenServiceV20110615
Issue-Key: sts.get-federation-token-session-calls-to-sts-operations|run#35|AssertionError|assert not wrong, "; ".join(wrong).replace(account, "<account>")
Issue-Key: sts.get-session-token-session-calls-to-iam-and-sts-operations|run#37|AssertionError|assert not wrong, "; ".join(wrong).replace(account, "<account>")
Issue-Commit: 7c686d71091806786cdd6101a8e36a3d7eadfcb8

# sts refuses GetAccessKeyInfo to any session credential with 'Cannot call GetAccessKeyInfo with session credentials', and gives a federated AssumeRole the same blanket sentence instead of 'Roles may not be assumed by federated users' (2 causes)

> _Note: this issue was identified against the code base at commit `7c686d71091806786cdd6101a8e36a3d7eadfcb8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetAccessKeyInfo

## Description
Two net-new causes in the guard sts applies to callers presenting temporary (session) credentials.

Cause 1 of 2: GetAccessKeyInfo is refused for session credentials, for both a GetFederationToken session and an assumed-role session.
AWS: Smithy: sts#GetAccessKeyInfo takes AccessKeyId (@required, @length 16-128, pattern ^[\w]*$), returns Account, and declares NO errors — it is not one of the operations that requires long-term credentials. Recorded behavior: GetAccessKeyInfo is SERVED to a federated session and to an assumed-role session (as are GetCallerIdentity and DecodeAuthorizationMessage); only GetSessionToken (and, for a federated caller, GetFederationToken) are refused with the long-term-credential message, whose fixed template is "Cannot call " + the operation name + " with session credentials" at code `AccessDenied`.
Emulator: GetAccessKeyInfo is refused with ('AccessDenied', 'Cannot call GetAccessKeyInfo with session credentials') for both session kinds — the operation has been added to the set the session-credential guard refuses.
Why: GetAccessKeyInfo is how a caller resolves which account an access key id belongs to, and temporary credentials are the normal credential for code doing that (a role session, a federated console session). Refusing it leaves that lookup unavailable to every non-static credential, and the refusal is an AccessDenied, so a client reads it as a permissions problem in its own policy — which no policy change can fix.
Fix: the sts front door's session-credential guard — restrict the refused set to the operations that genuinely require long-term credentials and let GetAccessKeyInfo through for session callers.
Traces: sts.get-session-token-session-calls-to-iam-and-sts-operations, sts.get-federation-token-session-calls-to-sts-operations.

Cause 2 of 2: AssumeRole called by a GetFederationToken session is refused with the generic session-credential sentence instead of the federated-user one.
AWS: Smithy: sts#AssumeRole declares ExpiredTokenException, MalformedPolicyDocumentException, PackedPolicyTooLargeException and RegionDisabledException; this refusal is the service's AccessDenied. Recorded behavior: a federated user may not assume a role at all, and the refusal is code `AccessDenied` with the fixed message "Roles may not be assumed by federated users" — a statement about WHO the caller is, with no variable part. (An assumed-role session, by contrast, may call AssumeRole.)
Emulator: ('AccessDenied', 'Cannot call AssumeRole with session credentials') — the right code, but the blanket session-credential sentence, which also wrongly implies that any temporary credential is refused.
Why: the two messages mean different things. AWS's sentence tells the caller that federation is the blocker, so the fix is a different credential source; the emulator's sentence tells it that temporary credentials are the blocker, so a caller holding an assumed-role session concludes AssumeRole is unavailable to it too. Clients and operators match on this fixed text when classifying refusals, so the re-wording also breaks exact-match handling.
Fix: the sts AssumeRole handler's caller-kind check — refuse a federated caller with 'Roles may not be assumed by federated users' rather than routing it through the session-credential guard's message.
Traces: sts.get-federation-token-session-calls-to-sts-operations.

## Evidence
Expected (AWS): ('federated', 'GetAccessKeyInfo') -> served; ('assumed-role', 'GetAccessKeyInfo') -> served; ('federated', 'AssumeRole') -> ('AccessDenied', 'Roles may not be assumed by federated users'). Only GetSessionToken (and GetFederationToken for a federated caller) carry ('AccessDenied', 'Cannot call <op> with session credentials').

Actual (emulator), verbatim from the replay:
- 'AssertionError: (\'federated\', \'AssumeRole\'): got (\'AccessDenied\', \'Cannot call AssumeRole with session credentials\'); (\'federated\', \'GetAccessKeyInfo\'): got (\'AccessDenied\', \'Cannot call GetAccessKeyInfo with session credentials\')'
- 'AssertionError: sts:GetAccessKeyInfo: got (\'AccessDenied\', \'Cannot call GetAccessKeyInfo with session credentials\')' (GetSessionToken session)

Both checks' other legs (GetCallerIdentity, DecodeAuthorizationMessage, the GetSessionToken/GetFederationToken refusals, and AssumeRole from an assumed-role session) matched, so the guard's membership — not its message template — is what diverges in cause 1.

## Steps to Reproduce
1. Create an IAM user with an allow-all inline policy and a long-term access key, plus a role the account root may assume with an allow-all policy.
2. Mint a federated session with GetFederationToken(Name=..., Policy=<allow-all>) and an assumed-role session with AssumeRole, using the user's long-term key.
3. As EACH session call GetAccessKeyInfo(AccessKeyId=<the user's key id>) — expect it served (Account returned); observe ('AccessDenied', 'Cannot call GetAccessKeyInfo with session credentials').
4. As the FEDERATED session call AssumeRole(RoleArn=<role>, RoleSessionName=...) — expect ('AccessDenied', 'Roles may not be assumed by federated users'); observe ('AccessDenied', 'Cannot call AssumeRole with session credentials').
5. As a GetSessionToken session repeat step 3 — expect it served; observe the same refusal.
