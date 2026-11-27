Issue-Source: trace-sweep-divergence
Issue-Trace: create-document-requires-name-grammar
Issue-Step: task#7
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: create-document-requires-name-grammar ssm-document-requires-version-name-pattern ssm-automation-loop-document-validation ssm-document-permission-max-results-range ssm-list-documents-filters-count-bound ssm-list-documents-legacy-filter-key-outside-enum ssm-list-documents-tag-filter-value-length-bounds ssm-update-document-default-version-number-grammar ssm-public-automation-document-description ssm-catalog-list-documents
Issue-Shape-Id: com.amazonaws.ssm#AmazonSSM
Issue-Key: ssm|CreateDocument|2xx->2xx|$.DocumentDescription.CloudProviders
Issue-Key: ssm|CreateDocument|2xx->2xx|$.DocumentDescription.PlatformTypes[*]
Issue-Key: ssm|CreateDocument|2xx->2xx|$.DocumentDescription.SchemaVersion
Issue-Key: ssm|DescribeDocument|2xx->2xx|$.Document.CloudProviders
Issue-Key: ssm|DescribeDocument|2xx->2xx|$.Document.PlatformTypes[*]
Issue-Key: ssm|DescribeDocument|2xx->2xx|$.Document.SchemaVersion
Issue-Key: ssm|CreateDocument|2xx->2xx|$.DocumentDescription.Requires[*]
Issue-Key: ssm|DescribeDocument|2xx->2xx|$.Document.Requires[*]
Issue-Key: ssm|ListDocuments|2xx->2xx|$.DocumentIdentifiers[*].CloudProviders
Issue-Key: ssm|GetDocument|2xx->2xx|$.AllowInvalidContent
Issue-Key: ssm|DescribeDocument|2xx->2xx|$.Document.Category
Issue-Key: ssm|DescribeDocument|2xx->2xx|$.Document.CategoryEnum
Issue-Key: ssm|ListDocuments|2xx->2xx|$.DocumentIdentifiers[Name="AWS-ApplyAnsiblePlaybooks"].Categories
Issue-Key: ssm|ListDocuments|2xx->2xx|$.DocumentIdentifiers[Name="AWS-ApplyAnsiblePlaybooks"].CreatedDate
Issue-Key: ssm|ListDocuments|2xx->2xx|$.DocumentIdentifiers[Name="AWS-ConfigureAWSPackage"].Categories
Issue-Key: ssm|ListDocuments|2xx->2xx|$.DocumentIdentifiers[Name="AWS-ConfigureAWSPackage"].CreatedDate
Issue-Key: ssm|ListDocuments|2xx->2xx|$.DocumentIdentifiers[Name="AWS-ConfigureCloudWatch"].Categories
Issue-Key: ssm|ListDocuments|2xx->2xx|$.DocumentIdentifiers[Name="AWS-ConfigureCloudWatch"].CreatedDate
Issue-Key: ssm|ListDocuments|2xx->2xx|$.DocumentIdentifiers[Name="AWS-CreateImage"].Categories
Issue-Key: ssm|ListDocuments|2xx->2xx|$.DocumentIdentifiers[Name="AWS-CreateSnapshot"].Categories
Issue-Key: ssm|ListDocuments|2xx->2xx|$.DocumentIdentifiers[Name="AWS-CreateSnapshot"].CreatedDate
Issue-Key: ssm|ListDocuments|2xx->2xx|$.DocumentIdentifiers[Name="AWS-DeleteSnapshot"].Categories
Issue-Key: ssm|ListDocuments|2xx->2xx|$.DocumentIdentifiers[Name="AWS-DeleteSnapshot"].CreatedDate
Issue-Key: ssm|ListDocuments|2xx->2xx|$.DocumentIdentifiers[Name="AWS-FindWindowsUpdates"].Categories
Issue-Key: ssm|ListDocuments|2xx->2xx|$.DocumentIdentifiers[Name="AWS-FindWindowsUpdates"].CreatedDate
Issue-Key: ssm|ListDocuments|2xx->2xx|$.DocumentIdentifiers[Name="AWS-InstallApplication"].Categories
Issue-Key: ssm|ListDocuments|2xx->2xx|$.DocumentIdentifiers[Name="AWS-InstallApplication"].CreatedDate
Issue-Key: ssm|ListDocuments|2xx->2xx|$.DocumentIdentifiers[Name="AWS-InstallPowerShellModule"].Categories
Issue-Key: ssm|ListDocuments|2xx->2xx|$.DocumentIdentifiers[Name="AWS-InstallPowerShellModule"].CreatedDate
Issue-Key: ssm|ListDocuments|2xx->2xx|$.DocumentIdentifiers[Name="AWS-InstallWindowsUpdates"].Categories
Issue-Key: ssm|ListDocuments|2xx->2xx|$.DocumentIdentifiers[Name="AWS-InstallWindowsUpdates"].CreatedDate
Issue-Key: ssm|ListDocuments|2xx->2xx|$.DocumentIdentifiers[Name="AWS-JoinDirectoryServiceDomain"].Categories
Issue-Key: ssm|ListDocuments|2xx->2xx|$.DocumentIdentifiers[Name="AWS-JoinDirectoryServiceDomain"].CreatedDate
Issue-Key: ssm|ListDocuments|2xx->2xx|$.DocumentIdentifiers[Name="AWS-PatchInstanceWithRollback"].Categories
Issue-Key: ssm|ListDocuments|2xx->2xx|$.DocumentIdentifiers[Name="AWS-PatchInstanceWithRollback"].CreatedDate
Issue-Key: ssm|ListDocuments|2xx->2xx|$.DocumentIdentifiers[Name="AWS-RefreshAssociation"].Categories
Issue-Key: ssm|ListDocuments|2xx->2xx|$.DocumentIdentifiers[Name="AWS-RefreshAssociation"].CreatedDate
Issue-Key: ssm|ListDocuments|2xx->2xx|$.DocumentIdentifiers[Name="AWS-RefreshAssociation"].SchemaVersion
Issue-Key: ssm|ListDocuments|2xx->2xx|$.DocumentIdentifiers[Name="AWS-ResizeInstance"].Categories
Issue-Key: ssm|ListDocuments|2xx->2xx|$.DocumentIdentifiers[Name="AWS-ResizeInstance"].CreatedDate
Issue-Key: ssm|ListDocuments|2xx->2xx|$.DocumentIdentifiers[Name="AWS-RestartEC2Instance"].Categories
Issue-Key: ssm|ListDocuments|2xx->2xx|$.DocumentIdentifiers[Name="AWS-RunDocument"].CreatedDate
Issue-Key: ssm|ListDocuments|2xx->2xx|$.DocumentIdentifiers[Name="AWS-RunInspecChecks"].CreatedDate
Issue-Key: ssm|ListDocuments|2xx->2xx|$.DocumentIdentifiers[Name="AWS-RunInspecChecks"].PlatformTypes[*]
Issue-Key: ssm|ListDocuments|2xx->2xx|$.DocumentIdentifiers[Name="AWS-RunPatchBaseline"].Categories
Issue-Key: ssm|ListDocuments|2xx->2xx|$.DocumentIdentifiers[Name="AWS-RunPatchBaseline"].CreatedDate
Issue-Key: ssm|ListDocuments|2xx->2xx|$.DocumentIdentifiers[Name="AWS-RunRemoteScript"].Categories
Issue-Key: ssm|ListDocuments|2xx->2xx|$.DocumentIdentifiers[Name="AWS-StartEC2Instance"].Categories
Issue-Key: ssm|ListDocuments|2xx->2xx|$.DocumentIdentifiers[Name="AWS-StartEC2Instance"].CreatedDate
Issue-Key: ssm|ListDocuments|2xx->2xx|$.DocumentIdentifiers[Name="AWS-StartInteractiveCommand"].CreatedDate
Issue-Key: ssm|ListDocuments|2xx->2xx|$.DocumentIdentifiers[Name="AWS-StartNonInteractiveCommand"].CreatedDate
Issue-Key: ssm|ListDocuments|2xx->2xx|$.DocumentIdentifiers[Name="AWS-StartPortForwardingSessionToRemoteHost"].CreatedDate
Issue-Key: ssm|ListDocuments|2xx->2xx|$.DocumentIdentifiers[Name="AWS-StopEC2Instance"].Categories
Issue-Key: ssm|ListDocuments|2xx->2xx|$.DocumentIdentifiers[Name="AWS-StopEC2Instance"].CreatedDate
Issue-Key: ssm|ListDocuments|2xx->2xx|$.DocumentIdentifiers[Name="AWS-TerminateEC2Instance"].Categories
Issue-Key: ssm|ListDocuments|2xx->2xx|$.DocumentIdentifiers[Name="AWS-TerminateEC2Instance"].CreatedDate
Issue-Key: ssm|ListDocuments|2xx->2xx|$.DocumentIdentifiers[Name="AWS-UpdateLinuxAmi"].Categories
Issue-Key: ssm|ListDocuments|2xx->2xx|$.DocumentIdentifiers[Name="AWS-UpdateLinuxAmi"].CreatedDate
Issue-Related: 20261114T164648Z-ssm-2-net-new-causes-sendautomationsignal-stopstep-precondition.md
Issue-Related: 20261125T060148Z-ssm-seeded-patch-and-document-reference-catalogs-are.md
Issue-Commit: 255279eeed7e5207bd6ac2895e89724e049cf5ef

# SSM document read projections omit modeled members and misstate the seeded document catalog

> _Note: this issue was identified against the code base at commit `255279eeed7e5207bd6ac2895e89724e049cf5ef`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeDocument

## Description
Ten net-new causes, all in the SSM Documents read path: the document projections the emulator serializes are not the documents AWS serializes, and the seeded AWS-owned document catalog carries invented metadata. Bundled because one serializer/seed layer produces all of them.

Cause 1 of 10: CreateDocument response projection.
AWS: CreateDocument returns DocumentDescription with CloudProviders populated (['AWS'] for a document created in an AWS account), SchemaVersion present (empty string when the content declares none) and PlatformTypes containing only the platforms the content actually declares. Smithy: CreateDocument -> DocumentDescription (members CloudProviders, SchemaVersion, PlatformTypes); recorded behavior supplies the values.
Emulator: omits DocumentDescription.CloudProviders and DocumentDescription.SchemaVersion entirely and emits a fixed PlatformTypes list of Windows, Linux, MacOS for every document.
Why: a client reading CloudProviders/SchemaVersion sees null where AWS always sends a value, and platform-based document selection picks a document for platforms it does not support.
Fix: the CreateDocument response serializer / document projection.
Traces: create-document-requires-name-grammar#task#7.

Cause 2 of 10: DescribeDocument carries the same projection defect.
AWS: Document.CloudProviders = ['AWS'], Document.SchemaVersion present, Document.PlatformTypes only the declared platforms.
Emulator: CloudProviders and SchemaVersion missing; PlatformTypes padded with Windows, Linux, MacOS.
Why: same read is wrong on the describe path, so a client cannot recover the value by re-reading.
Fix: the DescribeDocument response serializer.
Traces: create-document-requires-name-grammar#task#8.

Cause 3 of 10: CreateDocument rewrites Requires[].Name into an ARN.
AWS: the Requires list is echoed as submitted — the required document's plain name (e.g. the name the request sent) and its Version. Smithy: DocumentRequires$Name targets DocumentARN but AWS stores and returns the submitted value.
Emulator: rewrites Requires[0].Name to arn:aws:ssm:<region>:<account>:document/<name>.
Why: a client that compares the Requires entry it submitted with the entry returned (or passes it to a later CreateDocument) no longer matches, because the identifier changed shape.
Fix: the CreateDocument handler / document state write (store the submitted name verbatim).
Traces: create-document-requires-name-grammar#task#9.

Cause 4 of 10: DescribeDocument returns the same ARN-rewritten Requires entry.
AWS: Document.Requires[].Name is the plain document name as submitted.
Emulator: returns the ARN form.
Why: the rewrite is persisted, not just a serializer accident.
Fix: the document state record for Requires plus the DescribeDocument projection.
Traces: create-document-requires-name-grammar#task#10.

Cause 5 of 10: CreateDocument omits CloudProviders for an automation document.
AWS: DocumentDescription.CloudProviders = ['AWS'].
Emulator: member absent.
Why: as cause 1, on a second document type — showing the omission is unconditional.
Fix: the CreateDocument response projection.
Traces: ssm-automation-loop-document-validation#task#4.

Cause 6 of 10: DescribeDocument omits CloudProviders.
AWS: Document.CloudProviders = ['AWS'].
Emulator: member absent.
Fix: the DescribeDocument response projection.
Traces: ssm-automation-loop-document-validation#task#5.

Cause 7 of 10: ListDocuments omits CloudProviders on each identifier.
AWS: DocumentIdentifiers[].CloudProviders = ['AWS'].
Emulator: member absent.
Why: a client filtering or displaying documents by cloud provider sees nothing for every document.
Fix: the ListDocuments identifier projection.
Traces: ssm-list-documents-filters-count-bound#task#3.

Cause 8 of 10: GetDocument omits AllowInvalidContent.
AWS: GetDocument returns AllowInvalidContent = false for a document created without that flag (recorded behavior only — the on-disk Smithy GetDocumentResult does not model the member, so the recording is the requirement).
Emulator: member absent.
Why: a client reading the flag cannot tell whether content validation was bypassed.
Fix: the GetDocument response projection.
Traces: ssm-update-document-default-version-number-grammar#task#8.

Cause 9 of 10: DescribeDocument reports no categories for an AWS-owned automation runbook.
AWS: Document.Category is the document's human-readable category list and Document.CategoryEnum the matching enum list (for the probed AWS runbook: Instance management / Cost management and InstanceManagement / CostManagement).
Emulator: both arrays empty for every AWS-owned document.
Why: console-style grouping and any category filter sees an uncategorised catalog.
Fix: the seeded AWS document catalog plus the DescribeDocument projection.
Traces: ssm-public-automation-document-description#task#1.

Cause 10 of 10: the seeded AWS-owned document catalog metadata is invented.
AWS: each AWS-* document in ListDocuments carries its real Categories, CreatedDate, DocumentFormat (several are YAML), SchemaVersion, DocumentType, Author, Description, ReviewStatus and only the PlatformTypes it supports.
Emulator: Categories missing for most documents, CreatedDate collapsed to a constant (1451606400 / one seed timestamp) for every document, DocumentFormat reported JSON where AWS publishes YAML, PlatformTypes padded with Linux/MacOS, and SchemaVersion/TargetType/Author/Description/ReviewStatus diverging on individual documents.
Why: a client that selects an AWS document by format, platform, category or schema version picks the wrong document or fails to parse the content it then fetches.
Fix: the seeded document reference catalog (state seed data). Related but NOT covering this: open issue 20261125T060148Z-ssm-seeded-patch-and-document-reference-catalogs-are.md owns the patch catalog clusters only; this ListDocuments metadata cluster is unowned.
Traces: ssm-catalog-list-documents#task#1.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout.
Cause 1 (create-document-requires-name-grammar#task#7, 200->200): $.DocumentDescription.CloudProviders missing (expected ['AWS']); $.DocumentDescription.SchemaVersion missing (expected ''); $.DocumentDescription.PlatformTypes[0..2] extra ('Windows','Linux','MacOS').
Cause 2 (create-document-requires-name-grammar#task#8): same keys under $.Document.
Cause 3 (create-document-requires-name-grammar#task#9): $.DocumentDescription.Requires[0] expected {'Name': '<submitted-document-name>', 'Version': '1'}, got {'Name': 'arn:aws:ssm:us-west-2:000000000000:document/<submitted-document-name>', 'Version': '1'}.
Cause 4 (create-document-requires-name-grammar#task#10): same under $.Document.Requires[0].
Cause 5 (ssm-automation-loop-document-validation#task#4): $.DocumentDescription.CloudProviders missing.
Cause 6 (ssm-automation-loop-document-validation#task#5): $.Document.CloudProviders missing.
Cause 7 (ssm-list-documents-filters-count-bound#task#3): $.DocumentIdentifiers[0].CloudProviders missing.
Cause 8 (ssm-update-document-default-version-number-grammar#task#8): $.AllowInvalidContent missing (expected False).
Cause 9 (ssm-public-automation-document-description#task#1): $.Document.Category expected ['Instance management','Cost management'], got []; $.Document.CategoryEnum expected ['InstanceManagement','CostManagement'], got [].
Cause 10 (ssm-catalog-list-documents#task#1): e.g. AWS-ASGEnterStandby.Categories expected ['InstanceManagement'] got None; AWS-ASGEnterStandby.CreatedDate expected 1518661792.37 got 1451606400; AWS-AddOpsItemDedupStringToEventBridgeRule.DocumentFormat expected 'YAML' got 'JSON'; AWS-ApplyDSCMofs.PlatformTypes[1..2] extra.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus (replay_wire_corpus).
2. Cause 1-4: run fixture create-document-requires-name-grammar and inspect steps task#7..task#10 (CreateDocument / DescribeDocument of a document with a Requires entry).
3. Cause 5-6: fixture ssm-automation-loop-document-validation steps task#4, task#5.
4. Cause 7: fixture ssm-list-documents-filters-count-bound step task#3.
5. Cause 8: fixture ssm-update-document-default-version-number-grammar step task#8.
6. Cause 9: fixture ssm-public-automation-document-description step task#1.
7. Cause 10: fixture ssm-catalog-get-inventory... no: fixture ssm-catalog-list-documents step task#1 (ListDocuments over the seeded AWS catalog).
