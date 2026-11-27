Issue-Source: trace-sweep-divergence
Issue-Trace: datazone-custom-asset-publish-to-catalog-listing
Issue-Step: task#7
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: datazone-custom-asset-publish-to-catalog-listing
Issue-Shape-Id: com.amazonaws.datazone#DataZone
Issue-Key: datazone|CreateAsset|2xx->2xx|$.formsOutput[*].renderingConfig
Issue-Key: datazone|GetListing|2xx->2xx|$.item.assetListing.assetFormsMetadata
Issue-Commit: 7815a7c486f6433d295c8c37df76b6ba67dbf690

# datazone drops metadata-form rendering metadata from asset and listing responses

> _Note: this issue was identified against the code base at commit `7815a7c486f6433d295c8c37df76b6ba67dbf690`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateAsset

## Description
Two net-new causes, both in the same fault: the emulator's DataZone asset/listing serializers never emit the metadata-form rendering metadata that the live API returns alongside a form, so the per-form rendering block is absent on asset creation and the published listing carries no form metadata at all.

Cause 1 of 2: CreateAsset omits formsOutput[].renderingConfig.
  AWS: recorded behavior only - the checked-in Smithy model (services/service-com-amazonaws-datazone-datazone/smithy-model/com-amazonaws-datazone-datazone.json) models CreateAssetOutput.formsOutput as the required FormOutputList of FormOutput{formName, typeName, typeRevision, content} and has no renderingConfig member or RenderingConfig shape, so the field is an unmodelled-but-live addition. The recording shows AWS answering CreateAsset with HTTP 201 and, for every entry of formsOutput, a renderingConfig object carrying the form type's rendering settings (in the recorded step, {"collapse": false}). The value is a property of the referenced form type, not of the request. Hypothesis: renderingConfig is defaulted from the form type's rendering configuration when the type does not declare one, which is why the recorded value is the all-defaults object rather than absent.
  Emulator: answers CreateAsset 201 (status matches) but every formsOutput entry is serialized without renderingConfig; the replayed step reports "$.formsOutput[0].renderingConfig: missing (expected {'collapse': False}, got None)".
  Why: a client that reads formsOutput[].renderingConfig to decide how to render a form's fields (collapsed vs expanded) gets None and either crashes on attribute access or silently falls back to a different presentation than AWS specifies; SDK response models that mark the block present will deserialize it as absent.
  Fix: the CreateAsset handler/serializer for formsOutput in the datazone service crate - the FormOutput projection must carry the rendering configuration resolved from the form type (defaulted when the type declares none), and the asset state must retain it so later reads stay consistent.
  Traces: datazone-custom-asset-publish-to-catalog-listing#task#7.

Cause 2 of 2: GetListing omits item.assetListing.assetFormsMetadata.
  AWS: recorded behavior only - the on-disk model's AssetListing structure (members assetId, assetRevision, assetType, createdAt, forms, latestTimeSeriesDataPointForms, glossaryTerms, governedGlossaryTerms, owningProjectId) has no assetFormsMetadata member, so this too is live-only. The recording shows AWS answering GetListing with HTTP 200 and item.assetListing.assetFormsMetadata as a list with one entry per metadata form attached to the published asset, each entry carrying formName, typeName, typeRevision and the same renderingConfig block as Cause 1 (recorded entry: formName/typeName equal to the form type created earlier in the fixture, typeRevision "1", renderingConfig {"collapse": false}).
  Emulator: answers GetListing 200 (status matches) with the field absent entirely; the replayed step reports "$.item.assetListing.assetFormsMetadata: missing (expected [{...}], got None)".
  Why: a consumer browsing the catalog listing cannot discover which metadata forms the published asset carries or how to render them, so a publish-then-read workflow that drives UI or downstream validation off the listing sees a form-less asset even though the asset was created with forms.
  Emulator component to fix: the GetListing handler's AssetListing projection and the listing state it reads - publishing an asset must copy the asset's form metadata (name, type name, type revision, rendering configuration) onto the listing item so GetListing can serialize it.
  Traces: datazone-custom-asset-publish-to-catalog-listing#task#9.

Neither cause is covered by an existing open issue: the open datazone issue "generated resource ids are 9 characters..." concerns id minting, not form metadata, and the other open datazone issues are unimplemented-operation stubs for different operations.

## Evidence
Cause 1 - fixture datazone-custom-asset-publish-to-catalog-listing step task#7, CreateAsset: expected status 201, actual 201; body diff "$.formsOutput[0].renderingConfig: missing (expected {'collapse': False}, got None)".
Cause 2 - same fixture, step task#9, GetListing: expected status 200, actual 200; body diff "$.item.assetListing.assetFormsMetadata: missing (expected [{'formName': 'sim2real_probe_dz_form_2ac7fcbd', 'renderingConfig': {'collapse': False}, 'typeName': 'sim2real_probe_dz_form_2ac7fcbd', 'typeRevision': '1'}], got None)" - the recorded form name/type name is this run's generated value and only the presence/shape of the entry (formName, typeName, typeRevision, renderingConfig) is the requirement.
Smithy: CreateAsset -> POST /v2/domains/{domainIdentifier}/assets, 201, output member formsOutput (required, FormOutputList of FormOutput); GetListing -> GET /v2/domains/{domainIdentifier}/listings/{identifier}, 200, output member item (ListingItem union, assetListing -> AssetListing). Neither renderingConfig nor assetFormsMetadata is present in the checked-in model.

## Steps to Reproduce
1. Replay fixture datazone-custom-asset-publish-to-catalog-listing against the emulator (replay_wire_corpus).
2. Step task#7 creates a custom asset that attaches a metadata form of a previously created form type; inspect the 201 body's formsOutput entries - renderingConfig is absent where AWS returns the form type's rendering block.
3. Steps task#8/task#9 publish the asset and GetListing the resulting listing; inspect the 200 body's item.assetListing - assetFormsMetadata is absent where AWS returns one entry per attached form.
