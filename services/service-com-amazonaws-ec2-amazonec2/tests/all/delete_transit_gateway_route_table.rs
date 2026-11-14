// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `DeleteTransitGatewayRouteTable`, driven over `ec2Query` (README.md
//! / OPERATIONS.md "Wire protocol").
//!
//! The rule under test: deleting a transit gateway route table takes that table out of the
//! store together with everything stored as a PART of it — the route propagations into it
//! and the prefix list references it held, whose bytes go back to the store's budget — and
//! answers the removed table in the `deleting` member of its own state catalog, so no later
//! reader resolves it; a table one of its transit gateway's default roles names, and a table
//! an attachment is still associated with, are refused instead with their own code while
//! nothing is removed.
//!
//! Each row of the success table is a distinct input along what that rule generalises over:
//! how many tags the deleted table carries, whether it is still `pending` or already
//! `available` when the delete arrives, and how many propagations and prefix list references
//! are stored in it. The refusal rows are one per way a named route table id can fail to
//! resolve, one per default role a gateway's own table can hold, one for a table that is
//! still associated, and one for a dry run of a request that would have succeeded.

use std::time::Duration;

use ec2::ops::types::{TRANSIT_GATEWAY_ROUTE_TABLE_STATE_VALUES, TransitGatewayVpcAttachment};
use ec2::state::{
    ComAmazonawsEc2Amazonec2State as Ec2State, MAX_TAGS_PER_RESOURCE,
    TRANSIT_GATEWAY_ATTACHMENT_STATE_AVAILABLE, TRANSITION_DELAY, TransitGatewayAttachmentView,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{as_timestamp, call, element, elements, error_code, unescaped};
use super::wire::param;

/// The Region every call here is signed for.
const REGION: &str = "us-east-1";

/// The HTTP status EC2 answers `DryRunOperation` with.
const DRY_RUN_STATUS: u16 = 412;

/// The state a delete puts a transit gateway route table into. Pinned to the modelled
/// catalog by the assertion in [`a_delete_removes_the_table_and_everything_stored_in_it`],
/// which requires it to be one of `TransitGatewayRouteTableState`'s members.
const DELETING: &str = "deleting";

/// Long enough past [`TRANSITION_DELAY`] that a move the clock owes is certainly due.
fn past_the_transition() -> Duration {
    TRANSITION_DELAY + Duration::from_millis(250)
}

/// One call of `action` signed for the Region above.
async fn invoke(
    router: &MultiServiceRouter,
    action: &str,
    params: &[(String, String)],
) -> (u16, String) {
    call(router, REGION, action, params).await
}

/// The same call, requiring HTTP 200, and the body it answered.
async fn invoke_ok(
    case: &str,
    router: &MultiServiceRouter,
    action: &str,
    params: &[(String, String)],
) -> String {
    let (status, body) = invoke(router, action, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from {action}, got {status} with body {body}"
    );
    body
}

/// One `TransitGatewayRouteTableId=` request member.
fn table_param(route_table_id: &str) -> Vec<(String, String)> {
    vec![param("TransitGatewayRouteTableId", route_table_id)]
}

/// A transit gateway created through the wire with `options`: its id and the ids of the
/// default route tables the create owed it, as its response reports them.
async fn transit_gateway(
    router: &MultiServiceRouter,
    options: &[(String, String)],
) -> (String, Option<String>, Option<String>) {
    let body = invoke_ok("the gateway under test", router, "CreateTransitGateway", options).await;
    let transit_gateway_id =
        element(&body, "transitGatewayId").expect("a created gateway reports its id");
    (
        transit_gateway_id,
        element(&body, "associationDefaultRouteTableId"),
        element(&body, "propagationDefaultRouteTableId"),
    )
}

/// The `TagSpecifications` form fields that tag the route table a create makes.
fn tag_params(tags: &[(String, String)]) -> Vec<(String, String)> {
    let mut params = vec![param(
        "TagSpecifications.1.ResourceType",
        ec2::state::RESOURCE_TYPE_TRANSIT_GATEWAY_ROUTE_TABLE,
    )];
    for (index, (key, value)) in tags.iter().enumerate() {
        let position = index + 1;
        params.push(param(
            &format!("TagSpecifications.1.Tag.{position}.Key"),
            key,
        ));
        params.push(param(
            &format!("TagSpecifications.1.Tag.{position}.Value"),
            value,
        ));
    }
    params
}

/// An ADDITIONAL route table of `transit_gateway_id`, created through the wire with `tags`:
/// its id and the creation time the create reported for it.
async fn additional_route_table(
    case: &str,
    router: &MultiServiceRouter,
    transit_gateway_id: &str,
    tags: &[(String, String)],
) -> (String, String) {
    let mut request = vec![param("TransitGatewayId", transit_gateway_id)];
    if !tags.is_empty() {
        request.extend(tag_params(tags));
    }
    let body = invoke_ok(case, router, "CreateTransitGatewayRouteTable", &request).await;
    (
        element(&body, "transitGatewayRouteTableId").expect("a created route table reports its id"),
        element(&body, "creationTime").expect("a created route table reports its creation time"),
    )
}

/// One VPC attachment of `transit_gateway_id` seeded straight into the store, as its minted
/// id: the attachment-creating operations are their own issues, and a propagation and an
/// association both need an attachment the store holds.
fn seed_attachment(
    engine: &engine::InMemoryEngine,
    transit_gateway_id: &str,
    vpc_id: &str,
) -> String {
    engine
        .state::<Ec2State>()
        .lock()
        .insert_transit_gateway_attachment(TransitGatewayAttachmentView::Vpc(
            TransitGatewayVpcAttachment {
                transit_gateway_id: Some(transit_gateway_id.to_string()),
                vpc_id: Some(vpc_id.to_string()),
                vpc_owner_id: Some(engine::emulator_account_id().to_string()),
                state: Some(TRANSIT_GATEWAY_ATTACHMENT_STATE_AVAILABLE.to_string()),
                ..Default::default()
            },
        ))
        .expect("the store holds the seeded attachment")
}

/// Everything about the store this suite compares before and after a call.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Snapshot {
    /// The route tables the store holds, in its own stable order.
    route_tables: Vec<String>,
    /// The (route table, attachment) pairs that propagate.
    propagations: Vec<(String, String)>,
    /// The (route table, prefix list) pairs the store holds a reference for.
    prefix_list_references: Vec<(String, String)>,
    /// The caller-supplied bytes the store has charged.
    stored_bytes: usize,
}

/// The store as it stands right now.
fn snapshot(engine: &engine::InMemoryEngine) -> Snapshot {
    let state = engine.state::<Ec2State>();
    let store = state.lock();
    Snapshot {
        route_tables: store.transit_gateway_route_tables.keys().cloned().collect(),
        propagations: store.transit_gateway_propagations.keys().cloned().collect(),
        prefix_list_references: store
            .transit_gateway_prefix_list_references
            .keys()
            .cloned()
            .collect(),
        stored_bytes: store.stored_bytes(),
    }
}

/// Whether the store still owes `route_table_id` a clock-driven state move.
fn owes_a_move(engine: &engine::InMemoryEngine, route_table_id: &str) -> bool {
    engine
        .state::<Ec2State>()
        .lock()
        .transit_gateway_route_table_available_at
        .contains_key(route_table_id)
}

/// `count` distinct tags, for the row that asks for as many as EC2 documents per resource.
fn many_tags(count: usize) -> Vec<(String, String)> {
    (1..=count)
        .map(|nth| (format!("key-{nth}"), format!("value-{nth}")))
        .collect()
}

/// One (key, value) pair, as the table rows spell their tags.
fn tag(key: &str, value: &str) -> (String, String) {
    (key.to_string(), value.to_string())
}

/// One delete, as a row of the table.
struct Case {
    /// The case name every assertion of the row quotes.
    name: &'static str,
    /// The tags the deleted table was created with.
    tags: Vec<(String, String)>,
    /// Whether the table is left to reach `available` before it is deleted.
    deleted_once_available: bool,
    /// How many attachments propagate their routes into the table when it is deleted.
    propagated_attachments: usize,
    /// The prefix lists the table holds a blackhole reference for when it is deleted.
    prefix_lists: Vec<&'static str>,
}

/// Deleting a route table removes that table and everything stored in it, gives their bytes
/// back, and leaves every other record of the account where it was — and the removal is
/// observable to every later reader: the describe of that id refuses it, a second delete
/// refuses it, and the state move the clock still owed the table never lands.
#[tokio::test]
async fn a_delete_removes_the_table_and_everything_stored_in_it() {
    assert!(
        TRANSIT_GATEWAY_ROUTE_TABLE_STATE_VALUES.contains(&DELETING),
        "the state a delete answers with must be a member of the modelled catalog {:?}",
        TRANSIT_GATEWAY_ROUTE_TABLE_STATE_VALUES
    );

    let cases = vec![
        Case {
            name: "no tags, deleted while still pending, holding nothing",
            tags: Vec::new(),
            deleted_once_available: false,
            propagated_attachments: 0,
            prefix_lists: Vec::new(),
        },
        Case {
            name: "one tag, deleted once available, holding one propagation and one reference",
            tags: vec![tag("Name", "spoke")],
            deleted_once_available: true,
            propagated_attachments: 1,
            prefix_lists: vec!["pl-00000000000000aa1"],
        },
        Case {
            name: "three tags, deleted while pending, holding two propagations and two references",
            // An empty tag value is a value EC2 accepts, so one row carries it.
            tags: vec![tag("Env", "prod"), tag("Tier", "edge"), tag("Owner", "")],
            deleted_once_available: false,
            propagated_attachments: 2,
            prefix_lists: vec!["pl-00000000000000aa1", "pl-0000bb22"],
        },
        Case {
            name: "as many tags as EC2 documents per resource, deleted once available",
            tags: many_tags(MAX_TAGS_PER_RESOURCE),
            deleted_once_available: true,
            propagated_attachments: 0,
            prefix_lists: vec!["pl-0000cc33"],
        },
    ];

    for case in cases {
        let Case {
            name,
            tags,
            deleted_once_available,
            propagated_attachments,
            prefix_lists,
        } = case;
        let (router, engine) = super::ec2_query::emulator();
        // The prefix lists the references below route: a reference routes exactly the ranges
        // its list holds, so the create resolves the `PrefixListId` against the account's own
        // lists. Which list it is has no part in this row, which is about the DELETE.
        {
            let store = engine.state::<Ec2State>();
            let mut guard = store.lock();
            for prefix_list_id in prefix_lists
                .iter()
                .copied()
                .chain(["pl-00000000000000ff9"])
            {
                guard.managed_prefix_lists.insert(
                    prefix_list_id.to_string(),
                    ec2::state::ManagedPrefixListRecord {
                        view: ec2::ops::types::ManagedPrefixList {
                            prefix_list_id: Some(prefix_list_id.to_string()),
                            address_family: Some("IPv4".to_string()),
                            state: Some("create-complete".to_string()),
                            prefix_list_name: Some(format!("seeded-{prefix_list_id}")),
                            max_entries: Some(5),
                            version: Some(1),
                            owner_id: Some(engine::emulator_account_id().to_string()),
                            ..Default::default()
                        },
                        entries: Vec::new(),
                        pending: None,
                    },
                );
            }
        }
        let (transit_gateway_id, association_table, _) = transit_gateway(&router, &[]).await;
        let default_route_table_id = association_table
            .expect("a gateway created with the documented defaults owns a default route table");

        // One propagation and one prefix list reference of ANOTHER route table of the same
        // gateway, so the row shows the delete removing only what belongs to its own table.
        let untouched_attachment =
            seed_attachment(&engine, &transit_gateway_id, "vpc-00000000000000001");
        invoke_ok(
            name,
            &router,
            "EnableTransitGatewayRouteTablePropagation",
            &[
                param("TransitGatewayRouteTableId", &default_route_table_id),
                param("TransitGatewayAttachmentId", &untouched_attachment),
            ],
        )
        .await;
        invoke_ok(
            name,
            &router,
            "CreateTransitGatewayPrefixListReference",
            &[
                param("TransitGatewayRouteTableId", &default_route_table_id),
                param("PrefixListId", "pl-00000000000000ff9"),
                param("Blackhole", "true"),
            ],
        )
        .await;
        // The attachments whose routes will propagate into the table under test exist BEFORE
        // the baseline is taken: an attachment is a resource of the GATEWAY, not a part of
        // the route table, so the delete leaves it stored and the baseline must hold it too.
        let attachments: Vec<String> = (0..propagated_attachments)
            .map(|nth| {
                seed_attachment(
                    &engine,
                    &transit_gateway_id,
                    &format!("vpc-0000000000000001{nth}"),
                )
            })
            .collect();
        let before = snapshot(&engine);

        let (route_table_id, created_at) =
            additional_route_table(name, &router, &transit_gateway_id, &tags).await;
        for attachment_id in &attachments {
            invoke_ok(
                name,
                &router,
                "EnableTransitGatewayRouteTablePropagation",
                &[
                    param("TransitGatewayRouteTableId", &route_table_id),
                    param("TransitGatewayAttachmentId", attachment_id),
                ],
            )
            .await;
        }
        for prefix_list_id in &prefix_lists {
            invoke_ok(
                name,
                &router,
                "CreateTransitGatewayPrefixListReference",
                &[
                    param("TransitGatewayRouteTableId", &route_table_id),
                    param("PrefixListId", prefix_list_id),
                    param("Blackhole", "true"),
                ],
            )
            .await;
        }
        if deleted_once_available {
            tokio::time::sleep(past_the_transition()).await;
        }

        let (status, body) = invoke(
            &router,
            "DeleteTransitGatewayRouteTable",
            &table_param(&route_table_id),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {name:?}: expected HTTP 200 from the delete, got {status} with body {body}"
        );

        // The answer is the table that was removed, in the state a delete puts it into.
        assert_eq!(
            element(&body, "transitGatewayRouteTableId").as_deref(),
            Some(route_table_id.as_str()),
            "case {name:?}: expected the table the request named, got {body}"
        );
        assert_eq!(
            element(&body, "transitGatewayId").as_deref(),
            Some(transit_gateway_id.as_str()),
            "case {name:?}: expected the gateway the table belonged to, got {body}"
        );
        assert_eq!(
            element(&body, "state").as_deref(),
            Some(DELETING),
            "case {name:?}: expected the state of a delete, got {body}"
        );
        assert_eq!(
            element(&body, "defaultAssociationRouteTable").as_deref(),
            Some("false"),
            "case {name:?}: an additional route table holds no default role, got {body}"
        );
        assert_eq!(
            element(&body, "defaultPropagationRouteTable").as_deref(),
            Some("false"),
            "case {name:?}: an additional route table holds no default role, got {body}"
        );
        assert_eq!(
            element(&body, "creationTime").as_deref(),
            Some(created_at.as_str()),
            "case {name:?}: expected the creation time of the stored record, got {body}"
        );
        assert!(
            as_timestamp(&created_at).is_some(),
            "case {name:?}: expected a modelled timestamp, got {created_at:?}"
        );
        assert_eq!(
            elements(&body, "key"),
            tags.iter()
                .map(|(key, _)| key.clone())
                .collect::<Vec<String>>(),
            "case {name:?}: expected the tags the deleted table carried, got {body}"
        );

        // The store is back to exactly what it held before the table was created: the table,
        // its propagations and its references are gone, their bytes are back in the budget,
        // and the records of the other route table are untouched.
        let after = snapshot(&engine);
        assert_eq!(
            after, before,
            "case {name:?}: the delete must leave the store as it was before the table was \
             created, expected {before:?}, got {after:?}"
        );
        assert!(
            !owes_a_move(&engine, &route_table_id),
            "case {name:?}: the clock must owe a removed table nothing, and it still owes \
             {route_table_id:?} a move"
        );

        // Every later reader of that id refuses it, so the removal is not just the answer of
        // one call — while the other route table of the same gateway still reads back.
        for action in [
            "DescribeTransitGatewayRouteTables",
            "GetTransitGatewayRouteTablePropagations",
            "GetTransitGatewayPrefixListReferences",
            "DeleteTransitGatewayRouteTable",
        ] {
            let params = if action == "DescribeTransitGatewayRouteTables" {
                vec![param("TransitGatewayRouteTableIds.1", &route_table_id)]
            } else {
                table_param(&route_table_id)
            };
            let (status, body) = invoke(&router, action, &params).await;
            assert_eq!(
                status, 400,
                "case {name:?}: {action} must refuse the removed table, got {status} with \
                 body {body}"
            );
            assert_eq!(
                error_code(&body),
                ec2::state::INVALID_ROUTE_TABLE_ID_NOT_FOUND,
                "case {name:?}: {action} must answer the route table's own not-found code, \
                 got {body}"
            );
        }
        let body = invoke_ok(name, &router, "DescribeTransitGatewayRouteTables", &[]).await;
        assert_eq!(
            elements(&body, "transitGatewayRouteTableId"),
            vec![default_route_table_id.clone()],
            "case {name:?}: only the deleted table may be gone, got {body}"
        );

        // The move the clock owed the table before the delete never lands, because it went
        // with the record: the table does not reappear in any state.
        tokio::time::sleep(past_the_transition()).await;
        let body = invoke_ok(name, &router, "DescribeTransitGatewayRouteTables", &[]).await;
        assert_eq!(
            elements(&body, "transitGatewayRouteTableId"),
            vec![default_route_table_id],
            "case {name:?}: a removed table must not come back once the clock passes its \
             move, got {body}"
        );
    }
}

/// A route table id that does not resolve, a table the transit gateway owns as a default,
/// a table an attachment is still associated with, and a dry run are each refused with their
/// own documented code — never with a 501 or an empty success — and nothing is removed.
#[tokio::test]
async fn a_delete_that_is_not_admissible_is_refused_and_removes_nothing() {
    let (router, engine) = super::ec2_query::emulator();

    // A gateway with the documented defaults: its one table is BOTH default tables.
    let (transit_gateway_id, association_table, _) = transit_gateway(&router, &[]).await;
    let default_association_table = association_table
        .expect("a gateway created with the documented defaults owns a default route table");
    // A gateway whose create was told not to associate by default: its one table is the
    // default PROPAGATION table only, so the two default roles are covered separately.
    let (_, no_association_table, propagation_table) = transit_gateway(
        &router,
        &[param("Options.DefaultRouteTableAssociation", "disable")],
    )
    .await;
    assert_eq!(
        no_association_table, None,
        "a gateway created without the default association owns no association route table"
    );
    let default_propagation_table = propagation_table
        .expect("a gateway created with the default propagation owns a propagation route table");

    // A table that is still associated with an attachment, and one that nothing holds, both
    // additional tables of the first gateway.
    let (associated_table, _) =
        additional_route_table("the associated table", &router, &transit_gateway_id, &[]).await;
    let attachment_id = seed_attachment(&engine, &transit_gateway_id, "vpc-00000000000000002");
    invoke_ok(
        "the association that blocks the delete",
        &router,
        "AssociateTransitGatewayRouteTable",
        &[
            param("TransitGatewayRouteTableId", &associated_table),
            param("TransitGatewayAttachmentId", &attachment_id),
        ],
    )
    .await;
    let (free_table, _) = additional_route_table(
        "the table a dry run names",
        &router,
        &transit_gateway_id,
        &[tag("Name", "free")],
    )
    .await;

    let before = snapshot(&engine);

    // (case, the request's fields, the HTTP status and the code expected)
    type RefusalCase = (String, Vec<(String, String)>, u16, &'static str);
    let mut cases: Vec<RefusalCase> = Vec::new();
    // One row per way a value can fail to be a route table id of this kind: a hex suffix of
    // neither published length, a suffix that is not hex at all, the id of another resource,
    // and the id form of a route table ANNOUNCEMENT, which extends the same prefix.
    for id in [
        "tgw-rtb-0123456789abcde",
        "tgw-rtb-not-an-id",
        "tgw-attach-0123456789abcdef0",
        "tgw-rtb-an-0123456789abcdef0",
    ] {
        cases.push((
            format!("a value that is not a route table id: {id:?}"),
            table_param(id),
            400,
            ec2::state::INVALID_ROUTE_TABLE_ID_MALFORMED,
        ));
    }
    // One row per published id length that names no table of the account.
    for id in ["tgw-rtb-0123456789abcdef0", "tgw-rtb-0000000f"] {
        cases.push((
            format!("a well-formed id no create ever minted: {id:?}"),
            table_param(id),
            400,
            ec2::state::INVALID_ROUTE_TABLE_ID_NOT_FOUND,
        ));
    }
    cases.push((
        "the default association route table of its gateway".to_string(),
        table_param(&default_association_table),
        400,
        ec2::state::INCORRECT_STATE,
    ));
    cases.push((
        "the default propagation route table of its gateway".to_string(),
        table_param(&default_propagation_table),
        400,
        ec2::state::INCORRECT_STATE,
    ));
    cases.push((
        "a table an attachment is still associated with".to_string(),
        table_param(&associated_table),
        400,
        ec2::state::DEPENDENCY_VIOLATION,
    ));
    cases.push((
        "a dry run of a delete that would have succeeded".to_string(),
        {
            let mut params = table_param(&free_table);
            params.push(param("DryRun", "true"));
            params
        },
        DRY_RUN_STATUS,
        ec2::state::DRY_RUN_OPERATION,
    ));

    for (name, request, expected_status, expected_code) in &cases {
        let (status, body) = invoke(&router, "DeleteTransitGatewayRouteTable", request).await;
        assert_eq!(
            status, *expected_status,
            "case {name:?}: expected HTTP {expected_status}, got {status} with body {body}"
        );
        let code = error_code(&body);
        assert_eq!(
            code, *expected_code,
            "case {name:?}: expected the code {expected_code:?}, got {code:?} in {body}"
        );
    }

    // A malformed id is named in the refusal, by the one wording every operation that
    // resolves a `TransitGatewayRouteTableId` answers with.
    let malformed = "tgw-rtb-not-an-id";
    let (_, body) = invoke(
        &router,
        "DeleteTransitGatewayRouteTable",
        &table_param(malformed),
    )
    .await;
    assert_eq!(
        element(&body, "Message").map(|message| unescaped(&message)),
        Some(format!("Invalid Transit Gateway Route Table id {malformed}.")),
        "the refusal must name the resource and the id the request gave, got {body}"
    );

    let after = snapshot(&engine);
    assert_eq!(
        after, before,
        "a refused delete must remove nothing: expected {before:?}, got {after:?}"
    );

    // The table the dry run named is still there to be deleted, which is what makes the dry
    // run's refusal a report about permission rather than about the table.
    let (status, body) = invoke(
        &router,
        "DeleteTransitGatewayRouteTable",
        &table_param(&free_table),
    )
    .await;
    assert_eq!(
        status, 200,
        "the table a dry run named must still be deletable, got {status} with body {body}"
    );
    assert_eq!(
        element(&body, "state").as_deref(),
        Some(DELETING),
        "expected the state of a delete, got {body}"
    );
    assert!(
        !snapshot(&engine)
            .route_tables
            .contains(&free_table.to_string()),
        "the delete that followed the dry run must remove the table"
    );
}
