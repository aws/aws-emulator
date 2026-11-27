Issue-Source: trace-sweep-divergence
Issue-Trace: evs-catalog-get-versions
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: evs-catalog-get-versions
Issue-Shape-Id: com.amazonaws.evs#AmazonElasticVMwareService
Issue-Key: evs|GetVersions|2xx->2xx|$.instanceTypeEsxVersions[instanceType="r7i.metal-24xl"]
Issue-Key: evs|GetVersions|2xx->2xx|$.instanceTypeEsxVersions[instanceType="r7i.metal-48xl"]
Issue-Key: evs|GetVersions|2xx->2xx|$.vcfVersions[*]
Issue-Commit: 5c66e8f363b2794f9d87fe11d0e172b4338f59d2

# GetVersions publishes the r7i.metal instance types Amazon EVS does not provide

> _Note: this issue was identified against the code base at commit `5c66e8f363b2794f9d87fe11d0e172b4338f59d2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetVersions

## Description
Cause 1 of 1: the EVS version catalogue treats EVERY value of the modelled `InstanceType` enum as hardware Amazon EVS provides, so both halves of the `GetVersions` response advertise the two r7i types that the real catalogue does not offer.

AWS: `GetVersions` is the discovery call that reports what Amazon EVS PROVIDES. Smithy: `GetVersions` takes an empty `GetVersionsRequest` and returns `GetVersionsResponse` with the two @required members `vcfVersions` (`VcfVersionList` of `VcfVersionInfo`) and `instanceTypeEsxVersions` (`InstanceTypeEsxVersionsList` of `InstanceTypeEsxVersionsInfo`); errors are only `InternalServerException` (500) and `ThrottlingException` (429). `VcfVersionInfo$instanceTypes` is documented as "EC2 instance types provided by Amazon EVS for this VCF version for creating environments" and `InstanceTypeEsxVersionsInfo` as "Information about ESX versions offered for each EC2 instance type" — i.e. both lists state what the service OFFERS, not what the `InstanceType` enum can spell. The `InstanceType` enum declares five values (`i4i.metal`, `i7i.metal-24xl`, `i7i.metal-48xl`, `r7i.metal-24xl`, `r7i.metal-48xl`), and `HostInfoForCreate$instanceType` documents "With VCF 5.2.x, you can use only the i4i.metal, i7i.metal-24xl, and i7i.metal-48xl instance types. All of these instance types are supported with vcfVersion set to SELF_DEPLOYED" — the enum therefore spells types the catalogue need not offer for a given version. The recording shows the catalogue AWS actually serves on this API version: `instanceTypeEsxVersions` carries an entry ONLY for the i-family types (`i4i.metal`, `i7i.metal-24xl`, `i7i.metal-48xl`) and NO entry for either r7i type, and the `VCF-5.2.2` and `SELF_DEPLOYED` entries each list `instanceTypes: ["i4i.metal", "i7i.metal-24xl", "i7i.metal-48xl"]`. HTTP 200 on both sides; this is a response-content divergence, not a status or error-code one.

Emulator: HTTP 200 with a catalogue derived from the enum. `instanceTypeEsxVersions` carries two EXTRA entries, `{instanceType: "r7i.metal-24xl", esxVersions: [...all nine offered builds...]}` and `{instanceType: "r7i.metal-48xl", esxVersions: [...the same nine...]}`, which AWS does not return at all. In `vcfVersions`, the `VCF-5.2.2` entry and the `SELF_DEPLOYED` entry each report `instanceTypes: ["i4i.metal", "i7i.metal-24xl", "i7i.metal-48xl", "r7i.metal-24xl", "r7i.metal-48xl"]` instead of the three i-family types. Everything else in the response matches: the `VCF-5.2.1` entry (status `RESTRICTED`, default `ESXi-8.0U3b-...`, narrowed instance-type list) is correct, both per-version `status` and `defaultEsxVersion` values are correct, and the per-instance-type `esxVersions` build list is correct for the types that ARE provided. So the root cause is specifically that the catalogue's instance-type dimension is populated from the modelled enum rather than stated per entry.

Why: a client that uses `GetVersions` as the discovery call it is documented to be — picking hardware for `CreateEnvironment`/`CreateEnvironmentHost` from the published `instanceTypes`, or sizing a host plan from `instanceTypeEsxVersions` — is told that `r7i.metal-24xl` and `r7i.metal-48xl` are available for VCF-5.2.2 and SELF_DEPLOYED. Against the emulator that choice then succeeds (the create paths validate against this same catalogue), so a plan or test suite calibrated on the emulator picks hardware that AWS never offered and breaks on the real service. The r7i entries in `instanceTypeEsxVersions` also make any client that enumerates provided hardware (an inventory or capacity report driven off this call) report machine types that do not exist in the service's offering.

Fix: the service's catalogue state, not the handler — `GetVersions`' handler is a faithful projection of it. The provided-instance-type dimension must be a per-entry catalogue fact rather than the modelled enum: the `VCF-5.2.2` and `SELF_DEPLOYED` rows of `PROVIDED_VCF_VERSIONS` must name their own instance-type lists (the three i-family types) instead of reusing `INSTANCE_TYPE_VALUES`, and the `instanceTypeEsxVersions` projection must iterate the instance types the catalogue actually provides (the union of the rows' lists) instead of every modelled `InstanceType` value, so the response carries no entry for a type the service does not offer. Because the create paths read the same catalogue, narrowing it also makes `CreateEnvironment`/`CreateEnvironmentHost` refuse r7i hardware, which is the behaviour the related open issue `20261127T144759Z-evs-createenvironmenthost-validation--r5.md` describes — that issue covers only the `VCF-5.2.2` row and explicitly leaves the `SELF_DEPLOYED` row and the `instanceTypeEsxVersions` list carrying every modelled type, so it does NOT cover this divergence and carries no replay key for it. Any crate test that asserts every modelled `InstanceType` value appears somewhere in the published catalogue encodes the defect and must be re-pointed at the recorded catalogue.

Traces: `evs-catalog-get-versions#task#1` (GetVersions, 200 vs 200, four diverging JSON paths: `$.instanceTypeEsxVersions[instanceType="r7i.metal-24xl"]`, `$.instanceTypeEsxVersions[instanceType="r7i.metal-48xl"]`, `$.vcfVersions[1]`, `$.vcfVersions[2]`). Repo citations: `services/service-com-amazonaws-evs-amazonelasticvmwareservice/src/state.rs:2989-3011` (`PROVIDED_VCF_VERSIONS`, rows at 2997-3003 and 3004-3010 using `INSTANCE_TYPE_VALUES`), `:3138-3152` (`provided_vcf_versions`), `:3173-3184` (`instance_type_esx_versions`, iterating `INSTANCE_TYPE_VALUES`), `src/ops/types/shared_types.rs:287` (`INSTANCE_TYPE_VALUES`), `src/ops/get_versions.rs:169-172` (the handler that projects both).

## Evidence
Replay of fixture `evs-catalog-get-versions`, step `task#1` (GetVersions). Expected status 200, actual status 200; the body diverges on four paths:

1. `$.instanceTypeEsxVersions[instanceType="r7i.metal-24xl"]`: extra — expected None, got `{'instanceType': 'r7i.metal-24xl', 'esxVersions': ['ESXi-8.0U3b-24280767', 'ESXi-8.0U3g-24859861', 'ESXi-9.0.2.0.25148076', 'ESXi-9.0.2.0100.25595025', 'ESXi-9.1.0.0100.25433460', 'ESXi-9.1.0.0200.25557999', 'ESXi-8.0U3i-25205845', 'ESXi-8.0U3j-25429389', 'ESXi-8.0U3k-25595708']}`.
2. `$.instanceTypeEsxVersions[instanceType="r7i.metal-48xl"]`: extra — expected None, got the same nine builds under `instanceType: 'r7i.metal-48xl'`.
3. `$.vcfVersions[1]` (VCF-5.2.2): expected `instanceTypes: ['i4i.metal', 'i7i.metal-24xl', 'i7i.metal-48xl']`, got `['i4i.metal', 'i7i.metal-24xl', 'i7i.metal-48xl', 'r7i.metal-24xl', 'r7i.metal-48xl']` (status `AVAILABLE` and `defaultEsxVersion` `ESXi-8.0U3g-24859861` both matched).
4. `$.vcfVersions[2]` (SELF_DEPLOYED): expected `instanceTypes: ['i4i.metal', 'i7i.metal-24xl', 'i7i.metal-48xl']`, got the same five-type list (status `AVAILABLE` and `defaultEsxVersion` `ESXi-9.0.2.0.25148076` both matched).

`$.vcfVersions[0]` (VCF-5.2.1, status `RESTRICTED`) matched, and the `esxVersions` build lists for the three provided i-family types matched — so only the instance-type dimension of the catalogue diverges.

Model evidence:
* `smithy_query(query="operation", operation="GetVersions")` — empty input; output members `vcfVersions` (required) and `instanceTypeEsxVersions` (required); errors `InternalServerException` (500), `ThrottlingException` (429).
* `smithy_query(query="shape", shape="InstanceType")` — values `["i4i.metal", "i7i.metal-24xl", "i7i.metal-48xl", "r7i.metal-24xl", "r7i.metal-48xl"]`; `shape="VcfVersion"` — `["VCF-5.2.1", "VCF-5.2.2", "SELF_DEPLOYED"]`.
* `smithy_query(query="shape", shape="VcfVersionInfo")` — `instanceTypes` doc: "EC2 instance types provided by Amazon EVS for this VCF version for creating environments"; `smithy_query(query="shape", shape="InstanceTypeEsxVersionsInfo")` — `esxVersions` doc: "The list of ESX versions offered for this instance type".
* `smithy_query(query="doc", shape="HostInfoForCreate", member="instanceType")` — "With VCF 5.2.x, you can use only the i4i.metal, i7i.metal-24xl, and i7i.metal-48xl instance types. All of these instance types are supported with vcfVersion set to SELF_DEPLOYED."

Emulator source: `src/state.rs:3002` and `:3009` set `instance_types: INSTANCE_TYPE_VALUES` (all five modelled types) for the `VCF-5.2.2` and `SELF_DEPLOYED` rows, and `src/state.rs:3173-3184` builds `instanceTypeEsxVersions` by mapping over `INSTANCE_TYPE_VALUES`, which is why two entries AWS never returns appear.

## Steps to Reproduce
1. Boot the emulator from this checkout with the EVS service mounted and replay fixture `evs-catalog-get-versions`, step `task#1`.
2. Issue the recorded `GetVersions` request (empty body) to the EVS endpoint.
3. Read `$.instanceTypeEsxVersions` in the 200 response: the emulator returns five entries, including `instanceType: "r7i.metal-24xl"` and `instanceType: "r7i.metal-48xl"`. AWS returns entries only for `i4i.metal`, `i7i.metal-24xl` and `i7i.metal-48xl`.
4. Read `$.vcfVersions[?(@.vcfVersion=='VCF-5.2.2')].instanceTypes` and `$.vcfVersions[?(@.vcfVersion=='SELF_DEPLOYED')].instanceTypes`: both report all five modelled instance types; AWS reports only the three i-family types for each.
5. Confirm the catalogue is applied as well as published: `CreateEnvironment` with `vcfVersion: "VCF-5.2.2"`, let the environment settle to `CREATED`, then `CreateEnvironmentHost` with `host.instanceType: "r7i.metal-24xl"` — the emulator answers 200 and persists a host on hardware the published AWS catalogue does not offer for that version.
6. Contrast with `VCF-5.2.1`, whose catalogue row states its own narrowed list: the same request there is refused through the per-version instance-type check, showing the divergence is the enum-derived rows rather than a missing check.
