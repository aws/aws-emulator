Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2.op-contract-modify-vpc-endpoint-notfound-rejected ec2.vpc-endpoint-gateway-create-reject-bad-service ec2.ec2-iot-data-endpoint-rejects-non-full-access-endpoint-policy ec2.ipam-default-scope-deletion-rejected ec2.op-contract-delete-ipam-scope ec2.op-contract-delete-ipam-prefix-list-resolver ec2.ipam-delete-with-dependents-rejected-until-removed-in-order ec2.ltver-launch-naming-a-nonexistent-template-version-rejected ec2.ebsio-multi-attach-shares-one-volume-with-two-instances-while-a-plain-volume-is-refused ec2.ebsio-online-volume-grow-preserves-the-bytes-and-the-guest-sees-the-new-capacity ec2.ebsio-snapshot-restored-into-another-availability-zone-carries-the-payload ec2.ec2-ebs-volume-bytes-survive-detach-and-reattach-to-a-different-instance
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2.op-contract-modify-vpc-endpoint-notfound-rejected|run#14|AssertionError|assert message == "The Vpc Endpoint Id 'vpce-00000000000000000' does not exist", message
Issue-Key: ec2.vpc-endpoint-gateway-create-reject-bad-service|run#27|AssertionError|assert modify_message == expected_modify, (
Issue-Key: ec2.ec2-iot-data-endpoint-rejects-non-full-access-endpoint-policy|_error#9|AssertionError|raise AssertionError("the call succeeded; real AWS rejects it, so there is no error")
Issue-Key: ec2.ipam-default-scope-deletion-rejected|run#15|AssertionError|assert message == (
Issue-Key: ec2.op-contract-delete-ipam-scope|run#22|AssertionError|assert code == "InvalidIpamScopeId.NotFound", f"unexpected code for a mutation on the deleted scope {code}"
Issue-Key: ec2.op-contract-delete-ipam-prefix-list-resolver|run#20|AssertionError|assert second_code == "InvalidIpamPrefixListResolverId.NotFound", f"unexpected re-delete code {second_code}"
Issue-Key: ec2.ipam-delete-with-dependents-rejected-until-removed-in-order|_poll#11|Timeout|time.sleep(interval_s)
Issue-Key: ec2.ltver-launch-naming-a-nonexistent-template-version-rejected|_poll#9|Timeout|time.sleep(interval_s)
Issue-Key: ec2.ebsio-multi-attach-shares-one-volume-with-two-instances-while-a-plain-volume-is-refused|run#39|AssertionError|assert a_fields is not None, (
Issue-Key: ec2.ebsio-online-volume-grow-preserves-the-bytes-and-the-guest-sees-the-new-capacity|run#20|AssertionError|assert base is not None, (
Issue-Key: ec2.ebsio-snapshot-restored-into-another-availability-zone-carries-the-payload|run#22|AssertionError|assert a_fields is not None, (
Issue-Key: ec2.ec2-ebs-volume-bytes-survive-detach-and-reattach-to-a-different-instance|run#22|AssertionError|assert a_fields is not None, (
Issue-Commit: 1d457c6418d8d9996f08d1176214615fe9686e31

# EC2: 8 net-new causes — VPC-endpoint not-found message and missing full-access-policy refusal, IPAM scope/prefix-list-resolver refusals, two never-converging flows, and EBS guests that never report a write

> _Note: this issue was identified against the code base at commit `1d457c6418d8d9996f08d1176214615fe9686e31`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ModifyVpcEndpoint

## Description
Eight net-new root causes in the ec2 service, bundled because they share that service token. Each is a separate defect with its own failing checks.

Cause 1 of 8: the VPC-endpoint not-found message is misspelled, so it does not match AWS.
AWS: ModifyVpcEndpoint (Smithy: VpcEndpointId required, DryRun, PolicyDocument, Add/Remove*Ids, no modelled error shapes) refuses an unknown endpoint id with a client error whose message is the fixed text "The Vpc Endpoint Id '<vpce-id>' does not exist" — note the space in "Vpc Endpoint" — with the id taken from the request.
Emulator: the message is "The VpcEndpoint Id '<vpce-id>' does not exist" (no space).
Why: the message is part of the error contract for this class; assertions and operators comparing against AWS's text see a divergence, and the same wrong template is used on both the modify and the gateway-endpoint path, so it is one template, not one call.
Fix: the ModifyVpcEndpoint handler's not-found refusal message (the shared VPC-endpoint not-found message template).
Traces: ec2.op-contract-modify-vpc-endpoint-notfound-rejected, ec2.vpc-endpoint-gateway-create-reject-bad-service.

Cause 2 of 8: ModifyVpcEndpoint accepts a narrowing endpoint policy on a service that supports only the full-access policy.
AWS: for an interface endpoint whose service declares VpcEndpointPolicySupported=false (com.amazonaws.<region>.iot.data), the endpoint carries the full-access default PolicyDocument and ModifyVpcEndpoint with any PolicyDocument that is not full access is refused with code `InvalidParameter` and the message "Service com.amazonaws.<region>.iot.data only supports the full-access endpoint policy." — fixed text with the service name taken from the request/endpoint; the stored document is unchanged.
Emulator: the call SUCCEEDS (no error raised at all).
Why: a client can silently install a policy the real service would reject, so an estate that passes here fails on AWS, and the emulator's stored policy diverges from what AWS would hold.
Fix: ModifyVpcEndpoint's validation — gate PolicyDocument on the target service's VpcEndpointPolicySupported flag in the endpoint-service catalogue.
Traces: ec2.ec2-iot-data-endpoint-rejects-non-full-access-endpoint-policy.

Cause 3 of 8: DeleteIpamScope's default-scope protection message is emulator-authored prose.
AWS: deleting an IPAM's PrivateDefaultScopeId or PublicDefaultScopeId is refused with code `IncorrectState` and the message "The specified action is not valid. The scope <ipam-scope-id> is a default scope." — fixed text plus the scope id from the request; both default scopes stay create-complete.
Emulator: code is right, message is "The IPAM scope (<ipam-scope-id>) is a default scope and cannot be deleted."
Why: the refusal text is the contract for this error class; clients surfacing or matching it diverge.
Fix: the DeleteIpamScope handler's default-scope refusal message.
Traces: ec2.ipam-default-scope-deletion-rejected.

Cause 4 of 8: a mutation naming an already-deleted IPAM scope answers NotImplemented instead of a not-found refusal.
AWS: once a scope reaches delete-complete, a mutation naming it is refused with code `InvalidIpamScopeId.NotFound` (HTTP 400), the id taken from the request.
Emulator: code `NotImplemented` (HTTP 501).
Why: a client cannot distinguish "already deleted" from a server fault, so idempotent teardown loops retry a permanent failure.
Fix: the DeleteIpamScope/IPAM-scope mutation handlers and the IPAM scope state lookup (tombstoned scopes must resolve to a not-found refusal).
Traces: ec2.op-contract-delete-ipam-scope.

Cause 5 of 8: re-deleting an IPAM prefix-list resolver is refused with the wrong code.
AWS: a second DeleteIpamPrefixListResolver on the same resolver is refused with code `InvalidIpamPrefixListResolverId.NotFound`.
Emulator: code `IncorrectState`.
Why: callers that treat NotFound as success (idempotent delete) and IncorrectState as retryable loop forever on an already-deleted resolver.
Fix: the DeleteIpamPrefixListResolver handler's refusal classification for an id that no longer exists.
Traces: ec2.op-contract-delete-ipam-prefix-list-resolver.

Cause 6 of 8: the IPAM dependent-deletion ordering flow never converges.
AWS: recorded behavior only — deleting an IPAM with dependents is refused until pool CIDRs, pools and scopes are removed in order, each delete reaching its terminal state (delete-complete) so the next step can proceed.
Emulator: the check never completes — it is killed by the 60 s check timeout, i.e. some IPAM delete step never reaches its terminal state (the poll loop never satisfies its predicate).
Why: a client tearing an IPAM tree down waits forever; no error is returned to react to.
Fix: the IPAM delete handlers and IPAM state machine (delete-in-progress → delete-complete transitions and the dependent-ordering refusal).
Traces: ec2.ipam-delete-with-dependents-rejected-until-removed-in-order.

Cause 7 of 8: the launch-template-version launch refusal flow never converges.
AWS: recorded behavior only — RunInstances naming a nonexistent launch-template version is refused promptly and the surrounding template/instance state settles, so the check finishes.
Emulator: the check is killed by the 60 s check timeout; some polled state in the flow never settles.
Why: a client blocks indefinitely instead of receiving the refusal.
Fix: the RunInstances launch-template-version resolution path and whatever polled resource state fails to converge in that flow.
Traces: ec2.ltver-launch-naming-a-nonexistent-template-version-rejected.

Cause 8 of 8: a launched guest never reports the write its user data performs, so every EBS byte-level flow fails.
AWS: recorded behavior only — a t3.micro guest booted from the Amazon Linux AMI runs its user data and emits the write report (the probe's field line) to the serial console, so the console contains the guest's own report after the write.
Emulator: the console ends after cloud-init's ci-info banner ("Cloud-init v. 23.4 running 'init' ...", ci-info lines, launch-time) and the guest's write report is absent — the probe reads None. All four EBS flows that depend on bytes written inside the guest fail on that one missing step: Multi-Attach sharing one io2 volume with two instances, an online volume grow preserving bytes, a snapshot restored into another Availability Zone carrying the payload, and volume bytes surviving detach/reattach to a different instance.
Why: EBS durability, Multi-Attach and snapshot semantics cannot be observed at all — a client verifying that its data survived a detach, a grow, or a cross-AZ restore gets no evidence, and the emulator cannot show whether the bytes moved.
Fix: the EC2 data-plane guest execution path (user-data execution and serial-console capture for a launched instance) and the EBS volume data path it writes through. Related open issues cover other checks of the same guest gap (20261113T123420Z-ec2-data-plane-a-launched-guest-never-runs.md and 20261107T124656Z-autoscaling-ec2-s3-an-asg-launched-instance-never-runs-its-userdata.md); they do not carry these four check ids.
Traces: ec2.ebsio-multi-attach-shares-one-volume-with-two-instances-while-a-plain-volume-is-refused, ec2.ebsio-online-volume-grow-preserves-the-bytes-and-the-guest-sees-the-new-capacity, ec2.ebsio-snapshot-restored-into-another-availability-zone-carries-the-payload, ec2.ec2-ebs-volume-bytes-survive-detach-and-reattach-to-a-different-instance.

## Evidence
Replayed behavioral suite, service heading `### ec2`:
- ec2.op-contract-modify-vpc-endpoint-notfound-rejected — AssertionError: expected "The Vpc Endpoint Id 'vpce-...' does not exist", got "The VpcEndpoint Id 'vpce-...' does not exist"
- ec2.vpc-endpoint-gateway-create-reject-bad-service — AssertionError: expected message "The Vpc Endpoint Id 'vpce-...' does not exist", got "The VpcEndpoint Id 'vpce-...' does not exist"
- ec2.ec2-iot-data-endpoint-rejects-non-full-access-endpoint-policy — AssertionError: the call succeeded; real AWS rejects it, so there is no error. Check asserts ModifyVpcEndpoint(PolicyDocument=<iot:Publish-only document>) raises ('InvalidParameter', 'Service com.amazonaws.<region>.iot.data only supports the full-access endpoint policy.')
- ec2.ipam-default-scope-deletion-rejected — AssertionError: unexpected default-scope-protection message for ipam-scope-...: 'The IPAM scope (ipam-scope-...) is a default scope and cannot be deleted.'; check asserts code IncorrectState and message "The specified action is not valid. The scope <ipam-scope-id> is a default scope."
- ec2.op-contract-delete-ipam-scope — AssertionError: unexpected code for a mutation on the deleted scope NotImplemented (expected InvalidIpamScopeId.NotFound)
- ec2.op-contract-delete-ipam-prefix-list-resolver — AssertionError: unexpected re-delete code IncorrectState (expected InvalidIpamPrefixListResolverId.NotFound)
- ec2.ipam-delete-with-dependents-rejected-until-removed-in-order — Error: Check timed out after 60.0s
- ec2.ltver-launch-naming-a-nonexistent-template-version-rejected — Error: Check timed out after 60.0s
- ec2.ebsio-multi-attach-... / ec2.ebsio-online-volume-grow-... / ec2.ebsio-snapshot-restored-into-another-availability-zone-... / ec2.ec2-ebs-volume-bytes-survive-detach-and-reattach-... — AssertionError: "guest A never reported a write — it said None"; the captured console ends with cloud-init ci-info lines (instance-id, instance-type t3.micro, availability-zone, private-ip, launch-time) and contains no guest write report.

Smithy (services/service-com-amazonaws-ec2-amazonec2/smithy-model/com-amazonaws-ec2-amazonec2.json): ModifyVpcEndpoint input = {DryRun, VpcEndpointId (required), ResetPolicy, PolicyDocument, Add/RemoveRouteTableIds, Add/RemoveSubnetIds, Add/RemoveSecurityGroupIds, IpAddressType, DnsOptions, PrivateDnsEnabled, SubnetConfigurations} → {Return}; DeleteIpamScope input = {DryRun, IpamScopeId (required)} → {IpamScope}. No modelled error shapes (ec2Query), so code + message are the contract.

## Steps to Reproduce
Re-replay the listed check ids. Per cause: (1) ModifyVpcEndpoint(VpcEndpointId='vpce-00000000000000000', PolicyDocument='{}') and the gateway-endpoint path; compare the message word for word. (2) Create an interface endpoint for com.amazonaws.<region>.iot.data, confirm its PolicyDocument is the full-access default, then ModifyVpcEndpoint with a policy allowing only iot:Publish — expect ('InvalidParameter', 'Service com.amazonaws.<region>.iot.data only supports the full-access endpoint policy.') and an unchanged stored document. (3) CreateIpam, then DeleteIpamScope on PrivateDefaultScopeId and PublicDefaultScopeId. (4) CreateIpamScope → DeleteIpamScope → wait for delete-complete → mutate the deleted scope. (5) CreateIpamPrefixListResolver → delete twice. (6) Build an IPAM with pools/CIDRs and delete out of order, then in order. (7) RunInstances with LaunchTemplate{Version: <nonexistent>}. (8) RunInstances with user data that writes to an attached EBS volume and echoes a field line, then read GetConsoleOutput: the guest's report must appear.
