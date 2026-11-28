// SPDX-License-Identifier: CC0-1.0

//! How this service QUOTES the expression in a `@pattern` constraint clause, end
//! to end over HTTP.
//!
//! RULE: the requirement a `@pattern` violation states is the expression a value
//! has to satisfy, quoted WITHOUT the whole-value `^` / `$` the model writes
//! around it — that pair is how Smithy turns a find into a whole-string match
//! rather than part of the character grammar, so a client that lifts the
//! expression out of the message and recompiles it gets the one the service
//! advertises. Everything else of the clause is unchanged: the counted envelope,
//! the member path, the echoed value (withheld for a `@sensitive` member), the
//! `;`-join and the ordering of the clauses of one member.
//!
//! The rule is a property of the CLAUSE, so it holds wherever this service words
//! one: for the boundary walk in front of a handler (every row of the table
//! below, over a `@httpQuery` member, a `@httpLabel` member and a body member,
//! at one and at two violations) and for a handler that judges the syntax of a
//! member itself ([`a_handler_authored_pattern_clause_is_quoted_unanchored`]).
//!
//! Each row states the MODELLED expression as the model writes it and checks the
//! clause against the expression minus that pair, so a row cannot pass by quoting
//! the model's own text.
//!
//! Every assertion here goes through the router, so it exercises the real wire
//! path (route matching, model constraint validation, serialization) rather than
//! calling a handler directly.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_bedrockagent_amazonbedrockagentbuildtimelambda::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 `Authorization` header scoped to the `bedrock` signing name: the
/// routing key this service claims on.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/eu-west-1/bedrock/aws4_request, \
     SignedHeaders=host;x-amz-date, Signature=0123456789abcdef";

/// The refusal class every row answers, and the status this model binds to it
/// (`smithy_query(query="operation",operation="DeleteRetrievalInstruction")`).
const VALIDATION: (&str, u16) = ("ValidationException", 400);

/// An id of the modelled `Id` shape, carried by every row whose path needs a
/// knowledge base id that is itself inside its syntax
/// (`smithy_query(query="shape",shape="Id")`: `^[0-9a-zA-Z]{10}$`).
const KNOWLEDGE_BASE_ID: &str = "0123456789";

/// An id of the modelled `RetrievalInstructionId` shape
/// (`smithy_query(query="shape",shape="RetrievalInstructionId")`:
/// `^[a-z0-9]{10}$`), so the only clause a row of that path produces is the one
/// it is about.
const RETRIEVAL_INSTRUCTION_ID: &str = "ri00000001";

/// The modelled `@pattern` of every member a row below refuses, as the MODEL
/// writes it. Sources, one `smithy_query(query="shape",shape=…)` each:
/// `RetrievalInstructionNumericalVersion`, `RetrievalInstructionName`, `Id`,
/// `VpcConfigurationId`, `RetrievalInstructionId`.
const NUMERICAL_VERSION_PATTERN: &str = "^[1-9][0-9]{0,8}$";
const RETRIEVAL_INSTRUCTION_NAME_PATTERN: &str = "^([0-9a-zA-Z][_-]?){1,100}$";
const ID_PATTERN: &str = "^[0-9a-zA-Z]{10}$";
const VPC_CONFIGURATION_ID_PATTERN: &str = "^[a-z0-9](?:[a-z0-9-]{30}[a-z0-9])$";
const RETRIEVAL_INSTRUCTION_ID_PATTERN: &str = "^[a-z0-9]{10}$";

/// The modelled `FlowIdentifier` `@pattern`
/// (`smithy_query(query="shape",shape="FlowIdentifier")`), which a flow handler
/// judges itself.
const FLOW_IDENTIFIER_PATTERN: &str =
    "^(arn:aws:bedrock:[a-z0-9-]{1,20}:[0-9]{12}:flow/[0-9a-zA-Z]{10})|([0-9a-zA-Z]{10})$";

/// `pattern` minus the whole-value pair the model writes around it — the
/// expression a clause has to quote. Computed here, in the test, so no row reads
/// its expectation off the code under test.
fn stated(pattern: &str) -> String {
    pattern
        .strip_prefix('^')
        .and_then(|body| body.strip_suffix('$'))
        .unwrap_or_else(|| panic!("the modelled pattern {pattern:?} is written ^…$"))
        .to_string()
}

/// The requirement clause a `@pattern` violation states.
fn pattern_requirement(pattern: &str) -> String {
    format!(
        "Member must satisfy regular expression pattern: {}",
        stated(pattern)
    )
}

/// One HTTP answer from the emulator, parsed.
#[derive(Debug)]
struct Answer {
    status: u16,
    /// The restJson1 error shape name, from `x-amzn-errortype` or the body's
    /// `__type`; `None` on a success.
    error_type: Option<String>,
    body: Value,
}

impl Answer {
    /// The refusal's `message` member, or the empty string on an answer carrying
    /// none.
    fn message(&self) -> String {
        self.body["message"].as_str().unwrap_or_default().to_string()
    }
}

/// The emulator under test: one router over one shared engine, exactly as the
/// production router mounts this service.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(engine));
        Self { router }
    }

    async fn call(&self, method: &str, uri: &str, body: Option<Value>) -> Answer {
        let bytes = body
            .map(|value| Bytes::from(serde_json::to_vec(&value).expect("serialize request body")))
            .unwrap_or_default();
        let request = http::Request::builder()
            .method(method)
            .uri(uri)
            .header("host", "bedrock-agent.eu-west-1.amazonaws.com")
            .header("authorization", AUTHORIZATION)
            .header("content-type", "application/json")
            .body(bytes)
            .expect("build the request");

        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let header_error = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .map(ToString::to_string);
        let raw = response.into_body();
        let body: Value = if raw.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&raw).unwrap_or_else(|error| {
                panic!(
                    "{method} {uri} -> {status} body is not JSON ({error}): {}",
                    String::from_utf8_lossy(&raw)
                )
            })
        };
        let error_type = header_error.or_else(|| {
            body.get("__type")
                .and_then(Value::as_str)
                .map(ToString::to_string)
        });
        Answer {
            status,
            error_type,
            body,
        }
    }
}

/// One row: a request whose only bad value breaks a `@pattern`, and the whole
/// message the refusal has to carry.
struct Row {
    case: &'static str,
    method: &'static str,
    uri: String,
    body: Option<Value>,
    expected: String,
    /// The modelled expression the clause is about, which the message must NOT
    /// carry as the model writes it.
    modelled: &'static str,
}

/// RULE: a `@pattern` clause quotes the expression unanchored, in every binding
/// the model carries a constrained member in and at any number of clauses.
#[tokio::test]
async fn every_pattern_clause_quotes_the_expression_without_its_whole_value_anchors() {
    let emulator = Emulator::new();

    // The rows differ along the BINDING of the refused member (a query string, a
    // URI label, a body member), along whether the model marks its value
    // `@sensitive` (so whether the clause echoes the value at all), along the
    // number of clauses one member produces (one, and the length-then-pattern
    // pair), and along the shape and so the expression quoted.
    let rows: Vec<Row> = vec![
        Row {
            case: "a @httpQuery member outside its pattern",
            method: "DELETE",
            uri: format!(
                "/retrievalinstructions/{RETRIEVAL_INSTRUCTION_ID}\
                 ?retrievalInstructionVersion=DRAFT"
            ),
            body: None,
            expected: format!(
                "1 validation error detected: Value 'DRAFT' at 'retrievalInstructionVersion' \
                 failed to satisfy constraint: {}",
                pattern_requirement(NUMERICAL_VERSION_PATTERN)
            ),
            modelled: NUMERICAL_VERSION_PATTERN,
        },
        Row {
            case: "a @httpQuery member whose value is a number the expression excludes",
            method: "DELETE",
            uri: format!(
                "/retrievalinstructions/{RETRIEVAL_INSTRUCTION_ID}\
                 ?retrievalInstructionVersion=0"
            ),
            body: None,
            expected: format!(
                "1 validation error detected: Value '0' at 'retrievalInstructionVersion' \
                 failed to satisfy constraint: {}",
                pattern_requirement(NUMERICAL_VERSION_PATTERN)
            ),
            modelled: NUMERICAL_VERSION_PATTERN,
        },
        Row {
            case: "a @sensitive body member outside its pattern, whose value is withheld",
            method: "POST",
            uri: "/retrievalinstructions/".to_string(),
            body: Some(json!({
                "name": "not a name!",
                "instruction": { "persona": "a librarian" },
            })),
            expected: format!(
                "1 validation error detected: Value at 'name' failed to satisfy constraint: {}",
                pattern_requirement(RETRIEVAL_INSTRUCTION_NAME_PATTERN)
            ),
            modelled: RETRIEVAL_INSTRUCTION_NAME_PATTERN,
        },
        Row {
            case: "a @httpLabel member outside its pattern",
            method: "POST",
            uri: format!(
                "/knowledgebases/{KNOWLEDGE_BASE_ID}/datasources/{KNOWLEDGE_BASE_ID}\
                 /ingestionjobs/job_00001/stop"
            ),
            body: None,
            expected: format!(
                "1 validation error detected: Value 'job_00001' at 'ingestionJobId' failed to \
                 satisfy constraint: {}",
                pattern_requirement(ID_PATTERN)
            ),
            modelled: ID_PATTERN,
        },
        Row {
            case: "a @httpLabel member breaking its length AND its pattern",
            method: "GET",
            uri: format!("/knowledgebases/{KNOWLEDGE_BASE_ID}/vpcconfigurations/short-id"),
            body: None,
            expected: format!(
                "2 validation errors detected: Value 'short-id' at 'vpcConfigurationId' failed \
                 to satisfy constraint: Member must have length greater than or equal to 32; \
                 Value 'short-id' at 'vpcConfigurationId' failed to satisfy constraint: {}",
                pattern_requirement(VPC_CONFIGURATION_ID_PATTERN)
            ),
            modelled: VPC_CONFIGURATION_ID_PATTERN,
        },
        Row {
            case: "a @httpLabel member of the admitted length breaking only its pattern",
            method: "GET",
            uri: format!(
                "/knowledgebases/{KNOWLEDGE_BASE_ID}\
                 /vpcconfigurations/Abcdefghijklmnopqrstuvwxyz012345"
            ),
            body: None,
            expected: format!(
                "1 validation error detected: Value 'Abcdefghijklmnopqrstuvwxyz012345' at \
                 'vpcConfigurationId' failed to satisfy constraint: {}",
                pattern_requirement(VPC_CONFIGURATION_ID_PATTERN)
            ),
            modelled: VPC_CONFIGURATION_ID_PATTERN,
        },
        Row {
            case: "a @httpLabel member of another shape, of the admitted length",
            method: "DELETE",
            uri: "/retrievalinstructions/ABCDEFGHIJ".to_string(),
            body: None,
            expected: format!(
                "1 validation error detected: Value 'ABCDEFGHIJ' at 'retrievalInstructionId' \
                 failed to satisfy constraint: {}",
                pattern_requirement(RETRIEVAL_INSTRUCTION_ID_PATTERN)
            ),
            modelled: RETRIEVAL_INSTRUCTION_ID_PATTERN,
        },
    ];

    let (code, status) = VALIDATION;
    for row in &rows {
        let case = row.case;
        let answer = emulator.call(row.method, &row.uri, row.body.clone()).await;
        assert_eq!(
            answer.status, status,
            "case {case:?}: expected HTTP {status}, got {answer:?}"
        );
        assert_eq!(
            answer.error_type.as_deref(),
            Some(code),
            "case {case:?}: expected {code}, got {answer:?}"
        );
        assert_eq!(
            answer.message(),
            row.expected,
            "case {case:?}: expected the refusal worded {:?}, got {:?}",
            row.expected,
            answer.message()
        );
        assert!(
            !answer.message().contains(row.modelled),
            "case {case:?}: the clause must not quote the model's own anchored text \
             {:?}, got {:?}",
            row.modelled,
            answer.message()
        );
    }
}

/// RULE applied to the clause a HANDLER words: a flow identifier of neither form
/// the modelled `FlowIdentifier` admits is refused with the same clause, quoting
/// the same expression the same way.
#[tokio::test]
async fn a_handler_authored_pattern_clause_is_quoted_unanchored() {
    let emulator = Emulator::new();
    let (code, status) = VALIDATION;

    // The rows differ along the operation that addresses the flow (a read, a
    // delete, the work a prepare starts) and along the value, which is the
    // clause's one echoed part. Each value is one the boundary walk admits —
    // the modelled expression's second branch is anchored only at its end, so a
    // value ENDING in ten alphanumerics gets past it — and that the handler then
    // refuses as naming neither form.
    for identifier in ["not-an-identifier", "0123456789012", "nope0123456789"] {
        for (operation, method, uri, body) in [
            (
                "GetFlow",
                "GET",
                format!("/flows/{identifier}/"),
                None::<Value>,
            ),
            (
                "DeleteFlow",
                "DELETE",
                format!("/flows/{identifier}/"),
                None::<Value>,
            ),
            (
                "PrepareFlow",
                "POST",
                format!("/flows/{identifier}/"),
                Some(json!({})),
            ),
        ] {
            let case = format!("{operation} on the flow identifier {identifier:?}");
            let answer = emulator.call(method, &uri, body).await;
            assert_eq!(
                answer.status, status,
                "case {case:?}: expected HTTP {status}, got {answer:?}"
            );
            assert_eq!(
                answer.error_type.as_deref(),
                Some(code),
                "case {case:?}: expected {code}, got {answer:?}"
            );
            let expected = format!(
                "Value '{identifier}' at 'flowIdentifier' failed to satisfy constraint: {}",
                pattern_requirement(FLOW_IDENTIFIER_PATTERN)
            );
            assert_eq!(
                answer.message(),
                expected,
                "case {case:?}: expected the refusal worded {expected:?}, got {:?}",
                answer.message()
            );
            assert!(
                !answer.message().contains(FLOW_IDENTIFIER_PATTERN),
                "case {case:?}: the clause must not quote the model's own anchored text, \
                 got {:?}",
                answer.message()
            );
        }
    }
}
