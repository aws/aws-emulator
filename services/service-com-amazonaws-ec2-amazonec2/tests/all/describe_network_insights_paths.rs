// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `DescribeNetworkInsightsPaths`, driven over the wire the service
//! actually speaks: `ec2Query` — an `application/x-www-form-urlencoded` POST whose body
//! carries `Action=DescribeNetworkInsightsPaths`, answered with XML (README.md /
//! OPERATIONS.md "Wire protocol").
//!
//! The rules under test are:
//!
//! 1. the describe REPORTS THE PATH STORE and nothing else: every member a stored path
//!    carries — the id it is keyed by, its ARN, its creation date, the source and
//!    destination it names, the addresses, the protocol, the destination port, the filters
//!    scoping it at either end and its tags — is reported as stored, in path-id order, and a
//!    path a later `DeleteNetworkInsightsPath` removes is no longer reported;
//! 2. it is a READ: describing a path changes nothing a later caller sees — the store holds
//!    the same paths, with the same members, however often it is called;
//! 3. `NetworkInsightsPathIds` narrows the report to the paths named, and an id the account
//!    holds no path under is refused with EC2's `InvalidNetworkInsightsPathId.NotFound`
//!    rather than reported as an absent row;
//! 4. each filter name the model documents narrows on the member it names, values within one
//!    filter are joined with OR and separate filters with AND, EC2's `*`/`?` wildcards hold,
//!    a filter naming a member the path does not carry selects nothing, and a name outside
//!    that catalog is refused with `InvalidFilter` naming the catalog;
//! 5. `MaxResults` bounds each page and `NextToken` resumes it, so a walk reports every path
//!    exactly once in path-id order; a page size outside the published range, a token this
//!    service did not issue and an oversized id or filter list are each refused with EC2's
//!    own error code;
//! 6. a `DryRun` reports `DryRunOperation` and reports no path — unless the request would
//!    have been refused anyway, which is the answer it gets instead.
//!
//! Each table row is a distinct input along what the rules generalise over — what a path
//! names (a resource pair, an address pair, a source alone), which protocol and whether a
//! destination port is set, whether a filter scopes it at the source, at the destination or
//! at neither, whether a port range carries both of its bounds or one, how many tags it
//! carries and how long their values are, which filter name and value form narrows the
//! report, which page size at each published bound and one past it — so no one recorded call
//! is the specification.

use ec2::ops::types::{FilterPortRange, PathFilter, Tag};
use ec2::state::{
    ComAmazonawsEc2Amazonec2State as Ec2State, NetworkInsightsPathSpec, DRY_RUN_OPERATION,
    INVALID_FILTER, INVALID_NETWORK_INSIGHTS_PATH_ID_NOT_FOUND, INVALID_PAGINATION_TOKEN,
    INVALID_PAGINATION_TOKEN_MESSAGE, INVALID_PARAMETER_VALUE, MAX_FILTERS, MAX_RESOURCE_IDS,
    NETWORK_INSIGHTS_PATH_ID_PREFIX, RESOURCE_TYPE_NETWORK_INSIGHTS_PATH,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{as_timestamp, call, element, error_code, error_message, filters, indexed};
use super::wire::{child, child_text, document, is_long_id, param, Param};

/// The Region every call here is signed for. A path's ARN names the Region it was created
/// for, so it is read back out of that ARN below.
const REGION: &str = "us-west-2";

/// The HTTP status EC2 answers `DryRunOperation` with: the request was refused because a
/// precondition the CALLER set holds, not because anything was wrong with it.
const DRY_RUN_STATUS: u16 = 412;

/// The filter names this operation supports, in the order the model enumerates them.
///
/// `DescribeNetworkInsightsPathsRequest$Filters` documents them itself: "destination - The ID
/// of the resource. filter-at-source.source-address - The source IPv4 address at the source.
/// filter-at-source.source-port-range - The source port range at the source.
/// filter-at-source.destination-address - The destination IPv4 address at the source.
/// filter-at-source.destination-port-range - The destination port range at the source.
/// filter-at-destination.source-address - The source IPv4 address at the destination.
/// filter-at-destination.source-port-range - The source port range at the destination.
/// filter-at-destination.destination-address - The destination IPv4 address at the
/// destination. filter-at-destination.destination-port-range - The destination port range at
/// the destination. protocol - The protocol. source - The ID of the resource":
///
/// ```text
/// smithy_query(query="doc",shape="DescribeNetworkInsightsPathsRequest",member="Filters",model_path="services/service-com-amazonaws-ec2-amazonec2/smithy-model/com-amazonaws-ec2-amazonec2.json")
/// ```
///
/// Every entry is exercised below, including the four `filter-at-destination.*` names, which
/// the issue this suite was filed under never mentioned.
const SUPPORTED_FILTERS: [&str; 11] = [
    "destination",
    "filter-at-source.source-address",
    "filter-at-source.source-port-range",
    "filter-at-source.destination-address",
    "filter-at-source.destination-port-range",
    "filter-at-destination.source-address",
    "filter-at-destination.source-port-range",
    "filter-at-destination.destination-address",
    "filter-at-destination.destination-port-range",
    "protocol",
    "source",
];

/// A well-formed path id the account holds no path under.
const UNKNOWN_PATH_ID: &str = "nip-0ffffffffffffffff";

/// One `DescribeNetworkInsightsPaths` call carrying `params`, as its HTTP status and XML body.
async fn describe(router: &MultiServiceRouter, params: &[Param]) -> (u16, String) {
    call(router, REGION, "DescribeNetworkInsightsPaths", params).await
}

/// One `DescribeNetworkInsightsPaths` call that has to succeed, and the body it answered.
async fn described(case: &str, router: &MultiServiceRouter, params: &[Param]) -> String {
    let (status, body) = describe(router, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with body {body}"
    );
    body
}

/// One `DeleteNetworkInsightsPath` call naming `path_id`: the writer this suite proves the
/// describe reports the STORE through, rather than a list of its own.
async fn delete_path(router: &MultiServiceRouter, path_id: &str) -> (u16, String) {
    call(
        router,
        REGION,
        "DeleteNetworkInsightsPath",
        &[param("NetworkInsightsPathId", path_id)],
    )
    .await
}

/// `Tag`s from `(key, value)` pairs.
fn tags(pairs: &[(&str, &str)]) -> Vec<Tag> {
    pairs
        .iter()
        .map(|(key, value)| Tag {
            key: Some((*key).to_string()),
            value: Some((*value).to_string()),
        })
        .collect()
}

/// One `FilterPortRange` carrying both of its bounds.
fn ports(from_port: i32, to_port: i32) -> FilterPortRange {
    FilterPortRange {
        from_port: Some(from_port),
        to_port: Some(to_port),
    }
}

/// The paths every table here is written against, as `(case, what a create stores for it)`.
///
/// They differ along everything the report, the narrowing and the filters could plausibly be
/// sensitive to: whether the path names resources, addresses or only a source, which protocol
/// and whether a destination port is set, whether a filter scopes it at the source, at the
/// destination, at both or at neither, whether a port range carries both bounds or one, and
/// how many tags it carries and how long their values are.
fn fixtures() -> Vec<(&'static str, NetworkInsightsPathSpec)> {
    vec![
        (
            "a resource-to-resource tcp path on port 443, unfiltered and untagged",
            NetworkInsightsPathSpec {
                source: Some("igw-0123456789abcdef0".to_string()),
                destination: Some("i-0123456789abcdef0".to_string()),
                protocol: Some("tcp".to_string()),
                destination_port: Some(443),
                region: REGION.to_string(),
                ..NetworkInsightsPathSpec::default()
            },
        ),
        (
            "an address-to-address udp path with no port and one tag",
            NetworkInsightsPathSpec {
                source: Some("eni-0123456789abcdef0".to_string()),
                destination: Some("eni-0fedcba9876543210".to_string()),
                source_ip: Some("10.0.1.4".to_string()),
                destination_ip: Some("10.0.2.7".to_string()),
                protocol: Some("udp".to_string()),
                region: REGION.to_string(),
                tags: tags(&[("Name", "east-west")]),
                ..NetworkInsightsPathSpec::default()
            },
        ),
        (
            "a tcp path filtered at both ends with three tags",
            NetworkInsightsPathSpec {
                source: Some("nlb-0123456789abcdef0".to_string()),
                destination: Some("i-0fedcba9876543210".to_string()),
                protocol: Some("tcp".to_string()),
                destination_port: Some(0),
                filter_at_source: Some(PathFilter {
                    source_address: Some("10.0.0.5".to_string()),
                    source_port_range: Some(ports(1024, 65535)),
                    destination_address: Some("192.0.2.10".to_string()),
                    destination_port_range: Some(ports(80, 80)),
                }),
                filter_at_destination: Some(PathFilter {
                    source_address: Some("198.51.100.7".to_string()),
                    source_port_range: Some(ports(0, 1023)),
                    destination_address: Some("203.0.113.9".to_string()),
                    destination_port_range: Some(ports(443, 443)),
                }),
                region: REGION.to_string(),
                tags: tags(&[("Name", "filtered"), ("Owner", "TeamB"), ("Tier", "prod")]),
                ..NetworkInsightsPathSpec::default()
            },
        ),
        (
            "a path with a source and a protocol only",
            NetworkInsightsPathSpec {
                source: Some("vpc-0123456789abcdef0".to_string()),
                protocol: Some("tcp".to_string()),
                region: REGION.to_string(),
                ..NetworkInsightsPathSpec::default()
            },
        ),
        (
            "a udp path scoped at the source by a one-bound port range, with a long tag value",
            NetworkInsightsPathSpec {
                source: Some("tgw-0123456789abcdef0".to_string()),
                destination: Some("i-00000000000000001".to_string()),
                protocol: Some("udp".to_string()),
                destination_port: Some(65535),
                filter_at_source: Some(PathFilter {
                    source_address: None,
                    source_port_range: Some(FilterPortRange {
                        from_port: Some(3389),
                        to_port: None,
                    }),
                    destination_address: None,
                    destination_port_range: None,
                }),
                region: REGION.to_string(),
                tags: vec![Tag {
                    key: Some("Description".to_string()),
                    value: Some("d".repeat(200)),
                }],
                ..NetworkInsightsPathSpec::default()
            },
        ),
    ]
}

/// Write the seeded paths into the engine's store, the way `CreateNetworkInsightsPath` would
/// — through the store's own one path-insertion path, which is where every member a path
/// reports is derived — and answer the ids they were given, in store order.
///
/// That create is still a scaffold stub of its own, so this suite seeds the way the
/// `DeleteNetworkInsightsPath` suite does rather than building a second copy of the modelled
/// shape by hand.
fn seed(engine: &engine::InMemoryEngine) -> Vec<String> {
    let store = engine.state::<Ec2State>();
    let mut guard = store.lock();
    fixtures()
        .into_iter()
        .map(|(case, spec)| {
            guard
                .insert_network_insights_path(spec)
                .unwrap_or_else(|| panic!("case {case:?}: the store holds room for the fixture"))
        })
        .collect()
}

/// One path as a response reported it: every member of `NetworkInsightsPath`, read out of the
/// PARSED document so a member of the path itself is never confused with one of the same name
/// nested in a filter at either end.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
struct ReportedPath {
    path_id: String,
    path_arn: String,
    created_date: String,
    source: Option<String>,
    destination: Option<String>,
    source_ip: Option<String>,
    destination_ip: Option<String>,
    protocol: Option<String>,
    destination_port: Option<String>,
    filter_at_source: Option<ReportedFilter>,
    filter_at_destination: Option<ReportedFilter>,
    tags: Vec<(String, String)>,
}

/// One `PathFilter` as a response reported it, with each port range as `(from, to)`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
struct ReportedFilter {
    source_address: Option<String>,
    destination_address: Option<String>,
    source_port_range: Option<(Option<String>, Option<String>)>,
    destination_port_range: Option<(Option<String>, Option<String>)>,
}

/// One reported `PathFilter`, or `None` when the path carries no filter at that end.
fn reported_filter(path: roxmltree::Node<'_, '_>, name: &str) -> Option<ReportedFilter> {
    let end = child(path, name)?;
    let range = |member: &str| {
        child(end, member)
            .map(|range| (child_text(range, "fromPort"), child_text(range, "toPort")))
    };
    Some(ReportedFilter {
        source_address: child_text(end, "sourceAddress"),
        destination_address: child_text(end, "destinationAddress"),
        source_port_range: range("sourcePortRange"),
        destination_port_range: range("destinationPortRange"),
    })
}

/// Every path a response reported, in the order it reported them: each element carrying a
/// `networkInsightsPathId` child is one path.
fn reported(case: &str, body: &str) -> Vec<ReportedPath> {
    let doc = document(case, body);
    doc.descendants()
        .filter(|node| node.is_element() && child(*node, "networkInsightsPathId").is_some())
        .map(|node| ReportedPath {
            path_id: child_text(node, "networkInsightsPathId").unwrap_or_default(),
            path_arn: child_text(node, "networkInsightsPathArn").unwrap_or_default(),
            created_date: child_text(node, "createdDate").unwrap_or_default(),
            source: child_text(node, "source"),
            destination: child_text(node, "destination"),
            source_ip: child_text(node, "sourceIp"),
            destination_ip: child_text(node, "destinationIp"),
            protocol: child_text(node, "protocol"),
            destination_port: child_text(node, "destinationPort"),
            filter_at_source: reported_filter(node, "filterAtSource"),
            filter_at_destination: reported_filter(node, "filterAtDestination"),
            tags: child(node, "tagSet")
                .map(|set| {
                    set.children()
                        .filter(|item| item.is_element() && item.tag_name().name() == "item")
                        .map(|item| {
                            (
                                child_text(item, "key").unwrap_or_default(),
                                child_text(item, "value").unwrap_or_default(),
                            )
                        })
                        .collect()
                })
                .unwrap_or_default(),
        })
        .collect()
}

/// Every path id a response reported, in the order it reported them.
fn reported_ids(case: &str, body: &str) -> Vec<String> {
    reported(case, body)
        .into_iter()
        .map(|path| path.path_id)
        .collect()
}

/// The ids of the paths the STORE holds, in store order.
fn stored_ids(engine: &engine::InMemoryEngine) -> Vec<String> {
    engine
        .state::<Ec2State>()
        .lock()
        .network_insights_paths
        .keys()
        .cloned()
        .collect()
}

// ---------------------------------------------------------------------------
// RULE 1: the store is what is reported
// ---------------------------------------------------------------------------

/// RULE 1: a request naming nothing reports every path the account holds, in path-id order,
/// and each path carries exactly what the store holds for it: the id it is keyed by, a
/// long-form ARN naming the Region and the path, a creation date, the source and destination,
/// the addresses, the protocol, the destination port, the filters at either end and the tags.
/// One row per seeded path.
#[tokio::test]
async fn the_describe_reports_every_member_the_store_holds_for_each_path() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);
    let expected_order = stored_ids(&engine);

    let body = described("every path", &router, &[]).await;
    let found = reported("every path", &body);
    assert_eq!(
        found
            .iter()
            .map(|path| path.path_id.clone())
            .collect::<Vec<_>>(),
        expected_order,
        "every path: expected the store's paths in path-id order, got {found:?}"
    );

    for (position, (case, spec)) in fixtures().into_iter().enumerate() {
        let path_id = &seeded[position];
        let path = found
            .iter()
            .find(|path| path.path_id == *path_id)
            .unwrap_or_else(|| panic!("case {case:?}: {path_id} must be reported, got {found:?}"));

        assert!(
            is_long_id(NETWORK_INSIGHTS_PATH_ID_PREFIX, &path.path_id),
            "case {case:?}: expected a long {NETWORK_INSIGHTS_PATH_ID_PREFIX}- id, got {:?}",
            path.path_id
        );
        let expected_region = format!(":{REGION}:");
        assert!(
            path.path_arn.starts_with("arn:")
                && path.path_arn.contains(&expected_region)
                && path
                    .path_arn
                    .ends_with(&format!("{RESOURCE_TYPE_NETWORK_INSIGHTS_PATH}/{path_id}")),
            "case {case:?}: expected an ARN naming {REGION} and \
             {RESOURCE_TYPE_NETWORK_INSIGHTS_PATH}/{path_id}, got {:?}",
            path.path_arn
        );
        assert!(
            as_timestamp(&path.created_date).is_some(),
            "case {case:?}: expected an ISO 8601 createdDate, got {:?}",
            path.created_date
        );

        assert_eq!(
            path.source, spec.source,
            "case {case:?}: expected the stored source {:?}, got {:?}",
            spec.source, path.source
        );
        assert_eq!(
            path.destination, spec.destination,
            "case {case:?}: expected the stored destination {:?}, got {:?}",
            spec.destination, path.destination
        );
        assert_eq!(
            path.source_ip, spec.source_ip,
            "case {case:?}: expected the stored sourceIp {:?}, got {:?}",
            spec.source_ip, path.source_ip
        );
        assert_eq!(
            path.destination_ip, spec.destination_ip,
            "case {case:?}: expected the stored destinationIp {:?}, got {:?}",
            spec.destination_ip, path.destination_ip
        );
        assert_eq!(
            path.protocol, spec.protocol,
            "case {case:?}: expected the stored protocol {:?}, got {:?}",
            spec.protocol, path.protocol
        );
        assert_eq!(
            path.destination_port,
            spec.destination_port.map(|port| port.to_string()),
            "case {case:?}: expected the stored destinationPort {:?}, got {:?}",
            spec.destination_port,
            path.destination_port
        );
        assert_eq!(
            path.filter_at_source,
            spec.filter_at_source.as_ref().map(expected_filter),
            "case {case:?}: expected the stored filterAtSource, got {:?}",
            path.filter_at_source
        );
        assert_eq!(
            path.filter_at_destination,
            spec.filter_at_destination.as_ref().map(expected_filter),
            "case {case:?}: expected the stored filterAtDestination, got {:?}",
            path.filter_at_destination
        );
        let expected_tags: Vec<(String, String)> = spec
            .tags
            .iter()
            .map(|tag| {
                (
                    tag.key.clone().unwrap_or_default(),
                    tag.value.clone().unwrap_or_default(),
                )
            })
            .collect();
        assert_eq!(
            path.tags, expected_tags,
            "case {case:?}: expected the stored tags {expected_tags:?}, got {:?}",
            path.tags
        );
    }
}

/// The stored `PathFilter` as the wire reports it: each member as its text, each port range
/// as the decimal renderings of the bounds it carries.
fn expected_filter(filter: &PathFilter) -> ReportedFilter {
    let range = |range: &Option<FilterPortRange>| {
        range.as_ref().map(|range| {
            (
                range.from_port.map(|port| port.to_string()),
                range.to_port.map(|port| port.to_string()),
            )
        })
    };
    ReportedFilter {
        source_address: filter.source_address.clone(),
        destination_address: filter.destination_address.clone(),
        source_port_range: range(&filter.source_port_range),
        destination_port_range: range(&filter.destination_port_range),
    }
}

/// RULE 1: the report follows the STORE rather than a list of its own — a path a delete
/// removes stops being reported, and every other path is reported exactly as before. One row
/// per seeded path, deleted from a store seeded afresh each time.
#[tokio::test]
async fn a_deleted_path_is_no_longer_reported() {
    for (position, (case, _)) in fixtures().into_iter().enumerate() {
        let (router, engine) = super::ec2_query::emulator();
        let seeded = seed(&engine);
        let path_id = seeded[position].clone();

        let before = reported(case, &described(case, &router, &[]).await);
        assert!(
            before.iter().any(|path| path.path_id == path_id),
            "case {case:?}: {path_id} must be reported before the delete, got {before:?}"
        );

        let (status, body) = delete_path(&router, &path_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the delete expected HTTP 200, got {status} with body {body}"
        );

        let after = reported(case, &described(case, &router, &[]).await);
        let expected: Vec<ReportedPath> = before
            .into_iter()
            .filter(|path| path.path_id != path_id)
            .collect();
        assert_eq!(
            after, expected,
            "case {case:?}: expected exactly the paths that were not deleted, got {after:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// RULE 2: describing is a read
// ---------------------------------------------------------------------------

/// RULE 2: the describe is a pure read — calling it, with and without every narrowing member
/// the model publishes, leaves the store holding the same paths with the same members, so
/// what a later caller sees is what it would have seen anyway.
#[tokio::test]
async fn describing_changes_nothing_a_later_caller_sees() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);
    let first = reported("first read", &described("first read", &router, &[]).await);

    let calls: Vec<(&str, Vec<Param>)> = vec![
        ("no narrowing", Vec::new()),
        (
            "one id",
            indexed("NetworkInsightsPathId", &[seeded[0].as_str()]),
        ),
        ("a protocol filter", filters(&[("protocol", &["tcp"])])),
        ("a page of one", vec![param("MaxResults", "1")]),
    ];
    for (case, params) in calls {
        for _ in 0..3 {
            let body = described(case, &router, &params).await;
            assert!(
                !body.is_empty(),
                "case {case:?}: expected a body from the describe"
            );
        }
        let again = reported(case, &described(case, &router, &[]).await);
        assert_eq!(
            again, first,
            "case {case:?}: repeated reads must leave every path as it was, got {again:?}"
        );
        assert_eq!(
            stored_ids(&engine),
            seeded,
            "case {case:?}: repeated reads must leave the store's paths as they were"
        );
    }
}

// ---------------------------------------------------------------------------
// RULE 3: NetworkInsightsPathIds
// ---------------------------------------------------------------------------

/// RULE 3: `NetworkInsightsPathIds` narrows the report to exactly the paths named, in path-id
/// order whatever order they were asked in, and a repeated id is not a repeated row. The rows
/// are each single id, several subsets, a reversed order, a duplicated id and the whole set.
#[tokio::test]
async fn the_id_list_narrows_the_report_to_the_paths_it_names() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);
    let subsets: Vec<Vec<usize>> = vec![
        vec![0],
        vec![1],
        vec![2],
        vec![3],
        vec![4],
        vec![0, 2],
        vec![4, 3, 1],
        vec![2, 2],
        vec![0, 1, 2, 3, 4],
    ];

    for subset in subsets {
        let case = format!("ids {subset:?}");
        let asked: Vec<&str> = subset.iter().map(|at| seeded[*at].as_str()).collect();
        let body = described(&case, &router, &indexed("NetworkInsightsPathId", &asked)).await;

        let mut expected: Vec<String> = subset.iter().map(|at| seeded[*at].clone()).collect();
        expected.sort();
        expected.dedup();
        let found = reported_ids(&case, &body);
        assert_eq!(
            found, expected,
            "case {case:?}: expected exactly {expected:?} in path-id order, got {found:?}"
        );
    }
    assert_eq!(
        stored_ids(&engine),
        seeded,
        "narrowing must leave the store alone"
    );
}

/// RULE 3: an id the account holds no path under is refused with
/// `InvalidNetworkInsightsPathId.NotFound` naming that id, and nothing is reported — whatever
/// the id's form, and whether or not it is asked for beside an id that does resolve. The rows
/// are each way a value can fail to name a path of the account.
#[tokio::test]
async fn an_id_that_names_no_path_of_the_account_is_refused() {
    let prefix = NETWORK_INSIGHTS_PATH_ID_PREFIX;
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);
    let held = seeded[0].clone();

    let cases: Vec<(&str, Vec<String>, String)> = vec![
        (
            "the long form",
            vec![format!("{prefix}-00000000000000000")],
            format!("{prefix}-00000000000000000"),
        ),
        (
            "the short form",
            vec![format!("{prefix}-00000000")],
            format!("{prefix}-00000000"),
        ),
        (
            "an upper-case suffix",
            vec![format!("{prefix}-0ABCDEF01")],
            format!("{prefix}-0ABCDEF01"),
        ),
        (
            "another resource's id",
            vec!["vpc-00000000000000000".to_string()],
            "vpc-00000000000000000".to_string(),
        ),
        (
            "free text",
            vec!["not-a-network-insights-path".to_string()],
            "not-a-network-insights-path".to_string(),
        ),
        (
            "a held id beside an unknown one",
            vec![held.clone(), UNKNOWN_PATH_ID.to_string()],
            UNKNOWN_PATH_ID.to_string(),
        ),
    ];

    for (case, asked, named) in cases {
        let ids: Vec<&str> = asked.iter().map(String::as_str).collect();
        let (status, body) = describe(&router, &indexed("NetworkInsightsPathId", &ids)).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            INVALID_NETWORK_INSIGHTS_PATH_ID_NOT_FOUND,
            "case {case:?}: expected {INVALID_NETWORK_INSIGHTS_PATH_ID_NOT_FOUND}, got {body}"
        );
        assert_eq!(
            error_message(&body),
            format!("The network insights path ID '{named}' does not exist"),
            "case {case:?}: expected the refusal to name {named}, got {body}"
        );
        assert!(
            reported(case, &body).is_empty(),
            "case {case:?}: a refused describe must report no path, got {body}"
        );
    }
    assert_eq!(
        stored_ids(&engine),
        seeded,
        "a refused describe must leave the store alone"
    );
}

// ---------------------------------------------------------------------------
// RULE 4: the documented filters
// ---------------------------------------------------------------------------

/// RULE 4: each documented filter name narrows on the member of the path it names. The rows
/// cover every name in the catalog, each on a value form of its own — an exact id, an exact
/// address, a both-bound port range, a one-bound port range, a protocol, and `*`/`?`
/// wildcards — plus a value no seeded path carries, which selects nothing rather than
/// everything.
#[tokio::test]
async fn each_documented_filter_narrows_on_the_member_it_names() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    // `(case, filter name, values, the seeded positions that must come back)`.
    let cases: Vec<(&str, &str, Vec<&str>, Vec<usize>)> = vec![
        (
            "destination, exact",
            "destination",
            vec!["i-0123456789abcdef0"],
            vec![0],
        ),
        (
            "destination, a prefix wildcard",
            "destination",
            vec!["eni-*"],
            vec![1],
        ),
        (
            "destination, a value no path carries",
            "destination",
            vec!["i-ffffffffffffffff0"],
            vec![],
        ),
        ("source, exact", "source", vec!["tgw-0123456789abcdef0"], vec![4]),
        (
            "source, two values joined with OR",
            "source",
            vec!["igw-0123456789abcdef0", "vpc-0123456789abcdef0"],
            vec![0, 3],
        ),
        ("protocol, tcp", "protocol", vec!["tcp"], vec![0, 2, 3]),
        ("protocol, udp", "protocol", vec!["udp"], vec![1, 4]),
        (
            "protocol, a single-character wildcard",
            "protocol",
            vec!["?cp"],
            vec![0, 2, 3],
        ),
        (
            "filter-at-source.source-address, exact",
            "filter-at-source.source-address",
            vec!["10.0.0.5"],
            vec![2],
        ),
        (
            "filter-at-source.destination-address, exact",
            "filter-at-source.destination-address",
            vec!["192.0.2.10"],
            vec![2],
        ),
        (
            "filter-at-source.source-port-range, both bounds",
            "filter-at-source.source-port-range",
            vec!["1024-65535"],
            vec![2],
        ),
        (
            "filter-at-source.source-port-range, one bound",
            "filter-at-source.source-port-range",
            vec!["3389"],
            vec![4],
        ),
        (
            "filter-at-source.source-port-range, a wildcard over both ranges",
            "filter-at-source.source-port-range",
            vec!["*"],
            vec![2, 4],
        ),
        (
            "filter-at-source.destination-port-range, both bounds",
            "filter-at-source.destination-port-range",
            vec!["80-80"],
            vec![2],
        ),
        (
            "filter-at-destination.source-address, exact",
            "filter-at-destination.source-address",
            vec!["198.51.100.7"],
            vec![2],
        ),
        (
            "filter-at-destination.destination-address, a wildcard",
            "filter-at-destination.destination-address",
            vec!["203.0.113.*"],
            vec![2],
        ),
        (
            "filter-at-destination.source-port-range, both bounds",
            "filter-at-destination.source-port-range",
            vec!["0-1023"],
            vec![2],
        ),
        (
            "filter-at-destination.destination-port-range, both bounds",
            "filter-at-destination.destination-port-range",
            vec!["443-443"],
            vec![2],
        ),
        (
            "filter-at-destination.source-port-range, a path that is not filtered there",
            "filter-at-destination.source-port-range",
            vec!["3389"],
            vec![],
        ),
    ];

    for (case, name, values, expected) in cases {
        let body = described(case, &router, &filters(&[(name, &values)])).await;
        let found = reported_ids(case, &body);
        let expected: Vec<String> = expected.iter().map(|at| seeded[*at].clone()).collect();
        assert_eq!(
            found, expected,
            "case {case:?}: expected {expected:?} for {name}={values:?}, got {found:?}"
        );
    }
    assert_eq!(
        stored_ids(&engine),
        seeded,
        "filtering must leave the store alone"
    );
}

/// One row of the AND table: the case, the `Filters` entries it sends, and the seeded
/// positions that must come back for them.
type AndRow = (
    &'static str,
    Vec<(&'static str, &'static [&'static str])>,
    Vec<usize>,
);

/// RULE 4: several filters are joined with AND, and they compose with `NetworkInsightsPathIds`
/// — so a path has to satisfy every one of them to be reported.
#[tokio::test]
async fn several_filters_are_joined_with_and() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    let cases: Vec<AndRow> = vec![
        (
            "tcp and a destination both hold",
            vec![
                ("protocol", &["tcp"][..]),
                ("destination", &["i-0fedcba9876543210"][..]),
            ],
            vec![2],
        ),
        (
            "tcp and a destination that only a udp path carries",
            vec![
                ("protocol", &["tcp"][..]),
                ("destination", &["eni-0fedcba9876543210"][..]),
            ],
            vec![],
        ),
        (
            "a protocol and a source port range",
            vec![
                ("protocol", &["udp"][..]),
                ("filter-at-source.source-port-range", &["3389"][..]),
            ],
            vec![4],
        ),
        (
            "three filters over one path",
            vec![
                ("protocol", &["tcp"][..]),
                ("filter-at-source.source-address", &["10.0.0.5"][..]),
                ("filter-at-destination.destination-port-range", &["443-443"][..]),
            ],
            vec![2],
        ),
    ];

    for (case, entries, expected) in cases {
        let body = described(case, &router, &filters(&entries)).await;
        let found = reported_ids(case, &body);
        let expected_ids: Vec<String> = expected.iter().map(|at| seeded[*at].clone()).collect();
        assert_eq!(
            found, expected_ids,
            "case {case:?}: expected {expected_ids:?}, got {found:?}"
        );

        // The same filters, narrowed further by an id list naming one path that satisfies
        // them and one that does not: the AND holds across the two members too.
        let asked: Vec<&str> = vec![seeded[2].as_str(), seeded[4].as_str()];
        let mut params = filters(&entries);
        params.extend(indexed("NetworkInsightsPathId", &asked));
        let body = described(case, &router, &params).await;
        let found = reported_ids(case, &body);
        let expected_ids: Vec<String> = expected
            .iter()
            .filter(|at| **at == 2 || **at == 4)
            .map(|at| seeded[*at].clone())
            .collect();
        assert_eq!(
            found, expected_ids,
            "case {case:?}: with an id list expected {expected_ids:?}, got {found:?}"
        );
    }
}

/// RULE 4: every name the model documents is ACCEPTED, and a name outside that catalog is
/// refused with `InvalidFilter` naming the whole supported catalog — so an unsupported filter
/// can never read as "no results". The rows are each documented name, then a name near one of
/// them, a name of another describe's catalog, the `tag:` family this operation does not
/// document, and an empty name.
#[tokio::test]
async fn the_filter_catalog_is_what_the_model_documents() {
    let (router, engine) = super::ec2_query::emulator();
    seed(&engine);

    for name in SUPPORTED_FILTERS {
        let case = format!("the documented filter {name}");
        let (status, body) = describe(&router, &filters(&[(name, &["*"])])).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a documented filter must be accepted, got {status} with body {body}"
        );
    }

    let expected_message = format!("Filter name must be one of [{}]", SUPPORTED_FILTERS.join(", "));
    let refused = [
        "filter-at-source.source-port",
        "filter-at-middle.source-address",
        "status",
        "tag:Name",
        "tag-key",
        "network-insights-path-id",
        "",
    ];
    for name in refused {
        let case = format!("the undocumented filter {name:?}");
        let (status, body) = describe(&router, &filters(&[(name, &["anything"])])).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            INVALID_FILTER,
            "case {case:?}: expected {INVALID_FILTER}, got {body}"
        );
        assert_eq!(
            error_message(&body),
            expected_message,
            "case {case:?}: expected the message naming the supported catalog, got {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// RULE 5: paging and the request-size caps
// ---------------------------------------------------------------------------

/// RULE 5: `MaxResults` bounds each page and `NextToken` resumes from where the last page
/// ended, so walking the pages reports every path exactly once in path-id order — at every
/// page size, and with a filter narrowing the walk too.
#[tokio::test]
async fn max_results_pages_the_report_and_the_token_resumes_it() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);
    let mut expected = seeded.clone();
    expected.sort();

    for page_size in [1usize, 2, 5, 100] {
        let case = format!("MaxResults={page_size}");
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let mut params = vec![param("MaxResults", &page_size.to_string())];
            if let Some(next) = token.as_deref() {
                params.push(param("NextToken", next));
            }
            let body = described(&case, &router, &params).await;
            let page = reported_ids(&case, &body);
            assert!(
                page.len() <= page_size,
                "case {case:?}: a page holds at most MaxResults paths, got {page:?}"
            );
            walked.extend(page);
            token = element(&body, "nextToken");
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            walked, expected,
            "case {case:?}: the walk must report every path exactly once in id order, got \
             {walked:?}"
        );
    }

    // The same walk, narrowed by a filter: paging and filtering compose, and the last page of
    // a narrowed walk carries no token.
    let case = "a filtered walk of one path per page";
    let mut walked: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let mut params = filters(&[("protocol", &["udp"])]);
        params.push(param("MaxResults", "1"));
        if let Some(next) = token.as_deref() {
            params.push(param("NextToken", next));
        }
        let body = described(case, &router, &params).await;
        walked.extend(reported_ids(case, &body));
        token = element(&body, "nextToken");
        if token.is_none() {
            break;
        }
    }
    let mut expected_udp = vec![seeded[1].clone(), seeded[4].clone()];
    expected_udp.sort();
    assert_eq!(
        walked, expected_udp,
        "case {case:?}: expected the udp paths {expected_udp:?}, got {walked:?}"
    );

    let body = described("no MaxResults", &router, &[]).await;
    assert_eq!(
        reported_ids("no MaxResults", &body),
        expected,
        "no MaxResults: expected every path in one answer"
    );
    assert_eq!(
        element(&body, "nextToken"),
        None,
        "no MaxResults: a complete answer carries no token, got {body}"
    );
}

/// RULE 5: `MaxResults` outside the range the model publishes is refused; each published
/// bound itself is accepted. The rows are both bounds and one past each.
#[tokio::test]
async fn max_results_is_accepted_at_its_bounds_and_refused_past_them() {
    let (router, engine) = super::ec2_query::emulator();
    seed(&engine);

    // The published bounds (`NetworkInsightsMaxResults`, `range 1..100`) and one past each.
    for (max_results, expected_status) in [(0, 400), (1, 200), (100, 200), (101, 400)] {
        let case = format!("MaxResults={max_results}");
        let (status, body) =
            describe(&router, &[param("MaxResults", &max_results.to_string())]).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} with body {body}"
        );
    }
}

/// RULE 5: a `NextToken` this service did not issue is refused with `InvalidPaginationToken`
/// and EC2's own sentence for it, rather than read as a cursor. The rows are free text, a
/// path id, an almost-token and an empty value.
#[tokio::test]
async fn a_token_this_service_did_not_issue_is_refused() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    for token in ["not-a-token", seeded[0].as_str(), "!!!!", ""] {
        let case = format!("NextToken={token:?}");
        let (status, body) = describe(&router, &[param("NextToken", token)]).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            INVALID_PAGINATION_TOKEN,
            "case {case:?}: expected {INVALID_PAGINATION_TOKEN}, got {body}"
        );
        assert_eq!(
            error_message(&body),
            INVALID_PAGINATION_TOKEN_MESSAGE,
            "case {case:?}: expected EC2's own pagination-token sentence, got {body}"
        );
    }
}

/// RULE 5: the request-size caps are enforced before the scan — an id list or a filter list
/// at the cap is an ordinary request, one past it is refused for its size alone.
#[tokio::test]
async fn an_oversized_id_or_filter_list_is_refused() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);
    let held = seeded[0].clone();

    for (count, expected_status) in [(MAX_RESOURCE_IDS, 200), (MAX_RESOURCE_IDS + 1, 400)] {
        let case = format!("{count} ids");
        let ids: Vec<&str> = (0..count).map(|_| held.as_str()).collect();
        let (status, body) = describe(&router, &indexed("NetworkInsightsPathId", &ids)).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} with body {body}"
        );
        if expected_status == 400 {
            assert_eq!(
                error_code(&body),
                INVALID_PARAMETER_VALUE,
                "case {case:?}: expected {INVALID_PARAMETER_VALUE}, got {body}"
            );
        }
    }

    for (count, expected_status) in [(MAX_FILTERS, 200), (MAX_FILTERS + 1, 400)] {
        let case = format!("{count} filters");
        let entries: Vec<(&str, &[&str])> = (0..count).map(|_| ("protocol", &["*"][..])).collect();
        let (status, body) = describe(&router, &filters(&entries)).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} with body {body}"
        );
        if expected_status == 400 {
            assert_eq!(
                error_code(&body),
                INVALID_PARAMETER_VALUE,
                "case {case:?}: expected {INVALID_PARAMETER_VALUE}, got {body}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// RULE 6: DryRun
// ---------------------------------------------------------------------------

/// RULE 6: `DryRun` reports the permitted outcome — `DryRunOperation`, HTTP 412 — and reports
/// no path at all, while `DryRun=false` is an ordinary describe. The rows are a dry run of
/// each narrowing the operation publishes.
#[tokio::test]
async fn a_dry_run_reports_the_permitted_outcome_and_no_path() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    let calls: Vec<(&str, Vec<Param>)> = vec![
        ("a bare dry run", Vec::new()),
        (
            "a dry run naming one id",
            indexed("NetworkInsightsPathId", &[seeded[2].as_str()]),
        ),
        (
            "a dry run with a filter",
            filters(&[("protocol", &["tcp"])]),
        ),
        ("a dry run with a page size", vec![param("MaxResults", "2")]),
    ];

    for (case, params) in calls {
        let mut dry = params.clone();
        dry.push(param("DryRun", "true"));
        let (status, body) = describe(&router, &dry).await;
        assert_eq!(
            status, DRY_RUN_STATUS,
            "case {case:?}: expected HTTP {DRY_RUN_STATUS}, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            DRY_RUN_OPERATION,
            "case {case:?}: expected {DRY_RUN_OPERATION}, got {body}"
        );
        assert!(
            reported(case, &body).is_empty(),
            "case {case:?}: a dry run must report no path, got {body}"
        );

        // `DryRun=false` is no dry run at all: the describe answers.
        let mut wet = params.clone();
        wet.push(param("DryRun", "false"));
        let body = described(case, &router, &wet).await;
        assert!(
            !reported_ids(case, &body).is_empty(),
            "case {case:?}: DryRun=false must report the paths, got {body}"
        );
    }
    assert_eq!(
        stored_ids(&engine),
        seeded,
        "a dry run must leave the store alone"
    );
}

/// RULE 6: a dry run asks whether the request WOULD have worked, so one that would have been
/// refused is refused with THAT reason rather than reported as permitted. The rows are an id
/// naming no path of the account, an undocumented filter name, a page size past the published
/// bound and a token this service did not issue.
#[tokio::test]
async fn a_dry_run_that_would_have_been_refused_gets_that_reason() {
    let (router, engine) = super::ec2_query::emulator();
    seed(&engine);

    let cases: Vec<(&str, Vec<Param>, &str)> = vec![
        (
            "an unknown path id",
            indexed("NetworkInsightsPathId", &[UNKNOWN_PATH_ID]),
            INVALID_NETWORK_INSIGHTS_PATH_ID_NOT_FOUND,
        ),
        (
            "an undocumented filter",
            filters(&[("tag:Name", &["east-west"])]),
            INVALID_FILTER,
        ),
        (
            "a page size past the published bound",
            vec![param("MaxResults", "101")],
            // `NetworkInsightsMaxResults` is a modelled `range 1..100`, so the BOUNDARY
            // validator refuses this before the handler runs — with its own
            // `ValidationError`, not with a code the handler chose.
            "ValidationError",
        ),
        (
            "a token this service did not issue",
            vec![param("NextToken", "not-a-token")],
            INVALID_PAGINATION_TOKEN,
        ),
    ];

    for (case, params, expected_code) in cases {
        let mut dry = params.clone();
        dry.push(param("DryRun", "true"));
        let (status, body) = describe(&router, &dry).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {case:?}: expected {expected_code} rather than a permitted dry run, got {body}"
        );
    }
}

/// RULE 1 and RULE 5 on an account that holds no path at all: the report is an empty set
/// rather than a refusal, and it carries no token.
#[tokio::test]
async fn an_account_with_no_path_is_reported_as_an_empty_set() {
    let (router, _engine) = super::ec2_query::emulator();

    for (case, params) in [
        ("no narrowing", Vec::new()),
        ("a filter", filters(&[("protocol", &["tcp"])])),
        ("a page size", vec![param("MaxResults", "1")]),
    ] {
        let body = described(case, &router, &params).await;
        assert!(
            reported_ids(case, &body).is_empty(),
            "case {case:?}: expected no path, got {body}"
        );
        assert_eq!(
            element(&body, "nextToken"),
            None,
            "case {case:?}: an empty report carries no token, got {body}"
        );
    }
}
