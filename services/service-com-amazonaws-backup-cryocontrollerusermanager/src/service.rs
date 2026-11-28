// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-backup-cryocontrollerusermanager` service: how it claims requests and how it words the
//! failures the framework hands back.
//!
//! EDIT THIS FILE. It is emitted once with working defaults and is never
//! regenerated, so a choice made here survives adding another service or
//! re-running the generator.
//!
//! `EmulatedService` is what the router mounts (`EmulatedService::new(engine)`).
//! It wraps the generated [`ServiceDefinition`], which carries the model-derived half
//! of this service — its identity, its wire protocol, and its operation routing
//! table — built from the Smithy model at compile time. You do not edit the
//! entry; you edit the behavioral seams below.
//!
//! Five opt-in seams are yours to change; every one defaults to the framework's
//! own behaviour, so an unedited service still routes and answers correctly:
//!
//! - `can_handle` — which requests this service claims. The default matches the
//!   request's resolved route key against this service's signing name. Edit it
//!   for anything the model does not express: a signing name shared by several
//!   services, or routing that keys on the access-key id.
//! - `override_operation` — serve a request the model's routing key cannot name
//!   correctly. The default returns `None`, so every request resolves through the
//!   generated routing table. Return `Some(op)` to route one yourself.
//! - `override_refusal` — how this service words a request the framework refused
//!   before any handler ran. The default returns `None`, keeping the framework's
//!   wording. See the inline note on that method for how to reword.
//! - `override_routing_error` — how this service words a request that reached it
//!   but named no modeled operation. The default returns `None`, so the entry
//!   renders the protocol-correct default (query -> 400 InvalidAction, awsJson ->
//!   404, …). Return `Some(resp)` to answer in your own envelope if desired.
//! - `override_framework_error` — how this service words a transport-level
//!   failure the orchestrator detects up front (e.g. a malformed `aws-chunked`
//!   body). The default returns `None`, keeping the entry's protocol-correct
//!   rendering. Return `Some(resp)` to reword it if desired.
//!
//! The model-derived half of this service — its identity, wire protocol, routing
//! table, and the default error renderers — lives in the generated `ServiceDefinition`
//! and is reached, not edited, through the one `definition` method below. You
//! do not write it; editing the entry would let this file drift from the model.

use std::collections::HashMap;
use std::sync::Arc;

use smithy_server_runtime::{http, ErasedOperation, Operation, Refusal, Refused, Service};

pub mod pre_handler;

/// The `com-amazonaws-backup-cryocontrollerusermanager` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// One [`pre_handler::PreHandlerGate`] per operation whose refusal of a bad request AWS
    /// Backup decides AHEAD of the framework's deserialize-and-validate boundary, keyed by
    /// the routing key the model names that operation by ([`Operation::NAME`], the same key
    /// the generated routing table uses). Built once here, never per request; each entry
    /// holds a routing key and a decision rather than an operation or a store of its own.
    gates: HashMap<&'static str, Box<dyn ErasedOperation>>,
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self {
            gates: pre_handler_gates::<Eng>(),
            entry: crate::ServiceDefinition::new(engine),
        }
    }
}

/// The pre-boundary gate of each operation that has one, keyed by the routing key the model
/// names that operation by.
///
/// ONE list, read from the operations' own [`Operation::NAME`] and [`Operation::INPUT_SCHEMA`]
/// so neither can drift from the model, and each entry pairs an operation with the decision
/// that operation's own file states ([`pre_handler`] for why a gate rather than a reworded
/// refusal):
///
/// * `StartBackupJob`, whose request naming no role to perform the backup under is an
///   AUTHORIZATION refusal and not a parameter one;
/// * `StartReportJob`, whose malformed `ReportPlanName` is refused with the service's own
///   sentence stating the grammar a plan name has;
/// * `StartScanJob`, which is fronted by AWS's request validator, so its boundary refusals
///   carry that validator's envelope and error type.
///
/// Every other operation resolves through the generated routing table, and so does a gated
/// one whose request the decision passes on — the gate delegates to it rather than serving
/// the request itself.
fn pre_handler_gates<Eng: Send + Sync + 'static>() -> HashMap<&'static str, Box<dyn ErasedOperation>>
{
    let mut gates: HashMap<&'static str, Box<dyn ErasedOperation>> = HashMap::new();
    macro_rules! gate {
        ($($module:ident :: $operation:ident),+ $(,)?) => {
            $({
                gates.insert(
                    <crate::ops::$module::$operation<Eng> as Operation>::NAME,
                    Box::new(pre_handler::PreHandlerGate::<
                        crate::ops::$module::$operation<Eng>,
                    >::new(
                        crate::ops::$module::refused_at_the_boundary,
                    )) as Box<dyn ErasedOperation>,
                );
            })+
        };
    }
    gate!(
        start_backup_job::StartBackupJob,
        start_report_job::StartReportJob,
        start_scan_job::StartScanJob,
    );
    gates
}

impl<Eng: Send + Sync + 'static> Service for EmulatedService<Eng> {
    fn definition(&self) -> &dyn smithy_server_runtime::ErasedServiceDefinition {
        // The model-derived half of this service — identity, protocol, routing
        // table, and the default error renderers — lives in the generated entry.
        // The router reads it through this one accessor. Do not edit the entry.
        &self.entry
    }

    fn can_handle(&self, parts: &http::request::Parts) -> bool {
        // Claim a request whose resolved route key is this service's signing
        // name. Edit this for anything the model does not express — a signing
        // name shared by several services, or access-key-id-based routing.
        match smithy_server_runtime::resolve_route_key(parts) {
            Some(key) => key == self.entry.identity().signing_name(),
            None => false,
        }
    }

    fn override_operation(
        &self,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<&dyn ErasedOperation> {
        // Serve an operation whose refusal of a bad request AWS Backup decides AHEAD
        // of the framework's deserialize-and-validate boundary through that
        // operation's gate, so the decision is made where the real service makes it
        // (see [`pre_handler`]). Every other operation resolves through the generated
        // routing table, and so does a gated one whose request its decision passes
        // on — the gate delegates to it rather than serving the request itself.
        //
        // A request that names no modelled operation has nothing to decide: the
        // routing failure is the answer it gets.
        let key = self.entry.protocol().routing_key(req).ok()?;
        self.gates.get(key).map(|gate| &**gate)
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // AWS Backup does not answer a refused request with the framework's
        // generic `ValidationException`: every operation in this service models
        // `MissingParameterValueException` for a required member the request
        // left out, and `InvalidParameterValueException` for a value that is
        // present but outside what the model admits (the `BackupVaultName`
        // pattern, a `MaxResults` outside 1..1000, an enum value the model does
        // not list). Both the code and the message are replaced, so a client
        // branching on the code sees the code this service's own errors carry.
        //
        // The operations this seam cannot answer for do not reach it: their
        // refusal is decided ahead of the boundary instead (see [`pre_handler`]),
        // because this seam is handed the violations ALONE — no caller and no
        // operation — and cannot move the status off the 400 a constraint refusal
        // carries or name the `Message` member this service's own shapes use.
        match refusal {
            Refusal::Invalid(violations) if !violations.is_empty() => {
                let absent = violations.iter().all(|violation| {
                    violation.kind == aws_smithy_schema_server::ViolationKind::MissingRequiredMember
                });
                let code = if absent {
                    "MissingParameterValueException"
                } else {
                    "InvalidParameterValueException"
                };
                let message = violations
                    .iter()
                    .map(aws_smithy_schema_server::Violation::message)
                    .collect::<Vec<_>>()
                    .join("; ");
                Some(Refused {
                    code: code.to_string(),
                    message,
                })
            }
            // Anything else (an authentication refusal, or a refusal with no
            // violation to name) keeps the framework's wording.
            _ => None,
        }
    }

    fn override_routing_error(
        &self,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<aws_smithy_runtime_api::http::Response> {
        // The router calls this when a request reached this service but named no
        // modeled operation — neither `override_operation` nor the generated
        // routing table resolved it. Returning `None` renders the protocol-correct
        // default for this service's envelope (query -> 400 InvalidAction text/xml,
        // awsJson -> 404, rpcv2Cbor -> 404, rest -> 501). Return `Some(resp)` to
        // word that routing failure in your own envelope instead.
        let _ = req;
        None
    }

    fn override_framework_error(
        &self,
        error: &aws_smithy_schema_server::ProtocolError,
    ) -> Option<aws_smithy_runtime_api::http::Response> {
        // The router calls this on a transport-level failure it detects up front —
        // before (or instead of) naming an operation — such as a malformed
        // `aws-chunked` body. Returning `None` keeps the entry's protocol-correct
        // rendering. Return `Some(resp)` to reword that failure in your own
        // envelope. The error is borrowed, so the default renderer can still
        // consume it on `None`.
        let _ = error;
        None
    }
}
