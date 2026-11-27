Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: elbv2__lambda.register-lambda-target-without-invoke-permission-rejected elbv2__lambda.invoke-permission-scoped-to-one-target-group-rejects-another elbv2__lambda.lambda-target-event-base64-encodes-bodies-by-content-type
Issue-Key: elbv2__lambda.register-lambda-target-without-invoke-permission-rejected|_refusal#16|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded: {result!r}")
Issue-Key: elbv2__lambda.invoke-permission-scoped-to-one-target-group-rejects-another|_refusal#18|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded: {result!r}")
Issue-Key: elbv2__lambda.lambda-target-event-base64-encodes-bodies-by-content-type|run#13|AssertionError|assert event.get("isBase64Encoded") is True, (
Issue-Commit: 96db52f5b4a4b596ab227bd8becc0930845733a7

# elbv2__lambda: 2 net-new causes — RegisterTargets never checks the Lambda invoke grant, and the ALB's Lambda event sets isBase64Encoded wrongly for non-text bodies

> _Note: this issue was identified against the code base at commit `96db52f5b4a4b596ab227bd8becc0930845733a7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RegisterTargets

## Description
Two net-new root causes on the ALB↔Lambda path.

Cause 1 of 2: RegisterTargets accepts a Lambda target the elasticloadbalancing principal is not permitted to invoke.
AWS: Smithy (elasticloadbalancingv2) — RegisterTargets models InvalidTargetException (awsQueryError InvalidTarget, httpError 400) and the operation verifies, at registration time, that the function's resource policy grants lambda:InvokeFunction to elasticloadbalancing.amazonaws.com for THAT target group. Registration is refused with HTTP 400 and the message `elasticloadbalancing principal does not have permission to invoke <function-arn> from target group <target-group-arn>` (fixed text; the function ARN and target-group ARN come from the request), reported with error code AccessDenied. A grant whose SourceArn names a different target group does not satisfy the check.
Emulator: both the ungranted registration and the registration into a second target group the grant does not name answer HTTP 200 with an empty body, so the target is registered without any permission evaluation.
Why: a client's permission wiring is never validated — a target group that cannot invoke its Lambda in AWS registers cleanly here, and the client's negative control for a missing/ mis-scoped grant passes wrongly.
Fix: the RegisterTargets handler's lambda-target validation (resolve the function's resource policy and match principal, action and SourceArn against the target group).
Traces: elbv2__lambda.register-lambda-target-without-invoke-permission-rejected, elbv2__lambda.invoke-permission-scoped-to-one-target-group-rejects-another.

Cause 2 of 2: the ALB→Lambda request event does not base64-encode bodies whose content type is not text.
AWS: documented ALB Lambda-target event format — the ALB passes a request body through as text with isBase64Encoded false only for text/*, application/json, application/javascript and XML content with no Content-Encoding; any other body (for example application/x-www-form-urlencoded, or text/plain carrying Content-Encoding: gzip) arrives base64-encoded with isBase64Encoded true.
Emulator: a body posted with Content-Type: application/x-www-form-urlencoded arrives with isBase64Encoded=False and the raw text body, so the flag and the encoding of body do not follow content type. The application/json positive control passed.
Why: a Lambda target written against AWS decodes body when isBase64Encoded is true; here it receives the flag false for a body AWS would have encoded, so a correct handler mis-parses (or, after a fix on the client side, double-decodes) every non-text request.
Fix: the ALB data-plane Lambda-target event construction (content-type/Content-Encoding classification that sets isBase64Encoded and encodes body).
Traces: elbv2__lambda.lambda-target-event-base64-encodes-bodies-by-content-type.

## Evidence
Cause 1: `AssertionError: expected the call to be refused, but it succeeded: {'ResponseMetadata': {'HTTPStatusCode': 200, ...}}` for RegisterTargets(TargetGroupArn=<tg>, Targets=[{'Id': <lambda alias arn>}]); expected ('AccessDenied', 'elasticloadbalancing principal does not have permission to invoke <function-arn> from target group <target-group-arn>').
Cause 2: `AssertionError: a body sent with {'Content-Type': 'application/x-www-form-urlencoded'} arrived with isBase64Encoded=False and body 'name=ada&lang=en'` — expected isBase64Encoded true with base64(body); the same is required for {'Content-Type': 'text/plain', 'Content-Encoding': 'gzip'} (the payload text itself is a per-run value).

## Steps to Reproduce
1. Build and boot the emulator from this checkout and replay the behavioral suite.
2. Cause 1: create a Lambda function and alias, create a lambda-type target group with NO lambda:add-permission grant, and call elbv2 RegisterTargets with the alias ARN — expect a 400 AccessDenied refusal. Then grant lambda:InvokeFunction to elasticloadbalancing.amazonaws.com with SourceArn=<target group A> and call RegisterTargets into target group B with the same alias — expect the same refusal.
3. Cause 2: register a Lambda target that echoes its event behind an ALB listener and POST the same payload three times with headers {'Content-Type': 'application/json'}, {'Content-Type': 'application/x-www-form-urlencoded'} and {'Content-Type': 'text/plain', 'Content-Encoding': 'gzip'}; inspect isBase64Encoded and body in the echoed event.
