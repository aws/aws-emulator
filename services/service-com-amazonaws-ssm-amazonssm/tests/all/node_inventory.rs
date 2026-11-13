// SPDX-License-Identifier: CC0-1.0

//! `GetInventory` end to end: the Systems Manager Inventory of this Region's managed
//! nodes, narrowed by inventory filters, counted by aggregators and served one page at a
//! time.
//!
//! Every case goes through the front door a client uses — `POST /` with
//! `X-Amz-Target: AmazonSSM.GetInventory` and an `awsJson1_1` body, dispatched by the
//! multi-service router through the shared harness in `all/parameter_store.rs`.
//!
//! The inventory itself is the NODE STORE: a node's `AWS:InstanceInformation` item is the
//! projection of the record it registered with, so the cases seed nodes through the
//! service's own state (`state::ComAmazonawsSsmAmazonssmState::insert_managed_node`, the
//! store the node describes read too) because a node enters it only by REGISTERING and no
//! public API of this service registers one yet. Seeding is what lets these cases prove the
//! response is computed from persisted state: what is registered is what is reported, per
//! Region, and nothing else.
//!
//! The suites are tables: each row is one input along the axis the rule under test
//! generalises over (attribute, operator, value, value count, filter count, inventory type,
//! nesting, group, page size, Region), rather than the single filter and the single
//! aggregator the recorded units happened to send.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_ssm_amazonssm::state::{
    ComAmazonawsSsmAmazonssmState, ManagedNodeRecord, RESOURCE_TYPE_EC2_INSTANCE,
    RESOURCE_TYPE_MANAGED_INSTANCE,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::parameter_store::{call, call_in_region, error_code, error_message, router_with_engine};

/// The operation under test.
const OPERATION: &str = "GetInventory";

/// A second Region, to pin that a node's inventory is read only through the Region it
/// registered in. The first is the harness's own [`crate::parameter_store::REGION`].
const OTHER_REGION: &str = "us-east-1";

/// The one inventory type this service holds items of.
const TYPE: &str = "AWS:InstanceInformation";

/// An EC2 node id whose suffix is 8 characters long (the modelled `InstanceId` pattern
/// `^(^i-(\w{8}|\w{17})$)|(^mi-\w{17}$)$` admits both lengths).
const EC2_SHORT: &str = "i-0abc1234";

/// An EC2 node id whose suffix is 17 characters long (same pattern). Lexicographically the
/// FIRST of the three seeded ids, which is the order a page walks them in.
const EC2_LONG: &str = "i-0123456789abcdef0";

/// A hybrid node id: `mi-` and 17 characters (same pattern).
const HYBRID: &str = "mi-01234567890123456";

/// A node id that is well formed but names no registered node.
const UNREGISTERED: &str = "i-0dead999";

/// The instant every seeded node last reported, as a whole number of seconds.
const LAST_PING: Duration = Duration::from_secs(1_700_000_000);

/// [`LAST_PING`] as the modelled `InventoryItemCaptureTime` spells it (pattern
/// `^(20)[0-9][0-9]-(0[1-9]|1[012])-([12][0-9]|3[01]|0[1-9])(T)…(Z)$`).
const LAST_PING_RENDERED: &str = "2023-11-14T22:13:20Z";

/// A filter key over the one type this service holds items of.
fn key(attribute: &str) -> String {
    format!("{TYPE}.{attribute}")
}

/// One inventory filter, with the operator it names.
fn filter(attribute: &str, values: &[&str], operator: &str) -> Value {
    json!({ "Key": key(attribute), "Values": values, "Type": operator })
}

/// A router with just this service mounted, and the engine it was mounted on — the handle a
/// case seeds the node store through.
fn service() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    router_with_engine()
}

/// Register `record` in `region` the way a node's own registration would.
fn register(engine: &Arc<engine::InMemoryEngine>, region: &str, record: ManagedNodeRecord) {
    assert!(
        engine
            .state::<ComAmazonawsSsmAmazonssmState>()
            .lock()
            .insert_managed_node(region, record),
        "a seeded node must carry the node id it is stored under"
    );
}

/// How many nodes the store holds, to pin that a query wrote nothing.
fn registered_count(engine: &Arc<engine::InMemoryEngine>) -> usize {
    engine
        .state::<ComAmazonawsSsmAmazonssmState>()
        .lock()
        .managed_nodes
        .len()
}

/// Three nodes in the harness's Region, plus one in [`OTHER_REGION`], spanning node kind,
/// platform, platform version, agent version, node status, address and name — and one node
/// that has reported no agent version at all.
fn seed_fleet(engine: &Arc<engine::InMemoryEngine>) {
    let at = SystemTime::UNIX_EPOCH + LAST_PING;

    let mut linux = ManagedNodeRecord::registering(EC2_SHORT, RESOURCE_TYPE_EC2_INSTANCE, at);
    linux.properties.platform_type = Some("Linux".to_string());
    linux.properties.platform_name = Some("Amazon Linux".to_string());
    linux.properties.platform_version = Some("2023".to_string());
    linux.properties.agent_version = Some("3.2.1234.0".to_string());
    linux.properties.instance_state = Some("Active".to_string());
    linux.properties.ip_address = Some("10.0.1.17".to_string());
    linux.properties.computer_name = Some("ip-10-0-1-17.us-west-2.compute.internal".to_string());
    register(engine, crate::parameter_store::REGION, linux);

    let mut windows = ManagedNodeRecord::registering(EC2_LONG, RESOURCE_TYPE_EC2_INSTANCE, at);
    windows.properties.platform_type = Some("Windows".to_string());
    windows.properties.platform_name = Some("Microsoft Windows Server 2022 Datacenter".to_string());
    windows.properties.platform_version = Some("10.0.20348".to_string());
    windows.properties.agent_version = Some("3.1.1.0".to_string());
    windows.properties.instance_state = Some("Active".to_string());
    register(engine, crate::parameter_store::REGION, windows);

    // A hybrid node that has reported no agent version and is no longer running.
    let mut hybrid = ManagedNodeRecord::registering(HYBRID, RESOURCE_TYPE_MANAGED_INSTANCE, at);
    hybrid.properties.platform_type = Some("Linux".to_string());
    hybrid.properties.platform_name = Some("Ubuntu".to_string());
    hybrid.properties.platform_version = Some("22.04".to_string());
    hybrid.properties.instance_state = Some("Stopped".to_string());
    register(engine, crate::parameter_store::REGION, hybrid);

    // Same account, another Region: never reported through the harness's Region.
    let mut elsewhere =
        ManagedNodeRecord::registering("i-0eeeeeee", RESOURCE_TYPE_EC2_INSTANCE, at);
    elsewhere.properties.platform_type = Some("Linux".to_string());
    register(engine, OTHER_REGION, elsewhere);
}

/// The entities of a successful query.
fn entities(body: &Value) -> &Vec<Value> {
    body["Entities"]
        .as_array()
        .expect("a query carries Entities")
}

/// A query that must have succeeded, with its body.
async fn query(router: &MultiServiceRouter, case: &str, body: &Value) -> Value {
    let (status, parsed) = call(router, OPERATION, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with {parsed}"
    );
    parsed
}

/// The node ids a query reported, in the order it reported them.
async fn reported_ids(router: &MultiServiceRouter, case: &str, body: &Value) -> Vec<String> {
    let parsed = query(router, case, body).await;
    entities(&parsed)
        .iter()
        .map(|entity| {
            entity["Id"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: every entity carries an Id"))
                .to_string()
        })
        .collect()
}

/// The content entries of the one aggregated entity, each as an ordered map so a row is
/// compared whatever order the attributes serialise in.
async fn aggregated_content(
    router: &MultiServiceRouter,
    case: &str,
    body: &Value,
    type_name: &str,
) -> Vec<AggregatedRow> {
    let parsed = query(router, case, body).await;
    let reported = entities(&parsed);
    assert_eq!(
        reported.len(),
        1,
        "case {case:?}: counts are reported as one entity, got {parsed}"
    );
    assert_eq!(
        reported[0]["Id"].as_str(),
        Some(""),
        "case {case:?}: a count belongs to no node, got {parsed}"
    );
    let item = &reported[0]["Data"][type_name];
    assert_eq!(
        item["TypeName"].as_str(),
        Some(type_name),
        "case {case:?}: the item names the type it counted, got {parsed}"
    );
    item["Content"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: the item carries Content, got {parsed}"))
        .iter()
        .map(|entry| {
            entry
                .as_object()
                .unwrap_or_else(|| panic!("case {case:?}: every entry is a map, got {parsed}"))
                .iter()
                .map(|(name, value)| {
                    (
                        name.clone(),
                        value
                            .as_str()
                            .unwrap_or_else(|| {
                                panic!("case {case:?}: every entry value is a string, got {parsed}")
                            })
                            .to_string(),
                    )
                })
                .collect()
        })
        .collect()
}

/// One expected aggregated row.
fn row(pairs: &[(&str, &str)]) -> AggregatedRow {
    pairs
        .iter()
        .map(|(name, value)| ((*name).to_string(), (*value).to_string()))
        .collect()
}

/// One aggregated content entry, as an ordered map so a row is compared whatever order the
/// attributes serialise in.
type AggregatedRow = BTreeMap<String, String>;

/// Node ids in the order a query reports them (the store's node-id order).
fn sorted(ids: &[&str]) -> Vec<String> {
    let mut ids: Vec<String> = ids.iter().map(|id| (*id).to_string()).collect();
    ids.sort();
    ids
}

/// An `Aggregators` list of `levels` nested aggregators, each carrying `groups` groups: the
/// shape whose entries multiply with the nesting while the request grows by one line.
fn nested_groups(levels: usize, groups: usize) -> Value {
    let group_list: Vec<Value> = (0..groups)
        .map(|index| {
            json!({
                "Name": format!("Group{index}"),
                "Filters": [filter("PlatformType", &["Linux"], "Equal")],
            })
        })
        .collect();
    let mut aggregator = json!({ "Groups": group_list });
    for _ in 1..levels {
        aggregator = json!({ "Groups": group_list, "Aggregators": [aggregator] });
    }
    json!([aggregator])
}

#[tokio::test]
async fn a_node_is_reported_as_the_inventory_item_it_registered_with() {
    let (router, engine) = service();
    seed_fleet(&engine);

    let parsed = query(&router, "the default type over the seeded fleet", &json!({})).await;
    let reported = entities(&parsed);
    assert_eq!(
        reported.len(),
        3,
        "one entity per node of the Region, got {parsed}"
    );

    let hybrid = reported
        .iter()
        .find(|entity| entity["Id"].as_str() == Some(HYBRID))
        .unwrap_or_else(|| panic!("the hybrid node is reported, got {parsed}"));
    let item = &hybrid["Data"][TYPE];
    assert_eq!(
        item["TypeName"].as_str(),
        Some(TYPE),
        "the item names its own type, got {parsed}"
    );
    assert_eq!(
        item["SchemaVersion"].as_str(),
        Some("1.0"),
        "the item carries the schema version of its type, got {parsed}"
    );
    assert_eq!(
        item["CaptureTime"].as_str(),
        Some(LAST_PING_RENDERED),
        "the item was captured when the node last reported, got {parsed}"
    );

    // Every attribute of the type the seeded record holds a value for, and no entry for
    // one it does not: this node reported no agent version, no address and no computer
    // name, and nothing records which agent implementation reported.
    let content = item["Content"]
        .as_array()
        .unwrap_or_else(|| panic!("the item carries Content, got {parsed}"));
    assert_eq!(content.len(), 1, "one entry per item, got {parsed}");
    let expected = json!({
        "InstanceId": HYBRID,
        "InstanceStatus": "Stopped",
        "PlatformName": "Ubuntu",
        "PlatformType": "Linux",
        "PlatformVersion": "22.04",
        "ResourceType": RESOURCE_TYPE_MANAGED_INSTANCE,
    });
    assert_eq!(
        content[0], expected,
        "the entry is the projection of the registered record, got {}",
        content[0]
    );
}

#[tokio::test]
async fn a_query_reports_only_the_nodes_of_the_region_it_was_signed_for_and_writes_nothing() {
    let (router, engine) = service();
    seed_fleet(&engine);
    let before = registered_count(&engine);

    let ids = reported_ids(&router, "the signing Region", &json!({})).await;
    assert_eq!(
        ids,
        sorted(&[EC2_SHORT, EC2_LONG, HYBRID]),
        "only the nodes registered in this Region are reported"
    );

    let (status, parsed) = call_in_region(&router, OTHER_REGION, OPERATION, &json!({})).await;
    assert_eq!(status, 200, "the other Region answers too, got {parsed}");
    let elsewhere: Vec<&str> = entities(&parsed)
        .iter()
        .filter_map(|entity| entity["Id"].as_str())
        .collect();
    assert_eq!(
        elsewhere,
        vec!["i-0eeeeeee"],
        "the other Region reports its own node alone, got {parsed}"
    );

    // Reading twice reports the same thing, and the store holds what it held: a query
    // must not change what a later caller sees.
    let again = reported_ids(&router, "the same query again", &json!({})).await;
    assert_eq!(again, ids, "a second query reports what the first did");
    assert_eq!(
        registered_count(&engine),
        before,
        "a query registers no node: expected {before} nodes"
    );
}

#[tokio::test]
async fn a_filter_selects_the_nodes_whose_attribute_satisfies_it() {
    let (router, engine) = service();
    seed_fleet(&engine);

    // (case, Filters, the node ids the query reports)
    let cases: Vec<(&str, Value, Vec<String>)> = vec![
        (
            "Equal on the platform type",
            json!([filter("PlatformType", &["Linux"], "Equal")]),
            sorted(&[EC2_SHORT, HYBRID]),
        ),
        (
            "a filter naming no operator compares whole values",
            json!([{ "Key": key("PlatformType"), "Values": ["Windows"] }]),
            sorted(&[EC2_LONG]),
        ),
        (
            "NotEqual on the same attribute",
            json!([filter("PlatformType", &["Linux"], "NotEqual")]),
            sorted(&[EC2_LONG]),
        ),
        (
            "BeginWith on the platform name",
            json!([filter("PlatformName", &["Microsoft"], "BeginWith")]),
            sorted(&[EC2_LONG]),
        ),
        (
            "GreaterThan orders the agent version segment by segment",
            json!([filter("AgentVersion", &["3.2.0.0"], "GreaterThan")]),
            sorted(&[EC2_SHORT]),
        ),
        (
            "LessThan on the same attribute",
            json!([filter("AgentVersion", &["3.2.0.0"], "LessThan")]),
            sorted(&[EC2_LONG]),
        ),
        (
            "Exists asks only whether the attribute is reported",
            json!([filter("AgentVersion", &["ignored"], "Exists")]),
            sorted(&[EC2_SHORT, EC2_LONG]),
        ),
        (
            "Exists on an attribute no node reports",
            json!([filter("AgentType", &["amazon-ssm-agent"], "Exists")]),
            Vec::new(),
        ),
        (
            "two values union",
            json!([filter("PlatformType", &["Linux", "Windows"], "Equal")]),
            sorted(&[EC2_SHORT, EC2_LONG, HYBRID]),
        ),
        (
            "two filters intersect",
            json!([
                filter("PlatformType", &["Linux"], "Equal"),
                filter("ResourceType", &[RESOURCE_TYPE_EC2_INSTANCE], "Equal"),
            ]),
            sorted(&[EC2_SHORT]),
        ),
        (
            "the node status this type carries",
            json!([filter("InstanceStatus", &["Stopped"], "Equal")]),
            sorted(&[HYBRID]),
        ),
        (
            "the node's own id",
            json!([filter("InstanceId", &[EC2_LONG], "Equal")]),
            sorted(&[EC2_LONG]),
        ),
        (
            "a well-formed id no node registered under",
            json!([filter("InstanceId", &[UNREGISTERED], "Equal")]),
            Vec::new(),
        ),
        (
            "an address and a computer name one node alone reports",
            json!([
                filter("IpAddress", &["10.0.1.17"], "Equal"),
                filter("ComputerName", &["ip-10-0-1"], "BeginWith"),
            ]),
            sorted(&[EC2_SHORT]),
        ),
        (
            // A default inventory type this service knows and holds no item of
            // (`InventoryItemSchema$TypeName`): nothing has been collected for it.
            "an attribute of a type nothing has been collected for",
            json!([{
                "Key": "AWS:Application.Name",
                "Values": ["bash"],
                "Type": "Equal",
            }]),
            Vec::new(),
        ),
        (
            "an attribute of a caller's own type",
            json!([{ "Key": "Custom:Fleet.Owner", "Values": ["team"], "Type": "Equal" }]),
            Vec::new(),
        ),
    ];

    for (case, filters, expected) in cases {
        let ids = reported_ids(&router, case, &json!({ "Filters": filters })).await;
        assert_eq!(
            ids, expected,
            "case {case:?}: expected {expected:?}, got {ids:?}"
        );
    }
}

#[tokio::test]
async fn a_filter_key_that_names_no_attribute_of_an_inventory_type_is_invalid_filter() {
    let (router, engine) = service();
    seed_fleet(&engine);

    /// `InvalidFilter`'s own documentation, which is how the service states the refusal.
    const STATED: &str =
        "The filter name isn't valid. Verify that you entered the correct name and try again.";

    // (case, the Filters key)
    let cases: Vec<(&str, &str)> = vec![
        ("an attribute name naming no type", "PlatformType"),
        ("a type naming no attribute", TYPE),
        ("a type with an empty attribute", "AWS:InstanceInformation."),
        (
            "an attribute outside the schema of the type",
            "AWS:InstanceInformation.NotAnAttribute",
        ),
        ("a type neither Amazon's nor a caller's", "Fleet.Owner"),
        (
            "an Amazon type this service does not publish",
            "AWS:NotAnInventoryType.Name",
        ),
    ];

    for (case, bad_key) in cases {
        let body = json!({ "Filters": [{ "Key": bad_key, "Values": ["x"], "Type": "Equal" }] });
        let (status, parsed) = call(&router, OPERATION, &body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: the key is refused, got {status} with {parsed}"
        );
        let code = error_code(&parsed);
        assert_eq!(
            code, "InvalidFilter",
            "case {case:?}: expected InvalidFilter, got {code:?} with {parsed}"
        );
        let stated = error_message(&parsed);
        assert_eq!(
            stated, STATED,
            "case {case:?}: expected {STATED:?}, got {stated:?}"
        );
    }
}

#[tokio::test]
async fn an_aggregator_counts_the_nodes_per_value_of_the_attribute_it_names() {
    let (router, engine) = service();
    seed_fleet(&engine);

    // One row per attribute `GetInventorySchemaRequest$Aggregator` documents this type as
    // supporting aggregation on, each with the counts the seeded fleet holds.
    let cases: Vec<(&str, &str, Vec<AggregatedRow>)> = vec![
        (
            "the platform type",
            "PlatformType",
            vec![
                row(&[("PlatformType", "Linux"), ("Count", "2")]),
                row(&[("PlatformType", "Windows"), ("Count", "1")]),
            ],
        ),
        (
            "the platform name",
            "PlatformName",
            vec![
                row(&[("PlatformName", "Amazon Linux"), ("Count", "1")]),
                row(&[
                    ("PlatformName", "Microsoft Windows Server 2022 Datacenter"),
                    ("Count", "1"),
                ]),
                row(&[("PlatformName", "Ubuntu"), ("Count", "1")]),
            ],
        ),
        (
            "the platform version",
            "PlatformVersion",
            vec![
                row(&[("PlatformVersion", "10.0.20348"), ("Count", "1")]),
                row(&[("PlatformVersion", "2023"), ("Count", "1")]),
                row(&[("PlatformVersion", "22.04"), ("Count", "1")]),
            ],
        ),
    ];

    for (case, attribute, expected) in cases {
        let body = json!({ "Aggregators": [{ "Expression": key(attribute) }] });
        let content = aggregated_content(&router, case, &body, TYPE).await;
        assert_eq!(
            content, expected,
            "case {case:?}: expected {expected:?}, got {content:?}"
        );
    }

    // The filters narrow what is counted: aggregation reports counts of the selected
    // nodes, not of the fleet.
    let body = json!({
        "Filters": [filter("ResourceType", &[RESOURCE_TYPE_EC2_INSTANCE], "Equal")],
        "Aggregators": [{ "Expression": key("PlatformType") }],
    });
    let content = aggregated_content(&router, "counted over the filtered nodes", &body, TYPE).await;
    let expected = vec![
        row(&[("PlatformType", "Linux"), ("Count", "1")]),
        row(&[("PlatformType", "Windows"), ("Count", "1")]),
    ];
    assert_eq!(
        content, expected,
        "a filtered aggregation counts the selected nodes: expected {expected:?}, got \
         {content:?}"
    );
}

#[tokio::test]
async fn a_nested_aggregator_refines_each_count_and_a_group_counts_both_sides() {
    let (router, engine) = service();
    seed_fleet(&engine);

    let nested = json!({
        "Aggregators": [{
            "Expression": key("PlatformType"),
            "Aggregators": [{ "Expression": key("PlatformName") }],
        }],
    });
    let content = aggregated_content(&router, "a nested aggregator", &nested, TYPE).await;
    let expected = vec![
        row(&[
            ("PlatformType", "Linux"),
            ("PlatformName", "Amazon Linux"),
            ("Count", "1"),
        ]),
        row(&[
            ("PlatformType", "Linux"),
            ("PlatformName", "Ubuntu"),
            ("Count", "1"),
        ]),
        row(&[
            ("PlatformType", "Windows"),
            ("PlatformName", "Microsoft Windows Server 2022 Datacenter"),
            ("Count", "1"),
        ]),
    ];
    assert_eq!(
        content, expected,
        "a nested aggregator reports the refined counts: expected {expected:?}, got \
         {content:?}"
    );

    // "Groups return a count of resources that match and don't match the specified
    // criteria" (`InventoryAggregator$Groups`), over rows differing in how many of the
    // three nodes the criteria match and in how many filters the criteria carry.
    let cases: Vec<(&str, Value, Vec<AggregatedRow>)> = vec![
        (
            "a group two of the three nodes match",
            json!([{
                "Groups": [{
                    "Name": "Linux",
                    "Filters": [filter("PlatformType", &["Linux"], "Equal")],
                }],
            }]),
            vec![row(&[("Linux", "2"), ("NotLinux", "1")])],
        ),
        (
            "two groups, each counted over the same fleet",
            json!([{
                "Groups": [
                    {
                        "Name": "Running",
                        "Filters": [filter("InstanceStatus", &["Active"], "Equal")],
                    },
                    {
                        "Name": "Hybrid",
                        "Filters": [filter(
                            "ResourceType",
                            &[RESOURCE_TYPE_MANAGED_INSTANCE],
                            "Equal",
                        )],
                    },
                ],
            }]),
            vec![
                row(&[("Running", "2"), ("NotRunning", "1")]),
                row(&[("Hybrid", "1"), ("NotHybrid", "2")]),
            ],
        ),
        (
            "a group whose two criteria no node satisfies together",
            json!([{
                "Groups": [{
                    "Name": "StoppedWindows",
                    "Filters": [
                        filter("InstanceStatus", &["Stopped"], "Equal"),
                        filter("PlatformType", &["Windows"], "Equal"),
                    ],
                }],
            }]),
            vec![row(&[("StoppedWindows", "0"), ("NotStoppedWindows", "3")])],
        ),
        (
            "a group refined by a nested group, over the nodes it matched",
            json!([{
                "Groups": [{
                    "Name": "Linux",
                    "Filters": [filter("PlatformType", &["Linux"], "Equal")],
                }],
                "Aggregators": [{
                    "Groups": [{
                        "Name": "Running",
                        "Filters": [filter("InstanceStatus", &["Active"], "Equal")],
                    }],
                }],
            }]),
            vec![row(&[
                ("Linux", "2"),
                ("NotLinux", "1"),
                ("Running", "1"),
                ("NotRunning", "1"),
            ])],
        ),
        (
            "a group that matched nothing, which leaves its nested aggregator nothing to \
             refine",
            json!([{
                "Groups": [{
                    "Name": "Macs",
                    "Filters": [filter("PlatformType", &["MacOS"], "Equal")],
                }],
                "Aggregators": [{ "Expression": key("PlatformName") }],
            }]),
            vec![row(&[("Macs", "0"), ("NotMacs", "3")])],
        ),
    ];

    for (case, aggregators, expected) in cases {
        let body = json!({ "Aggregators": aggregators });
        let content = aggregated_content(&router, case, &body, TYPE).await;
        assert_eq!(
            content, expected,
            "case {case:?}: expected {expected:?}, got {content:?}"
        );
    }
}

#[tokio::test]
async fn an_aggregator_the_service_cannot_count_over_is_invalid_aggregator() {
    let (router, engine) = service();
    seed_fleet(&engine);

    /// `InvalidAggregatorException`'s own documentation.
    const STATED: &str = "The specified aggregator isn't valid for the group type. Verify that \
                          the aggregator you provided is supported.";

    // (case, the Aggregators list). Every attribute of this type outside the three
    // `GetInventorySchemaRequest$Aggregator` documents it as aggregating on is refused,
    // whichever of them it is and however deeply nested the aggregator sits.
    let cases: Vec<(&str, Value)> = vec![
        (
            "the node status, which the type supports no aggregation on",
            json!([{ "Expression": key("InstanceStatus") }]),
        ),
        (
            "the node id, the same",
            json!([{ "Expression": key("InstanceId") }]),
        ),
        (
            "the agent version, the same",
            json!([{ "Expression": key("AgentVersion") }]),
        ),
        (
            "an attribute outside the schema of the type",
            json!([{ "Expression": key("NotAnAttribute") }]),
        ),
        (
            "an attribute name naming no type",
            json!([{ "Expression": "PlatformType" }]),
        ),
        (
            "a type naming no attribute",
            json!([{ "Expression": TYPE }]),
        ),
        (
            "an Amazon type this service does not publish",
            json!([{ "Expression": "AWS:NotAnInventoryType.Name" }]),
        ),
        (
            "an aggregator naming neither an expression nor groups",
            json!([{}]),
        ),
        (
            "an aggregator naming both",
            json!([{
                "Expression": key("PlatformType"),
                "Groups": [{
                    "Name": "Linux",
                    "Filters": [filter("PlatformType", &["Linux"], "Equal")],
                }],
            }]),
        ),
        (
            "a nested aggregator over an attribute the type supports no aggregation on",
            json!([{
                "Expression": key("PlatformType"),
                "Aggregators": [{ "Expression": key("InstanceStatus") }],
            }]),
        ),
        (
            "a second aggregator of the list, where the first is countable",
            json!([
                { "Expression": key("PlatformName") },
                { "Expression": key("ComputerName") },
            ]),
        ),
        (
            "a nesting deeper than this service walks",
            nested_groups(11, 1),
        ),
        (
            // Each group is one entry of the result and a nested list is asked for once
            // per group, so five levels of ten groups ask for 10^5 entries where an
            // inventory item's content may carry 10,000
            // (`InventoryItemEntryList`, len 0..10000).
            "an aggregation whose groups multiply into more entries than an item carries",
            nested_groups(5, 10),
        ),
    ];

    for (case, aggregators) in cases {
        let body = json!({ "Aggregators": aggregators });
        let (status, parsed) = call(&router, OPERATION, &body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: the aggregator is refused, got {status} with {parsed}"
        );
        let code = error_code(&parsed);
        assert_eq!(
            code, "InvalidAggregatorException",
            "case {case:?}: expected InvalidAggregatorException, got {code:?} with {parsed}"
        );
        let stated = error_message(&parsed);
        assert_eq!(
            stated, STATED,
            "case {case:?}: expected {STATED:?}, got {stated:?}"
        );
    }

    // A group's filter keys are filter keys, held to the same rule the request's own are.
    let body = json!({
        "Aggregators": [{
            "Groups": [{
                "Name": "Linux",
                "Filters": [{ "Key": "PlatformType", "Values": ["Linux"], "Type": "Equal" }],
            }],
        }],
    });
    let (status, parsed) = call(&router, OPERATION, &body).await;
    assert_eq!(
        status, 400,
        "a group filter key naming no type is refused, got {status} with {parsed}"
    );
    let code = error_code(&parsed);
    assert_eq!(
        code, "InvalidFilter",
        "a group filter key is refused as a filter key: got {code:?} with {parsed}"
    );
}

#[tokio::test]
async fn the_reported_type_is_the_one_the_result_attributes_name() {
    let (router, engine) = service();
    seed_fleet(&engine);

    // (case, ResultAttributes, how many entities the query reports)
    let cases: Vec<(&str, Value, usize)> = vec![
        (
            "the default type, named explicitly",
            json!([{ "TypeName": TYPE }]),
            3,
        ),
        (
            // A type this service knows and holds no item of: `PutInventory`, the
            // operation a caller writes an item with, is still a scaffold.
            "a default Amazon type nothing has been collected for",
            json!([{ "TypeName": "AWS:Network" }]),
            0,
        ),
        (
            "a caller's own type nothing has been collected for",
            json!([{ "TypeName": "Custom:Fleet" }]),
            0,
        ),
    ];

    for (case, result_attributes, expected) in cases {
        let parsed = query(
            &router,
            case,
            &json!({ "ResultAttributes": result_attributes }),
        )
        .await;
        let reported = entities(&parsed).len();
        assert_eq!(
            reported, expected,
            "case {case:?}: expected {expected} entities, got {reported} in {parsed}"
        );
    }

    // A type name that names no inventory type at all is the modelled refusal of that
    // member.
    let body = json!({ "ResultAttributes": [{ "TypeName": "AWS:NotAnInventoryType" }] });
    let (status, parsed) = call(&router, OPERATION, &body).await;
    assert_eq!(
        status, 400,
        "a type name naming no inventory type is refused, got {status} with {parsed}"
    );
    let code = error_code(&parsed);
    assert_eq!(
        code, "InvalidResultAttributeException",
        "expected InvalidResultAttributeException, got {code:?} with {parsed}"
    );
    let stated = error_message(&parsed);
    assert_eq!(
        stated, "The specified inventory item result attribute isn't valid.",
        "the refusal is stated as the modelled error documents it, got {stated:?}"
    );
}

#[tokio::test]
async fn a_page_holds_max_results_entities_and_resumes_at_the_node_it_stopped_at() {
    let (router, engine) = service();
    seed_fleet(&engine);

    // One row per page size, from the modelled minimum to a size past the fleet: the pages
    // a walk of the whole fleet is served in, and the cursor each page hands out.
    let cases: Vec<(&str, i64, Vec<Vec<String>>)> = vec![
        (
            "the modelled minimum page size",
            1,
            vec![
                sorted(&[EC2_LONG]),
                sorted(&[EC2_SHORT]),
                sorted(&[HYBRID]),
            ],
        ),
        (
            "two nodes a page",
            2,
            vec![sorted(&[EC2_LONG, EC2_SHORT]), sorted(&[HYBRID])],
        ),
        (
            "a page size past the fleet",
            50,
            vec![sorted(&[EC2_LONG, EC2_SHORT, HYBRID])],
        ),
    ];

    for (case, max_results, pages) in cases {
        let mut token: Option<String> = None;
        for (index, expected) in pages.iter().enumerate() {
            let mut body = json!({ "MaxResults": max_results });
            if let Some(token) = &token {
                body["NextToken"] = json!(token);
            }
            let parsed = query(&router, case, &body).await;
            let ids: Vec<String> = entities(&parsed)
                .iter()
                .filter_map(|entity| entity["Id"].as_str().map(str::to_string))
                .collect();
            assert_eq!(
                &ids, expected,
                "case {case:?} page {index}: expected {expected:?}, got {ids:?}"
            );
            token = parsed["NextToken"].as_str().map(str::to_string);
            let last = index + 1 == pages.len();
            assert_eq!(
                token.is_none(),
                last,
                "case {case:?} page {index}: a cursor is handed out only while nodes are \
                 waiting, got {parsed}"
            );
        }
    }

    // A token this service did not issue is the modelled refusal.
    let body = json!({ "NextToken": "not-a-cursor-this-service-issued" });
    let (status, parsed) = call(&router, OPERATION, &body).await;
    assert_eq!(
        status, 400,
        "a foreign cursor is refused, got {status} with {parsed}"
    );
    let code = error_code(&parsed);
    assert_eq!(
        code, "InvalidNextToken",
        "expected InvalidNextToken, got {code:?} with {parsed}"
    );
}

#[tokio::test]
async fn an_account_that_has_registered_no_node_holds_no_inventory() {
    let (router, _engine) = service();

    // Nothing is seeded: a query invents no node and no item, and an aggregation over an
    // empty fleet counts nothing at all.
    let parsed = query(&router, "an empty store", &json!({})).await;
    assert!(
        entities(&parsed).is_empty(),
        "an account with no registered node holds no inventory, got {parsed}"
    );

    let body = json!({ "Aggregators": [{ "Expression": key("PlatformType") }] });
    let parsed = query(&router, "an aggregation over an empty fleet", &body).await;
    assert!(
        entities(&parsed).is_empty(),
        "a fleet that reports no value of the attribute is counted as nothing, got {parsed}"
    );
}
