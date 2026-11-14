Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cognito-identity__cognito-idp.xsvc-cognito-identity-2-sstc-disabled-user-token-rejected-and-stays-revoked-after-enable cognito-identity__cognito-idp.xsvc-cognito-identity-2-sstc-disabled-then-deleted-user-token-rejected-twin-pool-still-vends
Issue-Key: cognito-identity__cognito-idp.xsvc-cognito-identity-2-sstc-disabled-user-token-rejected-and-stays-revoked-after-enable|run#12|AssertionError|assert f":assumed-role/{role}/" in arn, f"control: {label} must vend {role!r}, got {arn!r}"
Issue-Key: cognito-identity__cognito-idp.xsvc-cognito-identity-2-sstc-disabled-then-deleted-user-token-rejected-twin-pool-still-vends|run#12|AssertionError|assert f":assumed-role/{role}/" in arn, f"control: {label} must vend {role!r}, got {arn!r}"
Issue-Commit: d0dc720ebbf11903c1fa9407d1c82e53eafd4b75

# cognito-identity__cognito-idp: both server-side-token-check scenarios fail on their positive control — a user-pool token vends the ambient emulator principal, not the pool's authenticated role (1 cause)

> _Note: this issue was identified against the code base at commit `d0dc720ebbf11903c1fa9407d1c82e53eafd4b75`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetCredentialsForIdentity

## Description
ONE root cause (cause 1 of 1).

AWS: with ServerSideTokenCheck enabled on a pool's CognitoIdentityProviders entry, a live user-pool ID token still federates normally: GetCredentialsForIdentity vends an sts:AssumeRoleWithWebIdentity session for the pool's authenticated role, and sts:GetCallerIdentity under those credentials names that role (arn:aws:sts::<account>:assumed-role/<role>/CognitoIdentityCredentials). Both checks use that as their in-file positive control before exercising the revoked/disabled-user legs.
Emulator: the control leg already diverges — the vended credentials resolve to arn:aws:iam::000000000000:user/emulator, the emulator's ambient IAM user, so the role the pool is configured with is never assumed and the checks cannot proceed to the server-side-token-check behavior they exist to measure.
Why: the role a Cognito-federated session runs as is not the configured one, so neither the enabled-user (allow) nor the disabled/deleted-user (revoke) half of server-side token checking can be distinguished — every session is the same over-privileged principal.
Fix: the cognito-identity GetCredentialsForIdentity handler (services/service-com-amazonaws-cognitoidentity-awscognitoidentityservice/src/ops/get_credentials_for_identity.rs) must resolve the identity's role and mint the session through the emulator's STS AssumeRoleWithWebIdentity path.
Related: the companion issue filed in this pass under service token cognito-identity records the same defect from the cognito-identity-only checks; it does not carry these two checks' replay keys. Open issue 20261023T185902Z-cognito-identity-cognito-idp-both-group-role-mapping.md covers the two group/role-mapping checks in this same service group that hang, not these two.
Traces: cognito-identity__cognito-idp.xsvc-cognito-identity-2-sstc-disabled-user-token-rejected-and-stays-revoked-after-enable, cognito-identity__cognito-idp.xsvc-cognito-identity-2-sstc-disabled-then-deleted-user-token-rejected-twin-pool-still-vends.

## Evidence
Replay of suite simparity-behavioral-ga-prod-v1:

- xsvc-cognito-identity-2-sstc-disabled-user-token-rejected-and-stays-revoked-after-enable: "AssertionError: control: SSTC=True must vend 'sim2real-xsvc-sstc-on-438bd8774ac1', got 'arn:aws:iam::000000000000:user/emulator'".
- xsvc-cognito-identity-2-sstc-disabled-then-deleted-user-token-rejected-twin-pool-still-vends: "AssertionError: control: SSTC=True must vend 'sim2real-xsvc-sstc-on-6c669222c8b4', got 'arn:aws:iam::000000000000:user/emulator'".

Model (smithy_query operation=GetCredentialsForIdentity): output Credentials {AccessKeyId, SecretKey, SessionToken, Expiration}; auth [] / optionalAuth true.

## Steps to Reproduce
1. Boot the emulator and replay the behavioral suite; re-replay both check ids in trace_ids.
2. Manual repro: create a user pool + app client and a user; create an identity pool with CognitoIdentityProviders=[{ProviderName:<issuer>, ClientId:<client>, ServerSideTokenCheck:True}] and an authenticated role trusting cognito-identity.amazonaws.com; sign the user in, GetId + GetCredentialsForIdentity with the ID token, then sts:GetCallerIdentity with the vended credentials -> expect a session of the configured authenticated role; observe arn:aws:iam::000000000000:user/emulator.
