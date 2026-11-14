// SPDX-License-Identifier: CC0-1.0

//! One stored object: what a `PutObject` recorded, the annotations and the replication
//! state it carries, and what a read of it reports.

use super::*;

/// One object this emulator holds, as a single `PutObject` recorded it.
///
/// Only the members this emulator can report back faithfully are kept: everything a
/// stored object's reader (`GetObject`) answers comes from here, so a member that is not
/// stored is not reported either.
#[derive(Debug, Clone)]
pub struct Object {
    /// The object's bytes, exactly as the request carried them.
    pub body: Vec<u8>,
    /// The version id this stored version is addressed by, or `None` for the `null`
    /// version — a version written while the bucket's versioning status was not `Enabled`.
    ///
    /// Minted by [`ComAmazonawsS3Amazons3State::put_object`], the one place that sees both
    /// the bucket's versioning state and the store's version sequence, so no handler
    /// constructs one: an `Object` handed to that call carries `None` and comes back
    /// stored with whatever the bucket's state called for.
    pub version_id: Option<String>,
    /// The object's entity tag, including the quotes S3 reports it with — see
    /// [`object_etag`].
    pub etag: String,
    /// When the object was last written, which is what `Last-Modified` reports.
    pub last_modified: SystemTime,
    /// A `StorageClass` value; [`DEFAULT_STORAGE_CLASS`] when the request named none.
    pub storage_class: String,
    pub content_type: Option<String>,
    pub cache_control: Option<String>,
    pub content_disposition: Option<String>,
    pub content_encoding: Option<String>,
    pub content_language: Option<String>,
    pub expires: Option<String>,
    pub website_redirect_location: Option<String>,
    /// The user-defined metadata the request carried in its `x-amz-meta-` headers.
    pub metadata: BTreeMap<String, String>,
    /// The object's tag set, in the order the request that set it carried it — the
    /// `x-amz-tagging` header of the write, or the modelled `Tagging` container of a later
    /// `PutObjectTagging`.
    ///
    /// Empty when no tag set is associated with the object: "no tags" and "an empty tag
    /// set" are the same state, which is why `PutObjectTagging` with an empty `TagSet`
    /// leaves an object where `DeleteObjectTagging` would. A `PutObject` REPLACES the
    /// whole object ("You cannot use PutObject to only update a single piece of metadata
    /// for an existing object"), so the tag set of the object it replaces goes with it:
    /// the write's own header decides the new one, and
    /// [`ComAmazonawsS3Amazons3State::replace_object_tags`] is what sets one afterwards.
    pub tags: Vec<ResourceTag>,
    /// How this object is encrypted at rest — see [`ObjectEncryption`]. Decided by the
    /// write that stored it and never afterwards, like every other member here: "You
    /// cannot use PutObject to only update a single piece of metadata for an existing
    /// object."
    pub encryption: ObjectEncryption,
    /// The checksum the write that stored this object computed over the bytes it stored —
    /// see [`ObjectChecksum`] — or `None` when that write named no checksum algorithm,
    /// which is the state an object whose reader reports no checksum at all is in.
    ///
    /// Decided by that write, like [`Self::encryption`]: "this checksum is only present if
    /// the checksum was uploaded with the object" (`GetObjectOutput$ChecksumSHA256`), so
    /// nothing afterwards computes one for an object that was stored without it.
    pub checksum: Option<ObjectChecksum>,
    /// The parts a multipart upload assembled this object from — see [`ObjectPartRecord`] —
    /// or empty for an object one write stored whole.
    ///
    /// "A collection of parts associated with a multipart upload"
    /// (`GetObjectAttributesParts`) is what a read of the object's attributes reports from
    /// this: the parts are gone as parts once the upload is completed (no `ListParts` reaches
    /// them), but the object remembers how it was assembled, which is the only way that
    /// collection can be reported at all.
    pub parts: Vec<ObjectPartRecord>,
    /// The Object Lock retention in force on this stored version — see
    /// [`ObjectRetention`] — or `None` while no retention is associated with it, which is
    /// the state a write leaves a version in.
    ///
    /// Unlike every member above it, this one is set AFTER the write: a retention is
    /// "placed" on an object by `PutObjectRetention` ("Places an Object Retention
    /// configuration on an object"), through
    /// [`ComAmazonawsS3Amazons3State::replace_object_retention`], which is the one place
    /// that sees the byte budgets it has to stay inside.
    pub retention: Option<ObjectRetention>,
    /// The Object Lock LEGAL HOLD on this stored version: `Some(true)` while a hold is in
    /// place, `Some(false)` once one was taken off, and `None` while no legal hold has ever
    /// been applied to it — the state a write leaves a version in.
    ///
    /// Three states rather than two, because "never applied" is one S3 reports apart from
    /// `OFF`: `HeadObject`'s `ObjectLockLegalHoldStatus` "is not returned if the specified
    /// version of this object has never had a legal hold applied"
    /// (`smithy_query(query="doc",shape="HeadObjectOutput",
    /// member="ObjectLockLegalHoldStatus")`), so a reader of a version nobody has placed a
    /// hold on has no status to report.
    ///
    /// Set AFTER the write, like [`Self::retention`]: a legal hold is applied to an object
    /// by `PutObjectLegalHold` ("applies a legal hold configuration to the specified
    /// object"), through
    /// [`ComAmazonawsS3Amazons3State::replace_object_legal_hold`].
    pub legal_hold: Option<bool>,
    /// The restore `RestoreObject` last initiated on this stored version — see
    /// [`ObjectRestore`] — or `None` while no restore has been asked for, which is the
    /// state a write leaves a version in.
    ///
    /// Set AFTER the write, like [`Self::retention`]: a restore is initiated on an
    /// already-archived object ("you must first initiate a restore request, and then wait
    /// until a temporary copy of the object is available", `RestoreObject`'s own
    /// documentation), through
    /// [`ComAmazonawsS3Amazons3State::restore_object_version`], which is the one place that
    /// sees the byte budgets it has to stay inside.
    pub restore: Option<ObjectRestore>,
    /// The annotations attached to this stored object version, by name.
    ///
    /// "An annotation is a named payload of 1 byte to 1 MiB that you can associate with a
    /// specific object or object version" (`PutObjectAnnotation`'s own documentation), so
    /// the annotations of one version live WITH that version and a write of the object does
    /// not carry them forward — "You cannot use PutObject to only update a single piece of
    /// metadata for an existing object", so the object a write replaces takes its
    /// annotations with it, exactly as it takes its tag set.
    ///
    /// Keyed by name, which is how every annotation operation addresses one, and sorted,
    /// because that is the order a listing of them reports. Set AFTER the write, like
    /// [`Self::retention`] and [`Self::restore`], through
    /// [`ComAmazonawsS3Amazons3State::put_object_annotation`] — the one place that sees the
    /// per-object ceiling and the byte budgets an annotation has to stay inside.
    pub annotations: BTreeMap<String, Annotation>,
    /// The grants of the access control list last set on this stored version, or `None`
    /// while no ACL has ever been set on it — the state a write leaves a version in, and
    /// the state whose reader reports the owner's `FULL_CONTROL` alone
    /// ([`default_acl_grants`]), because an object is private until an ACL says otherwise.
    ///
    /// Set AFTER the write, like [`Self::retention`]: an ACL is set on an object by
    /// `PutObjectAcl` ("Uses the acl subresource to set the access control list (ACL)
    /// permissions for … an existing object in an S3 bucket", that operation's own
    /// documentation), through
    /// [`ComAmazonawsS3Amazons3State::replace_object_acl`] — the one place that sees the
    /// byte budgets an ACL has to stay inside.
    ///
    /// The RAW record, kept whatever the bucket's Object Ownership setting is:
    /// [`Bucket::object_acl_grants`] is what a reader reports, and it reports the default
    /// while the bucket's ACLs are disabled — disabling them takes nothing away, so these
    /// grants are reported again once ACLs affect permissions on the bucket.
    pub acl: Option<Vec<AclGrant>>,
    /// What replication did with this stored version — see [`ObjectReplication`] — or
    /// `None` for a version no replication rule covers, which is the state of every object
    /// in a bucket with no replication configuration.
    ///
    /// "If your request involves a bucket that is either a source or a destination in a
    /// replication rule" the reader reports a `ReplicationStatus`
    /// (`GetObjectOutput$ReplicationStatus`); for every other object there is nothing to
    /// report, which is why this is an `Option` and why a reader of an object outside every
    /// rule reports no status at all.
    ///
    /// Settled by the write that stored the version, through
    /// [`ComAmazonawsS3Amazons3State::replicate_write`] — the one place that performs the
    /// replication and records its outcome, so the status describes a copy that was really
    /// made.
    pub replication: Option<ObjectReplication>,
}

/// One annotation attached to one stored object version: the payload
/// `PutObjectAnnotation` recorded under a name, which `GetObjectAnnotation` answers with.
///
/// The name is not here — it is the key the object holds this record under
/// ([`Object::annotations`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Annotation {
    /// The annotation's bytes, exactly as the request carried them.
    pub payload: Vec<u8>,
    /// The annotation's entity tag, including the quotes S3 reports it with — the digest of
    /// its payload, computed the one way every entity tag here is ([`object_etag`]).
    pub etag: String,
    /// When the annotation was last written, which is what a read of it reports as
    /// `Last-Modified` and a listing of them as `AnnotationEntry$LastModified`.
    pub last_modified: SystemTime,
    /// The checksum the write that recorded this payload computed OVER that payload, under
    /// the algorithm the request named, or `None` for a write that named none.
    ///
    /// An annotation payload is checksummed the way a stored object's bytes are — the write
    /// reports the value it computed and a later read reports the STORED one
    /// ([`ObjectChecksum`]) — so `PutObjectAnnotation`'s answer, a `GetObjectAnnotation`
    /// that enabled the checksum mode and the `ChecksumAlgorithm` a listing reports for the
    /// annotation can never disagree.
    pub checksum: Option<ObjectChecksum>,
}

impl Annotation {
    /// The annotation `payload` makes, written at `at` and checksummed as `checksum`: the
    /// payload with the entity tag of those bytes beside it, so no caller derives a tag of
    /// its own.
    pub fn new(payload: Vec<u8>, at: SystemTime, checksum: Option<ObjectChecksum>) -> Self {
        Self {
            etag: object_etag(&payload),
            last_modified: at,
            checksum,
            payload,
        }
    }

    /// The size of the annotation payload, in bytes — what `AnnotationEntry$Size` reports
    /// and what a read of it answers as `Content-Length`.
    pub fn size(&self) -> i64 {
        i64::try_from(self.payload.len()).unwrap_or(i64::MAX)
    }

    /// How many bytes this annotation retains: its payload AND the strings kept beside it —
    /// the entity tag and the recorded checksum — the way [`UploadedPart::retained_bytes`]
    /// counts a part's, so no annotation escapes the budgets a stored object's bytes are
    /// bounded by.
    pub fn retained_bytes(&self) -> usize {
        self.payload.len()
            + self.etag.len()
            + self
                .checksum
                .as_ref()
                .map_or(0, ObjectChecksum::retained_bytes)
    }
}

/// The server-side encryption in force on one stored object: the configuration the write
/// that stored it settled on, which every reader of that object reports back.
///
/// The members are `PutObject`'s own encryption members
/// (`smithy_query(query="operation",operation="PutObject")`): `ServerSideEncryption`
/// (`x-amz-server-side-encryption`), `SSEKMSKeyId`, `SSEKMSEncryptionContext` and
/// `BucketKeyEnabled` — the four `PutObject`, `GetObject` and `HeadObject` all report
/// back as bound response headers.
///
/// A write that names one or more of them is stored under what it named; one that names
/// none inherits the bucket's default encryption configuration
/// ([`Bucket::default_object_encryption`]), which is what makes every object in a bucket
/// encrypted even when no client ever asked.
///
/// What this records is an object's encryption CONFIGURATION, not a key and not a
/// ciphertext: this emulator holds an object's bytes in this process's memory and
/// performs no cryptography, so the only thing it can answer faithfully is which
/// configuration the object was stored under — which is exactly what those response
/// members report. For SSE-C that configuration is the algorithm and the DIGEST of the
/// customer-provided key, which is what S3 itself keeps: "Amazon S3 does not store the
/// encryption key you provide" (`PutObject`'s own documentation of the SSE-C headers), and
/// the digest is what every later read of the object is matched against.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ObjectEncryption {
    /// A `ServerSideEncryption` value (`AES256`, `aws:kms`, …), or `None` for an object
    /// stored before any encryption state was recorded for it.
    pub sse_algorithm: Option<String>,
    /// `SSEKMSKeyId`: for an object stored under SSE-KMS, the ARN of the key it is
    /// encrypted under — the key the write's reference resolved to through KMS
    /// ([`sse_kms::write_key`]), whether that reference was the request's own
    /// `SSEKMSKeyId`, the `KMSMasterKeyID` of the bucket's default encryption configuration
    /// ([`Bucket::default_object_encryption`]) or nothing at all, which is this account's
    /// Amazon Web Services managed key for the service. "You can specify the key ID, key
    /// alias, or the Amazon Resource Name (ARN) of the KMS key" are three spellings of one
    /// key, and what an object records is the key. `None` for an object whose encryption is
    /// not SSE-KMS, and for a reference KMS resolved nothing for.
    pub kms_key_id: Option<String>,
    /// `SSEKMSEncryptionContext`: "a Base64-encoded string holding a UTF-8 encoded JSON
    /// of additional encryption context key-value pairs", verbatim.
    pub kms_encryption_context: Option<String>,
    /// `BucketKeyEnabled`: whether the object uses an S3 Bucket Key for SSE-KMS.
    pub bucket_key_enabled: Option<bool>,
    /// `SSECustomerAlgorithm` — the algorithm a write encrypted the object under with a
    /// CUSTOMER-provided key ([`SSE_CUSTOMER_ALGORITHM`]), and `None` for an object stored
    /// under any other kind of encryption.
    pub sse_customer_algorithm: Option<String>,
    /// `SSECustomerKeyMD5` — the digest of that key, which is the only thing about it S3
    /// keeps and what a later read's own key is matched against
    /// ([`sse_customer::read_refusal`]).
    pub sse_customer_key_md5: Option<String>,
}

impl ObjectEncryption {
    /// Whether this record names NO encryption member at all — what a write that leaves
    /// every one of `PutObject`'s encryption headers off parses to, and exactly the case
    /// where the bucket's own default encryption configuration is what the object is
    /// stored under ([`Bucket::default_object_encryption`]).
    pub fn names_no_member(&self) -> bool {
        self == &Self::default()
    }

    /// Whether this object's encryption is SSE-KMS — the only kind a KMS key id, an
    /// encryption context or an S3 Bucket Key belongs to
    /// (`ApplyServerSideEncryptionByDefault$KMSMasterKeyID`: "This parameter is allowed if
    /// and only if `SSEAlgorithm` is set to `aws:kms` or `aws:kms:dsse`", the two values
    /// [`KMS_SSE_ALGORITHMS`] holds).
    pub fn uses_kms(&self) -> bool {
        self.sse_algorithm
            .as_deref()
            .is_some_and(|algorithm| KMS_SSE_ALGORITHMS.contains(&algorithm))
    }

    /// The header name of the first member here that only SSE-KMS has, or `None` when this
    /// record names none of them: what a caller is told it may not combine with another
    /// algorithm. An S3 Bucket Key counts only when it is switched ON — "Setting this
    /// header to true causes Amazon S3 to use an S3 Bucket Key"
    /// (`PutObjectRequest$BucketKeyEnabled`), so `false` asks for nothing KMS-only.
    pub fn kms_only_member(&self) -> Option<&'static str> {
        if self.kms_key_id.is_some() {
            return Some("x-amz-server-side-encryption-aws-kms-key-id");
        }
        if self.kms_encryption_context.is_some() {
            return Some("x-amz-server-side-encryption-context");
        }
        if self.bucket_key_enabled == Some(true) {
            return Some("x-amz-server-side-encryption-bucket-key-enabled");
        }
        None
    }

    /// Whether an object's own `BucketKeyEnabled` is REPORTED, and as what: `Some(true)` for
    /// an object written with an S3 Bucket Key in force, and `None` for every other object.
    ///
    /// The member is bound to `x-amz-server-side-encryption-bucket-key-enabled` with no
    /// `@default` (`smithy_query(query="operation",operation="HeadObject")`), and "setting
    /// this header to `true` causes Amazon S3 to use an S3 Bucket Key"
    /// (`PutObjectRequest$BucketKeyEnabled`) — so the header states that S3 used one, and an
    /// object written without one carries no such header at all rather than one stating
    /// `false`. "Member absent" and "member false" are different facts about an object, and a
    /// client reads the presence of the header as the decision S3 made for it.
    ///
    /// This is the OBJECT-level projection, and it is the opposite of the BUCKET-level rule:
    /// `ServerSideEncryptionRule$BucketKeyEnabled` is a configuration SETTING with a
    /// documented off state ("General purpose buckets - By default, S3 Bucket Key is not
    /// enabled"), which `GetBucketEncryption` reports as `false` explicitly.
    pub fn reported_bucket_key_enabled(&self) -> Option<bool> {
        self.bucket_key_enabled.filter(|enabled| *enabled)
    }

    /// How many bytes this record retains — the caller-supplied strings it holds, which
    /// the per-bucket and service-wide budgets must count like every other string kept
    /// beside an object.
    pub fn retained_bytes(&self) -> usize {
        [
            &self.sse_algorithm,
            &self.kms_key_id,
            &self.kms_encryption_context,
            &self.sse_customer_algorithm,
            &self.sse_customer_key_md5,
        ]
        .iter()
        .filter_map(|member| member.as_ref())
        .map(String::len)
        .sum()
    }
}

/// The `ObjectLockLegalHoldStatus` value that means a legal hold IS in place on an object
/// version — one of that enum's two values
/// (`smithy_query(query="shape",shape="ObjectLockLegalHoldStatus")`, which
/// [`crate::ops::types::OBJECT_LOCK_LEGAL_HOLD_STATUS_VALUES`] carries).
pub const LEGAL_HOLD_ON_STATUS: &str = "ON";

/// The other `ObjectLockLegalHoldStatus` value: no legal hold is in place on the version.
pub const LEGAL_HOLD_OFF_STATUS: &str = "OFF";

/// The `ObjectLockRetentionMode` value whose retention bars a weakening write unless the
/// caller asks to bypass it — one of that enum's two values
/// (`smithy_query(query="shape",shape="ObjectLockRetentionMode")`, which
/// [`crate::ops::types::OBJECT_LOCK_RETENTION_MODE_VALUES`] carries).
pub const GOVERNANCE_RETENTION_MODE: &str = "GOVERNANCE";

/// The other `ObjectLockRetentionMode` value: the one whose retention nothing may weaken
/// while it is in force, bypass asked for or not.
pub const COMPLIANCE_RETENTION_MODE: &str = "COMPLIANCE";

/// The Object Lock RETENTION in force on one stored object version: the configuration
/// `PutObjectRetention` last placed on it, which `GetObjectRetention` reports back.
///
/// The members are `ObjectLockRetention`'s own
/// (`smithy_query(query="shape",shape="ObjectLockRetention")`): the `Mode` the version is
/// locked in and the `RetainUntilDate` that lock runs to. Both together or neither — "the
/// container element for the Object Retention configuration"
/// (`PutObjectRetentionRequest$Retention`) describes one period in one mode, so a version
/// either carries a retention or carries none, which is why this lives behind an
/// `Option` on [`Object`].
///
/// What it records is a retention SETTING, not an enforcement engine: the instant the
/// period runs to is kept so every reader answers the same date, and
/// [`Self::in_force_at`] is what a write that would weaken it is judged against.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObjectRetention {
    /// An `ObjectLockRetentionMode` value — [`GOVERNANCE_RETENTION_MODE`] or
    /// [`COMPLIANCE_RETENTION_MODE`].
    pub mode: String,
    /// When the retention period runs to: `ObjectLockRetention$RetainUntilDate`, "the date
    /// on which this Object Lock Retention will expire".
    pub retain_until: SystemTime,
}

impl ObjectRetention {
    /// Whether the period is still running at `instant`, which is what makes the retention
    /// a restriction on a later write rather than a record of one that has run out: a
    /// retention "will expire" at its `RetainUntilDate`
    /// (`ObjectLockRetention$RetainUntilDate`), so the instant it names is the first one it
    /// no longer covers.
    pub fn in_force_at(&self, instant: SystemTime) -> bool {
        instant < self.retain_until
    }

    /// Whether this retention protects LESS than `other` does — the comparison a
    /// `PutObjectRetention` over an existing retention is judged by: a period that ends
    /// earlier (shortening it) or a mode that can itself be lifted where the one in force
    /// cannot (`GOVERNANCE` placed over `COMPLIANCE`).
    pub fn weakens(&self, other: &Self) -> bool {
        self.retain_until < other.retain_until
            || (other.mode == COMPLIANCE_RETENTION_MODE && self.mode != COMPLIANCE_RETENTION_MODE)
    }

    /// How many bytes this record retains — the caller-supplied string it holds, which the
    /// per-bucket and service-wide budgets must count like every other string kept beside
    /// an object ([`ObjectEncryption::retained_bytes`] counts its own the same way).
    pub fn retained_bytes(&self) -> usize {
        self.mode.len()
    }
}

/// The request field a `RetainUntilDate` that does not end in the future is reported
/// against: the member of the `Retention` document that carries it
/// (`smithy_query(query="shape",shape="ObjectLockRetention")` reports
/// `RetainUntilDate`), which is how S3's `InvalidArgument` document names the argument it
/// rejected.
pub const RETAIN_UNTIL_DATE_ARGUMENT: &str = "RetainUntilDate";

/// The `<Message>` S3 refuses a retention whose period does not end in the FUTURE with —
/// fixed text, with no variable part.
///
/// A retention period is the span a version is protected FOR, and it "will expire" at the
/// date it names (`ObjectLockRetention$RetainUntilDate`), so a date at or before now
/// describes a period that protects nothing: the value is a bad ARGUMENT rather than a
/// permission the caller lacks, which is why it is refused with
/// [`INVALID_ARGUMENT_CODE`] and HTTP 400 and not with [`ACCESS_DENIED_CODE`].
pub const RETAIN_UNTIL_DATE_NOT_IN_FUTURE_MESSAGE: &str =
    "The retain until date must be in the future!";

/// The request field a `x-amz-object-lock-retain-until-date` a WRITE carries is reported
/// against: the header itself, which is where that write's retention date arrives
/// (`smithy_query(query="operation",operation="PutObject")` reports
/// `ObjectLockRetainUntilDate: {httpHeader: "x-amz-object-lock-retain-until-date"}`).
pub const OBJECT_LOCK_RETAIN_UNTIL_DATE_HEADER: &str = "x-amz-object-lock-retain-until-date";


/// The IAM action that LIFTING a governance-mode retention is authorized under: "to
/// override or remove governance-mode retention settings you must have the
/// `s3:BypassGovernanceRetention` permission" (Amazon S3 User Guide, *S3 Object Lock*),
/// which is what a request carrying `x-amz-bypass-governance-retention` asks the service to
/// do.
pub const BYPASS_GOVERNANCE_RETENTION_ACTION: &str = "s3:BypassGovernanceRetention";

/// How S3 words the 403 [`ACCESS_DENIED_CODE`] of a request a policy in force EXPLICITLY
/// DENIES the caller: who was refused, which IAM `action` on which `resource`, and that it
/// was an explicit deny in the resource's own policy that refused it.
///
/// The same sentence shape every authorization refusal of this crate is worded in
/// ([`block_public_access_denial`] states the guardrail half of it), with the reason clause
/// IAM publishes for this decision rather than for an absent grant
/// (`service_common::iam_authorization::not_authorized_by_resource_policy` states that
/// other one). Every value in it comes from the request and from this emulator's own
/// identity — the principal is the caller ([`caller_principal_arn`]), the action is the one
/// the policy denied and the resource is the one the request addressed — so nothing in it
/// is a recorded value.
pub fn explicit_resource_policy_denial(action: &str, resource: &str) -> String {
    authorization_denial(
        &caller_principal_arn(),
        action,
        resource,
        RESOURCE_POLICY_DENY_REASON,
    )
}

/// The reason clause IAM publishes for a request the RESOURCE's own policy explicitly denies
/// — see [`explicit_resource_policy_denial`], and
/// [`crate::service::request_authorization`] for the identity-based half of the same
/// decision.
pub const RESOURCE_POLICY_DENY_REASON: &str = "with an explicit deny in a resource-based policy";

/// The ONE sentence every authorization refusal of this service is worded in: who was
/// refused (`principal`), which IAM `action`, on which `resource`, and the `reason` clause
/// saying what refused it — an explicit deny in one of the documents in force
/// ([`RESOURCE_POLICY_DENY_REASON`]), the absence of any grant, or a Block Public Access
/// setting ([`block_public_access_denial`]).
///
/// One place, so every refusal of this crate reads alike and only the clause that names the
/// reason differs. Every value in it comes from the request and from the policies in force:
/// the principal from the credential the request presented, the action from what the request
/// asked to do and the resource from the bucket or object it addressed.
pub fn authorization_denial(principal: &str, action: &str, resource: &str, reason: &str) -> String {
    format!(
        "User: {principal} is not authorized to perform: {action} on resource: \
         \"{resource}\" {reason}"
    )
}

/// [`authorization_denial`] for an action that addresses NO resource of its own: who was
/// refused, which IAM `action`, and the `reason` clause saying what refused it.
///
/// Such an action is published with no resource types at all — `s3:ListAllMyBuckets` is the
/// account's own listing rather than a call on one bucket — so the refusal names no resource:
/// there is nothing for the `on resource:` clause to carry, and quoting one would name a
/// resource the request never addressed. Every other word is the sentence above.
pub fn account_wide_authorization_denial(principal: &str, action: &str, reason: &str) -> String {
    format!("User: {principal} is not authorized to perform: {action} {reason}")
}

/// The Object Lock state a read of one stored object version REPORTS: the three
/// `x-amz-object-lock-*` headers `GetObject` and `HeadObject` both declare, as
/// [`Object::reported_object_lock`] renders them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReportedObjectLock {
    /// `ObjectLockMode` — an [`ObjectRetention::mode`] value, for a version carrying a
    /// retention.
    pub mode: Option<String>,
    /// `ObjectLockRetainUntilDate` — the instant that retention runs to.
    pub retain_until: Option<SystemTime>,
    /// `ObjectLockLegalHoldStatus` — [`LEGAL_HOLD_ON_STATUS`] or [`LEGAL_HOLD_OFF_STATUS`]
    /// for a version a legal hold has been applied to, and nothing for one that never has.
    pub legal_hold_status: Option<String>,
}

/// The Object Lock a WRITE asked for, as [`requested_object_lock`] read it off the three
/// `x-amz-object-lock-*` members every object write declares.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RequestedObjectLock {
    /// The retention the write named, for [`Object::retention`] — `None` when it named
    /// neither a mode nor a date, in which case the bucket's own default retention is what
    /// locks the version ([`Bucket::put_object`] applies it).
    pub retention: Option<ObjectRetention>,
    /// The legal hold the write named, for [`Object::legal_hold`] — `None` when it named no
    /// status, which leaves the version with no hold ever applied.
    pub legal_hold: Option<bool>,
}

impl RequestedObjectLock {
    /// Whether the write named any Object Lock member at all — what decides whether the
    /// bucket has to carry an Object Lock configuration for it to be served.
    pub fn names_a_member(&self) -> bool {
        self.retention.is_some() || self.legal_hold.is_some()
    }
}

/// Why the Object Lock a write asked for cannot be recorded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObjectLockRequestRejection {
    /// One of `x-amz-object-lock-mode` and `x-amz-object-lock-retain-until-date` without the
    /// other: a retention runs in a mode TO a date, so half of one describes no period.
    IncompleteRetention,
    /// A `x-amz-object-lock-retain-until-date` outside the range of instants this platform
    /// can hold, which no period a version is locked for can run to.
    UnrepresentableRetainUntilDate,
    /// A `x-amz-object-lock-retain-until-date` that is not in the FUTURE, which describes a
    /// period that protects nothing — see [`RETAIN_UNTIL_DATE_NOT_IN_FUTURE_MESSAGE`].
    RetainUntilDateNotInFuture,
}

/// The Object Lock an object write asked for, read off the three members that carry it, or
/// why it cannot be recorded.
///
/// "The Object Lock mode that you want to apply to this object" with "the date and time when
/// you want this object's Object Lock to expire" beside it
/// (`PutObjectRequest$ObjectLockMode` and `$ObjectLockRetainUntilDate`), and "specifies
/// whether a legal hold will be applied to this object"
/// (`$ObjectLockLegalHoldStatus`) — so a write that names them stores them ON the version it
/// writes, which is what a later delete of that version is judged against
/// ([`Bucket::object_lock_delete_refusal`]), and `PutObjectRetention` /
/// `PutObjectLegalHold` are what change them afterwards.
///
/// ONE place for the rule, so every write that declares these members — the one-shot write,
/// the copy and the multipart initiation whose settings the completed object carries —
/// records the same lock from the same values.
/// `now` is the instant the period is judged against: a retention "will expire" at the date
/// it names, so one that does not end after `now` protects nothing and is refused rather
/// than recorded ([`RETAIN_UNTIL_DATE_NOT_IN_FUTURE_MESSAGE`] is the sentence, the same rule
/// `PutObjectRetention` judges its own `RetainUntilDate` by).
pub fn requested_object_lock(
    mode: Option<&str>,
    retain_until_date: Option<&::aws_smithy_types::DateTime>,
    legal_hold_status: Option<&str>,
    now: SystemTime,
) -> Result<RequestedObjectLock, ObjectLockRequestRejection> {
    let retention = match (mode, retain_until_date) {
        (None, None) => None,
        (Some(mode), Some(date)) => {
            let Some(retain_until) = instant_of(date) else {
                return Err(ObjectLockRequestRejection::UnrepresentableRetainUntilDate);
            };
            if retain_until <= now {
                return Err(ObjectLockRequestRejection::RetainUntilDateNotInFuture);
            }
            Some(ObjectRetention {
                mode: mode.to_string(),
                retain_until,
            })
        }
        _ => return Err(ObjectLockRequestRejection::IncompleteRetention),
    };
    Ok(RequestedObjectLock {
        retention,
        // "ON" is a hold in place and "OFF" is one that is not; a write naming no status
        // leaves the version with none ever applied, which is a third state a reader reports
        // apart from `OFF` (see [`Object::legal_hold`]).
        legal_hold: legal_hold_status.map(|status| status == LEGAL_HOLD_ON_STATUS),
    })
}

/// The one value the `ObjectLockEnabled` enum publishes
/// (`smithy_query(query="shape",shape="ObjectLockEnabled")`, whose values
/// [`crate::ops::types::OBJECT_LOCK_ENABLED_VALUES`] carries): "indicates whether this
/// bucket has an Object Lock configuration enabled"
/// (`ObjectLockConfiguration$ObjectLockEnabled`).
///
/// What a reader of a bucket's Object Lock configuration reports for it, and the only value
/// a put of one may name — there is no second state for that member to be in, which is why
/// [`Bucket::object_lock_enabled`] keeps it as a `bool`.
pub const OBJECT_LOCK_ENABLED_STATUS: &str = "Enabled";

/// How many days a default retention period of one YEAR runs for.
///
/// A default retention period is "the number of days" or "the number of years that you want
/// to specify for the default retention period" (`DefaultRetention$Days` and
/// `DefaultRetention$Years`) counted from the object's own creation time. The API publishes
/// no calendar for the second of those, so a year is this many days here wherever it falls.
pub(super) const DAYS_PER_RETENTION_YEAR: i64 = 365;

/// The longest default retention period a rule may name, in YEARS: "you can set a retention
/// period of up to 100 years" (Amazon S3 User Guide, *S3 Object Lock*).
///
/// A put naming a longer period is refused rather than recorded, so every period a bucket
/// carries is one an object written into it can really be locked for.
pub const MAX_DEFAULT_RETENTION_YEARS: i32 = 100;

/// The same ceiling in DAYS, the other unit a period may be given in: derived from
/// [`MAX_DEFAULT_RETENTION_YEARS`] through [`DAYS_PER_RETENTION_YEAR`], so the two bounds
/// cannot drift apart.
pub const MAX_DEFAULT_RETENTION_DAYS: i32 =
    MAX_DEFAULT_RETENTION_YEARS * DAYS_PER_RETENTION_YEAR as i32;

/// The PERIOD a bucket's default Object Lock retention runs for.
///
/// "The DefaultRetention period can be either Days or Years but you must select one. You
/// cannot specify Days and Years at the same time" (`PutObjectLockConfiguration`'s own
/// documentation), so this is one of the two and never both nor neither — which a pair of
/// `Option`s could not say.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DefaultRetentionPeriod {
    /// `DefaultRetention$Days` — "the number of days that you want to specify for the
    /// default retention period".
    Days(i32),
    /// `DefaultRetention$Years` — "the number of years that you want to specify for the
    /// default retention period".
    Years(i32),
}

impl DefaultRetentionPeriod {
    /// Whether the period is one a rule may name: it has to run FORWARD from the write it
    /// locks, so a count of zero or fewer is no period at all, and no further than the
    /// published ceiling ([`MAX_DEFAULT_RETENTION_YEARS`]).
    ///
    /// The one place that judgement lives, so what a bucket carries is always a period an
    /// object written into it can be locked for — the write path never has to drop a lock it
    /// cannot compute.
    pub fn is_within_published_bounds(self) -> bool {
        match self {
            Self::Days(days) => (1..=MAX_DEFAULT_RETENTION_DAYS).contains(&days),
            Self::Years(years) => (1..=MAX_DEFAULT_RETENTION_YEARS).contains(&years),
        }
    }

    /// How many days the period runs for — a year counted as [`DAYS_PER_RETENTION_YEAR`] of
    /// them — or `None` when that product leaves the range this type can hold.
    pub(super) fn days(self) -> Option<i64> {
        match self {
            Self::Days(days) => Some(i64::from(days)),
            Self::Years(years) => i64::from(years).checked_mul(DAYS_PER_RETENTION_YEAR),
        }
    }

    /// How many seconds the period runs for, or `None` when the arithmetic leaves the range
    /// this type can hold.
    pub(super) fn seconds(self) -> Option<i64> {
        self.days()?.checked_mul(SECONDS_PER_DAY)
    }
}

/// The DEFAULT RETENTION a bucket's Object Lock configuration carries: the retention every
/// new object placed in the bucket is locked under.
///
/// The members are `DefaultRetention`'s own
/// (`smithy_query(query="shape",shape="DefaultRetention")`) less its event hold, which
/// nothing in this emulator carries out: the `Mode` new objects are locked in and the period
/// that lock runs for. "The DefaultRetention settings require both a mode and a period"
/// (`PutObjectLockConfiguration`'s own documentation), so the pair is ONE value here rather
/// than two members that could disagree, and a bucket either carries a default retention or
/// carries none — which is why this lives behind an `Option` on [`Bucket`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BucketDefaultRetention {
    /// An `ObjectLockRetentionMode` value — [`GOVERNANCE_RETENTION_MODE`] or
    /// [`COMPLIANCE_RETENTION_MODE`].
    pub mode: String,
    /// How long after it is written an object stays locked.
    pub period: DefaultRetentionPeriod,
}

impl BucketDefaultRetention {
    /// The retention an object written at `written_at` is locked under by this rule.
    ///
    /// `None` only for a date outside the range this platform can hold, which a stored rule
    /// cannot reach: [`DefaultRetentionPeriod::is_within_published_bounds`] is what every put
    /// of a configuration passes through, so the period here is at most the published
    /// ceiling.
    ///
    /// "The rule specified in the Object Lock configuration will be applied by default to
    /// every new object placed in the specified bucket"
    /// (`PutObjectLockConfiguration`'s own documentation), and the period is counted from
    /// the object's own creation time, so the date each version is locked to is its own
    /// rather than a property of the bucket.
    pub fn retention_at(&self, written_at: SystemTime) -> Option<ObjectRetention> {
        let seconds = u64::try_from(self.period.seconds()?).ok()?;
        Some(ObjectRetention {
            mode: self.mode.clone(),
            retain_until: written_at.checked_add(std::time::Duration::from_secs(seconds))?,
        })
    }
}

/// How long after the `RestoreObject` that initiated it a temporary copy of an archived
/// object becomes available in this emulator.
///
/// NOT a fidelity claim about retrieval time: a real Glacier retrieval takes minutes to
/// hours depending on the tier, which no emulated workflow could wait for. What IS faithful
/// is that the copy is NOT there when the call returns — "you must first initiate a restore
/// request, and then wait until a temporary copy of the object is available"
/// (`RestoreObject`'s own documentation) — so the restore is reported as ongoing for a
/// span, and a caller reading the object's state straight afterwards sees
/// `ongoing-request="true"` exactly as it would against AWS. The instant it falls due is
/// stored ([`ObjectRestore::available_at`]) and nothing sleeps for it.
pub const RESTORE_COMPLETION_DELAY: std::time::Duration = std::time::Duration::from_millis(500);
/// The restore in force on one stored object version: what the last `RestoreObject` on it
/// asked for, and when the temporary copy it initiated falls due.
///
/// The members are `RestoreRequest`'s own
/// (`smithy_query(query="shape",shape="RestoreRequest")`): the `Days` the active copy lives
/// for and the `Tier` the retrieval is processed at. A restore is one job on one version —
/// "To restore a specific object version, you can provide a version ID. If you don't
/// provide a version ID, Amazon S3 restores the current version" — so a version either
/// carries one or carries none, which is why this lives behind an `Option` on [`Object`].
///
/// Nothing here is a copy of the object: the archived bytes are the ones the store already
/// holds. What it records is the STATE of the restore, which is what every reader of the
/// object reports through its `x-amz-restore` header ([`Self::header_value`]): ongoing
/// until the copy falls due, then available until it expires.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObjectRestore {
    /// A `Tier` value — the retrieval tier the restore is processed at
    /// ([`DEFAULT_RESTORE_TIER`] when the request named none).
    pub tier: String,
    /// "Lifetime of the active copy in days" (`RestoreRequest$Days`): how long the
    /// temporary copy lives once it is available.
    pub days: i32,
    /// When the temporary copy falls due — the instant the restore stops being reported as
    /// ongoing, [`RESTORE_COMPLETION_DELAY`] after the call that initiated it.
    pub available_at: SystemTime,
}

impl ObjectRestore {
    /// Whether the retrieval is still running at `instant`: "If the object restoration is
    /// in progress, the header returns the value `ongoing-request="true"`"
    /// (`HeadObjectOutput$Restore`).
    pub fn in_progress_at(&self, instant: SystemTime) -> bool {
        instant < self.available_at
    }

    /// When Amazon S3 is scheduled to delete the temporary copy: [`Self::days`] days after
    /// it became available, rounded up to midnight UTC the way every S3 lifetime date is
    /// ([`midnight_after_days`]), or `None` when that instant is outside the range this
    /// platform can hold.
    pub fn expires_at(&self) -> Option<SystemTime> {
        midnight_after_days(self.available_at, self.days)
    }

    /// That expiry as the HTTP date the `Restore` member reports it as, or `None` when the
    /// lifetime names an instant no date this store can render falls on.
    ///
    /// The one place the date is rendered, so what a reader reports and what
    /// `RestoreObject` checks a lifetime against cannot drift: a restore whose expiry
    /// cannot be rendered is refused rather than recorded, which is what keeps every
    /// recorded restore reportable.
    pub fn expiry_date(&self) -> Option<String> {
        ::aws_smithy_types::DateTime::from(self.expires_at()?)
            .fmt(::aws_smithy_types::date_time::Format::HttpDate)
            .ok()
    }

    /// This restore as the `Restore` response member carries it at `instant`, or `None`
    /// once the temporary copy has expired (the version is archived again, and the header
    /// is reported only "if either the archive restoration is in progress … or an archive
    /// copy is already restored").
    ///
    /// The two documented renderings, verbatim from `HeadObjectOutput$Restore`:
    /// `ongoing-request="true"` while the retrieval runs, and
    /// `ongoing-request="false", expiry-date="Fri, 21 Dec 2012 00:00:00 GMT"` once the copy
    /// is there — the expiry date as an HTTP date, the way [`ObjectExpiration::header_value`]
    /// renders its own.
    pub fn header_value(&self, instant: SystemTime) -> Option<String> {
        if self.in_progress_at(instant) {
            return Some(String::from("ongoing-request=\"true\""));
        }
        let expires_at = self.expires_at()?;
        if instant >= expires_at {
            return None;
        }
        let expiry_date = self.expiry_date()?;
        Some(format!(
            "ongoing-request=\"false\", expiry-date=\"{expiry_date}\""
        ))
    }

    /// How many bytes this record retains — the caller-supplied string it holds, which the
    /// per-bucket and service-wide budgets must count like every other string kept beside
    /// an object ([`ObjectRetention::retained_bytes`] counts its own the same way).
    pub fn retained_bytes(&self) -> usize {
        self.tier.len()
    }
}

/// The `Tier` a restore is processed at when the request names none.
///
/// From `RestoreObject`'s own documentation: "Standard - Standard retrievals allow you to
/// access any of your archived objects within several hours. This is the default option for
/// retrieval requests that do not specify the retrieval option." `Standard` is one of the
/// `Tier` enum's values (`smithy_query(query="shape",shape="Tier")`).
pub const DEFAULT_RESTORE_TIER: &str = "Standard";

/// The `ReplicationStatus` a SOURCE object version carries once its replication is done:
/// every rule that covers it copied it to its destination.
///
/// One of the values the enum publishes
/// (`smithy_query(query="shape",shape="com.amazonaws.s3#ReplicationStatus")` reports
/// `[COMPLETE, PENDING, FAILED, REPLICA, COMPLETED]`).
pub const REPLICATION_STATUS_COMPLETED: &str = "COMPLETED";

/// The `ReplicationStatus` a source object version carries while the replication its
/// bucket's configuration calls for has not been reported done yet.
pub const REPLICATION_STATUS_PENDING: &str = "PENDING";

/// The `ReplicationStatus` a source object version carries when the replication a rule
/// calls for could not be performed — a destination this emulator cannot write.
pub const REPLICATION_STATUS_FAILED: &str = "FAILED";

/// The `ReplicationStatus` the COPY carries: "the object in the destination bucket is a
/// replica" (`HeadObjectOutput$ReplicationStatus`), which is a finished object from the
/// moment it is written.
pub const REPLICATION_STATUS_REPLICA: &str = "REPLICA";

/// How long after the write that started it a replication is reported done.
///
/// Replication is asynchronous — "Amazon S3 replicates objects asynchronously"
/// (`PutBucketReplication`'s own documentation), with no published duration — so the span a
/// source object spends in [`REPLICATION_STATUS_PENDING`] is this emulator's own, chosen
/// short enough that a caller polling the status sees it settle and long enough that a
/// caller reading straight after the write still sees the transient state.
pub const REPLICATION_SETTLES_AFTER: Duration = Duration::from_secs(3);

/// What replication did with one stored object version: the status it settles on, and when
/// that status is reported.
///
/// The copy itself is made by the write that recorded the version
/// ([`ComAmazonawsS3Amazons3State::replicate_write`]), so this records an outcome rather
/// than a promise; what the clock still owes is only the moment the source version stops
/// reporting [`REPLICATION_STATUS_PENDING`] and reports that outcome
/// ([`Self::status_at`]) — the asynchrony AWS's own replication has.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObjectReplication {
    /// The status this version settles on: [`REPLICATION_STATUS_COMPLETED`] or
    /// [`REPLICATION_STATUS_FAILED`] for a source version, and
    /// [`REPLICATION_STATUS_REPLICA`] for the copy.
    pub settled: String,
    /// The instant from which [`Self::status_at`] reports [`Self::settled`]; before it, a
    /// source version reports [`REPLICATION_STATUS_PENDING`]. The write's own instant for a
    /// replica, which is finished the moment it exists.
    pub settles_at: SystemTime,
}

impl ObjectReplication {
    /// The outcome `settled` of a SOURCE version written at `at`, reported from
    /// [`REPLICATION_SETTLES_AFTER`] past that write.
    pub fn source(settled: &str, at: SystemTime) -> Self {
        Self {
            settled: settled.to_string(),
            settles_at: at + REPLICATION_SETTLES_AFTER,
        }
    }

    /// The replica written at `at` — [`REPLICATION_STATUS_REPLICA`] from the moment it is
    /// stored, because the copy is what the status describes and it is already there.
    pub fn replica(at: SystemTime) -> Self {
        Self {
            settled: REPLICATION_STATUS_REPLICA.to_string(),
            settles_at: at,
        }
    }

    /// The `ReplicationStatus` a reader at `now` reports for this version:
    /// [`REPLICATION_STATUS_PENDING`] until [`Self::settles_at`], and the settled outcome
    /// from then on.
    ///
    /// A pure function of the clock: a read reports it and writes nothing.
    pub fn status_at(&self, now: SystemTime) -> &str {
        if now < self.settles_at {
            REPLICATION_STATUS_PENDING
        } else {
            &self.settled
        }
    }

    /// How many bytes this record retains — the status string kept beside the object, which
    /// the per-bucket and service-wide budgets count like every other string an object
    /// holds.
    pub fn retained_bytes(&self) -> usize {
        self.settled.len()
    }
}


impl Object {
    /// A freshly written object of `body`, as a write that names nothing about it beyond its
    /// content type stores it.
    ///
    /// A convenience for a writer that carries only bytes — the data plane of another service
    /// of this emulator delivering into a bucket — so it does not restate this struct's whole
    /// member list: the entity tag is computed from the bytes the object is stored with, the
    /// storage class is [`DEFAULT_STORAGE_CLASS`], the version id is minted by
    /// [`ComAmazonawsS3Amazons3State::put_object`] rather than here, and the members a later
    /// call places on a stored version — its tag set, its Object Lock retention and legal
    /// hold, its restore, its annotations and its ACL — are unset, which is the state a write
    /// leaves a version in. S3's
    /// own `PutObject` and `CopyObject` build the object they store themselves, because they
    /// carry the cache-control, metadata, tag set and encryption configuration of the request.
    pub fn stored(body: Vec<u8>, content_type: Option<String>) -> Self {
        Self {
            etag: object_etag(&body),
            body,
            version_id: None,
            last_modified: SystemTime::now(),
            storage_class: DEFAULT_STORAGE_CLASS.to_string(),
            content_type,
            cache_control: None,
            content_disposition: None,
            content_encoding: None,
            content_language: None,
            expires: None,
            website_redirect_location: None,
            metadata: BTreeMap::new(),
            tags: Vec::new(),
            encryption: ObjectEncryption::default(),
            retention: None,
            legal_hold: None,
            restore: None,
            annotations: BTreeMap::new(),
            acl: None,
            // A writer that carries only bytes names no checksum algorithm, so the object
            // it stores carries no checksum either.
            checksum: None,
            // Nor is it assembled from parts: one write stored it whole.
            parts: Vec::new(),
            // Replication is settled by the store once the version is recorded (see
            // [`ComAmazonawsS3Amazons3State::replicate_write`]), not by the writer.
            replication: None,
        }
    }

    /// How many bytes this stored object retains: its payload AND every string kept
    /// beside it (its entity tag, storage class, the response headers it carries, its
    /// user-defined metadata and its tag set).
    ///
    /// This, not the payload alone, is what the per-bucket and service-wide budgets are
    /// computed from: a one-byte object with large metadata or a large
    /// `Content-Disposition` retains just as much of this process's memory as a large
    /// payload does, so none of it may escape the ceilings.
    pub fn retained_bytes(&self) -> usize {
        self.body.len()
            + self.etag.len()
            + self.storage_class.len()
            + self.version_id.as_ref().map_or(0, String::len)
            + self.metadata_bytes()
            + tag_set_bytes(&self.tags)
            + self.annotation_bytes()
            + acl_grants_bytes(self.acl.as_deref().unwrap_or_default())
            + self.encryption.retained_bytes()
            + self
                .checksum
                .as_ref()
                .map_or(0, ObjectChecksum::retained_bytes)
            + self
                .parts
                .iter()
                .map(ObjectPartRecord::retained_bytes)
                .sum::<usize>()
            + self
                .retention
                .as_ref()
                .map_or(0, ObjectRetention::retained_bytes)
            + self
                .restore
                .as_ref()
                .map_or(0, ObjectRestore::retained_bytes)
            + self
                .replication
                .as_ref()
                .map_or(0, ObjectReplication::retained_bytes)
            + [
                &self.content_type,
                &self.cache_control,
                &self.content_disposition,
                &self.content_encoding,
                &self.content_language,
                &self.expires,
                &self.website_redirect_location,
            ]
            .iter()
            .filter_map(|header| header.as_ref())
            .map(String::len)
            .sum::<usize>()
    }

    /// How many bytes this object's annotations retain: each one's name, with the payload
    /// and entity tag of the annotation held under it ([`Annotation::retained_bytes`]).
    ///
    /// Counted by [`Self::retained_bytes`] like every other string kept beside an object,
    /// so an annotation is bounded by the same per-bucket and service-wide budgets the
    /// object's own bytes are.
    pub fn annotation_bytes(&self) -> usize {
        self.annotations
            .iter()
            .map(|(name, annotation)| name.len() + annotation.retained_bytes())
            .sum()
    }

    /// The annotation attached to this object under `name`, or `None` when it carries none
    /// — which is what makes a read or a delete naming a name this object does not hold
    /// answer `NoSuchAnnotation`.
    pub fn annotation(&self, name: &str) -> Option<&Annotation> {
        self.annotations.get(name)
    }

    /// How many tags this object's tag set carries, or `None` when it carries none —
    /// what `GetObject`'s and `HeadObject`'s `TagCount` reports: "The number of tags, if
    /// any, on the object", so an object with no tags reports no count rather than a zero.
    ///
    /// ONE place, so the two readers of this member cannot drift. The count fits an `i32`
    /// for any tag set this store accepts ([`validate_tag_set`] bounds it at
    /// [`MAX_TAGS_PER_OBJECT`]), and a set that somehow grew past that reports the largest
    /// count the member can carry rather than a wrapped one.
    pub fn tag_count(&self) -> Option<i32> {
        (!self.tags.is_empty()).then(|| i32::try_from(self.tags.len()).unwrap_or(i32::MAX))
    }

    /// How large this object's user-defined metadata is, in UTF-8 bytes summed over every
    /// key and value — what [`MAX_OBJECT_METADATA_BYTES`] bounds.
    pub fn metadata_bytes(&self) -> usize {
        self.metadata
            .iter()
            .map(|(key, value)| key.len() + value.len())
            .sum()
    }

    /// The Object Lock state a read of this stored version reports — see
    /// [`ReportedObjectLock`].
    ///
    /// "The Object Lock mode, if any, that's in effect for this object" and "the date and
    /// time when the Object Lock retention period expires"
    /// (`GetObjectOutput$ObjectLockMode` / `$ObjectLockRetainUntilDate`) are the retention
    /// `PutObjectRetention` placed on THIS version, reported as the record stands — the
    /// same record `GetObjectRetention` reports, so the readers of one retention cannot
    /// disagree about it. The legal hold status "is not returned if the specified version
    /// of this object has never had a legal hold applied"
    /// (`GetObjectOutput$ObjectLockLegalHoldStatus`), so a version nobody has placed one on
    /// reports nothing rather than [`LEGAL_HOLD_OFF_STATUS`].
    ///
    /// ONE place, so `GetObject` and `HeadObject` report one version's lock identically.
    pub fn reported_object_lock(&self) -> ReportedObjectLock {
        ReportedObjectLock {
            mode: self
                .retention
                .as_ref()
                .map(|retention| retention.mode.clone()),
            retain_until: self.retention.as_ref().map(|retention| retention.retain_until),
            legal_hold_status: self.legal_hold.map(|held| {
                if held {
                    LEGAL_HOLD_ON_STATUS.to_string()
                } else {
                    LEGAL_HOLD_OFF_STATUS.to_string()
                }
            }),
        }
    }

    /// Whether this object's bytes are ARCHIVED at `instant` — in a storage class that
    /// holds no copy a read can serve, with no restored temporary copy available.
    ///
    /// "The storage class of the object can't be `GLACIER` or `DEEP_ARCHIVE` unless you first
    /// restore a copy of the object" (`InvalidObjectState`'s own condition, from
    /// `GetObject`'s documentation of it): the two classes [`ARCHIVED_STORAGE_CLASSES`]
    /// names, and the restore that lifts the bar is the one `RestoreObject` initiated on THIS
    /// version — in force only once the temporary copy has fallen due
    /// ([`ObjectRestore::header_value`] reports the same three states) and only until it
    /// expires, after which the version is archived again.
    pub fn archived_at(&self, instant: SystemTime) -> bool {
        if !ARCHIVED_STORAGE_CLASSES.contains(&self.storage_class.as_str()) {
            return false;
        }
        self.restore
            .as_ref()
            .is_none_or(|restore| restore.in_progress_at(instant) || restore.header_value(instant).is_none())
    }

    /// How many parts a multipart upload assembled this object from, or `None` for an object
    /// one write stored whole.
    ///
    /// What a part-scoped read reports as `PartsCount`: "the count of parts this object has.
    /// This value is only returned if you specify `partNumber` in your request and the object
    /// was uploaded as a multipart upload" (`GetObjectOutput$PartsCount`) — so an object that
    /// was NOT uploaded as a multipart upload reports no count, whatever the read asked for.
    pub fn parts_count(&self) -> Option<i32> {
        (!self.parts.is_empty())
            .then(|| i32::try_from(self.parts.len()).unwrap_or(i32::MAX))
    }

    /// The span of this object's bytes that `part_number` names, or `None` when the object
    /// has no such part.
    ///
    /// A part-scoped read "effectively performs a 'ranged' GET request for the part
    /// specified" (`GetObjectRequest$PartNumber`), so the span is the part's own place in the
    /// object: the parts were concatenated "in ascending order by part number"
    /// (`CompleteMultipartUpload`'s own documentation), which makes a part's first byte the
    /// sum of the sizes before it.
    ///
    /// An object stored by ONE write has a single part — the whole object — because that is
    /// what a part-scoped read of it answers; it reports no `PartsCount`
    /// ([`Self::parts_count`]) all the same, since it was not uploaded as a multipart upload.
    /// A part holding no bytes names no span, because `Content-Range` cannot state one.
    pub fn part_span(&self, part_number: i32) -> Option<ByteSpan> {
        if self.parts.is_empty() {
            return (part_number == MIN_PART_NUMBER)
                .then(|| ByteSpan::new(0, self.body.len().checked_sub(1)?))
                .flatten();
        }
        let mut first = 0usize;
        for part in &self.parts {
            let size = usize::try_from(part.size).unwrap_or(0);
            if part.part_number == part_number {
                return ByteSpan::new(first, first + size.checked_sub(1)?);
            }
            first += size;
        }
        None
    }

    /// The checksum a response carrying `span` of this object reports — `None` for a span
    /// no checksum of this store's describes, and for an object written under no checksum
    /// algorithm at all.
    ///
    /// Every `Checksum*` member is "the Base64 encoded … checksum of the object" (each
    /// member's own documentation), so the value a response advertises has to be the digest
    /// of the bytes that response CARRIES: a reader validating it recomputes the digest over
    /// the body it received. So
    ///
    /// * a read of the whole object (`span` of `None`, or a span covering every byte)
    ///   reports the checksum the write recorded;
    /// * a read narrowed to exactly one part of the object reports THAT part's own digest,
    ///   which the completion recorded beside it ([`ObjectPartRecord::checksum`]), under the
    ///   object's algorithm;
    /// * any other narrowed read reports no checksum, because this store holds no digest of
    ///   those bytes and the object's own would describe a body the response does not carry.
    ///
    /// ONE place for the rule, so a GET and a HEAD of the same span cannot advertise
    /// different checksums.
    pub fn checksum_of_span(&self, span: Option<ByteSpan>) -> Option<ObjectChecksum> {
        let stored = self.checksum.as_ref()?;
        let Some(span) = span else {
            return Some(stored.clone());
        };
        if span.first() == 0 && span.len() >= self.body.len() {
            return Some(stored.clone());
        }
        let part = self
            .parts
            .iter()
            .find(|part| self.part_span(part.part_number) == Some(span))?;
        Some(ObjectChecksum {
            algorithm: stored.algorithm.clone(),
            value: part.checksum.clone()?,
            // The object's own type, which "determines how part-level checksums are
            // combined to create an object-level checksum for multipart objects"
            // (`GetObjectOutput$ChecksumType`) — a property of the object, not of the span
            // this response carries of it.
            checksum_type: stored.checksum_type.clone(),
        })
    }
}
