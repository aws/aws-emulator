// SPDX-License-Identifier: CC0-1.0

//! The refusal a contradictory `CacheUsageLimits` envelope is owed, over the whole
//! path a client walks: the VPC network it stands up through Amazon EC2 first, the
//! create it places in those subnets, the live cache a refused modify must leave
//! alone, and the cache it deletes afterwards.
//!
//! The rule under test: a `CacheUsageLimits` member whose `Minimum` would end up
//! greater than its `Maximum` names capacity no cache can run, so every operation
//! that would STORE such an envelope (`CreateServerlessCache`,
//! `ModifyServerlessCache`) refuses it with the modelled client fault — HTTP 400,
//! `awsQuery` code `InvalidParameterValue` — in a sentence naming the offending
//! member, before anything is written; a consistent envelope, including one whose
//! two bounds are equal, is accepted and reported back exactly as sent.
//!
//! `CacheUsageLimits.DataStorage{Minimum,Maximum}` and
//! `CacheUsageLimits.ECPUPerSecond{Minimum,Maximum}` are plain `IntegerOptional`
//! with no modelled `@range`
//! (`smithy_query(query="shape",shape="DataStorage,ECPUPerSecond")`), so no
//! boundary check stops a contradictory envelope: the refusal is the operation's,
//! and it is the one a client branches on — a non-modelled status is neither a
//! client fault nor retryable to any SDK.
//!
//! The tables are inputs, not one recorded request: the rows differ along what the
//! rule generalises over — which member is contradictory, whether the request names
//! both bounds or only the one that contradicts what the cache already runs, how far
//! apart the two bounds are (far, one apart, and the two published bounds inverted),
//! and which operation is asked. Nothing reaches into either service's state: the
//! network exists only because EC2 answered for it, and a cache's limits only because
//! a later `DescribeServerlessCaches` reported them.

use service_com_amazonaws_elasticache_amazonelasticachev9::state;

use crate::harness::{params, poll_until, Emulator, Reply, REGION};

/// The Availability Zones the fixture's subnets sit in, written `<region><letter>` —
/// the form AWS names a zone of a Region in. One per subnet, so the fixture spans
/// three zones the way a client's own network does.
const ZONES: [&str; 3] = ["us-east-1a", "us-east-1b", "us-east-1c"];

/// The storage unit every row sends, the one `DataStorage$Unit` (a required
/// `DataStorageUnit` enum) publishes.
const STORAGE_UNIT: &str = "GB";

/// The engine the rows run on: `Engine` "must be set to valkey, memcached or redis",
/// and a usage-limit envelope belongs to a serverless cache of any of them.
const ENGINE: &str = "valkey";

/// The form pairs one request sends beside its action, as [`params`] builds them.
type Pairs = Vec<(String, String)>;

/// One refusal row: the case name, the usage-limit pairs the request sends, and the
/// member names whose sentence the refusal may quote (one entry where only one member
/// of the envelope is contradictory).
type RefusalRow = (&'static str, Pairs, Vec<&'static str>);

/// One accepted row: the case name, the usage-limit pairs the request sends, and every
/// `(element suffix, text)` a later read of the cache must report.
type AcceptedRow = (&'static str, Pairs, Vec<(&'static str, String)>);

/// The sentence a contradictory envelope is refused with, whose only variable part is
/// the member the offending bounds belong to.
fn inverted_sentence(member: &str) -> String {
    format!("Invalid values for {member}. Minimum must not be greater than Maximum.")
}

/// An emulator with the account network a client stands up before it places a cache:
/// a VPC of its own, one subnet per zone of [`ZONES`], an internet gateway attached
/// to it and a route table for it.
///
/// Built through Amazon EC2's own API over the in-process callback, because a subnet
/// and a gateway are EC2's resources and their identifiers are EC2's to mint — the
/// very place this service resolves a placement against. A client that stands up this
/// fixture and then sends a contradictory envelope must be answered by the
/// ElastiCache operation, so every call of the fixture is asserted to succeed here.
struct Fixture {
    emulator: Emulator,
    /// The subnets `ec2:CreateSubnet` minted, in zone order, as EC2 reported them.
    subnets: Vec<String>,
}

impl Fixture {
    async fn stand_up() -> Self {
        let emulator = Emulator::new();
        let ec2 = emulator.ec2(REGION);
        let vpc = ec2
            .create_vpc()
            .cidr_block("10.21.0.0/16")
            .send()
            .await
            .unwrap_or_else(|error| panic!("EC2 must create the client's VPC: {error:?}"))
            .vpc()
            .and_then(::aws_sdk_ec2::types::Vpc::vpc_id)
            .expect("CreateVpc must report the VPC it minted")
            .to_string();

        let mut subnets = Vec::with_capacity(ZONES.len());
        for (index, zone) in ZONES.iter().enumerate() {
            let subnet = ec2
                .create_subnet()
                .vpc_id(&vpc)
                .cidr_block(format!("10.21.{}.0/24", index + 1))
                .availability_zone(*zone)
                .send()
                .await
                .unwrap_or_else(|error| panic!("EC2 must create a subnet in {zone}: {error:?}"))
                .subnet()
                .and_then(::aws_sdk_ec2::types::Subnet::subnet_id)
                .unwrap_or_else(|| panic!("CreateSubnet must report the subnet it minted in {zone}"))
                .to_string();
            subnets.push(subnet);
        }

        let gateway = ec2
            .create_internet_gateway()
            .send()
            .await
            .unwrap_or_else(|error| panic!("EC2 must create the internet gateway: {error:?}"))
            .internet_gateway()
            .and_then(::aws_sdk_ec2::types::InternetGateway::internet_gateway_id)
            .expect("CreateInternetGateway must report the gateway it minted")
            .to_string();
        ec2.attach_internet_gateway()
            .internet_gateway_id(&gateway)
            .vpc_id(&vpc)
            .send()
            .await
            .unwrap_or_else(|error| panic!("EC2 must attach the internet gateway: {error:?}"));
        ec2.create_route_table()
            .vpc_id(&vpc)
            .send()
            .await
            .unwrap_or_else(|error| panic!("EC2 must create the route table: {error:?}"))
            .route_table()
            .and_then(::aws_sdk_ec2::types::RouteTable::route_table_id)
            .expect("CreateRouteTable must report the table it minted");

        Self { emulator, subnets }
    }

    /// The fixture's subnets as the `SubnetIds` pairs a request names them in.
    fn subnet_pairs(&self) -> Pairs {
        self.subnets
            .iter()
            .enumerate()
            .map(|(index, id)| (format!("SubnetIds.SubnetId.{}", index + 1), id.clone()))
            .collect()
    }

    /// `CreateServerlessCache` for `name`, placed in the fixture's subnets, with the
    /// usage-limit pairs a row sends.
    async fn create(&self, name: &str, limits: &[(String, String)]) -> Reply {

        let mut request = params(&[("ServerlessCacheName", name), ("Engine", ENGINE)]);
        request.extend(limits.iter().cloned());
        request.extend(self.subnet_pairs());
        self.emulator.call("CreateServerlessCache", &request).await
    }

    /// `ModifyServerlessCache` for `name` with the usage-limit pairs a row sends.
    async fn modify(&self, name: &str, limits: &[(String, String)]) -> Reply {
        let mut request = params(&[("ServerlessCacheName", name)]);
        request.extend(limits.iter().cloned());
        self.emulator.call("ModifyServerlessCache", &request).await
    }

    /// `DescribeServerlessCaches` for the one cache `name` resolves to.
    async fn describe(&self, name: &str) -> Reply {
        self.emulator
            .call(
                "DescribeServerlessCaches",
                &params(&[("ServerlessCacheName", name)]),
            )
            .await
    }

    /// The cache `name` resolves to, once the clock owes it `status`.
    async fn settled(&self, case: &str, name: &str, status: &str) -> Reply {
        poll_until(
            &self.emulator,
            REGION,
            case,
            &format!("the cache reaching {status}"),
            "DescribeServerlessCaches",
            &params(&[("ServerlessCacheName", name)]),
            |reply| reply.text("member/Status") == Some(status),
        )
        .await
    }
}

/// The `DataStorage` pairs for the bounds a row names (`None` leaves a bound unsent,
/// which is how a request contradicts only what the cache already runs).
fn storage(minimum: Option<i32>, maximum: Option<i32>) -> Pairs {
    let mut pairs = vec![(
        "CacheUsageLimits.DataStorage.Unit".to_string(),
        STORAGE_UNIT.to_string(),
    )];
    if let Some(minimum) = minimum {
        pairs.push((
            "CacheUsageLimits.DataStorage.Minimum".to_string(),
            minimum.to_string(),
        ));
    }
    if let Some(maximum) = maximum {
        pairs.push((
            "CacheUsageLimits.DataStorage.Maximum".to_string(),
            maximum.to_string(),
        ));
    }
    pairs
}

/// The `ECPUPerSecond` pairs for the bounds a row names; see [`storage`].
fn ecpu(minimum: Option<i32>, maximum: Option<i32>) -> Pairs {
    let mut pairs = Vec::new();
    if let Some(minimum) = minimum {
        pairs.push((
            "CacheUsageLimits.ECPUPerSecond.Minimum".to_string(),
            minimum.to_string(),
        ));
    }
    if let Some(maximum) = maximum {
        pairs.push((
            "CacheUsageLimits.ECPUPerSecond.Maximum".to_string(),
            maximum.to_string(),
        ));
    }
    pairs
}

/// `left` followed by `right`, for a row that names both members.
fn both(left: Pairs, right: Pairs) -> Pairs {
    let mut pairs = left;
    pairs.extend(right);
    pairs
}

/// Assert that `reply` is the modelled client refusal — HTTP 400 with the `awsQuery`
/// code `InvalidParameterValue` — whose message is the inverted-envelope sentence for
/// one of `members`.
fn assert_inverted_refusal(case: &str, reply: &Reply, members: &[&str]) {
    assert_eq!(
        reply.status, 400,
        "case {case:?}: a contradictory usage-limit envelope must be refused with HTTP 400, \
         got {} {}",
        reply.status, reply.body
    );
    assert_eq!(
        reply.error_code(),
        "InvalidParameterValue",
        "case {case:?}: expected the modelled error code InvalidParameterValue, got {:?} ({})",
        reply.error_code(),
        reply.body
    );
    let expected: Vec<String> = members.iter().copied().map(inverted_sentence).collect();
    assert!(
        expected.iter().any(|sentence| sentence == reply.error_message()),
        "case {case:?}: expected one of the sentences {expected:?}, got {:?}",
        reply.error_message()
    );
}

/// Assert that `reply` reports a not-found fault for a cache this account does not
/// hold.
fn assert_not_found(case: &str, reply: &Reply) {
    assert_eq!(
        reply.status, 404,
        "case {case:?}: a cache this account does not hold must answer HTTP 404, got {} {}",
        reply.status, reply.body
    );
    assert_eq!(
        reply.error_code(),
        "ServerlessCacheNotFoundFault",
        "case {case:?}: expected ServerlessCacheNotFoundFault, got {:?} ({})",
        reply.error_code(),
        reply.body
    );
}

#[tokio::test]
async fn a_create_refuses_every_contradictory_usage_limit_envelope_and_stores_nothing() {
    let fixture = Fixture::stand_up().await;

    // The axis: which member contradicts itself, how far apart its two bounds are,
    // and whether the other member is consistent beside it. The bounds the rows at
    // the published edges use are the ones `state::validate_cache_usage_limits`
    // publishes for `DataStorage` (`DATA_STORAGE_MIN_GB` ..= `DATA_STORAGE_MAX_GB`),
    // so no literal here stands in for them.
    let rows: Vec<RefusalRow> = vec![
        (
            "storage bounds far apart the wrong way round",
            storage(Some(50), Some(10)),
            vec!["DataStorage"],
        ),
        (
            "storage bounds one apart the wrong way round",
            storage(Some(2), Some(1)),
            vec!["DataStorage"],
        ),
        (
            "the two published storage bounds, inverted",
            storage(
                Some(state::DATA_STORAGE_MAX_GB),
                Some(state::DATA_STORAGE_MIN_GB),
            ),
            vec!["DataStorage"],
        ),
        (
            "ECPU bounds far apart the wrong way round",
            ecpu(Some(9000), Some(1000)),
            vec!["ECPUPerSecond"],
        ),
        (
            "ECPU bounds one apart the wrong way round",
            ecpu(Some(1001), Some(1000)),
            vec!["ECPUPerSecond"],
        ),
        (
            "a contradictory ECPU envelope beside a consistent storage one",
            both(storage(Some(1), Some(10)), ecpu(Some(9000), Some(1000))),
            vec!["ECPUPerSecond"],
        ),
        (
            "a contradictory storage envelope beside a consistent ECPU one",
            both(storage(Some(50), Some(10)), ecpu(Some(1000), Some(5000))),
            vec!["DataStorage"],
        ),
        (
            // Both members are contradictory, so the sentence names one of them.
            "the recorded envelope with both members inverted",
            both(storage(Some(50), Some(10)), ecpu(Some(9000), Some(1000))),
            vec!["DataStorage", "ECPUPerSecond"],
        ),
    ];

    for (case, limits, members) in &rows {
        let refused = fixture.create("inverted-cache", limits).await;
        assert_inverted_refusal(case, &refused, members);

        // And the refusal came before anything was written: the account holds no
        // cache under that name.
        let read_back = fixture.describe("inverted-cache").await;
        assert_not_found(case, &read_back);
    }
}

#[tokio::test]
async fn a_create_accepts_a_consistent_usage_limit_envelope_and_reports_it_back() {
    let fixture = Fixture::stand_up().await;

    // The axis: the distance between the two bounds — a plain range, the two equal
    // (the case just inside the rule), and the published storage bounds the right way
    // round — and whether a member is named at all.
    let rows: Vec<AcceptedRow> = vec![
        (
            "a plain range on both members",
            both(storage(Some(1), Some(10)), ecpu(Some(1000), Some(5000))),
            vec![
                ("DataStorage/Minimum", "1".to_string()),
                ("DataStorage/Maximum", "10".to_string()),
                ("ECPUPerSecond/Minimum", "1000".to_string()),
                ("ECPUPerSecond/Maximum", "5000".to_string()),
            ],
        ),
        (
            "equal bounds on both members",
            both(storage(Some(10), Some(10)), ecpu(Some(2000), Some(2000))),
            vec![
                ("DataStorage/Minimum", "10".to_string()),
                ("DataStorage/Maximum", "10".to_string()),
                ("ECPUPerSecond/Minimum", "2000".to_string()),
                ("ECPUPerSecond/Maximum", "2000".to_string()),
            ],
        ),
        (
            "the two published storage bounds the right way round",
            storage(
                Some(state::DATA_STORAGE_MIN_GB),
                Some(state::DATA_STORAGE_MAX_GB),
            ),
            vec![
                (
                    "DataStorage/Minimum",
                    state::DATA_STORAGE_MIN_GB.to_string(),
                ),
                (
                    "DataStorage/Maximum",
                    state::DATA_STORAGE_MAX_GB.to_string(),
                ),
            ],
        ),
        (
            "one bound on its own, which bounds nothing it could invert",
            ecpu(Some(1000), None),
            vec![("ECPUPerSecond/Minimum", "1000".to_string())],
        ),
    ];

    for (index, (case, limits, reported)) in rows.iter().enumerate() {
        let name = format!("consistent-cache-{index}");
        let created = fixture.create(&name, limits).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: a consistent envelope must be accepted with HTTP 200, got {} {}",
            created.status, created.body
        );
        assert_eq!(
            created.text("Status"),
            Some(state::SERVERLESS_STATUS_CREATING),
            "case {case:?}: a created cache must answer the transient status, got {:?}",
            created.text("Status")
        );

        // The round trip: the limits a later read reports are the ones sent, and the
        // placement is the subnets EC2 minted for this account.
        let settled = fixture
            .settled(case, &name, state::SERVERLESS_STATUS_AVAILABLE)
            .await;
        for (member, expected) in reported {
            assert_eq!(
                settled.text(member),
                Some(expected.as_str()),
                "case {case:?}: expected {member} {expected:?} after the round trip, got {:?} ({})",
                settled.text(member),
                settled.body
            );
        }
        assert_eq!(
            settled.all("SubnetId"),
            fixture
                .subnets
                .iter()
                .map(String::as_str)
                .collect::<Vec<&str>>(),
            "case {case:?}: expected the cache to report the subnets it was placed in, got {:?}",
            settled.all("SubnetId")
        );
    }
}

#[tokio::test]
async fn a_refused_modify_leaves_the_live_cache_running_the_limits_it_had() {
    let fixture = Fixture::stand_up().await;
    let created = fixture
        .create(
            "live-cache",
            &both(storage(Some(1), Some(10)), ecpu(Some(1000), Some(5000))),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "the baseline create must answer HTTP 200, got {} {}",
        created.status, created.body
    );
    fixture
        .settled(
            "the baseline",
            "live-cache",
            state::SERVERLESS_STATUS_AVAILABLE,
        )
        .await;

    /// What the baseline cache runs, which every refused modify must leave in place.
    const BASELINE: [(&str, &str); 5] = [
        ("DataStorage/Unit", STORAGE_UNIT),
        ("DataStorage/Minimum", "1"),
        ("DataStorage/Maximum", "10"),
        ("ECPUPerSecond/Minimum", "1000"),
        ("ECPUPerSecond/Maximum", "5000"),
    ];

    // The axis: whether the request carries the contradiction on its own, or carries
    // ONE bound that contradicts the one the cache is already running — the envelope
    // a modify would end up with is what the rule is about, not the pairs the request
    // happens to name.
    let rows: Vec<RefusalRow> = vec![
        (
            "both storage bounds inverted in the request",
            storage(Some(50), Some(10)),
            vec!["DataStorage"],
        ),
        (
            "both ECPU bounds inverted in the request",
            ecpu(Some(9000), Some(1000)),
            vec!["ECPUPerSecond"],
        ),
        (
            "a storage minimum above the maximum the cache is running",
            storage(Some(20), None),
            vec!["DataStorage"],
        ),
        (
            "an ECPU maximum below the minimum the cache is running",
            ecpu(None, Some(500)),
            vec!["ECPUPerSecond"],
        ),
    ];

    for (case, limits, members) in &rows {
        let refused = fixture.modify("live-cache", limits).await;
        assert_inverted_refusal(case, &refused, members);

        // Nothing was written: the live cache still runs every value it was created
        // with, in the status it was in.
        let untouched = fixture.describe("live-cache").await;
        for (member, expected) in BASELINE {
            assert_eq!(
                untouched.text(member),
                Some(expected),
                "case {case:?}: a refused modify must leave {member} at {expected:?}, got {:?} ({})",
                untouched.text(member),
                untouched.body
            );
        }
        assert_eq!(
            untouched.text("Status"),
            Some(state::SERVERLESS_STATUS_AVAILABLE),
            "case {case:?}: a refused modify must leave the cache available, got {:?}",
            untouched.text("Status")
        );
    }

    // And the operation still works: a consistent widening of the same two members is
    // applied and reported.
    let widened = fixture
        .modify(
            "live-cache",
            &both(storage(Some(1), Some(20)), ecpu(Some(1000), Some(9000))),
        )
        .await;
    assert_eq!(
        widened.status, 200,
        "a consistent widening must answer HTTP 200, got {} {}",
        widened.status, widened.body
    );
    let settled = fixture
        .settled(
            "the widening",
            "live-cache",
            state::SERVERLESS_STATUS_AVAILABLE,
        )
        .await;
    for (member, expected) in [
        ("DataStorage/Minimum", "1"),
        ("DataStorage/Maximum", "20"),
        ("ECPUPerSecond/Minimum", "1000"),
        ("ECPUPerSecond/Maximum", "9000"),
    ] {
        assert_eq!(
            settled.text(member),
            Some(expected),
            "the widened cache must report {member} {expected:?}, got {:?} ({})",
            settled.text(member),
            settled.body
        );
    }

    // Once the cache is gone the same contradictions are a not-found fault instead:
    // there is no envelope to refuse.
    let deleted = fixture
        .emulator
        .call(
            "DeleteServerlessCache",
            &params(&[("ServerlessCacheName", "live-cache")]),
        )
        .await;
    assert_eq!(
        deleted.status, 200,
        "DeleteServerlessCache must answer HTTP 200, got {} {}",
        deleted.status, deleted.body
    );
    let gone = poll_until(
        &fixture.emulator,
        REGION,
        "the deleted cache",
        "the cache leaving the store",
        "DescribeServerlessCaches",
        &params(&[("ServerlessCacheName", "live-cache")]),
        |reply| reply.status == 404,
    )
    .await;
    assert_not_found("the deleted cache", &gone);
    for (case, limits, _) in &rows {
        let refused = fixture.modify("live-cache", limits).await;
        assert_not_found(case, &refused);
    }
}
