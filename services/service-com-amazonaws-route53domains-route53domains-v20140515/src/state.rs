// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-route53domains-route53domains-v20140515 service.
//!
//! This is the registrar's record: the domains the account holds, and the asynchronous
//! operation record that every mutating Route 53 Domains call appends. A handler reaches
//! it through [`service_state`].
//!
//! What the REGISTRY knows rather than what the account holds — the names registered
//! outside this account, and the standard registration price of a TLD — lives in the
//! [`registry`] child module and is re-exported here, so a handler reaches all of it
//! through `crate::state`.
//!
//! The domain-model helpers live here too — name normalisation, the documented name
//! syntax, the public-suffix rule that decides what a registry delegates, the delegation
//! set, and the registration period — for two reasons: `src/lib.rs` is generated, so this
//! crate cannot declare another top-level module, and every operation that takes a
//! `DomainName` has to resolve it exactly the same way, which only holds if there is one
//! implementation.

mod registry;
pub use registry::{
    seeded_registrations, standard_registration_price, standard_registration_rate,
    standard_transfer_price, unavailable_domain_message, Transferability, AVAILABILITY_AVAILABLE,
    AVAILABILITY_UNAVAILABLE, CURRENCY_USD, DEFAULT_STANDARD_REGISTRATION_RATE,
    PRICE_TYPE_STANDARD, STANDARD_REGISTRATION_RATES, TRANSFERABLE_DOMAIN_IN_OWN_ACCOUNT,
    TRANSFERABLE_UNTRANSFERABLE,
};

use crate::ops::types::{ContactDetail, Nameserver};
use std::collections::{BTreeMap, BTreeSet};

// ---------------------------------------------------------------------------
// Modelled enum members this slice writes
// ---------------------------------------------------------------------------

/// `SUBMITTED`, from the model's `OperationStatus` enum
/// (`crate::ops::types::OPERATION_STATUS_VALUES`).
pub const STATUS_SUBMITTED: &str = "SUBMITTED";
/// `SUCCESSFUL`, from the model's `OperationStatus` enum.
pub const STATUS_SUCCESSFUL: &str = "SUCCESSFUL";

/// `REGISTER_DOMAIN`, from the model's `OperationType` enum
/// (`crate::ops::types::OPERATION_TYPE_VALUES`).
pub const TYPE_REGISTER_DOMAIN: &str = "REGISTER_DOMAIN";
/// `DELETE_DOMAIN`, from the model's `OperationType` enum.
pub const TYPE_DELETE_DOMAIN: &str = "DELETE_DOMAIN";
/// `ENABLE_AUTORENEW`, from the model's `OperationType` enum.
pub const TYPE_ENABLE_AUTORENEW: &str = "ENABLE_AUTORENEW";
/// `DISABLE_AUTORENEW`, from the model's `OperationType` enum.
pub const TYPE_DISABLE_AUTORENEW: &str = "DISABLE_AUTORENEW";

/// The EPP status a freshly registered domain carries: ICANN forbids transferring a
/// domain away within 60 days of registration, which Route 53 expresses as the transfer
/// lock (`DisableDomainTransferLock`'s own documentation names
/// `clientTransferProhibited` as the status the lock sets).
pub const STATUS_CLIENT_TRANSFER_PROHIBITED: &str = "clientTransferProhibited";

// ---------------------------------------------------------------------------
// The clock
// ---------------------------------------------------------------------------

/// Wall-clock milliseconds since the Unix epoch: the one clock this service reads.
pub fn now_millis() -> i64 {
    let since_epoch = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    i64::try_from(since_epoch.as_millis()).unwrap_or(i64::MAX)
}

/// How long after the submitting call this emulator owes an operation record's move out
/// of `SUBMITTED` into `SUCCESSFUL`.
///
/// Route 53 Domains documents no span for this (registration takes minutes to hours),
/// so the span is this emulator's own choice. It is short, but deliberately never zero:
/// a caller that polls `GetOperationDetail` straight after `RegisterDomain` must still
/// observe the transient `SUBMITTED` status, the way it would against the real service.
pub const OPERATION_SETTLE_MILLIS: i64 = 500;

/// The most operation records this emulator keeps, and the most domains it registers.
///
/// Both stores are driven by the caller — a record is appended by every mutating call,
/// and a domain by every registration — so each needs a ceiling of its own, or a client
/// could grow this process's memory without limit. Route 53 Domains publishes a default
/// quota of 20 domains per account and raises it on request, so no fixed number is a
/// rule; these are this emulator's stated ceilings, far above any test or demo workload.
///
/// Past [`MAX_DOMAINS`], `RegisterDomain` refuses with the modelled `DomainLimitExceeded`
/// ("The number of domains has exceeded the allowed threshold for the account"). The
/// operation log has no modelled refusal and nothing a client can delete from it, so
/// [`record_operation`](ComAmazonawsRoute53domainsRoute53domainsV20140515State::record_operation)
/// drops its oldest records instead: an id a client could still usefully poll is a recent
/// one, and keeping the log bounded also bounds the per-request work the scans over it do
/// under the shared lock.
pub const MAX_DOMAINS: usize = 200;
/// See [`MAX_DOMAINS`].
pub const MAX_OPERATION_RECORDS: usize = 1000;

// ---------------------------------------------------------------------------
// Records
// ---------------------------------------------------------------------------

/// A registered domain: what `RegisterDomain` writes and `GetDomainDetail`,
/// `ListDomains`, `CheckDomainAvailability` and the auto-renew operations read.
#[derive(Debug, Clone)]
pub struct Domain {
    /// The registered name, lower-cased (domain names are not case sensitive).
    pub name: String,
    pub auto_renew: bool,
    pub transfer_lock: bool,
    pub admin_contact: ContactDetail,
    pub registrant_contact: ContactDetail,
    pub tech_contact: ContactDetail,
    pub billing_contact: Option<ContactDetail>,
    pub admin_privacy: bool,
    pub registrant_privacy: bool,
    pub tech_privacy: bool,
    pub billing_privacy: bool,
    pub nameservers: Vec<Nameserver>,
    pub status_list: Vec<String>,
    pub registrar_name: String,
    pub creation_millis: i64,
    pub updated_millis: i64,
    pub expiration_millis: i64,
}

/// One asynchronous operation record, as `GetOperationDetail` and `ListOperations`
/// report it.
#[derive(Debug, Clone)]
pub struct OperationRecord {
    pub operation_id: String,
    pub domain_name: String,
    /// A member of the model's `OperationType` enum.
    pub op_type: &'static str,
    /// A member of the model's `OperationStatus` enum.
    pub status: &'static str,
    pub submitted_millis: i64,
    pub last_updated_millis: i64,
    /// The instant the clock owes this record's move out of `SUBMITTED`.
    settles_at_millis: i64,
}

/// Per-service state for com-amazonaws-route53domains-route53domains-v20140515. A handler reaches it through [`service_state`].
#[derive(Debug)]
pub struct ComAmazonawsRoute53domainsRoute53domainsV20140515State {
    /// The account's domains, keyed by the lower-cased domain name.
    pub domains: BTreeMap<String, Domain>,
    /// Every operation record, in submission order — the order `ListOperations` sorts.
    pub operations: Vec<OperationRecord>,
    /// The names this registry holds for somebody else — see [`seeded_registrations`].
    /// They are registered, so they are not available, but they are not this account's:
    /// no domain-scoped read or mutation resolves one.
    pub registered_elsewhere: BTreeSet<String>,
}

/// The store a fresh process starts from: no domains of its own, no operation records,
/// and the registry's standing third-party registrations.
impl Default for ComAmazonawsRoute53domainsRoute53domainsV20140515State {
    fn default() -> Self {
        Self {
            domains: BTreeMap::new(),
            operations: Vec::new(),
            registered_elsewhere: seeded_registrations(),
        }
    }
}

impl ComAmazonawsRoute53domainsRoute53domainsV20140515State {
    /// Whether the REGISTRY holds `name` for anybody: this account, or a third party.
    ///
    /// This is the question `CheckDomainAvailability` answers, and it is wider than
    /// "does this account hold it" — the account's own domains are only part of what is
    /// already registered.
    pub fn is_registered(&self, name: &str) -> bool {
        self.domains.contains_key(name) || self.registered_elsewhere.contains(name)
    }

    /// What the registry's record of `name` means for transferring it IN to this
    /// registrar — the question `CheckDomainTransferability` answers.
    ///
    /// It is the same record [`Self::is_registered`] reads, told apart by WHERE the
    /// registration lives: this account's own domains, a third party's registration
    /// outside this registrar, or no registration at all.
    pub fn transferability(&self, name: &str) -> Transferability {
        if self.domains.contains_key(name) {
            Transferability::DomainInOwnAccount
        } else if self.registered_elsewhere.contains(name) {
            Transferability::HeldElsewhere
        } else {
            Transferability::NotRegistered
        }
    }

    /// Append the operation record a mutating call owes and return its `OperationId`.
    ///
    /// The record starts `SUBMITTED` and the clock owes its move to `SUCCESSFUL`
    /// [`OPERATION_SETTLE_MILLIS`] later; [`Self::settle_operations`] materialises that
    /// move when a later call observes the record.
    ///
    /// The log is capped at [`MAX_OPERATION_RECORDS`]: the oldest records are dropped to
    /// make room, so however many calls arrive the store cannot grow past that.
    pub fn record_operation(
        &mut self,
        domain_name: &str,
        op_type: &'static str,
        now_millis: i64,
    ) -> String {
        // The model constrains `OperationId` only as a string of at most 255 characters;
        // AWS renders it in UUID form, so the id is a v4 UUID from the `uuid` crate
        // (which the sibling qbusiness service mints its ids with too).
        let operation_id = ::uuid::Uuid::new_v4().to_string();
        if self.operations.len() >= MAX_OPERATION_RECORDS {
            let overflow = self.operations.len() - MAX_OPERATION_RECORDS + 1;
            self.operations.drain(..overflow);
        }
        self.operations.push(OperationRecord {
            operation_id: operation_id.clone(),
            domain_name: domain_name.to_string(),
            op_type,
            status: STATUS_SUBMITTED,
            submitted_millis: now_millis,
            last_updated_millis: now_millis,
            settles_at_millis: now_millis.saturating_add(OPERATION_SETTLE_MILLIS),
        });
        operation_id
    }

    /// Materialise every operation completion the clock has already made due.
    ///
    /// This is the one write a reporting operation may perform: the move was owed at
    /// `settles_at_millis` whether or not anybody called, so recording it on read
    /// changes nothing a later caller would otherwise have seen.
    pub fn settle_operations(&mut self, now_millis: i64) {
        for record in &mut self.operations {
            if record.status == STATUS_SUBMITTED && now_millis >= record.settles_at_millis {
                record.status = STATUS_SUCCESSFUL;
                record.last_updated_millis = record.settles_at_millis;
            }
        }
    }

    /// The record `operation_id` names, if this account has one.
    pub fn operation(&self, operation_id: &str) -> Option<&OperationRecord> {
        self.operations
            .iter()
            .find(|record| record.operation_id == operation_id)
    }
}

// ---------------------------------------------------------------------------
// Domain names
// ---------------------------------------------------------------------------

/// Why a request's `DomainName` cannot name a domain at this registrar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DomainNameError {
    /// Empty, or outside the characters and label rules the model documents.
    Invalid,
    /// Well formed, but not a name a registry delegates: its top-level domain is not one
    /// anybody registers under, or the name sits below the level a registrar sells.
    UnsupportedTld,
}

/// The longest a single DNS label may be, in octets (RFC 1035 §2.3.4). Not in the Smithy
/// model, but a rule every registry enforces, so a longer label is not a name a
/// registrar could ever hold.
const MAX_LABEL_LEN: usize = 63;

/// Whether `candidate` is a suffix that a registry delegates names under.
///
/// Asked of the Public Suffix List, which the `psl` crate carries compiled in: a name is
/// a public suffix exactly when names are registered directly beneath it. That makes this
/// a rule rather than a list this crate maintains — `com`, `co.uk`, `com.au`, `dev`,
/// `app`, `ai` and every ccTLD answer yes, while `invalid`, `test`, `localhost` (RFC
/// 2606, in no registry), an unassigned label such as `zzz`, and an ordinary subdomain
/// such as `b.example.com` answer no.
///
/// `psl::suffix` returns the public suffix OF its argument, so `candidate` is itself a
/// public suffix only when the answer is `candidate` and the list knows that rule rather
/// than falling back to its implicit "any unknown single label" rule.
fn is_public_suffix(candidate: &str) -> bool {
    psl::suffix(candidate.as_bytes())
        .is_some_and(|suffix| suffix.is_known() && suffix.as_bytes() == candidate.as_bytes())
}

/// Resolve a request's `DomainName` to the key the domain store is indexed by.
///
/// One rule, applied by every operation that takes a `DomainName`:
///
/// * domain names are not case sensitive, so the name is lower-cased;
/// * the name must be non-empty and contain only the characters the `DomainName`
///   member's documentation allows — letters `a`-`z`, digits `0`-`9`, hyphens that are
///   neither the first nor the last character of a label, and `.` separating the labels
///   — otherwise [`DomainNameError::Invalid`];
/// * no label may exceed the 63 octets DNS allows one (RFC 1035 §2.3.4), which every
///   registry enforces, and the whole name may not exceed the 255 characters the
///   `DomainName` member's `@length` allows: either is [`DomainNameError::Invalid`];
/// * what follows the first label must be a public suffix — see [`is_public_suffix`] —
///   because that is precisely the level at which a registry accepts a registration. The
///   `DomainName` member's documentation requires the TLD to be "a TLD that Route 53
///   supports"; anything that is not a registrable name at all is refused here with
///   [`DomainNameError::UnsupportedTld`], which is the error the model declares for a
///   top-level domain Route 53 cannot register under. This also refuses a third-level
///   name such as `a.b.example.com`, which no registrar sells.
pub fn resolve_domain_name(raw: &str) -> Result<String, DomainNameError> {
    let name = raw.to_ascii_lowercase();
    if name.is_empty() || name.len() > 255 {
        return Err(DomainNameError::Invalid);
    }

    let labels: Vec<&str> = name.split('.').collect();
    if labels.len() < 2 {
        return Err(DomainNameError::Invalid);
    }
    for label in &labels {
        let ok = !label.is_empty()
            && label.len() <= MAX_LABEL_LEN
            && !label.starts_with('-')
            && !label.ends_with('-')
            && label
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
        if !ok {
            return Err(DomainNameError::Invalid);
        }
    }

    if !is_public_suffix(&labels[1..].join(".")) {
        return Err(DomainNameError::UnsupportedTld);
    }
    Ok(name)
}

/// The message an `InvalidInput` refusal of a malformed `DomainName` carries.
///
/// It names the member and the rule the value failed, the way the framework's boundary
/// validator words a refusal, so a caller can fix the request from the message alone.
pub fn invalid_domain_name_message(raw: &str) -> String {
    // A value with nothing in it is not a name that broke one of the syntax rules: the
    // member carries no name at all, and that condition is refused in its own words
    // ("Invalid request. Problems: " followed by the problem).
    if raw.trim().is_empty() {
        return "Invalid request. Problems: Domain name cannot be blank".to_string();
    }
    format!(
        "Value '{raw}' at 'domainName' failed to satisfy constraint: Member must be a \
         domain name of one label under a top-level domain, where every label is made up \
         of letters, digits and interior hyphens"
    )
}

/// The message an `UnsupportedTLD` refusal carries.
///
/// Wording the real service is observed to use for this refusal: the fixed text is the
/// leading "The " and the trailing " TLD is not supported", and the one variable part is
/// the TOP-LEVEL DOMAIN of the requested name — the labels after the first, unquoted —
/// since the TLD, not the whole name, is what the registrar does not offer. The TLD is
/// lower-cased, the way every other use of a domain name in this crate is, because
/// domain names are not case sensitive.
pub fn unsupported_tld_message(raw: &str) -> String {
    format!("The {} TLD is not supported", tld_of(raw).to_ascii_lowercase())
}

/// The message the `InvalidInput` refusal of a domain this account does not hold carries.
pub fn unknown_domain_message(name: &str) -> String {
    format!("Domain {name} is not registered to this account")
}

/// The message the `DomainLimitExceeded` refusal carries when the account is at
/// [`MAX_DOMAINS`].
pub fn domain_limit_message() -> String {
    format!("The number of domains has exceeded the allowed threshold of {MAX_DOMAINS}")
}

/// The TLD of an already resolved domain name: everything after its first label.
pub fn tld_of(name: &str) -> &str {
    name.split_once('.').map_or("", |(_, tld)| tld)
}

/// The registrar of record for a TLD.
///
/// `RegisterDomain`'s own documentation names the two registrars Route 53 uses — "either
/// for Amazon Registrar or for our registrar associate, Gandi" — and Amazon Registrar is
/// the registrar for the TLDs Amazon operates itself (`.com`, `.net`, `.org`); every
/// other TLD is registered through Gandi.
pub fn registrar_name(tld: &str) -> &'static str {
    match tld {
        "com" | "net" | "org" => "Amazon Registrar, Inc.",
        _ => "Gandi SAS",
    }
}

/// The four name servers Route 53 assigns a newly registered domain's hosted zone.
///
/// `RegisterDomain`'s documentation states the shape of this: registration "[c]reates a
/// Route 53 hosted zone that has the same name as the domain. Route 53 assigns four name
/// servers to your hosted zone and automatically updates your domain registration with
/// the names of these name servers." Route 53's delegation sets take one name server from
/// each of four TLDs and are named `ns-<n>.awsdns-<nn>.<tld>`.
///
/// Derived from the domain name, so the same domain always has the same delegation set
/// and two different domains almost never share one.
pub fn default_nameservers(name: &str) -> Vec<Nameserver> {
    // FNV-1a over the domain name: a tiny, well-defined non-cryptographic hash. Nothing
    // here is security-sensitive — it only has to spread names over the delegation sets.
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in name.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }

    ["com", "net", "org", "co.uk"]
        .into_iter()
        .enumerate()
        .map(|(index, zone)| {
            let shift = u32::try_from(index).unwrap_or(0) * 13;
            let server = (hash.rotate_left(shift)) % 4096;
            let set = (hash.rotate_right(shift + 7)) % 64;
            Nameserver {
                name: format!("ns-{server}.awsdns-{set:02}.{zone}"),
                glue_ips: None,
            }
        })
        .collect()
}

/// The instant a registration that started at `creation_millis` and ran for `years`
/// expires: the same calendar date and time of day, `years` years later.
///
/// A registration period is measured in years (`DurationInYears`), not in days, so this
/// is calendar arithmetic. 29 February falls back to 28 February in a non-leap year,
/// the only date with no anniversary. A `years` value the calendar cannot reach — which
/// the modelled `@range(min: 1, max: 10)` already refuses before any handler runs — and
/// an unrepresentable instant both fall back to `creation_millis`, so no arithmetic here
/// can overflow on a value a caller chose.
pub fn expiration_millis(creation_millis: i64, years: i32) -> i64 {
    let Ok(start) =
        time::OffsetDateTime::from_unix_timestamp_nanos(i128::from(creation_millis) * 1_000_000)
    else {
        return creation_millis;
    };
    let Some(year) = start.year().checked_add(years) else {
        return creation_millis;
    };
    let Ok(date) = time::Date::from_calendar_date(year, start.month(), start.day())
        .or_else(|_| time::Date::from_calendar_date(year, start.month(), 28))
    else {
        return creation_millis;
    };
    let nanos = date.with_time(start.time()).assume_utc().unix_timestamp_nanos();
    i64::try_from(nanos / 1_000_000).unwrap_or(creation_millis)
}

/// A modelled `Timestamp` as the epoch milliseconds this service's records hold.
///
/// Saturating end to end, so a client-chosen timestamp cannot overflow the conversion.
pub fn millis_of(at: ::aws_smithy_types::DateTime) -> i64 {
    at.secs()
        .saturating_mul(1000)
        .saturating_add(i64::from(at.subsec_nanos() / 1_000_000))
}

/// A modelled timestamp from this service's millisecond clock.
pub fn date_time(millis: i64) -> ::aws_smithy_types::DateTime {
    ::aws_smithy_types::DateTime::from_millis(millis)
}

// ---------------------------------------------------------------------------
// Reaching the store
// ---------------------------------------------------------------------------

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsRoute53domainsRoute53domainsV20140515State>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsRoute53domainsRoute53domainsV20140515State>)
}

/// The failure a handler answers when [`service_state`] cannot reach the store.
///
/// Unreachable in this emulator — the router builds one `InMemoryEngine` and hands it to
/// every operation's constructor, so the downcast always succeeds — and this service's
/// model declares no server fault, so the scaffold channel's `NotImplemented` (HTTP 501)
/// is the only status left that does not blame the caller for a request that was fine.
pub fn state_unavailable<E>(operation: &str) -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::NotImplemented(crate::scaffolded::NotImplemented {
        message: format!(
            "operation {operation} could not reach this emulator's route53domains state store"
        ),
    })
}
