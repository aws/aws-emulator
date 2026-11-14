// SPDX-License-Identifier: CC0-1.0

//! What the three interface-trunking operations answer, end to end over the wire the
//! service actually speaks: `ec2Query`, an `application/x-www-form-urlencoded` POST whose
//! body carries `Action=<Operation>`, answered with XML (README.md / OPERATIONS.md "Wire
//! protocol"). ec2Query is selected by the `ec2` signing name in the request's SigV4
//! credential scope, so every call here carries one.
//!
//! The rule under test: interface trunking — joining a `branch` network interface to a
//! `trunk` one, reporting those joins and taking one down — is a capability EC2 grants an
//! ACCOUNT out of band, so a caller whose account does not hold it is refused EVERY
//! operation the capability gates — HTTP 400 `OperationNotPermitted`, naming the account the
//! request was made by — and told nothing about the interfaces, the associations, the VLAN
//! id or the GRE key the request named. The refusal therefore does not depend on the
//! request: it comes before any member of it is read, so it is the answer whatever the
//! request carries and in whichever Region it was made. For the reader that means a REFUSAL
//! and not an empty page: an account outside the capability is not told that it holds no
//! associations, it is told it may not ask.
//!
//! That is what the table below is: each row is a distinct input along what the rule
//! generalises over — which interfaces the request names (two the account really holds,
//! created through `CreateNetworkInterface` with the `trunk` and `branch` types this
//! operation documents as its prerequisite; an id that names no interface; something that is
//! not an interface id at all; two empty ids), which protocol member carries the association
//! (`VlanId`, `GreKey`, neither, both, a VLAN id outside any VLAN's range) and whether a
//! `ClientToken` is given and repeated — and no row
//! is the specification. One case asserts the same refusal for a request made in another
//! Region, because the capability is a fact of the account rather than of a Region.
//!
//! `DryRun` is the one member read ahead of that refusal, so it has a table of its own: the
//! flag asks what the CALLER may do rather than what this account's capability admits, and
//! all three operations answer it 412 `DryRunOperation`.
//!
//! The interfaces the rows name ARE created over the wire, through the implemented
//! `CreateNetworkInterface`, and read back through the implemented
//! `DescribeNetworkInterfaces` after every refused call: a refused association stores
//! nothing, so the account's interfaces have to read back exactly as they were created.
//!
//! `DescribeTrunkInterfaceAssociations` and `DisassociateTrunkInterface` get a table each,
//! below, varying what THEY generalise over — the association ids named, the filters and the
//! page asked for — because the capability gates the
//! operation rather than any one of its members.

use ec2::state::ComAmazonawsEc2Amazonec2State as Ec2State;
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{element, elements, error_code, error_message, filters, indexed};
use super::wire::{Param, param};

/// The Region the fixture's interfaces are created in: the account's default VPC and its
/// default subnets are seeded for the emulator's default Region.
const REGION: &str = "us-east-1";

/// A Region the fixture created nothing in, to show the refusal is a fact of the ACCOUNT
/// rather than of the Region a request was made in.
const OTHER_REGION: &str = "eu-west-1";

/// The EC2 error code a caller that may not use interface trunking reads.
const OPERATION_NOT_PERMITTED: &str = "OperationNotPermitted";

/// The HTTP status that code is answered with.
const REFUSAL_STATUS: u16 = 400;

/// What a `DryRun` request a caller holds the action's permissions for is answered with, and
/// the status EC2 carries it on — the permitted side of the probe, answered ahead of the
/// account-level capability the three operations are gated by.
const DRY_RUN_CODE: &str = "DryRunOperation";
const DRY_RUN_STATUS: u16 = 412;
const DRY_RUN_MESSAGE: &str = "Request would have succeeded, but DryRun flag is set.";

/// The interface types this operation documents as its prerequisite: "use
/// CreateNetworkInterface command and set the interface type to trunk. You must also create
/// a network interface for each branch network interface that you want to associate"
/// (`AssociateTrunkInterface`). Both are members of `NetworkInterfaceCreationType`
/// (`ec2::ops::types::NETWORK_INTERFACE_CREATION_TYPE_VALUES`).
const TRUNK_TYPE: &str = "trunk";
const BRANCH_TYPE: &str = "branch";

/// The type an interface created without naming one carries, so one row can name an
/// interface that is neither a trunk nor a branch: "The type of network interface. The
/// default is interface" (`CreateNetworkInterfaceRequest$InterfaceType`, whose enum
/// `NetworkInterfaceCreationType` does not admit this value — it is the DEFAULT, asked for
/// by naming no type at all).
const PLAIN_TYPE: &str = "interface";

/// The message EC2 refuses the operation with, spelled out here rather than taken from the
/// crate: only its one variable part — the account the request was made by, which is the
/// single account this emulator serves — comes from the emulator.
fn expected_message() -> String {
    format!(
        "User {account} is not permitted to perform this operation",
        account = ::engine::emulator_account_id()
    )
}

fn emulator() -> (MultiServiceRouter, std::sync::Arc<engine::InMemoryEngine>) {
    super::ec2_query::emulator()
}

/// One `AssociateTrunkInterface` call carrying `params`, made in `region`.
async fn associate(
    router: &MultiServiceRouter,
    region: &str,
    params: &[Param],
) -> (u16, String) {
    super::ec2_query::call(router, region, "AssociateTrunkInterface", params).await
}

/// One `DescribeTrunkInterfaceAssociations` call carrying `params`, made in `region`.
async fn describe_associations(
    router: &MultiServiceRouter,
    region: &str,
    params: &[Param],
) -> (u16, String) {
    super::ec2_query::call(router, region, "DescribeTrunkInterfaceAssociations", params).await
}

/// One `DisassociateTrunkInterface` call carrying `params`, made in `region`.
async fn disassociate(
    router: &MultiServiceRouter,
    region: &str,
    params: &[Param],
) -> (u16, String) {
    super::ec2_query::call(router, region, "DisassociateTrunkInterface", params).await
}

/// The first default subnet of the account's default VPC — a subnet the store really holds,
/// so an interface created into it is one the service itself minted.
fn default_subnet(engine: &engine::InMemoryEngine) -> String {
    let state = engine.state::<Ec2State>();
    let store = state.lock();
    let vpc_id = store
        .default_vpc_id(REGION)
        .expect("a fresh account has its default VPC");
    store
        .subnets
        .iter()
        .find(|(_, subnet)| subnet.vpc_id.as_deref() == Some(vpc_id.as_str()))
        .map(|(subnet_id, _)| subnet_id.clone())
        .expect("the default VPC comes with its default subnets")
}

/// Create one network interface through the API and answer its id: of the type
/// `interface_type` names, or of the default type when it names none.
async fn create_interface(
    router: &MultiServiceRouter,
    subnet_id: &str,
    interface_type: Option<&str>,
) -> String {
    let described = interface_type.unwrap_or(PLAIN_TYPE);
    let mut params = vec![
        param("SubnetId", subnet_id),
        param("Description", &format!("a {described} interface")),
    ];
    if let Some(interface_type) = interface_type {
        params.push(param("InterfaceType", interface_type));
    }
    let (status, body) =
        super::ec2_query::call(router, REGION, "CreateNetworkInterface", &params).await;
    assert_eq!(
        status, 200,
        "CreateNetworkInterface({described}) must succeed, got {status} with body {body}"
    );
    element(&body, "networkInterfaceId")
        .expect("CreateNetworkInterface reports the networkInterfaceId it minted")
}

/// Every interface the account holds, as `(id, interfaceType, status)` triples in the order
/// `DescribeNetworkInterfaces` reports them: what a refused association must leave untouched.
async fn interface_report(router: &MultiServiceRouter, case: &str) -> Vec<(String, String, String)> {
    let (status, body) = super::ec2_query::call(router, REGION, "DescribeNetworkInterfaces", &[]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeNetworkInterfaces must succeed, got {status} with body {body}"
    );
    let ids = elements(&body, "networkInterfaceId");
    let types = elements(&body, "interfaceType");
    let statuses = elements(&body, "status");
    assert_eq!(
        (ids.len(), types.len()),
        (statuses.len(), statuses.len()),
        "case {case:?}: every reported interface carries an id, a type and a status, got {body}"
    );
    ids.into_iter()
        .zip(types)
        .zip(statuses)
        .map(|((id, interface_type), status)| (id, interface_type, status))
        .collect()
}

/// Assert one call was refused with the account-level refusal, naming the case and what the
/// service actually answered.
fn assert_refused(case: &str, status: u16, body: &str) {
    assert_eq!(
        status, REFUSAL_STATUS,
        "case {case:?}: expected HTTP {REFUSAL_STATUS}, got {status} with body {body}"
    );
    assert_eq!(
        error_code(body),
        OPERATION_NOT_PERMITTED,
        "case {case:?}: expected the error code {OPERATION_NOT_PERMITTED:?}, got {:?} with \
         body {body}",
        error_code(body)
    );
    assert_eq!(
        error_message(body),
        expected_message(),
        "case {case:?}: expected the message {:?}, got {:?}",
        expected_message(),
        error_message(body)
    );
}

/// Every request this operation can be asked, whatever it names, is refused the operation —
/// and leaves the account's interfaces as they were.
#[tokio::test]
async fn every_association_request_is_refused_as_not_permitted() {
    let (router, engine) = emulator();
    let subnet_id = default_subnet(&engine);
    let trunk = create_interface(&router, &subnet_id, Some(TRUNK_TYPE)).await;
    let branch = create_interface(&router, &subnet_id, Some(BRANCH_TYPE)).await;
    let plain = create_interface(&router, &subnet_id, None).await;
    let before = interface_report(&router, "before any association request").await;
    for (id, expected_type) in [
        (&trunk, TRUNK_TYPE),
        (&branch, BRANCH_TYPE),
        (&plain, PLAIN_TYPE),
    ] {
        assert!(
            before
                .iter()
                .any(|(reported, interface_type, _)| reported == id
                    && interface_type == expected_type),
            "the fixture's {expected_type} interface {id} must be reported before any \
             association request, got {before:?}"
        );
    }

    // Each row varies one factor of the request: which interfaces it names, which protocol
    // member carries the association and the idempotency token.
    let rows: Vec<(&str, Vec<Param>)> = vec![
        (
            "the account's own trunk and branch interfaces, VLAN protocol",
            vec![
                param("TrunkInterfaceId", &trunk),
                param("BranchInterfaceId", &branch),
                param("VlanId", "101"),
            ],
        ),
        (
            "the same pair, GRE protocol",
            vec![
                param("TrunkInterfaceId", &trunk),
                param("BranchInterfaceId", &branch),
                param("GreKey", "7"),
            ],
        ),
        (
            "the same pair, naming no protocol member at all",
            vec![
                param("TrunkInterfaceId", &trunk),
                param("BranchInterfaceId", &branch),
            ],
        ),
        (
            "the same pair, naming both protocol members",
            vec![
                param("TrunkInterfaceId", &trunk),
                param("BranchInterfaceId", &branch),
                param("VlanId", "4094"),
                param("GreKey", "4294967"),
            ],
        ),
        (
            "a VLAN id no VLAN carries",
            vec![
                param("TrunkInterfaceId", &trunk),
                param("BranchInterfaceId", &branch),
                param("VlanId", "999999"),
            ],
        ),
        (
            "a plain interface named as the trunk",
            vec![
                param("TrunkInterfaceId", &plain),
                param("BranchInterfaceId", &branch),
                param("VlanId", "12"),
            ],
        ),
        (
            "the branch and the trunk the other way round",
            vec![
                param("TrunkInterfaceId", &branch),
                param("BranchInterfaceId", &trunk),
                param("VlanId", "12"),
            ],
        ),
        (
            "one interface named as both sides",
            vec![
                param("TrunkInterfaceId", &trunk),
                param("BranchInterfaceId", &trunk),
                param("VlanId", "12"),
            ],
        ),
        (
            "a well-formed id the account holds no interface for",
            vec![
                param("TrunkInterfaceId", "eni-0123456789abcdef0"),
                param("BranchInterfaceId", &branch),
                param("VlanId", "12"),
            ],
        ),
        (
            "a branch id that is not an interface id at all",
            vec![
                param("TrunkInterfaceId", &trunk),
                param("BranchInterfaceId", "not-an-interface-id"),
                param("VlanId", "12"),
            ],
        ),
        (
            "both ids present but empty",
            vec![
                param("TrunkInterfaceId", ""),
                param("BranchInterfaceId", ""),
            ],
        ),
        (
            "an idempotency token, first use",
            vec![
                param("TrunkInterfaceId", &trunk),
                param("BranchInterfaceId", &branch),
                param("VlanId", "77"),
                param("ClientToken", "trunk-association-token"),
            ],
        ),
        (
            "the same idempotency token again",
            vec![
                param("TrunkInterfaceId", &trunk),
                param("BranchInterfaceId", &branch),
                param("VlanId", "77"),
                param("ClientToken", "trunk-association-token"),
            ],
        ),
    ];

    for (case, params) in &rows {
        let (status, body) = associate(&router, REGION, params).await;
        assert_refused(case, status, &body);
        assert!(
            element(&body, "interfaceAssociation").is_none()
                && element(&body, "associationId").is_none(),
            "case {case:?}: a refused request reports no association, got {body}"
        );
    }

    // The capability belongs to the account, not to a Region, so the same request made in
    // another Region reads the same refusal.
    let (status, body) = associate(
        &router,
        OTHER_REGION,
        &[
            param("TrunkInterfaceId", &trunk),
            param("BranchInterfaceId", &branch),
            param("VlanId", "101"),
        ],
    )
    .await;
    assert_refused("the same pair, asked in another Region", status, &body);

    // Nothing was stored: every interface the account holds reads back exactly as the
    // creates left it, with the type it was created with and the status of an interface
    // nothing holds.
    let after = interface_report(&router, "after every association request").await;
    assert_eq!(
        after, before,
        "a refused association stores nothing, so the account's interfaces must read back \
         unchanged: expected {before:?}, got {after:?}"
    );
    for (id, expected_type) in [
        (&trunk, TRUNK_TYPE),
        (&branch, BRANCH_TYPE),
        (&plain, PLAIN_TYPE),
    ] {
        assert!(
            after
                .iter()
                .any(|(reported, interface_type, _)| reported == id
                    && interface_type == expected_type),
            "interface {id} must still read back as a {expected_type} interface, got {after:?}"
        );
    }
}

/// Assert a refused call reported nothing about an association: the reader's result list and
/// its page token, and the id either side of the pair reports, are all absent.
fn assert_reports_no_association(case: &str, body: &str) {
    for absent in [
        "interfaceAssociationSet",
        "interfaceAssociation",
        "associationId",
        "nextToken",
        "return",
    ] {
        assert!(
            element(body, absent).is_none(),
            "case {case:?}: a refused request reports no <{absent}>, got {body}"
        );
    }
}

/// Every `DescribeTrunkInterfaceAssociations` request, whatever it asks for, is refused the
/// operation — not answered with an empty page — and changes nothing a later caller reads.
#[tokio::test]
async fn every_association_report_request_is_refused_as_not_permitted() {
    let (router, engine) = emulator();
    let subnet_id = default_subnet(&engine);
    let trunk = create_interface(&router, &subnet_id, Some(TRUNK_TYPE)).await;
    let branch = create_interface(&router, &subnet_id, Some(BRANCH_TYPE)).await;
    let before = interface_report(&router, "before any report request").await;

    // Each row varies one factor of the request: which associations it selects by id, which
    // filter selects them and the page asked for. The
    // two filter names are the ones the operation documents: "gre-key - The ID of a trunk
    // interface association. interface-protocol - The interface protocol. Valid values are
    // VLAN and GRE" (`DescribeTrunkInterfaceAssociationsRequest$Filters`).
    //
    // `MaxResults` is asked for at each of the bounds the model publishes for it
    // (`DescribeTrunkInterfaceAssociationsMaxResults`, range 5..255) and once between them.
    // A value OUTSIDE that range is not a row here: the modelled constraint is refused by
    // the boundary validator before this handler runs, so such a request says nothing about
    // the account-level rule under test.
    let rows: Vec<(&str, Vec<Param>)> = vec![
        ("naming nothing at all", vec![]),
        (
            "one association id",
            indexed("AssociationId", &["tia-0123456789abcdef0"]),
        ),
        (
            "three association ids",
            indexed(
                "AssociationId",
                &[
                    "tia-0123456789abcdef0",
                    "tia-00112233445566778",
                    "tia-aaaabbbbccccddddd",
                ],
            ),
        ),
        (
            "an association id that is not an id at all",
            indexed("AssociationId", &["not-an-association-id"]),
        ),
        (
            "an association id present but empty",
            indexed("AssociationId", &[""]),
        ),
        ("the gre-key filter", filters(&[("gre-key", &["7"])])),
        (
            "the interface-protocol filter, VLAN",
            filters(&[("interface-protocol", &["VLAN"])]),
        ),
        (
            "the interface-protocol filter, GRE",
            filters(&[("interface-protocol", &["GRE"])]),
        ),
        (
            "both documented filters together",
            filters(&[
                ("gre-key", &["7", "8"]),
                ("interface-protocol", &["VLAN", "GRE"]),
            ]),
        ),
        (
            "a filter name the operation documents none of",
            filters(&[("not-a-filter-this-operation-has", &["anything"])]),
        ),
        (
            "the smallest page the model admits",
            vec![param("MaxResults", "5")],
        ),
        (
            "the largest page the model admits",
            vec![param("MaxResults", "255")],
        ),
        (
            "a page in between, continued from a token",
            vec![
                param("MaxResults", "100"),
                param("NextToken", "a-token-no-page-of-this-account-handed-out"),
            ],
        ),
        (
            "a continuation token alone",
            vec![param("NextToken", "another-token")],
        ),
        (
            "ids, a filter and a page together",
            {
                let mut params = indexed("AssociationId", &["tia-0123456789abcdef0"]);
                params.extend(filters(&[("interface-protocol", &["VLAN"])]));
                params.push(param("MaxResults", "5"));
                params
            },
        ),
    ];

    for (case, params) in &rows {
        let (status, body) = describe_associations(&router, REGION, params).await;
        assert_refused(case, status, &body);
        assert_reports_no_association(case, &body);
    }

    // A report is a read, so asking twice answers twice the same way: the first call neither
    // consumed anything nor moved anything the second one would have seen.
    let (first_status, first_body) = describe_associations(&router, REGION, &[]).await;
    let (second_status, second_body) = describe_associations(&router, REGION, &[]).await;
    assert_eq!(
        (first_status, error_code(&first_body), error_message(&first_body)),
        (second_status, error_code(&second_body), error_message(&second_body)),
        "case {:?}: a report changes nothing, so the second call must answer exactly as the \
         first: got {first_status} {first_body} then {second_status} {second_body}",
        "the same report asked twice"
    );

    // The capability belongs to the account, not to a Region, so the same request made in
    // another Region reads the same refusal.
    let (status, body) = describe_associations(&router, OTHER_REGION, &[]).await;
    assert_refused("a report asked in another Region", status, &body);

    // And the account's interfaces read back exactly as the creates left them: a refused
    // report stores nothing, and an association request after one is still refused.
    let after = interface_report(&router, "after every report request").await;
    assert_eq!(
        after, before,
        "a refused report stores nothing, so the account's interfaces must read back \
         unchanged: expected {before:?}, got {after:?}"
    );
    let (status, body) = associate(
        &router,
        REGION,
        &[
            param("TrunkInterfaceId", &trunk),
            param("BranchInterfaceId", &branch),
            param("VlanId", "101"),
        ],
    )
    .await;
    assert_refused("an association asked after a refused report", status, &body);
}

/// Every `DisassociateTrunkInterface` request, whatever association it names, is refused the
/// operation and leaves the account's interfaces as they were.
#[tokio::test]
async fn every_disassociation_request_is_refused_as_not_permitted() {
    let (router, engine) = emulator();
    let subnet_id = default_subnet(&engine);
    let trunk = create_interface(&router, &subnet_id, Some(TRUNK_TYPE)).await;
    let branch = create_interface(&router, &subnet_id, Some(BRANCH_TYPE)).await;
    let before = interface_report(&router, "before any disassociation request").await;

    // Each row varies one factor of the request: the shape of the association id it names and
    // whether an idempotency token is given and repeated. No id
    // can name an association the account holds, because the operation that would have made
    // one is refused the same way.
    let rows: Vec<(&str, Vec<Param>)> = vec![
        (
            "an association id in the form the API reports",
            vec![param("AssociationId", "tia-0123456789abcdef0")],
        ),
        (
            "another id of the same form",
            vec![param("AssociationId", "tia-00112233445566778")],
        ),
        (
            "an id that is not an association id at all",
            vec![param("AssociationId", "not-an-association-id")],
        ),
        (
            "an id that names one of the account's own interfaces instead",
            vec![param("AssociationId", &trunk)],
        ),
        (
            "an id present but empty",
            vec![param("AssociationId", "")],
        ),
        (
            "an idempotency token, first use",
            vec![
                param("AssociationId", "tia-0123456789abcdef0"),
                param("ClientToken", "trunk-disassociation-token"),
            ],
        ),
        (
            "the same idempotency token again",
            vec![
                param("AssociationId", "tia-0123456789abcdef0"),
                param("ClientToken", "trunk-disassociation-token"),
            ],
        ),
    ];

    for (case, params) in &rows {
        let (status, body) = disassociate(&router, REGION, params).await;
        assert_refused(case, status, &body);
        assert_reports_no_association(case, &body);
    }

    // The capability belongs to the account, not to a Region.
    let (status, body) = disassociate(
        &router,
        OTHER_REGION,
        &[param("AssociationId", "tia-0123456789abcdef0")],
    )
    .await;
    assert_refused("the same id, asked in another Region", status, &body);

    // Nothing was removed and nothing was stored: the account's interfaces read back exactly
    // as the creates left them, and the branch interface is still a branch.
    let after = interface_report(&router, "after every disassociation request").await;
    assert_eq!(
        after, before,
        "a refused disassociation changes nothing, so the account's interfaces must read \
         back unchanged: expected {before:?}, got {after:?}"
    );
    assert!(
        after
            .iter()
            .any(|(reported, interface_type, _)| reported == &branch
                && interface_type == BRANCH_TYPE),
        "interface {branch} must still read back as a {BRANCH_TYPE} interface, got {after:?}"
    );
}

/// `DryRun` is answered AHEAD of the capability refusal, on all three operations: the flag
/// asks "whether you have the required permissions for the action, without actually making
/// the request" — a question about the CALLER, which the account-level capability is not — so
/// a dry run reads 412 `DryRunOperation` and changes nothing.
#[tokio::test]
async fn a_dry_run_reads_the_permission_verdict_rather_than_the_capability_refusal() {
    let (router, engine) = emulator();
    let subnet_id = default_subnet(&engine);
    let trunk = create_interface(&router, &subnet_id, Some(TRUNK_TYPE)).await;
    let branch = create_interface(&router, &subnet_id, Some(BRANCH_TYPE)).await;
    let before = interface_report(&router, "before any dry run").await;

    // The rows span the three operations and, per operation, the kinds of input the rule
    // generalises over: a request naming resources the account holds, one naming resources it
    // does not, one whose ids are not ids of their kind at all, and one naming nothing beside
    // the flag. The Region is a row too, the capability being a fact of the account.
    let rows: Vec<(&str, &str, &str, Vec<Param>)> = vec![
        (
            "an association of the account's own trunk and branch interfaces",
            "AssociateTrunkInterface",
            REGION,
            vec![
                param("TrunkInterfaceId", &trunk),
                param("BranchInterfaceId", &branch),
                param("VlanId", "77"),
                param("DryRun", "true"),
            ],
        ),
        (
            "an association naming ids the account holds no interface for",
            "AssociateTrunkInterface",
            REGION,
            vec![
                param("TrunkInterfaceId", "eni-0123456789abcdef0"),
                param("BranchInterfaceId", "eni-00112233445566778"),
                param("GreKey", "7"),
                param("DryRun", "true"),
            ],
        ),
        (
            "an association naming values that are not interface ids at all",
            "AssociateTrunkInterface",
            OTHER_REGION,
            vec![
                param("TrunkInterfaceId", "not-an-interface-id"),
                param("BranchInterfaceId", "neither-is-this"),
                param("DryRun", "true"),
            ],
        ),
        (
            "a report naming nothing beside the flag",
            "DescribeTrunkInterfaceAssociations",
            REGION,
            vec![param("DryRun", "true")],
        ),
        (
            "a report selecting one association by id, in another Region",
            "DescribeTrunkInterfaceAssociations",
            OTHER_REGION,
            {
                let mut params = indexed("AssociationId", &["tia-0123456789abcdef0"]);
                params.push(param("DryRun", "true"));
                params
            },
        ),
        (
            "a disassociation of an id in the form the API reports",
            "DisassociateTrunkInterface",
            REGION,
            vec![
                param("AssociationId", "tia-0123456789abcdef0"),
                param("DryRun", "true"),
            ],
        ),
        (
            "a disassociation of a value that is not an association id at all",
            "DisassociateTrunkInterface",
            REGION,
            vec![
                param("AssociationId", "not-an-association-id"),
                param("ClientToken", "trunk-dry-run-token"),
                param("DryRun", "true"),
            ],
        ),
    ];

    for (case, action, region, params) in &rows {
        let (status, body) = super::ec2_query::call(&router, region, action, params).await;
        assert_eq!(
            status, DRY_RUN_STATUS,
            "case {case:?}: expected HTTP {DRY_RUN_STATUS}, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            DRY_RUN_CODE,
            "case {case:?}: expected {DRY_RUN_CODE}, got {:?} with body {body}",
            error_code(&body)
        );
        assert_eq!(
            error_message(&body),
            DRY_RUN_MESSAGE,
            "case {case:?}: expected EC2's own dry-run wording, got {:?}",
            error_message(&body)
        );
        assert_reports_no_association(case, &body);
    }

    // `DryRun=false` is not a dry run: the request reads the capability refusal.
    let (status, body) = associate(
        &router,
        REGION,
        &[
            param("TrunkInterfaceId", &trunk),
            param("BranchInterfaceId", &branch),
            param("VlanId", "77"),
            param("DryRun", "false"),
        ],
    )
    .await;
    assert_refused("an association with DryRun=false", status, &body);

    // No dry run wrote anything: every interface reads back exactly as the creates left it.
    let after = interface_report(&router, "after every dry run").await;
    assert_eq!(
        after, before,
        "a dry run stores nothing, so the account's interfaces must read back unchanged: \
         expected {before:?}, got {after:?}"
    );
}
