// SPDX-License-Identifier: CC0-1.0

//! @op Decapsulate
//! IMPLEMENTED: runs the DECAPSULATION direction of the key-encapsulation mechanism the
//! named KMS key's `KeySpec` names, and answers with the shared secret the ciphertext
//! yields under that key's private half. This is the ONLY file to touch for this
//! operation.
//!
//! The key must be one that decapsulates: its `KeyUsage` has to be
//! `ENCAPSULATE_DECAPSULATE`, which only an `ML_KEM_*` key spec carries, and any other
//! key is "a KMS key [whose] KeyUsage ... is incompatible with the API operation"
//! (`smithy_query(query="doc",shape="InvalidKeyUsageException")`) — the same judgement
//! `Encapsulate` makes.
//!
//! What the mechanism does with a ciphertext is the mechanism's own rule, and
//! `crate::state::MlKemKeyPair` is where it is run rather than decided here: a ciphertext
//! this key encapsulated yields exactly the secret that encapsulation produced, and one
//! of the same parameter set made under ANOTHER key yields an unrelated secret rather
//! than a refusal — the implicit rejection of FIPS 203, which is what keeps a caller from
//! learning whether a ciphertext was meant for this key. A blob whose LENGTH is not the
//! one the key's parameter set gives is no ciphertext of that mechanism at all, and is
//! refused `InvalidCiphertextException` naming the member, the key spec and the two
//! lengths.
//!
//! ## Inputs
//!
//! * `KeyId` (KeyIdType, REQUIRED) — the key whose private half decapsulates.
//! * `SharedSecretCiphertextBlob` (CiphertextType, REQUIRED) — what `Encapsulate`
//!   returned.
//! * `EncapsulationAlgorithm` (EncapsulationAlgorithmSpec, REQUIRED) — the mechanism to
//!   run.
//! * `GrantTokens` (GrantTokenList) — grant tokens the caller draws permission from.
//! * `DryRun` (NullableBooleanType) — check the request and refuse.
//!
//! ## Outputs
//!
//! * `KeyId` (KeyIdType) — the key ARN of the key that decapsulated.
//! * `SharedSecret` (PlaintextType) — the secret the ciphertext decapsulated to.
//! * `EncapsulationAlgorithm` (EncapsulationAlgorithmSpec) — the mechanism that ran.
//!
//! ## Errors
//!
//! * `DependencyTimeoutException` (HTTP 503) — the system timed out.
//! * `DisabledException` (HTTP 400) — the KMS key is not enabled.
//! * `DryRunOperationException` (HTTP 400) — the `DryRun` option was set.
//! * `InvalidCiphertextException` (HTTP 400) — the ciphertext is not a valid one.
//! * `InvalidGrantTokenException` (HTTP 400) — a grant token is not valid.
//! * `InvalidKeyUsageException` (HTTP 400) — the key's `KeyUsage` does not decapsulate.
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
#[smithy_macros::operation_error("com.amazonaws.kms#Decapsulate")]
#[derive(Debug)]
pub enum DecapsulateErrorModeled {
    DependencyTimeoutException(DependencyTimeoutException),
    DisabledException(DisabledException),
    DryRunOperationException(DryRunOperationException),
    InvalidCiphertextException(InvalidCiphertextException),
    InvalidGrantTokenException(InvalidGrantTokenException),
    InvalidKeyUsageException(InvalidKeyUsageException),
    KeyUnavailableException(KeyUnavailableException),
    KMSInternalException(KMSInternalException),
    KMSInvalidStateException(KMSInvalidStateException),
    NotFoundException(NotFoundException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::decapsulate::DecapsulateError`).
pub type DecapsulateError = crate::scaffolded::Scaffolded<DecapsulateErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel.
impl From<DecapsulateErrorModeled> for DecapsulateError {
    fn from(modeled: DecapsulateErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `Decapsulate` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`].
pub struct Decapsulate<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> Decapsulate<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for Decapsulate<Eng>
{
    type Input = DecapsulateInput;
    type Output = DecapsulateOutput;
    type Error = DecapsulateError;

    const NAME: &'static str = "Decapsulate";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DECAPSULATE_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DECAPSULATE_OUTPUT_SCHEMA;
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

        // `Decapsulate` models no `InvalidArnException`, so a malformed ARN is reported
        // the same way as an identifier that names nothing: the key is not found.
        let key = match state.resolve(&region, &input.key_id) {
            crate::state::KeyRef::Found(key) => key,
            crate::state::KeyRef::UnauthorizedAccount => {
                return Err(crate::state::unauthorized_account(Self::NAME));
            }
            crate::state::KeyRef::NotFound | crate::state::KeyRef::MalformedArn => {
                return Err(DecapsulateErrorModeled::NotFoundException(NotFoundException {
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
                return Err(DecapsulateErrorModeled::DisabledException(DisabledException {
                    message: Some(crate::state::invalid_state_message(key)),
                })
                .into());
            }
            return Err(DecapsulateErrorModeled::KMSInvalidStateException(
                KMSInvalidStateException {
                    message: Some(crate::state::invalid_state_message(key)),
                },
            )
            .into());
        }

        // A token that is not one this service minted names no grant at all, so the
        // TOKEN "is not valid". Everything else about permission is the service's one
        // authorization decision (`crate::state::authorization`). `Decapsulate` takes no
        // `EncryptionContext`, so no encryption-context grant constraint applies to it.
        let named = match state.grants_for_tokens(&region, &tokens) {
            Ok(named) => named,
            Err(crate::state::GrantRefusal::MalformedToken) => {
                return Err(DecapsulateErrorModeled::InvalidGrantTokenException(
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

        // Which mechanism this key decapsulates under is exactly the key's own
        // encapsulation algorithms — the list that follows from its key spec — and a key
        // with no such list cannot decapsulate at all.
        let Some(algorithms) = key.algorithms_under(crate::state::KEY_USAGE_ENCAPSULATE_DECAPSULATE)
        else {
            return Err(invalid_key_usage(crate::state::wrong_key_usage_message(
                key,
                Self::NAME,
            )));
        };
        // "Specifies the key encapsulation algorithm used to recover the shared secret. The
        // only valid value is ML_KEM"
        // (`smithy_query(query="doc",shape="DecapsulateRequest",member="EncapsulationAlgorithm")`):
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

        let Some(pair) = key.material.ml_kem() else {
            return Err(DecapsulateErrorModeled::KeyUnavailableException(
                KeyUnavailableException {
                    message: Some(crate::state::invalid_state_message(key)),
                },
            )
            .into());
        };
        // A ciphertext of the mechanism is a fixed number of bytes, decided by the key's
        // own parameter set, so a blob of any other length is not a ciphertext of this key
        // at all: "the specified ciphertext ... is corrupted, missing, or otherwise
        // invalid" (`smithy_query(query="doc",shape="InvalidCiphertextException")`). The
        // refusal names the member, the key spec that decides the length, the length that
        // spec requires and the length the request carried — it says nothing about the
        // CONTENT of the blob, which is the mechanism's business below.
        let submitted = input.shared_secret_ciphertext_blob.len();
        if submitted != pair.ciphertext_len() {
            return Err(invalid_ciphertext(
                pair.ciphertext_length_mismatch_message(submitted),
            ));
        }

        // "The request was rejected because the DryRun parameter was specified"
        // (`smithy_query(query="doc",shape="DryRunOperationException")`): a dry run checks
        // the request and then always fails, so it is answered AFTER every check above
        // and BEFORE the mechanism runs.
        if input.dry_run == Some(true) {
            return Err(DecapsulateErrorModeled::DryRunOperationException(
                DryRunOperationException {
                    message: Some(crate::state::DRY_RUN_MESSAGE.to_string()),
                },
            )
            .into());
        }

        // What a ciphertext of the right length yields is the MECHANISM's rule: the secret
        // this key encapsulated for one it produced, and an unrelated secret for one made
        // under another key. Nothing here compares the ciphertext to anything first.
        let Some(shared_secret) = pair.decapsulate(&input.shared_secret_ciphertext_blob) else {
            return Err(internal("the shared secret could not be decapsulated"));
        };

        // KMS answers with the key ARN, not the identifier the request used.
        let key_arn = key.arn.clone();
        let key_id = key.key_id.clone();

        // The key was just used in a successful cryptographic operation, which is what
        // `GetKeyLastUsage` reports once the tracking has produced the record.
        state.record_last_usage(&region, &key_id, Self::NAME);

        Ok(DecapsulateOutput {
            key_id: Some(key_arn),
            shared_secret: Some(shared_secret),
            encapsulation_algorithm: Some(algorithm),
        })
    }
}

/// The bytes are not a ciphertext of the named key's parameter set.
///
/// The refusal names what is wrong with the blob — the member, the key spec that decides
/// a ciphertext's length, and the two lengths — and nothing about its content: whether a
/// well-formed ciphertext was MEANT for this key is what the mechanism's implicit
/// rejection withholds, so no refusal may disclose it either.
fn invalid_ciphertext(message: impl Into<String>) -> DecapsulateError {
    DecapsulateErrorModeled::InvalidCiphertextException(InvalidCiphertextException {
        message: Some(message.into()),
    })
    .into()
}

/// Refuse a key that cannot decapsulate: "the KeyUsage value of the KMS key is
/// incompatible with the API operation"
/// (`smithy_query(query="doc",shape="InvalidKeyUsageException")`).
fn invalid_key_usage(message: impl Into<String>) -> DecapsulateError {
    DecapsulateErrorModeled::InvalidKeyUsageException(InvalidKeyUsageException {
        message: Some(message.into()),
    })
    .into()
}

/// The state store is missing, or the mechanism refused: a server fault rather than a bad
/// request.
fn internal(message: impl Into<String>) -> DecapsulateError {
    DecapsulateErrorModeled::KMSInternalException(KMSInternalException {
        message: Some(message.into()),
    })
    .into()
}
