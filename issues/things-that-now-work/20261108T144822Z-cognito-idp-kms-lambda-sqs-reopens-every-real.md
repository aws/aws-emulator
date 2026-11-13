Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cognito-idp__kms__lambda__sqs.deep-cognito-idp-1-real-code-email-otp-wrong-code-rejected cognito-idp__kms__lambda__sqs.deep-cognito-idp-1-real-code-forgot-password-code-reuse-rejected cognito-idp__kms__lambda__sqs.deep-cognito-idp-1-real-code-forgot-password-confirm cognito-idp__kms__lambda__sqs.deep-cognito-idp-1-real-code-forgot-password-wrong-code-rejected cognito-idp__kms__lambda__sqs.deep-cognito-idp-1-real-code-reissued-reset-code-invalidates-first-rejected cognito-idp__kms__lambda__sqs.deep-cognito-idp-1-real-code-resent-signup-code-invalidates-first-rejected cognito-idp__kms__lambda__sqs.deep-cognito-idp-1-real-code-signup-confirm cognito-idp__kms__lambda__sqs.deep-cognito-idp-1-real-code-signup-wrong-code-rejected cognito-idp__kms__lambda__sqs.deep-cognito-idp-1-real-code-verify-email-wrong-code-rejected cognito-idp__kms__lambda__sqs.deep-cognito-idp-1-real-code-verify-updated-email
Issue-Key: cognito-identity-provider|SignUp|UserLambdaValidationException
Issue-Commit: 5bdc422e571ff089197010dd8b23a6f4d1e72235

# cognito-idp__kms__lambda__sqs: REOPENS — every real-verification-code flow is now refused 400 UserLambdaValidationException: the nodejs runtime's bundled @aws-sdk/client-kms exports no DecryptCommand (1 cause)

> _Note: this issue was identified against the code base at commit `5bdc422e571ff089197010dd8b23a6f4d1e72235`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SignUp

## Description
REOPENS 20261025T022656Z-cognito-idp-kms-lambda-sqs-every-real-verification.md: its fix left the call refused.

Cause 1 of 1: the pool's CustomEmailSender trigger is now invoked, but the handler can no longer be loaded by the emulator's in-process Node.js Lambda runtime, and the resulting handler failure is surfaced as a refusal of the user-facing operation. All ten real-verification-code flows (sign-up confirm, forgot-password confirm, EMAIL_OTP, resend/reissue invalidation, verify-email and verify-updated-email) die on their very first SignUp. Failing checks: the ten cognito-idp__kms__lambda__sqs.deep-cognito-idp-1-real-code-* ids in trace_ids.

  AWS:      Smithy (services/service-com-amazonaws-cognitoidentityprovider-awscognitoidentityproviderservice): SignUp takes required ClientId + Username and answers 200 with required UserConfirmed + UserSub plus CodeDeliveryDetails; UserLambdaValidationException / UnexpectedLambdaException / InvalidLambdaResponseException (all HTTP 400) are modeled only for a configured trigger that itself fails. With LambdaConfig {CustomEmailSender {LambdaVersion "V1_0", LambdaArn}, KMSKeyID}, recorded behavior is: SignUp answers 200 and invokes the function with triggerSource "CustomEmailSender_SignUp", request.code being an AWS-Encryption-SDK message encrypted under the pool's KMSKeyID; the handler decrypts it with KMS Decrypt and the 6-digit plaintext is then accepted by ConfirmSignUp / ConfirmForgotPassword / RespondToAuthChallenge (200) and a wrong, reused or superseded code refused CodeMismatchException / ExpiredCodeException (HTTP 400). The handler in these flows is a nodejs22.x ESM module whose first line is `import { KMSClient, DecryptCommand } from "@aws-sdk/client-kms"`, which real AWS's runtime resolves and links.
  Emulator: every one of the ten flows is refused at SignUp with HTTP 400, Error.Code=UserLambdaValidationException, Error.Message="CustomEmailSender failed with error SyntaxError: Could not find export 'DecryptCommand' in module 'file:///var/runtime/node_modules/@aws-sdk/client-kms'." The fixed text of that message class is "CustomEmailSender failed with error " followed by the handler's own error; here the variable part is the emulator runtime's own ESM link failure. The module path resolves, so the bundled @aws-sdk/client-kms exists but is an incomplete shim exporting the client (KMSClient) and not the SDK's command classes, so a named import of DecryptCommand fails at module-link time, before any handler code runs.
  Why:      no account can complete a sign-up confirmation, a password reset or an EMAIL_OTP sign-in when the pool uses the documented custom-sender trigger: the operation that must answer 200 and deliver a code answers 400, and none of the code-refusal taxonomies (wrong code, reused code, reissued-code invalidation, session expiry) that protect those flows can be reached. The client is also told the fault is its own Lambda's validation.
  Fix:      the emulator's in-process Node.js Lambda runtime module surface — /var/runtime/node_modules/@aws-sdk/client-kms must export the client's command classes (DecryptCommand and siblings) and not only KMSClient — and, downstream, the cognito-idp CustomEmailSender dispatch shared by SignUp, ResendConfirmationCode, ForgotPassword, GetUserAttributeVerificationCode and the EMAIL_OTP challenge path, which must not report a runtime-internal link error as the pool owner's Lambda validation failure. Related open issues report the trigger as never invoked at all (20261025T015335Z-cognito-idp-kms-lambda-sqs-the-customemailsender-lambda.md, 20261025T182751Z-cognito-idp-kms-lambda-sqs-the-email-otp.md, 20261029T175759Z-cognito-idp-kms-lambda-sqs-three-more-real.md, 20261107T194121Z-cognito-idp-kms-lambda-sqs-a-user-pool.md) — none carries this refusal's replay key, and the incomplete-bundled-SDK shape is the same one reported for other clients in 20261107T205257Z-events-lambda-sns-sqs-the-lambda-runtime-s.md.
  Traces:   the ten cognito-idp__kms__lambda__sqs.deep-cognito-idp-1-real-code-* checks listed in trace_ids.

## Evidence
All ten checks — verdict [error], identical failure.

Actual (replay output, e.g. cognito-idp__kms__lambda__sqs.deep-cognito-idp-1-real-code-signup-confirm):
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
  botocore.errorfactory.UserLambdaValidationException: An error occurred (UserLambdaValidationException) when calling the SignUp operation: CustomEmailSender failed with error SyntaxError: Could not find export 'DecryptCommand' in module 'file:///var/runtime/node_modules/@aws-sdk/client-kms'.

Expected: SignUp answers HTTP 200 with UserConfirmed=false, UserSub and CodeDeliveryDetails, and invokes the nodejs22.x CustomEmailSender function (triggerSource "CustomEmailSender_SignUp") whose `import { KMSClient, DecryptCommand } from "@aws-sdk/client-kms"` links successfully; the decrypted 6-digit code is then accepted by ConfirmSignUp (200) and a wrong code refused HTTP 400 CodeMismatchException "Invalid verification code provided, please try again." (fixed text, no variable part).

Actual: HTTP 400 UserLambdaValidationException on SignUp for all ten flows, carrying the runtime's own SyntaxError about the missing DecryptCommand export; nothing downstream is reached. Function names, key ARNs, queue URLs, usernames and code digits are per-run values — the requirement is the 200 outcome and a runtime whose bundled @aws-sdk/client-kms exports the command classes.

## Steps to Reproduce
1. Create a symmetric KMS key whose policy lets cognito-idp.amazonaws.com Encrypt/CreateGrant, an SQS queue, and a Lambda with Runtime="nodejs22.x" whose index.mjs begins `import { KMSClient, DecryptCommand } from "@aws-sdk/client-kms";`, AWS-Encryption-SDK-decrypts event.request.code via kms.send(new DecryptCommand({...})) and forwards {username, triggerSource, code} to the queue; grant lambda:InvokeFunction to cognito-idp.amazonaws.com.
2. CreateUserPool with LambdaConfig = {CustomEmailSender: {LambdaVersion: "V1_0", LambdaArn: <fn arn>}, KMSKeyID: <key arn>}, AutoVerifiedAttributes=["email"], plus an app client with ALLOW_USER_AUTH/ALLOW_USER_PASSWORD_AUTH.
3. Call SignUp(ClientId=<client>, Username=<user>, Password=<pw>, UserAttributes=[{Name:"email",Value:<addr>}]).
   Expected: HTTP 200 with CodeDeliveryDetails and one trigger invocation delivering a 6-digit code.
   Observed: HTTP 400 UserLambdaValidationException — "CustomEmailSender failed with error SyntaxError: Could not find export 'DecryptCommand' in module 'file:///var/runtime/node_modules/@aws-sdk/client-kms'."
4. The same refusal appears for ForgotPassword / ResendConfirmationCode / the EMAIL_OTP InitiateAuth path, so no code-consuming leg (confirm, wrong-code, reuse, reissue-invalidation) can be driven.
