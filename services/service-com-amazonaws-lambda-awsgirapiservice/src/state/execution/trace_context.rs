// SPDX-License-Identifier: CC0-1.0

//! The X-RAY TRACE CONTEXT one invocation runs with: what the handler reads its own trace
//! context out of, and where that context comes from.
//!
//! The execution environment reports it in `_X_AMZN_TRACE_ID`, one of the variables the
//! Lambda runtime sets itself and will not let a function's configuration set
//! ([`super::super::RESERVED_ENVIRONMENT_KEYS`]) — "the X-Ray tracing header", which
//! "changes with each invocation", so it is a value of the INVOCATION rather than of the
//! function.
//!
//! What it carries is decided by the function's own `TracingConfig` mode and by what the
//! caller supplied, and the three fields are `service_common`'s one propagation
//! ([`::service_common::xray_trace::hop_tracing_header`]) rather than this service's own:
//!
//! * `Active` — "Set Mode to Active to sample and trace a subset of incoming requests with
//!   X-Ray" (`TracingConfig$Mode`): Lambda records a segment for the invocation, so every
//!   invocation of such a function HAS a trace context. The chain is the one the caller
//!   named when it named one in X-Ray's own form and one Lambda starts otherwise, the
//!   `Parent` is the span of the segment Lambda records for this invocation — which is what
//!   the handler's own subsegments hang from — and the sampling decision is the caller's
//!   where it took one, this function's own (it samples) where it did not.
//! * `PassThrough` — the other value of the mode enum, and the one "a function gets when
//!   the caller named none" ([`super::super::DEFAULT_TRACING_MODE`]): Lambda records
//!   nothing of its own, so it has no segment to name as a parent and nothing to decide. A
//!   caller's tracing header is passed on as written, and an invocation that carried none
//!   runs under a chain Lambda started for it, with the decision `0` — so the variable is
//!   always reported, and the chain of an invocation nobody traced is still recognisable as
//!   one chain across the calls it makes ([`super::super::recursion`]).
//!
//! Nothing is kept here: the context is computed per invocation from the request and the
//! resolved configuration, and travels on the [`Invocation`](super::Invocation) the run is
//! performed from.
//!
//! # What an invocation X-Ray records leaves behind
//!
//! A recorded invocation produces TWO segment documents ([`segment_documents`]), which is
//! the shape X-Ray reports a Lambda invocation in: the SERVICE segment, whose `origin` is
//! `AWS::Lambda` and which covers Lambda's own handling of the invocation, and the FUNCTION
//! segment, whose `origin` is `AWS::Lambda::Function` and which covers the run itself. The
//! function segment's parent is the service segment, the service segment's parent is the
//! span the CALLER named (and nothing, for an invocation that started the chain), and the
//! function segment's own id is the `Parent` the run reports — so the subsegments an
//! instrumented handler sends to the daemon ([`super::xray_daemon`]) nest inside it.
//!
//! They are submitted through the one AWS SDK seam a run has ([`super::aws_api`]), which is
//! also the seam the handler's own datagrams are submitted over, so a trace's documents all
//! arrive in the store `BatchGetTraces` answers from.

use std::time::SystemTime;

use ::serde_json::json;

/// The request header a caller's tracing header arrives on, as X-Ray spells it. The ONE
/// spelling, shared with the recursive-loop guardrail's reading of the same header
/// ([`crate::state::recursion::TRACE_HEADER`]).
pub use crate::state::recursion::TRACE_HEADER;

/// The environment variable the execution environment reports this invocation's trace
/// context in: "`_X_AMZN_TRACE_ID` – The X-Ray tracing header", which the Lambda
/// Developer Guide lists among the variables the runtime sets and a function may not.
pub const TRACE_ID_VARIABLE: &str = "_X_AMZN_TRACE_ID";

/// The `origin` of the segment covering LAMBDA's own handling of an invocation.
const SERVICE_ORIGIN: &str = "AWS::Lambda";

/// The `origin` of the segment covering the FUNCTION's own run.
const FUNCTION_ORIGIN: &str = "AWS::Lambda::Function";

/// The X-Ray trace context one invocation runs with.
#[derive(Debug, Clone)]
pub struct InvocationTrace {
    /// What the run reports in [`TRACE_ID_VARIABLE`].
    pub header: String,
    /// The spans of the two segments X-Ray RECORDS for this invocation, or `None` for an
    /// invocation it records nothing for: a function that traces nothing of its own, and a
    /// request whose sampling decision says it is not recorded.
    pub recorded: Option<Recorded>,
}

/// The spans of the segments a recorded invocation produces.
#[derive(Debug, Clone)]
pub struct Recorded {
    /// The chain the segments are filed under.
    pub trace_id: String,
    /// The span of the [`SERVICE_ORIGIN`] segment.
    pub service_span: String,
    /// The span of the [`FUNCTION_ORIGIN`] segment, which is the `Parent` the run reports
    /// and so the span the handler's own subsegments hang from.
    pub function_span: String,
    /// The span the CALLER named as the parent of this invocation, where it named one.
    pub caller_span: Option<String>,
}

/// The tracing header the caller of this invocation supplied, or `None` where it supplied
/// none.
#[must_use]
pub fn supplied_trace_header(ctx: &smithy_server_runtime::OperationContext<'_>) -> Option<String> {
    ctx.parts()
        .headers
        .get(TRACE_HEADER)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string)
}

/// The trace context ONE invocation runs with — see the module docs for the rule each
/// `TracingConfig` mode follows.
///
/// Every invocation HAS one: `_X_AMZN_TRACE_ID` is a variable the runtime reports for each
/// invocation, so a function that records nothing of its own and a caller that named no chain
/// still run inside one, unsampled and unrecorded.
#[must_use]
pub fn invocation_trace_context(
    supplied: Option<&str>,
    tracing_mode: &str,
    at: SystemTime,
) -> InvocationTrace {
    let records = tracing_mode == crate::state::TRACING_MODE_ACTIVE;
    // A function that records nothing of its own has no span to name as a parent, so it
    // passes on what it was given; one that records has a span for the run itself.
    let function_span = records.then(::service_common::xray_trace::mint_span_id);
    let header = ::service_common::xray_trace::onward_tracing_header(
        supplied,
        function_span.as_deref(),
        at,
    )
    // A function that records nothing of its own, called by a client that named no chain —
    // or named one in no form X-Ray reads — still RUNS inside a chain: Lambda reports
    // `_X_AMZN_TRACE_ID` for every invocation, so the chain is Lambda's own where the caller
    // named none, with the span of its own handling as the parent and the decision `0`
    // because nothing of this invocation is recorded. It is what makes the hops of a chain
    // this emulator itself started recognisable as one chain, which is what the
    // recursive-loop guardrail counts ([`crate::state::recursion`]).
    .unwrap_or_else(|| {
        ::service_common::xray_trace::stamped_tracing_header(
            &::service_common::xray_trace::mint_trace_id(at),
            &::service_common::xray_trace::mint_span_id(),
            false,
        )
    });
    // A chain is RECORDED only where the decision says so: `Sampled=0` is an invocation
    // whose work still happens and whose trace is not recorded.
    let sampled = ::service_common::xray_trace::sampling_decision(&header).unwrap_or(false);
    let trace_id = ::service_common::xray_trace::continued_root(&header).map(str::to_string);
    let recorded = match (sampled.then_some(function_span).flatten(), trace_id) {
        (Some(function_span), Some(trace_id)) => Some(Recorded {
            trace_id,
            service_span: ::service_common::xray_trace::mint_span_id(),
            function_span,
            caller_span: supplied
                .and_then(::service_common::xray_trace::parent_span)
                .map(str::to_string),
        }),
        _ => None,
    };
    InvocationTrace { header, recorded }
}

/// The two segment documents a recorded invocation produces, in the order a reader sees
/// them: Lambda's own handling of the invocation, then the function's run inside it.
///
/// Every value is read off the invocation: the chain its own header named, the function it
/// ran, the instants the run began and ended, and whether it faulted.
#[must_use]
pub fn segment_documents(
    recorded: &Recorded,
    function_name: &str,
    started: SystemTime,
    ended: SystemTime,
    faulted: bool,
) -> Vec<String> {
    let from = ::service_common::xray_segments::epoch_seconds(started);
    let to = ::service_common::xray_segments::epoch_seconds(ended);
    let mut service = json!({
        "trace_id": recorded.trace_id,
        "id": recorded.service_span,
        "name": function_name,
        "start_time": from,
        "end_time": to,
        "origin": SERVICE_ORIGIN,
    });
    if let Some(caller_span) = recorded.caller_span.as_deref() {
        if let Some(object) = service.as_object_mut() {
            object.insert("parent_id".to_string(), json!(caller_span));
        }
    }
    let function = json!({
        "trace_id": recorded.trace_id,
        "id": recorded.function_span,
        "parent_id": recorded.service_span,
        "name": function_name,
        "start_time": from,
        "end_time": to,
        "origin": FUNCTION_ORIGIN,
        // "fault" is what X-Ray reads a server-side failure of the recorded work off, which
        // for a run is the handler having raised.
        "fault": faulted,
    });
    vec![service.to_string(), function.to_string()]
}

/// The service token X-Ray is reached under through a run's own SDK seam
/// ([`super::aws_api`]), which is that service's SDK id as a client names it.
const XRAY_NAMESPACE: &str = "xray";

/// The operation a producer submits its segments with: "Uploads segment documents to Amazon
/// Web Services X-Ray" (`PutTraceSegments`).
const PUT_TRACE_SEGMENTS: &str = "PutTraceSegments";

/// How long the submission of a run's own segments is given, as the delivery of its log is:
/// a bound on an in-process hop made off the answer, so a service that does not answer cannot
/// hold an invocation open.
const CALL_CEILING: std::time::Duration = std::time::Duration::from_secs(5);

/// Submit `documents` as the segments X-Ray recorded for this invocation.
///
/// Made over the run's own SDK seam ([`super::aws_api`]), which is the seam the handler's own
/// datagrams are submitted over, so every document of one trace arrives in the store
/// `BatchGetTraces` answers from. Reporting a trace is never part of answering the
/// invocation: a submission nothing answered, or one X-Ray refused, is traced where an
/// undelivered log is and the caller is answered either way.
pub async fn report_segments(
    aws: &super::aws_api::AwsAccess,
    region: &str,
    documents: Vec<String>,
) {
    if documents.is_empty() {
        return;
    }
    let body = json!({ "TraceSegmentDocuments": documents }).to_string();
    match aws
        .call_within(XRAY_NAMESPACE, PUT_TRACE_SEGMENTS, region, &body, CALL_CEILING)
        .await
    {
        Ok(answer) if answer.status < 300 => {}
        Ok(answer) => ::tracing::debug!(
            status = answer.status,
            error_type = answer.error_type.as_deref().unwrap_or_default(),
            "the segments this invocation produced were not accepted"
        ),
        Err(refusal) => ::tracing::debug!(
            refusal = ?refusal,
            "the segments this invocation produced were not submitted"
        ),
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, UNIX_EPOCH};

    use super::{invocation_trace_context, segment_documents};

    /// Which trace context an invocation runs with — over a TABLE whose rows differ along
    /// the function's `TracingConfig` mode and what the caller supplied (a sampled chain, an
    /// unsampled one, a chain with no decision, a malformed root, and no header at all).
    #[test]
    fn an_invocations_trace_context_follows_the_mode_and_the_caller() {
        let root = "1-5759e988-bd862e3fe1be46a994272793";
        let at = UNIX_EPOCH + Duration::from_secs(0x5759_e988);
        // What the invocation's `_X_AMZN_TRACE_ID` must be.
        enum Expected {
            /// Lambda STAMPS its own: this chain (`None` for one Lambda started), its own
            /// span as the parent, and this decision.
            Stamped(Option<&'static str>, &'static str),
            /// The caller's header, passed on as written.
            Verbatim,
        }
        let rows: Vec<(&str, &str, Option<String>, Expected)> = vec![
            (
                "an active function called with a sampled chain",
                "Active",
                Some(format!("Root={root};Sampled=1")),
                Expected::Stamped(Some(root), "1"),
            ),
            (
                "an active function called with an unsampled chain",
                "Active",
                Some(format!("Root={root};Sampled=0")),
                Expected::Stamped(Some(root), "0"),
            ),
            (
                "an active function called with a chain and no decision",
                "Active",
                Some(format!("Root={root}")),
                Expected::Stamped(Some(root), "1"),
            ),
            (
                "an active function called with a root that is not a trace id",
                "Active",
                Some("Root=not-a-trace-id;Sampled=1".to_string()),
                Expected::Stamped(None, "1"),
            ),
            (
                "an active function called with no header at all",
                "Active",
                None,
                Expected::Stamped(None, "1"),
            ),
            (
                "a pass-through function called with a sampled chain",
                "PassThrough",
                Some(format!("Root={root};Sampled=1")),
                Expected::Verbatim,
            ),
            (
                "a pass-through function called with no header at all",
                "PassThrough",
                None,
                Expected::Stamped(None, "0"),
            ),
            (
                "a pass-through function called with a root that is not a trace id",
                "PassThrough",
                Some("Root=not-a-trace-id;Sampled=1".to_string()),
                Expected::Stamped(None, "0"),
            ),
        ];
        for (case, mode, supplied, expected) in rows {
            let context = invocation_trace_context(supplied.as_deref(), mode, at);
            // The segments an invocation produces follow the same decision: an invocation
            // Lambda records produces two, and one it does not produces none.
            let recorded = context.recorded.as_ref();
            let header = context.header.clone();
            match expected {
                Expected::Verbatim => {
                    assert_eq!(
                        Some(header.as_str()),
                        supplied.as_deref(),
                        "case {case:?}: expected the caller's header passed on as written, got \
                         {header:?}"
                    );
                    assert!(
                        recorded.is_none(),
                        "case {case:?}: a function that traces nothing records no segment"
                    );
                    continue;
                }
                Expected::Stamped(chain, decision) => {
                    let context = header;
                    let named = ::service_common::xray_trace::trace_field(&context, "Root")
                        .unwrap_or_default();
                    assert!(
                        ::service_common::xray_trace::is_trace_id(named),
                        "case {case:?}: expected a trace id as the Root of {context:?}, got \
                         {named:?}"
                    );
                    if let Some(continued) = chain {
                        assert_eq!(
                            named, continued,
                            "case {case:?}: expected {context:?} to continue {continued:?}, got \
                             {named:?}"
                        );
                    }
                    let parent = ::service_common::xray_trace::trace_field(&context, "Parent")
                        .unwrap_or_default();
                    assert!(
                        ::service_common::xray_trace::is_span_id(parent),
                        "case {case:?}: expected a span id as the Parent of {context:?}, got \
                         {parent:?}"
                    );
                    assert_eq!(
                        ::service_common::xray_trace::trace_field(&context, "Sampled"),
                        Some(decision),
                        "case {case:?}: expected the decision {decision:?} in {context:?}"
                    );
                    // Only a SAMPLED invocation is recorded, and the function segment's own
                    // span is the `Parent` the run reports, so the handler's subsegments
                    // nest inside it.
                    assert_eq!(
                        recorded.is_some(),
                        decision == "1",
                        "case {case:?}: expected the invocation to be recorded={}, got \
                         {recorded:?}",
                        decision == "1"
                    );
                    if let Some(recorded) = recorded {
                        assert_eq!(
                            recorded.function_span, parent,
                            "case {case:?}: expected the function segment to be the span the \
                             run reports, got {recorded:?}"
                        );
                        assert_eq!(
                            recorded.trace_id, named,
                            "case {case:?}: expected the segments filed under the chain the \
                             header named, got {recorded:?}"
                        );
                        let documents = segment_documents(
                            recorded,
                            "probe",
                            at,
                            at + Duration::from_millis(25),
                            false,
                        );
                        let origins: Vec<String> = documents
                            .iter()
                            .filter_map(|text| {
                                ::serde_json::from_str::<::serde_json::Value>(text).ok()
                            })
                            .filter_map(|document| {
                                document["origin"].as_str().map(str::to_string)
                            })
                            .collect();
                        assert_eq!(
                            origins,
                            vec!["AWS::Lambda".to_string(), "AWS::Lambda::Function".to_string()],
                            "case {case:?}: expected Lambda's two segments, got {documents:?}"
                        );
                    }
                }
            }
        }
    }
}
