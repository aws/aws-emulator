// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-macie2-macie2 service.
//!
//! This holds the two records the implemented slice reads and writes: the per
//! region Macie session (`EnableMacie` mints it, `GetMacieSession` projects it,
//! `UpdateMacieSession` edits it and `DisableMacie` retires it together with
//! everything it owns) and the custom-data-identifier registry
//! (`CreateCustomDataIdentifier` mints an entry, `GetCustomDataIdentifier` and
//! `ListCustomDataIdentifiers` project it, `DeleteCustomDataIdentifier` soft
//! deletes it) — plus the per-region automated sensitive data discovery
//! configuration (`UpdateAutomatedDiscoveryConfiguration` writes it,
//! `GetAutomatedDiscoveryConfiguration` projects it) and the per-region
//! findings-publication configuration
//! (`PutFindingsPublicationConfiguration` writes it,
//! `GetFindingsPublicationConfiguration` projects it). Nothing a handler answers
//! with is computed from the request
//! alone: every member of every response is read back out of here.
//!
//! The member plane lives here too: the accounts `CreateMember` associates with
//! this administrator account, which `GetMember` projects and `ListMembers`
//! enumerates.

use std::collections::BTreeMap;
use std::time::SystemTime;

use crate::ops::types::SeverityLevel;

/// Region used when a request carries no SigV4 credential scope to read one
/// from (an unsigned in-process call), matching the emulator's default region.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The `arnNamespace` of this service, per its `aws.api#service` trait — the
/// service segment of every ARN this crate mints.
pub const ARN_SERVICE: &str = "macie2";

/// The resource-type segment of a custom data identifier's ARN. Taken from the
/// only Macie ARN grammar the model pins, `CreateAllowListResponse$arn`
/// (`^arn:(aws|aws-cn|aws-us-gov):macie2:…:allow-list\/[a-z0-9]{22}$`), with
/// this resource's own type in place of `allow-list`.
pub const CUSTOM_DATA_IDENTIFIER_RESOURCE_TYPE: &str = "custom-data-identifier";

/// The resource-type segment of a member account's ARN, read from the same
/// Macie ARN grammar as [`CUSTOM_DATA_IDENTIFIER_RESOURCE_TYPE`] with this
/// resource's own type in place of `allow-list`. A member is addressed by the
/// account id it carries (`GetMember$id` is an account id), so that is the
/// resource portion after this segment.
pub const MEMBER_RESOURCE_TYPE: &str = "member";

/// Width of a Macie resource id whose alphabet the model pins to lowercase
/// alphanumerics: `CreateAllowListResponse$id` is `^[a-z0-9]{22}$` and
/// `ClassificationScopeId` is `^[0-9a-z]*$`, so neither can carry a hyphen.
///
/// This is NOT the id format of every Macie resource: the format a resource's
/// ids take is the one the service issues for THAT resource type, and a custom
/// data identifier's is a canonical UUID ([`new_custom_data_identifier_id`]),
/// which this alphabet could not spell.
pub const RESOURCE_ID_LEN: usize = 22;

/// The resource portion of the service-linked role Amazon Macie uses on an
/// account's behalf, which `GetMacieSession` reports as `serviceRole`.
pub const SERVICE_LINKED_ROLE_RESOURCE: &str =
    "role/aws-service-role/macie.amazonaws.com/AWSServiceRoleForAmazonMacie";

/// The `status` a session takes when `EnableMacie` names none: the member's doc
/// states "To enable Amazon Macie and start all Macie activities for the
/// account, set this value to ENABLED", which is what enabling Macie asks for.
/// An entry of [`crate::ops::types::MACIE_STATUS_VALUES`].
pub const DEFAULT_MACIE_STATUS: &str = "ENABLED";

/// The `findingPublishingFrequency` a session takes when the request names
/// none. An entry of
/// [`crate::ops::types::FINDING_PUBLISHING_FREQUENCY_VALUES`], and the
/// frequency with which Macie publishes policy-finding updates by default.
pub const DEFAULT_FINDING_PUBLISHING_FREQUENCY: &str = "FIFTEEN_MINUTES";

/// The `AutomatedDiscoveryStatus` under which Macie performs automated
/// sensitive data discovery activities: the status doc states "ENABLED, use the
/// specified settings to perform automated sensitive data discovery
/// activities". An entry of
/// [`crate::ops::types::AUTOMATED_DISCOVERY_STATUS_VALUES`].
pub const AUTOMATED_DISCOVERY_ENABLED: &str = "ENABLED";

/// The `AutomatedDiscoveryStatus` under which Macie performs none of those
/// activities: "DISABLED, don't perform automated sensitive data discovery
/// activities". An entry of
/// [`crate::ops::types::AUTOMATED_DISCOVERY_STATUS_VALUES`].
pub const AUTOMATED_DISCOVERY_DISABLED: &str = "DISABLED";

/// The automated sensitive data discovery `status` a read reports for an
/// account that has never configured it: nothing has asked for those
/// activities, so none are performed.
pub const DEFAULT_AUTOMATED_DISCOVERY_STATUS: &str = AUTOMATED_DISCOVERY_DISABLED;

/// The `autoEnableOrganizationMembers` an automated-discovery configuration
/// takes when the request names none: the member's doc states "Valid values
/// are: ALL (default), enable it for all existing accounts and new member
/// accounts". An entry of [`crate::ops::types::AUTO_ENABLE_MODE_VALUES`].
pub const DEFAULT_AUTO_ENABLE_ORGANIZATION_MEMBERS: &str = "ALL";

/// Whether sensitive data (classification) findings are published to Security
/// Hub when nothing has configured findings publication: the member's doc
/// states "The default value is false."
pub const DEFAULT_PUBLISH_CLASSIFICATION_FINDINGS: bool = false;

/// Whether policy findings are published to Security Hub when nothing has
/// configured findings publication: the member's doc states "The default value
/// is true."
pub const DEFAULT_PUBLISH_POLICY_FINDINGS: bool = true;

/// How a reporting operation states that this account has no Macie session in
/// the region the request addressed. Macie has to be enabled before any of its
/// resources can be read or written.
pub const MACIE_NOT_ENABLED_MESSAGE: &str = "Macie is not enabled.";

/// The widest `name` a custom data identifier may carry: its member doc states
/// "The name can contain as many as 128 characters."
pub const MAX_NAME_CHARS: usize = 128;

/// The widest `regex` a custom data identifier may carry: its member doc states
/// "The expression can contain as many as 512 characters."
pub const MAX_REGEX_CHARS: usize = 512;

/// The widest `description` a custom data identifier may carry: its member doc
/// states "The description can contain as many as 512 characters."
pub const MAX_DESCRIPTION_CHARS: usize = 512;

/// The most `keywords` one custom data identifier may list: its member doc
/// states "The array can contain as many as 50 keywords."
pub const MAX_KEYWORDS: usize = 50;

/// The narrowest keyword, in characters: its member doc states "Each keyword
/// can contain 3-90 UTF-8 characters."
pub const MIN_KEYWORD_CHARS: usize = 3;

/// The widest keyword, in characters. Same doc sentence as
/// [`MIN_KEYWORD_CHARS`].
pub const MAX_KEYWORD_CHARS: usize = 90;

/// The most `ignoreWords` one custom data identifier may list: its member doc
/// states "The array can contain as many as 10 ignore words."
pub const MAX_IGNORE_WORDS: usize = 10;

/// The narrowest ignore word, in characters: its member doc states "Each ignore
/// word can contain 4-90 UTF-8 characters."
pub const MIN_IGNORE_WORD_CHARS: usize = 4;

/// The widest ignore word, in characters. Same doc sentence as
/// [`MIN_IGNORE_WORD_CHARS`].
pub const MAX_IGNORE_WORD_CHARS: usize = 90;

/// The smallest `maximumMatchDistance`: its member doc states "The distance can
/// be 1-300 characters."
pub const MIN_MAXIMUM_MATCH_DISTANCE: i32 = 1;

/// The largest `maximumMatchDistance`. Same doc sentence as
/// [`MIN_MAXIMUM_MATCH_DISTANCE`].
pub const MAX_MAXIMUM_MATCH_DISTANCE: i32 = 300;

/// The `maximumMatchDistance` a custom data identifier takes when the request
/// names none: its member doc states "The default value is 50."
pub const DEFAULT_MAXIMUM_MATCH_DISTANCE: i32 = 50;

/// The most `severityLevels` one custom data identifier may list: its member doc
/// states "You can specify as many as three SeverityLevel objects in this
/// array, one for each severity: LOW, MEDIUM, or HIGH."
pub const MAX_SEVERITY_LEVELS: usize = 3;

/// The severities a `SeverityLevel` may assign, LOWEST FIRST. The member doc
/// requires the occurrences thresholds to be "in ascending order by severity,
/// moving from LOW to HIGH", so this order is the rule rather than the
/// alphabetical order of [`crate::ops::types::DATA_IDENTIFIER_SEVERITY_VALUES`].
pub const SEVERITIES_ASCENDING: [&str; 3] = ["LOW", "MEDIUM", "HIGH"];

/// The `occurrencesThreshold` of the severity level the service applies when a
/// create names no `severityLevels`: the member doc states "By default, Amazon
/// Macie creates findings for S3 objects that contain at least one occurrence
/// of text that matches the detection criteria", so one occurrence is enough.
pub const DEFAULT_SEVERITY_OCCURRENCES_THRESHOLD: i64 = 1;

/// The `severity` of that same default level: the doc sentence continues "and
/// Macie assigns the MEDIUM severity to those findings". An entry of
/// [`SEVERITIES_ASCENDING`].
pub const DEFAULT_SEVERITY: &str = "MEDIUM";

/// The page size `ListCustomDataIdentifiers` uses when a request names no
/// `maxResults`. The member is a bare `__integer` with neither a modelled range
/// nor a documented default, so this is the EMULATOR's choice.
pub const DEFAULT_PAGE_SIZE: usize = 50;

/// The most summaries one `ListCustomDataIdentifiers` response carries, however
/// large a `maxResults` a request asks for. The EMULATOR's own bound on the
/// amplification between a tiny request and the response it produces, not an
/// AWS rule.
pub const MAX_PAGE_SIZE: usize = 200;

/// The most members one `ListMembers` response carries, which is also the page
/// size a request naming no `maxResults` gets: that operation's `maxResults`
/// carries a modelled range of 1 to 25, so 25 is the page the service itself
/// bounds this read to.
pub const MAX_MEMBER_PAGE_SIZE: usize = 25;

/// How a paginated read refuses a `nextToken` it did not issue. A token is this
/// service's own opaque encoding of a page cursor, so one it did not hand out
/// names no page.
pub const FOREIGN_PAGE_TOKEN_MESSAGE: &str =
    "The nextToken is not a token that this service issued.";

/// The most custom data identifiers one emulator process stores, across every
/// region. The EMULATOR's own memory bound, not an AWS quota: without it any
/// caller could grow one process's heap without limit. A create past it answers
/// the `ServiceQuotaExceededException` the operation models.
pub const MAX_CUSTOM_DATA_IDENTIFIERS: usize = 10_000;

/// The most member accounts one emulator process associates, across every
/// region. The EMULATOR's own memory bound, on the same footing as
/// [`MAX_CUSTOM_DATA_IDENTIFIERS`]: a create past it answers the
/// `ServiceQuotaExceededException` the operation models.
pub const MAX_MEMBERS: usize = 10_000;

/// The `RelationshipStatus` a member account carries the moment `CreateMember`
/// associates it: the association exists and no membership invitation has been
/// sent for it yet. An entry of the modelled `RelationshipStatus` enum.
pub const MEMBER_CREATED_RELATIONSHIP_STATUS: &str = "Created";

/// The `RelationshipStatus` values under which an account IS a current member
/// of its administrator account — the membership has been established and is
/// still live — which is what a read reporting "only current member accounts"
/// reports. Entries of the modelled `RelationshipStatus` enum:
///
/// * `Enabled` — the membership is in force;
/// * `Paused` — in force, with Macie suspended for the account;
/// * `Invited` — the membership invitation is outstanding, so the
///   establishment of the membership is under way.
///
/// Every other entry of that enum is an account this administrator has a
/// record of but no live membership with: the association exists and nothing
/// has been sent to the account yet
/// ([`MEMBER_CREATED_RELATIONSHIP_STATUS`]), the invitation never reached it
/// (`EmailVerificationInProgress`, `EmailVerificationFailed`), the membership
/// ended because the administrator removed the account (`Removed`) or the
/// account itself resigned (`Resigned`), or the account cannot be administered
/// here at all (`RegionDisabled`, `AccountSuspended`). Membership is stated
/// POSITIVELY so that a status this list does not name is never taken for a
/// live membership.
///
/// The model names the ten statuses but does not settle which of them count as
/// current, so only `Created` is pinned by what `CreateMember` and
/// `ListMembers` document between them; the rest of the partition — `Invited`
/// above all — is read off the member-plane lifecycle and is worth revisiting
/// when the operations that move an account between these statuses are
/// implemented.
pub const CURRENT_MEMBER_RELATIONSHIP_STATUSES: [&str; 3] = ["Enabled", "Paused", "Invited"];

/// How many regions one emulator process keeps Macie state for. The region a
/// request addresses comes off the wire, so the NUMBER of per-region records is
/// caller-chosen; this bounds it. The emulator's own limit, well above the
/// handful of regions any one client session uses.
pub const MAX_EMULATED_REGIONS: usize = 32;

/// The single Macie session of one account in one region.
///
/// Every member of `GetMacieSessionResponse` is a field here.
#[derive(Debug, Clone)]
pub struct MacieSession {
    /// `MacieStatus` as the wire words it: an entry of
    /// [`crate::ops::types::MACIE_STATUS_VALUES`].
    pub status: String,
    /// `FindingPublishingFrequency` as the wire words it: an entry of
    /// [`crate::ops::types::FINDING_PUBLISHING_FREQUENCY_VALUES`].
    pub finding_publishing_frequency: String,
    /// The service-linked role Macie uses for this account, as an ARN.
    pub service_role: String,
    pub created_at: SystemTime,
    pub updated_at: SystemTime,
}

/// The automated sensitive data discovery configuration of one account in one
/// region.
///
/// Every member of `GetAutomatedDiscoveryConfigurationResponse` is a field here
/// or projected from one. The record exists once something has configured
/// automated sensitive data discovery for the region; until then a read reports
/// the never-configured defaults.
#[derive(Debug, Clone)]
pub struct AutomatedDiscoveryConfiguration {
    /// `AutomatedDiscoveryStatus` as the wire words it: an entry of
    /// [`crate::ops::types::AUTOMATED_DISCOVERY_STATUS_VALUES`].
    pub status: String,
    /// `AutoEnableMode` as the wire words it: an entry of
    /// [`crate::ops::types::AUTO_ENABLE_MODE_VALUES`].
    pub auto_enable_organization_members: String,
    /// The classification scope automated sensitive data discovery uses, which
    /// Macie materialises together with this configuration.
    pub classification_scope_id: String,
    /// The sensitivity inspection template automated sensitive data discovery
    /// uses, materialised with this configuration too.
    pub sensitivity_inspection_template_id: String,
    /// When automated sensitive data discovery was INITIALLY enabled, which is
    /// `None` until it has been enabled once: its member doc states "This value
    /// is null if automated sensitive data discovery has never been enabled."
    pub first_enabled_at: Option<SystemTime>,
    /// When it was MOST RECENTLY disabled, which is `None` while it is enabled:
    /// its member doc states "This value is null if automated sensitive data
    /// discovery is currently enabled."
    pub disabled_at: Option<SystemTime>,
    /// When these settings or this status most recently CHANGED.
    pub last_updated_at: SystemTime,
}

/// The settings that determine which findings one account's Macie publishes to
/// Security Hub, in one region.
///
/// Both members of the modelled `SecurityHubConfiguration` are required, so a
/// stored configuration always carries both; an account that has never written
/// one is reported with [`DEFAULT_PUBLISH_CLASSIFICATION_FINDINGS`] and
/// [`DEFAULT_PUBLISH_POLICY_FINDINGS`], the values the two members document.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FindingsPublicationConfiguration {
    /// Whether sensitive data findings are published to Security Hub.
    pub publish_classification_findings: bool,
    /// Whether policy findings are published to Security Hub.
    pub publish_policy_findings: bool,
}

impl Default for FindingsPublicationConfiguration {
    /// The configuration an account has until something writes one.
    fn default() -> Self {
        Self {
            publish_classification_findings: DEFAULT_PUBLISH_CLASSIFICATION_FINDINGS,
            publish_policy_findings: DEFAULT_PUBLISH_POLICY_FINDINGS,
        }
    }
}

/// A stored custom data identifier.
///
/// Every member of `GetCustomDataIdentifierResponse` and of
/// `CustomDataIdentifierSummary` is a field here or projected from one.
#[derive(Debug, Clone)]
pub struct CustomDataIdentifier {
    pub id: String,
    pub arn: String,
    /// The region the create that minted this identifier addressed. Macie is a
    /// regional service, so an identifier is only visible to a request naming
    /// the region it was created in — which is also the region in its ARN.
    pub region: String,
    pub name: String,
    pub description: Option<String>,
    pub regex: String,
    pub keywords: Vec<String>,
    pub ignore_words: Vec<String>,
    /// Always stored, because the member carries a documented default
    /// ([`DEFAULT_MAXIMUM_MATCH_DISTANCE`]) that the service applies when the
    /// request names none.
    pub maximum_match_distance: i32,
    pub severity_levels: Vec<SeverityLevel>,
    /// Tags in a `BTreeMap` so a read reports them in a stable order.
    pub tags: BTreeMap<String, String>,
    pub created_at: SystemTime,
    /// Whether `DeleteCustomDataIdentifier` has retired this identifier. That
    /// operation "Soft deletes a custom data identifier", so the record stays
    /// and this flag is what `GetCustomDataIdentifierResponse$deleted` reports.
    pub deleted: bool,
}

impl CustomDataIdentifier {
    /// This identifier's `keywords`, as a read reports them: PRESENT, and an
    /// empty array when it lists none.
    ///
    /// A list member of a custom data identifier read is always serialized, so
    /// "lists nothing" is an empty array rather than an absent member — the
    /// difference a client that iterates the collection depends on.
    pub fn keywords_output(&self) -> Option<Vec<String>> {
        Some(self.keywords.clone())
    }

    /// This identifier's `ignoreWords`, as a read reports them, on the same
    /// footing as [`Self::keywords_output`].
    pub fn ignore_words_output(&self) -> Option<Vec<String>> {
        Some(self.ignore_words.clone())
    }

    /// This identifier's `severityLevels`, as a read reports them. A stored
    /// identifier always carries the level set that is in effect — the one the
    /// create named, or [`default_severity_levels`] — so this reports the
    /// severity configuration a client reads to learn what findings will carry.
    pub fn severity_levels_output(&self) -> Option<Vec<SeverityLevel>> {
        Some(self.severity_levels.clone())
    }

    /// This identifier's `tags`, as a read reports them: the modelled `TagMap`
    /// is a hash map, and absent rather than empty when it carries none.
    pub fn tags_output(&self) -> Option<std::collections::HashMap<String, String>> {
        if self.tags.is_empty() {
            return None;
        }
        Some(
            self.tags
                .iter()
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect(),
        )
    }
}

/// An account associated with this Macie administrator account, in one region.
///
/// Every member of `GetMemberResponse` and of the modelled `Member` structure is
/// a field here or projected from one.
#[derive(Debug, Clone)]
pub struct Member {
    /// The associated account's id, which is also how a read addresses it
    /// (`GetMember$id`) and the resource portion of [`Self::arn`].
    pub account_id: String,
    /// The email address the association was created with.
    pub email: String,
    pub arn: String,
    /// The region the `CreateMember` that associated this account addressed.
    /// Macie is a regional service, so an association is only visible to a
    /// request naming that region — which is also the region in its ARN.
    pub region: String,
    /// The account that administers this member, which is the account the
    /// create was made by.
    pub administrator_account_id: String,
    /// `RelationshipStatus` as the wire words it.
    pub relationship_status: String,
    /// Tags in a `BTreeMap` so a read reports them in a stable order.
    pub tags: BTreeMap<String, String>,
    /// When a membership invitation was last sent to the account, which is
    /// `None` until one is: the member's doc states "This value is null if a
    /// Macie membership invitation hasn't been sent to the account."
    pub invited_at: Option<SystemTime>,
    /// When the status of this relationship most recently changed, which
    /// `updatedAt` reports.
    pub updated_at: SystemTime,
}

impl Member {
    /// Whether this account is a CURRENT member of its administrator account —
    /// its membership is established and still live — which is what a
    /// `ListMembers` reports unless it asks for all accounts. See
    /// [`CURRENT_MEMBER_RELATIONSHIP_STATUSES`] for the statuses that is true
    /// under.
    pub fn is_current_association(&self) -> bool {
        CURRENT_MEMBER_RELATIONSHIP_STATUSES.contains(&self.relationship_status.as_str())
    }

    /// This member's `tags`, as a read reports them: the modelled `TagMap` is a
    /// hash map, and absent rather than empty when it carries none.
    pub fn tags_output(&self) -> Option<std::collections::HashMap<String, String>> {
        if self.tags.is_empty() {
            return None;
        }
        Some(
            self.tags
                .iter()
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect(),
        )
    }
}

/// Per-service state for com-amazonaws-macie2-macie2. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsMacie2Macie2State {
    /// The Macie session of each region this emulator has seen an `EnableMacie`
    /// for. Macie is enabled per account and Region, so there is one session
    /// per region rather than one per process.
    pub sessions: BTreeMap<String, MacieSession>,
    /// The automated sensitive data discovery configuration of each region
    /// something has configured it for. A key of this map is always a key of
    /// [`Self::sessions`]: Macie has to be enabled in a region before its
    /// automated discovery can be configured there.
    pub automated_discovery: BTreeMap<String, AutomatedDiscoveryConfiguration>,
    /// The findings-publication configuration of each region something has
    /// written one for (`PutFindingsPublicationConfiguration`), which
    /// `GetFindingsPublicationConfiguration` reports. A key of this map is
    /// always a key of [`Self::sessions`], for the same reason as
    /// [`Self::automated_discovery`].
    pub findings_publication: BTreeMap<String, FindingsPublicationConfiguration>,
    /// Custom data identifiers keyed by id, so an id-addressed read is
    /// O(log n) and an enumeration is in a stable total order.
    pub custom_data_identifiers: BTreeMap<String, CustomDataIdentifier>,
    /// The accounts associated with this administrator account, keyed by the
    /// region the association was made in and the member's account id — the
    /// same account may be a member in two regions, and each association is its
    /// own record.
    pub members: BTreeMap<(String, String), Member>,
    /// [`idempotency_key`] -> the resource the call carrying that client token
    /// produced: the region for `EnableMacie`, the minted id for
    /// `CreateCustomDataIdentifier`. This is what makes a retry replay the
    /// original result instead of producing a second one.
    pub idempotency: BTreeMap<String, String>,
    /// How many service-assigned identifiers this store has handed out.
    ///
    /// This is what makes every minted id UNIQUE: [`Self::next_id_seed`] hands
    /// out each value once and [`new_resource_id`] turns it into an id by a
    /// mapping injective in the seed, so two identifiers can never coincide
    /// however the clock behaves.
    pub minted_ids: u64,
}

impl ComAmazonawsMacie2Macie2State {
    /// The Macie session of `region`, or `None` when Macie is not enabled
    /// there.
    pub fn session(&self, region: &str) -> Option<&MacieSession> {
        self.sessions.get(region)
    }

    /// The automated sensitive data discovery configuration of `region`, or
    /// `None` when nothing has configured it there yet.
    pub fn automated_discovery_configuration(
        &self,
        region: &str,
    ) -> Option<&AutomatedDiscoveryConfiguration> {
        self.automated_discovery.get(region)
    }

    /// The findings-publication configuration of `region`, or `None` when
    /// nothing has written one there yet.
    pub fn findings_publication_configuration(
        &self,
        region: &str,
    ) -> Option<&FindingsPublicationConfiguration> {
        self.findings_publication.get(region)
    }

    /// The custom data identifier `id` of `region`, soft deleted or not.
    /// `None` when no identifier of that region carries the id, which is what
    /// an id from another region answers too.
    pub fn identifier(&self, region: &str, id: &str) -> Option<&CustomDataIdentifier> {
        self.custom_data_identifiers
            .get(id)
            .filter(|identifier| identifier.region == region)
    }

    /// The same record, for a write.
    pub fn identifier_mut(&mut self, region: &str, id: &str) -> Option<&mut CustomDataIdentifier> {
        self.custom_data_identifiers
            .get_mut(id)
            .filter(|identifier| identifier.region == region)
    }

    /// The identifiers `ListCustomDataIdentifiers` enumerates for `region`, in
    /// id order.
    ///
    /// Soft-deleted identifiers are left out: `CustomDataIdentifierSummary`
    /// carries no `deleted` member, so a page could not tell a live identifier
    /// from a retired one, and what the operation retrieves is "information
    /// about the custom data identifiers for an account".
    pub fn live_identifiers<'a>(
        &'a self,
        region: &'a str,
    ) -> impl Iterator<Item = &'a CustomDataIdentifier> {
        self.custom_data_identifiers
            .values()
            .filter(move |identifier| identifier.region == region && !identifier.deleted)
    }

    /// Retire everything Macie holds for `region`: `DisableMacie` "Disables
    /// Amazon Macie and deletes all settings and resources for a Macie
    /// account", so the session, the region's automated-discovery
    /// configuration, its custom data identifiers and the client tokens that
    /// minted them all go.
    pub fn forget_region(&mut self, region: &str) {
        self.sessions.remove(region);
        self.automated_discovery.remove(region);
        self.findings_publication.remove(region);
        self.custom_data_identifiers
            .retain(|_, identifier| identifier.region != region);
        self.members.retain(|_, member| member.region != region);
        let prefix = region_key_prefix(region);
        self.idempotency.retain(|key, _| !key.starts_with(&prefix));
    }

    /// The account `account_id` associated in `region`, or `None` when no
    /// association of that region names it — which is what an account
    /// associated only in another region answers too.
    pub fn member(&self, region: &str, account_id: &str) -> Option<&Member> {
        self.members
            .get(&(region.to_string(), account_id.to_string()))
    }

    /// The same record, for a write.
    pub fn member_mut(&mut self, region: &str, account_id: &str) -> Option<&mut Member> {
        self.members
            .get_mut(&(region.to_string(), account_id.to_string()))
    }

    /// The accounts associated in `region`, in account-id order — the order the
    /// store holds them in, so the total order a page boundary rests on is
    /// stable.
    pub fn members_of<'a>(&'a self, region: &'a str) -> impl Iterator<Item = &'a Member> {
        self.members
            .iter()
            .filter(move |((member_region, _), _)| member_region == region)
            .map(|(_, member)| member)
    }

    /// The next identifier seed, handed out exactly once. Call it once per id a
    /// handler is about to mint; [`new_resource_id`] turns it into the id. A
    /// seed that goes unused costs nothing but the counter advancing.
    pub fn next_id_seed(&mut self) -> u64 {
        self.minted_ids = self.minted_ids.wrapping_add(1);
        self.minted_ids
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsMacie2Macie2State>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsMacie2Macie2State>)
}

/// The region this request was signed for, which is the region its Macie
/// session and its custom data identifiers belong to.
///
/// Read from the SigV4 credential scope, which is where a request names its
/// region. A scope region not shaped like an AWS region cannot appear in the
/// modelled ARN grammar, so anything else falls back to [`DEFAULT_REGION`]
/// rather than minting an ARN the model rejects.
pub fn request_region(ctx: &smithy_server_runtime::OperationContext<'_>) -> String {
    ::emulator_server::sigv4_credential_scope(&ctx.parts().headers)
        .map(|scope| scope.region)
        .filter(|region| is_region_shaped(region))
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// Whether `region` matches the region segment the modelled Macie ARN grammar
/// allows (`[a-z1-9-]{9,20}` on `CreateAllowListResponse$arn`), read as the
/// usual AWS region grammar: lowercase alphanumeric parts joined by `-`,
/// starting with a letter.
fn is_region_shaped(region: &str) -> bool {
    region.starts_with(|c: char| c.is_ascii_lowercase())
        && region.split('-').all(|part| {
            !part.is_empty() && part.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        })
}

/// Separates the parts of a composite store key. A unit separator occurs in
/// none of them: a region is `[a-z0-9-]`, an operation name is an identifier,
/// and a `clientToken` that carried one could not be sent as a JSON string
/// member without escaping.
const KEY_SEPARATOR: char = '\u{1f}';

/// The prefix every [`idempotency_key`] of `region` starts with, which is what
/// lets [`ComAmazonawsMacie2Macie2State::forget_region`] drop exactly that
/// region's tokens.
fn region_key_prefix(region: &str) -> String {
    format!("{region}{KEY_SEPARATOR}")
}

/// The key a `clientToken` replay is remembered under: the region the request
/// addressed, the operation it called, and the token.
///
/// The region is part of it because a Macie resource belongs to one region, so
/// the same token used against two regions is two different calls. The
/// operation is part of it because the two operations that accept a token mint
/// different things.
pub fn idempotency_key(region: &str, operation: &str, client_token: &str) -> String {
    format!("{region}{KEY_SEPARATOR}{operation}{KEY_SEPARATOR}{client_token}")
}

/// Mint the ARN of a custom data identifier:
/// `arn:aws:macie2:<region>:<account>:custom-data-identifier/<id>`.
pub fn custom_data_identifier_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("{CUSTOM_DATA_IDENTIFIER_RESOURCE_TYPE}/{id}"),
    )
}

/// Render a page cursor — the record a page ended on — as the opaque
/// `nextToken` the next request replays. A `nextToken` is modelled as a bare
/// string, so the cursor is rendered in hex: an alphabet that survives any
/// transport and that no client can mistake for a value it may construct
/// itself.
pub fn encode_page_token(cursor: &str) -> String {
    ::hex::encode(cursor)
}

/// Read a cursor back out of a `nextToken`. `None` when the token is not one
/// this service issued.
pub fn decode_page_token(token: &str) -> Option<String> {
    String::from_utf8(::hex::decode(token).ok()?).ok()
}

/// How the service refuses a call addressing a custom data identifier the
/// addressed region's registry does not hold. The fixed text is the service's;
/// the only variable part is the id the request named.
pub fn custom_data_identifier_not_found_message(id: &str) -> String {
    format!("Custom Data Identifier not found for id: {id}")
}

/// Mint the ARN of an associated member account:
/// `arn:aws:macie2:<region>:<account>:member/<memberAccountId>`.
pub fn member_arn(region: &str, account_id: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("{MEMBER_RESOURCE_TYPE}/{account_id}"),
    )
}

/// The ARN of the service-linked role Macie uses on this account's behalf,
/// which is what `GetMacieSessionResponse$serviceRole` reports. An IAM ARN, so
/// it carries no region.
pub fn service_role_arn() -> String {
    ::engine::emulator_arn("iam", "", SERVICE_LINKED_ROLE_RESOURCE)
}

/// The id of a Macie resource whose ids the service issues as
/// [`RESOURCE_ID_LEN`] lowercase base-36 digits, which is the grammar the model
/// pins for an allow list and a classification scope.
///
/// Base 36 rather than hex because hex would fill only 16 of the grammar's 36
/// digits. `seed` comes from
/// [`ComAmazonawsMacie2Macie2State::next_id_seed`], and both [`spread`] and
/// [`base36`] are injective in it, so no two identifiers of one store can carry
/// the same id.
pub fn new_resource_id(seed: u64, now: SystemTime) -> String {
    base36(spread(seed, now), RESOURCE_ID_LEN)
}

/// The bits of [`mix64`] of a seed that a UUID's low half has room for: the top
/// two bits of that half are where the variant lives, so the seed's other two
/// bits go beside the version instead.
const UUID_LOW_HALF_BITS: u32 = 62;

/// How far the top byte of [`mix64`] of a seed is shifted down to leave exactly
/// the bits [`UUID_LOW_HALF_BITS`] had no room for. Shifting a byte by it is
/// TOTAL — the result cannot exceed a nibble — so none of those bits can be
/// lost on the way into the layout below.
const UUID_HIGH_BITS_SHIFT: u32 = 8 - (64 - UUID_LOW_HALF_BITS);

/// The id of a custom data identifier for `seed`: a canonical UUID — 36
/// characters, lowercase hex in 8-4-4-4-12 groups — which is the form Macie
/// issues one of these ids in. The text comes from
/// [`::service_common::uuid_from_random_bytes`], this tree's one rendering of 16
/// service-produced bytes as a UUID, so neither the grouping nor the version
/// and variant bits are spelled out here.
///
/// INJECTIVE in `seed`, the property [`new_resource_id`] rests on too: all 64
/// bits of [`mix64`] of the seed land in positions that rendering leaves free
/// ([`UUID_LOW_HALF_BITS`] of them in the low half, the remaining two in the
/// nibble beside the version), so no two identifiers of one store can carry the
/// same id however the clock behaves. The rest of the bytes carry the instant it
/// was minted at, so the ids of one store do not read as a counter.
pub fn new_custom_data_identifier_id(seed: u64, now: SystemTime) -> String {
    let unique = mix64(seed);
    let instant = mix64(minted_at_bits(now) ^ unique).to_be_bytes();

    let mut bytes = [0_u8; 16];
    // The instant-derived half, except for the two positions a UUID reserves.
    bytes[..6].copy_from_slice(&instant[..6]);
    bytes[7] = instant[6];
    // The bits of `unique` the low half has no room for, in the low nibble of
    // the byte whose high nibble carries the version.
    bytes[6] = unique.to_be_bytes()[0] >> UUID_HIGH_BITS_SHIFT;
    // The rest of `unique`; the bits this leaves clear at the top are the ones
    // the variant occupies.
    let low = unique & ((1 << UUID_LOW_HALF_BITS) - 1);
    bytes[8..].copy_from_slice(&low.to_be_bytes());

    ::service_common::uuid_from_random_bytes(bytes)
}

/// The low 64 bits of the instant `now`, in nanoseconds since the epoch — what
/// a minted id carries so that the ids of one store do not read as a counter.
fn minted_at_bits(now: SystemTime) -> u64 {
    let nanos = now
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    // The mask keeps this total rather than relying on a lossy cast.
    u64::try_from(nanos & u128::from(u64::MAX)).unwrap_or_default()
}

/// 128 bits for one identifier: `seed` spread over the whole range, with the
/// instant it was minted at in the high half so ids of one store do not read as
/// a counter.
///
/// INJECTIVE in `seed`, which is what carries uniqueness: the low half is
/// [`mix64`] of the seed and `mix64` is a bijection on `u64`, so distinct seeds
/// give distinct ids whatever the clock does.
fn spread(seed: u64, now: SystemTime) -> u128 {
    // The low 64 bits of the instant are enough to spread the high half.
    let minted_at = minted_at_bits(now);
    let high = mix64(minted_at ^ mix64(seed));
    (u128::from(high) << 64) | u128::from(mix64(seed))
}

/// SplitMix64's finalizer: a BIJECTION on `u64` built from an add, two
/// xor-shift-multiplies by odd constants and a final xor-shift, each of which
/// is itself invertible. Used only to spread a counter over the whole range —
/// never as a hash of caller data, and never where a cryptographic digest is
/// needed.
fn mix64(value: u64) -> u64 {
    let mut z = value.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// `value` as exactly `width` lowercase base-36 digits, zero-padded.
fn base36(mut value: u128, width: usize) -> String {
    const DIGITS: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let mut digits = vec![b'0'; width];
    for slot in digits.iter_mut().rev() {
        *slot = DIGITS[(value % 36) as usize];
        value /= 36;
    }
    String::from_utf8(digits).expect("base-36 digits are ASCII")
}

/// A `SystemTime` as the modelled `__timestampIso8601` type.
pub fn to_date_time(time: SystemTime) -> ::aws_smithy_types::DateTime {
    let since_epoch = time
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    ::aws_smithy_types::DateTime::from_secs_and_nanos(
        i64::try_from(since_epoch.as_secs()).unwrap_or(i64::MAX),
        since_epoch.subsec_nanos(),
    )
}

/// Where `severity` sits in [`SEVERITIES_ASCENDING`], or `None` when it is not
/// one of the severities a `SeverityLevel` may assign.
pub fn severity_rank(severity: &str) -> Option<usize> {
    SEVERITIES_ASCENDING
        .iter()
        .position(|known| *known == severity)
}

/// The severity configuration the service applies to a custom data identifier
/// whose create named no `severityLevels`: one level, assigning
/// [`DEFAULT_SEVERITY`] from [`DEFAULT_SEVERITY_OCCURRENCES_THRESHOLD`]
/// occurrence onwards.
///
/// Stored with the identifier, the same way the documented
/// [`DEFAULT_MAXIMUM_MATCH_DISTANCE`] is, so every later read reports the
/// configuration that is actually in effect.
pub fn default_severity_levels() -> Vec<SeverityLevel> {
    vec![SeverityLevel {
        severity: DEFAULT_SEVERITY.to_string(),
        occurrences_threshold: DEFAULT_SEVERITY_OCCURRENCES_THRESHOLD,
    }]
}

#[cfg(test)]
mod tests {
    use super::{
        base36, custom_data_identifier_arn, default_severity_levels, is_region_shaped,
        new_custom_data_identifier_id, new_resource_id, service_role_arn, severity_rank, spread,
        DEFAULT_SEVERITY, DEFAULT_SEVERITY_OCCURRENCES_THRESHOLD, MAX_SEVERITY_LEVELS,
        RESOURCE_ID_LEN, SEVERITIES_ASCENDING,
    };
    use std::time::SystemTime;

    /// Every custom-data-identifier id is a canonical UUID — the form the
    /// service issues one in — and two seeds never collide, over a span of
    /// seeds and of minting instants.
    #[test]
    fn minted_custom_data_identifier_ids_are_canonical_uuids_and_never_repeat() {
        let mut seen = std::collections::BTreeSet::new();
        let instants = [
            SystemTime::UNIX_EPOCH,
            SystemTime::UNIX_EPOCH + std::time::Duration::from_nanos(1),
            SystemTime::now(),
        ];
        for now in instants {
            for seed in 1..=256_u64 {
                let id = new_custom_data_identifier_id(seed, now);
                let case = format!("seed {seed} at {now:?}");
                assert_eq!(
                    id.chars().count(),
                    36,
                    "case {case}: a canonical UUID is 36 characters, got {id:?}"
                );
                assert_eq!(
                    id.match_indices('-').map(|(at, _)| at).collect::<Vec<_>>(),
                    vec![8, 13, 18, 23],
                    "case {case}: a canonical UUID groups 8-4-4-4-12, got {id:?}"
                );
                assert!(
                    id.chars()
                        .all(|c| c == '-' || c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
                    "case {case}: a canonical UUID is lowercase hex, got {id:?}"
                );
                assert!(
                    seen.insert(id.clone()),
                    "case {case}: {id:?} was already minted for another seed"
                );
            }
        }
    }

    /// The severity configuration a create naming no `severityLevels` gets: one
    /// level, carrying the values the member doc states — "at least one
    /// occurrence" at the MEDIUM severity — and inside every documented bound
    /// on the member.
    #[test]
    fn the_default_severity_level_is_the_documented_one() {
        let levels = default_severity_levels();
        assert_eq!(
            levels.len(),
            1,
            "the default is a single level, got {levels:?}"
        );
        assert!(
            levels.len() <= MAX_SEVERITY_LEVELS,
            "the default must fit the documented bound of {MAX_SEVERITY_LEVELS}, got {levels:?}"
        );
        let level = &levels[0];
        assert_eq!(
            level.severity, "MEDIUM",
            "the default level assigns the MEDIUM severity, got {level:?}"
        );
        assert_eq!(
            level.occurrences_threshold, 1,
            "the default level produces a finding from one occurrence, got {level:?}"
        );
        assert_eq!(
            level.severity, DEFAULT_SEVERITY,
            "the level and the constant naming its severity must agree, got {level:?}"
        );
        assert_eq!(
            level.occurrences_threshold, DEFAULT_SEVERITY_OCCURRENCES_THRESHOLD,
            "the level and the constant naming its threshold must agree, got {level:?}"
        );
        assert!(
            severity_rank(&level.severity).is_some(),
            "the default severity must be one the service orders, got {level:?}"
        );
    }

    /// Every minted id has the modelled width and alphabet, and two seeds never
    /// collide — the property the id grammar and uniqueness both rest on.
    #[test]
    fn minted_ids_fit_the_modelled_grammar_and_never_repeat() {
        let now = SystemTime::now();
        let mut seen = std::collections::BTreeSet::new();
        for seed in 1..=256_u64 {
            let id = new_resource_id(seed, now);
            assert_eq!(
                id.chars().count(),
                RESOURCE_ID_LEN,
                "case seed {seed}: a Macie id is {RESOURCE_ID_LEN} characters, got {id:?}"
            );
            assert!(
                id.chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit()),
                "case seed {seed}: a Macie id is lowercase alphanumeric, got {id:?}"
            );
            assert!(
                seen.insert(id.clone()),
                "case seed {seed}: {id:?} was already minted for another seed"
            );
        }
    }

    /// `spread` is injective in the seed, which is what the uniqueness above
    /// actually rests on.
    #[test]
    fn spread_is_injective_in_the_seed() {
        let now = SystemTime::now();
        for seed in 1..=64_u64 {
            assert_ne!(
                spread(seed, now),
                spread(seed + 1, now),
                "case seed {seed}: two seeds must not spread to the same bits"
            );
        }
    }

    /// Padding is total: a value needing fewer digits than the width is
    /// zero-filled, and the width is never exceeded.
    #[test]
    fn base36_pads_to_the_requested_width() {
        for (case, value, expected) in [
            ("zero", 0_u128, "0000"),
            ("one digit", 35, "000z"),
            ("two digits", 36, "0010"),
        ] {
            let rendered = base36(value, 4);
            assert_eq!(
                rendered, expected,
                "case {case:?}: expected {expected:?}, got {rendered:?}"
            );
        }
    }

    /// Only an AWS-region-shaped scope reaches a minted ARN.
    #[test]
    fn region_shape_accepts_regions_and_refuses_anything_else() {
        for case in ["us-east-1", "eu-west-3", "ap-southeast-4", "us-gov-west-1"] {
            assert!(
                is_region_shaped(case),
                "case {case:?}: a published region must be accepted"
            );
        }
        for case in ["", "US-EAST-1", "us_east_1", "1-east", "us--east-1", "us-east-"] {
            assert!(
                !is_region_shaped(case),
                "case {case:?}: a value that is not region-shaped must be refused"
            );
        }
    }

    /// The ARNs this service mints carry the canonical emulator account and the
    /// region the request addressed.
    #[test]
    fn minted_arns_carry_the_region_and_the_emulator_account() {
        for case in ["us-east-1", "eu-central-1"] {
            let arn = custom_data_identifier_arn(case, "abcdefghij0123456789ab");
            let expected = format!(
                "arn:aws:macie2:{case}:{}:custom-data-identifier/abcdefghij0123456789ab",
                ::engine::emulator_account_id()
            );
            assert_eq!(
                arn, expected,
                "case {case:?}: expected {expected:?}, got {arn:?}"
            );
        }
        let role = service_role_arn();
        assert!(
            role.starts_with(&format!("arn:aws:iam::{}:role/", ::engine::emulator_account_id())),
            "the service role must be an IAM role ARN of the emulator account, got {role:?}"
        );
    }

    /// The severity order the `severityLevels` rule is stated in.
    #[test]
    fn severity_rank_follows_the_documented_order() {
        for (index, severity) in SEVERITIES_ASCENDING.iter().enumerate() {
            assert_eq!(
                severity_rank(severity),
                Some(index),
                "case {severity:?}: expected rank {index}, got {:?}",
                severity_rank(severity)
            );
        }
        assert_eq!(
            severity_rank("CRITICAL"),
            None,
            "a severity a SeverityLevel cannot assign has no rank"
        );
    }
}
