Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cognito-idp__iam__kms__lambda__sqs__sts.deep-cognito-idp-1-real-code-forgot-password-policy-refusal-keeps-code-and-sessions
Issue-Key: cognito-identity-provider|ConfirmForgotPassword|InvalidPasswordException
Issue-Commit: 2ac91d382b67c33b3b1ad8ceed805222f48956bf

# cognito-idp ConfirmForgotPassword's InvalidPasswordException says "Password did not conform with policy" instead of AWS's "Password does not conform to policy"

> _Note: this issue was identified against the code base at commit `2ac91d382b67c33b3b1ad8ceed805222f48956bf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ConfirmForgotPassword

## Description
One cause.

Cause 1 of 1: ConfirmForgotPassword's password-policy refusal message uses the wrong fixed text.
AWS: ConfirmForgotPassword refuses a Password that violates the pool's PasswordPolicy with InvalidPasswordException (Smithy: ConfirmForgotPassword errors include InvalidPasswordException, client error; ConfirmForgotPasswordRequest.Password is required, PasswordType @length 0..256, pattern ^[\S]+$ — the model says nothing about the message text, so the wording below is recorded behavior only). The recorded refusal text for this operation is the fixed sentence "Password does not conform to policy: " followed by the reason clause selected from the pool's PasswordPolicy — here "Password not long enough" for a password shorter than MinimumLength. The fixed part is "Password does not conform to policy: "; the variable part is the trailing reason clause ("Password not long enough", "Password must have uppercase characters", etc.), chosen from the pool's configured policy and the submitted password, never from one recording's literal value.
Emulator: serves InvalidPasswordException with "Password did not conform with policy: Password not long enough" — the fixed prefix is the SignUp/AdminCreateUser variant ("did not conform with policy"), not the "does not conform to policy" form this operation uses. Note the resolved issue 20261029T182116Z-cognito-idp-input-validation-refusals-carry-the-wrong.md asserted the opposite mapping for ConfirmForgotPassword (it listed ConfirmForgotPassword under the "did not conform with policy" wording); this check shows that mapping is wrong for ConfirmForgotPassword, so the per-operation selection of the two prefixes needs correcting rather than the shared duplicated-word bug it already fixed. That issue is resolved and does not cover this failure.
Why: clients and tooling that match the refusal text AWS sends (surfacing it to end users during a password reset, or asserting on it) see a sentence AWS never sends for ConfirmForgotPassword, while the error class and the rest of the flow are correct. The rest of the check's semantics hold (the refused reset does not spend the confirmation code and sessions minted before it keep working), so the message is the only divergence.
Fix: the shared password-policy validation error construction in the cognito-idp crate, where the per-operation prefix is selected — ConfirmForgotPassword (and the other reset/confirm paths that share it) must render the "Password does not conform to policy: " prefix.
Traces: cognito-idp__iam__kms__lambda__sqs__sts.deep-cognito-idp-1-real-code-forgot-password-policy-refusal-keeps-code-and-sessions.

## Evidence
Verbatim from the replay of check cognito-idp__iam__kms__lambda__sqs__sts.deep-cognito-idp-1-real-code-forgot-password-policy-refusal-keeps-code-and-sessions:

AssertionError: expected ('InvalidPasswordException', 'Password does not conform to policy: Password not long enough'), got ('InvalidPasswordException', 'Password did not conform with policy: Password not long enough')

Error class (InvalidPasswordException) matches; only the fixed prefix of the message diverges ("does not conform to policy" vs "did not conform with policy"). The trailing reason clause ("Password not long enough") is derived from the pool's PasswordPolicy and the submitted password and is already correct.

## Steps to Reproduce
1. Create a user pool with a CustomEmailSender trigger (so a real reset code is issued) and the default PasswordPolicy (MinimumLength=8), plus an app client with USER_PASSWORD_AUTH.
2. SignUp and ConfirmSignUp a user with a conforming password.
3. ForgotPassword(ClientId, Username) and obtain the real reset code.
4. ConfirmForgotPassword(ClientId, Username, ConfirmationCode=<real code>, Password='short1').
5. Observe InvalidPasswordException with Message 'Password did not conform with policy: Password not long enough'; AWS answers 'Password does not conform to policy: Password not long enough'.
