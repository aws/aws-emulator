// SPDX-License-Identifier: CC0-1.0

//! `ListProperties`, end to end over HTTP.
//!
//! RULE under test: `ListProperties` reports the properties of ONE component —
//! the ones the component stores itself TOGETHER WITH the ones the component
//! type it instantiates, or a type that type extends, defines and it never
//! restated — a page at a time in property-name order, each row carrying the
//! definition that gives the property its meaning (the component's own, else
//! the inherited one) and the value it reads as (the value written to it, else
//! its definition's default, else nothing). The component is addressed by
//! `componentPath` or, for a top-level one, by `componentName`; `maxResults`
//! caps the page and the opaque `nextToken` resumes at the next name. A scope
//! this service holds nothing at is a 404 `ResourceNotFoundException` and a
//! request addressing no component is a 400 `ValidationException`, while a
//! component with no property is a page of no rows.
//!
//! The rows differ along what the rule generalises over: WHERE a property comes
//! from (stored on the component, defined by its type, defined by a type that
//! type extends), WHERE its value comes from (written, defaulted, nowhere),
//! the DATA TYPE it travels in (`STRING`, `DOUBLE`, `BOOLEAN`, `INTEGER`,
//! `LIST`), HOW the component is addressed (by name, by a one-segment path, by
//! a composite path, by both members at once), how MANY properties it has, and
//! the page SIZE asked for.

use serde_json::{json, Value};

use crate::harness::{Emulator, Resp, REGION};

/// The workspace every case below reads from.
const WORKSPACE: &str = "properties";

/// The component type whose definitions a component INHERITS through
/// `extendsFrom`: one carrying a default, one carrying none.
const BASE: &str = "com.example.base";
/// A component type that extends [`BASE`] and declares a composite component
/// type of it, so an entity component of this type carries both inherited
/// definitions and a composite component addressed by `Spec/inner`.
const DERIVED: &str = "com.example.derived";
/// A component type that defines no property at all: the boundary the rule sits
/// on, a component with nothing to list.
const EMPTY: &str = "com.example.empty";
/// A component type defining [`WIDE_PROPERTIES`] properties, so a page size can
/// divide its component's properties evenly, unevenly or not at all.
const WIDE: &str = "com.example.wide";
/// How many properties [`WIDE`] defines.
const WIDE_PROPERTIES: usize = 7;

/// The property names of a [`WIDE`] component, in the order a page reports them.
fn wide_property_names() -> Vec<String> {
    (1..=WIDE_PROPERTIES)
        .map(|index| format!("prop{index}"))
        .collect()
}

/// The workspace, component types and entities every case below reads.
async fn seed(emulator: &Emulator) {
    let created = emulator.create_workspace(WORKSPACE).await;
    assert_eq!(
        created.status, 200,
        "CreateWorkspace: expected 200, got {created:?}"
    );

    let wide_definitions: serde_json::Map<String, Value> = wide_property_names()
        .into_iter()
        .map(|property_name| (property_name, json!({"dataType": {"type": "INTEGER"}})))
        .collect();

    let component_types: Vec<(&str, Value)> = vec![
        (
            BASE,
            json!({
                "propertyDefinitions": {
                    // A default, so an inherited property nothing wrote to still
                    // reads as a value.
                    "Tag": {
                        "dataType": {"type": "STRING"},
                        "defaultValue": {"stringValue": "unset-tag"},
                    },
                    // No default, so the property is listed with no value.
                    "Serial": {"dataType": {"type": "STRING"}, "isExternalId": true},
                },
            }),
        ),
        (
            DERIVED,
            json!({
                "extendsFrom": [BASE],
                "propertyDefinitions": {
                    "Pressure": {
                        "dataType": {"type": "DOUBLE"},
                        "defaultValue": {"doubleValue": 1.5},
                    },
                    // A container data type takes a `nestedType`.
                    "Readings": {
                        "dataType": {"type": "LIST", "nestedType": {"type": "STRING"}},
                    },
                },
                "compositeComponentTypes": {"inner": {"componentTypeId": BASE}},
            }),
        ),
        (EMPTY, json!({})),
        (WIDE, json!({"propertyDefinitions": wide_definitions})),
    ];
    for (component_type_id, body) in component_types {
        let created = emulator
            .create_component_type(WORKSPACE, component_type_id, body)
            .await;
        assert_eq!(
            created.status, 200,
            "CreateComponentType {component_type_id:?}: expected 200, got {created:?}"
        );
    }

    let entities: Vec<(&str, Value)> = vec![
        (
            "unit-1",
            json!({
                "entityId": "unit-1",
                "entityName": "unit-1",
                "components": {
                    // Writes a value over an inherited default, adds a property
                    // of its own, and leaves two inherited ones untouched.
                    "Spec": {
                        "componentTypeId": DERIVED,
                        "properties": {
                            "Pressure": {"value": {"doubleValue": 3.5}},
                            "Local": {
                                "definition": {"dataType": {"type": "STRING"}},
                                "value": {"stringValue": "only here"},
                            },
                            "Readings": {"value": {"listValue": [
                                {"stringValue": "a"},
                                {"stringValue": "b"},
                            ]}},
                        },
                    },
                    // A component of a type that defines no property inherits
                    // nothing, so every property it has is one it stores
                    // itself.
                    "Plain": {
                        "componentTypeId": EMPTY,
                        "properties": {
                            "Solo": {
                                "definition": {"dataType": {"type": "BOOLEAN"}},
                                "value": {"booleanValue": true},
                            },
                            // A value with no definition anywhere: still a
                            // property of the component.
                            "Loose": {"value": {"stringValue": "no definition"}},
                        },
                    },
                    "Blank": {"componentTypeId": EMPTY},
                },
                "compositeComponents": {
                    "Spec/inner": {"properties": {"Tag": {"value": {"stringValue": "deep"}}}},
                },
            }),
        ),
        (
            "unit-2",
            json!({
                "entityId": "unit-2",
                "entityName": "unit-2",
                "components": {"Wide": {"componentTypeId": WIDE}},
            }),
        ),
    ];
    for (entity_id, body) in entities {
        let created = emulator.create_entity(WORKSPACE, body).await;
        assert_eq!(
            created.status, 200,
            "CreateEntity {entity_id:?}: expected 200, got {created:?}"
        );
    }
}

/// The `propertySummaries` of one response.
fn summaries(response: &Resp) -> Vec<Value> {
    response
        .member("propertySummaries")
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// The `propertyName` of each summary, in the order they were reported.
fn names(response: &Resp) -> Vec<String> {
    summaries(response)
        .iter()
        .map(|summary| {
            summary["propertyName"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// One property a page must report.
#[derive(Debug)]
struct Expected {
    name: &'static str,
    /// The `type` of the `dataType` the reported definition declares, or `None`
    /// where no definition reaches the property at all.
    data_type: Option<&'static str>,
    /// The `DataValue` member the reported value travels in and the value
    /// itself, or `None` where the property reads as nothing.
    value: Option<(&'static str, Value)>,
}

/// RULE: a page reports every property the addressed component has — stored on
/// it or defined by the component type it instantiates or a type that type
/// extends — in name order, each with its effective definition and the value it
/// reads as.
#[tokio::test]
async fn a_page_reports_every_property_the_component_has() {
    let emulator = Emulator::new();
    seed(&emulator).await;

    // The properties of `Spec`: two written, one local, two inherited from the
    // type chain. Shared by the rows that address that component three ways.
    let spec = || {
        vec![
            Expected {
                name: "Local",
                data_type: Some("STRING"),
                value: Some(("stringValue", json!("only here"))),
            },
            Expected {
                name: "Pressure",
                data_type: Some("DOUBLE"),
                value: Some(("doubleValue", json!(3.5))),
            },
            Expected {
                name: "Readings",
                data_type: Some("LIST"),
                value: Some((
                    "listValue",
                    json!([{"stringValue": "a"}, {"stringValue": "b"}]),
                )),
            },
            Expected {
                name: "Serial",
                data_type: Some("STRING"),
                value: None,
            },
            Expected {
                name: "Tag",
                data_type: Some("STRING"),
                value: Some(("stringValue", json!("unset-tag"))),
            },
        ]
    };
    // The properties of the composite component `Spec/inner`: one written, one
    // inherited from the type its declaration names.
    let inner = || {
        vec![
            Expected {
                name: "Serial",
                data_type: Some("STRING"),
                value: None,
            },
            Expected {
                name: "Tag",
                data_type: Some("STRING"),
                value: Some(("stringValue", json!("deep"))),
            },
        ]
    };

    let rows: Vec<(&str, &str, Value, Vec<Expected>)> = vec![
        (
            "aComponentAddressedByName",
            "unit-1",
            json!({"componentName": "Spec"}),
            spec(),
        ),
        (
            // A path of one segment names the top-level component itself.
            "aComponentAddressedByAOneSegmentPath",
            "unit-1",
            json!({"componentPath": "Spec"}),
            spec(),
        ),
        (
            "aCompositeComponentAddressedByItsPath",
            "unit-1",
            json!({"componentPath": "Spec/inner"}),
            inner(),
        ),
        (
            // Both members: the path is the more specific address, so it wins.
            "aPathWinsOverAName",
            "unit-1",
            json!({"componentName": "Plain", "componentPath": "Spec/inner"}),
            inner(),
        ),
        (
            // A component that inherits nothing: only what it stores, including
            // a property no definition reaches.
            "aComponentThatInstantiatesNoType",
            "unit-1",
            json!({"componentName": "Plain"}),
            vec![
                Expected {
                    name: "Loose",
                    data_type: None,
                    value: Some(("stringValue", json!("no definition"))),
                },
                Expected {
                    name: "Solo",
                    data_type: Some("BOOLEAN"),
                    value: Some(("booleanValue", json!(true))),
                },
            ],
        ),
        (
            // Nothing stored and nothing defined: a page of no rows.
            "aComponentWithNoPropertyAtAll",
            "unit-1",
            json!({"componentName": "Blank"}),
            vec![],
        ),
        (
            "aComponentWhoseTypeDefinesEveryProperty",
            "unit-2",
            json!({"componentName": "Wide"}),
            vec![
                Expected {
                    name: "prop1",
                    data_type: Some("INTEGER"),
                    value: None,
                },
                Expected {
                    name: "prop2",
                    data_type: Some("INTEGER"),
                    value: None,
                },
                Expected {
                    name: "prop3",
                    data_type: Some("INTEGER"),
                    value: None,
                },
                Expected {
                    name: "prop4",
                    data_type: Some("INTEGER"),
                    value: None,
                },
                Expected {
                    name: "prop5",
                    data_type: Some("INTEGER"),
                    value: None,
                },
                Expected {
                    name: "prop6",
                    data_type: Some("INTEGER"),
                    value: None,
                },
                Expected {
                    name: "prop7",
                    data_type: Some("INTEGER"),
                    value: None,
                },
            ],
        ),
    ];

    for (case, entity_id, address, expected) in rows {
        let mut request = address;
        request["entityId"] = json!(entity_id);
        let listed = emulator.list_properties(WORKSPACE, request).await;
        assert_eq!(
            listed.status, 200,
            "case {case:?}: expected 200 from ListProperties, got {listed:?}"
        );
        assert_eq!(
            names(&listed),
            expected.iter().map(|want| want.name).collect::<Vec<_>>(),
            "case {case:?}: expected exactly these properties in name order, got {listed:?}"
        );
        assert!(
            listed.member("nextToken").is_null(),
            "case {case:?}: one page holds every property here, so no token is minted, got \
             {listed:?}"
        );

        for (summary, want) in summaries(&listed).iter().zip(&expected) {
            assert_eq!(
                summary["definition"]["dataType"]["type"].as_str(),
                want.data_type,
                "case {case:?} {}: wrong definition, got {summary:?}",
                want.name
            );
            match &want.value {
                Some((member, value)) => assert_eq!(
                    summary["value"][member],
                    *value,
                    "case {case:?} {}: expected value.{member} to be {value}, got {summary:?}",
                    want.name
                ),
                None => assert!(
                    summary["value"].is_null(),
                    "case {case:?} {}: expected no value, got {summary:?}",
                    want.name
                ),
            }
            // Every value this service can store fits the maximum the flag
            // documents (50), since that is also the top of the modelled length
            // of a `listValue` and a `mapValue`.
            assert_eq!(
                summary["areAllPropertyValuesReturned"].as_bool(),
                Some(true),
                "case {case:?} {}: expected every value to be reported, got {summary:?}",
                want.name
            );
        }
    }
}

/// RULE (one property, one answer): the definition a row carries is the one the
/// component type reports for it through `GetComponentType`, and the value is
/// the one `GetPropertyValue` reads for the same property.
#[tokio::test]
async fn a_row_agrees_with_the_other_readers_of_the_property() {
    let emulator = Emulator::new();
    seed(&emulator).await;

    let listed = emulator
        .list_properties(WORKSPACE, json!({"entityId": "unit-1", "componentName": "Spec"}))
        .await;
    assert_eq!(
        listed.status, 200,
        "expected 200 from ListProperties, got {listed:?}"
    );
    let rows = summaries(&listed);

    // The definition of an INHERITED property is the component type's own —
    // whichever type on the chain declared it — except for `isInherited`, which
    // describes the READER's relationship to the definition: a component that
    // never declared the property reports it inherited, while the component type
    // that declared it reports its own definition.
    let definitions: Vec<(&str, &str)> = vec![("Tag", BASE), ("Serial", BASE), ("Pressure", DERIVED)];
    for (property_name, component_type_id) in definitions {
        let read = emulator
            .get_component_type(WORKSPACE, component_type_id)
            .await;
        assert_eq!(
            read.status, 200,
            "case {property_name:?}: expected 200 from GetComponentType, got {read:?}"
        );
        let row = rows
            .iter()
            .find(|row| row["propertyName"] == property_name)
            .unwrap_or_else(|| panic!("case {property_name:?}: expected the row, got {listed:?}"));
        let mut listed_definition = row["definition"].clone();
        let mut declared_definition = read.member("propertyDefinitions")[property_name].clone();
        assert_eq!(
            (
                listed_definition["isInherited"].as_bool(),
                declared_definition["isInherited"].as_bool(),
            ),
            (Some(true), Some(false)),
            "case {property_name:?}: the component inherits this definition and the component \
             type declares it, got {row:?} and {declared_definition:?}"
        );
        listed_definition["isInherited"] = Value::Null;
        declared_definition["isInherited"] = Value::Null;
        assert_eq!(
            listed_definition, declared_definition,
            "case {property_name:?}: expected one definition to be reported alike by \
             ListProperties and GetComponentType, got {row:?}"
        );
    }

    // The value of EVERY listed property is the one GetPropertyValue reads for
    // it, whether it was written, defaulted or neither.
    for row in &rows {
        let property_name = row["propertyName"].as_str().expect("a reported name");
        let read = emulator
            .get_property_value(
                WORKSPACE,
                json!({
                    "entityId": "unit-1",
                    "componentName": "Spec",
                    "selectedProperties": [property_name],
                }),
            )
            .await;
        assert_eq!(
            read.status, 200,
            "case {property_name:?}: expected 200 from GetPropertyValue, got {read:?}"
        );
        assert_eq!(
            row["value"], read.member("propertyValues")[property_name]["propertyValue"],
            "case {property_name:?}: expected one value to be reported alike by ListProperties \
             and GetPropertyValue, got {row:?}"
        );
    }
}

/// RULE (the refusals): a scope this service holds nothing at is a 404
/// `ResourceNotFoundException`, and a request addressing no component at all is
/// a 400 `ValidationException`.
#[tokio::test]
async fn a_scope_that_holds_nothing_is_refused() {
    let emulator = Emulator::new();
    seed(&emulator).await;

    let rows: Vec<(&str, &str, Value, u16, &str)> = vec![
        (
            "unknownWorkspace",
            "absent",
            json!({"entityId": "unit-1", "componentName": "Spec"}),
            404,
            "ResourceNotFoundException",
        ),
        (
            "unknownEntity",
            WORKSPACE,
            json!({"entityId": "absent", "componentName": "Spec"}),
            404,
            "ResourceNotFoundException",
        ),
        (
            "unknownComponentName",
            WORKSPACE,
            json!({"entityId": "unit-1", "componentName": "Missing"}),
            404,
            "ResourceNotFoundException",
        ),
        (
            "unknownComponentPath",
            WORKSPACE,
            json!({"entityId": "unit-1", "componentPath": "Spec/absent"}),
            404,
            "ResourceNotFoundException",
        ),
        (
            // One segment deeper than anything created.
            "deeperThanCreated",
            WORKSPACE,
            json!({"entityId": "unit-1", "componentPath": "Spec/inner/leaf"}),
            404,
            "ResourceNotFoundException",
        ),
        (
            // The component exists, but belongs to another entity.
            "componentOfAnotherEntity",
            WORKSPACE,
            json!({"entityId": "unit-2", "componentName": "Spec"}),
            404,
            "ResourceNotFoundException",
        ),
        (
            // A summary names no component, so a page is the properties of one
            // component: with neither member nothing is addressed.
            "noComponentAddressed",
            WORKSPACE,
            json!({"entityId": "unit-1"}),
            400,
            "ValidationException",
        ),
    ];

    for (case, workspace_id, request, status, error_type) in rows {
        let refused = emulator.list_properties(workspace_id, request).await;
        assert_eq!(
            refused.status, status,
            "case {case:?}: expected {status} from ListProperties, got {refused:?}"
        );
        assert_eq!(
            refused.error_type.as_deref(),
            Some(error_type),
            "case {case:?}: expected a {error_type}, got {refused:?}"
        );
    }
}

/// RULE (the paging): whatever page size is asked for, following `nextToken`
/// walks the component's properties exactly once, in name order, and the token
/// is absent on the last page.
#[tokio::test]
async fn every_page_size_walks_the_properties_exactly_once() {
    let emulator = Emulator::new();
    seed(&emulator).await;

    // Each scope with the whole set it holds, in the name order a page reports.
    let scopes: Vec<(&str, Value, Vec<String>)> = vec![
        ("unit-2", json!({"componentName": "Wide"}), wide_property_names()),
        (
            "unit-1",
            json!({"componentName": "Spec"}),
            ["Local", "Pressure", "Readings", "Serial", "Tag"]
                .iter()
                .map(|name| (*name).to_string())
                .collect(),
        ),
        (
            "unit-1",
            json!({"componentPath": "Spec/inner"}),
            vec!["Serial".to_string(), "Tag".to_string()],
        ),
    ];

    for (entity_id, address, whole) in scopes {
        // 1 row at a time, an uneven divisor, the exact size, one past it, and
        // the largest page the member admits.
        for page_size in [1_u32, 2, 3, 7, 200] {
            let case = format!("{entity_id}:{address}:{page_size}");
            let mut walked: Vec<String> = Vec::new();
            let mut next_token: Option<String> = None;
            let mut pages = 0;
            loop {
                let mut request = address.clone();
                request["entityId"] = json!(entity_id);
                request["maxResults"] = json!(page_size);
                if let Some(token) = &next_token {
                    request["nextToken"] = json!(token);
                }
                let page = emulator.list_properties(WORKSPACE, request).await;
                assert_eq!(
                    page.status, 200,
                    "case {case:?}: expected 200 from ListProperties, got {page:?}"
                );
                let reported = names(&page);
                assert!(
                    reported.len() <= page_size as usize,
                    "case {case:?}: a page must not exceed maxResults, got {reported:?}"
                );
                walked.extend(reported);
                next_token = page
                    .member("nextToken")
                    .as_str()
                    .map(std::string::ToString::to_string);
                pages += 1;
                assert!(
                    pages <= whole.len() + 1,
                    "case {case:?}: paging must terminate, walked {walked:?}"
                );
                if next_token.is_none() {
                    break;
                }
            }
            assert_eq!(
                walked, whole,
                "case {case:?}: following nextToken must walk the properties exactly once"
            );
        }

        // A page size of zero: the member publishes no minimum, so it is served
        // as what it says — no rows — and the token names where the next call
        // resumes from, which is the position this one started at.
        let mut request = address.clone();
        request["entityId"] = json!(entity_id);
        request["maxResults"] = json!(0);
        let empty = emulator.list_properties(WORKSPACE, request).await;
        assert_eq!(
            empty.status, 200,
            "case {address}: expected 200 for maxResults 0, got {empty:?}"
        );
        assert!(
            names(&empty).is_empty(),
            "case {address}: expected no rows for maxResults 0, got {empty:?}"
        );
        assert!(
            empty.member("nextToken").is_string(),
            "case {address}: a component with properties left over mints a token, got {empty:?}"
        );
    }
}

/// RULE (the token's scope): a `nextToken` is bound to the component it was
/// minted for, so one replayed against another component is refused with the
/// modelled `ValidationException` rather than silently resumed.
#[tokio::test]
async fn a_token_from_another_component_is_refused() {
    let emulator = Emulator::new();
    seed(&emulator).await;

    let first = emulator
        .list_properties(
            WORKSPACE,
            json!({"entityId": "unit-1", "componentName": "Spec", "maxResults": 1}),
        )
        .await;
    assert_eq!(
        first.status, 200,
        "expected 200 from ListProperties, got {first:?}"
    );
    let token = first
        .member("nextToken")
        .as_str()
        .unwrap_or_else(|| panic!("expected a token for a component of five properties, got {first:?}"))
        .to_string();

    // Each row replays that token somewhere it was not minted for.
    let rows: Vec<(&str, Value)> = vec![
        (
            "anotherComponent",
            json!({"entityId": "unit-1", "componentName": "Plain", "nextToken": token.clone()}),
        ),
        (
            "anotherAddressForTheSameComponent",
            json!({"entityId": "unit-1", "componentPath": "Spec", "nextToken": token.clone()}),
        ),
        (
            "anotherEntity",
            json!({"entityId": "unit-2", "componentName": "Wide", "nextToken": token.clone()}),
        ),
        (
            "notATokenAtAll",
            json!({"entityId": "unit-1", "componentName": "Spec", "nextToken": "not-a-token"}),
        ),
    ];
    for (case, request) in rows {
        let refused = emulator.list_properties(WORKSPACE, request).await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected 400 from ListProperties, got {refused:?}"
        );
        assert_eq!(
            refused.error_type.as_deref(),
            Some("ValidationException"),
            "case {case:?}: expected a ValidationException, got {refused:?}"
        );
    }

    // The same token on the component it WAS minted for resumes there.
    let resumed = emulator
        .list_properties(
            WORKSPACE,
            json!({"entityId": "unit-1", "componentName": "Spec", "nextToken": token}),
        )
        .await;
    assert_eq!(
        resumed.status, 200,
        "expected 200 when the token is replayed on its own component, got {resumed:?}"
    );
    assert_eq!(
        names(&resumed),
        vec![
            "Pressure".to_string(),
            "Readings".to_string(),
            "Serial".to_string(),
            "Tag".to_string(),
        ],
        "expected the properties to resume after the first row, got {resumed:?}"
    );
}

/// RULE (a listing reports state, and changes none): the properties a page
/// reports follow the stored component, and serving a page leaves what a later
/// caller sees untouched.
#[tokio::test]
async fn a_page_follows_stored_state_and_changes_nothing() {
    let emulator = Emulator::new();
    seed(&emulator).await;

    let request = json!({"entityId": "unit-1", "componentName": "Spec"});
    let before = emulator.list_properties(WORKSPACE, request.clone()).await;
    assert_eq!(
        before.status, 200,
        "expected 200 from ListProperties, got {before:?}"
    );
    let again = emulator.list_properties(WORKSPACE, request.clone()).await;
    assert_eq!(
        summaries(&again),
        summaries(&before),
        "expected two reads of one component to report the same rows, got {again:?}"
    );

    // Nothing a later caller sees changed because two pages were served: the
    // stored properties are exactly the ones the create persisted.
    {
        let state = emulator.state();
        let state = state.lock();
        let stored: Vec<&String> = state.region(REGION).workspaces[WORKSPACE].entities["unit-1"]
            .components["Spec"]
            .properties
            .keys()
            .collect();
        assert_eq!(
            stored,
            vec!["Local", "Pressure", "Readings"],
            "expected serving a page to write nothing, got {stored:?}"
        );
    }

    // A property the component GAINS is reported by the next page: the listing
    // follows stored state rather than anything the handler holds.
    let updated = emulator
        .update_entity(
            WORKSPACE,
            "unit-1",
            json!({"componentUpdates": {"Spec": {"propertyUpdates": {"Added": {
                "updateType": "CREATE",
                "definition": {"dataType": {"type": "LONG"}},
                "value": {"longValue": 11},
            }}}}}),
        )
        .await;
    assert_eq!(
        updated.status, 200,
        "expected 200 from UpdateEntity, got {updated:?}"
    );

    let after = emulator.list_properties(WORKSPACE, request).await;
    assert_eq!(
        names(&after),
        vec![
            "Added".to_string(),
            "Local".to_string(),
            "Pressure".to_string(),
            "Readings".to_string(),
            "Serial".to_string(),
            "Tag".to_string(),
        ],
        "expected the added property to be listed in name order, got {after:?}"
    );
    let added = summaries(&after)
        .into_iter()
        .find(|summary| summary["propertyName"] == "Added")
        .unwrap_or_else(|| panic!("expected the added property, got {after:?}"));
    assert_eq!(
        added["definition"]["dataType"]["type"].as_str(),
        Some("LONG"),
        "expected the stored definition, got {added:?}"
    );
    assert_eq!(
        added["value"]["longValue"],
        json!(11),
        "expected the stored value, got {added:?}"
    );
}
