Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 61b3c1d663b263c5c590b906e5a6614cc7178701

# ListComponents reports an empty-string componentTypeId for a component created without one, outside the @required/@length(1..256)/@pattern the modelled ComponentSummary member declares

_Filed: 2026-11-01T00:28:45Z_
_Operation: ListComponents | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `61b3c1d663b263c5c590b906e5a6614cc7178701`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

A component may legitimately be created with no `componentTypeId` — `ComponentRequest.componentTypeId` is optional, and the crate has a whole test module for such "untyped" components. `ListComponents` reports every component it lists as a `ComponentSummary`, whose `componentTypeId` is modelled `@required` with `@length(min: 1, max: 256)` and `@pattern(^[a-zA-Z_\.\-0-9:]+$)`. For an untyped component the handler fills that member with `String::default()` — the empty string, which satisfies neither the length minimum nor the pattern — so the page carries a response value the model forbids.

The handler's own comment admits the member is required and that it has nothing to put there. The crate's sibling reader of the SAME component, `GetEntity`, reports the same stored field through `ComponentResponse.componentTypeId`, which the model leaves optional, and correctly OMITS it (`component.component_type_id.clone()`). So one and the same untyped component reads back as an absent member through GetEntity and as an invalid empty string through ListComponents.

## Evidence

THE MODEL — `smithy_query(query="shape", shape="ComponentSummary")`:
```
"componentTypeId": {"target": "ComponentTypeId", "required": true, "length": {"min": 1, "max": 256}, "pattern": "^[a-zA-Z_\\.\\-0-9:]+$", "doc": "<p>The ID of the component type.</p>"}
```
An empty string is outside both `length.min: 1` and the pattern (which requires one or more characters).

THE HANDLER SERVES THE EMPTY STRING, AND SAYS SO — services/service-com-amazonaws-iottwinmaker-awsiottwinmaker/src/ops/list_components.rs:383-390:
```
            Self::Component(component) => ComponentSummary {
                component_name: component.component_name.clone(),
                // Required on the summary but optional on the request a
                // component is created from, so a component stored without one
                // has no id to report — and is still listed, because a listing
                // reports the components the entity holds rather than hiding the
                // ones whose create named no component type.
                component_type_id: component.component_type_id.clone().unwrap_or_default(),
```

THE SIBLING READER OF THE SAME FIELD OMITS IT INSTEAD — src/ops/get_entity.rs:336-339 (`ComponentResponse.componentTypeId` is optional in the model, so an untyped component reports no member at all):
```
            let response = ComponentResponse {
                component_name: Some(component.component_name.clone()),
                description: component.description.clone(),
                component_type_id: component.component_type_id.clone(),
```

THE STORED FIELD IS GENUINELY OPTIONAL — src/ops/list_components.rs:391-392 resolves `definedIn` from `component.component_type_id.as_deref()` as an `Option`, and `smithy_query(query="shape", shape="ComponentRequest")`/the untyped-component test module `tests/all/create_entity_untyped_definition_order.rs` show a component create may name no `componentTypeId`.

TEST COVERAGE: tests/all/component_listing.rs:183-188 pins the current empty string for the untyped component `Gamma`:
```
                Expected {
                    name: "Gamma",
                    component_type_id: "",
                    component_path: None,
                    description: Some("typeless"),
                },
```
under a RULE comment (tests/all/component_listing.rs:155-158) about WHICH LEVEL of the tree a page reports — it states no reason for serving a value outside the member's modelled bounds, so that expectation moves with the fix.

## Steps to Reproduce

1. CreateWorkspace `w`.
2. POST /workspaces/w/entities with `{"entityId":"e","entityName":"e","components":{"Gamma":{"description":"typeless","properties":{"p":{"definition":{"dataType":{"type":"STRING"},"isTimeSeries":false}}}}}}` — a component with no `componentTypeId`, which `ComponentRequest` permits.
3. POST /workspaces/w/entities/e/components-list (ListComponents, no `componentPath`).
4. Observe the summary for `Gamma` carries `"componentTypeId": ""` — a value outside the `@length(min: 1, max: 256)` and `@pattern(^[a-zA-Z_\.\-0-9:]+$)` the modelled `ComponentSummary.componentTypeId` declares for that `@required` member.
5. GET /workspaces/w/entities/e (GetEntity) and observe `components.Gamma` carries NO `componentTypeId` member at all — the same stored field, reported two different ways by two readers of this crate.
