Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: route53profiles.dns-firewall-resolution-native-protocol-vpc-only
Issue-Shape-Id: com.amazonaws.route53profiles#Route53Profiles
Issue-Key: route53profiles.dns-firewall-resolution-native-protocol-vpc-only|run#9|AssertionError|assert before[0]["answers"], (
Issue-Commit: 07a81b42592fdd972bbc8a5b9a2141006a4e2532

# route53profiles: the VPC Route 53 Resolver answers SERVFAIL for a public name from inside the associated VPC, so a Profile's DNS Firewall BLOCK rule can never be observed taking effect

> _Note: this issue was identified against the code base at commit `07a81b42592fdd972bbc8a5b9a2141006a4e2532`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AssociateProfile

## Description
One net-new root cause, reached by 1 check.

Cause 1 of 1: the VPC-provided Route 53 Resolver (the address every ENI in a VPC resolves through, 169.254.169.253 / the VPC's .2) returns SERVFAIL with an empty answer section for an ordinary public name queried from inside the VPC, so the only data-plane observation a Route 53 Profile's DNS Firewall association has — the DNS answer itself — never exists.

AWS: recorded behavior only for the data plane; no Smithy model describes DNS answers (route53profiles models only the control plane: AssociateProfile, AssociateResourceToProfile, GetProfileAssociation, GetProfileResourceAssociation). What the recording shows is that a query for a publicly resolvable name (A record) issued from inside a freshly created VPC, through the VPC resolver, before any DNS Firewall rule exists, comes back with rcode NOERROR and at least one answer record; and that after a FIREWALL_RULE_GROUP holding a BLOCK rule with BlockResponse=NXDOMAIN is associated to a Profile (AssociateResourceToProfile, Status COMPLETE) and that Profile is associated to the VPC (AssociateProfile, Status COMPLETE), the blocked name then answers rcode NXDOMAIN with an empty answer section while a twin control name still answers NOERROR with records. The rcode and the presence/absence of answer records are the requirement; the particular domain names and the addresses in the answer are per-run request values, not part of it.

Emulator: the in-VPC querier is invoked successfully and a DNS response really comes back (the transaction id matches, so this is the emulator's resolver replying, not a timeout), but the reply is rcode SERVFAIL with answers == [] for the baseline public-name query taken before any firewall object is created. Graded output: AssertionError: 'example.com.' did not resolve from inside the fresh VPC before any firewall rule existed, so blocking it below would prove nothing: {'query': 'example.com.', 'qtype': 'A', 'id_match': True, 'rcode': 'SERVFAIL', 'answers': []}. Because the pre-condition fails, the check never reaches the Profile/firewall assertions, so the emulator's NXDOMAIN-after-BLOCK behavior and its control-domain isolation are both unverified.

Why: a client that wires a DNS Firewall rule group through a Route 53 Profile onto a VPC gets a control plane that reports COMPLETE at every hop while the VPC's resolver answers nothing usable. Name resolution from any VPC-attached workload (a Lambda with a VpcConfig ENI, an instance) fails with SERVFAIL instead of resolving, so the entire effect of a Profile — the reason the Profile exists — is invisible and unenforceable: neither that a blocked name becomes NXDOMAIN nor that an unrelated name keeps resolving can be relied on.

Fix: the VPC DNS resolver data plane the emulator serves to in-VPC workloads (the component answering queries addressed to the VPC resolver address), plus the path that applies a Profile's associated FIREWALL_RULE_GROUP resource associations to the VPCs the Profile is associated with when composing that answer. It must resolve a public name to a NOERROR answer instead of SERVFAIL, and must apply the Profile-attached BLOCK rule as NXDOMAIN with no records for the listed domain only.

Related open issue, not a duplicate: 20261119T232925Z-ec2-lambda-logs-route53resolver-the-vpc-route-53-resolver-answers-nothing-from.md describes the same SERVFAIL from the VPC resolver, but it is bound to the ec2__lambda__logs__route53resolver checks and carries none of this check's replay keys, and it does not cover the route53profiles path (Profile -> resource association -> VPC association) that this check exercises. Re-replay route53profiles.dns-firewall-resolution-native-protocol-vpc-only to confirm this one.

Traces: route53profiles.dns-firewall-resolution-native-protocol-vpc-only (mismatch).

## Evidence
Expected (recorded AWS behavior), queried through the VPC resolver by a Lambda with an ENI in the VPC under test:
  - baseline, before any firewall object exists: blocked-candidate name / A -> rcode NOERROR, answers non-empty; control name / A -> rcode NOERROR, answers non-empty.
  - after AssociateResourceToProfile(<firewall rule group arn>) reaches Status COMPLETE and AssociateProfile(ResourceId=<vpc>) reaches Status COMPLETE, with a BLOCK rule whose BlockResponse=NXDOMAIN over a domain list holding the blocked name: blocked name / A -> rcode NXDOMAIN with answers == []; control name / A -> still NOERROR with answers non-empty.

Actual (emulator), route53profiles.dns-firewall-resolution-native-protocol-vpc-only [mismatch]:
  AssertionError: 'example.com.' did not resolve from inside the fresh VPC before any firewall rule existed, so blocking it below would prove nothing: {'query': 'example.com.', 'qtype': 'A', 'id_match': True, 'rcode': 'SERVFAIL', 'answers': []}

The response is served (id_match True), so this is the emulator's resolver answering SERVFAIL where AWS answers NOERROR with an address. The domain names shown ('example.com.', 'example.net.') are the check's request values; the invariant is that the VPC resolver produces no usable answer for a public name, and consequently never produces the NXDOMAIN-vs-NOERROR split a Profile-attached BLOCK rule must produce.

## Steps to Reproduce
1. ec2 CreateVpc (CidrBlock 10.83.0.0/16) and CreateSubnet (10.83.1.0/24) in it; take the VPC's default security group.
2. iam CreateRole for lambda.amazonaws.com and attach AWSLambdaVPCAccessExecutionRole.
3. lambda CreateFunction (python3.12) with VpcConfig on that subnet + security group, whose handler sends a UDP DNS query for a public name (A) to the VPC resolver address and returns {rcode, answers}; wait for State=Active.
4. Invoke it for a public name (A) and a second public control name (A), before creating any firewall object. Expected: both rcode NOERROR with non-empty answers. Observed: rcode SERVFAIL with answers == [].
5. (Unreached, because step 4 fails.) route53resolver CreateFirewallDomainList + CreateFirewallRuleGroup + CreateFirewallRule(Action=BLOCK, BlockResponse=NXDOMAIN, Priority=1); route53profiles CreateProfile, AssociateResourceToProfile(ResourceArn=<rule group arn>, ResourceProperties='{"priority": 110}') polled to COMPLETE, AssociateProfile(ProfileId, ResourceId=<vpc>) polled to COMPLETE; re-invoke the querier and poll. Expected: the blocked name answers NXDOMAIN with no records while the control name keeps answering NOERROR with records.
