// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `DescribeLocalGateways`, driven over the wire the service actually
//! speaks: `ec2Query` — an `application/x-www-form-urlencoded` POST whose body carries
//! `Action=DescribeLocalGateways`, answered with XML (README.md / OPERATIONS.md "Wire
//! protocol"). ec2Query is selected by the `ec2` signing name in the request's SigV4
//! credential scope, so every call here carries one.
//!
//! The rule under test is that the operation REPORTS THE STORE: "By default, all local
//! gateways are described. Alternatively, you can filter the results"
//! (`DescribeLocalGateways`) — the set it answers with is the account's local gateways,
//! narrowed by the `LocalGatewayIds` and the documented `Filters` the request names and cut
//! into `MaxResults`-sized pages. Every table row is a distinct input along what that
//! generalises over — which filter, how many values, how many filters, which ids, which
//! page size, which state, which Outpost — and an account with no Outpost (the empty store)
//! is one row among them rather than the specification.

use ec2::ops::types::Tag;
use ec2::state::{ComAmazonawsEc2Amazonec2State as Ec2State, LocalGatewaySpec};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{element, elements, error_code, filters, indexed, unescaped};

/// The Region every call here is signed for. The local gateway store is per account rather
/// than per Region — a gateway is part of an Outpost rack — so the Region only selects the
/// ec2Query envelope, while the Outpost ARNs the seeded gateways carry name Regions of
/// their own.
const REGION: &str = "us-east-1";

/// The ec2Query action under test.
const ACTION: &str = "DescribeLocalGateways";

/// The XML element each reported gateway's id arrives in (`LocalGateway$LocalGatewayId`'s
/// `xmlName`).
const ID_ELEMENT: &str = "localGatewayId";

/// The form-field prefix a `LocalGatewayIds` entry arrives under
/// (`DescribeLocalGatewaysRequest$LocalGatewayIds`' `xmlName`, `LocalGatewayId`).
const ID_PARAM: &str = "LocalGatewayId";

/// One call carrying `params`, as its HTTP status and XML body.
async fn describe(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    super::ec2_query::call(router, REGION, ACTION, params).await
}

/// The gateway ids a successful response reports, in the order it reported them.
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

/// A tag, as the seeded gateways carry them.
fn tag(key: &str, value: &str) -> Tag {
    Tag {
        key: Some(key.to_string()),
        value: Some(value.to_string()),
    }
}

/// The ARN of the Outpost `name` in `region`, as `LocalGateway$OutpostArn` carries it.
fn outpost_arn(region: &str, name: &str) -> String {
    format!(
        "arn:aws:outposts:{region}:{}:outpost/{name}",
        engine::emulator_account_id()
    )
}

/// What one seeded local gateway holds: the Outpost rack it is part of and the state it is
/// in. Nothing in the EC2 API this emulator serves racks an Outpost, so a gateway is
/// written straight through the store's own seam — the one a gateway placed by an Outpost
/// order would use.
struct GatewayRow {
    /// The Region of the Outpost's ARN, so `outpost-arn` has more than one namespace to
    /// select within.
    region: &'static str,
    outpost: &'static str,
    /// "The state of the local gateway" (`LocalGateway$State`), which the model leaves an
    /// unconstrained `String` (`smithy_query(query="shape",shape="LocalGateway")`) — so
    /// these are the states AWS reports for an Outpost's gateway rather than members of a
    /// modelled enum.
    state: &'static str,
    tags: Vec<Tag>,
}

/// The gateways the filter, id and paging tables are written against. They differ along
/// every attribute the documented filters select on — the gateway id, the Outpost ARN and
/// the state — and along the one member that is merely reported, the tags (none, one and
/// two). Three rows share a state and two more share another, so a filter that selects on
/// one reports more than a single row.
fn seeded_rows() -> Vec<GatewayRow> {
    vec![
        GatewayRow {
            region: REGION,
            outpost: "op-00000000000000001",
            state: "available",
            tags: vec![tag("Name", "rack-a"), tag("Owner", "TeamA")],
        },
        GatewayRow {
            region: REGION,
            outpost: "op-00000000000000002",
            state: "available",
            tags: vec![tag("Name", "rack-b")],
        },
        GatewayRow {
            region: REGION,
            outpost: "op-00000000000000003",
            state: "pending",
            tags: Vec::new(),
        },
        GatewayRow {
            region: "us-west-2",
            outpost: "op-00000000000000004",
            state: "available",
            tags: vec![tag("Name", "rack-d")],
        },
        GatewayRow {
            region: "us-west-2",
            outpost: "op-00000000000000005",
            state: "deleting",
            tags: vec![tag("Env", "test")],
        },
        GatewayRow {
            region: "eu-west-1",
            outpost: "op-00000000000000006",
            state: "deleted",
            tags: Vec::new(),
        },
        GatewayRow {
            region: REGION,
            outpost: "op-00000000000000007",
            state: "pending",
            tags: vec![tag("Name", "rack-g")],
        },
    ]
}

/// Write the seeded local gateways into the engine's store and report the ids they were
/// given, in store order.
fn seed(engine: &engine::InMemoryEngine) -> Vec<String> {
    let store = engine.state::<Ec2State>();
    let mut guard = store.lock();
    let mut ids = Vec::new();
    for row in seeded_rows() {
        let case = row.outpost;
        ids.push(
            guard
                .insert_local_gateway(LocalGatewaySpec {
                    outpost_arn: outpost_arn(row.region, row.outpost),
                    state: row.state.to_string(),
                    tags: row.tags,
                })
                .unwrap_or_else(|| {
                    panic!("case {case:?}: the store must take the seeded local gateway")
                }),
        );
    }
    ids
}

/// The ids at `indices`, in store order.
fn ids_at(seeded: &[String], indices: &[usize]) -> Vec<String> {
    indices.iter().map(|index| seeded[*index].clone()).collect()
}

/// How many local gateways the store holds.
fn stored_gateways(engine: &engine::InMemoryEngine) -> usize {
    engine.state::<Ec2State>().lock().local_gateways.len()
}

// ---------------------------------------------------------------------------
// The store is what is reported
// ---------------------------------------------------------------------------

/// An account with no Outpost holds no local gateway — a gateway is part of an Outpost rack
/// rather than something any EC2 call creates — so the operation answers HTTP 200 with an
/// empty set, and describing is a READ: two calls report the same thing and the store is
/// untouched.
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
        "fresh account: expected no local gateway, got {reported:?} from {body}"
    );
    assert!(
        element(&body, "nextToken").is_none(),
        "fresh account: expected no pagination token, got {body}"
    );

    let again = described_ids(&router, "a second call", &[]).await;
    assert!(
        again.is_empty(),
        "a second call: expected no local gateway, got {again:?}"
    );
    assert_eq!(
        stored_gateways(&engine),
        0,
        "describing local gateways must not write to the store"
    );
}

/// Every gateway the store holds is reported, in store order, with every member of the
/// modelled shape — and one written after a call shows up in the next one, so the answer is
/// computed from the store rather than fixed. Describing leaves the store exactly as it
/// found it.
#[tokio::test]
async fn every_stored_gateway_is_reported_and_a_later_write_is_visible() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    let (status, body) = describe(&router, &[]).await;
    assert_eq!(
        status, 200,
        "all gateways: expected HTTP 200, got {status} with body {body}"
    );
    let reported = elements(&body, ID_ELEMENT);
    assert_eq!(
        reported, seeded,
        "all gateways: expected every stored gateway in store order, got {reported:?}"
    );

    // Every member of the reported shape comes from the stored gateway: the id the store
    // minted, the Outpost it is part of, the owning account and the state, plus its tags.
    for (index, row) in seeded_rows().iter().enumerate() {
        let case = format!("row {index}");
        let expected: Vec<(&str, String)> = vec![
            (ID_ELEMENT, seeded[index].clone()),
            ("outpostArn", outpost_arn(row.region, row.outpost)),
            ("ownerId", engine::emulator_account_id().to_string()),
            ("state", row.state.to_string()),
        ];
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

    assert_eq!(
        stored_gateways(&engine),
        seeded.len(),
        "describing local gateways must not write to the store"
    );

    let late_id = engine
        .state::<Ec2State>()
        .lock()
        .insert_local_gateway(LocalGatewaySpec {
            outpost_arn: outpost_arn(REGION, "op-00000000000000009"),
            state: "available".to_string(),
            tags: vec![tag("Name", "late")],
        })
        .expect("after a later write: the store must take the gateway");
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
/// the comparison is case-sensitive, and EC2's `*` wildcard matches a run of characters.
#[tokio::test]
async fn each_documented_filter_selects_the_gateways_whose_attribute_matches_a_value() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);
    let account = engine::emulator_account_id().to_string();
    let all: Vec<usize> = (0..seeded.len()).collect();

    // One row per filter name `DescribeLocalGatewaysRequest$Filters` documents —
    // `local-gateway-id`, `outpost-arn`, `owner-id`, `state` — each asserted on a gateway
    // the issue never cited, plus rows for several values, a value nothing holds, the wrong
    // case and a wildcard.
    let cases: Vec<(&str, Vec<String>, Vec<usize>)> = vec![
        ("local-gateway-id", vec![seeded[2].clone()], vec![2]),
        (
            "local-gateway-id",
            vec![seeded[5].clone(), seeded[1].clone()],
            vec![1, 5],
        ),
        (
            "local-gateway-id",
            vec![seeded[0].to_uppercase()],
            Vec::new(),
        ),
        (
            "outpost-arn",
            vec![outpost_arn("us-west-2", "op-00000000000000004")],
            vec![3],
        ),
        (
            "outpost-arn",
            vec![
                outpost_arn(REGION, "op-00000000000000001"),
                outpost_arn(REGION, "op-00000000000000007"),
            ],
            vec![0, 6],
        ),
        // The Outposts of one Region, by EC2's own `*` wildcard.
        (
            "outpost-arn",
            vec!["arn:aws:outposts:us-west-2:*".to_string()],
            vec![3, 4],
        ),
        (
            "outpost-arn",
            vec![outpost_arn(REGION, "op-00000000000000099")],
            Vec::new(),
        ),
        ("owner-id", vec![account.clone()], all.clone()),
        // An account id the emulator does not use (its own is
        // `engine::emulator_account_id()`), so no gateway is owned by it.
        ("owner-id", vec!["123456789012".to_string()], Vec::new()),
        ("state", vec!["available".to_string()], vec![0, 1, 3]),
        (
            "state",
            vec!["pending".to_string(), "deleting".to_string()],
            vec![2, 4, 6],
        ),
        ("state", vec!["delet*".to_string()], vec![4, 5]),
        ("state", vec!["Available".to_string()], Vec::new()),
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

/// One row of the AND table: the filters to send, and the indices of the gateways that must
/// come back.
type FilterCombination<'a> = (Vec<(&'a str, &'a [&'a str])>, Vec<usize>);

/// Several filters are joined with an AND: only the gateways matching every one are
/// reported.
#[tokio::test]
async fn several_filters_are_joined_with_an_and() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);
    let account = engine::emulator_account_id().to_string();
    let owner: [&str; 1] = [account.as_str()];
    let first_outpost = outpost_arn(REGION, "op-00000000000000001");
    let third_outpost = outpost_arn(REGION, "op-00000000000000003");
    let first_outpost_values: [&str; 1] = [first_outpost.as_str()];
    let third_outpost_values: [&str; 1] = [third_outpost.as_str()];
    let gateway_ids = ids_at(&seeded, &[0, 1, 2]);
    let named: Vec<&str> = gateway_ids.iter().map(String::as_str).collect();

    let cases: Vec<FilterCombination<'_>> = vec![
        (
            vec![
                ("state", &["available"][..]),
                ("outpost-arn", &first_outpost_values[..]),
            ],
            vec![0],
        ),
        (
            vec![
                ("state", &["available"][..]),
                ("outpost-arn", &third_outpost_values[..]),
            ],
            Vec::new(),
        ),
        (
            vec![("state", &["pending"][..]), ("owner-id", &owner[..])],
            vec![2, 6],
        ),
        (
            vec![
                ("local-gateway-id", &named[..]),
                ("state", &["available"][..]),
                ("owner-id", &owner[..]),
            ],
            vec![0, 1],
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
    // with the wrong case, the plural, members this operation's `Filters` doc does NOT name
    // a filter for, the tag filters it does not name either (even though the gateway
    // carries tags), and an empty name.
    let cases = [
        "local-gateway",
        "local_gateway_id",
        "Local-Gateway-Id",
        "local-gateway-ids",
        "outpost-id",
        "local-gateway-route-table-id",
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
            "case {name:?}: a refused request reports no gateway, got {body}"
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
    // number of gateway ids — each at the crate's cap (`ec2::state`) and one past it.
    let many_filters = |count: usize| -> Vec<(String, String)> {
        let entries: Vec<(&str, &[&str])> = (0..count)
            .map(|_| ("state", &["available"][..]))
            .collect();
        filters(&entries)
    };
    let many_values = |count: usize| -> Vec<(String, String)> {
        let values: Vec<String> = (0..count).map(|index| format!("state-{index}")).collect();
        let borrowed: Vec<&str> = values.iter().map(String::as_str).collect();
        filters(&[("state", &borrowed)])
    };
    let one_value_of = |bytes: usize| -> Vec<(String, String)> {
        let value = "a".repeat(bytes);
        filters(&[("state", &[value.as_str()])])
    };
    let many_ids = |count: usize| -> Vec<(String, String)> {
        let ids: Vec<String> = (0..count).map(|index| format!("lgw-{index:017x}")).collect();
        let borrowed: Vec<&str> = ids.iter().map(String::as_str).collect();
        indexed(ID_PARAM, &borrowed)
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
                "case {case:?}: a refused request reports no gateway, got {body}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// LocalGatewayIds
// ---------------------------------------------------------------------------

/// The ids narrow the report to the gateways named, in store order whatever order they were
/// asked for; an id that names no gateway of the account contributes no row.
#[tokio::test]
async fn the_ids_narrow_the_report_to_the_gateways_named() {
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
        let reported = described_ids(&router, &case, &indexed(ID_PARAM, &borrowed)).await;
        let mut expected = asked.clone();
        expected.sort_unstable();
        assert_eq!(
            reported,
            ids_at(&seeded, &expected),
            "case {case:?}: expected the gateways named, in store order, got {reported:?}"
        );
    }

    // An id of the right shape that names no gateway of the account, alone and beside a
    // stored one.
    let unknown = "lgw-0000000000000beef";
    for (case, asked) in [
        ("an unknown id alone", vec![unknown.to_string()]),
        (
            "an unknown id beside a stored one",
            vec![unknown.to_string(), seeded[1].clone()],
        ),
    ] {
        let borrowed: Vec<&str> = asked.iter().map(String::as_str).collect();
        let reported = described_ids(&router, case, &indexed(ID_PARAM, &borrowed)).await;
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

/// One row of the compose table: which seeded gateways the request names, which states it
/// filters for, and the indices of the gateways that must come back.
struct ComposedCase {
    case: &'static str,
    asked: Vec<usize>,
    states: Vec<&'static str>,
    expected_indices: Vec<usize>,
}

/// The ids and the filters compose: a gateway is reported only when it is both named and
/// selected.
#[tokio::test]
async fn the_ids_and_the_filters_compose() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    // Rows: ids whose states differ, so the filter drops some of them; a filter no named
    // gateway satisfies; and a filter that selects gateways the ids do not name.
    let cases: Vec<ComposedCase> = vec![
        ComposedCase {
            case: "available of three",
            asked: vec![0, 2, 3],
            states: vec!["available"],
            expected_indices: vec![0, 3],
        },
        ComposedCase {
            case: "deleted of two",
            asked: vec![0, 1],
            states: vec!["deleted"],
            expected_indices: Vec::new(),
        },
        ComposedCase {
            case: "pending of all",
            asked: (0..seeded.len()).collect(),
            states: vec!["pending"],
            expected_indices: vec![2, 6],
        },
        ComposedCase {
            case: "deleting of one",
            asked: vec![4],
            states: vec!["deleting"],
            expected_indices: vec![4],
        },
    ];
    for ComposedCase {
        case,
        asked,
        states,
        expected_indices,
    } in cases
    {
        let ids = ids_at(&seeded, &asked);
        let borrowed: Vec<&str> = ids.iter().map(String::as_str).collect();
        let mut params = indexed(ID_PARAM, &borrowed);
        params.extend(filters(&[("state", &states)]));
        let reported = described_ids(&router, case, &params).await;
        let expected = ids_at(&seeded, &expected_indices);
        assert_eq!(
            reported, expected,
            "case {case:?}: expected {expected:?}, got {reported:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Pagination
// ---------------------------------------------------------------------------

/// Whatever the page size, walking the pages covers every gateway exactly once, in store
/// order, and the last page carries no token.
#[tokio::test]
async fn paging_covers_every_gateway_exactly_once_for_each_page_size() {
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
                "case {case:?}: expected at most {max_results} gateways in a page, got {page:?}"
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
            "case {case:?}: expected every gateway exactly once in store order, got {walked:?}"
        );
    }
}

/// A page is a cut of the FILTERED set, not of the whole store: `MaxResults` and a filter
/// compose.
#[tokio::test]
async fn a_page_is_cut_from_the_filtered_set() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    // Six of the seven seeded gateways are in one of these three states, so the model's
    // minimum page size cuts the filtered set into two pages.
    let selected_states: &[&str] = &["available", "pending", "deleting"];
    let page_params = || {
        let mut params = filters(&[("state", selected_states)]);
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
        "filtered first page: expected the first five gateways in those states, got {first:?}"
    );
    let token = element(&body, "nextToken")
        .expect("filtered first page: a sixth gateway follows, so a token must be issued");

    let mut next = page_params();
    next.push(("NextToken".to_string(), token));
    let second = described_ids(&router, "filtered second page", &next).await;
    assert_eq!(
        second,
        ids_at(&seeded, &[6]),
        "filtered second page: expected the remaining gateway in those states, got {second:?}"
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
// The id of a gateway the account does not hold
// ---------------------------------------------------------------------------

/// The operations that resolve a `LocalGatewayId` all refuse an id the account holds no
/// gateway for with the resource's own code, and each words the sentence the way AWS words
/// it for THAT operation: `CreateLocalGatewayRouteTable` names the gateway by the API's own
/// `LocalGateway` — the same `<Kind>` token the error code carries — while every other
/// reader of the id names it by the `ResourceType` member `local-gateway`, which is also
/// the spelling the sentence for a MALFORMED id of this resource uses. Driven through each
/// operation that takes one, so neither sentence can drift between them.
#[tokio::test]
async fn an_unresolvable_local_gateway_id_is_refused_with_the_resources_own_sentence() {
    let (router, engine) = super::ec2_query::emulator();
    seed(&engine);

    // Rows: a well-formed id of each length EC2 issues (8 and 17 hexadecimal digits) that
    // names no stored gateway, then ids that are no local gateway id at all — the bare
    // prefix, a non-hexadecimal suffix, the same digits under another prefix, and a route
    // table's id, whose suffix is not hexadecimal under this prefix.
    let unknown = ["lgw-0123456789abcdef0", "lgw-01234567"];
    let malformed = [
        "lgw",
        "lgw-0123456789abcdefg",
        "lgwrtb-00000000000000000",
        "lgw-rtb-059615ef7d000000",
    ];
    // Each operation beside the resource noun ITS refusal names the gateway by.
    let actions = [
        ("CreateLocalGatewayRouteTable", "LocalGateway"),
        ("CreateLocalGatewayVirtualInterfaceGroup", "local-gateway"),
    ];

    for (action, noun) in actions {
        for (id, expected_code, expected_sentence) in unknown
            .iter()
            .map(|id| {
                (
                    *id,
                    ec2::state::INVALID_LOCAL_GATEWAY_ID_NOT_FOUND,
                    format!("The {noun} ID '{id}' does not exist"),
                )
            })
            .chain(malformed.iter().map(|id| {
                (
                    *id,
                    ec2::state::INVALID_LOCAL_GATEWAY_ID_MALFORMED,
                    format!("The local-gateway ID '{id}' is malformed"),
                )
            }))
        {
            let case = format!("{action} with {id:?}");
            let (status, body) = super::ec2_query::call(
                &router,
                REGION,
                action,
                &[("LocalGatewayId".to_string(), id.to_string())],
            )
            .await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400, got {status} with body {body}"
            );
            assert_eq!(
                error_code(&body),
                expected_code,
                "case {case:?}: expected {expected_code:?}, got {body}"
            );
            // The EC2 error envelope is XML, so the message's apostrophes arrive escaped.
            assert!(
                unescaped(&body).contains(&expected_sentence),
                "case {case:?}: expected the message {expected_sentence:?}, got {body}"
            );
        }
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
        "DryRun=true: a dry run reports no gateway, got {body}"
    );

    let reported = described_ids(
        &router,
        "DryRun=false",
        &[("DryRun".to_string(), "false".to_string())],
    )
    .await;
    assert_eq!(
        reported, seeded,
        "DryRun=false: expected every stored gateway, got {reported:?}"
    );
}
