// SPDX-License-Identifier: CC0-1.0

//! The node configurations this service OFFERS for an action, and the filters a
//! request narrows them with.
//!
//! `DescribeNodeConfigurationOptions` "returns properties of possible node
//! configurations such as node type, number of nodes, and disk usage. The
//! orderable options for the node configurations are returned for the action type
//! specified" (`smithy_query(query="doc",
//! operation="DescribeNodeConfigurationOptions")`), and its `ActionType` says what
//! the options are evaluated FOR: "restore-cluster to get configuration
//! combinations based on an existing snapshot", "recommend-node-config to get
//! configuration recommendations based on an existing cluster or snapshot" and
//! "resize-cluster to get configuration combinations for elastic resize based on
//! an existing cluster".
//!
//! So an option is not a record anything creates: it is COMPUTED from the shape of
//! the source the request names and from the catalog of orderable shapes this
//! service already publishes ([`ORDERABLE_NODE_TYPES`],
//! [`orderable_cluster_types`], [`MIN_MULTI_NODE_NODES`]) — the same catalog
//! `DescribeOrderableClusterOptions` reports and a create is admitted against, so
//! a configuration this operation offers is one a cluster can really have.

use super::*;
use crate::ops::types::{NodeConfigurationOption, NodeConfigurationOptionsFilter};

/// `ActionType` of the options evaluated for a restore of an existing snapshot.
///
/// Source: `smithy_query(query="shape", shape="ActionType")` — the enum's values
/// are `restore-cluster`, `recommend-node-config` and `resize-cluster`.
pub const ACTION_RESTORE_CLUSTER: &str = "restore-cluster";
/// `ActionType` of the options recommended for an existing cluster or snapshot.
/// See [`ACTION_RESTORE_CLUSTER`].
pub const ACTION_RECOMMEND_NODE_CONFIG: &str = "recommend-node-config";
/// `ActionType` of the options an elastic resize of an existing cluster can move
/// onto. See [`ACTION_RESTORE_CLUSTER`].
pub const ACTION_RESIZE_CLUSTER: &str = "resize-cluster";

/// `NodeConfigurationOption$Mode` every configuration this service offers
/// carries.
///
/// Source: `smithy_query(query="shape", shape="Mode")` — the enum's values are
/// `standard` and `high-performance`. Every shape this service's orderable
/// catalog publishes is a standard configuration; nothing in this emulator
/// provisions the other, so no option claims it.
pub const MODE_STANDARD: &str = "standard";

/// The filter names `NodeConfigurationOptionsFilter$Name` admits.
///
/// Source: `smithy_query(query="shape",
/// shape="NodeConfigurationOptionsFilterName")` — the enum's values are
/// `NodeType`, `NumberOfNodes`, `EstimatedDiskUtilizationPercent` and `Mode`.
pub const FILTER_NODE_TYPE: &str = "NodeType";
/// See [`FILTER_NODE_TYPE`].
pub const FILTER_NUMBER_OF_NODES: &str = "NumberOfNodes";
/// See [`FILTER_NODE_TYPE`].
pub const FILTER_DISK_UTILIZATION: &str = "EstimatedDiskUtilizationPercent";
/// See [`FILTER_NODE_TYPE`].
pub const FILTER_MODE: &str = "Mode";

/// The `MaxRecords` window `DescribeNodeConfigurationOptions` publishes, which is
/// NOT the one every other paginated read of this service publishes.
///
/// Source: `smithy_query(query="doc",
/// shape="DescribeNodeConfigurationOptionsMessage", member="MaxRecords")` —
/// "Default: 500 Constraints: minimum 100, maximum 500." Only the minimum is a
/// refusal and the maximum caps the page served, exactly as on every other window
/// ([`page_bounds_within`]).
pub const NODE_CONFIGURATION_MAX_RECORDS_WINDOW: MaxRecordsWindow = MaxRecordsWindow {
    default: 500,
    min: 100,
    max: 500,
};

/// Why an action whose source the request did not name is refused, naming the
/// action and the member it needs.
///
/// The rule is the `ActionType`'s own documentation: "restore-cluster to get
/// configuration combinations based on an existing snapshot",
/// "recommend-node-config … based on an existing cluster or snapshot" and
/// "resize-cluster to get configuration combinations for elastic resize based on
/// an existing cluster" — so each action has a source it is evaluated against,
/// and a request that names none describes nothing to evaluate.
pub fn node_configuration_source_required_message(action: &str) -> String {
    let needed = match action {
        ACTION_RESTORE_CLUSTER => "SnapshotIdentifier or SnapshotArn",
        ACTION_RESIZE_CLUSTER => "ClusterIdentifier",
        _ => "ClusterIdentifier, SnapshotIdentifier or SnapshotArn",
    };
    format!(
        "{needed} must be specified to evaluate node configuration options for ActionType \
         {action}."
    )
}

/// The shape of the SOURCE an action's options are evaluated for: the cluster or
/// the snapshot the request named.
///
/// Both are the same thing to this computation — a node type and a node count —
/// which is why one value serves the three action types.
#[derive(Debug, Clone)]
pub struct NodeConfigurationSource {
    /// The node type the cluster runs on, or the snapshot was taken of.
    pub node_type: String,
    /// How many nodes it runs on, which is the count an elastic resize is
    /// measured against.
    pub number_of_nodes: i32,
}

/// The node configurations this service offers for `action`, given the shape of
/// the source the request named, in catalog order.
///
/// The options are the ORDERABLE shapes, so each one is a configuration a cluster
/// of this emulator can really have:
///
/// * for an elastic resize ([`ACTION_RESIZE_CLUSTER`]) the node type is the
///   source's own — "the type of nodes that you add must match the node type for
///   the cluster" (`smithy_query(query="doc", operation="ResizeCluster")`) — and
///   the counts are the ones that type is orderable at;
/// * for a restore or a recommendation every node type a cluster is orderable
///   with is offered, because a restore creates a new cluster of any of them.
///
/// `EstimatedDiskUtilizationPercent` is deliberately not reported: it is a fact
/// about the DISK the restored data occupies, and this control plane stores no
/// disk measurement to compute it from — so no option claims a utilisation
/// nothing measured.
pub fn node_configuration_options(
    action: &str,
    source: &NodeConfigurationSource,
) -> Vec<NodeConfigurationOption> {
    let elastic = action == ACTION_RESIZE_CLUSTER;
    ORDERABLE_NODE_TYPES
        .iter()
        .filter(|node_type| {
            if elastic {
                **node_type == source.node_type
            } else {
                !orderable_cluster_types(node_type).is_empty()
            }
        })
        .flat_map(|node_type| {
            offered_node_counts(node_type, source.number_of_nodes)
                .into_iter()
                .map(move |number_of_nodes| NodeConfigurationOption {
                    node_type: Some((*node_type).to_string()),
                    number_of_nodes: Some(number_of_nodes),
                    estimated_disk_utilization_percent: None,
                    mode: Some(MODE_STANDARD.to_string()),
                })
        })
        .collect()
}

/// The node counts `node_type` is offered at for a source running on
/// `source_nodes` nodes, in ascending order.
///
/// The counts come from the published catalog and from the source itself: the
/// one-node cluster where the catalog offers a `single-node` shape of that type,
/// the smallest multi-node cluster ([`MIN_MULTI_NODE_NODES`]), and the source's
/// own count where it is larger than that — so a cluster is always offered the
/// shape it already has, which is what makes the list usable for an elastic
/// resize.
fn offered_node_counts(node_type: &str, source_nodes: i32) -> Vec<i32> {
    let mut counts: Vec<i32> = Vec::new();
    if orderable_cluster_types(node_type).contains(&CLUSTER_TYPE_SINGLE_NODE) {
        counts.push(1);
    }
    if orderable_cluster_types(node_type).contains(&CLUSTER_TYPE_MULTI_NODE) {
        counts.push(MIN_MULTI_NODE_NODES);
        if source_nodes > MIN_MULTI_NODE_NODES && !exceeds_nodes_per_cluster_limit(source_nodes) {
            counts.push(source_nodes);
        }
    }
    counts.dedup();
    counts
}

/// Why a filter this service cannot evaluate is refused, naming the member.
///
/// The model admits four filter names and seven operators, and the VALUES are
/// bounded by nothing at all, so a filter whose values cannot be read as the
/// property it names is refused rather than silently matching everything.
pub fn node_configuration_filter_message(name: &str, operator: &str) -> String {
    format!(
        "Invalid value for Filter: {name} cannot be compared with operator {operator} against \
         the values given."
    )
}

/// Whether `option` satisfies every filter the request carried.
///
/// Each filter "compare[s] Name using Operator to Values"
/// (`smithy_query(query="doc", shape="NodeConfigurationOptionsFilter",
/// member="Values")`), so the filters narrow the SAME list and an option has to
/// satisfy all of them. A filter naming the disk utilisation matches nothing,
/// because no option this service offers reports one (see
/// [`node_configuration_options`]).
pub fn option_matches_filters(
    option: &NodeConfigurationOption,
    filters: &[NodeConfigurationOptionsFilter],
) -> Result<bool, String> {
    for filter in filters {
        let name = filter.name.clone().unwrap_or_default();
        let operator = filter.operator.clone().unwrap_or_default();
        let values = filter.values.clone().unwrap_or_default();
        let matched = match name.as_str() {
            FILTER_NODE_TYPE => text_matches(
                option.node_type.as_deref().unwrap_or_default(),
                &operator,
                &values,
            ),
            FILTER_MODE => text_matches(
                option.mode.as_deref().unwrap_or_default(),
                &operator,
                &values,
            ),
            FILTER_NUMBER_OF_NODES => {
                number_matches(f64::from(option.number_of_nodes.unwrap_or_default()), &operator, &values)?
            }
            FILTER_DISK_UTILIZATION => match option.estimated_disk_utilization_percent {
                Some(percent) => number_matches(percent, &operator, &values)?,
                None => false,
            },
            // A name outside the modelled enum never reaches here: the boundary
            // validator refuses a value outside a modelled constraint before a
            // handler runs.
            _ => true,
        };
        if !matched {
            return Ok(false);
        }
    }
    Ok(true)
}

/// Whether `held` satisfies one filter over a TEXT property.
///
/// "If filter Name is NodeType only the 'in' operator is supported. Provide one
/// value to evaluate for 'eq', 'lt', 'le', 'gt', and 'ge'"
/// (`smithy_query(query="doc", shape="NodeConfigurationOptionsFilter",
/// member="Operator")`): a text property is a name, so only membership and
/// equality order it.
fn text_matches(held: &str, operator: &str, values: &[String]) -> bool {
    match operator {
        "eq" => values.first().is_some_and(|wanted| wanted == held),
        "in" => values.iter().any(|wanted| wanted == held),
        // An ordering operator over a name has no meaning, so nothing is offered
        // for it rather than every option being.
        _ => false,
    }
}

/// Whether `held` satisfies one filter over a NUMERIC property, or why the
/// filter's values cannot be read as numbers.
fn number_matches(held: f64, operator: &str, values: &[String]) -> Result<bool, String> {
    let parsed: Vec<f64> = values
        .iter()
        .map(|value| {
            value.trim().parse::<f64>().map_err(|_| {
                node_configuration_filter_message(FILTER_NUMBER_OF_NODES, operator)
            })
        })
        .collect::<Result<_, _>>()?;
    let first = parsed.first().copied();
    Ok(match operator {
        "eq" => first.is_some_and(|wanted| held == wanted),
        "lt" => first.is_some_and(|wanted| held < wanted),
        "le" => first.is_some_and(|wanted| held <= wanted),
        "gt" => first.is_some_and(|wanted| held > wanted),
        "ge" => first.is_some_and(|wanted| held >= wanted),
        "in" => parsed.contains(&held),
        // "between" compares against the pair the values name, so a filter that
        // carries anything else than two of them describes no range.
        "between" => match parsed.as_slice() {
            [low, high] => held >= *low && held <= *high,
            _ => false,
        },
        _ => false,
    })
}
