Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cognito-idp.complete-webauthn-registration-foreign-origin-rejected cognito-idp.complete-webauthn-registration-other-client-token-rejected cognito-idp.passkey-enrollment-without-relying-party-refused cognito-idp.complete-webauthn-registration-foreign-rp-id-hash-rejected cognito-idp.admin-create-user-validation-data-is-transient cognito-idp.admin-confirm-sign-up-oversized-client-metadata-key-rejected cognito-idp.custom-message-trigger-stall-rejects-admin-email-change-but-keeps-new-email
Issue-Shape-Id: com.amazonaws.cognitoidentityprovider#AWSCognitoIdentityProviderService
Issue-Key: cognito-idp.complete-webauthn-registration-foreign-origin-rejected|run#16|AssertionError|assert refused == expected, (
Issue-Key: cognito-idp.complete-webauthn-registration-other-client-token-rejected|run#22|AssertionError|assert message == "Challenge is not issued by the same client as the presented access token", (
Issue-Key: cognito-idp.passkey-enrollment-without-relying-party-refused|run#18|AssertionError|assert got == ("WebAuthnConfigurationMissingException", _MISSING_MESSAGE), (
Issue-Key: cognito-idp.complete-webauthn-registration-foreign-rp-id-hash-rejected|_refusal#20|AssertionError|raise AssertionError(f"the call was accepted, answering {answer!r}")
Issue-Key: cognito-identity-provider|AdminCreateUser|InvalidParameterException
Issue-Key: cognito-idp.admin-confirm-sign-up-oversized-client-metadata-key-rejected|run#13|AssertionError|assert (code, message) == ("InvalidParameterException", expected), (
Issue-Key: cognito-idp.custom-message-trigger-stall-rejects-admin-email-change-but-keeps-new-email|run#26|AssertionError|assert got == ("UnexpectedLambdaException",
Issue-Commit: 7eb46ae4754f02cb00b49bce755f449d67bad34e

# cognito-idp: WebAuthn refusals and five residual divergences (AdminCreateUser ValidationData, AdminConfirmSignUp metadata, Lambda timeout wording)

> _Note: this issue was identified against the code base at commit `7eb46ae4754f02cb00b49bce755f449d67bad34e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CompleteWebAuthnRegistration

## Description
Six defects left after the other clusters.

Cause 1 of 6: the WebAuthn refusal messages are the emulator's own prose.
AWS: CompleteWebAuthnRegistration from an origin that is not allowed is refused WebAuthnOriginNotAllowedException 'The origin is not allowed'; completing with an access token of a different client is refused WebAuthnClientMismatchException 'Challenge is not issued by the same client as the presented access token'; starting a passkey registration on a pool with neither a relying-party ID nor a domain is refused with the message AWS words for that configuration error, not the emulator's sentence. The fixed text is the requirement; no request value appears in these messages.
Emulator: answers 'The registration origin <origin> does not align with the user pool relying party ID <rp>.', 'The access token is for a different client than the one in the original StartWebAuthnRegistration request.' and 'This user pool does not have a relying party ID or a user pool domain configured.'
Why: clients matching the documented WebAuthn errors see unrecognised text (and the emulator leaks the configured relying party in one of them).
Fix: the WebAuthn registration handlers' error messages.
Traces: cognito-idp.complete-webauthn-registration-foreign-origin-rejected, cognito-idp.complete-webauthn-registration-other-client-token-rejected, cognito-idp.passkey-enrollment-without-relying-party-refused.

Cause 2 of 6: a credential whose rpIdHash belongs to another relying party is accepted.
AWS: refuses the registration (recorded behavior only: the recording shows a ClientError).
Emulator: answers HTTP 200 with an empty body, storing the credential.
Why: a passkey minted for another relying party becomes a valid first factor — a real authentication bypass.
Fix: verify the attestation's rpIdHash against the pool's relying-party ID in CompleteWebAuthnRegistration.
Traces: cognito-idp.complete-webauthn-registration-foreign-rp-id-hash-rejected.

Cause 3 of 6: AdminCreateUser refuses ValidationData that AWS accepts.
AWS: ValidationData values of 2048 and 2049 characters are accepted on a pool with no pre sign-up trigger; the data is transient, so each user reads back FORCE_CHANGE_PASSWORD holding only 'sub'.
Emulator: refuses AdminCreateUser with InvalidParameterException "1 validation error detected: Value '<the 2048-character value>' ...", so the user is never created.
Why: callers that pass validation data to their triggers cannot create users at all.
Fix: the AdminCreateUser ValidationData validation (bound/member) and the transient handling of the data.
Traces: cognito-idp.admin-create-user-validation-data-is-transient.

Cause 4 of 6: AdminConfirmSignUp judges user status before validating ClientMetadata.
AWS: an over-long ClientMetadata KEY is refused with the map-key length validation error naming 'clientMetadata'.
Emulator: answers NotAuthorizedException 'User cannot be confirmed. Current status is FORCE_CHANGE_PASSWORD' — the oversized key is never reported.
Why: the caller is told about user status for a malformed request, so the input error is invisible.
Fix: run AdminConfirmSignUp input validation before the user-status check.
Traces: cognito-idp.admin-confirm-sign-up-oversized-client-metadata-key-rejected.

Cause 5 of 6: the Lambda-timeout refusal is worded wrongly.
AWS: a trigger that overruns its limit is reported as UnexpectedLambdaException '<Trigger> invocation failed due to error Socket timeout while invoking Lambda function.' — fixed text with the trigger name substituted (here CustomMessage).
Emulator: answers '<Trigger> failed with error Socket timeout while invoking Lambda function.'
Why: message divergence on the documented trigger-failure error; clients that match it cannot. Related open issue 20261031T170200Z-cognito-idp-lambda-reopens-cognito-idp-lambda-preauthentication-pretokengeneration-triggers-every.md records the same wording for PreTokenGeneration; this CustomMessage check is not marked owned.
Fix: the trigger-invocation error mapping shared by the trigger call sites.
Traces: cognito-idp.custom-message-trigger-stall-rejects-admin-email-change-but-keeps-new-email.

Cause 6 of 6: (bundled with Cause 5) the attribute write ordering around a failing CustomMessage trigger.
AWS: when the CustomMessage trigger stalls on an admin email change, the refusal still leaves the new email stored with email_verified 'false'; the next change succeeds.
Emulator: cannot be observed past the Cause-5 message mismatch; the same replay covers it.
Why: a client cannot tell whether the attribute write landed.
Fix: same handler as Cause 5 — keep the attribute write ahead of the verification-message step.
Traces: cognito-idp.custom-message-trigger-stall-rejects-admin-email-change-but-keeps-new-email.

## Evidence
Cause 1 — "registration from 'https://signin.example.net' answered ('WebAuthnOriginNotAllowedException', 'The registration origin https://signin.example.net does not align with the user pool relying party ID example.com.'), expected ('WebAuthnOriginNotAllowedException', 'The origin is not allowed')"; "WebAuthnClientMismatchException carried message 'The access token is for a different client than the one in the original StartWebAuthnRegistration request.'" where the check requires 'Challenge is not issued by the same client as the presented access token'; "enrolling a passkey with no relying party or domain answered ('WebAuthnConfigurationMissingException', 'This user pool does not have a relying party ID or a user pool domain configured.')".
Cause 2 — "the call was accepted, answering {'ResponseMetadata': {... 'HTTPStatusCode': 200 ...}}".
Cause 3 — botocore.errorfactory.InvalidParameterException on AdminCreateUser: "1 validation error detected: Value 'vvvv...' ..." for ValidationData=[{'Name':'invite','Value':'v'*2048}].
Cause 4 — "a 131073-character metadata key was refused with 'NotAuthorizedException' ... 'User cannot be confirmed. Current status is FORCE_CHANGE_PASSWORD'".
Cause 5 — got ('UnexpectedLambdaException', 'CustomMessage failed with error Socket timeout while invoking Lambda function.'), expected ('UnexpectedLambdaException', 'CustomMessage invocation failed due to error Socket timeout while invoking Lambda function.').

## Steps to Reproduce
1. Replay the check ids in trace_ids.
2. Cause 1/2: StartWebAuthnRegistration, then CompleteWebAuthnRegistration with (a) a credential from a foreign origin, (b) an access token of another app client, (c) a credential whose authenticator data carries another relying party's rpIdHash; and StartWebAuthnRegistration on a pool with no relying-party ID or domain.
3. Cause 3: AdminCreateUser with ValidationData=[{'Name':'invite','Value':'v'*2048}] and 'v'*2049, then AdminGetUser.
4. Cause 4: SignUp, then AdminConfirmSignUp with a ClientMetadata key of 131073 characters.
5. Cause 5: attach a CustomMessage trigger that sleeps past the limit and call AdminUpdateUserAttributes to change the email.
