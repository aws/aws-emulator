Issue-Source: trace-sweep-divergence
Issue-Trace: ec2-catalog-describe-instance-types-a1-c9g
Issue-Step: task#8
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ec2-catalog-describe-instance-types-a1-c9g ec2-catalog-describe-instance-types-c9gd-m6gd ec2-describe-images-self-owned-pagination-and-params
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5d.12xlarge"].networkInfo.networkCards.item.baselineBandwidthInGbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5d.12xlarge"].networkInfo.networkCards.item.defaultEnaQueueCountPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5d.12xlarge"].networkInfo.networkCards.item.networkPerformance
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5d.12xlarge"].networkInfo.networkCards.item.peakBandwidthInGbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5d.18xlarge"].networkInfo.networkCards.item.baselineBandwidthInGbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5d.18xlarge"].networkInfo.networkCards.item.defaultEnaQueueCountPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5d.18xlarge"].networkInfo.networkCards.item.networkPerformance
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5d.18xlarge"].networkInfo.networkCards.item.peakBandwidthInGbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5d.24xlarge"].networkInfo.networkCards.item.defaultEnaQueueCountPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5d.2xlarge"].networkInfo.networkCards.item.baselineBandwidthInGbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5d.2xlarge"].networkInfo.networkCards.item.defaultEnaQueueCountPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5d.4xlarge"].networkInfo.networkCards.item.baselineBandwidthInGbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5d.4xlarge"].networkInfo.networkCards.item.defaultEnaQueueCountPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5d.9xlarge"].networkInfo.networkCards.item.baselineBandwidthInGbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5d.9xlarge"].networkInfo.networkCards.item.defaultEnaQueueCountPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5d.9xlarge"].networkInfo.networkCards.item.networkPerformance
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5d.9xlarge"].networkInfo.networkCards.item.peakBandwidthInGbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5d.large"].networkInfo.networkCards.item.baselineBandwidthInGbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5d.large"].networkInfo.networkCards.item.defaultEnaQueueCountPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5d.metal"].networkInfo.networkCards.item.defaultEnaQueueCountPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5d.xlarge"].networkInfo.networkCards.item.baselineBandwidthInGbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5d.xlarge"].networkInfo.networkCards.item.defaultEnaQueueCountPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c9gd.12xlarge"].networkInfo.networkCards.item.additionalFlexibleNetworkInterfaces
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c9gd.12xlarge"].networkInfo.networkCards.item.baselineBandwidthInGbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c9gd.12xlarge"].networkInfo.networkCards.item.defaultEnaQueueCountPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c9gd.12xlarge"].networkInfo.networkCards.item.maximumEnaQueueCount
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c9gd.12xlarge"].networkInfo.networkCards.item.maximumEnaQueueCountPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c9gd.12xlarge"].networkInfo.networkCards.item.maximumNetworkInterfaces
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c9gd.12xlarge"].networkInfo.networkCards.item.networkPerformance
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c9gd.12xlarge"].networkInfo.networkCards.item.peakBandwidthInGbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c9gd.16xlarge"].instanceStorageInfo.disks.item.count
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c9gd.16xlarge"].networkInfo.networkCards.item.additionalFlexibleNetworkInterfaces
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c9gd.16xlarge"].networkInfo.networkCards.item.baselineBandwidthInGbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c9gd.16xlarge"].networkInfo.networkCards.item.defaultEnaQueueCountPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c9gd.16xlarge"].networkInfo.networkCards.item.maximumEnaQueueCount
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c9gd.16xlarge"].networkInfo.networkCards.item.maximumEnaQueueCountPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c9gd.16xlarge"].networkInfo.networkCards.item.maximumNetworkInterfaces
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c9gd.16xlarge"].networkInfo.networkCards.item.networkPerformance
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c9gd.16xlarge"].networkInfo.networkCards.item.peakBandwidthInGbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c9gd.24xlarge"].networkInfo.networkCards.item.additionalFlexibleNetworkInterfaces
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c9gd.24xlarge"].networkInfo.networkCards.item.baselineBandwidthInGbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c9gd.24xlarge"].networkInfo.networkCards.item.defaultEnaQueueCountPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c9gd.24xlarge"].networkInfo.networkCards.item.maximumEnaQueueCount
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c9gd.24xlarge"].networkInfo.networkCards.item.maximumEnaQueueCountPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c9gd.24xlarge"].networkInfo.networkCards.item.maximumNetworkInterfaces
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c9gd.24xlarge"].networkInfo.networkCards.item.networkPerformance
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c9gd.24xlarge"].networkInfo.networkCards.item.peakBandwidthInGbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c9gd.2xlarge"].networkInfo.networkCards.item.additionalFlexibleNetworkInterfaces
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c9gd.2xlarge"].networkInfo.networkCards.item.baselineBandwidthInGbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c9gd.2xlarge"].networkInfo.networkCards.item.defaultEnaQueueCountPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c9gd.2xlarge"].networkInfo.networkCards.item.maximumEnaQueueCount
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c9gd.2xlarge"].networkInfo.networkCards.item.maximumEnaQueueCountPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c9gd.2xlarge"].networkInfo.networkCards.item.networkPerformance
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c9gd.2xlarge"].networkInfo.networkCards.item.peakBandwidthInGbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c9gd.48xlarge"].instanceStorageInfo.disks.item.count
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c9gd.48xlarge"].networkInfo.networkCards.item.additionalFlexibleNetworkInterfaces
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c9gd.48xlarge"].networkInfo.networkCards.item.baselineBandwidthInGbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c9gd.48xlarge"].networkInfo.networkCards.item.defaultEnaQueueCountPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c9gd.48xlarge"].networkInfo.networkCards.item.interfaceTypeSet.item
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c9gd.48xlarge"].networkInfo.networkCards.item.maximumEnaQueueCount
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c9gd.48xlarge"].networkInfo.networkCards.item.maximumEnaQueueCountPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c9gd.48xlarge"].networkInfo.networkCards.item.maximumNetworkInterfaces
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c9gd.48xlarge"].networkInfo.networkCards.item.networkPerformance
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c9gd.48xlarge"].networkInfo.networkCards.item.peakBandwidthInGbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c9gd.4xlarge"].networkInfo.networkCards.item.additionalFlexibleNetworkInterfaces
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c9gd.4xlarge"].networkInfo.networkCards.item.baselineBandwidthInGbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c9gd.4xlarge"].networkInfo.networkCards.item.defaultEnaQueueCountPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c9gd.4xlarge"].networkInfo.networkCards.item.maximumEnaQueueCount
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c9gd.4xlarge"].networkInfo.networkCards.item.maximumEnaQueueCountPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c9gd.4xlarge"].networkInfo.networkCards.item.networkPerformance
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c9gd.4xlarge"].networkInfo.networkCards.item.peakBandwidthInGbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c9gd.8xlarge"].networkInfo.networkCards.item.additionalFlexibleNetworkInterfaces
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c9gd.8xlarge"].networkInfo.networkCards.item.baselineBandwidthInGbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c9gd.8xlarge"].networkInfo.networkCards.item.defaultEnaQueueCountPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c9gd.8xlarge"].networkInfo.networkCards.item.maximumEnaQueueCount
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c9gd.8xlarge"].networkInfo.networkCards.item.maximumEnaQueueCountPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c9gd.8xlarge"].networkInfo.networkCards.item.maximumNetworkInterfaces
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c9gd.8xlarge"].networkInfo.networkCards.item.networkPerformance
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c9gd.8xlarge"].networkInfo.networkCards.item.peakBandwidthInGbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c9gd.large"].networkInfo.networkCards.item.additionalFlexibleNetworkInterfaces
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c9gd.large"].networkInfo.networkCards.item.baselineBandwidthInGbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c9gd.large"].networkInfo.networkCards.item.defaultEnaQueueCountPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c9gd.large"].networkInfo.networkCards.item.maximumEnaQueueCount
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c9gd.large"].networkInfo.networkCards.item.maximumEnaQueueCountPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c9gd.large"].networkInfo.networkCards.item.networkPerformance
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c9gd.large"].networkInfo.networkCards.item.peakBandwidthInGbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c9gd.medium"].networkInfo.networkCards.item.additionalFlexibleNetworkInterfaces
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c9gd.medium"].networkInfo.networkCards.item.baselineBandwidthInGbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c9gd.medium"].networkInfo.networkCards.item.defaultEnaQueueCountPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c9gd.medium"].networkInfo.networkCards.item.maximumEnaQueueCount
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c9gd.medium"].networkInfo.networkCards.item.maximumEnaQueueCountPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c9gd.medium"].networkInfo.networkCards.item.maximumNetworkInterfaces
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c9gd.medium"].networkInfo.networkCards.item.networkPerformance
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c9gd.medium"].networkInfo.networkCards.item.peakBandwidthInGbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c9gd.metal-48xl"].instanceStorageInfo.disks.item.count
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c9gd.metal-48xl"].networkInfo.networkCards.item.additionalFlexibleNetworkInterfaces
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c9gd.metal-48xl"].networkInfo.networkCards.item.baselineBandwidthInGbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c9gd.metal-48xl"].networkInfo.networkCards.item.defaultEnaQueueCountPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c9gd.metal-48xl"].networkInfo.networkCards.item.interfaceTypeSet.item
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c9gd.metal-48xl"].networkInfo.networkCards.item.maximumEnaQueueCount
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c9gd.metal-48xl"].networkInfo.networkCards.item.maximumEnaQueueCountPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c9gd.metal-48xl"].networkInfo.networkCards.item.maximumNetworkInterfaces
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c9gd.metal-48xl"].networkInfo.networkCards.item.networkPerformance
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c9gd.metal-48xl"].networkInfo.networkCards.item.peakBandwidthInGbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c9gd.xlarge"].networkInfo.networkCards.item.additionalFlexibleNetworkInterfaces
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c9gd.xlarge"].networkInfo.networkCards.item.baselineBandwidthInGbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c9gd.xlarge"].networkInfo.networkCards.item.defaultEnaQueueCountPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c9gd.xlarge"].networkInfo.networkCards.item.maximumEnaQueueCount
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c9gd.xlarge"].networkInfo.networkCards.item.maximumEnaQueueCountPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c9gd.xlarge"].networkInfo.networkCards.item.networkPerformance
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c9gd.xlarge"].networkInfo.networkCards.item.peakBandwidthInGbps
Issue-Key: ec2|DescribeImages|2xx->2xx|$.DescribeImagesResponse.imagesSet.item[name="sim2real-probe-ec2-img-58ab8652-1"].blockDeviceMapping.item.ebs.encrypted
Issue-Key: ec2|DescribeImages|2xx->2xx|$.DescribeImagesResponse.imagesSet.item[name="sim2real-probe-ec2-img-58ab8652-1"].freeTierEligible
Issue-Key: ec2|DescribeImages|2xx->2xx|$.DescribeImagesResponse.imagesSet.item[name="sim2real-probe-ec2-img-58ab8652-2"].blockDeviceMapping.item.ebs.encrypted
Issue-Key: ec2|DescribeImages|2xx->2xx|$.DescribeImagesResponse.imagesSet.item[name="sim2real-probe-ec2-img-58ab8652-2"].freeTierEligible
Issue-Key: ec2|DescribeImages|2xx->2xx|$.DescribeImagesResponse.imagesSet.item[name="sim2real-probe-ec2-img-58ab8652-3"].blockDeviceMapping.item.ebs.encrypted
Issue-Key: ec2|DescribeImages|2xx->2xx|$.DescribeImagesResponse.imagesSet.item[name="sim2real-probe-ec2-img-58ab8652-3"].freeTierEligible
Issue-Key: ec2|DescribeImages|2xx->2xx|$.DescribeImagesResponse.imagesSet.item[name="sim2real-probe-ec2-img-58ab8652-4"].blockDeviceMapping.item.ebs.encrypted
Issue-Key: ec2|DescribeImages|2xx->2xx|$.DescribeImagesResponse.imagesSet.item[name="sim2real-probe-ec2-img-58ab8652-4"].freeTierEligible
Issue-Key: ec2|DescribeImages|2xx->2xx|$.DescribeImagesResponse.imagesSet.item[name="sim2real-probe-ec2-img-58ab8652-5"].blockDeviceMapping.item.ebs.encrypted
Issue-Key: ec2|DescribeImages|2xx->2xx|$.DescribeImagesResponse.imagesSet.item[name="sim2real-probe-ec2-img-58ab8652-5"].freeTierEligible
Issue-Key: ec2|DescribeImages|2xx->2xx|$.DescribeImagesResponse.imagesSet.item[name="sim2real-probe-ec2-img-58ab8652-6"].blockDeviceMapping.item.ebs.encrypted
Issue-Key: ec2|DescribeImages|2xx->2xx|$.DescribeImagesResponse.imagesSet.item[name="sim2real-probe-ec2-img-58ab8652-6"].freeTierEligible
Issue-Key: ec2|DescribeImages|2xx->2xx|$.DescribeImagesResponse.imagesSet.item[name="sim2real-probe-ec2-img-58ab8652-7"].blockDeviceMapping.item.ebs.encrypted
Issue-Key: ec2|DescribeImages|2xx->2xx|$.DescribeImagesResponse.imagesSet.item[name="sim2real-probe-ec2-img-58ab8652-7"].freeTierEligible
Issue-Key: ec2|DescribeImages|2xx->2xx|$.DescribeImagesResponse.imagesSet.item[name="sim2real-probe-ec2-img-58ab8652-8"].blockDeviceMapping.item.ebs.encrypted
Issue-Key: ec2|DescribeImages|2xx->2xx|$.DescribeImagesResponse.imagesSet.item[name="sim2real-probe-ec2-img-58ab8652-8"].freeTierEligible
Issue-Related: 20261030T232813Z-ec2-describeinstancetypes-catalog-sweep-returns-synthesized-ebs-network.md
Issue-Related: 20261031T181451Z-ec2-describeinstancetypes-catalog-sweep-computes-per-type-ebs.md
Issue-Commit: 444741a3bf382435d77b2de94f3ea7c734469d34

# ec2 read payloads carry synthesized catalog facts: DescribeInstanceTypes attributes and DescribeImages members

> _Note: this issue was identified against the code base at commit `444741a3bf382435d77b2de94f3ea7c734469d34`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeInstanceTypes

## Description
Two causes, both in the ec2 read path that serves facts the emulator derives instead of storing. Both answer HTTP 200 with the modelled envelope, so the divergence is in member values and member presence, not in status or error code.

Cause 1 of 2: DescribeInstanceTypes reports instance-type attributes synthesized from the type NAME, so the hardware facts of nearly every type diverge from AWS and several modelled members are omitted entirely. Fixture ec2-catalog-describe-instance-types-a1-c9g, step task#8. The same cause is reported by 126 net-new divergence clusters of this replay, one per page of the ec2-catalog-describe-instance-types-* fixture family (second representative: ec2-catalog-describe-instance-types-c9gd-m6gd, step task#1, listed in trace_ids).
  AWS: DescribeInstanceTypes returns, per type, the facts AWS publishes for that type. Smithy (com.amazonaws.ec2, InstanceTypeInfo): ebsInfo -> EbsInfo.EbsOptimizedInfo {baselineBandwidthInMbps/maximumBandwidthInMbps (integer, Mbps), baselineIops/maximumIops (integer), baselineThroughputInMBps/maximumThroughputInMBps (double, MB/s)} and EbsInfo.maximumEbsAttachments; instanceStorageInfo -> InstanceStorageInfo {disks.item.count, disks.item.sizeInGB, totalSizeInGB}; networkInfo -> NetworkInfo {networkPerformance, maximumNetworkInterfaces, ipv4AddressesPerInterface, ipv4AddressesPerSecondaryInterface, ipv6AddressesPerInterface, trafficMirroringSupport, networkCards.item.{networkPerformance, peakBandwidthInGbps, baselineBandwidthInGbps, maximumNetworkInterfaces}}; processorInfo.sustainedClockSpeedInGhz; vCpuInfo.validCores (one item per supported core count); rebootMigrationSupport (enum RebootMigrationSupport); supportedBootModes; dedicatedHostsSupported. The recorded AWS responses also carry members the checked-in model does not define for InstanceTypeInfo/NetworkInfo (cpuBurstModel, networkInfo.bandwidthWeightings, networkInfo.connectionTrackingConfiguration.defaultTcpEstablishedTimeout, networkInfo.secondaryNetworkSupported, networkInfo.enaSrdSupported, networkCards.item.defaultEnaQueueCountPerInterface / maximumEnaQueueCount / maximumEnaQueueCountPerInterface / additionalFlexibleNetworkInterfaces, ebsInfo.attachmentLimitType) - recorded behavior only: AWS emits them for these types and the recording is the requirement. The per-type values are facts of the AWS catalog, not of a run: they do not vary by account, Region or clock.
  Emulator: the handler's own doc states the policy - 'every member is derived by crate::state::instance_type_info from the type's name - the family, generation, capability letters and size AWS's naming conventions define - read against the per-family ratios the EC2 User Guide publishes'. The derivation produces numbers that disagree with the published facts for most types and never emits the members listed above, so they are absent from the XML. Examples from the two representative steps: for c5d.24xlarge the emulator answers ebsOptimizedInfo.baselineBandwidthInMbps 20400 / baselineIops 90000 / baselineThroughputInMBps 2550.0 and maximumEbsAttachments 25 where AWS answers 19000 / 80000 / 2375.0 and 23, and instanceStorageInfo disks.item.count 2 x sizeInGB 1200 (totalSizeInGB 2400) where AWS answers 4 x 900 (totalSizeInGB 3600); for c9gd.metal-48xl it answers 40800 / 180000 / 5100.0 and maximumEbsAttachments 21 where AWS answers 72000 / 288000 / 9000.0 and 79, and 6 x 1900 disks where AWS answers 3 x 3800. cpuBurstModel, rebootMigrationSupport, networkInfo.trafficMirroringSupport, networkInfo.bandwidthWeightings, networkCards.item.defaultEnaQueueCountPerInterface and the other members above are missing rather than wrong, and vCpuInfo.validCores carries the wrong item set for the type's core count. The concrete numbers above are examples of the derivation, not the requirement: the requirement is the published fact for each type.
  Why: a client that sizes storage, EBS throughput or ENI/queue budgets from DescribeInstanceTypes - autoscaling planners, cost/capacity tools, Terraform-style validators, anything that selects a type by attribute - plans against numbers that do not exist on AWS, and a client that reads a member AWS always returns (cpuBurstModel, rebootMigrationSupport, bandwidthWeightings, defaultEnaQueueCountPerInterface) finds it absent and either defaults it or fails deserialization. Because the fault is in the shared derivation, EVERY DescribeInstanceTypes response in the emulator is affected, not one family.
  Fix: the instance-type catalog in the ec2 crate's state - crate::state::instance_type_info, read by services/service-com-amazonaws-ec2-amazonec2/src/ops/describe_instance_types.rs - must serve stored per-type facts rather than values derived from the type name, and must populate the members it currently omits; the operation's serializer must then emit them.
  Traces: ec2-catalog-describe-instance-types-a1-c9g#task#8 (representative); ec2-catalog-describe-instance-types-c9gd-m6gd#task#1; 126 clusters total across the ec2-catalog-describe-instance-types-* pages in this replay.

Cause 2 of 2: DescribeImages omits the per-mapping EBS encryption flag and reports the wrong free-tier flag for self-owned AMIs. Fixture ec2-describe-images-self-owned-pagination-and-params, step task#15.
  AWS: Smithy (com.amazonaws.ec2): Image.BlockDeviceMappings (xmlName blockDeviceMapping) -> BlockDeviceMapping.Ebs -> EbsBlockDevice.Encrypted (xmlName encrypted, Boolean) - 'Indicates whether the encryption state of an EBS volume is changed while being restored from a backing snapshot'; Image.FreeTierEligible (xmlName freeTierEligible, Boolean) - 'Indicates whether the image is eligible for Amazon Web Services Free Tier'. The recording shows AWS returning encrypted=false on every EBS mapping of these self-registered AMIs and freeTierEligible=true for each of them - recorded behavior only for the free-tier value, which AWS computes for the image rather than defaulting to false.
  Emulator: for every image in the page the emulator omits blockDeviceMapping.item.ebs.encrypted entirely (expected 'false', got nothing) and answers freeTierEligible 'false' where the recording has 'true'. Both diverge on all 8 images the step describes, so it is the image read model, not one row.
  Why: a client that checks whether an AMI's root volume is encrypted sees no encrypted element at all and must guess; a client that filters or reports on free-tier eligibility sees every AMI as ineligible, so free-tier selection logic takes the wrong branch against the emulator and the right one against AWS.
  Fix: the DescribeImages read model and serializer in services/service-com-amazonaws-ec2-amazonec2/src/ops/describe_images.rs (and the image record in the ec2 crate's state it reads) must carry the EBS mapping's encrypted flag and the image's free-tier eligibility and emit them.
  Traces: ec2-describe-images-self-owned-pagination-and-params#task#15.

Related open issues that do NOT carry these replay keys (the replay reports all 127 clusters as unowned, so this issue is net-new rather than a duplicate): 20261107T184734Z-ec2-describeinstancetypes-instance-type-catalog-returns-wrong-and.md (keyed on ec2-catalog-describe-instance-types-a1-c9g#task#33), 20261106T142039Z-ec2-describeinstancetypes-catalog-pages-still-diverge-per-instance-type.md, 20261106T122956Z-ec2-instance-type-catalog-pages-a1-c9g-through.md, 20261103T123451Z-ec2-static-catalogs-serve-emulator-authored-instance-type.md.

## Evidence
Cause 1 - fixture ec2-catalog-describe-instance-types-a1-c9g, step task#8, DescribeInstanceTypes, expected HTTP 200 / got HTTP 200, value divergences inside $.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="..."]:
  .cpuBurstModel: expected 'none', got nothing (member absent)
  .ebsInfo.ebsOptimizedInfo.baselineBandwidthInMbps: expected '19000', got '20400' (c5d.24xlarge)
  .ebsInfo.ebsOptimizedInfo.baselineIops: expected '80000', got '90000'
  .ebsInfo.ebsOptimizedInfo.baselineThroughputInMBps: expected '2375.0', got '2550.0'
  .ebsInfo.ebsOptimizedInfo.maximumBandwidthInMbps: expected '19000', got '20400'
  .ebsInfo.ebsOptimizedInfo.maximumIops: expected '80000', got '90000'
  .ebsInfo.ebsOptimizedInfo.maximumThroughputInMBps: expected '2375.0', got '2550.0'
  .ebsInfo.maximumEbsAttachments: expected '23', got '25'
  .instanceStorageInfo.disks.item.count: expected '4', got '2'
  .instanceStorageInfo.disks.item.sizeInGB: expected '900', got '1200'
  .instanceStorageInfo.totalSizeInGB: expected '3600', got '2400'
  .networkInfo.networkCards.item.defaultEnaQueueCountPerInterface: expected '8', got nothing
  plus .networkInfo.networkPerformance, .networkInfo.trafficMirroringSupport, .processorInfo.sustainedClockSpeedInGhz, .rebootMigrationSupport, .vCpuInfo.validCores.item[*] and .supportedBootModes.item diverging or absent across c5d.large .. c5d.metal in the same response.

Cause 1 - second cluster, fixture ec2-catalog-describe-instance-types-c9gd-m6gd, step task#1, expected 200 / got 200 (c9gd.metal-48xl):
  .ebsInfo.ebsOptimizedInfo.baselineBandwidthInMbps: expected '72000', got '40800'
  .ebsInfo.ebsOptimizedInfo.baselineIops: expected '288000', got '180000'
  .ebsInfo.ebsOptimizedInfo.baselineThroughputInMBps: expected '9000.0', got '5100.0'
  .ebsInfo.maximumEbsAttachments: expected '79', got '21'
  .instanceStorageInfo.disks.item.count: expected '3', got '6'
  .instanceStorageInfo.disks.item.sizeInGB: expected '3800', got '1900'
  .networkInfo.bandwidthWeightings: expected {'item': ['default','ebs-1','vpc-1']}, got nothing
  .cpuBurstModel: expected 'none', got nothing
  plus networkInfo.connectionTrackingConfiguration.defaultTcpEstablishedTimeout, networkInfo.secondaryNetworkSupported, networkInfo.enaSrdSupported, ebsInfo.attachmentLimitType, networkCards.item.maximumEnaQueueCount / maximumEnaQueueCountPerInterface / additionalFlexibleNetworkInterfaces absent for the c9gd.* types of the page.

Cause 2 - fixture ec2-describe-images-self-owned-pagination-and-params, step task#15, DescribeImages, expected HTTP 200 / got HTTP 200, repeated for every image sim2real-probe-ec2-img-*-1 .. -8 in $.DescribeImagesResponse.imagesSet.item[name="..."]:
  .blockDeviceMapping.item.ebs.encrypted: expected 'false', got nothing (member absent)
  .freeTierEligible: expected 'true', got 'false'
(The image names and ids above are per-run values from the recording; the requirement is that the members are present and carry the image's real encryption and free-tier state.)

## Steps to Reproduce
1. Clone the repo and boot the emulator built from it.
2. Replay the wire corpus simparity-wire-aws-v1 (replay_wire_corpus).
3. Cause 1: step task#8 of fixture ec2-catalog-describe-instance-types-a1-c9g issues DescribeInstanceTypes for a page of the catalog (c5d.* among them) and compares the whole instanceTypeSet against the recorded AWS response; the step answers 200 but the per-type attributes above diverge or are absent. Re-replay ec2-catalog-describe-instance-types-c9gd-m6gd#task#1 (c9gd.*) to see the same cause with different numbers - it reproduces on every page of the ec2-catalog-describe-instance-types-* family (126 clusters in this replay).
4. Cause 2: step task#15 of fixture ec2-describe-images-self-owned-pagination-and-params registers 8 self-owned AMIs and calls DescribeImages with Owner=self; the response omits blockDeviceMapping/item/ebs/encrypted on every mapping and reports freeTierEligible=false for every image.
