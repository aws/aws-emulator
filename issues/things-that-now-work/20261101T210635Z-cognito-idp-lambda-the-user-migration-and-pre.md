Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cognito-idp__lambda.user-migration-trigger-source-distinguishes-forgot-password-from-authentication cognito-idp__lambda.xsvc-cognito-idp-3-pretokengeneration-v2-triggersource-distinguishes-refresh-from-authentication
Issue-Key: cognito-idp__lambda.user-migration-trigger-source-distinguishes-forgot-password-from-authentication|_retry_on_lambda_warmup#15|Timeout|raise
Issue-Key: cognito-idp__lambda.xsvc-cognito-idp-3-pretokengeneration-v2-triggersource-distinguishes-refresh-from-authentication|_retry_on_lambda_warmup#16|Timeout|raise
Issue-Commit: 62e4c5cd3531cb5ea16934ee4e8b2e43c8dd65a3

# cognito-idp__lambda: the user-migration and pre-token-generation-v2 triggerSource distinctions are never observable — both sign-in flows hang to the harness budget

> _Note: this issue was identified against the code base at commit `62e4c5cd3531cb5ea16934ee4e8b2e43c8dd65a3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
InitiateAuth

## Description
Two distinct trigger-dispatch root causes in the Cognito user-pool -> Lambda trigger path. Both end in `Error: Check timed out after 60.0s`: the sign-in flow never produces a terminal answer, so the trigger payload can never be inspected.

Cause 1 of 2 — UserMigration triggerSource: AWS: recorded behavior only (the Smithy model carries LambdaConfig.UserMigration but no trigger payload semantics). AWS invokes the user-migration Lambda with triggerSource `UserMigration_Authentication` when the unknown user signs in through InitiateAuth, and with `UserMigration_ForgotPassword` when the same unknown user starts ForgotPassword — the two flows are distinguishable by that field alone, and each returns the migrated user plus tokens / the reset code path.
Emulator: neither flow completes; the check is killed at 60s without ever seeing a trigger event, so the two triggerSource values cannot be told apart.
Failing check: cognito-idp__lambda.user-migration-trigger-source-distinguishes-forgot-password-from-authentication.

Cause 2 of 2 — PreTokenGeneration V2 triggerSource: AWS: recorded behavior only. For a V2_0 pre-token-generation trigger AWS passes triggerSource `TokenGeneration_Authentication` on an initial sign-in and `TokenGeneration_RefreshTokens` when the same session is refreshed, and the claims the trigger adds/overrides appear in the issued access token for each case.
Emulator: the authentication and refresh calls never settle — `Error: Check timed out after 60.0s` — so no token is issued and no triggerSource is observable.
Failing check: cognito-idp__lambda.xsvc-cognito-idp-3-pretokengeneration-v2-triggersource-distinguishes-refresh-from-authentication.

Why: a client that branches on triggerSource (the documented way a single trigger Lambda serves several flows) cannot run at all: sign-in and refresh never return, so migration-on-first-sign-in and claim customisation are both unusable, and an application cannot distinguish "trigger not invoked" from "call still pending".

Fix: the InitiateAuth / RespondToAuthChallenge / ForgotPassword handlers and the user-pool Lambda-trigger dispatcher that must synchronously invoke the configured trigger with the correct triggerSource and then answer the caller. Related open issue 20261023T114649Z-cognito-idp-cognito-user-pool-lambda-triggers-are-not-invoked.md covers other trigger checks of this pool but does not carry these two.

Traces: the two check ids listed above.

## Evidence
Both failures report `Error: Check timed out after 60.0s` (replay verdicts [mismatch]).
- user-migration-trigger-source-distinguishes-forgot-password-from-authentication: expected the trigger event's triggerSource to be `UserMigration_Authentication` for the sign-in flow and `UserMigration_ForgotPassword` for the reset flow; actual: no flow completes within 60s.
- xsvc-cognito-idp-3-pretokengeneration-v2-triggersource-distinguishes-refresh-from-authentication: expected `TokenGeneration_Authentication` on initial sign-in and `TokenGeneration_RefreshTokens` on refresh, with the trigger's claim overrides visible in the access token; actual: no tokens issued, no verdict within 60s.

## Steps to Reproduce
1. Create a user pool with a UserMigration trigger and, separately, a V2_0 PreTokenGeneration trigger.
2. Sign in an unknown user via InitiateAuth (USER_PASSWORD_AUTH) and record the trigger event's triggerSource; then call ForgotPassword for the same unknown user and record it again.
3. For the pre-token trigger: sign in, then refresh with REFRESH_TOKEN_AUTH, and compare the triggerSource and the claims in the issued access tokens.
4. Against the emulator each of these calls fails to reach a terminal answer and the checks are killed at the 60s harness budget.
