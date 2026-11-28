// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-marketplaceagreement-awsmpcommerceservice-v20200301` service: how it claims requests and how it words the
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
//!
//! ## How a refused request is stated
//!
//! This service models its own `ValidationException`, and that shape carries
//! `reason` (a `ValidationExceptionReason`) and `fields` (entries of
//! `{name, message}`) beside `message`. The framework validates every modelled
//! constraint — `DescribeAgreementInput$agreementId` has a pattern and a length,
//! `MaxResults` a range, `FilterList` a length, … — BEFORE any handler runs, and
//! renders a violation in the protocol's generic refusal body, which carries
//! `message` alone: `Service::override_refusal` chooses only a code and a
//! message, so the modelled members cannot be stated through it at all.
//!
//! [`ModelledValidation`] is therefore interposed in front of EVERY modelled
//! operation: it reads the violations the boundary walk would find and, when
//! there are any, answers them as this service's own `ValidationException`
//! through [`crate::state::validation_exception_for`] — the same rendering the
//! handlers' own refusals use, so one refused member reads alike wherever it was
//! found. A request with no violation is handed to the generated walk unchanged.
//!
//! ## The authorization layer every operation runs under
//!
//! AWS decides authorization at the IAM layer, in FRONT of the service: a caller
//! whose identity-based policies do not allow the action it asked for is refused
//! before the request's own input is looked at and before anything is read or
//! written. So that decision is taken for EVERY operation of this service, in
//! [`Authorized`] — which fronts [`ModelledValidation`] in the one table
//! [`Service::override_operation`] serves this service's operations through — and
//! nowhere else. [`authorization`] holds the store lookups and the sentence.
//!
//! ## What every call publishes once it has been answered
//!
//! The entitlements an accepted agreement grants are resources of two OTHER
//! services — the Marketplace Entitlement Service holds the product entitlement
//! a seller reads, and License Manager the licence an entitlement names — so
//! they are published there rather than only reported from here
//! ([`crate::state::granted_entitlements`]). That publication follows the
//! agreements' status and the clock, which an acceptance, an amendment, a
//! cancellation, a replacement, an expiry and a renewal all move, so it is made
//! in [`Authorized`] after every call this service answers instead of from each
//! handler: one place, which no handler can forget, and which the clock-owed
//! transitions a reporting operation materialises reach as well.

use smithy_server_runtime::{
    http, BoxFuture, DeserializableStruct, ErasedOperation, Operation, Refusal, Refused, Service,
    Violation,
};
use std::collections::HashMap;
use std::marker::PhantomData;
use std::sync::Arc;

/// Whether IAM lets a request to this service perform the action it names: the
/// decision [`Authorized`] takes in front of every operation.
pub mod authorization;

/// The wire error code (`__type`) and HTTP status this service's modelled
/// `ValidationException` carries (`smithy_query(query="shape",
/// shape="ValidationException")`: `errorKind=client`, `httpError=400`).
const VALIDATION_EXCEPTION_CODE: &str = "ValidationException";
/// See [`VALIDATION_EXCEPTION_CODE`].
const VALIDATION_EXCEPTION_STATUS: u16 = 400;

/// Serves one modelled operation, stating a refused input in this service's own
/// modelled `ValidationException` instead of the framework's generic refusal
/// body.
///
/// WHY this is a seam and not a line in each handler: the boundary walk refuses a
/// request BEFORE `handle` runs, and the only seam it offers
/// (`Service::override_refusal` -> `Refused`) carries a code and a message —
/// there is nowhere in it to put the modelled `reason` and `fields`. So the
/// violations are read here, where they are still structured, and rendered
/// through [`crate::state::validation_exception_for`].
///
/// Generic over the OPERATION, so the same wrapper serves every one of them and
/// nothing about a particular operation's input is spelled out here: the schema
/// it validates against and the routing key it delegates under are read off the
/// operation's own `impl Operation`.
struct ModelledValidation<Op> {
    /// The operation this wrapper fronts. `fn() -> Op` rather than `Op`, so the
    /// wrapper is `Send + Sync` for the operation's sake alone and owns nothing.
    operation: PhantomData<fn() -> Op>,
}

impl<Op: Operation> ModelledValidation<Op> {
    const fn new() -> Self {
        Self {
            operation: PhantomData,
        }
    }

    /// The violations in `req`, read the way the framework's own walk reads them:
    /// deserialize the input against the operation's schema, then validate the
    /// built value against it. `None` means the request carries no structured
    /// refusal — either it is valid, or it failed in a way that has no
    /// member-level form (a body the codec cannot read at all), which the
    /// generated walk renders itself.
    fn violations(
        protocol: &dyn smithy_server_runtime::ServerHttpProtocol,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<Vec<Violation>> {
        let schema = Op::INPUT_SCHEMA;
        let error = match protocol.deserialize_input(req, schema) {
            Ok(mut deser) => match <Op::Input as DeserializableStruct>::deserialize(&mut *deser) {
                // A built input: every modelled constraint, in one walk.
                Ok(input) => {
                    let violations =
                        aws_smithy_schema_server::constraints::validate(&input, schema);
                    return (!violations.is_empty()).then_some(violations);
                }
                // Absence is found while READING the request, so it arrives as a
                // codec error; `refusal_of` gives it the same structured form a
                // constraint violation has.
                Err(e) => aws_smithy_schema_server::ProtocolError::Serde(e),
            },
            Err(e) => e,
        };
        match smithy_server_runtime::refusal_of(&error) {
            Some(Refusal::Invalid(violations)) if !violations.is_empty() => Some(violations),
            _ => None,
        }
    }
}

impl<Op: Operation> ErasedOperation for ModelledValidation<Op> {
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r aws_smithy_runtime_api::http::Request,
        ctx: &'r smithy_server_runtime::OperationContext<'r>,
    ) -> BoxFuture<'r, aws_smithy_runtime_api::http::Response> {
        Box::pin(async move {
            let protocol = svc.definition().protocol();
            if let Some(violations) = Self::violations(protocol, req) {
                let refused = crate::state::validation_exception_for(&violations);
                return protocol
                    .serialize_error(
                        &refused,
                        &crate::ops::types::SCHEMA_VALIDATION_EXCEPTION,
                        VALIDATION_EXCEPTION_CODE,
                        VALIDATION_EXCEPTION_STATUS,
                    )
                    .unwrap_or_else(|e| svc.definition().framework_error_response(e, req));
            }
            // Nothing to restate: run the generated deserialize -> validate ->
            // handle -> serialize walk. Reached through the routing table rather
            // than through `override_operation`, which would route straight back
            // here. The walk reads and validates the body again — the price of
            // answering a refusal in the modelled shape, paid once per request on
            // a body the transport has already bounded.
            match svc.definition().routing_table().get(Op::NAME) {
                Some(op) => op.run(svc, req, ctx).await,
                None => svc.definition().routing_error_response(req),
            }
        })
    }
}

/// One modelled operation, served behind the decision AWS takes in front of a
/// handler: IAM's authorization of the action.
///
/// It carries a handle on the ONE shared engine, which is what the decision reads
/// the caller's identity and its policies through ([`authorization`]), and the
/// [`ModelledValidation`] front the request is handed to once IAM allows it — so
/// an authorized request takes exactly the path it took before this layer
/// existed, and a refused one reaches neither the boundary walk nor the handler.
///
/// WHY it is a layer and not a line in each handler: the decision is a property
/// of the request's identity and the action it names rather than of what the
/// handler does, so taking it once in front of every operation is what keeps any
/// one handler from forgetting it — and it must be taken BEFORE the request's
/// own input is read, which is where this wrapper sits.
///
/// The same engine is what the entitlements this service's agreements grant are
/// published through once the request has been answered
/// ([`crate::state::granted_entitlements::publish`]), for the reason the module
/// docs give: that publication is a property of the agreements' status rather
/// than of any one operation.
struct Authorized<Op, Eng> {
    /// The validation front this wrapper hands an authorized request to. A
    /// zero-sized value naming the operation type, like the one it fronts.
    validated: ModelledValidation<Op>,
    /// The shared engine, so the principal's policies are read from the one store
    /// every service of this emulator shares.
    engine: Arc<Eng>,
}

impl<Op: Operation, Eng> Authorized<Op, Eng> {
    fn new(engine: Arc<Eng>) -> Self {
        Self {
            validated: ModelledValidation::new(),
            engine,
        }
    }
}

impl<Op: Operation, Eng: Send + Sync + 'static> ErasedOperation for Authorized<Op, Eng> {
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r aws_smithy_runtime_api::http::Request,
        ctx: &'r smithy_server_runtime::OperationContext<'r>,
    ) -> BoxFuture<'r, aws_smithy_runtime_api::http::Response> {
        Box::pin(async move {
            // IAM's decision, taken before anything of the request's own input is
            // read: the action is this operation's own name under this service's
            // prefix. A refused request reaches neither the boundary walk nor the
            // handler, so it reports nothing about what the account holds and
            // changes nothing.
            if let Err(denial) = authorization::authorize(&*self.engine, ctx, Op::NAME) {
                return authorization::refusal_response(svc.definition().protocol(), &denial);
            }
            let answered = self.validated.run(svc, req, ctx).await;
            // The entitlements the agreements of this store now grant, published
            // to the services that OWN them: the Marketplace Entitlement Service
            // holds the product entitlement a seller reads, and License Manager
            // the licence an entitlement names
            // (`crate::state::granted_entitlements`). It is done HERE, once per
            // call, rather than in each handler, because those grants follow the
            // agreements' status and the clock: an acceptance grants them, an
            // amendment restates them, and a cancellation, a replacement, an
            // expiry or a renewal — the last two owed by the clock and
            // materialised by whichever call next settles the chain — takes them
            // away. Reconciling after every call is what keeps any one of those
            // paths from forgetting, and a call that moved neither the store nor
            // the clock past something due publishes nothing at all: the grants
            // are derived, so there is nothing to restate until one of them
            // changes (`crate::state::granted_entitlements::GrantSummary`).
            //
            // It is synchronous store work, so when a publication is OWED it
            // runs on the BLOCKING pool rather than on the async worker this
            // request is served on — which keeps this task yielding for its
            // whole life — and it is awaited there, so a caller holding this
            // call's answer already sees what it published. A call that owes
            // none pays only for reading that summary, and a publication that
            // panicked or was cancelled leaves the records to the next call and
            // this answer untouched.
            let now = ::std::time::SystemTime::now();
            if crate::state::granted_entitlements::publication_due(&*self.engine, now) {
                let engine = Arc::clone(&self.engine);
                let _published = ::tokio::task::spawn_blocking(move || {
                    crate::state::granted_entitlements::publish(&*engine, now);
                })
                .await;
            }
            answered
        })
    }

    /// The schemes [`ModelledValidation`] answers with, which is the framework's
    /// default for a wrapper and exactly what this service answered before this
    /// layer existed: a request is signed (or not) as the MODEL says for the
    /// operation it names, which the generated operation behind these wrappers
    /// decides, so standing in front of it changes nothing about its signing.
    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        self.validated.auth_schemes()
    }
}

/// One [`Authorized`] (fronting one [`ModelledValidation`]) per modelled
/// operation, keyed by the routing key the model names that operation by.
///
/// EVERY operation is wrapped, for both layers' own reason:
/// `ValidationException` and `AccessDeniedException` are service-common errors
/// (`smithy_query(query="digest")` lists both under `common_errors`), so a
/// refused input is stated in the modelled envelope and an unauthorized caller is
/// refused whichever operation the request named — not only the ones whose
/// refusals happen to have been observed.
macro_rules! modelled_validation_table {
    ($($module:ident :: $operation:ident),* $(,)?) => {
        fn modelled_validation_table<Eng: Send + Sync + 'static>(
            engine: &Arc<Eng>,
        ) -> HashMap<&'static str, Box<dyn ErasedOperation>>
        {
            let mut table: HashMap<&'static str, Box<dyn ErasedOperation>> = HashMap::new();
            $(
                table.insert(
                    <crate::ops::$module::$operation<Eng> as Operation>::NAME,
                    Box::new(Authorized::<crate::ops::$module::$operation<Eng>, Eng>::new(
                        Arc::clone(engine),
                    )),
                );
            )*
            table
        }
    };
}

modelled_validation_table!(
    accept_agreement_approval::AcceptAgreementApproval,
    accept_agreement_cancellation_request::AcceptAgreementCancellationRequest,
    accept_agreement_payment_request::AcceptAgreementPaymentRequest,
    accept_agreement_request::AcceptAgreementRequest,
    batch_create_billing_adjustment_request::BatchCreateBillingAdjustmentRequest,
    cancel_agreement::CancelAgreement,
    cancel_agreement_cancellation_request::CancelAgreementCancellationRequest,
    cancel_agreement_payment_request::CancelAgreementPaymentRequest,
    cancel_agreement_request::CancelAgreementRequest,
    create_agreement_request::CreateAgreementRequest,
    describe_agreement::DescribeAgreement,
    get_agreement_approval::GetAgreementApproval,
    get_agreement_cancellation_request::GetAgreementCancellationRequest,
    get_agreement_contacts::GetAgreementContacts,
    get_agreement_entitlements::GetAgreementEntitlements,
    get_agreement_payment_request::GetAgreementPaymentRequest,
    get_agreement_request::GetAgreementRequest,
    get_agreement_request_approval_plans::GetAgreementRequestApprovalPlans,
    get_agreement_request_terms::GetAgreementRequestTerms,
    get_agreement_terms::GetAgreementTerms,
    get_billing_adjustment_request::GetBillingAdjustmentRequest,
    list_agreement_approvals::ListAgreementApprovals,
    list_agreement_cancellation_requests::ListAgreementCancellationRequests,
    list_agreement_charges::ListAgreementCharges,
    list_agreement_invoice_line_items::ListAgreementInvoiceLineItems,
    list_agreement_payment_requests::ListAgreementPaymentRequests,
    list_agreement_requests::ListAgreementRequests,
    list_billing_adjustment_requests::ListBillingAdjustmentRequests,
    reject_agreement_approval::RejectAgreementApproval,
    reject_agreement_cancellation_request::RejectAgreementCancellationRequest,
    reject_agreement_payment_request::RejectAgreementPaymentRequest,
    search_agreements::SearchAgreements,
    send_agreement_cancellation_request::SendAgreementCancellationRequest,
    send_agreement_payment_request::SendAgreementPaymentRequest,
    start_agreement_approvals::StartAgreementApprovals,
    update_purchase_orders::UpdatePurchaseOrders,
);

/// The `com-amazonaws-marketplaceagreement-awsmpcommerceservice-v20200301` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// Takes IAM's decision and then states a refused input in this service's own
    /// modelled `ValidationException`, one entry per modelled operation; see
    /// [`Authorized`] and [`ModelledValidation`].
    modelled_validation: HashMap<&'static str, Box<dyn ErasedOperation>>,
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self {
            modelled_validation: modelled_validation_table::<Eng>(&engine),
            entry: crate::ServiceDefinition::new(engine),
        }
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
        // This service signs as `aws-marketplace` — a signing name it SHARES with
        // every other AWS Marketplace-family service (Catalog, Reporting, Deployment,
        // Discovery, Entitlement, Metering), each serving its own endpoint
        // (`agreement-marketplace` here). The router's routing key comes from the
        // SigV4 credential scope first, so all of them resolve to `aws-marketplace`
        // and matching on the signing name ALONE makes every one of them claim every
        // request — which the router answers `500 AmbiguousService`.
        //
        // So the claim is narrowed to this service's own modelled wire surface, by the
        // one rule `service-common` states for every service under a shared signing
        // name. This model is `awsJson1_0` + `rpcv2Cbor`, both RPC protocols: they post
        // every operation to one path (`/`, or
        // `/service/<Service>/operation/<Operation>`) and name the target service out
        // of band. Those paths are shared with its RPC peers (the Entitlement Service,
        // Metering) rather than owned, so `claims_request` claims a request only when
        // the service it NAMES is this one — `X-Amz-Target:
        // AWSMPCommerceService_v20200301.<OperationName>` on awsJson1_0
        // (`OPERATIONS.md`), or the `{Service}` label of an rpcv2Cbor path.
        service_common::claims_request(
            parts,
            self.entry.identity().signing_name(),
            crate::ROUTE_PATH_SEGMENTS,
            Some(self.entry.identity().shape_id()),
        )
    }

    fn override_operation(
        &self,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<&dyn ErasedOperation> {
        // Every modelled operation is served through `Authorized` and the
        // `ModelledValidation` it fronts, so a caller IAM does not allow is refused
        // before anything is read, and a refused input is stated in this service's
        // own modelled `ValidationException` — with the modelled `reason` and
        // `fields` — rather than in the framework's generic refusal body. See those
        // types for why neither can be done from inside `handle`. The operation
        // names come from the model (each operation's own `Operation::NAME`), and a
        // request naming no modelled operation is left to the generated routing
        // table, which refuses it.
        let routing_key = self.definition().protocol().routing_key(req).ok()?;
        self.modelled_validation
            .get(routing_key)
            .map(|wrapped| wrapped.as_ref())
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // Nothing to reword here: a refusal of a modelled constraint never reaches
        // this seam, because `ModelledValidation` answers it BEFORE the framework
        // renders it. The words are not the problem — the generic refusal body
        // cannot carry the modelled `reason`/`fields` members at all, and
        // `Refused` is only a code and a message.
        let _ = refusal;
        None
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
