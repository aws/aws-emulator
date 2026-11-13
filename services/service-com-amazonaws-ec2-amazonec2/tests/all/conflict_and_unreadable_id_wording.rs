// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the WORDING of EC2's conflict and unreadable-id refusals, driven
//! over the wire the service actually speaks — `ec2Query`, an
//! `application/x-www-form-urlencoded` POST answered with XML (README.md / OPERATIONS.md
//! "Wire protocol") — through the crate's shared [`super::ec2_query`] harness.
//!
//! The rule under test: a refusal that reports a resource the account ALREADY HOLDS, or an
//! identifier the service cannot read, answers the sentence EC2 answers for that error
//! class — the class's fixed text exactly, terminating period included where the class has
//! one, with only the variable parts EC2 interpolates, taken from the request and from the
//! store. Those variable parts are the attributes by which the CALLER identifies the
//! conflicting object (a resource's name, the id of the resource that scopes that name),
//! never an id the service minted that the caller has not been shown, and never a value the
//! class leaves out. EC2 models no error shapes
//! (`smithy_query(query="operation",operation="CreateLaunchTemplate")` reports `errors: []`),
//! so each sentence below is the recorded one for its class, and classes that name the same
//! resource kind from different operations do NOT share one sentence: EC2's per-operation
//! validators word them differently, so each operation keeps the sentence recorded for it.
//!
//! Each table row is a distinct input along what the rule generalises over — a name's length
//! and character classes, which resource scopes the name, how a request spells a name it is
//! matched against case-insensitively, and the FORM in which an identifier falls outside the
//! one the service issues — so the one case a recording happens to contain is one row among
//! many rather than the specification.
//!
//! What each operation STORES is its own suite's subject
//! ([`super::create_security_group`], [`super::placement_groups`],
//! [`super::launch_templates`], [`super::capacity_reservation_topology`]); this suite asserts
//! only the wording of their refusals, and that a refused call stores nothing a later read
//! can see.

use ec2::state::{
    LAUNCH_TEMPLATE_NAME_PUNCTUATION, MAX_LAUNCH_TEMPLATE_NAME_CHARS,
    MAX_PLACEMENT_GROUP_NAME_CHARS, MIN_LAUNCH_TEMPLATE_NAME_CHARS, SECURITY_GROUP_PUNCTUATION,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{element, error_code, error_message, indexed};
use super::wire::{child, document, param, Param};

/// The Region every call here is signed for.
const REGION: &str = "us-east-1";

/// The instance type, platform and zone every Capacity Reservation here reserves capacity
/// for — members of the modelled `InstanceType` and `CapacityReservationInstancePlatform`
/// catalogs.
const INSTANCE_TYPE: &str = "c5.xlarge";
const INSTANCE_PLATFORM: &str = "Linux/UNIX";
const AVAILABILITY_ZONE: &str = "us-east-1a";

/// One ec2Query call through the shared harness, signed for [`REGION`].
async fn api(router: &MultiServiceRouter, action: &str, params: &[Param]) -> (u16, String) {
    super::ec2_query::call(router, REGION, action, params).await
}

/// The emulator the tests drive, as [`super::ec2_query::emulator`] builds it.
fn emulator() -> MultiServiceRouter {
    super::ec2_query::emulator().0
}

/// Assert that `status` and `body` are a 400 carrying `code` and exactly `message`.
///
/// The one reading of a refusal this suite makes: status, code and the whole message, so a
/// row cannot pass on a sentence that merely CONTAINS the expected text.
fn assert_refusal(case: &str, status: u16, body: &str, code: &str, message: &str) {
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status} with body {body}"
    );
    let got_code = error_code(body);
    assert_eq!(
        got_code, code,
        "case {case:?}: expected error code {code:?}, got {got_code:?} with body {body}"
    );
    let got_message = error_message(body);
    assert_eq!(
        got_message, message,
        "case {case:?}: expected message {message:?}, got {got_message:?} with body {body}"
    );
}

// ---------------------------------------------------------------------------
// A launch template name the account already holds
// ---------------------------------------------------------------------------

/// The whole sentence EC2 answers a create under a launch template name already taken.
///
/// FIXED text: the class interpolates nothing, so the name the request sent is NOT part of
/// it — a create names one `LaunchTemplateName`, and that is the value the caller already
/// holds.
const LAUNCH_TEMPLATE_NAME_TAKEN: &str = "Launch template name already in use.";

/// One `LaunchTemplateData.<member>` form field.
fn launch_data(member: &str, value: &str) -> Param {
    param(&format!("LaunchTemplateData.{member}"), value)
}

/// The names the duplicate-launch-template table is written against.
///
/// They differ along everything the rule generalises over for a name: its LENGTH, at both
/// published bounds ([`MIN_LAUNCH_TEMPLATE_NAME_CHARS`] and
/// [`MAX_LAUNCH_TEMPLATE_NAME_CHARS`]), and its CHARACTER CLASSES — letters, digits and each
/// mark [`LAUNCH_TEMPLATE_NAME_PUNCTUATION`] admits. None of them is the name the recorded
/// instance carried.
fn taken_launch_template_names() -> Vec<String> {
    let mut punctuated = String::from("lt");
    for mark in LAUNCH_TEMPLATE_NAME_PUNCTUATION.chars() {
        punctuated.push(mark);
        punctuated.push('7');
    }
    vec![
        "a".repeat(MIN_LAUNCH_TEMPLATE_NAME_CHARS),
        "z".repeat(MAX_LAUNCH_TEMPLATE_NAME_CHARS),
        "0123456789".to_string(),
        punctuated,
    ]
}

/// A create under a name the account already holds answers the class's FIXED sentence, which
/// repeats no value back.
#[tokio::test]
async fn a_taken_launch_template_name_answers_the_classes_fixed_sentence() {
    let router = emulator();

    for name in taken_launch_template_names() {
        let case = format!("a second launch template named {name:?}");
        let (status, body) = api(
            &router,
            "CreateLaunchTemplate",
            &[
                param("LaunchTemplateName", &name),
                launch_data("ImageId", "ami-00000000000000001"),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the first create must succeed, got {status} with body {body}"
        );

        // A DIFFERENT request under the same name, carrying no ClientToken, so this is a
        // fresh create rather than an idempotent repeat.
        let (status, body) = api(
            &router,
            "CreateLaunchTemplate",
            &[
                param("LaunchTemplateName", &name),
                launch_data("InstanceType", "t3.large"),
            ],
        )
        .await;
        assert_refusal(
            &case,
            status,
            &body,
            "InvalidLaunchTemplateName.AlreadyExistsException",
            LAUNCH_TEMPLATE_NAME_TAKEN,
        );
        // The class interpolates nothing: the name the caller sent is not repeated back.
        let message = error_message(&body);
        assert!(
            !message.contains(&name),
            "case {case:?}: the sentence must not repeat the name back, got {message:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// A placement group name the account already holds
// ---------------------------------------------------------------------------

/// The names the duplicate-placement-group table is written against: they differ in LENGTH,
/// up to the published maximum ([`MAX_PLACEMENT_GROUP_NAME_CHARS`]), and in CHARACTER
/// CLASSES — "Up to 255 ASCII characters" (`CreatePlacementGroupRequest$GroupName`) admits
/// spaces and marks that have to survive XML escaping on the way back out.
fn taken_placement_group_names() -> Vec<String> {
    vec![
        "p".to_string(),
        "g".repeat(MAX_PLACEMENT_GROUP_NAME_CHARS),
        "group.with-mixed_chars+1".to_string(),
        "group with spaces & <punctuation>".to_string(),
    ]
}

/// A create under a placement group name the account already holds names the group by the
/// name the request sent, in a sentence that terminates with a period.
#[tokio::test]
async fn a_taken_placement_group_name_is_named_in_the_refusal() {
    let router = emulator();

    // The name is what must be unique, not the rest of the request, so the second create
    // walks every strategy: the sentence is the same whichever one it names.
    for name in taken_placement_group_names() {
        let case = format!("a second placement group named {name:?}");
        let (status, body) = api(
            &router,
            "CreatePlacementGroup",
            &[param("GroupName", &name), param("Strategy", "cluster")],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the first create must succeed, got {status} with body {body}"
        );

        for strategy in ["cluster", "spread", "partition"] {
            let case = format!("{case} as {strategy}");
            let (status, body) = api(
                &router,
                "CreatePlacementGroup",
                &[param("GroupName", &name), param("Strategy", strategy)],
            )
            .await;
            assert_refusal(
                &case,
                status,
                &body,
                "InvalidPlacementGroup.Duplicate",
                &format!("The placement group '{name}' already exists."),
            );
        }
    }
}

// ---------------------------------------------------------------------------
// A security group name the VPC already holds
// ---------------------------------------------------------------------------

/// One row of the duplicate-security-group table.
struct GroupRow {
    /// What the row covers.
    case: &'static str,
    /// The CIDR of a VPC the create names, or `None` to let the create fall back to the
    /// Region's default VPC — the fall-back whose chosen VPC the caller learns only from
    /// this sentence.
    vpc_cidr: Option<&'static str>,
    /// The name the first create stores.
    stored_name: &'static str,
    /// The name the second create sends: the stored one, or another spelling of it, since
    /// "names are case-insensitive" (`CreateSecurityGroupRequest$GroupName`).
    second_name: &'static str,
}

/// The rows the duplicate-security-group table is written against: they differ in which VPC
/// scopes the name (one the request names, and the Region's default), in the name's length
/// and character classes (`SECURITY_GROUP_PUNCTUATION`), and in whether the second create
/// spells the name as the first did.
fn group_rows() -> Vec<GroupRow> {
    vec![
        GroupRow {
            case: "a VPC the request names",
            vpc_cidr: Some("10.20.0.0/16"),
            stored_name: "probe-named-vpc",
            second_name: "probe-named-vpc",
        },
        GroupRow {
            case: "the Region's default VPC, which the create falls back to",
            vpc_cidr: None,
            stored_name: "probe-default-vpc",
            second_name: "probe-default-vpc",
        },
        GroupRow {
            case: "a name spelled in another case, matched case-insensitively",
            vpc_cidr: Some("10.21.0.0/16"),
            stored_name: "probe-mixed-case",
            second_name: "PROBE-Mixed-Case",
        },
        GroupRow {
            case: "a one-character name",
            vpc_cidr: Some("10.22.0.0/16"),
            stored_name: "s",
            second_name: "s",
        },
        GroupRow {
            case: "a name carrying the punctuation the alphabet admits",
            vpc_cidr: Some("10.23.0.0/16"),
            // Each mark from `SECURITY_GROUP_PUNCTUATION`, which is the documented alphabet
            // this name is held to; asserted below to be within it.
            stored_name: "sg._-:/()#,@[]+=&;{}!$*",
            second_name: "sg._-:/()#,@[]+=&;{}!$*",
        },
    ]
}

/// `CreateVpc` for `cidr`, as the `VpcId` it minted.
async fn create_vpc(case: &str, router: &MultiServiceRouter, cidr: &str) -> String {
    let (status, body) = api(router, "CreateVpc", &[param("CidrBlock", cidr)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CreateVpc, got {status} with body {body}"
    );
    element(&body, "vpcId").unwrap_or_else(|| panic!("case {case:?}: no <vpcId> in {body}"))
}

/// The `vpcId` `DescribeSecurityGroups` reports for `group_id` — the VPC the create really
/// put the group in, read back out of the store rather than assumed.
async fn described_vpc_of(case: &str, router: &MultiServiceRouter, group_id: &str) -> String {
    let (status, body) = api(
        router,
        "DescribeSecurityGroups",
        &[param("GroupId.1", group_id)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeSecurityGroups, got {status} \
         with body {body}"
    );
    let parsed = document(case, &body);
    let group = parsed
        .descendants()
        .find(|node| node.is_element() && child(*node, "groupName").is_some())
        .unwrap_or_else(|| panic!("case {case:?}: no security group in {body}"));
    child(group, "vpcId")
        .and_then(|node| node.text().map(str::to_string))
        .unwrap_or_else(|| panic!("case {case:?}: the group reports no vpcId in {body}"))
}

/// A second security group under a name its VPC already holds is refused naming BOTH the
/// name the request sent and the VPC that scopes it — including when the VPC is the one the
/// create fell back to.
#[tokio::test]
async fn a_taken_security_group_name_is_named_with_the_vpc_that_scopes_it() {
    for row in group_rows() {
        let router = emulator();
        let case = row.case;

        // Every mark of a row's name is within the documented alphabet, so a refusal below
        // is the DUPLICATE one rather than a complaint about the name itself.
        assert!(
            row.stored_name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || SECURITY_GROUP_PUNCTUATION.contains(c)),
            "case {case:?}: the row's name {:?} must be within the documented alphabet",
            row.stored_name
        );

        let mut first = vec![
            param("GroupName", row.stored_name),
            param("GroupDescription", "a group this suite refuses a twin of"),
        ];
        if let Some(cidr) = row.vpc_cidr {
            let vpc_id = create_vpc(case, &router, cidr).await;
            first.push(param("VpcId", &vpc_id));
        }
        let (status, body) = api(&router, "CreateSecurityGroup", &first).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the first create must succeed, got {status} with body {body}"
        );
        let group_id =
            element(&body, "groupId").unwrap_or_else(|| panic!("case {case:?}: no id in {body}"));
        // The VPC the group really landed in: the one the request named, or the one the
        // fall-back chose.
        let vpc_id = described_vpc_of(case, &router, &group_id).await;

        let mut second = vec![
            param("GroupName", row.second_name),
            param("GroupDescription", "the twin"),
        ];
        if row.vpc_cidr.is_some() {
            second.push(param("VpcId", &vpc_id));
        }
        let (status, body) = api(&router, "CreateSecurityGroup", &second).await;
        assert_refusal(
            case,
            status,
            &body,
            "InvalidGroup.Duplicate",
            &format!(
                "The security group '{name}' already exists for VPC '{vpc_id}'",
                name = row.second_name,
            ),
        );

        // The same name in ANOTHER VPC is no conflict at all, which is why the sentence has
        // to carry the VPC: the name alone does not identify the group it collided with.
        let elsewhere = create_vpc(case, &router, "172.31.240.0/20").await;
        let (status, body) = api(
            &router,
            "CreateSecurityGroup",
            &[
                param("GroupName", row.second_name),
                param("GroupDescription", "the same name, another VPC"),
                param("VpcId", &elsewhere),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the same name in another VPC must be accepted, got {status} \
             with body {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// Identifiers the service cannot read
// ---------------------------------------------------------------------------

/// The ways a value falls OUTSIDE the long id form this service issues under a prefix: the
/// all-zero placeholder, a leading digit above zero, too few and too many digits, a
/// non-hexadecimal digit, upper case, another resource's prefix, and no suffix at all.
///
/// `prefix` is the id prefix of the resource under test, so one catalog of FORMS serves every
/// resource kind below rather than each suite listing its own.
fn unreadable_ids(prefix: &str) -> Vec<String> {
    vec![
        format!("{prefix}-00000000000000000"),
        format!("{prefix}-1234567890abcdef0"),
        format!("{prefix}-0abcdef12"),
        format!("{prefix}-0123456789abcdef12"),
        format!("{prefix}-0123456789abcdefg"),
        format!("{prefix}-0123456789ABCDEF0"),
        format!("x{prefix}-0123456789abcdef0"),
        format!("{prefix}-"),
        format!("not-a-{prefix}-id"),
    ]
}

/// One `CreateCapacityReservation` for capacity in [`AVAILABILITY_ZONE`], as the id it
/// minted. An immediate create is `active`, so the reservation is one a cancel and a modify
/// both reach their own parameter checks through.
async fn reserve(case: &str, router: &MultiServiceRouter) -> String {
    let (status, body) = api(
        router,
        "CreateCapacityReservation",
        &[
            param("InstanceType", INSTANCE_TYPE),
            param("InstancePlatform", INSTANCE_PLATFORM),
            param("InstanceCount", "1"),
            param("AvailabilityZone", AVAILABILITY_ZONE),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: the reservation must be created, got {status} with body {body}"
    );
    element(&body, "capacityReservationId")
        .unwrap_or_else(|| panic!("case {case:?}: no reservation id in {body}"))
}

/// Every operation that takes a CANCELLATION QUOTE id answers one sentence for a value it
/// cannot read as one, naming the quote by that value.
#[tokio::test]
async fn an_unreadable_cancellation_quote_id_answers_the_quotes_sentence() {
    let router = emulator();
    let reservation_id = reserve("the reservation a cancel names", &router).await;

    for quote_id in unreadable_ids("crq") {
        // The two operations that take a cancellation quote id: the describe of the quotes
        // themselves, and the cancel that spends one.
        let calls: Vec<(&str, Vec<Param>)> = vec![
            (
                "DescribeCapacityReservationCancellationQuotes",
                indexed("CapacityReservationCancellationQuoteId", &[&quote_id]),
            ),
            (
                "CancelCapacityReservation",
                vec![
                    param("CapacityReservationId", &reservation_id),
                    param("QuoteId", &quote_id),
                ],
            ),
        ];
        for (action, params) in calls {
            let case = format!("{action} with the quote id {quote_id:?}");
            let (status, body) = api(&router, action, &params).await;
            assert_refusal(
                &case,
                status,
                &body,
                "InvalidCapacityReservationCancellationQuoteId.Malformed",
                &format!("Cancellation Quote Id '{quote_id}' is malformed."),
            );
        }
    }

    // Nothing above spent the reservation: a refused cancel leaves it as it was.
    let (status, body) = api(
        &router,
        "DescribeCapacityReservations",
        &[param("CapacityReservationId.1", &reservation_id)],
    )
    .await;
    assert_eq!(
        status, 200,
        "the reservation must still be describable, got {status} with body {body}"
    );
    let state = element(&body, "state").unwrap_or_default();
    assert_eq!(
        state, "active",
        "a refused cancel must leave the reservation active, got {state:?} in {body}"
    );
}

/// A MODIFICATION quote id the service cannot read is named in the same shape of sentence as
/// the cancellation quote's, with this quote's own label.
#[tokio::test]
async fn an_unreadable_modification_quote_id_answers_the_quotes_sentence() {
    let router = emulator();
    let reservation_id = reserve("the reservation a modify names", &router).await;

    for quote_id in unreadable_ids("crmq") {
        let case = format!("ModifyCapacityReservation with the quote id {quote_id:?}");
        let (status, body) = api(
            &router,
            "ModifyCapacityReservation",
            &[
                param("CapacityReservationId", &reservation_id),
                param("QuoteId", &quote_id),
                param("AcceptModificationTerms", "true"),
            ],
        )
        .await;
        assert_refusal(
            &case,
            status,
            &body,
            "InvalidCapacityReservationModificationQuoteId.Malformed",
            &format!("Modification Quote Id '{quote_id}' is malformed."),
        );
    }
}

/// A Capacity Reservation id the service cannot read is named in the sentence of the
/// OPERATION that refuses it: the topology's own, which differs from the one the
/// reservation's other validators answer for the very same value.
#[tokio::test]
async fn an_unreadable_reservation_id_answers_the_refusing_operations_sentence() {
    let router = emulator();

    for reservation_id in unreadable_ids("cr") {
        let case = format!("DescribeCapacityReservationTopology with {reservation_id:?}");
        let (status, body) = api(
            &router,
            "DescribeCapacityReservationTopology",
            &indexed("CapacityReservationId", &[&reservation_id]),
        )
        .await;
        assert_refusal(
            &case,
            status,
            &body,
            "InvalidCapacityReservationId.Malformed",
            &format!("Capacity Reservation Id '{reservation_id}' is malformed."),
        );

        // The SAME value through an operation whose own validator words it the other way:
        // the two sentences belong to the two operations, not to the resource.
        let case = format!("AcceptCapacityReservationBillingOwnership with {reservation_id:?}");
        let (status, body) = api(
            &router,
            "AcceptCapacityReservationBillingOwnership",
            &[param("CapacityReservationId", &reservation_id)],
        )
        .await;
        assert_refusal(
            &case,
            status,
            &body,
            "InvalidCapacityReservationId.Malformed",
            &format!("Capacity Reservation ID '{reservation_id}' is malformed"),
        );
    }
}
