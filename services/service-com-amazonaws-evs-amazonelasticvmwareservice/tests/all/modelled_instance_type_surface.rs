// SPDX-License-Identifier: CC0-1.0

//! The hardware surface of Amazon EVS is the MODEL's `InstanceType` value set, end to end
//! over awsJson1_0.
//!
//! The rule under test: every EC2 instance type the model declares is hardware this
//! service PUBLISHES and SERVES, and no string outside that set is either — `GetVersions`
//! reports one `instanceTypeEsxVersions` entry per declared value and for nothing else,
//! each of those types is provided for at least one VCF version the same response reports,
//! a host asked for on it at such a version is created and read back reporting it (running
//! on a bare metal instance of exactly that type in the account's own Amazon EC2 store),
//! and a string outside the declared set is refused with a `ValidationException` naming
//! the member and the value set the model declares, storing nothing.
//!
//! Where each clause comes from (`smithy_query(query="shape",
//! shape="InstanceType,VcfVersionInfo,InstanceTypeEsxVersionsInfo,HostInfoForCreate")` and
//! `smithy_query(query="operation", operation="GetVersions,CreateEnvironmentHost")`):
//!
//! * "Returns information about VCF versions, ESX versions and EC2 instance types provided
//!   by Amazon EVS. For each VCF version, the response also includes the default ESX
//!   version and provided EC2 instance types." (`GetVersions`) — the discovery call, so
//!   what it publishes is what a create takes.
//! * "Information about ESX versions offered for each EC2 instance type." / "The list of
//!   ESX versions offered for this instance type." (`InstanceTypeEsxVersionsInfo`, both
//!   members `@required`.)
//! * "EC2 instance types provided by Amazon EVS for this VCF version for creating
//!   environments." (`VcfVersionInfo$instanceTypes`, `@required`) — hardware offered for
//!   creating environments, so hardware some provided version can be created on.
//! * "The EC2 instance type that represents the host." (`HostInfoForCreate$instanceType`,
//!   `@required`, an `InstanceType` enum) — so a string outside the enum fails the
//!   modelled constraint; `tests/all/validation_message_rendering.rs` owns the WORDING of
//!   such a refusal, and `tests/all/host_instance_type_catalogue.rs` the per-version
//!   restriction inside the set.
//! * "An ESX host that runs on an Amazon EC2 bare metal instance" (`Host` doc) — the host
//!   IS that instance, so the instance the account's own Amazon EC2 store holds for it runs
//!   the type that was asked for.
//!
//! The positive rows are the declared values themselves, read from the enum this crate
//! GENERATES from the model rather than written out here, so a model that gains or loses an
//! instance type moves this suite with it instead of leaving it behind: each row differs in
//! which instance type is asked for and, with it, which VCF version the environment runs.
//! The negative rows differ along the FORM a string outside the set takes: hardware Amazon
//! EC2 offers but Amazon EVS does not, another case of a declared value, and a near-miss
//! spelling of one.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};

use service_com_amazonaws_ec2_amazonec2 as ec2;
use service_com_amazonaws_evs_amazonelasticvmwareservice::ops::types as evs_types;

use crate::environment_lifecycle::{
    call, emulator, error_type, settle, vlans_with_prefix, Emulator, HOST_KEY_PAIR_NAME,
};

/// Strings that name no declared `InstanceType`, one per form such a string takes: an EC2
/// instance type Amazon EVS does not provide, the upper-case spelling of a declared value,
/// and a size of a declared family that the model does not declare.
const UNDECLARED_INSTANCE_TYPES: [&str; 3] = ["m5.large", "I4I.METAL", "i4i.metal-24xl"];

/// A `hostName` holds letters, digits and hyphens only (`smithy_query(query="shape",
/// shape="HostName")` reports the pattern `^([a-zA-Z0-9\-]*)$`), so an instance type worked
/// into a host name has its separators replaced.
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

/// The `GetVersions` body, which must be a 200.
async fn get_versions(router: &Emulator) -> Value {
    let (status, body) = call(router, "GetVersions", &json!({})).await;
    assert_eq!(
        status, 200,
        "case \"the published catalogue\": expected 200 from GetVersions, got {status} ({}) \
         {body}",
        error_type(&body)
    );
    body
}

/// The instance types `instanceTypeEsxVersions` reports, in the order it reported them.
fn published_instance_types(body: &Value) -> Vec<String> {
    body["instanceTypeEsxVersions"]
        .as_array()
        .unwrap_or_else(|| {
            panic!(
                "case \"the published catalogue\": GetVersions must report \
                 instanceTypeEsxVersions, got {body}"
            )
        })
        .iter()
        .map(|entry| {
            entry["instanceType"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!(
                        "case \"the published catalogue\": instanceType is required, got \
                         {entry}"
                    )
                })
                .to_string()
        })
        .collect()
}

/// `instance type -> the VCF versions the catalogue provides it for`, in catalogue order.
fn providing_versions(body: &Value) -> BTreeMap<String, Vec<String>> {
    let mut providing: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for entry in body["vcfVersions"].as_array().unwrap_or_else(|| {
        panic!("case \"the published catalogue\": GetVersions must report vcfVersions, got {body}")
    }) {
        let vcf_version = entry["vcfVersion"]
            .as_str()
            .unwrap_or_else(|| {
                panic!("case \"the published catalogue\": vcfVersion is required, got {entry}")
            })
            .to_string();
        for instance_type in entry["instanceTypes"].as_array().unwrap_or_else(|| {
            panic!("case \"the published catalogue\": instanceTypes is required, got {entry}")
        }) {
            let instance_type = instance_type
                .as_str()
                .unwrap_or_else(|| {
                    panic!(
                        "case \"the published catalogue\": instanceTypes holds a non-string, \
                         got {entry}"
                    )
                })
                .to_string();
            providing
                .entry(instance_type)
                .or_default()
                .push(vcf_version.clone());
        }
    }
    providing
}

/// Create an environment at `vcf_version` and settle the deployment the clock owes it, so a
/// host can be added to it.
async fn deployed_environment(
    router: &Emulator,
    case: &str,
    vcf_version: &str,
    name: &str,
) -> String {
    let (vpc_id, subnet_id) = router.fresh_network();
    let request = json!({
        "vpcId": vpc_id,
        "serviceAccessSubnetId": subnet_id,
        "vcfVersion": vcf_version,
        "termsAccepted": true,
        "environmentName": name,
        "initialVlans": vlans_with_prefix(28),
    });
    let (status, body) = call(router, "CreateEnvironment", &request).await;
    assert_eq!(
        status, 200,
        "case {case:?}: a VCF version the catalogue reports must be accepted by \
         CreateEnvironment, got {status} ({}) {body}",
        error_type(&body)
    );
    let environment = body["environment"]["environmentId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no environmentId in {body}"))
        .to_string();
    // A host can only be added to a deployed environment, which is the move to CREATED the
    // clock owes it. Applied from an instant past its due one rather than waited for, so
    // these rows cost no wall-clock time.
    settle(router);
    environment
}

/// The hosts `ListEnvironmentHosts` reports for one environment.
async fn listed_hosts(router: &Emulator, case: &str, environment: &str) -> Vec<Value> {
    let (status, body) = call(
        router,
        "ListEnvironmentHosts",
        &json!({ "environmentId": environment }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 from ListEnvironmentHosts, got {status} ({}) {body}",
        error_type(&body)
    );
    body["environmentHosts"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: no environmentHosts in {body}"))
        .clone()
}

/// The instance type the account's own Amazon EC2 store holds for the instance `instance_id`,
/// or `None` when it holds no such instance at all.
fn held_instance_type(router: &Emulator, instance_id: &str) -> Option<String> {
    let state = ec2::state::service_state(&*router.engine)
        .expect("the in-memory engine carries the account's EC2 store");
    let store = state.lock();
    store
        .instances
        .get(instance_id)
        .map(|instance| instance.instance_type.clone())
}

#[tokio::test]
async fn the_catalogue_publishes_the_modelled_instance_types_and_nothing_else() {
    let router = emulator();
    let body = get_versions(&router).await;
    let published = published_instance_types(&body);

    // The set is the `InstanceType` enum this crate generates FROM the model: the discovery
    // call describes every type a create may ask for, and nothing a request could not name.
    let modelled: BTreeSet<&str> = evs_types::INSTANCE_TYPE_VALUES.iter().copied().collect();
    let reported: BTreeSet<&str> = published.iter().map(String::as_str).collect();
    assert_eq!(
        reported, modelled,
        "case \"the modelled InstanceType values\": expected {modelled:?}, got {reported:?}"
    );
    assert_eq!(
        published.len(),
        reported.len(),
        "case \"the modelled InstanceType values\": no instance type may be reported twice, \
         got {published:?}"
    );

    // And every published type is hardware an environment can actually be created on: the
    // two halves of one response agree, so the builds offered for a type are not offered on
    // hardware no provided VCF version runs.
    let providing = providing_versions(&body);
    for instance_type in &published {
        let versions = providing.get(instance_type).map(Vec::as_slice).unwrap_or(&[]);
        assert!(
            !versions.is_empty(),
            "case {instance_type:?}: an instance type the catalogue offers builds for must be \
             provided for some VCF version, got {providing:?}"
        );
    }
}

#[tokio::test]
async fn a_host_is_created_on_every_modelled_instance_type_and_read_back_on_it() {
    let router = emulator();
    let body = get_versions(&router).await;
    let providing = providing_versions(&body);

    // One row per modelled instance type, each at a VCF version the catalogue provides that
    // type for — a host is built only on hardware its environment's version is provided on.
    for (index, instance_type) in evs_types::INSTANCE_TYPE_VALUES.iter().enumerate() {
        let case = format!("a host on {instance_type}");
        let vcf_version = providing
            .get(*instance_type)
            .and_then(|versions| versions.first())
            .unwrap_or_else(|| {
                panic!(
                    "case {case:?}: every modelled instance type is provided for some VCF \
                     version, got {providing:?}"
                )
            })
            .clone();
        let environment = deployed_environment(
            &router,
            &case,
            &vcf_version,
            &format!("modelled-type-{index}"),
        )
        .await;

        let host_name = format!("esx-{}-{index}", slug(instance_type));
        let request = json!({
            "environmentId": environment,
            "host": {
                "hostName": host_name,
                "keyName": HOST_KEY_PAIR_NAME,
                "instanceType": instance_type,
            },
        });
        let (status, answer) = call(&router, "CreateEnvironmentHost", &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a modelled instance type the catalogue provides for \
             {vcf_version} must be accepted, got {status} ({}) {answer}",
            error_type(&answer)
        );
        let reported = answer["host"]["instanceType"].as_str().unwrap_or("<none>");
        assert_eq!(
            reported, *instance_type,
            "case {case:?}: the host must report the hardware it was built on, got \
             {reported:?}"
        );
        let ec2_instance_id = answer["host"]["ec2InstanceId"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            !ec2_instance_id.is_empty(),
            "case {case:?}: a created host runs on an EC2 bare metal instance, got {answer}"
        );

        // Read back through the list: the stored host reports the same hardware.
        let listed = listed_hosts(&router, &case, &environment).await;
        let read_back = listed
            .iter()
            .find(|host| host["hostName"] == json!(host_name))
            .unwrap_or_else(|| panic!("case {case:?}: the created host must be listed, got {listed:?}"));
        assert_eq!(
            read_back["instanceType"],
            json!(instance_type),
            "case {case:?}: the listed host must report the hardware it was built on, got {read_back}"
        );

        // And the instance the host IS runs that type in the account's own Amazon EC2 store.
        let held = held_instance_type(&router, &ec2_instance_id);
        assert_eq!(
            held.as_deref(),
            Some(*instance_type),
            "case {case:?}: Amazon EC2 must hold the instance {ec2_instance_id} on the type \
             the host was built on, got {held:?}"
        );
    }
}

#[tokio::test]
async fn a_string_outside_the_modelled_value_set_is_refused_and_stores_no_host() {
    let router = emulator();
    let body = get_versions(&router).await;
    let providing = providing_versions(&body);
    let (instance_type, versions) = providing
        .iter()
        .next()
        .expect("the catalogue provides at least one instance type");
    let vcf_version = versions
        .first()
        .expect("a provided instance type names the version providing it")
        .clone();
    let case = format!("an environment of {vcf_version} provided on {instance_type}");
    let environment =
        deployed_environment(&router, &case, &vcf_version, "undeclared-types").await;

    // The whole declared set is what a refusal reports, so it is read from the enum this
    // crate generates from the model rather than written out here.
    let declared = evs_types::INSTANCE_TYPE_VALUES.join(", ");
    for undeclared in UNDECLARED_INSTANCE_TYPES {
        let case = format!("the instanceType {undeclared}");
        let request = json!({
            "environmentId": environment,
            "host": {
                "hostName": format!("esx-{}", slug(undeclared)),
                "keyName": HOST_KEY_PAIR_NAME,
                "instanceType": undeclared,
            },
        });
        let (status, answer) = call(&router, "CreateEnvironmentHost", &request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a string outside the modelled value set must be refused, got \
             {status} {answer}"
        );
        assert_eq!(
            error_type(&answer),
            "ValidationException",
            "case {case:?}: expected a ValidationException, got {answer}"
        );
        let message = answer["message"].as_str().unwrap_or_default();
        assert!(
            message.contains("host.instanceType"),
            "case {case:?}: the refusal must name the member that carried it, got {message:?}"
        );
        assert!(
            message.contains(&declared),
            "case {case:?}: the refusal must state the value set the model declares \
             ({declared}), got {message:?}"
        );
    }

    // A refused request stores nothing, so the environment holds no host at all.
    let listed = listed_hosts(&router, &case, &environment).await;
    assert!(
        listed.is_empty(),
        "case {case:?}: a refused request must store no host, got {listed:?}"
    );
}
