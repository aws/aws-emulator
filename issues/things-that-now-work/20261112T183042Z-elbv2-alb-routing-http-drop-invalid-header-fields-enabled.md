Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: elbv2.drop-invalid-header-fields-strips-the-header-before-the-target
Issue-Shape-Id: com.amazonaws.elasticloadbalancingv2#ElasticLoadBalancing_v10
Issue-Key: elbv2.drop-invalid-header-fields-strips-the-header-before-the-target|_poll_until#27|Timeout|time.sleep(interval_s)
Issue-Commit: 87f8763cddaacde10d4e73c0f6e88e76a96af320

# ALB routing.http.drop_invalid_header_fields.enabled flow produces no observable behavior: the Lambda-target data plane never answers and the check is cut off at the replay budget

> _Note: this issue was identified against the code base at commit `87f8763cddaacde10d4e73c0f6e88e76a96af320`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ModifyLoadBalancerAttributes

## Description
Cause 1 of 1: the end-to-end behavior of the Application Load Balancer attribute `routing.http.drop_invalid_header_fields.enabled` — refusing a non-boolean value without storing it, then stripping an invalid-named request header before the target while still serving the request — is not observable on the emulator at all. The whole scenario is cut off by the replay budget without a single served response. Failing check id: elbv2.drop-invalid-header-fields-strips-the-header-before-the-target.

NOTE: open issue 20261112T075102Z-elbv2-drop-invalid-header-fields-alb-flow-never-completes.md describes the same flow, but the replay report did NOT mark this check as OWNED by it (its recorded Issue-Key no longer matches the check's current replay key), so that issue does not carry this check's replay key and this one is filed to carry it. A fixer should treat the two as the same root cause.

AWS: (a) ModifyLoadBalancerAttributes accepts `routing.http.drop_invalid_header_fields.enabled` only with the value `true` or `false`. Smithy: LoadBalancerAttribute$Key documents the key as "Indicates whether HTTP headers with invalid header fields are removed by the load balancer (true) or routed to targets (false). The default is false"; ModifyLoadBalancerAttributes input members `LoadBalancerArn` and `Attributes` (LoadBalancerAttributes, @length 0..20) are required; its modeled errors are InvalidConfigurationRequestException (HTTP 400) and LoadBalancerNotFoundException (HTTP 400). A value outside the attribute's domain is answered with the service-common `ValidationError` at HTTP 400, message `The value of '<attribute key>' must be 'true' or 'false', but was '<submitted value>'` — fixed text "The value of '" … "' must be 'true' or 'false', but was '" … "'", with the attribute key and the submitted value taken verbatim from the request (never the literal key/value of one recording). The refused value is not stored: DescribeLoadBalancerAttributes keeps reporting the prior value for that key. (b) With the attribute `true` the ALB removes request headers whose names are not valid HTTP tokens (e.g. a name containing underscores) BEFORE forwarding, still forwards the request, still answers the target's HTTP 200, and delivers the valid headers of the same request unchanged; with the attribute `false` (the default) both the invalid-named and the valid header reach the target. Recorded behavior and the ALB attributes documentation the check cites. (c) A `lambda`-type target group with a registered function is invoked per request and the ALB passes the client's headers in the invocation event's `headers`/`multiValueHeaders`; health checks are off by default for such a group, so target health stays `unused`.

Emulator: the replay yields exactly one result for this check — `Check timed out after 60.0s` — with no step-level status, error code or body. None of the scenario's steps (create internet-facing ALB and reach State=active, refuse the non-boolean attribute value, create the echoing Lambda and register it into a `lambda` target group, create the HTTP:80 listener, request through the published DNSName with the attribute true and then false) resolves inside the budget, so neither the strip-on-true behavior, the deliver-on-false control, nor the attribute read-back is ever observed. Hypothesis: the emulator's ALB data plane never answers a request forwarded to a `lambda` target group, so the check's bounded data-plane poll spins until the budget cuts it off; the attribute's domain validation is already implemented (see Traces) and is unlikely to be the stall.

Why: a client that enables `routing.http.drop_invalid_header_fields.enabled` to have the ALB sanitize header names gets neither the sanitization nor a served request — it hangs instead of receiving the target's HTTP 200, and cannot tell whether the attribute took effect. A client whose ALB forwards to a Lambda target receives no response at all where AWS returns the function's statusCode/body.

Fix: the elbv2 ALB data-plane request path — forwarding/invoking a `lambda` target group and answering the client with the function's response — plus the header-sanitization step on that path gated by `routing.http.drop_invalid_header_fields.enabled`, and the attribute's read-back through DescribeLoadBalancerAttributes. The attribute's domain check in the ModifyLoadBalancerAttributes handler is not the component to change.

Traces: elbv2.drop-invalid-header-fields-strips-the-header-before-the-target. Attribute-domain classification already present at services/service-com-amazonaws-elasticloadbalancingv2-elasticloadbalancing-v10/src/ops/modify_load_balancer_attributes.rs:193-239.

## Evidence
Expected (AWS):
1. modify_load_balancer_attributes(LoadBalancerArn=<alb>, Attributes=[{Key:'routing.http.drop_invalid_header_fields.enabled', Value:<non-boolean>}]) -> HTTP 400, Code `ValidationError`, Message "The value of 'routing.http.drop_invalid_header_fields.enabled' must be 'true' or 'false', but was '<the submitted value>'"; describe_load_balancer_attributes still reports the prior value for that key.
2. With the attribute 'true': GET http://<alb DNSName>/ carrying one invalid-named header (underscores) and one valid header -> HTTP 200 from the Lambda target; the echoed request headers contain the valid header with its value and do NOT contain the invalid-named one.
3. With the attribute 'false': the same request -> HTTP 200 and the echoed headers contain BOTH headers with their values.

Actual (emulator):
- Replay verdict: [mismatch] elbv2.drop-invalid-header-fields-strips-the-header-before-the-target
- Error detail, in full: `Check timed out after 60.0s`
- No step reported a status, an error code or a response body; the check's own bounded polls (attribute read-back, and the data-plane echo through the published DNSName) never satisfied their predicate before the replay cut the check off, so none of expectations 1-3 was reached.

## Steps to Reproduce
1. Boot the emulator and replay check `elbv2.drop-invalid-header-fields-strips-the-header-before-the-target` (suite simparity-behavioral-aws-v1, service heading `elbv2`).
2. Equivalently, by hand against the emulator:
   a. ec2 describe_vpcs / describe_subnets to pick a VPC and two AZ subnets; create a security group allowing HTTP:80 ingress.
   b. elbv2 create_load_balancer(Scheme='internet-facing', Subnets=[two subnets], SecurityGroups=[sg]); wait for State=active.
   c. elbv2 modify_load_balancer_attributes(LoadBalancerArn=<alb>, Attributes=[{Key:'routing.http.drop_invalid_header_fields.enabled', Value:'<non-boolean>'}]) — expect ValidationError with the message above; then describe_load_balancer_attributes and confirm the value was not stored.
   d. iam create_role for Lambda; lambda create_function with a handler echoing event['headers'] as JSON; wait for State=Active; lambda add_permission(Principal='elasticloadbalancing.amazonaws.com', SourceArn=<tg arn>).
   e. elbv2 create_target_group(TargetType='lambda'); register_targets(Targets=[{Id:<function arn>}]); create_listener(Port=80, Protocol='HTTP', DefaultActions=[forward to that target group]).
   f. GET http://<DNSName>/ with one underscore-named header and one valid header, and read the echoed headers.
   g. Set the attribute to 'true' and repeat (f): expect HTTP 200 with only the valid header echoed. Set it to 'false' and repeat (f): expect HTTP 200 with both headers echoed.
3. Observed: the flow never reaches a served response; the replay terminates the check at its 60 s budget with `Check timed out after 60.0s`.
