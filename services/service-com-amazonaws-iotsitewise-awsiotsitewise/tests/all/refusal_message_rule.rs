// SPDX-License-Identifier: CC0-1.0

//! The two refusal-WORDING rules this service follows, pinned over a SET of
//! inputs rather than over the one request an issue happened to record:
//!
//! 1. An absent resource is stated in the form the service words THAT resource's
//!    absence in, naming the identifier the REQUEST supplied for it — whatever
//!    form that identifier takes (a raw id, an `externalId:` label, a stream
//!    alias, a workspace name) and whichever operation did the lookup. Most
//!    resources share one form, the kind followed by the identifier; the
//!    families with their own sentence are a resource addressed by ARN (where
//!    the ARN is the dimension the request named), a HIERARCHY (worded as a
//!    hierarchy that "doesn't exist" by the operations that look one up to act on
//!    the association it holds — the removal and the listing; the forward
//!    association states instead that the hierarchy the request names is not one
//!    the asset has, which is a statement about the request rather than an absent
//!    resource), an ASSET
//!    addressed by the `externalId:` selector (reported as an external id that
//!    does not exist, by the bare external id), a COMPOSITE MODEL of an asset
//!    model (which names the asset model it was looked for in), a time series
//!    addressed by its `alias` (reported as a `dataAlias` absent from the
//!    account the lookup ran in) and a GATEWAY (whose read and whose update are
//!    worded differently) — the families the companion suite next door drives
//!    through every operation that addresses one.
//! 2. A uniqueness collision names the DIMENSION of the request that collided
//!    (`externalId`, `ID`, `name`) rather than an identifier the rejected request
//!    would have minted, and points its `resourceId` at the resource that already
//!    holds that dimension.
//!
//! The expected strings below are written out BY HAND from those rules, as an
//! independent oracle: the suite never calls the crate's message helpers, so a
//! handler that drifts back to a template of its own cannot agree with itself.
//! Rule 1's `<Kind> with ID <identifier> not found` and rule 2's
//! `Another <resource> is already using the same <dimension>` are the forms AWS
//! was recorded answering for this service's asset and asset-model paths.
//!
//! Everything is driven over HTTP through the same `MultiServiceRouter` the
//! emulator boots with, so every assertion is about the wire behaviour a client
//! sees.

use serde_json::{json, Value};

use super::asset_lifecycle::{call, error_code, router, REGION};
// The not-found/uniqueness suite next door already exposes the two create
// helpers in the form these rows need (the whole request body, so a row can
// carry an `assetExternalId` or a chosen id) and the error-message reader, so
// this suite reuses them rather than growing a second copy.
use super::hierarchies_etags_and_refusals::{create_asset, create_model, error_message};

/// A well-formed `ID` / `CustomID` UUID nothing in this suite is ever stored
/// under.
const UNKNOWN_ID: &str = "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee";

/// A second one, so a row can be wrong in two places at once.
const OTHER_UNKNOWN_ID: &str = "11111111-2222-4333-8444-555555555555";

/// A `PropertyAlias` no stream in this suite stands under.
const UNKNOWN_ALIAS: &str = "/company/windfarm/9/turbine/9/no-such";

/// A `WorkspaceName` no workspace answers to.
const UNKNOWN_WORKSPACE: &str = "shop-floor";

/// The `@httpQuery`-bound timestamps the aggregate readers require, as the
/// ISO-8601 `date-time` form a query parameter carries them in.
const WINDOW: &str = "startDate=2026-01-01T00:00:00Z&endDate=2026-01-02T00:00:00Z";

/// One row of the not-found table: the case name, the HTTP method, the URI, the
/// request body, and the message the rule produces for it.
type NotFoundRow = (&'static str, &'static str, String, Option<Value>, String);

/// The sentence a time series addressed by its `alias` is reported absent with,
/// written out by hand here as the oracle: the account the alias was resolved in
/// — the one every ARN this emulator mints carries — and the alias from the
/// request.
fn data_alias_absence(alias: &str) -> String {
    format!(
        "dataAlias with accountId AwsAccountId{{value={}}}, \
         alias PropertyAlias(super=PropertyAlias{{value={alias}}}) not found",
        engine::EMULATOR_ACCOUNT_ID
    )
}

#[tokio::test]
async fn an_absent_resource_is_stated_as_its_kind_and_the_requested_identifier() {
    let router = router();
    let model_id = create_model(
        &router,
        json!({
            "assetModelName": "Boiler",
            "assetModelProperties": [],
            "assetModelExternalId": "boiler_model"
        }),
    )
    .await;
    let asset_id = create_asset(
        &router,
        json!({ "assetName": "Boiler1", "assetModelId": model_id }),
    )
    .await;
    let account = engine::EMULATOR_ACCOUNT_ID;
    let unknown_arn = format!("arn:aws:iotsitewise:{REGION}:{account}:asset/{UNKNOWN_ID}");

    // Rows span the operations that look a resource up, every resource KIND this
    // service refuses an absence of, and every identifier FORM the models admit
    // for one. The expected message is built from the row's own identifier, so no
    // row can pass by matching a hard-coded id.
    let rows: Vec<NotFoundRow> = vec![
        (
            "UpdateAsset, a raw id",
            "PUT",
            format!("/assets/{UNKNOWN_ID}"),
            Some(json!({ "assetName": "Whatever" })),
            format!("Asset with ID {UNKNOWN_ID} not found"),
        ),
        (
            "UpdateAsset, an externalId label",
            "PUT",
            "/assets/externalId:no_such_asset".to_string(),
            Some(json!({ "assetName": "Whatever" })),
            "asset with externalId: no_such_asset doesn't exist.".to_string(),
        ),
        (
            "UpdateAssetProperty, a raw id",
            "PUT",
            format!("/assets/{UNKNOWN_ID}/properties/{OTHER_UNKNOWN_ID}"),
            Some(json!({})),
            format!("Asset with ID {UNKNOWN_ID} not found"),
        ),
        (
            "UpdateAssetProperty, an externalId label",
            "PUT",
            format!("/assets/externalId:no_such_asset/properties/{OTHER_UNKNOWN_ID}"),
            Some(json!({})),
            "asset with externalId: no_such_asset doesn't exist.".to_string(),
        ),
        (
            "DisassociateTimeSeriesFromAssetProperty, a raw id",
            "POST",
            format!(
                "/timeseries/disassociate?alias={UNKNOWN_ALIAS}&assetId={UNKNOWN_ID}\
                 &propertyId={OTHER_UNKNOWN_ID}"
            ),
            None,
            format!("Asset with ID {UNKNOWN_ID} not found"),
        ),
        (
            "AssociateAssets, an absent parent asset",
            "POST",
            format!("/assets/{UNKNOWN_ID}/associate"),
            Some(json!({ "hierarchyId": OTHER_UNKNOWN_ID, "childAssetId": asset_id })),
            format!("Asset with ID {UNKNOWN_ID} not found"),
        ),
        (
            "AssociateAssets, an absent child asset",
            "POST",
            format!("/assets/{asset_id}/associate"),
            Some(json!({ "hierarchyId": OTHER_UNKNOWN_ID, "childAssetId": UNKNOWN_ID })),
            format!("Asset with ID {UNKNOWN_ID} not found"),
        ),
        (
            "ListAssetModelProperties, a raw id",
            "GET",
            format!("/asset-models/{UNKNOWN_ID}/properties"),
            None,
            format!("Asset Model with ID {UNKNOWN_ID} not found"),
        ),
        (
            "ListAssetModelProperties, an externalId label",
            "GET",
            "/asset-models/externalId:no_such_model/properties".to_string(),
            None,
            "Asset Model with ID externalId:no_such_model not found".to_string(),
        ),
        (
            "DescribeAssetModelCompositeModel, an absent asset model",
            "GET",
            format!("/asset-models/{UNKNOWN_ID}/composite-models/{OTHER_UNKNOWN_ID}"),
            None,
            format!("Asset Model with ID {UNKNOWN_ID} not found"),
        ),
        (
            "DescribeAssetModelCompositeModel, an absent composite model",
            "GET",
            format!("/asset-models/{model_id}/composite-models/{UNKNOWN_ID}"),
            None,
            format!("CompositeModel: {UNKNOWN_ID} not found for AssetModel: {model_id}"),
        ),
        (
            "DescribeAssetModelCompositeModel, an absent composite model by label",
            "GET",
            format!("/asset-models/{model_id}/composite-models/externalId:no_such_composite"),
            None,
            format!(
                "CompositeModel: externalId:no_such_composite not found for \
                 AssetModel: {model_id}"
            ),
        ),
        (
            "CreateAssetModelCompositeModel, an absent asset model",
            "POST",
            format!("/asset-models/{UNKNOWN_ID}/composite-models"),
            Some(json!({
                "assetModelCompositeModelName": "Orphan",
                "assetModelCompositeModelType": "CUSTOM"
            })),
            format!("Asset Model with ID {UNKNOWN_ID} not found"),
        ),
        (
            "DescribeGateway, a raw id",
            "GET",
            format!("/20200301/gateways/{UNKNOWN_ID}"),
            None,
            format!(
                "No gateway with ID {UNKNOWN_ID} was found. Please verify that you supplied \
                 the correct gateway ID and try again."
            ),
        ),
        (
            "DescribeTimeSeries, an alias",
            "GET",
            format!("/timeseries/describe?alias={UNKNOWN_ALIAS}"),
            None,
            data_alias_absence(UNKNOWN_ALIAS),
        ),
        (
            "DeleteTimeSeries, an alias",
            "POST",
            format!("/timeseries/delete?alias={UNKNOWN_ALIAS}"),
            None,
            data_alias_absence(UNKNOWN_ALIAS),
        ),
        (
            "DeleteTimeSeries, an asset id",
            "POST",
            format!("/timeseries/delete?assetId={UNKNOWN_ID}&propertyId={OTHER_UNKNOWN_ID}"),
            None,
            format!("Asset with ID {UNKNOWN_ID} not found"),
        ),
        (
            "GetAssetPropertyValue, an alias",
            "GET",
            format!("/properties/latest?propertyAlias={UNKNOWN_ALIAS}"),
            None,
            format!("Time Series with ID {UNKNOWN_ALIAS} not found"),
        ),
        (
            "GetAssetPropertyValueHistory, an alias",
            "GET",
            format!("/properties/history?propertyAlias={UNKNOWN_ALIAS}"),
            None,
            format!("Time Series with ID {UNKNOWN_ALIAS} not found"),
        ),
        (
            "GetAssetPropertyValueHistory, an asset id",
            "GET",
            format!("/properties/history?assetId={UNKNOWN_ID}&propertyId={OTHER_UNKNOWN_ID}"),
            None,
            format!("Asset with ID {UNKNOWN_ID} not found"),
        ),
        (
            "GetAssetPropertyAggregates, an asset id",
            "GET",
            format!(
                "/properties/aggregates?assetId={UNKNOWN_ID}&propertyId={OTHER_UNKNOWN_ID}\
                 &aggregateTypes=COUNT&resolution=1m&{WINDOW}"
            ),
            None,
            format!("Asset with ID {UNKNOWN_ID} not found"),
        ),
        (
            "GetAssetPropertyAggregates, an alias",
            "GET",
            format!(
                "/properties/aggregates?propertyAlias={UNKNOWN_ALIAS}\
                 &aggregateTypes=COUNT&resolution=1m&{WINDOW}"
            ),
            None,
            format!("Time Series with ID {UNKNOWN_ALIAS} not found"),
        ),
        (
            "GetInterpolatedAssetPropertyValues, an asset id",
            "GET",
            format!(
                "/properties/interpolated?assetId={UNKNOWN_ID}&propertyId={OTHER_UNKNOWN_ID}\
                 &startTimeInSeconds=1000&endTimeInSeconds=1100&intervalInSeconds=10\
                 &quality=GOOD&type=LINEAR_INTERPOLATION"
            ),
            None,
            format!("Asset with ID {UNKNOWN_ID} not found"),
        ),
        (
            "GetInterpolatedAssetPropertyValues, an alias",
            "GET",
            format!(
                "/properties/interpolated?propertyAlias={UNKNOWN_ALIAS}\
                 &startTimeInSeconds=1000&endTimeInSeconds=1100&intervalInSeconds=10\
                 &quality=GOOD&type=LINEAR_INTERPOLATION"
            ),
            None,
            format!("Time Series with ID {UNKNOWN_ALIAS} not found"),
        ),
        (
            "ListTimeSeries, a workspace name",
            "GET",
            format!("/timeseries?workspaceName={UNKNOWN_WORKSPACE}"),
            None,
            format!("Workspace with ID {UNKNOWN_WORKSPACE} not found"),
        ),
        (
            "DeleteTimeSeries, a workspace name",
            "POST",
            format!("/timeseries/delete?workspaceName={UNKNOWN_WORKSPACE}&alias={UNKNOWN_ALIAS}"),
            None,
            format!("Workspace with ID {UNKNOWN_WORKSPACE} not found"),
        ),
        (
            "ListTagsForResource, an ARN",
            "GET",
            format!("/tags?resourceArn={unknown_arn}"),
            None,
            format!("Resource with ARN {unknown_arn} not found"),
        ),
        (
            "TagResource, an ARN",
            "POST",
            format!("/tags?resourceArn={unknown_arn}"),
            Some(json!({ "tags": { "Stage": "test" } })),
            format!("Resource with ARN {unknown_arn} not found"),
        ),
        (
            "UntagResource, an ARN",
            "DELETE",
            format!("/tags?resourceArn={unknown_arn}&tagKeys=Stage"),
            None,
            format!("Resource with ARN {unknown_arn} not found"),
        ),
    ];

    for (case, method, uri, body, expected) in &rows {
        let (status, answered) = call(&router, method, uri, body.clone()).await;
        assert_eq!(
            status, 404,
            "case {case:?}: an absent resource is a 404, got {status} {answered}"
        );
        assert_eq!(
            error_code(&answered),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {answered}"
        );
        assert_eq!(
            error_message(&answered),
            expected,
            "case {case:?}: expected {expected:?}, got {:?}",
            error_message(&answered)
        );
    }
}

#[tokio::test]
async fn the_batch_readers_word_an_absent_resource_the_same_way_per_entry() {
    let router = router();

    // Each pair is (entry for the latest-value batch, entry for the aggregate
    // batch, the message the rule produces), spanning both identifier forms an
    // entry may carry.
    let rows: Vec<(&str, Value, Value, String)> = vec![
        (
            "an asset id nothing is stored under",
            json!({
                "entryId": "unknownasset",
                "assetId": UNKNOWN_ID,
                "propertyId": OTHER_UNKNOWN_ID
            }),
            json!({
                "entryId": "unknownasset",
                "assetId": UNKNOWN_ID,
                "propertyId": OTHER_UNKNOWN_ID,
                "aggregateTypes": ["COUNT"],
                "resolution": "1m",
                "startDate": 1_767_225_600,
                "endDate": 1_767_312_000
            }),
            format!("Asset with ID {UNKNOWN_ID} not found"),
        ),
        (
            "an alias no stream stands under",
            json!({ "entryId": "unknownalias", "propertyAlias": UNKNOWN_ALIAS }),
            json!({
                "entryId": "unknownalias",
                "propertyAlias": UNKNOWN_ALIAS,
                "aggregateTypes": ["COUNT"],
                "resolution": "1m",
                "startDate": 1_767_225_600,
                "endDate": 1_767_312_000
            }),
            format!("Time Series with ID {UNKNOWN_ALIAS} not found"),
        ),
    ];

    let batches = [
        (
            "BatchGetAssetPropertyValue",
            "/properties/batch/latest",
            0usize,
        ),
        (
            "BatchGetAssetPropertyAggregates",
            "/properties/batch/aggregates",
            1usize,
        ),
    ];

    for (operation, uri, which) in batches {
        let entries: Vec<Value> = rows
            .iter()
            .map(|(_, latest, aggregates, _)| {
                if which == 0 {
                    latest.clone()
                } else {
                    aggregates.clone()
                }
            })
            .collect();
        let (status, answered) =
            call(&router, "POST", uri, Some(json!({ "entries": entries }))).await;
        assert_eq!(
            status, 200,
            "{operation}: a per-entry failure is reported in the body, got {status} {answered}"
        );
        for (case, latest, _, expected) in &rows {
            let entry_id = latest["entryId"].as_str().expect("an entryId");
            let reported = answered["errorEntries"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .find(|reported| reported["entryId"] == entry_id)
                .unwrap_or_else(|| {
                    panic!("{operation} case {case:?}: no error entry for {entry_id} in {answered}")
                });
            assert_eq!(
                reported["errorCode"].as_str().unwrap_or(""),
                "ResourceNotFoundException",
                "{operation} case {case:?}: expected the not-found code, got {reported}"
            );
            assert_eq!(
                reported["errorMessage"].as_str().unwrap_or(""),
                expected,
                "{operation} case {case:?}: expected {expected:?}, got {reported}"
            );
        }
    }
}

#[tokio::test]
async fn the_alias_addressed_write_path_words_an_absent_stream_the_same_way() {
    let router = router();
    let model_id = create_model(
        &router,
        json!({
            "assetModelName": "Turbine",
            "assetModelProperties": [
                {"name": "Temperature", "dataType": "DOUBLE", "type": {"measurement": {}}}
            ]
        }),
    )
    .await;
    let asset_id = create_asset(
        &router,
        json!({ "assetName": "Turbine1", "assetModelId": model_id }),
    )
    .await;
    let (status, described) = call(&router, "GET", &format!("/assets/{asset_id}"), None).await;
    assert_eq!(status, 200, "DescribeAsset: got {status} {described}");
    let property_id = described["assetProperties"][0]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("no materialised property in {described}"))
        .to_string();

    // `AssociateTimeSeriesToAssetProperty` BINDS an existing stream rather than
    // creating one, so an alias that names none is the same absence the readers
    // answer — and the asset and the property both resolve here, so the alias is
    // the only thing missing.
    let aliases = [
        ("a slash-separated alias", UNKNOWN_ALIAS.to_string()),
        (
            "an opc-ua style alias",
            "nsu=http://example.com;s=NoSuch".to_string(),
        ),
    ];
    for (case, alias) in &aliases {
        let expected = data_alias_absence(alias);
        let (status, answered) = call(
            &router,
            "POST",
            &format!(
                "/timeseries/associate?alias={alias}&assetId={asset_id}\
                 &propertyId={property_id}"
            ),
            None,
        )
        .await;
        assert_eq!(
            status, 404,
            "case {case:?}: an absent stream is a 404, got {status} {answered}"
        );
        assert_eq!(
            error_code(&answered),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {answered}"
        );
        assert_eq!(
            error_message(&answered),
            expected,
            "case {case:?}: expected {expected:?}, got {:?}",
            error_message(&answered)
        );
    }
}

#[tokio::test]
async fn each_association_operation_words_an_absent_hierarchy_as_the_fact_it_reports() {
    let router = router();
    let model_id = create_model(
        &router,
        json!({ "assetModelName": "Boiler", "assetModelProperties": [] }),
    )
    .await;
    let parent = create_asset(
        &router,
        json!({ "assetName": "Parent", "assetModelId": model_id }),
    )
    .await;
    let child = create_asset(
        &router,
        json!({ "assetName": "Child", "assetModelId": model_id }),
    )
    .await;

    // Both label forms a `hierarchyId` may take, on both operations. The two
    // operations report DIFFERENT facts about the same request, so they word it
    // differently: the forward operation says the hierarchy the request names is
    // not one the asset has (a statement about the request, 400), while the
    // removal says there is no such hierarchy to remove a child from (404).
    let labels = [
        ("a raw id", UNKNOWN_ID.to_string()),
        ("an externalId label", "externalId:no_such_hierarchy".to_string()),
    ];
    for (case, hierarchy) in &labels {
        let (status, answered) = call(
            &router,
            "POST",
            &format!("/assets/{parent}/associate"),
            Some(json!({ "hierarchyId": hierarchy, "childAssetId": child })),
        )
        .await;
        let expected = format!("Given HierarchyId {hierarchy} not present in the Asset {parent}");
        assert_eq!(
            status, 400,
            "case {case:?} on associate: a hierarchy the asset has not got is a 400, \
             got {status} {answered}"
        );
        assert_eq!(
            error_message(&answered),
            expected,
            "case {case:?} on associate: expected {expected:?}, got {:?}",
            error_message(&answered)
        );

        let (status, answered) = call(
            &router,
            "POST",
            &format!("/assets/{parent}/disassociate"),
            Some(json!({ "hierarchyId": hierarchy, "childAssetId": child })),
        )
        .await;
        let expected = format!("A hierarchy with ID {hierarchy} doesn't exist");
        assert_eq!(
            status, 404,
            "case {case:?} on disassociate: an absent hierarchy is a 404, \
             got {status} {answered}"
        );
        assert_eq!(
            error_message(&answered),
            expected,
            "case {case:?} on disassociate: expected {expected:?}, got {:?}",
            error_message(&answered)
        );
    }
}

#[tokio::test]
async fn a_uniqueness_collision_names_the_dimension_that_collided() {
    let router = router();
    let model_id = create_model(
        &router,
        json!({ "assetModelName": "Boiler", "assetModelProperties": [] }),
    )
    .await;
    let holder = create_asset(
        &router,
        json!({
            "assetName": "Boiler1",
            "assetModelId": model_id,
            "assetExternalId": "boiler_one"
        }),
    )
    .await;
    let other = create_asset(
        &router,
        json!({ "assetName": "Boiler2", "assetModelId": model_id }),
    )
    .await;
    let composite_model = create_model(
        &router,
        json!({ "assetModelName": "Composites", "assetModelProperties": [] }),
    )
    .await;
    let taken_composite_id = "12345678-1234-4234-8234-123456789abc";
    let (status, created) = call(
        &router,
        "POST",
        &format!("/asset-models/{composite_model}/composite-models"),
        Some(json!({
            "assetModelCompositeModelName": "Taken",
            "assetModelCompositeModelType": "CUSTOM",
            "assetModelCompositeModelId": taken_composite_id
        })),
    )
    .await;
    assert_eq!(
        status, 202,
        "the first composite model: got {status} {created}"
    );
    // A second asset model, so one row can collide with a composite-model id held
    // by a DIFFERENT model: the id has to be unique across every stored model.
    let stranger_model = create_model(
        &router,
        json!({ "assetModelName": "OtherComposites", "assetModelProperties": [] }),
    )
    .await;
    let (status, gateway) = call(
        &router,
        "POST",
        "/20200301/gateways",
        Some(json!({
            "gatewayName": "the-one-gateway",
            "gatewayPlatform": { "greengrassV2": { "coreDeviceThingName": "core-device-one" } }
        })),
    )
    .await;
    assert_eq!(status, 201, "the first gateway: got {status} {gateway}");
    let gateway_id = gateway["gatewayId"].as_str().unwrap_or("").to_string();

    // Rows differ in the resource, the DIMENSION that collided and the operation
    // that refused: the wording follows the dimension, not the operation.
    // (case, method, uri, body, expected message, the resource the refusal points at)
    let rows: Vec<(&str, &str, String, Value, String, String)> = vec![
        (
            "UpdateAsset reassigning an externalId another asset holds",
            "PUT",
            format!("/assets/{other}"),
            json!({ "assetName": "Boiler2", "assetExternalId": "boiler_one" }),
            "Another asset is already using the same externalId".to_string(),
            holder.clone(),
        ),
        (
            "CreateAsset claiming an externalId another asset holds",
            "POST",
            "/assets".to_string(),
            json!({
                "assetName": "Boiler3",
                "assetModelId": model_id,
                "assetExternalId": "boiler_one"
            }),
            "Another asset is already using the same externalId".to_string(),
            holder.clone(),
        ),
        (
            "CreateAsset claiming an assetId another asset holds",
            "POST",
            "/assets".to_string(),
            json!({ "assetName": "Boiler4", "assetModelId": model_id, "assetId": holder }),
            "Another asset is already using the same ID".to_string(),
            holder.clone(),
        ),
        (
            "CreateGateway claiming a gatewayName another gateway holds",
            "POST",
            "/20200301/gateways".to_string(),
            json!({
                "gatewayName": "the-one-gateway",
                "gatewayPlatform": { "greengrassV2": { "coreDeviceThingName": "core-device-two" } }
            }),
            "Another gateway is already using the same name".to_string(),
            gateway_id.clone(),
        ),
        (
            "CreateAssetModelCompositeModel claiming an id another composite model holds",
            "POST",
            format!("/asset-models/{composite_model}/composite-models"),
            json!({
                "assetModelCompositeModelName": "Second",
                "assetModelCompositeModelType": "CUSTOM",
                "assetModelCompositeModelId": taken_composite_id
            }),
            "Another composite model is already using the same ID".to_string(),
            // A composite model carries no ARN of its own, so this refusal points at
            // the asset model the request addressed, which is also the model holding
            // the taken id here.
            composite_model.clone(),
        ),
        (
            "CreateAssetModelCompositeModel claiming an id another model's composite model holds",
            "POST",
            format!("/asset-models/{stranger_model}/composite-models"),
            json!({
                "assetModelCompositeModelName": "Third",
                "assetModelCompositeModelType": "CUSTOM",
                "assetModelCompositeModelId": taken_composite_id
            }),
            "Another composite model is already using the same ID".to_string(),
            // The holder is another asset model this time, and the refusal still
            // points at the asset model the request addressed — the only one it can
            // name an ARN for — so this row distinguishes the two.
            stranger_model.clone(),
        ),
        (
            "CreateAssetModel claiming an assetModelName another asset model holds",
            "POST",
            "/asset-models".to_string(),
            json!({ "assetModelName": "Boiler", "assetModelProperties": [] }),
            "Another asset model is already using the same name".to_string(),
            model_id.clone(),
        ),
        (
            "UpdateAssetModel renaming onto a name another asset model holds",
            "PUT",
            format!("/asset-models/{stranger_model}"),
            json!({ "assetModelName": "Boiler" }),
            "Another asset model is already using the same name".to_string(),
            model_id.clone(),
        ),
    ];

    for (case, method, uri, body, expected, points_at) in &rows {
        let (status, answered) = call(&router, method, uri, Some(body.clone())).await;
        assert_eq!(
            status, 409,
            "case {case:?}: a uniqueness collision is a 409, got {status} {answered}"
        );
        assert_eq!(
            error_code(&answered),
            "ResourceAlreadyExistsException",
            "case {case:?}: expected ResourceAlreadyExistsException, got {answered}"
        );
        assert_eq!(
            error_message(&answered),
            expected,
            "case {case:?}: expected {expected:?}, got {:?}",
            error_message(&answered)
        );
        assert_eq!(
            answered["resourceId"].as_str().unwrap_or(""),
            points_at,
            "case {case:?}: the refusal must point at the resource that already holds the \
             dimension, got {answered}"
        );
    }

    // The refused update wrote nothing: the asset that collided still carries no
    // external id, so the wording above is a refusal and not a report.
    let (status, described) = call(&router, "GET", &format!("/assets/{other}"), None).await;
    assert_eq!(status, 200, "DescribeAsset: got {status} {described}");
    assert_eq!(
        described.get("assetExternalId").and_then(Value::as_str),
        None,
        "a refused update must not assign the external id, got {described}"
    );
}
