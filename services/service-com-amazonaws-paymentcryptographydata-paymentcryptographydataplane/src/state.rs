// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-paymentcryptographydata-paymentcryptographydataplane service.
//!
//! This service holds no keys of its own. A Payment Cryptography key is minted and
//! kept by the CONTROL plane (`CreateKey`, `ImportKey`, `CreateAlias`), and that
//! record is the one resource every operation here reads: [`resolve_key`] resolves a
//! `KeyIdentifier` — a key ARN, an alias ARN or a bare `alias/<name>` — against that
//! service's own store, so an alias and the ARN it is bound to reach the same key,
//! the `KeyArn` an operation answers with is always the canonical one that store
//! holds, the key check value is the one `GetKey` reports, and an identifier naming
//! no key is `ResourceNotFoundException` rather than a key this service invented.
//!
//! What this service does keep is each key's WORKING MATERIAL: the bytes its
//! computations run under, derived once per key from what that key publishes (see
//! [`crypto::working_material`]) and kept here, so every later call under the same
//! key — in any of the operations — runs under the same bytes. That is what makes a
//! ciphertext decryptable, a MAC verifiable and a card verification value
//! re-checkable.

use std::collections::BTreeMap;

use ::service_com_amazonaws_paymentcryptography_paymentcryptographycontrolplane::state as key_store;

pub mod crypto;

/// Per-service state for com-amazonaws-paymentcryptographydata-paymentcryptographydataplane. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsPaymentcryptographydataPaymentcryptographydataplaneState {
    /// The working material of each key this service has run a computation under, by
    /// the canonical key ARN the control plane holds that key under.
    ///
    /// A `BTreeMap` so the store walks one stable order, and private so the only way
    /// an entry is written is [`Self::working_material`] — which derives it from the
    /// key's own published facts and never regenerates one that is already here.
    working_material: BTreeMap<String, WorkingMaterial>,
}

/// A key's working material, whose `Debug` reports its length and never its bytes.
///
/// The redaction lives on the type rather than on one `Debug` impl of the store, so
/// neither a field added later nor a caller formatting the value on its own can put
/// key bytes in a log or a test failure.
#[derive(Clone, PartialEq, Eq)]
struct WorkingMaterial(Vec<u8>);

impl ::std::fmt::Debug for WorkingMaterial {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        write!(f, "<redacted {} bytes of key material>", self.0.len())
    }
}

impl ComAmazonawsPaymentcryptographydataPaymentcryptographydataplaneState {
    /// The working material of the key `facts` describes, derived on the first call
    /// that needs it and read from the store on every later one.
    ///
    /// `None` when the key's algorithm names no symmetric computation this service
    /// performs.
    fn working_material(&mut self, facts: &KeyFacts) -> Option<Vec<u8>> {
        if let Some(material) = self.working_material.get(&facts.key_arn) {
            return Some(material.0.clone());
        }
        let material = crypto::working_material(
            &facts.key_algorithm,
            &facts.key_arn,
            &facts.key_check_value,
        )?;
        self.working_material
            .insert(facts.key_arn.clone(), WorkingMaterial(material.clone()));
        Some(material)
    }
}

/// What the control plane's record of a key says about it, as this service reads it.
#[derive(Debug, Clone)]
struct KeyFacts {
    key_arn: String,
    key_check_value: String,
    key_algorithm: String,
    key_usage: String,
    modes: KeyModes,
}

/// The modes of use a key carries, as the operations here judge them.
///
/// From the control plane's `KeyModesOfUse`: each operation's own documentation names
/// the mode it needs ("the key must have KeyModesOfUse set to Encrypt", "set to
/// Generate", "set to Generate and Verify"), and `NoRestrictions` is the mode that
/// places no restriction on how the key is used.
#[derive(Debug, Clone, Copy)]
pub struct KeyModes {
    encrypt: bool,
    decrypt: bool,
    generate: bool,
    verify: bool,
    no_restrictions: bool,
}

impl KeyModes {
    /// Whether the key may be used to encrypt.
    #[must_use]
    pub fn encrypt(&self) -> bool {
        self.encrypt || self.no_restrictions
    }

    /// Whether the key may be used to decrypt.
    #[must_use]
    pub fn decrypt(&self) -> bool {
        self.decrypt || self.no_restrictions
    }

    /// Whether the key may be used to generate a value.
    #[must_use]
    pub fn generate(&self) -> bool {
        self.generate || self.no_restrictions
    }

    /// Whether the key may be used to verify a value.
    #[must_use]
    pub fn verify(&self) -> bool {
        self.verify || self.no_restrictions
    }
}

/// One key as an operation here uses it: what it reports about itself, and the
/// working material its computations run under.
#[derive(Debug)]
pub struct ResolvedKey {
    /// The canonical `KeyArn` the control plane holds this key under — what every
    /// operation answers with, whatever identifier the request named.
    pub key_arn: String,
    /// The key check value the control plane computed from this key's own secret,
    /// which is what `GetKey` reports for it.
    pub key_check_value: String,
    /// The key's modelled `KeyAlgorithm`.
    pub key_algorithm: String,
    /// The key's modelled `KeyUsage`, the TR-31 key block usage it was created for.
    pub key_usage: String,
    /// The key's modes of use.
    pub modes: KeyModes,
    /// The bytes this key's computations run under. Private, and in the redacted
    /// wrapper, so an operation reaches it only through [`Self::material`] and no
    /// formatter can put it anywhere.
    material: WorkingMaterial,
}

impl ResolvedKey {
    /// This key's working material, for one computation.
    #[must_use]
    pub fn material(&self) -> &[u8] {
        &self.material.0
    }
}

/// Why a `KeyIdentifier` could not be used.
#[derive(Debug, Clone)]
pub enum KeyRefusal {
    /// No key of that identifier exists: the modelled `ResourceNotFoundException`,
    /// whose `ResourceId` is the identifier the request named.
    NotFound(String),
    /// The key exists but cannot perform the request: the modelled
    /// `ValidationException`, worded by the message.
    NotUsable(String),
    /// A store this service reads is not reachable: the modelled
    /// `InternalServerException`, worded by the message.
    Unavailable(String),
}

/// The key `identifier` names, with the working material its computations run under.
///
/// `identifier` is anything the modelled `KeyArnOrKeyAliasType` admits, and it is
/// resolved by the control plane's own store, so an alias resolves to exactly the key
/// it is bound to and a key ARN has to match the stored one in every field — one
/// naming another region or account names no key here.
///
/// A key whose deletion has completed is gone, so it is reported absent; a key that
/// is disabled or waiting to be deleted exists and cannot be used, so it is refused
/// with the reason.
pub fn resolve_key<Eng: Send + Sync + 'static>(
    engine: &Eng,
    identifier: &str,
) -> Result<ResolvedKey, KeyRefusal> {
    let facts = key_facts(engine, identifier)?;
    // The control plane's guard is released above, before this service's own is
    // taken: no operation here ever holds both.
    let Some(state) = service_state(engine) else {
        return Err(KeyRefusal::Unavailable(
            "The Payment Cryptography data plane state store is unavailable.".to_string(),
        ));
    };
    let material = state.lock().working_material(&facts).ok_or_else(|| {
        KeyRefusal::NotUsable(format!(
            "KeyAlgorithm {} is not valid for this operation.",
            facts.key_algorithm
        ))
    })?;
    Ok(ResolvedKey {
        key_arn: facts.key_arn,
        key_check_value: facts.key_check_value,
        key_algorithm: facts.key_algorithm,
        key_usage: facts.key_usage,
        modes: facts.modes,
        material: WorkingMaterial(material),
    })
}

/// What the control plane's store says about the key `identifier` names.
fn key_facts<Eng: Send + Sync + 'static>(
    engine: &Eng,
    identifier: &str,
) -> Result<KeyFacts, KeyRefusal> {
    let Some(keys) = key_store::service_state(engine) else {
        return Err(KeyRefusal::Unavailable(
            "The Payment Cryptography key store is unavailable.".to_string(),
        ));
    };
    let now = key_store::now();
    let keys = keys.lock();
    let Some(key) = keys.key(identifier) else {
        return Err(KeyRefusal::NotFound(identifier.to_string()));
    };
    let state = key.effective_state(now);
    if state == key_store::KEY_STATE_DELETE_COMPLETE {
        return Err(KeyRefusal::NotFound(identifier.to_string()));
    }
    if state != key_store::KEY_STATE_CREATE_COMPLETE {
        return Err(KeyRefusal::NotUsable(format!(
            "Key {} is in KeyState {state} and cannot be used for cryptographic operations.",
            key.key_arn
        )));
    }
    if !key.enabled {
        return Err(KeyRefusal::NotUsable(format!(
            "Key {} is not enabled.",
            key.key_arn
        )));
    }
    let modes = &key.key_attributes.key_modes_of_use;
    Ok(KeyFacts {
        key_arn: key.key_arn.clone(),
        key_check_value: key.key_check_value.clone(),
        key_algorithm: key.key_attributes.key_algorithm.clone(),
        key_usage: key.key_attributes.key_usage.clone(),
        modes: KeyModes {
            encrypt: modes.encrypt,
            decrypt: modes.decrypt,
            generate: modes.generate,
            verify: modes.verify,
            no_restrictions: modes.no_restrictions,
        },
    })
}

/// The `KeyUsage` a card verification key carries, from the control plane's modelled
/// `KeyUsage`: the TR-31 card verification key, which is the CVK the card data
/// operations' own documentation requires.
pub const KEY_USAGE_CARD_VERIFICATION: &str = "TR31_C0_CARD_VERIFICATION_KEY";

/// The `ValidationData` length applied when the request names none: "The length of
/// the CVV or CSC to be generated. The default value is 3"
/// (`GenerateCardValidationData$ValidationDataLength`).
pub const DEFAULT_VALIDATION_DATA_LENGTH: i32 = 3;

/// The `Reason` a failed MAC verification carries, from the modelled
/// `VerificationFailedReason`.
pub const VERIFICATION_FAILED_INVALID_MAC: &str = "INVALID_MAC";

/// The `Reason` a failed card data verification carries, from the same shape.
pub const VERIFICATION_FAILED_INVALID_VALIDATION_DATA: &str = "INVALID_VALIDATION_DATA";

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsPaymentcryptographydataPaymentcryptographydataplaneState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsPaymentcryptographydataPaymentcryptographydataplaneState>)
}
