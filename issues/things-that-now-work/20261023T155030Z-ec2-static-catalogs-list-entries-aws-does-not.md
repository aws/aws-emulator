Issue-Source: trace-sweep-divergence
Issue-Trace: ec2-catalog-describe-instance-type-offerings
Issue-Step: task#27
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ec2-catalog-describe-instance-type-offerings ec2-catalog-get-vpn-connection-device-types ec2-spot-fleet-request-lifecycle-monitoring
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|DescribeInstanceTypeOfferings|2xx->2xx|$.DescribeInstanceTypeOfferingsResponse.instanceTypeOfferingSet.item[instanceType="c8gn.metal-24xl"]
Issue-Key: ec2|DescribeInstanceTypeOfferings|2xx->2xx|$.DescribeInstanceTypeOfferingsResponse.instanceTypeOfferingSet.item[instanceType="c8gn.metal-48xl"]
Issue-Key: ec2|DescribeInstanceTypeOfferings|2xx->2xx|$.DescribeInstanceTypeOfferingsResponse.instanceTypeOfferingSet.item[instanceType="c8ib.metal-48xl"]
Issue-Key: ec2|DescribeInstanceTypeOfferings|2xx->2xx|$.DescribeInstanceTypeOfferingsResponse.instanceTypeOfferingSet.item[instanceType="c8ib.metal-96xl"]
Issue-Key: ec2|DescribeInstanceTypeOfferings|2xx->2xx|$.DescribeInstanceTypeOfferingsResponse.instanceTypeOfferingSet.item[instanceType="i3.metal"]
Issue-Key: ec2|DescribeInstanceTypeOfferings|2xx->2xx|$.DescribeInstanceTypeOfferingsResponse.instanceTypeOfferingSet.item[instanceType="m8gb.metal-24xl"]
Issue-Key: ec2|DescribeInstanceTypeOfferings|2xx->2xx|$.DescribeInstanceTypeOfferingsResponse.instanceTypeOfferingSet.item[instanceType="m8gb.metal-48xl"]
Issue-Key: ec2|DescribeInstanceTypeOfferings|2xx->2xx|$.DescribeInstanceTypeOfferingsResponse.instanceTypeOfferingSet.item[instanceType="m8gn.metal-24xl"]
Issue-Key: ec2|DescribeInstanceTypeOfferings|2xx->2xx|$.DescribeInstanceTypeOfferingsResponse.instanceTypeOfferingSet.item[instanceType="m8gn.metal-48xl"]
Issue-Key: ec2|DescribeInstanceTypeOfferings|2xx->2xx|$.DescribeInstanceTypeOfferingsResponse.instanceTypeOfferingSet.item[instanceType="m8ib.metal-48xl"]
Issue-Key: ec2|DescribeInstanceTypeOfferings|2xx->2xx|$.DescribeInstanceTypeOfferingsResponse.instanceTypeOfferingSet.item[instanceType="m8ib.metal-96xl"]
Issue-Key: ec2|DescribeInstanceTypeOfferings|2xx->2xx|$.DescribeInstanceTypeOfferingsResponse.instanceTypeOfferingSet.item[instanceType="m8idb.metal-48xl"]
Issue-Key: ec2|DescribeInstanceTypeOfferings|2xx->2xx|$.DescribeInstanceTypeOfferingsResponse.instanceTypeOfferingSet.item[instanceType="m8idb.metal-96xl"]
Issue-Key: ec2|DescribeInstanceTypeOfferings|2xx->2xx|$.DescribeInstanceTypeOfferingsResponse.instanceTypeOfferingSet.item[instanceType="m8in.metal-48xl"]
Issue-Key: ec2|DescribeInstanceTypeOfferings|2xx->2xx|$.DescribeInstanceTypeOfferingsResponse.instanceTypeOfferingSet.item[instanceType="m8in.metal-96xl"]
Issue-Key: ec2|DescribeInstanceTypeOfferings|2xx->2xx|$.DescribeInstanceTypeOfferingsResponse.instanceTypeOfferingSet.item[instanceType="m9g.metal-24xl"]
Issue-Key: ec2|DescribeInstanceTypeOfferings|2xx->2xx|$.DescribeInstanceTypeOfferingsResponse.instanceTypeOfferingSet.item[instanceType="m9gd.metal-24xl"]
Issue-Key: ec2|DescribeInstanceTypeOfferings|2xx->2xx|$.DescribeInstanceTypeOfferingsResponse.instanceTypeOfferingSet.item[instanceType="r8gb.metal-24xl"]
Issue-Key: ec2|DescribeInstanceTypeOfferings|2xx->2xx|$.DescribeInstanceTypeOfferingsResponse.instanceTypeOfferingSet.item[instanceType="r8gb.metal-48xl"]
Issue-Key: ec2|DescribeInstanceTypeOfferings|2xx->2xx|$.DescribeInstanceTypeOfferingsResponse.instanceTypeOfferingSet.item[instanceType="r8gn.metal-24xl"]
Issue-Key: ec2|DescribeInstanceTypeOfferings|2xx->2xx|$.DescribeInstanceTypeOfferingsResponse.instanceTypeOfferingSet.item[instanceType="r8gn.metal-48xl"]
Issue-Key: ec2|DescribeInstanceTypeOfferings|2xx->2xx|$.DescribeInstanceTypeOfferingsResponse.instanceTypeOfferingSet.item[instanceType="r8ib.metal-48xl"]
Issue-Key: ec2|DescribeInstanceTypeOfferings|2xx->2xx|$.DescribeInstanceTypeOfferingsResponse.instanceTypeOfferingSet.item[instanceType="r8ib.metal-96xl"]
Issue-Key: ec2|DescribeInstanceTypeOfferings|2xx->2xx|$.DescribeInstanceTypeOfferingsResponse.instanceTypeOfferingSet.item[instanceType="r8idb.metal-48xl"]
Issue-Key: ec2|DescribeInstanceTypeOfferings|2xx->2xx|$.DescribeInstanceTypeOfferingsResponse.instanceTypeOfferingSet.item[instanceType="r8idb.metal-96xl"]
Issue-Key: ec2|DescribeInstanceTypeOfferings|2xx->2xx|$.DescribeInstanceTypeOfferingsResponse.instanceTypeOfferingSet.item[instanceType="r8idn.metal-48xl"]
Issue-Key: ec2|DescribeInstanceTypeOfferings|2xx->2xx|$.DescribeInstanceTypeOfferingsResponse.instanceTypeOfferingSet.item[instanceType="r8idn.metal-96xl"]
Issue-Key: ec2|DescribeInstanceTypeOfferings|2xx->2xx|$.DescribeInstanceTypeOfferingsResponse.instanceTypeOfferingSet.item[instanceType="r8in.metal-48xl"]
Issue-Key: ec2|DescribeInstanceTypeOfferings|2xx->2xx|$.DescribeInstanceTypeOfferingsResponse.instanceTypeOfferingSet.item[instanceType="r8in.metal-96xl"]
Issue-Key: ec2|DescribeInstanceTypeOfferings|2xx->2xx|$.DescribeInstanceTypeOfferingsResponse.instanceTypeOfferingSet.item[instanceType="u-6tb1.metal"]
Issue-Key: ec2|GetVpnConnectionDeviceTypes|2xx->2xx|$.GetVpnConnectionDeviceTypesResponse.vpnConnectionDeviceTypeSet.item[*]
Issue-Key: ec2|GetVpnConnectionDeviceTypes|2xx->2xx|$headers.content-type
Issue-Key: ec2|DescribeImages|2xx->2xx|$.DescribeImagesResponse.imagesSet
Issue-Related: 20261023T102725Z-ec2-responses-send-content-type-text-xml-without.md
Issue-Related: 20261023T155030Z-ec2-content-type-lacks-charset-on-fourteen-further.md
Issue-Commit: 6d7833292dd9d4a45d8e52e2aa7ee7e5a822f7f5

# EC2 static catalogs list entries AWS does not return

> _Note: this issue was identified against the code base at commit `6d7833292dd9d4a45d8e52e2aa7ee7e5a822f7f5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeInstanceTypeOfferings

## Description
Three causes with one root: the emulator's seeded reference catalogs are not the ones AWS serves for this region, so read-only catalog operations answer 200 with the wrong inventory.

Cause 1 of 3 — DescribeInstanceTypeOfferings offers instance types AWS does not offer in the region.
AWS: for a given instance-type family filter, DescribeInstanceTypeOfferings returns only the types actually offered at locationType 'region' for that region; the recorded responses for these 17 steps do NOT include the bare-metal variants the emulator adds. Smithy: no enumeration of offered types is modeled, so this is recorded behavior only.
Emulator: each of the 17 filtered queries returns EXTRA offering items for the metal variants of the family — c8gn.metal-24xl/48xl, c8ib.metal-48xl/96xl, i3.metal, m8gb.metal-24xl/48xl, m8gn.metal-24xl/48xl, m8ib.metal-48xl/96xl, m8idb.metal-48xl/96xl, m8in.metal-48xl/96xl, m9g.metal-24xl, m9gd.metal-24xl, r8gb.metal-24xl/48xl, r8gn.metal-24xl/48xl, r8ib.metal-48xl/96xl, r8idb.metal-48xl/96xl, r8idn.metal-48xl/96xl, r8in.metal-48xl/96xl, u-6tb1.metal — each reported as {locationType 'region', location 'us-west-2'}.
Traces: ec2-catalog-describe-instance-type-offerings steps task#27, 30, 53, 91, 93, 96, 98, 100, 102, 103, 144, 146, 149, 151, 152, 153, 165 (one cluster each, all 200->200, all keyed only on extra instanceType items).

Cause 2 of 3 — GetVpnConnectionDeviceTypes returns a different device catalog.
AWS: the vpnConnectionDeviceTypeSet is the full AWS-published list, ordered as recorded and starting with the AXGate / Barracuda / Checkpoint / Cisco Meraki entries and their vpnConnectionDeviceTypeId values.
Emulator: a short, differently ordered list (first items Check Point Security Gateway, Cisco ASA, Cisco ISR, Fortinet Fortigate, Juniper J-Series, Juniper SRX) with different vpnConnectionDeviceTypeId values; the replay reports a divergence on every list index from item[0] through item[53].
Trace: ec2-catalog-get-vpn-connection-device-types#task#1.

Cause 3 of 3 — DescribeImages returns imagesSet with the wrong type/contents for the public AMI catalog.
AWS: DescribeImages for the Amazon Linux public AMIs returns imagesSet as an object with an 'item' LIST of image records (imageId, imageLocation, imageState, imageOwnerId 137112412989, creationDate, isPublic, architecture, blockDeviceMapping, platformDetails, usageOperation, deprecationTime, publicSsmParameterName, ...).
Emulator: imagesSet is a different type than the recorded list-bearing object ('type_mismatch'), so the public-AMI catalog this query depends on is not served in the recorded shape.
Trace: ec2-spot-fleet-request-lifecycle-monitoring#task#1.

Why: these are the discovery calls clients use before committing to a resource. A client that enumerates offerings to pick a launchable instance type will pick a metal type the region does not offer and then fail at RunInstances with an unrelated error; capacity-planning and cost tooling that diffs the offering list against its own inventory reports phantom capacity. A client that resolves a customer-gateway device type id from GetVpnConnectionDeviceTypes gets an id AWS does not know, so the follow-up GetVpnConnectionDeviceSampleConfiguration cannot be made. And a client that resolves an AMI id by listing public images cannot parse the list at all.

Fix: the seeded reference data in the ec2 service crate backing these three read-only handlers — the instance-type-offerings table (it must not list region offerings AWS does not publish for the region), the VPN connection device-type table, and the public-AMI catalog plus the DescribeImages serializer that renders imagesSet.

Traces: the 19 fixture#step bindings named under the three causes above.

## Evidence
Replay of simparity-wire-aws-v1, 19 divergence clusters, all status 200 expected and returned.

Cause 1 — 17 clusters with divergence_key 'service=ec2|DescribeInstanceTypeOfferings|status=200->200|keys=...item[instanceType="<type>"],$headers.content-type'. Representative, fixture ec2-catalog-describe-instance-type-offerings step task#27:
  '$.DescribeInstanceTypeOfferingsResponse.instanceTypeOfferingSet.item[instanceType="c8gn.metal-24xl"]: extra (expected None, got {"instanceType": "c8gn.metal-24xl", "locationType": "region", "location": "us-west-2"})'
  and the same for c8gn.metal-48xl. The other 16 steps report the same 'extra' verdict for the metal variants listed in the description.

Cause 2 — divergence_key 'service=ec2|GetVpnConnectionDeviceTypes|status=200->200|keys=...item[0]..item[53],$headers.content-type', fixture ec2-catalog-get-vpn-connection-device-types step task#1:
  'item[0]: missing (expected {"platform": "NF", "software": "aos 3.2+", "vendor": "AXGate", "vpnConnectionDeviceTypeId": "681ac807"}, got {"vpnConnectionDeviceTypeId": "3b0a37c2", "vendor": "Check Point", "platform": "Security Gateway", "software": "R77.10+"})' and 53 further index divergences.

Cause 3 — divergence_key 'service=ec2|DescribeImages|status=200->200|keys=$.DescribeImagesResponse.imagesSet,$headers.content-type', fixture ec2-spot-fleet-request-lifecycle-monitoring step task#1:
  '$.DescribeImagesResponse.imagesSet: type_mismatch (expected {"item": [{imageId ami-004f66d587b3a9267, imageOwnerId 137112412989, ...}, ...]})'.

Individual ami-/snap-/afi- ids and creationDate values in the recordings are catalog data from the recorded account/region and are cited as examples of the expected shape and ownership, not as literal requirements beyond the published catalog.

## Steps to Reproduce
1. git_clone() then replay_wire_corpus().
2. Replay fixture ec2-catalog-describe-instance-type-offerings. At step task#27 (and the 16 other steps in trace_ids) the emulator's instanceTypeOfferingSet contains metal variants (c8gn.metal-24xl, c8gn.metal-48xl, ... u-6tb1.metal) that the recorded region offering list does not contain.
3. Replay fixture ec2-catalog-get-vpn-connection-device-types, step task#1: compare vpnConnectionDeviceTypeSet against the recording — the vendor/platform/software/id entries differ from index 0 through 53.
4. Replay fixture ec2-spot-fleet-request-lifecycle-monitoring, step task#1: DescribeImagesResponse.imagesSet does not have the recorded object-with-item-list shape carrying the public Amazon Linux AMIs.
