// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `DescribeOutpostLags`, driven over the wire the service actually
//! speaks: `ec2Query` — an `application/x-www-form-urlencoded` POST whose body carries
//! `Action=<Operation>`, answered with XML (README.md / OPERATIONS.md "Wire protocol").
//! ec2Query is selected by the `ec2` signing name in the request's SigV4 credential scope, so
//! every call here carries one.
//!
//! One rule, with its own tables: the describe reports THE STORE. The set it answers with is
//! the account's Outpost link aggregation groups, narrowed by the `OutpostLagIds` and `Filters`
//! the request names and cut into `MaxResults`-sized pages, and it writes nothing. Of the
//! filters `DescribeOutpostLagsRequest$Filters` documents, four select on a fact of the LAG
//! itself and the other seven on an attribute of a service link virtual interface associated
//! with it, so a LAG is selected when one of its interfaces matches.
//!
//! The rows differ along which filter, how many values, how many filters, which ids, which
//! members a LAG and its interfaces hold a value for, how many interfaces a LAG has (including
//! none at all), which page size — and an account with no Outpost (the empty store) is one row
//! among them rather than the specification.

use ec2::ops::types::Tag;
use ec2::state::{
    ComAmazonawsEc2Amazonec2State as Ec2State, LocalGatewayVirtualInterfaceGroupSpec,
    OUTPOST_LAG_ID_PREFIX, OutpostLagSpec, SERVICE_LINK_VIF_ID_PREFIX,
    ServiceLinkVirtualInterfaceSpec as InterfaceSpec,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{element, elements, error_code, filters, indexed, unescaped};
use super::wire::{child_text, document, set_items};

/// The Region every call here is signed for. The LAG store is per account rather than per
/// Region, so the Region only selects the ec2Query envelope — and the Region a stored service
/// link virtual interface's ARN names is the one its writer was signed for, which is why one
/// seeded row carries [`OTHER_REGION`].
const REGION: &str = "us-east-1";

/// The Region of the one seeded Outpost written from another Region, so the reported ARNs have
/// two Region namespaces to tell apart.
const OTHER_REGION: &str = "us-west-2";

/// The ec2Query action under test.
const ACTION: &str = "DescribeOutpostLags";

/// The XML element each reported LAG's id arrives in (`OutpostLag$OutpostLagId`'s `xmlName`).
const ID_ELEMENT: &str = "outpostLagId";

/// One `DescribeOutpostLags` call, as its status and body.
async fn describe(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    super::ec2_query::call(router, REGION, ACTION, params).await
}

/// The LAG ids one successful call reports, in the order the response carries them.
async fn described_ids(
    router: &MultiServiceRouter,
    case: &str,
    params: &[(String, String)],
) -> Vec<String> {
    let (status, body) = describe(router, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from {ACTION}, got {status} with body {body}"
    );
    elements(&body, ID_ELEMENT)
}

/// One `MaxResults` parameter.
fn max_results_param(max_results: i64) -> Vec<(String, String)> {
    vec![("MaxResults".to_string(), max_results.to_string())]
}

/// A tag, as the seeded LAGs carry them.
fn tag(key: &str, value: &str) -> Tag {
    Tag {
        key: Some(key.to_string()),
        value: Some(value.to_string()),
    }
}

/// What one seeded Outpost LAG holds: the Outpost it aggregates the links of, the Region that
/// Outpost's ARN names, the state a writer pinned on it, and its tags.
struct LagRow {
    /// "The Outpost ID" the LAG's own `OutpostArn` names.
    outpost_id: &'static str,
    region: &'static str,
    /// "The current state of the Outpost LAG" (`OutpostLag$State`), which the model leaves an
    /// unconstrained string, so a writer pins it.
    state: &'static str,
    tags: Vec<Tag>,
}

/// The LAGs the tables below are written against. They differ along the Outpost (two LAGs share
/// one, so an Outpost filter reports more than a single row), the Region that Outpost's ARN
/// names, the state, the tags, and how many service link virtual interfaces run over them —
/// including one with NONE, which holds no value for any of the interface filters.
fn lag_rows() -> Vec<LagRow> {
    vec![
        LagRow {
            outpost_id: "op-00000000000000001",
            region: REGION,
            state: "available",
            tags: vec![tag("Name", "rack-a"), tag("Env", "prod")],
        },
        LagRow {
            outpost_id: "op-00000000000000001",
            region: REGION,
            state: "pending",
            tags: Vec::new(),
        },
        LagRow {
            outpost_id: "op-00000000000000002",
            region: REGION,
            state: "available",
            tags: vec![tag("Name", "rack-b")],
        },
        LagRow {
            outpost_id: "op-00000000000000003",
            region: OTHER_REGION,
            state: "available",
            tags: Vec::new(),
        },
        LagRow {
            outpost_id: "op-00000000000000004",
            region: REGION,
            state: "available",
            tags: vec![tag("Name", "rack-d")],
        },
    ]
}

/// What one seeded service link virtual interface holds, beside the LAG it runs over.
struct InterfaceRow {
    /// The index into [`lag_rows`] of the LAG this interface runs over.
    lag: usize,
    /// "The virtual local area network for the service link virtual interface" — absent on the
    /// row that holds no value for it, so an interface filter has a member with no value in it.
    vlan: Option<i32>,
    local_address: Option<&'static str>,
    peer_address: Option<&'static str>,
    peer_bgp_asn: Option<i64>,
    /// "The current state of the service link virtual interface", one of the values
    /// `ServiceLinkVirtualInterfaceConfigurationState` publishes — the generated catalog
    /// `ec2::ops::types::SERVICE_LINK_VIRTUAL_INTERFACE_CONFIGURATION_STATE_VALUES`, which
    /// [`every_seeded_configuration_state_is_a_modelled_one`] checks these against.
    configuration_state: &'static str,
}

/// The interfaces the filter tables are written against. Two LAGs have an interface on the same
/// VLAN and with the same peer ASN, so a filter selecting on one reports more than a single LAG;
/// one LAG has two interfaces that differ in every attribute, one has an interface holding no
/// value for most of them, and one LAG has no interface at all.
fn interface_rows() -> Vec<InterfaceRow> {
    vec![
        InterfaceRow {
            lag: 0,
            vlan: Some(101),
            local_address: Some("10.0.0.1"),
            peer_address: Some("10.0.0.2"),
            peer_bgp_asn: Some(65_000),
            configuration_state: "available",
        },
        InterfaceRow {
            lag: 0,
            vlan: Some(102),
            local_address: Some("10.0.0.5"),
            peer_address: Some("10.0.0.6"),
            peer_bgp_asn: Some(65_001),
            configuration_state: "pending",
        },
        InterfaceRow {
            lag: 1,
            vlan: Some(101),
            local_address: Some("10.1.0.1"),
            peer_address: Some("10.1.0.2"),
            peer_bgp_asn: Some(65_000),
            configuration_state: "available",
        },
        InterfaceRow {
            lag: 2,
            vlan: Some(201),
            local_address: Some("10.2.0.1"),
            peer_address: Some("10.2.0.2"),
            peer_bgp_asn: Some(4_294_967_295),
            configuration_state: "deleting",
        },
        InterfaceRow {
            lag: 4,
            vlan: None,
            local_address: None,
            peer_address: None,
            peer_bgp_asn: None,
            configuration_state: "available",
        },
    ]
}

/// The ARN of the Outpost a row names, as `OutpostLag$OutpostArn` carries it:
/// `arn:aws:outposts:<region>:<account>:outpost/<outpost-id>`.
fn outpost_arn(region: &str, outpost_id: &str) -> String {
    format!(
        "arn:aws:outposts:{region}:{}:outpost/{outpost_id}",
        engine::emulator_account_id()
    )
}

/// The ARN of a service link virtual interface, as
/// `ServiceLinkVirtualInterface$ServiceLinkVirtualInterfaceArn` carries it:
/// `arn:aws:ec2:<region>:<account>:service-link-virtual-interface/<id>`.
fn interface_arn(region: &str, interface_id: &str) -> String {
    format!(
        "arn:aws:ec2:{region}:{}:service-link-virtual-interface/{interface_id}",
        engine::emulator_account_id()
    )
}

/// What one seeding wrote: the LAG ids in store order, and the interface ids in store order.
struct Seeded {
    lags: Vec<String>,
    interfaces: Vec<String>,
}

/// Write the seeded LAGs and the interfaces over them into the engine's store — the seam a
/// second-generation Outposts rack is placed through, nothing in the EC2 API this emulator
/// serves racking an Outpost — and report the ids they were given, in store order.
fn seed(engine: &engine::InMemoryEngine) -> Seeded {
    let store = engine.state::<Ec2State>();
    let mut guard = store.lock();
    let mut lags = Vec::new();
    for row in lag_rows() {
        let case = row.outpost_id;
        lags.push(
            guard
                .insert_outpost_lag(OutpostLagSpec {
                    outpost_arn: outpost_arn(row.region, row.outpost_id),
                    state: row.state.to_string(),
                    // The LAGs start with no interface on them, so every id one reports later
                    // can only have been added by a write that put one there.
                    local_gateway_virtual_interface_ids: Vec::new(),
                    service_link_virtual_interface_ids: Vec::new(),
                    tags: row.tags.clone(),
                })
                .unwrap_or_else(|| panic!("case {case:?}: the store must take the seeded LAG")),
        );
    }
    let mut interfaces = Vec::new();
    for row in interface_rows() {
        let case = row.lag;
        let lag_row = &lag_rows()[row.lag];
        interfaces.push(
            guard
                .insert_service_link_virtual_interface(InterfaceSpec {
                    outpost_arn: outpost_arn(lag_row.region, lag_row.outpost_id),
                    outpost_lag_id: Some(lags[row.lag].clone()),
                    local_address: row.local_address.map(str::to_string),
                    peer_address: row.peer_address.map(str::to_string),
                    peer_bgp_asn: row.peer_bgp_asn,
                    vlan: row.vlan,
                    region: lag_row.region.to_string(),
                    configuration_state: row.configuration_state.to_string(),
                    tags: Vec::new(),
                })
                .unwrap_or_else(|| {
                    panic!("case {case:?}: the store must take the seeded interface")
                }),
        );
    }
    Seeded { lags, interfaces }
}

/// The ids at `indices`, in store order.
fn ids_at(seeded: &[String], indices: &[usize]) -> Vec<String> {
    indices.iter().map(|index| seeded[*index].clone()).collect()
}

/// The reported LAG whose `outpostLagId` is `lag_id`, read through the XML tree so a member of
/// the LAG is never confused with one nested in a set.
fn reported_member(case: &str, body: &str, lag_id: &str, member: &str) -> Option<String> {
    let parsed = document(case, body);
    parsed
        .descendants()
        .find(|node| {
            node.is_element() && child_text(*node, ID_ELEMENT).as_deref() == Some(lag_id)
        })
        .and_then(|lag| child_text(lag, member))
}

/// The entries of the `set` of the reported LAG `lag_id` — the `<item>`s an ec2Query list member
/// is written as.
fn reported_set(case: &str, body: &str, lag_id: &str, set: &str) -> Vec<String> {
    let parsed = document(case, body);
    parsed
        .descendants()
        .find(|node| {
            node.is_element() && child_text(*node, ID_ELEMENT).as_deref() == Some(lag_id)
        })
        .map(|lag| {
            set_items(lag, set)
                .into_iter()
                .map(|item| item.text().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// The store is what is reported
// ---------------------------------------------------------------------------

/// Every configuration state the seeded interfaces pin is a value the model publishes, so the
/// table above is written against the modelled enum rather than against prose.
#[test]
fn every_seeded_configuration_state_is_a_modelled_one() {
    for row in interface_rows() {
        let state = row.configuration_state;
        assert!(
            ec2::ops::types::SERVICE_LINK_VIRTUAL_INTERFACE_CONFIGURATION_STATE_VALUES
                .contains(&state),
            "case {state:?}: expected a value of ServiceLinkVirtualInterfaceConfigurationState, \
             got {state:?}"
        );
    }
}

/// An account with no Outpost holds no LAG, so the describe reports none — and describing is a
/// READ: it leaves the store exactly as it found it, so a second call answers the same.
#[tokio::test]
async fn an_account_with_no_outpost_reports_no_lag_and_the_read_writes_nothing() {
    let (router, engine) = super::ec2_query::emulator();

    let (status, body) = describe(&router, &[]).await;
    assert_eq!(
        status, 200,
        "the empty store: expected HTTP 200, got {status} with body {body}"
    );
    let reported = elements(&body, ID_ELEMENT);
    assert!(
        reported.is_empty(),
        "the empty store: expected no LAG, got {reported:?}"
    );

    let again = described_ids(&router, "a second call", &[]).await;
    assert!(
        again.is_empty(),
        "a second call: expected no LAG, got {again:?}"
    );
    assert_eq!(
        engine.state::<Ec2State>().lock().outpost_lags.len(),
        0,
        "describing Outpost LAGs must not write to the store"
    );
}

/// Every LAG the store holds is reported, in store order, with every member of the modelled
/// shape — and one written after a call shows up in the next one, so the answer is computed from
/// the store rather than fixed.
#[tokio::test]
async fn every_stored_lag_is_reported_and_a_later_write_is_visible() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    let (status, body) = describe(&router, &[]).await;
    assert_eq!(
        status, 200,
        "all LAGs: expected HTTP 200, got {status} with body {body}"
    );
    let reported = elements(&body, ID_ELEMENT);
    assert_eq!(
        reported, seeded.lags,
        "all LAGs: expected every stored LAG in store order, got {reported:?}"
    );

    // Every member of the reported shape comes from the stored LAG: the minted id, the ARN of
    // the Outpost its writer named, the owning account, the state, the interfaces over it and
    // the tags.
    for (index, row) in lag_rows().iter().enumerate() {
        let case = format!("row {index}");
        let lag_id = &seeded.lags[index];
        assert!(
            ec2::state::outpost_lag_id_is_well_formed(lag_id),
            "case {case:?}: expected a well-formed Outpost LAG id, got {lag_id:?}"
        );
        assert!(
            lag_id.starts_with(&format!("{OUTPOST_LAG_ID_PREFIX}-")),
            "case {case:?}: expected an id of the Outpost LAG prefix, got {lag_id:?}"
        );
        for (member, expected) in [
            ("outpostArn", outpost_arn(row.region, row.outpost_id)),
            ("ownerId", engine::emulator_account_id().to_string()),
            ("state", row.state.to_string()),
        ] {
            let actual = reported_member(&case, &body, lag_id, member);
            assert_eq!(
                actual.as_deref(),
                Some(expected.as_str()),
                "case {case:?}: expected <{member}> {expected:?}, got {actual:?}"
            );
        }
        // "The service link virtual interface IDs associated with the Outpost LAG": the ids the
        // store added when each interface was written over this LAG.
        let expected_interfaces: Vec<String> = interface_rows()
            .iter()
            .enumerate()
            .filter(|(_, interface)| interface.lag == index)
            .map(|(position, _)| seeded.interfaces[position].clone())
            .collect();
        let actual = reported_set(&case, &body, lag_id, "serviceLinkVirtualInterfaceIdSet");
        assert_eq!(
            actual, expected_interfaces,
            "case {case:?}: expected the interfaces over the LAG {expected_interfaces:?}, got \
             {actual:?}"
        );
        for interface_id in &expected_interfaces {
            assert!(
                ec2::state::service_link_virtual_interface_id_is_well_formed(interface_id),
                "case {case:?}: expected a well-formed interface id, got {interface_id:?}"
            );
            assert!(
                interface_id.starts_with(&format!("{SERVICE_LINK_VIF_ID_PREFIX}-")),
                "case {case:?}: expected an id of the interface prefix, got {interface_id:?}"
            );
        }
        for reported_tag in &row.tags {
            let key = reported_tag.key.clone().unwrap_or_default();
            let value = reported_tag.value.clone().unwrap_or_default();
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
        .insert_outpost_lag(OutpostLagSpec {
            outpost_arn: outpost_arn(REGION, "op-00000000000000009"),
            state: "available".to_string(),
            local_gateway_virtual_interface_ids: Vec::new(),
            service_link_virtual_interface_ids: Vec::new(),
            tags: vec![tag("Name", "late")],
        })
        .expect("after a later write: the store must take the LAG");
    let after = described_ids(&router, "after a later write", &[]).await;
    let mut expected = seeded.lags.clone();
    expected.push(late_id.clone());
    assert_eq!(
        after, expected,
        "after a later write: expected {late_id:?} to be reported too, got {after:?}"
    );
}

/// A virtual interface created ON a LAG is one of "The IDs of the local gateway virtual
/// interfaces associated with the Outpost LAG", so the LAG reports it after the create — the
/// association being kept by the one place an interface is stored.
#[tokio::test]
async fn a_virtual_interface_created_on_a_lag_is_reported_by_it() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    let group_id = engine
        .state::<Ec2State>()
        .lock()
        .insert_local_gateway_virtual_interface_group(LocalGatewayVirtualInterfaceGroupSpec {
            local_gateway_id: "lgw-00000000000000001".to_string(),
            local_gateway_virtual_interface_ids: Vec::new(),
            local_bgp_asn: Some(64_512),
            local_bgp_asn_extended: None,
            region: REGION.to_string(),
            configuration_state: "available".to_string(),
            tags: Vec::new(),
        })
        .expect("the store must take the seeded virtual interface group");

    // The LAG the interface is created on, and one it is not: only the first gains the id.
    let (status, body) = super::ec2_query::call(
        &router,
        REGION,
        "CreateLocalGatewayVirtualInterface",
        &[
            (
                "LocalGatewayVirtualInterfaceGroupId".to_string(),
                group_id.clone(),
            ),
            ("OutpostLagId".to_string(), seeded.lags[2].clone()),
            ("Vlan".to_string(), "301".to_string()),
            ("LocalAddress".to_string(), "10.3.0.1".to_string()),
            ("PeerAddress".to_string(), "10.3.0.2".to_string()),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "the create: expected HTTP 200, got {status} with body {body}"
    );
    let interface_id = element(&body, "localGatewayVirtualInterfaceId")
        .expect("the create must report the minted virtual interface id");

    let (status, body) = describe(&router, &[]).await;
    assert_eq!(
        status, 200,
        "after the create: expected HTTP 200, got {status} with body {body}"
    );
    let on_the_lag = reported_set(
        "after the create",
        &body,
        &seeded.lags[2],
        "localGatewayVirtualInterfaceIdSet",
    );
    assert_eq!(
        on_the_lag,
        vec![interface_id.clone()],
        "after the create: expected the LAG to report {interface_id:?}, got {on_the_lag:?}"
    );
    let on_another_lag = reported_set(
        "another LAG",
        &body,
        &seeded.lags[0],
        "localGatewayVirtualInterfaceIdSet",
    );
    assert!(
        on_another_lag.is_empty(),
        "another LAG: expected no virtual interface, got {on_another_lag:?}"
    );
}

// ---------------------------------------------------------------------------
// Filters
// ---------------------------------------------------------------------------

/// Each documented filter selects on its own attribute — the LAG's own for the four that name
/// one, and the attributes of the interfaces associated with the LAG for the other seven —
/// values within one filter are OR'd, and the comparison is exact and case-sensitive.
#[tokio::test]
async fn each_documented_filter_selects_the_lags_whose_attribute_matches() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);
    let account = engine::emulator_account_id().to_string();
    let all: Vec<usize> = (0..seeded.lags.len()).collect();

    // One row per filter name `DescribeOutpostLagsRequest$Filters` documents, each asserted on a
    // LAG the issue never cited, plus rows for several values, a value nothing holds, the wrong
    // case, and the LAGs that hold no value for the attribute at all.
    let cases: Vec<(&str, Vec<String>, Vec<usize>)> = vec![
        // The LAG's own facts.
        ("outpost-lag-id", vec![seeded.lags[3].clone()], vec![3]),
        (
            "outpost-lag-id",
            vec![seeded.lags[4].clone(), seeded.lags[1].clone()],
            vec![1, 4],
        ),
        ("outpost-lag-id", vec![seeded.lags[0].to_uppercase()], Vec::new()),
        (
            "outpost-arn",
            vec![outpost_arn(REGION, "op-00000000000000001")],
            vec![0, 1],
        ),
        (
            "outpost-arn",
            vec![outpost_arn(OTHER_REGION, "op-00000000000000003")],
            vec![3],
        ),
        (
            "outpost-arn",
            vec![outpost_arn(REGION, "op-00000000000000099")],
            Vec::new(),
        ),
        ("outpost-id", vec!["op-00000000000000001".to_string()], vec![0, 1]),
        ("outpost-id", vec!["op-00000000000000004".to_string()], vec![4]),
        ("outpost-id", vec!["op-00000000000000099".to_string()], Vec::new()),
        ("owner-id", vec![account.clone()], all.clone()),
        // An account id the emulator does not use (its own is `engine::emulator_account_id()`),
        // so no LAG is owned by it.
        ("owner-id", vec!["123456789012".to_string()], Vec::new()),
        // The attributes of the interfaces associated with the LAG.
        (
            "service-link-virtual-interface-id",
            vec![seeded.interfaces[1].clone()],
            vec![0],
        ),
        (
            "service-link-virtual-interface-id",
            vec![seeded.interfaces[2].clone(), seeded.interfaces[3].clone()],
            vec![1, 2],
        ),
        (
            "service-link-virtual-interface-arn",
            vec![interface_arn(REGION, &seeded.interfaces[0])],
            vec![0],
        ),
        (
            "service-link-virtual-interface-arn",
            vec![interface_arn(OTHER_REGION, &seeded.interfaces[0])],
            Vec::new(),
        ),
        ("vlan", vec!["101".to_string()], vec![0, 1]),
        ("vlan", vec!["201".to_string()], vec![2]),
        ("vlan", vec!["102".to_string(), "201".to_string()], vec![0, 2]),
        // No interface runs on this VLAN, and the LAG with no interface and the interface with
        // no VLAN hold no value for it.
        ("vlan", vec!["999".to_string()], Vec::new()),
        ("local-address", vec!["10.1.0.1".to_string()], vec![1]),
        ("local-address", vec!["10.0.0.5".to_string()], vec![0]),
        ("peer-address", vec!["10.2.0.2".to_string()], vec![2]),
        ("peer-address", vec!["10.0.0.2".to_string()], vec![0]),
        ("peer-bgp-asn", vec!["65000".to_string()], vec![0, 1]),
        // The top of the 32-bit ASN space, which the interface member is a `Long` for.
        ("peer-bgp-asn", vec!["4294967295".to_string()], vec![2]),
        ("peer-bgp-asn", vec!["65535".to_string()], Vec::new()),
        (
            "configuration-state",
            vec!["available".to_string()],
            vec![0, 1, 4],
        ),
        ("configuration-state", vec!["pending".to_string()], vec![0]),
        ("configuration-state", vec!["deleting".to_string()], vec![2]),
        ("configuration-state", vec!["deleted".to_string()], Vec::new()),
        ("configuration-state", vec!["AVAILABLE".to_string()], Vec::new()),
    ];

    for (name, values, expected_indices) in cases {
        let borrowed: Vec<&str> = values.iter().map(String::as_str).collect();
        let case = format!("{name}={values:?}");
        let reported = described_ids(&router, &case, &filters(&[(name, &borrowed)])).await;
        let expected = ids_at(&seeded.lags, &expected_indices);
        assert_eq!(
            reported, expected,
            "case {case:?}: expected {expected:?}, got {reported:?}"
        );
    }
}

/// One row of the AND table: the filters to send, and the indices of the LAGs that must come
/// back.
type FilterCombination<'a> = (Vec<(&'a str, &'a [&'a str])>, Vec<usize>);

/// Several filters are joined with an AND: only the LAGs matching every one are reported.
#[tokio::test]
async fn several_filters_are_joined_with_an_and() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);
    let account = engine::emulator_account_id().to_string();
    let owner: [&str; 1] = [account.as_str()];

    let cases: Vec<FilterCombination<'_>> = vec![
        (
            vec![
                ("outpost-id", &["op-00000000000000001"][..]),
                ("vlan", &["101"][..]),
            ],
            vec![0, 1],
        ),
        (
            vec![
                ("outpost-id", &["op-00000000000000001"][..]),
                ("vlan", &["102"][..]),
            ],
            vec![0],
        ),
        (
            vec![
                ("outpost-id", &["op-00000000000000001"][..]),
                ("vlan", &["201"][..]),
            ],
            Vec::new(),
        ),
        (
            vec![
                ("owner-id", &owner[..]),
                ("configuration-state", &["available", "deleting"][..]),
            ],
            vec![0, 1, 2, 4],
        ),
        (
            vec![
                ("peer-bgp-asn", &["65000"][..]),
                ("local-address", &["10.1.0.1"][..]),
                ("owner-id", &owner[..]),
            ],
            vec![1],
        ),
    ];

    for (entries, expected_indices) in cases {
        let case = format!("{entries:?}");
        let reported = described_ids(&router, &case, &filters(&entries)).await;
        let expected = ids_at(&seeded.lags, &expected_indices);
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

    // Rows: a member this operation's `Filters` doc does NOT name a filter for even though the
    // shape carries it (the LAG's state, the local gateway virtual interfaces over it), the
    // underscore spelling, a documented name with the wrong case, the plural, a near-miss, the
    // tag filters it does not name either (even though the LAG carries tags), and an empty name.
    let cases = [
        "state",
        "local-gateway-virtual-interface-id",
        "outpost_id",
        "Outpost-Id",
        "outpost-ids",
        "outpost-lag",
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
            "case {name:?}: a refused request reports no LAG, got {body}"
        );
    }
}

/// One row of the bounds table: what the case is called, the form fields to send, and the HTTP
/// status the request must answer with.
type BoundsCase = (String, Vec<(String, String)>, u16);

/// The request-controlled factors of the store scan are bounded: each of the crate's caps is
/// accepted at its published value and refused one past it, so no request can ask for unbounded
/// filtering work.
#[tokio::test]
async fn the_filter_and_id_factors_are_accepted_at_their_caps_and_refused_past_them() {
    let (router, engine) = super::ec2_query::emulator();
    seed(&engine);

    let many_filters = |count: usize| -> Vec<(String, String)> {
        let entries: Vec<(&str, &[&str])> = (0..count)
            .map(|_| ("outpost-id", &["op-00000000000000001"][..]))
            .collect();
        filters(&entries)
    };
    let many_values = |count: usize| -> Vec<(String, String)> {
        let values: Vec<String> = (0..count).map(|index| format!("op-{index}")).collect();
        let borrowed: Vec<&str> = values.iter().map(String::as_str).collect();
        filters(&[("outpost-id", &borrowed)])
    };
    let one_value_of = |bytes: usize| -> Vec<(String, String)> {
        let value = "a".repeat(bytes);
        filters(&[("outpost-id", &[value.as_str()])])
    };
    let many_ids = |count: usize| -> Vec<(String, String)> {
        let ids: Vec<String> = (0..count).map(|index| format!("ola-{index:017x}")).collect();
        let borrowed: Vec<&str> = ids.iter().map(String::as_str).collect();
        indexed("OutpostLagId", &borrowed)
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
                "case {case:?}: a refused request reports no LAG, got {body}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// OutpostLagIds
// ---------------------------------------------------------------------------

/// The ids narrow the report to the LAGs named, in store order whatever order they were asked
/// for; an id that names no LAG of the account contributes no row.
#[tokio::test]
async fn the_ids_narrow_the_report_to_the_lags_named() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    // Rows differ in count and in the order the ids are asked for.
    let cases: Vec<Vec<usize>> = vec![
        vec![0],
        vec![4],
        vec![2, 3],
        vec![3, 2],
        vec![4, 1, 2],
        (0..seeded.lags.len()).collect(),
    ];
    for asked in cases {
        let ids = ids_at(&seeded.lags, &asked);
        let borrowed: Vec<&str> = ids.iter().map(String::as_str).collect();
        let case = format!("ids {asked:?}");
        let reported = described_ids(&router, &case, &indexed("OutpostLagId", &borrowed)).await;
        let mut expected = asked.clone();
        expected.sort_unstable();
        assert_eq!(
            reported,
            ids_at(&seeded.lags, &expected),
            "case {case:?}: expected the LAGs named, in store order, got {reported:?}"
        );
    }

    // An id of the right shape that names no LAG of the account, alone and beside a stored one.
    let unknown = "ola-0000000000000beef";
    for (case, asked) in [
        ("an unknown id alone", vec![unknown.to_string()]),
        (
            "an unknown id beside a stored one",
            vec![unknown.to_string(), seeded.lags[1].clone()],
        ),
    ] {
        let borrowed: Vec<&str> = asked.iter().map(String::as_str).collect();
        let reported = described_ids(&router, case, &indexed("OutpostLagId", &borrowed)).await;
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

/// Whatever the page size, walking the pages covers every LAG exactly once, in store order, and
/// the last page carries no token.
#[tokio::test]
async fn paging_covers_every_lag_exactly_once_for_each_page_size() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    // `OutpostLagMaxResults` is modelled `range 5..1000`, so the rows are its published lower
    // bound (which divides the store exactly), one that does not, and the upper bound, which
    // holds everything.
    for max_results in [5, 6, 1000] {
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
                "case {case:?}: expected at most {max_results} LAGs in a page, got {page:?}"
            );
            walked.extend(page);
            pages += 1;
            assert!(
                pages <= seeded.lags.len() + 1,
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
            walked, seeded.lags,
            "case {case:?}: expected every LAG exactly once in store order, got {walked:?}"
        );
    }
}

/// A page is a cut of the FILTERED set, not of the whole store: `MaxResults` and a filter
/// compose.
#[tokio::test]
async fn a_page_is_cut_from_the_filtered_set() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    // The seeded store holds fewer LAGs than the smallest page the model publishes, so the
    // filtered set is grown past it: five more LAGs on the Outpost of the seeded row 2, which
    // makes six on that Outpost and leaves the other four LAGs out of the filter.
    let shared_outpost = lag_rows()[2].outpost_id;
    let mut on_that_outpost = vec![seeded.lags[2].clone()];
    for index in 0..5 {
        let case = format!("extra LAG {index}");
        on_that_outpost.push(
            engine
                .state::<Ec2State>()
                .lock()
                .insert_outpost_lag(OutpostLagSpec {
                    outpost_arn: outpost_arn(REGION, shared_outpost),
                    state: "available".to_string(),
                    local_gateway_virtual_interface_ids: Vec::new(),
                    service_link_virtual_interface_ids: Vec::new(),
                    tags: Vec::new(),
                })
                .unwrap_or_else(|| panic!("case {case:?}: the store must take the LAG")),
        );
    }

    let page_params = || {
        let mut params = filters(&[("outpost-id", &[shared_outpost])]);
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
        on_that_outpost[..5].to_vec(),
        "filtered first page: expected the first five LAGs of that Outpost, got {first:?}"
    );
    let token = element(&body, "nextToken")
        .expect("filtered first page: a sixth LAG follows, so a token must be issued");

    let mut next = page_params();
    next.push(("NextToken".to_string(), token));
    let second = described_ids(&router, "filtered second page", &next).await;
    assert_eq!(
        second,
        on_that_outpost[5..].to_vec(),
        "filtered second page: expected the remaining LAG of that Outpost, got {second:?}"
    );
}

/// A page carries a token exactly when a LAG follows it, and the token names where the next page
/// starts.
#[tokio::test]
async fn a_token_is_issued_exactly_when_a_lag_follows_the_page() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    // The store holds five LAGs, so the model's minimum page size holds them all and issues no
    // token; narrowing to three ids with that page size does the same, and a filtered set larger
    // than the page issues one.
    let (status, body) = describe(&router, &max_results_param(5)).await;
    assert_eq!(
        status, 200,
        "a page that holds the store: expected HTTP 200, got {status} with body {body}"
    );
    assert_eq!(
        elements(&body, ID_ELEMENT),
        seeded.lags,
        "a page that holds the store: expected every LAG, got {body}"
    );
    assert!(
        element(&body, "nextToken").is_none(),
        "a page that holds the store: no LAG follows it, so no token may be issued, got {body}"
    );

    // One more LAG than the page holds, so a token is issued and the next page starts at the
    // LAG the token names.
    let late_id = engine
        .state::<Ec2State>()
        .lock()
        .insert_outpost_lag(OutpostLagSpec {
            outpost_arn: outpost_arn(REGION, "op-00000000000000005"),
            state: "available".to_string(),
            local_gateway_virtual_interface_ids: Vec::new(),
            service_link_virtual_interface_ids: Vec::new(),
            tags: Vec::new(),
        })
        .expect("a sixth LAG: the store must take it");
    let (status, body) = describe(&router, &max_results_param(5)).await;
    assert_eq!(
        status, 200,
        "a sixth LAG: expected HTTP 200, got {status} with body {body}"
    );
    assert_eq!(
        elements(&body, ID_ELEMENT),
        seeded.lags,
        "a sixth LAG: expected the first five LAGs, got {body}"
    );
    let token = element(&body, "nextToken")
        .expect("a sixth LAG: one follows the page, so a token must be issued");
    let mut params = max_results_param(5);
    params.push(("NextToken".to_string(), token));
    let second = described_ids(&router, "the second page", &params).await;
    assert_eq!(
        second,
        vec![late_id.clone()],
        "the second page: expected {late_id:?}, got {second:?}"
    );
}

/// `MaxResults` outside the range the model publishes is refused at the boundary; each published
/// bound itself is accepted.
#[tokio::test]
async fn max_results_is_accepted_at_its_bounds_and_refused_past_them() {
    let (router, engine) = super::ec2_query::emulator();
    seed(&engine);

    // The published bounds (`OutpostLagMaxResults`, `range 5..1000`) and one past each.
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

    // Rows: free text, punctuation, a hex string this service never minted, the bare resource id
    // a caller might guess the token is, and that id hex-encoded (the shape the token takes,
    // without the marker and checksum a minted token carries).
    let cases = [
        "not-a-token".to_string(),
        "###".to_string(),
        "6161".to_string(),
        seeded.lags[0].clone(),
        ::hex::encode(&seeded.lags[0]),
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

/// `DryRun` reports the permitted outcome — `DryRunOperation`, HTTP 412 — and describes nothing;
/// `DryRun=false` is an ordinary call.
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
        "DryRun=true: a dry run reports no LAG, got {body}"
    );

    let reported = described_ids(
        &router,
        "DryRun=false",
        &[("DryRun".to_string(), "false".to_string())],
    )
    .await;
    assert_eq!(
        reported, seeded.lags,
        "DryRun=false: expected every stored LAG, got {reported:?}"
    );
}
