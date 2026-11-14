// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the secondary network slice: `CreateSecondaryNetwork` and
//! `DescribeSecondaryNetworks`, driven over the wire the service actually speaks —
//! `ec2Query`, through the crate's shared [`super::ec2_query`] harness.
//!
//! The rules under test are:
//!
//! 1. `CreateSecondaryNetwork` MINTS A NETWORK FROM THE REQUEST and
//!    `DescribeSecondaryNetworks` REPORTS THE STORE: the CIDR block and the type the
//!    request names round-trip, the members the service owns (the network's id, its ARN,
//!    the association id of its CIDR block, the owning account) are derived rather than
//!    taken, and the reported set is the account's networks narrowed by the
//!    `SecondaryNetworkIds` and `Filters` the request names and cut into `MaxResults`-sized
//!    pages;
//! 2. a new network is `create-in-progress` with its CIDR block `associating`, and the
//!    CLOCK, not a caller, moves both to `create-complete` / `associated`;
//! 3. a CIDR block outside the published block size is refused with EC2's own error code
//!    and nothing is stored;
//! 4. one `ClientToken` names one network;
//! 5. `DeleteSecondaryNetwork` puts the network the request names into
//!    `delete-in-progress` with its CIDR block `disassociating`, the CLOCK moves both to
//!    `delete-complete` / `disassociated`, the store keeps reporting the network in those
//!    states (`DescribeSecondaryNetworks` documents both among its `state` filter's
//!    values), a repeat of the delete reports that same deletion without restarting its
//!    clock, and an id this service cannot resolve is refused with EC2's own code for the
//!    question it could not answer — `Invalid<Resource>Id.Malformed` for something that is
//!    not an id of this kind at all, before any lookup, and `Invalid<Resource>Id.NotFound`
//!    for a well-formed id no network has — while nothing changes.
//!
//! Each table row is a distinct input along what the rule generalises over — the block's
//! prefix length at each published bound and one past it, a block whose host bits are set,
//! every modelled network type, which filter, how many values, which ids, which page size —
//! so no row is the specification.

use std::time::Duration;

use service_com_amazonaws_ec2_amazonec2 as ec2;
use ec2::ops::types::SECONDARY_NETWORK_TYPE_VALUES;
use ec2::state::{
    MAX_CLIENT_TOKEN_CHARS, MAX_SECONDARY_NETWORK_IPV4_PREFIX, MIN_SECONDARY_NETWORK_IPV4_PREFIX,
    SECONDARY_NETWORK_ID_PREFIX, TRANSITION_DELAY,
};
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{element, elements, error_code, filters, indexed, unescaped};

/// The Region every call here is signed for. It is the Region a network's ARN names, so
/// the ARN assertions are derived from it.
const REGION: &str = "us-east-1";

/// A `(key, value)` request parameter.
fn p(key: &str, value: impl Into<String>) -> (String, String) {
    (key.to_string(), value.into())
}

/// One request's form parameters, as the ec2Query harness takes them.
type Params = Vec<(String, String)>;

fn emulator() -> MultiServiceRouter {
    super::ec2_query::emulator().0
}

async fn create(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    super::ec2_query::call(router, REGION, "CreateSecondaryNetwork", params).await
}

async fn describe(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    super::ec2_query::call(router, REGION, "DescribeSecondaryNetworks", params).await
}

async fn delete(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    super::ec2_query::call(router, REGION, "DeleteSecondaryNetwork", params).await
}

/// Wait until every clock-owed transition started before now has certainly fallen due.
/// Derived from the service's own span, not a literal, so the two cannot drift.
async fn past_the_transition() {
    tokio::time::sleep(TRANSITION_DELAY + Duration::from_millis(200)).await;
}

/// The one network type every row that does not vary the type is created with: the first
/// value of the modelled `SecondaryNetworkType` enum
/// (`smithy_query(query="shape",shape="SecondaryNetworkType")`, re-exported as
/// [`SECONDARY_NETWORK_TYPE_VALUES`]).
fn a_network_type() -> &'static str {
    SECONDARY_NETWORK_TYPE_VALUES
        .first()
        .expect("the modelled SecondaryNetworkType enum has at least one value")
}

/// One `CreateSecondaryNetwork` call for `cidr_block` and `network_type`, with whatever
/// extra parameters the case needs.
async fn create_network(
    router: &MultiServiceRouter,
    cidr_block: &str,
    network_type: &str,
    extra: Params,
) -> (u16, String) {
    let mut params = vec![
        p("Ipv4CidrBlock", cidr_block),
        p("NetworkType", network_type),
    ];
    params.extend(extra);
    create(router, &params).await
}

/// Create one network and answer the id it was given, failing the case if the create did
/// not succeed.
async fn created(
    router: &MultiServiceRouter,
    case: &str,
    cidr_block: &str,
    tags: &[(&str, &str)],
) -> String {
    let (status, body) = create_network(router, cidr_block, a_network_type(), tag_params(tags)).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CreateSecondaryNetwork, got {status} with \
         body {body}"
    );
    element(&body, "secondaryNetworkId")
        .unwrap_or_else(|| panic!("case {case:?}: expected a secondaryNetworkId in {body}"))
}

/// `TagSpecifications` form fields that tag the network the call creates.
fn tag_params(tags: &[(&str, &str)]) -> Params {
    if tags.is_empty() {
        return Params::new();
    }
    let mut params = vec![p("TagSpecification.1.ResourceType", "secondary-network")];
    for (index, (key, value)) in tags.iter().enumerate() {
        params.push(p(&format!("TagSpecification.1.Tag.{}.Key", index + 1), *key));
        params.push(p(
            &format!("TagSpecification.1.Tag.{}.Value", index + 1),
            *value,
        ));
    }
    params
}

/// The ids `DescribeSecondaryNetworks` reports for `params`, in the order it reported them.
async fn described_ids(
    router: &MultiServiceRouter,
    case: &str,
    params: &[(String, String)],
) -> Vec<String> {
    let (status, body) = describe(router, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeSecondaryNetworks, got {status} with \
         body {body}"
    );
    elements(&body, "secondaryNetworkId")
}

/// A WELL-FORMED secondary network id — the long EC2 resource id form under this
/// resource's own prefix, which is what the service mints — that no account of this test
/// file holds a network for: the mint counts up from one, so an all-`f` suffix is never
/// issued. The prefix comes from the service rather than a literal, so the two cannot
/// drift.
fn unknown_network_id() -> String {
    format!("{SECONDARY_NETWORK_ID_PREFIX}-0ffffffffffffffff")
}

/// One `MaxResults` parameter.
fn max_results_param(max_results: i64) -> Params {
    vec![p("MaxResults", max_results.to_string())]
}

/// The pair of states a secondary network reports — its own, then the one of its IPv4 CIDR
/// block association — in the order a response carries them, as `elements(body, "state")`
/// answers them.
fn state_pair(network: &str, association: &str) -> Vec<String> {
    vec![network.to_string(), association.to_string()]
}

/// What one seeded network is created with: its CIDR block and its tags.
struct NetworkSpec {
    cidr_block: &'static str,
    tags: Vec<(&'static str, &'static str)>,
}

/// The networks the filter, id and paging tables are written against. They differ along
/// every attribute the documented filters select on: the CIDR block (and its prefix
/// length) and the tags.
fn seeded_specs() -> Vec<NetworkSpec> {
    vec![
        NetworkSpec {
            cidr_block: "10.0.0.0/16",
            tags: vec![("Name", "web"), ("Owner", "TeamA")],
        },
        NetworkSpec {
            cidr_block: "10.1.0.0/20",
            tags: vec![("Name", "batch"), ("Owner", "TeamB")],
        },
        NetworkSpec {
            cidr_block: "10.2.0.0/28",
            tags: vec![("Name", "web")],
        },
        NetworkSpec {
            cidr_block: "192.168.0.0/24",
            tags: Vec::new(),
        },
        NetworkSpec {
            cidr_block: "172.16.0.0/12",
            tags: vec![("Owner", "TeamA")],
        },
        NetworkSpec {
            cidr_block: "10.3.0.0/16",
            tags: vec![("Project", "atlas")],
        },
    ]
}

/// What a seeded emulator holds, for the tables to name rows by.
struct Seeded {
    /// Every network id, in the order `DescribeSecondaryNetworks` reports them.
    ids: Vec<String>,
    /// The ARN of each network, in the same order.
    arns: Vec<String>,
    /// The IPv4 CIDR block association id of each network, in the same order.
    association_ids: Vec<String>,
}

/// Create the seeded networks through the API — no test reaches into the store — and
/// report what they were given.
async fn seed(router: &MultiServiceRouter) -> Seeded {
    let mut seeded = Seeded {
        ids: Vec::new(),
        arns: Vec::new(),
        association_ids: Vec::new(),
    };
    for spec in seeded_specs() {
        let case = format!("seed {}", spec.cidr_block);
        let (status, body) =
            create_network(router, spec.cidr_block, a_network_type(), tag_params(&spec.tags)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from CreateSecondaryNetwork, got {status} with \
             body {body}"
        );
        seeded.ids.push(
            element(&body, "secondaryNetworkId")
                .unwrap_or_else(|| panic!("case {case:?}: expected a secondaryNetworkId in {body}")),
        );
        seeded.arns.push(
            element(&body, "secondaryNetworkArn")
                .unwrap_or_else(|| panic!("case {case:?}: expected a secondaryNetworkArn in {body}")),
        );
        seeded.association_ids.push(
            element(&body, "associationId")
                .unwrap_or_else(|| panic!("case {case:?}: expected an associationId in {body}")),
        );
    }
    seeded
}

/// The ids at `indices` of a seeded emulator, in store order.
fn ids_at(seeded: &Seeded, indices: &[usize]) -> Vec<String> {
    indices
        .iter()
        .map(|index| seeded.ids[*index].clone())
        .collect()
}

// ---------------------------------------------------------------------------
// RULE 1: the store is what is reported
// ---------------------------------------------------------------------------

/// A fresh account holds no secondary network — nothing in AWS creates one for it — and
/// describing them is a READ: two calls report the same empty set.
#[tokio::test]
async fn a_fresh_account_holds_no_secondary_network_and_describing_changes_nothing() {
    let router = emulator();

    for case in ["first read", "second read"] {
        let (status, body) = describe(&router, &[]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}"
        );
        assert!(
            elements(&body, "secondaryNetworkId").is_empty(),
            "case {case:?}: a fresh account reports no secondary network, got {body}"
        );
        assert!(
            elements(&body, "nextToken").is_empty(),
            "case {case:?}: an exhausted report carries no token, got {body}"
        );
    }
}

/// Every member `CreateSecondaryNetwork` is given round-trips through
/// `DescribeSecondaryNetworks`, and every member the service owns is derived: the id, the
/// ARN built from it, the association id of the CIDR block and the owning account. The
/// CIDR block is stored in its canonical form, whatever form the request wrote it in.
#[tokio::test]
async fn a_created_network_round_trips_through_describe_in_canonical_form() {
    // Rows: the published bounds of the block size, a size between them, and two blocks
    // whose host bits are set — "we modify the specified CIDR block to its canonical form"
    // is the rule every IPv4 block of this service is stored by.
    let cases: Vec<(&str, &str)> = vec![
        ("10.0.0.0/16", "10.0.0.0/16"),
        ("172.16.0.0/12", "172.16.0.0/12"),
        ("10.9.9.0/28", "10.9.9.0/28"),
        ("10.1.2.3/24", "10.1.2.0/24"),
        ("100.68.0.18/18", "100.68.0.0/18"),
    ];

    for (requested, canonical) in cases {
        // Every modelled network type, so the catalog is covered rather than one entry of
        // it.
        for network_type in SECONDARY_NETWORK_TYPE_VALUES {
            let case = format!("{requested} as {network_type}");
            let router = emulator();
            let (status, body) = create_network(
                &router,
                requested,
                network_type,
                tag_params(&[("Name", "round-trip"), ("Owner", "TeamA")]),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200, got {status} with body {body}"
            );

            let id = element(&body, "secondaryNetworkId")
                .unwrap_or_else(|| panic!("case {case:?}: expected a secondaryNetworkId in {body}"));
            let expected_arn =
                engine::emulator_arn("ec2", REGION, &format!("secondary-network/{id}"));
            let expected_members = [
                ("secondaryNetworkArn", expected_arn.clone()),
                ("ownerId", engine::emulator_account_id().to_string()),
                ("type", (*network_type).to_string()),
                ("cidrBlock", canonical.to_string()),
            ];
            for (member, value) in &expected_members {
                assert_eq!(
                    element(&body, member).as_deref(),
                    Some(value.as_str()),
                    "case {case:?}: expected <{member}>{value}</{member}>, got {body}"
                );
            }
            // The network's own state, then its CIDR block association's: a network is
            // created before either has finished.
            assert_eq!(
                elements(&body, "state"),
                state_pair("create-in-progress", "associating"),
                "case {case:?}: expected the states of a brand new network, got {body}"
            );
            let association_id = element(&body, "associationId")
                .unwrap_or_else(|| panic!("case {case:?}: expected an associationId in {body}"));
            assert_ne!(
                association_id, id,
                "case {case:?}: the association has its own id, got {association_id} for {id}"
            );

            // The same network, read back through the describe.
            let (status, read) = describe(&router, &indexed("SecondaryNetworkId", &[&id])).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200 from the read back, got {status} with {read}"
            );
            assert_eq!(
                elements(&read, "secondaryNetworkId"),
                vec![id.clone()],
                "case {case:?}: expected exactly the created network, got {read}"
            );
            for (member, value) in &expected_members {
                assert_eq!(
                    element(&read, member).as_deref(),
                    Some(value.as_str()),
                    "case {case:?}: the read back must report <{member}>{value}</{member}>, \
                     got {read}"
                );
            }
            assert_eq!(
                element(&read, "associationId").as_deref(),
                Some(association_id.as_str()),
                "case {case:?}: the read back must report the same association id, got {read}"
            );
            for (key, value) in [("Name", "round-trip"), ("Owner", "TeamA")] {
                assert!(
                    read.contains(&format!("<key>{key}</key><value>{value}</value>"))
                        || (read.contains(&format!("<key>{key}</key>"))
                            && read.contains(&format!("<value>{value}</value>"))),
                    "case {case:?}: the read back must report the tag {key}={value}, got {read}"
                );
            }
        }
    }
}

/// A network written after a call shows up in the next one, in store order — the answer is
/// computed from the store, not fixed.
#[tokio::test]
async fn every_stored_network_is_reported_and_a_later_create_is_visible() {
    let router = emulator();
    let seeded = seed(&router).await;

    let reported = described_ids(&router, "all networks", &[]).await;
    assert_eq!(
        reported, seeded.ids,
        "all networks: expected every stored network in store order, got {reported:?}"
    );

    let late = created(&router, "a later create", "10.8.0.0/16", &[("Name", "late")]).await;
    let after = described_ids(&router, "after a later create", &[]).await;
    let mut expected = seeded.ids.clone();
    expected.push(late.clone());
    assert_eq!(
        after, expected,
        "after a later create: expected {late:?} to be reported too, got {after:?}"
    );
}

/// Whether `id` is a long-form EC2 id of `prefix` that this service could have ISSUED: 17
/// lowercase hexadecimal digits, leading digit `0`, not all `0` — the form the mint produces
/// and the form the malformed-id refusal is defined against (the same reading
/// `tests/all/capacity_reservation_fleets.rs` writes for `crf-…` ids).
fn is_issued_long_id(prefix: &str, id: &str) -> bool {
    let Some(suffix) = id
        .strip_prefix(prefix)
        .and_then(|rest| rest.strip_prefix('-'))
    else {
        return false;
    };
    suffix.len() == 17
        && suffix.bytes().all(|byte| byte.is_ascii_hexdigit())
        && suffix.bytes().all(|byte| !byte.is_ascii_uppercase())
        && suffix.starts_with('0')
        && suffix.bytes().any(|byte| byte != b'0')
}

/// Every id this service MINTS is one it also accepts: the id form the mint produces and
/// the form the malformed-id refusal is defined against are the same one, so no network can
/// be created under an id its own delete would call malformed.
#[tokio::test]
async fn every_minted_id_is_one_this_service_accepts() {
    let router = emulator();
    let seeded = seed(&router).await;

    for id in &seeded.ids {
        assert!(
            is_issued_long_id(SECONDARY_NETWORK_ID_PREFIX, id),
            "case {id:?}: expected an id of the form \
             {SECONDARY_NETWORK_ID_PREFIX}-<17 lowercase hex digits, leading 0, not all 0>, \
             got {id:?}"
        );
        // And the delete resolves it rather than refusing its form.
        let (status, body) = delete(&router, &[p("SecondaryNetworkId", id.clone())]).await;
        assert_eq!(
            status, 200,
            "case {id:?}: a minted id must resolve, got {status} with body {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// RULE 2: the clock owns the transition
// ---------------------------------------------------------------------------
/// A new network is `create-in-progress` with its block `associating`; the clock moves both
/// on with nobody asking, and reading them never writes.
#[tokio::test]
async fn the_clock_moves_a_new_network_to_create_complete() {
    let router = emulator();
    let id = created(&router, "clock", "10.4.0.0/16", &[("Name", "clock")]).await;
    let by_id = indexed("SecondaryNetworkId", &[&id]);

    let (status, before) = describe(&router, &by_id).await;
    assert_eq!(
        status, 200,
        "before the transition: expected HTTP 200, got {status} with body {before}"
    );
    assert_eq!(
        elements(&before, "state"),
        state_pair("create-in-progress", "associating"),
        "before the transition: a caller acting straight after the create sees the \
         transient states, got {before}"
    );

    past_the_transition().await;

    let (status, after) = describe(&router, &by_id).await;
    assert_eq!(
        status, 200,
        "after the transition: expected HTTP 200, got {status} with body {after}"
    );
    assert_eq!(
        elements(&after, "state"),
        state_pair("create-complete", "associated"),
        "after the transition: the clock owes the network both moves, got {after}"
    );

    let (_, again) = describe(&router, &by_id).await;
    assert_eq!(
        elements(&again, "state"),
        elements(&after, "state"),
        "a read must not change what a later caller sees: {after} then {again}"
    );
    // A terminal state stays terminal, and the set reported is unchanged.
    assert_eq!(
        elements(&again, "secondaryNetworkId"),
        vec![id],
        "a read must report the same network, got {again}"
    );
}

/// The `state` filters compare against the states the caller is shown, so a network selects
/// on the state the clock has reached rather than the one it was stored with.
#[tokio::test]
async fn the_state_filters_select_on_the_state_the_clock_has_reached() {
    let router = emulator();
    let id = created(&router, "state filter", "10.5.0.0/16", &[]).await;

    let in_progress = described_ids(
        &router,
        "create-in-progress",
        &filters(&[("state", &["create-in-progress"])]),
    )
    .await;
    assert_eq!(
        in_progress,
        vec![id.clone()],
        "create-in-progress: expected the new network, got {in_progress:?}"
    );

    past_the_transition().await;

    let cases: Vec<(&str, &str, Vec<String>)> = vec![
        ("state", "create-in-progress", Vec::new()),
        ("state", "create-complete", vec![id.clone()]),
        ("ipv4-cidr-block-association.state", "associating", Vec::new()),
        (
            "ipv4-cidr-block-association.state",
            "associated",
            vec![id.clone()],
        ),
    ];
    for (name, value, expected) in cases {
        let case = format!("{name}={value}");
        let reported = described_ids(&router, &case, &filters(&[(name, &[value])])).await;
        assert_eq!(
            reported, expected,
            "case {case:?}: expected {expected:?}, got {reported:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// RULE 3: the published block size
// ---------------------------------------------------------------------------

/// The block size the model publishes is the rule: each published bound is accepted and
/// one past it is refused, as is anything that is not an IPv4 CIDR block at all — and a
/// refused create stores nothing.
#[tokio::test]
async fn the_published_block_size_is_the_rule() {
    // Rows: each published bound, one past each, and five values that are not blocks.
    let accepted = [
        MIN_SECONDARY_NETWORK_IPV4_PREFIX,
        MIN_SECONDARY_NETWORK_IPV4_PREFIX + 1,
        MAX_SECONDARY_NETWORK_IPV4_PREFIX - 1,
        MAX_SECONDARY_NETWORK_IPV4_PREFIX,
    ];
    for prefix_length in accepted {
        let router = emulator();
        let cidr_block = format!("10.0.0.0/{prefix_length}");
        let (status, body) = create_network(&router, &cidr_block, a_network_type(), Params::new()).await;
        assert_eq!(
            status, 200,
            "case {cidr_block:?}: a published block size is accepted, got {status} with \
             body {body}"
        );
    }

    let refused: Vec<String> = vec![
        format!("10.0.0.0/{}", MIN_SECONDARY_NETWORK_IPV4_PREFIX - 1),
        format!("10.0.0.0/{}", MAX_SECONDARY_NETWORK_IPV4_PREFIX + 1),
        "10.0.0.0/0".to_string(),
        "10.0.0.0/33".to_string(),
        "10.0.0.0".to_string(),
        "not-a-cidr-block/16".to_string(),
        "10.0.0.0/sixteen".to_string(),
        String::new(),
    ];
    for cidr_block in refused {
        let router = emulator();
        let (status, body) = create_network(&router, &cidr_block, a_network_type(), Params::new()).await;
        assert_eq!(
            status, 400,
            "case {cidr_block:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidParameterValue",
            "case {cidr_block:?}: expected InvalidParameterValue, got {body}"
        );
        let reported = described_ids(&router, &cidr_block, &[]).await;
        assert!(
            reported.is_empty(),
            "case {cidr_block:?}: a refused create stores nothing, got {reported:?}"
        );
    }
}

/// A `TagSpecifications` entry naming any resource type other than the one this call
/// creates is refused, and nothing is stored.
#[tokio::test]
async fn a_tag_specification_for_another_resource_type_is_refused() {
    for resource_type in ["vpc", "subnet", "volume", "instance"] {
        let router = emulator();
        let mut tags = tag_params(&[("Name", "nope")]);
        tags[0] = p("TagSpecification.1.ResourceType", resource_type);
        let (status, body) = create_network(&router, "10.0.0.0/16", a_network_type(), tags).await;
        assert_eq!(
            status, 400,
            "case {resource_type:?}: expected HTTP 400, got {status} with body {body}"
        );
        let reported = described_ids(&router, resource_type, &[]).await;
        assert!(
            reported.is_empty(),
            "case {resource_type:?}: a refused create stores nothing, got {reported:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// RULE 1: filters
// ---------------------------------------------------------------------------

/// Each documented filter selects on its own attribute, values within one filter are OR'd,
/// and the comparison is exact, case-sensitive and carries EC2's wildcards.
///
/// The two `state` filters are left to
/// [`the_state_filters_select_on_the_state_the_clock_has_reached`], which pins them either
/// side of the clock-owed transition: a table over six seeded networks must not depend on
/// how long seeding took.
#[tokio::test]
async fn each_documented_filter_selects_the_networks_whose_attribute_equals_a_value() {
    let router = emulator();
    let seeded = seed(&router).await;
    let account = engine::emulator_account_id().to_string();
    let all: Vec<usize> = (0..seeded.ids.len()).collect();

    // One row per filter name `DescribeSecondaryNetworksRequest$Filters` documents (plus
    // the variable-key `tag:<key>` form), each asserted on a network the issue never cited.
    let cases: Vec<(&str, Vec<String>, Vec<usize>)> = vec![
        (
            "ipv4-cidr-block-association.cidr-block",
            vec!["192.168.0.0/24".into()],
            vec![3],
        ),
        (
            "ipv4-cidr-block-association.cidr-block",
            vec!["10.0.0.0/16".into(), "10.3.0.0/16".into()],
            vec![0, 5],
        ),
        // A block no network has: an empty set, not an error.
        (
            "ipv4-cidr-block-association.cidr-block",
            vec!["10.7.0.0/16".into()],
            vec![],
        ),
        (
            "ipv4-cidr-block-association.association-id",
            vec![seeded.association_ids[1].clone()],
            vec![1],
        ),
        ("owner-id", vec![account], all.clone()),
        ("owner-id", vec!["111122223333".into()], vec![]),
        (
            "secondary-network-id",
            vec![seeded.ids[4].clone()],
            vec![4],
        ),
        (
            "secondary-network-id",
            vec![seeded.ids[0].clone(), seeded.ids[3].clone()],
            vec![0, 3],
        ),
        (
            "secondary-network-arn",
            vec![seeded.arns[2].clone()],
            vec![2],
        ),
        ("type", vec![a_network_type().into()], all.clone()),
        ("type", vec!["ethernet".into()], vec![]),
        ("tag-key", vec!["Owner".into()], vec![0, 1, 4]),
        ("tag-key", vec!["Project".into()], vec![5]),
        ("tag:Owner", vec!["TeamA".into()], vec![0, 4]),
        ("tag:Name", vec!["web".into()], vec![0, 2]),
        // "Filter values are case-sensitive" (`Filter$Values`).
        ("tag:Owner", vec!["teama".into()], vec![]),
        // Filter values carry EC2's wildcards: `*` for zero or more characters, `?` for
        // exactly one ("List and filter using the CLI and API", the guide the modelled
        // `Filter` shape links to).
        (
            "ipv4-cidr-block-association.cidr-block",
            vec!["10.*".into()],
            vec![0, 1, 2, 5],
        ),
        (
            "ipv4-cidr-block-association.cidr-block",
            vec!["10.?.0.0/16".into()],
            vec![0, 5],
        ),
        ("tag:Name", vec!["we*".into()], vec![0, 2]),
        ("tag:Name", vec!["*atch".into()], vec![1]),
        ("tag-key", vec!["*".into()], vec![0, 1, 2, 4, 5]),
        // A documented filter carrying no value at all: there is no value for the
        // attribute to match, so it selects nothing rather than everything.
        ("state", vec![], vec![]),
        ("secondary-network-id", vec![], vec![]),
    ];

    for (name, values, expected_indices) in cases {
        let case = format!("{name}={values:?}");
        let borrowed: Vec<&str> = values.iter().map(String::as_str).collect();
        let reported = described_ids(&router, &case, &filters(&[(name, &borrowed)])).await;
        let expected = ids_at(&seeded, &expected_indices);
        assert_eq!(
            reported, expected,
            "case {case:?}: expected {expected:?}, got {reported:?}"
        );
    }
}

/// One row of the AND table: the filters to send, and the indices of the networks that must
/// come back.
type FilterCombination<'a> = (Vec<(&'a str, &'a [&'a str])>, Vec<usize>);

/// Several filters are joined with an AND: only the networks matching every one are
/// reported.
#[tokio::test]
async fn several_filters_are_joined_with_an_and() {
    let router = emulator();
    let seeded = seed(&router).await;

    let cases: Vec<FilterCombination<'_>> = vec![
        (
            vec![("tag:Name", &["web"][..]), ("tag:Owner", &["TeamA"][..])],
            vec![0],
        ),
        (
            vec![
                ("type", &["rdma"][..]),
                ("tag-key", &["Owner"][..]),
                (
                    "ipv4-cidr-block-association.cidr-block",
                    &["10.1.0.0/20"][..],
                ),
            ],
            vec![1],
        ),
        // Each filter matches something, but no network matches both.
        (
            vec![
                ("tag:Project", &["atlas"][..]),
                ("tag:Owner", &["TeamA"][..]),
            ],
            vec![],
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

/// A filter name the operation does not define is refused with EC2's
/// `InvalidParameterValue`, rather than silently reported as an empty set.
#[tokio::test]
async fn an_undefined_filter_name_is_refused() {
    let router = emulator();
    seed(&router).await;

    // Rows: a near-miss of a defined name, the underscore spelling, a defined name with the
    // wrong case, the tag filter without its key separator, the tag filter with a separator
    // but no key, a filter another operation defines, and an empty name.
    let cases = [
        "secondary-network-ids",
        "ipv4-cidr-block-association.cidr",
        "owner_id",
        "Owner-Id",
        "tag",
        "tag:",
        "cidr",
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
    }
}

// ---------------------------------------------------------------------------
// RULE 1: SecondaryNetworkIds
// ---------------------------------------------------------------------------

/// `SecondaryNetworkIds` narrows the report to the ids named, in store order whatever order
/// they were asked for.
#[tokio::test]
async fn secondary_network_ids_narrow_the_report_to_the_ids_named() {
    let router = emulator();
    let seeded = seed(&router).await;

    // Rows differ in count and in the order the ids are asked for.
    let cases: Vec<Vec<usize>> = vec![
        vec![0],
        vec![5],
        vec![2, 4],
        vec![4, 2],
        vec![3, 1, 5],
        (0..seeded.ids.len()).collect(),
    ];
    for asked in cases {
        let ids = ids_at(&seeded, &asked);
        let borrowed: Vec<&str> = ids.iter().map(String::as_str).collect();
        let case = format!("SecondaryNetworkIds={borrowed:?}");
        let reported =
            described_ids(&router, &case, &indexed("SecondaryNetworkId", &borrowed)).await;
        let mut expected = asked.clone();
        expected.sort_unstable();
        let expected = ids_at(&seeded, &expected);
        assert_eq!(
            reported, expected,
            "case {case:?}: expected {expected:?}, got {reported:?}"
        );
    }
}

/// A `SecondaryNetworkIds` entry this service cannot resolve is refused — even alongside
/// ids that do exist — and with the SAME code `DeleteSecondaryNetwork` answers for it: the
/// id's form is read before the lookup wherever an id of this kind is read, so a reader and
/// a writer never disagree about what an id is.
#[tokio::test]
async fn a_secondary_network_id_that_names_no_network_is_refused() {
    let router = emulator();
    let seeded = seed(&router).await;
    let known = seeded.ids[0].clone();

    // Rows: a well-formed id of this service's own form that nothing has (alone, and beside
    // a known id in both orders) — not found; then a short-form id, a value that is not an
    // id at all, and an unresolvable id beside a known one — malformed.
    let cases: Vec<(Vec<String>, &str)> = vec![
        (
            vec![unknown_network_id()],
            "InvalidSecondaryNetworkId.NotFound",
        ),
        (
            vec![known.clone(), unknown_network_id()],
            "InvalidSecondaryNetworkId.NotFound",
        ),
        (
            vec![unknown_network_id(), known.clone()],
            "InvalidSecondaryNetworkId.NotFound",
        ),
        (
            vec![format!("{SECONDARY_NETWORK_ID_PREFIX}-0abc1234")],
            "InvalidSecondaryNetworkId.Malformed",
        ),
        (
            vec!["not-a-secondary-network-id".to_string()],
            "InvalidSecondaryNetworkId.Malformed",
        ),
        (
            vec![known, "not-a-secondary-network-id".to_string()],
            "InvalidSecondaryNetworkId.Malformed",
        ),
    ];
    for (ids, expected_code) in cases {
        let borrowed: Vec<&str> = ids.iter().map(String::as_str).collect();
        let case = format!("SecondaryNetworkIds={borrowed:?}");
        let (status, body) = describe(&router, &indexed("SecondaryNetworkId", &borrowed)).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {case:?}: expected {expected_code}, got {body}"
        );
        assert!(
            elements(&body, "secondaryNetworkId").is_empty(),
            "case {case:?}: a refused request reports no network, got {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// RULE 1: pagination
// ---------------------------------------------------------------------------

/// Whatever the page size, walking the pages covers every network exactly once, in store
/// order, and the last page carries no token.
#[tokio::test]
async fn paging_covers_every_network_exactly_once_for_each_page_size() {
    let router = emulator();
    let seeded = seed(&router).await;

    // `DescribeSecondaryNetworksMaxResults` is modelled `range 5..1000`, so the rows are
    // its bottom bound, a size that divides the store exactly, one that does not, and the
    // size that holds everything.
    for max_results in [5, 6, 7, 1000] {
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
            let page = elements(&body, "secondaryNetworkId");
            assert!(
                page.len() <= usize::try_from(max_results).unwrap_or_default(),
                "case {case:?}: a page may not exceed MaxResults, got {page:?}"
            );
            walked.extend(page);
            pages += 1;
            assert!(
                pages <= seeded.ids.len() + 1,
                "case {case:?}: paging did not terminate, walked {walked:?}"
            );
            match elements(&body, "nextToken").first() {
                Some(token) => {
                    params = max_results_param(max_results);
                    params.push(p("NextToken", token.clone()));
                }
                None => break,
            }
        }
        assert_eq!(
            walked, seeded.ids,
            "case {case:?}: expected every network exactly once in store order, got {walked:?}"
        );
    }
}

/// A page is a cut of the FILTERED set, not of the whole store: `MaxResults` and a filter
/// compose, and a network the filter excludes appears on no page at all.
#[tokio::test]
async fn a_page_is_cut_from_the_filtered_set() {
    let router = emulator();
    let seeded = seed(&router).await;
    // Two more tagged networks, so the filtered set is larger than the smallest page size
    // the model admits (`range 5..1000`) and therefore spans two pages. The one seeded
    // network with no tags at all (index 3) is outside the filtered set.
    let mut tagged = ids_at(&seeded, &[0, 1, 2, 4, 5]);
    for (cidr_block, name) in [("10.10.0.0/16", "extra-one"), ("10.11.0.0/16", "extra-two")] {
        tagged.push(created(&router, name, cidr_block, &[("Name", name)]).await);
    }

    let selector: Vec<(&str, &[&str])> = vec![("tag-key", &["*"][..])];
    let mut params = filters(&selector);
    params.extend(max_results_param(5));

    let (status, body) = describe(&router, &params).await;
    assert_eq!(status, 200, "filtered first page: got {status} {body}");
    let first = elements(&body, "secondaryNetworkId");
    assert_eq!(
        first,
        tagged[..5].to_vec(),
        "filtered first page: expected the first five tagged networks, got {first:?}"
    );
    let token = elements(&body, "nextToken")
        .first()
        .cloned()
        .unwrap_or_else(|| panic!("filtered first page: expected a NextToken in {body}"));

    let mut next = filters(&selector);
    next.extend(max_results_param(5));
    next.push(p("NextToken", token));
    let second = described_ids(&router, "filtered second page", &next).await;
    assert_eq!(
        second,
        tagged[5..].to_vec(),
        "filtered second page: expected the remaining tagged networks, got {second:?}"
    );
    assert!(
        !first.contains(&seeded.ids[3]) && !second.contains(&seeded.ids[3]),
        "the untagged network is outside the filtered set, so it is on no page: \
         {first:?} then {second:?}"
    );
}

/// `MaxResults` outside the range the model publishes is refused at the boundary; each
/// published bound itself is accepted.
#[tokio::test]
async fn max_results_is_accepted_at_its_bounds_and_refused_past_them() {
    let router = emulator();
    seed(&router).await;

    // The published bounds (`range 5..1000`) and one past each.
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
    let router = emulator();
    let seeded = seed(&router).await;

    // Rows: free text, punctuation, a hex string this service never minted, the bare
    // resource id a caller might guess the token is, that id hex-encoded, and a token
    // carrying this service's marker with a checksum that does not match.
    let forged_marker = ::hex::encode(format!("ec2-1:0000000000000000:{}", seeded.ids[0]));
    let cases = [
        "not-a-token".to_string(),
        "###".to_string(),
        "6161".to_string(),
        seeded.ids[0].clone(),
        ::hex::encode(&seeded.ids[0]),
        forged_marker,
    ];
    for token in cases {
        let (status, body) = describe(&router, &[p("NextToken", token.clone())]).await;
        assert_eq!(
            status, 400,
            "case {token:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidPaginationToken",
            "case {token:?}: expected InvalidPaginationToken, got {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// RULE 4: one ClientToken names one network
// ---------------------------------------------------------------------------

/// A repeat of a create carrying the same `ClientToken` reports the network that token
/// already created and creates nothing; a different token creates another network. The
/// token is echoed only when the request carried one.
#[tokio::test]
async fn one_client_token_names_one_network() {
    let router = emulator();

    let (status, first) = create_network(
        &router,
        "10.0.0.0/16",
        a_network_type(),
        vec![p("ClientToken", "token-one")],
    )
    .await;
    assert_eq!(
        status, 200,
        "first create: expected HTTP 200, got {status} with body {first}"
    );
    let id = element(&first, "secondaryNetworkId")
        .unwrap_or_else(|| panic!("first create: expected a secondaryNetworkId in {first}"));
    assert_eq!(
        element(&first, "clientToken").as_deref(),
        Some("token-one"),
        "first create: the token the request carried is echoed, got {first}"
    );

    // The same token, and a DIFFERENT CIDR block: the token names the network it created,
    // so the replay reports that one.
    let (status, replay) = create_network(
        &router,
        "10.6.0.0/16",
        a_network_type(),
        vec![p("ClientToken", "token-one")],
    )
    .await;
    assert_eq!(
        status, 200,
        "replay: expected HTTP 200, got {status} with body {replay}"
    );
    assert_eq!(
        element(&replay, "secondaryNetworkId").as_deref(),
        Some(id.as_str()),
        "replay: expected the network the token created, got {replay}"
    );

    let (status, second) = create_network(
        &router,
        "10.6.0.0/16",
        a_network_type(),
        vec![p("ClientToken", "token-two")],
    )
    .await;
    assert_eq!(
        status, 200,
        "second token: expected HTTP 200, got {status} with body {second}"
    );
    let second_id = element(&second, "secondaryNetworkId")
        .unwrap_or_else(|| panic!("second token: expected a secondaryNetworkId in {second}"));
    assert_ne!(
        second_id, id,
        "second token: a different token creates a different network, got {second_id}"
    );

    let (status, untokened) =
        create_network(&router, "10.7.0.0/16", a_network_type(), Params::new()).await;
    assert_eq!(
        status, 200,
        "no token: expected HTTP 200, got {status} with body {untokened}"
    );
    assert!(
        elements(&untokened, "clientToken").is_empty(),
        "no token: the token is reported only if the request carried one, got {untokened}"
    );

    let reported = described_ids(&router, "after the replay", &[]).await;
    assert_eq!(
        reported.len(),
        3,
        "after the replay: expected exactly the three networks created, got {reported:?}"
    );
}

// ---------------------------------------------------------------------------
// DryRun
// ---------------------------------------------------------------------------

/// `DryRun` reports the permitted outcome — `DryRunOperation`, HTTP 412 — and neither
/// creates nor describes; `DryRun=false` is an ordinary call.
#[tokio::test]
async fn dry_run_reports_the_permitted_outcome_without_doing_the_work() {
    let router = emulator();

    let (status, body) = create_network(
        &router,
        "10.0.0.0/16",
        a_network_type(),
        vec![p("DryRun", "true")],
    )
    .await;
    assert_eq!(
        status, 412,
        "create DryRun=true: expected HTTP 412, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "DryRunOperation",
        "create DryRun=true: expected DryRunOperation, got {body}"
    );
    assert!(
        described_ids(&router, "after a dry-run create", &[])
            .await
            .is_empty(),
        "create DryRun=true: a dry run stores nothing"
    );

    // A dry run of a request that WOULD have been refused is refused with that reason
    // instead.
    let (status, body) = create_network(
        &router,
        "10.0.0.0/8",
        a_network_type(),
        vec![p("DryRun", "true")],
    )
    .await;
    assert_eq!(
        status, 400,
        "create DryRun=true with a bad block: expected HTTP 400, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidParameterValue",
        "create DryRun=true with a bad block: expected InvalidParameterValue, got {body}"
    );

    let (status, body) = create_network(
        &router,
        "10.0.0.0/16",
        a_network_type(),
        vec![p("DryRun", "false")],
    )
    .await;
    assert_eq!(
        status, 200,
        "create DryRun=false: expected HTTP 200, got {status} with body {body}"
    );
    let id = element(&body, "secondaryNetworkId")
        .unwrap_or_else(|| panic!("create DryRun=false: expected a secondaryNetworkId in {body}"));

    let (status, body) = describe(&router, &[p("DryRun", "true")]).await;
    assert_eq!(
        status, 412,
        "describe DryRun=true: expected HTTP 412, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "DryRunOperation",
        "describe DryRun=true: expected DryRunOperation, got {body}"
    );
    assert!(
        elements(&body, "secondaryNetworkId").is_empty(),
        "describe DryRun=true: a dry run reports no network, got {body}"
    );

    let reported = described_ids(&router, "describe DryRun=false", &[p("DryRun", "false")]).await;
    assert_eq!(
        reported,
        vec![id],
        "describe DryRun=false: expected the ordinary report, got {reported:?}"
    );
}

// ---------------------------------------------------------------------------
// RULE 5: a delete is a clock-owed move the store keeps reporting
// ---------------------------------------------------------------------------

/// What one delete row creates and how it deletes it.
struct DeleteCase {
    cidr_block: &'static str,
    tags: Vec<(&'static str, &'static str)>,
    client_token: Option<&'static str>,
}

/// The rows the delete table runs. They differ along everything the rule generalises over:
/// the block size (both published bounds and one between them), how many tags the network
/// carries, and whether the delete carries a `ClientToken`.
fn delete_cases() -> Vec<DeleteCase> {
    vec![
        DeleteCase {
            cidr_block: "10.0.0.0/16",
            tags: vec![("Name", "deleted"), ("Owner", "TeamA")],
            client_token: Some("delete-token"),
        },
        DeleteCase {
            cidr_block: "172.16.0.0/12",
            tags: Vec::new(),
            client_token: None,
        },
        DeleteCase {
            cidr_block: "10.9.9.0/28",
            tags: vec![("Project", "atlas")],
            client_token: None,
        },
        DeleteCase {
            cidr_block: "192.168.4.0/24",
            tags: vec![("Name", "deleted")],
            client_token: Some("x"),
        },
    ]
}

/// A delete reports the network it names as the store then holds it — `delete-in-progress`
/// with its CIDR block `disassociating`, every other member the one the create minted — the
/// clock moves both states on with nobody asking, the network stays reportable in them, and
/// a network nobody deleted is untouched.
#[tokio::test]
async fn deleting_a_network_hands_it_to_the_clock_and_the_store_keeps_reporting_it() {
    for row in delete_cases() {
        // Every modelled network type, so the catalog is covered rather than one entry.
        for network_type in SECONDARY_NETWORK_TYPE_VALUES {
            let case = format!(
                "{} as {network_type} with {} tag(s), token {:?}",
                row.cidr_block,
                row.tags.len(),
                row.client_token
            );
            let router = emulator();
            // A second network nobody asks to delete, so the delete's reach is visible.
            let untouched = created(&router, &case, "10.200.0.0/16", &[("Name", "kept")]).await;

            let (status, created_body) = create_network(
                &router,
                row.cidr_block,
                network_type,
                tag_params(&row.tags),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200 from the create, got {status} with \
                 body {created_body}"
            );
            let id = element(&created_body, "secondaryNetworkId").unwrap_or_else(|| {
                panic!("case {case:?}: expected a secondaryNetworkId in {created_body}")
            });

            let mut params = vec![p("SecondaryNetworkId", id.clone())];
            if let Some(token) = row.client_token {
                params.push(p("ClientToken", token));
            }
            let (status, deleted) = delete(&router, &params).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200 from the delete, got {status} with \
                 body {deleted}"
            );
            assert_eq!(
                elements(&deleted, "state"),
                state_pair("delete-in-progress", "disassociating"),
                "case {case:?}: expected the states of a network being deleted, got {deleted}"
            );
            // A delete REPORTS the network it deleted; it mints nothing, so every member
            // the service owns is the one the create gave it.
            for member in [
                "secondaryNetworkId",
                "secondaryNetworkArn",
                "ownerId",
                "type",
                "cidrBlock",
                "associationId",
            ] {
                assert_eq!(
                    element(&deleted, member),
                    element(&created_body, member),
                    "case {case:?}: <{member}> must be the created network's, got {deleted}"
                );
            }
            match row.client_token {
                Some(token) => assert_eq!(
                    element(&deleted, "clientToken").as_deref(),
                    Some(token),
                    "case {case:?}: the token the request carried is echoed, got {deleted}"
                ),
                None => assert!(
                    elements(&deleted, "clientToken").is_empty(),
                    "case {case:?}: the token is reported only if the request carried one, \
                     got {deleted}"
                ),
            }

            // The same network, read back: the deletion is in the store, not just in the
            // answer.
            let by_id = indexed("SecondaryNetworkId", &[&id]);
            let (status, before) = describe(&router, &by_id).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200 from the read back, got {status} \
                 with {before}"
            );
            assert_eq!(
                elements(&before, "state"),
                state_pair("delete-in-progress", "disassociating"),
                "case {case:?}: a caller acting straight after the delete sees the \
                 transient states, got {before}"
            );

            past_the_transition().await;

            let (status, after) = describe(&router, &by_id).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200 after the transition, got {status} \
                 with {after}"
            );
            assert_eq!(
                elements(&after, "state"),
                state_pair("delete-complete", "disassociated"),
                "case {case:?}: the clock owes the network both moves, got {after}"
            );
            // Reading it twice reports the same thing: a read writes nothing.
            let (_, again) = describe(&router, &by_id).await;
            assert_eq!(
                elements(&again, "state"),
                elements(&after, "state"),
                "case {case:?}: a read must not change what a later caller sees: \
                 {after} then {again}"
            );
            // The record is still the one that was created, tags and all.
            for (key, value) in &row.tags {
                assert!(
                    after.contains(&format!("<key>{key}</key>"))
                        && after.contains(&format!("<value>{value}</value>")),
                    "case {case:?}: a deleted network still reports the tag {key}={value}, \
                     got {after}"
                );
            }

            // The network nobody deleted finished being CREATED instead.
            let (_, kept) = describe(&router, &indexed("SecondaryNetworkId", &[&untouched])).await;
            assert_eq!(
                elements(&kept, "state"),
                state_pair("create-complete", "associated"),
                "case {case:?}: a network nobody deleted is untouched, got {kept}"
            );

            // The delete states are values of the documented filters, so they select the
            // deleted network and only it.
            let selected = described_ids(
                &router,
                &case,
                &filters(&[("state", &["delete-complete"])]),
            )
            .await;
            assert_eq!(
                selected,
                vec![id.clone()],
                "case {case:?}: state=delete-complete selects the deleted network, \
                 got {selected:?}"
            );
            let by_association = described_ids(
                &router,
                &case,
                &filters(&[("ipv4-cidr-block-association.state", &["disassociated"])]),
            )
            .await;
            assert_eq!(
                by_association,
                vec![id.clone()],
                "case {case:?}: the association state selects the deleted network, \
                 got {by_association:?}"
            );
            let still_creating = described_ids(
                &router,
                &case,
                &filters(&[("state", &["create-in-progress", "create-complete"])]),
            )
            .await;
            assert_eq!(
                still_creating,
                vec![untouched.clone()],
                "case {case:?}: a deleted network is in neither create state, \
                 got {still_creating:?}"
            );
        }
    }
}

/// An id this service cannot resolve is refused with EC2's own code for the question it
/// could not answer, and nothing is deleted: something that is not a secondary network id
/// at all is `InvalidSecondaryNetworkId.Malformed` BEFORE any lookup, while a well-formed
/// id the account holds no network for is `InvalidSecondaryNetworkId.NotFound`.
#[tokio::test]
async fn deleting_an_id_no_network_has_is_refused_and_changes_nothing() {
    let router = emulator();
    let seeded = seed(&router).await;

    // Rows: every way an id can fall outside the form this service issues — `<prefix>-<17
    // lowercase hex digits>`, leading `0`, not all `0` — and then the one way it can be
    // inside that form and still resolve to nothing. The all-zero suffix is the row the
    // recorded corpus probes; the others vary the prefix, the suffix length, its alphabet,
    // its case and its leading digit.
    let rows: Vec<(String, &str)> = vec![
        (
            format!("{SECONDARY_NETWORK_ID_PREFIX}-00000000000000000"),
            "InvalidSecondaryNetworkId.Malformed",
        ),
        (
            format!("{SECONDARY_NETWORK_ID_PREFIX}-10000000000000000"),
            "InvalidSecondaryNetworkId.Malformed",
        ),
        (
            format!("{SECONDARY_NETWORK_ID_PREFIX}-0abc1234"),
            "InvalidSecondaryNetworkId.Malformed",
        ),
        (
            format!("{SECONDARY_NETWORK_ID_PREFIX}-0123456789abcdefg"),
            "InvalidSecondaryNetworkId.Malformed",
        ),
        (
            format!("{SECONDARY_NETWORK_ID_PREFIX}-0123456789ABCDEF0"),
            "InvalidSecondaryNetworkId.Malformed",
        ),
        (
            format!("{SECONDARY_NETWORK_ID_PREFIX}-0123456789abcdef00"),
            "InvalidSecondaryNetworkId.Malformed",
        ),
        (
            "vpc-0123456789abcdef0".to_string(),
            "InvalidSecondaryNetworkId.Malformed",
        ),
        (
            "not-a-secondary-network-id".to_string(),
            "InvalidSecondaryNetworkId.Malformed",
        ),
        (
            SECONDARY_NETWORK_ID_PREFIX.to_uppercase() + "-0123456789ABCDEF0",
            "InvalidSecondaryNetworkId.Malformed",
        ),
        (String::new(), "InvalidSecondaryNetworkId.Malformed"),
        (
            unknown_network_id(),
            "InvalidSecondaryNetworkId.NotFound",
        ),
        (
            format!("{SECONDARY_NETWORK_ID_PREFIX}-0123456789abcdef0"),
            "InvalidSecondaryNetworkId.NotFound",
        ),
    ];
    for (id, expected_code) in rows {
        let (status, body) = delete(&router, &[p("SecondaryNetworkId", id.clone())]).await;
        assert_eq!(
            status, 400,
            "case {id:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {id:?}: expected {expected_code}, got {body}"
        );
        // The message is built from the request, so it names the value it refused.
        if !id.is_empty() {
            let message = unescaped(&element(&body, "Message").unwrap_or_default());
            assert!(
                message.contains(&id),
                "case {id:?}: expected the refusal to name the id it refused, got {message:?}"
            );
        }
        assert!(
            elements(&body, "secondaryNetworkId").is_empty(),
            "case {id:?}: a refused delete reports no network, got {body}"
        );
    }

    let reported = described_ids(&router, "after the refusals", &[]).await;
    assert_eq!(
        reported, seeded.ids,
        "after the refusals: every stored network is still reported, got {reported:?}"
    );
    let deleting = described_ids(
        &router,
        "after the refusals",
        &filters(&[("state", &["delete-in-progress", "delete-complete"])]),
    )
    .await;
    assert!(
        deleting.is_empty(),
        "after the refusals: no network is being deleted, got {deleting:?}"
    );
}

/// A repeat of a delete is REFUSED — the network is already on its way out, which is no
/// state a delete runs from — and the deletion the first call started keeps its own clock:
/// the network reaches `delete-complete` at the instant that first delete made due.
#[tokio::test]
async fn repeating_a_delete_reports_the_same_deletion_without_restarting_its_clock() {
    let router = emulator();
    let id = created(&router, "repeat", "10.4.0.0/16", &[("Name", "repeat")]).await;
    let params = vec![p("SecondaryNetworkId", id.clone())];

    let (status, first) = delete(&router, &params).await;
    assert_eq!(
        status, 200,
        "first delete: expected HTTP 200, got {status} with body {first}"
    );
    assert_eq!(
        elements(&first, "state"),
        state_pair("delete-in-progress", "disassociating"),
        "first delete: expected the states of a network being deleted, got {first}"
    );

    let (status, immediate_repeat) = delete(&router, &params).await;
    assert_eq!(
        status, 400,
        "immediate repeat: expected HTTP 400, got {status} with body {immediate_repeat}"
    );
    assert_eq!(
        error_code(&immediate_repeat),
        "InvalidState",
        "immediate repeat: expected the wrong-state refusal, got {immediate_repeat}"
    );
    let (_, while_deleting) = describe(&router, &indexed("SecondaryNetworkId", &[&id])).await;
    assert_eq!(
        elements(&while_deleting, "state"),
        state_pair("delete-in-progress", "disassociating"),
        "immediate repeat: a refused repeat leaves the deletion as it was, got \
         {while_deleting}"
    );

    past_the_transition().await;

    // The repeat AFTER the move fell due is what shows the clock was never restarted: a
    // restarted deletion would still read `delete-in-progress` here.
    let (status, late_repeat) = delete(&router, &params).await;
    assert_eq!(
        status, 400,
        "late repeat: expected HTTP 400, got {status} with body {late_repeat}"
    );
    assert_eq!(
        error_code(&late_repeat),
        "InvalidState",
        "late repeat: expected the wrong-state refusal, got {late_repeat}"
    );

    let (_, read) = describe(&router, &indexed("SecondaryNetworkId", &[&id])).await;
    assert_eq!(
        elements(&read, "state"),
        state_pair("delete-complete", "disassociated"),
        "after the late repeat: expected the deleted network, got {read}"
    );
}

/// `DryRun` reports the permitted outcome — `DryRunOperation`, HTTP 412 — and deletes
/// nothing; a dry run of a request that WOULD have been refused is refused with that reason
/// instead.
#[tokio::test]
async fn a_dry_run_delete_reports_the_permitted_outcome_without_deleting() {
    let router = emulator();
    let id = created(&router, "dry run", "10.5.0.0/16", &[]).await;

    let (status, body) = delete(
        &router,
        &[p("SecondaryNetworkId", id.clone()), p("DryRun", "true")],
    )
    .await;
    assert_eq!(
        status, 412,
        "delete DryRun=true: expected HTTP 412, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "DryRunOperation",
        "delete DryRun=true: expected DryRunOperation, got {body}"
    );

    let (_, read) = describe(&router, &indexed("SecondaryNetworkId", &[&id])).await;
    assert!(
        !elements(&read, "state")
            .iter()
            .any(|state| state.starts_with("delete") || state.starts_with("disassoci")),
        "delete DryRun=true: a dry run deletes nothing, got {read}"
    );

    // A dry run naming a network the account does not hold is refused with that reason, and
    // one naming something that is not a secondary network id at all with the malformed
    // reason the lookup never gets to.
    for (id, expected_code) in [
        (unknown_network_id(), "InvalidSecondaryNetworkId.NotFound"),
        (
            "not-a-secondary-network-id".to_string(),
            "InvalidSecondaryNetworkId.Malformed",
        ),
    ] {
        let (status, body) = delete(
            &router,
            &[p("SecondaryNetworkId", id.clone()), p("DryRun", "true")],
        )
        .await;
        assert_eq!(
            status, 400,
            "delete DryRun=true on {id:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            expected_code,
            "delete DryRun=true on {id:?}: expected {expected_code}, got {body}"
        );
    }

    let (status, body) = delete(
        &router,
        &[p("SecondaryNetworkId", id.clone()), p("DryRun", "false")],
    )
    .await;
    assert_eq!(
        status, 200,
        "delete DryRun=false: expected HTTP 200, got {status} with body {body}"
    );
    assert_eq!(
        elements(&body, "state"),
        state_pair("delete-in-progress", "disassociating"),
        "delete DryRun=false: expected an ordinary delete, got {body}"
    );
}

/// A delete's `ClientToken` is held to the published constraint — "Maximum 128 ASCII
/// characters" (`ClientToken`), re-exported as [`MAX_CLIENT_TOKEN_CHARS`] — the bound
/// itself being accepted and anything past it refused with nothing deleted.
#[tokio::test]
async fn a_client_token_past_the_published_limit_is_refused_and_deletes_nothing() {
    // Rows: one below the bound, the bound itself, one past it, and far past it.
    let cases = [
        (MAX_CLIENT_TOKEN_CHARS - 1, true),
        (MAX_CLIENT_TOKEN_CHARS, true),
        (MAX_CLIENT_TOKEN_CHARS + 1, false),
        (MAX_CLIENT_TOKEN_CHARS * 4, false),
    ];
    for (length, accepted) in cases {
        let case = format!("a {length}-character ClientToken");
        let router = emulator();
        let id = created(&router, &case, "10.6.0.0/16", &[]).await;
        let token = "t".repeat(length);

        let (status, body) = delete(
            &router,
            &[p("SecondaryNetworkId", id.clone()), p("ClientToken", token)],
        )
        .await;
        if accepted {
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200, got {status} with body {body}"
            );
            assert_eq!(
                elements(&body, "state"),
                state_pair("delete-in-progress", "disassociating"),
                "case {case:?}: expected an ordinary delete, got {body}"
            );
            continue;
        }
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidParameterValue",
            "case {case:?}: expected InvalidParameterValue, got {body}"
        );
        let (_, read) = describe(&router, &indexed("SecondaryNetworkId", &[&id])).await;
        assert!(
            !elements(&read, "state")
                .iter()
                .any(|state| state.starts_with("delete") || state.starts_with("disassoci")),
            "case {case:?}: a refused delete deletes nothing, got {read}"
        );
    }
}
