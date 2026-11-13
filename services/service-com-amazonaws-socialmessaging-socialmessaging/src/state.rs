// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-socialmessaging-socialmessaging service.
//!
//! One store holds the WhatsApp Business Account (WABA) control plane: the linked
//! accounts, the phone numbers registered under them, the signups that are still
//! in progress, and the messages that were sent. Every handler reads and writes
//! THIS store through [`service_state`], so a write made by one request is what the
//! next request reads.

use std::collections::{HashMap, VecDeque};
use std::time::{Duration, SystemTime};

use sha2::{Digest, Sha256};

use crate::ops::types::{
    AccessDeniedException, InvalidParametersException, LinkedWhatsAppBusinessAccount,
    LinkedWhatsAppBusinessAccountIdMetaData, LinkedWhatsAppBusinessAccountSummary, Tag,
    WhatsAppBusinessAccountEventDestination, WhatsAppCallSettings, WhatsAppPhoneNumberDetail,
    WhatsAppPhoneNumberSummary,
};
use spki::der::Decode;

/// This service's ARN namespace and SigV4 signing name (`OPERATIONS.md`: "sigv4
/// signing name: social-messaging", "ARN namespace: social-messaging").
pub const ARN_SERVICE: &str = "social-messaging";

/// The region an ARN carries when the request is not SigV4-signed, so an
/// unsigned call still mints a well-formed ARN.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The `waba-` prefix the model's `LinkedWhatsAppBusinessAccountId` pattern
/// `(^waba-.*$)|(^arn:.*:waba/[0-9a-zA-Z]+$)` requires of the short form.
pub const LINKED_ACCOUNT_ID_PREFIX: &str = "waba-";

/// The `phone-number-id-` prefix the model's `WhatsAppPhoneNumberId` pattern
/// `(^phone-number-id-.*$)|(^arn:.*:phone-number-id/[0-9a-zA-Z]+$)` requires.
pub const PHONE_NUMBER_ID_PREFIX: &str = "phone-number-id-";

/// Members of the model's `RegistrationStatus` enum (see
/// `crate::ops::types::REGISTRATION_STATUS_VALUES`, `["COMPLETE", "INCOMPLETE"]`).
pub const REGISTRATION_STATUS_COMPLETE: &str = "COMPLETE";
/// See [`REGISTRATION_STATUS_COMPLETE`].
pub const REGISTRATION_STATUS_INCOMPLETE: &str = "INCOMPLETE";

/// What Meta reports for a phone number that has no messaging history yet, so no
/// quality signal exists to rate it. Fits `WhatsAppPhoneNumberQualityRating`'s
/// `len ..10`.
pub const QUALITY_RATING_UNKNOWN: &str = "UNKNOWN";

/// The page size `ListLinkedWhatsAppBusinessAccounts` uses when the caller sends
/// no `maxResults`: the maximum the model's `MaxResults` range (`1..100`) allows.
pub const MAX_RESULTS_LIMIT: i32 = 100;

/// How many linked accounts this emulator holds. Emulator-chosen (AWS publishes no
/// WABA quota through this API) and reported with `AssociateWhatsAppBusinessAccount`'s
/// own modelled `LimitExceededException`, which says exactly this: "The request was
/// denied because it would exceed one or more service quotas or limits." Every
/// account is created by a request, so without this bound a client could grow the
/// process without limit.
pub const MAX_LINKED_ACCOUNTS: usize = 128;

/// How many phone numbers one linked account holds. Emulator-chosen for the same
/// reason, and refused with the same modelled error. With [`MAX_LINKED_ACCOUNTS`]
/// it bounds `phone_numbers` and `arn_index` too.
pub const MAX_PHONE_NUMBERS_PER_ACCOUNT: usize = 32;

/// How many signups may be in progress at once. Emulator-chosen, refused with the
/// same modelled error, so a stream of callback-only requests cannot grow the store.
pub const MAX_PENDING_SIGNUPS: usize = 64;

/// How long a signup stays finalizable. Emulator-chosen (AWS publishes no lifetime
/// for `associateInProgressToken`): a signup nobody finalized within this span is
/// dropped, so an abandoned one does not occupy the store forever.
pub const PENDING_SIGNUP_TTL: Duration = Duration::from_secs(15 * 60);

/// How many accepted sends the store retains, oldest evicted first. Emulator-chosen:
/// `SendWhatsAppMessage` appends a caller-sized blob per call and models no quota
/// error, so the record is trimmed rather than the call refused.
pub const MAX_RETAINED_SENT_MESSAGES: usize = 128;

/// The bytes of retained message bodies, which with [`MAX_RETAINED_SENT_MESSAGES`]
/// bounds the send record whatever size the modelled `1..2048000` blob arrives at.
/// The newest send is always retained, even on its own.
pub const MAX_RETAINED_SENT_MESSAGE_BYTES: usize = 8 * 1024 * 1024;

/// How many accepted conversion events the store retains, oldest evicted first.
/// Emulator-chosen for the same reason as [`MAX_RETAINED_SENT_MESSAGES`]:
/// `SendWhatsAppConversionEvent` appends a caller-sized blob per call and models no quota
/// error, so the record is trimmed rather than the call refused.
pub const MAX_RETAINED_CONVERSION_EVENTS: usize = 128;

/// The bytes of retained conversion-event payloads, which with
/// [`MAX_RETAINED_CONVERSION_EVENTS`] bounds the record whatever size the modelled
/// `1..1024000` blob arrives at. The newest event is always retained, even on its own.
pub const MAX_RETAINED_CONVERSION_EVENT_BYTES: usize = 8 * 1024 * 1024;

/// How many digits a minted dataset id carries.
///
/// The model's `WhatsAppDatasetId` is `len 10..20, pattern ^[0-9]+$` and documents a
/// Meta-generated "numeric string of 10 to 20 digits"; 15 sits inside that range at both
/// ends, so a minted id satisfies the constraint with room on either side.
const DATASET_ID_DIGITS: u32 = 15;

/// How many digits a minted Meta-side resource id carries — a Flow id (`MetaFlowId`,
/// `len 1..100, pattern ^[0-9]+$`) and a message-template id (`MetaTemplateId`, the same
/// constraint). Both ranges hold [`DATASET_ID_DIGITS`] with room on either side, so one
/// width serves every numeric id Meta hands back and the ids all read alike.
const META_ID_DIGITS: u32 = DATASET_ID_DIGITS;

/// The `MetaFlowStatus` a Flow starts in: `CreateWhatsAppFlow` documents that "the Flow is
/// created in DRAFT status". One of the values that operation's doc lists — DRAFT,
/// PUBLISHED, DEPRECATED, BLOCKED, THROTTLED — and inside `MetaFlowStatus`'s `len 1..20`.
pub const FLOW_STATUS_DRAFT: &str = "DRAFT";

/// The `MetaFlowStatus` a Flow reaches when the create asked for it: "if `publish` is set
/// to `true` and a valid `flowJson` is provided, the Flow is published immediately"
/// (`CreateWhatsAppFlow`'s doc). Immediately, so no transition is owed on it.
pub const FLOW_STATUS_PUBLISHED: &str = "PUBLISHED";

/// The `templateStatus` a submitted message template carries while Meta is still reviewing
/// it (`CreateWhatsAppMessageTemplate`'s output member documents "the status of the created
/// template, such as PENDING or APPROVED").
pub const TEMPLATE_STATUS_PENDING: &str = "PENDING";

/// The `templateStatus` a reviewed template carries once the review is through — the other
/// value [`TEMPLATE_STATUS_PENDING`]'s member documents.
pub const TEMPLATE_STATUS_APPROVED: &str = "APPROVED";

/// How long a submitted template stays [`TEMPLATE_STATUS_PENDING`] before the review it is
/// waiting on is due. Emulator-chosen: Meta publishes no review time, and the span exists
/// so a caller reading the template straight after creating it still sees the status the
/// create reported. Nothing sleeps for it — the due instant is stored and the status is
/// read off the clock (see [`MessageTemplateRecord::status_at`]).
pub const TEMPLATE_REVIEW_SPAN: Duration = Duration::from_secs(3);

/// The `businessPublicKeySignatureStatus` a stored business public key reports.
///
/// The member documents two values, VALID and MISMATCH, and the mismatch is between the
/// key WhatsApp holds and the one the service holds. This emulator is both sides of that
/// comparison — the key a read reports is the one the write stored — so a stored key is
/// always the key that was set, and the status it reports is this one.
pub const BUSINESS_PUBLIC_KEY_SIGNATURE_VALID: &str = "VALID";

/// The message this service's `InvalidParametersException` carries: the model documents the
/// error as "one or more parameters provided to the action are not valid", and that is what
/// a request whose members cannot hold together is refused with — a constant, like every
/// other error text this service states, so nothing of the request reaches it.
pub const INVALID_PARAMETERS_MESSAGE: &str =
    "One or more parameters provided to the action are not valid.";

/// The member a request submits business public key material in, spelled as the wire
/// spells it (`smithy_query(query="operation", operation="PutWhatsAppBusinessPublicKey")`:
/// `businessPublicKey` -> `BusinessPublicKeyPem`). It is the only input member of this
/// service that carries key material.
pub const BUSINESS_PUBLIC_KEY_MEMBER: &str = "businessPublicKey";

/// The sentence a request whose key material is not a public key is refused with: the
/// offending input MEMBER NAME, as the wire spells it, then fixed text. The member name is
/// the only variable part — nothing of the key itself, of the number it was submitted for,
/// or of the account reaches it.
pub fn invalid_public_key_message(member: &str) -> String {
    format!("{member} is not a valid X.509 RSA public key.")
}

/// Refuse key material that is not a PEM-encoded X.509 RSA public key.
///
/// The member is documented as "the PEM-encoded 2048-bit RSA public key to set", and the
/// model constrains it only by length, so the shape of the material is judged here: the
/// value has to carry a PEM envelope labelled as a public key ([`PUBLIC_KEY_PEM_LABEL`]),
/// that envelope has to hold an X.509 `SubjectPublicKeyInfo`, and that structure has to
/// name the RSA algorithm and hold an RSA public key. Anything else — text that is not PEM
/// at all, an envelope holding something that is not a public key structure, or a public key
/// of another algorithm — names no RSA key to set.
///
/// The decision is made from the REQUEST alone, so an operation asks here BEFORE it
/// resolves the resource the request names: a caller whose key material cannot be accepted
/// learns nothing from the answer about what the account holds, and the store is left
/// exactly as it was found.
///
/// The envelope and the structures inside it are read by the crates that implement those
/// formats ([`pem`], [`spki`], [`pkcs1`]), not by anything hand-rolled here: the value is
/// caller-controlled input.
pub fn require_x509_rsa_public_key(
    member: &str,
    value: &str,
) -> Result<(), InvalidParametersException> {
    if is_x509_rsa_public_key(value) {
        return Ok(());
    }
    Err(InvalidParametersException {
        message: Some(invalid_public_key_message(member)),
    })
}

/// The PEM label an X.509 `SubjectPublicKeyInfo` is enveloped under: RFC 7468 section 13
/// gives "PUBLIC KEY" for that structure, and it is the label [`spki`] declares for it.
const PUBLIC_KEY_PEM_LABEL: &str = "PUBLIC KEY";

/// Whether `value` is a PEM-encoded X.509 RSA public key: the envelope, the
/// `SubjectPublicKeyInfo` it holds, its algorithm, and the RSA key inside it.
fn is_x509_rsa_public_key(value: &str) -> bool {
    let Ok(envelope) = ::pem::parse(value) else {
        return false;
    };
    if envelope.tag() != PUBLIC_KEY_PEM_LABEL {
        return false;
    }
    let Ok(public_key) = ::spki::SubjectPublicKeyInfoRef::from_der(envelope.contents()) else {
        return false;
    };
    if public_key.algorithm.oid != ::pkcs1::ALGORITHM_OID {
        return false;
    }
    let Some(rsa_key) = public_key.subject_public_key.as_bytes() else {
        return false;
    };
    ::pkcs1::RsaPublicKey::from_der(rsa_key).is_ok()
}

/// The message this service's `ThrottledRequestException` carries: the model documents the
/// error as "the request was denied due to request throttling", and that is what a create
/// past one of this store's bounds is refused with — a constant, for the same reason as
/// [`INVALID_PARAMETERS_MESSAGE`].
pub const THROTTLED_REQUEST_MESSAGE: &str = "The request was denied due to request throttling.";

/// How many Flows one linked account holds. Emulator-chosen, like
/// [`MAX_PHONE_NUMBERS_PER_ACCOUNT`]: each `CreateWhatsAppFlow` call adds a record, so
/// without a bound a stream of creates could grow the process without limit. The Flow
/// operations model no quota error, so a create past the bound is refused with the
/// `ThrottledRequestException` they do model — the one refusal that says the request is not
/// being accepted now, and which stops saying so once a Flow is deleted.
pub const MAX_FLOWS_PER_ACCOUNT: usize = 32;

/// How many message templates one linked account holds, for the same reason and refused
/// the same way as [`MAX_FLOWS_PER_ACCOUNT`]. With the modelled `1..6000` bound on a
/// template definition it also bounds the bytes the stored definitions take.
pub const MAX_MESSAGE_TEMPLATES_PER_ACCOUNT: usize = 32;

/// The message this service's `ResourceNotFoundException` carries.
///
/// The text is CONSTANT: nothing of the request reaches it, so a client's match on it does
/// not depend on what it asked for and the lookup key never reaches an error the service
/// keeps opaque. The model documents the error only as "The resource was not found."; this
/// text is the one the service's READS were recorded answering, identically for two
/// accounts named by 36- and 115-character ids, which is what shows it is not derived from
/// the request. Every not-found path takes a constant; the `id`-scoped Meta lookups' own is
/// [`ID_NOT_FOUND_FOR_ACCOUNT_MESSAGE`] and the phone-number lookups' is
/// [`ORIGINATION_PHONE_NUMBER_NOT_FOUND_MESSAGE`].
pub const RESOURCE_NOT_FOUND_MESSAGE: &str = "Requested resource not found.";

/// The message the PHONE-NUMBER lookup paths' `ResourceNotFoundException` carries.
///
/// A constant too, and the one that lookup was recorded answering for a phone number this
/// account has not registered: it names the KIND of key that resolved to no resource — an
/// origination phone number id — and nothing of the value itself, so it does not vary
/// with the request. The rule is the one [`RESOURCE_NOT_FOUND_MESSAGE`] states — a
/// not-found message is a constant, not a formatted one — with each lookup stating the
/// constant it was recorded with; every operation that resolves a phone-number id (the
/// phone-number read, the send, the business-public-key read and write, the media delete)
/// states this one.
pub const ORIGINATION_PHONE_NUMBER_NOT_FOUND_MESSAGE: &str =
    "Origination phone number id not found for account.";

/// The message the `id`-scoped Meta lookups' `ResourceNotFoundException` carries.
///
/// A constant too, and the one those paths were recorded answering for an `id` that names
/// no linked account: the dataset create, the conversion event, the Flow read and create,
/// the message-template read and create and the media delete all state it. They state
/// their own text rather than the one the account read states
/// ([`RESOURCE_NOT_FOUND_MESSAGE`]), so the rule — a not-found message is a constant, not
/// a formatted one — holds with each path stating the constant it was recorded with. The
/// resource the miss is reported for is the one the `id` member names, whether the id
/// resolved to no account or the account holds no such Flow, template or media object.
pub const ID_NOT_FOUND_FOR_ACCOUNT_MESSAGE: &str = "Id not found for account.";

/// The message the DISASSOCIATION path's `ResourceNotFoundException` carries.
///
/// A constant too, and the one that path was recorded answering for an `id` that names no
/// linked account: it says the resource asked to be disassociated does not exist and
/// nothing of the id itself, so it does not vary with the request. Its text is NOT the one
/// the account reads state ([`RESOURCE_NOT_FOUND_MESSAGE`]) — the same lookup miss is
/// worded per operation — which is the distinction a caller reads to tell which lookup
/// failed.
pub const DISASSOCIATION_NOT_FOUND_MESSAGE: &str =
    "Resource requested for disassociation does not exist.";

/// The header the console's WhatsApp sign-up flow presents on the sign-up callback it
/// posts.
///
/// The sign-up operation is reached through that flow alone — its model doc says it "is
/// only used through the Amazon Web Services console during sign-up" — and a console
/// request carries the flow's own CSRF token in this header. A request made with anything
/// else does not carry it, which is the difference [`require_console_request`] decides on.
pub const CONSOLE_CSRF_HEADER: &str = "x-csrf-token";

/// The message a console-only operation refuses a request made outside the console with.
///
/// Constant, like every other error text this service states: the refusal is decided from
/// the absence of [`CONSOLE_CSRF_HEADER`] alone, so nothing of the request — no id, no
/// name, no value — reaches it.
pub const CSRF_HEADER_MISSING_MESSAGE: &str = "CSRF header does not exist";

/// Admit only the requests that arrive through the console flow a console-only operation
/// is served through.
///
/// A request carrying no [`CONSOLE_CSRF_HEADER`] — or carrying it empty, which is the same
/// absence as far as the flow is concerned — did not come through that flow, and is
/// refused with the 403 access-denied envelope that refusal is answered in
/// ([`crate::scaffolded::ConsoleAccessDenied`]). The decision is made from the REQUEST
/// alone, so an operation asks here BEFORE it reads or writes anything: a refused call
/// leaves the store exactly as it found it, records no signup and mints no token.
pub fn require_console_request(
    ctx: &smithy_server_runtime::OperationContext<'_>,
) -> Result<(), crate::scaffolded::ConsoleAccessDenied> {
    let through_the_console = ctx
        .parts()
        .headers
        .get(CONSOLE_CSRF_HEADER)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|token| !token.trim().is_empty());
    if through_the_console {
        return Ok(());
    }
    Err(crate::scaffolded::ConsoleAccessDenied::new(
        CSRF_HEADER_MISSING_MESSAGE,
    ))
}

/// The message this service refuses a request that hands it a role of ANOTHER account
/// with.
///
/// Constant, like every other error text this service states: neither the role, the
/// destination it would write to, nor the account the request named is formatted into it,
/// so the refusal says only that passing a role across an account boundary is not
/// permitted — which is the whole of the decision.
pub const CROSS_ACCOUNT_PASS_ROLE_MESSAGE: &str = "Cross-account pass role is not allowed.";

/// Refuse a request that hands this service an IAM role belonging to another account.
///
/// A role an operation accepts is one the SERVICE will assume on the caller's behalf, so
/// handing one over requires `iam:PassRole` on it and a principal can hold that only for a
/// role in its own account: a role named in another account is refused outright, with the
/// 403 `AccessDeniedException` the service models, whatever the rest of the request says.
/// The decision is made from the REQUEST alone — the account the role ARN names against
/// the calling account — so every operation that takes a role asks here BEFORE it resolves
/// any resource the request references and before it writes anything, and a caller that
/// may not pass the role never learns from the answer whether the resource exists.
///
/// `Ok` means only that no entry names another account: a destination with no role, and a
/// role whose account field cannot be read at all, are both left to the rest of the
/// handler, which is what refuses them.
///
/// The calling account is `engine::emulator_account_id()`: this emulator presents ONE
/// account to every caller, so the account a request is made in is that one and nothing
/// has to be read off the request's credentials. A later change that gave the emulator
/// more than one account would supply the caller's account here instead.
pub fn authorize_passed_roles(
    destinations: &[WhatsAppBusinessAccountEventDestination],
) -> Result<(), AccessDeniedException> {
    for destination in destinations {
        let Some(role_arn) = destination.role_arn.as_deref() else {
            continue;
        };
        // The tree's one ARN parse, so this service cannot disagree with another about
        // which account an ARN names.
        let account = ::service_common::arn::parse(role_arn)
            .map(|parsed| parsed.account)
            .filter(|account| !account.is_empty());
        let Some(account) = account else {
            continue;
        };
        if account != ::engine::emulator_account_id() {
            return Err(AccessDeniedException {
                message: Some(CROSS_ACCOUNT_PASS_ROLE_MESSAGE.to_string()),
            });
        }
    }
    Ok(())
}

/// A WhatsApp Business Account linked to this account, as
/// `AssociateWhatsAppBusinessAccount` stored it.
#[derive(Debug, Clone)]
pub struct LinkedAccount {
    /// The service-minted id, `waba-<32 hex>`.
    pub id: String,
    /// The service-minted ARN, `arn:aws:social-messaging:<region>:<account>:waba/<32 hex>`.
    pub arn: String,
    /// The WhatsApp Business Account id at Meta.
    pub waba_id: String,
    /// The account's display name.
    pub waba_name: String,
    /// One of [`REGISTRATION_STATUS_COMPLETE`] / [`REGISTRATION_STATUS_INCOMPLETE`].
    pub registration_status: String,
    /// When the account was linked.
    pub link_date: SystemTime,
    /// As last written by `PutWhatsAppBusinessAccountEventDestinations` (or at
    /// association time).
    pub event_destinations: Vec<WhatsAppBusinessAccountEventDestination>,
    /// The phone numbers registered under this account, in registration order.
    pub phone_number_ids: Vec<String>,
    /// The Meta Conversions API dataset, once one has been created.
    pub dataset_id: Option<String>,
    /// Meta's Marketing Messages onboarding status, when Meta reports one.
    pub marketing_messages_onboarding_status: Option<String>,
    /// Tags supplied at association time.
    pub tags: HashMap<String, String>,
    /// The WhatsApp Flows created under this account, by [`FlowRecord::flow_id`].
    ///
    /// Held ON the account rather than in a store-wide map because a Flow belongs to one
    /// WhatsApp Business Account: every Flow operation names the account beside the Flow,
    /// and unlinking the account takes its Flows with it.
    pub flows: HashMap<String, FlowRecord>,
    /// The WhatsApp message templates created under this account, by
    /// [`MessageTemplateRecord::meta_template_id`] — held on the account for the same
    /// reason as [`Self::flows`].
    pub message_templates: HashMap<String, MessageTemplateRecord>,
}

/// A WhatsApp Flow created under a linked account.
#[derive(Debug, Clone)]
pub struct FlowRecord {
    /// The Meta-side id, a numeric string ([`ComAmazonawsSocialmessagingSocialmessagingState::mint_flow_id`]).
    pub flow_id: String,
    /// The name the create gave the Flow.
    pub flow_name: String,
    /// The categories the create classified the Flow with.
    pub categories: Vec<String>,
    /// One of [`FLOW_STATUS_DRAFT`] / [`FLOW_STATUS_PUBLISHED`].
    pub status: String,
    /// Whether the Flow HAS a JSON definition — the create supplied one, or the Flow was
    /// cloned from one that had. A Flow cannot be published without it.
    pub has_definition: bool,
    /// The Flow JSON schema version the definition declares, when it declares one.
    pub json_version: Option<String>,
    /// The data API version the definition declares, when it declares one.
    pub data_api_version: Option<String>,
    /// The endpoint Meta calls for a data-exchange Flow, when the create named one.
    pub endpoint_uri: Option<String>,
    /// When the Flow was created.
    pub created_at: SystemTime,
}

/// A WhatsApp message template created under a linked account.
#[derive(Debug, Clone)]
pub struct MessageTemplateRecord {
    /// The Meta-side id, a numeric string
    /// ([`ComAmazonawsSocialmessagingSocialmessagingState::mint_meta_template_id`]).
    pub meta_template_id: String,
    /// The template's name, as its definition declares it.
    pub name: String,
    /// The template's language code, when its definition declares one.
    pub language: Option<String>,
    /// The template's category, when its definition declares one.
    pub category: Option<String>,
    /// The definition exactly as the create supplied it, which is what the read reports.
    pub definition: String,
    /// When the template was submitted.
    pub created_at: SystemTime,
    /// When Meta's review of it falls due, [`TEMPLATE_REVIEW_SPAN`] after the submission.
    pub review_due_at: SystemTime,
}

impl MessageTemplateRecord {
    /// This template's status at `now`: [`TEMPLATE_STATUS_PENDING`] until the review it is
    /// waiting on falls due, [`TEMPLATE_STATUS_APPROVED`] from that instant on.
    ///
    /// Read off the clock rather than written by a timer: the move was owed at
    /// [`Self::review_due_at`] whether or not anybody called, so every reader sees the same
    /// status at the same instant and no read changes what a later one sees.
    pub fn status_at(&self, now: SystemTime) -> &'static str {
        if now < self.review_due_at {
            TEMPLATE_STATUS_PENDING
        } else {
            TEMPLATE_STATUS_APPROVED
        }
    }
}

/// The business public key set for a phone number by `PutWhatsAppBusinessPublicKey`.
///
/// The operation takes the key one of two ways — the PEM itself, or the ARN of an
/// asymmetric KMS key — and the model documents them as mutually exclusive, so exactly one
/// of these two members is set.
#[derive(Debug, Clone)]
pub struct BusinessPublicKey {
    /// The PEM the caller set, when it set the key material directly.
    pub pem: Option<String>,
    /// The KMS key the caller named, when the key material lives in KMS. The key material
    /// of a customer managed key is not this service's to hold, so a key set this way has
    /// no PEM here and the read reports none.
    pub kms_key_arn: Option<String>,
    /// When the key was set.
    pub written_at: SystemTime,
}

/// A media object held for a phone number.
///
/// `PostWhatsAppMessageMedia` is what uploads one (still scaffolded); the delete is what
/// removes it, and reports a miss for a media id this number holds none of.
#[derive(Debug, Clone)]
pub struct MediaObject {
    /// The media id, as the upload minted it.
    pub media_id: String,
    /// When the object was stored.
    pub stored_at: SystemTime,
}

/// A phone number registered under a linked account.
#[derive(Debug, Clone)]
pub struct PhoneNumberRecord {
    /// The service-minted id, `phone-number-id-<32 hex>`.
    pub phone_number_id: String,
    /// The service-minted ARN, `arn:aws:social-messaging:<region>:<account>:phone-number-id/<32 hex>`.
    pub arn: String,
    /// The [`LinkedAccount::id`] this number belongs to.
    pub linked_account_id: String,
    /// The E.164 number messages are sent from.
    pub phone_number: String,
    /// The phone number id at Meta, as the setup finalization named it.
    pub meta_phone_number_id: String,
    /// The number as it appears in a recipient's display.
    pub display_phone_number: String,
    /// The display name for this number.
    pub display_phone_number_name: String,
    /// Meta's quality rating for the number.
    pub quality_rating: String,
    /// The two-letter ISO region Meta stores this number's data in.
    pub data_localization_region: Option<String>,
    /// Call settings, once `UpdateLinkedWhatsAppBusinessAccountPhoneNumber` wrote some.
    pub call_settings: Option<WhatsAppCallSettings>,
    /// Tags supplied at registration time.
    pub tags: HashMap<String, String>,
    /// The business public key, once `PutWhatsAppBusinessPublicKey` set one. Held on the
    /// number's own record because the key belongs to that number: one number has one key,
    /// and unregistering the number takes it with it.
    pub business_public_key: Option<BusinessPublicKey>,
    /// The media objects held for this number, by [`MediaObject::media_id`] — on the
    /// record for the same reason as [`Self::business_public_key`].
    pub media: HashMap<String, MediaObject>,
}

/// A signup started by a `signupCallback` and not yet finalized.
#[derive(Debug, Clone)]
pub struct PendingSignup {
    /// The Meta-issued access token the callback carried.
    pub access_token: String,
    /// Where WhatsApp sends callback notifications for this signup.
    pub callback_url: Option<String>,
    /// When the callback arrived.
    pub created_at: SystemTime,
}

/// One accepted `SendWhatsAppMessage` call.
#[derive(Debug, Clone)]
pub struct SentMessage {
    /// The minted, per-call message id returned to the caller.
    pub message_id: String,
    /// The [`PhoneNumberRecord::phone_number_id`] the message was sent from.
    pub origination_phone_number_id: String,
    /// The message body exactly as the caller supplied it.
    pub message: Vec<u8>,
    /// The Meta Graph API version the caller asked for.
    pub meta_api_version: String,
    /// When the call was accepted.
    pub sent_at: SystemTime,
}

/// One accepted `SendWhatsAppConversionEvent` call.
#[derive(Debug, Clone)]
pub struct ConversionEvent {
    /// The minted, per-call `requestId` returned to the caller.
    pub request_id: String,
    /// The [`LinkedAccount::id`] the event was sent for.
    pub linked_account_id: String,
    /// The [`LinkedAccount::dataset_id`] the event was sent to.
    pub dataset_id: String,
    /// The Conversions API payload exactly as the caller supplied it.
    pub event_data: Vec<u8>,
    /// When the call was accepted.
    pub sent_at: SystemTime,
}

/// Whether a fresh store reports WhatsApp setup as enabled for the account.
///
/// `setupEnabled` is an account-level fact `ListLinkedWhatsAppBusinessAccounts` reports
/// beside the listing, and the recorded listing reports it `true` for an account with no
/// linked WhatsApp Business Account at all, so it is not derived from what the account
/// holds. No operation this service models enables or disables WhatsApp setup — it is
/// settled for the account before any of them is called — so the store starts from this
/// value and [`ComAmazonawsSocialmessagingSocialmessagingState::setup_enabled`] is what
/// the listing reads.
pub const SETUP_ENABLED_DEFAULT: bool = true;

/// Per-service state for com-amazonaws-socialmessaging-socialmessaging. A handler reaches it through [`service_state`].
#[derive(Debug)]
pub struct ComAmazonawsSocialmessagingSocialmessagingState {
    /// Linked accounts by [`LinkedAccount::id`].
    pub linked_accounts: HashMap<String, LinkedAccount>,
    /// Phone numbers by [`PhoneNumberRecord::phone_number_id`].
    pub phone_numbers: HashMap<String, PhoneNumberRecord>,
    /// Pending signups by their `associateInProgressToken`.
    pub pending_signups: HashMap<String, PendingSignup>,
    /// Every accepted send, oldest first. Bounded by [`MAX_RETAINED_SENT_MESSAGES`]
    /// and [`MAX_RETAINED_SENT_MESSAGE_BYTES`]; see [`Self::record_sent_message`].
    pub sent_messages: VecDeque<SentMessage>,
    /// The bytes of the message bodies `sent_messages` currently holds.
    pub retained_message_bytes: usize,
    /// Every accepted conversion event, oldest first. Bounded by
    /// [`MAX_RETAINED_CONVERSION_EVENTS`] and [`MAX_RETAINED_CONVERSION_EVENT_BYTES`];
    /// see [`Self::record_conversion_event`].
    pub conversion_events: VecDeque<ConversionEvent>,
    /// The bytes of the payloads `conversion_events` currently holds.
    pub retained_conversion_event_bytes: usize,
    /// ARN -> resource id, so an ARN-form `id` input resolves to the same record
    /// as the short form.
    pub arn_index: HashMap<String, String>,
    /// Whether WhatsApp setup is enabled for this account, as the listing reports it
    /// beside `linkedAccounts`. Starts at [`SETUP_ENABLED_DEFAULT`].
    pub setup_enabled: bool,
    /// Monotonic counter behind every minted id, so each one is unique.
    pub id_seq: u64,
}

/// A fresh store holds no resources and reports WhatsApp setup as
/// [`SETUP_ENABLED_DEFAULT`], which is the one field whose empty value is not its
/// type's default — hence the hand-written impl rather than a derive.
impl Default for ComAmazonawsSocialmessagingSocialmessagingState {
    fn default() -> Self {
        Self {
            linked_accounts: HashMap::new(),
            phone_numbers: HashMap::new(),
            pending_signups: HashMap::new(),
            sent_messages: VecDeque::new(),
            retained_message_bytes: 0,
            conversion_events: VecDeque::new(),
            retained_conversion_event_bytes: 0,
            arn_index: HashMap::new(),
            setup_enabled: SETUP_ENABLED_DEFAULT,
            id_seq: 0,
        }
    }
}

impl ComAmazonawsSocialmessagingSocialmessagingState {
    /// Whether the store already holds [`MAX_LINKED_ACCOUNTS`] accounts, so a
    /// further association must be refused instead of growing it.
    pub fn linked_accounts_full(&self) -> bool {
        self.linked_accounts.len() >= MAX_LINKED_ACCOUNTS
    }

    /// Drop every signup nobody finalized within [`PENDING_SIGNUP_TTL`]. This
    /// materialises what the clock already made due: each entry was owed its
    /// removal at the same instant whether or not anyone called.
    pub fn expire_pending_signups(&mut self, now: SystemTime) {
        self.pending_signups
            .retain(|_, signup| match now.duration_since(signup.created_at) {
                Ok(age) => age < PENDING_SIGNUP_TTL,
                // A `created_at` in the future (clock skew) is not yet expired.
                Err(_) => true,
            });
    }

    /// Whether [`MAX_PENDING_SIGNUPS`] signups are already in progress. Call
    /// [`Self::expire_pending_signups`] first, so only live ones count.
    pub fn pending_signups_full(&self) -> bool {
        self.pending_signups.len() >= MAX_PENDING_SIGNUPS
    }

    /// Record an accepted send, evicting the oldest records until the retained set
    /// is back inside [`MAX_RETAINED_SENT_MESSAGES`] and
    /// [`MAX_RETAINED_SENT_MESSAGE_BYTES`]. The send just accepted is always kept.
    pub fn record_sent_message(&mut self, message: SentMessage) {
        self.retained_message_bytes += message.message.len();
        self.sent_messages.push_back(message);
        while self.sent_messages.len() > MAX_RETAINED_SENT_MESSAGES
            || (self.retained_message_bytes > MAX_RETAINED_SENT_MESSAGE_BYTES
                && self.sent_messages.len() > 1)
        {
            match self.sent_messages.pop_front() {
                Some(evicted) => {
                    self.retained_message_bytes = self
                        .retained_message_bytes
                        .saturating_sub(evicted.message.len());
                }
                None => break,
            }
        }
    }

    /// Record an accepted conversion event, evicting the oldest records until the
    /// retained set is back inside [`MAX_RETAINED_CONVERSION_EVENTS`] and
    /// [`MAX_RETAINED_CONVERSION_EVENT_BYTES`]. The event just accepted is always kept.
    pub fn record_conversion_event(&mut self, event: ConversionEvent) {
        self.retained_conversion_event_bytes += event.event_data.len();
        self.conversion_events.push_back(event);
        while self.conversion_events.len() > MAX_RETAINED_CONVERSION_EVENTS
            || (self.retained_conversion_event_bytes > MAX_RETAINED_CONVERSION_EVENT_BYTES
                && self.conversion_events.len() > 1)
        {
            match self.conversion_events.pop_front() {
                Some(evicted) => {
                    self.retained_conversion_event_bytes = self
                        .retained_conversion_event_bytes
                        .saturating_sub(evicted.event_data.len());
                }
                None => break,
            }
        }
    }

    /// 32 lowercase hex characters, unique per call: the digest of `kind` and this
    /// store's monotonic sequence number. Distinct `kind`s never collide, so a
    /// linked-account suffix is never also a phone-number suffix.
    pub fn next_hex32(&mut self, kind: &str) -> String {
        self.id_seq += 1;
        let mut hasher = Sha256::new();
        hasher.update(kind.as_bytes());
        hasher.update(b":");
        hasher.update(self.id_seq.to_be_bytes());
        hex::encode(&hasher.finalize()[..16])
    }

    /// Mint the `(id, arn)` pair for a new linked account.
    pub fn mint_linked_account_id(&mut self, region: &str) -> (String, String) {
        let suffix = self.next_hex32("waba");
        let arn = ::engine::emulator_arn(ARN_SERVICE, region, &format!("waba/{suffix}"));
        (format!("{LINKED_ACCOUNT_ID_PREFIX}{suffix}"), arn)
    }

    /// Mint the `(id, arn)` pair for a newly registered phone number.
    pub fn mint_phone_number_id(&mut self, region: &str) -> (String, String) {
        let suffix = self.next_hex32("phone-number-id");
        let arn =
            ::engine::emulator_arn(ARN_SERVICE, region, &format!("phone-number-id/{suffix}"));
        (format!("{PHONE_NUMBER_ID_PREFIX}{suffix}"), arn)
    }

    /// Mint an `associateInProgressToken` (the model caps it at 50 characters).
    pub fn mint_signup_token(&mut self) -> String {
        format!("signup-{}", self.next_hex32("signup"))
    }

    /// Mint the message id `SendWhatsAppMessage` returns. Meta message ids carry
    /// the `wamid.` prefix.
    pub fn mint_message_id(&mut self) -> String {
        format!("wamid.{}", self.next_hex32("message"))
    }

    /// Mint the Meta Conversions API dataset id `CreateWhatsAppDataset` returns:
    /// [`DATASET_ID_DIGITS`] decimal digits with a non-zero lead, read off this store's
    /// own unique digest, so it satisfies `WhatsAppDatasetId` and two datasets never get
    /// the same id.
    pub fn mint_dataset_id(&mut self) -> String {
        self.mint_numeric_id("dataset", DATASET_ID_DIGITS)
    }

    /// Mint the Flow id `CreateWhatsAppFlow` returns: [`META_ID_DIGITS`] decimal digits,
    /// so it satisfies `MetaFlowId` (`^[0-9]+$`) and two Flows never get the same id.
    pub fn mint_flow_id(&mut self) -> String {
        self.mint_numeric_id("flow", META_ID_DIGITS)
    }

    /// Mint the template id `CreateWhatsAppMessageTemplate` returns, the same way and for
    /// the same reason as [`Self::mint_flow_id`]: `MetaTemplateId` carries the same
    /// numeric constraint.
    pub fn mint_meta_template_id(&mut self) -> String {
        self.mint_numeric_id("message-template", META_ID_DIGITS)
    }

    /// `digits` decimal digits with a non-zero lead, read off this store's own unique
    /// digest — the one place a numeric Meta-side id is minted, so the dataset, Flow and
    /// template ids cannot drift apart in form.
    fn mint_numeric_id(&mut self, kind: &str, digits: u32) -> String {
        let digest = self.next_hex32(kind);
        // 15 hex characters are 60 bits, which a u64 holds.
        let value = u64::from_str_radix(&digest[..15], 16).unwrap_or(self.id_seq);
        let lead = 10_u64.pow(digits - 1);
        (lead + value % (9 * lead)).to_string()
    }

    /// Mint the `requestId` `SendWhatsAppConversionEvent` returns: the 8-4-4-4-12 form a
    /// request id takes, over this store's own unique digest, so each accepted call
    /// reports its own id.
    pub fn mint_request_id(&mut self) -> String {
        let hex = self.next_hex32("conversion-event");
        format!(
            "{}-{}-{}-{}-{}",
            &hex[0..8],
            &hex[8..12],
            &hex[12..16],
            &hex[16..20],
            &hex[20..32]
        )
    }

    /// The stored linked account an `id` input names, accepting both the
    /// `waba-...` short form and the account ARN.
    pub fn resolve_linked_account(&self, id: &str) -> Option<&LinkedAccount> {
        self.linked_account_id(id)
            .and_then(|resolved| self.linked_accounts.get(&resolved))
    }

    /// The key under which the linked account an `id` input names is stored.
    pub fn linked_account_id(&self, id: &str) -> Option<String> {
        if self.linked_accounts.contains_key(id) {
            return Some(id.to_string());
        }
        self.arn_index
            .get(id)
            .filter(|resolved| self.linked_accounts.contains_key(resolved.as_str()))
            .cloned()
    }

    /// The stored phone number an `id` input names, accepting both the
    /// `phone-number-id-...` short form and the number's ARN.
    pub fn resolve_phone_number(&self, id: &str) -> Option<&PhoneNumberRecord> {
        self.phone_number_id(id)
            .and_then(|resolved| self.phone_numbers.get(&resolved))
    }

    /// The key under which the phone number an `id` input names is stored.
    pub fn phone_number_id(&self, id: &str) -> Option<String> {
        if self.phone_numbers.contains_key(id) {
            return Some(id.to_string());
        }
        self.arn_index
            .get(id)
            .filter(|resolved| self.phone_numbers.contains_key(resolved.as_str()))
            .cloned()
    }

    /// The Flow an account holds under `flow_id`, or `None` when that account holds none.
    ///
    /// Scoped to the account the request named, so a Flow of ANOTHER account is a miss
    /// here exactly as an unknown id is: a Flow is only ever read through the account that
    /// owns it.
    pub fn resolve_flow(&self, account_id: &str, flow_id: &str) -> Option<&FlowRecord> {
        self.linked_accounts
            .get(account_id)
            .and_then(|account| account.flows.get(flow_id))
    }

    /// The template an account holds under `meta_template_id`, scoped to that account for
    /// the same reason as [`Self::resolve_flow`].
    pub fn resolve_template(
        &self,
        account_id: &str,
        meta_template_id: &str,
    ) -> Option<&MessageTemplateRecord> {
        self.linked_accounts
            .get(account_id)
            .and_then(|account| account.message_templates.get(meta_template_id))
    }

    /// The template an account holds under `name` — and under `language` too when the
    /// caller named one, since a template is identified at Meta by the name and language
    /// together and one name can carry a translation per language.
    ///
    /// The most recently submitted match, so a name carrying more than one template
    /// resolves to the same one on every read.
    pub fn template_by_name(
        &self,
        account_id: &str,
        name: &str,
        language: Option<&str>,
    ) -> Option<&MessageTemplateRecord> {
        self.linked_accounts
            .get(account_id)?
            .message_templates
            .values()
            .filter(|template| template.name == name)
            .filter(|template| match language {
                Some(language) => template.language.as_deref() == Some(language),
                None => true,
            })
            .max_by(|left, right| {
                (left.created_at, &left.meta_template_id)
                    .cmp(&(right.created_at, &right.meta_template_id))
            })
    }

    /// The tags of the resource `arn` names, or `None` when this store holds no
    /// resource under that ARN.
    ///
    /// Both resource kinds this service tags — a linked account and a registered
    /// phone number — keep their tags on their own record and are reachable through
    /// [`Self::arn_index`], so a tag read is over the SAME map the resource was
    /// created with and reports nothing a write did not put there.
    pub fn tags_for_arn(&self, arn: &str) -> Option<Vec<Tag>> {
        let id = self.arn_index.get(arn)?;
        let tags = self
            .linked_accounts
            .get(id)
            .map(|account| &account.tags)
            .or_else(|| self.phone_numbers.get(id).map(|number| &number.tags))?;
        Some(tag_list(tags))
    }

    /// Every linked account whose setup is not finished, keyed by its Meta
    /// account id — what `signupCallback` reports back to the console.
    pub fn accounts_with_incomplete_setup(
        &self,
    ) -> HashMap<String, LinkedWhatsAppBusinessAccountIdMetaData> {
        self.linked_accounts
            .values()
            .filter(|account| account.registration_status == REGISTRATION_STATUS_INCOMPLETE)
            .map(|account| {
                (
                    account.waba_id.clone(),
                    LinkedWhatsAppBusinessAccountIdMetaData {
                        account_name: Some(account.waba_name.clone()),
                        registration_status: Some(account.registration_status.clone()),
                        // An account with an unfinished setup has no registered
                        // number, so there is none to report here.
                        unregistered_whats_app_phone_numbers: None,
                        waba_id: Some(account.id.clone()),
                    },
                )
            })
            .collect()
    }

    /// The summaries of the numbers registered under `account`, in registration order.
    pub fn phone_number_summaries(
        &self,
        account: &LinkedAccount,
    ) -> Vec<WhatsAppPhoneNumberSummary> {
        account
            .phone_number_ids
            .iter()
            .filter_map(|id| self.phone_numbers.get(id))
            .map(PhoneNumberRecord::to_summary)
            .collect()
    }
}

/// An account's registration status: a linked account with no registered phone
/// number has not finished setting up.
pub fn registration_status_for(phone_number_count: usize) -> &'static str {
    if phone_number_count == 0 {
        REGISTRATION_STATUS_INCOMPLETE
    } else {
        REGISTRATION_STATUS_COMPLETE
    }
}

/// The `(E.164, display)` number pair for a Meta phone-number id.
///
/// The real service reports the number Meta holds against the id. This emulator
/// has no Meta, so it derives one in the North American range reserved for
/// fictional use — `+1 <area> 555-0100` through `555-0199` — from the id itself,
/// so every later read of the same registration reports the same number and two
/// different registrations get different numbers.
pub fn derive_phone_number(meta_phone_number_id: &str) -> (String, String) {
    let digest = Sha256::digest(meta_phone_number_id.as_bytes());
    // A NANP area code is 3 digits whose first is 2-9.
    let area = 200 + ((u32::from(digest[0]) << 3) | (u32::from(digest[1]) >> 5)) % 800;
    let line = u32::from(digest[2]) % 100;
    (
        format!("+1{area}55501{line:02}"),
        format!("+1 {area} 555-01{line:02}"),
    )
}

/// What a Flow's JSON definition declares, as the read reports it back.
///
/// Meta's Flow JSON carries its own schema version, and a data-exchange Flow carries the
/// data API version it speaks; the modelled `jsonVersion` and `dataApiVersion` members of
/// the Flow read are those two values. Nothing else of the definition is retained: the
/// modelled surface carries no Flow JSON back, so the bytes themselves are read once, here,
/// and what the read reports is kept.
#[derive(Debug, Clone, Default)]
pub struct FlowDefinition {
    /// The `version` the definition declares, when it declares one inside the modelled
    /// `MetaFlowJsonVersion` bound (`len 1..10`).
    pub json_version: Option<String>,
    /// The `data_api_version` the definition declares, under the same bound
    /// (`MetaFlowDataApiVersion`, `len 1..10`).
    pub data_api_version: Option<String>,
}

/// Read a Flow's JSON definition, or `None` when the bytes are not a JSON object.
///
/// `None` is what "no valid `flowJson`" means for the create: a definition that is not a
/// JSON object at all is not one the Flow can be published from.
pub fn parse_flow_json(flow_json: &[u8]) -> Option<FlowDefinition> {
    let definition: ::serde_json::Value = ::serde_json::from_slice(flow_json).ok()?;
    let members = definition.as_object()?;
    Some(FlowDefinition {
        json_version: bounded_version(members.get("version")),
        data_api_version: bounded_version(members.get("data_api_version")),
    })
}

/// A version a Flow definition states, as the wire member carries it: Flow JSON writes a
/// version as a string (`"7.3"`) and a definition that writes it as a number states the
/// same value, so both read back the same way. A value past the modelled `1..10` bound is
/// not reported at all rather than reported truncated.
fn bounded_version(value: Option<&::serde_json::Value>) -> Option<String> {
    let text = match value {
        Some(::serde_json::Value::String(text)) => text.clone(),
        Some(::serde_json::Value::Number(number)) => number.to_string(),
        _ => return None,
    };
    (1..=MAX_VERSION_LENGTH.into()).contains(&text.chars().count()).then_some(text)
}

/// The modelled length both version members share: `MetaFlowJsonVersion` and
/// `MetaFlowDataApiVersion` are each `len 1..10`.
const MAX_VERSION_LENGTH: u8 = 10;

/// What a message template's definition declares.
///
/// A template definition is the Meta template object: its `name` is what identifies the
/// template, and its `language` and `category` are what the create reports back and a read
/// by name selects on.
#[derive(Debug, Clone)]
pub struct TemplateDefinition {
    /// The template's name — a template without one names nothing and is not a template.
    pub name: String,
    /// The template's language code, when the definition declares one.
    pub language: Option<String>,
    /// The template's category, when the definition declares one.
    pub category: Option<String>,
}

/// Read a message template's definition, or `None` when the bytes are not a JSON object
/// carrying a non-empty `name` — which is a definition no template can be created from.
pub fn parse_template_definition(definition: &str) -> Option<TemplateDefinition> {
    let parsed: ::serde_json::Value = ::serde_json::from_str(definition).ok()?;
    let members = parsed.as_object()?;
    let name = members.get("name")?.as_str()?;
    if name.is_empty() {
        return None;
    }
    Some(TemplateDefinition {
        name: name.to_string(),
        language: bounded_text(members.get("language"), MAX_TEMPLATE_LANGUAGE_LENGTH),
        category: bounded_text(members.get("category"), MAX_TEMPLATE_CATEGORY_LENGTH),
    })
}

/// The modelled length of a template's language code (`MetaTemplateLanguage`, `len 1..6`).
const MAX_TEMPLATE_LANGUAGE_LENGTH: usize = 6;

/// The modelled length of a template's category (`MetaTemplateCategory`, `len 1..100`).
const MAX_TEMPLATE_CATEGORY_LENGTH: usize = 100;

/// A string member of a definition, reported only when it fits the modelled bound the
/// output member carries — a longer value is not reported at all rather than truncated.
fn bounded_text(value: Option<&::serde_json::Value>, max: usize) -> Option<String> {
    let text = value?.as_str()?;
    (1..=max)
        .contains(&text.chars().count())
        .then(|| text.to_string())
}

/// A modelled `TagList` as the key/value store a resource keeps.
///
/// A tag with no value is stored as an empty string, which is how a tag whose
/// value member is absent reads back.
pub fn tag_map(tags: Vec<Tag>) -> HashMap<String, String> {
    tags.into_iter()
        .map(|tag| (tag.key, tag.value.unwrap_or_default()))
        .collect()
}

/// A resource's stored tags as the modelled `TagList` a read reports.
///
/// Ordered by key, because a `HashMap` has no order of its own and two reads of one
/// unchanged resource report the same list.
pub fn tag_list(tags: &HashMap<String, String>) -> Vec<Tag> {
    let mut list: Vec<Tag> = tags
        .iter()
        .map(|(key, value)| Tag {
            key: key.clone(),
            value: Some(value.clone()),
        })
        .collect();
    list.sort_by(|left, right| left.key.cmp(&right.key));
    list
}

/// A stored instant as the wire timestamp the model's `timestamp` members carry.
pub fn to_date_time(at: SystemTime) -> ::aws_smithy_types::DateTime {
    let since_epoch = at
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    ::aws_smithy_types::DateTime::from_secs_f64(since_epoch.as_secs_f64())
}

/// The region this request was signed for, so a resource's ARN carries the region
/// the caller addressed. Falls back to [`DEFAULT_REGION`] for an unsigned request.
pub fn request_region(ctx: &smithy_server_runtime::OperationContext<'_>) -> String {
    ::emulator_server::sigv4_credential_scope(&ctx.parts().headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

impl PhoneNumberRecord {
    /// This record as the `WhatsAppPhoneNumberSummary` a linked account reports.
    pub fn to_summary(&self) -> WhatsAppPhoneNumberSummary {
        WhatsAppPhoneNumberSummary {
            arn: self.arn.clone(),
            phone_number: self.phone_number.clone(),
            phone_number_id: self.phone_number_id.clone(),
            meta_phone_number_id: self.meta_phone_number_id.clone(),
            display_phone_number_name: self.display_phone_number_name.clone(),
            display_phone_number: self.display_phone_number.clone(),
            quality_rating: self.quality_rating.clone(),
            data_localization_region: self.data_localization_region.clone(),
        }
    }

    /// This record as the `WhatsAppPhoneNumberDetail` the phone-number read returns.
    pub fn to_detail(&self) -> WhatsAppPhoneNumberDetail {
        WhatsAppPhoneNumberDetail {
            arn: self.arn.clone(),
            phone_number: self.phone_number.clone(),
            phone_number_id: self.phone_number_id.clone(),
            meta_phone_number_id: self.meta_phone_number_id.clone(),
            display_phone_number_name: self.display_phone_number_name.clone(),
            display_phone_number: self.display_phone_number.clone(),
            quality_rating: self.quality_rating.clone(),
            data_localization_region: self.data_localization_region.clone(),
        }
    }
}

impl LinkedAccount {
    /// This account as the summary the list operation returns.
    pub fn to_summary(&self) -> LinkedWhatsAppBusinessAccountSummary {
        LinkedWhatsAppBusinessAccountSummary {
            arn: self.arn.clone(),
            id: self.id.clone(),
            waba_id: self.waba_id.clone(),
            registration_status: self.registration_status.clone(),
            link_date: to_date_time(self.link_date),
            waba_name: self.waba_name.clone(),
            event_destinations: self.event_destinations.clone(),
            marketing_messages_onboarding_status: self
                .marketing_messages_onboarding_status
                .clone(),
            // No Direct Send enforcement is modelled, and the model omits these
            // members when none is active.
            direct_send_enforcement_status: None,
            direct_send_enforcement_status_expires_at: None,
            dataset_id: self.dataset_id.clone(),
        }
    }

    /// This account as the full `LinkedWhatsAppBusinessAccount` the read returns.
    pub fn to_account(
        &self,
        phone_numbers: Vec<WhatsAppPhoneNumberSummary>,
    ) -> LinkedWhatsAppBusinessAccount {
        LinkedWhatsAppBusinessAccount {
            arn: self.arn.clone(),
            id: self.id.clone(),
            waba_id: self.waba_id.clone(),
            registration_status: self.registration_status.clone(),
            link_date: to_date_time(self.link_date),
            waba_name: self.waba_name.clone(),
            event_destinations: self.event_destinations.clone(),
            marketing_messages_onboarding_status: self
                .marketing_messages_onboarding_status
                .clone(),
            direct_send_enforcement_status: None,
            direct_send_enforcement_status_expires_at: None,
            dataset_id: self.dataset_id.clone(),
            phone_numbers,
        }
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsSocialmessagingSocialmessagingState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsSocialmessagingSocialmessagingState>)
}
