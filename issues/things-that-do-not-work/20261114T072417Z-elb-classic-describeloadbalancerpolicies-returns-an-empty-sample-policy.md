Issue-Source: trace-sweep-divergence
Issue-Trace: elb-catalog-describe-load-balancer-policies
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: elb-catalog-describe-load-balancer-policies
Issue-Shape-Id: com.amazonaws.elasticloadbalancing#ElasticLoadBalancing_v7
Issue-Key: elb|DescribeLoadBalancerPolicies|2xx->2xx|$.DescribeLoadBalancerPoliciesResponse.DescribeLoadBalancerPoliciesResult.PolicyDescriptions.member[*]
Issue-Commit: 84939cc283904302bdfe02403fef158918b6c804

# ELB Classic DescribeLoadBalancerPolicies returns an empty sample-policy catalog

> _Note: this issue was identified against the code base at commit `84939cc283904302bdfe02403fef158918b6c804`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeLoadBalancerPolicies

## Description
1 cause.

Cause 1 of 1: DescribeLoadBalancerPolicies called WITHOUT a LoadBalancerName must return the service's built-in sample/predefined policy catalog; the emulator answers 200 with an empty PolicyDescriptions list.

AWS: Smithy: operation DescribeLoadBalancerPolicies, input DescribeLoadBalancerPoliciesInput {LoadBalancerName: AccessPointName (optional), PolicyNames: PolicyNames}, output DescribeLoadBalancerPoliciesOutput {PolicyDescriptions: PolicyDescriptions}; errors AccessPointNotFoundException (awsQueryError LoadBalancerNotFound, 400) and PolicyNotFoundException (awsQueryError PolicyNotFound, 400). The operation's own documentation states the rule: "If you don't specify a load balancer name, the action returns descriptions of the specified sample policies, or descriptions of all sample policies." So a DescribeLoadBalancerPolicies with no LoadBalancerName and no PolicyNames is a catalog read over the service-owned predefined policies, and it must return one PolicyDescription per predefined policy, each populated per Smithy shape PolicyDescription {PolicyName, PolicyTypeName, PolicyAttributeDescriptions: list<PolicyAttributeDescription {AttributeName, AttributeValue}>}. The recording shows that catalog as a non-empty PolicyDescriptions list (11 members, member[0]..member[10]) whose PolicyAttributeDescriptions are the SSL-negotiation attribute set — protocol toggles (AttributeName "Protocol-TLSv1"/"Protocol-SSLv3"/"Protocol-TLSv1.1"/"Protocol-TLSv1.2"), the cipher-order toggle ("Server-Defined-Cipher-Order") and one entry per cipher suite ("ECDHE-ECDSA-AES128-GCM-SHA256", "AES256-SHA", "DHE-RSA-AES128-SHA", ...) — each with AttributeValue "true" or "false". The requirement is that the catalog is served and that each predefined policy carries its name, its policy type and its full enabled/disabled attribute set; the specific policy names, the exact cipher list and the exact member count are the service's own catalog contents as of the recording, not a per-run value.

Emulator: the request is accepted and answered 200 (expected status 200, actual status 200) with a well-formed DescribeLoadBalancerPoliciesResponse envelope, but PolicyDescriptions is empty: replay reports every expected member missing — $.DescribeLoadBalancerPoliciesResponse.DescribeLoadBalancerPoliciesResult.PolicyDescriptions.member[0] through member[10] all "missing", e.g. member[0] expected {PolicyAttributeDescriptions: {member: [{AttributeName: 'Protocol-TLSv1', AttributeValue: 'true'}, {AttributeName: 'Protocol-SSLv3', AttributeValue: 'false'}, ...]}} and nothing returned. No modeled error is raised either: neither AccessPointNotFoundException/LoadBalancerNotFound nor PolicyNotFoundException/PolicyNotFound, so the emulator is not refusing the catalog read, it simply has no catalog to read from.

Why: a client relying on AWS's behavior uses this call to discover which predefined SSL negotiation policies exist and which protocols and cipher suites each enables, then feeds a chosen PolicyName into CreateLoadBalancerPolicy / SetLoadBalancerPoliciesOfListener for an HTTPS listener. Because the emulator answers success with an empty list instead of failing, the client cannot distinguish "this account/region has no predefined policies" from "the catalog is unavailable": it concludes no predefined policy exists and either aborts policy selection or silently configures a listener with no negotiation policy. Any tooling that validates a user-supplied policy name against the catalog, or that renders the protocol/cipher matrix, gets an empty result and reports the service as having nothing to offer. The failure is silent, which is worse than a modeled error, because every caller treats 200 as authoritative.

Fix: the DescribeLoadBalancerPolicies handler and the state it reads. The service needs a seeded, service-owned catalog of predefined policy descriptions (policy name, policy type name, and the full PolicyAttributeDescriptions attribute set with its true/false values) that is independent of any user-created load balancer, and the handler's no-LoadBalancerName branch must serve that catalog — filtered by PolicyNames when the request supplies them, and raising PolicyNotFoundException/PolicyNotFound for a requested name the catalog does not contain. The serializer already emits the correct envelope, so the gap is the handler branch plus the seed data, not the XML shape.

Traces: elb-catalog-describe-load-balancer-policies#task#1 (DescribeLoadBalancerPolicies, verdict mismatch, 200 -> 200, 11 missing PolicyDescriptions members).

Related open issues, neither of which covers this cause: 20261103T144333Z-elastic-load-balancing-setloadbalancerpoliciesoflistener-stub.md (SetLoadBalancerPoliciesOfListener is an unimplemented scaffold stub — a different operation, the write side) and 20261109T233833Z-elasticloadbalancing-elb-classic-publishes-an-unreachable-elb-amazonaws-com.md (DNSName host fidelity). No open issue mentions DescribeLoadBalancerPolicies.

## Evidence
Fixture elb-catalog-describe-load-balancer-policies, step task#1, operation DescribeLoadBalancerPolicies, service elb, verdict mismatch.

Expected (recorded AWS): HTTP 200, DescribeLoadBalancerPoliciesResponse.DescribeLoadBalancerPoliciesResult.PolicyDescriptions.member[0..10] present — 11 predefined policy descriptions, each with a PolicyAttributeDescriptions.member list of AttributeName/AttributeValue pairs covering the protocol toggles and cipher suites, e.g. member[0] = {PolicyAttributeDescriptions: {member: [{AttributeName: 'Protocol-TLSv1', AttributeValue: 'true'}, {AttributeName: 'Protocol-SSLv3', AttributeValue: 'false'}, {AttributeName: 'Protocol-TLSv1.1', AttributeValue: 'true'}, {AttributeName: 'Protocol-TLSv1.2', AttributeValue: 'true'}, {AttributeName: 'Server-Defined-Cipher-Order', AttributeValue: 'true'}, {AttributeName: 'ECDHE-ECDSA-AES128-GCM-SHA256', AttributeValue: 'true'}, ... {AttributeName: 'DHE-RSA-AES128-SHA', AttributeValue: 'false'}, ...]}}.

Actual (emulator): HTTP 200 with the same response envelope but PolicyDescriptions empty. Replay detail: "$.DescribeLoadBalancerPoliciesResponse.DescribeLoadBalancerPoliciesResult.PolicyDescriptions.member[0]: missing (expected {...})", and the same 'missing' verdict for member[1] through member[10] (divergence_key keys list enumerates member[0]..member[10]).

No error was returned: no AccessPointNotFoundException/LoadBalancerNotFound and no PolicyNotFoundException/PolicyNotFound, so the empty list is served as a successful catalog read.

Smithy evidence cited: DescribeLoadBalancerPolicies documentation — "If you don't specify a load balancer name, the action returns descriptions of the specified sample policies, or descriptions of all sample policies."; output member PolicyDescriptions -> list<PolicyDescription{PolicyName, PolicyTypeName, PolicyAttributeDescriptions}>; PolicyAttributeDescription{AttributeName, AttributeValue}; modeled errors AccessPointNotFoundException (LoadBalancerNotFound, 400) and PolicyNotFoundException (PolicyNotFound, 400).

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus (replay_wire_corpus), or drive the step directly.
2. Send an ELB Classic (elasticloadbalancing query protocol) DescribeLoadBalancerPolicies request with NO LoadBalancerName parameter and NO PolicyNames parameter — the sample/predefined policy catalog read. This is fixture elb-catalog-describe-load-balancer-policies, step task#1.
3. Inspect the response body at DescribeLoadBalancerPoliciesResponse.DescribeLoadBalancerPoliciesResult.PolicyDescriptions.
4. Observed: HTTP 200 with PolicyDescriptions containing no member elements.
5. Expected: HTTP 200 with one PolicyDescriptions.member per predefined policy (11 in the recording), each carrying PolicyName, PolicyTypeName and a populated PolicyAttributeDescriptions.member list of AttributeName/AttributeValue pairs for the protocol toggles, Server-Defined-Cipher-Order and each cipher suite.
6. Also check the filtered form: repeat with PolicyNames.member.1 set to a predefined policy name (expect just that description) and with a name that is not in the catalog (expect HTTP 400 PolicyNotFound / PolicyNotFoundException).
