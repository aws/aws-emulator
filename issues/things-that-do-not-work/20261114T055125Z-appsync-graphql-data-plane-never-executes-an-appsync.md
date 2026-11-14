Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: appsync.e2e-data-source-introspection-driven-resolver-flow
Issue-Key: appsync.e2e-data-source-introspection-driven-resolver-flow|_graphql_request_with_retries#51|TimeoutError|raise TimeoutError(
Issue-Commit: d42870bc3a3955aaff0594ef70435ba8ae8c0650

# AppSync GraphQL data plane never executes an APPSYNC_JS resolver over a RELATIONAL_DATABASE (RDS_HTTP_ENDPOINT) data source: every query comes back with GraphQL errors, so the introspection-driven round trip never completes (1 cause)

> _Note: this issue was identified against the code base at commit `d42870bc3a3955aaff0594ef70435ba8ae8c0650`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateResolver

## Description
Cause 1 of 1: a resolver created from StartDataSourceIntrospection's discovered column names, bound to a RELATIONAL_DATABASE data source whose rdsHttpEndpointConfig points at an Aurora cluster with the Data API enabled, is never executed successfully by the emulator's GraphQL data plane. Every POST of the query to the API's published GraphQL endpoint returns a response that carries an "errors" member (or cannot be answered at all), so the check's retry-until-error-free loop never succeeds and burns the whole run budget. Failing check: appsync.e2e-data-source-introspection-driven-resolver-flow.

AWS: CreateResolver (Smithy: appsync CreateResolver, POST /v1/apis/{apiId}/types/{typeName}/resolvers; input members dataSourceName (ResourceName), code (Code, @length 1..32768), runtime (AppSyncRuntime, APPSYNC_JS/1.0.0); errors BadRequestException 400, NotFoundException 404, UnauthorizedException 401, ConcurrentModificationException 409, InternalFailureException 500) registers a unit resolver that AppSync then RUNS for the bound field. CreateDataSource with type=RELATIONAL_DATABASE and relationalDatabaseConfig.relationalDatabaseSourceType=RDS_HTTP_ENDPOINT makes AppSync issue the resolver's statement through the RDS Data API using the data source's serviceRoleArn and awsSecretStoreArn. The GraphQL data-plane POST itself is not in the Smithy model — recorded behavior only: POSTing {"query": "query { getWidget(id: 1) { name } }"} to GraphqlApi$uris.GRAPHQL with header x-api-key: <apiKey> on an API_KEY api returns HTTP 200 and a body with NO "errors" member and data.getWidget.name equal to the value of the row previously written through rds-data.ExecuteStatement. The APPSYNC_JS request function builds the statement with sql`...`/createMySQLStatement and the response function maps ctx.result through toJsonObject; the requirement is that the emulator actually evaluates those functions, runs the statement against the same Data API state, and returns the resulting row in the GraphQL data envelope.

Emulator: the control plane upstream all answers — CreateDataSource on the RDS_HTTP_ENDPOINT source, StartDataSourceIntrospection/GetDataSourceIntrospection reaching SUCCESS and discovering the widgets table's id and name columns, StartSchemaCreation reaching SUCCESS, CreateResolver, and GetResolver echoing dataSourceName — but the data plane never produces an error-free response for the field. Each iteration of the retry loop either returns a body containing "errors" or fails the HTTP attempt, for the whole retry window, and the check ends with TimeoutError("a GraphQL response without errors: the run budget ended first; not graded") raised from _graphql_request_with_retries. The harness keeps only the last body internally, so the specific GraphQL error text is not surfaced in the replay output; what is established is that no attempt ever produced the AWS-shaped envelope without errors. Note the previous failure mode of this same check was PublishedUrlRejected on uris.GRAPHQL (open issue 20261113T045825Z-appsync-publishes-graphqlapi-uris-graphql-on-a-real.md, which carries that key); the request now reaches the emulator's own listener and reaches the loop body, so the surviving divergence is resolver execution, not endpoint publication. Sibling open issues report the same 'resolver is never dispatched' symptom for other data-source kinds (20261114T043034Z-appsync-dynamodb-events-vtl-mapping-template-resolvers-are.md, 20261114T043034Z-appsync-events-sqs-amazon-eventbridge-data-source-resolvers.md, 20261114T003845Z-appsync-lambda-2-net-new-causes-direct-lambda.md); none of them covers the RELATIONAL_DATABASE/RDS_HTTP_ENDPOINT path or this check.

Why: a client that follows the documented path — discover a table with StartDataSourceIntrospection, build a schema and an APPSYNC_JS resolver from the discovered field names, then query the API — gets GraphQL errors instead of its row. The control plane's 200s on CreateDataSource/CreateResolver therefore mean nothing: no AppSync-over-Aurora application can be exercised against the emulator, and a caller cannot distinguish 'resolver wrong' from 'emulator does not run resolvers'.

Fix: the GraphQL data-plane executor (the handler serving the published GRAPHQL endpoint and the resolver-dispatch path it uses) must evaluate an APPSYNC_JS unit resolver's request/response functions, execute the produced statement against the RELATIONAL_DATABASE data source through the emulator's rds-data state using the configured secret, and return {"data": ...} with no "errors" member; the @aws-appsync/utils/rds helpers (sql, createMySQLStatement, toJsonObject) must be available to that sandbox.

Traces: appsync.e2e-data-source-introspection-driven-resolver-flow.

## Evidence
Check appsync.e2e-data-source-introspection-driven-resolver-flow [error].

Expected (AWS): POST to api["uris"]["GRAPHQL"] with {"query": "query { getWidget(id: 1) { name } }"} and header x-api-key: <apiKey> -> HTTP 200, body with no "errors" key and data.getWidget.name == the name of the row inserted through rds-data.ExecuteStatement (in this recording 'widget-alpha'; the literal value and the generated apiId/api key are per-run values, the requirement is the row written in step 3).

Actual (emulator): no attempt in the retry window ever returned a body without an "errors" member, so the loop ran to its deadline and raised:

  File "check_appsync_e2e_data_source_introspection_driven_resolver_flow.py", line 187, in run
    body = _graphql_request_with_retries(
  File "check_appsync_e2e_data_source_introspection_driven_resolver_flow.py", line 343, in _graphql_request_with_retries
    raise TimeoutError(
  TimeoutError: a GraphQL response without errors: the run budget ended first; not graded

Everything before the data-plane POST succeeded against the emulator: create_data_source(type=RELATIONAL_DATABASE, rdsHttpEndpointConfig -> cluster ARN/testdb/secret ARN), start_data_source_introspection + get_data_source_introspection reaching SUCCESS with the widgets model carrying fields id and name, start_schema_creation reaching SUCCESS, create_resolver(runtime APPSYNC_JS 1.0.0), and get_resolver returning dataSourceName == "widgets_rds".

Smithy cited: appsync CreateResolver (POST /v1/apis/{apiId}/types/{typeName}/resolvers; dataSourceName/code/runtime members; errors BadRequestException 400 / NotFoundException 404 / UnauthorizedException 401 / ConcurrentModificationException 409 / InternalFailureException 500); CreateDataSource type RELATIONAL_DATABASE with relationalDatabaseConfig.rdsHttpEndpointConfig; GraphqlApi$uris (MapOfStringToString). The data-plane POST and its response envelope are not modeled — recorded behavior only.

## Steps to Reproduce
1. secretsmanager.create_secret with a JSON {username,password} body; keep the ARN.
2. rds.create_db_cluster (Engine aurora-mysql, EnableHttpEndpoint=True, ServerlessV2ScalingConfiguration 0.5/2, DatabaseName=testdb, a DB subnet group over the run's network) plus a db.serverless member instance; wait until the instance is available and until rds-data.execute_statement 'SELECT 1' succeeds.
3. Through rds-data.execute_statement: CREATE TABLE widgets (id INT NOT NULL PRIMARY KEY, name VARCHAR(64) NOT NULL); INSERT INTO widgets (id, name) VALUES (1, 'widget-alpha').
4. iam.create_role trusting appsync.amazonaws.com, with rds-data:ExecuteStatement/BatchExecuteStatement on the cluster ARN and secretsmanager:GetSecretValue on the secret ARN.
5. api = appsync.create_graphql_api(name=..., authenticationType="API_KEY")["graphqlApi"]; appsync.create_api_key(apiId=api["apiId"]).
6. appsync.create_data_source(apiId, name="widgets_rds", type="RELATIONAL_DATABASE", serviceRoleArn=<role>, relationalDatabaseConfig={relationalDatabaseSourceType: RDS_HTTP_ENDPOINT, rdsHttpEndpointConfig: {awsRegion, dbClusterIdentifier: <cluster ARN>, databaseName: testdb, awsSecretStoreArn: <secret ARN>}}).
7. appsync.start_data_source_introspection(rdsDataApiConfig={databaseName, resourceArn, secretArn}); poll get_data_source_introspection until SUCCESS and read the widgets model's field names (id, name).
8. appsync.start_schema_creation with 'type Widget { id: String name: String } type Query { getWidget(id: Int!): Widget } schema { query: Query }'; poll get_schema_creation_status to SUCCESS.
9. appsync.create_resolver(apiId, typeName="Query", fieldName="getWidget", dataSourceName="widgets_rds", runtime={name: APPSYNC_JS, runtimeVersion: 1.0.0}, code=an APPSYNC_JS request function using sql`SELECT id, name FROM widgets WHERE id = ${id}` + createMySQLStatement and a response function using toJsonObject(ctx.result)[0]).
10. POST {"query": "query { getWidget(id: 1) { name } }"} to api["uris"]["GRAPHQL"] with header x-api-key: <apiKey>, retrying for ~120 s.
   Observed: no response without an "errors" member is ever returned; the retry loop runs out of budget.
   Expected: 200 with {"data": {"getWidget": {"name": "widget-alpha"}}} and no "errors" member.
