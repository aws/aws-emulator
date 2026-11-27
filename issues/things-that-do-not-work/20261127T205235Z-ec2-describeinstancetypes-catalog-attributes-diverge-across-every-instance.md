Issue-Source: trace-sweep-divergence
Issue-Trace: ec2-account-and-instance-type-metadata
Issue-Step: task#3
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ec2-account-and-instance-type-metadata ec2-catalog-describe-instance-types-a1-c9g ec2-catalog-describe-instance-types-m6i-r4 ec2-catalog-describe-instance-types-r9g-z1d
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item.networkInfo.networkCards.item.baselineBandwidthInGbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item.networkInfo.networkCards.item.defaultEnaQueueCountPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item.networkInfo.networkCards.item.peakBandwidthInGbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item.rebootMigrationSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="a1.2xlarge"].networkInfo.networkCards.item.defaultEnaQueueCountPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="a1.4xlarge"].networkInfo.networkCards.item.defaultEnaQueueCountPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="a1.large"].networkInfo.networkCards.item.defaultEnaQueueCountPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="a1.medium"].networkInfo.networkCards.item.defaultEnaQueueCountPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="a1.metal"].networkInfo.networkCards.item.defaultEnaQueueCountPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="a1.xlarge"].networkInfo.networkCards.item.defaultEnaQueueCountPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c1.medium"].networkInfo.networkCards.item.baselineBandwidthInGbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c1.medium"].networkInfo.networkCards.item.peakBandwidthInGbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c1.xlarge"].networkInfo.networkCards.item.baselineBandwidthInGbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c1.xlarge"].networkInfo.networkCards.item.defaultEnaQueueCountPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c3.2xlarge"].networkInfo.networkCards.item.baselineBandwidthInGbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c3.2xlarge"].networkInfo.networkCards.item.defaultEnaQueueCountPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c3.4xlarge"].networkInfo.networkCards.item.baselineBandwidthInGbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c3.4xlarge"].networkInfo.networkCards.item.defaultEnaQueueCountPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c3.8xlarge"].networkInfo.networkCards.item.defaultEnaQueueCountPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c3.large"].networkInfo.networkCards.item.baselineBandwidthInGbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c3.large"].networkInfo.networkCards.item.defaultEnaQueueCountPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c3.large"].networkInfo.networkCards.item.peakBandwidthInGbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c3.xlarge"].networkInfo.networkCards.item.baselineBandwidthInGbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c3.xlarge"].networkInfo.networkCards.item.defaultEnaQueueCountPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c4.large"].networkInfo.networkCards.item.peakBandwidthInGbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c4.xlarge"].networkInfo.networkCards.item.peakBandwidthInGbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item.networkInfo.trafficMirroringSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item.phcSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item.processorInfo.sustainedClockSpeedInGhz
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item.ebsInfo.maximumEbsAttachments
Issue-Related: 20261029T215142Z-ec2-describe-projections-omit-modeled-members-and-seed.md
Issue-Related: 20261030T124526Z-ec2-describeinstancetypes-serves-a-synthesized-instance-type-catalog.md
Issue-Related: 20261120T081949Z-ec2-instance-type-and-fpga-image-catalog-reads.md
Issue-Commit: ba97c8568f94e8f0e1657393f851b27fdc51a8e0

# ec2 DescribeInstanceTypes catalog attributes diverge across every instance family

> _Note: this issue was identified against the code base at commit `ba97c8568f94e8f0e1657393f851b27fdc51a8e0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeInstanceTypes

## Description
DescribeInstanceTypes answers 200 but the instance-type catalog behind it is computed from per-family heuristics instead of being served from AWS's recorded instance-type data, so nearly every attribute group diverges and some modeled members are omitted entirely. The replay clusters this into ~95 divergence clusters spanning the whole ec2-catalog-describe-instance-types-* family (lines covering a1 through z1d) plus the account-metadata fixture; the 7 causes below are the ones this issue binds for replay, grouped by attribute family. This is a BUNDLE OF 7 CAUSES; all seven must be closed, and closing them should be verified by re-replaying the whole ec2-catalog-describe-instance-types-* set, because the remaining clusters of this same root cause are not individually listed here.

AWS (applies to all causes): recorded behavior only — the Smithy model defines the InstanceTypeInfo member set but not the per-type values. AWS serves, per instance type, the authoritative values for networkInfo (networkCards.item.baselineBandwidthInGbps / peakBandwidthInGbps / defaultEnaQueueCountPerInterface, ipv6Supported, ipv6AddressesPerInterface, trafficMirroringSupport), ebsInfo (maximumEbsAttachments, encryptionSupport, ebsOptimizedSupport and the ebsOptimizedInfo sub-structure), vCpuInfo (defaultVCpus, defaultCores, defaultThreadsPerCore), processorInfo.sustainedClockSpeedInGhz, placementGroupInfo.supportedStrategies, supportedVirtualizationTypes, supportedBootModes, autoRecoverySupported, rebootMigrationSupport and phcSupport. Members AWS always emits for a type (for example networkCards.item.defaultEnaQueueCountPerInterface and processorInfo.sustainedClockSpeedInGhz) must be present, and list members (supportedVirtualizationTypes.item, placementGroupInfo.supportedStrategies.item, supportedBootModes.item) must carry every value AWS reports, not one.

Cause 1 of 7: the single-type describe in the account-metadata trajectory — networkCards baselineBandwidthInGbps/peakBandwidthInGbps wrong, defaultEnaQueueCountPerInterface omitted, rebootMigrationSupport wrong.
Emulator: baseline 0.78 vs 0.064, peak 10.0 vs 5.0, defaultEnaQueueCountPerInterface absent, rebootMigrationSupport 'unsupported' vs 'supported'.
Traces: ec2-account-and-instance-type-metadata#task#3.

Cause 2 of 7: the a1 family — defaultEnaQueueCountPerInterface omitted for every size, and a1.metal's autoRecoverySupported, ebsInfo.maximumEbsAttachments, networkInfo.trafficMirroringSupport and rebootMigrationSupport wrong.
Traces: ec2-catalog-describe-instance-types-a1-c9g#task#1.

Cause 3 of 7: the c1 family — ebsInfo.encryptionSupport/ebsOptimizedSupport wrong, ebsOptimizedInfo omitted, vCpuInfo.defaultVCpus/defaultCores/defaultThreadsPerCore wrong, supportedVirtualizationTypes collapsed to one value, network bandwidths and ENA queue counts wrong.
Traces: ec2-catalog-describe-instance-types-a1-c9g#task#2.

Cause 4 of 7: the c3 family — networkCards baselineBandwidthInGbps/peakBandwidthInGbps and defaultEnaQueueCountPerInterface wrong for every size.
Traces: ec2-catalog-describe-instance-types-a1-c9g#task#3.

Cause 5 of 7: the c4 family — networkInfo.ipv6Supported reported 'false' and ipv6AddressesPerInterface 0 for types AWS reports as IPv6-capable with a per-size address count, plus wrong peakBandwidthInGbps.
Traces: ec2-catalog-describe-instance-types-a1-c9g#task#4.

Cause 6 of 7: trafficMirroringSupport/phcSupport inverted, placementGroupInfo.supportedStrategies served empty where AWS lists partition+spread, processorInfo.sustainedClockSpeedInGhz omitted, supportedBootModes wrong.
Traces: ec2-catalog-describe-instance-types-m6i-r4#task#28.

Cause 7 of 7: ebsInfo.maximumEbsAttachments wrong, trafficMirroringSupport/rebootMigrationSupport inverted, supportedVirtualizationTypes collapsed to a single value where AWS lists hvm+paravirtual.
Traces: ec2-catalog-describe-instance-types-r9g-z1d#task#3.

Why (all causes): DescribeInstanceTypes is how a client chooses an instance type — ENA queue counts and network bandwidth drive network sizing, ipv6Supported gates dual-stack launches, maximumEbsAttachments gates volume attachment, placementGroupInfo.supportedStrategies gates placement-group creation, and an omitted member makes an SDK field null. Every one of those decisions is made on invented data, and an omitted member can crash a client that treats it as always present.
Fix: the instance-type catalog data source behind the DescribeInstanceTypes handler in the ec2 crate — serve the recorded per-type attributes rather than deriving them from family/size heuristics, and emit the full modeled member set including list members with all their values.

Related open issues (neither carries these replay keys): 20261124T131456Z-ec2-describeinstancetypes-serves-a-locally-minted-instance-type.md and 20261124T114924Z-ec2-describeinstancetypes-validcores-ena-queue-counts-and-efa.md.

## Evidence
Cause 1 — 200/200: networkCards.item.baselineBandwidthInGbps expected '0.064' got '0.78'; defaultEnaQueueCountPerInterface expected '2' got None; peakBandwidthInGbps expected '5.0' got '10.0'; rebootMigrationSupport expected 'supported' got 'unsupported'.
Cause 2 — 200/200: defaultEnaQueueCountPerInterface expected '8' got None for a1.medium/large/xlarge/2xlarge/4xlarge/metal; a1.metal autoRecoverySupported expected 'true' got 'false'; maximumEbsAttachments expected '31' got '27'; trafficMirroringSupport expected 'supported' got 'unsupported'; rebootMigrationSupport expected 'supported' got 'unsupported'.
Cause 3 — 200/200: c1.medium encryptionSupport expected 'supported' got 'unsupported'; baselineBandwidthInGbps expected '0.3' got '0.5'; peakBandwidthInGbps expected '1.2' got '10.0'; supportedVirtualizationTypes.item expected ['hvm','paravirtual'] got 'paravirtual'; vCpuInfo.defaultCores/defaultVCpus expected '2' got '1'; c1.xlarge ebsOptimizedInfo missing (expected baselineBandwidthInMbps '1000', baselineIops '8000', baselineThroughputInMBps '125.0', ...); ebsOptimizedSupport expected 'supported' got 'unsupported'; defaultEnaQueueCountPerInterface expected '4' got '1'.
Cause 4 — 200/200: c3.* baselineBandwidthInGbps expected '0.5'/'0.7'/'1.0'/'2.0' got '0.75'/'1.25'/'2.5'/'5.0'; defaultEnaQueueCountPerInterface expected '8' got '1'; c3.large peakBandwidthInGbps expected '1.2' got '10.0'.
Cause 5 — 200/200: c4.large/xlarge/2xlarge/4xlarge/8xlarge ipv6Supported expected 'true' got 'false'; ipv6AddressesPerInterface expected '10'/'15'/'15'/'30'/'30' got '0'; peakBandwidthInGbps expected '1.2'/'2.8' got '10.0'.
Cause 6 — 200/200: trafficMirroringSupport expected 'unsupported' got 'supported'; phcSupport expected 'supported' got 'unsupported'; placementGroupInfo.supportedStrategies expected {'item': ['partition','spread']} got ''; processorInfo.sustainedClockSpeedInGhz expected '4.0' got None; supportedBootModes.item expected 'legacy-bios' got 'uefi'.
Cause 7 — 200/200: ebsInfo.maximumEbsAttachments expected '39' got '27'; trafficMirroringSupport expected 'unsupported' got 'supported'; rebootMigrationSupport expected 'supported' got 'unsupported'; supportedVirtualizationTypes.item expected ['hvm','paravirtual'] got 'paravirtual'.
The per-type numbers quoted are the recorded catalog values for those types; the requirement is that the served catalog matches AWS's per-type data and member set, not these literals in isolation.

## Steps to Reproduce
1. Boot the emulator and replay the wire corpus (trace set simparity-wire-aws-v1).
2. Cause 1: ec2-account-and-instance-type-metadata step task#3.
3. Causes 2-5: ec2-catalog-describe-instance-types-a1-c9g steps task#1 (a1), task#2 (c1), task#3 (c3), task#4 (c4).
4. Cause 6: ec2-catalog-describe-instance-types-m6i-r4 step task#28.
5. Cause 7: ec2-catalog-describe-instance-types-r9g-z1d step task#3.
6. For each, diff instanceTypeSet.item against the fixture: networkInfo (bandwidths, ENA queue counts, ipv6*), ebsInfo, vCpuInfo, processorInfo, placementGroupInfo, supportedVirtualizationTypes/BootModes, autoRecoverySupported, rebootMigrationSupport, phcSupport, trafficMirroringSupport.
7. After fixing, re-replay the entire ec2-catalog-describe-instance-types-* fixture family — roughly 95 clusters share this root cause.
