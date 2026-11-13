// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `AssociateAssets`, driven over HTTP through the same
//! `MultiServiceRouter` the emulator boots with, so every assertion is about the
//! wire behaviour a real client sees.
//!
//! The rule these suites pin: `AssociateAssets` records a parent -> child edge of
//! the asset hierarchy TREE in the shared store, and accepts the request exactly
//! when the stored resources allow that edge — the parent, the child and the
//! hierarchy all resolve (by raw id or `externalId:` alias), the child is an
//! instance of the hierarchy's `childAssetModelId`, and the child is not already
//! somebody's child. Anything else is a modelled refusal that persists nothing.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_iotsitewise_awsiotsitewise::state::service_state;

// The crate's shared test harness lives in the `asset_lifecycle` suite; this file
// reaches those helpers rather than growing a second copy of them.
use crate::asset_lifecycle::{call, error_code, router_with_engine};

/// A UUID in the modelled `ID` form that names no stored resource.
const ABSENT_ID: &str = "11111111-2222-3333-4444-555555555555";

/// The `InvalidRequestException` message `AssociateAssets` refuses a child that
/// already has a parent with — the association repeated once it is established,
/// and a child that hangs under any other parent or hierarchy, which are one
/// fact about the child.
///
/// Entirely FIXED text: it carries no asset, hierarchy or model id, so no value
/// of the request or of the store fills it.
pub(crate) const CHILD_ALREADY_HAS_A_PARENT: &str = "Child asset already has a parent";

/// Create an asset model from `body` and return its id. Unlike the harness's
/// `create_model`, this takes the whole request body: these suites need
/// `assetModelHierarchies` and `assetModelExternalId`.
pub(crate) async fn create_model(router: &MultiServiceRouter, body: Value) -> String {
    let (status, parsed) = call(router, "POST", "/asset-models", Some(body.clone())).await;
    assert_eq!(status, 202, "create model {body}: got {status} {parsed}");
    parsed["assetModelId"]
        .as_str()
        .unwrap_or_else(|| panic!("create model {body}: no assetModelId in {parsed}"))
        .to_string()
}

/// Create an asset from `body` and return its id.
pub(crate) async fn create_asset(router: &MultiServiceRouter, body: Value) -> String {
    let (status, parsed) = call(router, "POST", "/assets", Some(body.clone())).await;
    assert_eq!(status, 202, "create asset {body}: got {status} {parsed}");
    parsed["assetId"]
        .as_str()
        .unwrap_or_else(|| panic!("create asset {body}: no assetId in {parsed}"))
        .to_string()
}

/// The minted id of the `name` hierarchy of the stored asset model `model_id`, read
/// back through `DescribeAssetModel`.
pub(crate) async fn hierarchy_id(router: &MultiServiceRouter, model_id: &str, name: &str) -> String {
    let (status, body) = call(router, "GET", &format!("/asset-models/{model_id}"), None).await;
    assert_eq!(status, 200, "describe model {model_id}: got {status} {body}");
    body["assetModelHierarchies"]
        .as_array()
        .unwrap_or_else(|| panic!("model {model_id} reports no hierarchies in {body}"))
        .iter()
        .find(|hierarchy| hierarchy["name"] == name)
        .and_then(|hierarchy| hierarchy["id"].as_str())
        .unwrap_or_else(|| panic!("model {model_id} has no hierarchy {name:?} in {body}"))
        .to_string()
}

/// `POST /assets/{assetId}/associate`.
pub(crate) async fn associate(
    router: &MultiServiceRouter,
    parent: &str,
    hierarchy: &str,
    child: &str,
) -> (u16, Value) {
    call(
        router,
        "POST",
        &format!("/assets/{parent}/associate"),
        Some(json!({ "hierarchyId": hierarchy, "childAssetId": child })),
    )
    .await
}

/// The asset ids `ListAssets` reports for `query`.
async fn list_asset_ids(router: &MultiServiceRouter, query: &str) -> Vec<String> {
    let (status, body) = call(router, "GET", &format!("/assets?{query}"), None).await;
    assert_eq!(status, 200, "list assets {query}: got {status} {body}");
    body["assetSummaries"]
        .as_array()
        .unwrap_or_else(|| panic!("list assets {query}: no assetSummaries in {body}"))
        .iter()
        .map(|summary| summary["id"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// Two asset models: a leaf `Turbine` model, and a `Farm` model with two
/// hierarchies over it. The second hierarchy names its child model by the
/// `externalId:` alias, so the stored `childAssetModelId` is the resolved id either
/// way. Returns `(turbineModelId, farmModelId)`.
async fn farm_models(router: &MultiServiceRouter) -> (String, String) {
    let turbine = create_model(
        router,
        json!({
            "assetModelName": "Turbine",
            "assetModelExternalId": "turbine-model",
        }),
    )
    .await;
    let farm = create_model(
        router,
        json!({
            "assetModelName": "Farm",
            "assetModelExternalId": "farm-model",
            "assetModelHierarchies": [
                {"name": "turbines", "externalId": "farm-turbines", "childAssetModelId": turbine},
                {"name": "spares", "childAssetModelId": "externalId:turbine-model"},
            ],
        }),
    )
    .await;
    (turbine, farm)
}

#[tokio::test]
async fn associating_a_child_records_it_under_the_named_hierarchy() {
    let (router, engine) = router_with_engine();
    let (turbine_model, farm_model) = farm_models(&router).await;
    let turbines = hierarchy_id(&router, &farm_model, "turbines").await;

    let farm = create_asset(
        &router,
        json!({
            "assetName": "Farm 1",
            "assetModelId": farm_model,
            "assetExternalId": "farm-1",
        }),
    )
    .await;
    let first = create_asset(
        &router,
        json!({"assetName": "Turbine 1", "assetModelId": turbine_model}),
    )
    .await;
    let second = create_asset(
        &router,
        json!({"assetName": "Turbine 2", "assetModelId": "externalId:turbine-model"}),
    )
    .await;
    let third = create_asset(
        &router,
        json!({
            "assetName": "Turbine 3",
            "assetModelId": turbine_model,
            "assetExternalId": "turbine-3",
        }),
    )
    .await;

    // Rows: the same kind of association asked for with each form of `CustomID`
    // label the modelled members accept — a raw UUID or an `externalId:` alias —
    // varied independently across the parent, the hierarchy and the child.
    let cases: Vec<(&str, String, String, String)> = vec![
        ("all raw ids", farm.clone(), turbines.clone(), first.clone()),
        (
            "parent by externalId",
            "externalId:farm-1".to_string(),
            turbines.clone(),
            second.clone(),
        ),
        (
            "hierarchy and child by externalId",
            farm.clone(),
            "externalId:farm-turbines".to_string(),
            "externalId:turbine-3".to_string(),
        ),
    ];
    for (case, parent, hierarchy, child) in &cases {
        let (status, body) = associate(&router, parent, hierarchy, child).await;
        assert_eq!(status, 200, "case {case:?}: expected 200, got {status} {body}");
        assert_eq!(
            body,
            Value::Null,
            "case {case:?}: the modelled output is empty, got {body}"
        );
    }

    // Persisted against the RESOLVED ids, in association order, whichever label
    // each row used.
    {
        let state = service_state(&*engine).expect("the service store");
        let store = state.lock();
        let children = children_of(&store, &farm, &turbines);
        assert_eq!(
            children,
            vec![first.clone(), second.clone(), third.clone()],
            "every associated child must be stored under (parent, hierarchy) in \
             association order, got {children:?}"
        );
        for child in [&first, &second, &third] {
            let parent = parent_association(&store, child);
            assert_eq!(
                parent,
                Some((farm.clone(), turbines.clone())),
                "case {child:?}: expected the farm as parent, got {parent:?}"
            );
        }
        assert!(
            parent_association(&store, &farm).is_none(),
            "the parent asset must stay nobody's child, got {:?}",
            parent_association(&store, &farm)
        );
    }

    // And the association is observable through an implemented reader: a child is
    // no longer TOP_LEVEL, while every asset of the model is still there.
    let top_level = list_asset_ids(&router, "filter=TOP_LEVEL").await;
    assert_eq!(
        top_level,
        vec![farm.clone()],
        "only the unassociated farm is TOP_LEVEL, got {top_level:?}"
    );
    let mut all = list_asset_ids(&router, &format!("filter=ALL&assetModelId={turbine_model}")).await;
    all.sort();
    let mut expected = vec![first.clone(), second.clone(), third.clone()];
    expected.sort();
    assert_eq!(
        all, expected,
        "every turbine is still listed under ALL, got {all:?}"
    );
}

#[tokio::test]
async fn refuses_an_association_the_stored_definitions_do_not_allow() {
    let (router, engine) = router_with_engine();
    let (turbine_model, farm_model) = farm_models(&router).await;
    let turbines = hierarchy_id(&router, &farm_model, "turbines").await;
    // A third model, so a candidate child can be an instance of some model OTHER
    // than the hierarchy's `childAssetModelId`.
    let gearbox_model = create_model(&router, json!({"assetModelName": "Gearbox"})).await;

    let farm = create_asset(
        &router,
        json!({"assetName": "Farm 1", "assetModelId": farm_model}),
    )
    .await;
    let turbine = create_asset(
        &router,
        json!({"assetName": "Turbine 1", "assetModelId": turbine_model}),
    )
    .await;
    let gearbox = create_asset(
        &router,
        json!({"assetName": "Gearbox 1", "assetModelId": gearbox_model}),
    )
    .await;

    // Rows: one per way a stored definition can refuse the edge, each at both the
    // raw-id and the `externalId:`-alias end of the label forms where it applies.
    // An asset the request names but the store does not hold is the absent
    // RESOURCE (404); a hierarchy the parent's model does not declare is a
    // statement about the REQUEST (400), since a hierarchy is a grouping of that
    // model rather than a resource of its own.
    let cases: Vec<(&str, String, String, String, u16, &str)> = vec![
        (
            "parent names no stored asset",
            ABSENT_ID.to_string(),
            turbines.clone(),
            turbine.clone(),
            404,
            "ResourceNotFoundException",
        ),
        (
            "parent alias names no stored asset",
            "externalId:no-such-farm".to_string(),
            turbines.clone(),
            turbine.clone(),
            404,
            "ResourceNotFoundException",
        ),
        (
            "child names no stored asset",
            farm.clone(),
            turbines.clone(),
            ABSENT_ID.to_string(),
            404,
            "ResourceNotFoundException",
        ),
        (
            "hierarchy is not defined by the parent's model",
            farm.clone(),
            ABSENT_ID.to_string(),
            turbine.clone(),
            400,
            "InvalidRequestException",
        ),
        (
            "hierarchy alias is not defined by the parent's model",
            farm.clone(),
            "externalId:no-such-hierarchy".to_string(),
            turbine.clone(),
            400,
            "InvalidRequestException",
        ),
        (
            "child is an instance of another asset model",
            farm.clone(),
            turbines.clone(),
            gearbox.clone(),
            400,
            "InvalidRequestException",
        ),
        (
            "child is an instance of the parent's own model",
            farm.clone(),
            turbines.clone(),
            farm.clone(),
            400,
            "InvalidRequestException",
        ),
    ];
    for (case, parent, hierarchy, child, expected_status, expected_code) in &cases {
        let (status, body) = associate(&router, parent, hierarchy, child).await;
        assert_eq!(
            status, *expected_status,
            "case {case:?}: expected {expected_status}, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            *expected_code,
            "case {case:?}: unexpected error code in {body}"
        );
    }

    // A refusal persists nothing: no row above made anybody a child.
    let state = service_state(&*engine).expect("the service store");
    let store = state.lock();
    for asset in [&farm, &turbine, &gearbox] {
        let parent = parent_association(&store, asset);
        assert!(
            parent.is_none(),
            "case {asset:?}: a refused association must store nothing, got {parent:?}"
        );
    }
}

#[tokio::test]
async fn a_child_is_associated_to_one_parent_hierarchy_at_a_time() {
    let (router, _engine) = router_with_engine();
    let (turbine_model, farm_model) = farm_models(&router).await;
    let turbines = hierarchy_id(&router, &farm_model, "turbines").await;
    let spares = hierarchy_id(&router, &farm_model, "spares").await;

    let farm = create_asset(
        &router,
        json!({"assetName": "Farm 1", "assetModelId": farm_model}),
    )
    .await;
    let other_farm = create_asset(
        &router,
        json!({"assetName": "Farm 2", "assetModelId": farm_model}),
    )
    .await;
    let turbine = create_asset(
        &router,
        json!({"assetName": "Turbine 1", "assetModelId": turbine_model}),
    )
    .await;

    let (status, body) = associate(&router, &farm, &turbines, &turbine).await;
    assert_eq!(status, 200, "first association: got {status} {body}");

    // Rows: the three ways a second association collides with the first — the same
    // edge again, another hierarchy of the same parent, and another parent. Every
    // one of them is the same fact about the child (it already has a parent) and so
    // the same terminal statement about the request: 400 `InvalidRequestException`
    // carrying the one fixed sentence. How recently the first association was
    // recorded does not enter into it.
    let cases: Vec<(&str, String, String)> = vec![
        (
            "the same hierarchy again",
            farm.clone(),
            turbines.clone(),
        ),
        (
            "another hierarchy of the same parent",
            farm.clone(),
            spares.clone(),
        ),
        (
            "another parent's hierarchy",
            other_farm.clone(),
            turbines.clone(),
        ),
    ];
    for (case, parent, hierarchy) in &cases {
        let (status, body) = associate(&router, parent, hierarchy, &turbine).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidRequestException",
            "case {case:?}: unexpected error code in {body}"
        );
        // The child already has a parent, whichever of the three ways this row
        // collides with the first association, so every row carries the one
        // fixed sentence that states it.
        assert_eq!(
            body["message"].as_str(),
            Some(CHILD_ALREADY_HAS_A_PARENT),
            "case {case:?}: expected the message {CHILD_ALREADY_HAS_A_PARENT:?}, got {body}"
        );
    }

    // The first association survives every refusal above.
    let top_level = list_asset_ids(&router, "filter=TOP_LEVEL").await;
    let mut expected = vec![farm.clone(), other_farm.clone()];
    expected.sort();
    let mut actual = top_level.clone();
    actual.sort();
    assert_eq!(
        actual, expected,
        "the associated turbine must stay out of TOP_LEVEL, got {top_level:?}"
    );
}

#[tokio::test]
async fn associations_chain_through_several_levels() {
    let (router, engine) = router_with_engine();
    // Three models, each a hierarchy over the next: the deepest first, because a
    // hierarchy definition has to name a model that already exists.
    let turbine_model = create_model(&router, json!({"assetModelName": "Turbine"})).await;
    let farm_model = create_model(
        &router,
        json!({
            "assetModelName": "Farm",
            "assetModelHierarchies": [
                {"name": "turbines", "childAssetModelId": turbine_model},
            ],
        }),
    )
    .await;
    let region_model = create_model(
        &router,
        json!({
            "assetModelName": "Region",
            "assetModelHierarchies": [
                {"name": "farms", "childAssetModelId": farm_model},
            ],
        }),
    )
    .await;
    let turbines = hierarchy_id(&router, &farm_model, "turbines").await;
    let farms = hierarchy_id(&router, &region_model, "farms").await;

    let region = create_asset(
        &router,
        json!({"assetName": "Region 1", "assetModelId": region_model}),
    )
    .await;
    let farm = create_asset(
        &router,
        json!({"assetName": "Farm 1", "assetModelId": farm_model}),
    )
    .await;
    let turbine = create_asset(
        &router,
        json!({"assetName": "Turbine 1", "assetModelId": turbine_model}),
    )
    .await;

    // Rows: each level of the chain, associated bottom-up to show the order the
    // calls arrive in does not matter.
    let cases: Vec<(&str, String, String, String)> = vec![
        (
            "turbine under farm",
            farm.clone(),
            turbines.clone(),
            turbine.clone(),
        ),
        (
            "farm under region",
            region.clone(),
            farms.clone(),
            farm.clone(),
        ),
    ];
    for (case, parent, hierarchy, child) in &cases {
        let (status, body) = associate(&router, parent, hierarchy, child).await;
        assert_eq!(status, 200, "case {case:?}: expected 200, got {status} {body}");
    }

    let state = service_state(&*engine).expect("the service store");
    let store = state.lock();
    for (case, parent, hierarchy, child) in &cases {
        let stored = parent_association(&store, child);
        assert_eq!(
            stored,
            Some((parent.clone(), hierarchy.clone())),
            "case {case:?}: expected the stored edge, got {stored:?}"
        );
    }
    assert!(
        parent_association(&store, &region).is_none(),
        "the root of the chain is nobody's child, got {:?}",
        parent_association(&store, &region)
    );
}

/// The children stored under `(parent, hierarchy)`, as an owned list.
///
/// An INDEPENDENT reader of the store: the handler reaches the same index through
/// its own helper, so this walks `asset_hierarchy_children` itself rather than
/// calling the code under test.
fn children_of(
    store: &service_com_amazonaws_iotsitewise_awsiotsitewise::state::ComAmazonawsIotsitewiseAwsiotsitewiseState,
    parent: &str,
    hierarchy: &str,
) -> Vec<String> {
    store
        .asset_hierarchy_children
        .iter()
        .filter(|((stored_parent, stored_hierarchy), _)| {
            stored_parent == parent && stored_hierarchy == hierarchy
        })
        .flat_map(|(_, children)| children.iter().cloned())
        .collect()
}

/// The `(parent, hierarchy)` an asset is stored as a child of, if any.
fn parent_association(
    store: &service_com_amazonaws_iotsitewise_awsiotsitewise::state::ComAmazonawsIotsitewiseAwsiotsitewiseState,
    child: &str,
) -> Option<(String, String)> {
    store
        .asset_hierarchy_children
        .iter()
        .find(|(_, children)| children.iter().any(|stored| stored == child))
        .map(|((parent, hierarchy), _)| (parent.clone(), hierarchy.clone()))
}
