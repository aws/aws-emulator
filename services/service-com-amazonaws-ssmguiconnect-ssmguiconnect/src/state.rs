// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-ssmguiconnect-ssmguiconnect service.
//!
//! SSM GUI Connect models exactly ONE resource: the per-account/per-Region
//! *connection recording preferences* singleton. `UpdateConnectionRecordingPreferences`
//! creates-or-replaces it, `GetConnectionRecordingPreferences` reports it, and
//! `DeleteConnectionRecordingPreferences` removes it — so the whole service is the
//! lifecycle of one [`RecordingPrefsRecord`].
//!
//! PER REGION, because that is the scope the resource exists in: this service is a
//! regional one, so the record a call reaches is the record of the Region its own
//! credential scope names ([`request_region`]) and of no other. The store therefore
//! holds one record per configured Region rather than one for the whole process, and a
//! Region nothing has configured has no resource to report — which is what every read
//! answers [`resource_not_found_message`] for. The account side of that scope is the
//! single account this emulator serves (`::emulator_server::emulator_account_id`), so
//! the Region is what a request varies.
//!
//! Besides the record this module owns the rules the operations share, so no handler
//! can drift from another:
//!
//! * [`refuse_preferences`] — the modeled constraints on
//!   `ConnectionRecordingPreferences` that the framework's boundary validator cannot
//!   enforce (see that function's own docs), worded through the same
//!   [`constraint_message`] the boundary's own refusals are.
//! * [`constraint_message`] — how THIS service words a modeled constraint the boundary
//!   validator refused, which is not the framework's aggregated envelope.
//! * [`resource_not_found_message`] — the one template every path reports the absent
//!   singleton with.
//! * [`just_in_time_node_access`] — the account-level just-in-time node access gate
//!   `UpdateConnectionRecordingPreferences` is refused by, answered from the account's own
//!   persisted enrollment rather than from a copy held here.
//! * [`authorization`] — whether the CALLER's own identity-based IAM policies allow the
//!   action an operation asks for, which every operation is refused by before it reads
//!   or writes anything, and before the account gate above.
//! * [`request_region`] — the Region a request addresses its resources in, the key
//!   every record here is held under.
//! * [`mint_client_token`] — the service-provided value for an `@idempotencyToken`
//!   member the caller left unset.
//! * [`idempotency_key`] — how an `@idempotent` call's outcome is keyed, which the
//!   handler that LOOKS a replay up and the store that RECORDS the outcome both derive
//!   here and nowhere else ([`IdempotencyKey`] admits no other constructor).
//! * [`audit`] — the structured access record every operation emits, so a call to this
//!   service (above all a state-changing one) leaves an attributable trail.

use std::collections::{HashMap, VecDeque};
use std::time::SystemTime;

use aws_smithy_schema_server::constraints::{path, Violation, ViolationKind};

use crate::ops::types::{ConnectionRecordingPreferences, RecordingDestinations, S3Bucket};

/// Whether the CALLER's own IAM policies let a request perform the action it asks for
/// — the decision every operation of this service takes before it reads or writes
/// anything. See that module's docs.
pub mod authorization;

/// Whether the ACCOUNT is enrolled in just-in-time node access in the Region a request was
/// signed for — the precondition `UpdateConnectionRecordingPreferences` is gated on. See
/// that module's docs.
pub mod just_in_time_node_access;

/// The Region a request addresses this service's resources in: the one its SigV4
/// credential scope names, defaulting to [`authorization::DEFAULT_REGION`] for a request
/// that carries none.
///
/// Re-exported from [`authorization`], which reads it to scope the resource an
/// authorization decision is taken against, so the Region a record is STORED under and
/// the Region a call is DECIDED for are read from the request in exactly one way.
pub use authorization::request_region;

/// Most idempotency outcomes retained at once, after which the OLDEST is evicted.
///
/// The map is keyed by a caller-supplied `ClientToken`, so without a cap a client
/// sending a fresh token per request would grow it without bound for the life of the
/// process. The bound is on the number of entries rather than on their age because
/// entry count is what the memory cost is proportional to, and it is set far above any
/// plausible in-flight retry window for a singleton that a caller configures rarely:
/// evicting the oldest entry can only turn a replay that old into a fresh call, which
/// is the same answer the caller would get from a service whose own idempotency window
/// had expired. Eviction is FIFO — strictly by the order the retained OUTCOMES were
/// recorded in, tracked in `idempotency_order` — so it never drops a newer outcome to
/// keep an older one, not even when the newer one reuses a token the store already
/// held.
pub const MAX_IDEMPOTENCY_OUTCOMES: usize = 1024;

/// Most Regions this store holds a record for at once: past it an Update that would give
/// a FURTHER Region its first record is refused with the modelled
/// `ServiceQuotaExceededException` ([`region_ceiling_message`]) and nothing is written.
///
/// A request's Region is its own SigV4 credential scope, which the caller chooses, so
/// without a ceiling a caller could repeat an Update under fresh Region names until the
/// process ran out of memory. A partition's Regions are a fixed, published set and AWS
/// publishes no limit of this kind, so the figure is this emulator's own, set well above
/// the largest partition. A Delete frees its Region's entry, so the room a caller used is
/// returned by the call that removes the record.
pub const MAX_CONFIGURED_REGIONS: usize = 64;

/// The `message` an Update refused for want of room for a further Region carries.
///
/// It names the ceiling that was reached and says whose it is: AWS publishes no quota on
/// the Regions an account may configure recording preferences in, so this bound is THIS
/// EMULATOR's, and the sentence says so rather than claiming a limit the service does not
/// have.
#[must_use]
pub fn region_ceiling_message() -> String {
    format!(
        "this emulator holds connection recording preferences in at most \
         {MAX_CONFIGURED_REGIONS} Regions at once"
    )
}

/// Per-service state for com-amazonaws-ssmguiconnect-ssmguiconnect. A handler reaches it through [`service_state`].
///
/// Every field is private and reached through the methods below: a Region's record and
/// the idempotency record have to move together (an outcome that outlived the record
/// it describes would answer a replay with a state the store no longer holds), the
/// records have to stay inside [`MAX_CONFIGURED_REGIONS`] and the idempotency store
/// inside [`MAX_IDEMPOTENCY_OUTCOMES`].
#[derive(Debug, Default)]
pub struct ComAmazonawsSsmguiconnectSsmguiconnectState {
    /// The connection recording preferences record of each Region that has one, keyed
    /// by the Region a request reaches it in ([`request_region`]). The resource is a
    /// per-account/per-Region singleton, so a Region holds at most one record and no
    /// entry at all means never configured (or deleted) there, which every read of THAT
    /// Region reports as `ResourceNotFoundException` — whatever another Region holds.
    ///
    /// A request's Region is the caller's own credential scope, so the ONE call that can
    /// add an entry here ([`Self::put_preferences`]) is held to
    /// [`MAX_CONFIGURED_REGIONS`] entries by [`Self::has_room_for`], which the Update
    /// handler asks before writing; a read materialises nothing and a Delete frees the
    /// entry it removes.
    connection_recording_preferences: HashMap<String, RecordingPrefsRecord>,
    /// First outcome per `@idempotencyToken`, so replaying an `@idempotent`
    /// Update/Delete with the same `ClientToken` returns that outcome instead of
    /// mutating twice. Keyed by operation and Region as well as token: the same token
    /// replayed against a *different* operation, or in a different Region, is a
    /// different call. Bounded at [`MAX_IDEMPOTENCY_OUTCOMES`] entries.
    idempotency_tokens: HashMap<IdempotencyKey, StoredResponse>,
    /// The order the retained outcomes were recorded in — least recently first — which
    /// is what makes the eviction above FIFO. Holds exactly the map's keys, once each,
    /// so the two are inserted and removed together; re-recording a key moves it to the
    /// back, because an entry's age is the age of the OUTCOME it holds, not of the
    /// token that keys it.
    idempotency_order: VecDeque<IdempotencyKey>,
}

/// The stored connection recording preferences singleton.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordingPrefsRecord {
    /// `ConnectionRecordingPreferences$KMSKeyArn`.
    pub kms_key_arn: String,
    /// `RecordingDestinations$S3Buckets`, stored in the order it arrived.
    pub s3_buckets: Vec<S3BucketRecord>,
    /// The `ClientToken` of the Update that last wrote this record — what both
    /// that Update and every later Get report.
    pub client_token: String,
    /// When the record was last written. Not on the wire (the model declares no
    /// timestamp member); it is what distinguishes one write of the singleton from
    /// the next, so a replayed token can tell whether the record it wrote is still
    /// the one stored.
    pub last_updated: SystemTime,
}

/// One entry of `RecordingDestinations$S3Buckets`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct S3BucketRecord {
    /// `S3Bucket$BucketOwner`.
    pub bucket_owner: String,
    /// `S3Bucket$BucketName`.
    pub bucket_name: String,
}

/// The operations that carry an `@idempotencyToken`, used as the first part of
/// [`IdempotencyKey`] so one token replayed against two operations is two calls.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum IdempotentOperation {
    /// `UpdateConnectionRecordingPreferences`.
    Update,
    /// `DeleteConnectionRecordingPreferences`.
    Delete,
}

/// What [`ComAmazonawsSsmguiconnectSsmguiconnectState`]'s idempotency store is keyed by:
/// the operation, the Region the call was made in ([`request_region`]) and the
/// `ClientToken` it carried.
///
/// The Region is part of the key because the resource the outcome describes is itself
/// per-Region: a token replayed in another Region addresses another record, so it is a
/// different call and has to run for real there.
///
/// Its parts are PRIVATE, so [`idempotency_key`] is the only way to build one: the key a
/// handler looks a replay up under and the key this store records an outcome under are
/// then the same derivation by construction, and changing that derivation (normalising
/// the token, adding or dropping a part) is a compile error at every site rather than a
/// lookup that silently stops matching.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct IdempotencyKey {
    /// Which `@idempotent` operation the outcome belongs to.
    operation: IdempotentOperation,
    /// The Region the call was made in ([`request_region`]).
    region: String,
    /// The `ClientToken` the call carried.
    client_token: String,
}

impl IdempotencyKey {
    /// The operation this key is the key of.
    #[must_use]
    pub fn operation(&self) -> IdempotentOperation {
        self.operation
    }

    /// The Region this key is the key of — the Region whose record the outcome it holds
    /// describes.
    #[must_use]
    pub fn region(&self) -> &str {
        &self.region
    }

    /// The `ClientToken` this key is the key of.
    #[must_use]
    pub fn client_token(&self) -> &str {
        &self.client_token
    }
}

/// The key an `@idempotent` call's outcome is recorded under — the one place the three
/// parts of [`IdempotencyKey`] are put together, so a handler and this store cannot
/// spell it differently.
#[must_use]
pub fn idempotency_key(
    operation: IdempotentOperation,
    region: &str,
    client_token: &str,
) -> IdempotencyKey {
    IdempotencyKey {
        operation,
        region: region.to_string(),
        client_token: client_token.to_string(),
    }
}

/// The first outcome of an `@idempotent` call, replayed when the store is still in
/// the state that call left it in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StoredResponse {
    /// An Update that persisted this record.
    Updated(RecordingPrefsRecord),
    /// A Delete that removed a Region's record. The response carries only the
    /// `ClientToken`, which is the key's own token, so there is nothing to hold.
    Deleted,
}

impl ComAmazonawsSsmguiconnectSsmguiconnectState {
    /// The record `region` holds, or `None` when nothing is configured there — whatever
    /// another Region holds.
    pub fn preferences(&self, region: &str) -> Option<&RecordingPrefsRecord> {
        self.connection_recording_preferences.get(region)
    }

    /// Whether a record may be written for `region`: either the store already holds one
    /// there — replacing it needs no further room — or the Regions it holds records for
    /// are inside [`MAX_CONFIGURED_REGIONS`].
    ///
    /// What the Update handler asks before [`Self::put_preferences`], the one call that
    /// can give a further Region an entry.
    pub fn has_room_for(&self, region: &str) -> bool {
        self.connection_recording_preferences.contains_key(region)
            || self.connection_recording_preferences.len() < MAX_CONFIGURED_REGIONS
    }

    /// How many Regions hold a record. Never above [`MAX_CONFIGURED_REGIONS`].
    pub fn configured_region_count(&self) -> usize {
        self.connection_recording_preferences.len()
    }

    /// Create-or-REPLACE `region`'s record, and record the outcome under `record`'s own
    /// `ClientToken` in that Region. The stored value is exactly `record`, never a merge
    /// with what was there before, and no other Region's record is touched.
    ///
    /// The ONE growth site of the per-Region records, so a caller asks
    /// [`Self::has_room_for`] first — see
    /// `crate::ops::update_connection_recording_preferences`.
    pub fn put_preferences(&mut self, region: &str, record: RecordingPrefsRecord) {
        let key = idempotency_key(IdempotentOperation::Update, region, &record.client_token);
        self.connection_recording_preferences
            .insert(region.to_string(), record.clone());
        self.record_outcome(key, StoredResponse::Updated(record));
    }

    /// Remove `region`'s record and record the outcome under `client_token` in that
    /// Region. No other Region's record is touched.
    ///
    /// SUCCEEDS whether or not anything was configured there: Delete is `@idempotent`,
    /// and "there are no preferences now" is the state it is asked for, not a resource it
    /// failed to find. The outcome is therefore recorded on both paths — a replay of
    /// the token answers the same way the first call did.
    ///
    /// The returned flag says whether a record was actually removed, which is what the
    /// caller audits; it is not a success/failure signal.
    pub fn remove_preferences(&mut self, region: &str, client_token: &str) -> bool {
        let removed = self.connection_recording_preferences.remove(region).is_some();
        let key = idempotency_key(IdempotentOperation::Delete, region, client_token);
        self.record_outcome(key, StoredResponse::Deleted);
        removed
    }

    /// The outcome a replay of `key` should answer with, or `None` when the call has
    /// to run for real.
    ///
    /// An outcome is replayable only while the store is still in the state that call
    /// left it in, IN THE REGION the key names: the record an Update wrote is still the
    /// record that Region holds, or the record a Delete removed is still absent there.
    /// Otherwise a later call changed the resource out from under the token, and
    /// answering from the cache would report a state the next reader does not see.
    pub fn replayable_outcome(&self, key: &IdempotencyKey) -> Option<&StoredResponse> {
        let outcome = self.recorded_outcome(key)?;
        let held = self.preferences(key.region());
        let still_current = match outcome {
            StoredResponse::Updated(record) => held == Some(record),
            StoredResponse::Deleted => held.is_none(),
        };
        still_current.then_some(outcome)
    }

    /// The outcome still RETAINED under `key`, whether or not a replay could be
    /// answered from it — `None` once it was evicted (or never recorded).
    ///
    /// Retention and replayability are two different questions: an outcome the store
    /// still holds stops being replayable as soon as a later call changes the resource,
    /// which is the condition [`Self::replayable_outcome`] adds on top of this one.
    pub fn recorded_outcome(&self, key: &IdempotencyKey) -> Option<&StoredResponse> {
        self.idempotency_tokens.get(key)
    }

    /// How many idempotency outcomes are retained. Never above
    /// [`MAX_IDEMPOTENCY_OUTCOMES`].
    pub fn retained_outcome_count(&self) -> usize {
        self.idempotency_tokens.len()
    }

    /// Record `outcome` under `key`, evicting the least recently recorded entries until
    /// the store is back inside [`MAX_IDEMPOTENCY_OUTCOMES`].
    ///
    /// Re-recording a key the store already holds replaces its place in
    /// `idempotency_order` as well as its outcome: the queue orders entries by when the
    /// outcome each holds was recorded, so what is evicted is always the least recently
    /// recorded outcome. Keeping the old place would let a fresh outcome inherit the age
    /// of the token's first recording and be dropped ahead of outcomes recorded before
    /// it.
    fn record_outcome(&mut self, key: IdempotencyKey, outcome: StoredResponse) {
        let replaced = self.idempotency_tokens.insert(key.clone(), outcome).is_some();
        // A key that was already held still sits at the place its PREVIOUS outcome was
        // recorded at; that place describes an outcome the store no longer holds, so it
        // goes before the fresh one is queued.
        let stale = replaced
            .then(|| self.idempotency_order.iter().position(|held| *held == key))
            .flatten();
        if let Some(stale) = stale {
            self.idempotency_order.remove(stale);
        }
        self.idempotency_order.push_back(key);
        while self.idempotency_tokens.len() > MAX_IDEMPOTENCY_OUTCOMES {
            match self.idempotency_order.pop_front() {
                Some(oldest) => {
                    self.idempotency_tokens.remove(&oldest);
                }
                // Unreachable while the two stay in step, and breaking rather than
                // looping is what keeps it unreachable instead of hanging.
                None => break,
            }
        }
    }
}

impl RecordingPrefsRecord {
    /// Build the record a request asks to persist. Every nested member is stored;
    /// none is dropped.
    pub fn from_request(prefs: &ConnectionRecordingPreferences, client_token: String) -> Self {
        Self {
            kms_key_arn: prefs.kms_key_arn.clone(),
            s3_buckets: prefs
                .recording_destinations
                .s3_buckets
                .iter()
                .map(|bucket| S3BucketRecord {
                    bucket_owner: bucket.bucket_owner.clone(),
                    bucket_name: bucket.bucket_name.clone(),
                })
                .collect(),
            client_token,
            last_updated: SystemTime::now(),
        }
    }

    /// The wire shape for what is stored. The ONE place a response is built from
    /// the record, so Update and Get cannot report the same record differently.
    pub fn to_preferences(&self) -> ConnectionRecordingPreferences {
        ConnectionRecordingPreferences {
            recording_destinations: RecordingDestinations {
                s3_buckets: self
                    .s3_buckets
                    .iter()
                    .map(|bucket| S3Bucket {
                        bucket_owner: bucket.bucket_owner.clone(),
                        bucket_name: bucket.bucket_name.clone(),
                    })
                    .collect(),
            },
            kms_key_arn: self.kms_key_arn.clone(),
        }
    }
}

/// The resource name this service reports the connection recording preferences
/// singleton as, in [`resource_not_found_message`]. It is the resource
/// `GetConnectionRecordingPreferences` and `DeleteConnectionRecordingPreferences`
/// address, and the shape the model names it with
/// (`smithy_query(query="shape", shape="ConnectionRecordingPreferences")`).
pub const CONNECTION_RECORDING_PREFERENCES: &str = "ConnectionRecordingPreferences";

/// This service's `ResourceNotFoundException` message: a fixed template naming the
/// resource that could not be found, never free prose about the account or Region.
///
/// The ONE place the text is built, so Get, Delete and any later reader report an
/// absent resource identically and a client can parse WHICH resource was missing.
///
/// The doubled space is the template's own: the service interpolates a resource-type
/// qualifier before `resource` that is empty for this resource, and the recorded
/// AWS text (`The following  resource cannot be found: '…'`) carries the gap it
/// leaves. Reproduced rather than tidied — a client matching the template sees the
/// service's text, not ours.
pub fn resource_not_found_message(resource: &str) -> String {
    format!("The following  resource cannot be found: '{resource}'")
}

/// Whether a `@length` bound on a member counts characters or collection values,
/// which is what decides the sentence [`constraint_message`] states.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BoundedKind {
    /// A string member: the bound is on its length.
    Characters,
    /// A list member: the bound is on how many values it holds.
    Values,
}

/// Every `@length`-constrained member of this service's input shapes, keyed by the
/// dotted lowerCamel path the framework's boundary walk reports a violation at.
///
/// Source: `smithy_query(query="closure",
/// operation="UpdateConnectionRecordingPreferences,DeleteConnectionRecordingPreferences")`
/// on this service's model — `ClientToken` `@length(1, 64)` (both operations),
/// `ConnectionRecordingPreferences$KMSKeyArn` `@length(1, 2048)` and
/// `RecordingDestinations$S3Buckets` `@length(1, 1)`. The BOUNDS are not repeated
/// here: the violation the framework hands over already carries the bound it broke,
/// so this table cannot drift from the model's numbers.
///
/// The paths are the framework's own spelling (`path::member` lowercases a member's
/// first character only), which is why `KMSKeyArn` appears as `kMSKeyArn`.
const LENGTH_BOUNDED_MEMBERS: &[(&str, BoundedKind)] = &[
    ("clientToken", BoundedKind::Characters),
    (
        "connectionRecordingPreferences.kMSKeyArn",
        BoundedKind::Characters,
    ),
    (
        "connectionRecordingPreferences.recordingDestinations.s3Buckets",
        BoundedKind::Values,
    ),
];

/// How THIS service words a boundary constraint refusal, or `None` when it words the
/// refusal the way the framework does.
///
/// SSM GuiConnect states a constraint violation as ONE sentence — no `N validation
/// error(s) detected` prefix, and no echo of the offending value, which it withholds —
/// in one of two forms, each keyed to the constraint that was broken:
///
/// * a `@length` bound: the member path and the bound it broke
///   ([`length_bound_sentence`]);
/// * a `@pattern`: the member path alone ([`invalid_value_sentence`]), since the
///   requirement is a regular expression the server holds and this service states
///   neither it nor the value that failed it.
///
/// Both sentences are built from the request (the member the caller sent and the bound
/// the model declares for it), so neither carries text keyed to any particular request.
///
/// With several violations the first one this service words is stated, because the
/// form is a single sentence rather than an envelope of clauses. A violation of
/// anything else (an absent `@required` member, a `@range` bound) keeps the framework's
/// wording: those are not the forms recorded for this service, so inventing another
/// would be guessing.
pub fn constraint_message(violations: &[Violation]) -> Option<String> {
    violations.iter().find_map(violation_sentence)
}

/// The sentence for ONE violation, or `None` when this service does not word that
/// violation itself.
fn violation_sentence(violation: &Violation) -> Option<String> {
    match &violation.kind {
        // The pattern is a requirement the server holds and the value is the caller's:
        // the sentence names neither, so EVERY `@pattern` member of this service's input
        // shapes is worded from its path alone and nothing has to be tabulated per
        // member.
        ViolationKind::Pattern(_) => {
            Some(invalid_value_sentence(&model_member_path(&violation.path)))
        }
        _ => length_bound_sentence(violation),
    }
}

/// How this service states that a member held a value the model does not admit for it:
/// the member, by its MODEL path, and nothing else.
///
/// `member` is the dotted model member path ([`model_member_path`]) — the only variable
/// part of the sentence, and the only place a refusal names WHICH member was wrong, so a
/// client parses it from here.
pub fn invalid_value_sentence(member: &str) -> String {
    format!("The value provided for {member} isn't valid.")
}

/// The sentence for one `@length` violation, or `None` when this service does not
/// word that violation itself.
fn length_bound_sentence(violation: &Violation) -> Option<String> {
    let (_, bounded) = LENGTH_BOUNDED_MEMBERS
        .iter()
        .find(|(path, _)| *path == violation.path)?;
    let member = model_member_path(&violation.path);
    match (bounded, &violation.kind) {
        // The recorded forms: a string's upper bound and a list's lower bound. The
        // other side of each bound is the same sentence with the other comparison,
        // which is how the two recorded sentences already differ from each other.
        (BoundedKind::Characters, ViolationKind::MaxLength(max)) => {
            Some(format!("The length for {member} cannot be greater than {max}."))
        }
        (BoundedKind::Characters, ViolationKind::MinLength(min)) => {
            Some(format!("The length for {member} cannot be less than {min}."))
        }
        (BoundedKind::Values, ViolationKind::MinLength(min)) => Some(format!(
            "{member} must contain at least {min} {}.",
            values(*min)
        )),
        (BoundedKind::Values, ViolationKind::MaxLength(max)) => Some(format!(
            "{member} cannot contain more than {max} {}.",
            values(*max)
        )),
        _ => None,
    }
}

/// `value` / `values`, agreeing with the count the sentence states.
fn values(count: i64) -> &'static str {
    if count == 1 {
        "value"
    } else {
        "values"
    }
}

/// A violation path as the MODEL spells the members it names:
/// `connectionRecordingPreferences.recordingDestinations.s3Buckets` ->
/// `ConnectionRecordingPreferences.RecordingDestinations.S3Buckets`, and
/// `connectionRecordingPreferences.recordingDestinations.s3Buckets.1.member.bucketName`
/// -> `ConnectionRecordingPreferences.RecordingDestinations.S3Buckets.BucketName`.
///
/// The framework builds a path segment by lowercasing a member name's FIRST character
/// (`path::member`), so uppercasing it back recovers the model's own name exactly —
/// including `S3Buckets` and `KMSKeyArn`, whose later characters it never touched.
///
/// A step from a collection into one of its values is spelled `<position-or-key>.member`
/// by that same builder (`path::element` / `path::entry`), and BOTH of those segments are
/// dropped: the model names a collection's values through the collection member itself,
/// so a model member path carries no index — and a position or a map key is a value of
/// the particular request rather than a member of the model.
fn model_member_path(path: &str) -> String {
    let segments: Vec<&str> = path.split('.').collect();
    let mut named: Vec<String> = Vec::with_capacity(segments.len());
    let mut at = 0;
    while at < segments.len() {
        if segments.get(at + 1).copied() == Some(COLLECTION_STEP) {
            at += 2;
            continue;
        }
        let mut characters = segments[at].chars();
        named.push(match characters.next() {
            Some(first) => first.to_uppercase().chain(characters).collect::<String>(),
            None => String::new(),
        });
        at += 1;
    }
    named.join(".")
}

/// The segment the framework's path builder marks a step from a collection into one of
/// its values with (`aws_smithy_schema_server::constraints::path`'s collection step),
/// which [`model_member_path`] drops together with the position or key before it.
const COLLECTION_STEP: &str = "member";

/// AWS's denial text for an account that is not enrolled in just-in-time node access.
///
/// It is what distinguishes the FEATURE gate from an IAM denial, which is the only
/// reason the wording matters: the requirement is the rejection class (HTTP 403,
/// `AccessDeniedException`), and this is the recorded example of the gate's own
/// message.
pub const JUST_IN_TIME_NODE_ACCESS_DISABLED: &str = "Just-in-time node access is not enabled. \
     You must set up just-in-time node access to record RDP connections.";

/// The modeled `@pattern` of `S3Bucket$BucketName`, copied verbatim from
/// `smithy_query(query="shape", shape="S3Bucket")` on this service's model.
///
/// Reported in the refusal message so a caller sees the requirement it failed, the
/// way the framework's own `@pattern` violation does.
pub const BUCKET_NAME_PATTERN: &str = r"(?=^.{3,63}$)(?!^(\d+\.)+\d+$)(^(([a-z0-9]|[a-z0-9][a-z0-9\-]*[a-z0-9])\.)*([a-z0-9]|[a-z0-9][a-z0-9\-]*[a-z0-9])$)";

/// The negative lookahead of [`BUCKET_NAME_PATTERN`], `(?!^(\d+\.)+\d+$)`: a name made
/// only of digit labels joined by dots (an IPv4-shaped name) is refused. At least one
/// dot is required by the `+`, so a single all-digit label like `123` is NOT refused.
fn bucket_name_is_all_numeric_labels(name: &str) -> bool {
    name.contains('.')
        && name
            .split('.')
            .all(|label| !label.is_empty() && label.bytes().all(|b| b.is_ascii_digit()))
}

/// The capturing body of [`BUCKET_NAME_PATTERN`]: dot-separated labels, each one a
/// single `[a-z0-9]` or a `[a-z0-9]`-delimited run that may contain `-`. An empty label
/// (a leading, trailing or doubled dot) satisfies neither alternative, so it is refused.
fn bucket_name_labels_are_well_formed(name: &str) -> bool {
    name.split('.').all(|label| {
        let edge = |b: u8| b.is_ascii_lowercase() || b.is_ascii_digit();
        match label.as_bytes() {
            [] => false,
            [only] => edge(*only),
            [first, inner @ .., last] => {
                edge(*first)
                    && edge(*last)
                    && inner.iter().all(|b| edge(*b) || *b == b'-')
            }
        }
    })
}

/// Longest value a refusal's own `Violation` record carries, the same bound the
/// framework's constraint walk applies to one it builds: the value is caller-controlled,
/// so an unbounded one would let a request grow what the refusal path holds.
const MAX_ECHOED_VALUE: usize = 256;

/// The refused value as the `Violation` record carries it: itself when short, otherwise a
/// character-boundary-safe prefix plus the elision the framework's own renderer appends,
/// so a value this service records reads the same as one the boundary walk recorded.
///
/// This service's own wording WITHHOLDS the value ([`constraint_message`]); the record
/// still carries it bounded, because that is what a `Violation` is and what the framework
/// would have put in it.
///
/// Rendering only — the ORIGINAL value is what the check above runs on. Eliding before
/// the check would compare text the caller never sent.
fn echoed(value: &str) -> String {
    if value.len() <= MAX_ECHOED_VALUE {
        return value.to_string();
    }
    let mut end = MAX_ECHOED_VALUE;
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}... [{} bytes]", &value[..end], value.len())
}

/// True when `name` satisfies the modeled `S3Bucket$BucketName` pattern.
///
/// [`BUCKET_NAME_PATTERN`] uses look-around, which the `regex` crate behind the
/// framework's `@pattern` check cannot compile — an uncompilable model pattern is
/// treated there as matching, so the boundary validator passes every bucket name.
/// The pattern is therefore evaluated here as its three independent parts: the
/// `(?=^.{3,63}$)` length lookahead, the `(?!^(\d+\.)+\d+$)` negative lookahead
/// ([`bucket_name_is_all_numeric_labels`]) and the label body
/// ([`bucket_name_labels_are_well_formed`]).
///
/// Each part is plain string logic rather than a regex: the three are a character-class
/// test over dot-separated labels, which Rust expresses directly, and keeping them out
/// of a regex crate keeps this service crate's dependency set — and therefore the
/// workspace lockfile — untouched by this change.
pub fn bucket_name_matches_model_pattern(name: &str) -> bool {
    (3..=63).contains(&name.chars().count())
        && !bucket_name_is_all_numeric_labels(name)
        && bucket_name_labels_are_well_formed(name)
}

/// Refuse a `ConnectionRecordingPreferences` that violates a modeled constraint the
/// boundary validator let through, as the `ValidationException` message body; `None`
/// when it satisfies every one.
///
/// Only constraints the boundary CANNOT enforce are re-checked here — today that is
/// `S3Bucket$BucketName`, for the reason [`bucket_name_matches_model_pattern`]
/// documents. Every other modeled constraint on this input (`ClientToken`'s
/// `@length`, `S3Buckets`' `@length(1, 1)`, `BucketOwner`'s `^[0-9]{12}$`,
/// `KMSKeyArn`'s `@length(1, 2048)`, and every `@required` member) compiles and is
/// already refused before a handler runs, so duplicating it here would be dead code.
///
/// The message is built from the request alone — the member path that failed, worded
/// through [`constraint_message`], the SAME renderer `service.rs`'s `override_refusal`
/// words a boundary refusal with — so a refusal from here and a refusal from the
/// boundary are one sentence and cannot drift apart.
pub fn refuse_preferences(prefs: &ConnectionRecordingPreferences) -> Option<String> {
    let violations: Vec<Violation> = prefs
        .recording_destinations
        .s3_buckets
        .iter()
        .enumerate()
        .filter(|(_, bucket)| !bucket_name_matches_model_pattern(&bucket.bucket_name))
        .map(|(index, bucket)| Violation {
            // Spelled by the framework's own path builder, position included, so the
            // violation names the member exactly as the boundary walk would have — and
            // `constraint_message` maps that one spelling to the model's dotted path.
            path: path::join(&[
                path::member("ConnectionRecordingPreferences"),
                path::member("RecordingDestinations"),
                path::member("S3Buckets"),
                path::element(index + 1),
                path::member("BucketName"),
            ]),
            kind: ViolationKind::Pattern(BUCKET_NAME_PATTERN.to_string()),
            value: Some(echoed(&bucket.bucket_name)),
            sensitive: false,
        })
        .collect();
    constraint_message(&violations)
}

/// The service-provided value for an `@idempotencyToken` member the caller left
/// unset.
///
/// The member is modeled `@idempotencyToken` with `@length(1, 64)` and NO `@pattern`,
/// so what it has to be is a value no other call produces — not a particular shape.
/// This mints 32 lowercase hex characters from two sources that cannot repeat within a
/// process: a monotonic per-process counter, and `RandomState`, whose keys the standard
/// library seeds from the operating system and varies per instance. It is a per-run,
/// server-generated value: nothing about it is derived from the request or from a
/// recording.
///
/// Built on the standard library rather than a UUID crate deliberately. The token has
/// no modeled shape to be faithful to, and adding any dependency to this crate puts an
/// entry in the workspace `Cargo.lock` — the one file every other service's change also
/// rewrites, and the overlap the serialized squash-merge cannot resolve. Keeping this
/// service's dependency set exactly as the scaffold left it keeps the change confined to
/// its own crate.
pub fn mint_client_token() -> String {
    use std::fmt::Write as _;
    use std::hash::{BuildHasher, Hasher};

    /// Calls minted so far in this process; the one input that is guaranteed distinct
    /// per call even if two `RandomState`s ever agreed.
    static MINTED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

    let sequence = MINTED.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let mut token = String::with_capacity(32);
    for half in 0..2_u64 {
        let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
        hasher.write_u64(sequence);
        hasher.write_u64(half);
        let _ = write!(token, "{:016x}", hasher.finish());
    }
    token
}

/// Emit the structured access record for one call to this service.
///
/// Every operation calls this on every branch it can return from, so a reader of the
/// emulator's log can attribute each call — above all the two state-changing ones,
/// whose effect on the singleton is otherwise invisible after the fact. The record
/// carries the operation, the identity the request PRESENTED (the SigV4 access key id,
/// which is an identifier and not a secret), the request target it named, and the
/// outcome; the subscriber supplies the timestamp.
///
/// There is no network peer in the record because a Smithy handler is not given one:
/// `OperationContext::parts()` carries the request's method, URI, headers and
/// extensions, and nothing in this framework puts a peer address in them. The presented
/// credential is therefore what identifies the caller.
///
/// Every interpolated value is request-derived, so each is bounded and stripped of the
/// control characters that would otherwise let one request forge a second log line:
/// [`audit_field`] does both.
///
/// Deliberately metadata only: neither the stored preferences nor the `ClientToken` is
/// logged, so a caller's configuration never reaches the log, and nothing here can be
/// replayed as a credential.
pub fn audit(
    operation: &str,
    ctx: &smithy_server_runtime::OperationContext<'_>,
    outcome: AuditOutcome,
) {
    let parts = ctx.parts();
    tracing::info!(
        target: "emulator::audit",
        service = ctx.identity().signing_name(),
        operation,
        access_key_id = %audit_field(&::engine::extract_akid(&parts.headers)),
        request_target = %audit_field(&format!("{} {}", parts.method, parts.uri.path())),
        outcome = outcome.as_str(),
        "ssm-guiconnect api call"
    );
}

/// Longest request-derived value one audit field carries.
///
/// A SigV4 access key id and a request path are both caller-controlled, so an unbounded
/// echo would let one request emit an arbitrarily long log line.
const MAX_AUDIT_FIELD: usize = 128;

/// A request-derived value as it may appear in an audit record: control characters
/// (above all CR and LF, which would forge a second record) replaced, and the whole
/// thing bounded by [`MAX_AUDIT_FIELD`] on a character boundary.
fn audit_field(value: &str) -> String {
    value
        .chars()
        .map(|c| if c.is_control() { '.' } else { c })
        .take(MAX_AUDIT_FIELD)
        .collect()
}

/// What a call answered, as the one closed set [`audit`] records.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuditOutcome {
    /// The singleton was created or replaced.
    PreferencesWritten,
    /// The singleton was removed.
    PreferencesDeleted,
    /// The singleton was reported to a reader.
    PreferencesRead,
    /// A Delete found nothing configured, which is the state it was asked for: the
    /// call succeeded and removed nothing.
    PreferencesAlreadyAbsent,
    /// An `@idempotent` call replayed an earlier outcome and changed nothing.
    IdempotentReplay,
    /// The request was refused as invalid (400).
    ValidationRefused,
    /// A write was refused for want of room for a further Region's record (402).
    QuotaExceeded,
    /// The caller was refused 403: either by its own IAM policies
    /// ([`authorization`]) or by the account-level just-in-time node access gate.
    AccessDenied,
    /// There was no configured singleton to report or remove (404).
    NotFound,
    /// The service's own store was unreachable (500).
    StoreUnavailable,
}

impl AuditOutcome {
    /// The stable token this outcome is logged as.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::PreferencesWritten => "preferences-written",
            Self::PreferencesDeleted => "preferences-deleted",
            Self::PreferencesRead => "preferences-read",
            Self::PreferencesAlreadyAbsent => "preferences-already-absent",
            Self::IdempotentReplay => "idempotent-replay",
            Self::ValidationRefused => "validation-refused",
            Self::QuotaExceeded => "quota-exceeded",
            Self::AccessDenied => "access-denied",
            Self::NotFound => "not-found",
            Self::StoreUnavailable => "store-unavailable",
        }
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsSsmguiconnectSsmguiconnectState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsSsmguiconnectSsmguiconnectState>)
}
