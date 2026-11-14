// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `DescribeLocalGatewayVirtualInterfaceGroups`, driven over the wire
//! the service actually speaks: `ec2Query` — an `application/x-www-form-urlencoded` POST
//! whose body carries `Action=DescribeLocalGatewayVirtualInterfaceGroups`, answered with
//! XML (README.md / OPERATIONS.md "Wire protocol"). ec2Query is selected by the `ec2`
//! signing name in the request's SigV4 credential scope, so every call here carries one.
//!
//! The rule under test is that the operation REPORTS THE STORE: the set it answers with is
//! the account's local gateway virtual interface groups, narrowed by the
//! `LocalGatewayVirtualInterfaceGroupIds` and `Filters` the request names and cut into
//! `MaxResults`-sized pages. Every table row is a distinct input along what that
//! generalises over — which filter, how many values, how many filters, which ids, how many
//! virtual interfaces one group holds, which page size — and an account with no Outpost
//! (the empty store) is one row among them rather than the specification.

use ec2::ops::types::Tag;
use ec2::state::{
    ComAmazonawsEc2Amazonec2State as Ec2State, LocalGatewayVirtualInterfaceGroupSpec as GroupSpec,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{element, elements, error_code, filters, indexed, unescaped};

/// The Region every call here is signed for. The group store is per account rather than per
/// Region, so the Region only selects the ec2Query envelope — and the Region a stored
/// group's ARN names is the one its writer was signed for, which is why one seeded row
/// carries [`OTHER_REGION`].
const REGION: &str = "us-east-1";

/// The Region of the one seeded group written from another Region, so the reported ARNs
/// have two Region namespaces to tell apart.
const OTHER_REGION: &str = "us-west-2";

/// The ec2Query action under test.
const ACTION: &str = "DescribeLocalGatewayVirtualInterfaceGroups";

/// The XML element each reported group's id arrives in
/// (`LocalGatewayVirtualInterfaceGroup$LocalGatewayVirtualInterfaceGroupId`'s `xmlName`).
const ID_ELEMENT: &str = "localGatewayVirtualInterfaceGroupId";

/// One call carrying `params`, as its HTTP status and XML body.
async fn describe(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    super::ec2_query::call(router, REGION, ACTION, params).await
}

/// The group ids a successful response reports, in the order it reported them.
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

/// A tag, as the seeded groups carry them.
fn tag(key: &str, value: &str) -> Tag {
    Tag {
        key: Some(key.to_string()),
        value: Some(value.to_string()),
    }
}

/// What one seeded local gateway virtual interface group holds.
struct GroupRow {
    local_gateway_id: &'static str,
    /// "The IDs of the virtual interfaces"
    /// (`LocalGatewayVirtualInterfaceGroup$LocalGatewayVirtualInterfaceIds`) — a LIST, so
    /// the rows differ in how many it holds, down to none.
    virtual_interface_ids: Vec<&'static str>,
    local_bgp_asn: Option<i32>,
    local_bgp_asn_extended: Option<i64>,
    region: &'static str,
    /// "The current state of the local gateway virtual interface group"
    /// (`LocalGatewayVirtualInterfaceGroup$ConfigurationState`), one of the values
    /// `LocalGatewayVirtualInterfaceGroupConfigurationState` publishes — the generated
    /// catalog `ec2::ops::types::LOCAL_GATEWAY_VIRTUAL_INTERFACE_GROUP_CONFIGURATION_STATE_VALUES`,
    /// which [`every_seeded_state_is_a_modelled_configuration_state`] checks these against.
    configuration_state: &'static str,
    tags: Vec<Tag>,
}

/// The groups the filter and paging tables are written against. They differ along every
/// attribute the documented filters select on — the local gateway and the virtual
/// interfaces in the group — and along every member that is merely reported: the two ASNs
/// (present and absent), the Region the ARN names, the configuration state and the tags.
/// Three rows share a local gateway and two more share another, so a filter that selects on
/// one reports more than a single row.
fn seeded_rows() -> Vec<GroupRow> {
    vec![
        GroupRow {
            local_gateway_id: "lgw-00000000000000001",
            virtual_interface_ids: vec![
                "lgw-vif-00000000000000001",
                "lgw-vif-00000000000000002",
            ],
            local_bgp_asn: Some(64512),
            local_bgp_asn_extended: None,
            region: REGION,
            configuration_state: "available",
            tags: vec![tag("Name", "rack-a"), tag("Owner", "TeamA")],
        },
        GroupRow {
            local_gateway_id: "lgw-00000000000000001",
            virtual_interface_ids: vec!["lgw-vif-00000000000000003"],
            local_bgp_asn: Some(64513),
            local_bgp_asn_extended: Some(4_200_000_000),
            region: REGION,
            configuration_state: "pending",
            tags: vec![tag("Name", "rack-a")],
        },
        GroupRow {
            local_gateway_id: "lgw-00000000000000001",
            virtual_interface_ids: Vec::new(),
            local_bgp_asn: None,
            local_bgp_asn_extended: None,
            region: REGION,
            configuration_state: "incomplete",
            tags: Vec::new(),
        },
        GroupRow {
            local_gateway_id: "lgw-00000000000000002",
            virtual_interface_ids: vec![
                "lgw-vif-00000000000000004",
                "lgw-vif-00000000000000005",
                "lgw-vif-00000000000000006",
            ],
            local_bgp_asn: Some(64514),
            local_bgp_asn_extended: None,
            region: OTHER_REGION,
            configuration_state: "available",
            tags: vec![tag("Name", "rack-b")],
        },
        GroupRow {
            local_gateway_id: "lgw-00000000000000003",
            virtual_interface_ids: vec!["lgw-vif-00000000000000007"],
            local_bgp_asn: None,
            local_bgp_asn_extended: Some(4_200_000_001),
            region: REGION,
            configuration_state: "deleting",
            tags: vec![tag("Env", "test")],
        },
        GroupRow {
            local_gateway_id: "lgw-00000000000000003",
            virtual_interface_ids: vec!["lgw-vif-00000000000000008"],
            local_bgp_asn: Some(65000),
            local_bgp_asn_extended: None,
            region: REGION,
            configuration_state: "deleted",
            tags: Vec::new(),
        },
        GroupRow {
            local_gateway_id: "lgw-00000000000000004",
            virtual_interface_ids: vec!["lgw-vif-00000000000000009"],
            local_bgp_asn: Some(65001),
            local_bgp_asn_extended: None,
            region: REGION,
            configuration_state: "pending",
            tags: vec![tag("Name", "rack-c")],
        },
    ]
}

/// Write the seeded groups into the engine's store, the way the operation that creates a
/// local gateway virtual interface group would, and report the ids they were given in store
/// order.
fn seed(engine: &engine::InMemoryEngine) -> Vec<String> {
    let store = engine.state::<Ec2State>();
    let mut guard = store.lock();
    let mut ids = Vec::new();
    for row in seeded_rows() {
        let case = row.configuration_state;
        ids.push(
            guard
                .insert_local_gateway_virtual_interface_group(GroupSpec {
                    local_gateway_id: row.local_gateway_id.to_string(),
                    local_gateway_virtual_interface_ids: row
                        .virtual_interface_ids
                        .iter()
                        .map(|id| (*id).to_string())
                        .collect(),
                    local_bgp_asn: row.local_bgp_asn,
                    local_bgp_asn_extended: row.local_bgp_asn_extended,
                    region: row.region.to_string(),
                    configuration_state: row.configuration_state.to_string(),
                    tags: row.tags,
                })
                .unwrap_or_else(|| panic!("case {case:?}: the store must take the seeded group")),
        );
    }
    ids
}

/// The ids at `indices`, in store order.
fn ids_at(seeded: &[String], indices: &[usize]) -> Vec<String> {
    indices.iter().map(|index| seeded[*index].clone()).collect()
}

/// The ARN of a local gateway virtual interface group, as
/// `LocalGatewayVirtualInterfaceGroup$LocalGatewayVirtualInterfaceGroupArn` carries it:
/// `arn:aws:ec2:<region>:<account>:local-gateway-virtual-interface-group/<id>`.
fn group_arn(region: &str, group_id: &str) -> String {
    format!(
        "arn:aws:ec2:{region}:{}:local-gateway-virtual-interface-group/{group_id}",
        engine::emulator_account_id()
    )
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
            ec2::ops::types::LOCAL_GATEWAY_VIRTUAL_INTERFACE_GROUP_CONFIGURATION_STATE_VALUES
                .contains(&state),
            "case {state:?}: expected a value of \
             LocalGatewayVirtualInterfaceGroupConfigurationState, got {state:?}"
        );
    }
}

/// An account with no Outpost holds no virtual interface group — a local gateway and the
/// groups on it come with an Outpost rather than from any EC2 call — so the operation
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
        "fresh account: expected no group, got {reported:?} from {body}"
    );
    assert!(
        element(&body, "nextToken").is_none(),
        "fresh account: expected no pagination token, got {body}"
    );

    let again = described_ids(&router, "a second call", &[]).await;
    assert!(
        again.is_empty(),
        "a second call: expected no group, got {again:?}"
    );
    assert_eq!(
        engine
            .state::<Ec2State>()
            .lock()
            .local_gateway_virtual_interface_groups
            .len(),
        0,
        "describing groups must not write to the store"
    );
}

/// Every group the store holds is reported, in store order, with every member of the
/// modelled shape — and one written after a call shows up in the next one, so the answer is
/// computed from the store rather than fixed.
#[tokio::test]
async fn every_stored_group_is_reported_and_a_later_write_is_visible() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    let (status, body) = describe(&router, &[]).await;
    assert_eq!(
        status, 200,
        "all groups: expected HTTP 200, got {status} with body {body}"
    );
    let reported = elements(&body, ID_ELEMENT);
    assert_eq!(
        reported, seeded,
        "all groups: expected every stored group in store order, got {reported:?}"
    );

    // Every member of the reported shape comes from the stored group: the local gateway,
    // the virtual interfaces in it, the ARN built from the Region its writer was signed
    // for, the owning account, the ASNs, the configuration state and the tags.
    for (index, row) in seeded_rows().iter().enumerate() {
        let case = format!("row {index}");
        let mut expected: Vec<(&str, String)> = vec![
            ("localGatewayId", row.local_gateway_id.to_string()),
            (ID_ELEMENT, seeded[index].clone()),
            (
                "localGatewayVirtualInterfaceGroupArn",
                group_arn(row.region, &seeded[index]),
            ),
            ("ownerId", engine::emulator_account_id().to_string()),
            ("configurationState", row.configuration_state.to_string()),
        ];
        if let Some(asn) = row.local_bgp_asn {
            expected.push(("localBgpAsn", asn.to_string()));
        }
        if let Some(asn) = row.local_bgp_asn_extended {
            expected.push(("localBgpAsnExtended", asn.to_string()));
        }
        for (member, value) in expected {
            assert!(
                body.contains(&format!("<{member}>{value}</{member}>")),
                "case {case:?}: expected <{member}>{value}</{member}>, got {body}"
            );
        }
        for virtual_interface_id in &row.virtual_interface_ids {
            assert!(
                body.contains(&format!("<item>{virtual_interface_id}</item>")),
                "case {case:?}: expected the virtual interface {virtual_interface_id:?} to be \
                 reported, got {body}"
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
        .insert_local_gateway_virtual_interface_group(GroupSpec {
            local_gateway_id: "lgw-00000000000000009".to_string(),
            local_gateway_virtual_interface_ids: vec!["lgw-vif-00000000000000010".to_string()],
            local_bgp_asn: Some(65002),
            local_bgp_asn_extended: None,
            region: REGION.to_string(),
            configuration_state: "available".to_string(),
            tags: vec![tag("Name", "late")],
        })
        .expect("after a later write: the store must take the group");
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
async fn each_documented_filter_selects_the_groups_whose_attribute_equals_a_value() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);
    let account = engine::emulator_account_id().to_string();
    let all: Vec<usize> = (0..seeded.len()).collect();

    // One row per filter name `DescribeLocalGatewayVirtualInterfaceGroupsRequest$Filters`
    // documents, each asserted on a group the issue never cited, plus rows for several
    // values, a value nothing holds, and the wrong case.
    let cases: Vec<(&str, Vec<String>, Vec<usize>)> = vec![
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
            "local-gateway-virtual-interface-group-id",
            vec![seeded[3].clone()],
            vec![3],
        ),
        (
            "local-gateway-virtual-interface-group-id",
            vec![seeded[6].clone(), seeded[1].clone()],
            vec![1, 6],
        ),
        (
            "local-gateway-virtual-interface-group-id",
            vec![seeded[0].to_uppercase()],
            Vec::new(),
        ),
        // A LIST member: the interior id of the three-interface group selects that group.
        (
            "local-gateway-virtual-interface-id",
            vec!["lgw-vif-00000000000000005".to_string()],
            vec![3],
        ),
        (
            "local-gateway-virtual-interface-id",
            vec![
                "lgw-vif-00000000000000001".to_string(),
                "lgw-vif-00000000000000007".to_string(),
            ],
            vec![0, 4],
        ),
        // The group with no virtual interface holds no value for this attribute, so no
        // value can select it.
        (
            "local-gateway-virtual-interface-id",
            vec!["lgw-vif-00000000000000099".to_string()],
            Vec::new(),
        ),
        ("owner-id", vec![account.clone()], all.clone()),
        // An account id the emulator does not use (its own is
        // `engine::emulator_account_id()`), so no group is owned by it.
        ("owner-id", vec!["123456789012".to_string()], Vec::new()),
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

/// One row of the AND table: the filters to send, and the indices of the groups that must
/// come back.
type FilterCombination<'a> = (Vec<(&'a str, &'a [&'a str])>, Vec<usize>);

/// Several filters are joined with an AND: only the groups matching every one are reported.
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
                (
                    "local-gateway-virtual-interface-id",
                    &["lgw-vif-00000000000000003"][..],
                ),
            ],
            vec![1],
        ),
        (
            vec![
                ("local-gateway-id", &["lgw-00000000000000001"][..]),
                (
                    "local-gateway-virtual-interface-id",
                    &["lgw-vif-00000000000000007"][..],
                ),
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
                ("local-gateway-id", &["lgw-00000000000000002"][..]),
                (
                    "local-gateway-virtual-interface-id",
                    &[
                        "lgw-vif-00000000000000004",
                        "lgw-vif-00000000000000009",
                    ][..],
                ),
                ("owner-id", &owner[..]),
            ],
            vec![3],
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

    // Rows: a near-miss of a documented name, the underscore spelling, a documented name
    // with the wrong case, the plural, a member this operation's `Filters` doc does NOT
    // name a filter for, the tag filters it does not name either (even though the group
    // carries tags), and an empty name.
    let cases = [
        "local-gateway-virtual-interface-group",
        "local_gateway_id",
        "Local-Gateway-Id",
        "local-gateway-ids",
        "configuration-state",
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
            "case {name:?}: a refused request reports no group, got {body}"
        );
    }
}

/// One row of the bounds table: what the case is called, the form fields to send, and the
/// HTTP status the request must answer with.
type BoundsCase = (String, Vec<(String, String)>, u16);

/// The request-controlled factors of the store scan are bounded: each of the crate's caps
/// is accepted at its published value and refused one past it, so no request can ask for
/// unbounded filtering work.
#[tokio::test]
async fn the_filter_and_id_factors_are_accepted_at_their_caps_and_refused_past_them() {
    let (router, engine) = super::ec2_query::emulator();
    seed(&engine);

    // Rows: the number of filters, the values in one filter, the bytes in one value and the
    // number of group ids — each at the crate's cap (`ec2::state`) and one past it.
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
            .map(|index| format!("lgw-vif-grp-{index:017x}"))
            .collect();
        let borrowed: Vec<&str> = ids.iter().map(String::as_str).collect();
        indexed("LocalGatewayVirtualInterfaceGroupId", &borrowed)
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
                "case {case:?}: a refused request reports no group, got {body}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// LocalGatewayVirtualInterfaceGroupIds
// ---------------------------------------------------------------------------

/// The ids narrow the report to the groups named, in store order whatever order they were
/// asked for; an id that names no group of the account contributes no row.
#[tokio::test]
async fn the_ids_narrow_the_report_to_the_groups_named() {
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
            &indexed("LocalGatewayVirtualInterfaceGroupId", &borrowed),
        )
        .await;
        let mut expected = asked.clone();
        expected.sort_unstable();
        assert_eq!(
            reported,
            ids_at(&seeded, &expected),
            "case {case:?}: expected the groups named, in store order, got {reported:?}"
        );
    }

    // An id of the right shape that names no group of the account, alone and beside a
    // stored one.
    let unknown = "lgw-vif-grp-0000000000000beef";
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
            &indexed("LocalGatewayVirtualInterfaceGroupId", &borrowed),
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

/// Whatever the page size, walking the pages covers every group exactly once, in store
/// order, and the last page carries no token.
#[tokio::test]
async fn paging_covers_every_group_exactly_once_for_each_page_size() {
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
                "case {case:?}: expected at most {max_results} groups in a page, got \
                 {page:?}"
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
            "case {case:?}: expected every group exactly once in store order, got {walked:?}"
        );
    }
}

/// A page is a cut of the FILTERED set, not of the whole store: `MaxResults` and a filter
/// compose.
#[tokio::test]
async fn a_page_is_cut_from_the_filtered_set() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    // Six of the seven seeded groups belong to these three local gateways, so the model's
    // minimum page size cuts the filtered set into two pages.
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
        "filtered first page: expected the first five groups of those gateways, got {first:?}"
    );
    let token = element(&body, "nextToken")
        .expect("filtered first page: a sixth group follows, so a token must be issued");

    let mut next = page_params();
    next.push(("NextToken".to_string(), token));
    let second = described_ids(&router, "filtered second page", &next).await;
    assert_eq!(
        second,
        ids_at(&seeded, &[5]),
        "filtered second page: expected the remaining group of those gateways, got {second:?}"
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

/// A `NextToken` this service did not issue is refused with `InvalidPaginationToken`,
/// rather than read as a cursor.
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
        let (status, body) =
            describe(&router, &[("NextToken".to_string(), token.clone())]).await;
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
        "DryRun=true: a dry run reports no group, got {body}"
    );

    let reported = described_ids(
        &router,
        "DryRun=false",
        &[("DryRun".to_string(), "false".to_string())],
    )
    .await;
    assert_eq!(
        reported, seeded,
        "DryRun=false: expected every stored group, got {reported:?}"
    );
}
