// SPDX-License-Identifier: CC0-1.0

//! The IoT SiteWise control-plane rules this suite pins, each over a SET of
//! inputs rather than one recorded request:
//!
//! 1. An asset hierarchy association is a stored `(parent, hierarchy, child)`
//!    triple: `AssociateAssets` adds it and `DisassociateAssets` removes it, and
//!    either call is refused `404 ResourceNotFoundException` — changing nothing —
//!    when any member of the triple names nothing the store holds, including the
//!    association itself.
//! 2. `DescribeAssetModel` reports the model's `ETag`: a 43-character
//!    `^[\w-]{43}$` token derived from the model's content, so it is stable
//!    across reads and differs between models.
//! 3. `DeleteAssetModel` evaluates its conditional headers BEFORE deleting
//!    anything: a failed `If-Match` / `If-None-Match` precondition is
//!    `412 PreconditionFailedException` and the model survives.
//! 4. A delete refused because a dependent resource still exists is a TERMINAL
//!    request error (`400 InvalidRequestException`), not the retryable
//!    `409 ConflictingOperationException`.
//! 5. A value outside a rule the model declares is refused with the code this
//!    service models, `InvalidRequestException`.
//! 6. A not-found refusal names the resource kind and the identifier the REQUEST
//!    supplied; a uniqueness refusal names the dimension that collided.
//!
//! Everything is driven over HTTP through the same `MultiServiceRouter` the
//! emulator boots with, so every assertion is about the wire behaviour a client
//! sees.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_iotsitewise_awsiotsitewise::state::service_state;
use service_com_amazonaws_iotsitewise_awsiotsitewise::EmulatedService;

/// The region the test signs for.
const REGION: &str = "us-west-2";

/// Comfortably past `state::ACTIVATION_DELAY`, so the owed CREATING -> ACTIVE
/// transition has fallen due and the model has an ACTIVE version.
pub(crate) const PAST_ACTIVATION: Duration = Duration::from_millis(900);

/// A router with just this service mounted on a fresh shared engine.
pub(crate) fn router() -> MultiServiceRouter {
    router_with_engine().0
}

/// The same, keeping a handle on the shared engine so a test can read the store
/// directly — the only way to observe the association set while
/// `ListAssociatedAssets` is still a 501 scaffold.
fn router_with_engine() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    (router, engine)
}

/// One request, with the extra request headers `headers` carries, answered as
/// `(status, response headers, parsed body)`.
pub(crate) async fn call_with_headers(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<Value>,
    headers: &[(&str, &str)],
) -> (u16, ::http::HeaderMap, Value) {
    let payload = body
        .map(|value| Bytes::from(serde_json::to_vec(&value).expect("a serializable body")))
        .unwrap_or_default();
    let mut request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json")
        // The SigV4 credential scope is what routes the request to this service.
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTEST/20260101/{REGION}/iotsitewise/aws4_request, \
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
    let response_headers = response.headers().clone();
    let bytes = response.into_body();
    let parsed = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|_| {
            Value::String(String::from_utf8_lossy(bytes.as_ref()).into_owned())
        })
    };
    (status, response_headers, parsed)
}

/// One request without extra headers, answered as `(status, parsed body)`.
pub(crate) async fn call(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let (status, _, parsed) = call_with_headers(router, method, uri, body, &[]).await;
    (status, parsed)
}

/// The wire error code of an error body (`__type`), or `""`.
pub(crate) fn error_code(body: &Value) -> &str {
    body.get("__type").and_then(Value::as_str).unwrap_or("")
}

/// The `message` of an error body, or `""`.
pub(crate) fn error_message(body: &Value) -> &str {
    body.get("message").and_then(Value::as_str).unwrap_or("")
}

/// `POST /asset-models` with `body`, answering the created id.
pub(crate) async fn create_model(router: &MultiServiceRouter, body: Value) -> String {
    let (status, created) = call(router, "POST", "/asset-models", Some(body.clone())).await;
    assert_eq!(status, 202, "CreateAssetModel {body}: got {status} {created}");
    created["assetModelId"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateAssetModel {body}: no assetModelId in {created}"))
        .to_string()
}

/// `POST /assets` with `body`, answering the created id.
pub(crate) async fn create_asset(router: &MultiServiceRouter, body: Value) -> String {
    let (status, created) = call(router, "POST", "/assets", Some(body.clone())).await;
    assert_eq!(status, 202, "CreateAsset {body}: got {status} {created}");
    created["assetId"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateAsset {body}: no assetId in {created}"))
        .to_string()
}

/// The `ETag` response header of `DescribeAssetModel` on `model_id`.
pub(crate) async fn describe_e_tag(router: &MultiServiceRouter, model_id: &str) -> String {
    let (status, headers, body) =
        call_with_headers(router, "GET", &format!("/asset-models/{model_id}"), None, &[]).await;
    assert_eq!(status, 200, "DescribeAssetModel {model_id}: got {body}");
    headers
        .get("etag")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_else(|| {
            panic!("DescribeAssetModel {model_id}: no ETag header among {headers:?}")
        })
        .to_string()
}

/// `true` when `tag` is a value the modelled `ETag` pattern `^[\w-]{43}$` admits.
///
/// Hand-rolled from the pattern `smithy_query(query="shape", shape="ETag")`
/// reports, as an INDEPENDENT oracle: it admits every character class the pattern
/// does rather than the ones `crate::state::asset_model_etag`'s encoder happens to
/// emit, so a change of encoding cannot make one bug agree with itself.
pub(crate) fn is_modelled_e_tag(tag: &str) -> bool {
    tag.chars().count() == 43
        && tag
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-')
}

/// A valid-but-unknown `ETag`: 43 characters the pattern admits, which no stored
/// model can hash to.
pub(crate) const FOREIGN_E_TAG: &str = "0123456789_0123456789-0123456789_012345678-";

/// One row of the conditional-delete table: the case name, the conditional
/// request headers it sends, and the status `DeleteAssetModel` must answer.
type ConditionalDeleteCase<'a> = (&'a str, Vec<(&'a str, &'a str)>, u16);

/// The same row once the model it runs against has been created: the row, plus
/// that model's id.
type ConditionalDeleteRow<'a> = (&'a str, &'a Vec<(&'a str, &'a str)>, u16, String);

/// A valid `CustomID` UUID no create in this suite ever mints.
const UNKNOWN_ID: &str = "11111111-2222-3333-4444-555555555555";
/// A second one, so a case can be wrong in two places at once.
const OTHER_UNKNOWN_ID: &str = "99999999-8888-7777-6666-555555555555";

/// The parent/child models and assets every association case is run against.
struct Hierarchy {
    parent_model: String,
    child_model: String,
    parent_asset: String,
    child_asset: String,
    /// The id of the parent model's hierarchy, as the parent ASSET reports it.
    hierarchy_id: String,
    /// An asset of the PARENT model — not a child the hierarchy admits.
    foreign_asset: String,
}

/// Two models (the parent carrying one hierarchy to the child model, with an
/// external id on the hierarchy), one asset of each, plus one extra asset of the
/// parent model.
async fn hierarchy_fixture(router: &MultiServiceRouter) -> Hierarchy {
    let child_model = create_model(
        router,
        json!({
            "assetModelName": "Sensor",
            "assetModelProperties": [{
                "name": "Temperature",
                "dataType": "DOUBLE",
                "type": { "measurement": {} }
            }]
        }),
    )
    .await;
    let parent_model = create_model(
        router,
        json!({
            "assetModelName": "Boiler",
            "assetModelProperties": [],
            "assetModelHierarchies": [{
                "name": "Sensors",
                "externalId": "boiler-sensors",
                "childAssetModelId": child_model,
            }]
        }),
    )
    .await;
    let parent_asset = create_asset(
        router,
        json!({
            "assetName": "Boiler1",
            "assetModelId": parent_model,
            "assetExternalId": "boiler-1"
        }),
    )
    .await;
    let child_asset = create_asset(
        router,
        json!({
            "assetName": "Sensor1",
            "assetModelId": child_model,
            "assetExternalId": "sensor-1"
        }),
    )
    .await;
    let foreign_asset = create_asset(
        router,
        json!({ "assetName": "Boiler2", "assetModelId": parent_model }),
    )
    .await;

    let (status, described) = call(router, "GET", &format!("/assets/{parent_asset}"), None).await;
    assert_eq!(status, 200, "DescribeAsset {parent_asset}: got {described}");
    let hierarchy_id = described["assetHierarchies"][0]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("the parent asset reports no hierarchy: {described}"))
        .to_string();

    Hierarchy {
        parent_model,
        child_model,
        parent_asset,
        child_asset,
        hierarchy_id,
        foreign_asset,
    }
}

/// How many assets of `asset_model_id` `ListAssets` reports as top level — the
/// implemented reader of the association set: an associated child is some other
/// asset's child and drops out of this answer.
async fn top_level_count(router: &MultiServiceRouter, asset_model_id: &str) -> usize {
    let (status, body) = call(
        router,
        "GET",
        &format!("/assets?filter=TOP_LEVEL&assetModelId={asset_model_id}"),
        None,
    )
    .await;
    assert_eq!(status, 200, "ListAssets TOP_LEVEL: got {status} {body}");
    body["assetSummaries"]
        .as_array()
        .map_or(0, |summaries| summaries.len())
}

#[tokio::test]
async fn an_association_triple_that_names_nothing_stored_is_refused_and_changes_nothing() {
    let (router, engine) = router_with_engine();
    let fixture = hierarchy_fixture(&router).await;

    // The rows span every way a triple can fail to name a stored association: a
    // parent that does not exist, a hierarchy the parent does not carry, a child
    // that does not exist, and — the recorded case — a triple whose three parts
    // all exist but which was never associated. The last two rows run the same
    // refusal through the `externalId:` label form, which the model admits for
    // every one of the three members.
    let cases: Vec<(&str, Value)> = vec![
        (
            "unknown parent asset",
            json!({
                "assetId": UNKNOWN_ID,
                "hierarchyId": fixture.hierarchy_id,
                "childAssetId": fixture.child_asset,
            }),
        ),
        (
            "unknown hierarchy on a stored parent",
            json!({
                "assetId": fixture.parent_asset,
                "hierarchyId": UNKNOWN_ID,
                "childAssetId": fixture.child_asset,
            }),
        ),
        (
            "unknown child asset",
            json!({
                "assetId": fixture.parent_asset,
                "hierarchyId": fixture.hierarchy_id,
                "childAssetId": OTHER_UNKNOWN_ID,
            }),
        ),
        (
            "every part stored, never associated",
            json!({
                "assetId": fixture.parent_asset,
                "hierarchyId": fixture.hierarchy_id,
                "childAssetId": fixture.child_asset,
            }),
        ),
        (
            "every part stored by externalId label, never associated",
            json!({
                "assetId": "externalId:boiler-1",
                "hierarchyId": "externalId:boiler-sensors",
                "childAssetId": "externalId:sensor-1",
            }),
        ),
        (
            "unknown hierarchy by externalId label",
            json!({
                "assetId": "externalId:boiler-1",
                "hierarchyId": "externalId:not-a-hierarchy",
                "childAssetId": "externalId:sensor-1",
            }),
        ),
    ];

    for (case, body) in &cases {
        let asset_id = body["assetId"].as_str().expect("a label");
        let (status, answered) = call(
            &router,
            "POST",
            &format!("/assets/{asset_id}/disassociate"),
            Some(body.clone()),
        )
        .await;
        assert_eq!(
            status, 404,
            "case {case:?}: DisassociateAssets must be refused 404, got {status} {answered}"
        );
        assert_eq!(
            error_code(&answered),
            "ResourceNotFoundException",
            "case {case:?}: DisassociateAssets refusal code, got {answered}"
        );
        assert!(
            !error_message(&answered).is_empty(),
            "case {case:?}: the refusal carries a message, got {answered}"
        );
        // Nothing was written: no association exists under the stored hierarchy.
        let children = {
            let state = service_state(&*engine).expect("the iotsitewise store");
            let store = state.lock();
            store
                .hierarchy_children(&fixture.parent_asset, &fixture.hierarchy_id)
                .to_vec()
        };
        assert!(
            children.is_empty(),
            "case {case:?}: a refused DisassociateAssets must change nothing, got {children:?}"
        );
    }

    // The forward operation refuses the same triples, and which refusal it is
    // follows what the triple failed to name: an ASSET the store does not hold is
    // the absent resource (404), while a hierarchy the parent's model does not
    // declare is a statement about the request (400) — a hierarchy is a grouping
    // of that model, not a resource of its own, so the forward operation tells the
    // caller its request names a grouping this asset has not got.
    let forward: [(&str, u16, &str); 3] = [
        ("unknown parent asset", 404, "ResourceNotFoundException"),
        (
            "unknown hierarchy on a stored parent",
            400,
            "InvalidRequestException",
        ),
        ("unknown child asset", 404, "ResourceNotFoundException"),
    ];
    for ((case, body), (expected_case, expected_status, expected_code)) in
        cases.iter().take(3).zip(forward)
    {
        assert_eq!(
            *case, expected_case,
            "the forward rows must line up with the rows above, got {case:?} against \
             {expected_case:?}"
        );
        let asset_id = body["assetId"].as_str().expect("a label");
        let (status, answered) = call(
            &router,
            "POST",
            &format!("/assets/{asset_id}/associate"),
            Some(body.clone()),
        )
        .await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: AssociateAssets must be refused {expected_status}, got \
             {status} {answered}"
        );
        assert_eq!(
            error_code(&answered),
            expected_code,
            "case {case:?}: AssociateAssets refusal code, got {answered}"
        );
    }

    // A child of another model is a REQUEST error, not an absent resource: the
    // hierarchy admits children of exactly one asset model.
    let (status, answered) = call(
        &router,
        "POST",
        &format!("/assets/{}/associate", fixture.parent_asset),
        Some(json!({
            "assetId": fixture.parent_asset,
            "hierarchyId": fixture.hierarchy_id,
            "childAssetId": fixture.foreign_asset,
        })),
    )
    .await;
    assert_eq!(
        status, 400,
        "a child of the wrong asset model is a request error, got {status} {answered}"
    );
    assert_eq!(
        error_code(&answered),
        "InvalidRequestException",
        "wrong-model child refusal code, got {answered}"
    );
}

#[tokio::test]
async fn associating_then_disassociating_a_triple_round_trips_through_stored_state() {
    let (router, engine) = router_with_engine();
    let fixture = hierarchy_fixture(&router).await;

    // Before: the child is nobody's child, so TOP_LEVEL reports it.
    assert_eq!(
        top_level_count(&router, &fixture.child_model).await,
        1,
        "before associating, the child asset is top level"
    );

    // Associate by raw id. A repeat of the SAME triple is refused as the terminal
    // request error it is — the child already has a parent, whether the first
    // association was recorded an instant or an hour ago — and the stored
    // association set stays single-valued, which is what `DisassociateAssets`
    // below has to remove exactly once.
    let associate = json!({
        "assetId": fixture.parent_asset,
        "hierarchyId": fixture.hierarchy_id,
        "childAssetId": fixture.child_asset,
    });
    let (status, answered) = call(
        &router,
        "POST",
        &format!("/assets/{}/associate", fixture.parent_asset),
        Some(associate.clone()),
    )
    .await;
    assert_eq!(
        status, 200,
        "AssociateAssets must succeed, got {status} {answered}"
    );
    let (status, answered) = call(
        &router,
        "POST",
        &format!("/assets/{}/associate", fixture.parent_asset),
        Some(associate),
    )
    .await;
    assert_eq!(
        status, 400,
        "a repeat of the association is a terminal request error, got {status} {answered}"
    );
    assert_eq!(
        error_code(&answered),
        "InvalidRequestException",
        "repeat-association refusal code, got {answered}"
    );
    let children = {
        let state = service_state(&*engine).expect("the iotsitewise store");
        let store = state.lock();
        store
            .hierarchy_children(&fixture.parent_asset, &fixture.hierarchy_id)
            .to_vec()
    };
    assert_eq!(
        children,
        vec![fixture.child_asset.clone()],
        "the association set holds the child exactly once, got {children:?}"
    );
    assert_eq!(
        top_level_count(&router, &fixture.child_model).await,
        0,
        "once associated, the child asset is no longer top level"
    );

    // Disassociate through the `externalId:` label form — the same association,
    // named the other way the model admits.
    let (status, answered) = call(
        &router,
        "POST",
        "/assets/externalId:boiler-1/disassociate",
        Some(json!({
            "assetId": "externalId:boiler-1",
            "hierarchyId": "externalId:boiler-sensors",
            "childAssetId": "externalId:sensor-1",
        })),
    )
    .await;
    assert_eq!(
        status, 200,
        "DisassociateAssets of a stored association must succeed, got {status} {answered}"
    );
    assert_eq!(
        top_level_count(&router, &fixture.child_model).await,
        1,
        "once disassociated, the child asset is top level again"
    );

    // And the association is gone, so removing it again removes nothing.
    let (status, answered) = call(
        &router,
        "POST",
        &format!("/assets/{}/disassociate", fixture.parent_asset),
        Some(json!({
            "assetId": fixture.parent_asset,
            "hierarchyId": fixture.hierarchy_id,
            "childAssetId": fixture.child_asset,
        })),
    )
    .await;
    assert_eq!(
        status, 404,
        "a second DisassociateAssets has nothing to remove, got {status} {answered}"
    );
    assert_eq!(
        error_code(&answered),
        "ResourceNotFoundException",
        "second DisassociateAssets refusal code, got {answered}"
    );
    let _ = fixture.parent_model;
}

#[tokio::test]
async fn describe_asset_model_reports_a_stable_content_derived_etag() {
    let router = router();

    // Rows differ along what the ETag generalises over: a model's name, its
    // description, how many properties it carries, and whether it carries a
    // hierarchy. Each must get its own tag, and each tag must survive a re-read.
    // `assetModelName` is unique per account, so every row names its own model
    // and the name is one of the dimensions the rows vary rather than a constant.
    let child_model = create_model(
        &router,
        json!({ "assetModelName": "Child", "assetModelProperties": [] }),
    )
    .await;
    let cases: Vec<(&str, Value)> = vec![
        (
            "no properties",
            json!({ "assetModelName": "Alpha", "assetModelProperties": [] }),
        ),
        (
            "same shape, another name",
            json!({ "assetModelName": "Beta", "assetModelProperties": [] }),
        ),
        (
            "a description",
            json!({
                "assetModelName": "Gamma",
                "assetModelDescription": "a description of its own",
                "assetModelProperties": []
            }),
        ),
        (
            "one property",
            json!({
                "assetModelName": "Delta",
                "assetModelProperties": [{
                    "name": "Temperature",
                    "dataType": "DOUBLE",
                    "type": { "measurement": {} }
                }]
            }),
        ),
        (
            "two properties",
            json!({
                "assetModelName": "Epsilon",
                "assetModelProperties": [
                    {
                        "name": "Temperature",
                        "dataType": "DOUBLE",
                        "type": { "measurement": {} }
                    },
                    {
                        "name": "Serial",
                        "dataType": "STRING",
                        "type": { "attribute": {} }
                    }
                ]
            }),
        ),
        (
            "a hierarchy",
            json!({
                "assetModelName": "Zeta",
                "assetModelProperties": [],
                "assetModelHierarchies": [{
                    "name": "Children",
                    "childAssetModelId": child_model,
                }]
            }),
        ),
    ];

    let mut seen: Vec<(&str, String)> = Vec::new();
    for (case, body) in &cases {
        let model_id = create_model(&router, body.clone()).await;
        let tag = describe_e_tag(&router, &model_id).await;
        assert!(
            is_modelled_e_tag(&tag),
            "case {case:?}: the ETag must satisfy ^[\\w-]{{43}}$, got {tag:?}"
        );
        // A read never changes what a later caller sees: the same model read
        // again reports the same tag.
        let again = describe_e_tag(&router, &model_id).await;
        assert_eq!(
            again, tag,
            "case {case:?}: the ETag must be stable across reads, got {again:?} then {tag:?}"
        );
        for (earlier, earlier_tag) in &seen {
            assert_ne!(
                &tag, earlier_tag,
                "case {case:?}: must not share the ETag of case {earlier:?}, both {tag:?}"
            );
        }
        seen.push((case, tag));
    }
    assert_eq!(
        seen.len(),
        cases.len(),
        "every case contributed an ETag, got {seen:?}"
    );
}

#[tokio::test]
async fn delete_asset_model_evaluates_its_conditional_headers_before_deleting() {
    let router = router();

    // A model whose ACTIVE version exists: the conditional header pairs below are
    // evaluated against it. `$ETAG` is substituted with the tag the read reports,
    // which is the only value a real caller can put in `If-Match`.
    //
    // Rows span both headers, both `Match-For-Version-Type` values, the absent
    // version type, and a correct as well as a foreign `If-Match` token.
    let cases: Vec<ConditionalDeleteCase<'_>> = vec![
        ("no conditional header", vec![], 202),
        (
            "If-None-Match on an existing ACTIVE version",
            vec![("if-none-match", "*"), ("match-for-version-type", "ACTIVE")],
            412,
        ),
        (
            "If-None-Match on an existing LATEST version",
            vec![("if-none-match", "*"), ("match-for-version-type", "LATEST")],
            412,
        ),
        (
            "If-None-Match with no version type",
            vec![("if-none-match", "*")],
            412,
        ),
        (
            "If-Match with the tag the read reported",
            vec![("if-match", "$ETAG")],
            202,
        ),
        (
            "If-Match with the tag the read reported, ACTIVE",
            vec![("if-match", "$ETAG"), ("match-for-version-type", "ACTIVE")],
            202,
        ),
        (
            "If-Match with a foreign tag",
            vec![("if-match", FOREIGN_E_TAG)],
            412,
        ),
        (
            "If-Match with a foreign tag, LATEST",
            vec![("if-match", FOREIGN_E_TAG), ("match-for-version-type", "LATEST")],
            412,
        ),
    ];

    // One model per row, all created first and then waited for ONCE: the owed
    // CREATING -> ACTIVE transition is a clock the store shares, so every model
    // has its ACTIVE version after a single wait.
    let mut rows: Vec<ConditionalDeleteRow<'_>> = Vec::new();
    for (case, headers, expected) in &cases {
        let model_id = create_model(
            &router,
            json!({ "assetModelName": case, "assetModelProperties": [] }),
        )
        .await;
        rows.push((case, headers, *expected, model_id));
    }
    tokio::time::sleep(PAST_ACTIVATION).await;

    for (case, headers, expected, model_id) in &rows {
        let tag = describe_e_tag(&router, model_id).await;
        let headers: Vec<(&str, &str)> = headers
            .iter()
            .map(|(name, value)| {
                (*name, if *value == "$ETAG" { tag.as_str() } else { *value })
            })
            .collect();

        let (status, _, answered) = call_with_headers(
            &router,
            "DELETE",
            &format!("/asset-models/{model_id}"),
            None,
            &headers,
        )
        .await;
        assert_eq!(
            status, *expected,
            "case {case:?}: DeleteAssetModel {headers:?} answered {status} {answered}"
        );

        let (described_status, described) =
            call(&router, "GET", &format!("/asset-models/{model_id}"), None).await;
        if *expected == 412 {
            assert_eq!(
                error_code(&answered),
                "PreconditionFailedException",
                "case {case:?}: a failed precondition's code, got {answered}"
            );
            assert_eq!(
                answered["resourceId"].as_str(),
                Some(model_id.as_str()),
                "case {case:?}: the 412 names the resource it was evaluated on, got {answered}"
            );
            // The guard worked: the model is still there, still ACTIVE.
            assert_eq!(
                described_status, 200,
                "case {case:?}: a refused delete leaves the model, got {described}"
            );
            assert_eq!(
                described["assetModelStatus"]["state"], "ACTIVE",
                "case {case:?}: the surviving model is still ACTIVE, got {described}"
            );
        } else {
            assert_eq!(
                answered["assetModelStatus"]["state"], "DELETING",
                "case {case:?}: an accepted delete reports DELETING, got {answered}"
            );
            assert_eq!(
                described_status, 404,
                "case {case:?}: the deleted model is gone, got {described}"
            );
        }
    }
}

#[tokio::test]
async fn if_none_match_deletes_a_model_whose_targeted_version_does_not_exist_yet() {
    let router = router();
    // `If-None-Match: *` asks for the delete only if the version
    // `Match-For-Version-Type` names does NOT exist. Straight after the create the
    // owed CREATING -> ACTIVE transition has not fallen due, so there is no ACTIVE
    // version for the guard to protect and the delete goes through.
    let model_id = create_model(
        &router,
        json!({ "assetModelName": "StillCreating", "assetModelProperties": [] }),
    )
    .await;
    let (status, _, answered) = call_with_headers(
        &router,
        "DELETE",
        &format!("/asset-models/{model_id}"),
        None,
        &[("if-none-match", "*"), ("match-for-version-type", "ACTIVE")],
    )
    .await;
    assert_eq!(
        status, 202,
        "no ACTIVE version exists yet, so the conditional delete proceeds: got {status} {answered}"
    );
    assert_eq!(
        answered["assetModelStatus"]["state"], "DELETING",
        "the accepted conditional delete reports DELETING, got {answered}"
    );
}

#[tokio::test]
async fn a_delete_blocked_by_a_dependent_resource_is_a_terminal_request_error() {
    let router = router();

    // The one way a model is still in use when the delete is called, and the two
    // ways it is not. The rule: a model is held back by the assets created from it
    // ("You must delete all assets created from an asset model before you can
    // delete the model"), and NOT by a reference another model's definition makes
    // to it — a hierarchy naming it as its child model is a grouping, not a
    // dependant. The refusal is terminal — the caller must delete the assets first
    // — so it is 400 InvalidRequestException and never the retryable 409.
    let with_asset = create_model(
        &router,
        json!({
            "assetModelName": "HasAnAsset",
            "assetModelProperties": [{
                "name": "Temperature",
                "dataType": "DOUBLE",
                "type": { "measurement": {} }
            }]
        }),
    )
    .await;
    let dependent_asset = create_asset(
        &router,
        json!({ "assetName": "Instance", "assetModelId": with_asset }),
    )
    .await;

    let referenced = create_model(
        &router,
        json!({ "assetModelName": "Referenced", "assetModelProperties": [] }),
    )
    .await;
    let referrer = create_model(
        &router,
        json!({
            "assetModelName": "Referrer",
            "assetModelProperties": [],
            "assetModelHierarchies": [{
                "name": "Children",
                "childAssetModelId": referenced,
            }]
        }),
    )
    .await;
    let unused = create_model(
        &router,
        json!({ "assetModelName": "Unused", "assetModelProperties": [] }),
    )
    .await;

    let cases: Vec<(&str, &str, u16)> = vec![
        ("a live asset instantiates it", with_asset.as_str(), 400),
        (
            "another model's hierarchy merely names it",
            referenced.as_str(),
            202,
        ),
        ("nothing depends on it", unused.as_str(), 202),
    ];
    for (case, model_id, expected) in &cases {
        let (status, answered) =
            call(&router, "DELETE", &format!("/asset-models/{model_id}"), None).await;
        assert_eq!(
            status, *expected,
            "case {case:?}: DeleteAssetModel answered {status} {answered}"
        );
        if *expected == 400 {
            assert_eq!(
                error_code(&answered),
                "InvalidRequestException",
                "case {case:?}: a dependent resource is a terminal request error, got {answered}"
            );
        }
    }

    // Once the dependant is gone the identical call succeeds — which is what makes
    // the refusal above a precondition on the caller and not a transient conflict.
    let (status, answered) = call(
        &router,
        "DELETE",
        &format!("/assets/{dependent_asset}"),
        None,
    )
    .await;
    assert_eq!(status, 202, "DeleteAsset: got {status} {answered}");
    let (status, answered) = call(
        &router,
        "DELETE",
        &format!("/asset-models/{with_asset}"),
        None,
    )
    .await;
    assert_eq!(
        status, 202,
        "with its only asset deleted the model deletes: got {status} {answered}"
    );
    let _ = referrer;
}

#[tokio::test]
async fn delete_asset_is_refused_while_a_child_is_associated_under_any_hierarchy() {
    let (router, _engine) = router_with_engine();

    // A parent model with TWO hierarchies to the same child model, so the rule can
    // be exercised on each of them: the dependency is "a child under ANY hierarchy
    // of this asset", not a property of one particular hierarchy.
    let child_model = create_model(
        &router,
        json!({
            "assetModelName": "Sensor",
            "assetModelProperties": [{
                "name": "Temperature",
                "dataType": "DOUBLE",
                "type": { "measurement": {} }
            }]
        }),
    )
    .await;
    let parent_model = create_model(
        &router,
        json!({
            "assetModelName": "Boiler",
            "assetModelProperties": [],
            "assetModelHierarchies": [
                { "name": "Sensors", "childAssetModelId": child_model },
                { "name": "Spares", "childAssetModelId": child_model },
            ]
        }),
    )
    .await;
    let (status, described) =
        call(&router, "GET", &format!("/asset-models/{parent_model}"), None).await;
    assert_eq!(status, 200, "DescribeAssetModel: got {status} {described}");
    let hierarchies: Vec<(String, String)> = described["assetModelHierarchies"]
        .as_array()
        .unwrap_or_else(|| panic!("no hierarchies in {described}"))
        .iter()
        .map(|hierarchy| {
            (
                hierarchy["name"].as_str().unwrap_or("").to_string(),
                hierarchy["id"].as_str().unwrap_or("").to_string(),
            )
        })
        .collect();
    assert_eq!(hierarchies.len(), 2, "two hierarchies, got {described}");

    // One row per hierarchy: with a child associated under it, the parent's delete is
    // refused and the parent survives; once the child is disassociated, the IDENTICAL
    // delete is the call that succeeds.
    for (name, hierarchy_id) in &hierarchies {
        let parent = create_asset(
            &router,
            json!({ "assetName": format!("Boiler-{name}"), "assetModelId": parent_model }),
        )
        .await;
        let child = create_asset(
            &router,
            json!({ "assetName": format!("Sensor-{name}"), "assetModelId": child_model }),
        )
        .await;
        let triple = json!({
            "assetId": parent,
            "hierarchyId": hierarchy_id,
            "childAssetId": child,
        });
        let (status, answered) = call(
            &router,
            "POST",
            &format!("/assets/{parent}/associate"),
            Some(triple.clone()),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {name:?}: AssociateAssets got {status} {answered}"
        );

        let (status, refused) = call(&router, "DELETE", &format!("/assets/{parent}"), None).await;
        assert_eq!(
            status, 400,
            "case {name:?}: deleting a parent with a child must be refused, got {status} {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "InvalidRequestException",
            "case {name:?}: a dependent resource is a terminal request error, got {refused}"
        );
        // The parent survives, with the association intact.
        let (status, survived) = call(&router, "GET", &format!("/assets/{parent}"), None).await;
        assert_eq!(
            status, 200,
            "case {name:?}: a refused delete leaves the parent, got {status} {survived}"
        );
        assert_eq!(
            top_level_count(&router, &child_model).await,
            0,
            "case {name:?}: the child is still associated"
        );

        // Deleting the CHILD is refused for the same reason seen from the other end:
        // it is "associated to another asset", which the operation's documentation
        // forbids deleting.
        let (status, refused) = call(&router, "DELETE", &format!("/assets/{child}"), None).await;
        assert_eq!(
            status, 400,
            "case {name:?}: deleting an associated child must be refused, got {status} {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "InvalidRequestException",
            "case {name:?}: a still-associated asset is a terminal request error, got {refused}"
        );
        let (status, survived) = call(&router, "GET", &format!("/assets/{child}"), None).await;
        assert_eq!(
            status, 200,
            "case {name:?}: a refused delete leaves the child, got {status} {survived}"
        );

        // Disassociated, BOTH ends delete — which is what makes the two refusals
        // preconditions on the caller rather than broken operations.
        let (status, answered) = call(
            &router,
            "POST",
            &format!("/assets/{parent}/disassociate"),
            Some(triple),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {name:?}: DisassociateAssets got {status} {answered}"
        );
        let (status, answered) = call(&router, "DELETE", &format!("/assets/{child}"), None).await;
        assert_eq!(
            status, 202,
            "case {name:?}: the disassociated child deletes, got {status} {answered}"
        );
        let (status, answered) = call(&router, "DELETE", &format!("/assets/{parent}"), None).await;
        assert_eq!(
            status, 202,
            "case {name:?}: with no children the delete succeeds, got {status} {answered}"
        );
        assert_eq!(
            answered["assetStatus"]["state"], "DELETING",
            "case {name:?}: the accepted delete reports DELETING, got {answered}"
        );
    }

    // An asset that is in no hierarchy at all deletes straight away: the refusals
    // above are about the association and nothing else.
    let lone = create_asset(
        &router,
        json!({ "assetName": "Boiler-lone", "assetModelId": parent_model }),
    )
    .await;
    let (status, answered) = call(&router, "DELETE", &format!("/assets/{lone}"), None).await;
    assert_eq!(
        status, 202,
        "an unassociated asset deletes, got {status} {answered}"
    );

    // The same refusal when the child goes away first by DELETING the parent's
    // dependency through DisassociateAssets rather than per hierarchy row.
    let parent = create_asset(
        &router,
        json!({ "assetName": "Boiler-disassociated", "assetModelId": parent_model }),
    )
    .await;
    let child = create_asset(
        &router,
        json!({ "assetName": "Sensor-disassociated", "assetModelId": child_model }),
    )
    .await;
    let triple = json!({
        "assetId": parent,
        "hierarchyId": hierarchies[0].1,
        "childAssetId": child,
    });
    let (status, answered) = call(
        &router,
        "POST",
        &format!("/assets/{parent}/associate"),
        Some(triple.clone()),
    )
    .await;
    assert_eq!(status, 200, "AssociateAssets got {status} {answered}");
    for (case, asset) in [("the parent", &parent), ("the child", &child)] {
        let (status, refused) = call(&router, "DELETE", &format!("/assets/{asset}"), None).await;
        assert_eq!(
            status, 400,
            "case {case:?}: the delete is refused while the association stands, got {status} {refused}"
        );
    }
    let (status, answered) = call(
        &router,
        "POST",
        &format!("/assets/{parent}/disassociate"),
        Some(triple),
    )
    .await;
    assert_eq!(status, 200, "DisassociateAssets got {status} {answered}");
    for (case, asset) in [("the parent", &parent), ("the child", &child)] {
        let (status, answered) = call(&router, "DELETE", &format!("/assets/{asset}"), None).await;
        assert_eq!(
            status, 202,
            "case {case:?}: once disassociated the delete succeeds, got {status} {answered}"
        );
    }
}

#[tokio::test]
async fn a_violation_of_a_modelled_rule_is_refused_with_this_services_own_error_code() {
    let router = router();
    let model_id = create_model(
        &router,
        json!({ "assetModelName": "Tagged", "assetModelProperties": [] }),
    )
    .await;
    let resource_arn = format!(
        "arn:aws:iotsitewise:{REGION}:000000000000:asset-model/{model_id}"
    );

    // Rows span the constraint classes the model declares and the operations they
    // sit on: a map longer than its `@length` max, a map shorter than its min, an
    // absent required member, and an integer outside its `@range`. Every one of
    // them is the SAME refusal to a caller, so every one must carry the code this
    // service models.
    let many_tags: Value = Value::Object(
        (0..51)
            .map(|at| (format!("key{at}"), Value::String(format!("value{at}"))))
            .collect(),
    );
    let cases: Vec<(&str, &str, String, Option<Value>, &str)> = vec![
        (
            "51 tags, one past @length max 50",
            "POST",
            format!("/tags?resourceArn={resource_arn}"),
            Some(json!({ "tags": many_tags })),
            "tags",
        ),
        (
            "no tags at all, one under @length min 1",
            "POST",
            format!("/tags?resourceArn={resource_arn}"),
            Some(json!({ "tags": {} })),
            "tags",
        ),
        (
            "CreateAssetModel without the required assetModelName",
            "POST",
            "/asset-models".to_string(),
            Some(json!({ "assetModelProperties": [] })),
            "assetModelName",
        ),
        (
            "ListAssets maxResults past @range max 250",
            "GET",
            format!("/assets?assetModelId={model_id}&maxResults=251"),
            None,
            "maxResults",
        ),
        (
            "ListAssets maxResults under @range min 1",
            "GET",
            format!("/assets?assetModelId={model_id}&maxResults=0"),
            None,
            "maxResults",
        ),
    ];

    for (case, method, uri, body, member) in &cases {
        let (status, answered) = call(&router, method, uri, body.clone()).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a modelled-rule violation is a 400, got {status} {answered}"
        );
        assert_eq!(
            error_code(&answered),
            "InvalidRequestException",
            "case {case:?}: the refusal code this service models, got {answered}"
        );
        let message = error_message(&answered);
        assert!(
            message.contains(member),
            "case {case:?}: the message names the member {member:?}, got {message:?}"
        );
    }
}

#[tokio::test]
async fn not_found_and_uniqueness_refusals_are_worded_from_the_request() {
    let router = router();
    let model_id = create_model(
        &router,
        json!({
            "assetModelName": "Boiler",
            "assetModelProperties": [],
            "assetModelExternalId": "boiler-model"
        }),
    )
    .await;
    let asset_id = create_asset(
        &router,
        json!({
            "assetName": "Boiler1",
            "assetModelId": model_id,
            "assetExternalId": "boiler-1"
        }),
    )
    .await;

    // Rows span the operations that look a resource up and both label forms the
    // model admits. The expected message is built from the row's own identifier,
    // so no row can pass by matching a hard-coded id.
    let cases: Vec<(&str, &str, String, Option<Value>, String)> = vec![
        (
            "DescribeAsset, unknown id",
            "GET",
            format!("/assets/{UNKNOWN_ID}"),
            None,
            format!("Asset with ID {UNKNOWN_ID} not found"),
        ),
        (
            // Addressed by the selector, the asset's absence is reported as an
            // EXTERNAL ID that does not exist, by the bare external id.
            "DescribeAsset, unknown externalId label",
            "GET",
            "/assets/externalId:not-an-asset".to_string(),
            None,
            "asset with externalId: not-an-asset doesn't exist.".to_string(),
        ),
        (
            "DeleteAsset, unknown id",
            "DELETE",
            format!("/assets/{OTHER_UNKNOWN_ID}"),
            None,
            format!("Asset with ID {OTHER_UNKNOWN_ID} not found"),
        ),
        (
            "DescribeAssetModel, unknown id",
            "GET",
            format!("/asset-models/{UNKNOWN_ID}"),
            None,
            format!("Asset Model with ID {UNKNOWN_ID} not found"),
        ),
        (
            "DeleteAssetModel, unknown externalId label",
            "DELETE",
            "/asset-models/externalId:not-a-model".to_string(),
            None,
            "Asset Model with ID externalId:not-a-model not found".to_string(),
        ),
        (
            "ListAssets, unknown asset model",
            "GET",
            format!("/assets?assetModelId={UNKNOWN_ID}"),
            None,
            format!("Asset Model with ID {UNKNOWN_ID} not found"),
        ),
        (
            "CreateAsset, unknown asset model",
            "POST",
            "/assets".to_string(),
            Some(json!({ "assetName": "Orphan", "assetModelId": UNKNOWN_ID })),
            format!("Asset Model with ID {UNKNOWN_ID} not found"),
        ),
    ];

    for (case, method, uri, body, expected) in &cases {
        let (status, answered) = call(&router, method, uri, body.clone()).await;
        assert_eq!(
            status, 404,
            "case {case:?}: an absent resource is a 404, got {status} {answered}"
        );
        assert_eq!(
            error_code(&answered),
            "ResourceNotFoundException",
            "case {case:?}: the refusal code, got {answered}"
        );
        assert_eq!(
            error_message(&answered),
            expected,
            "case {case:?}: expected {expected:?}, got {:?}",
            error_message(&answered)
        );
    }

    // A uniqueness refusal names the DIMENSION of the request that collided, and
    // points at the resource that already holds it — never at an id the rejected
    // request would have minted.
    let (status, answered) = call(
        &router,
        "POST",
        "/assets",
        Some(json!({
            "assetName": "Boiler1Again",
            "assetModelId": model_id,
            "assetExternalId": "boiler-1"
        })),
    )
    .await;
    assert_eq!(
        status, 409,
        "a duplicate assetExternalId is a 409, got {status} {answered}"
    );
    assert_eq!(
        error_code(&answered),
        "ResourceAlreadyExistsException",
        "duplicate externalId refusal code, got {answered}"
    );
    assert_eq!(
        error_message(&answered),
        "Another asset is already using the same externalId",
        "duplicate externalId message, got {answered}"
    );
    assert_eq!(
        answered["resourceId"].as_str(),
        Some(asset_id.as_str()),
        "the refusal points at the asset that holds the externalId, got {answered}"
    );

    // The same rule on the model's own uniqueness dimension.
    let (status, answered) = call(
        &router,
        "POST",
        "/asset-models",
        Some(json!({
            "assetModelName": "BoilerAgain",
            "assetModelProperties": [],
            "assetModelExternalId": "boiler-model"
        })),
    )
    .await;
    assert_eq!(
        status, 409,
        "a duplicate assetModelExternalId is a 409, got {status} {answered}"
    );
    assert_eq!(
        error_message(&answered),
        "Another asset model is already using the same externalId",
        "duplicate model externalId message, got {answered}"
    );
}
