// SPDX-License-Identifier: CC0-1.0

//! @op GetParameter
//! Get information about a single parameter by specifying the parameter name — the read
//! half of Parameter Store. This is the ONLY file to touch for this operation.
//!
//! The answer is computed from the persisted parameter `PutParameter` wrote (see
//! [`crate::state`]): the parameter's newest version by default, or the version or label
//! a `name:selector` suffix names. Nothing here mutates the store — a read must not
//! change what a later caller sees.
//!
//! ## `WithDecryption` and `SecureString`
//!
//! A `SecureString`'s value is stored as the KMS ciphertext `PutParameter` sealed it into,
//! so `WithDecryption` selects which of the two forms this read reports: absent or false
//! it reports the stored ciphertext, base64-encoded, which only KMS can open; true it
//! reports the plaintext, and only if the parameter's key permits the caller `kms:Decrypt`
//! under the encryption context binding the value to this parameter — otherwise the read
//! is refused with `AccessDeniedException` (HTTP 400) carrying KMS's own sentence, and a
//! value no key of this endpoint opens any more — one whose key is gone, or whose key is in
//! a key state that runs no cryptographic operation — is `InvalidKeyId`. See
//! [`crate::state::secure_string`]. For `String` and `StringList` the flag selects
//! nothing, as it does for the service too ("This flag is ignored for String and
//! StringList parameter types").
//!
//! ## Parameters nobody created
//!
//! A name this Region and account hold no parameter for is the modelled
//! `ParameterNotFound` — the error code alone, with NO message: the shape's `message` is an
//! optional member and Parameter Store words none for this refusal, so nothing here invents
//! a sentence for it. The same holds for every read and delete of this slice that reports a
//! name it holds no parameter for. A read of the RESERVED hierarchy below is the one
//! exception, because what reported nothing there is another service: that refusal carries
//! the sentence [`crate::state::secretsmanager_reference::not_found_message`] words.
//!
//! ## AWS's own parameters
//!
//! The AWS-managed `/aws` hierarchy is NOT the account's: no caller can write there —
//! `PutParameter` refuses a name prefixed with `aws` or `ssm` — and AWS publishes its own
//! parameters in it, in every account and Region alike. A name in that hierarchy is
//! therefore resolved against the published catalog
//! ([`crate::state::public_parameters`]) rather than against the store, and only a name
//! neither holds is `ParameterNotFound`. What the catalog publishes is read out of the
//! state that OWNS it — the public parameter names Amazon EC2's image catalog publishes an
//! AMI under, and the id of that image — so the value a read reports is the id of an image
//! this account can really launch rather than an invented one. Such a parameter belongs to
//! no account, so the ARN a read reports for it names none, and reading it back by that ARN
//! resolves it: a field an ARN leaves EMPTY names no account and no Region at all, so it
//! constrains nothing ([`crate::state::arn_names_this_endpoint`]).
//!
//! ## Secrets, read through Parameter Store
//!
//! One hierarchy of that same AWS-owned namespace is reserved for Amazon Web Services
//! Secrets Manager: a name under `/aws/reference/secretsmanager` names a SECRET, so this
//! read resolves it against Secrets Manager — that service's own authorization gate, the
//! staging labels its versions carry and its own envelope — and reports it as a
//! `SecureString` parameter of the secret's identity. A `name:selector` suffix selects by
//! staging label there, so it serves that label's version's own bytes rather than a
//! parameter version. `WithDecryption` is what asks for those bytes: this service holds no
//! ciphertext of its own for a reference, so a read that asks for no decryption is served
//! the reference's identity without its value. See
//! [`crate::state::secretsmanager_reference`].
//!
//! ## Shape of an operation
//!
//! Each operation is a struct that owns the shared engine and implements
//! [`smithy_server_runtime::Operation`]. The framework constructs one per service via
//! `GetParameter::new(engine)` and drives it through the centralized request walk:
//! deserialize → validate → `handle` → serialize. You write ONLY the `handle` body.
//!
//! ## Reaching this service's state
//!
//! The engine is held on `self` (constructor-injected), NOT passed per request — so
//! engine access is guaranteed by the time any request runs. Reach the store through
//! [`crate::state::service_state`] and read and write it through the guard it hands back:
//!
//! ```ignore
//!     let state = crate::state::service_state(&*self.engine)
//!         .ok_or(/* a modelled server error */)?;
//!     state.lock().your_field = /* ... */;
//! ```
//!
//! `ctx` ([`smithy_server_runtime::OperationContext`]) carries request metadata only —
//! headers, URI, method, and anything middleware placed in `ctx.parts().extensions`.
//! The engine is NOT reached through `ctx`.
//!
//! @generated by the emulator core scaffold op-stub generator.
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `GetParameter`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-ssm-amazonssm/smithy-model/com-amazonaws-ssm-amazonssm.json")
//! ```
//!
//! Get information about a single parameter by specifying the parameter name. Parameter
//! names can't contain spaces. The service removes any spaces specified for the beginning
//! or end of a parameter name. If the specified name for a parameter contains spaces
//! between characters, the request fails with a ValidationException error. To get
//! information about more than one parameter at a time, use the GetParameters operation.
//! Parameter Store throughput defines the number of API transactions per second (TPS) that
//! Systems Manager can process. This applies to GetParameter, GetParameters, and
//! PutParameter API calls for your Amazon Web Services account and Amazon Web Services
//! Region. By default, Parameter Store is configured with a standard throughput quota
//! suitable for low- to moderate-volume workloads. Applications that retrieve configuration
//! data infrequently or operate at smaller scale can use this default setting without
//! additional cost. For higher-volume workloads, you can enable higher throughput. This
//! increases the maximum number of supported transactions per second for your account and
//! Region. Increased throughput supports applications and workloads that need concurrent
//! access to multiple parameters. If you experience ThrottlingException: Rate exceeded
//! errors, enable higher throughput. For more information, see Changing Parameter Store
//! throughput.
//!
//! ## Input
//!
//! * `Name` (PSParameterName, len 1..2048) **required** — The name or Amazon Resource Name
//!   (ARN) of the parameter that you want to query. For parameters shared with you from
//!   another account, you must use the full ARN. To query by parameter label, use "Name":
//!   "name:label". To query by parameter version, use "Name": "name:version". For more
//!   information about shared parameters, see Working with shared parameters in the Amazon
//!   Web Services Systems Manager User Guide.
//! * `WithDecryption` (Boolean, default false) — Return decrypted values for secure string
//!   parameters. This flag is ignored for String and StringList parameter types.
//!
//! ## Output
//!
//! * `Parameter` (Parameter) — Information about a parameter.
//!
//! ## Errors
//!
//! * `InternalServerError` (HTTP 500) — An error occurred on the server side.
//! * `InvalidKeyId` (HTTP 400) — The query key ID isn't valid.
//! * `ParameterNotFound` (HTTP 400) — The parameter couldn't be found. Verify the name and
//!   try again. For the DeleteParameter and GetParameter actions, if the specified parameter
//!   doesn't exist, the ParameterNotFound exception is not recorded in CloudTrail event logs.
//! * `ParameterVersionNotFound` (HTTP 400) — The specified parameter version wasn't found.
//!   Verify the parameter name and version, and try again.
//! * `AccessDeniedException` (HTTP 400) — widened in through `extra_errors` (see below):
//!   "The requester doesn't have permissions to perform the requested operation", which is
//!   what a `WithDecryption` read of a value whose KMS key does not permit the caller
//!   `kms:Decrypt` is refused with.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `GetParameterError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-ssm-amazonssm.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `GetParameterError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
//
// `AccessDeniedException` is widened in through `extra_errors`: this SERVICE models it
// ("The requester doesn't have permissions to perform the requested operation") and other
// operations declare it, and it is the refusal a decrypt the parameter's KMS key does not
// permit this caller answers with.
#[smithy_macros::operation_error(
    "com.amazonaws.ssm#GetParameter",
    extra_errors = ["AccessDeniedException"]
)]
#[derive(Debug)]
pub enum GetParameterErrorModeled {
    AccessDeniedException(AccessDeniedException),
    InternalServerError(InternalServerError),
    InvalidKeyId(InvalidKeyId),
    ParameterNotFound(ParameterNotFound),
    ParameterVersionNotFound(ParameterVersionNotFound),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::get_parameter::GetParameterError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type GetParameterError = crate::scaffolded::Scaffolded<GetParameterErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<GetParameterErrorModeled> for GetParameterError {
    fn from(modeled: GetParameterErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `GetParameter` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct GetParameter<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> GetParameter<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for GetParameter<Eng>
{
    type Input = GetParameterInput;
    type Output = GetParameterOutput;
    type Error = GetParameterError;

    const NAME: &'static str = "GetParameter";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::GET_PARAMETER_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::GET_PARAMETER_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            GetParameterErrorModeled::InternalServerError(InternalServerError {
                message: Some("the parameter store is unavailable".to_string()),
            })
        })?;

        // The spaces rule is the one refusal a read makes: the service trims the name
        // and fails a name with spaces between characters with a `ValidationException`,
        // which `GetParameter` models no error for.
        let name = crate::state::trimmed_name(&input.name)
            .map_err(crate::scaffolded::Scaffolded::validation)?;

        // `ParameterNotFound` carries NO message: the member is optional and the service
        // sends none for a name it holds no parameter for, so a client comparing the whole
        // error payload reads the code alone (see the module docs).
        let not_found = || {
            GetParameterErrorModeled::ParameterNotFound(ParameterNotFound { message: None })
        };

        // A name this service could never have stored (an ARN of another service, say)
        // names no parameter, so it is a miss rather than a refusal.
        let Some(query) = crate::state::parse_parameter_query(name) else {
            return Err(not_found().into());
        };
        // `name:` names neither a version nor a label: the selector is missing rather
        // than unknown, so the request is refused rather than answered with a
        // version-not-found for a selector that was never given.
        if query.selector == Some("") {
            return Err(crate::scaffolded::Scaffolded::validation(format!(
                "Name '{name}' ends in ':' but names no version or label after it"
            )));
        }
        // A full ARN names the account and the Region its parameter lives in, and a
        // parameter is resolved through the endpoint it is asked for: an ARN naming
        // another account or another Region than this request was signed for names
        // nothing reachable here, while a field the ARN leaves EMPTY names neither and
        // resolves here — which is the spelling the ARN of a parameter AWS publishes
        // carries. The Region therefore never comes from the request body — only from the
        // signed credential scope, which [`request_region`] constrains.
        let region = crate::state::request_region(ctx.parts());
        if !query.names_this_endpoint(&region) {
            return Err(not_found().into());
        }

        // A name in the hierarchy reserved for Secrets Manager references names a SECRET
        // rather than a parameter, so it is resolved against that service — its
        // authorization gate, its staging labels and its envelope — and never against this
        // store, which no caller can write that hierarchy into. Resolved before the store
        // is read, because nothing in it could hold such a name.
        if let Some(secret_id) =
            crate::state::secretsmanager_reference::referenced_secret(query.name)
        {
            let parameter = crate::state::secretsmanager_reference::resolve(
                &*self.engine,
                ctx,
                &region,
                query.path,
                secret_id,
                query.selector,
                input.with_decryption.unwrap_or(false),
            )
            .await
            .map_err(reference_refusal)?;
            return Ok(GetParameterOutput {
                parameter: Some(parameter),
            });
        }

        // The store guard is released before the key is asked, so no state of this
        // service is held while KMS's decision is made.
        let published = crate::state::public_parameters::published(
            &*self.engine,
            &region,
            query.name,
        );
        let (mut parameter, to_open) = {
            let store = state.lock();
            let record = match store.parameter(&region, query.name) {
                Some(record) => record,
                // A name in the hierarchy AWS publishes its own parameters in is resolved
                // against THAT catalog, which this account's store holds nothing of; a
                // name neither holds is the miss below.
                None => match published.as_ref() {
                    Some(record) => record,
                    None => return Err(not_found().into()),
                },
            };
            let Some(version) = record.select(query.selector) else {
                // The parameter exists, so what the request named is a version or a label
                // this parameter does not carry — and the refusal says which of the two
                // the caller asked for.
                return Err(GetParameterErrorModeled::ParameterVersionNotFound(
                    ParameterVersionNotFound {
                        message: Some(crate::state::selector_not_found_message(
                            query.path,
                            query.selector.unwrap_or_default(),
                        )),
                    },
                )
                .into());
            };
            (
                version.to_parameter(record, query.selector),
                crate::state::secure_string::value_to_open(input.with_decryption, record, version)
                    .map(|sealed| (0usize, sealed))
                    .into_iter()
                    .collect::<Vec<_>>(),
            )
        };

        // A `WithDecryption` read of a `SecureString` has its value opened under the
        // parameter's own KMS key, so that key's policy decides the read before the value
        // is reported; a read that asks for none reports the stored ciphertext.
        for (_, plaintext) in
            crate::state::secure_string::open(&*self.engine, ctx, &region, &to_open).map_err(
                |refusal| match refusal {
                    crate::state::secure_string::OpenRefusal::Denied(message) => {
                        GetParameterErrorModeled::AccessDeniedException(AccessDeniedException {
                            message,
                        })
                    }
                    crate::state::secure_string::OpenRefusal::Key(message) => {
                        GetParameterErrorModeled::InvalidKeyId(InvalidKeyId {
                            message: Some(message),
                        })
                    }
                },
            )?
        {
            parameter.value = Some(plaintext);
        }

        Ok(GetParameterOutput {
            parameter: Some(parameter),
        })
    }
}

/// How a read of the reserved Secrets Manager hierarchy that reported no value is refused.
///
/// A reference nothing resolves is `ParameterNotFound`, and that refusal CARRIES a message
/// (unlike a name this Region simply holds no parameter for): the read is a read of another
/// service, so the sentence names the reference and carries the source's own account of why
/// nothing was reported for it
/// ([`crate::state::secretsmanager_reference::not_found_message`]). A read the service this
/// one DEPENDS ON would not serve — it refused the caller, or the value could not be opened
/// under the secret's key — is refused as that dependency failing
/// ([`crate::state::secretsmanager_reference::DEPENDENCY_FAILURE_MESSAGE`]) rather than
/// relayed as the other service's own refusal, which this operation models no error for and
/// states nothing of. An emulator that answered with no Secrets Manager at all resolved
/// nothing, which is a server-side fault.
fn reference_refusal(refusal: crate::state::secretsmanager_reference::Refusal) -> GetParameterError {
    use crate::state::secretsmanager_reference::Refusal;
    match refusal {
        Refusal::NotFound(message) => {
            GetParameterErrorModeled::ParameterNotFound(ParameterNotFound {
                message: Some(message),
            })
            .into()
        }
        Refusal::Dependency => crate::scaffolded::Scaffolded::validation(
            crate::state::secretsmanager_reference::DEPENDENCY_FAILURE_MESSAGE,
        ),
        Refusal::Unavailable(message) => {
            GetParameterErrorModeled::InternalServerError(InternalServerError {
                message: Some(message),
            })
            .into()
        }
    }
}
