Issue-Source: trace-sweep-divergence
Issue-Trace: elasticloadbalancing-alb-forwards-health-checks-to-lambda-target
Issue-Step: task#18
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: elasticloadbalancing-alb-forwards-health-checks-to-lambda-target elbv2-describe-target-health-include-anomaly-detection elasticloadbalancing-set-security-groups-round-trip elbv2env-alb-create-provisioning-and-subnet-reconfig elbv2err-nlb-elastic-ip-rejections elbv2err-create-load-balancer-input-rejections
Issue-Shape-Id: com.amazonaws.elasticloadbalancingv2#ElasticLoadBalancing_v10
Issue-Key: elasticloadbalancing|DescribeTargetHealth|2xx->2xx|$.DescribeTargetHealthResponse.DescribeTargetHealthResult.TargetHealthDescriptions.member.AdministrativeOverride
Issue-Key: elasticloadbalancing|DescribeTargetHealth|2xx->2xx|$.DescribeTargetHealthResponse.DescribeTargetHealthResult.TargetHealthDescriptions.member.Target.AvailabilityZone
Issue-Key: elasticloadbalancing|DescribeTargetHealth|2xx->2xx|$.DescribeTargetHealthResponse.DescribeTargetHealthResult.TargetHealthDescriptions.member.AnomalyDetection
Issue-Key: elasticloadbalancing|DescribeLoadBalancers|2xx->2xx|$.DescribeLoadBalancersResponse.DescribeLoadBalancersResult.LoadBalancers.member.DNSName
Issue-Key: elasticloadbalancing|CreateLoadBalancer|2xx->2xx|$.CreateLoadBalancerResponse.CreateLoadBalancerResult.LoadBalancers.member.DNSName
Issue-Key: elasticloadbalancing|CreateLoadBalancer|4xx->2xx|$.CreateLoadBalancerResponse.CreateLoadBalancerResult.LoadBalancers.member.DNSName
Issue-Related: 20261029T185946Z-elasticloadbalancing-elbv2-accepts-invalid-and-prohibited-requests-with-http.md
Issue-Commit: 622e0f603a4886f3f4d7e4980155df7d3d956f5b

# elbv2 responses omit modeled target-health fields, advertise an unreachable DNSName, and skip the internet-gateway subnet check

> _Note: this issue was identified against the code base at commit `622e0f603a4886f3f4d7e4980155df7d3d956f5b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeTargetHealth

## Description
5 net-new causes in the elbv2 (service token `elasticloadbalancing`) surface: three in the DescribeTargetHealth / DescribeLoadBalancers / CreateLoadBalancer serializers and state projection, and one missing validation in the CreateLoadBalancer handler. Each cause must be closed before this issue is submitted.

Cause 1 of 5: DescribeTargetHealth omits AdministrativeOverride and Target.AvailabilityZone for a registered target.
AWS: Smithy: DescribeTargetHealth output TargetHealthDescriptions -> TargetHealthDescription has members Target, HealthCheckPort, TargetHealth, AnomalyDetection and AdministrativeOverride; AdministrativeOverride is a structure of {State: TargetAdministrativeOverrideStateEnum, Reason: TargetAdministrativeOverrideReasonEnum, Description: Description}, and TargetDescription.AvailabilityZone (target ZoneName) is documented as "An Availability Zone or `all`", whose only supported value for a `lambda` target type is `all`. The recording shows AWS serializing, for a healthy target with no override in effect, AdministrativeOverride = {State: no_override, Reason: AdministrativeOverride.NoOverride, Description: No override is currently active on target} together with Target.AvailabilityZone = all for the lambda target.
Emulator: both members are absent from the XML — $.DescribeTargetHealthResponse.DescribeTargetHealthResult.TargetHealthDescriptions.member.AdministrativeOverride is missing (None) and the same member's Target.AvailabilityZone is missing (None); the status is 200 and the rest of the description matches.
Why: a client that reads AdministrativeOverride to decide whether a target is administratively drained (the usual pre-deploy/drain check) sees no override structure at all and cannot distinguish "no override" from "field not reported"; SDK models that mark the member non-optional on deserialization get a null dereference. Target.AvailabilityZone missing means a caller cannot tell which zone (or `all`) the registration applies to, so round-tripping a DescribeTargetHealth result back into RegisterTargets/DeregisterTargets loses the zone scope.
Fix: the DescribeTargetHealth serializer plus the target-registration state it projects from — the registration must carry its AvailabilityZone (`all` for lambda targets and for ALB target groups with cross-zone load balancing on) and the response must always emit an AdministrativeOverride structure, defaulting to the no-override state when nothing is overridden.
Traces: elasticloadbalancing-alb-forwards-health-checks-to-lambda-target#task#18.

Cause 2 of 5: DescribeTargetHealth omits AnomalyDetection even when the caller asks for it via Include.
AWS: Smithy: DescribeTargetHealth input member Include (ListOfDescribeTargetHealthIncludeOptions) selects optional blocks; output member AnomalyDetection is a structure {Result: AnomalyResultEnum, MitigationInEffect: MitigationInEffectEnum} documented as "If no anomalies were detected, the result is `normal`". The recording shows AWS answering a request that includes the anomaly-detection option with AnomalyDetection = {Result: normal, MitigationInEffect: no} on each target health description.
Emulator: answers 200 but $.DescribeTargetHealthResponse.DescribeTargetHealthResult.TargetHealthDescriptions.member.AnomalyDetection is missing (None), i.e. the Include option is accepted and then ignored.
Why: the Include parameter's only observable effect is the extra block, so a client that requests anomaly detection gets a silently truncated answer; code that branches on Result == anomalous / MitigationInEffect can never observe the healthy baseline and either crashes on the absent structure or reports "unknown" for every target.
Fix: the DescribeTargetHealth handler's Include handling and its serializer — when the anomaly-detection include option is present, emit AnomalyDetection with Result `normal` and MitigationInEffect `no` for targets with no detected anomaly.
Traces: elbv2-describe-target-health-include-anomaly-detection#task#10.

Cause 3 of 5: DescribeLoadBalancers reports a DNSName the emulator does not serve.
AWS: Smithy: LoadBalancer.DNSName is documented as "The public DNS name of the load balancer" — i.e. a name that resolves to the load balancer the caller just created and can be connected to. The recording shows the real service returning a resolvable elb.amazonaws.com name for the described load balancer.
Emulator: returns a DNSName in the amazonaws.com namespace which is not an address the emulator is reachable on (replay verdict `host_not_own` on $.DescribeLoadBalancersResponse.DescribeLoadBalancersResult.LoadBalancers.member.DNSName); status 200, value structurally shaped like AWS's but announcing a host other than the one the request arrived on.
Why: the DNSName is the one field a client uses to actually send traffic to the load balancer. A test or application that creates an ALB and then issues an HTTP request to the returned DNSName leaves the emulator entirely — the name either fails to resolve or resolves to real AWS — so every data-plane assertion built on a described load balancer is unreachable rather than failed.
Fix: the load-balancer state/address allocation that mints DNSName (shared with Cause 4) must emit a host that resolves to the emulator's own data listener, keeping AWS's name shape (scheme prefix for internal load balancers, name, account-scoped suffix, region) while remaining an address the emulator answers on.
Traces: elasticloadbalancing-set-security-groups-round-trip#task#3 (cluster also covers elasticloadbalancing-set-security-groups-round-trip#task#5, elbv2env-alb-create-provisioning-and-subnet-reconfig#task#6, #task#15, elbv2err-nlb-elastic-ip-rejections#task#7).

Cause 4 of 5: CreateLoadBalancer returns the same unreachable DNSName at creation time.
AWS: Smithy: CreateLoadBalancer output LoadBalancers -> LoadBalancer.DNSName, "The public DNS name of the load balancer". The recording treats the exact name as per-run (shaped, not asserted) but requires it to be a host that is actually the load balancer's.
Emulator: the created load balancer's $.CreateLoadBalancerResponse.CreateLoadBalancerResult.LoadBalancers.member.DNSName is reported as a host the emulator does not own (`host_not_own`), so the value is announced but not served.
Why: same client impact as Cause 3, at the moment of creation — the first thing a provisioning client does with CreateLoadBalancer's result is point a DNS record, a health probe or an HTTP client at DNSName.
Fix: the CreateLoadBalancer handler's DNSName assignment (same allocation as Cause 3); closing Cause 3 alone is not enough because the creation response is produced before any describe.
Traces: elbv2env-alb-create-provisioning-and-subnet-reconfig#task#5 (cluster also covers elbv2err-nlb-elastic-ip-rejections#task#6).

Cause 5 of 5: CreateLoadBalancer accepts an internet-facing load balancer in a VPC that has no internet gateway.
AWS: Smithy: CreateLoadBalancer declares InvalidSubnetException (awsQueryError code `InvalidSubnet`, HTTP 400, Sender type). The recording shows AWS refusing the request with HTTP 400 and ErrorResponse.Error = {Type: Sender, Code: InvalidSubnet, Message: "VPC <vpc-id> has no internet gateway"} — fixed text "VPC " ... " has no internet gateway", with the VPC id taken from the subnets named in the request.
Emulator: answers HTTP 200 and creates the load balancer ($.ErrorResponse missing; a LoadBalancer with a DNSName is returned instead).
Why: an internet-facing load balancer in a VPC with no internet gateway can never receive traffic, so AWS rejects it at create time. A client relying on that rejection (infrastructure code that validates a network layout, or a negative test asserting InvalidSubnet) instead gets a successful create and a load balancer that silently never serves, moving the failure from a precise 400 at provisioning time to an unexplained connectivity outage later.
Fix: CreateLoadBalancer's input validation — before provisioning, check that the subnets of an internet-facing load balancer belong to a VPC with an attached internet gateway and raise InvalidSubnetException (400, code InvalidSubnet) with AWS's message wording when it does not.
Traces: elbv2err-create-load-balancer-input-rejections#task#8.

Related open issues cover other elbv2 divergences in the same fixtures (20261103T215504Z-elasticloadbalancing-elbv2-rejects-malformed-identifiers-and-invalid-inputs-with.md, 20261029T185946Z-elasticloadbalancing-elbv2-accepts-invalid-and-prohibited-requests-with-http.md, 20261104T190207Z-elasticloadbalancing-elbv2-subnet-refusal-wording-and-describerules-paging-diverge.md); none of them carries the replay keys of the five causes above.

## Evidence
All five from replay of trace set simparity-wire-aws-v1 (emulator up, /__admin/health 200):

1. elasticloadbalancing-alb-forwards-health-checks-to-lambda-target#task#18 — DescribeTargetHealth, status 200 vs 200; $...TargetHealthDescriptions.member.AdministrativeOverride: missing (expected {'Reason': 'AdministrativeOverride.NoOverride', 'Description': 'No override is currently active on target', 'State': 'no_override'}, got None); $...TargetHealthDescriptions.member.Target.AvailabilityZone: missing (expected 'all', got None). divergence_key service=elasticloadbalancing|DescribeTargetHealth|status=200->200|keys=...AdministrativeOverride,...Target.AvailabilityZone

2. elbv2-describe-target-health-include-anomaly-detection#task#10 — DescribeTargetHealth, 200 vs 200; $...TargetHealthDescriptions.member.AnomalyDetection: missing (expected {'Result': 'normal', 'MitigationInEffect': 'no'}, got None). divergence_key service=elasticloadbalancing|DescribeTargetHealth|status=200->200|keys=...AnomalyDetection

3. elasticloadbalancing-set-security-groups-round-trip#task#3 — DescribeLoadBalancers, 200 vs 200; $...LoadBalancers.member.DNSName: host_not_own — the emulator announces an elb.amazonaws.com host it is not reached on. 5 affected steps (also #task#5, elbv2env-alb-create-provisioning-and-subnet-reconfig#task#6/#task#15, elbv2err-nlb-elastic-ip-rejections#task#7).

4. elbv2env-alb-create-provisioning-and-subnet-reconfig#task#5 — CreateLoadBalancer, 200 vs 200; $.CreateLoadBalancerResponse.CreateLoadBalancerResult.LoadBalancers.member.DNSName: host_not_own. 2 affected steps (also elbv2err-nlb-elastic-ip-rejections#task#6).

5. elbv2err-create-load-balancer-input-rejections#task#8 — CreateLoadBalancer, expected 400, got 200; $.ErrorResponse missing (expected Error {Type: Sender, Code: InvalidSubnet, Message: 'VPC vpc-00000000000000080 has no internet gateway'}) and a LoadBalancer with a DNSName returned instead. divergence_key service=elasticloadbalancing|CreateLoadBalancer|status=400->200|keys=...DNSName,$.ErrorResponse,status

## Steps to Reproduce
1. Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1.
2. Cause 1: replay fixture elasticloadbalancing-alb-forwards-health-checks-to-lambda-target to task#18 and inspect the DescribeTargetHealth XML — no AdministrativeOverride element and no Target/AvailabilityZone element.
3. Cause 2: replay elbv2-describe-target-health-include-anomaly-detection to task#10 (DescribeTargetHealth with the anomaly-detection Include option) — no AnomalyDetection element.
4. Cause 3: replay elasticloadbalancing-set-security-groups-round-trip to task#3 and try to connect to the DNSName DescribeLoadBalancers returns — it is not an address the emulator serves.
5. Cause 4: replay elbv2env-alb-create-provisioning-and-subnet-reconfig to task#5 and do the same with CreateLoadBalancer's DNSName.
6. Cause 5: replay elbv2err-create-load-balancer-input-rejections to task#8 — CreateLoadBalancer for an internet-facing load balancer in a VPC with no internet gateway returns 200 instead of 400 InvalidSubnet.
