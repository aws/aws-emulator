Issue-Source: trace-sweep-divergence
Issue-Trace: opensearch-catalog-describe-packages
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: opensearch-catalog-describe-packages opensearch-catalog-describe-reserved-instance-offerings opensearch-catalog-get-compatible-versions opensearch-catalog-get-package-version-history opensearch-catalog-list-instance-type-details opensearch-catalog-list-versions
Issue-Shape-Id: com.amazonaws.opensearch#AmazonOpenSearchService
Issue-Key: opensearch|DescribePackages|2xx->2xx|$.PackageDetailsList[*]
Issue-Key: opensearch|DescribeReservedInstanceOfferings|2xx->2xx|$.ReservedInstanceOfferings[*]
Issue-Key: opensearch|GetCompatibleVersions|2xx->2xx|$.CompatibleVersions[*]
Issue-Key: opensearch|GetPackageVersionHistory|2xx->4xx|$.PackageID
Issue-Key: opensearch|GetPackageVersionHistory|2xx->4xx|$.PackageVersionHistoryList
Issue-Key: opensearch|GetPackageVersionHistory|2xx->4xx|status
Issue-Key: opensearch|ListInstanceTypeDetails|2xx->2xx|$.InstanceTypeDetails[*]
Issue-Key: opensearch|ListVersions|2xx->2xx|$.Versions[*]
Issue-Commit: 92c3b140342b1bc1fab2a232b8cee3f25ea78ff5

# opensearch: the package, offering, version and instance-type catalogs are synthetic

> _Note: this issue was identified against the code base at commit `92c3b140342b1bc1fab2a232b8cee3f25ea78ff5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribePackages

## Description
10 net-new root causes on the opensearch service token: every static reference catalog this service serves differs from AWS's. Recorded behavior only - the model constrains the shape, not the contents.

Cause 1 of 10 - DescribePackages serves none of AWS's catalog packages. AWS returns the AWS-published package catalog, each entry carrying PackageID, PackageName, PackageOwner, PackageType (e.g. ZIP-PLUGIN), PackageStatus AVAILABLE, EngineVersion, AvailablePackageVersion, AvailablePluginProperties (ClassName, Description, Name, UncompressedSizeInBytes, Version), AvailablePackageConfiguration (ConfigurationRequirement, LicenseFilepath, LicenseRequirement, RequiresRestartForConfigurationUpdate), CreatedAt and LastUpdatedAt. The emulator returns none of the 92 entries the recording contains. Trace: opensearch-catalog-describe-packages#task#1.

Cause 2 of 10 - DescribeReservedInstanceOfferings serves a different offering set. AWS's offerings are current-generation (c7i, or1, r8g, ...) with 94608000/31536000 durations and a RecurringCharges entry on every offering including ALL_UPFRONT (amount 0.0, frequency Hourly); the emulator serves m4/d2-era offerings and returns RecurringCharges: [] for ALL_UPFRONT. Trace: opensearch-catalog-describe-reserved-instance-offerings#task#1.

Cause 3 of 10 - GetCompatibleVersions returns the wrong upgrade matrix. AWS's unfiltered matrix starts at Elasticsearch_7.7 and each entry's TargetVersions is only the versions that source may upgrade to; the emulator starts at Elasticsearch_1.5 and enumerates every later version as a target. Trace: opensearch-catalog-get-compatible-versions#task#1.

Cause 4 of 10 - GetPackageVersionHistory refuses every catalog package. AWS: 200 with PackageID echoed and PackageVersionHistoryList holding one entry per version (CommitMessage, CreatedAt, PackageConfiguration, PackageVersion, PluginProperties). Emulator: 400 with no PackageID and no history - the same for all 90 catalog package ids the fixture walks. Trace: opensearch-catalog-get-package-version-history#task#1.

Cause 5 of 10 - ListInstanceTypeDetails, unfiltered: AWS serves the current generation (r5.large.search, m7i.2xlarge.search, m5.large.search, c5.large.search, ...) with an AvailabilityZones member and 'coordinator' in InstanceRole where supported; the emulator serves a legacy c4/c5-era catalog, omits AvailabilityZones and never reports coordinator. Trace: opensearch-catalog-list-instance-type-details#task#1.

Cause 6 of 10 - ListInstanceTypeDetails filtered on warm support returns the wrong set and reports WarmEnabled false on every entry where AWS reports true. Trace: opensearch-catalog-list-instance-type-details#task#6.

Cause 7 of 10 - ListInstanceTypeDetails filtered with advanced security disabled returns the wrong set and still reports WarmEnabled false where AWS reports true. Trace: opensearch-catalog-list-instance-type-details#task#7.

Cause 8 of 10 - ListInstanceTypeDetails filtered with encryption disabled returns the wrong set; AWS's entries still report EncryptionEnabled true and WarmEnabled true for the types that support them. Trace: opensearch-catalog-list-instance-type-details#task#13.

Cause 9 of 10 - ListInstanceTypeDetails with every capability filter off returns the wrong set, led by c4.2xlarge.search instead of AWS's r4.2xlarge.search, and reports WarmEnabled false where AWS reports true. Trace: opensearch-catalog-list-instance-type-details#task#16.

Cause 10 of 10 - ListVersions returns the engine versions oldest-first and led by the Elasticsearch line; AWS returns them newest-first led by OpenSearch_3.7, OpenSearch_3.5, OpenSearch_3.3, OpenSearch_3.1. Trace: opensearch-catalog-list-versions#task#1.

Why: these catalogs are what a client chooses from. A client that picks a package, an engine version, an upgrade target, an instance type or a reserved-instance offering out of the emulator's answer picks something AWS does not offer, and the emulator hides everything AWS does offer. Cause 4 makes the package-version surface unusable at all: a client cannot discover any version of any catalog package. The per-filter causes 6-9 additionally mean the capability flags (WarmEnabled, EncryptionEnabled) do not describe the instance types they are attached to, so capability-based selection chooses types that cannot do the job.

Fix: the static reference data behind these readers (package catalog with its per-version history, reserved-instance offering table, upgrade-compatibility matrix, instance-type catalog with its AZ/role/capability attributes, engine-version table) and the capability filtering applied to ListInstanceTypeDetails.

## Evidence
Cause 1: "$.PackageDetailsList[0]: missing (expected {... 'PackageID': 'pkg-10b3c5c4ab016794431c100a4963d8d1e07e3673', 'PackageName': 'babel-street-match', 'PackageStatus': 'AVAILABLE', 'PackageType': 'ZIP-PLUGIN' ...}, got None)" for indices 0..92.
Cause 2: "$.ReservedInstanceOfferings[0]: missing (expected {...'InstanceType': 'c7i.xlarge.search', 'PaymentOption': 'ALL_UPFRONT', 'RecurringCharges': [{'RecurringChargeAmount': 0.0, 'RecurringChargeFrequency': 'Hourly'}]...}, got {...'InstanceType': 'm4.4xlarge.search', 'RecurringCharges': []})".
Cause 3: "$.CompatibleVersions[0]: missing (expected SourceVersion 'Elasticsearch_7.7' with 7 targets, got SourceVersion 'Elasticsearch_1.5' with 18 targets)".
Cause 4: "status: expected 200, got 400", "$.PackageID: missing (expected 'pkg-10b3c5c4...', got None)", "$.PackageVersionHistoryList: missing (expected [v2, v1], got None)" - affected_count 90.
Cause 5: "$.InstanceTypeDetails[0]: missing (expected {... 'AvailabilityZones': None, 'InstanceRole': ['data','master','coordinator'], 'InstanceType': 'r5.large.search'...}, got {'InstanceType': 'c4.2xlarge.search', 'InstanceRole': ['data','master']})".
Causes 6-9: same shape, plus "'WarmEnabled': True" expected against "'WarmEnabled': False" actual on every entry.
Cause 10: "$.Versions[0]: missing (expected 'OpenSearch_3.7', got 'Elasticsearch_1.5')".

## Steps to Reproduce
1. Replay opensearch-catalog-describe-packages step task#1 and diff PackageDetailsList against the recording.
2. Replay opensearch-catalog-get-package-version-history step task#1: every catalog PackageID answers 400 instead of 200 with its version history.
3. Replay opensearch-catalog-list-instance-type-details steps task#1, task#6, task#7, task#13 and task#16 (unfiltered and each capability filter), opensearch-catalog-list-versions step task#1, opensearch-catalog-get-compatible-versions step task#1 and opensearch-catalog-describe-reserved-instance-offerings step task#1.
