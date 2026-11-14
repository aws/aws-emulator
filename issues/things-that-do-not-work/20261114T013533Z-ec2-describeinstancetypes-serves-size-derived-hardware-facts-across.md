Issue-Source: trace-sweep-divergence
Issue-Trace: ec2-catalog-describe-instance-types-a1-c9g
Issue-Step: task#5
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ec2-catalog-describe-instance-types-a1-c9g ec2-catalog-describe-instance-types-c9gd-m6gd ec2-catalog-describe-instance-types-m6i-r4
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5.18xlarge"].vCpuInfo.validCores.item[*]
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5.metal"].ebsInfo.maximumEbsAttachments
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5.metal"].networkInfo.trafficMirroringSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.12xlarge"].vCpuInfo.validCores.item[*]
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.16xlarge"].vCpuInfo.validCores.item[*]
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.24xlarge"].vCpuInfo.validCores.item[*]
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.4xlarge"].ebsInfo.ebsOptimizedInfo.baselineIops
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.4xlarge"].vCpuInfo.validCores.item[*]
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.8xlarge"].vCpuInfo.validCores.item[*]
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="gr6.4xlarge"].networkInfo.networkCards.item.baselineBandwidthInGbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="gr6.4xlarge"].networkInfo.networkCards.item.defaultEnaQueueCountPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="gr6.4xlarge"].networkInfo.networkCards.item.networkPerformance
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="gr6.4xlarge"].networkInfo.networkCards.item.peakBandwidthInGbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="gr6.8xlarge"].networkInfo.networkCards.item.defaultEnaQueueCountPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="gr6.8xlarge"].networkInfo.networkCards.item.interfaceTypeSet.item
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r4.16xlarge"].networkInfo.networkCards.item.defaultEnaQueueCountPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r4.2xlarge"].networkInfo.networkCards.item.baselineBandwidthInGbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r4.2xlarge"].networkInfo.networkCards.item.defaultEnaQueueCountPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r4.4xlarge"].networkInfo.networkCards.item.baselineBandwidthInGbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r4.4xlarge"].networkInfo.networkCards.item.defaultEnaQueueCountPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r4.8xlarge"].networkInfo.networkCards.item.baselineBandwidthInGbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r4.8xlarge"].networkInfo.networkCards.item.defaultEnaQueueCountPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r4.8xlarge"].networkInfo.networkCards.item.networkPerformance
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r4.8xlarge"].networkInfo.networkCards.item.peakBandwidthInGbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r4.large"].networkInfo.networkCards.item.baselineBandwidthInGbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r4.large"].networkInfo.networkCards.item.defaultEnaQueueCountPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r4.xlarge"].networkInfo.networkCards.item.baselineBandwidthInGbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r4.xlarge"].networkInfo.networkCards.item.defaultEnaQueueCountPerInterface
Issue-Related: 20261104T201311Z-ec2-instance-type-catalog-is-synthesized-per-family.md
Issue-Related: 20261108T044413Z-ec2-describeinstancetypes-per-type-attributes-still-diverge-on-the.md
Issue-Related: 20261112T211315Z-ec2-describeinstancetypes-catalog-pages-serve-synthesized-per-type.md
Issue-Related: 20261112T231741Z-ec2-describeinstancetypes-catalog-rows-are-synthesized-from-a.md
Issue-Commit: 588eab0496abf0a5bc1b8cec7adc2c989702bf8c

# ec2 DescribeInstanceTypes serves size-derived hardware facts across every catalog page (1 cause, 153 clusters)

> _Note: this issue was identified against the code base at commit `588eab0496abf0a5bc1b8cec7adc2c989702bf8c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeInstanceTypes

## Description
Cause 1 of 1 (the single root cause behind 153 net-new divergence clusters): the ec2 instance-type catalog behind DescribeInstanceTypes is computed from the type's name/size instead of carrying the real per-type hardware facts, and it omits several modeled members outright. The same field groups diverge on every catalog page replayed (a1-c9g, c9gd-m6gd, m6i-r4 and the other pages of the ec2-catalog-describe-instance-types-* family), so one data/serializer fix closes all of them - but it must be re-replayed against every page, not only the fixtures named below.

AWS: recorded behavior only - the per-type attribute values are catalog data, not model constraints (the model only fixes the member set/shape of InstanceTypeInfo). DescribeInstanceTypes answers HTTP 200 and each instanceTypeSet item reports that instance type's published facts: networkInfo.networkCards.item.baselineBandwidthInGbps / peakBandwidthInGbps and networkInfo.networkPerformance, networkInfo.networkCards.item.defaultEnaQueueCountPerInterface, networkInfo.enaSrdSupported, networkInfo.ipv4AddressesPerSecondaryInterface, networkInfo.trafficMirroringSupport, networkInfo.efaInfo/efaSupported, ebsInfo.attachmentLimitType, ebsInfo.maximumEbsAttachments and ebsInfo.ebsOptimizedInfo.{baseline,maximum}{BandwidthInMbps,Iops,ThroughputInMBps}, memoryInfo.sizeInMiB, processorInfo.manufacturer and sustainedClockSpeedInGhz, vCpuInfo.validCores, currentGeneration, autoRecoverySupported, dedicatedHostsSupported, rebootMigrationSupport, freeTierEligible, instanceStorageSupported/instanceStorageInfo and gpuInfo for accelerated types.
Emulator: those values are derived from the type's size tier rather than looked up, so they diverge for most types (e.g. baselineBandwidthInGbps reported as a fraction of the peak instead of the published figure; ebsOptimizedInfo baseline/maximum bandwidth, iops and throughput scaled from the size; maximumEbsAttachments a constant; memoryInfo.sizeInMiB rounded to a power of two; sustainedClockSpeedInGhz, processorInfo.manufacturer, currentGeneration, autoRecoverySupported, dedicatedHostsSupported, rebootMigrationSupport, attachmentLimitType and freeTierEligible flipped; vCpuInfo.validCores enumerated by a stride that is not the type's), and several members are not serialized at all (networkCards.item.defaultEnaQueueCountPerInterface, networkInfo.trafficMirroringSupport, networkInfo.enaSrdSupported, gpuInfo and instanceStorageInfo for the types that have them).
Why: instance-type selection, EBS/ENI attachment planning, EFA/GPU capability checks and free-tier gating all read these fields; a client sized against the emulator picks types that cannot host its workload, and code reading an omitted member gets null where AWS always serves a value.
Fix: the ec2 instance-type catalog data behind DescribeInstanceTypes (per-type records replacing the size-derived computation) and the InstanceTypeInfo serializer for the members it never emits.
Traces: ec2-catalog-describe-instance-types-a1-c9g#task#5 and #task#6, ec2-catalog-describe-instance-types-c9gd-m6gd#task#15, ec2-catalog-describe-instance-types-m6i-r4#task#44; 153 clusters in total across the ec2-catalog-describe-instance-types-* fixtures.

Related open issues that do NOT carry these replay keys (they name other pages/steps, so these clusters stay unrun under them): 20261111T001230Z-ec2-describeinstancetypes-rows-for-m5zn-m6a-and-m6i.md, 20261111T015735Z-ec2-describeinstancetypes-catalog-pages-c9gd-m6gd-and-m6i.md, 20261112T182924Z-ec2-describeinstancetypes-rows-for-the-c4-c9gd-m6i.md, 20261112T211315Z-ec2-describeinstancetypes-catalog-pages-serve-synthesized-per-type.md, 20261112T231741Z-ec2-describeinstancetypes-catalog-rows-are-synthesized-from-a.md.

## Evidence
Replay of trace set simparity-wire-aws-v1: 153 divergence clusters with divergence_key prefix 'service=ec2|DescribeInstanceTypes|status=200->200', all verdict=mismatch, stub_path empty, owned_by empty. Representative details:
- ec2-catalog-describe-instance-types-a1-c9g#task#5: c5.2xlarge networkCards.item.baselineBandwidthInGbps expected '2.5', got '2.08'; c5.18xlarge peakBandwidthInGbps expected '25.0', got '18.75'; defaultEnaQueueCountPerInterface missing (expected '8', got None) on every c5 size; c5.18xlarge vCpuInfo.validCores expected 4,6,8,10,12,14,... got 2,4,6,8,10,12,...; c5.metal ebsInfo.maximumEbsAttachments and networkInfo.trafficMirroringSupport diverge.
- ec2-catalog-describe-instance-types-a1-c9g#task#6: c5a.* baselineBandwidthInGbps expected '1.25'/'20.0', got '1.04'/'16.66'; validCores list shifted; defaultEnaQueueCountPerInterface missing.
- ec2-catalog-describe-instance-types-c9gd-m6gd#task#15: gr6.4xlarge autoRecoverySupported expected 'false' got 'true'; dedicatedHostsSupported expected 'false' got 'true'; ebsInfo.attachmentLimitType expected 'dedicated' got 'shared'; ebsOptimizedInfo.baselineBandwidthInMbps expected '8000' got '4600'; baselineIops expected '32000' got '18750'; maximumThroughputInMBps expected '1000.0' got '593.75'; maximumEbsAttachments expected '32' got '27'; gpuInfo missing (expected NVIDIA L4 gpus item, got None); instanceStorageInfo/instanceStorageSupported, enaSrdSupported, ipv4AddressesPerSecondaryInterface, processorInfo.manufacturer and rebootMigrationSupport also diverge.
- ec2-catalog-describe-instance-types-m6i-r4#task#44: r4.xlarge currentGeneration expected 'false' got 'true'; attachmentLimitType expected 'shared' got 'dedicated'; ebsOptimizedInfo.baselineBandwidthInMbps expected '850' got '1150'; memoryInfo.sizeInMiB expected '31232' got '32768'; sustainedClockSpeedInGhz expected '2.3' got '2.4'; rebootMigrationSupport expected 'supported' got 'unsupported'; defaultEnaQueueCountPerInterface missing.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1.
2. Replay fixture ec2-catalog-describe-instance-types-a1-c9g step task#5 (DescribeInstanceTypes over the c5 family) and diff each instanceTypeSet item against the recorded response: network card bandwidths, defaultEnaQueueCountPerInterface, vCpuInfo.validCores, ebsInfo members.
3. Repeat for ec2-catalog-describe-instance-types-a1-c9g#task#6, ec2-catalog-describe-instance-types-c9gd-m6gd#task#15 and ec2-catalog-describe-instance-types-m6i-r4#task#44, then re-replay the whole ec2-catalog-describe-instance-types-* fixture family to confirm all 153 clusters clear.
