// SPDX-License-Identifier: CC0-1.0

//! The rules every writer of a requested property DEFINITION or property GROUP
//! meets, beside the ones that resolve a definition's `dataType` and a value's
//! type.
//!
//! Two of them live here because they hold for every writer of the member
//! rather than for one operation:
//!
//! * a member whose model target is an ENUMERATION carries one of the values
//!   that enumeration defines, so a value outside the list is refused rather
//!   than stored — which is what keeps a REQUIRED response member
//!   (`PropertyDefinitionResponse::dataType`,
//!   `ComponentPropertyGroupResponse::groupType`) reportable with a value the
//!   model admits, and keeps a property writable at all, since no `DataValue`
//!   can carry a type the model does not define
//!   ([`super::data_value_type_violation`]). The value lists are the crate's
//!   own (`crate::ops::types`). A request off the WIRE is answered by the
//!   boundary's own value-list check before a handler sees it, as the sibling
//!   handlers of this service note where they restate the rule; a writer that
//!   is not fed from the wire — the one that lands the entries of a metadata
//!   document ([`super::write_document_into_workspace`]) — has no such check in
//!   front of it, and this rule is what holds for it;
//! * a property GROUP groups properties of the schema it is declared on, so
//!   each name it carries is one that schema HAS. No boundary can decide that
//!   one: it is a question about the schema the write leaves behind. The
//!   component-TYPE side of the rule is [`super::property_groups_violation`];
//!   the COMPONENT side, whose properties reach it through the component type
//!   it instantiates, is [`component_property_groups_violation`] below.

use super::*;

/// Why `value`, supplied for a member whose model target is an enumeration,
/// cannot be stored — `None` when it is one of `accepted`.
///
/// `member` is the member in the caller's own terms, so a refusal names the
/// exact place that has to be fixed, and the reason names the values the model
/// publishes rather than only the one that failed.
pub fn enum_value_violation(member: &str, value: &str, accepted: &[&str]) -> Option<String> {
    (!accepted.contains(&value)).then(|| {
        format!(
            "{member} {value} is not one of {}",
            accepted.join(", ")
        )
    })
}

/// Why a requested `dataType` cannot be stored — `None` when it can.
///
/// `DataType::type` targets the modelled `Type` enumeration
/// ([`crate::ops::types::TYPE_VALUES`]) and is `@required` on the `DataType` a
/// read reports, so a value outside that list is refused: it would otherwise be
/// reported verbatim on a required member the model does not admit it on, and
/// would leave the property unwritable for good, since no `DataValue` can carry
/// a type the model does not define ([`super::data_value_type_violation`]).
///
/// The NESTED type is held to the same rule: `nestedType` targets `DataType`
/// itself, so the walk follows it to the bottom and names the member it
/// refused. A `dataType` the document reader could find no `type` in at all
/// carries the empty string ([`super::write_document_into_workspace`]'s
/// reader), which is outside the list like any other value the model does not
/// define.
pub fn data_type_violation(
    member: &str,
    data_type: &crate::ops::types::DataType,
) -> Option<String> {
    let mut member = member.to_string();
    let mut data_type = data_type;
    loop {
        if let Some(violation) = enum_value_violation(
            &format!("{member}.type"),
            &data_type.r#type,
            crate::ops::types::TYPE_VALUES,
        ) {
            return Some(violation);
        }
        let nested = data_type.nested_type.as_deref()?;
        member = format!("{member}.nestedType");
        data_type = nested;
    }
}

/// Why a requested property definition cannot be stored, as far as the modelled
/// value lists its members target decide it — `None` when nothing in it is
/// outside one.
///
/// `member` is the definition's own member path (`propertyDefinitions.<name>`,
/// `components.<name>.properties.<name>.definition`), so the refusal names the
/// definition the caller supplied. Shared by every writer of a definition — a
/// component type's and a component's — so one definition is held to one rule
/// wherever it arrives.
pub fn property_definition_violation(
    member: &str,
    definition: &crate::ops::types::PropertyDefinitionRequest,
) -> Option<String> {
    let data_type = definition.data_type.as_ref()?;
    data_type_violation(&format!("{member}.dataType"), data_type)
}

/// [`property_definition_violation`] asked of a whole requested map of
/// definitions, in property-NAME order — the order every writer of a requested
/// map walks one ([`super::requested_in_key_order`]), so a request carrying
/// more than one refusable definition is always refused for the first of them
/// in key order rather than for whichever the hash map's iteration order
/// reached first.
///
/// `member` is the map the definitions hang off (`propertyDefinitions`).
pub fn property_definitions_violation<'definitions>(
    member: &str,
    definitions: impl IntoIterator<
        Item = (
            &'definitions str,
            &'definitions crate::ops::types::PropertyDefinitionRequest,
        ),
    >,
) -> Option<String> {
    definitions
        .into_iter()
        .find_map(|(property_name, definition)| {
            property_definition_violation(&format!("{member}.{property_name}"), definition)
        })
}

/// Why the property groups a COMPONENT — top-level or composite — would come to
/// hold cannot be stored, as the message a `ValidationException` carries;
/// `None` when every one of them can.
///
/// The component-level counterpart of [`super::property_groups_violation`],
/// which asks the same two questions of a component TYPE's groups:
///
/// * `groupType` targets the modelled `GroupType` enumeration
///   ([`crate::ops::types::GROUP_TYPE_VALUES`]), and is `@required` on the
///   `ComponentPropertyGroupResponse` a read reports, so a value outside that
///   list is refused rather than reported verbatim on that member;
/// * a group GROUPS properties of the component, so each name it carries has to
///   be one the component HAS — one of its own, or one the component type it
///   instantiates and the types that type extends define
///   ([`Workspace::component_property_names`]).
///
/// Asked of the groups and the properties as the write LEAVES them, so a group
/// may name a property the same request declares, exactly as a component type's
/// group may name a definition its own request carries. Where the component's
/// schema is not fully known to this emulator — it names no component type at
/// all beyond its own declarations, names one this workspace does not hold, or
/// one whose inheritance reaches a built-in type of the real service this
/// emulator does not model — the properties that type gives it cannot be
/// enumerated, so a name this workspace cannot account for is not called
/// undefined and the group stands: the same stand-down the component-type rule
/// makes on the same unknowable chain. A group carrying no names at all names
/// nothing to check: an empty list INHERITS the names of the declaration behind
/// it ([`super::refined_property_group`]).
///
/// `member` is the member the component hangs off in the caller's own terms
/// (`components.<name>`, `compositeComponents.<path>`,
/// `componentUpdates.<name>`), so the refusal names the exact place that has to
/// be fixed. Walked in group-NAME order, and each group's names in the order
/// the request listed them, so a request carrying more than one refusable group
/// is always refused for the first of them in key order.
pub fn component_property_groups_violation(
    workspace: &Workspace,
    member: &str,
    component_type_id: Option<&str>,
    properties: &BTreeMap<String, PropertyRecord>,
    property_groups: &BTreeMap<String, PropertyGroupRecord>,
) -> Option<String> {
    if property_groups.is_empty() {
        return None;
    }
    // Resolved ONCE for the whole map: the component's properties are the same
    // for every group hanging off it, so the inheritance chain behind them is
    // walked once per component rather than once per group.
    let component = ComponentSchema::of(workspace, component_type_id, properties);
    property_groups.iter().find_map(|(group_name, group)| {
        component.group_violation(&format!("{member}.propertyGroups.{group_name}"), group)
    })
}

/// [`component_property_groups_violation`] asked of ONE property group, under
/// the member path that group itself hangs off
/// (`componentUpdates.<name>.propertyGroupUpdates.<group>`).
///
/// What a writer that PATCHES groups one entry at a time needs: the entry is
/// judged as the record it leaves behind, against the properties the component
/// holds after the same request's property updates.
pub fn component_property_group_violation(
    workspace: &Workspace,
    member: &str,
    component_type_id: Option<&str>,
    properties: &BTreeMap<String, PropertyRecord>,
    group: &PropertyGroupRecord,
) -> Option<String> {
    ComponentSchema::of(workspace, component_type_id, properties).group_violation(member, group)
}

/// The part of a component's EFFECTIVE schema the property-group rule is
/// decided against: every property the component has, and whether that set is
/// all of them.
struct ComponentSchema {
    /// Every property the component holds, its own and the ones its component
    /// type gives it ([`Workspace::component_property_names`]).
    property_names: BTreeSet<String>,
    /// Whether this emulator holds every definition the component is given
    /// ([`component_schema_is_known`]); when it does not, a name the set does
    /// not carry cannot be called one the component does not have.
    is_known: bool,
}

impl ComponentSchema {
    /// The schema of a component instantiating `component_type_id` and holding
    /// `properties`.
    fn of(
        workspace: &Workspace,
        component_type_id: Option<&str>,
        properties: &BTreeMap<String, PropertyRecord>,
    ) -> Self {
        Self {
            property_names: workspace.component_property_names(component_type_id, properties),
            is_known: component_schema_is_known(workspace, component_type_id),
        }
    }

    /// Why `group`, hanging off `member`, cannot be stored on a component with
    /// this schema — `None` when it can.
    fn group_violation(&self, member: &str, group: &PropertyGroupRecord) -> Option<String> {
        if let Some(group_type) = group.group_type.as_deref() {
            if let Some(violation) = enum_value_violation(
                &format!("{member}.groupType"),
                group_type,
                crate::ops::types::GROUP_TYPE_VALUES,
            ) {
                return Some(violation);
            }
        }
        if !self.is_known {
            return None;
        }
        group
            .property_names
            .iter()
            .find(|property_name| !self.property_names.contains(*property_name))
            .map(|property_name| {
                format!(
                    "{member}.propertyNames names property {property_name}, which the component \
                     does not have"
                )
            })
    }
}

/// Whether this emulator holds every property definition a component
/// instantiating `component_type_id` is given.
///
/// A component takes the properties of the component type it names and of the
/// types that type extends, so the set is known only when this workspace holds
/// that whole chain. It does not when the component names no component type —
/// then it has only what it declares itself, which IS the whole set — so that
/// case answers `true`; it does not when the id names a component type the
/// workspace does not hold (a built-in of the real service this emulator does
/// not model, which `CreateEntity` deliberately accepts), nor when the chain
/// behind a stored one reaches such a type.
fn component_schema_is_known(workspace: &Workspace, component_type_id: Option<&str>) -> bool {
    let Some(component_type_id) = component_type_id else {
        return true;
    };
    let Some(stored) = workspace.component_types.get(component_type_id) else {
        return false;
    };
    let (_, holds_unstored) = workspace.ancestor_component_type_ids(&stored.extends_from);
    !holds_unstored
}
