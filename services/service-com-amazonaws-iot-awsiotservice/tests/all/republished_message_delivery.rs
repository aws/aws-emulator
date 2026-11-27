// SPDX-License-Identifier: CC0-1.0

//! What a `republish` topic-rule action does at the published data endpoint, driven over a
//! real MQTT connection to it: the message it puts back into the broker reaches the SESSIONS
//! subscribed to its topic, and the rules whose statements cover that topic as well.
//!
//! RULE: `Action$republish` is "publish to another MQTT topic", so the message its rule
//! selected becomes a NEW publication of the message broker on the action's own topic — it is
//! handed to every session subscribed to a filter covering that topic, at the QoS the action
//! names, and evaluated against the rules whose `FROM` clauses cover it, so a staging chain
//! reaches its second rule. A republish the action's role does not grant happens at all: the
//! action names "the ARN of the IAM role that grants access" and the call it has to grant is
//! the publish, so nothing is delivered for it.
//!
//! The rows differ along what the rule generalises over: the QoS the action republishes at,
//! the depth of the destination topic, a one-hop republish and a two-hop chain, and a role
//! that grants the publish beside one that does not.
//!
//! The device harness, the published address and the emulator assembly are the sibling
//! suite's (`tests/all/data_endpoint_mqtt_transport.rs`), shared rather than copied; the
//! `awsQuery` helpers are `tests/all/topic_rule_kinesis_action.rs`'s.
//!
//! Nothing here waits on a span: every publication is made at QoS 1 and answered by the
//! broker's own PUBACK, and each delivery is read off the subscriber's connection under the
//! sibling suite's generous packet deadline.

use serde_json::{json, Value};
use service_com_amazonaws_iot_awsiotservice::state::rules_engine::{
    IOT_PUBLISH_ACTION, SERVICE_PRINCIPAL,
};

use crate::data_endpoint_mqtt_transport::{emulator, Device, Emulator, Transport};
use crate::topic_rule_kinesis_action::{form_encode, xml_text};

/// The Region the sibling harness signs every request for, and so the Region every ARN below
/// is scoped to.
const REGION: &str = "us-east-1";
/// The account every emulated ARN carries (`emulator_server::EMULATOR_ACCOUNT_ID`).
const ACCOUNT_ID: &str = "000000000000";
/// The SQL version every rule is stored with (`TopicRulePayload$awsIotSqlVersion`).
const SQL_VERSION: &str = "2016-03-23";

/// The role every republish below is performed under: it trusts the rules engine's service
/// principal and grants the publish on every topic of this account.
const GRANTING_ROLE: &str = "iot-republish-granting";
/// The role that grants NOTHING, which is what a refused republish is performed under.
const BARRING_ROLE: &str = "iot-republish-barring";

/// The filter the far-end subscriber holds: every topic the rows republish onto.
const DESTINATION_FILTER: &str = "republished/#";

/// One republish row: the rule that performs it, and what the far-end subscriber is handed.
struct Row {
    /// What the case is called in an assertion.
    case: &'static str,
    /// The rule's name (`RuleName`, `pattern ^[a-zA-Z0-9_]+$`).
    rule: &'static str,
    /// The topic the device publishes on, which the rule's `FROM` clause covers.
    source: &'static str,
    /// The action's `topic` — where the message is republished.
    destination: &'static str,
    /// The action's `qos`, which is the QoS the delivery is made at.
    qos: u8,
    /// Whether the action's role grants the publish.
    granted: bool,
}

/// The rows: the two QoS levels a republish action names, destination topics of two depths, a
/// role that grants the publish and one that does not.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a republish at QoS 1 onto a two-level topic",
            rule: "republish_at_qos1",
            source: "staging/qos1",
            destination: "republished/qos1",
            qos: 1,
            granted: true,
        },
        Row {
            case: "a republish at QoS 0 onto a deeper topic",
            rule: "republish_at_qos0",
            source: "staging/qos0",
            destination: "republished/deep/levels/leaf",
            qos: 0,
            granted: true,
        },
        Row {
            case: "a republish whose role grants no publish",
            rule: "republish_barred",
            source: "staging/barred",
            destination: "republished/barred",
            qos: 1,
            granted: false,
        },
    ]
}

/// The two rules of the CHAIN: the first republishes onto a topic the second is written
/// against, and the second republishes onto the topic the subscriber holds.
const CHAIN_FIRST_RULE: &str = "republish_chain_first";
/// See [`CHAIN_FIRST_RULE`].
const CHAIN_SECOND_RULE: &str = "republish_chain_second";
/// The topic the chain's device publishes on.
const CHAIN_SOURCE: &str = "staging/chain";
/// The topic the first rule republishes onto, which the second rule's `FROM` clause covers.
const CHAIN_MIDDLE: &str = "staging/chain/normalized";
/// The topic the second rule republishes onto, which the far-end subscriber holds.
const CHAIN_DESTINATION: &str = "republished/chain";

/// The message every row publishes.
const MESSAGE: &str = r#"{"marker":"republished","temp":42}"#;

/// One IAM call over the `awsQuery` protocol that must be served, answered with its XML body.
async fn iam(emulator: &Emulator, action: &str, params: &[(&str, &str)]) -> String {
    let mut body = format!("Action={action}&Version=2010-05-08");
    for (name, value) in params {
        body.push('&');
        body.push_str(&form_encode(name));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let (status, rendered) = emulator
        .send(
            "POST",
            "/",
            "iam",
            "application/x-www-form-urlencoded",
            &[],
            body.into_bytes(),
        )
        .await;
    assert_eq!(
        status, 200,
        "setup: IAM {action} answered {status} with {rendered}"
    );
    rendered
}

/// Create `role_name` trusting the rules engine's service principal, with an inline policy
/// granting the publish on every topic of this account when `grants_publish`, and answer the
/// ARN IAM persisted.
async fn given_role(emulator: &Emulator, role_name: &str, grants_publish: bool) -> String {
    let trust = json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": { "Service": SERVICE_PRINCIPAL },
            "Action": "sts:AssumeRole",
        }],
    })
    .to_string();
    let created = iam(
        emulator,
        "CreateRole",
        &[("RoleName", role_name), ("AssumeRolePolicyDocument", &trust)],
    )
    .await;
    let arn = xml_text(&created, "Arn")
        .unwrap_or_else(|| panic!("setup: CreateRole({role_name}) reported no Arn: {created}"));
    if grants_publish {
        let document = json!({
            "Version": "2012-10-17",
            "Statement": [{
                "Effect": "Allow",
                "Action": IOT_PUBLISH_ACTION,
                "Resource": format!("arn:aws:iot:{REGION}:{ACCOUNT_ID}:topic/*"),
            }],
        })
        .to_string();
        iam(
            emulator,
            "PutRolePolicy",
            &[
                ("RoleName", role_name),
                ("PolicyName", "republish"),
                ("PolicyDocument", &document),
            ],
        )
        .await;
    }
    arn
}

/// Store one topic rule whose single action republishes what its statement selected.
async fn given_republish_rule(
    emulator: &Emulator,
    rule: &str,
    source: &str,
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
                "sql": format!("SELECT * FROM '{source}'"),
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

/// A republished message is a publication OF the broker: the far-end subscriber is handed it
/// on the action's own topic, at the QoS the action names — and a chain of two republishing
/// rules reaches its end, while a republish the role bars delivers nothing.
#[tokio::test]
async fn a_republished_message_reaches_the_sessions_subscribed_to_its_topic() {
    let emulator = emulator();
    let granting = given_role(&emulator, GRANTING_ROLE, true).await;
    let barring = given_role(&emulator, BARRING_ROLE, false).await;

    let rows = rows();
    for row in &rows {
        let role = if row.granted { &granting } else { &barring };
        given_republish_rule(
            &emulator,
            row.rule,
            row.source,
            row.destination,
            row.qos,
            role,
        )
        .await;
    }
    // The chain: one rule normalises onto a topic the second is written against.
    given_republish_rule(
        &emulator,
        CHAIN_FIRST_RULE,
        CHAIN_SOURCE,
        CHAIN_MIDDLE,
        1,
        &granting,
    )
    .await;
    given_republish_rule(
        &emulator,
        CHAIN_SECOND_RULE,
        CHAIN_MIDDLE,
        CHAIN_DESTINATION,
        1,
        &granting,
    )
    .await;

    // The address the control plane publishes, which is the call that opens the broker's
    // socket, and the two sessions on it: one listening at the far end, one publishing.
    let address = emulator.discover().await;
    let mut subscriber = Device::connect(Transport::Clear, &address, "republish-subscriber").await;
    let granted = subscriber.subscribe(&[(DESTINATION_FILTER, 1)]).await;
    assert_eq!(
        granted,
        vec![1],
        "the far-end subscription must be granted QoS 1, got {granted:?}"
    );
    let mut publisher = Device::connect(Transport::Clear, &address, "republish-publisher").await;

    for row in &rows {
        // The PUBACK the broker answers this with is what says it has performed the rule's
        // actions: the publication is acknowledged after the engine has run.
        publisher
            .publish_at(row.source, MESSAGE.as_bytes(), 1, false)
            .await;
        if !row.granted {
            // Nothing is delivered for a republish the role bars, which is proved by a LATER
            // publication arriving instead: the next delivery is the chain's, below.
            continue;
        }
        let delivered = subscriber.next_delivery().await;
        assert_eq!(
            delivered.topic, row.destination,
            "case {:?}: expected the delivery on the action's own topic, got {delivered:?}",
            row.case
        );
        assert_eq!(
            String::from_utf8_lossy(&delivered.payload),
            MESSAGE,
            "case {:?}: expected the selected message, got {delivered:?}",
            row.case
        );
        assert_eq!(
            delivered.qos, row.qos,
            "case {:?}: expected the delivery at the action's QoS, got {delivered:?}",
            row.case
        );
        assert!(
            !delivered.retain,
            "case {:?}: a republished message is no retained replay, got {delivered:?}",
            row.case
        );
    }

    // The chain's own publication, made LAST: its arrival is also what says the barred row
    // above delivered nothing, since the broker writes one session's deliveries in the order
    // it took them.
    publisher
        .publish_at(CHAIN_SOURCE, MESSAGE.as_bytes(), 1, false)
        .await;
    let delivered = subscriber.next_delivery().await;
    assert_eq!(
        delivered.topic, CHAIN_DESTINATION,
        "case the two-hop chain: expected the delivery at the chain's end (and nothing for the \
         barred row), got {delivered:?}"
    );
    assert_eq!(
        String::from_utf8_lossy(&delivered.payload),
        MESSAGE,
        "case the two-hop chain: expected the selected message, got {delivered:?}"
    );

    // The IoT data plane reads the same broker: the destination topics hold no RETAINED
    // message, because a republish is a publication and not a retained one.
    let (status, rendered) = emulator
        .send(
            "GET",
            "/retainedMessage/republished%2Fqos1",
            "iotdata",
            "application/json",
            &[],
            Vec::new(),
        )
        .await;
    assert_eq!(
        status, 404,
        "case the retained store: a republished message must retain nothing, got {status} with \
         {rendered}"
    );
    let parsed: Value = serde_json::from_str(&rendered).unwrap_or(Value::Null);
    assert!(
        parsed.get("message").is_some(),
        "case the retained store: expected the modelled refusal, got {rendered}"
    );
}
