Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2.spotint-cancelling-a-fulfilled-spot-request-leaves-the-guest-running-with-no-notice ec2.spotint-spot-price-below-market-is-never-fulfilled-and-launches-no-instance ec2.run-instances-client-token-with-changed-parameters-rejected ec2.hibernate-launch-with-root-volume-too-small-for-ram-rejected ec2.spotint-spot-request-fulfilment-boots-a-guest-that-reports-no-interruption-notice
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2.spotint-cancelling-a-fulfilled-spot-request-leaves-the-guest-running-with-no-notice|run#11|KeyError|assert inst["InstanceLifecycle"] == "spot", (
Issue-Key: ec2.spotint-spot-price-below-market-is-never-fulfilled-and-launches-no-instance|run#21|Timeout|time.sleep(2)
Issue-Key: ec2.run-instances-client-token-with-changed-parameters-rejected|_legs#3|AssertionError|assert err.get("Message") == _MISMATCH_MESSAGE, err.get("Message")
Issue-Key: ec2.hibernate-launch-with-root-volume-too-small-for-ram-rejected|run#17|AssertionError|assert message == (
Issue-Key: ec2.spotint-spot-request-fulfilment-boots-a-guest-that-reports-no-interruption-notice|run#29|AssertionError|assert probe["Status"] == "Success", (
Issue-Commit: c3e5fb924e64874a3c7124e31a5bf7fe6194984f

# ec2 RunInstances and Spot purchase: 5 net-new causes — a MarketType=spot launch is not a spot instance, an under-bid spot request never settles, two RunInstances refusal messages are emulator-authored prose, and the in-guest shell has no coreutils

> _Note: this issue was identified against the code base at commit `c3e5fb924e64874a3c7124e31a5bf7fe6194984f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RunInstances

## Description
Five net-new root causes on the ec2 RunInstances / Spot-purchase path. All five must be closed before this issue is submitted.

=== Cause 1 of 5: RunInstances with InstanceMarketOptions MarketType=spot does not mint a spot instance — the Instance carries no InstanceLifecycle (and no SpotInstanceRequestId) ===
AWS: Smithy (services/service-com-amazonaws-ec2-amazonec2/smithy-model/com-amazonaws-ec2-amazonec2.json) declares Instance$InstanceLifecycle targeting the InstanceLifecycleType enum [spot, scheduled, capacity-block, interruptible-capacity-reservation] and Instance$SpotInstanceRequestId. Recorded behavior: a RunInstances carrying InstanceMarketOptions={MarketType: "spot", SpotOptions:{SpotInstanceType:"one-time", InstanceInterruptionBehavior:"terminate"}} returns an instance whose InstanceLifecycle is "spot" and which names the implicit spot request's id in SpotInstanceRequestId; that request is then describable (DescribeSpotInstanceRequests), and after CancelSpotInstanceRequests it reports State "cancelled" with Status.Code "request-canceled-and-instance-running" while the instance keeps InstanceLifecycle "spot".
Emulator: the RunInstances response instance has no InstanceLifecycle member at all (the check raises KeyError: 'InstanceLifecycle'), so the purchase option is dropped: the launch is served as an ordinary on-demand instance and no implicit spot request is minted to describe or cancel.
Why: a client cannot tell a spot instance from an on-demand one, cannot find the request id to cancel or to tag, and every spot-aware behaviour downstream (interruption handling, request-cancelled-but-instance-running, cost attribution) is unobservable.
Fix: the RunInstances handler services/service-com-amazonaws-ec2-amazonec2/src/ops/run_instances.rs and the instance record in crate::state — honour InstanceMarketOptions by creating the implicit spot request, storing the lifecycle on the instance, and serializing InstanceLifecycle/SpotInstanceRequestId on RunInstances and DescribeInstances.
Traces: ec2.spotint-cancelling-a-fulfilled-spot-request-leaves-the-guest-running-with-no-notice.

=== Cause 2 of 5: a RequestSpotInstances bid below the market never settles out of pending-evaluation, so the price-too-low outcome is unreachable ===
AWS: no modelled errors on RequestSpotInstances (ec2Query — recorded behavior only); the request record carries State and Status.Code. Recorded behavior: a one-time request whose SpotPrice is below the live market price is ACCEPTED, then settles to State "open" with Status.Code "price-too-low", names no InstanceId, and launches nothing, while the same launch specification with no maximum price is fulfilled.
Emulator: the check never gets a verdict — it ends `Check timed out after 60.0s` while its only wait before the first assertion is the bounded poll for the request leaving Status.Code "pending-evaluation". Hypothesis: spot requests are never evaluated against a market price, so they stay in pending-evaluation forever and neither price-too-low nor fulfilment is ever reported.
Why: a client cannot observe why a spot request is not fulfilled; a bid that AWS parks as open/price-too-low hangs in pending-evaluation, so capacity planning, bid-tuning and any "did my request fail on price" branch never resolve.
Fix: the spot request state machine behind services/service-com-amazonaws-ec2-amazonec2/src/ops/request_spot_instances.rs and describe_spot_instance_requests.rs — evaluate an accepted request against the spot price the service publishes (DescribeSpotPriceHistory) and settle it to open/price-too-low (no InstanceId) or to fulfilment.
Traces: ec2.spotint-spot-price-below-market-is-never-fulfilled-and-launches-no-instance.

=== Cause 3 of 5: the IdempotentParameterMismatch message for a reused RunInstances ClientToken is emulator-authored prose ===
AWS: no modelled errors on RunInstances (recorded behavior only). Recorded behavior: reusing a ClientToken with changed parameters is refused IdempotentParameterMismatch with the fixed message `Arguments on this idempotent request are inconsistent with arguments used in previous request(s).` (no variable parts), and nothing is created.
Emulator: the error code matches but the message is `The request uses the same client token as a previous, but non-identical request.` — text AWS never emits for this class.
Why: the message is part of the refusal contract; operators and tooling that surface or match AWS's wording see different text for the same refusal, and a client cannot recognise the documented idempotency failure from the message it is given.
Fix: the client-token idempotency check in the RunInstances handler services/service-com-amazonaws-ec2-amazonec2/src/ops/run_instances.rs must render AWS's fixed text for IdempotentParameterMismatch.
Traces: ec2.run-instances-client-token-with-changed-parameters-rejected.

=== Cause 4 of 5: the UnsupportedHibernationConfiguration message for a root volume too small for RAM is emulator-authored prose ===
AWS: no modelled errors on RunInstances (recorded behavior only). Recorded behavior: a hibernation-enabled launch (HibernationOptions.Configured=true) whose encrypted root volume is smaller than the instance type's RAM is refused UnsupportedHibernationConfiguration with the fixed message `The root device does not have the disk space required for hibernation. Increase the root device volume size so that it has enough free space to accommodate the total RAM for your instance type and your expected volume usage, and try again.` (no variable parts), leaving no instance and no volume behind.
Emulator: the code matches but the message is `For hibernation, the root device volume must be large enough to store the instance memory (RAM) and accommodate your expected usage.`
Why: same contract break as Cause 3 — the refusal is correct but unrecognisable; a client or operator reading AWS's documented sentence gets different prose, and message-based assertions and runbooks fail.
Fix: the hibernation precondition refusal in services/service-com-amazonaws-ec2-amazonec2/src/ops/run_instances.rs (its hibernation validation) must render AWS's fixed text.
Traces: ec2.hibernate-launch-with-root-volume-too-small-for-ram-rejected.

=== Cause 5 of 5: the guest an EC2 instance boots has no basic shell utilities, so an in-guest AWS-RunShellScript probe ends Failed ===
AWS: recorded behavior only — the AL2023 AMI the check launches boots a guest whose shell provides the standard coreutils, so an AWS-RunShellScript invocation that uses mktemp/cat/wc/grep/rm completes with Status "Success" and ResponseCode 0 and reports the IMDS spot-interruption paths it probes.
Emulator: the invocation ends Status "Failed" with stderr `sh: mktemp: command not found` followed by the cascade of `cat: : No such file or directory`, `wc: : No such file or directory`, `grep: : No such file or directory`, `rm: cannot remove '': Is a directory` — the guest image has no mktemp, so every later command receives an empty path. Nothing about the instance's spot/interruption state can be observed from inside the guest. (Open issues 20261115T003554Z-ec2-guests-observe-nothing-in-guest-commands-exit.md and 20261115T000000Z-ec2-ssm-a-managed-nodes-guest-has-no-network-programs.md record neighbouring guest-environment gaps; the replay did not mark this check as covered by either.)
Why: any scenario whose truth is only visible inside the guest (IMDS reads, interruption notices, filesystem state) cannot be run: the command the client sends fails on the environment rather than on the behaviour under test.
Fix: the guest image/filesystem the EC2 data plane boots and the AWS-RunShellScript execution path that runs commands in it (services/service-com-amazonaws-ec2-amazonec2 guest/data-plane support) must provide the AMI's standard coreutils so a shell script runs as recorded.
Traces: ec2.spotint-spot-request-fulfilment-boots-a-guest-that-reports-no-interruption-notice.

## Evidence
Cause 1 — Expected: RunInstances with InstanceMarketOptions {MarketType: spot} returns an instance with InstanceLifecycle == "spot" and a non-empty SpotInstanceRequestId. Actual: `KeyError: 'InstanceLifecycle'` at `assert inst["InstanceLifecycle"] == "spot"` — the member is absent from the response.

Cause 2 — Expected: the under-bid request settles to State "open", Status.Code "price-too-low", with no InstanceId, within the poll window. Actual: `Error: Check timed out after 60.0s` with no verdict; the only wait before the first assertion is the poll for Status.Code != "pending-evaluation".

Cause 3 — Expected message (code IdempotentParameterMismatch): `Arguments on this idempotent request are inconsistent with arguments used in previous request(s).` Actual message: `The request uses the same client token as a previous, but non-identical request.`

Cause 4 — Expected message (code UnsupportedHibernationConfiguration): `The root device does not have the disk space required for hibernation. Increase the root device volume size so that it has enough free space to accommodate the total RAM for your instance type and your expected volume usage, and try again.` Actual message: `For hibernation, the root device volume must be large enough to store the instance memory (RAM) and accommodate your expected usage.`

Cause 5 — Expected: the in-guest probe invocation reports Status "Success" / ResponseCode 0. Actual: `AssertionError: the in-guest probe was 'Failed': 'Failed' stderr="sh: mktemp: command not found\ncat: : No such file or directory\ncat: : No such file or directory\nwc: : No such file or directory\ngrep: : No such file or directory\ngrep: : No such file or directory\nwc: : No such file or directory\ngrep: : No such file or directory\nrm: cannot remove '': Is a directory\n"`.

## Steps to Reproduce
Cause 1: in a VPC/subnet of your own, RunInstances(ImageId=<AL2023 AMI>, InstanceType=t3.micro, SubnetId=<subnet>, InstanceMarketOptions={"MarketType":"spot","SpotOptions":{"SpotInstanceType":"one-time","InstanceInterruptionBehavior":"terminate"}}) — inspect Instances[0]: InstanceLifecycle and SpotInstanceRequestId are missing.

Cause 2: read a market price with DescribeSpotPriceHistory for t3.micro/Linux-UNIX in the AZ, then RequestSpotInstances(Type="one-time", InstanceCount=1, SpotPrice="0.001", LaunchSpecification={ImageId, InstanceType=t3.micro, SubnetId, SecurityGroupIds}) and poll DescribeSpotInstanceRequests — the request stays at Status.Code "pending-evaluation" and never reaches State open / Status.Code price-too-low.

Cause 3: RunInstances(ClientToken="tok", InstanceType=t3.micro, ...), then RunInstances with the SAME ClientToken but InstanceType=t3.small — observe code IdempotentParameterMismatch with the emulator's own message instead of AWS's fixed text.

Cause 4: RunInstances(InstanceType=m5.large, HibernationOptions={"Configured":true}, BlockDeviceMappings=[{DeviceName:<root>, Ebs:{Encrypted:true, VolumeType:"gp3", VolumeSize:<fewer GiB than the type's RAM>}}]) — observe UnsupportedHibernationConfiguration with the emulator's own message.

Cause 5: launch an AL2023 instance with an SSM-capable instance profile, wait for it to be a managed node, then SendCommand(DocumentName="AWS-RunShellScript") with a script that uses mktemp/cat/wc/grep/rm — the invocation ends Failed with `sh: mktemp: command not found`.
