// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-resourceexplorer2-resourceexplorer service.
//!
//! Everything the implemented operations answer with is derived from the values
//! stored here: one index per Region, the views created in each Region, the
//! Region's default view, and the resource inventory the Region's index holds.
//!
//! This module also carries the few rules more than one operation needs, because
//! an operation file is the only file for that operation and may not be reached
//! from a sibling:
//!
//! * [`ComAmazonawsResourceexplorer2ResourceexplorerState::settle`] — the
//!   clock-owed index lifecycle transitions (`CREATING` -> `ACTIVE`, `UPDATING` ->
//!   `ACTIVE`, `DELETING` -> `DELETED`, taking the Region's views with it), and the
//!   setup-task steps that wait on them.
//! * [`ComAmazonawsResourceexplorer2ResourceexplorerState::insert_index`] /
//!   [`ComAmazonawsResourceexplorer2ResourceexplorerState::insert_view`] — the ONE
//!   place an index or a view is created, so the operations that create one cannot
//!   disagree about its ARN or its stored shape.
//! * [`ComAmazonawsResourceexplorer2ResourceexplorerState::index_for_new_view`] — the
//!   index a new view is queried through, and the account-level setup a Region nothing
//!   has acted on is served as already having.
//! * [`IndexEntry::to_wire`] / [`ViewEntry::to_wire`] — the modelled `Index` and
//!   `View` structures, so `CreateView` and `GetView` cannot disagree about what a
//!   stored view looks like.
//! * [`SetupTask`] — the multi-Region setup tasks
//!   `CreateResourceExplorerSetup` starts and `GetResourceExplorerSetup` reports on.
//! * [`paginate`] / [`encode_page_token`] — the opaque `NextToken` cursors the
//!   `@paginated` operations share. The offset travels INSIDE the token, so a
//!   `@readonly` list operation never writes cursor state.
//! * [`query_matches`] — the Resource Explorer search query syntax, used by
//!   `Search` and `ListResources` for both the caller's own query and the view's
//!   `Filters.FilterString`, with [`is_filter_name`] / [`free_form_term`] telling a
//!   filter prefix from free-form text.
//! * [`ComAmazonawsResourceexplorer2ResourceexplorerState::query_view`] — the view a
//!   query must use, and the single refusal both ways of failing to resolve one carry.
//! * [`IndexedResource::to_wire`] — the modelled `Resource` structure, so the two query
//!   operations cannot disagree about how an indexed resource is reported.
//! * [`owners`] — the owning services whose resources a Region's index holds, read from
//!   those services' own records at query time.
//! * [`supported_types`] — the service-wide supported-type catalogue
//!   `ListSupportedResourceTypes` publishes, which is static service metadata rather than
//!   anything this store holds.

use std::collections::HashMap;
use std::sync::OnceLock;
use std::time::{Duration, SystemTime};

use regex::Regex;
use sha2::{Digest, Sha256};

mod setup_tasks;
pub use setup_tasks::*;

pub mod owners;

pub mod supported_types;

/// The ARN namespace, SigV4 signing name and endpoint prefix of this service, and
/// therefore the `Service` of every resource it owns.
pub const SERVICE_NAMESPACE: &str = "resource-explorer-2";

/// The Region an unsigned request (one carrying no SigV4 credential scope to read
/// a Region from) is served in. Generated emulators default to `us-east-1`.
pub const DEFAULT_REGION: &str = "us-east-1";

/// How long after `CreateIndex`, `DeleteIndex` or `UpdateIndexType` the index's
/// non-terminal state (`CREATING` / `DELETING` / `UPDATING`) settles.
///
/// CHOSEN, not documented: AWS gives no duration for index creation, deletion or a
/// type change, only that all three are performed by asynchronous background tasks.
/// The span is deliberately non-zero, so a caller that reads straight after the call
/// still observes the transient state, and short enough that nothing has to sleep for
/// long.
pub const INDEX_SETTLE_DELAY: Duration = Duration::from_millis(500);

/// `Search` returns at most this many resources in total, and `Count` cannot
/// exceed it. Documented on `SearchInput$QueryString`: "You can specify an empty
/// string to return all results up to the limit of 1,000 total results. The
/// operation can return only the first 1,000 results."
///
/// `ListResources` is bounded by the same total, which is what makes its own
/// documented pagination rule hold: "The ListResources operation does not generate a
/// NextToken if you set MaxResults to 1000" (`ListResourcesInput$NextToken`) — a page
/// of 1,000 already carries every result the operation can return, so there is nothing
/// left for a token to resume at.
pub const SEARCH_RESULT_LIMIT: usize = 1000;

/// The maximum number of views one Region holds, from Resource Explorer's "Views per
/// Region" service quota (an adjustable default of 100).
///
/// It is also what BOUNDS the stored view map: `CreateView` is caller-driven, so without
/// a ceiling a caller could grow `views` without end. Reaching it is the operation's own
/// modelled `ServiceQuotaExceededException`, which carries the quota's name and value.
pub const VIEWS_PER_REGION_QUOTA: usize = 100;

/// The most tag key/value pairs one index or view carries, from the Amazon Web Services
/// tagging limit of 50 tags per resource.
///
/// It is also what BOUNDS a resource's stored tag map: `TagResource` is caller-driven and
/// a tag lives as long as the resource it is attached to, so without a ceiling a caller
/// could grow one resource's tags without end. Reaching it is the operation's own modelled
/// `ValidationException`, which names the member at fault and the count the request would
/// leave behind.
pub const MAX_TAGS_PER_RESOURCE: usize = 50;

/// The input member a tagging refusal blames, named the way the model declares it.
pub const TAGS_MEMBER: &str = "Tags";

/// The `ViewName` of the view a Region gets when Resource Explorer is turned on there.
///
/// Turning the service on in a Region indexes that Region's resources "so that they can
/// be queried by using the `Search` operation" (`CreateIndex`'s own documentation), and
/// every query must use a view — so the Region starts with one unfiltered view, which is
/// also its default. A caller that wants a scoped view creates its own with `CreateView`
/// and can make that one the default with `AssociateDefaultView`.
pub const DEFAULT_VIEW_NAME: &str = "all-resources";

/// Whether `region` is an AWS Region this emulator serves.
///
/// A request's Region comes from its SigV4 credential scope, which is caller-controlled
/// text, and the index map is KEYED by Region — so the key space has to be the closed set
/// of AWS Regions rather than whatever the header says. The check is
/// `emulator_telemetry::Region::new`, the workspace's one validator for a Region token: it
/// enforces the closed grammar, the maximum length AND membership of
/// `policy::KNOWN_REGIONS`. Creating an index is therefore refused for a Region that has
/// no endpoint, and an account's indexes are bounded by that list.
pub fn is_servable_region(region: &str) -> bool {
    ::emulator_telemetry::Region::new(region).is_ok()
}

/// How a refusal reads when the Region a request is addressed to is not an Amazon Web
/// Services Region at all, with that token as its only variable part.
///
/// Shared, because every operation that keys something on the request's Region refuses
/// the same way and they may not word it differently.
pub fn not_a_region_message(region: &str) -> String {
    format!("{region} is not an Amazon Web Services Region.")
}

/// The lifecycle of an index, as the `IndexState` enum names it on the wire.
///
/// Every one of the model's five `IndexState` values is reachable here. `DELETED` is
/// TERMINAL: the record of a deleted index keeps it instead of disappearing, so an
/// operation naming that index afterwards is answered from a record in a terminal state
/// rather than from nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IndexLifecycle {
    /// The index was just created and has not finished coming up.
    Creating,
    /// The index is usable.
    Active,
    /// The index is being torn down, along with the Region's views.
    Deleting,
    /// The deletion finished: "Resource Explorer successfully deleted the index"
    /// (`IndexState$DELETED`). The index holds nothing and backs nothing, and the
    /// Region is free to hold a new one.
    Deleted,
    /// The index's type is being switched between `LOCAL` and `AGGREGATOR`, which is
    /// what starts or stops replication with the account's other Regions.
    Updating,
}

impl IndexLifecycle {
    /// The `IndexState` value this state is called on the wire.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Creating => "CREATING",
            Self::Active => "ACTIVE",
            Self::Deleting => "DELETING",
            Self::Deleted => "DELETED",
            Self::Updating => "UPDATING",
        }
    }

    /// Whether this is the terminal state a deleted index keeps.
    pub fn is_deleted(self) -> bool {
        matches!(self, Self::Deleted)
    }

    /// Whether an index in this state still occupies its Region — anything but the
    /// terminal `DELETED`, which leaves the Region free to hold a new index.
    pub fn occupies_region(self) -> bool {
        !self.is_deleted()
    }
}

/// The `IndexType` value `CreateIndex` produces: "This operation creates only a
/// *local* index" (`CreateIndex` documentation).
pub const INDEX_TYPE_LOCAL: &str = "LOCAL";

/// The optional resource property a view can ask `Search` to report, as
/// `IncludedProperty$Name` names it.
pub const TAGS_PROPERTY: &str = "tags";

/// Every value `IncludedProperty$Name` accepts.
///
/// A CLOSED set, from that member's own documentation: "You can specify the following
/// property names for this field: `tags`". A view is caller-supplied, so the names it
/// carries have to come from this list rather than from the request — an unlisted name
/// would otherwise be stored as a property that nothing can ever report.
pub const VIEW_PROPERTY_NAMES: &[&str] = &[TAGS_PROPERTY];

/// Whether `name` is a property name a view may include, compared case-insensitively
/// like every other Resource Explorer comparison.
pub fn is_view_property(name: &str) -> bool {
    VIEW_PROPERTY_NAMES
        .iter()
        .any(|known| known.eq_ignore_ascii_case(name))
}

/// How a view write refuses an `IncludedProperties` entry outside the catalog: the
/// message names the member at fault and the values it does accept, so `CreateView` and
/// `UpdateView` cannot word the same refusal differently.
pub fn unsupported_property_message(name: &str) -> String {
    format!(
        "{name} is not a supported value for IncludedProperties: the supported values \
         are {}.",
        VIEW_PROPERTY_NAMES.join(", ")
    )
}

/// The `IndexType` value of the one index that collects the account's other Regions:
/// "You can have only one aggregator index per Amazon Web Services account"
/// (`UpdateIndexType` documentation). It is the type that "receive[s] replicated
/// index information from all other Regions where there is a user-owned index"
/// (`CreateResourceExplorerSetupInput$AggregatorRegions`).
pub const INDEX_TYPE_AGGREGATOR: &str = "AGGREGATOR";

/// The two values the model's `IndexType` enum admits, which are therefore the only
/// types an index can be switched to.
pub const INDEX_TYPES: [&str; 2] = [INDEX_TYPE_LOCAL, INDEX_TYPE_AGGREGATOR];

/// How long after the account's aggregator index is demoted to `LOCAL` another index
/// may be promoted in its place.
///
/// DOCUMENTED, not chosen: "After you demote an aggregator index to a local index, you
/// must wait 24 hours before you can promote another index to be the new aggregator
/// index for the account" (`UpdateIndexType` documentation).
pub const AGGREGATOR_PROMOTION_WAIT: Duration = Duration::from_secs(24 * 60 * 60);

/// How the account's one-aggregator rule reads when a promotion runs into it, in the
/// words `ConflictException` and `UpdateIndexType` document for exactly this case. It
/// carries nothing from the request: which index is already the aggregator is not part
/// of what AWS says here.
///
/// Shared, because more than one operation can promote an index to `AGGREGATOR` and
/// they may not disagree about how the refusal reads.
pub const AGGREGATOR_EXISTS_MESSAGE: &str =
    "You already have an AGGREGATOR index in a different Amazon Web Services Region. \
     Before you can promote a different index to be the aggregator index for the \
     account, you must first demote the existing aggregator index to type LOCAL.";

/// How the wait after a demotion reads when a promotion runs into it, with the
/// documented [`AGGREGATOR_PROMOTION_WAIT`] as its only variable part.
pub fn aggregator_wait_message() -> String {
    format!(
        "You must wait {} hours after demoting an aggregator index to a local index \
         before you can promote another index to be the new aggregator index for the \
         account.",
        AGGREGATOR_PROMOTION_WAIT.as_secs() / 3_600
    )
}

/// The account's most recent demotion of its aggregator index back to `LOCAL`.
///
/// Kept because [`AGGREGATOR_PROMOTION_WAIT`] runs from that instant: the wait is a
/// property of the ACCOUNT, not of any one index, so it outlives the index type that
/// caused it.
#[derive(Debug, Clone)]
pub struct AggregatorDemotion {
    /// The index that was demoted, by ARN.
    pub index_arn: String,
    /// When the demotion was requested.
    pub at: SystemTime,
}

/// One Region's index.
#[derive(Debug, Clone)]
pub struct IndexEntry {
    /// `arn:aws:resource-explorer-2:<region>:<account>:index/<uuid>`.
    pub arn: String,
    pub region: String,
    /// `LOCAL` or `AGGREGATOR`.
    pub index_type: String,
    pub state: IndexLifecycle,
    /// When the clock owes this index its next state change, if it is in a
    /// non-terminal one. Never slept on: the transition is materialised by
    /// whichever call next looks at the state.
    pub settles_at: Option<SystemTime>,
    pub created_at: SystemTime,
    pub last_updated_at: SystemTime,
    pub tags: HashMap<String, String>,
    /// The `@idempotencyToken` the creating request carried, for replaying it.
    pub client_token: Option<String>,
}

impl IndexEntry {
    /// The modelled `Index` structure for this stored index.
    ///
    /// The ONE place an `Index` is built, so the operations that report one cannot
    /// disagree about what a stored index looks like.
    pub fn to_wire(&self) -> crate::ops::types::Index {
        crate::ops::types::Index {
            region: Some(self.region.clone()),
            arn: Some(self.arn.clone()),
            r#type: Some(self.index_type.clone()),
        }
    }
}

/// One view, as `CreateView` stored it.
#[derive(Debug, Clone)]
pub struct ViewEntry {
    /// `arn:aws:resource-explorer-2:<region>:<account>:view/<name>/<uuid>`.
    pub view_arn: String,
    /// Unique within its Region.
    pub view_name: String,
    pub region: String,
    /// The account that owns the view.
    pub owner: String,
    /// The account, organization or OU ARN the view's results are scoped to.
    pub scope: String,
    /// `SearchFilter$FilterString`, ANDed into every `Search` through this view.
    pub filter_string: Option<String>,
    /// `IncludedProperty$Name` values, e.g. `tags`.
    pub included_properties: Vec<String>,
    pub created_at: SystemTime,
    pub last_updated_at: SystemTime,
    pub tags: HashMap<String, String>,
    /// The `@idempotencyToken` the creating request carried, for replaying it.
    pub client_token: Option<String>,
}

impl ViewEntry {
    /// The modelled `View` structure for this stored view.
    ///
    /// The ONE place a `View` is built, so `CreateView` cannot return a shape that
    /// `GetView` reads back differently.
    pub fn to_wire(&self) -> crate::ops::types::View {
        crate::ops::types::View {
            view_arn: Some(self.view_arn.clone()),
            view_name: Some(self.view_name.clone()),
            owner: Some(self.owner.clone()),
            last_updated_at: Some(::aws_smithy_types::DateTime::from(self.last_updated_at)),
            scope: Some(self.scope.clone()),
            included_properties: Some(
                self.included_properties
                    .iter()
                    .map(|name| crate::ops::types::IncludedProperty { name: name.clone() })
                    .collect(),
            ),
            filters: self
                .filter_string
                .as_ref()
                .map(|filter_string| crate::ops::types::SearchFilter {
                    filter_string: filter_string.clone(),
                }),
        }
    }

    /// Whether this view asks for the named optional property
    /// (`IncludedProperty$Name`) in search results. Case-insensitive, like every
    /// other Resource Explorer comparison.
    pub fn includes_property(&self, name: &str) -> bool {
        self.included_properties
            .iter()
            .any(|included| included.eq_ignore_ascii_case(name))
    }
}

/// A resource the Region's index holds, in the form the query operations report it.
///
/// The emulator indexes the resources it can actually observe: the ones this
/// service owns in that Region — its index and its views — plus the ones the
/// services in [`owners`] hold there, each described from the values persisted
/// for it by the service that OWNS it, never from the caller's request.
#[derive(Debug, Clone)]
pub struct IndexedResource {
    pub arn: String,
    pub owning_account_id: String,
    pub region: String,
    pub service: String,
    /// `<service>:<type>`, as Resource Explorer derives it from the ARN.
    pub resource_type: String,
    /// The CloudFormation type name for this resource type, when there is one.
    pub cfn_resource_type: Option<String>,
    pub last_reported_at: SystemTime,
    pub tags: HashMap<String, String>,
}

impl IndexedResource {
    /// Every value a free-form (unprefixed) query term is matched against.
    pub fn searchable_fields(&self) -> Vec<&str> {
        let mut fields = vec![
            self.arn.as_str(),
            self.owning_account_id.as_str(),
            self.region.as_str(),
            self.service.as_str(),
            self.resource_type.as_str(),
        ];
        if let Some(cfn) = &self.cfn_resource_type {
            fields.push(cfn.as_str());
        }
        for (key, value) in &self.tags {
            fields.push(key.as_str());
            fields.push(value.as_str());
        }
        fields
    }

    /// The modelled `Resource` structure for this indexed resource.
    ///
    /// The ONE place a `Resource` is built, so `Search` and `ListResources` cannot
    /// disagree about what an indexed resource looks like.
    ///
    /// `include_tags` is whether the view the query used asks for the
    /// [`TAGS_PROPERTY`]: `IncludedProperties` "specifies optional fields that you
    /// want included in search results from this view", and `tags` is the one
    /// property name the model documents — so a view that does not ask for it gets
    /// no properties at all.
    pub fn to_wire(&self, include_tags: bool) -> crate::ops::types::Resource {
        crate::ops::types::Resource {
            arn: Some(self.arn.clone()),
            owning_account_id: Some(self.owning_account_id.clone()),
            region: Some(self.region.clone()),
            resource_type: Some(self.resource_type.clone()),
            service: Some(self.service.clone()),
            cfn_resource_type: self.cfn_resource_type.clone(),
            last_reported_at: Some(::aws_smithy_types::DateTime::from(self.last_reported_at)),
            properties: include_tags.then(|| {
                vec![crate::ops::types::ResourceProperty {
                    name: Some(TAGS_PROPERTY.to_string()),
                    last_reported_at: Some(::aws_smithy_types::DateTime::from(
                        self.last_reported_at,
                    )),
                    data: Some(tags_property_data(&self.tags)),
                }]
            }),
        }
    }
}

/// The `tags` resource property's `Data` document for one resource's tag set.
///
/// `ResourceProperty$Data` is a `@document` whose "content of this field is a JSON object
/// that varies based on the resource type"
/// (`smithy_query(query="shape",shape="ResourceProperty")`), and what Resource Explorer
/// puts in it for the `tags` property is the resource's tag set as a LIST, one entry per
/// tag, each a structure with the members `Key` and `Value` — the same form every AWS
/// tagging API reports a tag set in. A client walking a resource's tags iterates that
/// list and reads those two members, so a flat `{key: value}` object is a document no such
/// walk can read.
///
/// The entries are ordered by tag key, so two reads of one unchanged resource report the
/// same document (the stored tag set is a hash map, whose iteration order is not stable).
fn tags_property_data(tags: &HashMap<String, String>) -> ::aws_smithy_types::Document {
    let mut entries: Vec<(&String, &String)> = tags.iter().collect();
    entries.sort_by_key(|(key, _)| *key);
    ::aws_smithy_types::Document::Array(
        entries
            .into_iter()
            .map(|(key, value)| {
                ::aws_smithy_types::Document::Object(
                    [
                        (
                            TAG_KEY_MEMBER.to_string(),
                            ::aws_smithy_types::Document::String(key.clone()),
                        ),
                        (
                            TAG_VALUE_MEMBER.to_string(),
                            ::aws_smithy_types::Document::String(value.clone()),
                        ),
                    ]
                    .into_iter()
                    .collect(),
                )
            })
            .collect(),
    )
}

/// The member one entry of the [`TAGS_PROPERTY`] document names a tag's key in.
const TAG_KEY_MEMBER: &str = "Key";

/// The member one entry of the [`TAGS_PROPERTY`] document carries that key's value in.
const TAG_VALUE_MEMBER: &str = "Value";

/// Per-service state for com-amazonaws-resourceexplorer2-resourceexplorer. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsResourceexplorer2ResourceexplorerState {
    /// At most one index per Region: "You can create only one index in a Region."
    pub indexes: HashMap<String, IndexEntry>,
    /// Every view, keyed by its own ARN.
    pub views: HashMap<String, ViewEntry>,
    /// The default view of each Region, as `AssociateDefaultView` set it.
    pub default_view_arns: HashMap<String, String>,
    /// How many resources this store has minted, so the identifier it derives for the
    /// next one cannot collide with any it has already handed out.
    pub minted_resources: u64,
    /// The multi-Region setup tasks this store has recorded, keyed by `TaskId`.
    pub setup_tasks: HashMap<String, SetupTask>,
    /// The last time the account's aggregator index was demoted to `LOCAL`, if ever:
    /// what [`Self::aggregator_promotion_blocked_until`] measures the documented wait
    /// from.
    pub aggregator_demotion: Option<AggregatorDemotion>,
}

/// Everything one stored view's members come from, for
/// [`ComAmazonawsResourceexplorer2ResourceexplorerState::insert_view`].
#[derive(Debug, Clone, Default)]
pub struct NewView {
    pub view_name: String,
    pub region: String,
    /// Absent means the default `CreateView` documents: "the value defaults to the
    /// Amazon Web Services account used to call this operation".
    pub scope: Option<String>,
    pub filter_string: Option<String>,
    pub included_properties: Vec<String>,
    pub tags: HashMap<String, String>,
    pub client_token: Option<String>,
}

impl ComAmazonawsResourceexplorer2ResourceexplorerState {
    /// Mint the identifier for a resource of `kind` this store is creating now: the
    /// `<uuid>` segment of a Resource Explorer index or view ARN.
    ///
    /// Uniqueness comes from the PERSISTED [`minted_resources`] counter, not from
    /// randomness, so two resources created in the same instant can never share an
    /// identifier. That counter, the resource kind and the creating instant are hashed
    /// with SHA-256 (the `sha2` crate) and the first 16 bytes are laid out as the
    /// 8-4-4-4-12 hex form, with the version and variant nibbles RFC 4122 requires —
    /// version 8 ("custom"), because the value is derived rather than random, and the
    /// digest is what keeps the counter and the clock out of the identifier.
    ///
    /// [`minted_resources`]: Self::minted_resources
    pub fn mint_resource_id(&mut self, kind: &str, now: SystemTime) -> String {
        let serial = self.minted_resources;
        self.minted_resources = self.minted_resources.wrapping_add(1);
        let nanos = now
            .duration_since(SystemTime::UNIX_EPOCH)
            .map_or(0, |since| since.as_nanos());

        let mut hasher = Sha256::new();
        hasher.update(kind.as_bytes());
        hasher.update(serial.to_be_bytes());
        hasher.update(nanos.to_be_bytes());
        let digest = hasher.finalize();

        let mut bytes = [0u8; 16];
        bytes.copy_from_slice(&digest[..16]);
        bytes[6] = (bytes[6] & 0x0f) | 0x80;
        bytes[8] = (bytes[8] & 0x3f) | 0x80;
        let hex = ::hex::encode(bytes);
        format!(
            "{}-{}-{}-{}-{}",
            &hex[0..8],
            &hex[8..12],
            &hex[12..16],
            &hex[16..20],
            &hex[20..32]
        )
    }

    /// Materialise every index lifecycle transition the clock already owes at
    /// `now`, and nothing else.
    ///
    /// Each transition was due at `now` whether or not anybody called, so running
    /// it here — rather than on a timer — changes no outcome a caller could
    /// observe. Every handler, including the `@readonly` ones, calls this first so
    /// they all answer from the same settled state.
    pub fn settle(&mut self, now: SystemTime) {
        let due: Vec<(String, IndexLifecycle)> = self
            .indexes
            .iter()
            .filter(|(_, index)| index.settles_at.is_some_and(|at| at <= now))
            .map(|(region, index)| (region.clone(), index.state))
            .collect();
        for (region, state) in due {
            match state {
                IndexLifecycle::Creating => {
                    if let Some(index) = self.indexes.get_mut(&region) {
                        index.state = IndexLifecycle::Active;
                        index.settles_at = None;
                        index.last_updated_at = now;
                    }
                }
                IndexLifecycle::Deleting => {
                    // "Resource Explorer also deletes all views in that Region."
                    self.views.retain(|_, view| view.region != region);
                    self.default_view_arns.remove(&region);
                    // The index itself stops being an index the Region holds, but its
                    // record stays in the terminal `DELETED` state: a deletion that
                    // finished is not the same thing as an index that never existed, and
                    // an operation naming the ARN afterwards has to be able to tell them
                    // apart.
                    if let Some(index) = self.indexes.get_mut(&region) {
                        index.state = IndexLifecycle::Deleted;
                        index.settles_at = None;
                        index.last_updated_at = now;
                        index.tags.clear();
                    }
                }
                IndexLifecycle::Updating => {
                    // "When the asynchronous tasks complete, the Status response of
                    // that operation changes from UPDATING to ACTIVE."
                    if let Some(index) = self.indexes.get_mut(&region) {
                        index.state = IndexLifecycle::Active;
                        index.settles_at = None;
                        index.last_updated_at = now;
                    }
                }
                IndexLifecycle::Active | IndexLifecycle::Deleted => {
                    // Neither state is ever put on the clock, so nothing is owed.
                }
            }
        }
        // The setup-task steps wait on the index lifecycle above, so they settle
        // against the state it just left behind.
        self.settle_setup_tasks(now);
    }

    /// Create `region`'s index, as the operations that turn Resource Explorer on in
    /// a Region do.
    ///
    /// The ONE place an index is created: indexing is an asynchronous background
    /// task, so the index starts out `CREATING` and the clock owes it `ACTIVE`.
    ///
    /// Turning Resource Explorer on in a Region stores that Region's resources "so that
    /// they can be queried by using the `Search` operation" (`CreateIndex`'s own
    /// documentation), and every query must use a view — so a Region being turned on
    /// also gets the view a query that names none uses: one unfiltered view named
    /// [`DEFAULT_VIEW_NAME`], which is that Region's default. A Region that already
    /// holds a view of its own keeps what it has; `AssociateDefaultView` is what moves
    /// the pointer afterwards and `DisassociateDefaultView` what clears it.
    pub fn insert_index(
        &mut self,
        region: &str,
        index_type: &str,
        tags: HashMap<String, String>,
        client_token: Option<String>,
        now: SystemTime,
    ) -> IndexEntry {
        let resource_id = self.mint_resource_id("index", now);
        let entry = IndexEntry {
            arn: ::engine::emulator_arn(
                SERVICE_NAMESPACE,
                region,
                &format!("index/{resource_id}"),
            ),
            region: region.to_string(),
            index_type: index_type.to_string(),
            state: IndexLifecycle::Creating,
            settles_at: Some(now + INDEX_SETTLE_DELAY),
            created_at: now,
            last_updated_at: now,
            tags,
            client_token,
        };
        self.indexes.insert(region.to_string(), entry.clone());
        if self.views_in_region(region).is_empty() {
            let view = self.insert_view(
                NewView {
                    view_name: DEFAULT_VIEW_NAME.to_string(),
                    region: region.to_string(),
                    ..NewView::default()
                },
                now,
            );
            self.default_view_arns
                .insert(region.to_string(), view.view_arn);
        }
        // What was stored, so the caller reports the stored values and nothing else.
        entry
    }

    /// Create the view `new` describes in its Region, as the operations that create
    /// a view do.
    ///
    /// The ONE place a view is created, so a view created as part of a multi-Region
    /// setup is indistinguishable from one `CreateView` made.
    pub fn insert_view(&mut self, new: NewView, now: SystemTime) -> ViewEntry {
        let resource_id = self.mint_resource_id("view", now);
        let entry = ViewEntry {
            view_arn: ::engine::emulator_arn(
                SERVICE_NAMESPACE,
                &new.region,
                &format!("view/{}/{resource_id}", new.view_name),
            ),
            view_name: new.view_name,
            region: new.region,
            owner: ::engine::emulator_account_id().to_string(),
            scope: new
                .scope
                .unwrap_or_else(|| ::engine::emulator_arn("iam", "", "root")),
            filter_string: new.filter_string,
            included_properties: new.included_properties,
            created_at: now,
            last_updated_at: now,
            tags: new.tags,
            client_token: new.client_token,
        };
        self.views.insert(entry.view_arn.clone(), entry.clone());
        // What was stored, so the caller reports the stored values and nothing else.
        entry
    }

    /// The Region's index when it can back view creation and search — that is,
    /// when it exists and is neither being torn down nor already deleted.
    pub fn live_index(&self, region: &str) -> Option<&IndexEntry> {
        self.indexes.get(region).filter(|index| {
            !matches!(
                index.state,
                IndexLifecycle::Deleting | IndexLifecycle::Deleted
            )
        })
    }

    /// The index a view created in `region` is queried through: the one the Region
    /// holds, else the one the account's own setup already gave a Region no call has
    /// ever acted on.
    ///
    /// Turning Resource Explorer on in a Region is ACCOUNT setup, performed outside the
    /// operations modelled here (the console's one-click setup, an organization-wide
    /// rollout), so a client reaching this emulator may well have it on in a Region
    /// nothing created an index in through this API. A Region with no index record at
    /// all is therefore a Region nobody has acted on, and it is served as one that is
    /// already turned on: its index is materialised here, together with the default view
    /// [`Self::insert_index`] gives every Region that is turned on, and it is
    /// materialised ACTIVE because setup the account already did is not work that starts
    /// now — nothing is owed on the clock for it.
    ///
    /// A Region whose index record is PRESENT is a Region this account has acted on,
    /// including the terminal record a finished deletion leaves behind, so it keeps
    /// exactly the state those calls left it in: a Region that was turned off stays off,
    /// and answers `None` like one whose index is still being torn down. A token that is
    /// no Region this emulator serves can key no index either, and answers `None` too.
    pub fn index_for_new_view(&mut self, region: &str, now: SystemTime) -> Option<&IndexEntry> {
        if !is_servable_region(region) {
            return None;
        }
        if !self.indexes.contains_key(region) {
            self.insert_index(region, INDEX_TYPE_LOCAL, HashMap::new(), None, now);
            if let Some(index) = self.indexes.get_mut(region) {
                index.state = IndexLifecycle::Active;
                index.settles_at = None;
            }
        }
        self.live_index(region)
    }

    /// The index record that still OCCUPIES `region`, in whatever state.
    ///
    /// The one place "this Region already has an index" is decided, so a Region whose
    /// index was deleted can be given a new one while a Region whose index is merely
    /// being torn down cannot.
    pub fn present_index(&self, region: &str) -> Option<&IndexEntry> {
        self.indexes
            .get(region)
            .filter(|index| index.state.occupies_region())
    }

    /// The account's aggregator index, when it holds one that is still there.
    ///
    /// There is at most one: "You can have only one aggregator index per Amazon Web
    /// Services account." An index on its way out is not it, and neither is the record of
    /// one whose deletion finished, for the same reason [`Self::live_index`] leaves both
    /// out — it has stopped replicating anything.
    pub fn aggregator_index(&self) -> Option<&IndexEntry> {
        self.indexes.values().find(|index| {
            !matches!(
                index.state,
                IndexLifecycle::Deleting | IndexLifecycle::Deleted
            ) && index.index_type == INDEX_TYPE_AGGREGATOR
        })
    }

    /// The instant until which the account may not promote the index `index_arn` names
    /// to `AGGREGATOR`, when the documented wait after the last demotion is still
    /// running at `now`.
    ///
    /// The wait blocks promoting "another index" than the one that was demoted, so
    /// re-promoting the index that was itself demoted is not held back by it.
    pub fn aggregator_promotion_blocked_until(
        &self,
        index_arn: &str,
        now: SystemTime,
    ) -> Option<SystemTime> {
        let demotion = self.aggregator_demotion.as_ref()?;
        if demotion.index_arn == index_arn {
            return None;
        }
        let until = demotion.at + AGGREGATOR_PROMOTION_WAIT;
        (now < until).then_some(until)
    }

    /// The Region's view with this name, if any. View names are unique per Region;
    /// the comparison is exact, because the model's `ViewName` pattern admits mixed
    /// case and AWS documents only that "the name must be unique within its Amazon Web
    /// Services Region".
    pub fn view_by_name(&self, region: &str, name: &str) -> Option<&ViewEntry> {
        self.views
            .values()
            .find(|view| view.region == region && view.view_name == name)
    }

    /// The Region's views, ordered by ARN so paging over them is stable.
    pub fn views_in_region(&self, region: &str) -> Vec<&ViewEntry> {
        let mut views: Vec<&ViewEntry> = self
            .views
            .values()
            .filter(|view| view.region == region)
            .collect();
        views.sort_by(|a, b| a.view_arn.cmp(&b.view_arn));
        views
    }

    /// The stored tag map of the index or view `arn` names, when this account holds one.
    ///
    /// The ONE place a tagging operation resolves a `resourceArn`, so the three
    /// operations bound to `/tags/{resourceArn}` cannot disagree about which resources
    /// are taggable or where a resource's tags live: an index's and a view's tags are
    /// held ON the index and the view, which is why `GetIndex`, `GetView` and `Search`
    /// report a tag write without knowing anything about the operation that made it.
    pub fn tags_of_resource(&self, arn: &str) -> Option<&HashMap<String, String>> {
        if let Some(view) = self.views.get(arn) {
            return Some(&view.tags);
        }
        self.indexes
            .values()
            .find(|index| index.arn == arn && index.state.occupies_region())
            .map(|index| &index.tags)
    }

    /// The same resolution, for the operations that WRITE a resource's tags.
    pub fn tags_of_resource_mut(&mut self, arn: &str) -> Option<&mut HashMap<String, String>> {
        if let Some(view) = self.views.get_mut(arn) {
            return Some(&mut view.tags);
        }
        self.indexes
            .values_mut()
            .find(|index| index.arn == arn && index.state.occupies_region())
            .map(|index| &mut index.tags)
    }

    /// The indexes a `ListIndexes` caller can see, ordered by ARN.
    ///
    /// An index that is being torn down, or whose deletion already finished, is left
    /// out: `Index` carries no state member, so listing one would report a usable index
    /// that is not.
    pub fn listable_indexes(&self) -> Vec<&IndexEntry> {
        let mut indexes: Vec<&IndexEntry> = self
            .indexes
            .values()
            .filter(|index| {
                !matches!(
                    index.state,
                    IndexLifecycle::Deleting | IndexLifecycle::Deleted
                )
            })
            .collect();
        indexes.sort_by(|a, b| a.arn.cmp(&b.arn));
        indexes
    }

    /// The view a query must use: the one the caller named, else the Region's default.
    ///
    /// `None` means no view resolved. For an ARN the request NAMED that is the
    /// authorization failure `ViewArn` documents — "if you don't have permission to use
    /// the default view, then the operation fails with a 401 Unauthorized exception" —
    /// rather than a report that the view is absent: authorization matches the ARN before
    /// the query runs. What a caller that named NO view gets when the Region has no
    /// default is the calling operation's own business, so each one decides it.
    ///
    /// A named ARN resolves wherever the request was signed: a view ARN carries its own
    /// Region, and the index the query then reads is that view's. `region` is used only
    /// for the default view, which is per-Region by definition.
    pub fn query_view(&self, region: &str, view_arn: Option<&str>) -> Option<&ViewEntry> {
        match view_arn {
            Some(arn) => self.views.get(arn),
            None => self
                .default_view_arns
                .get(region)
                .and_then(|arn| self.views.get(arn)),
        }
    }

    /// The contents of the Region's index: the resources this service owns there and
    /// the `owned` ones the services in [`owners`] hold there, each described from
    /// persisted state and ordered by ARN.
    ///
    /// A Region with no index holds nothing, which is what turning Resource
    /// Explorer off in a Region means — so a resource another service owns is indexed
    /// only while this Region's index is there to hold it. `owned` is read from those
    /// services BEFORE this store is locked, so no two stores are ever locked in two
    /// orders; rows of another Region are dropped here, because an index holds only
    /// "the resources that are located in the same Region as the index".
    pub fn indexed_resources(
        &self,
        region: &str,
        owned: &[IndexedResource],
    ) -> Vec<IndexedResource> {
        let Some(index) = self.live_index(region) else {
            return Vec::new();
        };
        let account = ::engine::emulator_account_id().to_string();
        let mut resources = vec![IndexedResource {
            arn: index.arn.clone(),
            owning_account_id: account.clone(),
            region: index.region.clone(),
            service: SERVICE_NAMESPACE.to_string(),
            // This service's own two types, read from the one set of rows
            // (`owners::INDEXED_TYPES`) a Region's index is built from, so a type a query
            // can find is a type the published catalogue names.
            resource_type: owners::OWN_INDEX.resource_type.to_string(),
            cfn_resource_type: Some(owners::OWN_INDEX.cfn_resource_type.to_string()),
            last_reported_at: index.last_updated_at,
            tags: index.tags.clone(),
        }];
        for view in self.views_in_region(region) {
            resources.push(IndexedResource {
                arn: view.view_arn.clone(),
                owning_account_id: account.clone(),
                region: view.region.clone(),
                service: SERVICE_NAMESPACE.to_string(),
                resource_type: owners::OWN_VIEW.resource_type.to_string(),
                cfn_resource_type: Some(owners::OWN_VIEW.cfn_resource_type.to_string()),
                last_reported_at: view.last_updated_at,
                tags: view.tags.clone(),
            });
        }
        resources.extend(
            owned
                .iter()
                .filter(|resource| resource.region == region)
                .cloned(),
        );
        resources.sort_by(|a, b| a.arn.cmp(&b.arn));
        resources
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsResourceexplorer2ResourceexplorerState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsResourceexplorer2ResourceexplorerState>)
}

// ---------------------------------------------------------------------------
// Request metadata
// ---------------------------------------------------------------------------

/// The Region this request is addressed to: the Region of its SigV4 credential
/// scope, falling back to [`DEFAULT_REGION`] for an unsigned request.
///
/// Resource Explorer is a regional service whose index, views and default view
/// all belong to "the Region in which you call this operation", so every handler
/// resolves the Region the same way.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .filter(|region| !region.is_empty())
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The resource-id portion of an ARN: whatever follows the last `/` or `:`.
pub fn resource_id_of_arn(arn: &str) -> &str {
    arn.rsplit(['/', ':']).next().unwrap_or(arn)
}

/// The Region whose index a query is going to read, as far as the REQUEST says.
///
/// A query that names a view reads the index of THAT view's Region — a view ARN carries
/// its own Region, and a view only ever reports its own Region's resources — while one
/// that names none uses the Region it was signed for, which is where its default view
/// lives. Resolving it from the request alone is what lets a handler read the owning
/// services ([`owners`]) before it locks this service's store; the authoritative Region
/// is still the resolved view's own, and [`ComAmazonawsResourceexplorer2ResourceexplorerState::indexed_resources`]
/// drops any row that does not belong to it.
pub fn queried_region(parts: &::http::request::Parts, view_arn: Option<&str>) -> String {
    view_arn
        .and_then(::service_common::arn::parse)
        .map(|arn| arn.region.to_string())
        .filter(|region| !region.is_empty())
        .unwrap_or_else(|| request_region(parts))
}

// ---------------------------------------------------------------------------
// Resource ARNs
// ---------------------------------------------------------------------------

/// Whether `arn` names a resource in THIS service's ARN namespace.
///
/// Resource Explorer owns only the `index/*` and `view/*` resources of its own
/// [`SERVICE_NAMESPACE`], so an ARN naming another service's resource — or a value that
/// is not an ARN, which names nothing — is outside what its operations can act or
/// authorize on. The ARN itself is read by the workspace's one resource-ARN parse
/// (`service_common::arn`), so this service cannot disagree with another about what an
/// ARN is or which vendor field says whose resource it names.
pub fn is_own_arn(arn: &str) -> bool {
    ::service_common::arn::is_resource_arn_of(arn, SERVICE_NAMESPACE)
}

/// How an `UnauthorizedException` from this service is worded: the refusal says only
/// that the principal is not permitted to perform the operation, so it carries nothing
/// from the request.
pub const UNAUTHORIZED_MESSAGE: &str = "Unauthorized";

/// How a `ValidationException` from this service reads when the request carries no
/// member the refusal could name: the shape's OWN documented wording
/// (`smithy_query(query="doc",shape="ValidationException")`), which states the class
/// and nothing from the request or the store.
pub const VALIDATION_MESSAGE: &str = "You provided an invalid value for one of the \
                                      operation's parameters. Check the syntax for the \
                                      operation, and try again.";

/// The `ConflictException` message for the view-conflict class, the one the shape's own
/// documentation describes: "you specified parameters that didn't match the original
/// request, or you attempted to create a view with a name that already exists in this
/// Amazon Web Services Region".
///
/// WHOLLY FIXED text: the refusal names neither the view, nor the Region, nor the ARN of
/// the view that is already there — which is also why it cannot disclose a view the
/// caller was not able to name correctly.
pub const VIEW_CONFLICT_MESSAGE: &str = "The request failed because either the request \
                                         parameters didn't match to the original request \
                                         or a view with the specified name already exists \
                                         in this AWS Region.";

/// The `ViewName` `@pattern` the model declares
/// (`smithy_query(query="shape",shape="ViewName")`).
///
/// Re-declared here because `CreateViewInput` carries `skip_validation = ["ViewName"]`:
/// the boundary walk must not refuse a name that cannot produce a matchable view ARN,
/// because that refusal belongs to the authorization class, which runs first and is
/// worded differently.
pub const VIEW_NAME_PATTERN: &str = r"^[a-zA-Z0-9\-]{1,64}$";

/// Whether `name` is a `ViewName` the modelled pattern admits.
///
/// Matched with the maintained `regex` engine against [`VIEW_NAME_PATTERN`] itself, so
/// the rule enforced is the model's and not a re-statement of it. The expression is
/// anchored, so a find is an anchored match; it is compiled once per process.
pub fn is_well_formed_view_name(name: &str) -> bool {
    static COMPILED: OnceLock<Regex> = OnceLock::new();
    COMPILED
        .get_or_init(|| {
            Regex::new(VIEW_NAME_PATTERN).expect("the modelled ViewName pattern compiles")
        })
        .is_match(name)
}

/// The FIXED text of the refusal that reports no view exists for the ARN a request
/// named: AWS states the class and then the ARN it was given, which is the only
/// variable part. Held in one place so every operation that resolves a `ViewArn`
/// against the store words that fact identically.
pub const VIEW_NOT_FOUND_PREFIX: &str = "Views Not Found for provided arn: ";

/// How this service words "the ARN you gave names no view I hold": the fixed text of
/// [`VIEW_NOT_FOUND_PREFIX`] followed by the ARN the REQUEST carried, verbatim — not a
/// value re-rendered from the store, which a lookup miss has none of.
pub fn view_not_found_message(view_arn: &str) -> String {
    format!("{VIEW_NOT_FOUND_PREFIX}{view_arn}")
}

/// The `ResourceNotFoundException` message for an own-namespace ARN that this account
/// holds no resource for, worded with the kind of resource the ARN names — the two kinds
/// this service owns are `index/<id>` and `view/<name>/<id>`.
///
/// A view ARN is worded by [`view_not_found_message`], so an operation that resolves
/// either kind of ARN says the same thing about a missing view as one that resolves only
/// views.
pub fn resource_not_found_message(arn: &str) -> String {
    match ::service_common::arn::resource_type(arn) {
        Some("view") => view_not_found_message(arn),
        Some("index") => format!("Index not found: {arn}"),
        _ => format!("Resource not found: {arn}"),
    }
}

// ---------------------------------------------------------------------------
// Pagination
// ---------------------------------------------------------------------------

/// A short, stable digest of the values a paginated request is bound to.
///
/// Folded into the page token's `kind` so a `NextToken` issued for one request cannot
/// resume a DIFFERENT one at that offset: a token whose fingerprint does not match the
/// request is refused, rather than answered with a slice of an unrelated result set.
pub fn request_fingerprint(parts: &[&str]) -> String {
    let mut hasher = Sha256::new();
    for part in parts {
        // Length-prefixed, so no two different part lists hash the same bytes.
        hasher.update((part.len() as u64).to_be_bytes());
        hasher.update(part.as_bytes());
    }
    ::hex::encode(&hasher.finalize()[..8])
}

/// Build the opaque `NextToken` that resumes `kind`'s listing at `offset`.
///
/// The offset travels inside the token rather than in a server-side cursor table,
/// so a `@readonly` paginated operation stores nothing and a token stays valid
/// for the life of the emulator.
pub fn encode_page_token(kind: &str, offset: usize) -> String {
    ::hex::encode(format!("{kind}:{offset}"))
}

/// The offset a `NextToken` issued by [`encode_page_token`] for `kind` resumes
/// at, or `None` when the token was not issued for this listing.
pub fn decode_page_token(kind: &str, token: &str) -> Option<usize> {
    let decoded = ::hex::decode(token).ok()?;
    let text = String::from_utf8(decoded).ok()?;
    text.strip_prefix(&format!("{kind}:"))?.parse().ok()
}

/// One page of `items`, plus the token that resumes after it.
///
/// `max_results` absent means "no page limit from the caller", so the whole
/// remainder is returned and no token is issued. An unparseable or out-of-range
/// token is an error, with the message naming the member — the caller has to fix
/// the request from that text.
pub fn paginate<T: Clone>(
    items: &[T],
    kind: &str,
    next_token: Option<&str>,
    max_results: Option<i32>,
) -> Result<(Vec<T>, Option<String>), String> {
    let offset = match next_token {
        Some(token) => decode_page_token(kind, token).ok_or_else(|| {
            "NextToken is not a token this operation issued; omit it to start from \
             the first page"
                .to_string()
        })?,
        None => 0,
    };
    if offset > items.len() {
        return Err(
            "NextToken refers to a page that no longer exists; omit it to start from \
             the first page"
                .to_string(),
        );
    }
    let limit = match max_results {
        Some(max) if max > 0 => max as usize,
        // A non-positive MaxResults is refused by the modelled range before a
        // handler runs, so only "absent" reaches here.
        _ => items.len().saturating_sub(offset),
    };
    let end = offset.saturating_add(limit).min(items.len());
    let page = items[offset..end].to_vec();
    let next = (end < items.len()).then(|| encode_page_token(kind, end));
    Ok((page, next))
}

// ---------------------------------------------------------------------------
// Search query syntax
// ---------------------------------------------------------------------------

/// Whether `resource` matches a Resource Explorer search query.
///
/// The grammar is the one documented in the "Search query syntax reference for
/// Resource Explorer" that `SearchInput$QueryString` and
/// `SearchFilter$FilterString` both point at:
///
/// * a query is whitespace-separated TERMS, all of which must match (logical AND);
/// * a term prefixed with `-` must NOT match;
/// * `name:value` filters one field, for the filter names [`filter_fields`] lists;
/// * any other term is free-form text, matched against every indexed field;
/// * a value ending in `*` matches by prefix;
/// * "the search is completely case insensitive";
/// * an empty query matches everything.
///
/// The syntax is proprietary to this service — no published crate parses it — so
/// it is implemented here, from the grammar rather than from any one query.
pub fn query_matches(query: &str, resource: &IndexedResource) -> bool {
    query
        .split_whitespace()
        .all(|term| term_matches(term, resource))
}

/// One term of a query, split the one way the grammar splits it: whether it is negated,
/// and its filter `name` and `value` when it carries a filter prefix at all.
///
/// The split on the FIRST colon only is what lets a filter value carry colons of its own
/// (`resourcetype:ec2:instance`).
fn split_term(term: &str) -> (bool, &str, Option<(&str, &str)>) {
    let (negated, body) = match term.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, term),
    };
    (negated, body, body.split_once(':'))
}

/// The filter names that select on a resource's TAGS: the `tag`, `tag.key` and
/// `tag.value` filters of the search query syntax reference.
///
/// The ONE place those names are listed. [`filter_fields`] reaches them only through
/// [`is_tag_filter_name`], so the names it resolves and the names a tag filter is
/// recognised by cannot drift.
const TAG_FILTER_NAMES: [&str; 3] = ["tag", "tag.key", "tag.value"];

/// Whether a filter name selects on a resource's TAGS, compared case-insensitively like
/// every other Resource Explorer comparison and without allocating: this runs once per
/// term of every query.
fn is_tag_filter_name(name: &str) -> bool {
    TAG_FILTER_NAMES
        .iter()
        .any(|filter| filter.eq_ignore_ascii_case(name))
}

/// The name of the first term in `query` that filters on a resource's TAGS, written the
/// way the query wrote it, or `None` when no term does.
///
/// A tag filter reads a resource's tags, which a view only carries when it includes the
/// `tags` property ([`TAGS_PROPERTY`]) — so this is what tells a caller's query, or a
/// view's own filter string, apart from one a view without that property can answer.
pub fn tags_filter_name(query: &str) -> Option<&str> {
    query.split_whitespace().find_map(|term| {
        let (_, _, filter) = split_term(term);
        let (name, _) = filter?;
        is_tag_filter_name(name).then_some(name)
    })
}

/// Whether `name` is one of the filter prefixes the search query syntax documents.
///
/// Asked of a NAME alone, so it answers the question `ListResources` has to ask of its
/// own filter string, whose documented restriction is that it "supports only filter
/// prefixes with optional operators. It doesn't support free-form text."
///
/// Answered through [`filter_fields`] itself — the ONE place the filter names live —
/// against a resource record carrying no values, because which names are filter names
/// is a property of the name and not of any resource.
pub fn is_filter_name(name: &str) -> bool {
    filter_fields(name, valueless_resource()).is_some()
}

/// A resource record carrying no values, for asking [`filter_fields`] about a filter
/// name on its own. Built once per process: this is asked once per term of a
/// caller-supplied filter string.
fn valueless_resource() -> &'static IndexedResource {
    static PROBE: OnceLock<IndexedResource> = OnceLock::new();
    PROBE.get_or_init(|| IndexedResource {
        arn: String::new(),
        owning_account_id: String::new(),
        region: String::new(),
        service: String::new(),
        resource_type: String::new(),
        cfn_resource_type: None,
        last_reported_at: SystemTime::UNIX_EPOCH,
        tags: HashMap::new(),
    })
}

/// The first term of `query` that is NOT a complete supported filter operator, written
/// the way the query wrote it, or `None` when every term is one.
///
/// A term is a complete filter operator when it is a documented filter name
/// ([`is_filter_name`]) followed by `:` and a NON-EMPTY value — the `name:value` pair the
/// search query syntax reference spells out, optionally negated with `-`. Every other
/// term is free-form text: one with no `name:value` prefix at all, one whose prefix is
/// not a filter name (exactly the terms [`term_matches`] falls back to matching as text),
/// and one carrying a filter prefix with nothing after the colon, which is a prefix and
/// not a pair — it selects no value of the field it names. An empty term contributes
/// nothing either way, so it is not free-form text.
pub fn free_form_term(query: &str) -> Option<&str> {
    query.split_whitespace().find(|term| {
        let (_, body, filter) = split_term(term);
        !body.is_empty()
            && match filter {
                Some((name, value)) => !is_filter_name(name) || value.is_empty(),
                None => true,
            }
    })
}

/// The candidate values of the field a documented filter name selects, or `None`
/// when the name is not a filter name (so the term is free-form text).
///
/// The names are those of the search query syntax reference: `arn`, `id`,
/// `region`, `service`, `resourcetype`, `globalresourcetype`, `accountid`, `tag`,
/// `tag.key` and `tag.value`.
fn filter_fields(name: &str, resource: &IndexedResource) -> Option<Vec<String>> {
    if is_tag_filter_name(name) {
        return Some(if name.eq_ignore_ascii_case("tag.key") {
            resource.tags.keys().cloned().collect()
        } else if name.eq_ignore_ascii_case("tag.value") {
            resource.tags.values().cloned().collect()
        } else {
            // The `tag` filter matches the whole `key=value` pair.
            resource
                .tags
                .iter()
                .map(|(key, value)| format!("{key}={value}"))
                .collect()
        });
    }
    let fields = match name.to_ascii_lowercase().as_str() {
        "arn" => vec![resource.arn.clone()],
        "id" => vec![resource_id_of_arn(&resource.arn).to_string()],
        "region" => vec![resource.region.clone()],
        "service" => vec![resource.service.clone()],
        "resourcetype" => vec![resource.resource_type.clone()],
        "accountid" => vec![resource.owning_account_id.clone()],
        // A global resource has no Region in its ARN; a regional one can never
        // match a globalresourcetype filter.
        "globalresourcetype" => {
            if resource.region.is_empty() {
                vec![resource.resource_type.clone()]
            } else {
                Vec::new()
            }
        }
        _ => return None,
    };
    Some(fields)
}

/// Whether one term of a query holds for `resource`.
fn term_matches(term: &str, resource: &IndexedResource) -> bool {
    let (negated, body, filter) = split_term(term);
    if body.is_empty() {
        return true;
    }
    let hit = match filter {
        Some((name, value)) => match filter_fields(name, resource) {
            Some(fields) => fields.iter().any(|field| value_matches(value, field)),
            // An unrecognized prefix is not a filter, so the whole term is text.
            None => free_text_matches(body, resource),
        },
        None => free_text_matches(body, resource),
    };
    hit != negated
}

/// Whether a filter value matches one field value: case-insensitive equality, or
/// a prefix match when the value ends in the `*` wildcard.
fn value_matches(value: &str, field: &str) -> bool {
    match value.strip_suffix('*') {
        Some(prefix) => field
            .to_ascii_lowercase()
            .starts_with(&prefix.to_ascii_lowercase()),
        None => field.eq_ignore_ascii_case(value),
    }
}

/// Whether free-form text occurs in any indexed field of `resource`.
///
/// Deliberately an unanchored, case-insensitive SUBSTRING match, not the prefix match
/// [`value_matches`] applies to a filtered value: free-form text in a Resource Explorer
/// query is matched against the whole of each indexed field, so a trailing `*` adds
/// nothing and is stripped. The two rules differ on purpose; keep them that way.
fn free_text_matches(text: &str, resource: &IndexedResource) -> bool {
    let needle = text.trim_end_matches('*').to_ascii_lowercase();
    if needle.is_empty() {
        return true;
    }
    resource
        .searchable_fields()
        .iter()
        .any(|field| field.to_ascii_lowercase().contains(&needle))
}
