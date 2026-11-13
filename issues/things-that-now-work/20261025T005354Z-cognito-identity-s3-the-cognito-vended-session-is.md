Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cognito-identity__s3.xsvc-cognito-identity-3-bucket-policy-grants-the-federated-role-and-its-deny-prefix-wins
Issue-Key: cognito-identity__s3.xsvc-cognito-identity-3-bucket-policy-grants-the-federated-role-and-its-deny-prefix-wins|run#33|AssertionError|assert f":assumed-role/{role_name}/" in who, (
Issue-Commit: d0dc720ebbf11903c1fa9407d1c82e53eafd4b75

# cognito-identity__s3: the Cognito-vended session is not a session of the mapped role, so a bucket policy written for that role principal cannot be evaluated (1 cause)

> _Note: this issue was identified against the code base at commit `d0dc720ebbf11903c1fa9407d1c82e53eafd4b75`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetCredentialsForIdentity

## Description
ONE root cause (cause 1 of 1).

AWS: GetCredentialsForIdentity vends an sts:AssumeRoleWithWebIdentity session for the role the pool maps the identity to, so sts:GetCallerIdentity under those credentials answers an ARN containing ':assumed-role/<mapped-role-name>/'. That is exactly the principal a bucket policy naming the role ARN grants (and whose Deny on a prefix must win), which is why the check asserts the session's identity before it probes the allow/deny prefixes.
Emulator: the vended session resolves to arn:aws:iam::000000000000:user/emulator — the emulator's ambient IAM user, not a session of the mapped role — so the bucket policy's principal never matches the caller and neither the grant nor the overriding prefix Deny can be exercised.
Why: resource-policy authorization for Cognito-federated callers cannot be expressed at all: the caller is a single shared IAM user for every identity and every pool, so a bucket policy that grants one federated role and denies a prefix is inert.
Fix: the cognito-identity GetCredentialsForIdentity handler (services/service-com-amazonaws-cognitoidentity-awscognitoidentityservice/src/ops/get_credentials_for_identity.rs) must mint the session through the emulator's STS AssumeRoleWithWebIdentity path for the resolved/mapped role so that S3's policy evaluation sees the assumed-role principal.
Related: the companion issue filed in this pass under service token cognito-identity records the same defect from the cognito-identity-only checks and does not carry this check's replay key.
Traces: cognito-identity__s3.xsvc-cognito-identity-3-bucket-policy-grants-the-federated-role-and-its-deny-prefix-wins.

## Evidence
Replay of suite simparity-behavioral-ga-prod-v1:

cognito-identity__s3.xsvc-cognito-identity-3-bucket-policy-grants-the-federated-role-and-its-deny-prefix-wins
  File "check_…_bucket_policy_grants_the_federated_role_and_its_deny_prefix_wins.py", line 243, in run
    assert f":assumed-role/{role_name}/" in who, (
  AssertionError: the vended session is 'arn:aws:iam::000000000000:user/emulator', not a session of the mapped role 'sim2real-xsvc-ci3-guest-b292c8f4be79'

Model (smithy_query operation=GetCredentialsForIdentity): output Credentials {AccessKeyId, SecretKey, SessionToken, Expiration}; auth [] / optionalAuth true.

## Steps to Reproduce
1. Boot the emulator and replay the behavioral suite; re-replay the check id in trace_ids.
2. Manual repro: create an identity pool whose unauthenticated/guest role is <role>; GetId + GetCredentialsForIdentity; call sts:GetCallerIdentity with the vended credentials -> expect an ARN containing ':assumed-role/<role>/'; observe arn:aws:iam::000000000000:user/emulator. Then attach a bucket policy that Allows arn:aws:iam::<account>:role/<role> on the bucket and Denies one prefix, and read an object under each prefix with that session -> expect allow outside the prefix and deny inside it.
