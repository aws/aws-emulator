// SPDX-License-Identifier: CC0-1.0

//! Which component type an entity's component instantiates, end to end over
//! HTTP.
//!
//! RULE under test: a component of an entity IS an instance of a component type,
//! so every write that brings a component into being NAMES the
//! `componentTypeId` it instantiates — one that names none is refused with the
//! modelled `ValidationException` (HTTP 400) and nothing is persisted — and
//! every reader reports the id that write named, within the bounds the model
//! declares for the member it is reported on:
//! `smithy_query(query="shape", shape="ComponentSummary")` marks
//! `componentTypeId` `@required` with `@length(min: 1, max: 256)` and
//! `@pattern(^[a-zA-Z_\.\-0-9:]+$)`, which no component that named none could be
//! reported on. An entry that PATCHES a component the entity already holds may
//! leave the member out: the component keeps the id it has.
//!
//! The rows differ along what the rule generalises over: WHICH writer brings the
//! component into being (`CreateEntity`'s `components`, `UpdateEntity`'s
//! `componentUpdates` with an explicit `CREATE` and with the create its absent
//! `updateType` makes it), HOW MANY components the request carries and where
//! the one naming no type sits among legal ones, and — on the accepted side —
//! the FORM of the id named (a type the workspace holds, one that extends
//! another, a built-in the service ships, one the workspace holds nothing
//! under, and the shortest and longest the member admits).

use serde_json::{json, Map, Value};

use crate::harness::{Emulator, Resp};

/// The workspace every case below works in.
const WORKSPACE: &str = "instances";

/// The component type the legal components instantiate.
const SENSOR: &str = "com.example.sensor";
/// A component type that extends [`SENSOR`], so an instance of it inherits
/// through a chain.
const TUNED: &str = "com.example.tuned";
/// A component type the service ships that is not abstract, so a component may
/// instantiate it.
const BUILT_IN: &str = "com.amazon.iottwinmaker.documents";
/// A `componentTypeId` this workspace holds no component type under. Such a
/// component is stored: a real workspace carries more built-in component types
/// than this emulator models, so an id it does not hold is not refused.
const UNHELD: &str = "com.example.unheld";

/// The component name every single-component case uses.
const COMPONENT: &str = "Spec";

/// The characters the modelled `ComponentTypeId` pattern admits
/// (`^[a-zA-Z_\.\-0-9:]+$`, from
/// `smithy_query(query="shape", shape="ComponentSummary")`), so a generated id
/// of them is one the member accepts.
const PATTERN_CHARACTERS: &str = "abcXYZ_.-09:";

/// The greatest number of characters `ComponentSummary::componentTypeId` admits
/// (`@length(min: 1, max: 256)`, same source), and the least.
const MAX_ID_CHARS: usize = 256;
const MIN_ID_CHARS: usize = 1;

/// A `componentTypeId` of exactly `characters` characters, drawn from the
/// characters the member's pattern admits.
fn id_of_length(characters: usize) -> String {
    PATTERN_CHARACTERS
        .chars()
        .cycle()
        .take(characters)
        .collect()
}

/// Whether `component_type_id` is inside the bounds the modelled
/// `ComponentSummary::componentTypeId` declares: `@length(min: 1, max: 256)` and
/// `@pattern(^[a-zA-Z_\.\-0-9:]+$)`.
fn within_modelled_bounds(component_type_id: &str) -> bool {
    let characters = component_type_id.chars().count();
    (MIN_ID_CHARS..=MAX_ID_CHARS).contains(&characters)
        && component_type_id
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || "_.-:".contains(character))
}

/// The workspace and the component types the cases instantiate.
async fn seed(emulator: &Emulator) {
    let created = emulator.create_workspace(WORKSPACE).await;
    assert_eq!(
        created.status, 200,
        "CreateWorkspace: expected 200, got {created:?}"
    );
    // In reference order: `extendsFrom` may only name a component type the
    // workspace already holds.
    for (component_type_id, body) in [
        (
            SENSOR,
            json!({"propertyDefinitions": {"Serial": {"dataType": {"type": "STRING"}}}}),
        ),
        (TUNED, json!({"extendsFrom": [SENSOR]})),
    ] {
        let created = emulator
            .create_component_type(WORKSPACE, component_type_id, body)
            .await;
        assert_eq!(
            created.status, 200,
            "CreateComponentType {component_type_id:?}: expected 200, got {created:?}"
        );
    }
}

/// A JSON object built from `entries`, for a request whose map is generated
/// rather than written out.
fn object(entries: impl IntoIterator<Item = (String, Value)>) -> Value {
    Value::Object(entries.into_iter().collect::<Map<String, Value>>())
}

/// A component naming no `componentTypeId` — the member left OUT of the request
/// rather than sent as null.
fn naming_no_type() -> Value {
    json!({"description": "names no component type"})
}

/// How a refused row asks for the component.
#[derive(Debug)]
enum Writer {
    /// `CreateEntity`, carrying the row's `components` map.
    Create(Value),
    /// `UpdateEntity`, carrying the row's `componentUpdates` map, applied to an
    /// entity the row stands up first with one legal component.
    Update(Value),
}

/// One refused row: what it is about, the writer it drives, and the request
/// member the refusal must name.
#[derive(Debug)]
struct Refused {
    case: &'static str,
    writer: Writer,
    member: &'static str,
}

/// Every refused row.
fn refused() -> Vec<Refused> {
    let twelve: Vec<String> = (0..12).map(|index| format!("comp{index:02}")).collect();
    vec![
        Refused {
            // The boundary: one component, naming no type.
            case: "oneComponent",
            writer: Writer::Create(json!({COMPONENT: naming_no_type()})),
            member: "components.Spec",
        },
        Refused {
            // The offender sits among components that are perfectly legal, and
            // the walk is in component-name order, so the refusal names the
            // first offender in the request's own key order.
            case: "amongLegalComponents",
            writer: Writer::Create(json!({
                "Aaa": {"componentTypeId": SENSOR},
                "Zzz": naming_no_type(),
                "Mmm": naming_no_type(),
            })),
            member: "components.Mmm",
        },
        Refused {
            // Twelve components, generated rather than written out, with the
            // first in key order naming no type.
            case: "twelveComponents",
            writer: Writer::Create(object(twelve.iter().enumerate().map(|(index, name)| {
                let component = if index == 0 {
                    naming_no_type()
                } else {
                    json!({"componentTypeId": SENSOR})
                };
                (name.clone(), component)
            }))),
            member: "components.comp00",
        },
        Refused {
            // The other writer, asked explicitly to create a component.
            case: "updateTypeCreate",
            writer: Writer::Update(json!({
                "Fresh": {"updateType": "CREATE", "description": "no type"},
            })),
            member: "componentUpdates.Fresh",
        },
        Refused {
            // The same writer with NO `updateType`: an entry naming a component
            // the entity does not have is the create its absence makes it.
            case: "updateTypeAbsent",
            writer: Writer::Update(json!({"Fresh": {"description": "no type"}})),
            member: "componentUpdates.Fresh",
        },
    ]
}

/// The components `GetEntity` reports for `entity_id`.
async fn reported_components(emulator: &Emulator, entity_id: &str) -> Value {
    let read = emulator.get_entity(WORKSPACE, entity_id).await;
    assert_eq!(
        read.status, 200,
        "GetEntity {entity_id:?}: expected 200, got {read:?}"
    );
    read.member("components").clone()
}

/// The `componentSummaries` of one `ListComponents` page.
fn summaries(response: &Resp) -> Vec<Value> {
    response
        .member("componentSummaries")
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// RULE (refused half): a write that would bring a component into being naming
/// no `componentTypeId` is a 400 `ValidationException` naming that member, and
/// nothing it asked for is persisted.
#[tokio::test]
async fn a_component_naming_no_component_type_is_refused() {
    let emulator = Emulator::new();
    seed(&emulator).await;

    for row in refused() {
        let entity_id = format!("entity-{}", row.case);
        let refused = match &row.writer {
            Writer::Create(components) => {
                emulator
                    .create_entity(
                        WORKSPACE,
                        json!({
                            "entityId": entity_id,
                            "entityName": row.case,
                            "components": components.clone(),
                        }),
                    )
                    .await
            }
            Writer::Update(component_updates) => {
                // The entity the patch is applied to, with one legal component
                // so the refusal cannot be about an entity that is not there.
                let created = emulator
                    .create_entity(
                        WORKSPACE,
                        json!({
                            "entityId": entity_id,
                            "entityName": row.case,
                            "components": {COMPONENT: {"componentTypeId": SENSOR}},
                        }),
                    )
                    .await;
                assert_eq!(
                    created.status, 200,
                    "case {:?}: expected 200 from CreateEntity, got {created:?}",
                    row.case
                );
                emulator
                    .update_entity(
                        WORKSPACE,
                        &entity_id,
                        json!({"componentUpdates": component_updates.clone()}),
                    )
                    .await
            }
        };

        assert_eq!(
            (refused.status, refused.error_type.as_deref()),
            (400, Some("ValidationException")),
            "case {:?}: expected the modelled refusal, got {refused:?}",
            row.case
        );
        let message = refused.member("message").as_str().unwrap_or_default();
        assert_eq!(
            message,
            format!("{}.componentTypeId is required", row.member),
            "case {:?}: the refusal must name the member the component hangs off, got {message:?}",
            row.case
        );

        match &row.writer {
            // A refused create persists nothing at all.
            Writer::Create(_) => {
                let read = emulator.get_entity(WORKSPACE, &entity_id).await;
                assert_eq!(
                    (read.status, read.error_type.as_deref()),
                    (404, Some("ResourceNotFoundException")),
                    "case {:?}: a refused create must persist no entity, got {read:?}",
                    row.case
                );
            }
            // A refused patch leaves the entity exactly as its create left it.
            Writer::Update(_) => {
                let components = reported_components(&emulator, &entity_id).await;
                assert!(
                    components["Fresh"].is_null(),
                    "case {:?}: a refused patch must persist no component, got {components:?}",
                    row.case
                );
                assert_eq!(
                    components[COMPONENT]["componentTypeId"].as_str(),
                    Some(SENSOR),
                    "case {:?}: the component the create stored must be untouched, got \
                     {components:?}",
                    row.case
                );
            }
        }
    }
}

/// RULE (accepted half, and what a reader reports): a component created naming a
/// `componentTypeId` is stored instantiating it, and BOTH readers of a component
/// report that id — within the bounds the modelled member declares — whatever
/// form it takes.
#[tokio::test]
async fn every_stored_component_reports_the_component_type_it_instantiates() {
    let emulator = Emulator::new();
    seed(&emulator).await;

    // The rows span the form of the id: held, inherited-through, shipped by the
    // service, held by nothing in this workspace, and the two ends of the
    // member's `@length(1..256)`.
    let shortest = id_of_length(MIN_ID_CHARS);
    let longest = id_of_length(MAX_ID_CHARS);
    let rows: Vec<(&str, &str)> = vec![
        ("aTypeTheWorkspaceHolds", SENSOR),
        ("aTypeThatExtendsAnother", TUNED),
        ("aTypeTheServiceShips", BUILT_IN),
        ("aTypeTheWorkspaceDoesNotHold", UNHELD),
        ("theShortestIdTheMemberAdmits", shortest.as_str()),
        ("theLongestIdTheMemberAdmits", longest.as_str()),
    ];

    for (case, component_type_id) in rows {
        assert!(
            within_modelled_bounds(component_type_id),
            "case {case:?}: the row's own id must be one the member admits, got \
             {component_type_id:?}"
        );
        let entity_id = format!("entity-{case}");
        let created = emulator
            .create_entity(
                WORKSPACE,
                json!({
                    "entityId": entity_id,
                    "entityName": case,
                    "components": {COMPONENT: {"componentTypeId": component_type_id}},
                }),
            )
            .await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected 200 from CreateEntity, got {created:?}"
        );

        // Read back through the whole-entity reader ...
        let components = reported_components(&emulator, &entity_id).await;
        assert_eq!(
            components[COMPONENT]["componentTypeId"].as_str(),
            Some(component_type_id),
            "case {case:?}: GetEntity must report the created componentTypeId, got {components:?}"
        );

        // ... and through the listing, whose summary is REQUIRED to carry it.
        let listed = emulator
            .list_components(WORKSPACE, &entity_id, json!({}))
            .await;
        assert_eq!(
            listed.status, 200,
            "case {case:?}: expected 200 from ListComponents, got {listed:?}"
        );
        let summary = summaries(&listed)
            .into_iter()
            .find(|summary| summary["componentName"] == COMPONENT)
            .unwrap_or_else(|| panic!("case {case:?}: expected the component's row, got {listed:?}"));
        let reported = summary["componentTypeId"].as_str().unwrap_or_default();
        assert_eq!(
            reported, component_type_id,
            "case {case:?}: ListComponents must report the created componentTypeId, got {summary:?}"
        );
        assert!(
            within_modelled_bounds(reported),
            "case {case:?}: every reported componentTypeId must satisfy the member's \
             length and pattern, got {reported:?}"
        );
    }
}

/// RULE (the patch half): an entry that patches a component the entity already
/// holds may leave `componentTypeId` out — the component keeps the id it has —
/// while one that names an id re-points the component at it, and every row of a
/// listing still reports an id the member admits.
#[tokio::test]
async fn a_patch_keeps_or_repoints_the_component_type_it_finds() {
    let emulator = Emulator::new();
    seed(&emulator).await;

    let entity_id = "patched";
    let created = emulator
        .create_entity(
            WORKSPACE,
            json!({
                "entityId": entity_id,
                "entityName": entity_id,
                "components": {COMPONENT: {"componentTypeId": SENSOR}},
            }),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "expected 200 from CreateEntity, got {created:?}"
    );

    // Each row: what it patches, and the id the component must instantiate
    // afterwards.
    let rows: Vec<(&str, Value, &str)> = vec![
        (
            "aDescriptionOnly",
            json!({COMPONENT: {"description": "patched"}}),
            SENSOR,
        ),
        (
            "anExplicitUpdateWithNoType",
            json!({COMPONENT: {"updateType": "UPDATE", "description": "patched again"}}),
            SENSOR,
        ),
        (
            "aPropertyOnly",
            json!({COMPONENT: {"propertyUpdates": {
                "Serial": {"value": {"stringValue": "SN-1"}},
            }}}),
            SENSOR,
        ),
        (
            "aRepointedType",
            json!({COMPONENT: {"componentTypeId": TUNED}}),
            TUNED,
        ),
        (
            "aRepointToATypeTheWorkspaceDoesNotHold",
            json!({COMPONENT: {"componentTypeId": UNHELD}}),
            UNHELD,
        ),
    ];

    for (case, component_updates, expected) in rows {
        let patched = emulator
            .update_entity(
                WORKSPACE,
                entity_id,
                json!({"componentUpdates": component_updates}),
            )
            .await;
        assert_eq!(
            patched.status, 200,
            "case {case:?}: expected 200 from UpdateEntity, got {patched:?}"
        );

        let components = reported_components(&emulator, entity_id).await;
        assert_eq!(
            components[COMPONENT]["componentTypeId"].as_str(),
            Some(expected),
            "case {case:?}: GetEntity must report the id the component instantiates, got \
             {components:?}"
        );

        let listed = emulator
            .list_components(WORKSPACE, entity_id, json!({}))
            .await;
        assert_eq!(
            listed.status, 200,
            "case {case:?}: expected 200 from ListComponents, got {listed:?}"
        );
        for summary in summaries(&listed) {
            let reported = summary["componentTypeId"].as_str().unwrap_or_default();
            assert!(
                within_modelled_bounds(reported),
                "case {case:?}: every row of a listing must report an id the member admits, \
                 got {summary:?}"
            );
        }
        let summary = summaries(&listed)
            .into_iter()
            .find(|summary| summary["componentName"] == COMPONENT)
            .unwrap_or_else(|| panic!("case {case:?}: expected the component's row, got {listed:?}"));
        assert_eq!(
            summary["componentTypeId"].as_str(),
            Some(expected),
            "case {case:?}: ListComponents must report the id the component instantiates, got \
             {summary:?}"
        );
    }
}
