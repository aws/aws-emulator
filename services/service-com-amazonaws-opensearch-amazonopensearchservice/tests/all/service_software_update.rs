// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `StartServiceSoftwareUpdate`.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a
//! client sees, not what a handler returns. The harness and the baseline
//! `CreateDomain` body come from [`super::domain_lifecycle`]; this suite adds no
//! second copy of them.
//!
//! The rule under test: a service software update deploys the release a domain's own
//! record has AVAILABLE, and is possible only while such a release is available and
//! nothing else is deploying; a request the record has nothing to deploy for cannot be
//! served as asked, so it is refused with the operation's modelled
//! `ValidationException` (HTTP 400) and deploys nothing. The operation models exactly
//! `BaseException` 400, `InternalException` 500, `ResourceNotFoundException` and
//! `ValidationException` 400
//! (`smithy_query(query="operation",operation="StartServiceSoftwareUpdate")`), so no
//! other code or status is part of its contract — the missing-resource refusal being
//! answered on the status this service carries that shape on
//! ([`RECORDED_NOT_FOUND_STATUS`]) rather than on the one the model annotates.
//!
//! The assertions span SETS of inputs: three release pairs and three forms of
//! `ScheduleAt` on the deploying side, and on the refused side a domain as
//! `CreateDomain` left it, one already deploying, one whose update has just landed, a
//! second call on a domain whose update is in flight, a domain that does not exist and
//! one that was deleted, plus every way a `DesiredStartTime` can fail to belong with
//! the `ScheduleAt` it was sent with.
//!
//! A release only becomes available to a domain once a deployment has landed leaving
//! one behind, so most rows write that precondition into the store directly (through
//! [`super::domain_lifecycle::Emulator::store`], as the sibling rollback suite does).
//! One case reaches it entirely over the wire instead: a landed rollback leaves the
//! release it came off available, and that is the release a start then deploys.

use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_opensearch_amazonopensearchservice::state::{
    PendingUpdate, ServiceSoftware, SoftwareDeployment, DOMAIN_NOT_AVAILABLE_FOR_UPDATE,
    ROLLBACK_WINDOW,
};

use super::domain_lifecycle::{
    create_baseline, error_code, Answer, Emulator, BASELINE_SERVICE_SOFTWARE, PREFIX,
    RECORDED_NOT_FOUND_STATUS, SETTLE_WAIT,
};

/// `POST /2021-01-01/opensearch/serviceSoftwareUpdate/start`, with the schedule
/// members a row sends (none for a request that sends only the domain name).
pub(crate) async fn start(emulator: &Emulator, name: &str, schedule: Value) -> Answer {
    let mut body = json!({ "DomainName": name });
    if let Some(members) = schedule.as_object() {
        for (member, value) in members {
            body[member] = value.clone();
        }
    }
    emulator
        .call(
            "POST",
            &format!("{PREFIX}/opensearch/serviceSoftwareUpdate/start"),
            Some(body),
        )
        .await
}

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

/// The `ServiceSoftwareOptions` `DescribeDomain` reports for `name`.
async fn reported_software(emulator: &Emulator, name: &str) -> Value {
    reported_status(emulator, name).await["ServiceSoftwareOptions"].clone()
}

/// `ServiceSoftwareOptions$NewVersion` for a domain with no release to move onto:
/// "the new service software version, if one is available" is the empty version rather
/// than no member at all, so a caller reading it always has a version string to
/// compare.
const NO_NEW_VERSION: &str = "";

/// The four members every assertion below reads off a `ServiceSoftwareOptions`.
fn software_summary(options: &Value) -> (Option<&str>, Option<&str>, Option<&str>, Option<bool>) {
    (
        options["CurrentVersion"].as_str(),
        options["NewVersion"].as_str(),
        options["UpdateStatus"].as_str(),
        options["UpdateAvailable"].as_bool(),
    )
}

/// Whether an answer reports the update as cancellable, which is the member
/// `CancelServiceSoftwareUpdate` keys on.
fn cancellable(options: &Value) -> Option<bool> {
    options["Cancellable"].as_bool()
}

/// The error message of a refused answer, with the case named on failure.
pub(crate) fn message(answer: &Answer, case: &str) -> String {
    answer.body["message"]
        .as_str()
        .or_else(|| answer.body["Message"].as_str())
        .unwrap_or_else(|| panic!("case {case}: expected an error message, got {:?}", answer.body))
        .to_owned()
}

/// Put `name`'s service software record into the state a case is about.
pub(crate) fn seed(emulator: &Emulator, name: &str, software: ServiceSoftware) {
    let store = emulator.store();
    let mut guard = store.lock();
    let domain = guard
        .domains
        .get_mut(name)
        .unwrap_or_else(|| panic!("case seed({name:?}): the domain must have been created"));
    domain.service_software = software;
}

/// The record a domain carries when a release is available for it to move onto: it
/// runs `current`, `available` is waiting, and nothing is deploying.
pub(crate) fn with_update_available(current: &str, available: &str) -> ServiceSoftware {
    ServiceSoftware {
        current_version: current.to_string(),
        available_version: Some(available.to_string()),
        deployment: None,
        pending_update: None,
        rollback_to: None,
        rollback_until: None,
        optional_deployment: true,
    }
}

/// `CreateDomain` for `name` and then wait out the provisioning span, so the only
/// thing in flight on the record afterwards is whatever a case puts there.
pub(crate) async fn settled_domain(emulator: &Emulator, name: &str) {
    create_baseline(emulator, name).await;
    tokio::time::sleep(SETTLE_WAIT).await;
}

/// An instant as the epoch-millisecond count a `Long` timestamp member carries.
fn epoch_millis(at: SystemTime) -> i64 {
    i64::try_from(
        at.duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis(),
    )
    .expect("an instant of this clock fits a Long")
}

#[tokio::test]
async fn the_available_release_is_deployed_and_the_clock_lands_it() {
    let emulator = Emulator::new();

    // Rows differ along what the rule generalises over: which releases the domain runs
    // and is moving onto (plain label, patched label, older label), and the form the
    // schedule takes for an update that starts at once — none sent at all ("Default:
    // NOW if you don't specify a value for DesiredStartTime"), `NOW` itself, and
    // `TIMESTAMP` with an instant that has already come.
    let now = SystemTime::now();
    let rows: Vec<(&str, &str, &str, Value)> = vec![
        ("start-default", "R20250101", "R20250601", json!({})),
        (
            "start-now",
            "R20240101",
            "R20250101-P2",
            json!({ "ScheduleAt": "NOW" }),
        ),
        (
            "start-timestamp",
            "R20231201",
            "R20240101",
            json!({
                "ScheduleAt": "TIMESTAMP",
                "DesiredStartTime": epoch_millis(now - Duration::from_secs(60)),
            }),
        ),
    ];

    for (name, current, available, schedule) in rows {
        settled_domain(&emulator, name).await;
        seed(&emulator, name, with_update_available(current, available));

        // The record says an update is available, so a client can tell the call will be
        // served before making it.
        let offered = reported_software(&emulator, name).await;
        assert_eq!(
            software_summary(&offered),
            (
                Some(current),
                Some(available),
                Some("ELIGIBLE"),
                Some(true)
            ),
            "case DescribeDomain({name:?}, before the start): expected an available release, got \
             {offered:?}"
        );

        let answer = start(&emulator, name, schedule.clone()).await;
        assert_eq!(
            answer.status, 200,
            "case StartServiceSoftwareUpdate({name:?}, {schedule}): expected 200, got {} with \
             {:?}",
            answer.status, answer.body
        );
        let options = answer.body["ServiceSoftwareOptions"].clone();
        assert_eq!(
            software_summary(&options),
            (
                Some(current),
                Some(available),
                Some("IN_PROGRESS"),
                Some(false)
            ),
            "case StartServiceSoftwareUpdate({name:?}): expected the deployment in flight onto \
             the available release, got {options:?}"
        );

        // The answer is the STORED record: `DescribeDomain` reports the same deployment,
        // and the domain is processing it.
        let status = reported_status(&emulator, name).await;
        assert_eq!(
            status["ServiceSoftwareOptions"], options,
            "case DescribeDomain({name:?}, deploying): expected the record the start answered \
             with, got {:?}",
            status["ServiceSoftwareOptions"]
        );
        assert_eq!(
            (
                status["Processing"].as_bool(),
                status["DomainProcessingStatus"].as_str(),
            ),
            (Some(true), Some("UpdatingServiceSoftware")),
            "case DescribeDomain({name:?}, deploying): a domain applying a service software \
             deployment is processing it, got {status:?}"
        );

        // The clock, not the reader, lands the deployment: afterwards the domain runs
        // the release it was moving onto, with nothing further available.
        tokio::time::sleep(SETTLE_WAIT).await;
        let status = reported_status(&emulator, name).await;
        let landed = status["ServiceSoftwareOptions"].clone();
        assert_eq!(
            software_summary(&landed),
            (
                Some(available),
                Some(NO_NEW_VERSION),
                Some("COMPLETED"),
                Some(false),
            ),
            "case DescribeDomain({name:?}, landed): expected the domain to run the release it \
             was moving onto, got {landed:?}"
        );
        assert_eq!(
            (
                status["Processing"].as_bool(),
                status["DomainProcessingStatus"].as_str(),
            ),
            (Some(false), Some("Active")),
            "case DescribeDomain({name:?}, landed): nothing is deploying any more, got {status:?}"
        );

        // What a landed update leaves behind: the release the domain came off is the one
        // a rollback inside the window returns it to.
        let answer = rollback(&emulator, name).await;
        assert_eq!(
            answer.status, 200,
            "case RollbackServiceSoftwareUpdate({name:?}): expected 200, got {} with {:?}",
            answer.status, answer.body
        );
        let returned = answer.body["RollbackServiceSoftwareOptions"].clone();
        assert_eq!(
            (
                returned["RollbackAvailable"].as_bool(),
                returned["CurrentVersion"].as_str(),
                returned["NewVersion"].as_str(),
            ),
            (Some(true), Some(available), Some(current)),
            "case RollbackServiceSoftwareUpdate({name:?}): expected the landed update to be \
             undoable back onto the release the domain came off, got {returned:?}"
        );
    }
}

#[tokio::test]
async fn an_update_scheduled_for_later_waits_for_its_instant_and_then_runs() {
    let emulator = Emulator::new();

    // "TIMESTAMP - Lets you specify a custom date and time to apply the update": the
    // instant the caller asked for is kept, so until it comes the update is requested
    // and not begun — `DeploymentStatus`'s PENDING_UPDATE, the one state
    // `ServiceSoftwareOptions$Cancellable` is true in — and the domain is not yet
    // processing it. Rows: an hour out, a day out, and one close enough to watch the
    // clock begin AND land it. Each start is taken from the instant of its own call, so
    // the span a row asks for is the span it waits.
    let rows: Vec<(&str, Duration, bool)> = vec![
        ("later-hour", Duration::from_secs(60 * 60), false),
        ("later-day", Duration::from_secs(24 * 60 * 60), false),
        ("later-soon", Duration::from_millis(1_000), true),
    ];

    for (name, ahead, _) in &rows {
        settled_domain(&emulator, name).await;
        seed(
            &emulator,
            name,
            with_update_available("R20250101", "R20250601"),
        );

        let answer = start(
            &emulator,
            name,
            json!({
                "ScheduleAt": "TIMESTAMP",
                "DesiredStartTime": epoch_millis(SystemTime::now() + *ahead),
            }),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case StartServiceSoftwareUpdate({name:?}, {ahead:?} ahead): expected 200, got {} \
             with {:?}",
            answer.status, answer.body
        );
        let options = answer.body["ServiceSoftwareOptions"].clone();
        assert_eq!(
            (software_summary(&options), cancellable(&options)),
            (
                (
                    Some("R20250101"),
                    Some("R20250601"),
                    Some("PENDING_UPDATE"),
                    Some(false)
                ),
                Some(true)
            ),
            "case StartServiceSoftwareUpdate({name:?}, {ahead:?} ahead): expected an update \
             requested and not begun, got {options:?}"
        );
        let status = reported_status(&emulator, name).await;
        assert_eq!(
            status["ServiceSoftwareOptions"], options,
            "case DescribeDomain({name:?}, {ahead:?} ahead): expected the record the start \
             answered with, got {:?}",
            status["ServiceSoftwareOptions"]
        );
        assert_eq!(
            (
                status["Processing"].as_bool(),
                status["DomainProcessingStatus"].as_str(),
            ),
            (Some(false), Some("Active")),
            "case DescribeDomain({name:?}, {ahead:?} ahead): an update that has not begun is \
             not being applied, got {status:?}"
        );
    }

    // After the provisioning span the distant rows have still not begun: the caller's
    // instant, not that span, is what they wait for. The close row's instant does come,
    // and the clock then begins its deployment and lands it, so that domain ends on the
    // release it was moving onto.
    tokio::time::sleep(SETTLE_WAIT).await;
    for (name, ahead, _) in rows.iter().filter(|(_, _, soon)| !soon) {
        let software = reported_software(&emulator, name).await;
        assert_eq!(
            software_summary(&software),
            (
                Some("R20250101"),
                Some("R20250601"),
                Some("PENDING_UPDATE"),
                Some(false)
            ),
            "case DescribeDomain({name:?}, {ahead:?} ahead): an update scheduled for later has \
             not begun yet, got {software:?}"
        );
    }

    tokio::time::sleep(Duration::from_millis(1_500)).await;
    for (name, ahead, _) in rows.iter().filter(|(_, _, soon)| *soon) {
        let landed = reported_software(&emulator, name).await;
        assert_eq!(
            (software_summary(&landed), cancellable(&landed)),
            (
                (
                    Some("R20250601"),
                    Some(NO_NEW_VERSION),
                    Some("COMPLETED"),
                    Some(false),
                ),
                Some(false)
            ),
            "case DescribeDomain({name:?}, {ahead:?} ahead): expected the scheduled update to \
             have run once its instant came, got {landed:?}"
        );
    }
}

#[tokio::test]
async fn an_off_peak_window_start_schedules_the_available_release() {
    let emulator = Emulator::new();
    let name = "start-off-peak";
    settled_domain(&emulator, name).await;
    seed(
        &emulator,
        name,
        with_update_available("R20250101", "R20250601"),
    );

    // "OFF_PEAK_WINDOW - Marks the update to be picked up during an upcoming off-peak
    // window": the schedule form is accepted and the update it records is onto the
    // release the record has available, waiting for that window rather than beginning
    // now.
    let answer = start(&emulator, name, json!({ "ScheduleAt": "OFF_PEAK_WINDOW" })).await;
    assert_eq!(
        answer.status, 200,
        "case StartServiceSoftwareUpdate({name:?}, OFF_PEAK_WINDOW): expected 200, got {} with \
         {:?}",
        answer.status, answer.body
    );
    let options = answer.body["ServiceSoftwareOptions"].clone();
    assert_eq!(
        (software_summary(&options), cancellable(&options)),
        (
            (
                Some("R20250101"),
                Some("R20250601"),
                Some("PENDING_UPDATE"),
                Some(false)
            ),
            Some(true)
        ),
        "case StartServiceSoftwareUpdate({name:?}, OFF_PEAK_WINDOW): expected the update to be \
         waiting for the window, onto the available release, got {options:?}"
    );
    assert_eq!(
        reported_software(&emulator, name).await,
        options,
        "case DescribeDomain({name:?}): expected the record the start answered with"
    );
}

#[tokio::test]
async fn a_domain_with_nothing_to_deploy_is_refused_and_keeps_its_record() {
    let emulator = Emulator::new();

    // One row per way a domain's record can have nothing to deploy: as `CreateDomain`
    // left it, with a deployment already in flight, with an update already scheduled for
    // a later instant, with a landed update behind it and nothing further available, and
    // with a landed rollback that left no release available either.
    let rows: Vec<(&str, Option<ServiceSoftware>)> = vec![
        ("no-update", None),
        (
            "already-deploying",
            Some(ServiceSoftware {
                deployment: Some(SoftwareDeployment {
                    target_version: "R20250601".to_string(),
                    completes_at: SystemTime::now() + Duration::from_secs(60 * 60),
                    rollback: false,
                }),
                ..with_update_available("R20250101", "R20250601")
            }),
        ),
        (
            "already-scheduled",
            Some(ServiceSoftware {
                available_version: None,
                pending_update: Some(PendingUpdate {
                    target_version: "R20250601".to_string(),
                    starts_at: SystemTime::now() + Duration::from_secs(60 * 60),
                }),
                ..with_update_available("R20250101", "R20250601")
            }),
        ),
        (
            "update-landed",
            Some(ServiceSoftware {
                available_version: None,
                rollback_to: Some("R20241001".to_string()),
                rollback_until: Some(SystemTime::now() + ROLLBACK_WINDOW),
                ..with_update_available("R20250101", "R20250601")
            }),
        ),
        (
            "rollback-landed",
            Some(ServiceSoftware {
                available_version: None,
                ..with_update_available("R20241001", "R20250101")
            }),
        ),
    ];

    for (name, software) in rows {
        settled_domain(&emulator, name).await;
        if let Some(software) = software {
            seed(&emulator, name, software);
        }
        let before = reported_software(&emulator, name).await;

        let answer = start(&emulator, name, json!({})).await;
        assert_eq!(
            answer.status, 400,
            "case StartServiceSoftwareUpdate({name:?}): expected HTTP 400, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            error_code(&answer),
            "ValidationException",
            "case StartServiceSoftwareUpdate({name:?}): expected the modelled validation refusal, \
             got {:?}",
            answer.body
        );
        let stated = message(&answer, name);
        assert_eq!(
            stated, DOMAIN_NOT_AVAILABLE_FOR_UPDATE,
            "case StartServiceSoftwareUpdate({name:?}): expected the service's one wording for a \
             domain that cannot take an update, got {stated:?}"
        );

        // A refused start deploys nothing: the record is exactly as it was.
        let after = reported_software(&emulator, name).await;
        assert_eq!(
            after, before,
            "case StartServiceSoftwareUpdate({name:?}): a refused start must leave the record \
             alone: {before:?} then {after:?}"
        );
    }

    // And the record a fresh domain carries, which the first row was refused on: it
    // runs the current release with nothing available to move onto.
    let fresh = reported_software(&emulator, "no-update").await;
    assert_eq!(
        software_summary(&fresh),
        (
            Some(BASELINE_SERVICE_SOFTWARE),
            Some(NO_NEW_VERSION),
            Some("COMPLETED"),
            Some(false)
        ),
        "case DescribeDomain(no-update): expected a settled service software record, got {fresh:?}"
    );
}

#[tokio::test]
async fn a_second_start_while_one_is_in_flight_is_refused() {
    let emulator = Emulator::new();
    let name = "start-twice";
    settled_domain(&emulator, name).await;
    seed(
        &emulator,
        name,
        with_update_available("R20250101", "R20250601"),
    );

    let first = start(&emulator, name, json!({})).await;
    assert_eq!(
        first.status, 200,
        "case StartServiceSoftwareUpdate({name:?}, first): expected 200, got {} with {:?}",
        first.status, first.body
    );
    let in_flight = reported_software(&emulator, name).await;

    // One deployment at a time: the release the first call took is no longer available,
    // so a second call has nothing to deploy.
    let second = start(&emulator, name, json!({})).await;
    assert_eq!(
        second.status, 400,
        "case StartServiceSoftwareUpdate({name:?}, second): expected HTTP 400, got {} with {:?}",
        second.status, second.body
    );
    assert_eq!(
        error_code(&second),
        "ValidationException",
        "case StartServiceSoftwareUpdate({name:?}, second): expected the modelled validation \
         refusal, got {:?}",
        second.body
    );
    assert_eq!(
        reported_software(&emulator, name).await,
        in_flight,
        "case StartServiceSoftwareUpdate({name:?}, second): a refused start must leave the \
         deployment in flight untouched"
    );
}

#[tokio::test]
async fn the_release_a_rollback_left_available_is_what_a_start_deploys() {
    let emulator = Emulator::new();
    let name = "rolled-then-started";
    settled_domain(&emulator, name).await;

    // The precondition a landed update leaves, so the whole chain below runs over the
    // wire: roll back, let the rollback land, and the release the domain came off is
    // available again — which is the release a start then deploys.
    seed(
        &emulator,
        name,
        ServiceSoftware {
            available_version: None,
            rollback_to: Some("R20241001".to_string()),
            rollback_until: Some(SystemTime::now() + ROLLBACK_WINDOW),
            ..with_update_available("R20250101", "R20250601")
        },
    );

    let rolled = rollback(&emulator, name).await;
    assert_eq!(
        rolled.status, 200,
        "case RollbackServiceSoftwareUpdate({name:?}): expected 200, got {} with {:?}",
        rolled.status, rolled.body
    );
    tokio::time::sleep(SETTLE_WAIT).await;
    let available = reported_software(&emulator, name).await;
    assert_eq!(
        software_summary(&available),
        (
            Some("R20241001"),
            Some("R20250101"),
            Some("ELIGIBLE"),
            Some(true)
        ),
        "case DescribeDomain({name:?}, rolled back): expected the release it came off to be \
         available again, got {available:?}"
    );

    let answer = start(&emulator, name, json!({})).await;
    assert_eq!(
        answer.status, 200,
        "case StartServiceSoftwareUpdate({name:?}): expected 200, got {} with {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        software_summary(&answer.body["ServiceSoftwareOptions"]),
        (
            Some("R20241001"),
            Some("R20250101"),
            Some("IN_PROGRESS"),
            Some(false)
        ),
        "case StartServiceSoftwareUpdate({name:?}): expected the deployment onto the release the \
         rollback left available, got {:?}",
        answer.body
    );

    tokio::time::sleep(SETTLE_WAIT).await;
    let landed = reported_software(&emulator, name).await;
    assert_eq!(
        software_summary(&landed),
        (
            Some("R20250101"),
            Some(NO_NEW_VERSION),
            Some("COMPLETED"),
            Some(false),
        ),
        "case DescribeDomain({name:?}, landed): expected the domain back on the release the \
         rollback had left, got {landed:?}"
    );
}

#[tokio::test]
async fn a_start_on_a_domain_that_does_not_exist_is_the_modelled_not_found() {
    let emulator = Emulator::new();

    // Rows: a name no domain was ever created under, and one whose domain was deleted
    // while a release was available to it.
    settled_domain(&emulator, "deleted-domain").await;
    seed(
        &emulator,
        "deleted-domain",
        with_update_available("R20250101", "R20250601"),
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
        let answer = start(&emulator, name, json!({})).await;
        assert_eq!(
            answer.status, RECORDED_NOT_FOUND_STATUS,
            "case StartServiceSoftwareUpdate({name:?}): expected {RECORDED_NOT_FOUND_STATUS}, got \
             {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            error_code(&answer),
            "ResourceNotFoundException",
            "case StartServiceSoftwareUpdate({name:?}): expected ResourceNotFoundException, got \
             {:?}",
            answer.body
        );
    }
}

#[tokio::test]
async fn a_desired_start_time_that_does_not_belong_with_its_schedule_is_refused() {
    let emulator = Emulator::new();
    let name = "schedule-pairing";
    settled_domain(&emulator, name).await;
    seed(
        &emulator,
        name,
        with_update_available("R20250101", "R20250601"),
    );
    let before = reported_software(&emulator, name).await;

    // "You only need to specify this parameter if you set ScheduleAt to TIMESTAMP", and
    // that value "must also provide a value for DesiredStartTime": one row for the
    // missing instant, and one per other `ScheduleAt` value sent with an instant.
    let at = epoch_millis(SystemTime::now() + Duration::from_secs(60 * 60));
    for schedule in [
        json!({ "ScheduleAt": "TIMESTAMP" }),
        json!({ "ScheduleAt": "NOW", "DesiredStartTime": at }),
        json!({ "ScheduleAt": "OFF_PEAK_WINDOW", "DesiredStartTime": at }),
    ] {
        let answer = start(&emulator, name, schedule.clone()).await;
        assert_eq!(
            answer.status, 400,
            "case StartServiceSoftwareUpdate({name:?}, {schedule}): expected HTTP 400, got {} \
             with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            error_code(&answer),
            "ValidationException",
            "case StartServiceSoftwareUpdate({name:?}, {schedule}): expected the modelled \
             validation refusal, got {:?}",
            answer.body
        );

        // The pairing is a property of the request alone, so it is refused before
        // anything the request names is resolved: nothing is deployed, and a name no
        // domain exists under is refused the same way rather than as not found.
        assert_eq!(
            reported_software(&emulator, name).await,
            before,
            "case StartServiceSoftwareUpdate({name:?}, {schedule}): a refused request must \
             deploy nothing"
        );
        let absent = start(&emulator, "absent-domain", schedule.clone()).await;
        assert_eq!(
            absent.status, 400,
            "case StartServiceSoftwareUpdate(absent-domain, {schedule}): expected the request to \
             be refused before the domain is resolved, got {} with {:?}",
            absent.status, absent.body
        );
    }

    // A `ScheduleAt` outside the modelled enum never reaches the record either.
    for invalid in ["LATER", "now", ""] {
        let answer = start(&emulator, name, json!({ "ScheduleAt": invalid })).await;
        assert_eq!(
            answer.status, 400,
            "case StartServiceSoftwareUpdate({name:?}, ScheduleAt={invalid:?}): expected HTTP \
             400, got {} with {:?}",
            answer.status, answer.body
        );
    }
}
