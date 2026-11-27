// SPDX-License-Identifier: CC0-1.0

//! How this service states the VALUE SET of `SourceAccessType` when a request names a source
//! access configuration `Type` the type does not admit.
//!
//! RULE: a value outside an enumerated type's set is refused with the modelled
//! `ValidationException` (HTTP 400) naming the rejected value, the member path — including
//! the 1-based position of the list element the value sits in — and the whole set that type
//! admits, listed in the order AWS states it in for that enum SHAPE, which is neither the
//! order the model declares the enum in nor ascending order. For `SourceAccessType` (the
//! fifteen values [`DECLARED_VALUES`] names, `smithy_query(query="shape",
//! shape="SourceAccessType")`) that order is [`STATED_SET`].
//!
//! The rows differ along what the rule generalises over: WHICH value is rejected (a word of
//! no enum, a declared value in the wrong case, a value of a sibling enum this one does not
//! declare, a prefix of a declared value, a declared value respelled with a hyphen), WHICH
//! POSITION of the `SourceAccessConfigurations` list carries it (the first, the second of
//! two, the last of three), and WHICH OPERATION the list arrived in — the order belongs to
//! the enum shape, so `CreateEventSourceMapping` and `UpdateEventSourceMapping` state it
//! alike. The admitted rows beside them are the control: every value the type DOES declare
//! is accepted, so the sentence answers a value outside the set and not the operation.
//!
//! Every row drives real HTTP requests through the production `MultiServiceRouter`, and the
//! refused rows are checked to have changed nothing: a refused create stores no mapping, and
//! a refused update leaves the configurations the mapping already held.

use std::sync::Arc;

use serde_json::{json, Value};

use crate::event_source_mappings::{
    call, create_function, create_mapping, create_request, get_mapping, list_mappings,
    refusal_message, router_on, Answer, MAPPINGS_URI, MSK_SOURCE, STATE_DISABLED,
};

/// The modelled error a value outside an enumerated type's set is refused with.
const VALIDATION_EXCEPTION: &str = "ValidationException";

/// Every value `SourceAccessType` declares, in the model's own declaration order
/// (`smithy_query(query="shape", shape="SourceAccessType")`). The control rows send them
/// all, so a value this emulator refuses that the model admits fails here.
const DECLARED_VALUES: &[&str] = &[
    "BASIC_AUTH",
    "VPC_SUBNET",
    "VPC_SECURITY_GROUP",
    "SASL_SCRAM_512_AUTH",
    "SASL_SCRAM_256_AUTH",
    "VIRTUAL_HOST",
    "CLIENT_CERTIFICATE_TLS_AUTH",
    "SERVER_ROOT_CA_CERTIFICATE",
    "OAUTHBEARER_AUTH",
    "OAUTHBEARER_SCOPE",
    "OAUTHBEARER_AUDIENCE",
    "OAUTHBEARER_LOGICAL_CLUSTER",
    "OAUTHBEARER_IDENTITY_POOL",
    "IAM_AUTH",
    "IAM_OAUTHBEARER_AUTH",
];

/// The order AWS lists `SourceAccessType`'s value set in, as the answer the corpus carries
/// for a `Type` outside the set states it — the order `src/service.rs`'s
/// `REPORTED_ENUM_ORDERS` holds for this enum.
const STATED_SET: &str = "[CLIENT_CERTIFICATE_TLS_AUTH, OAUTHBEARER_AUDIENCE, \
                          OAUTHBEARER_SCOPE, OAUTHBEARER_AUTH, IAM_AUTH, VPC_SUBNET, \
                          OAUTHBEARER_IDENTITY_POOL, VPC_SECURITY_GROUP, \
                          SERVER_ROOT_CA_CERTIFICATE, IAM_OAUTHBEARER_AUTH, BASIC_AUTH, \
                          SASL_SCRAM_512_AUTH, VIRTUAL_HOST, OAUTHBEARER_LOGICAL_CLUSTER, \
                          SASL_SCRAM_256_AUTH]";

/// The topic every mapping of this suite consumes.
const TOPIC: &str = "order-events";

/// The secret an access configuration points at: "The value for your chosen configuration in
/// `Type`" (`smithy_query(query="shape", shape="SourceAccessConfiguration")`), within the
/// member's own `@length(1, 200)` and pattern.
const SECRET_URI: &str = "arn:aws:secretsmanager:us-east-1:000000000000:secret:kafka-auth";

/// The member a refusal names the list in, as the wire spells it in a request.
const REQUEST_MEMBER: &str = "SourceAccessConfigurations";

/// The member a refusal names the list in, in the lowerCamel spelling a clause states.
const REPORTED_MEMBER: &str = "sourceAccessConfigurations";

/// One row: the `Type` of each access configuration a request carries, and which of them —
/// by its 1-based position — the enum does not admit.
struct Row {
    case: &'static str,
    types: &'static [&'static str],
    /// The 1-based position of the element the clause must name, or `None` when every value
    /// the row carries is one the enum declares.
    refused_at: Option<usize>,
}

/// The catalog. Each rejected value is a different way of being outside the set, at a
/// different position of the list; the admitted rows carry only values the model declares.
///
/// An admitted row is a control for THIS rule — which values the enum admits — and not a
/// claim about which of them an event source of a given kind uses: the documentation scopes
/// each value to a source (`VPC_SUBNET` to self-managed Apache Kafka, `VIRTUAL_HOST` to
/// RabbitMQ, and so on), and nothing in this emulator ties a `Type` to the source a mapping
/// names, so the rows send them on one source and read back what was stored.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a word of no enum, in the only configuration",
            types: &["NOT_A_TYPE"],
            refused_at: Some(1),
        },
        Row {
            case: "a declared value in the wrong case",
            types: &["virtual_host"],
            refused_at: Some(1),
        },
        Row {
            case: "the second of two configurations, a declared value respelled with hyphens",
            types: &["BASIC_AUTH", "IAM-AUTH"],
            refused_at: Some(2),
        },
        Row {
            case: "the last of three configurations, a prefix of a declared value",
            types: &["VPC_SUBNET", "VPC_SECURITY_GROUP", "OAUTHBEARER"],
            refused_at: Some(3),
        },
        Row {
            case: "a value of the sibling starting-position enum this one does not declare",
            types: &["AT_TIMESTAMP"],
            refused_at: Some(1),
        },
        Row {
            case: "every value the enum declares",
            types: DECLARED_VALUES,
            refused_at: None,
        },
        Row {
            case: "one declared value, in the only configuration",
            types: &["IAM_AUTH"],
            refused_at: None,
        },
    ]
}

impl Row {
    /// The whole refusal this row must answer: the envelope, the rejected value as the
    /// request sent it, the member path down to the element's own `type`, and the set in the
    /// order AWS states it.
    fn refusal(&self) -> Option<String> {
        let position = self.refused_at?;
        let value = self.types.get(position - 1)?;
        Some(format!(
            "1 validation error detected: Value '{value}' at \
             '{REPORTED_MEMBER}.{position}.member.type' failed to satisfy constraint: Member \
             must satisfy enum value set: {STATED_SET}"
        ))
    }
}

/// The access configurations a row sends: one per `Type`, each pointing at the same secret.
fn access_configurations(types: &[&str]) -> Value {
    Value::Array(
        types
            .iter()
            .map(|r#type| json!({ "Type": r#type, "URI": SECRET_URI }))
            .collect(),
    )
}

/// The `Type` of each configuration a mapping reports, in the order it reports them.
fn reported_types(mapping: &Value) -> Vec<String> {
    mapping
        .get(REQUEST_MEMBER)
        .and_then(Value::as_array)
        .map(|configurations| {
            configurations
                .iter()
                .map(|configuration| {
                    configuration
                        .get("Type")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string()
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Assert `answer` is the refusal `expected` states, with the modelled status and code.
fn assert_refused(case: &str, answer: &Answer, expected: &str) {
    assert_eq!(
        answer.status,
        400,
        "case {case:?}: expected HTTP 400, got {} {}",
        answer.status,
        answer.text()
    );
    let code = answer.error_code();
    assert_eq!(
        code, VALIDATION_EXCEPTION,
        "case {case:?}: expected {VALIDATION_EXCEPTION}, got {code:?}"
    );
    let message = refusal_message(answer).unwrap_or_default();
    assert_eq!(
        message, expected,
        "case {case:?}: expected {expected:?}, got {message:?}"
    );
}

#[tokio::test]
async fn a_source_access_type_outside_the_set_is_refused_with_awss_own_ordering() {
    let router = router_on(&Arc::new(engine::InMemoryEngine::new()));

    for (index, row) in rows().iter().enumerate() {
        let case = row.case;
        let function = format!("source-access-type-{index}");
        create_function(&router, &function).await;
        let request = create_request(
            &function,
            Some(MSK_SOURCE),
            &json!({
                "Topics": [TOPIC],
                "Enabled": false,
                REQUEST_MEMBER: access_configurations(row.types),
            }),
        );
        let answer = create_mapping(&router, &request).await;
        let listed = list_mappings(&router, &format!("FunctionName={function}")).await;
        let held = listed.mappings().len();

        let Some(expected) = row.refusal() else {
            assert_eq!(
                answer.status,
                202,
                "case {case:?}: every value the enum declares must be accepted, got {} {}",
                answer.status,
                answer.text()
            );
            assert_eq!(
                held, 1,
                "case {case:?}: an accepted create must store one mapping, got {held} \
                 mapping(s) in {}",
                listed.text()
            );
            let stored = reported_types(&listed.mappings()[0]);
            assert_eq!(
                stored,
                row.types.to_vec(),
                "case {case:?}: expected the stored mapping to report {:?}, got {stored:?}",
                row.types
            );
            continue;
        };

        assert_refused(case, &answer, &expected);
        assert_eq!(
            held, 0,
            "case {case:?}: a refused create must store no mapping, got {held} mapping(s) \
             in {}",
            listed.text()
        );
    }
}

#[tokio::test]
async fn an_update_states_the_same_set_in_the_same_order_and_changes_nothing() {
    let router = router_on(&Arc::new(engine::InMemoryEngine::new()));
    let function = "source-access-type-update";
    create_function(&router, function).await;

    // A mapping asked for with polling paused is answered in the terminal state it asked
    // for, so an update of it is not inside the create window.
    let held = &["SASL_SCRAM_512_AUTH", "CLIENT_CERTIFICATE_TLS_AUTH"];
    let created = create_mapping(
        &router,
        &create_request(
            function,
            Some(MSK_SOURCE),
            &json!({
                "Topics": [TOPIC],
                "Enabled": false,
                REQUEST_MEMBER: access_configurations(held),
            }),
        ),
    )
    .await;
    assert_eq!(
        created.status,
        202,
        "the mapping under test must be created, got {} {}",
        created.status,
        created.text()
    );
    assert_eq!(
        created.string("State"),
        Some(STATE_DISABLED.to_string()),
        "a mapping created with polling paused must answer {STATE_DISABLED}, got {}",
        created.text()
    );
    let uuid = created
        .string("UUID")
        .unwrap_or_else(|| panic!("the create answers a UUID: {}", created.text()));

    // The same clause, from the other operation that carries the list: the second element's
    // own `type`, and the set in the one order this service states for the shape.
    let case = "an update whose second configuration is outside the set";
    let refused = call(
        &router,
        "PUT",
        &format!("{MAPPINGS_URI}/{uuid}"),
        &json!({ REQUEST_MEMBER: access_configurations(&["IAM_AUTH", "SASL_SCRAM_128_AUTH"]) }),
    )
    .await;
    let expected = format!(
        "1 validation error detected: Value 'SASL_SCRAM_128_AUTH' at \
         '{REPORTED_MEMBER}.2.member.type' failed to satisfy constraint: Member must satisfy \
         enum value set: {STATED_SET}"
    );
    assert_refused(case, &refused, &expected);

    let reading = get_mapping(&router, &uuid).await;
    let stored = reported_types(&reading.json());
    assert_eq!(
        stored,
        held.to_vec(),
        "case {case:?}: a refused update must leave the stored configurations {held:?}, got \
         {stored:?} in {}",
        reading.text()
    );

    // The control: the same member, every value declared, is accepted and read back.
    let case = "an update whose configurations are all declared values";
    let accepted = call(
        &router,
        "PUT",
        &format!("{MAPPINGS_URI}/{uuid}"),
        &json!({ REQUEST_MEMBER: access_configurations(DECLARED_VALUES) }),
    )
    .await;
    assert_eq!(
        accepted.status,
        202,
        "case {case:?}: expected the modelled 202, got {} {}",
        accepted.status,
        accepted.text()
    );
    let reading = get_mapping(&router, &uuid).await;
    let stored = reported_types(&reading.json());
    assert_eq!(
        stored,
        DECLARED_VALUES.to_vec(),
        "case {case:?}: expected the stored mapping to report {DECLARED_VALUES:?}, got \
         {stored:?} in {}",
        reading.text()
    );
}
