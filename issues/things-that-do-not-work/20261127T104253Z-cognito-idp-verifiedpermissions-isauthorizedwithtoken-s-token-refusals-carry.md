Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cognito-idp__verifiedpermissions.access-token-field-holding-an-id-token-rejected cognito-idp__verifiedpermissions.token-use-mismatch-between-token-slots-rejected cognito-idp__verifiedpermissions.expired-id-token-rejected-until-a-refreshed-token-is-presented
Issue-Key: cognito-idp__verifiedpermissions.access-token-field-holding-an-id-token-rejected|run#20|AssertionError|assert refused == ("ValidationException",
Issue-Key: cognito-idp__verifiedpermissions.token-use-mismatch-between-token-slots-rejected|run#32|AssertionError|assert swapped_id == (
Issue-Key: cognito-idp__verifiedpermissions.expired-id-token-rejected-until-a-refreshed-token-is-presented|run#19|AssertionError|assert expired == ("ValidationException",
Issue-Commit: 7f2f9e8fbacb5ccb6dc94ed688d25f78f8760a08

# cognito-idp__verifiedpermissions: IsAuthorizedWithToken's token refusals carry the wrong ValidationException message — a token in the wrong slot gets the generic "Failed to verify …" sentence instead of AWS's slot-mismatch sentence, and an expired token's reason text diverges (2 causes)

> _Note: this issue was identified against the code base at commit `7f2f9e8fbacb5ccb6dc94ed688d25f78f8760a08`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
IsAuthorizedWithToken

## Description
Two net-new root causes in how Verified Permissions words the modelled ValidationException it answers for a presented token it will not use. In both the HTTP status (400) and the error code (ValidationException) are right; the message text is not. The messages are produced centrally, in the verifiedpermissions crate's token-refusal lift, so both operations that take tokens (IsAuthorizedWithToken and BatchIsAuthorizedWithToken) answer them.

Cause 1 of 2: a token presented in the slot the OTHER token kind belongs in is refused with the service's generic token-verification sentence instead of AWS's fixed slot-mismatch sentence. Failing check ids for this cause: cognito-idp__verifiedpermissions.access-token-field-holding-an-id-token-rejected, cognito-idp__verifiedpermissions.token-use-mismatch-between-token-slots-rejected.
AWS: Smithy (verifiedpermissions model) IsAuthorizedWithToken$accessToken — "Must be an access token. Verified Permissions returns an error if the token_use claim in the submitted token isn't access"; IsAuthorizedWithToken$identityToken — "Must be an ID token. Verified Permissions returns an error if the token_use claim in the submitted token isn't id". The operation models ValidationException, errorKind client, HTTP 400 (httpErrorDefaulted). The sentence itself is recorded behavior only: AWS answers 400 ValidationException with the message "Expected AccessToken in the AccessToken parameter but received IdentityToken" when a token whose token_use is "id" arrives in accessToken, and the symmetric "Expected IdentityToken in the IdentityToken parameter but received AccessToken" when a token whose token_use is "access" arrives in identityToken. The whole sentence is FIXED text of the form "Expected <Slot> in the <Slot> parameter but received <OtherKind>": the only parts that vary with the request are which slot the token arrived in (AccessToken / IdentityToken) and which kind its token_use claim says it actually is (AccessToken / IdentityToken). No identifier, claim value, pool id or token substring from the request appears in it.
Emulator: the call is refused 400 ValidationException, but the message is the crate's generic verification wording — observed: 'Failed to verify AccessToken: its token_use claim is "id", not "access"'. That is the `Failed to verify <member>: <reason>` sentence the NotVerified arm of the token-refusal lift composes for ANY token that fails verification, so a slot/typing mistake is indistinguishable from a signature, issuer or clock failure. (Only the ID-token-as-accessToken direction was observed: the token-use-mismatch check asserts on that direction first and never reaches the access-token-as-identityToken leg, which the same code path words the same way.)
Why: a client that must tell "you passed the token in the wrong parameter — move it" (a caller bug, fixable by re-sending with the other member) apart from "this token did not verify" (a trust/identity-source/expiry problem, not fixable by re-sending) can only do so from the message, since both classes share status 400 and code ValidationException. Matching AWS's sentence fails against the emulator, so such a client takes the wrong recovery path, and conversely a client's regex for "Failed to verify" now matches a request error.
Fix: the token-kind (token_use) check in the verifiedpermissions crate's token verification — crate::state::token, whose TokenRefusal the lift in crate::ops (token_refusal / RefusedToken) turns into the modelled error — must carry a refusal class of its own for "token presented in the slot of the other kind", worded as AWS's fixed slot-mismatch sentence, rather than being folded into the NotVerified `Failed to verify <member>: <reason>` wording. Because both token operations lift the same description, fixing it there fixes both.
Traces: cognito-idp__verifiedpermissions.access-token-field-holding-an-id-token-rejected (assertion at check line 71, after IsAuthorizedWithToken with accessToken=<ID token>); cognito-idp__verifiedpermissions.token-use-mismatch-between-token-slots-rejected (assertion at check line 153, same call shape). File/line citation for the wording: services/service-com-amazonaws-verifiedpermissions-verifiedpermissions/src/ops/mod.rs lines 1278-1281 (NotVerified arm) and the doc contract in src/ops/is_authorized_with_token.rs lines 20-29.

Cause 2 of 2: the reason text of the ValidationException for a token whose exp has passed diverges from AWS's. Failing check id for this cause: cognito-idp__verifiedpermissions.expired-id-token-rejected-until-a-refreshed-token-is-presented.
AWS: Smithy documents that "Verified Permissions validates each token that is specified in a request by checking its expiration date and its signature" (IsAuthorizedWithToken documentation) and models ValidationException at HTTP 400. The sentence is recorded behavior only: a token presented after its exp is refused 400 ValidationException with the message "Failed to verify IdentityToken: Jwt decode internal error: ExpiredSignature". Fixed text: "Failed to verify ", then the member the token arrived in (IdentityToken or AccessToken — the only request-derived part), then ": Jwt decode internal error: ExpiredSignature". No timestamp, pool id or token value appears in it.
Emulator: the same call is refused 400 ValidationException with 'Failed to verify IdentityToken: the token is expired'. The prefix and the member are right; the reason clause is the crate's own prose instead of the "Jwt decode internal error: ExpiredSignature" reason AWS reports.
Why: the expiry reason is the one signal that tells a client to refresh the token and retry (as opposed to re-configuring the identity source or repairing the token). A client keying on AWS's reason clause sees an unrecognised reason and cannot classify the refusal, so a refresh-and-retry path that works against AWS stalls or surfaces the failure to the user against the emulator.
Fix: the expiry branch of crate::state::token in the verifiedpermissions crate — the reason string it attaches to the NotVerified refusal for a token past exp — must report AWS's reason clause; the surrounding `Failed to verify <member>: <reason>` composition in crate::ops (src/ops/mod.rs line 1280) already matches.
Traces: cognito-idp__verifiedpermissions.expired-id-token-rejected-until-a-refreshed-token-is-presented (assertion at check line 64, after waiting past the ID token's exp and calling IsAuthorizedWithToken with identityToken=<expired ID token>). The surrounding legs of that check — the pre-exp ALLOW and the REFRESH_TOKEN_AUTH re-issue being ALLOWed again — are not implicated.

Neither cause is covered by any open issue: the only open verifiedpermissions issue, 20261029T093530Z-verifiedpermissions-updatepolicy-validation.md, is about UpdatePolicy scope validation and carries neither of these checks' keys.

## Evidence
Check cognito-idp__verifiedpermissions.access-token-field-holding-an-id-token-rejected, mismatch:
```
File "check_cognito_idp__verifiedpermissions_access_token_field_holding_an_id_token_rejected.py", line 71, in run
  assert refused == ("ValidationException",
AssertionError: an ID token in the accessToken field got ('ValidationException', 'Failed to verify AccessToken: its token_use claim is "id", not "access"')
```
Expected: ('ValidationException', 'Expected AccessToken in the AccessToken parameter but received IdentityToken') at HTTP 400.
Actual: ('ValidationException', 'Failed to verify AccessToken: its token_use claim is "id", not "access"') at HTTP 400.

Check cognito-idp__verifiedpermissions.token-use-mismatch-between-token-slots-rejected, mismatch (same cause, independent scenario):
```
File "check_cognito_idp__verifiedpermissions_token_use_mismatch_between_token_slots_rejected.py", line 153, in run
  assert swapped_id == (
AssertionError: an ID token sent as accessToken answered ('ValidationException', 'Failed to verify AccessToken: its token_use claim is "id", not "access"')
```
Expected: ('ValidationException', 'Expected AccessToken in the AccessToken parameter but received IdentityToken'); the check's next leg (never reached) expects ('ValidationException', 'Expected IdentityToken in the IdentityToken parameter but received AccessToken') for an access token sent as identityToken.
Fixed vs variable: the whole sentence is fixed except the slot name and the received kind, both derived from the request (which member carried the token, and its token_use claim).

Check cognito-idp__verifiedpermissions.expired-id-token-rejected-until-a-refreshed-token-is-presented, mismatch:
```
File "check_cognito_idp__verifiedpermissions_expired_id_token_rejected_until_a_refreshed_token_is_presented.py", line 64, in run
  assert expired == ("ValidationException",
AssertionError: the expired ID token got ('ValidationException', 'Failed to verify IdentityToken: the token is expired')
```
Expected: ('ValidationException', 'Failed to verify IdentityToken: Jwt decode internal error: ExpiredSignature') at HTTP 400.
Actual: ('ValidationException', 'Failed to verify IdentityToken: the token is expired') at HTTP 400.
Fixed vs variable: 'Failed to verify ' + <member the token arrived in> + ': Jwt decode internal error: ExpiredSignature'; only the member name comes from the request.

## Steps to Reproduce
1. cognito-idp: CreateUserPool; CreateUserPoolClient with ExplicitAuthFlows ALLOW_ADMIN_USER_PASSWORD_AUTH and ALLOW_REFRESH_TOKEN_AUTH; AdminCreateUser(MessageAction='SUPPRESS'); AdminSetUserPassword(Permanent=True).
2. AdminInitiateAuth(AuthFlow='ADMIN_USER_PASSWORD_AUTH') and keep AuthenticationResult.IdToken / .AccessToken (token_use 'id' and 'access' respectively).
3. verifiedpermissions: CreatePolicyStore(validationSettings={'mode':'OFF'}); CreateIdentitySource(principalEntityType='Ledger::Member', configuration={'cognitoUserPoolConfiguration':{'userPoolArn':<pool arn>,'clientIds':[<client id>]}}); CreatePolicy with a static permit statement.
4. Cause 1: IsAuthorizedWithToken(policyStoreId=<store>, accessToken=<the ID token>, action=..., resource=...). Expected 400 ValidationException 'Expected AccessToken in the AccessToken parameter but received IdentityToken'; observed 400 ValidationException 'Failed to verify AccessToken: its token_use claim is "id", not "access"'. Symmetrically, IsAuthorizedWithToken(identityToken=<the access token>) must answer 'Expected IdentityToken in the IdentityToken parameter but received AccessToken'.
5. Cause 2: wait until the ID token's exp has passed (default pool ID-token lifetime), then IsAuthorizedWithToken(policyStoreId=<store>, identityToken=<that expired ID token>, ...). Expected 400 ValidationException 'Failed to verify IdentityToken: Jwt decode internal error: ExpiredSignature'; observed 400 ValidationException 'Failed to verify IdentityToken: the token is expired'.
