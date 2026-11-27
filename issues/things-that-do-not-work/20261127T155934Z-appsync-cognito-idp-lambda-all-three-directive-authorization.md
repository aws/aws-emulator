Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: appsync__cognito-idp__lambda.iam-directive-field-rejects-lambda-and-user-pool-callers appsync__cognito-idp__lambda.lambda-directive-field-answers-authorizer-resolver-context appsync__cognito-idp__lambda.user-pools-directive-field-answers-pool-user-identity
Issue-Key: appsync__cognito-idp__lambda.iam-directive-field-rejects-lambda-and-user-pool-callers|_until#43|Timeout|time.sleep(2)
Issue-Key: appsync__cognito-idp__lambda.lambda-directive-field-answers-authorizer-resolver-context|_wait#39|Timeout|time.sleep(2)
Issue-Key: appsync__cognito-idp__lambda.user-pools-directive-field-answers-pool-user-identity|_poll#34|Timeout|time.sleep(2)
Issue-Commit: 85a8ef40f44f446d2f7f13d4131def4b1abce9f7

# appsync__cognito-idp__lambda: all three directive-authorization checks hang — @aws_iam/@aws_lambda/@aws_cognito_user_pools field auth never answers within 60s

> _Note: this issue was identified against the code base at commit `85a8ef40f44f446d2f7f13d4131def4b1abce9f7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateGraphqlApi

## Description
Cause 1 of 1: per-field auth-directive requests never settle.
AWS: an API with additionalAuthenticationProviders for AWS_LAMBDA and AMAZON_COGNITO_USER_POOLS, whose schema marks fields with @aws_iam, @aws_lambda and @aws_cognito_user_pools, answers each GraphQL request promptly: a field directive-restricted to IAM refuses a Lambda-authorized and a user-pool-authorized caller (an error per denied field, data null), a @aws_lambda field answers with the authorizer's resolverContext, and a @aws_cognito_user_pools field answers the pool user's identity. Each of these is a handful of control-plane calls plus one or two GraphQL POSTs.
Emulator: all three checks end with "Check timed out after 60.0s" — no answer or no terminal state is reached, so none of the directive-authorization behaviour is exercised at all. Hypothesis: the multi-provider authorization path (or the Cognito/Lambda authorizer call it makes per field) blocks instead of answering.
Why: a client issuing a GraphQL request against an API with additional authentication providers hangs instead of receiving data or a denial — no timeout, no error, nothing to retry on.
Fix: the per-field auth-directive evaluation path for additionalAuthenticationProviders (AWS_LAMBDA and AMAZON_COGNITO_USER_POOLS) in the GraphQL data plane — it must always answer.
Traces: appsync__cognito-idp__lambda.iam-directive-field-rejects-lambda-and-user-pool-callers, appsync__cognito-idp__lambda.lambda-directive-field-answers-authorizer-resolver-context, appsync__cognito-idp__lambda.user-pools-directive-field-answers-pool-user-identity

## Evidence
All three checks under the appsync__cognito-idp__lambda heading report only "Error: Check timed out after 60.0s" (0/3 passed for the service). No HTTP status, error code or body was produced for any of them.

## Steps to Reproduce
Create a Cognito user pool and a Lambda authorizer function; CreateGraphqlApi with a primary auth mode plus additionalAuthenticationProviders for AWS_LAMBDA and AMAZON_COGNITO_USER_POOLS; StartSchemaCreation with fields marked @aws_iam, @aws_lambda and @aws_cognito_user_pools; then POST each field with a SigV4 caller, a Lambda-authorizer token and a user-pool id token. Each request hangs past 60s. Replay the three checks in trace_ids.
