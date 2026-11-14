// SPDX-License-Identifier: CC0-1.0

//! The PRE-CONDITIONS a mutating EC2 call is held to, and what a delete leaves readable —
//! driven over `ec2Query` (README.md / OPERATIONS.md "Wire protocol").
//!
//! Two rules, both about state the request did not send:
//!
//! 1. before a create, a launch or a delete takes effect, the resources the request NAMES
//!    have to admit it — a referenced resource has to exist and to be compatible with the
//!    other resources named, and a resource something still depends on cannot be deleted — and
//!    a request that fails one of those is refused HTTP 400 with EC2's own code and sentence
//!    for that class, with nothing written;
//! 2. once a delete has taken effect the resource stops being readable: a describe BY ID
//!    answers that resource's own `.NotFound`, and an unfiltered describe no longer carries
//!    it.
//!
//! The rows of each table differ along what the rule generalises over: which Region a locale
//! names, which family and how many CIDRs a pool holds, which KIND of resource occupies the
//! one being deleted, which side of a cross-VPC reference the request spells, and how much of
//! the delete's own span the clock has run.
//!
//! The same two rules are asserted of other resources by
//! [`super::deleted_resources_and_dependencies`] (a VPC's, a security group's and a route
//! table's dependencies, and the reads of those resources once they are gone); this suite is
//! the IPAM, launch, prefix-list, endpoint and transit gateway half of them. Each suite of this
//! crate posts through the shared [`super::ec2_query`] harness and keeps only its own
//! per-suite `call`/`ok`/`member` wrappers, which is how every other suite here is arranged.

use std::time::{Duration, SystemTime};

use ec2::ops::types::{IPAM_POOL_CIDR_STATE_VALUES, NatGateway, Tag};
use ec2::state::{
    ADDRESS_FAMILY_IPV4, ADDRESS_FAMILY_IPV6, ComAmazonawsEc2Amazonec2State as Ec2State,
    DEPENDENCY_VIOLATION, INCORRECT_STATE, INVALID_GROUP_NOT_FOUND, INVALID_IPAM_ID_NOT_FOUND,
    INVALID_PARAMETER, INVALID_PREFIX_LIST_ID_NOT_FOUND, INVALID_TRANSIT_GATEWAY_ID_NOT_FOUND,
    INVALID_VPC_ENDPOINT_ID_NOT_FOUND, IPAM_POOL_CIDR_STATE_PENDING_PROVISION,
    IPAM_POOL_CIDR_STATE_PROVISIONED, NAT_GATEWAY_STATE_AVAILABLE, NAT_GATEWAY_STATE_DELETED,
    NAT_GATEWAY_STATE_DELETING, TRANSITION_DELAY, system_time_to_date_time,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{element, elements, emulator, error_code, error_message};
use super::wire::{Param, param};

/// The Region every call here is signed for.
const REGION: &str = "us-east-1";

/// Another Region of the same partition, used as a locale an IPAM may or may not operate in.
const OTHER_REGION: &str = "us-west-2";

/// A well-formed AMI id. `RunInstances` only reads an image's own attributes when the account
/// holds one under the id, so a launch of this suite is about its NETWORK rather than its
/// image.
const AMI: &str = "ami-0123456789abcdef0";

/// Long enough past [`TRANSITION_DELAY`] that a move or a removal the clock owes is certainly
/// due. Derived from the service's own span, not a literal, so the two cannot drift.
async fn past_the_transition() {
    tokio::time::sleep(TRANSITION_DELAY + Duration::from_millis(250)).await;
}

/// One `action` call carrying `params`, as its HTTP status and XML body.
async fn call(router: &MultiServiceRouter, action: &str, params: &[Param]) -> (u16, String) {
    super::ec2_query::call(router, REGION, action, params).await
}

/// One `action` call that has to succeed, and the body it answered.
async fn ok(case: &str, router: &MultiServiceRouter, action: &str, params: &[Param]) -> String {
    let (status, body) = call(router, action, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from {action}, got {status} with body {body}"
    );
    body
}

/// One element of a response body, or a panic naming the case and showing the body.
fn member(case: &str, body: &str, name: &str) -> String {
    element(body, name).unwrap_or_else(|| panic!("case {case:?}: no <{name}> in {body}"))
}

/// Assert that `action` was refused HTTP 400 with `code` and exactly `message`.
async fn refused(
    case: &str,
    router: &MultiServiceRouter,
    action: &str,
    params: &[Param],
    code: &str,
    message: &str,
) {
    let (status, body) = call(router, action, params).await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400 from {action}, got {status} with body {body}"
    );
    let actual_code = error_code(&body);
    assert_eq!(
        actual_code, code,
        "case {case:?}: expected the code {code:?} from {action}, got {actual_code:?} in {body}"
    );
    let actual_message = error_message(&body);
    assert_eq!(
        actual_message, message,
        "case {case:?}: expected the message {message:?}, got {actual_message:?}"
    );
}

/// One IPAM created over the wire, with the Regions it operates in, and the default private
/// scope a pool is created in.
struct Ipam {
    id: String,
    private_scope: String,
}

/// Create one IPAM operating in `regions` beside the Region the call is signed for.
async fn create_ipam(case: &str, router: &MultiServiceRouter, regions: &[&str]) -> Ipam {
    let params: Vec<Param> = regions
        .iter()
        .enumerate()
        .map(|(index, region)| {
            param(&format!("OperatingRegion.{}.RegionName", index + 1), region)
        })
        .collect();
    let body = ok(case, router, "CreateIpam", &params).await;
    Ipam {
        id: member(case, &body, "ipamId"),
        private_scope: member(case, &body, "privateDefaultScopeId"),
    }
}

/// Create one IPAM pool of `family` in `scope_id`, with `extra` beside it, and answer the
/// `IpamPoolId` the service minted.
async fn create_pool(
    case: &str,
    router: &MultiServiceRouter,
    scope_id: &str,
    family: &str,
    extra: &[Param],
) -> String {
    let mut params = vec![
        param("IpamScopeId", scope_id),
        param("AddressFamily", family),
    ];
    params.extend(extra.iter().cloned());
    let body = ok(case, router, "CreateIpamPool", &params).await;
    member(case, &body, "ipamPoolId")
}

// ---------------------------------------------------------------------------
// Cause 1: a pool's locale has to be a Region its IPAM operates in
// ---------------------------------------------------------------------------

/// `CreateIpamPool` refuses a locale outside the operating Regions of the IPAM its scope
/// belongs to, and accepts every locale inside them — the Regions "where the IPAM is allowed
/// to manage IP address CIDRs" (`Ipam$OperatingRegions`).
///
/// The rows differ along the locale: each Region the IPAM operates in (its home Region and the
/// one it was created with), and two it does not — neither of which the recorded instance
/// names.
#[tokio::test]
async fn an_ipam_pool_must_be_located_in_a_region_its_ipam_operates_in() {
    let (router, _engine) = emulator();
    let ipam = create_ipam("the IPAM", &router, &[OTHER_REGION]).await;

    // (case, the locale the create names, whether the IPAM operates there)
    let rows: [(&str, &str, bool); 4] = [
        ("the IPAM's home Region", REGION, true),
        ("an operating Region the create named", OTHER_REGION, true),
        ("a Region the IPAM does not operate in", "eu-west-1", false),
        ("another Region it does not operate in", "ap-south-1", false),
    ];

    for (case, locale, operates) in rows {
        let params = vec![
            param("IpamScopeId", &ipam.private_scope),
            param("AddressFamily", ADDRESS_FAMILY_IPV4),
            param("Locale", locale),
        ];
        if operates {
            let body = ok(case, &router, "CreateIpamPool", &params).await;
            assert_eq!(
                element(&body, "locale").as_deref(),
                Some(locale),
                "case {case:?}: expected the pool to report the locale it was created with, \
                 got {body}"
            );
            continue;
        }
        refused(
            case,
            &router,
            "CreateIpamPool",
            &params,
            DEPENDENCY_VIOLATION,
            &format!(
                "The specified action is not valid. Ipam {ipam} does not contain the \
                 operating Region {locale}.",
                ipam = ipam.id,
            ),
        )
        .await;
        // Nothing was created: the scope still holds only the pools the accepted rows made.
        let pools = ok(
            case,
            &router,
            "DescribeIpamPools",
            &[
                param("Filter.1.Name", "locale"),
                param("Filter.1.Value.1", locale),
            ],
        )
        .await;
        assert!(
            elements(&pools, "ipamPoolId").is_empty(),
            "case {case:?}: expected no pool in {locale:?}, got {pools}"
        );
    }
}

// ---------------------------------------------------------------------------
// Cause 2: a pool with CIDRs provisioned to it cannot be deleted
// ---------------------------------------------------------------------------

/// The state words this suite pins are the model's own, so a row cannot assert a word the API
/// does not have.
#[test]
fn the_asserted_pool_cidr_state_words_are_the_models_own() {
    for (case, word) in [
        (
            "a CIDR being provisioned",
            IPAM_POOL_CIDR_STATE_PENDING_PROVISION,
        ),
        ("a provisioned CIDR", IPAM_POOL_CIDR_STATE_PROVISIONED),
    ] {
        assert!(
            IPAM_POOL_CIDR_STATE_VALUES.contains(&word),
            "case {case:?}: expected {word:?} to be a member of the modelled catalog, which \
             carries {IPAM_POOL_CIDR_STATE_VALUES:?}"
        );
    }
}

/// A CIDR provisioned to an IPAM pool is reported by `GetIpamPoolCidrs`, blocks the pool's
/// delete with `DependencyViolation`, and stops blocking it once `DeprovisionIpamPoolCidr` has
/// taken it back.
///
/// The rows differ along the address family the pool holds and the number of ranges
/// provisioned to it, which is what the dependency generalises over.
#[tokio::test]
async fn an_ipam_pool_cannot_be_deleted_while_a_cidr_is_provisioned_to_it() {
    // (case, the family, the ranges provisioned to the pool)
    let rows: [(&str, &str, &[&str]); 3] = [
        ("one IPv4 range", ADDRESS_FAMILY_IPV4, &["10.0.0.0/16"]),
        (
            "two IPv4 ranges",
            ADDRESS_FAMILY_IPV4,
            &["10.1.0.0/16", "10.2.0.0/24"],
        ),
        ("one IPv6 range", ADDRESS_FAMILY_IPV6, &["2001:db8::/32"]),
    ];

    for (case, family, ranges) in rows {
        let (router, _engine) = emulator();
        let ipam = create_ipam(case, &router, &[]).await;
        let pool_id = create_pool(case, &router, &ipam.private_scope, family, &[]).await;

        for cidr in ranges {
            let body = ok(
                case,
                &router,
                "ProvisionIpamPoolCidr",
                &[param("IpamPoolId", &pool_id), param("Cidr", cidr)],
            )
            .await;
            assert_eq!(
                element(&body, "cidr").as_deref(),
                Some(*cidr),
                "case {case:?}: expected the provision to answer {cidr:?}, got {body}"
            );
            // A provision is work AWS reports as under way, so the range starts in the
            // transient state of its catalog.
            assert_eq!(
                element(&body, "state").as_deref(),
                Some(IPAM_POOL_CIDR_STATE_PENDING_PROVISION),
                "case {case:?}: expected the provision to answer the transient state, got {body}"
            );
        }

        // Read back out of the store: the ranges the pool holds are the ones provisioned.
        let read = ok(
            case,
            &router,
            "GetIpamPoolCidrs",
            &[param("IpamPoolId", &pool_id)],
        )
        .await;
        let mut reported = elements(&read, "cidr");
        reported.sort();
        let mut expected: Vec<String> = ranges.iter().map(|cidr| (*cidr).to_string()).collect();
        expected.sort();
        assert_eq!(
            reported, expected,
            "case {case:?}: expected the pool to report {expected:?}, got {read}"
        );

        // The delete is refused while any of them is there, in EC2's own wording for this
        // class, and the pool is still deletable afterwards — nothing was written.
        refused(
            case,
            &router,
            "DeleteIpamPool",
            &[param("IpamPoolId", &pool_id)],
            DEPENDENCY_VIOLATION,
            &format!(
                "The specified action is not valid for {pool_id}. There are one or more pool \
                 cidrs provisioned in the pool."
            ),
        )
        .await;

        // The clock owes each range `provisioned`, which it reaches whether or not anybody
        // called — and a provisioned range blocks the delete exactly as a pending one does.
        past_the_transition().await;
        let settled = ok(
            case,
            &router,
            "GetIpamPoolCidrs",
            &[param("IpamPoolId", &pool_id)],
        )
        .await;
        for state in elements(&settled, "state") {
            assert_eq!(
                state, IPAM_POOL_CIDR_STATE_PROVISIONED,
                "case {case:?}: expected every range to have settled, got {settled}"
            );
        }

        // "To deprovision pool CIDRs, see DeprovisionIpamPoolCidr" (`DeleteIpamPool`): taking
        // the ranges back is what makes the pool deletable.
        for cidr in ranges {
            ok(
                case,
                &router,
                "DeprovisionIpamPoolCidr",
                &[param("IpamPoolId", &pool_id), param("Cidr", cidr)],
            )
            .await;
        }
        ok(
            case,
            &router,
            "DeleteIpamPool",
            &[param("IpamPoolId", &pool_id)],
        )
        .await;
    }
}

/// A pool WITHIN another is given space out of its source pool: a provision that names a
/// netmask length rather than a range gets a free block of that size from the source pool, and
/// two such pools never get overlapping space.
///
/// The rows differ along the size asked for, which is what the choice generalises over.
#[tokio::test]
async fn a_provision_that_names_a_size_is_given_free_space_from_the_source_pool() {
    let (router, _engine) = emulator();
    let ipam = create_ipam("the IPAM", &router, &[]).await;
    let source = create_pool(
        "the source pool",
        &router,
        &ipam.private_scope,
        ADDRESS_FAMILY_IPV4,
        &[],
    )
    .await;
    ok(
        "the source pool",
        &router,
        "ProvisionIpamPoolCidr",
        &[param("IpamPoolId", &source), param("Cidr", "10.50.0.0/16")],
    )
    .await;

    let mut given: Vec<String> = Vec::new();
    for netmask_length in ["24", "20", "24"] {
        let case = format!("a child pool asking for a /{netmask_length}");
        let child = create_pool(
            &case,
            &router,
            &ipam.private_scope,
            ADDRESS_FAMILY_IPV4,
            &[param("SourceIpamPoolId", &source)],
        )
        .await;
        let body = ok(
            &case,
            &router,
            "ProvisionIpamPoolCidr",
            &[
                param("IpamPoolId", &child),
                param("NetmaskLength", netmask_length),
            ],
        )
        .await;
        let cidr = member(&case, &body, "cidr");
        assert!(
            cidr.ends_with(&format!("/{netmask_length}")),
            "case {case:?}: expected a /{netmask_length} block, got {cidr:?}"
        );
        // The block comes out of the source pool's own space.
        assert!(
            ec2::state::cidr_block_contains_block("10.50.0.0/16", &cidr),
            "case {case:?}: expected a block inside the source pool's range, got {cidr:?}"
        );
        // And out of space nothing else holds.
        for held in &given {
            assert!(
                !ec2::state::cidr_blocks_overlap(held, &cidr),
                "case {case:?}: expected {cidr:?} not to overlap the already-given {held:?}"
            );
        }
        given.push(cidr);
    }
}

// ---------------------------------------------------------------------------
// Cause 3: a deleted IPAM stops resolving
// ---------------------------------------------------------------------------

/// `DeleteIpam` is refused while a pool is still in one of the IPAM's scopes, deletes the
/// IPAM's own pools when the request carries `Cascade`, and the deleted IPAM then stops
/// resolving: `DescribeIpams` for its id answers `InvalidIpamId.NotFound`.
#[tokio::test]
async fn a_deleted_ipam_stops_resolving() {
    let (router, _engine) = emulator();

    // An IPAM that still holds a pool is not deletable: the pool is a resource inside it.
    let held = create_ipam("an IPAM holding a pool", &router, &[]).await;
    let pool_id = create_pool(
        "an IPAM holding a pool",
        &router,
        &held.private_scope,
        ADDRESS_FAMILY_IPV4,
        &[],
    )
    .await;
    refused(
        "an IPAM holding a pool",
        &router,
        "DeleteIpam",
        &[param("IpamId", &held.id)],
        DEPENDENCY_VIOLATION,
        &format!(
            "The specified action is not valid for {ipam}. There are one or more pools in the \
             ipam.",
            ipam = held.id,
        ),
    )
    .await;
    // Refused, so nothing moved: the IPAM is still there to be read.
    let still_there = ok(
        "an IPAM holding a pool",
        &router,
        "DescribeIpams",
        &[param("IpamId.1", &held.id)],
    )
    .await;
    assert_eq!(
        elements(&still_there, "ipamId"),
        vec![held.id.clone()],
        "a refused delete must leave the IPAM readable, got {still_there}"
    );

    // "Deletes all IPAM pools in private scopes … and the IPAM" (`DeleteIpamRequest$Cascade`).
    let deleted = ok(
        "a cascading delete",
        &router,
        "DeleteIpam",
        &[param("IpamId", &held.id), param("Cascade", "true")],
    )
    .await;
    assert_eq!(
        element(&deleted, "state").as_deref(),
        Some("delete-in-progress"),
        "the delete answers the transient state of the IPAM it started on, got {deleted}"
    );
    // The pool the cascade took with it is on its way out too.
    let pools = ok(
        "a cascading delete",
        &router,
        "DescribeIpamPools",
        &[param("IpamPoolId.1", &pool_id)],
    )
    .await;
    assert!(
        element(&pools, "state")
            .is_some_and(|state| state.starts_with("delete")),
        "the cascade deletes the pools in the IPAM's private scopes, got {pools}"
    );

    // The removal the clock owes the IPAM: once it is due, the id resolves to nothing.
    past_the_transition().await;
    refused(
        "a deleted IPAM",
        &router,
        "DescribeIpams",
        &[param("IpamId.1", &held.id)],
        INVALID_IPAM_ID_NOT_FOUND,
        &format!("The IPAM ({ipam}) does not exist.", ipam = held.id),
    )
    .await;
    // And it is gone from the unfiltered report as well.
    let all = ok("a deleted IPAM", &router, "DescribeIpams", &[]).await;
    assert!(
        !elements(&all, "ipamId").contains(&held.id),
        "the deleted IPAM must be gone from the unfiltered report, got {all}"
    );
}

// ---------------------------------------------------------------------------
// Causes 4 and 5: a resource something occupies cannot be deleted
// ---------------------------------------------------------------------------

/// `DeleteSubnet` is refused while anything still holds a place in the subnet, in EC2's own
/// wording, and succeeds once nothing does.
///
/// The rows differ along the KIND of occupant, which is what the dependency generalises over:
/// an instance the launch placed there, a network interface created there, a NAT gateway
/// created there, one that has already reached `deleted` (which holds nothing), and a subnet
/// nothing ever occupied.
#[tokio::test]
async fn a_subnet_cannot_be_deleted_while_anything_holds_a_place_in_it() {
    /// What occupies the subnet of one row.
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Occupant {
        Instance,
        NetworkInterface,
        NatGateway,
        DeletingNatGateway,
        DeletedNatGateway,
        Nothing,
    }

    // (case, the occupant, whether the delete is refused)
    let rows: [(&str, Occupant, bool); 6] = [
        ("an instance in the subnet", Occupant::Instance, true),
        (
            "a network interface in the subnet",
            Occupant::NetworkInterface,
            true,
        ),
        ("a NAT gateway in the subnet", Occupant::NatGateway, true),
        (
            "a NAT gateway whose delete has been asked for",
            Occupant::DeletingNatGateway,
            false,
        ),
        (
            "a NAT gateway that has reached deleted",
            Occupant::DeletedNatGateway,
            false,
        ),
        ("a subnet nothing occupies", Occupant::Nothing, false),
    ];

    for (case, occupant, blocked) in rows {
        let (router, engine) = emulator();
        let vpc_id = {
            let body = ok(case, &router, "CreateVpc", &[param("CidrBlock", "10.9.0.0/16")]).await;
            member(case, &body, "vpcId")
        };
        let subnet_id = {
            let body = ok(
                case,
                &router,
                "CreateSubnet",
                &[
                    param("VpcId", &vpc_id),
                    param("CidrBlock", "10.9.1.0/24"),
                    param("AvailabilityZone", &format!("{REGION}a")),
                ],
            )
            .await;
            member(case, &body, "subnetId")
        };

        match occupant {
            Occupant::Instance => {
                ok(
                    case,
                    &router,
                    "RunInstances",
                    &[
                        param("ImageId", AMI),
                        param("MinCount", "1"),
                        param("MaxCount", "1"),
                        param("SubnetId", &subnet_id),
                    ],
                )
                .await;
            }
            Occupant::NetworkInterface => {
                ok(
                    case,
                    &router,
                    "CreateNetworkInterface",
                    &[param("SubnetId", &subnet_id)],
                )
                .await;
            }
            // `CreateNatGateway` is still a scaffold with its own open issue, so the gateway
            // is seeded through the store's own writer — the same record a create would
            // store, in each of the three states the rows are about.
            Occupant::NatGateway
            | Occupant::DeletingNatGateway
            | Occupant::DeletedNatGateway => {
                let state = match occupant {
                    Occupant::NatGateway => NAT_GATEWAY_STATE_AVAILABLE,
                    Occupant::DeletingNatGateway => NAT_GATEWAY_STATE_DELETING,
                    _ => NAT_GATEWAY_STATE_DELETED,
                };
                let store = engine.state::<Ec2State>();
                let mut guard = store.lock();
                let nat_gateway_id = guard.next_id("nat");
                guard.insert_nat_gateway(NatGateway {
                    nat_gateway_id: Some(nat_gateway_id),
                    state: Some(state.to_string()),
                    subnet_id: Some(subnet_id.clone()),
                    vpc_id: Some(vpc_id.clone()),
                    create_time: Some(system_time_to_date_time(SystemTime::now())),
                    tags: Some(Vec::<Tag>::new()),
                    ..NatGateway::default()
                });
            }
            Occupant::Nothing => {}
        }

        if blocked {
            refused(
                case,
                &router,
                "DeleteSubnet",
                &[param("SubnetId", &subnet_id)],
                DEPENDENCY_VIOLATION,
                &format!("The subnet '{subnet_id}' has dependencies and cannot be deleted."),
            )
            .await;
            // Refused, so the subnet is still there.
            let read = ok(
                case,
                &router,
                "DescribeSubnets",
                &[param("SubnetId.1", &subnet_id)],
            )
            .await;
            assert_eq!(
                elements(&read, "subnetId"),
                vec![subnet_id.clone()],
                "case {case:?}: a refused delete must leave the subnet readable, got {read}"
            );
            continue;
        }
        ok(
            case,
            &router,
            "DeleteSubnet",
            &[param("SubnetId", &subnet_id)],
        )
        .await;
    }
}

// ---------------------------------------------------------------------------
// Cause 6: a launch cannot apply a security group of another VPC
// ---------------------------------------------------------------------------

/// `RunInstances` refuses a security group that cannot be applied in the subnet the launch is
/// placed in: one of another VPC, and one the account holds no group for.
///
/// The rows differ along how the group is named (by id and by name) and which VPC holds it,
/// which is what the rule generalises over.
#[tokio::test]
async fn a_launch_cannot_apply_a_security_group_from_another_vpc() {
    let (router, _engine) = emulator();

    // Two VPCs, each with a subnet and a group of its own.
    let mut networks: Vec<(String, String, String)> = Vec::new();
    for (index, cidr) in ["10.21.0.0/16", "10.22.0.0/16"].iter().enumerate() {
        let case = "the networks";
        let body = ok(case, &router, "CreateVpc", &[param("CidrBlock", cidr)]).await;
        let vpc_id = member(case, &body, "vpcId");
        let body = ok(
            case,
            &router,
            "CreateSubnet",
            &[
                param("VpcId", &vpc_id),
                param("CidrBlock", &format!("10.2{}.1.0/24", index + 1)),
                param("AvailabilityZone", &format!("{REGION}a")),
            ],
        )
        .await;
        let subnet_id = member(case, &body, "subnetId");
        let body = ok(
            case,
            &router,
            "CreateSecurityGroup",
            &[
                param("VpcId", &vpc_id),
                param("GroupName", &format!("tier-{index}")),
                param("GroupDescription", "the group a launch names"),
            ],
        )
        .await;
        let group_id = member(case, &body, "groupId");
        networks.push((vpc_id, subnet_id, group_id));
    }
    let (first_vpc, first_subnet, first_group) = networks[0].clone();
    let (_, _, second_group) = networks[1].clone();

    // A group of the subnet's own VPC launches.
    let launched = ok(
        "a group of the subnet's VPC",
        &router,
        "RunInstances",
        &[
            param("ImageId", AMI),
            param("MinCount", "1"),
            param("MaxCount", "1"),
            param("SubnetId", &first_subnet),
            param("SecurityGroupId.1", &first_group),
        ],
    )
    .await;
    assert!(
        elements(&launched, "groupId").contains(&first_group),
        "the launched instance must report the group it was launched with, got {launched}"
    );

    // (case, the group the launch names, the code and message it is refused with)
    let refusals: [(&str, Param, String, String); 3] = [
        (
            "a group of another VPC, by id",
            param("SecurityGroupId.1", &second_group),
            INVALID_PARAMETER.to_string(),
            format!(
                "Security group {second_group} and subnet {first_subnet} belong to different \
                 networks."
            ),
        ),
        (
            "a group of another VPC, by name",
            param("SecurityGroup.1", "tier-1"),
            INVALID_GROUP_NOT_FOUND.to_string(),
            format!("The security group 'tier-1' does not exist in VPC '{first_vpc}'"),
        ),
        (
            "a group the account holds none of",
            param("SecurityGroupId.1", "sg-0ffffffffffffffff"),
            INVALID_GROUP_NOT_FOUND.to_string(),
            "The security group 'sg-0ffffffffffffffff' does not exist".to_string(),
        ),
    ];

    for (case, group, code, message) in refusals {
        let params = vec![
            param("ImageId", AMI),
            param("MinCount", "1"),
            param("MaxCount", "1"),
            param("SubnetId", &first_subnet),
            group,
        ];
        refused(case, &router, "RunInstances", &params, &code, &message).await;
    }

    // Nothing the refused launches named was stored: the subnet holds exactly the one
    // instance the accepted launch put there.
    let described = ok(
        "the refused launches",
        &router,
        "DescribeInstances",
        &[
            param("Filter.1.Name", "subnet-id"),
            param("Filter.1.Value.1", &first_subnet),
        ],
    )
    .await;
    assert_eq!(
        elements(&described, "instanceId").len(),
        1,
        "a refused launch must store no instance, got {described}"
    );
}

// ---------------------------------------------------------------------------
// Cause 7: a rule naming a prefix list nothing resolves
// ---------------------------------------------------------------------------

/// `AuthorizeSecurityGroupIngress` refuses a `PrefixListIds` entry that names no prefix list
/// the account can see with that resource's own not-found class, whatever the value LOOKS
/// like: a prefix list id is not one of this API's `<prefix>-<hex>` resource ids, so "no list
/// of that id" is the only question there is to ask about it.
///
/// The rows differ along the SPELLING of the unknown id: a word after the prefix, a mixed
/// value, and each of the two id lengths the service and the Region's published lists use.
#[tokio::test]
async fn a_rule_naming_an_unknown_prefix_list_is_refused_whatever_the_id_looks_like() {
    let (router, _engine) = emulator();
    let group_id = {
        let body = ok(
            "the group",
            &router,
            "CreateSecurityGroup",
            &[
                param("GroupName", "prefix-list-rules"),
                param("GroupDescription", "the group whose rules name a prefix list"),
            ],
        )
        .await;
        member("the group", &body, "groupId")
    };

    for prefix_list_id in [
        "pl-corpvpn",
        "pl-CORPVPN",
        "pl-0zzzzzzzzzzzzzzzz",
        "pl-0123456789abcdef0",
        "pl-0a1b2c3d",
    ] {
        let case = format!("the prefix list id {prefix_list_id:?}");
        let params = vec![
            param("GroupId", &group_id),
            param("IpPermissions.1.IpProtocol", "tcp"),
            param("IpPermissions.1.FromPort", "22"),
            param("IpPermissions.1.ToPort", "22"),
            param("IpPermissions.1.PrefixListIds.1.PrefixListId", prefix_list_id),
        ];
        refused(
            &case,
            &router,
            "AuthorizeSecurityGroupIngress",
            &params,
            INVALID_PREFIX_LIST_ID_NOT_FOUND,
            &format!("The prefix list ID '{prefix_list_id}' does not exist"),
        )
        .await;
    }

    // And no rule was stored for any of them.
    let described = ok(
        "the refused rules",
        &router,
        "DescribeSecurityGroupRules",
        &[
            param("Filter.1.Name", "group-id"),
            param("Filter.1.Value.1", &group_id),
        ],
    )
    .await;
    assert!(
        elements(&described, "prefixListId").is_empty(),
        "a refused rule must store nothing, got {described}"
    );
}

// ---------------------------------------------------------------------------
// Causes 8 and 9: a deleted endpoint and a deleted transit gateway stop resolving
// ---------------------------------------------------------------------------

/// A deleted VPC endpoint stops resolving: `DescribeVpcEndpoints` for its id answers
/// `InvalidVpcEndpointId.NotFound`, and the unfiltered report no longer carries it.
///
/// The rows differ along the endpoint TYPE, which is what decides the state an endpoint is
/// created in and the resources it holds — neither of which changes what a delete leaves
/// readable.
#[tokio::test]
async fn a_deleted_vpc_endpoint_stops_resolving() {
    let (router, _engine) = emulator();
    let vpc_id = {
        let body = ok(
            "the network",
            &router,
            "CreateVpc",
            &[param("CidrBlock", "10.31.0.0/16")],
        )
        .await;
        member("the network", &body, "vpcId")
    };
    let subnet_id = {
        let body = ok(
            "the network",
            &router,
            "CreateSubnet",
            &[
                param("VpcId", &vpc_id),
                param("CidrBlock", "10.31.1.0/24"),
                param("AvailabilityZone", &format!("{REGION}a")),
            ],
        )
        .await;
        member("the network", &body, "subnetId")
    };

    // (case, the endpoint type, the service name, whether the endpoint sits in a subnet)
    let rows: [(&str, &str, &str, bool); 2] = [
        (
            "a gateway endpoint",
            "Gateway",
            "com.amazonaws.us-east-1.s3",
            false,
        ),
        (
            "an interface endpoint",
            "Interface",
            "com.amazonaws.us-east-1.ssm",
            true,
        ),
    ];

    for (case, endpoint_type, service_name, in_subnet) in rows {
        let mut params = vec![
            param("VpcId", &vpc_id),
            param("ServiceName", service_name),
            param("VpcEndpointType", endpoint_type),
            // These rows are about an id that stops resolving once its endpoint is deleted,
            // so they ask for no private hosted zone: a VPC you create has DNS hostnames off,
            // which is what an endpoint associating one needs.
            param("PrivateDnsEnabled", "false"),
        ];
        if in_subnet {
            params.push(param("SubnetId.1", &subnet_id));
        }
        let created = ok(case, &router, "CreateVpcEndpoint", &params).await;
        let endpoint_id = member(case, &created, "vpcEndpointId");

        // Readable before the delete, by the id the create minted.
        let read = ok(
            case,
            &router,
            "DescribeVpcEndpoints",
            &[param("VpcEndpointId.1", &endpoint_id)],
        )
        .await;
        assert_eq!(
            elements(&read, "vpcEndpointId"),
            vec![endpoint_id.clone()],
            "case {case:?}: the created endpoint must be readable, got {read}"
        );

        let deleted = ok(
            case,
            &router,
            "DeleteVpcEndpoints",
            &[param("VpcEndpointId.1", &endpoint_id)],
        )
        .await;
        assert!(
            elements(&deleted, "resourceId").is_empty(),
            "case {case:?}: the delete must report no unsuccessful item, got {deleted}"
        );

        refused(
            case,
            &router,
            "DescribeVpcEndpoints",
            &[param("VpcEndpointId.1", &endpoint_id)],
            INVALID_VPC_ENDPOINT_ID_NOT_FOUND,
            &format!("The Vpc Endpoint Id '{endpoint_id}' does not exist"),
        )
        .await;
        let all = ok(case, &router, "DescribeVpcEndpoints", &[]).await;
        assert!(
            !elements(&all, "vpcEndpointId").contains(&endpoint_id),
            "case {case:?}: the deleted endpoint must be gone from the unfiltered report, got \
             {all}"
        );
    }
}

/// `DeleteTransitGateway` is refused while an attachment still holds the gateway, and a
/// gateway that is deleted stops resolving: the filtered and the unfiltered
/// `DescribeTransitGateways` both stop carrying it, and a describe by id answers
/// `InvalidTransitGatewayID.NotFound`.
#[tokio::test]
async fn a_deleted_transit_gateway_stops_resolving() {
    let (router, _engine) = emulator();
    let vpc_id = {
        let body = ok(
            "the network",
            &router,
            "CreateVpc",
            &[param("CidrBlock", "10.41.0.0/16")],
        )
        .await;
        member("the network", &body, "vpcId")
    };
    let subnet_id = {
        let body = ok(
            "the network",
            &router,
            "CreateSubnet",
            &[
                param("VpcId", &vpc_id),
                param("CidrBlock", "10.41.1.0/24"),
                param("AvailabilityZone", &format!("{REGION}a")),
            ],
        )
        .await;
        member("the network", &body, "subnetId")
    };

    // An attachment holds the gateway it is on, so the delete is refused while one is there.
    let attached = {
        let body = ok("an attached gateway", &router, "CreateTransitGateway", &[]).await;
        member("an attached gateway", &body, "transitGatewayId")
    };
    // An attachment reaches `available` one span after its gateway does, so the refusal is
    // asserted of a gateway whose attachment is in service.
    let attachment = ok(
        "an attached gateway",
        &router,
        "CreateTransitGatewayVpcAttachment",
        &[
            param("TransitGatewayId", &attached),
            param("VpcId", &vpc_id),
            param("SubnetIds.1", &subnet_id),
        ],
    )
    .await;
    assert!(
        element(&attachment, "transitGatewayAttachmentId").is_some(),
        "the attachment the refusal is about must exist, got {attachment}"
    );
    refused(
        "an attached gateway",
        &router,
        "DeleteTransitGateway",
        &[param("TransitGatewayId", &attached)],
        INCORRECT_STATE,
        &format!(
            "{attached} has non-deleted Transit Gateway Attachments and cannot be deleted."
        ),
    )
    .await;
    let still_there = ok(
        "an attached gateway",
        &router,
        "DescribeTransitGateways",
        &[param("TransitGatewayIds.1", &attached)],
    )
    .await;
    assert_eq!(
        elements(&still_there, "transitGatewayId"),
        vec![attached.clone()],
        "a refused delete must leave the gateway readable, got {still_there}"
    );

    // A gateway nothing holds is deletable, and then stops resolving.
    let gateway_id = {
        let body = ok("a gateway nothing holds", &router, "CreateTransitGateway", &[]).await;
        member("a gateway nothing holds", &body, "transitGatewayId")
    };
    let deleted = ok(
        "a gateway nothing holds",
        &router,
        "DeleteTransitGateway",
        &[param("TransitGatewayId", &gateway_id)],
    )
    .await;
    assert_eq!(
        element(&deleted, "state").as_deref(),
        Some("deleting"),
        "the delete answers the state a deletion puts a gateway into, got {deleted}"
    );
    // The transient state a read straight after the delete corroborates, before the removal
    // the clock owes falls due.
    let during = ok(
        "a gateway being deleted",
        &router,
        "DescribeTransitGateways",
        &[param("TransitGatewayIds.1", &gateway_id)],
    )
    .await;
    assert_eq!(
        element(&during, "state").as_deref(),
        Some("deleting"),
        "a caller acting straight after the delete still sees it under way, got {during}"
    );
    // And once it is due, the gateway resolves to nothing.
    past_the_transition().await;
    refused(
        "a deleted gateway",
        &router,
        "DescribeTransitGateways",
        &[param("TransitGatewayIds.1", &gateway_id)],
        INVALID_TRANSIT_GATEWAY_ID_NOT_FOUND,
        &format!("Transit Gateway {gateway_id} was deleted or does not exist."),
    )
    .await;
    // The unfiltered report carries the gateway the refusal above left alone, and nothing of
    // the deleted one.
    let all = ok("a deleted gateway", &router, "DescribeTransitGateways", &[]).await;
    let reported = elements(&all, "transitGatewayId");
    assert!(
        reported.contains(&attached) && !reported.contains(&gateway_id),
        "the deleted gateway must be gone and the attached one left, got {all}"
    );
}
