Issue-Source: trace-sweep-divergence
Issue-Trace: lightsail-catalog-get-blueprints
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: lightsail-catalog-get-blueprints lightsail-catalog-and-operations-lifecycle lightsail-catalog-get-bundles lightsail-catalog-get-regions lightsail-cloudformation-and-copy-error-paths lightsail-instance-error-paths lightsail-catalog-get-relational-database-blueprints lightsail-catalog-get-relational-database-bundles
Issue-Shape-Id: com.amazonaws.lightsail#Lightsail_20161128
Issue-Key: lightsail|GetBlueprints|2xx->2xx|$.blueprints[*]
Issue-Key: lightsail|GetBundles|2xx->2xx|$.bundles[*]
Issue-Key: lightsail|GetRegions|2xx->2xx|$.regions[name="ap-east-1"].description
Issue-Key: lightsail|GetRegions|2xx->2xx|$.regions[name="ap-south-1"].description
Issue-Key: lightsail|GetRegions|2xx->2xx|$.regions[name="ap-southeast-1"].description
Issue-Key: lightsail|GetRegions|2xx->2xx|$.regions[name="ap-southeast-2"].description
Issue-Key: lightsail|GetRegions|2xx->2xx|$.regions[name="ap-southeast-3"].description
Issue-Key: lightsail|GetRegions|2xx->2xx|$.regions[name="ap-southeast-5"].description
Issue-Key: lightsail|GetRegions|2xx->2xx|$.regions[name="ca-central-1"].description
Issue-Key: lightsail|GetRegions|2xx->2xx|$.regions[name="eu-south-2"].description
Issue-Key: lightsail|GetRegions|2xx->2xx|$.regions[name="eu-west-3"].description
Issue-Key: lightsail|GetRegions|2xx->2xx|$.regions[name="sa-east-1"].description
Issue-Key: lightsail|GetRelationalDatabaseBlueprints|2xx->2xx|$.blueprints[*]
Issue-Key: lightsail|GetRelationalDatabaseBundles|2xx->2xx|$.bundles[name="2Xlarge with High Availability"].price
Issue-Key: lightsail|GetRelationalDatabaseBundles|2xx->2xx|$.bundles[name="2Xlarge"].price
Issue-Key: lightsail|GetRelationalDatabaseBundles|2xx->2xx|$.bundles[name="Xlarge with High Availability"].price
Issue-Key: lightsail|GetRelationalDatabaseBundles|2xx->2xx|$.bundles[name="Xlarge"].price
Issue-Commit: 3e431842aa1114a5335d2ac8ff029b97b6e2d1f1

# Lightsail catalog tables still serve emulator-authored descriptions, prices and truncated tiers

> _Note: this issue was identified against the code base at commit `3e431842aa1114a5335d2ac8ff029b97b6e2d1f1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetBlueprints

## Description
Six net-new root causes in the Lightsail static-catalog family. Every one of the read-only catalog operations below answers 200 from a hand-written reference table in the crate, and each table still diverges from the catalog AWS serves: emulator-authored description prose and product/license URLs instead of AWS's, a catalog `version` column that is the OS year instead of AWS's catalog date, missing blueprint and bundle tiers (so every later list position shifts), stale prices, a stale engine minor version, and shortened region descriptions that drop AWS's geography wording. These are DATA faults in six separate tables, so each is its own cause and each must be closed separately; the issue is MEDIUM because each operation answers and is well-formed, which is exactly why a client cannot detect the divergence.

Cause 1 of 6: GetBlueprints serves hand-written blueprint descriptions, a wrong `version` column and non-AWS productUrl/licenseUrl, so all 39 recorded entries mismatch.
AWS: Smithy (com.amazonaws.lightsail#GetBlueprints -> output member `blueprints`, a BlueprintList of com.amazonaws.lightsail#Blueprint) models blueprintId, name, group, type, description, isActive, minPower, version, versionCode, productUrl, licenseUrl, platform and appCategory; the model fixes the members but not their values, so the catalog contents are recorded behavior only. The recording shows, for each blueprint, AWS's own marketplace description text (for blueprintId 'windows_server_2025': the full 'Amazon Lightsail helps you build, deploy, scale, and manage Microsoft applications ... requiring Windows software.' paragraph; for 'windows_server_2022': the 'Windows Server is an environment for deploying applications using the Microsoft Web Platform ... Learn more about Windows Server 2022 on the AWS Marketplace.' paragraph), a `version` that is the catalog build date '2026.09.17' rather than the OS release year, a productUrl under aws.amazon.com/marketplace/pp/<prodview-id> and a licenseUrl under d7umqicpi7263.cloudfront.net/eula/product/<id>/<id>.txt.
Emulator: the same blueprintId/name/group/minPower are served, but description is a one-line emulator sentence ('Windows Server 2025 operating system image for a Lightsail instance.'), version is the OS year ('2025'), productUrl points at www.microsoft.com and licenseUrl at www.microsoft.com/licensing/docs/view/Product-Terms. Status is 200 -> 200; the divergence is on $.blueprints[0] through $.blueprints[38], i.e. every entry of the listing.
Why: a client that shows or matches a blueprint description, pins a blueprint by catalog `version`, or follows the licence/product link gets emulator prose and the wrong link for every image in the catalog; because blueprintId and name are right, the listing looks correct and the mismatch surfaces only downstream.
Fix: the crate's static instance-blueprint catalog table backing the GetBlueprints handler — the description, version, productUrl and licenseUrl columns, for every row. Not handler logic: the serializer and the member set are already correct.
Traces: lightsail-catalog-get-blueprints#task#1.

Cause 2 of 6: the same blueprint table omits the Windows SQL Server blueprints, so the list is short and positions after index 3 shift.
AWS: recorded behavior — $.blueprints[4] is blueprintId 'windows_server_2022_sql_2022_express' (name 'SQL Server 2022 Express', group 'windows_2022_sql_exp_2022', type 'app', platform 'WINDOWS', minPower 2000, version '2026.09.17', versionCode '1', with the marketplace productUrl/licenseUrl for prodview-c2jz4lr4h2yc6), i.e. the Windows SQL app blueprints follow the plain Windows OS blueprints before any Linux entry.
Emulator: $.blueprints[4] is 'amazon_linux_2023' (group 'amazon-linux', type 'os', platform 'LINUX_UNIX', minPower 0) — the Windows SQL Server rows are absent entirely, so the Linux block starts one tier early and every subsequent index is off. Status 200 -> 200.
Why: the Windows SQL blueprints cannot be selected at all, so no SQL-Server instance workflow can be driven; and a client reading the catalog by position gets a Linux OS image where AWS serves a Windows app image, including a minPower of 0 instead of 2000, which silently permits a bundle too small for the image.
Fix: the same static blueprint catalog table — add the missing Windows SQL Server app rows in AWS's order with their minPower/type/group values. This cause is listed separately because it is a missing-row fault rather than a wrong-column fault, and it is observed through a different fixture path.
Traces: lightsail-catalog-and-operations-lifecycle#task#4.

Cause 3 of 6: GetBundles omits the instance bundle tiers above 4xlarge and prices 4xlarge wrong.
AWS: Smithy (com.amazonaws.lightsail#GetBundles -> `bundles`, a BundleList of com.amazonaws.lightsail#Bundle: bundleId, name, price, cpuCount, diskSizeInGb, instanceType, isActive, power, ramSizeInGb, transferPerMonthInGb, supportedPlatforms, publicIpv4AddressCount). Values are recorded behavior only: the recording serves the LINUX_UNIX tiers in ascending size — '4xlarge_3_0' (cpuCount 16, ram 64.0, power 6000, price 384.0), then '8xlarge_3_0' (cpuCount 32, ram 128.0, power 7000, price 884.0, transfer 9216), '12xlarge_3_0' (cpuCount 48, ram 192.0, power 7500, price 1324.0, transfer 10240), '16xlarge_3_0' (cpuCount 64, power 8000, price 1764.0) — and only then the WINDOWS variants.
Emulator: '4xlarge_3_0' is served with price 324.0 instead of 384.0, and the Linux tiers above it do not exist: index 8 is 'nano_win_3_0' and index 9 is 'micro_win_3_0', i.e. the WINDOWS block begins where AWS still has large Linux tiers, so $.bundles[7] through $.bundles[99] all mismatch. Status 200 -> 200.
Why: bundleId from this listing is the value passed to CreateInstances and `power` is what a blueprint's minPower is compared against, so the largest Linux bundles cannot be selected at all, and any sizing or cost calculation over the 4xlarge tier is understated by 60 USD/month.
Fix: the crate's static instance-bundle catalog table backing GetBundles — correct the 4xlarge price and add the 8xlarge/12xlarge/16xlarge (and larger) LINUX_UNIX rows ahead of the WINDOWS variants.
Traces: lightsail-catalog-get-bundles#task#1.

Cause 4 of 6: GetRegions still serves shortened region descriptions that drop AWS's geography wording.
AWS: Smithy (com.amazonaws.lightsail#GetRegions -> `regions`, a RegionList of com.amazonaws.lightsail#RegionInfo with continentCode, description, displayName, name, availabilityZones, relationalDatabaseAvailabilityZones). The description text is recorded behavior only; the recording shows AWS naming the wider served geography and, for opt-in regions, the fixed prefix 'This opt-in Region (Region that is disabled by default) is recommended to serve users in ' with the geography and a closing period: eu-west-3 'This region is recommended to serve users in France and central Europe', ap-southeast-1 'This region is recommended to serve users in India and Southeast Asia', ap-southeast-2 'This region is recommended to serve users in Austalia, New Zealand, and the South Pacific' (AWS's own spelling of 'Austalia' is part of the served string), ap-south-1 'This region is recommended to serve users in India and southern Asia', eu-south-2 '... in Spain and South Europe.', ap-southeast-3 '... in Indonesia and Southeast Asia.', ap-southeast-5 '... in Malaysia and Southeast Asia.', ap-east-1 '... in Hong Kong and Asia.'.
Emulator: the opt-in prefix is now present, but each description is truncated to the single territory and drops AWS's regional clause — 'This region is recommended to serve users in France', '... in Singapore and southeast Asia' (wrong territory for ap-southeast-1), '... in Australia and New Zealand', '... in Spain', '... in Indonesia', '... in Malaysia', '... in Hong Kong' — diverging on 10 regions (ap-east-1, ap-south-1, ap-southeast-1, ap-southeast-2, ap-southeast-3, ap-southeast-5, ca-central-1, eu-south-2, eu-west-3, sa-east-1). Status 200 -> 200.
Why: this description is the only human-readable guidance in the response and is what a console or region picker renders; ap-southeast-1 is additionally described as serving a different country than AWS states, so a client choosing a region on this text chooses differently than it would against AWS.
Fix: the crate's static region table backing GetRegions — the description column for those 10 rows, using AWS's full sentence per region (fixed lead-in plus AWS's geography clause, opt-in rows keeping the opt-in prefix and the trailing period).
Traces: lightsail-catalog-get-regions#task#1 (the same cluster also covers lightsail-cloudformation-and-copy-error-paths#task#1 and lightsail-instance-error-paths#task#7).

Cause 5 of 6: GetRelationalDatabaseBlueprints serves a stale PostgreSQL 18 minor version.
AWS: Smithy (com.amazonaws.lightsail#GetRelationalDatabaseBlueprints -> `blueprints`, a RelationalDatabaseBlueprintList of com.amazonaws.lightsail#RelationalDatabaseBlueprint: blueprintId, engine, engineVersion, engineDescription, engineVersionDescription, engineVersionRequired, isEngineDefault). Values are recorded behavior only: the recording serves blueprintId 'postgres_18' with engineVersion '18.6' and engineVersionDescription 'PostgreSQL 18.6-R1', i.e. engineVersionDescription is '<engineDescription> <engineVersion>-R1'.
Emulator: 'postgres_18' is served with engineVersion '18.5' and engineVersionDescription 'PostgreSQL 18.5-R1' — the '-R1' form is right, the minor version is one release behind, so $.blueprints[6] mismatches. Status 200 -> 200.
Why: a client that creates a database from this blueprintId and then asserts the engine version it received, or that pins a minor version read from the catalog, reads a version the service does not offer.
Fix: the crate's static relational-database blueprint table backing GetRelationalDatabaseBlueprints — the engineVersion/engineVersionDescription columns of the postgres_18 row.
Traces: lightsail-catalog-get-relational-database-blueprints#task#1.

Cause 6 of 6: GetRelationalDatabaseBundles prices the Xlarge and 2Xlarge tiers below AWS.
AWS: Smithy (com.amazonaws.lightsail#GetRelationalDatabaseBundles -> `bundles`, a RelationalDatabaseBundleList of com.amazonaws.lightsail#RelationalDatabaseBundle: bundleId, name, price, ramSizeInGb, diskSizeInGb, transferPerMonthInGb, cpuCount, isEncrypted, isActive). `price` is documented as the cost of the bundle in US dollars and its values are recorded behavior only: the recording serves 'Xlarge' at 245.0, 'Xlarge with High Availability' at 490.0, '2Xlarge' at 490.0 and '2Xlarge with High Availability' at 980.0 — the HA variant being exactly twice its standard tier.
Emulator: those four tiers are present with the right names but priced 230.0, 460.0, 460.0 and 920.0. Status 200 -> 200; no other member of the listing diverges.
Why: this listing is the source for managed-database cost estimation and budget checks, so a client plans against prices roughly 6% below the ones AWS charges, and the discrepancy grows with the tier.
Fix: the crate's static relational-database bundle table backing GetRelationalDatabaseBundles — the price column of the xlarge, xlarge-HA, 2xlarge and 2xlarge-HA rows.
Traces: lightsail-catalog-get-relational-database-bundles#task#1.

Related but NOT covering any of these: the RESOLVED issue 'Lightsail static catalog listings serve stale, reordered and incomplete entries' (20261029T234852Z-lightsail-static-catalog-listings-serve-stale-reordered-and.md) covered the earlier state of these same tables (wrong ordering, 1_0 relational bundle generation, missing '-R1' suffix, missing opt-in prefix). Those specific defects are gone; the six causes above are what these tables still get wrong, they carry no owning open issue (replay reported owned_by empty for each), and a resolved issue covers nothing. The open issue 'Lightsail GetContainerServicePowers serves wrong RAM sizes and a wrong small-tier price' (20261030T201423Z) is the same family but a different table and is not re-filed here.

## Evidence
Replay of simparity-wire-aws-v1 against the emulator built from this checkout (emulator_up=true, 34 steps, 17 passed, score 50%). All six clusters are verdict=mismatch with expected_status 200 and actual_status 200 — the operations answer, the catalog data diverges — and each reported stub_path empty and owned_by empty, i.e. no open issue carries its replay key.

Cause 1 — lightsail-catalog-get-blueprints#task#1 (GetBlueprints), divergence_key lightsail|GetBlueprints|status=200->200|keys=$.blueprints[0]..$.blueprints[38] (all 39 positions):
  $.blueprints[0] expected blueprintId 'windows_server_2025' with description 'Amazon Lightsail helps you build, deploy, scale, and manage Microsoft applications quickly, easily, and cost effectively with Windows Server 2025. ... and running any other application requiring Windows software.', version '2026.09.17', productUrl 'https://aws.amazon.com/marketplace/pp/prodview-g76csvka4r3by', licenseUrl 'https://d7umqicpi7263.cloudfront.net/eula/product/b0e459f5-2c38-4939-ad2a-d9784ecab690/0e7c7387-f24a-45b5-9206-e5ab5ff5fd9c.txt'; got the same blueprintId/name/group/minPower(500)/versionCode('1') but description 'Windows Server 2025 operating system image for a Lightsail instance.', version '2025', productUrl 'https://www.microsoft.com/en-us/windows-server', licenseUrl 'https://www.microsoft.com/licensing/docs/view/Product-Terms'.
  $.blueprints[1] expected 'windows_server_2022' with AWS's 'Windows Server is an environment for deploying applications using the Microsoft Web Platform ...' description and the same d7umqicpi7263.cloudfront.net licence host.

Cause 2 — lightsail-catalog-and-operations-lifecycle#task#4 (GetBlueprints), keys=$.blueprints[4]:
  expected {'blueprintId': 'windows_server_2022_sql_2022_express', 'name': 'SQL Server 2022 Express', 'group': 'windows_2022_sql_exp_2022', 'type': 'app', 'platform': 'WINDOWS', 'minPower': 2000, 'version': '2026.09.17', 'versionCode': '1'}; got {'blueprintId': 'amazon_linux_2023', 'name': 'Amazon Linux 2023', 'group': 'amazon-linux', 'type': 'os', 'platform': 'LINUX_UNIX', 'minPower': 0, 'version': '2023'}. Indices 0..3 matched in this fixture, which isolates the fault to the missing Windows SQL rows rather than to ordering of the Windows OS block.

Cause 3 — lightsail-catalog-get-bundles#task#1 (GetBundles), keys=$.bundles[7]..$.bundles[99]:
  $.bundles[7] expected '4xlarge_3_0' price 384.0 (cpuCount 16, ram 64.0, disk 1280, power 6000, transfer 8192); got the same row with price 324.0.
  $.bundles[8] expected '8xlarge_3_0' (cpuCount 32, ram 128.0, power 7000, price 884.0, transfer 9216); got 'nano_win_3_0' (WINDOWS, price 9.5).
  $.bundles[9] expected '12xlarge_3_0' (cpuCount 48, ram 192.0, power 7500, price 1324.0, transfer 10240); got 'micro_win_3_0' (WINDOWS, price 16.0).
  $.bundles[10] expected '16xlarge_3_0' (cpuCount 64, power 8000, price 1764.0). The large LINUX_UNIX tiers are absent and the WINDOWS block begins in their place, so every position from 7 on mismatches.

Cause 4 — lightsail-catalog-get-regions#task#1 (GetRegions; the cluster also covers lightsail-cloudformation-and-copy-error-paths#task#1 and lightsail-instance-error-paths#task#7), keys = the `description` of ap-east-1, ap-south-1, ap-southeast-1, ap-southeast-2, ap-southeast-3, ap-southeast-5, ca-central-1, eu-south-2, eu-west-3, sa-east-1:
  eu-west-3 expected 'This region is recommended to serve users in France and central Europe', got 'This region is recommended to serve users in France'.
  ap-southeast-1 expected '... in India and Southeast Asia', got '... in Singapore and southeast Asia'.
  ap-southeast-2 expected '... in Austalia, New Zealand, and the South Pacific', got '... in Australia and New Zealand'.
  ap-south-1 expected '... in India and southern Asia'.
  eu-south-2 expected 'This opt-in Region (Region that is disabled by default) is recommended to serve users in Spain and South Europe.', got '... in Spain'.
  ap-southeast-3 expected the opt-in form '... in Indonesia and Southeast Asia.', got '... in Indonesia'. ap-southeast-5 expected '... in Malaysia and Southeast Asia.', got '... in Malaysia'. ap-east-1 expected '... in Hong Kong and Asia.', got '... in Hong Kong'.
  In each case the fixed lead-in ('This region is recommended to serve users in ' / the opt-in prefix) matches and only AWS's geography clause is missing or replaced.

Cause 5 — lightsail-catalog-get-relational-database-blueprints#task#1, keys=$.blueprints[6]:
  expected {'blueprintId': 'postgres_18', 'engine': 'postgres', 'engineDescription': 'PostgreSQL', 'engineVersion': '18.6', 'engineVersionDescription': 'PostgreSQL 18.6-R1', 'isEngineDefault': True}; got the same row with engineVersion '18.5' and engineVersionDescription 'PostgreSQL 18.5-R1'.

Cause 6 — lightsail-catalog-get-relational-database-bundles#task#1:
  $.bundles[name="Xlarge"].price expected 245.0, got 230.0.
  $.bundles[name="Xlarge with High Availability"].price expected 490.0, got 460.0.
  $.bundles[name="2Xlarge"].price expected 490.0, got 460.0.
  $.bundles[name="2Xlarge with High Availability"].price expected 980.0, got 920.0.

No status, error code or error-message divergence is involved in any of the six: each is a 200 whose catalog rows or columns are wrong. The literal strings, versions and prices quoted above are the recorded contents of static reference tables, which is precisely what this family of operations is required to serve.

## Steps to Reproduce
Boot the emulator and point a botocore lightsail client at its data listener.

Cause 1 — ls.get_blueprints(): blueprints[0] must be 'windows_server_2025' with AWS's full marketplace description paragraph, version '2026.09.17', productUrl under aws.amazon.com/marketplace/pp/ and licenseUrl under d7umqicpi7263.cloudfront.net/eula/product/. Observed a one-line emulator description, version '2025' and microsoft.com URLs; repeat for blueprints[1] ('windows_server_2022') and every other row.
Cause 2 — ls.get_blueprints() in the catalog/operations lifecycle path: blueprints[4] must be 'windows_server_2022_sql_2022_express' (type 'app', platform 'WINDOWS', minPower 2000). Observed 'amazon_linux_2023' (type 'os', LINUX_UNIX, minPower 0) — assert the Windows SQL Server app blueprints exist at all.
Cause 3 — ls.get_bundles(): bundles[7] must be '4xlarge_3_0' with price 384.0 (observed 324.0), and bundles[8..10] must be '8xlarge_3_0' / '12xlarge_3_0' / '16xlarge_3_0' before any WINDOWS bundle. Observed 'nano_win_3_0' and 'micro_win_3_0' at 8 and 9.
Cause 4 — ls.get_regions(): index the regions by name and assert each description is AWS's full sentence, e.g. eu-west-3 '... in France and central Europe', ap-southeast-1 '... in India and Southeast Asia', ap-southeast-2 '... in Austalia, New Zealand, and the South Pacific', eu-south-2 the opt-in form '... in Spain and South Europe.'. Observed the territory-only form for all ten of ap-east-1, ap-south-1, ap-southeast-1, ap-southeast-2, ap-southeast-3, ap-southeast-5, ca-central-1, eu-south-2, eu-west-3 and sa-east-1.
Cause 5 — ls.get_relational_database_blueprints(): the 'postgres_18' row must report engineVersion '18.6' and engineVersionDescription 'PostgreSQL 18.6-R1'. Observed '18.5' / 'PostgreSQL 18.5-R1'.
Cause 6 — ls.get_relational_database_bundles(): index bundles by name and assert price Xlarge 245.0, 'Xlarge with High Availability' 490.0, 2Xlarge 490.0, '2Xlarge with High Availability' 980.0. Observed 230.0, 460.0, 460.0 and 920.0.

All six causes must be closed before submitting. Re-replay: lightsail-catalog-get-blueprints#task#1, lightsail-catalog-and-operations-lifecycle#task#4, lightsail-catalog-get-bundles#task#1, lightsail-catalog-get-regions#task#1, lightsail-catalog-get-relational-database-blueprints#task#1, lightsail-catalog-get-relational-database-bundles#task#1.
