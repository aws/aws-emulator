// SPDX-License-Identifier: CC0-1.0

//! The MESSAGE a `nodejs` runtime reports for the most common uncaught `TypeError` a handler
//! raises: a property read or write on `undefined` or `null`.
//!
//! RULE: a managed `nodejs` runtime reports an error the way the JavaScript engine that
//! runtime embeds words it — a property READ on `undefined` or `null` is
//! `Cannot read properties of <undefined|null> (reading '<name>')` and a property WRITE on
//! one is `Cannot set properties of <undefined|null> (setting '<name>')`, with the property
//! name taken from the failing expression — and that one message is what the invocation's
//! error document carries (`errorMessage`), what opens the trace
//! (`<errorType>: <errorMessage>`), and what the function's log group holds for the run. An
//! error of any other class keeps its own message.
//!
//! That message is the text a client debugging a handler searches its logs for, so it is a
//! requirement rather than a detail: a search for the property that was read has to find the
//! run that failed on it.
//!
//! The rows differ along what the rule generalises over: the OPERATION (a read and a write),
//! the VALUE it was performed on (`undefined` and `null`), the PROPERTY NAME (a plain one and
//! one reached through a computed key), and an error of another class, which keeps the
//! engine's own wording.
//!
//! What the function's own LOG GROUP holds for these same runs — the reader a client
//! debugging one uses — is [`crate::node_unhandled_error_log`]'s, over these same rows.
//!
//! Every row RUNS a handler, so each takes the target's one invocation stream
//! ([`crate::invoke_request_id::one_invocation_stream_at_a_time`]).

use serde_json::Value;

use crate::invocation_log_delivery::{create_function, emulator, invoke, Created, NODE_RUNTIME};

/// A well-formed execution role ARN. No IAM is mounted here, so nothing is known about the
/// role and no log write is withheld.
pub(crate) const ROLE: &str = "arn:aws:iam::000000000000:role/service-role/lambda-exec";

/// One row: the handler's body, and the message every reader of the run must report.
///
/// `pub(crate)` because the two READERS of one raised error are two suites: the error
/// document the invocation answers (here) and the function's own log group
/// ([`crate::node_unhandled_error_log`]), and both read the same rows rather than two copies
/// of them.
pub(crate) struct Row {
    pub(crate) case: &'static str,
    /// The function's name, which is also the default log group's last segment.
    pub(crate) name: &'static str,
    /// The handler's source: a module whose `handler` raises.
    pub(crate) source: &'static str,
    /// The class of the error raised.
    pub(crate) error_type: &'static str,
    /// The message the runtime reports for it.
    pub(crate) message: &'static str,
}

pub(crate) const ROWS: &[Row] = &[
    Row {
        case: "a property read on undefined",
        name: "node-reads-undefined",
        source: "exports.handler = async (event) => {\n\
                 \x20 return event.body.connectionId;\n\
                 };\n",
        error_type: "TypeError",
        message: "Cannot read properties of undefined (reading 'connectionId')",
    },
    Row {
        case: "a property read on null",
        name: "node-reads-null",
        source: "exports.handler = async (event) => {\n\
                 \x20 const held = null;\n\
                 \x20 return held.replace('a', 'b');\n\
                 };\n",
        error_type: "TypeError",
        message: "Cannot read properties of null (reading 'replace')",
    },
    Row {
        case: "a property read through a computed key",
        name: "node-reads-computed",
        source: "exports.handler = async (event) => {\n\
                 \x20 const key = 'routeKey';\n\
                 \x20 return event.requestContext[key];\n\
                 };\n",
        error_type: "TypeError",
        message: "Cannot read properties of undefined (reading 'routeKey')",
    },
    Row {
        case: "a property write on undefined",
        name: "node-writes-undefined",
        source: "exports.handler = async (event) => {\n\
                 \x20 event.requestContext.stage = 'prod';\n\
                 \x20 return 'never';\n\
                 };\n",
        error_type: "TypeError",
        message: "Cannot set properties of undefined (setting 'stage')",
    },
    Row {
        case: "an error of another class, which keeps its own message",
        name: "node-throws-its-own",
        source: "exports.handler = async (event) => {\n\
                 \x20 throw new Error('the handler gave up');\n\
                 };\n",
        error_type: "Error",
        message: "the handler gave up",
    },
];

/// The error document an invocation that raised answers reports the runtime's own wording of
/// the error — in its `errorMessage`, in the class it names and in the line its trace opens
/// with — and so does the log the invocation itself reports.
#[tokio::test]
async fn a_handler_that_raises_reports_the_runtimes_own_wording() {
    for row in ROWS {
        let _stream = crate::invoke_request_id::one_invocation_stream_at_a_time().await;
        let case = row.case;
        let router = emulator();
        create_function(
            &router,
            &Created {
                case,
                name: row.name,
                source: row.source,
                runtime: NODE_RUNTIME,
                role: ROLE,
                log_group: None,
            },
        )
        .await;
        // The event carries nothing the handlers above read, which is what makes each of
        // them raise.
        let (reported, payload) = invoke(&router, case, row.name).await;

        assert_eq!(
            payload.get("errorType").and_then(Value::as_str),
            Some(row.error_type),
            "case {case:?}: expected the error document to name {expected:?}, got {payload}",
            expected = row.error_type
        );
        assert_eq!(
            payload.get("errorMessage").and_then(Value::as_str),
            Some(row.message),
            "case {case:?}: expected the error document to carry {expected:?}, got {payload}",
            expected = row.message
        );
        let head = format!("{}: {}", row.error_type, row.message);
        assert_eq!(
            payload
                .get("trace")
                .and_then(Value::as_array)
                .and_then(|trace| trace.first())
                .and_then(Value::as_str),
            Some(head.as_str()),
            "case {case:?}: expected the trace to open with {head:?}, got {payload}"
        );
        assert!(
            reported.contains(row.message),
            "case {case:?}: expected the invocation's own log to carry {expected:?}, got \
             {reported:?}",
            expected = row.message
        );
    }
}
