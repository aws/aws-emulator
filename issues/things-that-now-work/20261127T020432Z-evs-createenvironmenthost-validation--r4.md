Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: b4d546d4232932d32ca317b0739ae1583d9e6982

# A host is admitted on an instance type this service's own GetVersions catalogue does not provide for the environment's VCF version

_Filed: 2026-11-27T02:04:32Z_
_Operation: CreateEnvironmentHost | Class: validation_

> _Note: this issue was identified against the code base at commit `b4d546d4232932d32ca317b0739ae1583d9e6982`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

This crate holds ONE version catalogue (`crate::state::PROVIDED_VCF_VERSIONS`) and `GetVersions` publishes it. For each VCF version the catalogue states a PER-VERSION instance-type list, and for `VCF-5.2.1` that list is a strict SUBSET of the modelled `InstanceType` enum: `["i4i.metal", "i7i.metal-24xl"]`, leaving out `i7i.metal-48xl`.

No create path ever consults that list. `crate::state::provided_vcf_version(..).instance_types` is read in exactly one place — `provided_vcf_versions()`, the `GetVersions` projection (grep over the crate: the only non-test readers are `state.rs:3011` and the field decl/doc). So `CreateEnvironmentHost` on a `VCF-5.2.1` environment accepts `host.instanceType = i7i.metal-48xl` and persists a host on hardware the same service's catalogue says that version is not provided on; the `hosts` entries of `CreateEnvironment` are admitted the same way, which is the case the member doc is literally about ("...for this VCF version for creating environments").

Two repo sides contradict each other here:

1. The SAME handler already applies the catalogue's other per-version restriction. `create_environment_host.rs:342-351` reads `available_esx_versions(&environment.vcf_version)` — derived from the very same `ProvidedVcfVersion` entry — and refuses an `esxVersion` the catalogue does not offer for that version. The sibling restriction carried on the sibling field of the same catalogue entry is never asked.

2. The crate states the principle "do not publish a restriction the service does not apply" and then breaks it in the other half of the same response. `instance_type_esx_versions()`'s doc refuses to narrow a type's build list precisely because `CreateEnvironmentHost` applies no such restriction, while `provided_vcf_versions()` publishes exactly such an unapplied restriction per version.

So a caller that uses `GetVersions` as the discovery call it is documented to be (the crate calls it "the discovery call that tells a caller what it may pass to `CreateEnvironment`", state.rs:2908-2910) is told `VCF-5.2.1` runs on two instance types, and then finds the service happily building a `VCF-5.2.1` host on the third.

## Evidence

The catalogue states the per-version restriction — `services/service-com-amazonaws-evs-amazonelasticvmwareservice/src/state.rs:2892-2898`:

    /// The EC2 instance types Amazon EVS offers this version on — "EC2 instance types
    /// provided by Amazon EVS for this VCF version for creating environments"
    /// (`VcfVersionInfo$instanceTypes`), stated PER version: a version is offered a subset
    /// of the modelled `InstanceType` values, which is the whole of
    /// [`INSTANCE_TYPE_VALUES`] for a version offered on every type Amazon EVS provides
    /// and a narrower list for one it does not.
    pub instance_types: &'static [&'static str],

and `state.rs:2912-2918` makes `VCF-5.2.1` one of the narrower ones:

    ProvidedVcfVersion {
        vcf_version: "VCF-5.2.1",
        ...
        instance_types: &["i4i.metal", "i7i.metal-24xl"],
    },

while the modelled enum has three values (`smithy_query(query="shape", shape="InstanceType")`): `["i4i.metal", "i7i.metal-24xl", "i7i.metal-48xl"]`.

The handler consults the SAME catalogue entry for `esxVersion` and refuses — `src/ops/create_environment_host.rs:342-351`:

        if let Some(requested) = input.esx_version.as_deref() {
            let available = crate::state::available_esx_versions(&environment.vcf_version);
            if !available.is_empty() && !available.contains(&requested) {
                failures.push(crate::state::unavailable_esx_version_failure(
                    requested,
                    &environment.vcf_version,
                    &available,
                ));
            }
        }

— and never asks `provided_vcf_version(&environment.vcf_version).instance_types` about `input.host.instance_type`. A grep for `instance_types|provided_vcf_version` over `src/` returns only `state.rs` (the decl/doc, the two catalogue rows, `provided_vcf_version`, `available_esx_versions`, and the `GetVersions` projection at `state.rs:3011-3015`) and `src/ops/get_versions.rs:170` — no create-path reader.

The crate states the principle it breaks — `src/state.rs:3023-3028`:

    /// One entry per `InstanceType` the model declares, each carrying every build of
    /// [`OFFERED_ESX_BUILDS`] in catalogue order: "The list of ESX versions offered for this
    /// instance type" is what Amazon EVS can install on that hardware, and neither the model
    /// nor this service ties a build to an instance type — `CreateEnvironmentHost` accepts
    /// every modelled instance type on an environment of any version — so narrowing one type's
    /// list would state a restriction the service does not apply.

That sentence admits the gap ("`CreateEnvironmentHost` accepts every modelled instance type on an environment of any version") and uses it as the reason NOT to publish an unapplied restriction — yet `provided_vcf_versions()` (`state.rs:3004-3018`) publishes one.

Test coverage: `tests/all/versions_catalog.rs` only pins the POSITIVE direction. Its comment at lines 457-460 reads "'EC2 instance types provided by Amazon EVS for this VCF version for creating environments': what the catalogue reports is what the service accepts, so each reported version creates an environment and each instance type it reports adds a host to that environment", and the loop at 461-488 asserts only that every REPORTED type is accepted (`status == 200`). No test asserts anything about a type the catalogue does NOT report for a version, so nothing pins today's acceptance. `tests/all/versions_catalog.rs:317-345` pins that the reported `instanceTypes` equal `evs::PROVIDED_VCF_VERSIONS`, so the catalogue rows must stay as they are.

## Steps to Reproduce

1. Start the emulator with the EVS service mounted on the in-memory engine (the `emulator()` harness used by `tests/all/versions_catalog.rs`).
2. Call `GetVersions` and read `vcfVersions`: the `VCF-5.2.1` entry reports `instanceTypes: ["i4i.metal", "i7i.metal-24xl"]` — `i7i.metal-48xl` is absent (projected from `state.rs:2912-2918`).
3. `CreateEnvironment` with `vcfVersion: "VCF-5.2.1"` and the rest of a valid request, then let the clock settle the environment to `CREATED`.
4. Call `CreateEnvironmentHost` with `environmentId` = that environment and `host: {hostName: "esx-48xl", keyName: "evs-key", instanceType: "i7i.metal-48xl"}`.
5. Observe HTTP 200 and a persisted host: `ListEnvironmentHosts` reports `instanceType: "i7i.metal-48xl"` for a `VCF-5.2.1` environment, an instance type the same service's `GetVersions` says is not provided for that VCF version.
6. For contrast, repeat step 4 with `esxVersion` set to a build off the `VCF-5.2.1` release line: that request IS refused with the aggregated `ValidationException` naming `esxVersion`, by `create_environment_host.rs:342-351` reading the same catalogue entry.
7. Same divergence through `CreateEnvironment` itself: pass `hosts` entries with `instanceType: "i7i.metal-48xl"` alongside `vcfVersion: "VCF-5.2.1"` — the entries are materialised as hosts rather than refused.
