Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2.concurrent-modify-ipam-resource-cidr-conflicting-updates
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2.concurrent-modify-ipam-resource-cidr-conflicting-updates|_poll_until#21|Timeout|time.sleep(interval_s)
Issue-Commit: 0e9afbb76f97c880d990543e0ccf2d5bfecf65a1

# ec2: the IPAM resource-CIDR race scenario never reaches a verdict — two concurrent ModifyIpamResourceCidr calls on one pool-allocated VPC CIDR stall past the harness cap

> _Note: this issue was identified against the code base at commit `0e9afbb76f97c880d990543e0ccf2d5bfecf65a1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ModifyIpamResourceCidr

## Description
Cause 1 of 1: the concurrent-ModifyIpamResourceCidr scenario on a pool-allocated VPC's resource CIDR produces no observable settled state inside the replay budget — the check is cut off at the 60 s harness cap with no response to grade.

AWS: recorded behavior only (the ec2 Smithy model declares ModifyIpamResourceCidr and GetIpamResourceCidrs but states nothing about concurrency or reconciliation latency). The recording shows the sequence completing well inside the budget: after CreateVpc with Ipv4IpamPoolId/Ipv4NetmaskLength, GetIpamResourceCidrs(IpamScopeId=<private default scope>, ResourceId=<vpc>) surfaces the VPC's resource CIDR; two simultaneous ModifyIpamResourceCidr calls on that same ResourceId/ResourceCidr/ResourceRegion — one with Monitored=true, one with Monitored=false — both return (each either succeeding or being refused with a modelled error code), the record converges to exactly one of ManagementState `managed` or `ignored`, and a further ModifyIpamResourceCidr asserting that same state round-trips and reports it back. No step hangs.

Emulator: the check never gets that far — it ends with "Check timed out after 60.0s" and no AWS response to compare, so one of the ec2 IPAM calls in that sequence does not return (or does not converge) in bounded time. Hypothesis: either GetIpamResourceCidrs never surfaces the pool-allocated VPC's resource CIDR (the preceding poll has a 900 s deadline, so a never-appearing record consumes the whole harness budget), or the two simultaneous ModifyIpamResourceCidr calls on one record contend and at least one never completes. Only the stall is recorded; the status/code of the hanging call is unknown because none was returned.

Why: a client that reconciles IPAM monitoring state — anything that flips Monitored on a VPC's resource CIDR, or that reads GetIpamResourceCidrs to discover pool-allocated VPCs — blocks forever instead of getting an answer. Two writers to one record get neither a success nor a modelled conflict error, so a caller has no outcome to retry on and no converged ManagementState to read.

Fix: the ec2 IPAM state and the ModifyIpamResourceCidr / GetIpamResourceCidrs handlers: the resource-CIDR record for a pool-allocated VPC must be discoverable through GetIpamResourceCidrs in bounded time, and concurrent ModifyIpamResourceCidr calls on one record must each terminate (returning a result or a modelled refusal) and leave the record in exactly one of the valid ManagementState values rather than holding a lock or spinning.

Traces: ec2.concurrent-modify-ipam-resource-cidr-conflicting-updates.

## Evidence
ec2.concurrent-modify-ipam-resource-cidr-conflicting-updates:
  Error: Check timed out after 60.0s

The check's sequence (from its source in the replay report): CreateIpam -> poll State==create-complete; CreateIpamPool -> poll State==create-complete; ProvisionIpamPoolCidr 10.222.0.0/16 -> poll state==provisioned; CreateVpc(Ipv4IpamPoolId=<pool>, Ipv4NetmaskLength=24); poll GetIpamResourceCidrs(IpamScopeId=<private default scope>, ResourceId=<vpc>) for the VPC's resource CIDR (900 s deadline); then two ModifyIpamResourceCidr calls released together from a threading.Barrier(2) with Monitored=True and Monitored=False; then poll ManagementState in ("managed", "ignored") and re-assert that state with one more ModifyIpamResourceCidr.

No AWS response was returned to grade: the run produced only the timeout, so the emulator served nothing for at least one call in that chain within 60 s, where the recording completes the whole chain.

## Steps to Reproduce
1. CreateIpam with OperatingRegions=[<region>]; wait for State create-complete. Note PrivateDefaultScopeId.
2. CreateIpamPool(IpamScopeId=<scope>, AddressFamily=ipv4, Locale=<region>); wait create-complete. ProvisionIpamPoolCidr(Cidr=10.222.0.0/16); wait state provisioned.
3. CreateVpc(Ipv4IpamPoolId=<pool>, Ipv4NetmaskLength=24).
4. Poll GetIpamResourceCidrs(IpamScopeId=<scope>, ResourceId=<vpc>) until the VPC's resource CIDR appears.
5. From two threads released at the same instant, call ModifyIpamResourceCidr(ResourceId=<vpc>, ResourceCidr=<cidr>, ResourceRegion=<region>, CurrentIpamScopeId=<scope>) with Monitored=true and Monitored=false.
6. Expected: both calls return (success or a modelled error), GetIpamResourceCidrs then reports ManagementState managed or ignored, and re-asserting that state round-trips. Observed: the sequence does not complete — the run is cut off at the 60 s cap with no response.
