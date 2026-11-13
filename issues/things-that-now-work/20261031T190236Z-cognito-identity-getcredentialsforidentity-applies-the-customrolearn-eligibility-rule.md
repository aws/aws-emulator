Issue-Source: trace-sweep-divergence
Issue-Trace: cogid-creds-developer-authenticated-credentials-envelope
Issue-Step: task#7
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cogid-creds-developer-authenticated-credentials-envelope cogid-merge-getid-blank-absorbed-by-registered-developer-login
Issue-Shape-Id: com.amazonaws.cognitoidentity#AWSCognitoIdentityService
Issue-Key: cognito-identity|GetCredentialsForIdentity|4xx->4xx|$headers.x-amzn-errortype
Issue-Related: 20261030T212445Z-cognito-identity-developer-identity-token-and-credential-refusals.md
Issue-Commit: 30fa5e7e24c2e7ff08a1ee60d32c6c6e65abb481

# cognito-identity: GetCredentialsForIdentity applies the CustomRoleArn eligibility rule before validating the login token

> _Note: this issue was identified against the code base at commit `30fa5e7e24c2e7ff08a1ee60d32c6c6e65abb481`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetCredentialsForIdentity

## Description
One root cause, on the GetCredentialsForIdentity refusal-ordering path.

Cause 1 of 1: GetCredentialsForIdentity is called with a Logins entry whose login token is NOT a Cognito-issued OpenID token (e.g. a developer-provider raw token, or any opaque string), AND with a CustomRoleArn that the pool's role configuration does not permit (no SAML provider and no RoleMappings entry for the provider). Two checks therefore both fail, and which one the service reports is observable.

AWS: the login token is validated FIRST, so the refusal is the token refusal, not the role refusal. HTTP 400, body member `__type` = `NotAuthorizedException`, response header `x-amzn-errortype: NotAuthorizedException:` (protocol is awsJson1_1, so the error class appears in both places and a wrong class is wrong in both). The message is entirely fixed text with NO variable parts taken from the request or state: `Invalid login token. Not a Cognito token.`. Smithy (com.amazonaws.cognitoidentity#AWSCognitoIdentityService): GetCredentialsForIdentity errors = ExternalServiceException(400), InvalidIdentityPoolConfigurationException(400), InvalidParameterException(400), NotAuthorizedException(@httpError 403), ResourceConflictException(409), ResourceNotFoundException(404), TooManyRequestsException(429), InternalErrorException(500); input CustomRoleArn targets ARNString @length min 20 max 2048; Logins is a LoginsMap @length min 0 max 10. Both candidate error classes are modeled, but the model does not order the checks and does not state the wire status, so the ordering and the 400-not-403 status are recorded behavior only.

Emulator: HTTP 400 (status matches) but the wrong class and the wrong message — body `__type` = `InvalidParameterException`, header `x-amzn-errortype: InvalidParameterException:`, message `Only SAML providers and providers with RoleMappings support custom role ARN.` (also entirely fixed text). It evaluates CustomRoleArn eligibility first and returns before the Logins token is ever inspected, so the token refusal is unreachable whenever a disallowed CustomRoleArn is also present.

Why: a client branches on the error class. NotAuthorizedException means "re-authenticate: refresh the federated login and retry"; InvalidParameterException means "your request parameters are wrong: fix CustomRoleArn". Against the emulator the caller is told to fix a role ARN it did not get wrong, so it either retries the same dead token indefinitely or surfaces a misleading error, and a negative test written against AWS for the bad-token path fails only on the emulator. The emulator also masks the real condition: the token is bad regardless of whether CustomRoleArn is supplied.

Fix: validation ordering and error selection in the GetCredentialsForIdentity handler's input validation — every Logins token must be validated (and NotAuthorizedException raised with AWS's fixed message) before the CustomRoleArn eligibility rule is applied.

Traces: cogid-creds-developer-authenticated-credentials-envelope#task#7 and cogid-merge-getid-blank-absorbed-by-registered-developer-login#task#11 (2 steps, one cluster).

Open issues that do NOT cover this (the replay reports this cluster as unowned — none of them carries its replay key): '20261030T212445Z-cognito-identity-developer-identity-token-and-credential-refusals.md' describes the same ordering fault in prose but its recorded keys cover only the body members `$.__type`/`$.message` and not this cluster's `$headers.x-amzn-errortype` key, so the cluster is not gated by it; '20261029T230931Z-cognito-identity-error-responses-omit-the-x-amzn.md' is about the header being ABSENT, whereas here the header IS present and merely names the wrong class; '20261029T230931Z-cognito-identity-vended-tokens-and-credentials-are-not.md' and '20261023T185902Z-cognito-identity-input-validation-and-error-classification-diverge.md' are different faults.

## Evidence
Replay of simparity-wire-aws-v1 against the booted emulator (emulator_up=true, 498 steps, 408 passed, 90 mismatched, score 81.9%, bootstrap_signal=false). Cluster divergence_key `service=cognito-identity|GetCredentialsForIdentity|status=400->400|keys=$.__type,$.message,$headers.x-amzn-errortype`, owned_by empty, stub_path empty, affected_count 2.

Representative step cogid-creds-developer-authenticated-credentials-envelope#task#7 (also cogid-merge-getid-blank-absorbed-by-registered-developer-login#task#11):
- status: expected 400, got 400 (matches)
- $.__type: expected 'NotAuthorizedException', got 'InvalidParameterException'
- $.message: expected 'Invalid login token. Not a Cognito token.', got 'Only SAML providers and providers with RoleMappings support custom role ARN.'
- $headers.x-amzn-errortype: expected the NotAuthorizedException form, got 'InvalidParameterException:'

Both messages are 100% fixed text for their error class: neither template has variable parts, so the expected string above is the requirement verbatim and nothing in it comes from a per-run value (no ids, names, timestamps or request ids are involved).

Smithy citation: GetCredentialsForIdentity errors include NotAuthorizedException (@httpError 403) and InvalidParameterException (@httpError 400); input members IdentityId (required, @length 1..55, pattern ^[\w-]+:[0-9a-f-]+$), Logins (LoginsMap, @length 0..10), CustomRoleArn (ARNString, @length 20..2048). The check ordering between the Logins-token check and the CustomRoleArn eligibility rule, and the fact AWS answers 400 for NotAuthorizedException on this path, are recorded behavior only.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the simparity-wire-aws-v1 wire corpus.
2. Replay fixture cogid-creds-developer-authenticated-credentials-envelope, step task#7: GetCredentialsForIdentity for a developer-authenticated identity, passing a Logins entry whose value is not a Cognito-issued OpenID token together with a CustomRoleArn the pool's configuration does not permit (no SAML provider, no RoleMappings entry for that provider).
3. Observe the response: expected HTTP 400 with body {"__type": "NotAuthorizedException", "message": "Invalid login token. Not a Cognito token."} and header x-amzn-errortype naming NotAuthorizedException; the emulator answers HTTP 400 with __type InvalidParameterException, message 'Only SAML providers and providers with RoleMappings support custom role ARN.' and header 'InvalidParameterException:'.
4. Confirm the ordering is the cause by repeating the call WITHOUT CustomRoleArn: the bad login token must still be refused as NotAuthorizedException with the same fixed message.
5. Same divergence at cogid-merge-getid-blank-absorbed-by-registered-developer-login#task#11.
