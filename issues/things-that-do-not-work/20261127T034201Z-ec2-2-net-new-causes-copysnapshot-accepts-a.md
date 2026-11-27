Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2.copy-snapshot-cross-account-without-permission-blocked ec2.op-contract-modify-ipam-resource-cidr
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2.copy-snapshot-cross-account-without-permission-blocked|_error_detail#13|AssertionError|raise AssertionError(f"expected a ClientError, but the call succeeded and returned {result!r}")
Issue-Key: ec2.op-contract-modify-ipam-resource-cidr|run#20|AssertionError|assert message == (
Issue-Commit: 0ebf0829cad241d824be0f94d62bd70dc38785d4

# ec2: 2 net-new causes — CopySnapshot accepts a SourceSnapshotId the account neither owns nor is granted, and ModifyIpamResourceCidr's InvalidResourceCidr.NotFound message is not AWS's wording

> _Note: this issue was identified against the code base at commit `0ebf0829cad241d824be0f94d62bd70dc38785d4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CopySnapshot

## Description
Two net-new, independent root causes in the EC2 emulator, both grounded in failing behavioral checks. Neither is covered by an existing open issue: 20261105T021000Z-ec2-5-net-new-ebs-snapshot-volume-root.md covers CopySnapshot accepting a MALFORMED PresignedUrl, and 20261109T171600Z-ec2-2-net-new-causes-instance-state-change.md covers CopySnapshot from a DELETED source being refused synchronously — Cause 1 below is a source snapshot that never existed in, and was never shared with, the calling account, which is a different admission path.

Cause 1 of 2: CopySnapshot admits a well-formed SourceSnapshotId the caller has no access to, and mints a new snapshot.
AWS: recorded behavior only — the EC2 Smithy model declares no modelled errors for CopySnapshot (errors: [], ec2Query common error envelope), and SourceSnapshotId is only `required` with no pattern, so the rule is not in the model. The recording shows AWS refusing CopySnapshot for a syntactically valid snapshot id that the calling account neither owns nor holds create-volume-permission on, with HTTP 400, Error.Code `InvalidParameterValue` and Error.Message exactly `The specified snapshotId is invalid` — note this refusal deliberately differs from the `InvalidSnapshot.NotFound` that DescribeSnapshots/DeleteSnapshot use, so that existence does not leak across accounts. The message is entirely fixed text: it names no id and interpolates nothing from the request. AWS also creates no snapshot as a side effect, so DescribeSnapshots(OwnerIds=['self']) is unchanged across the refused call.
Emulator: the call SUCCEEDS with HTTP 200 and a CopySnapshotResult carrying a freshly minted SnapshotId (the replay observed `{'SnapshotId': 'snap-...', 'HTTPStatusCode': 200}` for SourceSnapshotId `snap-0123456789abcdef0`, an id the account's own snapshot set provably does not contain). No error is raised at all, and a snapshot that AWS would never have created now exists in the account.
Why: a client that relies on CopySnapshot to enforce source-snapshot authorization gets a silent success where AWS denies. Cross-account existence/authorization probing becomes possible — any guessable snapshot id can be "copied" — and the account accumulates snapshots (and their cost and their cleanup obligations) that the real service would never have created. Negative-path tests and IaC error handling that catch InvalidParameterValue never fire.
Fix: the CopySnapshot handler's input validation and its state lookup — resolve SourceSnapshotId against the caller's owned snapshots plus snapshots explicitly shared with the caller BEFORE any snapshot is created, and refuse unresolvable ids with InvalidParameterValue / `The specified snapshotId is invalid`, leaving snapshot state untouched.
Traces: ec2.copy-snapshot-cross-account-without-permission-blocked.

Cause 2 of 2: ModifyIpamResourceCidr returns the right error code but not AWS's error message for a resource CIDR IPAM has not discovered.
AWS: recorded behavior only — the model (ModifyIpamResourceCidr: ResourceId, ResourceCidr, ResourceRegion, CurrentIpamScopeId, Monitored all required; errors: []) does not state the refusal, so the recording is the requirement. AWS refuses a ResourceId/ResourceCidr pair that IPAM has never discovered in the named scope with Error.Code `InvalidResourceCidr.NotFound` and Error.Message of the form `The specified resource ID <ResourceId> and CIDR <ResourceCidr> does not exist.` — the fixed text is `The specified resource ID `, ` and CIDR `, and ` does not exist.`; the two variable parts are the request's ResourceId and ResourceCidr values verbatim (in the recorded run those were `vpc-00000000000000000` and `10.250.0.0/24`, which fill the template and are not part of it).
Emulator: the code is correct (`InvalidResourceCidr.NotFound`) but the message is the fixed, request-independent string `The requested resource CIDR could not be found.` — it interpolates neither the ResourceId nor the ResourceCidr and shares no wording with AWS's text.
Why: a client that surfaces, logs, parses or asserts on Error.Message sees a message that does not name which resource id and CIDR were not found, so operators lose the only diagnostic distinguishing "wrong scope" from "wrong CIDR" from "not yet discovered". Tests and tooling that match AWS's message fail against the emulator even though the code matches.
Fix: the ModifyIpamResourceCidr handler's refusal path — build the InvalidResourceCidr.NotFound message from the request's ResourceId and ResourceCidr using AWS's template instead of emitting a constant string.
Traces: ec2.op-contract-modify-ipam-resource-cidr.

## Evidence
Cause 1 — check ec2.copy-snapshot-cross-account-without-permission-blocked:
Request: CopySnapshot(SourceRegion=<region>, SourceSnapshotId='snap-0123456789abcdef0', Description=...), after asserting that id is NOT in DescribeSnapshots(OwnerIds=['self']).
Expected (AWS): HTTP 400, Error.Code 'InvalidParameterValue', Error.Message 'The specified snapshotId is invalid' (fixed text, no interpolation); and DescribeSnapshots(OwnerIds=['self']) unchanged — no snapshot created.
Actual (emulator): HTTP 200 with a new snapshot id. Replay output:
  AssertionError: expected a ClientError, but the call succeeded and returned {'SnapshotId': 'snap-000000000000000ac', 'ResponseMetadata': {'RequestId': '...', 'HTTPStatusCode': 200, ...}}
(the particular generated snapshot id and request id are per-run values, not requirements; the requirement is that the call is refused and no snapshot is minted).

Cause 2 — check ec2.op-contract-modify-ipam-resource-cidr:
Request: ModifyIpamResourceCidr(ResourceId='vpc-00000000000000000', ResourceCidr='10.250.0.0/24', ResourceRegion=<region>, CurrentIpamScopeId=<private default scope of a fresh IPAM>, Monitored=False).
Expected (AWS): Error.Code 'InvalidResourceCidr.NotFound' and Error.Message 'The specified resource ID vpc-00000000000000000 and CIDR 10.250.0.0/24 does not exist.' — i.e. template 'The specified resource ID <ResourceId> and CIDR <ResourceCidr> does not exist.' filled from the request.
Actual (emulator): code matched; message was 'The requested resource CIDR could not be found.' Replay output:
  check_ec2_op_contract_modify_ipam_resource_cidr.py, line 104, in run
    assert message == (
  AssertionError: The requested resource CIDR could not be found.

## Steps to Reproduce
Cause 1 (ec2.copy-snapshot-cross-account-without-permission-blocked):
1. ec2 create_volume(AvailabilityZone=<available az>, Size=1, VolumeType='gp3'); wait for state 'available'.
2. ec2 create_snapshot(VolumeId=<volume>); wait for state 'completed'.
3. Record own = {s['SnapshotId'] for s in describe_snapshots(OwnerIds=['self'])['Snapshots']} and confirm 'snap-0123456789abcdef0' is not in it.
4. Call ec2 copy_snapshot(SourceRegion=<region>, SourceSnapshotId='snap-0123456789abcdef0', Description='...').
   Expect ClientError InvalidParameterValue / 'The specified snapshotId is invalid'. The emulator returns HTTP 200 with a new SnapshotId.
5. Re-read describe_snapshots(OwnerIds=['self']) — on AWS the set is unchanged; on the emulator a new snapshot appeared.
6. Control: copy_snapshot of the snapshot from step 2 must still succeed.

Cause 2 (ec2.op-contract-modify-ipam-resource-cidr):
1. ec2 create_ipam(OperatingRegions=[{'RegionName': <region>}]); poll describe_ipams until State == 'create-complete'; take PrivateDefaultScopeId.
2. Call ec2 modify_ipam_resource_cidr(ResourceId='vpc-00000000000000000', ResourceCidr='10.250.0.0/24', ResourceRegion=<region>, CurrentIpamScopeId=<PrivateDefaultScopeId>, Monitored=False).
3. Inspect the ClientError: Error.Code is 'InvalidResourceCidr.NotFound' (correct), but Error.Message is 'The requested resource CIDR could not be found.' instead of 'The specified resource ID vpc-00000000000000000 and CIDR 10.250.0.0/24 does not exist.'
