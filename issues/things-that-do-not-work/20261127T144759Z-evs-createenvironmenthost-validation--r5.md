Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: f9b7321d6a22920f7cbed43b33d059dedecbc1ee

# A VCF-5.2.2 host is admitted on the r7i instance types, which the instanceType member doc excludes for VCF 5.2.x

_Filed: 2026-11-27T14:47:59Z_
_Operation: CreateEnvironmentHost | Class: validation_

> _Note: this issue was identified against the code base at commit `f9b7321d6a22920f7cbed43b33d059dedecbc1ee`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The per-version instance-type restriction this crate applies is read from `crate::state::PROVIDED_VCF_VERSIONS`. The `VCF-5.2.1` row states its own narrowed list, but the `VCF-5.2.2` row states `instance_types: INSTANCE_TYPE_VALUES` — the WHOLE modelled `InstanceType` enum. That enum carries five values, two of which (`r7i.metal-24xl`, `r7i.metal-48xl`) the `HostInfoForCreate$instanceType` documentation explicitly excludes for VCF 5.2.x: "With VCF 5.2.x, you can use only the i4i.metal, i7i.metal-24xl, and i7i.metal-48xl instance types." `VCF-5.2.2` is a VCF 5.2.x version (`VcfVersion` declares exactly `VCF-5.2.1 | VCF-5.2.2 | SELF_DEPLOYED`).

So `CreateEnvironmentHost` against a `VCF-5.2.2` environment answers HTTP 200 and persists a host on `r7i.metal-24xl` / `r7i.metal-48xl`, and `CreateEnvironment` materialises `hosts` entries naming them, where the member's own doc says those types cannot be used with that version. `GetVersions` publishes the same unrestricted list for `VCF-5.2.2`, so the discovery call also tells a caller it may pass hardware the doc excludes.

This is also inconsistent inside the crate: `VCF-5.2.1`, the OTHER VCF 5.2.x version, is narrowed by the same table and a request naming a type outside its row IS refused through the same helper. Nothing in the crate names the r7i types as hardware `VCF-5.2.2` is offered on — a grep for `r7i` over `src/` returns only the generated enum constant — so `INSTANCE_TYPE_VALUES` is standing in for a per-version catalogue fact rather than stating one, which is exactly what the field's own doc says it must not do.

Note this is NOT the already-fixed 'the create path never consults the catalogue' gap (the check exists and runs) and NOT a reversal of the recording-pinned `VCF-5.2.1` row (which stays as it is): the divergence is the `VCF-5.2.2` row's content, which started admitting hardware the doc excludes once the modelled enum grew past the three i-family types. The `SELF_DEPLOYED` row is a separate case — the quoted sentence only affirms the three types for it and restricts nothing — so leaving that row carrying every modelled type keeps `tests/all/modelled_instance_type_surface.rs` satisfied.

## Evidence

Smithy, `smithy_query(query="doc", shape="HostInfoForCreate", member="instanceType")` — the sentence that states the behaviour:

    <p>The EC2 instance type that represents the host.</p> <note> <p>With VCF 5.2.x, you can
    use only the <code>i4i.metal</code>, <code>i7i.metal-24xl</code>, and
    <code>i7i.metal-48xl</code> instance types. All of these instance types are supported
    with <code>vcfVersion</code> set to <code>SELF_DEPLOYED</code>.</p> </note>

and `smithy_query(query="shape", shape="VcfVersion,InstanceType")`:

    "VcfVersion":  values: ["VCF-5.2.1", "VCF-5.2.2", "SELF_DEPLOYED"]
    "InstanceType": values: ["i4i.metal", "i7i.metal-24xl", "i7i.metal-48xl", "r7i.metal-24xl", "r7i.metal-48xl"]

so `VCF-5.2.2` is a VCF 5.2.x version and the enum carries two types the sentence excludes for it.

The repo side — `services/service-com-amazonaws-evs-amazonelasticvmwareservice/src/ops/types/shared_types.rs:287`:

    pub const INSTANCE_TYPE_VALUES: &[&str] = &["i4i.metal", "i7i.metal-24xl", "i7i.metal-48xl", "r7i.metal-24xl", "r7i.metal-48xl"];

and the catalogue row for `VCF-5.2.2`, `src/state.rs:2997-3003`:

    ProvidedVcfVersion {
        vcf_version: "VCF-5.2.2",
        status: VERSION_STATUS_AVAILABLE,
        esx_line: ESX_LINE_8_0_U3,
        default_esx_version: "ESXi-8.0U3g-24859861",
        instance_types: INSTANCE_TYPE_VALUES,
    },

beside the OTHER VCF 5.2.x row, which IS narrowed — `src/state.rs:2990-2996`:

    ProvidedVcfVersion {
        vcf_version: "VCF-5.2.1",
        ...
        instance_types: &["i4i.metal", "i7i.metal-24xl"],
    },

The check that lets the r7i types through, `src/state.rs:3108-3116`:

    pub fn unprovided_instance_type_failure(
        member: &str,
        instance_type: &str,
        vcf_version: &str,
    ) -> Option<crate::ops::types::ValidationExceptionField> {
        let provided = provided_instance_types(vcf_version);
        if provided.is_empty() || provided.contains(&instance_type) {
            return None;
        }

reached from `src/ops/create_environment_host.rs:367-371`:

        failures.extend(crate::state::unprovided_instance_type_failure(
            "host.instanceType",
            &input.host.instance_type,
            &environment.vcf_version,
        ));

and per `hosts` entry from `src/ops/create_environment.rs:559-575`.

The crate's own rule for this field, `src/state.rs:2967-2976`:

    /// The EC2 instance types Amazon EVS offers this version on — "EC2 instance types
    /// provided by Amazon EVS for this VCF version for creating environments"
    /// (`VcfVersionInfo$instanceTypes`), stated PER version: a version is offered a subset
    /// of the modelled `InstanceType` values, which is the whole of
    /// [`INSTANCE_TYPE_VALUES`] for a version offered on every type Amazon EVS provides
    /// and a narrower list for one it does not. Published AND applied: a host is built on
    /// one of the types its environment's version names here
    /// ([`unprovided_instance_type_failure`]) ...

No comment, constant or test in the crate says `VCF-5.2.2` is offered on the r7i types: `grep -n 'r7i' src/` returns only `src/ops/types/shared_types.rs:287`.

Test coverage (nothing pins today's acceptance of r7i on VCF-5.2.2):
* `tests/all/host_instance_type_catalogue.rs:30-32` — "The expectations are read from the WIRE catalogue rather than written here: every row's verdict is whether the `GetVersions` response lists that instance type for that VCF version" — so the per-row verdicts follow a narrowed catalogue.
* `tests/all/versions_catalog.rs:349-359` asserts only that the published `instanceTypes` equal `evs::PROVIDED_VCF_VERSIONS` (a projection check).
* `tests/all/modelled_instance_type_surface.rs:239-246` and `:257-268` require every modelled type be provided for SOME reported version — satisfied by the `SELF_DEPLOYED` row, which the quoted sentence does not restrict.
* The suites that build hosts on a concrete type (`created_environment_hosts.rs:443-448`, `host_ec2_instances.rs:360-361`, `environment_health_checks.rs:296`, `check_impairment_onset.rs:224`) all use `i4i.metal` / `i7i.metal-48xl`, which `VCF-5.2.2` keeps providing.

## Steps to Reproduce

1. Start the emulator with the EVS service mounted on the in-memory engine (the `emulator()` harness used by `tests/all/host_instance_type_catalogue.rs`).
2. Call `GetVersions` and read the `VCF-5.2.2` entry's `instanceTypes`: it reports all five modelled types, `r7i.metal-24xl` and `r7i.metal-48xl` included (projected from `src/state.rs:3002`).
3. `CreateEnvironment` with `vcfVersion: "VCF-5.2.2"` and the rest of a valid request, then settle the clock so the environment reaches `CREATED`.
4. Call `CreateEnvironmentHost` with that `environmentId` and `host: {hostName: "esx-r7i", keyName: "evs-key", instanceType: "r7i.metal-24xl"}`.
5. Observe HTTP 200 and a persisted host: `ListEnvironmentHosts` reports `instanceType: "r7i.metal-24xl"` for a VCF 5.2.x environment, although `HostInfoForCreate$instanceType` documents that with VCF 5.2.x "you can use only the i4i.metal, i7i.metal-24xl, and i7i.metal-48xl instance types".
6. For contrast, repeat step 4 against a `VCF-5.2.1` environment: the request IS refused with the aggregated `ValidationException` naming `host.instanceType`, through the same `unprovided_instance_type_failure` helper — the two VCF 5.2.x versions disagree.
7. Same divergence through `CreateEnvironment` itself: pass `hosts` entries with `instanceType: "r7i.metal-48xl"` alongside `vcfVersion: "VCF-5.2.2"` — the entries are materialised as hosts rather than refused.
