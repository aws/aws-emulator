Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__sns__sqs.xsvc-apigateway-2-sns-publish-message-attributes-reach-sqs-subscriber apigateway__sns__sqs.xsvc-apigateway-2-sns-role-missing-publish-denied-surfaces-authorization-error
Issue-Key: apigateway__sns__sqs.xsvc-apigateway-2-sns-publish-message-attributes-reach-sqs-subscriber|_poll_http#19|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__sns__sqs.xsvc-apigateway-2-sns-role-missing-publish-denied-surfaces-authorization-error|_poll_http#18|Timeout|time.sleep(interval_s)
Issue-Commit: 0bdb628a6eeeae829cbfa42cf4e1c8a43263b6d6

# apigateway__sns__sqs: a POST to a deployed stage whose AWS integration publishes to SNS still never reaches an accepted response — both checks exhaust the 60 s budget

> _Note: this issue was identified against the code base at commit `0bdb628a6eeeae829cbfa42cf4e1c8a43263b6d6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Publish

## Description
1 net-new root cause.

Cause 1 of 1: a request through a deployed REST API stage whose non-proxy AWS-type integration targets Amazon SNS `Publish` never reaches an accepted response, so the caller stalls until the harness budget expires. Failing checks: apigateway__sns__sqs.xsvc-apigateway-2-sns-publish-message-attributes-reach-sqs-subscriber, apigateway__sns__sqs.xsvc-apigateway-2-sns-role-missing-publish-denied-surfaces-authorization-error.

AWS: recorded behavior only for the per-case statuses; what is structural is that every request to a deployed stage yields an HTTP response. For the granted case AWS answers the method response 200, the integration response template renders `{"published": true, "messageId": "<MessageId from $.PublishResponse.PublishResult.MessageId>"}`, the two typed message attributes the request template set (`color` String, `qty` Number) arrive at the envelope subscriber as `{"Type":..., "Value":...}` and at the raw subscriber as SQS `MessageAttributes` `{"StringValue":..., "DataType":...}`, and the topic's FilterPolicy on `color` admits only the matching publish. For the denied case the integration credentials role that holds `sns:GetTopicAttributes` but not `sns:Publish` earns Amazon SNS's own `AuthorizationError` (HTTP 403), which the error integration response surfaces to the caller as `{"published": false, "surfacedCode": "AuthorizationError", "surfacedType": "Sender", "surfacedMessage": "<SNS's own message>"}`. The fixed-text parts of that are the code `AuthorizationError` and the type `Sender`; the message's variable parts are the role/assumed-role principal ARN and the topic ARN from the request, and the MessageId is a per-run value that is not part of the requirement.

Emulator: no accepted response ever arrives. Both checks hang in the invoke poll (`_poll_http`, which accepts only `status == 200 and '"published": true' in body`, deadline 240 s) and are cut off by the harness: the only detail reported is `Error: Check timed out after 60.0s`. Neither the granted 200 nor the denied 403 is observed, so nothing downstream (the SNS envelope, the raw delivery, the filter-policy differential, the surfaced AuthorizationError) is reached either.

Why: a client that POSTs to this stage is never answered — it blocks instead of getting a status. API Gateway's SNS service-proxy integration (the standard "publish without a Lambda" pattern) and the way an integration-role authorization failure is surfaced to the caller are both unusable against the emulator, and a hang is strictly worse for callers than a wrong status because no timeout/retry policy sees an outcome.

Fix: the execute-api data-plane request path for an `AWS`-type integration whose `uri` names SNS (`arn:aws:apigateway:<region>:sns:path//`): stage/route resolution plus AWS-service integration dispatch must ALWAYS terminate in a method response — the granted call with the rendered integration response template, the ungranted call with SNS's `AuthorizationError` mapped through the error integration response.

Related: this is NOT a new defect report of the integration itself — resolved issue 20261023T210711Z-apigateway-sns-sqs-both-api-gateway-to-sns.md claims the SNS integration dispatch fixed, but records in its own Resolution that "both recorded units still report `Check timed out after 60.0s`", that the fix was NOT CONFIRMED BY REPLAY, and that "a further report should be filed against whatever that poll waits on rather than against this integration". This is that report: after that fix, both checks still never get an accepted response. The remaining blocker is therefore upstream of or alongside the SNS dispatch — the stage invoke request itself, or the request-template/`passthroughBehavior: NEVER` + `integration.request.header.Content-Type: 'application/x-www-form-urlencoded'` leg, or the integration-response rendering — and should be traced from the invoke URL inward rather than from the SNS client outward.

## Evidence
Expected (AWS): POST <stage invoke URL>/publish with `Content-Type: application/json` and an `x-color` header -> HTTP 200 with body `{"published": true, "messageId": "<MessageId>"}`; the envelope subscriber's SNS JSON carries `TopicArn` = the topic, `Message` = the verbatim client body, and `MessageAttributes` == {"color": {"Type": "String", "Value": <header>}, "qty": {"Type": "Number", "Value": "7"}}; the raw subscriber's SQS message Body is the verbatim client body with `MessageAttributes` == {"color": {"StringValue": <header>, "DataType": "String"}, "qty": {"StringValue": "7", "DataType": "Number"}}. For the denied route: HTTP 403 surfaced as `{"published": false, "surfacedCode": "AuthorizationError", "surfacedType": "Sender", "surfacedMessage": <SNS's message naming the principal and the topic ARN>}` and nothing delivered.

Actual (emulator): no response the poll accepts ever arrives. Replay detail for BOTH checks is exactly:
  Error: Check timed out after 60.0s
with no status, headers or body recorded. The poll that hangs is the check's own first invoke of the freshly deployed stage:
  _, _, t = _poll_http(url, accept=lambda s, t: s == 200 and '"published": true' in t, body=raw1, headers={"Content-Type": "application/json", "x-color": color}, description="SNS Publish through the AWS non-proxy integration")
and `_poll_http` only raises after 240 s, so the harness's 60 s cap is reached first — i.e. the stage produced nothing the accept() admits (and no HTTPError either, since 4xx/5xx are returned as results, not raised).

Integration under test (from the check source, the shape the emulator must answer for):
  put_integration(type="AWS", integrationHttpMethod="POST", uri="arn:aws:apigateway:<region>:sns:path//", credentials=<role arn>, passthroughBehavior="NEVER", requestParameters={"integration.request.header.Content-Type": "'application/x-www-form-urlencoded'"}, requestTemplates={"application/json": "Action=Publish&TopicArn=...&Message=$util.urlEncode($input.body)&MessageAttributes.entry.1.Name=color&..."})
  put_integration_response(statusCode="200", responseTemplates={"application/json": '{"published": true, "messageId": "$input.path(\'$.PublishResponse.PublishResult.MessageId\')"}'})

Note on values: the topic ARN, queue URLs, role ARNs, api id, stage host/port, the generated `color` token and the MessageId differ every run and are not requirements. The requirement is that the stage always answers, with the statuses, codes and attribute shapes above.

## Steps to Reproduce
1. sns.create_topic; create two SQS queues, give each a policy admitting sns.amazonaws.com for the topic, and subscribe one with RawMessageDelivery=false and one with RawMessageDelivery=true plus FilterPolicy {"color": [<token>]}.
2. iam.create_role trusted by apigateway.amazonaws.com with an inline Allow of sns:Publish on the topic (and, for the second check, a second role holding only sns:GetTopicAttributes).
3. apigateway: create a REGIONAL REST API, a /publish resource, put_method POST authorizationType=NONE with requestParameters {"method.request.header.x-color": True}, and the AWS-type SNS integration shown in Evidence (credentials = the role, passthroughBehavior NEVER, the Action=Publish request template, the integration response template). create_deployment + create_stage "live".
4. POST the stage invoke URL /publish with Content-Type: application/json, an x-color header and a JSON body {"order": "...", "qty": 7}.
5. Observed: no HTTP response the caller can accept — the request neither returns 200 with `"published": true` nor any 4xx/5xx; the caller blocks and the harness cuts the check off at 60 s. Expected: the 200/attribute behavior (and, on the role without sns:Publish, the 403 AuthorizationError) described in Evidence.
6. Or replay the two check ids in trace_ids.
