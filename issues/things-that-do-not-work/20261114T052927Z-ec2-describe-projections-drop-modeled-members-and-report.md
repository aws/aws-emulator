Issue-Source: trace-sweep-divergence
Issue-Trace: ec2-madl-traj-vpc-subnet-discovery-filters
Issue-Step: task#8
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ec2-madl-traj-vpc-subnet-discovery-filters ec2-subnets-subnet-ids-and-empty-list ec2-describe-nat-gateways-pagination ec2-describe-images-self-owned-pagination-and-params ec2-pst-traj-route-filters-match-per-table-not-per-route
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|DescribeSubnets|2xx->2xx|$.DescribeSubnetsResponse.subnetSet.item[subnetId="subnet-00000000000000095"].dhcpOptionsId
Issue-Key: ec2|DescribeSubnets|2xx->2xx|$.DescribeSubnetsResponse.subnetSet.item[subnetId="subnet-00000000000000095"].dhcpOptionsSource
Issue-Key: ec2|DescribeSubnets|2xx->2xx|$.DescribeSubnetsResponse.subnetSet.item[subnetId="subnet-00000000000000097"].dhcpOptionsId
Issue-Key: ec2|DescribeSubnets|2xx->2xx|$.DescribeSubnetsResponse.subnetSet.item[subnetId="subnet-00000000000000097"].dhcpOptionsSource
Issue-Key: ec2|DescribeNatGateways|2xx->2xx|$.DescribeNatGatewaysResponse.natGatewaySet.item[subnetId="subnet-00000000000000097"].attachedApplianceSet
Issue-Key: ec2|DescribeNatGateways|2xx->2xx|$.DescribeNatGatewaysResponse.natGatewaySet.item[subnetId="subnet-00000000000000099"].attachedApplianceSet
Issue-Key: ec2|DescribeNatGateways|2xx->2xx|$.DescribeNatGatewaysResponse.natGatewaySet.item[subnetId="subnet-0000000000000009b"].attachedApplianceSet
Issue-Key: ec2|DescribeNatGateways|2xx->2xx|$.DescribeNatGatewaysResponse.natGatewaySet.item[subnetId="subnet-0000000000000009d"].attachedApplianceSet
Issue-Key: ec2|DescribeNatGateways|2xx->2xx|$.DescribeNatGatewaysResponse.natGatewaySet.item[subnetId="subnet-0000000000000009f"].attachedApplianceSet
Issue-Key: ec2|DescribeNatGateways|2xx->2xx|$.DescribeNatGatewaysResponse.natGatewaySet.item[subnetId="subnet-000000000000000a1"].attachedApplianceSet
Issue-Key: ec2|DescribeImages|2xx->2xx|$.DescribeImagesResponse.imagesSet.item[name="sim2real-probe-ec2-img-cc8832cc-1"].blockDeviceMapping.item.ebs.encrypted
Issue-Key: ec2|DescribeImages|2xx->2xx|$.DescribeImagesResponse.imagesSet.item[name="sim2real-probe-ec2-img-cc8832cc-1"].freeTierEligible
Issue-Key: ec2|DescribeImages|2xx->2xx|$.DescribeImagesResponse.imagesSet.item[name="sim2real-probe-ec2-img-cc8832cc-2"].blockDeviceMapping.item.ebs.encrypted
Issue-Key: ec2|DescribeImages|2xx->2xx|$.DescribeImagesResponse.imagesSet.item[name="sim2real-probe-ec2-img-cc8832cc-2"].freeTierEligible
Issue-Key: ec2|DescribeImages|2xx->2xx|$.DescribeImagesResponse.imagesSet.item[name="sim2real-probe-ec2-img-cc8832cc-3"].blockDeviceMapping.item.ebs.encrypted
Issue-Key: ec2|DescribeImages|2xx->2xx|$.DescribeImagesResponse.imagesSet.item[name="sim2real-probe-ec2-img-cc8832cc-3"].freeTierEligible
Issue-Key: ec2|DescribeImages|2xx->2xx|$.DescribeImagesResponse.imagesSet.item[name="sim2real-probe-ec2-img-cc8832cc-4"].blockDeviceMapping.item.ebs.encrypted
Issue-Key: ec2|DescribeImages|2xx->2xx|$.DescribeImagesResponse.imagesSet.item[name="sim2real-probe-ec2-img-cc8832cc-4"].freeTierEligible
Issue-Key: ec2|DescribeImages|2xx->2xx|$.DescribeImagesResponse.imagesSet.item[name="sim2real-probe-ec2-img-cc8832cc-5"].blockDeviceMapping.item.ebs.encrypted
Issue-Key: ec2|DescribeImages|2xx->2xx|$.DescribeImagesResponse.imagesSet.item[name="sim2real-probe-ec2-img-cc8832cc-5"].freeTierEligible
Issue-Key: ec2|DescribeImages|2xx->2xx|$.DescribeImagesResponse.imagesSet.item[name="sim2real-probe-ec2-img-cc8832cc-6"].blockDeviceMapping.item.ebs.encrypted
Issue-Key: ec2|DescribeImages|2xx->2xx|$.DescribeImagesResponse.imagesSet.item[name="sim2real-probe-ec2-img-cc8832cc-6"].freeTierEligible
Issue-Key: ec2|DescribeImages|2xx->2xx|$.DescribeImagesResponse.imagesSet.item[name="sim2real-probe-ec2-img-cc8832cc-7"].blockDeviceMapping.item.ebs.encrypted
Issue-Key: ec2|DescribeImages|2xx->2xx|$.DescribeImagesResponse.imagesSet.item[name="sim2real-probe-ec2-img-cc8832cc-7"].freeTierEligible
Issue-Key: ec2|DescribeImages|2xx->2xx|$.DescribeImagesResponse.imagesSet.item[name="sim2real-probe-ec2-img-cc8832cc-8"].blockDeviceMapping.item.ebs.encrypted
Issue-Key: ec2|DescribeImages|2xx->2xx|$.DescribeImagesResponse.imagesSet.item[name="sim2real-probe-ec2-img-cc8832cc-8"].freeTierEligible
Issue-Key: ec2|DescribeRouteTables|2xx->2xx|$.DescribeRouteTablesResponse.routeTableSet.item[routeTableId="rtb-000000000000000a8"].routeSet.item[*].state
Issue-Related: 20261109T225653Z-ec2-describe-reads-omit-modeled-members-publish-foreign.md
Issue-Commit: 19014e1eb0a1b430b60672bc03abfedd0d5a09dd

# ec2 describe projections drop modeled members and report stale route state (4 causes)

> _Note: this issue was identified against the code base at commit `19014e1eb0a1b430b60672bc03abfedd0d5a09dd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeSubnets

## Description
This issue bundles 4 net-new causes, all in ec2 read paths that project stored resources onto the wire: three omit members AWS always serves, one serves a state value the emulator never recomputes. A fixer must close ALL FOUR and re-replay each fixture#step named below before submitting.

Cause 1 of 4 - DescribeSubnets omits dhcpOptionsId and dhcpOptionsSource (representative: fixture ec2-madl-traj-vpc-subnet-discovery-filters, step task#8; second cluster ec2-subnets-subnet-ids-and-empty-list#task#4).
AWS: recorded behavior only - the checked-out Smithy model's com.amazonaws.ec2#Subnet shape declares no DhcpOptionsId/DhcpOptionsSource member, so the model says nothing about them. The recording shows DescribeSubnets answering HTTP 200 with, on EVERY subnetSet item, dhcpOptionsId carrying the id of the DHCP option set that applies to the subnet (the one associated with the subnet's VPC, e.g. a dopt-<17-hex> id) and dhcpOptionsSource carrying 'vpc' when the subnet inherits that option set from its VPC rather than having a subnet-level override.
Emulator: both elements are absent from every subnet item (replay detail: dhcpOptionsId missing (expected 'dopt-...', got None); dhcpOptionsSource missing (expected 'vpc', got None)). Status is 200 in both recordings, so only the member set diverges.
Why: a client that reads a subnet's DHCP configuration (domain name, DNS resolver, NTP settings) to decide whether it must create a subnet-level override, or that renders which option set is in effect, sees nothing at all and cannot distinguish 'inherited from VPC' from 'unset'.
Fix: the DescribeSubnets subnet projection/serializer - derive dhcpOptionsId from the subnet's VPC DHCP-options association and emit dhcpOptionsSource='vpc' for an inherited association.
Traces: ec2-madl-traj-vpc-subnet-discovery-filters#task#8, ec2-subnets-subnet-ids-and-empty-list#task#4.

Cause 2 of 4 - DescribeNatGateways omits attachedApplianceSet (representative: fixture ec2-describe-nat-gateways-pagination, step task#20).
AWS: Smithy: NatGateway.AttachedAppliances -> xmlName/ec2QueryName attachedApplianceSet / AttachedApplianceSet, target NatGatewayAttachedApplianceList, documented as 'The proxy appliances attached to the NAT Gateway for filtering and inspecting traffic to prevent data exfiltration.' The recording shows the element PRESENT and EMPTY on every natGatewaySet item for gateways with no attached appliances.
Emulator: the member is not serialized at all (replay detail: attachedApplianceSet: missing (expected '', got None)) for all seven NAT gateways in the page.
Why: AWS distinguishes 'no appliances attached' (empty list) from a member that is not part of the response; a client iterating attachedApplianceSet gets null instead of an empty collection, and strict consumers of the ec2Query XML see a different shape than AWS serves.
Fix: the DescribeNatGateways NAT-gateway projection/serializer - always emit attachedApplianceSet, empty when the gateway has no appliances.
Traces: ec2-describe-nat-gateways-pagination#task#20.

Cause 3 of 4 - DescribeImages drops blockDeviceMapping ebs.encrypted and reports freeTierEligible false for self-owned AMIs (representative: fixture ec2-describe-images-self-owned-pagination-and-params, step task#15).
AWS: Smithy: Image.FreeTierEligible - 'Indicates whether the image is eligible for Amazon Web Services Free Tier' (true when the AMI can be used to launch instances under the Free Tier limits); EbsBlockDevice.Encrypted -> xmlName encrypted, a boolean that is serialized for each blockDeviceMapping item. The recording shows freeTierEligible 'true' for each of these self-owned AMIs and encrypted 'false' present on each image's blockDeviceMapping ebs element.
Emulator: freeTierEligible is 'false' for every self-owned image and blockDeviceMapping.item.ebs.encrypted is omitted entirely (replay detail, repeated for all 8 images in the page: ebs.encrypted missing (expected 'false', got None); freeTierEligible value_mismatch (expected 'true', got 'false')).
Why: free-tier gating reads the wrong answer, and a client auditing AMIs for unencrypted snapshots gets no encryption flag at all rather than an explicit 'false', so an unencrypted image is indistinguishable from an image whose encryption state is unknown.
Fix: the DescribeImages image projection - carry the stored block-device encryption flag into ebs.encrypted and compute freeTierEligible for images the account owns.
Traces: ec2-describe-images-self-owned-pagination-and-params#task#15.

Cause 4 of 4 - DescribeRouteTables keeps a route whose target is unavailable in state 'active' instead of 'blackhole' (representative: fixture ec2-pst-traj-route-filters-match-per-table-not-per-route, step task#11; same cluster also covers task#12 and task#13 of that fixture).
AWS: Smithy: Route.State -> RouteState enum, documented as 'The state of the route. The blackhole state indicates that the route's target isn't available (for example, the specified gateway isn't attached to the VPC, or the specified NAT instance has been terminated).' The recording shows the affected routeSet item reading state 'blackhole' after its target stopped being available, while the other routes of the same table stay 'active'.
Emulator: that route reads state 'active' (replay detail: routeSet.item[2].state value_mismatch (expected 'blackhole', got 'active')) and stays 'active' across the three consecutive describe steps, so the state is never recomputed from target availability.
Why: blackhole is the only signal AWS gives that a route is installed but unusable; a client that troubleshoots connectivity or that filters route tables on route.state=blackhole concludes every route is healthy and never surfaces the broken one.
Fix: route state in the route-table state layer / the DescribeRouteTables projection - derive each route's state from whether its target (gateway attachment, NAT gateway/instance, interface, peering connection) is still available instead of storing 'active' at creation.
Traces: ec2-pst-traj-route-filters-match-per-table-not-per-route#task#11, #task#12, #task#13.

Related OPEN issues that do NOT carry these replay keys, so these clusters stay unrun under them: 20261111T001230Z-ec2-describe-and-create-responses-omit-modeled-members.md, 20261112T182924Z-ec2-describeimages-omits-ebs-encrypted-and-reports-freetiereligible.md, 20261112T231741Z-ec2-describeimages-drops-blockdevicemapping-ebs-encrypted-and-reports.md, 20261112T211315Z-ec2-read-responses-carry-the-wrong-optional-members.md, 20261114T013533Z-ec2-read-paths-serve-no-catalog-records-and.md.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout (emulator_up true, bootstrap_signal false): 5 divergence clusters, all verdict=mismatch with empty stub_path and empty owned_by.
- ec2-madl-traj-vpc-subnet-discovery-filters#task#8, DescribeSubnets, expected 200 / actual 200: '$.DescribeSubnetsResponse.subnetSet.item[subnetId="subnet-...97"].dhcpOptionsId: missing (expected 'dopt-03d13527b92584567', got None)' and '...dhcpOptionsSource: missing (expected 'vpc', got None)', repeated for every subnet in the page.
- ec2-subnets-subnet-ids-and-empty-list#task#4, DescribeSubnets, 200/200: same two members missing (expected 'dopt-0000000000000008e' / 'vpc').
- ec2-describe-nat-gateways-pagination#task#20, DescribeNatGateways, 200/200: '$.DescribeNatGatewaysResponse.natGatewaySet.item[subnetId="..."].attachedApplianceSet: missing (expected '', got None)' for all 7 gateways.
- ec2-describe-images-self-owned-pagination-and-params#task#15, DescribeImages, 200/200: 'blockDeviceMapping.item.ebs.encrypted: missing (expected 'false', got None)' and 'freeTierEligible: value_mismatch (expected 'true', got 'false')' for all 8 self-owned images.
- ec2-pst-traj-route-filters-match-per-table-not-per-route#task#11 (affected also task#12, task#13), DescribeRouteTables, 200/200: '$.DescribeRouteTablesResponse.routeTableSet.item[routeTableId="rtb-..."].routeSet.item[2].state: value_mismatch (expected 'blackhole', got 'active')'.
The id/name values quoted above are per-environment; the requirement is the member being present and the state being derived, not those literals.

## Steps to Reproduce
1. Clone this checkout, build and boot the emulator, then replay trace set simparity-wire-aws-v1.
2. Cause 1: replay ec2-madl-traj-vpc-subnet-discovery-filters step task#8 and ec2-subnets-subnet-ids-and-empty-list step task#4; in each DescribeSubnets response check every subnetSet item for dhcpOptionsId and dhcpOptionsSource ('vpc' for a VPC-inherited option set).
3. Cause 2: replay ec2-describe-nat-gateways-pagination step task#20 and check that each natGatewaySet item carries an attachedApplianceSet element (empty when no appliance is attached).
4. Cause 3: replay ec2-describe-images-self-owned-pagination-and-params step task#15 and check each imagesSet item for blockDeviceMapping.item.ebs.encrypted and for freeTierEligible=true on the self-owned AMIs.
5. Cause 4: replay ec2-pst-traj-route-filters-match-per-table-not-per-route steps task#11, task#12 and task#13 and check that the route whose target is no longer available reads state 'blackhole'.
6. All five steps must pass before the issue is closed.
