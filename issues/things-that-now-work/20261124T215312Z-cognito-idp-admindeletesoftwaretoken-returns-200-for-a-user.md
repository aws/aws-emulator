Issue-Source: trace-sweep-divergence
Issue-Trace: cognito-idp-admin-managed-device-and-mfa-settings
Issue-Step: task#6
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cognito-idp-admin-managed-device-and-mfa-settings cognito-idp-admin-user-settings-and-software-token cognito-idp-auth-event-feedback-and-software-token cognito-idp-user-mfa-settings-and-refresh-token-exchange cognito-idp-user-settings-and-mfa
Issue-Shape-Id: com.amazonaws.cognitoidentityprovider#AWSCognitoIdentityProviderService
Issue-Key: cognito-idp|AdminDeleteSoftwareToken|4xx->2xx|$
Issue-Key: cognito-idp|AdminDeleteSoftwareToken|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: cognito-idp|AdminDeleteSoftwareToken|4xx->2xx|status
Issue-Commit: 6f44d1835fbd492fdd033592217b9af14d15284a

# cognito-idp: AdminDeleteSoftwareToken returns 200 for a user with no registered TOTP factor instead of ResourceNotFoundException

> _Note: this issue was identified against the code base at commit `6f44d1835fbd492fdd033592217b9af14d15284a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AdminDeleteSoftwareToken

## Description
Cause 1 of 1: AdminDeleteSoftwareToken is missing its "no software token registered" refusal path entirely — it reports unconditional success for a user who has never completed TOTP enrollment (no verified software token / no SOFTWARE_TOKEN_MFA factor on the user).

AWS: AdminDeleteSoftwareToken deletes a user's registered time-based one-time password (TOTP) factor (Smithy: AdminDeleteSoftwareToken, input UserPoolId + Username both @required, output AdminDeleteSoftwareTokenResponse is empty; modeled errors include ResourceNotFoundException, InvalidParameterException, UserNotFoundException, OperationNotEnabledException, UserNotConfirmedException, NotAuthorizedException, TooManyRequestsException, InternalErrorException). When the named user has no software token to delete, the operation does NOT succeed: it is refused as ResourceNotFoundException. The recorded wire response is HTTP 400 with body {"__type": "ResourceNotFoundException", "message": "Software token MFA has not been configured for this user."} and header x-amzn-errortype: "ResourceNotFoundException:" (the trailing colon is part of the recorded header value, which is how the awsJson1_1 Cognito endpoint emits it). Note the model annotates ResourceNotFoundException with @httpError 404, but the recorded wire status for this refusal is 400; the recording is authoritative for the status to serve. Message shape: the text "Software token MFA has not been configured for this user." is entirely FIXED text for this error class — it interpolates no part of the request or of emulator state (no user name, pool id or value appears in it), so it must be reproduced verbatim.

Emulator: the request is accepted and answered HTTP 200 with an empty JSON body ({}) and no x-amzn-errortype header at all. Replay detail: "status: expected 400, got 200 / $: empty_body (expected {'__type': 'ResourceNotFoundException', 'message': 'Software token MFA has not been configured for this user.'}, got {}) / $headers.x-amzn-errortype: missing (expected 'ResourceNotFoundException:', got None)". The same shape of divergence appears on five independent fixtures, so this is the operation's default behavior and not a state artifact of one recording.

Why: a client cannot distinguish "the TOTP factor was removed" from "there was nothing to remove". Admin tooling that deletes a lost TOTP device and then branches on the refusal (to tell an operator the user was never enrolled, or to skip straight to AssociateSoftwareToken) takes the success branch unconditionally; idempotency/convergence checks that assert the second delete fails never see a failure; and SDK error handlers for ResourceNotFoundException on this operation are dead code against the emulator. Because no x-amzn-errortype header is emitted either, clients that classify errors from the header see no error even if the body were corrected.

Fix: AdminDeleteSoftwareToken's handler and its validation in the cognito-idp service crate. Before mutating, look up the target user's software-token/MFA state and, when no verified software token exists for that user, return ResourceNotFoundException with HTTP status 400, the awsJson1_1 error body {"__type":"ResourceNotFoundException","message":"Software token MFA has not been configured for this user."} and the x-amzn-errortype response header, through the service's shared awsJson error serializer so the header is populated the way the other refusals on this service are. The success path should clear the user's software token and its SOFTWARE_TOKEN_MFA entry in state so a following delete also refuses.

Related open issues do NOT cover this: 20261121T105536Z-cognito-idp-setusermfapreference-uses-the-pool-s-no.md is about the wording SetUserMFAPreference uses for an unverified software token, and 20261102T111936Z-cognito-idp-response-payloads-diverge-read-paths-omit.md is about omitted read-path fields; neither adds the missing AdminDeleteSoftwareToken refusal.

Traces: cognito-idp-admin-managed-device-and-mfa-settings#task#6 (representative), cognito-idp-admin-user-settings-and-software-token#task#7, cognito-idp-auth-event-feedback-and-software-token#task#7, cognito-idp-user-mfa-settings-and-refresh-token-exchange#task#10, cognito-idp-user-settings-and-mfa#task#10.

## Evidence
Expected (recorded AWS): HTTP 400; body {"__type": "ResourceNotFoundException", "message": "Software token MFA has not been configured for this user."}; header x-amzn-errortype: "ResourceNotFoundException:".
Actual (emulator): HTTP 200; body {} (empty success payload); x-amzn-errortype header absent.
Replay cluster key: service=cognito-idp|AdminDeleteSoftwareToken|status=400->200|keys=$,$headers.x-amzn-errortype,status — verdict mismatch, 5 affected steps across 5 fixtures.
Smithy: AdminDeleteSoftwareToken declares ResourceNotFoundException (client error, member `message`:MessageType) among its errors; output shape is empty, so a 200 carries no signal a client could use instead.

## Steps to Reproduce
1. Create a user pool and a confirmed user, and do NOT complete TOTP enrollment for that user (no AssociateSoftwareToken/VerifySoftwareToken, so the user has no verified software token).
2. Call AdminDeleteSoftwareToken with that pool's UserPoolId and the user's Username (awsJson1_1 target AWSCognitoIdentityProviderService.AdminDeleteSoftwareToken).
3. Observe HTTP 200 with an empty body and no x-amzn-errortype header, instead of HTTP 400 ResourceNotFoundException with message "Software token MFA has not been configured for this user.".
4. Equivalent replay: re-run fixture cognito-idp-admin-managed-device-and-mfa-settings step task#6 (also cognito-idp-admin-user-settings-and-software-token#task#7, cognito-idp-auth-event-feedback-and-software-token#task#7, cognito-idp-user-mfa-settings-and-refresh-token-exchange#task#10, cognito-idp-user-settings-and-mfa#task#10).
