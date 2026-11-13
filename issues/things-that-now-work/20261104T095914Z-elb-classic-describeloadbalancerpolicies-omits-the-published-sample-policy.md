Issue-Source: trace-sweep-divergence
Issue-Trace: elb-catalog-describe-load-balancer-policies
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: elb-catalog-describe-load-balancer-policies
Issue-Shape-Id: com.amazonaws.elasticloadbalancing#ElasticLoadBalancing_v7
Issue-Key: elb|DescribeLoadBalancerPolicies|2xx->2xx|$.DescribeLoadBalancerPoliciesResponse.DescribeLoadBalancerPoliciesResult.PolicyDescriptions.member[*]
Issue-Commit: a57dea262f41459244daf18c6d5b4cdf00e6a648

# ELB Classic DescribeLoadBalancerPolicies omits the published sample-policy catalog

> _Note: this issue was identified against the code base at commit `a57dea262f41459244daf18c6d5b4cdf00e6a648`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeLoadBalancerPolicies

## Description
Cause 1 of 1: a DescribeLoadBalancerPolicies request that names no LoadBalancerName answers a PolicyDescriptions list that contains none of the sample policies AWS reports.

AWS: DescribeLoadBalancerPolicies is documented (Smithy: com.amazonaws.elasticloadbalancing#DescribeLoadBalancerPolicies) as "If you don't specify a load balancer name, the action returns descriptions of the specified sample policies, or descriptions of all sample policies. The names of the sample policies have the ELBSample- prefix." The recorded response for the no-argument form carries ELEVEN PolicyDescription members (recorded behavior: $.DescribeLoadBalancerPoliciesResponse.DescribeLoadBalancerPoliciesResult.PolicyDescriptions.member[0] .. member[10]), each a complete SSL-negotiation policy description: PolicyName, PolicyTypeName and a PolicyAttributeDescriptions list that carries the SSL PROTOCOL attributes (Protocol-SSLv2, Protocol-SSLv3, Protocol-TLSv1, Protocol-TLSv1.1, Protocol-TLSv1.2), the Server-Defined-Cipher-Order attribute, and ONE attribute per cipher suite the policy's published configuration covers (ECDHE-ECDSA-AES128-GCM-SHA256, ECDHE-RSA-AES128-GCM-SHA256, ECDHE-ECDSA-AES128-SHA256, ..., AES128-SHA, AES256-GCM-SHA384, AES256-SHA, DHE-RSA-AES128-SHA, DHE-DSS-AES128-SHA, CAMELLIA128-SHA, ...), each with AttributeValue "true" or "false" according to whether that policy enables it. The individual policy names, the exact cipher roster and each cipher's true/false value belong to the catalog AWS publishes; the requirement is that the catalog is served whole, not that one recording's values are copied.

Emulator: the handler serves a catalog of only TWO predefined sample policies (ELBSample-ELBDefaultCipherPolicy and ELBSample-OpenSSLDefaultCipherPolicy), and each of those carries five Protocol-* attributes only — the Server-Defined-Cipher-Order attribute and every per-cipher attribute are deliberately absent (state.rs documents them as "deliberately NOT carried here"). Replay therefore reports every one of the eleven recorded PolicyDescriptions members as missing from the actual 200 response, with no cluster member matching even on attribute content.

Why: a client that enumerates the sample/predefined SSL negotiation policies (to pick a cipher policy, to diff a load balancer's SSLNegotiationPolicyType attributes against a sample, or to render the catalog in a console/IaC plan) sees an almost-empty catalog and cannot find the policy or cipher attribute it is looking for. Because the call still answers HTTP 200, the client treats the truncated catalog as authoritative rather than retrying or failing.

Fix: the DescribeLoadBalancerPolicies sample-policy data in this service's state — the SAMPLE_POLICIES catalog the no-load-balancer branch of the handler projects — must carry the full published set of ELBSample-* policies and, for each, the complete PolicyAttributeDescriptions list (protocol attributes, Server-Defined-Cipher-Order and one entry per cipher suite). The handler's filtering/projection path (describe_sample) is already correct; it is the catalog it reads from that is short.

Traces: elb-catalog-describe-load-balancer-policies#task#1 (operation DescribeLoadBalancerPolicies, expected 200, actual 200, eleven PolicyDescriptions members missing).

## Evidence
Fixture elb-catalog-describe-load-balancer-policies, step task#1, operation DescribeLoadBalancerPolicies.
Expected status 200, actual status 200 — body diverges.
Replay detail: "$.DescribeLoadBalancerPoliciesResponse.DescribeLoadBalancerPoliciesResult.PolicyDescriptions.member[0]: missing (expected {'PolicyAttributeDescriptions': {'member': [{'AttributeValue': 'true', 'AttributeName': 'Protocol-TLSv1'}, {'AttributeValue': 'false', 'AttributeName': 'Protocol-SSLv3'}, {'AttributeValue': 'true', 'AttributeName': 'Protocol-TLSv1.1'}, {'AttributeValue': 'true', 'AttributeName': 'Protocol-TLSv1.2'}, {'AttributeValue': 'true', 'AttributeName': 'Server-Defined-Cipher-Order'}, {'AttributeValue': 'true', 'AttributeName': 'ECDHE-ECDSA-AES128-GCM-SHA256'}, ... ]}})" and the same 'missing' verdict for member[1] through member[10] — i.e. ALL eleven recorded sample-policy descriptions are absent from the emulator's response.
Smithy: DescribeLoadBalancerPolicies doc — "If you don't specify a load balancer name, the action returns descriptions of the specified sample policies, or descriptions of all sample policies. The names of the sample policies have the ELBSample- prefix."; output member PolicyDescriptions -> list of PolicyDescription {PolicyName, PolicyTypeName, PolicyAttributeDescriptions}.
Emulator source: services/service-com-amazonaws-elasticloadbalancing-elasticloadbalancing-v7/src/state.rs lines 514-553 define SAMPLE_POLICIES with exactly two entries carrying five Protocol-* attributes each; services/service-com-amazonaws-elasticloadbalancing-elasticloadbalancing-v7/src/ops/describe_load_balancer_policies.rs lines 168-197 serve that catalog for the no-LoadBalancerName branch.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus (replay_wire_corpus).
2. Fixture elb-catalog-describe-load-balancer-policies, step task#1: issue an ELB Classic (API version 2012-06-01) query POST with Action=DescribeLoadBalancerPolicies and NO LoadBalancerName and no PolicyNames.
3. Inspect DescribeLoadBalancerPoliciesResponse/DescribeLoadBalancerPoliciesResult/PolicyDescriptions: AWS returns eleven ELBSample-* PolicyDescription members, each with protocol, Server-Defined-Cipher-Order and per-cipher PolicyAttributeDescriptions; the emulator returns the two-entry protocol-only catalog, so none of the recorded members is present.
