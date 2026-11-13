// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the dependency rule `DeleteAssetModel` enforces, driven
//! over HTTP through the same `MultiServiceRouter` the emulator boots with, so
//! every assertion is about the wire behaviour a real client sees.
//!
//! The rule these suites pin: an asset model is refused while a resource this
//! service still holds DEPENDS on it — an asset instantiating it, or another
//! asset model's composite model reusing it as its component model — the refusal
//! being a terminal `InvalidRequestException` (400) that names the dependent,
//! with the model and that dependent left exactly as they were; once the last
//! dependent is gone the same delete is accepted and the model reads back as
//! absent. A reference another model's DEFINITION makes to it — a hierarchy
//! naming it as its child model, a property formula reading through such a
//! hierarchy — is not such a dependency: the delete is accepted while one
//! stands, which the sibling `asset_hierarchy_refusal_classes` suite pins.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::asset_lifecycle::{call, create_asset, error_code, router};

/// Composite model ids the creates fix, so the expected refusal wording is known
/// to the test without asking the emulator. Each is a lowercase hyphenated UUID
/// with RFC 4122's version-4 nibble, which is what the modelled `ID` pattern
/// admits.
const COMPOSITE_IDS: [&str; 8] = [
    "11111111-1111-4111-8111-111111111111",
    "22222222-2222-4222-8222-222222222222",
    "33333333-3333-4333-8333-333333333333",
    "44444444-4444-4444-8444-444444444444",
    "55555555-5555-4555-8555-555555555555",
    "66666666-6666-4666-8666-666666666666",
    "77777777-7777-4777-8777-777777777777",
    "88888888-8888-4888-8888-888888888888",
];

/// The external id the reused component model carries, so the delete can also be
/// addressed by its `externalId:` label.
const COMPONENT_EXTERNAL_ID: &str = "the_component_model";

/// The property the reused component model lends to every composite model that
/// reuses it.
const LENT_PROPERTY: &str = "Flow";

/// One composite model the fixture creates on one of the holders.
struct Composite {
    /// Index into the row's holders of the asset model that holds it.
    holder: usize,
    /// The `assetModelCompositeModelId` the create asks for.
    id: &'static str,
    /// The `assetModelCompositeModelType` the create asks for. Catalog source:
    /// the `assetModelCompositeModelType` documentation, "Valid values are
    /// AWS/ALARM, CUSTOM, or AWS/L4E_ANOMALY".
    composite_type: &'static str,
    /// The composite model this one is nested inside, when it is not top-level.
    parent: Option<&'static str>,
    /// Whether it reuses the component model (`composedAssetModelId`) or carries
    /// its own inline property definition instead.
    reuses: bool,
}

/// One row of the refusal table: the asset models and composite models to build,
/// and how the delete addresses the component model.
struct Row {
    /// What this row varies, named by every assertion that reads it.
    case: &'static str,
    /// The asset models holding the composite models, as
    /// `(assetModelName, assetModelType)`.
    holders: &'static [(&'static str, &'static str)],
    /// The composite models to create, in order.
    composites: &'static [Composite],
    /// Address the doomed component model by its `externalId:` label rather than
    /// its raw id.
    by_external_id: bool,
}

/// The rows: one per thing the rule generalises over — the type of the asset
/// model holding the reusing composite model, the composite model's own type,
/// its nesting depth, how many asset models reuse the component model, whether
/// the holder also holds composite models that reuse nothing, and the label form
/// the delete names the component model by.
fn refusal_rows() -> Vec<Row> {
    vec![
        Row {
            case: "a top-level CUSTOM composite model on an ASSET_MODEL holder",
            holders: &[("Boiler", "ASSET_MODEL")],
            composites: &[Composite {
                holder: 0,
                id: COMPOSITE_IDS[0],
                composite_type: "CUSTOM",
                parent: None,
                reuses: true,
            }],
            by_external_id: false,
        },
        Row {
            case: "a composite model nested one deep, beside an inline sibling",
            holders: &[("Turbine", "ASSET_MODEL")],
            composites: &[
                Composite {
                    holder: 0,
                    id: COMPOSITE_IDS[1],
                    composite_type: "CUSTOM",
                    parent: None,
                    reuses: false,
                },
                Composite {
                    holder: 0,
                    id: COMPOSITE_IDS[2],
                    composite_type: "CUSTOM",
                    parent: Some(COMPOSITE_IDS[1]),
                    reuses: true,
                },
            ],
            by_external_id: false,
        },
        Row {
            case: "an AWS/L4E_ANOMALY composite model",
            holders: &[("Chiller", "ASSET_MODEL")],
            composites: &[Composite {
                holder: 0,
                id: COMPOSITE_IDS[3],
                composite_type: "AWS/L4E_ANOMALY",
                parent: None,
                reuses: true,
            }],
            by_external_id: false,
        },
        Row {
            case: "an AWS/ALARM composite model",
            holders: &[("Alarmed", "ASSET_MODEL")],
            composites: &[Composite {
                holder: 0,
                id: COMPOSITE_IDS[4],
                composite_type: "AWS/ALARM",
                parent: None,
                reuses: true,
            }],
            by_external_id: false,
        },
        Row {
            case: "a holder that is itself a COMPONENT_MODEL",
            holders: &[("Housing", "COMPONENT_MODEL")],
            composites: &[Composite {
                holder: 0,
                id: COMPOSITE_IDS[5],
                composite_type: "CUSTOM",
                parent: None,
                reuses: true,
            }],
            by_external_id: false,
        },
        Row {
            case: "composite models on two different holders reusing it",
            holders: &[("First", "ASSET_MODEL"), ("Second", "ASSET_MODEL")],
            composites: &[
                Composite {
                    holder: 0,
                    id: COMPOSITE_IDS[6],
                    composite_type: "CUSTOM",
                    parent: None,
                    reuses: true,
                },
                Composite {
                    holder: 1,
                    id: COMPOSITE_IDS[7],
                    composite_type: "AWS/L4E_ANOMALY",
                    parent: None,
                    reuses: true,
                },
            ],
            by_external_id: false,
        },
        Row {
            case: "the delete addressing the component model by its externalId: label",
            holders: &[("Pumped", "ASSET_MODEL")],
            composites: &[Composite {
                holder: 0,
                id: COMPOSITE_IDS[0],
                composite_type: "CUSTOM",
                parent: None,
                reuses: true,
            }],
            by_external_id: true,
        },
    ]
}

/// Create an asset model from `body` and return its id.
async fn create_model(router: &MultiServiceRouter, case: &str, body: Value) -> String {
    let (status, created) = call(router, "POST", "/asset-models", Some(body.clone())).await;
    assert_eq!(
        status, 202,
        "case {case:?}: CreateAssetModel({body}) got {status} {created}"
    );
    created["assetModelId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no assetModelId in {created}"))
        .to_string()
}

/// Create the `COMPONENT_MODEL` every row reuses: one property, and an external
/// id so the delete can be addressed either way.
async fn create_component_model(router: &MultiServiceRouter, case: &str) -> String {
    create_model(
        router,
        case,
        json!({
            "assetModelName": "Pump",
            "assetModelType": "COMPONENT_MODEL",
            "assetModelExternalId": COMPONENT_EXTERNAL_ID,
            "assetModelProperties": [
                {"name": LENT_PROPERTY, "dataType": "DOUBLE", "type": {"measurement": {}}},
            ],
        }),
    )
    .await
}

/// `POST /asset-models/{holder}/composite-models` for `composite`, which must be
/// accepted.
async fn create_composite(
    router: &MultiServiceRouter,
    case: &str,
    holder: &str,
    composite: &Composite,
    component_model: &str,
) {
    let mut body = json!({
        "assetModelCompositeModelId": composite.id,
        "assetModelCompositeModelName": format!("Composite {}", composite.id),
        "assetModelCompositeModelType": composite.composite_type,
    });
    if composite.reuses {
        body["composedAssetModelId"] = json!(component_model);
    } else {
        body["assetModelCompositeModelProperties"] = json!([
            {"name": "Pressure", "dataType": "DOUBLE", "type": {"measurement": {}}},
        ]);
    }
    if let Some(parent) = composite.parent {
        body["parentAssetModelCompositeModelId"] = json!(parent);
    }
    let (status, created) = call(
        router,
        "POST",
        &format!("/asset-models/{holder}/composite-models"),
        Some(body.clone()),
    )
    .await;
    assert_eq!(
        status, 202,
        "case {case:?}: CreateAssetModelCompositeModel({body}) got {status} {created}"
    );
}

/// `DELETE /asset-models/{label}`.
async fn delete_model(router: &MultiServiceRouter, label: &str) -> (u16, Value) {
    call(router, "DELETE", &format!("/asset-models/{label}"), None).await
}

/// The property names `DescribeAssetModelCompositeModel` reports for a composite
/// model, which for a component-model-based one are the component model's.
async fn composite_property_names(
    router: &MultiServiceRouter,
    case: &str,
    holder: &str,
    composite_id: &str,
) -> Vec<String> {
    let (status, described) = call(
        router,
        "GET",
        &format!("/asset-models/{holder}/composite-models/{composite_id}"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeAssetModelCompositeModel got {status} {described}"
    );
    described["assetModelCompositeModelProperties"]
        .as_array()
        .unwrap_or_else(|| {
            panic!("case {case:?}: no assetModelCompositeModelProperties in {described}")
        })
        .iter()
        .map(|property| {
            property["name"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: a property without a name in {property}"))
                .to_string()
        })
        .collect()
}

#[tokio::test]
async fn a_component_model_a_composite_model_still_reuses_cannot_be_deleted() {
    for row in refusal_rows() {
        let case = row.case;
        let router = router();
        let component = create_component_model(&router, case).await;

        let mut holders: Vec<String> = Vec::new();
        for (name, model_type) in row.holders {
            holders.push(
                create_model(
                    &router,
                    case,
                    json!({"assetModelName": name, "assetModelType": model_type}),
                )
                .await,
            );
        }
        for composite in row.composites {
            create_composite(
                &router,
                case,
                &holders[composite.holder],
                composite,
                &component,
            )
            .await;
        }

        // Before the delete, every reusing composite model reads the property it
        // borrows from the component model — what the delete must not quietly
        // take away.
        for composite in row.composites.iter().filter(|composite| composite.reuses) {
            let names = composite_property_names(
                &router,
                case,
                &holders[composite.holder],
                composite.id,
            )
            .await;
            assert_eq!(
                names,
                vec![LENT_PROPERTY.to_string()],
                "case {case:?}: the composite model must read the component model's property \
                 before the delete, got {names:?}"
            );
        }

        let label = if row.by_external_id {
            format!("externalId:{COMPONENT_EXTERNAL_ID}")
        } else {
            component.clone()
        };
        let (status, refused) = delete_model(&router, &label).await;
        assert_eq!(
            status, 400,
            "case {case:?}: deleting a reused component model must be refused, got \
             {status} {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "InvalidRequestException",
            "case {case:?}: unexpected error code in {refused}"
        );
        // The refusal names a composite model that reuses the model and the asset
        // model holding it, so the caller knows what to delete first. Any of the
        // reusing composite models is a correct one to name.
        let wanted: Vec<String> = row
            .composites
            .iter()
            .filter(|composite| composite.reuses)
            .map(|composite| {
                format!(
                    "Asset model {component} is referenced by composite model {} of asset model \
                     {} when delete was called",
                    composite.id, holders[composite.holder]
                )
            })
            .collect();
        let message = refused["message"].as_str().unwrap_or("").to_string();
        assert!(
            wanted.contains(&message),
            "case {case:?}: expected one of {wanted:?}, got {message:?} in {refused}"
        );

        // Nothing moved: the component model is still there with its property, and
        // every composite model that reuses it still reads that property.
        let (status, described) =
            call(&router, "GET", &format!("/asset-models/{component}"), None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the refused model must still be readable, got {status} {described}"
        );
        for composite in row.composites.iter().filter(|composite| composite.reuses) {
            let names = composite_property_names(
                &router,
                case,
                &holders[composite.holder],
                composite.id,
            )
            .await;
            assert_eq!(
                names,
                vec![LENT_PROPERTY.to_string()],
                "case {case:?}: the refused delete must leave the composite model's properties \
                 alone, got {names:?}"
            );
        }
    }
}

#[tokio::test]
async fn an_asset_model_is_refused_while_any_dependent_the_store_holds_names_it() {
    // Rows: the two dependency directions this store holds a delete back for,
    // each built on its own router so only that dependent stands.
    let kinds = [
        "an asset instantiating it",
        "another model's composite model reusing it as its component model",
    ];
    for kind in kinds {
        let router = router();
        // The doomed model: a COMPONENT_MODEL for the composition direction (only
        // one of that type can be reused), an ASSET_MODEL otherwise (only one of
        // that type can be instantiated).
        let composition = kind.contains("composite model");
        let model_type = if composition {
            "COMPONENT_MODEL"
        } else {
            "ASSET_MODEL"
        };
        let doomed = create_model(
            &router,
            kind,
            json!({
                "assetModelName": "Doomed",
                "assetModelType": model_type,
                "assetModelProperties": [
                    {"name": LENT_PROPERTY, "dataType": "DOUBLE", "type": {"measurement": {}}},
                ],
            }),
        )
        .await;

        // Build exactly one dependent of the kind this row is about, and remember
        // how to remove it again.
        let remove: String = match kind {
            "an asset instantiating it" => {
                let asset = create_asset(&router, "Instance", &doomed).await;
                format!("/assets/{asset}")
            }
            _ => {
                let holder =
                    create_model(&router, kind, json!({"assetModelName": "Holder"})).await;
                create_composite(
                    &router,
                    kind,
                    &holder,
                    &Composite {
                        holder: 0,
                        id: COMPOSITE_IDS[0],
                        composite_type: "CUSTOM",
                        parent: None,
                        reuses: true,
                    },
                    &doomed,
                )
                .await;
                format!(
                    "/asset-models/{holder}/composite-models/{}",
                    COMPOSITE_IDS[0]
                )
            }
        };

        let (status, refused) = delete_model(&router, &doomed).await;
        assert_eq!(
            status, 400,
            "case {kind:?}: the delete must be refused while the dependent stands, got \
             {status} {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "InvalidRequestException",
            "case {kind:?}: unexpected error code in {refused}"
        );
        let message = refused["message"].as_str().unwrap_or("");
        assert!(
            message.contains("when delete was called"),
            "case {kind:?}: the refusal must say the delete found a dependent, got {refused}"
        );
        let (status, described) =
            call(&router, "GET", &format!("/asset-models/{doomed}"), None).await;
        assert_eq!(
            status, 200,
            "case {kind:?}: the refused model must still be readable, got {status} {described}"
        );

        // And with that one dependent removed the SAME delete is accepted, so the
        // refusal was about the dependency and nothing else.
        let (status, removed) = call(&router, "DELETE", &remove, None).await;
        assert_eq!(
            status, 202,
            "case {kind:?}: removing the dependent ({remove}) got {status} {removed}"
        );
        let (status, deleted) = delete_model(&router, &doomed).await;
        assert_eq!(
            status, 202,
            "case {kind:?}: the delete must be accepted once nothing depends on the model, got \
             {status} {deleted}"
        );
        assert_eq!(
            deleted["assetModelStatus"]["state"].as_str(),
            Some("DELETING"),
            "case {kind:?}: the accepted delete must report DELETING, got {deleted}"
        );
        let (status, gone) = call(&router, "GET", &format!("/asset-models/{doomed}"), None).await;
        assert_eq!(
            status, 404,
            "case {kind:?}: the deleted model must read back as absent, got {status} {gone}"
        );
    }
}

#[tokio::test]
async fn a_component_model_nothing_reuses_is_deleted() {
    let router = router();

    // Rows: the ways an asset model can be free of composition dependents — never
    // reused, reused only by a composite model that has since been deleted, and
    // reused only by a composite model whose whole holder is gone.
    let cases = ["never reused", "the composite model was deleted", "the holder was deleted"];
    for case in cases {
        let component = create_model(
            &router,
            case,
            json!({
                "assetModelName": format!("Pump {case}"),
                "assetModelType": "COMPONENT_MODEL",
                "assetModelProperties": [
                    {"name": LENT_PROPERTY, "dataType": "DOUBLE", "type": {"measurement": {}}},
                ],
            }),
        )
        .await;
        if case != "never reused" {
            let holder = create_model(
                &router,
                case,
                json!({"assetModelName": format!("Holder {case}")}),
            )
            .await;
            let composite_id = COMPOSITE_IDS[0];
            create_composite(
                &router,
                case,
                &holder,
                &Composite {
                    holder: 0,
                    id: composite_id,
                    composite_type: "CUSTOM",
                    parent: None,
                    reuses: true,
                },
                &component,
            )
            .await;
            let path = if case == "the composite model was deleted" {
                format!("/asset-models/{holder}/composite-models/{composite_id}")
            } else {
                format!("/asset-models/{holder}")
            };
            let (status, removed) = call(&router, "DELETE", &path, None).await;
            assert_eq!(
                status, 202,
                "case {case:?}: removing the dependent ({path}) got {status} {removed}"
            );
        }

        let (status, deleted) = delete_model(&router, &component).await;
        assert_eq!(
            status, 202,
            "case {case:?}: a component model nothing reuses must be deleted, got \
             {status} {deleted}"
        );
        let (status, gone) = call(&router, "GET", &format!("/asset-models/{component}"), None).await;
        assert_eq!(
            status, 404,
            "case {case:?}: the deleted model must read back as absent, got {status} {gone}"
        );
    }
}
