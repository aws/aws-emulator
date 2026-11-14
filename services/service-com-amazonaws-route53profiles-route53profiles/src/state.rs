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

/// `ProfileStatus` of a resource association that is still being added to the
/// Profile — a member of [`crate::ops::types::PROFILE_STATUS_VALUES`].
pub const STATUS_CREATING: &str = "CREATING";

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
    pub status_message: Option<String>,
    pub creation_time: SystemTime,
    pub modification_time: SystemTime,
}

/// How long a resource association reports [`STATUS_CREATING`] before the move to
/// [`STATUS_COMPLETE`] the clock owes falls due.
///
/// AWS answers `AssociateResourceToProfile` with a resource association that is
/// still being built and reaches `COMPLETE` on its own later; an emulator must not
/// make a caller wait as long as AWS does, so the transition is owed after this
/// span instead. It is deliberately not zero, so a caller reading straight after
/// the associate still observes the transient status, exactly as it would against
/// AWS.
pub const ASSOCIATION_SETTLE_DELAY: Duration = Duration::from_secs(3);

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
    pub creation_time: SystemTime,
    pub modification_time: SystemTime,
    /// The instant at which the move from [`STATUS_CREATING`] to
    /// [`STATUS_COMPLETE`] is owed, or `None` for an association with no
    /// transition in flight.
    pub settled_at: Option<Instant>,
}

impl ProfileResourceAssociation {
    /// This association's `Status` as of `now`: [`STATUS_CREATING`] until the
    /// transition the clock owes falls due, [`STATUS_COMPLETE`] after it.
    pub fn status_at(&self, now: Instant) -> &'static str {
        match self.settled_at {
            Some(due) if now < due => STATUS_CREATING,
            _ => STATUS_COMPLETE,
        }
    }
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

/// The `StatusMessage` reported beside a resource association's `Status`: every
/// status this emulator reports carries the sentence AWS attaches to it.
pub fn resource_association_status_message(status: &str) -> Option<String> {
    match status {
        STATUS_CREATING => Some("Creating the Profile resource association".to_string()),
        STATUS_COMPLETE => Some("Completed creation of Profile resource association".to_string()),
        _ => None,
    }
}

/// The resources a Route 53 Profile can hold, as the `<service>` and
/// resource-type segments of the resource's ARN
/// (`arn:<partition>:<service>:<region>:<account>:<type>/<id>`).
///
/// A Profile carries DNS configuration only: a private hosted zone, a Resolver
/// rule, a DNS Firewall rule group, a Resolver query-logging or DNSSEC-validation
/// configuration, and an interface VPC endpoint. Any other ARN names a resource
/// the service cannot associate, whatever else is well-formed about it.
const ASSOCIABLE_ARN_SEGMENTS: [(&str, &str); 6] = [
    ("route53", "hostedzone"),
    ("route53resolver", "resolver-rule"),
    ("route53resolver", "firewall-rule-group"),
    ("route53resolver", "resolver-query-log-config"),
    ("route53resolver", "resolver-dnssec-config"),
    ("ec2", "vpc-endpoint"),
];

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
        .any(|(owner, associable)| *owner == service && *associable == segment)
        .then(|| segment.to_ascii_uppercase().replace('-', "_"))
}

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

    /// The stored association holding `resource_id`, whatever Profile it names, or
    /// `None` when that VPC is not associated.
    ///
    /// A VPC can have only one Profile associated with it, so this is the lookup
    /// `AssociateProfile` refuses on — independent of the `ProfileId` asked for.
    pub fn association_for_resource(&self, resource_id: &str) -> Option<&ProfileAssociation> {
        self.profile_associations
            .values()
            .find(|association| association.resource_id == resource_id)
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

/// A stored instant as the modelled `Rfc3339Timestamp` the wire carries.
///
/// Built from whole seconds plus sub-second nanoseconds so a stored instant keeps
/// the resolution `SystemTime::now()` gave it; going through an `f64` of epoch
/// seconds would quantise it to roughly a quarter of a microsecond.
pub fn timestamp(at: SystemTime) -> ::aws_smithy_types::DateTime {
    let since_epoch = at.duration_since(UNIX_EPOCH).unwrap_or_default();
    ::aws_smithy_types::DateTime::from_secs_and_nanos(
        i64::try_from(since_epoch.as_secs()).unwrap_or(i64::MAX),
        since_epoch.subsec_nanos(),
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
    }
}

/// The stored resource association as the wire `ProfileResourceAssociation`
/// shape, member for member, with its `Status` and `StatusMessage` as of `now`.
pub fn profile_resource_association_shape(
    association: &ProfileResourceAssociation,
    now: Instant,
) -> crate::ops::types::ProfileResourceAssociation {
    let status = association.status_at(now);
    crate::ops::types::ProfileResourceAssociation {
        id: Some(association.id.clone()),
        name: Some(association.name.clone()),
        owner_id: Some(association.owner_id.clone()),
        profile_id: Some(association.profile_id.clone()),
        resource_arn: Some(association.resource_arn.clone()),
        resource_type: Some(association.resource_type.clone()),
        resource_properties: association.resource_properties.clone(),
        status: Some(status.to_string()),
        status_message: resource_association_status_message(status),
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

/// The message every "this id names nothing" refusal of this service carries: one
/// template shared by every resource kind of the Resolver family this service
/// belongs to, whose only variable part is the id, taken verbatim from the request.
///
/// The family reports its sub-error code inside the message rather than in
/// `Error.Code`, which stays `ResourceNotFoundException`.
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

/// `LimitExceededException` refusing to associate a VPC that a Profile already
/// holds: a VPC admits one Profile association, so a second one asks for more than
/// that per-VPC allowance.
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

/// The `ResourceType` a Profile-resource-association error reports.
pub const PROFILE_RESOURCE_ASSOCIATION_RESOURCE_TYPE: &str = "PROFILE_RESOURCE_ASSOCIATION";

/// `ResourceNotFoundException` for a resource-association id that names no stored
/// resource association.
///
/// The Resolver family answers a lookup of an id it holds nothing for with one
/// template, naming only the id from the request.
pub fn profile_resource_association_not_found(
    association_id: &str,
) -> crate::ops::types::ResourceNotFoundException {
    resource_not_found(association_id, PROFILE_RESOURCE_ASSOCIATION_RESOURCE_TYPE)
}

/// `ValidationException` for a `ResourceArn` naming a resource a Profile cannot
/// hold. The ARN is echoed from the request, which is all that varies.
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
