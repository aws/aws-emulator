Issue-Source: trace-sweep-divergence
Issue-Trace: ec2-account-and-instance-type-metadata
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ec2-account-and-instance-type-metadata ec2-catalog-describe-instance-types-a1-c9g ec2-catalog-describe-instance-types-c9gd-m6gd ec2-catalog-describe-vpc-endpoint-services ec2-gateway-vpc-endpoint-lifecycle
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item.ebsInfo.ebsOptimizedInfo.baselineBandwidthInMbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item.ebsInfo.ebsOptimizedInfo.baselineIops
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item.ebsInfo.ebsOptimizedInfo.baselineThroughputInMBps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item.ebsInfo.ebsOptimizedInfo.maximumBandwidthInMbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item.ebsInfo.ebsOptimizedInfo.maximumIops
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item.ebsInfo.ebsOptimizedInfo.maximumThroughputInMBps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="a1.2xlarge"].ebsInfo.ebsOptimizedInfo.baselineBandwidthInMbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="a1.2xlarge"].ebsInfo.ebsOptimizedInfo.baselineIops
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="a1.2xlarge"].ebsInfo.ebsOptimizedInfo.baselineThroughputInMBps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="a1.2xlarge"].ebsInfo.ebsOptimizedInfo.maximumBandwidthInMbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="a1.2xlarge"].ebsInfo.ebsOptimizedInfo.maximumIops
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="a1.2xlarge"].ebsInfo.ebsOptimizedInfo.maximumThroughputInMBps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="a1.4xlarge"].ebsInfo.ebsOptimizedInfo.baselineBandwidthInMbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="a1.4xlarge"].ebsInfo.ebsOptimizedInfo.baselineIops
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="a1.4xlarge"].ebsInfo.ebsOptimizedInfo.baselineThroughputInMBps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="a1.4xlarge"].ebsInfo.ebsOptimizedInfo.maximumBandwidthInMbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="a1.4xlarge"].ebsInfo.ebsOptimizedInfo.maximumIops
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="a1.4xlarge"].ebsInfo.ebsOptimizedInfo.maximumThroughputInMBps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="a1.large"].ebsInfo.ebsOptimizedInfo.baselineBandwidthInMbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="a1.large"].ebsInfo.ebsOptimizedInfo.baselineIops
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="a1.large"].ebsInfo.ebsOptimizedInfo.baselineThroughputInMBps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="a1.large"].ebsInfo.ebsOptimizedInfo.maximumBandwidthInMbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="a1.large"].ebsInfo.ebsOptimizedInfo.maximumIops
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="a1.large"].ebsInfo.ebsOptimizedInfo.maximumThroughputInMBps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="a1.medium"].ebsInfo.ebsOptimizedInfo.baselineBandwidthInMbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="a1.medium"].ebsInfo.ebsOptimizedInfo.baselineIops
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="a1.medium"].ebsInfo.ebsOptimizedInfo.baselineThroughputInMBps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="a1.medium"].ebsInfo.ebsOptimizedInfo.maximumBandwidthInMbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="a1.medium"].ebsInfo.ebsOptimizedInfo.maximumIops
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="a1.medium"].ebsInfo.ebsOptimizedInfo.maximumThroughputInMBps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="a1.metal"].ebsInfo.ebsOptimizedInfo.baselineBandwidthInMbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="a1.metal"].ebsInfo.ebsOptimizedInfo.baselineIops
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="a1.metal"].ebsInfo.ebsOptimizedInfo.baselineThroughputInMBps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="a1.metal"].ebsInfo.ebsOptimizedInfo.maximumBandwidthInMbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="a1.metal"].ebsInfo.ebsOptimizedInfo.maximumIops
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="a1.metal"].ebsInfo.ebsOptimizedInfo.maximumThroughputInMBps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="a1.xlarge"].ebsInfo.ebsOptimizedInfo.baselineBandwidthInMbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="a1.xlarge"].ebsInfo.ebsOptimizedInfo.baselineThroughputInMBps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="a1.xlarge"].ebsInfo.ebsOptimizedInfo.maximumBandwidthInMbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="a1.xlarge"].ebsInfo.ebsOptimizedInfo.maximumIops
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="a1.xlarge"].ebsInfo.ebsOptimizedInfo.maximumThroughputInMBps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[*]
Issue-Key: ec2|DescribeVpcEndpointServices|2xx->2xx|$.DescribeVpcEndpointServicesResponse.serviceDetailSet.item
Issue-Key: ec2|DescribeVpcEndpointServices|2xx->2xx|$.DescribeVpcEndpointServicesResponse.serviceNameSet.item
Issue-Key: ec2|DescribeVpcEndpointServices|2xx->2xx|$.DescribeVpcEndpointServicesResponse.serviceDetailSet.item.baseEndpointDnsNameSet.item
Issue-Key: ec2|DescribeVpcEndpointServices|2xx->2xx|$.DescribeVpcEndpointServicesResponse.serviceDetailSet.item.serviceId
Issue-Key: ec2|DescribeVpcEndpointServices|2xx->2xx|$.DescribeVpcEndpointServicesResponse.serviceDetailSet.item.serviceType.item
Issue-Key: ec2|DescribeVpcEndpointServices|2xx->2xx|$.DescribeVpcEndpointServicesResponse.serviceDetailSet.item.supportedIpAddressTypeSet.item
Issue-Related: 20261029T200817Z-ec2-read-paths-omit-modeled-members-and-serve.md
Issue-Related: 20261029T215142Z-ec2-describe-projections-omit-modeled-members-and-seed.md
Issue-Commit: 78d5907a1724d3304a3021142ffd4f12ae7136a6

# EC2 static catalogs diverge from AWS: instance-type attributes and VPC-endpoint-service details are emulator-authored

> _Note: this issue was identified against the code base at commit `78d5907a1724d3304a3021142ffd4f12ae7136a6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeInstanceTypes

## Description
Seven net-new wire divergences share one root cause: the emulator answers the two big read-only EC2 catalogs - the instance-type catalog and the VPC-endpoint-service catalog - from seeded data it authored itself rather than from AWS's values, and that data is both wrong per attribute and incomplete per entry. This is the dominant family in this replay: the great majority of the remaining unowned DescribeInstanceTypes clusters across the ec2-catalog-describe-instance-types-* fixtures (roughly 170 clusters, one per probed instance-type page) are the same defect, so fixing the seed/derivation fixes them together. This issue bundles 7 causes; all 7 must be closed. The open issues '20261029T200817Z-ec2-read-paths-omit-modeled-members-and-serve.md', '20261029T175114Z-ec2-read-paths-wrong-or-missing-members-in.md' and '20261023T155030Z-ec2-static-catalogs-list-entries-aws-does-not.md' describe this family on other steps but do not carry these replay keys.

Cause 1 of 7: DescribeInstanceTypes for a single type omits modeled members and reports derived EBS/network numbers that do not match AWS.
AWS: recorded behavior only for the values (the catalog is data, not a modeled constraint). The recording shows, per instance type, cpuBurstModel, networkInfo.defaultNetworkCardIndex, networkInfo.enaSrdSupported, networkInfo.flexibleEnaQueuesSupport, networkInfo.maximumNetworkCards, networkInfo.networkCards, networkInfo.networkPerformance, networkInfo.secondaryNetworkSupported, networkInfo.trafficMirroringSupport, phcSupport, processorInfo.sustainedClockSpeedInGhz and rebootMigrationSupport as present members, and concrete per-type values for dedicatedHostsSupported, freeTierEligible, ebsInfo.maximumEbsAttachments and the six ebsInfo.ebsOptimizedInfo baseline/maximum bandwidth-IOPS-throughput numbers.
Emulator: those members are absent and the ebsOptimizedInfo / attachment / flag values are computed from an emulator formula, so they differ from AWS for the same instance type.
Why: callers size workloads and validate instance selection off these numbers; a client reading maximumEbsAttachments or baselineThroughputInMBps gets a figure that no real instance has, and a client reading networkCards or phcSupport gets nothing.
Fix: the DescribeInstanceTypes handler's instance-type catalog data/derivation (the seeded catalog must carry AWS's per-type values and all modeled members).
Traces: ec2-account-and-instance-type-metadata#task#3.

Cause 2 of 7: the same defect across the a1/c9g catalog page - every probed type in the page diverges on the same member set (cpuBurstModel, dedicatedHostsSupported, ebsInfo.*, networkInfo.*, nitroEnclavesSupport, nitroTpmSupport, phcSupport, placementGroupInfo.supportedStrategies ordering, processorInfo.sustainedClockSpeedInGhz, rebootMigrationSupport, ebsInfo.encryptionSupport).
AWS: recorded behavior only - the recorded page lists each a1.* type with its real attribute values and with encryptionSupport 'supported'.
Emulator: encryptionSupport 'unsupported', inverted dedicatedHostsSupported, formula-derived EBS numbers and the members above omitted.
Why: as cause 1, now for a whole instance family, so any catalog-driven selection is wrong for every entry.
Fix: same component as cause 1.
Traces: ec2-catalog-describe-instance-types-a1-c9g#task#1.

Cause 3 of 7: catalog pages are missing whole instance-type entries.
AWS: recorded behavior only - the recorded page carries a list of instanceTypeSet items (e.g. a GPU type with gpuInfo.gpus naming manufacturer NVIDIA, name 'T4', memoryInfo.sizeInMiB 16384, workloadSet graphics/ml-ai, instanceStorageInfo disks, ebsInfo.attachmentLimitType 'shared', connectionTrackingConfiguration defaults).
Emulator: instanceTypeSet.item[0] through item[6] are absent from the response - the emulator's catalog does not contain those types at all.
Why: a client enumerating available instance types cannot see types AWS offers, so capacity and launch planning silently excludes them.
Fix: same component as cause 1 (catalog completeness, not just per-attribute values).
Traces: ec2-catalog-describe-instance-types-c9gd-m6gd#task#7.

Cause 4 of 7: a second page of the same catalog fixture is missing four entries whose recorded attributes include ebsInfo.attachmentLimitType 'dedicated', maximumEbsAttachments 32 and an L4 gpuInfo block.
AWS: recorded behavior only, as above.
Emulator: instanceTypeSet.item[0] through item[3] absent.
Why: as cause 3.
Fix: same component as cause 1.
Traces: ec2-catalog-describe-instance-types-c9gd-m6gd#task#12.

Cause 5 of 7: a third page of the same catalog fixture is missing six entries (recorded attributes include a 'RTX PRO Server 6000' gpuInfo block, instanceStorageInfo disks 1x1900 GB ssd, ipv4AddressesPerInterface 64).
AWS: recorded behavior only, as above.
Emulator: instanceTypeSet.item[0] through item[5] absent.
Why: as cause 3.
Fix: same component as cause 1.
Traces: ec2-catalog-describe-instance-types-c9gd-m6gd#task#14.

Cause 6 of 7: DescribeVpcEndpointServices returns a service-detail catalog that does not match AWS's.
AWS: recorded behavior only - the recorded serviceDetailSet lists each endpoint service with acceptanceRequired, availabilityZoneIdSet and availabilityZoneSet (four AZs for us-west-2), one baseEndpointDnsName per service, managesVpcEndpoints 'false', owner 'amazon', privateDnsName, privateDnsNameVerificationState 'verified', a privateDnsNameSet containing both the amazonaws.com and api.aws names, a real serviceId, serviceName, serviceRegion, serviceType {Interface}, supportedIpAddressTypeSet {ipv4, ipv6} and vpcEndpointPolicySupported 'true'.
Emulator: serviceDetailSet.item has the wrong type/contents against that list, and serviceNameSet.item diverges too.
Why: a client that discovers endpoint services, their AZ coverage or their private DNS names from this call gets an inventory that does not exist in AWS, and CreateVpcEndpoint choices made from it are unsound.
Fix: the DescribeVpcEndpointServices handler's endpoint-service catalog data.
Traces: ec2-catalog-describe-vpc-endpoint-services#task#18 (same cluster: #task#23, #task#26).

Cause 7 of 7: DescribeVpcEndpointServices for one gateway service reports the wrong serviceType, DNS-name cardinality and IP-address-type set.
AWS: recorded behavior only - for the queried gateway service the recording carries a single serviceType element {serviceType 'Gateway'}, one baseEndpointDnsName, a shaped serviceId, and supportedIpAddressTypeSet.item as the list ['ipv4','ipv6'].
Emulator: serviceType.item is a two-element list [{'Gateway'},{'Interface'}], baseEndpointDnsNameSet.item is a two-element list, serviceId is the emulator's own 'vpce-svc-fffbb61c', and supportedIpAddressTypeSet.item is the scalar 'ipv4'.
Why: a client branching on Gateway vs Interface sees both types for one service and may build an interface endpoint for a gateway-only service; the scalar-vs-list difference also breaks strict deserializers.
Fix: same component as cause 6 (per-service catalog entry contents and their list serialization).
Traces: ec2-gateway-vpc-endpoint-lifecycle#task#3.

## Evidence
Cause 1 (ec2-account-and-instance-type-metadata#task#3): cpuBurstModel missing (expected 'credit'); dedicatedHostsSupported expected 'true' got 'false'; ebsOptimizedInfo.baselineBandwidthInMbps expected '87' got '71'; baselineIops expected '500' got '375'; baselineThroughputInMBps expected '10.875' got '8.875'; maximumBandwidthInMbps expected '2085' got '4750'; maximumIops expected '11800' got '18750'; maximumThroughputInMBps expected '260.625' got '593.75'; maximumEbsAttachments expected '27' got '28'; freeTierEligible expected 'false' got 'true'; networkInfo.defaultNetworkCardIndex / enaSrdSupported / flexibleEnaQueuesSupport / maximumNetworkCards / networkCards / networkPerformance / secondaryNetworkSupported / trafficMirroringSupport, phcSupport, processorInfo.sustainedClockSpeedInGhz and rebootMigrationSupport all missing.
Cause 2 (…-a1-c9g#task#1): same member set per a1.* type, plus ebsInfo.encryptionSupport expected 'supported' got 'unsupported'.
Causes 3-5 (…-c9gd-m6gd#task#7/#task#12/#task#14): $.DescribeInstanceTypesResponse.instanceTypeSet.item[0..6] / item[0..3] / item[0..5] missing (expected full instance-type structures, got None).
Cause 6 (ec2-catalog-describe-vpc-endpoint-services#task#18): $.DescribeVpcEndpointServicesResponse.serviceDetailSet.item type_mismatch against the recorded service list, and serviceNameSet.item diverges.
Cause 7 (ec2-gateway-vpc-endpoint-lifecycle#task#3): serviceType.item type_mismatch (expected {'serviceType': 'Gateway'}, got [{'serviceType': 'Gateway'}, {'serviceType': 'Interface'}]); supportedIpAddressTypeSet.item type_mismatch (expected ['ipv4','ipv6'], got 'ipv4'); baseEndpointDnsNameSet.item and serviceId diverge from the recorded shape.

## Steps to Reproduce
Replay simparity-wire-aws-v1 and inspect ec2-account-and-instance-type-metadata#task#3, ec2-catalog-describe-instance-types-a1-c9g#task#1, ec2-catalog-describe-instance-types-c9gd-m6gd#task#7/#task#12/#task#14, ec2-catalog-describe-vpc-endpoint-services#task#18 and ec2-gateway-vpc-endpoint-lifecycle#task#3. Diff the instanceTypeSet / serviceDetailSet contents against the recorded responses; then re-run the whole ec2-catalog-describe-instance-types-* family, which is the same defect at scale.
