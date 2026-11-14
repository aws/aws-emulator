Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cognito-idp.admin-initiate-and-respond-to-auth-challenge-with-event-history cognito-idp.admin-respond-to-auth-challenge-refuses-a-replayed-totp-without-spending-the-session cognito-idp.admin-set-user-mfa-preference-persists-across-relogin cognito-idp.auth-session-validity-expires-the-challenge-session-while-a-longer-lived-twin-survives cognito-idp.software-token-mfa-sign-in-withholds-tokens-until-the-real-totp-is-supplied cognito-idp.e2e-mfa-enrollment-then-enforced-login-requires-code cognito-idp.e2e-mfa-challenge-wrong-totp-rejected cognito-idp.e2e-signup-confirm-mfa-login-teardown cognito-idp.e2e-signup-mfa-enforced-then-delete-blocks-login cognito-idp.e2e-device-remembered-skips-mfa-challenge cognito-idp.e2e-device-remember-skips-mfa-then-forget-reinstates cognito-idp.e2e-remembered-device-skips-mfa-on-next-login cognito-idp.device-srp-auth-completes-with-a-locally-computed-device-verifier cognito-idp.verify-software-token-wrong-code cognito-idp.software-token-code-window cognito-idp.code-mismatch-vs-expired-precision cognito-idp.associate-software-token-no-credentials-rejected cognito-idp.mfa-setup-challenge-flow-software-token cognito-idp.user-pool-replica-lifecycle cognito-idp.user-pool-replica-update-lifecycle
Issue-Shape-Id: com.amazonaws.cognitoidentityprovider#AWSCognitoIdentityProviderService
Issue-Key: cognito-idp.admin-initiate-and-respond-to-auth-challenge-with-event-history|run#9|AssertionError|assert resp.get("ChallengeName") == "SOFTWARE_TOKEN_MFA", resp
Issue-Key: cognito-idp.admin-respond-to-auth-challenge-refuses-a-replayed-totp-without-spending-the-session|run#20|AssertionError|assert first.get("ChallengeName") == "SOFTWARE_TOKEN_MFA", f"expected SOFTWARE_TOKEN_MFA, got {first.get('ChallengeName')!r}"
Issue-Key: cognito-idp.admin-set-user-mfa-preference-persists-across-relogin|run#18|AssertionError|assert auth2.get("ChallengeName") == "SOFTWARE_TOKEN_MFA", (
Issue-Key: cognito-idp.auth-session-validity-expires-the-challenge-session-while-a-longer-lived-twin-survives|_open_session#20|AssertionError|assert init.get("ChallengeName") == "SOFTWARE_TOKEN_MFA", (
Issue-Key: cognito-idp.software-token-mfa-sign-in-withholds-tokens-until-the-real-totp-is-supplied|run#20|KeyError|assert init["ChallengeName"] == "SOFTWARE_TOKEN_MFA", (
Issue-Key: cognito-idp.e2e-mfa-enrollment-then-enforced-login-requires-code|run#20|KeyError|assert challenge["ChallengeName"] == "SOFTWARE_TOKEN_MFA", (
Issue-Key: cognito-idp.e2e-mfa-challenge-wrong-totp-rejected|run#23|AssertionError|assert login2.get("ChallengeName") == "SOFTWARE_TOKEN_MFA"
Issue-Key: cognito-idp.e2e-signup-confirm-mfa-login-teardown|run#27|AssertionError|assert login2.get("ChallengeName") == "SOFTWARE_TOKEN_MFA", (
Issue-Key: cognito-idp.e2e-signup-mfa-enforced-then-delete-blocks-login|run#22|AssertionError|assert challenge == "SOFTWARE_TOKEN_MFA", (
Issue-Key: cognito-idp.e2e-device-remembered-skips-mfa-challenge|run#17|AssertionError|assert challenge1.get("ChallengeName") == "SOFTWARE_TOKEN_MFA"
Issue-Key: cognito-idp.e2e-device-remember-skips-mfa-then-forget-reinstates|run#19|KeyError|assert challenge["ChallengeName"] == "SOFTWARE_TOKEN_MFA"
Issue-Key: cognito-idp.e2e-remembered-device-skips-mfa-on-next-login|run#18|KeyError|assert challenge1["ChallengeName"] == "SOFTWARE_TOKEN_MFA", (
Issue-Key: cognito-idp.device-srp-auth-completes-with-a-locally-computed-device-verifier|run#18|KeyError|assert challenge["ChallengeName"] == "SOFTWARE_TOKEN_MFA", (
Issue-Key: cognito-identity-provider|VerifySoftwareToken|EnableSoftwareTokenMFAException
Issue-Key: cognito-idp.software-token-code-window|run#17|AssertionError|assert stale_message == expected_stale_message, (
Issue-Key: cognito-idp.code-mismatch-vs-expired-precision|run#44|AssertionError|assert st_msg == "Code mismatch", (
Issue-Key: cognito-idp.associate-software-token-no-credentials-rejected|run#15|AssertionError|assert code == "NotAuthorizedException", (
Issue-Key: cognito-identity-provider|AdminRespondToAuthChallenge|NotImplemented
Issue-Key: cognito-idp.user-pool-replica-lifecycle|run#16|AssertionError|assert status == "INACTIVE", (
Issue-Key: cognito-idp.user-pool-replica-update-lifecycle|_poll_until#19|AssertionError|raise AssertionError(f"condition not met within {timeout_s}s, last value={value!r}")
Issue-Commit: 36cbc41e18f8f7f03eaee998bf950ce282c69b6c

# cognito-idp: enrolled TOTP MFA is never challenged — (Admin)InitiateAuth issues tokens instead of SOFTWARE_TOKEN_MFA — plus VerifySoftwareToken's code-mismatch message, AssociateSoftwareToken's no-credential refusal, the MFA_SETUP challenge response and the replica lifecycle (5 causes)

> _Note: this issue was identified against the code base at commit `36cbc41e18f8f7f03eaee998bf950ce282c69b6c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AdminInitiateAuth

## Description
Five net-new root causes in cognito-idp. All five must be closed before submitting. Cause 1 is an authentication bypass (a full token set is issued without the second factor), so the bundle is HIGH.

Cause 1 of 5: a user with an ENROLLED software token and a preferred TOTP setting is signed in without any challenge — (Admin)InitiateAuth returns AuthenticationResult instead of ChallengeName=SOFTWARE_TOKEN_MFA.
AWS: Smithy — AdminInitiateAuth/InitiateAuth output carries ChallengeName (ChallengeNameType, whose values include SOFTWARE_TOKEN_MFA), Session, ChallengeParameters and AuthenticationResult. Once AssociateSoftwareToken + VerifySoftwareToken have enrolled a TOTP factor and (Admin)SetUserMFAPreference marks SoftwareTokenMfaSettings Enabled/PreferredMfa (or the pool's MfaConfiguration is ON with software token as the only method), the password flows (ADMIN_USER_PASSWORD_AUTH / USER_PASSWORD_AUTH / USER_SRP_AUTH, and DEVICE_SRP_AUTH on an unremembered device) stop at a challenge: the response carries ChallengeName='SOFTWARE_TOKEN_MFA' and a Session, carries NO AuthenticationResult, and tokens are issued only after (Admin)RespondToAuthChallenge supplies the correct SOFTWARE_TOKEN_MFA_CODE (a wrong code is CodeMismatchException, a replayed one does not spend the session, and the preference survives across logins and is skipped only for a device confirmed+remembered via ConfirmDevice/UpdateDeviceStatus).
Emulator: the same call returns HTTP 200 with no ChallengeName member at all, ChallengeParameters {} and a complete AuthenticationResult (AccessToken, IdToken, RefreshToken, ExpiresIn 3600, TokenType Bearer) — the checks read ChallengeName as None or raise KeyError on it. The enrolled/preferred MFA state recorded by VerifySoftwareToken and (Admin)SetUserMFAPreference is simply not consulted when the challenge is selected.
Why: MFA is not enforced at all — anyone holding the password authenticates as the user even though the account requires a second factor, which is exactly the control the pool was configured for; and every downstream behaviour (wrong-code refusal, session spending/expiry, device-remembered skip, MFA-preference persistence) is unobservable because the challenge never happens.
Fix: the challenge-selection step of the (Admin)InitiateAuth handlers (services/service-com-amazonaws-cognitoidentityprovider-awscognitoidentityproviderservice/src/ops/admin_initiate_auth.rs and initiate_auth.rs) must read the user's enrolled/preferred MFA settings and the pool's MfaConfiguration before issuing tokens, emit ChallengeName=SOFTWARE_TOKEN_MFA with a Session and withhold AuthenticationResult, and the (Admin)RespondToAuthChallenge SOFTWARE_TOKEN_MFA branch must verify the TOTP code against the enrolled secret.
Traces: cognito-idp.admin-initiate-and-respond-to-auth-challenge-with-event-history, cognito-idp.admin-respond-to-auth-challenge-refuses-a-replayed-totp-without-spending-the-session, cognito-idp.admin-set-user-mfa-preference-persists-across-relogin, cognito-idp.auth-session-validity-expires-the-challenge-session-while-a-longer-lived-twin-survives, cognito-idp.software-token-mfa-sign-in-withholds-tokens-until-the-real-totp-is-supplied, cognito-idp.e2e-mfa-enrollment-then-enforced-login-requires-code, cognito-idp.e2e-mfa-challenge-wrong-totp-rejected, cognito-idp.e2e-signup-confirm-mfa-login-teardown, cognito-idp.e2e-signup-mfa-enforced-then-delete-blocks-login, cognito-idp.e2e-device-remembered-skips-mfa-challenge, cognito-idp.e2e-device-remember-skips-mfa-then-forget-reinstates, cognito-idp.e2e-remembered-device-skips-mfa-on-next-login, cognito-idp.device-srp-auth-completes-with-a-locally-computed-device-verifier. Open issue 20261029T182116Z-cognito-idp-mfa-challenge-state-machine-mfa-setup.md covers the ENROLLMENT-less case (MFA_SETUP never issued for a user with no factor); this cause is the enrolled-factor case and is not described there.

Cause 2 of 5: VerifySoftwareToken's wrong/stale-code refusal message has extra emulator-authored text.
AWS: Smithy — VerifySoftwareToken models EnableSoftwareTokenMFAException (HTTP 400, "thrown when there is a code mismatch and the service fails to configure the software token TOTP multi-factor authentication"). Recorded behavior for the message: the exact text is `Code mismatch` — the whole message is fixed text with no variable part (no code, user or pool value is interpolated), and the checks assert equality against it for both a plainly wrong code and a code from a stale time step.
Emulator: HTTP 400 EnableSoftwareTokenMFAException with message `Code mismatch and fail enable Software Token MFA` — the fixed prefix `Code mismatch` followed by emulator-authored text AWS does not send.
Why: a client that matches on the message (the only way to tell this refusal class from the expired-code class, which shares the 400/class in some flows) misclassifies it, and the suite's mismatch-vs-expired precision check cannot distinguish the two.
Fix: the refusal text in the VerifySoftwareToken handler's code-verification path.
Traces: cognito-idp.verify-software-token-wrong-code, cognito-idp.software-token-code-window, cognito-idp.code-mismatch-vs-expired-precision.

Cause 3 of 5: AssociateSoftwareToken with neither AccessToken nor Session is refused with the wrong error class.
AWS: Smithy — AssociateSoftwareToken's input members AccessToken and Session are both optional and the operation is optionalAuth; its documentation states "You can authorize an AssociateSoftwareToken request with either the user's access token, or a session string from a challenge response that you received from Amazon Cognito". Presenting neither is an authorization failure: NotAuthorizedException (modeled on this operation, HTTP 403), not a request-shape complaint.
Emulator: InvalidParameterException (HTTP 400).
Why: an SDK caller classifies a missing credential as a malformed request instead of an auth failure, so retry/re-authenticate logic keyed on NotAuthorizedException never fires.
Fix: the credential-resolution step of the AssociateSoftwareToken handler — raise NotAuthorizedException when neither AccessToken nor Session is supplied, before parameter validation.
Traces: cognito-idp.associate-software-token-no-credentials-rejected.

Cause 4 of 5: AdminRespondToAuthChallenge with ChallengeName=MFA_SETUP is an unimplemented branch answering NotImplemented.
AWS: Smithy — MFA_SETUP is a modeled ChallengeNameType value and AdminRespondToAuthChallenge's output is AuthenticationResult / ChallengeName / Session / ChallengeParameters; responding to MFA_SETUP with the session from AssociateSoftwareToken (and the verified software token) completes enrollment and authentication. NotImplemented is not a modeled error of this operation (its error set is AliasExists/CodeMismatch/ExpiredCode/InvalidParameter/NotAuthorized/MFAMethodNotFound/SoftwareTokenMFANotFound/... all 400/403/404).
Emulator: ClientError (NotImplemented) "operation AdminRespondToAuthChallenge with ChallengeName=MFA_SETUP is scaffolded but not implemented in this emulator".
Why: the first-sign-in TOTP enrollment flow cannot be completed through the admin API, and the error code is outside the operation's modeled set so no SDK retry/classification applies.
Fix: the MFA_SETUP branch of the AdminRespondToAuthChallenge handler (services/service-com-amazonaws-cognitoidentityprovider-awscognitoidentityproviderservice/src/ops/admin_respond_to_auth_challenge.rs).
Traces: cognito-idp.mfa-setup-challenge-flow-software-token. The open issue 20261029T182116Z-cognito-idp-mfa-challenge-state-machine-mfa-setup.md lists this check under its NEW_PASSWORD_REQUIRED/RespondToAuthChallenge cause; the failure has since moved to this distinct admin-side MFA_SETUP branch and the suite no longer binds it to that issue.

Cause 5 of 5: the multi-Region user-pool replica lifecycle still cannot be driven to completion — a freshly created replica never reads back INACTIVE and a deleted one is never removed from the listing.
AWS: Smithy — UserPoolReplicaType carries RegionName, Status (ReplicaStatusType: CREATING | ACTIVE | INACTIVE | DELETING), Role and UserPoolArn, and ListUserPoolReplicas returns those rows. A freshly created replica settles INACTIVE, UpdateUserPoolReplica moves it ACTIVE/INACTIVE, and DeleteUserPoolReplica removes it so the Region is no longer listed and UpdateUserPoolReplica on it answers ResourceNotFoundException "Replica not found in the requested region". Recorded behavior only for durations (AWS takes minutes, which is why the checks poll).
Emulator: in user-pool-replica-lifecycle the replica Region is listed once after CreateUserPoolReplica but the status read that follows yields no status at all (None) where INACTIVE is due; in user-pool-replica-update-lifecycle the CREATING/ACTIVE/INACTIVE legs are served but after DeleteUserPoolReplica the row is still Status='DELETING' and is never removed, with the check's remaining budget already exhausted (the poll was granted 0.0 s).
Why: multi-Region replication cannot be observed end to end — a client cannot see a replica become usable, nor see a delete actually take effect, even though every individual call answers 200.
Fix: the replica state machine and its listing (the replica status/span bookkeeping in src/state.rs read by src/ops/list_user_pool_replicas.rs and written by create_user_pool_replica.rs / delete_user_pool_replica.rs), together with the latency of the multi-Region-CMK prerequisite those legs depend on, which currently consumes the checks' budget.
Traces: cognito-idp.user-pool-replica-lifecycle, cognito-idp.user-pool-replica-update-lifecycle. These two reproduce with the identical assertions and replay keys recorded in the already-resolved issue 20261103T200951Z-cognito-idp-2-net-new-causes-getprovisionedlimit-refuses.md (its Cause 2): that fix did not change the observed behaviour, so the cause is still open and is carried here.

## Evidence
Cause 1 — verbatim from the replay:
  admin-initiate-and-respond-to-auth-challenge-with-event-history: AssertionError: {'ChallengeParameters': {}, 'AuthenticationResult': {'AccessToken': 'eyJ...', 'ExpiresIn': 3600, 'TokenType': 'Bearer', 'RefreshToken': 'eyJ...'}} — asserted `resp.get("ChallengeName") == "SOFTWARE_TOKEN_MFA"`
  admin-respond-to-auth-challenge-refuses-a-replayed-totp-without-spending-the-session: AssertionError: expected SOFTWARE_TOKEN_MFA, got None
  admin-set-user-mfa-preference-persists-across-relogin: AssertionError: expected the persisted MFA preference to force a SOFTWARE_TOKEN_MFA challenge, got ChallengeName=None
  auth-session-validity-expires-...-twin-survives: AssertionError: MFA ON must force the software-token challenge on the short client, got None
  e2e-signup-confirm-mfa-login-teardown: AssertionError: expected a SOFTWARE_TOKEN_MFA challenge now that MFA is enrolled and preferred, got None
  e2e-signup-mfa-enforced-then-delete-blocks-login: AssertionError: expected the enforced-MFA login to stop at a SOFTWARE_TOKEN_MFA challenge, got None
  software-token-mfa-sign-in-withholds-tokens-until-the-real-totp-is-supplied / e2e-mfa-enrollment-then-enforced-login-requires-code / e2e-remembered-device-skips-mfa-on-next-login / e2e-device-remember-skips-mfa-then-forget-reinstates / device-srp-auth-completes-with-a-locally-computed-device-verifier: KeyError: 'ChallengeName' (the response has no such member)
  e2e-mfa-challenge-wrong-totp-rejected / e2e-device-remembered-skips-mfa-challenge: bare AssertionError on `login.get("ChallengeName") == "SOFTWARE_TOKEN_MFA"`
Expected (AWS): HTTP 200 with ChallengeName='SOFTWARE_TOKEN_MFA', a Session, and NO AuthenticationResult; tokens only after RespondToAuthChallenge with the correct SOFTWARE_TOKEN_MFA_CODE.

Cause 2 — verbatim:
  verify-software-token-wrong-code: AssertionError: expected message 'Code mismatch', got 'Code mismatch and fail enable Software Token MFA'
  software-token-code-window: AssertionError: expected message 'Code mismatch', got 'Code mismatch and fail enable Software Token MFA'
  code-mismatch-vs-expired-precision: AssertionError: VerifySoftwareToken: expected the exact EnableSoftwareTokenMFAException message, got 'Code mismatch and fail enable Software Token MFA'
Expected: HTTP 400 EnableSoftwareTokenMFAException, message exactly 'Code mismatch' (entirely fixed text).

Cause 3 — verbatim:
  associate-software-token-no-credentials-rejected: AssertionError: expected NotAuthorizedException calling AssociateSoftwareToken with no AccessToken and no Session, got InvalidParameterException
Expected: HTTP 403 NotAuthorizedException. Actual: HTTP 400 InvalidParameterException.

Cause 4 — verbatim:
  mfa-setup-challenge-flow-software-token: botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the AdminRespondToAuthChallenge operation: operation AdminRespondToAuthChallenge with ChallengeName=MFA_SETUP is scaffolded but not implemented in this emulator
Expected: HTTP 200 with AuthenticationResult (or the next modeled challenge) — NotImplemented is not in this operation's error set.

Cause 5 — verbatim:
  user-pool-replica-lifecycle: AssertionError: expected a freshly created replica to settle INACTIVE, got None (raised after create_user_pool_replica succeeded and ListUserPoolReplicas had listed the Region once)
  user-pool-replica-update-lifecycle: AssertionError: condition not met within 0.0s, last value='DELETING' (the post-DeleteUserPoolReplica poll for the Region to disappear)
Expected: Status INACTIVE after creation, ACTIVE/INACTIVE after each UpdateUserPoolReplica, and no row for the Region after DeleteUserPoolReplica, with UpdateUserPoolReplica on the gone replica answering ResourceNotFoundException 'Replica not found in the requested region'.

## Steps to Reproduce
Cause 1: CreateUserPool (+ app client with ALLOW_ADMIN_USER_PASSWORD_AUTH) and a confirmed user with a permanent password; AssociateSoftwareToken + VerifySoftwareToken with a valid TOTP code to enroll the factor; AdminSetUserMFAPreference(SoftwareTokenMfaSettings={Enabled:true, PreferredMfa:true}) (and/or SetUserPoolMfaConfig MfaConfiguration=ON with SoftwareTokenMfaConfiguration); then AdminInitiateAuth(ADMIN_USER_PASSWORD_AUTH) with the correct password. Observed: no ChallengeName and a full AuthenticationResult. Expected: ChallengeName='SOFTWARE_TOKEN_MFA' + Session and no tokens until AdminRespondToAuthChallenge supplies SOFTWARE_TOKEN_MFA_CODE. Repeat with a wrong code (expect CodeMismatchException), with a replayed code (the session must survive), and with a ConfirmDevice-remembered device (the challenge must then be skipped).
Cause 2: VerifySoftwareToken with a wrong 6-digit code, and with a code from a stale 30 s step -> expect EnableSoftwareTokenMFAException with message exactly 'Code mismatch'.
Cause 3: Call AssociateSoftwareToken with neither AccessToken nor Session -> expect NotAuthorizedException (403).
Cause 4: With MfaConfiguration=ON and a user with no enrolled factor, AdminInitiateAuth to obtain the MFA_SETUP session, AssociateSoftwareToken with that Session, VerifySoftwareToken, then AdminRespondToAuthChallenge(ChallengeName='MFA_SETUP', Session=...) -> expect AuthenticationResult, not NotImplemented.
Cause 5: Create a multi-Region CMK and replicate it; CreateUserPool(UserPoolTier=PLUS, KeyConfiguration={KeyType:CUSTOMER_MANAGED_KEY, KmsKeyArn:<key>}); CreateUserPoolReplica(RegionName=<second region>) and poll ListUserPoolReplicas for Status INACTIVE; UpdateUserPoolReplica ACTIVE then INACTIVE; DeleteUserPoolReplica and poll until the Region is gone, then UpdateUserPoolReplica on it -> expect ResourceNotFoundException 'Replica not found in the requested region'. Time each leg: the lifecycle must be observable without the CMK prerequisite consuming the whole budget.
