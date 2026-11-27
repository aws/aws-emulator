Issue-Source: trace-sweep-divergence
Issue-Trace: ec2-albos-eni-lookup-id-and-filter-mistakes
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ec2-albos-eni-lookup-id-and-filter-mistakes ec2-ec2mr-pip-describe-network-interfaces-maxresults-floor-and-filter-name-precedence-rejected ec2-usg-traj-filter-name-and-page-size-validation ec2-usg-traj-malformed-vs-absent-ids-and-filters-answer-empty ec2-asgssh-traj-key-pair-import-format-and-lookup-mistakes ec2-unsigned-and-presigned-describe-requests ec2-dry-run-delete-validation-across-network-resources ec2-network-interface-permission-grant-and-revoke ec2-network-interface-permission-lifecycle ec2-get-instance-tpm-ek-pub-invalid-instance-stable-error ec2-ipam-trajwire-read-side-mistakes ec2-trajwire-deb-deb-launch-then-attach-device-mistakes ec2-network-interface-ipv4-prefix-delegation
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|DescribeNetworkInterfaces|4xx->4xx|$.Response.Errors.Error.Code
Issue-Key: ec2|DescribeNetworkInterfaces|4xx->4xx|$.Response.Errors.Error.Message
Issue-Key: ec2|DescribeKeyPairs|4xx->4xx|$.Response.Errors.Error.Code
Issue-Key: ec2|DescribeKeyPairs|4xx->4xx|$.Response.Errors.Error.Message
Issue-Key: ec2|DescribeNetworkInterfacePermissions|4xx->4xx|$
Issue-Key: ec2|DescribeNetworkInterfacePermissions|4xx->4xx|$.Response.Errors.Error.Code
Issue-Key: ec2|DescribeNetworkInterfacePermissions|4xx->4xx|$.Response.Errors.Error.Message
Issue-Key: ec2|DeleteNetworkInterfacePermission|4xx->4xx|$.Response.Errors.Error.Code
Issue-Key: ec2|DeleteNetworkInterfacePermission|4xx->4xx|$.Response.Errors.Error.Message
Issue-Key: ec2|GetInstanceTpmEkPub|4xx->4xx|$.Response.Errors.Error.Code
Issue-Key: ec2|GetInstanceTpmEkPub|4xx->4xx|$.Response.Errors.Error.Message
Issue-Key: ec2|DescribeIpamPools|4xx->4xx|$.Response.Errors.Error.Code
Issue-Key: ec2|AttachVolume|4xx->4xx|$.Response.Errors.Error.Code
Issue-Key: ec2|AttachVolume|4xx->4xx|$.Response.Errors.Error.Message
Issue-Key: ec2|CreateNetworkInterface|4xx->5xx|$.Response.Errors.Error.Code
Issue-Key: ec2|CreateNetworkInterface|4xx->5xx|$.Response.Errors.Error.Message
Issue-Key: ec2|CreateNetworkInterface|4xx->5xx|status
Issue-Commit: ba97c8568f94e8f0e1657393f851b27fdc51a8e0

# ec2 refusal classifications and messages diverge on identifier, filter and page-size validation

> _Note: this issue was identified against the code base at commit `ba97c8568f94e8f0e1657393f851b27fdc51a8e0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeNetworkInterfaces

## Description
Eight independent refusal paths in the ec2 crate return the WRONG error code, or AWS's code with re-worded message text. The shared fault: each handler mints its own error code/message string instead of the one AWS's ec2 query protocol emits for that error class, so the `<Response><Errors><Error><Code>` a client switches on is not AWS's. This is a BUNDLE OF 8 CAUSES; all eight must be closed.

Cause 1 of 8: DescribeNetworkInterfaces MaxResults above the modeled ceiling.
AWS: Smithy: DescribeNetworkInterfaces.MaxResults targets DescribeNetworkInterfacesMaxResults @range {min: 5, max: 1000}; the ec2 model declares no per-operation errors, so the refusal is recorded behavior only. Recorded response: 400 with Code 'InvalidParameterValue' and Message 'Value ( <value> ) for parameter MaxResults is invalid. Expecting a value smaller than or equal to 1000.' Fixed text: 'Value ( ', ' ) for parameter ', ' is invalid. Expecting a value smaller than or equal to ', '.'; variable parts: the rejected value from the request, the parameter name spelled as the public request member (MaxResults), and the modeled ceiling (1000).
Emulator: 400 with Code 'InvalidMaxResults' and Message 'Value ( 1001 ) for parameter maxResults is invalid. Expecting a value smaller than or equal to 1000.' — an invented code, and the parameter name lower-camel ('maxResults') instead of the public member name.
Why: a client catching InvalidParameterValue (the code every EC2 parameter refusal uses) does not catch InvalidMaxResults, so paging guards and retry classifiers fall through to a generic failure.
Fix: the DescribeNetworkInterfaces handler's MaxResults validation and the shared ec2 parameter-refusal error builder (code + parameter-name casing).
Traces: ec2-albos-eni-lookup-id-and-filter-mistakes#task#4 (cluster also covers ec2-ec2mr-pip-describe-network-interfaces-maxresults-floor-and-filter-name-precedence-rejected#task#1/#task#3, ec2-usg-traj-filter-name-and-page-size-validation#task#6, ec2-usg-traj-malformed-vs-absent-ids-and-filters-answer-empty#task#8).

Cause 2 of 8: DescribeKeyPairs with a syntactically invalid KeyPairId.
AWS: recorded behavior only (no modeled errors). AWS validates the id FORM before lookup and answers 400 Code 'InvalidParameterValue', Message "Invalid value '<id>' for keyPairId." Fixed text: "Invalid value '", "' for ", '.'; variable parts: the rejected id from the request and the request member name (keyPairId).
Emulator: 400 Code 'InvalidKeyPair.NotFound', Message "The keyPairId '<id>' does not exist" — it treats a malformed id as an absent resource.
Why: a client distinguishes 'I built a bad id' (InvalidParameterValue, non-retryable, a bug) from 'the key pair is gone' (InvalidKeyPair.NotFound, a provisioning race it may wait on); conflating them makes both unhandleable.
Fix: DescribeKeyPairs id-shape validation, ahead of the key-pair state lookup.
Traces: ec2-asgssh-traj-key-pair-import-format-and-lookup-mistakes#task#11 (cluster also covers ec2-unsigned-and-presigned-describe-requests#task#5).

Cause 3 of 8: DescribeNetworkInterfacePermissions on an absent permission id.
AWS: recorded behavior only. 400 Code 'InvalidPermissionID.NotFound', Message "The ENI permission ID '<id>' does not exist". Fixed text: "The ENI permission ID '", "' does not exist"; variable part: the permission id from the request.
Emulator: 400 Code 'InvalidNetworkInterfacePermissionID.NotFound', Message "The NetworkInterfacePermission ID '<id>' does not exist" — both the code and the resource label are the emulator's own, derived from the Smithy member name NetworkInterfacePermissionIds rather than AWS's wire code.
Why: the replay shows a poller that waits for InvalidPermissionID.NotFound never terminating (12 attempts), because the code it is told to expect never appears.
Fix: the not-found error constructor for network-interface permissions (code + message template), shared by the describe and delete paths.
Traces: ec2-dry-run-delete-validation-across-network-resources#task#25 (cluster also covers ec2-network-interface-permission-grant-and-revoke#task#7, ec2-network-interface-permission-lifecycle#task#8).

Cause 4 of 8: DeleteNetworkInterfacePermission on an absent permission id — the same wrong code/label on the delete path.
AWS: recorded behavior only. 400 Code 'InvalidPermissionID.NotFound', Message "The ENI permission ID '<id>' does not exist".
Emulator: 400 Code 'InvalidNetworkInterfacePermissionID.NotFound', Message "The NetworkInterfacePermission ID '<id>' does not exist".
Why: idempotent teardown that swallows InvalidPermissionID.NotFound re-raises instead, so a cleanup step fails on a second run.
Fix: DeleteNetworkInterfacePermission's not-found refusal (same shared constructor as cause 3 — fixing one does not automatically close the other if the delete path builds its own).
Traces: ec2-network-interface-permission-grant-and-revoke#task#8.

Cause 5 of 8: GetInstanceTpmEkPub with an instance id that is not a valid EC2 instance id.
AWS: recorded behavior only. AWS checks id syntax first: 400 Code 'InvalidInstanceID.Malformed', Message "The instance ID '<id>' is malformed". Fixed text: "The instance ID '", "' is malformed"; variable part: the id from the request.
Emulator: 400 Code 'InvalidInstanceID.NotFound', Message "The instance ID '<id>' does not exist" — it skips the syntax check and reports an absent instance.
Why: the recording pins this as a STABLE refusal across repeated calls; a client that retries NotFound (expecting eventual consistency) now retries a permanently malformed request.
Fix: GetInstanceTpmEkPub input validation — instance-id syntax check ahead of the state lookup.
Traces: ec2-get-instance-tpm-ek-pub-invalid-instance-stable-error#task#1 (cluster also covers #task#2, #task#3 of the same fixture).

Cause 6 of 8: DescribeIpamPools with an unrecognised filter name.
AWS: recorded behavior only. An unknown Filter.Name is refused 400 with Code 'InvalidFilter'.
Emulator: 400 with Code 'InvalidParameterValue'.
Why: EC2 clients branch on InvalidFilter to retry with a corrected filter set; InvalidParameterValue reads as a value-level problem on an unrelated member.
Fix: the IPAM describe handlers' filter-name validation — refuse an unknown filter name with InvalidFilter, not the generic parameter refusal.
Traces: ec2-ipam-trajwire-read-side-mistakes#task#5.

Cause 7 of 8: AttachVolume naming a volume that does not exist.
AWS: recorded behavior only. The volume id in the recording is well-formed ('vol-' + 17 hex chars) and simply absent, so AWS answers 400 Code 'InvalidVolume.NotFound', Message "The volume '<volume-id>' does not exist." Fixed text: "The volume '", "' does not exist."; variable part: the volume id from the request.
Emulator: 400 Code 'InvalidParameterValue', Message "The volume ID '<volume-id>' is malformed" — it rejects a well-formed 17-character volume id as malformed, so its id-syntax matcher only accepts the shorter legacy form.
Why: attach/retry logic keyed on InvalidVolume.NotFound (wait for the volume to appear) instead sees a permanent malformed-parameter error; and any client using modern 17-char volume ids is told its ids are invalid.
Fix: the ec2 volume-id syntax matcher (accept the 17-hex-char form) and AttachVolume's not-found refusal.
Traces: ec2-trajwire-deb-deb-launch-then-attach-device-mistakes#task#10.

Cause 8 of 8: CreateNetworkInterface with Ipv4Prefix outside the subnet CIDR answers 501 NotImplemented instead of AWS's 400 refusal.
AWS: recorded behavior only (CreateNetworkInterface models no errors). A requested IPv4 prefix outside the subnet's CIDR is refused 400 Code 'InvalidParameterValue', Message "The prefix <prefix> is outside of the subnet's cidrBlock." Fixed text: 'The prefix ', " is outside of the subnet's cidrBlock."; variable part: the prefix from the request.
Emulator: 501 with Code 'NotImplemented' and Message 'operation CreateNetworkInterface with assigned IPv4 prefixes is scaffolded but not implemented in this emulator' — an emulator-authored message, and a 5xx where AWS answers 4xx.
Why: a 501 is classified as a transient server fault, so SDK retry policies retry a request AWS rejects outright; and the prefix-delegation validation rule is never exercised.
Fix: CreateNetworkInterface's IPv4-prefix handling — validate the requested prefix against the subnet CIDR and refuse with the modeled 400 instead of the scaffold's NotImplemented.
Traces: ec2-network-interface-ipv4-prefix-delegation#task#6 (cluster also covers #task#7).

Related open issues (none of which carries these replay keys): 20261127T125651Z-ec2-9-net-new-refusal-fidelity-causes-deleteplacementgroup.md and 20261126T150319Z-ec2-3-net-new-refusal-fidelity-causes-traffic.md cover different operations.

## Evidence
Cause 1 — DescribeNetworkInterfaces, expected 400/actual 400: Code expected 'InvalidParameterValue', got 'InvalidMaxResults'; Message expected 'Value ( 1001 ) for parameter MaxResults is invalid. Expecting a value smaller than or equal to 1000.', got 'Value ( 1001 ) for parameter maxResults is invalid. Expecting a value smaller than or equal to 1000.' (5 steps in cluster).
Cause 2 — DescribeKeyPairs 400/400: Code expected 'InvalidParameterValue', got 'InvalidKeyPair.NotFound'; Message expected "Invalid value 'key-0123456789abcdef0' for keyPairId.", got "The keyPairId 'key-0123456789abcdef0' does not exist".
Cause 3 — DescribeNetworkInterfacePermissions 400/400: Code expected 'InvalidPermissionID.NotFound', got 'InvalidNetworkInterfacePermissionID.NotFound'; Message expected "The ENI permission ID 'eni-perm-000000000000000ce' does not exist", got "The NetworkInterfacePermission ID 'eni-perm-000000000000000ce' does not exist"; replay also records 'poll: error code ... not in [InvalidPermissionID.NotFound] after 12 attempt(s)'.
Cause 4 — DeleteNetworkInterfacePermission 400/400: same code/message divergence on id eni-perm-000000000000000cd.
Cause 5 — GetInstanceTpmEkPub 400/400: Code expected 'InvalidInstanceID.Malformed', got 'InvalidInstanceID.NotFound'; Message expected "The instance ID 'i-0123456789abcdef0' is malformed", got "The instance ID 'i-0123456789abcdef0' does not exist" (3 steps).
Cause 6 — DescribeIpamPools 400/400: Code expected 'InvalidFilter', got 'InvalidParameterValue'.
Cause 7 — AttachVolume 400/400: Code expected 'InvalidVolume.NotFound', got 'InvalidParameterValue'; Message expected "The volume 'vol-0b7d1c2e3f4a5b6c7' does not exist.", got "The volume ID 'vol-0b7d1c2e3f4a5b6c7' is malformed".
Cause 8 — CreateNetworkInterface expected 400, got 501: Code expected 'InvalidParameterValue', got 'NotImplemented'; Message expected "The prefix 10.99.0.0/28 is outside of the subnet's cidrBlock.", got 'operation CreateNetworkInterface with assigned IPv4 prefixes is scaffolded but not implemented in this emulator' (2 steps).
The quoted ids, values and prefixes are the recordings' request values filling the message templates; only the fixed text and the template structure are the requirement.

## Steps to Reproduce
1. Boot the emulator and replay the wire corpus (trace set simparity-wire-aws-v1).
2. Cause 1: ec2-albos-eni-lookup-id-and-filter-mistakes step task#4 — DescribeNetworkInterfaces with MaxResults=1001.
3. Cause 2: ec2-asgssh-traj-key-pair-import-format-and-lookup-mistakes step task#11 — DescribeKeyPairs with KeyPairId='key-0123456789abcdef0'.
4. Cause 3: ec2-dry-run-delete-validation-across-network-resources step task#25 — DescribeNetworkInterfacePermissions for an absent eni-perm id.
5. Cause 4: ec2-network-interface-permission-grant-and-revoke step task#8 — DeleteNetworkInterfacePermission for an absent eni-perm id.
6. Cause 5: ec2-get-instance-tpm-ek-pub-invalid-instance-stable-error step task#1 — GetInstanceTpmEkPub with an invalid instance id.
7. Cause 6: ec2-ipam-trajwire-read-side-mistakes step task#5 — DescribeIpamPools with an unknown filter name.
8. Cause 7: ec2-trajwire-deb-deb-launch-then-attach-device-mistakes step task#10 — AttachVolume naming an absent 17-char volume id.
9. Cause 8: ec2-network-interface-ipv4-prefix-delegation step task#6 — CreateNetworkInterface with an Ipv4Prefix outside the subnet CIDR.
10. Compare each response's HTTP status, Response.Errors.Error.Code and .Message with the fixture's expectation.
