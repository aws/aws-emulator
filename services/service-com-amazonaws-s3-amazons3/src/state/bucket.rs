// SPDX-License-Identifier: CC0-1.0

//! One general purpose bucket: the record every bucket-scoped operation reads and writes,
//! with the configurations, the object versions and the uploads it holds.

use super::*;

/// One general purpose bucket this emulator holds.
#[derive(Debug, Clone, Default)]
pub struct Bucket {
    /// The Region the bucket lives in — what `HeadBucket` reports as
    /// `x-amz-bucket-region` and what decides the form of `CreateBucket`'s `Location`.
    pub region: String,
    /// When the bucket was created: the instant
    /// [`ComAmazonawsS3Amazons3State::create_bucket`] recorded it, which is what
    /// `Bucket$CreationDate` reports for a bucket of this account — whether the reader is
    /// S3's own `ListBuckets` or another service that reports the account's buckets.
    ///
    /// Private and never reassigned, so the value a reader sees does not move while the
    /// bucket exists. `None` only in a `Default`-built value, which is never a bucket this
    /// store holds: `create_bucket` is the one place a bucket comes into being and it
    /// always records the instant. A reader reports no creation date rather than inventing
    /// one.
    pub(super) created_at: Option<SystemTime>,
    /// The bucket policy `PutBucketPolicy` attached, verbatim, or `None` while the
    /// bucket has none. A bucket is created WITHOUT a policy, which is what makes
    /// `GetBucketPolicy` answer `NoSuchBucketPolicy` until one is put.
    pub policy: Option<String>,
    /// The versioning state `PutBucketVersioning` last set on the bucket, or `None`
    /// while the bucket has no versioning state — the state a bucket is created in, and
    /// what makes `GetBucketVersioning` report no `Status` until a put sets one.
    pub versioning: Option<BucketVersioning>,
    /// Whether S3 Object Lock is enabled on the bucket — "whether this bucket has an
    /// Object Lock configuration enabled" (`ObjectLockConfiguration$ObjectLockEnabled`,
    /// whose enum has the single value `Enabled`).
    ///
    /// A `bool` rather than an `Option<String>` because that enum names ONE state: a
    /// bucket either has Object Lock enabled or has no Object Lock configuration at all,
    /// and there is no second value for an `Option` to distinguish. `false` is the state
    /// every bucket is created in unless the create asks for it
    /// (`CreateBucketRequest$ObjectLockEnabledForBucket`, "specifies whether you want S3
    /// Object Lock to be enabled for the new bucket"), and it is what makes an object-level
    /// Object Lock call on the bucket a request S3 cannot carry out
    /// ([`OBJECT_LOCK_CONFIGURATION_MISSING_MESSAGE`]).
    ///
    /// Private, like `tags`, so the accessor pair below is the only way in or out.
    pub(super) object_lock_enabled: bool,
    /// The default retention the bucket's Object Lock configuration names
    /// ([`BucketDefaultRetention`]), or `None` while that configuration carries no rule —
    /// the state a bucket Object Lock was enabled on by `CreateBucket` is in, and what makes
    /// a write place no retention of its own.
    ///
    /// Only ever set beside [`Self::object_lock_enabled`]: the rule is one member of the
    /// `ObjectLockConfiguration` that member is the other half of, and
    /// [`Self::set_object_lock_configuration`] is the one place either of them moves.
    ///
    /// Private, like `tags`, so the accessor pair below is the only way in or out.
    pub(super) object_lock_rule: Option<BucketDefaultRetention>,
    /// The Transfer Acceleration state the last `PutBucketAccelerateConfiguration` set on
    /// the bucket, or `None` while the bucket has none.
    ///
    /// `None` is NOT a third state and NOT a stored default: "A bucket has no Transfer
    /// Acceleration state if a state has never been set on the bucket", and a read of such
    /// a bucket "does not return a state value" for it
    /// (`GetBucketAccelerateConfiguration`'s own documentation) — the same distinction
    /// [`Self::versioning`] keeps between "never set" and "set to one of the published
    /// values". The API publishes no delete for the subresource, so a bucket leaves this
    /// state only by a put and never goes back to it.
    ///
    /// Private, like `tags`, so the accessor pair below is the only way in or out.
    pub(super) accelerate: Option<String>,
    /// The `Payer` the last `PutBucketRequestPayment` set on the bucket, or `None` while
    /// its request payment configuration has never been set.
    ///
    /// `None` is NOT "nobody pays": a bucket that has never been configured is one the
    /// bucket owner pays for ([`DEFAULT_PAYER`]), which is what a read of it reports.
    /// Keeping that case as `None` rather than as a stored copy of the default is what
    /// lets `GetBucketRequestPayment` report it without writing — the same distinction
    /// [`Self::encryption`] keeps for a never-configured bucket.
    ///
    /// Private, like `tags`, so the accessor pair below is the only way in or out.
    pub(super) request_payment: Option<String>,
    /// The attribute-based access control (ABAC) status `PutBucketAbac` last set on the
    /// bucket, or `None` while no put has set one.
    ///
    /// `None` is NOT a third state: ABAC is a PROPERTY every general purpose bucket has,
    /// and `BucketAbacStatus` names both of the states it can be in — `Enabled` or
    /// `Disabled` (`smithy_query(query="shape",shape="BucketAbacStatus")`). A bucket is
    /// created without ABAC (`CreateBucket` carries no member for it) and the API publishes
    /// neither a delete for the subresource nor a "there is none" error for a read of it,
    /// so a bucket no put has touched carries [`DEFAULT_BUCKET_ABAC_STATUS`]. Keeping that
    /// case as `None` rather than as a stored copy of the default is what lets a read of a
    /// never-configured bucket report it without writing — the same distinction
    /// [`Self::encryption`] keeps.
    ///
    /// Private, like `tags`, so the accessor pair below is the only way in or out.
    pub(super) abac: Option<String>,
    /// The Block Public Access settings in force on the bucket: the ones
    /// `PutPublicAccessBlock` last set, the all-on configuration the bucket was created
    /// with ([`BucketPublicAccessBlock::all_enabled`]) while no put has replaced it, or
    /// `None` once `DeletePublicAccessBlock` has withdrawn the subresource — the one state
    /// in which `GetPublicAccessBlock` answers
    /// [`NO_SUCH_PUBLIC_ACCESS_BLOCK_CONFIGURATION_CODE`].
    ///
    /// Private, like `tags`, so the accessor trio below is the only way in or out.
    pub(super) public_access_block: Option<BucketPublicAccessBlock>,
    /// The bucket's tag set, in the order the tagging request carried it.
    ///
    /// Private, and empty when no tag set is associated with the bucket: "no tags" and
    /// "an empty tag set" are the same state on AWS, which is why `PutBucketTagging` with
    /// an empty `TagSet` and `DeleteBucketTagging` leave a bucket in the same place.
    pub(super) tags: Vec<ResourceTag>,
    /// The bucket's ownership controls: one entry per `OwnershipControlsRule`, holding that
    /// rule's `ObjectOwnership` value in the order the request carried it, or `None` while
    /// no ownership controls are associated with the bucket.
    ///
    /// `None` and an empty list are NOT the same state here, which is why this is an
    /// `Option` where [`Self::tags`] is a plain `Vec`: `OwnershipControls$Rules` is a
    /// REQUIRED member (`smithy_query(query="shape",shape="OwnershipControls")`), so a
    /// bucket either has ownership controls that name at least one rule or has none at
    /// all — and "none at all" is what makes `GetBucketOwnershipControls` answer
    /// [`OWNERSHIP_CONTROLS_NOT_FOUND_CODE`] rather than report an empty container.
    ///
    /// Private, like `tags`, so the pair of accessors below is the only way in or out.
    pub(super) ownership_controls: Option<Vec<String>>,
    /// The bucket's EFFECTIVE `ObjectOwnership` setting, or `None` while nothing has
    /// changed it from the one a bucket is created with.
    ///
    /// Distinct from [`Self::ownership_controls`], which is the `OwnershipControls`
    /// SUBRESOURCE a reader reports: the two part company in exactly the case
    /// `GetBucketOwnershipControls` describes, a bucket with no controls associated with it
    /// at all. A freshly created bucket has none (so the read is not-found) and yet is
    /// `BucketOwnerEnforced` — "by default, `ObjectOwnership` is set to
    /// `BucketOwnerEnforced` and ACLs are disabled" — while a bucket whose controls were
    /// DELETED has none and its ACLs affect permissions again. `None` here is the former:
    /// [`DEFAULT_OBJECT_OWNERSHIP`], derived rather than stored so a create writes no copy
    /// of the default.
    ///
    /// Private, like `ownership_controls`, so the accessor pair below is the only way in or
    /// out.
    pub(super) object_ownership: Option<String>,
    /// What makes this bucket a DIRECTORY bucket — the zone it is placed in and the
    /// redundancy it is stored with ([`DirectoryBucketPlacement`]) — or `None` for a
    /// general purpose bucket, which is every bucket a create that named no
    /// `CreateBucketConfiguration$Bucket` of type [`DIRECTORY_BUCKET_TYPE`] made.
    ///
    /// The two kinds of bucket are the same record because they hold objects the same way:
    /// what differs is what the API serves on them — a session (`CreateSession`), an
    /// appending write (`PutObjectRequest$WriteOffsetBytes`), an ARN on the create, and the
    /// storage class their objects are stored in — each of which reads this one field
    /// rather than guessing from the bucket's name.
    ///
    /// Private, like `tags`, so the accessor pair below is the only way in or out.
    pub(super) directory: Option<DirectoryBucketPlacement>,
    /// The access control list `PutBucketAcl` last set on the bucket — one entry per
    /// `Grant` of the `AccessControlPolicy` it applied, in the order that ACL names them
    /// — or `None` while no ACL has ever been set on it.
    ///
    /// `None` is NOT "no access": a bucket that has never had an ACL set carries the
    /// canned `private` ACL ([`default_acl_grants`]), which is what a read of it reports.
    /// Keeping that case as `None` rather than as a stored copy of the default is what
    /// lets `GetBucketAcl` report it without writing — the same distinction
    /// [`Self::encryption`] keeps for a never-configured bucket.
    ///
    /// The OWNER is not stored beside the grants: an ACL cannot transfer a bucket, so the
    /// owner a reader reports is always the account that created it
    /// ([`canonical_user_id`]), whatever `AccessControlPolicy$Owner` a request carries.
    ///
    /// Private, like `tags`, so the accessor pair below is the only way in or out.
    pub(super) acl: Option<Vec<AclGrant>>,
    /// The bucket's event notification configuration, as
    /// `PutBucketNotificationConfiguration` last set it.
    ///
    /// NOT an `Option`, unlike `ownership_controls`: a bucket is created with the EMPTY
    /// configuration ("By default, your bucket has no event notifications configured. That
    /// is, the notification configuration will be an empty NotificationConfiguration"), and
    /// a put carrying an empty container is how notifications are turned off again — the
    /// same state, so there is nothing for a `None` to mean.
    ///
    /// Private, like `tags`: the accessor pair below and
    /// [`ComAmazonawsS3Amazons3State::put_notification_configuration`] (which mints the
    /// ids) are the only way in or out.
    pub(super) notification: BucketNotificationConfiguration,
    /// The bucket's own default server-side-encryption configuration: one entry per
    /// `ServerSideEncryptionRule` the last `PutBucketEncryption` carried, in that order,
    /// or `None` while no configuration of the bucket's own is associated with it.
    ///
    /// `None` is NOT "no encryption": a bucket without a configuration of its own is
    /// encrypted with SSE-S3 by default (see [`default_encryption_rule`]), which is what a
    /// read of a never-configured bucket reports. Keeping that case as `None` rather than
    /// as a stored copy of the default is what lets a reader report it without writing.
    ///
    /// Private, like `tags` and `ownership_controls`, so the accessor pair below is the
    /// only way in or out.
    pub(super) encryption: Option<Vec<BucketEncryptionRule>>,
    /// The replication configuration the last `PutBucketReplication` associated with the
    /// bucket, or `None` while the bucket has none — the state a bucket is created in, and
    /// what makes `GetBucketReplication` answer
    /// [`REPLICATION_CONFIGURATION_NOT_FOUND_CODE`] until a put sets one.
    ///
    /// The MODELLED container is stored whole, because that is exactly what the operation
    /// takes and what `GetBucketReplication` reports back: "Specify the replication
    /// configuration in the request body", and the read's payload is the same
    /// `ReplicationConfiguration` shape. Keeping the submitted container means a later read
    /// answers with the configuration the bucket actually carries rather than with a
    /// re-derived copy of it.
    ///
    /// Private, like `tags` and `encryption`, so the accessor pair below is the only way in
    /// or out.
    pub(super) replication: Option<crate::ops::types::ReplicationConfiguration>,
    /// The lifecycle configuration the last `PutBucketLifecycleConfiguration` associated
    /// with the bucket, or `None` while the bucket has none — the state a bucket is created
    /// in, and what makes `GetBucketLifecycleConfiguration` answer
    /// [`NO_SUCH_LIFECYCLE_CONFIGURATION_CODE`] until a put sets one.
    ///
    /// Private, like `tags` and `replication`, so the accessor pair below and
    /// [`ComAmazonawsS3Amazons3State::put_lifecycle_configuration`] (which mints the rule
    /// identifiers a request omitted) are the only way in or out. See [`BucketLifecycle`]
    /// for what is kept.
    pub(super) lifecycle: Option<BucketLifecycle>,
    /// The bucket's CloudWatch request-metrics configurations, by the id each one is
    /// identified by: "Sets a metrics configuration (specified by the metrics configuration
    /// ID) for the bucket" (`PutBucketMetricsConfiguration`'s own documentation).
    ///
    /// A MAP rather than one value, because a bucket carries MANY of them — "You can have
    /// up to 1,000 metrics configurations per bucket" (same documentation,
    /// [`MAX_METRICS_CONFIGURATIONS_PER_BUCKET`]) — each addressable on its own by the id
    /// the request named it with, which is what a read, a replacement and a delete of one
    /// configuration leave the others untouched by. A `BTreeMap`, so iterating it is
    /// ascending id order and a listing needs no sort step. Empty for a bucket no metrics
    /// configuration has been put on, which is the state a bucket is created in.
    ///
    /// The MODELLED container is stored whole, the way [`Self::replication`] is: that is
    /// exactly what the put takes as its payload and what `GetBucketMetricsConfiguration`
    /// reports back, so a read answers the configuration the bucket actually carries rather
    /// than a re-derived copy of it.
    ///
    /// Private, like `tags` and `replication`, so the accessor pair below is the only way
    /// in or out.
    pub(super) metrics_configurations: BTreeMap<String, crate::ops::types::MetricsConfiguration>,
    /// The bucket's storage class analysis configurations, by the id each one is identified
    /// by: "Sets an analytics configuration for the bucket (specified by the analytics
    /// configuration ID)" (`PutBucketAnalyticsConfiguration`'s own documentation).
    ///
    /// A MAP rather than one value, for the reason [`Self::metrics_configurations`] is one —
    /// "You can have up to 1,000 analytics configurations per bucket" (same documentation,
    /// [`MAX_ANALYTICS_CONFIGURATIONS_PER_BUCKET`]) — each addressable on its own by the id
    /// the request named it with, so a read, a replacement or a delete of one configuration
    /// leaves every other one exactly as it was. A `BTreeMap`, so iterating it IS ascending
    /// id order and a listing needs no sort step. Empty for a bucket no analytics
    /// configuration has been put on, the state a bucket is created in.
    ///
    /// The MODELLED container is stored whole, the way [`Self::replication`] is: it is
    /// exactly what the put carries as its payload and what
    /// `GetBucketAnalyticsConfiguration` reports back, so a read answers the configuration
    /// the bucket actually carries — every member the request named, including the
    /// destination's own account id — rather than a re-derived copy of it.
    ///
    /// Private, like `tags` and `metrics_configurations`, so the accessor pair below is the
    /// only way in or out.
    pub(super) analytics_configurations: BTreeMap<String, crate::ops::types::AnalyticsConfiguration>,
    /// The bucket's S3 Inventory configurations, by the inventory id each one is identified
    /// by: a put "adds an S3 Inventory configuration (identified by the inventory ID) to the
    /// bucket" (`PutBucketInventoryConfiguration`'s own documentation).
    ///
    /// A MAP rather than one value, for the reason [`Self::analytics_configurations`] is
    /// one — "You can have up to 1,000 inventory configurations per bucket" (same
    /// documentation, [`MAX_INVENTORY_CONFIGURATIONS_PER_BUCKET`]) — each addressable on its
    /// own by the id the request named it with, so a read, a replacement or a delete of one
    /// configuration leaves every other one exactly as it was. A `BTreeMap`, so iterating it
    /// IS ascending id order and a listing needs no sort step. Empty for a bucket no
    /// inventory configuration has been put on, the state a bucket is created in.
    ///
    /// The MODELLED container is stored whole, the way [`Self::replication`] is: it is
    /// exactly what the put carries as its payload and what
    /// `GetBucketInventoryConfiguration` reports back, so a read answers the configuration
    /// the bucket actually carries — the destination, the schedule, the optional fields it
    /// asked for — rather than a re-derived copy of it.
    ///
    /// Private, like `tags` and `analytics_configurations`, so the accessor pair below is
    /// the only way in or out.
    pub(super) inventory_configurations: BTreeMap<String, crate::ops::types::InventoryConfiguration>,
    /// The S3 Intelligent-Tiering configurations the bucket carries, by the id each one is
    /// identified by — empty while the bucket carries none, which is the state it is
    /// created in.
    ///
    /// A bucket carries MANY of them, not one: "Puts a S3 Intelligent-Tiering
    /// configuration to the specified bucket. You can have up to 1,000 S3
    /// Intelligent-Tiering configurations per bucket"
    /// (`PutBucketIntelligentTieringConfiguration`'s own documentation), and every other
    /// operation on one names it by "the ID used to identify the S3 Intelligent-Tiering
    /// configuration" (that request's `Id`) — so the id is the key and a put under an id
    /// already present replaces that configuration alone.
    ///
    /// The MODELLED container is stored whole, for the reason [`Self::replication`] is:
    /// it is exactly what the put carries and what a read reports back, so a reader
    /// answers with the configuration the bucket actually carries rather than a re-derived
    /// copy. A `BTreeMap`, so iterating it IS ascending id order and a listing needs no
    /// sort step, and a page's resume point is simply the next id in it.
    ///
    /// Private, like `tags` and `replication`, so the accessors below are the only way in
    /// or out.
    pub(super) intelligent_tiering: BTreeMap<String, crate::ops::types::IntelligentTieringConfiguration>,
    /// The bucket's website configuration, as `PutBucketWebsite` last set it, or `None`
    /// while no website configuration is associated with the bucket.
    ///
    /// `None` is the state a bucket is created in — a bucket is configured as a website
    /// only by "adding a website configuration" to it (`GetBucketWebsite`'s own
    /// documentation) — and it is what makes a read answer
    /// [`NO_SUCH_WEBSITE_CONFIGURATION_CODE`] rather than report a configuration that does
    /// not exist. Nothing is derived for a never-configured bucket, so a read of one writes
    /// nothing, the way it does not for [`Self::encryption`].
    ///
    /// Private, like `tags`, so the accessor pair below is the only way in or out.
    pub(super) website: Option<BucketWebsite>,
    /// The bucket's cors configuration — one entry per `CORSRule` the last
    /// `PutBucketCors` carried, in that order — or `None` while no cors configuration is
    /// associated with the bucket.
    ///
    /// `None` is the state a bucket is created in: a bucket services cross-origin requests
    /// only once "you add the cors subresource to the bucket" (`PutBucketCors`'s own
    /// documentation), and it is what makes a read answer
    /// [`NO_SUCH_CORS_CONFIGURATION_CODE`] rather than report a configuration that does not
    /// exist. An `Option` rather than a plain `Vec` for the reason `ownership_controls` is
    /// one: `CORSConfiguration$CORSRules` is REQUIRED, so a bucket either has a
    /// configuration naming at least one rule or has none at all.
    ///
    /// Private, like `tags`, so the accessor pair below is the only way in or out.
    pub(super) cors: Option<Vec<BucketCorsRule>>,
    /// Where the bucket's server access logs are delivered, as the last
    /// `PutBucketLogging` set it, or `None` while server access logging is not enabled on
    /// the bucket.
    ///
    /// `None` is the state a bucket is created in, and the state an empty
    /// `BucketLoggingStatus` puts it back into: "To enable logging, you use
    /// `LoggingEnabled` and its children request elements. To disable logging, you use an
    /// empty `BucketLoggingStatus` request element" (`PutBucketLogging`'s own
    /// documentation). It is what makes `GetBucketLogging` report a status container with
    /// no `LoggingEnabled` in it — a 200 rather than a not-found, since every bucket HAS a
    /// logging status and "not enabled" is one of the states it can be in.
    ///
    /// The MODELLED container is stored whole, the way [`Self::replication`] is: it is
    /// exactly what the put carries inside its payload and what `GetBucketLogging` reports
    /// back, so a read answers the configuration the bucket actually carries — every
    /// member the request named, its target grants and its key format included — rather
    /// than a re-derived copy of it.
    ///
    /// Private, like `tags`, so the accessor pair below is the only way in or out.
    pub(super) logging: Option<crate::ops::types::LoggingEnabled>,
    /// Every VERSION the bucket holds of every key, NEWEST FIRST within each key.
    ///
    /// Entry 0 of a key's chain is its CURRENT version — what a reader that names no
    /// `versionId` answers with — and the rest are the noncurrent ones a later write or a
    /// delete displaced, each addressable by its own [`Object::version_id`]. A chain holds
    /// a [`StoredVersion::DeleteMarker`] wherever a delete that named no version was made
    /// on a bucket carrying a versioning state: "If bucket versioning is enabled, the
    /// operation inserts a delete marker, which becomes the current version of the object"
    /// (`DeleteObject`'s documentation), so the version it displaced is still stored behind
    /// it. A bucket that was never versioning-enabled holds exactly one entry per key, the
    /// `null` version, and no markers at all.
    ///
    /// A `BTreeMap`, so iterating it IS the ascending UTF-8 byte order S3 lists keys in
    /// (`String`'s `Ord` compares bytes) and a listing needs no sort step. A key is present
    /// only while its chain is non-empty, so "the bucket holds no version of this key" and
    /// "the bucket has no such key" are the same state.
    ///
    /// Private for the same reason as `tags`: a version is recorded only through
    /// [`ComAmazonawsS3Amazons3State::put_object`] / [`Self::remove_object`] /
    /// [`Self::remove_version`], which are the only places that see every ceiling and the
    /// store's version sequence.
    pub(super) versions: BTreeMap<String, Vec<StoredVersion>>,
    /// The emulated service that provisioned this bucket on the account's behalf, or
    /// `None` for a bucket a client created over S3's own API
    /// ([`ComAmazonawsS3Amazons3State::create_bucket`]).
    ///
    /// Another AWS service whose own resource IS an S3-compatible object store (a
    /// Lightsail bucket is the one in this workspace: "Access keys grant full
    /// programmatic access to the specified bucket and its objects") stands that store up
    /// here, through [`ComAmazonawsS3Amazons3State::provision_bucket`], so the objects its
    /// resource holds are the objects THIS service's data plane serves — one object store
    /// in this emulator, not two. Such a bucket is nevertheless not a bucket of the
    /// caller's own S3 account (AWS keeps it in a service-managed account), so a reader
    /// that reports the account's buckets leaves it out:
    /// [`ComAmazonawsS3Amazons3State::account_buckets`].
    ///
    /// Private, like `tags`, and written only where the bucket comes into being: a stored
    /// bucket never changes hands.
    pub(super) provisioned_by: Option<String>,
    /// Whether AWS itself publishes this bucket, rather than any caller owning it — one of
    /// the world-readable buckets the catalog in [`aws_published`] names.
    ///
    /// Such a bucket is in the store before the first request is served, because that is
    /// what "AWS publishes it" means: nothing creates it and nothing has to. It is served
    /// by this service's object data plane exactly as any other bucket is — that is the
    /// point of holding it — while it belongs to no caller, so
    /// [`ComAmazonawsS3Amazons3State::account_buckets`] leaves it out of the account's own
    /// buckets and the whole-store budgets subtract what the catalog itself holds
    /// ([`ComAmazonawsS3Amazons3State::published_retained_bytes`]).
    ///
    /// Private and written only where the bucket comes into being, like `provisioned_by`.
    pub(super) aws_published: bool,
    /// The multipart uploads the bucket has in progress, by upload id.
    ///
    /// An upload lands here when `CreateMultipartUpload` initiates it and leaves only
    /// when it is completed or aborted: "After you initiate a multipart upload and upload
    /// one or more parts … you must either complete or abort the multipart upload"
    /// (`CreateMultipartUpload`'s own documentation). Keyed by the id the store minted
    /// ([`mint_upload_id`]), because that is what every later request names the upload by;
    /// sorted, because `ListMultipartUploads` reports uploads in key order and an id is
    /// what distinguishes two uploads of one key.
    ///
    /// Private, like `versions`, and written only through
    /// [`ComAmazonawsS3Amazons3State::create_multipart_upload`], which is the only place
    /// that sees the ceilings an in-progress upload counts against.
    pub(super) multipart_uploads: BTreeMap<String, MultipartUpload>,
    /// The `(Key, UploadId)` addresses the bucket HELD an upload under that is now gone:
    /// completed, or already aborted.
    ///
    /// What an abort needs to tell the two cases apart — "this upload is already gone", which
    /// is the state an abort asks for and so a no-op, from "this id names no upload of this
    /// bucket at all", which is `NoSuchUpload`. An id lands here the moment the upload it
    /// addressed leaves `multipart_uploads`, and nothing takes it out again: an id is minted
    /// once ([`mint_upload_id`]) and never re-issued, so the address stays retired for the
    /// life of the bucket.
    ///
    /// Bounded by [`MAX_RETIRED_MULTIPART_UPLOADS_PER_BUCKET`] with the oldest address
    /// dropped first (an emulator guard, not an AWS limit: a caller looping on
    /// initiate-and-abort must not grow this for the life of the process), which is why it is
    /// a queue rather than a set. Private, like `multipart_uploads`, and written only through
    /// [`Bucket::retire_multipart_upload`].
    pub(super) retired_multipart_uploads: VecDeque<(String, String)>,
    /// The bucket's S3 Metadata configuration, as `CreateBucketMetadataConfiguration`
    /// recorded it, or `None` while the bucket carries none — the state a bucket is
    /// created in, and what makes `GetBucketMetadataConfiguration` answer
    /// [`NO_SUCH_CONFIGURATION_CODE`] until a create records one.
    ///
    /// At most one, so a value rather than a map: see [`BucketMetadataConfiguration`].
    ///
    /// Private, like `tags` and `encryption`, so the accessor pair below is the only way
    /// in or out.
    pub(super) metadata_configuration: Option<BucketMetadataConfiguration>,

    /// When one of the bucket's CONFIGURATIONS was last written, or `None` while nothing
    /// has been configured on it since it was created.
    ///
    /// Recorded by every mutator of a bucket subresource
    /// ([`Bucket::record_configuration_write`]) and read by the one thing that reports it:
    /// the REGIONAL view of `ListBuckets`, whose `CreationDate` is the instant the Region's
    /// record of the bucket was last written rather than the instant the bucket came into
    /// being ([`Bucket::creation_date_reported_in`]).
    ///
    /// Private, like `tags` and `encryption`, so the pair of accessors is the only way in or
    /// out.
    pub(super) configured_at: Option<SystemTime>,
}

impl Bucket {
    /// When the bucket was created, as `create_bucket` recorded it. Stable across reads:
    /// nothing updates it. `None` only for a `Default`-built value, which this store
    /// never holds — a reader reports no creation date rather than inventing one.
    pub fn created_at(&self) -> Option<SystemTime> {
        self.created_at
    }

    /// The emulated service that provisioned the bucket as the object store of one of ITS
    /// resources, or `None` for a bucket created over S3's own API. See the field.
    pub fn provisioned_by(&self) -> Option<&str> {
        self.provisioned_by.as_deref()
    }

    /// Whether AWS publishes this bucket rather than a caller owning it. See the
    /// `aws_published` field and the [`aws_published`] catalog.
    pub fn is_aws_published(&self) -> bool {
        self.aws_published
    }

    /// The bucket's tag set, empty when none is associated with it.
    pub fn tags(&self) -> &[ResourceTag] {
        &self.tags
    }

    /// Record that one of the bucket's configurations was written NOW — see the
    /// `configured_at` field.
    pub(super) fn record_configuration_write(&mut self) {
        self.configured_at = Some(SystemTime::now());
    }

    /// Forget that anything has configured the bucket, leaving the instant it was created
    /// as the only one any endpoint reports.
    ///
    /// What a CREATE settles with once it has applied what the request asked for: the
    /// subresources a create installs — the Object Ownership setting, the ACL, an Object Lock
    /// configuration — come into being WITH the bucket rather than being written onto it
    /// afterwards, so nothing has configured it since it was created.
    pub fn forget_configuration_writes(&mut self) {
        self.configured_at = None;
    }

    /// The `CreationDate` a `ListBuckets` served by the endpoint of `region` reports for this
    /// bucket.
    ///
    /// Recorded behavior: the endpoint of [`DEFAULT_BUCKET_REGION`] — the Region a request
    /// that names none is signed for, and the one the global endpoint is — reports the
    /// instant the bucket was created, and every OTHER Region's endpoint reports the instant
    /// that Region's record of the bucket was last written, so writing a bucket configuration
    /// advances the date the regional view reports while leaving the global one where it is.
    /// A bucket nothing has been configured on has only the one instant to report, whichever
    /// endpoint asks.
    pub fn creation_date_reported_in(&self, region: &str) -> Option<SystemTime> {
        if region == DEFAULT_BUCKET_REGION {
            return self.created_at;
        }
        self.configured_at.or(self.created_at)
    }

    /// Replace the whole tag set with `tags`.
    ///
    /// A replacement, not a merge: "When this operation sets the tags for a bucket, it
    /// will overwrite any current tags the bucket already has. You cannot use this
    /// operation to add tags to an existing list of tags" (`PutBucketTagging`'s own
    /// documentation).
    pub fn replace_tags(&mut self, tags: Vec<ResourceTag>) {
        self.record_configuration_write();
        self.tags = tags;
    }

    /// Set the bucket's versioning state from the `VersioningConfiguration` a
    /// `PutBucketVersioning` request carried.
    ///
    /// A REPLACEMENT of the whole subresource, the way [`Bucket::replace_tags`] replaces
    /// the tag set: `VersioningConfiguration` is "Container for setting the versioning
    /// state" (`PutBucketVersioningRequest$VersioningConfiguration`), so what the bucket
    /// carries afterwards is exactly what this request carried and nothing of the
    /// configuration before it.
    pub fn set_versioning(&mut self, status: &str, mfa_delete: Option<&str>) {
        self.record_configuration_write();
        self.versioning = Some(BucketVersioning {
            status: status.to_string(),
            mfa_delete: mfa_delete.map(str::to_string),
        });
    }

    /// Whether the bucket's versioning status is `Enabled`, which is what makes a write
    /// mint a version id and keep the version it supersedes.
    pub fn versioning_enabled(&self) -> bool {
        self.versioning
            .as_ref()
            .is_some_and(|versioning| versioning.status == VERSIONING_ENABLED_STATUS)
    }

    /// Whether S3 Object Lock is enabled on the bucket — see [`Self::object_lock_enabled`].
    ///
    /// A pure read: a bucket Object Lock was never enabled on carries no configuration,
    /// which is reported rather than stored.
    pub fn object_lock_enabled(&self) -> bool {
        self.object_lock_enabled
    }

    /// Enable S3 Object Lock on the bucket, and with it versioning: "Object Lock works only
    /// in versioned buckets" (Amazon S3 User Guide, *Using S3 Object Lock*), so a bucket
    /// that carries an Object Lock configuration carries the `Enabled` versioning status
    /// that lets a lock be addressed to a version at all.
    ///
    /// One place, so no caller can record half of that pair.
    pub fn enable_object_lock(&mut self) {
        self.record_configuration_write();
        self.object_lock_enabled = true;
        // A bucket already in that state keeps the versioning configuration it carries, MFA
        // delete included: there is nothing to set.
        if !self.versioning_enabled() {
            self.set_versioning(VERSIONING_ENABLED_STATUS, None);
        }
    }

    /// The default retention new objects written to this bucket are locked under, or `None`
    /// when its Object Lock configuration names no rule — see [`Self::object_lock_rule`].
    ///
    /// A pure read: a bucket whose configuration carries no rule is reported as carrying
    /// none rather than having one stored for it.
    pub fn object_lock_default_retention(&self) -> Option<&BucketDefaultRetention> {
        self.object_lock_rule.as_ref()
    }

    /// Record the `ObjectLockConfiguration` a `PutObjectLockConfiguration` carried: Object
    /// Lock enabled on the bucket (with the versioning state Object Lock needs, through
    /// [`Self::enable_object_lock`]), and `default_retention` as the rule its new objects are
    /// locked under.
    ///
    /// A REPLACEMENT of the whole subresource, the way [`Self::set_versioning`] replaces the
    /// versioning state: the request carries "the Object Lock configuration that you want to
    /// apply to the specified bucket"
    /// (`PutObjectLockConfigurationRequest$ObjectLockConfiguration`) as its complete
    /// payload, so a configuration naming no rule leaves the bucket carrying none.
    pub fn set_object_lock_configuration(
        &mut self,
        default_retention: Option<BucketDefaultRetention>,
    ) {
        self.enable_object_lock();
        self.record_configuration_write();
        self.object_lock_rule = default_retention;
    }

    /// Why S3 refuses a request that would permanently remove the stored version `object`
    /// of `key` — `bucket` being this bucket's own name, which the authorization decision
    /// names the object by — judged at `at`, or `None` when nothing bars it.
    /// `bypass_requested` is whether the request asked to bypass governance-mode
    /// restrictions; what is answered is the `<Message>` the 403 [`ACCESS_DENIED_CODE`]
    /// carries.
    ///
    /// S3 Object Lock is a WORM protection, and what it protects is the VERSION: "if you
    /// try to delete objects protected by … retention and the retention period has not
    /// expired, the operation fails" (Amazon S3 User Guide, *S3 Object Lock*). So a
    /// permanent delete is refused while
    ///
    /// * a legal hold is on the version — judged FIRST and independently of anything the
    ///   request asks for, because a legal hold "provides the same protection as a
    ///   retention period" while having "no expiration date" and is lifted only by
    ///   `PutObjectLegalHold`, never by the governance bypass, which asks about
    ///   "Governance-mode restrictions"
    ///   (`DeleteObjectRequest$BypassGovernanceRetention`) and nothing else; or
    /// * the version's retention period is still running
    ///   ([`ObjectRetention::in_force_at`]) — unless it is a [`GOVERNANCE_RETENTION_MODE`]
    ///   one, the request asked to bypass it, and the caller may perform
    ///   [`BYPASS_GOVERNANCE_RETENTION_ACTION`]
    ///   ([`Self::governance_bypass_denied`]), since the header alone is not the privilege.
    ///
    /// The two are refused with the two sentences S3 words them in: the lock itself bars the
    /// call ([`OBJECT_LOCK_ACCESS_DENIED_MESSAGE`]), while a caller the policy denies the
    /// override to is refused the OVERRIDE, which is an authorization decision about that
    /// action ([`explicit_resource_policy_denial`]) and leaves the retention standing.
    ///
    /// An expired retention protects nothing, which is what lets the same delete succeed
    /// once the period has lapsed. A version carrying neither is refused nothing here.
    pub fn object_lock_delete_refusal(
        &self,
        bucket: &str,
        key: &str,
        object: &Object,
        at: SystemTime,
        bypass_requested: bool,
    ) -> Option<String> {
        if object.legal_hold == Some(true) {
            return Some(OBJECT_LOCK_ACCESS_DENIED_MESSAGE.to_string());
        }
        let retention = object
            .retention
            .as_ref()
            .filter(|retention| retention.in_force_at(at))?;
        if !bypass_requested || retention.mode != GOVERNANCE_RETENTION_MODE {
            return Some(OBJECT_LOCK_ACCESS_DENIED_MESSAGE.to_string());
        }
        self.governance_bypass_denied(bucket, key).then(|| {
            explicit_resource_policy_denial(
                BYPASS_GOVERNANCE_RETENTION_ACTION,
                &object_arn(bucket, key),
            )
        })
    }

    /// Whether the caller may NOT lift the governance-mode retention on the object `key` of
    /// the bucket named `bucket`.
    ///
    /// The bypass a request asks for is a PRIVILEGED override rather than a self-service
    /// one: "to override or remove governance-mode retention settings you must have the
    /// `s3:BypassGovernanceRetention` permission" (Amazon S3 User Guide, *S3 Object Lock*),
    /// so every operation whose `BypassGovernanceRetention` member can lift a retention asks
    /// this before honouring it — the delete of a retained version
    /// ([`Self::object_lock_delete_refusal`]) and the retention write that would weaken one.
    pub fn governance_bypass_denied(&self, bucket: &str, key: &str) -> bool {
        self.caller_is_denied(BYPASS_GOVERNANCE_RETENTION_ACTION, &object_arn(bucket, key))
    }

    /// Whether this bucket's own policy REFUSES the calling principal `action` on
    /// `resource`.
    ///
    /// Decided by the tree's one evaluation of IAM policy documents
    /// ([`policy_decision_for_anonymous`] asks it for the caller that presents no
    /// credential), for the principal a credentialed request here speaks for
    /// ([`caller_principal_arn`]) together with the spellings that name its whole account —
    /// a statement denying the account refuses each of its principals.
    ///
    /// The request carries a value for every condition key this service can ANSWER
    /// ([`REQUEST_CONDITION_KEYS`]) and declares those keys as ones it models, so a clause
    /// over one of them is DECIDED: the guardrail clause AWS publishes for a bucket policy —
    /// a `Deny` of everything that did not arrive over a secure transport — holds for no
    /// request this emulator serves, and the same clause stated the other way holds for
    /// every one.
    ///
    /// Only a deny this evaluation could decide refuses: a clause over a key this service
    /// does not answer leaves the statement UNDECIDED, and the evaluation's reading of an
    /// undecided `Deny` is that it still refuses — fail-closed, which is right where a GRANT
    /// is at stake and wrong here, because it would refuse the bucket owner's own read of
    /// every bucket whose policy conditions anything on a key nothing in this emulator
    /// models. The keys a decision needed and the request did not carry are what says which
    /// case this is (`Outcome::required_context_keys`).
    ///
    /// Only a DENY is reported at all, because this emulator serves one account and the
    /// caller is that account: a document that says nothing about the request leaves the
    /// bucket's own owner able to act on it, which is how every other object operation here
    /// treats it. A bucket carrying no policy denies nothing.
    pub(super) fn caller_is_denied(&self, action: &str, resource: &str) -> bool {
        let Some(policy) = self.policy.as_deref() else {
            return false;
        };
        let caller = caller_principal_arn();
        let mut principals = vec![caller.clone()];
        principals.extend(::service_common::iam_authorization::account_principals(
            &caller,
        ));
        let context = [(
            SECURE_TRANSPORT_KEY.to_string(),
            SECURE_TRANSPORT_VALUE.to_string(),
        )];
        let outcome = ::service_common::iam_authorization::resource_outcome_prepared(
            &::service_common::iam_authorization::PreparedDocument::read(policy),
            &::service_common::iam_authorization::Request::new(action, resource, &principals)
                .with_context(&context)
                .with_published_keys(REQUEST_CONDITION_KEYS),
        );
        outcome.decision == ::service_common::iam_authorization::Decision::Denied
            && outcome.required_context_keys.is_empty()
    }

    /// The object a delete that names NO `versionId` would remove PERMANENTLY, or `None`
    /// when such a delete takes nothing away.
    ///
    /// Which one it is follows [`Self::remove_object`]: on a versioning-ENABLED bucket the
    /// delete "inserts a delete marker, which becomes the current version of the object"
    /// and every stored version stays, so nothing is removed at all; on any other bucket it
    /// takes the `null` version away — the one entry carrying no minted id — which is a
    /// permanent removal like the one a `versionId` asks for, and so is subject to the same
    /// Object Lock protection ([`Self::object_lock_delete_refusal`]).
    pub fn permanently_deleted_version(&self, key: &str) -> Option<&Object> {
        if self.versioning_enabled() {
            return None;
        }
        let position = self.null_version_position(key)?;
        self.versions.get(key)?.get(position)?.as_object()
    }

    /// The bucket's Transfer Acceleration state — a `BucketAccelerateStatus` value — or
    /// `None` while no state has ever been set on it, which is what makes a read report no
    /// state value at all rather than a default. See [`Self::accelerate`].
    pub fn accelerate_status(&self) -> Option<&str> {
        self.accelerate.as_deref()
    }

    /// Set the bucket's Transfer Acceleration state from the `AccelerateConfiguration` a
    /// `PutBucketAccelerateConfiguration` request carried.
    ///
    /// A REPLACEMENT of the whole subresource, the way [`Bucket::set_versioning`] replaces
    /// the versioning state: `AccelerateConfiguration` is the "Container for setting the
    /// transfer acceleration state" the request carries as its whole payload, so the state
    /// the bucket is in afterwards is the one it names.
    pub fn set_accelerate_status(&mut self, status: &str) {
        self.record_configuration_write();
        self.accelerate = Some(status.to_string());
    }

    /// Who pays for downloads from the bucket: the `Payer` its request payment
    /// configuration was last set to, or [`DEFAULT_PAYER`] while it has never been set.
    ///
    /// Derived rather than stored for a never-configured bucket, so a read reports the
    /// default without writing one — the way [`Self::acl_grants`] reports the canned ACL
    /// a bucket starts with.
    pub fn payer(&self) -> &str {
        self.request_payment.as_deref().unwrap_or(DEFAULT_PAYER)
    }

    /// Associate `payer` with the bucket as its whole request payment configuration.
    ///
    /// A replacement, not a merge: `RequestPaymentConfiguration` is the "Container for
    /// Payer" a `PutBucketRequestPayment` carries as its payload, and it holds that one
    /// REQUIRED member (`smithy_query(query="shape",shape="RequestPaymentConfiguration")`),
    /// so what the request names becomes the bucket's configuration in full — the way
    /// [`Bucket::set_versioning`] replaces the versioning state.
    pub fn set_payer(&mut self, payer: &str) {
        self.record_configuration_write();
        self.request_payment = Some(payer.to_string());
    }

    /// The ABAC status the bucket carries: the one `PutBucketAbac` last set, or
    /// [`DEFAULT_BUCKET_ABAC_STATUS`] for a bucket no put has set one on.
    ///
    /// A pure read — the default is DERIVED here rather than stored, so reporting it
    /// writes nothing and a later caller sees exactly what this one saw.
    pub fn abac_status(&self) -> &str {
        self.abac.as_deref().unwrap_or(DEFAULT_BUCKET_ABAC_STATUS)
    }

    /// Set the bucket's ABAC status from the `AbacStatus` a `PutBucketAbac` request
    /// carried.
    ///
    /// A REPLACEMENT of the whole subresource, the way [`Bucket::set_versioning`] replaces
    /// the versioning state: the request carries the complete `AbacStatus` container as
    /// its payload, so the state the bucket is in afterwards is the one it names.
    pub fn set_abac_status(&mut self, status: &str) {
        self.record_configuration_write();
        self.abac = Some(status.to_string());
    }

    /// The bucket's Block Public Access settings, or `None` when no
    /// `PublicAccessBlockConfiguration` is associated with the bucket.
    pub fn public_access_block(&self) -> Option<BucketPublicAccessBlock> {
        self.public_access_block
    }

    /// Associate `configuration` with the bucket as its whole Block Public Access
    /// configuration.
    ///
    /// A replacement, not a merge: `PutPublicAccessBlock` "creates or modifies the
    /// PublicAccessBlock configuration for an Amazon S3 bucket" (that operation's own
    /// documentation) from the complete `PublicAccessBlockConfiguration` it carries as its
    /// payload, so what the request names becomes the bucket's configuration in full — the
    /// way [`Bucket::set_ownership_controls`] replaces the ownership controls.
    pub fn set_public_access_block(&mut self, configuration: BucketPublicAccessBlock) {
        self.record_configuration_write();
        self.public_access_block = Some(configuration);
    }

    /// Remove the bucket's Block Public Access configuration, leaving none associated with
    /// it, and report whether the bucket carried one.
    ///
    /// What `DeletePublicAccessBlock` does — "Removes the PublicAccessBlock configuration
    /// for an Amazon S3 bucket" — which leaves the bucket with no configuration at all, the
    /// one state in which a later `GetPublicAccessBlock` reports none. It is NOT the state
    /// the bucket was created in: a create installs
    /// [`BucketPublicAccessBlock::all_enabled`].
    pub fn clear_public_access_block(&mut self) -> bool {
        self.record_configuration_write();
        self.public_access_block.take().is_some()
    }

    /// Remove the bucket's tag set, leaving no tags associated with it.
    pub fn clear_tags(&mut self) {
        self.record_configuration_write();
        self.tags.clear();
    }

    /// The bucket's ownership control rules — one `ObjectOwnership` value per rule, in the
    /// order they were applied — or `None` when no ownership controls are associated with
    /// the bucket.
    pub fn ownership_controls(&self) -> Option<&[String]> {
        self.ownership_controls.as_deref()
    }

    /// Associate `rules` with the bucket as its whole ownership controls.
    ///
    /// A replacement, not a merge: `PutBucketOwnershipControls` "creates or modifies
    /// OwnershipControls for an Amazon S3 bucket" (that operation's own documentation) and
    /// carries the complete `OwnershipControls` container as its payload, so what the
    /// request names becomes the bucket's controls in full.
    pub fn set_ownership_controls(&mut self, rules: Vec<String>) {
        // The rules the container names are what the bucket's effective setting becomes:
        // the LAST one wins, the way a later rule in one document overrides an earlier one.
        if let Some(last) = rules.last() {
            self.object_ownership = Some(last.clone());
        }
        self.record_configuration_write();
        self.ownership_controls = Some(rules);
    }

    /// Remove the bucket's ownership controls, leaving none associated with it.
    ///
    /// What `DeleteBucketOwnershipControls` does — "Removes OwnershipControls for an Amazon
    /// S3 bucket" — which puts the bucket back in the state it was created in, so a later
    /// `GetBucketOwnershipControls` reports no ownership controls again
    /// ([`OWNERSHIP_CONTROLS_NOT_FOUND_CODE`]). Scoped to this one bucket's controls:
    /// nothing else on the record is touched.
    ///
    /// A bucket with no `OwnershipControls` is one whose ACLs are ENABLED, so the
    /// effective setting becomes [`OBJECT_WRITER`] rather than the
    /// [`DEFAULT_OBJECT_OWNERSHIP`] a create derives: that default belongs to the create,
    /// and withdrawing the controls is the documented way back to ACLs that affect
    /// permissions.
    pub fn clear_ownership_controls(&mut self) {
        self.record_configuration_write();
        self.ownership_controls = None;
        self.object_ownership = Some(OBJECT_WRITER.to_string());
    }

    /// The bucket's effective `ObjectOwnership` setting: what nothing has changed it from
    /// is [`DEFAULT_OBJECT_OWNERSHIP`].
    pub fn object_ownership(&self) -> &str {
        self.object_ownership
            .as_deref()
            .unwrap_or(DEFAULT_OBJECT_OWNERSHIP)
    }

    /// Set the bucket's effective `ObjectOwnership` setting — what `CreateBucket`'s
    /// `x-amz-object-ownership` header names, and what the last rule of a
    /// `PutBucketOwnershipControls` payload applies.
    pub fn set_object_ownership(&mut self, ownership: &str) {
        self.record_configuration_write();
        self.object_ownership = Some(ownership.to_string());
    }

    /// The zone and redundancy this bucket was placed with as a DIRECTORY bucket, or `None`
    /// for a general purpose bucket. See [`Bucket::directory`].
    pub fn directory_placement(&self) -> Option<&DirectoryBucketPlacement> {
        self.directory.as_ref()
    }

    /// Whether this is a directory bucket — the one question the operations that serve only
    /// directory buckets, and the ones that serve only general purpose buckets, ask.
    pub fn is_directory(&self) -> bool {
        self.directory.is_some()
    }

    /// Place this bucket in a zone as a directory bucket. Called by `CreateBucket` under the
    /// same guard that recorded the bucket, so no reader can see the bucket before its kind
    /// is settled.
    pub fn set_directory_placement(&mut self, placement: DirectoryBucketPlacement) {
        self.directory = Some(placement);
    }

    /// The storage class an object written into this bucket is stored in when the write
    /// names none: the S3 Express One Zone class for a directory bucket
    /// ([`EXPRESS_ONEZONE_STORAGE_CLASS`]) and [`DEFAULT_STORAGE_CLASS`] for a general
    /// purpose one — "if you don't specify, S3 Standard is the default storage class"
    /// (`PutObjectRequest$StorageClass`).
    pub fn default_storage_class(&self) -> &'static str {
        if self.is_directory() {
            EXPRESS_ONEZONE_STORAGE_CLASS
        } else {
            DEFAULT_STORAGE_CLASS
        }
    }

    /// The ARN of this bucket, held under the name `name` it is recorded in the store by:
    /// "the Amazon Resource Name (ARN) of the S3 bucket. ARNs uniquely identify Amazon Web
    /// Services resources" (`HeadBucketOutput$BucketArn`).
    ///
    /// Which form it takes is decided by the bucket's own KIND, not by the operation
    /// reporting it: a directory bucket is named in the `s3express` namespace, with the
    /// Region and account its zone is in ([`directory_bucket_arn`]), and a general purpose
    /// bucket by its globally unique name alone ([`bucket_arn`]). One place, so every
    /// reader that reports a bucket's ARN reports the same string for the same bucket.
    pub fn arn(&self, name: &str) -> String {
        if self.is_directory() {
            directory_bucket_arn(&self.region, name)
        } else {
            bucket_arn(name)
        }
    }

    /// Whether ACLs are DISABLED on the bucket, i.e. its effective `ObjectOwnership`
    /// setting is [`BUCKET_OWNER_ENFORCED`] — which is what a bucket is CREATED with
    /// unless the create or a later `PutBucketOwnershipControls` names another value.
    ///
    /// "If your bucket uses the bucket owner enforced setting for S3 Object Ownership,
    /// ACLs are disabled and no longer affect permissions" (`PutBucketAcl`'s own
    /// documentation): a request to SET an ACL on such a bucket is refused with
    /// [`ACL_NOT_SUPPORTED_CODE`], while a request to READ one still answers — with the
    /// bucket-owner-full-control ACL, which is [`default_acl_grants`].
    pub fn acls_disabled(&self) -> bool {
        self.object_ownership() == BUCKET_OWNER_ENFORCED
    }

    /// The grants of the bucket's access control list: the ACL last set on it, or the
    /// canned `private` ACL every bucket starts with ([`default_acl_grants`]).
    ///
    /// Derived rather than stored for a bucket that has never had an ACL set, so a read
    /// reports the default without writing one. A bucket whose ACLs are disabled
    /// ([`Self::acls_disabled`]) reports the default too: "requests to read ACLs are
    /// still supported and return the bucket-owner-full-control ACL with the owner being
    /// the account that created the bucket" (`GetBucketAcl`'s own documentation).
    ///
    /// `account` is the Block Public Access configuration the owning ACCOUNT carries, which
    /// the reader takes off the store
    /// ([`ComAmazonawsS3Amazons3State::owner_account_public_access_block`]): it is in force on
    /// this bucket together with the bucket's own ([`settings_in_force`]), so a public grant
    /// an account-level `IgnorePublicAcls` ignores is not reported either.
    pub fn acl_grants(&self, account: Option<BucketPublicAccessBlock>) -> Vec<AclGrant> {
        let stored = match self.acl.as_deref() {
            Some(grants) if !self.acls_disabled() => grants.to_vec(),
            _ => default_acl_grants(),
        };
        self.reported_acl_grants(stored, account)
    }

    /// The grants of `stored` an ACL READ reports, given the bucket's Block Public Access
    /// settings: all of them, less every PUBLIC grant ([`acl_grant_is_public`]) while
    /// `IgnorePublicAcls` is on.
    ///
    /// "Setting this element to TRUE causes Amazon S3 to ignore all public ACLs on this
    /// bucket and any objects that it contains"
    /// (`PublicAccessBlockConfiguration$IgnorePublicAcls`): a grant S3 ignores gives nobody
    /// anything, so a read of the ACL does not report it. The stored grants are untouched —
    /// the setting "doesn't affect the persistence of any existing ACLs" (same
    /// documentation) — so they are reported again once it is turned off, exactly as
    /// disabling ACLs hides them without taking them away.
    ///
    /// ONE rule for the bucket's own ACL and for the ACL of any object it contains, which
    /// is what the setting says it covers — and one rule for the two LEVELS the setting is
    /// configured at: `account` is the owning account's configuration, in force together with
    /// the bucket's own ([`settings_in_force`]).
    pub(super) fn reported_acl_grants(
        &self,
        stored: Vec<AclGrant>,
        account: Option<BucketPublicAccessBlock>,
    ) -> Vec<AclGrant> {
        if settings_in_force(self.public_access_block(), account).ignore_public_acls {
            return stored
                .into_iter()
                .filter(|grant| !acl_grant_is_public(grant))
                .collect();
        }
        stored
    }

    /// The grants a `PutBucketAcl` (or the create's canned ACL) last set on the bucket,
    /// or `None` when none ever was — the raw record, NOT what a reader reports.
    ///
    /// [`Self::acl_grants`] reports the default for a bucket whose ACLs are disabled,
    /// because that is what `GetBucketAcl` answers; this is what the bucket actually
    /// carries, which is what decides whether switching Object Ownership to
    /// [`BUCKET_OWNER_ENFORCED`] would disable an ACL that still grants something.
    pub fn stored_acl_grants(&self) -> Option<&[AclGrant]> {
        self.acl.as_deref()
    }

    /// Associate `grants` with the bucket as its whole access control list.
    ///
    /// A replacement, not a merge: `PutBucketAcl` "sets the permissions on an existing
    /// bucket using access control lists" (that operation's own documentation) from the
    /// one ACL the request names — a canned ACL, the `x-amz-grant-*` headers or an
    /// `AccessControlPolicy` body — so what the request names becomes the bucket's ACL in
    /// full, the way [`Bucket::set_ownership_controls`] replaces the ownership controls.
    pub fn set_acl_grants(&mut self, grants: Vec<AclGrant>) {
        self.record_configuration_write();
        self.acl = Some(grants);
    }

    /// The grants of the access control list of `object`, one of the objects this bucket
    /// holds: the ACL last set on that version, or the owner's `FULL_CONTROL` alone
    /// ([`default_acl_grants`]) when none ever was.
    ///
    /// Derived rather than stored for an object nobody has set an ACL on, so a read reports
    /// the default without writing one — the way [`Self::acl_grants`] reports the bucket's
    /// own default. An object of a bucket whose ACLs are disabled
    /// ([`Self::acls_disabled`]) reports the default too: "If your bucket uses the bucket
    /// owner enforced setting for S3 Object Ownership, requests to read ACLs are still
    /// supported and return the bucket-owner-full-control ACL with the owner being the
    /// account that created the bucket" (`GetObjectAcl`'s own documentation). The grants
    /// the version stores are untouched by that, so they are reported again once ACLs
    /// affect permissions on the bucket.
    ///
    /// On the BUCKET rather than on [`Object`] because the answer turns on the bucket's
    /// Object Ownership setting and on its Block Public Access settings
    /// ([`Self::reported_acl_grants`], which drops the public grants S3 ignores), exactly as
    /// [`Self::object_expiration`] lives here because it turns on the bucket's lifecycle
    /// configuration.
    pub fn object_acl_grants(
        &self,
        object: &Object,
        account: Option<BucketPublicAccessBlock>,
    ) -> Vec<AclGrant> {
        let stored = match object.acl.as_deref() {
            Some(grants) if !self.acls_disabled() => grants.to_vec(),
            _ => default_acl_grants(),
        };
        self.reported_acl_grants(stored, account)
    }

    /// Why S3 refuses a read of `key` — the bytes or the metadata of the version `object`
    /// names, `None` for an address this bucket holds no object at — to a caller that
    /// presents NO credential, or `None` when the public may read it.
    ///
    /// The ANONYMOUS caller is a principal the bucket has to grant access to explicitly,
    /// so what it may read is what this bucket's own configuration gives to EVERYONE, and
    /// everything else is refused ([`ACCESS_DENIED_MESSAGE`]) rather than served:
    ///
    /// * an ACL grant to `AllUsers` ([`acl_grants_to_all_users`]) carrying the `READ` the
    ///   read needs, judged on the grants an ACL read reports — which is where
    ///   `IgnorePublicAcls` has already dropped the public grants S3 ignores, so a grant
    ///   that is ignored admits nobody;
    /// * a bucket policy statement that allows [`GET_OBJECT_ACTION`] on this object to
    ///   every principal ([`policy_decision_for_anonymous`]) — unless
    ///   `RestrictPublicBuckets` is on, which "restricts access to … buckets with public
    ///   policies to only AWS service principals and authorized users within the bucket
    ///   owner's account" (`PublicAccessBlockConfiguration$RestrictPublicBuckets`), a set
    ///   this caller is in neither of. An explicit `Deny` refuses the read whatever the
    ///   ACL grants, the way IAM combines the two.
    ///
    /// A Requester Pays bucket refuses it ahead of all of that
    /// ([`Self::requester_pays_anonymous_refusal`]): there is no account to charge. And a
    /// request that came through an access point confined to a VPC is refused ahead of even
    /// that, for the network it arrived over.
    ///
    /// `account` is the Block Public Access configuration the owning ACCOUNT carries, in
    /// force on this bucket together with its own ([`settings_in_force`]), so either level's
    /// `IgnorePublicAcls` or `RestrictPublicBuckets` governs this read.
    pub fn anonymous_object_read_refusal(
        &self,
        bucket: &str,
        key: &str,
        object: Option<&Object>,
        account: Option<BucketPublicAccessBlock>,
    ) -> Option<&'static str> {
        // An access point confined to a VPC answers no request from outside it, whoever
        // makes it: the restriction is on the network the path is reachable over, so it is
        // decided before this bucket's own configuration is read and nothing in that
        // configuration lifts it (the same reading the credentialed decision takes,
        // `crate::state::access_points::addressed_outside_its_vpc`).
        if crate::state::access_points::addressed_outside_its_vpc().is_some() {
            return Some(ACCESS_DENIED_MESSAGE);
        }
        if let Some(message) = self.requester_pays_anonymous_refusal() {
            return Some(message);
        }
        let granted_by_acl = object.is_some_and(|object| {
            acl_grants_to_all_users(&self.object_acl_grants(object, account), PERMISSION_READ)
        });
        let restricted = settings_in_force(self.public_access_block(), account)
            .restrict_public_buckets;
        let by_policy = self
            .policy
            .as_deref()
            .map(|policy| {
                policy_decision_for_anonymous(policy, GET_OBJECT_ACTION, &object_arn(bucket, key))
            })
            // A bucket carrying no policy has no policy to state anything, which is the
            // decision a document that says nothing about the request reaches.
            .unwrap_or_default();
        let granted = match by_policy {
            ::service_common::iam_authorization::Decision::Denied => false,
            ::service_common::iam_authorization::Decision::Allowed => !restricted || granted_by_acl,
            ::service_common::iam_authorization::Decision::NotStated => granted_by_acl,
        };
        (!granted).then_some(ACCESS_DENIED_MESSAGE)
    }

    /// Why S3 refuses a read of `key` — the bytes or the metadata of the version `addressed`
    /// names, `None` for an address this bucket holds no object at — to the CREDENTIALED
    /// caller, or `None` when this bucket's policy leaves the read to it.
    ///
    /// A bucket policy is a RESOURCE-BASED policy, so it is in force on every request against
    /// the bucket's resources and not only on the public ones
    /// ([`Self::anonymous_object_read_refusal`] is the same question asked for the caller that
    /// presents none): an explicit `Deny` about the request "wins, whatever any `Allow` says"
    /// (IAM's own evaluation order, which [`Self::caller_is_denied`] answers by), and this
    /// emulator serves one account that owns its buckets, so a document saying nothing about
    /// the request leaves the read to that owner.
    ///
    /// WHICH refusal it is depends on what the address resolved to, and the two are worded as
    /// S3 words them:
    ///
    /// * a version the read DID resolve, whose read the policy denies, is refused the read
    ///   itself — the `AccessDenied` [`ACCESS_DENIED_MESSAGE`] states, about
    ///   [`GET_OBJECT_ACTION`] on that object;
    /// * an address holding NOTHING is answered by what the caller may learn about the
    ///   bucket's contents at all: "if you have the `s3:ListBucket` permission on the bucket,
    ///   Amazon S3 returns an HTTP status code 404 Not Found error. If you don't have the
    ///   `s3:ListBucket` permission, Amazon S3 returns an HTTP status code 403 Access Denied
    ///   error" (`GetObject`'s own documentation) — so a policy denying
    ///   [`LIST_BUCKET_ACTION`] on this bucket refuses the read instead of reporting the key
    ///   absent, with the sentence an explicit deny carries
    ///   ([`explicit_resource_policy_denial`]), and one that does not leaves the 404 to be
    ///   answered.
    ///
    /// Asked BEFORE anything about the key is reported, which is what keeps a refusal from
    /// saying whether the object is there.
    pub fn caller_object_read_refusal(
        &self,
        bucket: &str,
        key: &str,
        addressed: Option<&Object>,
    ) -> Option<String> {
        if addressed.is_some() {
            return self
                .caller_is_denied(GET_OBJECT_ACTION, &object_arn(bucket, key))
                .then(|| ACCESS_DENIED_MESSAGE.to_string());
        }
        let resource = self.arn(bucket);
        self.caller_is_denied(LIST_BUCKET_ACTION, &resource)
            .then(|| explicit_resource_policy_denial(LIST_BUCKET_ACTION, &resource))
    }

    /// Why REQUESTER PAYS refuses a request against this bucket that carries no credential
    /// ([`REQUESTER_PAYS_ANONYMOUS_MESSAGE`]), or `None` for a bucket whose owner pays.
    ///
    /// "Requests to a Requester Pays bucket must be authenticated: you cannot make anonymous
    /// requests" (`PutBucketRequestPayment`'s own documentation of the feature), because such
    /// a request names no account to charge. So the refusal is about the CALLER rather than
    /// about what the bucket grants: it stands however public the object is, and whether or
    /// not the request acknowledges the charge, which is why it is answered ahead of every
    /// grant ([`Self::anonymous_object_read_refusal`]) and why the sentence is the same
    /// wherever an unauthenticated caller is refused — an operation's own authorization, or
    /// the signature boundary that refuses a request before a handler sees it
    /// ([`crate::service::sigv4`]).
    pub fn requester_pays_anonymous_refusal(&self) -> Option<&'static str> {
        (self.payer() == PAYER_REQUESTER).then_some(REQUESTER_PAYS_ANONYMOUS_MESSAGE)
    }

    /// The bucket's event notification configuration — empty when the bucket has no event
    /// notifications configured, which is the state it is created in.
    ///
    /// Every destination configuration it holds carries an `Id`, minted by
    /// [`ComAmazonawsS3Amazons3State::put_notification_configuration`] when the request
    /// named none.
    pub fn notification_configuration(&self) -> &BucketNotificationConfiguration {
        &self.notification
    }

    /// The bucket's own default server-side-encryption rules, in the order they were
    /// applied, or `None` when no configuration of the bucket's own is associated with it
    /// — in which case [`default_encryption_rule`] is what applies to the bucket.
    pub fn encryption(&self) -> Option<&[BucketEncryptionRule]> {
        self.encryption.as_deref()
    }

    /// Associate `rules` with the bucket as its whole default encryption configuration.
    ///
    /// A replacement, not a merge: `PutBucketEncryption` "configures default encryption
    /// and Amazon S3 Bucket Keys for an existing bucket" from the complete
    /// `ServerSideEncryptionConfiguration` it carries as its payload, so what the request
    /// names becomes the bucket's configuration in full.
    pub fn set_encryption(&mut self, rules: Vec<BucketEncryptionRule>) {
        self.record_configuration_write();
        self.encryption = Some(rules);
    }

    /// Take the bucket's own default encryption configuration away, reporting whether it
    /// carried one.
    ///
    /// What `DeleteBucketEncryption` does: it "resets the default encryption for the bucket
    /// as server-side encryption with Amazon S3 managed keys (SSE-S3)" (that operation's own
    /// documentation) — which is exactly the state a bucket that was never configured is in,
    /// so dropping the stored rules is what resets it: [`Self::effective_encryption_rules`]
    /// then computes [`default_encryption_rule`] again for every reader, and a write that
    /// names no encryption of its own inherits that default
    /// ([`Self::default_object_encryption`]).
    ///
    /// Scoped to this one subresource, the way [`Self::clear_replication`] is scoped to the
    /// replication one: every other configuration the bucket holds, and its objects, are
    /// left exactly as they were.
    pub fn clear_encryption(&mut self) -> bool {
        self.record_configuration_write();
        self.encryption.take().is_some()
    }

    /// The bucket's replication configuration, or `None` while no configuration is
    /// associated with it.
    pub fn replication(&self) -> Option<&crate::ops::types::ReplicationConfiguration> {
        self.replication.as_ref()
    }

    /// The bucket's S3 Metadata configuration, or `None` while it carries none — in which
    /// case a read of it answers [`NO_SUCH_CONFIGURATION_CODE`].
    pub fn metadata_configuration(&self) -> Option<&BucketMetadataConfiguration> {
        self.metadata_configuration.as_ref()
    }

    /// Associate `configuration` with the bucket as its whole S3 Metadata configuration.
    ///
    /// The caller decides whether the bucket may take one: a bucket already carrying a
    /// metadata configuration is one `CreateBucketMetadataConfiguration` refuses rather
    /// than overwrites, so this is reached only once per configuration — with
    /// `GetBucketMetadataConfiguration` reporting it from then on.
    pub fn set_metadata_configuration(&mut self, configuration: BucketMetadataConfiguration) {
        self.record_configuration_write();
        self.metadata_configuration = Some(configuration);
    }

    /// Take the bucket's S3 Metadata configuration away, handing back the configuration it
    /// carried — `None` for a bucket that carried none.
    ///
    /// What `DeleteBucketMetadataConfiguration` does to the BUCKET: it "deletes an S3
    /// Metadata configuration from a general purpose bucket" (that operation's own
    /// documentation), so the whole subresource goes and a read of it reports none again. The
    /// tables that configuration provisioned are NOT the bucket's: they are tables of the
    /// managed table bucket's namespace, which the STORE holds — which is why what goes is
    /// handed back rather than dropped, and why
    /// [`ComAmazonawsS3Amazons3State::delete_metadata_configuration`] is the one way in: it
    /// takes the configuration off the bucket and settles what its tables leave in that
    /// namespace in one step, so the two records cannot be written apart.
    pub(super) fn take_metadata_configuration(&mut self) -> Option<BucketMetadataConfiguration> {
        // Only a delete that took a configuration OFF the bucket configured it — see
        // [`Self::delete_metrics_configuration`].
        let taken = self.metadata_configuration.take();
        if taken.is_some() {
            self.record_configuration_write();
        }
        taken
    }

    /// Record `configuration_state` as the inventory table state of the bucket's S3 Metadata
    /// configuration, with `namespace` the namespace an inventory table this creates is
    /// created in and `failure` why such a table could not be created — reporting whether the
    /// bucket carries a configuration to record it on.
    ///
    /// What `UpdateBucketMetadataInventoryTableConfiguration` does: it "enables or disables a
    /// live inventory table for an S3 Metadata configuration on a general purpose bucket"
    /// (that operation's own documentation), so the state the request names replaces the one
    /// the configuration carried and the inventory table behind it follows
    /// ([`set_optional_metadata_table_state`]). Scoped to that one table: the journal table,
    /// its record expiration and the annotation table are left exactly as they were, the way
    /// [`Self::clear_replication`] is scoped to the one subresource it names.
    ///
    /// `false` is a bucket carrying NO metadata configuration, in which case nothing is
    /// written: there is no configuration to enable a table for, and the caller answers
    /// [`NO_SUCH_CONFIGURATION_CODE`].
    ///
    /// PRIVATE: a table this provisions is a table of the managed table bucket's namespace,
    /// which the STORE holds, so [`ComAmazonawsS3Amazons3State::update_metadata_inventory_table`]
    /// is the one way in — it is what decides the name, judges it against that namespace and
    /// records it there, so the bucket's record and the namespace cannot be written apart.
    pub(super) fn update_metadata_inventory_table(
        &mut self,
        configuration_state: String,
        namespace: &str,
        failure: Option<MetadataTableFailure>,
        now: SystemTime,
    ) -> bool {
        let Some(configuration) = self.metadata_configuration.as_mut() else {
            return false;
        };
        set_optional_metadata_table_state(
            &mut configuration.inventory,
            configuration_state,
            // `InventoryTableConfigurationUpdates` carries no `Role`: only the annotation
            // table is managed by an IAM role.
            None,
            namespace,
            METADATA_INVENTORY_TABLE_NAME,
            failure,
            now,
        );
        true
    }

    /// Record `configuration_state` as the annotation table state of the bucket's S3 Metadata
    /// configuration and `role` as the IAM role that manages it, with `namespace` the
    /// namespace an annotation table this creates is created in and `failure` why such a
    /// table could not be created — reporting whether the bucket carries a configuration to
    /// record them on.
    ///
    /// What `UpdateBucketMetadataAnnotationTableConfiguration` does: it is used "to enable or
    /// disable the annotation table, or to update its associated IAM role" (that operation's
    /// own documentation), so the state the request names replaces the one the configuration
    /// carried and the annotation table behind it follows
    /// ([`set_optional_metadata_table_state`]), while a `role` the request names replaces the
    /// role in effect and `None` leaves it as it was ("the new IAM role ARN to apply",
    /// `AnnotationTableConfigurationUpdates$Role`, which is optional). Scoped to that one
    /// table: the journal table, its record expiration and the inventory table are left
    /// exactly as they were, the way [`Self::update_metadata_inventory_table`] is scoped to the
    /// one table it names.
    ///
    /// `false` is a bucket carrying NO metadata configuration, in which case nothing is
    /// written: there is no configuration to enable a table for, and the caller answers
    /// [`METADATA_CONFIGURATION_NOT_FOUND_CODE`].
    pub fn update_metadata_annotation_table(
        &mut self,
        configuration_state: String,
        role: Option<String>,
        namespace: &str,
        failure: Option<MetadataTableFailure>,
        now: SystemTime,
    ) -> bool {
        let Some(configuration) = self.metadata_configuration.as_mut() else {
            return false;
        };
        set_optional_metadata_table_state(
            &mut configuration.annotation,
            configuration_state,
            role,
            namespace,
            METADATA_ANNOTATION_TABLE_NAME,
            // `failure` is why a table this call CREATES could not be created: the role it
            // would be filled through does not grant S3 Metadata what reading annotations
            // needs ([`crate::state::metadata_annotation_role`]), which is the one rule the
            // create that provisions the same table is judged by too.
            failure,
            now,
        );
        true
    }

    /// Record `record_expiration` as the journal table record expiration of the bucket's S3
    /// Metadata configuration — reporting whether the bucket carries a configuration to
    /// record it on.
    ///
    /// What `UpdateBucketMetadataJournalTableConfiguration` does: it "enables or disables
    /// journal table record expiration for an S3 Metadata configuration on a general purpose
    /// bucket" (that operation's own documentation), and the only thing its payload carries
    /// is that setting (`JournalTableConfigurationUpdates` has `RecordExpiration` as its one,
    /// required member), so the settings the request names replace the ones the configuration
    /// carried in full. Scoped to that one setting: the journal TABLE itself — which every
    /// configuration has, so there is none to create or drop — its name and its status, and
    /// the inventory and annotation tables, are left exactly as they were, the way
    /// [`Self::update_metadata_inventory_table`] is scoped to the one table it names.
    ///
    /// `false` is a bucket carrying NO metadata configuration, in which case nothing is
    /// written: there is no journal table to expire records of, and the caller answers
    /// [`NO_SUCH_CONFIGURATION_CODE`].
    pub fn update_metadata_journal_record_expiration(
        &mut self,
        record_expiration: crate::ops::types::RecordExpiration,
    ) -> bool {
        let Some(configuration) = self.metadata_configuration.as_mut() else {
            return false;
        };
        configuration.journal_record_expiration = record_expiration;
        true
    }

    /// Associate `configuration` with the bucket as its whole replication configuration.
    ///
    /// A replacement, not a merge: `PutBucketReplication` "creates a replication
    /// configuration or replaces an existing one" from the complete
    /// `ReplicationConfiguration` it carries as its payload, so what the request names
    /// becomes the bucket's configuration in full.
    pub fn set_replication(
        &mut self,
        configuration: crate::ops::types::ReplicationConfiguration,
    ) {
        self.record_configuration_write();
        self.replication = Some(configuration);
    }

    /// Remove the bucket's replication configuration, reporting whether it carried one.
    ///
    /// What `DeleteBucketReplication` does — it "deletes the replication configuration from
    /// the bucket" (that operation's own documentation) — which puts the bucket back in the
    /// state it was created in, so a later `GetBucketReplication` reports no configuration
    /// again ([`REPLICATION_CONFIGURATION_NOT_FOUND_CODE`]). Scoped to this one
    /// subresource: the bucket's versioning, its objects and every other configuration are
    /// left exactly as they were, the way [`Self::clear_lifecycle`] is scoped to the
    /// lifecycle one.
    pub fn clear_replication(&mut self) -> bool {
        self.record_configuration_write();
        self.replication.take().is_some()
    }

    /// The bucket's lifecycle configuration, or `None` while no configuration is associated
    /// with it.
    pub fn lifecycle(&self) -> Option<&BucketLifecycle> {
        self.lifecycle.as_ref()
    }

    /// Remove the bucket's lifecycle configuration, reporting whether it carried one.
    ///
    /// What `DeleteBucketLifecycle` does: it "removes all the lifecycle configuration rules
    /// in the lifecycle subresource associated with the bucket" (that operation's own
    /// documentation), which leaves the bucket in the state it was created in — so a bucket
    /// that carried none is already there and nothing changes.
    pub fn clear_lifecycle(&mut self) -> bool {
        self.record_configuration_write();
        self.lifecycle.take().is_some()
    }

    /// When the bucket's lifecycle configuration expires the object stored at `key`, and
    /// which rule does it — or `None` when no rule in effect expires that object.
    ///
    /// "If the object expiration is configured … the response includes this header. It
    /// includes the expiry-date and rule-id key-value pairs providing object expiration
    /// information" (`GetObjectOutput$Expiration`), so every reader and writer of an object
    /// reports this one answer.
    ///
    /// The EARLIEST expiry wins when several rules apply, because that is when the object
    /// actually goes; rules that expire it at the same instant are settled by the order the
    /// configuration lists them. Only a rule whose `Status` is `Enabled` counts, since a
    /// disabled rule "is not currently being applied", and only an `Expiration` that names
    /// a day count or a date dates anything — `ExpiredObjectDeleteMarker` is about a delete
    /// marker rather than about this object.
    ///
    /// Derived, never stored: a later tagging call moves an object into or out of a
    /// tag-filtered rule, so the answer belongs to the object's CURRENT state.
    pub fn object_expiration(&self, key: &str, object: &Object) -> Option<ObjectExpiration> {
        let size = i64::try_from(object.body.len()).ok()?;
        self.lifecycle()?
            .rules
            .iter()
            .filter(|rule| rule.status == LIFECYCLE_RULE_ENABLED)
            .filter(|rule| lifecycle_rule_applies(rule, key, size, &object.tags))
            .filter_map(|rule| {
                let expiration = rule.expiration.as_ref()?;
                let expires_at = match (expiration.days, &expiration.date) {
                    (Some(days), _) => midnight_after_days(object.last_modified, days)?,
                    (None, Some(date)) => instant_of(date)?,
                    (None, None) => return None,
                };
                Some(ObjectExpiration {
                    expires_at,
                    rule_id: rule.id.clone().unwrap_or_default(),
                })
            })
            .min_by_key(|expiration| expiration.expires_at)
    }

    /// [`Self::object_expiration`] as the `Expiration` response member carries it, or `None`
    /// when no rule in effect expires the object.
    ///
    /// The one place the lookup and the rendering are put together, so every operation whose
    /// output documents that member reports the same string for the same object.
    pub fn object_expiration_header(&self, key: &str, object: &Object) -> Option<String> {
        self.object_expiration(key, object)
            .and_then(|expiration| expiration.header_value())
    }

    /// When the bucket's lifecycle configuration aborts the in-progress multipart upload of
    /// `key` initiated at `initiated_at`, and which rule does it — or `None` when no rule
    /// in effect aborts it.
    ///
    /// "If the bucket has a lifecycle rule configured with an action to abort incomplete
    /// multipart uploads and the prefix in the lifecycle rule matches the object name in
    /// the request, the response includes this header"
    /// (`CreateMultipartUploadOutput$AbortDate`): the PREFIX is what selects the upload,
    /// because an upload in progress is not yet an object and so has no tag set or size for
    /// a rule's other predicates to read.
    ///
    /// The earliest abort wins when several rules apply, for the reason the earliest
    /// expiry does in [`Self::object_expiration`].
    pub fn multipart_upload_abort(
        &self,
        key: &str,
        initiated_at: SystemTime,
    ) -> Option<MultipartUploadAbort> {
        self.lifecycle()?
            .rules
            .iter()
            .filter(|rule| rule.status == LIFECYCLE_RULE_ENABLED)
            .filter(|rule| key.starts_with(lifecycle_rule_prefix(rule)))
            .filter_map(|rule| {
                let days = rule
                    .abort_incomplete_multipart_upload
                    .as_ref()?
                    .days_after_initiation?;
                Some(MultipartUploadAbort {
                    aborts_at: midnight_after_days(initiated_at, days)?,
                    rule_id: rule.id.clone().unwrap_or_default(),
                })
            })
            .min_by_key(|abort| abort.aborts_at)
    }

    /// The metrics configuration the bucket carries under `id`, or `None` when it carries
    /// none under that id — the state every bucket starts in, and what makes a read answer
    /// [`NO_SUCH_CONFIGURATION_CODE`].
    pub fn metrics_configuration(
        &self,
        id: &str,
    ) -> Option<&crate::ops::types::MetricsConfiguration> {
        self.metrics_configurations.get(id)
    }

    /// Drop the metrics configuration the bucket carries under `id`, reporting whether one
    /// was there to drop.
    ///
    /// Per-ID, like the put: a delete "specified by the metrics configuration ID" removes
    /// THAT configuration and leaves every other id the bucket carries in place. `false`
    /// means the bucket carried none under that id, which is what makes a delete answer
    /// [`NO_SUCH_CONFIGURATION_CODE`] rather than report a removal it did not make.
    pub fn delete_metrics_configuration(&mut self, id: &str) -> bool {
        // Only a delete that REMOVED a configuration configured the bucket: a call naming an
        // id the bucket does not carry is refused ([`NO_SUCH_CONFIGURATION_CODE`]) and wrote
        // nothing.
        let removed = self.metrics_configurations.remove(id).is_some();
        if removed {
            self.record_configuration_write();
        }
        removed
    }

    /// Every metrics configuration the bucket carries, in ascending id order — empty for a
    /// bucket that carries none.
    ///
    /// The order is the store's own (a `BTreeMap` of ids), so two listings of an unchanged
    /// bucket report the same sequence and a page can resume at the next id; the stored
    /// containers themselves, so a listing reports what the puts recorded.
    pub fn metrics_configurations(
        &self,
    ) -> impl Iterator<Item = (&str, &crate::ops::types::MetricsConfiguration)> + '_ {
        self.metrics_configurations
            .iter()
            .map(|(id, configuration)| (id.as_str(), configuration))
    }

    /// Associate `configuration` with the bucket under `id`, and report what that did.
    ///
    /// A full replacement of the configuration of that id, and of that one only: "If you're
    /// updating an existing metrics configuration, note that this is a full replacement of
    /// the existing metrics configuration. If you don't include the elements you want to
    /// keep, they are erased" (`PutBucketMetricsConfiguration`'s own documentation). Every
    /// other id the bucket carries is left exactly as it was.
    ///
    /// The ceiling is checked HERE, where the bucket's configuration count is, so a put
    /// cannot reach past [`MAX_METRICS_CONFIGURATIONS_PER_BUCKET`]; replacing an id the
    /// bucket already carries adds no configuration and so is never refused for it.
    pub fn put_metrics_configuration(
        &mut self,
        id: String,
        configuration: crate::ops::types::MetricsConfiguration,
    ) -> BucketConfigurationWrite {
        let written = put_keyed_configuration(
            &mut self.metrics_configurations,
            id,
            configuration,
            MAX_METRICS_CONFIGURATIONS_PER_BUCKET,
        );
        // Only a put that was TAKEN configured the bucket: one refused at the ceiling wrote
        // nothing, so it left the bucket's configuration exactly as it was.
        if written != BucketConfigurationWrite::QuotaReached {
            self.record_configuration_write();
        }
        written
    }

    /// The analytics configuration the bucket carries under `id`, or `None` when it carries
    /// none under that id — the state every bucket starts in, and what makes a read answer
    /// [`NO_SUCH_CONFIGURATION_CODE`].
    ///
    /// The stored container itself, so what a reader reports is what the put recorded rather
    /// than a re-derived copy — the reading [`Self::metrics_configuration`] gives.
    pub fn analytics_configuration(
        &self,
        id: &str,
    ) -> Option<&crate::ops::types::AnalyticsConfiguration> {
        self.analytics_configurations.get(id)
    }

    /// Drop the analytics configuration the bucket carries under `id`, reporting whether one
    /// was there to drop.
    ///
    /// Per-ID, like the put: a delete "specified by the analytics configuration ID"
    /// (`DeleteBucketAnalyticsConfiguration`'s own documentation) removes THAT configuration
    /// and leaves every other id the bucket carries in place. `false` means the bucket
    /// carried none under that id, which is what makes a delete answer
    /// [`NO_SUCH_CONFIGURATION_CODE`] rather than report a removal it did not make.
    pub fn delete_analytics_configuration(&mut self, id: &str) -> bool {
        // Only a delete that REMOVED a configuration configured the bucket — see
        // [`Self::delete_metrics_configuration`].
        let removed = self.analytics_configurations.remove(id).is_some();
        if removed {
            self.record_configuration_write();
        }
        removed
    }

    /// Every analytics configuration the bucket carries, in ascending id order — empty for a
    /// bucket that carries none.
    ///
    /// The order is the store's own (a `BTreeMap` of ids), the reading
    /// [`Self::metrics_configurations`] gives: two listings of an unchanged bucket report the
    /// same sequence and a page can resume at the next id. The stored containers themselves,
    /// so a listing reports what the puts recorded.
    pub fn analytics_configurations(
        &self,
    ) -> impl Iterator<Item = (&str, &crate::ops::types::AnalyticsConfiguration)> + '_ {
        self.analytics_configurations
            .iter()
            .map(|(id, configuration)| (id.as_str(), configuration))
    }

    /// Associate `configuration` with the bucket under `id`, and report what that did.
    ///
    /// Per-ID and a full replacement of that id alone: a put "specified by the analytics
    /// configuration ID" (`PutBucketAnalyticsConfiguration`'s own documentation) carries the
    /// whole container, so the configuration stored under `id` becomes exactly the one this
    /// request named and every other id the bucket carries is left as it was.
    ///
    /// The ceiling is checked HERE, where the bucket's configuration count is, so a put
    /// cannot reach past [`MAX_ANALYTICS_CONFIGURATIONS_PER_BUCKET`]; replacing an id the
    /// bucket already carries adds no configuration and so is never refused for it.
    pub fn put_analytics_configuration(
        &mut self,
        id: String,
        configuration: crate::ops::types::AnalyticsConfiguration,
    ) -> BucketConfigurationWrite {
        let written = put_keyed_configuration(
            &mut self.analytics_configurations,
            id,
            configuration,
            MAX_ANALYTICS_CONFIGURATIONS_PER_BUCKET,
        );
        // Only a put that was TAKEN configured the bucket — see
        // [`Self::put_metrics_configuration`].
        if written != BucketConfigurationWrite::QuotaReached {
            self.record_configuration_write();
        }
        written
    }

    /// The S3 Inventory configuration the bucket carries under `id`, or `None` when it
    /// carries none under that id — the state every bucket starts in, and what makes a read
    /// answer [`NO_SUCH_CONFIGURATION_CODE`].
    ///
    /// The stored container itself, so what a reader reports is what the put recorded rather
    /// than a re-derived copy — the reading [`Self::analytics_configuration`] gives.
    pub fn inventory_configuration(
        &self,
        id: &str,
    ) -> Option<&crate::ops::types::InventoryConfiguration> {
        self.inventory_configurations.get(id)
    }

    /// Every S3 Inventory configuration the bucket carries, in ascending id order — empty
    /// for a bucket that carries none.
    ///
    /// The order is the store's own (a `BTreeMap` of ids), the reading
    /// [`Self::analytics_configurations`] gives: two listings of an unchanged bucket report
    /// the same sequence and a page can resume at the next id. The stored containers
    /// themselves, so a listing reports what the puts recorded.
    pub fn inventory_configurations(
        &self,
    ) -> impl Iterator<Item = (&str, &crate::ops::types::InventoryConfiguration)> + '_ {
        self.inventory_configurations
            .iter()
            .map(|(id, configuration)| (id.as_str(), configuration))
    }

    /// Associate `configuration` with the bucket under `id`, and report what that did.
    ///
    /// Per-ID and a full replacement of that id alone: a put "adds an S3 Inventory
    /// configuration (identified by the inventory ID) to the bucket"
    /// (`PutBucketInventoryConfiguration`'s own documentation) and carries the whole
    /// container, so the configuration stored under `id` becomes exactly the one this request
    /// named and every other id the bucket carries is left as it was.
    ///
    /// The ceiling is checked HERE, where the bucket's configuration count is, so a put
    /// cannot reach past [`MAX_INVENTORY_CONFIGURATIONS_PER_BUCKET`]; replacing an id the
    /// bucket already carries adds no configuration and so is never refused for it.
    pub fn put_inventory_configuration(
        &mut self,
        id: String,
        configuration: crate::ops::types::InventoryConfiguration,
    ) -> BucketConfigurationWrite {
        let written = put_keyed_configuration(
            &mut self.inventory_configurations,
            id,
            configuration,
            MAX_INVENTORY_CONFIGURATIONS_PER_BUCKET,
        );
        // Only a put that was TAKEN configured the bucket — see
        // [`Self::put_metrics_configuration`].
        if written != BucketConfigurationWrite::QuotaReached {
            self.record_configuration_write();
        }
        written
    }

    /// Drop the S3 Inventory configuration the bucket carries under `id`, reporting whether
    /// there was one to drop.
    ///
    /// Per-ID, like the put: a delete "deletes an S3 Inventory configuration (identified by
    /// the inventory ID) from the bucket" (`DeleteBucketInventoryConfiguration`'s own
    /// documentation) removes THAT configuration and leaves every other id the bucket carries
    /// in place. `false` means the bucket carried none under that id, which is what makes a
    /// delete answer [`NO_SUCH_CONFIGURATION_CODE`] rather than report a removal it did not
    /// make.
    pub fn delete_inventory_configuration(&mut self, id: &str) -> bool {
        // Only a delete that REMOVED a configuration configured the bucket — see
        // [`Self::delete_metrics_configuration`].
        let removed = self.inventory_configurations.remove(id).is_some();
        if removed {
            self.record_configuration_write();
        }
        removed
    }

    /// Every S3 Intelligent-Tiering configuration the bucket carries, in ascending id
    /// order — empty for a bucket that carries none.
    ///
    /// The order is the store's own (a `BTreeMap` of ids), so two listings of an unchanged
    /// bucket report the same sequence and a page can resume at the next id.
    pub fn intelligent_tiering_configurations(
        &self,
    ) -> impl Iterator<Item = (&str, &crate::ops::types::IntelligentTieringConfiguration)> + '_
    {
        self.intelligent_tiering
            .iter()
            .map(|(id, configuration)| (id.as_str(), configuration))
    }

    /// Associate `configuration` with the bucket under `id`, reporting what that did.
    ///
    /// Per-ID, not whole-bucket: `PutBucketIntelligentTieringConfiguration` "puts a S3
    /// Intelligent-Tiering configuration to the specified bucket" under the `Id` the
    /// request names, so a put under an id the bucket already carries replaces THAT
    /// configuration and leaves every other one in place.
    ///
    /// A put that would add a NEW id past
    /// [`MAX_INTELLIGENT_TIERING_CONFIGURATIONS_PER_BUCKET`] writes nothing and reports
    /// [`IntelligentTieringWrite::ConfigurationCountReached`], which is the published
    /// ceiling rather than this emulator's own; replacing an id already present is never
    /// refused by it, since it adds no configuration.
    pub fn put_intelligent_tiering_configuration(
        &mut self,
        id: String,
        configuration: crate::ops::types::IntelligentTieringConfiguration,
    ) -> IntelligentTieringWrite {
        if !self.intelligent_tiering.contains_key(&id)
            && self.intelligent_tiering.len() >= MAX_INTELLIGENT_TIERING_CONFIGURATIONS_PER_BUCKET
        {
            return IntelligentTieringWrite::ConfigurationCountReached;
        }
        self.record_configuration_write();
        self.intelligent_tiering.insert(id, configuration);
        IntelligentTieringWrite::Stored
    }

    /// The one S3 Intelligent-Tiering configuration the bucket carries under `id`, or
    /// `None` when it carries none under that id.
    ///
    /// The stored container itself, so what a reader reports is what the put recorded
    /// rather than a re-derived copy — the reading [`Self::replication`] gives.
    pub fn intelligent_tiering_configuration(
        &self,
        id: &str,
    ) -> Option<&crate::ops::types::IntelligentTieringConfiguration> {
        self.intelligent_tiering.get(id)
    }

    /// Drop the S3 Intelligent-Tiering configuration the bucket carries under `id`,
    /// reporting whether there was one to drop.
    ///
    /// Per-ID, the way [`Self::put_intelligent_tiering_configuration`] records one: every
    /// other configuration the bucket carries stays exactly as it was, so a bucket whose
    /// last configuration is removed carries none again.
    pub fn delete_intelligent_tiering_configuration(&mut self, id: &str) -> bool {
        // Only a delete that REMOVED a configuration configured the bucket — see
        // [`Self::delete_metrics_configuration`].
        let removed = self.intelligent_tiering.remove(id).is_some();
        if removed {
            self.record_configuration_write();
        }
        removed
    }

    /// The default encryption rules IN FORCE on the bucket: the ones a
    /// `PutBucketEncryption` associated with it, or the single SSE-S3 rule every bucket
    /// carries until one does ([`default_encryption_rule`]).
    ///
    /// A configured rule that names NO `BlockedEncryptionTypes` leaves the bucket's blocked
    /// set at [`DEFAULT_BLOCKED_ENCRYPTION_TYPES`], which is what this materialises:
    /// blocking an encryption type is "a bucket-level setting … used to prevent the upload
    /// of new objects encrypted with the specified server-side encryption type", one whose
    /// own enum publishes the value that UNBLOCKS it (`NONE`, beside `SSE-C` —
    /// `smithy_query(query="shape",shape="EncryptionType")`), so a configuration that says
    /// nothing about it neither turns the blocking off nor hides it from a reader.
    ///
    /// COMPUTED, never stored: a bucket that was never configured keeps [`Self::encryption`]
    /// at `None`, so reading this changes nothing a later caller sees. This is what
    /// `GetBucketEncryption` reports and what [`Self::default_object_encryption`] resolves a
    /// write's inherited encryption from, so the two can never disagree.
    pub fn effective_encryption_rules(&self) -> Vec<BucketEncryptionRule> {
        match self.encryption() {
            Some(configured) => configured
                .iter()
                .map(|rule| BucketEncryptionRule {
                    blocked_encryption_types: rule
                        .blocked_encryption_types
                        .clone()
                        .or_else(|| Some(default_blocked_encryption_types())),
                    ..rule.clone()
                })
                .collect(),
            None => vec![default_encryption_rule()],
        }
    }

    /// The encryption an object written to this bucket WITHOUT naming any encryption of its
    /// own is stored under: the bucket's default encryption configuration.
    ///
    /// "Amazon S3 encrypts data with server-side encryption by using Amazon S3 managed keys
    /// (SSE-S3) by default. You can optionally tell Amazon S3 to encrypt data at rest by
    /// using server-side encryption with other key options"
    /// (`PutObjectRequest$ServerSideEncryption`) — the other key options being the ones
    /// `PutBucketEncryption` configures, which is why the rule applied here is the bucket's
    /// effective one rather than SSE-S3 always.
    ///
    /// The rule that decides it is the first of [`Self::effective_encryption_rules`] that
    /// names an `ApplyServerSideEncryptionByDefault` algorithm; a configuration whose rules
    /// name none (`ServerSideEncryptionRule` makes that container optional) leaves the
    /// bucket on the SSE-S3 base default. The Bucket Key setting travels with the algorithm
    /// only when that algorithm is SSE-KMS, because an S3 Bucket Key is "for object
    /// encryption with server-side encryption using Key Management Service (KMS) keys
    /// (SSE-KMS)" (`PutObjectRequest$BucketKeyEnabled`) and nothing else; an encryption
    /// CONTEXT is never inherited, because it is a per-request value no bucket
    /// configuration carries.
    pub fn default_object_encryption(&self) -> ObjectEncryption {
        let rules = self.effective_encryption_rules();
        let rule = rules
            .iter()
            .find(|rule| rule.sse_algorithm.is_some())
            .cloned()
            .unwrap_or_else(default_encryption_rule);
        let inherited = ObjectEncryption {
            sse_algorithm: rule.sse_algorithm,
            kms_key_id: rule.kms_master_key_id,
            kms_encryption_context: None,
            bucket_key_enabled: rule.bucket_key_enabled,
            // A bucket default never names a CUSTOMER-provided key: SSE-C is a per-request
            // key the caller supplies and S3 does not keep, so no configuration can carry
            // one to inherit.
            ..ObjectEncryption::default()
        };
        if inherited.uses_kms() {
            inherited
        } else {
            ObjectEncryption {
                sse_algorithm: inherited.sse_algorithm,
                ..ObjectEncryption::default()
            }
        }
    }

    /// The bucket's website configuration, or `None` when no website configuration is
    /// associated with the bucket.
    pub fn website(&self) -> Option<&BucketWebsite> {
        self.website.as_ref()
    }

    /// Associate `configuration` with the bucket as its whole website configuration.
    ///
    /// A replacement, not a merge: `PutBucketWebsite` "sets the configuration of the
    /// website that is specified in the website subresource" from the complete
    /// `WebsiteConfiguration` it carries as its payload (that operation's own
    /// documentation), so what the request names becomes the bucket's configuration in
    /// full — the way [`Bucket::set_ownership_controls`] replaces the ownership controls.
    pub fn set_website(&mut self, configuration: BucketWebsite) {
        self.record_configuration_write();
        self.website = Some(configuration);
    }

    /// Remove the bucket's website configuration, leaving none associated with it.
    ///
    /// What `DeleteBucketWebsite` does — "This action removes the website configuration for
    /// a bucket" (that operation's own documentation) — which puts the bucket back in the
    /// state it was created in, so a later `GetBucketWebsite` answers
    /// [`NO_SUCH_WEBSITE_CONFIGURATION_CODE`] again. Scoped to this one bucket's website
    /// configuration: nothing else on the record is touched, the way [`Bucket::clear_cors`]
    /// is scoped to the cors configuration.
    pub fn clear_website(&mut self) {
        self.record_configuration_write();
        self.website = None;
    }

    /// The bucket's cors configuration, or `None` when no cors configuration is associated
    /// with the bucket.
    pub fn cors(&self) -> Option<&[BucketCorsRule]> {
        self.cors.as_deref()
    }

    /// Associate `rules` with the bucket as its whole cors configuration.
    ///
    /// A replacement, not a merge: `PutBucketCors` "sets the cors configuration for your
    /// bucket. If the configuration exists, Amazon S3 replaces it" (that operation's own
    /// documentation), so what the request names becomes the bucket's configuration in full
    /// — the way [`Bucket::set_website`] replaces the website configuration.
    pub fn set_cors(&mut self, rules: Vec<BucketCorsRule>) {
        self.record_configuration_write();
        self.cors = Some(rules);
    }

    /// Remove the bucket's cors configuration, leaving none associated with it.
    ///
    /// What `DeleteBucketCors` does — "Deletes the cors configuration information set for
    /// the bucket" (that operation's own documentation) — which puts the bucket back in the
    /// state it was created in, so a later `GetBucketCors` answers
    /// [`NO_SUCH_CORS_CONFIGURATION_CODE`] again. Scoped to this one bucket's cors
    /// configuration: nothing else on the record is touched, the way
    /// [`Bucket::clear_ownership_controls`] is scoped to the ownership controls.
    pub fn clear_cors(&mut self) {
        self.record_configuration_write();
        self.cors = None;
    }

    /// Where the bucket's server access logs are delivered, or `None` while logging is not
    /// enabled on it. See [`Self::logging`].
    pub fn logging(&self) -> Option<&crate::ops::types::LoggingEnabled> {
        self.logging.as_ref()
    }

    /// Set the bucket's logging status to `logging`: `Some` to enable server access
    /// logging with that configuration, `None` to disable it.
    ///
    /// A replacement of the whole subresource, the way [`Bucket::set_cors`] replaces the
    /// cors configuration: `BucketLoggingStatus` is the complete "container for logging
    /// status information" a `PutBucketLogging` carries as its payload, so what the bucket
    /// carries afterwards is exactly what that container named — and a container naming no
    /// `LoggingEnabled` is how logging is turned off again ("To disable logging, you use an
    /// empty `BucketLoggingStatus` request element", that operation's own documentation).
    pub fn set_logging(&mut self, logging: Option<crate::ops::types::LoggingEnabled>) {
        self.record_configuration_write();
        self.logging = logging;
    }

    /// The object stored at `key`, or `None` when the bucket holds no current object
    /// there.
    ///
    /// The CURRENT version: what a read that names no `versionId` answers with, and the
    /// only version a bucket that was never versioning-enabled has. `None` when the key's
    /// current version is a DELETE MARKER — "if the current version of the object is a
    /// delete marker, Amazon S3 behaves as if the object was deleted"
    /// (`GetObject`/`DeleteObject`) — and also when the bucket holds no such key at all:
    /// the two are the same thing to every reader of a current object.
    pub fn object(&self, key: &str) -> Option<&Object> {
        self.current_version(key)?.as_object()
    }

    /// The key's CURRENT version, object or delete marker, or `None` when the bucket holds
    /// no version of it.
    ///
    /// Entry 0 of the chain: "the delete marker … becomes the current version of the
    /// object", and a write makes what it stores current.
    pub fn current_version(&self, key: &str) -> Option<&StoredVersion> {
        self.versions.get(key)?.first()
    }

    /// The version of `key` that `version_id` names — current or noncurrent, object or
    /// delete marker — or `None` when the bucket holds no such version.
    ///
    /// [`NULL_VERSION_ID`] names the version a write made while the bucket was not
    /// versioning-enabled, which is stored with no minted id of its own.
    pub fn stored_version(&self, key: &str, version_id: &str) -> Option<&StoredVersion> {
        self.versions
            .get(key)?
            .iter()
            .find(|held| held.named_by(version_id))
    }

    /// The OBJECT version of `key` that `version_id` names, or `None` when the bucket
    /// holds no such version or that version is a delete marker (which holds no bytes to
    /// answer a read with — see [`Self::stored_version`] for the version itself).
    pub fn object_version(&self, key: &str, version_id: &str) -> Option<&Object> {
        self.stored_version(key, version_id)?.as_object()
    }

    /// The object version a request ADDRESSES: the one `version_id` names, or the key's
    /// current object when the request names no version.
    ///
    /// One place, so every operation that acts on "this object, optionally this version of
    /// it" resolves the address the same way — "the version ID for the object that you want
    /// to apply this Object Retention configuration to"
    /// (`PutObjectRetentionRequest$VersionId`), and with none the current version, which is
    /// what a reader naming no `versionId` answers with.
    pub fn addressed_object(&self, key: &str, version_id: Option<&str>) -> Option<&Object> {
        match version_id {
            None => self.object(key),
            Some(version_id) => self.object_version(key, version_id),
        }
    }

    /// Every version of `key` the bucket holds, NEWEST FIRST: the current one followed by
    /// the noncurrent ones, delete markers among them.
    ///
    /// The read seam for anything that reports a key's versions rather than its current
    /// object, so no caller reaches into the chain itself.
    pub fn object_versions(&self, key: &str) -> &[StoredVersion] {
        self.versions.get(key).map_or(&[], Vec::as_slice)
    }

    /// Every CURRENT OBJECT the bucket holds, as `(key, object)` in ascending key order.
    ///
    /// The read seam for a listing over current objects (`ListObjectsV2`): ascending UTF-8
    /// byte order, because that is the order S3 lists keys in ("returns some or all of the
    /// objects in a bucket … in lexicographical order"), and a key whose current version
    /// is a delete marker is NOT here — a delete marker is what makes a key stop appearing
    /// in such a listing while its versions are still stored.
    pub fn objects(&self) -> impl Iterator<Item = (&str, &Object)> + '_ {
        self.versions
            .iter()
            .filter_map(|(key, chain)| Some((key.as_str(), chain.first()?.as_object()?)))
    }

    /// The in-progress multipart upload `upload_id` names, or `None` when the bucket has
    /// no such upload in progress — which is what makes a read or a part upload naming an
    /// id this store never minted (or one a completion or an abort has since taken away)
    /// answer `NoSuchUpload`.
    pub fn multipart_upload(&self, upload_id: &str) -> Option<&MultipartUpload> {
        self.multipart_uploads.get(upload_id)
    }

    /// Every multipart upload the bucket has in progress, keyed by upload id.
    ///
    /// The read seam for anything that reports the bucket's uploads rather than one of
    /// them, so no caller reaches into the map. Read-only: an upload is recorded only
    /// through [`ComAmazonawsS3Amazons3State::create_multipart_upload`], the only place
    /// that sees every ceiling.
    pub fn multipart_uploads(&self) -> &BTreeMap<String, MultipartUpload> {
        &self.multipart_uploads
    }

    /// Record that the bucket's upload at `(key, upload_id)` is GONE: a completion or an
    /// abort has taken it away.
    ///
    /// Called wherever an upload leaves [`Bucket::multipart_uploads`], so the two addresses
    /// an abort has to tell apart — an upload that is already gone, and an id that never
    /// addressed an upload here — stay distinguishable
    /// ([`Bucket::held_multipart_upload`]). Bounded by
    /// [`MAX_RETIRED_MULTIPART_UPLOADS_PER_BUCKET`], oldest address first, and an address
    /// already recorded is not recorded twice.
    pub(super) fn retire_multipart_upload(&mut self, key: &str, upload_id: &str) {
        if self.held_multipart_upload(key, upload_id) {
            return;
        }
        while self.retired_multipart_uploads.len() >= MAX_RETIRED_MULTIPART_UPLOADS_PER_BUCKET {
            self.retired_multipart_uploads.pop_front();
        }
        self.retired_multipart_uploads
            .push_back((key.to_string(), upload_id.to_string()));
    }

    /// Whether the bucket HELD an upload at `(key, upload_id)` that is now gone — completed,
    /// or already aborted ([`Bucket::retire_multipart_upload`] records it).
    ///
    /// What makes a repeated `AbortMultipartUpload` a no-op rather than a refusal: the state
    /// such an abort asks for already holds. An id this bucket never addressed an upload by
    /// is not one of these.
    pub fn held_multipart_upload(&self, key: &str, upload_id: &str) -> bool {
        self.retired_multipart_uploads
            .iter()
            .any(|(held_key, held_id)| held_key == key && held_id == upload_id)
    }

    /// Delete `key` the way a `DeleteObject` that named NO `versionId` does, reporting
    /// what that did ([`ObjectDeletion`]).
    ///
    /// Which of the three things it does turns on the bucket's versioning state, and
    /// `PutBucketVersioning` / `DeleteObject` document each:
    ///
    /// * versioning status `Enabled` — "the operation inserts a delete marker, which
    ///   becomes the current version of the object. To permanently delete an object in a
    ///   versioned bucket, you must include the object's versionId in the request": the
    ///   marker is PREPENDED to the key's chain under the id `minted`, nothing stored is
    ///   taken away, and every version behind it stays addressable by its own id.
    /// * versioning status `Suspended` — "All objects added to the bucket receive the
    ///   version ID null", and a delete "removes the null version (if there is one) and
    ///   inserts a delete marker with version ID null": the `null` version goes, whether it
    ///   is an object or an older null marker, and a null marker becomes current. Every
    ///   version minted while versioning was enabled stays.
    /// * no versioning state at all — such a bucket has no version history to keep, so the
    ///   `null` version is removed in place and everything it retained is released.
    ///
    /// `minted` is the version id the delete marker of an `Enabled` bucket is stored under,
    /// which only [`ComAmazonawsS3Amazons3State::remove_object`] can mint (it holds the
    /// store's version sequence); it is ignored on any other bucket, whose marker is the
    /// `null` one.
    ///
    /// PRIVATE, so a delete goes through
    /// [`ComAmazonawsS3Amazons3State::remove_object`] and cannot name a bucket that does
    /// not exist. NOTHING is written when the answer is a ceiling: a delete marker is a
    /// stored version like any other, so it is refused rather than recorded once the bucket
    /// is at [`MAX_OBJECTS_PER_BUCKET`] stored versions.
    pub(super) fn remove_object(&mut self, key: &str, minted: Option<String>) -> ObjectDeletion {
        let held_object = self.object(key).is_some();

        // A bucket with no versioning state keeps no history: the delete takes the `null`
        // version away, and with it the last entry of the key's chain.
        if self.versioning.is_none() {
            return if self.remove_null_version(key) {
                ObjectDeletion::Removed
            } else {
                ObjectDeletion::NothingThere
            };
        }

        // An `Enabled` bucket's marker is a version of its own ("All objects added to the
        // bucket receive a unique version ID"); a suspended bucket's is the `null` one,
        // which is why it displaces whatever null version the key held.
        let version_id = if self.versioning_enabled() { minted } else { None };
        let displaces_null = version_id.is_none() && self.null_version_position(key).is_some();
        // The marker is a stored version like any other, so it is refused rather than
        // recorded once the bucket is full — unless it takes the place of the null version
        // it displaces, which leaves the count where it was.
        if !displaces_null && self.stored_versions() >= MAX_OBJECTS_PER_BUCKET {
            return ObjectDeletion::VersionCountReached;
        }
        if displaces_null {
            self.remove_null_version(key);
        }
        self.versions
            .entry(key.to_string())
            .or_default()
            .insert(
                0,
                StoredVersion::DeleteMarker(DeleteMarker {
                    version_id: version_id.clone(),
                    last_modified: SystemTime::now(),
                }),
            );
        ObjectDeletion::MarkerLaid {
            version_id,
            displaced_object: held_object,
        }
    }

    /// Where in `key`'s chain the `null` version is — the one entry that carries no minted
    /// id, object or delete marker — or `None` when the bucket holds none.
    ///
    /// At most one such entry can exist: only a write or a delete made while the bucket was
    /// not versioning-enabled stores one, and each of those displaces the one before it
    /// ("All objects added to the bucket receive the version ID null").
    pub(super) fn null_version_position(&self, key: &str) -> Option<usize> {
        self.versions
            .get(key)?
            .iter()
            .position(|held| held.version_id().is_none())
    }

    /// Remove the `null` version of `key`, reporting whether the bucket held one. See
    /// [`Self::null_version_position`].
    pub(super) fn remove_null_version(&mut self, key: &str) -> bool {
        let Some(position) = self.null_version_position(key) else {
            return false;
        };
        let Some(chain) = self.versions.get_mut(key) else {
            return false;
        };
        chain.remove(position);
        if chain.is_empty() {
            self.versions.remove(key);
        }
        true
    }

    /// The object stored at `key` for writing one of its subresources, or `None` when the
    /// bucket holds no such key.
    ///
    /// PRIVATE, and reached only through
    /// [`ComAmazonawsS3Amazons3State::replace_object_tags`]: that is the one place that
    /// sees the byte budgets a subresource write also has to stay inside, so a handler
    /// reaching an object this way directly would bypass them.
    pub(super) fn object_mut(&mut self, key: &str) -> Option<&mut Object> {
        self.versions.get_mut(key)?.first_mut()?.as_object_mut()
    }

    /// The object version a request addresses, for writing one of its subresources: the
    /// one `version_id` names, or the key's current object when it names none — the
    /// mutable twin of [`Self::addressed_object`].
    ///
    /// PRIVATE, and reached only through
    /// [`ComAmazonawsS3Amazons3State::replace_object_retention`] and
    /// [`ComAmazonawsS3Amazons3State::restore_object_version`], for the reason
    /// [`Self::object_mut`] is private: those are the places that see the byte budgets a
    /// subresource write also has to stay inside.
    pub(super) fn addressed_object_mut(&mut self, key: &str, version_id: Option<&str>) -> Option<&mut Object> {
        match version_id {
            None => self.object_mut(key),
            Some(version_id) => self
                .versions
                .get_mut(key)?
                .iter_mut()
                .find(|held| held.named_by(version_id))?
                .as_object_mut(),
        }
    }

    /// Store `object` at `key` as the key's current version, reporting what that did.
    ///
    /// A replacement, not a merge: "If it receives multiple write requests for the same
    /// object simultaneously, it overwrites all but the last object written", and "You
    /// cannot use PutObject to only update a single piece of metadata for an existing
    /// object. You must put the entire object with updated metadata" (`PutObject`'s own
    /// documentation).
    ///
    /// `minted` is the version id this write mints, which the caller passes exactly when
    /// the bucket's versioning status is `Enabled` — "All objects added to the bucket
    /// receive a unique version ID" (`PutBucketVersioning`).
    ///
    /// What the write does to the version it supersedes is [`Self::keeps_superseded`]: on
    /// a versioning-enabled bucket it is KEPT as a noncurrent version, so both stay
    /// addressable, and so is a version that carries an id of its own whatever the
    /// bucket's status is now; the `null` version of a bucket that is not
    /// versioning-enabled is replaced in place — "Suspended … All objects added to the
    /// bucket receive the version ID null".
    ///
    /// PRIVATE, and reached only through
    /// [`ComAmazonawsS3Amazons3State::put_object`]: the per-bucket ceilings checked here
    /// cannot see the service-wide ones, so a handler calling this directly would bypass
    /// them. NOTHING is written unless the answer is [`ObjectWrite::Created`] or
    /// [`ObjectWrite::Replaced`]: an over-long key, metadata past
    /// [`MAX_OBJECT_METADATA_BYTES`], a bucket at [`MAX_OBJECTS_PER_BUCKET`] stored
    /// versions and a write past [`MAX_OBJECT_BYTES_PER_BUCKET`] retained bytes are each
    /// refused here. Replacing an object releases everything it retained, so the budget is
    /// over what the bucket WOULD hold, not over everything ever written — while a
    /// versioned write releases nothing, because the version it supersedes is still there.
    pub(super) fn put_object(&mut self, key: &str, mut object: Object, minted: Option<String>) -> ObjectWrite {
        if key.len() > MAX_OBJECT_KEY_BYTES {
            return ObjectWrite::KeyTooLong { bytes: key.len() };
        }
        let metadata_bytes = object.metadata_bytes();
        if metadata_bytes > MAX_OBJECT_METADATA_BYTES {
            return ObjectWrite::MetadataTooLarge {
                bytes: metadata_bytes,
            };
        }
        // The id is on the object before anything is measured, so the bytes it retains are
        // inside the budgets like every other string kept beside the payload.
        object.version_id = minted;
        // "The rule specified in the Object Lock configuration will be applied by default to
        // every new object placed in the specified bucket"
        // (`PutObjectLockConfiguration`'s own documentation), so a bucket carrying a default
        // retention locks the version this write stores — counted from that version's own
        // creation time, and only where the write placed no retention itself, because a
        // retention the request names is the one the object is stored with.
        //
        // Here, because this is the only place that sees both the bucket's configuration and
        // the object being written: every writer gets the default rather than each one
        // re-deriving it. Before the budgets below, so the bytes it retains are inside them.
        if object.retention.is_none() {
            object.retention = self
                .object_lock_rule
                .as_ref()
                .and_then(|rule| rule.retention_at(object.last_modified));
        }
        let retaining = self.keeps_superseded(key);
        let held = self.object(key).is_some();
        // The write adds an entry unless it replaces the key's current version in place,
        // which it does only when that version is not retained and there is one.
        let adds_version = retaining || self.current_version(key).is_none();
        if adds_version && self.stored_versions() >= MAX_OBJECTS_PER_BUCKET {
            return ObjectWrite::ObjectCountReached;
        }
        let would_hold = self.retained_bytes_after_write(key, retaining) + object.retained_bytes();
        if would_hold > MAX_OBJECT_BYTES_PER_BUCKET {
            return ObjectWrite::ByteBudgetReached { would_hold };
        }
        let chain = self.versions.entry(key.to_string()).or_default();
        if adds_version {
            chain.insert(0, StoredVersion::Object(Box::new(object)));
        } else {
            chain[0] = StoredVersion::Object(Box::new(object));
        }
        if held {
            ObjectWrite::Replaced
        } else {
            ObjectWrite::Created
        }
    }

    /// Every key the bucket holds at least one version of, in ascending UTF-8 byte order
    /// — the keys with a current object, plus the keys whose only versions left are
    /// noncurrent ones or delete markers.
    ///
    /// The read seam for a listing over VERSIONS (`ListObjectVersions`), the way
    /// [`Self::objects`] is the seam for a listing over current objects. Ascending order,
    /// because that is the order S3 lists keys in.
    pub fn version_keys(&self) -> Vec<&str> {
        self.versions.keys().map(String::as_str).collect()
    }

    /// Remove the version of `key` that `version_id` names, reporting whether the bucket
    /// held such a version.
    ///
    /// "To permanently delete an object in a versioned bucket, you must include the
    /// object's versionId in the request" (`DeleteObject`'s documentation). Removing the
    /// key's CURRENT version promotes the newest version left to current, which is what
    /// makes a versioned bucket's delete reversible: the version before it becomes what a
    /// read without a `versionId` answers with. Everything the removed version retained is
    /// released with it.
    ///
    /// PRIVATE, so a delete goes through
    /// [`ComAmazonawsS3Amazons3State::remove_object_version`] and cannot name a bucket
    /// that does not exist.
    pub(super) fn remove_version(&mut self, key: &str, version_id: &str) -> bool {
        let Some(chain) = self.versions.get_mut(key) else {
            return false;
        };
        let Some(position) = chain.iter().position(|held| held.named_by(version_id)) else {
            return false;
        };
        // Removing entry 0 promotes the version behind it to current, because the chain is
        // newest first: that is what makes a versioned bucket's delete reversible — the
        // version before it becomes what a read without a `versionId` answers with.
        chain.remove(position);
        if chain.is_empty() {
            self.versions.remove(key);
        }
        true
    }

    /// Whether a write or a delete at `key` KEEPS the version it displaces rather than
    /// releasing it.
    ///
    /// Two things keep it. A bucket whose versioning status is `Enabled` keeps every
    /// version it already holds, including one stored before versioning was enabled ("All
    /// objects added to the bucket receive a unique version ID", and the objects that were
    /// there keep "a version ID of null"). And a version that carries an id of its own is
    /// kept whatever the bucket's status is now, because nothing but a delete naming that
    /// id takes it away ("To permanently delete an object in a versioned bucket, you must
    /// include the object's versionId in the request") — which is what makes suspension a
    /// state of the bucket rather than of the versions it holds.
    ///
    /// What is left is the `null` version of a bucket that is not versioning-enabled:
    /// "Suspended … All objects added to the bucket receive the version ID null", so a
    /// write replaces it and a delete removes it, in place.
    pub(super) fn keeps_superseded(&self, key: &str) -> bool {
        self.versioning_enabled()
            || self
                .current_version(key)
                .is_some_and(|held| held.version_id().is_some())
    }

    /// Whether the bucket holds NOTHING at all: no version of any key — no current object,
    /// no noncurrent version retained behind one, and no delete marker.
    ///
    /// This is the emptiness `DeleteBucket` is over — "All objects (including all object
    /// versions and delete markers) in the bucket must be deleted before the bucket itself
    /// can be deleted" (that operation's own documentation) — so it spans the bucket's
    /// whole version history rather than its current view. [`Self::objects`] alone would
    /// report a versioning-enabled bucket whose every key has been deleted as empty, while
    /// each of those deletes laid a delete marker over a version that is still stored and
    /// still addressable by its id: both of them have to be deleted first.
    pub fn holds_no_versions(&self) -> bool {
        self.stored_versions() == 0
    }

    /// How many versions this bucket stores, over every key: each key's current version
    /// plus every noncurrent one it keeps, delete markers included.
    ///
    /// What [`MAX_OBJECTS_PER_BUCKET`] counts, because a retained noncurrent version holds
    /// this process's memory exactly as a current one does.
    pub fn stored_versions(&self) -> usize {
        self.versions.values().map(Vec::len).sum()
    }

    /// How many bytes this bucket's objects retain — payloads AND every string kept
    /// beside them, over every version it stores, so no part of a stored object escapes
    /// the budgets.
    ///
    /// The parts of the multipart uploads it has in progress are counted too: a part held
    /// against an upload occupies this process's memory exactly as a stored object's
    /// payload does, and is released only when a completion or an abort takes the upload
    /// away — so the two share one budget rather than each having its own.
    pub(super) fn retained_bytes(&self) -> usize {
        self.versions
            .values()
            .flatten()
            .map(StoredVersion::retained_bytes)
            .sum::<usize>()
            + self
                .multipart_uploads
                .values()
                .map(MultipartUpload::retained_bytes)
                .sum::<usize>()
    }

    /// How many bytes this bucket would still retain with a write at `key` applied, not
    /// counting the incoming object itself.
    ///
    /// Everything it holds now, less the key's current version when that version is
    /// replaced in place — which is every write except one that keeps what it supersedes.
    pub(super) fn retained_bytes_after_write(&self, key: &str, retaining_current: bool) -> usize {
        let released = if retaining_current {
            0
        } else {
            self.current_version(key)
                .map_or(0, StoredVersion::retained_bytes)
        };
        self.retained_bytes().saturating_sub(released)
    }
}
