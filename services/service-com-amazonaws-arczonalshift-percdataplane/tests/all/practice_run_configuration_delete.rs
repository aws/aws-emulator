// SPDX-License-Identifier: CC0-1.0

//! Deleting the practice run configuration ARC holds for a managed resource, driven end
//! to end over the wire.
//!
//! Every case here builds the production `MultiServiceRouter`, registers this service on
//! one shared engine and dispatches real `restJson1` requests, so what is pinned is the
//! HTTP behaviour rather than a handler called directly.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one request:
//!
//! * a deletion STOPS ARC holding the configuration for the resource it names, so the
//!   call's own answer reports the resource and the autoshift status it carries, and every
//!   later read — `GetManagedResource`, `ListManagedResources` — reports the resource as
//!   having none and the `practiceRunStatus` derived from it as `DISABLED`, whichever way
//!   the resource reached ARC (seeded, or registered by the service that owns it);
//! * the configuration is held PER RESOURCE, so deleting one resource's leaves every
//!   other resource's exactly as it was;
//! * zonal autoshift depends on the configuration — "Before you can delete a practice run
//!   configuration for a resource[,] you must disable zonal autoshift for the resource"
//!   (`smithy_query(query="doc", shape="DeletePracticeRunConfiguration")`) — so a deletion
//!   asked for while it is `ENABLED` is refused 409 `ConflictException`
//!   (`AutoShiftEnabled`) and leaves the configuration held, while the same deletion is
//!   served once autoshift is disabled;
//! * there is no precondition that a configuration exist: the operation is `@idempotent`,
//!   so a deletion asked of a resource that holds none — including a second deletion of
//!   one just removed — is SERVED, answers exactly what the first did, and leaves the
//!   resource holding nothing;
//! * the target of the call is authorized before it is resolved, so a `resourceIdentifier`
//!   ARC holds nothing for is refused 403 `AccessDeniedException`.
//!
//! The rows differ along what those rules generalise over: WHICH configuration the
//! resource held before the deletion (each of the configurations the create suite spans,
//! from the one member the model requires to every list at its largest), which resource
//! the call names and how it reached ARC, how many resources hold a configuration at the
//! time, and the FORM and SIZE of a target identifier.

use serde_json::Value;

use super::practice_run_configuration::{
    assert_holds, create, get_managed_resource, hold, listed_practice_run_status, REGISTERED_ARN,
    REGISTERED_ZONES, ROWS, UNHELD_TARGETS,
};
use super::registered_managed_resources::register;
use super::zonal_autoshift_configuration::update as set_autoshift;
use super::zonal_shift_lifecycle::{call, emulator, encoded, error_code, seeded};
use service_com_amazonaws_arczonalshift_percdataplane::ops::types::CONFLICT_EXCEPTION_REASON_VALUES;
use service_com_amazonaws_arczonalshift_percdataplane::state::{
    AUTOSHIFT_ENABLED_MESSAGE, AUTOSHIFT_ENABLED_REASON, TARGET_UNAVAILABLE_MESSAGE,
};

/// `DeletePracticeRunConfiguration` over the wire: `DELETE
/// /configuration/{resourceIdentifier}`, the binding the model declares
/// (`smithy_query(query="operation", operation="DeletePracticeRunConfiguration")`).
async fn delete(
    router: &smithy_server_runtime::MultiServiceRouter,
    resource: &str,
) -> (u16, Value) {
    call(
        router,
        "DELETE",
        &format!("/configuration/{}", encoded(resource)),
        "",
    )
    .await
}

// The two helpers the cases below reach for are the sibling suites' own: `set_autoshift`
// is `UpdateZonalAutoshiftConfiguration` over the wire (the only operation that writes the
// status a deletion is refused for), and `hold` writes a row's configuration for a
// resource, which is the state each deletion starts from.

/// Read one managed resource's practice run configuration back, and that
/// `ListManagedResources` agrees the resource has none.
async fn assert_holds_nothing(
    router: &smithy_server_runtime::MultiServiceRouter,
    case: &str,
    arn: &str,
) {
    let (status, view) = get_managed_resource(router, arn).await;
    assert_eq!(
        status, 200,
        "case {case}: GetManagedResource answers 200, got {status} {view}"
    );
    assert!(
        view["practiceRunConfiguration"].is_null(),
        "case {case}: the deleted configuration must not be reported, got {view}"
    );
    assert_eq!(
        listed_practice_run_status(router, arn).await.as_deref(),
        Some("DISABLED"),
        "case {case}: a resource with no configuration reports DISABLED"
    );
}

/// That a conflict ARC answers is one the model declares a reason for, so a client
/// branching on `reason` sees a value of the modelled enum.
fn assert_modelled_reason(case: &str, body: &Value) {
    let reason = body["reason"].as_str().unwrap_or_default();
    assert!(
        CONFLICT_EXCEPTION_REASON_VALUES.contains(&reason),
        "case {case}: {reason} is not one of the modelled conflict reasons, got {body}"
    );
}

#[tokio::test]
async fn a_deleted_practice_run_configuration_is_no_longer_held_by_any_read() {
    for row in ROWS {
        // One emulator per row, so each row's reads are about its own store, and the
        // targets span the resources ARC was seeded with and one its owner registered.
        let (router, engine) = emulator();
        register(&engine, REGISTERED_ARN, "practice-run-eks", REGISTERED_ZONES);
        let mut targets: Vec<String> = seeded(&engine).into_iter().map(|(arn, _)| arn).collect();
        targets.push(REGISTERED_ARN.to_string());

        for target in &targets {
            let case = format!("{}: {target}", row.case);
            let held = hold(&router, row, target).await;

            // Before the deletion the resource HAS the configuration: what follows is
            // computed from the deletion rather than reported unconditionally.
            let (_, before) = get_managed_resource(&router, target).await;
            assert_holds(&case, &before["practiceRunConfiguration"], &held);

            let (status, answer) = delete(&router, target).await;
            assert_eq!(
                status, 200,
                "case {case}: DeletePracticeRunConfiguration answers 200, got {status} \
                 {answer}"
            );
            assert_eq!(
                (answer["arn"].as_str(), answer["zonalAutoshiftStatus"].as_str()),
                (Some(target.as_str()), Some("DISABLED")),
                "case {case}: expected the resource's ARN and its current autoshift \
                 status, got {answer}"
            );
            assert!(
                answer["name"].as_str().is_some_and(|name| !name.is_empty()),
                "case {case}: the resource's name is required, got {answer}"
            );
            // The deletion's answer carries no configuration at all: the model declares
            // the response three members, and none of them is one.
            assert!(
                answer["practiceRunConfiguration"].is_null(),
                "case {case}: the deletion reports no configuration, got {answer}"
            );

            // Read back through implemented readers: the configuration is GONE from the
            // store, not just absent from the answer.
            assert_holds_nothing(&router, &case, target).await;

            // And the resource can hold a configuration again, which is what "the
            // resource has none" means to the operation that creates one: the second
            // create is served rather than refused as a duplicate.
            let (status, recreated) = create(&router, &ROWS[0].create_body(target)).await;
            assert_eq!(
                status, 201,
                "case {case}: a resource whose configuration was deleted can hold one \
                 again, got {status} {recreated}"
            );
            assert_holds(&case, &recreated["practiceRunConfiguration"], &ROWS[0].configuration());
        }
    }
}

#[tokio::test]
async fn deleting_one_resources_configuration_leaves_every_other_resources_held() {
    let (router, engine) = emulator();
    register(&engine, REGISTERED_ARN, "practice-run-eks", REGISTERED_ZONES);
    let mut targets: Vec<String> = seeded(&engine).into_iter().map(|(arn, _)| arn).collect();
    targets.push(REGISTERED_ARN.to_string());

    // Every resource holds a configuration of its own, so what each read reports after a
    // deletion is per resource rather than per store.
    let mut held: Vec<(String, serde_json::Map<String, Value>)> = Vec::new();
    for (index, target) in targets.iter().enumerate() {
        let row = &ROWS[index % ROWS.len()];
        held.push((target.clone(), hold(&router, row, target).await));
    }

    // One deletion at a time, in order: after each, the deleted resource holds nothing
    // and every resource not yet deleted still holds exactly what was written for it.
    for deleted in 0..held.len() {
        let (target, _) = held[deleted].clone();
        let (status, answer) = delete(&router, &target).await;
        assert_eq!(
            status, 200,
            "case {target}: the deletion answers 200, got {status} {answer}"
        );
        for (index, (other, configuration)) in held.iter().enumerate() {
            let case = format!("{other} after deleting {target}");
            if index <= deleted {
                assert_holds_nothing(&router, &case, other).await;
            } else {
                let (_, view) = get_managed_resource(&router, other).await;
                assert_holds(&case, &view["practiceRunConfiguration"], configuration);
                assert_eq!(
                    listed_practice_run_status(&router, other).await.as_deref(),
                    Some("ENABLED"),
                    "case {case}: a resource that still has a configuration reports ENABLED"
                );
            }
        }
    }
}

#[tokio::test]
async fn deleting_a_configuration_while_zonal_autoshift_is_enabled_is_refused_409() {
    let (router, engine) = emulator();
    register(&engine, REGISTERED_ARN, "practice-run-eks", REGISTERED_ZONES);
    let mut targets: Vec<String> = seeded(&engine).into_iter().map(|(arn, _)| arn).collect();
    targets.push(REGISTERED_ARN.to_string());

    for (index, target) in targets.iter().enumerate() {
        // The configuration the resource holds is varied across the rows, so the refusal
        // is shown to be about the autoshift status rather than about what is held.
        let row = &ROWS[index % ROWS.len()];
        let case = format!("{target}, holding {}", row.case);
        let held = hold(&router, row, target).await;
        let (status, enabled) = set_autoshift(&router, target, "ENABLED").await;
        assert_eq!(
            status, 200,
            "case {case}: enabling zonal autoshift answers 200, got {status} {enabled}"
        );

        let (status, body) = delete(&router, target).await;
        assert_eq!(status, 409, "case {case}: expected 409, got {status} {body}");
        assert_eq!(
            (error_code(&body).as_str(), body["reason"].as_str()),
            ("ConflictException", Some(AUTOSHIFT_ENABLED_REASON)),
            "case {case}: expected an {AUTOSHIFT_ENABLED_REASON} conflict, got {body}"
        );
        assert_eq!(
            body["message"].as_str(),
            Some(AUTOSHIFT_ENABLED_MESSAGE),
            "case {case}: expected the refusal ARC words for a resource whose zonal \
             autoshift is enabled, got {body}"
        );
        assert_modelled_reason(&case, &body);
        assert!(
            !body.to_string().contains(target.as_str()),
            "case {case}: the refusal must not disclose the target, got {body}"
        );

        // The refusal persisted nothing: the resource still holds its configuration, and
        // zonal autoshift is still enabled for it.
        let (_, view) = get_managed_resource(&router, target).await;
        assert_holds(&case, &view["practiceRunConfiguration"], &held);
        assert_eq!(
            view["zonalAutoshiftStatus"].as_str(),
            Some("ENABLED"),
            "case {case}: a refused deletion must not change the autoshift status, got {view}"
        );

        // And once autoshift is disabled, the same deletion is served: the 409 is the
        // resource's state, not the operation.
        let (status, disabled) = set_autoshift(&router, target, "DISABLED").await;
        assert_eq!(
            status, 200,
            "case {case}: disabling zonal autoshift answers 200, got {status} {disabled}"
        );
        let (status, answer) = delete(&router, target).await;
        assert_eq!(
            status, 200,
            "case {case}: the deletion is served once autoshift is disabled, got {status} \
             {answer}"
        );
        assert_holds_nothing(&router, &case, target).await;
    }
}

#[tokio::test]
async fn deleting_a_configuration_a_resource_does_not_have_is_served() {
    let (router, engine) = emulator();
    register(&engine, REGISTERED_ARN, "practice-run-eks", REGISTERED_ZONES);
    let mut targets: Vec<String> = seeded(&engine).into_iter().map(|(arn, _)| arn).collect();
    targets.push(REGISTERED_ARN.to_string());

    // Two ways a resource can have no configuration: it never had one, and it had one that
    // was just deleted — a second deletion of the same resource. The operation is
    // `@idempotent`, so both are served and answer what a deletion answers.
    for (index, target) in targets.iter().enumerate() {
        let never_held = format!("{target}, which never held a configuration");
        let (status, body) = delete(&router, target).await;
        assert_eq!(
            status, 200,
            "case {never_held}: expected 200, got {status} {body}"
        );
        assert_eq!(
            (body["arn"].as_str(), body["zonalAutoshiftStatus"].as_str()),
            (Some(target.as_str()), Some("DISABLED")),
            "case {never_held}: expected the resource's ARN and its current autoshift \
             status, got {body}"
        );
        assert!(
            body["name"].as_str().is_some_and(|name| !name.is_empty()),
            "case {never_held}: the resource's name is required, got {body}"
        );
        assert_holds_nothing(&router, &never_held, target).await;

        // The configuration held before the first deletion is varied across the rows, so
        // the second deletion is served whatever was removed, and answers the same thing.
        let row = &ROWS[index % ROWS.len()];
        let deleted_once = format!("{target}, whose {} was already deleted", row.case);
        hold(&router, row, target).await;
        let (status, first) = delete(&router, target).await;
        assert_eq!(
            status, 200,
            "case {deleted_once}: the first deletion answers 200, got {status} {first}"
        );
        let (status, again) = delete(&router, target).await;
        assert_eq!(
            status, 200,
            "case {deleted_once}: expected 200, got {status} {again}"
        );
        assert_eq!(
            again, first,
            "case {deleted_once}: repeating the deletion must answer the same thing, got \
             {again}"
        );
        assert_holds_nothing(&router, &deleted_once, target).await;
    }
}

#[tokio::test]
async fn a_deletion_for_a_target_arc_holds_nothing_for_is_refused_403() {
    let (router, engine) = emulator();
    let mut targets: Vec<(String, String)> = UNHELD_TARGETS
        .iter()
        .map(|(identifier, class)| ((*identifier).to_string(), (*class).to_string()))
        .collect();
    let (held_target, _zones) = seeded(&engine)[0].clone();
    targets.push((
        format!("{held_target}0"),
        "a near miss on an identifier the store holds".to_string(),
    ));

    // A configuration is held for a resource ARC does know, so the refusals below are the
    // authorization branch rather than the missing-configuration one.
    let held = hold(&router, &ROWS[0], &held_target).await;

    for (target, class) in &targets {
        let (status, body) = delete(&router, target).await;
        assert_eq!(
            status, 403,
            "case {class}: an unauthorized target is refused 403, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "AccessDeniedException",
            "case {class}: expected AccessDeniedException, got {body}"
        );
        assert_eq!(
            body["message"].as_str(),
            Some(TARGET_UNAVAILABLE_MESSAGE),
            "case {class}: expected the non-disclosing refusal message, got {body}"
        );
        assert!(
            !body.to_string().contains(target.as_str()),
            "case {class}: the refusal must not disclose the target, got {body}"
        );
    }

    // None of those calls touched the configuration the resource ARC does know holds.
    let (_, view) = get_managed_resource(&router, &held_target).await;
    assert_holds(
        "the resource ARC holds a configuration for",
        &view["practiceRunConfiguration"],
        &held,
    );
}
