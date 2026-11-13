// SPDX-License-Identifier: CC0-1.0

//! What a CloudFormation stack deploys for a resource property written as a
//! `{{resolve:secretsmanager:…}}` DYNAMIC REFERENCE.
//!
//! The rule under test: **a resource property written as a dynamic reference is resolved by
//! CloudFormation against the store the reference names, and the service that OWNS the
//! resource is asked for it with the VALUE that store holds — for the `secretsmanager` store,
//! the secret's text, or the one key of the JSON document it holds that the reference names,
//! read as the resource is created, so a reference may name a secret the same deployment
//! creates.** A reference the store cannot satisfy has no value, so that resource fails
//! carrying the diagnostic and nothing is passed off as created.
//!
//! Three crates have to agree for that to hold — CloudFormation resolves the reference and
//! makes the call, Secrets Manager holds the secret, and the service that owns the resource
//! is asked with the resolved value — so the suite lives here rather than in any one of them.
//!
//! The rows differ along what the rule generalises over: how the reference spells the secret
//! (its NAME, or the full ARN a `Ref` to the secret the same stack declares resolves to,
//! which carries colons of its own), whether it names a `json-key` or resolves to the whole
//! secret text, whether the optional `secret-string` and `version-stage` fields are written
//! out or left empty, whether the key's value is written in the document as a string or as a
//! number, whether the secret's own value is the template's or one Secrets Manager generated,
//! and whether the reference stands alone or inside a longer property value.
//!
//! Every expected value is computed from what the secret really holds — read through Secrets
//! Manager's own `GetSecretValue` — rather than read back out of the answer being checked;
//! for a generated password that is the only possible source, since the value is that
//! service's to draw.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

// The awsQuery form encoder and the XML element readers this crate writes ONCE.
use super::query_wire::{elements, form_encode, opt_element, outputs_of};

/// The Region every request here is signed for, and so the Region the stacks, the secrets,
/// the parameters and the broker belong to.
const REGION: &str = "us-east-1";

/// How long one poll waits before the next, and how many are made: every transition this
/// suite waits for is owed on the emulator's own clock, so the suite waits for the CONDITION
/// and never for a span. The budget is deliberately far longer than the spans themselves, so
/// a loaded host does not decide the outcome.
const POLL_INTERVAL: Duration = Duration::from_millis(25);
/// See [`POLL_INTERVAL`].
const POLLS: usize = 800;

/// The `StackStatus` a create reports while it runs, the one a completed create reports, and
/// the prefix every status a rolled-back create reports begins with.
const CREATE_IN_PROGRESS: &str = "CREATE_IN_PROGRESS";
const CREATE_COMPLETE: &str = "CREATE_COMPLETE";
const ROLLBACK: &str = "ROLLBACK";

/// The `BrokerState` Amazon MQ reports a broker in once it has finished coming up.
///
/// That service's `CreateBroker` is asynchronous, so the broker it mints is answered for in a
/// non-terminal state and this one is owed on ITS own clock, measured from the create
/// CloudFormation made for it — which is not the instant the stack's own span elapses. So this
/// is a CONDITION to wait for ([`described_running_broker`]) and never something the stack's
/// completion decides.
const BROKER_RUNNING: &str = "RUNNING";

/// The logical ids the templates here declare their resources under.
const SECRET_LOGICAL_ID: &str = "Secret";
const TARGET_LOGICAL_ID: &str = "Target";

/// The `secret-string` field of a reference, which is the only value that field may carry.
const SECRET_STRING: &str = "SecretString";

/// The version stage Secrets Manager serves by default, which a reference may also state.
const CURRENT_STAGE: &str = "AWSCURRENT";

/// The head of the diagnostic a reference that cannot be honoured fails its resource with.
const UNRESOLVED: &str = "Unable to resolve the dynamic reference";

/// The emulator with the services this rule spans, mounted on ONE engine exactly as
/// `core/router/src/lib.rs` mounts them. Request authentication is off, as in every other
/// suite here; the in-process callback CloudFormation makes signs its request either way (the
/// credential scope is what routes it).
fn build_router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_cloudformation_cloudformation::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_secretsmanager_secretsmanager::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(service_com_amazonaws_ssm_amazonssm::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(service_com_amazonaws_mq_mq::EmulatedService::new(engine));
    router
}

/// A SigV4 `Authorization` header whose credential scope names `service`, which is what
/// routes the request and what each handler reads its Region from. The signature is never
/// checked: this router runs with request authentication off.
fn authorization(service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20260101/{REGION}/{service}/aws4_request, \
         SignedHeaders=host, Signature=0000000000000000000000000000000000000000000000000000000000000000"
    )
}

/// Call one CloudFormation operation over its awsQuery wire.
async fn cloudformation(
    router: &MultiServiceRouter,
    action: &str,
    members: &[(&str, &str)],
) -> (u16, String) {
    let mut body = format!("Action={action}&Version=2010-05-15");
    for (key, value) in members {
        body.push('&');
        body.push_str(&form_encode(key));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("cloudformation.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", authorization("cloudformation"))
        .body(Bytes::from(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    (status, String::from_utf8_lossy(response.body()).to_string())
}

/// Call one operation of an awsJson1_1 service over its own wire.
async fn json_rpc(
    router: &MultiServiceRouter,
    signing_name: &str,
    target_prefix: &str,
    operation: &str,
    request: &Value,
) -> (u16, Value) {
    let http_request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("{signing_name}.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("{target_prefix}.{operation}"))
        .header("authorization", authorization(signing_name))
        .body(Bytes::from(request.to_string()))
        .expect("a well-formed request");
    let response = router.dispatch(http_request).await;
    let status = response.status().as_u16();
    let payload: Value = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    (status, payload)
}

/// Call one AWS Secrets Manager operation.
async fn secretsmanager(
    router: &MultiServiceRouter,
    operation: &str,
    request: &Value,
) -> (u16, Value) {
    json_rpc(
        router,
        "secretsmanager",
        "secretsmanager",
        operation,
        request,
    )
    .await
}

/// Call one AWS Systems Manager operation.
async fn ssm(router: &MultiServiceRouter, operation: &str, request: &Value) -> (u16, Value) {
    json_rpc(router, "ssm", "AmazonSSM", operation, request).await
}

/// Read one Amazon MQ broker over that service's restJson1 wire: `GET /v1/brokers/{id}`.
async fn described_broker(router: &MultiServiceRouter, broker_id: &str) -> (u16, Value) {
    let request = http::Request::builder()
        .method("GET")
        .uri(format!("/v1/brokers/{}", form_encode(broker_id)))
        .header("host", format!("mq.{REGION}.amazonaws.com"))
        .header("content-type", "application/json")
        .header("authorization", authorization("mq"))
        .body(Bytes::new())
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload: Value = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    (status, payload)
}

/// [`described_broker`], read again until the broker reports the state Amazon MQ owes a
/// created one ([`BROKER_RUNNING`]), or the polls run out.
///
/// The broker's own `CREATION_IN_PROGRESS` -> `RUNNING` move is owed on ITS clock, which the
/// stack reaching `CREATE_COMPLETE` says nothing about: the condition is waited for rather
/// than assumed to have happened by then. A read that is not answered at all is no transient
/// condition — the id came out of the stack's own outputs — so it is answered straight back
/// for the assertions to report rather than waited on.
async fn described_running_broker(router: &MultiServiceRouter, broker_id: &str) -> (u16, Value) {
    let mut last = described_broker(router, broker_id).await;
    for _ in 0..POLLS {
        let running = last.1.get("brokerState").and_then(Value::as_str) == Some(BROKER_RUNNING);
        if last.0 != 200 || running {
            return last;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
        last = described_broker(router, broker_id).await;
    }
    last
}

/// Submit one template as a stack create.
async fn create_stack(router: &MultiServiceRouter, case: &str, stack: &str, template: &str) {
    let (status, body) = cloudformation(
        router,
        "CreateStack",
        &[("StackName", stack), ("TemplateBody", template)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CreateStack, got {status} with body {body}"
    );
}

/// The `StackStatus` the stack reports now, with the answer it was read from.
async fn stack_status(router: &MultiServiceRouter, stack: &str) -> (String, String) {
    let (status, body) = cloudformation(router, "DescribeStacks", &[("StackName", stack)]).await;
    assert_eq!(
        status, 200,
        "DescribeStacks {stack:?}: expected HTTP 200, got {status} with body {body}"
    );
    (opt_element(&body, "StackStatus").unwrap_or_default(), body)
}

/// Poll `stack` until it reports a status the create has FINISHED in — anything other than
/// the one it is still running in — and answer that status with the answer it was read from.
/// Every transition on the way is owed on the stack's own clock, so this waits for the
/// condition rather than for a span.
async fn wait_past_the_create(router: &MultiServiceRouter, stack: &str) -> (String, String) {
    let mut last = stack_status(router, stack).await;
    for _ in 0..POLLS {
        if last.0 != CREATE_IN_PROGRESS {
            return last;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
        last = stack_status(router, stack).await;
    }
    last
}

/// Every `ResourceStatusReason` the stack has recorded, so a deployment that did not reach
/// the status the rule states reports WHY rather than only that it did not.
async fn failure_reasons(router: &MultiServiceRouter, stack: &str) -> Vec<String> {
    let (_, events) = cloudformation(router, "DescribeStackEvents", &[("StackName", stack)]).await;
    elements(&events, "ResourceStatusReason")
}

/// The `SecretString` AWS Secrets Manager really holds for one secret, read through that
/// service's own API — the source every expected value here is computed from.
async fn held_secret_string(router: &MultiServiceRouter, case: &str, secret_id: &str) -> String {
    let (status, answer) =
        secretsmanager(router, "GetSecretValue", &json!({ "SecretId": secret_id })).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from GetSecretValue for {secret_id:?}, got {status} \
         with body {answer}"
    );
    answer
        .get("SecretString")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: no SecretString for {secret_id:?} in {answer}"))
        .to_string()
}

/// One output of a completed stack, read through `DescribeStacks`.
fn output(case: &str, described: &str, key: &str) -> String {
    outputs_of(described)
        .remove(key)
        .unwrap_or_else(|| panic!("case {case:?}: no {key:?} output in {described}"))
}

/// How a template states the secret's own value.
#[derive(Debug)]
enum Held {
    /// `SecretString`: the value the template itself carries.
    Written(&'static str),
    /// `GenerateSecretString`: the document the template carries with ONE key generated by
    /// Secrets Manager, which is the form a template uses for a credential whose password
    /// nothing but that service knows.
    Generated {
        document: &'static str,
        key: &'static str,
        length: u64,
        /// The characters the generation must leave out (`ExcludeCharacters`), for a
        /// declaration whose consumer holds the value to a character rule of its own.
        excluded: Option<&'static str>,
    },
}

impl Held {
    /// The properties stating this value on an `AWS::SecretsManager::Secret`.
    fn declared(&self) -> Value {
        match self {
            Self::Written(text) => json!({ "SecretString": text }),
            Self::Generated {
                document,
                key,
                length,
                excluded,
            } => {
                let mut generate = json!({
                    "SecretStringTemplate": document,
                    "GenerateStringKey": key,
                    "PasswordLength": length,
                });
                if let Some(excluded) = excluded {
                    generate["ExcludeCharacters"] = json!(excluded);
                }
                json!({ "GenerateSecretString": generate })
            }
        }
    }
}

/// How a row's template WRITES the dynamic reference — the axis the rows differ along.
#[derive(Debug)]
struct Reference {
    /// Whether the secret is named by the ARN a `Ref` to it resolves to (which carries
    /// colons of its own) rather than by its name.
    by_arn: bool,
    /// Whether the optional `secret-string` field is written out or left empty.
    states_secret_string: bool,
    /// The `json-key` the reference names, or `None` for one resolving to the whole secret
    /// text.
    json_key: Option<&'static str>,
    /// The `version-stage` the reference names, or `None` for the one that service serves by
    /// default.
    version_stage: Option<&'static str>,
    /// What the property writes around the reference, for a value a template composes out of
    /// its own text and the secret.
    around: (&'static str, &'static str),
}

impl Reference {
    /// The reference as the template writes it:
    /// `{{resolve:secretsmanager:<secret id>:<secret string>:<json key>:<version stage>}}`,
    /// with every field this row leaves out written as nothing at all and the trailing empty
    /// ones dropped.
    fn text(&self, secret_name: &str) -> String {
        let mut key = if self.by_arn {
            format!("${{{SECRET_LOGICAL_ID}}}")
        } else {
            secret_name.to_string()
        };
        let mut fields = vec![
            if self.states_secret_string {
                SECRET_STRING
            } else {
                ""
            },
            self.json_key.unwrap_or(""),
            self.version_stage.unwrap_or(""),
        ];
        while fields.last() == Some(&"") {
            fields.pop();
        }
        for field in fields {
            key.push(':');
            key.push_str(field);
        }
        format!("{{{{resolve:secretsmanager:{key}}}}}")
    }

    /// The property value a row declares: the reference inside whatever the row writes around
    /// it, through `Fn::Sub` where the secret is named by the ARN of the secret the same
    /// stack declares.
    fn declared(&self, secret_name: &str) -> Value {
        let (before, after) = self.around;
        let text = format!("{before}{}{after}", self.text(secret_name));
        if self.by_arn {
            json!({ "Fn::Sub": text })
        } else {
            json!(text)
        }
    }

    /// What the rule says the owning service must have been asked for the property with,
    /// computed from the text the secret really holds.
    fn expected(&self, case: &str, held: &str) -> String {
        let value = match self.json_key {
            None => held.to_string(),
            Some(json_key) => {
                let document: Value = serde_json::from_str(held).unwrap_or_else(|_| {
                    panic!("case {case:?}: the secret does not hold a JSON document: {held:?}")
                });
                match document.get(json_key) {
                    Some(Value::String(text)) => text.clone(),
                    Some(other) => other.to_string(),
                    None => panic!("case {case:?}: the secret holds no {json_key:?}: {held:?}"),
                }
            }
        };
        let (before, after) = self.around;
        format!("{before}{value}{after}")
    }
}

/// One row of the table: the secret a stack declares, and the reference the resource beside it
/// is configured with.
struct Row {
    case: &'static str,
    /// The names this row's template declares. Distinct per row: every row deploys into the
    /// one account this suite's engine holds.
    secret_name: &'static str,
    parameter_name: &'static str,
    held: Held,
    reference: Reference,
}

/// The credential document the rows that write their own value declare: a JSON object with
/// string members and a numeric one, so a reference naming either resolves to how the
/// document writes it.
const WRITTEN_DOCUMENT: &str =
    r#"{"username":"admin","password":"Wr1tten-Example-Pass","port":5671}"#;

/// The document the generated rows declare, whose `password` key Secrets Manager draws.
const GENERATED_TEMPLATE: &str = r#"{"username":"admin"}"#;

/// The characters an Amazon MQ user's password may not carry, from that service's own
/// modelled documentation of `User$Password`: "must not contain commas, colons, or equal
/// signs (,:=)".
const MQ_EXCLUDED_CHARACTERS: &str = ",:=";

const ROWS: &[Row] = &[
    Row {
        // The recorded shape: a password Secrets Manager generated into a document, named by
        // the ARN of the secret the same stack declares.
        case: "a generated password behind a json key, by the secret's ARN",
        secret_name: "cfn-dynref-generated-arn",
        parameter_name: "/probe/cfn-dynref/generated-arn",
        held: Held::Generated {
            document: GENERATED_TEMPLATE,
            key: "password",
            length: 24,
            excluded: None,
        },
        reference: Reference {
            by_arn: true,
            states_secret_string: true,
            json_key: Some("password"),
            version_stage: None,
            around: ("", ""),
        },
    },
    Row {
        // The same secret named by its NAME rather than by its ARN.
        case: "a generated password behind a json key, by the secret's name",
        secret_name: "cfn-dynref-generated-name",
        parameter_name: "/probe/cfn-dynref/generated-name",
        held: Held::Generated {
            document: GENERATED_TEMPLATE,
            key: "password",
            length: 32,
            excluded: None,
        },
        reference: Reference {
            by_arn: false,
            states_secret_string: true,
            json_key: Some("password"),
            version_stage: None,
            around: ("", ""),
        },
    },
    Row {
        // A key of a document the TEMPLATE wrote, whose value is a string.
        case: "a string key of a secret document the template wrote",
        secret_name: "cfn-dynref-written-string",
        parameter_name: "/probe/cfn-dynref/written-string",
        held: Held::Written(WRITTEN_DOCUMENT),
        reference: Reference {
            by_arn: false,
            states_secret_string: true,
            json_key: Some("username"),
            version_stage: None,
            around: ("", ""),
        },
    },
    Row {
        // ...and one whose value the document writes as a NUMBER, which a property carries as
        // text either way.
        case: "a numeric key of a secret document the template wrote",
        secret_name: "cfn-dynref-written-number",
        parameter_name: "/probe/cfn-dynref/written-number",
        held: Held::Written(WRITTEN_DOCUMENT),
        reference: Reference {
            by_arn: true,
            states_secret_string: true,
            json_key: Some("port"),
            version_stage: None,
            around: ("", ""),
        },
    },
    Row {
        // No json-key at all: the reference resolves to the WHOLE secret text, which here is
        // not a JSON document.
        case: "the whole secret text, with no json key",
        secret_name: "cfn-dynref-whole-text",
        parameter_name: "/probe/cfn-dynref/whole-text",
        held: Held::Written("a-whole-secret-text"),
        reference: Reference {
            by_arn: false,
            states_secret_string: false,
            json_key: None,
            version_stage: None,
            around: ("", ""),
        },
    },
    Row {
        // The optional secret-string field left EMPTY while the json-key after it is written,
        // which is how a reference skips a field it does not state.
        case: "a json key with the secret-string field left empty",
        secret_name: "cfn-dynref-empty-field",
        parameter_name: "/probe/cfn-dynref/empty-field",
        held: Held::Written(WRITTEN_DOCUMENT),
        reference: Reference {
            by_arn: false,
            states_secret_string: false,
            json_key: Some("password"),
            version_stage: None,
            around: ("", ""),
        },
    },
    Row {
        // The version stage stated explicitly, after an ARN that carries colons of its own.
        case: "a json key at the default version stage, stated explicitly, by the ARN",
        secret_name: "cfn-dynref-stage",
        parameter_name: "/probe/cfn-dynref/stage",
        held: Held::Generated {
            document: GENERATED_TEMPLATE,
            key: "password",
            length: 16,
            excluded: None,
        },
        reference: Reference {
            by_arn: true,
            states_secret_string: true,
            json_key: Some("password"),
            version_stage: Some(CURRENT_STAGE),
            around: ("", ""),
        },
    },
    Row {
        // The reference INSIDE a longer value, which is how a template composes a connection
        // string out of its own text and a secret.
        case: "a reference inside a longer property value",
        secret_name: "cfn-dynref-composed",
        parameter_name: "/probe/cfn-dynref/composed",
        held: Held::Generated {
            document: GENERATED_TEMPLATE,
            key: "password",
            length: 20,
            excluded: None,
        },
        reference: Reference {
            by_arn: true,
            states_secret_string: true,
            json_key: Some("password"),
            version_stage: None,
            around: ("user=admin;password=", ";host=broker.example"),
        },
    },
];

/// The declaration of the secret a template carries, under the name it states.
fn declared_secret(secret_name: &str, held: &Held) -> Value {
    let mut properties = held.declared();
    properties["Name"] = json!(secret_name);
    json!({ "Type": "AWS::SecretsManager::Secret", "Properties": properties })
}

/// One template, built out of the resources it declares and the outputs it publishes.
fn template(resources: Vec<(&str, Value)>, outputs: Vec<(&str, Value)>) -> String {
    let mut declared = Map::new();
    for (logical_id, resource) in resources {
        declared.insert(logical_id.to_string(), resource);
    }
    let mut published = Map::new();
    for (key, value) in outputs {
        published.insert(key.to_string(), json!({ "Value": value }));
    }
    json!({
        "AWSTemplateFormatVersion": "2010-09-09",
        "Resources": Value::Object(declared),
        "Outputs": Value::Object(published),
    })
    .to_string()
}

/// The template one row deploys: the secret, and beside it an `AWS::SSM::Parameter` whose
/// `Value` is the reference under test — a property the owning service REPORTS, so what that
/// service was really asked for the resource with is readable.
fn row_template(row: &Row) -> String {
    template(
        vec![
            (SECRET_LOGICAL_ID, declared_secret(row.secret_name, &row.held)),
            (
                TARGET_LOGICAL_ID,
                json!({
                    "Type": "AWS::SSM::Parameter",
                    "DependsOn": SECRET_LOGICAL_ID,
                    "Properties": {
                        "Name": row.parameter_name,
                        "Type": "String",
                        "Value": row.reference.declared(row.secret_name),
                    },
                }),
            ),
        ],
        vec![
            ("SecretArn", json!({ "Ref": SECRET_LOGICAL_ID })),
            ("ParameterName", json!({ "Ref": TARGET_LOGICAL_ID })),
        ],
    )
}

/// What the parameter one row declared carries in AWS Systems Manager — the document the
/// owning service was really asked for the resource with.
async fn parameter_value(router: &MultiServiceRouter, case: &str, name: &str) -> String {
    let (status, answer) = ssm(router, "GetParameter", &json!({ "Name": name })).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from GetParameter for {name:?}, got {status} with body \
         {answer}"
    );
    answer
        .pointer("/Parameter/Value")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: no parameter value for {name:?} in {answer}"))
        .to_string()
}

/// Every row: the stack completes, and the value the owning service holds for the property is
/// the one the secret holds — never the reference's own text.
#[tokio::test]
async fn a_reference_reaches_the_owning_service_as_the_value_the_secret_holds() {
    let router = build_router();

    for (index, row) in ROWS.iter().enumerate() {
        let case = row.case;
        let stack = format!("cfn-dynref-{index}");
        create_stack(&router, case, &stack, &row_template(row)).await;
        let (status, described) = wait_past_the_create(&router, &stack).await;
        assert_eq!(
            status, CREATE_COMPLETE,
            "case {case:?}: expected the stack to reach {CREATE_COMPLETE}, got {described} with \
             resource failures {:?}",
            failure_reasons(&router, &stack).await
        );

        let published = output(case, &described, "ParameterName");
        assert_eq!(
            published, row.parameter_name,
            "case {case:?}: expected the stack to publish the declared parameter name {:?}, got \
             {published:?}",
            row.parameter_name
        );
        // The expected value is computed from what the SECRET holds, read through Secrets
        // Manager itself: for a generated password that is the only possible source, the
        // value being that service's to draw.
        let secret_arn = output(case, &described, "SecretArn");
        let held = held_secret_string(&router, case, &secret_arn).await;
        let expected = row.reference.expected(case, &held);
        let actual = parameter_value(&router, case, row.parameter_name).await;
        assert_eq!(
            actual, expected,
            "case {case:?}: expected the parameter to carry {expected:?}, got {actual:?}"
        );
        assert!(
            !actual.contains("{{resolve:"),
            "case {case:?}: expected the reference to have been resolved before the create, got \
             {actual:?}"
        );
    }
}

/// One row of the table of references nothing can honour: the secret the stack declares (none
/// at all, for a reference naming a secret nothing holds) and the reference text its resource
/// is declared with.
struct Unresolvable {
    case: &'static str,
    /// The secret the template declares, or `None` for a template declaring no secret.
    secret: Option<(&'static str, Held)>,
    parameter_name: &'static str,
    reference: &'static str,
}

const UNRESOLVABLE: &[Unresolvable] = &[
    Unresolvable {
        case: "a reference naming a secret nothing holds",
        secret: None,
        parameter_name: "/probe/cfn-dynref/absent-secret",
        reference: "{{resolve:secretsmanager:cfn-dynref-nothing-holds:SecretString:password}}",
    },
    Unresolvable {
        case: "a json key the secret's document has no member for",
        secret: Some((
            "cfn-dynref-absent-key",
            Held::Written(r#"{"username":"admin"}"#),
        )),
        parameter_name: "/probe/cfn-dynref/absent-key",
        reference: "{{resolve:secretsmanager:cfn-dynref-absent-key:SecretString:password}}",
    },
    Unresolvable {
        case: "a json key of a secret that holds no JSON document",
        secret: Some(("cfn-dynref-not-json", Held::Written("plain-text-secret"))),
        parameter_name: "/probe/cfn-dynref/not-json",
        reference: "{{resolve:secretsmanager:cfn-dynref-not-json:SecretString:password}}",
    },
    Unresolvable {
        case: "a secret-string field that is not the one supported",
        secret: Some((
            "cfn-dynref-bad-field",
            Held::Written(r#"{"password":"Wr1tten-Example-Pass"}"#),
        )),
        parameter_name: "/probe/cfn-dynref/bad-field",
        reference: "{{resolve:secretsmanager:cfn-dynref-bad-field:SecretBinary:password}}",
    },
];

/// Every row: the resource whose property carries a reference nothing can honour is not
/// created in the owning service, the stack does not complete, and the reason it records names
/// the reference.
#[tokio::test]
async fn a_reference_nothing_can_honour_fails_its_resource_with_the_diagnostic() {
    let router = build_router();

    for (index, row) in UNRESOLVABLE.iter().enumerate() {
        let case = row.case;
        let stack = format!("cfn-dynref-unresolvable-{index}");
        let mut resources = Vec::new();
        let mut target = json!({
            "Type": "AWS::SSM::Parameter",
            "Properties": {
                "Name": row.parameter_name,
                "Type": "String",
                "Value": row.reference,
            },
        });
        if let Some((secret_name, held)) = &row.secret {
            resources.push((SECRET_LOGICAL_ID, declared_secret(secret_name, held)));
            target["DependsOn"] = json!(SECRET_LOGICAL_ID);
        }
        resources.push((TARGET_LOGICAL_ID, target));
        create_stack(&router, case, &stack, &template(resources, Vec::new())).await;

        let (status, described) = wait_past_the_create(&router, &stack).await;
        let reasons = failure_reasons(&router, &stack).await;
        assert!(
            status.starts_with(ROLLBACK),
            "case {case:?}: expected the stack to roll back rather than report {status:?}, with \
             {described} and resource failures {reasons:?}"
        );
        assert!(
            reasons
                .iter()
                .any(|reason| reason.contains(UNRESOLVED) && reason.contains(row.reference)),
            "case {case:?}: expected a resource failure quoting {:?} and naming {:?}, got \
             {reasons:?}",
            UNRESOLVED,
            row.reference
        );
        // ...and nothing was created for it: the parameter the declaration asked for is not
        // one Systems Manager holds.
        let (status, answer) = ssm(&router, "GetParameter", &json!({ "Name": row.parameter_name }))
            .await;
        assert_ne!(
            status, 200,
            "case {case:?}: expected Systems Manager to hold no parameter {:?}, got {answer}",
            row.parameter_name
        );
    }
}

/// The estate the issue this suite was written for reports: a secret whose password Secrets
/// Manager generates, and an `AWS::AmazonMQ::Broker` beside it whose initial user's `Password`
/// is a dynamic reference to that key of that secret.
///
/// Amazon MQ holds a user's password to its own documented rule — "must not contain commas,
/// colons, or equal signs" — so a broker asked for with the reference's own TEXT is refused by
/// that service and the stack rolls back. A stack that completes with the broker running is
/// therefore a broker created with the password the reference resolved to.
#[tokio::test]
async fn a_broker_is_created_with_the_password_its_reference_resolves_to() {
    let case = "a RabbitMQ broker whose user password is a reference to a generated secret";
    let router = build_router();
    let stack = "cfn-dynref-broker";
    let secret_name = "cfn-dynref-broker-secret";
    let broker_name = "cfn-dynref-broker-rabbit";
    const BROKER_LOGICAL_ID: &str = "RabbitMqBroker";

    let body = template(
        vec![
            (
                SECRET_LOGICAL_ID,
                declared_secret(
                    secret_name,
                    // The broker holds its user's password to Amazon MQ's own documented
                    // rule — "must not contain commas, colons, or equal signs (,:=)" — so
                    // the declaration asks Secrets Manager to leave those three characters
                    // out of the password it generates, exactly as a template whose consumer
                    // has a character rule of its own has to.
                    &Held::Generated {
                        document: GENERATED_TEMPLATE,
                        key: "password",
                        length: 24,
                        excluded: Some(MQ_EXCLUDED_CHARACTERS),
                    },
                ),
            ),
            (
                BROKER_LOGICAL_ID,
                json!({
                    "Type": "AWS::AmazonMQ::Broker",
                    "DependsOn": SECRET_LOGICAL_ID,
                    "Properties": {
                        "BrokerName": broker_name,
                        "EngineType": "RABBITMQ",
                        "EngineVersion": "3.13",
                        "HostInstanceType": "mq.m5.large",
                        "DeploymentMode": "SINGLE_INSTANCE",
                        "PubliclyAccessible": true,
                        "Logs": { "General": true },
                        "Users": [{
                            "Username": "admin",
                            "Password": {
                                "Fn::Sub":
                                    format!("{{{{resolve:secretsmanager:${{{SECRET_LOGICAL_ID}}}:{SECRET_STRING}:password}}}}"),
                            },
                        }],
                    },
                }),
            ),
        ],
        vec![
            ("SecretArn", json!({ "Ref": SECRET_LOGICAL_ID })),
            ("BrokerId", json!({ "Ref": BROKER_LOGICAL_ID })),
            (
                "BrokerArn",
                json!({ "Fn::GetAtt": [BROKER_LOGICAL_ID, "Arn"] }),
            ),
        ],
    );

    create_stack(&router, case, stack, &body).await;
    let (status, described) = wait_past_the_create(&router, stack).await;
    assert_eq!(
        status, CREATE_COMPLETE,
        "case {case:?}: expected the stack to reach {CREATE_COMPLETE}, got {described} with \
         resource failures {:?}",
        failure_reasons(&router, stack).await
    );

    // The broker the stack published is one Amazon MQ answers for, configured as the template
    // declared it.
    let broker_id = output(case, &described, "BrokerId");
    let broker_arn = output(case, &described, "BrokerArn");
    let (status, broker) = described_running_broker(&router, &broker_id).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected Amazon MQ to resolve the broker id {broker_id:?} the stack \
         published, got {status} with body {broker}"
    );
    for (member, expected) in [
        ("brokerName", broker_name),
        ("brokerState", BROKER_RUNNING),
        // `DescribeBroker` reports the engine by the name Amazon MQ publishes for it
        // (`EngineType`'s modelled value spelled as that service answers it), not by the
        // enum spelling the declaration states.
        ("engineType", "RabbitMQ"),
        ("brokerArn", broker_arn.as_str()),
    ] {
        let reported = broker.get(member).and_then(Value::as_str);
        assert_eq!(
            reported,
            Some(expected),
            "case {case:?}: expected {member} {expected:?}, got {reported:?} in {broker}"
        );
    }
    let usernames: Vec<&str> = broker
        .get("users")
        .and_then(Value::as_array)
        .map(|users| {
            users
                .iter()
                .filter_map(|user| user.get("username").and_then(Value::as_str))
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(
        usernames,
        vec!["admin"],
        "case {case:?}: expected the declared user to have reached Amazon MQ, got {broker}"
    );
}
