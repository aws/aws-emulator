// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-ses-simpleemailservice service.
//!
//! One store per emulator: the identity list SES keys every send against — each
//! identity carrying the sending-authorization policies it holds, where its
//! notifications go ([`NotificationSettings`]), its Easy DKIM setup
//! ([`EasyDkimSettings`]) and its custom MAIL FROM setup
//! ([`MailFromSettings`]) — the send log the sending quota
//! is derived from, the account's sending limits, and the configuration sets a
//! send may name.
//!
//! Nothing here is a counter that a reader bumps: `SentLast24Hours` is COMPUTED
//! from the send log (see [`ComAmazonawsSesSimpleemailserviceState::sent_last_24_hours`]),
//! so it falls again as messages age out of the window instead of drifting.
//!
//! Every caller-driven store is BOUNDED, because every request reaching this
//! emulator can grow one of them: the identity list at [`MAX_IDENTITIES`]
//! entries, each identity's sending-authorization policies at
//! [`MAX_IDENTITY_POLICIES`] documents of [`MAX_POLICY_BYTES`], the configuration
//! sets at [`MAX_CONFIGURATION_SETS`], the receipt rule sets at
//! [`MAX_RECEIPT_RULE_SETS`] with each set's rules at
//! [`MAX_RECEIPT_RULES_PER_RULE_SET`], the receipt IP address filters at
//! [`MAX_RECEIPT_FILTERS`], and the send log
//! at the activity its longest report covers, with that retention's expiry
//! applied on append ([`ComAmazonawsSesSimpleemailserviceState::log_sent_message`])
//! and [`MAX_SENT_LOG_ENTRIES`] as the backstop. The text one log entry keeps is
//! capped at [`MAX_LOGGED_TEXT`]. The template store is bounded the same way, at
//! [`MAX_TEMPLATES`], and the custom verification email templates at
//! [`MAX_CUSTOM_VERIFICATION_EMAIL_TEMPLATES`].
//!
//! A listing this store serves a page at a time hands the caller an OPAQUE,
//! self-verifying cursor ([`encode_page_token`]), so a token this service never
//! issued is refused instead of being read as some arbitrary position.
//!
//! The identity list here is this surface's VIEW of one account-wide set of email
//! identities, which lives in the shared engine as
//! [`::service_common::ses_identities::SesIdentityDirectory`] and which the SES v2
//! surface publishes to and reads as well: an identity belongs to the account rather
//! than to the API that recorded it. What a record here adds to that set is what THIS
//! API adds to an identity — its policies, its Easy DKIM setup, its notification
//! topics, its custom MAIL FROM setup — and [`reconcile_identities`] is where the two
//! are brought back into agreement, on every operation this service serves.

/// Whether Amazon SES may really reach the destinations a receipt rule's actions name — the
/// ONE pre-flight every operation that stores a rule runs, read against those services' own
/// stores rather than decided here.
pub mod receipt_destinations;

/// Whether the resource an event destination publishes to EXISTS — the ONE pre-flight every
/// operation that stores an event destination runs, asked of the service that owns that
/// resource rather than decided here.
pub mod event_destination_targets;

/// Whether Amazon SES may really write a received message into the Amazon S3 destination an
/// `S3Action` names — the bucket's own policy and the KMS key's — which is read against those
/// services' own stores rather than decided here.
pub mod s3_action_authorization;

/// This surface's view of the ACCOUNT's email templates — one set for both SES APIs — and
/// the token the template listing pages it with.
pub mod templates;
pub use templates::*;

/// What the account's ACTIVE receipt rule set does with a message this account has just
/// accepted for one of its own recipients — the receiving half of this service.
pub mod inbound_delivery;

use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// The trailing window the sending quota is measured over.
///
/// The model documents the window: `GetSendQuotaResponse$Max24HourSend` is "The
/// maximum number of emails the user is allowed to send in a 24-hour interval"
/// and `SentLast24Hours` is "The number of emails sent during the previous 24
/// hours".
pub const SEND_QUOTA_WINDOW: Duration = Duration::from_secs(24 * 60 * 60);

/// The trailing window the sending statistics are reported over.
///
/// `GetSendStatistics` documents it: "The result is a list of data points,
/// representing the last two weeks of sending activity."
pub const SEND_STATISTICS_WINDOW: Duration = Duration::from_secs(14 * 24 * 60 * 60);

/// How long an accepted message stays in the send log: the LONGEST window any
/// reader of the log reports over, which is the sending statistics' two weeks
/// ([`SEND_STATISTICS_WINDOW`]) rather than the sending quota's 24 hours. An
/// entry the quota has stopped counting is still part of the statistics, so the
/// log cannot drop it any earlier without making a documented report impossible
/// to serve.
pub const SEND_LOG_RETENTION: Duration = SEND_STATISTICS_WINDOW;

/// The period one sending-statistics data point aggregates.
///
/// `GetSendStatistics` documents it: "Each data point in the list contains
/// statistics for a 15-minute period of time", and `SendDataPoint` repeats it.
pub const SEND_STATISTICS_PERIOD: Duration = Duration::from_secs(15 * 60);

/// The account's daily sending quota in the Amazon SES sandbox, which is where a
/// fresh account (and therefore this emulator's account) starts.
///
/// Held in [`AccountSettings`] rather than returned as a literal, so later work
/// that raises the quota changes the stored account record and every reader
/// follows.
pub const SANDBOX_MAX_24_HOUR_SEND: f64 = 200.0;

/// The account's maximum send rate (messages per second) in the Amazon SES
/// sandbox. Stored, for the same reason as [`SANDBOX_MAX_24_HOUR_SEND`].
pub const SANDBOX_MAX_SEND_RATE: f64 = 1.0;

/// The identities one account may hold, and so the ceiling on the identity
/// store: SES documents a limit of 10,000 verified identities per account per
/// Region, and a verification past it is refused rather than letting caller
/// input grow this store without end.
pub const MAX_IDENTITIES: usize = 10_000;

/// The configuration sets one account may hold, and so the ceiling on the
/// configuration-set store: SES documents a limit of 10,000 configuration sets
/// per account per Region, and a create past it is refused rather than letting
/// caller input grow this store without end.
pub const MAX_CONFIGURATION_SETS: usize = 10_000;

/// The characters a configuration set name may be built from, and the longest
/// such name.
///
/// `ConfigurationSet$Name` states the rule: "Contain only letters (a-z, A-Z),
/// numbers (0-9), underscores (_), or dashes (-)" and "Contain 64 characters or
/// fewer".
pub const MAX_CONFIGURATION_SET_NAME: usize = 64;

/// The recipients ONE message may address, across every recipient line it
/// carries.
///
/// Every send operation of this service documents the same ceiling: `SendEmail`
/// as "The message may not include more than 50 recipients, across the To:, CC:
/// and BCC: fields", and `SendRawEmail` as "Each message can include up to 50
/// recipient addresses across the To:, CC:, or BCC: lines". One constant, so the
/// operations that share the rule cannot disagree about the number.
pub const MAX_RECIPIENTS: usize = 50;

/// The destinations ONE bulk send may address, each of which is a separate
/// message.
///
/// `SendBulkTemplatedEmail` documents it: "You can specify up to 50 Destination
/// objects within a Destinations array." It is a ceiling on the destinations of
/// one call, which is not the [`MAX_RECIPIENTS`] ceiling on the recipients of
/// one message: a bulk send may address 50 destinations of 50 recipients each.
pub const MAX_BULK_DESTINATIONS: usize = 50;

/// The bytes one raw message may carry, attachments included.
///
/// `SendRawEmail` documents it: "The maximum message size, including
/// attachments, is 10 MB." It is the size [`MAX_LOGGED_TEXT`] keeps a prefix of,
/// and the bound on the bytes one request can hand this emulator to read: a
/// message past it is refused rather than accepted.
pub const MAX_MESSAGE_BYTES: usize = 10 * 1024 * 1024;

/// The `multipart` containers one raw message may declare, and so the deepest
/// MIME nesting this service reads.
///
/// Each level of a MIME tree is one part declaring a `multipart/*` media type,
/// and such a declaration is one header line, so [`MAX_MESSAGE_BYTES`] alone
/// would leave the DEPTH of the tree a request asks for effectively unbounded
/// while bounding its size. The declarations are counted on the raw bytes before
/// anything parses them, so the tree is never built deeper than this. Real
/// messages nest two or three deep — a `multipart/mixed` holding a
/// `multipart/alternative` — so this leaves a wide margin over anything a mail
/// client composes.
pub const MAX_MULTIPART_CONTAINERS: usize = 64;

/// The characters an event destination name may be built from, and the longest
/// such name.
///
/// `EventDestination$Name` states the rule: the name must "contain only ASCII
/// letters (a-z, A-Z), numbers (0-9), underscores (_), or dashes (-)" and
/// "contain 64 characters or fewer".
pub const MAX_EVENT_DESTINATION_NAME: usize = 64;

/// The event destinations one configuration set may hold, and so the ceiling on
/// the list each set carries: Amazon SES allows a configuration set ten of them,
/// and a create past that is refused with the modelled `LimitExceeded` — "a
/// resource could not be created because of service limits" — rather than
/// letting caller input grow a set without end.
pub const MAX_EVENT_DESTINATIONS: usize = 10;

/// The entries the send log may hold at once.
///
/// The account's own quota is the real ceiling — a day's accepted sends are at
/// most `max_24_hour_send` recipients, and [`SEND_LOG_RETENTION`] bounds how
/// many days of them the log keeps — and that retention's expiry is applied on
/// every append, so this is the backstop that still bounds the store if the
/// stored quota is raised later.
pub const MAX_SENT_LOG_ENTRIES: usize = 1_000;

/// The receipt rule sets one account may hold, and so the ceiling on the rule-set
/// store: SES documents a limit of 20 receipt rule sets per AWS account, and a
/// create or clone past it is refused with the modelled `LimitExceeded` rather
/// than letting caller input grow this store without end.
pub const MAX_RECEIPT_RULE_SETS: usize = 20;

/// The characters a receipt rule set name may carry.
///
/// From the model's own `ReceiptRuleSetName` documentation (e.g.
/// `CreateReceiptRuleSetRequest$RuleSetName`): the name must "Contain 64
/// characters or fewer".
pub const MAX_RECEIPT_RULE_SET_NAME: usize = 64;

/// The receipt rules ONE rule set may hold, and so the ceiling on a set's rule
/// list: SES documents a limit of 200 receipt rules per receipt rule set, and an
/// addition past it is refused with the modelled `LimitExceeded` rather than
/// letting caller input grow one set without end.
pub const MAX_RECEIPT_RULES_PER_RULE_SET: usize = 200;

/// The longest name a receipt rule may carry.
///
/// `ReceiptRule$Name` documents the same ceiling the rule set names carry
/// ("Contain 64 characters or fewer"), so this is that constant rather than a
/// second copy of the number.
pub const MAX_RECEIPT_RULE_NAME: usize = MAX_RECEIPT_RULE_SET_NAME;

/// The receipt IP address filters one account may hold, and so the ceiling on
/// the filter store: SES documents a limit of 100 receipt filters per AWS
/// account, and a create past it is refused with the modelled `LimitExceeded`
/// rather than letting caller input grow this store without end.
pub const MAX_RECEIPT_FILTERS: usize = 100;

/// The longest name a receipt IP address filter may carry.
///
/// `ReceiptFilter$Name` documents the same ceiling the rule and rule set names
/// carry ("Contain 64 characters or fewer"), so this is that constant rather
/// than a second copy of the number.
pub const MAX_RECEIPT_FILTER_NAME: usize = MAX_RECEIPT_RULE_SET_NAME;

/// The email templates one account may hold, and so the ceiling on the template
/// store: SES documents a limit of 10,000 email templates per account, and a
/// creation past it is refused rather than letting caller input grow this store
/// without end.
pub const MAX_TEMPLATES: usize = 10_000;

/// The custom verification email templates one account may hold, and so the
/// ceiling on that store: SES documents a limit of 50 custom verification email
/// templates per account per Region, and a creation past it is refused rather
/// than letting caller input grow this store without end.
pub const MAX_CUSTOM_VERIFICATION_EMAIL_TEMPLATES: usize = 50;

/// The bytes one custom verification email template's content may carry, and so
/// the size a creating call is refused from.
///
/// The model documents it on the member itself:
/// `CreateCustomVerificationEmailTemplateRequest$TemplateContent` is "The content
/// of the custom verification email. The total size of the email must be less
/// than 10 MB." Read in decimal megabytes, as the sentence writes them, so the
/// bound is 10,000,000 bytes rather than ten binary mebibytes. Applied where the
/// content is taken in, so this store is bounded in BYTES and not only in
/// entries ([`MAX_CUSTOM_VERIFICATION_EMAIL_TEMPLATES`]).
pub const MAX_CUSTOM_VERIFICATION_EMAIL_CONTENT_BYTES: usize = 10_000_000;

/// Whether the account a freshly booted emulator serves has been granted
/// production access.
///
/// A new Amazon SES account is in the sandbox, which is the same account state
/// the stored sandbox quota ([`SANDBOX_MAX_24_HOUR_SEND`]) already describes. No
/// operation of this service's model moves an account out of it — leaving the
/// sandbox is a separate, unmodelled account-review request — so nothing a
/// caller does changes this, and an operation that requires production access is
/// refused while [`AccountSettings::production_access_granted`] is false.
pub const PRODUCTION_ACCESS_AT_ACCOUNT_START: bool = false;

/// The bytes one sending-authorization policy document may carry.
///
/// `PutIdentityPolicyRequest$Policy` states the ceiling in prose — "The text of
/// the policy in JSON format. The policy cannot exceed 4 KB." — and the service
/// enforces that 4 KB as a round 4,000 bytes, which is also the number its
/// refusal names. The model carries only the member's lower bound (`length
/// 1..`), so this ceiling is the handler's to enforce; it is also what bounds one
/// entry of the policy store.
pub const MAX_POLICY_BYTES: usize = 4_000;

/// The sending-authorization policies ONE identity may hold at once, and so the
/// ceiling on an identity's policy map.
///
/// `GetIdentityPoliciesRequest$PolicyNames` is documented as "a list of the names
/// of policies to be retrieved. You can retrieve a maximum of 20 policies at a
/// time", so 20 policies is the whole set a caller can read back in one call —
/// the documented size of an identity's policy collection. A put past it is
/// refused rather than letting caller input grow this store without end.
pub const MAX_IDENTITY_POLICIES: usize = 20;

/// The characters of a message's subject and body the log keeps.
///
/// The log records THAT a message was accepted, with enough of it to identify
/// it; it is not a mailbox, and no operation serves the stored text back. A
/// message may be 10 MB, so a bounded prefix is kept rather than a whole body.
pub const MAX_LOGGED_TEXT: usize = 1024;

/// How long a custom MAIL FROM setup stays [`CustomMailFromStatus::Pending`]
/// before the clock owes it its move to [`CustomMailFromStatus::Success`].
///
/// SES reports the setup `Pending` until it has read the MAIL FROM domain's MX
/// record, and nothing a caller does moves it on: the move is the service's own,
/// so it is owed by the CLOCK from the moment
/// `SetIdentityMailFromDomain` records the setup. The span is this emulator's
/// choice — AWS takes as long as its DNS lookup takes — and it is not zero, so a
/// caller reading the setup straight after writing it still sees `Pending`, the
/// state whose behaviour `BehaviorOnMXFailure` describes.
pub const MAIL_FROM_SETUP_SPAN: Duration = Duration::from_secs(3);

/// How long an identity stays [`VerificationStatus::Pending`] before the clock
/// owes it its move to [`VerificationStatus::Success`].
///
/// Requesting verification only STARTS it: SES completes it when it observes
/// something this emulator cannot observe — the recipient following the link in
/// the verification message, or the domain's TXT record appearing in DNS — so
/// the span is this emulator's CHOICE rather than a documented duration, the
/// same convention the custom MAIL FROM setup follows
/// ([`MAIL_FROM_SETUP_SPAN`]). It is not zero, so a caller that reads or sends
/// straight after asking for verification still sees `Pending` and the refusals
/// that go with it. The due instant is stored on the record
/// ([`Identity::verification_due`]); nothing ever sleeps for it.
pub const IDENTITY_VERIFICATION_SPAN: Duration = Duration::from_secs(3);

/// How many Easy DKIM tokens one domain identity is given.
///
/// `VerifyDomainDkim` hands back a SET of tokens and documents what the caller
/// does with them: "Repeat this process for each token that's generated by this
/// operation", each published as its own `token._domainkey.<domain>` CNAME.
/// Amazon SES generates three.
pub const EASY_DKIM_TOKEN_COUNT: usize = 3;

/// The characters one Easy DKIM token carries, which is the length of the CNAME
/// label a caller publishes for it. Amazon SES's own tokens are 32 characters.
pub const EASY_DKIM_TOKEN_LEN: usize = 32;

/// Whether DKIM signing is on for an identity nobody has set it for.
///
/// `IdentityDkimAttributes$DkimEnabled` documents the default: "Is true if DKIM
/// signing is enabled for email sent from the identity. It's false otherwise.
/// The default value is true."
pub const DKIM_SIGNING_AT_IDENTITY_START: bool = true;

/// The class of an identity, as SES's `IdentityType` enum names it.
///
/// The wire spellings are the model's own enum values, published as
/// `crate::ops::types::IDENTITY_TYPE_VALUES`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdentityType {
    /// A single mailbox, e.g. the address `VerifyEmailIdentity` records.
    EmailAddress,
    /// A domain, which also covers the mailboxes under it and its subdomains.
    Domain,
}

impl IdentityType {
    /// The wire value for this class.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::EmailAddress => "EmailAddress",
            Self::Domain => "Domain",
        }
    }

    /// Parse a wire `IdentityType`; `None` for a value the model does not declare.
    pub fn from_wire(value: &str) -> Option<Self> {
        match value {
            "EmailAddress" => Some(Self::EmailAddress),
            "Domain" => Some(Self::Domain),
            _ => None,
        }
    }
}

/// The verification state of an identity, as SES's `VerificationStatus` enum
/// names it (wire spellings published as
/// `crate::ops::types::VERIFICATION_STATUS_VALUES`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VerificationStatus {
    /// Verification has started and has not completed.
    Pending,
    /// The identity is verified; SES accepts it as a sender.
    Success,
    /// Verification failed.
    Failed,
    /// Verification failed for a reason that may clear on its own.
    TemporaryFailure,
    /// Verification has not been attempted.
    NotStarted,
}

impl VerificationStatus {
    /// The wire value for this status.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Pending => "Pending",
            Self::Success => "Success",
            Self::Failed => "Failed",
            Self::TemporaryFailure => "TemporaryFailure",
            Self::NotStarted => "NotStarted",
        }
    }
}

/// The class of notification an identity publishes, as SES's `NotificationType`
/// enum names it (wire spellings published as
/// `crate::ops::types::NOTIFICATION_TYPE_VALUES`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotificationType {
    /// A message that could not be delivered.
    Bounce,
    /// A recipient's complaint about a delivered message.
    Complaint,
    /// A successful delivery.
    Delivery,
}

impl NotificationType {
    /// The wire value for this class.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Bounce => "Bounce",
            Self::Complaint => "Complaint",
            Self::Delivery => "Delivery",
        }
    }

    /// Parse a wire `NotificationType`; `None` for a value the model does not
    /// declare.
    ///
    /// The value set this accepts is pinned against the model's own published
    /// one: `tests/all/identity_notification_topics.rs` drives EVERY entry of
    /// `crate::ops::types::NOTIFICATION_TYPE_VALUES` through an operation that
    /// parses it here, and refuses values outside it, so a model that gained or
    /// renamed a value fails there rather than diverging quietly.
    pub fn from_wire(value: &str) -> Option<Self> {
        match value {
            "Bounce" => Some(Self::Bounce),
            "Complaint" => Some(Self::Complaint),
            "Delivery" => Some(Self::Delivery),
            _ => None,
        }
    }
}

/// Where one identity's notifications go, as
/// `GetIdentityNotificationAttributes` reports it.
///
/// A topic is `None` when none is set, which is what SES calls publishing
/// DISABLED for that class: `SetIdentityNotificationTopicRequest$SnsTopic` is
/// documented as "If the parameter is omitted from the request or a null value is
/// passed, `SnsTopic` is cleared and publishing is disabled."
#[derive(Debug, Clone)]
pub struct NotificationSettings {
    /// The topic bounce notifications are published to.
    pub bounce_topic: Option<String>,
    /// The topic complaint notifications are published to.
    pub complaint_topic: Option<String>,
    /// The topic delivery notifications are published to.
    pub delivery_topic: Option<String>,
    /// Whether bounce and complaint notifications are also forwarded as email
    /// (`SetIdentityFeedbackForwardingEnabled`).
    pub forwarding_enabled: bool,
    /// Whether the original email headers travel with a bounce notification
    /// (`SetIdentityHeadersInNotificationsEnabled`).
    pub headers_in_bounce_notifications_enabled: bool,
    /// The same, for a complaint notification.
    pub headers_in_complaint_notifications_enabled: bool,
    /// The same, for a delivery notification.
    pub headers_in_delivery_notifications_enabled: bool,
}

impl Default for NotificationSettings {
    /// A freshly recorded identity publishes nowhere and forwards feedback as
    /// email.
    ///
    /// `ForwardingEnabled` starts TRUE because SES cannot be asked for anything
    /// else yet: `SetIdentityFeedbackForwardingEnabledRequest$ForwardingEnabled`
    /// is documented as "This value can only be set to `false` when Amazon SNS
    /// topics are set for both `Bounce` and `Complaint` notification types", and
    /// an identity with no topics has neither. The header flags start false —
    /// the model's own `@default` for each of them.
    fn default() -> Self {
        Self {
            bounce_topic: None,
            complaint_topic: None,
            delivery_topic: None,
            forwarding_enabled: true,
            headers_in_bounce_notifications_enabled: false,
            headers_in_complaint_notifications_enabled: false,
            headers_in_delivery_notifications_enabled: false,
        }
    }
}

impl NotificationSettings {
    /// The topic `kind` notifications are published to, when one is set.
    pub fn topic(&self, kind: NotificationType) -> Option<&str> {
        match kind {
            NotificationType::Bounce => self.bounce_topic.as_deref(),
            NotificationType::Complaint => self.complaint_topic.as_deref(),
            NotificationType::Delivery => self.delivery_topic.as_deref(),
        }
    }

    /// Publish `kind` notifications to `topic`, or clear the topic (and so
    /// disable publishing for that class) with `None`.
    pub fn set_topic(&mut self, kind: NotificationType, topic: Option<String>) {
        let slot = match kind {
            NotificationType::Bounce => &mut self.bounce_topic,
            NotificationType::Complaint => &mut self.complaint_topic,
            NotificationType::Delivery => &mut self.delivery_topic,
        };
        *slot = topic;
    }

    /// Whether the original email headers travel with a `kind` notification.
    pub fn headers_enabled(&self, kind: NotificationType) -> bool {
        match kind {
            NotificationType::Bounce => self.headers_in_bounce_notifications_enabled,
            NotificationType::Complaint => self.headers_in_complaint_notifications_enabled,
            NotificationType::Delivery => self.headers_in_delivery_notifications_enabled,
        }
    }

    /// Include (or stop including) the original email headers in a `kind`
    /// notification.
    pub fn set_headers_enabled(&mut self, kind: NotificationType, enabled: bool) {
        let slot = match kind {
            NotificationType::Bounce => &mut self.headers_in_bounce_notifications_enabled,
            NotificationType::Complaint => &mut self.headers_in_complaint_notifications_enabled,
            NotificationType::Delivery => &mut self.headers_in_delivery_notifications_enabled,
        };
        *slot = enabled;
    }

    /// Whether a topic is set for both `Bounce` and `Complaint`, which is the
    /// condition `SetIdentityFeedbackForwardingEnabled` documents for turning
    /// feedback forwarding OFF: "This value can only be set to `false` when
    /// Amazon SNS topics are set for both `Bounce` and `Complaint` notification
    /// types."
    pub fn publishes_bounces_and_complaints(&self) -> bool {
        self.topic(NotificationType::Bounce).is_some()
            && self.topic(NotificationType::Complaint).is_some()
    }
}

/// The state of an identity's custom MAIL FROM setup, as SES's
/// `CustomMailFromStatus` enum names it (wire spellings published as
/// `crate::ops::types::CUSTOM_MAIL_FROM_STATUS_VALUES`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CustomMailFromStatus {
    /// SES has not read the required MX record yet.
    Pending,
    /// SES read the MX record: the custom MAIL FROM domain is in use.
    Success,
    /// Reading the MX record failed.
    Failed,
    /// Reading the MX record failed for a reason that may clear on its own.
    TemporaryFailure,
}

impl CustomMailFromStatus {
    /// The wire value for this status.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Pending => "Pending",
            Self::Success => "Success",
            Self::Failed => "Failed",
            Self::TemporaryFailure => "TemporaryFailure",
        }
    }
}

/// What SES does with a message when it cannot read the MAIL FROM domain's MX
/// record, as SES's `BehaviorOnMXFailure` enum names it (wire spellings published
/// as `crate::ops::types::BEHAVIOR_ON_M_X_FAILURE_VALUES`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BehaviorOnMxFailure {
    /// SES sends the message with amazonses.com (or a subdomain of it) as the
    /// MAIL FROM domain.
    ///
    /// The default, because `IdentityMailFromDomainAttributes$BehaviorOnMXFailure`
    /// is `@required` while `SetIdentityMailFromDomainRequest$BehaviorOnMXFailure`
    /// is optional, so a setup recorded without one still has a behaviour to
    /// report — and it is the one that still delivers the message.
    #[default]
    UseDefaultValue,
    /// SES answers `MailFromDomainNotVerified` and does not send the message.
    RejectMessage,
}

impl BehaviorOnMxFailure {
    /// The wire value for this behaviour.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::UseDefaultValue => "UseDefaultValue",
            Self::RejectMessage => "RejectMessage",
        }
    }

    /// Parse a wire `BehaviorOnMXFailure`; `None` for a value the model does not
    /// declare.
    ///
    /// The value set this accepts is pinned against the model's own published one:
    /// `tests/all/identity_mail_from_domain.rs` drives EVERY entry of
    /// `crate::ops::types::BEHAVIOR_ON_M_X_FAILURE_VALUES` through the operation
    /// that parses it here, and refuses values outside it.
    pub fn from_wire(value: &str) -> Option<Self> {
        match value {
            "UseDefaultValue" => Some(Self::UseDefaultValue),
            "RejectMessage" => Some(Self::RejectMessage),
            _ => None,
        }
    }
}

/// One identity's custom MAIL FROM setup, as `SetIdentityMailFromDomain` writes
/// it and `GetIdentityMailFromDomainAttributes` reports it.
///
/// An identity with no custom MAIL FROM domain holds NO settings at all
/// ([`Identity::mail_from`] is `None`): "A value of null disables the custom MAIL
/// FROM setting for the identity"
/// (`SetIdentityMailFromDomainRequest$MailFromDomain`).
#[derive(Debug, Clone)]
pub struct MailFromSettings {
    /// The custom MAIL FROM domain, as the request named it.
    pub domain: String,
    /// Whether SES has read the domain's MX record yet.
    pub status: CustomMailFromStatus,
    /// What a send does while the status is not
    /// [`CustomMailFromStatus::Success`].
    pub behavior_on_mx_failure: BehaviorOnMxFailure,
    /// The INSTANT the move out of [`CustomMailFromStatus::Pending`] is due, and
    /// `None` once there is no move left to make. An instant rather than a timer:
    /// nothing sleeps for it, and a reader materialises the move only once the
    /// clock has already made it due ([`Self::settle`]).
    pub status_due: Option<SystemTime>,
}

impl MailFromSettings {
    /// A freshly recorded setup for `domain`: `Pending`, with its move to
    /// `Success` due [`MAIL_FROM_SETUP_SPAN`] after the call that recorded it.
    pub fn pending(
        domain: String,
        behavior_on_mx_failure: BehaviorOnMxFailure,
        now: SystemTime,
    ) -> Self {
        Self {
            domain,
            status: CustomMailFromStatus::Pending,
            behavior_on_mx_failure,
            status_due: Some(now + MAIL_FROM_SETUP_SPAN),
        }
    }

    /// Materialise the status move the clock already owes at `now`, and answer
    /// the status to report.
    ///
    /// This is the ONLY write a reporting operation makes: the move was owed at
    /// `now` whether or not anyone called, so applying it changes nothing a
    /// later caller would otherwise have seen.
    pub fn settle(&mut self, now: SystemTime) -> CustomMailFromStatus {
        if self.status_due.is_some_and(|due| now >= due) {
            self.status = CustomMailFromStatus::Success;
            self.status_due = None;
        }
        self.status
    }
}

/// One identity's Easy DKIM setup, as `VerifyDomainDkim` records it and
/// `GetIdentityDkimAttributes` reports it.
///
/// An identity nobody has asked for Easy DKIM for holds NO setup at all
/// ([`Identity::dkim`] is `None`), which is the [`VerificationStatus::NotStarted`]
/// the report answers with: `IdentityDkimAttributes$DkimVerificationStatus`
/// "describes whether Amazon SES has successfully verified the DKIM DNS records
/// (tokens) published in the domain name's DNS", and for a domain whose tokens
/// were never generated there is nothing published to verify.
#[derive(Debug, Clone)]
pub struct EasyDkimSettings {
    /// The tokens the caller publishes as `<token>._domainkey.<domain>` CNAME
    /// records, in the order they were generated — [`EASY_DKIM_TOKEN_COUNT`] of
    /// them, each [`EASY_DKIM_TOKEN_LEN`] characters.
    ///
    /// Generated ONCE and kept: re-asking for Easy DKIM answers with the stored
    /// set, so the records a caller has already put in DNS keep matching what
    /// the service reports.
    pub tokens: Vec<String>,
    /// Whether Amazon SES has verified the published CNAME records.
    ///
    /// A generated setup stays [`VerificationStatus::Pending`]: what moves it on
    /// is Amazon SES reading the records in the DOMAIN's DNS — "when Amazon SES
    /// detects that you've added these records to the DNS configuration for a
    /// domain, you can start sending email from that domain" — which is the
    /// domain owner's act outside this API and in a zone this emulator never
    /// resolves. Reporting `Success` would report a detection nothing performed,
    /// so the status waits on the records instead of on a clock, which is also
    /// what a domain whose tokens are never published reads as at Amazon SES.
    pub status: VerificationStatus,
}

impl EasyDkimSettings {
    /// A freshly generated setup holding `tokens`, with the tokens still to be
    /// published ([`Self::status`]).
    pub fn pending(tokens: Vec<String>) -> Self {
        Self {
            tokens,
            status: VerificationStatus::Pending,
        }
    }
}

/// One identity in the account's identity list.
#[derive(Debug, Clone)]
pub struct Identity {
    /// The identity itself: an email address, or a domain.
    pub name: String,
    /// Which class of identity this is.
    pub identity_type: IdentityType,
    /// This identity's verification state.
    pub verification_status: VerificationStatus,
    /// The domain-verification TXT token. `Some` only for a domain identity:
    /// `IdentityVerificationAttributes$VerificationToken` is documented as
    /// "The verification token for a domain identity. Null for email address
    /// identities."
    ///
    /// The value is the account REGISTRY's
    /// ([`::service_common::ses_identities::SesIdentityRecord::verification_token`]),
    /// copied onto this record when the identity is published, so a domain recorded
    /// through either SES surface carries one TXT value and every read of it answers
    /// the same.
    pub verification_token: Option<String>,
    /// This identity's Easy DKIM setup, when one has been asked for:
    /// `VerifyDomainDkim` records it and `GetIdentityDkimAttributes` reports it.
    /// `None` is Easy DKIM NEVER SET UP for the identity, which is how every
    /// identity starts.
    ///
    /// Easy DKIM is set up for the ACCOUNT's identity, so the setup here is the one
    /// the account's identity registry carries
    /// ([`::service_common::ses_identities::SesDkimSetup`]) whenever another API over
    /// those identities recorded it: one identity, one set of tokens its owner
    /// publishes, whichever API handed them out.
    pub dkim: Option<EasyDkimSettings>,
    /// Whether DKIM signing is on for email sent from this identity, as
    /// `SetIdentityDkimEnabled` writes it and
    /// `GetIdentityDkimAttributes` reports it. Starts at
    /// [`DKIM_SIGNING_AT_IDENTITY_START`].
    ///
    /// Independent of [`Self::dkim`]: the flag says whether SES signs, the setup
    /// says whether the tokens it would sign with have been published.
    pub dkim_enabled: bool,
    /// Where this identity's notifications go, as
    /// `SetIdentityNotificationTopic` writes it and
    /// `GetIdentityNotificationAttributes` reports it.
    pub notifications: NotificationSettings,
    /// This identity's custom MAIL FROM setup, when one is set:
    /// `SetIdentityMailFromDomain` records it and
    /// `GetIdentityMailFromDomainAttributes` reports it. `None` is custom MAIL
    /// FROM DISABLED for the identity, which is how every identity starts.
    pub mail_from: Option<MailFromSettings>,
    /// The sending-authorization policies attached to this identity, keyed by
    /// policy name.
    ///
    /// Held ON the identity record, because a policy exists only for an identity
    /// the account owns: nothing can attach one to an identity that is not in
    /// the list, and removing the identity removes its policies with it. A
    /// `BTreeMap` so `ListIdentityPolicies` answers in one stable
    /// (lexicographic) order, and bounded at [`MAX_IDENTITY_POLICIES`] entries
    /// of [`MAX_POLICY_BYTES`] each.
    ///
    /// The set is the ACCOUNT's identity's, so it is the one the account's identity
    /// registry carries
    /// ([`::service_common::ses_identities::SesIdentityRecord::policies`]): a policy
    /// attached through another API over these identities is listed here, and one
    /// attached here is listed there. Every write publishes it
    /// ([`publish_identity`]).
    ///
    /// The document is kept VERBATIM as the caller submitted it, so
    /// `GetIdentityPolicies` reports back the text that was stored rather than a
    /// re-rendering of it.
    pub policies: BTreeMap<String, String>,
    /// When the identity was added to the list.
    pub verified_at: SystemTime,
    /// The INSTANT the move out of [`VerificationStatus::Pending`] is due, and
    /// `None` once there is no move left to make. An instant rather than a
    /// timer: nothing sleeps for it, and a reader materialises the move only
    /// once the clock has already made it due ([`Self::settle_verification`]).
    pub verification_due: Option<SystemTime>,
    /// Whether the account's identity registry
    /// ([`::service_common::ses_identities::SesIdentityDirectory`]) already carries
    /// this identity.
    ///
    /// The registry is the account's SET of identities, which the SES v2 surface
    /// publishes to and reads as well, so this flag is what tells an identity
    /// DELETED through that surface — registered here, gone from the registry —
    /// from one this service has recorded but not published yet
    /// ([`reconcile_identities`]).
    pub registered: bool,
}

impl Identity {
    /// Materialise the verification move the clock already owes this identity at
    /// `now`, and answer the status to report.
    ///
    /// This is the ONLY write a reporting operation makes about an identity's
    /// verification: the move was owed at `now` whether or not anyone called, so
    /// applying it changes nothing a later caller would otherwise have seen.
    pub fn settle_verification(&mut self, now: SystemTime) -> VerificationStatus {
        if self.verification_status == VerificationStatus::Pending
            && self.verification_due.is_some_and(|due| now >= due)
        {
            self.verification_status = VerificationStatus::Success;
            self.verification_due = None;
        }
        self.verification_status
    }
}

/// One entry of the append-only send log.
#[derive(Debug, Clone)]
pub struct SentMessage {
    /// The id `SendEmail` answered with.
    pub message_id: String,
    /// The `Source` the message was sent from.
    pub source: String,
    /// Every recipient, To: + Cc: + Bcc: flattened in that order.
    pub destinations: Vec<String>,
    /// The message subject, when the message carried one.
    pub subject: Option<String>,
    /// The message body (text part if present, else the HTML part).
    pub body: Option<String>,
    /// The configuration set the send named, when it named one.
    pub configuration_set: Option<String>,
    /// The message tags, in request order.
    pub tags: Vec<(String, String)>,
    /// When the message was accepted.
    pub sent_at: SystemTime,
}

/// The account-level sending settings every send is checked against.
#[derive(Debug, Clone)]
pub struct AccountSettings {
    /// The daily sending quota, reported as `GetSendQuota$Max24HourSend`.
    pub max_24_hour_send: f64,
    /// The maximum send rate, reported as `GetSendQuota$MaxSendRate`.
    pub max_send_rate: f64,
    /// Whether sending is enabled for the whole account
    /// (`UpdateAccountSendingEnabled`); a send is refused with
    /// `AccountSendingPausedException` while it is false.
    pub sending_enabled: bool,
    /// Whether this account has been granted production access, which is the
    /// opposite of being in the sandbox. An operation that SES serves only to an
    /// account out of the sandbox — `SendCustomVerificationEmail`, whose
    /// `ProductionAccessNotGrantedException` reports exactly this — is refused
    /// while it is false. Starts at [`PRODUCTION_ACCESS_AT_ACCOUNT_START`].
    pub production_access_granted: bool,
}

impl Default for AccountSettings {
    fn default() -> Self {
        Self {
            max_24_hour_send: SANDBOX_MAX_24_HOUR_SEND,
            max_send_rate: SANDBOX_MAX_SEND_RATE,
            sending_enabled: true,
            production_access_granted: PRODUCTION_ACCESS_AT_ACCOUNT_START,
        }
    }
}

/// One email template in the account's template store.
///
/// The members are the model's own `Template` shape: a name, and the subject,
/// text and HTML parts a send renders. `created_at` is the instant the template
/// was recorded, which `ListTemplates` reports as
/// `TemplateMetadata$CreatedTimestamp`.
#[derive(Debug, Clone)]
pub struct EmailTemplate {
    /// The template's name, which is its key in the store.
    pub name: String,
    /// `Template$SubjectPart` — "The subject line of the email."
    pub subject_part: Option<String>,
    /// `Template$TextPart` — the body shown to clients that display no HTML.
    pub text_part: Option<String>,
    /// `Template$HtmlPart` — "The HTML body of the email."
    pub html_part: Option<String>,
    /// When the template was recorded.
    pub created_at: SystemTime,
    /// Whether the account's email-template registry already carries this template
    /// ([`templates`]). False on a record this surface has just recorded and not published
    /// yet; a record that is registered here and gone from the registry was DELETED through
    /// the other SES surface, so a reconciliation drops it.
    pub registered: bool,
}

/// A configuration set a send may name.
#[derive(Debug, Clone, Default)]
pub struct ConfigurationSet {
    /// The set's name, which is its key in the store.
    pub name: String,
    /// Whether sending is enabled for this set
    /// (`UpdateConfigurationSetSendingEnabled`); a send that names it is refused
    /// with `ConfigurationSetSendingPausedException` while it is false.
    pub sending_enabled: bool,
    /// Whether Amazon SES publishes reputation metrics (bounce and complaint
    /// rates) to Amazon CloudWatch for this set
    /// (`UpdateConfigurationSetReputationMetricsEnabled`), reported as
    /// `ReputationOptions$ReputationMetricsEnabled`. Per set: the member
    /// documents "The default value is false", and one set's flag says nothing
    /// about another's.
    pub reputation_metrics_enabled: bool,
    /// The custom open-and-click redirect domain of this set's `TrackingOptions`
    /// sub-resource.
    ///
    /// The sub-resource EXISTS exactly while this is `Some`: that is what
    /// `CreateConfigurationSetTrackingOptions` adds, what
    /// `UpdateConfigurationSetTrackingOptions` replaces (and what its
    /// `TrackingOptionsDoesNotExistException` reports the absence of), and what
    /// `DescribeConfigurationSet` reports as `TrackingOptions`.
    pub custom_redirect_domain: Option<String>,
    /// The `TlsPolicy` of this set's `DeliveryOptions` sub-resource: whether a
    /// message that uses the set is only delivered over a TLS connection
    /// (`Require`) or may fall back to plain text (`Optional`).
    ///
    /// The sub-resource EXISTS exactly while this is `Some`: that is what
    /// `PutConfigurationSetDeliveryOptions` writes and what
    /// `DescribeConfigurationSet` reports as `DeliveryOptions`. The model
    /// documents no default for `DeliveryOptions$TlsPolicy`, so a set nobody has
    /// written delivery options for carries none rather than a policy the
    /// service never stored.
    pub tls_policy: Option<String>,
    /// When this set's reputation metrics were last reset — a *fresh start* —
    /// and `None` while sending for the set "has never been disabled and later
    /// re-enabled", which is what `ReputationOptions$LastFreshStart` documents
    /// and what `DescribeConfigurationSet` reports.
    ///
    /// Written only by [`Self::set_sending_enabled`], which is where that rule
    /// lives.
    pub last_fresh_start: Option<SystemTime>,
    /// The event destinations this set publishes its sending events to, in the
    /// order they were added.
    ///
    /// Held as the modelled [`crate::ops::types::EventDestination`] shape, so a
    /// destination a caller wrote is reported back unchanged by
    /// `DescribeConfigurationSet` instead of being projected through a second
    /// copy of the same structure. Grown ONLY through
    /// [`Self::add_event_destination`], which is where its ceiling
    /// ([`MAX_EVENT_DESTINATIONS`]) is applied; a destination's `Name` is its
    /// key within the set, which is what
    /// `CreateConfigurationSetEventDestination`'s
    /// `EventDestinationAlreadyExistsException` reports a conflict on.
    pub event_destinations: Vec<crate::ops::types::EventDestination>,
    /// Whether the ACCOUNT's configuration-set registry
    /// ([`::service_common::ses_configuration_sets`]) already carries this set.
    ///
    /// A set this service recorded is published there, so the SES v2 surface answers for it
    /// too; this flag is what tells a set deleted through that surface — registered here,
    /// gone from the registry — from one this service has recorded but not published yet
    /// ([`reconcile_configuration_sets`] copies it back onto this record).
    pub registered: bool,
}

impl ConfigurationSet {
    /// Enable or disable email sending for this set, as of `at`.
    ///
    /// Re-enabling a set whose sending was disabled resets the set's reputation
    /// metrics — `ReputationOptions$LastFreshStart`: "When you disable email
    /// sending for a configuration set [...] and later re-enable it, the
    /// reputation metrics for the configuration set (but not for the entire
    /// Amazon SES account) are reset." So the fresh start is stamped on the
    /// disabled-to-enabled transition only: a call that sets the state the set
    /// already holds is idempotent and resets nothing, and disabling never
    /// stamps one.
    pub fn set_sending_enabled(&mut self, enabled: bool, at: SystemTime) {
        if enabled && !self.sending_enabled {
            self.last_fresh_start = Some(at);
        }
        self.sending_enabled = enabled;
    }

    /// This set's event destination named `name`, and `None` while it carries
    /// none of that name.
    ///
    /// The name is a key, so it is compared exactly: `EventDestination$Name`
    /// admits both letter cases and nothing normalises one into the other.
    pub fn event_destination(&self, name: &str) -> Option<&crate::ops::types::EventDestination> {
        self.event_destinations
            .iter()
            .find(|destination| destination.name == name)
    }

    /// Replace this set's event destination named like `destination`, keeping
    /// the place it holds in the list, and say whether there was one to replace.
    ///
    /// `false` when the set carries no destination of that name: the caller
    /// refuses the request with the modelled `EventDestinationDoesNotExist`
    /// rather than recording a destination nobody created — that is what
    /// [`Self::add_event_destination`] is for. The place is kept because the
    /// order the destinations were added in is what every reader reports them
    /// in, and replacing one of them reorders none.
    pub fn replace_event_destination(
        &mut self,
        destination: crate::ops::types::EventDestination,
    ) -> bool {
        let Some(held) = self
            .event_destinations
            .iter_mut()
            .find(|held| held.name == destination.name)
        else {
            return false;
        };
        *held = destination;
        true
    }

    /// Record `destination` on this set, and say whether it was recorded.
    ///
    /// `false` when the set already holds [`MAX_EVENT_DESTINATIONS`]
    /// destinations: the caller refuses the request with the modelled
    /// `LimitExceeded` rather than growing the set past the limit. The caller
    /// has already decided that the name is free — a name the set already
    /// carries is a naming conflict, which
    /// `EventDestinationAlreadyExistsException` reports.
    pub fn add_event_destination(
        &mut self,
        destination: crate::ops::types::EventDestination,
    ) -> bool {
        if self.event_destinations.len() >= MAX_EVENT_DESTINATIONS {
            return false;
        }
        self.event_destinations.push(destination);
        true
    }

    /// Take this set's event destination named `name` off it, and say whether
    /// there was one to take.
    ///
    /// `false` when the set carries no destination of that name: the caller
    /// refuses the request with the modelled `EventDestinationDoesNotExist`
    /// rather than answering the removal of nothing. The destinations that
    /// remain keep their order, which is the order every reader reports them in,
    /// and the name is freed, so a later create may record one of that name
    /// again. The comparison is exact, for the reason
    /// [`Self::event_destination`] gives.
    pub fn remove_event_destination(&mut self, name: &str) -> bool {
        let Some(at) = self
            .event_destinations
            .iter()
            .position(|held| held.name == name)
        else {
            return false;
        };
        self.event_destinations.remove(at);
        true
    }
}

/// A receipt rule set: a named, ordered collection of receipt rules.
///
/// The rules are kept as the modelled [`crate::ops::types::ReceiptRule`] shape,
/// so a rule a caller wrote is reported back unchanged by every reader instead
/// of being projected through a second copy of the same structure.
#[derive(Debug, Clone)]
pub struct ReceiptRuleSet {
    /// The set's name, which is its key in the store.
    pub name: String,
    /// When the set was created, reported as
    /// `ReceiptRuleSetMetadata$CreatedTimestamp`.
    pub created_at: SystemTime,
    /// The set's rules, in the order SES applies them. Grown ONLY through
    /// [`Self::insert_rule`], which is where its ceiling
    /// ([`MAX_RECEIPT_RULES_PER_RULE_SET`]) is applied; a handler that pushed
    /// here directly would let caller input grow one set without end.
    pub rules: Vec<crate::ops::types::ReceiptRule>,
}

impl ReceiptRuleSet {
    /// Where the rule named `name` sits in this set's ordered rule list, and
    /// `None` when the set holds no rule of that name.
    ///
    /// A rule's position is what the order incoming mail is evaluated in is made
    /// of, so every operation that places a rule relative to another one reads
    /// it from here.
    pub fn rule_position(&self, name: &str) -> Option<usize> {
        self.rules.iter().position(|held| held.name == name)
    }

    /// Add `rule` to this set at `at`, the position it is to occupy in the
    /// evaluation order, pushing the rules from that position on one place
    /// later.
    ///
    /// The caller has already decided that the rule's name is free — a name the
    /// set already holds is a naming conflict, which the creating operation
    /// answers with its own modelled `AlreadyExists` — so this only applies the
    /// set's own ceiling: `false`, having stored nothing, when the set already
    /// holds [`MAX_RECEIPT_RULES_PER_RULE_SET`] rules, which the caller refuses
    /// with the modelled `LimitExceeded` rather than growing the list past the
    /// limit. A position past the end appends, so the list keeps no gaps.
    pub fn insert_rule(&mut self, rule: crate::ops::types::ReceiptRule, at: usize) -> bool {
        if self.rules.len() >= MAX_RECEIPT_RULES_PER_RULE_SET {
            return false;
        }
        self.rules.insert(at.min(self.rules.len()), rule);
        true
    }

    /// Move the rule held at `from` so that it sits directly AFTER the rule held
    /// at `after`, or first in the evaluation order when `after` is `None`.
    ///
    /// The rule travels whole and the set's length never changes — a move
    /// rewrites positions and nothing else — so this applies no ceiling of its
    /// own ([`Self::insert_rule`] is where a GROWING set meets
    /// [`MAX_RECEIPT_RULES_PER_RULE_SET`]). The index arithmetic lives here
    /// rather than in a handler: taking the rule out first shifts every later
    /// rule one place earlier, so the target index depends on which side of the
    /// rule its anchor sits on. Asking for the position a rule already occupies
    /// (its anchor is itself, or the rule directly before it) leaves the order
    /// untouched.
    pub fn move_rule(&mut self, from: usize, after: Option<usize>) {
        if from >= self.rules.len() {
            return;
        }
        let to = match after {
            // "If this parameter is null, the rule is inserted at the beginning
            // of the rule list" — the first position in the evaluation order.
            None => 0,
            // The anchor sits before the rule: once the rule is taken out the
            // anchor keeps its index, so the place directly after it is `at + 1`.
            Some(at) if at < from => at + 1,
            // The anchor sits at or after the rule: taking the rule out moves
            // the anchor to `at - 1`, so the place directly after it is `at`.
            // An anchor past the end of the list anchors to the last rule.
            Some(at) => at.min(self.rules.len() - 1),
        };
        if to == from {
            return;
        }
        let rule = self.rules.remove(from);
        self.rules.insert(to, rule);
    }

    /// Take the rule named `name` out of this set, and say whether a rule was
    /// taken out.
    ///
    /// The rules that sat after it close up, so the order the rules this set
    /// keeps are evaluated in is unchanged. `false`, having changed nothing, when
    /// the set holds no rule of that name — which the deleting operation, modelling
    /// no not-found error for a rule, answers as the state the caller asked for.
    pub fn remove_rule(&mut self, name: &str) -> bool {
        match self.rule_position(name) {
            Some(at) => {
                self.rules.remove(at);
                true
            }
            None => false,
        }
    }
}

/// One custom verification email template in the account's store.
///
/// The members are the model's own, as the creating request carries them
/// (`CreateCustomVerificationEmailTemplateRequest`): the name, the sender, the
/// subject, the content, and the two URLs a recipient is sent to. That is a
/// SUPERSET of the modelled `CustomVerificationEmailTemplate` shape, which
/// carries no `TemplateContent` while `GetCustomVerificationEmailTemplate`
/// reports one — so the record keeps what every reader of it needs instead of
/// that shape alone.
///
/// Kept VERBATIM as the caller submitted it: a reader reports back the text that
/// was stored rather than a re-rendering of it.
#[derive(Debug, Clone, Default)]
pub struct CustomVerificationEmailTemplate {
    /// `TemplateName`, which is this record's key in the store.
    pub template_name: String,
    /// `FromEmailAddress` — "The email address that the custom verification
    /// email is sent from."
    pub from_email_address: String,
    /// `TemplateSubject` — "The subject line of the custom verification email."
    pub template_subject: String,
    /// `TemplateContent` — "The content of the custom verification email",
    /// bounded at [`MAX_CUSTOM_VERIFICATION_EMAIL_CONTENT_BYTES`] by the
    /// operations that take it in.
    pub template_content: String,
    /// `SuccessRedirectionURL` — where a recipient whose address was verified is
    /// sent.
    pub success_redirection_url: String,
    /// `FailureRedirectionURL` — where a recipient whose address was not
    /// verified is sent.
    pub failure_redirection_url: String,
}

/// Per-service state for com-amazonaws-ses-simpleemailservice. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsSesSimpleemailserviceState {
    /// The identity list, keyed by [`identity_key`] — the key the ACCOUNT's
    /// identity registry compares identities by, so one identity of the account is
    /// one entry here however a request spelled the DNS name in it. The spelling it
    /// was recorded with is [`Identity::name`], which is the value every listing and
    /// report answers with. A `BTreeMap` so every listing is in one stable
    /// (lexicographic) order, which is what makes `ListIdentities`' pagination
    /// resumable. Bounded at [`MAX_IDENTITIES`].
    pub identities: BTreeMap<String, Identity>,
    /// The send log, oldest first. Appended through
    /// [`Self::log_sent_message`], which is where its bounds are applied.
    pub sent_messages: Vec<SentMessage>,
    /// The account's sending settings.
    pub account: AccountSettings,
    /// The configuration sets, keyed by name. Grown ONLY through
    /// [`Self::add_configuration_set`], which is where its ceiling
    /// ([`MAX_CONFIGURATION_SETS`]) is applied; a handler that inserted here
    /// directly would let caller input grow this store without end.
    pub configuration_sets: BTreeMap<String, ConfigurationSet>,
    /// The receipt rule sets, keyed by name. A `BTreeMap` so every listing is in
    /// one stable (lexicographic) order. Bounded at [`MAX_RECEIPT_RULE_SETS`].
    pub receipt_rule_sets: BTreeMap<String, ReceiptRuleSet>,
    /// The NAME of the receipt rule set email receiving is currently served by,
    /// and `None` while no set is active — which is where an account starts, and
    /// what disables email receiving completely.
    ///
    /// A name rather than a copy of the set, so the active set's metadata and
    /// rules are the ones its entry in [`Self::receipt_rule_sets`] holds at the
    /// moment of the read ([`Self::active_rule_set`]) instead of a second,
    /// stale copy of them. This is a pointer into that store, so it grows by at
    /// most one name and needs no ceiling of its own.
    pub active_receipt_rule_set: Option<String>,
    /// The receipt IP address filters, keyed by name, held as the modelled
    /// [`crate::ops::types::ReceiptFilter`] shape so a filter a caller wrote is
    /// reported back unchanged instead of being projected through a second copy
    /// of the same structure. A `BTreeMap` so every listing is in one stable
    /// (lexicographic) order. Grown ONLY through [`Self::add_receipt_filter`],
    /// which is where its ceiling ([`MAX_RECEIPT_FILTERS`]) is applied.
    pub receipt_filters: BTreeMap<String, crate::ops::types::ReceiptFilter>,
    /// The email templates, keyed by name. A `BTreeMap` so every listing is in
    /// one stable (lexicographic) order, which is what makes `ListTemplates`'
    /// pagination resumable. Bounded at [`MAX_TEMPLATES`].
    pub templates: BTreeMap<String, EmailTemplate>,
    /// The custom verification email templates a `SendCustomVerificationEmail`
    /// may name, keyed by template name and held as
    /// [`CustomVerificationEmailTemplate`], which carries every member the
    /// creating request does. A `BTreeMap` so
    /// every listing is in one stable (lexicographic) order, which is what makes
    /// `ListCustomVerificationEmailTemplates`' pagination resumable. Grown ONLY
    /// through [`Self::put_custom_verification_email_template`], which is where
    /// its ceiling ([`MAX_CUSTOM_VERIFICATION_EMAIL_TEMPLATES`]) is applied.
    pub custom_verification_email_templates: BTreeMap<String, CustomVerificationEmailTemplate>,
    /// How many opaque ids this store has minted, which is what makes the next
    /// one UNIQUE rather than merely unlikely to repeat: every id digests this
    /// counter, and the counter is never reused. Written only by
    /// [`Self::mint_token`].
    minted_tokens: u64,
}

impl ComAmazonawsSesSimpleemailserviceState {
    /// Add `name` to the identity list as an identity of `identity_type`, and
    /// hand back the stored record.
    ///
    /// Idempotent: re-verifying an identity leaves the existing record — and the
    /// verification token a domain identity carries — in place, so the list never
    /// holds a duplicate, a token stays stable across reads, and asking again
    /// does not restart the span an identity already part-way through it has
    /// left to run.
    ///
    /// The verification TOKEN a domain identity carries is not minted here: it is
    /// the account registry's, which both SES surfaces publish an identity to, so a
    /// domain recorded through either one carries the same TXT value
    /// ([`reconcile_identities`] copies it back onto this record).
    ///
    /// The record is stored [`VerificationStatus::Pending`]: requesting
    /// verification only STARTS it — the identity's owner has yet to follow the
    /// link in the message SES sends, or to publish the domain's TXT record — so
    /// a send from the identity is refused until the move to
    /// [`VerificationStatus::Success`] the clock owes it falls due,
    /// [`IDENTITY_VERIFICATION_SPAN`] after this call
    /// ([`Identity::settle_verification`]).
    ///
    /// `None` when the list already holds [`MAX_IDENTITIES`] identities and this
    /// is a new one: the caller refuses the request rather than growing the store
    /// past the account's limit. Re-verifying an identity already in the list
    /// always succeeds, since it adds nothing.
    pub fn verify_identity(
        &mut self,
        name: &str,
        identity_type: IdentityType,
    ) -> Option<&Identity> {
        let key = identity_key(name);
        let is_new = !self.identities.contains_key(&key);
        if is_new && self.identities.len() >= MAX_IDENTITIES {
            return None;
        }
        let requested_at = SystemTime::now();
        Some(
            self.identities
                .entry(key)
                .or_insert_with(|| Identity {
                    name: name.to_string(),
                    identity_type,
                    verification_status: VerificationStatus::Pending,
                    verification_token: None,
                    dkim: None,
                    dkim_enabled: DKIM_SIGNING_AT_IDENTITY_START,
                    notifications: NotificationSettings::default(),
                    mail_from: None,
                    policies: BTreeMap::new(),
                    verified_at: requested_at,
                    verification_due: requested_at.checked_add(IDENTITY_VERIFICATION_SPAN),
                    // Published to the account registry by the operation that
                    // recorded it, so the other SES surface answers for the
                    // identity from the moment this call returns.
                    registered: false,
                }),
        )
    }

    /// Materialise the verification move the clock owes every identity of this
    /// account at `now` ([`Identity::settle_verification`]).
    ///
    /// Called by each operation that READS an identity's verification — the
    /// status report, the sender gate on a send, the verified-address listing,
    /// the tracking-domain check — so every one of them reads the same store the
    /// clock has already carried forward, rather than one reader seeing a move
    /// another reader does not.
    pub fn settle_identities(&mut self, now: SystemTime) {
        for identity in self.identities.values_mut() {
            identity.settle_verification(now);
        }
    }

    /// Set up Easy DKIM for the domain identity `domain`, and hand back the
    /// tokens it is to publish as CNAME records.
    ///
    /// The domain is ADDED to the identity list if it is not in it already:
    /// `VerifyDomainDkim` documents that "the domain that you specify is added
    /// to the list of identities that are associated with your account […] even
    /// if you haven't already associated the domain with your account by using
    /// the VerifyDomainIdentity operation".
    ///
    /// Idempotent, and the tokens are STORED: a domain that already carries a
    /// setup keeps it, so asking again answers with the same tokens — the
    /// records a caller has already published stay the ones the service reports.
    ///
    /// `held` is the setup the ACCOUNT already holds for the domain, when another
    /// API over the account's identities recorded one: Easy DKIM is set up for the
    /// identity rather than per API, so the tokens its owner publishes are those and
    /// this call stores and answers with them instead of minting a second set.
    ///
    /// `None` when the list already holds [`MAX_IDENTITIES`] identities and this
    /// is a new one, which is the ceiling [`Self::verify_identity`] applies.
    pub fn request_easy_dkim(
        &mut self,
        domain: &str,
        held: Option<EasyDkimSettings>,
    ) -> Option<Vec<String>> {
        self.verify_identity(domain, IdentityType::Domain)?;
        let key = identity_key(domain);
        if let Some(stored) = self
            .identities
            .get(&key)
            .and_then(|identity| identity.dkim.as_ref())
        {
            return Some(stored.tokens.clone());
        }
        // Minted only for a domain that is about to be given a setup and that the
        // account holds no setup for, so asking again never generates a second set
        // of tokens for one domain.
        let setup = match held {
            Some(held) => held,
            None => EasyDkimSettings::pending(
                (0..EASY_DKIM_TOKEN_COUNT)
                    .map(|_| self.new_dkim_token())
                    .collect(),
            ),
        };
        let tokens = setup.tokens.clone();
        let identity = self.identities.get_mut(&key)?;
        identity.dkim = Some(setup);
        Some(tokens)
    }

    /// The Easy DKIM setup that covers `identity`, and `None` while nothing
    /// covers it.
    ///
    /// Its OWN setup for a domain identity, and its domain's for an email
    /// address: `GetIdentityDkimAttributes` reports "a set of DKIM tokens that
    /// represent the identity. If the identity is an email address, the tokens
    /// represent the domain of that address", and Easy DKIM is set up on a
    /// domain, so an address is covered by the nearest domain identity above it
    /// — the same reading [`Self::covers_verified_domain`] applies to a sender's
    /// domain.
    pub fn easy_dkim_for(&self, identity: &str) -> Option<&EasyDkimSettings> {
        let key = identity_key(identity);
        if let Some(own) = self
            .identities
            .get(&key)
            .and_then(|stored| stored.dkim.as_ref())
        {
            return Some(own);
        }
        let (_, domain) = key.split_once('@')?;
        // Finished inside the body: the local `key` has to outlive the walk.
        for suffix in domain_suffixes(domain) {
            if let Some(covering) = self
                .identities
                .get(suffix)
                .and_then(|stored| stored.dkim.as_ref())
            {
                return Some(covering);
            }
        }
        None
    }

    /// Record `template` under its own name, replacing any template already
    /// stored under that name, and hand back the stored record.
    ///
    /// `None` when the store already holds [`MAX_TEMPLATES`] templates and this
    /// is a new one: the caller refuses the request rather than growing the store
    /// past the account's limit. Replacing a template already in the store always
    /// succeeds, since it adds nothing.
    pub fn put_template(&mut self, template: EmailTemplate) -> Option<&EmailTemplate> {
        let name = template.name.clone();
        if !self.templates.contains_key(&name) && self.templates.len() >= MAX_TEMPLATES {
            return None;
        }
        self.templates.insert(name.clone(), template);
        self.templates.get(&name)
    }

    /// Record `template` under the template name it carries, replacing any
    /// custom verification email template already stored under that name, and
    /// hand back the stored record.
    ///
    /// `None` when the store already holds
    /// [`MAX_CUSTOM_VERIFICATION_EMAIL_TEMPLATES`] templates and this is a new
    /// one: the caller refuses the request rather than growing the store past the
    /// account's limit. Replacing a template already in the store always
    /// succeeds, since it adds nothing.
    ///
    /// `CreateCustomVerificationEmailTemplate` is the creating caller; it refuses
    /// a name the store already holds before reaching here, so what replaces a
    /// stored template is `UpdateCustomVerificationEmailTemplate`.
    pub fn put_custom_verification_email_template(
        &mut self,
        template: CustomVerificationEmailTemplate,
    ) -> Option<&CustomVerificationEmailTemplate> {
        let name = template.template_name.clone();
        if !self.custom_verification_email_templates.contains_key(&name)
            && self.custom_verification_email_templates.len()
                >= MAX_CUSTOM_VERIFICATION_EMAIL_TEMPLATES
        {
            return None;
        }
        self.custom_verification_email_templates
            .insert(name.clone(), template);
        self.custom_verification_email_templates.get(&name)
    }

    /// Append an accepted message to the send log, bounding it as it grows.
    ///
    /// Two bounds, applied here so every writer gets them: the entries that have
    /// left [`SEND_LOG_RETENTION`] are dropped — no reader counts them any more,
    /// so this only materialises an expiry the clock has already made due — and,
    /// as a backstop, the oldest entries past [`MAX_SENT_LOG_ENTRIES`].
    ///
    /// The expiry is the LONGEST window a reader reports over, not the shortest:
    /// an entry the sending quota has stopped counting is still inside the
    /// window the sending statistics report, so dropping it at the quota's
    /// window would leave the statistics unable to report the two weeks they
    /// document.
    pub fn log_sent_message(&mut self, message: SentMessage) {
        let now = message.sent_at;
        self.sent_messages
            .retain(|logged| within_window(logged.sent_at, now, SEND_LOG_RETENTION));
        self.sent_messages.push(message);
        if let Some(excess) = self
            .sent_messages
            .len()
            .checked_sub(MAX_SENT_LOG_ENTRIES)
            .filter(|excess| *excess > 0)
        {
            self.sent_messages.drain(..excess);
        }
    }

    /// Whether SES would accept `address` as a `Source`.
    ///
    /// `SendEmail`: "The message must be sent from a verified email address or
    /// domain." So the addr-spec itself may be a verified identity, or any
    /// domain suffix of it may be — a verified domain covers the mailboxes in it
    /// and in its subdomains. An identity whose verification was only REQUESTED
    /// is not one of them: the caller settles the store first
    /// ([`Self::settle_identities`]), so what this reads is the verification the
    /// clock has carried through.
    ///
    /// The address is keyed ONCE ([`identity_key`]) and every name walked from that
    /// key, so a domain the identity was recorded with in another case covers the
    /// sender all the same — which is how DNS compares a domain name.
    pub fn is_verified_sender(&self, address: &str) -> bool {
        let key = identity_key(address);
        // Finished inside the body: the walk borrows the local `key`, which has to
        // outlive the iterator over it.
        for name in sender_identity_keys(&key) {
            if self.holds_verified_key(name) {
                return true;
            }
        }
        false
    }

    /// Whether this account HOLDS an identity for `address` — the addr-spec
    /// itself, or any domain suffix of it — whatever verification state the
    /// list holds for that identity.
    ///
    /// The weaker reading of a sender: the account has asked SES about the
    /// address, which a verification that has not completed is still evidence of,
    /// while whether the address is ELIGIBLE TO SEND is
    /// [`Self::is_verified_sender`] — which is what every operation that gates on
    /// a sender reads. Both readings walk the same names
    /// ([`sender_identity_keys`]), so an identity for a domain covers the
    /// mailboxes in it and in its subdomains here too, and both compare a domain
    /// the way DNS does.
    pub fn holds_sending_identity(&self, address: &str) -> bool {
        let key = identity_key(address);
        for name in sender_identity_keys(&key) {
            if self.identities.contains_key(name) {
                return true;
            }
        }
        false
    }

    /// Whether `domain` is itself a verified domain identity, or sits under one.
    ///
    /// A verified domain covers its subdomains, which is the same reading
    /// [`Self::is_verified_sender`] applies to a sender's domain; it is what lets
    /// a custom tracking subdomain of a verified domain be used.
    pub fn covers_verified_domain(&self, domain: &str) -> bool {
        let key = identity_key(domain);
        // Finished inside the body: the local `key` has to outlive the walk.
        for suffix in domain_suffixes(&key) {
            if self.holds_verified_key(suffix) {
                return true;
            }
        }
        false
    }

    /// Whether the identity list holds `key` — already an identity KEY — with a
    /// successful verification.
    ///
    /// The one lookup behind [`Self::is_verified`] and the sender readings above,
    /// so a caller that has keyed its value once is not made to key it again per
    /// candidate name.
    fn holds_verified_key(&self, key: &str) -> bool {
        self.identities
            .get(key)
            .is_some_and(|identity| identity.verification_status == VerificationStatus::Success)
    }

    /// The sending-authorization policies attached to `identity`, or `None` when
    /// this account holds no such identity.
    ///
    /// The set is the ACCOUNT's identity's, brought into agreement with the
    /// account's identity registry on every call ([`reconcile_identities`]), so a
    /// policy attached through another API over these identities is in it.
    ///
    /// `None` is "this account holds no such identity": `ListIdentityPolicies`
    /// answers that with an empty list — it models no error and the recorded
    /// service serves such a call — while `GetIdentityPolicies`,
    /// `PutIdentityPolicy` and `DeleteIdentityPolicy` refuse on it, the refusal
    /// their own documentation states.
    pub fn identity_policies(&self, identity: &str) -> Option<&BTreeMap<String, String>> {
        self.identities
            .get(&identity_key(identity))
            .map(|stored| &stored.policies)
    }

    /// [`Self::identity_policies`] for a writer: the same map, keyed the same
    /// way, so a policy is put on and read back from one record.
    pub fn identity_policies_mut(
        &mut self,
        identity: &str,
    ) -> Option<&mut BTreeMap<String, String>> {
        self.identities
            .get_mut(&identity_key(identity))
            .map(|stored| &mut stored.policies)
    }

    /// Whether `name` is in the identity list with a successful verification.
    ///
    /// The STORED status, which the caller has already settled against the clock
    /// ([`Self::settle_identities`]). The name is keyed the way the list is
    /// ([`identity_key`]), so a domain cased unlike the one the identity was
    /// recorded with names the same identity — which is how DNS compares a domain
    /// name.
    pub fn is_verified(&self, name: &str) -> bool {
        self.holds_verified_key(&identity_key(name))
    }

    /// The custom redirect domain a tracking-options request may be stored with,
    /// or the message Amazon SES refuses it as invalid tracking options with.
    ///
    /// Two conditions, which `InvalidTrackingOptionsException` lists as the ones
    /// it "appears most often in" — a value that is not "a valid domain or
    /// subdomain", and a tracking domain that is "not verified in Amazon SES" —
    /// and which the runtime reports apart, each in its own sentence NAMING the
    /// value it rejected ([`invalid_tracking_domain_message`] and
    /// [`unverified_tracking_domain_message`]): the submitted domain is the whole
    /// diagnostic content of the refusal, so the message carries it rather than
    /// the error shape's condition list. A tracking subdomain is covered by the
    /// verified domain above it ([`Self::covers_verified_domain`]), which is how
    /// a custom open-and-click domain is set up.
    ///
    /// Only a single trailing dot is taken off — the root label of a fully
    /// qualified name, which names the same domain — and that is what is STORED.
    /// Nothing else is normalised: a padded or doubly dotted value is not a
    /// domain name and is refused as one.
    ///
    /// Shared by `CreateConfigurationSetTrackingOptions` and
    /// `UpdateConfigurationSetTrackingOptions`, which read one rule between
    /// them; each wraps the message in its own modelled error.
    pub fn tracking_domain(&self, custom_redirect_domain: Option<&str>) -> Result<String, String> {
        let given = custom_redirect_domain.unwrap_or_default();
        let domain = given.strip_suffix('.').unwrap_or(given);
        if !is_domain_name(domain) {
            return Err(invalid_tracking_domain_message(given));
        }
        if !self.covers_verified_domain(domain) {
            return Err(unverified_tracking_domain_message(given));
        }
        Ok(domain.to_string())
    }

    /// The number of emails sent in the trailing [`SEND_QUOTA_WINDOW`], as
    /// `GetSendQuota$SentLast24Hours` reports it.
    ///
    /// DERIVED from the send log on every read rather than kept as a counter, so
    /// it falls again as messages age out of the window.
    ///
    /// Every RECIPIENT counts, not every message, because that is what SES
    /// counts: "For every message that you send, the total number of recipients
    /// (including each recipient in the To:, CC: and BCC: fields) is counted
    /// against the maximum number of emails you can send in a 24-hour period
    /// (your sending quota)" (`SendEmail`'s own documentation).
    pub fn sent_last_24_hours(&self, now: SystemTime) -> f64 {
        // Counted as an integer and converted once: an `f64` sum would answer
        // `-0` for an empty log, because `-0.0` is the identity std uses for
        // floating-point `Sum`, and `-0` is not what SES reports for "nothing
        // sent".
        let recipients: usize = self
            .sent_messages
            .iter()
            .filter(|message| within_window(message.sent_at, now, SEND_QUOTA_WINDOW))
            .map(|message| message.destinations.len())
            .sum();
        recipients as f64
    }

    /// The sending activity of the trailing [`SEND_STATISTICS_WINDOW`], bucketed
    /// into the [`SEND_STATISTICS_PERIOD`] data points `GetSendStatistics`
    /// reports: `(the period's start, the deliveries attempted in that period)`,
    /// oldest period first.
    ///
    /// DERIVED from the send log on every read, like [`Self::sent_last_24_hours`],
    /// so a period's numbers never drift from the messages the log holds and the
    /// list shortens again as activity ages out of the window.
    ///
    /// Every RECIPIENT is one delivery attempt, for the reason
    /// [`Self::sent_last_24_hours`] counts recipients: SES counts each recipient
    /// of a message as an email sent. A period the log has no message in is NOT a
    /// data point — the statistics report the periods there was activity in, not
    /// one entry per period of the window.
    pub fn send_data_points(&self, now: SystemTime) -> Vec<(SystemTime, u64)> {
        // Keyed by the period's start so the buckets come out in one stable,
        // chronological order however the log is ordered.
        let mut periods: BTreeMap<u64, u64> = BTreeMap::new();
        for message in self
            .sent_messages
            .iter()
            .filter(|message| within_window(message.sent_at, now, SEND_STATISTICS_WINDOW))
        {
            let Some(period) = statistics_period_start(message.sent_at) else {
                continue;
            };
            let deliveries = periods.entry(period).or_default();
            *deliveries = deliveries.saturating_add(message.destinations.len() as u64);
        }
        periods
            .into_iter()
            .filter_map(|(period, deliveries)| {
                let start = UNIX_EPOCH.checked_add(Duration::from_secs(period))?;
                Some((start, deliveries))
            })
            .collect()
    }

    /// The verified EMAIL ADDRESS identities of this account, in one stable
    /// (lexicographic) order — the view `ListVerifiedEmailAddresses` reports.
    ///
    /// A domain identity is not an email address, so it is not in this list;
    /// `ListIdentities` is the operation that reports both classes, which is what
    /// the deprecated listing's documentation points a caller at.
    pub fn verified_email_addresses(&self) -> Vec<String> {
        self.identities
            .values()
            .filter(|identity| {
                identity.identity_type == IdentityType::EmailAddress
                    && identity.verification_status == VerificationStatus::Success
            })
            .map(|identity| identity.name.clone())
            .collect()
    }

    /// A fresh opaque token for this store, as 64 hex characters.
    ///
    /// Unique rather than merely improbable: every token digests
    /// [`Self::minted_tokens`], which this method advances and never reuses, so
    /// no two tokens from one store can collide. Opaque because the digest also
    /// covers entropy the system seeded ([`system_entropy`]) and the minting
    /// instant, so a caller cannot derive the next token from one it holds, and
    /// NOTHING from the request goes in: a token identifies what the service
    /// stored, it is never computed from what the caller sent.
    ///
    /// Hashed with the `sha2` crate rather than a hand-written digest.
    fn mint_token(&mut self) -> String {
        self.minted_tokens = self.minted_tokens.wrapping_add(1);
        let minted_at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |since_epoch| since_epoch.as_nanos());
        let mut digest = Sha256::new();
        digest.update(self.minted_tokens.to_be_bytes());
        digest.update(system_entropy().to_be_bytes());
        digest.update(minted_at.to_be_bytes());
        hex::encode(digest.finalize())
    }

    /// A fresh `MessageId` in the shape SES answers `SendEmail` with:
    /// `0100<12 hex>-<8-4-4-4-12 hex>-000000`.
    ///
    /// Server-generated (see [`Self::mint_token`]) and stored on the
    /// [`SentMessage`] it identifies: the value the operation answers with is the
    /// one it wrote.
    pub fn new_message_id(&mut self) -> String {
        let token = self.mint_token();
        format!(
            "0100{}-{}-{}-{}-{}-{}-000000",
            &token[..12],
            &token[12..20],
            &token[20..24],
            &token[24..28],
            &token[28..32],
            &token[32..44]
        )
    }

    /// A fresh Easy DKIM token, the value a domain identity publishes as the
    /// label of a `<token>._domainkey.<domain>` CNAME record:
    /// [`EASY_DKIM_TOKEN_LEN`] opaque characters. Opaque to the caller; only its
    /// stability across reads matters, which the stored [`EasyDkimSettings`]
    /// record is what provides.
    fn new_dkim_token(&mut self) -> String {
        self.mint_token()[..EASY_DKIM_TOKEN_LEN].to_string()
    }

    /// Record a configuration set named `name`, and hand back the stored record.
    ///
    /// Idempotent: a name already in the store keeps the record (and the sending
    /// state) it already carries, so the caller can tell a conflict from a
    /// create by checking the store first.
    ///
    /// `None` when the store already holds [`MAX_CONFIGURATION_SETS`] sets and
    /// this is a new one: the caller refuses the request with the modelled
    /// `LimitExceeded` rather than growing the store past the account's limit.
    pub fn add_configuration_set(&mut self, name: &str) -> Option<&ConfigurationSet> {
        if !self.configuration_sets.contains_key(name)
            && self.configuration_sets.len() >= MAX_CONFIGURATION_SETS
        {
            return None;
        }
        Some(
            self.configuration_sets
                .entry(name.to_string())
                .or_insert_with(|| ConfigurationSet {
                    name: name.to_string(),
                    // A set SES has just created is not paused: `SendEmail`
                    // refuses a send naming it only after
                    // `UpdateConfigurationSetSendingEnabled` disables it.
                    sending_enabled: true,
                    // And it publishes no reputation metrics:
                    // `ReputationOptions$ReputationMetricsEnabled` documents
                    // "The default value is false", so a set starts off until
                    // `UpdateConfigurationSetReputationMetricsEnabled` enables
                    // it.
                    reputation_metrics_enabled: false,
                    // And it carries no `TrackingOptions` sub-resource:
                    // `CreateConfigurationSetTrackingOptions` is what adds one.
                    custom_redirect_domain: None,
                    // And no `DeliveryOptions` sub-resource either:
                    // `PutConfigurationSetDeliveryOptions` is what writes one.
                    tls_policy: None,
                    // A set whose sending has never been disabled and later
                    // re-enabled has had no fresh start.
                    last_fresh_start: None,
                    // And it publishes to no event destination:
                    // `CreateConfigurationSetEventDestination` is what adds one.
                    event_destinations: Vec::new(),
                    // And the account's configuration-set registry does not carry it yet:
                    // the operation that recorded it publishes it
                    // ([`publish_configuration_set`]), and a record written straight into
                    // this store reaches the registry on the next reconciliation.
                    registered: false,
                }),
        )
    }

    /// Record a receipt rule set named `name` holding `rules`, and hand back the
    /// stored record.
    ///
    /// The caller has already decided that `name` is free — a name already in
    /// the store is a naming conflict, which every creating operation answers
    /// with its own modelled `AlreadyExists` — so this only applies the
    /// account's own ceiling: `None` when the store already holds
    /// [`MAX_RECEIPT_RULE_SETS`] sets, which the caller refuses with the
    /// modelled `LimitExceeded` rather than growing the store past the limit.
    ///
    /// `created_at` is taken here, so the timestamp a reader reports is the one
    /// this write stored.
    pub fn add_receipt_rule_set(
        &mut self,
        name: &str,
        rules: Vec<crate::ops::types::ReceiptRule>,
    ) -> Option<&ReceiptRuleSet> {
        if !self.receipt_rule_sets.contains_key(name)
            && self.receipt_rule_sets.len() >= MAX_RECEIPT_RULE_SETS
        {
            return None;
        }
        Some(
            self.receipt_rule_sets
                .entry(name.to_string())
                .or_insert_with(|| ReceiptRuleSet {
                    name: name.to_string(),
                    created_at: SystemTime::now(),
                    rules,
                }),
        )
    }

    /// The receipt rule set that is currently active, and `None` while no set
    /// is active — the two cases a reader of the active set reports.
    ///
    /// Resolved through [`Self::receipt_rule_sets`] on every call rather than
    /// stored alongside the name, so a rule added to or removed from the active
    /// set after it was made active is part of what this hands back.
    pub fn active_rule_set(&self) -> Option<&ReceiptRuleSet> {
        self.active_receipt_rule_set
            .as_deref()
            .and_then(|name| self.receipt_rule_sets.get(name))
    }

    /// Record the receipt IP address filter `filter` under its own name.
    ///
    /// The caller has already decided that the name is free — a name already in
    /// the store is a naming conflict, which the creating operation answers with
    /// its modelled `AlreadyExists` — so this only applies the account's own
    /// ceiling: `false`, having stored nothing, when the store already holds
    /// [`MAX_RECEIPT_FILTERS`] filters, which the caller refuses with the
    /// modelled `LimitExceeded` rather than growing the store past the limit.
    pub fn add_receipt_filter(&mut self, filter: crate::ops::types::ReceiptFilter) -> bool {
        // A name the store already holds would be replaced rather than added, so
        // it does not grow the store and the ceiling does not apply to it.
        let held = self.receipt_filters.contains_key(&filter.name);
        if !held && self.receipt_filters.len() >= MAX_RECEIPT_FILTERS {
            return false;
        }
        self.receipt_filters.insert(filter.name.clone(), filter);
        true
    }
}

/// Whether a message accepted at `sent_at` is still inside the trailing `window`
/// at `now`.
///
/// The one rule every window in this store is read by — the sending quota's and
/// the sending statistics' — so a message is in or out of a window in exactly
/// one place. A message whose window end cannot be represented is still inside
/// it: it cannot have left a window that does not end.
fn within_window(sent_at: SystemTime, now: SystemTime, window: Duration) -> bool {
    sent_at
        .checked_add(window)
        .is_none_or(|leaves_window| leaves_window > now)
}

/// The start of the [`SEND_STATISTICS_PERIOD`] bucket `at` falls in, as whole
/// seconds since the epoch; `None` for an instant before the epoch, which no
/// accepted message carries.
///
/// The buckets are aligned to the epoch rather than to the time of the call, so
/// one message always falls in the same period however often the statistics are
/// read.
fn statistics_period_start(at: SystemTime) -> Option<u64> {
    let since_epoch = at.duration_since(UNIX_EPOCH).ok()?.as_secs();
    let period = SEND_STATISTICS_PERIOD.as_secs();
    Some(since_epoch - since_epoch % period)
}

/// Whether `value` is a receipt rule set name in the form SES documents.
///
/// The model states the rule on every `ReceiptRuleSetName` member (e.g.
/// `CreateReceiptRuleSetRequest$RuleSetName`): the name must "Contain only ASCII
/// letters (a-z, A-Z), numbers (0-9), underscores (_), or dashes (-)", "Start
/// and end with a letter or number", and "Contain 64 characters or fewer"
/// ([`MAX_RECEIPT_RULE_SET_NAME`]).
pub fn is_receipt_rule_set_name(value: &str) -> bool {
    is_receipt_name(value, false)
}

/// Whether `value` is a receipt RULE name in the form SES documents.
///
/// `ReceiptRule$Name` states the same three clauses a rule set name carries with
/// one more character admitted: the name must "Contain only ASCII letters (a-z,
/// A-Z), numbers (0-9), underscores (_), dashes (-), or periods (.)", "Start and
/// end with a letter or number", and "Contain 64 characters or fewer"
/// ([`MAX_RECEIPT_RULE_NAME`]). So the two names are read through ONE reading
/// ([`is_receipt_name`]) that differs only in whether the period is admitted,
/// rather than two copies of the same three clauses.
pub fn is_receipt_rule_name(value: &str) -> bool {
    is_receipt_name(value, true)
}

/// The reading both receipt names share: 7-bit ASCII letters, numbers,
/// underscores and dashes — and periods when `periods_admitted`, as a rule's own
/// name admits them — a letter or number at each end, and at most
/// [`MAX_RECEIPT_RULE_SET_NAME`] characters. The empty string ends with no letter
/// or number, so it is neither name.
fn is_receipt_name(value: &str, periods_admitted: bool) -> bool {
    let mut characters = value.chars();
    let Some(first) = characters.next() else {
        return false;
    };
    let last = characters.next_back().unwrap_or(first);
    value.len() <= MAX_RECEIPT_RULE_SET_NAME
        && value.chars().all(|character| {
            character.is_ascii_alphanumeric()
                || matches!(character, '_' | '-')
                || (periods_admitted && character == '.')
        })
        && first.is_ascii_alphanumeric()
        && last.is_ascii_alphanumeric()
}

/// Whether `value` is a receipt IP address filter name in the form SES
/// documents.
///
/// `ReceiptFilter$Name` states the same three clauses the receipt rule set name
/// carries — ASCII letters, numbers, underscores or dashes only, a letter or
/// number at each end, and [`MAX_RECEIPT_FILTER_NAME`] characters or fewer — so
/// the value is read through [`is_receipt_rule_set_name`] instead of repeating
/// them.
pub fn is_receipt_filter_name(value: &str) -> bool {
    is_receipt_rule_set_name(value)
}

/// The IPv4 address or range `value` names, or `None` when it is neither.
///
/// `ReceiptIpFilter$Cidr` documents the form: "A single IP address or a range of
/// IP addresses to block or allow, specified in Classless Inter-Domain Routing
/// (CIDR) notation. An example of a single email address is 10.0.0.1. An example
/// of a range of IP addresses is 10.0.0.1/24." A bare address is the range that
/// holds only it, so it reads as a `/32`; a range carries its own prefix length,
/// which `ipnet` accepts only within the address family's width. Both examples
/// the member gives are IPv4, and an IPv4 address is the only form it documents,
/// so an address of another family is one of the values this reads as neither.
pub fn parse_receipt_filter_cidr(value: &str) -> Option<::ipnet::Ipv4Net> {
    if let Ok(address) = value.parse::<std::net::Ipv4Addr>() {
        return Some(::ipnet::Ipv4Net::from(address));
    }
    value.parse::<::ipnet::Ipv4Net>().ok()
}

/// The listing name stamped into a `ListTemplates` page token, so a cursor cut
/// from the template list cannot resume another listing of this service.
pub const LISTING_TEMPLATES: &str = "ListTemplates";

/// The listing name stamped into a `ListReceiptRuleSets` page token, for the same
/// reason: a cursor cut from the receipt rule set list resumes only that listing.
pub const LISTING_RECEIPT_RULE_SETS: &str = "ListReceiptRuleSets";

/// The listing name stamped into a `ListConfigurationSets` page token, for the
/// same reason: a cursor cut from the configuration set list resumes only that
/// listing.
pub const LISTING_CONFIGURATION_SETS: &str = "ListConfigurationSets";

/// The listing name stamped into a `ListIdentities` page token, for the same
/// reason: a cursor cut from the identity list resumes only that listing.
pub const LISTING_IDENTITIES: &str = "ListIdentities";

/// The listing name stamped into a `ListCustomVerificationEmailTemplates` page
/// token, for the same reason: a cursor cut from the custom verification email
/// template list resumes only that listing.
pub const LISTING_CUSTOM_VERIFICATION_EMAIL_TEMPLATES: &str =
    "ListCustomVerificationEmailTemplates";

/// The bytes every page token of this service digests before the listing and the
/// key, so a token of this service is not a token of any other.
const PAGE_TOKEN_DOMAIN: &[u8] = b"com.amazonaws.ses#page-token\0";

/// The leading bytes of the digest a page token carries as its integrity tag.
/// Eight bytes is enough that a value the caller made up does not carry a
/// matching tag, while keeping the token short.
const PAGE_TOKEN_TAG_LEN: usize = 8;

/// The integrity tag of `key` for `listing`: the leading [`PAGE_TOKEN_TAG_LEN`]
/// bytes of SHA-256 over this service's domain separator, the listing, and the
/// key. Hashed with the `sha2` crate rather than a hand-written digest.
fn page_token_tag(listing: &str, key: &str) -> Vec<u8> {
    let mut digest = Sha256::new();
    digest.update(PAGE_TOKEN_DOMAIN);
    digest.update(listing.as_bytes());
    digest.update(b"\0");
    digest.update(key.as_bytes());
    digest.finalize()[..PAGE_TOKEN_TAG_LEN].to_vec()
}

/// A page token naming the `key` the NEXT page of `listing` starts at: the
/// integrity tag followed by the key, hex-encoded.
///
/// Opaque — a caller cannot read a position out of it and hands it back verbatim
/// — and self-verifying, so a token this listing never issued is refused by
/// [`decode_page_token`] instead of being read as an arbitrary position in it.
pub fn encode_page_token(listing: &str, key: &str) -> String {
    let mut bytes = page_token_tag(listing, key);
    bytes.extend_from_slice(key.as_bytes());
    ::hex::encode(bytes)
}

/// The key a token of `listing` resumes at, or `None` when `token` is not one
/// this service issued for that listing: not hex, too short to carry a tag, not
/// UTF-8, carrying no key, or carrying a tag that does not match the listing and
/// position it claims. The caller refuses such a token rather than restarting
/// the listing at some other position.
pub fn decode_page_token(listing: &str, token: &str) -> Option<String> {
    let bytes = ::hex::decode(token).ok()?;
    if bytes.len() <= PAGE_TOKEN_TAG_LEN {
        return None;
    }
    let (tag, key) = bytes.split_at(PAGE_TOKEN_TAG_LEN);
    let key = String::from_utf8(key.to_vec()).ok()?;
    (page_token_tag(listing, &key) == tag).then_some(key)
}

/// The addr-spec of a `Source`/recipient value, which may arrive either bare
/// (`user@example.com`) or with a display name (`"Me" <user@example.com>`).
///
/// A value with no angle-addr is returned trimmed and unchanged, so a malformed
/// value stays malformed for the caller's own check.
///
/// The reading itself is `service_common::ses_identities`', which the account's
/// identity registry keys by, so this surface and the registry cannot disagree about
/// which mailbox a value names.
pub fn addr_spec(value: &str) -> &str {
    ::service_common::ses_identities::addr_spec(value)
}

/// The identity an ARN of this service names, when it names one: the ARN's
/// `identity` resource, read down to the identity's own name and keyed by its
/// addr-spec. `None` for a value that is not an ARN of this service, names
/// another resource of it, or names no identity at all.
///
/// The ONE reading of the identity-ARN form in this service, so
/// [`identity_key`] and [`resolve_identity`] cannot disagree about what an
/// identity ARN names; the ARN itself is read by `service_common::arn`, the one
/// caller-supplied-ARN parse in this tree.
fn identity_named_by_arn(value: &str) -> Option<&str> {
    ::service_common::arn::is_resource_arn_of(value, SES_ARN_VENDOR)
        .then(|| ::service_common::arn::resource_id_of_type(value, IDENTITY_RESOURCE_TYPE))
        .flatten()
        .filter(|name| !name.is_empty())
        .map(addr_spec)
}

/// The key an identity is stored under, for a caller value that may name the
/// identity either way the policy operations document.
///
/// Their `Identity` member states both forms: "You can specify an identity by
/// using its name or by using its Amazon Resource Name (ARN). Examples:
/// user@example.com, example.com,
/// arn:aws:ses:us-east-1:123456789012:identity/example.com." So an ARN of this
/// service naming an `identity` resource is read down to the identity it names
/// ([`identity_named_by_arn`]), and anything else is the identity itself; either
/// way the value is keyed by its addr-spec, the way the writers key it
/// ([`addr_spec`]).
///
/// The DOMAIN part is compared the way DNS compares a name, so `Example.COM` and
/// `example.com` name ONE identity of this account: the keying is the account identity
/// registry's own (`service_common::ses_identities::identity_key`), so a record of this
/// surface and the account's record of the same identity cannot be reached under two
/// different keys. The spelling a request used is kept on the record itself
/// ([`Identity::name`]), which is the value `ListIdentities` and every per-identity
/// report answer with.
pub fn identity_key(value: &str) -> String {
    ::service_common::ses_identities::identity_key(named_identity(value))
}

/// The identity a request's value NAMES, in the SPELLING the request used: the
/// value itself for a plain identity, and the identity an identity ARN addresses
/// for an ARN ([`identity_named_by_arn`]).
///
/// What a message about the request reports, as against [`identity_key`], which is
/// what the identity list is keyed by: a refusal names the identity the caller
/// asked about, so its variable part comes from the request rather than from the
/// normalisation the lookup applies.
pub fn named_identity(value: &str) -> &str {
    let trimmed = value.trim();
    identity_named_by_arn(trimmed).unwrap_or_else(|| addr_spec(trimmed))
}

/// The vendor field of this service's own ARNs, which is its signing name.
const SES_ARN_VENDOR: &str = "ses";

/// The resource type an SES identity ARN addresses
/// (`arn:aws:ses:<region>:<account>:identity/<name>`).
const IDENTITY_RESOURCE_TYPE: &str = "identity";

/// The domain itself and every parent domain of it, longest first
/// (`mail.corp.example.com` -> `mail.corp.example.com`, `corp.example.com`,
/// `example.com`), stopping before the bare top-level label.
fn domain_suffixes(domain: &str) -> impl Iterator<Item = &str> {
    let labels = domain.match_indices('.').map(|(at, _)| at + 1);
    std::iter::once(0)
        .chain(labels)
        .filter(move |start| domain[*start..].contains('.'))
        .map(move |start| &domain[start..])
}

/// Every identity KEY of this account that would cover a sender, given that
/// sender's own key: the key itself, then each domain suffix of its domain,
/// longest first ([`domain_suffixes`]).
///
/// `key` is already an identity key ([`identity_key`]), so every name walked from
/// it is one too — which is what lets a caller key its value ONCE and compare a
/// domain the way DNS compares one at every step of the walk, rather than keying
/// each candidate name again.
///
/// The ONE walk both readings of a sender use —
/// [`ComAmazonawsSesSimpleemailserviceState::is_verified_sender`], which asks
/// whether each name is a verified identity, and
/// [`ComAmazonawsSesSimpleemailserviceState::holds_sending_identity`], which
/// asks only whether the list holds it — so the two cannot drift apart over
/// which names cover an address.
fn sender_identity_keys(key: &str) -> impl Iterator<Item = &str> {
    let domain = key.split_once('@').map(|(_, domain)| domain);
    std::iter::once(key).chain(domain.into_iter().flat_map(domain_suffixes))
}

/// Whether `value` is an email address in the form SES requires.
///
/// `SendEmail` states the form — "not in the format
/// UserName@[SubDomain.]Domain.TopLevelDomain" is what makes an address invalid
/// — and `SendEmail$Source` states the character set: "Amazon SES does not
/// support the SMTPUTF8 extension, as described in RFC6531. For this reason, the
/// email address string must be 7-bit ASCII."
///
/// That is the form every service whose model documents a member as an email
/// address reads, so it is read through the ONE shared reading of one
/// (`service_common::email_address`) rather than stated again here: a dot-atom
/// local part of 7-bit-ASCII atom characters — so a leading, trailing or doubled
/// dot, which would make an EMPTY atom, is not an address — an `@`, and a domain
/// name ([`is_domain_name`]).
pub fn is_email_address(value: &str) -> bool {
    ::service_common::email_address::is_email_address(value)
}

/// The vendor field of an Amazon SNS ARN (`aws.api#service$arnNamespace`): the
/// third field of a topic ARN.
const SNS_ARN_VENDOR: &str = "sns";

/// The partition every ARN this emulator mints and serves carries
/// (`engine::emulator_arn` writes `arn:aws:…`), so an ARN of another partition
/// names a resource in a partition this emulator does not serve.
const ARN_PARTITION: &str = "aws";

/// The vendor field of an Amazon Web Services Lambda ARN
/// (`aws.api#service$arnNamespace`): the third field of a function ARN.
const LAMBDA_ARN_VENDOR: &str = "lambda";

/// The vendor field of an Amazon Web Services KMS ARN: the third field of a key
/// or alias ARN.
const KMS_ARN_VENDOR: &str = "kms";

/// The resource type a KMS customer managed key's ARN addresses
/// (`arn:aws:kms:<region>:<account>:key/<id>`).
const KMS_KEY_RESOURCE_TYPE: &str = "key";

/// The resource type a KMS alias ARN addresses, which is the form
/// `S3Action$KmsKeyArn` documents for the managed key
/// (`arn:aws:kms:<region>:<account>:alias/aws/ses`).
const KMS_ALIAS_RESOURCE_TYPE: &str = "alias";

/// Whether `value` is the ARN of an Amazon SNS topic, as a receipt action's
/// `TopicArn` member documents one ("the Amazon Resource Name (ARN) of the Amazon
/// SNS topic").
///
/// The published form of a topic ARN, read exactly as
/// [`resolve_sns_topic`] reads it: the vendor is Amazon SNS's, and the resource
/// field is the topic NAME alone ([`is_sns_topic_name`]), since a topic ARN names
/// no subresource while a subscription ARN carries a further `:<subscription-id>`
/// segment. What is deliberately NOT read here is the clause that reading adds —
/// that the topic belongs to this account and Region — because a receipt action's
/// topic is one SES publishes to on the account's behalf, and whether SES may
/// publish to it is Amazon SNS's own state and permissions, which this service
/// does not hold.
fn is_sns_topic_arn(value: &str) -> bool {
    ::service_common::arn::parse_of(value, SNS_ARN_VENDOR)
        .is_some_and(|arn| is_sns_topic_name(arn.resource))
}

/// What `LambdaAction$FunctionArn` has to name: the member documents the form,
/// "An example of an Amazon Web Services Lambda function ARN is
/// arn:aws:lambda:us-west-2:account-id:function:MyFunction", so the resource type
/// is `function` and an ARN of another Lambda resource names no function to call.
const LAMBDA_FUNCTION_ARN: ::service_common::arn::Requirement = ::service_common::arn::Requirement {
    vendor: LAMBDA_ARN_VENDOR,
    resource_type: Some("function"),
    label: "an Amazon Web Services Lambda function",
};

/// Whether `value` is the ARN of an Amazon Web Services KMS key SES could encrypt
/// with.
///
/// `S3Action$KmsKeyArn` documents both forms a caller may give: "To use the Amazon
/// Web Services managed key, provide an ARN in the form of
/// arn:aws:kms:REGION:ACCOUNT-ID-WITHOUT-HYPHENS:alias/aws/ses", and "To use a
/// customer managed key that you created in Amazon Web Services KMS, provide the
/// ARN of the customer managed key" — an ARN of the `key` resource. So both
/// resource types name a key, and an ARN of another vendor or another KMS
/// resource names none. Read on the ARN alone: whether the key EXISTS is Amazon
/// Web Services KMS's own state, which this service does not hold.
fn is_kms_key_arn(value: &str) -> bool {
    ::service_common::arn::parse_of(value, KMS_ARN_VENDOR).is_some_and(|arn| {
        arn.resource_type() == KMS_KEY_RESOURCE_TYPE
            || arn.resource_type() == KMS_ALIAS_RESOURCE_TYPE
    })
}

/// Why one action of a receipt rule is an action SES could never take, keyed by
/// the refusal it is reported as.
///
/// The three modelled refusals a receipt action's members carry — one per
/// downstream service a rule's action reaches — plus the one the `ReceiptAction`
/// shape itself states and the model declares no error for. Each carries the
/// message, naming the member and its value, that the operation refusing the rule
/// renders.
#[derive(Debug, Clone)]
pub enum ReceiptActionDefect {
    /// The entry does not carry exactly one action: `ReceiptAction` documents
    /// that "An instance of this data type can represent only one action", so an
    /// entry setting none represents nothing and one setting several represents
    /// no single action either. The model expresses neither, so the operation
    /// refuses it through its own 400 channel.
    NotOneAction { message: String },
    /// A member whose own documentation states the form its value takes carries
    /// one outside it. The model declares no pattern for such a member and no
    /// error for the violation, so the operation refuses it through its own 400
    /// channel, exactly as it refuses an entry that is not one action.
    InvalidParameter { message: String },
    /// A `TopicArn` member names no Amazon SNS topic to publish to
    /// (`InvalidSnsTopic`), carrying the value it named as the topic.
    SnsTopic { topic: String, message: String },
    /// An `S3Action` names no bucket to write to, or no KMS key to encrypt with
    /// (`InvalidS3Configuration`), carrying the action's own bucket.
    S3Configuration { bucket: String, message: String },
    /// A `LambdaAction` names no Lambda function to call
    /// (`InvalidLambdaFunction`), carrying the value it named as the function.
    LambdaFunction { function_arn: String, message: String },
}

/// The defect of the FIRST action of `actions` SES could never take, and `None`
/// when every one of them is an action it could.
///
/// The ONE reading of a receipt rule's actions, so every operation that stores a
/// rule holds its actions to the same requirements instead of each keeping its
/// own copy of them. What is decided is the form of the values the action's
/// members name: a member the model documents as the ARN of a resource of another
/// service must name a resource of THAT service ([`is_sns_topic_arn`],
/// [`LAMBDA_FUNCTION_ARN`], [`is_kms_key_arn`]), an `S3Action` must name a bucket,
/// and a member documenting the charset its value takes must be inside it
/// ([`is_header_name`], [`is_header_value`]). Whether the resources those values
/// name EXIST, and whether SES may act on them, is the other service's own state
/// and permissions, read against that service's own store instead
/// ([`receipt_destinations::unauthorized_destination`]) — so a well-formed value is
/// accepted here.
pub fn receipt_actions_defect(
    actions: &[crate::ops::types::ReceiptAction],
) -> Option<ReceiptActionDefect> {
    actions
        .iter()
        .enumerate()
        // The awsQuery member path a caller wrote the action at, which is
        // 1-based, so a refusal names the member the request itself carried.
        .find_map(|(index, action)| receipt_action_defect(index + 1, action))
}

/// The defect of ONE action, written at `position` of the rule's action list.
fn receipt_action_defect(
    position: usize,
    action: &crate::ops::types::ReceiptAction,
) -> Option<ReceiptActionDefect> {
    let member = |name: &str| format!("Actions.member.{position}.{name}");
    let set = [
        action.s3_action.is_some(),
        action.bounce_action.is_some(),
        action.workmail_action.is_some(),
        action.lambda_action.is_some(),
        action.stop_action.is_some(),
        action.add_header_action.is_some(),
        action.sns_action.is_some(),
        action.connect_action.is_some(),
    ]
    .into_iter()
    .filter(|present| *present)
    .count();
    if set != 1 {
        return Some(ReceiptActionDefect::NotOneAction {
            message: format!("Actions.member.{position} must set exactly one action, got {set}"),
        });
    }

    if let Some(s3_action) = &action.s3_action {
        if s3_action.bucket_name.trim().is_empty() {
            return Some(ReceiptActionDefect::S3Configuration {
                bucket: s3_action.bucket_name.clone(),
                message: format!(
                    "{} must name an Amazon S3 bucket",
                    member("S3Action.BucketName")
                ),
            });
        }
        if let Some(key) = &s3_action.kms_key_arn {
            if !is_kms_key_arn(key) {
                return Some(ReceiptActionDefect::S3Configuration {
                    bucket: s3_action.bucket_name.clone(),
                    message: format!(
                        "{} {key:?} must be the ARN of an Amazon Web Services KMS key",
                        member("S3Action.KmsKeyArn")
                    ),
                });
            }
        }
    }
    if let Some(lambda_action) = &action.lambda_action {
        if !LAMBDA_FUNCTION_ARN.admits(&lambda_action.function_arn) {
            return Some(ReceiptActionDefect::LambdaFunction {
                function_arn: lambda_action.function_arn.clone(),
                message: format!(
                    "{} {:?} {}",
                    member("LambdaAction.FunctionArn"),
                    lambda_action.function_arn,
                    LAMBDA_FUNCTION_ARN.phrase()
                ),
            });
        }
    }
    if let Some(add_header_action) = &action.add_header_action {
        if !is_header_name(&add_header_action.header_name) {
            return Some(ReceiptActionDefect::InvalidParameter {
                message: invalid_header_name_message(&add_header_action.header_name),
            });
        }
        if !is_header_value(&add_header_action.header_value) {
            return Some(ReceiptActionDefect::InvalidParameter {
                message: invalid_header_value_message(&add_header_action.header_value),
            });
        }
    }
    // Every action may publish a notification to an Amazon SNS topic, and the
    // `SNSAction` is nothing but that topic, so the one topic reading covers
    // them all.
    topic_arn_members(action)
        .into_iter()
        .find(|(_, topic_arn)| !is_sns_topic_arn(topic_arn))
        .map(|(action_name, topic_arn)| ReceiptActionDefect::SnsTopic {
            topic: topic_arn.to_string(),
            message: format!(
                "{} {topic_arn:?} must be the ARN of an Amazon SNS topic",
                member(&format!("{action_name}.TopicArn"))
            ),
        })
}

/// Every Amazon SNS topic ONE receipt action notifies, as `(the member's own action, the
/// topic ARN it carries)` in the order the action's members declare them.
///
/// The ONE listing, because two readings ask about the same set: whether each value names a
/// topic at all ([`receipt_action_defect`]) and whether Amazon SES may publish to the topic it
/// names ([`receipt_destinations`]). Every action may publish a notification to a topic, and
/// an `SNSAction` is nothing but that topic.
pub fn topic_arn_members(
    action: &crate::ops::types::ReceiptAction,
) -> Vec<(&'static str, &str)> {
    [
        (
            "S3Action",
            action
                .s3_action
                .as_ref()
                .and_then(|set| set.topic_arn.as_deref()),
        ),
        (
            "BounceAction",
            action
                .bounce_action
                .as_ref()
                .and_then(|set| set.topic_arn.as_deref()),
        ),
        (
            "WorkmailAction",
            action
                .workmail_action
                .as_ref()
                .and_then(|set| set.topic_arn.as_deref()),
        ),
        (
            "LambdaAction",
            action
                .lambda_action
                .as_ref()
                .and_then(|set| set.topic_arn.as_deref()),
        ),
        (
            "StopAction",
            action
                .stop_action
                .as_ref()
                .and_then(|set| set.topic_arn.as_deref()),
        ),
        (
            "SNSAction",
            action
                .sns_action
                .as_ref()
                .map(|set| set.topic_arn.as_str()),
        ),
    ]
    .into_iter()
    .filter_map(|(action_name, topic_arn)| topic_arn.map(|arn| (action_name, arn)))
    .collect()
}

/// The longest `AddHeaderAction$HeaderName` SES accepts: the member documents the bound —
/// "The name must contain at least one character, and can contain up to 50 characters."
pub const MAX_HEADER_NAME: usize = 50;

/// The longest `AddHeaderAction$HeaderValue` SES accepts: "This value can contain up to 2048
/// characters."
pub const MAX_HEADER_VALUE: usize = 2048;

/// Whether `value` is a header name `AddHeaderAction$HeaderName` admits.
///
/// The member states the charset and the bounds and the model declares no pattern for them:
/// "The name must contain at least one character, and can contain up to 50 characters. It
/// consists of alphanumeric (a–z, A–Z, 0–9) characters and dashes."
#[must_use]
pub fn is_header_name(value: &str) -> bool {
    !value.is_empty()
        && value.chars().count() <= MAX_HEADER_NAME
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
}

/// Whether `value` is a header value `AddHeaderAction$HeaderValue` admits: "This value can
/// contain up to 2048 characters. It can't contain newline (\n) or carriage return (\r)
/// characters."
#[must_use]
pub fn is_header_value(value: &str) -> bool {
    value.chars().count() <= MAX_HEADER_VALUE && !value.contains(['\n', '\r'])
}

/// How SES words the refusal of a header name outside the charset
/// `AddHeaderAction$HeaderName` documents: the fixed sentence plus the name the request
/// carried.
#[must_use]
pub fn invalid_header_name_message(header_name: &str) -> String {
    format!("Invalid header name: {header_name}")
}

/// How SES words the refusal of a header value outside the form
/// `AddHeaderAction$HeaderValue` documents — the sibling member's refusal, worded the same
/// way over the value the request carried.
#[must_use]
pub fn invalid_header_value_message(header_value: &str) -> String {
    format!("Invalid header value: {header_value}")
}

/// The `TlsPolicy` a receipt rule the request gave none for holds: "Specifies whether Amazon
/// SES should require that incoming email is delivered over a connection encrypted with
/// Transport Layer Security (TLS) … The default is `Optional`" (`ReceiptRule$TlsPolicy`).
pub const DEFAULT_RECEIPT_RULE_TLS_POLICY: &str = "Optional";

/// The `Encoding` an `SNSAction` the request gave none for holds: "The default value is
/// UTF-8" (`SNSAction$Encoding`).
pub const DEFAULT_SNS_ACTION_ENCODING: &str = "UTF-8";

/// The `InvocationType` a `LambdaAction` the request gave none for holds: "The default value
/// is `Event`" (`LambdaAction$InvocationType`).
pub const DEFAULT_LAMBDA_ACTION_INVOCATION_TYPE: &str = "Event";

/// Fill in every member of `rule` whose own documentation states the value it holds when the
/// request omits it, so the stored rule carries it and every read of the rule reports it.
///
/// A member documented with a default is NOT optional in the resource — it is optional in the
/// REQUEST — so the default is materialised where the rule is stored rather than invented by
/// each reader: `DescribeReceiptRule`, `DescribeReceiptRuleSet` and
/// `DescribeActiveReceiptRuleSet` all project the stored rule, and a caller that reads a rule
/// and writes it back must not lose a value the service supplied. The modelled booleans
/// (`ReceiptRule$Enabled`, `ReceiptRule$ScanEnabled`, both `@default false`) carry their
/// default in the shape itself, so there is nothing to fill in for them.
///
/// Applied by every operation that STORES a rule, so a created and an updated rule read back
/// alike ([`crate::ops::types::ReceiptRule`] is what both store).
pub fn materialize_receipt_rule_defaults(rule: &mut crate::ops::types::ReceiptRule) {
    if rule.tls_policy.is_none() {
        rule.tls_policy = Some(DEFAULT_RECEIPT_RULE_TLS_POLICY.to_string());
    }
    for action in rule.actions.iter_mut().flatten() {
        if let Some(sns_action) = &mut action.sns_action {
            if sns_action.encoding.is_none() {
                sns_action.encoding = Some(DEFAULT_SNS_ACTION_ENCODING.to_string());
            }
        }
        if let Some(lambda_action) = &mut action.lambda_action {
            if lambda_action.invocation_type.is_none() {
                lambda_action.invocation_type =
                    Some(DEFAULT_LAMBDA_ACTION_INVOCATION_TYPE.to_string());
            }
        }
    }
}

/// The Region an operation serves a request that carries no SigV4 credential
/// scope in, which is the Region AWS's own `email` endpoint resolves to without
/// one.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The longest Region LABEL this service reads: every `*Arn` pattern in the AWS
/// models bounds the Region field at `[a-z0-9-]{1,20}`, so longer text is no
/// Region any AWS endpoint has.
const MAX_REGION_LENGTH: usize = 20;

/// Whether `value` is a Region LABEL, under the grammar those patterns state.
///
/// Read on the way IN ([`region_from_request`]) so the Region every later
/// comparison and message uses is bounded, whatever a caller's credential scope
/// carries.
fn is_region_label(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_REGION_LENGTH
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

/// The Region the caller's request is scoped to: the Region of its SigV4
/// credential scope, or [`DEFAULT_REGION`] when it carries none — or carries text
/// that is no Region label ([`is_region_label`]).
///
/// The scope is parsed by `emulator_server::sigv4_credential_scope`, the single
/// source of SigV4-scope parsing, so a header that is not a well-formed SigV4
/// `Authorization` yields the default rather than a bogus Region.
pub fn region_from_request(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .filter(|region| is_region_label(region))
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The identities ONE call may ask about, shared by the operations that report
/// per-identity attributes: each is documented as being able to "only get …
/// attributes for up to 100 identities at a time"
/// (`GetIdentityNotificationAttributes`, `GetIdentityVerificationAttributes` and
/// `GetIdentityMailFromDomainAttributes`). The model carries no `@length` for
/// any of those lists, so the boundary validator passes a longer one and the
/// operation refuses it itself.
pub const MAX_IDENTITIES_PER_CALL: usize = 100;

/// The characters an Amazon SNS topic name is made of, and the length it may
/// reach: "Topic names must be made up of only uppercase and lowercase ASCII
/// letters, numbers, underscores, and hyphens, and must be between 1 and 256
/// characters long" (the Amazon SNS documentation of a topic name, which is the
/// resource field of a topic ARN).
const MAX_TOPIC_NAME_LENGTH: usize = 256;

/// Whether `value` is an Amazon SNS topic NAME, as Amazon SNS documents one (see
/// [`MAX_TOPIC_NAME_LENGTH`]).
fn is_sns_topic_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_TOPIC_NAME_LENGTH
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
}

/// Whether the ARN `value`, of `vendor`, names a resource of the one account this
/// emulator serves, in the partition it serves and in the Region the caller's
/// request is scoped to.
///
/// An ARN names ONE account's resource in ONE Region, and this emulator serves a
/// single account ([`::engine::EMULATOR_ACCOUNT_ID`]) — so an ARN of another
/// account, partition or Region names nothing it holds. The six fields come from
/// ONE `arn::parse_of`; the Region needs no second reading, because the Region it
/// is compared against is already a label ([`region_from_request`]).
fn names_a_resource_here(value: &str, vendor: &str, region: &str) -> bool {
    ::service_common::arn::parse_of(value, vendor).is_some_and(|arn| {
        arn.partition == ARN_PARTITION
            && arn.account == ::engine::EMULATOR_ACCOUNT_ID
            && arn.region == region
    })
}

/// The Amazon SNS topic ARN a request names, or the reason the value names no
/// topic SES could publish to for this caller.
///
/// What a member the model documents as "the Amazon Resource Name (ARN) of the
/// Amazon SNS topic" has to carry. The six fields come from
/// `service_common::arn`, the one caller-supplied-ARN parse in this tree; what is
/// decided here is the rest of the published form of a topic ARN,
/// `arn:<partition>:sns:<region>:<account-id>:<topic-name>` — the vendor is
/// Amazon SNS's, the resource field is the topic NAME alone
/// ([`is_sns_topic_name`]), since a topic ARN names no subresource while a
/// subscription ARN carries a further `:<subscription-id>` segment — and that the
/// topic is one this caller could have: in its own account, partition and Region
/// ([`names_a_resource_here`]), as a notification topic of an identity is.
///
/// Read on the ARN alone: whether the topic EXISTS is Amazon SNS's own state,
/// which this service does not hold.
pub fn resolve_sns_topic<'value>(value: &'value str, region: &str) -> Result<&'value str, String> {
    let trimmed = value.trim();
    let names_a_topic = names_a_resource_here(trimmed, SNS_ARN_VENDOR, region)
        && ::service_common::arn::resource(trimmed).is_some_and(is_sns_topic_name);
    if names_a_topic {
        Ok(trimmed)
    } else {
        Err(format!(
            "Invalid SnsTopic {trimmed:?}: SnsTopic must be the ARN of an Amazon SNS topic of \
             account {} in {region}",
            ::engine::EMULATOR_ACCOUNT_ID
        ))
    }
}

/// Why an `EventDestination` a request supplies is not one Amazon SES could
/// publish email sending events to, and so WHICH refusal reports it.
///
/// `CreateConfigurationSetEventDestination` and
/// `UpdateConfigurationSetEventDestination` model one error per destination
/// class — `InvalidCloudWatchDestinationException`,
/// `InvalidFirehoseDestinationException`, `InvalidSNSDestinationException`, each
/// documented as "See the error message for details" — and none for the
/// destination-independent half of the rule (the name's form, which event types
/// it matches, and that exactly one destination is provided), which is a
/// parameter violation. Each variant carries the sentence its refusal reports.
#[derive(Debug, Clone)]
pub enum EventDestinationFault {
    /// A member of the event destination itself carries a value the operation
    /// cannot act on, which no modelled error of it covers.
    Parameter(String),
    /// The Amazon CloudWatch destination is invalid.
    CloudWatch(String),
    /// The Amazon Kinesis Firehose destination is invalid.
    Firehose(String),
    /// The Amazon SNS destination is invalid.
    Sns(String),
}

/// The Amazon Kinesis Firehose delivery stream an event destination publishes
/// to: `arn:<partition>:firehose:<region>:<account>:deliverystream/<name>`.
const FIREHOSE_DELIVERY_STREAM: ::service_common::arn::Requirement =
    ::service_common::arn::Requirement {
        vendor: "firehose",
        resource_type: Some("deliverystream"),
        label: "an Amazon Kinesis Firehose delivery stream",
    };

/// The longest Amazon CloudWatch dimension name, and the characters one may be
/// built from.
///
/// `CloudWatchDimensionConfiguration$DimensionName` states the rule: the name
/// must "contain only ASCII letters (a-z, A-Z), numbers (0-9), underscores (_),
/// dashes (-), or colons (:)" and "contain 256 characters or fewer".
const MAX_DIMENSION_NAME: usize = 256;

/// The longest default value of an Amazon CloudWatch dimension, and the
/// characters one may be built from.
///
/// `CloudWatchDimensionConfiguration$DefaultDimensionValue` states the rule: the
/// value must "contain only ASCII letters (a-z, A-Z), numbers (0-9),
/// underscores (_), dashes (-), at signs (@), or periods (.)" and "contain 256
/// characters or fewer".
const MAX_DEFAULT_DIMENSION_VALUE: usize = 256;

/// Whether `value` is an Amazon CloudWatch dimension name, under the rule
/// [`MAX_DIMENSION_NAME`] states.
fn is_dimension_name(value: &str) -> bool {
    !value.is_empty()
        && value.chars().count() <= MAX_DIMENSION_NAME
        && value.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '_' | '-' | ':')
        })
}

/// Whether `value` is a default dimension value, under the rule
/// [`MAX_DEFAULT_DIMENSION_VALUE`] states.
fn is_default_dimension_value(value: &str) -> bool {
    !value.is_empty()
        && value.chars().count() <= MAX_DEFAULT_DIMENSION_VALUE
        && value.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '_' | '-' | '@' | '.')
        })
}

/// What an `EventDestination` a request supplies has to carry, or the reason it
/// describes nothing Amazon SES could publish to.
///
/// `EventDestination` states the destination-independent half of the rule: a
/// `Name` of the documented form ([`is_event_destination_name`]), event types
/// drawn from the ones the model names
/// ([`crate::ops::EVENT_TYPE_VALUES`]), and — "when you create or update an
/// event destination, you must provide one, and only one, destination" — exactly
/// one of the three destination members. Each destination is then read for what
/// its own members have to name: an ARN names ONE account's resource in ONE
/// Region, and this emulator serves a single account
/// ([`names_a_resource_here`]), so a stream, a topic or a role of another
/// account, partition or Region is one this caller could not publish through.
/// Whether the resource EXISTS is the owning service's own state, which this
/// service does not hold — the same reading [`resolve_sns_topic`] makes.
///
/// Shared by `CreateConfigurationSetEventDestination` and
/// `UpdateConfigurationSetEventDestination`, which hold a destination to one
/// rule between them; each wraps the fault in its own modelled error.
/// Read every ARN member of `destination` as the trimmed value it names a
/// resource by, so the destination is STORED the way it was asked about.
///
/// [`check_event_destination`] judges, and [`resolve_sns_topic`] asks about, the
/// trimmed value of each ARN member; a store keeping the padded text would
/// report back a value no later lookup of this service would match, making the
/// record disagree with the resource it names. Shared by
/// `CreateConfigurationSetEventDestination` and
/// `UpdateConfigurationSetEventDestination`, which write one destination shape
/// between them.
pub fn trim_event_destination_arns(destination: &mut crate::ops::types::EventDestination) {
    if let Some(sns) = destination.sns_destination.as_mut() {
        sns.topic_arn = sns.topic_arn.trim().to_string();
    }
    if let Some(firehose) = destination.kinesis_firehose_destination.as_mut() {
        firehose.delivery_stream_arn = firehose.delivery_stream_arn.trim().to_string();
        firehose.iam_role_arn = firehose.iam_role_arn.trim().to_string();
    }
}

pub fn check_event_destination(
    destination: &crate::ops::types::EventDestination,
    region: &str,
) -> Result<(), EventDestinationFault> {
    if !is_event_destination_name(&destination.name) {
        return Err(EventDestinationFault::Parameter(format!(
            "Invalid EventDestination.Name {:?}: Name must contain only ASCII letters (a-z, A-Z), \
             numbers (0-9), underscores (_), or dashes (-), and {MAX_EVENT_DESTINATION_NAME} \
             characters or fewer",
            destination.name
        )));
    }
    // The model declares the value set of `MatchingEventTypes`, so the boundary
    // validator normally refuses anything else; this is the belt-and-braces
    // refusal, so an event type this service cannot publish is never stored as
    // one it can.
    if let Some(unknown) = destination
        .matching_event_types
        .iter()
        .find(|event_type| !crate::ops::EVENT_TYPE_VALUES.contains(&event_type.as_str()))
    {
        return Err(EventDestinationFault::Parameter(format!(
            "Invalid EventDestination.MatchingEventTypes entry {unknown:?}: each entry must be \
             one of {}",
            crate::ops::EVENT_TYPE_VALUES.join(", ")
        )));
    }
    let provided = [
        destination.cloud_watch_destination.is_some(),
        destination.kinesis_firehose_destination.is_some(),
        destination.sns_destination.is_some(),
    ]
    .into_iter()
    .filter(|provided| *provided)
    .count();
    if provided != 1 {
        return Err(EventDestinationFault::Parameter(format!(
            "You must provide one, and only one, destination: this EventDestination provides \
             {provided} of CloudWatchDestination, KinesisFirehoseDestination and SNSDestination"
        )));
    }

    if let Some(cloud_watch) = &destination.cloud_watch_destination {
        // "A list of dimensions upon which to categorize your emails": a
        // destination that categorizes them on nothing publishes no dimension,
        // so the list the member requires carries at least one configuration.
        // On the awsQuery wire an empty list carries no field at all, so such a
        // request provides no destination and the rule above refuses it first.
        if cloud_watch.dimension_configurations.is_empty() {
            return Err(EventDestinationFault::CloudWatch(
                "CloudWatchDestination.DimensionConfigurations must carry at least one dimension \
                 configuration"
                    .to_string(),
            ));
        }
        for dimension in &cloud_watch.dimension_configurations {
            if !is_dimension_name(&dimension.dimension_name) {
                return Err(EventDestinationFault::CloudWatch(format!(
                    "Invalid DimensionName {:?}: DimensionName must contain only ASCII letters \
                     (a-z, A-Z), numbers (0-9), underscores (_), dashes (-), or colons (:), and \
                     {MAX_DIMENSION_NAME} characters or fewer",
                    dimension.dimension_name
                )));
            }
            if !crate::ops::DIMENSION_VALUE_SOURCE_VALUES
                .contains(&dimension.dimension_value_source.as_str())
            {
                return Err(EventDestinationFault::CloudWatch(format!(
                    "Invalid DimensionValueSource {:?}: DimensionValueSource must be one of {}",
                    dimension.dimension_value_source,
                    crate::ops::DIMENSION_VALUE_SOURCE_VALUES.join(", ")
                )));
            }
            if !is_default_dimension_value(&dimension.default_dimension_value) {
                return Err(EventDestinationFault::CloudWatch(format!(
                    "Invalid DefaultDimensionValue {:?}: DefaultDimensionValue must contain only \
                     ASCII letters (a-z, A-Z), numbers (0-9), underscores (_), dashes (-), at \
                     signs (@), or periods (.), and {MAX_DEFAULT_DIMENSION_VALUE} characters or \
                     fewer",
                    dimension.default_dimension_value
                )));
            }
        }
    }

    if let Some(firehose) = &destination.kinesis_firehose_destination {
        let stream = firehose.delivery_stream_arn.trim();
        if !(names_a_resource_here(stream, FIREHOSE_DELIVERY_STREAM.vendor, region)
            && FIREHOSE_DELIVERY_STREAM.admits(stream))
        {
            return Err(EventDestinationFault::Firehose(format!(
                "Invalid DeliveryStreamARN {stream:?}: DeliveryStreamARN {} of account {} in \
                 {region}",
                FIREHOSE_DELIVERY_STREAM.phrase(),
                ::engine::EMULATOR_ACCOUNT_ID
            )));
        }
        // An IAM role is GLOBAL: IAM writes no Region into a role ARN, so the
        // account is all a role ARN of this emulator's one account has to carry.
        let role = firehose.iam_role_arn.trim();
        let role_of_this_account = ::service_common::arn::IAM_ROLE.admits(role)
            && ::service_common::arn::parse(role).is_some_and(|arn| {
                arn.partition == ARN_PARTITION && arn.account == ::engine::EMULATOR_ACCOUNT_ID
            });
        if !role_of_this_account {
            return Err(EventDestinationFault::Firehose(format!(
                "Invalid IAMRoleARN {role:?}: IAMRoleARN {} of account {}",
                ::service_common::arn::IAM_ROLE.phrase(),
                ::engine::EMULATOR_ACCOUNT_ID
            )));
        }
    }

    if let Some(sns) = &destination.sns_destination {
        // The same reading of a topic ARN every member of this service that
        // names one makes.
        if resolve_sns_topic(&sns.topic_arn, region).is_err() {
            return Err(EventDestinationFault::Sns(format!(
                "Invalid TopicARN {:?}: TopicARN must be the ARN of an Amazon SNS topic of \
                 account {} in {region}",
                sns.topic_arn.trim(),
                ::engine::EMULATOR_ACCOUNT_ID
            )));
        }
    }

    Ok(())
}

/// The identity-list key for an identity a request names, or the reason the
/// value names no identity this emulator holds.
///
/// [`identity_key`] reads the two forms the `Identity` member documents — the
/// identity's own name, and an ARN of this service naming an `identity`
/// resource. This COMPOSES with that one reading ([`identity_named_by_arn`]) and
/// adds what an ARN has to satisfy to name an identity HERE: an ARN names one
/// account's resource in one Region, and this emulator serves a single account
/// ([`names_a_resource_here`]), so an ARN of another account, partition or
/// Region names nothing — as does an ARN that names no identity at all (another
/// vendor's resource, or another resource of this service). The operations that
/// also read an Amazon SNS topic ARN ([`resolve_sns_topic`]) read BOTH their ARN
/// members this way, so one request is not held to two different readings of
/// what an ARN names; a value that is not an ARN at all is keyed exactly as
/// [`identity_key`] keys it.
///
/// `Err` carries the reason, for a caller that refuses the request with it.
pub fn resolve_identity(value: &str, region: &str) -> Result<String, String> {
    let trimmed = value.trim();
    // A value that is not an ARN at all is the identity's own name.
    if ::service_common::arn::parse(trimmed).is_none() {
        return Ok(identity_key(trimmed));
    }
    match identity_named_by_arn(trimmed)
        .filter(|_| names_a_resource_here(trimmed, SES_ARN_VENDOR, region))
    {
        Some(name) => Ok(::service_common::ses_identities::identity_key(name)),
        None => Err(format!(
            "Invalid Identity {trimmed:?}: an identity ARN must name an identity of account {} \
             in {region}",
            ::engine::EMULATOR_ACCOUNT_ID
        )),
    }
}

/// The sentence Amazon SES refuses a tracking-options write with when the custom
/// redirect domain it was given is not a domain name at all: fixed text around
/// the value the request supplied.
///
/// The value is the one SUBMITTED, verbatim — a refusal for a value that is not a
/// domain name has nothing normalised to name, and what the caller has to correct
/// is what the caller sent. Angle brackets delimit it, the form every refusal of
/// this surface names a value in.
pub fn invalid_tracking_domain_message(custom_redirect_domain: &str) -> String {
    format!("Domain <{custom_redirect_domain}> is not a valid domain.")
}

/// The sentence Amazon SES refuses a tracking-options write with when the custom
/// redirect domain is a domain name, but no verified identity of this account
/// covers it: the same form as [`invalid_tracking_domain_message`], for the other
/// condition `InvalidTrackingOptionsException` lists.
///
/// The two conditions are worded apart because they are different problems — a
/// typo'd value against a domain whose ownership the account has not proved — and
/// the message names the one domain at fault, so a caller configuring several can
/// tell which was rejected and why.
pub fn unverified_tracking_domain_message(custom_redirect_domain: &str) -> String {
    format!("Domain <{custom_redirect_domain}> is not verified under this account.")
}

/// Whether `value` is a domain name with at least two non-empty labels, the
/// `[SubDomain.]Domain.TopLevelDomain` form SES documents.
///
/// The shared reading again: a label is 7-bit ASCII letters, digits and hyphens,
/// and the top-level label must carry a letter, which admits an
/// internationalised (`xn--…`) TLD and refuses an all-numeric one.
pub fn is_domain_name(value: &str) -> bool {
    ::service_common::email_address::is_domain_name(value)
}

/// The domain an identity's mail comes from: the identity itself for a domain
/// identity, and the domain half of the address for an email-address identity.
///
/// Both forms are what `SetIdentityMailFromDomainRequest$MailFromDomain` is read
/// against — the member documents the identity as "an email address or a domain"
/// — so the rule is stated once, here.
pub fn identity_domain(identity: &str) -> &str {
    match identity.split_once('@') {
        Some((_, domain)) => domain,
        None => identity,
    }
}

/// Whether `candidate` is a domain strictly BELOW `parent`: it ends in `parent`
/// after a label separator, so it carries at least one more label.
///
/// Compared case-insensitively, because a domain name is: two spellings that
/// differ only in case name the same domain.
fn is_subdomain_of(candidate: &str, parent: &str) -> bool {
    let Some(prefix) = candidate
        .len()
        .checked_sub(parent.len())
        .filter(|prefix| *prefix > 1)
    else {
        return false;
    };
    candidate[prefix..].eq_ignore_ascii_case(parent) && candidate.as_bytes()[prefix - 1] == b'.'
}

/// The custom MAIL FROM domain `identity` may be configured with, or the message
/// SES refuses the request as naming an unusable one with.
///
/// `SetIdentityMailFromDomainRequest$MailFromDomain` states the rule: "The MAIL
/// FROM domain must 1) be a subdomain of the verified identity" — so it has to be
/// a domain name at all ([`is_domain_name`]) and to sit strictly below the
/// identity's own domain ([`identity_domain`]), which for an email-address
/// identity is the domain of the address. A name that IS the identity's domain is
/// not a subdomain of it, and so is not one SES accepts.
///
/// Only a single trailing dot is taken off — the root label of a fully qualified
/// name, which names the same domain — exactly as `VerifyDomainIdentity` reads
/// the domain it records.
pub fn mail_from_domain_for(identity: &str, value: &str) -> Result<String, String> {
    let trimmed = value.trim();
    let domain = trimmed.strip_suffix('.').unwrap_or(trimmed);
    if !is_domain_name(domain) {
        return Err(format!(
            "MailFromDomain must be a domain name of the form \
             [SubDomain.]Domain.TopLevelDomain, got {value:?}"
        ));
    }
    let parent = identity_domain(identity);
    if !is_subdomain_of(domain, parent) {
        return Err(format!(
            "Provided MAIL-FROM domain <{domain}> is not subdomain of the domain of the \
             identity <{identity}>."
        ));
    }
    Ok(domain.to_string())
}

/// Whether `name` is a configuration set name of the form the model documents.
///
/// `ConfigurationSet$Name` states both halves of the rule: a name must "contain
/// only letters (a-z, A-Z), numbers (0-9), underscores (_), or dashes (-)" and
/// "contain 64 characters or fewer" ([`MAX_CONFIGURATION_SET_NAME`]). A name
/// built from no characters at all names nothing, so it is not one either.
pub fn is_configuration_set_name(name: &str) -> bool {
    is_letters_digits_name(name, MAX_CONFIGURATION_SET_NAME)
}

/// Whether `name` is an event destination name of the form the model documents.
///
/// `EventDestination$Name` states the same two halves as a configuration set's
/// name does, over the same character set: the name must "contain only ASCII
/// letters (a-z, A-Z), numbers (0-9), underscores (_), or dashes (-)" and
/// "contain 64 characters or fewer" ([`MAX_EVENT_DESTINATION_NAME`]).
pub fn is_event_destination_name(name: &str) -> bool {
    is_letters_digits_name(name, MAX_EVENT_DESTINATION_NAME)
}

/// Whether `name` is built from ASCII letters, digits, underscores and dashes
/// alone and is at most `max` characters long.
///
/// The ONE reading of that form in this service, because the models of the
/// resources that carry a name — a configuration set
/// ([`is_configuration_set_name`]), an event destination
/// ([`is_event_destination_name`]) — document it in the same words. A name built
/// from no characters at all names nothing, so it is not one either.
fn is_letters_digits_name(name: &str, max: usize) -> bool {
    !name.is_empty()
        && name.chars().count() <= max
        && name
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '_' | '-'))
}

/// Every recipient of a `Destination`, To: then CC: then BCC:, in request order.
///
/// They are flattened because SES treats them as one recipient set: the
/// [`MAX_RECIPIENTS`] ceiling is "across the To:, CC: and BCC: fields", and each
/// one counts once against the sending quota. One reading here, so every
/// operation that carries a `Destination` — `SendEmail`, `SendTemplatedEmail`
/// and each destination of `SendBulkTemplatedEmail` — resolves the same
/// recipient set out of it.
pub fn destination_recipients(destination: &crate::ops::types::Destination) -> Vec<String> {
    [
        destination.to_addresses.as_ref(),
        destination.cc_addresses.as_ref(),
        destination.bcc_addresses.as_ref(),
    ]
    .into_iter()
    .flatten()
    .flatten()
    .cloned()
    .collect()
}

/// `text` as a log entry keeps it: at most [`MAX_LOGGED_TEXT`] characters, and
/// `None` for a text that is not there.
///
/// Truncated on a CHARACTER boundary, so a multi-byte character is never split.
pub fn logged_text(text: Option<&str>) -> Option<String> {
    text.map(|text| match text.char_indices().nth(MAX_LOGGED_TEXT) {
        Some((cut, _)) => text[..cut].to_string(),
        None => text.to_string(),
    })
}

/// How a send refuses an address that is not an address, for every operation
/// that takes one.
///
/// `SendEmail` states the rule and the consequence: a recipient address that "is
/// not in the format UserName@[SubDomain.]Domain.TopLevelDomain" makes the whole
/// message rejected. One wording here, so the operations that share that reading
/// ([`is_email_address`]) also share how they report it.
pub fn illegal_address_message(value: &str) -> String {
    format!(
        "Illegal address: {value:?} is not of the form \
         UserName@[SubDomain.]Domain.TopLevelDomain"
    )
}

/// How a send refuses a message whose sending identity this account has not
/// verified, for every operation that gates a send on one.
///
/// SES identities are per-REGION — an identity verified in one Region is not
/// verified in another — so the refusal names the Region the check ran in, upper
/// cased, before the identities that failed it. Everything but the Region token
/// and the identity list is fixed text; the Region comes from the request's
/// credential scope ([`region_from_request`]) and the identities from the request.
/// One wording, for the same reason as [`illegal_address_message`].
pub fn unverified_identities_message(region: &str, identities: &[&str]) -> String {
    format!(
        "Email address is not verified. The following identities failed the check in region \
         {}: {}",
        region.to_uppercase(),
        identities.join(", ")
    )
}

/// How a send refuses a message that would take the account past its 24-hour
/// sending quota, for every operation that counts against it.
///
/// `GetSendQuota$Max24HourSend` is "the maximum number of emails the user is
/// allowed to send in a 24-hour interval" and `SentLast24Hours` what has been
/// sent in it, so the refusal names both. One wording, for the same reason as
/// [`illegal_address_message`].
pub fn daily_quota_exceeded_message(quota: f64, already_sent: f64) -> String {
    format!(
        "Daily message quota exceeded: this account may send {quota} messages per 24 hours \
         and has sent {already_sent}"
    )
}

/// How this service states the absence of a custom verification email template,
/// for every operation that resolves one by name.
///
/// `CustomVerificationEmailTemplateDoesNotExistException` is "Indicates that a
/// custom verification email template with the name you specified does not
/// exist", and the sentence the service answers for it is FIXED: it speaks of
/// the provided name without quoting it, so no part of it comes from the
/// request and two calls differing only in the name they ask for are worded
/// identically. A constant rather than a formatter, because there is nothing to
/// interpolate. One wording, for the same reason as [`illegal_address_message`]:
/// the operations that resolve a template out of one store report its absence
/// identically.
pub const MISSING_CUSTOM_VERIFICATION_EMAIL_TEMPLATE_MESSAGE: &str =
    "Provided custom verification email template name does not exist.";

/// How this service refuses a custom verification email whose sender this
/// account has not verified, for every operation that gates one on it.
///
/// `FromEmailAddressNotVerifiedException` is "Indicates that the sender address
/// specified for a custom verification email is not verified, and is therefore
/// not eligible to send the custom verification email", so the address the
/// request (or the stored template) carried is the one variable part, stated
/// bare — the sentence names the address, it does not bracket it. One wording,
/// for the same reason as [`illegal_address_message`].
pub fn unverified_from_address_message(from_email_address: &str) -> String {
    format!("The from email address {from_email_address} is not verified.")
}

/// How this service states the absence of an event destination, for every
/// operation that resolves one on a configuration set that IS there.
///
/// `EventDestinationDoesNotExistException` is "Indicates that the event
/// destination does not exist", and the sentence the service answers for it
/// reports the lookup it made over the SET: the fixed text
/// `No EventDestination found for ` followed by the configuration set the
/// request named, bare, with no trailing period, and with the destination's own
/// name absent — the service does not report which destination was asked for.
/// One wording, for the same reason as [`illegal_address_message`]: the
/// operations that resolve a destination out of one set report its absence
/// identically.
pub fn missing_event_destination_message(configuration_set_name: &str) -> String {
    format!("No EventDestination found for {configuration_set_name}")
}

/// 64 bits of entropy this process did not choose, salting a minted id so it is
/// opaque as well as unique.
///
/// `std::collections::hash_map::RandomState::new` "is initialized with random
/// keys" (its own documentation) — the system seeds them — and a fresh one per
/// call carries fresh keys, so finishing its hasher yields a value a caller
/// cannot derive from the request or from an earlier id. It salts an identifier
/// and nothing else: it is never a key, a secret or a signature, and the
/// UNIQUENESS of an id comes from the stored counter it digests, not from here.
fn system_entropy() -> u64 {
    use std::collections::hash_map::RandomState;
    use std::hash::{BuildHasher, Hasher};
    RandomState::new().build_hasher().finish()
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
///
/// Every call RECONCILES this service's identity list against the account's identity
/// registry first ([`reconcile_identities`]), so an operation served here answers for
/// every identity the account holds — including the ones recorded through the SES v2
/// surface — and for none it has deleted. Its configuration sets are reconciled against
/// the account's configuration-set registry the same way
/// ([`reconcile_configuration_sets`]), and for the same reason. Its email templates are
/// reconciled against the account's email-template registry the same way
/// ([`reconcile_templates`]), and for the same reason.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsSesSimpleemailserviceState>> {
    let state = own_state(engine)?;
    let registry = identity_registry(engine)?;
    reconcile_identities(&state, &registry, SystemTime::now());
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
) -> Option<::engine::ServiceState<ComAmazonawsSesSimpleemailserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsSesSimpleemailserviceState>)
}

/// Record `name` as an identity of this account of class `identity_type` — in this
/// service's own identity list and in the account registry both SES surfaces answer
/// from — and hand back the stored record.
///
/// This is what the verification operations of this surface write through, so an
/// identity they record is one the v2 surface lists and reports from the moment the
/// call returns, and a DOMAIN identity carries the registry's TXT verification token
/// (the value the owner publishes at `_amazonses.<domain>`).
///
/// Idempotent, exactly as [`ComAmazonawsSesSimpleemailserviceState::verify_identity`]
/// is: an identity already held keeps its record, its token and whatever is left of
/// the span its pending verification started.
///
/// `None` when this account is already at its identity limit and this is a new
/// identity — the caller refuses the request — or when the engine has no store for
/// this service at all.
pub fn record_identity<Eng: Send + Sync + 'static>(
    engine: &Eng,
    name: &str,
    identity_type: IdentityType,
) -> Option<Identity> {
    let state = service_state(engine)?;
    let registry = identity_registry(engine)?;
    // The lock order of this service: its own store, then the registry.
    let mut state = state.lock();
    let mut registry = registry.lock();
    let recorded = state.verify_identity(name, identity_type)?.clone();
    let published = registry.publish(registry_record(&recorded))?.clone();
    let held = state.identities.get_mut(&identity_key(&recorded.name))?;
    held.registered = true;
    held.verification_token = published.verification_token;
    Some(held.clone())
}

/// Set up Easy DKIM for the domain identity `domain` — recording the domain as an
/// identity of this account if it is not one already, in this service's list and in
/// the account registry both — and hand back the tokens it is to publish as CNAME
/// records.
///
/// The identity half of `VerifyDomainDkim`, which "adds the domain to the list of
/// identities associated with your account": the same list, and the same publication,
/// as [`record_identity`].
///
/// The tokens are the ACCOUNT's: a domain the account already holds an Easy DKIM setup
/// for — because another API over its identities recorded one — keeps that setup, so
/// every surface answers with the records the owner was told to publish.
///
/// `None` when this account is already at its identity limit and the domain is a new
/// identity, or when the engine has no store for this service.
pub fn record_easy_dkim<Eng: Send + Sync + 'static>(
    engine: &Eng,
    domain: &str,
) -> Option<Vec<String>> {
    let state = service_state(engine)?;
    let registry = identity_registry(engine)?;
    let mut state = state.lock();
    let mut registry = registry.lock();
    // The setup the ACCOUNT already holds for the domain, when another API over its
    // identities recorded one: its tokens are the records the owner publishes, so
    // this call answers with them rather than with a second set.
    let held = registry
        .get(domain)
        .and_then(|record| record.dkim.as_ref())
        .map(|shared| EasyDkimSettings {
            tokens: shared.tokens.clone(),
            status: service_status(shared.status),
        });
    let tokens = state.request_easy_dkim(domain, held)?;
    let key = identity_key(domain);
    let recorded = state.identities.get(&key)?.clone();
    let published = registry.publish(registry_record(&recorded))?.clone();
    let held = state.identities.get_mut(&key)?;
    held.registered = true;
    held.verification_token = published.verification_token;
    Some(tokens)
}

/// Remove `name` from this account's identity list — from this service's own store and
/// from the account registry both — so neither SES surface reports it afterwards.
///
/// Removing an identity this account does not hold changes nothing, which is the rule
/// both delete operations of this surface follow.
pub fn forget_identity<Eng: Send + Sync + 'static>(engine: &Eng, name: &str) {
    let Some(state) = service_state(engine) else {
        return;
    };
    let Some(registry) = identity_registry(engine) else {
        return;
    };
    let mut state = state.lock();
    let mut registry = registry.lock();
    state.identities.remove(&identity_key(name));
    registry.forget(name);
}

/// Publish the identity `name` as this service now holds it — its verification, its
/// Easy DKIM setup and its sending-authorization policies — to the account's identity
/// registry, so every other API over the account's identities reports what this one
/// last wrote.
///
/// What an operation that CHANGES a shared fact about an identity calls once it has
/// written it. Changes nothing when the account holds no such identity, and writes
/// nothing but the registry's record of it. Reads this service's store WITHOUT
/// reconciling it ([`own_state`]): the change being published is newer than the
/// registry's copy of it.
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
/// SES v2 surface both publish to and read: call it with a handler's `&*self.engine`.
///
/// An operation that RECORDS or DELETES an identity publishes the change here, so the
/// other surface answers for it from the moment the call returns. Take this lock while
/// holding this service's own store, never the other way round, so two callers on
/// different surfaces cannot wait on each other.
pub fn identity_registry<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<::service_common::ses_identities::SesIdentityDirectory>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<
            ::service_common::ses_identities::SesIdentityDirectory,
        >)
}

/// Bring this service's identity list and the account's identity registry back into
/// agreement at `now`, so every API over the account's identities answers for ONE set
/// of them.
///
/// The reconciliation itself is the registry's
/// ([`::service_common::ses_identities::SesIdentityDirectory::reconcile`]), written
/// once there rather than per surface; what this service contributes is its
/// [`AccountIdentityRecord`] impl below — how its own table is keyed, how a record of
/// it is published, and what an adopted one starts with.
fn reconcile_identities(
    state: &::engine::ServiceState<ComAmazonawsSesSimpleemailserviceState>,
    registry: &::engine::ServiceState<::service_common::ses_identities::SesIdentityDirectory>,
    now: SystemTime,
) {
    // The lock order of this service: its own store, then the registry.
    let mut state = state.lock();
    let mut registry = registry.lock();
    registry.reconcile(&mut state.identities, now);
}

/// The account's configuration-set registry in the shared engine, which this service and
/// the SES v2 surface both publish to and read: call it with a handler's `&*self.engine`.
///
/// An operation that CREATES, CONFIGURES or DELETES a configuration set publishes the
/// change here, so the other surface answers for it from the moment the call returns. Take
/// this lock while holding this service's own store, never the other way round, so two
/// callers on different surfaces cannot wait on each other.
pub fn configuration_set_registry<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<
    ::engine::ServiceState<
        ::service_common::ses_configuration_sets::SesConfigurationSetDirectory,
    >,
> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<
            ::service_common::ses_configuration_sets::SesConfigurationSetDirectory,
        >)
}

/// Bring this service's configuration-set store and the account's configuration-set
/// registry back into agreement at `now`, so every API over the account's configuration
/// sets answers for ONE set of them.
///
/// The reconciliation itself is the registry's
/// ([`::service_common::ses_configuration_sets::SesConfigurationSetDirectory::reconcile`]),
/// written once there rather than per surface; what this service contributes is its
/// [`::service_common::ses_configuration_sets::AccountConfigurationSetRecord`] impl below.
fn reconcile_configuration_sets(
    state: &::engine::ServiceState<ComAmazonawsSesSimpleemailserviceState>,
    registry: &::engine::ServiceState<
        ::service_common::ses_configuration_sets::SesConfigurationSetDirectory,
    >,
    now: SystemTime,
) {
    // The lock order of this service: its own store, then the registry.
    let mut state = state.lock();
    let mut registry = registry.lock();
    registry.reconcile(&mut state.configuration_sets, now);
}

/// Publish the configuration set `name` as this service now holds it — its tracking
/// options, its delivery options, whether its reputation metrics are collected and whether
/// sending through it is enabled — to the account's configuration-set registry, so every
/// other API over the account's configuration sets reports what this one last wrote.
///
/// What an operation that CREATES or CHANGES a configuration set calls once it has written
/// it. Changes nothing when the account holds no such set, and writes nothing but the
/// registry's record of it. Reads this service's store WITHOUT reconciling it
/// ([`own_state`]): the change being published is newer than the registry's copy of it.
pub fn publish_configuration_set<Eng: Send + Sync + 'static>(engine: &Eng, name: &str) {
    let Some(state) = own_state(engine) else {
        return;
    };
    let Some(registry) = configuration_set_registry(engine) else {
        return;
    };
    // The lock order of this service: its own store, then the registry.
    let mut state = state.lock();
    let mut registry = registry.lock();
    let Some(held) = state.configuration_sets.get(name) else {
        return;
    };
    if registry
        .publish(
            ::service_common::ses_configuration_sets::AccountConfigurationSetRecord::published(
                held,
            ),
            SystemTime::now(),
        )
        .is_some()
    {
        if let Some(held) = state.configuration_sets.get_mut(name) {
            held.registered = true;
        }
    }
}

/// Remove the configuration set `name` names from this account — from this service's own
/// store and from the account registry both — so neither SES surface reports it afterwards.
///
/// Removing a set this account does not hold changes nothing, which is the rule the delete
/// operation of this surface follows.
pub fn forget_configuration_set<Eng: Send + Sync + 'static>(engine: &Eng, name: &str) {
    let Some(state) = own_state(engine) else {
        return;
    };
    let Some(registry) = configuration_set_registry(engine) else {
        return;
    };
    let mut state = state.lock();
    let mut registry = registry.lock();
    state.configuration_sets.remove(name);
    registry.forget(name);
}

impl ::service_common::ses_configuration_sets::AccountConfigurationSetRecord
    for ConfigurationSet
{
    fn set_name(&self) -> &str {
        &self.name
    }

    fn is_registered(&self) -> bool {
        self.registered
    }

    fn mark_registered(&mut self) {
        self.registered = true;
    }

    /// The account's configuration for the set, onto the members this surface's own model
    /// spells them with. Everything this surface adds beside them — the set's event
    /// destinations — is untouched.
    fn take_shared(
        &mut self,
        published: &::service_common::ses_configuration_sets::SesConfigurationSetRecord,
    ) {
        self.custom_redirect_domain = published.custom_redirect_domain.clone();
        // The policy is spelled with THIS model's own value set
        // (`crate::ops::types::TLS_POLICY_VALUES`), which the v2 surface spells in upper
        // case: a client of this API reads a value its own model declares.
        self.tls_policy = published.tls_policy.map(|policy| {
            match policy {
                ::service_common::ses_configuration_sets::SesTlsPolicy::Require => "Require",
                ::service_common::ses_configuration_sets::SesTlsPolicy::Optional => "Optional",
            }
            .to_string()
        });
        self.reputation_metrics_enabled = published.reputation_metrics_enabled;
        self.sending_enabled = published.sending_enabled;
        self.last_fresh_start = published.last_fresh_start;
    }

    fn published(
        &self,
    ) -> ::service_common::ses_configuration_sets::SesConfigurationSetRecord {
        ::service_common::ses_configuration_sets::SesConfigurationSetRecord {
            name: self.name.clone(),
            custom_redirect_domain: self.custom_redirect_domain.clone(),
            tls_policy: self
                .tls_policy
                .as_deref()
                .and_then(::service_common::ses_configuration_sets::SesTlsPolicy::of),
            reputation_metrics_enabled: self.reputation_metrics_enabled,
            sending_enabled: self.sending_enabled,
            last_fresh_start: self.last_fresh_start,
        }
    }

    /// This surface's record of a set the account holds that it does not: the set exists,
    /// carries the account's own configuration (applied by [`Self::take_shared`], which runs
    /// for every set both sides hold) and publishes to no event destination of this
    /// surface, since none has been created through it.
    fn adopt(
        record: &::service_common::ses_configuration_sets::SesConfigurationSetRecord,
        _created_at: SystemTime,
    ) -> Self {
        Self {
            name: record.name.clone(),
            sending_enabled: record.sending_enabled,
            reputation_metrics_enabled: record.reputation_metrics_enabled,
            custom_redirect_domain: record.custom_redirect_domain.clone(),
            tls_policy: record.tls_policy.map(|policy| {
                match policy {
                    ::service_common::ses_configuration_sets::SesTlsPolicy::Require => "Require",
                    ::service_common::ses_configuration_sets::SesTlsPolicy::Optional => {
                        "Optional"
                    }
                }
                .to_string()
            }),
            last_fresh_start: record.last_fresh_start,
            event_destinations: Vec::new(),
            registered: true,
        }
    }
}

impl ::service_common::ses_identities::AccountIdentityRecord for Identity {
    /// This service's table is keyed by [`identity_key`] — the account identity
    /// registry's key, which is the addr-spec with the domain part compared the way
    /// DNS compares a name — the way its own operations look one up.
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
        self.registered = true;
        // The TXT value a domain identity's owner publishes is the registry's, so both
        // surfaces report the one value, and it is stored here so every later read of
        // this record answers the same.
        self.verification_token = published.verification_token.clone();
        // The account may already hold an Easy DKIM setup for the identity, recorded
        // through another API over these identities, and its tokens are the records the
        // owner publishes: this record takes them rather than reporting none.
        if self.dkim.is_none() {
            self.dkim = published.dkim.as_ref().map(|shared| EasyDkimSettings {
                tokens: shared.tokens.clone(),
                status: service_status(shared.status),
            });
        }
    }

    /// The sending-authorization policies the ACCOUNT's identity holds, which this API
    /// lists and reports whichever API attached them, and the Easy DKIM setup it holds,
    /// whose tokens are the records its owner publishes.
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
        // none. A record already carrying a setup keeps it: the first setup stands,
        // because its tokens may already be in DNS.
        let gained = published.dkim.as_ref().filter(|_| self.dkim.is_none());
        if let Some(shared) = gained {
            self.dkim = Some(EasyDkimSettings {
                tokens: shared.tokens.clone(),
                status: service_status(shared.status),
            });
            self.dkim_enabled = shared.signing_enabled;
        }
    }

    fn published(&self) -> ::service_common::ses_identities::SesIdentityRecord {
        registry_record(self)
    }

    /// The registry's verification state, due instant and TXT token, with the defaults
    /// every identity of this surface starts with.
    fn adopt(
        published: &::service_common::ses_identities::SesIdentityRecord,
        now: SystemTime,
    ) -> Self {
        use ::service_common::ses_identities::{SesIdentityKind, SesVerificationState};

        Identity {
            name: published.name.clone(),
            identity_type: match published.kind {
                SesIdentityKind::EmailAddress => IdentityType::EmailAddress,
                SesIdentityKind::Domain => IdentityType::Domain,
            },
            verification_status: service_status(published.verification),
            verification_token: published.verification_token.clone(),
            // Easy DKIM is set up for the ACCOUNT's identity, so an identity recorded
            // through another API over these identities is adopted with the setup the
            // account holds for it — the tokens its owner publishes and where the
            // search for them stands — and with none when no setup was ever asked for.
            //
            // Notification topics are set by `SetIdentityNotificationTopic` and a
            // custom MAIL FROM domain by `SetIdentityMailFromDomain`: an identity
            // recorded elsewhere has had neither asked for, so each starts where every
            // identity of this surface starts it. Its sending-authorization policies
            // are the ACCOUNT's, so they are adopted with it.
            dkim: published.dkim.as_ref().map(|shared| EasyDkimSettings {
                tokens: shared.tokens.clone(),
                // An identity whose verification succeeded is one whose published
                // records the service has read, so a setup still waiting on them is
                // no longer — the rule every surface over these identities applies
                // when it adopts one.
                status: match (shared.status, published.verification) {
                    (SesVerificationState::Pending, SesVerificationState::Success) => {
                        VerificationStatus::Success
                    }
                    (status, _) => service_status(status),
                },
            }),
            dkim_enabled: published
                .dkim
                .as_ref()
                .map_or(DKIM_SIGNING_AT_IDENTITY_START, |shared| {
                    shared.signing_enabled
                }),
            notifications: NotificationSettings::default(),
            mail_from: None,
            policies: published.policies.clone().unwrap_or_default(),
            // The identity was added to the account's list when it was recorded, which
            // is not an instant this surface observed; the registry carries the
            // verification it is still owed, which is what a reader of this record
            // reports.
            verified_at: now,
            verification_due: published.verification_due,
            registered: true,
        }
    }
}

/// How the account registry carries the identity `identity`: the identity as it was
/// recorded, its class, the verification this service holds for it, and the Easy DKIM
/// setup it holds for it.
pub fn registry_record(
    identity: &Identity,
) -> ::service_common::ses_identities::SesIdentityRecord {
    use ::service_common::ses_identities::{SesDkimSetup, SesIdentityKind, SesIdentityRecord};

    SesIdentityRecord {
        name: identity.name.clone(),
        kind: match identity.identity_type {
            IdentityType::EmailAddress => SesIdentityKind::EmailAddress,
            IdentityType::Domain => SesIdentityKind::Domain,
        },
        verification: registry_state(identity.verification_status),
        verification_due: identity.verification_due,
        verification_token: identity.verification_token.clone(),
        // The Easy DKIM setup belongs to the account's identity rather than to this
        // API, so what `VerifyDomainDkim` recorded here is published for every surface
        // over these identities to report: the tokens the owner publishes, where the
        // search for them stands, and whether messages from the identity are signed.
        dkim: identity.dkim.as_ref().map(|dkim| SesDkimSetup {
            signing_enabled: identity.dkim_enabled,
            status: registry_state(dkim.status),
            tokens: dkim.tokens.clone(),
        }),
        // The policies that authorize other principals to send through the identity
        // belong to the account's identity too, and this API attaches and lists them,
        // so the set it holds is published for every surface over these identities to
        // report.
        policies: Some(identity.policies.clone()),
    }
}

/// This service's spelling of a registry verification state.
fn service_status(
    state: ::service_common::ses_identities::SesVerificationState,
) -> VerificationStatus {
    use ::service_common::ses_identities::SesVerificationState;

    match state {
        SesVerificationState::Pending => VerificationStatus::Pending,
        SesVerificationState::Success => VerificationStatus::Success,
        SesVerificationState::Failed => VerificationStatus::Failed,
        SesVerificationState::TemporaryFailure => VerificationStatus::TemporaryFailure,
        SesVerificationState::NotStarted => VerificationStatus::NotStarted,
    }
}

/// The registry's spelling of this service's verification status.
fn registry_state(
    status: VerificationStatus,
) -> ::service_common::ses_identities::SesVerificationState {
    use ::service_common::ses_identities::SesVerificationState;

    match status {
        VerificationStatus::Pending => SesVerificationState::Pending,
        VerificationStatus::Success => SesVerificationState::Success,
        VerificationStatus::Failed => SesVerificationState::Failed,
        VerificationStatus::TemporaryFailure => SesVerificationState::TemporaryFailure,
        VerificationStatus::NotStarted => SesVerificationState::NotStarted,
    }
}
