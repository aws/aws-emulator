Issue-Source: trace-sweep-divergence
Issue-Trace: opensearch-catalog-list-instance-type-details
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: opensearch-catalog-list-instance-type-details opensearch-catalog-describe-reserved-instance-offerings
Issue-Shape-Id: com.amazonaws.opensearch#AmazonOpenSearchService
Issue-Key: opensearch|ListInstanceTypeDetails|2xx->2xx|$.InstanceTypeDetails[*]
Issue-Key: opensearch|DescribeReservedInstanceOfferings|2xx->2xx|$.ReservedInstanceOfferings[*]
Issue-Commit: 7f3d8acfd1d18a62a70e68f35eeb77bb7bd3316f

# opensearch published catalogs still diverge: engine-version keying, encryption flag and catalog order (7 causes)

> _Note: this issue was identified against the code base at commit `7f3d8acfd1d18a62a70e68f35eeb77bb7bd3316f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListInstanceTypeDetails

## Description
SEVEN net-new causes on the opensearch-token published-catalog read surface (service token `opensearch`, Smithy service com.amazonaws.opensearch#AmazonOpenSearchService): six ListInstanceTypeDetails request shapes and one DescribeReservedInstanceOfferings. Every cluster answers 200 -> 200 and diverges only in the catalog that is served. A fixer MUST close all SEVEN causes before submitting.

Shared fault: the two published catalogs the emulator serves are still generated locally rather than reproducing the catalog AWS publishes - the instance-type listing is not keyed on the request's engine version, one of its per-entry capability flags is derived from a rule AWS's recordings contradict, and both listings are emitted in an order of the emulator's own choosing. Because BOTH operations are @paginated with pageSize MaxResults (max 100), the order is what decides which rows a client ever sees.

Smithy grounding (smithy_query, model_path services/service-com-amazonaws-opensearch-amazonopensearchservice/smithy-model/com-amazonaws-opensearch-amazonopensearchservice.json):
  ListInstanceTypeDetails: GET /2021-01-01/opensearch/instanceTypeDetails/{EngineVersion}, success 200; EngineVersion @required @httpLabel -> VersionString (length 14..18, pattern ^Elasticsearch_[0-9]{1}\.[0-9]{1,2}$|^OpenSearch_[0-9]{1,2}\.[0-9]{1,2}$); DomainName/MaxResults(default 0, range max 100)/NextToken/RetrieveAZs/InstanceType are @httpQuery; output InstanceTypeDetails -> InstanceTypeDetailsList + NextToken; @paginated{inputToken NextToken, outputToken NextToken, pageSize MaxResults}; errors ValidationException(400), BaseException(400 defaulted), ResourceNotFoundException(409), InternalException(500). InstanceTypeDetails{InstanceType -> OpenSearchPartitionInstanceType enum, EncryptionEnabled, CognitoEnabled, AppLogsEnabled, AdvancedSecurityEnabled, WarmEnabled, InstanceRole -> InstanceRoleList, AvailabilityZones -> AvailabilityZoneList}.
  DescribeReservedInstanceOfferings: GET /2021-01-01/opensearch/reservedInstanceOfferings, success 200; ReservedInstanceOfferingId/MaxResults(default 0, range max 100)/NextToken are @httpQuery; output ReservedInstanceOfferings -> ReservedInstanceOfferingList + NextToken; @paginated as above. ReservedInstanceOffering{ReservedInstanceOfferingId -> GUID, InstanceType, Duration (seconds), FixedPrice, UsagePrice, CurrencyCode, PaymentOption -> ReservedInstancePaymentOption, RecurringCharges -> RecurringChargeList}.

Cause 1 of 7: ListInstanceTypeDetails for the fixture's current OpenSearch release publishes a different catalog sequence from AWS's (fixture opensearch-catalog-list-instance-type-details, step task#1; the cluster also covers task#2..task#5).
AWS: per the model above, 200 with InstanceTypeDetailsList. The SEQUENCE is recorded behavior only: the recording leads with current-generation families - index 1 m7i.2xlarge.search, index 6 r7i.12xlarge.search, index 9 c7i.16xlarge.search (InstanceRole ['data','master']), index 13 i7i.4xlarge.search (InstanceRole ['data'] only).
Emulator: answers 200 but puts legacy families at those positions - index 1 c5.xlarge.search, index 6 i3.large.search, index 9 i3.4xlarge.search, index 13 m5.xlarge.search (InstanceRole ['data','master','coordinator']) - with ~62 of the listed indices differing from the recording.
Why: this list is paginated and capped at 100 per page, so the order is load-bearing rather than cosmetic. A client that reads page 1 (or an index-addressed row) to validate a requested type, size a domain or render an instance picker is offered legacy types where AWS offers the current generation, and the per-entry InstanceRole it reads at a given index belongs to a different type than AWS reports there.
Fix: the catalog projection behind the ListInstanceTypeDetails handler in the opensearch service crate must publish rows in the order AWS publishes them, since that order is what paginates.
Traces: opensearch-catalog-list-instance-type-details#task#1 (service opensearch, verdict mismatch, 200 -> 200).

Cause 2 of 7: the second recorded request shape returns the same locally generated sequence (same fixture, step task#6).
AWS: as cause 1; the recorded reply for this request shape again leads with m7i.2xlarge.search / r7i.12xlarge.search / c7i.16xlarge.search / i7i.4xlarge.search.
Emulator: byte-identical head to cause 1 (c5.xlarge.search / i3.large.search / i3.4xlarge.search / m5.xlarge.search), i.e. the answer does not vary with the request's query parameters.
Why: a client that narrows the catalog with the operation's @httpQuery selectors learns nothing from the narrowing, because every shape returns the same list in the same order.
Fix: as cause 1, and honour the request's selectors (domainName/instanceType/retrieveAZs) when choosing and ordering rows.
Traces: opensearch-catalog-list-instance-type-details#task#6.

Cause 3 of 7: the FGAC-off request shape diverges the same way (same fixture, step task#7; the cluster also covers task#8..task#11).
AWS: as cause 1, with AdvancedSecurityEnabled false on these rows while the instance-type set and its order are still AWS's.
Emulator: AdvancedSecurityEnabled follows the request correctly (false on both sides) but the types at each index are again the c5/i3/m5 sequence, ~54 indices diverging.
Why: as cause 1.
Fix: as cause 1.
Traces: opensearch-catalog-list-instance-type-details#task#7.

Cause 4 of 7: for the request shape recorded at task#12 the emulator reports EncryptionEnabled false on every row where AWS reports it true (same fixture, step task#12).
AWS: InstanceTypeDetails.EncryptionEnabled is the modeled per-entry flag 'whether encryption is supported for this instance type on the requested engine version'. Recorded behavior: for this engine version every row carries EncryptionEnabled true (index 0 r5.large.search, index 1 m7i.2xlarge.search, index 2 r4.2xlarge.search with InstanceRole ['data','master'], index 3 c4.2xlarge.search), with AdvancedSecurityEnabled false.
Emulator: every row carries EncryptionEnabled FALSE (and the legacy c5 sequence: index 0 c5.large.search, 1 c5.xlarge.search, 2 c5.2xlarge.search, 3 c5.4xlarge.search, with InstanceRole ['data','master','coordinator'] where AWS reports ['data','master']). All 90 listed indices diverge.
Why: a client that gates encryption at rest / node-to-node encryption - or fine-grained access control, which requires both - on this flag concludes the engine version supports none of them, and refuses to build or silently downgrades a domain configuration AWS accepts. This is the opposite verdict from AWS's for every type in the listing.
Fix: the per-entry capability derivation in the opensearch crate's instance-type catalog (its encryption-support helper in the crate's state module) must report the support AWS reports for the requested engine version and family, not deny it for the whole listing. Note this is a regression of the rule adopted when 20261124T122846Z-opensearch-instance-type-and-reserved-instance-catalogs-serve.md was resolved (that resolution gated EncryptionEnabled on the engine release); the recording for this release reports true.
Traces: opensearch-catalog-list-instance-type-details#task#12.

Cause 5 of 7: the same false encryption flag and the same substituted sequence for the request shape recorded at task#13 (same fixture, step task#13; the cluster also covers task#14, task#15).
AWS: as cause 4 - index 0 r5.large.search, 1 m7i.2xlarge.search, 2 r4.2xlarge.search, 3 c4.2xlarge.search, all EncryptionEnabled true.
Emulator: the c5 sequence with EncryptionEnabled false, every listed index diverging.
Why: as cause 4; this is a second recorded request shape, so a client cannot avoid the wrong flag by varying the request.
Fix: as cause 4.
Traces: opensearch-catalog-list-instance-type-details#task#13.

Cause 6 of 7: for a LEGACY Elasticsearch engine release the listing is not keyed on the engine version at all - it serves rows AWS does not publish for that release (same fixture, step task#16; the cluster also covers task#17).
AWS: EngineVersion is an @required @httpLabel, so the catalog is per release: the recording for the legacy Elasticsearch release lists that generation - index 0 r4.2xlarge.search (EncryptionEnabled false, InstanceRole ['data','master'], WarmEnabled true), index 1 r3.4xlarge.search, index 2 t2.medium.search (WarmEnabled FALSE), index 3 c4.2xlarge.search.
Emulator: serves the same c5 generation it serves for the current OpenSearch releases - index 0 c5.large.search, 1 c5.xlarge.search, 2 c5.2xlarge.search, 3 c5.4xlarge.search - each with InstanceRole ['data','master','coordinator'] and WarmEnabled true. The replay classes these rows 'extra': they are rows AWS does not publish for this release, and the r4/r3/t2/c4 rows AWS does publish are absent.
Why: a client asking which types a legacy Elasticsearch release supports is told it may use types that release cannot run and that the warm tier is available on a type where AWS reports it is not; a CreateDomain built from that answer is refused by the real service, and a capability gate on WarmEnabled takes the wrong branch.
Fix: key the ListInstanceTypeDetails catalog (membership, per-entry flags and InstanceRole) on the EngineVersion path label in the opensearch crate's catalog projection, instead of emitting one global generation.
Traces: opensearch-catalog-list-instance-type-details#task#16.

Cause 7 of 7: DescribeReservedInstanceOfferings publishes the offerings catalog grouped by instance type and term instead of in AWS's published order, so page 1 shows one instance type only (fixture opensearch-catalog-describe-reserved-instance-offerings, step task#1).
AWS: per the model above, 200 with ReservedInstanceOfferingList. The ORDER is recorded behavior only: the recording interleaves instance type, term and payment option - index 0 c7i.xlarge.search Duration 94608000 ALL_UPFRONT, index 1 or1.12xlarge.search Duration 94608000 NO_UPFRONT (RecurringChargeAmount 2.61), index 2 r8g.16xlarge.search Duration 31536000 PARTIAL_UPFRONT - each row carrying its own published FixedPrice, RecurringCharges and offering id.
Emulator: emits the catalog type-major - the first three rows are all c5.large.search with Duration 31536000 under ALL_UPFRONT / NO_UPFRONT / PARTIAL_UPFRONT - so the divergence runs over the whole listing (the cluster's key lists more than a thousand indices).
Why: the operation is @paginated with MaxResults capped at 100 over a catalog of well over a thousand offerings, so the publication order decides what a client can reach. A client that lists offerings to choose a reservation, or that filters the first page client-side, sees exactly one instance family and one term where AWS's first page spans several families and both the 1-year (31536000 s) and 3-year (94608000 s) terms; a reservation purchase driven off page 1 therefore cannot name the offering AWS would have offered.
Fix: the offering catalog projection behind DescribeReservedInstanceOfferings in the opensearch crate must publish rows in the service's published order (interleaving type, term and payment option) rather than nesting type over term over payment option.
Traces: opensearch-catalog-describe-reserved-instance-offerings#task#1.

HISTORY, so the fixer can decide deliberately rather than re-discover: issue 20261124T122846Z-opensearch-instance-type-and-reserved-instance-catalogs-serve.md is RESOLVED and therefore owns none of these clusters (this replay reports owned_by empty for all seven). Its resolution fixed catalog membership, InstanceRole and the MaxResults contract, but explicitly DECLINED the published ORDER of both listings and the per-offering price/id values; causes 1-3 and 7 are those order clusters, re-filed because they still diverge and because the pagination cap makes the order observable; causes 4-5 are a NEW consequence of that resolution's encryption rule; cause 6 is an engine-version keying gap the resolution did not close. Other open issues that do NOT cover these clusters: 20261126T155222Z-es-instance-type-catalog-ordering-and-service-software.md carries the `es`-token ListInstanceTypeDetails/DescribeDomain keys, not these `opensearch`-token ones; 20261126T221223Z-es-the-published-package-catalog-is-empty-so.md and 20261122T115902Z-opensearch-the-published-package-catalog-still-answers-neither.md concern the package catalog (DescribePackages / GetPackageVersionHistory).

## Evidence
Replay of corpus simparity-wire-aws-v1 (emulator_up true, 99 fixtures, 492 steps, 365 passed, 127 mismatched, score 74.2%). All seven clusters: verdict mismatch, expected_status == actual_status == 200, stub_path empty, owned_by empty (net-new).

Cause 1 - fixture opensearch-catalog-list-instance-type-details, step task#1, operation ListInstanceTypeDetails, affected_count 5 (task#1..task#5):
  $.InstanceTypeDetails[1]: missing (expected {'InstanceType': 'm7i.2xlarge.search', 'AvailabilityZones': None, 'EncryptionEnabled': True, 'CognitoEnabled': True, 'AppLogsEnabled': True, 'AdvancedSecurityEnabled': True, 'WarmEnabled': True, 'InstanceRole': ['data','master','coordinator']}, got {'InstanceType': 'c5.xlarge.search', ...})
  $.InstanceTypeDetails[6]: expected 'r7i.12xlarge.search', got 'i3.large.search'
  $.InstanceTypeDetails[9]: expected 'c7i.16xlarge.search' (InstanceRole ['data','master']), got 'i3.4xlarge.search'
  $.InstanceTypeDetails[13]: expected 'i7i.4xlarge.search' (InstanceRole ['data']), got 'm5.xlarge.search' (InstanceRole ['data','master','coordinator'])
  divergence_key: service=opensearch|ListInstanceTypeDetails|status=200->200|keys=$.InstanceTypeDetails[1],[5],[6],[9],[13],[15],[17],[18],[19],... (~62 indices)

Cause 2 - same fixture, step task#6, affected_count 1: identical expected head (m7i.2xlarge.search / r7i.12xlarge.search / c7i.16xlarge.search / i7i.4xlarge.search) and identical emulator head (c5.xlarge.search / i3.large.search / i3.4xlarge.search / m5.xlarge.search); ~57 indices diverge.

Cause 3 - same fixture, step task#7, affected_count 5 (task#7..task#11): same instance-type divergence with AdvancedSecurityEnabled False on BOTH sides and EncryptionEnabled True on both; ~54 indices diverge.

Cause 4 - same fixture, step task#12, affected_count 1:
  $.InstanceTypeDetails[0]: missing (expected {'InstanceType': 'r5.large.search', 'EncryptionEnabled': True, 'CognitoEnabled': True, 'AppLogsEnabled': True, 'AdvancedSecurityEnabled': False, 'WarmEnabled': True, 'InstanceRole': ['data','master','coordinator']}, got {'InstanceType': 'c5.large.search', 'EncryptionEnabled': False, ...})
  $.InstanceTypeDetails[1]: expected 'm7i.2xlarge.search' EncryptionEnabled True, got 'c5.xlarge.search' EncryptionEnabled False
  $.InstanceTypeDetails[2]: expected 'r4.2xlarge.search' InstanceRole ['data','master'], got 'c5.2xlarge.search' InstanceRole ['data','master','coordinator']
  $.InstanceTypeDetails[3]: expected 'c4.2xlarge.search', got 'c5.4xlarge.search'
  divergence_key lists all 90 indices [0]..[89]; EncryptionEnabled is False on every emulator row and True on every recorded row.

Cause 5 - same fixture, step task#13, affected_count 3 (task#13..task#15): same expected head (r5.large.search / m7i.2xlarge.search / r4.2xlarge.search / c4.2xlarge.search, EncryptionEnabled True) and same emulator head (c5.* , EncryptionEnabled False); all listed indices diverge.

Cause 6 - same fixture, step task#16, affected_count 2 (task#16, task#17), legacy Elasticsearch release:
  $.InstanceTypeDetails[0]: extra (expected {'InstanceType': 'r4.2xlarge.search', 'EncryptionEnabled': False, 'CognitoEnabled': False, 'AppLogsEnabled': False, 'AdvancedSecurityEnabled': False, 'WarmEnabled': True, 'InstanceRole': ['data','master']}, got {'InstanceType': 'c5.large.search', ..., 'InstanceRole': ['data','master','coordinator']})
  $.InstanceTypeDetails[1]: expected 'r3.4xlarge.search', got 'c5.xlarge.search'
  $.InstanceTypeDetails[2]: expected 't2.medium.search' with WarmEnabled FALSE, got 'c5.2xlarge.search' with WarmEnabled True
  $.InstanceTypeDetails[3]: expected 'c4.2xlarge.search', got 'c5.4xlarge.search'
  divergence_key lists indices [0]..[39] - the whole listing.

Cause 7 - fixture opensearch-catalog-describe-reserved-instance-offerings, step task#1, operation DescribeReservedInstanceOfferings, affected_count 1:
  $.ReservedInstanceOfferings[0]: missing (expected {'InstanceType': 'c7i.xlarge.search', 'Duration': 94608000, 'FixedPrice': 3602.673, 'PaymentOption': 'ALL_UPFRONT', 'RecurringCharges': [{'RecurringChargeAmount': 0.0, 'RecurringChargeFrequency': 'Hourly'}]}, got {'InstanceType': 'c5.large.search', 'Duration': 31536000, 'FixedPrice': 854.92, 'PaymentOption': 'ALL_UPFRONT', ...})
  $.ReservedInstanceOfferings[1]: expected {'InstanceType': 'or1.12xlarge.search', 'Duration': 94608000, 'PaymentOption': 'NO_UPFRONT', 'RecurringCharges': [{'RecurringChargeAmount': 2.61, ...}]}, got {'InstanceType': 'c5.large.search', 'Duration': 31536000, 'PaymentOption': 'NO_UPFRONT', 'RecurringCharges': [{'RecurringChargeAmount': 0.10608, ...}]}
  $.ReservedInstanceOfferings[2]: expected {'InstanceType': 'r8g.16xlarge.search', 'Duration': 31536000, 'FixedPrice': 18367.6615, 'PaymentOption': 'PARTIAL_UPFRONT'}, got {'InstanceType': 'c5.large.search', 'FixedPrice': 446.05, 'PaymentOption': 'PARTIAL_UPFRONT'}
  divergence_key: service=opensearch|DescribeReservedInstanceOfferings|status=200->200|keys=$.ReservedInstanceOfferings[0]..[1439]+ - the emulator's first three rows are one instance type under the three payment options, where AWS's first three rows are three different types and two different terms.

Per-run values are NOT the requirement: the individual FixedPrice/RecurringChargeAmount figures, the ReservedInstanceOfferingId GUIDs and the absolute index numbers of this recording are illustrations. The requirements are (a) the instance-type listing's membership, per-entry capability flags and InstanceRole are AWS's for the EngineVersion in the request, (b) EncryptionEnabled reports the support AWS reports rather than denying it across a listing, and (c) both listings are published in AWS's order, which is stable across calls and is what the MaxResults<=100 pages cut.

## Steps to Reproduce
1. git_clone, build and boot the emulator from the checkout, then point an opensearch client (boto3 client('opensearch')) at the data listener.
2. Causes 1-3: replay opensearch-catalog-list-instance-type-details steps task#1, task#6 and task#7 - GET /2021-01-01/opensearch/instanceTypeDetails/<the fixture's current OpenSearch EngineVersion> with the fixture's query parameters (list_instance_type_details(EngineVersion=...)). Compare $.InstanceTypeDetails element by element against the recorded reply: m7i.2xlarge.search, r7i.12xlarge.search, c7i.16xlarge.search and i7i.4xlarge.search must appear at AWS's positions with AWS's per-type InstanceRole. Also call with MaxResults=10 to see that the emulator's page 1 holds a different set of types than AWS's.
3. Causes 4-5: replay task#12 and task#13 of the same fixture and read EncryptionEnabled on every returned row: the emulator answers false on all of them where the recording answers true, and the types at index 0..3 must be r5.large.search / m7i.2xlarge.search / r4.2xlarge.search / c4.2xlarge.search.
4. Cause 6: replay task#16 of the same fixture - the same operation for the recorded LEGACY Elasticsearch release. The reply must be that release's catalog (r4.2xlarge.search, r3.4xlarge.search, t2.medium.search with WarmEnabled false, c4.2xlarge.search, each with that generation's InstanceRole), not the c5 listing the current-release requests return.
5. Cause 7: replay opensearch-catalog-describe-reserved-instance-offerings step task#1 - GET /2021-01-01/opensearch/reservedInstanceOfferings (describe_reserved_instance_offerings()). The head of the listing must interleave instance type, term and payment option as the recording does (c7i.xlarge.search/94608000/ALL_UPFRONT, or1.12xlarge.search/94608000/NO_UPFRONT, r8g.16xlarge.search/31536000/PARTIAL_UPFRONT), not emit one instance type across the three payment options; check the same with MaxResults=100 so the first page is comparable.
6. Re-replay ALL SEVEN steps - opensearch-catalog-list-instance-type-details#task#1, #task#6, #task#7, #task#12, #task#13, #task#16 and opensearch-catalog-describe-reserved-instance-offerings#task#1 - and confirm each passes before submitting.
