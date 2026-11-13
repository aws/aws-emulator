// SPDX-License-Identifier: CC0-1.0

//! The ONE hierarchy-label rule, driven through every operation that addresses a
//! hierarchy by label: `AssociateAssets`, `ListAssociatedAssets` and
//! `DisassociateAssets`.
//!
//! The rule: a `CustomID` label names the stored `(id, externalId)` pair whose raw
//! id equals the label, or — when the label carries the `externalId:` prefix — whose
//! external id equals the text after that prefix; the two spaces are separate, so
//! neither form falls back to the other. `crate::state::matches_id_label` is the one
//! place that comparison is made, and these suites pin that each of the three
//! operations resolves the SAME label to the SAME hierarchy, so a change to the rule
//! cannot leave associate, list and disassociate addressing different hierarchies.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The crate's shared harness: the request walk and the error-code reader live in
// `asset_lifecycle`, the create/associate walk in `asset_hierarchies`, so this suite
// reaches both rather than growing a second copy.
use crate::asset_hierarchies::{associate, create_asset, create_model};
use crate::asset_lifecycle::{call, error_code, router_with_engine};
// The answer reader of the suite that owns `ListAssociatedAssets`: ONE reader of an
// `assetSummaries` answer, as this fix leaves one definition of the label rule.
use crate::associated_assets::summary_ids;

/// Hierarchy ids this suite supplies itself, so a label can be written down before
/// anything is created. `AssetModelHierarchyDefinition.id` is a caller-suppliable
/// `ID` ("if you prefer to supply your own ID"), per smithy_query on that member.
const ALPHA_ID: &str = "a1a1a1a1-1111-2222-3333-444444444444";
const BETA_ID: &str = "b2b2b2b2-1111-2222-3333-444444444444";
const GAMMA_ID: &str = "c3c3c3c3-1111-2222-3333-444444444444";
/// A well-formed `ID` that names no stored hierarchy.
const ABSENT_ID: &str = "d4d4d4d4-1111-2222-3333-444444444444";

/// The external id of the `alpha` hierarchy.
const ALPHA_EXTERNAL: &str = "alpha-wing";

/// `GET /assets/{assetId}/hierarchies?hierarchyId={label}`.
async fn list_associated(
    router: &MultiServiceRouter,
    asset: &str,
    hierarchy_label: &str,
) -> (u16, Value) {
    call(
        router,
        "GET",
        &format!("/assets/{asset}/hierarchies?hierarchyId={hierarchy_label}"),
        None,
    )
    .await
}

/// `POST /assets/{assetId}/disassociate`.
async fn disassociate(
    router: &MultiServiceRouter,
    parent: &str,
    hierarchy_label: &str,
    child: &str,
) -> (u16, Value) {
    call(
        router,
        "POST",
        &format!("/assets/{parent}/disassociate"),
        Some(json!({ "hierarchyId": hierarchy_label, "childAssetId": child })),
    )
    .await
}

/// A parent asset whose model declares three hierarchies over one child model, and
/// `children` free child assets to associate under them.
struct Fixture {
    router: MultiServiceRouter,
    parent: String,
    children: Vec<String>,
}

impl Fixture {
    async fn build(children: usize) -> Self {
        let (router, _engine) = router_with_engine();
        let turbine = create_model(
            &router,
            json!({
                "assetModelName": "LabelTurbine",
                "assetModelExternalId": "label-turbine-model",
            }),
        )
        .await;
        // `alpha` carries both label forms; `beta`'s EXTERNAL id is `alpha`'s RAW id,
        // which `ExternalId` admits (smithy_query on ExternalId: pattern
        // ^[a-zA-Z0-9_][a-zA-Z_\-0-9.:]*[a-zA-Z0-9_]+$), so the two label spaces are
        // observably separate; `gamma` carries no external id at all.
        let farm = create_model(
            &router,
            json!({
                "assetModelName": "LabelFarm",
                "assetModelExternalId": "label-farm-model",
                "assetModelHierarchies": [
                    {
                        "name": "alpha",
                        "id": ALPHA_ID,
                        "externalId": ALPHA_EXTERNAL,
                        "childAssetModelId": &turbine,
                    },
                    {
                        "name": "beta",
                        "id": BETA_ID,
                        "externalId": ALPHA_ID,
                        "childAssetModelId": &turbine,
                    },
                    {"name": "gamma", "id": GAMMA_ID, "childAssetModelId": &turbine},
                ],
            }),
        )
        .await;

        let parent = create_asset(
            &router,
            json!({
                "assetName": "LabelFarm1",
                "assetModelId": &farm,
                "assetExternalId": "label-farm-one",
            }),
        )
        .await;
        let mut assets = Vec::new();
        for at in 0..children {
            assets.push(
                create_asset(
                    &router,
                    json!({
                        "assetName": format!("LabelTurbine{at}"),
                        "assetModelId": &turbine,
                    }),
                )
                .await,
            );
        }

        Self {
            router,
            parent,
            children: assets,
        }
    }
}

#[tokio::test]
async fn one_label_names_one_hierarchy_in_associate_list_and_disassociate() {
    // Rows differ along what the rule generalises over: which label form is used,
    // and which of the two id spaces the text lives in. The expected column is the
    // stored hierarchy id the label must name, in every operation.
    let cases: [(&str, String, &str); 4] = [
        (
            "the raw id of a hierarchy that also carries an external id",
            ALPHA_ID.to_string(),
            ALPHA_ID,
        ),
        (
            "the externalId form of that same hierarchy",
            format!("externalId:{ALPHA_EXTERNAL}"),
            ALPHA_ID,
        ),
        (
            "an externalId form whose text is another hierarchy's raw id",
            format!("externalId:{ALPHA_ID}"),
            BETA_ID,
        ),
        (
            "the raw id of a hierarchy that carries no external id",
            GAMMA_ID.to_string(),
            GAMMA_ID,
        ),
    ];

    let fixture = Fixture::build(cases.len()).await;
    for (at, (case, label, expected)) in cases.into_iter().enumerate() {
        let child = fixture.children[at].clone();

        // AssociateAssets resolves the label and records the edge under whatever
        // hierarchy it named.
        let (status, body) = associate(&fixture.router, &fixture.parent, &label, &child).await;
        assert_eq!(
            status, 200,
            "case {case:?}: associating under {label:?} expected 200, got {status} {body}"
        );

        // Reading the edge back by RAW hierarchy id is what says which hierarchy the
        // label named: exactly the expected one holds the child, the others none.
        for hierarchy in [ALPHA_ID, BETA_ID, GAMMA_ID] {
            let (status, body) = list_associated(&fixture.router, &fixture.parent, hierarchy).await;
            assert_eq!(
                status, 200,
                "case {case:?}: listing {hierarchy} expected 200, got {status} {body}"
            );
            let ids = summary_ids(&body);
            let want: Vec<String> = if hierarchy == expected {
                vec![child.clone()]
            } else {
                Vec::new()
            };
            assert_eq!(
                ids, want,
                "case {case:?}: hierarchy {hierarchy} expected {want:?}, got {ids:?}"
            );
        }

        // ListAssociatedAssets resolves the SAME label to the same hierarchy, so the
        // label form answers exactly what the raw id answered.
        let (status, body) = list_associated(&fixture.router, &fixture.parent, &label).await;
        assert_eq!(
            status, 200,
            "case {case:?}: listing {label:?} expected 200, got {status} {body}"
        );
        let ids = summary_ids(&body);
        assert_eq!(
            ids,
            vec![child.clone()],
            "case {case:?}: listing {label:?} expected {:?}, got {ids:?}",
            vec![child.clone()]
        );

        // DisassociateAssets resolves it too, so the edge the label recorded is the
        // edge the same label removes.
        let (status, body) = disassociate(&fixture.router, &fixture.parent, &label, &child).await;
        assert_eq!(
            status, 200,
            "case {case:?}: disassociating {label:?} expected 200, got {status} {body}"
        );
        let (status, body) = list_associated(&fixture.router, &fixture.parent, expected).await;
        assert_eq!(
            status, 200,
            "case {case:?}: listing {expected} after removal expected 200, got {status} {body}"
        );
        let ids = summary_ids(&body);
        assert!(
            ids.is_empty(),
            "case {case:?}: {expected} must hold nothing after removal, got {ids:?}"
        );
    }
}

#[tokio::test]
async fn a_label_naming_no_hierarchy_is_refused_by_every_operation() {
    let fixture = Fixture::build(1).await;
    let child = fixture.children[0].clone();

    // Rows span the ways a label can name no hierarchy: an id nothing declares, an
    // external id nothing declares, and — the half of the rule that keeps the two
    // spaces separate — the externalId form of a raw id that IS a stored hierarchy's
    // id (`alpha`'s neighbour `beta` is reached that way, so these two must be the
    // hierarchies that carry the text as an external id, not as an id).
    let cases: [(&str, String); 4] = [
        ("an id no hierarchy declares", ABSENT_ID.to_string()),
        (
            "an external id no hierarchy declares",
            "externalId:no-such-wing".to_string(),
        ),
        (
            "the externalId form of a hierarchy id no hierarchy carries externally",
            format!("externalId:{GAMMA_ID}"),
        ),
        (
            "the externalId form of the raw id of the hierarchy itself",
            format!("externalId:{BETA_ID}"),
        ),
    ];

    for (case, label) in cases {
        // `AssociateAssets` states a hierarchy the parent's model does not declare
        // as a statement about the REQUEST: a hierarchy is a grouping of that
        // model, so the caller is told its request names a grouping this asset has
        // not got. The read and the removal state the hierarchy's absence.
        let (status, body) = associate(&fixture.router, &fixture.parent, &label, &child).await;
        assert_eq!(
            status, 400,
            "case {case:?}: associating under {label:?} expected 400, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidRequestException",
            "case {case:?}: associating under {label:?} expected InvalidRequestException, \
             got {body}"
        );

        let (status, body) = list_associated(&fixture.router, &fixture.parent, &label).await;
        assert_eq!(
            status, 404,
            "case {case:?}: listing {label:?} expected 404, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: listing {label:?} expected ResourceNotFoundException, got {body}"
        );

        let (status, body) = disassociate(&fixture.router, &fixture.parent, &label, &child).await;
        assert_eq!(
            status, 404,
            "case {case:?}: disassociating {label:?} expected 404, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: disassociating {label:?} expected ResourceNotFoundException, \
             got {body}"
        );
    }

    // Every refusal above persisted nothing, so each hierarchy still holds nothing.
    for hierarchy in [ALPHA_ID, BETA_ID, GAMMA_ID] {
        let (status, body) = list_associated(&fixture.router, &fixture.parent, hierarchy).await;
        assert_eq!(
            status, 200,
            "case {hierarchy:?}: listing expected 200, got {status} {body}"
        );
        let ids = summary_ids(&body);
        assert!(
            ids.is_empty(),
            "case {hierarchy:?}: a refused call must persist nothing, got {ids:?}"
        );
    }
}
