// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `CreateCoipPool`, driven over the wire the service actually speaks:
//! `ec2Query` — an `application/x-www-form-urlencoded` POST whose body carries
//! `Action=CreateCoipPool`, answered with XML (README.md / OPERATIONS.md "Wire protocol").
//! ec2Query is selected by the `ec2` signing name in the request's SigV4 credential scope, so
//! every call here carries one.
//!
//! The rule under test: the create WRITES ONE CUSTOMER-OWNED ADDRESS POOL INTO THE STORE,
//! belonging to the local gateway route table the request names, carrying the tags the request
//! assigns and no address range at all until `CreateCoipCidr` provisions one, and answers that
//! stored pool — so `DescribeCoipPools` reports exactly the same pool. Everything the service
//! owns is minted here rather than taken from the request: the `PoolId` comes from the store's
//! id mint and the `PoolArn` is built from that id, the account and the Region the call was
//! signed for.
//!
//! What it refuses: a request that omits `LocalGatewayRouteTableId` altogether is refused while
//! the request is still being deserialized, and one that carries it empty names no route table,
//! so it is a missing parameter; a value that is no local gateway route table id is malformed, worded with the
//! sentence a create of a resource belonging to that table answers (the id unquoted and last),
//! before anything is looked up; a well-formed id the account holds no route table for is that
//! resource's own `NotFound`; a `TagSpecification` that names another resource type, or none at
//! all, is refused rather than silently dropped; and a dry run reports the permitted outcome
//! without writing. Every refusal leaves the store exactly as it was.
//!
//! The tables below differ along what the rule generalises over — which route table of the
//! account (two, in different `LocalGatewayRouteTableMode`s, and two pools on one of them), how
//! many tags a create assigns, and in which WAY a route table id is unusable (both published id
//! lengths for an absent table; the prefix without its separator, another local gateway
//! resource's id, a suffix of neither published length, an upper-case suffix, the prefix alone
//! and text that is no id at all for a malformed one) — so no single row is the specification.

use std::sync::Arc;

use ec2::state::{
    COIP_POOL_ID_PREFIX, ComAmazonawsEc2Amazonec2State as Ec2State, DRY_RUN_OPERATION,
    INVALID_LOCAL_GATEWAY_ROUTE_TABLE_ID_MALFORMED,
    INVALID_LOCAL_GATEWAY_ROUTE_TABLE_ID_NOT_FOUND, INVALID_PARAMETER_VALUE,
    LocalGatewayRouteTableSpec, MISSING_PARAMETER, coip_pool_id_is_well_formed,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::coip_pools::{REGION, describe_one, described_ids, set_items};
use super::ec2_query::{call, element, elements, error_code, error_message, unescaped};

/// The ec2Query action under test.
const ACTION: &str = "CreateCoipPool";

/// The action a created pool is read back through, named in the assertions about it. The calls
/// themselves go through the `coip_pools` suite's own readers — the ONE describe wrapper of this
/// test tree — so a create is asserted against exactly what that describe reports.
const DESCRIBE: &str = "DescribeCoipPools";

/// The HTTP status EC2 answers a refusal about the request with.
const REFUSAL_STATUS: u16 = 400;

/// The status a `DryRun` the caller is permitted to make answers with.
const DRY_RUN_STATUS: u16 = 412;

/// The `ResourceType` a `TagSpecification` of this action may name — the pool it creates, as
/// the `ResourceType` enum spells it
/// (`smithy_query(query="shape",shape="ResourceType",model_path="services/service-com-amazonaws-ec2-amazonec2/smithy-model/com-amazonaws-ec2-amazonec2.json")`).
const RESOURCE_TYPE: &str = "coip-pool";

/// The local gateway the seeded route tables belong to.
const LOCAL_GATEWAY_ID: &str = "lgw-00000000000000001";

/// A `(key, value)` request parameter.
fn p(key: &str, value: &str) -> (String, String) {
    (key.to_string(), value.to_string())
}

/// The emulator with two local gateway route tables of the account in its store, written
/// through the store's own writer seam — the one `CreateLocalGatewayRouteTable` writes through
/// — so no fixture is a value this suite invented beside the store. Answers the ids, in the
/// order they were written.
fn emulator() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>, Vec<String>) {
    let (router, engine) = super::ec2_query::emulator();
    let handle = engine.state::<Ec2State>();
    let mut store = handle.lock();
    // One route table in each `LocalGatewayRouteTableMode` member, so a pool is created on
    // more than one table and a pool written on one is never read off the other.
    let route_tables: Vec<String> = ["coip", "direct-vpc-routing"]
        .into_iter()
        .map(|mode| {
            store
                .insert_local_gateway_route_table(LocalGatewayRouteTableSpec {
                    local_gateway_id: LOCAL_GATEWAY_ID.to_string(),
                    outpost_arn: format!(
                        "arn:aws:outposts:{REGION}:{}:outpost/op-00000000000000001",
                        engine::emulator_account_id()
                    ),
                    region: REGION.to_string(),
                    mode: mode.to_string(),
                    state: "available".to_string(),
                    becomes: None,
                    tags: Vec::new(),
                })
                .unwrap_or_else(|| panic!("case {mode:?}: the store must take the route table"))
        })
        .collect();
    drop(store);
    (router, engine, route_tables)
}

/// One `CreateCoipPool` call carrying `params`, as its HTTP status and XML body.
async fn create(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    call(router, REGION, ACTION, params).await
}

/// The form fields of one create: the route table the pool belongs to, and the tags it assigns
/// to that pool as a single `TagSpecification` naming the pool's own resource type.
fn create_params(route_table_id: &str, tags: &[(&str, &str)]) -> Vec<(String, String)> {
    let mut params = vec![p("LocalGatewayRouteTableId", route_table_id)];
    if !tags.is_empty() {
        params.push(p("TagSpecification.1.ResourceType", RESOURCE_TYPE));
        for (position, (key, value)) in tags.iter().enumerate() {
            let nth = position + 1;
            params.push(p(&format!("TagSpecification.1.Tag.{nth}.Key"), key));
            params.push(p(&format!("TagSpecification.1.Tag.{nth}.Value"), value));
        }
    }
    params
}

/// The `(key, value)` pairs of the `tagSet` a body carries, in the order it wrote them.
fn tag_pairs(xml: &str) -> Vec<(String, String)> {
    let tag_set = element(xml, "tagSet").unwrap_or_default();
    elements(&tag_set, "key")
        .into_iter()
        .zip(elements(&tag_set, "value"))
        .map(|(key, value)| (unescaped(&key), unescaped(&value)))
        .collect()
}

/// How many pools the store holds.
fn stored_pools(engine: &engine::InMemoryEngine) -> usize {
    engine.state::<Ec2State>().lock().coip_pools.len()
}

/// One create under test: which seeded route table the pool belongs to and the tags it is
/// assigned.
struct Row {
    case: &'static str,
    route_table: usize,
    tags: Vec<(&'static str, &'static str)>,
}

/// The creates this suite drives: both seeded route tables, two pools on one of them (so one
/// route table holding more than one pool is covered), and every tag count from none to three
/// — including a value carrying spaces and one carrying none at all.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "the coip route table, no tags",
            route_table: 0,
            tags: Vec::new(),
        },
        Row {
            case: "the coip route table, one tag",
            route_table: 0,
            tags: vec![("Name", "outpost-pool")],
        },
        Row {
            case: "the direct-vpc-routing route table, two tags",
            route_table: 1,
            tags: vec![("Name", "second pool"), ("Tier", "prod")],
        },
        Row {
            case: "the direct-vpc-routing route table, three tags, one empty value",
            route_table: 1,
            tags: vec![("Name", "third"), ("Owner", "TeamA"), ("Note", "")],
        },
    ]
}

// ---------------------------------------------------------------------------
// The pool the create stores IS the pool it answers
// ---------------------------------------------------------------------------

/// RULE: each create stores one pool on the route table it names, mints that pool's own id and
/// ARN, assigns exactly the tags the request carried, holds no address range yet, and answers
/// the record `DescribeCoipPools` then reports — and every create adds one more pool.
#[tokio::test]
async fn each_create_stores_the_pool_it_answers() {
    let (router, engine, route_tables) = emulator();
    let mut minted: Vec<String> = Vec::new();

    for Row {
        case,
        route_table,
        tags,
    } in rows()
    {
        let expected_table = &route_tables[route_table];
        let (status, body) = create(&router, &create_params(expected_table, &tags)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}"
        );

        let pool_id = element(&body, "poolId").unwrap_or_default();
        assert!(
            coip_pool_id_is_well_formed(&pool_id),
            "case {case:?}: expected an id of the form {COIP_POOL_ID_PREFIX}<hex>, got \
             {pool_id:?} in {body}"
        );
        assert!(
            !minted.contains(&pool_id),
            "case {case:?}: each create mints its own pool, so {pool_id:?} must be new, held \
             already by {minted:?}"
        );

        let reported_table = element(&body, "localGatewayRouteTableId").unwrap_or_default();
        assert_eq!(
            &reported_table, expected_table,
            "case {case:?}: expected localGatewayRouteTableId {expected_table:?}, got \
             {reported_table:?}"
        );

        // `CoipPool$PoolArn` is "The ARN of the address pool": built from the pool's own minted
        // id, in the Region the call was signed for and the emulator's canonical account.
        let expected_arn = engine::emulator_arn("ec2", REGION, &format!("coip-pool/{pool_id}"));
        let reported_arn = element(&body, "poolArn").unwrap_or_default();
        assert_eq!(
            reported_arn, expected_arn,
            "case {case:?}: expected poolArn {expected_arn:?}, got {reported_arn:?}"
        );

        // A new pool holds no range until `CreateCoipCidr` provisions one, so the create
        // reports none rather than a range nothing allocated.
        let reported_cidrs = set_items(&body, "poolCidrSet");
        assert!(
            reported_cidrs.is_empty(),
            "case {case:?}: a pool holds no address range until one is provisioned, got \
             {reported_cidrs:?}"
        );

        let expected_tags: Vec<(String, String)> = tags
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect();
        let reported_tags = tag_pairs(&body);
        assert_eq!(
            reported_tags, expected_tags,
            "case {case:?}: expected tagSet {expected_tags:?}, got {reported_tags:?}"
        );

        // The stored pool, read back through the describe: the two surfaces cannot report one
        // pool differently.
        let described = describe_one(&router, case, &pool_id).await;
        for (member, expected) in [
            ("poolId", pool_id.clone()),
            ("localGatewayRouteTableId", expected_table.clone()),
            ("poolArn", expected_arn.clone()),
        ] {
            let read_back = element(&described, member).unwrap_or_default();
            assert_eq!(
                read_back, expected,
                "case {case:?}: expected {DESCRIBE} to report {member} {expected:?}, got \
                 {read_back:?}"
            );
        }
        let described_tags = tag_pairs(&described);
        assert_eq!(
            described_tags, expected_tags,
            "case {case:?}: expected {DESCRIBE} to report tagSet {expected_tags:?}, got \
             {described_tags:?}"
        );
        assert!(
            set_items(&described, "poolCidrSet").is_empty(),
            "case {case:?}: expected {DESCRIBE} to report no address range, got {described}"
        );

        minted.push(pool_id);
        assert_eq!(
            stored_pools(&engine),
            minted.len(),
            "case {case:?}: expected {} stored pools after the create",
            minted.len()
        );
    }

    let reported = described_ids(&router, "every create", &[]).await;
    assert_eq!(
        reported, minted,
        "expected {DESCRIBE} to report every created pool, got {reported:?}"
    );
}

// ---------------------------------------------------------------------------
// What the create refuses, and the store it leaves untouched
// ---------------------------------------------------------------------------

/// One refusal under test: the request, the code it must carry, and the sentence it must carry
/// where that sentence is the handler's own (the request boundary's validator words its own).
struct Refusal {
    case: String,
    params: Vec<(String, String)>,
    code: &'static str,
    message: Option<String>,
}

/// The sentence a create of a resource belonging to a local gateway route table words a value
/// that is no route table id with: the owning service's own words — a fixed prefix then the id
/// the REQUEST sent, unquoted and last — for a value SHAPED like an EC2 resource id, and the
/// request boundary's shared template for a value that is no identifier at all, which never
/// reaches that service. `shaped` is the row's own statement of which of the two it is.
fn malformed_message(id: &str, shaped: bool) -> String {
    if shaped {
        format!("Invalid LocalGateway Route Table id {id}")
    } else {
        format!("The local-gateway-route-table ID '{id}' is malformed")
    }
}

/// The sentence this resource words a well-formed id the account holds no route table for with.
fn not_found_message(id: &str) -> String {
    format!("The LocalGateway Route Table ID '{id}' does not exist")
}

/// The refusals this suite drives, against the one seeded route table a tagging row needs.
fn refusals(route_table_id: &str) -> Vec<Refusal> {
    let mut rows = vec![
        // A member the request omits ENTIRELY is refused while the request is still being
        // deserialized, before this operation's handler runs — with the framework's own
        // `MalformedInput` and its own sentence, not with the handler's `MissingParameter`, as
        // every other operation of this service answers it.
        Refusal {
            case: "no LocalGatewayRouteTableId at all".to_string(),
            params: Vec::new(),
            code: "MalformedInput",
            message: None,
        },
        // A member the request DOES carry, empty, reaches the handler: `LocalGatewayRouteTableId`
        // is `@required` but modelled `clientOptional`, and a parameter carrying no value names
        // no route table.
        Refusal {
            case: "an empty LocalGatewayRouteTableId".to_string(),
            params: vec![p("LocalGatewayRouteTableId", "")],
            code: MISSING_PARAMETER,
            message: Some(
                "The request must contain the parameter localGatewayRouteTableId".to_string(),
            ),
        },
    ];
    // A value that is no local gateway route table id at all: the prefix without its separator,
    // another local gateway resource's id, suffixes of neither published length, an upper-case
    // suffix, the prefix alone, and text that is no id at all. The second column is whether the
    // value is SHAPED like an EC2 resource id — a prefix, a separator and a lowercase
    // hexadecimal suffix — written out per row rather than recomputed, so the table says which
    // of the two sentences this class carries each row drives.
    for (malformed, shaped) in [
        ("lgwrtb-0123456789abcdef0", true),
        ("lgw-00000000000000001", true),
        ("lgw-rtb-0a1b2c3", true),
        ("lgw-rtb-0123456789abcdef", true),
        ("lgw-rtb-0A1B2C3D", false),
        ("lgw-rtb-", false),
        ("not-a-route-table-id", false),
    ] {
        rows.push(Refusal {
            case: format!("the unreadable route table id {malformed}"),
            params: vec![p("LocalGatewayRouteTableId", malformed)],
            code: INVALID_LOCAL_GATEWAY_ROUTE_TABLE_ID_MALFORMED,
            message: Some(malformed_message(malformed, shaped)),
        });
    }
    // Both id lengths EC2 publishes for this resource, held by no route table of the account.
    for absent in ["lgw-rtb-0123456789abcdef0", "lgw-rtb-0a1b2c3d"] {
        rows.push(Refusal {
            case: format!("the absent route table {absent}"),
            params: vec![p("LocalGatewayRouteTableId", absent)],
            code: INVALID_LOCAL_GATEWAY_ROUTE_TABLE_ID_NOT_FOUND,
            message: Some(not_found_message(absent)),
        });
    }
    // "You must specify the resource type to tag, otherwise the request will fail"
    // (`TagSpecification`), and the only type this action tags is the pool it creates.
    for other_type in ["vpc", "local-gateway-route-table", "ipv4pool-ec2"] {
        rows.push(Refusal {
            case: format!("tags assigned to the {other_type} resource type"),
            params: vec![
                p("LocalGatewayRouteTableId", route_table_id),
                p("TagSpecification.1.ResourceType", other_type),
                p("TagSpecification.1.Tag.1.Key", "Name"),
                p("TagSpecification.1.Tag.1.Value", "misdirected"),
            ],
            code: INVALID_PARAMETER_VALUE,
            message: Some(format!(
                "'{other_type}' is not a valid resource type for tagging on {ACTION}"
            )),
        });
    }
    rows.push(Refusal {
        case: "a TagSpecification naming no resource type".to_string(),
        params: vec![
            p("LocalGatewayRouteTableId", route_table_id),
            p("TagSpecification.1.Tag.1.Key", "Name"),
            p("TagSpecification.1.Tag.1.Value", "untyped"),
        ],
        code: MISSING_PARAMETER,
        message: Some("Tag specification resource type must have a value".to_string()),
    });
    rows
}

/// RULE: every refusal above carries HTTP 400, the code that says HOW the request failed, and
/// that class's own sentence — and NOTHING is stored, whichever way the request was wrong.
#[tokio::test]
async fn a_refused_create_stores_nothing() {
    let (router, engine, route_tables) = emulator();

    for Refusal {
        case,
        params,
        code,
        message,
    } in refusals(&route_tables[0])
    {
        let (status, body) = create(&router, &params).await;
        assert_eq!(
            status, REFUSAL_STATUS,
            "case {case:?}: expected HTTP {REFUSAL_STATUS}, got {status} with body {body}"
        );
        let answered_code = error_code(&body);
        assert_eq!(
            answered_code, code,
            "case {case:?}: expected the code {code:?}, got {answered_code:?} in {body}"
        );
        if let Some(message) = message {
            let answered = error_message(&body);
            assert_eq!(
                answered, message,
                "case {case:?}: expected the message {message:?}, got {answered:?}"
            );
        }
        assert_eq!(
            stored_pools(&engine),
            0,
            "case {case:?}: a refused create must store nothing"
        );
    }

    let reported = described_ids(&router, "after every refusal", &[]).await;
    assert!(
        reported.is_empty(),
        "a refused create must leave the store empty, got {reported:?}"
    );
}

// ---------------------------------------------------------------------------
// DryRun
// ---------------------------------------------------------------------------

/// RULE: `DryRun` is answered once the request's own FORM has been read and before anything is
/// resolved or written — so a caller this emulator permits is told the request would have
/// succeeded whether or not the route table it names exists, while a request whose route table
/// id cannot be read at all is still refused for its form, and no pool is stored either way.
#[tokio::test]
async fn a_dry_run_reports_the_outcome_and_writes_nothing() {
    let (router, engine, route_tables) = emulator();

    // Rows: a route table of the account, both published id lengths for one it does not hold
    // (the probe is about the CALLER, so it is answered before the lookup), and two values that
    // are no route table id at all (a form the handler reads first).
    let rows: Vec<(String, String, u16, &str, Option<String>)> = vec![
        (
            "a route table of the account".to_string(),
            route_tables[0].clone(),
            DRY_RUN_STATUS,
            DRY_RUN_OPERATION,
            None,
        ),
        (
            "a 17-digit route table the account does not hold".to_string(),
            "lgw-rtb-0123456789abcdef0".to_string(),
            DRY_RUN_STATUS,
            DRY_RUN_OPERATION,
            None,
        ),
        (
            "an 8-digit route table the account does not hold".to_string(),
            "lgw-rtb-0a1b2c3d".to_string(),
            DRY_RUN_STATUS,
            DRY_RUN_OPERATION,
            None,
        ),
        (
            "a value that is no route table id at all".to_string(),
            "not-a-route-table-id".to_string(),
            REFUSAL_STATUS,
            INVALID_LOCAL_GATEWAY_ROUTE_TABLE_ID_MALFORMED,
            Some(malformed_message("not-a-route-table-id", false)),
        ),
        (
            "a route table id of neither published length".to_string(),
            "lgw-rtb-0a1b2c3".to_string(),
            REFUSAL_STATUS,
            INVALID_LOCAL_GATEWAY_ROUTE_TABLE_ID_MALFORMED,
            Some(malformed_message("lgw-rtb-0a1b2c3", true)),
        ),
    ];

    for (case, route_table_id, expected_status, expected_code, expected_message) in rows {
        let mut params = create_params(&route_table_id, &[("Name", "probe")]);
        params.push(p("DryRun", "true"));
        let (status, body) = create(&router, &params).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} with body {body}"
        );
        let answered_code = error_code(&body);
        assert_eq!(
            answered_code, expected_code,
            "case {case:?}: expected the code {expected_code:?}, got {answered_code:?} in {body}"
        );
        if let Some(expected_message) = expected_message {
            let answered = error_message(&body);
            assert_eq!(
                answered, expected_message,
                "case {case:?}: expected the message {expected_message:?}, got {answered:?}"
            );
        }
        assert_eq!(
            stored_pools(&engine),
            0,
            "case {case:?}: a dry run must store nothing"
        );
    }

    let reported = described_ids(&router, "after every dry run", &[]).await;
    assert!(
        reported.is_empty(),
        "a dry run must leave the store empty, got {reported:?}"
    );
}
