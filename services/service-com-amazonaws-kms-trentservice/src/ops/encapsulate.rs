// SPDX-License-Identifier: CC0-1.0

//! @op Encapsulate
//! IMPLEMENTED: runs the ENCAPSULATION direction of the key-encapsulation mechanism the
//! named KMS key's `KeySpec` names, and answers with the shared secret it produced, the
//! ciphertext a holder of that key recovers the same secret from, and the mechanism it
//! ran. This is the ONLY file to touch for this operation.
//!
//! The key must be one that encapsulates: its `KeyUsage` has to be
//! `ENCAPSULATE_DECAPSULATE`, which only an `ML_KEM_*` key spec carries, and any other
//! key is "a KMS key [whose] KeyUsage ... is incompatible with the API operation"
//! (`smithy_query(query="doc",shape="InvalidKeyUsageException")`). The mechanism itself
//! is `crate::state::MlKemKeyPair`, over the key material the key was minted with, so
//! the ciphertext this answers with is one the private half held in this store opens —
//! and so is one a party holding only the published encapsulation key could have
//! produced without calling KMS.
//!
//! Every pre-condition a cryptographic operation shares is judged here in the order the
//! other cryptographic operations judge it: the identifier resolves, the key state
//! admits use, the grant tokens are well formed, the authorities in force allow the
//! call, and only then the mechanism runs. `DryRun` is answered after every check and
//! before anything is produced.
//!
//! ## Inputs
//!
//! * `KeyId` (KeyIdType, REQUIRED) — the key whose public half encapsulates the secret.
//! * `EncapsulationAlgorithm` (EncapsulationAlgorithmSpec, REQUIRED) — the mechanism to
//!   run.
//! * `GrantTokens` (GrantTokenList) — grant tokens the caller draws permission from.
//! * `DryRun` (NullableBooleanType) — check the request and refuse.
//!
//! ## Outputs
//!
//! * `KeyId` (KeyIdType) — the key ARN of the key that encapsulated.
//! * `SharedSecret` (PlaintextType) — the secret the encapsulation produced.
//! * `SharedSecretCiphertextBlob` (CiphertextType) — what `Decapsulate` recovers it from.
//! * `EncapsulationAlgorithm` (EncapsulationAlgorithmSpec) — the mechanism that ran.
//!
//! ## Errors
//!
//! * `DependencyTimeoutException` (HTTP 503) — the system timed out.
//! * `DisabledException` (HTTP 400) — the KMS key is not enabled.
//! * `DryRunOperationException` (HTTP 400) — the `DryRun` option was set.
//! * `InvalidGrantTokenException` (HTTP 400) — a grant token is not valid.
//! * `InvalidKeyUsageException` (HTTP 400) — the key's `KeyUsage` does not encapsulate.
//! * `KeyUnavailableException` (HTTP 500) — the key was not available.
//! * `KMSInternalException` (HTTP 500) — an internal exception occurred.
//! * `KMSInvalidStateException` (HTTP 400) — the key state is not valid for the request.
//! * `NotFoundException` (HTTP 400) — the identifier names no key of this account.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-kms-trentservice.json");

// The operation's MODELED errors.
#[smithy_macros::operation_error("com.amazonaws.kms#Encapsulate")]
#[derive(Debug)]
pub enum EncapsulateErrorModeled {
    DependencyTimeoutException(DependencyTimeoutException),
    DisabledException(DisabledException),
    DryRunOperationException(DryRunOperationException),
    InvalidGrantTokenException(InvalidGrantTokenException),
    InvalidKeyUsageException(InvalidKeyUsageException),
    KeyUnavailableException(KeyUnavailableException),
    KMSInternalException(KMSInternalException),
    KMSInvalidStateException(KMSInvalidStateException),
    NotFoundException(NotFoundException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::encapsulate::EncapsulateError`).
pub type EncapsulateError = crate::scaffolded::Scaffolded<EncapsulateErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel.
impl From<EncapsulateErrorModeled> for EncapsulateError {
    fn from(modeled: EncapsulateErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `Encapsulate` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`].
pub struct Encapsulate<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> Encapsulate<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for Encapsulate<Eng>
{
    type Input = EncapsulateInput;
    type Output = EncapsulateOutput;
    type Error = EncapsulateError;

    const NAME: &'static str = "Encapsulate";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::ENCAPSULATE_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::ENCAPSULATE_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let Some(state) = crate::state::service_state(&*self.engine) else {
            return Err(internal("the KMS state store is unavailable"));
        };

        // Who the request speaks for, resolved BEFORE this service's store is locked, so
        // the IAM and STS stores the resolution reads are never reached while this
        // service's guard is held.
        let caller = crate::state::authorization::caller(&*self.engine, ctx);
        let region = crate::state::request_region(ctx);
        let mut state = state.lock();

        let tokens = input.grant_tokens.clone().unwrap_or_default();

        // `Encapsulate` models no `InvalidArnException`, so a malformed ARN is reported
        // the same way as an identifier that names nothing: the key is not found.
        let key = match state.resolve(&region, &input.key_id) {
            crate::state::KeyRef::Found(key) => key,
            crate::state::KeyRef::UnauthorizedAccount => {
                return Err(crate::state::unauthorized_account(Self::NAME));
            }
            crate::state::KeyRef::NotFound | crate::state::KeyRef::MalformedArn => {
                return Err(EncapsulateErrorModeled::NotFoundException(NotFoundException {
                    message: Some(crate::state::key_not_found_message(&region, &input.key_id)),
                })
                .into());
            }
        };

        // A key that is not in the `Enabled` state cannot be used in a cryptographic
        // operation: a disabled key answers `DisabledException`, and any other
        // non-usable state answers `KMSInvalidStateException`, which names the state the
        // key is actually in. Judged BEFORE the grant tokens, so a key that cannot be
        // used at all reports why rather than reporting an authorization failure.
        if !key.is_usable() {
            if key.key_state == crate::state::KEY_STATE_DISABLED {
                return Err(EncapsulateErrorModeled::DisabledException(DisabledException {
                    message: Some(crate::state::invalid_state_message(key)),
                })
                .into());
            }
            return Err(EncapsulateErrorModeled::KMSInvalidStateException(
                KMSInvalidStateException {
                    message: Some(crate::state::invalid_state_message(key)),
                },
            )
            .into());
        }

        // A token that is not one this service minted names no grant at all, so the
        // TOKEN "is not valid". Everything else about permission is the service's one
        // authorization decision (`crate::state::authorization`). `Encapsulate` takes no
        // `EncryptionContext`, so no encryption-context grant constraint applies to it.
        let named = match state.grants_for_tokens(&region, &tokens) {
            Ok(named) => named,
            Err(crate::state::GrantRefusal::MalformedToken) => {
                return Err(EncapsulateErrorModeled::InvalidGrantTokenException(
                    InvalidGrantTokenException {
                        message: Some("The specified grant token is not valid.".to_string()),
                    },
                )
                .into());
            }
            // Every token is well formed but names a grant the store no longer holds,
            // which carries no permission; what else the caller holds is judged below.
            Err(crate::state::GrantRefusal::NotAuthorized) => Vec::new(),
        };
        let presented = (!tokens.is_empty()).then_some(named.as_slice());
        if let Err(message) = crate::state::authorization::authorize(
            &caller,
            &state,
            &region,
            key,
            Self::NAME,
            None,
            presented,
        ) {
            return Err(crate::scaffolded::Scaffolded::access_denied(message));
        }

        // Which mechanism this key encapsulates under is exactly the key's own
        // encapsulation algorithms — the list that follows from its key spec — and a key
        // with no such list cannot encapsulate at all. The same judgement `Decapsulate`
        // makes, so the two cannot disagree about what a key can do.
        let Some(algorithms) = key.algorithms_under(crate::state::KEY_USAGE_ENCAPSULATE_DECAPSULATE)
        else {
            return Err(invalid_key_usage(crate::state::wrong_key_usage_message(
                key,
                Self::NAME,
            )));
        };
        // "Specifies the key encapsulation algorithm used to generate the shared secret and
        // its encapsulated form. The only valid value is ML_KEM"
        // (`smithy_query(query="doc",shape="EncapsulateRequest",member="EncapsulationAlgorithm")`):
        // the member is required, so the boundary has already refused a request naming
        // none, and a mechanism the key's own spec does not name is "incompatible with the
        // type of key material in the KMS key (KeySpec)"
        // (`smithy_query(query="doc",shape="InvalidKeyUsageException")`).
        let algorithm = input.encapsulation_algorithm.clone();
        if !algorithms.contains(&algorithm) {
            return Err(invalid_key_usage(
                crate::state::incompatible_algorithm_message(&algorithm, &key.key_spec),
            ));
        }

        // "The request was rejected because the DryRun parameter was specified"
        // (`smithy_query(query="doc",shape="DryRunOperationException")`): a dry run checks
        // the request and then always fails, so it is answered AFTER every check above
        // and BEFORE anything is produced.
        if input.dry_run == Some(true) {
            return Err(EncapsulateErrorModeled::DryRunOperationException(
                DryRunOperationException {
                    message: Some(crate::state::DRY_RUN_MESSAGE.to_string()),
                },
            )
            .into());
        }

        // The mechanism runs over the key material this key holds, so the ciphertext is
        // one this key's own private half recovers the secret from.
        let Some(pair) = key.material.ml_kem() else {
            return Err(EncapsulateErrorModeled::KeyUnavailableException(
                KeyUnavailableException {
                    message: Some(crate::state::invalid_state_message(key)),
                },
            )
            .into());
        };
        let Some((shared_secret, ciphertext)) = pair.encapsulate() else {
            return Err(internal("the shared secret could not be encapsulated"));
        };

        // KMS answers with the key ARN, not the identifier the request used.
        let key_arn = key.arn.clone();
        let key_id = key.key_id.clone();

        // The key was just used in a successful cryptographic operation, which is what
        // `GetKeyLastUsage` reports once the tracking has produced the record.
        state.record_last_usage(&region, &key_id, Self::NAME);

        Ok(EncapsulateOutput {
            key_id: Some(key_arn),
            shared_secret: Some(shared_secret),
            shared_secret_ciphertext_blob: Some(ciphertext),
            encapsulation_algorithm: Some(algorithm),
        })
    }
}

/// Refuse a key that cannot encapsulate: "the KeyUsage value of the KMS key is
/// incompatible with the API operation"
/// (`smithy_query(query="doc",shape="InvalidKeyUsageException")`).
fn invalid_key_usage(message: impl Into<String>) -> EncapsulateError {
    EncapsulateErrorModeled::InvalidKeyUsageException(InvalidKeyUsageException {
        message: Some(message.into()),
    })
    .into()
}

/// The state store is missing, or the mechanism refused: a server fault rather than a bad
/// request.
fn internal(message: impl Into<String>) -> EncapsulateError {
    EncapsulateErrorModeled::KMSInternalException(KMSInternalException {
        message: Some(message.into()),
    })
    .into()
}
