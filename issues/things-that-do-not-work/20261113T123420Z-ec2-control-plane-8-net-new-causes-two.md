Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2.create-network-interface-permission ec2.op-contract-delete-network-interface-permission ec2.subnet-cidr-reservation-and-eni-permission ec2.tags-validation-boundaries ec2.istat-status-record-absent-while-pending-climbs-through-initializing-to-ok ec2.op-contract-describeInstanceStatus ec2.op-contract-modify-instance-cpu-options ec2.op-contract-purchase-scheduled-instances-rejected ec2.ipam-top-pool-reports-none-locale-depth-one-and-omits-allocation-rules ec2.dasgrefresh-run-command-to-a-stopped-warm-pool-instance-rejected-then-that-same-guest-serves
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2.create-network-interface-permission|run#11|AssertionError|assert message == expected_refusal, f"expected message {expected_refusal!r}, got {message!r}"
Issue-Key: ec2.op-contract-delete-network-interface-permission|run#8|AssertionError|assert grant_message == (
Issue-Key: ec2.subnet-cidr-reservation-and-eni-permission|run#29|AssertionError|assert err.get("Message") == (
Issue-Key: ec2.tags-validation-boundaries|run#8|AssertionError|assert reserved_message == expected_reserved, (
Issue-Key: ec2.istat-status-record-absent-while-pending-climbs-through-initializing-to-ok|_poll#9|TimeoutError|raise TimeoutError(f"{what or 'condition'}: the run budget ended first; not graded")
Issue-Key: ec2.op-contract-describeInstanceStatus|run#12|AssertionError|assert code == "InvalidInstanceID.NotFound", f"unexpected error code: {code}"
Issue-Key: ec2.op-contract-modify-instance-cpu-options|run#8|AssertionError|assert code == "InvalidState", f"expected InvalidState, got {code}"
Issue-Key: ec2.op-contract-purchase-scheduled-instances-rejected|run#5|AssertionError|assert err == "InvalidPurchaseToken.Malformed", (
Issue-Key: ec2.ipam-top-pool-reports-none-locale-depth-one-and-omits-allocation-rules|run#10|AssertionError|assert present == [], f"keys never set came back on the pool: {present!r}"
Issue-Key: ec2.dasgrefresh-run-command-to-a-stopped-warm-pool-instance-rejected-then-that-same-guest-serves|run#38|AssertionError|assert serving == {warm_id}, (
Issue-Commit: 6ec43c73f71e6013077e783d53ce3ba5d179d34d

# ec2 control plane: 8 net-new causes — two operations answer 501 NotImplemented instead of their refusals, DescribeInstanceStatus serves nothing and misclassifies an absent id, three refusal messages are emulator prose, DescribeIpamPools adds an unset member, and a warm pool is never promoted

> _Note: this issue was identified against the code base at commit `6ec43c73f71e6013077e783d53ce3ba5d179d34d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeInstanceStatus

## Description
Eight net-new root causes on EC2 control-plane paths. Every EC2 operation below models no errors in Smithy (ec2Query puts error codes outside the model), so each AWS code/message is taken from the recorded response the check asserts.

Cause 1 of 8: CreateNetworkInterfacePermission's cross-account refusal message is emulator prose.
AWS: granting a network-interface permission to an ordinary account is refused OperationNotPermitted with message "The account <AwsAccountId> is not permitted to receive cross account permissions." — the fixed text is everything but <AwsAccountId>, which is the account id from the request. Smithy: CreateNetworkInterfacePermission models no errors; recorded behavior supplies the code and text.
Emulator: the code matches but the message is "You are not authorized to grant permissions to account <AwsAccountId>".
Why: a client that matches on the refusal text (or surfaces it to an operator) sees wording AWS never produces. Note CreateNetworkInterfacePermission was implemented by the now-fixed 20261029T175114Z-ec2-createnetworkinterfacepermission-stub.md; the call is correctly refused, only its wording diverges, so this is a message-fidelity defect rather than a reopen.
Fix: the refusal text in the CreateNetworkInterfacePermission handler's validation (services/service-com-amazonaws-ec2-amazonec2/src/ops/create_network_interface_permission.rs).
Traces: ec2.create-network-interface-permission, ec2.op-contract-delete-network-interface-permission, ec2.subnet-cidr-reservation-and-eni-permission.

Cause 2 of 8: CreateTags' reserved-prefix refusal message is emulator prose.
AWS: a tag key beginning with 'aws:' is refused with message "Value ( <key> ) for parameter key is invalid. Tag keys starting with 'aws:' are reserved for internal use" — fixed text with <key> taken from the request (note the spaces inside the parentheses). Hypothesis: the code is InvalidParameterValue; the recording asserts the message.
Emulator: "Tag key 'aws:reserved-key' is invalid: the 'aws:' prefix is reserved".
Why: a client that asserts on AWS's exact parameter-validation wording, or parses the quoted value out of it, cannot recognise the refusal.
Fix: the reserved-prefix check in the CreateTags handler's validation (.../src/ops/create_tags.rs).
Traces: ec2.tags-validation-boundaries.

Cause 3 of 8: DescribeInstanceStatus serves no status record at all for a live instance.
AWS: once an instance is tracked, DescribeInstanceStatus returns one InstanceStatuses entry whose SystemStatus, InstanceStatus and AttachedEbsStatus climb from initializing to ok. Smithy: DescribeInstanceStatus output member InstanceStatuses (instanceStatusSet).
Emulator: the list stays empty for the whole poll — "all of ('SystemStatus', 'InstanceStatus', 'AttachedEbsStatus') reaching exactly 'ok' for i-<id> -- trajectory so far []". The open issue 20261103T071133Z-ec2-describeinstancestatus-stub.md calls the operation a scaffold stub, but the emulator answers 200 here (and a client error in Cause 4), so that issue does not describe or carry these failures.
Why: every readiness gate built on instance_status_ok (waiters, deployment scripts) hangs forever against the emulator.
Fix: the DescribeInstanceStatus handler and the instance status state it reads (.../src/ops/describe_instance_status.rs).
Traces: ec2.istat-status-record-absent-while-pending-climbs-through-initializing-to-ok.

Cause 4 of 8: DescribeInstanceStatus calls a well-formed absent instance id malformed.
AWS: a syntactically valid instance id that names nothing is InvalidInstanceID.NotFound with message "The instance ID '<id>' does not exist"; InvalidInstanceID.Malformed is reserved for ids that are not well formed.
Emulator: every single-hex-digit rotation of a real, well-formed id is refused InvalidInstanceID.Malformed, so the NotFound class is unreachable.
Why: a client that distinguishes "gone" from "bad input" — retry/cleanup logic keyed on InvalidInstanceID.NotFound — takes the wrong branch.
Fix: the id parsing/validation the DescribeInstanceStatus handler applies before lookup (.../src/ops/describe_instance_status.rs).
Traces: ec2.op-contract-describeInstanceStatus.

Cause 5 of 8: ModifyInstanceCpuOptions answers 501 NotImplemented instead of refusing a running instance.
AWS: CPU options can only change on a stopped instance; the call against a running instance is refused with code InvalidState (recorded behavior; the operation models no errors).
Emulator: HTTP 501 with code NotImplemented. The operation is not marked as a scaffold stub by the suite, so no stub issue covers it.
Why: a client that catches InvalidState and stops the instance before retrying instead sees a transport-level 501 and gives up.
Fix: the ModifyInstanceCpuOptions handler (.../src/ops/modify_instance_cpu_options.rs) — implement the state gate and the InvalidState refusal.
Traces: ec2.op-contract-modify-instance-cpu-options.

Cause 6 of 8: PurchaseScheduledInstances answers 501 NotImplemented instead of refusing a bad purchase token.
AWS: a malformed PurchaseToken is refused InvalidPurchaseToken.Malformed with message "The specified purchase token is not valid." (fixed text, no variable part).
Emulator: HTTP 501 with code NotImplemented.
Why: the operation's only negative class is unobservable, and a client sees a server-side 501 for what is a client error.
Fix: the PurchaseScheduledInstances handler and its token validation (.../src/ops/purchase_scheduled_instances.rs).
Traces: ec2.op-contract-purchase-scheduled-instances-rejected.

Cause 7 of 8: DescribeIpamPools reports members that were never set.
AWS: a top-level IPAM pool created without allocation rules omits SourceIpamPoolId, AllocationMinNetmaskLength, AllocationMaxNetmaskLength, AllocationDefaultNetmaskLength, AllocationResourceTags and PublicIpSource entirely.
Emulator: AllocationResourceTags comes back on a pool that never set it — "keys never set came back on the pool: ['AllocationResourceTags']".
Why: a client that treats the presence of an allocation rule as "a rule is in force" (or diffs a pool against its desired state) sees a rule that does not exist and reports drift.
Fix: the DescribeIpamPools serializer — omit unset optional members instead of emitting empty ones (.../src/ops/describe_ipam_pools.rs).
Traces: ec2.ipam-top-pool-reports-none-locale-depth-one-and-omits-allocation-rules.

Cause 8 of 8: a scale-out launches a cold instance instead of promoting the warm-pool instance.
AWS: an Auto Scaling group with a warm pool promotes a pre-initialised warm instance into service on scale-out; the group serves that instance id, and no new instance is launched.
Emulator: the warm instance is ignored and a cold one is launched — "scaling out must promote the pre-initialised warm-pool instance 'i-<warm>', not launch a cold one; the group is serving ['i-<cold>']".
Why: the entire point of a warm pool (fast scale-out from an already-booted guest) is lost, and a client that tracks which instance serves after a scale-out reads the wrong id.
Fix: the warm-pool promotion path of the Auto Scaling scale-out handler reached by this ec2-headed check (the autoscaling crate's desired-capacity change / warm-pool state), which must prefer a Warmed:Stopped instance over a new launch.
Traces: ec2.dasgrefresh-run-command-to-a-stopped-warm-pool-instance-rejected-then-that-same-guest-serves.

## Evidence
Cause 1 — expected message 'The account 111122223333 is not permitted to receive cross account permissions.', got 'You are not authorized to grant permissions to account 111122223333' (code OperationNotPermitted matched).
Cause 2 — expected message "Value ( aws:reserved-key ) for parameter key is invalid. Tag keys starting with 'aws:' are reserved for internal use", got "Tag key 'aws:reserved-key' is invalid: the 'aws:' prefix is reserved".
Cause 3 — TimeoutError: all of ('SystemStatus', 'InstanceStatus', 'AttachedEbsStatus') reaching exactly 'ok' for i-00000000000000001 -- trajectory so far [].
Cause 4 — AssertionError: unexpected error code: InvalidInstanceID.Malformed (expected InvalidInstanceID.NotFound with message "The instance ID '<id>' does not exist").
Cause 5 — AssertionError: expected InvalidState, got NotImplemented (HTTP 501).
Cause 6 — AssertionError: expected InvalidPurchaseToken.Malformed, got NotImplemented (HTTP 501); AWS message "The specified purchase token is not valid.".
Cause 7 — AssertionError: keys never set came back on the pool: ['AllocationResourceTags'].
Cause 8 — AssertionError: scaling out must promote the pre-initialised warm-pool instance 'i-00000000000000001', not launch a cold one; the group is serving ['i-00000000000000002'].

## Steps to Reproduce
1. CreateNetworkInterface, then CreateNetworkInterfacePermission(AwsAccountId=<ordinary account>, Permission=EIP-ASSOCIATE): compare the OperationNotPermitted message.
2. CreateTags on a real resource with key 'aws:reserved-key': compare the refusal message.
3. RunInstances, then poll DescribeInstanceStatus(InstanceIds=[id], IncludeAllInstances=true): InstanceStatuses stays empty.
4. DescribeInstanceStatus with a well-formed id that names nothing: code is InvalidInstanceID.Malformed.
5. ModifyInstanceCpuOptions(InstanceId=<running>, CoreCount=1, ThreadsPerCore=1): HTTP 501 NotImplemented.
6. PurchaseScheduledInstances with a malformed PurchaseToken: HTTP 501 NotImplemented.
7. CreateIpamPool with no allocation rules, then DescribeIpamPools: AllocationResourceTags is present.
8. Create an ASG with a warm pool, wait for a Warmed:Stopped instance, then raise desired capacity: a new cold instance is launched.
