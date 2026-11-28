// SPDX-License-Identifier: CC0-1.0

//! A host is built only on EC2 hardware Amazon EVS provides for its environment's VCF
//! version, over awsJson1_0.
//!
//! The rule under test: the `instanceType` a request asks a host for is one of the EC2
//! instance types this service's own version catalogue provides for that environment's VCF
//! version — the per-version list `GetVersions` reports as
//! `VcfVersionInfo$instanceTypes` — and a request asking for a type outside that list is
//! refused with the aggregated `ValidationException` naming the member that asked for it,
//! storing nothing. The restriction belongs to the catalogue rather than to one operation,
//! so both operations that take a `HostInfoForCreate` apply it: `CreateEnvironmentHost` for
//! its single `host`, and `CreateEnvironment` for each entry of `hosts`.
//!
//! Where each clause comes from (`smithy_query(query="shape",
//! shape="VcfVersionInfo,HostInfoForCreate,InstanceType")` and
//! `smithy_query(query="operation", operation="CreateEnvironmentHost,CreateEnvironment")`):
//!
//! * "EC2 instance types provided by Amazon EVS for this VCF version for creating
//!   environments." (`VcfVersionInfo$instanceTypes`, `@required`.)
//! * "The EC2 instance type that represents the host." (`HostInfoForCreate$instanceType`,
//!   `@required`, an `InstanceType` enum.)
//! * "Returns information about VCF versions, ESX versions and EC2 instance types provided
//!   by Amazon EVS. For each VCF version, the response also includes the default ESX
//!   version and provided EC2 instance types." (`GetVersions`) — the discovery call, so
//!   what it reports for a version is what a create of that version takes.
//! * `ValidationException` (HTTP 400) is the only client-input error either create models,
//!   and it carries a `fieldList`, so a refused member is named there.
//!
//! The expectations are read from the WIRE catalogue rather than written here: every row's
//! verdict is whether the `GetVersions` response lists that instance type for that VCF
//! version, so the published catalogue and the applied one cannot drift apart. The rows
//! differ along what the rule generalises over: which VCF version the environment runs,
//! which modelled instance type is asked for, whether that type is on the version's list,
//! which operation asks for it, how many entries of one request ask for an unprovided type,
//! and whether the request breaks other preconditions as well.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use service_com_amazonaws_evs_amazonelasticvmwareservice::ops::types as evs_types;
use service_com_amazonaws_evs_amazonelasticvmwareservice::state as evs;

use crate::environment_lifecycle::{
    call, emulator, error_type, settle, vlans_with_prefix, Emulator, HOST_KEY_PAIR_NAME,
};

/// Every `InstanceType` value the model declares, written out here rather than read from
/// the service, so this suite cannot drift along with the catalogue it checks.
///
/// Source: `smithy_query(query="shape", shape="InstanceType")` — `i4i.metal`,
/// `i7i.metal-24xl`, `i7i.metal-48xl`, `r7i.metal-24xl`, `r7i.metal-48xl`.
/// [`the_rows_cover_every_instance_type_the_model_declares`] holds this list to the enum
/// values this crate generates FROM that model.
const INSTANCE_TYPES: [&str; 5] = [
    "i4i.metal",
    "i7i.metal-24xl",
    "i7i.metal-48xl",
    "r7i.metal-24xl",
    "r7i.metal-48xl",
];

/// A VCF version Amazon EVS INSTALLS and provides `instance_type` on, read from this
/// service's own version catalogue.
///
/// A host is built only on hardware its environment's VCF version is provided on, so a
/// suite that wants a host of a given instance type SERVED creates its environment at a
/// version the catalogue provides that type on — exactly as a suite that wants a create
/// served registers the VPC it names. Read from the catalogue rather than written out, so a
/// catalogue that stops providing a type on one version moves every such suite with it.
pub(crate) fn vcf_version_providing(instance_type: &str) -> &'static str {
    evs::PROVIDED_VCF_VERSIONS
        .iter()
        .find(|provided| {
            evs::installs_vcf(provided.vcf_version)
                && provided.instance_types.contains(&instance_type)
        })
        .map(|provided| provided.vcf_version)
        .unwrap_or_else(|| {
            panic!("no VCF version Amazon EVS installs is provided on {instance_type:?}")
        })
}

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

/// The EC2 instance types `GetVersions` reports for each VCF version it provides, in the
/// order it reported them: the catalogue every row below takes its verdict from.
async fn published_catalogue(router: &Emulator) -> Vec<(String, Vec<String>)> {
    let (status, body) = call(router, "GetVersions", &json!({})).await;
    assert_eq!(
        status, 200,
        "case \"the published catalogue\": expected 200 from GetVersions, got {status} ({}) \
         {body}",
        error_type(&body)
    );
    let versions = body["vcfVersions"].as_array().unwrap_or_else(|| {
        panic!("case \"the published catalogue\": GetVersions must report vcfVersions, got {body}")
    });
    let catalogue: Vec<(String, Vec<String>)> = versions
        .iter()
        .map(|entry| {
            let vcf_version = entry["vcfVersion"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case \"the published catalogue\": vcfVersion is required, got {entry}")
                })
                .to_string();
            let instance_types = entry["instanceTypes"]
                .as_array()
                .unwrap_or_else(|| {
                    panic!(
                        "case \"the published catalogue\": instanceTypes is required, got {entry}"
                    )
                })
                .iter()
                .map(|value| {
                    value
                        .as_str()
                        .unwrap_or_else(|| {
                            panic!(
                                "case \"the published catalogue\": instanceTypes holds a \
                                 non-string, got {entry}"
                            )
                        })
                        .to_string()
                })
                .collect();
            (vcf_version, instance_types)
        })
        .collect();
    assert!(
        !catalogue.is_empty(),
        "case \"the published catalogue\": Amazon EVS provides at least one VCF version, got \
         {body}"
    );
    catalogue
}

/// Create an environment at `vcf_version` and settle the deployment the clock owes it, so a
/// host can be added to it. The VLAN blocks are `/24`, so one `vmkManagement` subnet holds
/// every host a row adds.
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
        "initialVlans": vlans_with_prefix(24),
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
    // A host can only be added after the environment is deployed, which is the move to
    // CREATED the clock owes it. Applied from an instant past its due one rather than waited
    // for, so these rows cost no wall-clock time.
    settle(router);
    environment
}

/// The `fieldList` members a refusal named, in the order it named them.
fn named_members(body: &Value) -> Vec<String> {
    body["fieldList"]
        .as_array()
        .map(|entries| {
            entries
                .iter()
                .filter_map(|entry| entry["name"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// The `fieldList` sentence reported for one member, or `None` when it named no such entry.
fn message_for(body: &Value, member: &str) -> Option<String> {
    body["fieldList"].as_array().and_then(|entries| {
        entries
            .iter()
            .find(|entry| entry["name"].as_str() == Some(member))
            .and_then(|entry| entry["message"].as_str())
            .map(str::to_string)
    })
}

/// The instance types the hosts of an environment report, as a set.
async fn listed_instance_types(router: &Emulator, case: &str, environment: &str) -> BTreeSet<String> {
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
        .iter()
        .filter_map(|host| host["instanceType"].as_str().map(str::to_string))
        .collect()
}

/// `CreateEnvironment$hosts` is modelled `@length(min: 4, max: 4)`
/// (`smithy_query(query="shape", shape="HostInfoForCreateList")`), so a create carries
/// exactly four entries.
const HOSTS_LENGTH: usize = 4;

/// The instance types the `hosts` entries of each create ask for, one entry list per
/// create: the modelled instance types spread over as many creates as it takes at
/// `HOSTS_LENGTH` entries each, with any entry past the last modelled type repeating a type
/// the version IS provided on — so a refusal names exactly the entries that asked for
/// hardware it is not provided on, and the creates together ask for every modelled type.
fn entry_instance_types(provided: &[String]) -> Vec<Vec<&'static str>> {
    let filler = INSTANCE_TYPES
        .iter()
        .copied()
        .find(|instance_type| provided.iter().any(|listed| listed == instance_type))
        .unwrap_or(INSTANCE_TYPES[0]);
    INSTANCE_TYPES
        .chunks(HOSTS_LENGTH)
        .map(|chunk| {
            let mut asked_for: Vec<&'static str> = chunk.to_vec();
            while asked_for.len() < HOSTS_LENGTH {
                asked_for.push(filler);
            }
            asked_for
        })
        .collect()
}

#[test]
fn the_rows_cover_every_instance_type_the_model_declares() {
    let modelled: BTreeSet<&str> = evs_types::INSTANCE_TYPE_VALUES.iter().copied().collect();
    let under_test: BTreeSet<&str> = INSTANCE_TYPES.iter().copied().collect();
    assert_eq!(
        under_test, modelled,
        "case \"the modelled InstanceType values\": expected {modelled:?}, got {under_test:?}"
    );
    // And the creates the `hosts` rows below make ask for all of them between them: a
    // create carries exactly `HOSTS_LENGTH` hosts, so the types are spread over as many
    // creates as that takes and none of them is left unasked for.
    let every_type: Vec<String> = INSTANCE_TYPES
        .iter()
        .map(|instance_type| (*instance_type).to_string())
        .collect();
    let creates = entry_instance_types(&every_type);
    for (ordinal, asked_for) in creates.iter().enumerate() {
        assert_eq!(
            asked_for.len(),
            HOSTS_LENGTH,
            "case \"the modelled InstanceType values\": create {ordinal} must carry \
             {HOSTS_LENGTH} hosts, got {asked_for:?}"
        );
    }
    let asked_for: BTreeSet<&str> = creates.iter().flatten().copied().collect();
    assert_eq!(
        asked_for, modelled,
        "case \"the modelled InstanceType values\": the creates must ask for every modelled \
         instance type, expected {modelled:?}, got {asked_for:?}"
    );
}

#[tokio::test]
async fn create_environment_host_builds_a_host_only_on_a_type_the_version_is_provided_on() {
    let router = emulator();
    let catalogue = published_catalogue(&router).await;

    // One environment per provided VCF version, and in each: one request per modelled
    // instance type. The verdict of a row is the catalogue's own: a type the response lists
    // for that version is built, and a type it leaves out is refused.
    for (index, (vcf_version, provided)) in catalogue.iter().enumerate() {
        let environment = deployed_environment(
            &router,
            vcf_version,
            vcf_version,
            &format!("instance-types-{index}"),
        )
        .await;
        for (ordinal, instance_type) in INSTANCE_TYPES.iter().enumerate() {
            let case = format!("{vcf_version} on {instance_type}");
            let host_name = format!("esx-{}-{index}-{ordinal}", slug(instance_type));
            let request = json!({
                "environmentId": environment,
                "host": {
                    "hostName": host_name,
                    "keyName": HOST_KEY_PAIR_NAME,
                    "instanceType": instance_type,
                },
            });
            let (status, answer) = call(&router, "CreateEnvironmentHost", &request).await;
            if provided.iter().any(|listed| listed == instance_type) {
                assert_eq!(
                    status, 200,
                    "case {case:?}: an instance type the catalogue provides for this VCF \
                     version must be accepted, got {status} ({}) {answer}",
                    error_type(&answer)
                );
                let reported = answer["host"]["instanceType"].as_str().unwrap_or("<none>");
                assert_eq!(
                    reported, *instance_type,
                    "case {case:?}: the host must report the instance type it was built on, \
                     got {reported:?}"
                );
                continue;
            }
            assert_eq!(
                status, 400,
                "case {case:?}: an instance type the catalogue does not provide for this VCF \
                 version must be refused, got {status} {answer}"
            );
            assert_eq!(
                error_type(&answer),
                "ValidationException",
                "case {case:?}: expected a ValidationException, got {answer}"
            );
            let members = named_members(&answer);
            assert_eq!(
                members,
                vec!["host.instanceType".to_string()],
                "case {case:?}: the refusal must name the member that asked for the \
                 instance type, got {answer}"
            );
            let message = message_for(&answer, "host.instanceType").unwrap_or_default();
            for expected in std::iter::once(instance_type.to_string())
                .chain(std::iter::once(vcf_version.clone()))
                .chain(provided.iter().cloned())
            {
                assert!(
                    message.contains(&expected),
                    "case {case:?}: the sentence must name {expected:?} — the type asked \
                     for, the VCF version it was asked for and the types that are provided \
                     for it — got {message:?}"
                );
            }
        }

        // A refused request stores nothing, so the environment holds exactly one host per
        // instance type its own VCF version is provided on.
        let listed = listed_instance_types(&router, vcf_version, &environment).await;
        let expected: BTreeSet<String> = provided.iter().cloned().collect();
        assert_eq!(
            listed, expected,
            "case {vcf_version:?}: the environment must hold a host of every provided \
             instance type and of no other, expected {expected:?}, got {listed:?}"
        );
    }
}

#[tokio::test]
async fn create_environment_refuses_every_hosts_entry_on_a_type_its_version_is_not_provided_on() {
    let router = emulator();
    let catalogue = published_catalogue(&router).await;

    // Per provided VCF version, the creates that ask for every modelled instance type
    // between them — a create carries exactly `HOSTS_LENGTH` hosts, so one per chunk of
    // that many types: every entry asking for a type the version is not provided on draws
    // its own `fieldList` entry, named by the index of the entry that asked for it, and a
    // create whose entries all ask for provided types adds them all.
    for (index, (vcf_version, provided)) in catalogue.iter().enumerate() {
        for (chunk, asked_for) in entry_instance_types(provided).into_iter().enumerate() {
            let case = format!("a create of {vcf_version} carrying the hosts {asked_for:?}");
            let hosts: Vec<Value> = asked_for
                .iter()
                .enumerate()
                .map(|(ordinal, instance_type)| {
                    json!({
                        "hostName": format!(
                            "req-{}-{index}-{chunk}-{ordinal}",
                            slug(instance_type),
                        ),
                        "keyName": HOST_KEY_PAIR_NAME,
                        "instanceType": instance_type,
                    })
                })
                .collect();
            let expected_members: Vec<String> = if evs::installs_vcf(vcf_version) {
                asked_for
                    .iter()
                    .enumerate()
                    .filter(|(_, instance_type)| {
                        !provided.iter().any(|listed| listed == *instance_type)
                    })
                    .map(|(ordinal, _)| format!("hosts[{ordinal}].instanceType"))
                    .collect()
            } else {
                // "no hosts are added and no VCF installation is performed" for the
                // self-deployed mode: a mode that builds no host names no hardware, so an
                // inert entry's instance type is not held to the catalogue (see
                // `CreateEnvironment`).
                Vec::new()
            };
            let (vpc_id, subnet_id) = router.fresh_network();
            let environment_name = format!("entry-types-{index}-{chunk}");
            let request = json!({
                "vpcId": vpc_id,
                "serviceAccessSubnetId": subnet_id,
                "vcfVersion": vcf_version,
                "termsAccepted": true,
                "environmentName": environment_name,
                "initialVlans": vlans_with_prefix(24),
                "hosts": hosts,
            });
            let (status, body) = call(&router, "CreateEnvironment", &request).await;

            if expected_members.is_empty() {
                assert_eq!(
                    status, 200,
                    "case {case:?}: every entry asks for a provided instance type, got \
                     {status} ({}) {body}",
                    error_type(&body)
                );
                let environment = body["environment"]["environmentId"]
                    .as_str()
                    .unwrap_or_else(|| panic!("case {case:?}: no environmentId in {body}"))
                    .to_string();
                settle(&router);
                let listed = listed_instance_types(&router, &case, &environment).await;
                let expected: BTreeSet<String> =
                    asked_for.iter().map(|value| (*value).to_string()).collect();
                // "no hosts are added and no VCF installation is performed" for the
                // self-deployed mode, so an environment that installs none holds none.
                let expected = if evs::installs_vcf(vcf_version) {
                    expected
                } else {
                    BTreeSet::new()
                };
                assert_eq!(
                    listed, expected,
                    "case {case:?}: expected the hosts {expected:?}, got {listed:?}"
                );
                continue;
            }

            assert_eq!(
                status, 400,
                "case {case:?}: an entry asking for an instance type the catalogue does not \
                 provide for this VCF version must refuse the create, got {status} {body}"
            );
            assert_eq!(
                error_type(&body),
                "ValidationException",
                "case {case:?}: expected a ValidationException, got {body}"
            );
            assert_eq!(
                named_members(&body),
                expected_members,
                "case {case:?}: the refusal must name one entry per host that asked for an \
                 unprovided instance type, got {body}"
            );
            // A refused create mints nothing, so no environment of that name exists.
            let (status, listed) = call(&router, "ListEnvironments", &json!({})).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected 200 from ListEnvironments, got {status} ({}) \
                 {listed}",
                error_type(&listed)
            );
            let names: Vec<&str> = listed["environmentSummaries"]
                .as_array()
                .unwrap_or_else(|| panic!("case {case:?}: no environmentSummaries in {listed}"))
                .iter()
                .filter_map(|summary| summary["environmentName"].as_str())
                .collect();
            assert!(
                !names.contains(&environment_name.as_str()),
                "case {case:?}: a refused create must mint no environment, got {names:?}"
            );
        }
    }
}

#[tokio::test]
async fn an_unprovided_instance_type_is_reported_beside_the_other_failures_of_one_request() {
    let router = emulator();
    let catalogue = published_catalogue(&router).await;

    // A version the catalogue provides on a SUBSET of the modelled instance types, and one
    // type it leaves out. Nothing to aggregate where every version is provided on every
    // type, which is a catalogue this rule refuses nothing for.
    let restricted = catalogue.iter().find_map(|(vcf_version, provided)| {
        INSTANCE_TYPES
            .iter()
            .find(|instance_type| !provided.iter().any(|listed| listed == **instance_type))
            .map(|instance_type| (vcf_version.as_str(), *instance_type))
    });
    let Some((vcf_version, instance_type)) = restricted else {
        return;
    };

    let case = format!("{vcf_version} on {instance_type}, with two more broken preconditions");
    let environment =
        deployed_environment(&router, &case, vcf_version, "aggregated-instance-type").await;
    // Three preconditions at once: the hardware the version is not provided on, an ESX
    // version Amazon EVS offers for no version at all, and an SSH key pair the account does
    // not hold. A request that breaks several is refused ONCE, with a `fieldList` entry per
    // failure.
    let request = json!({
        "environmentId": environment,
        "esxVersion": "7.0.3",
        "host": {
            "hostName": "esx-aggregated",
            "keyName": "no-such-key",
            "instanceType": instance_type,
        },
    });
    let (status, body) = call(&router, "CreateEnvironmentHost", &request).await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected 400, got {status} ({}) {body}",
        error_type(&body)
    );
    let members = named_members(&body);
    for expected in ["host.instanceType", "esxVersion", "host.keyName"] {
        assert!(
            members.iter().any(|member| member == expected),
            "case {case:?}: the aggregated refusal must name {expected:?}, got {members:?} \
             in {body}"
        );
    }
    // And the top-level message joins the sentences of the entries it carries.
    let message = body["message"].as_str().unwrap_or_default();
    let sentence = message_for(&body, "host.instanceType").unwrap_or_default();
    assert!(
        message.contains(&sentence) && !sentence.is_empty(),
        "case {case:?}: the top-level message must carry the instance type sentence \
         {sentence:?}, got {message:?}"
    );
    let listed = listed_instance_types(&router, &case, &environment).await;
    assert!(
        listed.is_empty(),
        "case {case:?}: a refused request must store no host, got {listed:?}"
    );
}
