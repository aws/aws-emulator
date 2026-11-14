// SPDX-License-Identifier: CC0-1.0

//! The LDAPS trajectory a client DISCOVERS by polling, driven end to end over HTTP.
//!
//! RULE: every status this service records on the way to LDAP security being in force is
//! reached by ASKING AGAIN — a client that registers a certificate securing LDAP calls and
//! polls `DescribeCertificate` until the registration leaves its transient state reads
//! `Registered`; one that then calls `EnableLDAPS` and polls `DescribeLDAPSSettings` until the
//! status leaves its transient one reads `Enabled` (never `EnableFailed`: a self-signed
//! certificate the service accepted is one it secures LDAP calls with); and one that then
//! calls `DisableLDAPS` reads `Disabled` on its very next read, because the modelled
//! `LDAPSStatus` enum carries no transient value on the way down
//! (`smithy_query(query="shape",shape="LDAPSStatus")` answers `["Enabling", "Enabled",
//! "EnableFailed", "Disabled"]`). No step of that trajectory is refused, and none of those
//! readings needs a wait of any particular length: each one is owed by the clock and
//! materialised for whoever reads next.
//!
//! The rows differ along what the rule generalises over: the directory type that serves an
//! LDAPS endpoint (Managed Microsoft AD and AD Connector), every value of the modelled
//! `LDAPSType`, the certificate the registration carries, and how many times the switch is
//! cycled — a deactivated switch is activated again the same way.
//!
//! Every case goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: DirectoryService_20150416.<Operation>` and an `awsJson1_1` body, and every
//! status asserted on is READ BACK through the operation a client discovers it by, so nothing
//! here reaches into the state struct.
//!
//! Each wait below waits for the CONDITION, re-reading until it holds or a generous deadline
//! passes, so no assertion depends on how fast the host runs.

use std::time::{Duration, Instant};

use serde_json::json;
use smithy_server_runtime::MultiServiceRouter;

use crate::ad_connector::{connect_directory, minimal_connect};
use crate::certificates::{
    certificate_pem, describe, register, STATE_REGISTERED, STATE_REGISTERING, TYPE_CLIENT_LDAPS,
};
use crate::directory_lifecycle::{describe_one, ok, service, string, STAGE_ACTIVE};
use crate::microsoft_ad::{create_microsoft_ad, minimal_microsoft_ad};

/// How long one of the waits below may take to see the reading it waits for: far longer than
/// any span this service publishes, and well inside what a client's own waiter allows, so a
/// loaded or a faster host changes nothing.
const POLL_DEADLINE: Duration = Duration::from_secs(20);

/// How often such a wait asks again.
const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// Every value of the modelled `LDAPSType` enum
/// (`smithy_query(query="shape",shape="LDAPSType")` answers `["Client"]`), which is what both
/// operations' required `Type` member admits.
const LDAPS_TYPE_CATALOG: [&str; 1] = ["Client"];

/// The values of the modelled `LDAPSStatus` enum a client's waiter watches for: the transient
/// status an activation answers in, the two terminal ones it can settle on, and the one a
/// deactivation records.
const STATUS_ENABLING: &str = "Enabling";
const STATUS_ENABLED: &str = "Enabled";
const STATUS_ENABLE_FAILED: &str = "EnableFailed";
const STATUS_DISABLED: &str = "Disabled";

/// The terminal state a registration that could not be made carries: the `RegisterFailed`
/// value of the modelled `CertificateState` enum
/// (`smithy_query(query="shape",shape="CertificateState")`), the other reading a client's
/// certificate waiter stops on.
const STATE_REGISTER_FAILED: &str = "RegisterFailed";

/// Which directory type a row drives, both of which serve an LDAPS endpoint
/// (`state::LDAPS_SUPPORTED_DIRECTORY_TYPES`).
#[derive(Debug, Clone, Copy)]
enum Kind {
    /// `CreateMicrosoftAD`: AWS Managed Microsoft AD.
    MicrosoftAd,
    /// `ConnectDirectory`: an AD Connector.
    Connector,
}

/// A directory of `kind` the client has WAITED for: created, then polled through
/// `DescribeDirectories` until it reports itself in service, which is what every LDAPS call
/// needs of it.
async fn directory_in_service(
    router: &MultiServiceRouter,
    kind: Kind,
    name: &str,
    case: &str,
) -> String {
    let directory_id = match kind {
        Kind::MicrosoftAd => {
            create_microsoft_ad(router, minimal_microsoft_ad(name), case).await
        }
        Kind::Connector => connect_directory(router, minimal_connect(name), case).await,
    };
    let started = Instant::now();
    loop {
        let described = describe_one(router, &directory_id, case).await;
        let stage = string(&described, "Stage", case);
        if stage == STAGE_ACTIVE {
            return directory_id;
        }
        assert!(
            started.elapsed() < POLL_DEADLINE,
            "case {case:?}: expected {directory_id} to report itself in service within \
             {POLL_DEADLINE:?}, last observed stage {stage:?}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// A certificate securing LDAP calls, registered with `directory_id` and then POLLED the way
/// a client's waiter does: `DescribeCertificate` until the registration leaves its transient
/// state, asserting the state it leaves it for is the one that puts it in service.
async fn certificate_registered(
    router: &MultiServiceRouter,
    directory_id: &str,
    common_name: &str,
    case: &str,
) -> String {
    let (pem, _) = certificate_pem(Some(common_name), (2030, 1, 1));
    let certificate_id = register(
        router,
        json!({
            "DirectoryId": directory_id,
            "CertificateData": pem,
            "Type": TYPE_CLIENT_LDAPS
        }),
        case,
    )
    .await;
    let started = Instant::now();
    loop {
        let certificate = describe(router, directory_id, &certificate_id, case).await;
        let state = string(&certificate, "State", case);
        if state == STATE_REGISTERED || state == STATE_REGISTER_FAILED {
            assert_eq!(
                state, STATE_REGISTERED,
                "case {case:?}: expected the registration of a certificate this service \
                 accepted to be made, got {certificate}"
            );
            return certificate_id;
        }
        assert_eq!(
            state, STATE_REGISTERING,
            "case {case:?}: expected a registration being made to report the transient \
             state, got {certificate}"
        );
        assert!(
            started.elapsed() < POLL_DEADLINE,
            "case {case:?}: expected the registration to leave {STATE_REGISTERING:?} within \
             {POLL_DEADLINE:?}, last observed {certificate}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// The `LDAPSStatus` reported for `directory_id`'s `ldaps_type`, read back through
/// `DescribeLDAPSSettings`; `None` when this service holds no setting for it at all.
async fn reported_status(
    router: &MultiServiceRouter,
    directory_id: &str,
    ldaps_type: &str,
    case: &str,
) -> Option<String> {
    let answer = ok(
        router,
        "DescribeLDAPSSettings",
        json!({ "DirectoryId": directory_id, "Type": ldaps_type }),
        case,
    )
    .await;
    answer["LDAPSSettingsInfo"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected LDAPSSettingsInfo, got {answer}"))
        .first()
        .and_then(|setting| setting["LDAPSStatus"].as_str().map(str::to_string))
}

/// The status a client's waiter stops on: re-read until the reported status is one of
/// `terminal`, and report which one it was.
async fn status_settling_on(
    router: &MultiServiceRouter,
    directory_id: &str,
    ldaps_type: &str,
    terminal: &[&str],
    case: &str,
) -> String {
    let started = Instant::now();
    loop {
        let reported = reported_status(router, directory_id, ldaps_type, case).await;
        if let Some(status) = reported
            .clone()
            .filter(|status| terminal.contains(&status.as_str()))
        {
            return status;
        }
        assert!(
            started.elapsed() < POLL_DEADLINE,
            "case {case:?}: expected one of {terminal:?} within {POLL_DEADLINE:?}, last \
             observed {reported:?}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// RULE: the documented LDAPS trajectory — register and wait for the certificate, activate
/// and wait for the switch, deactivate and read it back — reaches every one of its readings
/// for a client that polls, for every directory type that serves an LDAPS endpoint and every
/// `LDAPSType`, and runs the same way the second time round.
#[tokio::test]
async fn the_documented_ldaps_trajectory_converges_for_a_client_that_polls() {
    let router = service();

    // The rows differ in the directory type that serves the LDAPS endpoint and in the
    // `LDAPSType` the switch is carried per.
    let rows: [(&str, Kind); 2] = [
        ("a Managed Microsoft AD", Kind::MicrosoftAd),
        ("an AD Connector", Kind::Connector),
    ];

    for (index, (row, kind)) in rows.iter().enumerate() {
        for ldaps_type in LDAPS_TYPE_CATALOG {
            let case = &format!("{row} with Type {ldaps_type:?}");
            let directory_id = directory_in_service(
                &router,
                *kind,
                &format!("trajectory{index}.example.com"),
                case,
            )
            .await;

            // The baseline a client asserts before it enables anything: nothing has
            // activated LDAP security for this directory, so no setting of it is active.
            let baseline = reported_status(&router, &directory_id, ldaps_type, case).await;
            assert!(
                baseline.is_none(),
                "case {case:?}: expected no LDAPS setting at all before the activation, got \
                 {baseline:?}"
            );

            // The certificate the switch is served by, waited for the way a client waits.
            certificate_registered(&router, &directory_id, "ldaps.example.com", case).await;

            // Two cycles: a switch that has been turned off is turned on again the same
            // way, so neither reading is a one-shot.
            for cycle in 1..=2 {
                let case = &format!("{case}, cycle {cycle}");
                ok(
                    &router,
                    "EnableLDAPS",
                    json!({ "DirectoryId": &directory_id, "Type": ldaps_type }),
                    case,
                )
                .await;
                let settled = status_settling_on(
                    &router,
                    &directory_id,
                    ldaps_type,
                    &[STATUS_ENABLED, STATUS_ENABLE_FAILED],
                    case,
                )
                .await;
                assert_eq!(
                    settled, STATUS_ENABLED,
                    "case {case:?}: expected the activation to put LDAP secure calls in \
                     service, got {settled:?}"
                );

                ok(
                    &router,
                    "DisableLDAPS",
                    json!({ "DirectoryId": &directory_id, "Type": ldaps_type }),
                    case,
                )
                .await;
                // No wait at all on the way down: the modelled status enum carries no
                // transient value for it, so the client's very next read is the terminal
                // one.
                let after_disable =
                    reported_status(&router, &directory_id, ldaps_type, case).await;
                assert_eq!(
                    after_disable.as_deref(),
                    Some(STATUS_DISABLED),
                    "case {case:?}: expected the deactivation to be in force on the next \
                     read, got {after_disable:?}"
                );
                // ...and it stays there: the clock owes a deactivated switch nothing, which
                // is what a client that keeps polling for `Disabled` reads.
                let still = status_settling_on(
                    &router,
                    &directory_id,
                    ldaps_type,
                    &[STATUS_DISABLED],
                    case,
                )
                .await;
                assert_eq!(
                    still, STATUS_DISABLED,
                    "case {case:?}: expected the deactivated switch to stay put, got \
                     {still:?}"
                );
            }

            // The transient status the activation answers in is still observable to a
            // caller that reads straight after it — the trajectory converges without that
            // state having been taken away. Read immediately, which is inside the span the
            // activation owes its transition over (`crate::state::TRANSITION_SPAN`, the one
            // constant this assertion depends on): a shorter span would have to move this
            // assertion rather than loosen it.
            ok(
                &router,
                "EnableLDAPS",
                json!({ "DirectoryId": &directory_id, "Type": ldaps_type }),
                case,
            )
            .await;
            let straight_after = reported_status(&router, &directory_id, ldaps_type, case).await;
            assert_eq!(
                straight_after.as_deref(),
                Some(STATUS_ENABLING),
                "case {case:?}: expected the transient status right after the activation, \
                 got {straight_after:?}"
            );
        }
    }
}
