// SPDX-License-Identifier: CC0-1.0

//! The ONE answer to "which VPC security groups does this account have", for every
//! ElastiCache operation that attaches a resource to them.
//!
//! A VPC security group is an Amazon EC2 resource, so every operation of this service that
//! takes a `SecurityGroupIds` — `CreateCacheCluster`, `CreateReplicationGroup`,
//! `CreateServerlessCache` and the three modify operations that change which groups guard an
//! existing resource — resolves each identifier it is given against what EC2 holds for the
//! account in the Region the request is signed for. An identifier EC2 reports a group for is
//! accepted and attached to the stored resource; one it reports none for — never minted,
//! minted and then deleted, or a filter pattern rather than an identifier — is refused
//! `InvalidParameterValue`, the refusal names the identifiers that did not resolve and the
//! account the request was made for, and nothing is written: a refused create leaves no
//! resource and a refused modify leaves the groups the resource already had.
//!
//! The second half of the same resolution is WHICH VPC each group EC2 answered for guards: a
//! security group only ever guards network interfaces of its own VPC, so a request naming a
//! group of another VPC than the cache subnet group the resource is placed in is refused
//! `InvalidParameterCombination` naming the subnet group and both VPCs, and again nothing is
//! written.
//!
//! The rows differ along what the rule generalises over: how many identifiers the request
//! names, whether one is held or absent and in which position, the form the absent one takes
//! (both the short and the long id form AWS has used, and a filter pattern), whether an
//! absent identifier was never minted or was minted and deleted, and which VPC a group that
//! IS the account's belongs to. The operations are the other axis, so no single recorded
//! request is what makes this suite pass.
//!
//! EC2 is mounted beside ElastiCache on ONE engine by [`crate::harness`] and driven through
//! the AWS SDK over the same in-process callback this service's own resolution reaches it
//! through. Every identifier a row holds was MINTED by EC2 or is deliberately absent, so no
//! id written here can pass itself off as one the account holds.

use service_com_amazonaws_elasticache_amazonelasticachev9::state;

use crate::cache_cluster_lifecycle::subnet_group_params;
use crate::harness::{params, poll_until, Emulator, Reply, PAST_TRANSITION, REGION};

/// The `awsQuery` error code a refusal for an identifier that resolves to nothing carries,
/// from the model: `InvalidParameterValueException` has `awsQueryError` code
/// `InvalidParameterValue` with HTTP 400
/// (`smithy_query(query="operation",operation="CreateReplicationGroup")`).
const REFUSAL_CODE: &str = "InvalidParameterValue";

/// The `awsQuery` error code a refusal for members that cannot hold together carries, from
/// the same model entry: `InvalidParameterCombinationException` has `awsQueryError` code
/// `InvalidParameterCombination` with HTTP 400.
const COMBINATION_REFUSAL_CODE: &str = "InvalidParameterCombination";

/// Identifiers of the EC2 security-group form that EC2 mints no group for, so each reaches
/// the handler's resolution and resolves to nothing. Both the short and the long form AWS
/// has used for a security-group id appear, and the third is a filter PATTERN rather than an
/// identifier: the owner matches patterns in a filter value, so a request that sends one
/// must still be refused.
const ABSENT_GROUP: &str = "sg-00000000deadbeef";
const SHORT_ABSENT_GROUP: &str = "sg-0000beef";
const PATTERN_NOT_AN_ID: &str = "sg-*";

/// How a row names one security group.
#[derive(Debug, Clone, Copy)]
enum GroupRef {
    /// The `index`-th group EC2 minted for this row's emulator.
    Held(usize),
    /// The group EC2 minted and then deleted: an id the account HELD and holds no longer,
    /// which is a different thing from one it never had.
    Deleted,
    /// A literal the account has no group for.
    Absent(&'static str),
}

/// One row: the identifiers the request names, and the ones its refusal must name — empty
/// when every identifier resolves and the request must be accepted.
#[derive(Debug, Clone, Copy)]
struct GroupCase {
    case: &'static str,
    requested: &'static [GroupRef],
    unrecognized: &'static [GroupRef],
}

/// The axis: how many identifiers are named, which of them the account holds, where the
/// absent one sits, and what kind of absent value it is.
const CASES: &[GroupCase] = &[
    GroupCase {
        case: "one group the account holds",
        requested: &[GroupRef::Held(0)],
        unrecognized: &[],
    },
    GroupCase {
        case: "two groups the account holds, in the order named",
        requested: &[GroupRef::Held(1), GroupRef::Held(0)],
        unrecognized: &[],
    },
    GroupCase {
        case: "one group the account holds, named twice",
        requested: &[GroupRef::Held(0), GroupRef::Held(0)],
        unrecognized: &[],
    },
    GroupCase {
        case: "an id the account never had",
        requested: &[GroupRef::Absent(ABSENT_GROUP)],
        unrecognized: &[GroupRef::Absent(ABSENT_GROUP)],
    },
    GroupCase {
        case: "an id in the short form the account never had",
        requested: &[GroupRef::Absent(SHORT_ABSENT_GROUP)],
        unrecognized: &[GroupRef::Absent(SHORT_ABSENT_GROUP)],
    },
    GroupCase {
        case: "an id the account held and deleted",
        requested: &[GroupRef::Deleted],
        unrecognized: &[GroupRef::Deleted],
    },
    GroupCase {
        case: "a held group followed by an absent id",
        requested: &[GroupRef::Held(0), GroupRef::Absent(ABSENT_GROUP)],
        unrecognized: &[GroupRef::Absent(ABSENT_GROUP)],
    },
    GroupCase {
        case: "an absent id followed by a held group",
        requested: &[GroupRef::Absent(ABSENT_GROUP), GroupRef::Held(1)],
        unrecognized: &[GroupRef::Absent(ABSENT_GROUP)],
    },
    GroupCase {
        case: "two absent ids, both named in the refusal",
        requested: &[
            GroupRef::Absent(ABSENT_GROUP),
            GroupRef::Absent(SHORT_ABSENT_GROUP),
        ],
        unrecognized: &[
            GroupRef::Absent(ABSENT_GROUP),
            GroupRef::Absent(SHORT_ABSENT_GROUP),
        ],
    },
    GroupCase {
        case: "a filter pattern instead of an id",
        requested: &[GroupRef::Absent(PATTERN_NOT_AN_ID)],
        unrecognized: &[GroupRef::Absent(PATTERN_NOT_AN_ID)],
    },
];

/// The resource an operation attaches the groups to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Target {
    Cluster,
    ReplicationGroup,
    ServerlessCache,
}

impl Target {
    /// A prefix for the identifiers and group names one run of this suite mints, so two
    /// runs over one emulator cannot collide.
    fn label(self) -> &'static str {
        match self {
            Self::Cluster => "sgc",
            Self::ReplicationGroup => "sgr",
            Self::ServerlessCache => "sgs",
        }
    }

    /// The create operation that attaches the groups.
    fn create(self) -> &'static str {
        match self {
            Self::Cluster => "CreateCacheCluster",
            Self::ReplicationGroup => "CreateReplicationGroup",
            Self::ServerlessCache => "CreateServerlessCache",
        }
    }

    /// The modify operation that replaces them.
    fn modify(self) -> &'static str {
        match self {
            Self::Cluster => "ModifyCacheCluster",
            Self::ReplicationGroup => "ModifyReplicationGroup",
            Self::ServerlessCache => "ModifyServerlessCache",
        }
    }

    /// The form pairs that identify the resource, plus whatever else a create of it needs.
    fn identity(self, name: &str) -> Vec<(String, String)> {
        match self {
            // A create that provisions its own nodes names the node type they
            // run on, which this suite is not about.
            Self::Cluster => crate::harness::cluster_create_params(name, &[]),
            Self::ReplicationGroup => params(&[
                ("ReplicationGroupId", name),
                (
                    "ReplicationGroupDescription",
                    "the security-group resolution suite",
                ),
            ]),
            Self::ServerlessCache => {
                params(&[("ServerlessCacheName", name), ("Engine", "redis")])
            }
        }
    }
}

/// The groups the account HAS for one run: two EC2 minted and kept, and one it minted and
/// then deleted.
struct Groups {
    held: Vec<String>,
    deleted: String,
}

/// Mint the account's groups for `target` through EC2's own API.
async fn account_groups(emulator: &Emulator, target: Target) -> Groups {
    let held = emulator.security_groups(target.label(), 2).await;
    let deleted = emulator
        .security_group_named(&format!("{}-deleted", target.label()))
        .await;
    emulator.delete_security_group(&deleted).await;
    Groups { held, deleted }
}

/// The ids `refs` name, as the request will send them.
fn resolve(refs: &[GroupRef], groups: &Groups) -> Vec<String> {
    refs.iter()
        .map(|group| match group {
            GroupRef::Held(index) => groups.held[*index].clone(),
            GroupRef::Deleted => groups.deleted.clone(),
            GroupRef::Absent(literal) => (*literal).to_string(),
        })
        .collect()
}

/// `SecurityGroupIds` as the `awsQuery` wire carries the list.
fn group_pairs(ids: &[String]) -> Vec<(String, String)> {
    ids.iter()
        .enumerate()
        .map(|(index, id)| {
            (
                format!("SecurityGroupIds.SecurityGroupId.{}", index + 1),
                id.clone(),
            )
        })
        .collect()
}

/// Send `action` for `target`'s resource `name` with the group ids `ids`.
async fn call(
    emulator: &Emulator,
    target: Target,
    action: &str,
    name: &str,
    ids: &[String],
) -> Reply {
    let mut pairs = if action == target.create() {
        target.identity(name)
    } else {
        match target {
            Target::Cluster => params(&[("CacheClusterId", name)]),
            Target::ReplicationGroup => params(&[("ReplicationGroupId", name)]),
            Target::ServerlessCache => params(&[("ServerlessCacheName", name)]),
        }
    };
    pairs.extend(group_pairs(ids));
    emulator.call(action, &pairs).await
}

/// The security groups the stored resource reports, read back through the describe that
/// reports them: a cluster and a replication group's member clusters report a
/// `SecurityGroupMembership` list, a serverless cache reports the id list itself.
async fn attached_groups(emulator: &Emulator, target: Target, name: &str) -> Vec<String> {
    let read = match target {
        Target::Cluster => {
            emulator
                .call("DescribeCacheClusters", &params(&[("CacheClusterId", name)]))
                .await
        }
        Target::ReplicationGroup => {
            emulator
                .call(
                    "DescribeCacheClusters",
                    &params(&[("CacheClusterId", &format!("{name}-001"))]),
                )
                .await
        }
        Target::ServerlessCache => {
            emulator
                .call(
                    "DescribeServerlessCaches",
                    &params(&[("ServerlessCacheName", name)]),
                )
                .await
        }
    };
    assert_eq!(
        read.status, 200,
        "the read of {name} must answer 200, got {} {}",
        read.status, read.body
    );
    let element = match target {
        Target::ServerlessCache => "SecurityGroupIds/SecurityGroupId",
        _ => "SecurityGroups/member/SecurityGroupId",
    };
    read.all(element)
        .into_iter()
        .map(str::to_string)
        .collect()
}

/// Assert the resource `name` does not exist at all: the describe that resolves it answers
/// its modelled not-found fault, with the status, the `awsQuery` code and — for the
/// replication group, whose template the recorded refusal pins — the sentence AWS words it
/// with.
async fn assert_absent(case: &str, emulator: &Emulator, target: Target, name: &str) {
    let (action, pairs, status, code, message) = match target {
        Target::Cluster => (
            "DescribeCacheClusters",
            params(&[("CacheClusterId", name)]),
            404,
            "CacheClusterNotFound",
            None,
        ),
        Target::ReplicationGroup => (
            "DescribeReplicationGroups",
            params(&[("ReplicationGroupId", name)]),
            404,
            "ReplicationGroupNotFoundFault",
            Some(format!("ReplicationGroup {name} not found.")),
        ),
        Target::ServerlessCache => (
            "DescribeServerlessCaches",
            params(&[("ServerlessCacheName", name)]),
            404,
            "ServerlessCacheNotFoundFault",
            None,
        ),
    };
    let read = emulator.call(action, &pairs).await;
    assert_eq!(
        (read.status, read.error_code()),
        (status, code),
        "case {case}: {action} for {name} must answer {status} {code}, got {} {}",
        read.status,
        read.body
    );
    if let Some(message) = message {
        assert_eq!(
            read.error_message(),
            message,
            "case {case}: {action} for {name} must answer {message:?}, got {:?}",
            read.error_message()
        );
    }
}

/// The sentence a request naming security groups the account does not have is refused with.
///
/// Written out here rather than read from the service: the fixed text is the requirement,
/// and the variable parts are the identifiers that did not resolve and the account the
/// request was made for.
fn expected_message(unresolved: &[String]) -> String {
    format!(
        "Some security group Id not recognized by EC2: securityGroupIds[[{}]], awsAccountId[{}]",
        unresolved.join(", "),
        engine::emulator_account_id()
    )
}

/// Assert `reply` is the modelled refusal naming exactly `unresolved`.
fn assert_refused(case: &str, operation: &str, reply: &Reply, unresolved: &[String]) {
    assert_eq!(
        reply.status, 400,
        "case {case}: {operation} must answer HTTP 400, got {} {}",
        reply.status, reply.body
    );
    assert_eq!(
        reply.error_code(),
        REFUSAL_CODE,
        "case {case}: {operation} must answer {REFUSAL_CODE}, got {:?} ({})",
        reply.error_code(),
        reply.body
    );
    let expected = expected_message(unresolved);
    assert_eq!(
        reply.error_message(),
        expected,
        "case {case}: {operation} must answer {expected:?}, got {:?}",
        reply.error_message()
    );
}

/// Every row against the CREATE of `target`: an accepted create attaches exactly the groups
/// the request named, and a refused one leaves no resource behind.
async fn create_resolves_groups(target: Target) {
    let emulator = Emulator::new();
    let groups = account_groups(&emulator, target).await;
    let operation = target.create();

    for (index, case) in CASES.iter().enumerate() {
        let name = format!("{}-create-{index}", target.label());
        let requested = resolve(case.requested, &groups);
        let unresolved = resolve(case.unrecognized, &groups);
        let reply = call(&emulator, target, operation, &name, &requested).await;

        if unresolved.is_empty() {
            assert_eq!(
                reply.status, 200,
                "case {}: {operation} must answer 200, got {} {}",
                case.case, reply.status, reply.body
            );
            let attached = attached_groups(&emulator, target, &name).await;
            assert_eq!(
                attached, requested,
                "case {}: {operation} must attach {requested:?}, got {attached:?}",
                case.case
            );
        } else {
            assert_refused(case.case, operation, &reply, &unresolved);
            // Nothing was written: the resource the refused create named does not exist, as
            // the describe that resolves it says itself.
            assert_absent(case.case, &emulator, target, &name).await;
        }
    }
}

#[tokio::test]
async fn a_long_security_group_list_is_resolved_the_same_way_as_a_short_one() {
    // The count axis past the point where one question by identifier stops being the cheap
    // way to ask: a request may name as many groups as it likes, and the rule is the same
    // one — every identifier resolves against what the account HAS, or the request is
    // refused naming the ones that do not.
    let emulator = Emulator::new();
    let held = emulator.security_groups("bulk", 70).await;
    let accepted = call(
        &emulator,
        Target::ServerlessCache,
        "CreateServerlessCache",
        "bulk-groups-cache",
        &held,
    )
    .await;
    assert_eq!(
        accepted.status, 200,
        "a create naming {} groups the account holds must answer 200, got {} {}",
        held.len(),
        accepted.status,
        accepted.body
    );
    let attached = attached_groups(&emulator, Target::ServerlessCache, "bulk-groups-cache").await;
    assert_eq!(
        attached, held,
        "every group the request named must be attached, got {attached:?}"
    );

    // The same long list with ONE id the account does not have: refused, naming exactly
    // that id.
    let mut mixed = held.clone();
    mixed.insert(40, ABSENT_GROUP.to_string());
    let refused = call(
        &emulator,
        Target::ServerlessCache,
        "CreateServerlessCache",
        "bulk-groups-refused",
        &mixed,
    )
    .await;
    assert_refused(
        "a long list carrying one absent id",
        "CreateServerlessCache",
        &refused,
        &[ABSENT_GROUP.to_string()],
    );
    assert_absent(
        "a long list carrying one absent id",
        &emulator,
        Target::ServerlessCache,
        "bulk-groups-refused",
    )
    .await;
}

/// Every row against the MODIFY of `target`: an accepted modify replaces the group list
/// wholesale, and a refused one leaves the groups the resource already had.
async fn modify_resolves_groups(target: Target) {
    assert!(
        PAST_TRANSITION > state::CLUSTER_TRANSITION
            && PAST_TRANSITION > state::SERVERLESS_CACHE_TRANSITION,
        "these tests wait {PAST_TRANSITION:?}, which must out-last the spans this service \
         publishes ({:?}, {:?})",
        state::CLUSTER_TRANSITION,
        state::SERVERLESS_CACHE_TRANSITION
    );
    let emulator = Emulator::new();
    let groups = account_groups(&emulator, target).await;
    let operation = target.modify();
    // Each row gets its own resource, attached to the first group the account holds, so a
    // refused modify can be shown to have changed nothing.
    let before = vec![groups.held[0].clone()];
    for index in 0..CASES.len() {
        let name = format!("{}-modify-{index}", target.label());
        let created = call(&emulator, target, target.create(), &name, &before).await;
        assert_eq!(
            created.status, 200,
            "the {operation} row {index} needs its resource, got {} {}",
            created.status, created.body
        );
    }
    // A resource takes a change only once the clock owes it its usable status.
    std::thread::sleep(PAST_TRANSITION);

    for (index, case) in CASES.iter().enumerate() {
        let name = format!("{}-modify-{index}", target.label());
        let requested = resolve(case.requested, &groups);
        let unresolved = resolve(case.unrecognized, &groups);
        let reply = call(&emulator, target, operation, &name, &requested).await;

        if unresolved.is_empty() {
            assert_eq!(
                reply.status, 200,
                "case {}: {operation} must answer 200, got {} {}",
                case.case, reply.status, reply.body
            );
            let attached = attached_groups(&emulator, target, &name).await;
            assert_eq!(
                attached, requested,
                "case {}: {operation} must leave {requested:?} attached, got {attached:?}",
                case.case
            );
        } else {
            assert_refused(case.case, operation, &reply, &unresolved);
            let attached = attached_groups(&emulator, target, &name).await;
            assert_eq!(
                attached, before,
                "case {}: a refused {operation} must leave {before:?} attached, got {attached:?}",
                case.case
            );
        }
    }
}

#[tokio::test]
async fn create_cache_cluster_resolves_security_groups_against_ec2() {
    create_resolves_groups(Target::Cluster).await;
}

#[tokio::test]
async fn create_replication_group_resolves_security_groups_against_ec2() {
    create_resolves_groups(Target::ReplicationGroup).await;
}

#[tokio::test]
async fn create_serverless_cache_resolves_security_groups_against_ec2() {
    create_resolves_groups(Target::ServerlessCache).await;
}

#[tokio::test]
async fn modify_cache_cluster_resolves_security_groups_against_ec2() {
    modify_resolves_groups(Target::Cluster).await;
}

#[tokio::test]
async fn modify_replication_group_resolves_security_groups_against_ec2() {
    modify_resolves_groups(Target::ReplicationGroup).await;
}

#[tokio::test]
async fn modify_serverless_cache_resolves_security_groups_against_ec2() {
    modify_resolves_groups(Target::ServerlessCache).await;
}

#[tokio::test]
async fn a_group_the_account_minted_after_a_refusal_is_accepted_at_once() {
    // Nothing is cached: the identifier that was refused a moment ago is accepted as soon
    // as the owner of security groups holds a group for it, and refused again once the
    // group is deleted — so the answer comes from EC2 on every call rather than from a
    // decision this service remembers.
    let emulator = Emulator::new();
    let refused = call(
        &emulator,
        Target::ServerlessCache,
        "CreateServerlessCache",
        "late-group-cache",
        &[ABSENT_GROUP.to_string()],
    )
    .await;
    assert_refused(
        "before the group exists",
        "CreateServerlessCache",
        &refused,
        &[ABSENT_GROUP.to_string()],
    );

    let minted = emulator.security_group_named("late-group").await;
    let only_minted = std::slice::from_ref(&minted);
    let accepted = call(
        &emulator,
        Target::ServerlessCache,
        "CreateServerlessCache",
        "late-group-cache",
        only_minted,
    )
    .await;
    assert_eq!(
        accepted.status, 200,
        "once EC2 holds the group the create must answer 200, got {} {}",
        accepted.status, accepted.body
    );
    std::thread::sleep(PAST_TRANSITION);

    emulator.delete_security_group(&minted).await;
    let refused_again = call(
        &emulator,
        Target::ServerlessCache,
        "ModifyServerlessCache",
        "late-group-cache",
        only_minted,
    )
    .await;
    assert_refused(
        "after the group is deleted",
        "ModifyServerlessCache",
        &refused_again,
        only_minted,
    );
    // The cache still reports the group it was created with: the refusal changed nothing.
    let attached = attached_groups(&emulator, Target::ServerlessCache, "late-group-cache").await;
    assert_eq!(
        attached, only_minted,
        "a refused modify must leave the stored group list alone, got {attached:?}"
    );
}

/// One VPC of the account: its id, two subnets of it, and a security group of its own — each
/// minted by Amazon EC2, the service that owns all three.
struct AccountVpc {
    id: String,
    subnets: Vec<String>,
    group: String,
}

/// Build one VPC with two subnets and one security group through EC2's own API.
///
/// `label` keeps one VPC's names apart from the other's, and the zones are written as
/// `<region><letter>`, the form AWS names a zone of a Region in.
async fn account_vpc(emulator: &Emulator, label: &str, cidr: &str) -> AccountVpc {
    let ec2 = emulator.ec2(REGION);
    let id = ec2
        .create_vpc()
        .cidr_block(cidr)
        .send()
        .await
        .expect("EC2 must create the VPC")
        .vpc()
        .and_then(::aws_sdk_ec2::types::Vpc::vpc_id)
        .expect("CreateVpc must report the VPC it minted")
        .to_string();
    let mut subnets = Vec::new();
    for (index, letter) in ["a", "b"].iter().enumerate() {
        let subnet_cidr = format!(
            "{}.{}.0/24",
            cidr.trim_end_matches("/16").trim_end_matches(".0.0"),
            index
        );
        subnets.push(
            ec2.create_subnet()
                .vpc_id(&id)
                .cidr_block(&subnet_cidr)
                .availability_zone(format!("{REGION}{letter}"))
                .send()
                .await
                .expect("EC2 must create the subnet")
                .subnet()
                .and_then(::aws_sdk_ec2::types::Subnet::subnet_id)
                .expect("CreateSubnet must report the subnet it minted")
                .to_string(),
        );
    }
    let group = ec2
        .create_security_group()
        .group_name(format!("{label}-group"))
        .description(format!("the group of the {label} VPC"))
        .vpc_id(&id)
        .send()
        .await
        .expect("EC2 must create the security group")
        .group_id()
        .expect("CreateSecurityGroup must report the group id it minted")
        .to_string();
    AccountVpc {
        id,
        subnets,
        group,
    }
}

/// The sentence a request whose security groups are in another VPC than its placement is
/// refused with.
///
/// Written out here rather than read from the service: the fixed text is the requirement,
/// and the variable parts are the subnet group the resource is placed in, that group's VPC
/// and the group's VPC.
fn expected_vpc_mismatch(subnet_group: &str, placement_vpc: &str, group_vpc: &str) -> String {
    format!(
        "Subnet group [{subnet_group}] belongs to a different VPC [{placement_vpc}] than \
         [{group_vpc}]"
    )
}

#[tokio::test]
async fn a_security_group_of_another_vpc_than_the_placement_is_refused() {
    // The axis: WHICH VPC a group the account holds belongs to, over every operation that
    // takes both a placement and a security group list. The control row of each pair is the
    // same request with the placement's own group, so a refusal that fired for any other
    // reason fails here too.
    let emulator = Emulator::new();
    let here = account_vpc(&emulator, "here", "10.20.0.0/16").await;
    let there = account_vpc(&emulator, "there", "10.30.0.0/16").await;
    let subnet_group = "placed-in-here";
    let created_group = emulator
        .call(
            "CreateCacheSubnetGroup",
            &subnet_group_params(subnet_group, "the placement of this suite", &here.subnets),
        )
        .await;
    assert_eq!(
        created_group.status, 200,
        "CreateCacheSubnetGroup must answer 200, got {} {}",
        created_group.status, created_group.body
    );

    let expected = expected_vpc_mismatch(subnet_group, &here.id, &there.id);

    // The two creates: the request names the placement itself.
    for (operation, identity) in [
        (
            "CreateCacheCluster",
            crate::harness::cluster_create_params(
                "vpc-mismatch-cluster",
                &[("CacheSubnetGroupName", subnet_group)],
            ),
        ),
        (
            "CreateReplicationGroup",
            params(&[
                ("ReplicationGroupId", "vpc-mismatch-rg"),
                ("ReplicationGroupDescription", "the VPC mismatch rule"),
                ("CacheSubnetGroupName", subnet_group),
            ]),
        ),
    ] {
        let mut refused_request = identity.clone();
        refused_request.extend(group_pairs(std::slice::from_ref(&there.group)));
        let refused = emulator.call(operation, &refused_request).await;
        assert_eq!(
            (refused.status, refused.error_code()),
            (400, COMBINATION_REFUSAL_CODE),
            "case {operation}: a group of another VPC must be refused 400 \
             {COMBINATION_REFUSAL_CODE}, got {} {}",
            refused.status,
            refused.body
        );
        assert_eq!(
            refused.error_message(),
            expected,
            "case {operation}: expected {expected:?}, got {:?}",
            refused.error_message()
        );

        // The control: the placement's own group is accepted.
        let mut accepted_request = identity;
        accepted_request.extend(group_pairs(std::slice::from_ref(&here.group)));
        let accepted = emulator.call(operation, &accepted_request).await;
        assert_eq!(
            accepted.status, 200,
            "case {operation}: the placement's own group must be accepted, got {} {}",
            accepted.status, accepted.body
        );
    }

    // The two modifies: the placement is the one the resource was created in, so the rule
    // holds for a change too. Both resources above were created with the placement's own
    // group; a resource takes no change until the clock owes it its usable status, which is
    // reached by looking again at each one until it reports that status rather than by
    // waiting a fixed time.
    poll_until(
        &emulator,
        REGION,
        "the VPC mismatch rule",
        "the cluster vpc-mismatch-cluster settling at available",
        "DescribeCacheClusters",
        &params(&[("CacheClusterId", "vpc-mismatch-cluster")]),
        |reply| reply.text("CacheCluster/CacheClusterStatus") == Some(state::STATUS_AVAILABLE),
    )
    .await;
    poll_until(
        &emulator,
        REGION,
        "the VPC mismatch rule",
        "the replication group vpc-mismatch-rg settling at available",
        "DescribeReplicationGroups",
        &params(&[("ReplicationGroupId", "vpc-mismatch-rg")]),
        |reply| reply.text("ReplicationGroup/Status") == Some(state::STATUS_AVAILABLE),
    )
    .await;
    for (operation, identity, target, name) in [
        (
            "ModifyCacheCluster",
            params(&[("CacheClusterId", "vpc-mismatch-cluster")]),
            Target::Cluster,
            "vpc-mismatch-cluster",
        ),
        (
            "ModifyReplicationGroup",
            params(&[("ReplicationGroupId", "vpc-mismatch-rg")]),
            Target::ReplicationGroup,
            "vpc-mismatch-rg",
        ),
    ] {
        let mut request = identity;
        request.extend(group_pairs(std::slice::from_ref(&there.group)));
        let refused = emulator.call(operation, &request).await;
        assert_eq!(
            (refused.status, refused.error_code()),
            (400, COMBINATION_REFUSAL_CODE),
            "case {operation}: a group of another VPC must be refused 400 \
             {COMBINATION_REFUSAL_CODE}, got {} {}",
            refused.status,
            refused.body
        );
        assert_eq!(
            refused.error_message(),
            expected,
            "case {operation}: expected {expected:?}, got {:?}",
            refused.error_message()
        );
        // Nothing was written: the resource still reports the group it had.
        let attached = attached_groups(&emulator, target, name).await;
        assert_eq!(
            attached,
            vec![here.group.clone()],
            "case {operation}: a refused modify must leave the stored group list alone, got \
             {attached:?}"
        );
    }
}
