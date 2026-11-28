Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: appsync__cognito-idp__lambda.iam-directive-field-rejects-lambda-and-user-pool-callers
Issue-Key: appsync__cognito-idp__lambda.iam-directive-field-rejects-lambda-and-user-pool-callers|_until#43|Timeout|answer = read()
Issue-Commit: cb20f68e94bbc6cb070c362820a8401b3373aae4

# appsync__cognito-idp__lambda: the @aws_iam directive-field check still hangs — a multi-provider GraphQL API never answers within the check budget (not covered by the open issue's replay key)

> _Note: this issue was identified against the code base at commit `cb20f68e94bbc6cb070c362820a8401b3373aae4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateGraphqlApi

## Description
Cause 1 of 1: an AppSync API created with additionalAuthenticationProviders for AWS_LAMBDA and AMAZON_COGNITO_USER_POOLS never reaches an answerable state, so the per-field auth-directive leg of this check never runs and the check ends on the suite's own 60s budget.

NOTE ON OVERLAP: the open issue 20261127T155934Z-appsync-cognito-idp-lambda-all-three-directive-authorization.md describes the same symptom and even lists this check in its Issue-Trace-Ids, but its stored replay key for this check (`appsync__cognito-idp__lambda.iam-directive-field-rejects-lambda-and-user-pool-callers|_until#43|Timeout|time.sleep(2)`) no longer matches the key this replay produced, so the suite did not mark this failure OWNED and it is filed here rather than dropped. A fixer should treat it together with that issue; it is the same subsystem.

AWS: Smithy (appsync) models CreateGraphqlApi with authenticationType: AuthenticationType (AWS_IAM | AMAZON_COGNITO_USER_POOLS | AWS_LAMBDA | API_KEY | OPENID_CONNECT) plus additionalAuthenticationProviders, returning HTTP 200 and a graphqlApi whose uris.GRAPHQL is a servable endpoint; StartSchemaCreation followed by GetSchemaCreationStatus (SchemaStatus: PROCESSING | ACTIVE | DELETING | FAILED | SUCCESS | NOT_APPLICABLE) must leave PROCESSING and settle on SUCCESS for a valid SDL document. On the data plane (not modeled; documented AppSync behavior) every GraphQL POST is answered promptly with HTTP 200 and a JSON body: a field restricted by @aws_iam is served to a SigV4 caller, and to a Lambda-authorizer caller or a Cognito user-pool caller it is nulled with exactly one error whose errorType is the fixed text `Unauthorized`, whose message is AppSync's fixed template `Not Authorized to access <fieldName> on type <typeName>` (the field and type names are taken from the request's selection set — here `ledger` and `Query`), and whose path is the field path. A @aws_lambda field is answered from the authorizer's resolverContext and a @aws_cognito_user_pools field from the pool user's identity.

Emulator: the check never gets a terminal answer out of this API — it is still inside a poll loop when the suite's 60s budget expires, and the only detail recorded is `Error: Check timed out after 60.0s`. No HTTP status, error code or body is produced for any of the three GraphQL requests, so neither the schema-creation settle nor the directive-authorization decision is observable. Hypothesis: the authorization path for an API carrying additionalAuthenticationProviders (the Lambda-authorizer invocation and/or the user-pool token validation it performs per field) blocks instead of answering, or GetSchemaCreationStatus never leaves PROCESSING/NOT_APPLICABLE for this API; the recording does not state which, because nothing ever answers.

Why: a client that creates a multi-provider GraphQL API and POSTs a query hangs. It gets no data, no denial and no error to retry on, so every per-field authorization rule (@aws_iam vs @aws_lambda vs @aws_cognito_user_pools) is unverifiable and any caller of such an API stalls indefinitely rather than being allowed or refused.

Fix: the appsync GraphQL data-plane request handler and its authorization evaluation for additionalAuthenticationProviders (AWS_LAMBDA authorizer invocation, AMAZON_COGNITO_USER_POOLS token validation) plus the schema-creation status state machine behind StartSchemaCreation/GetSchemaCreationStatus — each must always reach a terminal answer: HTTP 200 with data for an allowed field, HTTP 200 with data null and one `Unauthorized` error for a refused one, and SchemaStatus SUCCESS for a valid schema.

Traces: appsync__cognito-idp__lambda.iam-directive-field-rejects-lambda-and-user-pool-callers (the sibling checks lambda-directive-field-answers-authorizer-resolver-context and user-pools-directive-field-answers-pool-user-identity fail the same way and are already carried by the open issue named above).

## Evidence
Suite report, heading `### appsync__cognito-idp__lambda`: `#### [mismatch] appsync__cognito-idp__lambda.iam-directive-field-rejects-lambda-and-user-pool-callers` with detail `Error: Check timed out after 60.0s`, and the service line `appsync__cognito-idp__lambda 0/3 (0%) 3 failed`.

Expected (AWS): CreateGraphqlApi -> 200; GetSchemaCreationStatus -> SUCCESS within seconds; POST `query { ledger }` SigV4-signed -> 200 `{"data": {"ledger": "ledger-entry"}}`; POST the same query with the Lambda authorizer token or with the pool id token -> 200, `data` = `{"ledger": null}` and exactly one error `(errorType="Unauthorized", message="Not Authorized to access ledger on type Query", path=["ledger"])` — `Unauthorized` and `Not Authorized to access ... on type ...` are fixed text, `ledger`/`Query` are the field and type from the request; POST `query { badge }` -> 200 `{"data": {"badge": "<holder from the authorizer's resolverContext>"}}`; POST `query { member }` -> 200 `{"data": {"member": "<the pool user's username>"}}`.

Actual (emulator): no response is ever observed. The check is still polling when the 60s budget ends; status, error code and body are all absent.

## Steps to Reproduce
1. CreateRole + CreateFunction (python3.13) for a Lambda authorizer that returns {"isAuthorized": true, "resolverContext": {"holder": ...}} for a token starting `allow-`, and wait for the function to go Active.
2. CreateUserPool + CreateUserPoolClient (ALLOW_ADMIN_USER_PASSWORD_AUTH), AdminCreateUser, AdminSetUserPassword, AdminInitiateAuth to obtain an IdToken.
3. CreateGraphqlApi with authenticationType=AWS_IAM and additionalAuthenticationProviders=[{AWS_LAMBDA, lambdaAuthorizerConfig.authorizerUri=<fn arn>, authorizerResultTtlInSeconds=0}, {AMAZON_COGNITO_USER_POOLS, userPoolConfig={userPoolId, awsRegion}}]; AddPermission for principal appsync.amazonaws.com on the function.
4. StartSchemaCreation with `type Query { ledger: String @aws_iam  badge: String @aws_lambda  member: String @aws_cognito_user_pools }`, then poll GetSchemaCreationStatus until it leaves PROCESSING/NOT_APPLICABLE.
5. CreateDataSource type=NONE and a resolver per field.
6. POST `query { ledger }` to uris.GRAPHQL with SigV4 (service `appsync`), then POST it with `Authorization: allow-<holder>` and with `Authorization: <IdToken>`; also POST `query { badge }` and `query { member }`.
Observed: step 4/6 never settles — the check is killed at 60s with no response recorded. Re-replay check appsync__cognito-idp__lambda.iam-directive-field-rejects-lambda-and-user-pool-callers to confirm a fix.
