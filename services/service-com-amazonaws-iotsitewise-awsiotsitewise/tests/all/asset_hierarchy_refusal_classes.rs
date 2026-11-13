// SPDX-License-Identifier: CC0-1.0

//! Which refusal CLASS the asset-hierarchy surface answers with, and in whose
//! words — pinned over a SET of inputs rather than over the one request a
//! recording happened to carry. Everything is driven over HTTP through the same
//! `MultiServiceRouter` the emulator boots with, so every assertion is about the
//! wire behaviour a real client sees.
//!
//! The rules this suite pins:
//!
//! 1. A `CustomID` member names a resource in one of two forms — a non-nil
//!    lowercase-hexadecimal UUID, or `externalId:` followed by an external id —
//!    and a value in NEITHER form is refused as a malformed request (400
//!    `InvalidRequestException`, the request-validation report naming the member
//!    and the expression it had to satisfy) BEFORE any lookup, while a
//!    well-formed reference that names nothing is the absent resource (404).
//! 2. A `hierarchyId` the parent asset's model does not declare is a statement
//!    about the REQUEST on the operation that would record an association (400,
//!    naming the hierarchy and the asset as the request wrote them) and the
//!    absent hierarchy on the operation that removes one (404), which answers
//!    the same for a hierarchy holding no such child.
//! 3. An asset has at most ONE parent: an association for a child that already
//!    has one is the terminal 400 carrying one fixed sentence, however the child
//!    came to have that parent and however recently.
//! 4. A delete of an asset still in a hierarchy is refused with the fixed
//!    sentence for the direction the association runs in, carrying no identifier
//!    of the request or of the store.
//! 5. `DeleteAssetModel` is held back by the ASSETS created from the model, and
//!    not by a reference another model's definition makes to it (a hierarchy
//!    naming it as its child model, a property formula reading through one).

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The crate's shared harness lives in `asset_lifecycle`, the create/associate
// walk in `asset_hierarchies`: this suite reaches both rather than growing a
// second copy of them.
use crate::asset_hierarchies::{associate, create_asset, create_model, hierarchy_id};
use crate::asset_lifecycle::{call, error_code, router};

/// The `@pattern` the model declares on `CustomID`, written out BY HAND from
/// `smithy_query(query="shape", shape="CustomID")` as an independent oracle —
/// less the `$` that closes it, which is how this service quotes an expression.
const CUSTOM_ID_PATTERN: &str = r"^(?!00000000-0000-0000-0000-000000000000)[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$|^externalId:[a-zA-Z0-9_][a-zA-Z_\-0-9.:]*[a-zA-Z0-9_]+";

/// A well-formed `CustomID` reference that names no stored resource.
const ABSENT_ID: &str = "9b9b9b9b-1111-2222-3333-444444444444";

/// A second one, so a row can name another absent resource.
const OTHER_ABSENT_ID: &str = "8c8c8c8c-1111-2222-3333-444444444444";

/// The fixed sentence a delete of an asset that is still somebody's CHILD is
/// refused with, written out by hand.
const DISASSOCIATE_FROM_THE_PARENT: &str =
    "You must disassociate the asset from its parent asset before you can delete it.";

/// The same for a delete of an asset whose hierarchies still hold children.
const DISASSOCIATE_THE_CHILDREN: &str =
    "You must disassociate the asset from all of its children assets before you can delete it.";

/// One clause of the request-validation report a member outside its `@pattern`
/// is refused with, written out by hand from the clause form the framework
/// states.
fn pattern_report_clause(member_path: &str) -> String {
    format!(
        "Value at '{member_path}' failed to satisfy constraint: Member must satisfy regular \
         expression pattern: {CUSTOM_ID_PATTERN}"
    )
}

/// The whole report for ONE such member: the clause in the framework's envelope.
fn pattern_report(member_path: &str) -> String {
    format!(
        "1 validation error detected: {}",
        pattern_report_clause(member_path)
    )
}

/// The `message` of an answer, or `""`.
fn message(body: &Value) -> &str {
    body.get("message").and_then(Value::as_str).unwrap_or("")
}

/// `label` as a URI path segment: the only character these rows carry that a URI
/// cannot hold literally is the space, which is percent-encoded here and arrives
/// at the handler decoded again, so the clause still echoes the member path and
/// the request still carries the label the row is about.
fn uri_label(label: &str) -> String {
    label.replace(' ', "%20")
}

/// Every `CustomID` label that is NOT a reference at all, as `(case, label)`:
/// one per thing the two branches of the pattern rule out.
///
/// Each is at least 13 characters long, the member's `@length` minimum, so the
/// boundary validator passes it through and the REFERENCE FORM is what refuses
/// it. The external-id branch constrains its two ENDS to `[a-zA-Z0-9_]` and its
/// interior to `[a-zA-Z_\-0-9.:]`, which is why a leading and a trailing
/// separator are both rows.
fn malformed_references() -> Vec<(&'static str, String)> {
    vec![
        ("a resource NAME", "WindTurbineModel".to_string()),
        (
            "the nil UUID the pattern's lookahead excludes",
            "00000000-0000-0000-0000-000000000000".to_string(),
        ),
        (
            "an UPPERCASE UUID",
            "9B9B9B9B-1111-2222-3333-444444444444".to_string(),
        ),
        (
            "a UUID with a non-hexadecimal digit",
            "9b9b9b9g-1111-2222-3333-444444444444".to_string(),
        ),
        (
            "a UUID whose groups are mis-sized",
            "9b9b9b9b1-111-2222-3333-444444444444".to_string(),
        ),
        (
            "an externalId: label opening with a separator",
            "externalId:-turbine-model".to_string(),
        ),
        (
            "an externalId: label closing with a separator",
            "externalId:turbine-model.".to_string(),
        ),
        (
            "an externalId: label holding a character outside the class",
            "externalId:turbine model".to_string(),
        ),
        (
            "an externalId: label made of whitespace",
            "externalId:   ".to_string(),
        ),
    ]
}

/// Every `CustomID` label that IS a reference, so a lookup decides it: both
/// forms the pattern admits, and the interior characters its second branch
/// admits.
fn well_formed_references() -> Vec<(&'static str, String)> {
    vec![
        ("a lowercase UUID", ABSENT_ID.to_string()),
        (
            "an externalId: label",
            "externalId:no_such_model".to_string(),
        ),
        (
            "an externalId: label carrying every interior character the class admits",
            "externalId:a-b.c:d_e9".to_string(),
        ),
    ]
}

#[tokio::test]
async fn an_asset_model_reference_is_checked_for_its_form_before_any_lookup() {
    let router = router();
    // One stored model, so a row can carry a reference that resolves beside the
    // one that does not.
    let held = create_model(&router, json!({"assetModelName": "Turbine"})).await;

    for (case, label) in malformed_references() {
        // DescribeAssetModel: the label is the whole address of the read, so the
        // report names the top-level member.
        let (status, body) = call(
            &router,
            "GET",
            &format!("/asset-models/{}", uri_label(&label)),
            None,
        )
        .await;
        let expected = pattern_report("assetModelId");
        assert_eq!(
            status, 400,
            "case {case:?} on DescribeAssetModel: expected 400, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidRequestException",
            "case {case:?} on DescribeAssetModel: unexpected error code in {body}"
        );
        assert_eq!(
            message(&body),
            expected,
            "case {case:?} on DescribeAssetModel: expected {expected:?}, got {:?}",
            message(&body)
        );

        // CreateAssetModel: the SECOND hierarchy definition carries it, so the
        // report names that element's member, counted the way the framework's own
        // walk counts a list position.
        let (status, body) = call(
            &router,
            "POST",
            "/asset-models",
            Some(json!({
                "assetModelName": format!("Farm {case}"),
                "assetModelHierarchies": [
                    {"name": "held", "childAssetModelId": held},
                    {"name": "malformed", "childAssetModelId": label},
                ],
            })),
        )
        .await;
        let expected = pattern_report("assetModelHierarchies.2.member.childAssetModelId");
        assert_eq!(
            status, 400,
            "case {case:?} on CreateAssetModel: expected 400, got {status} {body}"
        );
        assert_eq!(
            message(&body),
            expected,
            "case {case:?} on CreateAssetModel: expected {expected:?}, got {:?}",
            message(&body)
        );

        // UpdateAssetModel enforces the same rule on the same member.
        let (status, body) = call(
            &router,
            "PUT",
            &format!("/asset-models/{held}"),
            Some(json!({
                "assetModelName": "Turbine",
                "assetModelHierarchies": [
                    {"name": "malformed", "childAssetModelId": label},
                ],
            })),
        )
        .await;
        let expected = pattern_report("assetModelHierarchies.1.member.childAssetModelId");
        assert_eq!(
            status, 400,
            "case {case:?} on UpdateAssetModel: expected 400, got {status} {body}"
        );
        assert_eq!(
            message(&body),
            expected,
            "case {case:?} on UpdateAssetModel: expected {expected:?}, got {:?}",
            message(&body)
        );
    }

    // The other half of the rule: a reference in one of the two admitted forms
    // that names no stored model is the ABSENT RESOURCE, not a malformed request.
    for (case, label) in well_formed_references() {
        let (status, body) = call(
            &router,
            "GET",
            &format!("/asset-models/{}", uri_label(&label)),
            None,
        )
        .await;
        assert_eq!(
            status, 404,
            "case {case:?} on DescribeAssetModel: expected 404, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?} on DescribeAssetModel: unexpected error code in {body}"
        );

        let (status, body) = call(
            &router,
            "POST",
            "/asset-models",
            Some(json!({
                "assetModelName": format!("Farm {case}"),
                "assetModelHierarchies": [
                    {"name": "absent", "childAssetModelId": label},
                ],
            })),
        )
        .await;
        assert_eq!(
            status, 404,
            "case {case:?} on CreateAssetModel: expected 404, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?} on CreateAssetModel: unexpected error code in {body}"
        );
    }

    // Nothing above wrote: the model that was there still carries no hierarchy,
    // so every refusal left the store as it was.
    let (status, described) = call(&router, "GET", &format!("/asset-models/{held}"), None).await;
    assert_eq!(status, 200, "the held model must still read: {described}");
    assert_eq!(
        described["assetModelHierarchies"],
        json!([]),
        "a refused update must leave the model's hierarchies alone, got {described}"
    );
}

#[tokio::test]
async fn every_malformed_reference_of_one_request_is_reported_together() {
    let router = router();

    // The report is over the members TOGETHER, so neither the ORDER the faults
    // arrive in nor the presence of a second kind of fault changes it. Rows: a
    // malformed reference after a well-formed one that names NOTHING (the
    // resolution of element 1 must not pre-empt the syntax of element 2), the same
    // pair the other way round, and two malformed references, which are stated as
    // the two clauses they are.
    let first = pattern_report_clause("assetModelHierarchies.1.member.childAssetModelId");
    let second = pattern_report_clause("assetModelHierarchies.2.member.childAssetModelId");
    let rows: Vec<(&str, Value, String)> = vec![
        (
            "a malformed reference after a well-formed one that names nothing",
            json!([
                {"name": "absent", "childAssetModelId": ABSENT_ID},
                {"name": "malformed", "childAssetModelId": "WindTurbineModel"},
            ]),
            format!("1 validation error detected: {second}"),
        ),
        (
            "a malformed reference before a well-formed one that names nothing",
            json!([
                {"name": "malformed", "childAssetModelId": "WindTurbineModel"},
                {"name": "absent", "childAssetModelId": ABSENT_ID},
            ]),
            format!("1 validation error detected: {first}"),
        ),
        (
            "two malformed references",
            json!([
                {"name": "first", "childAssetModelId": "WindTurbineModel"},
                {"name": "second", "childAssetModelId": "externalId:turbine-model."},
            ]),
            format!("2 validation errors detected: {first}; {second}"),
        ),
    ];
    for (case, hierarchies, expected) in &rows {
        let (status, body) = call(
            &router,
            "POST",
            "/asset-models",
            Some(json!({
                "assetModelName": format!("Farm {case}"),
                "assetModelHierarchies": hierarchies,
            })),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidRequestException",
            "case {case:?}: unexpected error code in {body}"
        );
        assert_eq!(
            message(&body),
            expected,
            "case {case:?}: expected {expected:?}, got {:?}",
            message(&body)
        );
    }
}

/// The hand-written oracle above must be the model's own expression: the
/// `@pattern` the generated input schema carries for a `CustomID` member, less
/// the `$` this service's refusals drop. Read off the schema rather than
/// transcribed a second time, so a regenerated model fails this test instead of
/// quietly leaving a stale expression in a refusal.
#[tokio::test]
async fn the_quoted_expression_is_the_models_own_pattern() {
    use aws_smithy_schema_server::constraints::PatternTrait;
    use service_com_amazonaws_iotsitewise_awsiotsitewise::ops::types::DESCRIBE_ASSET_MODEL_INPUT_SCHEMA;

    let member = DESCRIBE_ASSET_MODEL_INPUT_SCHEMA
        .members()
        .iter()
        .find(|member| member.member_name() == Some("assetModelId"))
        .expect("DescribeAssetModel declares an assetModelId member");
    let traits = member
        .traits()
        .expect("the assetModelId member carries its modelled constraints");
    let declared = traits
        .get(&PatternTrait::TRAIT_ID)
        .and_then(|held| held.as_any().downcast_ref::<PatternTrait>())
        .expect("the assetModelId member declares a @pattern")
        .regex;
    let expected = declared
        .strip_suffix('$')
        .expect("the declared CustomID expression closes with an anchor");
    assert_eq!(
        CUSTOM_ID_PATTERN, expected,
        "the expression a refusal quotes must be the model's own, less its closing anchor: \
         the model declares {declared:?}"
    );
}

/// The fixture the association rows run over: a `Farm` model with two
/// hierarchies over a `Turbine` model, a `Gearbox` model with a hierarchy of its
/// own (whose id is therefore a hierarchy id the farm's model does NOT declare),
/// two farms and two turbines.
struct Farm {
    router: MultiServiceRouter,
    farm: String,
    other_farm: String,
    turbines: String,
    spares: String,
    /// A hierarchy id some OTHER asset model declares.
    elsewhere: String,
    turbine: String,
    other_turbine: String,
}

async fn farm_fixture() -> Farm {
    let router = router();
    let turbine_model = create_model(
        &router,
        json!({"assetModelName": "Turbine", "assetModelExternalId": "turbine_model"}),
    )
    .await;
    let farm_model = create_model(
        &router,
        json!({
            "assetModelName": "Farm",
            "assetModelExternalId": "farm_model",
            "assetModelHierarchies": [
                {
                    "name": "turbines",
                    "externalId": "farm_turbines",
                    "childAssetModelId": turbine_model,
                },
                {"name": "spares", "childAssetModelId": turbine_model},
            ],
        }),
    )
    .await;
    let gearbox_model = create_model(
        &router,
        json!({
            "assetModelName": "Gearbox",
            "assetModelHierarchies": [
                {"name": "spindles", "childAssetModelId": turbine_model},
            ],
        }),
    )
    .await;
    let turbines = hierarchy_id(&router, &farm_model, "turbines").await;
    let spares = hierarchy_id(&router, &farm_model, "spares").await;
    let elsewhere = hierarchy_id(&router, &gearbox_model, "spindles").await;

    let farm = create_asset(
        &router,
        json!({
            "assetName": "Farm 1",
            "assetModelId": farm_model,
            "assetExternalId": "farm_1",
        }),
    )
    .await;
    let other_farm = create_asset(
        &router,
        json!({"assetName": "Farm 2", "assetModelId": farm_model}),
    )
    .await;
    let turbine = create_asset(
        &router,
        json!({
            "assetName": "Turbine 1",
            "assetModelId": turbine_model,
            "assetExternalId": "turbine_1",
        }),
    )
    .await;
    let other_turbine = create_asset(
        &router,
        json!({"assetName": "Turbine 2", "assetModelId": turbine_model}),
    )
    .await;

    Farm {
        router,
        farm,
        other_farm,
        turbines,
        spares,
        elsewhere,
        turbine,
        other_turbine,
    }
}

#[tokio::test]
async fn a_hierarchy_the_parents_model_does_not_declare_is_a_request_error_on_associate() {
    let fixture = farm_fixture().await;

    // Rows: one per way a well-formed `hierarchyId` can fail to name a hierarchy
    // of the PARENT's model — an id nothing declares, an `externalId:` label
    // nothing declares, and the id of a hierarchy ANOTHER model declares — each
    // with the parent named by its id and by its `externalId:` alias, because the
    // sentence echoes both labels exactly as the request wrote them.
    let hierarchies: Vec<(&str, String)> = vec![
        ("an id no model declares", ABSENT_ID.to_string()),
        (
            "an externalId: label no model declares",
            "externalId:no_such_hierarchy".to_string(),
        ),
        (
            "the id of a hierarchy another model declares",
            fixture.elsewhere.clone(),
        ),
    ];
    let parents: Vec<(&str, String)> = vec![
        ("by id", fixture.farm.clone()),
        ("by externalId alias", "externalId:farm_1".to_string()),
    ];

    for (hierarchy_case, hierarchy) in &hierarchies {
        for (parent_case, parent) in &parents {
            let case = format!("{hierarchy_case}, parent {parent_case}");
            let (status, body) =
                associate(&fixture.router, parent, hierarchy, &fixture.turbine).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected 400, got {status} {body}"
            );
            assert_eq!(
                error_code(&body),
                "InvalidRequestException",
                "case {case:?}: unexpected error code in {body}"
            );
            let expected =
                format!("Given HierarchyId {hierarchy} not present in the Asset {parent}");
            assert_eq!(
                message(&body),
                expected,
                "case {case:?}: expected {expected:?}, got {:?}",
                message(&body)
            );
        }
    }

    // An ASSET the request names but the store does not hold stays the absent
    // resource, so the two refusals are not collapsed into one.
    for (case, parent, child) in [
        ("an absent parent", ABSENT_ID, fixture.turbine.as_str()),
        ("an absent child", fixture.farm.as_str(), OTHER_ABSENT_ID),
    ] {
        let (status, body) = associate(&fixture.router, parent, &fixture.turbines, child).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: unexpected error code in {body}"
        );
    }
}

#[tokio::test]
async fn an_association_for_a_child_that_already_has_a_parent_is_the_one_fixed_sentence() {
    let fixture = farm_fixture().await;

    let (status, body) = associate(
        &fixture.router,
        &fixture.farm,
        &fixture.turbines,
        &fixture.turbine,
    )
    .await;
    assert_eq!(status, 200, "the first association: got {status} {body}");

    // Rows: every way the child already has a parent — the same edge again, the
    // other hierarchy of the same parent, another parent entirely — each asked
    // IMMEDIATELY after the association that created the fact, so the answer
    // cannot depend on how far that association has settled.
    let rows: Vec<(&str, String, String)> = vec![
        (
            "the same edge again",
            fixture.farm.clone(),
            fixture.turbines.clone(),
        ),
        (
            "the same edge again, the parent by externalId alias",
            "externalId:farm_1".to_string(),
            fixture.turbines.clone(),
        ),
        (
            "another hierarchy of the same parent",
            fixture.farm.clone(),
            fixture.spares.clone(),
        ),
        (
            "another parent of the same model",
            fixture.other_farm.clone(),
            fixture.turbines.clone(),
        ),
    ];
    for (case, parent, hierarchy) in &rows {
        let (status, body) = associate(&fixture.router, parent, hierarchy, &fixture.turbine).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidRequestException",
            "case {case:?}: unexpected error code in {body}"
        );
        assert_eq!(
            message(&body),
            "Child asset already has a parent",
            "case {case:?}: expected the one fixed sentence, got {:?}",
            message(&body)
        );
    }

    // A child that has NO parent is still associated, so the rule refuses the
    // fact it is about and nothing else.
    let (status, body) = associate(
        &fixture.router,
        &fixture.farm,
        &fixture.turbines,
        &fixture.other_turbine,
    )
    .await;
    assert_eq!(
        status, 200,
        "a second, parentless child: got {status} {body}"
    );
}

#[tokio::test]
async fn disassociate_states_the_hierarchy_it_found_no_association_under() {
    let fixture = farm_fixture().await;
    let (status, body) = associate(
        &fixture.router,
        &fixture.farm,
        &fixture.turbines,
        &fixture.turbine,
    )
    .await;
    assert_eq!(status, 200, "the association: got {status} {body}");

    // Rows: one per way a disassociation names no recorded association — a
    // hierarchy no model declares (in both label forms), a hierarchy ANOTHER
    // model declares, a hierarchy of this very model that does not hold this
    // child, and the holding hierarchy asked for a child it does not hold. Every
    // one of them reports the hierarchy, by the label the request wrote.
    let rows: Vec<(&str, String, String)> = vec![
        (
            "a hierarchy no model declares",
            ABSENT_ID.to_string(),
            fixture.turbine.clone(),
        ),
        (
            "an externalId: label no model declares",
            "externalId:no_such_hierarchy".to_string(),
            fixture.turbine.clone(),
        ),
        (
            "a hierarchy another model declares",
            fixture.elsewhere.clone(),
            fixture.turbine.clone(),
        ),
        (
            "a declared hierarchy that does not hold this child",
            fixture.spares.clone(),
            fixture.turbine.clone(),
        ),
        (
            "the holding hierarchy, for a child it does not hold",
            fixture.turbines.clone(),
            fixture.other_turbine.clone(),
        ),
    ];
    for (case, hierarchy, child) in &rows {
        let (status, body) = call(
            &fixture.router,
            "POST",
            &format!("/assets/{}/disassociate", fixture.farm),
            Some(json!({ "hierarchyId": hierarchy, "childAssetId": child })),
        )
        .await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: unexpected error code in {body}"
        );
        let expected = format!("A hierarchy with ID {hierarchy} doesn't exist");
        assert_eq!(
            message(&body),
            expected,
            "case {case:?}: expected {expected:?}, got {:?}",
            message(&body)
        );
    }

    // The association every row above failed to find is still there to remove.
    let (status, body) = call(
        &fixture.router,
        "POST",
        &format!("/assets/{}/disassociate", fixture.farm),
        Some(json!({
            "hierarchyId": fixture.turbines,
            "childAssetId": fixture.turbine,
        })),
    )
    .await;
    assert_eq!(status, 200, "the real disassociation: got {status} {body}");
}

#[tokio::test]
async fn deleting_an_associated_asset_is_refused_with_the_sentence_for_its_direction() {
    let fixture = farm_fixture().await;
    let (status, body) = associate(
        &fixture.router,
        &fixture.farm,
        &fixture.turbines,
        &fixture.turbine,
    )
    .await;
    assert_eq!(status, 200, "the association: got {status} {body}");

    // Rows: both ends of one association, each addressed by its id and by its
    // `externalId:` alias — which end the delete names decides the sentence, and
    // how it names it decides nothing, because neither sentence carries any
    // identifier at all.
    let rows: Vec<(&str, String, &str)> = vec![
        (
            "the child, by id",
            fixture.turbine.clone(),
            DISASSOCIATE_FROM_THE_PARENT,
        ),
        (
            "the child, by externalId alias",
            "externalId:turbine_1".to_string(),
            DISASSOCIATE_FROM_THE_PARENT,
        ),
        (
            "the parent, by id",
            fixture.farm.clone(),
            DISASSOCIATE_THE_CHILDREN,
        ),
        (
            "the parent, by externalId alias",
            "externalId:farm_1".to_string(),
            DISASSOCIATE_THE_CHILDREN,
        ),
    ];
    let identifiers = [
        fixture.farm.as_str(),
        fixture.turbine.as_str(),
        fixture.turbines.as_str(),
    ];
    for (case, label, expected) in &rows {
        let (status, body) =
            call(&fixture.router, "DELETE", &format!("/assets/{label}"), None).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidRequestException",
            "case {case:?}: unexpected error code in {body}"
        );
        assert_eq!(
            message(&body),
            *expected,
            "case {case:?}: expected {expected:?}, got {:?}",
            message(&body)
        );
        for identifier in identifiers {
            assert!(
                !message(&body).contains(identifier),
                "case {case:?}: the sentence carries no identifier, but {identifier:?} appears \
                 in {:?}",
                message(&body)
            );
        }
    }

    // Once the association is gone both deletes go through, so each refusal was
    // about the association and nothing else.
    let (status, body) = call(
        &fixture.router,
        "POST",
        &format!("/assets/{}/disassociate", fixture.farm),
        Some(json!({
            "hierarchyId": fixture.turbines,
            "childAssetId": fixture.turbine,
        })),
    )
    .await;
    assert_eq!(status, 200, "the disassociation: got {status} {body}");
    for label in [fixture.turbine.as_str(), fixture.farm.as_str()] {
        let (status, body) =
            call(&fixture.router, "DELETE", &format!("/assets/{label}"), None).await;
        assert_eq!(
            status, 202,
            "deleting {label} once disassociated: got {status} {body}"
        );
    }
}

/// How a row of the delete suite below references the model it is about from
/// another model's definition.
enum Reference {
    /// A hierarchy naming it as its `childAssetModelId`.
    Hierarchy,
    /// A metric of the referring model reading one of its properties THROUGH
    /// such a hierarchy.
    Formula,
    /// Nothing references it at all.
    None,
}

#[tokio::test]
async fn an_asset_model_a_definition_merely_references_is_deleted() {
    // Rows: the two references another model's DEFINITION can make to a model,
    // plus the row where nothing references it. None of them holds the delete
    // back, so each is accepted 202 with the model reported DELETING.
    let rows = [
        ("a hierarchy naming it as the child model", Reference::Hierarchy),
        ("a metric reading through a hierarchy over it", Reference::Formula),
        ("nothing at all", Reference::None),
    ];
    for (case, reference) in rows {
        // Each row stands on its own store, so only that reference exists.
        let router = router();
        let child = create_model(
            &router,
            json!({
                "assetModelName": "Turbine",
                "assetModelProperties": [
                    {"name": "Power", "dataType": "DOUBLE", "type": {"measurement": {}}},
                ],
            }),
        )
        .await;
        let (status, described) =
            call(&router, "GET", &format!("/asset-models/{child}"), None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: describing the child model got {status} {described}"
        );
        // The reference names the child's property by its ID: the NAME spelling
        // is documented for a property of the same asset model only.
        let power = described["assetModelProperties"][0]["id"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no property id in {described}"))
            .to_string();

        let referrer = match reference {
            Reference::Hierarchy => Some(json!({
                "assetModelName": "Farm",
                "assetModelHierarchies": [
                    {"name": "turbines", "childAssetModelId": child},
                ],
            })),
            Reference::Formula => Some(json!({
                "assetModelName": "Rollup",
                "assetModelProperties": [{
                    "name": "TotalPower",
                    "dataType": "DOUBLE",
                    "type": {"metric": {
                        "expression": "sum(v)",
                        "variables": [{
                            "name": "v",
                            "value": {"propertyId": power, "hierarchyId": "turbines"},
                        }],
                        "window": {"tumbling": {"interval": "5m"}},
                    }},
                }],
                "assetModelHierarchies": [
                    {"name": "turbines", "childAssetModelId": child},
                ],
            })),
            Reference::None => None,
        };
        if let Some(body) = referrer {
            let (status, created) = call(&router, "POST", "/asset-models", Some(body)).await;
            assert_eq!(
                status, 202,
                "case {case:?}: creating the referring model got {status} {created}"
            );
        }

        let (status, deleted) =
            call(&router, "DELETE", &format!("/asset-models/{child}"), None).await;
        assert_eq!(
            status, 202,
            "case {case:?}: the delete must be accepted, got {status} {deleted}"
        );
        assert_eq!(
            deleted["assetModelId"].as_str(),
            Some(child.as_str()),
            "case {case:?}: the accepted delete names the model, got {deleted}"
        );
        assert_eq!(
            deleted["assetModelStatus"]["state"].as_str(),
            Some("DELETING"),
            "case {case:?}: the accepted delete reports DELETING, got {deleted}"
        );
    }

    // And the reference that DOES hold a delete back, so the rule is about the
    // assets created from the model rather than about references in general.
    let router = router();
    let child = create_model(&router, json!({"assetModelName": "Turbine"})).await;
    let instance = create_asset(
        &router,
        json!({"assetName": "Turbine 1", "assetModelId": child}),
    )
    .await;
    let (status, refused) = call(&router, "DELETE", &format!("/asset-models/{child}"), None).await;
    assert_eq!(
        status, 400,
        "a model with an asset must be refused, got {status} {refused}"
    );
    assert_eq!(
        error_code(&refused),
        "InvalidRequestException",
        "a model with an asset: unexpected error code in {refused}"
    );
    // With the asset gone the same delete is accepted.
    let (status, removed) = call(&router, "DELETE", &format!("/assets/{instance}"), None).await;
    assert_eq!(status, 202, "deleting the instance: got {status} {removed}");
    let (status, deleted) = call(&router, "DELETE", &format!("/asset-models/{child}"), None).await;
    assert_eq!(
        status, 202,
        "with its only asset gone the model deletes: got {status} {deleted}"
    );
}
