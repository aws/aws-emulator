Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cognito-identity.vended-session-outlives-pool-deletion-until-the-role-policy-is-revoked
Issue-Shape-Id: com.amazonaws.cognitoidentity#AWSCognitoIdentityService
Issue-Key: cognito-identity.vended-session-outlives-pool-deletion-until-the-role-policy-is-revoked|run#35|AssertionError|assert code is None, (
Issue-Commit: 4a8024be86a6dad049d0fd9bbb9addeb9359d2ed

# cognito-identity: credentials already vended by GetCredentialsForIdentity stop working the moment DeleteIdentityPool runs — the live S3 session is refused AccessDenied 'Sign the request with the emulator credentials'

> _Note: this issue was identified against the code base at commit `4a8024be86a6dad049d0fd9bbb9addeb9359d2ed`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetCredentialsForIdentity

## Description
One net-new root cause on the cognito-identity credential-vending path. It must be closed before submitting.

Related open issue that does NOT carry this replay key: 20261029T173827Z-cognito-identity-10-net-new-token-credential-refusal.md lists this same check id under its Cause 8 (the post-delete ResourceNotFoundException message naming the pool, asserted earlier in the check at `assert vend_message == expected`). The check now gets PAST that assertion and fails later, in the survival-read loop, on a different behavior; that issue's keys do not cover this frame. Also related but not carrying this key: 20261026T054328Z-cognito-identity-4-net-new-root-causes-a.md and 20261028T221339Z-cognito-identity-3-net-new-getcredentialsforidentity-root-causes.md.

Cause 1 of 1 — the lifetime of credentials vended by GetCredentialsForIdentity is modelled as a lease on the identity pool, so DeleteIdentityPool retroactively invalidates an already-vended session, and the resulting refusal is an emulator-authored message.

AWS: recorded behavior only — the Smithy model types only the artefact, not its lifetime (GetCredentialsForIdentity output member `Credentials` -> `Credentials` with `AccessKeyId`/`SecretKey`/`SessionToken`/`Expiration`; `DeleteIdentityPool` takes only `IdentityPoolId` and returns Unit, and models no effect on previously issued credentials). What GetCredentialsForIdentity hands back is an STS session of the IAM role the pool mapped, valid until its `Expiration`; it is bound to the ROLE, and the only things that stop it are expiry or a change to the role's own trust/identity policy. Nothing in the identity pool's own lifecycle reaches through to a session already issued, so an in-flight request signed with those credentials keeps being authorized exactly as the role's policy allows, and the operation it signs (here s3:GetObject on the one object the role's inline Allow names) keeps returning the object bytes. When that same session IS refused — e.g. after the role's only Allow is removed — the refusal is S3's own: error code `AccessDenied` with the message `User: arn:<partition>:sts::<account>:assumed-role/<role-name>/<session-name> is not authorized to perform: s3:GetObject on resource: "arn:<partition>:s3:::<bucket>/<key>" because no identity-based policy allows the s3:GetObject action` — fixed text, with partition/account/role name/session name/bucket/key taken from the request and the caller's session. Hypothesis (stated as such in the check's own header, which is marked DRAFT/unblessed): that survival-until-expiry is the measured AWS behavior; the lower bound this issue rests on regardless is that an identity-pool delete is not an STS session revocation, and that no AWS refusal message instructs the caller to sign with emulator credentials.

Emulator: the vended session works while the pool exists — the positive control read the exact object bytes through the vended credentials — and then, immediately after `DeleteIdentityPool`, the very same session is refused by S3 on the very same GetObject with error code `AccessDenied` and the message `Sign the request with the emulator credentials`. All three survival reads fail; the first one aborts the check. The error text is emulator-authored (it names 'the emulator credentials'), which indicates the session token stopped resolving at all when the pool record was dropped, and the SigV4/credential-resolution layer fell through to its generic refusal rather than any authorization decision about the role.

Why: a client relying on AWS's behavior loses in-flight work that AWS completes. Deprovisioning code that deletes an identity pool while sessions are still outstanding (tenant teardown, pool rotation, CFN stack delete) sees every live guest/authenticated session break instantly against the emulator, while against AWS those sessions drain normally until `Expiration`. The reverse is just as damaging for test fidelity: a client cannot use the emulator to verify how its app behaves across a pool rotation. On top of that, the refusal is unrecognisable — a caller cannot map `AccessDenied` + 'Sign the request with the emulator credentials' onto any AWS authorization outcome, so credential-expiry/retry logic that branches on the AccessDenied message (or on the distinction between an unrecognised key and a policy denial) takes the wrong branch, and the emulator-specific wording leaks into application-visible error paths.

Fix: the credential/session store shared by GetCredentialsForIdentity and the request-authentication layer — a session minted for an identity must be keyed by the role session (and its Expiration), not by the parent identity-pool record, so DeleteIdentityPool removes the pool and stops NEW vends without deleting sessions already issued; and the authentication layer's fall-through refusal for a session token it cannot resolve must emit S3's own AccessDenied/InvalidAccessKeyId wording rather than an instruction to use emulator credentials.

Traces: cognito-identity.vended-session-outlives-pool-deletion-until-the-role-policy-is-revoked.

## Evidence
Check cognito-identity.vended-session-outlives-pool-deletion-until-the-role-policy-is-revoked, [mismatch]:

AssertionError: read 1 of 3: deleting the identity pool must not revoke an already-vended STS session, but S3 refused it with 'AccessDenied': 'Sign the request with the emulator credentials'

Sequence observed in the replay:
- GetCredentialsForIdentity for a guest identity in a pool whose `unauthenticated` role carries exactly one inline Allow (s3:GetObject on arn:aws:s3:::<bucket>/<key>) returned Credentials: PASSED.
- Positive control: GetObject on that key, signed with the vended credentials, returned the exact payload bytes: PASSED.
- DeleteIdentityPool: 200.
- GetCredentialsForIdentity and DescribeIdentity against the deleted pool: refused ResourceNotFoundException with "IdentityPool '<identity-pool-id>' not found.": PASSED (so the earlier Cause 8 of 20261029T173827Z-cognito-identity-10-net-new-token-credential-refusal.md no longer fires here).
- Survival read 1 of 3, same session, same bucket/key: expected the object bytes (no error code). Got error code 'AccessDenied', message 'Sign the request with the emulator credentials'.

Expected on an AWS-shaped refusal of this GetObject (reached only after the role's own Allow is deleted, the leg the check never got to): code 'AccessDenied', message `User: arn:<partition>:sts::<account>:assumed-role/<role-name>/<session-name> is not authorized to perform: s3:GetObject on resource: "arn:<partition>:s3:::<bucket>/<key>" because no identity-based policy allows the s3:GetObject action` — the quoted sentence is the fixed text; partition, account, role name, session name (CognitoIdentityCredentials for a Cognito-vended session), bucket and key are the variable parts from the request and session. 'Sign the request with the emulator credentials' is not an AWS message for any error class.

## Steps to Reproduce
1. Boot the emulator and replay the behavioral suite; re-replay the single check in trace_ids to confirm a fix.
2. By hand:
   a. s3:CreateBucket <bucket>; PutObject <bucket>/<key> with a known payload.
   b. cognito-identity:CreateIdentityPool with AllowUnauthenticatedIdentities=true.
   c. iam:CreateRole trusting cognito-identity.amazonaws.com for sts:AssumeRoleWithWebIdentity with cognito-identity.amazonaws.com:aud = the pool id and amr = unauthenticated; iam:PutRolePolicy granting only s3:GetObject on arn:aws:s3:::<bucket>/<key>; cognito-identity:SetIdentityPoolRoles {"unauthenticated": <role arn>}.
   d. GetId, then GetCredentialsForIdentity; build an S3 client from the returned AccessKeyId/SecretKey/SessionToken and GetObject <bucket>/<key> — it returns the payload (control).
   e. DeleteIdentityPool on the pool.
   f. Repeat the GetObject with the SAME credentials from step (d), three times.
3. Expected: each GetObject still returns the exact payload bytes (the session is an STS session of the role, unaffected by the pool's deletion, until its Expiration).
4. Actual: every GetObject is refused with error code 'AccessDenied' and message 'Sign the request with the emulator credentials'.
5. Then iam:DeleteRolePolicy on the role's only inline policy and repeat the GetObject: this is where a refusal is due, and it must be code 'AccessDenied' with AWS's `User: arn:<partition>:sts::<account>:assumed-role/<role-name>/<session-name> is not authorized to perform: s3:GetObject on resource: "arn:<partition>:s3:::<bucket>/<key>" because no identity-based policy allows the s3:GetObject action` message.
