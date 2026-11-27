Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: appsync.deploy-appsync-2-cfn-update-replace-rollback-rejected appsync.graphql-query-validation-rejected-before-execution
Issue-Shape-Id: com.amazonaws.appsync#AWSDeepdishControlPlaneService
Issue-Key: appsync.deploy-appsync-2-cfn-update-replace-rollback-rejected|run#68|AssertionError|assert status_a == "UPDATE_ROLLBACK_COMPLETE", (
Issue-Key: appsync.graphql-query-validation-rejected-before-execution|run#13|AssertionError|assert "data" in body, (
Issue-Commit: dd2e1ed8a15dea5ebae0faa8e0eddff98d20301d

# appsync: 2 net-new causes — a Resolver on a field absent from the active schema is accepted (so a CloudFormation update never rolls back), and a GraphQL validation failure omits the `data` key from the response envelope

> _Note: this issue was identified against the code base at commit `dd2e1ed8a15dea5ebae0faa8e0eddff98d20301d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateResolver

## Description
Two distinct, net-new AppSync defects surfaced by the behavioral suite. Both are in the `appsync` service and must both be closed before this issue is submitted.

Cause 1 of 2: CreateResolver accepts a resolver for a type/field that the API's active schema does not declare, so a CloudFormation stack update that adds such a resolver succeeds instead of failing that resource and rolling the stack back.
  AWS: Smithy (services/service-com-amazonaws-appsync-awsdeepdishcontrolplaneservice): CreateResolver POST /v1/apis/{apiId}/types/{typeName}/resolvers, errors BadRequestException(400), ConcurrentModificationException(409), InternalFailureException(500), NotFoundException(404), UnauthorizedException(401). A resolver is attached to a field of a type in the API's active schema; when the named field does not exist on the named type, AWS refuses the call with its modeled not-found error (404 NotFoundException) — recorded behavior: the check's AWS run has the AWS::AppSync::Resolver resource for Query.ghost (a field absent from the active SDL, which declares only Query.echo) reach CREATE_FAILED, the stack update end UPDATE_ROLLBACK_COMPLETE, and a subsequent GetResolver(apiId, typeName=Query, fieldName=ghost) answer NotFoundException. The error message's fixed text is AWS's not-found wording for a resolver field; its variable parts are the request's typeName and fieldName. Hypothesis: AWS words it "No field named <fieldName> found on type <typeName>" — confirm against the recording before asserting the exact string; the status (404) and code (NotFoundException) are the requirement.
  Emulator: the GhostResolver resource is created successfully. The stack update reaches UPDATE_COMPLETE with no failed resource events — `AssertionError: stack A v4 ended 'UPDATE_COMPLETE', expected UPDATE_ROLLBACK_COMPLETE for a resolver on an absent field`. So CreateResolver performed no cross-check of typeName/fieldName against the API's active schema and returned 200.
  Why: a client relying on AWS's behavior sees a schema/resolver mismatch surfaced at deploy time — the bad resolver never lands and the stack self-heals by rolling back to the last good generation. Against the emulator the resolver is accepted and the stack reports UPDATE_COMPLETE, so an infrastructure change that AWS would reject is reported as applied; CloudFormation's rollback path for AppSync resolvers is never exercised, and the API ends up holding a resolver bound to a field no query can select.
  Fix: CreateResolver's validation in the AppSync control-plane handler (and the same check on UpdateResolver) — resolve apiId's active schema and refuse when typeName/fieldName is not a field of it, with the modeled NotFoundException; the CloudFormation AWS::AppSync::Resolver create handler then reports CREATE_FAILED and the stack rolls back.
  Traces: appsync.deploy-appsync-2-cfn-update-replace-rollback-rejected.

Cause 2 of 2: a GraphQL document that fails validation is answered with an envelope that has no `data` key at all, instead of `data: null` alongside `errors`.
  AWS: recorded behavior only (data-plane, not modeled in Smithy). AppSync distinguishes two refusal classes on the GraphQL endpoint. A document that PARSES but fails GraphQL validation (undefined field, an unsupplied non-null variable, a variable value that cannot coerce) is answered HTTP 200 with BOTH keys present: `data` present and null (validation happens before execution, so nothing resolved) and a non-empty `errors` array whose entries carry `locations` with integer line/column, no `path`, and NO `errorType` (a validation error is a request error, not one of AppSync's typed data-plane errors). The message names the offending symbol and the rule that refused it (e.g. "Validation error of type FieldUndefined" naming the field, "coerced Null value for NonNull type" naming the variable, "has an invalid value" naming the variable) — the rule phrase is AWS's fixed text, the quoted symbol is taken from the request document. A body AppSync cannot turn into a GraphQL operation is instead refused by the HTTP front door with 400 and NO `data` key, one error with errorType MalformedHttpRequestException and message "Unable to parse GraphQL query." (syntax error) or "Invalid request, `query` can't be empty." (empty query), and no `locations`. The presence/absence of the `data` key is exactly what separates the two classes.
  Emulator: for the validation class the response body carries only `errors` — `AssertionError: undefined field: a validation failure still returns a GraphQL envelope with a data entry, got keys ['errors']`. The `data` key is omitted, so a validation refusal is indistinguishable on the wire from a front-door parse refusal.
  Why: GraphQL clients (Amplify, Apollo, the AWS AppSync SDKs) branch on whether the envelope carries `data`: a present-but-null `data` means the operation was rejected before execution and is retriable/reportable as a request error, while a missing `data` means the request never became an operation. Omitting the key makes clients treat a validation failure as a malformed-transport failure, and generated typed clients that read `response.data` fail with a missing-field error instead of surfacing the validation message.
  Fix: the GraphQL data-plane response serializer — emit the `data` member (null) in the validation-failure envelope, and keep it absent only on the HTTP front-door MalformedHttpRequestException path.
  Traces: appsync.graphql-query-validation-rejected-before-execution.

Neither cause is covered by an open issue. Nearby but distinct open issues, named for the fixer's reference: 20261110T162826Z-appsync-createresolver-accepts-resolvers-aws-refuses-cachingkeys-prefix.md covers CreateResolver's cachingKeys-prefix and APPSYNC_JS-compile validation but not the schema field cross-check of cause 1; 20261116T072149Z-appsync-8-net-new-causes-graphql-data-plane.md covers data-plane caching/non-null-propagation/depth limits but not the missing `data` key of cause 2 (the emulator does raise the validation error here — it only drops the `data` member); 20261114T191235Z-appsync-cloudformation-4-net-new-causes-in-place.md covers in-place CFN property updates, not a failed-resource rollback.

## Evidence
Cause 1 — appsync.deploy-appsync-2-cfn-update-replace-rollback-rejected:
  Setup: a CFN stack holding AWS::AppSync::GraphQLApi (API_KEY) + ApiKey + GraphQLSchema (SDL declaring only `type Query { echo(msg: String): Echo }`) + NONE DataSource + EchoResolver. The v4 update adds GhostResolver = AWS::AppSync::Resolver with TypeName "Query", FieldName "ghost" (not in the SDL), Kind UNIT, Runtime APPSYNC_JS 1.0.0.
  Expected (AWS): GhostResolver -> CREATE_FAILED (CreateResolver refused, 404 NotFoundException naming type Query / field ghost); stack status UPDATE_ROLLBACK_COMPLETE; exactly {GhostResolver} in the failed-resource events; GetResolver(Query.ghost) afterwards -> NotFoundException; the v3 generation still serves (echo returns {msg, version:'v2'}) and GraphQLApi's physical id is unchanged.
  Actual (emulator): CreateResolver returned 200 for the absent field and the stack reached UPDATE_COMPLETE with no failed resource events.
  Assertion: `AssertionError: stack A v4 ended 'UPDATE_COMPLETE', expected UPDATE_ROLLBACK_COMPLETE for a resolver on an absent field` (check line 157).

Cause 2 — appsync.graphql-query-validation-rejected-before-execution:
  Setup: API_KEY GraphQL API, active SDL `schema { query: Query } type Query { echo(id: ID!): String }`, a NONE data source and a UNIT resolver on Query.echo. Control: `query Q($id: ID!) { echo(id: $id) }` with {"id":"abc"} returns 200, data.echo == "echo-abc", no errors.
  Request: POST the GraphQL endpoint with `{"query": "query { nope }"}` and header x-api-key.
  Expected (AWS): HTTP 200, body keys include BOTH `data` and `errors`; `data` is null; errors[0].message names 'nope' and contains "Validation error of type FieldUndefined"; errors[0] has integer locations line/column, path absent, errorType absent. Same shape for an unsupplied non-null variable (message contains "coerced Null value for NonNull type 'ID!'" naming 'id') and a non-coercible variable value (message contains "has an invalid value" naming 'id').
  Actual (emulator): body keys are ['errors'] only — the `data` member is absent.
  Assertion: `AssertionError: undefined field: a validation failure still returns a GraphQL envelope with a data entry, got keys ['errors']` (check line 169).

## Steps to Reproduce
Cause 1:
1. cloudformation.create_stack with a template containing AWS::AppSync::GraphQLApi (AuthenticationType API_KEY), AWS::AppSync::ApiKey, AWS::AppSync::GraphQLSchema whose Definition declares only `type Echo { msg: String version: String } type Query { echo(msg: String): Echo } schema { query: Query }`, a NONE AWS::AppSync::DataSource, and an AWS::AppSync::Resolver on Query.echo; wait for CREATE_COMPLETE.
2. cloudformation.update_stack with the same template plus a second AWS::AppSync::Resolver (TypeName "Query", FieldName "ghost", Kind UNIT, Runtime {APPSYNC_JS,1.0.0}, same DataSourceName) — `ghost` is not a field of Query in the active schema.
3. Poll describe_stacks for a terminal status. AWS: UPDATE_ROLLBACK_COMPLETE with GhostResolver CREATE_FAILED; emulator: UPDATE_COMPLETE.
4. appsync.get_resolver(apiId, typeName='Query', fieldName='ghost'): AWS answers NotFoundException after the rollback; the emulator serves the resolver it should never have created.
   (Equivalently, without CloudFormation: appsync.create_resolver(apiId, typeName='Query', fieldName='ghost', kind='UNIT', dataSourceName=<none-ds>) against an API whose active schema has no Query.ghost — AWS refuses 404 NotFoundException, the emulator returns 200.)

Cause 2:
1. appsync.create_graphql_api(authenticationType='API_KEY'); start_schema_creation with `schema { query: Query } type Query { echo(id: ID!): String }`; poll get_schema_creation_status to SUCCESS; create_api_key; create_data_source(type='NONE'); create_resolver on Query.echo (UNIT).
2. POST the api's uris.GRAPHQL with header x-api-key: <key> and body {"query": "query { nope }"}.
3. Observe the response body's top-level keys. AWS: 200 with `data` present and null plus a non-empty `errors` array (message names 'nope' and "Validation error of type FieldUndefined", with locations, no path, no errorType). Emulator: keys are ['errors'] — `data` missing.
4. Repeat with body {"query": "query Q($id: ID!) { echo(id: $id) }", "variables": {}} and with "variables": {"id": {"not": "an ID"}} — same missing-`data` divergence.
5. Contrast with the front-door class: body {"query": "query { echo(id: }"} and body {"query": ""} must be 400, NO `data` key, one error with errorType MalformedHttpRequestException and message "Unable to parse GraphQL query." / "Invalid request, `query` can't be empty." respectively.
