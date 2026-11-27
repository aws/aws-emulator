// SPDX-License-Identifier: CC0-1.0

//! `GetVersions`: the catalogue of what Amazon EVS PROVIDES, end to end over awsJson1_0.
//!
//! The rule under test: `GetVersions` reports one entry per `VcfVersion` the model
//! declares, and every member of an entry is that version's OWN catalogue fact — its
//! availability status (one of the two the member documents), the default ESX version its
//! Bill Of Materials names and the EC2 instance types it is offered on, which may be a
//! subset of the modelled instance types — plus, for each of those instance types, the ESX
//! versions offered on it; it is a read, so it reports the same catalogue however often it
//! is called and whatever a caller has created, and the catalogue it reports is the one the
//! rest of the service acts on. Every ESX version it reports — each `esxVersions` entry and
//! each `defaultEsxVersion` — is an ESXi BUILD identifier, and a version's default is one
//! of the builds every instance type it lists is offered:
//! `tests/all/esx_build_catalogue.rs` owns that half of the rule.
//!
//! Where each clause comes from (`smithy_query(query="operation",
//! operation="GetVersions")`, `smithy_query(query="shape",
//! shape="VcfVersionInfo,InstanceTypeEsxVersionsInfo,VcfVersion,InstanceType,EsxVersion")`
//! and the docs they carry):
//!
//! * "Returns information about VCF versions, ESX versions and EC2 instance types
//!   provided by Amazon EVS. For each VCF version, the response also includes the default
//!   ESX version and provided EC2 instance types." (`GetVersions`.) `@readonly`, so a
//!   read changes nothing, and both output members are `@required`.
//! * "The status for this VCF version. Valid values are: `AVAILABLE` … `RESTRICTED` …"
//!   (`VcfVersionInfo$status`.)
//! * "The default ESX version for this VCF version. It is based on Broadcom's Bill Of
//!   Materials (BOM)." (`VcfVersionInfo$defaultEsxVersion`.)
//! * "EC2 instance types provided by Amazon EVS for this VCF version for creating
//!   environments." (`VcfVersionInfo$instanceTypes`.)
//! * "Information about ESX versions offered for each EC2 instance type." /  "The list of
//!   ESX versions offered for this instance type." (`InstanceTypeEsxVersionsInfo`.)
//! * "`SELF_DEPLOYED`: You install VCF yourself." (`CreateEnvironmentRequest$vcfVersion`
//!   doc) — that value decides who installs VCF, and it is a `VcfVersion` the create
//!   operation takes, so the discovery call reports it like any other.
//! * "If you don't specify an ESX version when adding hosts using CreateEnvironmentHost
//!   action, Amazon EVS automatically uses the default ESX version for your environment's
//!   VCF version. To find the available ESX versions for a particular VCF version, use
//!   the GetVersions action." (`CreateEnvironmentHost`.)
//!
//! The rows differ along what the rule generalises over: which VCF version is reported,
//! which EC2 instance type, whether a host names an ESX version or leaves it to the
//! catalogue, how many times the catalogue is read, and what the account holds when it is
//! read. The completeness of the catalogue is checked against the `VcfVersion` enum values
//! this crate generates FROM the model, and against `CreateEnvironment` itself: a version
//! the create operation serves is one the catalogue describes, and a string the model does
//! not declare is refused there and absent here.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};

use service_com_amazonaws_evs_amazonelasticvmwareservice::ops::types::VCF_VERSION_VALUES;
use service_com_amazonaws_evs_amazonelasticvmwareservice::state as evs;

use crate::environment_lifecycle::{
    call, emulator, error_type, settle, vlans_with_prefix, Emulator,
};

/// Every `VcfVersion` value the model declares, written out here rather than read from
/// the service, so this suite cannot drift along with the handler it checks.
///
/// Source: `smithy_query(query="shape", shape="VcfVersion")` — `VCF-5.2.1`, `VCF-5.2.2`,
/// `SELF_DEPLOYED`. [`the_catalogue_covers_every_vcf_version_the_model_declares`] holds
/// this list to the enum values this crate generates FROM that model, so a model that
/// gains or loses a value cannot leave the list — or the catalogue — behind.
const VCF_VERSIONS: [&str; 3] = ["VCF-5.2.1", "VCF-5.2.2", "SELF_DEPLOYED"];

/// A string that names no declared `VcfVersion`: the negative row of the table in
/// [`no_version_create_environment_serves_is_missing_from_the_catalogue`], which is what
/// keeps the agreement between the catalogue and `CreateEnvironment` from being vacuous.
const UNDECLARED_VCF_VERSION: &str = "VCF-9.9.9";

/// A `hostName` holds letters, digits and hyphens only (`smithy_query(query="shape",
/// shape="HostName")` reports the pattern `^([a-zA-Z0-9\-]*)$`), so a VCF version worked
/// into a host name has its separators replaced.
fn slug(vcf_version: &str) -> String {
    vcf_version
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

/// Every `InstanceType` value the model declares.
///
/// Source: `smithy_query(query="shape", shape="InstanceType")` — `i4i.metal`,
/// `i7i.metal-24xl`, `i7i.metal-48xl`, `r7i.metal-24xl`, `r7i.metal-48xl`. The suites that
/// already cover host creation cite `i4i.metal`; the rows below cover them all, and
/// `tests/all/host_instance_type_catalogue.rs` holds this list to the enum values this
/// crate generates FROM the model.
const INSTANCE_TYPES: [&str; 5] = [
    "i4i.metal",
    "i7i.metal-24xl",
    "i7i.metal-48xl",
    "r7i.metal-24xl",
    "r7i.metal-48xl",
];

/// The `VcfVersionInfo$status` values the member's own documentation declares: "Valid
/// values are: `AVAILABLE` - This VCF version is available to you. `RESTRICTED` - This VCF
/// version has limited availability."
const STATUSES: [&str; 2] = ["AVAILABLE", "RESTRICTED"];

/// `EsxVersion`'s modelled bound: `@length(max: 128)`
/// (`smithy_query(query="shape", shape="EsxVersion")`).
const ESX_VERSION_MAX_LENGTH: usize = 128;

/// One VCF version as `GetVersions` reported it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ReportedVersion {
    vcf_version: String,
    status: String,
    default_esx_version: String,
    instance_types: Vec<String>,
}

/// Call `GetVersions` and answer the body, failing the test if it is not a 200.
async fn get_versions(router: &Emulator) -> Value {
    let (status, body) = call(router, "GetVersions", &json!({})).await;
    assert_eq!(
        status, 200,
        "case \"a plain GetVersions call\": expected 200, got {status} ({}) {body}",
        error_type(&body)
    );
    body
}

/// The `vcfVersions` list a response carries, in the order it reported them. Every member
/// of `VcfVersionInfo` is `@required`, so an absent one fails the test.
fn reported_versions(case: &str, body: &Value) -> Vec<ReportedVersion> {
    let list = body["vcfVersions"].as_array().unwrap_or_else(|| {
        panic!("case {case:?}: GetVersions must report a `vcfVersions` list, got {body}")
    });
    list.iter()
        .map(|entry| ReportedVersion {
            vcf_version: string_member(case, entry, "vcfVersion"),
            status: string_member(case, entry, "status"),
            default_esx_version: string_member(case, entry, "defaultEsxVersion"),
            instance_types: string_list(case, entry, "instanceTypes"),
        })
        .collect()
}

/// The `instanceTypeEsxVersions` list a response carries, as `instance type -> ESX
/// versions` in the order it reported them.
fn reported_instance_types(case: &str, body: &Value) -> Vec<(String, Vec<String>)> {
    let list = body["instanceTypeEsxVersions"]
        .as_array()
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: GetVersions must report an `instanceTypeEsxVersions` list, \
                 got {body}"
            )
        });
    list.iter()
        .map(|entry| {
            (
                string_member(case, entry, "instanceType"),
                string_list(case, entry, "esxVersions"),
            )
        })
        .collect()
}

/// One required string member of a reported entry.
fn string_member(case: &str, entry: &Value, member: &str) -> String {
    entry[member]
        .as_str()
        .unwrap_or_else(|| {
            panic!("case {case:?}: {member} is a required member, got {entry}")
        })
        .to_string()
}

/// One required list-of-string member of a reported entry.
fn string_list(case: &str, entry: &Value, member: &str) -> Vec<String> {
    entry[member]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: {member} is a required member, got {entry}"))
        .iter()
        .map(|value| {
            value
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: {member} holds a non-string: {entry}"))
                .to_string()
        })
        .collect()
}

/// Create an environment at `vcf_version` and wait for the deployment the clock owes it,
/// so a host can be added to it. The VLAN blocks are `/24` so one subnet holds every host
/// a row below adds.
async fn deployed_environment(router: &Emulator, case: &str, vcf_version: &str) -> String {
    let (vpc_id, subnet_id) = router.fresh_network();
    let request = json!({
        "vpcId": vpc_id,
        "serviceAccessSubnetId": subnet_id,
        "vcfVersion": vcf_version,
        "termsAccepted": true,
        "environmentName": format!("catalogue-{}", slug(vcf_version)),
        "initialVlans": vlans_with_prefix(24),
    });
    let (status, body) = call(router, "CreateEnvironment", &request).await;
    assert_eq!(
        status, 200,
        "case {case:?}: a VCF version the catalogue reports must be accepted by \
         CreateEnvironment, got {status} ({}) {body}",
        error_type(&body)
    );
    let reported = body["environment"]["vcfVersion"].as_str().unwrap_or("<none>");
    assert_eq!(
        reported, vcf_version,
        "case {case:?}: the environment must report the VCF version it was created at, \
         got {reported:?}"
    );
    let environment = body["environment"]["environmentId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no environmentId in {body}"))
        .to_string();
    // The environment is CREATING until the move to CREATED the clock owes it comes due,
    // and a host can only be added to a deployed environment. The transition is applied
    // against an instant past its due one rather than waited for, so this suite's rows
    // cost no wall-clock time.
    settle(router);
    environment
}

#[tokio::test]
async fn the_catalogue_reports_every_vcf_version_amazon_evs_provides() {
    let router = emulator();
    let body = get_versions(&router).await;
    let versions = reported_versions("the provided VCF versions", &body);

    // The set is every modelled `VcfVersion`: `GetVersions` is the discovery call that
    // tells a caller which versions it may pass to `CreateEnvironment`, and that operation
    // takes every declared value — the one that means the caller installs VCF itself
    // included, for which Amazon EVS still provides the ESX hosts.
    let expected: BTreeSet<&str> = VCF_VERSIONS.iter().copied().collect();
    let actual: BTreeSet<&str> = versions
        .iter()
        .map(|version| version.vcf_version.as_str())
        .collect();
    assert_eq!(
        actual, expected,
        "case \"the provided VCF versions\": expected {expected:?}, got {actual:?}"
    );
    assert_eq!(
        versions.len(),
        actual.len(),
        "case \"the provided VCF versions\": no version may be reported twice, got {:?}",
        versions
            .iter()
            .map(|version| version.vcf_version.as_str())
            .collect::<Vec<_>>()
    );

    for version in &versions {
        let case = version.vcf_version.as_str();
        assert!(
            STATUSES.contains(&version.status.as_str()),
            "case {case:?}: the status must be one of {STATUSES:?}, got {:?}",
            version.status
        );
        assert!(
            !version.default_esx_version.is_empty()
                && version.default_esx_version.chars().count() <= ESX_VERSION_MAX_LENGTH,
            "case {case:?}: the default ESX version must be a non-empty EsxVersion within \
             {ESX_VERSION_MAX_LENGTH} characters, got {:?}",
            version.default_esx_version
        );
        assert!(
            !version.instance_types.is_empty(),
            "case {case:?}: a provided VCF version is offered on at least one EC2 instance \
             type, got {:?}",
            version.instance_types
        );
        for instance_type in &version.instance_types {
            assert!(
                INSTANCE_TYPES.contains(&instance_type.as_str()),
                "case {case:?}: every reported instance type must be a modelled \
                 InstanceType {INSTANCE_TYPES:?}, got {instance_type:?}"
            );
        }
        let distinct: BTreeSet<&str> = version
            .instance_types
            .iter()
            .map(String::as_str)
            .collect();
        assert_eq!(
            distinct.len(),
            version.instance_types.len(),
            "case {case:?}: no instance type may be reported twice, got {:?}",
            version.instance_types
        );
    }
}

#[tokio::test]
async fn every_member_of_a_reported_version_is_that_versions_own_catalogue_fact() {
    let router = emulator();
    let body = get_versions(&router).await;
    let versions = reported_versions("the catalogue's own entries", &body);

    // Each member of `VcfVersionInfo` is a PER-VERSION fact, so the response is the
    // service's version catalogue projected entry by entry and member by member — no
    // member is a constant substituted for every version. The expectation is the catalogue
    // this service holds, not a value written here: which version is restricted, which
    // build its Bill Of Materials defaults to and which hardware it is offered on all move
    // as Amazon EVS publishes new builds, while the projection does not.
    let catalogue = evs::PROVIDED_VCF_VERSIONS;
    assert_eq!(
        versions.len(),
        catalogue.len(),
        "case \"the catalogue's own entries\": expected {} entries, got {:?}",
        catalogue.len(),
        versions
    );
    for (provided, reported) in catalogue.iter().zip(versions.iter()) {
        let case = provided.vcf_version;
        assert_eq!(
            reported.vcf_version, provided.vcf_version,
            "case {case:?}: expected the catalogue's entry for {:?}, got {:?}",
            provided.vcf_version, reported.vcf_version
        );
        assert_eq!(
            reported.status, provided.status,
            "case {case:?}: expected the status the catalogue carries for it ({:?}), got \
             {:?}",
            provided.status, reported.status
        );
        assert!(
            STATUSES.contains(&reported.status.as_str()),
            "case {case:?}: the status must be one of {STATUSES:?}, got {:?}",
            reported.status
        );
        assert_eq!(
            reported.default_esx_version, provided.default_esx_version,
            "case {case:?}: expected the default build the catalogue carries for it ({:?}), \
             got {:?}",
            provided.default_esx_version, reported.default_esx_version
        );
        let expected_types: Vec<&str> = provided.instance_types.to_vec();
        let reported_types: Vec<&str> = reported
            .instance_types
            .iter()
            .map(String::as_str)
            .collect();
        assert_eq!(
            reported_types, expected_types,
            "case {case:?}: expected the instance types the catalogue offers this version \
             ({expected_types:?}), got {reported_types:?}"
        );
        for instance_type in &reported_types {
            assert!(
                INSTANCE_TYPES.contains(instance_type),
                "case {case:?}: every offered instance type is a modelled InstanceType \
                 {INSTANCE_TYPES:?}, got {instance_type:?}"
            );
        }
    }

    // Both documented values of the member are reportable: the statuses the response
    // carries are exactly the ones the catalogue holds, so a version of limited
    // availability is reported as limited rather than as freely usable.
    let held: BTreeSet<&str> = catalogue.iter().map(|provided| provided.status).collect();
    let announced: BTreeSet<&str> = versions
        .iter()
        .map(|version| version.status.as_str())
        .collect();
    assert_eq!(
        announced, held,
        "case \"the statuses the catalogue holds\": expected {held:?}, got {announced:?}"
    );
    for status in &held {
        assert!(
            STATUSES.contains(status),
            "case \"the statuses the catalogue holds\": {status:?} is not one of \
             {STATUSES:?}"
        );
    }
}

#[tokio::test]
async fn each_instance_type_is_offered_the_esx_versions_of_the_versions_that_provide_it() {
    let router = emulator();
    let body = get_versions(&router).await;
    let versions = reported_versions("the catalogue's two halves", &body);
    let instance_types = reported_instance_types("the catalogue's two halves", &body);

    // The two halves of ONE response, checked against each other rather than against a
    // value written here: every instance type a provided VCF version lists is reported,
    // and the ESX versions it is offered include that version's default — the build a
    // host of that version runs when the request names none.
    let mut expected: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    for version in &versions {
        for instance_type in &version.instance_types {
            expected
                .entry(instance_type.as_str())
                .or_default()
                .insert(version.default_esx_version.as_str());
        }
    }

    let actual: BTreeMap<&str, BTreeSet<&str>> = instance_types
        .iter()
        .map(|(instance_type, esx_versions)| {
            (
                instance_type.as_str(),
                esx_versions.iter().map(String::as_str).collect(),
            )
        })
        .collect();
    for (instance_type, defaults) in &expected {
        let offered = actual.get(instance_type).unwrap_or_else(|| {
            panic!(
                "case {instance_type:?}: an instance type a provided VCF version lists must \
                 be reported with the ESX versions offered on it, got {actual:?}"
            )
        });
        assert!(
            defaults.is_subset(offered),
            "case {instance_type:?}: every default ESX version of a VCF version this type \
             is offered must be among its esxVersions, expected {defaults:?} within \
             {offered:?}"
        );
    }
    assert_eq!(
        instance_types.len(),
        actual.len(),
        "case \"the catalogue's two halves\": no instance type may be reported twice, got {:?}",
        instance_types
            .iter()
            .map(|(instance_type, _)| instance_type.as_str())
            .collect::<Vec<_>>()
    );
    for (instance_type, esx_versions) in &instance_types {
        assert!(
            !esx_versions.is_empty(),
            "case {instance_type:?}: an instance type Amazon EVS provides is offered at \
             least one ESX version, got {esx_versions:?}"
        );
        let distinct: BTreeSet<&str> = esx_versions.iter().map(String::as_str).collect();
        assert_eq!(
            distinct.len(),
            esx_versions.len(),
            "case {instance_type:?}: no ESX version may be reported twice, got \
             {esx_versions:?}"
        );
    }
}

#[tokio::test]
async fn every_version_and_instance_type_the_catalogue_reports_is_one_the_service_accepts() {
    let router = emulator();
    let body = get_versions(&router).await;
    let versions = reported_versions("the catalogue the service acts on", &body);

    // "EC2 instance types provided by Amazon EVS for this VCF version for creating
    // environments": what the catalogue reports is what the service accepts, so each
    // reported version creates an environment and each instance type it reports adds a
    // host to that environment.
    for version in &versions {
        let case = version.vcf_version.as_str();
        let environment = deployed_environment(&router, case, case).await;
        for (index, instance_type) in version.instance_types.iter().enumerate() {
            let case = format!("{case} on {instance_type}");
            let host_name = format!("esx-{}-{index}", slug(&case));
            let request = json!({
                "environmentId": environment,
                "host": {
                    "hostName": host_name,
                    "keyName": "evs-key",
                    "instanceType": instance_type,
                },
            });
            let (status, answer) = call(&router, "CreateEnvironmentHost", &request).await;
            assert_eq!(
                status, 200,
                "case {case:?}: an instance type the catalogue reports for this VCF version \
                 must be accepted, got {status} ({}) {answer}",
                error_type(&answer)
            );
            let reported = answer["host"]["instanceType"].as_str().unwrap_or("<none>");
            assert_eq!(
                reported, instance_type,
                "case {case:?}: the host must report the instance type it was created \
                 with, got {reported:?}"
            );
        }

        let (status, answer) = call(
            &router,
            "ListEnvironmentHosts",
            &json!({ "environmentId": environment }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from the read-back, got {status} {answer}"
        );
        let listed: BTreeSet<&str> = answer["environmentHosts"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: no environmentHosts in {answer}"))
            .iter()
            .filter_map(|host| host["instanceType"].as_str())
            .collect();
        let expected: BTreeSet<&str> = version
            .instance_types
            .iter()
            .map(String::as_str)
            .collect();
        assert_eq!(
            listed, expected,
            "case {case:?}: the environment must hold a host of every instance type the \
             catalogue reports for its version, expected {expected:?}, got {listed:?}"
        );
    }
}

#[tokio::test]
async fn a_host_that_names_no_esx_version_runs_the_catalogue_default_for_its_vcf_version() {
    let router = emulator();
    let body = get_versions(&router).await;
    let versions = reported_versions("the default ESX version", &body);

    // "If you don't specify an ESX version when adding hosts using CreateEnvironmentHost
    // action, Amazon EVS automatically uses the default ESX version for your environment's
    // VCF version." The version a host runs is not a member of the modelled `Host` shape,
    // so it is read back out of this service's own store.
    //
    // One row per provided VCF version, and in each: a host that names no ESX version, and
    // a host that names the version the catalogue offers for that VCF version. A version
    // the catalogue does NOT offer is refused instead — "To find the available ESX versions
    // for a particular VCF version, use the GetVersions action" — which the loop below
    // asserts after the accepted rows.
    let named: [(&str, bool); 2] = [
        ("no esxVersion at all", false),
        ("the esxVersion the catalogue offers", true),
    ];

    for version in &versions {
        let vcf_version = version.vcf_version.as_str();
        let environment = deployed_environment(&router, vcf_version, vcf_version).await;
        for (index, (named_case, names_the_offered_version)) in named.iter().enumerate() {
            let case = format!("{vcf_version} with {named_case}");
            let host_name = format!("esx-default-{index}-{}", slug(vcf_version));
            let esx_version = names_the_offered_version.then(|| version.default_esx_version.clone());
            let mut request = json!({
                "environmentId": environment,
                "host": {
                    "hostName": host_name,
                    "keyName": "evs-key",
                    "instanceType": INSTANCE_TYPES[index % INSTANCE_TYPES.len()],
                },
            });
            if let Some(esx_version) = esx_version.as_deref() {
                request["esxVersion"] = json!(esx_version);
            }
            let (status, answer) = call(&router, "CreateEnvironmentHost", &request).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected 200, got {status} ({}) {answer}",
                error_type(&answer)
            );

            // The version the host runs: the one the request named, or the catalogue's
            // default for the environment's VCF version.
            let expected = esx_version.unwrap_or_else(|| version.default_esx_version.clone());
            let state = evs::service_state(&*router.engine)
                .expect("the in-memory engine carries this service's store");
            let stored = state
                .lock()
                .environments
                .get(&environment)
                .and_then(|environment| environment.hosts.get(&host_name))
                .and_then(|host| host.esx_version.clone());
            assert_eq!(
                stored.as_deref(),
                Some(expected.as_str()),
                "case {case:?}: expected the host to run ESX {expected:?}, got {stored:?}"
            );
        }

        // A version the catalogue does not offer for this VCF version names no ESX
        // version Amazon EVS can install, so the host is refused and nothing is stored:
        // one row per form such a string takes — another release, a build-qualified
        // version of the offered one, and a word.
        let outside = [
            "7.0.3".to_string(),
            format!("{}-24674464", version.default_esx_version),
            "latest".to_string(),
        ];
        for (index, esx_version) in outside.iter().enumerate() {
            let case = format!("{vcf_version} with the esxVersion {esx_version}");
            let host_name = format!("esx-outside-{index}-{}", slug(vcf_version));
            let request = json!({
                "environmentId": environment,
                "esxVersion": esx_version,
                "host": {
                    "hostName": host_name,
                    "keyName": "evs-key",
                    "instanceType": INSTANCE_TYPES[index % INSTANCE_TYPES.len()],
                },
            });
            let (status, answer) = call(&router, "CreateEnvironmentHost", &request).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected 400, got {status} ({}) {answer}",
                error_type(&answer)
            );
            let named_members: Vec<&str> = answer["fieldList"]
                .as_array()
                .map(|entries| {
                    entries
                        .iter()
                        .filter_map(|entry| entry["name"].as_str())
                        .collect()
                })
                .unwrap_or_default();
            assert!(
                named_members.contains(&"esxVersion"),
                "case {case:?}: the refusal must name esxVersion, got {answer}"
            );
            let state = evs::service_state(&*router.engine)
                .expect("the in-memory engine carries this service's store");
            let stored = state
                .lock()
                .environments
                .get(&environment)
                .map(|environment| environment.hosts.contains_key(&host_name));
            assert_eq!(
                stored,
                Some(false),
                "case {case:?}: a refused request must store no host"
            );
        }
    }
}

#[tokio::test]
async fn the_catalogue_is_a_read_that_answers_the_same_whatever_the_account_holds() {
    let router = emulator();
    let first = get_versions(&router).await;

    // `@readonly`: however often it is called, the answer is the same one — byte for byte,
    // members and order included.
    for read in 1..=5 {
        let case = format!("read {read}");
        let again = get_versions(&router).await;
        assert_eq!(
            again, first,
            "case {case:?}: every read must report the same catalogue, got {again}"
        );
    }

    // And the catalogue is the SERVICE's, not the account's: creating an environment and
    // adding a host to it changes nothing about what Amazon EVS provides.
    let provided = reported_versions("the account's own resources", &first);
    let version = provided
        .first()
        .expect("at least one VCF version is provided")
        .vcf_version
        .clone();
    let environment = deployed_environment(&router, "the account's own resources", &version).await;
    let (status, answer) = call(
        &router,
        "CreateEnvironmentHost",
        &json!({
            "environmentId": environment,
            "host": {
                "hostName": "esx-catalogue-unchanged",
                "keyName": "evs-key",
                "instanceType": INSTANCE_TYPES[0],
            },
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"the account's own resources\": the host must be added, got {status} {answer}"
    );

    let after = get_versions(&router).await;
    assert_eq!(
        after, first,
        "case \"the account's own resources\": what the account holds must not change the \
         catalogue, got {after}"
    );
}

#[tokio::test]
async fn the_catalogue_covers_every_vcf_version_the_model_declares() {
    let router = emulator();
    let body = get_versions(&router).await;
    let versions = reported_versions("every declared VcfVersion", &body);
    let reported: BTreeSet<&str> = versions
        .iter()
        .map(|version| version.vcf_version.as_str())
        .collect();

    // The expectation here is the `VcfVersion` enum values this crate GENERATES from the
    // Smithy model, not a list written out in a test: the catalogue is complete with
    // respect to the model, so a declared value it omits fails this row whichever value
    // that is, and a model that gains or loses a value moves the expectation with it.
    let declared: BTreeSet<&str> = VCF_VERSION_VALUES.iter().copied().collect();
    assert_eq!(
        reported, declared,
        "case \"every declared VcfVersion\": the catalogue must report one entry per \
         declared VcfVersion, expected {declared:?}, got {reported:?}"
    );

    // …and the list this suite writes out is that same set, so the rows above cover every
    // declared value too.
    let written_out: BTreeSet<&str> = VCF_VERSIONS.iter().copied().collect();
    assert_eq!(
        written_out, declared,
        "case \"this suite's own list of versions\": expected the declared values \
         {declared:?}, got {written_out:?}"
    );

    // A string the model does not declare is no part of the catalogue: the set is the
    // declared values, not a superset of them.
    assert!(
        !reported.contains(UNDECLARED_VCF_VERSION),
        "case \"an undeclared VcfVersion\": the catalogue must not report \
         {UNDECLARED_VCF_VERSION:?}, got {reported:?}"
    );
}

#[tokio::test]
async fn no_version_create_environment_serves_is_missing_from_the_catalogue() {
    let router = emulator();
    let catalogue = get_versions(&router).await;
    let reported: Vec<String> = reported_versions("the versions the create operation takes", &catalogue)
        .iter()
        .map(|version| version.vcf_version.clone())
        .collect();

    // `GetVersions` is the discovery call for what `CreateEnvironment` takes, so the two
    // agree in BOTH directions: a version the create operation serves is described by the
    // catalogue, and a version it refuses is absent from it. The rows differ along the one
    // thing the rule generalises over — which `VcfVersion` the request names — with the
    // undeclared string as the row that would pass vacuously if the catalogue simply
    // reported everything.
    let rows: Vec<(&str, bool)> = VCF_VERSION_VALUES
        .iter()
        .map(|vcf_version| (*vcf_version, true))
        .chain(std::iter::once((UNDECLARED_VCF_VERSION, false)))
        .collect();

    for (vcf_version, declared) in rows {
        let case = vcf_version;
        let (vpc_id, subnet_id) = router.fresh_network();
        // Only the five `@required` members: the mode in which the caller installs VCF
        // itself does not use the install parameters, so a request carrying none of them
        // is served whatever version it names and the rows differ along the version alone.
        let request = json!({
            "vpcId": vpc_id,
            "serviceAccessSubnetId": subnet_id,
            "vcfVersion": vcf_version,
            "termsAccepted": true,
            "initialVlans": vlans_with_prefix(24),
        });
        let (status, body) = call(&router, "CreateEnvironment", &request).await;

        if !declared {
            assert_eq!(
                status, 400,
                "case {case:?}: a value outside the enumerated set must be refused, got \
                 {status} {body}"
            );
            assert!(
                error_type(&body).contains("ValidationException"),
                "case {case:?}: expected a ValidationException, got {}",
                error_type(&body)
            );
            assert!(
                !reported.iter().any(|version| version == vcf_version),
                "case {case:?}: a version CreateEnvironment refuses must not be in the \
                 catalogue, got {reported:?}"
            );
            continue;
        }

        assert_eq!(
            status, 200,
            "case {case:?}: a declared VcfVersion must be accepted, got {status} ({}) {body}",
            error_type(&body)
        );
        let environment_id = body["environment"]["environmentId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no environmentId in {body}"))
            .to_string();

        // The round trip: the stored environment reports the version it was created at…
        let (status, got) = call(
            &router,
            "GetEnvironment",
            &json!({ "environmentId": environment_id }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from the read-back, got {status} ({}) {got}",
            error_type(&got)
        );
        assert_eq!(
            got["environment"]["vcfVersion"],
            json!(vcf_version),
            "case {case:?}: the stored environment must report the version it was created \
             at, got {}",
            got["environment"]["vcfVersion"]
        );

        // …and the catalogue describes that very version.
        assert!(
            reported.iter().any(|version| version == vcf_version),
            "case {case:?}: a version CreateEnvironment serves must appear in the \
             catalogue, got {reported:?}"
        );
    }
}
