// SPDX-License-Identifier: CC0-1.0

//! The inventory this service holds about the managed nodes of a Region, and how an
//! inventory query reads it.
//!
//! Systems Manager Inventory is organised by TYPE: every inventory item a node reports
//! belongs to one type (`AWS:InstanceInformation`, `AWS:Application`, `AWS:Network`, a
//! `Custom:` type a caller wrote — `InventoryItemSchema$TypeName`), and every attribute of
//! an item is named by its type: `AWS:InstanceInformation.PlatformType`. That dotted form
//! is what an inventory filter key and an aggregator expression carry
//! ([`split_qualified_attribute`]), which is why a key naming no type, or naming no
//! attribute of a type whose schema this service knows, cannot be served at all.
//!
//! The one type this store holds data of is `AWS:InstanceInformation`: "Query inventory
//! information. This includes managed node status, such as `Stopped` or `Terminated`"
//! (`GetInventory`) — the item every managed node has by being registered, projected from
//! the node record the node describes report
//! ([`super::ManagedNodeRecord`], [`instance_information_content`]) rather than stored a
//! second time. Nothing here invents a node or an item: an account that has registered no
//! node holds no inventory, and an attribute the node record holds no value for is
//! reported by nothing and selected by no filter — the same rule
//! [`super::ManagedNodeRecord::filter_values`] follows for the node describes. The other
//! types are types this service knows and holds no item of, because the only item derived
//! here is the node's own and `PutInventory` — the operation a caller writes an item
//! with — is still a scaffold: a query for one of those types is answered with the nothing
//! that has been collected for it, never refused.
//!
//! AGGREGATION is narrower than filtering: only some attributes of a type can be counted
//! over. "Returns inventory schemas that support aggregation. For example, this call
//! returns the `AWS:InstanceInformation` type, because it supports aggregation based on the
//! `PlatformName`, `PlatformType`, and `PlatformVersion` attributes"
//! (`GetInventorySchemaRequest$Aggregator`) — so those three are what an aggregator
//! expression over this type may name ([`INSTANCE_INFORMATION_AGGREGATABLE_ATTRIBUTES`]),
//! and an expression naming another attribute of it is refused rather than counted.

use std::collections::HashMap;

use super::ManagedNodeRecord;

/// The inventory type every managed node has by being registered: the node's own
/// information, which is the default type an inventory query reports ("Valid value:
/// `AWS:InstanceInformation`. Default Value: `AWS:InstanceInformation`" —
/// `ResultAttribute$TypeName`).
pub const INVENTORY_TYPE_INSTANCE_INFORMATION: &str = "AWS:InstanceInformation";

/// The schema version the items of that type carry (`InventoryResultItem$SchemaVersion`,
/// pattern `^([0-9]{1,6})(\.[0-9]{1,6})$`): the first version of a schema this service
/// publishes and has never revised, since the attributes below are the ones a node has
/// reported since the type existed.
pub const INSTANCE_INFORMATION_SCHEMA_VERSION: &str = "1.0";

/// The default inventory types this service knows the names of: "Default inventory item
/// type names start with Amazon Web Services. … Default inventory item types include the
/// following: `AWS:AWSComponent`, `AWS:Application`, `AWS:InstanceInformation`,
/// `AWS:Network`, and `AWS:WindowsUpdate`" (`InventoryItemSchema$TypeName`).
///
/// A type this service knows is a type a query may name; only
/// [`INVENTORY_TYPE_INSTANCE_INFORMATION`] has items here, so a query naming one of the
/// others reads the nothing that has been collected for it.
pub const DEFAULT_INVENTORY_TYPES: &[&str] = &[
    "AWS:AWSComponent",
    "AWS:Application",
    INVENTORY_TYPE_INSTANCE_INFORMATION,
    "AWS:Network",
    "AWS:WindowsUpdate",
];

/// The prefix of a type name a caller's own schema carries, as against the Amazon ones:
/// "Custom inventory type names will start with Custom" (`InventoryItemSchema$TypeName`).
pub const CUSTOM_TYPE_PREFIX: &str = "Custom:";

/// The attributes an `AWS:InstanceInformation` item is made of, in the order an item
/// reports them. Each is one property of the registered node
/// ([`instance_information_attribute`]): the node's identity and kind, the agent it runs,
/// the platform it runs on, its address and name, and the node status `GetInventory`
/// documents this type as carrying.
pub const INSTANCE_INFORMATION_ATTRIBUTES: &[&str] = &[
    ATTRIBUTE_AGENT_TYPE,
    "AgentVersion",
    "ComputerName",
    "InstanceId",
    "InstanceStatus",
    "IpAddress",
    "PlatformName",
    "PlatformType",
    "PlatformVersion",
    "ResourceType",
];

/// The attribute naming WHICH agent reported the item. The node store records the agent's
/// version but not its implementation ([`super::ManagedNodeRecord`] holds the modelled
/// `InstanceProperty`, which has no member for it), so no node holds a value for this
/// attribute and no filter on it selects one — the same answer the node describes give a
/// filter on a property nothing has recorded.
pub const ATTRIBUTE_AGENT_TYPE: &str = "AgentType";

/// The attributes of [`INVENTORY_TYPE_INSTANCE_INFORMATION`] an aggregator may count over:
/// "this call returns the `AWS:InstanceInformation` type, because it supports aggregation
/// based on the `PlatformName`, `PlatformType`, and `PlatformVersion` attributes"
/// (`GetInventorySchemaRequest$Aggregator`).
pub const INSTANCE_INFORMATION_AGGREGATABLE_ATTRIBUTES: &[&str] =
    &["PlatformName", "PlatformType", "PlatformVersion"];

/// The inventory query operator that asks whether the attribute is reported at all, rather
/// than comparing its value (the modelled `InventoryQueryOperatorType` value `Exists`, the
/// one operator the node describes' `InstancePropertyFilterOperator` does not carry).
pub const OPERATOR_EXISTS: &str = "Exists";

/// The key an aggregated entry carries its count under, beside the attribute value it
/// counted: an aggregator "returns counts of inventory types based on one or more
/// expressions … you can see a count of how many Windows and Linux managed nodes exist in
/// your inventoried fleet" (`GetInventoryRequest$Aggregators`).
pub const AGGREGATION_COUNT_KEY: &str = "Count";

/// The prefix the count of the resources that do NOT match a group's criteria is reported
/// under: "Groups return a count of resources that match and don't match the specified
/// criteria" (`InventoryAggregator$Groups`), so a group of name `N` is reported as the
/// count under `N` beside the count under `NotN`.
pub const GROUP_COMPLEMENT_PREFIX: &str = "Not";

/// The `Id` of an entity that reports counts rather than one node: an aggregated count is
/// about the fleet, so it belongs to no node — "ID of the inventory result entity. For
/// example, for managed node inventory the result will be the managed node ID"
/// (`InventoryResultEntity$Id`), and a count is the inventory of no single node.
pub const AGGREGATED_ENTITY_ID: &str = "";

/// How this service states a filter key that names no attribute of an inventory type.
/// `InvalidFilter`'s own documentation: "The filter name isn't valid. Verify that you
/// entered the correct name and try again."
pub const INVALID_FILTER: &str =
    "The filter name isn't valid. Verify that you entered the correct name and try again.";

/// How this service states an aggregator it cannot count over: an expression it supports no
/// aggregation on, or an aggregator naming no one aggregation at all — neither an expression
/// nor groups, or both at once. `InvalidAggregatorException`'s own documentation: "The
/// specified aggregator isn't valid for the group type. Verify that the aggregator you
/// provided is supported."
pub const INVALID_AGGREGATOR: &str = "The specified aggregator isn't valid for the group type. \
                                      Verify that the aggregator you provided is supported.";

/// How many counted entries one aggregated read may report, and so how many a query may
/// ask it to count: the most an inventory item's content may carry
/// (`InventoryResultItem$Content` targets `InventoryItemEntryList`, `len 0..10000`), since
/// a count is reported as an entry of that content and an item cannot carry more.
///
/// Aggregating by groups MULTIPLIES: the entries of a nested aggregator are asked for once
/// per group of the aggregator that nests it, so a request that grows by one line asks for
/// a multiple of the entries, which is why how many a request asks for is counted before a
/// node is read and a query asking for more than an item may carry is refused.
pub const MAX_AGGREGATED_ENTRIES: usize = 10_000;

/// The lower bound the modelled `MaxResults` of an inventory query carries
/// (`GetInventoryRequest$MaxResults`, range 1..50).
pub const MIN_INVENTORY_PAGE_SIZE: i32 = 1;
/// The upper bound of the same member, and the page size a request naming no `MaxResults`
/// is served up to (see [`super::page_size`]).
pub const MAX_INVENTORY_PAGE_SIZE: i32 = 50;

/// The page size an inventory query serves, or the refusal message for a `MaxResults`
/// outside the modelled range — which the boundary validator already refuses, so this is a
/// backstop rather than the first line of defence.
pub fn inventory_page_size(max_results: Option<i32>) -> Result<usize, String> {
    super::page_size(
        max_results,
        MIN_INVENTORY_PAGE_SIZE,
        MAX_INVENTORY_PAGE_SIZE,
    )
}

/// The inventory type and the attribute of it a dotted name carries
/// (`AWS:InstanceInformation.PlatformType` → `("AWS:InstanceInformation",
/// "PlatformType")`), or `None` when the name is not that form.
///
/// A type name is `AWS:` or `Custom:` followed by the type itself
/// (`InventoryItemTypeName`, pattern `^(AWS|Custom):.*$`) and the attribute is what
/// follows the first `.` after it, so neither part may be empty.
pub fn split_qualified_attribute(name: &str) -> Option<(&str, &str)> {
    let (prefix, rest) = name.split_once(':')?;
    if !matches!(prefix, "AWS" | "Custom") {
        return None;
    }
    let (type_tail, attribute) = rest.split_once('.')?;
    (!type_tail.is_empty() && !attribute.is_empty())
        .then(|| (&name[..prefix.len() + 1 + type_tail.len()], attribute))
}

/// Whether `type_name` names an inventory type this service knows: one of the default
/// types it publishes, or a caller's own `Custom:` type.
pub fn is_known_inventory_type(type_name: &str) -> bool {
    DEFAULT_INVENTORY_TYPES.contains(&type_name) || type_name.starts_with(CUSTOM_TYPE_PREFIX)
}

/// Why an inventory filter key cannot be served, or `None` when it can.
///
/// A key has to name an attribute of an inventory type: the dotted form above, over a type
/// this service knows. For the one type whose schema this service holds, the attribute has
/// to be one of that schema's attributes; for a type it knows and holds no schema of, any
/// attribute name is a name that type may publish, so the filter is served — against the
/// nothing that type has collected — rather than refused.
pub fn filter_key_rejection(key: &str) -> Option<&'static str> {
    let Some((type_name, attribute)) = split_qualified_attribute(key) else {
        return Some(INVALID_FILTER);
    };
    if !is_known_inventory_type(type_name) {
        return Some(INVALID_FILTER);
    }
    let unknown_attribute = type_name == INVENTORY_TYPE_INSTANCE_INFORMATION
        && !INSTANCE_INFORMATION_ATTRIBUTES.contains(&attribute);
    unknown_attribute.then_some(INVALID_FILTER)
}

/// Why an aggregator expression cannot be counted over, or `None` when it can.
///
/// The expression is "the inventory type and attribute name for aggregation"
/// (`InventoryAggregator$Expression`), so it carries the same dotted form a filter key
/// does — and the attribute has to be one the type supports aggregation ON, which for the
/// one type whose aggregatable attributes this service knows is
/// [`INSTANCE_INFORMATION_AGGREGATABLE_ATTRIBUTES`].
pub fn aggregator_expression_rejection(expression: &str) -> Option<&'static str> {
    let Some((type_name, attribute)) = split_qualified_attribute(expression) else {
        return Some(INVALID_AGGREGATOR);
    };
    if !is_known_inventory_type(type_name) {
        return Some(INVALID_AGGREGATOR);
    }
    let not_aggregatable = type_name == INVENTORY_TYPE_INSTANCE_INFORMATION
        && !INSTANCE_INFORMATION_AGGREGATABLE_ATTRIBUTES.contains(&attribute);
    not_aggregatable.then_some(INVALID_AGGREGATOR)
}

/// The value `node` reports for one attribute of its `AWS:InstanceInformation` item, or
/// `None` when it reports none — either because the attribute is not one of that type's
/// ([`INSTANCE_INFORMATION_ATTRIBUTES`]) or because the node record holds no value for it.
///
/// Every value is a property of the registered node, so what an item reports is what the
/// node describes report: the item is a projection of the one node record rather than a
/// second copy of it. `InstanceStatus` is the node's own state
/// (`InstanceProperty$InstanceState`, "the current state of the node"), which is the
/// status `GetInventory` documents this type as carrying ("managed node status, such as
/// `Stopped` or `Terminated`").
pub fn instance_information_attribute<'a>(
    node: &'a ManagedNodeRecord,
    attribute: &str,
) -> Option<&'a str> {
    let properties = &node.properties;
    let value = match attribute {
        // Nothing records which agent implementation reported, so no node holds a value
        // for it (see [`ATTRIBUTE_AGENT_TYPE`]).
        ATTRIBUTE_AGENT_TYPE => &None,
        "AgentVersion" => &properties.agent_version,
        "ComputerName" => &properties.computer_name,
        "InstanceId" => &properties.instance_id,
        "InstanceStatus" => &properties.instance_state,
        "IpAddress" => &properties.ip_address,
        "PlatformName" => &properties.platform_name,
        "PlatformType" => &properties.platform_type,
        "PlatformVersion" => &properties.platform_version,
        "ResourceType" => &properties.resource_type,
        _ => return None,
    };
    value.as_deref()
}

/// The content of `node`'s `AWS:InstanceInformation` item: every attribute of that type
/// the node holds a value for, and no entry for one it does not — "Results include
/// attribute names and values" (`InventoryResultItem$Content`), and a node that has
/// reported no value for an attribute has no value to report.
pub fn instance_information_content(node: &ManagedNodeRecord) -> HashMap<String, String> {
    INSTANCE_INFORMATION_ATTRIBUTES
        .iter()
        .filter_map(|attribute| {
            instance_information_attribute(node, attribute)
                .map(|value| ((*attribute).to_string(), value.to_string()))
        })
        .collect()
}

/// When `node`'s `AWS:InstanceInformation` item was captured: the last time the node
/// reported to the service, and failing that the day it registered — "The time inventory
/// item data was captured" (`InventoryResultItem$CaptureTime`) is when the node last said
/// what it holds, so it comes from the node record rather than from the clock and two
/// reads of one node report the same instant.
///
/// Rendered as the modelled `InventoryItemCaptureTime` spells it (pattern
/// `^(20)[0-9][0-9]-…(Z)$`): whole seconds, so the instant is truncated to the second
/// rather than carrying the fraction a stored timestamp may hold.
pub fn capture_time(node: &ManagedNodeRecord) -> Option<String> {
    let stamped = node
        .properties
        .last_ping_date_time
        .or(node.properties.registration_date)?;
    ::aws_smithy_types::DateTime::from_secs(stamped.secs())
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
        .ok()
}

/// Whether `node` satisfies one inventory filter: the filter's key names an attribute of
/// an inventory type, and the node's value for it stands in the filter's relation to one
/// of the filter's values.
///
/// `Exists` asks only whether the attribute is reported, which is what makes it the one
/// operator a node answers without comparing anything; every other operator is the
/// comparison the node describes already make ([`super::operator_matches`]), so one rule
/// orders a version and compares a name wherever this service filters. A node that holds
/// no value for the attribute — including every node for an inventory type nothing has
/// collected — satisfies NO filter on it, whatever the operator: a filter selects the
/// nodes whose attribute satisfies it, and a node without the attribute has nothing to
/// satisfy it with.
pub fn matches_filter(
    node: &ManagedNodeRecord,
    key: &str,
    values: &[String],
    operator: Option<&str>,
) -> bool {
    let Some((type_name, attribute)) = split_qualified_attribute(key) else {
        return false;
    };
    if type_name != INVENTORY_TYPE_INSTANCE_INFORMATION {
        // A type this store holds no item of: the node reports no attribute of it.
        return false;
    }
    let Some(held) = instance_information_attribute(node, attribute) else {
        return false;
    };
    // The modelled operator defaults to `Equal`, the comparison a filter that names none
    // makes ("Equal" is the relation a key and a value state on their own).
    let operator = operator.unwrap_or(super::OPERATOR_EQUAL);
    if operator == OPERATOR_EXISTS {
        return true;
    }
    values
        .iter()
        .any(|wanted| super::operator_matches(operator, held, wanted))
}
