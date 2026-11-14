// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the reserved-cache-node surface:
//! `PurchaseReservedCacheNodesOffering`, which buys a reservation from the
//! published offering catalog, and `DescribeReservedCacheNodes`, which reports
//! the reservations an account bought.
//!
//! Every assertion goes over the wire, through the same router the production
//! emulator mounts (the harness in [`super::cache_cluster_lifecycle`]), so only
//! the HTTP status and the XML body are inspected and nothing reaches into the
//! service's state struct: a describe that is not computed from the persisted
//! purchase cannot pass.
//!
//! The tests are tables and invariants rather than one recorded answer. The
//! rule under test is that a reservation reports the attributes of the offering
//! it was bought from, and that every member `DescribeReservedCacheNodes`
//! documents selects the stored reservations whose copy of those attributes
//! matches it — so the rows differ along each axis of the catalog (engine, node
//! type generation, reservation term in both documented forms, payment option,
//! node count) and along each filter member, with the published `MaxRecords`
//! bounds and one past each.

use service_com_amazonaws_elasticache_amazonelasticachev9::state;

use crate::harness::{params, Emulator, Reply};

/// The operations under test.
const DESCRIBE: &str = "DescribeReservedCacheNodes";
const PURCHASE: &str = "PurchaseReservedCacheNodesOffering";
/// The describe of the catalog a reservation is bought from, used to check that
/// a reservation reports the offering's own prices.
const DESCRIBE_OFFERINGS: &str = "DescribeReservedCacheNodesOfferings";

/// The region the harness signs with, which is the Region a reservation's ARN
/// names.
const REGION: &str = "us-east-1";

/// One purchase row: a catalog entry named by the four attributes that identify
/// it, the `Duration` form the request sends, and how many nodes it reserves.
///
/// The attribute values come from the catalog's own sources — `state::ENGINES`
/// for the engine a `ProductDescription` names, `state::OFFERING_NODE_TYPES`
/// for the node types, `state::OFFERING_TERMS` for the terms (a year count and
/// its second count name the same term) and `state::STOCKED_OFFERING_TYPES` for
/// the payment options — and the rows span every one of those axes.
struct Row {
    case: &'static str,
    reservation_id: &'static str,
    product_description: &'static str,
    cache_node_type: &'static str,
    /// What the request sends for `Duration`: a year count on some rows, the
    /// second count on others, because both name the same term.
    duration_sent: &'static str,
    /// What `Duration` must be reported as: the term in seconds.
    duration_seconds: &'static str,
    offering_type: &'static str,
    /// What the request sends for `CacheNodeCount`, or `None` to exercise the
    /// documented default of 1.
    cache_node_count_sent: Option<&'static str>,
    /// What `CacheNodeCount` must be reported as.
    cache_node_count: &'static str,
}

/// The purchases every table below is built from: one per engine, both terms in
/// both documented forms, every stocked payment option, current- and
/// previous-generation node types, and node counts from the documented default
/// to a large reservation.
const ROWS: [Row; 6] = [
    Row {
        case: "redis, current generation, one year, all upfront, default count",
        reservation_id: "redis-m7g-1y-all",
        product_description: "redis",
        cache_node_type: "cache.m7g.large",
        duration_sent: "1",
        duration_seconds: "31536000",
        offering_type: "All Upfront",
        cache_node_count_sent: None,
        cache_node_count: "1",
    },
    Row {
        case: "memcached, burstable, three years in seconds, no upfront",
        reservation_id: "memcached-t4g-3y-no",
        product_description: "memcached",
        cache_node_type: "cache.t4g.micro",
        duration_sent: "94608000",
        duration_seconds: "94608000",
        offering_type: "No Upfront",
        cache_node_count_sent: Some("5"),
        cache_node_count: "5",
    },
    Row {
        case: "valkey, memory optimized, one year in seconds, partial upfront",
        reservation_id: "valkey-r6g-1y-partial",
        product_description: "valkey",
        cache_node_type: "cache.r6g.large",
        duration_sent: "31536000",
        duration_seconds: "31536000",
        offering_type: "Partial Upfront",
        cache_node_count_sent: Some("2"),
        cache_node_count: "2",
    },
    Row {
        case: "redis, previous generation, three years, all upfront, large count",
        reservation_id: "redis-m3-3y-all",
        product_description: "redis",
        cache_node_type: "cache.m3.medium",
        duration_sent: "3",
        duration_seconds: "94608000",
        offering_type: "All Upfront",
        cache_node_count_sent: Some("40"),
        cache_node_count: "40",
    },
    Row {
        case: "memcached, large memory optimized, three years, partial upfront",
        reservation_id: "memcached-r5-3y-partial",
        product_description: "memcached",
        cache_node_type: "cache.r5.2xlarge",
        duration_sent: "3",
        duration_seconds: "94608000",
        offering_type: "Partial Upfront",
        cache_node_count_sent: Some("1"),
        cache_node_count: "1",
    },
    Row {
        case: "valkey, burstable, one year, no upfront",
        reservation_id: "valkey-t3-1y-no",
        product_description: "valkey",
        cache_node_type: "cache.t3.micro",
        duration_sent: "1",
        duration_seconds: "31536000",
        offering_type: "No Upfront",
        cache_node_count_sent: Some("3"),
        cache_node_count: "3",
    },
];

impl Row {
    /// The `ReservedCacheNodesOfferingId` of the catalog entry this row buys.
    ///
    /// Derived through the same function the catalog mints its ids with, so the
    /// test names an offering by its attributes rather than by a pasted id.
    fn offering_id(&self) -> String {
        state::offering_id(
            self.product_description,
            self.cache_node_type,
            self.duration_seconds
                .parse::<i32>()
                .expect("a term in seconds"),
            self.offering_type,
        )
    }

    /// The form pairs that buy this row's reservation.
    fn purchase_pairs(&self) -> Vec<(String, String)> {
        let mut pairs = params(&[("ReservedCacheNodeId", self.reservation_id)]);
        pairs.push((
            "ReservedCacheNodesOfferingId".to_string(),
            self.offering_id(),
        ));
        if let Some(count) = self.cache_node_count_sent {
            pairs.extend(params(&[("CacheNodeCount", count)]));
        }
        pairs
    }
}

/// One `DescribeReservedCacheNodes` call with the given form pairs.
async fn describe(emulator: &Emulator, pairs: &[(&str, &str)]) -> Reply {
    emulator.call(DESCRIBE, &params(pairs)).await
}

/// One `PurchaseReservedCacheNodesOffering` call with the given form pairs.
async fn purchase(emulator: &Emulator, pairs: &[(&str, &str)]) -> Reply {
    emulator.call(PURCHASE, &params(pairs)).await
}

/// The `ReservedCacheNodeId` of every reservation in a reply, in document
/// order.
fn reservation_ids(reply: &Reply) -> Vec<&str> {
    reply.all("ReservedCacheNodeId")
}

/// Buy every row of [`ROWS`], asserting each purchase succeeds.
async fn purchase_every_row(emulator: &Emulator) {
    for row in &ROWS {
        let reply = emulator.call(PURCHASE, &row.purchase_pairs()).await;
        assert_eq!(
            reply.status, 200,
            "case {:?}: purchase must succeed, got {}",
            row.case, reply.body
        );
    }
}

#[tokio::test]
async fn a_fresh_account_holds_no_reservations() {
    let emulator = Emulator::new();
    let reply = describe(&emulator, &[]).await;
    assert_eq!(
        reply.status, 200,
        "a describe against a fresh account must succeed, got {}",
        reply.body
    );
    assert!(
        reservation_ids(&reply).is_empty(),
        "nothing has been purchased, so no reservation may be reported, got {}",
        reply.body
    );
    assert!(
        reply.marker().is_none(),
        "an empty answer has no further page to resume from, got {}",
        reply.body
    );
}

#[tokio::test]
async fn a_purchased_reservation_reports_the_offering_it_was_bought_from() {
    let emulator = Emulator::new();
    for row in &ROWS {
        let offering_id = row.offering_id();
        // What the catalog publishes for this offering: the reservation must
        // report the SAME node type, term, engine, payment option and prices.
        let offering = emulator
            .call(
                DESCRIBE_OFFERINGS,
                &params(&[("ReservedCacheNodesOfferingId", &offering_id)]),
            )
            .await;
        assert_eq!(
            offering.status, 200,
            "case {:?}: the offering must be published, got {}",
            row.case, offering.body
        );

        let bought = emulator.call(PURCHASE, &row.purchase_pairs()).await;
        assert_eq!(
            bought.status, 200,
            "case {:?}: purchase must succeed, got {}",
            row.case, bought.body
        );

        // The reservation read BACK through the describe, which is what proves
        // the purchase was persisted rather than echoed.
        let described = describe(
            &emulator,
            &[("ReservedCacheNodeId", row.reservation_id)],
        )
        .await;
        assert_eq!(
            described.status, 200,
            "case {:?}: describe must succeed, got {}",
            row.case, described.body
        );
        assert_eq!(
            reservation_ids(&described),
            vec![row.reservation_id],
            "case {:?}: the describe must report the one reservation bought, got {}",
            row.case,
            described.body
        );

        for reply in [&bought, &described] {
            let expected: &[(&str, &str)] = &[
                ("ReservedCacheNodeId", row.reservation_id),
                ("ReservedCacheNodesOfferingId", offering_id.as_str()),
                ("CacheNodeType", row.cache_node_type),
                ("Duration", row.duration_seconds),
                ("ProductDescription", row.product_description),
                ("OfferingType", row.offering_type),
                ("CacheNodeCount", row.cache_node_count),
            ];
            for (element, wanted) in expected {
                assert_eq!(
                    reply.text(element),
                    Some(*wanted),
                    "case {:?}: {element} must be {wanted:?}, got {}",
                    row.case,
                    reply.body
                );
            }
            // The prices are the offering's own, reported per reserved cache
            // node beside the count.
            for element in ["FixedPrice", "UsagePrice", "RecurringChargeAmount"] {
                assert_eq!(
                    reply.all(element),
                    offering.all(element),
                    "case {:?}: {element} must be the offering's {:?}, got {}",
                    row.case,
                    offering.all(element),
                    reply.body
                );
            }
            assert_eq!(
                reply.all("RecurringChargeFrequency"),
                offering.all("RecurringChargeFrequency"),
                "case {:?}: the recurring charge frequency must be the offering's, got {}",
                row.case,
                reply.body
            );
            // `StartTime` is "the time the reservation started", so a bought
            // reservation carries one.
            assert!(
                reply.text("StartTime").is_some_and(|at| !at.is_empty()),
                "case {:?}: a reservation must report a StartTime, got {}",
                row.case,
                reply.body
            );
            // The ARN form `ReservedCacheNode$ReservationARN`'s own example
            // gives: `…:reserved-instance:<reservation id>`.
            assert_eq!(
                reply.text("ReservationARN"),
                Some(
                    engine::emulator_arn(
                        state::ARN_NAMESPACE,
                        REGION,
                        &format!("reserved-instance:{}", row.reservation_id)
                    )
                    .as_str()
                ),
                "case {:?}: the reservation ARN must name the reservation, got {}",
                row.case,
                reply.body
            );
        }

        // The same reservation, selected by the attributes it was bought with
        // instead of by its identifier — with `Duration` in the form this row
        // sends, because a year count and its second count name one term. Each
        // row's four attributes name a different catalog entry, so the filter
        // selects exactly this reservation.
        let by_attributes = describe(
            &emulator,
            &[
                ("ProductDescription", row.product_description),
                ("CacheNodeType", row.cache_node_type),
                ("Duration", row.duration_sent),
                ("OfferingType", row.offering_type),
            ],
        )
        .await;
        assert_eq!(
            reservation_ids(&by_attributes),
            vec![row.reservation_id],
            "case {:?}: the attributes it was bought with must select it (Duration {:?}), got {}",
            row.case,
            row.duration_sent,
            by_attributes.body
        );
    }
}

#[tokio::test]
async fn a_reservation_moves_from_payment_pending_to_active() {
    let emulator = Emulator::new();
    let row = &ROWS[0];
    let bought = emulator.call(PURCHASE, &row.purchase_pairs()).await;
    assert_eq!(
        bought.status, 200,
        "purchase must succeed, got {}",
        bought.body
    );
    // The purchase's own answer is the transient state: the span is never zero.
    assert_eq!(
        bought.text("State"),
        Some(state::RESERVATION_STATE_PAYMENT_PENDING),
        "a fresh purchase must be {}, got {}",
        state::RESERVATION_STATE_PAYMENT_PENDING,
        bought.body
    );

    // The read that follows is answered once the clock owes the reservation its
    // next state.
    let settled = describe(&emulator, &[("ReservedCacheNodeId", row.reservation_id)]).await;
    assert_eq!(
        settled.text("State"),
        Some(state::RESERVATION_STATE_ACTIVE),
        "the read after the purchase must report {}, got {}",
        state::RESERVATION_STATE_ACTIVE,
        settled.body
    );
    // A read may not change what a later caller sees: the term is years long,
    // so a second read reports the same active reservation, with the same
    // identity and the same start time.
    let again = describe(&emulator, &[("ReservedCacheNodeId", row.reservation_id)]).await;
    for element in [
        "State",
        "StartTime",
        "ReservedCacheNodeId",
        "ReservationARN",
        "CacheNodeCount",
    ] {
        assert_eq!(
            again.text(element),
            settled.text(element),
            "{element} must be stable across reads, got {}",
            again.body
        );
    }
}

#[tokio::test]
async fn a_purchase_that_names_no_identifier_is_given_one() {
    let emulator = Emulator::new();
    let offering_id = ROWS[0].offering_id();
    let mut minted: Vec<String> = Vec::new();
    for attempt in 0..3 {
        let bought = purchase(
            &emulator,
            &[("ReservedCacheNodesOfferingId", offering_id.as_str())],
        )
        .await;
        assert_eq!(
            bought.status, 200,
            "attempt {attempt}: purchase must succeed, got {}",
            bought.body
        );
        let id = bought
            .text("ReservedCacheNodeId")
            .unwrap_or_default()
            .to_string();
        // The form `ReservationARN`'s own example carries: `ri-` and the
        // purchase instant, every field hyphen-separated.
        assert!(
            id.starts_with("ri-")
                && id.len() > 3
                && id[3..].bytes().all(|b| b.is_ascii_digit() || b == b'-'),
            "attempt {attempt}: a generated identifier must have the form ri-<instant>, got {id:?}"
        );
        assert!(
            !minted.contains(&id),
            "attempt {attempt}: a generated identifier must be unique, got {id:?} twice"
        );
        // The generated id names the reservation that was stored.
        let described = describe(&emulator, &[("ReservedCacheNodeId", id.as_str())]).await;
        assert_eq!(
            reservation_ids(&described),
            vec![id.as_str()],
            "attempt {attempt}: the generated id must name the stored reservation, got {}",
            described.body
        );
        minted.push(id);
    }
    let all = describe(&emulator, &[]).await;
    assert_eq!(
        reservation_ids(&all).len(),
        minted.len(),
        "every generated reservation must be reported, got {}",
        all.body
    );
}

/// One filter row: the case name, the form pairs sent, and the reservations of
/// [`ROWS`] that must come back, by `ReservedCacheNodeId`.
struct FilterCase {
    case: &'static str,
    pairs: &'static [(&'static str, &'static str)],
    expect: &'static [&'static str],
}

#[tokio::test]
async fn every_documented_filter_member_narrows_the_reservations() {
    // The axis: each filter member `DescribeReservedCacheNodes` documents, over
    // documented values of it — every engine, node types from both generations,
    // both terms in both the year and the second form, each stocked payment
    // option, an offering id, a value nothing was bought for, and several at
    // once.
    let cases: &[FilterCase] = &[
        FilterCase {
            case: "no filter at all",
            pairs: &[],
            expect: &[
                "memcached-r5-3y-partial",
                "memcached-t4g-3y-no",
                "redis-m3-3y-all",
                "redis-m7g-1y-all",
                "valkey-r6g-1y-partial",
                "valkey-t3-1y-no",
            ],
        },
        FilterCase {
            case: "product description redis",
            pairs: &[("ProductDescription", "redis")],
            expect: &["redis-m3-3y-all", "redis-m7g-1y-all"],
        },
        FilterCase {
            case: "product description memcached",
            pairs: &[("ProductDescription", "memcached")],
            expect: &["memcached-r5-3y-partial", "memcached-t4g-3y-no"],
        },
        FilterCase {
            case: "product description valkey",
            pairs: &[("ProductDescription", "valkey")],
            expect: &["valkey-r6g-1y-partial", "valkey-t3-1y-no"],
        },
        FilterCase {
            case: "product description nothing was bought for",
            pairs: &[("ProductDescription", "postgres")],
            expect: &[],
        },
        FilterCase {
            case: "current generation node type",
            pairs: &[("CacheNodeType", "cache.m7g.large")],
            expect: &["redis-m7g-1y-all"],
        },
        FilterCase {
            case: "previous generation node type",
            pairs: &[("CacheNodeType", "cache.m3.medium")],
            expect: &["redis-m3-3y-all"],
        },
        FilterCase {
            case: "node type nothing was bought on",
            pairs: &[("CacheNodeType", "cache.r7g.large")],
            expect: &[],
        },
        FilterCase {
            case: "one year term in years",
            pairs: &[("Duration", "1")],
            expect: &[
                "redis-m7g-1y-all",
                "valkey-r6g-1y-partial",
                "valkey-t3-1y-no",
            ],
        },
        FilterCase {
            case: "one year term in seconds",
            pairs: &[("Duration", "31536000")],
            expect: &[
                "redis-m7g-1y-all",
                "valkey-r6g-1y-partial",
                "valkey-t3-1y-no",
            ],
        },
        FilterCase {
            case: "three year term in years",
            pairs: &[("Duration", "3")],
            expect: &[
                "memcached-r5-3y-partial",
                "memcached-t4g-3y-no",
                "redis-m3-3y-all",
            ],
        },
        FilterCase {
            case: "three year term in seconds",
            pairs: &[("Duration", "94608000")],
            expect: &[
                "memcached-r5-3y-partial",
                "memcached-t4g-3y-no",
                "redis-m3-3y-all",
            ],
        },
        FilterCase {
            case: "all upfront",
            pairs: &[("OfferingType", "All Upfront")],
            expect: &["redis-m3-3y-all", "redis-m7g-1y-all"],
        },
        FilterCase {
            case: "partial upfront",
            pairs: &[("OfferingType", "Partial Upfront")],
            expect: &["memcached-r5-3y-partial", "valkey-r6g-1y-partial"],
        },
        FilterCase {
            case: "no upfront",
            pairs: &[("OfferingType", "No Upfront")],
            expect: &["memcached-t4g-3y-no", "valkey-t3-1y-no"],
        },
        FilterCase {
            case: "offering type in another case",
            pairs: &[("OfferingType", "no upfront")],
            expect: &["memcached-t4g-3y-no", "valkey-t3-1y-no"],
        },
        // A documented `OfferingType` the catalog stocks none of: served, and
        // nothing was bought under it.
        FilterCase {
            case: "legacy heavy utilization",
            pairs: &[("OfferingType", "Heavy Utilization")],
            expect: &[],
        },
        FilterCase {
            case: "engine and term together",
            pairs: &[("ProductDescription", "valkey"), ("Duration", "1")],
            expect: &[
                "valkey-r6g-1y-partial",
                "valkey-t3-1y-no",
            ],
        },
        FilterCase {
            case: "every narrowing member at once",
            pairs: &[
                ("ProductDescription", "memcached"),
                ("CacheNodeType", "cache.t4g.micro"),
                ("Duration", "3"),
                ("OfferingType", "No Upfront"),
            ],
            expect: &["memcached-t4g-3y-no"],
        },
        FilterCase {
            case: "a combination nothing was bought under",
            pairs: &[
                ("ProductDescription", "memcached"),
                ("CacheNodeType", "cache.m7g.large"),
            ],
            expect: &[],
        },
    ];

    let emulator = Emulator::new();
    purchase_every_row(&emulator).await;

    for FilterCase {
        case,
        pairs,
        expect,
    } in cases
    {
        let reply = describe(&emulator, pairs).await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: a documented filter value must be served, got {}",
            reply.body
        );
        assert_eq!(
            reservation_ids(&reply),
            *expect,
            "case {case:?}: expected {expect:?}, got {}",
            reply.body
        );
    }

    // The offering id is a filter member of its own: it selects the one
    // reservation bought from that catalog entry, and an offering nothing was
    // bought from is an empty page rather than a refusal, because this
    // operation models no not-found for an offering.
    for row in &ROWS {
        let reply = describe(
            &emulator,
            &[("ReservedCacheNodesOfferingId", row.offering_id().as_str())],
        )
        .await;
        assert_eq!(
            reservation_ids(&reply),
            vec![row.reservation_id],
            "case {:?}: the offering id must select the reservation bought from it, got {}",
            row.case,
            reply.body
        );
    }
    let unbought = state::offering_id("redis", "cache.r7g.large", 31_536_000, "All Upfront");
    let reply = describe(
        &emulator,
        &[("ReservedCacheNodesOfferingId", unbought.as_str())],
    )
    .await;
    assert_eq!(reply.status, 200, "an offering nothing was bought from must be served, got {}", reply.body);
    assert!(
        reservation_ids(&reply).is_empty(),
        "no reservation was bought from that offering, got {}",
        reply.body
    );
}

/// One refusal row: the case name, the action, the form pairs sent, and the
/// HTTP status plus `awsQuery` error code the request must be answered with (an
/// empty code meaning the request is served).
type RefusalCase = (
    &'static str,
    &'static str,
    &'static [(&'static str, &'static str)],
    u16,
    &'static str,
);

#[tokio::test]
async fn requests_outside_the_documented_sets_are_refused() {
    let emulator = Emulator::new();
    purchase_every_row(&emulator).await;

    // The axis: each member with its own documented value set — `Duration`
    // ("Valid Values: 1 | 3 | 31536000 | 94608000"), `OfferingType`,
    // `MaxRecords` ("minimum 20; maximum 100") on each published bound and one
    // past it, a `Marker` this service did not mint, an identifier no
    // reservation is held under, an offering the catalog does not publish, an
    // identifier already taken, and a node count that cannot be one.
    let cases: &[RefusalCase] = &[
        (
            "term of two years",
            DESCRIBE,
            &[("Duration", "2")],
            400,
            "InvalidParameterValue",
        ),
        (
            "a second past the one year term",
            DESCRIBE,
            &[("Duration", "31536001")],
            400,
            "InvalidParameterValue",
        ),
        (
            "term that is not a number",
            DESCRIBE,
            &[("Duration", "one year")],
            400,
            "InvalidParameterValue",
        ),
        (
            "offering type nobody sells",
            DESCRIBE,
            &[("OfferingType", "Gold Utilization")],
            400,
            "InvalidParameterValue",
        ),
        (
            "page size at the published minimum",
            DESCRIBE,
            &[("MaxRecords", "20")],
            200,
            "",
        ),
        (
            "page size one below the minimum",
            DESCRIBE,
            &[("MaxRecords", "19")],
            400,
            "InvalidParameterValue",
        ),
        (
            "page size at the published maximum",
            DESCRIBE,
            &[("MaxRecords", "100")],
            200,
            "",
        ),
        (
            "page size one above the maximum",
            DESCRIBE,
            &[("MaxRecords", "101")],
            400,
            "InvalidParameterValue",
        ),
        (
            "marker this service did not mint",
            DESCRIBE,
            &[("Marker", "not-a-marker")],
            400,
            "InvalidParameterValue",
        ),
        (
            "an identifier no reservation is held under",
            DESCRIBE,
            &[("ReservedCacheNodeId", "myreservationID")],
            404,
            "ReservedCacheNodeNotFound",
        ),
        // An identifier differing only in case names a different reservation:
        // the id is the customer's own, stored as it was given.
        (
            "an identifier that differs only in case",
            DESCRIBE,
            &[("ReservedCacheNodeId", "REDIS-M7G-1Y-ALL")],
            404,
            "ReservedCacheNodeNotFound",
        ),
        // The example from `ReservedCacheNodesOfferingId`'s own documentation:
        // a well-formed id the catalog does not publish.
        (
            "purchase of an offering the catalog does not publish",
            PURCHASE,
            &[(
                "ReservedCacheNodesOfferingId",
                "438012d3-4052-4cc7-b2e3-8d3372e0e706",
            )],
            404,
            "ReservedCacheNodesOfferingNotFound",
        ),
    ];

    for (case, action, pairs, status, code) in cases {
        let reply = emulator.call(action, &params(pairs)).await;
        assert_eq!(
            reply.status, *status,
            "case {case:?}: expected HTTP {status}, got {} with {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            *code,
            "case {case:?}: expected error code {code:?}, got {}",
            reply.body
        );
    }

    // A refusal says WHICH value it is about: the message of each not-found is
    // built from the request, so it names the identifier the caller asked for.
    for (case, action, member, value) in [
        (
            "unknown reservation",
            DESCRIBE,
            "ReservedCacheNodeId",
            "myreservationID",
        ),
        (
            "unknown offering",
            PURCHASE,
            "ReservedCacheNodesOfferingId",
            "438012d3-4052-4cc7-b2e3-8d3372e0e706",
        ),
    ] {
        let reply = emulator.call(action, &params(&[(member, value)])).await;
        let message = reply.text("Error/Message").unwrap_or_default();
        assert!(
            message.contains(value),
            "case {case:?}: the refusal must name the {member} asked for, got {message:?}"
        );
    }

    // A node count that cannot be a number of nodes, and an identifier this
    // account already holds a reservation under: both need the offering id of a
    // published entry, so they are built here rather than in the table above.
    let offering_id = ROWS[0].offering_id();
    for (case, count) in [("zero nodes", "0"), ("a negative count", "-1")] {
        let reply = purchase(
            &emulator,
            &[
                ("ReservedCacheNodesOfferingId", offering_id.as_str()),
                ("ReservedCacheNodeId", "count-case"),
                ("CacheNodeCount", count),
            ],
        )
        .await;
        assert_eq!(
            reply.status, 400,
            "case {case:?}: expected HTTP 400, got {}",
            reply.body
        );
        assert_eq!(
            reply.error_code(),
            "InvalidParameterValue",
            "case {case:?}: expected InvalidParameterValue, got {}",
            reply.body
        );
    }
    let taken = purchase(
        &emulator,
        &[
            ("ReservedCacheNodesOfferingId", offering_id.as_str()),
            ("ReservedCacheNodeId", ROWS[0].reservation_id),
        ],
    )
    .await;
    assert_eq!(
        taken.status, 404,
        "an identifier already taken must be refused, got {}",
        taken.body
    );
    assert_eq!(
        taken.error_code(),
        "ReservedCacheNodeAlreadyExists",
        "an identifier already taken must be refused as already existing, got {}",
        taken.body
    );
    // The refusals changed nothing: the six rows are still the only
    // reservations, each reported once.
    let all = describe(&emulator, &[]).await;
    assert_eq!(
        reservation_ids(&all).len(),
        ROWS.len(),
        "a refused purchase must store nothing, got {}",
        all.body
    );
}

#[tokio::test]
async fn reserving_past_the_store_bound_is_refused() {
    let emulator = Emulator::new();
    let offering_id = ROWS[0].offering_id();
    // The bound this store holds reservations up to
    // (`state::RESERVED_CACHE_NODE_QUOTA`): reserving exactly that many nodes
    // is served, and one node more is the modelled quota refusal.
    let at_bound = state::RESERVED_CACHE_NODE_QUOTA.to_string();
    let filled = purchase(
        &emulator,
        &[
            ("ReservedCacheNodesOfferingId", offering_id.as_str()),
            ("ReservedCacheNodeId", "at-the-bound"),
            ("CacheNodeCount", at_bound.as_str()),
        ],
    )
    .await;
    assert_eq!(
        filled.status, 200,
        "a reservation of exactly {at_bound} nodes must be served, got {}",
        filled.body
    );
    let past = purchase(
        &emulator,
        &[
            ("ReservedCacheNodesOfferingId", offering_id.as_str()),
            ("ReservedCacheNodeId", "one-too-many"),
            ("CacheNodeCount", "1"),
        ],
    )
    .await;
    assert_eq!(
        past.status, 400,
        "a reservation past the bound must be refused, got {}",
        past.body
    );
    assert_eq!(
        past.error_code(),
        "ReservedCacheNodeQuotaExceeded",
        "a reservation past the bound must be refused as over quota, got {}",
        past.body
    );
    let all = describe(&emulator, &[]).await;
    assert_eq!(
        reservation_ids(&all),
        vec!["at-the-bound"],
        "the refused purchase must store nothing, got {}",
        all.body
    );
}

#[tokio::test]
async fn pagination_walks_every_reservation_exactly_once() {
    let emulator = Emulator::new();
    let offering_id = ROWS[0].offering_id();
    // More reservations than the smallest page the operation serves
    // (`MaxRecords` "minimum 20"), so the walk needs a second page.
    let total = 25;
    let mut expected: Vec<String> = Vec::new();
    for index in 0..total {
        let id = format!("page-{index:02}");
        let reply = purchase(
            &emulator,
            &[
                ("ReservedCacheNodesOfferingId", offering_id.as_str()),
                ("ReservedCacheNodeId", id.as_str()),
            ],
        )
        .await;
        assert_eq!(
            reply.status, 200,
            "purchase {id} must succeed, got {}",
            reply.body
        );
        expected.push(id);
    }
    // The store's own order, which is the total order the `Marker` indexes
    // into.
    expected.sort();

    let first = describe(&emulator, &[("MaxRecords", "20")]).await;
    assert_eq!(
        first.status, 200,
        "the first page must be served, got {}",
        first.body
    );
    let first_ids: Vec<String> = reservation_ids(&first)
        .into_iter()
        .map(str::to_string)
        .collect();
    assert_eq!(
        first_ids.len(),
        20,
        "a page followed by a Marker must be full, got {}",
        first.body
    );
    let marker = first
        .marker()
        .unwrap_or_else(|| panic!("more records exist than the page size, so the page must carry a Marker: {}", first.body))
        .to_string();

    let second = describe(&emulator, &[("MaxRecords", "20"), ("Marker", marker.as_str())]).await;
    assert_eq!(
        second.status, 200,
        "the second page must be served, got {}",
        second.body
    );
    let second_ids: Vec<String> = reservation_ids(&second)
        .into_iter()
        .map(str::to_string)
        .collect();
    assert_eq!(
        second_ids.len(),
        total - 20,
        "the last page holds the remaining records, got {}",
        second.body
    );
    assert!(
        second.marker().is_none(),
        "the last page has nothing to resume from, got {}",
        second.body
    );

    let walked: Vec<String> = first_ids.into_iter().chain(second_ids).collect();
    assert_eq!(
        walked, expected,
        "paging must walk every reservation exactly once, in the store's order"
    );
}
