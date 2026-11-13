Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 61b3c1d663b263c5c590b906e5a6614cc7178701

# An entity component's dataType.type, propertyGroups.groupType and propertyGroups.propertyNames are stored unchecked, although the crate declares the wire value lists and the component-type writers apply the group rule

_Filed: 2026-11-01T00:28:45Z_
_Operation: CreateEntity | Class: validation_

> _Note: this issue was identified against the code base at commit `61b3c1d663b263c5c590b906e5a6614cc7178701`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

CreateEntity (and the UpdateEntity path that shares the same conversion helpers) stores three caller-supplied members of a component verbatim, with no check at all:

1. `components.*.properties.*.definition.dataType.type` — the member targets the modelled `Type` enum. The crate DECLARES that enum's wire value list (`TYPE_VALUES`) and never consults it anywhere in `src/`. A definition carrying `dataType: {"type": "FOO"}` is stored and then reported verbatim on `PropertyDefinitionResponse.dataType`, a REQUIRED member whose `DataType.type` is itself `@required` and targets `Type`. It also makes the property permanently unwritable: `crate::state::data_value_type_violation` compares the type a `DataValue` carries against the declared string, and no `DataValue` can ever carry "FOO", so every later CreateEntity/UpdateEntity/BatchPutPropertyValues write to that property is refused forever.

2. `components.*.propertyGroups.*.groupType` — the member targets the modelled `GroupType` enum, whose only value is `TABULAR`. The crate declares `GROUP_TYPE_VALUES` and never consults it. The stored string is reported verbatim on `ComponentPropertyGroupResponse.groupType` (REQUIRED, target `GroupType`) by GetEntity and ListComponents — the very thing the state module says it avoids: "a group stored without one is reported with the only value the model admits rather than with an invented string".

3. `components.*.propertyGroups.*.propertyNames` — never checked against the properties the component actually has, although BOTH component-type writers in this crate apply exactly that rule to the type-level `propertyGroups` member through `crate::state::property_groups_violation`, whose doc states the rule and says it is "Shared by every writer of a component type's `propertyGroups`". A component group may therefore name a property that `ListProperties` on the same component does not list.

This is not a rule the crate is unaware of: every OTHER handler of this service refuses an out-of-list value for an enum-shaped member precisely because, as list_metadata_transfer_jobs.rs says, the boundary validator does not enforce a modelled enum's value list — UpdatePricingPlan (`PRICING_MODE_VALUES`), ListMetadataTransferJobs (`SOURCE_TYPE_VALUES`, `DESTINATION_TYPE_VALUES`, `METADATA_TRANSFER_JOB_STATE_VALUES`), ListSyncResources (`SYNC_RESOURCE_STATE_VALUES`, `SYNC_RESOURCE_TYPE_VALUES`), GetPropertyValue (`ORDER_VALUES`), GetPropertyValueHistory (`ORDER_BY_TIME_VALUES`, `INTERPOLATION_TYPE_VALUES`), CreateSyncJob (`syncSource`) and UpdateEntity itself for the three update-type members. `groupType` and `dataType.type` are the two input enums left out.

## Evidence

THE CRATE DECLARES THE WIRE VALUE LISTS AND NEVER CONSULTS THEM — services/service-com-amazonaws-iottwinmaker-awsiottwinmaker/src/ops/types/shared_types.rs:398-399 and :466-467:
```
/// The values `GroupType` accepts on the wire.
pub const GROUP_TYPE_VALUES: &[&str] = &["TABULAR"];
...
/// The values `Type` accepts on the wire.
pub const TYPE_VALUES: &[&str] = &["BOOLEAN", "DOUBLE", "INTEGER", "LIST", "LONG", "MAP", "RELATIONSHIP", "STRING"];
```
`grep -r '_VALUES' src/` shows `GROUP_TYPE_VALUES` and `TYPE_VALUES` appearing ONLY in doc comments (state.rs:2607, state.rs:2913, state.rs:2941, state.rs:3481) — never in an expression.

THE MODEL — `smithy_query(query="shape", shape="GroupType,Type,ComponentPropertyGroupResponse,DataType")`:
```
"GroupType": {"type": "string", "values": ["TABULAR"]}
"Type": {"type": "string", "values": ["RELATIONSHIP","STRING","LONG","BOOLEAN","INTEGER","DOUBLE","LIST","MAP"]}
"ComponentPropertyGroupResponse": {... "groupType": {"target": "GroupType", "required": true}, "propertyNames": {"target": "PropertyNames", "required": true}}
"DataType": {... "type": {"target": "Type", "required": true, "doc": "<p>The underlying type of the data type.</p>"}}
```

THE WRITER STORES THEM VERBATIM — src/ops/create_entity.rs:570-581:
```
    property_groups
        .into_iter()
        .map(|(group_name, group)| {
            (
                group_name,
                crate::state::PropertyGroupRecord {
                    group_type: group.group_type,
                    property_names: group.property_names.unwrap_or_default(),
                },
            )
        })
        .collect()
```
and the UpdateEntity counterpart, whose helper does not even take the workspace, so it could not check a name — src/ops/update_entity.rs:772-805:
```
fn apply_property_group_updates(
    property_groups: &mut ::std::collections::BTreeMap<String, crate::state::PropertyGroupRecord>,
    member_path: &str,
    updates: Option<::std::collections::HashMap<String, ComponentPropertyGroupRequest>>,
) -> Result<(), UpdateEntityError> {
...
        if let Some(group_type) = update.group_type {
            group.group_type = Some(group_type);
        }
...
        if let Some(property_names) = update.property_names {
            group.property_names = property_names;
        }
```

THE READER REPORTS THEM ON THE REQUIRED MEMBERS — src/state.rs:2628-2639:
```
pub fn component_property_group_response(
    group: &PropertyGroupRecord,
    is_inherited: bool,
) -> crate::ops::types::ComponentPropertyGroupResponse {
    crate::ops::types::ComponentPropertyGroupResponse {
        group_type: group
            .group_type
            .clone()
            .unwrap_or_else(|| TABULAR_GROUP_TYPE.to_string()),
        property_names: group.property_names.clone(),
```
whose own doc states the intent the code breaks — src/state.rs:2606-2612:
```
/// The single value the model's `GroupType` enum defines
/// (`ops::types::GROUP_TYPE_VALUES`).
///
/// `groupType` is required on every property-group RESPONSE but optional on the
/// request, so a group stored without one is reported with the only value the
/// model admits rather than with an invented string.
pub const TABULAR_GROUP_TYPE: &str = "TABULAR";
```
and src/state.rs:3374-3383, which passes the stored `dataType` straight through:
```
    let Some(data_type) = definition.data_type.clone() else { ... };
    Some(crate::ops::types::PropertyDefinitionResponse {
        data_type,
```

THE SIBLING RULE THIS WRITER SKIPS (group membership) — src/state.rs:3225-3235:
```
/// A property group GROUPS properties of the component type, so each name it
/// carries has to be one the component type defines: one of its own
/// definitions, or one it inherits from the component types it extends. ...
/// Shared by every writer of a component type's `propertyGroups`.
pub fn property_groups_violation(workspace: &Workspace, record: &ComponentType) -> Option<String> {
```
applied by src/ops/create_component_type.rs:427 and src/ops/update_component_type.rs:392, with no component-level counterpart anywhere — unlike the `dataType` rule, which the crate DOES give a component-level counterpart (src/state.rs:2683-2687: "the same question about a COMPONENT's definition ... is [`Workspace::component_property_definition_needs_data_type`]", used at src/ops/update_entity.rs:711-717 and src/ops/create_entity.rs:338-345).

THE CRATE STATES WHY SUCH A VALUE REACHES THE HANDLER AT ALL — src/ops/list_metadata_transfer_jobs.rs:226-232:
```
        // The ends the listing is taken over. Both are modelled enumerations,
        // whose value lists the boundary validator does not enforce (the same
        // reason `UpdatePricingPlan` checks its `pricingMode` here), so a value
        // outside them is refused rather than quietly selecting no job.
        if !SOURCE_TYPE_VALUES.contains(&input.source_type.as_str()) {
```
and the siblings that do check: src/ops/update_pricing_plan.rs:202, src/ops/list_sync_resources.rs:268-270, src/ops/get_property_value.rs:979, src/ops/get_property_value_history.rs:408 and :513, src/ops/update_entity.rs:783 (`PROPERTY_GROUP_UPDATE_TYPE_VALUES`, checked on the very same request entry whose `groupType` is not).

TEST COVERAGE: tests/all/property_group_refinement.rs, tests/all/component_inherited_schema.rs and tests/all/component_refined_definition.rs drive component property groups, and tests/all/entity_property_value_data_type.rs drives the dataType rule; none of them sends a `groupType` or a `dataType.type` outside the modelled value list, nor a group `propertyNames` entry the component does not have — which is why the omission is unnoticed.

## Steps to Reproduce

1. CreateWorkspace `w`.
2. CreateComponentType `w`/`com.example.sensor` with `propertyDefinitions: {"heat": {"dataType": {"type": "DOUBLE"}}}`.
3. POST /workspaces/w/entities with `{"entityName":"e","entityId":"e","components":{"C":{"componentTypeId":"com.example.sensor","propertyGroups":{"G":{"groupType":"NOT_A_GROUP_TYPE","propertyNames":["notAProperty"]}}}}}` — accepted 200.
4. GET /workspaces/w/entities/e and observe `components.C.propertyGroups.G.groupType == "NOT_A_GROUP_TYPE"`, outside the only value `GroupType` admits (`TABULAR`), on a member the model marks `@required` with that enum as its target; and `propertyNames == ["notAProperty"]`, a property POST /workspaces/w/entities/e/properties-list does not list.
5. Contrast: CreateComponentType `w`/`com.example.bad` with `propertyGroups: {"G": {"propertyNames": ["notAProperty"]}}` IS refused `ValidationException` by `property_groups_violation` (src/ops/create_component_type.rs:427) — the same rule, the same request member shape, enforced on one writer and not the other.
6. POST /workspaces/w/entities with `{"entityName":"f","entityId":"f","components":{"C":{"properties":{"p":{"definition":{"dataType":{"type":"FOO"},"isTimeSeries":false}}}}}}` — accepted 200, and GetEntity reports `dataType.type == "FOO"`, outside the `Type` value list the crate itself prints at shared_types.rs:467.
7. UpdateEntity on `f` with `componentUpdates.C.propertyUpdates.p.value = {"stringValue":"x"}` (or any other DataValue) — refused forever with "the value written to property p is a STRING where that property's dataType is FOO" (src/state.rs:3545-3550): the unchecked enum value left the property unwritable.
