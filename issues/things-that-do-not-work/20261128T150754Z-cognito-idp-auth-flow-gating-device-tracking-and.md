Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cognito-idp.custom-auth-flow-rejected-on-client-without-allow-custom-auth cognito-idp.custom-auth-without-define-auth-challenge-trigger-rejected cognito-idp.initiate-auth-custom-auth-on-a-client-without-allow-custom-auth-rejected cognito-idp.password-srp-preferred-challenge-without-choice-flow-rejected cognito-idp.user-auth-flow-needs-allow-user-auth-rejected cognito-idp.user-auth-password-srp-completes-on-client-without-srp-flow cognito-idp.user-auth-select-challenge-wrong-password-rejected cognito-idp.initiate-auth-admin-user-password-auth-flow-rejected-on-the-public-operation cognito-idp.initiate-auth-admin-user-password-flow-rejected cognito-idp.initiate-auth-refuses-the-admin-password-flow-whatever-the-password-rejected cognito-idp.admin-password-sign-in-rejected-after-client-update-omits-explicit-auth-flows cognito-idp.admin-refresh-token-auth-rejected-while-the-client-rotates-refresh-tokens cognito-idp.refresh-token-auth-honoured-on-client-without-refresh-flow cognito-idp.admin-initiate-auth-mfa-required-without-phone-rejected cognito-idp.admin-set-user-settings-unknown-pool-rejected cognito-idp.admin-update-device-status-under-a-name-other-than-the-owner-rejected cognito-idp.forget-device-while-admin-password-reset-pending-rejected cognito-idp.update-device-status-deleted-user-rejected cognito-idp.update-device-status-while-password-reset-required-rejected cognito-idp.device-srp-answer-rejected-once-device-tracking-is-turned-off cognito-idp.list-devices-refuses-access-token-until-pool-tracks-devices cognito-idp.confirm-device-sign-bit-verifier-rejected cognito-idp.admin-respond-to-auth-challenge-in-inactive-replica-region-rejected cognito-idp.admin-update-user-attributes-in-secondary-replica-region-rejected cognito-idp.delete-user-attributes-in-replica-region-rejected cognito-idp.get-client-token-refused-in-inactive-replica-region cognito-idp.replica-region-admin-delete-user-attributes-rejected cognito-idp.replica-region-refuses-device-forget-and-mfa-preference-rejected cognito-idp.replica-region-refuses-password-and-device-writes-the-primary-accepts
Issue-Shape-Id: com.amazonaws.cognitoidentityprovider#AWSCognitoIdentityProviderService
Issue-Key: cognito-idp.custom-auth-flow-rejected-on-client-without-allow-custom-auth|run#20|AssertionError|assert (code, message) == (
Issue-Key: cognito-idp.custom-auth-without-define-auth-challenge-trigger-rejected|run#16|AssertionError|assert before == ("InvalidParameterException", "CUSTOM_AUTH is not enabled for the client."), (
Issue-Key: cognito-idp.initiate-auth-custom-auth-on-a-client-without-allow-custom-auth-rejected|run#17|AssertionError|assert (code, message) == ("InvalidParameterException", "CUSTOM_AUTH is not enabled for the client."), (
Issue-Key: cognito-idp.password-srp-preferred-challenge-without-choice-flow-rejected|run#18|AssertionError|assert got == want, f"USER_AUTH PASSWORD_SRP on a client without ALLOW_USER_AUTH: {got!r}, want {want!r}"
Issue-Key: cognito-idp.user-auth-flow-needs-allow-user-auth-rejected|run#17|AssertionError|assert got == ("InvalidParameterException", "USER_AUTH flow not enabled for this client"), (
Issue-Key: cognito-idp.user-auth-password-srp-completes-on-client-without-srp-flow|run#11|AssertionError|assert wrong == ("NotAuthorizedException", "Incorrect username or password."), (
Issue-Key: cognito-idp.user-auth-select-challenge-wrong-password-rejected|run#14|AssertionError|assert (code, message) == ("NotAuthorizedException", "Incorrect username or password."), (
Issue-Key: cognito-idp.initiate-auth-admin-user-password-auth-flow-rejected-on-the-public-operation|run#16|AssertionError|assert (code, message) == ("InvalidParameterException", "Initiate Auth method not supported."), (
Issue-Key: cognito-idp.initiate-auth-admin-user-password-flow-rejected|run#20|AssertionError|assert got == ("InvalidParameterException", "Initiate Auth method not supported."), (
Issue-Key: cognito-idp.initiate-auth-refuses-the-admin-password-flow-whatever-the-password-rejected|run#9|AssertionError|assert got == expected, (
Issue-Key: cognito-identity-provider|AdminInitiateAuth|InvalidParameterException
Issue-Key: cognito-identity-provider|CreateUserPool|InvalidParameterException
Issue-Key: cognito-idp.admin-update-device-status-under-a-name-other-than-the-owner-rejected|run#15|AssertionError|assert status_before == "remembered", f"the remembered device reads back status {status_before!r}"
Issue-Key: cognito-idp.forget-device-while-admin-password-reset-pending-rejected|run#15|AssertionError|assert _status_of(idp, pool_id, username, back_key) == "remembered", "the back-office device is not remembered"
Issue-Key: cognito-idp.update-device-status-deleted-user-rejected|run#8|AssertionError|assert status == "not_remembered", (
Issue-Key: cognito-idp.update-device-status-while-password-reset-required-rejected|_remembered_status#16|AssertionError|assert "dev:device_remembered_status" in attrs, (
Issue-Key: cognito-idp.device-srp-answer-rejected-once-device-tracking-is-turned-off|run#20|AssertionError|assert started.get("ChallengeName") == "DEVICE_SRP_AUTH", (
Issue-Key: cognito-idp.list-devices-refuses-access-token-until-pool-tracks-devices|_refusal#15|AssertionError|raise AssertionError(f"expected a refusal, the call succeeded: {result!r}")
Issue-Key: cognito-idp.confirm-device-sign-bit-verifier-rejected|_refusal#15|AssertionError|raise AssertionError(f"the call was expected to be refused but returned {answer!r}")
Issue-Key: cognito-idp.admin-respond-to-auth-challenge-in-inactive-replica-region-rejected|_refusal#20|AssertionError|raise AssertionError(f"the call was expected to be refused but returned {sorted(answer)!r}")
Issue-Key: cognito-idp.admin-update-user-attributes-in-secondary-replica-region-rejected|_refusal#19|AssertionError|raise AssertionError(f"the call was expected to be refused but succeeded: {answer!r}")
Issue-Key: cognito-idp.delete-user-attributes-in-replica-region-rejected|_refusal#33|AssertionError|raise AssertionError(f"the call was expected to be refused but returned {result!r}")
Issue-Key: cognito-idp.get-client-token-refused-in-inactive-replica-region|_refusal#38|AssertionError|raise AssertionError(f"expected a refusal, the call succeeded with keys {sorted(answer)!r}")
Issue-Key: cognito-idp.replica-region-admin-delete-user-attributes-rejected|_refusal#24|AssertionError|raise AssertionError(f"the call was expected to be refused but returned {answer!r}")
Issue-Key: cognito-idp.replica-region-refuses-device-forget-and-mfa-preference-rejected|_refusal#31|AssertionError|raise AssertionError(f"expected a refusal, the call succeeded with {got!r}")
Issue-Key: cognito-idp.replica-region-refuses-password-and-device-writes-the-primary-accepts|run#22|AssertionError|assert before == "not_remembered", f"a device confirmed without a prompt answer reads {before!r}"
Issue-Related: 20261128T150754Z-cognito-idp-request-validation-refusal-messages-diverge-from.md
Issue-Commit: 7eb46ae4754f02cb00b49bce755f449d67bad34e

# cognito-idp: auth-flow gating, device tracking and multi-region replicas diverge (9 causes)

> _Note: this issue was identified against the code base at commit `7eb46ae4754f02cb00b49bce755f449d67bad34e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
InitiateAuth

## Description
Nine defects around which auth flows a client may use, device tracking, and what a replica region may serve.

Cause 1 of 9: CUSTOM_AUTH and USER_AUTH answer a scaffold error instead of AWS's flow refusal.
AWS: when the app client does not allow the flow, (Admin)InitiateAuth refuses with InvalidParameterException naming the flow — e.g. 'USER_AUTH flow not enabled for this client' for USER_AUTH/PASSWORD_SRP on a client without ALLOW_USER_AUTH — and a CUSTOM_AUTH request on a client without ALLOW_CUSTOM_AUTH (or on a pool with no DefineAuthChallenge trigger) is likewise refused by the operation, not by a missing implementation.
Emulator: answers error code 'NotImplemented' with 'operation InitiateAuth with AuthFlow=CUSTOM_AUTH is scaffolded but not implemented in this emulator' (same for AuthFlow=USER_AUTH with any PREFERRED_CHALLENGE), so even the NEGATIVE cases, which need no flow implementation, cannot be exercised. Related open issue 20261025T014422Z-apigateway-cognito-idp-lambda-the-defineauthchallenge-trigger-chain.md and 20261024T114943Z-cognito-idp-lambda-the-pool-s-jwks-document.md own other checks of these flows; these checks were not marked as owned.
Why: a client that must see the flow refusal gets an unretriable server-side 'not implemented'.
Fix: the InitiateAuth/AdminInitiateAuth AuthFlow dispatch must validate the client's ExplicitAuthFlows and refuse before reaching the scaffold stub.
Traces: cognito-idp.custom-auth-flow-rejected-on-client-without-allow-custom-auth, cognito-idp.custom-auth-without-define-auth-challenge-trigger-rejected, cognito-idp.initiate-auth-custom-auth-on-a-client-without-allow-custom-auth-rejected, cognito-idp.password-srp-preferred-challenge-without-choice-flow-rejected, cognito-idp.user-auth-flow-needs-allow-user-auth-rejected, cognito-idp.user-auth-password-srp-completes-on-client-without-srp-flow, cognito-idp.user-auth-select-challenge-wrong-password-rejected.

Cause 2 of 9: the admin-only flow refusal on the public operation is worded wrongly.
AWS: InitiateAuth with AuthFlow=ADMIN_USER_PASSWORD_AUTH is refused InvalidParameterException 'Initiate Auth method not supported.' whatever the credentials.
Emulator: answers InvalidParameterException 'ADMIN_USER_PASSWORD_AUTH is not a valid AuthFlow for InitiateAuth'.
Why: message divergence on a documented refusal that marks the admin/public boundary.
Fix: the InitiateAuth AuthFlow validation message.
Traces: cognito-idp.initiate-auth-admin-user-password-auth-flow-rejected-on-the-public-operation, cognito-idp.initiate-auth-admin-user-password-flow-rejected, cognito-idp.initiate-auth-refuses-the-admin-password-flow-whatever-the-password-rejected.

Cause 3 of 9: a client whose ExplicitAuthFlows is omitted loses REFRESH_TOKEN_AUTH.
AWS: UpdateUserPoolClient/CreateUserPoolClient without ExplicitAuthFlows leaves the client able to refresh (and to run the legacy flows AWS enables by default), so refresh-token sign-ins keep working.
Emulator: refuses with InvalidParameterException 'REFRESH_TOKEN_AUTH flow not enabled for this client'.
Why: every client created or updated without an explicit flow list cannot refresh, so sessions die at the access-token lifetime.
Fix: the app-client flow defaults applied by Create/UpdateUserPoolClient and read by the auth flow check.
Traces: cognito-idp.admin-password-sign-in-rejected-after-client-update-omits-explicit-auth-flows, cognito-idp.admin-refresh-token-auth-rejected-while-the-client-rotates-refresh-tokens, cognito-idp.refresh-token-auth-honoured-on-client-without-refresh-flow.

Cause 4 of 9: CreateUserPool demands SMS configuration whenever MFA is enabled.
AWS: a pool may be created with MfaConfiguration OPTIONAL/ON without SmsConfiguration or phone_number auto-verification (software-token MFA alone is enough).
Emulator: refuses CreateUserPool with InvalidParameterException 'SMS configuration and Auto verification for phone_number are required when MFA is required/optional', so the pool under test cannot even be created.
Why: TOTP-only pools — the common case — cannot be provisioned.
Fix: the CreateUserPool MFA precondition.
Traces: cognito-idp.admin-initiate-auth-mfa-required-without-phone-rejected, cognito-idp.admin-set-user-settings-unknown-pool-rejected.

Cause 5 of 9: a remembered device's status is never recorded.
AWS: after ConfirmDevice/AdminUpdateDeviceStatus the device carries its remembered status, exposed as the dev:device_remembered_status attribute on (Admin)GetDevice, and AdminUpdateDeviceStatus under a username that does not own the key is refused.
Emulator: AdminGetDevice returns only ['device_name'] and the status reads None, so the refusals and the status transitions cannot be observed.
Why: device-remembering clients cannot tell whether a device is remembered, and ownership checks on the device key are unenforced.
Fix: device state (remembered status) in the ConfirmDevice/AdminUpdateDeviceStatus handlers and the (Admin)GetDevice serializer.
Traces: cognito-idp.admin-update-device-status-under-a-name-other-than-the-owner-rejected, cognito-idp.forget-device-while-admin-password-reset-pending-rejected, cognito-idp.update-device-status-deleted-user-rejected, cognito-idp.update-device-status-while-password-reset-required-rejected.

Cause 6 of 9: DEVICE_SRP_AUTH is never offered.
AWS: signing in with a remembered device key yields the DEVICE_SRP_AUTH challenge (and refuses the device continuation once device tracking is turned off).
Emulator: the sign-in returns no ChallengeName at all (None).
Why: the remembered-device flow is unreachable.
Fix: the device-tracking branch of the sign-in challenge selection.
Traces: cognito-idp.device-srp-answer-rejected-once-device-tracking-is-turned-off.

Cause 7 of 9: ListDevices is not gated on the pool's device-tracking configuration.
AWS: with device tracking off, ListDevices with an access token is refused (recorded behavior only: the recording shows a ClientError).
Emulator: answers HTTP 200 {'Devices': []}.
Why: callers see an empty list instead of the configuration error AWS raises.
Fix: device-configuration validation in the ListDevices handler.
Traces: cognito-idp.list-devices-refuses-access-token-until-pool-tracks-devices.

Cause 8 of 9: ConfirmDevice accepts a malformed password verifier.
AWS: a device password verifier whose high bit is set is refused (recorded behavior only: the recording shows a ClientError).
Emulator: answers HTTP 200 {'UserConfirmationNecessary': True}.
Why: invalid SRP material is stored as a usable device secret.
Fix: verifier validation in the ConfirmDevice handler.
Traces: cognito-idp.confirm-device-sign-bit-verifier-rejected.

Cause 9 of 9: an inactive/secondary replica region serves writes the primary owns.
AWS: user-pool writes (AdminUpdateUserAttributes, AdminDeleteUserAttributes, DeleteUserAttributes, AdminForgetDevice, AdminSetUserMFAPreference, AdminSetUserPassword, AdminRespondToAuthChallenge, GetClientToken) are refused in a region where the pool's replica is not active, while the primary accepts them.
Emulator: answers HTTP 200 in the replica region for all of them.
Why: a multi-region deployment cannot test or rely on the read-only nature of a replica; writes silently diverge between regions.
Fix: the replica/region check in the user-pool write path. Related open issue 20261125T015700Z-cognito-idp-the-multi-region-user-pool-replica.md covers the replica lifecycle only.
Traces: cognito-idp.admin-respond-to-auth-challenge-in-inactive-replica-region-rejected, cognito-idp.admin-update-user-attributes-in-secondary-replica-region-rejected, cognito-idp.delete-user-attributes-in-replica-region-rejected, cognito-idp.get-client-token-refused-in-inactive-replica-region, cognito-idp.replica-region-admin-delete-user-attributes-rejected, cognito-idp.replica-region-refuses-device-forget-and-mfa-preference-rejected, cognito-idp.replica-region-refuses-password-and-device-writes-the-primary-accepts.

## Evidence
Cause 1 — "USER_AUTH PASSWORD_SRP on a client without ALLOW_USER_AUTH: ('NotImplemented', 'operation InitiateAuth with AuthFlow=USER_AUTH and PREFERRED_CHALLENGE=PASSWORD_SRP is scaffolded but not implemented in this emulator'), want ('InvalidParameterException', 'USER_AUTH flow not enabled for this client')"; "CUSTOM_AUTH on a client without ALLOW_CUSTOM_AUTH answered ('NotImplemented', 'operation InitiateAuth with AuthFlow=CUSTOM_AUTH is scaffolded but not implemented in this emulator')".
Cause 2 — expected ('InvalidParameterException', 'Initiate Auth method not supported.'), got ('InvalidParameterException', 'ADMIN_USER_PASSWORD_AUTH is not a valid AuthFlow for InitiateAuth').
Cause 3 — AdminInitiateAuth raised InvalidParameterException 'REFRESH_TOKEN_AUTH flow not enabled for this client' after an UpdateUserPoolClient that omitted ExplicitAuthFlows.
Cause 4 — CreateUserPool raised InvalidParameterException 'SMS configuration and Auto verification for phone_number are required when MFA is required/optional'.
Cause 5 — "AdminGetDevice carried no dev:device_remembered_status attribute: ['device_name']"; "the remembered device reads back status None"; "an opt-in pool's newly confirmed device read None before any status update".
Cause 6 — "sign-in with a remembered device key gave None, not DEVICE_SRP_AUTH".
Cause 7 — "expected a refusal, the call succeeded: {'Devices': [], ... 'HTTPStatusCode': 200}".
Cause 8 — "the call was expected to be refused but returned {'UserConfirmationNecessary': True, ... 'HTTPStatusCode': 200}".
Cause 9 — "the call was expected to be refused but returned {'ResponseMetadata': {'HTTPStatusCode': 200 ...}}" in the replica region for AdminDeleteUserAttributes/DeleteUserAttributes/AdminUpdateUserAttributes/AdminForgetDevice/AdminSetUserMFAPreference; GetClientToken "succeeded with keys ['ClientAuthenticationResult']".

## Steps to Reproduce
1. Replay the check ids in trace_ids.
2. Cause 1: CreateUserPoolClient with ExplicitAuthFlows=['ALLOW_USER_PASSWORD_AUTH'] and call InitiateAuth with AuthFlow='CUSTOM_AUTH', then AuthFlow='USER_AUTH' with PreferredChallenge PASSWORD_SRP.
3. Cause 2: InitiateAuth with AuthFlow='ADMIN_USER_PASSWORD_AUTH'.
4. Cause 3: CreateUserPoolClient with flows, UpdateUserPoolClient without ExplicitAuthFlows, then AdminInitiateAuth REFRESH_TOKEN_AUTH.
5. Cause 4: CreateUserPool with MfaConfiguration='OPTIONAL' and no SmsConfiguration.
6. Cause 5/6/8: enable device tracking, sign in, ConfirmDevice, AdminUpdateDeviceStatus, then AdminGetDevice; sign in again with the device key.
7. Cause 7: with DeviceConfiguration absent, call ListDevices with an access token.
8. Cause 9: create a pool with a replica, then issue the writes above against the replica region's endpoint.
