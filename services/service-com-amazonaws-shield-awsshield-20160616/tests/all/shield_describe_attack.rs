// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `DescribeAttack`.
//!
//! Every case goes over the wire — an `awsJson1_1` `POST /` with
//! `X-Amz-Target: AWSShield_20160616.DescribeAttack` through the same `MultiServiceRouter`
//! the production router serves — so routing, deserialization, modelled-constraint
//! validation, the handler and serialization are all exercised.
//!
//! The rules under test, grounded in this operation's own Smithy documentation:
//!
//! * the operation "describes the details of a DDoS attack", so its answer is the one
//!   attack of the account's attack log that carries the `AttackId` the call names,
//!   reported with that attack's own identifier, attacked resource and instants
//!   (`AttackDetail`), an attack still under way carrying no `EndTime` ("the time the
//!   attack ended");
//! * `AttackCounters` is a "list of counters that describe the attack", so it carries one
//!   counter per unit the stored attack was observed in, reporting the peak the log holds
//!   for that unit, and nothing for a unit the attack was never observed in;
//! * detail the log holds nothing for — `SubResources` ("if applicable, additional detail
//!   about the resource being attacked"), `AttackProperties` and `Mitigations` — is not
//!   reported at all rather than invented;
//! * `Attack` is "the attack that you requested" and is optional, so an `AttackId` no
//!   attack of the account carries is answered with no attack detail at all — the call is
//!   served rather than refused, and nothing is invented for an attack nothing detected;
//! * the operation reports, so repeating it answers the same thing and leaves the
//!   account's attack log as it was.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_shield_awsshield_20160616::state::{service_state, AttackRecord};
use service_com_amazonaws_shield_awsshield_20160616::EmulatedService;

/// The `X-Amz-Target` prefix from this crate's `OPERATIONS.md` wire-protocol section.
const TARGET_PREFIX: &str = "AWSShield_20160616.";

/// One hour, the grid the seeded attack log sits on.
const HOUR: Duration = Duration::from_secs(3_600);

/// The greatest length `AttackId` admits (`AttackId`, len 1..128), so the longest
/// identifier a call may carry is exercised as well as the shortest.
const MAX_ATTACK_ID_LENGTH: usize = 128;

/// One answer from the emulator: the HTTP status and the parsed JSON body.
#[derive(Debug)]
struct Reply {
    status: u16,
    body: Value,
}

impl Reply {
    /// Assert this reply is a success, and hand back the `Attack` detail it carries.
    fn expect_attack(&self, case: &str) -> &Value {
        assert_eq!(
            self.status, 200,
            "case {case}: expected HTTP 200, got {self:?}"
        );
        self.body
            .get("Attack")
            .unwrap_or_else(|| panic!("case {case}: no Attack in {:?}", self.body))
    }
}

/// The emulator under test: one shared engine, one registered service, requests
/// dispatched through the production router.
struct Emulator {
    router: smithy_server_runtime::MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = smithy_server_runtime::MultiServiceRouter::new();
        // Signature verification is the emulator's concern, not this service's; the
        // routing key still comes from the `Authorization` header below.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        Self { router, engine }
    }

    async fn call(&self, operation: &str, body: &Value) -> Reply {
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("x-amz-target", format!("{TARGET_PREFIX}{operation}"))
            .header("content-type", "application/x-amz-json-1.1")
            .header(
                "authorization",
                "AWS4-HMAC-SHA256 Credential=AKIDEMULATOR/20260101/us-east-1/shield/aws4_request, \
                 SignedHeaders=host;x-amz-target, Signature=0123456789abcdef",
            )
            .body(bytes::Bytes::from(body.to_string()))
            .expect("a well-formed test request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let bytes = response.into_body();
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or_else(|e| {
                panic!(
                    "{operation}: response body is not JSON ({e}): {}",
                    String::from_utf8_lossy(&bytes)
                )
            })
        };
        Reply { status, body }
    }

    /// Record one attack Shield detected against the account. Reaches the store directly
    /// because Shield detects attacks rather than being asked to create them: no
    /// modelled operation writes the attack log.
    fn detect_attack(&self, seed: &Seed) {
        let state = service_state(&*self.engine).expect("the shield store");
        let mut guard = state.lock();
        guard.attacks.push(AttackRecord {
            attack_id: seed.attack_id.clone(),
            resource_arn: resource_arn(&seed.resource),
            start_time: base_instant() + HOUR * seed.started_hour,
            end_time: seed.ended_hour.map(|hour| base_instant() + HOUR * hour),
            max_bits_per_second: seed.bits_per_second,
            max_packets_per_second: seed.packets_per_second,
            max_requests_per_second: seed.requests_per_second,
            attack_vectors: seed.vectors.iter().map(|v| (*v).to_string()).collect(),
        });
    }

    /// How many attacks the account's log holds, so a reporting call can be shown to
    /// leave it alone.
    fn attacks_held(&self) -> usize {
        let state = service_state(&*self.engine).expect("the shield store");
        let guard = state.lock();
        guard.attacks.len()
    }
}

/// The instant the seeded attack log is measured from: a fixed point, because this
/// describe's answer depends on the request and the stored log, never on the clock.
fn base_instant() -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(1_767_225_600)
}

/// An instant as the epoch seconds the `awsJson1_1` wire form carries.
fn epoch_seconds(instant: SystemTime) -> f64 {
    instant
        .duration_since(SystemTime::UNIX_EPOCH)
        .expect("an instant after the epoch")
        .as_secs_f64()
}

/// An hour offset from [`base_instant`] as its wire timestamp.
fn hour_timestamp(hour: u32) -> f64 {
    epoch_seconds(base_instant() + HOUR * hour)
}

/// The ARN of one of the account's attacked resources.
fn resource_arn(resource: &str) -> String {
    format!("arn:aws:cloudfront::000000000000:distribution/{resource}")
}

/// The longest `AttackId` the model admits, built from characters its pattern
/// (`^[a-zA-Z0-9\-]*$`) allows.
fn longest_attack_id() -> String {
    let mut id = "Attack-".to_string();
    id.extend(std::iter::repeat_n('9', MAX_ATTACK_ID_LENGTH - id.len()));
    id
}

/// One attack to record before the call.
#[derive(Debug, Clone)]
struct Seed {
    attack_id: String,
    /// The resource the attack was directed at, as [`resource_arn`] names it.
    resource: String,
    /// When the attack started, in hours after [`base_instant`].
    started_hour: u32,
    /// When the attack ended, in hours after [`base_instant`], or `None` for an attack
    /// still under way.
    ended_hour: Option<u32>,
    /// The peak bits per second observed, or `None` for an attack never observed in that
    /// unit.
    bits_per_second: Option<f64>,
    /// The peak packets per second observed, like [`Seed::bits_per_second`].
    packets_per_second: Option<f64>,
    /// The peak requests per second observed, like [`Seed::bits_per_second`].
    requests_per_second: Option<f64>,
    /// The `AttackVectorDescription$VectorType` values the attack was attributed to.
    vectors: &'static [&'static str],
}

impl Seed {
    /// The counters, as `(Name, Unit, Max)` triples, the detail of this attack must
    /// report: one per unit the attack was observed in, in the order this service's
    /// `AttackVolume` lists the units. The names and the units are this service's own
    /// (`AttackVolume$BitsPerSecond`/`PacketsPerSecond`/`RequestsPerSecond`, and the
    /// `Unit` values `BITS`, `BYTES`, `PACKETS`, `REQUESTS`).
    fn expected_counters(&self) -> Vec<(String, String, f64)> {
        [
            ("BitsPerSecond", "BITS", self.bits_per_second),
            ("PacketsPerSecond", "PACKETS", self.packets_per_second),
            ("RequestsPerSecond", "REQUESTS", self.requests_per_second),
        ]
        .into_iter()
        .filter_map(|(name, unit, peak)| {
            peak.map(|max| (name.to_string(), unit.to_string(), max))
        })
        .collect()
    }
}

/// The account's attack log every case below reads, in the order Shield detected the
/// attacks. The rows differ along everything the rule generalises over: the identifier's
/// form and length, the attacked resource, whether the attack has ended, and which units
/// — none, one, or several — the attack was observed in. The vector types are values
/// `AttackVectorDescription$VectorType` documents ("The attack type. Valid values:
/// UDP_TRAFFIC ... SYN_FLOOD ... REQUEST_FLOOD ...").
fn log() -> Vec<Seed> {
    vec![
        Seed {
            // A finished network-level attack, observed in both network units.
            attack_id: "AttackId-alpha-1".to_string(),
            resource: "alpha".to_string(),
            started_hour: 0,
            ended_hour: Some(1),
            bits_per_second: Some(4_200_000.0),
            packets_per_second: Some(17_500.5),
            requests_per_second: None,
            vectors: &["UDP_TRAFFIC"],
        },
        Seed {
            // Still under way, and an application-level attack: requests per second only.
            attack_id: "AttackId-beta-1".to_string(),
            resource: "beta".to_string(),
            started_hour: 2,
            ended_hour: None,
            bits_per_second: None,
            packets_per_second: None,
            requests_per_second: Some(912.0),
            vectors: &["REQUEST_FLOOD", "HTTP_REFLECTION"],
        },
        Seed {
            // The shortest identifier the model admits (`AttackId`, len 1..128), and an
            // attack attributed to no unit and no vector at all.
            attack_id: "a".to_string(),
            resource: "gamma".to_string(),
            started_hour: 4,
            ended_hour: Some(5),
            bits_per_second: None,
            packets_per_second: None,
            requests_per_second: None,
            vectors: &[],
        },
        Seed {
            // The longest identifier the model admits, and every unit observed at once.
            attack_id: longest_attack_id(),
            resource: "delta".to_string(),
            started_hour: 6,
            ended_hour: Some(9),
            bits_per_second: Some(1.0),
            packets_per_second: Some(2.0),
            requests_per_second: Some(3.0),
            vectors: &["SYN_FLOOD"],
        },
    ]
}

/// An emulator whose account has seen every attack of [`log`].
fn seeded() -> Emulator {
    let emulator = Emulator::new();
    for seed in &log() {
        emulator.detect_attack(seed);
    }
    emulator
}

/// The `(Name, Unit, Max)` triples a reported `AttackCounters` list carries. The
/// statistics the log does not hold stay at the modelled defaults, so they are not part
/// of the comparison; `Max` is.
fn reported_counters(case: &str, attack: &Value) -> Vec<(String, String, f64)> {
    attack
        .get("AttackCounters")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case}: no AttackCounters in {attack}"))
        .iter()
        .map(|counter| {
            let text = |member: &str| {
                counter
                    .get(member)
                    .and_then(Value::as_str)
                    .unwrap_or_else(|| panic!("case {case}: no {member} in {counter}"))
                    .to_string()
            };
            let max = counter
                .get("Max")
                .and_then(Value::as_f64)
                .unwrap_or_else(|| panic!("case {case}: no Max in {counter}"));
            (text("Name"), text("Unit"), max)
        })
        .collect()
}


#[tokio::test]
async fn describe_attack_reports_the_logged_attack_the_id_names() {
    let emulator = seeded();

    for seed in &log() {
        let case = seed.attack_id.clone();
        let reply = emulator
            .call("DescribeAttack", &json!({"AttackId": seed.attack_id}))
            .await;
        let attack = reply.expect_attack(&case);

        assert_eq!(
            attack.get("AttackId").and_then(Value::as_str),
            Some(seed.attack_id.as_str()),
            "case {case}: expected AttackId {}, got {attack}",
            seed.attack_id
        );
        assert_eq!(
            attack.get("ResourceArn").and_then(Value::as_str),
            Some(resource_arn(&seed.resource).as_str()),
            "case {case}: expected ResourceArn {}, got {attack}",
            resource_arn(&seed.resource)
        );
        assert_eq!(
            attack.get("StartTime").and_then(Value::as_f64),
            Some(hour_timestamp(seed.started_hour)),
            "case {case}: expected StartTime {}, got {attack}",
            hour_timestamp(seed.started_hour)
        );
        assert_eq!(
            attack.get("EndTime").and_then(Value::as_f64),
            seed.ended_hour.map(hour_timestamp),
            "case {case}: expected EndTime {:?}, got {attack}",
            seed.ended_hour.map(hour_timestamp)
        );
        assert_eq!(
            reported_counters(&case, attack),
            seed.expected_counters(),
            "case {case}: expected the counters {:?}, got {attack}",
            seed.expected_counters()
        );
    }
}

#[tokio::test]
async fn the_detail_reports_nothing_the_attack_log_does_not_hold() {
    // The log keeps no sub-resource breakdown, no event properties and no mitigation
    // actions, so a detail carries none of those members rather than an invented one.
    let emulator = seeded();

    for seed in &log() {
        let case = seed.attack_id.clone();
        let reply = emulator
            .call("DescribeAttack", &json!({"AttackId": seed.attack_id}))
            .await;
        let attack = reply.expect_attack(&case);
        for member in ["SubResources", "AttackProperties", "Mitigations"] {
            assert!(
                attack.get(member).is_none(),
                "case {case}: expected no {member} member, got {attack}"
            );
        }
    }
}

#[tokio::test]
async fn an_attack_id_the_account_holds_no_attack_for_describes_no_attack() {
    // Each row is an identifier the seeded log holds no attack for: one unrelated to any
    // stored attack, a stored identifier in another case, a stored identifier with a
    // suffix, the prefix of a stored one, and the shortest and longest identifiers the
    // model admits. Every row satisfies the member's modelled constraints, so the answer
    // is the handler's, not validation's.
    let stored = log()[0].attack_id.clone();
    let cases: Vec<(&str, String)> = vec![
        (
            "an identifier no attack carries",
            "AttackId-omega-9".to_string(),
        ),
        ("a stored identifier in another case", stored.to_uppercase()),
        ("a stored identifier with a suffix", format!("{stored}-2")),
        ("the prefix of a stored identifier", stored[..8].to_string()),
        ("the shortest identifier the model admits", "Z".to_string()),
        (
            "the longest identifier the model admits",
            longest_attack_id().replace('9', "0"),
        ),
    ];

    let emulator = seeded();
    for (name, attack_id) in cases {
        let reply = emulator
            .call("DescribeAttack", &json!({"AttackId": attack_id}))
            .await;
        assert_eq!(
            reply.status, 200,
            "case {name}: expected the call to be served with HTTP 200, got {reply:?}"
        );
        assert!(
            reply
                .body
                .get("Attack")
                .is_none_or(|attack| attack == &json!({})),
            "case {name}: expected no attack detail, got {:?}",
            reply.body
        );
    }
}

#[tokio::test]
async fn an_account_no_attack_has_hit_describes_no_attack() {
    // The same answer on an empty log: an account with no detected attacks and one with
    // several are the same code path.
    let emulator = Emulator::new();
    let attack_id = log()[0].attack_id.clone();
    let reply = emulator
        .call("DescribeAttack", &json!({"AttackId": attack_id}))
        .await;
    assert_eq!(
        reply.status, 200,
        "case empty log: expected the call to be served with HTTP 200, got {reply:?}"
    );
    assert!(
        reply
            .body
            .get("Attack")
            .is_none_or(|attack| attack == &json!({})),
        "case empty log: expected no attack detail, got {:?}",
        reply.body
    );
}

#[tokio::test]
async fn every_listed_attack_can_be_described() {
    // The detail and the listing read one log: every attack `ListAttacks` summarises is
    // describable, and the two agree on the attack's identity, resource and instants.
    let emulator = seeded();
    let listed = emulator.call("ListAttacks", &json!({})).await;
    let summaries = listed
        .body
        .get("AttackSummaries")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case listed then described: no AttackSummaries in {:?}", listed))
        .clone();
    assert_eq!(
        summaries.len(),
        log().len(),
        "case listed then described: expected every logged attack to be listed, got {summaries:?}"
    );

    for summary in &summaries {
        let attack_id = summary
            .get("AttackId")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("case listed then described: no AttackId in {summary}"))
            .to_string();
        let reply = emulator
            .call("DescribeAttack", &json!({"AttackId": attack_id}))
            .await;
        let attack = reply.expect_attack(&attack_id);
        for member in ["AttackId", "ResourceArn", "StartTime", "EndTime"] {
            assert_eq!(
                attack.get(member),
                summary.get(member),
                "case {attack_id}: expected the detail's {member} to match the summary's \
                 {:?}, got {attack}",
                summary.get(member)
            );
        }
    }
}

#[tokio::test]
async fn describing_an_attack_changes_nothing_a_later_caller_sees() {
    let emulator = seeded();
    let held = emulator.attacks_held();
    let request = json!({"AttackId": log()[0].attack_id});

    let first = emulator.call("DescribeAttack", &request).await;
    let first_body = first.expect_attack("repeated read").clone();
    let second = emulator.call("DescribeAttack", &request).await;
    let second_body = second.expect_attack("repeated read");

    assert_eq!(
        &first_body, second_body,
        "case repeated read: a reporting operation must answer the same thing twice, \
         got {first_body:?} then {second_body:?}"
    );
    assert_eq!(
        emulator.attacks_held(),
        held,
        "case repeated read: the attack log must be untouched, it held {held} records"
    );
}
