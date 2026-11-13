// SPDX-License-Identifier: CC0-1.0

//! `ListComponents`, end to end over HTTP.
//!
//! RULE under test: `ListComponents` reports ONE level of an entity's component
//! tree, built from the stored component records — the entity's top-level
//! components when the request names no `componentPath`, and the composite
//! components stored directly inside the component that path leads to when it
//! does — a page at a time in component-name order, where `maxResults` caps the
//! page and the opaque `nextToken` resumes at the next name; a scope this
//! service holds nothing at is a 404 `ResourceNotFoundException`, while a
//! component that holds no composite component is a page of no rows.
//!
//! The rows differ along what the rule generalises over: the DEPTH of the
//! listed path, how MANY components the level holds, which optional members each
//! one carries, the page SIZE asked for, and — for the refusals — which step of
//! the scope is missing.

use serde_json::{json, Value};

use crate::harness::{Emulator, Resp, REGION};

/// The workspace every case below is created in.
const WORKSPACE: &str = "components";

/// The component types the entities below instantiate.
///
/// `com.example.sensor` declares three composite component types, one of which
/// (`nested`) instantiates a type that declares one itself, so a path two levels
/// deep resolves; `com.example.plain` declares none.
async fn seed(emulator: &Emulator) {
    let created = emulator.create_workspace(WORKSPACE).await;
    assert_eq!(
        created.status, 200,
        "CreateWorkspace: expected 200, got {created:?}"
    );

    let component_types: Vec<(&str, Value)> = vec![
        (
            "com.example.inner",
            json!({"propertyDefinitions": {"Pressure": {"dataType": {"type": "DOUBLE"}}}}),
        ),
        (
            "com.example.deep",
            json!({"compositeComponentTypes": {"leaf": {"componentTypeId": "com.example.inner"}}}),
        ),
        (
            "com.example.sensor",
            json!({"compositeComponentTypes": {
                "inner": {"componentTypeId": "com.example.inner"},
                "leaf": {"componentTypeId": "com.example.inner"},
                "nested": {"componentTypeId": "com.example.deep"},
            }}),
        ),
        ("com.example.plain", json!({})),
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

    // `tree`: three top-level components, one of which carries composite
    // components at two different depths.
    let entities: Vec<(&str, Value)> = vec![
        (
            "tree",
            json!({
                "entityId": "tree",
                "entityName": "Tree",
                "components": {
                    "Alpha": {
                        "componentTypeId": "com.example.sensor",
                        "description": "the sensor",
                        "properties": {
                            "Pressure": {"definition": {"dataType": {"type": "DOUBLE"}}},
                        },
                        "propertyGroups": {
                            "readings": {"groupType": "TABULAR", "propertyNames": ["Pressure"]},
                        },
                    },
                    "Beta": {"componentTypeId": "com.example.plain"},
                    // A component instantiating a type this workspace does not
                    // hold: it inherits nothing, which is what `definedIn`
                    // reports no component type for.
                    "Gamma": {
                        "componentTypeId": "com.example.absent",
                        "description": "unresolved type",
                    },
                },
                "compositeComponents": {
                    "Alpha/inner": {"description": "one level down"},
                    "Alpha/leaf": {},
                    "Alpha/nested": {},
                    "Alpha/nested/leaf": {"description": "two levels down"},
                },
            }),
        ),
        (
            // Only the DEEP composite component was created: nothing requires
            // the intermediate step of its path to have been created too.
            "sparse",
            json!({
                "entityId": "sparse",
                "entityName": "Sparse",
                "components": {"Sensor": {"componentTypeId": "com.example.sensor"}},
                "compositeComponents": {"Sensor/nested/leaf": {}},
            }),
        ),
        (
            // An entity with NO component at all: the boundary the rule sits on.
            "bare",
            json!({"entityId": "bare", "entityName": "Bare"}),
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

/// The `componentSummaries` of one response.
fn summaries(response: &Resp) -> Vec<Value> {
    response
        .member("componentSummaries")
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// The `componentName` of each summary, in the order they were reported.
fn names(response: &Resp) -> Vec<String> {
    summaries(response)
        .iter()
        .map(|summary| summary["componentName"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// One component a page must report.
#[derive(Debug)]
struct Expected {
    name: &'static str,
    /// The component type the component (or the declaration its path resolved
    /// to) names.
    component_type_id: &'static str,
    /// The whole path a COMPOSITE component was created by; absent for a
    /// top-level component, which its name alone addresses.
    component_path: Option<&'static str>,
    description: Option<&'static str>,
}

/// RULE: a request that names no `componentPath` reports the entity's top-level
/// components, and one that names a path reports the composite components
/// directly inside the component it leads to — each from the stored record, in
/// name order.
#[tokio::test]
async fn a_page_reports_one_level_of_the_component_tree() {
    let emulator = Emulator::new();
    seed(&emulator).await;

    let rows: Vec<(&str, &str, Option<&str>, Vec<Expected>)> = vec![
        (
            // No path: the entity's own top-level components.
            "topLevel",
            "tree",
            None,
            vec![
                Expected {
                    name: "Alpha",
                    component_type_id: "com.example.sensor",
                    component_path: None,
                    description: Some("the sensor"),
                },
                Expected {
                    name: "Beta",
                    component_type_id: "com.example.plain",
                    component_path: None,
                    description: None,
                },
                Expected {
                    name: "Gamma",
                    component_type_id: "com.example.absent",
                    component_path: None,
                    description: Some("unresolved type"),
                },
            ],
        ),
        (
            // One segment: the composite components inside that top-level
            // component, and NOT the one nested a level further down.
            "insideAComponent",
            "tree",
            Some("Alpha"),
            vec![
                Expected {
                    name: "inner",
                    component_type_id: "com.example.inner",
                    component_path: Some("Alpha/inner"),
                    description: Some("one level down"),
                },
                Expected {
                    name: "leaf",
                    component_type_id: "com.example.inner",
                    component_path: Some("Alpha/leaf"),
                    description: None,
                },
                Expected {
                    name: "nested",
                    component_type_id: "com.example.deep",
                    component_path: Some("Alpha/nested"),
                    description: None,
                },
            ],
        ),
        (
            // Two segments: one level deeper again.
            "insideACompositeComponent",
            "tree",
            Some("Alpha/nested"),
            vec![Expected {
                name: "leaf",
                component_type_id: "com.example.inner",
                component_path: Some("Alpha/nested/leaf"),
                description: Some("two levels down"),
            }],
        ),
        (
            // A path whose own composite component was never created, but which
            // a created one sits beneath.
            "ancestorOfACreatedPath",
            "sparse",
            Some("Sensor/nested"),
            vec![Expected {
                name: "leaf",
                component_type_id: "com.example.inner",
                component_path: Some("Sensor/nested/leaf"),
                description: None,
            }],
        ),
        (
            // A component that holds no composite component: a page of no rows,
            // not a refusal.
            "aLevelWithNothingInIt",
            "tree",
            Some("Alpha/inner"),
            vec![],
        ),
        ("aComponentWithNoComposites", "tree", Some("Beta"), vec![]),
        ("anEntityWithNoComponents", "bare", None, vec![]),
    ];

    for (case, entity_id, component_path, expected) in rows {
        let request = match component_path {
            None => json!({}),
            Some(component_path) => json!({"componentPath": component_path}),
        };
        let listed = emulator.list_components(WORKSPACE, entity_id, request).await;
        assert_eq!(
            listed.status, 200,
            "case {case:?}: expected 200 from ListComponents, got {listed:?}"
        );
        assert_eq!(
            names(&listed),
            expected.iter().map(|want| want.name).collect::<Vec<_>>(),
            "case {case:?}: expected exactly this level in name order, got {listed:?}"
        );
        assert!(
            listed.member("nextToken").is_null(),
            "case {case:?}: one page holds the whole level, so no token is minted, got {listed:?}"
        );

        for (summary, want) in summaries(&listed).iter().zip(&expected) {
            assert_eq!(
                summary["componentTypeId"].as_str(),
                Some(want.component_type_id),
                "case {case:?} {}: wrong componentTypeId, got {summary:?}",
                want.name
            );
            assert_eq!(
                summary["componentPath"].as_str(),
                want.component_path,
                "case {case:?} {}: wrong componentPath, got {summary:?}",
                want.name
            );
            assert_eq!(
                summary["description"].as_str(),
                want.description,
                "case {case:?} {}: wrong description, got {summary:?}",
                want.name
            );
            // A component has no lifecycle of its own, so it reports the status
            // of the entity holding it, which settles ACTIVE in its own create.
            assert_eq!(
                summary["status"]["state"].as_str(),
                Some("ACTIVE"),
                "case {case:?} {}: expected the entity's own state, got {summary:?}",
                want.name
            );
        }
    }
}

/// RULE (the reported members): a summary carries the stored property groups of
/// the component it reports, the same way `GetEntity` reports them.
#[tokio::test]
async fn a_summary_reports_the_stored_property_groups() {
    let emulator = Emulator::new();
    seed(&emulator).await;

    let listed = emulator.list_components(WORKSPACE, "tree", json!({})).await;
    assert_eq!(
        listed.status, 200,
        "expected 200 from ListComponents, got {listed:?}"
    );
    let alpha = summaries(&listed)
        .into_iter()
        .find(|summary| summary["componentName"] == "Alpha")
        .unwrap_or_else(|| panic!("expected the Alpha component, got {listed:?}"));
    let group = &alpha["propertyGroups"]["readings"];
    assert_eq!(
        group["groupType"].as_str(),
        Some("TABULAR"),
        "expected the created groupType, got {alpha:?}"
    );
    assert_eq!(
        group["propertyNames"].as_array().cloned().unwrap_or_default(),
        vec![json!("Pressure")],
        "expected the created propertyNames, got {alpha:?}"
    );
    assert_eq!(
        group["isInherited"].as_bool(),
        Some(false),
        "nothing is inherited here, got {alpha:?}"
    );

    // The same component, read through `GetEntity`: one component says the same
    // thing wherever it is reported.
    let read = emulator.get_entity(WORKSPACE, "tree").await;
    assert_eq!(read.status, 200, "expected 200 from GetEntity, got {read:?}");
    let component = &read.member("components")["Alpha"];
    assert_eq!(
        component["propertyGroups"]["readings"], *group,
        "expected ListComponents and GetEntity to report one property group alike, got \
         {component:?}"
    );
    assert_eq!(
        component["componentTypeId"], alpha["componentTypeId"],
        "expected ListComponents and GetEntity to report one componentTypeId alike, got \
         {component:?}"
    );
    assert_eq!(
        component["status"], alpha["status"],
        "expected ListComponents and GetEntity to report one status alike, got {component:?}"
    );
}

/// RULE (the refusal): a scope this service holds nothing at is a 404
/// `ResourceNotFoundException`, and the rows differ along which step of the
/// scope — workspace, entity or component path — is missing.
#[tokio::test]
async fn a_scope_that_holds_nothing_is_not_found() {
    let emulator = Emulator::new();
    seed(&emulator).await;

    let rows: Vec<(&str, &str, &str, Option<&str>)> = vec![
        ("unknownWorkspace", "absent", "tree", None),
        ("unknownEntity", WORKSPACE, "absent", None),
        ("unknownComponent", WORKSPACE, "tree", Some("Missing")),
        // A composite component path with nothing stored at or beneath it.
        ("unknownCompositeComponent", WORKSPACE, "tree", Some("Alpha/absent")),
        // One segment deeper than anything created.
        ("deeperThanCreated", WORKSPACE, "tree", Some("Alpha/inner/leaf")),
        // The component exists but belongs to another entity.
        ("componentOfAnotherEntity", WORKSPACE, "sparse", Some("Alpha")),
    ];

    for (case, workspace_id, entity_id, component_path) in rows {
        let request = match component_path {
            None => json!({}),
            Some(component_path) => json!({"componentPath": component_path}),
        };
        let refused = emulator
            .list_components(workspace_id, entity_id, request)
            .await;
        assert_eq!(
            refused.status, 404,
            "case {case:?}: expected 404 from ListComponents, got {refused:?}"
        );
        assert_eq!(
            refused.error_type.as_deref(),
            Some("ResourceNotFoundException"),
            "case {case:?}: expected a ResourceNotFoundException, got {refused:?}"
        );
    }
}

/// RULE (the paging): whatever page size is asked for, following `nextToken`
/// walks the level exactly once, in name order, and the token is absent on the
/// last page.
#[tokio::test]
async fn every_page_size_walks_the_level_exactly_once() {
    let emulator = Emulator::new();
    seed(&emulator).await;

    // A level of seven components, so a page size can divide it evenly, unevenly
    // or not at all.
    let mut components = serde_json::Map::new();
    for index in 1..=7 {
        components.insert(
            format!("c{index:02}"),
            json!({"componentTypeId": "com.example.plain"}),
        );
    }
    let created = emulator
        .create_entity(
            WORKSPACE,
            json!({
                "entityId": "paged",
                "entityName": "Paged",
                "components": Value::Object(components),
            }),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "expected 200 from CreateEntity, got {created:?}"
    );

    // Each scope with the whole set it holds, in the name order the listing
    // reports: the top level of `paged`, and one level down inside `tree`.
    let scopes: Vec<(&str, Option<&str>, Vec<&str>)> = vec![
        (
            "paged",
            None,
            vec!["c01", "c02", "c03", "c04", "c05", "c06", "c07"],
        ),
        ("tree", None, vec!["Alpha", "Beta", "Gamma"]),
        ("tree", Some("Alpha"), vec!["inner", "leaf", "nested"]),
    ];

    for (entity_id, component_path, whole) in scopes {
        // 1 row at a time, an uneven divisor, the exact size, one past it, and
        // the largest page the member admits.
        for page_size in [1_u32, 2, 3, 7, 200] {
            let case = format!("{entity_id}:{component_path:?}:{page_size}");
            let mut walked: Vec<String> = Vec::new();
            let mut next_token: Option<String> = None;
            let mut pages = 0;
            loop {
                let mut request = json!({"maxResults": page_size});
                if let Some(component_path) = component_path {
                    request["componentPath"] = json!(component_path);
                }
                if let Some(token) = &next_token {
                    request["nextToken"] = json!(token);
                }
                let page = emulator.list_components(WORKSPACE, entity_id, request).await;
                assert_eq!(
                    page.status, 200,
                    "case {case:?}: expected 200 from ListComponents, got {page:?}"
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
                "case {case:?}: following nextToken must walk the level exactly once"
            );
        }

        // A page size of zero: the member publishes no minimum, so it is served
        // as what it says — no rows — and the token names where the next call
        // resumes from, which is the position this one started at.
        let mut request = json!({"maxResults": 0});
        if let Some(component_path) = component_path {
            request["componentPath"] = json!(component_path);
        }
        let empty = emulator.list_components(WORKSPACE, entity_id, request).await;
        assert_eq!(
            empty.status, 200,
            "case {entity_id:?}: expected 200 for maxResults 0, got {empty:?}"
        );
        assert!(
            names(&empty).is_empty(),
            "case {entity_id:?}: expected no rows for maxResults 0, got {empty:?}"
        );
        assert!(
            empty.member("nextToken").is_string(),
            "case {entity_id:?}: a level with rows left over mints a token, got {empty:?}"
        );
    }
}

/// RULE (the token's scope): a `nextToken` is bound to the level it was minted
/// for, so one replayed against another entity or another `componentPath` is
/// refused with the modelled `ValidationException` rather than silently
/// resumed.
#[tokio::test]
async fn a_token_from_another_level_is_refused() {
    let emulator = Emulator::new();
    seed(&emulator).await;

    let first = emulator
        .list_components(WORKSPACE, "tree", json!({"maxResults": 1}))
        .await;
    assert_eq!(
        first.status, 200,
        "expected 200 from ListComponents, got {first:?}"
    );
    let token = first
        .member("nextToken")
        .as_str()
        .unwrap_or_else(|| panic!("expected a token for a level of three, got {first:?}"))
        .to_string();

    // Each row replays that token somewhere it was not minted for.
    let rows: Vec<(&str, &str, Value)> = vec![
        (
            "anotherComponentPath",
            "tree",
            json!({"componentPath": "Alpha", "nextToken": token.clone()}),
        ),
        (
            "anotherEntity",
            "sparse",
            json!({"nextToken": token.clone()}),
        ),
        (
            "notATokenAtAll",
            "tree",
            json!({"nextToken": "not-a-token"}),
        ),
    ];
    for (case, entity_id, request) in rows {
        let refused = emulator.list_components(WORKSPACE, entity_id, request).await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected 400 from ListComponents, got {refused:?}"
        );
        assert_eq!(
            refused.error_type.as_deref(),
            Some("ValidationException"),
            "case {case:?}: expected a ValidationException, got {refused:?}"
        );
    }

    // The same token on the level it WAS minted for resumes there.
    let resumed = emulator
        .list_components(WORKSPACE, "tree", json!({"nextToken": token}))
        .await;
    assert_eq!(
        resumed.status, 200,
        "expected 200 when the token is replayed on its own level, got {resumed:?}"
    );
    assert_eq!(
        names(&resumed),
        vec!["Beta".to_string(), "Gamma".to_string()],
        "expected the level to resume after the first row, got {resumed:?}"
    );
}

/// RULE (a listing reports state, and changes none): the components a page
/// reports follow the entity's stored components, and reading a page leaves what
/// a later caller sees untouched.
#[tokio::test]
async fn a_page_follows_stored_state_and_changes_nothing() {
    let emulator = Emulator::new();
    seed(&emulator).await;

    let before = emulator.list_components(WORKSPACE, "tree", json!({})).await;
    assert_eq!(
        before.status, 200,
        "expected 200 from ListComponents, got {before:?}"
    );
    let again = emulator.list_components(WORKSPACE, "tree", json!({})).await;
    assert_eq!(
        summaries(&again),
        summaries(&before),
        "expected two reads of one level to report the same rows, got {again:?}"
    );

    // Nothing a later caller sees changed because two pages were served: the
    // stored components are exactly the ones the create persisted.
    {
        let state = emulator.state();
        let state = state.lock();
        let stored: Vec<&String> = state.region(REGION).workspaces[WORKSPACE].entities["tree"]
            .components
            .keys()
            .collect();
        assert_eq!(
            stored,
            vec!["Alpha", "Beta", "Gamma"],
            "expected serving a page to write nothing, got {stored:?}"
        );
    }

    // A component the store gains is reported by the next page: the listing
    // follows stored state rather than anything the handler holds. The store is
    // written directly because `UpdateEntity` — the operation that adds a
    // component to an entity — is still a scaffold.
    {
        let state = emulator.state();
        let mut state = state.lock();
        let components = &mut state
            .region_mut(REGION)
            .workspaces
            .get_mut(WORKSPACE)
            .expect("the seeded workspace")
            .entities
            .get_mut("tree")
            .expect("the seeded entity")
            .components;
        components.insert(
            "Delta".to_string(),
            service_com_amazonaws_iottwinmaker_awsiottwinmaker::state::ComponentRecord {
                component_name: "Delta".to_string(),
                component_type_id: "com.example.plain".to_string(),
                description: Some("added later".to_string()),
                ..Default::default()
            },
        );
    }

    let after = emulator.list_components(WORKSPACE, "tree", json!({})).await;
    assert_eq!(
        names(&after),
        vec![
            "Alpha".to_string(),
            "Beta".to_string(),
            "Delta".to_string(),
            "Gamma".to_string(),
        ],
        "expected the added component to be listed in name order, got {after:?}"
    );
    let delta = summaries(&after)
        .into_iter()
        .find(|summary| summary["componentName"] == "Delta")
        .unwrap_or_else(|| panic!("expected the added component, got {after:?}"));
    assert_eq!(
        delta["description"].as_str(),
        Some("added later"),
        "expected the stored description, got {delta:?}"
    );
    assert_eq!(
        delta["componentTypeId"].as_str(),
        Some("com.example.plain"),
        "expected the stored componentTypeId, got {delta:?}"
    );
}
