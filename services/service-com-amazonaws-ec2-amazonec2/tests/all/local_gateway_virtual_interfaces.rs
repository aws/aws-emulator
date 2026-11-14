// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `CreateLocalGatewayVirtualInterface` and for the
//! `DescribeLocalGatewayVirtualInterfaces` that reports what it stored, driven over the wire
//! the service actually speaks: `ec2Query` — an `application/x-www-form-urlencoded` POST whose
//! body carries `Action=<Operation>`, answered with XML (README.md / OPERATIONS.md "Wire
//! protocol"). ec2Query is selected by the `ec2` signing name in the request's SigV4
//! credential scope, so every call here carries one.
//!
//! Two rules, each with its own table.
//!
//! 1. The CREATE computes its answer from the store: it creates the interface IN a virtual
//!    interface group the account HOLDS, mints the interface id, its ARN and the owning
//!    account itself, reads the local gateway and the local BGP ASN off that stored group,
//!    keeps the request's own Outpost LAG, VLAN, addresses and peer ASN, answers the
//!    non-terminal `pending` configuration state the clock owes `available`, adds the minted
//!    id to its group's interface list, and refuses a request naming a group the account does
//!    not hold with that resource's own `NotFound` code (and an id that is not one of that
//!    kind at all with its `.Malformed`). Its rows differ along which group, on which local
//!    gateway, with which local BGP ASN and in which Region, which Outpost LAG, which VLAN,
//!    which addresses, which of the two peer-ASN members, how many tags, which resource type
//!    a tag specification names, and whether the request only asks about permission.
//! 2. The DESCRIBE reports THE STORE: the set it answers with is the account's local gateway
//!    virtual interfaces, narrowed by the `LocalGatewayVirtualInterfaceIds` and `Filters` the
//!    request names and cut into `MaxResults`-sized pages. Its rows differ along which
//!    filter, how many values, how many filters, which ids, which members a virtual interface
//!    holds a value for, which page size — and an account with no Outpost (the empty store) is
//!    one row among them rather than the specification.

use std::time::Duration;

use ec2::ops::types::Tag;
use ec2::state::{
    ComAmazonawsEc2Amazonec2State as Ec2State, LOCAL_GATEWAY_VIF_ID_PREFIX,
    LOCAL_GATEWAY_VIF_STATE_AVAILABLE, LOCAL_GATEWAY_VIF_STATE_PENDING,
    LocalGatewayVirtualInterfaceGroupSpec, LocalGatewayVirtualInterfaceSpec as InterfaceSpec,
    TRANSITION_DELAY,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{element, elements, error_code, error_message, filters, indexed, unescaped};

/// The ec2Query action that creates one.
const CREATE: &str = "CreateLocalGatewayVirtualInterface";

/// Long enough past [`TRANSITION_DELAY`] that the move the clock owes a new interface is
/// certainly due.
fn past_the_transition() -> Duration {
    TRANSITION_DELAY + Duration::from_millis(250)
}

/// One form field.
fn param(name: &str, value: &str) -> (String, String) {
    (name.to_string(), value.to_string())
}

/// One `CreateLocalGatewayVirtualInterface` call, as its status and body.
async fn create(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    super::ec2_query::call(router, REGION, CREATE, params).await
}

/// The form fields for one `TagSpecification` naming `resource_type` and `tags`.
fn tag_specification(resource_type: &str, tags: &[(&str, &str)]) -> Vec<(String, String)> {
    let mut fields = vec![param("TagSpecification.1.ResourceType", resource_type)];
    for (index, (key, value)) in tags.iter().enumerate() {
        let position = index + 1;
        fields.push(param(&format!("TagSpecification.1.Tag.{position}.Key"), key));
        fields.push(param(
            &format!("TagSpecification.1.Tag.{position}.Value"),
            value,
        ));
    }
    fields
}

/// What one interface request names, beside the group it is created in.
struct Wire {
    outpost_lag_id: &'static str,
    vlan: i32,
    local_address: &'static str,
    peer_address: &'static str,
    /// The peer ASN in `PeerBgpAsn`, in `PeerBgpAsnExtended`, or in neither.
    peer_bgp_asn: Option<i32>,
    peer_bgp_asn_extended: Option<i64>,
}

/// The five `@required` members of a create, with the two optional ASN members when the row
/// carries one.
fn request(group_id: &str, wire: &Wire) -> Vec<(String, String)> {
    let mut fields = vec![
        param("LocalGatewayVirtualInterfaceGroupId", group_id),
        param("OutpostLagId", wire.outpost_lag_id),
        param("Vlan", &wire.vlan.to_string()),
        param("LocalAddress", wire.local_address),
        param("PeerAddress", wire.peer_address),
    ];
    if let Some(asn) = wire.peer_bgp_asn {
        fields.push(param("PeerBgpAsn", &asn.to_string()));
    }
    if let Some(asn) = wire.peer_bgp_asn_extended {
        fields.push(param("PeerBgpAsnExtended", &asn.to_string()));
    }
    fields
}

/// A request that would succeed, so a table that varies ONE member can leave the rest alone.
fn well_formed(group_id: &str) -> Vec<(String, String)> {
    request(
        group_id,
        &Wire {
            outpost_lag_id: "ola-00000000000000001",
            vlan: 100,
            local_address: "10.0.0.1",
            peer_address: "10.0.0.2",
            peer_bgp_asn: Some(65000),
            peer_bgp_asn_extended: None,
        },
    )
}

/// What one seeded virtual interface group is: the local gateway it belongs to, its local BGP
/// configuration, the Region its own ARN names and its configuration state. Nothing in the EC2
/// API this emulator serves creates an Outpost, so a group is written straight through the
/// store's own seam — the one `CreateLocalGatewayVirtualInterfaceGroup` will write through.
struct GroupRow {
    local_gateway_id: &'static str,
    local_bgp_asn: Option<i32>,
    region: &'static str,
    /// One of `LocalGatewayVirtualInterfaceGroupConfigurationState`'s members.
    configuration_state: &'static str,
}

/// The groups this suite creates interfaces in: two local gateways, a group with and a group
/// without a local BGP ASN, and one group whose own ARN names another Region.
fn group_rows() -> Vec<GroupRow> {
    vec![
        GroupRow {
            local_gateway_id: "lgw-00000000000000001",
            local_bgp_asn: Some(64512),
            region: REGION,
            configuration_state: "available",
        },
        GroupRow {
            local_gateway_id: "lgw-00000000000000001",
            local_bgp_asn: None,
            region: REGION,
            configuration_state: "available",
        },
        GroupRow {
            local_gateway_id: "lgw-00000000000000002",
            local_bgp_asn: Some(64513),
            region: OTHER_REGION,
            configuration_state: "pending",
        },
    ]
}

/// The Region every call here is signed for. The virtual interface store is per account
/// rather than per Region, so the Region only selects the ec2Query envelope — and the
/// Region a stored interface's ARN names is the one its writer was signed for, which is why
/// one seeded row carries [`OTHER_REGION`].
const REGION: &str = "us-east-1";

/// The Region of the one seeded virtual interface written from another Region, so the
/// reported ARNs have two Region namespaces to tell apart.
const OTHER_REGION: &str = "us-west-2";

/// The ec2Query action under test.
const ACTION: &str = "DescribeLocalGatewayVirtualInterfaces";

/// The XML element each reported virtual interface's id arrives in
/// (`LocalGatewayVirtualInterface$LocalGatewayVirtualInterfaceId`'s `xmlName`).
const ID_ELEMENT: &str = "localGatewayVirtualInterfaceId";

/// One call carrying `params`, as its HTTP status and XML body.
async fn describe(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    super::ec2_query::call(router, REGION, ACTION, params).await
}

/// The virtual interface ids a successful response reports, in the order it reported them.
async fn described_ids(
    router: &MultiServiceRouter,
    case: &str,
    params: &[(String, String)],
) -> Vec<String> {
    let (status, body) = describe(router, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with body {body}"
    );
    elements(&body, ID_ELEMENT)
}

/// One `MaxResults` parameter.
fn max_results_param(max_results: i64) -> Vec<(String, String)> {
    vec![("MaxResults".to_string(), max_results.to_string())]
}

/// A tag, as the seeded virtual interfaces carry them.
fn tag(key: &str, value: &str) -> Tag {
    Tag {
        key: Some(key.to_string()),
        value: Some(value.to_string()),
    }
}

/// What one seeded local gateway virtual interface holds.
struct InterfaceRow {
    local_gateway_id: &'static str,
    /// "The ID of the local gateway virtual interface group" — absent on the row that
    /// belongs to no group yet, so a report has a member with no value in it.
    group_id: Option<&'static str>,
    outpost_lag_id: Option<&'static str>,
    vlan: Option<i32>,
    local_address: Option<&'static str>,
    peer_address: Option<&'static str>,
    local_bgp_asn: Option<i32>,
    peer_bgp_asn: Option<i32>,
    peer_bgp_asn_extended: Option<i64>,
    region: &'static str,
    /// "The current state of the local gateway virtual interface"
    /// (`LocalGatewayVirtualInterface$ConfigurationState`), one of the values
    /// `LocalGatewayVirtualInterfaceConfigurationState` publishes — the generated catalog
    /// `ec2::ops::types::LOCAL_GATEWAY_VIRTUAL_INTERFACE_CONFIGURATION_STATE_VALUES`, which
    /// [`every_seeded_state_is_a_modelled_configuration_state`] checks these against.
    configuration_state: &'static str,
    tags: Vec<Tag>,
}

/// The virtual interfaces the filter and paging tables are written against. They differ
/// along every attribute the documented filters select on — the local gateway, the local and
/// peer addresses, both BGP ASNs and the VLAN — and along every member that is merely
/// reported: the group, the Outpost LAG, the extended peer ASN, the Region the ARN names,
/// the configuration state and the tags. Three rows share a local gateway and two more share
/// another, two share a VLAN and two share a peer ASN, so a filter that selects on one
/// reports more than a single row; one row holds no value for most attributes at all.
fn seeded_rows() -> Vec<InterfaceRow> {
    vec![
        InterfaceRow {
            local_gateway_id: "lgw-00000000000000001",
            group_id: Some("lgw-vif-grp-00000000000000001"),
            outpost_lag_id: Some("ola-00000000000000001"),
            vlan: Some(101),
            local_address: Some("10.0.0.1"),
            peer_address: Some("10.0.0.2"),
            local_bgp_asn: Some(64512),
            peer_bgp_asn: Some(65001),
            peer_bgp_asn_extended: None,
            region: REGION,
            configuration_state: "available",
            tags: vec![tag("Name", "rack-a"), tag("Owner", "TeamA")],
        },
        InterfaceRow {
            local_gateway_id: "lgw-00000000000000001",
            group_id: Some("lgw-vif-grp-00000000000000001"),
            outpost_lag_id: Some("ola-00000000000000001"),
            vlan: Some(102),
            local_address: Some("10.0.0.5"),
            peer_address: Some("10.0.0.6"),
            local_bgp_asn: Some(64512),
            peer_bgp_asn: Some(65002),
            peer_bgp_asn_extended: Some(4_200_000_000),
            region: REGION,
            configuration_state: "pending",
            tags: vec![tag("Name", "rack-a")],
        },
        // Holds no value for most attributes: no group, no LAG, no VLAN, no address and no
        // ASN — so no value of those filters can select it.
        InterfaceRow {
            local_gateway_id: "lgw-00000000000000001",
            group_id: None,
            outpost_lag_id: None,
            vlan: None,
            local_address: None,
            peer_address: None,
            local_bgp_asn: None,
            peer_bgp_asn: None,
            peer_bgp_asn_extended: None,
            region: REGION,
            configuration_state: "deleting",
            tags: Vec::new(),
        },
        InterfaceRow {
            local_gateway_id: "lgw-00000000000000002",
            group_id: Some("lgw-vif-grp-00000000000000002"),
            outpost_lag_id: Some("ola-00000000000000002"),
            vlan: Some(201),
            local_address: Some("10.1.0.1"),
            peer_address: Some("10.1.0.2"),
            local_bgp_asn: Some(64513),
            peer_bgp_asn: Some(65003),
            peer_bgp_asn_extended: None,
            region: OTHER_REGION,
            configuration_state: "available",
            tags: vec![tag("Name", "rack-b")],
        },
        InterfaceRow {
            local_gateway_id: "lgw-00000000000000003",
            group_id: Some("lgw-vif-grp-00000000000000003"),
            outpost_lag_id: None,
            vlan: Some(301),
            local_address: Some("10.2.0.1"),
            peer_address: Some("10.2.0.2"),
            local_bgp_asn: Some(64514),
            // The same peer ASN as the first row, so that filter selects two rows.
            peer_bgp_asn: Some(65001),
            peer_bgp_asn_extended: None,
            region: REGION,
            configuration_state: "deleted",
            tags: vec![tag("Env", "test")],
        },
        InterfaceRow {
            local_gateway_id: "lgw-00000000000000003",
            group_id: Some("lgw-vif-grp-00000000000000003"),
            outpost_lag_id: Some("ola-00000000000000003"),
            // The same VLAN as the first row, so that filter selects two rows.
            vlan: Some(101),
            local_address: Some("10.2.0.5"),
            peer_address: Some("10.2.0.6"),
            local_bgp_asn: Some(64514),
            peer_bgp_asn: Some(65004),
            peer_bgp_asn_extended: None,
            region: REGION,
            configuration_state: "available",
            tags: Vec::new(),
        },
        InterfaceRow {
            local_gateway_id: "lgw-00000000000000004",
            group_id: Some("lgw-vif-grp-00000000000000004"),
            outpost_lag_id: Some("ola-00000000000000004"),
            vlan: Some(401),
            local_address: Some("10.3.0.1"),
            peer_address: Some("10.3.0.2"),
            local_bgp_asn: None,
            peer_bgp_asn: None,
            peer_bgp_asn_extended: Some(4_200_000_001),
            region: REGION,
            configuration_state: "pending",
            tags: vec![tag("Name", "rack-c")],
        },
    ]
}

/// Write one virtual interface group into the store and answer the id it was given.
fn seed_group(engine: &engine::InMemoryEngine, row: &GroupRow) -> String {
    let case = row.local_gateway_id;
    engine
        .state::<Ec2State>()
        .lock()
        .insert_local_gateway_virtual_interface_group(LocalGatewayVirtualInterfaceGroupSpec {
            local_gateway_id: row.local_gateway_id.to_string(),
            // The groups start with no interface in them, so every id one reports later can
            // only have been added by a create.
            local_gateway_virtual_interface_ids: Vec::new(),
            local_bgp_asn: row.local_bgp_asn,
            local_bgp_asn_extended: None,
            region: row.region.to_string(),
            configuration_state: row.configuration_state.to_string(),
            tags: Vec::new(),
        })
        .unwrap_or_else(|| panic!("case {case:?}: the store must take the seeded group"))
}

/// Every group of the table above, in row order.
fn seed_all(engine: &engine::InMemoryEngine) -> Vec<String> {
    group_rows()
        .iter()
        .map(|row| seed_group(engine, row))
        .collect()
}

/// The spec one seeded row is written through.
fn spec_of(row: &InterfaceRow) -> InterfaceSpec {
    InterfaceSpec {
        local_gateway_id: row.local_gateway_id.to_string(),
        local_gateway_virtual_interface_group_id: row.group_id.map(str::to_string),
        outpost_lag_id: row.outpost_lag_id.map(str::to_string),
        vlan: row.vlan,
        local_address: row.local_address.map(str::to_string),
        peer_address: row.peer_address.map(str::to_string),
        local_bgp_asn: row.local_bgp_asn,
        peer_bgp_asn: row.peer_bgp_asn,
        peer_bgp_asn_extended: row.peer_bgp_asn_extended,
        region: row.region.to_string(),
        configuration_state: row.configuration_state.to_string(),
        tags: row.tags.clone(),
    }
}

/// Write the seeded virtual interfaces into the engine's store, the way the operation that
/// creates a local gateway virtual interface would, and report the ids they were given in
/// store order.
fn seed(engine: &engine::InMemoryEngine) -> Vec<String> {
    let store = engine.state::<Ec2State>();
    let mut guard = store.lock();
    let mut ids = Vec::new();
    for row in seeded_rows() {
        let case = row.configuration_state;
        ids.push(
            guard
                .insert_local_gateway_virtual_interface(spec_of(&row))
                .unwrap_or_else(|| {
                    panic!("case {case:?}: the store must take the seeded virtual interface")
                }),
        );
    }
    ids
}

/// The ids at `indices`, in store order.
fn ids_at(seeded: &[String], indices: &[usize]) -> Vec<String> {
    indices.iter().map(|index| seeded[*index].clone()).collect()
}

/// The ARN of a local gateway virtual interface, as
/// `LocalGatewayVirtualInterface$LocalGatewayVirtualInterfaceArn` carries it:
/// `arn:aws:ec2:<region>:<account>:local-gateway-virtual-interface/<id>`.
fn interface_arn(region: &str, interface_id: &str) -> String {
    format!(
        "arn:aws:ec2:{region}:{}:local-gateway-virtual-interface/{interface_id}",
        engine::emulator_account_id()
    )
}

/// How many virtual interfaces the store holds.
fn stored_interfaces(engine: &engine::InMemoryEngine) -> usize {
    engine
        .state::<Ec2State>()
        .lock()
        .local_gateway_virtual_interfaces
        .len()
}

/// "The IDs of the virtual interfaces" one stored group reports.
fn group_interface_ids(engine: &engine::InMemoryEngine, group_id: &str) -> Vec<String> {
    engine
        .state::<Ec2State>()
        .lock()
        .local_gateway_virtual_interface_groups
        .get(group_id)
        .and_then(|group| group.local_gateway_virtual_interface_ids.clone())
        .unwrap_or_default()
}

/// Assert that `body` carries `<member>value</member>`.
fn assert_member(case: &str, body: &str, member: &str, value: &str) {
    assert!(
        body.contains(&format!("<{member}>{value}</{member}>")),
        "case {case:?}: expected <{member}>{value}</{member}>, got {body}"
    );
}

// ---------------------------------------------------------------------------
// The create computes its answer from the store
// ---------------------------------------------------------------------------

/// One row of the create table: the group to create the interface in, what the request names,
/// and the tags to assign.
struct CreateRow {
    group: usize,
    wire: Wire,
    tags: Vec<(&'static str, &'static str)>,
}

/// Every member of a created interface comes from the request or from the group it is created
/// in: the minted id and ARN, the group's own id, local gateway and local BGP ASN, the Outpost
/// LAG, VLAN, addresses and peer ASN the request named, the owning account, the transient
/// configuration state and the tags — and the interface reads back identically through
/// `DescribeLocalGatewayVirtualInterfaces`, which is also where the group's interface list is
/// seen to have gained it.
#[tokio::test]
async fn a_created_virtual_interface_reports_its_group_the_request_and_the_minted_members() {
    let (router, engine) = super::ec2_query::emulator();
    let groups = seed_all(&engine);
    let rows = group_rows();

    // Rows differ along the group (its local gateway, its local BGP ASN — present and absent —
    // its Region and its configuration state), the Outpost LAG, the VLAN, the two addresses,
    // which of the two peer-ASN members carries the ASN (or neither), and the number of tags.
    let cases = vec![
        CreateRow {
            group: 0,
            wire: Wire {
                outpost_lag_id: "ola-00000000000000001",
                vlan: 1,
                local_address: "10.0.0.1",
                peer_address: "10.0.0.2",
                peer_bgp_asn: Some(65000),
                peer_bgp_asn_extended: None,
            },
            tags: Vec::new(),
        },
        CreateRow {
            group: 1,
            wire: Wire {
                outpost_lag_id: "ola-00000000000000002",
                vlan: 4094,
                local_address: "192.0.2.17",
                peer_address: "192.0.2.18",
                peer_bgp_asn: None,
                peer_bgp_asn_extended: Some(4_294_967_295),
            },
            tags: vec![("Name", "rack-a")],
        },
        CreateRow {
            group: 2,
            wire: Wire {
                outpost_lag_id: "ola-00000000000000003",
                vlan: 2048,
                local_address: "172.16.31.254",
                peer_address: "172.16.31.253",
                peer_bgp_asn: None,
                peer_bgp_asn_extended: None,
            },
            tags: vec![("Name", "rack-b"), ("Owner", "TeamA"), ("Tier", "edge")],
        },
    ];

    let mut minted: Vec<String> = Vec::new();
    for case_row in &cases {
        let case = format!(
            "group {} on VLAN {} with {} tag(s)",
            case_row.group,
            case_row.wire.vlan,
            case_row.tags.len()
        );
        let group_id = &groups[case_row.group];
        let row = &rows[case_row.group];

        let mut params = request(group_id, &case_row.wire);
        if !case_row.tags.is_empty() {
            params.extend(tag_specification(
                "local-gateway-virtual-interface",
                &case_row.tags,
            ));
        }
        let (status, body) = create(&router, &params).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from {CREATE}, got {status} with body {body}"
        );

        let virtual_interface_id = element(&body, ID_ELEMENT)
            .unwrap_or_else(|| panic!("case {case:?}: no <{ID_ELEMENT}> in {body}"));
        assert!(
            virtual_interface_id.starts_with(&format!("{LOCAL_GATEWAY_VIF_ID_PREFIX}-")),
            "case {case:?}: expected an id minted with the virtual interface prefix, got \
             {virtual_interface_id:?}"
        );
        assert!(
            !minted.contains(&virtual_interface_id),
            "case {case:?}: expected a fresh interface id, got the already-minted \
             {virtual_interface_id:?}"
        );

        // The members a successful create reports, and the same members as the reader answers
        // with: the ARN names the Region the REQUEST was signed for, never the group's own.
        let mut expected = vec![
            (
                "localGatewayVirtualInterfaceArn",
                interface_arn(REGION, &virtual_interface_id),
            ),
            (
                "localGatewayVirtualInterfaceGroupId",
                group_id.clone(),
            ),
            ("localGatewayId", row.local_gateway_id.to_string()),
            ("outpostLagId", case_row.wire.outpost_lag_id.to_string()),
            ("vlan", case_row.wire.vlan.to_string()),
            ("localAddress", case_row.wire.local_address.to_string()),
            ("peerAddress", case_row.wire.peer_address.to_string()),
            ("ownerId", engine::emulator_account_id().to_string()),
            (
                "configurationState",
                LOCAL_GATEWAY_VIF_STATE_PENDING.to_string(),
            ),
        ];
        if let Some(asn) = row.local_bgp_asn {
            expected.push(("localBgpAsn", asn.to_string()));
        }
        if let Some(asn) = case_row.wire.peer_bgp_asn {
            expected.push(("peerBgpAsn", asn.to_string()));
        }
        if let Some(asn) = case_row.wire.peer_bgp_asn_extended {
            expected.push(("peerBgpAsnExtended", asn.to_string()));
        }
        for (member, value) in &expected {
            assert_member(&case, &body, member, value);
        }
        for (key, value) in &case_row.tags {
            assert_member(&case, &body, "key", key);
            assert_member(&case, &body, "value", value);
        }

        // The round trip: the interface reads back through the describe with the same members,
        // so the create answered the record it stored rather than its request.
        let (read_status, read_body) = describe(
            &router,
            &indexed(
                "LocalGatewayVirtualInterfaceId",
                &[virtual_interface_id.as_str()],
            ),
        )
        .await;
        assert_eq!(
            read_status, 200,
            "case {case:?}: expected HTTP 200 from {ACTION}, got {read_status} with body \
             {read_body}"
        );
        assert_eq!(
            elements(&read_body, ID_ELEMENT),
            vec![virtual_interface_id.clone()],
            "case {case:?}: expected {ACTION} to report exactly the created interface, got \
             {read_body}"
        );
        for (member, value) in &expected {
            assert_member(&case, &read_body, member, value);
        }
        for (key, value) in &case_row.tags {
            assert_member(&case, &read_body, "key", key);
            assert_member(&case, &read_body, "value", value);
        }

        // A virtual interface is created IN a group, and a group reports the interfaces it
        // holds, so the group it was created in gained exactly this id.
        assert!(
            group_interface_ids(&engine, group_id).contains(&virtual_interface_id),
            "case {case:?}: the group must report the interface created in it, got {:?}",
            group_interface_ids(&engine, group_id)
        );

        minted.push(virtual_interface_id);
    }

    assert_eq!(
        stored_interfaces(&engine),
        cases.len(),
        "every created interface must be stored once"
    );
    // An absent optional member is absent rather than empty: the group with no local BGP ASN
    // reports none on its interface either.
    let (status, body) = describe(&router, &indexed("LocalGatewayVirtualInterfaceId", &[minted[1].as_str()])).await;
    assert_eq!(
        status, 200,
        "the ASN-less group: expected HTTP 200, got {status} with body {body}"
    );
    assert!(
        !body.contains("<localBgpAsn>"),
        "the ASN-less group: an interface of a group with no local BGP ASN must report none, \
         got {body}"
    );
}

/// A new interface is answered in the non-terminal `pending` configuration state, and the clock
/// owes it `available` one `TRANSITION_DELAY` later whether or not anybody calls again.
#[tokio::test]
async fn an_interface_starts_pending_and_the_clock_owes_it_available() {
    let (router, engine) = super::ec2_query::emulator();
    let groups = seed_all(&engine);

    let (status, body) = create(&router, &well_formed(&groups[0])).await;
    assert_eq!(
        status, 200,
        "the clock: expected HTTP 200 from {CREATE}, got {status} with body {body}"
    );
    assert_eq!(
        element(&body, "configurationState").as_deref(),
        Some(LOCAL_GATEWAY_VIF_STATE_PENDING),
        "the clock: a new interface starts in the transient state, got {body}"
    );
    let virtual_interface_id =
        element(&body, ID_ELEMENT).unwrap_or_else(|| panic!("the clock: no <{ID_ELEMENT}> in {body}"));

    tokio::time::sleep(past_the_transition()).await;

    let (read_status, read_body) = describe(&router, &[]).await;
    assert_eq!(
        read_status, 200,
        "after the transition: expected HTTP 200, got {read_status} with body {read_body}"
    );
    assert_eq!(
        elements(&read_body, ID_ELEMENT),
        vec![virtual_interface_id],
        "after the transition: expected the one interface, got {read_body}"
    );
    assert_eq!(
        element(&read_body, "configurationState").as_deref(),
        Some(LOCAL_GATEWAY_VIF_STATE_AVAILABLE),
        "after the transition: the clock owed the interface \
         {LOCAL_GATEWAY_VIF_STATE_AVAILABLE:?}, got {read_body}"
    );
    assert_eq!(
        stored_interfaces(&engine),
        1,
        "the clock may not store a second interface"
    );
}

// ---------------------------------------------------------------------------
// The group named must be a group of the account
// ---------------------------------------------------------------------------

/// A `LocalGatewayVirtualInterfaceGroupId` that is not a virtual interface group id of the
/// account is refused before anything is written: one that is not a group id at all with
/// `InvalidLocalGatewayVirtualInterfaceGroupID.Malformed`, and a well-formed one the account
/// holds no group for with `InvalidLocalGatewayVirtualInterfaceGroupID.NotFound` — whatever
/// else the request carries, the interface being created IN that group.
#[tokio::test]
async fn a_virtual_interface_group_the_account_does_not_hold_is_refused() {
    let (router, engine) = super::ec2_query::emulator();
    let groups = seed_all(&engine);

    // Rows: the long and the short well-formed id form (both unknown), then ids that are not a
    // virtual interface group id at all — the interface kind this action mints, the association
    // kind of this surface, another EC2 resource kind, a bare prefix, a suffix that is not
    // hexadecimal, and free text.
    let cases = [
        (
            "lgw-vif-grp-0123456789abcdef0",
            "InvalidLocalGatewayVirtualInterfaceGroupID.NotFound",
        ),
        (
            "lgw-vif-grp-01234567",
            "InvalidLocalGatewayVirtualInterfaceGroupID.NotFound",
        ),
        (
            "lgw-vif-0123456789abcdef0",
            "InvalidLocalGatewayVirtualInterfaceGroupID.Malformed",
        ),
        (
            "lgw-vif-grp-assoc-0123456789abcdef0",
            "InvalidLocalGatewayVirtualInterfaceGroupID.Malformed",
        ),
        (
            "lgw-rtb-0123456789abcdef0",
            "InvalidLocalGatewayVirtualInterfaceGroupID.Malformed",
        ),
        (
            "lgw-vif-grp-",
            "InvalidLocalGatewayVirtualInterfaceGroupID.Malformed",
        ),
        (
            "lgw-vif-grp-zzzzzzzzzzzzzzzzz",
            "InvalidLocalGatewayVirtualInterfaceGroupID.Malformed",
        ),
        (
            "not-a-group-id",
            "InvalidLocalGatewayVirtualInterfaceGroupID.Malformed",
        ),
    ];
    for (group_id, expected_code) in cases {
        let case = group_id.to_string();
        let (status, body) = create(&router, &well_formed(group_id)).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {case:?}: expected the code {expected_code:?}, got {body}"
        );
        if expected_code.ends_with(".NotFound") {
            assert_eq!(
                error_message(&body),
                format!("The local-gateway-virtual-interface-group ID '{group_id}' does not exist"),
                "case {case:?}: the refusal must name the id the request carried, got {body}"
            );
        }
    }

    assert_eq!(
        stored_interfaces(&engine),
        0,
        "a refused create must store nothing"
    );

    // The stored group still takes an interface, so the refusals above were about the id.
    let (status, body) = create(&router, &well_formed(&groups[0])).await;
    assert_eq!(
        status, 200,
        "the stored group: expected HTTP 200, got {status} with body {body}"
    );
}

/// One row of the missing-parameter table: what the case is called, the form fields to send,
/// and the error code the request must be refused with.
type MissingParameterCase = (&'static str, Vec<(String, String)>, &'static str);

/// A required parameter the request carried as an EMPTY value is refused with EC2's
/// `MissingParameter` naming it — every one of them is modelled `clientOptional`, so an empty
/// one reaches the handler rather than the boundary validator — and that refusal is answered
/// whether or not the request only asks about permission. A parameter left out ALTOGETHER is a
/// null the boundary validator refuses before the handler, with the framework's own
/// `MalformedInput`, as every other operation of this service answers it.
#[tokio::test]
async fn a_required_parameter_the_request_left_out_is_refused() {
    let (router, engine) = super::ec2_query::emulator();
    let groups = seed_all(&engine);

    /// The one request field named `name` replaced by `value`.
    fn with(group_id: &str, name: &str, value: &str) -> Vec<(String, String)> {
        well_formed(group_id)
            .into_iter()
            .map(|(field, held)| {
                if field == name {
                    (field, value.to_string())
                } else {
                    (field, held)
                }
            })
            .collect()
    }

    /// The well-formed request without the field named `name`.
    fn without(group_id: &str, name: &str) -> Vec<(String, String)> {
        well_formed(group_id)
            .into_iter()
            .filter(|(field, _)| field != name)
            .collect()
    }

    // Rows: each required member given empty (the handler's refusal — a VLAN of zero is the
    // integer form of "not given", 0 being no VLAN at all), then each left out entirely (the
    // boundary validator's).
    let mut cases: Vec<MissingParameterCase> = Vec::new();
    for field in [
        "LocalGatewayVirtualInterfaceGroupId",
        "OutpostLagId",
        "LocalAddress",
        "PeerAddress",
    ] {
        cases.push((field, with(&groups[0], field, ""), "MissingParameter"));
        cases.push((field, without(&groups[0], field), "MalformedInput"));
    }
    cases.push(("Vlan", with(&groups[0], "Vlan", "0"), "MissingParameter"));
    cases.push(("Vlan", without(&groups[0], "Vlan"), "MalformedInput"));

    for (case, params, expected_code) in cases {
        for dry_run in [false, true] {
            let case = format!("{case} ({expected_code}, DryRun={dry_run})");
            let mut params = params.clone();
            if dry_run {
                params.push(param("DryRun", "true"));
            }
            let (status, body) = create(&router, &params).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400, got {status} with body {body}"
            );
            assert_eq!(
                error_code(&body),
                expected_code,
                "case {case:?}: expected the code {expected_code:?}, got {body}"
            );
        }
    }
    assert_eq!(
        stored_interfaces(&engine),
        0,
        "a refused create must store nothing"
    );
}

/// One row of the dry-run table: what the case is called, the form fields to send, and the
/// status and code the request must be answered with once `DryRun=true` is added.
type DryRunCase = (&'static str, Vec<(String, String)>, u16, &'static str);

/// A dry run asks about PERMISSION rather than about the group named, so it reports
/// `DryRunOperation` whatever the request would have gone on to LOOK UP — and writes nothing.
/// A request that is not well formed is refused with its own reason even so: an id that is not
/// a virtual interface group id at all is read before the flag. The same requests without the
/// flag then take the group and the values into account.
#[tokio::test]
async fn a_dry_run_reports_it_would_have_succeeded_and_writes_nothing() {
    let (router, engine) = super::ec2_query::emulator();
    let groups = seed_all(&engine);

    // Rows: the request that would have succeeded, one naming a group the account does not
    // hold, one with a VLAN outside the ids a tagged frame can carry, and one with an address
    // that is not IPv4 — a dry run of each reports the same permission answer, every one of
    // those being something the handler would have gone on to look up or read. The last row is
    // the exception the crate's service-wide rule names: a value that is not a group id at all
    // is refused with ITS OWN reason, `DryRun` or not.
    let cases: Vec<DryRunCase> = vec![
        (
            "a request that would succeed",
            well_formed(&groups[0]),
            412,
            "DryRunOperation",
        ),
        (
            "an unknown group",
            well_formed("lgw-vif-grp-0123456789abcdef0"),
            412,
            "DryRunOperation",
        ),
        (
            "a VLAN outside the range",
            request(
                &groups[0],
                &Wire {
                    outpost_lag_id: "ola-00000000000000001",
                    vlan: 4095,
                    local_address: "10.0.0.1",
                    peer_address: "10.0.0.2",
                    peer_bgp_asn: None,
                    peer_bgp_asn_extended: None,
                },
            ),
            412,
            "DryRunOperation",
        ),
        (
            "an address that is not IPv4",
            request(
                &groups[0],
                &Wire {
                    outpost_lag_id: "ola-00000000000000001",
                    vlan: 100,
                    local_address: "2001:db8::1",
                    peer_address: "10.0.0.2",
                    peer_bgp_asn: None,
                    peer_bgp_asn_extended: None,
                },
            ),
            412,
            "DryRunOperation",
        ),
        (
            "a malformed group id",
            well_formed("not-a-group-id"),
            400,
            "InvalidLocalGatewayVirtualInterfaceGroupID.Malformed",
        ),
    ];
    for (case, params, expected_status, expected_code) in cases {
        let mut params = params;
        params.push(param("DryRun", "true"));
        let (status, body) = create(&router, &params).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {case:?}: expected the code {expected_code:?}, got {body}"
        );
        assert_eq!(
            stored_interfaces(&engine),
            0,
            "case {case:?}: a dry run may write nothing"
        );
    }

    let (status, body) = create(&router, &well_formed(&groups[0])).await;
    assert_eq!(
        status, 200,
        "after the dry run: expected HTTP 200, got {status} with body {body}"
    );
    assert_eq!(
        stored_interfaces(&engine),
        1,
        "after the dry run: the interface must be stored exactly once"
    );
}

// ---------------------------------------------------------------------------
// The values the interface carries
// ---------------------------------------------------------------------------

/// The VLAN must be one of the ids an 802.1Q tag can carry: 1 to 4094, with 0 reading as the
/// parameter not given and everything outside refused with `InvalidParameterValue`.
#[tokio::test]
async fn a_vlan_outside_the_ids_a_tagged_frame_can_carry_is_refused() {
    let (router, engine) = super::ec2_query::emulator();
    let groups = seed_all(&engine);

    // Rows: both published bounds, one inside each, one past each bound, the integer form of
    // "not given", and a value far outside.
    let cases: Vec<(i32, u16, &str)> = vec![
        (1, 200, ""),
        (2, 200, ""),
        (4093, 200, ""),
        (4094, 200, ""),
        (0, 400, "MissingParameter"),
        (-1, 400, "InvalidParameterValue"),
        (4095, 400, "InvalidParameterValue"),
        (4096, 400, "InvalidParameterValue"),
        (65535, 400, "InvalidParameterValue"),
    ];
    let mut accepted = 0;
    for (vlan, expected_status, expected_code) in cases {
        let case = format!("VLAN {vlan}");
        let params = request(
            &groups[0],
            &Wire {
                outpost_lag_id: "ola-00000000000000001",
                vlan,
                local_address: "10.0.0.1",
                peer_address: "10.0.0.2",
                peer_bgp_asn: None,
                peer_bgp_asn_extended: None,
            },
        );
        let (status, body) = create(&router, &params).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} with body {body}"
        );
        if expected_status == 200 {
            accepted += 1;
            assert_eq!(
                element(&body, "vlan").as_deref(),
                Some(vlan.to_string().as_str()),
                "case {case:?}: the interface must report the VLAN asked for, got {body}"
            );
        } else {
            assert_eq!(
                error_code(&body),
                expected_code,
                "case {case:?}: expected the code {expected_code:?}, got {body}"
            );
        }
    }
    assert_eq!(
        stored_interfaces(&engine),
        accepted,
        "only the VLANs inside the range may be stored"
    );
}

/// "Only IPv4 is supported" for both addresses: anything that is not a dotted quad is refused
/// with `InvalidParameterValue` naming the parameter it arrived in, and every dotted quad is
/// accepted and reported back.
#[tokio::test]
async fn an_address_that_is_not_ipv4_is_refused() {
    let (router, engine) = super::ec2_query::emulator();
    let groups = seed_all(&engine);

    // Rows: addresses at both ends of the IPv4 space, then an IPv6 address, an address with a
    // prefix length, a three-octet address, an octet outside a byte, a leading-zero octet and
    // free text — each tried in BOTH address members, so neither member is the specification.
    let cases: Vec<(&str, bool)> = vec![
        ("0.0.0.0", true),
        ("10.0.0.1", true),
        ("255.255.255.255", true),
        ("2001:db8::1", false),
        ("10.0.0.1/30", false),
        ("10.0.0", false),
        ("10.0.0.256", false),
        ("10.0.0.01", false),
        ("not-an-address", false),
    ];
    let mut accepted = 0;
    for (address, valid) in cases {
        for parameter in ["LocalAddress", "PeerAddress"] {
            let case = format!("{parameter}={address}");
            let params: Vec<(String, String)> = well_formed(&groups[0])
                .into_iter()
                .map(|(field, held)| {
                    if field == parameter {
                        (field, address.to_string())
                    } else {
                        (field, held)
                    }
                })
                .collect();
            let (status, body) = create(&router, &params).await;
            let expected_status = if valid { 200 } else { 400 };
            assert_eq!(
                status, expected_status,
                "case {case:?}: expected HTTP {expected_status}, got {status} with body {body}"
            );
            if valid {
                accepted += 1;
            } else {
                assert_eq!(
                    error_code(&body),
                    "InvalidParameterValue",
                    "case {case:?}: unexpected error code in {body}"
                );
                let message = error_message(&body);
                assert!(
                    message.contains(address),
                    "case {case:?}: the refusal must name the value the request carried, got \
                     {message:?}"
                );
            }
        }
    }
    assert_eq!(
        stored_interfaces(&engine),
        accepted,
        "only the requests naming two IPv4 addresses may be stored"
    );
}

/// One row of the peer-ASN table: what the case is called, the ASN in each of the two members,
/// and the status and code the request must be answered with.
type PeerAsnCase = (&'static str, Option<i32>, Option<i64>, u16, &'static str);

/// The peer ASN is ONE number, given in `PeerBgpAsn` or in the extended member that widens its
/// range: a request naming both is refused with `InvalidParameterCombination`, and a value
/// outside the 32-bit ASN space with `InvalidParameterValue`.
#[tokio::test]
async fn a_peer_asn_in_both_members_or_outside_the_asn_space_is_refused() {
    let (router, engine) = super::ec2_query::emulator();
    let groups = seed_all(&engine);

    // Rows: the lower bound and a 16-bit ASN in the narrow member, the lower bound and the
    // upper bound of the 32-bit space in the extended one, no ASN at all, then zero and a
    // negative in each member, one past the extended upper bound, and both members at once.
    let cases: Vec<PeerAsnCase> = vec![
        ("the lower bound", Some(1), None, 200, ""),
        ("a 16-bit ASN", Some(65_535), None, 200, ""),
        ("an extended lower bound", None, Some(1), 200, ""),
        (
            "the extended upper bound",
            None,
            Some(4_294_967_295),
            200,
            "",
        ),
        ("no ASN at all", None, None, 200, ""),
        ("zero", Some(0), None, 400, "InvalidParameterValue"),
        ("a negative ASN", Some(-1), None, 400, "InvalidParameterValue"),
        (
            "zero, extended",
            None,
            Some(0),
            400,
            "InvalidParameterValue",
        ),
        (
            "one past the extended upper bound",
            None,
            Some(4_294_967_296),
            400,
            "InvalidParameterValue",
        ),
        (
            "both members",
            Some(65_000),
            Some(65_000),
            400,
            "InvalidParameterCombination",
        ),
    ];
    let mut accepted = 0;
    for (case, peer_bgp_asn, peer_bgp_asn_extended, expected_status, expected_code) in cases {
        let params = request(
            &groups[0],
            &Wire {
                outpost_lag_id: "ola-00000000000000001",
                vlan: 100,
                local_address: "10.0.0.1",
                peer_address: "10.0.0.2",
                peer_bgp_asn,
                peer_bgp_asn_extended,
            },
        );
        let (status, body) = create(&router, &params).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} with body {body}"
        );
        if expected_status == 200 {
            accepted += 1;
            if let Some(asn) = peer_bgp_asn {
                assert_member(case, &body, "peerBgpAsn", &asn.to_string());
            }
            if let Some(asn) = peer_bgp_asn_extended {
                assert_member(case, &body, "peerBgpAsnExtended", &asn.to_string());
            }
        } else {
            assert_eq!(
                error_code(&body),
                expected_code,
                "case {case:?}: expected the code {expected_code:?}, got {body}"
            );
        }
    }
    assert_eq!(
        stored_interfaces(&engine),
        accepted,
        "only the requests naming one ASN inside the space may be stored"
    );
}

/// A `TagSpecification` may name only the resource type this action creates; any other is
/// refused with `InvalidParameterValue` rather than accepted and discarded.
#[tokio::test]
async fn a_tag_specification_naming_another_resource_type_is_refused() {
    let (router, engine) = super::ec2_query::emulator();
    let groups = seed_all(&engine);

    // Rows from `ResourceType`'s own members: the group this request names, the sibling
    // resources of this surface, and the type this action actually creates.
    let cases: Vec<(&str, u16)> = vec![
        ("local-gateway-virtual-interface-group", 400),
        ("local-gateway-route-table", 400),
        ("local-gateway", 400),
        ("local-gateway-virtual-interface", 200),
    ];
    for (resource_type, expected_status) in cases {
        let mut params = well_formed(&groups[0]);
        params.extend(tag_specification(resource_type, &[("Name", "tagged")]));
        let (status, body) = create(&router, &params).await;
        assert_eq!(
            status, expected_status,
            "case {resource_type:?}: expected HTTP {expected_status}, got {status} with body \
             {body}"
        );
        if expected_status == 400 {
            assert_eq!(
                error_code(&body),
                "InvalidParameterValue",
                "case {resource_type:?}: unexpected error code in {body}"
            );
        }
    }
    assert_eq!(
        stored_interfaces(&engine),
        1,
        "only the specification naming this action's own resource type may be stored"
    );

    // The one accepted specification's tags are the interface's, and they read back through the
    // describe.
    let (status, body) = describe(&router, &[]).await;
    assert_eq!(
        status, 200,
        "the tagged interface: expected HTTP 200, got {status} with body {body}"
    );
    for (member, value) in [("key", "Name"), ("value", "tagged")] {
        assert_member("the tagged interface", &body, member, value);
    }
}

// ---------------------------------------------------------------------------
// The store is what is reported
// ---------------------------------------------------------------------------

/// Every state the seeded rows pin is a value the model publishes, so the table above is
/// written against the modelled enum rather than against prose.
#[test]
fn every_seeded_state_is_a_modelled_configuration_state() {
    for row in seeded_rows() {
        let state = row.configuration_state;
        assert!(
            ec2::ops::types::LOCAL_GATEWAY_VIRTUAL_INTERFACE_CONFIGURATION_STATE_VALUES
                .contains(&state),
            "case {state:?}: expected a value of \
             LocalGatewayVirtualInterfaceConfigurationState, got {state:?}"
        );
    }
}

/// An account with no Outpost holds no virtual interface — a local gateway and the
/// interfaces on it come with an Outpost rather than from any EC2 call — so the operation
/// answers HTTP 200 with an empty set, and describing is a READ: two calls report the same
/// thing and the store is untouched.
#[tokio::test]
async fn an_account_with_no_outpost_reports_an_empty_set_and_describing_writes_nothing() {
    let (router, engine) = super::ec2_query::emulator();

    let (status, body) = describe(&router, &[]).await;
    assert_eq!(
        status, 200,
        "fresh account: expected HTTP 200, got {status} with body {body}"
    );
    let reported = elements(&body, ID_ELEMENT);
    assert!(
        reported.is_empty(),
        "fresh account: expected no virtual interface, got {reported:?} from {body}"
    );
    assert!(
        element(&body, "nextToken").is_none(),
        "fresh account: expected no pagination token, got {body}"
    );

    let again = described_ids(&router, "a second call", &[]).await;
    assert!(
        again.is_empty(),
        "a second call: expected no virtual interface, got {again:?}"
    );
    assert_eq!(
        engine
            .state::<Ec2State>()
            .lock()
            .local_gateway_virtual_interfaces
            .len(),
        0,
        "describing virtual interfaces must not write to the store"
    );
}

/// Every virtual interface the store holds is reported, in store order, with every member of
/// the modelled shape — and one written after a call shows up in the next one, so the answer
/// is computed from the store rather than fixed.
#[tokio::test]
async fn every_stored_interface_is_reported_and_a_later_write_is_visible() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    let (status, body) = describe(&router, &[]).await;
    assert_eq!(
        status, 200,
        "all interfaces: expected HTTP 200, got {status} with body {body}"
    );
    let reported = elements(&body, ID_ELEMENT);
    assert_eq!(
        reported, seeded,
        "all interfaces: expected every stored virtual interface in store order, got \
         {reported:?}"
    );

    // Every member of the reported shape comes from the stored virtual interface: the local
    // gateway and group, the ARN built from the Region its writer was signed for, the owning
    // account, the Outpost LAG, the VLAN, the addresses, the ASNs, the configuration state
    // and the tags.
    for (index, row) in seeded_rows().iter().enumerate() {
        let case = format!("row {index}");
        assert!(
            ec2::state::local_gateway_virtual_interface_id_is_well_formed(&seeded[index]),
            "case {case:?}: expected a well-formed virtual interface id, got {:?}",
            seeded[index]
        );
        let mut expected: Vec<(&str, String)> = vec![
            ("localGatewayId", row.local_gateway_id.to_string()),
            (ID_ELEMENT, seeded[index].clone()),
            (
                "localGatewayVirtualInterfaceArn",
                interface_arn(row.region, &seeded[index]),
            ),
            ("ownerId", engine::emulator_account_id().to_string()),
            ("configurationState", row.configuration_state.to_string()),
        ];
        for (member, value) in [
            ("localGatewayVirtualInterfaceGroupId", row.group_id),
            ("outpostLagId", row.outpost_lag_id),
            ("localAddress", row.local_address),
            ("peerAddress", row.peer_address),
        ] {
            if let Some(value) = value {
                expected.push((member, value.to_string()));
            }
        }
        for (member, value) in [
            ("vlan", row.vlan),
            ("localBgpAsn", row.local_bgp_asn),
            ("peerBgpAsn", row.peer_bgp_asn),
        ] {
            if let Some(value) = value {
                expected.push((member, value.to_string()));
            }
        }
        if let Some(asn) = row.peer_bgp_asn_extended {
            expected.push(("peerBgpAsnExtended", asn.to_string()));
        }
        for (member, value) in expected {
            assert!(
                body.contains(&format!("<{member}>{value}</{member}>")),
                "case {case:?}: expected <{member}>{value}</{member}>, got {body}"
            );
        }
        for stored_tag in &row.tags {
            let key = stored_tag.key.clone().unwrap_or_default();
            let value = stored_tag.value.clone().unwrap_or_default();
            assert!(
                body.contains(&format!("<key>{key}</key>")),
                "case {case:?}: expected the tag key {key:?} to be reported, got {body}"
            );
            assert!(
                body.contains(&format!("<value>{value}</value>")),
                "case {case:?}: expected the tag value {value:?} to be reported, got {body}"
            );
        }
    }

    let late_id = engine
        .state::<Ec2State>()
        .lock()
        .insert_local_gateway_virtual_interface(InterfaceSpec {
            local_gateway_id: "lgw-00000000000000009".to_string(),
            local_gateway_virtual_interface_group_id: Some(
                "lgw-vif-grp-00000000000000009".to_string(),
            ),
            outpost_lag_id: Some("ola-00000000000000009".to_string()),
            vlan: Some(909),
            local_address: Some("10.9.0.1".to_string()),
            peer_address: Some("10.9.0.2".to_string()),
            local_bgp_asn: Some(65010),
            peer_bgp_asn: Some(65011),
            peer_bgp_asn_extended: None,
            region: REGION.to_string(),
            configuration_state: "available".to_string(),
            tags: vec![tag("Name", "late")],
        })
        .expect("after a later write: the store must take the virtual interface");
    let after = described_ids(&router, "after a later write", &[]).await;
    let mut expected = seeded.clone();
    expected.push(late_id.clone());
    assert_eq!(
        after, expected,
        "after a later write: expected {late_id:?} to be reported too, got {after:?}"
    );
}

// ---------------------------------------------------------------------------
// Filters
// ---------------------------------------------------------------------------

/// Each documented filter selects on its own attribute, values within one filter are OR'd,
/// and the comparison is exact and case-sensitive.
#[tokio::test]
async fn each_documented_filter_selects_the_interfaces_whose_attribute_equals_a_value() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);
    let account = engine::emulator_account_id().to_string();
    let all: Vec<usize> = (0..seeded.len()).collect();

    // One row per filter name `DescribeLocalGatewayVirtualInterfacesRequest$Filters`
    // documents, each asserted on a virtual interface the issue never cited, plus rows for
    // several values, a value nothing holds, the wrong case, and the row that holds no value
    // for the attribute at all.
    let cases: Vec<(&str, Vec<String>, Vec<usize>)> = vec![
        ("local-address", vec!["10.0.0.1".to_string()], vec![0]),
        (
            "local-address",
            vec!["10.1.0.1".to_string(), "10.2.0.5".to_string()],
            vec![3, 5],
        ),
        ("local-address", vec!["10.9.9.9".to_string()], Vec::new()),
        ("local-bgp-asn", vec!["64514".to_string()], vec![4, 5]),
        ("local-bgp-asn", vec!["64512".to_string()], vec![0, 1]),
        ("local-bgp-asn", vec!["99999".to_string()], Vec::new()),
        (
            "local-gateway-id",
            vec!["lgw-00000000000000001".to_string()],
            vec![0, 1, 2],
        ),
        (
            "local-gateway-id",
            vec![
                "lgw-00000000000000003".to_string(),
                "lgw-00000000000000004".to_string(),
            ],
            vec![4, 5, 6],
        ),
        (
            "local-gateway-id",
            vec!["lgw-00000000000000099".to_string()],
            Vec::new(),
        ),
        (
            "local-gateway-virtual-interface-id",
            vec![seeded[3].clone()],
            vec![3],
        ),
        (
            "local-gateway-virtual-interface-id",
            vec![seeded[6].clone(), seeded[1].clone()],
            vec![1, 6],
        ),
        (
            "local-gateway-virtual-interface-id",
            vec![seeded[0].to_uppercase()],
            Vec::new(),
        ),
        ("owner-id", vec![account.clone()], all.clone()),
        // An account id the emulator does not use (its own is
        // `engine::emulator_account_id()`), so no virtual interface is owned by it.
        ("owner-id", vec!["123456789012".to_string()], Vec::new()),
        ("peer-address", vec!["10.0.0.6".to_string()], vec![1]),
        (
            "peer-address",
            vec!["10.2.0.2".to_string(), "10.3.0.2".to_string()],
            vec![4, 6],
        ),
        ("peer-bgp-asn", vec!["65001".to_string()], vec![0, 4]),
        ("peer-bgp-asn", vec!["65004".to_string()], vec![5]),
        ("vlan", vec!["101".to_string()], vec![0, 5]),
        ("vlan", vec!["102".to_string()], vec![1]),
        // The interface with no VLAN holds no value for this attribute, so no value can
        // select it.
        ("vlan", vec!["0".to_string()], Vec::new()),
    ];

    for (name, values, expected_indices) in cases {
        let borrowed: Vec<&str> = values.iter().map(String::as_str).collect();
        let case = format!("{name}={values:?}");
        let reported = described_ids(&router, &case, &filters(&[(name, &borrowed)])).await;
        let expected = ids_at(&seeded, &expected_indices);
        assert_eq!(
            reported, expected,
            "case {case:?}: expected {expected:?}, got {reported:?}"
        );
    }
}

/// One row of the AND table: the filters to send, and the indices of the virtual interfaces
/// that must come back.
type FilterCombination<'a> = (Vec<(&'a str, &'a [&'a str])>, Vec<usize>);

/// Several filters are joined with an AND: only the virtual interfaces matching every one
/// are reported.
#[tokio::test]
async fn several_filters_are_joined_with_an_and() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);
    let account = engine::emulator_account_id().to_string();
    let owner: [&str; 1] = [account.as_str()];

    let cases: Vec<FilterCombination<'_>> = vec![
        (
            vec![
                ("local-gateway-id", &["lgw-00000000000000001"][..]),
                ("vlan", &["102"][..]),
            ],
            vec![1],
        ),
        (
            vec![
                ("local-gateway-id", &["lgw-00000000000000001"][..]),
                ("vlan", &["201"][..]),
            ],
            Vec::new(),
        ),
        (
            vec![
                ("local-gateway-id", &["lgw-00000000000000003"][..]),
                ("owner-id", &owner[..]),
            ],
            vec![4, 5],
        ),
        (
            vec![
                ("local-bgp-asn", &["64514"][..]),
                ("peer-bgp-asn", &["65001", "65004"][..]),
                ("owner-id", &owner[..]),
            ],
            vec![4, 5],
        ),
        (
            vec![
                ("local-gateway-id", &["lgw-00000000000000003"][..]),
                ("peer-address", &["10.2.0.6"][..]),
                ("vlan", &["101"][..]),
            ],
            vec![5],
        ),
    ];

    for (entries, expected_indices) in cases {
        let case = format!("{entries:?}");
        let reported = described_ids(&router, &case, &filters(&entries)).await;
        let expected = ids_at(&seeded, &expected_indices);
        assert_eq!(
            reported, expected,
            "case {case:?}: expected {expected:?}, got {reported:?}"
        );
    }
}

/// A filter name this operation does not document is refused with EC2's
/// `InvalidParameterValue`, rather than silently reported as an empty set.
#[tokio::test]
async fn an_undefined_filter_name_is_refused() {
    let (router, engine) = super::ec2_query::emulator();
    seed(&engine);

    // Rows: a member this operation's `Filters` doc does NOT name a filter for even though
    // the shape carries it (the virtual interface group, the Outpost LAG, the configuration
    // state), the underscore spelling, a documented name with the wrong case, the plural, a
    // near-miss, the tag filters it does not name either (even though the interface carries
    // tags), and an empty name.
    let cases = [
        "local-gateway-virtual-interface-group-id",
        "outpost-lag-id",
        "configuration-state",
        "local_address",
        "Local-Address",
        "local-addresses",
        "local-gateway-virtual-interface",
        "tag-key",
        "tag:Name",
        "tag:",
        "",
    ];
    for name in cases {
        let (status, body) = describe(&router, &filters(&[(name, &["x"])])).await;
        assert_eq!(
            status, 400,
            "case {name:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidParameterValue",
            "case {name:?}: expected InvalidParameterValue, got {body}"
        );
        // The EC2 error envelope is XML, so the message's apostrophes arrive escaped.
        assert!(
            unescaped(&body).contains(&format!("The filter '{name}' is invalid")),
            "case {name:?}: the refusal must name the filter, got {body}"
        );
        assert!(
            elements(&body, ID_ELEMENT).is_empty(),
            "case {name:?}: a refused request reports no virtual interface, got {body}"
        );
    }
}

/// One row of the bounds table: what the case is called, the form fields to send, and the
/// HTTP status the request must answer with.
type BoundsCase = (String, Vec<(String, String)>, u16);

/// The request-controlled factors of the store scan are bounded: each of the crate's caps is
/// accepted at its published value and refused one past it, so no request can ask for
/// unbounded filtering work.
#[tokio::test]
async fn the_filter_and_id_factors_are_accepted_at_their_caps_and_refused_past_them() {
    let (router, engine) = super::ec2_query::emulator();
    seed(&engine);

    // Rows: the number of filters, the values in one filter, the bytes in one value and the
    // number of virtual interface ids — each at the crate's cap (`ec2::state`) and one past
    // it.
    let many_filters = |count: usize| -> Vec<(String, String)> {
        let entries: Vec<(&str, &[&str])> = (0..count)
            .map(|_| ("local-gateway-id", &["lgw-00000000000000001"][..]))
            .collect();
        filters(&entries)
    };
    let many_values = |count: usize| -> Vec<(String, String)> {
        let values: Vec<String> = (0..count).map(|index| format!("lgw-{index}")).collect();
        let borrowed: Vec<&str> = values.iter().map(String::as_str).collect();
        filters(&[("local-gateway-id", &borrowed)])
    };
    let one_value_of = |bytes: usize| -> Vec<(String, String)> {
        let value = "a".repeat(bytes);
        filters(&[("local-gateway-id", &[value.as_str()])])
    };
    let many_ids = |count: usize| -> Vec<(String, String)> {
        let ids: Vec<String> = (0..count)
            .map(|index| format!("lgw-vif-{index:017x}"))
            .collect();
        let borrowed: Vec<&str> = ids.iter().map(String::as_str).collect();
        indexed("LocalGatewayVirtualInterfaceId", &borrowed)
    };

    let cases: Vec<BoundsCase> = vec![
        (
            format!("{} filters", ec2::state::MAX_FILTERS),
            many_filters(ec2::state::MAX_FILTERS),
            200,
        ),
        (
            format!("{} filters", ec2::state::MAX_FILTERS + 1),
            many_filters(ec2::state::MAX_FILTERS + 1),
            400,
        ),
        (
            format!("{} filter values", ec2::state::MAX_FILTER_VALUES),
            many_values(ec2::state::MAX_FILTER_VALUES),
            200,
        ),
        (
            format!("{} filter values", ec2::state::MAX_FILTER_VALUES + 1),
            many_values(ec2::state::MAX_FILTER_VALUES + 1),
            400,
        ),
        (
            format!("a {}-byte filter value", ec2::state::MAX_FILTER_VALUE_BYTES),
            one_value_of(ec2::state::MAX_FILTER_VALUE_BYTES),
            200,
        ),
        (
            format!(
                "a {}-byte filter value",
                ec2::state::MAX_FILTER_VALUE_BYTES + 1
            ),
            one_value_of(ec2::state::MAX_FILTER_VALUE_BYTES + 1),
            400,
        ),
        (
            format!("{} ids", ec2::state::MAX_RESOURCE_IDS),
            many_ids(ec2::state::MAX_RESOURCE_IDS),
            200,
        ),
        (
            format!("{} ids", ec2::state::MAX_RESOURCE_IDS + 1),
            many_ids(ec2::state::MAX_RESOURCE_IDS + 1),
            400,
        ),
    ];

    for (case, params, expected_status) in cases {
        let (status, body) = describe(&router, &params).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} with body {body}"
        );
        if expected_status == 400 {
            assert_eq!(
                error_code(&body),
                "InvalidParameterValue",
                "case {case:?}: expected InvalidParameterValue, got {body}"
            );
            assert!(
                elements(&body, ID_ELEMENT).is_empty(),
                "case {case:?}: a refused request reports no virtual interface, got {body}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// LocalGatewayVirtualInterfaceIds
// ---------------------------------------------------------------------------

/// The ids narrow the report to the virtual interfaces named, in store order whatever order
/// they were asked for; an id that names no virtual interface of the account contributes no
/// row.
#[tokio::test]
async fn the_ids_narrow_the_report_to_the_interfaces_named() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    // Rows differ in count and in the order the ids are asked for.
    let cases: Vec<Vec<usize>> = vec![
        vec![0],
        vec![6],
        vec![2, 4],
        vec![4, 2],
        vec![5, 1, 3],
        (0..seeded.len()).collect(),
    ];
    for asked in cases {
        let ids = ids_at(&seeded, &asked);
        let borrowed: Vec<&str> = ids.iter().map(String::as_str).collect();
        let case = format!("ids {asked:?}");
        let reported = described_ids(
            &router,
            &case,
            &indexed("LocalGatewayVirtualInterfaceId", &borrowed),
        )
        .await;
        let mut expected = asked.clone();
        expected.sort_unstable();
        assert_eq!(
            reported,
            ids_at(&seeded, &expected),
            "case {case:?}: expected the virtual interfaces named, in store order, got \
             {reported:?}"
        );
    }

    // An id of the right shape that names no virtual interface of the account, alone and
    // beside a stored one.
    let unknown = "lgw-vif-0000000000000beef";
    for (case, asked) in [
        ("an unknown id alone", vec![unknown.to_string()]),
        (
            "an unknown id beside a stored one",
            vec![unknown.to_string(), seeded[1].clone()],
        ),
    ] {
        let borrowed: Vec<&str> = asked.iter().map(String::as_str).collect();
        let reported = described_ids(
            &router,
            case,
            &indexed("LocalGatewayVirtualInterfaceId", &borrowed),
        )
        .await;
        let expected: Vec<String> = asked
            .iter()
            .filter(|id| id.as_str() != unknown)
            .cloned()
            .collect();
        assert_eq!(
            reported, expected,
            "case {case:?}: expected {expected:?}, got {reported:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Pagination
// ---------------------------------------------------------------------------

/// Whatever the page size, walking the pages covers every virtual interface exactly once, in
/// store order, and the last page carries no token.
#[tokio::test]
async fn paging_covers_every_interface_exactly_once_for_each_page_size() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    // `LocalGatewayMaxResults` is modelled `range 5..1000`, so the rows are its published
    // lower bound, a size that divides the store exactly, one that does not, and the upper
    // bound, which holds everything.
    for max_results in [5, 7, 6, 1000] {
        let case = format!("MaxResults={max_results}");
        let mut params = max_results_param(max_results);
        let mut walked: Vec<String> = Vec::new();
        let mut pages = 0;
        loop {
            let (status, body) = describe(&router, &params).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200, got {status} with body {body}"
            );
            let page = elements(&body, ID_ELEMENT);
            assert!(
                page.len() <= usize::try_from(max_results).unwrap_or_default(),
                "case {case:?}: expected at most {max_results} virtual interfaces in a page, \
                 got {page:?}"
            );
            walked.extend(page);
            pages += 1;
            assert!(
                pages <= seeded.len() + 1,
                "case {case:?}: paging must terminate, walked {walked:?}"
            );
            match element(&body, "nextToken") {
                Some(token) => {
                    params = max_results_param(max_results);
                    params.push(("NextToken".to_string(), token));
                }
                None => break,
            }
        }
        assert_eq!(
            walked, seeded,
            "case {case:?}: expected every virtual interface exactly once in store order, got \
             {walked:?}"
        );
    }
}

/// A page is a cut of the FILTERED set, not of the whole store: `MaxResults` and a filter
/// compose.
#[tokio::test]
async fn a_page_is_cut_from_the_filtered_set() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    // Six of the seven seeded virtual interfaces belong to these three local gateways, so
    // the model's minimum page size cuts the filtered set into two pages.
    let selected_gateways: &[&str] = &[
        "lgw-00000000000000001",
        "lgw-00000000000000002",
        "lgw-00000000000000003",
    ];
    let page_params = || {
        let mut params = filters(&[("local-gateway-id", selected_gateways)]);
        params.extend(max_results_param(5));
        params
    };

    let (status, body) = describe(&router, &page_params()).await;
    assert_eq!(
        status, 200,
        "filtered first page: expected HTTP 200, got {status} with body {body}"
    );
    let first = elements(&body, ID_ELEMENT);
    assert_eq!(
        first,
        ids_at(&seeded, &[0, 1, 2, 3, 4]),
        "filtered first page: expected the first five interfaces of those gateways, got \
         {first:?}"
    );
    let token = element(&body, "nextToken")
        .expect("filtered first page: a sixth virtual interface follows, so a token must be issued");

    let mut next = page_params();
    next.push(("NextToken".to_string(), token));
    let second = described_ids(&router, "filtered second page", &next).await;
    assert_eq!(
        second,
        ids_at(&seeded, &[5]),
        "filtered second page: expected the remaining interface of those gateways, got \
         {second:?}"
    );
}

/// `MaxResults` outside the range the model publishes is refused at the boundary; each
/// published bound itself is accepted.
#[tokio::test]
async fn max_results_is_accepted_at_its_bounds_and_refused_past_them() {
    let (router, engine) = super::ec2_query::emulator();
    seed(&engine);

    // The published bounds (`LocalGatewayMaxResults`, `range 5..1000`) and one past each.
    let cases = [(4, 400), (5, 200), (1000, 200), (1001, 400)];
    for (max_results, expected_status) in cases {
        let (status, body) = describe(&router, &max_results_param(max_results)).await;
        assert_eq!(
            status, expected_status,
            "case MaxResults={max_results}: expected HTTP {expected_status}, got {status} with \
             body {body}"
        );
    }
}

/// A `NextToken` this service did not issue is refused with `InvalidPaginationToken`, rather
/// than read as a cursor.
#[tokio::test]
async fn a_next_token_this_service_did_not_issue_is_refused() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    // Rows: free text, punctuation, a hex string this service never minted, the bare
    // resource id a caller might guess the token is, and that id hex-encoded (the shape the
    // token takes, without the marker and checksum a minted token carries).
    let forged_marker = ::hex::encode(format!("ec2-1:0000000000000000:{}", seeded[0]));
    let cases = [
        "not-a-token".to_string(),
        "###".to_string(),
        "6161".to_string(),
        seeded[0].clone(),
        ::hex::encode(&seeded[0]),
        forged_marker,
    ];
    for token in cases {
        let (status, body) = describe(&router, &[("NextToken".to_string(), token.clone())]).await;
        assert_eq!(
            status, 400,
            "case {token:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidPaginationToken",
            "case {token:?}: expected InvalidPaginationToken, got {body}"
        );
        // EC2's message for this refusal is one fixed sentence, naming no token.
        assert!(
            unescaped(&body).contains(ec2::state::INVALID_PAGINATION_TOKEN_MESSAGE),
            "case {token:?}: expected EC2's own pagination-token sentence, got {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// DryRun
// ---------------------------------------------------------------------------

/// `DryRun` reports the permitted outcome — `DryRunOperation`, HTTP 412 — and describes
/// nothing; `DryRun=false` is an ordinary call.
#[tokio::test]
async fn dry_run_reports_the_permitted_outcome_without_describing() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    let (status, body) = describe(&router, &[("DryRun".to_string(), "true".to_string())]).await;
    assert_eq!(
        status, 412,
        "DryRun=true: expected HTTP 412, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "DryRunOperation",
        "DryRun=true: expected DryRunOperation, got {body}"
    );
    assert!(
        elements(&body, ID_ELEMENT).is_empty(),
        "DryRun=true: a dry run reports no virtual interface, got {body}"
    );

    let reported = described_ids(
        &router,
        "DryRun=false",
        &[("DryRun".to_string(), "false".to_string())],
    )
    .await;
    assert_eq!(
        reported, seeded,
        "DryRun=false: expected every stored virtual interface, got {reported:?}"
    );
}
