// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-marketplacemetering-awsmpmeteringservice service.
//!
//! One store backs the whole metering loop, so every operation observes what the
//! others wrote:
//!
//! * `registration_tokens` is what `ResolveCustomer` reads — a token is minted out
//!   of band (a buyer's browser hands it to the seller's site), so it is seeded into
//!   state and NEVER created by a lookup: a token that is not there is invalid.
//! * `products` is the seller's published catalog: the usage dimensions a product
//!   accepts, the customers entitled to it, and its public-key versions. It is what
//!   `InvalidProductCodeException`, `InvalidUsageDimensionException`,
//!   `CustomerNotEntitledException` / `CustomerNotSubscribed` and
//!   `InvalidPublicKeyVersionException` are decided from.
//! * `registrations` makes `RegisterUsage` stable: the signature is minted once per
//!   (product, public-key version, nonce) and read back on every later call.
//! * `metering_records` is the ledger `MeterUsage` and `BatchMeterUsage` both append
//!   to, with `dedup_index` and `idempotency` as the two indexes over it that decide
//!   a replay from a conflict.
//!
//! Every quantity in the ledger and its indexes is a [`Quantity`] — the value, not the
//! member or the text it arrived in — so the child `quantity` module is the one place
//! a request's quantity pair becomes something this store can compare.

use std::collections::hash_map::DefaultHasher;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::hash::{Hash, Hasher};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::ops::types::{Metadata, UsageAllocation};

/// One quantity, in either of the two members that spell it, and the allocations that
/// split it — the one place a quantity pair becomes a value.
mod quantity;

pub use quantity::*;

/// How far in the past a `MeterUsage` timestamp may be.
///
/// Cited: `MeterUsageRequest$Timestamp` — "Your application can meter usage for up
/// to six hours in the past."
pub const METER_USAGE_BACKDATING_SECS: i64 = 6 * 3600;

/// How far in the past a `BatchMeterUsage` record's timestamp may be.
///
/// Cited: `UsageRecord$Timestamp` — "Your application can meter usage for up to 24
/// hours in the past."
pub const BATCH_METER_USAGE_BACKDATING_SECS: i64 = 24 * 3600;

/// The number of seconds in the bucket `MeterUsage` de-duplicates within.
///
/// Cited: `MeterUsage` — "The Timestamp request parameter is rounded down to the
/// hour and used to enforce this once-per-hour rule for idempotency."
pub const USAGE_BUCKET_SECS: i64 = 3600;

/// `UsageRecordResultStatus` values, as `USAGE_RECORD_RESULT_STATUS_VALUES` in the
/// model lists them. Named here so the two metering handlers spell them once.
pub const STATUS_SUCCESS: &str = "Success";
/// The record's customer cannot use the product (see `UsageRecordResult$Status`).
pub const STATUS_CUSTOMER_NOT_SUBSCRIBED: &str = "CustomerNotSubscribed";
/// A previously metered record had the same customer, dimension and time but a
/// different quantity (see `UsageRecordResult$Status`).
pub const STATUS_DUPLICATE_RECORD: &str = "DuplicateRecord";

/// A registration token a buyer's browser handed to the seller's website, with the
/// registration it was minted for.
///
/// Seeded into state; `ResolveCustomer` only ever reads it.
#[derive(Debug, Clone, Default)]
pub struct TokenEntry {
    /// The legacy `CustomerIdentifier`. EMPTY for a new-style SaaS integration, for
    /// which `ResolveCustomer` leaves the member out of the response and the seller
    /// identifies the customer by `CustomerAWSAccountId` + `LicenseArn`
    /// (`ResolveCustomerResult$CustomerIdentifier`).
    pub customer_identifier: String,
    /// The buyer's AWS account id — digits only, as `CustomerAWSAccountId`'s
    /// `^[0-9]+$` pattern requires.
    pub customer_aws_account_id: String,
    pub product_code: String,
    pub license_arn: Option<String>,
    pub metadata: Option<Metadata>,
    /// When the token stops resolving. `None` is a token that does not expire.
    pub expires_at: Option<SystemTime>,
}

/// One version of a product's metering public key.
#[derive(Debug, Clone)]
pub struct PublicKeyEntry {
    /// When this key version was (or will be) rotated out. `RegisterUsage` reports it
    /// only once it has passed, because `RegisterUsageResult$PublicKeyRotationTimestamp`
    /// is "(Optional) Only included when public key version has expired".
    pub rotation_timestamp: SystemTime,
}

/// A product as its seller published it.
#[derive(Debug, Clone, Default)]
pub struct Product {
    /// The usage dimensions the product was published with; anything else is an
    /// `InvalidUsageDimensionException`.
    pub dimensions: BTreeSet<String>,
    /// The customers entitled to the product, held by `CustomerIdentifier` and/or
    /// `CustomerAWSAccountId` — both forms identify a customer on the wire.
    pub entitled_customers: BTreeSet<String>,
    pub public_key_versions: BTreeMap<i32, PublicKeyEntry>,
}

/// The key a `RegisterUsage` registration is stored under: the product, the public
/// key version it registered with, and the caller's nonce (empty when it sent none).
pub type RegistrationKey = (String, i32, String);

/// A stored `RegisterUsage` registration. The signature is minted once and read back
/// on every later call with the same key, so it is stable per registration.
#[derive(Debug, Clone)]
pub struct Registration {
    pub signature: String,
    pub public_key_rotation_timestamp: SystemTime,
    pub registered_at: SystemTime,
}

/// One metering record in the ledger.
#[derive(Debug, Clone)]
pub struct MeteringRecord {
    pub metering_record_id: String,
    pub product_code: String,
    /// The customer the usage is billed to, as the record identified it.
    pub customer_identifier: String,
    pub customer_aws_account_id: Option<String>,
    pub dimension: String,
    /// The quantity metered, by value: a record reported in the whole-number member
    /// and one reported in the decimal member are the same record when they report
    /// the same quantity.
    pub quantity: Quantity,
    /// The usage instant the caller reported, in epoch seconds.
    pub timestamp_secs: i64,
    pub usage_allocations: Vec<UsageAllocation>,
    pub created_at: SystemTime,
}

/// The de-duplication key: product, customer, dimension and the hour the reported
/// timestamp falls in.
pub type DedupKey = (String, String, String, i64);

/// What a de-duplication hit replays: the record already stored for the key, and the
/// quantity it was stored with (a different quantity is a duplicate request).
#[derive(Debug, Clone)]
pub struct DedupEntry {
    pub metering_record_id: String,
    pub quantity: Quantity,
}

/// What a `ClientToken` replay is checked against.
#[derive(Debug, Clone)]
pub struct IdempotencyEntry {
    /// A hash over every other member of the request the token was first seen on.
    pub request_fingerprint: u64,
    pub metering_record_id: String,
}

/// The most metering records this emulator keeps in the ledger.
///
/// CHOSEN, not cited: the real service's retention is not stated in the model. Every
/// record is caller-driven growth, so the ledger keeps the most recent this many and
/// drops the oldest with its de-duplication entry. Eviction is observable only as a
/// re-mint: reporting an evicted hour again stores a new record under a new
/// `MeteringRecordId` instead of replaying the old one — and an hour that old is
/// outside the backdating window anyway.
pub const MAX_METERING_RECORDS: usize = 10_000;

/// The most `ClientToken` -> record entries this emulator remembers.
///
/// CHOSEN, not cited. Tokens are caller-supplied (`ClientToken`, len 1..64), so this
/// map is caller-driven growth holding no resource of its own; past this many live
/// entries the oldest is evicted. An evicted token is indistinguishable from one
/// whose idempotency window has passed: the next call carrying it is metered afresh,
/// which the de-duplication index still answers with the already-stored record id.
pub const MAX_IDEMPOTENCY_TOKENS: usize = 1024;

/// The most `RegisterUsage` registrations this emulator remembers.
///
/// CHOSEN, not cited. The key carries the caller-supplied `Nonce`, so a fresh nonce
/// per call is caller-driven growth; past this many the oldest registration is
/// evicted. Eviction is observable only as a re-mint: the next call for an evicted
/// registration checks entitlement again and mints a new signature, exactly as the
/// first call for that nonce did.
pub const MAX_REGISTRATIONS: usize = 1024;

/// A bounded, insertion-ordered map: recording the (`CAP` + 1)-th key forgets the
/// oldest key rather than growing the map.
///
/// One type for every caller-keyed store in this service, so the eviction rule lives
/// in exactly one place — see [`IdempotencyLedger`] and [`Registrations`] for the two
/// it is used as, each carrying its own cap.
#[derive(Debug)]
pub struct BoundedMap<K: Ord + Clone, V, const CAP: usize> {
    by_key: BTreeMap<K, V>,
    oldest_first: VecDeque<K>,
}

/// Hand-written rather than derived: an empty map needs nothing of `K` and `V`, while
/// `#[derive(Default)]` would demand `K: Default, V: Default`.
impl<K: Ord + Clone, V, const CAP: usize> Default for BoundedMap<K, V, CAP> {
    fn default() -> Self {
        Self {
            by_key: BTreeMap::new(),
            oldest_first: VecDeque::new(),
        }
    }
}

impl<K: Ord + Clone, V, const CAP: usize> BoundedMap<K, V, CAP> {
    /// What a previous call recorded under `key`, if it is still remembered.
    pub fn get<Q>(&self, key: &Q) -> Option<&V>
    where
        K: ::std::borrow::Borrow<Q>,
        Q: Ord + ?Sized,
    {
        self.by_key.get(key)
    }

    /// Remember `value` under `key`, evicting the oldest entries to stay within `CAP`.
    pub fn record(&mut self, key: K, value: V) {
        if self.by_key.insert(key.clone(), value).is_none() {
            self.oldest_first.push_back(key);
        }
        while self.oldest_first.len() > CAP {
            if let Some(evicted) = self.oldest_first.pop_front() {
                self.by_key.remove(&evicted);
            }
        }
    }

    /// How many entries are remembered right now.
    pub fn len(&self) -> usize {
        self.by_key.len()
    }

    /// Whether nothing is remembered.
    pub fn is_empty(&self) -> bool {
        self.by_key.is_empty()
    }
}

/// The bounded `ClientToken` -> metering-record store, capped at
/// [`MAX_IDEMPOTENCY_TOKENS`].
pub type IdempotencyLedger = BoundedMap<String, IdempotencyEntry, MAX_IDEMPOTENCY_TOKENS>;

/// The bounded `RegisterUsage` registration store, capped at [`MAX_REGISTRATIONS`].
pub type Registrations = BoundedMap<RegistrationKey, Registration, MAX_REGISTRATIONS>;

/// Per-service state for com-amazonaws-marketplacemetering-awsmpmeteringservice. A handler reaches it through [`service_state`].
///
/// Every store a caller can grow is bounded. The two seeded maps
/// (`registration_tokens`, `products`) are fixtures no request writes to; the ledger
/// is capped at [`MAX_METERING_RECORDS`] with its index held to exactly one entry per
/// stored record; and the two caller-keyed stores are capped and evicted on their own
/// terms (see [`IdempotencyLedger`] and [`Registrations`]).
#[derive(Debug, Default)]
pub struct ComAmazonawsMarketplacemeteringAwsmpmeteringserviceState {
    /// Registration tokens, keyed by the token itself. Seeded; a lookup never
    /// creates one, so no request grows this map.
    pub registration_tokens: BTreeMap<String, TokenEntry>,
    /// The published products, keyed by `ProductCode`. Seeded; no request grows it.
    pub products: BTreeMap<String, Product>,
    /// `RegisterUsage` registrations. At most [`MAX_REGISTRATIONS`] entries.
    pub registrations: Registrations,
    /// The metering ledger, oldest first. At most [`MAX_METERING_RECORDS`] entries:
    /// a `VecDeque` so the oldest record is dropped without moving the rest.
    pub metering_records: VecDeque<MeteringRecord>,
    /// Index over [`Self::metering_records`] by [`DedupKey`] — exactly one entry per
    /// stored record, written and evicted by [`Self::record_usage`], so it is bounded
    /// by the ledger's own cap.
    pub dedup_index: BTreeMap<DedupKey, DedupEntry>,
    /// Index over [`Self::metering_records`] by `ClientToken`. At most
    /// [`MAX_IDEMPOTENCY_TOKENS`] entries.
    pub idempotency: IdempotencyLedger,
    /// Monotonic counter behind every minted `MeteringRecordId`.
    pub next_metering_record_seq: u64,
}

impl ComAmazonawsMarketplacemeteringAwsmpmeteringserviceState {
    /// Mint a fresh, unique `MeteringRecordId`.
    ///
    /// `MeteringRecordId` is an unconstrained `String` in the model, so the shape
    /// chosen here is the 8-4-4-4-12 hex form AWS record ids take; the value is a
    /// counter, so two records are never handed the same id.
    pub fn mint_metering_record_id(&mut self) -> String {
        self.next_metering_record_seq = self.next_metering_record_seq.wrapping_add(1);
        let hex = format!("{:032x}", self.next_metering_record_seq);
        format!(
            "{}-{}-{}-{}-{}",
            &hex[..8],
            &hex[8..12],
            &hex[12..16],
            &hex[16..20],
            &hex[20..]
        )
    }

    /// Append `record` to the ledger and index it for de-duplication.
    ///
    /// The one place a metering record is written, so `MeterUsage` and
    /// `BatchMeterUsage` cannot drift on what "already metered" means — and the one
    /// place the ledger is bounded: past [`MAX_METERING_RECORDS`] the oldest record is
    /// dropped together with the de-duplication entry that names it, so the index
    /// never outlives the record it points at.
    pub fn record_usage(&mut self, record: MeteringRecord) {
        self.dedup_index.insert(
            dedup_key(
                &record.product_code,
                &record.customer_identifier,
                &record.dimension,
                record.timestamp_secs,
            ),
            DedupEntry {
                metering_record_id: record.metering_record_id.clone(),
                quantity: record.quantity,
            },
        );
        self.metering_records.push_back(record);
        while self.metering_records.len() > MAX_METERING_RECORDS {
            let Some(evicted) = self.metering_records.pop_front() else {
                break;
            };
            let key = dedup_key(
                &evicted.product_code,
                &evicted.customer_identifier,
                &evicted.dimension,
                evicted.timestamp_secs,
            );
            if self
                .dedup_index
                .get(&key)
                .is_some_and(|entry| entry.metering_record_id == evicted.metering_record_id)
            {
                self.dedup_index.remove(&key);
            }
        }
    }

    /// The record already stored for this product/customer/dimension/hour, if any.
    pub fn dedup_entry(
        &self,
        product_code: &str,
        customer: &str,
        dimension: &str,
        timestamp_secs: i64,
    ) -> Option<&DedupEntry> {
        self.dedup_index
            .get(&dedup_key(product_code, customer, dimension, timestamp_secs))
    }
}

/// The [`DedupKey`] for a reported usage instant: the timestamp is rounded DOWN to
/// the hour, as `MeterUsage`'s once-per-hour rule requires.
pub fn dedup_key(
    product_code: &str,
    customer: &str,
    dimension: &str,
    timestamp_secs: i64,
) -> DedupKey {
    (
        product_code.to_string(),
        customer.to_string(),
        dimension.to_string(),
        hour_bucket(timestamp_secs),
    )
}

/// The hour `timestamp_secs` falls in, as a bucket index. Floors towards negative
/// infinity so a pre-1970 timestamp buckets the same way as any other.
pub fn hour_bucket(timestamp_secs: i64) -> i64 {
    timestamp_secs.div_euclid(USAGE_BUCKET_SECS)
}

/// `time` as whole epoch seconds.
pub fn epoch_secs(time: SystemTime) -> i64 {
    match time.duration_since(UNIX_EPOCH) {
        Ok(since) => i64::try_from(since.as_secs()).unwrap_or(i64::MAX),
        Err(before) => -i64::try_from(before.duration().as_secs()).unwrap_or(i64::MAX),
    }
}

/// Now, as whole epoch seconds.
pub fn now_epoch_secs() -> i64 {
    epoch_secs(SystemTime::now())
}

/// Base64url without padding (RFC 4648 §5), the encoding a JWT's segments use.
///
/// Built on the SDK's own standard-alphabet base64 (`aws_smithy_types::base64`, a
/// dependency this crate already carries) plus the two character substitutions and the
/// padding removal §5 defines, so no codec is hand-written here.
pub fn base64url_encode(bytes: impl AsRef<[u8]>) -> String {
    ::aws_smithy_types::base64::encode(bytes.as_ref())
        .trim_end_matches('=')
        .replace('+', "-")
        .replace('/', "_")
}

/// The signature `RegisterUsage` returns for a registration.
///
/// `RegisterUsageResult$Signature` is documented as "JWT Token", so the value is a
/// compact JWT over the stored registration's own claims: the product, the public key
/// version, the nonce, the customer and the instant the registration was made.
///
/// It is an UNSECURED JWT (RFC 7519 §6: `alg` `none`, empty third segment). The real
/// service signs with a private key whose public half it publishes for the seller to
/// verify against; this emulator publishes no such key, so a MAC here would be bytes no
/// client could ever verify — claiming a signature that does not exist. The header says
/// `none` instead, which is exactly what is true of it.
///
/// Pure in its arguments, so the same registration always has the same token — and the
/// value is persisted, so a later call reads it back rather than recomputing it.
pub fn registration_signature(
    product_code: &str,
    public_key_version: i32,
    nonce: &str,
    customer: &str,
    issued_at_secs: i64,
) -> String {
    let header = ::serde_json::json!({
        "typ": "JWT",
        "alg": "none",
        "kid": public_key_version.to_string(),
    });
    let claims = ::serde_json::json!({
        "iss": "aws-marketplace",
        "sub": customer,
        "productCode": product_code,
        "nonce": nonce,
        "publicKeyVersion": public_key_version,
        "iat": issued_at_secs,
    });
    format!(
        "{}.{}.",
        base64url_encode(header.to_string()),
        base64url_encode(claims.to_string())
    )
}

/// A hash over everything a `MeterUsage` request carries apart from its
/// `ClientToken`.
///
/// `MeterUsageRequest$ClientToken`: "Passing the same value to a later call to an
/// operation requires that you also pass the same value for all other parameters" —
/// so this is what a replay is compared against. Tags within one allocation are
/// sorted, because a tag set is a set and its wire order carries no meaning.
pub fn meter_usage_fingerprint(
    product_code: &str,
    dimension: &str,
    timestamp_secs: i64,
    quantity: Quantity,
    allocations: &[UsageAllocation],
) -> u64 {
    let mut hasher = DefaultHasher::new();
    product_code.hash(&mut hasher);
    dimension.hash(&mut hasher);
    timestamp_secs.hash(&mut hasher);
    quantity.hash(&mut hasher);
    allocations.len().hash(&mut hasher);
    for allocation in allocations {
        // By VALUE, like the quantity itself: a retry that spells one allocated share
        // in the other member carries the same request.
        allocation_quantity(allocation)
            .map(|reported| reported.value)
            .ok()
            .hash(&mut hasher);
        let mut tags: Vec<(&str, &str)> = allocation
            .tags
            .iter()
            .flatten()
            .map(|tag| (tag.key.as_str(), tag.value.as_str()))
            .collect();
        tags.sort_unstable();
        tags.hash(&mut hasher);
    }
    hasher.finish()
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsMarketplacemeteringAwsmpmeteringserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsMarketplacemeteringAwsmpmeteringserviceState>)
}
