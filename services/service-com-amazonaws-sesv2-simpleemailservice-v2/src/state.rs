// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-sesv2-simpleemailservice-v2 service.
//!
//! Everything the implemented operations answer with is computed from the records
//! below: the email-identity table, the contact-list table, the email-template
//! table, the configuration-set
//! table, the suppression-list table, the multi-region-endpoint table, the
//! dedicated-IP table, the export-job table, the import-job table, the
//! outbound-message log, the
//! per-account send counters and
//! the account-level attributes. Nothing here is per-request scratch — the whole
//! struct lives in the one shared engine (see [`service_state`]).
//!
//! The email-identity table here is this surface's VIEW of one account-wide set of
//! email identities, which lives in the shared engine as
//! [`::service_common::ses_identities::SesIdentityDirectory`] and which the classic
//! SES surface publishes to and reads as well: an identity belongs to the account
//! rather than to the API that created it. What a record here adds to that set is what
//! THIS API adds to an identity — how its model spells its DKIM configuration, its
//! tags, its default configuration set, its certificate associations — and
//! [`reconcile_identities`] is where the two are brought back into agreement, on every
//! operation this service serves. The Easy DKIM setup itself is the account identity's
//! ([`::service_common::ses_identities::SesDkimSetup`]): the tokens an owner publishes
//! are the same records whichever API handed them out.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::time::{Duration, SystemTime};

use smithy_server_runtime::{Violation, ViolationKind};

pub mod event_publication;

/// What the ACCOUNT does with a message this surface has just accepted for one of its own
/// recipients — the hand-off to Amazon SES email receiving, which the classic surface's store
/// owns, and the document a send assembles for it.
pub mod account_receiving;

/// The configuration-set plane: one configuration set, its event destinations, and the
/// wording every operation that addresses one refuses with. Re-exported below, so a
/// handler reaches each item as `crate::state::…`.
mod configuration_sets;
/// The dedicated-IP plane: the pools, the addresses in them, and the blocklist
/// listings observed for those addresses.
mod dedicated_ips;
/// Whether a domain identity's Easy DKIM records are published in the account's own DNS,
/// which is what completes its verification.
mod dkim_publication;
/// The order this service states an enumerated type's values in when it refuses a
/// value outside them.
mod enum_value_sets;
/// The export-job plane's one piece of behaviour: the file a run writes, which is the
/// records its data source selects rendered in the data format it asks for.
mod export_file;
/// The export-job plane's records: one export job, the status its run reports, and how a
/// cancellation ends that run.
mod export_job;
/// The rules one sending-authorization policy of an email identity has to satisfy, and
/// the sentences a policy write is refused with — shared by every operation that writes
/// one.
mod identity_policy_rules;
/// The S/MIME certificate an identity's association names, resolved against the service
/// that holds it.
mod identity_certificates;
/// The account's VDM metric series, which a metric query reports and a metrics export
/// writes into its file.
mod metric_series;
/// The identifiers this service mints in UUID form, and the one function that writes
/// them.
mod minted_ids;
/// The account's pricing plane: the plan ladder, and where a requested plan lands on
/// the account's pricing attributes.
mod pricing;
/// The suppression-list plane: one suppressed address and how a list's entries are
/// keyed.
mod suppression;
/// The import-job plane's one piece of behaviour: the Amazon S3 data source a run reads,
/// resolved against the account's own object storage, and what the store's answer about it
/// means for the run.
pub mod import_source;
/// A tenant's reputation-entity status records and its suppression preferences: what
/// each is held as while nothing has written it, and what a read reports it as.
mod tenant_reputation;
/// The hold a tenant's resource associations put on the resource they name: the one gate
/// every delete of an associable resource reads, and the sentence it refuses with.
mod tenant_associations;
/// The tenant a send names, resolved against the account's tenants: the one lookup both
/// send paths run before the message is weighed against the account's sending identities,
/// and the sentence an unresolvable name is refused with.
mod send_tenant;
/// The account's Virtual Deliverability Manager configuration as a gate: whether the
/// feature is on, and how a call that can only be answered from its data is refused
/// while it is off.
mod vdm;
/// What the REQUEST alone settles about an outbound message — the addresses it names, the
/// recipients it addresses, the headers, tags and attachments it carries — and how a defect
/// in it reads, which is the stage both send paths run before anything of the account is
/// read.
pub mod send_validation;
/// The template language a template SAVED in the account is written in, and how one is
/// evaluated against a set of rendering data — the full language, as against the inline
/// substitution below.
pub mod template_language;
/// The email-template plane's one piece of behaviour: how a template's content becomes a
/// destination's message, which is the message variables a send supplies substituted into
/// the template's parts.
pub mod template_rendering;
/// This surface's view of the ACCOUNT's email templates — one set for both SES APIs — and
/// the token the template listing pages it with.
pub mod templates;
mod constraint_refusals;
/// ONE email identity of the account and the states it carries: its class, its
/// verification, its DKIM setup, its custom MAIL FROM configuration and its certificate
/// associations.
mod email_identities;
pub use constraint_refusals::*;

pub use configuration_sets::*;
pub use dedicated_ips::*;
pub use dkim_publication::*;
pub use email_identities::*;
pub use enum_value_sets::*;
pub use export_file::*;
pub use export_job::*;
pub use identity_policy_rules::*;
pub use identity_certificates::*;
pub use metric_series::*;
pub use minted_ids::*;
pub use pricing::*;
pub use suppression::*;
pub use templates::*;
pub use send_tenant::*;
pub use tenant_associations::*;
pub use tenant_reputation::*;
pub use vdm::*;

/// How long after the EVIDENCE of an identity's verification an identity's `PENDING` ->
/// `SUCCESS` verification transition becomes due.
///
/// Real SES does not complete a verification on its own: it completes one only once it
/// OBSERVES the proof the identity's owner provides — the recipient following the link in
/// the verification message, or the Easy DKIM CNAME records appearing in the domain's own
/// DNS — and it keeps looking for the records on a schedule of its own
/// ([`VERIFICATION_PROBE_INTERVAL`]), reporting the identity `PENDING` and
/// `VerifiedForSendingStatus` false until one of those searches finds them. A DOMAIN's
/// records are the account's to publish and this service's to read
/// ([`resolve_dkim_publication`]), so this span is counted from the pass that FINDS them,
/// and a domain whose owner publishes nothing reads back unverified however many reads are
/// made. For an email-address identity the proof is a mailbox nothing here can watch, so
/// the span is counted from the create.
///
/// The span itself is CHOSEN, not documented: Amazon SES states no instant at which a
/// verification completes — "it can take up to 72 hours" after the records are published, a
/// span no emulated client could wait out — so what this fixes is the SHAPE of the
/// behaviour rather than its duration, and it is the span every other transition this
/// service's clock owes a resource falls due after ([`MAIL_FROM_MX_SETTLE`], the search for
/// the OTHER record set a caller publishes for a domain, is the same one). It is never
/// zero, so a client that provides the proof and reads straight back still sees the state
/// it was in — the transient `PENDING`, the `VerifiedForSendingStatus` false and the state
/// errors that go with them. The due instant is stored on the record; nothing ever sleeps
/// for it.
pub const VERIFICATION_SETTLE: Duration = Duration::from_secs(3);

/// How long after one search of an identity's DNS records the next one falls due.
///
/// Real SES re-queries the records it was told to look for on a schedule nothing
/// here can observe, so the cadence is CHOSEN, not documented: it is short enough
/// that `VerificationInfo$LastCheckedTimestamp` is a live instant and long enough
/// that two reads of one identity in the same breath report the same one. The due
/// instant is stored on the record; nothing ever sleeps for it.
pub const VERIFICATION_PROBE_INTERVAL: Duration = Duration::from_secs(3);

/// Top-level domains the DNS root does not delegate, so no name under one of them
/// can carry a DNS record for anybody.
///
/// `invalid` is the name RFC 6761 section 6.4 reserves for "domain names that are
/// sure to be invalid": a query for a name under it answers "no such name", whatever
/// its owner does, so the records SES looks for can never be published there. The
/// reserved SECOND-level names (`example.com` and its siblings) are deliberately NOT
/// here, and neither are the other special-use top-level names: those stand for a
/// zone a resolver may well answer for.
const UNDELEGATED_TOP_LEVEL_DOMAINS: [&str; 1] = ["invalid"];

/// Whether a DNS zone covering `domain` can exist at all.
///
/// `false` for a name whose top-level domain the root does not delegate
/// ([`UNDELEGATED_TOP_LEVEL_DOMAINS`]): a query for such a name answers "no such
/// name" however its owner configures DNS, so the records SES looks for can never be
/// published there. `true` otherwise — a zone that may exist, whose owner may or may
/// not have published them.
///
/// The name is compared the way DNS compares one ([`domain_key`]).
pub fn dns_zone_can_exist(domain: &str) -> bool {
    let name = domain_key(domain);
    let top_level = name
        .trim_end_matches('.')
        .rsplit('.')
        .next()
        .unwrap_or_default();
    !top_level.is_empty() && !UNDELEGATED_TOP_LEVEL_DOMAINS.contains(&top_level)
}

/// The `VerificationError` a search for `domain`'s DKIM records reports while they
/// have not been found.
///
/// The model's own two readings of a failed lookup: `HOST_NOT_FOUND` is "the queried
/// hostname does not exist or was not reachable at the time of the request", which is
/// what a name no DNS zone can cover answers; `TYPE_NOT_FOUND` is "the queried
/// hostname exists but does not have the requested type of DNS record", which is what
/// a name under a zone that may exist answers while the records it was told to publish
/// are not there.
pub fn dkim_lookup_error(domain: &str) -> &'static str {
    if dns_zone_can_exist(domain) {
        "TYPE_NOT_FOUND"
    } else {
        "HOST_NOT_FOUND"
    }
}

/// How long after a custom MAIL FROM domain is configured the search for that
/// domain's MX record is reported as having found it.
///
/// Real SES reports `SUCCESS` only once it observes something it cannot observe
/// here (the MX record the caller is told to publish appearing in the MAIL FROM
/// domain's DNS), so the span is CHOSEN, not documented: it is long enough that a
/// caller which reads straight back still sees the transient `PENDING` state the
/// configuration starts in, and the behaviour that goes with it. The due instant is
/// stored on the record; nothing ever sleeps for it.
pub const MAIL_FROM_MX_SETTLE: Duration = Duration::from_secs(3);

/// How long after a certificate association is created its `PROVISIONING` ->
/// `ACTIVE` or `FAILED` transition becomes due, and how long after it is removed its
/// `DEPROVISIONING` clean-up is due.
///
/// Both spans are CHOSEN, not documented: real SES prepares the certificate for
/// signing (and later cleans the association up) on its own schedule, which nothing
/// here can observe. The span is long enough that a caller which reads straight back
/// still sees the transient state and the state rules that go with it. The due
/// instant is stored on the record; nothing ever sleeps for it.
pub const CERTIFICATE_SETTLE: Duration = Duration::from_secs(3);

/// Certificate associations one email identity may hold.
///
/// A CHOSEN bound, not an AWS one: a domain identity takes one association per
/// address in the domain, so a long-lived emulator must not grow without limit while
/// serving them. It is far above what a client signing from a domain's addresses
/// needs.
pub const MAX_CERTIFICATES_PER_IDENTITY: usize = 1_000;

/// Easy DKIM publishes three CNAME tokens for a domain identity.
///
/// From the `DkimAttributes$Tokens` documentation: "this object contains a set of
/// unique strings that you use to create a set of CNAME records that you add to the
/// DNS configuration for your domain".
const EASY_DKIM_TOKEN_COUNT: usize = 3;

/// Length of one Easy DKIM token, in characters.
const EASY_DKIM_TOKEN_LEN: usize = 32;

/// How long after an export job is created its `CREATED` -> `PROCESSING`
/// transition becomes due.
///
/// "Export jobs run asynchronously. This operation returns a JobId. Call
/// GetExportJob with that ID until JobStatus is COMPLETED, FAILED, or CANCELLED":
/// real SES picks the run up and finishes it on its own schedule, which nothing here
/// can observe, so the span is CHOSEN, not documented. It is the emulator's
/// convention for a transition the clock owes, long enough that a caller which reads
/// straight back still sees the job in the status it was created in. The due instant
/// is stored on the record; nothing ever sleeps for it.
pub const EXPORT_JOB_PROCESSING_SETTLE: Duration = Duration::from_secs(3);

/// How long after an export job starts processing its `PROCESSING` -> `COMPLETED`
/// transition becomes due, counted from the instant processing fell due — so a run
/// is complete [`EXPORT_JOB_PROCESSING_SETTLE`] plus this after the job was created.
///
/// CHOSEN for the same reason as [`EXPORT_JOB_PROCESSING_SETTLE`], and deliberately
/// not zero: a caller that reads straight after the run started still sees it
/// processing. The due instant is stored on the record; nothing ever sleeps for it.
pub const EXPORT_JOB_COMPLETION_SETTLE: Duration = Duration::from_secs(3);

/// Export jobs the account may hold.
///
/// A CHOSEN bound, not an AWS one: an export job is created by a request, so a
/// long-lived emulator must not grow without limit while serving them. It is far
/// above what a client exercising the export API needs, and `CreateExportJob` refuses
/// past it with the `LimitExceededException` its model binds.
pub const MAX_EXPORT_JOBS: usize = 1_000;

/// The `JobStatus` an import job's run reports before it has started: "CREATED – Job
/// has just been created."
///
/// Source: `smithy_query(query="doc",shape="JobStatus")`; the values the member
/// accepts are also in `crate::ops::types::JOB_STATUS_VALUES`.
pub const IMPORT_JOB_CREATED: &str = "CREATED";

/// The `JobStatus` an import job's run reports while it is under way: "PROCESSING –
/// Job is processing."
pub const IMPORT_JOB_PROCESSING: &str = "PROCESSING";

/// The terminal `JobStatus` an import job's run reaches when it processed the data source
/// it was created for: "COMPLETED – Job has completed processing successfully."
///
/// A run reaches it only where the account's object storage ANSWERED for the source the
/// job reads ([`import_source`]); a run whose source could not be read reaches
/// [`IMPORT_JOB_FAILED`] instead. `CANCELLED` needs a call that cancels an import job,
/// which this service models none of.
pub const IMPORT_JOB_COMPLETED: &str = "COMPLETED";

/// The terminal `JobStatus` an import job's run reaches when it could not process its data
/// source at all: the model's `FAILED`, which the enum's own documentation glosses as "an
/// error occurred during processing".
///
/// What diagnosed the error is the run's read of the Amazon S3 object the job's
/// `ImportDataSource` names ([`import_source`]): the store answering that it holds no such
/// object, or that the read is not permitted, is a source the run cannot process, and the
/// job carries the `FailureInfo` saying so.
pub const IMPORT_JOB_FAILED: &str = "FAILED";

/// How long after an import job is created its `CREATED` -> `PROCESSING` transition
/// becomes due.
///
/// Real SES picks the run up on a schedule nothing here can observe, so the span is
/// CHOSEN, not documented: it is this emulator's convention for a transition the clock
/// owes, and deliberately not zero, so a caller that reads the job straight back still
/// sees it in the status it was created in. The due instant is stored on the record;
/// nothing ever sleeps for it.
pub const IMPORT_JOB_PROCESSING_SETTLE: Duration = Duration::from_secs(3);

/// How long after an import job starts processing its `PROCESSING` -> `COMPLETED`
/// transition becomes due, counted from the instant processing fell due — so a run is
/// complete [`IMPORT_JOB_PROCESSING_SETTLE`] plus this after the job was created.
///
/// CHOSEN for the same reason as [`IMPORT_JOB_PROCESSING_SETTLE`], and likewise not
/// zero: a caller that reads straight after the run started still sees it processing.
pub const IMPORT_JOB_COMPLETION_SETTLE: Duration = Duration::from_secs(3);

/// Import jobs the account may hold.
///
/// A CHOSEN bound, not an AWS one: an import job is created by a request, so a
/// long-lived emulator must not grow without limit while serving them. It is far above
/// what a client exercising the import API needs, and `CreateImportJob` refuses past it
/// with the `LimitExceededException` its model binds.
pub const MAX_IMPORT_JOBS: usize = 1_000;

/// The window the account's sending quota is measured over: SES states the quota
/// as `Max24HourSend`, "the maximum number of emails you can send in a 24-hour
/// period".
pub const SEND_QUOTA_WINDOW: Duration = Duration::from_secs(24 * 60 * 60);

/// Verified identities an account may hold: SES's documented quota is 10,000
/// identities per account per Region.
pub const MAX_IDENTITIES: usize = 10_000;

/// How long after an account asks for production access the decision on that
/// request becomes due.
///
/// Real SES decides the request by a review nothing here can observe (a support
/// case worked by a human), so the span is CHOSEN, not documented: it is the
/// emulator's convention for a transition the clock owes, long enough that a caller
/// which reads straight back still sees the request under review and the rules that
/// go with it. The due instant is stored on the account record; nothing ever sleeps
/// for it.
pub const PRODUCTION_ACCESS_REVIEW_SETTLE: Duration = Duration::from_millis(500);

/// Entries the outbound-message log retains, oldest evicted first.
///
/// A CHOSEN bound, not an AWS one: the log is what lets a caller read back what it
/// sent, and a long-lived emulator must not grow without limit while serving
/// requests. It is well above the number of messages the sending quota lets a
/// caller accept in one window.
pub const MAX_SENT_MESSAGES: usize = 1_000;

/// Characters of a message body the log retains, the rest truncated.
///
/// Also CHOSEN: the body is caller-sized (a raw MIME document has no modelled
/// length bound), so the retained log is bounded by entries times this.
pub const MAX_STORED_BODY_CHARS: usize = 8 * 1024;

/// Contact lists the account may hold.
///
/// A CHOSEN bound, not an AWS one: a contact list is created by a request, so a
/// long-lived emulator must not grow without limit while serving them. It is far
/// above what a client exercising the contact API needs, and `CreateContactList`
/// refuses past it with the `LimitExceededException` its model binds.
pub const MAX_CONTACT_LISTS: usize = 1_000;

/// Contacts one contact list may hold.
///
/// CHOSEN for the same reason, and for the same growth: one record per accepted
/// `CreateContact`.
pub const MAX_CONTACTS_PER_LIST: usize = 10_000;

/// Email templates the account may hold, and so the ceiling on the template store:
/// SES documents a limit of 10,000 email templates per account, and
/// `CreateEmailTemplate` refuses past it with the `LimitExceededException` its model
/// binds rather than growing the store without end.
pub const MAX_EMAIL_TEMPLATES: usize = 10_000;

/// The quota SES documents for one email template, in the unit it documents it in — and
/// the unit a refusal past it states ([`email_template_exceeds_maximum_size`]). The size
/// in bytes is derived from this rather than the other way round, so the sentence and the
/// bound cannot disagree.
pub const MAX_EMAIL_TEMPLATE_KILOBYTES: usize = 500;

/// The largest one email template may be, counting its subject line and both body
/// parts: SES documents a maximum size of 500 KB for an email template, and
/// `CreateEmailTemplate` refuses past it on the input rather than holding a template
/// no send could use. The parts are caller-sized — none of them carries a length bound
/// in the model — so this bound and [`MAX_EMAIL_TEMPLATES`] together are what bound the
/// template store.
pub const MAX_EMAIL_TEMPLATE_BYTES: usize = MAX_EMAIL_TEMPLATE_KILOBYTES * 1024;

/// Addresses the account's suppression lists may hold, across the account-level list
/// and every tenant's own.
///
/// CHOSEN for the same reason, and for the same growth: one record per accepted
/// suppression write, under a key the request's own address decides.
/// [`ComAmazonawsSesv2SimpleemailserviceV2State::suppress_destination`] is the one
/// way an entry enters the table and it refuses past this bound, so no sequence of
/// requests can grow the table without limit. It is well above what a client
/// exercising the suppression API needs.
pub const MAX_SUPPRESSED_DESTINATIONS: usize = 10_000;

/// How long after `CreateMultiRegionEndpoint` an endpoint's `CREATING` -> `READY`
/// transition becomes due.
///
/// The create call answers with the non-terminal `CREATING` ("the resource is being
/// provisioned") and the resource becomes `READY` once provisioning finishes, which
/// nothing here performs, so the span is CHOSEN, not documented: it is long enough
/// that a caller reading straight back still sees the transient state, and the due
/// instant is stored on the record so nothing ever sleeps for it.
pub const ENDPOINT_PROVISIONING_SETTLE: Duration = Duration::from_millis(500);

/// How long after `DeleteMultiRegionEndpoint` the deletion that call started is
/// complete.
///
/// The delete answers with the non-terminal `DELETING` ("the resource is being deleted
/// as requested"), so the endpoint is still there, being torn down, until this span
/// falls due: a read reports it as `DELETING`, a second delete collides with the one
/// already in flight, and once the span falls due the endpoint and its name are gone.
/// CHOSEN — AWS documents no duration for a tear-down — but deliberately not
/// sub-second like [`ENDPOINT_PROVISIONING_SETTLE`], which covers a resource becoming
/// usable. What this one covers is a refusal: the collision is a 5xx, so a client's
/// SDK retries the same delete for seconds before surfacing it, and a tear-down that
/// finished inside that retry chain would answer the retrying client `404` for a
/// resource it is still deleting — which is not what the service does. Stored as a due
/// instant, so nothing ever sleeps for it, and stated here ONCE: this constant is the
/// emulator's single knob for the span, and every call and test reads it from here.
pub const ENDPOINT_DELETION_SETTLE: Duration = Duration::from_secs(10);

/// The lifecycle status an endpoint whose deletion is still running is in, as AWS
/// names it when it refuses a second delete of that endpoint.
pub const DEPROVISIONING_STATUS: &str = "DEPROVISIONING";

/// Multi-region endpoints the store holds, across every primary region.
///
/// A CHOSEN bound, not an AWS one (SES states its quota per account per Region, and
/// this emulator serves one account): an endpoint is created by a request, so a
/// long-lived emulator must not grow without limit while serving them, and the bound
/// is on the WHOLE table — the region a record is keyed under comes from the request,
/// so a per-region bound would not bound anything. `CreateMultiRegionEndpoint` refuses
/// past it with the `LimitExceededException` its model binds.
pub const MAX_MULTI_REGION_ENDPOINTS: usize = 1_000;

/// Tags one resource may carry.
///
/// From the model's `Tag` documentation: "A resource can have as many as 50 tags."
pub const MAX_TAGS_PER_RESOURCE: usize = 50;

/// The longest a tag key may be, in characters.
///
/// From `Tag$Key`: "The maximum length of a tag key is 128 characters. The minimum
/// length is 1 character." `TagKey` is a plain string in the model, so the bound is
/// the handler's to enforce. Characters, not bytes: the documentation states what the
/// key "can contain", and it may contain "Unicode letters".
pub const MAX_TAG_KEY_CHARS: usize = 128;

/// The shortest a tag key may be, in characters: `Tag$Key`'s "The minimum length is 1
/// character" — a tag is keyed by its key, so there is no tag without one.
pub const MIN_TAG_KEY_CHARS: usize = 1;

/// The longest a tag value may be, in characters.
///
/// From `Tag$Value`: "The maximum length of a tag value is 256 characters. The minimum
/// length is 0 characters" — so a key with no value is a tag, and `TagValue` carries no
/// bound of its own in the model.
pub const MAX_TAG_VALUE_CHARS: usize = 256;

/// The prefix the model's `Tag` documentation reserves: "The `aws:` prefix is reserved
/// for use by Amazon Web Services; you can't use it in any tag keys or values that you
/// define."
pub const RESERVED_TAG_PREFIX: &str = "aws:";

/// The symbols a tag key or value may carry besides letters, digits and white space.
///
/// The model's `Tag` documentation lists "Unicode letters, digits, white space, or one
/// of the following symbols: _ . : / = + -", and the service's own refusal for a tag
/// outside the set ([`tag_restriction_refusal`]) names those seven and `@` as well, so
/// `@` is in the set the surface accepts. Written in the order that sentence names them.
pub const TAG_SYMBOLS: &str = "_.:/=+-@";

/// Whether `text` is in the character set a tag key or a tag value may be written in.
///
/// The set the surface states: letters, digits, white space and [`TAG_SYMBOLS`].
/// Letters and digits are read as the Unicode properties the model's `Tag`
/// documentation names them by ("Unicode letters, digits"), not as ASCII, so a key or
/// value written in any script is in the set while punctuation and symbols outside the
/// listed ones are not.
pub fn tag_text_is_in_charset(text: &str) -> bool {
    text.chars()
        .all(|c| c.is_alphanumeric() || c.is_whitespace() || TAG_SYMBOLS.contains(c))
}

/// The sentence this surface answers a tagging call whose tags it will not apply.
///
/// FIXED text, with nothing of the request in it: the refusal states that the resource was
/// not tagged, and echoes neither the key, nor the value, nor the ARN — a caller learns
/// which restriction it broke from the documentation of `Tag`, not from the sentence, and
/// nothing of what it submitted is reflected back to it.
pub const TAG_WRITE_FAILED: &str = "Failed to tag resource";

/// Whether the tag `key`/`value` breaks a LENGTH bound the model's `Tag` documentation
/// states for the shapes a tag is built from: the key's
/// ([`MIN_TAG_KEY_CHARS`]..=[`MAX_TAG_KEY_CHARS`] characters) or the value's
/// ([`MAX_TAG_VALUE_CHARS`]).
///
/// Such a bound is a constraint on the SHAPE a member targets, so a request carrying a
/// value outside it is refused where a request is validated against the shapes it is built
/// from — before any rule of the operation's own runs, and with no sentence of this
/// service's in it. Kept apart from [`tag_restriction_refusal`] for exactly that reason:
/// the two layers refuse two different conditions, so they must not collapse into one
/// error class or one message.
pub fn tag_breaks_shape_bound(key: &str, value: &str) -> bool {
    let key_chars = key.chars().count();
    !(MIN_TAG_KEY_CHARS..=MAX_TAG_KEY_CHARS).contains(&key_chars)
        || value.chars().count() > MAX_TAG_VALUE_CHARS
}

/// How this service words `BadRequestException` for a tag the SERVICE itself will not
/// store, or `None` when the pair breaks none of those restrictions.
///
/// Stated once, so every operation that accepts a `TagList` can read those restrictions
/// the same way rather than disagree about which tags it will store: the character set a
/// key and a value are written in ([`tag_text_is_in_charset`]) and the
/// [`RESERVED_TAG_PREFIX`] neither may carry. The sentences are fixed text, with nothing
/// of the request interpolated into either: the character-set one names the set the
/// surface accepts, and a tag carrying the reserved prefix is refused with the sentence
/// this surface answers a tagging call it will not apply ([`TAG_WRITE_FAILED`]), which
/// states no key, value or ARN at all.
pub fn tag_restriction_refusal(key: &str, value: &str) -> Option<String> {
    if !tag_text_is_in_charset(key) || !tag_text_is_in_charset(value) {
        return Some(tag_charset_refusal());
    }
    if key.starts_with(RESERVED_TAG_PREFIX) || value.starts_with(RESERVED_TAG_PREFIX) {
        return Some(TAG_WRITE_FAILED.to_string());
    }
    None
}

/// The sentence a tag key or value outside [`tag_text_is_in_charset`] is refused with.
///
/// Fixed text: the refusal is about the character set, which is the same whichever tag
/// of the request carried a character outside it. The special characters are listed
/// from [`TAG_SYMBOLS`], separated by spaces, so the sentence and the set the handler
/// enforces cannot drift apart.
pub fn tag_charset_refusal() -> String {
    let symbols = TAG_SYMBOLS
        .chars()
        .map(|symbol| symbol.to_string())
        .collect::<Vec<_>>()
        .join(" ");
    format!(
        "Tags can only contain letters, numbers, spaces, and the following special \
         characters: {symbols}"
    )
}

/// How this service words `BadRequestException` for a request that would leave one
/// resource carrying `total` tags, more than [`MAX_TAGS_PER_RESOURCE`].
///
/// Neither the model nor a recording of this surface states the sentence, so it is the
/// one `CreateDedicatedIpPool` already answers a create carrying more tags than a
/// resource may hold with — the model's own "A resource can have as many as 50 tags",
/// with the count the request would leave behind as its only variable part — so the
/// service words the one condition the same way whichever operation reads it.
pub fn too_many_tags(total: usize) -> String {
    format!("A resource can have as many as {MAX_TAGS_PER_RESOURCE} tags, got {total}.")
}

/// The resource TYPE the ARN `resource_arn` names and the id it addresses within that
/// type, or `None` when the value is not a resource ARN of this service naming a
/// resource at all.
///
/// SES writes a resource in the resource field of its ARN as `<type>/<id>`, so a value
/// of another vendor, or one carrying no id (a bare type names no resource), addresses
/// nothing this service can resolve — which is a bad REQUEST, refused on the input,
/// rather than a resource that happens to be missing. The ARN's six fields come from
/// the shared caller-supplied-ARN parse, which is where what an ARN of this vendor is —
/// fields, partition, the resource field's own colons — is decided.
pub fn ses_resource_of(resource_arn: &str) -> Option<(&str, &str)> {
    let resource_type = ::service_common::arn::parse_of(resource_arn, ARN_NAMESPACE)?.resource_type();
    let resource_id = ::service_common::arn::resource_id_of_type(resource_arn, resource_type)
        .filter(|resource_id| !resource_id.is_empty())?;
    Some((resource_type, resource_id))
}

/// How the tagging surface words `NotFoundException` for a `ResourceArn` that names
/// no resource the account holds in this Region.
///
/// One wording for every operation that resolves a `ResourceArn` to a tag set, so a
/// client sees the same sentence whichever of them first tells it the resource is gone.
///
/// This surface is RECORDED answering such a call with the fixed text
/// `No <Kind> present with name: ` followed by the NAME the ARN addresses the resource
/// by — never the whole ARN — where `<Kind>` is the resource kind the ARN's resource
/// type names ([`tagged_resource_kind`]). A tenant is addressed by an ARN carrying both
/// its name and its id ([`tenant_arn`]), and is recorded with ` with tenantId: ` run on
/// after the name without a separator, so a caller reads which tenant record was looked
/// for.
///
/// A value that is no resource ARN of this service at all never reaches here: every
/// operation refuses that on the input ([`UNSUPPORTED_RESOURCE_ARN`]).
pub fn tagged_resource_not_found(resource_arn: &str) -> String {
    let Some((resource_type, resource_id)) = ses_resource_of(resource_arn) else {
        return format!("Resource {resource_arn} does not exist.");
    };
    let kind = tagged_resource_kind(resource_type);
    match (resource_type, resource_id.split_once('/')) {
        ("tenant", Some((tenant_name, tenant_id))) => {
            format!("No {kind} present with name: {tenant_name}with tenantId: {tenant_id}")
        }
        _ => format!("No {kind} present with name: {resource_id}"),
    }
}

/// The resource KIND the ARN resource type `resource_type` names, as this service
/// spells one in a sentence about a resource it does not hold.
///
/// SES writes a resource type in an ARN in lower case with dashes and names the kind
/// itself in the model's own CamelCase spelling, so the kind is that type with each
/// dash-separated word capitalised — `contact-list` is a `ContactList` — except for the
/// identity, whose ARN type is `identity` while the model and the recorded sentence
/// both name the resource `EmailIdentity`.
fn tagged_resource_kind(resource_type: &str) -> String {
    if resource_type == "identity" {
        return "EmailIdentity".to_string();
    }
    resource_type
        .split('-')
        .map(|word| {
            let mut characters = word.chars();
            match characters.next() {
                Some(first) => first.to_ascii_uppercase().to_string() + characters.as_str(),
                None => String::new(),
            }
        })
        .collect()
}

/// Tenants the account may hold.
///
/// CHOSEN for the same reason as [`MAX_CONTACT_LISTS`]: one record per accepted
/// `CreateTenant`, so a long-lived emulator needs a ceiling. `CreateTenant`
/// refuses past it with the `LimitExceededException` its model binds.
pub const MAX_TENANTS: usize = 1_000;

/// Tenant-resource associations the store holds, across every tenant and resource.
///
/// A CHOSEN bound, not an AWS one: an association is created by a request and the
/// resource it is keyed under is a caller-supplied ARN, so a long-lived emulator must
/// not grow without limit while serving them, and the bound is on the WHOLE table —
/// a per-tenant bound would still leave the table growing with the tenant count.
/// `CreateTenantResourceAssociation` refuses past it with the `BadRequestException`
/// its model binds, which is the only limit-shaped error it binds at all.
pub const MAX_TENANT_RESOURCE_ASSOCIATIONS: usize = 10_000;

/// The longest `TenantName` SES accepts: its documentation states the name "can
/// contain up to 64 alphanumeric characters, including letters, numbers, hyphens
/// (-) and underscores (_) only". The model itself binds only the lower bound
/// (length 1..), so the upper bound and the character set are enforced by the
/// handler.
pub const MAX_TENANT_NAME_CHARS: usize = 64;

/// The ARN namespace this service stamps on a resource ARN it mints: SES's ARN
/// namespace is `ses`.
pub const ARN_NAMESPACE: &str = "ses";

/// The region an ARN carries when the request carries no SigV4 credential scope
/// naming one.
pub const DEFAULT_REGION: &str = "us-east-1";

/// `SendingStatus$ENABLED`: "sending is allowed for this entity".
pub const SENDING_STATUS_ENABLED: &str = "ENABLED";

/// `SendingStatus$REINSTATED`: "sending is allowed even if there are active
/// reputation findings".
pub const SENDING_STATUS_REINSTATED: &str = "REINSTATED";

/// `SendingStatus$DISABLED`: "sending is prevented for this entity".
pub const SENDING_STATUS_DISABLED: &str = "DISABLED";

/// Every `SendingStatus` the model binds, for the operations that take one as a
/// plain string and so refuse an unmodelled value themselves.
pub const SENDING_STATUSES: [&str; 3] = [
    SENDING_STATUS_ENABLED,
    SENDING_STATUS_REINSTATED,
    SENDING_STATUS_DISABLED,
];

/// The `SendingStatus` a freshly created tenant carries: sending is allowed for a
/// new tenant, and `ENABLED` is terminal — the clock owes no transition out of it.
pub const TENANT_SENDING_STATUS_CREATED: &str = SENDING_STATUS_ENABLED;

/// The one `ReputationEntityType` the model binds: "currently, only `RESOURCE` type
/// entities are supported, which represent resources in your Amazon SES account that
/// have reputation tracking capabilities".
pub const REPUTATION_ENTITY_TYPE_RESOURCE: &str = "RESOURCE";

/// Every `ReputationEntityFilterKey` the model binds, in the order the enum lists
/// them: a listing filter is a map keyed by one of these.
pub const REPUTATION_ENTITY_FILTER_KEYS: [&str; 4] = [
    "ENTITY_TYPE",
    "REPUTATION_IMPACT",
    "SENDING_STATUS",
    "ENTITY_REFERENCE_PREFIX",
];

/// Every `RecommendationImpact` the model binds, which is what a reputation entity's
/// `ReputationImpact` is one of.
pub const RECOMMENDATION_IMPACTS: [&str; 2] = ["LOW", "HIGH"];

/// `RecommendationStatus$OPEN`: the shortcoming the recommendation reports is still
/// there, so the recommendation still asks for something.
pub const RECOMMENDATION_STATUS_OPEN: &str = "OPEN";

/// `RecommendationStatus$FIXED`: the configuration the recommendation asked for is in
/// place, so the recommendation stands as a record of a shortcoming that was resolved.
pub const RECOMMENDATION_STATUS_FIXED: &str = "FIXED";

/// `RecommendationType$DKIM`: a domain's DKIM setup.
pub const RECOMMENDATION_TYPE_DKIM: &str = "DKIM";

/// `RecommendationType$SPF`: a domain's SPF setup, which for a sender of SES mail is
/// its custom MAIL FROM domain — `PutEmailIdentityMailFromAttributes` is what makes the
/// envelope sender a domain of the account's own, and so what lets that domain's SPF
/// record speak for the mail SES sends for it.
pub const RECOMMENDATION_TYPE_SPF: &str = "SPF";

/// The `RecommendationImpact` every recommendation derived here carries.
///
/// The model states no impact per recommendation type ("the recommendation impact, with
/// values like `HIGH` or `LOW`"), so the value is not read off a per-type table. Both
/// findings derived here are that a domain's own mail authentication is not set up, which
/// is what a receiving provider checks on every message it is offered and what it rejects
/// or spam-folders a message for — the higher of the two modelled impacts.
pub const RECOMMENDATION_IMPACT_HIGH: &str = "HIGH";

/// The `Recommendation$Description` of the DKIM finding: what the recommendation asks the
/// account to do, in SES's own terms for the mechanism (`CreateEmailIdentity` documents
/// the identity's Easy DKIM tokens as "a set of unique strings that you use to create a
/// set of CNAME records that you add to the DNS configuration for your domain").
const DKIM_RECOMMENDATION_DESCRIPTION: &str =
    "Add the DKIM records Amazon SES published for this domain to its DNS configuration, \
     so that receiving mail providers can verify that messages signed for the domain were \
     not altered in transit.";

/// The `Recommendation$Description` of the SPF finding, in the same terms: a custom MAIL
/// FROM domain is what `PutEmailIdentityMailFromAttributes` configures, and SES documents
/// the MX record the caller publishes for it as the record it searches for.
const SPF_RECOMMENDATION_DESCRIPTION: &str =
    "Configure a custom MAIL FROM domain for this domain and publish the MX record Amazon \
     SES asks for, so that the envelope sender of its messages is a domain whose SPF record \
     authorizes Amazon SES to send for it.";

/// What separates the two halves of [`recommendation_key`]: a unit separator, which no
/// ARN and no modelled enum value carries, so the key of one recommendation can never
/// read as the key of another.
const RECOMMENDATION_KEY_SEPARATOR: char = '\u{1f}';

/// The key one recommendation is ordered and paged by.
///
/// A recommendation is identified by the resource it is about and the type of finding it
/// reports — a resource carries at most one finding of each type — so that pair orders the
/// listing deterministically and names a position in it.
pub fn recommendation_key(recommendation: &crate::ops::types::Recommendation) -> String {
    format!(
        "{}{RECOMMENDATION_KEY_SEPARATOR}{}",
        recommendation.resource_arn.as_deref().unwrap_or_default(),
        recommendation.r#type.as_deref().unwrap_or_default()
    )
}

/// The `SendingStatus` in force on an entity whose Amazon-SES-managed status record is
/// unwritten.
///
/// SES writes that record itself, on a reputation finding it raised against the
/// entity; nothing here raises one — no finding is observed or recorded anywhere in
/// this service — so no entity carries such a record, the status Amazon SES permits is
/// this one, and the aggregate is decided by the customer-managed status alone.
pub const AWS_SES_MANAGED_SENDING_STATUS: &str = SENDING_STATUS_ENABLED;

/// The aggregate `SendingStatus` of an entity whose two status records read
/// `customer_managed` and `aws_ses_managed`.
///
/// `ReputationEntity$SendingStatusAggregate` states the rule: "if either the
/// customer-managed status or the Amazon Web Services Amazon SES-managed status is
/// DISABLED, the aggregate status will be DISABLED and the entity will not be allowed
/// to send emails. When the customer-managed status is set to REINSTATED, the entity
/// can continue sending even if there are active reputation findings, provided the
/// Amazon Web Services Amazon SES-managed status also permits sending."
pub fn aggregate_sending_status(customer_managed: &str, aws_ses_managed: &str) -> String {
    let statuses = [customer_managed, aws_ses_managed];
    if statuses.contains(&SENDING_STATUS_DISABLED) {
        return SENDING_STATUS_DISABLED.to_string();
    }
    if statuses.contains(&SENDING_STATUS_REINSTATED) {
        return SENDING_STATUS_REINSTATED.to_string();
    }
    SENDING_STATUS_ENABLED.to_string()
}

/// How this service words the refusal of a reputation entity reference that names no
/// resource this account holds.
///
/// A reputation entity is not created by a request of its own: it exists only for a
/// resource this account already owns, so a reference resolving to nothing is a
/// reference that cannot name an entity AT ALL — invalid input rather than a resource
/// that happens to be gone. Every operation taking a reference therefore refuses it as
/// invalid input, with this one sentence, so a read and a write cannot disagree about
/// what was not found.
///
/// The reference is the only variable part, taken verbatim from the request and
/// delimited with angle brackets the way this service delimits every other resource
/// name it interpolates (see [`configuration_set_not_found`]); the sentence names the
/// entity shape rather than a prose rendering of it, and ends without a period.
pub fn reputation_entity_not_found(reference: &str) -> String {
    format!("ReputationEntity <{reference}> does not exist")
}

/// The account field of an ARN naming a resource Amazon SES itself owns rather than
/// one of the caller's.
///
/// `ReputationEntity$ReputationManagementPolicy` is documented as "an Amazon Web
/// Services Amazon SES-managed policy": the policy belongs to the service, not to the
/// account applying it, so its ARN carries the partition owner in the account field
/// instead of the caller's account id — which is why it is NOT minted through
/// [`::emulator_server::emulator_arn`], the mint for this account's own resources.
const SERVICE_OWNED_ARN_ACCOUNT: &str = "aws";

/// The resource type an Amazon-SES-managed reputation management policy's ARN carries.
const REPUTATION_POLICY_RESOURCE_TYPE: &str = "reputation-policy";

/// The reputation management policy every tracked resource carries until a caller
/// applies another one: the service's own default policy.
pub const DEFAULT_REPUTATION_MANAGEMENT_POLICY: &str = "standard";

/// `arn:aws:ses:<region>:aws:reputation-policy/<name>` — the ARN of the
/// Amazon-SES-managed reputation management policy `policy_name` in `region`.
///
/// The Region is the one the call that reads or writes the policy executes in, the
/// account field is [`SERVICE_OWNED_ARN_ACCOUNT`] because the policy is the service's
/// own, and the resource field names the policy by the name the service knows it
/// under.
pub fn reputation_management_policy_arn(region: &str, policy_name: &str) -> String {
    format!(
        "arn:aws:{ARN_NAMESPACE}:{region}:{SERVICE_OWNED_ARN_ACCOUNT}:\
         {REPUTATION_POLICY_RESOURCE_TYPE}/{policy_name}"
    )
}

/// Every Amazon-SES-managed reputation management policy a caller may apply, in the
/// order `UpdateReputationEntityPolicy` documents them: "reputation management policies
/// are Amazon Web Services Amazon SES-managed (predefined policies). You can select
/// from none, standard, and strict policies."
pub const REPUTATION_MANAGEMENT_POLICIES: [&str; 3] =
    ["none", DEFAULT_REPUTATION_MANAGEMENT_POLICY, "strict"];

/// The name of the Amazon-SES-managed reputation management policy the ARN `policy`
/// names, or `None` when it names no such policy.
///
/// A policy is the SERVICE's, not the account's, so its ARN is one of this service's
/// namespace whose account field is [`SERVICE_OWNED_ARN_ACCOUNT`], whose Region field is
/// a Region label (a policy is selected in the Region the entity lives in) and whose
/// resource field names a [`REPUTATION_POLICY_RESOURCE_TYPE`] by one of the predefined
/// names ([`REPUTATION_MANAGEMENT_POLICIES`]). The ARN itself is read by the shared
/// parse, so what an ARN of this vendor is stays decided in one place.
pub fn reputation_management_policy_name(policy: &str) -> Option<&'static str> {
    let arn = ::service_common::arn::parse_of(policy, ARN_NAMESPACE)?;
    if arn.account != SERVICE_OWNED_ARN_ACCOUNT
        || ::service_common::arn::region_label(arn.region).is_none()
    {
        return None;
    }
    let name = ::service_common::arn::resource_id_of_type(policy, REPUTATION_POLICY_RESOURCE_TYPE)?;
    REPUTATION_MANAGEMENT_POLICIES
        .into_iter()
        .find(|predefined| *predefined == name)
}

/// The fixed sentence SES refuses a reputation-entity write whose
/// `ReputationEntityPolicy` names none of the predefined policies with, stated once for
/// every operation that takes one.
///
/// It carries NO part of the request: the refusal is about the value naming no resource
/// this surface can address — a caller does not author a reputation management policy,
/// it selects one of the Amazon-SES-managed policies ([`REPUTATION_MANAGEMENT_POLICIES`])
/// — exactly as [`UNSUPPORTED_RESOURCE_ARN`] is the fixed sentence for a `ResourceArn`
/// in none of the forms its surface addresses.
pub const INVALID_RESOURCE_IDENTIFIER: &str = "Provided resource identifier is invalid";

/// The account and Region one call is scoped to, and the check that decides which
/// resource ARNs that call may name.
///
/// The account is the one this emulator serves: a handler runs only after the
/// framework has verified the request's signature, so that account is the caller's
/// account, and the Region is the one the request's own SigV4 credential scope names.
/// Both are read from the call rather than from anything the request body carries.
#[derive(Debug, Clone)]
pub struct CallerScope {
    /// The account id an ARN must carry to be one this caller may name.
    pub account_id: &'static str,
    /// The Region an ARN must carry to be reachable from this call.
    pub region: String,
}

impl CallerScope {
    /// The scope the call `parts` belongs to.
    pub fn of(parts: &::http::request::Parts) -> Self {
        Self {
            account_id: ::emulator_server::emulator_account_id(),
            region: request_region(parts),
        }
    }

    /// Whether `arn` names a resource of this service that this call may reach: an
    /// ARN of this service's namespace whose Region is this call's and whose account
    /// is the caller's.
    ///
    /// A reputation entity is reached only by the ARN of the resource it tracks, and
    /// the reputation operations report and manage "reputation entities in your
    /// Amazon SES account in the current Amazon Web Services Region" — so the
    /// account and Region fields of a reference a request carries are checked here,
    /// against the call, before anything resolves it. A reference failing the check
    /// names no entity this call can reach, whatever it names elsewhere, and is
    /// answered exactly as one that was never created, so the two are
    /// indistinguishable to the caller.
    ///
    /// The ARN itself is read by the shared parse, which is where what an ARN of this
    /// vendor is — fields, partition, the resource field's own colons — is decided.
    pub fn covers_resource(&self, arn: &str) -> bool {
        ::service_common::arn::parse_of(arn, ARN_NAMESPACE)
            .is_some_and(|arn| arn.region == self.region && arn.account == self.account_id)
    }
}

/// The `SuppressionListScope` a tenant gets when the request names none:
/// `TenantSuppressionAttributes$SuppressionScope` documents that "if you don't
/// specify a suppression scope, the tenant defaults to ACCOUNT scope and uses the
/// account-level suppression list".
pub const DEFAULT_SUPPRESSION_SCOPE: &str = "ACCOUNT";

/// The `TenantSuppressionAttributes` a write of one tenant's preferences stores, from
/// the members the request carried, or the refusal wording for a value outside the
/// modelled set.
///
/// Shared by every operation that writes a tenant's suppression preferences, so the
/// preferences a tenant ends up with do not depend on which one wrote them: a member
/// the request omits takes its documented default rather than whatever happened to be
/// stored — `TenantSuppressionAttributes$SuppressionScope` "defaults to ACCOUNT scope"
/// when the request names none, and a tenant that names no reason adds nothing to its
/// list automatically. Both enums carry their values as plain strings here, so the
/// value set is the caller's to enforce; the wording is the service's shared one (see
/// [`modelled_enum_value`]).
///
/// Resolving the members is not the same as HOLDING them: a tenant holds preferences
/// only once a request states them, which is what the caller decides by calling this
/// at all (see [`TenantSuppressionPreferences`]).
pub fn tenant_suppression_preferences(
    suppressed_reasons: Option<Vec<String>>,
    suppression_scope: Option<String>,
) -> Result<TenantSuppressionPreferences, String> {
    let reasons = suppressed_reasons
        .unwrap_or_default()
        .into_iter()
        .map(|reason| {
            modelled_enum_value(
                "SuppressedReasons",
                reason,
                crate::ops::types::SUPPRESSION_LIST_REASON_VALUES,
            )
        })
        .collect::<Result<Vec<_>, _>>()?;
    let scope = match suppression_scope {
        Some(scope) => modelled_enum_value(
            "SuppressionScope",
            scope,
            crate::ops::types::SUPPRESSION_LIST_SCOPE_VALUES,
        )?,
        None => DEFAULT_SUPPRESSION_SCOPE.to_string(),
    };
    Ok(TenantSuppressionPreferences {
        suppressed_reasons: reasons,
        suppression_scope: scope,
    })
}

/// The `SuppressionOptions` a write of ONE configuration set's suppression list
/// preferences stores, from the members the request carried, or the refusal wording
/// for a value outside a modelled value set.
///
/// Shared by every operation that writes a configuration set's suppression
/// preferences, so the preferences a set ends up with do not depend on which one
/// wrote them. Unlike an account's or a tenant's, a configuration set's
/// `SuppressionOptions` is an OVERRIDE of the account's or the tenant's — the scope
/// member "overrides the tenant or account suppression scope for emails sent using
/// this configuration set" — so a member the request does not state is stored ABSENT
/// rather than defaulted, leaving the set following the preferences it did not
/// override. Every enum inside carries its value as a plain string, so the value set
/// is enforced here, with the service's shared wording ([`modelled_enum_value`]).
pub fn configuration_set_suppression_options(
    options: crate::ops::types::SuppressionOptions,
) -> Result<crate::ops::types::SuppressionOptions, String> {
    let suppressed_reasons = match options.suppressed_reasons {
        Some(reasons) => Some(
            reasons
                .into_iter()
                .map(|reason| {
                    modelled_enum_value(
                        "SuppressedReasons",
                        reason,
                        crate::ops::types::SUPPRESSION_LIST_REASON_VALUES,
                    )
                })
                .collect::<Result<Vec<_>, _>>()?,
        ),
        None => None,
    };
    let suppression_scope = match options.suppression_scope {
        Some(scope) => Some(modelled_enum_value(
            "SuppressionScope",
            scope,
            crate::ops::types::SUPPRESSION_LIST_SCOPE_VALUES,
        )?),
        None => None,
    };
    let validation_options = match options.validation_options {
        Some(validation) => Some(crate::ops::types::SuppressionValidationOptions {
            condition_threshold: suppression_condition_threshold(validation.condition_threshold)?,
        }),
        None => None,
    };
    Ok(crate::ops::types::SuppressionOptions {
        suppressed_reasons,
        suppression_scope,
        validation_options,
    })
}

/// The `SuppressionConditionThreshold` a write of suppression validation settings
/// stores, from the members the request carried, or the refusal wording for a value
/// outside a modelled value set.
///
/// The same structure carries the validation settings of an account and of a
/// configuration set, so both read it here: the switch is required and the confidence
/// threshold is stored only where the request carried one.
pub fn suppression_condition_threshold(
    threshold: crate::ops::types::SuppressionConditionThreshold,
) -> Result<crate::ops::types::SuppressionConditionThreshold, String> {
    let condition_threshold_enabled = modelled_enum_value(
        "ConditionThresholdEnabled",
        threshold.condition_threshold_enabled,
        crate::ops::types::FEATURE_STATUS_VALUES,
    )?;
    let overall_confidence_threshold = match threshold.overall_confidence_threshold {
        Some(confidence) => Some(crate::ops::types::SuppressionConfidenceThreshold {
            confidence_verdict_threshold: modelled_enum_value(
                "ConfidenceVerdictThreshold",
                confidence.confidence_verdict_threshold,
                crate::ops::types::SUPPRESSION_CONFIDENCE_VERDICT_THRESHOLD_VALUES,
            )?,
        }),
        None => None,
    };
    Ok(crate::ops::types::SuppressionConditionThreshold {
        condition_threshold_enabled,
        overall_confidence_threshold,
    })
}

/// The AWS-Region this request executes in, which is also the region to stamp into
/// the ARNs it mints: the SigV4 credential scope's region, else [`DEFAULT_REGION`].
///
/// A multi-region endpoint's primary region is "the AWS-Region where the operation is
/// executed", and only endpoints whose primary region is the Region a call executes in
/// are visible to it, so every multi-region-endpoint operation resolves the Region
/// here too.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// How a send refuses a sending identity this account has not verified, for every
/// operation that reports a failed verification check.
///
/// SES identities are per Region ([`request_region`]), so the refusal names the Region
/// whose identity list the check failed in — upper-cased, as the service writes it — and
/// then EVERY identity that failed it, comma-separated, in the order the check consulted
/// them. Each is named by the mailbox its value names ([`addr_spec`]), since that is the
/// identity the request asked SES to use. One wording, so two send paths cannot disagree
/// about how the same failed check reads.
pub fn unverified_identities_message(region: &str, identities: &[&str]) -> String {
    let failed: Vec<&str> = identities.iter().map(|identity| addr_spec(identity)).collect();
    format!(
        "Email address is not verified. The following identities failed the check in \
         region {}: {}",
        region.to_uppercase(),
        failed.join(", ")
    )
}

/// `arn:aws:ses:<region>:<account>:tenant/<name>/<id>` — the ARN SES reports for a
/// tenant: the resource part names the tenant AND its id, so the ARN identifies
/// the tenant record even after a tenant of the same name is recreated. Minted
/// once at create and read back from the record afterwards.
pub fn tenant_arn(region: &str, tenant_name: &str, tenant_id: &str) -> String {
    ::emulator_server::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("tenant/{tenant_name}/{tenant_id}"),
    )
}

/// `arn:aws:ses:<region>:<account>:identity/<identity>` — the ARN of an email
/// identity, which is how a refusal that must name the resource rather than the
/// request's spelling of it names one.
pub fn identity_arn(region: &str, identity: &str) -> String {
    ::emulator_server::emulator_arn(ARN_NAMESPACE, region, &format!("identity/{identity}"))
}

/// `arn:aws:ses:<region>:<account>:configuration-set/<name>` — the ARN of a
/// configuration set, which is how this service names the set a cross-service request
/// of its own came FROM ([`event_publication`]).
///
/// `configuration-set/<name>` is the resource form SES addresses a configuration set
/// by ([`TENANT_RESOURCE_TYPES`]), so the ARN minted here and the one the tagging and
/// tenant surfaces read are the same value.
pub fn configuration_set_arn(region: &str, configuration_set_name: &str) -> String {
    ::emulator_server::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("configuration-set/{configuration_set_name}"),
    )
}

/// Whether `tenant_name` satisfies the documented `TenantName` form: at most
/// [`MAX_TENANT_NAME_CHARS`] characters, each a letter, a digit, `-` or `_`.
pub fn tenant_name_is_well_formed(tenant_name: &str) -> bool {
    let length = tenant_name.chars().count();
    length > 0
        && length <= MAX_TENANT_NAME_CHARS
        && tenant_name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// How this service words the refusal of a `name` SES does not accept for a resource of
/// `kind`, stated once for every resource whose name carries the one character rule.
///
/// SES states the rejected name between angle brackets and then the characters it admits:
/// `Invalid <kind> name <<name>>: only alphanumeric ASCII characters, '_', and '-' are
/// allowed.` The name as the REQUEST spelled it is the only variable part beside the kind,
/// so a caller can read the value it sent back out from between the brackets. The member's
/// documented LENGTH bound is part of the same check and is refused in the same sentence,
/// since a name outside it is a name SES does not accept either.
pub fn invalid_resource_name(kind: &str, name: &str) -> String {
    format!(
        "Invalid {kind} name <{name}>: only alphanumeric ASCII characters, '_', and '-' \
         are allowed."
    )
}

/// How this service words `BadRequestException` for a `TenantName` that does not
/// satisfy the documented form, stated once for every operation that takes one — the
/// write path (`CreateTenant`) and the read path (`GetTenant`), which validates the
/// name before it looks a tenant up, alike.
pub fn invalid_tenant_name(tenant_name: &str) -> String {
    invalid_resource_name(TENANT_RESOURCE_KIND, tenant_name)
}

/// The resource kind a tenant's name is refused as, as SES names it in that sentence.
const TENANT_RESOURCE_KIND: &str = "tenant";

/// How this service words `NotFoundException` for a tenant the account does not hold.
///
/// ONE sentence for every operation that names a tenant it cannot resolve — the tenant
/// itself (`GetTenant`, `DeleteTenant`), a tenant's suppression list
/// (`GetSuppressedDestination`, `ListSuppressedDestinations`,
/// `PutTenantSuppressionAttributes`) and the tenant-resource association surface
/// (`CreateTenantResourceAssociation`, `DeleteTenantResourceAssociation`,
/// `ListTenantResources`) — so a client sees the same sentence whichever of them first
/// tells it the tenant is not there, and two views of one deleted tenant cannot
/// disagree about how it is gone.
///
/// The fixed text is `The requested tenant <` + `> does not exist.`; the requested name,
/// taken from the request, is the only variable part.
pub fn tenant_not_found(tenant_name: &str) -> String {
    format!("The requested tenant <{tenant_name}> does not exist.")
}

/// How this service words `AlreadyExistsException` for a tenant name already taken.
///
/// A tenant name is unique within ONE account, so the refusal names both the name the
/// request asked for and the account it is already taken in — the fact that tells a
/// caller holding credentials for several accounts where the collision is. Both are
/// variable parts: the name as the request spelled it, and the caller's account id
/// from the call's own scope ([`CallerScope`]).
pub fn tenant_already_exists(tenant_name: &str, account_id: &str) -> String {
    format!("Tenant with name {tenant_name} already exists in account {account_id}")
}

/// How this service words `AlreadyExistsException` for a resource already associated
/// with the named tenant, stated once for every operation that creates an
/// association. The resource ARN and the tenant name are the only variable parts,
/// both taken from the request.
pub fn tenant_resource_association_exists(tenant_name: &str, resource_arn: &str) -> String {
    format!("Resources {resource_arn} has already been associated with tenant {tenant_name}")
}

/// The resource kinds a tenant association may name, as (the resource-type token the
/// resource's own SES ARN carries, the modelled `ResourceType` value that kind is
/// reported as).
///
/// `ResourceType` binds exactly these three values —
/// `smithy_query(query="shape",shape="ResourceType")`: `EMAIL_IDENTITY` ("an email
/// address or domain that you use to send email"), `CONFIGURATION_SET` ("a set of
/// rules that you can apply to emails you send") and `EMAIL_TEMPLATE` ("a template
/// that defines the content of an email message") — and SES addresses each of them
/// with `identity/<identity>`, `configuration-set/<name>` and `template/<name>` in
/// the resource field of its ARN.
pub const TENANT_RESOURCE_TYPES: [(&str, &str); 3] = [
    ("identity", "EMAIL_IDENTITY"),
    ("configuration-set", "CONFIGURATION_SET"),
    ("template", "EMAIL_TEMPLATE"),
];

/// The modelled `ResourceType` the ARN `resource_arn` names, or `None` when it names
/// no kind of resource a tenant may hold.
///
/// `CreateTenantResourceAssociation` documents that "resources can be email
/// identities, configuration sets, or email templates", so a value naming anything
/// else — another vendor's resource, an SES resource of another kind, or no ARN at
/// all — addresses nothing this surface can associate. Which values name a resource
/// of this service at all is read by [`ses_resource_of`], so this surface and the
/// tagging surface cannot disagree about it; the kinds a tenant may hold are the
/// narrowing this surface adds.
pub fn tenant_resource_type(resource_arn: &str) -> Option<&'static str> {
    let (resource_type, _) = ses_resource_of(resource_arn)?;
    TENANT_RESOURCE_TYPES
        .iter()
        .find(|(token, _)| resource_type == *token)
        .map(|(_, modelled)| *modelled)
}

/// How this service words `BadRequestException` for a caller-supplied `ResourceArn`
/// in none of the resource-ARN forms the surface reading it addresses: the
/// tenant-resource surface for an ARN naming no resource kind a tenant may hold, the
/// tagging surface for a value that is no SES resource ARN, or one whose resource
/// field names no resource at all.
///
/// The sentence is fixed text with no variable part: the refusal is about the FORM of
/// the value — SES reads the ARN against the resource-ARN forms it addresses, and a
/// value matching none of them (another vendor's resource, an SES resource of a kind
/// the surface cannot address, or no ARN at all) names nothing it can act on. Stated
/// once, so two surfaces cannot word the one condition differently.
pub const UNSUPPORTED_RESOURCE_ARN: &str = "Provided ARN is not in SES resource ARN format";

/// How the tenant-resource association surface words `NotFoundException` for a
/// `ResourceArn` naming a resource of a kind a tenant may hold that the account does
/// not hold in this Region; `None` when the ARN names no such kind at all, which is
/// the malformed-input condition [`tenant_resource_type`] reads and
/// [`UNSUPPORTED_RESOURCE_ARN`] words.
///
/// The resource is named the way the rest of this API names a missing resource of that
/// KIND — the kind's label and the id its ARN addresses it under, in angle brackets —
/// so a caller reads which resource was not found rather than the whole ARN it passed.
/// The sentence is the one this surface is RECORDED answering a removal whose
/// `ResourceArn` names a configuration set the account does not hold: the fixed text
/// `Configuration set <` + `> does not exist:`, ending in a colon rather than a period,
/// with the configuration-set name parsed out of the ARN as its only variable part. The
/// other two kinds a tenant may hold ([`TENANT_RESOURCE_TYPES`]) follow that template
/// with their own label. The per-kind reads of the same condition elsewhere end the
/// sentence in a period instead, which is the wording THEY are recorded with; see
/// [`configuration_set_not_found`] and [`email_identity_not_found`].
pub fn tenant_resource_not_found(resource_arn: &str) -> Option<String> {
    let (resource_type, resource_id) = ses_resource_of(resource_arn)?;
    let label = match resource_type {
        "identity" => "Email identity",
        "configuration-set" => "Configuration set",
        "template" => "Email template",
        // Any other kind is one no tenant may hold, so the ARN is malformed input to
        // this surface rather than a resource that happens to be missing.
        _ => return None,
    };
    Some(format!("{label} <{resource_id}> does not exist:"))
}

/// The keys `ListTenantResourcesFilterKey` names: "currently, the only supported
/// filter key is RESOURCE_TYPE". The filter arrives as a map of plain strings, so
/// the handler is what refuses a key the enum does not name.
pub const LIST_TENANT_RESOURCES_FILTER_KEYS: [&str; 1] = ["RESOURCE_TYPE"];

/// Every `ListTenantsFilterKey` the model binds, in the order the enum lists them:
/// "TENANT_NAME_CONTAINS — filter by a substring of the tenant name" and
/// "SENDING_STATUS — filter by sending status". The filter arrives as a map of plain
/// strings, so the handler is what refuses a key the enum does not name.
pub const LIST_TENANTS_FILTER_KEYS: [&str; 2] = ["TENANT_NAME_CONTAINS", "SENDING_STATUS"];

/// Configuration sets the account may hold: SES's documented quota is 10,000
/// configuration sets per account per Region, and `CreateConfigurationSet` refuses
/// past it with the `LimitExceededException` its model binds.
pub const MAX_CONFIGURATION_SETS: usize = 10_000;

/// The longest a `ConfigurationSetName` may be, from the member's own
/// documentation: "The name can contain up to 64 alphanumeric characters, including
/// letters, numbers, hyphens (-) and underscores (_) only."
pub const MAX_CONFIGURATION_SET_NAME_LEN: usize = 64;

/// Event destinations one configuration set may hold.
///
/// A CHOSEN bound, not an AWS one: a destination is created by a request, so a
/// long-lived emulator must not grow without limit while serving them. It is far
/// above what a client publishing a set's events to its destinations needs, and
/// `CreateConfigurationSetEventDestination` refuses past it with the
/// `LimitExceededException` its model binds.
pub const MAX_EVENT_DESTINATIONS_PER_SET: usize = 1_000;

/// Dedicated IP pools the account may hold: SES's documented quota is 50 dedicated IP
/// pools per account per Region, and `CreateDedicatedIpPool` refuses past it with the
/// `LimitExceededException` its model binds.
pub const MAX_DEDICATED_IP_POOLS: usize = 50;

/// `ScalingMode$STANDARD`: "a dedicated IP pool where you can control which IPs are part
/// of the pool" (`DedicatedIpPool$ScalingMode`'s own documentation).
pub const SCALING_MODE_STANDARD: &str = "STANDARD";

/// `ScalingMode$MANAGED`: "a dedicated IP pool where the reputation and number of IPs are
/// automatically managed by Amazon SES" (`DedicatedIpPool$ScalingMode`'s own
/// documentation).
pub const SCALING_MODE_MANAGED: &str = "MANAGED";

/// Per-service state for com-amazonaws-sesv2-simpleemailservice-v2. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsSesv2SimpleemailserviceV2State {
    /// Email identities, keyed by [`identity_key`] of the `EmailIdentity` they
    /// were created with. Ordered, so `ListEmailIdentities` enumerates and pages
    /// deterministically.
    pub identities: BTreeMap<String, EmailIdentityRecord>,
    /// Tenants, keyed by the `TenantName` they were created with — an opaque name
    /// the API echoes back as given, so it is keyed exactly as supplied. Ordered,
    /// so `ListTenants` enumerates and pages deterministically.
    pub tenants: BTreeMap<String, TenantRecord>,
    /// Monotonic counter feeding [`Self::next_tenant_id`], so two tenants created
    /// in the same millisecond still get different ids.
    pub tenant_sequence: u64,
    /// Tenant-resource associations, keyed by the resource's `ResourceArn` and then
    /// the `TenantName` it is associated with: one resource may be associated with
    /// several tenants and one tenant with several resources, so the pair is the
    /// identity of an association. Ordered, so `ListResourceTenants` enumerates and
    /// pages one resource's tenants, and `ListTenantResources` one tenant's
    /// resources, deterministically.
    pub tenant_resource_associations: BTreeMap<(String, String), TenantResourceAssociationRecord>,
    /// Contact lists, keyed by the `ContactListName` they were created with — an
    /// opaque name the API echoes back as given, so it is keyed exactly as supplied.
    /// Ordered, so `ListContactLists` enumerates and pages deterministically.
    pub contact_lists: BTreeMap<String, ContactListRecord>,
    /// Email templates, keyed by the `TemplateName` they were created with — the name
    /// a send refers to and the API echoes back as given, so it is keyed exactly as
    /// supplied. Ordered, so `ListEmailTemplates` enumerates and pages
    /// deterministically. Bounded by [`MAX_EMAIL_TEMPLATES`].
    pub email_templates: BTreeMap<String, EmailTemplateRecord>,
    /// Multi-region endpoints, keyed by their primary region and then by the
    /// `EndpointName` they were created with. An endpoint is managed only from its
    /// primary region, so the region is part of the key: the same name in another
    /// region names another resource. Ordered, so `ListMultiRegionEndpoints`
    /// enumerates and pages deterministically within one region.
    pub multi_region_endpoints: BTreeMap<(String, String), MultiRegionEndpointRecord>,
    /// Configuration sets, keyed by the `ConfigurationSetName` they were created
    /// with. The name is an identifier the API echoes back as given and compares
    /// exactly, so it is keyed exactly as supplied. Ordered, so
    /// `ListConfigurationSets` enumerates and pages deterministically.
    pub configuration_sets: BTreeMap<String, ConfigurationSetRecord>,
    /// Dedicated IP pools, keyed by the `PoolName` they were created with. The name
    /// is the identity of a pool — every pool operation binds it as a path label and
    /// `ListDedicatedIpPools` reports it — so it is keyed exactly as supplied.
    /// Ordered, so `ListDedicatedIpPools` enumerates and pages deterministically.
    /// Bounded by [`MAX_DEDICATED_IP_POOLS`], which `CreateDedicatedIpPool` enforces.
    pub dedicated_ip_pools: BTreeMap<String, DedicatedIpPoolRecord>,
    /// Addresses on a suppression list, keyed by [`suppression_key`]: the list the
    /// entry belongs to (a tenant's, or the account's) and the address within it, so
    /// a tenant's list and the account-level list are separate. Ordered, so
    /// `ListSuppressedDestinations` enumerates and pages deterministically. Bounded
    /// by [`MAX_SUPPRESSED_DESTINATIONS`], which [`Self::suppress_destination`]
    /// enforces.
    pub suppressed_destinations: BTreeMap<String, SuppressedDestinationRecord>,
    /// The dedicated IP addresses the account holds, keyed by the address itself —
    /// the only identity a dedicated IP has. Ordered, so `GetDedicatedIps`
    /// enumerates and pages deterministically.
    pub dedicated_ips: BTreeMap<String, DedicatedIpRecord>,
    /// The blocklist listings observed for the account's dedicated IP addresses,
    /// keyed by the `BlacklistItemName` they were observed for — the address itself,
    /// the only name a listing is looked up by. Ordered, so a report of several
    /// addresses is assembled deterministically. No request mints a listing: a
    /// third-party blocklist operator lists an address and SES observes that listing
    /// out of band, exactly as a dedicated IP itself is allocated out of band
    /// ([`Self::dedicated_ips`]), so `GetBlacklistReports` only reads this table.
    pub blacklist_listings: BTreeMap<String, Vec<BlacklistListingRecord>>,
    /// Export jobs, keyed by the `JobId` minted for them — the only name an export
    /// job has, and the one every export-job operation addresses it by. Ordered, so
    /// the table enumerates deterministically whatever order the jobs were created
    /// in; the order a listing reports them in is
    /// [`ExportJobRecord::listing_key`]'s.
    ///
    /// Bounded by [`MAX_EXPORT_JOBS`], which `CreateExportJob` enforces on the one
    /// path that inserts here, and the file a completed run retains is bounded in
    /// turn by [`export_file::MAX_EXPORT_FILE_RECORDS`] rows read under
    /// [`export_file::MAX_EXPORT_FILE_VISITS`] visits — so neither the table nor one
    /// record grows with what a caller asks for.
    pub export_jobs: BTreeMap<String, ExportJobRecord>,
    /// Import jobs, keyed by the `JobId` minted for them — the only name an import job
    /// has, and the one every import-job operation addresses it by. Ordered, so the
    /// table enumerates deterministically whatever order the jobs were created in.
    /// Bounded by [`MAX_IMPORT_JOBS`], which `CreateImportJob` enforces.
    pub import_jobs: BTreeMap<String, ImportJobRecord>,
    /// The outbound-message log, oldest first: one entry per accepted `SendEmail`.
    pub sent_messages: Vec<SentMessage>,
    /// Per-account send counters, bumped by `SendEmail`.
    pub send_counters: SendQuotaState,
    /// The account-level attributes of this account in this Region: the sending
    /// and dedicated-IP switches, the reputation status, the suppression, VDM and
    /// pricing attributes and the account details, as the account operations left
    /// them.
    pub account: AccountState,
    /// Monotonic counter feeding [`Self::next_message_id`], so two sends in the
    /// same microsecond still get different message ids.
    pub message_sequence: u64,
    /// Monotonic counter feeding [`Self::next_endpoint_id`], so two endpoints
    /// created in the same microsecond still get different ids.
    pub endpoint_sequence: u64,
    /// Monotonic counter feeding [`Self::next_export_job_id`], so two export jobs
    /// created in the same microsecond still get different ids.
    pub export_job_sequence: u64,
    /// Monotonic counter feeding [`Self::next_import_job_id`], so two import jobs
    /// created in the same microsecond still get different ids.
    pub import_job_sequence: u64,
}

impl ComAmazonawsSesv2SimpleemailserviceV2State {
    /// Materialise every transition the clock already owes at `now`: an identity's
    /// verification, an endpoint's provisioning, the completion of an endpoint's
    /// deletion and the progress of an export job's run.
    ///
    /// A reporting operation may call this: the transition was due at `now` whether
    /// or not anybody called, so applying it changes nothing a later caller would
    /// otherwise have seen.
    pub fn settle(&mut self, now: SystemTime) {
        for identity in self.identities.values_mut() {
            identity.settle(now);
        }
        for endpoint in self.multi_region_endpoints.values_mut() {
            endpoint.settle(now);
        }
        // An export job's run writes its file as it completes, from the records its
        // data source selects as they stood when the run fell due — so the file is
        // written once, and every later download of the job serves those bytes. At most
        // `MAX_EXPORT_FILES_PER_CALL` are written here, so one call cannot be made to
        // pay for every pending run; the rest stay owed, and the next call writes them.
        let completing: Vec<String> = self
            .export_jobs
            .iter()
            .filter(|(_, job)| job.completion_falls_due(now))
            .map(|(job_id, _)| job_id.clone())
            .take(MAX_EXPORT_FILES_PER_CALL)
            .collect();
        for job_id in completing {
            let Some(job) = self.export_jobs.get(&job_id).cloned() else {
                continue;
            };
            let file = export_file::render(self, &job);
            if let Some(stored) = self.export_jobs.get_mut(&job_id) {
                stored.file = Some(file);
            }
        }
        // A run whose file has not been written is NOT complete: the status moves only
        // where the file it reports is there to be downloaded.
        for job in self.export_jobs.values_mut() {
            job.settle(now);
        }
        // A deletion that was due at `now` has completed, whether or not anybody
        // called, so the endpoint it was tearing down — and its name — are gone.
        self.multi_region_endpoints
            .retain(|_, endpoint| !endpoint.deletion_complete(now));
        // `sent_last_24h` is a view of the log over the trailing window, not an
        // independent tally: recompute it here so it cannot drift as the window
        // moves past entries that no longer count.
        let charged = self.recipients_in_quota_window(now);
        self.send_counters.sent_last_24h = charged;
        self.account.settle(now);
    }

    /// Every reputation entity reachable by the call `scope` describes, ordered by
    /// `ReputationEntityReference`.
    ///
    /// No operation creates a reputation entity: one exists for each resource whose
    /// reputation SES tracks and whose sending it can stop, referenced by that
    /// resource's ARN. The table is therefore DERIVED from the resource records on
    /// every read instead of being kept beside them, and a resource's own record
    /// stays the single place its status lives.
    ///
    /// Which resources those are is decided by the one member that carries an
    /// entity's status: a reputation entity's `CustomerManagedStatus` is a
    /// `SendingStatus`, and `Tenant$SendingStatus` is the only resource member in
    /// this service's model that targets that shape — the status the reputation
    /// writes move. A configuration set's `SendingOptions$SendingEnabled` and
    /// `ReputationOptions$ReputationMetricsEnabled` are a boolean switch and a
    /// metrics flag the configuration-set operations own, not a `SendingStatus` a
    /// reputation entity could report or a reputation write could move, and an email
    /// identity carries no sending status at all; so neither is a tracked resource
    /// here, and the derivation spans the tenants.
    ///
    /// The scope is applied here, so a read and a write of one call cannot disagree
    /// about which entities it reaches; see [`CallerScope::covers_resource`].
    pub fn reputation_entities(
        &self,
        scope: &CallerScope,
    ) -> Vec<crate::ops::types::ReputationEntity> {
        // "The reputation impact ... representing the highest impact reputation finding
        // currently active. Reputation findings can be retrieved using the
        // `ListRecommendations` operation": the findings every entity's impact is read
        // off are the account's own, derived once for the whole table so a listing and a
        // read of one entity cannot disagree about which findings were active.
        let findings = self.recommendations(scope);
        let mut entities: Vec<crate::ops::types::ReputationEntity> = self
            .tenants
            .values()
            .filter(|tenant| scope.covers_resource(&tenant.arn))
            .map(|tenant| tenant.reputation_entity(&findings))
            .collect();
        // Ordered by reference, so a listing of them enumerates and pages
        // deterministically whatever order the resources were created in.
        entities.sort_by(|left, right| {
            left.reputation_entity_reference
                .cmp(&right.reputation_entity_reference)
        });
        entities
    }

    /// Every recommendation the account's own records yield, reachable by the call
    /// `scope` describes, ordered by [`recommendation_key`].
    ///
    /// No operation creates a recommendation: SES raises one for a resource whose mail
    /// configuration it finds wanting, and marks it `FIXED` once the configuration it
    /// asked for is in place. The table is therefore DERIVED from the resource records
    /// on every read instead of being kept beside them, and each resource's own record
    /// stays the single place its configuration lives — the same derivation
    /// [`Self::reputation_entities`] makes for the entities of tracked resources.
    ///
    /// The resource a recommendation is about is an email identity
    /// (`Recommendation$ResourceArn` is documented with an `identity/…` ARN), so the
    /// identity table is what the findings are read off, and the ARN each carries is
    /// minted in the Region the call executes in. Reporting only: the derivation reads
    /// the records and writes nothing.
    pub fn recommendations(&self, scope: &CallerScope) -> Vec<crate::ops::types::Recommendation> {
        let mut recommendations: Vec<crate::ops::types::Recommendation> = self
            .identities_in(&scope.region)
            .flat_map(|(_, identity)| identity.recommendations(&scope.region))
            .collect();
        recommendations.sort_by_cached_key(recommendation_key);
        recommendations
    }

    /// The blocklist listings observed for the blacklist item `item_name` names, as
    /// the modelled `BlacklistEntry` list a report answers with; `None` when none has
    /// been observed for it.
    ///
    /// A value nothing has been observed for is not an error of any kind: the item
    /// name is a plain string the caller supplies and a report says what was found
    /// for it, so a name that is not one of the account's listed addresses — one of
    /// its dedicated IPs that no blocklist lists, an address it does not hold, or a
    /// value that is no address at all — simply has nothing to report.
    ///
    /// Ordered by when the listing happened and then by the blocklist's name, so one
    /// address's listings are reported in the same order on every read whatever order
    /// the observations arrived in. Reporting only: the derivation reads the table and
    /// writes nothing.
    pub fn blacklist_entries(
        &self,
        item_name: &str,
    ) -> Option<Vec<crate::ops::types::BlacklistEntry>> {
        let listings = self.blacklist_listings.get(item_name)?;
        if listings.is_empty() {
            return None;
        }
        let mut ordered: Vec<&BlacklistListingRecord> = listings.iter().collect();
        ordered.sort_by(|left, right| {
            left.listing_time
                .cmp(&right.listing_time)
                .then_with(|| left.rbl_name.cmp(&right.rbl_name))
        });
        Some(
            ordered
                .into_iter()
                .map(BlacklistListingRecord::entry)
                .collect(),
        )
    }

    /// The tenant a reputation entity reference names, for a write that has to
    /// change its status: the reference of a resource-type entity is that resource's
    /// ARN, which a tenant record carries from the moment it was created.
    ///
    /// `None` unless the reference is one the call `scope` describes may reach — the
    /// same scope [`Self::reputation_entities`] reports over, so a write can only
    /// reach an entity a listing of the same call would have reported.
    pub fn tenant_by_reputation_entity_reference(
        &mut self,
        reference: &str,
        scope: &CallerScope,
    ) -> Option<&mut TenantRecord> {
        if !scope.covers_resource(reference) {
            return None;
        }
        self.tenants
            .values_mut()
            .find(|tenant| tenant.arn == reference)
    }

    /// The tags stored on the resource `resource_arn` names, as the modelled `TagList`
    /// the tagging surface reports them; `None` when the call `scope` describes holds
    /// no such resource.
    ///
    /// SES writes each of its resources in the resource field of its ARN as the
    /// resource TYPE and then the id the resource is held under — an email identity as
    /// `identity/<identity>` (the form [`identity_arn`] mints), a contact list as
    /// `contact-list/<name>`, a configuration set as `configuration-set/<name>`, an
    /// email template as `template/<name>` (the form [`email_template_arn`] mints) and a
    /// tenant as `tenant/<name>/<id>` (the form [`tenant_arn`] mints) — so the type
    /// picks the table and the id picks the record within it. An identity resolves
    /// through [`identity_key`], so an ARN spelling the domain part in another case
    /// names the same identity the rest of the API does. A tenant's id is part of its
    /// ARN, so the whole value is compared against the ARN the tenant was minted with,
    /// which keeps a recreated tenant of the same name from answering for its
    /// predecessor.
    ///
    /// An ARN naming a resource kind this account holds no record of — a kind nothing
    /// creates here yet, or an id never created — resolves to nothing, exactly as a
    /// name that was never created: the two are indistinguishable to the caller.
    ///
    /// The scope is applied first, so an ARN of another account or another Region
    /// reaches nothing whatever it names elsewhere; see
    /// [`CallerScope::covers_resource`]. Reporting only: the resolution reads the
    /// tables and writes nothing, so a read leaves every later caller seeing exactly
    /// what it saw.
    pub fn resource_tags(
        &self,
        resource_arn: &str,
        scope: &CallerScope,
    ) -> Option<Vec<crate::ops::types::Tag>> {
        if !scope.covers_resource(resource_arn) {
            return None;
        }
        let (resource_type, resource_id) = ses_resource_of(resource_arn)?;
        match resource_type {
            // An identity is an identity OF the Region the call executes in, like
            // every other resource an ARN of this service names.
            "identity" => self
                .identity_in(&scope.region, resource_id)
                .map(EmailIdentityRecord::tag_list),
            "contact-list" => self
                .contact_lists
                .get(resource_id)
                .map(ContactListRecord::tag_list),
            "configuration-set" => self
                .configuration_sets
                .get(resource_id)
                .map(ConfigurationSetRecord::tag_list),
            // An email template belongs to the ACCOUNT, so the table this resolves
            // against is the account's template view — the one every template operation
            // of this surface reads, reconciled with the account's registry before a
            // handler reaches this store, so a template created through either SES
            // surface resolves here.
            "template" => self
                .email_templates
                .get(resource_id)
                .map(EmailTemplateRecord::tag_list),
            "tenant" => self
                .tenants
                .values()
                .find(|tenant| tenant.arn == resource_arn)
                .map(TenantRecord::tag_list),
            "dedicated-ip-pool" => self
                .dedicated_ip_pools
                .get(resource_id)
                .map(DedicatedIpPoolRecord::tag_list),
            _ => None,
        }
    }

    /// Whether the call `scope` describes holds the resource `resource_arn` names.
    ///
    /// Resolution is [`Self::resource_tags`]': that is the one place an ARN this
    /// service addresses is read against the tables holding its records, so "the
    /// account holds this resource" means the same to every surface that resolves a
    /// caller-supplied ARN — the tags are simply the view that read reports, and a
    /// resource carrying none still resolves (to an empty list). A kind this account
    /// keeps no table for yet resolves to nothing, exactly as an id that was never
    /// created: the two are indistinguishable to the caller.
    ///
    /// Reporting only: the resolution reads the tables and writes nothing.
    pub fn holds_resource(&self, resource_arn: &str, scope: &CallerScope) -> bool {
        self.resource_tags(resource_arn, scope).is_some()
    }

    /// The tags STORED on the resource `resource_arn` names, for a write that has to
    /// change them; `None` when the call `scope` describes holds no such resource.
    ///
    /// The ARN is resolved exactly as [`Self::resource_tags`] resolves one — the same
    /// scope check, the same resource types, the same per-type keying — so a write can
    /// only reach a resource a read of the same ARN would have reported, and the tags
    /// it changes are the ones that read reports. The pairs are handed back in the
    /// order they were supplied, which is the order the tagging surface reports them
    /// in, so a caller that replaces one tag's value does not see the rest reorder.
    pub fn resource_tags_mut(
        &mut self,
        resource_arn: &str,
        scope: &CallerScope,
    ) -> Option<&mut Vec<(String, String)>> {
        if !scope.covers_resource(resource_arn) {
            return None;
        }
        let (resource_type, resource_id) = ses_resource_of(resource_arn)?;
        match resource_type {
            "identity" => self
                .identity_in_mut(&scope.region, resource_id)
                .map(|identity| &mut identity.tags),
            "contact-list" => self
                .contact_lists
                .get_mut(resource_id)
                .map(|contact_list| &mut contact_list.tags),
            "configuration-set" => self
                .configuration_sets
                .get_mut(resource_id)
                .map(|configuration_set| &mut configuration_set.tags),
            "template" => self
                .email_templates
                .get_mut(resource_id)
                .map(|template| &mut template.tags),
            "tenant" => self
                .tenants
                .values_mut()
                .find(|tenant| tenant.arn == resource_arn)
                .map(|tenant| &mut tenant.tags),
            "dedicated-ip-pool" => self
                .dedicated_ip_pools
                .get_mut(resource_id)
                .map(|pool| &mut pool.tags),
            _ => None,
        }
    }

    /// Put `record` on the suppression list `key` names, refusing a NEW entry once
    /// the table holds [`MAX_SUPPRESSED_DESTINATIONS`].
    ///
    /// Every write to the table goes through here, so the ceiling cannot be bypassed:
    /// `false` means the table was full and nothing was stored, which the calling
    /// operation reports with the limit error its own model binds. Replacing an entry
    /// the table already holds grows nothing, so it is always accepted — which is
    /// what keeps a repeated suppression of one address idempotent.
    pub fn suppress_destination(
        &mut self,
        key: String,
        record: SuppressedDestinationRecord,
    ) -> bool {
        if !self.suppressed_destinations.contains_key(&key)
            && self.suppressed_destinations.len() >= MAX_SUPPRESSED_DESTINATIONS
        {
            return false;
        }
        self.suppressed_destinations.insert(key, record);
        true
    }

    /// The identity `name` names, as the Region `region` holds it — and `None` when
    /// that Region holds no such identity, whatever another Region holds.
    ///
    /// The ONE region-scoped identity lookup every read and write of this surface
    /// resolves an `EmailIdentity` through ([`EmailIdentityRecord::in_region`]), so a
    /// request signed for one Region cannot read, write or send from an identity the
    /// account holds in another.
    pub fn identity_in(&self, region: &str, name: &str) -> Option<&EmailIdentityRecord> {
        self.identities
            .get(&identity_key(name))
            .filter(|record| record.in_region(region))
    }

    /// [`Self::identity_in`] for a caller that is about to CHANGE the identity.
    pub fn identity_in_mut(
        &mut self,
        region: &str,
        name: &str,
    ) -> Option<&mut EmailIdentityRecord> {
        self.identities
            .get_mut(&identity_key(name))
            .filter(|record| record.in_region(region))
    }

    /// Every identity the Region `region` holds, keyed as the table keys them — the
    /// account's identity-key order, which is the order a listing pages in.
    pub fn identities_in<'store>(
        &'store self,
        region: &'store str,
    ) -> impl Iterator<Item = (&'store String, &'store EmailIdentityRecord)> + 'store {
        self.identities
            .iter()
            .filter(move |(_, record)| record.in_region(region))
    }

    /// The identity of `region` that authorises sending from `address`, if any.
    ///
    /// An address identity authorises itself; a domain identity authorises every
    /// address in that domain and in any of its subdomains. The comparison runs on
    /// [`identity_key`], so the domain part matches without regard to case. Only the
    /// identities of the Region the send executes in are considered, which is why a
    /// failed check names that Region ([`unverified_identities_message`]).
    pub fn sending_identity_in(&self, region: &str, address: &str) -> Option<&EmailIdentityRecord> {
        self.sending_identity_for(Some(region), address)
    }

    /// The identity that authorises sending from `address` in ANY Region of the
    /// account, for a reader that is reporting on a send already served rather than
    /// deciding one.
    pub fn sending_identity(&self, address: &str) -> Option<&EmailIdentityRecord> {
        self.sending_identity_for(None, address)
    }

    /// The identity that authorises sending from `address`, scoped to `region` when
    /// one is given ([`EmailIdentityRecord::answers_for`]).
    fn sending_identity_for(
        &self,
        region: Option<&str>,
        address: &str,
    ) -> Option<&EmailIdentityRecord> {
        let key = identity_key(address);
        if let Some(exact) = self
            .identities
            .get(&key)
            .filter(|record| record.answers_for(region))
        {
            return Some(exact);
        }
        self.covering_domain_identity(region, &key)
    }

    /// Every identity a send asks Amazon SES to use that this account has not verified, in
    /// the order the request states them.
    ///
    /// The verification check covers every identity ONE message involves: the sending
    /// identity the message is sent from, each recipient it addresses, and the address
    /// bounce and complaint notifications are forwarded to, whose use is authorised by that
    /// identity's own policy (`FeedbackForwardingEmailAddressIdentityArn`). The mailbox
    /// SIMULATOR's own mailboxes are not identities of any account and need none
    /// ([`is_mailbox_simulator`]), so they never fail the check.
    ///
    /// This is the list a failed check REPORTS ([`unverified_identities_message`]), which is
    /// why it names every identity the check rejected rather than only the first. Each is
    /// named as the request spells it ([`addr_spec`]), since a refusal names what the caller
    /// asked for, and one mailbox named twice is named once — the names already seen are
    /// held in a SET, so the pass costs one store lookup per address and nothing more.
    ///
    /// The caller bounds how many addresses it hands over: a single send weighs at most the
    /// recipients one message may address, and a bulk send the recipients of the entries it
    /// has messages for.
    pub fn unverified_send_identities(
        &self,
        region: &str,
        sender: &str,
        recipients: &[String],
        feedback_forwarding: Option<&str>,
    ) -> Vec<String> {
        let asked = ::std::iter::once(sender)
            .chain(recipients.iter().map(String::as_str))
            .chain(feedback_forwarding);
        self.unverified_among(region, asked)
    }

    /// Those of `addresses` this account holds no identity verified for sending in
    /// `region` for, in the order they arrive and each mailbox named once.
    ///
    /// The one pass behind the reading above: one store lookup per address and nothing
    /// more, with the mailbox simulator's own mailboxes passing it
    /// ([`is_mailbox_simulator`]).
    fn unverified_among<'a>(
        &self,
        region: &str,
        addresses: impl Iterator<Item = &'a str>,
    ) -> Vec<String> {
        let mut failed: Vec<String> = Vec::new();
        let mut seen: BTreeSet<&str> = BTreeSet::new();
        for address in addresses {
            let mailbox = addr_spec(address);
            if is_mailbox_simulator(mailbox) || !seen.insert(mailbox) {
                continue;
            }
            let verified = self
                .sending_identity_in(region, mailbox)
                .is_some_and(|identity| identity.verified_for_sending);
            if verified {
                continue;
            }
            failed.push(mailbox.to_string());
        }
        failed
    }

    /// The identities a send's verification check REFUSES the send for, or `None` when
    /// the check lets it through.
    ///
    /// WHICH of a message's identities the check weighs is what the account's production
    /// access decides. The identity the message is SENT FROM is weighed always —
    /// `SendEmailRequest$FromEmailAddress` is "the email address to use as the 'From'
    /// address for the email. The address that you specify has to be verified." While the
    /// account is in the sandbox — `GetAccount$ProductionAccessEnabled` false, "When your
    /// account is in the sandbox, you can only send email to verified identities" — the
    /// addresses the message would be MAILED to are weighed too: every recipient it
    /// addresses and the address its bounce and complaint notifications are forwarded to.
    /// Once the account has production access they are not, since such an account may
    /// address a message to anybody.
    ///
    /// The mailbox simulator's mailboxes are nobody's identity and fail no check
    /// ([`unverified_send_identities`](Self::unverified_send_identities)), so a send
    /// addressed to one is served in the sandbox as well.
    ///
    /// A BULK send is weighed here too, over the identities of the whole call: its one
    /// sending identity and every mailbox any of its entries addresses
    /// (`crate::ops::send_bulk_email`). One failed identity refuses the call, so a
    /// request none of whose messages SES would accept is refused rather than answered
    /// with a result per entry.
    ///
    /// What this returns is what the refusal REPORTS
    /// ([`unverified_identities_message`]): every identity the check weighed and rejected,
    /// in the order the request states them.
    pub fn identities_refusing_send(
        &self,
        region: &str,
        sender: &str,
        recipients: &[String],
        feedback_forwarding: Option<&str>,
    ) -> Option<Vec<String>> {
        let failed = if self.account.production_access_enabled {
            self.unverified_send_identities(region, sender, &[], None)
        } else {
            self.unverified_send_identities(region, sender, recipients, feedback_forwarding)
        };
        (!failed.is_empty()).then_some(failed)
    }

    /// The most specific domain identity that covers the address `key` names, if any,
    /// among the identities of `region` ([`EmailIdentityRecord::answers_for`]).
    ///
    /// `key` is an [`identity_key`]: the domain part is compared without regard to
    /// case, and a key that names no address (a domain identity's own key) is covered
    /// by nothing, since a domain identity publishes the records for its own name.
    fn covering_domain_identity(
        &self,
        region: Option<&str>,
        key: &str,
    ) -> Option<&EmailIdentityRecord> {
        let (_, domain) = key.rsplit_once('@')?;
        // Walk the domain and each of its parent domains: `a.b.example.com` is
        // covered by an identity for `a.b.example.com`, `b.example.com` or
        // `example.com`.
        let mut candidate = Some(domain);
        while let Some(suffix) = candidate {
            if let Some(found) = self.identities.get(suffix).filter(|record| {
                record.identity_kind == IdentityKind::Domain && record.answers_for(region)
            }) {
                return Some(found);
            }
            candidate = suffix.split_once('.').map(|(_, parent)| parent);
        }
        None
    }

    /// The identity whose DNS zone publishes the DKIM records `record`'s messages are
    /// signed with: the domain identity that covers an address identity, else the
    /// record itself.
    ///
    /// DKIM keys are published in a DOMAIN's DNS, so an email-address identity under a
    /// domain identity has no DKIM configuration of its own — it is the domain's
    /// configuration that signs its mail, and the domain's DNS that SES searches for
    /// it. An address with no covering domain identity is its own authority, which
    /// reports the unconfigured state it was created with.
    pub fn dkim_authority<'a>(&'a self, record: &'a EmailIdentityRecord) -> &'a EmailIdentityRecord {
        if record.identity_kind == IdentityKind::Domain {
            return record;
        }
        // The covering domain identity is one the SAME Region holds: a domain identity
        // of another Region publishes no records for this one's mail.
        self.covering_domain_identity(record.region.as_deref(), &identity_key(&record.name))
            .unwrap_or(record)
    }

    /// The modelled `DkimAttributes` of `record`, resolved from the identity whose DNS
    /// zone publishes its DKIM records ([`Self::dkim_authority`]).
    ///
    /// Every operation that reports an identity's DKIM configuration goes through
    /// here, so a create and the reads after it cannot disagree.
    pub fn dkim_attributes_of(
        &self,
        record: &EmailIdentityRecord,
    ) -> crate::ops::types::DkimAttributes {
        self.dkim_authority(record).dkim_attributes()
    }

    /// The modelled `VerificationInfo` of `record`: the search of the DNS zone that
    /// publishes its DKIM records ([`Self::dkim_authority`]).
    ///
    /// Every identity the account holds has a verification state to report, so the
    /// object itself is always there — what varies is how much of it is known. Where
    /// there is no DNS zone to search — an email-address identity that no domain
    /// identity covers is verified by the recipient following the link in the
    /// verification message — nothing has looked for a record, nothing has succeeded
    /// and nothing has failed, so every member of the object is unset rather than the
    /// object being left out.
    pub fn verification_info_of(
        &self,
        record: &EmailIdentityRecord,
    ) -> crate::ops::types::VerificationInfo {
        let authority = self.dkim_authority(record);
        if authority.identity_kind == IdentityKind::Domain {
            return authority.verification_info();
        }
        crate::ops::types::VerificationInfo::default()
    }

    /// Recipients charged against the sending quota within the trailing
    /// [`SEND_QUOTA_WINDOW`], derived from the log rather than from a counter that
    /// only ever grows, so the quota frees up as the window moves.
    pub fn recipients_in_quota_window(&self, now: SystemTime) -> f64 {
        let cutoff = now
            .checked_sub(SEND_QUOTA_WINDOW)
            .unwrap_or(SystemTime::UNIX_EPOCH);
        self.sent_messages
            .iter()
            .filter(|message| message.sent_at > cutoff)
            .map(|message| message.recipients().count() as f64)
            .sum()
    }

    /// Append an accepted message to the log, keeping it within
    /// [`MAX_SENT_MESSAGES`] by evicting the oldest entries.
    pub fn record_sent(&mut self, message: SentMessage) {
        self.sent_messages.push(message);
        if self.sent_messages.len() > MAX_SENT_MESSAGES {
            let overflow = self.sent_messages.len() - MAX_SENT_MESSAGES;
            self.sent_messages.drain(..overflow);
        }
    }

    /// The portion of a message body the log retains: at most
    /// [`MAX_STORED_BODY_CHARS`] characters.
    pub fn retained_body(body: &str) -> String {
        body.chars().take(MAX_STORED_BODY_CHARS).collect()
    }

    /// Mint the next outbound message id, in the shape SES answers an accepted send
    /// with: [`MESSAGE_ID_VERSION_PREFIX`] and a time field — sixteen hexadecimal
    /// digits together — then a hyphen-separated 8-4-4-4-12 hexadecimal field, then
    /// the trailing zero field.
    ///
    /// `OutboundMessageId` carries no modelled constraint, so the FORM is the one the
    /// service hands back — the same shape the classic SES surface over these same
    /// accounts mints, since one message id identifies one accepted message whichever
    /// surface accepted it — while the only guarantee this makes is the documented one:
    /// the id is unique per accepted message. Uniqueness is STRUCTURAL rather than
    /// probabilistic: the LAST field is the store's own monotonic sequence number, so
    /// two ids this store minted always differ in it — two sends in the same
    /// millisecond included — and the fields before it only keep the rest of the id
    /// opaque.
    pub fn next_message_id(&mut self, now: SystemTime) -> String {
        self.message_sequence += 1;
        let since_epoch = now
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default();
        let millis = since_epoch.as_millis();
        let nanos = since_epoch.as_nanos();
        let mut hex = String::with_capacity(32);
        for salt in [0_u64, u64::from_be_bytes(*b"Outbound")] {
            let mut hasher = DefaultHasher::new();
            salt.hash(&mut hasher);
            nanos.hash(&mut hasher);
            self.message_sequence.hash(&mut hasher);
            write!(hex, "{:016x}", hasher.finish()).expect("writing to a String cannot fail");
        }
        // The sequence number fills the last field, which is twelve hexadecimal digits
        // wide: a store that minted more ids than that field can hold would have to
        // mint 2^48 of them, far past the log this service keeps.
        let sequence = self.message_sequence & 0xffff_ffff_ffff;
        format!(
            "{MESSAGE_ID_VERSION_PREFIX}{millis:012x}-{}-{}-{}-{}-{sequence:012x}-000000",
            &hex[..8],
            &hex[8..12],
            &hex[12..16],
            &hex[16..20],
        )
    }

    /// Mint the id of a new multi-region endpoint.
    ///
    /// `EndpointId` is documented only as "the ID of the multi-region endpoint
    /// (global-endpoint)" and carries no modelled pattern and no modelled length, so the
    /// FORM is recorded behaviour: SES hands the id back as a PAIR OF DOT-SEPARATED
    /// ALPHANUMERIC LABELS — `<label>.<label>`, lowercase, one dot — which is the form a
    /// client reads when it validates the id or places it where the endpoint it names is
    /// addressed. It is NOT a dashed hexadecimal UUID, a bare run of hexadecimal
    /// characters or a prefixed token, so the id is rendered in that form
    /// ([`minted_label_pair`]) rather than in the UUID form this store writes the two job
    /// ids in.
    ///
    /// Beside the form, the identity the surface owes a client: the id is minted ONCE,
    /// here, for the endpoint being created, it is unique across endpoints, and it is
    /// stable for as long as the endpoint holds it (every read reports it off the record).
    /// The monotonic sequence number is part of what the bytes are derived from, so two
    /// endpoints created in the same instant still differ, and a name whose endpoint was
    /// deleted is minted a NEW id when it is created again rather than the id the deleted
    /// endpoint held.
    pub fn next_endpoint_id(&mut self, name: &str, region: &str, now: SystemTime) -> String {
        self.endpoint_sequence += 1;
        minted_label_pair(b"Endpoint", self.endpoint_sequence, now, &[name, region])
    }

    /// Mint the next `TenantId`.
    ///
    /// SES hands a tenant's id back in the documented form [`TENANT_ID_PREFIX`]
    /// followed by [`TENANT_ID_HEX_DIGITS`] lowercase hexadecimal characters, which is
    /// what a client that parses or validates one reads, so the id is minted in that
    /// form. The hexadecimal part is derived from this store's monotonic counter, the
    /// instant of the mint and the id's own domain ([`minted_hex`]), so two tenants
    /// created in the same instant still differ; the id is minted once, at create, and
    /// read back from the record afterwards.
    pub fn next_tenant_id(&mut self, now: SystemTime) -> String {
        self.tenant_sequence += 1;
        format!(
            "{TENANT_ID_PREFIX}{}",
            minted_hex(
                b"Tenant__",
                self.tenant_sequence,
                now,
                &[],
                TENANT_ID_HEX_DIGITS
            )
        )
    }

    /// Mint the `JobId` of a new export job.
    ///
    /// `JobId` is documented only as "a string that represents the export job ID" and
    /// the model bounds it at one character or more, and SES hands it back as a UUID, so
    /// it is minted as one ([`minted_uuid`]) while the only guarantees this makes are the
    /// ones a client depends on: the id is unique across export jobs and stable for as
    /// long as the job holds it (it is minted once, at create, and read back from the
    /// record afterwards). The monotonic sequence number is part of what the bytes are
    /// derived from, so two jobs created in the same instant still differ.
    pub fn next_export_job_id(&mut self, now: SystemTime) -> String {
        self.export_job_sequence += 1;
        minted_uuid(b"ExportJb", self.export_job_sequence, now, &[])
    }

    /// Mint the `JobId` of a new import job.
    ///
    /// `JobId` is documented only as "a string that represents the import job ID" and
    /// the model bounds it at one character or more, and SES hands it back as a UUID, so
    /// — exactly as [`Self::next_export_job_id`] does for the other job kind — it is
    /// minted as one ([`minted_uuid`]) while the only guarantees this makes are the ones
    /// a client depends on: the id is unique across import jobs and stable for as long as
    /// the job holds it (it is minted once, at create, and read back from the record
    /// afterwards). The monotonic sequence number is part of what the bytes are derived
    /// from, so two jobs created in the same instant still differ.
    pub fn next_import_job_id(&mut self, now: SystemTime) -> String {
        self.import_job_sequence += 1;
        minted_uuid(b"ImportJb", self.import_job_sequence, now, &[])
    }

    /// Remove the tenant `tenant_name` holds, with the state that belongs to the
    /// tenant itself, and answer the record removed — or `None` when the account holds
    /// no tenant of that name.
    ///
    /// `DeleteTenant`: "when you delete a tenant, its associations with resources are
    /// removed, but the resources themselves are not deleted". So nothing the tenant
    /// holds can block the removal, and every resource it was associated with stays
    /// exactly as it was. The association rows themselves stay in the table: a row is
    /// the RESOURCE's record of having been associated, which outlives the tenant, and
    /// every tenant-keyed view resolves the tenant record before it reads one
    /// ([`TenantResourceAssociationRecord::belongs_to`]) — so after this a row is
    /// reported by its resource's listing and by no tenant's.
    ///
    /// The tenant's suppression list is part of the tenant and not a resource, so its
    /// entries go with the record: an address reaches a tenant's list only while that
    /// tenant exists, so an entry left behind would be reachable by nothing except a
    /// later tenant that happened to take the same name.
    pub fn delete_tenant(&mut self, tenant_name: &str) -> Option<TenantRecord> {
        let removed = self.tenants.remove(tenant_name)?;
        self.suppressed_destinations
            .retain(|_, entry| entry.tenant_name.as_deref() != Some(tenant_name));
        Some(removed)
    }
}

/// One recommendation about the resource `arn` names.
///
/// `raised_at` is when the shortcoming the finding reports was first there and
/// `fixed_at` when the configuration it asks for was in place, or `None` while it is
/// not: that pair decides both the status and the instant the finding was last updated,
/// so the two cannot disagree. Stated once here, so every derived finding reports its
/// state the same way.
fn recommendation(
    arn: &str,
    recommendation_type: &str,
    description: &str,
    raised_at: SystemTime,
    fixed_at: Option<SystemTime>,
) -> crate::ops::types::Recommendation {
    crate::ops::types::Recommendation {
        resource_arn: Some(arn.to_string()),
        r#type: Some(recommendation_type.to_string()),
        description: Some(description.to_string()),
        status: Some(
            match fixed_at {
                Some(_) => RECOMMENDATION_STATUS_FIXED,
                None => RECOMMENDATION_STATUS_OPEN,
            }
            .to_string(),
        ),
        created_timestamp: Some(::aws_smithy_types::DateTime::from(raised_at)),
        // A finding cannot have been updated before it was raised, so the instant it was
        // resolved is read as no earlier than that.
        last_updated_timestamp: Some(::aws_smithy_types::DateTime::from(
            fixed_at.map_or(raised_at, |at| at.max(raised_at)),
        )),
        impact: Some(RECOMMENDATION_IMPACT_HIGH.to_string()),
    }
}

/// One contact list: the topics it publishes, its description and tags, and the
/// contacts subscribed to it.
///
/// The contacts live ON the list rather than in a table beside it: a contact is
/// identified by its list and its address together (every contact operation binds
/// both as path labels), and deleting the list takes its contacts with it.
#[derive(Debug, Clone)]
pub struct ContactListRecord {
    /// The `ContactListName` this record is keyed by, as the request spelled it.
    pub name: String,
    /// `Topics`: the interest groups the list publishes, in the order supplied.
    pub topics: Vec<crate::ops::types::Topic>,
    /// `Description`.
    pub description: Option<String>,
    /// Tags, in the order they were supplied.
    pub tags: Vec<(String, String)>,
    /// When the list was created.
    pub created_at: SystemTime,
    /// When the list was last changed.
    pub last_updated_at: SystemTime,
    /// The contacts on this list, keyed by [`identity_key`] of their address so a
    /// listing enumerates and pages deterministically.
    pub contacts: BTreeMap<String, ContactRecord>,
}

impl ContactListRecord {
    /// A list as `CreateContactList` stores it.
    pub fn new(name: &str, now: SystemTime) -> Self {
        Self {
            name: name.to_string(),
            topics: Vec::new(),
            description: None,
            tags: Vec::new(),
            created_at: now,
            last_updated_at: now,
            contacts: BTreeMap::new(),
        }
    }

    /// Whether the list publishes a topic called `topic_name`.
    pub fn publishes_topic(&self, topic_name: &str) -> bool {
        self.topics.iter().any(|topic| topic.topic_name == topic_name)
    }

    /// Replace this list's metadata with what `UpdateContactList` supplied — "updates
    /// contact list metadata. This operation does a complete replacement" — and apply
    /// what that replacement means for the contacts on the list.
    ///
    /// `topics` and `description` become exactly what the request supplied, so a member
    /// the request omitted is CLEARED rather than kept, and the topics keep the order
    /// supplied. A contact's stated preference names a topic OF this list (`CreateContact`
    /// refuses one that names anything else), so a preference whose topic the replacement
    /// drops ceases to exist along with the topic: it is removed from every contact on the
    /// list, and declaring that topic again later does not bring it back, because nothing
    /// kept it.
    pub fn replace_metadata(
        &mut self,
        topics: Vec<crate::ops::types::Topic>,
        description: Option<String>,
        now: SystemTime,
    ) {
        self.topics = topics;
        self.description = description;
        self.last_updated_at = now;

        let published: BTreeSet<&str> = self
            .topics
            .iter()
            .map(|topic| topic.topic_name.as_str())
            .collect();
        for contact in self.contacts.values_mut() {
            contact
                .topic_preferences
                .retain(|preference| published.contains(preference.topic_name.as_str()));
            // The contact's own `last_updated_at` stays where its own last write left it:
            // what changed is this list, not the contact, and the dropped preference is
            // the topic's removal showing through rather than a write to the contact.
        }
    }

    /// The default preferences in force FOR `contact`, as `Contact$TopicDefaultPreferences`
    /// reports them — or `None` when none of this list's defaults reaches it.
    ///
    /// `Topic$DefaultSubscriptionStatus` is "the default subscription status to be applied
    /// to a contact if the contact has not noted their preference for subscribing to a
    /// topic", so a topic the contact HAS stated a preference for contributes no default:
    /// the contact's own answer is the status in force, and the member reports only what
    /// `Contact$TopicDefaultPreferences` says it reports, "the default topic preferences
    /// APPLIED to the contact". A contact that has stated a preference for every topic
    /// this list publishes — and a list that publishes none — therefore has no default
    /// applied at all, which is reported as no member rather than as an empty list.
    ///
    /// Every operation that reports the member computes it here, so a contact read on its
    /// own and the same contact in a listing cannot disagree. The entries keep the order
    /// the list publishes its topics in.
    pub fn topic_default_preferences(
        &self,
        contact: &ContactRecord,
    ) -> Option<Vec<crate::ops::types::TopicPreference>> {
        // The contact's own answers are indexed ONCE, the same way `replace_metadata`
        // above indexes the published topics: the work is then the number of topics PLUS
        // the number of preferences, never their product. That matters because a listing
        // projects a whole page of contacts under one lock, and neither collection
        // carries a modelled bound (`Topics` and `TopicPreferenceList` declare no
        // `@length`: `smithy_query(query="shape",shape="Topics,TopicPreferenceList")`).
        let stated: BTreeSet<&str> = contact
            .topic_preferences
            .iter()
            .map(|preference| preference.topic_name.as_str())
            .collect();
        let applied: Vec<crate::ops::types::TopicPreference> = self
            .topics
            .iter()
            .filter(|topic| !stated.contains(topic.topic_name.as_str()))
            .map(|topic| crate::ops::types::TopicPreference {
                topic_name: topic.topic_name.clone(),
                subscription_status: topic.default_subscription_status.clone(),
            })
            .collect();
        (!applied.is_empty()).then_some(applied)
    }

    /// The modelled `ContactList` view of this record, as `ListContactLists` reports
    /// it.
    pub fn summary(&self) -> crate::ops::types::ContactList {
        crate::ops::types::ContactList {
            contact_list_name: Some(self.name.clone()),
            last_updated_timestamp: Some(::aws_smithy_types::DateTime::from(self.last_updated_at)),
        }
    }

    /// The modelled `TagList` view of this record's tags.
    pub fn tag_list(&self) -> Vec<crate::ops::types::Tag> {
        self.tags
            .iter()
            .map(|(key, value)| crate::ops::types::Tag {
                key: key.clone(),
                value: value.clone(),
            })
            .collect()
    }
}

/// One contact on a contact list.
#[derive(Debug, Clone)]
pub struct ContactRecord {
    /// The `EmailAddress` this contact was created with, as the request spelled it.
    pub email_address: String,
    /// `TopicPreferences`: the preferences the contact has stated, in the order
    /// supplied. A topic the contact has stated no preference for is answered from
    /// the list's own default instead
    /// ([`ContactListRecord::topic_default_preferences`]).
    pub topic_preferences: Vec<crate::ops::types::TopicPreference>,
    /// `UnsubscribeAll`: the contact is unsubscribed from every topic of the list.
    pub unsubscribe_all: bool,
    /// `AttributesData`: the caller's own JSON document about this contact, kept as
    /// supplied.
    pub attributes_data: Option<String>,
    /// When the contact was created.
    pub created_at: SystemTime,
    /// When the contact was last changed.
    pub last_updated_at: SystemTime,
}

/// The `SubscriptionStatus` a contact holds for a topic it is subscribed to, as the
/// model's own enum spells it (`SUBSCRIPTION_STATUS_VALUES` is `["OPT_IN",
/// "OPT_OUT"]`): "the contact's subscription status to a topic which is either
/// `OPT_IN` or `OPT_OUT`".
pub const SUBSCRIBED: &str = "OPT_IN";

impl ContactRecord {
    /// A contact as `CreateContact` stores it.
    pub fn new(email_address: &str, now: SystemTime) -> Self {
        Self {
            email_address: email_address.to_string(),
            topic_preferences: Vec::new(),
            unsubscribe_all: false,
            attributes_data: None,
            created_at: now,
            last_updated_at: now,
        }
    }

    /// Record that this contact is subscribed to `topic_name`.
    ///
    /// A message sent with list management "belongs to" the list and the topic it
    /// names, and the unsubscribe link it carries is what lets the contact opt OUT of
    /// that topic, so a contact of the list is subscribed to the topic a message it
    /// was sent belongs to: the preference stated for that topic is [`SUBSCRIBED`].
    /// Stating it goes through [`ContactRecord::merge_topic_preferences`], so it is
    /// one preference per topic here as everywhere else.
    pub fn subscribe_to_topic(&mut self, topic_name: &str) {
        self.merge_topic_preferences(vec![crate::ops::types::TopicPreference {
            topic_name: topic_name.to_string(),
            subscription_status: SUBSCRIBED.to_string(),
        }]);
    }

    /// The contact's preference for `topic_name`, when it has stated one.
    pub fn stated_preference(&self, topic_name: &str) -> Option<&str> {
        self.topic_preferences
            .iter()
            .find(|preference| preference.topic_name == topic_name)
            .map(|preference| preference.subscription_status.as_str())
    }

    /// Apply the topic preferences an update supplied.
    ///
    /// A preference is stated PER TOPIC — "the contact's preference for being opted-in
    /// to or opted-out of a topic" — so a supplied preference sets the status for the
    /// one topic it names, and a topic the request does not name keeps the preference
    /// the contact already stated. A newly stated topic is appended, so the order the
    /// preferences were first supplied in is kept.
    pub fn merge_topic_preferences(
        &mut self,
        supplied: Vec<crate::ops::types::TopicPreference>,
    ) {
        // The preferences already stated are indexed ONCE, the way
        // `ContactListRecord::topic_default_preferences` indexes them: the work is then
        // the number supplied PLUS the number stored, never their product. Neither
        // collection carries a modelled bound (`TopicPreferenceList` declares no
        // `@length`: `smithy_query(query="shape",shape="Topics,TopicPreferenceList")`),
        // and this runs under the shared store's lock, so a product-shaped merge would
        // let one request's list decide how long every other caller waits.
        //
        // The index keeps the FIRST position a name stands at, which is the entry the
        // scan this replaces would have found, so a stored list that holds a name twice
        // is updated in the same place as before.
        let mut position: BTreeMap<String, usize> = BTreeMap::new();
        for (at, stated) in self.topic_preferences.iter().enumerate() {
            position.entry(stated.topic_name.clone()).or_insert(at);
        }
        for preference in supplied {
            match position.get(&preference.topic_name) {
                Some(&at) => {
                    self.topic_preferences[at].subscription_status =
                        preference.subscription_status;
                }
                None => {
                    position.insert(preference.topic_name.clone(), self.topic_preferences.len());
                    self.topic_preferences.push(preference);
                }
            }
        }
    }

    /// The modelled `Contact` view of this record, as `ListContacts` reports it.
    ///
    /// `list` is the list this contact is on, which supplies the default preferences.
    /// Both preference members are the ones `GetContact` reports for the same contact —
    /// a contact that has stated no preference carries no `TopicPreferences` member at
    /// all rather than an empty list, and the defaults are the ones that apply to it —
    /// so one contact cannot read differently in a listing than on its own.
    pub fn summary(&self, list: &ContactListRecord) -> crate::ops::types::Contact {
        crate::ops::types::Contact {
            email_address: Some(self.email_address.clone()),
            topic_preferences: (!self.topic_preferences.is_empty())
                .then(|| self.topic_preferences.clone()),
            topic_default_preferences: list.topic_default_preferences(self),
            unsubscribe_all: self.unsubscribe_all,
            last_updated_timestamp: Some(::aws_smithy_types::DateTime::from(self.last_updated_at)),
        }
    }
}

/// One tenant: the logical container SES groups resources, reputation and sending
/// status under.
#[derive(Debug, Clone)]
pub struct TenantRecord {
    /// The `TenantName` this record is keyed by, as the request spelled it.
    pub name: String,
    /// `TenantId`, minted once by [`ComAmazonawsSesv2SimpleemailserviceV2State::next_tenant_id`].
    pub tenant_id: String,
    /// `TenantArn`, minted once from the request's region by [`tenant_arn`].
    pub arn: String,
    /// When the tenant was created.
    pub created_at: SystemTime,
    /// Tags, in the order they were supplied.
    pub tags: Vec<(String, String)>,
    /// The customer-managed status record of this tenant's reputation entity: the
    /// status the account's own reputation decisions left the tenant's sending in, and
    /// when they did. `None` until `UpdateReputationEntityCustomerManagedStatus`
    /// writes one — nothing has decided anything about this tenant's sending yet, so
    /// there is no record of a decision to report.
    pub customer_managed_status: Option<ReputationStatusRecord>,
    /// The Amazon-SES-managed status record of this tenant's reputation entity: `None`
    /// until Amazon SES itself stops or reinstates the tenant's sending on a reputation
    /// finding of its own. Nothing here raises such a finding, so no record is written;
    /// the status in force is then the one a resource SES has not acted on may send
    /// under ([`AWS_SES_MANAGED_SENDING_STATUS`]).
    pub aws_ses_managed_status: Option<ReputationStatusRecord>,
    /// The suppression list preferences a request STATED for this tenant, or `None`
    /// while none were stated: the tenant then uses the account-level suppression list,
    /// which is the documented default, and has no preferences of its own to report.
    pub suppression: Option<TenantSuppressionPreferences>,
    /// `ReputationEntity$ReputationManagementPolicy`: the ARN of the
    /// Amazon-SES-managed reputation management policy in force on this resource's
    /// reputation entity. The service's own default policy from the moment the
    /// resource exists ([`reputation_management_policy_arn`]), until
    /// `UpdateReputationEntityPolicy` applies another one.
    pub reputation_management_policy: String,
}

impl TenantRecord {
    /// A tenant as `CreateTenant` stores it: no status record on its reputation entity,
    /// because nothing has decided anything about its sending yet; no suppression
    /// preferences of its own until a request states them; and the service's default
    /// reputation management policy.
    ///
    /// `region` is the Region the create executes in, which is the Region of the
    /// Amazon-SES-managed policy the resource's reputation entity starts under: the
    /// policy is in force from the moment the resource exists, because reputation is
    /// tracked for a resource rather than created by a request of its own.
    pub fn new(
        name: &str,
        tenant_id: String,
        arn: String,
        region: &str,
        now: SystemTime,
    ) -> Self {
        Self {
            name: name.to_string(),
            tenant_id,
            arn,
            created_at: now,
            tags: Vec::new(),
            customer_managed_status: None,
            aws_ses_managed_status: None,
            suppression: None,
            reputation_management_policy: reputation_management_policy_arn(
                region,
                DEFAULT_REPUTATION_MANAGEMENT_POLICY,
            ),
        }
    }

    /// The modelled `TenantSuppressionAttributes` view of this record, shared by
    /// every operation that reports a tenant's suppression preferences, so a write
    /// and the read after it cannot disagree — and absent for a tenant no request has
    /// stated preferences for, whose list is the account's by default.
    pub fn suppression_attributes(&self) -> Option<crate::ops::types::TenantSuppressionAttributes> {
        self.suppression
            .as_ref()
            .map(TenantSuppressionPreferences::view)
    }

    /// The modelled `Tenant` view of this record, as `GetTenant` reports it.
    pub fn view(&self) -> crate::ops::types::Tenant {
        crate::ops::types::Tenant {
            tenant_name: Some(self.name.clone()),
            tenant_id: Some(self.tenant_id.clone()),
            tenant_arn: Some(self.arn.clone()),
            created_timestamp: Some(::aws_smithy_types::DateTime::from(self.created_at)),
            tags: Some(self.tag_list()),
            // "The status of sending capability for the tenant" is the tenant's
            // reputation entity's aggregate status, not the customer-managed record
            // alone: a tenant Amazon SES disabled may not send whatever the account
            // asked for.
            sending_status: Some(self.aggregate_sending_status()),
            suppression_attributes: self.suppression_attributes(),
        }
    }

    /// The modelled `TenantInfo` view of this record, as `ListTenants` reports it:
    /// "basic information about each tenant, such as tenant name, ID, ARN, and
    /// creation timestamp".
    ///
    /// Every member it carries is the one [`Self::view`] reports under the same
    /// name, read off the same stored record, so a listing and a read of one tenant
    /// cannot disagree about it; `TenantInfo` simply carries no tags and no
    /// suppression attributes.
    pub fn info_view(&self) -> crate::ops::types::TenantInfo {
        let full = self.view();
        crate::ops::types::TenantInfo {
            tenant_name: full.tenant_name,
            tenant_id: full.tenant_id,
            tenant_arn: full.tenant_arn,
            created_timestamp: full.created_timestamp,
            sending_status: full.sending_status,
        }
    }

    /// The customer-managed sending status in force on this tenant: the status the
    /// account last wrote, or, while it has written none, the status a tenant may send
    /// under from the moment it is created ([`TENANT_SENDING_STATUS_CREATED`]).
    pub fn customer_managed_sending_status(&self) -> &str {
        self.customer_managed_status
            .as_ref()
            .map_or(TENANT_SENDING_STATUS_CREATED, |record| {
                record.status.as_str()
            })
    }

    /// The Amazon-SES-managed sending status in force on this tenant: the status Amazon
    /// SES last wrote, or, while it has written none, the status it permits a resource
    /// it has raised no finding against ([`AWS_SES_MANAGED_SENDING_STATUS`]).
    pub fn aws_ses_managed_sending_status(&self) -> &str {
        self.aws_ses_managed_status
            .as_ref()
            .map_or(AWS_SES_MANAGED_SENDING_STATUS, |record| {
                record.status.as_str()
            })
    }

    /// The aggregate sending status of this tenant's reputation entity, over the two
    /// statuses in force on it — see [`aggregate_sending_status`].
    pub fn aggregate_sending_status(&self) -> String {
        aggregate_sending_status(
            self.customer_managed_sending_status(),
            self.aws_ses_managed_sending_status(),
        )
    }

    /// The modelled `ReputationEntity` view of this tenant, as
    /// `ListReputationEntities` and `GetReputationEntity` report it, over the account's
    /// reputation `findings`.
    pub fn reputation_entity(
        &self,
        findings: &[crate::ops::types::Recommendation],
    ) -> crate::ops::types::ReputationEntity {
        crate::ops::types::ReputationEntity {
            // "For resource-type entities, this is the Amazon Resource Name (ARN) of
            // the resource."
            reputation_entity_reference: Some(self.arn.clone()),
            reputation_entity_type: Some(REPUTATION_ENTITY_TYPE_RESOURCE.to_string()),
            // "The ARN of the reputation management policy applied to this entity":
            // an entity is always under one — the service's own default policy until
            // `UpdateReputationEntityPolicy` applies another — so the member is read
            // off the record rather than left absent.
            reputation_management_policy: Some(self.reputation_management_policy.clone()),
            // Each status record is reported only once its own owner has written it: a
            // record nothing wrote would tell a client a decision was taken about this
            // entity's sending when none was.
            customer_managed_status: self
                .customer_managed_status
                .as_ref()
                .map(ReputationStatusRecord::view),
            aws_ses_managed_status: self
                .aws_ses_managed_status
                .as_ref()
                .map(ReputationStatusRecord::view),
            sending_status_aggregate: Some(self.aggregate_sending_status()),
            reputation_impact: Some(reputation_impact(findings, &self.arn)),
        }
    }

    /// Write this tenant's suppression preferences, as a request that states
    /// `TenantSuppressionAttributes` asks for.
    ///
    /// Both members are replaced together: the preferences a request states are the
    /// tenant's whole preferences afterwards, never merged with what was there.
    pub fn set_suppression_preferences(&mut self, preferences: TenantSuppressionPreferences) {
        self.suppression = Some(preferences);
    }

    /// Write the customer-managed sending status of this tenant's reputation entity,
    /// as `UpdateReputationEntityCustomerManagedStatus` asks for: the status, the cause
    /// that write is ([`CUSTOMER_MANAGED_STATUS_CAUSE`]) and when it was written.
    pub fn set_customer_managed_sending_status(&mut self, status: String, now: SystemTime) {
        self.customer_managed_status = Some(ReputationStatusRecord::manually_updated(status, now));
    }

    /// Apply the reputation management policy `policy` to this resource's reputation
    /// entity, as `UpdateReputationEntityPolicy` asks for.
    ///
    /// The policy REPLACES the one in force: an entity is under exactly one policy,
    /// so the ARN written here is the one every later read reports. Nothing else on
    /// the entity moves — neither status record is touched, so the aggregate status
    /// is unchanged.
    pub fn set_reputation_management_policy(&mut self, policy: String) {
        self.reputation_management_policy = policy;
    }

    /// The modelled `TagList` view of this record's tags.
    pub fn tag_list(&self) -> Vec<crate::ops::types::Tag> {
        self.tags
            .iter()
            .map(|(key, value)| crate::ops::types::Tag {
                key: key.clone(),
                value: value.clone(),
            })
            .collect()
    }
}

/// One tenant-resource association: a resource (an email identity, a configuration
/// set or an email template, named by its ARN) usable when sending on behalf of the
/// tenant it is associated with.
///
/// The tenant's id is copied in at association time, so the association reports the
/// tenant record it was made against even after a tenant of the same name is
/// recreated.
#[derive(Debug, Clone)]
pub struct TenantResourceAssociationRecord {
    /// The `TenantName` the association was made for, as the request spelled it.
    pub tenant_name: String,
    /// The associated tenant's `TenantId`, read off the tenant record.
    pub tenant_id: String,
    /// The `ResourceArn` the association was made for, as the request spelled it.
    pub resource_arn: String,
    /// When the resource was associated with the tenant.
    pub associated_at: SystemTime,
}

impl TenantResourceAssociationRecord {
    /// The modelled `ResourceTenantMetadata` view of this association, as
    /// `ListResourceTenants` reports it.
    pub fn resource_tenant_view(&self) -> crate::ops::types::ResourceTenantMetadata {
        crate::ops::types::ResourceTenantMetadata {
            tenant_name: Some(self.tenant_name.clone()),
            tenant_id: Some(self.tenant_id.clone()),
            resource_arn: Some(self.resource_arn.clone()),
            associated_timestamp: Some(::aws_smithy_types::DateTime::from(self.associated_at)),
        }
    }

    /// Whether this association is one `tenant` holds — the record its `TenantName`
    /// resolves to NOW.
    ///
    /// The tenant's id is copied in at association time, so an association made
    /// against a tenant that has since been deleted belongs to no tenant at all, and in
    /// particular not to a later tenant that took the same name: a tenant-keyed view
    /// reports the associations of the tenant record it resolved and no others.
    pub fn belongs_to(&self, tenant: &TenantRecord) -> bool {
        self.tenant_name == tenant.name && self.tenant_id == tenant.tenant_id
    }

    /// The modelled `TenantResource` view of this association, as
    /// `ListTenantResources` reports it: the resource's ARN and the resource type that
    /// ARN names, which is read off the ARN rather than stored beside it so one
    /// record cannot disagree with itself about what it points at. The token reported
    /// for a kind is the one SES reports ([`reported_tenant_resource_type`]).
    pub fn tenant_resource_view(&self) -> crate::ops::types::TenantResource {
        crate::ops::types::TenantResource {
            resource_type: reported_tenant_resource_type(&self.resource_arn)
                .map(ToString::to_string),
            resource_arn: Some(self.resource_arn.clone()),
        }
    }
}

/// How this service words `NotFoundException` for a contact list it does not hold.
///
/// One wording for every operation that takes a `ContactListName` — the reads, the
/// writes and the contact operations nested under the list — so a client sees the same
/// sentence whichever call first tells it the list is gone.
pub fn contact_list_not_found(contact_list_name: &str) -> String {
    format!("List with name: {contact_list_name} doesn't exist.")
}

/// How this service words `NotFoundException` for a contact the named list does not
/// hold, stated once for every contact operation.
///
/// SES names the address it looked for and the kind of resource it looked in, not the
/// list's own name, so the sentence is the same whichever list was asked.
pub fn contact_not_found(email_address: &str) -> String {
    format!("{email_address} doesn't exist in List.")
}

/// How this service words `AlreadyExistsException` for an address the named list already
/// holds a contact under.
///
/// The counterpart of [`contact_not_found`], and worded the same way: the one variable
/// part is the address the request supplied, and the resource is named by its KIND rather
/// than by the list's own name, so one sentence serves whichever list was asked.
pub fn contact_already_exists(email_address: &str) -> String {
    format!("{email_address} already exists in List.")
}

/// Characters of `AttributesData` one contact may carry.
///
/// The model declares no constraint on the member; SES states the bound in the
/// sentence it refuses an unacceptable document with, "Attributes must be valid JSON
/// and it can contain up to 1000 characters.", which is the sentence
/// [`invalid_attributes_data`] builds from this constant.
pub const MAX_ATTRIBUTES_DATA_CHARS: usize = 1_000;

/// Whether `attributes_data` is something a contact may carry: "the attribute data
/// attached to a contact" is a JSON document of at most
/// [`MAX_ATTRIBUTES_DATA_CHARS`] characters.
///
/// The member carries no modelled constraint, so both halves are checked here —
/// once, for every operation that accepts attribute data — and the document is held
/// exactly as supplied once accepted.
pub fn attributes_data_is_acceptable(attributes_data: &str) -> bool {
    attributes_data.chars().count() <= MAX_ATTRIBUTES_DATA_CHARS
        && ::serde_json::from_str::<::serde_json::Value>(attributes_data).is_ok()
}

/// How this service words `BadRequestException` for attribute data that is not a
/// JSON document within the bound, stated once for every operation that takes one.
pub fn invalid_attributes_data() -> String {
    format!(
        "Attributes must be valid JSON and it can contain up to \
         {MAX_ATTRIBUTES_DATA_CHARS} characters."
    )
}

/// How this service words the refusal of a topic the named list does not publish.
///
/// One wording for every operation that names a `TopicName` within a list — the
/// contact writes and the send that resolves its list-management options — so a
/// client sees the same sentence whichever call first tells it the topic is not
/// there. SES names the list first and the topic it does not contain second.
pub fn contact_list_topic_not_found(contact_list_name: &str, topic_name: &str) -> String {
    format!("List: {contact_list_name} doesn't contain Topic: {topic_name}")
}

/// One email template in the account's template store.
///
/// The content is held as the model's own `EmailTemplateContent` — the subject line,
/// the text part and the HTML part exactly as the request supplied them — because a
/// template is rendered when a send names it and is never rewritten on the way in or
/// out. `created_at` is the instant the template was recorded, which
/// `ListEmailTemplates` reports as `EmailTemplateMetadata$CreatedTimestamp` ("the
/// time and date the template was created").
#[derive(Debug, Clone)]
pub struct EmailTemplateRecord {
    /// The `TemplateName` this record is keyed by, as the request spelled it.
    pub name: String,
    /// `TemplateContent`: the subject line, the HTML part and the text-only part.
    pub content: crate::ops::types::EmailTemplateContent,
    /// Tags, in the order they were supplied.
    pub tags: Vec<(String, String)>,
    /// When the template was created.
    pub created_at: SystemTime,
    /// Whether the account's email-template registry already carries this template
    /// ([`templates`]). False on a record this surface has just recorded and not published
    /// yet; a record that is registered here and gone from the registry was DELETED through
    /// the classic SES surface, so a reconciliation drops it.
    pub registered: bool,
}

impl EmailTemplateRecord {
    /// A template as `CreateEmailTemplate` stores it.
    pub fn new(
        name: &str,
        content: crate::ops::types::EmailTemplateContent,
        now: SystemTime,
    ) -> Self {
        Self {
            name: name.to_string(),
            content,
            tags: Vec::new(),
            created_at: now,
            registered: false,
        }
    }

    /// The modelled `EmailTemplateMetadata` view of this record, as
    /// `ListEmailTemplates` reports it.
    pub fn metadata(&self) -> crate::ops::types::EmailTemplateMetadata {
        crate::ops::types::EmailTemplateMetadata {
            template_name: Some(self.name.clone()),
            created_timestamp: Some(::aws_smithy_types::DateTime::from(self.created_at)),
        }
    }

    /// The modelled `TagList` view of this record's tags.
    pub fn tag_list(&self) -> Vec<crate::ops::types::Tag> {
        self.tags
            .iter()
            .map(|(key, value)| crate::ops::types::Tag {
                key: key.clone(),
                value: value.clone(),
            })
            .collect()
    }
}

/// How large the template content `content` is: the bytes of its subject line and of
/// both body parts, which is what [`MAX_EMAIL_TEMPLATE_BYTES`] bounds.
///
/// Stated once here so every operation that writes a template's content measures it
/// the same way.
pub fn email_template_size(content: &crate::ops::types::EmailTemplateContent) -> usize {
    [&content.subject, &content.text, &content.html]
        .into_iter()
        .filter_map(|part| part.as_deref())
        .map(str::len)
        .sum()
}

/// How this service words `NotFoundException` for an email template it does not hold.
///
/// One wording for every operation that takes an `EmailTemplateName` as the resource it
/// reads or writes — the read, the update and the delete — so a client sees the same
/// sentence whichever call first tells it the template is not there. The RENDER path words
/// the same condition differently ([`email_template_not_found_to_render`]), as SES does.
pub fn email_template_not_found(template_name: &str) -> String {
    format!("Template {template_name} does not exist.")
}

/// How this service words `NotFoundException` on the RENDER path for an email template the
/// account holds nothing under.
///
/// SES words this one condition differently per surface, and the render path's sentence is
/// ACCOUNT-SCOPED: the fixed text "Unable to find <", "> for <" and ">" around the name
/// the request asked for and the caller's own account id, which says that this account can
/// see no such template rather than that no such resource exists. The account is the one
/// this emulator serves, read from the emulator rather than from anything the request
/// carries — the same reading [`email_template_already_exists`] takes of it.
pub fn email_template_not_found_to_render(template_name: &str) -> String {
    format!(
        "Unable to find <{template_name}> for <{}>",
        ::emulator_server::emulator_account_id()
    )
}

/// How this service words `AlreadyExistsException` for a template name the account
/// already holds, for a create that attached `tag_count` tags.
///
/// SES has TWO sentences for one condition, and which one it answers is decided by the
/// path the create took rather than by what the store holds: a create that attaches tags
/// is refused with the bare statement that the name is taken, disclosing nothing about
/// the resource or the account, while a create that attaches none is refused with the
/// sentence that names both the template the request asked for and the account that
/// already holds it. The account is the one this emulator serves, read from the emulator
/// rather than from anything the request carries.
///
/// Stated once here — the selection included — so the two sentences cannot be chosen
/// differently by a second caller.
pub fn email_template_already_exists(template_name: &str, tag_count: usize) -> String {
    if tag_count == 0 {
        format!(
            "Template {template_name} already exists for account id {}",
            ::emulator_server::emulator_account_id()
        )
    } else {
        "Template already exists".to_string()
    }
}

/// How this service words `BadRequestException` for template content past the size quota
/// ([`MAX_EMAIL_TEMPLATE_BYTES`]).
///
/// The quota is a service limit rather than a modelled bound, and the sentence states it:
/// the template the request named, and the limit in the unit SES documents it in. What the
/// refused content MEASURED is not part of it — the sentence is about the quota, which is
/// also what tells this refusal apart from a member refused for its own modelled length.
///
/// One wording for every operation that writes a named template's content, so a client
/// sees the same sentence whichever call first tells it the template is too large.
pub fn email_template_exceeds_maximum_size(template_name: &str) -> String {
    format!(
        "Template {template_name} exceeds maximum allowed size of \
         {MAX_EMAIL_TEMPLATE_KILOBYTES} KB."
    )
}

/// The status of a multi-region endpoint, from the model's `Status` enum.
///
/// The enum also carries `FAILED` ("the resource failed to be provisioned"), which
/// nothing here reaches: provisioning an emulated endpoint is bookkeeping and cannot
/// fail.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndpointStatus {
    /// `CREATING`: the resource is being provisioned.
    Creating,
    /// `READY`: the resource is ready to use.
    Ready,
    /// `DELETING`: the resource is being deleted as requested.
    Deleting,
}

impl EndpointStatus {
    /// The modelled `Status` value.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Creating => "CREATING",
            Self::Ready => "READY",
            Self::Deleting => "DELETING",
        }
    }
}

/// One multi-region endpoint (global-endpoint): the regions its sending traffic is
/// split between, its provisioning status and its tags.
#[derive(Debug, Clone)]
pub struct MultiRegionEndpointRecord {
    /// The `EndpointName` this record is keyed by, as the request spelled it.
    pub name: String,
    /// The `EndpointId` minted for it at create time, stable for the life of the
    /// endpoint.
    pub endpoint_id: String,
    /// The primary region: the Region the create call executed in, which is also
    /// the only Region the endpoint can be read, listed or deleted from.
    pub primary_region: String,
    /// The secondary regions the create request's route details named, in the order
    /// they were supplied.
    pub secondary_regions: Vec<String>,
    /// The provisioning status.
    pub status: EndpointStatus,
    /// Tags, in the order they were supplied.
    pub tags: Vec<(String, String)>,
    /// When the endpoint was created.
    pub created_at: SystemTime,
    /// When the endpoint was last changed.
    pub last_updated_at: SystemTime,
    /// When the `CREATING` -> `READY` transition falls due.
    pub ready_due_at: SystemTime,
    /// When the deletion of this endpoint completes, once one has been requested.
    pub deletion_due_at: Option<SystemTime>,
}

impl MultiRegionEndpointRecord {
    /// An endpoint as `CreateMultiRegionEndpoint` stores it: being provisioned, with
    /// its `READY` transition already due at a known instant.
    pub fn new(
        name: &str,
        endpoint_id: String,
        primary_region: &str,
        secondary_regions: Vec<String>,
        now: SystemTime,
    ) -> Self {
        Self {
            name: name.to_string(),
            endpoint_id,
            primary_region: primary_region.to_string(),
            secondary_regions,
            status: EndpointStatus::Creating,
            tags: Vec::new(),
            created_at: now,
            last_updated_at: now,
            ready_due_at: now
                .checked_add(ENDPOINT_PROVISIONING_SETTLE)
                .unwrap_or(now),
            deletion_due_at: None,
        }
    }

    /// Start tearing this endpoint down, as `DeleteMultiRegionEndpoint` does: it
    /// carries the non-terminal `DELETING` status until the deletion completes.
    pub fn start_deletion(&mut self, now: SystemTime) {
        self.status = EndpointStatus::Deleting;
        self.last_updated_at = now;
        self.deletion_due_at = Some(
            now.checked_add(ENDPOINT_DELETION_SETTLE)
                .unwrap_or(now),
        );
    }

    /// Whether a deletion of this endpoint is still running at `now`.
    pub fn deletion_in_flight(&self, now: SystemTime) -> bool {
        self.deletion_due_at
            .is_some_and(|due_at| due_at > now)
    }

    /// Whether the deletion of this endpoint has completed at `now`, which is when
    /// the endpoint stops existing at all.
    pub fn deletion_complete(&self, now: SystemTime) -> bool {
        self.deletion_due_at.is_some_and(|due_at| due_at <= now)
    }

    /// Apply the provisioning transition if it is due at `now`.
    pub fn settle(&mut self, now: SystemTime) {
        if self.status == EndpointStatus::Creating && now >= self.ready_due_at {
            self.status = EndpointStatus::Ready;
            self.last_updated_at = self.ready_due_at;
        }
    }

    /// The regions traffic is split between: the primary region first, then the
    /// secondary regions the create request named.
    ///
    /// "From the data flow standpoint there is no difference between primary and
    /// secondary regions - sending traffic is divided between the two", so both the
    /// `Regions` a listing reports and the `Routes` a read reports are this one
    /// projection, and a create and the reads after it cannot disagree.
    pub fn regions(&self) -> Vec<String> {
        let mut regions = Vec::with_capacity(1 + self.secondary_regions.len());
        regions.push(self.primary_region.clone());
        regions.extend(self.secondary_regions.iter().cloned());
        regions
    }

    /// The modelled `Routes` view of this record.
    pub fn routes(&self) -> Vec<crate::ops::types::Route> {
        self.regions()
            .into_iter()
            .map(|region| crate::ops::types::Route { region })
            .collect()
    }

    /// The modelled `MultiRegionEndpoint` view of this record, as
    /// `ListMultiRegionEndpoints` reports it.
    pub fn summary(&self) -> crate::ops::types::MultiRegionEndpoint {
        crate::ops::types::MultiRegionEndpoint {
            endpoint_name: Some(self.name.clone()),
            status: Some(self.status.as_str().to_string()),
            endpoint_id: Some(self.endpoint_id.clone()),
            regions: Some(self.regions()),
            created_timestamp: Some(::aws_smithy_types::DateTime::from(self.created_at)),
            last_updated_timestamp: Some(::aws_smithy_types::DateTime::from(self.last_updated_at)),
        }
    }
}

/// How this service words `NotFoundException` for a multi-region endpoint the Region
/// the call executes in does not hold.
///
/// A read answers it with no message at all — the error code is the whole answer —
/// while a delete of an endpoint that is not there says so
/// ([`multi_region_endpoint_not_found_to_delete`]), so this is the one place that
/// states both.
pub fn multi_region_endpoint_not_found() -> crate::ops::types::NotFoundException {
    crate::ops::types::NotFoundException { message: None }
}

/// How this service words `NotFoundException` when a delete names an endpoint the
/// Region does not hold.
pub fn multi_region_endpoint_not_found_to_delete() -> crate::ops::types::NotFoundException {
    crate::ops::types::NotFoundException {
        message: Some("Endpoint does not exist.".to_string()),
    }
}

/// How this service words the refusal of a delete that collides with a tear-down
/// already running: the refusal names the status the resource is in, which for this
/// refusal is always [`DEPROVISIONING_STATUS`] — the one status a delete cannot act
/// on, because it is the one a delete put the resource into.
pub fn unable_to_delete_while_deprovisioning() -> String {
    format!("Unable to delete resource in {DEPROVISIONING_STATUS} status.")
}

/// How this service words the refusal of a `NextToken` that is not a position in the
/// listing being read, for the listings that render that refusal as
/// `BadRequestException`.
///
/// AWS's own sentence for the bad-cursor class on those listings, which is wholly FIXED
/// text: a token is refused for being something this listing never handed out, and there
/// is nothing else to say about it — in particular the rejected token is never echoed
/// back. ONE wording for all of them, whatever form the bad cursor takes, because it is
/// one condition; a second sentence for THOSE listings would be exactly the drift this
/// function exists to prevent.
///
/// The wording belongs to the operation that refuses the cursor, so a listing AWS words
/// differently states the condition in its own words beside its own handler and does not
/// reach this function: `ListSuppressedDestinations`, whose model binds
/// `InvalidNextTokenException` for it, and `ListEmailIdentities`, which echoes the cursor
/// it rejected.
pub fn invalid_next_token() -> String {
    "Invalid next token.".to_string()
}

/// How this service words the refusal of a METRIC READ or an EXPORT JOB whose window runs
/// backwards — an end bound that precedes the start bound, which describes no interval at all.
///
/// AWS's own sentence for that class on those operations, which is wholly FIXED text: the two
/// bounds are the caller's own and the sentence states the requirement they broke rather than
/// echoing them back. ONE wording for both of them — the metric queries and the export job's
/// data source — because it is one condition, and a second sentence for it would be exactly
/// the drift this function exists to prevent. The suppression-list LISTING words the same
/// class by naming the two bounds it was given
/// ([`crate::state::suppression_window_runs_backwards`]), which is that operation's own
/// recorded wording.
pub fn window_runs_backwards() -> String {
    "Expected start date to be before the end date.".to_string()
}

/// The version prefix an outbound message id carries: SES spells the id of a
/// message it accepted as `0100`, then hyphen-separated hexadecimal fields, which
/// is also the shape
/// [`ComAmazonawsSesv2SimpleemailserviceV2State::next_message_id`] mints.
pub const MESSAGE_ID_VERSION_PREFIX: &str = "0100";

/// `EventType`'s value for the acceptance of a message, as the model spells it
/// (see `crate::ops::types::EVENT_TYPE_VALUES`): the one sending event this slice
/// observes, because accepting the message is all it does with one.
pub const EVENT_TYPE_SEND: &str = "SEND";

/// Whether `message_id` has the form of an id this service could have issued for
/// an accepted message: the version prefix followed by at least one further
/// hyphen-separated field, every field non-empty and hexadecimal.
///
/// `OutboundMessageId` carries no modelled constraint, so this is what separates
/// the caller's mistake from a message that is simply not held: a value outside
/// the form names no message SES could ever have accepted, while a value inside it
/// names one the account may or may not hold.
pub fn message_id_is_well_formed(message_id: &str) -> bool {
    let Some((first, rest)) = message_id.split_once('-') else {
        return false;
    };
    first.len() > MESSAGE_ID_VERSION_PREFIX.len()
        && first.starts_with(MESSAGE_ID_VERSION_PREFIX)
        && [first]
            .into_iter()
            .chain(rest.split('-'))
            .all(|field| !field.is_empty() && field.chars().all(|c| c.is_ascii_hexdigit()))
}

/// How this service words `BadRequestException` for a `MessageId` that is not an
/// outbound message id at all, stated once for every operation that takes one.
///
/// The sentence carries no part of the request: the value is refused for not being
/// a message id at all, and there is nothing else to say about it.
pub fn invalid_message_id() -> String {
    "Invalid MessageId".to_string()
}

/// How this service words `NotFoundException` for a well-formed message id the
/// account holds no accepted message for, stated once for every operation that
/// reads a message by its id.
///
/// `NotFoundException` is documented only as "the resource you attempted to access
/// doesn't exist", which names no resource, so the sentence names the resource kind and
/// the requested id, which is its only variable part, taken from the request.
pub fn message_not_found(message_id: &str) -> String {
    format!("Message {message_id} does not exist.")
}

/// Apply `applied` over the headers `carried`, the way one message header applies over
/// another of the same name: a name `carried` already holds takes the applied value, in
/// the place and the spelling the applied header states, and a name it does not hold is
/// added after the ones it does.
///
/// Stated once for every place a message's headers are built from two sources — a bulk
/// entry's over the template's on the way in, and the message's own over the envelope
/// headers SES assembles on the way out to an event — so no two of them can disagree about
/// which headers one message carries. A header name compares without regard to case, as a
/// mail header name does.
pub fn apply_message_headers(
    carried: &mut Vec<(String, String)>,
    applied: impl IntoIterator<Item = (String, String)>,
) {
    // The names already carried are indexed ONCE, folded to lower case because a mail
    // header name compares without regard to case: the work is then the number applied
    // PLUS the number carried, never their product. `MessageHeaderList` carries no
    // modelled bound (`smithy_query(query="shape",shape="MessageHeaderList")`), so a
    // product-shaped merge would let one message's header list decide how long the send
    // path runs.
    //
    // The index keeps the FIRST position a name stands at, which is the header the scan
    // this replaces would have found: a mail message may carry two headers of one name,
    // and the applied value lands on the same one as before.
    let mut position: BTreeMap<String, usize> = BTreeMap::new();
    for (at, (written, _)) in carried.iter().enumerate() {
        position.entry(written.to_ascii_lowercase()).or_insert(at);
    }
    for (name, value) in applied {
        let key = name.to_ascii_lowercase();
        match position.get(&key) {
            Some(&at) => carried[at] = (name, value),
            None => {
                position.insert(key, carried.len());
                carried.push((name, value));
            }
        }
    }
}

/// How this service words the account's 24-hour sending quota being spent, stated once for
/// every operation that charges a message against it — `TooManyRequestsException` for a
/// send that was refused for it, and the `Error` of a bulk entry that reports it as that
/// entry's `BulkEmailStatus`.
///
/// Neither the model nor a recording of this surface states the sentence, so it names the
/// one condition it reports: how much of the quota the window has already charged, and the
/// quota itself, both read from the account's own counters.
pub fn daily_quota_exceeded(charged: f64, quota: f64) -> String {
    format!(
        "Daily message quota exceeded: {charged} of {quota} recipients already sent in the \
         last 24 hours."
    )
}

/// The `ImportDestinationType` the destination `destination` selects, or `None` when it
/// selects neither or both.
///
/// `ImportDestination` is "an object that contains details about the resource
/// destination the import job is going to target", and the two members it declares are
/// the two kinds of resource an import can target — a contact list or the account's
/// suppression list — so which one it carries is what the job's destination type is: a
/// request that names neither, or both, names no one destination to import into. The
/// values are the modelled ones (see `crate::ops::types::IMPORT_DESTINATION_TYPE_VALUES`).
pub fn import_destination_type(
    destination: &crate::ops::types::ImportDestination,
) -> Option<&'static str> {
    match (
        destination.contact_list_destination.as_ref(),
        destination.suppression_list_destination.as_ref(),
    ) {
        (Some(_), None) => Some("CONTACT_LIST"),
        (None, Some(_)) => Some("SUPPRESSION_LIST"),
        _ => None,
    }
}

/// How this service words `BadRequestException` for an `ImportDestination` that selects
/// no one destination.
///
/// The counterpart of [`invalid_export_data_source`], and worded the same way: the
/// sentence states the modelled rule — the structure carries exactly one of the two
/// destinations — and carries no part of the request, because what is wrong is which
/// members are present, which the sentence already names.
pub fn invalid_import_destination() -> String {
    "ImportDestination must contain exactly one of SuppressionListDestination or \
     ContactListDestination."
        .to_string()
}

/// How this service words `NotFoundException` for a `JobId` the account holds no import
/// job for, stated once for every operation that reads an import job by its id.
///
/// `NotFoundException` is documented only as "the resource you attempted to access
/// doesn't exist", which names no resource, so the sentence names the resource kind and
/// the requested id, which is its only variable part, taken from the request — the same
/// wording this service gives every other resource it is asked for by id.
pub fn import_job_not_found(job_id: &str) -> String {
    format!("Import job {job_id} does not exist.")
}

/// One import job: what it imports, where it imports it to, and when it was created.
///
/// Its STATUS is not stored: it is DERIVED from the instant the job was created
/// ([`Self::status`]), because the whole lifecycle of an import run is owed by the clock
/// rather than by any call — this service models no operation that cancels or restarts
/// an import job. So a reader reports the status the clock has made due without writing
/// to the store, and two readers of one job can never disagree about where it has got
/// to.
///
/// That status is a RECORD of the lifecycle the create owed, not a report on records
/// something imported: the records an import job reads live in the Amazon S3 object the
/// job's data source names, so [`Self::processed_records_count`] and
/// [`Self::failed_records_count`] count what this emulator really imported — none — and
/// nothing synthesises a number to match the status the lifecycle reached. An operation
/// that comes to ingest an import job's records must take them from somewhere this
/// emulator really holds and bump those counts as it does.
///
/// What the run DOES resolve is whether that object is there to be read at all
/// ([`import_source`]): the outcome of that read is recorded on [`Self::failure`], and it
/// is what decides which terminal status the run reaches.
#[derive(Debug, Clone)]
pub struct ImportJobRecord {
    /// The `JobId` this record is keyed by, minted when the job was created and stable
    /// for the life of the job.
    pub job_id: String,
    /// `ImportDestination`: the resource the job imports into, exactly as the create
    /// supplied it, which every read of the job reports back.
    pub destination: crate::ops::types::ImportDestination,
    /// `ImportDataSource`: the S3 location the job reads and the format of what is
    /// there, exactly as the create supplied it.
    pub data_source: crate::ops::types::ImportDataSource,
    /// When the job was created, which is the `CreatedTimestamp` a read reports and the
    /// instant its whole lifecycle is owed from.
    pub created_at: SystemTime,
    /// When the `CREATED` -> `PROCESSING` transition falls due.
    pub processing_due_at: SystemTime,
    /// When the run's terminal transition falls due.
    pub completion_due_at: SystemTime,
    /// `ProcessedRecordsCount`: "the current number of records processed" — the records
    /// this emulator has really imported for the job.
    pub processed_records_count: i32,
    /// `FailedRecordsCount`: "the number of records that failed processing because of
    /// invalid input or other reasons" — the ones this emulator really rejected.
    pub failed_records_count: i32,
    /// Whether the account's object storage has ANSWERED about the data source this run
    /// reads, which is what makes the answer below a fact about the source rather than an
    /// assumption; until it has, the resolution is still owed
    /// ([`import_source::resolve_due_sources`]).
    pub source_answered: bool,
    /// `FailureInfo`: "an object that contains the failure details about a job" — what the
    /// run's read of its data source diagnosed, or `None` when it diagnosed no failure.
    pub failure: Option<crate::ops::types::FailureInfo>,
}

impl ImportJobRecord {
    /// A job as a create stores it: just created, with both transitions of its run
    /// already due at known instants and nothing imported yet.
    pub fn new(
        job_id: String,
        destination: crate::ops::types::ImportDestination,
        data_source: crate::ops::types::ImportDataSource,
        now: SystemTime,
    ) -> Self {
        let processing_due_at = now.checked_add(IMPORT_JOB_PROCESSING_SETTLE).unwrap_or(now);
        Self {
            job_id,
            destination,
            data_source,
            created_at: now,
            processing_due_at,
            completion_due_at: processing_due_at
                .checked_add(IMPORT_JOB_COMPLETION_SETTLE)
                .unwrap_or(processing_due_at),
            processed_records_count: 0,
            failed_records_count: 0,
            source_answered: false,
            failure: None,
        }
    }

    /// The status of the job's run at `now`.
    ///
    /// Derived, so nothing is written because a job was read, and a run whose whole span
    /// has already elapsed reports the terminal status its own data source decided rather
    /// than one status per call: [`IMPORT_JOB_FAILED`] where the run's read of that source
    /// diagnosed a failure of it, and [`IMPORT_JOB_COMPLETED`] where it diagnosed none.
    pub fn status(&self, now: SystemTime) -> &'static str {
        if now >= self.completion_due_at {
            match self.failure {
                Some(_) => IMPORT_JOB_FAILED,
                None => IMPORT_JOB_COMPLETED,
            }
        } else if now >= self.processing_due_at {
            IMPORT_JOB_PROCESSING
        } else {
            IMPORT_JOB_CREATED
        }
    }

    /// Whether the run's own read of its Amazon S3 data source is still owed at `now`:
    /// the run has reached its terminal transition and nothing has answered about that
    /// source yet.
    pub fn source_resolution_falls_due(&self, now: SystemTime) -> bool {
        now >= self.completion_due_at && !self.source_answered
    }

    /// When the run completed, once the clock has made its terminal transition due.
    ///
    /// "The time stamp of when the import job was completed": a run that has not
    /// completed has no such instant, and one that has completed finished when it fell
    /// due, not when somebody first looked.
    pub fn completed_at(&self, now: SystemTime) -> Option<SystemTime> {
        (now >= self.completion_due_at).then_some(self.completion_due_at)
    }

    /// The `ImportDestinationType` this job's destination selects.
    ///
    /// A stored job always carries exactly one destination, because that is what
    /// `CreateImportJob` accepts ([`import_destination_type`]).
    pub fn destination_type(&self) -> Option<&'static str> {
        import_destination_type(&self.destination)
    }

    /// This job's position in a listing of the account's import jobs: the instant it was
    /// created, then its id.
    ///
    /// The instant leads, so the listing is in creation order; the id breaks the tie, so
    /// the order is total even for two jobs created in the same millisecond. The key is a
    /// position rather than a record, which is what keeps a page token valid when the job
    /// it names is no longer there — the same reading as the other job kind's
    /// ([`ExportJobRecord::listing_key`]).
    pub fn listing_key(&self) -> String {
        let millis = self
            .created_at
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        // Fixed-width hexadecimal, so comparing two keys as text orders them the way
        // comparing the instants they carry would.
        format!("{millis:032x}:{}", self.job_id)
    }

    /// The modelled `ImportJobSummary` view of this record at `now`, as `ListImportJobs`
    /// reports it.
    ///
    /// Every member is read off the stored job — including the status, which is derived
    /// from the instants its run's transitions fall due at ([`Self::status`]), so a
    /// summary reports exactly what a read of the same job reports and nothing is written
    /// because the job was listed.
    pub fn summary(&self, now: SystemTime) -> crate::ops::types::ImportJobSummary {
        crate::ops::types::ImportJobSummary {
            job_id: Some(self.job_id.clone()),
            // "The destination of the import job": what the create supplied, reported
            // back as it was stored.
            import_destination: Some(self.destination.clone()),
            job_status: Some(self.status(now).to_string()),
            created_timestamp: Some(::aws_smithy_types::DateTime::from(self.created_at)),
            // What the job has really imported, counted on the record.
            processed_records_count: Some(self.processed_records_count),
            failed_records_count: Some(self.failed_records_count),
        }
    }
}

/// One accepted outbound message.
#[derive(Debug, Clone)]
pub struct SentMessage {
    /// The `MessageId` returned to the caller.
    pub message_id: String,
    /// The `From` address the message was accepted for.
    pub from: Option<String>,
    /// `Destination$ToAddresses`.
    pub to: Vec<String>,
    /// `Destination$CcAddresses`.
    pub cc: Vec<String>,
    /// `Destination$BccAddresses`.
    pub bcc: Vec<String>,
    /// The subject, for a simple message.
    pub subject: Option<String>,
    /// The body: the text part of a simple message, or the raw MIME data.
    pub body: Option<String>,
    /// The HTML part of a simple message, when the request carried one. A simple
    /// message may carry both parts, and the log records what was accepted.
    pub body_html: Option<String>,
    /// The configuration set the message was sent with.
    pub configuration_set_name: Option<String>,
    /// The tenant the message was sent THROUGH (`SendEmailRequest$TenantName`), or `None`
    /// for a send of the account itself. Resolved against the account's tenants before the
    /// message was accepted ([`unresolved_send_tenant`]), so a name recorded here is a
    /// tenant the account held at that moment — which is what the surfaces that report or
    /// filter a message by its tenant read: the message-insights export's `TenantName`
    /// field and the `TENANT_NAME` metric dimension.
    pub tenant_name: Option<String>,
    /// `ReplyToAddresses`.
    pub reply_to: Vec<String>,
    /// `FeedbackForwardingEmailAddress`: where the caller asked for bounce and
    /// complaint notifications. Nothing here delivers a bounce, so it is recorded
    /// as part of what was accepted rather than acted on.
    pub feedback_forwarding: Option<String>,
    /// `EmailTags`, in the order they were supplied.
    pub email_tags: Vec<(String, String)>,
    /// The message headers the send supplied for this message ("the list of message
    /// headers that will be added to the email message"), in the order they apply to it.
    /// A published email event reports them beside the headers SES assembles from the
    /// message's own members ([`event_publication::notification`]).
    pub headers: Vec<(String, String)>,
    /// When the message was accepted.
    pub sent_at: SystemTime,
}

impl SentMessage {
    /// Every recipient of the message, in `To`, `Cc`, `Bcc` order. SES counts one
    /// recipient as one message against the account's sending quota.
    pub fn recipients(&self) -> impl Iterator<Item = &String> {
        self.to.iter().chain(self.cc.iter()).chain(self.bcc.iter())
    }

    /// The tags the accepted message carries, as the modelled `MessageTagList`
    /// reports them: the name/value pairs the request supplied, in the order it
    /// supplied them.
    ///
    /// Only those: the tags the real service adds of its own accord describe a
    /// delivery attempt nothing here makes, so none is invented here.
    pub fn email_tag_list(&self) -> Vec<crate::ops::types::MessageTag> {
        self.email_tags
            .iter()
            .map(|(name, value)| crate::ops::types::MessageTag {
                name: name.clone(),
                value: value.clone(),
            })
            .collect()
    }

    /// The per-recipient insights of this message, one entry per recipient in
    /// `To`, `Cc`, `Bcc` order — the order [`Self::recipients`] walks.
    ///
    /// The only event an entry carries is the acceptance of the message, stamped
    /// with the instant it was accepted: nothing in this slice delivers a message,
    /// opens it, bounces it or complains about it, so there is no later event that
    /// has happened to report, and no event `Details`, which only a bounce or a
    /// complaint carries. `Isp` is the recipient's own mail provider, which is not
    /// observable without a delivery attempt, so it is absent too.
    pub fn insights(&self) -> Vec<crate::ops::types::EmailInsights> {
        let accepted = ::aws_smithy_types::DateTime::from(self.sent_at);
        self.recipients()
            .map(|destination| crate::ops::types::EmailInsights {
                destination: Some(destination.clone()),
                isp: None,
                events: Some(vec![crate::ops::types::InsightsEvent {
                    timestamp: Some(accepted),
                    r#type: Some(EVENT_TYPE_SEND.to_string()),
                    details: None,
                }]),
            })
            .collect()
    }
}

/// Per-account send counters.
#[derive(Debug, Clone, Copy)]
pub struct SendQuotaState {
    /// Recipients accepted within the trailing [`SEND_QUOTA_WINDOW`]. A view of
    /// the state's outbound-message log, recomputed by
    /// [`ComAmazonawsSesv2SimpleemailserviceV2State::settle`] rather than tallied,
    /// so it frees up as the window moves.
    pub sent_last_24h: f64,
    /// The account's 24-hour sending quota.
    pub max_24h: f64,
    /// The account's maximum send rate, in messages per second. Recorded as part
    /// of the account's limits for the reporting operations that answer it
    /// (`GetAccount`); no per-second throttle is applied anywhere in this slice.
    pub max_send_rate: f64,
}

impl SendQuotaState {
    /// The modelled `SendQuota` view of these counters, as the operations that
    /// report the account's sending limits answer it.
    pub fn send_quota(&self) -> crate::ops::types::SendQuota {
        crate::ops::types::SendQuota {
            max24_hour_send: self.max_24h,
            max_send_rate: self.max_send_rate,
            sent_last24_hours: self.sent_last_24h,
        }
    }
}

impl Default for SendQuotaState {
    /// The limits a new account has in the SES sandbox: 200 messages per 24-hour
    /// period at 1 message per second.
    fn default() -> Self {
        Self {
            sent_last_24h: 0.0,
            max_24h: 200.0,
            max_send_rate: 1.0,
        }
    }
}

/// The reputation status of an account nothing has acted against.
///
/// `GetAccount`'s `EnforcementStatus` documents it as "HEALTHY – There are no
/// reputation-related issues that currently impact your account", which is the
/// status of an account whose sending nobody has reviewed or paused.
pub const ENFORCEMENT_STATUS_HEALTHY: &str = "HEALTHY";

/// `ReviewStatus`'s value for a production-access request still under review, as
/// the model spells it (see `crate::ops::types::REVIEW_STATUS_VALUES`).
///
/// `ReviewDetails$Status` documents it as "PENDING – We have received your appeal
/// and are in the process of reviewing it".
pub const REVIEW_STATUS_PENDING: &str = "PENDING";

/// `ReviewStatus`'s value for a production-access request that has been decided in
/// the account's favour: "GRANTED – Your appeal has been reviewed and your
/// production access has been granted".
pub const REVIEW_STATUS_GRANTED: &str = "GRANTED";

/// `FeatureStatus`'s value for a feature that is on, as the model spells it (see
/// `crate::ops::types::FEATURE_STATUS_VALUES`).
pub const FEATURE_ENABLED: &str = "ENABLED";

/// `FeatureStatus`'s value for a feature that is off.
pub const FEATURE_DISABLED: &str = "DISABLED";

/// `PricingPlan`'s value for an account on no paid plan, one of the four values
/// `PutAccountPricingAttributesRequest$Plan` documents.
pub const PRICING_PLAN_NONE: &str = "NONE";

/// `AccountSharedPoolDeliveryMode`'s value for delivery that may also leave shared IP
/// addresses of other Regions of the same partition, as the model spells it (see
/// `crate::ops::types::ACCOUNT_SHARED_POOL_DELIVERY_MODE_VALUES`).
///
/// It is the mode an account nothing has configured delivers under:
/// `DeliveryAttributes$SharedPoolDeliveryMode` documents that "Accounts that start
/// sending in a Region on or after the launch of Global Shared Delivery use
/// `GLOBAL` in that Region", and an account this emulator starts holds no earlier
/// sending history.
pub const SHARED_POOL_DELIVERY_MODE_GLOBAL: &str = "GLOBAL";

/// `AccountSharedPoolDeliveryMode`'s value for delivery from shared IP addresses of
/// the sending Region only.
pub const SHARED_POOL_DELIVERY_MODE_REGIONAL: &str = "REGIONAL";

/// `SharedPoolDeliveryMode`'s extra value, which only a CONFIGURATION SET's delivery
/// configuration can carry: the set defers to whatever mode the account holds.
pub const SHARED_POOL_DELIVERY_MODE_ACCOUNT: &str = "ACCOUNT";

/// The account-level attributes of one SES account in one Region.
///
/// A single record rather than a table: every member of it is per account and
/// Region, and the emulator serves one account in one Region, so the account
/// operations all read and write this one record.
#[derive(Debug, Clone)]
pub struct AccountState {
    /// `SendingEnabled`: whether email sending is enabled for the account.
    pub sending_enabled: bool,
    /// `DedicatedIpAutoWarmupEnabled`: whether dedicated IPs associated with the
    /// account are warmed up automatically.
    pub dedicated_ip_auto_warmup_enabled: bool,
    /// `ProductionAccessEnabled`: false while the account is in the sandbox, where
    /// it may only send to verified identities.
    pub production_access_enabled: bool,
    /// `EnforcementStatus`: the account's reputation status.
    pub enforcement_status: String,
    /// `SuppressionAttributes$SuppressedReasons`: the reasons for which an address
    /// is added to the account-level suppression list automatically.
    pub suppressed_reasons: Vec<String>,
    /// `SuppressionAttributes$ValidationAttributes`, once a request has set them.
    pub suppression_validation: Option<crate::ops::types::SuppressionValidationAttributes>,
    /// `VdmAttributes$VdmEnabled`: the status of the account's VDM configuration.
    pub vdm_enabled: String,
    /// `VdmAttributes$DashboardAttributes`, once a request has carried them, held
    /// as supplied so a read reports exactly what was written.
    pub vdm_dashboard_attributes: Option<crate::ops::types::DashboardAttributes>,
    /// `VdmAttributes$GuardianAttributes`, once a request has carried them, held as
    /// supplied for the same reason.
    pub vdm_guardian_attributes: Option<crate::ops::types::GuardianAttributes>,
    /// `PricingAttributes$CurrentPlan`: the plan currently active on the account.
    pub pricing_plan: String,
    /// `PricingAttributes$NextPlan`: the plan a scheduled change will activate.
    /// `NextPlan` "is empty when no scheduled change is pending", so it is absent
    /// until a change is scheduled.
    pub next_pricing_plan: Option<String>,
    /// `DeliveryAttributes$SharedPoolDeliveryMode`: the shared-pool delivery mode the
    /// account's own email is delivered under, which
    /// `PutAccountDeliveryAttributes` is what writes.
    pub shared_pool_delivery_mode: String,
    /// `Details`: the account details a request supplied, absent until one does.
    pub details: Option<crate::ops::types::AccountDetails>,
    /// When the decision on a production-access request that is still under review
    /// falls due, and `None` when no request is under review. The request is
    /// reported meanwhile as `Details$ReviewDetails` with the `PENDING` status;
    /// [`Self::settle`] is what turns it into the decision.
    pub production_access_review_due_at: Option<SystemTime>,
    /// Monotonic counter feeding [`Self::next_review_case_id`], so two
    /// production-access requests made in the same millisecond still get different
    /// case ids.
    pub review_case_sequence: u64,
}

impl Default for AccountState {
    /// The attributes of an account nobody has configured yet.
    ///
    /// Email sending and dedicated-IP auto warm-up are on — SES turns both on for
    /// an account and the account operations are what switch them off — the account
    /// is in the sandbox (which is what the sandbox [`SendQuotaState::default`]
    /// limits go with), its reputation is unenforced, and the account-level
    /// suppression list is on for both of its documented reasons. VDM is off until
    /// a request enables it, no pricing plan is active, and no account details have
    /// been supplied.
    fn default() -> Self {
        Self {
            sending_enabled: true,
            dedicated_ip_auto_warmup_enabled: true,
            production_access_enabled: false,
            enforcement_status: ENFORCEMENT_STATUS_HEALTHY.to_string(),
            suppressed_reasons: crate::ops::types::SUPPRESSION_LIST_REASON_VALUES
                .iter()
                .map(|reason| (*reason).to_string())
                .collect(),
            suppression_validation: None,
            vdm_enabled: FEATURE_DISABLED.to_string(),
            vdm_dashboard_attributes: None,
            vdm_guardian_attributes: None,
            pricing_plan: PRICING_PLAN_NONE.to_string(),
            next_pricing_plan: None,
            shared_pool_delivery_mode: SHARED_POOL_DELIVERY_MODE_GLOBAL.to_string(),
            details: None,
            production_access_review_due_at: None,
            review_case_sequence: 0,
        }
    }
}

impl AccountState {
    /// The modelled `SuppressionAttributes` view of this record.
    ///
    /// One projection shared by every operation that reports the account's
    /// suppression preferences, so a write and the read after it cannot disagree.
    pub fn suppression_attributes(&self) -> crate::ops::types::SuppressionAttributes {
        crate::ops::types::SuppressionAttributes {
            suppressed_reasons: Some(self.suppressed_reasons.clone()),
            validation_attributes: self.suppression_validation.clone(),
        }
    }

    /// The modelled `VdmAttributes` view of this record.
    ///
    /// The nested dashboard and guardian attributes carry one feature status each,
    /// and a sub-attribute nothing has carried is reported as absent rather than as
    /// a status this account does not hold.
    pub fn vdm_attributes(&self) -> crate::ops::types::VdmAttributes {
        crate::ops::types::VdmAttributes {
            vdm_enabled: self.vdm_enabled.clone(),
            dashboard_attributes: self.vdm_dashboard_attributes.clone(),
            guardian_attributes: self.vdm_guardian_attributes.clone(),
        }
    }

    /// The modelled `PricingAttributes` view of this record.
    pub fn pricing_attributes(&self) -> crate::ops::types::PricingAttributes {
        crate::ops::types::PricingAttributes {
            current_plan: Some(self.pricing_plan.clone()),
            next_plan: self.next_pricing_plan.clone(),
        }
    }

    /// The modelled `DeliveryAttributes` view of this record.
    ///
    /// One projection shared by every operation that reports the account's delivery
    /// attributes, so a write and the read after it cannot disagree.
    pub fn delivery_attributes(&self) -> crate::ops::types::DeliveryAttributes {
        crate::ops::types::DeliveryAttributes {
            shared_pool_delivery_mode: Some(self.shared_pool_delivery_mode.clone()),
        }
    }

    /// Whether a production-access request is still under review at `now`.
    ///
    /// A request whose decision is already due is not under review any more, even
    /// if nothing has read the account since: [`Self::settle`] is what records the
    /// decision, and it is owed at the same instant either way.
    pub fn production_access_review_pending(&self, now: SystemTime) -> bool {
        self.production_access_review_due_at
            .is_some_and(|due_at| now < due_at)
    }

    /// Record the decision the clock owes a production-access request at `now`.
    ///
    /// The review of a request SES has received ends in a decision, so the request
    /// is reported as `PENDING` only until its decision falls due; from then on the
    /// account reports the granted access and the review that granted it, whether or
    /// not anybody called in between. The case the request opened keeps its id: the
    /// decision is on that case, not on a new one.
    pub fn settle(&mut self, now: SystemTime) {
        let Some(due_at) = self.production_access_review_due_at else {
            return;
        };
        if now < due_at {
            return;
        }
        self.production_access_review_due_at = None;
        self.production_access_enabled = true;
        if let Some(review) = self
            .details
            .as_mut()
            .and_then(|details| details.review_details.as_mut())
        {
            review.status = Some(REVIEW_STATUS_GRANTED.to_string());
        }
    }

    /// Mint the id of the support case a production-access request opens.
    ///
    /// `ReviewDetails$CaseId` is documented only as "the associated support center
    /// case ID (if any)" and carries no modelled format, so the shape mirrors the
    /// case id AWS Support hands back while the only guarantee this makes is the one
    /// a client depends on: the id identifies one request and is stable for as long
    /// as the account reports that request's review (it is minted once, when the
    /// request is submitted, and read back from the record afterwards). Uniqueness is
    /// structural — the monotonic sequence number is part of the id — so two requests
    /// submitted in the same millisecond still differ.
    pub fn next_review_case_id(&mut self, account_id: &str, now: SystemTime) -> String {
        self.review_case_sequence += 1;
        let millis = now
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        let sequence = self.review_case_sequence;
        format!("case-{account_id}-{millis:012x}-{sequence:08x}")
    }
}

/// The domain of the Amazon SES mailbox SIMULATOR, whose mailboxes
/// (`success@`, `bounce@`, `complaint@`, `ooto@`, `suppressionlist@`) SES answers for
/// itself: they belong to no account, so no account verifies them as identities.
///
/// The one statement of that domain in this crate is the one the simulated answers are
/// decided by ([`event_publication::outcome::MAILBOX_SIMULATOR_DOMAIN`]); this names it so
/// the identity check and those answers cannot disagree about which mailboxes are the
/// simulator's.
pub const MAILBOX_SIMULATOR_DOMAIN: &str = event_publication::outcome::MAILBOX_SIMULATOR_DOMAIN;

/// Whether `address` is a mailbox of the SES mailbox simulator
/// ([`MAILBOX_SIMULATOR_DOMAIN`]).
///
/// The domain part is compared the way DNS compares a name ([`domain_key`]), so the
/// simulator is the same mailbox in any spelling.
pub fn is_mailbox_simulator(address: &str) -> bool {
    identity_domain(address)
        .eq_ignore_ascii_case(MAILBOX_SIMULATOR_DOMAIN)
        && addr_spec(address).contains('@')
}

/// The addr-spec of a `From`-style address.
///
/// An email address member may carry RFC 5322's display-name form, `Sender Name
/// <sender@example.com>`, which names the same mailbox as the bare address: the
/// mailbox is what the identity table is keyed by, so the angle-bracketed
/// addr-spec is what a lookup compares. A value with no angle brackets is already
/// an addr-spec.
pub fn addr_spec(address: &str) -> &str {
    match (address.rfind('<'), address.rfind('>')) {
        (Some(open), Some(close)) if open < close => address[open + 1..close].trim(),
        _ => address.trim(),
    }
}

/// How this service words `NotFoundException` for an email identity it does not
/// hold, on the operations that QUOTE the identity in angle brackets.
///
/// One sentence — the fixed text `Email identity <` + `> does not exist.`, with the
/// `EmailIdentity` the request spelled as its only variable part — shared by the
/// identity reads (`GetEmailIdentity`, `GetEmailIdentityPolicies`) and the
/// certificate operations, which are the operations recorded answering the quoted
/// form, so a client sees the same wording whichever of them first tells it the
/// identity is gone.
///
/// The identity WRITES, and the delete, are recorded answering the same sentence
/// with the identity unquoted: that form is [`email_identity_not_found_unquoted`],
/// and the two must not be swapped — a client that strips the brackets off one
/// operation's sentence would read the wrong resource name out of the other's.
pub fn email_identity_not_found(identity: &str) -> String {
    format!("Email identity <{identity}> does not exist.")
}

/// How this service words `NotFoundException` for an email identity it does not
/// hold, on the operations that name the identity UNQUOTED.
///
/// The same sentence as [`email_identity_not_found`] without the angle brackets
/// around the value: the fixed text `Email identity ` + ` does not exist.`, with the
/// `EmailIdentity` the request spelled as its only variable part. `DeleteEmailIdentity`
/// and `CreateEmailIdentityPolicy` — the operations that address the identity RECORD
/// without quoting its name — are the ones recorded answering this form, stated once
/// here so no two of them word the one condition differently.
///
/// The attribute writes (`PutEmailIdentity*Attributes`) do NOT answer it: they classify
/// an identity they cannot resolve as invalid input, worded by
/// [`identity_is_not_a_sending_identity`].
pub fn email_identity_not_found_unquoted(identity: &str) -> String {
    format!("Email identity {identity} does not exist.")
}

/// How this service words `BadRequestException` for an `EmailIdentity` that an
/// attribute write cannot resolve to one of the account's identities.
///
/// The `PutEmailIdentity*Attributes` writes set a property OF a sending identity, so
/// the value they carry is an input SES validates — the identity it has to sign,
/// forward or send for — rather than a resource the call addresses: a value naming
/// none of the account's identities is refused as invalid input, with the fixed text
/// `Identity ` + ` is invalid. Must be a verified email address or domain.` and the
/// `EmailIdentity` the request spelled, unquoted, as its only variable part.
///
/// This is deliberately NOT [`email_identity_not_found_unquoted`]: that sentence, and
/// the `NotFoundException` carrying it, belong to the operations that address the
/// identity RECORD — the reads, the delete, and the policy and certificate operations
/// that hang a child off it — which keep answering 404. The DKIM write states the same
/// refusal in terms of the mechanism it configures ([`domain_not_verified_for_dkim`]);
/// this is the sentence for the writes that configure no one mechanism of its own.
pub fn identity_is_not_a_sending_identity(identity: &str) -> String {
    format!("Identity {identity} is invalid. Must be a verified email address or domain.")
}

/// How this service words `BadRequestException` when `PutEmailIdentityDkimAttributes`
/// is asked to set DKIM signing for an identity the account does not hold.
///
/// DKIM signs a DOMAIN's mail, so the sentence this refusal carries names the domain
/// rather than the identity the request spelled: the fixed text is `Domain ` + ` is not
/// verified for DKIM signing.`, and its only variable part is the domain the requested
/// identity belongs to ([`identity_domain`]) — the whole value for a domain identity,
/// and the domain part of an address. That is the sentence this surface is RECORDED
/// answering for this refusal (quoted in the resolved report under
/// `issues/things-that-now-work/`); the writes that configure no one mechanism of their
/// own state the same refusal of the identity instead
/// ([`identity_is_not_a_sending_identity`]).
pub fn domain_not_verified_for_dkim(identity: &str) -> String {
    format!(
        "Domain {} is not verified for DKIM signing.",
        identity_domain(identity)
    )
}

/// The key an identity is stored and looked up under.
///
/// A domain name is case-insensitive — DNS compares labels without regard to case
/// — so `Example.COM` and `example.com` name one identity, and an address is
/// authorised by the identity for its domain whatever case the request spells the
/// domain in. The local part of an address belongs to the receiving mailbox, so it
/// is kept exactly as supplied, as is the whole value on the record, which is what
/// every response reports.
///
/// The comparison itself is `service_common::ses_identities`', which the account's
/// identity registry keys by, so this surface and the registry cannot disagree about
/// which records are one identity of the account.
pub fn identity_key(identity: &str) -> String {
    ::service_common::ses_identities::identity_key(identity)
}

/// The key a domain name is compared under.
///
/// DNS compares labels without regard to case, so one domain written in any case
/// names the same thing, and nothing else about the value is normalised: a value
/// carrying what a domain name cannot carry — surrounding whitespace, say — is
/// still a different string from every domain, so it matches none of them. The value
/// kept on a record, and reported by every response, is the one the request spelled.
pub fn domain_key(domain: &str) -> String {
    domain.to_lowercase()
}

/// The domain an email identity belongs to: the domain part of an address, or the
/// whole value of a domain identity, as the identity spells it.
///
/// Compare two of them with [`domain_key`]; this is the value a response reports.
pub fn identity_domain(identity: &str) -> &str {
    let identity = addr_spec(identity);
    match identity.rsplit_once('@') {
        Some((_, domain)) => domain,
        None => identity,
    }
}

/// Whether `candidate` is `domain` itself or a domain under it.
///
/// Both arguments are [`domain_key`]s, so the names compare the way DNS compares
/// them. `example.com` covers `example.com` and `any.subdomain.of.it`, and nothing
/// else — not `notexample.com`, and not the empty name or `.example.com`, neither of
/// which names a domain under it.
pub fn domain_covers(domain: &str, candidate: &str) -> bool {
    if domain.is_empty() || candidate.is_empty() {
        return false;
    }
    candidate == domain
        || candidate
            .strip_suffix(domain)
            // The remainder is the subdomain part, so it ends at the separating dot
            // and must carry at least one label character before it: `.example.com`
            // names no subdomain of `example.com`.
            .is_some_and(|parent| parent.ends_with('.') && parent.len() > 1)
}

/// The longest a single DNS label may be, in characters (RFC 1035 §2.3.4).
const MAX_DOMAIN_LABEL_CHARS: usize = 63;

/// The longest a domain name may be written out, in characters: RFC 1035 §2.3.4
/// limits a name to 255 octets on the wire, of which 253 are the name's own
/// characters.
const MAX_DOMAIN_NAME_CHARS: usize = 253;

/// Whether `name` is spelled as a domain name.
///
/// DNS's preferred name syntax (RFC 1035 §2.3.1, §2.3.4): dot-separated labels, each
/// one of [`MAX_DOMAIN_LABEL_CHARS`] or fewer ASCII letters, digits and hyphens that
/// begins and ends with neither a hyphen nor a dot, and [`MAX_DOMAIN_NAME_CHARS`] or
/// fewer characters in all. A value carrying anything else — whitespace, an `@`, an
/// empty label — names no domain at all, so nothing can be sent from it.
pub fn domain_name_is_well_formed(name: &str) -> bool {
    if name.is_empty() || name.chars().count() > MAX_DOMAIN_NAME_CHARS {
        return false;
    }
    name.split('.').all(|label| {
        !label.is_empty()
            && label.chars().count() <= MAX_DOMAIN_LABEL_CHARS
            && !label.starts_with('-')
            && !label.ends_with('-')
            && label
                .chars()
                .all(|character| character.is_ascii_alphanumeric() || character == '-')
    })
}

/// Whether `candidate` names a domain strictly under `domain`.
///
/// Both arguments are [`domain_key`]s, so the names compare the way DNS compares
/// them. `example.com` has `mail.example.com` and `bounce.mail.example.com` under
/// it, and nothing else — not itself, not `notexample.com`, and not the empty name
/// or `.example.com`, neither of which names a domain at all.
pub fn domain_is_under(domain: &str, candidate: &str) -> bool {
    candidate != domain && domain_covers(domain, candidate)
}

/// Whether `address` is an address in `domain` or in one of its subdomains.
///
/// Both arguments are [`identity_key`]s, so the domain parts compare the way DNS
/// compares them. A domain covers `user@its.own.name` and `user@any.subdomain.of.it`,
/// and nothing else — `example.com` does not cover `notexample.com`.
pub fn address_in_domain(domain: &str, address: &str) -> bool {
    let Some((local, address_domain)) = address.rsplit_once('@') else {
        return false;
    };
    if local.is_empty() {
        return false;
    }
    domain_covers(domain, address_domain)
}

/// The Easy DKIM CNAME tokens SES publishes for `domain`.
///
/// `DkimAttributes$Tokens` is documented as "a set of unique strings" the caller
/// turns into CNAME records, with no format of its own, so each token here is a
/// digest of the domain and the token's index: distinct per domain and per index,
/// and stable for as long as the identity holds it (the set is computed once, at
/// create, and then stored), which is what a client adding CNAME records depends
/// on. Two 64-bit digests make up the [`EASY_DKIM_TOKEN_LEN`] hex characters.
///
/// The digest is `std`'s, whose output is explicitly NOT guaranteed to be the same
/// across Rust releases — nothing here relies on that: a token set is computed once,
/// at create, and read back from the record afterwards, so no caller and no test may
/// assert a literal token value.
pub fn easy_dkim_tokens(domain: &str) -> Vec<String> {
    (0..EASY_DKIM_TOKEN_COUNT)
        .map(|index| {
            let mut token = String::with_capacity(EASY_DKIM_TOKEN_LEN);
            for salt in [0_u64, u64::from_be_bytes(*b"EasyDKIM")] {
                let mut hasher = DefaultHasher::new();
                salt.hash(&mut hasher);
                domain.hash(&mut hasher);
                index.hash(&mut hasher);
                write!(token, "{:016x}", hasher.finish())
                    .expect("writing to a String cannot fail");
            }
            token
        })
        .collect()
}

/// The tag `ListContactLists`'s own tokens carry.
pub const CONTACT_LISTS_PAGE_TOKEN_TAG: &str = "sesv2:contact-lists:";

/// The tag `ListContacts`'s own tokens carry.
pub const CONTACTS_PAGE_TOKEN_TAG: &str = "sesv2:contacts:";

/// The tag `ListEmailIdentityCertificates`'s own tokens carry.
pub const IDENTITY_CERTIFICATES_PAGE_TOKEN_TAG: &str = "sesv2:identity-certificates:";

/// The tag `ListSuppressedDestinations`'s own tokens carry.
pub const SUPPRESSED_DESTINATIONS_PAGE_TOKEN_TAG: &str = "sesv2:suppressed-destinations:";

/// The tag `ListMultiRegionEndpoints`'s own tokens carry.
pub const MULTI_REGION_ENDPOINTS_PAGE_TOKEN_TAG: &str = "sesv2:multi-region-endpoints:";

/// The tag `ListReputationEntities`'s own tokens carry.
pub const REPUTATION_ENTITIES_PAGE_TOKEN_TAG: &str = "sesv2:reputation-entities:";

/// The tag `ListResourceTenants`'s own tokens carry.
pub const RESOURCE_TENANTS_PAGE_TOKEN_TAG: &str = "sesv2:resource-tenants:";

/// The tag `ListTenantResources`'s own tokens carry.
pub const TENANT_RESOURCES_PAGE_TOKEN_TAG: &str = "sesv2:tenant-resources:";

/// The tag `ListTenants`'s own tokens carry.
pub const TENANTS_PAGE_TOKEN_TAG: &str = "sesv2:tenants:";

/// The tag `ListDedicatedIpPools`'s own tokens carry.
pub const DEDICATED_IP_POOLS_PAGE_TOKEN_TAG: &str = "sesv2:dedicated-ip-pools:";

/// The tag `GetDedicatedIps`'s own tokens carry.
pub const DEDICATED_IPS_PAGE_TOKEN_TAG: &str = "sesv2:dedicated-ips:";

/// The tag `ListExportJobs`'s own tokens carry.
pub const EXPORT_JOBS_PAGE_TOKEN_TAG: &str = "sesv2:export-jobs:";

/// The tag `ListImportJobs`'s own tokens carry.
pub const IMPORT_JOBS_PAGE_TOKEN_TAG: &str = "sesv2:import-jobs:";

/// The tag `ListRecommendations`'s own tokens carry.
pub const RECOMMENDATIONS_PAGE_TOKEN_TAG: &str = "sesv2:recommendations:";

/// Encode a `NextToken` that resumes the listing tagged `tag` after `last_returned`.
///
/// The token is opaque to the caller: it is the listing's tag and the last key
/// returned, base64'd — decodable by [`decode_tagged_page_token`] under the same tag
/// and by nothing else, so one listing's token is not a position in another's.
pub fn encode_tagged_page_token(tag: &str, last_returned: &str) -> String {
    ::aws_smithy_types::base64::encode(format!("{tag}{last_returned}"))
}

/// Decode a `NextToken` produced by [`encode_tagged_page_token`] under `tag`. `None`
/// when the caller made the token up, or when it belongs to another listing.
pub fn decode_tagged_page_token(tag: &str, token: &str) -> Option<String> {
    let decoded = String::from_utf8(::aws_smithy_types::base64::decode(token).ok()?).ok()?;
    decoded.strip_prefix(tag).map(ToString::to_string)
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// `None` means the engine is not the shared in-memory engine, so this service has
/// no store at all; a handler answers that with the scaffold's 501, never with a
/// fresh engine and never with a modelled error that would read as a real service
/// state (a throttle an SDK retries, say).
///
/// Every call RECONCILES this service's identity table against the account's identity
/// registry first ([`reconcile_identities`]), so an operation served here answers for
/// every identity the account holds — including the ones recorded through the classic
/// SES surface — and for none it has deleted. It then resolves what the account's own
/// DNS publishes for each identity still waiting on its DKIM records
/// ([`resolve_dkim_publication`]), so a domain identity is verified by the records its
/// owner published and by nothing else. Its configuration sets are reconciled
/// against the account's configuration-set registry the same way
/// ([`reconcile_configuration_sets`]), and for the same reason. Its email templates are
/// reconciled against the account's email-template registry the same way
/// ([`reconcile_templates`]), and for the same reason.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsSesv2SimpleemailserviceV2State>> {
    let state = own_state(engine)?;
    let registry = identity_registry(engine)?;
    reconcile_identities(&state, &registry, SystemTime::now());
    resolve_dkim_publication(&state, engine, SystemTime::now());
    let configuration_sets = configuration_set_registry(engine)?;
    reconcile_configuration_sets(&state, &configuration_sets, SystemTime::now());
    let templates = template_registry(engine)?;
    reconcile_templates(&state, &templates);
    Some(state)
}

/// This service's store WITHOUT reconciling it against the account's identity registry
/// first.
///
/// What a writer that has just changed a shared fact publishes FROM
/// ([`publish_identity`]): reconciling there would read the registry's older copy of
/// that fact back over the change before it was published, so the publication would
/// carry what the registry already held instead of what the call wrote. Every other
/// caller wants [`service_state`].
fn own_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsSesv2SimpleemailserviceV2State>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsSesv2SimpleemailserviceV2State>)
}

/// Publish the identity `name` as this service now holds it — its verification, its
/// Easy DKIM setup and its sending-authorization policies — to the account's identity
/// registry, so every other API over the account's identities reports what this one
/// last wrote.
///
/// What an operation that CHANGES a shared fact about an identity calls once it has
/// written it. Changes nothing when the account holds no such identity, and writes
/// nothing but the registry's record of it.
pub fn publish_identity<Eng: Send + Sync + 'static>(engine: &Eng, name: &str) {
    let Some(state) = own_state(engine) else {
        return;
    };
    let Some(registry) = identity_registry(engine) else {
        return;
    };
    // The lock order of this service: its own store, then the registry.
    let state = state.lock();
    let mut registry = registry.lock();
    let Some(held) = state.identities.get(&identity_key(name)) else {
        return;
    };
    registry.publish(registry_record(held));
}

/// The account's identity registry in the shared engine, which this service and the
/// classic SES surface both publish to and read: call it with a handler's
/// `&*self.engine`.
///
/// An operation that CREATES or DELETES an email identity publishes the change here,
/// so the other surface answers for it from the moment the call returns. Take this
/// lock while holding this service's own store, never the other way round, so two
/// callers on different surfaces cannot wait on each other.
pub fn identity_registry<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<::service_common::ses_identities::SesIdentityDirectory>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<
            ::service_common::ses_identities::SesIdentityDirectory,
        >)
}

/// The `NextToken` the identity listing hands back to resume at the identity the NEXT
/// page starts at, and the reading of one it is handed.
///
/// The registry's, not this surface's: the APIs over the account's identities page one
/// directory, so one token form serves them all
/// ([`::service_common::ses_identities::page_token`]).
pub use ::service_common::ses_identities::{
    page_cursor as identity_page_cursor, page_token as identity_page_token,
};

/// The key this account's identity-listing tokens are authenticated under
/// ([`identity_page_token`]), or `None` when the engine holds no registry at all.
///
/// Taken by value — [`PageTokenKey`] is `Copy` — so a listing reads the key and then goes
/// on to read the identities it is about to report without holding the registry's lock.
pub fn identity_page_token_key<Eng: Send + Sync + 'static>(engine: &Eng) -> Option<PageTokenKey> {
    Some(identity_registry(engine)?.lock().page_token_key())
}

/// Bring this service's identity table and the account's identity registry back into
/// agreement at `now`, so every API over the account's identities answers for ONE set
/// of them.
///
/// The reconciliation itself is the registry's
/// ([`::service_common::ses_identities::SesIdentityDirectory::reconcile`]), written
/// once there rather than per surface; what this service contributes is its
/// [`::service_common::ses_identities::AccountIdentityRecord`] impl below.
fn reconcile_identities(
    state: &::engine::ServiceState<ComAmazonawsSesv2SimpleemailserviceV2State>,
    registry: &::engine::ServiceState<::service_common::ses_identities::SesIdentityDirectory>,
    now: SystemTime,
) {
    // The lock order of this service: its own store, then the registry.
    let mut state = state.lock();
    let mut registry = registry.lock();
    registry.reconcile(&mut state.identities, now);
}

impl ::service_common::ses_identities::AccountIdentityRecord for EmailIdentityRecord {
    /// This service's table is keyed by [`identity_key`], the way its own operations
    /// look an identity up.
    fn table_key(name: &str) -> String {
        identity_key(name)
    }

    fn identity_name(&self) -> &str {
        &self.name
    }

    fn is_registered(&self) -> bool {
        self.registered
    }

    fn mark_registered(
        &mut self,
        published: &::service_common::ses_identities::SesIdentityRecord,
    ) {
        // The TXT verification token the registry holds for a domain identity is not
        // one this surface's model carries, so there is nothing of it to keep here.
        let _ = published;
        self.registered = true;
    }

    /// The sending-authorization policies the ACCOUNT's identity holds, which this API
    /// reports whichever API attached them, and the Easy DKIM setup it holds, whose
    /// tokens are the records its owner publishes.
    fn take_shared(
        &mut self,
        published: &::service_common::ses_identities::SesIdentityRecord,
    ) {
        if let Some(policies) = &published.policies {
            if self.policies != *policies {
                self.policies = policies.clone();
            }
        }
        // An Easy DKIM setup the account gained through another API over these
        // identities AFTER this record was written is the setup whose tokens the
        // identity's owner publishes, so this record reports it instead of reporting
        // none. A record already carrying a token set of its own keeps it: the first
        // setup stands, because those records may already be in DNS.
        let gained = published
            .dkim
            .as_ref()
            .filter(|_| self.dkim.tokens.is_empty());
        if let Some(shared) = gained {
            self.dkim = DkimState::shared(&identity_key(&self.name), shared);
        }
    }

    fn published(&self) -> ::service_common::ses_identities::SesIdentityRecord {
        registry_record(self)
    }

    fn adopt(
        published: &::service_common::ses_identities::SesIdentityRecord,
        now: SystemTime,
    ) -> Self {
        Self::adopted(published, now)
    }
}

/// How the account registry carries the identity `record`: the identity as it was
/// created, its class, the verification this service holds for it, and the Easy DKIM
/// setup it holds for it.
pub fn registry_record(
    record: &EmailIdentityRecord,
) -> ::service_common::ses_identities::SesIdentityRecord {
    use ::service_common::ses_identities::{SesDkimSetup, SesIdentityKind, SesIdentityRecord};

    SesIdentityRecord {
        name: record.name.clone(),
        kind: match record.identity_kind {
            IdentityKind::EmailAddress => SesIdentityKind::EmailAddress,
            IdentityKind::Domain => SesIdentityKind::Domain,
        },
        verification: registry_state(record.verification_status),
        // A verification nothing owes a move to — one that can never complete, or one
        // adopted in a state the account owes none to — carries no due instant, so no
        // reader of the registry completes a verification nothing was waiting on.
        verification_due: (record.verification_status == SesStatus::Pending
            && record.verification_move_owed
            && record.verification_can_complete())
            .then_some(record.verification_due_at),
        // The TXT value a domain identity's owner publishes is the registry's own: no
        // operation of this surface reports it, and the registry mints it once so the
        // classic surface answers one stable value for the identity.
        verification_token: None,
        // The Easy DKIM setup belongs to the account's identity rather than to this
        // API, so the setup SES generated here — the tokens the owner publishes as
        // CNAME records, where the search for them stands, and whether messages from
        // the identity are signed — is published for every surface over these
        // identities to report. An identity SES generated no token set for carries no
        // setup to share: one with no DKIM configuration at all, and one whose keys the
        // caller owns (BYODKIM), whose stored token is a selector rather than records
        // another API could report.
        dkim: (record.dkim.origin == DkimOrigin::AwsSes && !record.dkim.tokens.is_empty()).then(
            || SesDkimSetup {
                signing_enabled: record.dkim.signing_enabled,
                status: registry_state(record.dkim.status),
                tokens: record.dkim.tokens.clone(),
            },
        ),
        // The policies that authorize other principals to send through the identity
        // belong to the account's identity too, and this API attaches and reports them,
        // so the set it holds is published for every surface over these identities to
        // report.
        policies: Some(record.policies.clone()),
    }
}

/// The registry's spelling of one of this service's statuses.
fn registry_state(
    status: SesStatus,
) -> ::service_common::ses_identities::SesVerificationState {
    use ::service_common::ses_identities::SesVerificationState;

    match status {
        SesStatus::Pending => SesVerificationState::Pending,
        SesStatus::Success => SesVerificationState::Success,
        SesStatus::NotStarted => SesVerificationState::NotStarted,
        SesStatus::Failed => SesVerificationState::Failed,
        SesStatus::TemporaryFailure => SesVerificationState::TemporaryFailure,
    }
}

/// This service's spelling of a registry verification state.
fn adopted_status(state: ::service_common::ses_identities::SesVerificationState) -> SesStatus {
    use ::service_common::ses_identities::SesVerificationState;

    match state {
        SesVerificationState::Pending => SesStatus::Pending,
        SesVerificationState::Success => SesStatus::Success,
        SesVerificationState::NotStarted => SesStatus::NotStarted,
        SesVerificationState::Failed => SesStatus::Failed,
        SesVerificationState::TemporaryFailure => SesStatus::TemporaryFailure,
    }
}
