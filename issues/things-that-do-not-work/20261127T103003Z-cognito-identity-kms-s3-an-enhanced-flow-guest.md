Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cognito-identity__kms__s3.guest-session-scope-down-policies-limit-the-unauthenticated-role
Issue-Key: cognito-identity__kms__s3.guest-session-scope-down-policies-limit-the-unauthenticated-role|run#39|AssertionError|assert not wrong, "; ".join(wrong).replace(ctx.account, "<account>")
Issue-Commit: 8133ebe260b7cb1c7e11e2995dae089129b7fabf

# cognito-identity__kms__s3: an enhanced-flow guest session is not scoped down — iam:GetRole answers NoSuchEntity instead of a session-policy AccessDenied

> _Note: this issue was identified against the code base at commit `8133ebe260b7cb1c7e11e2995dae089129b7fabf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetCredentialsForIdentity

## Description
One root cause, grounded in a failing behavioral check.

Cause 1 of 1: credentials vended by the enhanced (GetCredentialsForIdentity) flow carry no session policies, so an action no session policy allows is evaluated as if it were allowed.
  AWS: recorded behavior only. The enhanced flow attaches two session policies to the unauthenticated role's session (the inline one, which covers s3:* among others but no iam: or kms:, and AmazonCognitoUnAuthedIdentitiesSessionPolicy, which covers a KMS subset). iam:GetRole is in neither, so IAM — which evaluates policy before resource existence — refuses the call with ('AccessDenied', 'User: arn:aws:sts::<account>:assumed-role/<unauth role>/CognitoIdentityCredentials is not authorized to perform: iam:GetRole on resource: <role arn> ...'). The basic (GetOpenIdToken + AssumeRoleWithWebIdentity) flow passes no session policy, so the same probe is answered by IAM's not-found: ('NoSuchEntity', 'The role with name <name> cannot be found.').
  Emulator: the enhanced-flow probe answers ('NoSuchEntity', 'The role with name sim2real-behav-guest-absent-<id> cannot be found.') — the basic-flow answer — showing the vended session is unrestricted; the s3 and kms probes (which both services answer not-found before policy) match, so the divergence is exactly the missing session-policy scope-down.
  Why: a client that relies on Cognito's guest credentials being scoped down (the whole point of the unauthenticated identity session policies) gets a session that is allowed actions AWS denies; code paths that assert the denial, and any security reasoning built on it, are wrong.
  Fix: the cognito-identity GetCredentialsForIdentity handler — attach the enhanced-flow session policies to the issued session, and have the IAM authorization gate evaluate them before resource existence.
  Traces: cognito-identity__kms__s3.guest-session-scope-down-policies-limit-the-unauthenticated-role.

## Evidence
cognito-identity__kms__s3.guest-session-scope-down-policies-limit-the-unauthenticated-role —
AssertionError: iam:GetRole in the enhanced flow: got ('NoSuchEntity', 'The role with name sim2real-behav-guest-absent-5dcfb65cdd34 cannot be found.').
Expected ('AccessDenied', 'User: arn:aws:sts::<account>:assumed-role/<unauth-role>/CognitoIdentityCredentials is not authorized to perform: iam:GetRole on resource: <absent role> ...'), the refusal no session policy allows.

## Steps to Reproduce
1. Create an identity pool allowing unauthenticated identities with an unauthenticated role.
2. GetId then GetCredentialsForIdentity (enhanced flow) and build an IAM client from the returned credentials.
3. Call iam:GetRole for a role name that does not exist.
4. Observe NoSuchEntity; AWS answers AccessDenied because no session policy on the vended session allows iam:GetRole. (The same probe through the basic flow correctly answers NoSuchEntity.)
