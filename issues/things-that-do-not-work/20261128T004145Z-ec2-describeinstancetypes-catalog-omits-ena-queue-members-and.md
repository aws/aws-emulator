Issue-Source: trace-sweep-divergence
Issue-Trace: ec2-catalog-describe-instance-types-a1-c9g
Issue-Step: task#6
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ec2-catalog-describe-instance-types-a1-c9g
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.12xlarge"].vCpuInfo.validCores.item
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.16xlarge"].vCpuInfo.validCores.item
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.24xlarge"].vCpuInfo.validCores.item
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.4xlarge"].vCpuInfo.validCores.item
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.8xlarge"].vCpuInfo.validCores.item
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c8ine.12xlarge"].networkInfo.ipv4AddressesPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c8ine.12xlarge"].networkInfo.ipv6AddressesPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c8ine.12xlarge"].networkInfo.maximumNetworkInterfaces
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c8ine.12xlarge"].networkInfo.networkCards.item.additionalFlexibleNetworkInterfaces
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c8ine.12xlarge"].networkInfo.networkCards.item.baselineBandwidthInGbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c8ine.12xlarge"].networkInfo.networkCards.item.defaultEnaQueueCountPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c8ine.12xlarge"].networkInfo.networkCards.item.maximumEnaQueueCount
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c8ine.12xlarge"].networkInfo.networkCards.item.maximumEnaQueueCountPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c8ine.12xlarge"].networkInfo.networkCards.item.maximumNetworkInterfaces
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c8ine.12xlarge"].networkInfo.networkCards.item.networkPerformance
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c8ine.12xlarge"].networkInfo.networkCards.item.peakBandwidthInGbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c8ine.12xlarge"].networkInfo.networkPerformance
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c8ine.2xlarge"].networkInfo.ipv4AddressesPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c8ine.2xlarge"].networkInfo.ipv6AddressesPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c8ine.2xlarge"].networkInfo.networkCards.item.additionalFlexibleNetworkInterfaces
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c8ine.2xlarge"].networkInfo.networkCards.item.baselineBandwidthInGbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c8ine.2xlarge"].networkInfo.networkCards.item.defaultEnaQueueCountPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c8ine.2xlarge"].networkInfo.networkCards.item.maximumEnaQueueCount
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c8ine.2xlarge"].networkInfo.networkCards.item.maximumEnaQueueCountPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c8ine.2xlarge"].networkInfo.networkCards.item.networkPerformance
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c8ine.2xlarge"].networkInfo.networkCards.item.peakBandwidthInGbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c8ine.2xlarge"].networkInfo.networkPerformance
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c8ine.4xlarge"].networkInfo.ipv4AddressesPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c8ine.4xlarge"].networkInfo.ipv6AddressesPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c8ine.4xlarge"].networkInfo.networkCards.item.additionalFlexibleNetworkInterfaces
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c8ine.4xlarge"].networkInfo.networkCards.item.baselineBandwidthInGbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c8ine.4xlarge"].networkInfo.networkCards.item.defaultEnaQueueCountPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c8ine.4xlarge"].networkInfo.networkCards.item.maximumEnaQueueCount
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c8ine.4xlarge"].networkInfo.networkCards.item.maximumEnaQueueCountPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c8ine.4xlarge"].networkInfo.networkCards.item.networkPerformance
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c8ine.4xlarge"].networkInfo.networkCards.item.peakBandwidthInGbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c8ine.4xlarge"].networkInfo.networkPerformance
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c8ine.8xlarge"].networkInfo.ipv4AddressesPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c8ine.8xlarge"].networkInfo.ipv6AddressesPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c8ine.8xlarge"].networkInfo.networkCards.item.additionalFlexibleNetworkInterfaces
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c8ine.8xlarge"].networkInfo.networkCards.item.baselineBandwidthInGbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c8ine.8xlarge"].networkInfo.networkCards.item.defaultEnaQueueCountPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c8ine.8xlarge"].networkInfo.networkCards.item.maximumEnaQueueCount
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c8ine.8xlarge"].networkInfo.networkCards.item.maximumEnaQueueCountPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c8ine.8xlarge"].networkInfo.networkCards.item.networkPerformance
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c8ine.8xlarge"].networkInfo.networkCards.item.peakBandwidthInGbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c8ine.8xlarge"].networkInfo.networkPerformance
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c8ine.large"].networkInfo.ipv4AddressesPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c8ine.large"].networkInfo.ipv6AddressesPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c8ine.large"].networkInfo.maximumNetworkInterfaces
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c8ine.large"].networkInfo.networkCards.item.additionalFlexibleNetworkInterfaces
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c8ine.large"].networkInfo.networkCards.item.baselineBandwidthInGbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c8ine.large"].networkInfo.networkCards.item.defaultEnaQueueCountPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c8ine.large"].networkInfo.networkCards.item.maximumEnaQueueCount
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c8ine.large"].networkInfo.networkCards.item.maximumEnaQueueCountPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c8ine.large"].networkInfo.networkCards.item.maximumNetworkInterfaces
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c8ine.large"].networkInfo.networkCards.item.networkPerformance
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c8ine.large"].networkInfo.networkCards.item.peakBandwidthInGbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c8ine.large"].networkInfo.networkPerformance
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c8ine.xlarge"].networkInfo.ipv4AddressesPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c8ine.xlarge"].networkInfo.ipv6AddressesPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c8ine.xlarge"].networkInfo.networkCards.item.additionalFlexibleNetworkInterfaces
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c8ine.xlarge"].networkInfo.networkCards.item.baselineBandwidthInGbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c8ine.xlarge"].networkInfo.networkCards.item.defaultEnaQueueCountPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c8ine.xlarge"].networkInfo.networkCards.item.maximumEnaQueueCount
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c8ine.xlarge"].networkInfo.networkCards.item.maximumEnaQueueCountPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c8ine.xlarge"].networkInfo.networkCards.item.networkPerformance
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c8ine.xlarge"].networkInfo.networkCards.item.peakBandwidthInGbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c8ine.xlarge"].networkInfo.networkPerformance
Issue-Related: 20261112T231741Z-ec2-describeinstancetypes-catalog-rows-are-synthesized-from-a.md
Issue-Commit: 244eae870d957920c579d40eef12d136da6f5bab

# ec2 DescribeInstanceTypes catalog omits ENA queue members and misreports bandwidth, vCPU and interface limits

> _Note: this issue was identified against the code base at commit `244eae870d957920c579d40eef12d136da6f5bab`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeInstanceTypes

## Description
DescribeInstanceTypes answers 200 with an instance-type catalog whose per-type attributes do not match the AWS catalog. 95 replay clusters diverge on this operation with no owning issue; every one of them is in the corpus's DescribeInstanceTypes catalog fixtures (verified: no diverging DescribeInstanceTypes cluster has a fixture_id outside the ec2-catalog-describe-instance-types-* family), and the two clusters named under Traces between them exhibit all five distinct defects below. The open issue 20261127T205235Z-ec2-describeinstancetypes-catalog-attributes-diverge-across-every-instance.md describes the same operation but does not carry the replay keys of these clusters, so they are filed here; a fixer should treat the whole bundled catalog dataset, not just the two instance families quoted, as in scope.

Cause 1 of 5: networkInfo.networkCards[].* ENA queue members are not emitted at all.
AWS: Smithy NetworkCardInfo members DefaultEnaQueueCountPerInterface (xmlName defaultEnaQueueCountPerInterface), MaximumEnaQueueCount (maximumEnaQueueCount), MaximumEnaQueueCountPerInterface (maximumEnaQueueCountPerInterface) and AdditionalFlexibleNetworkInterfaces (additionalFlexibleNetworkInterfaces) are populated by AWS for current instance types; the recordings carry them for every type in the page (e.g. defaultEnaQueueCountPerInterface for a c5a size, and defaultEnaQueueCountPerInterface/maximumEnaQueueCount/maximumEnaQueueCountPerInterface/additionalFlexibleNetworkInterfaces for a c8ine size).
Emulator: those members are absent from every networkCards item (replay reports `missing (expected '<n>', got None)`).
Why: a client sizing ENA queues or flexible interfaces from the catalog reads None and either crashes on the absent member or silently falls back to a wrong default.
Fix: the DescribeInstanceTypes handler's instance-type catalog data plus the NetworkCardInfo projection in its serializer.
Traces: ec2-catalog-describe-instance-types-a1-c9g#task#6, ec2-catalog-describe-instance-types-a1-c9g#task#33.

Cause 2 of 5: baselineBandwidthInGbps / peakBandwidthInGbps / networkPerformance are derived rather than taken from the catalog.
AWS: recorded behavior only for the per-type figures; Smithy types them NetworkCardInfo.BaselineBandwidthInGbps and PeakBandwidthInGbps (double) and NetworkCardInfo.NetworkPerformance (string), and AWS publishes a tabulated value per instance type, with baseline below peak for burstable-network sizes and baseline == peak for the full-bandwidth sizes.
Emulator: emits values computed from the size multiplier instead. The replay shows baseline/peak pairs that are each wrong in a different direction for different sizes of the same family, and networkPerformance text that contradicts the recorded figure (expected the catalog's Gigabit string, got the emulator's derived one).
Why: capacity planning and any assertion on advertised network performance read a figure AWS never publishes for that type, and the baseline/peak relationship is inverted for some sizes.
Fix: the DescribeInstanceTypes catalog data (tabulated per instance type), not the derivation in the handler.
Traces: ec2-catalog-describe-instance-types-a1-c9g#task#6, ec2-catalog-describe-instance-types-a1-c9g#task#33.

Cause 3 of 5: vCpuInfo.validCores is emitted as a single scalar instead of the list of valid core counts.
AWS: Smithy VCpuInfo.ValidCores targets CoreCountList (type list, xmlName validCores) and is documented as "The valid number of cores that can be configured for the instance type" — AWS emits one validCores.item per configurable core count, ascending, ending at the type's default core count.
Emulator: emits a single item holding only the maximum (replay reports `type_mismatch (expected ['1', '2', ... '32'], got '32')`), so the list collapses to one element.
Why: a client enumerating CPU-options choices sees exactly one legal core count and cannot offer or validate any reduced-core configuration.
Fix: the DescribeInstanceTypes serializer/projection for VCpuInfo (emit the whole CoreCountList).
Traces: ec2-catalog-describe-instance-types-a1-c9g#task#6.

Cause 4 of 5: networkInfo per-interface IP limits and interface counts are wrong for network-optimised types.
AWS: recorded behavior only for the per-type numbers; Smithy NetworkInfo.Ipv4AddressesPerInterface / Ipv6AddressesPerInterface / MaximumNetworkInterfaces and NetworkCardInfo.MaximumNetworkInterfaces are per-instance-type catalog values.
Emulator: serves family-generic numbers — the replay shows ipv4AddressesPerInterface and ipv6AddressesPerInterface too low and maximumNetworkInterfaces (both on networkInfo and on the networkCards item) too low for a network-optimised size.
Why: a client that provisions the maximum number of ENIs or secondary addresses the catalog advertises under-provisions, and one that validates a request against the advertised ceiling rejects a configuration AWS accepts.
Fix: the DescribeInstanceTypes catalog data for the network-optimised families.
Traces: ec2-catalog-describe-instance-types-a1-c9g#task#33.

Cause 5 of 5: rebootMigrationSupport is reported as unsupported for types AWS reports as supported.
AWS: recorded behavior only — the recording for the same page carries rebootMigrationSupport 'supported'.
Emulator: answers 'unsupported'.
Why: a client that gates scheduled-event handling on reboot migration takes the wrong branch for every affected type.
Fix: the DescribeInstanceTypes catalog data.
Traces: ec2-catalog-describe-instance-types-a1-c9g#task#33.

## Evidence
Replay cluster ec2-catalog-describe-instance-types-a1-c9g#task#6 (expected 200, actual 200): $.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.xlarge"].networkInfo.networkCards.item.baselineBandwidthInGbps value_mismatch (expected '1.25', got '1.04'); ...defaultEnaQueueCountPerInterface missing (expected '8', got None); item[instanceType="c5a.16xlarge"] baselineBandwidthInGbps expected '20.0' got '16.66', peakBandwidthInGbps expected '20.0' got '16.66'; item[instanceType="c5a.16xlarge"].vCpuInfo.validCores.item type_mismatch (expected ['1','2','3','4','8','12','16','20','24','28','32'], got '32'); item[instanceType="c5a.24xlarge"] baselineBandwidthInGbps expected '20.0' got '25.0', validCores expected the 15-element list, got '48'.

Replay cluster ec2-catalog-describe-instance-types-a1-c9g#task#33 (expected 200, actual 200): item[instanceType="c8ine.12xlarge"].networkInfo.ipv4AddressesPerInterface expected '50' got '30'; ipv6AddressesPerInterface expected '50' got '30'; maximumNetworkInterfaces expected '12' got '8'; networkCards.item.additionalFlexibleNetworkInterfaces missing (expected '0'); defaultEnaQueueCountPerInterface missing (expected '32'); maximumEnaQueueCount missing (expected '384'); maximumEnaQueueCountPerInterface missing (expected '64'); networkCards.item.maximumNetworkInterfaces expected '12' got '8'; networkPerformance expected '75 Gigabit' got '100 Gigabit'; baseline/peakBandwidthInGbps expected '75.0' got '100.0'. The same key set repeats for the other sizes of the family.

Related: ec2-account-and-instance-type-metadata#task#3 shows the same shape (rebootMigrationSupport expected 'supported', got 'unsupported'; defaultEnaQueueCountPerInterface missing) and is already owned by 20261127T205235Z-ec2-describeinstancetypes-catalog-attributes-diverge-across-every-instance.md, so it is not re-filed here.

Smithy citations: VCpuInfo.ValidCores -> CoreCountList (list, xmlName validCores); NetworkCardInfo.{DefaultEnaQueueCountPerInterface, MaximumEnaQueueCount, MaximumEnaQueueCountPerInterface, AdditionalFlexibleNetworkInterfaces, BaselineBandwidthInGbps, PeakBandwidthInGbps, NetworkPerformance, MaximumNetworkInterfaces}.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus (trace set simparity-wire-aws-v1).
2. Replay fixture ec2-catalog-describe-instance-types-a1-c9g, step task#6: DescribeInstanceTypes for the c5a family. Compare instanceTypeSet.item[*].networkInfo.networkCards.item.{baselineBandwidthInGbps, peakBandwidthInGbps, defaultEnaQueueCountPerInterface} and instanceTypeSet.item[*].vCpuInfo.validCores against the recorded response.
3. Replay the same fixture's step task#33: DescribeInstanceTypes for the c8ine family. Compare networkInfo.{ipv4AddressesPerInterface, ipv6AddressesPerInterface, maximumNetworkInterfaces, networkPerformance}, networkCards.item.{additionalFlexibleNetworkInterfaces, defaultEnaQueueCountPerInterface, maximumEnaQueueCount, maximumEnaQueueCountPerInterface, maximumNetworkInterfaces, baselineBandwidthInGbps, peakBandwidthInGbps} and rebootMigrationSupport.
4. Both steps answer 200 with diverging catalog attributes; the ENA queue members and additionalFlexibleNetworkInterfaces are absent from the response entirely and validCores carries a single element.
