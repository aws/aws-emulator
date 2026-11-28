Issue-Source: trace-sweep-divergence
Issue-Trace: lakeformation-data-lake-settings-and-principal
Issue-Step: task#4
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: lakeformation-data-lake-settings-and-principal
Issue-Shape-Id: com.amazonaws.lakeformation#AWSLakeFormation
Issue-Key: lakeformation|GetDataLakeSettings|2xx->2xx|$.DataLakeSettings.DisallowGrantOnIAMAllowedPrincipals
Issue-Key: lakeformation|GetDataLakeSettings|2xx->2xx|$.DataLakeSettings.SetSourceIdentity
Issue-Key: lakeformation|GetDataLakeSettings|2xx->2xx|$.DataLakeSettings.WhitelistedForExternalDataFiltering
Issue-Commit: 35f7de95e926ecd075d4f0c5efe73729faf5e0c1

# Lake Formation GetDataLakeSettings omits the undocumented DataLakeSettings wire members AWS always emits

> _Note: this issue was identified against the code base at commit `35f7de95e926ecd075d4f0c5efe73729faf5e0c1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetDataLakeSettings

## Description
GetDataLakeSettings serializes only the members the public Smithy model declares for DataLakeSettings, so three members that the live Lake Formation service puts on the wire in every GetDataLakeSettings response are absent from the emulator's body. 1 cause.

Cause 1 of 1: the DataLakeSettings structure in a GetDataLakeSettings 200 body is missing DisallowGrantOnIAMAllowedPrincipals, SetSourceIdentity and WhitelistedForExternalDataFiltering.
AWS: recorded behavior only — the Smithy model (closure for GetDataLakeSettings; shape com.amazonaws.lakeformation#DataLakeSettings) declares exactly DataLakeAdmins, ReadOnlyAdmins, CreateDatabaseDefaultPermissions, CreateTableDefaultPermissions, Parameters, TrustedResourceOwners, AllowExternalDataFiltering, AllowFullTableExternalDataAccess, ExternalDataFilteringAllowList and AuthorizedSessionTagValueList, and carries no jsonName trait on any of them, so none of the three names below is a modeled member or a modeled wire alias. The recording shows the real service nevertheless emitting them as additional keys of $.DataLakeSettings on a plain 200 (GetDataLakeSettings http code 200, output GetDataLakeSettingsResponse): a boolean DisallowGrantOnIAMAllowedPrincipals, a boolean SetSourceIdentity, and a list WhitelistedForExternalDataFiltering. The recording's values are the unconfigured-catalog state of a catalog nothing has written: both booleans false and the list empty; for a configured catalog they track whatever the last PutDataLakeSettings wrote (the booleans follow the settings' IAM-allowed-principals grant behavior and the SET_SOURCE_IDENTITY parameter, the list the external-data-filtering allow list), so false/false/[] is the default state, not a constant of the response.
Emulator: the keys are simply not present. Replay of three steps of the same fixture reports, on a 200 that is otherwise correct, "$.DataLakeSettings.DisallowGrantOnIAMAllowedPrincipals: missing (expected False, got None)", "$.DataLakeSettings.SetSourceIdentity: missing (expected False, got None)" and "$.DataLakeSettings.WhitelistedForExternalDataFiltering: missing (expected [], got None)". The handler returns Some(store.data_lake_settings_for(&catalog_id)) and the generated output schema has nowhere to put the three extra members, so they can never appear — the gap is present for every catalog and every settings state, including right after a PutDataLakeSettings write.
Why: a client that reads these fields — the console and the older SDK/CLI paths that still consult the legacy DisallowGrantOnIAMAllowedPrincipals / SetSourceIdentity / WhitelistedForExternalDataFiltering names, and any code that round-trips a GetDataLakeSettings body back into PutDataLakeSettings — sees them as absent/None against the emulator where real AWS always supplies a value. Code that treats a missing key as "feature unknown" takes a different branch, and a round-trip that reconstructs settings from the response silently drops the external-data-filtering allow list and the source-identity setting.
Fix: the GetDataLakeSettings response serializer for this service, together with the settings projection it reads — the DataLakeSettings body must carry the three extra members in addition to the modeled ones, derived from the stored settings for the addressed catalog (and from the unconfigured-catalog defaults when nothing has been written), not hard-coded to the recorded false/false/[].
Traces: lakeformation-data-lake-settings-and-principal#task#4, #task#6 and #task#8 — three reads of the settings in one fixture, all three missing the same keys.

No existing open issue covers this: the open lakeformation issues are about ListPermissions synthesizing LastUpdated/LastUpdatedBy, ListDataCellsFilter skipping Table.CatalogId validation, and the CloudFormation AWS::LakeFormation::DataLakeSettings handler not performing its PutDataLakeSettings write (that one is about DataLakeAdmins content, not the response's member set).

## Evidence
Fixture lakeformation-data-lake-settings-and-principal, step task#4 (verdict mismatch, expected status 200, actual status 200 — the divergence is the body):
  expected $.DataLakeSettings.DisallowGrantOnIAMAllowedPrincipals = false   | actual: key absent (None)
  expected $.DataLakeSettings.SetSourceIdentity = false                     | actual: key absent (None)
  expected $.DataLakeSettings.WhitelistedForExternalDataFiltering = []      | actual: key absent (None)
Same three keys absent at steps task#6 and task#8 of that fixture (3 affected steps in the cluster).
Model check: smithy_query(query="closure", operation="GetDataLakeSettings", model_path="services/service-com-amazonaws-lakeformation-awslakeformation/smithy-model/com-amazonaws-lakeformation-awslakeformation.json") lists DataLakeSettings' ten members and no jsonName trait, so the three names are undocumented wire members (recorded behavior only).
Code: services/service-com-amazonaws-lakeformation-awslakeformation/src/ops/get_data_lake_settings.rs lines 160-162 return only the modeled DataLakeSettings struct, so the serializer has no slot for them.
The recorded false/false/[] is the unconfigured-catalog state; the requirement is that the keys are always present and reflect the stored settings, not that they hold those values.

## Steps to Reproduce
1. POST /GetDataLakeSettings (target AWSLakeFormation.GetDataLakeSettings) with an empty body or {"CatalogId": "<account id>"} against a catalog nothing has configured. The response is 200 and $.DataLakeSettings lacks DisallowGrantOnIAMAllowedPrincipals, SetSourceIdentity and WhitelistedForExternalDataFiltering; AWS returns them as false, false and [].
2. PutDataLakeSettings for the same catalog with DataLakeSettings carrying ExternalDataFilteringAllowList entries and Parameters {"SET_SOURCE_IDENTITY": "TRUE"}, then GetDataLakeSettings again: the three keys are still absent, where AWS reflects the written state in WhitelistedForExternalDataFiltering and SetSourceIdentity.
3. Replay fixture lakeformation-data-lake-settings-and-principal; steps task#4, task#6 and task#8 all mismatch on exactly those three JSON paths.
