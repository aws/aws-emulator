// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-pcaconnectorad-pcaconnectorad service.
//!
//! The persisted records every handler in `src/ops/` reads and writes, plus the
//! small helpers several of them share (ARN minting, the request's region, the
//! clock-due settle of a resource still in a transient status, pagination tokens).
//! Keeping them here means two operations never grow two copies of one resource.
//!
//! The store is keyed on ARN alone and is therefore region-agnostic: a connector
//! minted under one Region's credential scope is readable from a request signed
//! for another. The ARN a resource carries still records the Region it was
//! created in, which is what the wire shapes report.

use std::collections::{HashMap, HashSet, VecDeque};
use std::time::{Duration, SystemTime};

use crate::ops::types::{TemplateDefinition, TemplateRevision, VpcInformation};

/// This service's ARN namespace and SigV4 signing name (`OPERATIONS.md`, "Wire
/// protocol"). Every ARN this service mints carries it.
pub const ARN_NAMESPACE: &str = "pca-connector-ad";

/// The region an ARN carries when the request is unsigned, so no SigV4 credential
/// scope names one. Emulators default to `us-east-1`
/// (`emulator_server::emulator_arn`'s doc).
pub const DEFAULT_REGION: &str = "us-east-1";

/// `ConnectorStatus` values, spelled as `types::CONNECTOR_STATUS_VALUES` holds them.
pub const CONNECTOR_STATUS_CREATING: &str = "CREATING";
/// See [`CONNECTOR_STATUS_CREATING`].
pub const CONNECTOR_STATUS_ACTIVE: &str = "ACTIVE";

/// `DirectoryRegistrationStatus` values, spelled as
/// `types::DIRECTORY_REGISTRATION_STATUS_VALUES` holds them.
pub const DIRECTORY_REGISTRATION_STATUS_CREATING: &str = "CREATING";
/// See [`DIRECTORY_REGISTRATION_STATUS_CREATING`].
pub const DIRECTORY_REGISTRATION_STATUS_ACTIVE: &str = "ACTIVE";
/// See [`DIRECTORY_REGISTRATION_STATUS_CREATING`]: the status a registration the
/// delete accepted carries until it stops existing.
pub const DIRECTORY_REGISTRATION_STATUS_DELETING: &str = "DELETING";
/// See [`DIRECTORY_REGISTRATION_STATUS_CREATING`]: the terminal status of a
/// registration whose directory could not be authorized.
pub const DIRECTORY_REGISTRATION_STATUS_FAILED: &str = "FAILED";

/// The `DirectoryRegistrationStatusReason` a failed registration reports when the
/// `DirectoryId` it names is not a directory the account holds, spelled as
/// `types::DIRECTORY_REGISTRATION_STATUS_REASON_VALUES` holds it. `StatusReason` is
/// "additional information about the directory registration status if the status is
/// failed", so it is set only beside
/// [`DIRECTORY_REGISTRATION_STATUS_FAILED`].
pub const DIRECTORY_REGISTRATION_STATUS_REASON_DIRECTORY_RESOURCE_NOT_FOUND: &str =
    "DIRECTORY_RESOURCE_NOT_FOUND";

/// `TemplateStatus` values, spelled as `types::TEMPLATE_STATUS_VALUES` holds them.
/// The enum has only `ACTIVE` and `DELETING`, so a created template is `ACTIVE`.
pub const TEMPLATE_STATUS_ACTIVE: &str = "ACTIVE";

/// `ServicePrincipalNameStatus` values, spelled as
/// `types::SERVICE_PRINCIPAL_NAME_STATUS_VALUES` holds them.
pub const SERVICE_PRINCIPAL_NAME_STATUS_CREATING: &str = "CREATING";
/// See [`SERVICE_PRINCIPAL_NAME_STATUS_CREATING`].
pub const SERVICE_PRINCIPAL_NAME_STATUS_ACTIVE: &str = "ACTIVE";
/// See [`SERVICE_PRINCIPAL_NAME_STATUS_CREATING`]: the terminal status of a name
/// that could not be written into the directory at all.
pub const SERVICE_PRINCIPAL_NAME_STATUS_FAILED: &str = "FAILED";

/// The `ServicePrincipalNameStatusReason` a failed service principal name reports
/// when the directory its registration authorizes is not one the account holds,
/// spelled as `types::SERVICE_PRINCIPAL_NAME_STATUS_REASON_VALUES` holds it.
/// `StatusReason` is "additional information for the status of a service principal
/// name if the status is failed", so it is set only beside
/// [`SERVICE_PRINCIPAL_NAME_STATUS_FAILED`].
pub const SERVICE_PRINCIPAL_NAME_STATUS_REASON_DIRECTORY_RESOURCE_NOT_FOUND: &str =
    "DIRECTORY_RESOURCE_NOT_FOUND";

/// How long after `CreateConnector` the connector settles from `CREATING` to
/// `ACTIVE`.
///
/// A CHOSEN span, not a documented one: AWS publishes no duration for connector
/// activation. It sits at the top of the 250-750 ms window the emulator uses for a
/// clock-owed transition — the widest the window allows, so a caller (or a test)
/// that reads the connector straight after creating it reliably observes
/// `CREATING` — and it is never zero.
pub const CONNECTOR_ACTIVATION_DELAY: Duration = Duration::from_millis(750);

/// How long after `CreateDirectoryRegistration` the registration settles out of
/// `CREATING` into the terminal status the authorization of its directory reaches
/// ([`directory_registration_outcome`]).
///
/// A CHOSEN span, not a documented one: AWS publishes no duration for directory
/// registration. It is a few seconds and never zero, so a caller (or a test) that
/// reads the registration straight after creating it reliably observes the
/// `CREATING` the 202 reported.
pub const DIRECTORY_REGISTRATION_ACTIVATION_DELAY: Duration = Duration::from_secs(3);

/// How long after `DeleteDirectoryRegistration` the registration stops existing.
///
/// A CHOSEN span, not a documented one: AWS publishes no duration for
/// deauthorizing a directory. It matches
/// [`DIRECTORY_REGISTRATION_ACTIVATION_DELAY`] and is never zero, so a caller that
/// reads the registration straight after deleting it observes the `DELETING` the
/// 202 accepted before the resource is gone.
pub const DIRECTORY_REGISTRATION_DELETION_DELAY: Duration = Duration::from_secs(3);

/// How long after `CreateServicePrincipalName` the name settles out of `CREATING`
/// into the terminal status writing it into the directory reaches
/// ([`service_principal_name_outcome`]).
///
/// A CHOSEN span, not a documented one: AWS publishes no duration for creating a
/// service principal name. It matches
/// [`DIRECTORY_REGISTRATION_ACTIVATION_DELAY`] and is never zero, so a caller (or a
/// test) that reads the name straight after the 202 reliably observes `CREATING`.
pub const SERVICE_PRINCIPAL_NAME_ACTIVATION_DELAY: Duration = Duration::from_secs(3);

/// `MaxResults` for a list call that supplies none: the modelled maximum of the
/// `MaxResults` member (`range 1..1000`), so an un-paged call returns everything.
pub const MAX_LIST_RESULTS: i32 = 1000;

/// The most connectors this emulator stores AT ALL, and the most templates it
/// stores under one connector. Past either, the create is refused with the
/// modelled `ServiceQuotaExceededException` instead of growing the store, so no
/// number of requests can grow the process heap without limit.
///
/// [`MAX_CONNECTORS`] is a bound on this STORE as a whole, not a per-account or
/// per-Region quota: the store is keyed on ARN alone (see the module header), so
/// the count it compares against spans every credential scope. The quota code and
/// refusal message say so rather than naming a Region the count does not isolate.
///
/// Both are the EMULATOR's store bounds, not quotas quoted from AWS: the service
/// publishes adjustable quotas, and these caps sit well above the default ones so
/// no faithful workflow reaches them. Only the refusal itself — a 402
/// `ServiceQuotaExceededException` once a quota is exhausted — is the service's
/// documented behaviour.
pub const MAX_CONNECTORS: usize = 100;
/// See [`MAX_CONNECTORS`].
pub const MAX_TEMPLATES_PER_CONNECTOR: usize = 200;
/// See [`MAX_CONNECTORS`]: the same kind of store bound, for the directory
/// registrations this emulator holds AT ALL. A directory id is caller-chosen, so
/// without it an unlimited number of distinct ids could grow the store without
/// limit.
pub const MAX_DIRECTORY_REGISTRATIONS: usize = 100;

/// How many `ClientToken` -> ARN records the store keeps. An idempotency record
/// is a cache, not a resource: AWS honours a token for a bounded window and
/// forgets it afterwards, so past this many the OLDEST record is dropped rather
/// than the request refused. That bounds the map against an unlimited number of
/// distinct caller-supplied tokens.
pub const MAX_IDEMPOTENCY_RECORDS: usize = 4096;

/// The `QuotaCode` a store-bound refusal reports. The member's only documented
/// rule is "The code associated with the service quota"; AWS's real codes are
/// opaque `L-xxxxxxxx` identifiers this emulator cannot derive, so these name the
/// quota in readable form and are EMULATOR-CHOSEN, not quoted from AWS.
pub const QUOTA_CODE_CONNECTORS_PER_STORE: &str = "ConnectorsPerStore";
/// See [`QUOTA_CODE_CONNECTORS_PER_STORE`].
pub const QUOTA_CODE_TEMPLATES_PER_CONNECTOR: &str = "TemplatesPerConnector";
/// See [`QUOTA_CODE_CONNECTORS_PER_STORE`].
pub const QUOTA_CODE_DIRECTORY_REGISTRATIONS_PER_STORE: &str = "DirectoryRegistrationsPerStore";

/// The tag every pagination cursor this service issues carries, so a token the
/// service did not mint is refused instead of being honoured as a cursor. Four
/// characters, so a tagged cursor's total length stays a multiple of four (see
/// [`encode_page_token`]).
const PAGE_TOKEN_TAG: &str = "pcad";

/// How many hex characters a cursor's `seq` field occupies: a `u64` rendered
/// zero-padded, so every cursor this service issues is the same length and the
/// tag plus the field is a multiple of four characters.
const PAGE_TOKEN_SEQ_WIDTH: usize = 16;

/// The Microsoft Active Directory certificate-template OID arc that every AD
/// template's object identifier sits under.
const AD_TEMPLATE_OID_ARC: &str = "1.3.6.1.4.1.311.21.8";

/// How this service words the refusal of a request naming a resource it does not
/// hold: the resource's type, then the identifier the request supplied, then a full
/// stop — `Connector was not found: <arn>.`
///
/// ONE wording for every resource type and every operation, because the sentence is
/// the service's and not any one handler's: a handler that spelled it itself is how
/// `GetConnector` and `GetDirectoryRegistration` came to answer the same failure in
/// two different voices. The identifier is quoted exactly as the request carried it,
/// so the caller reads back the value it sent rather than a normalised form of it.
#[must_use]
pub fn not_found_message(resource_type: &str, identifier: &str) -> String {
    format!("{resource_type} was not found: {identifier}.")
}

/// The `ResourceType` a refusal names for each resource this service holds, spelled
/// as the modelled shape names it.
pub const RESOURCE_TYPE_CONNECTOR: &str = "Connector";
/// See [`RESOURCE_TYPE_CONNECTOR`].
pub const RESOURCE_TYPE_TEMPLATE: &str = "Template";
/// See [`RESOURCE_TYPE_CONNECTOR`].
pub const RESOURCE_TYPE_DIRECTORY_REGISTRATION: &str = "DirectoryRegistration";
/// See [`RESOURCE_TYPE_CONNECTOR`].
pub const RESOURCE_TYPE_SERVICE_PRINCIPAL_NAME: &str = "ServicePrincipalName";

/// The expression `TemplateName` declares, as the model writes it
/// (`smithy_query(query="shape",shape="TemplateName")`): every member of this
/// service targeting that shape — a template's own `Name` and each entry of a
/// definition's `SupersededTemplates` — has to satisfy it.
///
/// It is quoted here because the boundary validator never applies it: that walk
/// compiles a `@pattern` with the `regex` crate and treats one it cannot compile as
/// satisfied, and this expression is written with negative look-ahead, which that
/// crate has no syntax for. [`template_name_report`] is that gap closed.
pub const TEMPLATE_NAME_PATTERN: &str = r"^(?!^\s+$)((?![\x5c'\x2b,;<=>#\x22])([\x20-\x7E]))+$";

/// The characters the expression's inner negative look-ahead excludes, spelled as
/// the model spells them (`[\x5c'\x2b,;<=>#\x22]`): backslash, apostrophe, plus,
/// comma, semicolon, less-than, equals, greater-than, hash and double quote.
const TEMPLATE_NAME_EXCLUDED: &[char] = &['\\', '\'', '+', ',', ';', '<', '=', '>', '#', '"'];

/// Whether `value` satisfies [`TEMPLATE_NAME_PATTERN`].
///
/// The expression is anchored at both ends, so a Smithy find of it is a whole-value
/// match, and what it admits is a sequence of ONE OR MORE characters, each inside
/// `[\x20-\x7E]` and none of them one the inner look-ahead excludes
/// ([`TEMPLATE_NAME_EXCLUDED`]), that is not whitespace from end to end — what the
/// look-ahead the expression opens with (`(?!^\s+$)`) says, and inside that printable
/// range the only whitespace character is the space.
///
/// Those classes are SCANNED rather than handed to a regex engine: a left-to-right
/// pass whose cost is linear in the value's length whatever the value holds, which is
/// the property the framework's own matching of every expression it CAN compile has,
/// and which a backtracking engine (the only kind that compiles a look-ahead) would
/// give up on an untrusted-input path.
#[must_use]
pub fn is_template_name(value: &str) -> bool {
    if value.is_empty() {
        return false;
    }
    if value.chars().all(|character| character == ' ') {
        return false;
    }
    value.chars().all(|character| {
        matches!(character, '\x20'..='\x7E') && !TEMPLATE_NAME_EXCLUDED.contains(&character)
    })
}

/// The message this service refuses a request whose `TemplateName` values are
/// outside [`TEMPLATE_NAME_PATTERN`] with, or `None` when every one of them is
/// inside it.
///
/// The wording is the one the validating layer in front of this service states for a
/// value its schema's regular expression does not admit: a bracketed list holding one
/// finding per offending value, each naming the expression the model declares and the
/// string the request submitted, and nothing else — no member path and no error count.
/// Both sentences are the shared ones
/// (`::service_common::validation_message`), so they cannot drift from the other
/// services that state the same findings.
///
/// `values` are the request's `TemplateName` values in the order the input declares
/// them, so the findings are listed in that order.
#[must_use]
pub fn template_name_report<'a>(values: impl IntoIterator<Item = &'a str>) -> Option<String> {
    let findings: Vec<String> = values
        .into_iter()
        .filter(|value| !is_template_name(value))
        .map(|value| {
            ::service_common::validation_message::pattern_mismatch(TEMPLATE_NAME_PATTERN, value)
        })
        .collect();
    (!findings.is_empty())
        .then(|| ::service_common::validation_message::bracketed_findings(&findings))
}

/// The `SupersededTemplates` entries a template definition carries, whichever
/// version of the definition union it is — each one a `TemplateName` and so subject
/// to [`TEMPLATE_NAME_PATTERN`]. An empty slice for a definition that supersedes
/// nothing.
#[must_use]
pub fn superseded_templates(definition: &TemplateDefinition) -> &[String] {
    match definition {
        TemplateDefinition::TemplateV2(template) => template.superseded_templates.as_deref(),
        TemplateDefinition::TemplateV3(template) => template.superseded_templates.as_deref(),
        TemplateDefinition::TemplateV4(template) => template.superseded_templates.as_deref(),
    }
    .unwrap_or_default()
}


/// One persisted connector. Mirrors the modelled `Connector`/`ConnectorSummary`
/// members, plus the bookkeeping the wire shapes do not carry (`seq` for stable
/// list ordering, `active_at` for the clock-due settle, `tags`).
#[derive(Debug, Clone)]
pub struct ConnectorRecord {
    pub arn: String,
    pub certificate_authority_arn: String,
    pub directory_id: String,
    pub vpc_information: VpcInformation,
    pub certificate_enrollment_policy_server_endpoint: String,
    pub status: String,
    pub status_reason: Option<String>,
    pub created_at: SystemTime,
    pub updated_at: SystemTime,
    /// The instant the clock owes this connector its `CREATING` -> `ACTIVE` move.
    pub active_at: SystemTime,
    pub tags: HashMap<String, String>,
    /// Monotonic creation sequence: the stable list order and paging cursor.
    pub seq: u64,
}

/// One persisted directory registration. Mirrors the modelled
/// `DirectoryRegistration`/`DirectoryRegistrationSummary` members, plus the
/// bookkeeping the wire shapes do not carry (`seq` for stable list ordering, the
/// clock-due settle of its transient statuses, `tags`).
#[derive(Debug, Clone)]
pub struct DirectoryRegistrationRecord {
    pub arn: String,
    pub directory_id: String,
    pub status: String,
    pub status_reason: Option<String>,
    pub created_at: SystemTime,
    pub updated_at: SystemTime,
    /// The instant the clock owes this registration the move out of `CREATING`
    /// into [`Self::terminal_status`].
    pub settles_at: SystemTime,
    /// What the authorization of this registration's directory reaches: `ACTIVE`
    /// when the account holds the directory the create named, `FAILED` when it
    /// does not ([`directory_registration_outcome`] is what decides it, once, on
    /// the create path).
    pub terminal_status: String,
    /// The `StatusReason` that terminal status carries, set only for a failure.
    pub terminal_status_reason: Option<String>,
    /// The instant the clock owes this registration its REMOVAL: set by
    /// `DeleteDirectoryRegistration`, which moves the record to `DELETING` and
    /// leaves it to the clock to stop existing. `None` for a registration nobody
    /// has deleted.
    pub deleted_at: Option<SystemTime>,
    pub tags: HashMap<String, String>,
    /// Monotonic creation sequence: the stable list order and paging cursor.
    pub seq: u64,
}

/// One persisted template, a child of exactly one connector.
#[derive(Debug, Clone)]
pub struct TemplateRecord {
    pub arn: String,
    pub connector_arn: String,
    pub name: String,
    pub definition: TemplateDefinition,
    pub object_identifier: String,
    pub policy_schema: i32,
    pub status: String,
    pub revision: TemplateRevision,
    pub created_at: SystemTime,
    pub updated_at: SystemTime,
    pub tags: HashMap<String, String>,
    /// Monotonic creation sequence: the stable list order and paging cursor.
    pub seq: u64,
}

/// One persisted service principal name: the name the connector authenticates to
/// Active Directory with, created under exactly one directory registration for
/// exactly one connector. Mirrors the modelled
/// `ServicePrincipalName`/`ServicePrincipalNameSummary` members, plus the
/// bookkeeping the wire shapes do not carry (the clock-due settle of its transient
/// status, `seq` for stable list ordering).
#[derive(Debug, Clone)]
pub struct ServicePrincipalNameRecord {
    pub directory_registration_arn: String,
    pub connector_arn: String,
    pub status: String,
    pub status_reason: Option<String>,
    pub created_at: SystemTime,
    pub updated_at: SystemTime,
    /// The instant the clock owes this name the move out of `CREATING` into
    /// [`Self::terminal_status`].
    pub settles_at: SystemTime,
    /// What writing this name into the directory reaches: `ACTIVE` when the
    /// registration's own authorization succeeded, `FAILED` when there was no
    /// directory to write it into ([`service_principal_name_outcome`] decides it,
    /// once, on the create path).
    pub terminal_status: String,
    /// The `StatusReason` that terminal status carries, set only for a failure.
    pub terminal_status_reason: Option<String>,
    /// Monotonic creation sequence: the stable list order and paging cursor.
    pub seq: u64,
}

/// Per-service state for com-amazonaws-pcaconnectorad-pcaconnectorad. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsPcaconnectoradPcaconnectoradState {
    /// Connectors by their own ARN.
    pub connectors: HashMap<String, ConnectorRecord>,
    /// Templates by their own ARN; `TemplateRecord::connector_arn` is the parent link.
    pub templates: HashMap<String, TemplateRecord>,
    /// Directory registrations by their own ARN. A directory is registered at most
    /// once (see [`Self::registration_by_directory_id`]).
    pub directory_registrations: HashMap<String, DirectoryRegistrationRecord>,
    /// Service principal names by the pair that identifies one: the directory
    /// registration it was created under and the connector it names. A name has no
    /// ARN of its own — the modelled operations address it by that pair — so the
    /// pair IS the key.
    ///
    /// Bounded by the two stores it is a product of: a name is only recorded for a
    /// registration and a connector this store already holds, which are capped at
    /// [`MAX_DIRECTORY_REGISTRATIONS`] and [`MAX_CONNECTORS`], so no number of
    /// requests can grow this map past their product.
    pub service_principal_names: HashMap<(String, String), ServicePrincipalNameRecord>,
    /// `<operation>:<ClientToken>` -> the ARN the first call with that token minted,
    /// so a retried create returns the original resource instead of a second one.
    /// Bounded at [`MAX_IDEMPOTENCY_RECORDS`]; write it through
    /// [`Self::record_idempotency`], never directly.
    pub idempotency: HashMap<String, String>,
    /// The keys of `idempotency` in the order they were recorded, so the oldest
    /// record is the one dropped when the map reaches its bound.
    idempotency_order: VecDeque<String>,
    /// Source of [`ConnectorRecord::seq`] / [`TemplateRecord::seq`].
    next_seq: u64,
}

impl ComAmazonawsPcaconnectoradPcaconnectoradState {
    /// The next creation sequence number. Monotonic for the life of the store, so a
    /// record's `seq` orders it against every other record ever created here.
    pub fn next_seq(&mut self) -> u64 {
        self.next_seq += 1;
        self.next_seq
    }

    /// Materialise every move of this service's resources the clock already owes at
    /// `now`: a connector's, a registration's and a service principal name's move out
    /// of `CREATING`, and the REMOVAL of a registration whose delete has come due.
    ///
    /// A reporting operation calls this before it reads: each move was due at its
    /// own recorded instant whether or not anybody called, so applying it is not a
    /// mutation caused by the read. Nothing here decides an OUTCOME — a
    /// registration's and a name's terminal status were decided on their create path
    /// and are only applied here — so two reads a moment apart cannot settle one
    /// resource two ways.
    pub fn settle(&mut self, now: SystemTime) {
        for connector in self.connectors.values_mut() {
            if connector.status == CONNECTOR_STATUS_CREATING && now >= connector.active_at {
                connector.status = CONNECTOR_STATUS_ACTIVE.to_string();
            }
        }
        // A registration the clock owes its removal STOPS EXISTING: the delete
        // answered 202 and moved it to `DELETING`, and past that instant nothing
        // reports it any more.
        let gone: Vec<String> = self
            .directory_registrations
            .values()
            .filter(|registration| {
                registration
                    .deleted_at
                    .is_some_and(|due_at| now >= due_at)
            })
            .map(|registration| registration.arn.clone())
            .collect();
        for arn in &gone {
            self.remove_directory_registration(arn);
        }
        for registration in self.directory_registrations.values_mut() {
            if registration.status == DIRECTORY_REGISTRATION_STATUS_CREATING
                && now >= registration.settles_at
            {
                registration.status = registration.terminal_status.clone();
                registration.status_reason = registration.terminal_status_reason.clone();
                // The resource changed when the move was DUE, not when it was read.
                registration.updated_at = registration.settles_at;
            }
        }
        for name in self.service_principal_names.values_mut() {
            if name.status == SERVICE_PRINCIPAL_NAME_STATUS_CREATING && now >= name.settles_at {
                name.status = name.terminal_status.clone();
                name.status_reason = name.terminal_status_reason.clone();
                // The resource changed when the move was DUE, not when it was read.
                name.updated_at = name.settles_at;
            }
        }
    }

    /// Every connector whose `seq` is past `after`, in creation order.
    pub fn connectors_after(&self, after: u64) -> Vec<&ConnectorRecord> {
        let mut found: Vec<&ConnectorRecord> = self
            .connectors
            .values()
            .filter(|connector| connector.seq > after)
            .collect();
        found.sort_by_key(|connector| connector.seq);
        found
    }

    /// Every directory registration whose `seq` is past `after`, in creation order.
    pub fn directory_registrations_after(&self, after: u64) -> Vec<&DirectoryRegistrationRecord> {
        let mut found: Vec<&DirectoryRegistrationRecord> = self
            .directory_registrations
            .values()
            .filter(|registration| registration.seq > after)
            .collect();
        found.sort_by_key(|registration| registration.seq);
        found
    }

    /// The registration of `directory_id`, if the directory is registered.
    ///
    /// A directory carries at most one registration: the modelled
    /// `DirectoryRegistrationArn` names the directory itself rather than a minted
    /// id, so a second registration of one directory has no distinct identity.
    /// The lookup is on the directory id and not on an ARN because this store is
    /// region-agnostic (see the module header).
    pub fn registration_by_directory_id(
        &self,
        directory_id: &str,
    ) -> Option<&DirectoryRegistrationRecord> {
        self.directory_registrations
            .values()
            .find(|registration| registration.directory_id == directory_id)
    }

    /// The service principal name created under `registration_arn` for
    /// `connector_arn`, if there is one. The pair identifies at most one name, and is
    /// the map's own key, so this is the lookup that key exists for.
    pub fn service_principal_name(
        &self,
        registration_arn: &str,
        connector_arn: &str,
    ) -> Option<&ServicePrincipalNameRecord> {
        self.service_principal_names
            .get(&(registration_arn.to_string(), connector_arn.to_string()))
    }

    /// The template called `name` on `connector_arn`, if there is one. Template
    /// names are unique per connector.
    pub fn template_by_name(&self, connector_arn: &str, name: &str) -> Option<&TemplateRecord> {
        self.templates
            .values()
            .find(|template| template.connector_arn == connector_arn && template.name == name)
    }

    /// How many templates `connector_arn` already carries, against
    /// [`MAX_TEMPLATES_PER_CONNECTOR`].
    pub fn template_count(&self, connector_arn: &str) -> usize {
        self.templates
            .values()
            .filter(|template| template.connector_arn == connector_arn)
            .count()
    }

    /// Record that `key` (`<operation>:<ClientToken>`) minted `arn`, dropping the
    /// oldest records once the store holds [`MAX_IDEMPOTENCY_RECORDS`] of them.
    /// Forgetting an old token is the honest outcome: a token the service no
    /// longer remembers mints a new resource, which is what AWS does once a
    /// token's window has passed.
    ///
    /// The key's scope is the caller's to choose, and it carries NO Region or
    /// credential scope: this store is region-agnostic (see the module header),
    /// so one `ClientToken` string reused for the same operation names the same
    /// resource whichever Region's scope the retry is signed for — which is the
    /// point of an idempotency token. A key whose resource has a parent (a
    /// template's connector) includes that parent, so a token reused against a
    /// different parent cannot answer with an ARN the caller did not address.
    pub fn record_idempotency(&mut self, key: String, arn: String) {
        if self.idempotency.insert(key.clone(), arn).is_none() {
            self.idempotency_order.push_back(key);
        }
        while self.idempotency_order.len() > MAX_IDEMPOTENCY_RECORDS {
            if let Some(oldest) = self.idempotency_order.pop_front() {
                self.idempotency.remove(&oldest);
            }
        }
    }

    /// Remove the directory registration `arn`, dropping any idempotency token that
    /// pointed at it (so a later create reusing that token mints a new registration
    /// rather than naming one that no longer exists).
    ///
    /// Returns whether a registration was actually present. The REMOVAL is the
    /// deauthorization completing: a deleted registration stops existing, so every
    /// reporting operation refuses the ARN afterwards.
    pub fn remove_directory_registration(&mut self, arn: &str) -> bool {
        let removed = self.directory_registrations.remove(arn).is_some();
        // A service principal name exists under its registration: once the
        // registration stops existing there is nothing left holding the name, so
        // nothing reports it any more either.
        self.service_principal_names
            .retain(|_, name| name.directory_registration_arn != arn);
        let idempotency = &mut self.idempotency;
        idempotency.retain(|_, recorded| recorded != arn);
        self.idempotency_order
            .retain(|key| idempotency.contains_key(key));
        removed
    }

    /// Remove `connector_arn` and cascade-remove its templates, dropping any
    /// idempotency token that pointed at a now-gone ARN (so a later create reusing
    /// that token mints a new resource rather than naming a deleted one).
    ///
    /// Returns whether a connector was actually present.
    pub fn remove_connector(&mut self, connector_arn: &str) -> bool {
        let removed = self.connectors.remove(connector_arn).is_some();
        // A service principal name names its connector: a connector that stops
        // existing leaves no name of itself behind.
        self.service_principal_names
            .retain(|_, name| name.connector_arn != connector_arn);
        let orphaned: HashSet<String> = self
            .templates
            .values()
            .filter(|template| template.connector_arn == connector_arn)
            .map(|template| template.arn.clone())
            .collect();
        for arn in &orphaned {
            self.templates.remove(arn);
        }
        let idempotency = &mut self.idempotency;
        idempotency.retain(|_, arn| arn != connector_arn && !orphaned.contains(arn));
        self.idempotency_order
            .retain(|key| idempotency.contains_key(key));
        removed
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsPcaconnectoradPcaconnectoradState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsPcaconnectoradPcaconnectoradState>)
}

/// The region this request was signed for, or [`DEFAULT_REGION`] when it carries no
/// SigV4 credential scope.
pub fn request_region(ctx: &smithy_server_runtime::OperationContext<'_>) -> String {
    ::emulator_server::sigv4_credential_scope(&ctx.parts().headers)
        .map(|scope| scope.region)
        .filter(|region| !region.is_empty())
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The SHARED Active Directory inventory in the shared engine: which directories the
/// account holds, as the Directory Service control plane that creates and deletes
/// them recorded it. Call it with a handler's `&*self.engine`.
///
/// This service creates no directory — a `DirectoryId` arriving here names one of
/// another service's resources — so whether the account holds it is knowable only
/// from that plane's own record. It is the ONE record under the shared type
/// ([`::service_common::ad_directories::AdDirectoryInventory`]) rather than a copy
/// inside [`ComAmazonawsPcaconnectoradPcaconnectoradState`]: the engine keys state
/// by TYPE, so a copy of its own here would be invisible to the plane that writes
/// it.
///
/// LOCK ORDER: this service takes its OWN store first and this inventory second,
/// which is the order every other reader of the inventory keeps.
pub fn ad_directory_inventory<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<::service_common::ad_directories::AdDirectoryInventory>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(
            ::engine::InMemoryEngine::state::<
                ::service_common::ad_directories::AdDirectoryInventory,
            >,
        )
}

/// The terminal state a registration of `directory_id` settles on — the status and
/// the `StatusReason` the clock applies at
/// [`DirectoryRegistrationRecord::settles_at`].
///
/// A directory registration AUTHORIZES this service with an Active Directory, so it
/// can only reach `ACTIVE` for a directory that is there to be authorized: the
/// account holds the directory exactly when the plane that creates directories
/// recorded it ([`ad_directory_inventory`]). For an id it holds no directory for the
/// authorization has nothing to act on, and the registration terminates in `FAILED`
/// carrying [`DIRECTORY_REGISTRATION_STATUS_REASON_DIRECTORY_RESOURCE_NOT_FOUND`].
///
/// Decided ONCE, on the create path, so the outcome a caller polls for is a function
/// of the request and of what the account held when the registration was created —
/// never of when somebody read it.
pub fn directory_registration_outcome<Eng: Send + Sync + 'static>(
    engine: &Eng,
    directory_id: &str,
) -> (String, Option<String>) {
    let held = ad_directory_inventory(engine)
        .is_some_and(|inventory| inventory.lock().holds(directory_id));
    if held {
        (DIRECTORY_REGISTRATION_STATUS_ACTIVE.to_string(), None)
    } else {
        (
            DIRECTORY_REGISTRATION_STATUS_FAILED.to_string(),
            Some(DIRECTORY_REGISTRATION_STATUS_REASON_DIRECTORY_RESOURCE_NOT_FOUND.to_string()),
        )
    }
}

/// The terminal state a service principal name settles on — the status and the
/// `StatusReason` the clock applies at [`ServicePrincipalNameRecord::settles_at`].
///
/// A service principal name is WRITTEN INTO the Active Directory its registration
/// authorizes, so it can only reach `ACTIVE` where that authorization itself
/// reaches `ACTIVE`: a registration whose directory the account does not hold has
/// no directory for the name to be created in, and the name terminates in `FAILED`
/// carrying [`SERVICE_PRINCIPAL_NAME_STATUS_REASON_DIRECTORY_RESOURCE_NOT_FOUND`] —
/// the same cause the registration itself reports.
///
/// `registration` is the record the create resolved, so the outcome is a function of
/// what the account held when the registration was created and of nothing read
/// later.
#[must_use]
pub fn service_principal_name_outcome(
    registration: &DirectoryRegistrationRecord,
) -> (String, Option<String>) {
    if registration.terminal_status == DIRECTORY_REGISTRATION_STATUS_ACTIVE {
        (SERVICE_PRINCIPAL_NAME_STATUS_ACTIVE.to_string(), None)
    } else {
        (
            SERVICE_PRINCIPAL_NAME_STATUS_FAILED.to_string(),
            Some(SERVICE_PRINCIPAL_NAME_STATUS_REASON_DIRECTORY_RESOURCE_NOT_FOUND.to_string()),
        )
    }
}

/// A fresh connector ARN of the modelled form
/// `arn:aws:pca-connector-ad:<region>:<account>:connector/<uuid>`.
pub fn connector_arn(region: &str, connector_id: &str) -> String {
    ::engine::emulator_arn(ARN_NAMESPACE, region, &format!("connector/{connector_id}"))
}

/// The ARN of the connector collection this store holds —
/// `arn:aws:pca-connector-ad:<region>:<account>:connector` — the resource a
/// connector-count refusal is about, since no connector was minted. The Region
/// segment is the one the request was signed for; the bound itself is
/// store-global, which the refusal's message and quota code state.
pub fn connector_collection_arn(region: &str) -> String {
    ::engine::emulator_arn(ARN_NAMESPACE, region, "connector")
}

/// A directory registration's ARN, of the modelled form
/// `arn:aws:pca-connector-ad:<region>:<account>:directory-registration/<directory id>`.
/// The resource segment is the directory's own id: the registration is named by
/// the directory it authorizes, not by a minted id.
pub fn directory_registration_arn(region: &str, directory_id: &str) -> String {
    ::engine::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("directory-registration/{directory_id}"),
    )
}

/// The ARN of the directory-registration collection this store holds —
/// `arn:aws:pca-connector-ad:<region>:<account>:directory-registration` — the
/// resource a registration-count refusal is about, since no registration was
/// minted. Mirrors [`connector_collection_arn`].
pub fn directory_registration_collection_arn(region: &str) -> String {
    ::engine::emulator_arn(ARN_NAMESPACE, region, "directory-registration")
}

/// A template ARN: the modelled child form, the parent connector ARN plus
/// `/template/<uuid>`.
pub fn template_arn(connector_arn: &str, template_id: &str) -> String {
    format!("{connector_arn}/template/{template_id}")
}

/// The certificate-enrollment policy-server endpoint a connector reports: the
/// connector's own id as the leading host label, then this service's regional
/// endpoint host (`<service>.<region>.amazonaws.com`, the spelling every other
/// endpoint of this service uses), with the AD policy-provider CEP path.
pub fn enrollment_policy_server_endpoint(connector_id: &str, region: &str) -> String {
    format!(
        "https://{connector_id}.{ARN_NAMESPACE}.{region}.amazonaws.com/ADPolicyProvider_CEP_Kerberos/service.svc/CEP"
    )
}

/// A fresh resource id: a lowercase hyphenated v4 UUID, the form every ARN this
/// service mints embeds (`ConnectorArn` and `TemplateArn` both pattern-match a
/// UUID there).
///
/// Minted by the framework's own v4 generator, `emulator_telemetry`'s `random()`
/// id constructor ("A fresh random (v4) identifier"), which is backed by the
/// `uuid` crate and which this crate already depends on — so this service
/// declares no id generator and no RNG of its own.
pub fn new_resource_id() -> String {
    ::emulator_telemetry::SessionId::random().as_str().to_string()
}

/// A template's object identifier: the AD certificate-template OID arc plus seven
/// numeric components derived from the template's own id, so the OID is stable
/// for the resource, unique to it, and inside the modelled
/// `CustomObjectIdentifier` pattern. `template_id` is an id as
/// [`new_resource_id`] renders one.
pub fn template_object_identifier(template_id: &str) -> String {
    let digits: Vec<u8> = template_id.bytes().filter(u8::is_ascii_hexdigit).collect();
    let mut oid = String::from(AD_TEMPLATE_OID_ARC);
    for chunk in digits.as_chunks::<4>().0.iter().take(7) {
        let component = std::str::from_utf8(chunk)
            .ok()
            .and_then(|text| u16::from_str_radix(text, 16).ok())
            .unwrap_or_default();
        oid.push('.');
        oid.push_str(&component.to_string());
    }
    oid
}

/// Encode a pagination cursor: the `seq` the page just served ended at, rendered
/// as [`PAGE_TOKEN_SEQ_WIDTH`] zero-padded hex characters behind
/// [`PAGE_TOKEN_TAG`]. Hex digits and the tag are inside the modelled `NextToken`
/// character class and the cursor's length is a multiple of four, so the token
/// matches the modelled pattern
/// `^(?:[A-Za-z0-9_-]{4})*(?:[A-Za-z0-9_-]{2}==|[A-Za-z0-9_-]{3}=)?$`.
pub fn encode_page_token(after_seq: u64) -> String {
    format!("{PAGE_TOKEN_TAG}{after_seq:0width$x}", width = PAGE_TOKEN_SEQ_WIDTH)
}

/// Decode a cursor produced by [`encode_page_token`]. `None` for anything this
/// service did not mint — a value carrying no [`PAGE_TOKEN_TAG`], a cursor field
/// of the wrong width, or one that is not hex — so a forged token is refused
/// rather than honoured. The caller answers `None` with a `ValidationException`.
pub fn decode_page_token(token: &str) -> Option<u64> {
    let cursor = token.strip_prefix(PAGE_TOKEN_TAG)?;
    if cursor.len() != PAGE_TOKEN_SEQ_WIDTH {
        return None;
    }
    u64::from_str_radix(cursor, 16).ok()
}

/// The message a paginated read of this service refuses an unreadable `NextToken`
/// with: the fixed sentence `Pagination token exception in operation '<operation>':
/// invalid payload wrapping schema <n>`, where the operation is the one the token
/// was presented to and `<n>` is the wrapping schema the presented token's own
/// payload declares ([`payload_wrapping_schema`]).
///
/// Both variable parts come from the request and nothing of the token's own text
/// travels in it, so an opaque value a caller sent is not echoed back. One wording
/// for every list operation of this service, so a client matching on the text of
/// this error class does not meet two sentences for it.
pub fn pagination_token_message(operation: &str, next_token: &str) -> String {
    format!(
        "Pagination token exception in operation '{operation}': invalid payload wrapping \
         schema {}",
        payload_wrapping_schema(next_token)
    )
}

/// The wrapping schema a pagination token's own payload declares: the first 16 bits
/// of that payload, read as a big-endian SIGNED integer.
///
/// A continuation token carries a WRAPPED payload whose leading field states which
/// schema the rest of it is written under, and a decoder handed a schema it does not
/// write reports the value it read — so the number is a function of the token
/// presented and of nothing else. A token is base64 text, so only its first group of
/// [`BASE64_GROUP`] characters carries those bits: a token shorter than one group is
/// read as if padded with zero sextets ([`ZERO_SEXTET`]), and leading characters that
/// are not base64 at all carry no payload to read a schema out of, which is the zero
/// this answers.
fn payload_wrapping_schema(next_token: &str) -> i16 {
    let mut leading: String = next_token.chars().take(BASE64_GROUP).collect();
    while leading.chars().count() < BASE64_GROUP {
        leading.push(ZERO_SEXTET);
    }
    // The base64url codec this workspace already reads opaque token payloads with,
    // so this service introduces no second decoder.
    let octets = ::service_common::jwt::decode_base64url(&leading).unwrap_or_default();
    i16::from_be_bytes([
        octets.first().copied().unwrap_or_default(),
        octets.get(1).copied().unwrap_or_default(),
    ])
}

/// The number of characters one base64 group spells three octets in (RFC 4648 §4).
const BASE64_GROUP: usize = 4;

/// The base64 character for six zero bits, which a token shorter than one group is
/// read as if padded with.
const ZERO_SEXTET: char = 'A';

/// The page a list operation of this service must serve: how many records it may
/// return, and the `seq` it starts past.
///
/// `MaxResults` is modelled `range 1..1000` and the boundary validator has already
/// refused anything outside it, so a value arriving here is in range; an absent one
/// means "everything", which is that same modelled maximum. The value is clamped
/// into the modelled range all the same, so a page size reaching a caller of this
/// helper is always at least one record and never more than the modelled maximum —
/// the invariant a cursor and a page bound both rest on — whatever the validator
/// let through.
///
/// `None` for a `NextToken` [`decode_page_token`] refuses, so a token this service
/// never issued is not honoured as a cursor; the caller answers it with its own
/// operation's `ValidationException`.
pub fn page_bounds(max_results: Option<i32>, next_token: Option<&str>) -> Option<(usize, u64)> {
    let modelled_max = usize::try_from(MAX_LIST_RESULTS).unwrap_or(usize::MAX);
    let page_size = usize::try_from(max_results.unwrap_or(MAX_LIST_RESULTS))
        .unwrap_or(modelled_max)
        .clamp(1, modelled_max);
    let after = match next_token {
        None => 0,
        Some(token) => decode_page_token(token)?,
    };
    Some((page_size, after))
}
