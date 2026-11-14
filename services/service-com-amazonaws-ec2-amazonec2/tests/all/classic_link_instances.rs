// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `DescribeClassicLinkInstances`, driven over the wire the service
//! actually speaks: `ec2Query` — an `application/x-www-form-urlencoded` POST whose body
//! carries `Action=DescribeClassicLinkInstances`, answered with XML (README.md /
//! OPERATIONS.md "Wire protocol"). ec2Query is selected by the `ec2` signing name in the
//! request's SigV4 credential scope, so every call here carries one.
//!
//! The rule under test is that the operation REPORTS THE LINKS THE STORE HOLDS: the set it
//! answers with is the account's linked EC2-Classic instances, each reported with the VPC
//! and security groups its link names and the tags its instance carries, narrowed by the
//! `InstanceIds` and `Filters` the request names and cut into `MaxResults`-sized pages. An
//! instance the account holds but has not linked is never reported — "This request only
//! returns information about EC2-Classic instances linked to a VPC through ClassicLink" —
//! and describing is a READ: two calls report the same thing.
//!
//! So every table row below is a distinct input along what that generalises over: which
//! filter, how many values, which values, how many security groups a link names (down to
//! none, and one the account no longer holds), which VPC, which ids, which page size. An
//! account with no link at all is one row among them rather than the specification.

use std::collections::BTreeMap;

use ec2::state::ComAmazonawsEc2Amazonec2State as Ec2State;
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{element, error_code, error_message, indexed};
use super::wire::{child, child_text, document, set_items};

/// The Region every call here is signed for. The link store is per account rather than per
/// Region, so the Region only selects the ec2Query envelope and the zones the launches land
/// in.
const REGION: &str = "us-west-2";

/// The ec2Query action under test.
const ACTION: &str = "DescribeClassicLinkInstances";

/// The image every launch here names. Which AMI it is does not matter to a ClassicLink
/// link, so one is used throughout.
const AMI: &str = "ami-0123456789abcdef0";

/// How many instances this suite launches. All but the last are linked, so the unlinked one
/// is what pins that a stored instance is not a reported row.
const LAUNCHED: usize = 8;

/// A security group id of the right shape that the account holds no group for: a link may
/// name a group that has since been deleted, and `GroupIdentifier$GroupName` is then absent
/// rather than invented.
const UNHELD_GROUP: &str = "sg-0000000000000beef";

/// One `(key, value)` request parameter.
fn p(key: &str, value: impl Into<String>) -> (String, String) {
    (key.to_string(), value.into())
}

/// One `DescribeClassicLinkInstances` call carrying `params`, as its HTTP status and body.
async fn describe(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    super::ec2_query::call(router, REGION, ACTION, params).await
}

/// One call that must succeed, as the rows it reported.
async fn described(
    router: &MultiServiceRouter,
    case: &str,
    params: &[(String, String)],
) -> Vec<Row> {
    let (status, body) = describe(router, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with body {body}"
    );
    rows(case, &body)
}

/// The instance ids a successful call reported, in the order it reported them.
async fn described_ids(
    router: &MultiServiceRouter,
    case: &str,
    params: &[(String, String)],
) -> Vec<String> {
    described(router, case, params)
        .await
        .into_iter()
        .map(|row| row.instance_id)
        .collect()
}

/// One `MaxResults` parameter.
fn max_results_param(max_results: i64) -> Vec<(String, String)> {
    vec![p("MaxResults", max_results.to_string())]
}

/// One `Filters` entry a case sends: the filter name and the values under it.
struct FilterEntry {
    name: &'static str,
    values: Vec<String>,
}

/// One filter entry carrying `values`.
fn entry(name: &'static str, values: &[&str]) -> FilterEntry {
    FilterEntry {
        name,
        values: values.iter().map(|value| (*value).to_string()).collect(),
    }
}

/// The form fields for a whole `Filters` list, as ec2Query flattens it into indexed keys.
fn filter_params(entries: &[FilterEntry]) -> Vec<(String, String)> {
    let mut params = Vec::new();
    for (index, entry) in entries.iter().enumerate() {
        params.push(p(&format!("Filter.{}.Name", index + 1), entry.name));
        for (position, value) in entry.values.iter().enumerate() {
            params.push(p(
                &format!("Filter.{}.Value.{}", index + 1, position + 1),
                value.as_str(),
            ));
        }
    }
    params
}

// ---------------------------------------------------------------------------
// Reading the response
// ---------------------------------------------------------------------------

/// One `instancesSet/item` of a response: every member the modelled `ClassicLinkInstance`
/// carries.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Row {
    instance_id: String,
    vpc_id: Option<String>,
    /// Each reported security group as `(groupId, groupName)` — the name absent for a group
    /// the account no longer holds.
    groups: Vec<(String, Option<String>)>,
    tags: BTreeMap<String, String>,
}

/// Every linked instance a response body reports, in document order.
fn rows(case: &str, body: &str) -> Vec<Row> {
    let parsed = document(case, body);
    let Some(set) = child(parsed.root_element(), "instancesSet") else {
        return Vec::new();
    };
    set.children()
        .filter(|node| node.is_element() && node.tag_name().name() == "item")
        .map(|item| Row {
            instance_id: child_text(item, "instanceId").unwrap_or_default(),
            vpc_id: child_text(item, "vpcId"),
            groups: set_items(item, "groupSet")
                .into_iter()
                .map(|group| {
                    (
                        child_text(group, "groupId").unwrap_or_default(),
                        child_text(group, "groupName"),
                    )
                })
                .collect(),
            tags: set_items(item, "tagSet")
                .into_iter()
                .map(|tag| {
                    (
                        child_text(tag, "key").unwrap_or_default(),
                        child_text(tag, "value").unwrap_or_default(),
                    )
                })
                .collect(),
        })
        .collect()
}

// ---------------------------------------------------------------------------
// The world the tables are written against
// ---------------------------------------------------------------------------

/// What one seeded link holds: the instance it is a link of (by launch index), the VPC it
/// links that instance to, and the security groups it was linked with.
struct LinkRow {
    instance: usize,
    vpc: VpcChoice,
    /// Which security groups the link names, as indices into [`Seeded::groups`], or
    /// [`GroupChoice::Unheld`] for one the account no longer holds.
    groups: Vec<GroupChoice>,
}

/// Which of the two VPCs a link names.
#[derive(Clone, Copy)]
enum VpcChoice {
    A,
    B,
}

/// Which security group a link names.
#[derive(Clone, Copy)]
enum GroupChoice {
    /// One of the groups this suite created, by index into [`Seeded::groups`].
    Held(usize),
    /// A well-formed group id the account holds no group for.
    Unheld,
}

/// The links the tables below are written against. They differ along every attribute the
/// documented filters select on — the VPC and the security groups — and along what a link
/// may hold: several groups, one, none at all, and one the account no longer holds. Several
/// rows share a VPC and several share a group, so a filter that selects on one reports more
/// than a single row.
fn link_rows() -> Vec<LinkRow> {
    vec![
        LinkRow {
            instance: 0,
            vpc: VpcChoice::A,
            groups: vec![GroupChoice::Held(0)],
        },
        LinkRow {
            instance: 1,
            vpc: VpcChoice::A,
            groups: vec![GroupChoice::Held(0), GroupChoice::Held(1)],
        },
        LinkRow {
            instance: 2,
            vpc: VpcChoice::B,
            groups: vec![GroupChoice::Held(2)],
        },
        // A link that names no security group: the row a `group-id` filter must select
        // nothing for, there being no value for it to match.
        LinkRow {
            instance: 3,
            vpc: VpcChoice::A,
            groups: Vec::new(),
        },
        LinkRow {
            instance: 4,
            vpc: VpcChoice::B,
            groups: vec![
                GroupChoice::Held(0),
                GroupChoice::Held(1),
                GroupChoice::Held(2),
            ],
        },
        LinkRow {
            instance: 5,
            vpc: VpcChoice::A,
            groups: vec![GroupChoice::Unheld],
        },
        LinkRow {
            instance: 6,
            vpc: VpcChoice::B,
            groups: vec![GroupChoice::Held(1)],
        },
        // The instance at index 7 is deliberately NOT linked.
    ]
}

/// The tags this suite writes onto instances, by launch index. The unlinked instance is
/// tagged too, with a key and value a filter row selects on, so a filter can never reach a
/// row that is not linked.
fn tag_rows() -> Vec<TagRow> {
    vec![
        TagRow {
            instance: 0,
            tags: vec![("Name", "web-1"), ("Owner", "TeamA")],
        },
        TagRow {
            instance: 1,
            tags: vec![("Owner", "TeamB")],
        },
        TagRow {
            instance: 2,
            tags: vec![("Name", "web-1")],
        },
        TagRow {
            instance: 7,
            tags: vec![("Name", "web-1"), ("Owner", "TeamA")],
        },
    ]
}

/// The tags one instance is given: which instance, by launch index, and its `(key, value)`
/// pairs.
struct TagRow {
    instance: usize,
    tags: Vec<(&'static str, &'static str)>,
}

/// The emulator the tables are driven against, and what was written into it.
struct Seeded {
    /// Every launched instance id, in launch order.
    instances: Vec<String>,
    /// The two VPCs, as `(VPC A, VPC B)`.
    vpcs: (String, String),
    /// The security groups this suite created, by the index a [`GroupChoice::Held`] names.
    groups: Vec<(String, String)>,
    /// The instance ids that were linked.
    linked: Vec<String>,
}

impl Seeded {
    /// The id of the VPC a link row names.
    fn vpc(&self, choice: VpcChoice) -> &str {
        match choice {
            VpcChoice::A => &self.vpcs.0,
            VpcChoice::B => &self.vpcs.1,
        }
    }

    /// The id of the security group a link row names.
    fn group_id(&self, choice: GroupChoice) -> String {
        match choice {
            GroupChoice::Held(index) => self.groups[index].0.clone(),
            GroupChoice::Unheld => UNHELD_GROUP.to_string(),
        }
    }

    /// The security group `(id, name)` pairs a link row's groups are reported as — the name
    /// absent for the group the account does not hold.
    fn reported_groups(&self, choices: &[GroupChoice]) -> Vec<(String, Option<String>)> {
        choices
            .iter()
            .map(|choice| match choice {
                GroupChoice::Held(index) => (
                    self.groups[*index].0.clone(),
                    Some(self.groups[*index].1.clone()),
                ),
                GroupChoice::Unheld => (UNHELD_GROUP.to_string(), None),
            })
            .collect()
    }

    /// The tags written onto the instance at `instance`.
    fn tags_of(&self, instance: usize) -> BTreeMap<String, String> {
        tag_rows()
            .into_iter()
            .find(|row| row.instance == instance)
            .map(|row| {
                row.tags
                    .into_iter()
                    .map(|(key, value)| (key.to_string(), value.to_string()))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The instance ids of `instances`, in the order the operation reports them (by
    /// `InstanceId`).
    fn ids_at(&self, instances: &[usize]) -> Vec<String> {
        let mut ids: Vec<String> = instances
            .iter()
            .map(|index| self.instances[*index].clone())
            .collect();
        ids.sort();
        ids
    }

    /// Every linked instance id, in the order the operation reports them.
    fn every_linked_id(&self) -> Vec<String> {
        let mut ids = self.linked.clone();
        ids.sort();
        ids
    }
}

/// Call `action`, require HTTP 200, and answer the first `id_element` of the response.
async fn created_id(
    router: &MultiServiceRouter,
    case: &str,
    action: &str,
    params: &[(String, String)],
    id_element: &str,
) -> String {
    let (status, body) = super::ec2_query::call(router, REGION, action, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: {action} must succeed, got {status} with body {body}"
    );
    element(&body, id_element).unwrap_or_else(|| {
        panic!("case {case:?}: {action} must report a <{id_element}>: {body}")
    })
}

/// Build the world: two VPCs, three security groups, [`LAUNCHED`] instances with the tags
/// [`tag_rows`] names, and the links [`link_rows`] names written through the store's one
/// link seam.
async fn seed(router: &MultiServiceRouter, engine: &engine::InMemoryEngine) -> Seeded {
    let vpc_a = created_id(
        router,
        "VPC A",
        "CreateVpc",
        &[p("CidrBlock", "10.0.0.0/16")],
        "vpcId",
    )
    .await;
    let vpc_b = created_id(
        router,
        "VPC B",
        "CreateVpc",
        &[p("CidrBlock", "10.1.0.0/16")],
        "vpcId",
    )
    .await;

    // Three groups, two in one VPC and one in the other, so a `group-id` filter and a
    // `vpc-id` filter select different sets.
    let mut groups: Vec<(String, String)> = Vec::new();
    for (name, vpc_id) in [
        ("classiclink-a", &vpc_a),
        ("classiclink-b", &vpc_a),
        ("classiclink-c", &vpc_b),
    ] {
        let group_id = created_id(
            router,
            name,
            "CreateSecurityGroup",
            &[
                p("GroupName", name),
                p("GroupDescription", "a group a ClassicLink link names"),
                p("VpcId", vpc_id.as_str()),
            ],
            "groupId",
        )
        .await;
        groups.push((group_id, name.to_string()));
    }

    let (status, body) = super::ec2_query::call(
        router,
        REGION,
        "RunInstances",
        &[
            p("ImageId", AMI),
            p("MinCount", LAUNCHED.to_string()),
            p("MaxCount", LAUNCHED.to_string()),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "RunInstances must succeed, got {status} with body {body}"
    );
    let instances = super::ec2_query::elements(&body, "instanceId");
    assert_eq!(
        instances.len(),
        LAUNCHED,
        "RunInstances must mint {LAUNCHED} instances, got {instances:?}"
    );

    for row in tag_rows() {
        let index = row.instance;
        let mut params = vec![p("ResourceId.1", instances[index].as_str())];
        for (position, (key, value)) in row.tags.iter().enumerate() {
            params.push(p(&format!("Tag.{}.Key", position + 1), *key));
            params.push(p(&format!("Tag.{}.Value", position + 1), *value));
        }
        let (status, body) = super::ec2_query::call(router, REGION, "CreateTags", &params).await;
        assert_eq!(
            status, 200,
            "CreateTags on instance {index} must succeed, got {status} with body {body}"
        );
    }

    let mut seeded = Seeded {
        instances,
        vpcs: (vpc_a, vpc_b),
        groups,
        linked: Vec::new(),
    };

    // Written the way `AttachClassicLinkVpc` must write them: through the store's one link
    // seam, which is what that operation (still a scaffold) has to call.
    for row in link_rows() {
        let instance_id = seeded.instances[row.instance].clone();
        let vpc_id = seeded.vpc(row.vpc).to_string();
        let group_ids: Vec<String> = row
            .groups
            .iter()
            .map(|choice| seeded.group_id(*choice))
            .collect();
        let took = engine
            .state::<Ec2State>()
            .lock()
            .link_classic_link_vpc(&instance_id, &vpc_id, group_ids);
        assert!(
            took,
            "the store must take the link of {instance_id} to {vpc_id}"
        );
        seeded.linked.push(instance_id);
    }
    seeded
}

/// The emulator with the world seeded into it.
async fn emulator() -> (MultiServiceRouter, std::sync::Arc<engine::InMemoryEngine>, Seeded) {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&router, &engine).await;
    (router, engine, seeded)
}

// ---------------------------------------------------------------------------
// The store is what is reported
// ---------------------------------------------------------------------------

/// An account that has linked nothing reports an empty set — EC2-Classic instances are not
/// created by any EC2 call and a link is made by `AttachClassicLinkVpc` — so an instance it
/// holds is not a reported row, and describing is a READ: two calls report the same thing
/// and the store is untouched.
#[tokio::test]
async fn an_account_with_no_link_reports_an_empty_set_and_describing_writes_nothing() {
    let (router, engine) = super::ec2_query::emulator();
    let instance_id = created_id(
        &router,
        "one unlinked instance",
        "RunInstances",
        &[p("ImageId", AMI), p("MinCount", "1"), p("MaxCount", "1")],
        "instanceId",
    )
    .await;

    let (status, body) = describe(&router, &[]).await;
    assert_eq!(
        status, 200,
        "an unlinked account: expected HTTP 200, got {status} with body {body}"
    );
    let reported = rows("an unlinked account", &body);
    assert!(
        reported.is_empty(),
        "an unlinked account: expected no linked instance, got {reported:?} from {body}"
    );
    assert!(
        element(&body, "nextToken").is_none(),
        "an unlinked account: expected no pagination token, got {body}"
    );

    let again = described_ids(&router, "a second call", &[]).await;
    assert!(
        again.is_empty(),
        "a second call: expected no linked instance, got {again:?}"
    );
    // Naming the instance explicitly cannot conjure a link either.
    let named = described_ids(
        &router,
        "the unlinked instance by id",
        &indexed("InstanceId", &[instance_id.as_str()]),
    )
    .await;
    assert!(
        named.is_empty(),
        "the unlinked instance by id: expected no row, got {named:?}"
    );
    assert_eq!(
        engine.state::<Ec2State>().lock().classic_link_instances.len(),
        0,
        "describing linked instances must not write a link"
    );
}

/// Every stored link is reported, in `InstanceId` order, with every member of the modelled
/// shape read off the resources the link names — and a link or a tag written after a call
/// shows up in the next one, so the answer is computed from the store rather than fixed.
#[tokio::test]
async fn every_stored_link_is_reported_and_later_writes_are_visible() {
    let (router, engine, seeded) = emulator().await;

    let reported = described(&router, "every link", &[]).await;
    let reported_ids: Vec<String> = reported.iter().map(|row| row.instance_id.clone()).collect();
    assert_eq!(
        reported_ids,
        seeded.every_linked_id(),
        "every link: expected every linked instance in InstanceId order, got {reported_ids:?}"
    );

    for row in link_rows() {
        let instance_id = &seeded.instances[row.instance];
        let case = format!("the link of instance {}", row.instance);
        let answered = reported
            .iter()
            .find(|answered| &answered.instance_id == instance_id)
            .unwrap_or_else(|| {
                panic!("case {case:?}: expected {instance_id} to be reported, got {reported_ids:?}")
            });
        assert_eq!(
            answered.vpc_id.as_deref(),
            Some(seeded.vpc(row.vpc)),
            "case {case:?}: expected the VPC the link names, got {:?}",
            answered.vpc_id
        );
        assert_eq!(
            answered.groups,
            seeded.reported_groups(&row.groups),
            "case {case:?}: expected the security groups the link names, got {:?}",
            answered.groups
        );
        assert_eq!(
            answered.tags,
            seeded.tags_of(row.instance),
            "case {case:?}: expected the tags of the instance, got {:?}",
            answered.tags
        );
    }

    // A tag written AFTER the link is reported by the next call: the tags come from the
    // instance rather than from a copy taken when the link was made.
    let late_tagged = seeded.instances[4].clone();
    let (status, body) = super::ec2_query::call(
        &router,
        REGION,
        "CreateTags",
        &[
            p("ResourceId.1", late_tagged.as_str()),
            p("Tag.1.Key", "Phase"),
            p("Tag.1.Value", "late"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "a late tag: CreateTags must succeed, got {status} with body {body}"
    );
    let after_tag = described(&router, "after a late tag", &[]).await;
    let tagged_row = after_tag
        .iter()
        .find(|row| row.instance_id == late_tagged)
        .unwrap_or_else(|| panic!("after a late tag: {late_tagged} must still be reported"));
    assert_eq!(
        tagged_row.tags.get("Phase").map(String::as_str),
        Some("late"),
        "after a late tag: expected the new tag to be reported, got {:?}",
        tagged_row.tags
    );

    // A link written after a call shows up in the next one.
    let late_linked = seeded.instances[7].clone();
    let vpc_id = seeded.vpcs.1.clone();
    let took = engine.state::<Ec2State>().lock().link_classic_link_vpc(
        &late_linked,
        &vpc_id,
        vec![seeded.groups[2].0.clone()],
    );
    assert!(took, "a late link: the store must take it");
    let after_link = described_ids(&router, "after a late link", &[]).await;
    let mut expected = seeded.every_linked_id();
    expected.push(late_linked.clone());
    expected.sort();
    assert_eq!(
        after_link, expected,
        "after a late link: expected {late_linked:?} to be reported too, got {after_link:?}"
    );
}

// ---------------------------------------------------------------------------
// Filters
// ---------------------------------------------------------------------------

/// Each documented filter selects on its own attribute, values within one filter are OR'd,
/// a value may carry EC2's `*` wildcard, and the comparison is case-sensitive.
#[tokio::test]
async fn each_documented_filter_selects_the_links_whose_attribute_equals_a_value() {
    let (router, _engine, seeded) = emulator().await;
    let (vpc_a, vpc_b) = seeded.vpcs.clone();
    let group_a = seeded.groups[0].0.clone();
    let group_b = seeded.groups[1].0.clone();
    let group_c = seeded.groups[2].0.clone();

    // One row per filter name `DescribeClassicLinkInstancesRequest$Filters` documents, each
    // with several values, a value nothing holds, a wildcard and the wrong case beside it.
    let cases: Vec<FilterCase> = vec![
        filter_case("vpc-id", &[vpc_a.as_str()], &[0, 1, 3, 5]),
        filter_case(
            "vpc-id",
            &[vpc_a.as_str(), vpc_b.as_str()],
            &[0, 1, 2, 3, 4, 5, 6],
        ),
        filter_case("vpc-id", &["vpc-0000000000000beef"], &[]),
        filter_case("vpc-id", &[vpc_b.to_uppercase().as_str()], &[]),
        filter_case("group-id", &[group_a.as_str()], &[0, 1, 4]),
        filter_case(
            "group-id",
            &[group_b.as_str(), group_c.as_str()],
            &[1, 2, 4, 6],
        ),
        // A link may name a group the account no longer holds; the link's own value is what
        // the filter compares against.
        filter_case("group-id", &[UNHELD_GROUP], &[5]),
        filter_case("group-id", &[group_a.to_uppercase().as_str()], &[]),
        filter_case("instance-id", &[seeded.instances[0].as_str()], &[0]),
        filter_case(
            "instance-id",
            &[
                seeded.instances[6].as_str(),
                seeded.instances[2].as_str(),
            ],
            &[2, 6],
        ),
        // The instance at index 7 is tagged like index 0 but is NOT linked, so no filter
        // reaches it.
        filter_case("instance-id", &[seeded.instances[7].as_str()], &[]),
        // EC2 filter values take `*` and `?` wildcards.
        filter_case("instance-id", &["i-*"], &[0, 1, 2, 3, 4, 5, 6]),
        filter_case("tag:Name", &["web-1"], &[0, 2]),
        filter_case("tag:Owner", &["TeamA"], &[0]),
        filter_case("tag:Owner", &["Team*"], &[0, 1]),
        filter_case("tag:Owner", &["teama"], &[]),
        filter_case("tag:Missing", &["web-1"], &[]),
        filter_case("tag-key", &["Owner"], &[0, 1]),
        filter_case("tag-key", &["Name"], &[0, 2]),
        filter_case("tag-key", &["Name", "Owner"], &[0, 1, 2]),
        filter_case("tag-key", &["Absent"], &[]),
        // A filter carrying NO value matches nothing: there is no value for the attribute
        // to match, which is how every filtered describe of this crate reads one.
        filter_case("vpc-id", &[], &[]),
    ];

    for case in cases {
        let label = format!("{}={:?}", case.entry.name, case.entry.values);
        let reported = described_ids(&router, &label, &filter_params(&[case.entry])).await;
        assert_eq!(
            reported,
            seeded.ids_at(&case.expected),
            "case {label:?}: expected the links {:?}, got {reported:?}",
            case.expected
        );
    }
}

/// One single-filter case: the filter it sends, and the links (by launch index) it must
/// select.
struct FilterCase {
    entry: FilterEntry,
    expected: Vec<usize>,
}

/// One single-filter case row.
fn filter_case(name: &'static str, values: &[&str], expected: &[usize]) -> FilterCase {
    FilterCase {
        entry: entry(name, values),
        expected: expected.to_vec(),
    }
}

/// Several filters are joined with AND: a link must satisfy every one of them.
#[tokio::test]
async fn several_filters_are_joined_with_and() {
    let (router, _engine, seeded) = emulator().await;
    let (vpc_a, vpc_b) = seeded.vpcs.clone();
    let group_a = seeded.groups[0].0.clone();

    let cases: Vec<AndCase> = vec![
        AndCase {
            case: "a VPC and a tag key",
            entries: vec![
                entry("vpc-id", &[vpc_a.as_str()]),
                entry("tag-key", &["Owner"]),
            ],
            expected: vec![0, 1],
        },
        AndCase {
            case: "a VPC and a group in it",
            entries: vec![
                entry("vpc-id", &[vpc_a.as_str()]),
                entry("group-id", &[group_a.as_str()]),
            ],
            expected: vec![0, 1],
        },
        AndCase {
            case: "a VPC and a group of the other VPC's links",
            entries: vec![
                entry("vpc-id", &[vpc_b.as_str()]),
                entry("group-id", &[group_a.as_str()]),
            ],
            expected: vec![4],
        },
        AndCase {
            case: "two filters nothing satisfies at once",
            entries: vec![
                entry("vpc-id", &[vpc_a.as_str()]),
                entry("instance-id", &[seeded.instances[2].as_str()]),
            ],
            expected: Vec::new(),
        },
    ];

    for case in cases {
        let label = case.case;
        let reported = described_ids(&router, label, &filter_params(&case.entries)).await;
        assert_eq!(
            reported,
            seeded.ids_at(&case.expected),
            "case {label:?}: expected the links {:?}, got {reported:?}",
            case.expected
        );
    }
}

/// One AND case: the filters it sends, and the links (by launch index) they must select
/// together.
struct AndCase {
    case: &'static str,
    entries: Vec<FilterEntry>,
    expected: Vec<usize>,
}

/// A filter name this operation does not document is refused — never silently ignored,
/// which would answer a wider set than the caller asked for.
#[tokio::test]
async fn an_undocumented_filter_name_is_refused() {
    let (router, _engine, _seeded) = emulator().await;

    // Rows: a name of another operation's catalog, the near-misses of the documented names
    // (wrong case, wrong separator, singular), the empty name, and the `tag` family without
    // a key — the doc says to "use the tag key in the filter name", so `tag` alone names no
    // tag.
    for name in [
        "group-name",
        "availability-zone",
        "VPC-ID",
        "vpc_id",
        "vpc",
        "instance-ids",
        "",
        "tag",
        "tag:",
    ] {
        let (status, body) = describe(&router, &filter_params(&[entry(name, &["any"])])).await;
        assert_eq!(
            status, 400,
            "case {name:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidParameterValue",
            "case {name:?}: expected InvalidParameterValue, got {body}"
        );
        assert!(
            error_message(&body).contains(&format!("The filter '{name}' is invalid")),
            "case {name:?}: the refusal must name the filter at fault, got {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// InstanceIds
// ---------------------------------------------------------------------------

/// `InstanceIds` narrows the report to the instances named; an id the account holds no
/// instance for is refused rather than answered as an absent row, and a value that is not
/// an instance id at all is refused as malformed first.
#[tokio::test]
async fn instance_ids_narrow_the_report_and_an_unresolvable_id_is_refused() {
    let (router, _engine, seeded) = emulator().await;

    let cases: Vec<IdCase> = vec![
        IdCase {
            case: "one linked id",
            ids: vec![seeded.instances[0].clone()],
            expected: vec![0],
        },
        IdCase {
            case: "several linked ids",
            ids: vec![
                seeded.instances[6].clone(),
                seeded.instances[1].clone(),
                seeded.instances[4].clone(),
            ],
            expected: vec![1, 4, 6],
        },
        IdCase {
            case: "a repeated id",
            ids: vec![seeded.instances[3].clone(), seeded.instances[3].clone()],
            expected: vec![3],
        },
        // The account holds this instance, but it is not linked: "This request only returns
        // information about EC2-Classic instances linked to a VPC through ClassicLink."
        IdCase {
            case: "a stored instance that is not linked",
            ids: vec![seeded.instances[7].clone()],
            expected: Vec::new(),
        },
        IdCase {
            case: "an unlinked id beside a linked one",
            ids: vec![seeded.instances[7].clone(), seeded.instances[2].clone()],
            expected: vec![2],
        },
    ];

    for case in cases {
        let label = case.case;
        let borrowed: Vec<&str> = case.ids.iter().map(String::as_str).collect();
        let reported = described_ids(&router, label, &indexed("InstanceId", &borrowed)).await;
        assert_eq!(
            reported,
            seeded.ids_at(&case.expected),
            "case {label:?}: expected the links {:?}, got {reported:?}",
            case.expected
        );
    }

    // An id of the right shape that names no instance of the account, and one that is not
    // an instance id at all.
    let refusals = [
        (
            "an unknown instance id",
            "i-00000000000000bee",
            "InvalidInstanceID.NotFound",
        ),
        (
            "an unknown id beside a linked one",
            "i-00000000000000bee",
            "InvalidInstanceID.NotFound",
        ),
        ("a malformed id", "i-123", "InvalidInstanceID.Malformed"),
        (
            "a value that is not an id",
            "not-an-instance",
            "InvalidInstanceID.Malformed",
        ),
    ];
    for (case, id, expected_code) in refusals {
        let mut ids = vec![id];
        if case == "an unknown id beside a linked one" {
            ids.push(seeded.instances[0].as_str());
        }
        let (status, body) = describe(&router, &indexed("InstanceId", &ids)).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {case:?}: expected {expected_code}, got {body}"
        );
    }
}

/// One `InstanceIds` case: the ids it sends, and the links (by launch index) it must
/// report.
struct IdCase {
    case: &'static str,
    ids: Vec<String>,
    expected: Vec<usize>,
}

// ---------------------------------------------------------------------------
// Pagination
// ---------------------------------------------------------------------------

/// Whatever the page size, walking the pages covers every linked instance exactly once, in
/// `InstanceId` order, and the last page carries no token.
#[tokio::test]
async fn paging_covers_every_link_exactly_once_for_each_page_size() {
    let (router, _engine, seeded) = emulator().await;
    let every = seeded.every_linked_id();

    // `DescribeClassicLinkInstancesMaxResults` is modelled `range 5..1000`, so the rows are
    // its published lower bound, a size that divides the stored set exactly, one that does
    // not, and the upper bound, which holds everything.
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
            let page = rows(&case, &body);
            assert!(
                page.len() <= usize::try_from(max_results).unwrap_or_default(),
                "case {case:?}: expected at most {max_results} rows in a page, got {}",
                page.len()
            );
            walked.extend(page.into_iter().map(|row| row.instance_id));
            pages += 1;
            assert!(
                pages <= every.len() + 1,
                "case {case:?}: paging must terminate, walked {walked:?}"
            );
            match element(&body, "nextToken") {
                Some(token) => {
                    params = max_results_param(max_results);
                    params.push(p("NextToken", token));
                }
                None => break,
            }
        }
        assert_eq!(
            walked, every,
            "case {case:?}: expected every linked instance exactly once in InstanceId order, \
             got {walked:?}"
        );
    }
}

/// A page is a cut of the FILTERED set, not of the whole store: `MaxResults` and a filter
/// compose.
#[tokio::test]
async fn a_page_is_cut_from_the_filtered_set() {
    let (router, _engine, seeded) = emulator().await;

    // Six of the seven links name one of these groups, so the model's minimum page size
    // cuts the filtered set into two pages.
    let selected = [
        seeded.groups[0].0.clone(),
        seeded.groups[1].0.clone(),
        seeded.groups[2].0.clone(),
        UNHELD_GROUP.to_string(),
    ];
    let page_params = || {
        let mut params = filter_params(&[FilterEntry {
            name: "group-id",
            values: selected.to_vec(),
        }]);
        params.extend(max_results_param(5));
        params
    };

    let (status, body) = describe(&router, &page_params()).await;
    assert_eq!(
        status, 200,
        "the filtered first page: expected HTTP 200, got {status} with body {body}"
    );
    let first: Vec<String> = rows("the filtered first page", &body)
        .into_iter()
        .map(|row| row.instance_id)
        .collect();
    assert_eq!(
        first,
        seeded.ids_at(&[0, 1, 2, 4, 5]),
        "the filtered first page: expected the first five matching links, got {first:?}"
    );
    let token = element(&body, "nextToken")
        .expect("the filtered first page: a sixth link follows, so a token must be issued");

    let mut next = page_params();
    next.push(p("NextToken", token));
    let second = described_ids(&router, "the filtered second page", &next).await;
    assert_eq!(
        second,
        seeded.ids_at(&[6]),
        "the filtered second page: expected the remaining matching link, got {second:?}"
    );
}

/// `MaxResults` outside the range the model publishes is refused at the boundary; each
/// published bound itself is accepted.
#[tokio::test]
async fn max_results_is_accepted_at_its_bounds_and_refused_past_them() {
    let (router, _engine, _seeded) = emulator().await;

    // The published bounds (`DescribeClassicLinkInstancesMaxResults`, `range 5..1000`) and
    // one past each.
    for (max_results, expected_status) in [(4, 400), (5, 200), (1000, 200), (1001, 400)] {
        let (status, body) = describe(&router, &max_results_param(max_results)).await;
        assert_eq!(
            status, expected_status,
            "case MaxResults={max_results}: expected HTTP {expected_status}, got {status} with \
             body {body}"
        );
    }
}

/// A `NextToken` this service did not issue is refused with `InvalidPaginationToken` rather
/// than read as a cursor.
#[tokio::test]
async fn a_next_token_this_service_did_not_issue_is_refused() {
    let (router, _engine, seeded) = emulator().await;
    let first = seeded.every_linked_id()[0].clone();

    // Rows: free text, punctuation, a hex string this service never minted, the bare
    // instance id a caller might guess the token is, that id hex-encoded, and a forged
    // token carrying the right marker with the wrong checksum.
    let cases = [
        "not-a-token".to_string(),
        "###".to_string(),
        "6161".to_string(),
        first.clone(),
        ::hex::encode(&first),
        ::hex::encode(format!("ec2-1:0000000000000000:{first}")),
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
        assert_eq!(
            error_message(&body),
            ec2::state::INVALID_PAGINATION_TOKEN_MESSAGE,
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
    let (router, _engine, seeded) = emulator().await;

    let (status, body) = describe(&router, &[p("DryRun", "true")]).await;
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
        rows("DryRun=true", &body).is_empty(),
        "DryRun=true: a dry run reports no linked instance, got {body}"
    );

    // A dry run that WOULD have been refused is refused with that reason instead: the
    // unresolvable id is answered before the flag is read.
    let mut refused = indexed("InstanceId", &["i-00000000000000bee"]);
    refused.push(p("DryRun", "true"));
    let (status, body) = describe(&router, &refused).await;
    assert_eq!(
        status, 400,
        "DryRun with an unknown id: expected HTTP 400, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidInstanceID.NotFound",
        "DryRun with an unknown id: expected InvalidInstanceID.NotFound, got {body}"
    );

    let reported = described_ids(&router, "DryRun=false", &[p("DryRun", "false")]).await;
    assert_eq!(
        reported,
        seeded.every_linked_id(),
        "DryRun=false: expected every stored link, got {reported:?}"
    );
}
