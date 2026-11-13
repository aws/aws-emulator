// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-paymentcryptography-paymentcryptographycontrolplane service.
//!
//! The service's primary resource is the **key**. Every key is stored exactly once,
//! keyed by its own `KeyArn` — the identity the model gives it (`Key` carries
//! `@references KeyResource{KeyIdentifier: KeyArn}`) — so each operation reads and
//! writes the one record. `aliases` is the second resource: a friendly name,
//! optionally bound to a key's ARN, which the modelled `KeyArnOrKeyAliasType` lets a
//! caller use anywhere a key ARN is accepted.
//!
//! Key material is REAL: a per-key secret of the length the key's algorithm calls
//! for is drawn when the key is minted, kept here, never returned, and is what the
//! key check value is computed from — so the KCV a caller reads back is derived from
//! stored material by the named KCV algorithm rather than being a fabricated string.
//!
//! The material is read in this module and in its children alone, and only by the two
//! computations that are defined in terms of it: the key check value here, and the
//! wrapped key an export answers with ([`key_export`]), which leaves the service
//! encrypted under another stored key rather than in the clear.

use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use cmac::Mac;
use hmac::Hmac;
use sha1::{Digest, Sha1};
use sha2::{Sha224, Sha256, Sha384, Sha512};
use x509_cert::attr::AttributeTypeAndValue;
use x509_cert::der::asn1::{Any, BitString, OctetString, SetOfVec};
use x509_cert::der::oid::AssociatedOid;
use x509_cert::der::{Decode, Encode, EncodePem, Tag as DerTag};
use x509_cert::ext::pkix::BasicConstraints;
use x509_cert::ext::Extension;
use x509_cert::name::{Name, RdnSequence, RelativeDistinguishedName};
use x509_cert::serial_number::SerialNumber;
use x509_cert::spki::{AlgorithmIdentifierOwned, ObjectIdentifier, SubjectPublicKeyInfoOwned};
use x509_cert::time::{Time, Validity as CertificateValidity};
use x509_cert::{Certificate, TbsCertificate, Version};

use crate::ops::types::{
    Alias, Key, KeyAttributes, KeySummary, ReplicationStatusType, Tag,
    SYMMETRIC_KEY_ALGORITHM_VALUES,
};

/// The block cipher, CMAC and CBC primitives every computation over key material in
/// this module and its children runs through.
mod block_cipher;
/// The initial DUKPT key an IPEK export derives from a base derivation key.
mod dukpt;
/// What `ExportKey` asks of this store, and the wrapped key it answers with.
mod key_export;
/// The ASC X9.143 (TR-31) key block an exported key is wrapped in.
mod key_block;

pub use key_export::{
    ExportRejection, KeyExport, WrappedKeyMaterial, KEY_CLASS_PUBLIC_KEY,
    KEY_USAGE_BASE_DERIVATION_KEY, KEY_USAGE_DIGITAL_SIGNATURE, WRAPPED_KEY_FORMAT_CRYPTOGRAM,
    WRAPPED_KEY_FORMAT_TR31, WRAPPING_KEY_ALGORITHMS, WRAPPING_KEY_USAGES,
};

/// The ARN service namespace for this service (`arnNamespace` / sigv4 name
/// `payment-cryptography`, from OPERATIONS.md's wire-protocol section).
pub const ARN_SERVICE: &str = "payment-cryptography";

/// The resource portion a key ARN carries before its id, from the modelled `KeyArn`
/// pattern `^arn:aws:payment-cryptography:…:key/[0-9a-zA-Z]{16,64}$`.
pub const KEY_ARN_RESOURCE_PREFIX: &str = "key/";

/// The prefix every alias name carries, from the modelled `AliasName` pattern
/// `^alias/[a-zA-Z0-9/_-]+$`.
pub const ALIAS_NAME_PREFIX: &str = "alias/";

/// The region used when a request carries no SigV4 credential scope this service can
/// mint ARNs for; the emulator's documented default region.
pub const DEFAULT_REGION: &str = "us-east-1";

/// Characters a generated key id is built from.
///
/// A subset of the modelled `KeyArn` id segment (`[0-9a-zA-Z]{16,64}`): the
/// lowercase-and-digit form the service's own key ids carry.
const KEY_ID_ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";

/// Length of the id segment of a generated key ARN.
///
/// 16 is the minimum the modelled `KeyArn` pattern admits and the length the
/// service's own key ids carry. Together with [`MIN_REGION_LEN`] it keeps every
/// minted ARN inside `KeyArn`'s 70..150 length bound.
pub const KEY_ID_LEN: usize = 16;

/// Shortest region identifier a minted ARN will carry.
///
/// The modelled `Region` pattern (`^[a-z]{2}-[a-z]{1,16}-[0-9]+$`) admits strings as
/// short as six characters, but the shortest published AWS region identifier is nine
/// (`us-east-1`) and a six-character one would build a `KeyArn` of 69 characters —
/// below that shape's modelled 70-character minimum.
const MIN_REGION_LEN: usize = 9;

/// Longest region identifier a minted ARN will carry, which keeps the ARN inside
/// `KeyArn`'s 150-character maximum with room to spare.
const MAX_REGION_LEN: usize = 24;

/// How many generated ids a single mint tries before it falls back to the enumerated
/// ids below.
///
/// A bound rather than a loop until one is free: with [`MAX_KEYS`] records and
/// `36^16` ids the first draw collides with vanishing probability, so this many
/// failures means the generator is not producing fresh ids at all (a stopped clock,
/// say) and the enumerated fallback is the one that terminates. The exact figure only
/// has to be far above the collision rate and small enough to cost nothing.
const MAX_KEY_ID_ATTEMPTS: usize = 1024;

/// The waiting period `DeleteKey` applies when the request omits `DeleteKeyInDays`.
///
/// From that member's own documentation: "The waiting period for key deletion. The
/// default value is seven days."
pub const DEFAULT_DELETE_KEY_IN_DAYS: i32 = 7;

/// A day, the unit `DeleteKeyInDays` counts in.
pub const ONE_DAY: Duration = Duration::from_secs(24 * 60 * 60);

/// The published bounds of `DeleteKeyInDays`, from
/// `smithy_query(query="operation",operation="DeleteKey")`, which gives that member
/// `range {min: 3, max: 180}`.
pub const MIN_DELETE_KEY_IN_DAYS: i32 = 3;
/// See [`MIN_DELETE_KEY_IN_DAYS`].
pub const MAX_DELETE_KEY_IN_DAYS: i32 = 180;

/// The instant a key scheduled for deletion `days` from `now` becomes deletable, or
/// `None` when `days` is outside the published 3..180 range or the arithmetic would
/// not fit.
///
/// The modelled range means the centralized walk already refuses an out-of-range
/// `DeleteKeyInDays` before a handler runs, but this function does not rely on that:
/// the value is request-controlled and is multiplied, so the bound is checked here
/// and the multiply and add are both checked. A handler maps `None` to the modelled
/// `ValidationException` rather than letting the arithmetic panic or wrap.
pub fn delete_pending_instant(now: SystemTime, days: i32) -> Option<SystemTime> {
    if !(MIN_DELETE_KEY_IN_DAYS..=MAX_DELETE_KEY_IN_DAYS).contains(&days) {
        return None;
    }
    let waiting = ONE_DAY.checked_mul(u32::try_from(days).ok()?)?;
    now.checked_add(waiting)
}

/// `KeyState` values, from `smithy_query(query="shape",shape="KeyState")`.
pub const KEY_STATE_CREATE_COMPLETE: &str = "CREATE_COMPLETE";
/// See [`KEY_STATE_CREATE_COMPLETE`].
pub const KEY_STATE_DELETE_PENDING: &str = "DELETE_PENDING";
/// See [`KEY_STATE_CREATE_COMPLETE`].
pub const KEY_STATE_DELETE_COMPLETE: &str = "DELETE_COMPLETE";

/// The `KeyOrigin` of a key this service generated, from that member's own
/// documentation: "For keys created within Amazon Web Services Payment
/// Cryptography, the value is `AWS_PAYMENT_CRYPTOGRAPHY`."
pub const KEY_ORIGIN_AWS: &str = "AWS_PAYMENT_CRYPTOGRAPHY";

/// `KeyReplicationState` values, from
/// `smithy_query(query="shape",shape="KeyReplicationState")`.
pub const KEY_REPLICATION_STATE_IN_PROGRESS: &str = "IN_PROGRESS";
/// See [`KEY_REPLICATION_STATE_IN_PROGRESS`].
pub const KEY_REPLICATION_STATE_SYNCHRONIZED: &str = "SYNCHRONIZED";
/// See [`KEY_REPLICATION_STATE_IN_PROGRESS`].
pub const KEY_REPLICATION_STATE_DELETE_IN_PROGRESS: &str = "DELETE_IN_PROGRESS";

/// The `MultiRegionKeyType` of a key that has replication regions of its own.
///
/// From that member's own documentation: a key "is a Primary Region key (PRK)
/// (which can be replicated to other Amazon Web Services Regions) or a Replica
/// Region key (RRK) (which is a copy of a PRK in another Region)". Every key this
/// store holds is one a caller created here and whose replicas are reported through
/// its own `ReplicationStatus`, so it is the primary of its own hierarchy.
pub const MULTI_REGION_KEY_TYPE_PRIMARY: &str = "PRIMARY";

/// How long replication to a region takes to settle, in either direction.
///
/// `ReplicationStatusType.Status` names both the states where the work is in flight
/// (`IN_PROGRESS` while a replica is being made, `DELETE_IN_PROGRESS` while one is
/// being torn down) and the one where it has settled (`SYNCHRONIZED`), so the move
/// between them is a transition the CLOCK owes: it falls due this long after the call
/// that asked for it and is derived from the stored instant, never slept for. The
/// span is deliberately non-zero, so a caller that reads the key straight after the
/// call still observes the in-flight state the real service reports.
pub const REPLICATION_SPAN: Duration = Duration::from_secs(3);

/// The largest page any list operation in this service builds.
///
/// The maximum of the modelled `MaxResults` range (1..100), which the centralized
/// walk already refuses values outside of — but no handler depends on that: each
/// clamps into 1..=this, so the page it allocates is sized by the server.
pub const MAX_PAGE_SIZE: usize = 100;

/// The page size a list operation serves when the request omits `MaxResults`.
///
/// From that member's own documentation, which every list operation in this service
/// repeats verbatim: "This value is optional. If you include a value, it must be
/// between 1 and 100, inclusive. If you do not include a value, it defaults to 50."
pub const DEFAULT_PAGE_SIZE: usize = 50;

/// The most keys this emulator will hold.
///
/// Emulator resource guard, NOT an AWS quota: keys are created by a caller and never
/// reclaimed (a deleted key keeps its record, in `DELETE_COMPLETE`), so the store
/// needs a bound. A create past it is refused with the modelled
/// `ServiceQuotaExceededException`.
pub const MAX_KEYS: usize = 1000;

/// The most aliases this emulator will hold. Emulator resource guard, NOT an AWS
/// quota, for the same reason as [`MAX_KEYS`].
pub const MAX_ALIASES: usize = 1000;

/// The most tags one key may carry: the maximum of the modelled `Tags` list length
/// (0..200), named so a handler can state it.
pub const MAX_TAGS: usize = 200;

/// The most default key replication regions this emulator will hold for the account.
///
/// Emulator resource guard, NOT an AWS quota, for the same reason as [`MAX_KEYS`]: the
/// set is grown by caller-supplied region strings and shrinks only when a caller
/// disables one by name, and every enable, disable and read answers the whole set. The
/// figure only has to be comfortably above the number of Regions AWS publishes, so no
/// real configuration meets it.
pub const MAX_ENABLED_REPLICATION_REGIONS: usize = 64;

/// The most replication regions this emulator will hold for ONE key.
///
/// Emulator resource guard, NOT an AWS quota, for the same reason as
/// [`MAX_ENABLED_REPLICATION_REGIONS`]: a key's replication configuration is grown by
/// caller-supplied region strings (`CreateKey`, `AddKeyReplicationRegions`) and every
/// read projects an entry per region, so the configuration needs a bound. The figure
/// only has to be comfortably above the number of Regions AWS publishes.
pub const MAX_KEY_REPLICATION_REGIONS: usize = 64;

/// The tag-key namespace AWS reserves for itself, in every service: a caller may not
/// set a tag whose key starts with it.
pub const RESERVED_TAG_PREFIX: &str = "aws:";

// ---------------------------------------------------------------------------
// TR-34 export parameters
// ---------------------------------------------------------------------------

/// The prefix every export token carries, from the modelled `ExportTokenId` pattern
/// `^(export-token-[0-9a-zA-Z]{16,64})?$`.
pub const EXPORT_TOKEN_PREFIX: &str = "export-token-";

/// Length of the id segment of a generated export token: the minimum that pattern
/// admits.
pub const EXPORT_TOKEN_ID_LEN: usize = 16;

/// How long an export token and its signing key certificate stay valid.
///
/// From `GetParametersForExport`'s own documentation: "The export token expires in 30
/// days."
pub const EXPORT_PARAMETERS_VALIDITY: Duration = Duration::from_secs(30 * 24 * 60 * 60);

/// How much validity a stored export token must have left for
/// `ReuseLastGeneratedToken` to answer with it instead of minting a new one.
///
/// From that member's own documentation: "If set to true and a valid export token
/// exists for the same key material type and signing key algorithm with at least 7
/// days of remaining validity, the existing token and signing key certificate are
/// returned."
pub const EXPORT_TOKEN_REUSE_REMAINING: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// The only `KeyAlgorithm` a TR-34 signing key certificate is issued for, from
/// `SigningKeyAlgorithm`'s own documentation: "RSA_2048 is the only signing key
/// algorithm allowed."
pub const TR34_SIGNING_KEY_ALGORITHM: &str = "RSA_2048";

/// The most export parameter sets this emulator will hold at once.
///
/// Emulator resource guard, NOT an AWS quota: each call may mint a set, so the store
/// needs a bound. Sets the clock has already expired do not count against it.
pub const MAX_EXPORT_PARAMETERS: usize = 100;

/// One TR-34 export parameter set: the export token a later `ExportKey` names, the
/// signing key certificate that signs the wrapped key inside the TR-34 payload, and
/// the instant both expire at.
#[derive(Debug, Clone)]
pub struct ExportParameters {
    /// The modelled `ExportTokenId` naming this set; also its key in the store.
    pub export_token: String,
    /// The `KeyMaterialType` the set was minted for, which with
    /// [`ExportParameters::signing_key_algorithm`] is what a reuse request matches on.
    pub key_material_type: String,
    /// The `KeyAlgorithm` of the signing key.
    pub signing_key_algorithm: String,
    /// The signing key certificate, as `SigningKeyCertificate` reports it.
    pub signing_key_certificate: String,
    /// The certificate of the root authority that signed it, as
    /// `SigningKeyCertificateChain` reports it.
    pub signing_key_certificate_chain: String,
    /// When this set was minted.
    pub created: SystemTime,
    /// When it expires, as `ParametersValidUntilTimestamp` reports it.
    pub valid_until: SystemTime,
    /// The private half of the signing key pair, PKCS#8 DER.
    ///
    /// Held because it is what signs the wrapped key inside the TR-34 key block a
    /// later `ExportKey` builds; never returned and never logged, and its `Debug` is
    /// redacted like a key's own material. Private to this module, so every set is
    /// minted through [`ComAmazonawsPaymentcryptographyPaymentcryptographycontrolplaneState::remember_export_parameters`]
    /// and no handler can store one whose certificate is not the certificate of this
    /// key.
    signing_key_pkcs8: KeyMaterial,
}

impl ExportParameters {
    /// The private half of this set's signing key pair, PKCS#8 DER: what signs the
    /// wrapped key inside the TR-34 key block a later `ExportKey` builds, and the key
    /// whose public half the signing key certificate carries.
    pub fn signing_key_pkcs8(&self) -> &[u8] {
        self.signing_key_pkcs8.bytes()
    }

    /// Whether this set is still valid at `now`.
    pub fn is_valid(&self, now: SystemTime) -> bool {
        now < self.valid_until
    }

    /// Whether this set still has `remaining` of its validity left at `now`, which is
    /// what `ReuseLastGeneratedToken` requires of the set it answers with.
    pub fn has_remaining(&self, now: SystemTime, remaining: Duration) -> bool {
        now.checked_add(remaining)
            .is_some_and(|deadline| deadline <= self.valid_until)
    }
}

// ---------------------------------------------------------------------------
// The stored key
// ---------------------------------------------------------------------------

/// One region a key is replicated to: when replication there was asked for, and when
/// its teardown was, if it has been.
///
/// Instants rather than a status string, because `ReplicationStatusType.Status` moves
/// on its own once the work falls due ([`REPLICATION_SPAN`]) — so the state a reader
/// reports is derived from these and no reader has to write.
#[derive(Debug, Clone)]
pub struct ReplicaRegion {
    /// The instant the call that asked for replication to this region ran: where the
    /// `IN_PROGRESS` span starts.
    pub replication_requested: SystemTime,
    /// The instant the call that asked for this region's replica to be torn down ran,
    /// if one has: where the `DELETE_IN_PROGRESS` span starts, after which this
    /// region is no longer one of the key's.
    pub removal_requested: Option<SystemTime>,
}

impl ReplicaRegion {
    /// A region whose replication starts at `now`.
    pub fn started(now: SystemTime) -> Self {
        Self {
            replication_requested: now,
            removal_requested: None,
        }
    }

    /// This region's `ReplicationStatusType.Status` at `now`, or `None` once its
    /// replica has finished being torn down and the region is no longer one this key
    /// replicates to.
    ///
    /// `key_is_deleting` is the key itself being scheduled for deletion, which takes
    /// its replicas with it: every region of a key waiting to be deleted reports the
    /// teardown that deletion started.
    fn status(&self, now: SystemTime, key_is_deleting: bool) -> Option<&'static str> {
        if let Some(requested) = self.removal_requested {
            if replication_span_elapsed(requested, now) {
                return None;
            }
            return Some(KEY_REPLICATION_STATE_DELETE_IN_PROGRESS);
        }
        if key_is_deleting {
            return Some(KEY_REPLICATION_STATE_DELETE_IN_PROGRESS);
        }
        Some(if replication_span_elapsed(self.replication_requested, now) {
            KEY_REPLICATION_STATE_SYNCHRONIZED
        } else {
            KEY_REPLICATION_STATE_IN_PROGRESS
        })
    }
}

/// Whether the replication work started at `since` has fallen due by `now`.
fn replication_span_elapsed(since: SystemTime, now: SystemTime) -> bool {
    since
        .checked_add(REPLICATION_SPAN)
        .is_some_and(|due| now >= due)
}

/// One stored key: every member `Key` and `KeySummary` report, plus the secret the
/// key check value is computed from.
#[derive(Debug, Clone)]
pub struct StoredKey {
    /// This record's identity and the store's key.
    pub key_arn: String,
    /// Immutable after creation, as `KeyAttributes`' own documentation says.
    pub key_attributes: KeyAttributes,
    /// Hex, computed from [`StoredKey::material`] by `key_check_value_algorithm`.
    pub key_check_value: String,
    pub key_check_value_algorithm: String,
    pub enabled: bool,
    pub exportable: bool,
    /// The state as last written. A `DELETE_PENDING` key whose
    /// `delete_pending_timestamp` has passed reports `DELETE_COMPLETE` — see
    /// [`StoredKey::effective_state`] — without a reader rewriting this field.
    pub key_state: String,
    pub key_origin: String,
    pub create_timestamp: SystemTime,
    pub usage_start_timestamp: Option<SystemTime>,
    pub usage_stop_timestamp: Option<SystemTime>,
    pub delete_pending_timestamp: Option<SystemTime>,
    /// The waiting period, in days, that the call which scheduled this key's
    /// deletion asked for.
    ///
    /// Not reported on the wire — `Key` models no such member — but stored because
    /// it is what tells an idempotent `DeleteKey` RETRY (the same waiting period, so
    /// the same intent) from a second request asking for a DIFFERENT period, which
    /// would move a deletion date the caller was already promised and is refused
    /// with the modelled `ConflictException`. The instant itself cannot answer that
    /// question: it is computed from the clock, so an identical retry a second later
    /// would not match it.
    pub delete_key_in_days: Option<i32>,
    pub derive_key_usage: Option<String>,
    /// The regions this key is replicated to, each with the instants the clock owes
    /// its replication state from.
    ///
    /// Written by the create that named a replication configuration and by the
    /// operations that change one afterwards (`AddKeyReplicationRegions`,
    /// `RemoveKeyReplicationRegions`); every member a reader reports about
    /// replication — `MultiRegionKeyType` and `ReplicationStatus` — is projected from
    /// it by [`StoredKey::replication_status`], so a key replicates to exactly the
    /// regions some call asked for and to nothing else.
    pub replication_regions: BTreeMap<String, ReplicaRegion>,
    /// Whether this key replicates to the account's DEFAULT replication regions
    /// rather than to a configuration of its own — true for a create that named no
    /// `ReplicationRegions`, which is what `UsingDefaultReplicationRegions` reports,
    /// and false from the moment a call writes a configuration onto this key.
    pub uses_default_replication_regions: bool,
    pub tags: BTreeMap<String, String>,
    /// The generated secret. Never returned and never logged: it is the source of
    /// the key check value and of nothing else in this slice. Its own `Debug` is
    /// redacted, so no formatter — a `tracing` field, a test assertion message, a
    /// derived `Debug` on the store — can put key material anywhere.
    material: KeyMaterial,
}

/// A key's generated secret, whose `Debug` reports its length and never its bytes.
///
/// The redaction lives on the type rather than on one `Debug` impl of
/// [`StoredKey`], so a field added to that record later cannot reintroduce the leak,
/// and neither can a caller that formats the material on its own.
#[derive(Clone, PartialEq, Eq)]
pub struct KeyMaterial(Vec<u8>);

impl KeyMaterial {
    /// The bytes, for the one computation allowed to see them: the key check value.
    fn bytes(&self) -> &[u8] {
        &self.0
    }
}

impl ::std::fmt::Debug for KeyMaterial {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        write!(f, "<redacted {} bytes of key material>", self.0.len())
    }
}

impl StoredKey {
    /// This key's check value under `kcv_algorithm`, computed from its own stored
    /// material.
    ///
    /// This is the only reader of [`StoredKey::material`], and the only way a check
    /// value is ever produced: the stored `key_check_value` is what this answered
    /// when the key was minted, so a reported KCV is always a function of the bytes
    /// this record holds. `None` when `kcv_algorithm` has no definition for a key of
    /// this algorithm.
    pub fn check_value_under(&self, kcv_algorithm: &str) -> Option<String> {
        key_check_value(
            kcv_algorithm,
            &self.key_attributes.key_algorithm,
            self.material.bytes(),
        )
    }

    /// The key's state as of `now`.
    ///
    /// A key scheduled for deletion becomes `DELETE_COMPLETE` when its
    /// `DeletePendingTimestamp` passes — a transition the clock owes, due at the
    /// instant `DeleteKey` stored, so it is derived here rather than written by
    /// whichever caller happens to read the key next.
    pub fn effective_state(&self, now: SystemTime) -> &str {
        match (self.key_state.as_str(), self.delete_pending_timestamp) {
            (KEY_STATE_DELETE_PENDING, Some(due)) if now >= due => KEY_STATE_DELETE_COMPLETE,
            (state, _) => state,
        }
    }

    /// `DeleteTimestamp`: present only once the deletion window has elapsed, as that
    /// member's documentation says ("present only when the `KeyState` is
    /// `DELETE_COMPLETE`").
    fn delete_timestamp_output(&self, now: SystemTime) -> Option<SystemTime> {
        if self.effective_state(now) != KEY_STATE_DELETE_COMPLETE {
            return None;
        }
        self.delete_pending_timestamp
    }

    /// `DeletePendingTimestamp`: present only while the key is still waiting to be
    /// deleted, as that member's documentation says.
    fn delete_pending_timestamp_output(&self, now: SystemTime) -> Option<SystemTime> {
        if self.effective_state(now) != KEY_STATE_DELETE_PENDING {
            return None;
        }
        self.delete_pending_timestamp
    }

    /// Whether this key is in the active state at `now`: the state a key is in once
    /// it has been created and before anything scheduled its deletion.
    ///
    /// What the operations that change a created key require of it — "The key must be
    /// in an active state to add Replication Regions"
    /// (`AddKeyReplicationRegions`' own documentation) — so a key on its way out is
    /// refused rather than quietly mutated.
    pub fn is_active(&self, now: SystemTime) -> bool {
        self.effective_state(now) == KEY_STATE_CREATE_COMPLETE
    }

    /// `ReplicationStatus`: the replication state of each region this key is
    /// replicated to at `now`, in one stable order.
    ///
    /// Derived, never written: each region's state is a function of the instant the
    /// call that asked for it ran, of [`REPLICATION_SPAN`] and of whether the key
    /// itself is on its way out — so a caller polling for a replica to settle sees it
    /// settle without anything else running, and a read leaves the record alone.
    /// Empty for a key nothing has replicated, and for one the clock has finished
    /// deleting: its replicas went with it.
    pub fn replication_status(&self, now: SystemTime) -> BTreeMap<String, &'static str> {
        let state = self.effective_state(now);
        if state == KEY_STATE_DELETE_COMPLETE {
            return BTreeMap::new();
        }
        let key_is_deleting = state == KEY_STATE_DELETE_PENDING;
        self.replication_regions
            .iter()
            .filter_map(|(region, replica)| {
                replica
                    .status(now, key_is_deleting)
                    .map(|status| (region.clone(), status))
            })
            .collect()
    }

    /// `MultiRegionKeyType`: this key's role in the Multi-Region key hierarchy at
    /// `now`, or `None` for a key that replicates nowhere and so is not a
    /// Multi-Region key at all.
    pub fn multi_region_key_type(&self, now: SystemTime) -> Option<&'static str> {
        (!self.replication_status(now).is_empty()).then_some(MULTI_REGION_KEY_TYPE_PRIMARY)
    }

    /// Stop using this key's material for cryptographic operations, as of `now`.
    ///
    /// `Enabled` "specifies whether the key is enabled" and `UsageStopTimestamp` is
    /// "the date and time after which Amazon Web Services Payment Cryptography will
    /// stop using the key material for cryptographic operations", so the call that
    /// makes the key inactive is what clears the one and stamps the other.
    pub fn stop_usage(&mut self, now: SystemTime) {
        self.enabled = false;
        self.usage_stop_timestamp = Some(now);
    }

    /// Start using this key's material for cryptographic operations again, as of
    /// `now`.
    ///
    /// The mirror of [`StoredKey::stop_usage`]: the key is enabled, usage starts at
    /// this instant, and the stop instant is cleared because nothing stops the key's
    /// use any more.
    pub fn start_usage(&mut self, now: SystemTime) {
        self.enabled = true;
        self.usage_start_timestamp = Some(now);
        self.usage_stop_timestamp = None;
    }

    /// Add or replace `tags` on this key, leaving every tag it carries under another
    /// key where it is.
    ///
    /// From `TagResource`'s own documentation: "To add a tag, specify a new tag key
    /// and a tag value. To edit a tag, specify an existing tag key and a new tag
    /// value" — so this is a per-tag-key merge, not a replacement of the whole set.
    /// Nothing is written when the request is refused: the merged set is judged
    /// first, so a refused call leaves the key's tags exactly as they were.
    pub fn apply_tags(&mut self, tags: &[Tag]) -> Result<(), TagRejection> {
        let updates = tag_map(tags)?;
        let mut merged = self.tags.clone();
        merged.extend(updates);
        if merged.len() > MAX_TAGS {
            return Err(TagRejection::QuotaExceeded(format!(
                "a key cannot carry more than {MAX_TAGS} tags, and this request would \
                 leave it carrying {}",
                merged.len()
            )));
        }
        self.tags = merged;
        Ok(())
    }

    /// Remove every tag in `tag_keys` from this key and answer how many were
    /// removed.
    ///
    /// From `UntagResource`'s `TagKeys` documentation: "If the Amazon Web Services
    /// Payment Cryptography key doesn't have the specified tag key, Amazon Web
    /// Services Payment Cryptography doesn't throw an exception or return a
    /// response" — so a key this record does not carry removes nothing and is not a
    /// failure.
    pub fn remove_tags(&mut self, tag_keys: &[String]) -> usize {
        tag_keys
            .iter()
            .filter(|tag_key| self.tags.remove(*tag_key).is_some())
            .count()
    }

    /// Replicate this key to `regions` from `now`, and answer `None` — storing
    /// nothing — when that would take it past [`MAX_KEY_REPLICATION_REGIONS`].
    ///
    /// A region this key already replicates to is left where it is, so a repeat does
    /// not restart a span the first call already owes; one whose replica is being torn
    /// down starts replicating again from this call.
    pub fn add_replication_regions(
        &mut self,
        regions: &[String],
        now: SystemTime,
    ) -> Option<()> {
        self.retire_removed_replicas(now);
        let would_hold = self
            .replication_regions
            .keys()
            .chain(regions.iter())
            .collect::<BTreeSet<&String>>()
            .len();
        if would_hold > MAX_KEY_REPLICATION_REGIONS {
            return None;
        }
        for region in regions {
            let replicating = self
                .replication_regions
                .get(region)
                .is_some_and(|replica| replica.removal_requested.is_none());
            if !replicating {
                self.replication_regions
                    .insert(region.clone(), ReplicaRegion::started(now));
            }
        }
        self.uses_default_replication_regions = false;
        Some(())
    }

    /// Tear down the replicas in `regions` from `now`, and answer the regions named
    /// that this key does not replicate to — storing nothing when there are any,
    /// since the caller named a configuration this key does not have.
    pub fn remove_replication_regions(
        &mut self,
        regions: &[String],
        now: SystemTime,
    ) -> Vec<String> {
        self.retire_removed_replicas(now);
        let unreplicated: Vec<String> = regions
            .iter()
            .filter(|region| !self.replication_regions.contains_key(*region))
            .cloned()
            .collect();
        if !unreplicated.is_empty() {
            return unreplicated;
        }
        for region in regions {
            if let Some(replica) = self.replication_regions.get_mut(region) {
                // The teardown already under way keeps the instant it started from,
                // so a repeat does not extend it.
                if replica.removal_requested.is_none() {
                    replica.removal_requested = Some(now);
                }
            }
        }
        self.uses_default_replication_regions = false;
        Vec::new()
    }

    /// Drop the regions whose replicas the clock has finished tearing down.
    ///
    /// Called only from the two operations that WRITE a replication configuration:
    /// [`StoredKey::replication_status`] already leaves a finished teardown out of
    /// what it reports, so no reader needs this — it keeps the record itself from
    /// holding a region the key no longer replicates to once a caller changes the
    /// configuration again.
    fn retire_removed_replicas(&mut self, now: SystemTime) {
        self.replication_regions.retain(|_, replica| {
            replica
                .removal_requested
                .is_none_or(|requested| !replication_span_elapsed(requested, now))
        });
    }

    /// This key as the modelled `Key` structure, every member read from what is
    /// stored.
    pub fn to_key(&self, now: SystemTime) -> Key {
        Key {
            key_arn: self.key_arn.clone(),
            key_attributes: self.key_attributes.clone(),
            key_check_value: self.key_check_value.clone(),
            key_check_value_algorithm: self.key_check_value_algorithm.clone(),
            enabled: self.enabled,
            exportable: self.exportable,
            key_state: self.effective_state(now).to_string(),
            key_origin: self.key_origin.clone(),
            create_timestamp: ::aws_smithy_types::DateTime::from(self.create_timestamp),
            usage_start_timestamp: self
                .usage_start_timestamp
                .map(::aws_smithy_types::DateTime::from),
            usage_stop_timestamp: self
                .usage_stop_timestamp
                .map(::aws_smithy_types::DateTime::from),
            delete_pending_timestamp: self
                .delete_pending_timestamp_output(now)
                .map(::aws_smithy_types::DateTime::from),
            delete_timestamp: self
                .delete_timestamp_output(now)
                .map(::aws_smithy_types::DateTime::from),
            derive_key_usage: self.derive_key_usage.clone(),
            multi_region_key_type: self
                .multi_region_key_type(now)
                .map(str::to_string),
            // `PrimaryRegion` names the Region of the primary key, which is what a
            // REPLICA key reports about the key it is a copy of. Every record this
            // store holds is a primary, reporting its replicas through
            // `ReplicationStatus` below, so there is no replica record to report a
            // primary region for.
            primary_region: None,
            replication_status: Some(self.replication_status(now))
                .filter(|status| !status.is_empty())
                .map(|status| {
                    status
                        .into_iter()
                        .map(|(region, state)| {
                            (
                                region,
                                ReplicationStatusType {
                                    status: state.to_string(),
                                    // `StatusMessage` carries "details about any
                                    // issues or progress updates": this replication
                                    // is proceeding as asked, so there are none.
                                    status_message: None,
                                },
                            )
                        })
                        .collect()
                }),
            // `UsingDefaultReplicationRegions`' own documentation: false means "the
            // key has a custom replication configuration that overrides the account
            // defaults", which is exactly a key whose create named its regions or
            // whose configuration a later call wrote.
            using_default_replication_regions: Some(self.uses_default_replication_regions),
            // Multi-Party Approval is a separate slice; this key went through none,
            // so it carries no MPA status.
            mpa_status: None,
        }
    }

    /// This key as the modelled `KeySummary` structure, every member read from what
    /// is stored.
    pub fn to_summary(&self, now: SystemTime) -> KeySummary {
        KeySummary {
            key_arn: self.key_arn.clone(),
            key_state: self.effective_state(now).to_string(),
            key_attributes: self.key_attributes.clone(),
            key_check_value: self.key_check_value.clone(),
            exportable: self.exportable,
            enabled: self.enabled,
            // The same projection `Key` carries, so a listed key and a fetched one
            // report the same multi-region role.
            multi_region_key_type: self.multi_region_key_type(now).map(str::to_string),
            // Absent for the same reason as on `Key`: see
            // [`StoredKey::to_key`].
            primary_region: None,
        }
    }
}

/// What `CreateKey` asks for, as the state module takes it.
#[derive(Debug, Clone, Default)]
pub struct NewKey {
    pub key_attributes: KeyAttributes,
    pub key_check_value_algorithm: Option<String>,
    pub exportable: bool,
    pub enabled: Option<bool>,
    pub tags: Vec<Tag>,
    pub derive_key_usage: Option<String>,
    pub replication_regions: Vec<String>,
}

/// Why a key could not be minted.
#[derive(Debug, Clone)]
pub enum MintRejection {
    /// The request is inadmissible; the message says why, naming the member.
    Invalid(String),
    /// The store is full; the message says so.
    QuotaExceeded(String),
}

/// Why a tag list could not be stored on a key.
///
/// Separate from [`MintRejection`] because a tag update is judged by the same two
/// rules whether it arrives with a create or on its own, and each handler answers
/// the modelled error its own operation declares for that class.
#[derive(Debug, Clone)]
pub enum TagRejection {
    /// The tag list is inadmissible; the message says why, naming the member.
    Invalid(String),
    /// Storing it would take the key past [`MAX_TAGS`]; the message says so.
    QuotaExceeded(String),
}

impl From<TagRejection> for MintRejection {
    fn from(rejection: TagRejection) -> Self {
        match rejection {
            TagRejection::Invalid(why) => MintRejection::Invalid(why),
            TagRejection::QuotaExceeded(why) => MintRejection::QuotaExceeded(why),
        }
    }
}

// ---------------------------------------------------------------------------
// The store
// ---------------------------------------------------------------------------

/// Per-service state for com-amazonaws-paymentcryptography-paymentcryptographycontrolplane. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsPaymentcryptographyPaymentcryptographycontrolplaneState {
    /// Every key, keyed by its `KeyArn`. A `BTreeMap` so `ListKeys` walks a stable,
    /// total order and a page token can name the ARN the next page starts at.
    pub keys: BTreeMap<String, StoredKey>,
    /// Alias name -> the `KeyArn` it is bound to, if any. A `BTreeMap` for the same
    /// reason.
    ///
    /// The binding is an `Option` because an alias can outlive its binding:
    /// `UpdateAlias` rebinds or clears one and `DeleteAlias` removes the alias, and
    /// both are still 501, so today only `CreateAlias` writes here and it always
    /// stores a bound alias — the operation's own documentation says an alias cannot
    /// be created without a key.
    pub aliases: BTreeMap<String, Option<String>>,
    /// The account's default key replication regions: the regions a key created
    /// without a replication configuration of its own is replicated to.
    ///
    /// `EnableDefaultKeyReplicationRegions` adds to this set and
    /// `DisableDefaultKeyReplicationRegions` removes from it, so it is a SET rather
    /// than a list: enabling a region twice enables it once, and
    /// `GetDefaultKeyReplicationRegions` answers it in one stable order.
    pub enabled_replication_regions: BTreeSet<String>,
    /// The TR-34 export parameter sets `GetParametersForExport` has handed out, keyed
    /// by the export token that names each one.
    pub export_parameters: BTreeMap<String, ExportParameters>,
    /// Mixed into generated key ids and key material, so two creates in the same
    /// nanosecond differ.
    counter: u64,
}

impl ComAmazonawsPaymentcryptographyPaymentcryptographycontrolplaneState {
    /// The `KeyArn` `identifier` names, accepting every form the modelled
    /// `KeyArnOrKeyAliasType` admits: a key ARN, an alias ARN, or a bare
    /// `alias/<name>`.
    ///
    /// A key ARN must equal the stored ARN in EVERY field, so one naming another
    /// partition, region or account resolves to nothing rather than to the key that
    /// happens to share its id. An alias resolves through [`Self::aliases`] to the
    /// key it is bound to, so an alias bound to nothing resolves to no key.
    pub fn resolve_key_arn(&self, identifier: &str) -> Option<String> {
        if let Some(name) = alias_name_from_identifier(identifier) {
            return self.aliases.get(&name).cloned().flatten();
        }
        self.keys
            .contains_key(identifier)
            .then(|| identifier.to_string())
    }

    /// The key `identifier` names, for a reporting operation.
    pub fn key(&self, identifier: &str) -> Option<&StoredKey> {
        let arn = self.resolve_key_arn(identifier)?;
        self.keys.get(&arn)
    }

    /// The key `identifier` names, for an operation the spec says mutates it.
    ///
    /// `key_arn` is this record's identity and the store's key, so it must never be
    /// reassigned through this handle.
    pub fn key_mut(&mut self, identifier: &str) -> Option<&mut StoredKey> {
        let arn = self.resolve_key_arn(identifier)?;
        self.keys.get_mut(&arn)
    }

    /// Mint a key in `region` from `request`, store it, and answer the stored
    /// record.
    ///
    /// This is where a key's material is drawn and its check value computed, so no
    /// caller can store a key whose KCV was not derived from its own material.
    pub fn mint_key(
        &mut self,
        region: &str,
        request: NewKey,
        now: SystemTime,
    ) -> Result<StoredKey, MintRejection> {
        // The request is judged before the store's capacity is, so an inadmissible
        // request earns the refusal it has actually earned whether or not the store
        // happens to be full. The capacity guard is applied at the one place that
        // grows the store, just before the insert below.
        let tags = tag_map(&request.tags)?;
        if let Some(why) = attribute_rejection(&request.key_attributes) {
            return Err(MintRejection::Invalid(why));
        }

        let key_algorithm = request.key_attributes.key_algorithm.clone();
        let material_len = material_len(&key_algorithm).ok_or_else(|| {
            MintRejection::Invalid(format!(
                "KeyAttributes.KeyAlgorithm {key_algorithm} is not an algorithm this \
                 service generates key material for"
            ))
        })?;
        // The algorithm in effect is the one the request named, or the one this
        // key's own algorithm dictates when it named none.
        let kcv_algorithm = match request.key_check_value_algorithm.clone() {
            Some(named) => named,
            None => default_kcv_algorithm(&key_algorithm)
                .ok_or_else(|| {
                    MintRejection::Invalid(format!(
                        "KeyAttributes.KeyAlgorithm {key_algorithm} has no default \
                         KeyCheckValueAlgorithm in this service"
                    ))
                })?
                .to_string(),
        };

        // The store's capacity is judged BEFORE any material is drawn, so a create
        // the store has no room for costs no key generation, and the only refusal
        // ordering left below is the one the request itself earns.
        if self.keys.len() >= MAX_KEYS {
            return Err(MintRejection::QuotaExceeded(format!(
                "this account already holds the maximum of {MAX_KEYS} keys"
            )));
        }

        // "New keys created in this account will automatically be replicated to
        // these regions unless explicitly configured otherwise during key creation"
        // (`EnabledReplicationRegions`' own documentation), so a create that named no
        // regions of its own takes the account's defaults as they stand now, and one
        // that named its own keeps them and overrides the defaults.
        let uses_default_replication_regions = request.replication_regions.is_empty();
        let requested_regions: Vec<String> = if uses_default_replication_regions {
            self.enabled_replication_regions.iter().cloned().collect()
        } else {
            request.replication_regions
        };
        // Replication to each of them starts with this call, so the key is a
        // Multi-Region primary from the moment it exists and its replicas settle a
        // span later, exactly as they do when a later call adds a region.
        let replication_regions: BTreeMap<String, ReplicaRegion> = requested_regions
            .into_iter()
            .map(|region| (region, ReplicaRegion::started(now)))
            .collect();
        if replication_regions.len() > MAX_KEY_REPLICATION_REGIONS {
            return Err(MintRejection::QuotaExceeded(format!(
                "a key cannot replicate to more than {MAX_KEY_REPLICATION_REGIONS} regions"
            )));
        }

        let material = self.fresh_material(material_len);
        // An id no stored key is using, so the insert below can only add a record.
        let key_id = self.fresh_key_id(region).ok_or_else(|| {
            MintRejection::QuotaExceeded(
                "no unused key id is available in this region".to_string(),
            )
        })?;
        let key_arn = key_arn(region, &key_id);
        let mut stored = StoredKey {
            key_arn: key_arn.clone(),
            key_attributes: request.key_attributes,
            // Filled in below, from the material this record holds.
            key_check_value: String::new(),
            key_check_value_algorithm: kcv_algorithm.clone(),
            // `Enabled`'s own documentation: "The default value is enabled."
            enabled: request.enabled.unwrap_or(true),
            exportable: request.exportable,
            // The service generates the key material while the call runs, so a
            // created key is complete by the time `CreateKey` answers.
            key_state: KEY_STATE_CREATE_COMPLETE.to_string(),
            key_origin: KEY_ORIGIN_AWS.to_string(),
            create_timestamp: now,
            // The material exists from the moment the key does, so the key is
            // usable for cryptographic operations from then on.
            usage_start_timestamp: Some(now),
            // Written by `StopKeyUsage`, the call that stops this key's use.
            usage_stop_timestamp: None,
            delete_pending_timestamp: None,
            delete_key_in_days: None,
            derive_key_usage: request.derive_key_usage,
            replication_regions,
            uses_default_replication_regions,
            tags,
            material,
        };
        stored.key_check_value = stored.check_value_under(&kcv_algorithm).ok_or_else(|| {
            MintRejection::Invalid(format!(
                "KeyCheckValueAlgorithm {kcv_algorithm} has no definition for a \
                 {key_algorithm} key; use {} instead",
                default_kcv_algorithm(&key_algorithm).unwrap_or("SHA_1")
            ))
        })?;

        // The one place the store grows. `key_arn` is an id [`Self::fresh_key_id`]
        // answered only because no stored key held it, so this can only add a
        // record, never replace one.
        self.keys.insert(key_arn, stored.clone());
        Ok(stored)
    }

    /// The account's default key replication regions, in one stable order.
    pub fn default_replication_regions(&self) -> Vec<String> {
        self.enabled_replication_regions.iter().cloned().collect()
    }

    /// Enable `regions` as default key replication regions and answer the whole set
    /// that is enabled afterwards.
    ///
    /// Additive, because `EnableDefaultKeyReplicationRegions`' own documentation says
    /// it "Enables Multi-Region key replication settings for your account, causing new
    /// keys to be automatically replicated to the specified Regions": a call names the
    /// regions it enables, not the whole configuration, so enabling one twice leaves
    /// one.
    ///
    /// `None` when the call would take the set past
    /// [`MAX_ENABLED_REPLICATION_REGIONS`], which is checked BEFORE anything is stored:
    /// the regions are caller-supplied strings and a disable only removes the ones a
    /// caller names, so without this one request with a long list — or a run of
    /// requests naming fresh region-shaped strings — would grow the set, and the answer
    /// built from it, without end. Nothing is stored on a refusal.
    pub fn enable_default_replication_regions(
        &mut self,
        regions: Vec<String>,
    ) -> Option<Vec<String>> {
        let requested: BTreeSet<String> = regions.into_iter().collect();
        let would_hold = self
            .enabled_replication_regions
            .union(&requested)
            .count();
        if would_hold > MAX_ENABLED_REPLICATION_REGIONS {
            return None;
        }
        self.enabled_replication_regions.extend(requested);
        Some(self.default_replication_regions())
    }

    /// Disable `regions` as default key replication regions and answer the set that
    /// remains enabled. A region that was not enabled changes nothing.
    pub fn disable_default_replication_regions(&mut self, regions: &[String]) -> Vec<String> {
        for region in regions {
            self.enabled_replication_regions.remove(region);
        }
        self.default_replication_regions()
    }

    /// The export parameter set `ReuseLastGeneratedToken` answers with, if this store
    /// holds one: the most recently minted set for the same key material type and
    /// signing key algorithm that still has [`EXPORT_TOKEN_REUSE_REMAINING`] of its
    /// validity left at `now`.
    pub fn reusable_export_parameters(
        &self,
        key_material_type: &str,
        signing_key_algorithm: &str,
        now: SystemTime,
    ) -> Option<&ExportParameters> {
        self.export_parameters
            .values()
            .filter(|parameters| {
                parameters.key_material_type == key_material_type
                    && parameters.signing_key_algorithm == signing_key_algorithm
                    && parameters.has_remaining(now, EXPORT_TOKEN_REUSE_REMAINING)
            })
            .max_by_key(|parameters| parameters.created)
    }

    /// How many export parameter sets are still valid at `now`.
    pub fn valid_export_parameters(&self, now: SystemTime) -> usize {
        self.export_parameters
            .values()
            .filter(|parameters| parameters.is_valid(now))
            .count()
    }

    /// Store an export parameter set for `signing_key`, minted at `now` and expiring
    /// at `valid_until`, and answer it.
    ///
    /// The sets the clock has already expired are dropped here: an expired token names
    /// no parameters a later `ExportKey` could use, so it is not kept and does not
    /// count against [`MAX_EXPORT_PARAMETERS`].
    ///
    /// `None` when the store already holds that many unexpired sets or when no unused
    /// export token is available — both judged under the SAME borrow that inserts, so
    /// concurrent calls cannot each pass a check and together push the store past the
    /// bound. Nothing is stored on a refusal.
    pub fn remember_export_parameters(
        &mut self,
        key_material_type: &str,
        signing_key_algorithm: &str,
        signing_key: MintedSigningKey,
        now: SystemTime,
        valid_until: SystemTime,
    ) -> Option<ExportParameters> {
        self.export_parameters
            .retain(|_, parameters| parameters.is_valid(now));
        if self.export_parameters.len() >= MAX_EXPORT_PARAMETERS {
            return None;
        }
        let export_token = self.fresh_export_token()?;
        let parameters = ExportParameters {
            export_token: export_token.clone(),
            key_material_type: key_material_type.to_string(),
            signing_key_algorithm: signing_key_algorithm.to_string(),
            signing_key_certificate: signing_key.certificate,
            signing_key_certificate_chain: signing_key.certificate_chain,
            created: now,
            valid_until,
            signing_key_pkcs8: signing_key.private_key_pkcs8,
        };
        self.export_parameters
            .insert(export_token, parameters.clone());
        Some(parameters)
    }

    /// An export token no stored parameter set is using, or `None` when every id this
    /// generator can produce is taken.
    ///
    /// Drawn from the same mixing stream as a key id, and checked against the store
    /// the same way, so no mint can ever replace a set a caller still holds the token
    /// for.
    fn fresh_export_token(&mut self) -> Option<String> {
        let mut seed = self.draw();
        for _ in 0..MAX_KEY_ID_ATTEMPTS {
            let candidate: String = (0..EXPORT_TOKEN_ID_LEN)
                .map(|_| {
                    let draw = splitmix64(&mut seed);
                    KEY_ID_ALPHABET[(draw % KEY_ID_ALPHABET.len() as u64) as usize] as char
                })
                .collect();
            let token = format!("{EXPORT_TOKEN_PREFIX}{candidate}");
            if !self.export_parameters.contains_key(&token) {
                return Some(token);
            }
        }
        // Bounded fallback, for the same reason as [`Self::fresh_key_id`]: the
        // zero-padded decimals are also `[0-9a-zA-Z]{16}`, and there are always more
        // of them than there are stored sets.
        (0u64..=self.export_parameters.len() as u64)
            .map(|n| format!("{EXPORT_TOKEN_PREFIX}{n:016}"))
            .find(|token| !self.export_parameters.contains_key(token))
    }

    /// A key id no stored key in `region` is using, or `None` when every id this
    /// generator can produce is taken.
    ///
    /// The clock and this store's counter are read ONCE per call and the mixer is
    /// then advanced for each character, the same pattern [`Self::fresh_material`]
    /// follows: consecutive characters come from consecutive states of one stream
    /// rather than from near-identical seeds, and a retry costs no further clock
    /// reads.
    fn fresh_key_id(&mut self, region: &str) -> Option<String> {
        let mut seed = self.draw();
        for _ in 0..MAX_KEY_ID_ATTEMPTS {
            let candidate: String = (0..KEY_ID_LEN)
                .map(|_| {
                    let draw = splitmix64(&mut seed);
                    KEY_ID_ALPHABET[(draw % KEY_ID_ALPHABET.len() as u64) as usize] as char
                })
                .collect();
            if !self.key_id_taken(region, &candidate) {
                return Some(candidate);
            }
        }
        // Bounded fallback, so a pathological clock can never spin here: the
        // zero-padded decimals are also `[0-9a-zA-Z]{16}`, and there are always more
        // of them than there are stored keys. Every candidate is checked, and an
        // exhausted id space answers `None` rather than an id already in use: the
        // caller refuses the create instead of overwriting a stored key.
        (0u64..=self.keys.len() as u64)
            .map(|n| format!("{n:016}"))
            .find(|id| !self.key_id_taken(region, id))
    }

    /// Whether this store already holds the key this id names in `region`.
    ///
    /// The store is keyed by the whole ARN, so the question is one map lookup.
    fn key_id_taken(&self, region: &str, id: &str) -> bool {
        self.keys.contains_key(&key_arn(region, id))
    }

    /// `len` bytes of fresh key material.
    ///
    /// The stream is a SplitMix64 mixed from the wall clock and this store's
    /// counter, so every key gets its own material and two keys minted in the same
    /// nanosecond still differ. It is NOT a cryptographic generator and does not
    /// need to be: this material never leaves the emulator and protects nothing. It
    /// exists so that each key's check value is computed from real stored bytes.
    fn fresh_material(&mut self, len: usize) -> KeyMaterial {
        // The clock and this store's counter are read ONCE per key, and the stream is
        // then advanced for the bytes that follow: consecutive bytes come from
        // consecutive states of one mixer rather than from near-identical seeds, and a
        // long key costs one clock read instead of one per byte.
        let mut seed = self.draw();
        let mut bytes = Vec::with_capacity(len);
        while bytes.len() < len {
            bytes.extend_from_slice(&splitmix64(&mut seed).to_le_bytes());
        }
        bytes.truncate(len);
        KeyMaterial(bytes)
    }

    /// One draw from this store's mixing stream.
    fn draw(&mut self) -> u64 {
        self.counter = self.counter.wrapping_add(1);
        let mut seed = now_nanos() ^ self.counter.rotate_left(17);
        splitmix64(&mut seed)
    }
}

/// Whether `key_algorithm` names a symmetric algorithm.
///
/// A symmetric key is a single secret; an RSA or ECC key is one half of a pair. The
/// split is taken from the model rather than from the shape of the name: the
/// generated `SYMMETRIC_KEY_ALGORITHM_VALUES` is exactly what
/// `smithy_query(query="shape",shape="SymmetricKeyAlgorithm")` reports, so an
/// algorithm the model gains later cannot be misclassified by a name prefix.
pub fn is_symmetric_algorithm(key_algorithm: &str) -> bool {
    SYMMETRIC_KEY_ALGORITHM_VALUES.contains(&key_algorithm)
}

/// The `KeyClass` of a key that is a single secret, from
/// `smithy_query(query="shape",shape="KeyClass")`.
pub const KEY_CLASS_SYMMETRIC: &str = "SYMMETRIC_KEY";
/// The `KeyClass` of a key this service generates as a pair, from the same shape.
pub const KEY_CLASS_ASYMMETRIC_PAIR: &str = "ASYMMETRIC_KEY_PAIR";

/// The `KeyClass` a key of `key_algorithm` carries when this service generates it: a
/// symmetric algorithm names one secret, an asymmetric one a whole pair.
pub fn generated_key_class(key_algorithm: &str) -> &'static str {
    if is_symmetric_algorithm(key_algorithm) {
        KEY_CLASS_SYMMETRIC
    } else {
        KEY_CLASS_ASYMMETRIC_PAIR
    }
}

/// Why a key's `KeyUsage`, `KeyClass` and `KeyAlgorithm` cannot describe a key this
/// service generates, or `None` when they agree.
///
/// `KeyAttributes`' own documentation points the caller at "Understanding key
/// attributes" for the valid combinations; what is enforced here is the rule the three
/// members state directly: each of them already says whether the key is a single
/// secret or an asymmetric pair — the usage through the TR-31 key block usage it names
/// ([`key_usage_class`]), the algorithm through its family ([`generated_key_class`]) —
/// and all of them have to say the same thing. A request where they disagree describes
/// no key at all and is refused with the modelled `ValidationException`.
fn attribute_rejection(attributes: &KeyAttributes) -> Option<String> {
    if let Some(why) = key_usage_rejection(attributes) {
        return Some(why);
    }
    key_class_rejection(attributes)
}

/// Why a key's `KeyUsage` cannot describe a key of its `KeyAlgorithm`, or `None` when
/// the two agree.
///
/// The refusal is worded the way the service words an attribute combination it does
/// not accept: the member that is not valid, then the member it is not valid for.
fn key_usage_rejection(attributes: &KeyAttributes) -> Option<String> {
    let by_algorithm = generated_key_class(&attributes.key_algorithm);
    let unusable = format!(
        "KeyAlgorithm {} is not valid for the KeyUsage {}.",
        attributes.key_algorithm, attributes.key_usage
    );
    let Some(by_usage) = key_usage_class(&attributes.key_usage) else {
        return Some(unusable);
    };
    (by_usage != by_algorithm).then_some(unusable)
}

/// The `KeyClass` the key block usage `key_usage` names: one secret, or one half of an
/// asymmetric pair.
///
/// The TR-31 key block usages `KeyUsage` enumerates ("as defined in section A.5.2 of
/// the TR-31 spec", that member's own documentation) each name a key of one kind: the
/// four asymmetric usages name a key pair — data encryption under a public key, key
/// agreement, digital signature and the TR-34 asymmetric key — and every other usage
/// names a symmetric key. The table names each modelled value
/// (`smithy_query(query="shape",shape="KeyUsage")`) rather than matching the spelling
/// of a member, the same rule [`material_len`] and [`default_kcv_algorithm`] follow, so
/// a usage the model gains later answers `None` and the create is refused instead of
/// being classified by a name prefix.
pub fn key_usage_class(key_usage: &str) -> Option<&'static str> {
    let class = match key_usage {
        "TR31_D1_ASYMMETRIC_KEY_FOR_DATA_ENCRYPTION"
        | "TR31_K2_TR34_ASYMMETRIC_KEY"
        | "TR31_K3_ASYMMETRIC_KEY_FOR_KEY_AGREEMENT"
        | "TR31_S0_ASYMMETRIC_KEY_FOR_DIGITAL_SIGNATURE" => KEY_CLASS_ASYMMETRIC_PAIR,
        "TR31_B0_BASE_DERIVATION_KEY"
        | "TR31_C0_CARD_VERIFICATION_KEY"
        | "TR31_D0_SYMMETRIC_DATA_ENCRYPTION_KEY"
        | "TR31_E0_EMV_MKEY_APP_CRYPTOGRAMS"
        | "TR31_E1_EMV_MKEY_CONFIDENTIALITY"
        | "TR31_E2_EMV_MKEY_INTEGRITY"
        | "TR31_E4_EMV_MKEY_DYNAMIC_NUMBERS"
        | "TR31_E5_EMV_MKEY_CARD_PERSONALIZATION"
        | "TR31_E6_EMV_MKEY_OTHER"
        | "TR31_K0_KEY_ENCRYPTION_KEY"
        | "TR31_K1_KEY_BLOCK_PROTECTION_KEY"
        | "TR31_M0_ISO_16609_MAC_KEY"
        | "TR31_M1_ISO_9797_1_MAC_KEY"
        | "TR31_M3_ISO_9797_3_MAC_KEY"
        | "TR31_M6_ISO_9797_5_CMAC_KEY"
        | "TR31_M7_HMAC_KEY"
        | "TR31_P0_PIN_ENCRYPTION_KEY"
        | "TR31_P1_PIN_GENERATION_KEY"
        | "TR31_V1_IBM3624_PIN_VERIFICATION_KEY"
        | "TR31_V2_VISA_PIN_VERIFICATION_KEY" => KEY_CLASS_SYMMETRIC,
        _ => return None,
    };
    Some(class)
}

/// Why a key's `KeyClass` and `KeyAlgorithm` cannot describe a key this service
/// generates, or `None` when they agree.
///
/// `KeyAttributes`' own documentation points the caller at "Understanding key
/// attributes" for the valid combinations; the rule enforced here is the one the two
/// members state directly: a key's algorithm already says whether the key is a single
/// secret or a pair, and its class has to say the same. That leaves exactly two
/// classes a generated key can carry — `SYMMETRIC_KEY` for TDES/AES/HMAC and
/// `ASYMMETRIC_KEY_PAIR` for RSA/ECC. The remaining modelled classes, `PUBLIC_KEY`
/// and `PRIVATE_KEY`, each name ONE HALF of a pair that already exists — what a key
/// block carries into `ImportKey`, not something a generate call can mint — so they
/// are refused with the modelled `ValidationException` whatever the algorithm.
///
/// Worded like [`key_usage_rejection`], which is how the service words an attribute
/// combination it does not accept: the member that is not valid, then the member it is
/// not valid for.
fn key_class_rejection(attributes: &KeyAttributes) -> Option<String> {
    let expected = generated_key_class(&attributes.key_algorithm);
    if attributes.key_class == expected {
        return None;
    }
    Some(format!(
        "KeyClass {} is not valid for the KeyAlgorithm {}.",
        attributes.key_class, attributes.key_algorithm,
    ))
}

/// `Tags` as a map, refusing a list that carries one tag key twice or a key in the
/// reserved namespace.
///
/// From the `Tags` member's own documentation: "You can't have more than one tag on
/// an Amazon Web Services Payment Cryptography key with the same tag key." The
/// `aws:` prefix is reserved for tags AWS itself sets, across every service, so a
/// caller-supplied key carrying it is refused rather than stored.
fn tag_map(tags: &[Tag]) -> Result<BTreeMap<String, String>, TagRejection> {
    if tags.len() > MAX_TAGS {
        return Err(TagRejection::Invalid(format!(
            "Tags carries {} entries, more than the {MAX_TAGS} this service accepts",
            tags.len()
        )));
    }
    let mut map = BTreeMap::new();
    for tag in tags {
        if tag.key.to_ascii_lowercase().starts_with(RESERVED_TAG_PREFIX) {
            return Err(TagRejection::Invalid(format!(
                "Tags carries the tag key '{}', and the '{RESERVED_TAG_PREFIX}' prefix \
                 is reserved",
                tag.key
            )));
        }
        if map.insert(tag.key.clone(), tag.value.clone()).is_some() {
            return Err(TagRejection::Invalid(format!(
                "Tags carries the tag key '{}' more than once; a key may carry each \
                 tag key only once",
                tag.key
            )));
        }
    }
    Ok(map)
}

/// The ARN of the key `id` in `region`.
pub fn key_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("{KEY_ARN_RESOURCE_PREFIX}{id}"),
    )
}

/// What every `KeyArn` this emulator mints starts with — the prefix a `ListKeys`
/// page token must carry to be one this service issued.
pub fn key_arn_prefix() -> String {
    format!("arn:aws:{ARN_SERVICE}:")
}

/// The alias name `identifier` names, or `None` when `identifier` is not an alias.
///
/// Both forms the modelled `KeyArnOrKeyAliasType` admits for an alias are accepted:
/// the bare `alias/<name>` and the alias ARN. An alias ARN must name this service in
/// this emulator's account, and its name is what identifies the alias, since an alias
/// is a name within an account.
///
/// The ARN's region field is deliberately NOT compared, unlike the whole-ARN match a
/// key identifier gets in [`ComAmazonawsPaymentcryptographyPaymentcryptographycontrolplaneState::resolve_key_arn`]:
/// a key ARN is this store's own key, so a field that differs names a record the store
/// does not hold, whereas an alias is stored under its name alone — the alias store
/// has no per-region partition to disagree with, so reading the region out of the ARN
/// would invent a scoping this emulator does not keep. The binding it resolves to
/// still carries the key's own ARN, region included.
pub fn alias_name_from_identifier(identifier: &str) -> Option<String> {
    if identifier.starts_with(ALIAS_NAME_PREFIX) {
        return Some(identifier.to_string());
    }
    let mut fields = identifier.splitn(6, ':');
    let (arn, partition, service, _region, account, resource) = (
        fields.next()?,
        fields.next()?,
        fields.next()?,
        fields.next()?,
        fields.next()?,
        fields.next()?,
    );
    if arn != "arn"
        || partition != "aws"
        || service != ARN_SERVICE
        || account != ::engine::emulator_account_id()
    {
        return None;
    }
    resource
        .starts_with(ALIAS_NAME_PREFIX)
        .then(|| resource.to_string())
}

/// The region this request was signed for, from its SigV4 credential scope; the
/// emulator's default region when it carries none this service can mint ARNs for.
///
/// The scope is caller-controlled text and the value lands inside a `KeyArn` whose
/// modelled pattern and length bound both constrain it, so anything the model would
/// not admit there is rejected here and the default used instead: a generated ARN
/// can never carry text the model does not allow.
pub fn request_region(parts: &::http::request::Parts) -> String {
    let requested = ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .filter(|region| is_mintable_region(region));
    requested.unwrap_or_else(|| {
        // Say so in the log rather than silently minting an ARN in another region.
        ::tracing::debug!(
            target: "emulator::audit",
            service = ARN_SERVICE,
            default_region = DEFAULT_REGION,
            "the request's credential scope names no region this service can build a \
             KeyArn for; the default region is used"
        );
        DEFAULT_REGION.to_string()
    })
}

/// Whether `region` matches the modelled `Region` pattern
/// `^[a-z]{2}-[a-z]{1,16}-[0-9]+$` and has a length that keeps a minted `KeyArn`
/// inside that shape's 70..150 bound (see [`MIN_REGION_LEN`]).
fn is_mintable_region(region: &str) -> bool {
    if !(MIN_REGION_LEN..=MAX_REGION_LEN).contains(&region.len()) {
        return false;
    }
    let mut fields = region.split('-');
    let (Some(area), Some(sub), Some(number), None) = (
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
    ) else {
        return false;
    };
    area.len() == 2
        && area.bytes().all(|b| b.is_ascii_lowercase())
        && (1..=16).contains(&sub.len())
        && sub.bytes().all(|b| b.is_ascii_lowercase())
        && !number.is_empty()
        && number.bytes().all(|b| b.is_ascii_digit())
}

/// Now, as a `SystemTime` — the instant every timestamp this service reports, and
/// every clock-owed transition it materialises, is read from.
pub fn now() -> SystemTime {
    SystemTime::now()
}

/// Now, in nanoseconds since the Unix epoch — only a generator seed.
fn now_nanos() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos() as u64)
}

/// SplitMix64: the generator's mixing step, so consecutive seeds do not produce
/// near-identical output.
fn splitmix64(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// Record one state-changing call in the emulator's structured log: which operation
/// changed what, and which principal asked for it.
///
/// The real service records every control-plane mutation (CloudTrail), so a
/// destructive call here leaves a local structured trace too. The caller's access key
/// id identifies the principal and is not a secret; no key material, no tag value and
/// no credential is ever logged.
pub fn record_mutation(parts: &::http::request::Parts, operation: &str, resource: &str) {
    ::tracing::info!(
        target: "emulator::audit",
        service = ARN_SERVICE,
        operation = operation,
        resource = resource,
        principal = %::engine::extract_akid(&parts.headers),
        "payment cryptography control plane state change"
    );
}

/// One tag as the modelled `Tag` structure.
pub fn tag(key: &str, value: &str) -> Tag {
    Tag {
        key: key.to_string(),
        value: value.to_string(),
    }
}

/// An alias as the modelled `Alias` structure.
pub fn alias(name: &str, key_arn: Option<&String>) -> Alias {
    Alias {
        alias_name: name.to_string(),
        key_arn: key_arn.cloned(),
    }
}

// ---------------------------------------------------------------------------
// Pagination
// ---------------------------------------------------------------------------

/// A page token naming the item the NEXT page starts at.
///
/// `NextToken` is modelled opaque, so the item's store key is hex-encoded rather
/// than handed back in the clear — this makes the token opaque to a caller, not
/// secret: hex is trivially reversible and the token carries nothing but a key ARN
/// or an alias name. Twice the longest `KeyArn` (150) is 300 characters, well inside
/// the member's 1..8192 bound.
pub fn encode_page_token(start_at: &str) -> String {
    hex::encode(start_at)
}

/// The store key a page token names, or `None` when the token is not one this
/// service issued. `expected_prefix` is what that key must start with, so a token
/// one list operation handed out is not accepted by another.
pub fn decode_page_token(token: &str, expected_prefix: &str) -> Option<String> {
    hex::decode(token)
        .ok()
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .filter(|start| start.len() <= MAX_TOKEN_TARGET_LEN && start.starts_with(expected_prefix))
}

/// A `ListTagsForResource` page token naming the tag key the NEXT page starts at.
///
/// The name is prefixed before it is encoded, so the prefix check
/// [`decode_page_token`] already applies keeps a token one list operation handed out
/// from being accepted by another: a tag token's payload names a tag key, a key
/// token's names a key ARN and an alias token's an alias name.
pub fn encode_tag_page_token(tag_key: &str) -> String {
    encode_page_token(&format!("{TAG_TOKEN_PREFIX}{tag_key}"))
}

/// The tag key a page token names, or `None` when the token is not one
/// `ListTagsForResource` issued: it must carry [`TAG_TOKEN_PREFIX`] and name a tag
/// key inside the modelled `TagKey` 1..128 length
/// (`smithy_query(query="shape",shape="TagKey")`).
pub fn decode_tag_page_token(token: &str) -> Option<String> {
    let payload = decode_page_token(token, TAG_TOKEN_PREFIX)?;
    let tag_key = payload.strip_prefix(TAG_TOKEN_PREFIX)?.to_string();
    (1..=MAX_TAG_KEY_LEN)
        .contains(&tag_key.len())
        .then_some(tag_key)
}

/// What a tag page token's payload starts with. Not a tag key itself: tag keys carry
/// no modelled prefix, so this one is the emulator's own token namespace.
const TAG_TOKEN_PREFIX: &str = "tag/";

/// The longest tag key a tag page token may name: the maximum of the modelled
/// `TagKey` length (1..128).
const MAX_TAG_KEY_LEN: usize = 128;

/// The longest store key a page token may name: the maximum length of the modelled
/// `KeyArnOrKeyAliasType`, which bounds both a `KeyArn` (150) and an alias name
/// (256) — source `smithy_query(query="operation",operation="GetKey")`.
const MAX_TOKEN_TARGET_LEN: usize = 322;

/// The page size to serve: what the request asked for, clamped into
/// 1..=[`MAX_PAGE_SIZE`], so the page a handler allocates is sized by the server;
/// [`DEFAULT_PAGE_SIZE`] — the documented default — when the request asked for none.
pub fn page_size(max_results: Option<i32>) -> usize {
    max_results.map_or(DEFAULT_PAGE_SIZE, |requested| {
        requested.clamp(1, MAX_PAGE_SIZE as i32) as usize
    })
}

// ---------------------------------------------------------------------------
// Key material and key check values
// ---------------------------------------------------------------------------

/// The key check value algorithm applied when the request names none, or `None` when
/// this service has no default for that algorithm.
///
/// From `KeyCheckValueAlgorithm`'s own documentation, which gives the computation
/// per key family: TDES keys are checked by encrypting a zero block (`ANSI_X9_24`),
/// AES keys by CMAC over a zero block (`CMAC`) and HMAC keys by "the hash selected
/// at key creation on a zero-length message" (`HMAC`). An asymmetric key has no
/// symmetric check value, so its KCV is the `SHA_1` digest of its material.
///
/// The table names the exact modelled `KeyAlgorithm` values
/// (`smithy_query(query="shape",shape="KeyAlgorithm")`) rather than matching the
/// spelling of a member, so an algorithm the model gains later answers `None` and the
/// create is refused instead of being classified by a name prefix — the same rule
/// [`material_len`] follows.
pub fn default_kcv_algorithm(key_algorithm: &str) -> Option<&'static str> {
    let algorithm = match key_algorithm {
        "TDES_2KEY" | "TDES_3KEY" => "ANSI_X9_24",
        "AES_128" | "AES_192" | "AES_256" => "CMAC",
        "HMAC_SHA224" | "HMAC_SHA256" | "HMAC_SHA384" | "HMAC_SHA512" => "HMAC",
        "RSA_2048" | "RSA_3072" | "RSA_4096" | "ECC_NIST_P256" | "ECC_NIST_P384"
        | "ECC_NIST_P521" => "SHA_1",
        _ => return None,
    };
    Some(algorithm)
}

/// How many bytes of material a key of `key_algorithm` carries, or `None` when this
/// service has no material length for that algorithm.
///
/// For the symmetric algorithms this is the algorithm's own key length; for HMAC it
/// is the block length of the selected hash, the length a generated HMAC key
/// carries; for the asymmetric algorithms it is the modulus or curve-field size. The
/// names come from `smithy_query(query="shape",shape="KeyAlgorithm")`.
///
/// `KeyAlgorithm` is a modelled enum, so the centralized walk refuses a value that is
/// not one of these before a handler runs — but an algorithm the model GAINS later
/// would otherwise be minted with a plausible-looking default length and a check
/// value derived from the wrong number of bytes, so an unknown algorithm answers
/// `None` and the caller refuses the request instead.
///
/// An asymmetric key's material is an opaque secret of that size rather than a
/// structured key pair: nothing in this slice exposes a public key or a certificate
/// — `GetPublicKeyCertificate`, `GetCertificateSigningRequest`, `ImportKey` and
/// `ExportKey` all still answer 501 — so the material's only job here is to be the
/// source of the key's check value.
fn material_len(key_algorithm: &str) -> Option<usize> {
    let len = match key_algorithm {
        "TDES_2KEY" => 16,
        "TDES_3KEY" => 24,
        "AES_128" => 16,
        "AES_192" => 24,
        "AES_256" => 32,
        "HMAC_SHA224" | "HMAC_SHA256" => 64,
        "HMAC_SHA384" | "HMAC_SHA512" => 128,
        "RSA_2048" => 256,
        "RSA_3072" => 384,
        "RSA_4096" => 512,
        "ECC_NIST_P256" => 32,
        "ECC_NIST_P384" => 48,
        "ECC_NIST_P521" => 66,
        _ => return None,
    };
    Some(len)
}

/// How many leading bytes of the computed check value a KCV carries.
///
/// From `KeyCheckValueAlgorithm`'s own documentation, which says each computation
/// retains "the 3 highest order bytes" / "the leftmost 3 bytes". The six hex
/// characters that renders sit inside the modelled `KeyCheckValue` 4..16 length
/// bound and satisfy its `^[0-9a-fA-F]+$` pattern.
pub const KCV_BYTES: usize = 3;

/// The key check value of `material`, computed by `kcv_algorithm`, or `None` when
/// that algorithm has no definition for a key of `key_algorithm`.
///
/// Each branch is the computation `KeyCheckValueAlgorithm`'s documentation
/// describes, run with real primitives over the key's own stored material:
///
/// * `ANSI_X9_24` — encrypt a block of eight zero bytes with the key and keep the
///   highest order bytes. Defined for the TDES algorithms and for them alone: that
///   documentation gives this computation only "For TDES keys", and gives AES keys a
///   CMAC check value instead — see [`zero_block_cipher_check`], which is what
///   refuses the other families.
/// * `CMAC` — a CMAC over 16 zero bytes. Defined for AES.
/// * `HMAC` — the selected hash over a zero-length message, keyed by the material.
///   Defined for the HMAC algorithms, which are what select the hash.
/// * `SHA_1` — the SHA-1 digest of the key material. Defined for any key.
pub fn key_check_value(
    kcv_algorithm: &str,
    key_algorithm: &str,
    material: &[u8],
) -> Option<String> {
    let digest = match kcv_algorithm {
        "ANSI_X9_24" => zero_block_cipher_check(key_algorithm, material)?,
        "CMAC" => cmac_check(key_algorithm, material)?,
        "HMAC" => hmac_check(key_algorithm, material)?,
        "SHA_1" => Sha1::digest(material).to_vec(),
        // Unreachable: `KeyCheckValueAlgorithm` is a modelled enum.
        _ => return None,
    };
    if digest.len() < KCV_BYTES {
        return None;
    }
    Some(hex::encode_upper(&digest[..KCV_BYTES]))
}

/// ANSI X9.24: the TDES key encrypting a block of eight zero bytes, highest order
/// bytes first.
///
/// Defined for the TDES algorithms and for them alone, because that is the only
/// family `KeyCheckValueAlgorithm`'s documentation gives this computation for: "For
/// TDES keys, the KCV is computed by encrypting 8 bytes, each with value of zero,
/// with the key to be checked and retaining the 3 highest order bytes of the
/// encrypted result." The same documentation gives AES keys a CMAC check value and
/// HMAC keys a hash of a zero-length message, so an AES or HMAC key named with
/// `ANSI_X9_24` answers `None` and the create is refused with the modelled
/// `ValidationException` naming the algorithm to use instead, rather than reporting a
/// check value the service does not define.
///
/// The two values are matched EXACTLY, the same rule [`material_len`] and
/// [`default_kcv_algorithm`] follow: an algorithm the model gains later is refused
/// rather than routed into this family's computation because its name begins the same
/// way.
fn zero_block_cipher_check(key_algorithm: &str, material: &[u8]) -> Option<Vec<u8>> {
    if !matches!(key_algorithm, "TDES_2KEY" | "TDES_3KEY") {
        return None;
    }
    let key = tdes_key(key_algorithm, material)?;
    block_cipher::tdes_encrypt_blocks(&key, &[0u8; block_cipher::TDES_BLOCK])
}

/// AES-CMAC over 16 zero bytes, the computation the documentation gives for AES
/// keys.
fn cmac_check(key_algorithm: &str, material: &[u8]) -> Option<Vec<u8>> {
    if !matches!(key_algorithm, "AES_128" | "AES_192" | "AES_256") {
        return None;
    }
    block_cipher::aes_cmac(material, &[0u8; block_cipher::AES_BLOCK])
}

/// The selected hash over a zero-length message, keyed by the key material — the
/// computation the documentation gives for HMAC keys.
fn hmac_check(key_algorithm: &str, material: &[u8]) -> Option<Vec<u8>> {
    let mac = match key_algorithm {
        "HMAC_SHA224" => <Hmac<Sha224> as Mac>::new_from_slice(material)
            .ok()?
            .finalize()
            .into_bytes()
            .to_vec(),
        "HMAC_SHA256" => <Hmac<Sha256> as Mac>::new_from_slice(material)
            .ok()?
            .finalize()
            .into_bytes()
            .to_vec(),
        "HMAC_SHA384" => <Hmac<Sha384> as Mac>::new_from_slice(material)
            .ok()?
            .finalize()
            .into_bytes()
            .to_vec(),
        "HMAC_SHA512" => <Hmac<Sha512> as Mac>::new_from_slice(material)
            .ok()?
            .finalize()
            .into_bytes()
            .to_vec(),
        _ => return None,
    };
    Some(mac)
}

/// The 24-byte 3DES key bundle a TDES key's material carries.
///
/// A two-key TDES key is the standard K1|K2|K1 bundle, so both variants drive the
/// same EDE3 cipher.
fn tdes_key(key_algorithm: &str, material: &[u8]) -> Option<[u8; 24]> {
    let len = match key_algorithm {
        "TDES_2KEY" => 16,
        "TDES_3KEY" => 24,
        _ => return None,
    };
    block_cipher::tdes_bundle(material.get(..len)?)
}

// ---------------------------------------------------------------------------
// The TR-34 signing key certificate
// ---------------------------------------------------------------------------

/// A freshly minted TR-34 signing key: its certificate, the certificate of the
/// authority that signed it, and the private half of its key pair.
#[derive(Debug)]
pub struct MintedSigningKey {
    /// The signing key certificate, as `SigningKeyCertificate` reports it.
    pub certificate: String,
    /// The root authority's certificate, as `SigningKeyCertificateChain` reports it.
    pub certificate_chain: String,
    /// The private half of the signing key pair, PKCS#8 DER. Private to this module
    /// and carried in the same redacted wrapper as a key's own material, so no
    /// formatter anywhere can print it.
    private_key_pkcs8: KeyMaterial,
}

/// The object identifiers the minted certificates name.
mod oid {
    use x509_cert::spki::ObjectIdentifier;

    /// `id-at-commonName` (RFC 5280 appendix A).
    pub const COMMON_NAME: ObjectIdentifier = ObjectIdentifier::new_unwrap("2.5.4.3");
    /// `id-at-organizationName` (RFC 5280 appendix A).
    pub const ORGANIZATION: ObjectIdentifier = ObjectIdentifier::new_unwrap("2.5.4.10");
    /// `sha256WithRSAEncryption` (RFC 8017 appendix C).
    pub const SHA256_WITH_RSA: ObjectIdentifier =
        ObjectIdentifier::new_unwrap("1.2.840.113549.1.1.11");
}

/// Mint the TR-34 signing key an export token hands out: an RSA-2048 key pair whose
/// certificate is signed by a freshly minted root authority of its own, both valid
/// from `not_before` until `not_after`.
///
/// Every byte of this is real: both key pairs come from `aws-lc-rs`
/// (`aws_lc_rs::rsa`), each certificate carries the DER `SubjectPublicKeyInfo` that
/// library computes from the key it generated, and each signature is an
/// RSASSA-PKCS1-v1_5 SHA-256 signature that same library makes over the DER of the
/// certificate's own `TBSCertificate` — so a caller can verify the signing key
/// certificate against the chain it is handed, and the private key the store keeps is
/// the private half of the key the certificate names.
///
/// `subject_common_name` names the signing key the certificate is issued for.
/// Generating two RSA key pairs is hundreds of milliseconds of CPU, so a caller runs
/// this with no lock held.
///
/// `None` when the library refuses to produce or encode a key pair, or when a
/// certificate field cannot be encoded (an instant X.509 cannot represent, say).
pub fn mint_tr34_signing_key(
    subject_common_name: &str,
    not_before: SystemTime,
    not_after: SystemTime,
) -> Option<MintedSigningKey> {
    let root = RsaKeyPairDer::generate()?;
    let signing = RsaKeyPairDer::generate()?;

    let root_name = distinguished_name(TR34_ROOT_COMMON_NAME)?;
    let signing_name = distinguished_name(subject_common_name)?;

    let root_certificate = certificate(
        &root_name,
        &root_name,
        &root.public_key_spki,
        (not_before, not_after),
        true,
        &root,
    )?;
    let signing_certificate = certificate(
        &root_name,
        &signing_name,
        &signing.public_key_spki,
        (not_before, not_after),
        false,
        &root,
    )?;

    Some(MintedSigningKey {
        certificate: signing_certificate,
        certificate_chain: root_certificate,
        private_key_pkcs8: KeyMaterial(signing.private_key_pkcs8),
    })
}

/// The common name of the root authority a minted signing key certificate is signed
/// by. It names the service and the key block standard the certificate is for.
const TR34_ROOT_COMMON_NAME: &str = "Payment Cryptography TR-34 Root CA";

/// The common name a minted TR-34 signing key certificate carries: the key it names is
/// the service's own signing key for a TR-34 key export.
pub const TR34_SIGNING_KEY_SUBJECT: &str = "Payment Cryptography TR-34 Signing Key";

/// The organization name every minted certificate carries.
const TR34_ORGANIZATION_NAME: &str = "Payment Cryptography";

/// An RSA key pair, as the two DER encodings a certificate needs.
struct RsaKeyPairDer {
    /// The public half, a DER X.509 `SubjectPublicKeyInfo`.
    public_key_spki: Vec<u8>,
    /// The private half, PKCS#8 DER.
    private_key_pkcs8: Vec<u8>,
}

impl RsaKeyPairDer {
    /// A fresh RSA-2048 key pair from `aws-lc-rs` — the modulus size
    /// [`TR34_SIGNING_KEY_ALGORITHM`] names.
    fn generate() -> Option<Self> {
        use ::aws_lc_rs::encoding::AsDer;

        let private_key =
            ::aws_lc_rs::rsa::PrivateDecryptingKey::generate(::aws_lc_rs::rsa::KeySize::Rsa2048)
                .ok()?;
        Some(Self {
            public_key_spki: private_key.public_key().as_der().ok()?.as_ref().to_vec(),
            private_key_pkcs8: private_key.as_der().ok()?.as_ref().to_vec(),
        })
    }

    /// `message` signed by this key pair's private half, RSASSA-PKCS1-v1_5 over
    /// SHA-256 — the signature `sha256WithRSAEncryption` names.
    fn sign(&self, message: &[u8]) -> Option<Vec<u8>> {
        let pair = ::aws_lc_rs::signature::RsaKeyPair::from_pkcs8(&self.private_key_pkcs8).ok()?;
        // A PKCS #1 signature is exactly as long as the key's modulus.
        let mut signature = vec![0u8; pair.public_modulus_len()];
        pair.sign(
            &::aws_lc_rs::signature::RSA_PKCS1_SHA256,
            &::aws_lc_rs::rand::SystemRandom::new(),
            message,
            &mut signature,
        )
        .ok()?;
        Some(signature)
    }
}

/// One X.509 certificate: `subject`, holding the public key `subject_public_key_spki`,
/// issued by `issuer` for `validity` and signed by `signer`.
///
/// `is_authority` marks the certificate as a certificate authority, which is what lets
/// the root certificate in the chain sign the signing key certificate (RFC 5280 section
/// 4.2.1.9).
fn certificate(
    issuer: &Name,
    subject: &Name,
    subject_public_key_spki: &[u8],
    validity: (SystemTime, SystemTime),
    is_authority: bool,
    signer: &RsaKeyPairDer,
) -> Option<String> {
    let algorithm = AlgorithmIdentifierOwned {
        oid: oid::SHA256_WITH_RSA,
        // RSASSA-PKCS1-v1_5 takes an explicit NULL parameter (RFC 8017 appendix C).
        parameters: Some(Any::new(DerTag::Null, Vec::new()).ok()?),
    };
    let tbs_certificate = TbsCertificate {
        version: Version::V3,
        serial_number: certificate_serial()?,
        signature: algorithm.clone(),
        issuer: issuer.clone(),
        validity: CertificateValidity {
            not_before: Time::try_from(validity.0).ok()?,
            not_after: Time::try_from(validity.1).ok()?,
        },
        subject: subject.clone(),
        subject_public_key_info: SubjectPublicKeyInfoOwned::from_der(subject_public_key_spki)
            .ok()?,
        issuer_unique_id: None,
        subject_unique_id: None,
        extensions: Some(vec![basic_constraints(is_authority)?]),
    };
    let signature = signer.sign(&tbs_certificate.to_der().ok()?)?;
    let certificate = Certificate {
        tbs_certificate,
        signature_algorithm: algorithm,
        signature: BitString::from_bytes(&signature).ok()?,
    };
    // "The signing key certificate in PEM format (base64 encoded)"
    // (`SigningKeyCertificate`'s own documentation): the PEM block itself, base64
    // encoded, which is the form `ImportKey`'s matching member is given too. The PEM
    // framing comes from `x509-cert`'s own encoder, not from anything written here.
    Some(::aws_smithy_types::base64::encode(
        certificate.to_pem(::x509_cert::der::pem::LineEnding::LF).ok()?,
    ))
}

/// The `basicConstraints` extension of a certificate that is, or is not, a certificate
/// authority (RFC 5280 section 4.2.1.9), which that section requires to be critical in
/// a CA certificate.
fn basic_constraints(is_authority: bool) -> Option<Extension> {
    let constraints = BasicConstraints {
        ca: is_authority,
        path_len_constraint: None,
    };
    Some(Extension {
        extn_id: BasicConstraints::OID,
        critical: true,
        extn_value: OctetString::new(constraints.to_der().ok()?).ok()?,
    })
}

/// A certificate serial number: 16 bytes from the OS random source, which RFC 5280
/// section 4.1.2.2 allows up to 20 of.
///
/// `SerialNumber::new` keeps the value the positive integer that section requires by
/// prepending a zero byte when the leading bit is set, which changes no value.
fn certificate_serial() -> Option<SerialNumber> {
    let mut bytes = [0u8; 16];
    ::aws_lc_rs::rand::fill(&mut bytes).ok()?;
    SerialNumber::new(&bytes).ok()
}

/// The X.500 distinguished name a minted certificate carries: the common name it is
/// given, under this service's organization name (RFC 5280 section 4.1.2.4).
fn distinguished_name(common_name: &str) -> Option<Name> {
    Some(Name::from(RdnSequence(vec![
        relative_distinguished_name(oid::ORGANIZATION, TR34_ORGANIZATION_NAME)?,
        relative_distinguished_name(oid::COMMON_NAME, common_name)?,
    ])))
}

/// One relative distinguished name: the `SET OF` holding the single attribute type and
/// its UTF8String value.
fn relative_distinguished_name(
    attribute: ObjectIdentifier,
    value: &str,
) -> Option<RelativeDistinguishedName> {
    let pair = AttributeTypeAndValue {
        oid: attribute,
        value: Any::new(DerTag::Utf8String, value.as_bytes()).ok()?,
    };
    Some(RelativeDistinguishedName::from(
        SetOfVec::try_from(vec![pair]).ok()?,
    ))
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsPaymentcryptographyPaymentcryptographycontrolplaneState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsPaymentcryptographyPaymentcryptographycontrolplaneState>)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ops::types::KeyModesOfUse;
    use aes::Aes128;
    use cmac::Cmac;

    /// A `CreateKey` request for `key_algorithm`, with the class that algorithm is
    /// generated as and a key usage of the same class — a PIN encryption key for a
    /// symmetric algorithm, a digital signature key for an asymmetric one — so the
    /// three attributes describe one key.
    fn request(key_algorithm: &str) -> NewKey {
        let key_usage = if is_symmetric_algorithm(key_algorithm) {
            "TR31_P0_PIN_ENCRYPTION_KEY"
        } else {
            "TR31_S0_ASYMMETRIC_KEY_FOR_DIGITAL_SIGNATURE"
        };
        NewKey {
            key_attributes: KeyAttributes {
                key_usage: key_usage.to_string(),
                key_class: generated_key_class(key_algorithm).to_string(),
                key_algorithm: key_algorithm.to_string(),
                key_modes_of_use: KeyModesOfUse {
                    encrypt: true,
                    decrypt: true,
                    ..KeyModesOfUse::default()
                },
            },
            exportable: true,
            ..NewKey::default()
        }
    }

    /// The deletion window is a transition the CLOCK owes: once the stored
    /// `DeletePendingTimestamp` has passed, every reader reports `DELETE_COMPLETE`
    /// and `DeleteTimestamp` instead of `DELETE_PENDING` and
    /// `DeletePendingTimestamp` — derived, never written back.
    ///
    /// The shortest window the model admits is three days
    /// ([`MIN_DELETE_KEY_IN_DAYS`]), so no request a test can send over the wire
    /// reaches the elapsed side of that transition; this unit test is what covers it,
    /// by storing the instant the window ends in the past.
    #[test]
    fn an_elapsed_deletion_window_reports_delete_complete() {
        // A table over the key families, so the rule is checked on a key of each
        // kind rather than on one algorithm.
        for key_algorithm in ["TDES_3KEY", "AES_256", "HMAC_SHA256", "RSA_2048"] {
            let mut store =
                ComAmazonawsPaymentcryptographyPaymentcryptographycontrolplaneState::default();
            let created = now();
            let stored = store
                .mint_key("us-east-1", request(key_algorithm), created)
                .unwrap_or_else(|why| panic!("{key_algorithm}: the key is minted, got {why:?}"));

            // Scheduled for deletion, with the window ending one second ago.
            let ended = created + Duration::from_secs(1);
            let key = store
                .key_mut(&stored.key_arn)
                .unwrap_or_else(|| panic!("{key_algorithm}: the stored key is found"));
            key.key_state = KEY_STATE_DELETE_PENDING.to_string();
            key.delete_pending_timestamp = Some(ended);

            // Before the window ends: still pending, with the pending timestamp and
            // no delete timestamp.
            let pending = store
                .key(&stored.key_arn)
                .expect("the stored key is found")
                .to_key(created);
            assert_eq!(
                pending.key_state, KEY_STATE_DELETE_PENDING,
                "{key_algorithm}: inside the window, expected {KEY_STATE_DELETE_PENDING}, got {:?}",
                pending.key_state
            );
            assert!(
                pending.delete_pending_timestamp.is_some() && pending.delete_timestamp.is_none(),
                "{key_algorithm}: inside the window, expected only DeletePendingTimestamp, got \
                 pending={:?} delete={:?}",
                pending.delete_pending_timestamp,
                pending.delete_timestamp
            );

            // After it: complete, with the timestamps swapped — and the stored
            // record still says DELETE_PENDING, because a read never writes.
            let elapsed = ended + Duration::from_secs(1);
            let complete = store
                .key(&stored.key_arn)
                .expect("the stored key is found")
                .to_key(elapsed);
            assert_eq!(
                complete.key_state, KEY_STATE_DELETE_COMPLETE,
                "{key_algorithm}: past the window, expected {KEY_STATE_DELETE_COMPLETE}, got {:?}",
                complete.key_state
            );
            assert!(
                complete.delete_pending_timestamp.is_none() && complete.delete_timestamp.is_some(),
                "{key_algorithm}: past the window, expected only DeleteTimestamp, got pending={:?} \
                 delete={:?}",
                complete.delete_pending_timestamp,
                complete.delete_timestamp
            );
            assert_eq!(
                store.keys[&stored.key_arn].key_state,
                KEY_STATE_DELETE_PENDING,
                "{key_algorithm}: a read must not rewrite the stored KeyState, got {:?}",
                store.keys[&stored.key_arn].key_state
            );
            // The check value a reader reports is still the one computed from this
            // key's own material, whatever the clock says about its state.
            assert_eq!(
                complete.key_check_value, stored.key_check_value,
                "{key_algorithm}: expected the stored KCV {:?}, got {:?}",
                stored.key_check_value, complete.key_check_value
            );
        }
    }

    /// Every modelled `KeyAlgorithm` this service generates has both a material
    /// length and a default KCV algorithm, and that default is defined for it — so no
    /// create of a modelled algorithm can be refused for want of a default.
    #[test]
    fn every_generated_algorithm_has_a_default_check_value() {
        for key_algorithm in crate::ops::types::KEY_ALGORITHM_VALUES {
            let mut store =
                ComAmazonawsPaymentcryptographyPaymentcryptographycontrolplaneState::default();
            let stored = store
                .mint_key("eu-west-1", request(key_algorithm), now())
                .unwrap_or_else(|why| panic!("{key_algorithm}: the key is minted, got {why:?}"));
            let expected = default_kcv_algorithm(key_algorithm)
                .unwrap_or_else(|| panic!("{key_algorithm}: has a default KCV algorithm"));
            assert_eq!(
                stored.key_check_value_algorithm, expected,
                "{key_algorithm}: expected the default {expected}, got {:?}",
                stored.key_check_value_algorithm
            );
            assert!(
                stored.key_check_value.len() == KCV_BYTES * 2
                    && stored
                        .key_check_value
                        .bytes()
                        .all(|b| b.is_ascii_hexdigit()),
                "{key_algorithm}: expected {} hex characters, got {:?}",
                KCV_BYTES * 2,
                stored.key_check_value
            );
        }
    }

    /// The keyed branches of [`key_check_value`] itself, driven through that entry
    /// point rather than around it.
    ///
    /// Two things have to hold, and each is checked where it can be:
    ///
    /// 1. The PRIMITIVE each branch runs is the one the standard defines. AES-CMAC
    ///    is pinned on RFC 4493 ("The AES-CMAC Algorithm") examples 1 and 2 and
    ///    HMAC-SHA-256 on RFC 4231 test case 2 — published answers, so a dependency
    ///    that computed something else would fail here.
    /// 2. The branch feeds that primitive what `KeyCheckValueAlgorithm`'s
    ///    documentation says and keeps what it says to keep: "16 bytes of zero" for
    ///    CMAC, "a zero-length message" for HMAC, and the leftmost three bytes of
    ///    the result. Those inputs have no published answer of their own, so the
    ///    expected value is re-derived HERE from the documented rule — a different
    ///    message, hash, byte count or hex case in the implementation fails this.
    ///
    /// The `SHA_1` branch is a plain digest of the material, so it is pinned the
    /// same way the zero-block branches are in
    /// `the_key_check_value_computation_matches_published_vectors`: on a published
    /// digest, SHA-1 of `abc` (`a9993e36…`, FIPS 180-2 appendix A).
    #[test]
    fn the_keyed_check_value_branches_match_their_published_vectors() {
        // 1. The primitives, on their standards' own vectors.
        let cmac_key =
            hex::decode("2b7e151628aed2a6abf7158809cf4f3c").expect("the RFC 4493 key is hex");
        let cmac_rows: &[(&str, &str)] = &[
            ("", "bb1d6929e95937287fa37d129b756746"),
            (
                "6bc1bee22e409f96e93d7e117393172a",
                "070a16b46b4d4144f79bdd9dd04a287c",
            ),
        ];
        for (message, expected) in cmac_rows {
            let mut mac = <Cmac<Aes128> as Mac>::new_from_slice(&cmac_key)
                .expect("AES-128 takes a 16-byte key");
            mac.update(&hex::decode(message).expect("the RFC 4493 message is hex"));
            let tag = hex::encode(mac.finalize().into_bytes());
            assert_eq!(
                tag, *expected,
                "case CMAC of {message:?}: expected the published {expected}, got {tag}"
            );
        }
        let mut rfc4231 =
            <Hmac<Sha256> as Mac>::new_from_slice(b"Jefe").expect("HMAC takes any key length");
        rfc4231.update(b"what do ya want for nothing?");
        let tag = hex::encode(rfc4231.finalize().into_bytes());
        let expected = "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843";
        assert_eq!(
            tag, expected,
            "case HMAC-SHA-256 RFC 4231 test case 2: expected the published {expected}, got {tag}"
        );

        // 2. The documented inputs and truncation, through `key_check_value`.
        //
        // CMAC: "the input data is 16 bytes of zero", keeping the three highest
        // order bytes. Re-derived from that sentence, over the RFC 4493 key so the
        // key is not one this test could have fitted to.
        let mut cmac = <Cmac<Aes128> as Mac>::new_from_slice(&cmac_key)
            .expect("AES-128 takes a 16-byte key");
        cmac.update(&[0u8; 16]);
        let documented = hex::encode_upper(&cmac.finalize().into_bytes()[..KCV_BYTES]);
        let computed = key_check_value("CMAC", "AES_128", &cmac_key);
        assert_eq!(
            computed.as_deref(),
            Some(documented.as_str()),
            "case CMAC AES_128: expected the documented {documented}, got {computed:?}"
        );

        // HMAC: "the hash selected at key creation on a zero-length message, taking
        // the leftmost 3 bytes", so the material is the key and nothing is fed in.
        let material: Vec<u8> = (0u8..64).collect();
        let hmac = <Hmac<Sha256> as Mac>::new_from_slice(&material)
            .expect("HMAC takes any key length");
        let documented = hex::encode_upper(&hmac.finalize().into_bytes()[..KCV_BYTES]);
        let computed = key_check_value("HMAC", "HMAC_SHA256", &material);
        assert_eq!(
            computed.as_deref(),
            Some(documented.as_str()),
            "case HMAC HMAC_SHA256: expected the documented {documented}, got {computed:?}"
        );

        // SHA_1: the digest of the material, leftmost three bytes — on the published
        // SHA-1 of `abc`, `a9993e364706816aba3e25717850c26c9cd0d89d`.
        let computed = key_check_value("SHA_1", "RSA_2048", b"abc");
        assert_eq!(
            computed.as_deref(),
            Some("A9993E"),
            "case SHA_1 of abc: expected the published A9993E, got {computed:?}"
        );
    }
}
