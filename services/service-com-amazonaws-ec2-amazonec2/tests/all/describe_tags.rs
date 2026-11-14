// SPDX-License-Identifier: CC0-1.0

//! What `DescribeTags` reports, over the wire the service speaks (`ec2Query`, through the
//! shared [`super::ec2_query`] harness).
//!
//! The rules under test are:
//!
//! 1. the report carries ONE entry per tag the account's resources hold — the resource's id,
//!    that resource's `ResourceType`, and the tag's key and value — so a tag written through
//!    `CreateTags` on a resource of ANY kind this service stores tags for is reported here,
//!    with the empty value a valueless tag is stored with, and a tag a later call removes or
//!    overwrites is reported as the store holds it then;
//! 2. the documented filters narrow that report: every filter has to hold, a filter's values
//!    are joined with an OR, EC2's `*` and `?` wildcards apply, comparisons are
//!    case-sensitive, and a filter naming an attribute an entry does not carry selects
//!    nothing;
//! 3. the report is paged by `MaxResults` behind an opaque `NextToken` that walks every
//!    selected tag exactly once, and a request outside the published page range, carrying a
//!    token this service did not mint, naming an undocumented filter or asking for a dry run
//!    is refused with EC2's own code — `InvalidParameterValue` for a filter name this read
//!    cannot resolve, naming the entry at fault.
//!
//! The rows of each table are distinct inputs along what each rule generalises over: which
//! kind of resource holds the tag, which form its value takes, which filter narrows the
//! report and which member makes the request refusable. No row is the specification.

use std::collections::BTreeSet;

use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, element, error_code, error_message, filters};

/// The Region every call here is signed for.
const REGION: &str = "us-west-2";

/// The image `RunInstances` launches from, in the long id form this service accepts.
const AMI: &str = "ami-0123456789abcdef0";

/// A `(key, value)` request parameter.
fn p(key: &str, value: impl Into<String>) -> (String, String) {
    (key.to_string(), value.into())
}

/// One request's form parameters, as the ec2Query harness takes them.
type Params = Vec<(String, String)>;

fn emulator() -> MultiServiceRouter {
    super::ec2_query::emulator().0
}

/// One entry of the report, as a comparable row: the resource, its type, the key and the
/// value.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct Described {
    resource_id: String,
    resource_type: String,
    key: String,
    value: String,
}

/// Every resource kind this suite tags, with the `ResourceType` the report has to name it by
/// (`smithy_query(query="shape",shape="ResourceType")`). They differ in how the store holds a
/// resource's tags — an instance keeps a tag MAP, a network interface the modelled `TagSet`,
/// every other kind the modelled `Tags` list — and in the id form the request carries.
const KINDS: [(&str, &str); 6] = [
    ("instance", "instance"),
    ("network-interface", "network-interface"),
    ("vpc", "vpc"),
    ("subnet", "subnet"),
    ("security-group", "security-group"),
    ("elastic-ip", "elastic-ip"),
];

/// The tags each row of the kind table writes, chosen so the report has to carry a plain
/// value, a value with a space in it, an EMPTY value, and two keys that differ only in case.
const STORED_TAGS: [(&str, &str); 5] = [
    ("env", "prod"),
    ("Name", "demo site"),
    ("blank", ""),
    ("owner", "team-a"),
    ("OWNER", "team-b"),
];

/// Call `action`, require HTTP 200, and answer the `id_element` of the response.
async fn created_id(
    router: &MultiServiceRouter,
    case: &str,
    action: &str,
    params: &[(String, String)],
    id_element: &str,
) -> String {
    let (status, body) = call(router, REGION, action, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: {action} must succeed, got {status}: {body}"
    );
    element(&body, id_element)
        .unwrap_or_else(|| panic!("case {case:?}: expected a <{id_element}> in {body}"))
}

/// One VPC, as the owner of the kinds that need one.
async fn create_vpc(router: &MultiServiceRouter, case: &str, cidr: &str) -> String {
    created_id(router, case, "CreateVpc", &[p("CidrBlock", cidr)], "vpcId").await
}

/// One subnet of a VPC this call creates.
async fn create_subnet(router: &MultiServiceRouter, case: &str) -> String {
    let vpc_id = create_vpc(router, case, "10.3.0.0/16").await;
    created_id(
        router,
        case,
        "CreateSubnet",
        &[p("VpcId", &vpc_id), p("CidrBlock", "10.3.1.0/24")],
        "subnetId",
    )
    .await
}

/// Create one resource of `kind` and answer its resource id.
async fn create_resource(router: &MultiServiceRouter, case: &str, kind: &str) -> String {
    match kind {
        "instance" => {
            created_id(
                router,
                case,
                "RunInstances",
                &[p("ImageId", AMI), p("MinCount", "1"), p("MaxCount", "1")],
                "instanceId",
            )
            .await
        }
        "network-interface" => {
            let subnet_id = create_subnet(router, case).await;
            created_id(
                router,
                case,
                "CreateNetworkInterface",
                &[p("SubnetId", &subnet_id)],
                "networkInterfaceId",
            )
            .await
        }
        "vpc" => create_vpc(router, case, "10.0.0.0/16").await,
        "subnet" => create_subnet(router, case).await,
        "security-group" => {
            let vpc_id = create_vpc(router, case, "10.1.0.0/16").await;
            created_id(
                router,
                case,
                "CreateSecurityGroup",
                &[
                    p("GroupName", "tagged-group"),
                    p("GroupDescription", "a group this suite tags"),
                    p("VpcId", &vpc_id),
                ],
                "groupId",
            )
            .await
        }
        "elastic-ip" => {
            created_id(
                router,
                case,
                "AllocateAddress",
                &[p("Domain", "vpc")],
                "allocationId",
            )
            .await
        }
        other => panic!("case {case:?}: this suite knows no resource kind {other:?}"),
    }
}

/// Write `tags` onto `resource_id` through `CreateTags`, requiring the write to succeed.
async fn tag_resource(
    router: &MultiServiceRouter,
    case: &str,
    resource_id: &str,
    tags: &[(&str, &str)],
) {
    let mut params = vec![p("ResourceId.1", resource_id)];
    for (index, (key, value)) in tags.iter().enumerate() {
        params.push(p(&format!("Tag.{}.Key", index + 1), *key));
        params.push(p(&format!("Tag.{}.Value", index + 1), *value));
    }
    let (status, body) = call(router, REGION, "CreateTags", &params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateTags on {resource_id} must succeed, got {status}: {body}"
    );
}

/// Every `tagSet` entry of a `DescribeTags` body, as the rows the report carries.
fn reported(case: &str, body: &str) -> Vec<Described> {
    let document = roxmltree::Document::parse(body)
        .unwrap_or_else(|error| panic!("case {case:?}: the body must be XML ({error}): {body}"));
    document
        .descendants()
        .filter(|node| node.is_element() && node.tag_name().name() == "tagSet")
        .flat_map(|set| {
            set.children()
                .filter(|node| node.is_element() && node.tag_name().name() == "item")
                .collect::<Vec<_>>()
        })
        .map(|item| {
            let text = |name: &str| {
                item.children()
                    .find(|node| node.is_element() && node.tag_name().name() == name)
                    .map(|node| node.text().unwrap_or_default().to_string())
                    .unwrap_or_default()
            };
            Described {
                resource_id: text("resourceId"),
                resource_type: text("resourceType"),
                key: text("key"),
                value: text("value"),
            }
        })
        .collect()
}

/// One `DescribeTags` call that must succeed, as the rows it reported and the `nextToken` it
/// carried.
async fn describe(
    router: &MultiServiceRouter,
    case: &str,
    params: &Params,
) -> (Vec<Described>, Option<String>) {
    let (status, body) = call(router, REGION, "DescribeTags", params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeTags must succeed, got {status}: {body}"
    );
    let token = super::ec2_query::member(&body, "nextToken");
    (reported(case, &body), token)
}

/// The rows the report carries for one resource, as a set — the report's order is explicitly
/// unspecified ("the order of the elements in the response … might vary"), so every
/// assertion compares sets.
async fn described_for(
    router: &MultiServiceRouter,
    case: &str,
    resource_id: &str,
) -> BTreeSet<Described> {
    let params = filters(&[("resource-id", &[resource_id])]);
    let (rows, _) = describe(router, case, &params).await;
    rows.into_iter().collect()
}

/// Rule 1: every tag written on a resource of any kind the service stores tags for is
/// reported, with that resource's id and `ResourceType` and the tag's own key and value.
#[tokio::test]
async fn describe_tags_reports_every_tag_of_every_resource_kind() {
    for (kind, resource_type) in KINDS {
        let case = format!("{kind}: every stored tag is reported");
        let router = emulator();
        let resource_id = create_resource(&router, &case, kind).await;
        tag_resource(&router, &case, &resource_id, &STORED_TAGS).await;

        let expected: BTreeSet<Described> = STORED_TAGS
            .iter()
            .map(|(key, value)| Described {
                resource_id: resource_id.clone(),
                resource_type: resource_type.to_string(),
                key: (*key).to_string(),
                value: (*value).to_string(),
            })
            .collect();
        let actual = described_for(&router, &case, &resource_id).await;
        assert_eq!(
            actual, expected,
            "case {case:?}: expected the report {expected:?}, got {actual:?}"
        );
    }
}

/// Rule 1, continued: the report is read off the live store, so a tag `DeleteTags` removes is
/// gone from it, a value `CreateTags` overwrites is reported as the new value, and a resource
/// that was never tagged contributes nothing.
#[tokio::test]
async fn describe_tags_reports_the_store_as_it_stands() {
    let case = "the report tracks what the store holds";
    let router = emulator();
    let vpc_id = create_vpc(&router, case, "10.0.0.0/16").await;
    let untagged_vpc_id = create_vpc(&router, case, "10.9.0.0/16").await;
    tag_resource(
        &router,
        case,
        &vpc_id,
        &[("env", "prod"), ("owner", "team-a")],
    )
    .await;

    // A resource nobody tagged carries no tag, so it contributes no entry at all.
    let untagged = described_for(&router, case, &untagged_vpc_id).await;
    assert!(
        untagged.is_empty(),
        "case {case:?}: an untagged resource must contribute no entry, got {untagged:?}"
    );

    // An overwritten value is reported as the value the store now holds.
    tag_resource(&router, case, &vpc_id, &[("env", "staging")]).await;
    let after_overwrite = described_for(&router, case, &vpc_id).await;
    let expected: BTreeSet<Described> = [("env", "staging"), ("owner", "team-a")]
        .iter()
        .map(|(key, value)| Described {
            resource_id: vpc_id.clone(),
            resource_type: "vpc".to_string(),
            key: (*key).to_string(),
            value: (*value).to_string(),
        })
        .collect();
    assert_eq!(
        after_overwrite, expected,
        "case {case:?}: expected {expected:?} after the overwrite, got {after_overwrite:?}"
    );

    // A removed tag is gone from the report.
    let (status, body) = call(
        &router,
        REGION,
        "DeleteTags",
        &[p("ResourceId.1", &vpc_id), p("Tag.1.Key", "env")],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: DeleteTags must succeed, got {status}: {body}"
    );
    let after_delete = described_for(&router, case, &vpc_id).await;
    let expected: BTreeSet<Described> = [Described {
        resource_id: vpc_id.clone(),
        resource_type: "vpc".to_string(),
        key: "owner".to_string(),
        value: "team-a".to_string(),
    }]
    .into_iter()
    .collect();
    assert_eq!(
        after_delete, expected,
        "case {case:?}: expected {expected:?} after the delete, got {after_delete:?}"
    );
}

/// Rule 1, continued: a request that names no filter at all reports the tags of EVERY
/// resource, not of one kind.
#[tokio::test]
async fn describe_tags_spans_every_tagged_resource_when_nothing_narrows_it() {
    let case = "an unfiltered report spans the kinds";
    let router = emulator();
    let vpc_id = create_vpc(&router, case, "10.0.0.0/16").await;
    let instance_id = create_resource(&router, case, "instance").await;
    tag_resource(&router, case, &vpc_id, &[("env", "prod")]).await;
    tag_resource(&router, case, &instance_id, &[("env", "staging")]).await;

    let (rows, _) = describe(&router, case, &Vec::new()).await;
    let reported: BTreeSet<Described> = rows.into_iter().collect();
    for expected in [
        Described {
            resource_id: vpc_id.clone(),
            resource_type: "vpc".to_string(),
            key: "env".to_string(),
            value: "prod".to_string(),
        },
        Described {
            resource_id: instance_id.clone(),
            resource_type: "instance".to_string(),
            key: "env".to_string(),
            value: "staging".to_string(),
        },
    ] {
        assert!(
            reported.contains(&expected),
            "case {case:?}: the unfiltered report must carry {expected:?}, got {reported:?}"
        );
    }
}

/// One row of the filter table: the case, the filters the request carries, and the
/// `(resource, key, value)` triples the report must then hold — the resource named as the
/// marker the setup below keys its ids by.
struct FilterRow {
    case: &'static str,
    filters: &'static [(&'static str, &'static [&'static str])],
    selected: &'static [(&'static str, &'static str, &'static str)],
}

/// Rule 2: the documented filters narrow the report, and only the entries that satisfy every
/// one of them are reported.
#[tokio::test]
async fn describe_tags_selects_exactly_what_the_filters_match() {
    let router = emulator();
    let case = "the filter table's setup";
    let vpc_id = create_vpc(&router, case, "10.0.0.0/16").await;
    let subnet_id = created_id(
        &router,
        case,
        "CreateSubnet",
        &[p("VpcId", &vpc_id), p("CidrBlock", "10.0.1.0/24")],
        "subnetId",
    )
    .await;
    tag_resource(
        &router,
        case,
        &vpc_id,
        &[("env", "prod"), ("team", "blue")],
    )
    .await;
    tag_resource(
        &router,
        case,
        &subnet_id,
        &[("env", "staging"), ("team", "blue")],
    )
    .await;

    // The kinds this table's expectations name, as the id and `ResourceType` of each.
    let resource = |marker: &str| -> (String, String) {
        match marker {
            "vpc" => (vpc_id.clone(), "vpc".to_string()),
            "subnet" => (subnet_id.clone(), "subnet".to_string()),
            other => panic!("this table knows no resource {other:?}"),
        }
    };

    let rows: [FilterRow; 14] = [
        FilterRow {
            case: "key selects every tag under that key, whichever resource holds it",
            filters: &[("key", &["env"])],
            selected: &[("vpc", "env", "prod"), ("subnet", "env", "staging")],
        },
        FilterRow {
            case: "value selects on the tag's value",
            filters: &[("value", &["prod"])],
            selected: &[("vpc", "env", "prod")],
        },
        FilterRow {
            case: "two values for one filter are joined with an OR",
            filters: &[("value", &["prod", "staging"])],
            selected: &[("vpc", "env", "prod"), ("subnet", "env", "staging")],
        },
        FilterRow {
            case: "resource-id selects the tags of one resource",
            filters: &[("resource-id", &["#vpc"])],
            selected: &[("vpc", "env", "prod"), ("vpc", "team", "blue")],
        },
        FilterRow {
            case: "resource-type selects every tag of that kind",
            filters: &[("resource-type", &["subnet"])],
            selected: &[("subnet", "env", "staging"), ("subnet", "team", "blue")],
        },
        FilterRow {
            case: "tag:<key> selects the tag under that key with that value",
            filters: &[("tag:env", &["prod"])],
            selected: &[("vpc", "env", "prod")],
        },
        FilterRow {
            case: "tag:<key> with a wildcard selects every tag under that key",
            filters: &[("tag:env", &["*"])],
            selected: &[("vpc", "env", "prod"), ("subnet", "env", "staging")],
        },
        FilterRow {
            case: "tag:<key> naming a key nothing holds selects nothing",
            filters: &[("tag:absent", &["*"])],
            selected: &[],
        },
        FilterRow {
            case: "two filters are joined with an AND",
            filters: &[("key", &["team"]), ("resource-type", &["vpc"])],
            selected: &[("vpc", "team", "blue")],
        },
        FilterRow {
            case: "three filters all have to hold",
            filters: &[
                ("key", &["env"]),
                ("value", &["staging"]),
                ("resource-id", &["#subnet"]),
            ],
            selected: &[("subnet", "env", "staging")],
        },
        FilterRow {
            case: "filters that cannot hold together select nothing",
            filters: &[("key", &["env"]), ("value", &["blue"])],
            selected: &[],
        },
        FilterRow {
            case: "a trailing wildcard matches every value with that prefix",
            filters: &[("value", &["bl*"])],
            selected: &[("vpc", "team", "blue"), ("subnet", "team", "blue")],
        },
        FilterRow {
            case: "the single-character wildcard matches exactly one character",
            filters: &[("key", &["en?"])],
            selected: &[("vpc", "env", "prod"), ("subnet", "env", "staging")],
        },
        FilterRow {
            case: "comparisons are case-sensitive",
            filters: &[("value", &["PROD"])],
            selected: &[],
        },
    ];

    for row in rows {
        // A filter value naming a resource is written as the marker, resolved here: an id
        // this run minted cannot be a literal in the table.
        let entries: Vec<(&str, Vec<String>)> = row
            .filters
            .iter()
            .map(|(name, values)| {
                let values: Vec<String> = values
                    .iter()
                    .map(|value| match value.strip_prefix('#') {
                        Some(marker) => resource(marker).0,
                        None => (*value).to_string(),
                    })
                    .collect();
                (*name, values)
            })
            .collect();
        let as_slices: Vec<(&str, Vec<&str>)> = entries
            .iter()
            .map(|(name, values)| (*name, values.iter().map(String::as_str).collect()))
            .collect();
        let params = filters(
            &as_slices
                .iter()
                .map(|(name, values)| (*name, values.as_slice()))
                .collect::<Vec<_>>(),
        );

        let (rows_reported, token) = describe(&router, row.case, &params).await;
        let expected: BTreeSet<Described> = row
            .selected
            .iter()
            .map(|(marker, key, value)| {
                let (resource_id, resource_type) = resource(marker);
                Described {
                    resource_id,
                    resource_type,
                    key: (*key).to_string(),
                    value: (*value).to_string(),
                }
            })
            .collect();
        let actual: BTreeSet<Described> = rows_reported.into_iter().collect();
        assert_eq!(
            actual, expected,
            "case {:?}: expected {expected:?}, got {actual:?}",
            row.case
        );
        assert!(
            token.is_none(),
            "case {:?}: a page that holds every selected tag must carry no nextToken, \
             got {token:?}",
            row.case
        );
    }
}

/// Rule 3: `MaxResults` pages the selected tags, and following the token walks every one of
/// them exactly once.
#[tokio::test]
async fn describe_tags_pages_every_selected_tag_exactly_once() {
    // One page size per row, spanning the published floor, a size that divides the set
    // evenly, one that does not, and one larger than the whole set.
    for page_size in [5, 6, 7, 11, 12, 20] {
        let case = format!("paging {TAG_COUNT} tags {page_size} at a time");
        let router = emulator();
        let vpc_id = create_vpc(&router, &case, "10.0.0.0/16").await;
        let stored: Vec<(String, String)> = (0..TAG_COUNT)
            .map(|index| (format!("key-{index:02}"), format!("value-{index:02}")))
            .collect();
        let as_pairs: Vec<(&str, &str)> = stored
            .iter()
            .map(|(key, value)| (key.as_str(), value.as_str()))
            .collect();
        tag_resource(&router, &case, &vpc_id, &as_pairs).await;

        let mut walked: Vec<Described> = Vec::new();
        let mut token: Option<String> = None;
        // Every page carries at least one tag, so the walk cannot take more turns than there
        // are tags; a token that never ran out would be the bug this bound catches.
        for turn in 0..=TAG_COUNT {
            let mut params = filters(&[("resource-id", &[vpc_id.as_str()])]);
            params.push(p("MaxResults", page_size.to_string()));
            if let Some(token) = token.as_deref() {
                params.push(p("NextToken", token));
            }
            let (rows, next) = describe(&router, &case, &params).await;
            assert!(
                rows.len() <= page_size,
                "case {case:?} turn {turn}: a page must hold at most {page_size} tags, \
                 got {}",
                rows.len()
            );
            walked.extend(rows);
            match next {
                None => break,
                Some(next) => token = Some(next),
            }
            assert!(
                turn < TAG_COUNT,
                "case {case:?}: the walk must end, got a token on turn {turn}"
            );
        }

        let expected: BTreeSet<Described> = stored
            .iter()
            .map(|(key, value)| Described {
                resource_id: vpc_id.clone(),
                resource_type: "vpc".to_string(),
                key: key.clone(),
                value: value.clone(),
            })
            .collect();
        let walked_set: BTreeSet<Described> = walked.iter().cloned().collect();
        assert_eq!(
            walked_set, expected,
            "case {case:?}: the walk must report every stored tag, got {walked_set:?}"
        );
        assert_eq!(
            walked.len(),
            expected.len(),
            "case {case:?}: the walk must report each tag once, got {walked:?}"
        );
    }
}

/// The tags one resource holds for the paging table: more than the page sizes below, and
/// inside the 50 tags a resource may carry (`CreateTags`).
const TAG_COUNT: usize = 12;

/// One row of the refusal table: the case, the parameters the request carries, the HTTP
/// status and EC2's code, and a fragment the message has to carry.
struct RefusalRow {
    case: &'static str,
    params: &'static [(&'static str, &'static str)],
    status: u16,
    code: &'static str,
    message: &'static str,
}

/// Rule 3: a request outside the published page range, carrying a token this service did not
/// mint, naming a filter this operation does not document, or asking for a dry run, is
/// refused with EC2's own code and reports nothing.
#[tokio::test]
async fn describe_tags_refuses_what_it_cannot_serve() {
    let rows: [RefusalRow; 9] = [
        RefusalRow {
            case: "a page size below the published minimum",
            params: &[("MaxResults", "4")],
            status: 400,
            code: "InvalidParameterValue",
            message: "Maximum results allowed are between 5 and 1000",
        },
        RefusalRow {
            case: "a page size of nothing",
            params: &[("MaxResults", "0")],
            status: 400,
            code: "InvalidParameterValue",
            message: "Maximum results allowed are between 5 and 1000",
        },
        RefusalRow {
            case: "a negative page size",
            params: &[("MaxResults", "-1")],
            status: 400,
            code: "InvalidParameterValue",
            message: "Maximum results allowed are between 5 and 1000",
        },
        RefusalRow {
            case: "a page size above the published maximum",
            params: &[("MaxResults", "1001")],
            status: 400,
            code: "InvalidParameterValue",
            message: "Maximum results allowed are between 5 and 1000",
        },
        RefusalRow {
            case: "a token this service did not mint",
            params: &[("NextToken", "not-a-token")],
            status: 400,
            code: "InvalidPaginationToken",
            message: ec2::state::INVALID_PAGINATION_TOKEN_MESSAGE,
        },
        RefusalRow {
            case: "a filter this operation does not document",
            params: &[("Filter.1.Name", "bogus"), ("Filter.1.Value.1", "x")],
            status: 400,
            code: "InvalidParameterValue",
            message: "The filter 'bogus' is invalid",
        },
        RefusalRow {
            case: "the tag filter prefix with no key after it",
            params: &[("Filter.1.Name", "tag:"), ("Filter.1.Value.1", "x")],
            status: 400,
            code: "InvalidParameterValue",
            message: "The filter 'tag:' is invalid",
        },
        RefusalRow {
            case: "a filter name that differs only in case",
            params: &[("Filter.1.Name", "Key"), ("Filter.1.Value.1", "env")],
            status: 400,
            code: "InvalidParameterValue",
            message: "The filter 'Key' is invalid",
        },
        RefusalRow {
            case: "a dry run by a caller that holds the permission",
            params: &[("DryRun", "true")],
            status: 412,
            code: "DryRunOperation",
            message: "Request would have succeeded, but DryRun flag is set",
        },
    ];

    for row in rows {
        let router = emulator();
        let vpc_id = create_vpc(&router, row.case, "10.0.0.0/16").await;
        tag_resource(&router, row.case, &vpc_id, &[("env", "prod")]).await;

        let params: Params = row
            .params
            .iter()
            .map(|(name, value)| p(name, *value))
            .collect();
        let (status, body) = call(&router, REGION, "DescribeTags", &params).await;
        assert_eq!(
            status, row.status,
            "case {:?}: expected HTTP {}, got {status}: {body}",
            row.case, row.status
        );
        let code = error_code(&body);
        assert_eq!(
            code, row.code,
            "case {:?}: expected {}, got {code}: {body}",
            row.case, row.code
        );
        let message = error_message(&body);
        assert!(
            message.contains(row.message),
            "case {:?}: expected a message carrying {:?}, got {message:?}",
            row.case,
            row.message
        );

        // A refused read reports nothing, and the store it would have read is untouched: the
        // tag written above is still there for the next call.
        let still_there = described_for(&router, row.case, &vpc_id).await;
        assert_eq!(
            still_there.len(),
            1,
            "case {:?}: a refused read must leave the stored tags alone, got {still_there:?}",
            row.case
        );
    }
}

/// Rule 3: both published bounds of the page range are served, not refused.
#[tokio::test]
async fn describe_tags_serves_the_published_page_bounds() {
    for page_size in ["5", "1000"] {
        let case = format!("a page size of {page_size} is served");
        let router = emulator();
        let vpc_id = create_vpc(&router, &case, "10.0.0.0/16").await;
        tag_resource(&router, &case, &vpc_id, &[("env", "prod")]).await;

        let mut params = filters(&[("resource-id", &[vpc_id.as_str()])]);
        params.push(p("MaxResults", page_size));
        let (rows, token) = describe(&router, &case, &params).await;
        assert_eq!(
            rows.len(),
            1,
            "case {case:?}: expected the one stored tag, got {rows:?}"
        );
        assert!(
            token.is_none(),
            "case {case:?}: a page holding every selected tag must carry no token, \
             got {token:?}"
        );
    }
}

/// Rule 3, continued: a `Filters` list the request can make arbitrarily expensive is refused at
/// the ceilings this read is the only enforcer of — the model puts no `@length` on `FilterList`
/// or `Filter$Values`, so a request past one of them is told so instead of being evaluated.
///
/// The bounds are the ones `crate::state` holds every describe of this service to
/// (`MAX_FILTERS`, `MAX_FILTER_VALUES`, `MAX_FILTER_VALUE_BYTES`); each row sends one more than
/// the bound it tests and leaves the other two well inside, so what the refusal names is the
/// factor the row grew. One row inside every bound proves the ceilings do not refuse an
/// ordinary request.
#[tokio::test]
async fn describe_tags_refuses_a_filter_list_past_its_ceilings() {
    /// The most filters one request may carry (`crate::state::MAX_FILTERS`).
    const MAX_FILTERS: usize = 32;
    /// The most values one filter may carry (`crate::state::MAX_FILTER_VALUES`).
    const MAX_FILTER_VALUES: usize = 64;
    /// The most bytes one filter value may carry (`crate::state::MAX_FILTER_VALUE_BYTES`, the
    /// longest tag value EC2 accepts in its widest UTF-8 encoding).
    const MAX_FILTER_VALUE_BYTES: usize = 1024;

    // (case, filters, values per filter, bytes per value, the fragment the answer must carry —
    // `None` for the row every bound admits).
    let rows: [(&str, usize, usize, usize, Option<&str>); 7] = [
        ("every bound respected", 2, 2, 3, None),
        ("the filter count at its bound", MAX_FILTERS, 1, 3, None),
        (
            "one filter too many",
            MAX_FILTERS + 1,
            1,
            3,
            Some("filters, above the maximum"),
        ),
        ("a filter's values at their bound", 1, MAX_FILTER_VALUES, 3, None),
        (
            "one value too many for a filter",
            1,
            MAX_FILTER_VALUES + 1,
            3,
            Some("values, above the maximum"),
        ),
        ("a value at its byte bound", 1, 1, MAX_FILTER_VALUE_BYTES, None),
        (
            "a value one byte too long",
            1,
            1,
            MAX_FILTER_VALUE_BYTES + 1,
            Some("bytes, above the maximum"),
        ),
    ];

    for (case, filter_count, values_per_filter, value_bytes, refusal) in rows {
        let router = emulator();
        let vpc_id = create_vpc(&router, case, "10.0.0.0/16").await;
        tag_resource(&router, case, &vpc_id, &[("env", "prod")]).await;

        // Every entry names the one filter this report documents for a tag's key, so the only
        // thing a row varies is the SIZE of the list, not whether the names resolve.
        let mut params: Params = Vec::new();
        for filter in 1..=filter_count {
            params.push(p(&format!("Filter.{filter}.Name"), "key"));
            for value in 1..=values_per_filter {
                params.push(p(
                    &format!("Filter.{filter}.Value.{value}"),
                    "v".repeat(value_bytes),
                ));
            }
        }

        let (status, body) = call(&router, REGION, "DescribeTags", &params).await;
        match refusal {
            None => assert_eq!(
                status, 200,
                "case {case:?}: a request inside every ceiling must be served, got {status}: \
                 {body}"
            ),
            Some(fragment) => {
                assert_eq!(
                    status, 400,
                    "case {case:?}: expected HTTP 400, got {status}: {body}"
                );
                let code = error_code(&body);
                assert_eq!(
                    code, "InvalidParameterValue",
                    "case {case:?}: expected InvalidParameterValue, got {code}: {body}"
                );
                let message = error_message(&body);
                assert!(
                    message.contains(fragment),
                    "case {case:?}: expected a message carrying {fragment:?}, got {message:?}"
                );
            }
        }

        // Refused or served, the read wrote nothing: the tag is still there for the next call.
        let still_there = described_for(&router, case, &vpc_id).await;
        assert_eq!(
            still_there.len(),
            1,
            "case {case:?}: the read must leave the stored tags alone, got {still_there:?}"
        );
    }
}
