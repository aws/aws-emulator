// SPDX-License-Identifier: CC0-1.0

//! How this service states a request it refuses BEFORE any handler runs.
//!
//! A request that breaks a rule the MODEL declares — a `@required` member absent, a value
//! outside a declared `@length`, `@range`, `@pattern` or enum value set — is refused at
//! the boundary, by the framework's own deserializer and constraint walk, before the
//! operation's `handle` is reached. `ValidationException` is a common error of every
//! operation of this service (`smithy_query(query="digest")`), and the model gives it a
//! required `message` plus a `fieldList` of `ValidationExceptionField {path, message}` —
//! the machine-readable half an SDK or a console renders to point at the offending
//! member. The framework's own refusal body carries an error code and a message and
//! nothing else ([`smithy_server_runtime::Refused`]), so a refusal rendered there can
//! never carry that field list.
//!
//! So every modelled operation is served behind [`ModelledValidation`], which collects
//! the violations of a request with the framework's own two steps and answers them in
//! this service's modelled shape, and hands every request it does not refuse to the
//! generated operation unchanged.
//!
//! # How this service words one
//!
//! One envelope sentence over one clause per violation:
//!
//! ```text
//! 1 validation error detected. Value at '/EncryptionAttributes' failed to satisfy constraint: Member must not be null
//! ```
//!
//! The fixed text is `<N> validation error(s) detected. ` followed by the per-field
//! clause, `Value at '<path>' failed to satisfy constraint: <requirement>`. Its variable
//! parts are the violation count, the path — the JSON pointer of the member in the
//! request document, spelled as the MODEL spells that member — and the requirement the
//! member failed. No submitted value is echoed anywhere in it, which is also what keeps a
//! `@sensitive` member (`PlainText`, `CipherText`, a PIN block) out of a refusal. The
//! same clause, without the envelope, is what each `fieldList` entry states about its own
//! member.

use smithy_server_runtime::{
    BoxFuture, DeserializableStruct, ErasedOperation, Operation, OperationContext,
    ServerHttpProtocol, Service, Violation, ViolationKind,
};
use std::collections::HashMap;
use std::marker::PhantomData;

use crate::ops::types::{ValidationException, ValidationExceptionField};

/// The wire error code this service refuses a request with: its modelled
/// `ValidationException`.
pub(crate) const VALIDATION_EXCEPTION: &str = "ValidationException";

/// `ValidationException`'s modelled `@httpError`
/// (`smithy_query(query="shape",shape="ValidationException")`).
pub(crate) const VALIDATION_EXCEPTION_STATUS: u16 = 400;

/// The framework's name for a step from a collection into one of its elements, which
/// [`service_common::json_pointer`] consumes: it is the walk's own spelling rather than a
/// member of any shape, so the casing below leaves it alone and lets that module read it.
const COLLECTION_STEP: &str = "member";

/// The path a clause and a `fieldList` entry name the member by: the JSON pointer of that
/// member in the request document.
///
/// The framework spells a violation's path as the dotted, lowerCamel trail of member names
/// it found the value under (`aws_smithy_schema_server::constraints::path`). This service
/// states the same position as a pointer whose segments are the member names as the MODEL
/// spells them — every member of every input shape of this model is PascalCase — so the
/// one thing done here is restoring that casing, segment by segment; the translation from
/// the dotted path to the pointer itself (the collection step, an element's base, the
/// characters a segment escapes) is [`service_common::json_pointer::json_pointer`]'s, which
/// owns that rule for every service.
///
/// A segment that is an element position is left as it is for that module to renumber, and
/// so is the collection step it reads. No input shape of this model carries a map, so no
/// segment of a path here is a key the caller chose — one that would have to be kept
/// exactly as sent rather than re-cased.
///
/// The casing is applied to the segment rather than looked up on the schema because a
/// member schema carries its own name and nothing of the shape it targets
/// ([`::aws_smithy_schema::Schema::member_name`]), so a NESTED member's modelled spelling
/// is not reachable from the operation's input schema at all — and the rule holds for
/// every member of every input shape of this model, which the suite's table pins across
/// operations and depths.
fn pointer(path: &str) -> String {
    let dotted: Vec<String> = path
        .split('.')
        .map(|segment| {
            if segment == COLLECTION_STEP || segment.bytes().all(|byte| byte.is_ascii_digit()) {
                return segment.to_string();
            }
            let mut characters = segment.chars();
            match characters.next() {
                Some(first) => first
                    .to_uppercase()
                    .chain(characters)
                    .collect::<String>(),
                None => segment.to_string(),
            }
        })
        .collect();
    ::service_common::json_pointer::json_pointer(&dotted.join("."))
}

/// The constraint half of a clause: what the value failed, as the validation framework
/// words it.
///
/// The sentence itself is the framework's own, read back out of the clause it renders for
/// that violation class rather than restated here, so a requirement this service states
/// and one the framework states are the same sentence — and a class added upstream is
/// worded rather than unhandled.
///
/// A `@pattern` requirement quotes the expression AS THE MODEL STATES IT, anchoring
/// included: the pattern is part of the refusal's fixed text, and a client that re-validates
/// locally with the expression it read out of the message has to be handed the one the model
/// declares rather than a rewriting of it.
fn requirement(kind: &ViolationKind) -> String {
    ::service_common::validation_message::requirement(kind.clone())
}

/// The clause one violation states: the member's pointer and the requirement it failed.
///
/// It names the member and the rule, and never the value that broke it: the value is the
/// caller's own input, several of this service's members are `@sensitive`, and AWS's
/// clause for this service carries no echo of it either.
fn clause(violation: &Violation) -> String {
    format!(
        "Value at '{}' failed to satisfy constraint: {}",
        pointer(&violation.path),
        requirement(&violation.kind)
    )
}

/// The modelled `ValidationException` for a set of boundary violations: the envelope
/// sentence over every clause, and one `fieldList` entry per violated member.
///
/// What this service's own recording pins is the SINGLE-violation envelope, including the
/// `detected.` that ends it. The count word and the clause separator for several
/// violations are the validation framework's own, read off
/// [`::aws_smithy_schema_server::constraints::envelope`] — which pluralises `validation
/// error` to `validation errors` and joins the clauses with `; ` — so a refusal of two
/// members is stated the way a refusal of one is, rather than in a format invented here.
///
/// `None` when there is nothing to state, i.e. no violation.
pub(crate) fn boundary_exception(violations: &[Violation]) -> Option<ValidationException> {
    if violations.is_empty() {
        return None;
    }
    let clauses: Vec<String> = violations.iter().map(clause).collect();
    let message = match clauses.len() {
        1 => format!("1 validation error detected. {}", clauses[0]),
        n => format!("{n} validation errors detected. {}", clauses.join("; ")),
    };
    let field_list = violations
        .iter()
        .zip(clauses)
        .map(|(violation, clause)| ValidationExceptionField {
            path: pointer(&violation.path),
            message: clause,
        })
        .collect();
    Some(ValidationException {
        message,
        field_list: Some(field_list),
    })
}

/// The synthetic error shape a request the codec could not read is answered in.
///
/// Deliberately in the `smithy.framework` namespace, as the scaffold's own synthetic
/// shapes are: this is NOT a shape in the service's model, because the failure it reports
/// is not one the model describes — no modelled rule was broken, the request never became
/// a value of a modelled shape at all. The wire error type comes from
/// [`SERIALIZATION_EXCEPTION`] rather than from this id.
static SERIALIZATION_EXCEPTION_MESSAGE: ::aws_smithy_schema::Schema =
    ::aws_smithy_schema::Schema::new_member(
        ::aws_smithy_schema::ShapeId::from_static(
            "smithy.framework#SerializationException$message",
            "smithy.framework",
            "message",
        ),
        ::aws_smithy_schema::ShapeType::String,
        "message",
        0,
    );

/// Schema for the synthetic serialization-failure shape. See
/// [`SERIALIZATION_EXCEPTION_MESSAGE`].
static SERIALIZATION_EXCEPTION_SCHEMA: ::aws_smithy_schema::Schema =
    ::aws_smithy_schema::Schema::new_struct(
        ::aws_smithy_schema::ShapeId::from_static(
            "smithy.framework#SerializationException",
            "smithy.framework",
            "SerializationException",
        ),
        ::aws_smithy_schema::ShapeType::Structure,
        &[&SERIALIZATION_EXCEPTION_MESSAGE],
    );

/// The wire error type a request the codec could not read is announced with.
///
/// restJson1 resolves the error CLASS from `x-amzn-errortype`, which is how an SDK picks
/// the exception it raises, so this value is part of the refusal's contract: a request
/// whose JSON does not fit the modelled shapes failed to deserialize, and is announced as
/// the framework failure it is rather than as the operation's modelled
/// `ValidationException`, which says that a value broke a modelled rule.
const SERIALIZATION_EXCEPTION: &str = "SerializationException";

/// The status a serialization failure is refused with: the request is the caller's to
/// fix, and nothing about it is retryable.
const SERIALIZATION_EXCEPTION_STATUS: u16 = 400;

/// The payload of a serialization failure: what the codec could not read.
#[derive(Debug, Clone)]
struct SerializationException {
    message: String,
}

impl ::aws_smithy_schema::serde::SerializableStruct for SerializationException {
    fn serialize_members(
        &self,
        serializer: &mut dyn ::aws_smithy_schema::serde::ShapeSerializer,
    ) -> Result<(), ::aws_smithy_schema::serde::SerdeError> {
        serializer.write_string(&SERIALIZATION_EXCEPTION_MESSAGE, self.message.as_str())
    }
}

/// One refusal as the wrapper states it: the modelled `ValidationException` for a value
/// that broke a modelled rule, the synthetic serialization failure for a request that
/// never became a value. Kept together so both are serialized through the one call that
/// renders an error in this service's protocol.
enum Stated {
    /// The modelled `ValidationException`, with its `fieldList`.
    Modelled(ValidationException),
    /// The synthetic serialization failure.
    Unreadable(SerializationException),
}

impl Stated {
    /// The payload to serialize.
    fn as_serializable(&self) -> &dyn ::aws_smithy_schema::serde::SerializableStruct {
        match self {
            Self::Modelled(modelled) => modelled,
            Self::Unreadable(unreadable) => unreadable,
        }
    }

    /// The schema that payload serializes against.
    fn schema(&self) -> &'static ::aws_smithy_schema::Schema {
        match self {
            Self::Modelled(_) => &crate::ops::types::SCHEMA_VALIDATION_EXCEPTION,
            Self::Unreadable(_) => &SERIALIZATION_EXCEPTION_SCHEMA,
        }
    }
}

/// The modelled-refusal wrapper for EVERY operation this service serves.
///
/// `ValidationException` is a common error of this service — the model declares it on all
/// fifteen operations — so every operation states a request refused before its handler ran
/// the same way, and there is no operation whose boundary refusal may answer in the
/// framework's envelope instead. An operation added to the model later is wrapped by
/// adding one line here.
pub(crate) fn modelled_validation_table<Eng: Send + Sync + 'static>(
) -> HashMap<&'static str, Box<dyn ErasedOperation>> {
    let mut refusals: HashMap<&'static str, Box<dyn ErasedOperation>> = HashMap::new();
    register::<crate::ops::decrypt_data::DecryptData<Eng>>(&mut refusals);
    register::<crate::ops::encrypt_data::EncryptData<Eng>>(&mut refusals);
    register::<crate::ops::generate_as2805_kek_validation::GenerateAs2805KekValidation<Eng>>(
        &mut refusals,
    );
    register::<crate::ops::generate_auth_request_cryptogram::GenerateAuthRequestCryptogram<Eng>>(
        &mut refusals,
    );
    register::<crate::ops::generate_card_validation_data::GenerateCardValidationData<Eng>>(
        &mut refusals,
    );
    register::<crate::ops::generate_mac::GenerateMac<Eng>>(&mut refusals);
    register::<crate::ops::generate_mac_emv_pin_change::GenerateMacEmvPinChange<Eng>>(
        &mut refusals,
    );
    register::<crate::ops::generate_pin_data::GeneratePinData<Eng>>(&mut refusals);
    register::<crate::ops::re_encrypt_data::ReEncryptData<Eng>>(&mut refusals);
    register::<crate::ops::translate_key_material::TranslateKeyMaterial<Eng>>(&mut refusals);
    register::<crate::ops::translate_pin_data::TranslatePinData<Eng>>(&mut refusals);
    register::<crate::ops::verify_auth_request_cryptogram::VerifyAuthRequestCryptogram<Eng>>(
        &mut refusals,
    );
    register::<crate::ops::verify_card_validation_data::VerifyCardValidationData<Eng>>(
        &mut refusals,
    );
    register::<crate::ops::verify_mac::VerifyMac<Eng>>(&mut refusals);
    register::<crate::ops::verify_pin_data::VerifyPinData<Eng>>(&mut refusals);
    refusals
}

/// Register one operation's modelled-refusal wrapper under the key the model names that
/// operation by, read off the operation itself so the table cannot be keyed by a
/// hand-typed name that drifts from the model.
fn register<Op: Operation>(refusals: &mut HashMap<&'static str, Box<dyn ErasedOperation>>) {
    refusals.insert(Op::NAME, Box::new(ModelledValidation::<Op>::new()));
}

/// One modelled operation, served so that a request refused BEFORE its handler runs is
/// answered in this service's modelled `ValidationException` — with the `fieldList` the
/// model declares — rather than in the framework's refusal body, which carries an error
/// code and a `message` and nothing else.
///
/// Zero-sized: the wrapper holds no engine and no operation instance. It carries the
/// operation TYPE, which is all it needs — the input shape to read the request into and
/// the input `Schema` to read it against — and hands a request the model accepts to the
/// one operation instance the generated routing table built from the shared engine. There
/// is therefore no second instance of any operation and no second store.
///
/// A request this wrapper does not refuse is READ TWICE, once here and once in the
/// operation's own sequence, because the framework hands no already-built input to an
/// operation. That cost is deliberate and bounded by the body limits the framework
/// already enforces; it is what buys the refusal a shape only the handler side could
/// otherwise have.
struct ModelledValidation<Op> {
    operation: PhantomData<fn() -> Op>,
}

impl<Op> ModelledValidation<Op> {
    const fn new() -> Self {
        Self {
            operation: PhantomData,
        }
    }
}

impl<Op: Operation> ErasedOperation for ModelledValidation<Op> {
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r aws_smithy_runtime_api::http::Request,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, aws_smithy_runtime_api::http::Response> {
        Box::pin(async move {
            let started = std::time::Instant::now();
            // The protocol is read, used and dropped inside this one block:
            // `&dyn ErasedServiceDefinition` is not `Sync`, so holding it across the await
            // below would make this future non-`Send`.
            let refused = {
                let protocol: &dyn ServerHttpProtocol = svc.definition().protocol();
                let stated = match boundary_of::<Op>(protocol, req) {
                    Boundary::Accepted => None,
                    Boundary::Violated(violations) => boundary_exception(&violations).map(
                        |modelled| {
                            (
                                Stated::Modelled(modelled),
                                VALIDATION_EXCEPTION,
                                VALIDATION_EXCEPTION_STATUS,
                            )
                        },
                    ),
                    Boundary::Unreadable(detail) => Some((
                        Stated::Unreadable(SerializationException { message: detail }),
                        SERIALIZATION_EXCEPTION,
                        SERIALIZATION_EXCEPTION_STATUS,
                    )),
                };
                match stated {
                    Some((stated, code, status)) => match protocol.serialize_error(
                        stated.as_serializable(),
                        stated.schema(),
                        code,
                        status,
                    ) {
                        Ok(response) => Some((response, code, status)),
                        // A refusal that will not serialize leaves the request to the
                        // framework, which refuses it in its own envelope — the very
                        // divergence this wrapper removes, so it is logged rather than
                        // passed over silently.
                        Err(error) => {
                            tracing::warn!(
                                operation = Op::NAME,
                                %error,
                                "this service's modelled refusal would not serialize; it \
                                 falls back to the framework's own envelope"
                            );
                            None
                        }
                    },
                    None => None,
                }
            };
            if let Some((response, code, status)) = refused {
                // The row the framework sequence would have recorded for this request, so
                // a refused one is not a hole in the telemetry series.
                record_refusal::<Op>(ctx, started.elapsed(), status, code);
                return response;
            }
            // Nothing to state: the request is served by the generated operation, which
            // runs the whole framework sequence (deserialize, validate, handle, serialize)
            // plus its telemetry over the same bytes. Reached through the routing table
            // rather than through `override_operation`, which would route straight back
            // here.
            match svc.definition().routing_table().get(Op::NAME) {
                Some(operation) => operation.run(svc, req, ctx).await,
                // Unreachable in practice: this wrapper is only reachable under the key
                // the generated table registered. Answer the framework's own
                // no-such-operation response rather than inventing one.
                None => svc.definition().routing_error_response(req),
            }
        })
    }

    /// `None` — the framework reads the schemes off the MODELLED operation the request's
    /// routing key names (the generated table's instance), which is what the model bakes
    /// them on, so wrapping an operation cannot change which schemes sign it.
    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        None
    }
}

/// What the boundary made of a request: the two refusal CLASSES it tells apart, and the
/// request it accepts.
///
/// The two classes are answered in different error channels because they are different
/// failures. A request that was READ and then broke a rule the model declares is the
/// modelled `ValidationException`. A request the codec could not read into the modelled
/// input shape at all — a member whose JSON type is not the modelled one, a body that is
/// not the document the protocol expects — broke no modelled rule: nothing of it was ever
/// a value, so it is a SERIALIZATION failure, and the error type a restJson1 client reads
/// out of `x-amzn-errortype` to pick its exception class has to say so.
enum Boundary {
    /// The model accepts the request; the generated operation serves it.
    Accepted,
    /// Values that broke rules the model declares.
    Violated(Vec<Violation>),
    /// The request could not be read into the modelled input shape; the codec says why.
    Unreadable(String),
}

/// What the boundary makes of `req` for `Op`, by the framework's OWN walk.
///
/// This is the framework's own code, in its order — read the request into the operation's
/// input shape ([`smithy_server_runtime::DeserializableStruct`]), then run the
/// constraint-trait walk over what was read
/// ([`::aws_smithy_schema_server::constraints::validate`]) — so it cannot find a different
/// violation set from the sequence that follows it. Only the WORDING, the SHAPE and the
/// error TYPE of the answer are this service's.
///
/// [`Boundary::Accepted`] for a request the model accepts, and for a failure the framework
/// detects before the codec is reached (a request the protocol cannot even hand a
/// deserializer for), which keeps the framework's own rendering. `member_name` arrives
/// already spelled as a violation path (`aws_smithy_schema_server::missing_member` applies
/// `constraints::path::member` and each enclosing member prepends its own segment), so it
/// places exactly as a constraint violation's path does.
fn boundary_of<Op: Operation>(
    protocol: &dyn ServerHttpProtocol,
    req: &aws_smithy_runtime_api::http::Request,
) -> Boundary {
    let Ok(mut deserializer) = protocol.deserialize_input(req, Op::INPUT_SCHEMA) else {
        return Boundary::Accepted;
    };
    let read = <Op::Input as DeserializableStruct>::deserialize(&mut *deserializer);
    drop(deserializer);
    match read {
        Ok(input) => {
            match ::aws_smithy_schema_server::constraints::validate(&input, Op::INPUT_SCHEMA) {
                violations if violations.is_empty() => Boundary::Accepted,
                violations => Boundary::Violated(violations),
            }
        }
        Err(::aws_smithy_schema::serde::SerdeError::MissingMember { member_name }) => {
            Boundary::Violated(vec![Violation {
                path: member_name,
                kind: ViolationKind::MissingRequiredMember,
                value: None,
                sensitive: false,
            }])
        }
        Err(error) => Boundary::Unreadable(error.to_string()),
    }
}

/// Record the refusal answered above as the operation row the framework sequence would
/// have recorded, so a request refused by the wrapper is not a hole in the telemetry
/// series: the same classification (from the refusal's status and error shape), the same
/// folded handle (this service's signing name and the operation's own name), and the same
/// already-sanitized request metadata. Everything is gated on a client collecting,
/// exactly as the framework gates it.
fn record_refusal<Op: Operation>(
    ctx: &OperationContext<'_>,
    latency: std::time::Duration,
    status: u16,
    code: &str,
) {
    if !emulator_telemetry::recording_active() {
        return;
    }
    let handle =
        emulator_telemetry::OperationHandle::folded(ctx.identity().signing_name_static(), Op::NAME);
    emulator_telemetry::record_generated_operation_with_handle(
        &handle,
        latency,
        Some(emulator_telemetry::OperationFailure::classify(status, code)),
        smithy_server_runtime::RequestTelemetry::from_headers(&ctx.parts().headers).into_metadata(),
    );
}
