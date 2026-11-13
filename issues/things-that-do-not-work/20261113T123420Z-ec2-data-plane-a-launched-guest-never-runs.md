Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2.ebsio-multi-attach-shares-one-volume-with-two-instances-while-a-plain-volume-is-refused ec2.ebsio-online-volume-grow-preserves-the-bytes-and-the-guest-sees-the-new-capacity ec2.ebsio-snapshot-restored-into-another-availability-zone-carries-the-payload ec2.ec2-ebs-volume-bytes-survive-detach-and-reattach-to-a-different-instance ec2.ec2-snapshot-restores-the-exact-bytes-written-to-the-source-volume ec2.ec2-instance-user-data-executes-and-the-guest-reports-the-nonce ec2.ec2-launch-with-a-private-ip-already-in-use-rejected ec2.ec2-attach-volume-across-availability-zones-rejected ec2.ec2-concurrent-attach-of-one-volume-to-two-instances-yields-a-single-winner ec2.ec2-elastic-ip-association-moves-which-address-serves-the-instance ec2.ec2-security-group-ingress-decides-whether-the-instance-answers ec2.ec2-stop-start-preserves-root-volume-bytes-and-reassigns-the-public-ip
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2.ebsio-multi-attach-shares-one-volume-with-two-instances-while-a-plain-volume-is-refused|run#40|AssertionError|assert len(a_fields) >= 5, (
Issue-Key: ec2.ebsio-online-volume-grow-preserves-the-bytes-and-the-guest-sees-the-new-capacity|run#21|AssertionError|assert len(base) >= 4, (
Issue-Key: ec2.ebsio-snapshot-restored-into-another-availability-zone-carries-the-payload|run#23|AssertionError|assert len(a_fields) >= 4, (
Issue-Key: ec2.ec2-ebs-volume-bytes-survive-detach-and-reattach-to-a-different-instance|run#23|AssertionError|assert len(a_fields) >= 4, (
Issue-Key: ec2.ec2-snapshot-restores-the-exact-bytes-written-to-the-source-volume|run#18|AssertionError|assert len(a_wrote) == 3 and a_wrote[1] == nonce, (
Issue-Key: ec2.ec2-instance-user-data-executes-and-the-guest-reports-the-nonce|run#14|AssertionError|assert iid_fields is not None and len(iid_fields) >= 2, (
Issue-Key: ec2.ec2-launch-with-a-private-ip-already-in-use-rejected|run#14|AssertionError|assert guest is not None and len(guest) >= 2 and guest[1] == ADDR_TAKEN, (
Issue-Key: ec2.ec2-attach-volume-across-availability-zones-rejected|run#35|AssertionError|assert fields is not None, (
Issue-Key: ec2.ec2-concurrent-attach-of-one-volume-to-two-instances-yields-a-single-winner|run#27|AssertionError|assert verdicts[winner].startswith("disk "), (
Issue-Key: ec2.ec2-elastic-ip-association-moves-which-address-serves-the-instance|_http_get#16|PublishedUrlRejected|sock = socket.create_connection(ctx.endpoint_host(host, port), timeout=timeout_s)
Issue-Key: ec2.ec2-security-group-ingress-decides-whether-the-instance-answers|_http_get#18|PublishedUrlRejected|sock = socket.create_connection(ctx.endpoint_host(host, port), timeout=timeout_s)
Issue-Key: ec2.ec2-stop-start-preserves-root-volume-bytes-and-reassigns-the-public-ip|run#22|AssertionError|assert stopped.get("PublicIpAddress") is None, (
Issue-Commit: 6ec43c73f71e6013077e783d53ce3ba5d179d34d

# ec2 data plane: a launched guest never runs its user-data payload, an attached volume never appears in the guest, the announced public IPv4 is not served, and a stopped instance keeps it (4 causes)

> _Note: this issue was identified against the code base at commit `6ec43c73f71e6013077e783d53ce3ba5d179d34d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RunInstances

## Description
Four net-new root causes in the EC2 instance data plane. All four were replayed in this sweep and none is covered by an open issue or by a scaffold-stub issue.

Cause 1 of 4: a launched guest never executes its user-data payload, so only literal text reaches the console.
AWS: RunInstances with UserData runs the script in the guest; a script that echoes a marker plus command substitutions (sha256sum of a device, the device size, a filesystem UUID, or an IMDSv2 read of instance-id / local-ipv4) emits all of those fields on the serial console. Smithy: RunInstances.UserData is modelled only as a base64 string with no behavioural trait, so this is recorded behavior only — the recordings show the full line (marker, nonce, and every computed field) in GetConsoleOutput.
Emulator: the console carries only the static parts. Every one of these checks reads back exactly the marker and the nonce and nothing else, e.g. "A's report must carry a nonce, the sha256 read back, the device size and the sha256 written, got ['SIM2REAL-WROTE', 'sim2real-nonce-<hex>']" and "the guest never reported its IMDSv2 instance id; it parsed as ['SIM2REAL-IID']". The console tail the emulator serves ends in a canned cloud-init transcript, so the shell commands and the IMDS fetches inside the user-data script produce no output at all.
Why: a client that provisions through user-data (bootstrapping, checksum verification, self-identification through IMDS) sees a booted, healthy instance whose script silently did nothing; any workflow that reads its result from the guest gets an empty value instead of a failure.
Fix: the instance-runtime component that boots a guest and feeds it UserData — it must execute the decoded script and emit its real output to the console stream, and its IMDS surface must answer the guest's reads.
Traces: ec2.ebsio-multi-attach-shares-one-volume-with-two-instances-while-a-plain-volume-is-refused, ec2.ebsio-online-volume-grow-preserves-the-bytes-and-the-guest-sees-the-new-capacity, ec2.ebsio-snapshot-restored-into-another-availability-zone-carries-the-payload, ec2.ec2-ebs-volume-bytes-survive-detach-and-reattach-to-a-different-instance, ec2.ec2-snapshot-restores-the-exact-bytes-written-to-the-source-volume, ec2.ec2-instance-user-data-executes-and-the-guest-reports-the-nonce, ec2.ec2-launch-with-a-private-ip-already-in-use-rejected.

Cause 2 of 4: an EBS volume AttachVolume reports as attached never appears as a block device inside the guest.
AWS: after AttachVolume succeeds for a volume and instance in the same Availability Zone, the guest sees a new block device at the requested device name (or its NVMe namespace) and can read and write it. Recorded behavior only (no Smithy trait describes it).
Emulator: the attach call succeeds but the guest reports no device: "the guest never saw a 1 GiB block device appear after the same-AZ attach succeeded — it reported None", and in the concurrency check every guest reports 'nodisk' ("the winner i-... attached the volume but its guest reports 'nodisk' — the attachment did not reach the machine").
Why: a client that attaches a volume and then formats or mounts it in the guest finds nothing to mount, while the control plane says the attachment is in use — the two views disagree permanently.
Fix: the AttachVolume handler's guest-side projection (services/service-com-amazonaws-ec2-amazonec2/src/ops/attach_volume.rs and the instance-runtime block-device model) — an attachment recorded in state must surface as a device in the guest.
Traces: ec2.ec2-attach-volume-across-availability-zones-rejected, ec2.ec2-concurrent-attach-of-one-volume-to-two-instances-yields-a-single-winner.

Cause 3 of 4: the public IPv4 address the emulator assigns an instance is not an address the emulator serves.
AWS: an instance with a public IPv4 address (auto-assigned, or an Elastic IP associated to it) answers on that address from outside the VPC when a security group admits the port; associating the EIP elsewhere moves which address serves it.
Emulator: the address it publishes cannot be reached at all. The harness refuses the published target: "endpoint_host was given 54.0.0.2:80 (published by the emulator or built by the check): its host is not one of the emulator's own hosts". The emulator announces a routable-looking public address (an invented 54.x literal) rather than an address it is reached on, so no data-plane leg of these checks can run.
Why: every client that takes PublicIpAddress / PublicDnsName out of DescribeInstances and connects to it is sent to an address nothing answers on; security-group and Elastic-IP behavior becomes unobservable.
Fix: the address-allocation and representation path for instance public addresses (RunInstances / AssociateAddress state plus the DescribeInstances serializer) must publish an address the emulator actually serves.
Traces: ec2.ec2-elastic-ip-association-moves-which-address-serves-the-instance, ec2.ec2-security-group-ingress-decides-whether-the-instance-answers.

Cause 4 of 4: a stopped instance keeps its auto-assigned public IPv4 address.
AWS: StopInstances releases an auto-assigned public IPv4 address; a stopped instance has no PublicIpAddress, and a later start assigns a new one. (An associated Elastic IP is the exception and stays.)
Emulator: the address survives the stop — "a stopped instance must not still hold an auto-assigned public address, got '54.0.0.2' (it had '54.0.0.2' while running)".
Why: a client that uses the absence of PublicIpAddress to tell a stopped instance from a running one, or that expects a new address after start/stop, reads stale connectivity state.
Fix: the StopInstances handler's state transition (services/service-com-amazonaws-ec2-amazonec2/src/ops/stop_instances.rs) must release the auto-assigned address, and DescribeInstances must omit it while stopped. The open issue 20261111T171305Z-ec2-7-net-new-causes-a-stopped-instance.md describes a neighbouring stopped-instance cause but does not carry this check.
Traces: ec2.ec2-stop-start-preserves-root-volume-bytes-and-reassigns-the-public-ip.

## Evidence
Cause 1 — expected the guest's report to carry nonce + sha256 + size/UUID (or the IMDSv2 instance-id / local-ipv4); got only ['SIM2REAL-WROTE', 'sim2real-nonce-<hex>'] / ['SIM2REAL-IID'] / ['SIM2REAL-LOCALIP'].
Cause 2 — expected a 1 GiB block device in the guest after a successful same-AZ AttachVolume; got None, and all four guests reported 'nodisk' in the concurrent-attach check while the control plane reported one winner.
Cause 3 — simparity.published.PublishedUrlRejected: endpoint_host was given 54.0.0.2:80 (published by the emulator or built by the check): its host is not one of the emulator's own hosts.
Cause 4 — AssertionError: a stopped instance must not still hold an auto-assigned public address, got '54.0.0.2' (it had '54.0.0.2' while running).

## Steps to Reproduce
1. RunInstances with a UserData script that echoes a marker plus a computed value (sha256sum of a device, lsblk size, or an IMDSv2 read) and poll GetConsoleOutput: only the marker and nonce appear.
2. CreateVolume in the instance's AZ, AttachVolume, then list block devices in the guest: no new device.
3. DescribeInstances the running instance and connect to its PublicIpAddress on a port its security group admits: the address is not one the emulator serves.
4. StopInstances and DescribeInstances again: PublicIpAddress is still present with the same value.
