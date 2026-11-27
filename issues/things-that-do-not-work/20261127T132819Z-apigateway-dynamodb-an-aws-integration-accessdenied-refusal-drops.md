Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__dynamodb.aws-integration-role-without-putitem-rejected-until-granted apigateway__dynamodb.query-through-stage-lists-items-in-numeric-sort-key-order apigateway__dynamodb.rest-vtl-query-orders-by-sort-key-per-querystring apigateway__dynamodb.stage-query-pages-by-limit-and-start-key-in-sort-order
Issue-Key: apigateway__dynamodb.aws-integration-role-without-putitem-rejected-until-granted|run#21|AssertionError|assert json.loads(text) == {"refused": "com.amazon.coral.service#AccessDeniedException"}, (
Issue-Key: apigateway__dynamodb.query-through-stage-lists-items-in-numeric-sort-key-order|run#30|Timeout|time.sleep(pause)
Issue-Key: apigateway__dynamodb.rest-vtl-query-orders-by-sort-key-per-querystring|_get_until_served#23|Timeout|time.sleep(min(3 * (attempt + 1), 15))
Issue-Key: apigateway__dynamodb.stage-query-pages-by-limit-and-start-key-in-sort-order|run#29|Timeout|time.sleep(min(2 * 2 ** attempt, 20))
Issue-Commit: 803b8f0256cf78557f71fb793e40c4a31d070d39

# apigateway__dynamodb: an AWS-integration AccessDenied refusal drops the coral __type namespace, and DynamoDB Query through a deployed stage never answers (2 causes)

> _Note: this issue was identified against the code base at commit `803b8f0256cf78557f71fb793e40c4a31d070d39`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutItem

## Description
Two net-new root causes in the API Gateway AWS_PROXY/AWS integration path to DynamoDB.

Cause 1 of 2: a DynamoDB AccessDeniedException surfaced to an API Gateway integration-response mapping carries a bare error code instead of the fully-qualified coral shape id.
AWS: DynamoDB is awsJson1_0. Its error responses carry `__type` as a fully-qualified shape id; for an authorization refusal of a data-plane call the body is `{"__type":"com.amazon.coral.service#AccessDeniedException", "message": ...}` with HTTP 400. A VTL integration-response template that reads `$input.path('$.__type')` therefore sees `com.amazon.coral.service#AccessDeniedException`. The fixed text here is the namespace prefix `com.amazon.coral.service#`; the variable part is the error name (`AccessDeniedException`).
Emulator: the integration reads back only `AccessDeniedException` — the body's `__type` is the unqualified error name, with no `com.amazon.coral.service#` namespace. Replay detail: `the refusal body must carry DynamoDB's AccessDeniedException type, got '{"refused": "AccessDeniedException"}'`.
Why: every client and every integration mapping that keys off `__type` (which is how awsJson1_0 clients classify errors) mis-classifies the refusal; API Gateway mappings that branch on the coral namespace fall through to their default arm, so an authorization failure is reported as an unknown error rather than AccessDenied.
Fix: the DynamoDB awsJson1_0 error serializer — it must emit the fully-qualified `__type` for authorization/service errors, not just for ValidationException.
Traces: apigateway__dynamodb.aws-integration-role-without-putitem-rejected-until-granted.

Cause 2 of 2: an API Gateway AWS integration that calls DynamoDB Query through a deployed stage never returns, so the check exhausts its 60s budget.
AWS: recorded behavior only. A REST API with an AWS integration to `dynamodb:Query`, deployed to a stage, answers the stage invoke with 200 and the mapped Query result (items in sort-key order, honouring Limit/ExclusiveStartKey from the query string).
Emulator: three independent checks that invoke a deployed stage whose integration is DynamoDB Query all report `Check timed out after 60.0s` — no response is produced at all. The same account's Query calls made directly against DynamoDB pass elsewhere in this suite, so the hang is in the API Gateway integration invoke path for the Query action, not in Query itself.
Why: a client calling its own deployed REST API hangs instead of receiving data; no status, no error, no timeout of its own. Any build or test that fronts a DynamoDB table with API Gateway stalls.
Fix: the API Gateway stage-invoke handler's AWS-integration dispatch for the DynamoDB Query action (request mapping, upstream call, integration-response mapping) — one of those legs never completes.
Traces: apigateway__dynamodb.query-through-stage-lists-items-in-numeric-sort-key-order, apigateway__dynamodb.rest-vtl-query-orders-by-sort-key-per-querystring, apigateway__dynamodb.stage-query-pages-by-limit-and-start-key-in-sort-order.

## Evidence
Cause 1 — expected body `{"refused": "com.amazon.coral.service#AccessDeniedException"}` (the mapping copies `$.__type`); actual `{"refused": "AccessDeniedException"}`.
Cause 2 — expected: stage invoke returns 200 with the Query result page; actual: no response within 60s, `Check timed out after 60.0s` for all three stage-Query checks.

## Steps to Reproduce
1. Create a REST API with an AWS integration to DynamoDB PutItem and a role without dynamodb:PutItem; invoke the stage and read `$input.path('$.__type')` from the integration response — observe the unqualified `AccessDeniedException`.
2. Create a REST API with an AWS integration to DynamoDB Query, deploy it to a stage, and GET the stage resource — observe the request never completes.
