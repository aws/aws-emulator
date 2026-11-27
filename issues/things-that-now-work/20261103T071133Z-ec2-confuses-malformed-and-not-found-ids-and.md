Issue-Source: trace-sweep-divergence
Issue-Trace: ec2-albos-eni-lookup-id-and-filter-mistakes
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ec2-albos-eni-lookup-id-and-filter-mistakes ec2-ec2mr-pip-describe-network-interfaces-maxresults-floor-and-filter-name-precedence-rejected ec2-usg-traj-filter-name-and-page-size-validation ec2-ipam-trajwire-stale-pool-reference ec2-sshp-traj-instance-connect-endpoint-lookup-and-create-mistakes ec2-tgwc-traj-tgw-id-lookup-mistakes ec2-trajwire-deb-deb-launch-then-attach-device-mistakes ec2-vpce-traj-endpoint-lookup-mistakes ec2-tgwc-traj-attachment-lookup-and-filter-mistakes
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|DescribeNetworkInterfaces|4xx->4xx|$.Response.Errors.Error.Code
Issue-Key: ec2|DescribeNetworkInterfaces|4xx->4xx|$.Response.Errors.Error.Message
Issue-Key: ec2|DescribeIpamPools|4xx->4xx|$.Response.Errors.Error.Code
Issue-Key: ec2|DescribeIpamPools|4xx->4xx|$.Response.Errors.Error.Message
Issue-Key: ec2|DescribeInstanceConnectEndpoints|4xx->4xx|$.Response.Errors.Error.Code
Issue-Key: ec2|DescribeInstanceConnectEndpoints|4xx->4xx|$.Response.Errors.Error.Message
Issue-Key: ec2|DescribeTransitGateways|4xx->4xx|$.Response.Errors.Error.Code
Issue-Key: ec2|DescribeTransitGateways|4xx->4xx|$.Response.Errors.Error.Message
Issue-Key: ec2|AttachVolume|4xx->4xx|$.Response.Errors.Error.Code
Issue-Key: ec2|AttachVolume|4xx->4xx|$.Response.Errors.Error.Message
Issue-Key: ec2|DescribeVpcEndpoints|4xx->4xx|$.Response.Errors.Error.Code
Issue-Key: ec2|DescribeVpcEndpoints|4xx->4xx|$.Response.Errors.Error.Message
Issue-Key: ec2|DescribeTransitGatewayAttachments|4xx->4xx|$.Response.Errors.Error.Message
Issue-Key: ec2|DescribeTransitGatewayVpcAttachments|4xx->4xx|$.Response.Errors.Error.Message
Issue-Commit: ce4f28f15b4841beefbad6c596b9d2158570753e

# EC2 confuses malformed and not-found ids and authors its own lookup refusal text

> _Note: this issue was identified against the code base at commit `ce4f28f15b4841beefbad6c596b9d2158570753e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeNetworkInterfaces

## Description
EC2 classifies a referenced resource id in two steps: syntax first (<Resource>.Malformed / Invalid <X> id) and existence second (<Resource>.NotFound). The emulator applies the two classes in the wrong order or on the wrong test, and in every case rewrites the sentence. The status (400) is right; the code and message are not. One id-resolution component decides all of these.

Cause 1 of 10: DescribeNetworkInterfaces with a syntactically invalid ENI id.
AWS: recorded behavior only — an id that is not a valid eni id is refused 400 Code 'InvalidNetworkInterfaceId.Malformed', Message 'Invalid id: "eni-10.80.4.37"' (fixed text 'Invalid id: ' + the request's id in double quotes).
Emulator: 400 Code 'InvalidNetworkInterfaceID.NotFound' (note also the differing ID casing in the code), Message "The networkInterface ID 'eni-10.80.4.37' does not exist".
Why: a client distinguishes 'you mistyped the id' from 'the resource is gone'; the emulator reports a lookup miss for a value that can never name a resource, so retry/backoff logic keyed on NotFound loops.
Fix: the shared id-parse/lookup helper used by services/service-com-amazonaws-ec2-amazonec2/src/ops/describe_network_interfaces.rs must refuse syntax before existence and emit the Malformed code.
Traces: ec2-albos-eni-lookup-id-and-filter-mistakes#task#1 (cluster also covers task#2/#4, ec2-ec2mr-pip-describe-network-interfaces-maxresults-floor-and-filter-name-precedence-rejected#task#1/#3, ec2-usg-traj-filter-name-and-page-size-validation#task#6).

Cause 2 of 10: DescribeIpamPools with 'ipam-pool-bogus'.
AWS: recorded behavior only — 400 Code 'InvalidIpamPoolId.NotFound', Message 'The IPAM pool (ipam-pool-bogus) does not exist.' (fixed text 'The IPAM pool (' + id + ') does not exist.').
Emulator: 400 Code 'InvalidIpamPoolId.Malformed', Message 'Invalid id: "ipam-pool-bogus"'.
Why: the inverse of cause 1 — AWS accepts this id shape and reports absence, the emulator rejects the shape, so a caller concludes its id format is wrong.
Fix: ipam pool id parsing/lookup in describe_ipam_pools.rs.
Traces: ec2-ipam-trajwire-stale-pool-reference#task#3.

Cause 3 of 10: DescribeIpamPools not-found sentence for a well-formed absent pool.
AWS: 400 (code matched) Message 'The IPAM pool (ipam-pool-0000000000000000f) does not exist.'
Emulator: "The IPAM pool ID 'ipam-pool-0000000000000000f' does not exist" — different fixed text (adds 'ID', single quotes, no trailing period).
Why: message text is part of the contract; assertions and user-facing output differ from AWS.
Fix: refusal text in describe_ipam_pools.rs.
Traces: ec2-ipam-trajwire-stale-pool-reference#task#2.

Cause 4 of 10: DescribeInstanceConnectEndpoints with 'eice-zzz'.
AWS: recorded behavior only — 400 Code 'InvalidInstanceConnectEndpointId.NotFound', Message 'The Instance Connect Endpoint eice-zzz does not exist. Add an existing Instance Connect Endpoint ID and retry your request.'
Emulator: 400 Code 'InvalidInstanceConnectEndpointId.Malformed', Message 'Invalid id: "eice-zzz"'.
Why: same inversion plus lost remediation sentence.
Fix: describe_instance_connect_endpoints.rs.
Traces: ec2-sshp-traj-instance-connect-endpoint-lookup-and-create-mistakes#task#8.

Cause 5 of 10: DescribeTransitGateways with 'tgw-xyz'.
AWS: recorded behavior only — 400 Code 'InvalidTransitGatewayID.Malformed', Message 'Invalid Transit Gateway id tgw-xyz.' (fixed text + the request's id, trailing period).
Emulator: 400 Code 'InvalidTransitGatewayID.NotFound', Message 'Transit Gateway tgw-xyz was deleted or does not exist.'
Why: as cause 1.
Fix: describe_transit_gateways.rs.
Traces: ec2-tgwc-traj-tgw-id-lookup-mistakes#task#2 (cluster also covers task#3/#4).

Cause 6 of 10: AttachVolume with a fabricated volume id.
AWS: recorded behavior only — 400 Code 'InvalidParameterValue', Message "The volume ID 'vol-0123456789abcdef0' is malformed".
Emulator: 400 Code 'InvalidVolume.NotFound', Message "The volume 'vol-0123456789abcdef0' does not exist."
Why: the caller is told to wait for a volume that will never appear instead of to fix the id it passed.
Fix: attach_volume.rs input validation.
Traces: ec2-trajwire-deb-deb-launch-then-attach-device-mistakes#task#11.

Cause 7 of 10: DescribeVpcEndpoints with 'vpce-123'.
AWS: recorded behavior only — 400 Code 'InvalidVpcEndpointId.NotFound', Message "The Vpc Endpoint Id 'vpce-123' does not exist".
Emulator: 400 Code 'InvalidVpcEndpointId.Malformed', Message 'Invalid id: "vpce-123"'.
Fix: describe_vpc_endpoints.rs.
Why: as cause 2.
Traces: ec2-vpce-traj-endpoint-lookup-mistakes#task#4 (cluster also covers task#11).

Cause 8 of 10: DescribeVpcEndpoints with a wrong-prefix id.
AWS: recorded behavior only — 400 (code matched) Message "Invalid Id: 'vpc-0a1b2c3d4e5f60718' (expecting 'vpce-...; the Id may only contain lowercase alphanumeric characters and a single dash')" — fixed text including the expected prefix and the character rule, with the request's id substituted.
Emulator: 'Invalid id: "vpc-0a1b2c3d4e5f60718"' — a generic sentence that states neither the expected prefix nor the character rule.
Why: the operator loses the only hint that the id belongs to another resource family.
Fix: refusal text in describe_vpc_endpoints.rs.
Traces: ec2-vpce-traj-endpoint-lookup-mistakes#task#3 (cluster also covers task#5/#10).

Cause 9 of 10: DescribeTransitGatewayAttachments not-found sentence.
AWS: 400 (code matched) Message 'Transit Gateway Attachment tgw-attach-0a1b2c3d4e5f60718 was deleted or does not exist.' (fixed text 'Transit Gateway Attachment ' + id + ' was deleted or does not exist.').
Emulator: "The transit-gateway-attachment ID 'tgw-attach-0a1b2c3d4e5f60718' does not exist" — emulator-authored sentence built from the internal resource-type token.
Fix: describe_transit_gateway_attachments.rs.
Why: message text diverges from AWS for every absent attachment.
Traces: ec2-tgwc-traj-attachment-lookup-and-filter-mistakes#task#1 (cluster also covers task#3).

Cause 10 of 10: DescribeTransitGatewayVpcAttachments not-found sentence.
AWS: as cause 9 — 'Transit Gateway Attachment <id> was deleted or does not exist.'
Emulator: "The transit-gateway-attachment ID '<id>' does not exist".
Fix: describe_transit_gateway_vpc_attachments.rs.
Why: as cause 9.
Traces: ec2-tgwc-traj-attachment-lookup-and-filter-mistakes#task#6.

## Evidence
Replayed wire corpus simparity-wire-aws-v1. Representative: ec2-albos-eni-lookup-id-and-filter-mistakes task#1 — expected Code 'InvalidNetworkInterfaceId.Malformed' / Message 'Invalid id: "eni-10.80.4.37"'; actual Code 'InvalidNetworkInterfaceID.NotFound' / Message "The networkInterface ID 'eni-10.80.4.37' does not exist" (status 400 both). Nine further clusters show the same Malformed/NotFound inversion or an emulator-authored sentence on DescribeIpamPools, DescribeInstanceConnectEndpoints, DescribeTransitGateways, AttachVolume, DescribeVpcEndpoints and the two transit-gateway attachment describes.

## Steps to Reproduce
1. Replay fixture ec2-albos-eni-lookup-id-and-filter-mistakes step task#1 (DescribeNetworkInterfaces with NetworkInterfaceId=eni-10.80.4.37).
2. Compare Code/Message with the recording: expected InvalidNetworkInterfaceId.Malformed / 'Invalid id: "eni-10.80.4.37"'.
3. Repeat for each fixture#step in trace_ids, in each case comparing both the error code class (Malformed vs NotFound) and the exact sentence.
