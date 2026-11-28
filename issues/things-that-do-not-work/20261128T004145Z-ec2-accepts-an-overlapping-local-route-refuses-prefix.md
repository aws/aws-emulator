Issue-Source: trace-sweep-divergence
Issue-Trace: ec2-create-route-more-specific-than-local-target-restriction
Issue-Step: task#7
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ec2-create-route-more-specific-than-local-target-restriction ec2-network-interface-ipv4-prefix-delegation ec2-describe-instance-attribute-ena-support ec2-instance-monitoring-toggle ec2-instance-reboot-and-status-report ec2-sshp-traj-keyless-public-instance-eic-push-and-eip-swap ec2-ecc-traj-os-classification-platform-vs-platform-details ec2-sshp-traj-ssh-ingress-rule-authoring-mistakes ec2-verified-access-delete-lifecycle
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|CreateRoute|4xx->2xx|$.Response
Issue-Key: ec2|CreateRoute|4xx->2xx|status
Issue-Key: ec2|CreateNetworkInterface|2xx->5xx|$.CreateNetworkInterfaceResponse
Issue-Key: ec2|CreateNetworkInterface|2xx->5xx|status
Issue-Key: ec2|DescribeInstances|2xx->2xx|$.DescribeInstancesResponse.reservationSet.item.instancesSet.item.dnsName
Issue-Key: ec2|DescribeInstances|2xx->2xx|$.DescribeInstancesResponse.reservationSet.item.instancesSet.item.networkInterfaceSet.item.association.publicDnsName
Issue-Key: ec2|DescribeInstances|2xx->2xx|$.DescribeInstancesResponse.reservationSet
Issue-Key: ec2|AuthorizeSecurityGroupIngress|4xx->4xx|$.Response.Errors.Error.Message
Issue-Key: ec2|DeleteVerifiedAccessInstance|2xx->2xx|$.DeleteVerifiedAccessInstanceResponse.verifiedAccessInstance.lastUpdatedTime
Issue-Related: 20261120T081949Z-ec2-dns-projections-announce-invented-hostnames-and-omit.md
Issue-Related: 20261121T084716Z-ec2-instance-public-dns-projection-is-synthesized-from.md
Issue-Commit: 244eae870d957920c579d40eef12d136da6f5bab

# ec2 accepts an overlapping local route, refuses prefix-delegation ENIs, and misprojects instance reads

> _Note: this issue was identified against the code base at commit `244eae870d957920c579d40eef12d136da6f5bab`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateRoute

## Description
Six independent ec2 divergences, each with its own fixture and step, none of them owned by an open issue and none blocked by an unimplemented operation.

Cause 1 of 6: CreateRoute accepts a destination CIDR that is equal to or more specific than one of the VPC's own CIDR blocks when the target is not an interface or an instance.
AWS: Smithy CreateRoute declares no operation-specific errors, so the refusal arrives in the EC2 query error envelope ($.Response.Errors.Error) with Code InvalidParameterValue and the fixed message "The destination CIDR block <destinationCidrBlock> is equal to or more specific than one of this VPC's CIDR blocks. This route can target only an interface or an instance." — the only variable part is the DestinationCidrBlock taken from the request; everything else is fixed text. Status 400.
Emulator: answers 200 with no $.Response error document at all, so the route is created.
Why: a client that relies on AWS rejecting a route that shadows the VPC-local route silently installs it, and subsequent traffic within the VPC is attributed to the wrong target; the client never sees the error it is written to handle.
Fix: CreateRoute's validation (compare the requested destination against the VPC's CIDR associations and the target kind before mutating the route table).
Traces: ec2-create-route-more-specific-than-local-target-restriction#task#7.

Cause 2 of 6: CreateNetworkInterface answers 501 when the request carries an IPv4 prefix (prefix delegation).
AWS: Smithy CreateNetworkInterfaceRequest models Ipv4Prefixes (Ipv4PrefixList, xmlName Ipv4Prefix) and Ipv4PrefixCount, and CreateNetworkInterfaceResult.NetworkInterface carries the assigned prefixes back (ipv4PrefixSet). AWS answers 200 with the interface in status 'pending' and ipv4PrefixSet holding the delegated prefix.
Emulator: answers 501 and no CreateNetworkInterfaceResponse body, so the operation cannot be used with prefix delegation even though it is implemented for the plain case.
Why: every prefix-delegation workflow (container/pod IP assignment) fails outright, and a 501 is not a status any AWS SDK retries or maps to a modeled error.
Fix: CreateNetworkInterface's handler — accept Ipv4Prefixes/Ipv4PrefixCount, record the prefixes in ENI state and project them as ipv4PrefixSet.
Traces: ec2-network-interface-ipv4-prefix-delegation#task#3 (the cluster also covers that fixture's task#4).

Cause 3 of 6: DescribeInstances advertises a public DNS name on a host the emulator does not answer on.
AWS: recorded behavior only for the exact name; AWS populates instancesSet.item.dnsName, networkInterfaceSet.item.publicDnsName, networkInterfaceSet.item.association.publicDnsName, privateIpAddressesSet.item.association.publicDnsName and publicIpDnsNameOptions.publicIpv4DnsName with a public DNS name that resolves to the instance's public address.
Emulator: emits a name in the AWS ec2-<dashed-public-ip>.<region>.compute.amazonaws.com form, which the replay rejects as host_not_own — the emulator announced an address other than the one it is reached on, so the name resolves nowhere the emulator serves.
Why: a client that connects to an instance using the DNS name the API handed it cannot reach the instance at all, even though the ip-address members are usable.
Fix: the DescribeInstances serializer/projection for the public-DNS members (publish a name on the host the emulator is reached on).
Traces: ec2-describe-instance-attribute-ena-support#task#4 (one cluster, 8 affected steps, also covering ec2-instance-monitoring-toggle#task#3 and #task#5, ec2-instance-reboot-and-status-report#task#2/#task#3/#task#4 and ec2-sshp-traj-keyless-public-instance-eic-push-and-eip-swap#task#19/#task#24).

Cause 4 of 6: a DescribeInstances filter that matches nothing in AWS matches an instance in the emulator.
AWS: recorded behavior only — the recording for this OS-classification step answers 200 with an empty reservationSet, i.e. no reservation matches the requested filter.
Emulator: answers 200 with a populated reservationSet (one reservation and its instance), so the filter is applied too loosely (the step distinguishes the platform member from platformDetails).
Why: a client that uses the filter to select only the matching instances acts on instances AWS would not have returned — the opposite of the safe failure — and any count or emptiness check is wrong.
Fix: DescribeInstances' filter evaluation (the platform / platform-details predicate) in the operation's handler.
Traces: ec2-ecc-traj-os-classification-platform-vs-platform-details#task#8.

Cause 5 of 6: AuthorizeSecurityGroupIngress re-words the out-of-range port refusal.
AWS: Smithy AuthorizeSecurityGroupIngressRequest.FromPort/ToPort are plain Integers with no modeled range, and AWS refuses an out-of-range TCP/UDP port in the EC2 query error envelope with status 400 and the fixed message "TCP/UDP (to) port (<port>) out of range" — fixed text "TCP/UDP (", ") port (", ") out of range"; the variable parts are which bound was violated (to/from) and the port value taken from the request. The status and error code already match.
Emulator: answers 400 with "Invalid value '<port>' for toPort. Value must be between -1 and 65535." — a generic range-validator message that shares no wording with AWS's.
Why: a client that matches on AWS's message text (or surfaces it to an operator) sees an emulator-authored sentence, and the "(to)"/"(from)" marker that tells it which bound failed is gone.
Fix: the AuthorizeSecurityGroupIngress validation's message template for the port-range refusal.
Traces: ec2-sshp-traj-ssh-ingress-rule-authoring-mistakes#task#6 (the cluster also covers that fixture's task#7).

Cause 6 of 6: DeleteVerifiedAccessInstance re-stamps lastUpdatedTime instead of returning the stored value.
AWS: Smithy DeleteVerifiedAccessInstanceResult.VerifiedAccessInstance (xmlName verifiedAccessInstance) returns the instance as it stood; recorded behavior only for the value — the recording returns the lastUpdatedTime the resource already carried from its last modification, so deleting it does not advance the member.
Emulator: returns a freshly read clock value instead of the stored one (the replayed value is later within the same second than the one recorded for the same resource in the same run), i.e. the delete path writes lastUpdatedTime = now before projecting the response.
Why: a client that uses lastUpdatedTime to detect whether a resource changed sees a modification that never happened, and a last-writer-wins comparison against a previously read value is wrong.
Fix: the DeleteVerifiedAccessInstance handler/state transition (project the stored lastUpdatedTime rather than re-stamping it).
Traces: ec2-verified-access-delete-lifecycle#task#13.

## Evidence
Cause 1 — ec2-create-route-more-specific-than-local-target-restriction#task#7, CreateRoute: expected status 400 with $.Response = {'Errors': {'Error': {'Code': 'InvalidParameterValue', 'Message': "The destination CIDR block 10.141.1.0/24 is equal to or more specific than one of this VPC's CIDR blocks. This route can target only an interface or an instance."}}, 'RequestID': <not asserted>}; actual status 200 and $.Response missing. (The CIDR 10.141.1.0/24 is the request's DestinationCidrBlock, a value from the request, not part of the template.)

Cause 2 — ec2-network-interface-ipv4-prefix-delegation#task#3 (cluster also covers #task#4), CreateNetworkInterface: expected status 200 with networkInterface.status 'pending' and ipv4PrefixSet.item.ipv4Prefix set to the delegated prefix; actual status 501 with no CreateNetworkInterfaceResponse. stub_path empty — the operation is not a scaffold stub.

Cause 3 — ec2-describe-instance-attribute-ena-support#task#4, DescribeInstances (expected 200, actual 200), 8 affected steps: dnsName, networkInterfaceSet.item.publicDnsName, networkInterfaceSet.item.association.publicDnsName, networkInterfaceSet.item.privateIpAddressesSet.item.association.publicDnsName, networkInterfaceSet.item.publicIpDnsNameOptions.publicIpv4DnsName and instancesSet.item.publicIpDnsNameOptions.publicIpv4DnsName all reported host_not_own against the emulator-served name.

Cause 4 — ec2-ecc-traj-os-classification-platform-vs-platform-details#task#8, DescribeInstances (expected 200, actual 200): $.DescribeInstancesResponse.reservationSet type_mismatch — expected '' (empty), got a reservationSet with one reservation and its instancesSet item.

Cause 5 — ec2-sshp-traj-ssh-ingress-rule-authoring-mistakes#task#6 (cluster also covers #task#7), AuthorizeSecurityGroupIngress (expected 400, actual 400): $.Response.Errors.Error.Message value_mismatch — expected 'TCP/UDP (to) port (65536) out of range', got "Invalid value '65536' for toPort. Value must be between -1 and 65535." (65536 is the request's ToPort.)

Cause 6 — ec2-verified-access-delete-lifecycle#task#13, DeleteVerifiedAccessInstance (expected 200, actual 200): $.DeleteVerifiedAccessInstanceResponse.verifiedAccessInstance.lastUpdatedTime value_mismatch — the recorded stored value vs a later same-second value produced by the delete path.

Smithy citations: CreateRoute (input RouteTableId required, DestinationCidrBlock, GatewayId/NetworkInterfaceId/InstanceId targets; errors: none modelled, EC2 query error envelope); CreateNetworkInterfaceRequest.{Ipv4Prefixes -> xmlName Ipv4Prefix, Ipv4PrefixCount}, CreateNetworkInterfaceResult.NetworkInterface; AuthorizeSecurityGroupIngressRequest.{FromPort, ToPort} (Integer, unconstrained); DeleteVerifiedAccessInstanceResult.VerifiedAccessInstance.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus (trace set simparity-wire-aws-v1).
2. Cause 1: replay ec2-create-route-more-specific-than-local-target-restriction up to task#7 — a CreateRoute whose DestinationCidrBlock is equal to or more specific than one of the VPC's CIDR blocks with a non-interface, non-instance target. It answers 200 instead of 400 InvalidParameterValue.
3. Cause 2: replay ec2-network-interface-ipv4-prefix-delegation task#3 and task#4 — CreateNetworkInterface with an Ipv4Prefix. It answers 501.
4. Cause 3: replay ec2-describe-instance-attribute-ena-support task#4 (or any of ec2-instance-monitoring-toggle task#3/task#5, ec2-instance-reboot-and-status-report task#2-#4) and inspect the public DNS members of the running instance; the name announced is not on a host the emulator answers on.
5. Cause 4: replay ec2-ecc-traj-os-classification-platform-vs-platform-details task#8 — the filtered DescribeInstances returns a reservation where the recording returns an empty reservationSet.
6. Cause 5: replay ec2-sshp-traj-ssh-ingress-rule-authoring-mistakes task#6 and task#7 — AuthorizeSecurityGroupIngress with ToPort 65536; compare the Message in the error envelope.
7. Cause 6: replay ec2-verified-access-delete-lifecycle to task#13 and compare verifiedAccessInstance.lastUpdatedTime with the value the resource reported before the delete.
