Issue-Source: trace-sweep-divergence
Issue-Trace: opensearch-catalog-list-instance-type-details
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: opensearch-catalog-list-instance-type-details opensearch-catalog-get-compatible-versions opensearch-catalog-describe-reserved-instance-offerings
Issue-Shape-Id: com.amazonaws.opensearch#AmazonOpenSearchService
Issue-Key: opensearch|ListInstanceTypeDetails|2xx->2xx|$.InstanceTypeDetails[*]
Issue-Key: opensearch|GetCompatibleVersions|2xx->2xx|$.CompatibleVersions[*]
Issue-Key: opensearch|DescribeReservedInstanceOfferings|2xx->2xx|$.ReservedInstanceOfferings[*]
Issue-Commit: 7ab16e4596463d769159d2478462aef7024df8a6

# opensearch instance-type, engine-version and reserved-instance catalogs serve stale emulator-authored data instead of AWS's

> _Note: this issue was identified against the code base at commit `7ab16e4596463d769159d2478462aef7024df8a6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListInstanceTypeDetails

## Description
Every read-only catalog operation on the opensearch service answers 200 but with an emulator-authored dataset built from a legacy Elasticsearch-era instance/version/pricing list, not the catalog AWS serves. The responses are structurally valid and therefore silently wrong. 7 causes, all MEDIUM.

Cause 1 of 7: ListInstanceTypeDetails (unfiltered, current engine version) returns the wrong instance catalog.
AWS: Smithy: ListInstanceTypeDetails GET /2021-01-01/opensearch/instanceTypeDetails/{EngineVersion}, success 200, output InstanceTypeDetails (list of InstanceTypeDetails), paginated with MaxResults @range max 100. AWS returns the instance types actually orderable for that engine version, each entry carrying InstanceType (pattern ^.*\..*\.search$), InstanceRole, and the capability flags EncryptionEnabled, CognitoEnabled, AppLogsEnabled, AdvancedSecurityEnabled, WarmEnabled, plus an AvailabilityZones member (null unless RetrieveAZs was requested). The recorded catalog is the current generation: r5/m7i/c7i/c8g/r7i/r8g/oi2/om2/or1/or2/i7i/i8g/i8ge families.
Emulator: returns a stale legacy set — c5.large/c5.xlarge/c5.2xlarge/c5.4xlarge/c5.9xlarge/c5.18xlarge, i3.*, m5.*, m4.*, r4.*, r3.*, c4.* — in a different order, and omits the AvailabilityZones member from every entry. No index of InstanceTypeDetails matches the recording.
Why: a client sizing a domain picks an instance type from this list; none of the current-generation types AWS offers are discoverable, and types the emulator offers are not orderable on AWS. Omitting AvailabilityZones also changes the shape clients destructure.
Fix: the instance-type catalog seed/state behind ListInstanceTypeDetails, and its serializer (AvailabilityZones must be emitted).
Traces: opensearch-catalog-list-instance-type-details#task#1 (5 affected steps, task#1..task#5).

Cause 2 of 7: ListInstanceTypeDetails reports WarmEnabled=false for instance types AWS marks warm-capable.
AWS: recorded behavior only (the model types WarmEnabled as a plain boolean with no documented rule). AWS reports WarmEnabled=true for the data/master-capable types in the recorded catalog.
Emulator: reports WarmEnabled=false for every entry in this cluster, alongside the same stale instance-type list as Cause 1.
Why: a client that filters the catalog on WarmEnabled to decide whether UltraWarm can be configured concludes no instance type supports warm storage, so the whole warm-tier configuration path is undiscoverable.
Fix: the per-entry capability-flag derivation in the ListInstanceTypeDetails handler/state.
Traces: opensearch-catalog-list-instance-type-details#task#6.

Cause 3 of 7: ListInstanceTypeDetails capability flags diverge for the AdvancedSecurityEnabled=false projection.
AWS: recorded behavior only. For the recorded request AWS returns the current-generation catalog with AdvancedSecurityEnabled=false but AppLogsEnabled, CognitoEnabled, EncryptionEnabled and WarmEnabled all true.
Emulator: returns the stale c5/legacy list with WarmEnabled=false, so both the instance types and the flag combination diverge on every one of the ~90 indices.
Why: the catalog is the only machine-readable source for which features a given instance type supports; a client reading it gets a feature matrix that does not match AWS, and any capability precondition it derives is wrong.
Fix: the ListInstanceTypeDetails handler's capability projection and the catalog it draws from.
Traces: opensearch-catalog-list-instance-type-details#task#7 (6 affected steps, task#7..task#12).

Cause 4 of 7: ListInstanceTypeDetails reports EncryptionEnabled=false for types AWS reports as encryption-capable.
AWS: recorded behavior only. For the recorded request AWS returns EncryptionEnabled=true (with AdvancedSecurityEnabled=false, AppLogsEnabled=true, CognitoEnabled=true, WarmEnabled=true) across the current-generation catalog.
Emulator: reports EncryptionEnabled=false and WarmEnabled=false, again over the stale legacy instance list.
Why: a client gating encryption-at-rest on this flag concludes the instance type cannot be encrypted and skips/refuses a configuration AWS accepts.
Fix: as Cause 3 — the capability projection in the ListInstanceTypeDetails handler.
Traces: opensearch-catalog-list-instance-type-details#task#13 (3 affected steps, task#13..task#15).

Cause 5 of 7: ListInstanceTypeDetails with the filtering/AZ query parameters still returns the stale catalog.
AWS: Smithy: ListInstanceTypeDetails input members DomainName (httpQuery domainName), InstanceType (httpQuery instanceType, @length 10..40, pattern ^.*\..*\.search$) and RetrieveAZs (httpQuery retrieveAZs) narrow/annotate the result. AWS returns the matching subset of the current catalog, led by r4.2xlarge.search / r3.4xlarge.search in the recording, with WarmEnabled=true and InstanceRole ['data','master'] for those entries.
Emulator: returns the same stale c5-led list regardless, with WarmEnabled=false and InstanceRole ['data','master','coordinator'], so the filtered read diverges at every index.
Why: the query parameters appear to work (200, well-formed list) but neither the membership nor the per-entry roles reflect the filter, so a client's narrowed catalog read is wrong in exactly the case it narrowed for.
Fix: the ListInstanceTypeDetails handler's query-parameter filtering and the catalog state.
Traces: opensearch-catalog-list-instance-type-details#task#16 (2 affected steps, task#16, task#17).

Cause 6 of 7: GetCompatibleVersions returns an upgrade matrix that is not AWS's.
AWS: Smithy: GetCompatibleVersions GET /2021-01-01/opensearch/compatibleVersions, success 200, output CompatibleVersions (list of CompatibleVersionsMap with SourceVersion and TargetVersions). AWS publishes one entry per upgradable source engine version, in its own order, with the exact set of in-place upgrade targets for each — the recording leads with Elasticsearch_7.7 and Elasticsearch_6.8 and includes Elasticsearch_5.x/6.x sources whose targets are the Elasticsearch 5.6/6.x line.
Emulator: returns a differently-ordered and differently-populated matrix — it leads with Elasticsearch_5.1 -> [Elasticsearch_5.3, 5.5, 5.6], drops the Elasticsearch_6.0 and Elasticsearch_5.6 -> 6.x rows AWS publishes, and inserts OpenSearch_2.3/2.5/2.11/3.3 rows at the recorded indices. Entries diverge at $.CompatibleVersions[0], [1], [3], [4], [21], [22], [25], [31].
Why: this matrix is the API a client uses to decide whether a target version is a legal in-place UpgradeDomain target. Reading the emulator's matrix, a client both believes in upgrade paths AWS refuses and misses paths AWS allows, so upgrade planning is wrong in both directions.
Fix: the version-compatibility table behind the GetCompatibleVersions handler.
Traces: opensearch-catalog-get-compatible-versions#task#1.

Cause 7 of 7: DescribeReservedInstanceOfferings mints synthetic single-instance-type offerings.
AWS: Smithy: DescribeReservedInstanceOfferings GET /2021-01-01/opensearch/reservedInstanceOfferings, success 200, output ReservedInstanceOfferings (list of ReservedInstanceOffering: ReservedInstanceOfferingId as a GUID @length 36 matching ^\p{XDigit}{8}-\p{XDigit}{4}-\p{XDigit}{4}-\p{XDigit}{4}-\p{XDigit}{12}$, InstanceType, Duration, FixedPrice, UsagePrice, CurrencyCode, PaymentOption, RecurringCharges), paginated. AWS returns offerings spanning the whole orderable instance catalog and both term lengths (1-year = 31536000 and 3-year = 94608000 seconds) across ALL_UPFRONT / PARTIAL_UPFRONT / NO_UPFRONT, with per-type prices and, for NO_UPFRONT/PARTIAL_UPFRONT, a non-zero RecurringChargeAmount.
Emulator: every offering it returns is for one instance type (c5.large.search) with Duration 31536000 only, synthetic offering ids, and prices/recurring charges that do not vary by instance type — so no index of ReservedInstanceOfferings matches, over 1300+ recorded entries.
Why: a client cannot discover a reserved-instance offering for any instance type it would actually run, and cannot discover 3-year terms at all; the offering ids it reads back cannot be purchased, so the PurchaseReservedInstanceOffering path is unreachable for a realistic plan.
Fix: the reserved-instance offering catalog seed/state behind the DescribeReservedInstanceOfferings handler.
Traces: opensearch-catalog-describe-reserved-instance-offerings#task#1.

Note: the open issues 20261114T061447Z-es-instance-type-catalog-is-a-stale-legacy.md and 20261030T180653Z-es-the-engine-version-compatibility-matrix-and-reserved.md describe the same family of defect on the legacy `es` service token; the replay reports owned_by=[] for all seven clusters above, so they are not covered by those issues. A fixer should expect the opensearch and es catalogs to share a backing table.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout. All seven clusters: verdict=mismatch, status 200 vs 200, stub_path="", owned_by=[].

Cause 1 — opensearch-catalog-list-instance-type-details#task#1 (5 steps):
  $.InstanceTypeDetails[1]: expected {InstanceType 'm7i.2xlarge.search', WarmEnabled true, AvailabilityZones null, InstanceRole ['data','master','coordinator'], ...}, got {InstanceType 'c5.xlarge.search', ... no AvailabilityZones member}
  $.InstanceTypeDetails[6]: expected 'r7i.12xlarge.search', got 'i3.large.search'
  $.InstanceTypeDetails[9]: expected 'c7i.16xlarge.search', got 'i3.4xlarge.search'
  $.InstanceTypeDetails[13]: expected 'i7i.4xlarge.search', got 'm5.xlarge.search'

Cause 2 — opensearch-catalog-list-instance-type-details#task#6:
  $.InstanceTypeDetails[0]: expected {InstanceType 'r5.large.search', WarmEnabled true}, got {InstanceType 'c5.large.search', WarmEnabled false}
  (every index [0]..[85] diverges)

Cause 3 — opensearch-catalog-list-instance-type-details#task#7 (6 steps):
  $.InstanceTypeDetails[0]: expected {InstanceType 'r5.large.search', AdvancedSecurityEnabled false, AppLogsEnabled true, WarmEnabled true}, got {InstanceType 'c5.large.search', AdvancedSecurityEnabled false, WarmEnabled false}
  (every index [0]..[89] diverges)

Cause 4 — opensearch-catalog-list-instance-type-details#task#13 (3 steps):
  $.InstanceTypeDetails[0]: expected {InstanceType 'r5.large.search', EncryptionEnabled true, WarmEnabled true}, got {InstanceType 'c5.large.search', EncryptionEnabled false, WarmEnabled false}

Cause 5 — opensearch-catalog-list-instance-type-details#task#16 (2 steps):
  $.InstanceTypeDetails[0]: expected {InstanceType 'r4.2xlarge.search', InstanceRole ['data','master'], WarmEnabled true}, got {InstanceType 'c5.large.search', InstanceRole ['data','master','coordinator'], WarmEnabled false}
  $.InstanceTypeDetails[1]: expected 'r3.4xlarge.search', got 'c5.xlarge.search'

Cause 6 — opensearch-catalog-get-compatible-versions#task#1:
  $.CompatibleVersions[0]: expected {SourceVersion 'Elasticsearch_7.7', TargetVersions [7.8, 7.9, 7.10, OpenSearch_1.0..1.3]}, got {SourceVersion 'Elasticsearch_5.1', TargetVersions ['Elasticsearch_5.3','Elasticsearch_5.5','Elasticsearch_5.6']}
  $.CompatibleVersions[21]: expected {SourceVersion 'Elasticsearch_6.0', TargetVersions [6.3, 6.4, 6.5, 6.7, 6.8]}, got {SourceVersion 'OpenSearch_2.3', ...}
  $.CompatibleVersions[22]: expected {SourceVersion 'Elasticsearch_5.1', TargetVersions ['Elasticsearch_5.6']}, got {SourceVersion 'OpenSearch_2.5', ...}
  $.CompatibleVersions[31]: expected {SourceVersion 'Elasticsearch_5.6', TargetVersions [6.3..6.8]}, got {SourceVersion 'OpenSearch_3.3', TargetVersions ['OpenSearch_3.5','OpenSearch_3.7']}

Cause 7 — opensearch-catalog-describe-reserved-instance-offerings#task#1:
  $.ReservedInstanceOfferings[0]: expected {InstanceType 'c7i.xlarge.search', Duration 94608000, PaymentOption 'ALL_UPFRONT'}, got {InstanceType 'c5.large.search', Duration 31536000, PaymentOption 'ALL_UPFRONT'}
  $.ReservedInstanceOfferings[1]: expected {InstanceType 'or1.12xlarge.search', Duration 94608000, PaymentOption 'NO_UPFRONT', RecurringCharges non-zero}, got {InstanceType 'c5.large.search', Duration 31536000, ...}
  $.ReservedInstanceOfferings[2]: expected {InstanceType 'r8g.16xlarge.search', Duration 31536000, PaymentOption 'PARTIAL_UPFRONT'}, got {InstanceType 'c5.large.search', ...}
  divergence_key enumerates $.ReservedInstanceOfferings[0] .. [1319]+ — no element matches

(Instance-type names, offering ids, prices and timestamps above are the recorded examples; the requirement is the catalog's membership, ordering and per-entry capability/term semantics, not these literal values.)

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. GET /2021-01-01/opensearch/instanceTypeDetails/{EngineVersion} for a current OpenSearch engine version, with and without the domainName / instanceType / retrieveAZs query parameters — fixture opensearch-catalog-list-instance-type-details, steps task#1, task#6, task#7, task#13, task#16. Compare InstanceTypeDetails membership, ordering, the AvailabilityZones member and the WarmEnabled / EncryptionEnabled / AdvancedSecurityEnabled flags against the recording.
3. GET /2021-01-01/opensearch/compatibleVersions — fixture opensearch-catalog-get-compatible-versions, step task#1. Compare the CompatibleVersions rows and their TargetVersions sets against the recording.
4. GET /2021-01-01/opensearch/reservedInstanceOfferings — fixture opensearch-catalog-describe-reserved-instance-offerings, step task#1. Check that offerings cover more than one instance type and include the 3-year (94608000 s) term.
5. replay_wire_corpus reports all seven clusters as mismatches with stub_path="" and owned_by=[].
