Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: route53profiles.profile-answers-private-zone-names-in-the-associated-vpc-only-for-zones-added-to-it
Issue-Shape-Id: com.amazonaws.route53profiles#Route53Profiles
Issue-Key: route53profiles.profile-answers-private-zone-names-in-the-associated-vpc-only-for-zones-added-to-it|_poll#29|Timeout|value = read()
Issue-Commit: 760ea4c235b0ff162849feaaf99865499e566e9b

# route53profiles: a private hosted zone shared through a Profile is never answered by the associated VPC's Resolver, so the check never completes (60s timeout)

> _Note: this issue was identified against the code base at commit `760ea4c235b0ff162849feaaf99865499e566e9b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AssociateProfile

## Description
One net-new root cause, reached by 1 check.

Cause 1 of 1: the Route 53 Profiles control plane accepts a private-hosted-zone resource association and a VPC association and reports both as settled, but nothing propagates that association into the DNS data plane the VPC's Resolver serves. The check that grades this end to end never completes: it is killed by the suite's 60s budget while waiting on the Resolver, instead of either passing or failing an assertion.

AWS: recorded behavior only for the data plane — the Smithy model of route53profiles describes the control plane only (AssociateProfile POST /profileassociation, AssociateResourceToProfile POST /profileresourceassociation, GetProfileAssociation, GetProfileResourceAssociation, ListProfileResourceAssociations) and models no DNS answer. What the recording requires is the Profile's whole point: a private hosted zone attached ONLY to a different (home) VPC is reachable from a second VPC once (a) the zone's ARN is added to a Profile by AssociateResourceToProfile and that resource association reaches Status COMPLETE, and (b) that Profile is associated with the second VPC by AssociateProfile and that association reaches Status COMPLETE. Then, queried from inside that second VPC through its VPC Resolver (the VPC CIDR's .2 address) over TCP port 53: a name in the shared zone answers rcode NOERROR with exactly the A records published in the zone and a TTL in (0, record TTL]; a name in a second private zone homed on the same other VPC but NOT added to the Profile stays rcode NXDOMAIN with an empty answer section; and before the Profile is associated, a name in the shared zone is also NXDOMAIN with an empty answer section. The rcodes, the record set returned and the TTL ceiling are the requirement; the zone apexes, host labels, addresses and VPC/zone/profile ids in any one recording are per-run request values and are not part of it.

Emulator: the check is killed at the suite's 60s ceiling with "Check timed out after 60.0s" — the emulator never reaches the state the check waits for, so no graded assertion is ever produced. Every wait in the check is a poll on emulator-reported state: get_change to INSYNC, Lambda function_active_v2, GetProfileResourceAssociation Status in {COMPLETE,FAILED} (300s budget), GetProfileAssociation Status in {COMPLETE,FAILED} (600s budget), and finally a 300s poll for the VPC Resolver to answer the shared zone's name with NOERROR and a non-empty answer section. The emulator's own code rules the control-plane waits out as the stall: route53 change propagation is 500ms (services/service-com-amazonaws-route53-awsdnsv20130401/src/state.rs:115, CHANGE_PROPAGATION) and the Profile resource association settles after 3s (services/service-com-amazonaws-route53profiles-route53profiles/src/state.rs:112, ASSOCIATION_SETTLE_DELAY), while AssociateProfile stores the VPC association already in its settled phase (services/service-com-amazonaws-route53profiles-route53profiles/src/ops/associate_profile.rs:254). Hypothesis: the only wait left that can burn 60s is the last one — the VPC Resolver never answers a name in the Profile-shared private zone, so that 300s poll spins until the check is killed. Nothing in the route53profiles crate wires an associated hostedzone ARN into any resolver: the ARN is only classified as associable (services/service-com-amazonaws-route53profiles-route53profiles/src/state.rs:352-353, ASSOCIABLE_ARN_SEGMENTS) and stored on a record that is reported back; there is no path from a stored resource association to the DNS answers served to the VPCs the Profile is associated with.

Why: a client gets a control plane that reports every hop COMPLETE while the sharing has no effect, and — worse than a wrong answer — the data plane never answers at all, so a caller that polls for the shared name (the documented way to wait for a Profile to take effect) hangs until its own timeout rather than being refused. Cross-VPC private DNS via Profiles, and the negative half of the contract (a private zone deliberately left out of the Profile must stay NXDOMAIN, i.e. the Profile must not leak zones it was not given), are both unobservable and unenforceable.

Fix: the VPC DNS resolver data plane the emulator serves to in-VPC workloads (the component answering queries addressed to the VPC Resolver address, over TCP/53 as well as UDP/53), plus the propagation path that composes that answer from a Profile's settled ProfileResourceAssociations for the VPCs named by its settled ProfileAssociations — it must resolve names from a Profile-attached private hosted zone it was given, and only those, leaving a private zone not added to the Profile NXDOMAIN with no records. The route53profiles handlers for AssociateResourceToProfile and AssociateProfile are where that propagation has to be published from; the per-record state in this crate's state.rs is where it has to be readable.

Related open issue, NOT a duplicate: 20261120T045811Z-route53profiles-the-vpc-route-53-resolver-answers-servfail.md reports the same VPC Resolver answering SERVFAIL for a PUBLIC name, is bound to the route53profiles.dns-firewall-resolution-native-protocol-vpc-only check and carries none of this check's replay keys, and concerns the DNS Firewall (FIREWALL_RULE_GROUP) path and public recursion rather than the private-hosted-zone sharing path graded here.

Traces: route53profiles.profile-answers-private-zone-names-in-the-associated-vpc-only-for-zones-added-to-it (mismatch, timed out after 60.0s).

## Evidence
Expected (recorded AWS behavior), all queries issued from inside the second VPC to that VPC's Resolver (<vpc-cidr>.2) over TCP port 53 by a Lambda with an ENI in that VPC:
  - before AssociateProfile: a host name in the private zone -> rcode NXDOMAIN (3), answer section empty.
  - after AssociateResourceToProfile(ResourceArn=arn:<partition>:route53:::hostedzone/<shared-zone-id>) reports Status COMPLETE and AssociateProfile(ProfileId=<profile>, ResourceId=<vpc>) reports Status COMPLETE: that host name -> rcode NOERROR (0), answer section exactly the A records published in the zone for it, each with 0 < TTL <= the record's own TTL.
  - a host name in a second private hosted zone homed on the same other VPC but never added to the Profile -> rcode NXDOMAIN (3), answer section empty.
  - ListProfileResourceAssociations(ProfileId) -> exactly the one zone ARN that was added.

Actual (emulator), route53profiles.profile-answers-private-zone-names-in-the-associated-vpc-only-for-zones-added-to-it [mismatch]:
  Error: Check timed out after 60.0s

No assertion was graded: the check was killed inside one of its polls, so the emulator never produced the state being waited for. The waits and their budgets are get_change->INSYNC (180s), Lambda function_active_v2 (600s), GetProfileResourceAssociation Status->COMPLETE (300s), GetProfileAssociation Status->COMPLETE (600s), and the VPC Resolver answering the shared zone's name with NOERROR + records (300s). The emulator's own constants make the control-plane waits sub-second to 3s (route53 CHANGE_PROPAGATION = 500ms; route53profiles ASSOCIATION_SETTLE_DELAY = 3s; AssociateProfile stores the VPC association already settled), which leaves the Resolver poll as the wait that cannot be satisfied. The route53profiles crate stores an associated hostedzone ARN but has no propagation path from it to any DNS answer, so the shared zone's name is never served inside the associated VPC.

## Steps to Reproduce
1. ec2 CreateVpc (e.g. 10.91.0.0/16) as the zones' HOME VPC and ModifyVpcAttribute EnableDnsHostnames=true on it. Use a SECOND, separate VPC with subnets and a security group as the VPC under test.
2. route53 CreateHostedZone twice with HostedZoneConfig.PrivateZone=true and VPC={VPCRegion,<home vpc>} — one zone to share, one to leave out. ChangeResourceRecordSets UPSERT an A record in each (TTL 60) and poll GetChange until INSYNC. GetHostedZone on both: VPCs must list the home VPC only, so no answer can come from a direct association.
3. route53profiles CreateProfile.
4. lambda CreateFunction (python3.12) with VpcConfig on the VPC under test, whose handler opens TCP port 53 to the VPC Resolver address (<cidr network>+2), sends a length-prefixed recursive A query and returns the reply; wait for State=Active.
5. Invoke it for <host>.<shared apex>. Expected rcode NXDOMAIN (3) with no answers, because the Profile is not associated yet.
6. route53profiles AssociateResourceToProfile(ProfileId, ResourceArn=arn:<partition>:route53:::hostedzone/<shared zone id>, Name=...) and poll GetProfileResourceAssociation until Status is COMPLETE.
7. route53profiles AssociateProfile(ProfileId, ResourceId=<VPC under test>, Name=...) and poll GetProfileAssociation until Status is COMPLETE. ListProfileResourceAssociations(ProfileId) must list exactly the one zone ARN.
8. Re-invoke the relay for <host>.<shared apex>, polling for success. Expected rcode NOERROR with exactly that zone's A record and 0 < TTL <= 60. Observed: no such answer ever appears, and the check is killed by its 60s budget while polling here.
9. Invoke the relay for a host in the zone that was NOT added to the Profile. Expected (unreached): rcode NXDOMAIN with no answers.
