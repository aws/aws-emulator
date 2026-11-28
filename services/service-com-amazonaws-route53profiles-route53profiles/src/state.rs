// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-route53profiles-route53profiles service.
//!
//! Everything the Profile slice reports is read back off a record stored here, so a
//! create is visible to every later read and a delete is visible as an absence.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// The service's own ARN namespace / sigv4 signing name, as the Smithy model's
/// `aws.api#service` trait declares it (`arnNamespace`/`endpointPrefix`
/// `route53profiles`).
pub const ARN_NAMESPACE: &str = "route53profiles";

/// The region used when a request carries no SigV4 credential scope to read one
/// from (an anonymous, host-routed request). Every generated emulator defaults
/// here.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The resource segment of a Profile ARN, as the service's IAM resource type
/// `profile` spells it (`...:route53profiles:<region>:<account>:profile/<id>`).
const PROFILE_RESOURCE: &str = "profile";

/// The resource segment of a Profile-association ARN, as the service's IAM
/// resource type `profile-association` spells it.
const PROFILE_ASSOCIATION_RESOURCE: &str = "profile-association";

/// The id prefix of a Profile (`rp-` + 16 hex digits).
const PROFILE_ID_PREFIX: &str = "rp";

/// The id prefix of a Profile association (`rpassoc-` + 16 hex digits).
const PROFILE_ASSOCIATION_ID_PREFIX: &str = "rpassoc";

/// The id prefix of a Profile *resource* association (`rpr-` + 16 hex digits) —
/// the id `GetProfileResourceAssociation` and
/// `UpdateProfileResourceAssociation` take.
const PROFILE_RESOURCE_ASSOCIATION_ID_PREFIX: &str = "rpr";

/// `ProfileStatus` of a Profile or association this emulator has finished
/// building — a member of [`crate::ops::types::PROFILE_STATUS_VALUES`].
pub const STATUS_COMPLETE: &str = "COMPLETE";

/// `ProfileStatus` of an association whose resource is still being applied to the
/// Profile: adding a DNS resource CHANGES the Profile the resource is being added
/// to, so that is the status the association carries until the change has been
/// applied — a member of [`crate::ops::types::PROFILE_STATUS_VALUES`].
pub const STATUS_UPDATING: &str = "UPDATING";

/// `ProfileStatus` of a Profile or association that has just been removed.
pub const STATUS_DELETING: &str = "DELETING";

/// `ShareStatus` of a Profile this account owns and has not shared — a member of
/// [`crate::ops::types::SHARE_STATUS_VALUES`].
pub const SHARE_STATUS_NOT_SHARED: &str = "NOT_SHARED";

/// A persisted Route 53 Profile: every member the `Profile` shape reports.
#[derive(Debug, Clone)]
pub struct Profile {
    /// The minted `Id` (`rp-` + 16 hex digits).
    pub id: String,
    /// `Arn`, built once at create time from the request's region.
    pub arn: String,
    pub name: String,
    /// `OwnerId` — the account this emulator presents.
    pub owner_id: String,
    /// `Status`, one of [`crate::ops::types::PROFILE_STATUS_VALUES`].
    pub status: String,
    /// `StatusMessage`; `None` while nothing further needs saying.
    pub status_message: Option<String>,
    /// `ShareStatus`, one of [`crate::ops::types::SHARE_STATUS_VALUES`].
    pub share_status: String,
    pub creation_time: SystemTime,
    pub modification_time: SystemTime,
    /// The `ClientToken` the create call carried, echoed by every read.
    pub client_token: String,
}

/// A persisted association between a Profile and a VPC: every member the
/// `ProfileAssociation` shape reports.
#[derive(Debug, Clone)]
pub struct ProfileAssociation {
    /// The minted `Id` (`rpassoc-` + 16 hex digits).
    pub id: String,
    /// The association's ARN, built once at create time from that request's region
    /// and kept on the record — the key its [`ComAmazonawsRoute53profilesRoute53profilesState::tags`]
    /// entry lives under, so removing the association removes its tags whatever
    /// region the removing request is signed for.
    pub arn: String,
    pub name: String,
    pub owner_id: String,
    pub profile_id: String,
    /// `ResourceId` — the id of the associated VPC.
    pub resource_id: String,
    /// `Status`, one of [`crate::ops::types::PROFILE_STATUS_VALUES`].
    pub status: String,
    /// `StatusMessage` — the sentence belonging to the lifecycle phase this record
    /// is in ([`profile_association_status_message`]).
    pub status_message: Option<String>,
    pub creation_time: SystemTime,
    pub modification_time: SystemTime,
    /// `Priority` — the order Route 53 VPC Resolver evaluates the VPC's Profile
    /// associations in, lowest first. Always a value, because an association made
    /// without one is made with [`DEFAULT_ASSOCIATION_PRIORITY`]
    /// (`ProfileAssociation$Priority`: "Associations created without a priority have
    /// priority 100").
    pub priority: i32,
    /// The end of the propagation of a priority change, which the clock owes this
    /// record, or `None` when nothing is owed ([`OwedTransition`]).
    pub owed_settle: Option<OwedTransition>,
}

/// How long a resource association is reported in the phase it is created in before
/// the move to its settled phase the clock owes falls due.
///
/// AWS answers `AssociateResourceToProfile` with an association that is still being
/// applied to the Profile and settles it on its own later; an emulator must not make
/// a caller wait as long as AWS does, so the transition is owed after this span
/// instead. It is deliberately not zero, so a caller reading straight after the
/// associate still observes the transient phase, exactly as it would against AWS.
pub const ASSOCIATION_SETTLE_DELAY: Duration = Duration::from_secs(3);

/// A change to a record the CLOCK owes, rather than a caller: the move an
/// association makes on its own once the work the associate call started has had
/// time to finish.
///
/// Both halves of one reading of the clock are kept, because they answer different
/// questions: [`Self::due`] is the monotonic instant the move falls due at, which is
/// what a later call compares against, and [`Self::at`] is the wall-clock instant to
/// report as the record's `ModificationTime` once it has — the instant the record
/// CHANGED, not the instant some caller happened to notice, so every caller is told
/// the same one.
#[derive(Debug, Clone, Copy)]
pub struct OwedTransition {
    /// The monotonic instant the move falls due at.
    pub due: Instant,
    /// The wall-clock instant that same moment is, for `ModificationTime`.
    pub at: SystemTime,
}

impl OwedTransition {
    /// The move owed [`ASSOCIATION_SETTLE_DELAY`] after a call served at `started`
    /// (monotonic) / `at` (wall clock).
    pub fn settle_after(started: Instant, at: SystemTime) -> Self {
        Self {
            due: started + ASSOCIATION_SETTLE_DELAY,
            at: at + ASSOCIATION_SETTLE_DELAY,
        }
    }

    /// Whether the clock has already made this move due as of `now`.
    pub fn is_due(&self, now: Instant) -> bool {
        now >= self.due
    }
}

/// A persisted association between a Profile and a DNS resource: every member the
/// `ProfileResourceAssociation` shape reports.
#[derive(Debug, Clone)]
pub struct ProfileResourceAssociation {
    /// The minted `Id` (`rpr-` + 16 hex digits).
    pub id: String,
    pub name: String,
    pub owner_id: String,
    pub profile_id: String,
    /// `ResourceArn` — the ARN of the associated DNS resource, as the request gave
    /// it.
    pub resource_arn: String,
    /// `ResourceType`, derived from [`Self::resource_arn`] by
    /// [`resource_type_of_arn`].
    pub resource_type: String,
    /// `ResourceProperties`, the JSON document the request carried, echoed back
    /// unchanged; `None` when the request named none.
    pub resource_properties: Option<String>,
    /// `Status`, one of [`crate::ops::types::PROFILE_STATUS_VALUES`].
    pub status: String,
    /// `StatusMessage` — the sentence belonging to this record's lifecycle phase and
    /// to the kind of resource it holds
    /// ([`resource_association_status_message`]).
    pub status_message: Option<String>,
    pub creation_time: SystemTime,
    pub modification_time: SystemTime,
    /// The end of this association's application to the Profile, which the clock
    /// owes it, or `None` once that has been applied to the record
    /// ([`OwedTransition`]).
    pub owed_settle: Option<OwedTransition>,
}

impl ProfileResourceAssociation {
    /// This association's `Status` as of `now`: the stored one, with the move to
    /// [`STATUS_COMPLETE`] the clock owes applied when it has already fallen due.
    ///
    /// The answer a read of this record gives at that instant, for a caller holding
    /// only a shared borrow of the store — a data plane asking which resources a
    /// Profile has actually pushed, which has no call of its own to materialise the
    /// move with.
    pub fn status_at(&self, now: Instant) -> &str {
        if self.owed_settle.is_some_and(|owed| owed.is_due(now)) {
            STATUS_COMPLETE
        } else {
            &self.status
        }
    }

    /// Apply the move to [`STATUS_COMPLETE`] the clock owes this record, if it has
    /// already fallen due as of `now`.
    ///
    /// Writes nothing before the due instant and nothing after the move has been
    /// applied, so this only ever materialises a change the clock had already made
    /// due — the record a later caller reads is the one this call leaves behind.
    fn settle_if_due(&mut self, now: Instant) {
        let Some(owed) = self.owed_settle.filter(|owed| owed.is_due(now)) else {
            return;
        };
        self.status = STATUS_COMPLETE.to_string();
        self.status_message = Some(resource_association_status_message(
            AssociationPhase::Settled,
            &self.resource_type,
        ));
        // The record changed when the move fell DUE, so that instant is its
        // modification time whoever reads it and whenever they do.
        self.modification_time = owed.at;
        self.owed_settle = None;
    }
}

impl ProfileAssociation {
    /// Apply the move to [`STATUS_COMPLETE`] the clock owes this record, if it has
    /// already fallen due as of `now`.
    ///
    /// A priority change propagates before it applies to every query from the VPC
    /// (`UpdateProfileAssociation`: the association is `UPDATING` until the change
    /// propagates, and then `COMPLETE`), so that move is the clock's rather than a
    /// caller's: nothing is written before the due instant and nothing after the move
    /// has been applied, and the record this leaves behind is the one every later
    /// caller reads.
    fn settle_if_due(&mut self, now: Instant) {
        let Some(owed) = self.owed_settle.filter(|owed| owed.is_due(now)) else {
            return;
        };
        self.status = STATUS_COMPLETE.to_string();
        self.status_message = Some(profile_association_status_message(AssociationPhase::Settled));
        // The record changed when the move fell DUE, so that instant is its
        // modification time whoever reads it and whenever they do.
        self.modification_time = owed.at;
        self.owed_settle = None;
    }

    /// This association as the wire shape, reported in `phase`: the stored record
    /// with the `Status` and `StatusMessage` of the lifecycle step being reported.
    ///
    /// The lifecycle CALLS report the step they just performed — the model documents
    /// their output member as information about that call, including its status —
    /// where a read reports the stored record.
    pub fn shape_in_phase(
        &self,
        phase: AssociationPhase,
    ) -> crate::ops::types::ProfileAssociation {
        crate::ops::types::ProfileAssociation {
            status: Some(profile_association_status(phase).to_string()),
            status_message: Some(profile_association_status_message(phase)),
            ..profile_association_shape(self)
        }
    }
}

/// Where an association stands in its lifecycle: the phase its `StatusMessage`
/// states, which is what carries the association from the call that asked for it to
/// the record every read reports and on to its removal.
///
/// The phase is NOT the wire `Status`: AWS reports a VPC association `COMPLETE` from
/// the moment it is recorded while the associate response still words its message as
/// a creation in progress, so the two are reported together but are not one value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssociationPhase {
    /// The association is being applied — the phase the associate call reports.
    BeingApplied,
    /// A change a caller asked for is propagating — the phase an update call that
    /// changed how the association is evaluated reports.
    BeingUpdated,
    /// The association has been applied and is in force.
    Settled,
    /// The association is being removed — the phase the disassociate call reports.
    BeingRemoved,
}

/// The `StatusMessage` reported beside a Profile's `Status`: every status this
/// emulator reports for a Profile carries the sentence AWS attaches to it, so no
/// read path has to word one of its own.
///
/// The two statuses a Profile of this emulator is ever reported in are the terminal
/// [`STATUS_COMPLETE`] a created Profile is handed back in and the
/// [`STATUS_DELETING`] the delete response reports; a status nothing reports has no
/// sentence here.
pub fn profile_status_message(status: &str) -> Option<String> {
    match status {
        STATUS_COMPLETE => Some("Created Profile".to_string()),
        STATUS_DELETING => Some("Deleting Profile".to_string()),
        _ => None,
    }
}

/// The `ProfileStatus` a Profile-to-DNS-resource association is reported in while
/// it is in `phase`.
///
/// Applying a DNS resource to a Profile is reported as an UPDATE of the Profile the
/// resource is being added to, not as the creation of something separate, so that is
/// the status such an association carries until it is in force.
pub fn resource_association_status(phase: AssociationPhase) -> &'static str {
    match phase {
        AssociationPhase::BeingApplied | AssociationPhase::BeingUpdated => STATUS_UPDATING,
        AssociationPhase::Settled => STATUS_COMPLETE,
        AssociationPhase::BeingRemoved => STATUS_DELETING,
    }
}

/// The `StatusMessage` reported beside a Profile-to-DNS-resource association's
/// `Status`: the sentence AWS words that lifecycle phase in, which NAMES the kind of
/// resource the association holds, so a client watching one kind of resource being
/// attached reads prose about that kind.
///
/// `resource_type` is the record's own `ResourceType`; the kind is named as
/// [`resource_kind_phrase`] spells it. The sentences for an association being
/// applied and for one that has settled are the wording AWS answers those steps
/// with; the removal sentence is that same template on the step this service's own
/// delete path reports, worded after the Profile's own removal sentence
/// ([`profile_status_message`]).
pub fn resource_association_status_message(phase: AssociationPhase, resource_type: &str) -> String {
    let kind = resource_kind_phrase(resource_type);
    match phase {
        AssociationPhase::BeingApplied | AssociationPhase::BeingUpdated => {
            format!("Updating the Profile to {kind} association")
        }
        AssociationPhase::Settled => {
            format!("Completed creation of Profile to {kind} association")
        }
        AssociationPhase::BeingRemoved => format!("Deleting the Profile to {kind} association"),
    }
}

/// The `ProfileStatus` a Profile-to-VPC association is reported in while it is in
/// `phase`.
///
/// Such an association exists the moment it is recorded — AWS reports it in the
/// terminal [`STATUS_COMPLETE`] from the associate response onwards — so only its
/// removal moves the status; which lifecycle step is being reported is what its
/// `StatusMessage` states.
pub fn profile_association_status(phase: AssociationPhase) -> &'static str {
    match phase {
        AssociationPhase::BeingApplied | AssociationPhase::Settled => STATUS_COMPLETE,
        // A changed priority has to propagate before it applies to every query from
        // the VPC, which is the one thing that moves a live VPC association's status
        // (`UpdateProfileAssociation`'s own documentation).
        AssociationPhase::BeingUpdated => STATUS_UPDATING,
        AssociationPhase::BeingRemoved => STATUS_DELETING,
    }
}

/// The `StatusMessage` reported beside a Profile-to-VPC association's `Status`: the
/// sentence AWS words that lifecycle step in — the creation the associate call
/// started, the created association every read reports, and the removal the
/// disassociate call performed.
///
/// One association kind, so nothing varies but the step — the same shape as the
/// sentences [`profile_status_message`] reports for the Profile itself, whose
/// removal sentence is what the removal step here is worded after.
pub fn profile_association_status_message(phase: AssociationPhase) -> String {
    match phase {
        AssociationPhase::BeingApplied => "Creating Profile Association".to_string(),
        AssociationPhase::BeingUpdated => "Updating Profile Association".to_string(),
        AssociationPhase::Settled => "Created Profile Association".to_string(),
        AssociationPhase::BeingRemoved => "Deleting Profile Association".to_string(),
    }
}

/// The resources a Route 53 Profile can hold: the `<service>` and resource-type
/// segments of the resource's ARN
/// (`arn:<partition>:<service>:<region>:<account>:<type>/<id>`), with the words AWS
/// names that kind of resource by in prose.
///
/// A Profile carries DNS configuration only: a private hosted zone, a Resolver
/// rule, a DNS Firewall rule group, a Resolver query-logging or DNSSEC-validation
/// configuration, and an interface VPC endpoint. Any other ARN names a resource
/// the service cannot associate, whatever else is well-formed about it.
///
/// The prose names are the ones the model's own documentation of these resources
/// uses — `ProfileResourceAssociation$ResourceType` ("Resource type, such as a
/// private hosted zone, or DNS Firewall rule group") and, for the Resolver
/// resources, the resource each ARN segment names — so a message naming the kind
/// names it in the service's own words rather than in the wire's upper-snake
/// spelling.
const ASSOCIABLE_ARN_SEGMENTS: [(&str, &str, &str); 6] = [
    ("route53", "hostedzone", "private hosted zone"),
    ("route53resolver", "resolver-rule", "Resolver rule"),
    (
        "route53resolver",
        "firewall-rule-group",
        "DNS Firewall rule group",
    ),
    (
        "route53resolver",
        "resolver-query-log-config",
        "Resolver query logging configuration",
    ),
    (
        "route53resolver",
        "resolver-dnssec-config",
        "Resolver DNSSEC validation configuration",
    ),
    ("ec2", "vpc-endpoint", "VPC endpoint"),
];

/// The words a `StatusMessage` names the resource of type `resource_type` by, from
/// [`ASSOCIABLE_ARN_SEGMENTS`].
///
/// A type outside that table is named by its own words — the wire spelling read as
/// the words it is made of — so every association reports a sentence about the kind
/// of resource it holds whatever is associated.
pub fn resource_kind_phrase(resource_type: &str) -> String {
    ASSOCIABLE_ARN_SEGMENTS
        .iter()
        .find(|(_, segment, _)| resource_type_of_segment(segment) == resource_type)
        .map(|(_, _, kind)| (*kind).to_string())
        .unwrap_or_else(|| resource_type.to_ascii_lowercase().replace('_', " "))
}

/// The `ResourceType` of the resource `resource_arn` names, or `None` when a
/// Profile cannot hold that resource.
///
/// The type is the ARN's own resource-type segment in the upper-snake spelling the
/// wire uses (`firewall-rule-group` -> `FIREWALL_RULE_GROUP`), so one rule covers
/// every associable resource rather than a table of hand-written answers.
pub fn resource_type_of_arn(resource_arn: &str) -> Option<String> {
    let mut fields = resource_arn.split(':');
    if fields.next()? != "arn" {
        return None;
    }
    // partition, then the service that owns the resource.
    let _partition = fields.next()?;
    let service = fields.next()?;
    // region, account, then the resource itself.
    let _region = fields.next()?;
    let _account = fields.next()?;
    let (segment, resource_id) = fields.next()?.split_once('/')?;
    if resource_id.is_empty() {
        return None;
    }
    ASSOCIABLE_ARN_SEGMENTS
        .iter()
        .any(|(owner, associable, _)| *owner == service && *associable == segment)
        .then(|| resource_type_of_segment(segment))
}

/// The `ResourceType` the wire reports for the resource an ARN's resource-type
/// `segment` names: that segment in the upper-snake spelling the wire uses
/// (`firewall-rule-group` -> `FIREWALL_RULE_GROUP`).
fn resource_type_of_segment(segment: &str) -> String {
    segment.to_ascii_uppercase().replace('-', "_")
}

/// The `Priority` an association made without one gets, from
/// `ProfileAssociation$Priority`'s own documentation: "Associations created without a
/// priority have priority 100."
pub const DEFAULT_ASSOCIATION_PRIORITY: i32 = 100;

/// How many Profile associations ONE VPC admits, from `AssociateProfile`'s own
/// documentation: "A VPC can have up to two Profile associations." A further one is
/// the operation's `LimitExceededException`
/// ([`vpc_association_limit_exceeded`]).
pub const MAX_ASSOCIATIONS_PER_VPC: usize = 2;

/// The most Profiles this emulator stores for one account.
///
/// A ceiling, not a claim about AWS's published quota: it exists so a create loop
/// cannot grow the emulator's heap without bound. `CreateProfile` models
/// `LimitExceededException`, which is the refusal once the store is full.
pub const MAX_PROFILES: usize = 100;

/// The most Profile associations this emulator stores, for the same reason.
/// `AssociateProfile` models `LimitExceededException`.
///
/// Set to the quota that operation's own documentation cites — "a Profile can be
/// associated with 1000 of VPCs (and you can request a higher quota)" — so a client
/// exercising the documented number of VPCs is not refused.
pub const MAX_PROFILE_ASSOCIATIONS: usize = 1000;

/// The most Profile *resource* associations this emulator stores.
///
/// A ceiling, not a claim about AWS's published quota: it exists so an associate
/// loop cannot grow the emulator's heap without bound.
/// `AssociateResourceToProfile` models `LimitExceededException`, which is the
/// refusal once the store is full.
pub const MAX_PROFILE_RESOURCE_ASSOCIATIONS: usize = 1000;

/// The most resources [`ComAmazonawsRoute53profilesRoute53profilesState::tags`] holds
/// an entry for: one per resource that can exist at once.
///
/// Every entry is removed with the resource it describes, and both resource stores are
/// themselves capped, so this ceiling is structurally unreachable; it is enforced
/// anyway so the map cannot outgrow the resources it describes even if a future
/// cleanup path is missed.
pub const MAX_TAGGED_RESOURCES: usize = MAX_PROFILES + MAX_PROFILE_ASSOCIATIONS;

/// The most `ClientToken` entries kept for `CreateProfile` idempotency.
///
/// At most one token is recorded per stored Profile and a token is dropped with
/// the Profile it minted, so this cap is never the binding one in practice; it is
/// enforced anyway, by evicting the oldest token, so the map cannot accumulate
/// caller-chosen strings independently of the Profiles they name.
pub const MAX_CLIENT_TOKENS: usize = MAX_PROFILES;

/// Per-service state for com-amazonaws-route53profiles-route53profiles. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsRoute53profilesRoute53profilesState {
    /// Every Profile, keyed by `Id`. A `BTreeMap` so `ListProfiles` pages in a
    /// stable, resumable order. Bounded by [`MAX_PROFILES`].
    pub profiles: BTreeMap<String, Profile>,
    /// `CreateProfile` idempotency: `ClientToken` -> Profile `Id`. Bounded by
    /// [`MAX_CLIENT_TOKENS`]; write it through [`Self::record_client_token`].
    pub client_tokens: HashMap<String, String>,
    /// The order [`Self::client_tokens`] was written in, so the cap evicts the
    /// oldest token rather than an arbitrary one.
    client_token_order: VecDeque<String>,
    /// Every Profile association, keyed by its own `Id`, in the same stable
    /// order for `ListProfileAssociations`. Bounded by
    /// [`MAX_PROFILE_ASSOCIATIONS`].
    pub profile_associations: BTreeMap<String, ProfileAssociation>,
    /// Every Profile resource association, keyed by its own `Id`, in the same
    /// stable order for `ListProfileResourceAssociations`. Bounded by
    /// [`MAX_PROFILE_RESOURCE_ASSOCIATIONS`].
    pub profile_resource_associations: BTreeMap<String, ProfileResourceAssociation>,
    /// Tags carried by the `Tags` input of `CreateProfile`/`AssociateProfile`,
    /// keyed by the resource's ARN — the key `ListTagsForResource`,
    /// `TagResource` and `UntagResource` take, so that slice reads real data.
    pub tags: BTreeMap<String, BTreeMap<String, String>>,
    /// Per-state clock-derived prefix for minted ids, set on the first mint.
    id_seed: u64,
    /// Monotonic counter making each minted id unique within this state.
    next_id: u64,
}

impl ComAmazonawsRoute53profilesRoute53profilesState {
    /// Apply every resource-association move the clock has ALREADY made due as of
    /// `now`, so each record reports the phase it had reached at that reading.
    ///
    /// Called by the operations that report a resource association. It changes
    /// nothing a caller did not already own: a move is applied only once its stored
    /// due instant has passed, which it would have whether or not anybody called,
    /// and the record it leaves behind is what every later caller reads.
    pub fn settle_due_associations(&mut self, now: Instant) {
        for association in self.profile_resource_associations.values_mut() {
            association.settle_if_due(now);
        }
        for association in self.profile_associations.values_mut() {
            association.settle_if_due(now);
        }
    }

    /// Mint a fresh, unused Profile `Id`.
    pub fn mint_profile_id(&mut self) -> String {
        self.mint_id(PROFILE_ID_PREFIX)
    }

    /// Mint a fresh, unused Profile-association `Id`.
    pub fn mint_profile_association_id(&mut self) -> String {
        self.mint_id(PROFILE_ASSOCIATION_ID_PREFIX)
    }

    /// Mint a fresh, unused Profile-resource-association `Id`.
    pub fn mint_profile_resource_association_id(&mut self) -> String {
        self.mint_id(PROFILE_RESOURCE_ASSOCIATION_ID_PREFIX)
    }

    /// Mint an id of the form `<prefix>-<16 hex digits>`, which fits
    /// `ResourceId`'s modelled 1..64 length for either prefix.
    ///
    /// The clock-derived seed keeps ids from different runs apart; the counter
    /// keeps ids within one run unique, and the loop re-draws rather than ever
    /// handing back an id already in use by either resource kind.
    fn mint_id(&mut self, prefix: &str) -> String {
        if self.id_seed == 0 {
            // 32 bits of the current nanosecond clock -> 8 hex digits.
            self.id_seed = ((nanos_since_epoch() as u64) & 0xFFFF_FFFF).max(1);
        }
        loop {
            self.next_id = self.next_id.wrapping_add(1);
            let id = format!(
                "{prefix}-{:08x}{:08x}",
                self.id_seed,
                self.next_id & 0xFFFF_FFFF
            );
            if !self.profiles.contains_key(&id)
                && !self.profile_associations.contains_key(&id)
                && !self.profile_resource_associations.contains_key(&id)
            {
                return id;
            }
        }
    }

    /// Record `token` as having minted `profile_id`, evicting the oldest token
    /// first if that would push the map past [`MAX_CLIENT_TOKENS`].
    pub fn record_client_token(&mut self, token: String, profile_id: &str) {
        // Re-recording a token already in the map replaces its value rather than
        // growing it, so no eviction is owed and an unrelated live entry is kept.
        if let Some(minted) = self.client_tokens.get_mut(&token) {
            *minted = profile_id.to_string();
            return;
        }
        while self.client_tokens.len() >= MAX_CLIENT_TOKENS {
            match self.client_token_order.pop_front() {
                Some(oldest) => {
                    self.client_tokens.remove(&oldest);
                }
                // The order queue and the map are written together, so this is
                // unreachable; clearing rather than looping forever keeps the cap
                // an invariant even if they ever drift.
                None => {
                    self.client_tokens.clear();
                    break;
                }
            }
        }
        if self
            .client_tokens
            .insert(token.clone(), profile_id.to_string())
            .is_none()
        {
            self.client_token_order.push_back(token);
        }
    }

    /// Drop every idempotency token that minted `profile_id`, so a token never
    /// outlives the Profile it names.
    pub fn forget_client_tokens_for(&mut self, profile_id: &str) {
        self.client_tokens.retain(|_, minted| minted != profile_id);
        let live = &self.client_tokens;
        self.client_token_order.retain(|token| live.contains_key(token));
    }

    /// The id of the stored association of `profile_id` with `resource_id`, or
    /// `None` when that pair is not associated.
    ///
    /// `DisassociateProfile` names the pair, not the association id, so this is what
    /// it resolves its path labels through.
    pub fn association_id_for_pair(&self, profile_id: &str, resource_id: &str) -> Option<String> {
        self.profile_associations
            .values()
            .find(|association| {
                association.profile_id == profile_id && association.resource_id == resource_id
            })
            .map(|association| association.id.clone())
    }

    /// Every stored association holding `resource_id`, whatever Profile each names,
    /// in id order.
    ///
    /// A VPC admits [`MAX_ASSOCIATIONS_PER_VPC`] Profile associations, each with its
    /// own priority, so what a VPC already holds is a SET: this is the lookup
    /// `AssociateProfile` weighs its allowance and its priorities against,
    /// independent of the `ProfileId` asked for.
    pub fn associations_for_resource(&self, resource_id: &str) -> Vec<&ProfileAssociation> {
        self.profile_associations
            .values()
            .filter(|association| association.resource_id == resource_id)
            .collect()
    }

    /// The stored association of `resource_id` that already evaluates at `priority`,
    /// disregarding the one whose id is `besides`, or `None` when that priority is
    /// free for the VPC.
    ///
    /// Route 53 VPC Resolver evaluates a VPC's Profile associations in priority
    /// order, so a priority identifies one of them: both the operation that adds an
    /// association to a VPC and the one that re-prioritises an existing one refuse a
    /// priority another association of the SAME VPC holds. `besides` is the
    /// association being changed, which never conflicts with itself.
    pub fn association_with_priority(
        &self,
        resource_id: &str,
        priority: i32,
        besides: Option<&str>,
    ) -> Option<&ProfileAssociation> {
        self.associations_for_resource(resource_id)
            .into_iter()
            .find(|association| {
                association.priority == priority
                    && besides.is_none_or(|excluded| association.id != excluded)
            })
    }

    /// The stored resource association of `profile_id` with the resource
    /// `resource_arn` names, or `None` when that pair is not associated.
    ///
    /// `AssociateResourceToProfile` names the pair rather than an association id,
    /// so this is the lookup it refuses a repeat on, and
    /// `DisassociateResourceFromProfile` the one it resolves its path labels
    /// through.
    pub fn resource_association_for_pair(
        &self,
        profile_id: &str,
        resource_arn: &str,
    ) -> Option<&ProfileResourceAssociation> {
        self.profile_resource_associations.values().find(|association| {
            association.profile_id == profile_id && association.resource_arn == resource_arn
        })
    }

    /// A stored association that still names `profile_id`, or `None` when none
    /// does. `DeleteProfile` refuses with `ConflictException` while one does, and
    /// the refusal names the association the caller has to delete first, so the
    /// answer is the record rather than a bare yes/no.
    ///
    /// Associations are keyed in a `BTreeMap`, so when several of them block the
    /// delete the one reported is the same on every call.
    pub fn association_blocking_profile(&self, profile_id: &str) -> Option<&ProfileAssociation> {
        self.profile_associations
            .values()
            .find(|association| association.profile_id == profile_id)
    }

    /// Record `tags` against `arn`, replacing whatever was there. An empty list
    /// stores nothing, so an untagged resource has no entry at all, and a new entry
    /// is never added past [`MAX_TAGGED_RESOURCES`].
    pub fn record_tags(&mut self, arn: &str, tags: Vec<crate::ops::types::Tag>) {
        if tags.is_empty() {
            return;
        }
        let tags = tags.into_iter().map(|tag| (tag.key, tag.value)).collect();
        // Replacing an existing resource's tags does not grow the map; a new entry is
        // added only below the cap. The `None` case is unreachable while every entry
        // is dropped with its resource and both resource stores are capped \u2014 the cap
        // is what makes that an invariant rather than an assumption.
        let at_cap = self.tags.len() >= MAX_TAGGED_RESOURCES;
        match self.tags.get_mut(arn) {
            Some(existing) => *existing = tags,
            None if !at_cap => {
                self.tags.insert(arn.to_string(), tags);
            }
            None => {}
        }
    }

    /// Whether the store holds a resource whose ARN is `arn`.
    ///
    /// A tag operation addresses the resource by its ARN, so this is the lookup
    /// each of them resolves that ARN through: a Profile and a Profile association
    /// are the resources this service mints an ARN for, and their records keep it.
    pub fn holds_tagged_resource(&self, arn: &str) -> bool {
        self.profiles.values().any(|profile| profile.arn == arn)
            || self
                .profile_associations
                .values()
                .any(|association| association.arn == arn)
    }

    /// The tags `arn` carries, as the modelled `TagMap` the wire reports: empty for
    /// a resource nothing has tagged.
    pub fn tags_of(&self, arn: &str) -> HashMap<String, String> {
        self.tags
            .get(arn)
            .map(|tags| {
                tags.iter()
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Add `requested` to the tags of `arn`, which is an upsert keyed on the tag
    /// key: a key the resource already carries takes the requested value, and a key
    /// new to the resource is added. Keys are compared exactly, as tag keys are
    /// case-sensitive.
    ///
    /// Answers `false`, leaving the stored tags EXACTLY as they were, when the
    /// result would carry more than [`MAX_TAGS_PER_RESOURCE`] tags.
    ///
    /// An entry is added only for a resource the store holds, and
    /// [`MAX_TAGGED_RESOURCES`] is the two resource stores' caps together, so the
    /// map cannot outgrow that ceiling through this path.
    pub fn merge_tags(&mut self, arn: &str, requested: &HashMap<String, String>) -> bool {
        let held = self.tags.get(arn);
        let added = requested
            .keys()
            .filter(|key| held.is_none_or(|tags| !tags.contains_key(*key)))
            .count();
        if held.map_or(0, BTreeMap::len) + added > MAX_TAGS_PER_RESOURCE {
            return false;
        }
        // A request that names no tag leaves no entry behind, so a resource nobody
        // has tagged still has none at all.
        if requested.is_empty() {
            return true;
        }
        let stored = self.tags.entry(arn.to_string()).or_default();
        for (key, value) in requested {
            stored.insert(key.clone(), value.clone());
        }
        true
    }

    /// Drop every tag of `arn` whose key is named in `keys`, leaving the rest.
    ///
    /// Idempotent: a key the resource does not carry removes nothing, so removing
    /// the same keys twice leaves the same tags. A resource left carrying none has
    /// no entry at all, exactly like one that was never tagged.
    pub fn remove_tags(&mut self, arn: &str, keys: &[String]) {
        let Some(stored) = self.tags.get_mut(arn) else {
            return;
        };
        stored.retain(|key, _| !keys.contains(key));
        if stored.is_empty() {
            self.tags.remove(arn);
        }
    }
}

/// The most tags one resource carries.
///
/// The modelled `TagMap` is `length 0..200` — it is both the map `TagResource`
/// takes and the map `ListTagsForResource` reports — so 200 tags is the most one
/// call can hand over and the most a resource can report back. A write that would
/// take a resource past it is refused and the stored tags are left as they were.
pub const MAX_TAGS_PER_RESOURCE: usize = 200;

/// The resources of this service that carry tags, as the resource-type segment of
/// the resource's ARN with the `ResourceType` an error about it reports.
///
/// A Profile and a Profile association are the resources this service mints an ARN
/// for ([`profile_arn`], [`profile_association_arn`]), so they are the resources a
/// tag operation's `ResourceArn` can name; a Profile *resource* association is
/// addressed by its own id and has no ARN, so no tag hangs off one.
const TAGGABLE_ARN_SEGMENTS: [(&str, &str); 2] = [
    (PROFILE_RESOURCE, PROFILE_RESOURCE_TYPE),
    (PROFILE_ASSOCIATION_RESOURCE, PROFILE_ASSOCIATION_RESOURCE_TYPE),
];

/// The resource a tag operation's `ResourceArn` names: the `ResourceType` an error
/// about it reports, and the id the ARN names it by (the segment after the `/` in
/// `arn:<partition>:<service>:<region>:<account>:<type>/<id>`).
///
/// `None` for anything else — an ARN of another service, one this service mints no
/// resource of that type for, or a string that is no ARN at all — so a tag
/// operation refuses such a value as an invalid ARN rather than reporting a
/// resource that could never exist as merely absent.
pub fn tagged_resource_of_arn(resource_arn: &str) -> Option<(&'static str, &str)> {
    let mut fields = resource_arn.split(':');
    if fields.next()? != "arn" {
        return None;
    }
    // partition, then the service that owns the resource.
    let _partition = fields.next()?;
    if fields.next()? != ARN_NAMESPACE {
        return None;
    }
    // region, account, then the resource itself.
    let _region = fields.next()?;
    let _account = fields.next()?;
    let (segment, resource_id) = fields.next()?.split_once('/')?;
    if resource_id.is_empty() {
        return None;
    }
    TAGGABLE_ARN_SEGMENTS
        .iter()
        .find(|(name, _)| *name == segment)
        .map(|(_, resource_type)| (*resource_type, resource_id))
}

/// `ValidationException` refusing a tag write that would take one resource past
/// [`MAX_TAGS_PER_RESOURCE`] tags.
///
/// `TagResource` models no limit error, so the refusal is the `ValidationException`
/// it does model. The sentence states the limit the modelled `TagMap` fixes and
/// nothing from the request, so the rejected tags are not echoed back: the status
/// (400) and the error shape are what the contract fixes, and the wording is an
/// example.
pub fn tag_limit_exceeded() -> crate::ops::types::ValidationException {
    crate::ops::types::ValidationException {
        message: Some(format!(
            "A resource can have at most {MAX_TAGS_PER_RESOURCE} tags."
        )),
    }
}

/// The ARN of a Profile in `region`.
pub fn profile_arn(region: &str, profile_id: &str) -> String {
    ::engine::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("{PROFILE_RESOURCE}/{profile_id}"),
    )
}

/// The ARN of a Profile association in `region`.
pub fn profile_association_arn(region: &str, association_id: &str) -> String {
    ::engine::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("{PROFILE_ASSOCIATION_RESOURCE}/{association_id}"),
    )
}

/// The region this request was signed for, or [`DEFAULT_REGION`] when it carries
/// no SigV4 credential scope.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .filter(|region| !region.is_empty())
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// How many SUBSECOND digits a timestamp this service reports carries.
///
/// restJson1 writes a timestamp with no `@timestampFormat` as epoch seconds, and every
/// timestamp this service's recorded responses carry has at most three fractional
/// digits — millisecond resolution. The model types the member and says nothing about
/// precision, so this is the resolution the service reports at rather than a modelled
/// constraint: a client comparing one response with another, or decoding these numbers
/// at millisecond resolution, sees a number shaped the way the service shapes it.
const SUBSECOND_DIGITS: u32 = 3;

/// A stored instant as the modelled `Rfc3339Timestamp` the wire carries, at the
/// resolution this service reports one at ([`SUBSECOND_DIGITS`]).
///
/// The ONE encoder for every timestamp this service reports, so no shape's
/// projection can report an instant in a different form from another's.
///
/// Truncated rather than rounded, so the reported instant never moves ahead of the
/// one the clock read: a value is reported at the start of the millisecond it fell
/// in.
pub fn timestamp(at: SystemTime) -> ::aws_smithy_types::DateTime {
    let since_epoch = at.duration_since(UNIX_EPOCH).unwrap_or_default();
    let scale = 10_u32.pow(9 - SUBSECOND_DIGITS);
    ::aws_smithy_types::DateTime::from_secs_and_nanos(
        i64::try_from(since_epoch.as_secs()).unwrap_or(i64::MAX),
        (since_epoch.subsec_nanos() / scale) * scale,
    )
}

/// The stored Profile as the wire `Profile` shape, member for member.
pub fn profile_shape(profile: &Profile) -> crate::ops::types::Profile {
    crate::ops::types::Profile {
        id: Some(profile.id.clone()),
        arn: Some(profile.arn.clone()),
        name: Some(profile.name.clone()),
        owner_id: Some(profile.owner_id.clone()),
        status: Some(profile.status.clone()),
        status_message: profile.status_message.clone(),
        share_status: Some(profile.share_status.clone()),
        creation_time: Some(timestamp(profile.creation_time)),
        modification_time: Some(timestamp(profile.modification_time)),
        client_token: Some(profile.client_token.clone()),
    }
}

/// The stored Profile as the wire `ProfileSummary` shape — the subset
/// `ListProfiles` reports.
pub fn profile_summary_shape(profile: &Profile) -> crate::ops::types::ProfileSummary {
    crate::ops::types::ProfileSummary {
        id: Some(profile.id.clone()),
        arn: Some(profile.arn.clone()),
        name: Some(profile.name.clone()),
        share_status: Some(profile.share_status.clone()),
    }
}

/// The stored association as the wire `ProfileAssociation` shape, member for
/// member.
pub fn profile_association_shape(
    association: &ProfileAssociation,
) -> crate::ops::types::ProfileAssociation {
    crate::ops::types::ProfileAssociation {
        id: Some(association.id.clone()),
        name: Some(association.name.clone()),
        owner_id: Some(association.owner_id.clone()),
        profile_id: Some(association.profile_id.clone()),
        resource_id: Some(association.resource_id.clone()),
        status: Some(association.status.clone()),
        status_message: association.status_message.clone(),
        creation_time: Some(timestamp(association.creation_time)),
        modification_time: Some(timestamp(association.modification_time)),
        priority: Some(association.priority),
    }
}

/// The stored resource association as the wire `ProfileResourceAssociation`
/// shape, member for member.
pub fn profile_resource_association_shape(
    association: &ProfileResourceAssociation,
) -> crate::ops::types::ProfileResourceAssociation {
    crate::ops::types::ProfileResourceAssociation {
        id: Some(association.id.clone()),
        name: Some(association.name.clone()),
        owner_id: Some(association.owner_id.clone()),
        profile_id: Some(association.profile_id.clone()),
        resource_arn: Some(association.resource_arn.clone()),
        resource_type: Some(association.resource_type.clone()),
        resource_properties: association.resource_properties.clone(),
        status: Some(association.status.clone()),
        status_message: association.status_message.clone(),
        creation_time: Some(timestamp(association.creation_time)),
        modification_time: Some(timestamp(association.modification_time)),
    }
}

/// The `@pattern` the model declares on `Name`, verbatim — the shape every `Name`
/// member of this service targets (`smithy_query(query="shape",shape="Name")`:
/// `length 0..64`, this pattern).
///
/// Stated here because this constraint has to be applied HERE rather than left to
/// the centralized constraint walk: that walk compiles a `@pattern` with the `regex`
/// crate, which has no look-around, and treats a pattern it cannot compile as
/// matching every value (`vendor/smithy-schema-server/src/constraints.rs`) — so the
/// leading negative lookahead, the half that forbids an all-digits name, would admit
/// everything. How it is applied is [`name_matches_pattern`]; the text is kept
/// verbatim so the expression a refusal advertises is the model's own.
pub const NAME_PATTERN: &str = r"^(?!^[0-9]+$)([a-zA-Z0-9\-_' ']+)$";

/// The characters [`NAME_PATTERN`]'s class admits, beside the ASCII letters and
/// digits its ranges list: a hyphen (written `\-`, which inside a character class is
/// the literal rather than a range operator), an underscore, an apostrophe and a
/// space.
const NAME_LITERAL_CHARS: [char; 4] = ['-', '_', '\'', ' '];

/// Whether `character` is one [`NAME_PATTERN`]'s class admits. The class lists ASCII
/// ranges, so a letter or digit of another script is not one of them.
fn is_name_char(character: char) -> bool {
    character.is_ascii_alphanumeric() || NAME_LITERAL_CHARS.contains(&character)
}

/// The member path a refusal of a `Name` blames: the modelled member name as the
/// validating walk spells a path segment, which is the Smithy member name with its
/// first character lowercased (`vendor/smithy-schema-server/src/constraints.rs`).
const NAME_MEMBER_PATH: &str = "name";

/// Whether `name` satisfies the modelled [`NAME_PATTERN`].
///
/// Applied as the pattern's independent parts rather than by bringing a look-around
/// regex engine onto a path that matches request text: the pattern's ONLY look-around
/// is the leading negative lookahead, which states exactly "the whole value is not a
/// run of digits", and what it guards is a single character class repeated between
/// the whole-value anchors. So the expression is three independent facts about the
/// value — at least one character, every character one the class admits, and not
/// every character a digit — each of them one pass over it, so the work this does is
/// linear in the length of the value whatever a caller sends.
///
/// WHICH of the parts a name broke is deliberately not reported: the model declares
/// ONE expression for the member, so a name outside it is refused with that
/// expression — see [`name_pattern_violation`] — exactly as a `@pattern` the walk
/// could apply is.
pub(crate) fn name_matches_pattern(name: &str) -> bool {
    !name.is_empty()
        && name.chars().all(is_name_char)
        && !name.chars().all(|character| character.is_ascii_digit())
}

/// The message refusing a `Name` outside [`NAME_PATTERN`], or `None` when the value
/// satisfies it — the ONE refusal path for this constraint, so every operation that
/// takes a `Name` refuses one in the same words.
///
/// Nothing about the sentence is written here: the clause, the expression it
/// advertises and the envelope around it are the framework's and the cross-service
/// validation-message renderings', so a `@pattern` this service has to apply itself
/// is refused in exactly the words a `@pattern` the walk could apply is.
pub fn name_pattern_violation(name: &str) -> Option<String> {
    if name_matches_pattern(name) {
        return None;
    }
    let clause = ::aws_smithy_schema_server::Violation {
        path: NAME_MEMBER_PATH.to_string(),
        kind: ::aws_smithy_schema_server::ViolationKind::Pattern(
            ::service_common::validation::unanchored(NAME_PATTERN),
        ),
        // The clause quotes the value the request carried; `Name` carries no
        // `@sensitive`, so there is nothing to withhold.
        value: Some(name.to_string()),
        sensitive: false,
    }
    .message();
    Some(
        ::service_common::validation_message::envelope_over(std::slice::from_ref(&clause))
            .unwrap_or(clause),
    )
}

/// The `ResourceType` a Profile error reports.
pub const PROFILE_RESOURCE_TYPE: &str = "PROFILE";

/// The `ResourceType` a Profile-association error reports.
pub const PROFILE_ASSOCIATION_RESOURCE_TYPE: &str = "PROFILE_ASSOCIATION";

/// The message a "this id names no resource" refusal of this service carries: one
/// template shared by the resource kinds whose readers report an unresolvable id as
/// a missing resource — a Profile, a Profile association, a tagged resource — whose
/// only variable part is the id, taken verbatim from the request.
///
/// The family reports its sub-error code inside the message rather than in
/// `Error.Code`, which stays `ResourceNotFoundException`.
///
/// A Profile RESOURCE association id is not one of those: see
/// [`profile_resource_association_not_found`] for the template that plane carries, and
/// [`invalid_profile_resource_association_id`] for a value that is no such id at all.
fn resource_not_found_message(id: &str) -> String {
    format!("[RSLVR-05007] Can't find the resource with ID \"{id}\".")
}

/// `ResourceNotFoundException` for an id that names no stored resource of
/// `resource_type`, built from the request so it names the value given.
pub fn resource_not_found(
    id: &str,
    resource_type: &str,
) -> crate::ops::types::ResourceNotFoundException {
    crate::ops::types::ResourceNotFoundException {
        message: Some(resource_not_found_message(id)),
        resource_type: Some(resource_type.to_string()),
    }
}

/// `ResourceNotFoundException` for a `ProfileId` that names no stored Profile.
pub fn profile_not_found(profile_id: &str) -> crate::ops::types::ResourceNotFoundException {
    resource_not_found(profile_id, PROFILE_RESOURCE_TYPE)
}

/// `ResourceNotFoundException` for an association id that names no stored
/// association.
pub fn profile_association_not_found(
    association_id: &str,
) -> crate::ops::types::ResourceNotFoundException {
    resource_not_found(association_id, PROFILE_ASSOCIATION_RESOURCE_TYPE)
}

/// `ResourceNotFoundException` for a (`ProfileId`, `ResourceId`) pair that names no
/// stored association.
///
/// An operation addressed by the pair rather than by an association id cannot say
/// which half of it is at fault, so its refusal names both ids from the request
/// under this family's own sub-error code for an unresolvable pair.
pub fn profile_resource_pair_not_found(
    profile_id: &str,
    resource_id: &str,
) -> crate::ops::types::ResourceNotFoundException {
    crate::ops::types::ResourceNotFoundException {
        message: Some(format!(
            "[RSLVR-05300] Can't locate the Profile ID: \"{profile_id}\" or resource ID \
             \"{resource_id}\"."
        )),
        resource_type: Some(PROFILE_ASSOCIATION_RESOURCE_TYPE.to_string()),
    }
}

/// `ConflictException` refusing to delete a Profile an association still names.
///
/// The one variable part is the id of the association that blocks the delete, read
/// off this service's own state — the resource the caller has to remove before
/// retrying — not the `ProfileId` the request named.
pub fn profile_still_associated(association_id: &str) -> crate::ops::types::ConflictException {
    crate::ops::types::ConflictException {
        message: Some(format!(
            "[RSLVR-05102] The Profile is associated with a resource: \"{association_id}\". \
             Delete the association and try again."
        )),
    }
}

/// `LimitExceededException` refusing to associate a VPC that already holds
/// [`MAX_ASSOCIATIONS_PER_VPC`] Profile associations: a further one asks for more
/// than that per-VPC allowance (`AssociateProfile`: the request fails with a
/// `LimitExceededException` if the VPC already has the maximum number of Profile
/// associations).
///
/// The one variable part is the `ResourceId` the request named.
pub fn vpc_association_limit_exceeded(
    resource_id: &str,
) -> crate::ops::types::LimitExceededException {
    crate::ops::types::LimitExceededException {
        message: Some(format!(
            "VPC [{resource_id}] exceeded its maximum profile association limit."
        )),
        resource_type: Some(PROFILE_ASSOCIATION_RESOURCE_TYPE.to_string()),
    }
}

/// `ValidationException` refusing an association of a VPC that already has one when
/// the request states no `Priority` (`AssociateProfile`: the request fails with a
/// `ValidationException` when you omit `Priority` and the VPC already has a Profile
/// association).
///
/// The VPC's associations are evaluated in priority order, so a further one has to
/// state where in that order it goes; only the first association with a VPC may be
/// made without one, and it gets [`DEFAULT_ASSOCIATION_PRIORITY`]. The one variable
/// part is the `ResourceId` the request named.
pub fn priority_required_for_resource(
    resource_id: &str,
) -> crate::ops::types::ValidationException {
    crate::ops::types::ValidationException {
        message: Some(format!(
            "VPC [{resource_id}] already has a Profile association. Specify a Priority between \
             {MIN_ASSOCIATION_PRIORITY} and {MAX_ASSOCIATION_PRIORITY} that no other Profile \
             association for the VPC uses."
        )),
    }
}

/// `ValidationException` refusing a `Priority` another Profile association of the
/// same VPC already uses (`AssociateProfile` and `UpdateProfileAssociation` both
/// fail with a `ValidationException` when another Profile association for the VPC
/// already uses the priority that you specify).
///
/// The variable parts are the priority the request asked for, the `ResourceId` of
/// the VPC and the id of the association already evaluating at that priority, read
/// off this service's own state.
pub fn priority_already_in_use(
    priority: i32,
    resource_id: &str,
    holder_id: &str,
) -> crate::ops::types::ValidationException {
    crate::ops::types::ValidationException {
        message: Some(format!(
            "Priority {priority} is already used by Profile association \"{holder_id}\" for VPC \
             [{resource_id}]. Specify a priority that no other Profile association for the VPC \
             uses."
        )),
    }
}

/// `ValidationException` refusing a repeat of an association that already exists
/// with a different `Name` or `Priority` (`AssociateProfile`: the request fails with
/// a `ValidationException` when the Profile is already associated with the VPC and
/// you specify a different `Name` or `Priority`; to change the priority, use
/// `UpdateProfileAssociation`).
///
/// The variable parts are the ids the request named and the id of the association
/// that already holds the pair, which is the association an update would address.
pub fn association_not_updatable_by_associate(
    profile_id: &str,
    resource_id: &str,
    association_id: &str,
) -> crate::ops::types::ValidationException {
    crate::ops::types::ValidationException {
        message: Some(format!(
            "Profile [{profile_id}] is already associated with VPC [{resource_id}] by Profile \
             association \"{association_id}\". Use UpdateProfileAssociation to change the Name \
             or Priority of an existing Profile association."
        )),
    }
}

/// The lowest `Priority` a Profile association may be evaluated at, as the model's
/// `Priority` shape declares its `@range` (`smithy_query(query="shape",
/// shape="Priority")`: `range 100..9900`) and `UpdateProfileAssociation`'s
/// documentation states it ("the range 100–9900").
pub const MIN_ASSOCIATION_PRIORITY: i32 = 100;

/// The highest `Priority` a Profile association may be evaluated at, from that same
/// `@range`.
pub const MAX_ASSOCIATION_PRIORITY: i32 = 9900;

/// The `ResourceType` a Profile-resource-association error reports.
pub const PROFILE_RESOURCE_ASSOCIATION_RESOURCE_TYPE: &str = "PROFILE_RESOURCE_ASSOCIATION";

/// `ValidationException` for a `ProfileResourceAssociationId` whose FORM this service
/// cannot read as one of its Profile resource association ids
/// ([`is_profile_resource_association_id`]).
///
/// Such a value is settled as a PARAMETER before any lookup, so it is refused as an id
/// the service will not accept rather than reported as an absent resource — the same
/// refusal, and the same error shape, this family gives every other
/// "<thing> in your request isn't valid" value ([`invalid_resource_arn`]). An id of the
/// right form that resolves to nothing is the other refusal
/// ([`profile_resource_association_not_found`]).
///
/// The one variable part is the id, taken verbatim from the request.
pub fn invalid_profile_resource_association_id(
    association_id: &str,
) -> crate::ops::types::ValidationException {
    crate::ops::types::ValidationException {
        message: Some(format!(
            "[RSLVR-05200] The Profile resource association ID in your request isn't valid: \
             \"{association_id}\". Check the ID and try again."
        )),
    }
}

/// Whether `association_id` has the form of a Profile resource association id of this
/// service: the prefix such an id is minted under followed by hex digits.
///
/// What the FORM decides is which refusal an id the store holds nothing for gets: an id
/// this service could have minted names a resource that is absent, while anything else is
/// a value it does not accept as such an id at all.
pub fn is_profile_resource_association_id(association_id: &str) -> bool {
    association_id
        .strip_prefix(PROFILE_RESOURCE_ASSOCIATION_ID_PREFIX)
        .and_then(|rest| rest.strip_prefix('-'))
        .is_some_and(|digits| {
            !digits.is_empty() && digits.chars().all(|digit| digit.is_ascii_hexdigit())
        })
}

/// `ResourceNotFoundException` for a resource-association id of the right form
/// ([`is_profile_resource_association_id`]) that names no stored resource association —
/// one whose association has been removed, and one that was never minted.
///
/// This plane does NOT share the service's resource-kind-agnostic template
/// ([`resource_not_found_message`]): an id that resolves to no Profile resource
/// association is reported with the association's own sub-error code and sentence,
/// whose single variable part is the id taken verbatim from the request.
pub fn profile_resource_association_not_found(
    association_id: &str,
) -> crate::ops::types::ResourceNotFoundException {
    crate::ops::types::ResourceNotFoundException {
        message: Some(format!(
            "[RSLVR-05200] Can't locate the Profile association with ID: \"{association_id}\"."
        )),
        resource_type: Some(PROFILE_RESOURCE_ASSOCIATION_RESOURCE_TYPE.to_string()),
    }
}

/// `ValidationException` for a `ResourceArn` this service cannot read as naming a
/// resource of the kind the operation addresses — one a Profile cannot hold, or one
/// that is no taggable resource of this service ([`tagged_resource_of_arn`]). The
/// ARN is echoed from the request, which is all that varies.
pub fn invalid_resource_arn(resource_arn: &str) -> crate::ops::types::ValidationException {
    crate::ops::types::ValidationException {
        message: Some(format!(
            "[RSLVR-05022] The ARN in your request isn't valid:\"{resource_arn}\". Provide a \
             valid ARN."
        )),
    }
}

/// The lowest `priority` a DNS Firewall rule group may be associated with, from
/// `AssociateResourceToProfile$ResourceProperties`' own documentation: "The
/// allowed values for priority are between 100 and 9900."
pub const MIN_FIREWALL_PRIORITY: i64 = 100;

/// The highest `priority` a DNS Firewall rule group may be associated with, from
/// the same sentence.
pub const MAX_FIREWALL_PRIORITY: i64 = 9900;

/// `ValidationException` for a DNS Firewall rule group `priority` outside
/// [`MIN_FIREWALL_PRIORITY`]..=[`MAX_FIREWALL_PRIORITY`]: the values outside that
/// range are reserved for the service's own use, so none of them is accepted
/// whatever was asked for — hence a message with no variable part.
pub fn reserved_firewall_priority() -> crate::ops::types::ValidationException {
    crate::ops::types::ValidationException {
        message: Some(format!(
            "[RSLVR-02017] The priority value you provided is reserved. Provide a number \
             between \"{MIN_FIREWALL_PRIORITY}\" and \"{MAX_FIREWALL_PRIORITY}\"."
        )),
    }
}

/// The `ResourceType` of a DNS Firewall rule group, as
/// [`resource_type_of_arn`] spells it: the one resource kind whose
/// `ResourceProperties` carries a `priority`.
pub const FIREWALL_RULE_GROUP_RESOURCE_TYPE: &str = "FIREWALL_RULE_GROUP";

/// The `ResourceProperties` member a DNS Firewall rule group association carries,
/// from `AssociateResourceToProfile$ResourceProperties`' documentation: "If you
/// are adding a DNS Firewall rule group, include also a priority."
pub const PRIORITY_PROPERTY: &str = "priority";

/// Whether the `ResourceProperties` document a request carried states a DNS
/// Firewall `priority` the service accepts.
///
/// Only a DNS Firewall rule group's properties carry a priority, so every other
/// resource type's document passes; for a rule group, a document that states a
/// priority has to state one inside the documented range, and a document that
/// cannot be read as JSON states none that is.
pub fn firewall_priority_accepted(resource_type: &str, resource_properties: Option<&str>) -> bool {
    if resource_type != FIREWALL_RULE_GROUP_RESOURCE_TYPE {
        return true;
    }
    let Some(document) = resource_properties else {
        return true;
    };
    let Ok(parsed) = ::serde_json::from_str::<::serde_json::Value>(document) else {
        return false;
    };
    match parsed.get(PRIORITY_PROPERTY) {
        None => true,
        Some(priority) => priority
            .as_i64()
            .is_some_and(|value| (MIN_FIREWALL_PRIORITY..=MAX_FIREWALL_PRIORITY).contains(&value)),
    }
}

/// The DNS Firewall `priority` the `ResourceProperties` document `resource_properties`
/// states, or `None` when it states none — a document that names no `priority`, one that
/// cannot be read as JSON, and no document at all.
///
/// The reader beside [`firewall_priority_accepted`], so the one shape of that document is
/// read in one place: that function decides which priorities the service ACCEPTS, and this
/// one reads the priority a stored association carries, for whatever orders the rule groups
/// a Profile pushes onto a VPC.
pub fn firewall_priority_of(resource_properties: Option<&str>) -> Option<i64> {
    ::serde_json::from_str::<::serde_json::Value>(resource_properties?)
        .ok()?
        .get(PRIORITY_PROPERTY)?
        .as_i64()
}

/// The message for the one failure that is not about the request: this service was
/// mounted on an engine that holds no store for it.
pub const NO_STORE: &str =
    "the com-amazonaws-route53profiles-route53profiles store is not mounted on this engine";

/// The largest page a list operation of this service returns: `MaxResults`'
/// modelled maximum (`range 1..100`) on both `ListProfiles` and
/// `ListProfileAssociations`.
pub const MAX_PAGE_SIZE: usize = 100;

/// The page size a list call that named no `MaxResults` uses, clamped into the
/// modelled range.
pub fn page_size(max_results: Option<i32>) -> usize {
    max_results
        .and_then(|requested| usize::try_from(requested).ok())
        .unwrap_or(MAX_PAGE_SIZE)
        .clamp(1, MAX_PAGE_SIZE)
}

/// An opaque continuation token resuming strictly after `last_returned_id`.
///
/// `marker` names the issuing operation and token version, so a token one list
/// operation issued is not read as a resume point by another — without it any
/// hex-decodable string would be accepted as a continuation. Hex keeps the token
/// printable whatever an id contains while staying decodable by the next call.
pub fn encode_next_token(marker: &str, last_returned_id: &str) -> String {
    ::hex::encode(format!("{marker}{last_returned_id}"))
}

/// The id a token issued by [`encode_next_token`] under `marker` resumes after,
/// or `None` for any value that operation never issued.
pub fn decode_next_token(marker: &str, token: &str) -> Option<String> {
    let decoded = String::from_utf8(::hex::decode(token).ok()?).ok()?;
    let resume_after = decoded.strip_prefix(marker)?;
    (!resume_after.is_empty()).then(|| resume_after.to_string())
}

/// `InvalidNextTokenException` for a `NextToken` this service never issued.
pub fn invalid_next_token(token: &str) -> crate::ops::types::InvalidNextTokenException {
    crate::ops::types::InvalidNextTokenException {
        message: Some(format!(
            "The NextToken value {token} is not a pagination token this operation issued."
        )),
    }
}

fn nanos_since_epoch() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsRoute53profilesRoute53profilesState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsRoute53profilesRoute53profilesState>)
}
