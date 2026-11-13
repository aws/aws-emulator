// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `RollbackServiceSoftwareUpdate`.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a
//! client sees. The harness and the baseline `CreateDomain` body come from
//! [`super::domain_lifecycle`]; this suite adds no second copy of them.
//!
//! The rule under test: a service software rollback is available on a domain ONLY
//! while a landed deployment has left an earlier release to return to, the rollback
//! window of that deployment is still open and no deployment is in flight; a call
//! inside that window starts a deployment back onto the earlier release, and a call
//! outside it declines with `RollbackAvailable` false, deploying nothing. So the
//! assertions span a SET of inputs — three different release pairs and window
//! remainders on the available side, and on the unavailable side a domain no
//! deployment has landed on, a window closed at the instant it was set and one closed
//! by twice its own length, a record with a window but no release behind it, and a
//! rollback that has itself just landed. A domain with a change already in progress
//! is a REFUSAL rather than a decline, which `super::domain_activity_gates` asserts.
//!
//! The precondition a landed deployment leaves is written into the store directly
//! (through [`super::domain_lifecycle::Emulator::store`]), so each row starts from
//! exactly the record it is about. Everything the rule is asserted ON is read back
//! through the wire — the rollback's own answer and `DescribeDomain`.

use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_opensearch_amazonopensearchservice::state::{
    ServiceSoftware, NO_ROLLBACK_TARGET_DESCRIPTION, ROLLBACK_WINDOW,
};

use super::domain_lifecycle::{
    create_baseline, error_code, Answer, Emulator, BASELINE_SERVICE_SOFTWARE, PREFIX,
    RECORDED_NOT_FOUND_STATUS, SETTLE_WAIT,
};

/// `POST /2021-01-01/opensearch/serviceSoftwareUpdate/rollback`.
async fn rollback(emulator: &Emulator, name: &str) -> Answer {
    emulator
        .call(
            "POST",
            &format!("{PREFIX}/opensearch/serviceSoftwareUpdate/rollback"),
            Some(json!({ "DomainName": name })),
        )
        .await
}

/// The `ServiceSoftwareOptions` `DescribeDomain` reports for `name`.
async fn reported_software(emulator: &Emulator, name: &str) -> Value {
    let answer = emulator.describe(name).await;
    assert_eq!(
        answer.status, 200,
        "case DescribeDomain({name:?}): expected 200, got {} with {:?}",
        answer.status, answer.body
    );
    answer.body["DomainStatus"]["ServiceSoftwareOptions"].clone()
}

/// The `DomainStatus` `DescribeDomain` reports for `name`.
async fn reported_status(emulator: &Emulator, name: &str) -> Value {
    let answer = emulator.describe(name).await;
    assert_eq!(
        answer.status, 200,
        "case DescribeDomain({name:?}): expected 200, got {} with {:?}",
        answer.status, answer.body
    );
    answer.body["DomainStatus"].clone()
}

/// Put `name`'s service software record into the state a deployment would have left.
fn seed(emulator: &Emulator, name: &str, software: ServiceSoftware) {
    let store = emulator.store();
    let mut guard = store.lock();
    let domain = guard
        .domains
        .get_mut(name)
        .unwrap_or_else(|| panic!("case seed({name:?}): the domain must have been created"));
    domain.service_software = software;
}

/// The record a landed UPDATE leaves: the domain runs `current`, having come off
/// `came_off`, whose rollback window closes `remaining` from now.
fn after_update(current: &str, came_off: &str, remaining: Duration) -> ServiceSoftware {
    ServiceSoftware {
        current_version: current.to_string(),
        available_version: None,
        deployment: None,
        pending_update: None,
        rollback_to: Some(came_off.to_string()),
        rollback_until: Some(SystemTime::now() + remaining),
        optional_deployment: true,
    }
}

/// `CreateDomain` for `name` and then wait out the provisioning span, so the only
/// thing in flight on the record afterwards is whatever a case puts there.
async fn settled_domain(emulator: &Emulator, name: &str) {
    create_baseline(emulator, name).await;
    tokio::time::sleep(SETTLE_WAIT).await;
}

#[tokio::test]
async fn a_rollback_inside_the_window_returns_the_domain_to_the_release_it_came_off() {
    let emulator = Emulator::new();
    // Rows differ along what the rule generalises over: which releases the landed
    // deployment ran between (plain label, patched label, older label) and how much of
    // the rollback window is left (a minute, an hour, all of it).
    let rows: Vec<(&str, &str, &str, Duration)> = vec![
        (
            "rollback-minute",
            "R20250101",
            "R20241001",
            Duration::from_secs(60),
        ),
        (
            "rollback-hour",
            "R20250601-P2",
            "R20250101",
            Duration::from_secs(60 * 60),
        ),
        ("rollback-full", "R20240101", "R20231201", ROLLBACK_WINDOW),
    ];

    for (name, current, came_off, remaining) in rows {
        settled_domain(&emulator, name).await;
        seed(&emulator, name, after_update(current, came_off, remaining));

        let answer = rollback(&emulator, name).await;
        assert_eq!(
            answer.status, 200,
            "case RollbackServiceSoftwareUpdate({name:?}, {current} <- {came_off}): expected 200, \
             got {} with {:?}",
            answer.status, answer.body
        );
        let options = answer.body["RollbackServiceSoftwareOptions"].clone();
        assert_eq!(
            (
                options["RollbackAvailable"].as_bool(),
                options["CurrentVersion"].as_str(),
                options["NewVersion"].as_str(),
            ),
            (Some(true), Some(current), Some(came_off)),
            "case RollbackServiceSoftwareUpdate({name:?}): expected the rollback to report the \
             release it runs and the one it returns to, got {options:?}"
        );
        assert!(
            options["Description"]
                .as_str()
                .is_some_and(|text| !text.is_empty()),
            "case RollbackServiceSoftwareUpdate({name:?}): expected a description of the rollback \
             status, got {options:?}"
        );

        // The deployment the rollback started is in flight: the domain still runs the
        // release it was on, and reports the one it is moving to.
        let status = reported_status(&emulator, name).await;
        let software = status["ServiceSoftwareOptions"].clone();
        assert_eq!(
            (
                software["CurrentVersion"].as_str(),
                software["NewVersion"].as_str(),
                software["UpdateStatus"].as_str(),
                software["UpdateAvailable"].as_bool(),
            ),
            (
                Some(current),
                Some(came_off),
                Some("IN_PROGRESS"),
                Some(false)
            ),
            "case DescribeDomain({name:?}, rolling back): expected the rollback in flight, got \
             {software:?}"
        );
        assert_eq!(
            (
                status["Processing"].as_bool(),
                status["DomainProcessingStatus"].as_str(),
            ),
            (Some(true), Some("UpdatingServiceSoftware")),
            "case DescribeDomain({name:?}, rolling back): a domain applying a service software \
             deployment is processing it, got {status:?}"
        );

        // A read does not move the record: the second answer is the first.
        let again = reported_software(&emulator, name).await;
        assert_eq!(
            again, software,
            "case DescribeDomain({name:?}): a read must not change the service software record: \
             {software:?} then {again:?}"
        );

        // The clock, not the reader, lands the rollback.
        tokio::time::sleep(SETTLE_WAIT).await;
        let status = reported_status(&emulator, name).await;
        let software = status["ServiceSoftwareOptions"].clone();
        assert_eq!(
            (
                software["CurrentVersion"].as_str(),
                software["NewVersion"].as_str(),
                software["UpdateStatus"].as_str(),
                software["UpdateAvailable"].as_bool(),
            ),
            (
                Some(came_off),
                Some(current),
                Some("ELIGIBLE"),
                Some(true)
            ),
            "case DescribeDomain({name:?}, rolled back): the domain now runs the earlier release, \
             with the one it left available again, got {software:?}"
        );
        assert_eq!(
            (
                status["Processing"].as_bool(),
                status["DomainProcessingStatus"].as_str(),
            ),
            (Some(false), Some("Active")),
            "case DescribeDomain({name:?}, rolled back): nothing is deploying any more, got \
             {status:?}"
        );

        // A landed rollback opens no window to roll forward again.
        let answer = rollback(&emulator, name).await;
        assert_eq!(
            answer.status, 200,
            "case RollbackServiceSoftwareUpdate({name:?}, twice): expected 200, got {} with {:?}",
            answer.status, answer.body
        );
        let options = answer.body["RollbackServiceSoftwareOptions"].clone();
        assert_eq!(
            (
                options["RollbackAvailable"].as_bool(),
                options["CurrentVersion"].as_str(),
                options["NewVersion"].as_str(),
            ),
            (Some(false), Some(came_off), None),
            "case RollbackServiceSoftwareUpdate({name:?}, twice): a rollback cannot itself be \
             rolled back, got {options:?}"
        );
    }
}

#[tokio::test]
async fn a_rollback_the_record_does_not_allow_is_declined_and_deploys_nothing() {
    let emulator = Emulator::new();
    // One row per way a domain can fail the rule, each with the record it is put in
    // and the release it is left running.
    let rows: Vec<(&str, Option<ServiceSoftware>, &str)> = vec![
        // A domain no deployment has ever landed on: as `CreateDomain` left it.
        ("never-deployed", None, BASELINE_SERVICE_SOFTWARE),
        // A window that shuts at the instant it was set, so it is already closed when
        // the call arrives — the narrowest miss.
        (
            "window-just-closed",
            Some(after_update(
                "R20250101",
                "R20241001",
                Duration::from_secs(0),
            )),
            "R20250101",
        ),
        // And closed long ago: twice the window's own length.
        (
            "window-long-closed",
            Some(ServiceSoftware {
                rollback_until: Some(SystemTime::now() - ROLLBACK_WINDOW),
                ..after_update("R20250101", "R20241001", Duration::from_secs(0))
            }),
            "R20250101",
        ),
        // An open window but no release behind it: there is nothing to return to.
        (
            "nothing-to-return-to",
            Some(ServiceSoftware {
                rollback_to: None,
                ..after_update("R20250101", "R20241001", Duration::from_secs(60 * 60))
            }),
            "R20250101",
        ),
        // A deployment already in flight is NOT one of these rows: that domain has a
        // change in progress, which is refused rather than declined — see
        // `super::domain_activity_gates`.
    ];

    for (name, software, current) in rows {
        settled_domain(&emulator, name).await;
        if let Some(software) = software {
            seed(&emulator, name, software);
        }
        let before = reported_software(&emulator, name).await;

        let answer = rollback(&emulator, name).await;
        assert_eq!(
            answer.status, 200,
            "case RollbackServiceSoftwareUpdate({name:?}): expected 200, got {} with {:?}",
            answer.status, answer.body
        );
        let options = answer.body["RollbackServiceSoftwareOptions"].clone();
        assert_eq!(
            (
                options["RollbackAvailable"].as_bool(),
                options["CurrentVersion"].as_str(),
                options["NewVersion"].as_str(),
            ),
            (Some(false), Some(current), None),
            "case RollbackServiceSoftwareUpdate({name:?}): expected a decline naming the release \
             the domain runs and no release to return to, got {options:?}"
        );
        assert!(
            options["Description"]
                .as_str()
                .is_some_and(|text| !text.is_empty()),
            "case RollbackServiceSoftwareUpdate({name:?}): expected a description of the rollback \
             status, got {options:?}"
        );

        // A declined rollback deploys nothing: the record is exactly as it was.
        let after = reported_software(&emulator, name).await;
        assert_eq!(
            after, before,
            "case RollbackServiceSoftwareUpdate({name:?}): a declined rollback must leave the \
             record alone: {before:?} then {after:?}"
        );
    }

    // And the record a fresh domain carries, which the first row was declined on: it
    // runs the current release with nothing deploying and nothing to cancel.
    let fresh = reported_software(&emulator, "never-deployed").await;
    assert_eq!(
        (
            fresh["CurrentVersion"].as_str(),
            fresh["UpdateStatus"].as_str(),
            fresh["UpdateAvailable"].as_bool(),
            fresh["Cancellable"].as_bool(),
        ),
        (
            Some(BASELINE_SERVICE_SOFTWARE),
            Some("COMPLETED"),
            Some(false),
            Some(false),
        ),
        "case DescribeDomain(never-deployed): expected a settled service software record, got \
         {fresh:?}"
    );
}

#[tokio::test]
async fn a_rollback_on_a_domain_that_does_not_exist_is_the_modelled_not_found() {
    let emulator = Emulator::new();
    // Rows: a name no domain was ever created under, and one whose domain was deleted
    // while it was inside its rollback window.
    settled_domain(&emulator, "deleted-domain").await;
    seed(
        &emulator,
        "deleted-domain",
        after_update("R20250101", "R20241001", Duration::from_secs(60 * 60)),
    );
    let deleted = emulator
        .call(
            "DELETE",
            &format!("{PREFIX}/opensearch/domain/deleted-domain"),
            None,
        )
        .await;
    assert_eq!(
        deleted.status, 200,
        "case DeleteDomain(deleted-domain): expected 200, got {} with {:?}",
        deleted.status, deleted.body
    );

    for name in ["absent-domain", "deleted-domain"] {
        let answer = rollback(&emulator, name).await;
        assert_eq!(
            answer.status, RECORDED_NOT_FOUND_STATUS,
            "case RollbackServiceSoftwareUpdate({name:?}): expected \
             {RECORDED_NOT_FOUND_STATUS}, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            error_code(&answer),
            "ResourceNotFoundException",
            "case RollbackServiceSoftwareUpdate({name:?}): expected ResourceNotFoundException, \
             got {:?}",
            answer.body
        );
    }
}

#[tokio::test]
async fn a_domain_with_no_release_to_return_to_is_declined_with_the_services_own_reason() {
    let emulator = Emulator::new();

    // `RollbackServiceSoftwareOptions$Description` is "a description of the rollback
    // status": it is the only machine-readable reason a decline carries, so the rule is
    // that it names WHY the rollback was declined. Rows one to four are the ways a
    // domain can have no earlier release to return to, which all state the service's
    // one sentence for that reason; rows five and six fail the rule for a DIFFERENT
    // reason and must not state it.
    let rows: Vec<(&str, Option<ServiceSoftware>, bool)> = vec![
        // A domain no deployment has ever landed on: as `CreateDomain` left it.
        ("reason-never-deployed", None, true),
        // An open window with nothing behind it.
        (
            "reason-no-target",
            Some(ServiceSoftware {
                rollback_to: None,
                ..after_update("R20250101", "R20241001", Duration::from_secs(60 * 60))
            }),
            true,
        ),
        // A closed window with nothing behind it either.
        (
            "reason-no-target-closed",
            Some(ServiceSoftware {
                rollback_to: None,
                rollback_until: None,
                ..after_update("R20250101", "R20241001", Duration::from_secs(0))
            }),
            true,
        ),
        // A rollback that has itself landed: rolling back does not open a window to
        // roll forward again, so there is no release to return to.
        (
            "reason-rolled-back",
            Some(ServiceSoftware {
                available_version: Some("R20250101".to_string()),
                rollback_to: None,
                rollback_until: None,
                ..after_update("R20241001", "R20250101", Duration::from_secs(0))
            }),
            true,
        ),
        // A release to return to, but its window has closed: a different reason.
        (
            "reason-window-closed",
            Some(after_update("R20250101", "R20241001", Duration::from_secs(0))),
            false,
        ),
        // A deployment already in flight is NOT one of these rows either: that domain
        // has a change in progress, which is refused rather than declined — see
        // `super::domain_activity_gates`.
    ];

    for (name, software, no_release_to_return_to) in rows {
        settled_domain(&emulator, name).await;
        if let Some(software) = software {
            seed(&emulator, name, software);
        }
        let answer = rollback(&emulator, name).await;
        assert_eq!(
            answer.status, 200,
            "case RollbackServiceSoftwareUpdate({name:?}): a decline is answered, not refused: \
             got {} with {:?}",
            answer.status, answer.body
        );
        let options = answer.body["RollbackServiceSoftwareOptions"].clone();
        assert_eq!(
            options["RollbackAvailable"].as_bool(),
            Some(false),
            "case RollbackServiceSoftwareUpdate({name:?}): expected the decline, got {options:?}"
        );
        let stated = options["Description"].as_str().unwrap_or_default();
        if no_release_to_return_to {
            assert_eq!(
                stated, NO_ROLLBACK_TARGET_DESCRIPTION,
                "case RollbackServiceSoftwareUpdate({name:?}): expected \
                 {NO_ROLLBACK_TARGET_DESCRIPTION:?}, got {stated:?}"
            );
        } else {
            assert_ne!(
                stated, NO_ROLLBACK_TARGET_DESCRIPTION,
                "case RollbackServiceSoftwareUpdate({name:?}): a domain declined for another \
                 reason must not state the no-previous-version one, got {stated:?}"
            );
            assert!(
                !stated.is_empty(),
                "case RollbackServiceSoftwareUpdate({name:?}): expected a description of the \
                 rollback status, got {options:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_domain_name_outside_its_constraint_never_reaches_the_record() {
    let emulator = Emulator::new();
    // `DomainName` is `@length(min: 3, max: 28)` with `@pattern("^[a-z][a-z0-9\-]+$")`:
    // one row past each published bound, and one per character class the pattern
    // refuses.
    let too_long = format!("a{}", "b".repeat(28));
    for name in [
        // Two characters: one short of the minimum.
        "ab",
        // 29 characters: one past the maximum.
        too_long.as_str(),
        // Upper case, which the pattern does not allow.
        "Abc",
        // A leading digit, where the pattern requires a letter.
        "1abc",
        // An underscore, which is in neither character class.
        "a_bc",
    ] {
        let answer = rollback(&emulator, name).await;
        assert_eq!(
            answer.status, 400,
            "case RollbackServiceSoftwareUpdate({name:?}): expected 400, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            error_code(&answer),
            "ValidationException",
            "case RollbackServiceSoftwareUpdate({name:?}): expected ValidationException, got {:?}",
            answer.body
        );
    }

    // The bounds themselves reach the handler, which rolls the domain they name back:
    // three characters, and the full 28.
    let longest = format!("a{}", "b".repeat(27));
    for name in ["abc", longest.as_str()] {
        settled_domain(&emulator, name).await;
        seed(
            &emulator,
            name,
            after_update("R20250101", "R20241001", Duration::from_secs(60 * 60)),
        );
        let answer = rollback(&emulator, name).await;
        assert_eq!(
            answer.status, 200,
            "case RollbackServiceSoftwareUpdate({name:?}): a name at its bound reaches the \
             record, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.body["RollbackServiceSoftwareOptions"]["NewVersion"].as_str(),
            Some("R20241001"),
            "case RollbackServiceSoftwareUpdate({name:?}): expected the release this domain \
             returns to, got {:?}",
            answer.body
        );
    }
}
