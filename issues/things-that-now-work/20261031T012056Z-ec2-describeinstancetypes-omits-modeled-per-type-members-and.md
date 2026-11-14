Issue-Source: trace-sweep-divergence
Issue-Trace: ec2-catalog-describe-instance-types-a1-c9g
Issue-Step: task#6
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ec2-catalog-describe-instance-types-a1-c9g ec2-catalog-describe-instance-types-m6i-r4 ec2-catalog-describe-instance-types-r5-r8in
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.12xlarge"].cpuBurstModel
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.12xlarge"].dedicatedHostsSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.12xlarge"].ebsInfo.ebsOptimizedInfo.baselineBandwidthInMbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.12xlarge"].ebsInfo.ebsOptimizedInfo.baselineIops
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.12xlarge"].ebsInfo.ebsOptimizedInfo.baselineThroughputInMBps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.12xlarge"].ebsInfo.ebsOptimizedInfo.maximumBandwidthInMbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.12xlarge"].ebsInfo.ebsOptimizedInfo.maximumIops
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.12xlarge"].ebsInfo.ebsOptimizedInfo.maximumThroughputInMBps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.12xlarge"].ebsInfo.maximumEbsAttachments
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.12xlarge"].hibernationSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.12xlarge"].networkInfo.defaultNetworkCardIndex
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.12xlarge"].networkInfo.enaSrdSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.12xlarge"].networkInfo.flexibleEnaQueuesSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.12xlarge"].networkInfo.maximumNetworkCards
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.12xlarge"].networkInfo.networkCards
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.12xlarge"].networkInfo.networkPerformance
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.12xlarge"].networkInfo.secondaryNetworkSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.12xlarge"].networkInfo.trafficMirroringSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.12xlarge"].phcSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.12xlarge"].processorInfo.sustainedClockSpeedInGhz
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.12xlarge"].rebootMigrationSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.12xlarge"].vCpuInfo.validCores.item[*]
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.16xlarge"].cpuBurstModel
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.16xlarge"].dedicatedHostsSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.16xlarge"].ebsInfo.ebsOptimizedInfo.baselineBandwidthInMbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.16xlarge"].ebsInfo.ebsOptimizedInfo.baselineIops
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.16xlarge"].ebsInfo.ebsOptimizedInfo.baselineThroughputInMBps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.16xlarge"].ebsInfo.ebsOptimizedInfo.maximumBandwidthInMbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.16xlarge"].ebsInfo.ebsOptimizedInfo.maximumIops
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.16xlarge"].ebsInfo.ebsOptimizedInfo.maximumThroughputInMBps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.16xlarge"].ebsInfo.maximumEbsAttachments
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.16xlarge"].hibernationSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.16xlarge"].networkInfo.defaultNetworkCardIndex
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.16xlarge"].networkInfo.enaSrdSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.16xlarge"].networkInfo.flexibleEnaQueuesSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.16xlarge"].networkInfo.maximumNetworkCards
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.16xlarge"].networkInfo.networkCards
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.16xlarge"].networkInfo.networkPerformance
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.16xlarge"].networkInfo.secondaryNetworkSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.16xlarge"].networkInfo.trafficMirroringSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.16xlarge"].phcSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.16xlarge"].processorInfo.sustainedClockSpeedInGhz
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.16xlarge"].rebootMigrationSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.16xlarge"].vCpuInfo.validCores.item[*]
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.24xlarge"].cpuBurstModel
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.24xlarge"].dedicatedHostsSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.24xlarge"].ebsInfo.ebsOptimizedInfo.baselineBandwidthInMbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.24xlarge"].ebsInfo.ebsOptimizedInfo.baselineIops
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.24xlarge"].ebsInfo.ebsOptimizedInfo.baselineThroughputInMBps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.24xlarge"].ebsInfo.ebsOptimizedInfo.maximumBandwidthInMbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.24xlarge"].ebsInfo.ebsOptimizedInfo.maximumIops
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.24xlarge"].ebsInfo.ebsOptimizedInfo.maximumThroughputInMBps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.24xlarge"].ebsInfo.maximumEbsAttachments
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.24xlarge"].networkInfo.defaultNetworkCardIndex
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.24xlarge"].networkInfo.enaSrdSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.24xlarge"].networkInfo.flexibleEnaQueuesSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.24xlarge"].networkInfo.maximumNetworkCards
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.24xlarge"].networkInfo.networkCards
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.24xlarge"].networkInfo.networkPerformance
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.24xlarge"].networkInfo.secondaryNetworkSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.24xlarge"].networkInfo.trafficMirroringSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.24xlarge"].phcSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.24xlarge"].processorInfo.sustainedClockSpeedInGhz
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.24xlarge"].rebootMigrationSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.24xlarge"].vCpuInfo.validCores.item[*]
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.2xlarge"].cpuBurstModel
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.2xlarge"].dedicatedHostsSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.2xlarge"].ebsInfo.ebsOptimizedInfo.baselineBandwidthInMbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.2xlarge"].ebsInfo.ebsOptimizedInfo.baselineIops
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.2xlarge"].ebsInfo.ebsOptimizedInfo.baselineThroughputInMBps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.2xlarge"].ebsInfo.ebsOptimizedInfo.maximumBandwidthInMbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.2xlarge"].ebsInfo.ebsOptimizedInfo.maximumIops
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.2xlarge"].ebsInfo.ebsOptimizedInfo.maximumThroughputInMBps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.2xlarge"].ebsInfo.maximumEbsAttachments
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.2xlarge"].hibernationSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.2xlarge"].networkInfo.defaultNetworkCardIndex
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.2xlarge"].networkInfo.enaSrdSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.2xlarge"].networkInfo.flexibleEnaQueuesSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.2xlarge"].networkInfo.maximumNetworkCards
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.2xlarge"].networkInfo.networkCards
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.2xlarge"].networkInfo.networkPerformance
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.2xlarge"].networkInfo.secondaryNetworkSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.2xlarge"].networkInfo.trafficMirroringSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.2xlarge"].phcSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.2xlarge"].processorInfo.sustainedClockSpeedInGhz
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.2xlarge"].rebootMigrationSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.4xlarge"].cpuBurstModel
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.4xlarge"].dedicatedHostsSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.4xlarge"].ebsInfo.ebsOptimizedInfo.baselineBandwidthInMbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.4xlarge"].ebsInfo.ebsOptimizedInfo.baselineIops
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.4xlarge"].ebsInfo.ebsOptimizedInfo.baselineThroughputInMBps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.4xlarge"].ebsInfo.ebsOptimizedInfo.maximumBandwidthInMbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.4xlarge"].ebsInfo.ebsOptimizedInfo.maximumIops
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.4xlarge"].ebsInfo.ebsOptimizedInfo.maximumThroughputInMBps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.4xlarge"].ebsInfo.maximumEbsAttachments
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.4xlarge"].hibernationSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.4xlarge"].networkInfo.defaultNetworkCardIndex
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.4xlarge"].networkInfo.enaSrdSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.4xlarge"].networkInfo.flexibleEnaQueuesSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.4xlarge"].networkInfo.maximumNetworkCards
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.4xlarge"].networkInfo.networkCards
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.4xlarge"].networkInfo.networkPerformance
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.4xlarge"].networkInfo.secondaryNetworkSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.4xlarge"].networkInfo.trafficMirroringSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.4xlarge"].phcSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.4xlarge"].processorInfo.sustainedClockSpeedInGhz
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.4xlarge"].rebootMigrationSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.4xlarge"].vCpuInfo.validCores.item[*]
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.8xlarge"].cpuBurstModel
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.8xlarge"].dedicatedHostsSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.8xlarge"].ebsInfo.ebsOptimizedInfo.baselineBandwidthInMbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.8xlarge"].ebsInfo.ebsOptimizedInfo.baselineIops
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.8xlarge"].ebsInfo.ebsOptimizedInfo.baselineThroughputInMBps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.8xlarge"].ebsInfo.ebsOptimizedInfo.maximumBandwidthInMbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.8xlarge"].ebsInfo.ebsOptimizedInfo.maximumIops
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.8xlarge"].ebsInfo.ebsOptimizedInfo.maximumThroughputInMBps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.8xlarge"].ebsInfo.maximumEbsAttachments
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.8xlarge"].hibernationSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.8xlarge"].networkInfo.defaultNetworkCardIndex
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.8xlarge"].networkInfo.enaSrdSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.8xlarge"].networkInfo.flexibleEnaQueuesSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.8xlarge"].networkInfo.maximumNetworkCards
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.8xlarge"].networkInfo.networkCards
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.8xlarge"].networkInfo.networkPerformance
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.8xlarge"].networkInfo.secondaryNetworkSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.8xlarge"].networkInfo.trafficMirroringSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.8xlarge"].phcSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.8xlarge"].processorInfo.sustainedClockSpeedInGhz
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.8xlarge"].rebootMigrationSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.8xlarge"].vCpuInfo.validCores.item[*]
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.large"].cpuBurstModel
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.large"].dedicatedHostsSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.large"].ebsInfo.ebsOptimizedInfo.baselineBandwidthInMbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.large"].ebsInfo.ebsOptimizedInfo.baselineIops
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.large"].ebsInfo.ebsOptimizedInfo.baselineThroughputInMBps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.large"].ebsInfo.ebsOptimizedInfo.maximumBandwidthInMbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.large"].ebsInfo.ebsOptimizedInfo.maximumIops
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.large"].ebsInfo.ebsOptimizedInfo.maximumThroughputInMBps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.large"].ebsInfo.maximumEbsAttachments
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.large"].hibernationSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.large"].networkInfo.defaultNetworkCardIndex
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.large"].networkInfo.enaSrdSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.large"].networkInfo.flexibleEnaQueuesSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.large"].networkInfo.maximumNetworkCards
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.large"].networkInfo.networkCards
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.large"].networkInfo.networkPerformance
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.large"].networkInfo.secondaryNetworkSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.large"].networkInfo.trafficMirroringSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.large"].phcSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.large"].processorInfo.sustainedClockSpeedInGhz
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.large"].rebootMigrationSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.xlarge"].cpuBurstModel
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.xlarge"].dedicatedHostsSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.xlarge"].ebsInfo.ebsOptimizedInfo.baselineBandwidthInMbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.xlarge"].ebsInfo.ebsOptimizedInfo.baselineIops
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.xlarge"].ebsInfo.ebsOptimizedInfo.baselineThroughputInMBps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.xlarge"].ebsInfo.ebsOptimizedInfo.maximumBandwidthInMbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.xlarge"].ebsInfo.ebsOptimizedInfo.maximumIops
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.xlarge"].ebsInfo.ebsOptimizedInfo.maximumThroughputInMBps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.xlarge"].ebsInfo.maximumEbsAttachments
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.xlarge"].hibernationSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.xlarge"].networkInfo.defaultNetworkCardIndex
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.xlarge"].networkInfo.enaSrdSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.xlarge"].networkInfo.flexibleEnaQueuesSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.xlarge"].networkInfo.maximumNetworkCards
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.xlarge"].networkInfo.networkCards
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.xlarge"].networkInfo.networkPerformance
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.xlarge"].networkInfo.secondaryNetworkSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.xlarge"].networkInfo.trafficMirroringSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.xlarge"].phcSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.xlarge"].processorInfo.sustainedClockSpeedInGhz
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.xlarge"].rebootMigrationSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item.instanceStorageInfo.disks.item.count
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item.instanceStorageInfo.disks.item.sizeInGB
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item.instanceStorageInfo.totalSizeInGB
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item.networkInfo.connectionTrackingConfiguration.defaultTcpEstablishedTimeout
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item.networkInfo.efaInfo.maximumEfaInterfaces
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item.nitroEnclavesSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item.supportedUsageClasses.item[*]
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.12xlarge"].cpuBurstModel
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.12xlarge"].ebsInfo.attachmentLimitType
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.12xlarge"].ebsInfo.ebsOptimizedInfo.baselineBandwidthInMbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.12xlarge"].ebsInfo.ebsOptimizedInfo.baselineIops
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.12xlarge"].ebsInfo.ebsOptimizedInfo.baselineThroughputInMBps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.12xlarge"].ebsInfo.ebsOptimizedInfo.maximumBandwidthInMbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.12xlarge"].ebsInfo.ebsOptimizedInfo.maximumIops
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.12xlarge"].ebsInfo.ebsOptimizedInfo.maximumThroughputInMBps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.12xlarge"].ebsInfo.maximumEbsAttachments
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.12xlarge"].networkInfo.connectionTrackingConfiguration.defaultTcpEstablishedTimeout
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.12xlarge"].networkInfo.defaultNetworkCardIndex
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.12xlarge"].networkInfo.enaSrdSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.12xlarge"].networkInfo.flexibleEnaQueuesSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.12xlarge"].networkInfo.ipv4AddressesPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.12xlarge"].networkInfo.ipv4AddressesPerSecondaryInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.12xlarge"].networkInfo.ipv6AddressesPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.12xlarge"].networkInfo.maximumNetworkCards
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.12xlarge"].networkInfo.maximumNetworkInterfaces
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.12xlarge"].networkInfo.networkCards
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.12xlarge"].networkInfo.networkPerformance
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.12xlarge"].networkInfo.secondaryNetworkSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.12xlarge"].networkInfo.trafficMirroringSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.12xlarge"].phcSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.12xlarge"].processorInfo.sustainedClockSpeedInGhz
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.12xlarge"].rebootMigrationSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.16xlarge"].cpuBurstModel
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.16xlarge"].ebsInfo.attachmentLimitType
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.16xlarge"].ebsInfo.ebsOptimizedInfo.baselineBandwidthInMbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.16xlarge"].ebsInfo.ebsOptimizedInfo.baselineIops
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.16xlarge"].ebsInfo.ebsOptimizedInfo.baselineThroughputInMBps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.16xlarge"].ebsInfo.ebsOptimizedInfo.maximumBandwidthInMbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.16xlarge"].ebsInfo.ebsOptimizedInfo.maximumIops
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.16xlarge"].ebsInfo.ebsOptimizedInfo.maximumThroughputInMBps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.16xlarge"].ebsInfo.maximumEbsAttachments
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.16xlarge"].networkInfo.connectionTrackingConfiguration.defaultTcpEstablishedTimeout
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.16xlarge"].networkInfo.defaultNetworkCardIndex
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.16xlarge"].networkInfo.enaSrdSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.16xlarge"].networkInfo.flexibleEnaQueuesSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.16xlarge"].networkInfo.ipv4AddressesPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.16xlarge"].networkInfo.ipv4AddressesPerSecondaryInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.16xlarge"].networkInfo.ipv6AddressesPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.16xlarge"].networkInfo.maximumNetworkCards
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.16xlarge"].networkInfo.maximumNetworkInterfaces
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.16xlarge"].networkInfo.networkCards
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.16xlarge"].networkInfo.networkPerformance
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.16xlarge"].networkInfo.secondaryNetworkSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.16xlarge"].networkInfo.trafficMirroringSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.16xlarge"].phcSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.16xlarge"].processorInfo.sustainedClockSpeedInGhz
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.16xlarge"].rebootMigrationSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.24xlarge"].cpuBurstModel
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.24xlarge"].ebsInfo.attachmentLimitType
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.24xlarge"].ebsInfo.ebsOptimizedInfo.baselineBandwidthInMbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.24xlarge"].ebsInfo.ebsOptimizedInfo.baselineIops
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.24xlarge"].ebsInfo.ebsOptimizedInfo.baselineThroughputInMBps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.24xlarge"].ebsInfo.ebsOptimizedInfo.maximumBandwidthInMbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.24xlarge"].ebsInfo.ebsOptimizedInfo.maximumIops
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.24xlarge"].ebsInfo.ebsOptimizedInfo.maximumThroughputInMBps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.24xlarge"].ebsInfo.maximumEbsAttachments
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.24xlarge"].networkInfo.connectionTrackingConfiguration.defaultTcpEstablishedTimeout
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.24xlarge"].networkInfo.defaultNetworkCardIndex
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.24xlarge"].networkInfo.enaSrdSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.24xlarge"].networkInfo.flexibleEnaQueuesSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.24xlarge"].networkInfo.ipv4AddressesPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.24xlarge"].networkInfo.ipv4AddressesPerSecondaryInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.24xlarge"].networkInfo.ipv6AddressesPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.24xlarge"].networkInfo.maximumNetworkCards
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.24xlarge"].networkInfo.maximumNetworkInterfaces
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.24xlarge"].networkInfo.networkCards
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.24xlarge"].networkInfo.networkPerformance
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.24xlarge"].networkInfo.secondaryNetworkSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.24xlarge"].networkInfo.trafficMirroringSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.24xlarge"].phcSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.24xlarge"].processorInfo.sustainedClockSpeedInGhz
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.24xlarge"].rebootMigrationSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.24xlarge"].vCpuInfo.validCores.item[*]
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.2xlarge"].cpuBurstModel
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.2xlarge"].ebsInfo.attachmentLimitType
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.2xlarge"].ebsInfo.ebsOptimizedInfo.baselineBandwidthInMbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.2xlarge"].ebsInfo.ebsOptimizedInfo.baselineIops
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.2xlarge"].ebsInfo.ebsOptimizedInfo.baselineThroughputInMBps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.2xlarge"].ebsInfo.ebsOptimizedInfo.maximumBandwidthInMbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.2xlarge"].ebsInfo.ebsOptimizedInfo.maximumIops
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.2xlarge"].ebsInfo.ebsOptimizedInfo.maximumThroughputInMBps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.2xlarge"].ebsInfo.maximumEbsAttachments
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.2xlarge"].networkInfo.connectionTrackingConfiguration.defaultTcpEstablishedTimeout
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.2xlarge"].networkInfo.defaultNetworkCardIndex
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.2xlarge"].networkInfo.enaSrdSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.2xlarge"].networkInfo.flexibleEnaQueuesSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.2xlarge"].networkInfo.ipv4AddressesPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.2xlarge"].networkInfo.ipv4AddressesPerSecondaryInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.2xlarge"].networkInfo.ipv6AddressesPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.2xlarge"].networkInfo.maximumNetworkCards
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.2xlarge"].networkInfo.networkCards
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.2xlarge"].networkInfo.networkPerformance
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.2xlarge"].networkInfo.secondaryNetworkSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.2xlarge"].networkInfo.trafficMirroringSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.2xlarge"].phcSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.2xlarge"].processorInfo.sustainedClockSpeedInGhz
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.2xlarge"].rebootMigrationSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.32xlarge"].cpuBurstModel
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.32xlarge"].ebsInfo.attachmentLimitType
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.32xlarge"].ebsInfo.ebsOptimizedInfo.baselineBandwidthInMbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.32xlarge"].ebsInfo.ebsOptimizedInfo.baselineIops
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.32xlarge"].ebsInfo.ebsOptimizedInfo.baselineThroughputInMBps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.32xlarge"].ebsInfo.ebsOptimizedInfo.maximumBandwidthInMbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.32xlarge"].ebsInfo.ebsOptimizedInfo.maximumIops
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.32xlarge"].ebsInfo.ebsOptimizedInfo.maximumThroughputInMBps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.32xlarge"].ebsInfo.maximumEbsAttachments
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.32xlarge"].networkInfo.connectionTrackingConfiguration.defaultTcpEstablishedTimeout
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.32xlarge"].networkInfo.defaultNetworkCardIndex
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.32xlarge"].networkInfo.enaSrdSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.32xlarge"].networkInfo.flexibleEnaQueuesSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.32xlarge"].networkInfo.ipv4AddressesPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.32xlarge"].networkInfo.ipv4AddressesPerSecondaryInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.32xlarge"].networkInfo.ipv6AddressesPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.32xlarge"].networkInfo.maximumNetworkCards
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.32xlarge"].networkInfo.maximumNetworkInterfaces
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.32xlarge"].networkInfo.networkCards
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.32xlarge"].networkInfo.networkPerformance
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.32xlarge"].networkInfo.secondaryNetworkSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.32xlarge"].networkInfo.trafficMirroringSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.32xlarge"].phcSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.32xlarge"].processorInfo.sustainedClockSpeedInGhz
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.32xlarge"].rebootMigrationSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.32xlarge"].vCpuInfo.validCores.item[*]
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.48xlarge"].cpuBurstModel
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.48xlarge"].ebsInfo.attachmentLimitType
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.48xlarge"].ebsInfo.ebsOptimizedInfo.baselineBandwidthInMbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.48xlarge"].ebsInfo.ebsOptimizedInfo.baselineIops
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.48xlarge"].ebsInfo.ebsOptimizedInfo.baselineThroughputInMBps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.48xlarge"].ebsInfo.ebsOptimizedInfo.maximumBandwidthInMbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.48xlarge"].ebsInfo.ebsOptimizedInfo.maximumIops
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.48xlarge"].ebsInfo.ebsOptimizedInfo.maximumThroughputInMBps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.48xlarge"].ebsInfo.maximumEbsAttachments
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.48xlarge"].networkInfo.connectionTrackingConfiguration.defaultTcpEstablishedTimeout
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.48xlarge"].networkInfo.defaultNetworkCardIndex
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.48xlarge"].networkInfo.efaInfo
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.48xlarge"].networkInfo.efaSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.48xlarge"].networkInfo.enaSrdSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.48xlarge"].networkInfo.flexibleEnaQueuesSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.48xlarge"].networkInfo.ipv4AddressesPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.48xlarge"].networkInfo.ipv4AddressesPerSecondaryInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.48xlarge"].networkInfo.ipv6AddressesPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.48xlarge"].networkInfo.maximumNetworkCards
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.48xlarge"].networkInfo.maximumNetworkInterfaces
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.48xlarge"].networkInfo.networkCards
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.48xlarge"].networkInfo.networkPerformance
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.48xlarge"].networkInfo.secondaryNetworkSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.48xlarge"].networkInfo.trafficMirroringSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.48xlarge"].phcSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.48xlarge"].processorInfo.sustainedClockSpeedInGhz
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.48xlarge"].rebootMigrationSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.48xlarge"].vCpuInfo.validCores.item[*]
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.4xlarge"].cpuBurstModel
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.4xlarge"].ebsInfo.attachmentLimitType
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.4xlarge"].ebsInfo.ebsOptimizedInfo.baselineBandwidthInMbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.4xlarge"].ebsInfo.ebsOptimizedInfo.baselineIops
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.4xlarge"].ebsInfo.ebsOptimizedInfo.baselineThroughputInMBps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.4xlarge"].ebsInfo.ebsOptimizedInfo.maximumBandwidthInMbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.4xlarge"].ebsInfo.ebsOptimizedInfo.maximumIops
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.4xlarge"].ebsInfo.ebsOptimizedInfo.maximumThroughputInMBps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.4xlarge"].ebsInfo.maximumEbsAttachments
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.4xlarge"].networkInfo.connectionTrackingConfiguration.defaultTcpEstablishedTimeout
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.4xlarge"].networkInfo.defaultNetworkCardIndex
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.4xlarge"].networkInfo.enaSrdSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.4xlarge"].networkInfo.flexibleEnaQueuesSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.4xlarge"].networkInfo.ipv4AddressesPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.4xlarge"].networkInfo.ipv4AddressesPerSecondaryInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.4xlarge"].networkInfo.ipv6AddressesPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.4xlarge"].networkInfo.maximumNetworkCards
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.4xlarge"].networkInfo.networkCards
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.4xlarge"].networkInfo.networkPerformance
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.4xlarge"].networkInfo.secondaryNetworkSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.4xlarge"].networkInfo.trafficMirroringSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.4xlarge"].phcSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.4xlarge"].processorInfo.sustainedClockSpeedInGhz
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.4xlarge"].rebootMigrationSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.8xlarge"].cpuBurstModel
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.8xlarge"].ebsInfo.attachmentLimitType
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.8xlarge"].ebsInfo.ebsOptimizedInfo.baselineBandwidthInMbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.8xlarge"].ebsInfo.ebsOptimizedInfo.baselineIops
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.8xlarge"].ebsInfo.ebsOptimizedInfo.baselineThroughputInMBps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.8xlarge"].ebsInfo.ebsOptimizedInfo.maximumBandwidthInMbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.8xlarge"].ebsInfo.ebsOptimizedInfo.maximumIops
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.8xlarge"].ebsInfo.ebsOptimizedInfo.maximumThroughputInMBps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.8xlarge"].ebsInfo.maximumEbsAttachments
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.8xlarge"].networkInfo.connectionTrackingConfiguration.defaultTcpEstablishedTimeout
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.8xlarge"].networkInfo.defaultNetworkCardIndex
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.8xlarge"].networkInfo.enaSrdSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.8xlarge"].networkInfo.flexibleEnaQueuesSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.8xlarge"].networkInfo.ipv4AddressesPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.8xlarge"].networkInfo.ipv4AddressesPerSecondaryInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.8xlarge"].networkInfo.ipv6AddressesPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.8xlarge"].networkInfo.maximumNetworkCards
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.8xlarge"].networkInfo.networkCards
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.8xlarge"].networkInfo.networkPerformance
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.8xlarge"].networkInfo.secondaryNetworkSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.8xlarge"].networkInfo.trafficMirroringSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.8xlarge"].phcSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.8xlarge"].processorInfo.sustainedClockSpeedInGhz
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.8xlarge"].rebootMigrationSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.96xlarge"].cpuBurstModel
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.96xlarge"].ebsInfo.attachmentLimitType
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.96xlarge"].ebsInfo.ebsCardSet
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.96xlarge"].ebsInfo.ebsOptimizedInfo.baselineBandwidthInMbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.96xlarge"].ebsInfo.ebsOptimizedInfo.baselineIops
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.96xlarge"].ebsInfo.ebsOptimizedInfo.baselineThroughputInMBps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.96xlarge"].ebsInfo.ebsOptimizedInfo.maximumBandwidthInMbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.96xlarge"].ebsInfo.ebsOptimizedInfo.maximumIops
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.96xlarge"].ebsInfo.ebsOptimizedInfo.maximumThroughputInMBps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.96xlarge"].ebsInfo.maximumEbsAttachments
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.96xlarge"].ebsInfo.maximumEbsCards
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.96xlarge"].networkInfo.connectionTrackingConfiguration.defaultTcpEstablishedTimeout
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.96xlarge"].networkInfo.defaultNetworkCardIndex
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.96xlarge"].networkInfo.efaInfo
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.96xlarge"].networkInfo.efaSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.96xlarge"].networkInfo.enaSrdSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.96xlarge"].networkInfo.flexibleEnaQueuesSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.96xlarge"].networkInfo.ipv4AddressesPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.96xlarge"].networkInfo.ipv4AddressesPerSecondaryInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.96xlarge"].networkInfo.ipv6AddressesPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.96xlarge"].networkInfo.maximumNetworkCards
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.96xlarge"].networkInfo.maximumNetworkInterfaces
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.96xlarge"].networkInfo.networkCards
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.96xlarge"].networkInfo.networkPerformance
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.96xlarge"].networkInfo.secondaryNetworkSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.96xlarge"].networkInfo.trafficMirroringSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.96xlarge"].phcSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.96xlarge"].processorInfo.sustainedClockSpeedInGhz
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.96xlarge"].rebootMigrationSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.96xlarge"].vCpuInfo.validCores.item[*]
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.large"].cpuBurstModel
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.large"].ebsInfo.attachmentLimitType
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.large"].ebsInfo.ebsOptimizedInfo.baselineBandwidthInMbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.large"].ebsInfo.ebsOptimizedInfo.baselineIops
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.large"].ebsInfo.ebsOptimizedInfo.baselineThroughputInMBps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.large"].ebsInfo.ebsOptimizedInfo.maximumBandwidthInMbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.large"].ebsInfo.ebsOptimizedInfo.maximumIops
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.large"].ebsInfo.ebsOptimizedInfo.maximumThroughputInMBps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.large"].ebsInfo.maximumEbsAttachments
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.large"].networkInfo.connectionTrackingConfiguration.defaultTcpEstablishedTimeout
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.large"].networkInfo.defaultNetworkCardIndex
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.large"].networkInfo.enaSrdSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.large"].networkInfo.flexibleEnaQueuesSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.large"].networkInfo.ipv4AddressesPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.large"].networkInfo.ipv4AddressesPerSecondaryInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.large"].networkInfo.ipv6AddressesPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.large"].networkInfo.maximumNetworkCards
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.large"].networkInfo.maximumNetworkInterfaces
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.large"].networkInfo.networkCards
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.large"].networkInfo.networkPerformance
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.large"].networkInfo.secondaryNetworkSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.large"].networkInfo.trafficMirroringSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.large"].phcSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.large"].processorInfo.sustainedClockSpeedInGhz
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.large"].rebootMigrationSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.xlarge"].cpuBurstModel
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.xlarge"].ebsInfo.attachmentLimitType
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.xlarge"].ebsInfo.ebsOptimizedInfo.baselineBandwidthInMbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.xlarge"].ebsInfo.ebsOptimizedInfo.baselineIops
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.xlarge"].ebsInfo.ebsOptimizedInfo.baselineThroughputInMBps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.xlarge"].ebsInfo.ebsOptimizedInfo.maximumBandwidthInMbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.xlarge"].ebsInfo.ebsOptimizedInfo.maximumIops
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.xlarge"].ebsInfo.ebsOptimizedInfo.maximumThroughputInMBps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.xlarge"].ebsInfo.maximumEbsAttachments
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.xlarge"].networkInfo.connectionTrackingConfiguration.defaultTcpEstablishedTimeout
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.xlarge"].networkInfo.defaultNetworkCardIndex
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.xlarge"].networkInfo.enaSrdSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.xlarge"].networkInfo.flexibleEnaQueuesSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.xlarge"].networkInfo.ipv4AddressesPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.xlarge"].networkInfo.ipv4AddressesPerSecondaryInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.xlarge"].networkInfo.ipv6AddressesPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.xlarge"].networkInfo.maximumNetworkCards
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.xlarge"].networkInfo.networkCards
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.xlarge"].networkInfo.networkPerformance
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.xlarge"].networkInfo.secondaryNetworkSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.xlarge"].networkInfo.trafficMirroringSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.xlarge"].phcSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.xlarge"].processorInfo.sustainedClockSpeedInGhz
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r8ib.xlarge"].rebootMigrationSupport
Issue-Related: 20261029T200817Z-ec2-read-paths-omit-modeled-members-and-serve.md
Issue-Related: 20261030T063325Z-ec2-static-catalogs-diverge-from-aws-instance-type.md
Issue-Related: 20261030T124526Z-ec2-describeinstancetypes-serves-a-synthesized-instance-type-catalog.md
Issue-Related: 20261030T210852Z-ec2-describeinstancetypes-synthesizes-instance-type-attributes-instead-of.md
Issue-Commit: 5c0f2e9076960c0c5ec367c612fbea7052ab391b

# ec2 DescribeInstanceTypes omits modeled per-type members and derives EBS, network, GPU and vCPU attributes instead of serving the AWS catalog

> _Note: this issue was identified against the code base at commit `5c0f2e9076960c0c5ec367c612fbea7052ab391b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeInstanceTypes

## Description
This issue bundles FOUR causes (4 of 4 below), all on ec2 DescribeInstanceTypes. All four share one fault: the emulator does not serve a real per-instance-type attribute catalog. It answers 200 with an instanceTypeSet whose entries (a) DROP several modeled members entirely and (b) carry attribute values derived from a formula (the same constants recur across unrelated families: ebsOptimizedInfo.maximumBandwidthInMbps 4750, maximumIops 18750, maximumThroughputInMBps 593.75, ebsInfo.maximumEbsAttachments 28, networkInfo.connectionTrackingConfiguration.defaultTcpEstablishedTimeout 432000) rather than the per-type values AWS returns. These are catalog facts, not per-run values: they do not vary by account, host, clock or request id, so they are requirements.

SCOPE: the replay reports 154 divergence clusters that are neither stub-blocked nor owned by an open issue, and EVERY one of them is service=ec2, operation=DescribeInstanceTypes, from the ec2-catalog-describe-instance-types-* fixtures. The four causes below are the four whose replay keys are named here; the remaining clusters are the same defect on other families/steps of the same fixtures. Three open issues describe this defect for OTHER instance-type keys and do not carry these clusters' keys: 20261030T063325Z-ec2-static-catalogs-diverge-from-aws-instance-type.md, 20261030T124526Z-ec2-describeinstancetypes-serves-a-synthesized-instance-type-catalog.md and 20261030T210852Z-ec2-describeinstancetypes-synthesizes-instance-type-attributes-instead-of.md (that one is bound to a1-c9g#task#4 with c4.* keys). A fix is only complete when all four causes below replay clean.

Note on model citation: smithy_query could not resolve the ec2 Smithy model in this checkout (model_path probes under services/service-com-amazonaws-ec2-amazonec2/smithy-model/ failed, so no shape dump was available). The members named below are therefore cited as they appear on the wire in the recorded AWS responses (EC2 Query protocol, DescribeInstanceTypesResponse.instanceTypeSet.item, i.e. the InstanceTypeInfo shape reached from DescribeInstanceTypes output) — recorded behavior only, no model-derived constraint is asserted.

--- Cause 1 of 4: modeled per-type members absent and EBS/network/vCPU attributes wrong for the c5a family. Fixture ec2-catalog-describe-instance-types-a1-c9g, step task#6.
AWS: recorded behavior only. For each instanceTypeSet.item AWS populates cpuBurstModel ('none' for non-burstable types), networkInfo.defaultNetworkCardIndex, networkInfo.enaSrdSupported, networkInfo.flexibleEnaQueuesSupport, networkInfo.maximumNetworkCards, networkInfo.networkCards, networkInfo.secondaryNetworkSupported, networkInfo.trafficMirroringSupport, phcSupport and rebootMigrationSupport, and returns per-type values for dedicatedHostsSupported, hibernationSupported, ebsInfo.ebsOptimizedInfo.*, ebsInfo.maximumEbsAttachments, networkInfo.networkPerformance, processorInfo.sustainedClockSpeedInGhz and vCpuInfo.validCores.
Emulator: those members are omitted from the served item (reported 'missing ... got None') and the values it does serve are wrong, e.g. for c5a.xlarge dedicatedHostsSupported 'true' (AWS 'false'), hibernationSupported 'true' (AWS 'false'), ebsOptimizedInfo.baselineBandwidthInMbps 1150 (AWS 400), baselineIops 6000 (AWS 1600), baselineThroughputInMBps 143.75 (AWS 50.0), maximumBandwidthInMbps 4750 (AWS 3170), maximumIops 18750 (AWS 13300), maximumThroughputInMBps 593.75 (AWS 396.25), maximumEbsAttachments 28 (AWS 27); vCpuInfo.validCores carries extra entries that AWS does not list for the type. Status matched (200 -> 200), so the divergence is purely the body.
Why: a client that sizes storage or networking from this call (choosing EBS-optimized throughput, counting network cards, checking hibernation or dedicated-host support) reads attributes that do not exist on the real type, and a client reading the modeled-but-absent members gets null where AWS always returns a value.
Fix: the DescribeInstanceTypes handler's instance-type catalog (its static per-type attribute table) and the serializer that renders instanceTypeSet.item — the table must carry AWS's per-type attributes instead of deriving them from vCPU/memory, and the serializer must emit the members it currently drops.
Traces: ec2-catalog-describe-instance-types-a1-c9g#task#6.

--- Cause 2 of 4: gpuInfo and instance-storage/memory attributes wrong for accelerated types. Fixture ec2-catalog-describe-instance-types-m6i-r4, step task#41.
AWS: recorded behavior only. For the accelerated type in this step AWS returns gpuInfo with gpus.item {count, gpuPartitionSize, logicalGpuCount, manufacturer 'NVIDIA', memoryInfo.sizeInMiB, name (e.g. 'B200'), workloadSet.item 'ml-ai'} plus gpuInfo.totalGpuMemoryInMiB, and per-type instanceStorageInfo.disks (count/sizeInGB), instanceStorageInfo.totalSizeInGB, memoryInfo.sizeInMiB, ebsInfo.attachmentLimitType 'dedicated', networkInfo.efaInfo.maximumEfaInterfaces, networkInfo.connectionTrackingConfiguration.defaultTcpEstablishedTimeout 350, supportedUsageClasses and nitroEnclavesSupport.
Emulator: gpuInfo is absent entirely; disks.item.count 13 (AWS 8) and disks.item.sizeInGB 1846 (AWS 3800); ebsInfo.attachmentLimitType 'shared' (AWS 'dedicated'); ebsOptimizedInfo.baselineBandwidthInMbps/maximumBandwidthInMbps 40800 (AWS 100000), baselineIops/maximumIops 180000 (AWS 400000), baseline/maximumThroughputInMBps 5100 (AWS 12500.0); maximumEbsAttachments 28 (AWS 64); memoryInfo.sizeInMiB, instanceStorageInfo.totalSizeInGB, networkInfo.maximumNetworkInterfaces, networkInfo.ipv4AddressesPerSecondaryInterface, supportedUsageClasses entries and vCpuInfo.validCores all diverge; cpuBurstModel, defaultNetworkCardIndex, enaSrdSupported, flexibleEnaQueuesSupport, maximumNetworkCards, networkCards, phcSupport and rebootMigrationSupport are missing. Status matched (200 -> 200).
Why: an accelerated-compute client cannot discover any GPU at all (gpuInfo absent means 'no accelerator' to every SDK), and capacity planners read storage, memory and EBS limits that belong to no real type.
Fix: same component as Cause 1 — the per-type catalog must include accelerator, instance-storage and memory facts, and the serializer must render gpuInfo when the type has one.
Traces: ec2-catalog-describe-instance-types-m6i-r4#task#41.

--- Cause 3 of 4: the same omission on the next accelerated type in the same fixture. Fixture ec2-catalog-describe-instance-types-m6i-r4, step task#42.
AWS: recorded behavior only; AWS returns gpuInfo for this type too (gpus.item name 'B300', manufacturer 'NVIDIA', count 8, memoryInfo.sizeInMiB and gpuInfo.totalGpuMemoryInMiB) and the same per-type instanceStorageInfo/memoryInfo/ebsInfo attributes.
Emulator: gpuInfo absent; disks.item.count 13 (AWS 8), disks.item.sizeInGB 1846 (AWS 3800); attachmentLimitType 'shared' (AWS 'dedicated'); ebsOptimizedInfo 40800/180000/5100 against AWS 100000/400000/12500.0; maximumEbsAttachments 28 (AWS 64); cpuBurstModel, networkInfo.defaultNetworkCardIndex, enaSrdSupported, flexibleEnaQueuesSupport, maximumNetworkCards, networkCards, phcSupport, rebootMigrationSupport missing; supportedUsageClasses and vCpuInfo.validCores wrong. Status matched (200 -> 200).
Why: identical client impact to Cause 2, and it shows the catalog is wrong for every accelerated type rather than one entry — fixing one type's row does not close it.
Fix: as Cause 1/2, in the catalog table rather than in a per-type special case.
Traces: ec2-catalog-describe-instance-types-m6i-r4#task#42.

--- Cause 4 of 4: EBS and connection-tracking attributes wrong for the r8ib family. Fixture ec2-catalog-describe-instance-types-r5-r8in, step task#27.
AWS: recorded behavior only. For r8ib.* AWS returns cpuBurstModel 'none', ebsInfo.attachmentLimitType 'dedicated', per-type ebsOptimizedInfo (for r8ib.large baselineBandwidthInMbps 1563, baselineIops 7500, baselineThroughputInMBps 195.375, maximumBandwidthInMbps 25000, maximumIops 120000, maximumThroughputInMBps 3125.0), maximumEbsAttachments 32, networkInfo.connectionTrackingConfiguration.defaultTcpEstablishedTimeout 350, networkInfo.defaultNetworkCardIndex 0, enaSrdSupported 'false', and per-type ipv4AddressesPerInterface / ipv6AddressesPerInterface / maximumNetworkInterfaces / networkCards / efaInfo / efaSupported.
Emulator: attachmentLimitType 'shared'; ebsOptimizedInfo 575/3000/71.875 baseline and 4750/18750/593.75 maximum (the same constants it serves for c5a); maximumEbsAttachments 28; defaultTcpEstablishedTimeout 432000; cpuBurstModel, defaultNetworkCardIndex, enaSrdSupported, flexibleEnaQueuesSupport, maximumNetworkCards, networkCards, phcSupport and rebootMigrationSupport missing; ipv4/ipv6-per-interface counts and validCores wrong. Status matched (200 -> 200).
Why: the repeated constants prove the values are synthesized, so any client that compares two instance types through this API (right-sizing, scheduler placement, EBS-throughput checks) gets the same answer for every family and picks the wrong type.
Fix: as above — one per-type attribute source for the whole catalog, including the connection-tracking defaults and the attachment-limit type.
Traces: ec2-catalog-describe-instance-types-r5-r8in#task#27.

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator up; /__admin/health 200): steps_total 2159, passed 1017, mismatched 880, score 47.1%, divergence_count 535. Of those clusters, 154 are neither stub-blocked (stub_path empty) nor owned by an open issue (owned_by empty), and all 154 are service=ec2 / operation=DescribeInstanceTypes from ec2-catalog-describe-instance-types-* fixtures; a verdict=mismatch with expected_status 200 and actual_status 200 in every case, i.e. body-only divergence.

Cause 1 - ec2-catalog-describe-instance-types-a1-c9g#task#6 (divergence_key service=ec2|DescribeInstanceTypes|status=200->200): expected vs actual for c5a.xlarge - cpuBurstModel: expected 'none', got None; dedicatedHostsSupported: expected 'false', got 'true'; hibernationSupported: expected 'false', got 'true'; ebsInfo.ebsOptimizedInfo.baselineBandwidthInMbps: expected '400', got '1150'; baselineIops: expected '1600', got '6000'; baselineThroughputInMBps: expected '50.0', got '143.75'; maximumBandwidthInMbps: expected '3170', got '4750'; maximumIops: expected '13300', got '18750'; maximumThroughputInMBps: expected '396.25', got '593.75'; ebsInfo.maximumEbsAttachments: expected '27', got '28'; networkInfo.defaultNetworkCardIndex: expected '0', got None; networkInfo.enaSrdSupported: expected 'false', got None; networkInfo.flexibleEnaQueuesSupport: expected 'unsupported', got None; networkInfo.maximumNetworkCards / networkInfo.networkCards / networkInfo.networkPerformance / networkInfo.secondaryNetworkSupported / networkInfo.trafficMirroringSupport / phcSupport / processorInfo.sustainedClockSpeedInGhz / rebootMigrationSupport: missing or value_mismatch; vCpuInfo.validCores.item[4..] present where AWS lists fewer entries. The cluster's key enumerates the same defect for c5a.large, c5a.2xlarge, c5a.4xlarge, c5a.8xlarge, c5a.12xlarge, c5a.16xlarge and c5a.24xlarge.

Cause 2 - ec2-catalog-describe-instance-types-m6i-r4#task#41: gpuInfo: expected {'gpus': {'item': {'count': '8', 'gpuPartitionSize': '1.0', 'logicalGpuCount': '8', 'manufacturer': 'NVIDIA', 'memoryInfo': {'sizeInMiB': '183359'}, 'name': 'B200', 'workloadSet': {'item': 'ml-ai'}}}, 'totalGpuMemoryInMiB': '1466872'}, got None; ebsInfo.attachmentLimitType: expected 'dedicated', got 'shared'; ebsOptimizedInfo.baselineBandwidthInMbps/maximumBandwidthInMbps: expected '100000', got '40800'; baselineIops/maximumIops: expected '400000', got '180000'; baseline/maximumThroughputInMBps: expected '12500.0', got '5100'; maximumEbsAttachments: expected '64', got '28'; instanceStorageInfo.disks.item.count: expected '8', got '13'; disks.item.sizeInGB: expected '3800', got '1846'; plus instanceStorageInfo.totalSizeInGB, memoryInfo.sizeInMiB, networkInfo.connectionTrackingConfiguration.defaultTcpEstablishedTimeout, networkInfo.efaInfo.maximumEfaInterfaces, networkInfo.ipv4AddressesPerSecondaryInterface, networkInfo.maximumNetworkInterfaces, nitroEnclavesSupport, supportedUsageClasses.item[0..2] and vCpuInfo.validCores.item[0..95] in the cluster key, and cpuBurstModel / defaultNetworkCardIndex / enaSrdSupported / flexibleEnaQueuesSupport / maximumNetworkCards / networkCards / phcSupport / rebootMigrationSupport missing.

Cause 3 - ec2-catalog-describe-instance-types-m6i-r4#task#42: gpuInfo: expected {'gpus': {'item': {'count': '8', 'gpuPartitionSize': '1.0', 'logicalGpuCount': '8', 'manufacturer': 'NVIDIA', 'memoryInfo': {'sizeInMiB': '275040'}, 'name': 'B300', 'workloadSet': {'item': 'ml-ai'}}}, 'totalGpuMemoryInMiB': '2200320'}, got None; the same ebsInfo/instanceStorageInfo/network divergences as Cause 2 (40800 vs 100000, 180000 vs 400000, 5100 vs 12500.0, 28 vs 64, disks 13x1846 vs 8x3800).

Cause 4 - ec2-catalog-describe-instance-types-r5-r8in#task#27: for r8ib.large - cpuBurstModel: expected 'none', got None; ebsInfo.attachmentLimitType: expected 'dedicated', got 'shared'; ebsOptimizedInfo.baselineBandwidthInMbps: expected '1563', got '575'; baselineIops: expected '7500', got '3000'; baselineThroughputInMBps: expected '195.375', got '71.875'; maximumBandwidthInMbps: expected '25000', got '4750'; maximumIops: expected '120000', got '18750'; maximumThroughputInMBps: expected '3125.0', got '593.75'; maximumEbsAttachments: expected '32', got '28'; networkInfo.connectionTrackingConfiguration.defaultTcpEstablishedTimeout: expected '350', got '432000'; networkInfo.defaultNetworkCardIndex: expected '0', got None; networkInfo.enaSrdSupported: expected 'false', got None. The cluster key repeats this for r8ib.2xlarge, 12xlarge, 16xlarge, 24xlarge, 32xlarge and 48xlarge, including networkInfo.efaInfo / efaSupported / ipv4AddressesPerInterface / ipv6AddressesPerInterface and vCpuInfo.validCores.

The emulator's maximum EBS figures (4750 / 18750 / 593.75), maximumEbsAttachments 28 and defaultTcpEstablishedTimeout 432000 are identical across the c5a, GPU and r8ib families above, which is the signature of synthesized attributes rather than a catalog.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus simparity-wire-aws-v1.
2. Cause 1: replay fixture ec2-catalog-describe-instance-types-a1-c9g, step task#6 - an EC2 Query DescribeInstanceTypes call over the c5a family. Compare DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.xlarge"] against the recording: cpuBurstModel, networkInfo.defaultNetworkCardIndex, networkInfo.enaSrdSupported, networkInfo.flexibleEnaQueuesSupport, networkInfo.maximumNetworkCards, networkInfo.networkCards, phcSupport and rebootMigrationSupport are absent from the emulator's body, and dedicatedHostsSupported, hibernationSupported, ebsInfo.ebsOptimizedInfo.*, ebsInfo.maximumEbsAttachments and vCpuInfo.validCores differ.
3. Cause 2: replay ec2-catalog-describe-instance-types-m6i-r4, step task#41 and read the accelerated type's item - gpuInfo is not present in the response at all, and instanceStorageInfo.disks, memoryInfo.sizeInMiB, ebsInfo.attachmentLimitType and ebsInfo.ebsOptimizedInfo.* differ from the recording.
4. Cause 3: replay ec2-catalog-describe-instance-types-m6i-r4, step task#42 and confirm gpuInfo is missing for the second accelerated type as well, with the same ebsInfo/instanceStorageInfo divergences.
5. Cause 4: replay ec2-catalog-describe-instance-types-r5-r8in, step task#27 and compare the r8ib.* items - ebsInfo.attachmentLimitType, ebsInfo.ebsOptimizedInfo.*, ebsInfo.maximumEbsAttachments and networkInfo.connectionTrackingConfiguration.defaultTcpEstablishedTimeout are the same constants the emulator serves for c5a.
6. Cross-check that the emulator returns the identical ebsOptimizedInfo maxima (4750 / 18750 / 593.75) and maximumEbsAttachments 28 for all three families, confirming the values are derived rather than read from a per-type catalog.
7. All four steps answer HTTP 200 with the expected top-level structure, so the divergence is body content only; re-replay these four fixture steps after the fix.
