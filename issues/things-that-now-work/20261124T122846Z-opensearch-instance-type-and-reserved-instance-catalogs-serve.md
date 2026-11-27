Issue-Source: trace-sweep-divergence
Issue-Trace: opensearch-catalog-list-instance-type-details
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: opensearch-catalog-list-instance-type-details opensearch-catalog-describe-reserved-instance-offerings
Issue-Shape-Id: com.amazonaws.opensearch#AmazonOpenSearchService
Issue-Key: opensearch|ListInstanceTypeDetails|2xx->2xx|$.InstanceTypeDetails[*]
Issue-Key: opensearch|DescribeReservedInstanceOfferings|2xx->2xx|$.ReservedInstanceOfferings[*]
Issue-Commit: 5834ad3c3ba75550acae8591eeb4264fb69879c4

# opensearch instance-type and reserved-instance catalogs serve a stale synthetic list, not AWS's published catalog (6 causes)

> _Note: this issue was identified against the code base at commit `5834ad3c3ba75550acae8591eeb4264fb69879c4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListInstanceTypeDetails

## Description
This bundle carries SIX net-new causes on the opensearch published-catalog read surface: five ListInstanceTypeDetails request shapes and one DescribeReservedInstanceOfferings. Every one answers 200 -> 200 and diverges only in the catalog it serves. A fixer must close all six before submitting.

Shared fault: both catalogs are synthesized from a small hard-coded table of legacy instance types instead of reproducing AWS's published catalog, and neither is keyed on the request's engine version or filters, so the same stale list comes back for every request shape.

Cause 1 of 6 - ListInstanceTypeDetails for the fixture's OpenSearch engine version (fixture opensearch-catalog-list-instance-type-details, step task#1; cluster also covers task#2..task#5).
AWS: Smithy com.amazonaws.opensearch#InstanceTypeDetails{InstanceType: OpenSearchPartitionInstanceType (enum), EncryptionEnabled, CognitoEnabled, AppLogsEnabled, AdvancedSecurityEnabled, WarmEnabled: Boolean, InstanceRole: InstanceRoleList, AvailabilityZones: AvailabilityZoneList}, documented "Lists all instance types and available features for a given OpenSearch or Elasticsearch version". Recorded behavior: for a current OpenSearch version AWS returns the full published catalog in its own order, led by current-generation families (m7i, c7i/c8g, r7i/r7g/r8g, or1/or2, oi2, om2, i7i, i8g/i8ge) alongside the legacy ones, each entry's capability flags and InstanceRole describing that type (e.g. oi2.8xlarge.search has InstanceRole ['data'] only).
Emulator: it returns a shorter list of legacy c5/i3/m5-generation types in its own order, with uniform capability flags and InstanceRole almost always ['data','master','coordinator']; no current-generation type appears at any index.
Why: a client that enumerates supported instance types for a version - to validate a requested type, size a domain or render a picker - is offered types AWS does not publish for that version and cannot see any current-generation type at all; and because InstanceRole/WarmEnabled/EncryptionEnabled are synthesized, a client that gates dedicated-master, UltraWarm or encryption support on them decides wrongly.
Fix: the ListInstanceTypeDetails handler's catalog data (the instance-type table it reads) and its ordering.
Traces: opensearch-catalog-list-instance-type-details#task#1.

Cause 2 of 6 - ListInstanceTypeDetails, second request shape (same fixture, step task#6).
AWS: as cause 1; the recorded response for this request shape is again AWS's published list for the requested version, with the same current-generation head.
Emulator: the same stale legacy list as cause 1, i.e. the response does not vary with the request.
Why: as cause 1 - and a client that narrows the catalog by request parameters learns nothing, because every narrowing returns the same list.
Fix: as cause 1, plus honouring the request's selection parameters when choosing catalog rows.
Traces: opensearch-catalog-list-instance-type-details#task#6.

Cause 3 of 6 - ListInstanceTypeDetails for the FGAC-off request shape (same fixture, step task#7; cluster also covers task#8..task#12).
AWS: as cause 1; in the recording the per-entry AdvancedSecurityEnabled is false for these rows while the instance-type set is still AWS's published set.
Emulator: the stale legacy list again; AdvancedSecurityEnabled follows the request but the instance types, their order and their InstanceRole values do not match the published catalog.
Why: as cause 1.
Fix: as cause 1.
Traces: opensearch-catalog-list-instance-type-details#task#7.

Cause 4 of 6 - ListInstanceTypeDetails, fourth request shape (same fixture, step task#13; cluster also covers task#14, task#15).
AWS: as cause 1.
Emulator: the same stale legacy list, so the entries at every asserted index differ from the recorded catalog.
Why: as cause 1.
Fix: as cause 1.
Traces: opensearch-catalog-list-instance-type-details#task#13.

Cause 5 of 6 - ListInstanceTypeDetails for a legacy Elasticsearch engine version (same fixture, step task#16; cluster also covers task#17): the catalog is not keyed on engine version at all.
AWS: as cause 1 - the catalog is per-version, so an old Elasticsearch version lists the generation available to it (r4/r3/c4/m4-class types) with that generation's capabilities, e.g. r4.2xlarge.search with EncryptionEnabled false and InstanceRole ['data','master'].
Emulator: it serves the SAME c5-generation list it serves for the current OpenSearch version, with EncryptionEnabled true and InstanceRole ['data','master','coordinator'].
Why: a client that asks which instance types an older engine version supports is told it can use types that version cannot run, and is told encryption at rest is supported where AWS reports it is not - so a CreateDomain built from the answer is refused.
Fix: key the ListInstanceTypeDetails catalog on the requested engine version (and the domain, when the request is domain-scoped), and record each generation's real capability flags.
Traces: opensearch-catalog-list-instance-type-details#task#16.

Cause 6 of 6 - DescribeReservedInstanceOfferings (fixture opensearch-catalog-describe-reserved-instance-offerings, step task#1): the offerings catalog is generated from one instance type and one term.
AWS: Smithy com.amazonaws.opensearch#ReservedInstanceOffering{ReservedInstanceOfferingId: GUID (36 chars, uuid pattern), InstanceType: OpenSearchPartitionInstanceType, Duration: Integer (seconds), FixedPrice/UsagePrice: Double, CurrencyCode: String, PaymentOption: ReservedInstancePaymentOption, RecurringCharges: RecurringChargeList}, documented "Details of an OpenSearch Reserved Instance offering". Recorded behavior: AWS publishes offerings spanning many instance types (c7i.xlarge.search, or1.12xlarge.search, r8g.16xlarge.search, ...), BOTH the 1-year and the 3-year term (Duration 31536000 and 94608000), each of the ALL_UPFRONT / PARTIAL_UPFRONT / NO_UPFRONT payment options, each with that offering's own published FixedPrice and RecurringCharges, under its own published offering id.
Emulator: every offering it returns is for a single instance type (c5.large.search) with Duration 31536000 only, prices derived from its own formula, and ids generated by the emulator; no 3-year offering and no other instance type appears anywhere in the list.
Why: a client that lists offerings to choose a reservation cannot find the instance type or the 3-year term it wants, and the prices it shows or budgets against are invented; PurchaseReservedInstanceOffering can then only ever be driven with an offering id that corresponds to nothing AWS publishes.
Fix: the DescribeReservedInstanceOfferings handler's offering catalog data (cover the published instance types, both durations and all three payment options, with per-offering prices and recurring charges) and its paging over that catalog.
Traces: opensearch-catalog-describe-reserved-instance-offerings#task#1.

Related open issues that do NOT cover these clusters (the replay reports owned_by empty for all six): 20261117T201942Z-es-instance-type-and-service-software-catalogs-are.md owns the es-endpoint instance-type/service-software catalog clusters, not the opensearch ones; 20261117T190848Z-opensearch-listinstancetypedetails-re-words-the-az-unsupported-instancetype.md is about the wording of a ListInstanceTypeDetails refusal, not the catalog it serves on success; 20261122T115902Z-opensearch-the-published-package-catalog-still-answers-neither.md covers the package catalog (DescribePackages).

## Evidence
Replayed corpus simparity-wire-aws-v1 (emulator_up=true, 492 steps, 139 mismatched). All six clusters: verdict mismatch, expected status == actual status (200), divergence in the list body.

Cause 1 - opensearch-catalog-list-instance-type-details#task#1 (affects task#1..task#5), operation ListInstanceTypeDetails:
  $.InstanceTypeDetails[1]: expected {'InstanceType': 'm7i.2xlarge.search', 'InstanceRole': ['data','master','coordinator'], 'WarmEnabled': True, ...}, got {'InstanceType': 'c5.xlarge.search', ...}
  $.InstanceTypeDetails[6]: expected 'r7i.12xlarge.search', got 'i3.large.search'
  $.InstanceTypeDetails[9]: expected 'c7i.16xlarge.search', got 'i3.4xlarge.search'
  $.InstanceTypeDetails[13]: expected 'i7i.4xlarge.search' with InstanceRole ['data'], got 'm5.xlarge.search' with InstanceRole ['data','master','coordinator']
  ~60 further indices diverge the same way.

Cause 2 - opensearch-catalog-list-instance-type-details#task#6: same expected head (m7i.2xlarge.search / r7i.12xlarge.search / c7i.16xlarge.search / i7i.4xlarge.search), same got head (c5.xlarge.search / i3.large.search / i3.4xlarge.search / m5.xlarge.search); ~57 indices diverge.

Cause 3 - opensearch-catalog-list-instance-type-details#task#7 (affects task#7..task#12): identical instance-type divergence with AdvancedSecurityEnabled False on both sides; ~54 indices diverge.

Cause 4 - opensearch-catalog-list-instance-type-details#task#13 (affects task#13..task#15): identical instance-type divergence; ~53 indices diverge.

Cause 5 - opensearch-catalog-list-instance-type-details#task#16 (affects task#16, task#17), legacy engine version:
  $.InstanceTypeDetails[0]: expected {'InstanceType': 'r4.2xlarge.search', 'EncryptionEnabled': False, 'InstanceRole': ['data','master'], ...}, got {'InstanceType': 'c5.large.search', 'EncryptionEnabled': True, 'InstanceRole': ['data','master','coordinator'], ...}
  $.InstanceTypeDetails[1]: expected 'r3.4xlarge.search', got 'c5.xlarge.search'
  $.InstanceTypeDetails[3]: expected 'c4.2xlarge.search', got 'c5.4xlarge.search'
  $.InstanceTypeDetails[5]: expected 'm4.xlarge.search', got 'c5.18xlarge.search'
  all 62 indices diverge - the response is the same list the current-version requests return.

Cause 6 - opensearch-catalog-describe-reserved-instance-offerings#task#1, operation DescribeReservedInstanceOfferings:
  $.ReservedInstanceOfferings[0]: expected {'InstanceType': 'c7i.xlarge.search', 'Duration': 94608000, 'FixedPrice': 3602.673, 'PaymentOption': 'ALL_UPFRONT', 'RecurringCharges': [{'RecurringChargeAmount': 0.0, ...}]}, got {'InstanceType': 'c5.large.search', 'Duration': 31536000, 'FixedPrice': 854.92, 'PaymentOption': 'ALL_UPFRONT', ...}
  $.ReservedInstanceOfferings[1]: expected {'InstanceType': 'or1.12xlarge.search', 'Duration': 94608000, 'PaymentOption': 'NO_UPFRONT', 'RecurringCharges': [{'RecurringChargeAmount': 2.61, ...}]}, got {'InstanceType': 'c5.large.search', 'Duration': 31536000, 'RecurringCharges': [{'RecurringChargeAmount': 0.10608, ...}]}
  $.ReservedInstanceOfferings[2]: expected {'InstanceType': 'r8g.16xlarge.search', 'Duration': 31536000, 'FixedPrice': 18367.6615, 'PaymentOption': 'PARTIAL_UPFRONT'}, got {'InstanceType': 'c5.large.search', 'FixedPrice': 446.05, ...}
  the divergence runs over ~1344 indices; no index carries an instance type other than c5.large.search and no index carries Duration 94608000.

Per-run values: the individual prices, the offering GUIDs and the exact index positions of this recording are illustrations, not the requirement. The requirements are: the instance-type catalog is AWS's published per-engine-version catalog (set, order and per-type capability flags/InstanceRole), and the offerings catalog spans the published instance types, both the 1-year (31536000 s) and 3-year (94608000 s) durations and all three payment options, each with its own fixed/recurring charges.

Smithy cited: com.amazonaws.opensearch#InstanceTypeDetails{InstanceType: OpenSearchPartitionInstanceType, EncryptionEnabled, CognitoEnabled, AppLogsEnabled, AdvancedSecurityEnabled, WarmEnabled, InstanceRole: InstanceRoleList, AvailabilityZones: AvailabilityZoneList}; com.amazonaws.opensearch#ReservedInstanceOffering{ReservedInstanceOfferingId: GUID, InstanceType, Duration: Integer, FixedPrice, UsagePrice, CurrencyCode, PaymentOption: ReservedInstancePaymentOption, RecurringCharges: RecurringChargeList}.

## Steps to Reproduce
1. git_clone, then replay_wire_corpus against the emulator built from the checkout.
2. Causes 1-4: replay fixture opensearch-catalog-list-instance-type-details steps task#1, task#6, task#7 and task#13 - each is a ListInstanceTypeDetails for a current OpenSearch engine version with a different request shape. Compare $.InstanceTypeDetails element by element with the recorded catalog: the current-generation families (m7i/c7i/c8g/r7i/r7g/r8g/or1/or2/oi2/om2/i7i/i8g/i8ge) must be present at AWS's positions, with AWS's per-type InstanceRole and capability flags.
3. Cause 5: replay step task#16 - the same operation for a legacy Elasticsearch engine version. The response must be that version's catalog (r4/r3/c4/m4-class, EncryptionEnabled false, InstanceRole ['data','master']), not the c5-generation list the current-version requests return.
4. Cause 6: replay fixture opensearch-catalog-describe-reserved-instance-offerings step task#1 (DescribeReservedInstanceOfferings, unfiltered). The page must contain offerings for several instance types and both Duration 31536000 and 94608000 across ALL_UPFRONT / PARTIAL_UPFRONT / NO_UPFRONT, with per-offering FixedPrice and RecurringCharges - not one synthesized instance type and term.
5. All six steps must pass against the recorded responses.

## Resolution

Three rules were wrong, and each is fixed where it is derived once:

1. `InstanceTypeDetails$EncryptionEnabled` reports encryption at rest TOGETHER WITH
   node-to-node encryption, and each of the two needs both an instance family that
   provides it and an engine release that has it — AWS documents encryption at rest from
   Elasticsearch 5.1 and node-to-node encryption from 6.0 — so a release older than the
   later of the two now reports the pair unsupported whatever the family
   (`state::offers_encryption_on`), which is what the recorded listing of an old
   Elasticsearch release carries for a family a current release reports it on.
   Fine-grained access control, which requires both, is therefore never reported where
   they are not. The capabilities the INSTANCE provides (the warm tier) and the ones the
   ENGINE provides (Cognito authentication, application logging) are unchanged.
2. `InstanceRole` is the roles an instance of the entry's FAMILY can be provisioned in, so
   a family the service publishes for the data tier alone reports the data role and
   nothing else: that is the OpenSearch-optimized families, and now also the
   storage-optimized generations that arrived after the engines forked (`i7i`, `i8g`
   beside `i8ge`), as the recorded catalog of a current release reports them. Because the
   roles have one derivation (`state::instance_roles`), the limit catalog publishes a
   block for exactly those roles too.
3. `MaxResults` is the bound the REQUEST names. `DescribeReservedInstanceOfferings`
   bounded a request that named none by a page size the emulator chose for itself, so
   nothing past the first hundred offerings was reported at all; the member is
   `@default(0)`, so such a request names no bound and the whole listing it selected is
   now reported in one answer with no token, as the recorded unfiltered answer carries
   every offering the Region publishes and as this service's sibling catalog reads
   already answer.

Held by `tests/all/published_catalog_contents.rs`, with the existing
`instance_type_catalog`, `instance_type_capability_flags`,
`current_generation_instance_types`, `reserved_instance_offerings`,
`reserved_offering_service_page`, `unbounded_catalog_reads` and `pagination_contract`
suites restated against the corrected rules.

DECLINED, as in 20261114T093806Z: the ORDER each of these sets is published in, which is
what both replay keys still report, and the per-offering price figures and
`ReservedInstanceOfferingId` values. `InstanceTypeDetailsList` and
`ReservedInstanceOfferingList` carry no ordering trait and no documented order, and no
value either entry carries orders the recorded listings — the offering listing interleaves
instance type, term and payment option, and the instance-type listing interleaves families
and sizes — so a recorded position is a function of AWS's own collection and of the
offering ids it mints, neither of which this API publishes. This issue says the same of
them: "the individual prices, the offering GUIDs and the exact index positions of this
recording are illustrations, not the requirement". Both catalogs are published in an order
that is a function of the catalog alone and is stable across calls, and the offering
catalog spans every instance type the service publishes times both published terms times
all three payment options, each with its own fixed price and its own recurring charge —
which is the requirement this issue states.
