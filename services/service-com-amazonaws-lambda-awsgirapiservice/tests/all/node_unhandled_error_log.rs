// SPDX-License-Identifier: CC0-1.0

//! What a node handler's UNCAUGHT error leaves in the function's own CloudWatch Logs group —
//! the reader a client debugging a failed invocation uses.
//!
//! RULE: an invocation whose handler raised delivers its whole run log to the function's log
//! group, and the error report in it carries the SAME message the invocation's error document
//! carries ([`crate::node_handler_error_wording`] owns that wording), inside the run's own
//! `START`/`END`/`REPORT` framing. So a search of the group for the property a handler read,
//! or for the message of the error it threw, finds the run that failed on it.
//!
//! The rows are [`crate::node_handler_error_wording::ROWS`] — the same raised errors, read
//! here through CloudWatch Logs' own readers rather than through the invocation's answer, so
//! nothing that did not reach that store can pass.
//!
//! Every row RUNS a handler, so each takes the target's one invocation stream
//! ([`crate::invoke_request_id::one_invocation_stream_at_a_time`]).

use crate::invocation_log_delivery::{
    create_function, delivered, emulator, invoke, streams, Created, NODE_RUNTIME,
};
use crate::node_handler_error_wording::{ROLE, ROWS};

/// The framing every run's log opens and closes with, whatever the handler did.
const FRAMING: [&str; 3] = ["START RequestId: ", "END RequestId: ", "REPORT RequestId: "];

/// A run whose handler raised leaves its error report in the function's group, framed by the
/// run's own lines — which is what a client searching the group for the failure finds.
#[tokio::test]
async fn the_group_holds_the_error_report_of_a_run_that_raised() {
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
        let (reported, _payload) = invoke(&router, case, row.name).await;

        let group = format!("/aws/lambda/{}", row.name);
        let held = streams(&router, case, &group).await;
        assert_eq!(
            held.len(),
            1,
            "case {case:?}: expected one stream for the invocation, got {held:?}"
        );
        let (lines, rebuilt) = delivered(&router, case, &group, &held[0]).await;
        assert_eq!(
            rebuilt, reported,
            "case {case:?}: expected the group to hold the log the invocation reported, got \
             {rebuilt:?} against {reported:?}"
        );
        assert!(
            lines.iter().any(|line| line.contains(row.message)),
            "case {case:?}: expected the error's own message {expected:?} in the group \
             {group:?}, got {lines:?}",
            expected = row.message
        );
        assert!(
            lines.iter().any(|line| line.contains(row.error_type)),
            "case {case:?}: expected the error's class {expected:?} in the group {group:?}, \
             got {lines:?}",
            expected = row.error_type
        );
        for framing in FRAMING {
            assert!(
                lines.iter().any(|line| line.starts_with(framing)),
                "case {case:?}: expected the run's {framing:?} line in the group, got {lines:?}"
            );
        }
    }
}
