// SPDX-License-Identifier: CC0-1.0

//! When an environment health check's impairment is dated from, end to end over
//! awsJson1_0.
//!
//! THE RULE under test: `Check$impairedSince` is "the time when environment health began
//! to be impaired" (`smithy_query(query="shape", shape="Check")`), so a FAILED check is
//! dated from the latest instant a fact THAT check reads was changed — including a fact
//! that is not part of the environment's own record. A check cannot have begun failing
//! before the fact that makes it fail was written.
//!
//! Two of the checks this service decides read exactly such a fact
//! (`smithy_query(query="shape", shape="CheckType")`):
//!
//! * `KEY_COVERAGE` — "Verifies that the VCF license key allocates sufficient vCPU cores
//!   for all deployed hosts": the cores to allocate are the account's own
//!   `vcfPortedCoreCount` setting ("The total number of VCF license cores ported to
//!   Amazon EVS for the account in that Region", `AccountSetting$name` doc), which lives
//!   in the account's settings and in no environment. So an impairment a
//!   `PutAccountSettings` caused is dated from that put, not from the environment's own
//!   last modification.
//! * `KEY_REUSE` — "Verifies that the VCF license key is not used by another Amazon EVS
//!   environment": the other environments' license keys. So an impairment another
//!   environment caused by taking the same key is dated from the instant that
//!   environment took it, which is after the impaired environment's own stamp whenever it
//!   was created later.
//!
//! The rows of the tables differ along what the rule generalises over: for the account
//! setting, the core count written, the instance type the hosts run on (and so the cores
//! the licenses have to cover), and whether a passing count was stated before the
//! impairing one; for the shared key, which member of `licenseInfo` carries the key the
//! two environments share.
//!
//! The suites that own the neighbouring rules: `environment_health_checks.rs` owns which
//! checks a settled environment reports and that an onset does not move while the check
//! keeps failing, and `environment_check_input_recompute.rs` owns that a call changing a
//! check input recomputes the results.
//!
//! The harness (`emulator`, `call`, `error_type`) is the one `environment_lifecycle`
//! owns, so every test drives the service the way a client does.

use std::time::Duration;

use serde_json::{json, Value};

use service_com_amazonaws_evs_amazonelasticvmwareservice::state as evs;

use crate::environment_check_input_recompute::{cores, get_environment, INSTANCE_TYPE};
use crate::environment_health_checks::check_of;
use crate::environment_lifecycle::{call, emulator, error_type, Emulator, PAST_THE_TRANSITION};
use crate::host_instance_type_catalogue::vcf_version_providing;

/// The wall-clock gap a test leaves between the environment's last modification and the
/// change that impairs one of its checks, so the two instants are far enough apart for
/// the onset to say which of them it was dated from.
const GAP: Duration = Duration::from_millis(250);

/// One VCF license key of the modelled shape
/// (`^[a-zA-Z0-9]{5}-[a-zA-Z0-9]{5}-[a-zA-Z0-9]{5}-[a-zA-Z0-9]{5}-[a-zA-Z0-9]{5}$`), built
/// from `seed` so two environments can ask for the same key or for different ones.
fn license_key(seed: &str) -> String {
    let group: String = format!("{seed}XXXXX").chars().take(5).collect();
    vec![group; 5].join("-")
}

/// A `licenseInfo` list (modelled `@length(1..1)`) carrying one solution key and one vSAN
/// key, each built from its own seed.
fn license_info(solution: &str, vsan: &str) -> Value {
    json!([{ "solutionKey": license_key(solution), "vsanKey": license_key(vsan) }])
}

/// Create an environment with `extra` merged into the request every rule accepts, and
/// report its id beside the window of epoch milliseconds the call ran in — the window the
/// environment took its license keys in.
async fn create_environment(
    router: &Emulator,
    case: &str,
    name: &str,
    extra: Value,
) -> (String, (i64, i64)) {
    let mut request = router.create_request(Some(name));
    for (member, value) in extra.as_object().expect("the extra members are an object") {
        request[member] = value.clone();
    }
    let before_ms = evs::now_ms();
    let (status, body) = call(router, "CreateEnvironment", &request).await;
    let after_ms = evs::now_ms();
    assert_eq!(
        status, 200,
        "case {case:?}: create {name}: expected HTTP 200, got {status} ({}) {body}",
        error_type(&body)
    );
    let id = body["environment"]["environmentId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: create {name}: no environmentId in {body}"))
        .to_string();
    (id, (before_ms, after_ms))
}

/// The same, waiting out the transition the clock owes the new environment with REAL
/// time: these tests assert the instants themselves, so they cannot settle the store from
/// a future instant the way [`crate::environment_lifecycle::settle`] does.
async fn create_settled_in_real_time(
    router: &Emulator,
    case: &str,
    name: &str,
    extra: Value,
) -> String {
    let (id, _) = create_environment(router, case, name, extra).await;
    std::thread::sleep(PAST_THE_TRANSITION);
    id
}

/// Four hosts of one instance type, named after the environment that holds them: "DNS
/// hostnames for hosts must be unique across Amazon EVS environments"
/// (`HostInfoForCreate$hostName` doc).
fn hosts_json(name: &str, instance_type: &str) -> Value {
    (0..evs::MINIMUM_HOST_COUNT)
        .map(|ordinal| {
            json!({
                "hostName": format!("{name}-{ordinal}"),
                "keyName": "evs-key",
                "instanceType": instance_type,
            })
        })
        .collect()
}

/// Write the account's ported VCF license core count for the Region the tests address,
/// and report the window of epoch milliseconds the write happened in.
async fn put_ported_cores(router: &Emulator, case: &str, ported: u64) -> (i64, i64) {
    let before_ms = evs::now_ms();
    let (status, body) = call(
        router,
        "PutAccountSettings",
        &json!({
            "settings": [{ "name": evs::VCF_PORTED_CORE_COUNT, "value": ported.to_string() }],
        }),
    )
    .await;
    let after_ms = evs::now_ms();
    assert_eq!(
        status, 200,
        "case {case:?}: put {} = {ported}: expected HTTP 200, got {status} ({}) {body}",
        evs::VCF_PORTED_CORE_COUNT,
        error_type(&body)
    );
    (before_ms, after_ms)
}

/// A reported `Timestamp` as the epoch milliseconds this service stores: the wire form is
/// epoch seconds with a millisecond fraction.
fn epoch_ms(case: &str, what: &str, reported: &Value) -> i64 {
    let seconds = reported
        .as_f64()
        .unwrap_or_else(|| panic!("case {case:?}: {what} is not a timestamp, got {reported}"));
    (seconds * 1000.0).round() as i64
}

/// One check's `result`, for an assertion message.
fn result_of(environment: &Value, check_type: &str) -> String {
    check_of(environment, check_type)["result"]
        .as_str()
        .unwrap_or("<no check>")
        .to_string()
}

/// Assert that the FAILED check `check_type` of `environment` is dated from the window
/// `[changed_from_ms, changed_to_ms]` the fact that impairs it changed in, and NOT from
/// the environment's own last modification `modified_at_ms`.
fn assert_onset_dates_from(
    case: &str,
    environment: &Value,
    check_type: &str,
    (changed_from_ms, changed_to_ms): (i64, i64),
    modified_at_ms: i64,
) -> i64 {
    let check = check_of(environment, check_type);
    assert_eq!(
        check["result"],
        json!(evs::CHECK_FAILED),
        "case {case:?}: expected {check_type} {}, got {} in {check}",
        evs::CHECK_FAILED,
        result_of(environment, check_type)
    );
    let onset_ms = epoch_ms(case, "impairedSince", &check["impairedSince"]);
    assert!(
        onset_ms >= changed_from_ms && onset_ms <= changed_to_ms,
        "case {case:?}: {check_type} began to be impaired when the fact it reads changed, \
         within [{changed_from_ms}, {changed_to_ms}], got {onset_ms} ({check})"
    );
    assert!(
        onset_ms > modified_at_ms,
        "case {case:?}: {check_type} was dated from the environment's own last \
         modification {modified_at_ms} rather than from the change at \
         {changed_from_ms}, got {onset_ms} ({check})"
    );
    onset_ms
}

#[tokio::test]
async fn key_coverage_is_impaired_from_the_instant_the_account_stated_its_ported_cores() {
    // Rows: the instance type the four hosts run on, the ported core count stated BEFORE
    // the impairing one (as a delta from the cores those hosts need, or nothing stated at
    // all), and the impairing count as a delta from those cores.
    // A delta is from the cores the hosts need; the impairing count is floored at one
    // core, so the last row states a count far below whatever those hosts need.
    let rows: Vec<(&str, &str, Option<i64>, i64)> = vec![
        ("no count was stated before it", INSTANCE_TYPE, None, -1),
        (
            "a count that covered the hosts was stated first",
            INSTANCE_TYPE,
            Some(0),
            -1,
        ),
        (
            "far more than the hosts need was stated first",
            "i7i.metal-24xl",
            Some(64),
            -32,
        ),
        (
            "the count was cut to a single core",
            "i7i.metal-48xl",
            Some(0),
            -1_000_000,
        ),
    ];

    for (index, (case, instance_type, stated_first, impairing_delta)) in
        rows.into_iter().enumerate()
    {
        // One emulator per row: the ported core count is an account-wide setting, so a
        // row must not read the value another row wrote.
        let router = emulator();
        let name = format!("onset-coverage-{index}");
        let required = cores(instance_type) * u64::try_from(evs::MINIMUM_HOST_COUNT).unwrap();
        let environment_id = create_settled_in_real_time(
            &router,
            case,
            &name,
            json!({
                "hosts": hosts_json(&name, instance_type),
                "licenseInfo": license_info("SOLAX", "VSANA"),
                // A host is built only on hardware its environment's VCF version is
                // provided on ("EC2 instance types provided by Amazon EVS for this VCF
                // version for creating environments", `VcfVersionInfo$instanceTypes`), so
                // the environment of a row runs a version the catalogue provides that
                // row's instance type on —
                // `tests/all/host_instance_type_catalogue.rs` owns that rule.
                "vcfVersion": vcf_version_providing(instance_type),
            }),
        )
        .await;

        // The result the row starts from: UNKNOWN while the account has stated no count,
        // and PASSED where the count stated first covers the hosts.
        let expected_before = if let Some(delta) = stated_first {
            let ported = u64::try_from(required as i64 + delta).expect("a positive core count");
            put_ported_cores(&router, case, ported).await;
            evs::CHECK_PASSED
        } else {
            evs::CHECK_UNKNOWN
        };
        let before = get_environment(&router, case, &environment_id).await;
        assert_eq!(
            result_of(&before, evs::CHECK_TYPE_KEY_COVERAGE),
            expected_before,
            "case {case:?}: before the impairing put: expected {expected_before}, got {}",
            before["checks"]
        );
        let modified_at_ms = epoch_ms(case, "modifiedAt", &before["modifiedAt"]);

        // The fact the check reads changes well after the environment's own last
        // modification, so the two instants cannot be confused for each other.
        std::thread::sleep(GAP);
        let impairing = u64::try_from((required as i64 + impairing_delta).max(1))
            .expect("a positive core count");
        assert!(
            impairing < required,
            "case {case:?}: {impairing} core(s) must be short of the {required} the hosts need"
        );
        let window = put_ported_cores(&router, case, impairing).await;

        let after = get_environment(&router, case, &environment_id).await;
        let onset_ms = assert_onset_dates_from(
            case,
            &after,
            evs::CHECK_TYPE_KEY_COVERAGE,
            window,
            modified_at_ms,
        );

        // The onset is STORED, not computed per read: a second read reports the same one.
        let again = get_environment(&router, case, &environment_id).await;
        assert_eq!(
            epoch_ms(
                case,
                "impairedSince",
                &check_of(&again, evs::CHECK_TYPE_KEY_COVERAGE)["impairedSince"]
            ),
            onset_ms,
            "case {case:?}: a second read re-dated the impairment, got {}",
            again["checks"]
        );
    }
}

/// One row of the shared-key table: the seeds of the solution key and the vSAN key each
/// of the two environments carries.
struct ReuseRow {
    case: &'static str,
    first_keys: (&'static str, &'static str),
    second_keys: (&'static str, &'static str),
}

#[tokio::test]
async fn key_reuse_is_impaired_from_the_instant_another_environment_took_the_key() {
    // Rows: the keys the FIRST environment carries and the keys the second one carries —
    // which member the shared key sits in differs per row, because reuse is about the KEY
    // and not about which member carried it.
    let rows = vec![
        ReuseRow {
            case: "the other environment re-uses the solution key",
            first_keys: ("SOLAX", "VSANA"),
            second_keys: ("SOLAX", "VSANB"),
        },
        ReuseRow {
            case: "the other environment re-uses the vSAN key",
            first_keys: ("SOLAX", "VSANA"),
            second_keys: ("SOLBX", "VSANA"),
        },
        ReuseRow {
            case: "the other environment carries the solution key as its vSAN key",
            first_keys: ("SOLAX", "VSANA"),
            second_keys: ("SOLBX", "SOLAX"),
        },
        ReuseRow {
            case: "the other environment carries both keys",
            first_keys: ("SOLAX", "VSANA"),
            second_keys: ("SOLAX", "VSANA"),
        },
    ];

    for (
        index,
        ReuseRow {
            case,
            first_keys,
            second_keys,
        },
    ) in rows.into_iter().enumerate()
    {
        // One emulator per row: a license key is unique across the whole store, so a row
        // must not see the keys another row's environments carry.
        let router = emulator();
        let first = create_settled_in_real_time(
            &router,
            case,
            &format!("onset-reuse-{index}-a"),
            json!({ "licenseInfo": license_info(first_keys.0, first_keys.1) }),
        )
        .await;

        // Its key is its own until the second environment exists.
        let before = get_environment(&router, case, &first).await;
        assert_eq!(
            result_of(&before, evs::CHECK_TYPE_KEY_REUSE),
            evs::CHECK_PASSED,
            "case {case:?}: one environment's key is used once, got {}",
            before["checks"]
        );
        let modified_at_ms = epoch_ms(case, "modifiedAt", &before["modifiedAt"]);

        // The second environment takes the key well after the first was last modified: it
        // carries the key from the instant the create minted it, which is inside that
        // call's own window.
        std::thread::sleep(GAP);
        let (second, window) = create_environment(
            &router,
            case,
            &format!("onset-reuse-{index}-b"),
            json!({ "licenseInfo": license_info(second_keys.0, second_keys.1) }),
        )
        .await;

        let after = get_environment(&router, case, &first).await;
        let onset_ms = assert_onset_dates_from(
            case,
            &after,
            evs::CHECK_TYPE_KEY_REUSE,
            window,
            modified_at_ms,
        );

        // The second environment settles: reuse is mutual, so it reports the failure too,
        // and nothing it does later re-dates the first environment's impairment.
        std::thread::sleep(PAST_THE_TRANSITION);
        let other = get_environment(&router, case, &second).await;
        assert_eq!(
            result_of(&other, evs::CHECK_TYPE_KEY_REUSE),
            evs::CHECK_FAILED,
            "case {case:?}: reuse impairs both environments, got {}",
            other["checks"]
        );
        let again = get_environment(&router, case, &first).await;
        assert_eq!(
            epoch_ms(
                case,
                "impairedSince",
                &check_of(&again, evs::CHECK_TYPE_KEY_REUSE)["impairedSince"]
            ),
            onset_ms,
            "case {case:?}: the impairment must keep the onset it began with, got {}",
            again["checks"]
        );
    }
}
