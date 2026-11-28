// SPDX-License-Identifier: CC0-1.0

//! How an initialization this service cannot complete is FRAMED on the wire a client reads.
//!
//! The rule under test: a modelled client failure of `InitializeService` is answered with
//! the status the model states for that error shape, the `x-amzn-errortype` header naming
//! it, an error body carrying the `__type` discriminator with the modelled `reason` and the
//! sentence naming which resources could not be created — and, because that answer CARRIES
//! content, a `content-length` stating that body's own byte length, so a client or an
//! intermediary that frames the message from the header reads exactly the bytes the origin
//! wrote. The success the model states for the same operation stays a `204` carrying no
//! body at all.
//!
//! The sibling suite `account_initialization.rs` pins WHEN that failure is concluded and
//! WHAT it says, measured on the router's own `dispatch`. This suite pins the FRAMING of
//! the very same answers where the framing is visible: on a served socket, which is the one
//! place the length a client reads is written, since the HTTP writer that states it sits
//! below anything this crate decides (and is also why the `204` row requires nothing of the
//! header — RFC 9110 §15.3.5 forbids a body on that status and the writer drops the header
//! from it).
//!
//! The rows differ along what the rule generalises over: which kinds of resource an
//! initialization could not establish — neither of them, the instance profiles alone, the
//! service-linked role alone — and therefore error bodies of three different lengths, plus
//! the unobstructed success as the contrast. Each refusal's length is asserted against the
//! bytes that arrived rather than against any one recorded number, so the requirement is
//! that the stated length tells the truth about the body, not that it is some constant.

use std::io::{Read, Write};
use std::net::TcpStream;

use serde_json::Value;

use service_com_amazonaws_mgn_applicationmigrationservice::state::SERVER_INSTANCE_PROFILES;

use crate::account_initialization::Emulator;

/// The Region the request's SigV4 credential scope names — also what routes it to this
/// service, since the scope's service name is this service's signing name.
const REGION: &str = "us-west-2";

/// The modelled error an initialization that could not establish what it needs is answered
/// with, and the status the model states for it
/// (`smithy_query(query="shape",shape="ValidationException")`: `errorKind` client,
/// `httpError` 400).
const VALIDATION: &str = "ValidationException";
const VALIDATION_STATUS: u16 = 400;

/// The `ValidationExceptionReason` such a failure carries. Source:
/// `smithy_query(query="shape",shape="ValidationExceptionReason")`, whose declared values
/// are `unknownOperation`, `cannotParse`, `fieldValidationFailed` and this one — the value
/// for a failure that is about neither the operation, nor parsing, nor a field.
const REASON_OTHER: &str = "other";

/// The status the model states for this operation's success
/// (`smithy_query(query="operation",operation="InitializeService")`: `http` code 204).
const NO_CONTENT_STATUS: u16 = 204;

/// What an initialization could not establish, seeded into IAM's own store as a
/// precondition no sequence of calls reaches cheaply: an account already at one of IAM's
/// quotas.
#[derive(Debug, Clone, Copy)]
enum Obstruction {
    /// Nothing obstructs it: the initialization establishes everything and succeeds.
    None,
    /// The account is at IAM's role quota, so neither the service-linked role nor the role
    /// an instance profile must hold can be created.
    RoleQuotaReached,
    /// The account is at IAM's instance profile quota, so no profile can be created while
    /// the service-linked role still can.
    InstanceProfileQuotaReached,
    /// The account is at IAM's role quota and already holds the server instance profiles an
    /// earlier initialization established, so only the service-linked role is left.
    RoleQuotaReachedWithProfilesHeld,
}

/// One answer as the socket delivered it: the status line's code, the header fields by
/// lower-cased name, and the body bytes that followed the head.
#[derive(Debug)]
struct Delivered {
    status: u16,
    headers: Vec<(String, String)>,
    body: String,
}

impl Delivered {
    /// The value of one header field, if the answer carries it.
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(field, _)| field == name)
            .map(|(_, value)| value.as_str())
    }

    /// The body parsed as JSON.
    fn json(&self) -> Value {
        serde_json::from_str(&self.body)
            .unwrap_or_else(|error| panic!("the answer's body is not JSON ({error}): {:?}", self.body))
    }
}

/// Everything the server answered one `POST /InitializeService` with, read straight off a
/// socket.
///
/// `connection: close` so the server closes the socket after answering and the read below
/// terminates on EOF rather than on a parse of the very framing these rows are about.
fn initialize_over_a_socket(port: u16) -> Delivered {
    let payload = "{}";
    let request = format!(
        "POST /InitializeService HTTP/1.1\r\n\
         host: localhost\r\n\
         content-type: application/json\r\n\
         authorization: AWS4-HMAC-SHA256 \
         Credential=AKIAEXAMPLEEXAMPLE/20260101/{REGION}/mgn/aws4_request, \
         SignedHeaders=host, Signature=0000\r\n\
         content-length: {}\r\n\
         connection: close\r\n\
         \r\n{payload}",
        payload.len()
    );
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect to the served port");
    stream
        .write_all(request.as_bytes())
        .expect("write the request");
    let mut answered = Vec::new();
    stream
        .read_to_end(&mut answered)
        .expect("read the whole answer");
    let answered = String::from_utf8_lossy(&answered).to_string();

    let (head, body) = answered
        .split_once("\r\n\r\n")
        .unwrap_or_else(|| panic!("a response head and body, got {answered:?}"));
    let mut lines = head.split("\r\n");
    let status_line = lines.next().unwrap_or_default();
    let status = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|code| code.parse().ok())
        .unwrap_or_else(|| panic!("a status code in {status_line:?}"));
    let headers = lines
        .filter_map(|line| line.split_once(':'))
        .map(|(field, value)| (field.trim().to_ascii_lowercase(), value.trim().to_string()))
        .collect();
    Delivered {
        status,
        headers,
        body: body.to_string(),
    }
}

/// The shape name of an error body's `__type`, which restJson1 resolves the error shape
/// from: the discriminator may carry the shape's namespace, and a client reads the name
/// after it.
fn error_shape(body: &Value) -> String {
    let discriminator = body["__type"]
        .as_str()
        .unwrap_or_else(|| panic!("an error body carries a __type, got {body:?}"));
    discriminator
        .rsplit('#')
        .next()
        .unwrap_or(discriminator)
        .to_string()
}

/// The port an endpoint names.
fn port_of(endpoint: &str) -> u16 {
    endpoint
        .rsplit(':')
        .next()
        .and_then(|port| port.parse().ok())
        .unwrap_or_else(|| panic!("an endpoint with a port, got {endpoint:?}"))
}

/// RULE: a refused initialization is delivered as the modelled client failure WITH its
/// body, and states that body's own length; the success it contrasts with carries no body.
#[tokio::test(flavor = "multi_thread")]
async fn an_initialization_is_delivered_with_the_framing_its_answer_needs() {
    // (case, what obstructs the initialization, the sentence the refusal carries — `None`
    // for the row that is not refused at all)
    let rows: [(&str, Obstruction, Option<&str>); 4] = [
        (
            "neither kind of resource can be created",
            Obstruction::RoleQuotaReached,
            Some("Failed to create SLR or instance profiles"),
        ),
        (
            "the instance profiles cannot be created",
            Obstruction::InstanceProfileQuotaReached,
            Some("Failed to create instance profiles"),
        ),
        (
            "only the service-linked role is left to create",
            Obstruction::RoleQuotaReachedWithProfilesHeld,
            Some("Failed to create SLR"),
        ),
        (
            "nothing obstructs the initialization",
            Obstruction::None,
            None,
        ),
    ];

    for (case, obstruction, refusal) in rows {
        let emulator = Emulator::new();
        match obstruction {
            Obstruction::None => {}
            Obstruction::RoleQuotaReached => emulator.fill_role_quota(&[]),
            Obstruction::InstanceProfileQuotaReached => emulator.fill_instance_profile_quota(),
            Obstruction::RoleQuotaReachedWithProfilesHeld => {
                emulator.fill_role_quota(SERVER_INSTANCE_PROFILES);
                emulator.seed_instance_profiles(SERVER_INSTANCE_PROFILES);
            }
        }

        let (endpoint, _served) =
            emulator_server::serve_on_ephemeral_port(emulator.router.clone()).await;
        let port = port_of(&endpoint);
        let delivered = tokio::task::spawn_blocking(move || initialize_over_a_socket(port))
            .await
            .expect("the socket exchange runs to completion");

        let Some(expected_message) = refusal else {
            assert_eq!(
                delivered.status, NO_CONTENT_STATUS,
                "case {case:?}: expected {NO_CONTENT_STATUS}, got {} with {:?}",
                delivered.status, delivered.body
            );
            assert!(
                delivered.body.is_empty(),
                "case {case:?}: the modelled success carries no body, got {:?}",
                delivered.body
            );
            assert_eq!(
                delivered.header("x-amzn-errortype"),
                None,
                "case {case:?}: a success names no error type, got {:?}",
                delivered.header("x-amzn-errortype")
            );
            continue;
        };

        assert_eq!(
            delivered.status, VALIDATION_STATUS,
            "case {case:?}: expected {VALIDATION_STATUS}, got {} with {:?}",
            delivered.status, delivered.body
        );
        assert_eq!(
            delivered.header("x-amzn-errortype"),
            Some(VALIDATION),
            "case {case:?}: expected the answer to name {VALIDATION}, got {:?}",
            delivered.header("x-amzn-errortype")
        );
        assert!(
            !delivered.body.is_empty(),
            "case {case:?}: a refusal must arrive with its error body, got none"
        );
        // The framing requirement: the length the answer states is this body's own, so a
        // client that frames the message from it reads exactly the bytes that were written.
        assert_eq!(
            delivered.header("content-length"),
            Some(delivered.body.len().to_string().as_str()),
            "case {case:?}: expected the stated length to be the body's own {}, got {:?}",
            delivered.body.len(),
            delivered.header("content-length")
        );

        let body = delivered.json();
        assert_eq!(
            error_shape(&body),
            VALIDATION,
            "case {case:?}: expected the __type discriminator to name {VALIDATION}, got {body:?}"
        );
        assert_eq!(
            body["reason"].as_str(),
            Some(REASON_OTHER),
            "case {case:?}: expected reason {REASON_OTHER:?}, got {:?}",
            body["reason"]
        );
        assert_eq!(
            body["message"].as_str(),
            Some(expected_message),
            "case {case:?}: expected message {expected_message:?}, got {:?}",
            body["message"]
        );
    }
}
