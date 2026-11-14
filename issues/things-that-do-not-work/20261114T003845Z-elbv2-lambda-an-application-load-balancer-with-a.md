Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: elbv2__lambda.malformed-lambda-target-response-becomes-a-502-after-the-invocation elbv2__lambda.xsvc-elbv2-3-lambda-invoke-permission-removed-is-denied-with-502-until-regranted
Issue-Key: elbv2__lambda.malformed-lambda-target-response-becomes-a-502-after-the-invocation|_drain#24|Timeout|got = sqs.receive_message(
Issue-Key: elbv2__lambda.xsvc-elbv2-3-lambda-invoke-permission-removed-is-denied-with-502-until-regranted|_poll_until#54|Timeout|time.sleep(interval_s)
Issue-Commit: 2ac91d382b67c33b3b1ad8ceed805222f48956bf

# elbv2__lambda: an Application Load Balancer with a Lambda target never answers, so neither a malformed target response (502) nor a revoked lambda:InvokeFunction grant (502 until regranted) is observable

> _Note: this issue was identified against the code base at commit `2ac91d382b67c33b3b1ad8ceed805222f48956bf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RegisterTargets / ALB data-plane HTTP request to a Lambda target group

## Description
Cause 1 of 1: requests to an ALB whose target group is of type `lambda` never produce a response within the checks' budgets, so every behaviour that depends on the ALB invoking the function is unreachable.
AWS: a target group with TargetType `lambda` holding a registered function makes the load balancer invoke that function per request with the ALB event envelope and turn the handler's response into the HTTP response. When the handler returns a document that is not a valid ALB response (missing/!int statusCode, non-string body, bad headers shape), the load balancer answers HTTP 502 AFTER the invocation has happened. When the function's resource policy no longer allows elasticloadbalancing.amazonaws.com to lambda:InvokeFunction, the load balancer answers HTTP 502 for every request until the grant is restored, at which point it serves 200 again. Documented in the ELB user guide, 'Lambda functions as targets'; the elbv2 Smithy model describes RegisterTargets and the target-group shape, not the data-plane mapping, so the 502 behaviour is documented behavior rather than model text.
Emulator: neither check ever gets an answer — one exhausts its 60 s budget and the other its 600 s budget ('Check timed out after 60.0s' / 'Check timed out after 600.0s') with no HTTP response from the load balancer's DNS name, so no status code (502 or otherwise) is ever produced.
Why: a client that fronts a Lambda function with an ALB gets a hung request rather than a response. It cannot tell a malformed handler response from a permission problem from a healthy target, and the documented 502 contract — the single signal the ALB gives for both failure classes — never appears.
Fix: the elbv2 data plane's Lambda target-group dispatch path — it must invoke the registered function, map its response, answer 502 when the response document is malformed, and answer 502 when the function's resource policy denies the load balancer's invoke.
Traces: elbv2__lambda.malformed-lambda-target-response-becomes-a-502-after-the-invocation, elbv2__lambda.xsvc-elbv2-3-lambda-invoke-permission-removed-is-denied-with-502-until-regranted.

The open issue 20261110T143849Z-elbv2-lambda-a-lambda-target-whose-health-path.md covers the health-check leg of this family; it does not carry these two checks' replay keys.

## Evidence
elbv2__lambda.malformed-lambda-target-response-becomes-a-502-after-the-invocation — Error: Check timed out after 60.0s.
elbv2__lambda.xsvc-elbv2-3-lambda-invoke-permission-removed-is-denied-with-502-until-regranted — Error: Check timed out after 600.0s.
Expected per AWS: the ALB answers HTTP 502 for a malformed Lambda target response (with the invocation already having occurred), and HTTP 502 for every request while the function's resource policy withholds lambda:InvokeFunction from elasticloadbalancing.amazonaws.com, returning to 200 once the grant is re-added.

## Steps to Reproduce
1. Create an ALB, a target group with TargetType lambda, and register a Lambda function; grant elasticloadbalancing.amazonaws.com lambda:InvokeFunction.
2. GET the load balancer's DNS name with a handler that returns a valid ALB response: AWS serves 200; the emulator never answers.
3. Point the target at a handler returning a malformed document: AWS answers 502; the emulator never answers.
4. RemovePermission for the load balancer and GET again: AWS answers 502 until the permission is re-added, then 200; the emulator never answers.
