Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: elbv2.describe-load-balancer-nonexistent-arn-rejected elbv2.describe-load-balancers-unknown-arn-fails elbv2.delete-load-balancer-then-not-found elbv2.delete-nonexistent-load-balancer-is-idempotent elbv2.delete-load-balancer-with-deletion-protection-rejected elbv2.deletion-protection-blocks-delete elbv2.delete-load-balancer-name-and-arn-lookups-refused-then-reused elbv2.deregister-target-draining-honors-delay elbv2.e2e-alb-target-routing-lifecycle elbv2.op-contract-delete-load-balancer elbv2.target-group-delete-blocked-while-referenced-then-freed-by-lb-delete elbv2.op-contract-describe-target-groups elbv2.op-contract-delete-target-group elbv2.delete-target-group-then-not-found elbv2.alb-listener-target-group-lifecycle elbv2.modify-rule-moves-the-in-use-hold-from-one-target-group-to-another elbv2.weighted-forward-rule-holds-both-target-groups-in-use-until-it-is-deleted elbv2.e2e-alb-traffic-lifecycle-flow elbv2.e2e-alb-traffic-routing-lifecycle elbv2.e2e-blue-green-weighted-cutover-flow elbv2.e2e-weighted-target-group-shift-flow elbv2.e2e-target-group-deletion-while-referenced-rejected elbv2.asgdx-tcp-health-check-with-path-rejected elbv2.asgdx-unhealthy-threshold-above-ten-rejected elbv2.create-listener-unsupported-protocol-version-rejected elbv2.create-target-group-health-check-protocol-incompatible-rejected elbv2.create-target-group-malformed-health-check-path-rejected elbv2.http-header-condition-matches-values-case-insensitively elbv2.query-string-condition-matches-case-insensitively elbv2.path-condition-evaluates-the-normalized-uri elbv2.create-rule-source-ip-host-header-method-and-header-name-format-rejected elbv2.target-group-anomaly-mitigation-requires-weighted-random-algorithm-rejected elbv2.internet-facing-alb-name-resolves-to-exactly-its-node-interface-addresses elbv2.alb-zonal-name-answers-its-node-zone-and-unenabled-zone-rejected-with-nxdomain elbv2.elbv2attr-deep-elbv2-1-server-header-removed-and-hsts-injected
Issue-Shape-Id: com.amazonaws.elasticloadbalancingv2#ElasticLoadBalancing_v10
Issue-Key: elbv2.describe-load-balancer-nonexistent-arn-rejected|run#14|AssertionError|assert message == "One or more load balancers not found", f"MESSAGE={message!r}"
Issue-Key: elbv2.describe-load-balancers-unknown-arn-fails|run#19|AssertionError|assert message == "One or more load balancers not found", f"MESSAGE={message!r}"
Issue-Key: elbv2.delete-load-balancer-then-not-found|run#16|AssertionError|assert message == "One or more load balancers not found", f"MSG={message!r}"
Issue-Key: elbv2.delete-nonexistent-load-balancer-is-idempotent|run#10|AssertionError|assert pre_message == "One or more load balancers not found", pre_message
Issue-Key: elbv2.delete-load-balancer-with-deletion-protection-rejected|run#16|AssertionError|assert message_after == "One or more load balancers not found", f"MSG={message_after!r}"
Issue-Key: elbv2.deletion-protection-blocks-delete|run#28|AssertionError|assert message2 == "One or more load balancers not found", message2
Issue-Key: elbv2.delete-load-balancer-name-and-arn-lookups-refused-then-reused|run#16|AssertionError|assert arn_message == "One or more load balancers not found", f"unexpected message {arn_message!r}"
Issue-Key: elbv2.deregister-target-draining-honors-delay|run#38|AssertionError|assert refused.response["Error"].get("Message") == expected_message, f'expected the message {expected_message!r}, got {refused.response["Error"].get("Message")!
Issue-Key: elbv2.e2e-alb-target-routing-lifecycle|run#29|AssertionError|assert message == "One or more load balancers not found", f"MSG={message!r}"
Issue-Key: elbv2.op-contract-delete-load-balancer|run#12|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: elbv2.target-group-delete-blocked-while-referenced-then-freed-by-lb-delete|run#25|AssertionError|assert gone_message == "One or more load balancers not found", f"MESSAGE={gone_message!r}"
Issue-Key: elbv2.op-contract-describe-target-groups|run#21|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: elbv2.op-contract-delete-target-group|run#10|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: elbv2.delete-target-group-then-not-found|run#15|AssertionError|assert message == "One or more target groups not found", f"MESSAGE={message!r}"
Issue-Key: elbv2.alb-listener-target-group-lifecycle|run#71|AssertionError|assert (code, msg) == ("TargetGroupNotFound", message) and isinstance(http, int) and 400 <= http < 500, (
Issue-Key: elbv2.modify-rule-moves-the-in-use-hold-from-one-target-group-to-another|run#28|AssertionError|assert (code, msg) == ("TargetGroupNotFound", message) and isinstance(http, int) and 400 <= http < 500, (
Issue-Key: elbv2.weighted-forward-rule-holds-both-target-groups-in-use-until-it-is-deleted|run#29|AssertionError|assert (code, msg) == ("TargetGroupNotFound", "One or more target groups not found") and isinstance(http, int) and 400 <= http < 500, (
Issue-Key: elbv2.e2e-alb-traffic-lifecycle-flow|run#71|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: elbv2.e2e-alb-traffic-routing-lifecycle|run#60|AssertionError|assert refused.response["Error"].get("Message") == expected_message, f'expected the message {expected_message!r}, got {refused.response["Error"].get("Message")!
Issue-Key: elbv2.e2e-blue-green-weighted-cutover-flow|run#57|AssertionError|assert gone_message == expected_gone, f"expected message {expected_gone!r}, got {gone_message!r}"
Issue-Key: elbv2.e2e-weighted-target-group-shift-flow|run#46|AssertionError|assert tg_message == expected, f"expected message {expected!r}, got {tg_message!r}"
Issue-Key: elbv2.e2e-target-group-deletion-while-referenced-rejected|run#29|AssertionError|assert gone_message == expected_gone, f"expected message {expected_gone!r}, got {gone_message!r}"
Issue-Key: elbv2.asgdx-tcp-health-check-with-path-rejected|run#12|AssertionError|assert left_message == "One or more target groups not found", f"MSG={left_message!r}"
Issue-Key: elbv2.asgdx-unhealthy-threshold-above-ten-rejected|run#13|AssertionError|assert left_msg == want_left, f"the lookup of {bad!r} said {left_msg!r}, expected {want_left!r}"
Issue-Key: elbv2.create-listener-unsupported-protocol-version-rejected|run#13|AssertionError|assert not_found_message == expected_not_found, (
Issue-Key: elbv2.create-target-group-health-check-protocol-incompatible-rejected|run#10|AssertionError|assert describe_message == "One or more target groups not found", f"MSG={describe_message!r}"
Issue-Key: elbv2.create-target-group-malformed-health-check-path-rejected|run#10|AssertionError|assert not_found_message == "One or more target groups not found", f"MSG={not_found_message!r}"
Issue-Key: elbv2.http-header-condition-matches-values-case-insensitively|run#8|AssertionError|assert (status, body.strip()) == (200, "BY-HEADER"), (
Issue-Key: elbv2.query-string-condition-matches-case-insensitively|run#8|AssertionError|assert (status, body.strip()) == (200, "BY-QUERY"), (
Issue-Key: elbv2.path-condition-evaluates-the-normalized-uri|run#7|AssertionError|assert got == (403, "ADMIN"), (
Issue-Key: elbv2.create-rule-source-ip-host-header-method-and-header-name-format-rejected|_error#10|AssertionError|raise AssertionError(f"expected the rule to be refused, but it was created: {result!r}")
Issue-Key: elbv2.target-group-anomaly-mitigation-requires-weighted-random-algorithm-rejected|expect_error#8|AssertionError|raise AssertionError(f"expected {(code, message)!r}, but the call succeeded: {result!r}")
Issue-Key: elbv2.internet-facing-alb-name-resolves-to-exactly-its-node-interface-addresses|_resolve#15|AssertionError|assert body["ok"], f"the relay could not exchange the query for {name!r}: {body['error']!r}"
Issue-Key: elbv2.alb-zonal-name-answers-its-node-zone-and-unenabled-zone-rejected-with-nxdomain|_resolve#20|AssertionError|assert body["ok"], f"the relay could not exchange the query for {name!r}: {body['error']!r}"
Issue-Key: elbv2.elbv2attr-deep-elbv2-1-server-header-removed-and-hsts-injected|_poll_until#27|Timeout|time.sleep(interval_s)
Issue-Commit: 96db52f5b4a4b596ab227bd8becc0930845733a7

# elbv2: 9 net-new causes — not-found refusals interpolate the requested identifiers, rule conditions match case-sensitively and unnormalized, and three validation/data-plane gaps

> _Note: this issue was identified against the code base at commit `96db52f5b4a4b596ab227bd8becc0930845733a7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeLoadBalancers

## Description
Nine distinct net-new root causes in the ELBv2 control plane and ALB data plane. Causes 1 and 2 share a wording defect in the not-found refusal path but are stated separately because they live in two different lookups; causes 3-9 are independent.

Cause 1 of 9: DescribeLoadBalancers/DeleteLoadBalancer LoadBalancerNotFound message interpolates the requested ARN.
AWS: Smithy (elasticloadbalancingv2, awsQuery) — DescribeLoadBalancers and DeleteLoadBalancer model LoadBalancerNotFoundException, awsQueryError code LoadBalancerNotFound, httpError 400. For a lookup by LoadBalancerArns (and for a delete of an ARN that no longer exists) AWS answers HTTP 400 LoadBalancerNotFound with the FIXED text `One or more load balancers not found` — no identifier is echoed. (Recorded behavior shows the interpolated form `Load balancers '[<name>]' not found` is AWS's wording only for DescribeLoadBalancers by Names; see cloudformation__elbv2__s3.deploy-alb-access-logs-tls-only-bucket-refuses-a-grant-to-a-non-elb-account, which asserts that name-lookup form.)
Emulator: every LoadBalancerNotFound refusal is built by interpolating the requested identifiers, e.g. `Load balancers '[arn:aws:elasticloadbalancing:<region>:<account>:loadbalancer/app/<name>/<id>]' not found`. Fixed text in the requirement: `One or more load balancers not found`; the variable part the emulator wrongly adds is the request's LoadBalancerArns list.
Why: a client that matches on the refusal message (log scrapers, idempotent-delete wrappers, CFN-style resource handlers that classify NotFound by message) never recognises the refusal, and a message that leaks the queried ARN diverges from every AWS SDK/CLI transcript.
Fix: the ELBv2 not-found refusal construction for load-balancer lookups — the handlers of DescribeLoadBalancers/DeleteLoadBalancer (and the shared load-balancer resolution helper they call) must emit the fixed message for ARN-form lookups and keep the interpolated form only for Names-form lookups.
Traces: elbv2.describe-load-balancer-nonexistent-arn-rejected, elbv2.describe-load-balancers-unknown-arn-fails, elbv2.delete-load-balancer-then-not-found, elbv2.delete-nonexistent-load-balancer-is-idempotent, elbv2.delete-load-balancer-with-deletion-protection-rejected, elbv2.deletion-protection-blocks-delete, elbv2.delete-load-balancer-name-and-arn-lookups-refused-then-reused, elbv2.deregister-target-draining-honors-delay, elbv2.e2e-alb-target-routing-lifecycle, elbv2.op-contract-delete-load-balancer, elbv2.target-group-delete-blocked-while-referenced-then-freed-by-lb-delete.
Related open issues filed under other service headings for the same wording defect (none carries these replay keys): 20261127T091242Z-apigateway-cloudformation-ec2-elbv2-elbv2-describeloadbalancers-loadbalancernotfound-message-names-the-requested-arn.md, 20261127T091242Z-cloudformation-cloudfront-ec2-elbv2-elbv2-describeloadbalancers-loadbalancernotfound-message-echoes-the-requested-arn.md, 20261127T091242Z-cloudformation-ec2-elbv2-elbv2-both-not-found-refusals-describeloadbalancers-describetargetgroups-echo.md.

Cause 2 of 9: DescribeTargetGroups/DeleteTargetGroup TargetGroupNotFound message interpolates the requested ARN or name.
AWS: Smithy — DescribeTargetGroups models TargetGroupNotFoundException (awsQueryError TargetGroupNotFound, httpError 400). For a lookup by TargetGroupArns or by Names that resolves nothing, AWS answers HTTP 400 TargetGroupNotFound with the FIXED text `One or more target groups not found`.
Emulator: answers 400 TargetGroupNotFound with `Target groups '[<arn-or-name>]' not found`, interpolating whatever the request asked for (both the ARN form and the bare-name form are affected).
Why: same as cause 1 — the code is right, the message is not, so message-matching clients and recorded-transcript comparisons break.
Fix: the ELBv2 not-found refusal construction for target-group lookups (DescribeTargetGroups/DeleteTargetGroup handlers and the shared target-group resolution helper).
Traces: elbv2.op-contract-describe-target-groups, elbv2.op-contract-delete-target-group, elbv2.delete-target-group-then-not-found, elbv2.alb-listener-target-group-lifecycle, elbv2.modify-rule-moves-the-in-use-hold-from-one-target-group-to-another, elbv2.weighted-forward-rule-holds-both-target-groups-in-use-until-it-is-deleted, elbv2.e2e-alb-traffic-lifecycle-flow, elbv2.e2e-alb-traffic-routing-lifecycle, elbv2.e2e-blue-green-weighted-cutover-flow, elbv2.e2e-weighted-target-group-shift-flow, elbv2.e2e-target-group-deletion-while-referenced-rejected, elbv2.asgdx-tcp-health-check-with-path-rejected, elbv2.asgdx-unhealthy-threshold-above-ten-rejected, elbv2.create-listener-unsupported-protocol-version-rejected, elbv2.create-target-group-health-check-protocol-incompatible-rejected, elbv2.create-target-group-malformed-health-check-path-rejected.
Related open issue (different heading, does not carry these keys): 20261127T091242Z-ec2-elbv2-elbv2-the-targetgroupnotfound-refusal-message-interpolates-the-requested.md.

Cause 3 of 9: an http-header listener-rule condition is matched case-sensitively on the value.
AWS: documented ALB rule-condition semantics (rule-condition-types) — http-header condition values are compared case-insensitively, so a condition value `prod` matches a request header `X-Env: PROD` and the rule's action is taken.
Emulator: the request with `X-Env: PROD` falls through to the listener default action and is answered 404 with body `DEFAULT`.
Why: a client routing on header values (environment/tenant switches) gets the default action instead of the matched rule, so traffic lands on the wrong target group.
Fix: the ALB data-plane rule-evaluation component, http-header condition matching (case-folded comparison of condition values).
Traces: elbv2.http-header-condition-matches-values-case-insensitively.

Cause 4 of 9: a query-string listener-rule condition is matched case-sensitively on key/value.
AWS: query-string condition key/value pairs are matched case-insensitively, so `version=v1` matches `?version=V1`.
Emulator: `?version=V1` falls to the listener default and is answered 404 `DEFAULT`.
Why: same routing divergence as cause 3 for query-string based routing.
Fix: the ALB data-plane rule-evaluation component, query-string condition matching.
Traces: elbv2.query-string-condition-matches-case-insensitively.

Cause 5 of 9: a path-pattern condition is not evaluated against the normalized request URI, and the request is refused outright.
AWS: the ALB normalizes the request target before evaluating path-pattern conditions, so `GET /static/../admin` is evaluated as `/admin` and the rule matching `/admin` serves it.
Emulator: answers HTTP 400 with body `This request target cannot be served.` — the request never reaches rule evaluation.
Why: a client relying on URI normalization sees a hard 400 instead of the routed response; a security control expressed as a path rule is also not exercised as AWS exercises it.
Fix: the ALB data-plane request-target parsing/normalization step ahead of rule evaluation.
Traces: elbv2.path-condition-evaluates-the-normalized-uri.

Cause 6 of 9: CreateRule accepts malformed host-header, http-request-method and http-header conditions.
AWS: Smithy — CreateRule models ValidationError-class refusals at httpError 400. AWS refuses each malformed condition value with its own message: `Condition value '<value>' contains a character that is not valid` (host-header `localhost`, http-request-method `get`, `G*`), `Condition value for 'http-header' cannot contain more than 40 characters` (a header name over 40 characters), and `You cannot specify 'host' as an HTTP header name` (http-header name `Host`). Fixed text is each sentence; the variable part is the offending condition value taken from the request. Documented neighbours (`example.123`, `PURGE`, `X-*`) are accepted.
Emulator: the call succeeds with HTTP 200 and creates the rule (e.g. priority 4 with `host-header: localhost` is returned as a created rule), so no refusal is produced; the source-ip CIDR cases ahead of it are refused correctly.
Why: a client validating its routing configuration against AWS gets a rule AWS would never have created, and the rule then matches traffic in the emulator that cannot exist in production.
Fix: CreateRule/ModifyRule condition validation.
Traces: elbv2.create-rule-source-ip-host-header-method-and-header-name-format-rejected. (Related open issue, different root cause set: 20261104T094656Z-elbv2-ten-overly-permissive-control-plane-gaps-calls.md.)

Cause 7 of 9: ModifyTargetGroupAttributes accepts anomaly mitigation together with the least-outstanding-requests algorithm.
AWS: Smithy — ModifyTargetGroupAttributes models InvalidConfigurationRequestException (awsQueryError InvalidConfigurationRequest, httpError 400). Setting `load_balancing.algorithm.anomaly_mitigation=on` while `load_balancing.algorithm.type=least_outstanding_requests` is refused with HTTP 400 InvalidConfigurationRequest `You cannot enable both anomaly mitigation and least outstanding requests algorithm on a target group` (anomaly mitigation requires weighted_random).
Emulator: answers 200 and echoes the attribute list with both `load_balancing.algorithm.type=least_outstanding_requests` and `load_balancing.algorithm.anomaly_mitigation=on` stored.
Why: an illegal target-group configuration is persisted, so a client's config-validation path passes here and fails in AWS.
Fix: ModifyTargetGroupAttributes attribute-combination validation.
Traces: elbv2.target-group-anomaly-mitigation-requires-weighted-random-algorithm-rejected.

Cause 8 of 9: the emulator's Lambda runtime cannot exchange a DNS query, so load-balancer name resolution cannot be observed at all.
AWS: recorded behavior only — the check resolves the load balancer's DNS name through a relay Lambda, which in AWS performs an ordinary UDP/TCP DNS exchange and returns the answer set (the ALB name must answer with exactly its node interface addresses, and a zonal name must answer its own zone while an unenabled zone is NXDOMAIN).
Emulator: the relay fails inside the function with `OSError: [Errno 38] a Lambda function in this emulator has no host filesystem to reach`, so no answer is ever produced for either the load balancer name or its zonal name.
Why: a client cannot resolve a load balancer's published name at all through the same path AWS supports, and no zonal/NXDOMAIN behavior can be exercised.
Fix: the Lambda invoke/runtime component that executes function code (its socket and filesystem capability), not the ELBv2 handlers.
Traces: elbv2.internet-facing-alb-name-resolves-to-exactly-its-node-interface-addresses, elbv2.alb-zonal-name-answers-its-node-zone-and-unenabled-zone-rejected-with-nxdomain.

Cause 9 of 9: the response-header mutation scenario (Server header removed, HSTS injected) reaches no observable step.
AWS: recorded behavior only — with the relevant routing attributes set, the ALB strips the target's `Server` header and injects `Strict-Transport-Security` on the response it serves.
Emulator: the check produces no observation at all and is cut off at the harness's 60 s cap with no response recorded, so neither the removal nor the injection is ever served.
Why: a client that depends on ALB-applied response headers observes nothing from the emulator.
Fix: the ALB data-plane response path (header mutation driven by load-balancer attributes).
Traces: elbv2.elbv2attr-deep-elbv2-1-server-header-removed-and-hsts-injected. (Related open issues about the ALB data plane not answering, which do not carry this key: 20261116T083232Z-elbv2-2-net-new-causes-the-alb-data.md, 20261123T141857Z-elbv2-4-net-new-causes-an-alb-answers.md.)

## Evidence
Cause 1: expected ('LoadBalancerNotFound', 'One or more load balancers not found'); got ('LoadBalancerNotFound', "Load balancers '[arn:aws:elasticloadbalancing:us-west-2:000000000000:loadbalancer/app/<name>/<id>]' not found") at HTTP 400 (identifier values differ per run; the fixed text is what diverges).
Cause 2: expected ('TargetGroupNotFound', 'One or more target groups not found'); got ('TargetGroupNotFound', "Target groups '[<arn-or-name>]' not found") at HTTP 400.
Cause 3: `X-Env: PROD was served (404, 'DEFAULT'); the header condition 'prod' must match it case-insensitively`.
Cause 4: `?version=V1 was served (404, 'DEFAULT'); the query-string condition version=v1 must match it case-insensitively`.
Cause 5: `GET /static/../admin was answered (400, 'This request target cannot be served.'); after normalization it is /admin and the rule must serve it`.
Cause 6: `expected the rule to be refused, but it was created: {'Rules': [{... 'Priority': '4', 'Conditions': [{'Field': 'host-header', 'Values': ['localhost'] ...}]}], 'ResponseMetadata': {'HTTPStatusCode': 200 ...}}`; expected ValidationError/400 "Condition value 'localhost' contains a character that is not valid".
Cause 7: `expected ('InvalidConfigurationRequest', 'You cannot enable both anomaly mitigation and least outstanding requests algorithm on a target group'), but the call succeeded` with HTTPStatusCode 200 and the attribute list containing load_balancing.algorithm.type=least_outstanding_requests plus load_balancing.algorithm.anomaly_mitigation=on.
Cause 8: `the relay could not exchange the query for '<lb-name>': 'OSError: [Errno 38] a Lambda function in this emulator has no host filesystem to reach'` (both the plain and the us-west-2a zonal name).
Cause 9: `Error: Check timed out after 60.0s` with no response observed.

## Steps to Reproduce
1. Build and boot the emulator from this checkout and replay the behavioral suite.
2. Causes 1-2: CreateLoadBalancer + CreateTargetGroup, delete them, then call DescribeLoadBalancers(LoadBalancerArns=[deleted arn]) and DescribeTargetGroups(TargetGroupArns=[deleted arn]) / DescribeTargetGroups(Names=['never-created']) and compare Error.Message with 'One or more load balancers not found' / 'One or more target groups not found'.
3. Cause 3/4/5: create an internal ALB + HTTP listener with a fixed-response default (404 'DEFAULT'), add rules whose conditions are http-header X-Env=prod, query-string version=v1 and path-pattern /admin, then send `X-Env: PROD`, `?version=V1` and `GET /static/../admin`.
4. Cause 6: CreateRule with Conditions=[{host-header: ['localhost']}], then ['get'], ['G*'] for http-request-method, an http-header name of 41 characters, and http-header name 'Host'.
5. Cause 7: CreateTargetGroup, then ModifyTargetGroupAttributes with load_balancing.algorithm.type=least_outstanding_requests and load_balancing.algorithm.anomaly_mitigation=on.
6. Cause 8: invoke a Lambda function that opens a socket and exchanges a DNS query for the load balancer's published name.
7. Cause 9: set the ALB attributes that strip Server and inject Strict-Transport-Security and read the served response headers.
