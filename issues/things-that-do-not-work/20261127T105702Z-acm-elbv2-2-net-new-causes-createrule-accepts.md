Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: acm__elbv2.create-rule-user-authentication-action-requires-https-listener-and-highest-order acm__elbv2.https-listener-omitted-sslpolicy-gets-default-security-policy
Issue-Key: acm__elbv2.create-rule-user-authentication-action-requires-https-listener-and-highest-order|expect_error#21|AssertionError|raise AssertionError(f"expected {(code, message)!r}, but the call succeeded: {result!r}")
Issue-Key: acm__elbv2.https-listener-omitted-sslpolicy-gets-default-security-policy|run#15|KeyError|assert created["SslPolicy"] == _DEFAULT_SSL_POLICY, (
Issue-Commit: 96db52f5b4a4b596ab227bd8becc0930845733a7

# acm__elbv2: 2 net-new causes — CreateRule accepts an authenticate-oidc action on an HTTP listener, and an HTTPS listener created without SslPolicy reports no security policy

> _Note: this issue was identified against the code base at commit `96db52f5b4a4b596ab227bd8becc0930845733a7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateRule

## Description
Two net-new root causes on the ELBv2 listener plane.

Cause 1 of 2: CreateRule accepts an authenticate-oidc action on an HTTP listener.
AWS: Smithy (elasticloadbalancingv2) — CreateRule models InvalidLoadBalancerActionException, awsQueryError code InvalidLoadBalancerAction, httpError 400. Authentication actions are supported only on HTTPS listeners, so a rule whose Actions contain Type 'authenticate-oidc' on an HTTP listener is refused with HTTP 400 InvalidLoadBalancerAction and the message `Actions of type 'authenticate-oidc' are supported only on HTTPS listeners` (fixed text; the action type named in it is the request's action Type). The same operation also refuses a forward action that is not ordered last with `An action of type 'forward' must have a higher order than all other actions`.
Emulator: the call succeeds with HTTP 200 and returns a created rule carrying the authenticate-oidc action (Order 1) plus the forward action, so the rule is persisted on an HTTP listener. The check stops at that first assertion, so the forward-order refusal above was not reached and is not claimed here.
Why: a client validating an OIDC-protected listener configuration gets an acceptance AWS would never give; the emulator then routes requests through an authentication action that cannot exist on a plaintext listener, and the client's own negative test for the misconfiguration passes wrongly.
Fix: CreateRule action validation (the listener-protocol/action-type legality check in the CreateRule handler).
Traces: acm__elbv2.create-rule-user-authentication-action-requires-https-listener-and-highest-order.

Cause 2 of 2: an HTTPS listener created without SslPolicy reports no security policy at all.
AWS: Smithy — CreateListener's SslPolicy member is optional (SslPolicyName) and the Listener returned by CreateListener/DescribeListeners carries SslPolicy. For an HTTPS listener created with SslPolicy omitted, AWS fills in the default security policy and the created listener reads back with SslPolicy = `ELBSecurityPolicy-2016-08`.
Emulator: the created listener is returned with no SslPolicy member at all (the check's `created['SslPolicy']` raises KeyError), so no default security policy is applied or published.
Why: a client reading back its listener to learn which TLS policy is in force gets a missing field, and any code that keys on listener['SslPolicy'] breaks; the listener's negotiated TLS bounds are also unspecified.
Fix: the CreateListener handler's listener-defaulting step (and the listener serializer that emits SslPolicy) for HTTPS/TLS protocols.
Traces: acm__elbv2.https-listener-omitted-sslpolicy-gets-default-security-policy.

## Evidence
Cause 1: `AssertionError: expected ('InvalidLoadBalancerAction', "Actions of type 'authenticate-oidc' are supported only on HTTPS listeners"), but the call succeeded: {'Rules': [{'RuleArn': ..., 'Priority': '1', 'Actions': [{'Type': 'authenticate-oidc', 'AuthenticateOidcConfig': {...}, 'Order': 1}, {'Type': 'forward', ...}]}], 'ResponseMetadata': {'HTTPStatusCode': 200, ...}}` — the ARNs, client id and secret in that body are per-run values, not part of the requirement.
Cause 2: `KeyError: 'SslPolicy'` raised on `created["SslPolicy"] == 'ELBSecurityPolicy-2016-08'` for an HTTPS listener created with SslPolicy omitted.

## Steps to Reproduce
1. Build and boot the emulator from this checkout and replay the behavioral suite.
2. Cause 1: create an internal ALB, create an HTTP listener on port 80, then CreateRule(ListenerArn=<http listener>, Priority=1, Conditions=[path-pattern /auth], Actions=[{Type: 'authenticate-oidc', AuthenticateOidcConfig: {...}, Order: 1}, {Type: 'forward', TargetGroupArn: <tg>, Order: 2}]) and observe the 200 instead of a 400 InvalidLoadBalancerAction.
3. Cause 2: request an ACM/IAM server certificate, then CreateListener(Protocol='HTTPS', Port=443, Certificates=[...], DefaultActions=[fixed-response]) with SslPolicy omitted and read SslPolicy off the created listener (and off DescribeListeners).
