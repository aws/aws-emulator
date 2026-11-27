// SPDX-License-Identifier: CC0-1.0

//! Which attributes of an inventory type a `GetInventory` COUNT AGGREGATOR may count over,
//! and what the service says about one it may not — end to end, through the front door a
//! client uses (`POST /` with `X-Amz-Target: AmazonSSM.GetInventory`, dispatched by the
//! shared harness in `all/parameter_store.rs`).
//!
//! RULE under test: **the attributes an aggregator may count over are the ones the
//! AGGREGATABLE schema of the aggregator's inventory type publishes — "this call returns the
//! `AWS:InstanceInformation` type, because it supports aggregation based on the
//! `PlatformName`, `PlatformType`, and `PlatformVersion` attributes"
//! (`smithy_query(query="doc", shape="GetInventorySchemaRequest", member="Aggregator")`), the
//! same set `GetInventorySchema(Aggregator=true)` reports. An expression naming any OTHER
//! attribute of a type whose schema does define it is `InvalidAggregatorException` (HTTP 400)
//! whose message names THAT attribute and THAT type — "Attribute &lt;AttributeName&gt; of
//! type &lt;TypeName&gt; does not support count aggregator." — so a client reading the refusal
//! learns which of its up-to-ten aggregators it has to drop, wherever in the list or the
//! nesting that aggregator sat.**
//!
//! So the two halves of the partition are driven from the service's own published schemas:
//! every attribute the full schema read reports for the type is either countable (because the
//! aggregatable read reports it) or refused in the sentence naming it. The rows therefore
//! differ along what the rule generalises over: which attribute of the type an expression
//! names, whether that attribute is aggregatable, where in the `Aggregators` list the
//! rejected aggregator sits, and how deeply it is nested.
//!
//! The counts an accepted aggregation reports, and a single rejected expression, are covered
//! by `all/node_inventory.rs`; what is here is the PARTITION — the whole set of attributes the
//! type defines, judged against the aggregatable schema the service publishes — and the
//! positions in a request a rejected aggregator can sit at.

use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_ssm_amazonssm::state::{
    ComAmazonawsSsmAmazonssmState, ManagedNodeRecord, RESOURCE_TYPE_EC2_INSTANCE,
    RESOURCE_TYPE_MANAGED_INSTANCE,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::parameter_store::{call, error_code, error_message, router_with_engine};

/// The inventory type whose aggregation this suite is about.
const TYPE: &str = "AWS:InstanceInformation";

/// The attributes of [`TYPE`] an aggregator may count over, written out here independently of
/// the service from the model's own doc (`smithy_query(query="doc",
/// shape="GetInventorySchemaRequest", member="Aggregator")`: "it supports aggregation based on
/// the `PlatformName`, `PlatformType`, and `PlatformVersion` attributes").
const AGGREGATABLE: [&str; 3] = ["PlatformName", "PlatformType", "PlatformVersion"];

/// The instant every seeded node last reported, as a whole number of seconds.
const LAST_PING: Duration = Duration::from_secs(1_700_000_000);

/// The nodes seeded here, as `(node id, platform name, platform type, platform version)`:
/// two platform types, three platform names, so every aggregatable attribute has more than
/// one group to count and the counts differ between attributes.
const FLEET: [(&str, &str, &str, &str); 3] = [
    ("i-0123456789abcdef0", "Amazon Linux", "Linux", "2023"),
    ("i-0abc1234", "Ubuntu", "Linux", "22.04"),
    (
        "mi-01234567890123456",
        "Microsoft Windows Server 2022 Datacenter",
        "Windows",
        "10.0.20348",
    ),
];

/// An expression over [`TYPE`]: "the inventory type and attribute name for aggregation"
/// (`InventoryAggregator$Expression`).
fn expression(attribute: &str) -> String {
    format!("{TYPE}.{attribute}")
}

/// How the service states an expression naming an attribute its inventory type supports no
/// count aggregation on, written out here independently of the service: fixed text apart from
/// the attribute and the type, both taken from the aggregator's own expression.
fn not_aggregatable(type_name: &str, attribute: &str) -> String {
    format!("Attribute {attribute} of type {type_name} does not support count aggregator.")
}

/// A router with just this service mounted, carrying [`FLEET`] as registered nodes — a node
/// enters the store only by REGISTERING and no public call of this service registers one, so
/// the fleet is seeded through the service's own state, the store its node describes read.
fn service_with_fleet() -> MultiServiceRouter {
    let (router, engine) = router_with_engine();
    let at = SystemTime::UNIX_EPOCH + LAST_PING;
    for (id, platform_name, platform_type, platform_version) in FLEET {
        let resource_type = if id.starts_with("mi-") {
            RESOURCE_TYPE_MANAGED_INSTANCE
        } else {
            RESOURCE_TYPE_EC2_INSTANCE
        };
        let mut record = ManagedNodeRecord::registering(id, resource_type, at);
        record.properties.platform_name = Some(platform_name.to_string());
        record.properties.platform_type = Some(platform_type.to_string());
        record.properties.platform_version = Some(platform_version.to_string());
        record.properties.instance_state = Some("Active".to_string());
        assert!(
            engine
                .state::<ComAmazonawsSsmAmazonssmState>()
                .lock()
                .insert_managed_node(crate::parameter_store::REGION, record),
            "case {id:?}: a seeded node must carry the node id it is stored under"
        );
    }
    router
}

/// The attribute names the ONE schema a `GetInventorySchema` read reports carries, in the
/// order it reports them — the published statement of what the type defines, and (for an
/// aggregation read) of what an aggregator may count over.
async fn schema_attributes(router: &MultiServiceRouter, case: &str, body: &Value) -> Vec<String> {
    let (status, parsed) = call(router, "GetInventorySchema", body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: the schema read must be served, got {status} with {parsed}"
    );
    let reported = parsed["Schemas"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: a schema read reports Schemas, got {parsed}"));
    assert_eq!(
        reported.len(),
        1,
        "case {case:?}: the one type under test is reported, got {parsed}"
    );
    assert_eq!(
        reported[0]["TypeName"].as_str(),
        Some(TYPE),
        "case {case:?}: the reported schema is the type under test, got {parsed}"
    );
    reported[0]["Attributes"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: a schema carries Attributes, got {parsed}"))
        .iter()
        .map(|attribute| {
            attribute["Name"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case {case:?}: an attribute carries a Name, got {parsed}")
                })
                .to_string()
        })
        .collect()
}

/// How many nodes an aggregation over `attribute` counted in all, over the groups it reported:
/// every node holds one value of an attribute it reported, so a count of the whole fleet sums
/// to the fleet.
async fn counted_nodes(router: &MultiServiceRouter, case: &str, attribute: &str) -> usize {
    let body = json!({ "Aggregators": [{ "Expression": expression(attribute) }] });
    let (status, parsed) = call(router, "GetInventory", &body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: an aggregatable attribute is counted, got {status} with {parsed}"
    );
    let entities = parsed["Entities"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: a query carries Entities, got {parsed}"));
    assert_eq!(
        entities.len(),
        1,
        "case {case:?}: counts are reported as one entity, got {parsed}"
    );
    let rows = entities[0]["Data"][TYPE]["Content"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: the counted item carries Content, got {parsed}"));
    assert!(
        !rows.is_empty(),
        "case {case:?}: the seeded fleet reports values of this attribute, got {parsed}"
    );
    rows.iter()
        .map(|entry| {
            assert!(
                entry[attribute].is_string(),
                "case {case:?}: a row names the attribute it groups by, got {entry}"
            );
            entry["Count"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: a row carries its Count, got {entry}"))
                .parse::<usize>()
                .unwrap_or_else(|_| panic!("case {case:?}: a Count is a number, got {entry}"))
        })
        .sum()
}

/// The code and message a `GetInventory` that must be refused with HTTP 400 answers.
async fn refusal(router: &MultiServiceRouter, case: &str, body: &Value) -> (String, String) {
    let (status, parsed) = call(router, "GetInventory", body).await;
    assert_eq!(
        status, 400,
        "case {case:?}: the aggregator is refused, got {status} with {parsed}"
    );
    (error_code(&parsed), error_message(&parsed))
}

#[tokio::test]
async fn the_aggregatable_schema_is_the_documented_set_and_each_of_it_counts_the_fleet() {
    let router = service_with_fleet();

    // The aggregatable schema read is the published statement of what an aggregator may count
    // over, so it reports the one type that supports aggregation with exactly those three
    // attributes and nothing else.
    let published = schema_attributes(
        &router,
        "the aggregatable schema",
        &json!({ "Aggregator": true }),
    )
    .await;
    assert_eq!(
        published,
        AGGREGATABLE.map(String::from).to_vec(),
        "expected the documented aggregatable attributes, got {published:?}"
    );

    // Each of them is countable over the persisted fleet, so every node the store holds is
    // counted in exactly one group of the answer.
    for attribute in AGGREGATABLE {
        let counted = counted_nodes(&router, attribute, attribute).await;
        assert_eq!(
            counted,
            FLEET.len(),
            "case {attribute:?}: the counts sum to the {} seeded nodes, got {counted}",
            FLEET.len()
        );
    }
}

#[tokio::test]
async fn every_attribute_the_type_defines_outside_that_set_is_refused_naming_it() {
    let router = service_with_fleet();

    // The full schema of the type is the published statement of which attributes it DEFINES,
    // so every one of them that the aggregatable schema leaves out is an attribute the type
    // supports no count aggregation on — whichever it is, cited by the recorded case or not.
    let defined = schema_attributes(&router, TYPE, &json!({ "TypeName": TYPE })).await;
    let refused: Vec<&String> = defined
        .iter()
        .filter(|attribute| !AGGREGATABLE.contains(&attribute.as_str()))
        .collect();
    assert!(
        refused.len() >= 10,
        "the type defines at least ten attributes beside the three aggregatable ones, got \
         {defined:?}"
    );

    for attribute in refused {
        let body = json!({ "Aggregators": [{ "Expression": expression(attribute) }] });
        let (code, message) = refusal(&router, attribute, &body).await;
        assert_eq!(
            code, "InvalidAggregatorException",
            "case {attribute:?}: expected InvalidAggregatorException, got {code:?}"
        );
        let expected = not_aggregatable(TYPE, attribute);
        assert_eq!(
            message, expected,
            "case {attribute:?}: expected {expected:?}, got {message:?}"
        );
    }
}

#[tokio::test]
async fn the_refusal_names_the_rejected_aggregator_wherever_it_sits() {
    let router = service_with_fleet();

    // (case, the Aggregators list, the attribute the refusal must name). The rejected
    // aggregator is named whether it is the last of a list at the modelled bound
    // (`InventoryAggregatorList`, len 1..10) or nested under aggregators that are themselves
    // countable — and the attribute it names is the one THAT aggregator asked for, not the one
    // of some other aggregator of the same request.
    let ten: Vec<Value> = AGGREGATABLE
        .iter()
        .cycle()
        .take(9)
        .map(|attribute| json!({ "Expression": expression(attribute) }))
        .chain(std::iter::once(
            json!({ "Expression": expression("SourceType") }),
        ))
        .collect();
    let cases: Vec<(&str, Value, &str)> = vec![
        (
            "the last of a list at the modelled bound of ten",
            json!(ten),
            "SourceType",
        ),
        (
            "nested two levels under countable aggregators",
            json!([{
                "Expression": expression("PlatformType"),
                "Aggregators": [{
                    "Expression": expression("PlatformName"),
                    "Aggregators": [{ "Expression": expression("IamRole") }],
                }],
            }]),
            "IamRole",
        ),
        (
            "the first rejected one, where a later aggregator is rejected too",
            json!([
                { "Expression": expression("AgentStatus") },
                { "Expression": expression("AvailabilityZone") },
            ]),
            "AgentStatus",
        ),
    ];

    for (case, aggregators, named) in cases {
        let body = json!({ "Aggregators": aggregators });
        let (code, message) = refusal(&router, case, &body).await;
        assert_eq!(
            code, "InvalidAggregatorException",
            "case {case:?}: expected InvalidAggregatorException, got {code:?}"
        );
        let expected = not_aggregatable(TYPE, named);
        assert_eq!(
            message, expected,
            "case {case:?}: expected {expected:?}, got {message:?}"
        );
    }

    // A refused aggregation changed nothing a later caller sees: the countable aggregator
    // still counts the same persisted fleet.
    let counted = counted_nodes(&router, "after the refusals", "PlatformType").await;
    assert_eq!(
        counted,
        FLEET.len(),
        "a refusal wrote nothing: expected the {} seeded nodes, got {counted}",
        FLEET.len()
    );
}
