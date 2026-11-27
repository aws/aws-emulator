Issue-Source: trace-sweep-divergence
Issue-Trace: ec2-traffic-mirror-filter-lifecycle
Issue-Step: task#5
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ec2-traffic-mirror-filter-lifecycle
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|ModifyTrafficMirrorFilterNetworkServices|2xx->2xx|$.ModifyTrafficMirrorFilterNetworkServicesResponse.trafficMirrorFilter.ingressFilterRuleSet.item.ruleAction
Issue-Commit: e62e0ae23225a1c6c2c24fb08c0dcaf4852934da

# ec2 ModifyTrafficMirrorFilterNetworkServices reports the ingress rule's ruleAction as accept for a rule stored as reject

> _Note: this issue was identified against the code base at commit `e62e0ae23225a1c6c2c24fb08c0dcaf4852934da`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ModifyTrafficMirrorFilterNetworkServices

## Description
Cause 1 of 1: the TrafficMirrorFilter this operation returns projects each ingress filter rule with ruleAction 'accept' regardless of the action the rule was created with, so a rule created with RuleAction=reject reads back as accept.
AWS: Smithy: ModifyTrafficMirrorFilterNetworkServices -> output member TrafficMirrorFilter (xmlName trafficMirrorFilter) -> TrafficMirrorFilter.ingressFilterRuleSet -> TrafficMirrorFilterRule.RuleAction (xmlName ruleAction), targeting enum TrafficMirrorRuleAction with values accept|reject and documented as 'The action assigned to the Traffic Mirror rule.' AWS echoes the filter as stored, so each rule in ingressFilterRuleSet/egressFilterRuleSet carries the RuleAction that rule was created or modified with; the recording shows ruleAction 'reject' for the ingress rule after the network-service modification.
Emulator: the same call answers 200 but the ingress rule in the echoed filter carries ruleAction 'accept'.
Why: traffic-mirror filters are allow/deny policy. A client that reads back the filter to confirm its policy — or that renders it to an operator — is told the rule mirrors (accepts) traffic the rule was written to reject, so a mirroring policy appears to be the opposite of what it is, and any read-modify-write round trip through this projection can persist the wrong action.
Fix: the state mutator this handler calls and the filter projection it returns — services/service-com-amazonaws-ec2-amazonec2/src/ops/modify_traffic_mirror_filter_network_services.rs lines 205-221 returns the record produced by state's modify_traffic_mirror_filter_network_services; that projection must carry each stored rule's own RuleAction instead of a default action. The open issue 20261117T164306Z-ec2-create-and-describe-output-members-diverge-from.md describes create/describe output members for traffic mirror filters but does not carry this replay key.
Traces: ec2-traffic-mirror-filter-lifecycle#task#5.

## Evidence
Replay of trace set simparity-wire-aws-v1, fixture ec2-traffic-mirror-filter-lifecycle, step task#5, operation ModifyTrafficMirrorFilterNetworkServices, expected status 200 / actual status 200, verdict mismatch:
  $.ModifyTrafficMirrorFilterNetworkServicesResponse.trafficMirrorFilter.ingressFilterRuleSet.item.ruleAction: value_mismatch (expected 'reject', got 'accept')
All other members of the echoed filter matched, so only the rule action is wrong. Cluster affected_count 1 (ec2-traffic-mirror-filter-lifecycle#task#5); no stub and no owning open issue.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay the wire corpus (trace set simparity-wire-aws-v1) and re-run fixture ec2-traffic-mirror-filter-lifecycle, step task#5.
3. Equivalently: CreateTrafficMirrorFilter; CreateTrafficMirrorFilterRule with TrafficDirection=ingress and RuleAction=reject; then ModifyTrafficMirrorFilterNetworkServices on that filter (e.g. AddNetworkServices=amazon-dns) and inspect trafficMirrorFilter.ingressFilterRuleSet.item.ruleAction in the 200 response — it is 'accept' where the stored rule's action is 'reject'.
