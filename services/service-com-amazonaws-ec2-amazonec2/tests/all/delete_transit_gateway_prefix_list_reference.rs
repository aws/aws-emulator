// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `DeleteTransitGatewayPrefixListReference`, driven over `ec2Query`
//! (README.md / OPERATIONS.md "Wire protocol") together with the
//! `CreateTransitGatewayPrefixListReference` that puts the route there and the
//! `GetTransitGatewayPrefixListReferences` that reads it back.
//!
//! The rule under test: deleting a prefix list reference takes ONE route — the (route table,
//! prefix list) pair the request names — out of its transit gateway route table, so the call
//! answers the STORED record in the non-terminal `deleting` state with the clock owing its
//! removal, the reader reports it in that state until the removal falls due and reports it no
//! longer afterwards, and every other route of the table is untouched. Every id the request
//! names is resolved first, so one of the wrong form is refused as malformed, a route table
//! that resolves to nothing as that resource's own not-found, and a pair the table does not
//! route as a route that is not in it; a refused delete and a dry run write nothing.
//!
//! Each table row is a distinct input along what that rule generalises over: whether the
//! route forwards to an attachment or blackholes, which published id form the prefix list
//! carries, and whether the route is still `pending` or already `available` when the delete
//! arrives.

use std::time::Duration;

use ec2::ops::types::TransitGatewayVpcAttachment;
use ec2::state::{
    ComAmazonawsEc2Amazonec2State as Ec2State, PREFIX_LIST_REFERENCE_STATE_AVAILABLE,
    PREFIX_LIST_REFERENCE_STATE_DELETING, PREFIX_LIST_REFERENCE_STATE_PENDING,
    TRANSIT_GATEWAY_ATTACHMENT_STATE_AVAILABLE, TransitGatewayAttachmentView,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, element, elements, error_code, filters};
use super::wire::param;

/// The Region every call here is signed for.
const REGION: &str = "us-east-1";

/// The HTTP status EC2 answers `DryRunOperation` with.
const DRY_RUN_STATUS: u16 = 412;

/// Long enough past the span a clock-owed move takes that it is certainly overdue: the
/// deadline the polls below give up at, generous because what is under test is the state a
/// reference reaches rather than how fast it reaches it.
fn move_deadline() -> Duration {
    Duration::from_secs(60)
}

/// How long a poll below waits between two reads of a reference's state.
fn poll_interval() -> Duration {
    Duration::from_millis(50)
}

/// Wait until the reference of `prefix_list_id` in `route_table_id` is reported in `state`.
///
/// Polls for that STATE rather than sleeping for the span the clock owes the move, so a
/// loaded machine cannot read the reference before the move has been applied.
async fn wait_until_reported(
    router: &MultiServiceRouter,
    route_table_id: &str,
    prefix_list_id: &str,
    state: &str,
) {
    let deadline = tokio::time::Instant::now() + move_deadline();
    loop {
        let reported = reported_states(router, route_table_id, prefix_list_id).await;
        if reported == vec![state.to_string()] {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the reference of {prefix_list_id} in {route_table_id} must reach {state:?}, \
             last read {reported:?}"
        );
        tokio::time::sleep(poll_interval()).await;
    }
}

/// Wait until the reference of `prefix_list_id` is no longer reported for `route_table_id` —
/// the removal a delete starts, which the clock owes the table.
async fn wait_until_gone(
    router: &MultiServiceRouter,
    route_table_id: &str,
    prefix_list_id: &str,
) {
    let deadline = tokio::time::Instant::now() + move_deadline();
    loop {
        let reported = reported_states(router, route_table_id, prefix_list_id).await;
        if reported.is_empty() {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the reference of {prefix_list_id} in {route_table_id} must stop being \
             reported, last read {reported:?}"
        );
        tokio::time::sleep(poll_interval()).await;
    }
}

/// One call of `action` signed for the Region above.
async fn invoke(
    router: &MultiServiceRouter,
    action: &str,
    params: &[(String, String)],
) -> (u16, String) {
    call(router, REGION, action, params).await
}

/// A transit gateway created through the wire, as the id of the default ASSOCIATION route
/// table the create owes it ("we create a default transit gateway route table and use it as
/// the default association route table" — `CreateTransitGateway`).
async fn route_table(router: &MultiServiceRouter) -> String {
    let (status, body) = invoke(router, "CreateTransitGateway", &[]).await;
    assert_eq!(
        status, 200,
        "CreateTransitGateway must succeed, got {status} with body {body}"
    );
    element(&body, "associationDefaultRouteTableId")
        .expect("a gateway created with the documented defaults has an association route table")
}

/// Store one available VPC attachment and answer its `TransitGatewayAttachmentId`, so a row
/// whose route forwards has somewhere to forward to.
fn seed_attachment(engine: &engine::InMemoryEngine) -> String {
    engine
        .state::<Ec2State>()
        .lock()
        .insert_transit_gateway_attachment(TransitGatewayAttachmentView::Vpc(
            TransitGatewayVpcAttachment {
                transit_gateway_id: Some("tgw-0123456789abcdef0".to_string()),
                vpc_id: Some("vpc-0123456789abcdef0".to_string()),
                vpc_owner_id: Some("210987654321".to_string()),
                state: Some(TRANSIT_GATEWAY_ATTACHMENT_STATE_AVAILABLE.to_string()),
                subnet_ids: Some(vec!["subnet-0123456789abcdef0".to_string()]),
                ..Default::default()
            },
        ))
        .expect("the store holds the seeded attachment")
}

/// Store a managed prefix list under each id the rows below name, so a reference created FOR
/// one names a prefix list the account holds.
///
/// A reference routes exactly the ranges its prefix list holds, so the create resolves the
/// `PrefixListId` against the account's own lists; which list it is has no part in any rule
/// here, the rows being about the DELETE.
fn seed_prefix_lists(engine: &engine::InMemoryEngine) {
    let store = engine.state::<Ec2State>();
    let mut guard = store.lock();
    for prefix_list_id in [
        "pl-00000000000000aa1",
        "pl-0000aa12",
        "pl-00000000000000bb2",
        "pl-0000bb23",
        "pl-0000cc34",
    ] {
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

/// The state the store holds for one (route table, prefix list) pair, or `None` when it holds
/// no reference for it at all.
fn stored_state(
    engine: &engine::InMemoryEngine,
    route_table_id: &str,
    prefix_list_id: &str,
) -> Option<String> {
    engine
        .state::<Ec2State>()
        .lock()
        .transit_gateway_prefix_list_references
        .get(&(route_table_id.to_string(), prefix_list_id.to_string()))
        .and_then(|record| record.view.state.clone())
}

/// How many bytes of caller-supplied data the store holds, which a removed record must give
/// back.
fn stored_bytes(engine: &engine::InMemoryEngine) -> usize {
    engine.state::<Ec2State>().lock().stored_bytes()
}

/// The states `GetTransitGatewayPrefixListReferences` reports for `route_table_id`, narrowed
/// to the prefix list `prefix_list_id`.
async fn reported_states(
    router: &MultiServiceRouter,
    route_table_id: &str,
    prefix_list_id: &str,
) -> Vec<String> {
    let mut params = vec![param("TransitGatewayRouteTableId", route_table_id)];
    params.extend(filters(&[("prefix-list-id", &[prefix_list_id])]));
    let (status, body) = invoke(router, "GetTransitGatewayPrefixListReferences", &params).await;
    assert_eq!(
        status, 200,
        "GetTransitGatewayPrefixListReferences must succeed, got {status} with body {body}"
    );
    elements(&body, "state")
}

/// Where the traffic a reference matches goes, which is what one row varies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Target {
    /// `Blackhole`: the traffic is dropped, and the reference names no attachment.
    Blackhole,
    /// `TransitGatewayAttachmentId`: the traffic is routed to the seeded attachment.
    Attachment,
}

/// One delete, as a row of the table.
struct Case {
    /// The case name every assertion of the row quotes.
    name: &'static str,
    /// The `PrefixListId` the route is for.
    prefix_list_id: &'static str,
    /// What the route does with the traffic it matches.
    target: Target,
    /// Whether the clock-owed `available` move has been let fall due before the delete, so
    /// the route is deleted from `available` rather than from `pending`.
    delete_when_available: bool,
}

/// Create one reference through the wire and answer the attachment id it forwards to, if any.
async fn create_reference(
    router: &MultiServiceRouter,
    case: &str,
    route_table_id: &str,
    prefix_list_id: &str,
    attachment_id: Option<&str>,
) {
    let mut request = vec![
        param("TransitGatewayRouteTableId", route_table_id),
        param("PrefixListId", prefix_list_id),
    ];
    match attachment_id {
        None => request.push(param("Blackhole", "true")),
        Some(attachment_id) => request.push(param("TransitGatewayAttachmentId", attachment_id)),
    }
    let (status, body) =
        invoke(router, "CreateTransitGatewayPrefixListReference", &request).await;
    assert_eq!(
        status, 200,
        "case {case:?}: the create must succeed, got {status} with body {body}"
    );
}

/// A delete answers the stored route in `deleting`, the reader reports it in that state until
/// the removal the clock owes falls due, and afterwards neither the reader nor the store holds
/// it — and the bytes it was charged come back.
#[tokio::test]
async fn deleting_a_prefix_list_reference_takes_the_route_out_of_its_route_table() {
    // Both published id forms of an EC2 resource id (8 or 17 lowercase hexadecimal digits,
    // `crate::state::prefixed_id_is_well_formed`) and both ends of the reference's life before
    // the delete are exercised.
    let cases = [
        Case {
            name: "a pending blackhole route for a long-form prefix list",
            prefix_list_id: "pl-00000000000000aa1",
            target: Target::Blackhole,
            delete_when_available: false,
        },
        Case {
            name: "an available blackhole route for a short-form prefix list",
            prefix_list_id: "pl-0000aa12",
            target: Target::Blackhole,
            delete_when_available: true,
        },
        Case {
            name: "a pending route to a VPC attachment",
            prefix_list_id: "pl-00000000000000bb2",
            target: Target::Attachment,
            delete_when_available: false,
        },
        Case {
            name: "an available route to a VPC attachment",
            prefix_list_id: "pl-0000bb23",
            target: Target::Attachment,
            delete_when_available: true,
        },
    ];

    for case in cases {
        let Case {
            name,
            prefix_list_id,
            target,
            delete_when_available,
        } = case;
        let (router, engine) = super::ec2_query::emulator();
        seed_prefix_lists(&engine);
        let route_table_id = route_table(&router).await;
        let attachment_id = match target {
            Target::Blackhole => None,
            Target::Attachment => Some(seed_attachment(&engine)),
        };
        let case = name.to_string();

        let bytes_before = stored_bytes(&engine);
        create_reference(
            &router,
            &case,
            &route_table_id,
            prefix_list_id,
            attachment_id.as_deref(),
        )
        .await;
        let expected_before_delete = if delete_when_available {
            wait_until_reported(
                &router,
                &route_table_id,
                prefix_list_id,
                PREFIX_LIST_REFERENCE_STATE_AVAILABLE,
            )
            .await;
            PREFIX_LIST_REFERENCE_STATE_AVAILABLE
        } else {
            PREFIX_LIST_REFERENCE_STATE_PENDING
        };
        assert_eq!(
            reported_states(&router, &route_table_id, prefix_list_id).await,
            vec![expected_before_delete.to_string()],
            "case {case:?}: the route must be in the table, in the state the clock left it"
        );

        let request = vec![
            param("TransitGatewayRouteTableId", &route_table_id),
            param("PrefixListId", prefix_list_id),
        ];
        let (status, body) =
            invoke(&router, "DeleteTransitGatewayPrefixListReference", &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from the delete, got {status} with body {body}"
        );
        assert_eq!(
            element(&body, "state").as_deref(),
            Some(PREFIX_LIST_REFERENCE_STATE_DELETING),
            "case {case:?}: expected the non-terminal state of a delete, got {body}"
        );
        assert_eq!(
            element(&body, "prefixListId").as_deref(),
            Some(prefix_list_id),
            "case {case:?}: expected the prefix list the request named, got {body}"
        );
        assert_eq!(
            element(&body, "transitGatewayRouteTableId").as_deref(),
            Some(route_table_id.as_str()),
            "case {case:?}: expected the route table the request named, got {body}"
        );
        assert_eq!(
            element(&body, "prefixListOwnerId").as_deref(),
            Some(engine::emulator_account_id()),
            "case {case:?}: expected the account this emulator serves, got {body}"
        );
        assert_eq!(
            element(&body, "blackhole").as_deref(),
            Some(if target == Target::Blackhole {
                "true"
            } else {
                "false"
            }),
            "case {case:?}: expected the deleted route's own blackhole flag, got {body}"
        );
        match attachment_id.as_deref() {
            None => assert_eq!(
                elements(&body, "transitGatewayAttachment"),
                Vec::<String>::new(),
                "case {case:?}: a blackhole route routes to no attachment, got {body}"
            ),
            Some(attachment_id) => assert_eq!(
                element(&body, "transitGatewayAttachmentId").as_deref(),
                Some(attachment_id),
                "case {case:?}: expected the attachment the deleted route forwarded to, got \
                 {body}"
            ),
        }

        // The span is never zero, so a caller reading straight after the delete still sees the
        // route on its way out rather than gone.
        assert_eq!(
            stored_state(&engine, &route_table_id, prefix_list_id).as_deref(),
            Some(PREFIX_LIST_REFERENCE_STATE_DELETING),
            "case {case:?}: the store must hold what the delete reported"
        );
        assert_eq!(
            reported_states(&router, &route_table_id, prefix_list_id).await,
            vec![PREFIX_LIST_REFERENCE_STATE_DELETING.to_string()],
            "case {case:?}: the reader must report the state the delete answered with"
        );

        // The removal the clock owed falls due on its own, without anybody asking for it, and
        // the record's bytes go back to the store's budget. Read until it has, rather than
        // after a span of this test's choosing.
        wait_until_gone(&router, &route_table_id, prefix_list_id).await;
        assert_eq!(
            reported_states(&router, &route_table_id, prefix_list_id).await,
            Vec::<String>::new(),
            "case {case:?}: a route the table no longer holds must not be reported"
        );
        assert_eq!(
            stored_state(&engine, &route_table_id, prefix_list_id),
            None,
            "case {case:?}: the due removal must be applied to the stored record too"
        );
        assert_eq!(
            stored_bytes(&engine),
            bytes_before,
            "case {case:?}: the removed record must give its bytes back"
        );

        // The route is gone, so there is nothing left to delete.
        let (status, body) =
            invoke(&router, "DeleteTransitGatewayPrefixListReference", &request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a second delete must be refused, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            ec2::state::INVALID_ROUTE_NOT_FOUND,
            "case {case:?}: expected {}, got {body}",
            ec2::state::INVALID_ROUTE_NOT_FOUND
        );
    }
}

/// A delete names ONE route, so the other routes of the same table are left exactly as they
/// were — including the same prefix list in ANOTHER route table, which is a different pair.
#[tokio::test]
async fn a_delete_leaves_every_other_route_of_the_table_alone() {
    let (router, engine) = super::ec2_query::emulator();
    seed_prefix_lists(&engine);
    let first_table = route_table(&router).await;
    let second_table = route_table(&router).await;

    // The prefix lists of the first table: one is deleted, the others must survive it.
    let deleted = "pl-00000000000000aa1";
    let kept = ["pl-00000000000000bb2", "pl-0000cc34"];
    for prefix_list_id in [deleted, kept[0], kept[1]] {
        create_reference(
            &router,
            "a table with three routes",
            &first_table,
            prefix_list_id,
            None,
        )
        .await;
    }
    // The same prefix list in a different route table is a different route.
    create_reference(
        &router,
        "the same prefix list in another table",
        &second_table,
        deleted,
        None,
    )
    .await;

    let (status, body) = invoke(
        &router,
        "DeleteTransitGatewayPrefixListReference",
        &[
            param("TransitGatewayRouteTableId", &first_table),
            param("PrefixListId", deleted),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "the delete must succeed, got {status} with body {body}"
    );
    wait_until_gone(&router, &first_table, deleted).await;

    // The clock-owed removal is applied by the next call, which is what a client of the
    // emulator would make: the route is gone from both the report and the store.
    assert_eq!(
        reported_states(&router, &first_table, deleted).await,
        Vec::<String>::new(),
        "the route the request named must not be reported any more"
    );
    assert_eq!(
        stored_state(&engine, &first_table, deleted),
        None,
        "the route the request named must be gone"
    );
    for prefix_list_id in kept {
        assert_eq!(
            reported_states(&router, &first_table, prefix_list_id).await,
            vec![PREFIX_LIST_REFERENCE_STATE_AVAILABLE.to_string()],
            "case {prefix_list_id:?}: another route of the table must be left alone"
        );
    }
    assert_eq!(
        reported_states(&router, &second_table, deleted).await.len(),
        1,
        "the same prefix list in another route table is a different route, and must be left \
         alone"
    );
}

/// Every id a delete names is resolved against the store before anything is written, so an id
/// of the wrong form is refused as malformed, a route table that resolves to nothing with that
/// resource's own not-found code, and a pair the table does not route as a route that is not in
/// it — never as a 501 or an empty success, and never by writing anything.
#[tokio::test]
async fn a_delete_whose_ids_do_not_resolve_is_refused_with_its_own_code() {
    let (router, engine) = super::ec2_query::emulator();
    seed_prefix_lists(&engine);
    let route_table_id = route_table(&router).await;
    let attachment_id = seed_attachment(&engine);
    let referenced = "pl-00000000000000aa1";
    create_reference(
        &router,
        "the route the refusals must not touch",
        &route_table_id,
        referenced,
        None,
    )
    .await;

    // (case, the route table id, the prefix list id, the code expected)
    let cases: Vec<(&str, &str, &str, &str)> = vec![
        (
            "a route table id of the right form that names no route table",
            "tgw-rtb-0000000000000000f",
            referenced,
            ec2::state::INVALID_ROUTE_TABLE_ID_NOT_FOUND,
        ),
        (
            "a short-form route table id that names no route table",
            "tgw-rtb-0000000f",
            referenced,
            ec2::state::INVALID_ROUTE_TABLE_ID_NOT_FOUND,
        ),
        (
            "a route table id that is not an id of that kind",
            "tgw-rtb-0123456789abcde",
            referenced,
            ec2::state::INVALID_ROUTE_TABLE_ID_MALFORMED,
        ),
        (
            "an attachment id where a route table id belongs",
            &attachment_id,
            referenced,
            ec2::state::INVALID_ROUTE_TABLE_ID_MALFORMED,
        ),
        (
            "a prefix list id that is not an id of that kind",
            &route_table_id,
            "pl-not-an-id",
            ec2::state::INVALID_PREFIX_LIST_ID_MALFORMED,
        ),
        (
            "a prefix list id carrying a digit outside the id alphabet",
            &route_table_id,
            "pl-0000000g",
            ec2::state::INVALID_PREFIX_LIST_ID_MALFORMED,
        ),
        (
            "a prefix list the named route table does not route",
            &route_table_id,
            "pl-00000000000000ff9",
            ec2::state::INVALID_ROUTE_NOT_FOUND,
        ),
        (
            "a short-form prefix list the named route table does not route",
            &route_table_id,
            "pl-0000ff98",
            ec2::state::INVALID_ROUTE_NOT_FOUND,
        ),
    ];

    for (name, table_id, prefix_list_id, expected) in cases {
        let case = format!("DeleteTransitGatewayPrefixListReference with {name}");
        let (status, body) = invoke(
            &router,
            "DeleteTransitGatewayPrefixListReference",
            &[
                param("TransitGatewayRouteTableId", table_id),
                param("PrefixListId", prefix_list_id),
            ],
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            expected,
            "case {case:?}: expected {expected}, got {body}"
        );
        assert_eq!(
            stored_state(&engine, &route_table_id, referenced).as_deref(),
            Some(PREFIX_LIST_REFERENCE_STATE_PENDING),
            "case {case:?}: a refused delete must write nothing"
        );
    }
}

/// A dry run of a delete that WOULD have succeeded performs nothing: the route is still in its
/// table, in the state it was in, and the call that follows is the one that deletes it.
#[tokio::test]
async fn a_dry_run_deletes_no_reference() {
    // Both ends of the reference's life before the delete, so a dry run is answered the same
    // way whichever state the route is in.
    for (case, prefix_list_id, delete_when_available) in [
        ("a pending route", "pl-00000000000000aa1", false),
        ("an available route", "pl-00000000000000bb2", true),
    ] {
        let (router, engine) = super::ec2_query::emulator();
        seed_prefix_lists(&engine);
        let route_table_id = route_table(&router).await;
        create_reference(&router, case, &route_table_id, prefix_list_id, None).await;
        let expected = if delete_when_available {
            wait_until_reported(
                &router,
                &route_table_id,
                prefix_list_id,
                PREFIX_LIST_REFERENCE_STATE_AVAILABLE,
            )
            .await;
            PREFIX_LIST_REFERENCE_STATE_AVAILABLE
        } else {
            PREFIX_LIST_REFERENCE_STATE_PENDING
        };
        assert_eq!(
            reported_states(&router, &route_table_id, prefix_list_id).await,
            vec![expected.to_string()],
            "case {case:?}: the route must be in the state the clock left it before the dry run"
        );

        let request = vec![
            param("TransitGatewayRouteTableId", &route_table_id),
            param("PrefixListId", prefix_list_id),
        ];
        let dry_run = {
            let mut dry_run = request.clone();
            dry_run.push(param("DryRun", "true"));
            dry_run
        };
        let (status, body) =
            invoke(&router, "DeleteTransitGatewayPrefixListReference", &dry_run).await;
        assert_eq!(
            status, DRY_RUN_STATUS,
            "case {case:?}: expected HTTP {DRY_RUN_STATUS}, got {status} with body {body}"
        );
        assert_eq!(
            stored_state(&engine, &route_table_id, prefix_list_id).as_deref(),
            Some(expected),
            "case {case:?}: a dry run must delete nothing"
        );

        let (status, body) =
            invoke(&router, "DeleteTransitGatewayPrefixListReference", &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 after a dry run, got {status} with body {body}"
        );
        assert_eq!(
            stored_state(&engine, &route_table_id, prefix_list_id).as_deref(),
            Some(PREFIX_LIST_REFERENCE_STATE_DELETING),
            "case {case:?}: the call after the dry run is the one that deletes the route"
        );
    }
}

/// A route whose removal is already owed is on its way out either way, so a second delete is
/// answered with the route as it stands rather than refused, and the removal the first delete
/// started still carries it out.
#[tokio::test]
async fn a_second_delete_answers_the_route_on_its_way_out() {
    let (router, engine) = super::ec2_query::emulator();
    seed_prefix_lists(&engine);
    let route_table_id = route_table(&router).await;
    let prefix_list_id = "pl-00000000000000aa1";
    create_reference(
        &router,
        "a route deleted twice",
        &route_table_id,
        prefix_list_id,
        None,
    )
    .await;
    let request = vec![
        param("TransitGatewayRouteTableId", &route_table_id),
        param("PrefixListId", prefix_list_id),
    ];

    for attempt in ["the first delete", "the second delete"] {
        let (status, body) =
            invoke(&router, "DeleteTransitGatewayPrefixListReference", &request).await;
        assert_eq!(
            status, 200,
            "case {attempt:?}: expected HTTP 200, got {status} with body {body}"
        );
        assert_eq!(
            element(&body, "state").as_deref(),
            Some(PREFIX_LIST_REFERENCE_STATE_DELETING),
            "case {attempt:?}: expected the state of a route on its way out, got {body}"
        );
    }

    wait_until_gone(&router, &route_table_id, prefix_list_id).await;
    assert_eq!(
        reported_states(&router, &route_table_id, prefix_list_id).await,
        Vec::<String>::new(),
        "the route must not be reported once its removal has fallen due"
    );
    assert_eq!(
        stored_state(&engine, &route_table_id, prefix_list_id),
        None,
        "the removal the delete started must carry the route out of its table"
    );
}
