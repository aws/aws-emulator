// SPDX-License-Identifier: CC0-1.0

//! The FORM each service names the request a response answered in, served by the ASSEMBLED
//! emulator.
//!
//! RULE: a response names the request it answered where its own wire protocol reports that
//! id — the `x-amzn-RequestId` response header for a JSON or REST protocol, the document's
//! own `<RequestId>` element for a query protocol — freshly per HTTP response, on a success
//! exactly as on a failure, and the id is spelled in the form of the SERVICE that answered:
//! an AWS request id is opaque to a client, so each service is free to spell it its own way,
//! and the emulator mints the form of the service rather than one form for all of them.
//! Two services' forms are driven here: the hyphenated lowercase 8-4-4-4-12 UUID most AWS
//! services report, and DynamoDB's single unbroken run of uppercase letters and digits.
//!
//! Why it lives here rather than in any one service crate: the difference between the forms
//! is only observable where more than one service answers. A per-service suite mounts its
//! OWN service alone, so none of them can see a second service's responses carrying the
//! same form as its own — which is exactly what one shared minting function for every
//! service looks like from a client. This suite drives the assembled router (the one
//! `build_router` produces and the one a client talks to) and compares the three services'
//! answers with each other.
//!
//! The rows span what the rule generalises over: three services, three wire protocols
//! (`awsQuery`, `awsJson1_0`, `restJson1`), two channels the id is reported on (the header
//! and the document's own element), a success and a failure for each service, and two
//! deliveries of every row — so a form keyed to one protocol, one status or one channel, and
//! an id that is not minted afresh, each fail a row.
//!
//! What the rule does not change, and this suite pins too: the status each call answers with
//! stays what that operation answers, and an id is never empty and never the all-zeros
//! placeholder a client reads as "no id".
//!
//! Assembling the whole router costs stack (see `boot_serve.rs`), so it is built — and the
//! requests driven — on a sized thread.

use std::collections::BTreeSet;
use std::sync::Arc;

use aws_emulator_router::{build_router, InMemoryEngine};
use serde_json::json;
use smithy_server_runtime::MultiServiceRouter;

// The awsQuery wire helpers this test crate states ONCE: the form-body percent-encoder and
// the element reader. Called rather than written again, as that module's header requires.
use crate::query_wire::{form_encode, opt_element};

/// Keep this at or above `boot_serve.rs`'s stack: the same assembly runs here.
const BOOT_STACK_SIZE: usize = 64 * 1024 * 1024;

/// The Region every call below is signed for.
const REGION: &str = "us-west-2";

/// The header AWS reports a response's request id in, written out here rather than read from
/// any crate so the assertion does not merely compare the implementation with itself. HTTP
/// header names are case-insensitive, which is how a client reads it back.
const REQUEST_ID_HEADER: &str = "x-amzn-requestid";

/// The element a query protocol's own document reports the same id in — inside
/// `<ResponseMetadata>` on a success, as a child of `<ErrorResponse>` on a failure.
const REQUEST_ID_ELEMENT: &str = "RequestId";

/// The id a client reads as "no id at all": the all-zeros UUID. No response may report it,
/// whichever form that service's ids take.
const PLACEHOLDER_ID: &str = "00000000-0000-0000-0000-000000000000";

/// How a service spells the id it names a request by.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Form {
    /// The hyphenated 8-4-4-4-12 run of lowercase hex digits (36 characters) that
    /// CloudFormation's and Lambda's recorded request ids carry — the form most AWS services
    /// report.
    Uuid,
    /// One unbroken run of uppercase letters and digits, no hyphen and no lowercase
    /// character, as every sample response's `x-amzn-RequestId` in the DynamoDB API
    /// reference shows. The width of one recorded token is fixed; the band asserted here is
    /// the range such a token is observed in, so the assertion pins the character class and
    /// the length band rather than one service's current width.
    UppercaseToken,
}

/// The inclusive length band an [`Form::UppercaseToken`] id is spelled at: read off the
/// recorded `x-amzn-RequestId` of DynamoDB responses, which are long fixed-width tokens
/// rather than the 36 characters of a UUID.
const UPPERCASE_TOKEN_WIDTHS: std::ops::RangeInclusive<usize> = 40..=64;

impl Form {
    /// Whether `id` is spelled in this form. Written out here, not imported from the service
    /// that mints it, so the assertion is against the form a client parses and not against
    /// the implementation.
    fn admits(self, id: &str) -> bool {
        match self {
            Self::Uuid => {
                let widths: Vec<usize> = id.split('-').map(str::len).collect();
                widths == vec![8, 4, 4, 4, 12]
                    && id
                        .chars()
                        .all(|c| c == '-' || (c.is_ascii_hexdigit() && !c.is_ascii_uppercase()))
            }
            Self::UppercaseToken => {
                UPPERCASE_TOKEN_WIDTHS.contains(&id.chars().count())
                    && id
                        .chars()
                        .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
            }
        }
    }

    /// What this form is, for a report that has to say what was expected.
    fn described(self) -> &'static str {
        match self {
            Self::Uuid => "a hyphenated lowercase 8-4-4-4-12 UUID",
            Self::UppercaseToken => "one unbroken run of 40-64 uppercase letters and digits",
        }
    }
}

/// Where a protocol reports the id, which is where a client reads it from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Channel {
    /// The response header alone — a protocol with no envelope element for the id
    /// (`awsJson1_0`, `restJson1`).
    Header,
    /// The document's own `<RequestId>` element, which is what a query protocol's SDK
    /// surfaces as the call's response metadata.
    Element,
}

/// One call to the assembled router.
struct Call<'a> {
    /// What this row is, for the assertion reports.
    case: &'a str,
    signing_name: &'a str,
    method: &'a str,
    path: &'a str,
    content_type: &'a str,
    /// `Some` for an RPC protocol that names its operation in `X-Amz-Target`.
    target: Option<&'a str>,
    body: String,
    /// The status this operation answers with, so a row that stopped reaching the operation
    /// fails rather than silently asserting the form of a refusal's id.
    status: u16,
    /// The form the SERVICE that answers spells its ids in.
    form: Form,
    /// Where that service's protocol reports the id.
    channel: Channel,
}

/// One awsQuery call, form-encoded as an AWS SDK sends it.
fn query(
    case: &'static str,
    signing_name: &'static str,
    version: &str,
    action: &str,
    params: &[(&str, &str)],
    status: u16,
) -> Call<'static> {
    let mut body = format!("Action={action}&Version={version}");
    for (name, value) in params {
        body.push('&');
        body.push_str(&form_encode(name));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    Call {
        case,
        signing_name,
        method: "POST",
        path: "/",
        content_type: "application/x-www-form-urlencoded",
        target: None,
        body,
        status,
        // CloudFormation's recorded request ids are UUIDs, and a query protocol states the
        // id in its own document.
        form: Form::Uuid,
        channel: Channel::Element,
    }
}

/// What one delivery answered: its status, the id it named the request by on the channel its
/// protocol reports it, the id its header carried (whichever protocol), and its body.
struct Answer {
    status: u16,
    reported: Option<String>,
    header: Option<String>,
    body: String,
}

/// Dispatch one call, signed for its own service, and read back what it reported.
fn deliver(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    call: &Call<'_>,
) -> Answer {
    let host = format!("{}.{REGION}.amazonaws.com", call.signing_name);
    let mut builder = http::Request::builder()
        .method(call.method)
        .uri(format!("https://{host}{}", call.path))
        .header("host", host.clone())
        .header("content-type", call.content_type)
        .header(
            // The signature is never checked: this router runs with authentication off, as
            // every suite in this crate runs it. What the scope carries is the Region and
            // the service the request routes to.
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTEST/20260101/{REGION}/{}/aws4_request, \
                 SignedHeaders=host, Signature=not-verified",
                call.signing_name
            ),
        );
    if let Some(target) = call.target {
        builder = builder.header("x-amz-target", target);
    }
    let request = builder
        .body(bytes::Bytes::from(call.body.clone()))
        .expect("a well-formed request");
    let response = runtime.block_on(router.dispatch(request));
    let (parts, body) = response.into_parts();
    let header = parts
        .headers
        .get(REQUEST_ID_HEADER)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let body = String::from_utf8_lossy(&body).to_string();
    let reported = match call.channel {
        Channel::Header => header.clone(),
        Channel::Element => opt_element(&body, REQUEST_ID_ELEMENT),
    };
    Answer {
        status: parts.status.as_u16(),
        reported,
        header,
        body,
    }
}

/// Assert one delivery named its request the way its service does, and return that id.
fn assert_names_its_request(call: &Call<'_>, delivery: usize, answer: &Answer) -> String {
    let case = call.case;
    assert_eq!(
        answer.status, call.status,
        "case {case:?} delivery {delivery}: expected HTTP {}, got {} with {}",
        call.status, answer.status, answer.body
    );
    let id = answer.reported.clone().unwrap_or_else(|| {
        panic!(
            "case {case:?} delivery {delivery}: every response names the request it answered \
             on {:?}, got none in {} with headers reporting {:?}",
            call.channel, answer.body, answer.header
        )
    });
    assert!(
        !id.is_empty() && id != PLACEHOLDER_ID,
        "case {case:?} delivery {delivery}: expected a real id, got {id:?}"
    );
    assert!(
        call.form.admits(&id),
        "case {case:?} delivery {delivery}: expected {}, got {id:?}",
        call.form.described()
    );
    // A client correlating on the header and one reading the document must see ONE value, so
    // a protocol that reports both reports the same id in both.
    if let (Channel::Element, Some(header)) = (call.channel, answer.header.as_deref()) {
        assert_eq!(
            header, id,
            "case {case:?} delivery {delivery}: the header and the document must name one \
             request, got {header:?} and {id:?}"
        );
    }
    id
}

/// The six surfaces the rule is driven over: a success and a failure of each service, each
/// an operation whose answer depends on nothing provisioned first.
fn rows() -> Vec<Call<'static>> {
    vec![
        query(
            "cloudformation, awsQuery, a success",
            "cloudformation",
            "2010-05-15",
            "ListStacks",
            &[],
            200,
        ),
        query(
            "cloudformation, awsQuery, a failure",
            "cloudformation",
            "2010-05-15",
            "DescribeStacks",
            &[("StackName", "request-id-no-such-stack")],
            400,
        ),
        Call {
            case: "dynamodb, awsJson1_0, a success",
            signing_name: "dynamodb",
            method: "POST",
            path: "/",
            content_type: "application/x-amz-json-1.0",
            target: Some("DynamoDB_20120810.ListTables"),
            body: json!({"Limit": 1}).to_string(),
            status: 200,
            form: Form::UppercaseToken,
            channel: Channel::Header,
        },
        Call {
            case: "dynamodb, awsJson1_0, a failure",
            signing_name: "dynamodb",
            method: "POST",
            path: "/",
            content_type: "application/x-amz-json-1.0",
            target: Some("DynamoDB_20120810.DescribeTable"),
            body: json!({"TableName": "request-id-no-such-table"}).to_string(),
            status: 400,
            form: Form::UppercaseToken,
            channel: Channel::Header,
        },
        Call {
            case: "lambda, restJson1, a success",
            signing_name: "lambda",
            method: "GET",
            path: "/2015-03-31/functions?MaxItems=1",
            content_type: "application/json",
            target: None,
            body: String::new(),
            status: 200,
            form: Form::Uuid,
            channel: Channel::Header,
        },
        Call {
            case: "lambda, restJson1, a failure",
            signing_name: "lambda",
            method: "GET",
            path: "/2015-03-31/functions/request-id-no-such-function",
            content_type: "application/json",
            target: None,
            body: String::new(),
            status: 404,
            form: Form::Uuid,
            channel: Channel::Header,
        },
    ]
}

#[test]
fn each_service_names_a_request_in_its_own_form_and_never_twice() {
    std::thread::Builder::new()
        .name("per-service-request-id-form".to_string())
        .stack_size(BOOT_STACK_SIZE)
        .spawn(|| {
            let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
            let mut router = build_router(engine);
            router.set_request_authentication(false);
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_time()
                .build()
                .expect("a current-thread runtime");

            let rows = rows();
            // Two deliveries of every row: the id is minted per RESPONSE, so the second
            // delivery of one call is answered with an id of its own.
            let mut named: BTreeSet<String> = BTreeSet::new();
            for delivery in 0..2 {
                for call in &rows {
                    let answer = deliver(&router, &runtime, call);
                    let id = assert_names_its_request(call, delivery, &answer);
                    assert!(
                        named.insert(id.clone()),
                        "case {:?} delivery {delivery}: every response names its OWN request, \
                         and {id} was already named",
                        call.case
                    );
                }
            }
            assert_eq!(
                named.len(),
                rows.len() * 2,
                "expected one id per response over {} deliveries, got {named:?}",
                rows.len() * 2
            );

            // The forms really are DIFFERENT between the services, which is the whole point
            // of the rule: no id either service minted is admitted by the other's form.
            let uuid_ids: Vec<&String> =
                named.iter().filter(|id| Form::Uuid.admits(id)).collect();
            let token_ids: Vec<&String> = named
                .iter()
                .filter(|id| Form::UppercaseToken.admits(id))
                .collect();
            assert_eq!(
                uuid_ids.len(),
                8,
                "case the two UUID-form services: expected one UUID per CloudFormation and \
                 Lambda response, got {uuid_ids:?}"
            );
            assert_eq!(
                token_ids.len(),
                4,
                "case the uppercase-token service: expected one token per DynamoDB response, \
                 got {token_ids:?}"
            );
            for id in &uuid_ids {
                assert!(
                    !Form::UppercaseToken.admits(id),
                    "case the two forms are distinguishable: {id} is admitted by both"
                );
            }
        })
        .expect("spawn the per-service request-id thread")
        .join()
        .expect("every service must name a request in its own form");
}
