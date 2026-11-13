// SPDX-License-Identifier: CC0-1.0

//! What ARN a refusal names a resource of this store by, driven over HTTP through
//! the same `MultiServiceRouter` the emulator boots with.
//!
//! The rule these rows pin: an error member that points at a resource — the
//! `resourceArn` of the modelled conflict, already-exists and precondition
//! failures, documented as "the ARN of the resource that conflicts with this
//! operation" / "that already exists" / "on which precondition failed"
//! (`smithy_query(query="shape", shape="ConflictingOperationException")`) —
//! reports the ARN that resource CARRIES, the one minted by the call that created
//! it and reported by every read of it, never one rebuilt from the refused
//! request. Only a resource this store no longer holds leaves its id as the single
//! thing to build an ARN from.
//!
//! Every resource below is created through requests signed for [`HOME`], and every
//! refusal is provoked by a request signed for [`AWAY`] — the two regional
//! endpoints one store is reachable from, since the handlers take the ARN region
//! from the caller's SigV4 credential scope. A refusal that named the requesting
//! region would hand the caller an ARN no asset or asset model of this store
//! carries, which is exactly what each row rules out: the reported ARN is compared
//! both with an independently built `HOME` ARN and with what a read of the named
//! resource reports.
//!
//! The rows span what the rule generalises over: the operation kind (create,
//! update, delete, associate, disassociate, property write, composite-model
//! create), the resource kind (asset, asset model, workspace), the modelled error
//! (409 `ConflictingOperationException`, 409 `ResourceAlreadyExistsException`, 412
//! `PreconditionFailedException`), what collided (a spent `clientToken`, a
//! caller-supplied id, an external id, a resource's unique name, a bound property
//! alias, an `If-Match` header) and how the request carries it (payload member,
//! query string, header).
//!
//! The delete rows are the case a build cannot be right about by construction: a
//! delete FREES the identity it removed — a caller-supplied `assetId` or
//! `assetModelId`, a `workspaceName` — so a later create can hold that identity
//! again, and the conflict a token spent on the delete answers has to name the ARN
//! the resource now holding it was minted with.

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The crate's shared test harness lives in the `asset_lifecycle` suite; the token
// helpers in the `client_token_*` suites, the hierarchy helpers in
// `asset_hierarchies`, the ingest helper in `dataset_data_segments` and the
// foreign ETag in `hierarchies_etags_and_refusals`. All are reused rather than
// copied.
use crate::asset_hierarchies::{associate, create_model as create_model_from, hierarchy_id};
use crate::asset_lifecycle::{
    call, create_asset, create_model, error_code, property_catalog, property_id,
    router_with_engine, REGION,
};
use crate::client_token_idempotency::with_token;
use crate::client_token_replay::token_for;
use crate::dataset_data_segments::ingest;
use crate::hierarchies_etags_and_refusals::FOREIGN_E_TAG;

/// The region every resource in this suite is CREATED in: the one the shared
/// harness signs for, so every ARN this store holds carries it.
const HOME: &str = REGION;

/// The OTHER regional endpoint the same store is reached from: the region every
/// refused request below is signed for, and the one no stored ARN carries.
const AWAY: &str = "eu-central-1";

/// The seconds a row's stream is ingested at, which is what creates the
/// disassociated time series an association row binds.
const SAMPLES: &[i64] = &[0, 5];

/// An external id a row collides with, shared by the asset rows.
const ASSET_EXTERNAL_ID: &str = "refusal-arn-asset";

/// An external id a row collides with, shared by the asset-model rows.
const MODEL_EXTERNAL_ID: &str = "refusal-arn-model";

/// Which kind of resource a refusal names, which decides how the resource reads
/// back and what its ARN's resource section looks like.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Asset,
    AssetModel,
    Workspace,
}

impl Kind {
    /// The `<type>/<id>` resource section of this kind's ARN.
    fn resource(self, id: &str) -> String {
        match self {
            Kind::Asset => format!("asset/{id}"),
            Kind::AssetModel => format!("asset-model/{id}"),
            // A workspace is addressed by its name, so the name is what the
            // resource section carries.
            Kind::Workspace => format!("workspace/{id}"),
        }
    }

    /// Where a read of `id` reports the resource, and the member carrying its ARN.
    fn read(self, id: &str) -> (String, &'static str) {
        match self {
            Kind::Asset => (format!("/assets/{id}"), "assetArn"),
            Kind::AssetModel => (format!("/asset-models/{id}"), "assetModelArn"),
            Kind::Workspace => (format!("/workspaces/{id}"), "workspaceArn"),
        }
    }
}

/// One row: a request signed for [`AWAY`] that must be refused, and the resource
/// of this store the refusal has to name.
struct Row {
    case: &'static str,
    method: &'static str,
    uri: String,
    body: Option<Value>,
    /// The extra request headers the row sends, for the conditional-header rows.
    headers: Vec<(&'static str, String)>,
    status: u16,
    code: &'static str,
    kind: Kind,
    /// The id the refusal's `resourceId` must carry.
    resource_id: String,
}

/// An ARN of this service as an INDEPENDENT oracle: built here from a region and
/// the emulator's account id, rather than through the helper the handlers use.
fn arn_in(region: &str, resource: &str) -> String {
    format!(
        "arn:aws:iotsitewise:{region}:{}:{resource}",
        engine::EMULATOR_ACCOUNT_ID
    )
}

/// Dispatch one request signed for `region`, carrying `headers`, as
/// `(status, parsed body)`.
///
/// The shared harness's `call` signs every request for one region, which is what
/// makes the stored ARN and a rebuilt one indistinguishable; this suite is about
/// telling them apart, so the credential scope's region is a parameter here.
/// Shared with the sibling suites that pin one refusal's ARN of their own
/// (`bulk_import_job_name_uniqueness`), rather than growing a copy of it.
pub(crate) async fn call_signed_for(
    router: &MultiServiceRouter,
    region: &str,
    method: &str,
    uri: &str,
    body: Option<Value>,
    headers: &[(&str, &str)],
) -> (u16, Value) {
    let payload = body
        .map(|value| Bytes::from(serde_json::to_vec(&value).expect("a serializable body")))
        .unwrap_or_default();
    let mut request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json")
        // The SigV4 credential scope is what routes the request to this service and
        // what the handlers read the ARN region from.
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTEST/20260101/{region}/iotsitewise/aws4_request, \
                 SignedHeaders=host, Signature=abc"
            ),
        );
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    let response = router
        .dispatch(request.body(payload).expect("a valid request"))
        .await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let parsed = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|_| {
            Value::String(String::from_utf8_lossy(bytes.as_ref()).into_owned())
        })
    };
    (status, parsed)
}

/// Create an asset of `model` carrying `external_id` and return its id.
async fn asset_with_external_id(
    router: &MultiServiceRouter,
    name: &str,
    model: &str,
    external_id: &str,
) -> String {
    let (status, created) = call(
        router,
        "POST",
        "/assets",
        Some(json!({
            "assetName": name,
            "assetModelId": model,
            "assetExternalId": external_id,
        })),
    )
    .await;
    assert_eq!(status, 202, "CreateAsset {name:?}: got {status} {created}");
    created["assetId"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateAsset {name:?}: no assetId in {created}"))
        .to_string()
}

/// The id of the `Temperature` property of `asset_id`, read back through
/// `DescribeAsset`.
async fn temperature_of(router: &MultiServiceRouter, asset_id: &str) -> String {
    let (status, described) = call(router, "GET", &format!("/assets/{asset_id}"), None).await;
    assert_eq!(
        status, 200,
        "DescribeAsset {asset_id}: got {status} {described}"
    );
    property_id(&described["assetProperties"], "Temperature")
}

/// `PUT /assets/{asset}/properties/{property}` — `UpdateAssetProperty`, signed
/// for [`HOME`].
async fn update_property(
    router: &MultiServiceRouter,
    asset_id: &str,
    property: &str,
    body: Value,
) -> (u16, Value) {
    call(
        router,
        "PUT",
        &format!("/assets/{asset_id}/properties/{property}"),
        Some(body),
    )
    .await
}

/// The rows, with every resource each of them names created — and every token each
/// of them reuses spent — by requests signed for [`HOME`] first.
async fn rows(router: &MultiServiceRouter) -> Vec<Row> {
    let plain = create_model(router, "RefusalArnPlain", property_catalog()).await;

    // --- the hierarchy pairs the association rows address --------------------
    let turbine_model =
        create_model_from(router, json!({ "assetModelName": "RefusalArnTurbine" })).await;
    let farm_model = create_model_from(
        router,
        json!({
            "assetModelName": "RefusalArnFarm",
            "assetModelHierarchies": [
                {"name": "turbines", "childAssetModelId": turbine_model},
            ],
        }),
    )
    .await;
    let turbines = hierarchy_id(router, &farm_model, "turbines").await;
    let disassociated_farm = create_asset(router, "RefusalArnFarmOne", &farm_model).await;
    let disassociated_turbine = create_asset(router, "RefusalArnTurbineOne", &turbine_model).await;
    let other_farm = create_asset(router, "RefusalArnFarmTwo", &farm_model).await;
    let other_turbine = create_asset(router, "RefusalArnTurbineTwo", &turbine_model).await;
    let associated_farm = create_asset(router, "RefusalArnFarmThree", &farm_model).await;
    let associated_turbine = create_asset(router, "RefusalArnTurbineThree", &turbine_model).await;
    let spare_farm = create_asset(router, "RefusalArnFarmFour", &farm_model).await;
    let spare_turbine = create_asset(router, "RefusalArnTurbineFour", &turbine_model).await;
    for (parent, child) in [
        (&disassociated_farm, &disassociated_turbine),
        (&other_farm, &other_turbine),
    ] {
        let (status, body) = associate(router, parent, &turbines, child).await;
        assert_eq!(status, 200, "AssociateAssets {parent}: got {status} {body}");
    }

    // The disassociation that spends the token the DisassociateAssets row reuses.
    let disassociate_token = token_for("spent on DisassociateAssets");
    let (status, body) = call(
        router,
        "POST",
        &format!("/assets/{disassociated_farm}/disassociate"),
        Some(json!({
            "hierarchyId": turbines,
            "childAssetId": disassociated_turbine,
            "clientToken": disassociate_token,
        })),
    )
    .await;
    assert_eq!(status, 200, "DisassociateAssets: got {status} {body}");

    // The association that spends the token the AssociateAssets row reuses.
    let associate_token = token_for("spent on AssociateAssets");
    let (status, body) = call(
        router,
        "POST",
        &format!("/assets/{associated_farm}/associate"),
        Some(json!({
            "hierarchyId": turbines,
            "childAssetId": associated_turbine,
            "clientToken": associate_token,
        })),
    )
    .await;
    assert_eq!(status, 200, "AssociateAssets: got {status} {body}");

    // --- the assets the create and update rows address -----------------------
    let create_token = token_for("spent on CreateAsset");
    let (status, created) = call(
        router,
        "POST",
        "/assets",
        Some(json!({
            "assetName": "RefusalArnCreatedFirst",
            "assetModelId": plain,
            "clientToken": create_token,
        })),
    )
    .await;
    assert_eq!(status, 202, "CreateAsset: got {status} {created}");
    let created_asset = created["assetId"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateAsset: no assetId in {created}"))
        .to_string();

    let external_asset =
        asset_with_external_id(router, "RefusalArnExternal", &plain, ASSET_EXTERNAL_ID).await;
    let updated_asset = create_asset(router, "RefusalArnUpdated", &plain).await;
    let other_asset = create_asset(router, "RefusalArnOther", &plain).await;
    let update_token = token_for("spent on UpdateAsset");
    let (status, body) = call(
        router,
        "PUT",
        &format!("/assets/{updated_asset}"),
        Some(json!({
            "assetName": "RefusalArnUpdatedRenamed",
            "clientToken": update_token,
        })),
    )
    .await;
    assert_eq!(status, 202, "UpdateAsset: got {status} {body}");

    // --- the asset properties the property rows address ----------------------
    let bound_asset = create_asset(router, "RefusalArnBound", &plain).await;
    let bound_property = temperature_of(router, &bound_asset).await;
    let free_asset = create_asset(router, "RefusalArnFree", &plain).await;
    let free_property = temperature_of(router, &free_asset).await;
    let property_token = token_for("spent on UpdateAssetProperty");
    let (status, body) = update_property(
        router,
        &bound_asset,
        &bound_property,
        with_token(
            &json!({ "propertyAlias": "/refusal/arn/bound" }),
            &property_token,
        ),
    )
    .await;
    assert_eq!(status, 200, "UpdateAssetProperty: got {status} {body}");

    // --- the streams the time-series rows bind and break --------------------
    let stream_asset = create_asset(router, "RefusalArnStream", &plain).await;
    let stream_property = temperature_of(router, &stream_asset).await;
    let second_stream_asset = create_asset(router, "RefusalArnStreamTwo", &plain).await;
    let second_stream_property = temperature_of(router, &second_stream_asset).await;
    ingest(router, "/refusal/arn/stream-one", SAMPLES).await;
    ingest(router, "/refusal/arn/stream-two", SAMPLES).await;
    let bind_token = token_for("spent on AssociateTimeSeries");
    let (status, body) = call(
        router,
        "POST",
        &format!(
            "/timeseries/associate?alias=/refusal/arn/stream-one&assetId={stream_asset}\
             &propertyId={stream_property}"
        ),
        Some(json!({ "clientToken": bind_token })),
    )
    .await;
    assert_eq!(
        status, 200,
        "AssociateTimeSeriesToAssetProperty: got {status} {body}"
    );

    let broken_asset = create_asset(router, "RefusalArnBroken", &plain).await;
    let broken_property = temperature_of(router, &broken_asset).await;
    let intact_asset = create_asset(router, "RefusalArnIntact", &plain).await;
    let intact_property = temperature_of(router, &intact_asset).await;
    for (asset, property, alias) in [
        (&broken_asset, &broken_property, "/refusal/arn/broken"),
        (&intact_asset, &intact_property, "/refusal/arn/intact"),
    ] {
        let (status, body) =
            update_property(router, asset, property, json!({ "propertyAlias": alias })).await;
        assert_eq!(
            status, 200,
            "UpdateAssetProperty binding {alias}: got {status} {body}"
        );
    }
    let break_token =
        token_for("spent on DisassociateTimeSeries");
    let (status, body) = call(
        router,
        "POST",
        &format!(
            "/timeseries/disassociate?alias=/refusal/arn/broken&assetId={broken_asset}\
             &propertyId={broken_property}"
        ),
        Some(json!({ "clientToken": break_token })),
    )
    .await;
    assert_eq!(
        status, 200,
        "DisassociateTimeSeriesFromAssetProperty: got {status} {body}"
    );

    // --- the asset models the model rows address -----------------------------
    let model_token = token_for("spent on CreateAssetModel");
    let (status, created) = call(
        router,
        "POST",
        "/asset-models",
        Some(json!({
            "assetModelName": "RefusalArnModelCreated",
            "clientToken": model_token,
        })),
    )
    .await;
    assert_eq!(status, 202, "CreateAssetModel: got {status} {created}");
    let created_model = created["assetModelId"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateAssetModel: no assetModelId in {created}"))
        .to_string();
    let external_model = create_model_from(
        router,
        json!({
            "assetModelName": "RefusalArnModelExternal",
            "assetModelExternalId": MODEL_EXTERNAL_ID,
        }),
    )
    .await;
    let updated_model = create_model(router, "RefusalArnModelUpdated", json!([])).await;
    let other_model = create_model(router, "RefusalArnModelOther", json!([])).await;
    let conditional_model = create_model(router, "RefusalArnModelConditional", json!([])).await;
    let model_update_token = token_for("spent on UpdateAssetModel");
    let (status, body) = call(
        router,
        "PUT",
        &format!("/asset-models/{updated_model}"),
        Some(json!({
            "assetModelName": "RefusalArnModelRenamed",
            "clientToken": model_update_token,
        })),
    )
    .await;
    assert_eq!(status, 202, "UpdateAssetModel: got {status} {body}");

    // --- the identities a delete freed, and the resources that hold them now --
    // Each delete below spends a token on a resource it removes; a create then
    // takes the identity that delete freed, signed for HOME like every other
    // create here. So when a DIFFERENT request reuses the spent token, the
    // resource its conflict names is one this store HOLDS — and the ARN it has to
    // report is the one that resource was minted with, which no rebuild from the
    // refused request's region can produce.
    let deleted_asset = create_asset(router, "RefusalArnDeleted", &plain).await;
    let asset_delete_token = token_for("spent on DeleteAsset");
    let (status, body) = call(
        router,
        "DELETE",
        &format!("/assets/{deleted_asset}?clientToken={asset_delete_token}"),
        None,
    )
    .await;
    assert_eq!(status, 202, "DeleteAsset: got {status} {body}");
    let (status, recreated) = call(
        router,
        "POST",
        "/assets",
        Some(json!({
            "assetName": "RefusalArnDeletedAgain",
            "assetModelId": plain,
            "assetId": deleted_asset,
        })),
    )
    .await;
    assert_eq!(
        status, 202,
        "CreateAsset reusing a freed assetId: got {status} {recreated}"
    );
    let spare_delete_asset = create_asset(router, "RefusalArnSpareDeleted", &plain).await;

    let deleted_model = create_model(router, "RefusalArnModelDeleted", json!([])).await;
    let model_delete_token = token_for("spent on DeleteAssetModel");
    let (status, body) = call(
        router,
        "DELETE",
        &format!("/asset-models/{deleted_model}?clientToken={model_delete_token}"),
        None,
    )
    .await;
    assert_eq!(status, 202, "DeleteAssetModel: got {status} {body}");
    let recreated_model = create_model_from(
        router,
        json!({
            "assetModelName": "RefusalArnModelDeletedAgain",
            "assetModelId": deleted_model,
        }),
    )
    .await;
    assert_eq!(
        recreated_model, deleted_model,
        "CreateAssetModel reusing a freed assetModelId must hold it, got {recreated_model}"
    );
    let spare_delete_model = create_model(router, "RefusalArnModelSpareDeleted", json!([])).await;

    let deleted_workspace = "refusal-arn-deleted";
    crate::support::ensure_workspace(router, deleted_workspace).await;
    let workspace_delete_token = token_for("spent on DeleteWorkspace");
    let (status, body) = call(
        router,
        "DELETE",
        &format!("/workspaces/{deleted_workspace}?clientToken={workspace_delete_token}"),
        None,
    )
    .await;
    assert_eq!(status, 200, "DeleteWorkspace: got {status} {body}");
    crate::support::ensure_workspace(router, deleted_workspace).await;
    let spare_delete_workspace = "refusal-arn-spare-deleted";
    crate::support::ensure_workspace(router, spare_delete_workspace).await;

    vec![
        Row {
            case: "DisassociateAssets under a clientToken spent on another request",
            method: "POST",
            uri: format!("/assets/{other_farm}/disassociate"),
            body: Some(json!({
                "hierarchyId": turbines,
                "childAssetId": other_turbine,
                "clientToken": disassociate_token,
            })),
            headers: Vec::new(),
            status: 409,
            code: "ConflictingOperationException",
            kind: Kind::Asset,
            resource_id: disassociated_farm.clone(),
        },
        Row {
            case: "AssociateAssets under a clientToken spent on another request",
            method: "POST",
            uri: format!("/assets/{spare_farm}/associate"),
            body: Some(json!({
                "hierarchyId": turbines,
                "childAssetId": spare_turbine,
                "clientToken": associate_token,
            })),
            headers: Vec::new(),
            status: 409,
            code: "ConflictingOperationException",
            kind: Kind::Asset,
            resource_id: associated_farm.clone(),
        },
        Row {
            case: "CreateAsset under a clientToken spent on another request",
            method: "POST",
            uri: "/assets".to_string(),
            body: Some(json!({
                "assetName": "RefusalArnCreatedSecond",
                "assetModelId": plain,
                "clientToken": create_token,
            })),
            headers: Vec::new(),
            status: 409,
            code: "ConflictingOperationException",
            kind: Kind::Asset,
            resource_id: created_asset.clone(),
        },
        Row {
            case: "CreateAsset naming an assetId an asset already holds",
            method: "POST",
            uri: "/assets".to_string(),
            body: Some(json!({
                "assetName": "RefusalArnIdCollision",
                "assetModelId": plain,
                "assetId": created_asset,
            })),
            headers: Vec::new(),
            status: 409,
            code: "ResourceAlreadyExistsException",
            kind: Kind::Asset,
            resource_id: created_asset.clone(),
        },
        Row {
            case: "CreateAsset naming an assetExternalId an asset already holds",
            method: "POST",
            uri: "/assets".to_string(),
            body: Some(json!({
                "assetName": "RefusalArnExternalCollision",
                "assetModelId": plain,
                "assetExternalId": ASSET_EXTERNAL_ID,
            })),
            headers: Vec::new(),
            status: 409,
            code: "ResourceAlreadyExistsException",
            kind: Kind::Asset,
            resource_id: external_asset.clone(),
        },
        Row {
            case: "UpdateAsset under a clientToken spent on another request",
            method: "PUT",
            uri: format!("/assets/{other_asset}"),
            body: Some(json!({
                "assetName": "RefusalArnOtherRenamed",
                "clientToken": update_token,
            })),
            headers: Vec::new(),
            status: 409,
            code: "ConflictingOperationException",
            kind: Kind::Asset,
            resource_id: updated_asset.clone(),
        },
        Row {
            case: "UpdateAsset assigning an assetExternalId another asset holds",
            method: "PUT",
            uri: format!("/assets/{other_asset}"),
            body: Some(json!({
                "assetName": "RefusalArnOther",
                "assetExternalId": ASSET_EXTERNAL_ID,
            })),
            headers: Vec::new(),
            status: 409,
            code: "ResourceAlreadyExistsException",
            kind: Kind::Asset,
            resource_id: external_asset.clone(),
        },
        Row {
            case: "UpdateAssetProperty under a clientToken spent on another request",
            method: "PUT",
            uri: format!("/assets/{free_asset}/properties/{free_property}"),
            body: Some(json!({
                "propertyNotificationState": "ENABLED",
                "clientToken": property_token,
            })),
            headers: Vec::new(),
            status: 409,
            code: "ConflictingOperationException",
            kind: Kind::Asset,
            resource_id: bound_asset.clone(),
        },
        Row {
            case: "UpdateAssetProperty binding a propertyAlias another property carries",
            method: "PUT",
            uri: format!("/assets/{free_asset}/properties/{free_property}"),
            body: Some(json!({ "propertyAlias": "/refusal/arn/bound" })),
            headers: Vec::new(),
            status: 409,
            code: "ConflictingOperationException",
            kind: Kind::Asset,
            resource_id: bound_asset.clone(),
        },
        Row {
            case: "AssociateTimeSeriesToAssetProperty under a spent clientToken",
            method: "POST",
            uri: format!(
                "/timeseries/associate?alias=/refusal/arn/stream-two\
                 &assetId={second_stream_asset}&propertyId={second_stream_property}"
            ),
            body: Some(json!({ "clientToken": bind_token })),
            headers: Vec::new(),
            status: 409,
            code: "ConflictingOperationException",
            kind: Kind::Asset,
            resource_id: stream_asset.clone(),
        },
        Row {
            case: "DisassociateTimeSeriesFromAssetProperty under a spent clientToken",
            method: "POST",
            uri: format!(
                "/timeseries/disassociate?alias=/refusal/arn/intact&assetId={intact_asset}\
                 &propertyId={intact_property}"
            ),
            body: Some(json!({ "clientToken": break_token })),
            headers: Vec::new(),
            status: 409,
            code: "ConflictingOperationException",
            kind: Kind::Asset,
            resource_id: broken_asset.clone(),
        },
        Row {
            case: "CreateAssetModel under a clientToken spent on another request",
            method: "POST",
            uri: "/asset-models".to_string(),
            body: Some(json!({
                "assetModelName": "RefusalArnModelSecond",
                "clientToken": model_token,
            })),
            headers: Vec::new(),
            status: 409,
            code: "ConflictingOperationException",
            kind: Kind::AssetModel,
            resource_id: created_model.clone(),
        },
        Row {
            case: "CreateAssetModel naming an assetModelExternalId a model already holds",
            method: "POST",
            uri: "/asset-models".to_string(),
            body: Some(json!({
                "assetModelName": "RefusalArnModelExternalCollision",
                "assetModelExternalId": MODEL_EXTERNAL_ID,
            })),
            headers: Vec::new(),
            status: 409,
            code: "ResourceAlreadyExistsException",
            kind: Kind::AssetModel,
            resource_id: external_model.clone(),
        },
        Row {
            case: "UpdateAssetModel under a clientToken spent on another request",
            method: "PUT",
            uri: format!("/asset-models/{other_model}"),
            body: Some(json!({
                "assetModelName": "RefusalArnModelOtherRenamed",
                "clientToken": model_update_token,
            })),
            headers: Vec::new(),
            status: 409,
            code: "ConflictingOperationException",
            kind: Kind::AssetModel,
            resource_id: updated_model.clone(),
        },
        Row {
            case: "UpdateAssetModel assigning an assetModelExternalId another model holds",
            method: "PUT",
            uri: format!("/asset-models/{other_model}"),
            body: Some(json!({
                "assetModelName": "RefusalArnModelOther",
                "assetModelExternalId": MODEL_EXTERNAL_ID,
            })),
            headers: Vec::new(),
            status: 409,
            code: "ResourceAlreadyExistsException",
            kind: Kind::AssetModel,
            resource_id: external_model.clone(),
        },
        Row {
            case: "UpdateAssetModel with an If-Match the model's version never had",
            method: "PUT",
            uri: format!("/asset-models/{conditional_model}"),
            body: Some(json!({ "assetModelName": "RefusalArnModelConditional" })),
            headers: vec![("if-match", FOREIGN_E_TAG.to_string())],
            status: 412,
            code: "PreconditionFailedException",
            kind: Kind::AssetModel,
            resource_id: conditional_model.clone(),
        },
        Row {
            case: "CreateAssetModelCompositeModel with an If-Match the model never had",
            method: "POST",
            uri: format!("/asset-models/{conditional_model}/composite-models"),
            body: Some(json!({
                "assetModelCompositeModelName": "RefusalArnComposite",
                "assetModelCompositeModelType": "CUSTOM",
            })),
            headers: vec![("if-match", FOREIGN_E_TAG.to_string())],
            status: 412,
            code: "PreconditionFailedException",
            kind: Kind::AssetModel,
            resource_id: conditional_model.clone(),
        },
        Row {
            case: "CreateAssetModel naming an assetModelId a model already holds",
            method: "POST",
            uri: "/asset-models".to_string(),
            body: Some(json!({
                "assetModelName": "RefusalArnModelIdCollision",
                "assetModelId": created_model,
            })),
            headers: Vec::new(),
            status: 409,
            code: "ResourceAlreadyExistsException",
            kind: Kind::AssetModel,
            resource_id: created_model.clone(),
        },
        Row {
            case: "CreateAssetModel naming an assetModelName a model already holds",
            method: "POST",
            uri: "/asset-models".to_string(),
            // The name `created_model` was created under, which is unique to it.
            body: Some(json!({ "assetModelName": "RefusalArnModelCreated" })),
            headers: Vec::new(),
            status: 409,
            code: "ResourceAlreadyExistsException",
            kind: Kind::AssetModel,
            resource_id: created_model.clone(),
        },
        Row {
            case: "DeleteAsset under a token spent on the delete of an assetId held again",
            method: "DELETE",
            uri: format!("/assets/{spare_delete_asset}?clientToken={asset_delete_token}"),
            body: None,
            headers: Vec::new(),
            status: 409,
            code: "ConflictingOperationException",
            kind: Kind::Asset,
            resource_id: deleted_asset.clone(),
        },
        Row {
            case: "DeleteAssetModel under a token spent on the delete of an id held again",
            method: "DELETE",
            uri: format!("/asset-models/{spare_delete_model}?clientToken={model_delete_token}"),
            body: None,
            headers: Vec::new(),
            status: 409,
            code: "ConflictingOperationException",
            kind: Kind::AssetModel,
            resource_id: deleted_model.clone(),
        },
        Row {
            case: "DeleteWorkspace under a token spent on the delete of a name held again",
            method: "DELETE",
            uri: format!(
                "/workspaces/{spare_delete_workspace}?clientToken={workspace_delete_token}"
            ),
            body: None,
            headers: Vec::new(),
            status: 409,
            code: "ConflictingOperationException",
            kind: Kind::Workspace,
            resource_id: deleted_workspace.to_string(),
        },
    ]
}

#[tokio::test]
async fn a_refusal_names_the_resource_by_the_arn_that_resource_carries() {
    let (router, _engine) = router_with_engine();

    for row in rows(&router).await {
        let case = row.case;
        let headers: Vec<(&str, &str)> = row
            .headers
            .iter()
            .map(|(name, value)| (*name, value.as_str()))
            .collect();
        let (status, refused) = call_signed_for(
            &router,
            AWAY,
            row.method,
            &row.uri,
            row.body.clone(),
            &headers,
        )
        .await;
        assert_eq!(
            (status, error_code(&refused)),
            (row.status, row.code),
            "case {case:?}: expected {} {}, got {status} {refused}",
            row.status,
            row.code
        );
        assert_eq!(
            refused["resourceId"].as_str(),
            Some(row.resource_id.as_str()),
            "case {case:?}: the refusal must name {}, got {refused}",
            row.resource_id
        );

        // The ARN the resource was MINTED with, which carries the region the call
        // that created it was signed for — not the one this request was.
        let expected = arn_in(HOME, &row.kind.resource(&row.resource_id));
        let reported = refused["resourceArn"].as_str().unwrap_or_default();
        assert_eq!(
            reported, expected,
            "case {case:?}: expected the stored ARN {expected}, got {reported} in {refused}"
        );
        assert_ne!(
            reported,
            arn_in(AWAY, &row.kind.resource(&row.resource_id)),
            "case {case:?}: the refusal must not rebuild the ARN from this request's \
             region, got {refused}"
        );

        // And it is the very ARN a read of that resource reports, so a client can
        // resolve it back to the resource the refusal pointed at.
        let (read_uri, member) = row.kind.read(&row.resource_id);
        let (status, read) = call(&router, "GET", &read_uri, None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the named resource must read back, got {status} {read}"
        );
        assert_eq!(
            read[member].as_str().unwrap_or_default(),
            reported,
            "case {case:?}: the refusal's ARN must be the one {member} reports, got {read}"
        );
    }
}
