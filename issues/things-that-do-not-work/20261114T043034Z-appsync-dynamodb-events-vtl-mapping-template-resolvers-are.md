Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: appsync__dynamodb__events.xsvc-appsync-2-resolver-error-in-a-200-response-is-a-successful-delivery-not-dead-lettered appsync__dynamodb__events.xsvc-appsync-2-role-without-graphql-denied-dead-letters-and-counts-failed-invocations
Issue-Key: appsync__dynamodb__events.xsvc-appsync-2-resolver-error-in-a-200-response-is-a-successful-delivery-not-dead-lettered|_poll_until#21|AssertionError|raise AssertionError(f"{description} not met within {timeout_s}s. Last result: {last!r}")
Issue-Key: appsync__dynamodb__events.xsvc-appsync-2-role-without-graphql-denied-dead-letters-and-counts-failed-invocations|_poll_until#22|AssertionError|raise AssertionError(f"{description} not met within {timeout_s}s. Last result: {last!r}")
Issue-Commit: 7446e1137d863a6c5f0117bdbe1b0c65f0c299cf

# appsync__dynamodb__events: VTL mapping-template resolvers are not executed — the GraphQL data plane answers "this endpoint does not run mapping templates; define it with APPSYNC_JS code"

> _Note: this issue was identified against the code base at commit `7446e1137d863a6c5f0117bdbe1b0c65f0c299cf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PostGraphQL (AppSync GraphQL data plane) / CreateResolver (requestMappingTemplate/responseMappingTemplate)

## Description
Cause 1 of 1: a UNIT resolver defined by VTL request/response mapping templates on an AMAZON_DYNAMODB data source is never run; the data plane returns a null field and an emulator-authored resolver error demanding APPSYNC_JS code.

AWS: mapping templates are a fully supported, modeled way to define a resolver — Smithy: Resolver$requestMappingTemplate and Resolver$responseMappingTemplate target MappingTemplate (@length min 1, max 65536, @pattern "^.*$"), and Resolver$code is documented as the alternative ("The resolver code that contains the request and response functions. When code is used, the runtime is required. The runtime value must be APPSYNC_JS") — i.e. APPSYNC_JS code is one option, never a precondition for executing a resolver. A UNIT resolver (Resolver$kind UNIT: "run a GraphQL query against a single data source") on an AMAZON_DYNAMODB data source (DataSourceType AMAZON_DYNAMODB, DataSource$dynamodbConfig) evaluates the VTL request template (including $util.dynamodb.toDynamoDBJson and the "version": "2018-05-29" PutItem/GetItem operations), performs the DynamoDB call under the data source's serviceRoleArn, and renders the result through the response template. A successful mutation is HTTP 200 with data.<field> populated from the written item and NO `errors` member; a resolver that raises (e.g. $util.error on $ctx.error) is still HTTP 200, carrying both data and an `errors` entry whose message/errorType come from the template — which is why such a response is a successful EventBridge delivery and not a dead letter.

Emulator: every such field is answered HTTP 200 with data.<field> null and errors: [{"path": ["<field>"], "message": "This field's resolver is defined by mapping templates, which this endpoint does not run; define it with APPSYNC_JS code."}]. The whole sentence is fixed emulator-authored text (the only variable part is the `path`, the field being resolved); AWS has no such error. CreateResolver accepts the templates, so the divergence is entirely in execution: no DynamoDB item is ever written or read, and a VTL resolver can never succeed nor produce its own $util.error.

Why: a client using the documented VTL resolver style gets a 200 whose payload says its field could not be resolved and whose side effect never happened. Downstream behaviour that depends on a resolved field is unobservable: an EventBridge rule whose AppSync target mutation 'succeeds with resolver errors in a 200' cannot be distinguished from a failed invocation (so dead-letter/failed-invocation accounting cannot be exercised), and an input-transformer-driven mutation never writes its mapped item.

Fix: the AppSync GraphQL data-plane resolver-execution path needs a VTL mapping-template engine alongside the APPSYNC_JS runtime: evaluate requestMappingTemplate (with the $util/$ctx bindings), dispatch the rendered operation to the resolver's data source (DynamoDB PutItem/GetItem here) under its serviceRoleArn, then render responseMappingTemplate and populate data/errors from it. Related open issues cover different causes (the published uris.GRAPHQL naming an unserved real-AWS host — 20261111T065053Z-appsync-dynamodb-events-creategraphqlapi-publishes-uris-graphql-on.md, 20261110T155738Z-appsync-dynamodb-events-the-graphqlapi-uri-published-to.md; and resolver persistence — 20261111T092526Z-appsync-updateresolver-does-not-persist-the-mapping-templates.md): the endpoint is now reached and the templates are stored, and this execution gap is the next, distinct cause.

Traces: appsync__dynamodb__events.xsvc-appsync-2-resolver-error-in-a-200-response-is-a-successful-delivery-not-dead-lettered, appsync__dynamodb__events.xsvc-appsync-2-role-without-graphql-denied-dead-letters-and-counts-failed-invocations. Two sibling checks in the same service (xsvc-appsync-2-near-miss-events-not-delivered-while-the-pipeline-is-live and xsvc-appsync-2-rule-input-transformer-mutation-writes-the-mapped-item) only report "Check timed out after 60.0s" — they warm the same VTL resolver first, so they are consistent with this cause, but their output carries no emulator diagnosis and they are not claimed here.

## Evidence
Expected (AWS): HTTP 200, body {"data": {"putItem": {"id": "<id>", "kind": "<kind>"}}} with no "errors" member, and the item present in the DynamoDB table (and, for the deliberately raising response template, 200 with an `errors` entry whose message/errorType come from $util.error).

Actual (emulator), from both checks' polling wait (150 s budget, last result shown):
status 200, body {'data': {'putItem': None}, 'errors': [{'path': ['putItem'], 'message': "This field's resolver is defined by mapping templates, which this endpoint does not run; define it with APPSYNC_JS code."}]}
- xsvc-appsync-2-resolver-error-in-a-200-response-is-a-successful-delivery-not-dead-lettered: AssertionError "a signed putItem mutation to resolve (data-source role propagation) not met within 150s. Last result: (200, {...as above...})"
- xsvc-appsync-2-role-without-graphql-denied-dead-letters-and-counts-failed-invocations: identical status/body and assertion.
The resolver was created with requestMappingTemplate '{"version": "2018-05-29", "operation": "PutItem", "key": {"id": $util.dynamodb.toDynamoDBJson($ctx.args.id)}, "attributeValues": {"kind": $util.dynamodb.toDynamoDBJson($ctx.args.kind)}}' and a $util.toJson($ctx.result) response template — both accepted by CreateResolver.

## Steps to Reproduce
1. CreateGraphqlApi (AWS_IAM) and StartSchemaCreation with `type Mutation { putItem(id: ID!, kind: String!): Item }`.
2. CreateTable in DynamoDB; CreateDataSource type=AMAZON_DYNAMODB with dynamodbConfig naming it and a serviceRoleArn allowing dynamodb:PutItem/GetItem.
3. CreateResolver (kind UNIT) for Mutation.putItem with the VTL requestMappingTemplate '{"version": "2018-05-29", "operation": "PutItem", "key": {"id": $util.dynamodb.toDynamoDBJson($ctx.args.id)}, "attributeValues": {"kind": $util.dynamodb.toDynamoDBJson($ctx.args.kind)}}' and responseMappingTemplate '$util.toJson($ctx.result)'.
4. POST the SigV4-signed mutation to the API's GraphQL endpoint.
Observed: 200 with data.putItem null and errors[0].message "This field's resolver is defined by mapping templates, which this endpoint does not run; define it with APPSYNC_JS code."; the table stays empty. Expected: 200 with data.putItem = {id, kind} and the item written.
