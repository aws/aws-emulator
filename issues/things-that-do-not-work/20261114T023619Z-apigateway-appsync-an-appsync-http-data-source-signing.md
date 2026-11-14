Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__appsync.appsync-http-data-source-sigv4-signs-request-to-api-gateway
Issue-Key: apigateway__appsync.appsync-http-data-source-sigv4-signs-request-to-api-gateway|_poll_until#38|Timeout|time.sleep(interval_s)
Issue-Commit: 0bdb628a6eeeae829cbfa42cf4e1c8a43263b6d6

# apigateway__appsync: an AppSync HTTP data source signing SigV4 to an API Gateway stage still never completes — the check now hangs to the 60 s cap instead of being refused

> _Note: this issue was identified against the code base at commit `0bdb628a6eeeae829cbfa42cf4e1c8a43263b6d6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateGraphqlApi

## Description
1 net-new root cause.

Cause 1 of 1: a GraphQL query posted to the AppSync API's own published endpoint never reaches an answer, so an HTTP data source that must SigV4-sign AppSync's outbound call to an API Gateway stage is never exercised. Failing check: apigateway__appsync.appsync-http-data-source-sigv4-signs-request-to-api-gateway.

AWS: recorded behavior only for the per-field statuses; what is structural is that the API's published data-plane address answers a POSTed GraphQL document. Smithy (appsync) models CreateGraphqlApi (POST /v1/apis, 200) returning GraphqlApi with `uris` and `dns` (MapOfStringToString, keys GRAPHQL and REALTIME); uris.GRAPHQL is the only address the service publishes for the data plane, and an emulator must publish an address it itself serves. On AWS, a query resolved by an HTTP data source whose authorizationConfig is AWS_IAM is signed with SigV4 by AppSync before it leaves, so an AWS_IAM-guarded API Gateway method admits the signed call and refuses the unsigned one (403), while the open method answers either way.

Emulator: nothing comes back. The check posts the GraphQL document to the published uris.GRAPHQL with an x-api-key header and never gets a response it accepts; the harness cuts it off and the only detail recorded is `Error: Check timed out after 60.0s`. None of the three fields (openViaUnsigned, guardedViaSigned, guardedViaUnsigned) is observed, so neither the SigV4 signing of HTTP-data-source egress nor the guarded method's signed/unsigned differential is exercised.

Why: a client cannot drive a single query through this API — it blocks rather than being answered, so the whole cross-service behavior (AppSync signing its own outbound call so an IAM-guarded REST method accepts it) is unobservable, and a caller gets no status to time out or retry on.

Fix: the appsync GraphQL data-plane listener and the GraphqlApi projection that must name it (the `uris`/`dns` construction in the CreateGraphqlApi / GetGraphqlApi handlers), plus the HTTP-data-source egress path that signs SigV4 toward the apigateway execute-api stage.

Related, NOT covering: open issue 20261111T065707Z-apigateway-appsync-the-graphql-api-s-published-uris.md carries this same check id and reports the published-URI defect — at that commit the very first data-plane request was REFUSED immediately with simparity.published.PublishedUrlRejected because uris.GRAPHQL was a real-AWS `*.appsync-api.<region>.amazonaws.com` host. The symptom has CHANGED: the request is no longer refused before it leaves, it is now sent and never answered within the budget. The check is not marked owned by that issue in this replay, so it is filed here; if the published URI now names the emulator's own host, what remains is the listener/resolver behind it rather than the URI text, and that issue's fix must not be closed on the strength of the URI alone. Open issue 20261110T215409Z-appsync-10-net-new-causes-graphql-realtime-endpoints.md (Cause 1) reports the appsync-only form of the published-endpoint defect.

## Evidence
Expected (AWS): graphqlApi['uris']['GRAPHQL'] names a host:port the emulator serves; POST {"query":"{ openViaUnsigned }"} with header x-api-key returns HTTP 200 and a GraphQL JSON document with `data`; POST {"query":"{ guardedViaSigned }"} returns 200 with the guarded method's payload (AppSync SigV4-signed the HTTP data source's call, so the AWS_IAM-guarded method admitted it); POST {"query":"{ guardedViaUnsigned }"} surfaces the guarded method's 403 refusal through the resolver.

Actual (emulator): no accepted response arrives for the first query. Replay detail is exactly:
  Error: Check timed out after 60.0s
with no status, headers or body recorded — unlike the earlier recording of this same check, which failed fast with simparity.published.PublishedUrlRejected on a `*.appsync-api.<region>.amazonaws.com` URI.

Scenario under test (from the check source): a REGIONAL REST API with one open method and one AWS_IAM-guarded method, deployed to a stage; then appsync.create_graphql_api(authenticationType='API_KEY'), start_schema_creation with an SDL exposing openViaUnsigned/guardedViaSigned/guardedViaUnsigned, an HTTP data source pointing at the stage invoke URL with authorizationConfig AWS_IAM, and UNIT resolvers; the check then posts GraphQL documents to the published uris.GRAPHQL.

Note on values: the api id, the AppSync api token in the published host, the stage host/port and the per-run names are not requirements. The requirement is that the published GRAPHQL endpoint is served and answers a POSTed GraphQL document, and that the AWS_IAM HTTP data source's egress is SigV4-signed.

## Steps to Reproduce
1. apigateway: create a REGIONAL REST API with one method at authorizationType=NONE and one at authorizationType=AWS_IAM (both backed by a mock or lambda integration), create_deployment + create_stage.
2. appsync.create_graphql_api(name=..., authenticationType='API_KEY'); create_api_key; start_schema_creation with an SDL exposing openViaUnsigned, guardedViaSigned and guardedViaUnsigned; create_data_source(type='HTTP', httpConfig={'endpoint': <stage invoke URL>, 'authorizationConfig': {'authorizationType': 'AWS_IAM', 'awsIamConfig': {...}}}); create_resolver (UNIT) for each field.
3. Read graphqlApi['uris']['GRAPHQL'] and POST {"query": "{ openViaUnsigned }"} to it with header x-api-key: <key>.
4. Observed: no response the caller can use — the request neither returns a GraphQL 200 document nor any 4xx/5xx; the caller blocks and the harness cuts the check off at 60 s. Expected: a 200 GraphQL document, then the signed/unsigned differential on the guarded field (200 vs 403).
5. Or replay the check id in trace_ids.
