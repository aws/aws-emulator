// SPDX-License-Identifier: CC0-1.0

//! WHAT a `republish` topic-rule action puts on the destination topic, read off a native MQTT
//! client's own connection to the published data endpoint.
//!
//! RULE: a rule's `actions` act on the message its `sql` SELECTED, so the message a
//! `republish` action puts back into the broker on its own `topic` is the statement's
//! evaluated projection — the attributes it names, plus the value of every function it
//! computes under that function's alias, where `topic()` is "the topic to which the message
//! was sent" and `topic(n)` its n-th level, both read off the topic the message was PUBLISHED
//! on and not the one it is republished to — and a statement whose condition the message does
//! not satisfy selects it for no action at all. Every session subscribed to a filter covering
//! the action's topic is handed that projection there, at the action's `qos`.
//!
//! The sibling suite pins WHERE a republished message is delivered and under which role
//! (`tests/all/republished_message_delivery.rs`, whose every rule selects the whole message);
//! this one pins what the delivered bytes ARE, which is the half a `SELECT *` cannot see.
//!
//! The rows differ along what the rule generalises over: the statement's projection (the whole
//! message, the whole message with a computed member added, a projection of computed members
//! beside an attribute, and one attribute alone), how many levels the published topic carries
//! (so the whole topic a `topic()` reports is a value no single level could be), the QoS the
//! action republishes at, and whether the statement's condition holds at all.
//!
//! The device harness, the published address and the emulator assembly are
//! `tests/all/data_endpoint_mqtt_transport.rs`'s, and the role a republish runs under is
//! `tests/all/republished_message_delivery.rs`'s — shared rather than copied. Both halves of
//! the one published address are driven, since a device of AWS IoT Core dials the encrypted
//! one.
//!
//! Nothing here waits on a span: every publication is made at QoS 1 and answered by the
//! broker's own PUBACK, and each leg ends with a SENTINEL publication whose own republish is
//! known to arrive — so the deliveries a row expects are read as "everything handed to the
//! subscriber before the sentinel", and a row expecting none is judged by the sentinel's
//! arrival rather than by a clock.

use serde_json::{json, Value};

use crate::data_endpoint_mqtt_transport::{emulator, Delivered, Device, Emulator, TRANSPORTS};
use crate::republished_message_delivery::given_role;

/// The SQL version every rule is stored with (`TopicRulePayload$awsIotSqlVersion`).
const SQL_VERSION: &str = "2016-03-23";

/// The role every republish below is performed under: it trusts the rules engine's service
/// principal and grants the publish on every topic of this account, so what a row observes is
/// its statement's doing and never the role's.
const ROLE: &str = "iot-republish-projection";

/// The filter the far-end subscriber holds: every topic the rows republish onto, and no topic
/// any row publishes on — so a delivery it is handed is a republished message and never the
/// original publication.
const DESTINATION_FILTER: &str = "restaged/#";

/// The message every row publishes: a document carrying one text attribute and one numeric
/// one, so a projection that names an attribute and a condition that reads one both have
/// something to read.
const MESSAGE: &str = r#"{"marker":"m-1","temp":42}"#;

/// The SENTINEL rule, the topic it is written against and the topic it republishes onto: a
/// publication made after a row's own, whose republish is known to arrive, so the row's
/// deliveries are everything handed to the subscriber before it.
const SENTINEL_RULE: &str = "projection_sentinel";
/// See [`SENTINEL_RULE`].
const SENTINEL_SOURCE: &str = "projected/sentinel";
/// See [`SENTINEL_RULE`].
const SENTINEL_DESTINATION: &str = "restaged/sentinel";

/// One row: the statement a rule carries, and the bytes its `republish` action must put on the
/// destination topic for [`MESSAGE`].
struct Row {
    /// What the case is called in an assertion.
    case: &'static str,
    /// The rule's name (`RuleName`, `pattern ^[a-zA-Z0-9_]+$`).
    rule: &'static str,
    /// The topic the device publishes on, which the statement's `FROM` clause covers.
    source: &'static str,
    /// What the statement SELECTS, between `SELECT` and `FROM`.
    select: &'static str,
    /// The statement's `WHERE` clause, for a row that carries one.
    condition: Option<&'static str>,
    /// The action's `topic` — where the selected message is republished.
    destination: &'static str,
    /// The action's `qos`, which is the QoS the delivery is made at.
    qos: u8,
    /// The document the subscriber must be handed on [`Row::destination`], and `None` for a
    /// statement that selects the message for no action.
    republished: Option<Value>,
}

impl Row {
    /// The statement the rule is stored with: the projection over the row's own source topic,
    /// under its condition where it has one.
    fn sql(&self) -> String {
        match self.condition {
            Some(condition) => format!(
                "SELECT {} FROM '{}' WHERE {condition}",
                self.select, self.source
            ),
            None => format!("SELECT {} FROM '{}'", self.select, self.source),
        }
    }
}

/// The rows: the projection forms a statement takes, over source topics of two depths, at both
/// QoS levels a republish action names, with and without a condition that holds.
fn rows() -> Vec<Row> {
    // The source topics, named once so each row's expected `topic()` value is the very topic
    // its publication is made on.
    let telemetry = "projected/telemetry";
    let deep = "projected/deep/levels/leaf";
    let segments = "projected/segments/leaf";
    vec![
        Row {
            case: "the whole message with the published topic added under an alias",
            rule: "projection_whole_plus_topic",
            source: telemetry,
            select: "*, topic() AS src",
            condition: None,
            destination: "restaged/telemetry",
            qos: 1,
            republished: Some(json!({ "marker": "m-1", "temp": 42, "src": telemetry })),
        },
        Row {
            case: "the published topic of a four-level source topic is the whole topic",
            rule: "projection_deep_topic",
            source: deep,
            select: "*, topic() AS src",
            condition: None,
            destination: "restaged/deep",
            qos: 1,
            republished: Some(json!({ "marker": "m-1", "temp": 42, "src": deep })),
        },
        Row {
            // `topic(Decimal)` numbers the levels from 1 (the AWS IoT SQL reference's
            // "topic(Decimal)": "the topic to which the message was sent"), so `topic(1)` is
            // the first level and `topic(3)` the third.
            case: "two numbered levels of the published topic beside one attribute",
            rule: "projection_topic_levels",
            source: segments,
            select: "topic(1) AS head, topic(3) AS leaf, marker",
            condition: None,
            destination: "restaged/segments",
            qos: 0,
            republished: Some(json!({ "head": "projected", "leaf": "leaf", "marker": "m-1" })),
        },
        Row {
            case: "one attribute alone",
            rule: "projection_one_attribute",
            source: "projected/narrow",
            select: "marker",
            condition: None,
            destination: "restaged/narrow",
            qos: 1,
            republished: Some(json!({ "marker": "m-1" })),
        },
        Row {
            case: "the whole message and nothing computed",
            rule: "projection_whole",
            source: "projected/whole",
            select: "*",
            condition: None,
            destination: "restaged/whole",
            qos: 1,
            republished: Some(json!({ "marker": "m-1", "temp": 42 })),
        },
        Row {
            case: "a statement whose condition the message does not satisfy",
            rule: "projection_filtered",
            source: "projected/filtered",
            select: "*, topic() AS src",
            condition: Some("temp > 100"),
            destination: "restaged/filtered",
            qos: 1,
            republished: None,
        },
    ]
}

/// Store one topic rule whose single action republishes what `sql` selected onto `destination`
/// at `qos`, as `CreateTopicRule` takes it: the body IS the `topicRulePayload`.
async fn given_republish_rule(
    emulator: &Emulator,
    rule: &str,
    sql: &str,
    destination: &str,
    qos: u8,
    role_arn: &str,
) {
    let (status, rendered) = emulator
        .send(
            "POST",
            &format!("/rules/{rule}"),
            "iot",
            "application/json",
            &[],
            json!({
                "sql": sql,
                "awsIotSqlVersion": SQL_VERSION,
                "ruleDisabled": false,
                "actions": [{
                    "republish": { "roleArn": role_arn, "topic": destination, "qos": qos },
                }],
            })
            .to_string()
            .into_bytes(),
        )
        .await;
    assert_eq!(
        status, 200,
        "setup: CreateTopicRule({rule}) answered {status} with {rendered}"
    );
}

/// Everything `subscriber` is handed before the SENTINEL's own republished message arrives on
/// [`SENTINEL_DESTINATION`] — the condition that says the earlier publications have been
/// evaluated and delivered, since the broker writes one session's deliveries in the order it
/// took them.
async fn deliveries_before_the_sentinel(
    subscriber: &mut Device,
    leg: &str,
    sentinel: &Value,
) -> Vec<Delivered> {
    let mut seen = Vec::new();
    loop {
        let delivered = subscriber.next_delivery().await;
        if delivered.topic == SENTINEL_DESTINATION {
            let body: Value = serde_json::from_slice(&delivered.payload).unwrap_or(Value::Null);
            assert_eq!(
                &body, sentinel,
                "leg {leg:?}: expected this leg's own sentinel on {SENTINEL_DESTINATION}, got \
                 {delivered:?}"
            );
            return seen;
        }
        seen.push(delivered);
    }
}

/// A `republish` action re-emits the message its rule's STATEMENT selected: the document the
/// far-end subscriber is handed on the action's topic carries exactly the members the
/// projection names — the computed `topic()` among them, valued at the topic the message was
/// published on — at the action's QoS, and nothing is republished for a message the statement's
/// condition does not select.
#[tokio::test]
async fn a_republish_action_re_emits_the_statements_projection_to_a_far_end_subscriber() {
    let emulator = emulator();
    let role_arn = given_role(&emulator, ROLE, true).await;

    let rows = rows();
    for row in &rows {
        given_republish_rule(
            &emulator,
            row.rule,
            &row.sql(),
            row.destination,
            row.qos,
            &role_arn,
        )
        .await;
    }
    given_republish_rule(
        &emulator,
        SENTINEL_RULE,
        &format!("SELECT * FROM '{SENTINEL_SOURCE}'"),
        SENTINEL_DESTINATION,
        1,
        &role_arn,
    )
    .await;

    // The address the control plane publishes, which is the call that opens the broker's
    // socket. One address, two halves — a device dials either.
    let address = emulator.discover().await;

    for transport in TRANSPORTS {
        let leg = transport.label();
        let mut subscriber = Device::connect(*transport, &address, "projection-subscriber").await;
        let granted = subscriber.subscribe(&[(DESTINATION_FILTER, 1)]).await;
        assert_eq!(
            granted,
            vec![1],
            "leg {leg:?}: the far-end subscription must be granted QoS 1, got {granted:?}"
        );
        let mut publisher = Device::connect(*transport, &address, "projection-publisher").await;

        // Every row's publication, then the sentinel's: the PUBACK of each says the broker has
        // run the rules for it.
        for row in &rows {
            publisher
                .publish_at(row.source, MESSAGE.as_bytes(), 1, false)
                .await;
        }
        let sentinel = json!({ "marker": "sentinel", "leg": leg });
        publisher
            .publish_at(SENTINEL_SOURCE, sentinel.to_string().as_bytes(), 1, false)
            .await;

        let delivered = deliveries_before_the_sentinel(&mut subscriber, leg, &sentinel).await;
        let expected: Vec<&Row> = rows.iter().filter(|row| row.republished.is_some()).collect();
        assert_eq!(
            delivered.len(),
            expected.len(),
            "leg {leg:?}: expected {} republished delivery(ies) and no other, got {delivered:?}",
            expected.len()
        );
        for (row, actual) in expected.iter().zip(delivered.iter()) {
            let case = format!("{} {leg}", row.case);
            assert_eq!(
                actual.topic, row.destination,
                "case {case:?}: expected the delivery on the action's own topic, got {actual:?}"
            );
            let body: Value = serde_json::from_slice(&actual.payload).unwrap_or(Value::Null);
            assert_eq!(
                Some(&body),
                row.republished.as_ref(),
                "case {case:?}: expected the statement's projection, got {body}"
            );
            assert_eq!(
                actual.qos, row.qos,
                "case {case:?}: expected the delivery at the action's QoS, got {actual:?}"
            );
            assert!(
                !actual.retain,
                "case {case:?}: a republished message is no retained replay, got {actual:?}"
            );
        }
    }
}
