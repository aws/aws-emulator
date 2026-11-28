Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cognito-idp.change-password-token-of-deleted-user-pool-rejected cognito-idp.get-user-attribute-verification-code-after-pool-deleted-not-found cognito-idp.get-user-auth-factors-token-of-deleted-pool-rejected cognito-idp.list-devices-with-a-token-from-a-deleted-pool-rejected cognito-idp.set-user-mfa-preference-after-pool-deletion-rejected cognito-idp.verify-software-token-rejects-access-token-of-deleted-pool cognito-idp.verify-user-attribute-after-pool-deletion-rejected cognito-idp.delete-user-attributes-token-of-deleted-app-client-rejected cognito-idp.update-user-attributes-token-of-deleted-app-client-rejected cognito-idp.associate-software-token-deleted-app-client-token-not-found cognito-idp.revoke-token-under-another-client-id-rejected cognito-idp.access-token-after-username-reregistered-unconfirmed-rejected cognito-idp.change-password-stale-token-of-unconfirmed-namesake-rejected cognito-idp.get-user-auth-factors-token-after-unconfirmed-signup-rejected cognito-idp.update-user-attributes-rejects-a-token-whose-username-is-now-unconfirmed cognito-idp.verify-software-token-for-reregistered-unconfirmed-user-rejected cognito-idp.set-user-settings-after-user-deletion-rejected cognito-idp.access-token-of-a-disabled-user-rejected-by-get-user cognito-idp.disabled-user-access-token-stays-revoked-after-re-enable-rejected cognito-idp.admin-respond-to-auth-challenge-for-deleted-app-client-not-found cognito-idp.respond-to-auth-challenge-rejects-a-session-whose-app-client-was-deleted cognito-idp.refresh-token-of-deleted-user-rejected-by-get-tokens-from-refresh-token cognito-idp.refresh-for-a-reset-required-user-rejected
Issue-Shape-Id: com.amazonaws.cognitoidentityprovider#AWSCognitoIdentityProviderService
Issue-Key: cognito-idp.change-password-token-of-deleted-user-pool-rejected|run#16|AssertionError|assert got == ("ResourceNotFoundException", f"User pool {pool_id} does not exist."), (
Issue-Key: cognito-idp.get-user-attribute-verification-code-after-pool-deleted-not-found|run#20|AssertionError|assert got == gone, f"GetUserAttributeVerificationCode with the deleted pool's token answered {got!r}"
Issue-Key: cognito-idp.get-user-auth-factors-token-of-deleted-pool-rejected|run#10|AssertionError|assert got == missing, (
Issue-Key: cognito-idp.list-devices-with-a-token-from-a-deleted-pool-rejected|run#19|AssertionError|assert got == expected, f"ListDevices with the deleted pool's token gave {got!r}"
Issue-Key: cognito-idp.set-user-mfa-preference-after-pool-deletion-rejected|run#19|AssertionError|assert got == missing, f"SetUserMFAPreference with the dead pool's token answered {got!r}"
Issue-Key: cognito-idp.verify-software-token-rejects-access-token-of-deleted-pool|run#21|AssertionError|assert got == ("ResourceNotFoundException", f"User pool {pool_id} does not exist."), (
Issue-Key: cognito-idp.verify-user-attribute-after-pool-deletion-rejected|run#16|AssertionError|assert after == expected, (
Issue-Key: cognito-idp.delete-user-attributes-token-of-deleted-app-client-rejected|_refusal#18|AssertionError|raise AssertionError(f"the call was expected to be refused but returned {answer!r}")
Issue-Key: cognito-idp.update-user-attributes-token-of-deleted-app-client-rejected|_refusal#19|AssertionError|raise AssertionError(f"expected a refusal, but the call succeeded: {result!r}")
Issue-Key: cognito-idp.associate-software-token-deleted-app-client-token-not-found|_refusal#15|AssertionError|raise AssertionError(f"the call was expected to be refused but returned keys {sorted(answer)!r}")
Issue-Key: cognito-idp.revoke-token-under-another-client-id-rejected|_refusal#12|AssertionError|raise AssertionError(f"expected a refusal, the call succeeded: {sorted(got)!r}")
Issue-Key: cognito-idp.access-token-after-username-reregistered-unconfirmed-rejected|run#15|AssertionError|assert got == ("UserNotConfirmedException", "User is not confirmed."), (
Issue-Key: cognito-idp.change-password-stale-token-of-unconfirmed-namesake-rejected|run#18|AssertionError|assert refused == ("UserNotConfirmedException", "User is not confirmed."), (
Issue-Key: cognito-idp.get-user-auth-factors-token-after-unconfirmed-signup-rejected|run#18|AssertionError|assert got == ("UserNotConfirmedException", "User is not confirmed."), (
Issue-Key: cognito-idp.update-user-attributes-rejects-a-token-whose-username-is-now-unconfirmed|run#19|AssertionError|assert got == _UNCONFIRMED, f"the old token on an unconfirmed username gave {got!r}"
Issue-Key: cognito-idp.verify-software-token-for-reregistered-unconfirmed-user-rejected|run#22|AssertionError|assert got == want, f"VerifySoftwareToken with the old token answered {got!r}, want {want!r}"
Issue-Key: cognito-idp.set-user-settings-after-user-deletion-rejected|run#22|AssertionError|assert stale == expected_stale, (
Issue-Key: cognito-idp.access-token-of-a-disabled-user-rejected-by-get-user|run#17|AssertionError|assert got == expected, f"GetUser with the disabled user's token answered {got!r}, expected {expected!r}"
Issue-Key: cognito-idp.disabled-user-access-token-stays-revoked-after-re-enable-rejected|run#17|AssertionError|assert got == disabled, f"GetUser on a disabled user's token: expected {disabled!r}, got {got!r}"
Issue-Key: cognito-idp.admin-respond-to-auth-challenge-for-deleted-app-client-not-found|run#17|AssertionError|assert got == ("ResourceNotFoundException", "Username/client id combination not found."), (
Issue-Key: cognito-idp.respond-to-auth-challenge-rejects-a-session-whose-app-client-was-deleted|run#21|AssertionError|assert (code, message) == expected, f"expected {expected!r}, got {(code, message)!r}"
Issue-Key: cognito-idp.refresh-token-of-deleted-user-rejected-by-get-tokens-from-refresh-token|run#16|AssertionError|assert got == ("NotAuthorizedException",
Issue-Key: cognito-idp.refresh-for-a-reset-required-user-rejected|run#18|AssertionError|assert refused == expected, (
Issue-Commit: 7eb46ae4754f02cb00b49bce755f449d67bad34e

# cognito-idp: access/refresh-token refusals are misclassified after the pool, app client or user behind the token is gone (9 causes)

> _Note: this issue was identified against the code base at commit `7eb46ae4754f02cb00b49bce755f449d67bad34e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ChangePassword

## Description
Nine defects in how the token-authenticated (data-plane) operations classify a token whose pool, app client or user no longer matches the state behind it. Each is a 400-class refusal whose error code and message are the requirement.

Cause 1 of 9: a token of a DELETED user pool answers NotAuthorizedException 'Invalid Access Token' instead of ResourceNotFoundException naming the pool.
AWS: the operation resolves the pool from the token's iss and refuses with ResourceNotFoundException and the message "User pool <pool id> does not exist." — fixed text with the pool id from the token.
Emulator: answers NotAuthorizedException 'Invalid Access Token' for ChangePassword, GetUserAttributeVerificationCode, GetUserAuthFactors, ListDevices, SetUserMFAPreference, VerifySoftwareToken and VerifyUserAttribute.
Why: a client cannot distinguish a deleted pool (fatal, do not retry, re-provision) from a bad token (re-authenticate).
Fix: the token-resolution step shared by the access-token operations must refuse a missing pool as ResourceNotFoundException.
Traces: cognito-idp.change-password-token-of-deleted-user-pool-rejected, cognito-idp.get-user-attribute-verification-code-after-pool-deleted-not-found, cognito-idp.get-user-auth-factors-token-of-deleted-pool-rejected, cognito-idp.list-devices-with-a-token-from-a-deleted-pool-rejected, cognito-idp.set-user-mfa-preference-after-pool-deletion-rejected, cognito-idp.verify-software-token-rejects-access-token-of-deleted-pool, cognito-idp.verify-user-attribute-after-pool-deletion-rejected.

Cause 2 of 9: a token whose APP CLIENT was deleted is still honoured.
AWS: once the client_id in the token no longer exists the call is refused (recorded behavior only: each recording shows a ClientError, never a 2xx).
Emulator: DeleteUserAttributes and UpdateUserAttributes answer HTTP 200 with an empty body and mutate the user; AssociateSoftwareToken answers 200 with a fresh SecretCode.
Why: tokens survive the revocation boundary AWS enforces — deleting an app client does not stop its tokens from writing.
Fix: validate the token's client_id against live app clients in the access-token authorisation path.
Traces: cognito-idp.delete-user-attributes-token-of-deleted-app-client-rejected, cognito-idp.update-user-attributes-token-of-deleted-app-client-rejected, cognito-idp.associate-software-token-deleted-app-client-token-not-found.

Cause 3 of 9: RevokeToken accepts a refresh token presented under a different app client.
AWS: refuses the call when the ClientId does not match the client the refresh token was issued to (recorded behavior only: the recording shows a refusal).
Emulator: answers HTTP 200 ({'ResponseMetadata': ...} only), revoking another client's token.
Why: any client in the account can revoke another client's sessions.
Fix: the RevokeToken handler must compare the token's client_id with the request's ClientId.
Traces: cognito-idp.revoke-token-under-another-client-id-rejected.

Cause 4 of 9: a token for a username that was deleted and signed up again answers UserNotFoundException instead of UserNotConfirmedException.
AWS: the username resolves to the new UNCONFIRMED user, so the call is refused UserNotConfirmedException 'User is not confirmed.' and the user is left untouched; after AdminConfirmSignUp the same operation succeeds with a fresh token.
Emulator: answers UserNotFoundException 'User does not exist.' for DeleteUserAttributes, ChangePassword, GetUserAuthFactors, UpdateUserAttributes and VerifySoftwareToken.
Why: a client sees 'user gone' and stops, where AWS tells it the user merely needs confirmation.
Fix: resolve the username in the pool before judging the token, and classify an UNCONFIRMED user as UserNotConfirmedException.
Traces: cognito-idp.access-token-after-username-reregistered-unconfirmed-rejected, cognito-idp.change-password-stale-token-of-unconfirmed-namesake-rejected, cognito-idp.get-user-auth-factors-token-after-unconfirmed-signup-rejected, cognito-idp.update-user-attributes-rejects-a-token-whose-username-is-now-unconfirmed, cognito-idp.verify-software-token-for-reregistered-unconfirmed-user-rejected.

Cause 5 of 9: SetUserSettings with a deleted user's token against a re-created namesake.
AWS: refuses NotAuthorizedException 'Invalid access token.' — the token itself is no longer valid for the new user.
Emulator: answers UserNotFoundException 'User does not exist.'
Why: wrong class (authorisation vs. missing resource) for the same state.
Fix: the SetUserSettings token check.
Traces: cognito-idp.set-user-settings-after-user-deletion-rejected.

Cause 6 of 9: a DISABLED user's access token is reported as revoked.
AWS: refuses NotAuthorizedException 'User is disabled.' while the user is disabled; after AdminEnableUser the same token is accepted again.
Emulator: answers NotAuthorizedException 'Access Token has been revoked' and keeps treating the token as revoked after the user is re-enabled.
Why: the message tells the client to re-authenticate when the real fix is to enable the user, and re-enabling does not restore the session as it does on AWS.
Fix: the disabled-user check in the access-token authorisation path must not mark the token revoked.
Traces: cognito-idp.access-token-of-a-disabled-user-rejected-by-get-user, cognito-idp.disabled-user-access-token-stays-revoked-after-re-enable-rejected.

Cause 7 of 9: a challenge session whose app client was deleted names the client instead of the pair.
AWS: (Admin)RespondToAuthChallenge refuses ResourceNotFoundException 'Username/client id combination not found.'
Emulator: answers ResourceNotFoundException 'User pool client <client id> does not exist.'
Why: message divergence on a modelled error; AWS deliberately does not disclose which half is missing.
Fix: the challenge-session lookup in the (Admin)RespondToAuthChallenge handlers.
Traces: cognito-idp.admin-respond-to-auth-challenge-for-deleted-app-client-not-found, cognito-idp.respond-to-auth-challenge-rejects-a-session-whose-app-client-was-deleted.

Cause 8 of 9: GetTokensFromRefreshToken for a deleted user.
AWS: refuses NotAuthorizedException 'The user has been deleted for the associated refresh token'.
Emulator: answers NotAuthorizedException 'Invalid Refresh Token.'
Why: the caller cannot tell a deleted user from a malformed token.
Fix: the GetTokensFromRefreshToken handler's user lookup.
Traces: cognito-idp.refresh-token-of-deleted-user-rejected-by-get-tokens-from-refresh-token.

Cause 9 of 9: refreshing for a RESET_REQUIRED user uses the wrong error class.
AWS: refuses InvalidParameterException 'Password reset required for the user'.
Emulator: answers NotAuthorizedException with the same message.
Why: clients branch on the code, not the text, so the reset flow is never entered.
Fix: the refresh-token auth path's user-status classification.
Traces: cognito-idp.refresh-for-a-reset-required-user-rejected.

## Evidence
Cause 1 — GetUserAuthFactors with a deleted pool's token: expected ('ResourceNotFoundException', 'User pool <pool id> does not exist.'), got ('NotAuthorizedException', 'Invalid Access Token'). Identical substitution on VerifyUserAttribute, ChangePassword, ListDevices, SetUserMFAPreference, VerifySoftwareToken, GetUserAttributeVerificationCode.
Cause 2 — DeleteUserAttributes/UpdateUserAttributes with a deleted app client's token: "the call was expected to be refused but returned {'ResponseMetadata': {'HTTPStatusCode': 200 ...}}"; AssociateSoftwareToken returned keys ['ResponseMetadata', 'SecretCode'].
Cause 3 — RevokeToken under another ClientId: "expected a refusal, the call succeeded: ['ResponseMetadata']".
Cause 4 — DeleteUserAttributes with the pre-delete token: expected ('UserNotConfirmedException', 'User is not confirmed.'), got ('UserNotFoundException', 'User does not exist.'). VerifySoftwareToken: got ('UserNotFoundException','User does not exist.',400), want ('UserNotConfirmedException','User is not confirmed.',400).
Cause 5 — SetUserSettings: expected ('NotAuthorizedException', 'Invalid access token.'), got ('UserNotFoundException', 'User does not exist.').
Cause 6 — GetUser with a disabled user's token: expected ('NotAuthorizedException', 'User is disabled.'), got ('NotAuthorizedException', 'Access Token has been revoked').
Cause 7 — RespondToAuthChallenge after DeleteUserPoolClient: expected ('ResourceNotFoundException', 'Username/client id combination not found.'), got ('ResourceNotFoundException', 'User pool client c415830ab8374e4dae47c48ef5 does not exist.').
Cause 8 — GetTokensFromRefreshToken: expected ('NotAuthorizedException', 'The user has been deleted for the associated refresh token'), got ('NotAuthorizedException', 'Invalid Refresh Token.').
Cause 9 — refresh for a RESET_REQUIRED user: expected ('InvalidParameterException', 'Password reset required for the user'), got ('NotAuthorizedException', 'Password reset required for the user').

## Steps to Reproduce
1. Replay the check ids in trace_ids.
2. Cause 1: create a pool+client, sign in, DeleteUserPool, then call ChangePassword/ListDevices/VerifyUserAttribute with the access token.
3. Cause 2: sign in, DeleteUserPoolClient, then call DeleteUserAttributes / UpdateUserAttributes / AssociateSoftwareToken with that token.
4. Cause 3: sign in under client A, create client B, call RevokeToken(Token=<A's refresh token>, ClientId=<B>).
5. Cause 4: sign up + confirm + sign in, AdminDeleteUser, SignUp the same username again, then call DeleteUserAttributes with the first token.
6. Cause 6: sign in, AdminDisableUser, call GetUser with the token; AdminEnableUser and call GetUser again.
7. Cause 7: start a NEW_PASSWORD_REQUIRED challenge, DeleteUserPoolClient, then RespondToAuthChallenge with the session.
8. Cause 8: sign in, AdminDeleteUser, call GetTokensFromRefreshToken.
9. Cause 9: AdminResetUserPassword, then refresh with the user's refresh token.
