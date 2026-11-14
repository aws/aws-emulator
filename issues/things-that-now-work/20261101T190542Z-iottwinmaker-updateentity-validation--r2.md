Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 7c21f9c68dc730e2cbbd4030b1f581b17345dc03

# Re-pointing a component's componentTypeId is not re-checked against the values the component already holds, so a read reports a value that contradicts the dataType it reports

_Filed: 2026-11-01T19:05:42Z_
_Operation: UpdateEntity | Class: validation_

> _Note: this issue was identified against the code base at commit `7c21f9c68dc730e2cbbd4030b1f581b17345dc03`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`apply_component_update` knows that an entry which re-points `componentUpdates.<name>.componentTypeId` invalidates what the component ALREADY holds: for that exact reason it re-asks the DEFINITION rule over the whole patched record (`stranded_component_property_definition`) rather than only over the entries the request named. The VALUE rule beside it — `Workspace::component_property_value_violation`, which the same function's own comment says exists "so a mistyped value is refused rather than left for a read to report against a `dataType` it contradicts" — is asked ONLY of values the request itself carries (`if let Some(value) = update.value.as_ref()`), never of the values the component already stores.

So a component whose property holds a value written under its old component type's `dataType` keeps that value when a patch re-points `componentTypeId` to a type whose declaration for the same property has a different `dataType`. Nothing refuses the patch and nothing rewrites the value, so `GetEntity` / `ListProperties` / `GetPropertyValue` then report, for one property, a `PropertyDefinitionResponse.dataType` of (say) DOUBLE beside a `value` of `{"stringValue": ...}` — precisely the state the crate's value rule is declared to prevent, and the state the crate refuses to create on every direct write (CreateEntity's `require_property_types`, UpdateEntity's own `propertyUpdates` pass, and BatchPutPropertyValues' `data_value_type_violation`).

The asymmetry is inside one function: the definition half of the pair is asked of the whole record after a re-point, the value half is not.

## Evidence

THE DEFINITION HALF, RE-ASKED OVER THE WHOLE RECORD BECAUSE A RE-POINT TAKES THE OLD INHERITANCE AWAY — services/service-com-amazonaws-iottwinmaker-awsiottwinmaker/src/ops/update_entity.rs:498-523:
```
    // Every definition the patched component would hold has to be reportable,
    // not just the ones this request carried: a declaration of the component's
    // own may omit its `dataType` because it REFINES one the component type it
    // instantiates declares, so an entry that re-points `componentTypeId` takes
    // the inheritance an older, already-stored declaration was relying on away
    // with it — the rule
    // [`crate::state::Workspace::component_property_definition_needs_data_type`]
    // asked of the whole record
    // ([`crate::state::Workspace::stranded_component_property_definition`]),
...
    if declares_component_type_id {
        if let Some(property_name) = workspace.stranded_component_property_definition(
            component_type_id.as_deref(),
            &component.properties,
        ) {
```

THE VALUE HALF, ASKED ONLY OF WHAT THE REQUEST CARRIES — src/ops/update_entity.rs:720-744:
```
                // The value this entry writes has to be a value of the
                // `dataType` the property ENDS UP with: the definition the
                // entry carries where it carries one (it replaces the
                // property's), else the property's own, refined over what the
                // holder's component type declares — the rule
                // `crate::state::Workspace::component_property_value_violation`
                // every writer of a property's value shares, decided before
                // the patch is applied so a mistyped value is refused rather
                // than left for a read to report against a `dataType` it
                // contradicts.
                if let Some(value) = update.value.as_ref() {
```
There is no counterpart to `stranded_component_property_definition` for values anywhere in the crate: `grep -n component_property_value_violation src/` reaches only the two per-request passes (src/ops/create_entity.rs:669 and src/ops/update_entity.rs:736).

THE STATE MODULE STATES WHY THE WHOLE-RECORD READING IS THE RIGHT ONE AFTER A RE-POINT — src/state.rs:1719-1727 (doc of `stranded_component_property_definition`):
```
    /// asked of the definitions a component ALREADY HOLDS rather than of one a
    /// request carries, because the rule's other input is the component type
    /// those declarations inherit a `dataType` from: a writer that re-points it
    /// is the call that takes the inheritance away, so it answers the rule over
    /// the WHOLE record it would store instead of over the entries the request
    /// happened to name.
```
The value rule has the same "other input" (`component_property_value_violation` resolves the dataType through `refined_component_property_definition(component_type_id, ...)`, src/state.rs:1763-1776), so a re-point moves its answer too.

TEST COVERAGE: the re-point suite exercises only the definition half — tests/all/component_type_repoint.rs:6-13 ("RULE under test: a writer of a component's definitions refuses a request that would leave the component holding a property definition with no `dataType` at all ... the entry that re-points `componentTypeId` is the call that takes that inheritance away"), and it declares a type for exactly this shape of case at tests/all/component_type_repoint.rs:50-52 ("A component type that declares [`PROPERTY`] itself, with another `dataType` than [`GAUGE`] does"). No case in that file writes a property VALUE at all (`grep -n '"value"\|stringValue\|doubleValue' tests/all/component_type_repoint.rs` → no matches), and the value-type suites (tests/all/entity_property_value_data_type.rs, tests/all/property_value_data_type.rs) only exercise values supplied by the refused request. Nothing pins the current outcome.

## Steps to Reproduce

1. CreateWorkspace(workspaceId="ws").
2. CreateComponentType(workspaceId="ws", componentTypeId="com.example.gauge", propertyDefinitions={"Reading": {"dataType": {"type": "STRING"}}}).
3. CreateComponentType(workspaceId="ws", componentTypeId="com.example.counter", propertyDefinitions={"Reading": {"dataType": {"type": "DOUBLE"}}}).
4. CreateEntity(workspaceId="ws", entityId="e1", entityName="E1", components={"Spec": {"componentTypeId": "com.example.gauge", "properties": {"Reading": {"value": {"stringValue": "warm"}}}}}) — accepted, the value matching the inherited STRING dataType.
5. UpdateEntity(workspaceId="ws", entityId="e1", componentUpdates={"Spec": {"updateType": "UPDATE", "componentTypeId": "com.example.counter"}}) — accepted with HTTP 200: `stranded_component_property_definition` finds nothing (the stored property record carries no definition of its own) and the value rule is never asked, because the request carries no value.
6. GetEntity(workspaceId="ws", entityId="e1") — observe components.Spec.properties.Reading reporting definition.dataType.type = "DOUBLE" beside value = {"stringValue": "warm"}.
7. Contrast the direct write the crate refuses: UpdateEntity(..., componentUpdates={"Spec": {"updateType": "UPDATE", "propertyUpdates": {"Reading": {"value": {"stringValue": "warm"}}}}}) against com.example.counter — refused with ValidationException by src/ops/update_entity.rs:736.
