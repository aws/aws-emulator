// SPDX-License-Identifier: CC0-1.0

//! The zonal autoshift configuration ARC holds for a managed resource, driven end to end
//! over the wire.
//!
//! Every case here builds the production `MultiServiceRouter`, registers this service on
//! one shared engine and dispatches real `restJson1` requests, so what is pinned is the
//! HTTP behaviour rather than a handler called directly.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one request:
//!
//! * the zonal autoshift status a caller sets is PERSISTED for the resource it names, so
//!   the update's own answer and every later read — `GetManagedResource`,
//!   `ListManagedResources` — report it, whichever way the resource reached ARC (seeded,
//!   or registered by the service that owns it), and setting it again changes nothing
//!   (the operation is `@idempotent`);
//! * zonal autoshift can only be `ENABLED` for a resource that has a practice run
//!   configuration — "Practice runs must be configured for zonal autoshift to be
//!   enabled" (the `DeletePracticeRunConfiguration` documentation) — so asking for
//!   `ENABLED` where ARC holds none is refused 409 `ConflictException` with the reason
//!   `PracticeConfigurationDoesNotExist` and leaves the status the resource had, while
//!   `DISABLED` is accepted whatever the resource holds;
//! * the status is held PER RESOURCE, so setting it for one moves no other resource's;
//! * the target of the call is authorized before it is resolved, so a
//!   `resourceIdentifier` ARC holds nothing for is refused 403 `AccessDeniedException`
//!   whichever status it asks for.
//!
//! The rows differ along what those rules generalise over: which status is asked for and
//! which one the resource already carries, which resource the call names and how it
//! reached ARC, how long a sequence of updates the resource has seen, and the FORM and
//! SIZE of a target identifier.

use serde_json::{json, Value};

use super::registered_managed_resources::register;
use super::zonal_shift_lifecycle::{call, emulator, encoded, error_code, seeded};
use service_com_amazonaws_arczonalshift_percdataplane::ops::types::{
    CONFLICT_EXCEPTION_REASON_VALUES, ZONAL_AUTOSHIFT_STATUS_VALUES,
};
use service_com_amazonaws_arczonalshift_percdataplane::state::{
    PRACTICE_CONFIGURATION_REQUIRED_MESSAGE, TARGET_UNAVAILABLE_MESSAGE,
    ZONAL_AUTOSHIFT_PRACTICE_CONFIGURATION_MISSING_REASON,
};

/// A resource an owning service registers with ARC, so every rule is asserted for a
/// resource ARC did not create as well as for the ones this store was seeded with.
const REGISTERED_ARN: &str = "arn:aws:eks:us-east-1:000000000000:cluster/zonal-autoshift-eks";
const REGISTERED_ZONES: &[&str] = &["use1-az1", "use1-az2", "use1-az3", "use1-az4"];

/// `UpdateZonalAutoshiftConfiguration` over the wire: `PUT
/// /managedresources/{resourceIdentifier}` (`smithy_query(query="operation",
/// operation="UpdateZonalAutoshiftConfiguration")`).
pub(crate) async fn update(
    router: &smithy_server_runtime::MultiServiceRouter,
    resource: &str,
    status: &str,
) -> (u16, Value) {
    call(
        router,
        "PUT",
        &format!("/managedresources/{}", encoded(resource)),
        &json!({ "zonalAutoshiftStatus": status }).to_string(),
    )
    .await
}

/// `GetManagedResource` over the wire.
async fn get_managed_resource(
    router: &smithy_server_runtime::MultiServiceRouter,
    arn: &str,
) -> (u16, Value) {
    call(
        router,
        "GET",
        &format!("/managedresources/{}", encoded(arn)),
        "",
    )
    .await
}

/// The `zonalAutoshiftStatus` `GetManagedResource` reports for one resource.
async fn read_status(
    router: &smithy_server_runtime::MultiServiceRouter,
    arn: &str,
) -> Option<String> {
    let (status, body) = get_managed_resource(router, arn).await;
    assert_eq!(
        status, 200,
        "case {arn}: GetManagedResource answers 200, got {status} {body}"
    );
    body["zonalAutoshiftStatus"].as_str().map(str::to_string)
}

/// The `zonalAutoshiftStatus` `ListManagedResources` reports for one resource, walking
/// the pages it hands out.
async fn listed_status(
    router: &smithy_server_runtime::MultiServiceRouter,
    arn: &str,
) -> Option<String> {
    let mut query = String::new();
    loop {
        let (status, body) = call(router, "GET", &format!("/managedresources{query}"), "").await;
        assert_eq!(
            status, 200,
            "ListManagedResources{query}: expected 200, got {status} {body}"
        );
        for item in body["items"].as_array().cloned().unwrap_or_default() {
            if item["arn"].as_str() == Some(arn) {
                return item["zonalAutoshiftStatus"].as_str().map(str::to_string);
            }
        }
        match body["nextToken"].as_str() {
            Some(token) => query = format!("?nextToken={}", encoded(token)),
            None => return None,
        }
    }
}

/// A practice run configuration for `resource`, written the way
/// `CreatePracticeRunConfiguration` takes it: `outcomeAlarms` is the only member the
/// model requires and its `@length` is 1..10 (`smithy_query(query="operation",
/// operation="CreatePracticeRunConfiguration")`), so one alarm is the smallest
/// configuration ARC accepts.
async fn create_practice_run_configuration(
    router: &smithy_server_runtime::MultiServiceRouter,
    resource: &str,
    alarm: &str,
) {
    let body = json!({
        "resourceIdentifier": resource,
        "outcomeAlarms": [{
            "type": "CLOUDWATCH",
            "alarmIdentifier": format!(
                "arn:aws:cloudwatch:us-east-1:000000000000:alarm:{alarm}"
            ),
        }],
    })
    .to_string();
    let (status, created) = call(router, "POST", "/configuration", &body).await;
    assert_eq!(
        status, 201,
        "case {resource}: CreatePracticeRunConfiguration answers 201, got {status} {created}"
    );
}

/// Every target of this suite: the resources this store was seeded with and one its
/// owner registered.
fn targets(engine: &engine::InMemoryEngine) -> Vec<String> {
    let mut targets: Vec<String> = seeded(engine).into_iter().map(|(arn, _)| arn).collect();
    targets.push(REGISTERED_ARN.to_string());
    targets
}

/// The sequences of statuses a resource is taken through, so each row differs in which
/// status is asked for AND in the one the resource already carries: every value
/// `ZonalAutoshiftStatus` declares, each asked for twice in a row (the idempotent case)
/// and each asked for after the other.
const SEQUENCES: &[&[&str]] = &[
    &["ENABLED"],
    &["DISABLED"],
    &["ENABLED", "ENABLED"],
    &["DISABLED", "DISABLED"],
    &["ENABLED", "DISABLED", "ENABLED"],
    &["DISABLED", "ENABLED", "DISABLED", "ENABLED"],
];

#[tokio::test]
async fn the_status_an_update_sets_is_persisted_and_every_later_read_reports_it() {
    for sequence in SEQUENCES {
        // One emulator per row, so each row's reads are about its own store.
        let (router, engine) = emulator();
        register(&engine, REGISTERED_ARN, "zonal-autoshift-eks", REGISTERED_ZONES);

        for target in targets(&engine) {
            // `ENABLED` requires a practice run configuration, so every target in this
            // case has one: what the sequence below varies is the status.
            create_practice_run_configuration(&router, &target, "outcome").await;

            // Before any update the resource reports the status ARC holds for a
            // resource nothing has been configured for: what follows is computed from
            // the writes, not reported unconditionally.
            let case = format!("{target}: {sequence:?}");
            assert_eq!(
                read_status(&router, &target).await.as_deref(),
                Some("DISABLED"),
                "case {case}: a resource no caller set a status for reports DISABLED"
            );

            for wanted in *sequence {
                let (status, answered) = update(&router, &target, wanted).await;
                assert_eq!(
                    status, 200,
                    "case {case}: setting {wanted} answers 200, got {status} {answered}"
                );
                assert_eq!(
                    (
                        answered["resourceIdentifier"].as_str(),
                        answered["zonalAutoshiftStatus"].as_str(),
                    ),
                    (Some(target.as_str()), Some(*wanted)),
                    "case {case}: expected the resource's ARN and {wanted}, got {answered}"
                );

                // Read back through implemented readers: the status is PERSISTED, not
                // just echoed.
                assert_eq!(
                    read_status(&router, &target).await.as_deref(),
                    Some(*wanted),
                    "case {case}: GetManagedResource must report {wanted}"
                );
                assert_eq!(
                    listed_status(&router, &target).await.as_deref(),
                    Some(*wanted),
                    "case {case}: ListManagedResources must report {wanted}"
                );

                // The configuration the resource holds is untouched by a status write:
                // the two records are separate.
                let (_, view) = get_managed_resource(&router, &target).await;
                assert!(
                    view["practiceRunConfiguration"]["outcomeAlarms"]
                        .as_array()
                        .is_some_and(|alarms| alarms.len() == 1),
                    "case {case}: the practice run configuration is unchanged, got {view}"
                );
            }
        }
    }
}

#[tokio::test]
async fn the_status_is_held_per_resource_and_moves_no_other_resource() {
    let (router, engine) = emulator();
    register(&engine, REGISTERED_ARN, "zonal-autoshift-eks", REGISTERED_ZONES);
    let targets = targets(&engine);
    for target in &targets {
        create_practice_run_configuration(&router, target, "outcome").await;
    }

    for (index, target) in targets.iter().enumerate() {
        let (status, answered) = update(&router, target, "ENABLED").await;
        assert_eq!(
            status, 200,
            "case {target}: setting ENABLED answers 200, got {status} {answered}"
        );
        // Every target up to this one has been enabled; every later one still reports
        // the status no caller has changed.
        for (other_index, other) in targets.iter().enumerate() {
            let expected = if other_index <= index {
                "ENABLED"
            } else {
                "DISABLED"
            };
            assert_eq!(
                read_status(&router, other).await.as_deref(),
                Some(expected),
                "case {target} enabled: {other} must report {expected}"
            );
        }
    }
}

#[tokio::test]
async fn enabling_zonal_autoshift_without_a_practice_run_configuration_is_refused_409() {
    // The reason is the model's (`smithy_query(query="shape",
    // shape="ConflictExceptionReason")`), so a reason this service invents fails here.
    assert!(
        CONFLICT_EXCEPTION_REASON_VALUES
            .contains(&ZONAL_AUTOSHIFT_PRACTICE_CONFIGURATION_MISSING_REASON),
        "{ZONAL_AUTOSHIFT_PRACTICE_CONFIGURATION_MISSING_REASON} is not a \
         ConflictExceptionReason the model declares"
    );

    let (router, engine) = emulator();
    register(&engine, REGISTERED_ARN, "zonal-autoshift-eks", REGISTERED_ZONES);

    for target in targets(&engine) {
        // `DISABLED` — the status that withdraws the authorization — is accepted for a
        // resource with no configuration: the refusal is computed from the status asked
        // for, not returned for every call.
        let (status, answered) = update(&router, &target, "DISABLED").await;
        assert_eq!(
            status, 200,
            "case {target}: DISABLED is accepted with no configuration, got {status} {answered}"
        );
        assert_eq!(
            answered["zonalAutoshiftStatus"].as_str(),
            Some("DISABLED"),
            "case {target}: expected DISABLED, got {answered}"
        );

        // Asked for twice, so the refusal is shown to be about the resource's state
        // rather than about the first call.
        for attempt in 1..=2 {
            let case = format!("{target}: attempt {attempt}");
            let (status, body) = update(&router, &target, "ENABLED").await;
            assert_eq!(
                status, 409,
                "case {case}: expected 409, got {status} {body}"
            );
            assert_eq!(
                (error_code(&body).as_str(), body["reason"].as_str()),
                (
                    "ConflictException",
                    Some(ZONAL_AUTOSHIFT_PRACTICE_CONFIGURATION_MISSING_REASON)
                ),
                "case {case}: expected a PracticeConfigurationDoesNotExist conflict, got {body}"
            );
            assert_eq!(
                body["message"].as_str(),
                Some(PRACTICE_CONFIGURATION_REQUIRED_MESSAGE),
                "case {case}: expected the requirement ARC words for this conflict, got {body}"
            );
            // A refused call writes nothing: the resource keeps the status it had.
            assert_eq!(
                read_status(&router, &target).await.as_deref(),
                Some("DISABLED"),
                "case {case}: a refused update must persist nothing"
            );
        }

        // The other side of the rule: once ARC holds a configuration for the resource,
        // the same call is accepted — so the refusal is read from the store.
        create_practice_run_configuration(&router, &target, "outcome").await;
        let (status, answered) = update(&router, &target, "ENABLED").await;
        assert_eq!(
            status, 200,
            "case {target}: ENABLED with a configuration answers 200, got {status} {answered}"
        );
        assert_eq!(
            read_status(&router, &target).await.as_deref(),
            Some("ENABLED"),
            "case {target}: the enabled status is persisted"
        );
    }
}

/// `resourceIdentifier`s no managed resource carries, as `(identifier, class)`:
/// ARN-shaped and not, each end of the modelled `ResourceIdentifier` length range
/// (8..1024), and a near miss on one the store holds.
const UNHELD_TARGETS: &[(&str, &str)] = &[
    (
        "arn:aws:elasticloadbalancing:us-east-1:111111111111:loadbalancer/app/absent/73e2d6bc24d8a067",
        "a load balancer ARN in another account",
    ),
    (
        "arn:aws:eks:us-east-1:000000000000:cluster/never-registered",
        "an EKS cluster ARN ARC has no record of",
    ),
    ("abcdefgh", "the shortest identifier the model accepts"),
    ("zonal-shift-alb", "a resource name rather than its ARN"),
];

#[tokio::test]
async fn an_update_for_a_target_arc_holds_nothing_for_is_refused_403() {
    let (router, engine) = emulator();
    let mut unheld: Vec<(String, String)> = UNHELD_TARGETS
        .iter()
        .map(|(identifier, class)| ((*identifier).to_string(), (*class).to_string()))
        .collect();
    let (held, _zones) = seeded(&engine)[0].clone();
    unheld.push((
        format!("{held}0"),
        "a near miss on an identifier the store holds".to_string(),
    ));
    // A registration its owner has withdrawn is a target ARC holds nothing for either.
    register(&engine, REGISTERED_ARN, "zonal-autoshift-eks", REGISTERED_ZONES);
    engine
        .state::<service_common::arc_managed_resources::ManagedResourceRegistry>()
        .lock()
        .deregister(REGISTERED_ARN);
    unheld.push((
        REGISTERED_ARN.to_string(),
        "a registration its owner withdrew".to_string(),
    ));

    for (target, class) in &unheld {
        // Both statuses the model declares, so the branch is shown to be taken whatever
        // the call asks for.
        for wanted in ZONAL_AUTOSHIFT_STATUS_VALUES {
            let case = format!("{class}, {wanted}");
            let (status, body) = update(&router, target, wanted).await;
            assert_eq!(
                status, 403,
                "case {case}: an unauthorized target is refused 403, got {status} {body}"
            );
            assert_eq!(
                error_code(&body),
                "AccessDeniedException",
                "case {case}: expected AccessDeniedException, got {body}"
            );
            assert_eq!(
                body["message"].as_str(),
                Some(TARGET_UNAVAILABLE_MESSAGE),
                "case {case}: expected the non-disclosing refusal message, got {body}"
            );
            assert!(
                !body.to_string().contains(target.as_str()),
                "case {case}: the refusal must not disclose the target, got {body}"
            );
        }
    }
}
