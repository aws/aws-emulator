// SPDX-License-Identifier: CC0-1.0

//! The rule this suite pins for `AssociateAssets`:
//!
//! an association the hierarchy TREE already holds for the child is a TERMINAL
//! request error — HTTP 400 `InvalidRequestException` whose `message` is the one
//! fixed sentence "Child asset already has a parent", with no variable parts, so
//! no asset, hierarchy or model identifier of the request or of the store appears
//! in it — and it is the answer whichever way the child already has a parent (the
//! repeat of the very same edge, another hierarchy of the same parent, or another
//! parent entirely), whichever `CustomID` label form each member of the triple was
//! written with, however recently that parent was recorded, and however many
//! callers asked at once.
//!
//! The sibling `asset_hierarchies` suite covers the single-caller repeat; this one
//! spans the label forms, the collision kinds and the CONCURRENT case, where every
//! loser of the race must end on this 400 rather than on a retry that never
//! clears.
//!
//! Everything is driven over HTTP through the same `MultiServiceRouter` the
//! emulator boots with, so every assertion is about the wire behaviour a real
//! client sees.

use std::sync::Arc;
use std::time::Duration;

use serde_json::json;
use serde_json::Value;
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_iotsitewise_awsiotsitewise::state::service_state;
use service_com_amazonaws_iotsitewise_awsiotsitewise::EmulatedService;

use crate::asset_hierarchies::{associate, create_asset, create_model, hierarchy_id};
use crate::asset_lifecycle::error_code;
// The ONE copy of the fixed sentence a test may compare against lives in the
// sibling suite, so the two cannot drift from each other.
use crate::asset_hierarchies::CHILD_ALREADY_HAS_A_PARENT;

/// How long a racer waits before repeating a call the emulator refused with a
/// retryable status, and how many times it may repeat it — so a racer that keeps
/// retrying reaches a terminal answer rather than giving up on a transient one.
const RETRY_BACKOFF: Duration = Duration::from_millis(50);
const MAX_RETRIES: usize = 40;

/// A router with just this service mounted on a fresh shared engine, shareable
/// across concurrently spawned callers.
fn shared_router() -> (Arc<MultiServiceRouter>, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    (Arc::new(router), engine)
}

/// The resources one of these tests associates over.
struct Farm {
    /// The parent asset the child is associated under, and a second parent of the
    /// same model, so "another parent" is a row this suite can ask for.
    farm: String,
    other_farm: String,
    /// Two hierarchies of the parent's model over the child's model, so "another
    /// hierarchy of the same parent" is a row too.
    turbines: String,
    spares: String,
    /// The child asset.
    turbine: String,
    /// Every identifier the store minted or the request supplied, which the fixed
    /// sentence must contain none of.
    identifiers: Vec<String>,
}

/// Two asset models — a leaf `Turbine` model and a `Farm` model with two
/// hierarchies over it — plus two farm assets and one turbine asset. Every
/// resource also carries an external id, so each member of the triple can be named
/// by either `CustomID` label form.
async fn farm_fixture(router: &MultiServiceRouter) -> Farm {
    let turbine_model = create_model(
        router,
        json!({"assetModelName": "Turbine", "assetModelExternalId": "turbine-model"}),
    )
    .await;
    let farm_model = create_model(
        router,
        json!({
            "assetModelName": "Farm",
            "assetModelExternalId": "farm-model",
            "assetModelHierarchies": [
                {"name": "turbines", "externalId": "farm-turbines", "childAssetModelId": turbine_model},
                {"name": "spares", "externalId": "farm-spares", "childAssetModelId": "externalId:turbine-model"},
            ],
        }),
    )
    .await;
    let turbines = hierarchy_id(router, &farm_model, "turbines").await;
    let spares = hierarchy_id(router, &farm_model, "spares").await;

    let farm = create_asset(
        router,
        json!({
            "assetName": "Farm 1",
            "assetModelId": farm_model,
            "assetExternalId": "farm-1",
        }),
    )
    .await;
    let other_farm = create_asset(
        router,
        json!({
            "assetName": "Farm 2",
            "assetModelId": farm_model,
            "assetExternalId": "farm-2",
        }),
    )
    .await;
    let turbine = create_asset(
        router,
        json!({
            "assetName": "Turbine 1",
            "assetModelId": turbine_model,
            "assetExternalId": "turbine-1",
        }),
    )
    .await;

    let identifiers = vec![
        turbine_model,
        farm_model,
        turbines.clone(),
        spares.clone(),
        farm.clone(),
        other_farm.clone(),
        turbine.clone(),
        "turbine-model".to_string(),
        "farm-model".to_string(),
        "farm-turbines".to_string(),
        "farm-spares".to_string(),
        "farm-1".to_string(),
        "farm-2".to_string(),
        "turbine-1".to_string(),
    ];
    Farm {
        farm,
        other_farm,
        turbines,
        spares,
        turbine,
        identifiers,
    }
}

/// Assert `body` is the fixed already-parented sentence and nothing else: the exact
/// text, and — independently of that equality — none of `identifiers` interpolated
/// into it, which is what "the message has no variable parts" means.
fn assert_fixed_sentence(case: &str, status: u16, body: &Value, identifiers: &[String]) {
    assert_eq!(
        status, 400,
        "case {case:?}: expected 400, got {status} {body}"
    );
    assert_eq!(
        error_code(body),
        "InvalidRequestException",
        "case {case:?}: expected InvalidRequestException, got {body}"
    );
    let message = body["message"].as_str().unwrap_or_default();
    assert_eq!(
        message, CHILD_ALREADY_HAS_A_PARENT,
        "case {case:?}: expected the message {CHILD_ALREADY_HAS_A_PARENT:?}, got {message:?}"
    );
    for identifier in identifiers {
        assert!(
            !message.contains(identifier.as_str()),
            "case {case:?}: the message must carry no identifier, but {identifier:?} \
             appears in {message:?}"
        );
    }
}

/// How many times `child` appears as a child anywhere in the stored hierarchy
/// index, and the `(parent, hierarchy)` it appears under.
///
/// An INDEPENDENT reader of the store: it walks the public index itself rather than
/// calling the helper the handler decides through.
fn stored_parents(shared: &engine::InMemoryEngine, child: &str) -> Vec<(String, String)> {
    let state = service_state(shared).expect("the service store");
    let store = state.lock();
    store
        .asset_hierarchy_children
        .iter()
        .filter(|(_, children)| children.iter().any(|stored| stored == child))
        .map(|((parent, hierarchy), _)| (parent.clone(), hierarchy.clone()))
        .collect()
}

#[tokio::test]
async fn every_already_parented_refusal_carries_the_one_fixed_sentence() {
    let (router, engine) = shared_router();
    let farm = farm_fixture(&router).await;

    let (status, body) = associate(&router, &farm.farm, &farm.turbines, &farm.turbine).await;
    assert_eq!(status, 200, "the first association: got {status} {body}");

    // Rows: the ways the child already has a parent (the same edge again, another
    // hierarchy of the same parent, another parent of the same model), crossed with
    // the `CustomID` label forms the modelled members accept — a raw UUID or an
    // `externalId:` alias — varied independently across the parent, the hierarchy
    // and the child. Every row is asked straight after the association that made
    // the child somebody's child, because that fact does not depend on how long
    // ago it was recorded.
    let cases: Vec<(&str, String, String, String)> = vec![
        (
            "the edge again, all raw ids",
            farm.farm.clone(),
            farm.turbines.clone(),
            farm.turbine.clone(),
        ),
        (
            "the edge again, parent by externalId",
            "externalId:farm-1".to_string(),
            farm.turbines.clone(),
            farm.turbine.clone(),
        ),
        (
            "the edge again, hierarchy by externalId",
            farm.farm.clone(),
            "externalId:farm-turbines".to_string(),
            farm.turbine.clone(),
        ),
        (
            "the edge again, child by externalId",
            farm.farm.clone(),
            farm.turbines.clone(),
            "externalId:turbine-1".to_string(),
        ),
        (
            "the edge again, every label by externalId",
            "externalId:farm-1".to_string(),
            "externalId:farm-turbines".to_string(),
            "externalId:turbine-1".to_string(),
        ),
        (
            "another hierarchy of the same parent",
            farm.farm.clone(),
            farm.spares.clone(),
            farm.turbine.clone(),
        ),
        (
            "another hierarchy of the same parent, by externalId",
            "externalId:farm-1".to_string(),
            "externalId:farm-spares".to_string(),
            "externalId:turbine-1".to_string(),
        ),
        (
            "another parent of the same model",
            farm.other_farm.clone(),
            farm.turbines.clone(),
            farm.turbine.clone(),
        ),
        (
            "another parent of the same model, by externalId",
            "externalId:farm-2".to_string(),
            "externalId:farm-turbines".to_string(),
            "externalId:turbine-1".to_string(),
        ),
    ];
    for (case, parent, hierarchy, child) in &cases {
        let (status, body) = associate(&router, parent, hierarchy, child).await;
        assert_fixed_sentence(case, status, &body, &farm.identifiers);
    }

    // Every row above persisted nothing: the child is still the one child of the
    // one hierarchy that accepted it.
    let parents = stored_parents(&engine, &farm.turbine);
    assert_eq!(
        parents,
        vec![(farm.farm.clone(), farm.turbines.clone())],
        "a refused association must persist nothing, got {parents:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_identical_associations_leave_one_winner_and_fixed_sentence_losers() {
    // Rows differ along how many callers race for the one edge; the rule is the
    // same for each, and one caller is the degenerate row where nobody loses.
    for racers in [1usize, 2, 6] {
        let case = format!("{racers} concurrent callers");
        let (router, engine) = shared_router();
        let farm = farm_fixture(&router).await;

        let mut running = Vec::with_capacity(racers);
        for _ in 0..racers {
            let router = Arc::clone(&router);
            let parent = farm.farm.clone();
            let hierarchy = farm.turbines.clone();
            let child = farm.turbine.clone();
            running.push(tokio::spawn(async move {
                // A client retries a 409: it would say another operation on the
                // same resource is still in flight, so a repeat could succeed.
                // This operation no longer answers one for a repeated
                // association, so the loop is what proves it: whatever the call
                // settles on is the terminal answer, reached without a retry
                // that never clears.
                for _ in 0..MAX_RETRIES {
                    let (status, body) = associate(&router, &parent, &hierarchy, &child).await;
                    if status != 409 {
                        return (status, body);
                    }
                    tokio::time::sleep(RETRY_BACKOFF).await;
                }
                associate(&router, &parent, &hierarchy, &child).await
            }));
        }

        let mut outcomes = Vec::with_capacity(racers);
        for handle in running {
            outcomes.push(
                handle
                    .await
                    .unwrap_or_else(|err| panic!("case {case:?}: a caller panicked: {err}")),
            );
        }

        let winners = outcomes
            .iter()
            .filter(|(status, _)| *status == 200)
            .count();
        assert_eq!(
            winners, 1,
            "case {case:?}: exactly one caller may record the association, got {outcomes:?}"
        );
        for (status, body) in outcomes.iter().filter(|(status, _)| *status != 200) {
            assert_fixed_sentence(&case, *status, body, &farm.identifiers);
        }

        // And the child ended up associated exactly once, under the hierarchy every
        // caller asked for.
        let parents = stored_parents(&engine, &farm.turbine);
        assert_eq!(
            parents,
            vec![(farm.farm.clone(), farm.turbines.clone())],
            "case {case:?}: the child must be associated exactly once, got {parents:?}"
        );
    }
}
