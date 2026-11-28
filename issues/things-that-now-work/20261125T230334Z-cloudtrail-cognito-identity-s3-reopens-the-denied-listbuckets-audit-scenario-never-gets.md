Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudtrail__cognito-identity__s3.xsvc-cognito-identity-1-vended-session-denied-list-buckets-audited-with-its-federation
Issue-Key: cloudtrail__cognito-identity__s3.xsvc-cognito-identity-1-vended-session-denied-list-buckets-audited-with-its-federation|_poll_until#35|Timeout|time.sleep(interval_s)
Issue-Commit: 50eec8fda4c6238e3cbd0bab35b991aa1443e257

# REOPENS: the denied-ListBuckets audit scenario never gets a Cognito session — GetCredentialsForIdentity refuses the pool's own developer-identity token as "Not a Cognito token", so the check is killed by the 60 s cap

> _Note: this issue was identified against the code base at commit `50eec8fda4c6238e3cbd0bab35b991aa1443e257`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetCredentialsForIdentity

## Description
REOPENS 20261116T235533Z-cognito-identity-getcredentialsforidentity-applies-the-customrolearn-eligibility-rule--r3.md: its fix left the call refused. That fix introduced the "Invalid login token. Not a Cognito token." NotAuthorizedException for a Logins value that is not a Cognito-issued OpenID token; it now fires for a token this emulator's own GetOpenIdTokenForDeveloperIdentity minted, so no Cognito-vended session can be obtained and this audit scenario reaches no assertion. (Fixed issue 20261110T092801Z-cognito-identity-a-just-minted-developer-identity-openid.md also claimed to stop a just-minted developer-identity token being refused by this operation.)

Cause 1 of 1: GetCredentialsForIdentity rejects a pool-issued developer-identity OpenID token, and the refusal is retried to the budget.

AWS: the caller passes the Token returned by GetOpenIdTokenForDeveloperIdentity back as the value of GetCredentialsForIdentity's Logins entry keyed "cognito-identity.amazonaws.com" and receives HTTP 200 with Credentials (AccessKeyId, SecretKey, SessionToken, Expiration) for the pool's authenticated role; the session then acts as arn:aws:sts::<account>:assumed-role/<roleName>/CognitoIdentityCredentials. Smithy (com.amazonaws.cognitoidentity#AWSCognitoIdentityService, awsJson1_1): GetCredentialsForIdentity input IdentityId (required, @length 1..55, @pattern ^[\w-]+:[0-9a-f-]+$), Logins (LoginsMap, @length 0..10), CustomRoleArn (@length 20..2048); output {IdentityId, Credentials}; errors ExternalServiceException(400), InternalErrorException(500), InvalidIdentityPoolConfigurationException(400), InvalidParameterException(400), NotAuthorizedException(client, @httpError 403), ResourceConflictException(409), ResourceNotFoundException(404), TooManyRequestsException(429). GetOpenIdTokenForDeveloperIdentity's output Token targets OIDCToken (@sensitive). The model does not classify Logins values; that a valid pool-issued developer-identity token must be accepted, and that "Invalid login token. Not a Cognito token." (wholly fixed text, no interpolated values) is reserved for a value that is not a Cognito-issued token, is recorded behavior.

Emulator: the vend is refused NotAuthorizedException "Invalid login token. Not a Cognito token." for the token the emulator minted seconds earlier for the same pool and identity with TokenDuration=900. The check wraps the vend in a poll that treats a ClientError as not-yet and re-vends for up to 180 s, so the refusal repeats until the 60 s replay cap ends the check: "Check timed out after 60.0s". Nothing after the vend — the denied S3 ListBuckets, the one-permission repair, and the CloudTrail event-history comparison of the denied and allowed records — is ever exercised.

Why: a client relying on AWS's behavior cannot get a web-identity session on the developer-authenticated path, and because the refusal class is NotAuthorizedException ("re-authenticate") its correct reaction — mint a fresh token and retry — loops forever. Downstream, no audit record for a Cognito-vended key can exist at all, so every assertion about how such a call is attributed is unreachable rather than merely wrong.

Fix: the Logins token validation stage of the GetCredentialsForIdentity handler — accept a token issued by this service's own GetOpenIdTokenForDeveloperIdentity (issuer/shape/pool binding under the "cognito-identity.amazonaws.com" login key) and continue to role resolution, keeping that fixed refusal for values that really are not pool-issued OpenID tokens.

Traces: cloudtrail__cognito-identity__s3.xsvc-cognito-identity-1-vended-session-denied-list-buckets-audited-with-its-federation.

Open issues the replay report does NOT record as owning this check, and that do not cover it: 20261122T053725Z-cloudtrail-cognito-identity-s3-a-cognito-vended-session.md and 20261123T160849Z-cloudtrail-cognito-identity-s3-a-denied-call-made.md both describe a missing userIdentity.sessionContext.webIdFederationData on an audit record that DOES exist — i.e. a session that was successfully vended; here the vend itself never succeeds. 20261029T173328Z-cloudtrail-s3-listbuckets-succeeds-for-a-cognito-vended-session.md describes the opposite outcome for the same scenario's denial leg (ListBuckets succeeding for a session with no policy), which also presupposes a vended session.

## Evidence
Behavioral suite replay against the emulator built from this checkout: 2331/2681 passed.

Failing check cloudtrail__cognito-identity__s3.xsvc-cognito-identity-1-vended-session-denied-list-buckets-audited-with-its-federation — verdict [mismatch], detail: "Check timed out after 60.0s".

The first blocking call is the credential vend, polled with a ClientError-tolerant predicate:
  vended = _poll_until(
      lambda: ci.get_credentials_for_identity(
          IdentityId=identity_id, Logins={"cognito-identity.amazonaws.com": token}),
      lambda r: bool(r.get("Credentials", {}).get("SessionToken")),
      description="GetCredentialsForIdentity to vend authenticated-role credentials")
where identity_id/token come from ci.get_open_id_token_for_developer_identity(IdentityPoolId=<pool>, Logins={<devProvider>: <user>}, TokenDuration=900), which answered 200. Because the vend never returns Credentials, the poll runs to its own 180 s budget and the 60 s replay cap fires first.

The same call, made directly (two sibling checks in the cognito-identity__kms__s3 group that do not wrap it in a poll), reports the emulator's actual answer verbatim:
  botocore.errorfactory.NotAuthorizedException: An error occurred (NotAuthorizedException) when calling the GetCredentialsForIdentity operation: Invalid login token. Not a Cognito token.

Expected (AWS): HTTP 200 with Credentials.{AccessKeyId, SecretKey, SessionToken, Expiration}. Message classification: "Invalid login token. Not a Cognito token." is entirely fixed text with no request- or state-derived parts; the defect is that it is emitted for a pool-issued token rather than any wording difference.

## Steps to Reproduce
1. CreateIdentityPool(IdentityPoolName=<name>, AllowUnauthenticatedIdentities=False, DeveloperProviderName=<devProvider>).
2. CreateRole with a trust policy allowing sts:AssumeRoleWithWebIdentity to Federated principal cognito-identity.amazonaws.com, conditioned on StringEquals cognito-identity.amazonaws.com:aud = the pool id and ForAnyValue:StringLike cognito-identity.amazonaws.com:amr = authenticated; SetIdentityPoolRoles(Roles={"authenticated": <roleArn>}). Attach no permission policy yet.
3. GetOpenIdTokenForDeveloperIdentity(IdentityPoolId=<pool>, Logins={<devProvider>: <user>}, TokenDuration=900) -> {IdentityId, Token}.
4. GetCredentialsForIdentity(IdentityId=<IdentityId>, Logins={"cognito-identity.amazonaws.com": <Token>}).
5. AWS answers 200 with a Credentials envelope; the emulator answers NotAuthorizedException "Invalid login token. Not a Cognito token." Repeating step 3 and 4 produces the same refusal, which is why the check's retry loop consumes its whole budget.
6. Re-replay to confirm: behavioral check cloudtrail__cognito-identity__s3.xsvc-cognito-identity-1-vended-session-denied-list-buckets-audited-with-its-federation.
