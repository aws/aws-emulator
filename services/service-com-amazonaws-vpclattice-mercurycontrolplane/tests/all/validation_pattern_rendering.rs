// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for how a `ValidationException` QUOTES the `@pattern` a submitted
//! value broke.
//!
//! Every case drives the emulator the way a client does — a restJson1 request through the
//! router that mounts the service — and reads the refusal off the wire, so nothing here
//! can pass on a canned body.
//!
//! The rule under test, asserted over a TABLE spanning the pattern-constrained members of
//! several operations rather than one row: the requirement a constraint violation states
//! for a `@pattern` quotes the expression WITHOUT the whole-value anchors the model writes
//! around it — the leading `^` and the trailing `$`, and only when both ends carry one —
//! while the match itself is still made against the model's pattern as written, so a value
//! that merely CONTAINS a match is still refused.
//!
//! The rows differ along what that rule generalises over: which operation and which
//! member carries the constraint, where the member is bound (a URI label or the request
//! body), whether the member is refused before any handler runs or by the handler's own
//! check, and the FORM of the modelled expression — a plain anchored pattern, an anchored
//! alternation, an alternation whose own branches carry anchors of their own (which keeps
//! them), and a pattern the model writes with no anchors at all (which is quoted
//! verbatim).

use crate::vpclattice_service_slice::{message_of, Lattice};

/// `ResourceArn`'s modelled `@pattern`, which `GetResourcePolicy.resourceArn` targets.
///
/// SOURCE: `smithy_query(query="shape",shape="ResourceArn")`. Written out here rather
/// than read from the crate, so each row pins the wire text against the MODEL and not
/// against whatever the code happens to hold.
const RESOURCE_ARN_PATTERN: &str = r"arn(:[a-z0-9]+([.-][a-z0-9]+)*){2}(:([a-z0-9]+([.-][a-z0-9]+)*)?){2}:((servicenetwork/sn)|(service/svc)|(resourceconfiguration/rcfg))-[0-9a-z]{17}";

/// `Arn`'s modelled `@pattern`, which `ListTagsForResource.resourceArn` targets.
/// SOURCE: `smithy_query(query="shape",shape="Arn")`, anchors dropped.
const TAGGABLE_ARN_PATTERN: &str =
    r"arn:[a-z0-9][-.a-z0-9]{0,62}:vpc-lattice:([a-z0-9][-.a-z0-9]{0,62})?:\d{12}?:[^/].{0,1023}";

/// `ServiceNetworkIdentifier`'s modelled `@pattern`: an anchored ALTERNATION, whose
/// branches carry no anchor of their own, so the outer pair is all that is dropped.
/// SOURCE: `smithy_query(query="shape",shape="ServiceNetworkIdentifier")`.
const SERVICE_NETWORK_IDENTIFIER_PATTERN: &str =
    r"((sn-[0-9a-z]{17})|(arn:[a-z0-9\-]+:vpc-lattice:[a-zA-Z0-9\-]+:\d{12}:servicenetwork/sn-[0-9a-z]{17}))";

/// `ListenerIdentifier`'s modelled `@pattern`: an alternation one of whose branches
/// carries an anchored expression of its OWN, which stays — only the pair at the two ends
/// of the model's text is dropped.
/// SOURCE: `smithy_query(query="shape",shape="ListenerIdentifier")`.
const LISTENER_IDENTIFIER_PATTERN: &str = r"((listener-[0-9a-z]{17})|(^arn:[a-z0-9\-]+:vpc-lattice:[a-zA-Z0-9\-]+:\d{12}:service/svc-[0-9a-z]{17}/listener/listener-[0-9a-z]{17}$))";

/// `ClientToken`'s modelled `@pattern`, which the model writes with NO anchors: there is
/// no pair to drop, so it is quoted exactly as the model declares it.
/// SOURCE: `smithy_query(query="shape",shape="ClientToken")`.
const CLIENT_TOKEN_PATTERN: &str = "[!-~]+";

/// `ServiceName`'s modelled `@pattern`, which `CreateService.name` targets — the one
/// pattern class this service has to check inside the handler, because the boundary's
/// regex engine cannot compile the look-ahead it is written with.
/// SOURCE: `smithy_query(query="shape",shape="ServiceName")`, anchors dropped.
const SERVICE_NAME_PATTERN: &str = r"(?!svc-)(?![-])(?!.*[-]$)(?!.*[-]{2})[a-z0-9-]+";

/// A well-formed service identifier of the modelled shape: the prefix plus 17 characters
/// of the modelled alphabet. It satisfies `ServiceIdentifier`'s own `@pattern`, so a row
/// that refuses ANOTHER label of the same request reports exactly one violation.
fn well_formed_service_id() -> String {
    format!("svc-{}", "0".repeat(17))
}

/// The whole message AWS answers for one refused member: the envelope over the clause,
/// with the requirement quoting `pattern`.
fn expected_message(member: &str, value: &str, pattern: &str) -> String {
    format!(
        "1 validation error detected: Value '{value}' at '{member}' failed to satisfy \
         constraint: Member must satisfy regular expression pattern: {pattern}"
    )
}

/// One row of the table: what is sent, and the message the refusal has to carry.
struct Row {
    case: &'static str,
    method: &'static str,
    path: String,
    body: Option<serde_json::Value>,
    member: &'static str,
    value: String,
    pattern: &'static str,
}

#[tokio::test]
async fn a_pattern_requirement_quotes_the_expression_without_its_whole_value_anchors() {
    let lattice = Lattice::new();
    let service_id = well_formed_service_id();

    let rows: Vec<Row> = vec![
        Row {
            case: "a resource-policy ARN sent as the bare resource id",
            method: "GET",
            path: format!("/resourcepolicy/sn-{}", "0".repeat(17)),
            body: None,
            member: "resourceArn",
            value: format!("sn-{}", "0".repeat(17)),
            pattern: RESOURCE_ARN_PATTERN,
        },
        Row {
            case: "a resource-policy ARN of a kind the member does not model",
            method: "DELETE",
            path: format!("/resourcepolicy/arn%3Aaws%3As3%3A%3A%3Abucket-{}", "0".repeat(17)),
            body: None,
            member: "resourceArn",
            value: format!("arn:aws:s3:::bucket-{}", "0".repeat(17)),
            pattern: RESOURCE_ARN_PATTERN,
        },
        Row {
            case: "a tagging ARN of another vendor",
            method: "GET",
            path: "/tags/arn%3Aaws%3As3%3A%3A%3Abucket".to_string(),
            body: None,
            member: "resourceArn",
            value: "arn:aws:s3:::bucket".to_string(),
            pattern: TAGGABLE_ARN_PATTERN,
        },
        Row {
            case: "a service-network identifier outside the modelled alphabet",
            method: "GET",
            path: "/servicenetworks/sn-not-an-identifier".to_string(),
            body: None,
            member: "serviceNetworkIdentifier",
            value: "sn-not-an-identifier".to_string(),
            pattern: SERVICE_NETWORK_IDENTIFIER_PATTERN,
        },
        Row {
            // The match is still ANCHORED: this value CONTAINS a well-formed identifier,
            // so a validator that had dropped the anchors from the check itself — and not
            // only from the message — would have accepted it.
            case: "a well-formed service-network identifier with a suffix after it",
            method: "GET",
            path: format!("/servicenetworks/sn-{}-extra", "0".repeat(17)),
            body: None,
            member: "serviceNetworkIdentifier",
            value: format!("sn-{}-extra", "0".repeat(17)),
            pattern: SERVICE_NETWORK_IDENTIFIER_PATTERN,
        },
        Row {
            case: "a listener identifier whose suffix is too short",
            method: "GET",
            path: format!("/services/{service_id}/listeners/listener-{}", "0".repeat(11)),
            body: None,
            member: "listenerIdentifier",
            value: format!("listener-{}", "0".repeat(11)),
            pattern: LISTENER_IDENTIFIER_PATTERN,
        },
        Row {
            case: "a client token of a character the pattern does not admit",
            method: "POST",
            path: "/servicenetworks".to_string(),
            body: Some(serde_json::json!({ "name": "token-net", "clientToken": " " })),
            member: "clientToken",
            value: " ".to_string(),
            pattern: CLIENT_TOKEN_PATTERN,
        },
        Row {
            // The handler's own check of a pattern the boundary cannot compile reports the
            // expression the same way.
            case: "a service name the modelled look-ahead forbids",
            method: "POST",
            path: "/services".to_string(),
            body: Some(serde_json::json!({ "name": "Upper-Case" })),
            member: "name",
            value: "Upper-Case".to_string(),
            pattern: SERVICE_NAME_PATTERN,
        },
    ];

    for row in rows {
        let case = row.case;
        let answer = lattice.call(row.method, &row.path, row.body).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected 400, got {} / {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code.as_deref(),
            Some("ValidationException"),
            "case {case:?}: expected the ValidationException wire code, got {:?} / {}",
            answer.error_code,
            answer.body
        );
        let message = message_of(case, &answer);
        assert_eq!(
            message,
            expected_message(row.member, &row.value, row.pattern),
            "case {case:?}: wrong validation message, got {message:?}"
        );
        assert!(
            !message.contains("pattern: ^"),
            "case {case:?}: the quoted expression must carry no leading anchor, got \
             {message:?}"
        );
        assert!(
            !message.ends_with('$'),
            "case {case:?}: the quoted expression must carry no trailing anchor, got \
             {message:?}"
        );
    }
}

#[tokio::test]
async fn a_value_the_modelled_pattern_admits_is_not_refused() {
    let lattice = Lattice::new();

    // Each row is a value its member's `@pattern` DOES admit, so the refusals above come
    // from the pattern and not from a check that refuses every value: none of them names a
    // persisted resource, so each is answered as a not-found rather than as a constraint
    // violation.
    let rows: Vec<(&str, String)> = vec![
        (
            "a well-formed service-network id",
            format!("/servicenetworks/sn-{}", "0".repeat(17)),
        ),
        (
            "a well-formed service-network ARN",
            format!(
                "/servicenetworks/arn%3Aaws%3Avpc-lattice%3Aus-west-2%3A123456789012%3Aservicenetwork%2Fsn-{}",
                "0".repeat(17)
            ),
        ),
        (
            "a well-formed resource-policy ARN",
            format!(
                "/resourcepolicy/arn%3Aaws%3Avpc-lattice%3Aus-west-2%3A123456789012%3Aservicenetwork%2Fsn-{}",
                "0".repeat(17)
            ),
        ),
    ];

    for (case, path) in rows {
        let answer = lattice.call("GET", &path, None).await;
        assert_eq!(
            answer.status, 404,
            "case {case:?}: a value the pattern admits is resolved, not refused: got {} / {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code.as_deref(),
            Some("ResourceNotFoundException"),
            "case {case:?}: expected the not-found wire code, got {:?} / {}",
            answer.error_code,
            answer.body
        );
    }
}
