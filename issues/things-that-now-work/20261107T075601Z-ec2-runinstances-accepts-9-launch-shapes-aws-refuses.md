Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2.arm64-ami-on-x86-64-instance-type-rejected ec2.arm64-instance-type-with-x86-64-ami-rejected ec2.cpuopts-core-count-outside-valid-cores-rejected ec2.cpuopts-partial-cpu-options-rejected ec2.hibernate-launch-on-a-type-without-hibernation-support-rejected ec2.hibernate-launch-with-root-volume-too-small-for-ram-rejected ec2.hibernate-guest-process-and-its-in-memory-value-survive-the-resume ec2.hibernate-stop-with-hibernate-on-an-instance-not-configured-for-it-rejected ec2.run-instances-client-token-with-changed-parameters-rejected ec2.ec2mr-dvpc-run-instances-rejects-nonexistent-subnet ec2.vscan-run-instances-with-an-unknown-instance-profile-rejected-invalid-parameter-value ec2.vscan-public-dns-name-appears-only-once-vpc-dns-hostnames-is-enabled ec2.timesync-chrony-in-the-guest-locks-onto-the-link-local-amazon-time-sync-source ec2.timesync-guest-clock-agrees-with-the-aws-api-date-header-inside-two-seconds ec2.vscan-encrypted-root-mapping-from-an-unencrypted-snapshot-uses-a-kms-key ec2.vscan-unencrypted-root-mapping-round-trips-onto-the-launched-volume ec2.spotint-one-time-spot-request-refuses-interruption-behavior-stop ec2.spotint-spot-price-below-market-is-never-fulfilled-and-launches-no-instance ec2.spotint-cancelling-a-fulfilled-spot-request-leaves-the-guest-running-with-no-notice ec2.spotint-spot-request-fulfilment-boots-a-guest-that-reports-no-interruption-notice
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2.arm64-ami-on-x86-64-instance-type-rejected|_launch_attempt#12|AssertionError|raise AssertionError(
Issue-Key: ec2.arm64-instance-type-with-x86-64-ami-rejected|_launch_attempt#13|AssertionError|raise AssertionError(
Issue-Key: ec2.cpuopts-core-count-outside-valid-cores-rejected|run#18|AssertionError|raise AssertionError(
Issue-Key: ec2.cpuopts-partial-cpu-options-rejected|run#15|AssertionError|raise AssertionError(
Issue-Key: ec2.hibernate-launch-on-a-type-without-hibernation-support-rejected|_refusal#13|AssertionError|raise AssertionError(
Issue-Key: ec2.hibernate-launch-with-root-volume-too-small-for-ram-rejected|_refusal#15|AssertionError|raise AssertionError(
Issue-Key: ec2.hibernate-guest-process-and-its-in-memory-value-survive-the-resume|run#24|AssertionError|assert vol["Encrypted"] is True and vol["Size"] >= mem_gib, (
Issue-Key: ec2.hibernate-stop-with-hibernate-on-an-instance-not-configured-for-it-rejected|run#27|AssertionError|assert inst.get("StateReason", {}).get("Code") == _REASON_HIBERNATE, (
Issue-Key: ec2.run-instances-client-token-with-changed-parameters-rejected|_client_error#3|AssertionError|raise AssertionError(
Issue-Key: ec2.ec2mr-dvpc-run-instances-rejects-nonexistent-subnet|_client_error#8|AssertionError|raise AssertionError(f"the call succeeded; real AWS rejects it. Result: {result!r}")
Issue-Key: ec2.vscan-run-instances-with-an-unknown-instance-profile-rejected-invalid-parameter-value|run#10|AssertionError|assert got == (
Issue-Key: ec2.vscan-public-dns-name-appears-only-once-vpc-dns-hostnames-is-enabled|run#11|AssertionError|assert ip.count(".") == 3, f"the public subnet must give the host a public IPv4, got {ip!r}"
Issue-Key: ec2.timesync-chrony-in-the-guest-locks-onto-the-link-local-amazon-time-sync-source|run#12|KeyError|assert running["PublicIpAddress"], (
Issue-Key: ec2.timesync-guest-clock-agrees-with-the-aws-api-date-header-inside-two-seconds|run#12|KeyError|assert running["PublicIpAddress"], (
Issue-Key: ec2.vscan-encrypted-root-mapping-from-an-unencrypted-snapshot-uses-a-kms-key|run#14|AssertionError|assert vol["Encrypted"] is True, f"the overridden root volume must be encrypted: {vol!r}"
Issue-Key: ec2.vscan-unencrypted-root-mapping-round-trips-onto-the-launched-volume|run#22|AssertionError|assert got == (8, "gp2", default, 100, snapshot), (
Issue-Key: ec2.spotint-one-time-spot-request-refuses-interruption-behavior-stop|run#10|AssertionError|raise AssertionError(
Issue-Key: ec2.spotint-spot-price-below-market-is-never-fulfilled-and-launches-no-instance|run#17|AssertionError|assert settled["Status"]["Code"] == "price-too-low", (
Issue-Key: ec2.spotint-cancelling-a-fulfilled-spot-request-leaves-the-guest-running-with-no-notice|run#11|KeyError|assert inst["InstanceLifecycle"] == "spot", (
Issue-Key: ec2|RequestSpotInstances|NotImplemented
Issue-Commit: 620c90c23a321396910fd66a692ef2fcecf33f7f

# EC2 RunInstances accepts 9 launch shapes AWS refuses and misrepresents the launched instance (architecture, CpuOptions, hibernation, idempotency, public IPv4, EBS mapping, spot)

> _Note: this issue was identified against the code base at commit `620c90c23a321396910fd66a692ef2fcecf33f7f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RunInstances

## Description
Nine net-new root causes on the EC2 launch path. Each must be closed before submitting.

Cause 1 of 9: RunInstances does not check the AMI architecture against the instance type.
AWS: launching an arm64 AMI on an x86_64 type (or an x86_64 AMI on t4g) is refused InvalidParameterValue — "The architecture 'arm64' of the specified instance type does not match the architecture 'x86_64' of the specified AMI..." (recorded behavior; ec2 Smithy ArchitectureValues on Image.Architecture and InstanceTypeInfo.ProcessorInfo.SupportedArchitectures) — and no instance is created.
Emulator: both launches are ACCEPTED and an instance is created.
Why: an impossible fleet is created; a client validating its AMI/type matrix gets a false pass.
Fix: RunInstances validation (compare Image.Architecture to the type's SupportedArchitectures).
Traces: ec2.arm64-ami-on-x86-64-instance-type-rejected, ec2.arm64-instance-type-with-x86-64-ami-rejected.

Cause 2 of 9: CpuOptions are neither validated nor honoured.
AWS: Smithy CpuOptionsRequest — "The CPU options for the instance. Both the core count and threads per core must be specified in the request." A partial CpuOptions is refused, and a CoreCount outside the type's ValidCores (m5.large: [1]) is refused InvalidParameterValue; a valid pair is reflected back on the instance.
Emulator: accepts CoreCount=2 on m5.large and accepts CpuOptions={"ThreadsPerCore": 1} alone, then invents the type default {"CoreCount": 1, "ThreadsPerCore": 2} on the created instance.
Why: a request AWS refuses succeeds and silently produces a topology other than the one asked for, so nothing downstream can trust CpuOptions.
Fix: RunInstances CpuOptions validation + the instance's CpuOptions projection.
Traces: ec2.cpuopts-core-count-outside-valid-cores-rejected, ec2.cpuopts-partial-cpu-options-rejected.

Cause 3 of 9: hibernation prerequisites are not enforced and a hibernating stop is not represented.
AWS: Smithy HibernationOptionsRequest is "valid only if the instance meets the hibernation prerequisites": a type that does not support hibernation (g6f.large) and a root volume too small for RAM are both refused UnsupportedHibernationConfiguration, hibernation requires an encrypted root volume, and StopInstances(Hibernate=true) on a configured instance leaves StateReason Code "Client.UserInitiatedHibernate".
Emulator: both unsupported launches are ACCEPTED and instances created; the root volume comes back Encrypted=False although the mapping asked for encryption; and the hibernating stop records StateReason Client.UserInitiatedShutdown ("User initiated shutdown").
Why: the unsupported configurations AWS refuses are silently created, and a client cannot tell a hibernate from a plain stop.
Fix: RunInstances hibernation validation, the block-device-mapping encryption path, and the StopInstances(Hibernate) state-reason writer.
Traces: ec2.hibernate-launch-on-a-type-without-hibernation-support-rejected, ec2.hibernate-launch-with-root-volume-too-small-for-ram-rejected, ec2.hibernate-guest-process-and-its-in-memory-value-survive-the-resume, ec2.hibernate-stop-with-hibernate-on-an-instance-not-configured-for-it-rejected.

Cause 4 of 9: RunInstances ignores ClientToken idempotency.
AWS: replaying a ClientToken with different parameters is refused IdempotentParameterMismatch (recorded behavior; ec2 RunInstances ClientToken member documents the idempotency contract).
Emulator: the second, parameter-changed call SUCCEEDS and launches another instance.
Why: a retrying client duplicates instances instead of being told its retry does not match.
Fix: RunInstances client-token bookkeeping in state.
Traces: ec2.run-instances-client-token-with-changed-parameters-rejected.

Cause 5 of 9: RunInstances accepts a SubnetId that does not exist.
AWS: refused InvalidSubnetID.NotFound naming the subnet id, with no reservation created.
Emulator: returns a 200 reservation with an instance whose NetworkInterfaces/SecurityGroups are empty.
Why: a typo'd subnet yields a half-materialised instance with no network identity instead of a refusal.
Fix: RunInstances subnet resolution/validation.
Traces: ec2.ec2mr-dvpc-run-instances-rejects-nonexistent-subnet.

Cause 6 of 9: a DryRun launch naming a nonexistent IAM instance profile answers DryRunOperation.
AWS: parameter validation runs before the dry-run short-circuit, so a nonexistent profile is refused InvalidParameterValue — "Value (<name>) for parameter iamInstanceProfile.name is invalid. Invalid IAM Instance Profile name" (fixed text; the name comes from the request).
Emulator: ('DryRunOperation', 'Request would have succeeded, but DryRun flag is set.').
Why: validate-first clients are told a launch that cannot succeed would succeed.
Fix: RunInstances DryRun ordering — validate the instance profile before reporting DryRunOperation.
Traces: ec2.vscan-run-instances-with-an-unknown-instance-profile-rejected-invalid-parameter-value.

Cause 7 of 9: an instance in a MapPublicIpOnLaunch subnet never gets a public IPv4 address.
AWS: a subnet with MapPublicIpOnLaunch=true gives the instance a PublicIpAddress (and, once the VPC enables DNS hostnames, a PublicDnsName); DescribeInstances carries both.
Emulator: PublicIpAddress is absent/empty for such an instance, so clients indexing it raise KeyError and the public-exposure derivation selects nothing.
Why: every public-reachability derivation and any client addressing the instance by its public address is wrong.
Fix: the RunInstances network path that honours subnet MapPublicIpOnLaunch, and the instance serializer's PublicIpAddress/PublicDnsName members.
Traces: ec2.vscan-public-dns-name-appears-only-once-vpc-dns-hostnames-is-enabled, ec2.timesync-chrony-in-the-guest-locks-onto-the-link-local-amazon-time-sync-source, ec2.timesync-guest-clock-agrees-with-the-aws-api-date-header-inside-two-seconds.

Cause 8 of 9: block-device-mapping overrides are dropped and gp2 volumes omit their derived Iops.
AWS: a root mapping that sets Encrypted=true (with a KmsKeyId) produces an encrypted volume; a gp2 volume reports its derived baseline Iops (100 for 8 GiB) and omits Throughput/KmsKeyId when unencrypted.
Emulator: the launched root volume reports Encrypted=False despite the override, and its Iops reads None.
Why: an encryption-at-rest assertion passes against an unencrypted volume, and performance-shape reads are empty.
Fix: the RunInstances block-device-mapping -> volume creation path and the DescribeVolumes projection of Encrypted/Iops.
Traces: ec2.vscan-encrypted-root-mapping-from-an-unencrypted-snapshot-uses-a-kms-key, ec2.vscan-unencrypted-root-mapping-round-trips-onto-the-launched-volume.

Cause 9 of 9: Spot requests are neither validated, priced, nor represented.
AWS: RequestSpotInstances with Type=one-time and InstanceInterruptionBehavior=stop is refused (an interruption behaviour the request cannot perform); a bid below the market price stays open with status code "price-too-low" and launches nothing; a fulfilled request's instance reports InstanceLifecycle="spot"; and a spot request naming an IAM instance profile is a modelled, supported shape.
Emulator: accepts one-time+stop and returns a request id; reports a below-market bid as Code "fulfilled" (state active); omits InstanceLifecycle from the launched instance (KeyError); and answers NotImplemented for RequestSpotInstances with an IAM instance profile.
Why: spot economics and spot-vs-on-demand identification are fabricated — a client cannot tell a spot instance from an on-demand one, nor an unfulfillable bid from a fulfilled one.
Fix: the RequestSpotInstances handler (validation, status-code state machine, IAM-profile support) and the Instance.InstanceLifecycle projection.
Traces: ec2.spotint-one-time-spot-request-refuses-interruption-behavior-stop, ec2.spotint-spot-price-below-market-is-never-fulfilled-and-launches-no-instance, ec2.spotint-cancelling-a-fulfilled-spot-request-leaves-the-guest-running-with-no-notice, ec2.spotint-spot-request-fulfilment-boots-a-guest-that-reports-no-interruption-notice.

## Evidence
From the behavioral replay:
- arm64-ami-on-x86-64-instance-type-rejected / arm64-instance-type-with-x86-64-ami-rejected [mismatch]: "RunInstances accepted it and launched 'i-...'".
- cpuopts-core-count-outside-valid-cores-rejected [mismatch]: "RunInstances accepted CoreCount=2 on m5.large, whose ValidCores are [1], and created ['i-...'] with CpuOptions [{'CoreCount': 1, 'ThreadsPerCore': 2}]". cpuopts-partial-cpu-options-rejected [mismatch]: "accepted the partial CpuOptions {'ThreadsPerCore': 1} ... the missing CoreCount was invented rather than refused".
- hibernate-launch-on-a-type-without-hibernation-support-rejected / hibernate-launch-with-root-volume-too-small-for-ram-rejected [mismatch]: "RunInstances ACCEPTED a launch real AWS refuses with UnsupportedHibernationConfiguration". hibernate-guest-process... [mismatch]: root volume came back 'Encrypted': False. hibernate-stop-with-hibernate... [mismatch]: StateReason {'Code': 'Client.UserInitiatedShutdown'} rather than 'Client.UserInitiatedHibernate'.
- run-instances-client-token-with-changed-parameters-rejected [mismatch]: "real AWS refuses this call, but it SUCCEEDED and returned {'ReservationId': 'r-...'}".
- ec2mr-dvpc-run-instances-rejects-nonexistent-subnet [mismatch]: "the call succeeded; real AWS rejects it" (instance returned with 'NetworkInterfaces': [], 'SecurityGroups': []).
- vscan-run-instances-with-an-unknown-instance-profile-rejected-invalid-parameter-value [mismatch]: got ('DryRunOperation', 'Request would have succeeded, but DryRun flag is set.').
- vscan-public-dns-name-appears-only-once-vpc-dns-hostnames-is-enabled [mismatch]: "the public subnet must give the host a public IPv4, got ''"; timesync-* [error]: KeyError: 'PublicIpAddress'.
- vscan-encrypted-root-mapping-from-an-unencrypted-snapshot-uses-a-kms-key [mismatch]: volume reads 'Encrypted': False; vscan-unencrypted-root-mapping-round-trips-onto-the-launched-volume [mismatch]: got (8, 'gp2', False, None, 'snap-...') — Iops None.
- spotint-one-time-spot-request-refuses-interruption-behavior-stop [mismatch]: accepted and returned ['sir-...']; spotint-spot-price-below-market... [mismatch]: got {'Code': 'fulfilled', ...} (state 'active'); spotint-cancelling-a-fulfilled-spot-request... [error]: KeyError: 'InstanceLifecycle'; spotint-spot-request-fulfilment... [error]: ClientError (NotImplemented) calling RequestSpotInstances with an IAM instance profile.

## Steps to Reproduce
Replay the behavioral suite and re-run every check in trace_ids. Each drives one RunInstances/RequestSpotInstances shape against a fresh VPC/subnet/SG: arm64-vs-x86 AMI/type pairs, CpuOptions (out-of-range and partial), HibernationOptions on an unsupported type / undersized root and StopInstances(Hibernate=true), a replayed ClientToken with changed parameters, a nonexistent SubnetId, a DryRun with an unknown instance profile, a MapPublicIpOnLaunch subnet, encrypted/unencrypted root block-device mappings, and the four spot paths.
