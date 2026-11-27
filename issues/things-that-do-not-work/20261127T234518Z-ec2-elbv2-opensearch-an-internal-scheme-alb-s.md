Issue-Severity: HIGH
Issue-Commit: 5cb989982ca164b028438556d2b2a85d4de8b473

# ec2__elbv2__opensearch: an internal-scheme ALB's DNSName is published as a bare "host:port" address, so the Cognito/OpenSearch diagnosis lifecycle cannot deploy

> _Note: this issue was identified against the code base at commit `5cb989982ca164b028438556d2b2a85d4de8b473`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateLoadBalancer

## Description
Cause 1 of 1: LoadBalancer.DNSName is published as a bare `<host>:<port>` address literal instead of AWS's scheme-dependent load-balancer DNS name, so an internal-scheme ALB's DNS name carries none of AWS's form — in particular not the fixed `internal-` prefix that identifies an internal load balancer. The deploy phase of the OpenSearch/Cognito/ALB diagnosis lifecycle fails on that value and the remaining four phases never run. Failing check: ec2__elbv2__opensearch.e2e-opensearch-cognito-alb-timeout-diagnosis-lifecycle.

AWS: Smithy (elasticloadbalancingv2) LoadBalancer.DNSName — "The public DNS name of the load balancer." AWS derives that name from the load balancer's Name, a generated id, the region and the Scheme given at CreateLoadBalancer: for Scheme='internal' it has the shape `internal-<name>-<generated-id>.<region>.elb.amazonaws.com`, and for 'internet-facing' the same shape without the prefix. The fixed text of that template is the literal `internal-`, the `.elb.amazonaws.com` suffix and the dot separators; the variable parts (name, generated id, region) come from the request and the service's state and are never a value copied from one run. DescribeLoadBalancers reports the same value, so this name is how a client tells an internal load balancer apart from a public one and what it hands to Route 53 aliases, CloudFront origins and application config.

Emulator: DNSName is an address literal with no name, no scheme-derived prefix and no region — e.g. `'127.0.0.1:44305'` for a load balancer created with Scheme='internal'. The host and port themselves are per-run values; the divergence is that the member holds a host:port address rather than the scheme-dependent DNS name, so `DNSName.lower().startswith('internal-')` is false and the name encodes neither the requested Name nor the region.

Why: a client that reads DescribeLoadBalancers to find the internal ALB in front of a VPC OpenSearch domain — the standard `LoadBalancers[?VpcId=='<vpc>' && Scheme=='internal'].DNSName` selection this lifecycle performs — cannot identify an internal load balancer at all, and anything that treats DNSName as a resolvable host name (a CNAME target, a Host header, a certificate SAN, a Cognito callback URL) is built on a value that is not a DNS name.

Fix: the load-balancer address publication in the elbv2 service — the DNSName assigned by CreateLoadBalancer and reported by DescribeLoadBalancers must carry AWS's scheme-dependent DNS-name form (the `internal-` prefix for Scheme='internal', the load balancer's Name, a generated id, the region and the `.elb.amazonaws.com` suffix), with the emulator's data plane reachable at the address that name denotes.

Related but not covering this: open issues 20261122T170634Z-ec2-elbv2-createloadbalancer-describeloadbalancers-publish-dnsname-as-a.md and 20261112T140934Z-elbv2-7-net-new-causes-loadbalancer-dnsname-is.md report the same host:port publication under the `ec2__elbv2` and `elbv2` services, and 20261115T034057Z-ec2-elbv2-reopens-the-alb-still-announces-an.md reports the announced address not being the endpoint the emulator is reached on. None of them carries this check's replay key, so this lifecycle stays blocked and unverified until this one is closed.

Traces: ec2__elbv2__opensearch.e2e-opensearch-cognito-alb-timeout-diagnosis-lifecycle.

## Evidence
Failing check ec2__elbv2__opensearch.e2e-opensearch-cognito-alb-timeout-diagnosis-lifecycle [mismatch]. The five-phase lifecycle dies in phase 1 of 5:

  File "check_ec2__elbv2__opensearch_e2e_opensearch_cognito_alb_timeout_diagnosis_lifecycle.py", line 447, in phase
  raise AssertionError(f"[phase {i}/{len(self.phases)} {name}] {exc}") from exc
  AssertionError: [phase 1/5 deploy] an internal ALB's DNS name carries the internal- prefix, got '127.0.0.1:44305'

Expected: for the stack's Scheme='internal' application load balancer, DNSName is `internal-<name>-<generated-id>.<region>.elb.amazonaws.com` — the literal `internal-` prefix, the load balancer's own Name, a generated id, the region and `.elb.amazonaws.com`. Actual: `'127.0.0.1:44305'`, a host:port address with no prefix, no name and no region (the particular host and port are per-run values). Because the deploy phase asserts the ALB it must later diagnose, the setup, solve, cleanup and destroy phases — including the DescribeDomain/DescribeLoadBalancers diagnosis trajectory — are never exercised.

## Steps to Reproduce
1. git_clone, then run_behavioral_suite so the emulator is serving.
2. Re-replay ec2__elbv2__opensearch.e2e-opensearch-cognito-alb-timeout-diagnosis-lifecycle.
3. Manually: deploy a VPC with two private subnets in different AZs, then CreateLoadBalancer with Type='application', Scheme='internal' on those subnets.
4. Read DNSName from the CreateLoadBalancer response and from DescribeLoadBalancers (including via the lifecycle's own selection `LoadBalancers[?VpcId=='<vpc-id>' && Scheme=='internal'].DNSName`).
5. DNSName must begin with the literal 'internal-' and end in '.<region>.elb.amazonaws.com'; the emulator returns a '<host>:<port>' address instead.
