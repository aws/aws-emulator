Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: appsync__cognito-identity__dynamodb.deep-appsync-2-appsync-authz-caller-credentials-getitem-denied-item-unchanged
Issue-Key: appsync__cognito-identity__dynamodb.deep-appsync-2-appsync-authz-caller-credentials-getitem-denied-item-unchanged|_poll_until#29|Timeout|time.sleep(interval_s)
Issue-Commit: 36cbc41e18f8f7f03eaee998bf950ce282c69b6c

# appsync__cognito-identity__dynamodb: the AWS_IAM caller-credentials DynamoDB resolver flow never reaches a terminal state — the check exhausts its whole budget with no answer

> _Note: this issue was identified against the code base at commit `36cbc41e18f8f7f03eaee998bf950ce282c69b6c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GraphQL

## Description
Cause 1 of 1: the AppSync AWS_IAM / caller-credentials path over a DynamoDB data source (APPSYNC_JS resolvers) never settles — the check runs out its time budget instead of getting either the item or the denial.

AWS: recorded behavior only (no model statement covers liveness). Smithy (appsync): CreateResolver accepts `runtime` (APPSYNC_JS 1.0.0) plus `code`, and a DynamoDB data source resolver issuing `GetItem`/`UpdateItem` through `util.dynamodb.toMapValues` returns promptly: the signed GraphQL POST answers HTTP 200 within seconds, either with the item in `data` or with a GraphQL error carrying the DynamoDB/authorization errorType, and the item is left unchanged when the call is denied. Every prerequisite in the recording also settles quickly: CreateIdentityPool/SetIdentityPoolRoles, GetId, GetCredentialsForIdentity for the guest identity, StartSchemaCreation reaching SUCCESS, and CreateDataSource/CreateResolver.

Emulator: the check ends with "Check timed out after 60.0s" — no status, no body, no error code. One of the polled conditions (guest-credential vend via GetCredentialsForIdentity, schema creation reaching SUCCESS, or the signed GraphQL POST itself) never satisfies its predicate, so the check spins until the harness budget is gone. Hypothesis: the same data-plane execution gap seen in the sibling appsync__cognito-identity checks (a resolver that never produces a result) leaves the poll on a resolved field unsatisfiable; the emulator produced no diagnostic to narrow it further.

Why: a client gets a hang rather than an answer — no item, no authorization denial, no error it can branch on. Request timeouts are the worst failure mode for a caller: retries pile up, and an authorization regression (a denied GetItem that silently mutates the item) cannot be detected at all because the call never completes.

Fix: the AppSync GraphQL data-plane executor for DynamoDB-backed APPSYNC_JS resolvers, plus the control-plane state it polls (schema-creation status and the Cognito guest-credential vend used to sign the call): each must reach a terminal state and answer, and a denied resolver call must come back as a GraphQL error rather than never returning.

Traces: appsync__cognito-identity__dynamodb.deep-appsync-2-appsync-authz-caller-credentials-getitem-denied-item-unchanged. Already-open 20261110T155738Z-appsync-cognito-identity-dynamodb-the-published-graphql-endpoint.md concerns the published endpoint host and does not cover this: the check routes the signed POST through the harness's endpoint route and still never completes.

## Evidence
Expected (AWS): every step answers, and the denied GetItem comes back as HTTP 200 with a GraphQL `errors` entry (authorization errorType) while the stored item is unchanged — the whole flow completes in seconds.

Actual (emulator): `Error: Check timed out after 60.0s` — the check never obtained a status or body from any of its polled steps.

## Steps to Reproduce
1. CreateIdentityPool with AllowUnauthenticatedIdentities=true; create the unauthenticated role trusted by that pool and SetIdentityPoolRoles.
2. GetId then GetCredentialsForIdentity for the guest identity.
3. CreateGraphqlApi authenticationType=AWS_IAM, StartSchemaCreation, CreateDataSource type=AMAZON_DYNAMODB with caller-credentials, CreateResolver with runtime APPSYNC_JS 1.0.0 and GetItem/UpdateItem code.
4. SigV4-sign (signing name `appsync`) a POST of the GetItem query with the vended guest credentials.
5. Observe that the flow never reaches a terminal result inside the check budget (60s), instead of returning the item or a GraphQL authorization error.
