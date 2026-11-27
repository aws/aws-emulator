// SPDX-License-Identifier: CC0-1.0

//! `Environment$checks` and the `environmentStatus` roll-up, end to end over awsJson1_0.
//!
//! THE RULE under test: a settled Amazon EVS environment reports one `Check` per check
//! type the service can decide from the facts it holds — each with its `type`, a `id`
//! that is unique and the same on every read, a `result`, and an `impairedSince` exactly
//! when that result is `FAILED` — and `environmentStatus` is the ROLL-UP of those
//! results: `FAILED` when any check failed, otherwise `PASSED` when any check passed,
//! and `UNKNOWN` when no check decided anything (an environment that is not `CREATED`
//! has run none).
//!
//! Where each clause comes from (`smithy_query(query="shape",
//! shape="Check,CheckType,CheckResult")` and the `Environment` members):
//!
//! * "A check on the environment to identify environment health and validate VMware VCF
//!   licensing compliance." (`Environment$checks` / `Check`.)
//! * The checks this emulator can decide, from `Check$type`'s own enumeration:
//!   `KEY_REUSE` "Verifies that the VCF license key is not used by another Amazon EVS
//!   environment", `KEY_COVERAGE` "Verifies that the VCF license key allocates
//!   sufficient vCPU cores for all deployed hosts", `HOST_COUNT` "Verifies that the
//!   environment meets the minimum host count", and `CONNECTOR_HEALTH` "Aggregate health
//!   across all connectors in the environment" (whose own rule
//!   `tests/all/environment_connector_health.rs` owns). The other nine types probe SDDC
//!   Manager, vCenter Server, Operations Manager or one connector's appliance — nothing
//!   this emulator runs — so no check of those types is reported here.
//! * "The time when environment health began to be impaired." (`Check$impairedSince`.)
//! * "Reports impaired functionality that stems from issues internal to the
//!   environment." (`Environment$environmentStatus`, a `CheckResult`.)
//! * The minimum host count is four: `CreateEnvironmentRequest$hosts` is modelled
//!   `@length(min: 4, max: 4)` (`smithy_query(query="operation",
//!   operation="CreateEnvironment")`), read here from [`evs::MINIMUM_HOST_COUNT`] so the
//!   suite and the service cannot drift apart over it.
//! * A host's cores are Amazon EC2's own fact about the bare metal instance type it runs
//!   on, and the cores an account's VCF licenses allocate are its own
//!   `vcfPortedCoreCount` setting ("The total number of VCF license cores ported to
//!   Amazon EVS for the account in that Region", `AccountSetting$name` doc).
//!
//! The rows of the tables below differ along what the rule generalises over: the number
//! of hosts an environment holds and whether they came from the create request or from
//! `CreateEnvironmentHost`, the instance type those hosts run on (every value of the
//! modelled `InstanceType` enum), whether a license key is absent, unique, shared with
//! another live environment, shared only through the vSAN member, or shared with an
//! environment that has been deleted, the ported core count against the cores the hosts
//! need (one below, exactly equal, far above, and unset), and the environment's own
//! state.
//!
//! The harness (`emulator`, `call`, `create`) is the one `environment_lifecycle` already
//! owns, so every test drives the service the way a client does.

use std::collections::BTreeSet;
use std::time::Duration;

use serde_json::{json, Value};

use service_com_amazonaws_ec2_amazonec2 as ec2;
use service_com_amazonaws_evs_amazonelasticvmwareservice::ops::types as evs_types;
use service_com_amazonaws_evs_amazonelasticvmwareservice::state as evs;

use crate::environment_lifecycle::{call, emulator, error_type, Emulator};
use crate::host_instance_type_catalogue::vcf_version_providing;

/// Long enough for the transition this service owes a new or deleted environment, or a
/// new host, to come due (its span is under a second by construction).
const PAST_THE_TRANSITION: Duration = Duration::from_millis(900);

/// The check types a settled environment of this emulator reports, in the order the
/// service reports them. Read from the service's own catalog, so a check type added
/// there fails this suite rather than passing unnoticed.
fn reported_check_types() -> Vec<&'static str> {
    vec![
        evs::CHECK_TYPE_KEY_REUSE,
        evs::CHECK_TYPE_KEY_COVERAGE,
        evs::CHECK_TYPE_HOST_COUNT,
        evs::CHECK_TYPE_CONNECTOR_HEALTH,
    ]
}

/// One VCF license key of the modelled shape
/// (`^[a-zA-Z0-9]{5}-[a-zA-Z0-9]{5}-[a-zA-Z0-9]{5}-[a-zA-Z0-9]{5}-[a-zA-Z0-9]{5}$`),
/// built from `seed` so two rows can ask for the same key or for different ones.
pub(crate) fn license_key(seed: &str) -> String {
    let group: String = format!("{seed}XXXXX").chars().take(5).collect();
    vec![group; 5].join("-")
}

/// A `licenseInfo` list (modelled `@length(1..1)`) carrying one solution key and one
/// vSAN key.
pub(crate) fn license_info(solution: &str, vsan: &str) -> Value {
    json!([{ "solutionKey": license_key(solution), "vsanKey": license_key(vsan) }])
}

/// A `hosts` list for a create request: one entry per name, all on `instance_type`.
pub(crate) fn hosts_json(names: &[&str], instance_type: &str) -> Value {
    names
        .iter()
        .map(|name| json!({ "hostName": name, "keyName": "evs-key", "instanceType": instance_type }))
        .collect()
}

/// Create an environment with `extra` merged into the request every rule accepts, and
/// return its id.
pub(crate) async fn create_with(router: &Emulator, case: &str, name: &str, extra: Value) -> String {
    let mut request = router.create_request(Some(name));
    for (member, value) in extra.as_object().expect("the extra members are an object") {
        request[member] = value.clone();
    }
    let (status, body) = call(router, "CreateEnvironment", &request).await;
    assert_eq!(
        status, 200,
        "case {case:?}: create {name}: expected HTTP 200, got {status} ({}) {body}",
        error_type(&body)
    );
    body["environment"]["environmentId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: create {name}: no environmentId in {body}"))
        .to_string()
}

/// The `environment` object `GetEnvironment` reports.
async fn get_environment(router: &Emulator, case: &str, environment_id: &str) -> Value {
    let (status, body) = call(
        router,
        "GetEnvironment",
        &json!({ "environmentId": environment_id }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: get {environment_id}: expected HTTP 200, got {status} ({}) {body}",
        error_type(&body)
    );
    body["environment"].clone()
}

/// Add one host to a deployed environment.
pub(crate) async fn add_host(
    router: &Emulator,
    case: &str,
    environment_id: &str,
    name: &str,
    instance_type: &str,
) {
    let (status, body) = call(
        router,
        "CreateEnvironmentHost",
        &json!({
            "environmentId": environment_id,
            "host": { "hostName": name, "keyName": "evs-key", "instanceType": instance_type },
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: add host {name}: expected HTTP 200, got {status} ({}) {body}",
        error_type(&body)
    );
}

/// The `Check` of one type an environment reports, or `Value::Null` when it reports none.
pub(crate) fn check_of(environment: &Value, check_type: &str) -> Value {
    environment["checks"]
        .as_array()
        .unwrap_or(&Vec::new())
        .iter()
        .find(|check| check["type"] == json!(check_type))
        .cloned()
        .unwrap_or(Value::Null)
}

/// The `result` of one check type, for an assertion message.
fn result_of(environment: &Value, check_type: &str) -> String {
    check_of(environment, check_type)["result"]
        .as_str()
        .unwrap_or("<no check>")
        .to_string()
}

/// The physical processor cores one host running on `instance_type` has, as Amazon EC2's
/// own instance-type record reports them (`VCpuInfo$DefaultCores`) — the same fact the
/// service reads, so this suite pins the RULE and not a core count copied by hand.
fn cores(instance_type: &str) -> u64 {
    let record = ec2::state::instance_type_record(instance_type).unwrap_or_else(|| {
        panic!("Amazon EC2 describes the Amazon EVS instance type {instance_type}")
    });
    let cores = u64::try_from(record.default_cores).expect("a core count is not negative");
    assert!(
        cores > 0,
        "instance type {instance_type}: expected a positive core count, got {cores}"
    );
    cores
}

/// Write the account's ported VCF license core count for the Region the tests address.
pub(crate) async fn put_ported_cores(router: &Emulator, case: &str, cores: u64) {
    let (status, body) = call(
        router,
        "PutAccountSettings",
        &json!({
            "settings": [{ "name": evs::VCF_PORTED_CORE_COUNT, "value": cores.to_string() }],
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: put {} = {cores}: expected HTTP 200, got {status} ({}) {body}",
        evs::VCF_PORTED_CORE_COUNT,
        error_type(&body)
    );
}

#[tokio::test]
async fn a_settled_environment_reports_one_check_per_decidable_check_type() {
    let router = emulator();
    let environment_id = create_with(
        &router,
        "a settled environment",
        "health-shape",
        json!({ "licenseInfo": license_info("AAAAA", "BBBBB") }),
    )
    .await;
    std::thread::sleep(PAST_THE_TRANSITION);

    let case = "a settled environment";
    let environment = get_environment(&router, case, &environment_id).await;
    let checks = environment["checks"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: a settled environment reports checks, got {environment}"))
        .clone();
    let types: Vec<&str> = checks
        .iter()
        .filter_map(|check| check["type"].as_str())
        .collect();
    assert_eq!(
        types,
        reported_check_types(),
        "case {case:?}: expected one check per decidable check type, got {types:?}"
    );

    let mut ids = BTreeSet::new();
    for check in &checks {
        let check_type = check["type"].as_str().unwrap_or_default();
        let id = check["id"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: {check_type}: no id in {check}"));
        assert_eq!(
            id,
            evs::check_id(&environment_id, check_type),
            "case {case:?}: {check_type}: expected the id of this check of this environment, got {id}"
        );
        assert!(
            ids.insert(id.to_string()),
            "case {case:?}: {check_type}: id {id} is already used by another check"
        );
        let result = check["result"].as_str().unwrap_or_default();
        assert!(
            [evs::CHECK_PASSED, evs::CHECK_FAILED, evs::CHECK_UNKNOWN].contains(&result),
            "case {case:?}: {check_type}: result {result:?} is not a CheckResult"
        );
        // `impairedSince` is "the time when environment health began to be impaired", so
        // exactly a FAILED check carries one.
        let impaired = &check["impairedSince"];
        assert_eq!(
            !impaired.is_null(),
            result == evs::CHECK_FAILED,
            "case {case:?}: {check_type}: a {result} check carries impairedSince {impaired}"
        );
    }

    // A read changes nothing a later caller sees: the same checks, ids and roll-up.
    let again = get_environment(&router, case, &environment_id).await;
    assert_eq!(
        again["checks"], environment["checks"],
        "case {case:?}: a second read reported different checks: {} then {}",
        environment["checks"], again["checks"]
    );
    assert_eq!(
        again["environmentStatus"], environment["environmentStatus"],
        "case {case:?}: a second read reported a different roll-up, got {}",
        again["environmentStatus"]
    );
}

#[tokio::test]
async fn host_count_is_the_minimum_host_count_against_the_hosts_the_environment_holds() {
    let router = emulator();
    let minimum = evs::MINIMUM_HOST_COUNT;
    // Rows: how many hosts the create request asked for, how many are added with
    // CreateEnvironmentHost afterwards, and the instance type they run on — every value
    // of the modelled `InstanceType` enum appears, so the rule is not tested on one type.
    let rows: Vec<(&str, usize, usize, &str)> = vec![
        ("no host at all", 0, 0, "i4i.metal"),
        ("one added host", 0, 1, "i4i.metal"),
        ("one short of the minimum", 0, minimum - 1, "i7i.metal-24xl"),
        ("exactly the minimum, added", 0, minimum, "i4i.metal"),
        ("one past the minimum, added", 0, minimum + 1, "i7i.metal-48xl"),
        ("the minimum, from the create request", minimum, 0, "i4i.metal"),
        ("the create request plus one added", minimum, 1, "i4i.metal"),
    ];

    for (index, (case, requested, added, instance_type)) in rows.into_iter().enumerate() {
        let names: Vec<String> = (0..requested)
            .map(|host| format!("req-{index}-{host}"))
            .collect();
        let requested_names: Vec<&str> = names.iter().map(String::as_str).collect();
        let mut extra = json!({});
        // A host is built only on hardware its environment's VCF version is provided on
        // ("EC2 instance types provided by Amazon EVS for this VCF version for creating
        // environments", `VcfVersionInfo$instanceTypes`), so the environment of a row runs
        // a version the catalogue provides that row's instance type on —
        // `tests/all/host_instance_type_catalogue.rs` owns that rule.
        extra["vcfVersion"] = json!(vcf_version_providing(instance_type));
        if requested > 0 {
            extra["hosts"] = hosts_json(&requested_names, instance_type);
        }
        let environment_id =
            create_with(&router, case, &format!("host-count-{index}"), extra).await;
        std::thread::sleep(PAST_THE_TRANSITION);
        for host in 0..added {
            add_host(
                &router,
                case,
                &environment_id,
                &format!("add-{index}-{host}"),
                instance_type,
            )
            .await;
        }

        let held = requested + added;
        let expected = if held >= minimum {
            evs::CHECK_PASSED
        } else {
            evs::CHECK_FAILED
        };
        let environment = get_environment(&router, case, &environment_id).await;
        let actual = result_of(&environment, evs::CHECK_TYPE_HOST_COUNT);
        assert_eq!(
            actual, expected,
            "case {case:?}: {held} host(s) against a minimum of {minimum}: expected \
             {expected}, got {actual}"
        );
        // The roll-up: an environment that fails a check is FAILED, and one whose
        // decided checks all passed is PASSED.
        let status = environment["environmentStatus"].as_str().unwrap_or_default();
        assert_eq!(
            status, expected,
            "case {case:?}: environmentStatus must roll up the checks ({:?}), got {status}",
            environment["checks"]
        );
    }
}

/// One row of the `KEY_REUSE` table: the license keys of the environment under test and
/// of one other environment, whether that other environment is deleted before the test
/// one is created, and the `KEY_REUSE` result the rule gives.
struct KeyReuseRow {
    case: &'static str,
    keys: Option<(&'static str, &'static str)>,
    other_keys: Option<(&'static str, &'static str)>,
    delete_other: bool,
    expected: &'static str,
}

#[tokio::test]
async fn key_reuse_fails_for_a_license_key_another_live_environment_carries() {
    let router = emulator();
    let rows = vec![
        KeyReuseRow {
            case: "no license key at all",
            keys: None,
            other_keys: None,
            delete_other: false,
            expected: evs::CHECK_UNKNOWN,
        },
        KeyReuseRow {
            case: "keys no other environment carries",
            keys: Some(("AAAAA", "BBBBB")),
            other_keys: Some(("CCCCC", "DDDDD")),
            delete_other: false,
            expected: evs::CHECK_PASSED,
        },
        KeyReuseRow {
            case: "a solution key another environment carries",
            keys: Some(("EEEEE", "FFFFF")),
            other_keys: Some(("EEEEE", "GGGGG")),
            delete_other: false,
            expected: evs::CHECK_FAILED,
        },
        KeyReuseRow {
            case: "only the vSAN key is shared",
            keys: Some(("HHHHH", "IIIII")),
            other_keys: Some(("JJJJJ", "IIIII")),
            delete_other: false,
            expected: evs::CHECK_FAILED,
        },
        KeyReuseRow {
            case: "a key carried as a solution key here and a vSAN key there",
            keys: Some(("KKKKK", "LLLLL")),
            other_keys: Some(("MMMMM", "KKKKK")),
            delete_other: false,
            expected: evs::CHECK_FAILED,
        },
        KeyReuseRow {
            case: "the only other holder has been deleted",
            keys: Some(("NNNNN", "OOOOO")),
            other_keys: Some(("NNNNN", "OOOOO")),
            delete_other: true,
            expected: evs::CHECK_PASSED,
        },
    ];

    for (index, row) in rows.into_iter().enumerate() {
        let KeyReuseRow {
            case,
            keys,
            other_keys,
            delete_other,
            expected,
        } = row;
        // The other environment first, so it is already in the store when the one under
        // test is created — the check must not depend on the order they were created in.
        if let Some((solution, vsan)) = other_keys {
            let other = create_with(
                &router,
                case,
                &format!("key-other-{index}"),
                json!({ "licenseInfo": license_info(solution, vsan) }),
            )
            .await;
            if delete_other {
                std::thread::sleep(PAST_THE_TRANSITION);
                let (status, body) = call(
                    &router,
                    "DeleteEnvironment",
                    &json!({ "environmentId": other }),
                )
                .await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: delete the other environment: got {status} ({}) {body}",
                    error_type(&body)
                );
            }
        }
        let mut extra = json!({});
        if let Some((solution, vsan)) = keys {
            extra["licenseInfo"] = license_info(solution, vsan);
        }
        let environment_id = create_with(&router, case, &format!("key-reuse-{index}"), extra).await;
        std::thread::sleep(PAST_THE_TRANSITION);

        let environment = get_environment(&router, case, &environment_id).await;
        let actual = result_of(&environment, evs::CHECK_TYPE_KEY_REUSE);
        assert_eq!(
            actual, expected,
            "case {case:?}: expected KEY_REUSE {expected}, got {actual} in {:?}",
            environment["checks"]
        );
        // Reuse is mutual: the other live environment reports the failure too, which is
        // what makes the check about the KEY and not about who created it first.
        if expected == evs::CHECK_FAILED {
            assert_eq!(
                environment["environmentStatus"],
                json!(evs::CHECK_FAILED),
                "case {case:?}: a failing check must roll up to FAILED, got {}",
                environment["environmentStatus"]
            );
        }
    }
}

#[tokio::test]
async fn key_coverage_compares_the_deployed_cores_against_the_accounts_ported_cores() {
    // Rows: the instance type the four hosts run on, the ported core count to set (as a
    // delta from the cores those hosts need, or nothing at all), whether the environment
    // carries a license key, and the expected KEY_COVERAGE result.
    let rows: Vec<(&str, &str, Option<i64>, bool, &str)> = vec![
        (
            "no ported core count has been set",
            "i4i.metal",
            None,
            true,
            evs::CHECK_UNKNOWN,
        ),
        (
            "no license key to cover the hosts with",
            "i4i.metal",
            Some(0),
            false,
            evs::CHECK_UNKNOWN,
        ),
        (
            "exactly the cores the hosts need",
            "i4i.metal",
            Some(0),
            true,
            evs::CHECK_PASSED,
        ),
        (
            "one core short",
            "i4i.metal",
            Some(-1),
            true,
            evs::CHECK_FAILED,
        ),
        (
            "far short of the cores the hosts need",
            "i7i.metal-48xl",
            Some(-64),
            true,
            evs::CHECK_FAILED,
        ),
        (
            "more cores than the hosts need",
            "i7i.metal-24xl",
            Some(16),
            true,
            evs::CHECK_PASSED,
        ),
    ];

    for (index, (case, instance_type, ported_delta, licensed, expected)) in
        rows.into_iter().enumerate()
    {
        // One emulator per row: the ported core count is an account-wide setting, so a
        // row must not see the value another row wrote.
        let router = emulator();
        let host_count = evs::MINIMUM_HOST_COUNT;
        let required = cores(instance_type) * host_count as u64;
        if let Some(delta) = ported_delta {
            let ported = (required as i64 + delta).max(0) as u64;
            put_ported_cores(&router, case, ported).await;
        }
        let names: Vec<String> = (0..host_count)
            .map(|host| format!("cover-{index}-{host}"))
            .collect();
        let host_names: Vec<&str> = names.iter().map(String::as_str).collect();
        let mut extra = json!({ "hosts": hosts_json(&host_names, instance_type) });
        // The environment runs a VCF version the catalogue provides this row's instance
        // type on, for the reason the host-count rows above name.
        extra["vcfVersion"] = json!(vcf_version_providing(instance_type));
        if licensed {
            extra["licenseInfo"] = license_info("AAAAA", "BBBBB");
        }
        let environment_id =
            create_with(&router, case, &format!("coverage-{index}"), extra).await;
        std::thread::sleep(PAST_THE_TRANSITION);

        let environment = get_environment(&router, case, &environment_id).await;
        let actual = result_of(&environment, evs::CHECK_TYPE_KEY_COVERAGE);
        assert_eq!(
            actual, expected,
            "case {case:?}: {host_count} x {instance_type} needs {required} core(s): expected \
             KEY_COVERAGE {expected}, got {actual} in {:?}",
            environment["checks"]
        );
        // The host count is met in every row here, so the roll-up is decided by the
        // coverage check alone: FAILED when it failed, PASSED when it passed, and PASSED
        // when it could not be decided but another check did pass.
        let expected_status = if expected == evs::CHECK_FAILED {
            evs::CHECK_FAILED
        } else {
            evs::CHECK_PASSED
        };
        let status = environment["environmentStatus"].as_str().unwrap_or_default();
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected environmentStatus {expected_status}, got {status} from {:?}",
            environment["checks"]
        );
    }
}

#[tokio::test]
async fn every_modelled_instance_type_has_a_core_count_a_check_can_read() {
    // The catalog is the modelled `InstanceType` enum the service itself carries
    // (`smithy_query(query="shape", shape="InstanceType")`), so a type added to the
    // model is covered here without this list being edited.
    for instance_type in evs_types::INSTANCE_TYPE_VALUES {
        let record = ec2::state::instance_type_record(instance_type);
        assert!(
            record.is_some(),
            "instance type {instance_type:?}: Amazon EC2 must describe the bare metal \
             instance an Amazon EVS host runs on, or KEY_COVERAGE cannot be decided"
        );
        let cores = cores(instance_type);
        let vcpus = record.map(|record| record.default_vcpus).unwrap_or_default();
        assert!(
            u64::try_from(vcpus).unwrap_or_default() >= cores,
            "instance type {instance_type:?}: expected at least one vCPU per core, got \
             {vcpus} vCPU(s) for {cores} core(s)"
        );
    }
}

#[tokio::test]
async fn an_impairment_keeps_its_onset_until_the_check_passes_again() {
    let router = emulator();
    let case = "an environment short of the minimum host count";
    let environment_id = create_with(&router, case, "impairment", json!({})).await;
    std::thread::sleep(PAST_THE_TRANSITION);

    let environment = get_environment(&router, case, &environment_id).await;
    let check = check_of(&environment, evs::CHECK_TYPE_HOST_COUNT);
    assert_eq!(
        check["result"],
        json!(evs::CHECK_FAILED),
        "case {case:?}: an environment with no host fails HOST_COUNT, got {check}"
    );
    let impaired_since = check["impairedSince"]
        .as_f64()
        .unwrap_or_else(|| panic!("case {case:?}: a failing check reports impairedSince, got {check}"));
    let created_at = environment["createdAt"]
        .as_f64()
        .unwrap_or_else(|| panic!("case {case:?}: no createdAt in {environment}"));
    assert!(
        impaired_since >= created_at,
        "case {case:?}: impairment began at {impaired_since}, before the environment \
         existed at {created_at}"
    );

    // Adding hosts does not re-date an impairment that was already there.
    for host in 0..evs::MINIMUM_HOST_COUNT - 1 {
        add_host(&router, case, &environment_id, &format!("slow-{host}"), "i4i.metal").await;
        let environment = get_environment(&router, case, &environment_id).await;
        let check = check_of(&environment, evs::CHECK_TYPE_HOST_COUNT);
        assert_eq!(
            check["result"],
            json!(evs::CHECK_FAILED),
            "case {case:?}: {} host(s) is still short of {}, got {check}",
            host + 1,
            evs::MINIMUM_HOST_COUNT
        );
        assert_eq!(
            check["impairedSince"].as_f64(),
            Some(impaired_since),
            "case {case:?}: the impairment must keep the onset it began with, got {check}"
        );
    }

    // The host that meets the minimum clears the impairment, onset and all.
    add_host(&router, case, &environment_id, "last", "i4i.metal").await;
    let environment = get_environment(&router, case, &environment_id).await;
    let check = check_of(&environment, evs::CHECK_TYPE_HOST_COUNT);
    assert_eq!(
        check["result"],
        json!(evs::CHECK_PASSED),
        "case {case:?}: {} hosts meet the minimum, got {check}",
        evs::MINIMUM_HOST_COUNT
    );
    assert!(
        check["impairedSince"].is_null(),
        "case {case:?}: a check that passes carries no impairedSince, got {check}"
    );
}

#[tokio::test]
async fn an_environment_that_is_not_created_reports_no_check_and_an_unknown_roll_up() {
    let router = emulator();
    let case = "an environment that is still being created";
    // CREATING: the create's own response, before the clock settles the environment.
    let mut request = router.create_request(Some("not-settled"));
    request["licenseInfo"] = license_info("AAAAA", "BBBBB");
    let (status, body) = call(&router, "CreateEnvironment", &request).await;
    assert_eq!(status, 200, "case {case:?}: create: got {status} {body}");
    let environment = body["environment"].clone();
    let environment_id = environment["environmentId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no environmentId in {body}"))
        .to_string();
    assert_eq!(
        environment["environmentState"],
        json!("CREATING"),
        "case {case:?}: expected state CREATING, got {}",
        environment["environmentState"]
    );
    assert_no_checks(case, "CREATING", &environment);

    // DELETING, and then DELETED: an environment on its way out reports no check either.
    std::thread::sleep(PAST_THE_TRANSITION);
    let (status, body) = call(
        &router,
        "DeleteEnvironment",
        &json!({ "environmentId": environment_id }),
    )
    .await;
    assert_eq!(status, 200, "case {case:?}: delete: got {status} {body}");
    let deleting = body["environment"].clone();
    assert_eq!(
        deleting["environmentState"],
        json!("DELETING"),
        "case {case:?}: expected DELETING, got {}",
        deleting["environmentState"]
    );
    assert_no_checks(case, "DELETING", &deleting);
    std::thread::sleep(PAST_THE_TRANSITION);
    let deleted = get_environment(&router, case, &environment_id).await;
    assert_eq!(
        deleted["environmentState"],
        json!("DELETED"),
        "case {case:?}: expected DELETED, got {}",
        deleted["environmentState"]
    );
    assert_no_checks(case, "DELETED", &deleted);
}

/// An environment that is not `CREATED` has run no check, so it reports none and rolls
/// up to `UNKNOWN`.
fn assert_no_checks(case: &str, state: &str, environment: &Value) {
    assert!(
        environment["checks"].is_null(),
        "case {case:?}: a {state} environment has run no check, got {}",
        environment["checks"]
    );
    assert_eq!(
        environment["environmentStatus"],
        json!(evs::CHECK_UNKNOWN),
        "case {case:?}: a {state} environment rolls up to UNKNOWN, got {}",
        environment["environmentStatus"]
    );
}
