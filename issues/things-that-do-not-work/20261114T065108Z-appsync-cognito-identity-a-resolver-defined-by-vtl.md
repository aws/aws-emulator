Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: appsync__cognito-identity.xsvc-appsync-3-authenticated-identity-field-grant-echoes-cognito-identity appsync__cognito-identity.xsvc-appsync-3-explicit-field-deny-overrides-api-wildcard-allow-denied appsync__cognito-identity.xsvc-appsync-3-unauthenticated-identity-role-without-grant-denied appsync__cognito-identity.xsvc-appsync-3-ungranted-query-field-denied
Issue-Key: appsync__cognito-identity.xsvc-appsync-3-authenticated-identity-field-grant-echoes-cognito-identity|run#14|AssertionError|assert status == 200 and not body.get("errors"), (
Issue-Key: appsync__cognito-identity.xsvc-appsync-3-explicit-field-deny-overrides-api-wildcard-allow-denied|run#10|AssertionError|assert status == 200 and not body.get("errors"), (
Issue-Key: appsync__cognito-identity.xsvc-appsync-3-unauthenticated-identity-role-without-grant-denied|run#10|AssertionError|assert status == 200 and not body.get("errors"), (
Issue-Key: appsync__cognito-identity.xsvc-appsync-3-ungranted-query-field-denied|run#10|AssertionError|assert status == 200 and not body.get("errors"), (
Issue-Commit: 36cbc41e18f8f7f03eaee998bf950ce282c69b6c

# appsync__cognito-identity: a resolver defined by VTL mapping templates is never executed — every such field answers data:null plus a "this endpoint does not run mapping templates" GraphQL error

> _Note: this issue was identified against the code base at commit `36cbc41e18f8f7f03eaee998bf950ce282c69b6c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GraphQL

## Description
Cause 1 of 1: the GraphQL data plane refuses to execute UNIT resolvers whose logic is a request/response MAPPING TEMPLATE (VTL), so every field backed by one resolves to null with a GraphQL error, and AWS_IAM field-level authorization can never be observed.

AWS: Smithy (appsync): CreateResolver accepts `requestMappingTemplate` and `responseMappingTemplate` (target MappingTemplate, @length 1..65536) as an alternative to `runtime`/`code`; a UNIT resolver on a NONE data source whose request template is `{"version": "2018-05-29", "payload": $util.toJson($context.identity)}` and whose response template is `$util.toJson($context.result)` resolves the field from that payload. Recorded behavior: the signed GraphQL POST answers HTTP 200 with `data` populated for the field and NO `errors` member; a refusal of such a field can then only come from the IAM policy (HTTP 401, x-amzn-ErrorType: UnauthorizedException, errorType UnauthorizedException, message "Permission denied"), which is what these checks are discriminating.

Emulator: the POST answers HTTP 200 but the field is null and the body carries a GraphQL error whose message is "This field's resolver is defined by mapping templates, which this endpoint does not run; define it with APPSYNC_JS code." with `path` = the field name — i.e. `{'data': {'allowed': None}, 'errors': [{'path': ['allowed'], 'message': "This field's resolver is defined by mapping templates, which this endpoint does not run; define it with APPSYNC_JS code."}]}`. That message is the emulator's own text; AWS has no such error class. It is served for the POSITIVE control of all four checks (an explicitly granted field, the api-wildcard-allowed field, and the authenticated-identity echo), so each check fails before it can even reach the denial it is testing.

Why: a client that stores its resolver logic as VTL mapping templates — still the documented and most common AppSync resolver form — gets a null field and an error for every query, instead of the resolved payload. Worse for authorization work: because a template-backed field never resolves, the emulator cannot distinguish "denied by the appsync:GraphQL IAM policy" (401 UnauthorizedException / "Permission denied") from "served", so IAM field-grant and explicit-deny behavior is unverifiable and an over-permissive policy decision would go unnoticed.

Fix: the AppSync GraphQL data-plane field executor — it must interpret the resolver's `requestMappingTemplate`/`responseMappingTemplate` (at minimum `$util.toJson(...)`, `$context.identity`, `$context.result` and the `{"version", "payload"}` NONE-data-source contract) instead of short-circuiting with an unimplemented-runtime GraphQL error, and keep the IAM authorization decision ahead of it so an ungranted field still answers 401 UnauthorizedException "Permission denied". Related but distinct already-open issues: 20261110T173718Z-appsync-cognito-identity-creategraphqlapi-publishes-uris-graphql-on.md (endpoint host) does NOT cover this; these checks reached the endpoint and got a template error.

Traces: appsync__cognito-identity.xsvc-appsync-3-authenticated-identity-field-grant-echoes-cognito-identity, appsync__cognito-identity.xsvc-appsync-3-explicit-field-deny-overrides-api-wildcard-allow-denied, appsync__cognito-identity.xsvc-appsync-3-unauthenticated-identity-role-without-grant-denied, appsync__cognito-identity.xsvc-appsync-3-ungranted-query-field-denied.

## Evidence
Expected (AWS): HTTP 200, body `{"data": {"allowed": {...echo of $context.identity...}}}` with no `errors` member for a field whose UNIT resolver is a VTL template pair on a NONE data source; a policy refusal instead answers HTTP 401 with x-amzn-ErrorType: UnauthorizedException and message "Permission denied".

Actual (emulator), identical in all four checks: HTTP 200 with `{'data': {'allowed': None}, 'errors': [{'path': ['allowed'], 'message': "This field's resolver is defined by mapping templates, which this endpoint does not run; define it with APPSYNC_JS code."}]}`. Assertion text: "the granted field was not served cleanly" / "positive control: the wildcard-allowed field was not served cleanly" / "positive control: the authenticated identity was not served".

The fixed text of the diverging message is the whole sentence quoted above; its only variable part is `path`, which is the queried field name from the request.

## Steps to Reproduce
1. CreateGraphqlApi with authenticationType=AWS_IAM; StartSchemaCreation with `schema { query: Query } type Query { allowed: AWSJSON other: AWSJSON }`.
2. CreateDataSource type=NONE.
3. CreateResolver for Query.allowed with requestMappingTemplate `{"version": "2018-05-29", "payload": $util.toJson($context.identity)}` and responseMappingTemplate `$util.toJson($context.result)` (no `runtime`/`code`).
4. Vend Cognito identity-pool credentials, SigV4-sign (signing name `appsync`) a POST of `{"query": "query { allowed }"}` to the api's GRAPHQL uri.
5. Observe HTTP 200 with data.allowed = null and the mapping-template error above, instead of the echoed $context.identity payload.
