Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__ec2__ssm.obs-ec2-2-ebs-write-telemetry-published-only-while-attached
Issue-Key: cloudwatch__ec2__ssm.obs-ec2-2-ebs-write-telemetry-published-only-while-attached|run#3|AssertionError|assert _marker(out, "EBS-DD-RC") == "0", f"dd to {dev} failed in the guest: stdout={out!r}"
Issue-Commit: f214ea914d97c777aba1c48734a44751ff321c53

# cloudwatch__ec2__ssm: an in-guest write to an attached EBS volume now FAILS — dd exits 1 with no output and the readback is the empty-string digest

> _Note: this issue was identified against the code base at commit `f214ea914d97c777aba1c48734a44751ff321c53`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AttachVolume

## Description
Cause 1 of 1: a program run through SSM Run Command on an SSM-managed node that writes to the block device of an EBS volume the emulator reports as attached now exits NON-ZERO and moves no bytes, and a subsequent read of the device hashes to the empty input, so every AWS/EBS per-volume telemetry assertion behind it is unreachable (failing check: cloudwatch__ec2__ssm.obs-ec2-2-ebs-write-telemetry-published-only-while-attached).

AWS: recorded behavior only — no Smithy member governs in-guest device I/O. ec2 AttachVolume / DescribeVolumes report the attachment as in-use/attached, and the emulator already does that and already resolves the volume's NVMe serial (the recording's EBS-DEV and EBS-SERIAL markers are correct: the device resolved to /dev/nvme1n1 and the serial to the volume id with hyphens removed). On a real instance the volume is presented in the guest at /dev/disk/by-id/nvme-Amazon_Elastic_Block_Store_<volume id with hyphens removed>; a write to that device (here `dd if=/dev/urandom of=<device> bs=1M count=<N> oflag=direct conv=fsync`) stores the bytes, EXITS 0, and reports a transfer of bs*count bytes; reading the same range back returns those bytes, so a digest of the readback is the digest of what was written, never the digest of empty input. Those transferred bytes are what CloudWatch then publishes under AWS/EBS on the dimension set [VolumeId] (VolumeWriteBytes/VolumeWriteOps, and the read counterparts), Period=60, and publication stops once the volume is detached. The requirement is that general rule; the byte count (64 MiB), the volume id and the instance id in the recording are per-run values filling it.

Emulator: the dd against the attached volume's device node now exits 1 (marker EBS-DD-RC 1) with an empty transfer report (EBS-DD empty) and an empty payload (EBS-PAYLOAD empty), and the read-back digest is e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855 — the SHA-256 of zero bytes — confirming nothing was stored. The device and serial resolution ahead of it succeed, so the failure is in the write path itself, not in device presentation. This is a changed signature for this check: open issue 20261124T232759Z-cloudwatch-ec2-ssm-an-in-guest-write-to.md reports the earlier behavior for the same four EBS-telemetry checks (dd exiting 0 while transferring nothing, replay key `assert written == _WRITE_BYTES`); the write is now REFUSED by the guest (non-zero exit), which that issue's wording and replay key do not cover, so it is filed here and that issue should be fixed together with it.

Why: a client cannot do EBS I/O from inside an instance at all. A workload that writes to an attached data volume gets a hard failure from a device the control plane reports as attached and healthy, so provisioning scripts (mkfs, dd seeding, database initialisation) abort; and because no bytes ever move, every AWS/EBS observability path built on them is dead — per-volume throughput/IOPS dashboards, alarms on VolumeWriteBytes, and the "telemetry only while attached" semantics the check grades.

Fix: the ec2 volume-attachment data plane together with the guest command-execution path behind SSM Run Command — a write issued against an attached volume's device node must transfer the bytes into that volume's backing store and return the program's real exit status and transfer report, a read of the same range must return those bytes, and the transferred bytes must feed the AWS/EBS metric publisher CloudWatch serves on the [VolumeId] dimension (and stop feeding it after DetachVolume).

Traces: cloudwatch__ec2__ssm.obs-ec2-2-ebs-write-telemetry-published-only-while-attached (first in-guest I/O gate, before any CloudWatch assertion is reached).

## Evidence
Failing check cloudwatch__ec2__ssm.obs-ec2-2-ebs-write-telemetry-published-only-while-attached:

AssertionError: dd to /dev/disk/by-id/nvme-Amazon_Elastic_Block_Store_vol000000000000000ca failed in the guest: stdout='EBS-DEV /dev/nvme1n1\nEBS-SERIAL vol000000000000000ca\nEBS-T0 1795687644\nEBS-DD-RC 1\nEBS-DD \nEBS-T1 1795687652\nEBS-READBACK e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855\nEBS-PAYLOAD \n'

Expected: EBS-DD-RC 0, EBS-DD reporting 67108864 bytes (bs=1M count=64) transferred, EBS-READBACK equal to the digest of the written payload, and AWS/EBS VolumeWriteBytes on Dimensions=[{VolumeId:<vol>}], Period=60, covering the write window.
Actual: EBS-DD-RC 1 (the write failed), no transfer report, EBS-PAYLOAD empty, and EBS-READBACK = e3b0c44298... (SHA-256 of the empty input), i.e. nothing was stored. Device resolution (EBS-DEV) and serial resolution (EBS-SERIAL) are correct.

The volume id, instance id, timestamps and the 64 MiB size are per-run values; the requirement is that the write succeed, report the requested byte count, read back identically, and be counted into AWS/EBS.

## Steps to Reproduce
1. Clone the repo and boot the emulator built from it (the behavioral suite does this).
2. Create a VPC/subnet and an instance profile whose role has arn:aws:iam::aws:policy/AmazonSSMManagedInstanceCore; launch a t3.micro from /aws/service/ami-amazon-linux-latest/al2023-ami-kernel-default-x86_64 and wait for the node to be an Online SSM managed node.
3. ec2 CreateVolume(Size=1, VolumeType='gp3') in the instance's zone, then AttachVolume(VolumeId=<vol>, InstanceId=<iid>, Device='/dev/sdf'); wait until the attachment reads in-use/attached.
4. ssm SendCommand(DocumentName='AWS-RunShellScript') on the node with:
   dev=/dev/disk/by-id/nvme-Amazon_Elastic_Block_Store_$(echo <vol> | tr -d -)
   echo "EBS-DEV $(readlink -f $dev)"; echo "EBS-SERIAL $(lsblk -dno SERIAL $(readlink -f $dev))"
   dd if=/dev/urandom of=$dev bs=1M count=64 oflag=direct conv=fsync 2>/tmp/w.err; echo "EBS-DD-RC $?"
   grep -o '^[0-9]* bytes' /tmp/w.err
   dd if=$dev bs=1M count=64 iflag=direct 2>/dev/null | sha256sum
5. GetCommandInvocation: the dd exit code comes back 1, the transfer report is empty, and the readback digest is e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855 (empty input). Expected exit 0, "67108864 bytes", and a digest of the written payload.
6. cloudwatch GetMetricStatistics(Namespace='AWS/EBS', MetricName='VolumeWriteBytes', Dimensions=[{Name:'VolumeId',Value:<vol>}], Period=60, Statistics=['Sum']) over the write window — no datapoint covers the write; then DetachVolume and confirm publication stops.
