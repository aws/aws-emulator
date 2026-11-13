// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-customerprofiles-customerprofiles-20200815 service.
//!
//! The service's one aggregate is the DOMAIN — "a container for all customer data, such
//! as customer profile attributes, object types, profile keys, and encryption keys"
//! (`CreateDomain`). Every profile, and the searchable key index profiles are found
//! through, hangs off its domain, so `DeleteDomain` ("Deletes a specific domain and all
//! of its customer data, such as customer profile attributes and their related objects")
//! drops the whole subtree by removing one map entry.
//!
//! Domains are held in a `BTreeMap` and a domain's profiles in a `BTreeMap` keyed by
//! profile id, so every listing and search walks state in one stable order — which is
//! what makes the `NextToken` cursor below resumable.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::time::{Duration, SystemTime};

use crate::ops::types::{
    Address, AttributeDetails, Conditions, CreateCalculatedAttributeDefinitionInput,
    DataStoreResponse, Filter, MatchingRequest, MatchingResponse, ObjectTypeField, ObjectTypeKey,
    Profile, Readiness, RecommenderSchemaField, ResultsSummary, RuleBasedMatchingRequest,
    RuleBasedMatchingResponse,
};

/// How long after the call that configures it the asynchronous work a domain kicks off
/// is treated as finished: the rule-based matching rule reaches `ACTIVE`, and an enabled
/// data store reaches 100% readiness.
///
/// AWS does not document a duration for either, so this is a CHOSEN span. It is
/// deliberately non-zero: a caller that reads the domain back immediately must still
/// observe the transient state the create call reported.
pub const ASYNC_WORK_SPAN: Duration = Duration::from_millis(500);

/// The page size applied when a caller sends no `MaxResults`. `SearchProfiles`
/// documents its `MaxResults` as "The maximum number of objects returned per page. The
/// default is 20 if this parameter is not included in the request." `ListDomains`
/// documents no default for its own `MaxResults` ("The maximum number of objects
/// returned per page"), so it serves the one this service states, which is this value.
pub const DEFAULT_PAGE_SIZE: usize = 20;

/// The largest number of search values one `SearchProfiles` request may carry in total,
/// across its required `Values` list and every `AdditionalSearchKeys[*].Values` list.
///
/// The model bounds the number of search KEYS (one required plus up to four additional)
/// but puts no length on a key's value list, so this is a CHOSEN protective cap. It
/// bounds the index probes, and with [`DEFAULT_PAGE_SIZE`]/`MaxResults` bounding the
/// page it bounds the PRODUCT of page size and value count that describing a page costs.
/// It is far above any real search, which carries a handful of values.
pub const MAX_SEARCH_VALUES: usize = 100;

// ---------------------------------------------------------------------------
// Store bounds
//
// Every record below is created by a caller and reclaimed only by an explicit delete, so
// each growth site carries a cap checked BEFORE the insert and nothing caller-sized
// reaches the store unchecked.
//
// The product is therefore finite: at most MAX_DOMAINS domains, each holding at most
// MAX_PROFILES_PER_DOMAIN profiles, each profile indexed under at most the handful of
// standard keys `standard_search_keys` can derive from it, and each caller-sized map on
// a record (`Tags` 1..50, `Attributes` entries 1..255 characters) bounded by the length
// the model declares for it and refused at the boundary before a handler runs.
// ---------------------------------------------------------------------------

/// Customer Profiles' published quota "Domains per AWS account per Region: 100".
pub const MAX_DOMAINS: usize = 100;

/// Emulator resource guard, NOT an AWS quota (the service publishes no per-domain
/// profile limit): the most profiles this emulator stores for one domain. Set well above
/// any realistic emulator workload.
pub const MAX_PROFILES_PER_DOMAIN: usize = 10_000;

/// Emulator resource guard, NOT an AWS quota (the service publishes no limit on a
/// profile's attribute map): the most attributes this emulator keeps on one profile.
///
/// `UpdateProfile` MERGES the attributes a request carries into the ones already stored,
/// so one request's own map does not bound what accumulates on a profile: the cap is
/// checked against the MERGED map, before it is stored, which is what keeps repeated
/// updates from growing one profile without bound. Set well above any realistic emulator
/// workload.
pub const MAX_ATTRIBUTES_PER_PROFILE: usize = 1_000;

/// Emulator resource guard, NOT an AWS quota (the service publishes no limit on the key
/// values one profile carries): the most key values this emulator associates with one
/// profile, across every key name.
///
/// `AddProfileKey` ACCUMULATES values on a profile, so one request's `Values` list does
/// not bound what a sequence of requests stores: the cap is checked against the merged
/// set, before anything is written, which is what keeps repeated calls from growing one
/// profile without bound. Set well above any realistic emulator workload.
pub const MAX_KEY_VALUES_PER_PROFILE: usize = 1_000;

/// Emulator resource guard, NOT an AWS quota: the most profile object types this
/// emulator stores for one domain. Set well above any realistic emulator workload, and
/// checked before the insert so repeated `PutProfileObjectType` calls cannot grow the
/// store without bound.
pub const MAX_OBJECT_TYPES_PER_DOMAIN: usize = 1_000;

/// Emulator resource guard, NOT an AWS quota (the service publishes no per-domain
/// profile object limit): the most profile objects this emulator stores for one domain,
/// across every object type and profile in it. Checked before the insert, so repeated
/// `PutProfileObject` calls cannot grow the store without bound, and set well above any
/// realistic emulator workload.
pub const MAX_PROFILE_OBJECTS_PER_DOMAIN: usize = 10_000;

/// Seconds in one day — the unit every expiry this service measures in days is held in
/// (`ProfileObjectType$ExpirationDays`, `Domain$DefaultExpirationDays`).
pub const SECONDS_PER_DAY: u64 = 24 * 60 * 60;

/// Emulator resource guard, NOT an AWS quota: the most upload jobs this emulator stores
/// for one domain. Set well above any realistic emulator workload, and checked before
/// the insert so repeated `CreateUploadJob` calls cannot grow the store without bound.
pub const MAX_UPLOAD_JOBS_PER_DOMAIN: usize = 1_000;

/// Emulator resource guard, NOT an AWS quota: the most calculated attribute definitions
/// this emulator stores for one domain. Set well above any realistic emulator workload,
/// and checked before the insert so repeated `CreateCalculatedAttributeDefinition` calls
/// cannot grow the store without bound.
pub const MAX_CALCULATED_ATTRIBUTES_PER_DOMAIN: usize = 1_000;

/// How long after `CreateCalculatedAttributeDefinition` the indexing of the historical
/// data the definition asked for is treated as finished.
///
/// AWS documents no duration for it, so this is a CHOSEN span. It is deliberately
/// non-zero: a caller that reads the definition back immediately must still observe the
/// transient status the create call reported.
pub const HISTORICAL_DATA_INDEXING_SPAN: Duration = Duration::from_secs(3);

/// Emulator resource guard, NOT an AWS quota: the most recommender schemas this emulator
/// stores for one domain. Set well above any realistic emulator workload, and checked
/// before the insert so repeated `CreateRecommenderSchema` calls cannot grow the store
/// without bound.
pub const MAX_RECOMMENDER_SCHEMAS_PER_DOMAIN: usize = 1_000;

/// The `RecommenderSchemaStatus` a stored recommender schema reports. The status has two
/// modelled values, `ACTIVE` and `DELETING`, and a schema is usable for training as soon
/// as it is defined, so a stored one is active and nothing but its removal moves it.
pub const RECOMMENDER_SCHEMA_ACTIVE: &str = "ACTIVE";

/// The dataset types a recommender schema may carry columns for, as
/// `CreateRecommenderSchemaRequest$Fields` states them: "The `_webAnalytics` and
/// `_catalogItem` keys are supported." Sorted, so a refusal lists them in one order.
pub const SUPPORTED_RECOMMENDER_DATASET_TYPES: [&str; 2] = ["_catalogItem", "_webAnalytics"];

/// The `ContentType` of a column holding text — the value
/// `RecommenderSchemaField$ContentType` documents as its default.
pub const CONTENT_TYPE_STRING: &str = "STRING";

/// The `ContentType` of a column holding numbers.
pub const CONTENT_TYPE_NUMBER: &str = "NUMBER";

/// The `BadRequestException` message for a dataset type the domain holds no data table
/// for, worded as this service words a resource of a kind it cannot find: the kind, the
/// name the request carried, and "not found" (the same form `ObjectType {name} not found`
/// takes).
pub fn data_table_not_found(dataset_type: &str) -> String {
    format!("DataTable {dataset_type} not found")
}

/// The fewest columns one dataset type of a recommender schema may list: the `@length`
/// minimum of `RecommenderSchemaFieldList`.
pub const MIN_RECOMMENDER_SCHEMA_COLUMNS: usize = 1;

/// The most columns one dataset type of a recommender schema may list: the `@length`
/// maximum of `RecommenderSchemaFieldList`.
pub const MAX_RECOMMENDER_SCHEMA_COLUMNS: usize = 9;

/// This service's ARN namespace — the `profile` every `Arn` on its wire carries
/// (`^arn:([a-z\d-]+):profile:.*:.*:.+$`), which is also its SigV4 signing name.
pub const ARN_SERVICE: &str = "profile";

/// The region an ARN is built in when the request carries no SigV4 credential scope to
/// read one from: the emulator's documented default region.
pub const DEFAULT_REGION: &str = "us-east-1";

/// `ReadinessStatus` while the historical data a calculated attribute asked for is still
/// being indexed — the first of the modelled statuses.
pub const READINESS_PREPARING: &str = "PREPARING";

/// `ReadinessStatus` once every object the calculated attribute covers is indexed, so the
/// attribute is ready for use.
pub const READINESS_COMPLETED: &str = "COMPLETED";

/// The `DataExpiry` an upload job gets when the request carries none.
/// `CreateUploadJobRequest$DataExpiry` documents it: "The expiry duration for the
/// profiles ingested with the job. If not provided, the system default of 2 weeks is
/// used." Two weeks, in the days the member is measured in.
pub const DEFAULT_UPLOAD_JOB_DATA_EXPIRY_DAYS: i32 = 14;

/// The `UploadJobStatus` a job is created in: "CREATED: The upload job has been created,
/// but has not started processing yet." Nothing but a later call moves a job out of it,
/// so it is not a clock-owed transition.
pub const UPLOAD_JOB_CREATED: &str = "CREATED";

/// `RuleBasedMatchingStatus` while the rule is being built. Its documentation: "The
/// system is creating the rule-based matching rule." A domain created with rule-based
/// matching enabled starts here rather than at `PENDING`, because `PENDING` is described
/// as the wait an EXISTING domain serves — "If it is a new domain, the system will skip
/// the PENDING stage."
pub const RULE_BASED_MATCHING_IN_PROGRESS: &str = "IN_PROGRESS";

/// `RuleBasedMatchingStatus` once the rule is built: "The rule is ready to use."
pub const RULE_BASED_MATCHING_ACTIVE: &str = "ACTIVE";

/// Predefined `SearchProfiles` key naming a profile by its own id.
pub const KEY_PROFILE_ID: &str = "_profileId";
/// Predefined `SearchProfiles` key naming a profile by its account number.
pub const KEY_ACCOUNT: &str = "_account";
/// Predefined `SearchProfiles` key naming a profile by any of its email addresses.
pub const KEY_EMAIL: &str = "_email";
/// Predefined `SearchProfiles` key naming a profile by any of its phone numbers.
pub const KEY_PHONE: &str = "_phone";
/// Predefined `SearchProfiles` key naming a profile by its full name.
pub const KEY_FULL_NAME: &str = "_fullName";

/// Per-service state for com-amazonaws-customerprofiles-customerprofiles-20200815. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsCustomerprofilesCustomerprofiles20200815State {
    /// Every domain this emulator holds, keyed by `DomainName` — the name is the
    /// domain's identity (`@httpLabel` on every domain-scoped operation).
    pub domains: BTreeMap<String, Domain>,
    /// The secret this store's continuation tokens are authenticated under, so a
    /// `NextToken` a caller BUILT is told from one a read ISSUED ([`page_cursor`]).
    ///
    /// Drawn from the system's randomness when the store is created and never reported:
    /// `PageTokenKey`'s own `Debug` renders nothing of it, so this state's `Debug` carries
    /// no key material either.
    pub page_tokens: ::service_common::page_tokens::PageTokenKey,
}

/// One span of asynchronous work the clock owes a resource: when the call that started
/// it ran, and when it falls due. Stored as instants so no handler ever sleeps for one,
/// and held per piece of work, so one resource reaching its terminal state is independent
/// of another's.
#[derive(Debug, Clone, Copy)]
pub struct AsyncWork {
    /// The instant the call that started this work ran.
    pub started_at: SystemTime,
    /// The instant it falls due, one [`ASYNC_WORK_SPAN`] later.
    pub due_at: SystemTime,
}

impl AsyncWork {
    /// Work started by a call made at `now`.
    pub fn started(now: SystemTime) -> Self {
        Self {
            started_at: now,
            due_at: now + ASYNC_WORK_SPAN,
        }
    }

    /// Whether this work is due as of `now`.
    pub fn is_due(&self, now: SystemTime) -> bool {
        now >= self.due_at
    }

    /// How far through its span this work is, as a percentage in 0..=100 (the range
    /// `Readiness$ProgressPercentage` declares).
    pub fn progress_percentage(&self, now: SystemTime) -> i32 {
        progress_percentage(self.started_at, self.due_at, now)
    }
}

/// One stored domain: the configuration `CreateDomain` persisted, the timestamps it
/// generated, and the customer data nested under it.
#[derive(Debug, Clone)]
pub struct Domain {
    /// `DomainName`, as stored (also this domain's key in [`ComAmazonawsCustomerprofilesCustomerprofiles20200815State::domains`]).
    pub name: String,
    /// `DefaultExpirationDays`, as stored.
    pub default_expiration_days: i32,
    /// `DefaultEncryptionKey`, as stored.
    pub default_encryption_key: Option<String>,
    /// `DeadLetterQueueUrl`, as stored.
    pub dead_letter_queue_url: Option<String>,
    /// `Matching`, as the wire reports it. `MatchingResponse` has no status member, so
    /// the stored value is the whole answer.
    pub matching: Option<MatchingResponse>,
    /// `RuleBasedMatching`, as stored, with `Status` left unset: the status is a
    /// function of the clock, so it is computed on every read by
    /// [`Domain::rule_based_matching_response`] rather than stored.
    pub rule_based_matching: Option<RuleBasedMatchingResponse>,
    /// Whether `DataStore` was requested enabled, as stored. The readiness beside it on
    /// the wire is a function of the clock — see [`Domain::data_store_response`].
    pub data_store_enabled: Option<bool>,
    /// `Tags`, as stored.
    pub tags: Option<HashMap<String, String>>,
    /// `CreatedAt`: generated once, here, and never recomputed.
    pub created_at: SystemTime,
    /// `LastUpdatedAt`: the last time the DOMAIN's own configuration changed. Writing a
    /// profile is not an edit of the domain, so it does not move this.
    pub last_updated_at: SystemTime,
    /// The span the rule-based matching rule takes to build, as started by the call that
    /// configured it. Its own span, so a status that has reached `ACTIVE` does not
    /// regress because an unrelated member of the same domain was configured.
    pub rule_based_matching_work: AsyncWork,
    /// The span the data store takes to prepare, as started by the call that enabled it.
    /// Its own span, for the same reason.
    pub data_store_work: AsyncWork,
    /// The domain's profiles, keyed by `ProfileId`.
    pub profiles: BTreeMap<String, StoredProfile>,
    /// The domain's profile object types, keyed by `ObjectTypeName` — the name is the
    /// object type's identity (`@httpLabel` on every object-type operation).
    pub object_types: BTreeMap<String, StoredObjectType>,
    /// The domain's ingested profile objects, keyed by [`ProfileObjectId`]. The object
    /// type and the profile lead the key, so the objects `ListProfileObjects` serves for
    /// one of those pairs are a contiguous range walked in ascending unique-key order —
    /// which is what makes that listing's `NextToken` cursor resumable.
    pub profile_objects: BTreeMap<ProfileObjectId, StoredProfileObject>,
    /// The domain's upload jobs, keyed by the `JobId` `CreateUploadJob` minted — the id
    /// is the job's identity (`@httpLabel` on every upload-job operation). A `BTreeMap`,
    /// so a listing walks them in one stable order.
    pub upload_jobs: BTreeMap<String, StoredUploadJob>,
    /// The domain's calculated attribute definitions, keyed by `CalculatedAttributeName`
    /// — the name is the definition's identity (`@httpLabel` on every
    /// calculated-attribute operation), so the (domain, name) pair identifies exactly
    /// one definition. A `BTreeMap`, so a listing walks them in one stable order.
    pub calculated_attributes: BTreeMap<String, StoredCalculatedAttribute>,
    /// The domain's recommender schemas, keyed by `RecommenderSchemaName` — the name is
    /// the schema's identity within the domain (`@httpLabel` on every recommender-schema
    /// operation), so the (domain, name) pair identifies exactly one schema. A
    /// `BTreeMap`, so a listing walks them in one stable order.
    pub recommender_schemas: BTreeMap<String, StoredRecommenderSchema>,
    /// `KeyName` -> `Value` -> the ids of the profiles indexed under that pair. This is
    /// what `SearchProfiles` reads; [`Domain::put_profile`] fills it and
    /// [`Domain::remove_profile`] empties it. Nested rather than keyed by a
    /// `(KeyName, Value)` tuple so a lookup probes it with borrowed strings and
    /// allocates nothing per probe.
    pub key_index: HashMap<String, HashMap<String, BTreeSet<String>>>,
    /// `ProfileId` -> `KeyName` -> the values `AddProfileKey` associated with that
    /// profile under that key. The profile side of the index above: [`key_index`] answers
    /// "which profiles carry this pair", this answers "which pairs does this profile
    /// carry", which is what un-indexing a profile and reporting `FoundByItems` need.
    ///
    /// Only the ADDITIONAL keys live here. The predefined keys a profile's own attributes
    /// populate are derived from the profile by [`standard_search_keys`], so they are
    /// never stored twice.
    ///
    /// [`key_index`]: Domain::key_index
    pub profile_keys: BTreeMap<String, BTreeMap<String, BTreeSet<String>>>,
}

/// One stored profile object type: the configuration `PutProfileObjectType` persisted
/// and the timestamps the service keeps for it.
///
/// `MaxAvailableProfileObjectCount` is NOT stored: it is the unused part of the count
/// assigned to the type, so it is computed on every read by
/// [`StoredObjectType::max_available_profile_object_count`].
#[derive(Debug, Clone)]
pub struct StoredObjectType {
    /// `ObjectTypeName`, as stored (also this record's key in
    /// [`Domain::object_types`]).
    pub name: String,
    /// `Description`, as stored.
    pub description: String,
    /// `TemplateId`, as stored. When it is set the template supplies the members
    /// `PutProfileObjectType` refuses beside it, so those stay unset here.
    pub template_id: Option<String>,
    /// `ExpirationDays`, as stored.
    pub expiration_days: Option<i32>,
    /// `EncryptionKey`, as stored.
    pub encryption_key: Option<String>,
    /// `AllowProfileCreation`, as stored. The member is modelled with a default of
    /// `false`, so an absent one is stored as `false`.
    pub allow_profile_creation: bool,
    /// `SourceLastUpdatedTimestampFormat`, as stored.
    pub source_last_updated_timestamp_format: Option<String>,
    /// `MaxProfileObjectCount`, as stored.
    pub max_profile_object_count: Option<i32>,
    /// `SourcePriority`, as stored.
    pub source_priority: Option<i32>,
    /// `Fields`, as stored.
    pub fields: Option<HashMap<String, ObjectTypeField>>,
    /// `Keys`, as stored.
    pub keys: Option<HashMap<String, Vec<ObjectTypeKey>>>,
    /// `Tags`, as stored.
    pub tags: Option<HashMap<String, String>>,
    /// `CreatedAt`: generated when the object type was first defined, and kept when a
    /// later put redefines it.
    pub created_at: SystemTime,
    /// `LastUpdatedAt`: the last time the definition was written.
    pub last_updated_at: SystemTime,
}

impl StoredObjectType {
    /// `MaxAvailableProfileObjectCount` — "the amount of provisioned profile object max
    /// count available": the count assigned to this type less the profile objects held
    /// under it. A type with no assigned count reports none.
    ///
    /// `objects_held` is the number of profile objects the domain stores for this type,
    /// which the caller counts — see [`Domain::profile_object_count`].
    pub fn max_available_profile_object_count(&self, objects_held: i32) -> Option<i32> {
        self.max_profile_object_count
            .map(|assigned| assigned.saturating_sub(objects_held).max(0))
    }
}

/// One stored upload job: the metadata `CreateUploadJob` persisted ("The metadata is
/// created for the job with the provided field mapping and unique key") and the progress
/// the service keeps for it.
///
/// The progress members are stored rather than derived: a job leaves
/// [`UPLOAD_JOB_CREATED`] only when a caller starts or stops it, never because the clock
/// moved, so a read reports exactly what the last call wrote.
#[derive(Debug, Clone)]
pub struct StoredUploadJob {
    /// `JobId`, as minted (also this record's key in [`Domain::upload_jobs`]).
    pub job_id: String,
    /// `DisplayName`, as stored.
    pub display_name: String,
    /// `Fields`, as stored: "The mapping between CSV Columns and Profile Object
    /// attributes."
    pub fields: HashMap<String, ObjectTypeField>,
    /// `UniqueKey`, as stored.
    pub unique_key: String,
    /// `DataExpiry`, as stored: the request's value, or
    /// [`DEFAULT_UPLOAD_JOB_DATA_EXPIRY_DAYS`] when it carried none.
    pub data_expiry: i32,
    /// `Status`, one of the modelled `UploadJobStatus` values.
    pub status: String,
    /// `StatusReason`: why the job is in its current status, set only by a status that
    /// has one.
    pub status_reason: Option<String>,
    /// `CreatedAt`: generated once, when the job was created, and never recomputed.
    pub created_at: SystemTime,
    /// `CompletedAt`: when the job finished processing, so unset until it has.
    pub completed_at: Option<SystemTime>,
    /// `ResultsSummary`: "The summary of results for the upload job, including the
    /// number of updated, created, and failed records." The summary itself is part of
    /// every job record from the moment it is created — a reader of a stored job always
    /// gets the structure — and it is its three counters that stay unset until the job
    /// has processed records.
    pub results_summary: ResultsSummary,
}

impl StoredUploadJob {
    /// A freshly created job: the metadata the request carried, in
    /// [`UPLOAD_JOB_CREATED`] with nothing processed yet.
    pub fn new(
        job_id: String,
        display_name: String,
        fields: HashMap<String, ObjectTypeField>,
        unique_key: String,
        data_expiry: i32,
        now: SystemTime,
    ) -> Self {
        Self {
            job_id,
            display_name,
            fields,
            unique_key,
            data_expiry,
            status: UPLOAD_JOB_CREATED.to_string(),
            status_reason: None,
            created_at: now,
            completed_at: None,
            // Nothing has been processed yet, so the summary is present with none of
            // its three counters populated.
            results_summary: ResultsSummary::default(),
        }
    }
}

/// One stored calculated attribute definition: the configuration
/// `CreateCalculatedAttributeDefinition` persisted and the timestamps the service keeps
/// for it.
///
/// `Status` and `Readiness` are NOT stored: they report how far the indexing of the
/// definition's historical data has got, which is a function of the clock, so both are
/// computed on every read from [`StoredCalculatedAttribute::indexed_at`].
#[derive(Debug, Clone)]
pub struct StoredCalculatedAttribute {
    /// `CalculatedAttributeName`, as stored (also this record's key in
    /// [`Domain::calculated_attributes`]).
    pub name: String,
    /// `DisplayName`, as stored.
    pub display_name: Option<String>,
    /// `Description`, as stored.
    pub description: Option<String>,
    /// `AttributeDetails`, as stored: the expression and the attributes it names.
    pub attribute_details: AttributeDetails,
    /// `Conditions`, as stored.
    pub conditions: Option<Conditions>,
    /// `Filter`, as stored.
    pub filter: Option<Filter>,
    /// `Statistic`, one of the modelled `Statistic` values.
    pub statistic: String,
    /// `UseHistoricalData`, as stored: whether data ingested before the definition was
    /// created is included, which is what gives the definition anything to index.
    pub use_historical_data: Option<bool>,
    /// `Tags`, as stored.
    pub tags: Option<HashMap<String, String>>,
    /// `CreatedAt`: generated once, when the definition was created, and never
    /// recomputed.
    pub created_at: SystemTime,
    /// `LastUpdatedAt`: the last time the definition was written.
    pub last_updated_at: SystemTime,
    /// When the indexing [`HISTORICAL_DATA_INDEXING_SPAN`] covers falls due. Stored as an
    /// instant so no handler ever sleeps for it, and equal to `created_at` for a
    /// definition that asked for no historical data: there is nothing to index, so it is
    /// ready at once.
    pub indexed_at: SystemTime,
}

impl StoredCalculatedAttribute {
    /// A freshly created definition: the configuration the request carried, taken from the
    /// already-validated request itself so no member can be transposed on the way in, with
    /// the instant its historical-data indexing falls due. A definition that did not ask
    /// for historical data has nothing to index, so that instant is `now`.
    pub fn from_request(input: CreateCalculatedAttributeDefinitionInput, now: SystemTime) -> Self {
        let indexed_at = if input.use_historical_data == Some(true) {
            now + HISTORICAL_DATA_INDEXING_SPAN
        } else {
            now
        };
        Self {
            name: input.calculated_attribute_name,
            display_name: input.display_name,
            description: input.description,
            attribute_details: input.attribute_details,
            conditions: input.conditions,
            filter: input.filter,
            statistic: input.statistic,
            use_historical_data: input.use_historical_data,
            tags: input.tags,
            created_at: now,
            last_updated_at: now,
            indexed_at,
        }
    }

    /// Whether every object this definition covers is indexed as of `now`.
    pub fn is_indexed(&self, now: SystemTime) -> bool {
        now >= self.indexed_at
    }

    /// `Status` as the wire reports it: the indexing is either still under way or done.
    pub fn status(&self, now: SystemTime) -> String {
        if self.is_indexed(now) {
            READINESS_COMPLETED.to_string()
        } else {
            READINESS_PREPARING.to_string()
        }
    }

    /// `Readiness` as the wire reports it: how far the indexing has got, as a percentage
    /// in 0..=100 (the range `Readiness$ProgressPercentage` declares), and a sentence
    /// saying whether the attribute can be used yet.
    ///
    /// `Readiness$Message` is documented only as "Any customer messaging", so the two
    /// sentences below are CHOSEN wording, in the same form as the one
    /// [`Domain::data_store_response`] reports for a data store.
    pub fn readiness(&self, now: SystemTime) -> Readiness {
        Readiness {
            progress_percentage: Some(progress_percentage(self.created_at, self.indexed_at, now)),
            message: Some(
                if self.is_indexed(now) {
                    "All historical data has been processed."
                } else {
                    "Historical data is being processed."
                }
                .to_string(),
            ),
        }
    }
}

/// One stored recommender schema: "the set of data columns available for training
/// recommenders and filters under a domain" (`CreateRecommenderSchema`), as the create
/// persisted it, plus the ARN it was minted with and the instant it was created.
///
/// `Status` is NOT stored: a defined schema is usable, so it reports
/// [`RECOMMENDER_SCHEMA_ACTIVE`] for as long as the record exists.
#[derive(Debug, Clone)]
pub struct StoredRecommenderSchema {
    /// `RecommenderSchemaName`, as stored (also this record's key in
    /// [`Domain::recommender_schemas`]).
    pub name: String,
    /// `RecommenderSchemaArn`: minted once, when the schema was created, so a later read
    /// reports the ARN the schema was created with.
    pub arn: String,
    /// `Fields`, as stored: the dataset type -> column definitions map the request
    /// carried.
    pub fields: HashMap<String, Vec<RecommenderSchemaField>>,
    /// `Tags`, as stored.
    pub tags: Option<HashMap<String, String>>,
    /// `CreatedAt`: generated once, when the schema was created, and never recomputed.
    pub created_at: SystemTime,
}

impl StoredRecommenderSchema {
    /// `Status` as the wire reports it: a stored schema is active.
    pub fn status(&self) -> String {
        RECOMMENDER_SCHEMA_ACTIVE.to_string()
    }
}

/// One stored profile: the modelled shape plus the timestamps the service keeps for it.
#[derive(Debug, Clone)]
pub struct StoredProfile {
    /// The profile exactly as it is reported back, `ProfileId` included. `FoundByItems`
    /// is NOT stored: it describes which search keys matched one particular search, so
    /// `SearchProfiles` computes it per response.
    pub profile: Profile,
    /// When the profile was created.
    pub created_at: SystemTime,
    /// When the profile was last written.
    pub last_updated_at: SystemTime,
}

/// The identity of one ingested profile object within a domain: the `ObjectTypeName` it
/// was ingested under, the `ProfileId` it is tied to, and its `ProfileObjectUniqueKey`.
///
/// The profile is empty for an object the ingest could tie to no profile. A `ProfileId`
/// is 32 hexadecimal characters (`uuid`, `^[a-f0-9]{32}$`), so no `ListProfileObjects` or
/// `DeleteProfileObject` request can name that profile: an object no profile claims is
/// held, and is unreachable, exactly as the service leaves one it could not map.
pub type ProfileObjectId = (String, String, String);

/// One ingested profile object: the JSON `PutProfileObject` was given, the unique key the
/// service derived for it, the profile it was tied to, and the index values its object
/// type's keys select out of it.
#[derive(Debug, Clone)]
pub struct StoredProfileObject {
    /// `ObjectTypeName`: the type the object was ingested under.
    pub object_type_name: String,
    /// `ProfileObjectUniqueKey`, as derived by [`profile_object_unique_key`].
    pub unique_key: String,
    /// `Object`: the JSON the request carried, stored as given so a listing reports the
    /// object that was ingested rather than a re-rendering of it.
    pub object: String,
    /// The `ProfileId` this object is tied to, or empty when it is tied to none — see
    /// [`ProfileObjectId`].
    pub profile_id: String,
    /// `KeyName` -> the values this object carries for that key: what an
    /// `ObjectFilter` on `ListProfileObjects` selects by ("include profile objects with
    /// the specified index values"). A `LOOKUP_ONLY` key contributes none, because it
    /// "is only used to match a profile but is not persisted to be used for searching".
    pub index_values: BTreeMap<String, BTreeSet<String>>,
    /// When the object was first ingested. The retention its object type sets runs from
    /// here.
    pub created_at: SystemTime,
    /// When the object was last ingested under the same unique key.
    pub last_updated_at: SystemTime,
}

impl StoredProfileObject {
    /// Whether this object's retention has run out as of `now`: `expiration_days` after
    /// it was ingested. A retention of no days keeps it, because a domain always carries
    /// a `DefaultExpirationDays` of at least one day and an object type's own
    /// `ExpirationDays` is modelled at 1..1098.
    pub fn is_expired(&self, expiration_days: i32, now: SystemTime) -> bool {
        let days = u64::try_from(expiration_days).unwrap_or_default();
        if days == 0 {
            return false;
        }
        now >= self.created_at + Duration::from_secs(days * SECONDS_PER_DAY)
    }

    /// This object as one entry of a `ListProfileObjects` page.
    pub fn as_item(&self) -> crate::ops::types::ListProfileObjectsItem {
        crate::ops::types::ListProfileObjectsItem {
            object_type_name: Some(self.object_type_name.clone()),
            profile_object_unique_key: Some(self.unique_key.clone()),
            object: Some(self.object.clone()),
        }
    }

    /// Whether this object carries any of `values` for `key_name` — the test one
    /// `ObjectFilter` applies.
    pub fn matches_filter(&self, key_name: &str, values: &[String]) -> bool {
        self.index_values
            .get(key_name)
            .is_some_and(|held| values.iter().any(|value| held.contains(value)))
    }
}

impl Domain {
    /// A new domain holding no customer data: the two members `CreateDomain` requires,
    /// the timestamps it generated, and the instant the asynchronous work its creation
    /// starts falls due. The optional configuration members are set by the caller of
    /// this constructor.
    pub fn new(name: String, default_expiration_days: i32, now: SystemTime) -> Self {
        Self {
            name,
            default_expiration_days,
            default_encryption_key: None,
            dead_letter_queue_url: None,
            matching: None,
            rule_based_matching: None,
            data_store_enabled: None,
            tags: None,
            created_at: now,
            last_updated_at: now,
            rule_based_matching_work: AsyncWork::started(now),
            data_store_work: AsyncWork::started(now),
            profiles: BTreeMap::new(),
            object_types: BTreeMap::new(),
            profile_objects: BTreeMap::new(),
            upload_jobs: BTreeMap::new(),
            calculated_attributes: BTreeMap::new(),
            recommender_schemas: BTreeMap::new(),
            key_index: HashMap::new(),
            profile_keys: BTreeMap::new(),
        }
    }

    /// Whether this domain holds a data table for `dataset_type`.
    ///
    /// A dataset type's data table is the domain's OBJECT TYPE of that name — the
    /// definition the domain ingests that data through — so a domain that defines none for
    /// a dataset type holds no columns a recommender schema could select from it.
    pub fn holds_data_table(&self, dataset_type: &str) -> bool {
        self.object_types.contains_key(dataset_type)
    }

    /// `Matching` as the wire reports it.
    pub fn matching_response(&self) -> Option<MatchingResponse> {
        self.matching.clone()
    }

    /// `Tags` as a domain projection reports it: the tag map this domain carries, PRESENT
    /// and empty for a domain that carries none.
    ///
    /// Unlike the domain's other optional members — `DefaultEncryptionKey`,
    /// `DeadLetterQueueUrl`, `Matching`, `RuleBasedMatching`, each absent from the response
    /// when it was never set — the tag map is always reported, so a caller that indexes
    /// `Tags` can tell "this domain carries no tags" from "tags were not reported".
    pub fn reported_tags(&self) -> Option<HashMap<String, String>> {
        Some(self.tags.clone().unwrap_or_default())
    }

    /// `RuleBasedMatching` as the wire reports it: the stored configuration with
    /// `Status` resolved from the clock. A configuration that is not enabled builds no
    /// rule, so it reports no status.
    pub fn rule_based_matching_response(
        &self,
        now: SystemTime,
    ) -> Option<RuleBasedMatchingResponse> {
        let mut response = self.rule_based_matching.clone()?;
        response.status = (response.enabled == Some(true)).then(|| {
            if self.rule_based_matching_work.is_due(now) {
                RULE_BASED_MATCHING_ACTIVE.to_string()
            } else {
                RULE_BASED_MATCHING_IN_PROGRESS.to_string()
            }
        });
        Some(response)
    }

    /// `DataStore` as the wire reports it: what was requested, plus the readiness of an
    /// enabled store resolved from the clock.
    pub fn data_store_response(&self, now: SystemTime) -> Option<DataStoreResponse> {
        let enabled = self.data_store_enabled?;
        let readiness = enabled.then(|| Readiness {
            progress_percentage: Some(self.data_store_work.progress_percentage(now)),
            message: Some(
                if self.data_store_work.is_due(now) {
                    "The data store is ready."
                } else {
                    "The data store is being prepared."
                }
                .to_string(),
            ),
        });
        Some(DataStoreResponse {
            enabled: Some(enabled),
            readiness,
        })
    }

    /// Store `stored` and index it under every standard search key its attributes
    /// populate, plus every additional key already associated with that id. A profile
    /// already held under the same id is replaced, index included.
    pub fn put_profile(&mut self, stored: StoredProfile) {
        let Some(id) = stored.profile.profile_id.clone() else {
            return;
        };
        // The additional keys associated with this profile belong to the PROFILE, not to
        // the members a write carries, so they are read out before the profile is dropped
        // and re-indexed with it: writing a profile's standard members associates no key
        // and disassociates none.
        let additional = self.profile_keys.get(&id).cloned().unwrap_or_default();
        self.remove_profile(&id);
        for (key_name, value) in standard_search_keys(&stored.profile) {
            self.index_key(&id, key_name, value);
        }
        for (key_name, values) in &additional {
            for value in values {
                self.index_key(&id, key_name.clone(), value.clone());
            }
        }
        if !additional.is_empty() {
            self.profile_keys.insert(id.clone(), additional);
        }
        self.profiles.insert(id, stored);
    }

    /// Remove the profile, the additional keys associated with it, and every index entry
    /// that pointed at it.
    pub fn remove_profile(&mut self, profile_id: &str) -> Option<StoredProfile> {
        let stored = self.profiles.remove(profile_id)?;
        for (key_name, value) in standard_search_keys(&stored.profile) {
            self.unindex_key(profile_id, &key_name, &value);
        }
        for (key_name, values) in self.profile_keys.remove(profile_id).unwrap_or_default() {
            for value in values {
                self.unindex_key(profile_id, &key_name, &value);
            }
        }
        Some(stored)
    }

    /// Index one (key name, value) pair as reaching `profile_id`.
    fn index_key(&mut self, profile_id: &str, key_name: String, value: String) {
        self.key_index
            .entry(key_name)
            .or_default()
            .entry(value)
            .or_default()
            .insert(profile_id.to_string());
    }

    /// Drop one (key name, value) pair's entry for `profile_id`, and the pair itself once
    /// no profile is reached through it.
    fn unindex_key(&mut self, profile_id: &str, key_name: &str, value: &str) {
        let Some(by_value) = self.key_index.get_mut(key_name) else {
            return;
        };
        if by_value.get_mut(value).is_some_and(|ids| {
            ids.remove(profile_id);
            ids.is_empty()
        }) {
            by_value.remove(value);
        }
        if by_value.is_empty() {
            self.key_index.remove(key_name);
        }
    }

    /// Associate every value in `values` with `profile_id` under `key_name`, indexing each
    /// pair so a search finds the profile through it.
    ///
    /// A value the profile already carries under that key associates nothing further: the
    /// key holds a SET of values. Applied in place, value by value, so one caller's
    /// association never replaces another's.
    ///
    /// Nothing is written when the profile would be left carrying more than
    /// [`MAX_KEY_VALUES_PER_PROFILE`] key values; the number it would then carry comes
    /// back in `Err`, for the refusal's message.
    pub fn add_profile_key(
        &mut self,
        profile_id: &str,
        key_name: &str,
        values: &[String],
    ) -> Result<(), usize> {
        {
            let held = self.profile_keys.get(profile_id);
            let held_count: usize = held
                .map(|keys| keys.values().map(BTreeSet::len).sum())
                .unwrap_or_default();
            let under_key = held.and_then(|keys| keys.get(key_name));
            let fresh: BTreeSet<&str> = values
                .iter()
                .map(String::as_str)
                .filter(|value| !under_key.is_some_and(|held| held.contains(*value)))
                .collect();
            let resulting = held_count + fresh.len();
            if resulting > MAX_KEY_VALUES_PER_PROFILE {
                return Err(resulting);
            }
        }
        for value in values {
            self.profile_keys
                .entry(profile_id.to_string())
                .or_default()
                .entry(key_name.to_string())
                .or_default()
                .insert(value.clone());
            self.index_key(profile_id, key_name.to_string(), value.clone());
        }
        Ok(())
    }

    /// The (key name, value) pairs this domain has `profile` indexed under: the predefined
    /// keys its own attributes populate, plus every key value associated with it by
    /// `AddProfileKey`. Sorted and deduplicated, so a pair that is both appears once.
    ///
    /// This is the set a `SearchProfiles` key is tested against to report `FoundByItems`,
    /// so a profile is reported as found by exactly the pairs it is reachable through.
    pub fn profile_index_keys(&self, profile: &Profile) -> Vec<(String, String)> {
        let mut keys = standard_search_keys(profile);
        if let Some(additional) = profile
            .profile_id
            .as_deref()
            .and_then(|id| self.profile_keys.get(id))
        {
            keys.extend(additional.iter().flat_map(|(key_name, values)| {
                values
                    .iter()
                    .map(move |value| (key_name.clone(), value.clone()))
            }));
        }
        keys.sort();
        keys.dedup();
        keys
    }

    /// The ids of the profiles indexed under `key_name` for ANY of `values` — the
    /// "key-value(s) pair" one `SearchProfiles` search key describes. Ascending by id,
    /// so a page boundary is reproducible.
    pub fn lookup(&self, key_name: &str, values: &[String]) -> BTreeSet<String> {
        let Some(by_value) = self.key_index.get(key_name) else {
            return BTreeSet::new();
        };
        values
            .iter()
            .filter_map(|value| by_value.get(value.as_str()))
            .flatten()
            .cloned()
            .collect()
    }

    /// How long an object ingested under `object_type_name` is kept, in days: the object
    /// type's own `ExpirationDays` ("The number of days until the data in the object
    /// expires") when it sets one, else the domain's `DefaultExpirationDays` ("The
    /// default number of days until the data within the domain expires").
    pub fn object_expiration_days(&self, object_type_name: &str) -> i32 {
        self.object_types
            .get(object_type_name)
            .and_then(|object_type| object_type.expiration_days)
            .unwrap_or(self.default_expiration_days)
    }

    /// Move every additional key value associated with `from` onto `to`, re-indexing each
    /// pair under the profile that now carries it: "all the profileKeys in the
    /// `ProfileIdsToBeMerged` will be moved to the main profile".
    ///
    /// A pair moves rather than being copied, so what the domain holds does not grow and
    /// no cap applies here.
    pub fn move_profile_keys(&mut self, from: &str, to: &str) {
        if from == to {
            return;
        }
        for (key_name, values) in self.profile_keys.remove(from).unwrap_or_default() {
            for value in values {
                self.unindex_key(from, &key_name, &value);
                self.profile_keys
                    .entry(to.to_string())
                    .or_default()
                    .entry(key_name.clone())
                    .or_default()
                    .insert(value.clone());
                self.index_key(to, key_name.clone(), value);
            }
        }
    }

    /// Drop every ingested object whose retention has run out as of `now`.
    ///
    /// This materialises an expiry the CLOCK already made due — the data was owed
    /// removal at that instant whether or not anybody called — so every operation that
    /// touches this domain's objects does it first, and none of them reports data the
    /// domain should no longer hold.
    pub fn expire_profile_objects(&mut self, now: SystemTime) {
        let expired: Vec<ProfileObjectId> = self
            .profile_objects
            .iter()
            .filter(|(_, stored)| {
                stored.is_expired(self.object_expiration_days(&stored.object_type_name), now)
            })
            .map(|(id, _)| id.clone())
            .collect();
        for id in expired {
            self.profile_objects.remove(&id);
        }
    }

    /// Store `object`, replacing any object already held under the same identity: a
    /// re-ingest of the same object type and unique key is the same object.
    pub fn put_profile_object(&mut self, object: StoredProfileObject) {
        self.profile_objects.insert(
            (
                object.object_type_name.clone(),
                object.profile_id.clone(),
                object.unique_key.clone(),
            ),
            object,
        );
    }

    /// The objects tied to one profile under one object type, in ascending unique-key
    /// order.
    pub fn profile_objects_of<'a>(
        &'a self,
        object_type_name: &'a str,
        profile_id: &'a str,
    ) -> impl Iterator<Item = &'a StoredProfileObject> + 'a {
        let from = (
            object_type_name.to_string(),
            profile_id.to_string(),
            String::new(),
        );
        self.profile_objects
            .range(from..)
            .take_while(move |((object_type, profile, _), _)| {
                object_type == object_type_name && profile == profile_id
            })
            .map(|(_, stored)| stored)
    }

    /// The number of objects the domain holds under one object type, across every
    /// profile, counting only those whose retention has not run out as of `now` — what
    /// `MaxAvailableProfileObjectCount` reports as consumed.
    pub fn profile_object_count(&self, object_type_name: &str, now: SystemTime) -> i32 {
        let expiration_days = self.object_expiration_days(object_type_name);
        let held = self
            .profile_objects
            .iter()
            .filter(|((object_type, _, _), stored)| {
                object_type == object_type_name && !stored.is_expired(expiration_days, now)
            })
            .count();
        i32::try_from(held).unwrap_or(i32::MAX)
    }

    /// Remove one object, answering the record that was removed.
    pub fn remove_profile_object(
        &mut self,
        object_type_name: &str,
        profile_id: &str,
        unique_key: &str,
    ) -> Option<StoredProfileObject> {
        self.profile_objects.remove(&(
            object_type_name.to_string(),
            profile_id.to_string(),
            unique_key.to_string(),
        ))
    }

    /// Remove every object the given predicate over (object type, profile) selects, and
    /// answer how many went. The one removal path behind deleting a profile, deleting an
    /// object type, and anything else that drops a whole branch of this index.
    pub fn remove_profile_objects(
        &mut self,
        selected: impl Fn(&str, &str) -> bool,
    ) -> usize {
        let dropped: Vec<ProfileObjectId> = self
            .profile_objects
            .keys()
            .filter(|(object_type, profile, _)| selected(object_type, profile))
            .cloned()
            .collect();
        let count = dropped.len();
        for id in dropped {
            self.profile_objects.remove(&id);
        }
        count
    }

    /// Move every object tied to `from` onto `to`, keeping each object's own unique key:
    /// the objects of a profile merged into another belong to the profile that absorbed
    /// it.
    pub fn move_profile_objects(&mut self, from: &str, to: &str) {
        let moved: Vec<ProfileObjectId> = self
            .profile_objects
            .keys()
            .filter(|(_, profile, _)| profile == from)
            .cloned()
            .collect();
        for id in moved {
            let Some(mut stored) = self.profile_objects.remove(&id) else {
                continue;
            };
            stored.profile_id = to.to_string();
            self.put_profile_object(stored);
        }
    }

    /// The profile an ingested object belongs to: the first profile already indexed under
    /// one of `probes`, which are the (predefined key, value) pairs the object's keys
    /// select, in the order the service searches them.
    pub fn match_profile(&self, probes: &[(String, String)]) -> Option<String> {
        probes.iter().find_map(|(key_name, value)| {
            self.lookup(key_name, ::std::slice::from_ref(value))
                .into_iter()
                .next()
        })
    }
}

/// A fresh `ProfileId`: the 32 lowercase hex characters the modelled pattern
/// `^[a-f0-9]{32}$` requires, taken from a v4 UUID's simple (undashed) form. One minter,
/// so every profile this service stores — created by a call or inferred from an ingested
/// object — carries an id of that one shape.
pub fn new_profile_id() -> String {
    ::uuid::Uuid::new_v4().simple().to_string()
}

/// The predefined `SearchProfiles` keys a profile's OWN attributes populate.
///
/// `SearchProfiles$KeyName` lists the predefined keys: `_account`, `_profileId`,
/// `_assetId`, `_caseId`, `_orderId`, `_fullName`, `_phone`, `_email`, `_ctrContactId`,
/// and the connector ids (`_marketoLeadId`, `_salesforce*`, `_zendesk*`,
/// `_serviceNow*`, `_segmentUserId`, `_shopify*`). Only the five returned here are
/// derivable from a profile itself; the rest are keys of ingested profile OBJECTS, which
/// carry their own index values (see [`ProfileObjectIngest`]).
///
/// Every email member feeds `_email` and every phone member feeds `_phone`, because
/// those keys name a class of value rather than one member. The result is sorted and
/// deduplicated so indexing and un-indexing walk the same entries.
pub fn standard_search_keys(profile: &Profile) -> Vec<(String, String)> {
    let full = full_name(profile);
    let singles = [
        (KEY_PROFILE_ID, profile.profile_id.as_deref()),
        (KEY_ACCOUNT, profile.account_number.as_deref()),
        (KEY_EMAIL, profile.email_address.as_deref()),
        (KEY_EMAIL, profile.personal_email_address.as_deref()),
        (KEY_EMAIL, profile.business_email_address.as_deref()),
        (KEY_PHONE, profile.phone_number.as_deref()),
        (KEY_PHONE, profile.mobile_phone_number.as_deref()),
        (KEY_PHONE, profile.home_phone_number.as_deref()),
        (KEY_PHONE, profile.business_phone_number.as_deref()),
        (KEY_FULL_NAME, full.as_deref()),
    ];
    let mut keys: Vec<(String, String)> = singles
        .into_iter()
        .filter_map(|(name, value)| {
            let value = value?;
            (!value.is_empty()).then(|| (name.to_string(), value.to_string()))
        })
        .collect();
    keys.sort();
    keys.dedup();
    keys
}

/// A profile's full name: its given, middle and family names in that order, space
/// separated, skipping the ones it does not carry. `None` when it carries none.
fn full_name(profile: &Profile) -> Option<String> {
    let parts: Vec<&str> = [
        profile.first_name.as_deref(),
        profile.middle_name.as_deref(),
        profile.last_name.as_deref(),
    ]
    .into_iter()
    .flatten()
    .filter(|part| !part.is_empty())
    .collect();
    (!parts.is_empty()).then(|| parts.join(" "))
}

// ---------------------------------------------------------------------------
// Ingesting a profile object
//
// An ingest is given a JSON object and the name of the object type that describes it.
// That type's `Fields` say where each piece of data sits in the object (`Source`) and
// which member of the standard profile model it belongs to (`Target`); its `Keys` say
// which of those fields form a key, and what each key may be used for
// (`StandardIdentifiers`). Everything below reads an object through that definition, so
// every operation that ingests one reads it the same way.
// ---------------------------------------------------------------------------

/// `StandardIdentifier` of a key that ties the object to a profile: "PROFILE ... means
/// that this key can be used to tie an object to a PROFILE".
const IDENTIFIER_PROFILE: &str = "PROFILE";

/// `StandardIdentifier` of a key searched only after every `PROFILE` key: "If a key a is
/// marked as SECONDARY, it will be used to search for profiles after all other PROFILE
/// keys have been searched."
const IDENTIFIER_SECONDARY: &str = "SECONDARY";

/// `StandardIdentifier` of the key that identifies the OBJECT: "UNIQUE means that it can
/// be used to uniquely identify an object."
const IDENTIFIER_UNIQUE: &str = "UNIQUE";

/// `StandardIdentifier` of a key that "is only used to match a profile but is not
/// persisted to be used for searching of the profile".
const IDENTIFIER_LOOKUP_ONLY: &str = "LOOKUP_ONLY";

/// The prefix of an `ObjectTypeField$Target`: "the location of the data in the standard
/// ProfileObject model. For example: _profile.Address.PostalCode".
pub const PROFILE_TARGET_PREFIX: &str = "_profile.";

/// The container a target names to reach one entry of the profile's `Attributes` map.
const ATTRIBUTES_CONTAINER: &str = "Attributes";

/// What separates the values of a multi-field UNIQUE key in the object identity derived
/// from it: a character no modelled value of this service carries.
const UNIQUE_VALUE_SEPARATOR: char = '\u{1f}';

/// The predefined `SearchProfiles` keys a stored profile is indexed under, which are
/// therefore the keys an ingest can look a profile up by. The same five
/// [`standard_search_keys`] indexes a profile into.
const PREDEFINED_PROFILE_KEYS: [&str; 5] =
    [KEY_PROFILE_ID, KEY_ACCOUNT, KEY_EMAIL, KEY_PHONE, KEY_FULL_NAME];

/// The values `PartyType` admits, as the model's enum spells them.
const PARTY_TYPES: [&str; 3] = ["INDIVIDUAL", "BUSINESS", "OTHER"];

/// The values `Gender` admits, as the model's enum spells them.
const GENDERS: [&str; 3] = ["MALE", "FEMALE", "UNSPECIFIED"];

/// The values `ProfileType` admits, as the model's enum spells them.
const PROFILE_TYPES: [&str; 2] = ["ACCOUNT_PROFILE", "PROFILE"];

/// What one ingested profile object yields, read through its object type's definition.
#[derive(Debug, Default)]
pub struct ProfileObjectIngest {
    /// `KeyName` -> the values the object carries for that key: what a
    /// `ListProfileObjects` `ObjectFilter` selects by.
    pub index_values: BTreeMap<String, BTreeSet<String>>,
    /// The value of the type's UNIQUE key, when it defines one and the object carries a
    /// value for it: what identifies this object among the objects of its type.
    pub unique_value: Option<String>,
    /// The (predefined key, value) pairs that identify the profile this object belongs
    /// to, in the order they are searched: every `PROFILE` key first, then every
    /// `SECONDARY` key, which is searched "after all other PROFILE keys".
    pub probes: Vec<(String, String)>,
    /// The standard profile the object's data maps into through its fields' `Target`s —
    /// the profile an ingest creates when it finds none to tie the object to.
    pub mapped_profile: Profile,
    /// Whether any of the object's data reached that profile. With none there is nothing
    /// to infer a profile FROM, so an object that maps nothing creates none.
    pub maps_profile_data: bool,
}

/// Read one ingested object through the definition of the object type it names.
pub fn read_profile_object(
    object_type: &StoredObjectType,
    object: &::serde_json::Value,
) -> ProfileObjectIngest {
    let mut ingest = ProfileObjectIngest::default();

    // Every field the type defines maps one piece of the object onto one member of the
    // standard profile model, so the mapped profile is built from all of them, not only
    // from the ones a key names.
    let mut fields: Vec<(&String, &ObjectTypeField)> =
        object_type.fields.iter().flatten().collect();
    fields.sort_by(|left, right| left.0.cmp(right.0));
    for (field_name, field) in &fields {
        let Some(value) = field_value(field_name, Some(field), object) else {
            continue;
        };
        if let Some(target) = field.target.as_deref() {
            ingest.maps_profile_data |=
                apply_profile_target(&mut ingest.mapped_profile, target, value);
        }
    }

    let mut keys: Vec<(&String, &Vec<ObjectTypeKey>)> = object_type.keys.iter().flatten().collect();
    keys.sort_by(|left, right| left.0.cmp(right.0));
    let mut searched_last: Vec<(String, String)> = Vec::new();
    for (key_name, definitions) in &keys {
        for definition in definitions.iter() {
            let identifiers: Vec<&str> = definition
                .standard_identifiers
                .iter()
                .flatten()
                .map(String::as_str)
                .collect();
            let mut values: Vec<(Option<&str>, String)> = Vec::new();
            for field_name in definition.field_names.iter().flatten() {
                let field = object_type
                    .fields
                    .as_ref()
                    .and_then(|fields| fields.get(field_name));
                if let Some(value) = field_value(field_name, field, object) {
                    values.push((field.and_then(|field| field.target.as_deref()), value));
                }
            }
            if values.is_empty() {
                continue;
            }

            // A `LOOKUP_ONLY` key is the one kind that contributes no index value: it
            // "is not persisted to be used for searching of the profile".
            if !identifiers.contains(&IDENTIFIER_LOOKUP_ONLY) {
                ingest
                    .index_values
                    .entry((*key_name).clone())
                    .or_default()
                    .extend(values.iter().map(|(_, value)| value.clone()));
            }
            // "Each ProfileObject can have only 1 UNIQUE key", so the first one defined
            // identifies the object.
            if identifiers.contains(&IDENTIFIER_UNIQUE) && ingest.unique_value.is_none() {
                ingest.unique_value = Some(
                    values
                        .iter()
                        .map(|(_, value)| value.as_str())
                        .collect::<Vec<&str>>()
                        .join(&UNIQUE_VALUE_SEPARATOR.to_string()),
                );
            }

            let ties_to_profile = identifiers.contains(&IDENTIFIER_PROFILE);
            let secondary = identifiers.contains(&IDENTIFIER_SECONDARY);
            if !ties_to_profile && !secondary {
                continue;
            }
            let mut probes: Vec<(String, String)> = Vec::new();
            for (target, value) in &values {
                // A key NAMED after one of the predefined profile keys looks the profile
                // up under that key directly; otherwise the field's `Target` says which
                // member of the standard profile the value belongs to, and that member's
                // predefined key is the one to look under.
                if is_predefined_profile_key(key_name) {
                    probes.push(((*key_name).clone(), value.clone()));
                }
                if let Some(search_key) = target.and_then(standard_search_key_for_target) {
                    probes.push((search_key.to_string(), value.clone()));
                }
            }
            if ties_to_profile {
                ingest.probes.extend(probes);
            } else {
                searched_last.extend(probes);
            }
        }
    }
    ingest.probes.extend(searched_last);

    // An object type that defines neither fields nor keys says nothing about where the
    // object's data belongs, so the object's own members do: a member named after a
    // member of the standard profile model is read as that member — the mapping the
    // service's own definitions spell out when they name the two alike.
    if fields.is_empty() && keys.is_empty() {
        let mut members: Vec<(&String, &::serde_json::Value)> =
            object.as_object().into_iter().flatten().collect();
        members.sort_by(|left, right| left.0.cmp(right.0));
        for (member, value) in members {
            let Some(text) = scalar_text(value) else {
                continue;
            };
            let target = format!("{PROFILE_TARGET_PREFIX}{member}");
            ingest.maps_profile_data |=
                apply_profile_target(&mut ingest.mapped_profile, &target, text.clone());
            if let Some(search_key) = standard_search_key_for_target(&target) {
                ingest.probes.push((search_key.to_string(), text));
            }
        }
        // A profile named by its own id IS the profile, so that look-up comes first.
        ingest
            .probes
            .sort_by_key(|(key_name, _)| usize::from(key_name != KEY_PROFILE_ID));
    }

    ingest
}

/// The value one field of an object type selects out of an ingested object.
///
/// `ObjectTypeField$Source` is the path the data sits at ("_source.FirstName, where
/// “_source” is a ProfileObjectType"); a field reference the type defines no field for is
/// read as the object member of that name, which is the only path the reference itself
/// spells.
fn field_value(
    field_name: &str,
    field: Option<&ObjectTypeField>,
    object: &::serde_json::Value,
) -> Option<String> {
    let source = field
        .and_then(|field| field.source.as_deref())
        .unwrap_or(field_name);
    source_value(object, source)
}

/// The value a `Source` path selects out of an ingested object.
///
/// The leading segment of a path with more than one segment names the object type the
/// data came from, so the path into the object itself starts after it; a single-segment
/// path is a member of the object.
pub fn source_value(object: &::serde_json::Value, source: &str) -> Option<String> {
    let mut segments = source.split('.').peekable();
    let mut current = object;
    if source.split('.').count() > 1 {
        segments.next();
    }
    for segment in segments {
        current = current.get(segment)?;
    }
    scalar_text(current)
}

/// One JSON scalar as the text a key value or a profile member carries. An array, an
/// object and a null are not values either can hold, so they select nothing.
fn scalar_text(value: &::serde_json::Value) -> Option<String> {
    match value {
        ::serde_json::Value::String(text) => (!text.is_empty()).then(|| text.clone()),
        ::serde_json::Value::Number(number) => Some(number.to_string()),
        ::serde_json::Value::Bool(flag) => Some(flag.to_string()),
        _ => None,
    }
}

/// Whether `key_name` is one of the predefined keys a profile is indexed under.
fn is_predefined_profile_key(key_name: &str) -> bool {
    PREDEFINED_PROFILE_KEYS.contains(&key_name)
}

/// The predefined `SearchProfiles` key the standard-profile member a `Target` names is
/// indexed under, or `None` for a member no predefined key covers.
///
/// This is the inverse of [`standard_search_keys`], so a value extracted from an ingested
/// object probes exactly the index a profile carrying that value was indexed into.
/// `_fullName` is not reachable: it is the concatenation of three members, so no single
/// member feeds it.
pub fn standard_search_key_for_target(target: &str) -> Option<&'static str> {
    match target.strip_prefix(PROFILE_TARGET_PREFIX)? {
        "ProfileId" => Some(KEY_PROFILE_ID),
        "AccountNumber" => Some(KEY_ACCOUNT),
        "EmailAddress" | "PersonalEmailAddress" | "BusinessEmailAddress" => Some(KEY_EMAIL),
        "PhoneNumber" | "MobilePhoneNumber" | "HomePhoneNumber" | "BusinessPhoneNumber" => {
            Some(KEY_PHONE)
        }
        _ => None,
    }
}

/// Write the value a field's `Target` names onto `profile`, answering whether the target
/// named a member of the standard profile model.
///
/// The member path is what follows [`PROFILE_TARGET_PREFIX`]: one of the profile's own
/// members, an entry of its `Attributes` map, or a member of one of its four addresses.
/// `ProfileId` is not among them — the service generates a profile's id, so a target
/// naming it carries no data onto the profile.
pub fn apply_profile_target(profile: &mut Profile, target: &str, value: String) -> bool {
    let Some(path) = target.strip_prefix(PROFILE_TARGET_PREFIX) else {
        return false;
    };
    if let Some((container, member)) = path.split_once('.') {
        return match container {
            ATTRIBUTES_CONTAINER => apply_attribute(profile, member, value),
            "Address" => apply_address_target(&mut profile.address, member, value),
            "ShippingAddress" => apply_address_target(&mut profile.shipping_address, member, value),
            "MailingAddress" => apply_address_target(&mut profile.mailing_address, member, value),
            "BillingAddress" => apply_address_target(&mut profile.billing_address, member, value),
            _ => false,
        };
    }
    match path {
        "AccountNumber" => profile.account_number = Some(value),
        "AdditionalInformation" => profile.additional_information = Some(value),
        "BusinessName" => profile.business_name = Some(value),
        "FirstName" => profile.first_name = Some(value),
        "MiddleName" => profile.middle_name = Some(value),
        "LastName" => profile.last_name = Some(value),
        "BirthDate" => profile.birth_date = Some(value),
        "PhoneNumber" => profile.phone_number = Some(value),
        "MobilePhoneNumber" => profile.mobile_phone_number = Some(value),
        "HomePhoneNumber" => profile.home_phone_number = Some(value),
        "BusinessPhoneNumber" => profile.business_phone_number = Some(value),
        "EmailAddress" => profile.email_address = Some(value),
        "PersonalEmailAddress" => profile.personal_email_address = Some(value),
        "BusinessEmailAddress" => profile.business_email_address = Some(value),
        "PartyTypeString" => profile.party_type_string = Some(value),
        "GenderString" => profile.gender_string = Some(value),
        // An enum-valued member carries only a value its enum admits, so data outside
        // the enum maps onto nothing.
        "PartyType" => return apply_enum_member(&mut profile.party_type, &PARTY_TYPES, value),
        "Gender" => return apply_enum_member(&mut profile.gender, &GENDERS, value),
        "ProfileType" => return apply_enum_member(&mut profile.profile_type, &PROFILE_TYPES, value),
        _ => return false,
    }
    true
}

/// Write one entry of the profile's `Attributes` map, up to the cap on how many
/// attributes one profile holds.
fn apply_attribute(profile: &mut Profile, member: &str, value: String) -> bool {
    let attributes = profile.attributes.get_or_insert_with(HashMap::new);
    if attributes.len() >= MAX_ATTRIBUTES_PER_PROFILE && !attributes.contains_key(member) {
        return false;
    }
    attributes.insert(member.to_string(), value);
    true
}

/// Write one member of one of the profile's addresses, answering whether the name is a
/// member `Address` declares. The address is stored only when it is.
fn apply_address_target(slot: &mut Option<Address>, member: &str, value: String) -> bool {
    let mut address = slot.clone().unwrap_or_default();
    match member {
        "Address1" => address.address1 = Some(value),
        "Address2" => address.address2 = Some(value),
        "Address3" => address.address3 = Some(value),
        "Address4" => address.address4 = Some(value),
        "City" => address.city = Some(value),
        "County" => address.county = Some(value),
        "State" => address.state = Some(value),
        "Province" => address.province = Some(value),
        "Country" => address.country = Some(value),
        "PostalCode" => address.postal_code = Some(value),
        _ => return false,
    }
    *slot = Some(address);
    true
}

/// Write `value` onto an enum-valued member of a profile, but only when the model's enum
/// admits it.
fn apply_enum_member(member: &mut Option<String>, admitted: &[&str], value: String) -> bool {
    if !admitted.contains(&value.as_str()) {
        return false;
    }
    *member = Some(value);
    true
}

/// What identifies one ingested object: the value of its type's UNIQUE key when the type
/// defines one, else the object's own JSON. Either way the same data ingested twice has
/// the same identity, and so is the same object.
pub fn profile_object_identity(
    unique_value: Option<&str>,
    object: &::serde_json::Value,
) -> String {
    match unique_value {
        Some(value) => value.to_string(),
        None => object.to_string(),
    }
}

/// The `ProfileObjectUniqueKey` of an ingested object: "the unique identifier of the
/// profile object generated by the service".
///
/// A digest over the object type and the object's identity, so the key is opaque, is
/// within the member's modelled 1..255 characters whatever the object's size, and is
/// STABLE: re-ingesting an object of the same identity derives the same key and so
/// replaces that object instead of storing a second copy of it.
pub fn profile_object_unique_key(object_type_name: &str, identity: &str) -> String {
    use ::sha2::Digest as _;
    let mut digest = ::sha2::Sha256::new();
    digest.update(object_type_name.as_bytes());
    digest.update([0u8]);
    digest.update(identity.as_bytes());
    ::hex::encode(digest.finalize())
}

/// The refusal for an `Object` that is not what the member must hold. Fixed text for the
/// class: it interpolates nothing out of the request, whose `Object` is sensitive.
const NOT_AN_OBJECT: &str = "Object must be a string serialized from a JSON object";

/// Why the ingest of ONE object was refused.
///
/// Every class here is a bad request about that one object, so ingesting a single object
/// reports it as `BadRequestException` while ingesting a batch reports it as that item's
/// own entry — the same classes, read off the same ingest.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IngestRefusal {
    /// `Object` is not a string serialized from a JSON object.
    NotAnObject,
    /// The domain already holds as many profiles as this emulator keeps for one
    /// ([`MAX_PROFILES_PER_DOMAIN`]), so no profile can be inferred for the object.
    ProfileCapReached,
    /// The domain already holds as many profile objects as this emulator keeps for one
    /// ([`MAX_PROFILE_OBJECTS_PER_DOMAIN`]).
    ObjectCapReached,
}

impl IngestRefusal {
    /// The HTTP status this refusal is answered with: every class is a bad request about
    /// the object, which is the status `BadRequestException` ("The input you provided is
    /// invalid") is bound to, and which a batch item's `Code` ("The HTTP status code for
    /// the error") carries.
    pub const fn status(self) -> i32 {
        400
    }

    /// The sentence the refusal is reported with: fixed text per class, whose only
    /// variable parts are the domain the request named and the cap it reached.
    pub fn message(self, domain_name: &str) -> String {
        match self {
            Self::NotAnObject => NOT_AN_OBJECT.to_string(),
            Self::ProfileCapReached => format!(
                "The maximum number of profiles for domain {domain_name} has been reached: {MAX_PROFILES_PER_DOMAIN}"
            ),
            Self::ObjectCapReached => format!(
                "The maximum number of profile objects for domain {domain_name} has been reached: {MAX_PROFILE_OBJECTS_PER_DOMAIN}"
            ),
        }
    }
}

/// Ingest ONE object into `domain` under `object_type`, answering the
/// `ProfileObjectUniqueKey` the service derived for it.
///
/// The one ingest path, shared by every operation that takes an object in, so an object
/// lands in the same place and earns the same key however it arrived: the object is read
/// through its type's definition, stored under the unique key derived from its own
/// identity, and tied to the profile that type's keys identify — or, when the type allows
/// profile creation and no profile matches, to a profile inferred from the data the type
/// maps onto one. Re-ingesting an object of the same identity replaces it, so it keeps the
/// instant it first arrived.
///
/// `now` is the instant of the call. The caller holds the store, looked the domain and the
/// object type up in it, and has already materialised the expiry the clock made due
/// ([`Domain::expire_profile_objects`]).
pub fn ingest_profile_object(
    domain: &mut Domain,
    object_type_name: &str,
    object_type: &StoredObjectType,
    object_text: &str,
    now: SystemTime,
) -> Result<String, IngestRefusal> {
    // `Object` is "a string that is serialized from a JSON object", and its modelled
    // length is already enforced at the boundary. Parsing it here, before the store is
    // written, is what keeps an object carrying anything else from landing.
    let object: ::serde_json::Value =
        ::serde_json::from_str(object_text).map_err(|_| IngestRefusal::NotAnObject)?;
    if !object.is_object() {
        return Err(IngestRefusal::NotAnObject);
    }

    // Read the object through that definition: the key values it carries, the value that
    // identifies it, the profiles it could belong to, and the standard profile its data
    // maps into.
    let ingest = read_profile_object(object_type, &object);
    let unique_key = profile_object_unique_key(
        object_type_name,
        &profile_object_identity(ingest.unique_value.as_deref(), &object),
    );

    // The service "tries to fetch a standard profile and associate this object with the
    // profile"; failing that, and only when the type allows profile creation, "an
    // inferred profile can get created" out of the data the object maps onto a profile —
    // which is what makes the next object carrying the same key values map onto that same
    // profile. An object whose type maps none of it onto a profile gives nothing to infer
    // one from.
    let profile_id = match domain.match_profile(&ingest.probes) {
        Some(profile_id) => profile_id,
        None if object_type.allow_profile_creation && ingest.maps_profile_data => {
            if domain.profiles.len() >= MAX_PROFILES_PER_DOMAIN {
                return Err(IngestRefusal::ProfileCapReached);
            }
            let profile_id = new_profile_id();
            let mut profile = ingest.mapped_profile;
            profile.profile_id = Some(profile_id.clone());
            domain.put_profile(StoredProfile {
                profile,
                created_at: now,
                last_updated_at: now,
            });
            profile_id
        }
        // A type that does not allow profile creation leaves an object no profile claims
        // unmapped: it is held, and no profile reports it.
        None => String::new(),
    };

    // At most this many objects per domain: however many requests arrive, and however
    // many objects one of them carries, the store cannot grow past the cap. Re-ingesting
    // an object the domain already holds replaces it, so it keeps the instant it first
    // arrived and is not growth.
    let created_at = domain
        .profile_objects_of(object_type_name, &profile_id)
        .find(|stored| stored.unique_key == unique_key)
        .map(|stored| stored.created_at);
    if created_at.is_none() && domain.profile_objects.len() >= MAX_PROFILE_OBJECTS_PER_DOMAIN {
        return Err(IngestRefusal::ObjectCapReached);
    }

    domain.put_profile_object(StoredProfileObject {
        object_type_name: object_type_name.to_string(),
        unique_key: unique_key.clone(),
        // Stored as the request carried it, so a listing reports the object that was
        // ingested rather than a re-rendering of it.
        object: object_text.to_string(),
        profile_id,
        index_values: ingest.index_values,
        created_at: created_at.unwrap_or(now),
        last_updated_at: now,
    });

    Ok(unique_key)
}

/// Take one page out of `items`, already in ascending cursor order.
///
/// `next_token` is the cursor a previous page handed back: the page resumes at the first
/// item whose cursor is not before it, so an item deleted between two pages is skipped
/// rather than shifting the window. The returned token is the cursor of the first item
/// that did not fit, and is `None` when the page is the last one.
///
/// The CURSOR is what this takes and hands back, never the `NextToken` a caller sees: a
/// read whose tokens are authenticated ([`page_token`], [`page_cursor`]) resolves the one
/// into the other around this call.
pub fn paginate<T>(
    items: impl IntoIterator<Item = (String, T)>,
    next_token: Option<&str>,
    max_results: Option<i32>,
) -> (Vec<T>, Option<String>) {
    let limit = max_results.map_or(DEFAULT_PAGE_SIZE, |max| max.max(1) as usize);
    let mut page = Vec::new();
    let mut token = None;
    for (cursor, item) in items {
        if next_token.is_some_and(|resume| cursor.as_str() < resume) {
            continue;
        }
        if page.len() >= limit {
            token = Some(cursor);
            break;
        }
        page.push(item);
    }
    (page, token)
}

/// The `NextToken` a read hands back for `cursor`, authenticated under this store's own
/// secret: the authenticator over the cursor and the cursor itself
/// (`service_common::page_tokens`), rendered as the hexadecimal text an opaque token
/// travels as.
///
/// A continuation token is opaque, not confidential — the cursor inside it is a position
/// this service already reported — so nothing here hides it. What a caller cannot do is
/// produce the authenticator for a position of its own, which is what makes [`page_cursor`]
/// able to refuse a token this store never issued.
///
/// THE READS THAT TAKE IT: `SearchProfiles`, whose recorded refusal of a token it did not
/// issue is what this stands up. This service's other paginated reads still hand back their
/// bare cursor and are to be migrated onto this pair as each one's own refusal is recorded.
pub fn page_token(key: ::service_common::page_tokens::PageTokenKey, cursor: &str) -> String {
    ::hex::encode(key.mint(cursor))
}

/// The cursor `token` names, or `None` for any value this store did not issue: text that is
/// not the hexadecimal rendering [`page_token`] writes, and one whose authenticator does not
/// match the cursor it travels with.
pub fn page_cursor(
    key: ::service_common::page_tokens::PageTokenKey,
    token: &str,
) -> Option<String> {
    key.cursor_of(&::hex::decode(token).ok()?)
}

/// The value a profile's STRING alternative carries, for a write that states the member in
/// its enumerated form, its string form, or neither.
///
/// `Profile$PartyTypeString` is documented as "An alternative to PartyType which accepts
/// any string as input", and `GenderString` is the same alternative to `Gender`
/// (`smithy_query(query="doc", shape="Profile", member="PartyTypeString")`): the two members
/// are two renderings of ONE value a profile carries, not two values. So a write that states
/// the enumerated member states that same value as the string form — the enum's own member
/// IS a string the wider member admits — and a read reports both; a write that states the
/// string form keeps exactly what it sent, since nothing narrows the wider member.
pub fn string_alternative(stated: &Option<String>, alternative: Option<String>) -> Option<String> {
    alternative.or_else(|| stated.clone())
}

/// How far through a span that started at `started_at` and falls due at `due_at` the
/// clock has got as of `now`, as a percentage in 0..=100 — the range every
/// `Readiness$ProgressPercentage` on this service's wire declares. One definition of the
/// clamp and the rounding, so two resources reporting progress cannot drift apart.
///
/// A span of no length is complete: there is nothing left to wait for.
pub fn progress_percentage(started_at: SystemTime, due_at: SystemTime, now: SystemTime) -> i32 {
    let span = due_at
        .duration_since(started_at)
        .unwrap_or_default()
        .as_secs_f64();
    if span <= 0.0 {
        return 100;
    }
    let elapsed = now
        .duration_since(started_at)
        .unwrap_or_default()
        .as_secs_f64();
    ((elapsed / span) * 100.0).clamp(0.0, 100.0).round() as i32
}

/// A stored instant as the modelled `timestamp` the wire carries.
pub fn to_date_time(instant: SystemTime) -> ::aws_smithy_types::DateTime {
    let since_epoch = instant
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    ::aws_smithy_types::DateTime::from_secs_f64(since_epoch.as_secs_f64())
}

/// The longest a tag key may be: the `@length` maximum of `TagMap`'s key shape `TagKey`.
pub const MAX_TAG_KEY_LENGTH: usize = 128;

/// The expression `TagKey` declares, as the model spells it. Its leading negative
/// lookahead is what reserves the `aws:` prefix.
pub const TAG_KEY_PATTERN: &str = r"^(?!aws:)[a-zA-Z+-=._:/]+$";

/// The prefix the `TagKey` pattern's lookahead excludes: AWS keeps it for its own tags.
pub const RESERVED_TAG_KEY_PREFIX: &str = "aws:";

/// Why `key` is not a tag key `TagMap` admits at `member`, or `None` when it is one.
///
/// The constraints are the model's own, on the map's KEY shape: `@length(min 1, max 128)`
/// and [`TAG_KEY_PATTERN`]. They are enforced here because a map's key is not a leaf the
/// boundary's constraint walk validates — it captures the key as the path segment of that
/// entry's VALUE, so a key reaches a handler unchecked — and this is the one place a tag
/// key is held to them, shared by every operation that takes a tag map.
///
/// The sentence is NOT written here: it is built from the framework's own violation and
/// rendered by its one envelope, so a key refused by a handler reads exactly like a value
/// the boundary refused for the same constraint class.
pub fn tag_key_violation(member: &str, key: &str) -> Option<String> {
    let length = key.chars().count();
    let kind = if length < 1 {
        ::aws_smithy_schema_server::constraints::ViolationKind::MinLength(1)
    } else if length > MAX_TAG_KEY_LENGTH {
        ::aws_smithy_schema_server::constraints::ViolationKind::MaxLength(
            MAX_TAG_KEY_LENGTH as i64,
        )
    } else if !admits_tag_key(key) {
        ::aws_smithy_schema_server::constraints::ViolationKind::Pattern(
            TAG_KEY_PATTERN.to_string(),
        )
    } else {
        return None;
    };
    let violation = ::aws_smithy_schema_server::constraints::Violation {
        path: ::aws_smithy_schema_server::constraints::path::member(member),
        kind,
        // Already held to the key's own modelled maximum, because `Violation::message`
        // interpolates the value verbatim and a key arrives as long as the caller chose.
        value: Some(echoed_tag_key(key)),
        sensitive: false,
    };
    ::aws_smithy_schema_server::constraints::envelope(std::slice::from_ref(&violation))
}

/// The first tag key of `tags` that [`tag_key_violation`] refuses, in ascending key order
/// so one request earns one sentence however the map happened to iterate.
pub fn first_tag_key_violation(
    member: &str,
    tags: Option<&HashMap<String, String>>,
) -> Option<String> {
    let mut keys: Vec<&String> = tags?.keys().collect();
    keys.sort();
    keys.into_iter()
        .find_map(|key| tag_key_violation(member, key))
}

/// Whether every character of `key` is one [`TAG_KEY_PATTERN`] admits, and the key does
/// not carry the reserved prefix.
///
/// The character set is the pattern's own: ASCII letters, the `+`-to-`=` range it spells
/// (which covers the digits and `, - . /` and `: ; < =`), and `.`, `_`, `:`, `/`.
fn admits_tag_key(key: &str) -> bool {
    !key.starts_with(RESERVED_TAG_KEY_PREFIX)
        && key.chars().all(|character| {
            character.is_ascii_alphabetic()
                || ('+'..='=').contains(&character)
                || matches!(character, '.' | '_' | ':' | '/')
        })
}

/// `key` as a refusal quotes it: at most the key's own modelled maximum of characters, so
/// a message cannot be sized by the request.
fn echoed_tag_key(key: &str) -> String {
    key.chars().take(MAX_TAG_KEY_LENGTH).collect()
}

/// The server fault every handler answers with when the store cannot be reached. One
/// wording, so a caller sees the same failure whichever operation it called.
pub fn store_unavailable() -> crate::ops::types::InternalServerException {
    crate::ops::types::InternalServerException {
        message: Some("the Customer Profiles store is unavailable".to_string()),
    }
}

/// The `ResourceNotFoundException` message for a profile object type a domain does not
/// hold, as the service words that refusal: the resource kind, the name the request
/// carried, and "not found". Shared by every operation addressing one object type, so
/// the same absent name reads the same whichever of them was called.
pub fn object_type_not_found(object_type_name: &str) -> String {
    format!("ObjectType {object_type_name} not found")
}

/// The matching configuration a request carries, as the domain stores and reports it.
/// `MatchingResponse` has no clock-derived member, so the stored value is the whole
/// answer. Shared by the operations that configure a domain, so a configuration reads
/// back the same however it was submitted.
pub fn matching_response(request: MatchingRequest) -> MatchingResponse {
    MatchingResponse {
        enabled: Some(request.enabled),
        job_schedule: request.job_schedule,
        auto_merging: request.auto_merging,
        exporting_config: request.exporting_config,
    }
}

/// The rule-based matching configuration a request carries, as the domain stores it:
/// `Status` is left unset because it is a function of the clock and is resolved on every
/// read by [`Domain::rule_based_matching_response`]. Shared by the operations that
/// configure a domain, so a configuration reads back the same however it was submitted.
pub fn rule_based_matching_response(request: RuleBasedMatchingRequest) -> RuleBasedMatchingResponse {
    RuleBasedMatchingResponse {
        enabled: Some(request.enabled),
        matching_rules: request.matching_rules,
        status: None,
        max_allowed_rule_level_for_merging: request.max_allowed_rule_level_for_merging,
        max_allowed_rule_level_for_matching: request.max_allowed_rule_level_for_matching,
        attribute_types_selector: request.attribute_types_selector,
        conflict_resolution: request.conflict_resolution,
        exporting_config: request.exporting_config,
    }
}

/// A member documented to clear the stored value when it arrives as an empty string:
/// anything else replaces it.
pub fn clear_if_empty(value: String) -> Option<String> {
    (!value.is_empty()).then_some(value)
}

/// The `ResourceNotFoundException` an operation answers with when the service sends that
/// refusal carrying no prose at all. The model leaves `Message` optional, so the member
/// is OMITTED from the body rather than filled from one of the sentences below: whether a
/// refusal carries a sentence is a property of the OPERATION that raises it, and an
/// operation that sends none must not invent one.
pub fn not_found_without_message() -> crate::ops::types::ResourceNotFoundException {
    crate::ops::types::ResourceNotFoundException { message: None }
}

/// The `ResourceNotFoundException` message for a domain this emulator does not hold, as
/// the service words that refusal: the resource kind and the name the request carried.
/// The domain-scoped operations whose refusal DOES carry a sentence answer with this, so
/// the same absent name reads the same whichever of them was called.
pub fn domain_not_found(domain_name: &str) -> String {
    format!("Domain {domain_name} is not found")
}

/// The `ResourceNotFoundException` message the PROFILE-COLLECTION operations word an
/// absent domain with: the operations that create or search the profiles of a domain
/// (`POST /domains/{DomainName}/profiles` and `POST /domains/{DomainName}/profiles/search`)
/// report that absence in their own fixed sentence, which names the input MEMBER rather
/// than the resource kind and says which account the domain was looked for in. Its
/// variable parts are the `DomainName` the request's URI label carried and this
/// emulator's own account id — never a literal. Shared by those operations, so the same
/// absent name reads the same whichever of them was called.
pub fn profile_collection_domain_not_found(domain_name: &str) -> String {
    format!(
        "DomainName {domain_name} is not found in account {}",
        ::engine::emulator_account_id()
    )
}

/// The `ResourceNotFoundException` message the UPLOAD-JOB operations word an absent
/// domain with: the service reports that absence from its upload-job path in its own
/// fixed sentence, whose only variable part is the `DomainName` the request's URI label
/// carried. Shared by every operation on the upload-job surface, so the same absent name
/// reads the same whichever of them was called.
pub fn upload_job_domain_not_found(domain_name: &str) -> String {
    format!("Domain with name {domain_name} could not be found")
}

/// The `ResourceNotFoundException` message for a profile a domain does not hold, as the
/// service words that refusal: a fixed sentence naming the resource kind and referring to
/// the domain generically, whose ONLY variable part is the `ProfileId` the request carried
/// — the domain's own name is not interpolated, even though the request carries it. Shared
/// by every operation addressing one profile by id, so the same absent id reads the same
/// whichever of them was called.
pub fn profile_not_found(profile_id: &str) -> String {
    format!("Profile with profileId {profile_id} is not found in the domain")
}

/// The `ResourceNotFoundException` message for a profile object the domain holds no
/// record of under the object type and profile the request named, worded like every other
/// per-resource absence this service reports: the resource kind, the identifier the
/// request carried, and "not found" (the form `object_type_not_found` takes).
pub fn profile_object_not_found(profile_object_unique_key: &str) -> String {
    format!("ProfileObject {profile_object_unique_key} not found")
}

/// The `ResourceNotFoundException` message for a recommender schema a domain does not
/// hold, worded like every other per-domain resource this service reports absent: the
/// resource kind, the name the request carried, and the domain it was looked for in.
pub fn recommender_schema_not_found(domain_name: &str, recommender_schema_name: &str) -> String {
    format!(
        "The recommender schema {recommender_schema_name} does not exist in domain {domain_name}"
    )
}

/// The `ResourceNotFoundException` message for a calculated attribute definition a domain
/// does not hold, as the service words that refusal: the fixed sentence naming the
/// resource kind and "not found", whose only variable part is the name the request's
/// `{CalculatedAttributeName}` label carried. It is the sibling of the already-exists
/// sentence `CreateCalculatedAttributeDefinition` refuses a held name with, and shared by
/// every operation addressing one definition, so the same absent name reads the same
/// whichever of them was called.
pub fn calculated_attribute_not_found(calculated_attribute_name: &str) -> String {
    format!("Calculated attribute definition with name {calculated_attribute_name} not found")
}

/// One recommender schema's `RecommenderSchemaArn`: this service's ARN namespace, the
/// region the schema was created in, and the domain-scoped resource path the schema is
/// addressed by — the same path its operations are bound to.
pub fn recommender_schema_arn(
    region: &str,
    domain_name: &str,
    recommender_schema_name: &str,
) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("domains/{domain_name}/recommender-schemas/{recommender_schema_name}"),
    )
}

/// The region to build a new resource's ARN in: the one the caller signed for, else
/// [`DEFAULT_REGION`]. An ARN is minted once and stored, so a later read from another
/// region still reports the ARN the resource was created with.
pub fn request_region(ctx: &smithy_server_runtime::OperationContext<'_>) -> String {
    ::emulator_server::sigv4_credential_scope(&ctx.parts().headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsCustomerprofilesCustomerprofiles20200815State>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsCustomerprofilesCustomerprofiles20200815State>)
}
