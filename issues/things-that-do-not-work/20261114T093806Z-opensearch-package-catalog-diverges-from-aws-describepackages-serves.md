Issue-Source: trace-sweep-divergence
Issue-Trace: opensearch-catalog-get-package-version-history
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: opensearch-catalog-get-package-version-history opensearch-catalog-describe-packages
Issue-Shape-Id: com.amazonaws.opensearch#AmazonOpenSearchService
Issue-Key: opensearch|GetPackageVersionHistory|2xx->4xx|$.PackageID
Issue-Key: opensearch|GetPackageVersionHistory|2xx->4xx|$.PackageVersionHistoryList
Issue-Key: opensearch|GetPackageVersionHistory|2xx->4xx|status
Issue-Key: opensearch|DescribePackages|2xx->2xx|$.PackageDetailsList[*]
Issue-Commit: 7ab16e4596463d769159d2478462aef7024df8a6

# opensearch package catalog diverges from AWS — DescribePackages serves a legacy synthetic plugin list and GetPackageVersionHistory refuses real catalog package IDs

> _Note: this issue was identified against the code base at commit `7ab16e4596463d769159d2478462aef7024df8a6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetPackageVersionHistory

## Description
The emulator's OpenSearch package plane is backed by an emulator-authored catalog of legacy Elasticsearch analysis plugins instead of the AWS-managed package catalog. That one substitution produces two distinct wire faults: the catalog listing returns the wrong set of packages, and every per-package read of a real catalog package ID is refused outright. 2 causes.

Cause 1 of 2: GetPackageVersionHistory refuses a package that exists in the catalog.
AWS: Smithy: GetPackageVersionHistory GET /2021-01-01/packages/{PackageID}/history, success code 200, output members PackageID (pattern ^([FG][0-9]+)$|^(pkg-[a-f0-9]+)$) and PackageVersionHistoryList. For a package present in the account-visible AWS-managed catalog AWS answers 200 and returns one PackageVersionHistoryList entry per published version, newest first, each entry carrying CreatedAt, PackageVersion, CommitMessage, PackageConfiguration (ConfigurationRequirement, LicenseRequirement, LicenseFilepath, RequiresRestartForConfigurationUpdate) and PluginProperties (Name, Description, Version, ClassName, UncompressedSizeInBytes). The recorded response is 200 with PackageID echoed and a two-version history. The modeled refusal for a package that genuinely does not exist is ResourceNotFoundException with httpError 409 (ValidationException/400 is modeled only for a malformed request), so a 400 is not the modeled answer for an unknown-but-well-formed PackageID either.
Emulator: answers HTTP 400 with no PackageID and no PackageVersionHistoryList. The requested PackageID is syntactically valid against the modeled pattern and is listed by AWS's DescribePackages, so the refusal is purely a consequence of the emulator's catalog not containing it.
Why: a client enumerating packages and then reading each one's version history (the normal way to pick a PackageVersion before AssociatePackages) gets a hard client error on every package. Because the operation is paginated and modeled as a read, SDK/CLI callers surface it as a request fault rather than an empty result, so package-version selection cannot be automated against the emulator at all. This is the single widest divergence in the corpus: 90 replayed steps across the fixture hit it.
Fix: the GetPackageVersionHistory handler and the package catalog state it reads — the catalog must contain the AWS-managed packages with their per-version history, and a well-formed PackageID that is absent must be refused with the modeled ResourceNotFoundException (409), not a bare 400.
Traces: opensearch-catalog-get-package-version-history#task#1 (representative; 90 affected steps, task#1..task#25 and beyond in the same cluster).

Cause 2 of 2: DescribePackages returns an emulator-authored legacy plugin catalog instead of the AWS package catalog.
AWS: Smithy: DescribePackages POST /2021-01-01/packages/describe, success 200, output PackageDetailsList of PackageDetails. AWS returns the account-visible AWS-managed package catalog; each PackageDetails entry carries PackageID, PackageName, PackageType, PackageDescription, PackageStatus, PackageOwner, CreatedAt, LastUpdatedAt, AvailablePackageVersion, EngineVersion, ErrorDetails, AvailablePluginProperties (Name, Description, Version, ClassName, UncompressedSizeInBytes) and AvailablePackageConfiguration (ConfigurationRequirement, LicenseRequirement, LicenseFilepath, RequiresRestartForConfigurationUpdate). The recorded catalog is the current marketplace/AWS-managed plugin set (entries such as a ZIP-PLUGIN owned by a third-party vendor at EngineVersion OpenSearch_2.17 with AvailablePackageVersion v2).
Emulator: returns a different list entirely — ~92 entries of legacy Elasticsearch analysis plugins (analysis-phonetic and siblings) pinned to EngineVersion Elasticsearch_7.10, with no PackageOwner, no AvailablePackageConfiguration and no ErrorDetails member. Every index of PackageDetailsList diverges ($.PackageDetailsList[0] through [92]).
Why: a client cannot discover any package AWS actually offers, cannot read PackageOwner or the configuration/licence requirements that decide whether a package needs a restart or a licence file, and every PackageID it learns here is one no real account would return — so a plan built from this listing fails against AWS. It is also the upstream of Cause 1: the IDs AWS publishes are exactly the ones the history read refuses.
Fix: the package catalog seed/state behind DescribePackages (and the PackageDetails serializer, which must emit PackageOwner, AvailablePackageConfiguration and ErrorDetails).
Traces: opensearch-catalog-describe-packages#task#1.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout.

Cause 1 — opensearch-catalog-get-package-version-history#task#1 (GetPackageVersionHistory), 90 affected steps:
  status: expected 200, got 400
  $.PackageID: missing (expected a pkg-<hex> id, got None)
  $.PackageVersionHistoryList: missing (expected a 2-entry history [{PackageVersion: v2, CreatedAt, PackageConfiguration{ConfigurationRequirement: OPTIONAL, LicenseRequirement: REQUIRED, LicenseFilepath, RequiresRestartForConfigurationUpdate: true}, PluginProperties{Name, Description, Version, ClassName, UncompressedSizeInBytes}}, {PackageVersion: v1, ...}], got None)

Cause 2 — opensearch-catalog-describe-packages#task#1 (DescribePackages), status 200 vs 200, every element of PackageDetailsList diverges:
  $.PackageDetailsList[0]: expected an AWS-managed ZIP-PLUGIN entry carrying PackageOwner, EngineVersion 'OpenSearch_2.17', AvailablePackageVersion 'v2', AvailablePackageConfiguration{ConfigurationRequirement, LicenseRequirement, LicenseFilepath, RequiresRestartForConfigurationUpdate} and ErrorDetails; got an 'analysis-phonetic' entry with EngineVersion 'Elasticsearch_7.10' and no PackageOwner / AvailablePackageConfiguration / ErrorDetails member
  divergence_key lists $.PackageDetailsList[0] .. $.PackageDetailsList[92] — i.e. no element matches

Related but NOT covering these: the open issue 20261029T190009Z-es-package-vpc-endpoint-tagging-and-reserved-instance.md concerns `es` package refusal messages (AssociatePackages / DissociatePackages wording), not the opensearch catalog contents or the GetPackageVersionHistory status; the replay reports owned_by=[] for both clusters above.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay fixture opensearch-catalog-get-package-version-history, step task#1: GET /2021-01-01/packages/{PackageID}/history for a PackageID listed by the AWS package catalog. Emulator answers 400 with no body members; AWS answers 200 with PackageID and PackageVersionHistoryList.
3. Replay fixture opensearch-catalog-describe-packages, step task#1: POST /2021-01-01/packages/describe with no filters. Compare PackageDetailsList against the recording — the emulator returns ~92 legacy Elasticsearch analysis plugins instead of the AWS-managed catalog, and omits PackageOwner / AvailablePackageConfiguration / ErrorDetails.
4. Both steps are reported by replay_wire_corpus as mismatches with stub_path="" and owned_by=[].
