Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudtrail__cognito-identity__s3.xsvc-cognito-identity-1-vended-session-denied-list-buckets-audited-with-its-federation
Issue-Key: cloudtrail__cognito-identity__s3.xsvc-cognito-identity-1-vended-session-denied-list-buckets-audited-with-its-federation|run#38|AssertionError|assert granted_key != key, "the post-grant session must be a distinct, freshly vended key"
Issue-Commit: d4edb10874cfe8cae1b981ecc37a41d1433fca3c

# cloudtrail__cognito-identity__s3: GetCredentialsForIdentity re-serves the same AccessKeyId for an identity instead of vending a fresh session per call

> _Note: this issue was identified against the code base at commit `d4edb10874cfe8cae1b981ecc37a41d1433fca3c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetCredentialsForIdentity

## Description
Cause 1 of 1: two successive GetCredentialsForIdentity calls for the same IdentityId and the same Logins token hand back credentials carrying an identical AccessKeyId, so a caller cannot obtain a new session and the two sessions cannot be told apart in the audit trail.

AWS: GetCredentialsForIdentity vends a new set of temporary credentials on every call — Credentials{AccessKeyId, SecretKey, SessionToken, Expiration} for the identity's mapped role — so each call's AccessKeyId differs from the previous one, which is what lets a client pick up a permission change by re-vending and what makes each session separately attributable in CloudTrail (userIdentity.accessKeyId). Smithy: cognito-identity#GetCredentialsForIdentity returns GetCredentialsForIdentityResponse{IdentityId, Credentials{AccessKeyId, SecretKey, SessionToken, Expiration}}; that the value is freshly minted per call is recorded behavior (measured on real AWS in this check: a session vended before a policy grant stayed on the pre-grant denial for 180 s, so only a freshly vended session saw the grant).
Emulator: the second call returns Credentials whose AccessKeyId equals the first call's, so the check's assertion that the post-grant session is "a distinct, freshly vended key" fails.
Why: a client that re-vends credentials to pick up an authorization change gets the old session back, so either the grant appears never to take effect or — worse — a stale session is reused past the point the client believes it was replaced; and because both the denied and the allowed call are attributed to one accessKeyId, per-session auditing collapses to a single key.
Fix: the cognito-identity GetCredentialsForIdentity handler and the session state it mints from (vend a new AccessKeyId/SecretKey/SessionToken triple per call rather than caching one per identity).
Traces: cloudtrail__cognito-identity__s3.xsvc-cognito-identity-1-vended-session-denied-list-buckets-audited-with-its-federation.

## Evidence
cloudtrail__cognito-identity__s3.xsvc-cognito-identity-1-vended-session-denied-list-buckets-audited-with-its-federation — AssertionError: the post-grant session must be a distinct, freshly vended key (assert granted_key != key).
Expected per AWS: GetCredentialsForIdentity(IdentityId, Logins={cognito-identity.amazonaws.com: <token>}) called twice yields two Credentials with different AccessKeyId values, each usable as its own STS session and each appearing as its own userIdentity.accessKeyId in event history.
Actual: the AccessKeyId from the second call is byte-identical to the first.

## Steps to Reproduce
1. Create an identity pool with a developer provider and an authenticated role with no permission policy; mint an identity with GetOpenIdTokenForDeveloperIdentity.
2. Call GetCredentialsForIdentity and record Credentials.AccessKeyId.
3. Attach a permission policy to the role, then call GetCredentialsForIdentity again with the same IdentityId and Logins.
4. Compare the two AccessKeyId values: AWS returns two different keys; the emulator returns the same one.
