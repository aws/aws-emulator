Issue-Source: trace-sweep-divergence
Issue-Trace: ec2-catalog-describe-instance-types-a1-c9g
Issue-Step: task#9
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ec2-catalog-describe-instance-types-a1-c9g ec2-catalog-describe-instance-type-offerings ec2-catalog-get-vpn-connection-device-types
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5n.18xlarge"].networkInfo.networkCards.item.defaultEnaQueueCountPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5n.18xlarge"].networkInfo.networkCards.item.interfaceTypeSet.item[*]
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5n.2xlarge"].networkInfo.networkCards.item.baselineBandwidthInGbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5n.2xlarge"].networkInfo.networkCards.item.defaultEnaQueueCountPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5n.2xlarge"].networkInfo.networkCards.item.networkPerformance
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5n.2xlarge"].networkInfo.networkCards.item.peakBandwidthInGbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5n.4xlarge"].networkInfo.networkCards.item.baselineBandwidthInGbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5n.4xlarge"].networkInfo.networkCards.item.defaultEnaQueueCountPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5n.4xlarge"].networkInfo.networkCards.item.networkPerformance
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5n.4xlarge"].networkInfo.networkCards.item.peakBandwidthInGbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5n.9xlarge"].networkInfo.networkCards.item.defaultEnaQueueCountPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5n.9xlarge"].networkInfo.networkCards.item.interfaceTypeSet.item
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5n.large"].networkInfo.networkCards.item.baselineBandwidthInGbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5n.large"].networkInfo.networkCards.item.defaultEnaQueueCountPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5n.large"].networkInfo.networkCards.item.networkPerformance
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5n.large"].networkInfo.networkCards.item.peakBandwidthInGbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5n.metal"].networkInfo.networkCards.item.defaultEnaQueueCountPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5n.metal"].networkInfo.networkCards.item.interfaceTypeSet.item[*]
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5n.xlarge"].networkInfo.networkCards.item.baselineBandwidthInGbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5n.xlarge"].networkInfo.networkCards.item.defaultEnaQueueCountPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5n.xlarge"].networkInfo.networkCards.item.networkPerformance
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5n.xlarge"].networkInfo.networkCards.item.peakBandwidthInGbps
Issue-Key: ec2|DescribeInstanceTypeOfferings|2xx->2xx|$.DescribeInstanceTypeOfferingsResponse.instanceTypeOfferingSet.item[instanceType="c8gn.metal-24xl"]
Issue-Key: ec2|DescribeInstanceTypeOfferings|2xx->2xx|$.DescribeInstanceTypeOfferingsResponse.instanceTypeOfferingSet.item[instanceType="c8gn.metal-48xl"]
Issue-Key: ec2|GetVpnConnectionDeviceTypes|2xx->2xx|$.GetVpnConnectionDeviceTypesResponse.vpnConnectionDeviceTypeSet.item[*]
Issue-Related: 20261101T141251Z-ec2-describeinstancetypes-catalog-sweep-per-type-attributes-are.md
Issue-Commit: 19014e1eb0a1b430b60672bc03abfedd0d5a09dd

# ec2 static catalogs serve synthesized records: instance types, type offerings and VPN device types (3 causes)

> _Note: this issue was identified against the code base at commit `19014e1eb0a1b430b60672bc03abfedd0d5a09dd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeInstanceTypes

## Description
This issue bundles 3 net-new causes, all in ec2 read-only catalog operations that must serve AWS's published reference data and instead serve data the emulator makes up. A fixer must close ALL THREE and re-replay each fixture#step named below before submitting.

Cause 1 of 3 - DescribeInstanceTypes reports size-derived hardware facts and omits defaultEnaQueueCountPerInterface (representative: fixture ec2-catalog-describe-instance-types-a1-c9g, step task#9; the same cause reproduces in 95+ further clusters across the ec2-catalog-describe-instance-types-* pages, all of them net-new - owned_by empty).
AWS: recorded behavior only for the VALUES (the model fixes only the member set: Smithy InstanceTypeInfo with memoryInfo.sizeInMiB, processorInfo.sustainedClockSpeedInGhz, vCpuInfo.validCores, hibernationSupported, networkInfo.networkPerformance and networkInfo.networkCards -> NetworkCardInfo{baselineBandwidthInGbps, peakBandwidthInGbps, networkPerformance, defaultEnaQueueCountPerInterface, interfaceTypeSet}, ebsInfo.ebsOptimizedInfo -> EbsOptimizedInfo{baselineBandwidthInMbps, baselineIops, baselineThroughputInMBps, maximumBandwidthInMbps, maximumIops, maximumThroughputInMBps}, networkInfo.efaSupported/efaInfo). The recording shows HTTP 200 and each instanceTypeSet item carrying that type's published facts - e.g. for c5n.2xlarge: memoryInfo.sizeInMiB 21504, processorInfo.sustainedClockSpeedInGhz 3.4, networkInfo.networkPerformance and networkCards.item.networkPerformance 'Up to 25 Gigabit', networkCards.item.baselineBandwidthInGbps 10.0 / peakBandwidthInGbps 25.0, ebsOptimizedInfo.baselineIops 10000 / maximumIops 20000, vCpuInfo.validCores 2 and 4, hibernationSupported false, and defaultEnaQueueCountPerInterface 8.
Emulator: those fields are computed from the type's size tier instead of looked up, so they diverge per type - for the same c5n.2xlarge it answers memoryInfo.sizeInMiB 16384, sustainedClockSpeedInGhz 3.1, networkPerformance '11.11 Gigabit' (a string shape AWS never uses; AWS words it 'Up to N Gigabit' or 'N Gigabit'), baselineBandwidthInGbps/peakBandwidthInGbps both 11.11, baselineIops 12000 / maximumIops 18750, validCores 1 and 2, hibernationSupported true - and networkCards.item.defaultEnaQueueCountPerInterface is not serialized at all (missing, expected '8'). Other pages additionally diverge on efaSupported/efaInfo, interfaceTypeSet and ebsOptimizedInfo bandwidth/throughput.
Why: instance-type selection, memory/network/EBS sizing and EFA capability checks all read these numbers; a client sized against the emulator picks a type that cannot host its workload, and code reading defaultEnaQueueCountPerInterface gets null where AWS always serves a value.
Fix: the ec2 instance-type catalog data behind DescribeInstanceTypes (real per-type records replacing the size-derived computation) plus the InstanceTypeInfo serializer for the members it never emits.
Traces: ec2-catalog-describe-instance-types-a1-c9g#task#9, plus 95+ further clusters on the other ec2-catalog-describe-instance-types-* pages (e.g. ec2-catalog-describe-instance-types-c9gd-m6gd, ec2-catalog-describe-instance-types-m6i-r4) - re-replay the whole fixture family, not only the representative step.

Cause 2 of 3 - DescribeInstanceTypeOfferings offers instance types AWS does not offer in the location (representative: fixture ec2-catalog-describe-instance-type-offerings, step task#27; same cause at steps task#30, task#53, task#91, task#93, task#96, task#98, task#100, task#102, task#103, task#144, task#146, task#149, task#151, task#152, task#153 of the same fixture).
AWS: recorded behavior only - the operation answers HTTP 200 with one instanceTypeOfferingSet item per instance type that is actually offered in the requested locationType/location, so a type that is not offered there is simply ABSENT from the set. The recording contains no offering for the *.metal-24xl / *.metal-48xl / *.metal-96xl variants of the c8gn, c8ib, m8gb, m8gn, m8ib, m8idb, m8in, m9g, m9gd, r8gb, r8gn, r8ib, r8idb, r8idn and r8in families, and none for i3.metal, in this region.
Emulator: it returns those types as offered (replay detail: instanceTypeOfferingSet.item[instanceType="c8gn.metal-24xl"]: extra (expected None, got {'instanceType': 'c8gn.metal-24xl', 'locationType': 'region', 'location': ...}), and the same 'extra' verdict for each type listed above).
Why: DescribeInstanceTypeOfferings is exactly the pre-flight a client uses to decide whether a type is launchable in a region/AZ; the emulator answers yes for types that do not exist there, so the client proceeds to a RunInstances/CreateFleet that real AWS refuses.
Fix: the offerings catalog behind DescribeInstanceTypeOfferings (the per-location offering table it enumerates, which currently includes every type name it can synthesize) - not the serializer.
Traces: ec2-catalog-describe-instance-type-offerings#task#27 and the sibling steps listed above.

Cause 3 of 3 - GetVpnConnectionDeviceTypes serves a different customer-gateway device catalog (representative: fixture ec2-catalog-get-vpn-connection-device-types, step task#1).
AWS: Smithy: GetVpnConnectionDeviceTypes -> VpnConnectionDeviceTypes (xmlName vpnConnectionDeviceTypeSet) of VpnConnectionDeviceType{vpnConnectionDeviceTypeId, vendor, platform, software}, documented as the 'List of customer gateway devices that have a sample configuration file available for use'. The entries, their ids and their order are AWS's published catalog constants, not per-run values: the recording's first entries are vendor 'AXGate' platform 'NF' software 'aos 3.2+' with vpnConnectionDeviceTypeId '681ac807', then 'AXGate'/'UTM'/'aos 2.1+' ('9af4506c'), then the Barracuda entries, then vendor 'Checkpoint' with platform 'Gaia' for software 'R77.10+' and 'R80.10+', then 'Cisco Meraki'/'MX Series'/'15.12+ (WebUI)'.
Emulator: every index of the set diverges - it emits different vpnConnectionDeviceTypeId values for the same device (e.g. '6edf2732' instead of '681ac807' for AXGate NF), drops the second AXGate entry so the whole list is shifted, and spells vendor/platform/software differently ('Check Point'/'Security Gateway' instead of 'Checkpoint'/'Gaia', 'MX'/'15.12+' instead of 'MX Series'/'15.12+ (WebUI)').
Why: the id returned here is the handle a client passes to GetVpnConnectionDeviceSampleConfiguration, so the emulator hands out ids that do not exist in AWS, and any UI or automation that matches on vendor/platform text fails to find the device it just listed.
Fix: the static VPN customer-gateway device-type catalog behind GetVpnConnectionDeviceTypes (its records, ids and ordering).
Traces: ec2-catalog-get-vpn-connection-device-types#task#1.

Related OPEN issues that do NOT carry these replay keys, so these clusters stay unrun under them: 20261114T013533Z-ec2-describeinstancetypes-serves-size-derived-hardware-facts-across.md, 20261112T211315Z-ec2-describeinstancetypes-catalog-pages-serve-synthesized-per-type.md, 20261112T231741Z-ec2-describeinstancetypes-catalog-rows-are-synthesized-from-a.md, 20261112T182924Z-ec2-describeinstancetypes-rows-for-the-c4-c9gd-m6i.md, 20261111T180943Z-ec2-read-paths-serve-emulator-derived-catalog-data.md.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout (emulator_up true, bootstrap_signal false). All clusters below are verdict=mismatch with empty stub_path and empty owned_by.
- ec2-catalog-describe-instance-types-a1-c9g#task#9, DescribeInstanceTypes, expected 200 / actual 200: 'c5n.2xlarge.ebsInfo.ebsOptimizedInfo.baselineIops: value_mismatch (expected '10000', got '12000')', 'maximumIops (expected '20000', got '18750')', 'hibernationSupported (expected 'false', got 'true')', 'memoryInfo.sizeInMiB (expected '21504', got '16384')', 'networkCards.item.baselineBandwidthInGbps (expected '10.0', got '11.11')', 'networkCards.item.defaultEnaQueueCountPerInterface: missing (expected '8', got None)', 'networkCards.item.networkPerformance (expected 'Up to 25 Gigabit', got '11.11 Gigabit')', 'peakBandwidthInGbps (expected '25.0', got '11.11')', 'processorInfo.sustainedClockSpeedInGhz (expected '3.4', got '3.1')', 'vCpuInfo.validCores.item[0] (expected '2', got '1')'; the same divergence_key family repeats for c5n.large/xlarge/4xlarge/9xlarge/18xlarge/metal and for 95+ further clusters on the other catalog pages.
- ec2-catalog-describe-instance-type-offerings#task#27, DescribeInstanceTypeOfferings, 200/200: 'item[instanceType="c8gn.metal-24xl"]: extra (expected None, got {...})' and 'c8gn.metal-48xl: extra'; sibling steps report the same 'extra' verdict for c8ib.metal-48xl/96xl, i3.metal, m8gb/m8gn/m8ib/m8idb/m8in.metal-*, m9g/m9gd.metal-24xl and r8gb/r8gn/r8ib/r8idb/r8idn/r8in.metal-*.
- ec2-catalog-get-vpn-connection-device-types#task#1, GetVpnConnectionDeviceTypes, 200/200: 'vpnConnectionDeviceTypeSet.item[0]: missing (expected {'platform': 'NF', 'software': 'aos 3.2+', 'vendor': 'AXGate', 'vpnConnectionDeviceTypeId': '681ac807'}, got {'vpnConnectionDeviceTypeId': '6edf2732', 'vendor': 'AXGate', 'platform': 'NF', 'software': 'aos 3.2+'})', 'item[1] expected AXGate/UTM/'aos 2.1+'/'9af4506c', got Barracuda/CloudGen Firewall/8.0+', 'item[3] expected vendor 'Checkpoint' platform 'Gaia', got vendor 'Check Point' platform 'Security Gateway'' - all 55 indices diverge.

## Steps to Reproduce
1. Clone this checkout, build and boot the emulator, then replay trace set simparity-wire-aws-v1.
2. Cause 1: replay ec2-catalog-describe-instance-types-a1-c9g step task#9 and diff every instanceTypeSet item against the recording (memoryInfo.sizeInMiB, processorInfo.sustainedClockSpeedInGhz, networkPerformance strings, networkCards baseline/peak bandwidth and defaultEnaQueueCountPerInterface, ebsOptimizedInfo iops/bandwidth/throughput, vCpuInfo.validCores, hibernationSupported); then re-replay every other ec2-catalog-describe-instance-types-* page to confirm all 95+ clusters clear.
3. Cause 2: replay ec2-catalog-describe-instance-type-offerings step task#27 (and task#30, #53, #91, #93, #96, #98, #100, #102, #103, #144, #146, #149, #151, #152, #153) and confirm the response contains no instance type the recording does not offer in that location.
4. Cause 3: replay ec2-catalog-get-vpn-connection-device-types step task#1 and diff the whole vpnConnectionDeviceTypeSet (ids, vendor, platform, software and order) against the recording.
5. All three causes must pass before the issue is closed.
