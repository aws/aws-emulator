// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-socialmessaging-socialmessaging` service: how it claims requests and how it words the
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
//! - `override_operation` — EDITED HERE: the key and the handler stay the generated
//!   table's, but each operation is served through [`WordedRefusals`], which is where a
//!   constraint refusal is worded — see [`clause`] — and where the shape a refused
//!   PARAMETER is named with is completed — see [`error_type_locator`].
//! - `override_refusal` — how this service words a request the framework refused
//!   before any handler ran. EDITED HERE as the fallback for a refusal that does not
//!   reach [`WordedRefusals`]; see the inline note on that method.
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
//!
//! ## How this service states a constraint violation
//!
//! `1 validation error detected: <clause>` over one clause per violation, each
//! `Value '<value>' at '<member path>' failed to satisfy constraint: <requirement>`.
//! The parts this service does NOT take from the framework's own wording:
//!
//! * a `@pattern` requirement names the expression AWS reports, not the model literal:
//!   the outer `^`/`$` anchors are dropped, and an expression that is not outer-anchored
//!   is wrapped in `.*` … `.*` ([`rendered_pattern`]);
//! * the value is quoted only when the service HAS the value as text and it is short
//!   enough to quote; a blob member, and anything past [`MAX_QUOTED_VALUE`], is stated
//!   as `Value at '<member path>' …` with no value at all, never as a byte count or an
//!   element count ([`clause`]);
//! * a list-valued member is quoted as the list — `[<element>, <element>, …]`, with a
//!   structure element as `<ShapeName>(<member>=<value>, …)` — which is why the wording
//!   happens in [`WordedRefusals`], per operation, where the request can be re-read:
//!   the `override_refusal` seam is handed the violations alone. Which value a clause
//!   states is [`echo`]'s job.

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use aws_smithy_schema_server::ProtocolError;
use service_common::validation::{refusing_violations_with_echoes, SubmittedEcho};
use smithy_server_runtime::{
    http, BoxFuture, ErasedOperation, Operation, OperationContext, OperationRoutingTable, Refusal,
    Refused, Service, Violation,
};
use std::sync::Arc;

mod echo;
mod error_type_locator;

use echo::envelope;

/// The error code this service names an input-constraint violation with. It is the code
/// the framework's own refusal renderer picks, and the one the recorded responses carry;
/// the code this service answers is NOT changed here. It is restated because a chosen
/// [`Refused`] replaces the code and the message TOGETHER — a client branches on the code,
/// so one without the other is worse than neither. (The shape behind it is the
/// framework's synthetic `smithy.framework#ValidationException`, `crate::scaffolded`; the
/// operations' own modelled client-input error is `InvalidParametersException`, which a
/// HANDLER returns and which a constraint refusal is not.)
const VALIDATION_EXCEPTION: &str = "ValidationException";

/// The `com-amazonaws-socialmessaging-socialmessaging` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// The same modelled operations the generated table holds, each wrapped in
    /// [`WordedRefusals`]. Keyed by the routing key the protocol resolves — the key the
    /// generated table uses — and built from the SAME shared engine, so a wrapped
    /// operation reads and writes the one store.
    worded_refusals: OperationRoutingTable,
}

/// Wire one `crate::ops::<module>::<Operation>` per line into `table`, each wrapped in
/// [`WordedRefusals`] and keyed by its operation shape name — the routing key
/// `ServerHttpProtocol::routing_key` resolves and the generated table is keyed by.
macro_rules! wrap_operations {
    ($table:expr, $engine:expr, $($module:ident :: $operation:ident),+ $(,)?) => {
        $(
            $table.add_operation(
                stringify!($operation),
                ::std::boxed::Box::new(WordedRefusals::new(
                    crate::ops::$module::$operation::new(::std::sync::Arc::clone(&$engine)),
                )) as ::std::boxed::Box<dyn ErasedOperation>,
            );
        )+
    };
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        let mut worded_refusals = OperationRoutingTable::new();
        wrap_operations!(
            worded_refusals,
            engine,
            associate_whats_app_business_account::AssociateWhatsAppBusinessAccount,
            create_whats_app_dataset::CreateWhatsAppDataset,
            create_whats_app_flow::CreateWhatsAppFlow,
            create_whats_app_message_template::CreateWhatsAppMessageTemplate,
            create_whats_app_message_template_from_library::CreateWhatsAppMessageTemplateFromLibrary,
            create_whats_app_message_template_media::CreateWhatsAppMessageTemplateMedia,
            delete_whats_app_flow::DeleteWhatsAppFlow,
            delete_whats_app_message_media::DeleteWhatsAppMessageMedia,
            delete_whats_app_message_template::DeleteWhatsAppMessageTemplate,
            deprecate_whats_app_flow::DeprecateWhatsAppFlow,
            disassociate_whats_app_business_account::DisassociateWhatsAppBusinessAccount,
            get_linked_whats_app_business_account::GetLinkedWhatsAppBusinessAccount,
            get_linked_whats_app_business_account_phone_number::GetLinkedWhatsAppBusinessAccountPhoneNumber,
            get_whats_app_business_public_key::GetWhatsAppBusinessPublicKey,
            get_whats_app_call_permission::GetWhatsAppCallPermission,
            get_whats_app_flow::GetWhatsAppFlow,
            get_whats_app_flow_preview::GetWhatsAppFlowPreview,
            get_whats_app_message_media::GetWhatsAppMessageMedia,
            get_whats_app_message_template::GetWhatsAppMessageTemplate,
            list_linked_whats_app_business_accounts::ListLinkedWhatsAppBusinessAccounts,
            list_tags_for_resource::ListTagsForResource,
            list_whats_app_flow_assets::ListWhatsAppFlowAssets,
            list_whats_app_flows::ListWhatsAppFlows,
            list_whats_app_message_templates::ListWhatsAppMessageTemplates,
            list_whats_app_template_library::ListWhatsAppTemplateLibrary,
            post_whats_app_message_media::PostWhatsAppMessageMedia,
            publish_whats_app_flow::PublishWhatsAppFlow,
            put_whats_app_business_account_event_destinations::PutWhatsAppBusinessAccountEventDestinations,
            put_whats_app_business_public_key::PutWhatsAppBusinessPublicKey,
            send_whats_app_call_event::SendWhatsAppCallEvent,
            send_whats_app_conversion_event::SendWhatsAppConversionEvent,
            send_whats_app_message::SendWhatsAppMessage,
            tag_resource::TagResource,
            untag_resource::UntagResource,
            update_linked_whats_app_business_account_phone_number::UpdateLinkedWhatsAppBusinessAccountPhoneNumber,
            update_whats_app_flow::UpdateWhatsAppFlow,
            update_whats_app_flow_assets::UpdateWhatsAppFlowAssets,
            update_whats_app_message_template::UpdateWhatsAppMessageTemplate,
        );
        Self {
            entry: crate::ServiceDefinition::new(engine),
            worded_refusals,
        }
    }
}

/// One modelled operation, served so that a refusal of a modelled CONSTRAINT is worded
/// the way this service words one.
///
/// Nothing here re-implements a check, and no check is repeated with different words: the
/// violations come from the framework's own machinery (the protocol's deserializer and
/// `constraints::validate` over the built input) and every submitted value from
/// `service_common::validation`'s single walk over that input, both through [`echo`]; the
/// status and the error code come from the framework's refusal channel, and only the
/// SENTENCE is this service's (see [`clause`]). A request this wrapper does not refuse is
/// handed to the generated [`ErasedOperation::run`], which owns deserialize -> validate ->
/// handle -> serialize, telemetry and every modelled error.
///
/// The wrapper exists because the clause needs the REQUEST: the value a clause quotes is
/// the one the caller submitted, and for a list-valued member the framework's violation
/// carries only `<n> items`. The `override_refusal` seam is handed the violations alone,
/// so the restoring step has to happen per operation, where the input can be re-read.
/// The cost is DELIBERATE: a valid request's body is read twice, once here and once in
/// the operation's own sequence, because the framework hands no already-built input to an
/// operation.
///
/// The STRUCTURALLY right home for this is the runtime seam itself — `override_refusal`
/// taking the built input (or the request) beside the violations, so no wrapper and no
/// second read were needed. That signature lives in `vendor/`, which is read-only here, so
/// it cannot be changed from this tree; a service copying this wrapper should check whether
/// that seam has since grown the parameter rather than copying it blindly.
struct WordedRefusals<O> {
    operation: O,
}

impl<O> WordedRefusals<O> {
    /// Wrap one operation, already constructed from the shared engine.
    fn new(operation: O) -> Self {
        Self { operation }
    }
}

impl<O> ErasedOperation for WordedRefusals<O>
where
    O: Operation + ErasedOperation,
{
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r RtRequest,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        Box::pin(async move {
            // The protocol is read, used and dropped inside this one statement:
            // `&dyn ErasedServiceDefinition` is not `Sync`, so holding it across the
            // await below would make this future non-`Send`.
            let refused =
                refusing_violations_with_echoes::<O>(svc.definition().protocol(), req);
            if let Some(message) = envelope(&refused) {
                let violations: Vec<Violation> = refused
                    .into_iter()
                    .map(|(violation, _)| violation)
                    .collect();
                return svc.definition().refusal_response(
                    ProtocolError::Invalid(violations),
                    Some(Refused {
                        code: VALIDATION_EXCEPTION.to_string(),
                        message,
                    }),
                    req,
                );
            }
            let answered = self.operation.run(svc, req, ctx).await;
            // Every response of a modelled operation passes through here, which is where
            // the one thing this service completes about a response is completed: how it
            // NAMES the shape of a refused parameter ([`error_type_locator`]). The
            // definition is read AFTER the await, so nothing non-`Sync` is held across it.
            error_type_locator::answered(answered, svc.definition().identity().shape_id())
        })
    }

    /// The wrapped operation's modelled auth schemes: wrapping an operation must not
    /// change which schemes sign it.
    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        self.operation.auth_schemes()
    }
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
        // NOT a re-route: the key is the one the generated table's own protocol resolves,
        // and the operation it names is the one served — wrapped in [`WordedRefusals`] so
        // a constraint refusal is worded the way this service words one. A key the model
        // does not name resolves to `None` here exactly as it does in the generated
        // table, so the router's routing-error path is unchanged.
        let key = self.entry.protocol().routing_key(req).ok()?;
        self.worded_refusals.get(key)
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // The centralized walk calls this on a framework-level refusal (an absent
        // required member, or a value outside a declared bound) to let this service
        // reword it. [`WordedRefusals`] answers every constraint refusal of a modelled
        // operation before the walk reaches here, so this is the fallback for one that
        // does not reach it, and it states the same sentence from the same clause
        // builder — minus the value, because this seam is handed the violations without
        // the request they came from and the framework's stored echo may be its own
        // summary of the value (a byte count, an element count) rather than anything the
        // caller sent. The code stays the modelled `ValidationException` (HTTP 400),
        // which is what the framework would have answered.
        //
        // That omission is UNREACHABLE for a modelled operation today — the whole-surface
        // test in this file's `tests` module pins that every operation the model declares
        // is served behind [`WordedRefusals`], which has the request and so quotes the
        // value — and it is deliberately not guessed from the framework's stored echo:
        // that echo may be an elided prefix of caller-supplied bytes or a count summary,
        // neither of which this service states. A refusal reaching here with a value worth
        // quoting therefore means the surface test has failed first.
        //
        // An authentication refusal keeps the framework's wording: it is not an
        // input-constraint failure.
        match refusal {
            Refusal::Invalid(violations) => {
                let refused: Vec<(Violation, SubmittedEcho)> = violations
                    .iter()
                    .map(|violation| {
                        let echo = match violation.value {
                            None => SubmittedEcho::Absent,
                            Some(_) => SubmittedEcho::Unrendered,
                        };
                        (violation.clone(), echo)
                    })
                    .collect();
                Some(Refused {
                    code: VALIDATION_EXCEPTION.to_string(),
                    message: envelope(&refused)?,
                })
            }
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

#[cfg(test)]
mod tests {
    //! The one thing a wire test cannot see: that the hand-maintained `wrap_operations!`
    //! list is the WHOLE surface. How a clause is worded is `echo`'s own tests.

    use super::*;

    /// This service's Smithy model, as it ships beside this crate. Read at test time
    /// rather than embedded: it is megabytes of JSON that no non-test build needs.
    const MODEL_PATH: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/smithy-model/com-amazonaws-socialmessaging-socialmessaging.json"
    );

    /// RULE applied to the whole surface: every operation the model declares is served
    /// behind [`WordedRefusals`], under the same key the generated routing table is keyed
    /// by.
    ///
    /// The names come from the MODEL rather than from a second hand list, so an operation
    /// added to the model and not to `wrap_operations!` fails here instead of silently
    /// answering a constraint refusal in the framework's wording.
    #[test]
    fn every_modelled_operation_words_its_own_refusals() {
        let model = std::fs::read_to_string(MODEL_PATH)
            .expect("the model that generated this crate ships beside it");
        let model: serde_json::Value =
            serde_json::from_str(&model).expect("the model that generated this crate is JSON");
        let shapes = model["shapes"]
            .as_object()
            .expect("a Smithy model declares shapes");
        let operations: Vec<&str> = shapes
            .iter()
            .filter(|(_, shape)| shape["type"] == "operation")
            .filter_map(|(id, _)| id.rsplit('#').next())
            .collect();
        assert!(
            operations.len() > 10,
            "case the model's surface: expected this service's operations, got {operations:?}"
        );

        let service = EmulatedService::new(Arc::new(engine::InMemoryEngine::new()));
        for operation in operations {
            assert!(
                service.entry.routing_table().get(operation).is_some(),
                "case {operation}: the generated table keys an operation by its shape \
                 name; this one resolved to nothing, so the wrapper's key spelling is wrong"
            );
            assert!(
                service.worded_refusals.get(operation).is_some(),
                "case {operation}: every modelled operation is served behind \
                 WordedRefusals; this one is missing from wrap_operations!, so a \
                 constraint refusal of it is worded by the framework instead of by this \
                 service"
            );
        }
    }
}
