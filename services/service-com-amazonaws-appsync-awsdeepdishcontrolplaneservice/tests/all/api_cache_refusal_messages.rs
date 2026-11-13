// SPDX-License-Identifier: CC0-1.0

//! What AppSync's api-cache refusals SAY, and when a teardown is served, end to end
//! over HTTP.
//!
//! Three rules, each asserted over a table that spans the class it generalises over
//! rather than the one request that exposed it:
//!
//! 1. a cache operation that finds no cache to address is refused 404
//!    `NotFoundException` whose message names the CALL that met the absence and whether
//!    the cache was torn down or never configured; the teardown is not one of them — it
//!    asks for the absence itself, so an empty cache slot already satisfies it and it is
//!    served;
//! 2. a create on an API that already has a cache is refused 400 `BadRequestException`
//!    with AppSync's fixed sentence for that class, and the stored cache is untouched;
//! 3. a teardown's precondition is the CREATION of the instance, not the status it is
//!    in at the moment: a cache whose create has not completed cannot be deleted, while
//!    one that exists is deletable even while new settings are being applied to it — so
//!    an update never leaves a cache that cannot be removed, and two deletes racing on
//!    one cache remove it once and are both served.
//!
//! The sentences asserted here are written out in full rather than read from the
//! service, so the assertion is about AppSync's wording rather than about whatever the
//! handler happens to build.
//!
//! The `call`/`create_api`/`Answer` harness is the error-contract suite's.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_appsync_awsdeepdishcontrolplaneservice::{state, EmulatedService};
use smithy_server_runtime::MultiServiceRouter;

use crate::error_contract::{call, create_api, Answer};

/// The span the service records for a cache's transition to available — read from the
/// service itself, so this suite waits exactly as long as the service asks for rather
/// than on a number of its own.
const CACHE_SPAN: Duration = state::CACHE_SPAN;

/// Slack on top of a wait, covering the scheduler rather than a guessed duration.
const SLACK: Duration = Duration::from_millis(100);

/// `ApiCachingBehavior`'s whole value set, in the order the model declares it.
/// Source: `smithy_query(query="shape", shape="ApiCachingBehavior")`.
const CACHING_BEHAVIORS: &[&str] = &[
    "FULL_REQUEST_CACHING",
    "PER_RESOLVER_CACHING",
    "OPERATION_LEVEL_CACHING",
];

/// `ApiCacheType` values spanning the class the rules generalise over: the generic
/// instance identifiers and the discouraged legacy EC2-style ones alike.
/// Source: `smithy_query(query="shape", shape="ApiCacheType")`.
const CACHE_TYPES: &[&str] = &["MEDIUM", "LARGE_4X", "XLARGE", "R4_XLARGE"];

/// `CacheHealthMetricsConfig`'s whole value set.
/// Source: `smithy_query(query="shape", shape="CacheHealthMetricsConfig")`.
const HEALTH_METRICS: &[&str] = &["ENABLED", "DISABLED"];

/// One cache operation that ADDRESSES an existing cache, as a client reaches it: the
/// call's name, its HTTP method, the path under the API, whether it carries a body, and
/// the sentence AppSync's `NotFoundException` carries when that call finds no cache to
/// address — one for an API that never had a cache and one for an API whose cache was
/// torn down.
///
/// `DeleteApiCache` is not one of them: a teardown asks for the absence of the cache, so
/// an empty slot already satisfies it and it renders no absent-cache refusal at all (its
/// served no-op is asserted on its own below).
///
/// Source for the method and path of each: `smithy_query(query="digest")` — `GET`/`POST
/// …/ApiCaches`, `POST …/ApiCaches/update`, `DELETE …/ApiCaches`, `DELETE
/// …/FlushCache`.
struct CacheOp {
    name: &'static str,
    method: &'static str,
    path: &'static str,
    carries_body: bool,
    /// AWS's sentence when no cache was ever configured on the API.
    never_configured_message: &'static str,
    /// AWS's sentence when the API's cache was torn down.
    deleted_message: &'static str,
}

/// Every cache operation that addresses an existing cache, each with AWS's sentence for
/// the two absences it can meet.
const CACHE_OPS: &[CacheOp] = &[
    CacheOp {
        name: "GetApiCache",
        method: "GET",
        path: "ApiCaches",
        carries_body: false,
        never_configured_message: "Unable to get the cache as it doesn't exist, please create the \
                                   cache first.",
        deleted_message: "Unable to get the cache as it has been deleted, please re-create the \
                          cache.",
    },
    CacheOp {
        name: "UpdateApiCache",
        method: "POST",
        path: "ApiCaches/update",
        carries_body: true,
        never_configured_message: "Unable to update the cache as it doesn't exist, please create \
                                   the cache first.",
        deleted_message: "Unable to update the cache as it has been deleted, please re-create \
                          the cache.",
    },
    CacheOp {
        name: "FlushApiCache",
        method: "DELETE",
        path: "FlushCache",
        carries_body: false,
        never_configured_message: "Unable to flush the cache as it doesn't exist, please create \
                                   the cache first.",
        deleted_message: "Unable to flush the cache as it has been deleted, please re-create the \
                          cache.",
    },
];

/// AWS's sentence for a read of an API whose cache was torn down, the one the lifecycle
/// assertions below read back.
const DELETED_MESSAGE: &str =
    "Unable to get the cache as it has been deleted, please re-create the cache.";

/// AWS's sentence for a read of an API no teardown of which has ever removed a cache —
/// one that never had one at all.
const NEVER_CONFIGURED_MESSAGE: &str =
    "Unable to get the cache as it doesn't exist, please create the cache first.";

/// AWS's fixed sentence for a create on an API that already has a cache.
const ALREADY_ENABLED_MESSAGE: &str = "The API has already enabled caching.";

/// A router with only this service mounted, over a fresh engine.
fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// A `CreateApiCache` body with the members the create requires, plus the optional
/// health-metrics setting.
fn create_cache_body(ttl: i64, behavior: &str, cache_type: &str, health: &str) -> Value {
    json!({
        "ttl": ttl,
        "apiCachingBehavior": behavior,
        "type": cache_type,
        "transitEncryptionEnabled": false,
        "atRestEncryptionEnabled": true,
        "healthMetricsConfig": health,
    })
}

/// An `UpdateApiCache` body with the three members the update requires.
fn update_cache_body(ttl: i64, behavior: &str, cache_type: &str) -> Value {
    json!({
        "ttl": ttl,
        "apiCachingBehavior": behavior,
        "type": cache_type,
    })
}

/// Create the API's cache.
async fn create_cache(
    router: &MultiServiceRouter,
    api_id: &str,
    ttl: i64,
    behavior: &str,
    cache_type: &str,
    health: &str,
) -> Answer {
    call(
        router,
        "POST",
        &format!("/v1/apis/{api_id}/ApiCaches"),
        Some(create_cache_body(ttl, behavior, cache_type, health)),
    )
    .await
}

/// Read the API's cache back.
async fn get_cache(router: &MultiServiceRouter, api_id: &str) -> Answer {
    call(router, "GET", &format!("/v1/apis/{api_id}/ApiCaches"), None).await
}

/// Update the API's cache.
async fn update_cache(
    router: &MultiServiceRouter,
    api_id: &str,
    ttl: i64,
    behavior: &str,
    cache_type: &str,
) -> Answer {
    call(
        router,
        "POST",
        &format!("/v1/apis/{api_id}/ApiCaches/update"),
        Some(update_cache_body(ttl, behavior, cache_type)),
    )
    .await
}

/// Delete the API's cache.
async fn delete_cache(router: &MultiServiceRouter, api_id: &str) -> Answer {
    call(
        router,
        "DELETE",
        &format!("/v1/apis/{api_id}/ApiCaches"),
        None,
    )
    .await
}

/// Send one cache operation at the API, with a body when the operation takes one.
async fn send(
    router: &MultiServiceRouter,
    operation: &CacheOp,
    api_id: &str,
    ttl: i64,
    behavior: &str,
    cache_type: &str,
) -> Answer {
    let body = operation
        .carries_body
        .then(|| update_cache_body(ttl, behavior, cache_type));
    call(
        router,
        operation.method,
        &format!("/v1/apis/{api_id}/{}", operation.path),
        body,
    )
    .await
}

/// Whether the response body carries no members at all — the shape of the modelled
/// empty `DeleteApiCacheResponse`, which is either an object with no members or no body
/// at all.
fn carries_no_members(body: &Value) -> bool {
    match body {
        Value::Null => true,
        Value::Object(members) => members.is_empty(),
        _ => false,
    }
}

/// Fire two `UpdateApiCache` calls at one cache at the same time, and assert both are
/// served — the arbitration the deletes below must survive.
async fn two_racing_updates(
    router: &MultiServiceRouter,
    api_id: &str,
    ttl: i64,
    behavior: &str,
    cache_type: &str,
    case: &str,
) {
    let (first, second) = tokio::join!(
        update_cache(router, api_id, ttl, behavior, cache_type),
        update_cache(router, api_id, ttl, behavior, cache_type),
    );
    for (racer, answer) in [first, second].into_iter().enumerate() {
        assert_eq!(
            answer.status, 200,
            "case {case:?} racing update {racer}: expected 200, got {answer:?}"
        );
    }
}

/// Why the API the row addresses has no cache.
#[derive(Debug, Clone, Copy)]
enum Absence {
    /// No create ever ran for this API.
    NeverConfigured,
    /// A cache existed and a delete removed it.
    Deleted,
}

/// Rule 1: an absent cache's refusal names the call that met it, and a read of an API
/// whose cache was torn down names the deletion — while the teardown, which asks for
/// that absence, is served instead of refused.
///
/// The rows differ along what the rule generalises over: why the cache is absent, the
/// settings the deleted cache was created with, and which of the four cache operations
/// meets the absence (every row drives all four).
#[tokio::test]
async fn an_absent_caches_refusal_names_the_call_that_met_it() {
    let router = router();

    // (case, why the cache is absent, ttl the row's cache was created with)
    let rows: &[(&str, Absence, i64)] = &[
        ("an api that never had a cache", Absence::NeverConfigured, 300),
        ("an api whose minimum-ttl cache was deleted", Absence::Deleted, 1),
        ("an api whose mid-window cache was deleted", Absence::Deleted, 600),
        ("an api whose maximum-ttl cache was deleted", Absence::Deleted, 3_600),
    ];

    for (index, (case, absence, ttl)) in rows.iter().enumerate() {
        let cache_type = CACHE_TYPES[index % CACHE_TYPES.len()];
        let behavior = CACHING_BEHAVIORS[index % CACHING_BEHAVIORS.len()];
        let health = HEALTH_METRICS[index % HEALTH_METRICS.len()];
        let case = format!("{case} ({cache_type} caching {behavior}, metrics {health})");
        let api_id = create_api(&router, &format!("Api_cache_absent_{index}")).await;

        if let Absence::Deleted = absence {
            let created = create_cache(&router, &api_id, *ttl, behavior, cache_type, health).await;
            assert_eq!(
                created.status, 200,
                "case {case:?}: expected the seed create to be accepted, got {created:?}"
            );
            // The instance has to have been created before it can be torn down.
            tokio::time::sleep(CACHE_SPAN + SLACK).await;
            let deleted = delete_cache(&router, &api_id).await;
            assert_eq!(
                deleted.status, 200,
                "case {case:?}: expected the seed delete to be served, got {deleted:?}"
            );
        }

        for operation in CACHE_OPS {
            let answer = send(&router, operation, &api_id, *ttl, behavior, cache_type).await;
            // Which sentence the operation owes for the absence this row built.
            let expected = match absence {
                Absence::NeverConfigured => operation.never_configured_message,
                Absence::Deleted => operation.deleted_message,
            };
            assert_eq!(
                answer.status, 404,
                "case {case:?} {}: expected status 404, got {answer:?}",
                operation.name
            );
            assert_eq!(
                answer.error_type, "NotFoundException",
                "case {case:?} {}: expected error type NotFoundException, got {answer:?}",
                operation.name
            );
            assert_eq!(
                answer.message, expected,
                "case {case:?} {}: expected {expected:?}, got {answer:?}",
                operation.name
            );
        }

        // The teardown meets the same absence and is SERVED: it asked for exactly what
        // it found, so it answers the modelled empty response rather than a refusal. It
        // removed no instance, so it changes nothing a later reader sees: the API reports
        // the same absence afterwards as before.
        let no_op = delete_cache(&router, &api_id).await;
        assert_eq!(
            no_op.status, 200,
            "case {case:?} DeleteApiCache: expected the teardown of an absent cache to be \
             served, got {no_op:?}"
        );
        assert!(
            carries_no_members(&no_op.body),
            "case {case:?} DeleteApiCache: expected a response with no members, got {no_op:?}"
        );
        let after_no_op = get_cache(&router, &api_id).await;
        assert_eq!(
            after_no_op.status, 404,
            "case {case:?}: expected the cache to still be absent, got {after_no_op:?}"
        );
        let after_expected = match absence {
            Absence::NeverConfigured => NEVER_CONFIGURED_MESSAGE,
            Absence::Deleted => DELETED_MESSAGE,
        };
        assert_eq!(
            after_no_op.message, after_expected,
            "case {case:?}: expected {after_expected:?} after a teardown that removed \
             nothing, got {after_no_op:?}"
        );

        // None of those refusals wrote anything: a create is still accepted, and once
        // it has run the read reports a cache rather than either absence.
        let recreated = create_cache(&router, &api_id, *ttl, behavior, cache_type, health).await;
        assert_eq!(
            recreated.status, 200,
            "case {case:?}: expected the cache to be creatable, got {recreated:?}"
        );
        let read = get_cache(&router, &api_id).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: expected the re-created cache to be readable, got {read:?}"
        );
        assert_eq!(
            read.body["apiCache"]["ttl"].as_i64(),
            Some(*ttl),
            "case {case:?}: expected the re-created cache to report its own ttl, got {read:?}"
        );
    }
}

/// Rule 2: a create on an API that already has a cache is refused with AppSync's fixed
/// sentence for that class, and the cache the first create stored is untouched.
///
/// The rows differ along the settings the two creates carry and along the lifecycle
/// state the stored instance is in when the second create arrives (still being created,
/// available, having new settings applied) — the refusal is about caching already being
/// enabled, so it does not depend on any of them.
#[tokio::test]
async fn a_second_create_is_refused_because_caching_is_already_enabled() {
    let router = router();

    /// Where in its lifecycle the stored instance is when the second create arrives.
    #[derive(Debug, Clone, Copy)]
    enum Stored {
        BeingCreated,
        Available,
        BeingModified,
    }

    let rows: &[(&str, i64, i64, Stored)] = &[
        ("a cache still being created", 300, 60, Stored::BeingCreated),
        ("an available cache", 1, 3_600, Stored::Available),
        ("a cache having new settings applied", 900, 120, Stored::BeingModified),
    ];

    for (index, (case, ttl, second_ttl, stored)) in rows.iter().enumerate() {
        let cache_type = CACHE_TYPES[index % CACHE_TYPES.len()];
        let behavior = CACHING_BEHAVIORS[index % CACHING_BEHAVIORS.len()];
        let health = HEALTH_METRICS[index % HEALTH_METRICS.len()];
        let case = format!("{case} ({cache_type} caching {behavior}, metrics {health})");
        let api_id = create_api(&router, &format!("Api_cache_duplicate_{index}")).await;
        let created = create_cache(&router, &api_id, *ttl, behavior, cache_type, health).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected the first create to be accepted, got {created:?}"
        );

        match stored {
            Stored::BeingCreated => {}
            Stored::Available => tokio::time::sleep(CACHE_SPAN + SLACK).await,
            Stored::BeingModified => {
                tokio::time::sleep(CACHE_SPAN + SLACK).await;
                let updated = update_cache(&router, &api_id, *ttl, behavior, cache_type).await;
                assert_eq!(
                    updated.status, 200,
                    "case {case:?}: expected the update to be accepted, got {updated:?}"
                );
            }
        }

        // A second create, with settings of its own, is refused — and so is a third,
        // since the refusal writes nothing.
        for attempt in 1..=2 {
            let again = create_cache(
                &router,
                &api_id,
                *second_ttl,
                CACHING_BEHAVIORS[(index + 1) % CACHING_BEHAVIORS.len()],
                CACHE_TYPES[(index + 1) % CACHE_TYPES.len()],
                HEALTH_METRICS[(index + 1) % HEALTH_METRICS.len()],
            )
            .await;
            assert_eq!(
                again.status, 400,
                "case {case:?} create {attempt}: expected status 400, got {again:?}"
            );
            assert_eq!(
                again.error_type, "BadRequestException",
                "case {case:?} create {attempt}: expected error type BadRequestException, \
                 got {again:?}"
            );
            assert_eq!(
                again.message, ALREADY_ENABLED_MESSAGE,
                "case {case:?} create {attempt}: expected {ALREADY_ENABLED_MESSAGE:?}, \
                 got {again:?}"
            );
        }

        // The refused creates changed nothing: the stored cache still reports the
        // settings the accepted calls left on it.
        let read = get_cache(&router, &api_id).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: expected the stored cache to survive, got {read:?}"
        );
        assert_eq!(
            read.body["apiCache"]["ttl"].as_i64(),
            Some(*ttl),
            "case {case:?}: expected the stored ttl to be unchanged, got {read:?}"
        );
        assert_eq!(
            read.body["apiCache"]["type"].as_str(),
            Some(cache_type),
            "case {case:?}: expected the stored type to be unchanged, got {read:?}"
        );
    }
}

/// Rule 3, its served half: an instance that has been created is torn down whatever the
/// service is doing to it, so a cache that has just been given new settings is still
/// deletable — and after the delete the read reports a deleted cache.
///
/// The rows differ along how many updates preceded the delete and along the settings
/// each update carried, so the rule holds however often a caller reconfigures a cache.
#[tokio::test]
async fn a_cache_is_deletable_while_its_new_settings_are_being_applied() {
    let router = router();

    let rows: &[(&str, usize, i64)] = &[
        ("one update, mid-window ttl", 1, 600),
        ("two updates in a row", 2, 30),
        ("three updates in a row, the maximum ttl", 3, 3_600),
    ];

    for (index, (case, updates, ttl)) in rows.iter().enumerate() {
        let cache_type = CACHE_TYPES[index % CACHE_TYPES.len()];
        let behavior = CACHING_BEHAVIORS[index % CACHING_BEHAVIORS.len()];
        let health = HEALTH_METRICS[index % HEALTH_METRICS.len()];
        let case = format!("{case} ({cache_type} caching {behavior}, metrics {health})");
        let api_id = create_api(&router, &format!("Api_cache_modifying_{index}")).await;
        let created = create_cache(&router, &api_id, 300, behavior, cache_type, health).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected the create to be accepted, got {created:?}"
        );
        // The instance has to exist before it accepts new settings.
        tokio::time::sleep(CACHE_SPAN + SLACK).await;

        for round in 1..=*updates {
            let updated = update_cache(&router, &api_id, *ttl, behavior, cache_type).await;
            assert_eq!(
                updated.status, 200,
                "case {case:?} update {round}: expected 200, got {updated:?}"
            );
        }

        // Straight after the last update, with the new settings still being applied:
        // the instance exists, so the teardown is served.
        let read = get_cache(&router, &api_id).await;
        assert_eq!(
            read.body["apiCache"]["status"].as_str(),
            Some("MODIFYING"),
            "case {case:?}: expected the instance to be applying its new settings, got {read:?}"
        );
        let deleted = delete_cache(&router, &api_id).await;
        assert_eq!(
            deleted.status, 200,
            "case {case:?}: expected the delete to be served, got {deleted:?}"
        );

        // And the cache is gone, reported as deleted rather than as never configured.
        let gone = get_cache(&router, &api_id).await;
        assert_eq!(
            gone.status, 404,
            "case {case:?}: expected the cache to be gone, got {gone:?}"
        );
        assert_eq!(
            gone.message, DELETED_MESSAGE,
            "case {case:?}: expected {DELETED_MESSAGE:?}, got {gone:?}"
        );
    }
}

/// Rule 3, its refusal half: the precondition is the CREATION, so an instance whose
/// create has not completed is not deletable, and the cache stays where it is.
#[tokio::test]
async fn a_cache_whose_create_has_not_completed_is_not_deletable() {
    let router = router();

    for (index, cache_type) in CACHE_TYPES.iter().enumerate() {
        let behavior = CACHING_BEHAVIORS[index % CACHING_BEHAVIORS.len()];
        let health = HEALTH_METRICS[index % HEALTH_METRICS.len()];
        let case = format!("a {cache_type} cache caching {behavior}, metrics {health}");
        let api_id = create_api(&router, &format!("Api_cache_creating_{index}")).await;
        let created = create_cache(&router, &api_id, 300, behavior, cache_type, health).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected the create to be accepted, got {created:?}"
        );
        assert_eq!(
            created.body["apiCache"]["status"].as_str(),
            Some("CREATING"),
            "case {case:?}: expected a provisioning instance, got {created:?}"
        );

        let refused = delete_cache(&router, &api_id).await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected status 400, got {refused:?}"
        );
        assert_eq!(
            refused.error_type, "BadRequestException",
            "case {case:?}: expected error type BadRequestException, got {refused:?}"
        );
        assert_eq!(
            refused.message, state::CACHE_NOT_DELETABLE_MESSAGE,
            "case {case:?}: expected the not-deletable sentence, got {refused:?}"
        );

        // The refusal kept the cache, so the read still reports it.
        let read = get_cache(&router, &api_id).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: expected the cache to survive the refusal, got {read:?}"
        );
    }
}

/// Rule 3's arbitration: when two deletes race on one cache the cache is removed once
/// and BOTH callers are served — the teardown asks for the absence of the cache, so the
/// racer that arrives second finds what it asked for. Asserted after concurrent updates
/// too, so a reconfigured cache is removable by the same rule.
#[tokio::test]
async fn two_racing_deletes_remove_the_cache_once_and_both_are_served() {
    let router = router();

    // (case, how many PAIRS of simultaneous updates race before the deletes do)
    let rows: &[(&str, usize)] = &[
        ("no update before the deletes", 0),
        ("one pair of racing updates before the deletes", 1),
        ("two pairs of racing updates before the deletes", 2),
    ];

    for (index, (case, racing_rounds)) in rows.iter().enumerate() {
        let cache_type = CACHE_TYPES[index % CACHE_TYPES.len()];
        let behavior = CACHING_BEHAVIORS[index % CACHING_BEHAVIORS.len()];
        let health = HEALTH_METRICS[index % HEALTH_METRICS.len()];
        let case = format!("{case} ({cache_type} caching {behavior}, metrics {health})");
        let api_id = create_api(&router, &format!("Api_cache_race_{index}")).await;
        let created = create_cache(&router, &api_id, 300, behavior, cache_type, health).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected the create to be accepted, got {created:?}"
        );
        tokio::time::sleep(CACHE_SPAN + SLACK).await;

        // The racing updates: each is served, and none of them leaves the cache in a
        // state the delete below cannot remove.
        for _ in 0..*racing_rounds {
            two_racing_updates(&router, &api_id, 600, behavior, cache_type, &case).await;
        }

        let (first, second) = tokio::join!(
            delete_cache(&router, &api_id),
            delete_cache(&router, &api_id)
        );
        for (racer, answer) in [&first, &second].into_iter().enumerate() {
            assert_eq!(
                answer.status, 200,
                "case {case:?} racing delete {racer}: expected status 200, got {answer:?}"
            );
            assert!(
                carries_no_members(&answer.body),
                "case {case:?} racing delete {racer}: expected a response with no members, \
                 got {answer:?}"
            );
        }

        // The cache is gone for every later caller, whichever racer removed it — and the
        // racer that arrived second removed nothing, so the absence a reader meets is
        // still the one a torn-down cache leaves behind.
        let gone = get_cache(&router, &api_id).await;
        assert_eq!(
            gone.status, 404,
            "case {case:?}: expected the cache to be gone, got {gone:?}"
        );
        assert_eq!(
            gone.message, DELETED_MESSAGE,
            "case {case:?}: expected {DELETED_MESSAGE:?}, got {gone:?}"
        );
    }
}
