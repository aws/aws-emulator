// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-billing-awsbilling` service: how it claims requests and how it words the
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

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use aws_smithy_schema_server::ProtocolError;
use smithy_server_runtime::{
    http, BoxFuture, DeserializableStruct, ErasedOperation, Operation, OperationContext,
    PreDispatchProtocol, Refusal, Refused, ServerHttpProtocol, Service, Violation, ViolationKind,
};
use std::sync::Arc;

use crate::ops::types::{ValidationException, ValidationExceptionField};

/// The error code this service answers an input-constraint violation with: the
/// modeled `ValidationException` (`@httpError 400`), which every operation of
/// this service declares. Restated here because `override_refusal` replaces the
/// code and the message together.
const VALIDATION_EXCEPTION: &str = "ValidationException";

/// The status the modeled `ValidationException` carries (`@httpError 400`).
const VALIDATION_STATUS: u16 = 400;

/// How a request REFUSED before the operation ran is rendered: the JSON
/// envelope, at the protocol version the model declares.
///
/// A refusal is answered by the request-validation layer in front of the
/// operation rather than by the endpoint that serves it, and the recorded wire
/// shows the two announcing different versions: a refusal keeps
/// `application/x-amz-json-1.0`, while a request this service ACCEPTED is
/// answered in the codec [`Service::override_protocol`] selects.
const PRE_DISPATCH: PreDispatchProtocol<'static> = PreDispatchProtocol::Json {
    content_type: "application/x-amz-json-1.0",
    errortype_header: false,
};

/// Render one constraint violation the way this service states it: the member
/// path and the violated constraint, and NOTHING of the value that was
/// rejected.
///
/// Returns `None` for a violation class this service has no wording for, which
/// lets [`constraint_envelope`] decline the whole refusal and keep the
/// framework's wording rather than invent a sentence for it ([`ViolationKind`]
/// is `#[non_exhaustive]`, so a class can be added upstream).
fn constraint_clause(violation: &Violation) -> Option<String> {
    let requirement = constraint_requirement(violation)?;
    // An absent member has no submitted value at all, so it is reported as the
    // null it was: that is a fact of the request's shape, not a rendering of a
    // value the caller sent. Every other violation names the member only —
    // several of this service's constrained members are `@sensitive`
    // (`BillingViewName`, `BillingViewDescription`), and none of them belongs in
    // a 400 body.
    let subject = match &violation.kind {
        ViolationKind::MissingRequiredMember => "Value null",
        _ => "Value",
    };
    Some(format!(
        "{subject} at '{}' failed to satisfy constraint: {requirement}",
        violation.path
    ))
}

/// The requirement one violation states — the half of a clause that describes
/// the rule, with nothing of the member or the value in it.
///
/// Shared by the message envelope ([`constraint_clause`]) and the
/// `ValidationExceptionField` entry ([`constraint_field`]), so one violation
/// cannot be described two ways in one response.
fn constraint_requirement(violation: &Violation) -> Option<String> {
    let requirement = match &violation.kind {
        ViolationKind::MissingRequiredMember => "Member must not be null".to_string(),
        ViolationKind::MinLength(min) => {
            format!("Member must have length greater than or equal to {min}")
        }
        ViolationKind::MaxLength(max) => {
            format!("Member must have length less than or equal to {max}")
        }
        ViolationKind::MinValue(min) => {
            format!("Member must have value greater than or equal to {min}")
        }
        ViolationKind::MaxValue(max) => {
            format!("Member must have value less than or equal to {max}")
        }
        // The requirement reported is the expression itself, without the `^` /
        // `$` the model spells it with: the service states the pattern a value
        // must satisfy, not the anchoring the model uses to make the match
        // total, so a client that re-validates locally with the pattern it read
        // out of the message uses the one AWS documents.
        ViolationKind::Pattern(pattern) => format!(
            "Member must satisfy regular expression pattern: {}",
            unanchored(pattern)
        ),
        ViolationKind::EnumValue(allowed) => {
            format!("Member must satisfy enum value set: [{}]", allowed.join(", "))
        }
        ViolationKind::UniqueItems => "Member must have unique values".to_string(),
        _ => return None,
    };
    Some(requirement)
}

/// A `@pattern` expression without the anchors a model may spell it with. A `$`
/// escaped by a backslash is a literal dollar sign inside the expression, not
/// the end anchor, so it stays.
fn unanchored(pattern: &str) -> &str {
    let body = pattern.strip_prefix('^').unwrap_or(pattern);
    match body.strip_suffix('$') {
        Some(trimmed) => {
            let backslashes = trimmed.len() - trimmed.trim_end_matches('\\').len();
            if backslashes % 2 == 0 {
                trimmed
            } else {
                body
            }
        }
        None => body,
    }
}

/// The `ValidationException` message for a refusal: the AWS envelope
/// (`N validation error(s) detected: `) over one clause per violation.
///
/// `None` declines the whole refusal — see [`constraint_clause`].
fn constraint_envelope(violations: &[Violation]) -> Option<String> {
    let clauses: Vec<String> = violations
        .iter()
        .map(constraint_clause)
        .collect::<Option<Vec<_>>>()?;
    match clauses.len() {
        0 => None,
        1 => Some(format!("1 validation error detected: {}", clauses[0])),
        n => Some(format!(
            "{n} validation errors detected: {}",
            clauses.join("; ")
        )),
    }
}

/// The `ValidationException$fieldList` entry for one violation: the member that
/// was rejected, and why.
///
/// The entry's `message` states the value's PLACE rather than the value — this
/// service never echoes submitted input back (see [`constraint_clause`]), so the
/// place carries [`crate::state::REDACTED_VALUE`] — and the member it belongs to
/// is the entry's own `name`, which is why the sentence does not repeat the
/// member path the way the envelope's clause does.
fn constraint_field(violation: &Violation) -> Option<ValidationExceptionField> {
    let requirement = constraint_requirement(violation)?;
    let subject = match &violation.kind {
        // An absent member has no submitted value to stand in for.
        ViolationKind::MissingRequiredMember => "null".to_string(),
        _ => crate::state::REDACTED_VALUE.to_string(),
    };
    Some(ValidationExceptionField {
        name: violation.path.clone(),
        message: format!("Value {subject} failed to satisfy constraint: {requirement}"),
    })
}

/// The whole modeled `ValidationException` this service refuses `violations`
/// with: the message envelope, the `reason` the model requires, and one
/// `fieldList` entry per violating member.
///
/// `None` declines the refusal — see [`constraint_clause`] — leaving the
/// framework's own rendering in place rather than answering a half-filled error.
fn constraint_exception(violations: &[Violation]) -> Option<ValidationException> {
    let message = constraint_envelope(violations)?;
    let field_list: Vec<ValidationExceptionField> = violations
        .iter()
        .map(constraint_field)
        .collect::<Option<Vec<_>>>()?;
    Some(ValidationException {
        message,
        // Every violation here is about a request MEMBER, which is what this
        // `ValidationExceptionReason` value names.
        reason: crate::state::VALIDATION_REASON_FIELD.to_string(),
        field_list: Some(field_list),
    })
}

/// The codec a request is READ in: the author's
/// [`Service::override_protocol`] when it returns one, else the model-derived
/// protocol — the same selection the framework's own dispatch makes, so a
/// refusal written here reads exactly the bytes the operation would have.
fn read_protocol(svc: &dyn Service) -> &dyn ServerHttpProtocol {
    svc.override_protocol()
        .unwrap_or_else(|| svc.definition().protocol())
}

/// The codec a request REFUSED before the operation ran is answered in: the
/// model-derived one, for the reason [`PRE_DISPATCH`] gives — the refusal comes
/// from the validation layer in front of the operation, not from the endpoint
/// behind it, and the recorded wire shows the two announcing different versions.
fn refusal_protocol(svc: &dyn Service) -> &dyn ServerHttpProtocol {
    svc.definition().protocol()
}

/// One operation, served with its modelled input constraints enforced by THIS
/// service rather than by the framework's pre-handler validator.
///
/// The framework refuses a request whose member falls outside a modelled
/// constraint before the handler runs, and renders that refusal as a code and a
/// message — the only two things [`Refused`] carries. This service's
/// `ValidationException` declares more than that: `reason` (which the model
/// makes `@required`) and a `fieldList` naming the member that was rejected, the
/// same members its handler-written refusals already fill. A client that
/// branches on `reason`, or that shows `fieldList[].name` to say WHICH member
/// was wrong, needs them on every refusal, not only on the ones a handler words.
///
/// So the wrapped operation is reached through here: the request is read and
/// checked against the operation's own modelled input schema first, and a
/// violation is answered as the complete modeled error. A request that violates
/// nothing is handed to the operation untouched, and a failure this service has
/// no wording for is left to the framework.
pub struct ConstraintRefusals<O> {
    inner: O,
}

impl<O> ConstraintRefusals<O> {
    /// Wrap `inner`, which stays the operation that answers every request this
    /// does not refuse.
    pub fn new(inner: O) -> Self {
        Self { inner }
    }
}

impl<O: Operation> ConstraintRefusals<O> {
    /// The input as `O` will see it, or the protocol-level failure that stopped
    /// it being read at all.
    fn read_input(
        protocol: &dyn ServerHttpProtocol,
        req: &RtRequest,
    ) -> Result<O::Input, ProtocolError> {
        let mut deserializer = protocol.deserialize_input(req, O::INPUT_SCHEMA)?;
        O::Input::deserialize(&mut *deserializer).map_err(ProtocolError::Serde)
    }

    /// This service's refusal of `req`, or `None` when the request violates no
    /// modelled constraint (or fails in a way this service has no wording for,
    /// which the framework renders instead).
    fn refusal(svc: &dyn Service, req: &RtRequest) -> Option<RtResponse> {
        let violations = match Self::read_input(read_protocol(svc), req) {
            Ok(input) => aws_smithy_schema_server::validate(&input, O::INPUT_SCHEMA),
            // A member the shape requires but the request omits is found while
            // READING the request, not while walking it; `refusal_of` is what
            // describes that structurally, and to a caller it is the same class
            // of refusal as a bad value. Any other read failure has no
            // structured form, so it is left to the framework.
            Err(error) => match smithy_server_runtime::refusal_of(&error) {
                Some(Refusal::Invalid(violations)) => violations,
                _ => return None,
            },
        };
        let exception = constraint_exception(&violations)?;
        refusal_protocol(svc)
            .serialize_error(
                &exception,
                &crate::ops::types::SCHEMA_VALIDATION_EXCEPTION,
                VALIDATION_EXCEPTION,
                VALIDATION_STATUS,
            )
            .ok()
    }
}

impl<O: Operation> ErasedOperation for ConstraintRefusals<O> {
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r RtRequest,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        Box::pin(async move {
            match Self::refusal(svc, req) {
                Some(refused) => refused,
                None => {
                    smithy_server_runtime::run_operation_http(
                        &self.inner,
                        svc,
                        req,
                        ctx,
                        PRE_DISPATCH,
                    )
                    .await
                }
            }
        })
    }
}

/// One operation wired through [`ConstraintRefusals`] for the table
/// [`Service::override_operation`] consults: the operation's own wire name, so
/// the key cannot drift from the routing key the protocol resolves, and the
/// wrapped operation behind it.
macro_rules! constrained {
    ($engine:expr, $module:ident :: $operation:ident) => {
        (
            <crate::ops::$module::$operation<Eng> as Operation>::NAME,
            ::std::boxed::Box::new(ConstraintRefusals::new(
                crate::ops::$module::$operation::new(Arc::clone($engine)),
            )) as ::std::boxed::Box<dyn ErasedOperation>,
        )
    };
}

/// The `com-amazonaws-billing-awsbilling` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// The codec this service reads and answers in, built once here because
    /// [`Service::override_protocol`] hands back a borrow of `self`.
    protocol:
        aws_smithy_schema_server::HttpRpcServerProtocol<aws_smithy_schema_server::json::JsonCodec>,
    /// Every operation of this service, each served through
    /// [`ConstraintRefusals`] so a request member outside its modelled
    /// constraints is refused in the modeled `ValidationException` shape —
    /// `reason` and `fieldList` included — whichever operation it arrived at.
    /// Keyed by the operation's wire name; `override_operation` looks the
    /// request's routing key up here.
    constrained: Vec<(&'static str, Box<dyn ErasedOperation>)>,
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        let constrained: Vec<(&'static str, Box<dyn ErasedOperation>)> = vec![
            constrained!(&engine, associate_contract::AssociateContract),
            constrained!(&engine, associate_source_views::AssociateSourceViews),
            constrained!(
                &engine,
                cancel_contractual_investment_attestation::CancelContractualInvestmentAttestation
            ),
            constrained!(&engine, create_billing_view::CreateBillingView),
            constrained!(
                &engine,
                create_contractual_investment_attestation::CreateContractualInvestmentAttestation
            ),
            constrained!(&engine, delete_billing_view::DeleteBillingView),
            constrained!(&engine, disassociate_contract::DisassociateContract),
            constrained!(&engine, disassociate_source_views::DisassociateSourceViews),
            constrained!(&engine, get_billing_preferences::GetBillingPreferences),
            constrained!(&engine, get_billing_view::GetBillingView),
            constrained!(&engine, get_contract_information::GetContractInformation),
            constrained!(&engine, get_credit_allocation_history::GetCreditAllocationHistory),
            constrained!(&engine, get_credits::GetCredits),
            constrained!(
                &engine,
                get_enterprise_support_charge_summary::GetEnterpriseSupportChargeSummary
            ),
            constrained!(
                &engine,
                get_enterprise_support_contract_details::GetEnterpriseSupportContractDetails
            ),
            constrained!(&engine, get_resource_policy::GetResourcePolicy),
            constrained!(&engine, list_billing_view_segments::ListBillingViewSegments),
            constrained!(&engine, list_billing_views::ListBillingViews),
            constrained!(
                &engine,
                list_business_support_account_charges::ListBusinessSupportAccountCharges
            ),
            constrained!(
                &engine,
                list_business_support_subscription_history::ListBusinessSupportSubscriptionHistory
            ),
            constrained!(&engine, list_contracts::ListContracts),
            constrained!(
                &engine,
                list_contractual_investment_attestations::ListContractualInvestmentAttestations
            ),
            constrained!(
                &engine,
                list_contractual_investment_summaries::ListContractualInvestmentSummaries
            ),
            constrained!(&engine, list_contractual_investments::ListContractualInvestments),
            constrained!(
                &engine,
                list_enterprise_support_linked_account_charges::ListEnterpriseSupportLinkedAccountCharges
            ),
            constrained!(
                &engine,
                list_source_views_for_billing_view::ListSourceViewsForBillingView
            ),
            constrained!(&engine, list_tags_for_resource::ListTagsForResource),
            constrained!(&engine, redeem_credits::RedeemCredits),
            constrained!(&engine, split_credit::SplitCredit),
            constrained!(&engine, tag_resource::TagResource),
            constrained!(&engine, transfer_credit::TransferCredit),
            constrained!(&engine, untag_resource::UntagResource),
            constrained!(&engine, update_billing_preferences::UpdateBillingPreferences),
            constrained!(&engine, update_billing_view::UpdateBillingView),
        ];
        Self {
            constrained,
            protocol: aws_smithy_schema_server::AwsJson11(),
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
        // Every operation of this service is served through
        // `ConstraintRefusals`, which refuses a request member outside its
        // modelled constraints in this service's own `ValidationException`
        // shape — `reason` (which the model makes `@required`) and `fieldList`
        // included — instead of the code-and-message-only refusal the framework
        // renders before a handler runs. The operation itself is unchanged: a
        // request that violates nothing reaches the same handler the generated
        // routing table would have run.
        let key = self.entry.protocol().routing_key(req).ok()?;
        self.constrained
            .iter()
            .find(|(name, _)| *name == key)
            .map(|(_, operation)| operation.as_ref())
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // The centralized walk calls this on a framework-level refusal (an absent
        // required member, or a value outside a declared bound) to let this
        // service reword it.
        //
        // This service states a constraint violation WITHOUT the value that
        // violated it: the message names the member path and the constraint
        // only, so caller-supplied input — including the `@sensitive` members of
        // the billing-view operations — is never echoed back into a client's
        // logs. The code stays the modeled `ValidationException` (HTTP 400),
        // which is what the framework would have answered; only the wording
        // changes.
        //
        // An authentication refusal keeps the framework's wording: it is not an
        // input-constraint failure.
        match refusal {
            Refusal::Invalid(violations) => Some(Refused {
                code: VALIDATION_EXCEPTION.to_string(),
                message: constraint_envelope(violations)?,
            }),
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

    fn override_protocol(&self) -> Option<&dyn ServerHttpProtocol> {
        // The model declares `awsJson1_0`, but the endpoint that serves this
        // service answers `application/x-amz-json-1.1`, and a client (or proxy)
        // that keys its JSON-RPC parsing off the response content-type reads the
        // version the response announces, not the model's. Both versions of the
        // awsJson family carry one codec — the body bytes are identical — so the
        // announced version is all this changes. A request refused before the
        // operation ran does not reach here: see `PRE_DISPATCH`.
        Some(&self.protocol)
    }
}
