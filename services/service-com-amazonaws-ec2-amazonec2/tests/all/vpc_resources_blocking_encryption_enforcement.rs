// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `GetVpcResourcesBlockingEncryptionEnforcement`, driven over the
//! wire the service actually speaks: `ec2Query` — an
//! `application/x-www-form-urlencoded` POST whose body carries `Action=…`, answered with
//! XML (README.md / OPERATIONS.md "Wire protocol").
//!
//! The RULE under test: the report names every resource of the ONE VPC the request names
//! whose traffic VPC Encryption Control cannot encrypt — an instance whose instance type
//! does not automatically encrypt in-transit traffic
//! (`NetworkInfo$EncryptionInTransitSupported`), and a VPC attachment to a transit gateway
//! that does not support VPC Encryption Control
//! (`TransitGatewayOptions$EncryptionSupport`) — and nothing else; a VPC that holds none
//! is reported empty rather than refused, and a VPC the account does not have is refused
//! rather than reported empty.
//!
//! Each test is a TABLE whose rows differ along what the rule generalises over: the
//! instance TYPE (pre-Nitro generations, the named earlier types with encrypting adapters,
//! and the generations from which every type encrypts), the KIND of resource (an instance
//! and a transit gateway attachment), where the resource sits (this VPC, another VPC, a
//! subnet nothing minted, no subnet at all), the encryption state of a gateway (every
//! member of the modelled `EncryptionStateValue` enum and a gateway reporting none), the
//! state of an attachment, the page size at each published bound and one past it, and the
//! form of a `VpcId` or a `NextToken` that resolves to nothing — including a token this
//! service really minted, for another VPC's report.

use std::time::{Duration, SystemTime};

use ec2::ops::types::{
    EncryptionSupport, INSTANCE_TYPE_VALUES, TransitGatewayOptions, TransitGatewayVpcAttachment,
};
use ec2::state::{
    ComAmazonawsEc2Amazonec2State as Ec2State, TRANSIT_GATEWAY_ATTACHMENT_STATE_AVAILABLE,
    TRANSIT_GATEWAY_ATTACHMENT_STATE_DELETED, TRANSIT_GATEWAY_ATTACHMENT_STATE_FAILED,
    TRANSIT_GATEWAY_ATTACHMENT_STATE_PENDING_ACCEPTANCE,
    TRANSIT_GATEWAY_ATTACHMENT_STATE_REJECTED, TRANSITION_DELAY, TransitGatewayAttachmentView,
    TransitGatewaySpec,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, element, error_code};
use super::wire::{Param, child, child_text, document, param};

/// The Region every call here is signed for.
const REGION: &str = "us-west-2";

/// The image every launch here names. `RunInstances` stores the id it was given, so the
/// image a launch names has no bearing on this report.
const AMI: &str = "ami-0123456789abcdef0";

/// The HTTP status EC2 answers `DryRunOperation` with.
const DRY_RUN_STATUS: u16 = 412;

/// The `ResourceType` word a report names an instance by.
const TYPE_INSTANCE: &str = "instance";

/// The `ResourceType` word a report names a transit gateway attachment by.
const TYPE_TRANSIT_GATEWAY_ATTACHMENT: &str = "transit-gateway-attachment";

/// Long enough past [`TRANSITION_DELAY`] that a move the clock owes is certainly due.
async fn past_the_transition() {
    tokio::time::sleep(TRANSITION_DELAY + Duration::from_millis(400)).await;
}

// ---------------------------------------------------------------------------
// Calling the service
// ---------------------------------------------------------------------------

/// One `action` call carrying `params`, as its HTTP status and XML body.
async fn api(router: &MultiServiceRouter, action: &str, params: &[Param]) -> (u16, String) {
    call(router, REGION, action, params).await
}

/// One `action` call that has to succeed, and the body it answered.
async fn ok(case: &str, router: &MultiServiceRouter, action: &str, params: &[Param]) -> String {
    let (status, body) = api(router, action, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from {action}, got {status} with body {body}"
    );
    body
}

/// One `GetVpcResourcesBlockingEncryptionEnforcement` call.
async fn report(router: &MultiServiceRouter, params: &[Param]) -> (u16, String) {
    api(
        router,
        "GetVpcResourcesBlockingEncryptionEnforcement",
        params,
    )
    .await
}

/// A VPC with `cidr_block`, as its id.
async fn create_vpc(case: &str, router: &MultiServiceRouter, cidr_block: &str) -> String {
    let body = ok(case, router, "CreateVpc", &[param("CidrBlock", cidr_block)]).await;
    element(&body, "vpcId").unwrap_or_else(|| panic!("case {case:?}: no <vpcId> in {body}"))
}

/// A subnet of `vpc_id` with `cidr_block`, as its id.
async fn create_subnet(
    case: &str,
    router: &MultiServiceRouter,
    vpc_id: &str,
    cidr_block: &str,
) -> String {
    let body = ok(
        case,
        router,
        "CreateSubnet",
        &[param("VpcId", vpc_id), param("CidrBlock", cidr_block)],
    )
    .await;
    element(&body, "subnetId").unwrap_or_else(|| panic!("case {case:?}: no <subnetId> in {body}"))
}

/// One instance of `instance_type` in `subnet_id` (or in no subnet at all), as its id.
async fn launch(
    case: &str,
    router: &MultiServiceRouter,
    subnet_id: Option<&str>,
    instance_type: &str,
) -> String {
    let mut params = vec![
        param("ImageId", AMI),
        param("MinCount", "1"),
        param("MaxCount", "1"),
        param("InstanceType", instance_type),
    ];
    if let Some(subnet_id) = subnet_id {
        params.push(param("SubnetId", subnet_id));
    }
    let body = ok(case, router, "RunInstances", &params).await;
    element(&body, "instanceId").unwrap_or_else(|| panic!("case {case:?}: no <instanceId> in {body}"))
}

// ---------------------------------------------------------------------------
// Reading the answer
// ---------------------------------------------------------------------------

/// One reported non-compliant resource. A resource is a RECORD, not one element, so it is
/// read with a parser rather than by finding a tag.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ResourceRow {
    id: String,
    kind: String,
    description: String,
    is_excludable: String,
}

/// The `<item>` elements of the `set` member of `node`.
fn items<'a, 'i>(node: roxmltree::Node<'a, 'i>, set: &str) -> Vec<roxmltree::Node<'a, 'i>> {
    child(node, set)
        .map(|set| set.children().filter(roxmltree::Node::is_element).collect())
        .unwrap_or_default()
}

/// The resources a response reports, in the order it reports them.
fn reported(case: &str, body: &str) -> Vec<ResourceRow> {
    let document = document(case, body);
    items(document.root_element(), "nonCompliantResourceSet")
        .into_iter()
        .map(|resource| ResourceRow {
            id: child_text(resource, "id").unwrap_or_default(),
            kind: child_text(resource, "type").unwrap_or_default(),
            description: child_text(resource, "description").unwrap_or_default(),
            is_excludable: child_text(resource, "isExcludable").unwrap_or_default(),
        })
        .collect()
}

/// The ids a response reports, in the order it reports them.
fn reported_ids(case: &str, body: &str) -> Vec<String> {
    reported(case, body)
        .into_iter()
        .map(|resource| resource.id)
        .collect()
}

/// The `nextToken` of a response, if it carries a non-empty one.
fn next_token(case: &str, body: &str) -> Option<String> {
    let document = document(case, body);
    child_text(document.root_element(), "nextToken").filter(|token| !token.is_empty())
}

// ---------------------------------------------------------------------------
// The instance-type table
// ---------------------------------------------------------------------------

/// One row of the instance-type table: an instance type, and whether it automatically
/// encrypts in-transit traffic between instances.
///
/// `encrypts` is written out per row from what AWS publishes ("Encryption in transit",
/// Amazon EC2 User Guide: the types built on the Nitro System from its 4th version — the
/// 6th instance generation and later — plus the earlier C5a, C5ad, C5n, D3, D3en, G4, I3en,
/// I4i, Im4gn, Is4gen, M5dn, M5n, M5zn, P3dn, P4d, R5dn, R5n, X2idn, X2iedn, X2iezn and the
/// high-memory U-*), never derived from the emulator's own answer.
#[derive(Debug, Clone, Copy)]
struct TypeRow {
    case: &'static str,
    instance_type: &'static str,
    encrypts: bool,
}

/// The table the instance half of the rule is asserted over: the rows differ along the
/// generation of the type, whether it is one of the earlier types AWS names one by one, and
/// which family of that generation it belongs to.
const TYPE_ROWS: [TypeRow; 19] = [
    TypeRow {
        case: "a pre-Nitro burstable type",
        instance_type: "t2.micro",
        encrypts: false,
    },
    TypeRow {
        case: "a 4th-generation general-purpose type",
        instance_type: "m4.large",
        encrypts: false,
    },
    TypeRow {
        case: "a 5th-generation general-purpose type",
        instance_type: "m5.large",
        encrypts: false,
    },
    TypeRow {
        case: "a 3rd-generation GPU type",
        instance_type: "p3.2xlarge",
        encrypts: false,
    },
    TypeRow {
        case: "a 5th-generation compute type with the 100-Gbps adapter",
        instance_type: "c5n.large",
        encrypts: true,
    },
    TypeRow {
        case: "a named 5th-generation network-optimised type",
        instance_type: "m5n.large",
        encrypts: true,
    },
    TypeRow {
        case: "a named 5th-generation network-optimised type with local storage",
        instance_type: "r5dn.large",
        encrypts: true,
    },
    TypeRow {
        case: "a named 4th-generation GPU type",
        instance_type: "g4dn.xlarge",
        encrypts: true,
    },
    TypeRow {
        case: "a named 3rd-generation storage type",
        instance_type: "i3en.large",
        encrypts: true,
    },
    TypeRow {
        case: "a named 3rd-generation GPU type",
        instance_type: "p3dn.24xlarge",
        encrypts: true,
    },
    TypeRow {
        case: "a 6th-generation Intel type",
        instance_type: "m6i.large",
        encrypts: true,
    },
    TypeRow {
        case: "a 6th-generation Graviton network-optimised type",
        instance_type: "c6gn.large",
        encrypts: true,
    },
    TypeRow {
        case: "a 7th-generation AMD type",
        instance_type: "c7a.2xlarge",
        encrypts: true,
    },
    // The two family-name shapes whose digits are NOT a generation: an Apple-hardware
    // family, and the high-memory families whose digits state a memory size. The
    // Apple-hardware family is not one of the types AWS names and names no generation that
    // all encrypt, so it blocks; the high-memory family IS one AWS names (`U-*`), and the
    // two rows below — one whose memory size would read as a generation past the encrypting
    // one, one whose would read as a generation below it — report alike, which is what
    // keeps the `6tb` of `u-6tb1` from being read as a 6th generation.
    TypeRow {
        case: "an Apple-hardware type",
        instance_type: "mac2.metal",
        encrypts: false,
    },
    TypeRow {
        case: "a high-memory type whose memory size would read as a 6th generation",
        instance_type: "u-6tb1.metal",
        encrypts: true,
    },
    TypeRow {
        case: "a high-memory type whose memory size would read as a 3rd generation",
        instance_type: "u-3tb1.56xlarge",
        encrypts: true,
    },
    // Three of the named families below the encrypting generation that the catalog's own
    // list carries beside the ones above: an AMD compute type, a dense-storage type and a
    // memory-optimised type.
    TypeRow {
        case: "a named 5th-generation AMD compute type",
        instance_type: "c5a.large",
        encrypts: true,
    },
    TypeRow {
        case: "a named 3rd-generation dense-storage type",
        instance_type: "d3.xlarge",
        encrypts: true,
    },
    TypeRow {
        case: "a named 2nd-generation memory-optimised type",
        instance_type: "x2idn.16xlarge",
        encrypts: true,
    },
];

/// Every row names an instance type the model declares (the `InstanceType` enum, as
/// `ec2::ops::types::INSTANCE_TYPE_VALUES` carries it), so a row can never pass by naming a
/// type no launch would accept.
fn assert_rows_are_modelled() {
    for row in TYPE_ROWS {
        assert!(
            INSTANCE_TYPE_VALUES.contains(&row.instance_type),
            "case {:?}: {} is not a value of the modelled InstanceType enum",
            row.case,
            row.instance_type
        );
    }
}

// ---------------------------------------------------------------------------
// Seeding a transit gateway attachment
// ---------------------------------------------------------------------------

/// Write a transit gateway whose options report `encryption_state` (or no encryption
/// support at all), plus a VPC attachment of `vpc_id` in `attachment_state`, and answer the
/// attachment's id and the gateway's.
///
/// Both go through the store's own writer seams — `CreateTransitGatewayVpcAttachment` and
/// `ModifyTransitGateway` are still scaffold stubs, each with its own open issue, so there
/// is no public call that mints either yet.
fn seed_attachment(
    engine: &engine::InMemoryEngine,
    vpc_id: &str,
    encryption_state: Option<&str>,
    attachment_state: &str,
) -> (String, String) {
    let state = engine.state::<Ec2State>();
    let mut store = state.lock();
    let transit_gateway_id = store.insert_transit_gateway(TransitGatewaySpec {
        description: None,
        region: REGION.to_string(),
        created_at: SystemTime::now(),
        options: TransitGatewayOptions {
            encryption_support: encryption_state.map(|state| EncryptionSupport {
                encryption_state: Some(state.to_string()),
                state_message: None,
            }),
            ..Default::default()
        },
        tags: Vec::new(),
    });
    let attachment_id = store
        .insert_transit_gateway_attachment(TransitGatewayAttachmentView::Vpc(
            TransitGatewayVpcAttachment {
                transit_gateway_id: Some(transit_gateway_id.clone()),
                vpc_id: Some(vpc_id.to_string()),
                state: Some(attachment_state.to_string()),
                ..Default::default()
            },
        ))
        .expect("the attachment store has room for one more attachment");
    (attachment_id, transit_gateway_id)
}

// ---------------------------------------------------------------------------
// The rule
// ---------------------------------------------------------------------------

/// An instance of the VPC is reported exactly while its instance type does not
/// automatically encrypt in-transit traffic, named by the `instance` resource type, not
/// excludable, and with a description naming the type that cannot encrypt.
#[tokio::test]
async fn an_instance_blocks_enforcement_exactly_while_its_type_does_not_encrypt_in_transit() {
    assert_rows_are_modelled();
    let (router, _engine) = super::ec2_query::emulator();
    let vpc_id = create_vpc("the VPC under test", &router, "10.0.0.0/16").await;
    let subnet_id = create_subnet("the VPC under test", &router, &vpc_id, "10.0.1.0/24").await;

    let launched: Vec<(TypeRow, String)> = {
        let mut launched = Vec::new();
        for row in TYPE_ROWS {
            let instance_id = launch(row.case, &router, Some(&subnet_id), row.instance_type).await;
            launched.push((row, instance_id));
        }
        launched
    };

    let (status, body) = report(&router, &[param("VpcId", &vpc_id)]).await;
    assert_eq!(status, 200, "reporting the VPC under test: got {body}");
    let found = reported("the whole VPC", &body);

    for (row, instance_id) in &launched {
        let case = row.case;
        let entry = found.iter().find(|resource| &resource.id == instance_id);
        if row.encrypts {
            assert!(
                entry.is_none(),
                "case {case:?}: {} encrypts in transit, so {instance_id} blocks nothing, got \
                 {entry:?}",
                row.instance_type
            );
            continue;
        }
        let entry = entry.unwrap_or_else(|| {
            panic!(
                "case {case:?}: {} does not encrypt in transit, so {instance_id} must be \
                 reported, got {found:?}",
                row.instance_type
            )
        });
        assert_eq!(
            entry.kind, TYPE_INSTANCE,
            "case {case:?}: expected the resource type {TYPE_INSTANCE:?}, got {:?}",
            entry.kind
        );
        assert_eq!(
            entry.is_excludable, "false",
            "case {case:?}: an instance cannot be excluded from enforcement, got {:?}",
            entry.is_excludable
        );
        assert!(
            entry.description.contains(row.instance_type),
            "case {case:?}: expected a description naming {}, got {:?}",
            row.instance_type,
            entry.description
        );
    }

    let expected: Vec<String> = {
        let mut ids: Vec<String> = launched
            .iter()
            .filter(|(row, _)| !row.encrypts)
            .map(|(_, instance_id)| instance_id.clone())
            .collect();
        ids.sort();
        ids
    };
    assert_eq!(
        reported_ids("the whole VPC", &body),
        expected,
        "the report names exactly the instances that cannot encrypt, in id order, got {found:?}"
    );
}

/// The report is about the VPC the request names: an instance of another VPC, of a subnet
/// nothing minted, or of no subnet at all is never in it.
#[tokio::test]
async fn the_report_names_only_the_resources_of_the_named_vpc() {
    let (router, _engine) = super::ec2_query::emulator();
    let here = create_vpc("the VPC under test", &router, "10.0.0.0/16").await;
    let here_subnet = create_subnet("the VPC under test", &router, &here, "10.0.1.0/24").await;
    let there = create_vpc("the other VPC", &router, "10.1.0.0/16").await;
    let there_subnet = create_subnet("the other VPC", &router, &there, "10.1.1.0/24").await;

    // Rows: an instance of each VPC, and one launched with no subnet at all — which the
    // service places in the DEFAULT VPC's own subnet, so neither report below names it. A
    // launch naming a subnet nothing minted is no row here: the account holds no such subnet,
    // so EC2 refuses that launch rather than placing an instance outside every VPC.
    let mine = launch("in the VPC under test", &router, Some(&here_subnet), "t2.micro").await;
    let theirs = launch("in the other VPC", &router, Some(&there_subnet), "m4.large").await;
    let subnetless = launch("in no subnet at all", &router, None, "t2.micro").await;

    for (case, vpc_id, expected) in [
        ("the VPC under test", here.clone(), vec![mine.clone()]),
        ("the other VPC", there.clone(), vec![theirs.clone()]),
    ] {
        let (status, body) = report(&router, &[param("VpcId", &vpc_id)]).await;
        assert_eq!(status, 200, "case {case:?}: got {body}");
        let found = reported_ids(case, &body);
        assert_eq!(
            found, expected,
            "case {case:?}: expected {expected:?} in the report, got {found:?}"
        );
        assert!(
            !found.contains(&subnetless),
            "case {case:?}: {subnetless} sits in neither VPC, got {found:?}"
        );
    }
}

/// A VPC attachment of the named VPC is reported exactly while the transit gateway it
/// attaches does not support VPC Encryption Control and the attachment was not rejected,
/// named by the `transit-gateway-attachment` resource type and not excludable.
#[tokio::test]
async fn an_attachment_blocks_enforcement_unless_its_gateway_supports_encryption_control() {
    let (router, engine) = super::ec2_query::emulator();
    let vpc_id = create_vpc("the VPC under test", &router, "10.0.0.0/16").await;
    let other_vpc_id = create_vpc("the other VPC", &router, "10.1.0.0/16").await;

    // Rows: every member of the modelled `EncryptionStateValue` enum (`enabling`,
    // `enabled`, `disabling`, `disabled`), a gateway reporting no encryption support at
    // all, the attachment states either side of a decision, and an attachment of another
    // VPC.
    let rows: [(&str, &str, Option<&str>, &str, bool); 10] = [
        (
            "a gateway reporting no encryption support",
            &vpc_id,
            None,
            TRANSIT_GATEWAY_ATTACHMENT_STATE_AVAILABLE,
            true,
        ),
        (
            "a gateway whose encryption is enabled",
            &vpc_id,
            Some("enabled"),
            TRANSIT_GATEWAY_ATTACHMENT_STATE_AVAILABLE,
            false,
        ),
        (
            "a gateway whose encryption is still enabling",
            &vpc_id,
            Some("enabling"),
            TRANSIT_GATEWAY_ATTACHMENT_STATE_AVAILABLE,
            true,
        ),
        (
            "a gateway whose encryption is disabling",
            &vpc_id,
            Some("disabling"),
            TRANSIT_GATEWAY_ATTACHMENT_STATE_AVAILABLE,
            true,
        ),
        (
            "a gateway whose encryption is disabled",
            &vpc_id,
            Some("disabled"),
            TRANSIT_GATEWAY_ATTACHMENT_STATE_AVAILABLE,
            true,
        ),
        (
            "an attachment still pending acceptance",
            &vpc_id,
            None,
            TRANSIT_GATEWAY_ATTACHMENT_STATE_PENDING_ACCEPTANCE,
            true,
        ),
        (
            "a rejected attachment",
            &vpc_id,
            None,
            TRANSIT_GATEWAY_ATTACHMENT_STATE_REJECTED,
            false,
        ),
        (
            "an attachment of another VPC",
            &other_vpc_id,
            None,
            TRANSIT_GATEWAY_ATTACHMENT_STATE_AVAILABLE,
            false,
        ),
        // Two more members of the modelled `TransitGatewayAttachmentState` in which an
        // attachment carries no traffic and never will again: nothing in this crate mints
        // them yet, and a report must not name such an attachment when something does.
        (
            "a deleted attachment",
            &vpc_id,
            None,
            TRANSIT_GATEWAY_ATTACHMENT_STATE_DELETED,
            false,
        ),
        (
            "a failed attachment",
            &vpc_id,
            None,
            TRANSIT_GATEWAY_ATTACHMENT_STATE_FAILED,
            false,
        ),
    ];

    let seeded: Vec<(&str, String, String, bool)> = rows
        .iter()
        .map(|(case, attached_vpc, encryption_state, attachment_state, blocks)| {
            let (attachment_id, transit_gateway_id) =
                seed_attachment(&engine, attached_vpc, *encryption_state, attachment_state);
            (*case, attachment_id, transit_gateway_id, *blocks)
        })
        .collect();

    let (status, body) = report(&router, &[param("VpcId", &vpc_id)]).await;
    assert_eq!(status, 200, "reporting the VPC under test: got {body}");
    let found = reported("the whole VPC", &body);

    for (case, attachment_id, transit_gateway_id, blocks) in &seeded {
        let entry = found.iter().find(|resource| &resource.id == attachment_id);
        if !blocks {
            assert!(
                entry.is_none(),
                "case {case:?}: {attachment_id} blocks nothing, got {entry:?}"
            );
            continue;
        }
        let entry = entry.unwrap_or_else(|| {
            panic!("case {case:?}: expected {attachment_id} in the report, got {found:?}")
        });
        assert_eq!(
            entry.kind, TYPE_TRANSIT_GATEWAY_ATTACHMENT,
            "case {case:?}: expected the resource type {TYPE_TRANSIT_GATEWAY_ATTACHMENT:?}, got \
             {:?}",
            entry.kind
        );
        assert_eq!(
            entry.is_excludable, "false",
            "case {case:?}: an attachment cannot be excluded from enforcement, got {:?}",
            entry.is_excludable
        );
        assert!(
            entry.description.contains(transit_gateway_id),
            "case {case:?}: expected a description naming {transit_gateway_id}, got {:?}",
            entry.description
        );
    }

    let expected: Vec<String> = {
        let mut ids: Vec<String> = seeded
            .iter()
            .filter(|(_, _, _, blocks)| *blocks)
            .map(|(_, attachment_id, _, _)| attachment_id.clone())
            .collect();
        ids.sort();
        ids
    };
    assert_eq!(
        reported_ids("the whole VPC", &body),
        expected,
        "the report names exactly the attachments whose gateway cannot encrypt, in id order, got \
         {found:?}"
    );
}

/// A VPC that holds nothing non-compliant is reported EMPTY — the question the operation
/// answers before enforcement is turned on — and the resources of a VPC that holds only
/// compliant ones are not reported either.
#[tokio::test]
async fn a_vpc_with_nothing_non_compliant_is_reported_empty() {
    let (router, engine) = super::ec2_query::emulator();

    // Rows: a VPC with nothing in it at all, a VPC whose instances all encrypt, and a VPC
    // whose only attachment is to a gateway that supports encryption control.
    let bare = create_vpc("a bare VPC", &router, "10.0.0.0/16").await;
    let compliant = create_vpc("a compliant VPC", &router, "10.1.0.0/16").await;
    let subnet_id = create_subnet("a compliant VPC", &router, &compliant, "10.1.1.0/24").await;
    for row in TYPE_ROWS.iter().filter(|row| row.encrypts) {
        launch(row.case, &router, Some(&subnet_id), row.instance_type).await;
    }
    seed_attachment(
        &engine,
        &compliant,
        Some("enabled"),
        TRANSIT_GATEWAY_ATTACHMENT_STATE_AVAILABLE,
    );

    for (case, vpc_id) in [("a bare VPC", bare), ("a compliant VPC", compliant)] {
        let (status, body) = report(&router, &[param("VpcId", &vpc_id)]).await;
        assert_eq!(status, 200, "case {case:?}: got {body}");
        let found = reported(case, &body);
        assert!(
            found.is_empty(),
            "case {case:?}: nothing in {vpc_id} blocks enforcement, got {found:?}"
        );
    }
}

/// A terminated instance is not a resource of the VPC any more, so the CLOCK takes it out
/// of the report — no caller has to.
#[tokio::test]
async fn a_terminated_instance_leaves_the_report() {
    let (router, _engine) = super::ec2_query::emulator();
    let vpc_id = create_vpc("the VPC under test", &router, "10.0.0.0/16").await;
    let subnet_id = create_subnet("the VPC under test", &router, &vpc_id, "10.0.1.0/24").await;
    let instance_id = launch("a type that cannot encrypt", &router, Some(&subnet_id), "t2.micro")
        .await;

    let (status, body) = report(&router, &[param("VpcId", &vpc_id)]).await;
    assert_eq!(status, 200, "before the terminate: got {body}");
    assert_eq!(
        reported_ids("before the terminate", &body),
        vec![instance_id.clone()],
        "case {instance_id:?}: a live instance that cannot encrypt blocks enforcement"
    );

    past_the_transition().await;
    ok(
        "terminating the instance",
        &router,
        "TerminateInstances",
        &[param("InstanceId.1", &instance_id)],
    )
    .await;
    past_the_transition().await;

    let (status, body) = report(&router, &[param("VpcId", &vpc_id)]).await;
    assert_eq!(status, 200, "after the terminate: got {body}");
    assert!(
        reported_ids("after the terminate", &body).is_empty(),
        "case {instance_id:?}: a terminated instance blocks nothing, got {body}"
    );
}

/// A VPC the account does not have is refused with `InvalidVpcID.NotFound`, whatever form
/// the id a caller sent takes.
#[tokio::test]
async fn a_vpc_the_account_does_not_have_is_refused() {
    let (router, _engine) = super::ec2_query::emulator();
    let vpc_id = create_vpc("the VPC under test", &router, "10.0.0.0/16").await;
    let subnet_id = create_subnet("the VPC under test", &router, &vpc_id, "10.0.1.0/24").await;

    // A VPC that was created and then deleted, so its id is one the account genuinely had.
    let deleted = create_vpc("a VPC to delete", &router, "10.2.0.0/16").await;
    ok(
        "deleting a VPC",
        &router,
        "DeleteVpc",
        &[param("VpcId", &deleted)],
    )
    .await;

    // Rows: the long and the short id form, a well-formed id of another resource type, the
    // id of a resource the account HAS but which is not a VPC, and a VPC the account had.
    let cases = [
        ("a long-form VPC id nothing minted", "vpc-0123456789abcdef0".to_string()),
        ("a short-form VPC id", "vpc-12345678".to_string()),
        ("a subnet id in the VpcId member", "subnet-0123456789abcdef0".to_string()),
        ("the id of a subnet the account has", subnet_id.clone()),
        ("a VPC the account no longer has", deleted),
    ];
    for (case, candidate) in cases {
        let (status, body) = report(&router, &[param("VpcId", &candidate)]).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 for {candidate}, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidVpcID.NotFound",
            "case {case:?}: expected InvalidVpcID.NotFound for {candidate}, got {body}"
        );
    }
}

/// `MaxResults` cuts the report into pages in resource-id order, the token resumes from
/// where the page ended, and the walk reports every blocking resource exactly once and
/// then ends.
#[tokio::test]
async fn max_results_and_next_token_page_the_report_in_resource_id_order() {
    let (router, engine) = super::ec2_query::emulator();
    let vpc_id = create_vpc("the VPC under test", &router, "10.0.0.0/16").await;
    let subnet_id = create_subnet("the VPC under test", &router, &vpc_id, "10.0.1.0/24").await;

    // Seven blocking resources — more than the smallest page the model admits, so a page
    // size at that bound takes more than one request — of both kinds, so the order a walk
    // reports them in is not the order of any one store.
    let mut expected: Vec<String> = Vec::new();
    for index in 0..5 {
        expected.push(
            launch(
                "one of the VPC's blocking instances",
                &router,
                Some(&subnet_id),
                TYPE_ROWS[index % 4].instance_type,
            )
            .await,
        );
    }
    for _ in 0..2 {
        let (attachment_id, _) = seed_attachment(
            &engine,
            &vpc_id,
            None,
            TRANSIT_GATEWAY_ATTACHMENT_STATE_AVAILABLE,
        );
        expected.push(attachment_id);
    }
    expected.sort();

    // Rows: the published bounds of `GetVpcResourcesBlockingEncryptionEnforcementMaxResults`
    // (range 5..1000) and a request that names no page size at all.
    for page_size in [Some(5), Some(1000), None] {
        let case = format!("MaxResults={page_size:?}");
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut calls = 0;
        loop {
            let mut params = vec![param("VpcId", &vpc_id)];
            if let Some(size) = page_size {
                params.push(param("MaxResults", &size.to_string()));
            }
            if let Some(cursor) = &token {
                params.push(param("NextToken", cursor));
            }
            let (status, body) = report(&router, &params).await;
            assert_eq!(status, 200, "case {case:?}: got {body}");
            let page = reported_ids(&case, &body);
            if let Some(size) = page_size {
                assert!(
                    page.len() <= size,
                    "case {case:?}: a page may not exceed MaxResults, got {page:?}"
                );
            }
            let mut sorted = page.clone();
            sorted.sort();
            assert_eq!(
                page, sorted,
                "case {case:?}: a page is reported in resource-id order, got {page:?}"
            );
            walked.extend(page);
            token = next_token(&case, &body);
            calls += 1;
            if token.is_none() {
                break;
            }
            assert!(
                calls <= expected.len() + 2,
                "case {case:?}: the walk must end, got {calls} requests and the token {token:?}"
            );
        }
        assert_eq!(
            walked, expected,
            "case {case:?}: the walk reports every blocking resource exactly once, in id order, \
             got {walked:?}"
        );
    }
}

/// `MaxResults` is accepted at the bounds the model publishes and refused past them.
#[tokio::test]
async fn max_results_is_accepted_at_its_bounds_and_refused_past_them() {
    let (router, _engine) = super::ec2_query::emulator();
    let vpc_id = create_vpc("the VPC under test", &router, "10.0.0.0/16").await;

    // `GetVpcResourcesBlockingEncryptionEnforcementMaxResults` is modelled `range 5..1000`,
    // so the rows are its published bounds and one past each.
    for (max_results, expected_status) in [(4, 400), (5, 200), (1000, 200), (1001, 400)] {
        let params = vec![
            param("VpcId", &vpc_id),
            param("MaxResults", &max_results.to_string()),
        ];
        let (status, body) = report(&router, &params).await;
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
    let (router, _engine) = super::ec2_query::emulator();
    let vpc_id = create_vpc("the VPC under test", &router, "10.0.0.0/16").await;
    let subnet_id = create_subnet("the VPC under test", &router, &vpc_id, "10.0.1.0/24").await;
    let instance_id = launch("a type that cannot encrypt", &router, Some(&subnet_id), "t2.micro")
        .await;

    // A token this service really did mint — for ANOTHER VPC's report, which is a report
    // this one never described.
    let other_vpc_id = create_vpc("the other VPC", &router, "10.1.0.0/16").await;
    let (status, body) = report(&router, &[param("VpcId", &other_vpc_id)]).await;
    assert_eq!(status, 200, "reporting the other VPC: got {body}");
    let foreign = next_token("the other VPC", &body)
        .unwrap_or_else(|| panic!("the other VPC's report must carry a token: {body}"));

    // Rows: free text, punctuation, hex this service never minted, the bare resource id a
    // caller might guess the cursor is, that id hex-encoded, a forged token carrying this
    // service's marker with a checksum that does not match its cursor, and a real token
    // minted for another VPC's report.
    let cases = [
        "not-a-token".to_string(),
        "###".to_string(),
        "6161".to_string(),
        instance_id.clone(),
        ::hex::encode(&instance_id),
        ::hex::encode(format!("ec2-1:0000000000000000:{instance_id}")),
        foreign,
    ];
    for token in cases {
        let params = vec![param("VpcId", &vpc_id), param("NextToken", &token)];
        let (status, body) = report(&router, &params).await;
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

/// A dry run reports the outcome and reads nothing; reporting twice reports the same
/// thing, so this report cannot change what a later caller sees.
#[tokio::test]
async fn a_dry_run_reports_the_outcome_and_a_report_changes_nothing() {
    let (router, engine) = super::ec2_query::emulator();
    let vpc_id = create_vpc("the VPC under test", &router, "10.0.0.0/16").await;
    let subnet_id = create_subnet("the VPC under test", &router, &vpc_id, "10.0.1.0/24").await;
    launch("a type that cannot encrypt", &router, Some(&subnet_id), "t2.micro").await;
    seed_attachment(
        &engine,
        &vpc_id,
        None,
        TRANSIT_GATEWAY_ATTACHMENT_STATE_AVAILABLE,
    );

    let (status, body) = report(&router, &[param("VpcId", &vpc_id)]).await;
    assert_eq!(status, 200, "the first report: got {body}");
    let first = reported("the first report", &body);
    assert_eq!(
        first.len(),
        2,
        "the seeded VPC holds two blocking resources, got {first:?}"
    );
    let held = {
        let store = engine.state::<Ec2State>();
        let store = store.lock();
        (
            store.instances.len(),
            store.transit_gateway_attachments.len(),
        )
    };

    // Rows: a dry run on a VPC the account has, and one on a VPC it does not — "if you
    // have the required permissions, the error response is DryRunOperation"
    // (`GetVpcResourcesBlockingEncryptionEnforcementRequest$DryRun`), and every caller of
    // this emulator has them.
    for (case, candidate) in [
        ("a VPC of the account", vpc_id.clone()),
        (
            "a VPC the account does not have",
            "vpc-0123456789abcdef0".to_string(),
        ),
    ] {
        let params = vec![param("VpcId", &candidate), param("DryRun", "true")];
        let (status, body) = report(&router, &params).await;
        assert_eq!(
            status, DRY_RUN_STATUS,
            "case {case:?}: expected HTTP {DRY_RUN_STATUS}, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "DryRunOperation",
            "case {case:?}: expected DryRunOperation, got {body}"
        );
    }

    let (status, body) = report(&router, &[param("VpcId", &vpc_id)]).await;
    assert_eq!(status, 200, "the second report: got {body}");
    let second = reported("the second report", &body);
    assert_eq!(
        second, first,
        "a report is a read: expected {first:?}, got {second:?}"
    );
    let still_held = {
        let store = engine.state::<Ec2State>();
        let store = store.lock();
        (
            store.instances.len(),
            store.transit_gateway_attachments.len(),
        )
    };
    assert_eq!(
        still_held, held,
        "a report writes nothing: expected {held:?} stored records, got {still_held:?}"
    );
}
