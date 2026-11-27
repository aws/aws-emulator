Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__xray.untraced-stage-records-no-segment-for-an-upstream-sampled-request
Issue-Key: apigateway__xray.untraced-stage-records-no-segment-for-an-upstream-sampled-request|_first_recorded#12|Timeout|time.sleep(pause)
Issue-Commit: 2b0c9769718f858c1f805a325f14628bb527d009

# apigateway__xray: a REST API stage with tracingEnabled=true records no X-Ray stage segment for an upstream-sampled request, so the traced-vs-untraced stage contrast is unobservable (1 cause)

> _Note: this issue was identified against the code base at commit `2b0c9769718f858c1f805a325f14628bb527d009`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
BatchGetTraces

## Description
Cause 1 of 1: the API Gateway edge does not record an X-Ray segment for a request served by a stage whose tracingEnabled is true, so nothing is ever readable through X-Ray for that request and the check is terminated at the 60 s harness ceiling instead of reaching an assertion. Failing check: apigateway__xray.untraced-stage-records-no-segment-for-an-upstream-sampled-request (replay verdict `mismatch`, Error: 'Check timed out after 60.0s'). The check is marked neither ALREADY FILED nor OWNED in this run's report; apigateway and xray are both bootstrapped and answering.

RELATED, NOT COVERING: 20261025T080135Z-apigateway-dynamodb-lambda-s3-xray-both-end-to.md, 20261027T085253Z-apigateway-dynamodb-lambda-s3-xray-caller-trace-context.md and 20261027T205529Z-apigateway-dynamodb-lambda-s3-xray-the-traced-rest.md all describe a traced REST API stage producing no trace, but every one of them is keyed on apigateway__dynamodb__lambda__s3__xray checks whose path runs through an AWS_PROXY integration into Lambda (and whose stage GET was recorded answering 404 UnknownService). This failure is on a MOCK integration with no Lambda in the path, and the stage GET must have answered 200 with the mock body here, because the check asserts that before it waits and no assertion fired — so it isolates the stage-level X-Ray segment emission itself.

AWS: recorded behavior only for the producing side (no Smithy model describes edge segment emission); the read side is modeled — xray BatchGetTraces POST /Traces -> 200 {Traces[].{Id, Segments[].Document}, UnprocessedTraceIds}, paginated by NextToken; apigateway CreateStage/UpdateStage carry `tracingEnabled` ("Specifies whether active tracing with X-ray is enabled"). Recorded behavior: for a REGIONAL REST API whose root GET is answered by a MOCK integration and which is deployed to two stages of the same deployment — one with tracingEnabled=false, one with tracingEnabled=true — a GET carrying `X-Amzn-Trace-Id: Root=<caller id>;Sampled=1` to the TRACED stage is filed under the caller's own trace id and that trace holds exactly one segment with origin `AWS::ApiGateway::Stage` whose `name` is `<rest api name>/<stage name>`; the same GET to the UNTRACED stage is answered 200 but records no segment of that origin at all; and after /tracingEnabled is patched to true on that stage, a further GET to it does produce its own `<api name>/<stage name>` stage segment. The api name and stage names are per-run values filling that `name` template; the requirement is one AWS::ApiGateway::Stage segment per traced stage request, under the caller's Root id, and none for an untraced stage.

Emulator: the stage builds and serves — create_rest_api, put_method/put_integration (MOCK)/put_method_response/put_integration_response, create_deployment, create_stage for both stages all succeed, and get_stage reports tracingEnabled false/true as set (asserted before any waiting). The GET helper asserts HTTP 200 with the mock body '"pong"' on every call, and no assertion and no ClientError ever fired. What never happens is the read: the check sends up to 12 upstream-sampled GETs to the traced stage, polling xray.batch_get_traces (all pages) after each for any trace holding a segment document with origin 'AWS::ApiGateway::Stage', with its own 180 s budget, and is cut off at the 60 s ceiling. So X-Ray holds no stage segment for a traced stage's request, and the two later legs — that the untraced twin stage records nothing, and that patching /tracingEnabled to true starts recording — are never reachable.

Why: active tracing on a stage is how a client gets the edge hop of a distributed trace. With no AWS::ApiGateway::Stage segment ever recorded, a client's trace starts at its backend rather than at the API, so edge latency and 4xx/5xx attribution at the gateway are invisible; and because the traced case records nothing either, a client cannot verify the sampling/tracing switch at all — it cannot distinguish 'tracing off' from 'tracing not implemented', which is exactly the inversion that makes an untraced stage look compliant.

Fix: the execute-api stage data plane — the front-door request path that serves a deployed REST API stage — must, when the stage's tracingEnabled is true, emit one X-Ray segment per request with origin `AWS::ApiGateway::Stage` and name `<rest api name>/<stage name>` under the request's trace id (continuing an inbound `X-Amzn-Trace-Id: Root=<id>;Sampled=1`), and emit none when tracingEnabled is false; the xray trace store must accept that edge-produced segment and make it readable through BatchGetTraces like any ingested segment.

Traces: apigateway__xray.untraced-stage-records-no-segment-for-an-upstream-sampled-request.

## Evidence
Replay output:

[mismatch] apigateway__xray.untraced-stage-records-no-segment-for-an-upstream-sampled-request
  Error: Check timed out after 60.0s

What the check establishes BEFORE it waits (so these are not what diverged): create_rest_api(endpointConfiguration={'types':['REGIONAL']}); root GET with authorizationType NONE, a MOCK integration with requestTemplates {'application/json': '{"statusCode": 200}'}, a 200 method response and an integration response with responseTemplates {'application/json': '{"reply": "pong"}'}; one create_deployment deployed to stage 'dark' (tracingEnabled=False) and stage 'lit' (tracingEnabled=True); get_stage reports {'dark': False, 'lit': True} (asserted). Every GET is asserted to answer HTTP 200 containing '"pong"' before any polling, with header X-Amzn-Trace-Id: Root=1-<8 hex epoch>-<24 hex>;Sampled=1.

Expected (recorded AWS): batch_get_traces(TraceIds=[the caller's own Root id]) for a GET to the 'lit' stage returns that trace with exactly one segment document whose origin is 'AWS::ApiGateway::Stage' and whose name is '<api name>/lit'; the same read for the 'dark' stage's GET returns no such segment for the whole hold window; after update_stage(patchOperations=[{'op':'replace','path':'/tracingEnabled','value':'true'}]) on 'dark' (get_stage then reports true, which would be asserted), a GET to 'dark' yields exactly one segment named '<api name>/dark'.

Observed: the check is cut off inside the first read loop at the 60 s replay ceiling — no error response, no failed assertion — i.e. the traced stage served 200 but X-Ray never held any AWS::ApiGateway::Stage segment for the sampled request, so neither the required ABSENCE for the untraced stage nor the lift after enabling tracing was ever observed.

Fixed text vs per-run values: the origin 'AWS::ApiGateway::Stage' and the name template '<rest api name>/<stage name>' are the requirement; the api id, generated api name, trace ids, host/port of the invoke URL and timestamps are per-run values.

## Steps to Reproduce
1. git_clone, then run_behavioral_suite (it builds and boots the emulator); confirm bootstrap_signal is false and both apigateway and xray answer.
2. apigateway.create_rest_api(name=<unique>, endpointConfiguration={'types': ['REGIONAL']}); on the root resource put_method(httpMethod='GET', authorizationType='NONE'), put_integration(type='MOCK', requestTemplates={'application/json': '{"statusCode": 200}'}), put_method_response(statusCode='200'), put_integration_response(statusCode='200', responseTemplates={'application/json': '{"reply": "pong"}'}).
3. create_deployment, then create_stage(stageName='dark', tracingEnabled=False) and create_stage(stageName='lit', tracingEnabled=True) from that deployment; confirm get_stage reports tracingEnabled False / True.
4. GET the 'lit' stage invoke URL with header 'X-Amzn-Trace-Id: Root=1-<8 hex epoch>-<24 hex>;Sampled=1' — observe HTTP 200 with {"reply": "pong"}.
5. Page xray.batch_get_traces(TraceIds=[that Root id]) and look for a segment document with origin 'AWS::ApiGateway::Stage'. Observe none is ever returned (expected exactly one, named '<api name>/lit'); repeating the GET and the read does not change it, which is what consumes the check's budget.
6. After the fix, also confirm the negative and the lift: the same GET to the 'dark' stage records NO AWS::ApiGateway::Stage segment, and after update_stage(patchOperations=[{'op':'replace','path':'/tracingEnabled','value':'true'}]) a further GET to 'dark' records exactly one segment named '<api name>/dark'.
7. Re-replay apigateway__xray.untraced-stage-records-no-segment-for-an-upstream-sampled-request to confirm the fix.
