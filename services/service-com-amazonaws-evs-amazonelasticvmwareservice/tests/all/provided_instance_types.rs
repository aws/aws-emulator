// SPDX-License-Identifier: CC0-1.0

//! Which EC2 hardware Amazon EVS PROVIDES, over awsJson1_0: the catalogue reports the
//! instance types it offers environments on, and no other.
//!
//! The rule under test: an `instanceTypeEsxVersions` entry is reported for exactly the EC2
//! instance types Amazon EVS provides — the types its provided VCF versions list as
//! `VcfVersionInfo$instanceTypes` — and not for every value the `InstanceType` enum can
//! spell; a modelled type the catalogue provides on no version is reported by neither half
//! of the response and is refused by the create paths at every provided version. The enum
//! is what a request may SAY; the catalogue is what the service OFFERS, and `GetVersions` is
//! the discovery call for the second.
//!
//! Where each clause comes from (`smithy_query(query="operation", operation="GetVersions")`,
//! `smithy_query(query="shape",
//! shape="VcfVersionInfo,InstanceTypeEsxVersionsInfo,InstanceType")`):
//!
//! * "Information about ESX versions offered for each EC2 instance type." /  "The list of
//!   ESX versions offered for this instance type." (`InstanceTypeEsxVersionsInfo`.)
//! * "EC2 instance types provided by Amazon EVS for this VCF version for creating
//!   environments." (`VcfVersionInfo$instanceTypes`, `@required`.)
//! * "Returns information about VCF versions, ESX versions and EC2 instance types provided
//!   by Amazon EVS." (`GetVersions`.)
//!
//! The rows differ along what the rule generalises over: every modelled `InstanceType`,
//! each classified by the WIRE catalogue rather than by a value written here, so a catalogue
//! that starts or stops providing a type moves the verdicts with it; and, for the refusal
//! half, along the VCF version the environment runs.

use std::collections::BTreeSet;

use serde_json::json;

use service_com_amazonaws_evs_amazonelasticvmwareservice::ops::types as evs_types;

use crate::environment_lifecycle::{
    call, emulator, error_type, settle, vlans_with_prefix, Emulator, HOST_KEY_PAIR_NAME,
};

/// Every `InstanceType` value the model declares, written out here rather than read from
/// the service, so this suite cannot drift along with the catalogue it checks.
///
/// Source: `smithy_query(query="shape", shape="InstanceType")` — `i4i.metal`,
/// `i7i.metal-24xl`, `i7i.metal-48xl`, `r7i.metal-24xl`, `r7i.metal-48xl`.
/// [`the_rows_are_the_modelled_instance_types`] holds this list to the enum values this
/// crate generates FROM that model.
const INSTANCE_TYPES: [&str; 5] = [
    "i4i.metal",
    "i7i.metal-24xl",
    "i7i.metal-48xl",
    "r7i.metal-24xl",
    "r7i.metal-48xl",
];

/// A `hostName` holds letters, digits and hyphens only (`smithy_query(query="shape",
/// shape="HostName")` reports the pattern `^([a-zA-Z0-9\-]*)$`), so a value worked into a
/// host name has its separators replaced.
fn slug(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character
            } else {
                '-'
            }
        })
        .collect()
}

/// The catalogue as the WIRE states it: the VCF versions with the instance types each
/// provides, and the instance types the response reports ESX versions for.
async fn catalogue(router: &Emulator) -> (Vec<(String, Vec<String>)>, Vec<String>) {
    let (status, body) = call(router, "GetVersions", &json!({})).await;
    assert_eq!(
        status, 200,
        "case \"the published catalogue\": expected 200 from GetVersions, got {status} ({}) \
         {body}",
        error_type(&body)
    );
    let versions = body["vcfVersions"]
        .as_array()
        .unwrap_or_else(|| panic!("case \"the published catalogue\": no vcfVersions in {body}"))
        .iter()
        .map(|entry| {
            let vcf_version = entry["vcfVersion"]
                .as_str()
                .unwrap_or_else(|| panic!("vcfVersion is required, got {entry}"))
                .to_string();
            let instance_types = entry["instanceTypes"]
                .as_array()
                .unwrap_or_else(|| panic!("instanceTypes is required, got {entry}"))
                .iter()
                .map(|value| {
                    value
                        .as_str()
                        .unwrap_or_else(|| panic!("instanceTypes holds a non-string: {entry}"))
                        .to_string()
                })
                .collect();
            (vcf_version, instance_types)
        })
        .collect();
    let reported = body["instanceTypeEsxVersions"]
        .as_array()
        .unwrap_or_else(|| {
            panic!("case \"the published catalogue\": no instanceTypeEsxVersions in {body}")
        })
        .iter()
        .map(|entry| {
            let instance_type = entry["instanceType"]
                .as_str()
                .unwrap_or_else(|| panic!("instanceType is required, got {entry}"))
                .to_string();
            assert!(
                entry["esxVersions"]
                    .as_array()
                    .is_some_and(|versions| !versions.is_empty()),
                "case {instance_type:?}: a provided instance type is offered at least one ESX \
                 version, got {entry}"
            );
            instance_type
        })
        .collect();
    (versions, reported)
}

#[test]
fn the_rows_are_the_modelled_instance_types() {
    let modelled: BTreeSet<&str> = evs_types::INSTANCE_TYPE_VALUES.iter().copied().collect();
    let under_test: BTreeSet<&str> = INSTANCE_TYPES.iter().copied().collect();
    assert_eq!(
        under_test, modelled,
        "case \"the modelled InstanceType values\": expected {modelled:?}, got {under_test:?}"
    );
}

#[tokio::test]
async fn esx_versions_are_reported_for_exactly_the_types_the_catalogue_provides() {
    let router = emulator();
    let (versions, reported) = catalogue(&router).await;

    // What the catalogue PROVIDES, read off the response's own per-version lists.
    let provided: BTreeSet<&str> = versions
        .iter()
        .flat_map(|(_, instance_types)| instance_types.iter().map(String::as_str))
        .collect();
    assert!(
        !provided.is_empty(),
        "setup: Amazon EVS provides at least one EC2 instance type, got {versions:?}"
    );
    let reported_set: BTreeSet<&str> = reported.iter().map(String::as_str).collect();
    assert_eq!(
        reported_set.len(),
        reported.len(),
        "no instance type may be reported twice, got {reported:?}"
    );

    // One row per modelled instance type: a type some version provides is reported with its
    // ESX versions, and a type no version provides is reported by neither half.
    for instance_type in INSTANCE_TYPES {
        let is_provided = provided.contains(instance_type);
        assert_eq!(
            reported_set.contains(instance_type),
            is_provided,
            "case {instance_type:?}: a type the catalogue provides must be reported with its \
             ESX versions and a type it does not must not be reported at all (provided: \
             {is_provided}), got {reported:?}"
        );
    }
}

#[tokio::test]
async fn a_modelled_type_the_catalogue_provides_on_no_version_is_refused_by_every_version() {
    let router = emulator();
    let (versions, _) = catalogue(&router).await;

    let provided: BTreeSet<&str> = versions
        .iter()
        .flat_map(|(_, instance_types)| instance_types.iter().map(String::as_str))
        .collect();
    // Nothing to refuse where the catalogue provides every modelled type, which is a
    // catalogue this half of the rule says nothing about.
    let unprovided: Vec<&str> = INSTANCE_TYPES
        .iter()
        .copied()
        .filter(|instance_type| !provided.contains(instance_type))
        .collect();
    if unprovided.is_empty() {
        return;
    }

    // Rows: every version the catalogue provides, each asked for every modelled type it
    // provides on no version. The request is well formed in every other respect, so the
    // hardware is the only thing that can refuse it.
    for (index, (vcf_version, _)) in versions.iter().enumerate() {
        let (vpc_id, subnet_id) = router.fresh_network();
        let request = json!({
            "vpcId": vpc_id,
            "serviceAccessSubnetId": subnet_id,
            "vcfVersion": vcf_version,
            "termsAccepted": true,
            "environmentName": format!("unprovided-{index}"),
            "initialVlans": vlans_with_prefix(24),
        });
        let (status, body) = call(&router, "CreateEnvironment", &request).await;
        assert_eq!(
            status, 200,
            "case {vcf_version:?}: a provided VCF version must be accepted, got {status} ({}) \
             {body}",
            error_type(&body)
        );
        let environment = body["environment"]["environmentId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {vcf_version:?}: no environmentId in {body}"))
            .to_string();
        settle(&router);

        for instance_type in &unprovided {
            let case = format!("{vcf_version} on {instance_type}");
            let request = json!({
                "environmentId": environment,
                "host": {
                    "hostName": format!("esx-{}-{index}", slug(instance_type)),
                    "keyName": HOST_KEY_PAIR_NAME,
                    "instanceType": instance_type,
                },
            });
            let (status, answer) = call(&router, "CreateEnvironmentHost", &request).await;
            assert_eq!(
                status, 400,
                "case {case:?}: hardware the catalogue provides on no version must be refused, \
                 got {status} ({}) {answer}",
                error_type(&answer)
            );
            assert_eq!(
                error_type(&answer),
                "ValidationException",
                "case {case:?}: expected a ValidationException, got {answer}"
            );
        }

        // A refused request stores nothing: the environment holds no host at all.
        let (status, answer) = call(
            &router,
            "ListEnvironmentHosts",
            &json!({ "environmentId": environment }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {vcf_version:?}: expected 200 from ListEnvironmentHosts, got {status} {answer}"
        );
        let hosts = answer["environmentHosts"]
            .as_array()
            .map(Vec::len)
            .unwrap_or_default();
        assert_eq!(
            hosts, 0,
            "case {vcf_version:?}: a refused request must store no host, got {answer}"
        );
    }
}
