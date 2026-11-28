Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2__elbv2__opensearch.e2e-opensearch-cognito-alb-timeout-diagnosis-lifecycle
Issue-Key: ec2__elbv2__opensearch.e2e-opensearch-cognito-alb-timeout-diagnosis-lifecycle|phase#8|AssertionError|raise AssertionError(f"[phase {i}/{len(self.phases)} {name}] {exc}") from exc
Issue-Commit: 7da368b21ee7d04c70045aa1c38fe932bbd09ef4

# ec2__elbv2__opensearch: an internal-scheme ALB's DNSName is published as a bare "host:port" address, so the OpenSearch/Cognito ALB diagnosis lifecycle dies in its deploy phase

> _Note: this issue was identified against the code base at commit `7da368b21ee7d04c70045aa1c38fe932bbd09ef4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateLoadBalancer

## Description
Cause 1 of 1: LoadBalancer.DNSName is published as a bare `<host>:<port>` address literal instead of AWS's scheme-dependent load-balancer DNS name, so an internal-scheme application load balancer's DNS name carries none of AWS's form — in particular not the fixed `internal-` prefix that identifies an internal load balancer. The deploy phase (1 of 5) of the Cognito-auth VPC OpenSearch + internal ALB lifecycle asserts that prefix on the stack's ALB and fails on the address literal, so the setup, solve, cleanup and destroy phases — including the DescribeDomain/DescribeLoadBalancers diagnosis trajectory the check exists to exercise — are never reached. Failing check: ec2__elbv2__opensearch.e2e-opensearch-cognito-alb-timeout-diagnosis-lifecycle.

AWS: Smithy (elasticloadbalancingv2) LoadBalancer.DNSName — "The public DNS name of the load balancer." AWS derives that name from the load balancer's Name, a generated id, the region and the Scheme supplied at CreateLoadBalancer: for Scheme='internal' the name has the shape `internal-<name>-<generated-id>.<region>.elb.amazonaws.com`, and for Scheme='internet-facing' the same shape without that prefix. The FIXED text of that template is the literal `internal-`, the `.elb.amazonaws.com` suffix and the dot separators; the VARIABLE parts — the load balancer's own Name, the generated id and the region — come from the request and the service's state and are never a value copied from one recorded run. DescribeLoadBalancers reports the same value, so this name is how a client tells an internal load balancer apart from a public one, and it is what is handed to Route 53 alias targets, CloudFront origins, Cognito callback URLs and application configuration.

Emulator: DNSName is returned as an address literal with no name component, no scheme-derived prefix and no region — e.g. `'127.0.0.1:43601'` for a load balancer created with Scheme='internal'. The particular host and port are per-run values and are not themselves the requirement; the divergence is that the member holds a host:port authority rather than the scheme-dependent DNS name, so `DNSName.lower().startswith('internal-')` is false and the value encodes neither the requested Name nor the region.

Why: a client that locates the internal ALB in front of a VPC OpenSearch domain by the standard selection `LoadBalancers[?VpcId=='<vpc-id>' && Scheme=='internal'].DNSName` — exactly what this lifecycle's read-only diagnosis performs — cannot identify an internal load balancer at all, and anything that treats DNSName as a resolvable host name (a CNAME target, a Host header, a certificate SAN, a Cognito callback URL) is built on a value that is not a DNS name. Nothing downstream of the deploy phase can run.

Fix: the load-balancer address publication in the elbv2 service — the DNSName assigned by CreateLoadBalancer and reported by DescribeLoadBalancers must carry AWS's scheme-dependent DNS-name form (the `internal-` prefix for Scheme='internal', the load balancer's Name, a generated id, the region and the `.elb.amazonaws.com` suffix), with the emulator's data plane reachable at the address that name denotes.

Related but not covering this: open issue 20261127T234518Z-ec2-elbv2-opensearch-an-internal-scheme-alb-s.md describes the same host:port publication for this same lifecycle but does not carry this check's replay key (its header has no Issue-Trace-Ids/Issue-Key), so the replay suite does not treat this check as owned and the lifecycle stays unverified; open issues 20261122T170634Z-ec2-elbv2-createloadbalancer-describeloadbalancers-publish-dnsname-as-a.md and 20261123T141857Z-elbv2-4-net-new-causes-an-alb-answers.md report the host:port publication under the `ec2__elbv2` and `elbv2` services, and 20261115T034057Z-ec2-elbv2-reopens-the-alb-still-announces-an.md reports the announced address not being the endpoint the emulator is reached on. None of them carries this check.

Traces: ec2__elbv2__opensearch.e2e-opensearch-cognito-alb-timeout-diagnosis-lifecycle (phase 1/5 'deploy').

## Evidence
Failing check ec2__elbv2__opensearch.e2e-opensearch-cognito-alb-timeout-diagnosis-lifecycle [mismatch]. The five-phase lifecycle (deploy, setup, solve, cleanup, destroy) dies in phase 1 of 5:

  File "contextlib.py", line 158, in __exit__
  self.gen.throw(value)
  File "check_ec2__elbv2__opensearch_e2e_opensearch_cognito_alb_timeout_diagnosis_lifecycle.py", line 447, in phase
  raise AssertionError(f"[phase {i}/{len(self.phases)} {name}] {exc}") from exc
  AssertionError: [phase 1/5 deploy] an internal ALB's DNS name carries the internal- prefix, got '127.0.0.1:43601'

Expected: for the stack's Scheme='internal' application load balancer, DNSName is `internal-<name>-<generated-id>.<region>.elb.amazonaws.com` — the literal `internal-` prefix (fixed text), then the load balancer's own Name, a generated id and the region (values from the request and the emulator's state), then the fixed `.elb.amazonaws.com` suffix.

Actual: `'127.0.0.1:43601'` — a host:port authority with no prefix, no name and no region. The specific host and port shown are per-run values; the defect is the shape of the published value.

Because the deploy phase asserts the very ALB it must later diagnose, the setup, solve (opensearch DescribeDomain + elbv2 DescribeLoadBalancers trajectory), cleanup and destroy phases are never exercised, and the service pass rate for ec2__elbv2__opensearch is 1/2 (50%).

## Steps to Reproduce
1. git_clone, then run_behavioral_suite so the emulator is built and serving.
2. Re-replay ec2__elbv2__opensearch.e2e-opensearch-cognito-alb-timeout-diagnosis-lifecycle.
3. Manually: deploy a VPC with two private subnets in different AZs, then CreateLoadBalancer with Type='application', Scheme='internal' on those subnets.
4. Read DNSName from the CreateLoadBalancer response and from DescribeLoadBalancers, including via the lifecycle's own selection `LoadBalancers[?VpcId=='<vpc-id>' && Scheme=='internal'].DNSName`.
5. DNSName must begin with the literal 'internal-' and end in '.<region>.elb.amazonaws.com'; the emulator returns a '<host>:<port>' address instead, so the deploy phase's `internal-` prefix assertion fails and the remaining four phases never run.
