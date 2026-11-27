Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cognito-idp__kms__lambda__sqs.confirm-sign-up-verified-email-alias-taken-rejected
Issue-Key: cognito-idp__kms__lambda__sqs.confirm-sign-up-verified-email-alias-taken-rejected|_code_for#30|AssertionError|raise AssertionError(f"no {source} code for the new user arrived; deliveries seen {seen!r}")
Issue-Commit: 8133ebe260b7cb1c7e11e2995dae089129b7fabf

# cognito-idp__kms__lambda__sqs: the CustomEmailSender_SignUp trigger is never invoked, so a confirm-sign-up alias-collision flow never gets a code

> _Note: this issue was identified against the code base at commit `8133ebe260b7cb1c7e11e2995dae089129b7fabf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SignUp

## Description
One root cause, grounded in a failing behavioral check. Two open issues describe the same trigger never producing a code for other flows (20261025T182751Z-cognito-idp-kms-lambda-sqs-the-email-otp.md and 20261029T175759Z-cognito-idp-kms-lambda-sqs-three-more-real.md); neither carries this check's replay key, so it is filed here and those issues are named for the fixer.

Cause 1 of 1: SignUp on a pool configured with a CustomEmailSender Lambda produces no CustomEmailSender_SignUp invocation, so no verification code is ever delivered.
  AWS: recorded behavior only. With LambdaConfig.CustomEmailSender set and a KMS key configured for the pool, SignUp invokes the trigger with triggerSource 'CustomEmailSender_SignUp' and the KMS-encrypted code, the handler forwards the delivery (here to SQS), and the code can then be used with ConfirmSignUp; a second sign-up that collides on a verified email alias is refused AliasExistsException.
  Emulator: the delivery wait ends with no invocation at all — 'no CustomEmailSender_SignUp code for the new user arrived; deliveries seen []' — so the flow never reaches the alias-collision assertion.
  Why: a client whose sign-up emails are sent by a CustomEmailSender Lambda gets no code, so no user can ever be confirmed through that pool, and the verified-alias rules that depend on confirmation are unobservable.
  Fix: the cognito-idp SignUp handler's trigger dispatch (CustomEmailSender invocation, with the code wrapped under the pool's KMS key).
  Traces: cognito-idp__kms__lambda__sqs.confirm-sign-up-verified-email-alias-taken-rejected.

## Evidence
cognito-idp__kms__lambda__sqs.confirm-sign-up-verified-email-alias-taken-rejected —
File "check_...py", line 329, in sign_up -> AssertionError: no CustomEmailSender_SignUp code for the new user arrived; deliveries seen [].
Expected: one CustomEmailSender_SignUp invocation carrying the encrypted code, then ConfirmSignUp succeeding and the colliding sign-up refused AliasExistsException.

## Steps to Reproduce
1. Create a symmetric CMK and a user pool with KeyConfiguration set to it, AutoVerifiedAttributes=['email'], AliasAttributes including email, and LambdaConfig.CustomEmailSender pointing at a Lambda that forwards deliveries to an SQS queue.
2. cognito-idp.sign_up(...) for a new user.
3. Poll the SQS queue for a CustomEmailSender_SignUp delivery.
4. Observe no delivery at all; AWS invokes the trigger and the code arrives.
