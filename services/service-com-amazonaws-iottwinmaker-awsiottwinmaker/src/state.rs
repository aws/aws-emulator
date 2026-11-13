// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-iottwinmaker-awsiottwinmaker service.
//!
//! The store is REGIONAL before it is anything else: IoT TwinMaker is a
//! regional service, so everything below lives inside one
//! [`RegionalState`] and the top-level
//! [`ComAmazonawsIottwinmakerAwsiottwinmakerState`] is just the map from a
//! Region to its store. A request is served out of the store of the Region its
//! own credential scope names ([`request_region`]), so a resource created
//! through one Region's endpoint is invisible through another's and the same id
//! may name an independent resource in each.
//!
//! Inside one Region, IoT TwinMaker's control plane is a two-level tree: a
//! *workspace* owns the
//! *entities*, *component types*, *scenes* and *sync jobs* created inside it.
//! That containment is modelled literally here — `Workspace::entities`,
//! `Workspace::component_types`, `Workspace::scenes` and
//! `Workspace::sync_jobs` — so none of them can outlive its workspace and two
//! operations cannot read two copies of one resource.
//!
//! Nothing derivable is stored: `hasChildEntities` is computed by scanning the
//! workspace's entities for the id as a parent, and a workspace's
//! `linkedServices` by reading the sources of the sync jobs it holds
//! (`Workspace::linked_services`). A resource's `state`, by
//! contrast, IS stored. An entity's create settles it in `ACTIVE` within the
//! call that asked for it, so a created entity has no provisioning instant for
//! a read to compute a transient state from; an entity's UPDATE does open such
//! a window, so the record carries the instant the state it was left in falls
//! due (`Entity::settles_at`), which a reader reports through rather than
//! sleeps for. A component type's create answers the
//! non-terminal `CREATING`, so its record also carries the instant its
//! transition to `ACTIVE` falls due (`ComponentType::settles_at`), which a
//! reader materialises rather than sleeps for.
//!
//! Not everything is inside that tree: a *metadata transfer job* is addressed
//! with no workspace in its path and may name a different workspace at each
//! end of the transfer, so the jobs live beside the workspaces
//! (`metadata_transfer_jobs`). A job is accepted in the non-terminal
//! `VALIDATING` state and carries the instant its outcome falls due
//! (`MetadataTransferJob::settles_at`), the same way a component type carries
//! its own — and so does a *sync job* (`SyncJob::settles_at`), which
//! `CreateSyncJob` accepts in the non-terminal `CREATING` state. A sync job
//! also OWNS the resources its synchronisation has acted on
//! (`SyncJob::sync_resources`), keyed by their identity in the sync source, so
//! they cannot outlive the job any more than the job can outlive its
//! workspace — see [`SyncResource`] for what writes one.
//!
//! The account's *pricing plan* is the same shape of thing: a plan change the
//! service bills from the start of the next cycle is held beside the plan in
//! force (`RegionalState::pending_pricing_plan`) carrying the instant it falls
//! due, and a reader materialises it rather than sleeps for it
//! (`RegionalState::settle_pricing_plan`) — which is also where the tier a
//! bundled plan is billed at follows the account's billable entity count across
//! a band boundary.

use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::time::{Duration, SystemTime};
/// How a resource is NAMED and what a refusal that could not resolve one says.
mod identity;
/// The opaque pagination tokens this service's listings mint and read.
mod page_tokens;
/// The one projection of a stored property every reader of one reports through.
mod property_projection;
mod workspace_rules;
/// What a metadata transfer job READS, WRITES and REPORTS: the document its
/// source describes and the run that lands it in its destination.
mod metadata_transfer;
pub use identity::*;
pub use metadata_transfer::*;
pub use page_tokens::*;
pub use property_projection::*;

/// The `ParentEntityId` sentinel the service uses for an entity with no parent.
/// Taken from the `ParentEntityId` pattern, which admits the literal `$ROOT`
/// beside a uuid and a plain id.
pub const ROOT_PARENT_ENTITY_ID: &str = "$ROOT";

/// The `State` enum values this slice produces. The full value list is
/// `STATE_VALUES` in `ops::types` (`CREATING`, `UPDATING`, `DELETING`, `ACTIVE`,
/// `ERROR`).
///
/// `ACTIVE` is the state an entity SETTLES in, and the one a stored entity
/// carries: a create completes within the call that asked for it, so no entity
/// is ever persisted in the non-terminal `CREATING` state — recorded traffic
/// reads a just-created entity back as `ACTIVE`, with no step anywhere in the
/// corpus observing it `CREATING`. `UPDATING` is what an update in flight
/// leaves an entity in ([`Entity::begin_update`]) until the clock settles it
/// back in `ACTIVE`; `ERROR` belongs to a failed transition this emulator never
/// produces, and `DELETING` is what `DeleteEntity` reports for the removal it
/// performs.
pub const STATE_ACTIVE: &str = "ACTIVE";
/// See [`STATE_ACTIVE`].
pub const STATE_DELETING: &str = "DELETING";
/// See [`STATE_ACTIVE`]. A COMPONENT TYPE, unlike an entity, is reported in
/// this non-terminal state by the call that creates it and settles in
/// [`STATE_ACTIVE`] once [`COMPONENT_TYPE_SETTLE_SPAN`] has elapsed.
pub const STATE_CREATING: &str = "CREATING";
/// See [`STATE_ACTIVE`]. The non-terminal state an UPDATE in flight leaves a
/// resource in: `UpdateComponentType` and `UpdateEntity` answer with it, and
/// the clock settles the resource back in [`STATE_ACTIVE`] once the matching
/// span ([`COMPONENT_TYPE_SETTLE_SPAN`], [`ENTITY_SETTLE_SPAN`]) has elapsed —
/// for a component type, exactly as it does after a create.
pub const STATE_UPDATING: &str = "UPDATING";

/// How long a component type stays in a non-terminal state — [`STATE_CREATING`]
/// after a create, [`STATE_UPDATING`] after an update — before the clock makes
/// its transition to [`STATE_ACTIVE`] due.
///
/// `CreateComponentType` answers with the component type's `state`, and the
/// state of a component type the service has just accepted is the non-terminal
/// `CREATING` — so there IS a provisioning window here, unlike an entity, which
/// is reported `ACTIVE` by its own create. `UpdateComponentType` opens the same
/// window with `UPDATING`: the call reports the change as in flight and the
/// component type is `ACTIVE` again once it has landed. AWS documents no
/// duration for either window, so this span is CHOSEN by this emulator; it is
/// deliberately non-zero, so a caller that reads the component type back in the
/// same breath as the create or the update still observes the transient state,
/// and deliberately short, so a client that polls reaches `ACTIVE` without a
/// long wait. Nothing sleeps for it: the instant it is due is stored on the
/// record (see [`ComponentType::settles_at`]) and a reader materialises the
/// transition the clock already owed.
pub const COMPONENT_TYPE_SETTLE_SPAN: Duration = Duration::from_millis(500);

/// How long an entity stays in [`STATE_UPDATING`] after an update before the
/// clock makes its transition back to [`STATE_ACTIVE`] due.
///
/// `UpdateEntity` answers with "the current state of the entity update", and the
/// state of an update the service has just accepted is the non-terminal
/// `UPDATING`: the change is reported as in flight and the entity is `ACTIVE`
/// again once it has landed. (A CREATE opens no such window — an entity's
/// create settles it in [`STATE_ACTIVE`] inside the call that asked for it; see
/// that constant.) AWS documents no duration for this window, so this span is
/// CHOSEN by this emulator; it is deliberately non-zero, so a caller that reads
/// the entity back in the same breath as the update still observes the
/// transient state, and deliberately short, so a client that polls reaches
/// `ACTIVE` without a long wait. Nothing sleeps for it: the instant it is due is
/// stored on the record (see [`Entity::settles_at`]) and every read reports the
/// state the clock has already made true.
pub const ENTITY_SETTLE_SPAN: Duration = Duration::from_millis(500);

/// The name of the function whose presence makes a component type's schema
/// initialized.
///
/// `isSchemaInitialized` is documented as "whether the component type has a
/// schema initializer and that the schema initializer has run": the schema
/// initializer is the entry of a component type's `functions` map under this
/// name, so a component type whose `functions` carry none has no schema to
/// initialize. See [`ComponentType::is_schema_initialized`].
pub const SCHEMA_INITIALIZER_FUNCTION: &str = "schemaInitializer";

/// The Region a request is served in when it carries no SigV4 credential scope
/// to read one from: the Region its ARNs are built in, and the Region whose
/// store ([`RegionalState`]) answers it.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The most Regions this store holds resources for at once, counted per Region
/// and not per resource: past it a create that would give a further Region its
/// FIRST resource is refused as a full store is, and nothing is written.
///
/// The Region of a request is its own SigV4 credential scope, which the caller
/// chooses, so without a ceiling a caller could repeat a create under fresh
/// Region names until the process ran out of memory, each name carrying its own
/// [`RegionalState`] and its own per-Region room for resources. A partition's
/// Regions are a fixed, published set; AWS publishes no limit of this kind, so
/// the figure is this emulator's own, set well above the largest partition.
pub const MAX_REGIONS: usize = 64;

/// The message a create refused for want of room for a further Region carries:
/// it names the ceiling that was reached, the way the per-Region workspace
/// quota's refusal names its own.
pub fn region_ceiling_message() -> String {
    format!("the limit of {MAX_REGIONS} regions has been reached")
}

/// The largest page any list operation of this service serves, applied
/// SERVER-SIDE so a request cannot ask for an unbounded one, and the page size
/// served when a request names none.
///
/// Both come from the `maxResults` member's own documentation: "The maximum
/// number of results to return at one time. The default is 25." The ceiling is
/// the top of the member's modelled range (0..200), applied by the service
/// whatever the caller asks for, so the page is bounded by this code and not
/// only by the boundary validator.
pub const MAX_PAGE_SIZE: usize = 200;
/// See [`MAX_PAGE_SIZE`].
pub const DEFAULT_PAGE_SIZE: usize = 25;

/// What ONE paginated operation's `maxResults` member documents, and so the two
/// parameters the shared page-size rule ([`page_size`]) is applied with.
///
/// The rule is the same for every paginated operation of this service; only
/// these two numbers differ between them, so they are parameters rather than
/// part of the rule: `ListSyncJobs` documents a default of 50 where the other
/// listings document [`DEFAULT_PAGE_SIZE`], and a member documenting a minimum
/// of 1 refuses the zero a member documenting a minimum of 0 serves as a page of
/// no rows. A rule is built with [`PageSizeRule::minimum_one`] or
/// [`PageSizeRule::minimum_zero`], so its minimum is never negative.
#[derive(Debug, Clone, Copy)]
pub struct PageSizeRule {
    /// The page size served for a request that names no `maxResults`, from the
    /// member's documented default.
    default: usize,
    /// The smallest `maxResults` the member's documentation admits.
    minimum: i32,
}

impl PageSizeRule {
    /// The rule for a `maxResults` documenting a minimum of 1 — an explicit zero
    /// is refused — served `default` when a request names none.
    pub const fn minimum_one(default: usize) -> Self {
        Self {
            default,
            minimum: 1,
        }
    }

    /// The rule for a `maxResults` documenting a minimum of 0 — a zero is the
    /// maximum number of results to return at one time, literally, so it is
    /// served as a page of no rows rather than refused — served `default` when a
    /// request names none.
    pub const fn minimum_zero(default: usize) -> Self {
        Self {
            default,
            minimum: 0,
        }
    }

    /// The refusal message for a `maxResults` of `requested`, which this rule's
    /// minimum does not admit.
    ///
    /// A member documenting a POSITIVE minimum is asked for a smaller value by a
    /// caller that paged at zero, so the message names the minimum and the
    /// default the member can be omitted for. A member documenting a minimum of
    /// 0 admits every value its published range carries, so a value below it
    /// arrives only from a caller that got past that range, and the message names
    /// the range and the value instead.
    fn refusal(self, requested: i32) -> String {
        if self.minimum > 0 {
            format!(
                "maxResults must be at least {}; omit it for the default of {}",
                self.minimum, self.default
            )
        } else {
            format!(
                "maxResults must be between {} and {MAX_PAGE_SIZE}, got {requested}",
                self.minimum
            )
        }
    }
}

/// The page size a paginated operation serves for a request's `maxResults` under
/// `rule`, or the message refusing a value the rule does not admit.
///
/// ONE rule, shared by every paginated operation of this service so the page a
/// caller is served means the same thing wherever it is read: a request naming
/// no `maxResults` is served the operation's documented default — never every
/// row it could report — a value below the member's documented minimum is
/// refused, and anything else is served as asked for, capped at
/// [`MAX_PAGE_SIZE`], the most this service ever serves. The page a handler
/// BUILDS is therefore bounded by this code and not only by the boundary
/// validator.
///
/// The refusal is a message rather than an error: every operation models its own
/// `ValidationException` (HTTP 400), so the caller wraps the message in its own.
pub fn page_size(max_results: Option<i32>, rule: PageSizeRule) -> Result<usize, String> {
    match max_results {
        None => Ok(rule.default),
        Some(requested) if requested < rule.minimum => Err(rule.refusal(requested)),
        // A value below the rule's minimum is already refused above and no rule
        // has a negative minimum, so the ceiling is applied in the type the
        // member arrived in: there is no conversion fallback left for two copies
        // of this rule to disagree about. The CEILING is what decides the answer
        // here; the lower bound is unreachable belt-and-braces, kept only so a
        // value that somehow got past the arm above cannot become an enormous
        // page.
        Some(requested) => Ok(requested.clamp(0, MAX_PAGE_SIZE_I32) as usize),
    }
}

/// [`MAX_PAGE_SIZE`] as the modelled `maxResults` carries it, so a requested page
/// size is capped while it is still the integer the request named.
const MAX_PAGE_SIZE_I32: i32 = MAX_PAGE_SIZE as i32;

/// The most components `GetEntity` reports for one entity, and the most
/// properties it reports for one component.
///
/// Both are documented on the flags that report whether the response was
/// truncated: "The maximum number of components returned is 30"
/// (`areAllComponentsReturned`) and "The maximum number of properties returned
/// is 800" (`areAllPropertiesReturned`).
pub const MAX_COMPONENTS_RETURNED: usize = 30;
/// See [`MAX_COMPONENTS_RETURNED`].
pub const MAX_PROPERTIES_RETURNED: usize = 800;

/// The most values of ONE list- or map-typed property a read reports, from the
/// documentation of the flag that reports whether it had to truncate them
/// ("the maximum number of values per property returned is 50",
/// `areAllPropertyValuesReturned`).
///
/// It is also exactly the top of the modelled `length` of a `DataValue`'s
/// `listValue` and `mapValue` (0..50), so no value this service can store
/// carries more — see [`all_property_values_returned`], which is where every
/// reader of the flag decides it.
pub const MAX_PROPERTY_VALUES_RETURNED: usize = 50;

/// The most tags one taggable resource of this service carries: the service's
/// "Tags per resource" QUOTA, which is not adjustable.
///
/// A cumulative breach of it — a call whose tags, added to the ones the resource
/// already carries, would leave it past this many — is a quota refusal
/// ([`tags_per_resource_quota_exceeded`]), reported as
/// `ServiceQuotaExceededException` (HTTP 402). That is a different failure from
/// the per-CALL bound the `TagMap` shape states
/// ([`MAX_TAGS_PER_CALL`]).
pub const MAX_TAGS_PER_RESOURCE: usize = 50;

/// Whether a tagging call would leave a resource past the per-resource tag
/// quota, `resulting` being the size of the tag set it would be left holding.
///
/// The decision is ONE function, so no caller can refuse on one count and report
/// another. The refusal it drives carries an EMPTY message: the quota breach
/// names no value of the request.
pub fn tags_per_resource_quota_exceeded(resulting: usize) -> bool {
    resulting > MAX_TAGS_PER_RESOURCE
}

/// The most tags ONE tagging call carries, from the model rather than chosen
/// here: `TagMap` is `@length(min: 0, max: 50)`
/// (`smithy_query(query="shape",shape="TagMap")`), so a request naming more
/// entries than this is outside the shape itself and is refused by the
/// centralized request walk's boundary validation before any handler runs. It is
/// a DIFFERENT failure from the per-resource quota
/// [`MAX_TAGS_PER_RESOURCE`] states, which is about what the resource would be
/// left holding and is decided by the handler.
pub const MAX_TAGS_PER_CALL: usize = 50;

/// The tag-key prefix Amazon Web Services reserves for the tags it applies
/// itself.
///
/// AWS's tagging rules reserve this prefix across every tagging API: a caller
/// may neither create nor edit nor delete a tag whose key begins with it, so a
/// request naming one is refused and nothing it asked for is applied. The
/// reservation is of the NAMESPACE rather than of one spelling of it, so the
/// prefix is matched however it is cased (see [`is_reserved_tag_key`]).
pub const RESERVED_TAG_KEY_PREFIX: &str = "aws:";

/// Whether `key` lies in the namespace [`RESERVED_TAG_KEY_PREFIX`] reserves.
///
/// Only the prefix is reserved, so a key that merely CONTAINS it further along,
/// or one that starts with the letters without the colon, is an ordinary
/// caller-owned key. A key too short to carry the prefix cannot be in the
/// namespace, and neither can one whose first bytes are the middle of a
/// multi-byte character — `str::get` answers `None` for both.
pub fn is_reserved_tag_key(key: &str) -> bool {
    key.get(..RESERVED_TAG_KEY_PREFIX.len())
        .is_some_and(|head| head.eq_ignore_ascii_case(RESERVED_TAG_KEY_PREFIX))
}

/// The requirement `keys` fails when any of them lies in the namespace
/// [`RESERVED_TAG_KEY_PREFIX`] reserves, as the message of the refusal that
/// earns, or `None` when every key is the caller's own to write.
///
/// `member` is the member of the request that carries the keys (`tags`,
/// `tagKeys`), so the message is built from the request: it names the member at
/// fault, the keys that failed and the requirement they failed, the same
/// construction as this service's other validation messages. The offending keys
/// are reported in key order, so the refusal does not depend on the iteration
/// order of the map the request was deserialized into.
///
/// Decided on the keys ALONE, so every writer of a tag set — the tagging
/// operations and every create that takes a `tags` map — refuses the whole
/// request before touching the store: a request mixing a legal key with a
/// reserved one applies neither.
///
/// The reason names as many keys as the request carried, which the model bounds
/// one at a time (50 of them, of 128 characters each) and not as the set a
/// reason renders, so the message is brought within the bound of the member it
/// is reported on HERE ([`clamped_message`]) rather than at each writer that
/// reports it.
pub fn reserved_tag_keys_violation<'a, I>(member: &str, keys: I) -> Option<String>
where
    I: IntoIterator<Item = &'a str>,
{
    let mut reserved: Vec<&str> = keys
        .into_iter()
        .filter(|key| is_reserved_tag_key(key))
        .collect();
    if reserved.is_empty() {
        return None;
    }
    reserved.sort_unstable();
    reserved.dedup();
    Some(clamped_message(
        format!(
            "{member} names the tag {} {} in the {RESERVED_TAG_KEY_PREFIX} namespace, which is \
             reserved for the tags Amazon Web Services applies itself: such a key cannot be \
             created, changed or removed by a caller.",
            if reserved.len() == 1 { "key" } else { "keys" },
            reserved.join(", "),
        ),
        ERROR_MESSAGE_MAX_CHARS,
    ))
}

/// A map the REQUEST carried, in key order.
///
/// Every modelled request map deserializes into a
/// [`std::collections::HashMap`], whose iteration order is neither key order nor
/// the same from one process to the next. A rule that refuses a request by
/// walking one of its maps answers with the FIRST offender it meets, so walking
/// it in hash order would answer one identical request with different messages
/// on different runs. Collected into a [`BTreeMap`] here, so which entry a
/// refusal names — and the order in which entries are applied — is a function of
/// the request alone. Shared by every writer that walks a requested map, and an
/// absent map reads as an empty one so a caller has one less `Option` to guard.
pub fn requested_in_key_order<T>(
    requested: Option<std::collections::HashMap<String, T>>,
) -> BTreeMap<String, T> {
    requested.unwrap_or_default().into_iter().collect()
}

/// The most workspaces this emulator holds, the most entities one workspace
/// holds, and the most component types one workspace holds, before
/// `CreateWorkspace` / `CreateEntity` / `CreateComponentType` answer the
/// `ServiceQuotaExceededException` (HTTP 402) each operation models.
///
/// The real service's quotas are per-account and adjustable, so no fixed number
/// is AWS's; these are this emulator's own declared ceilings, so the in-memory
/// store cannot be grown without bound by a caller that keeps creating.
pub const MAX_WORKSPACES: usize = 50;
/// See [`MAX_WORKSPACES`].
pub const MAX_ENTITIES_PER_WORKSPACE: usize = 1000;
/// See [`MAX_WORKSPACES`].
pub const MAX_COMPONENT_TYPES_PER_WORKSPACE: usize = 1000;
/// See [`MAX_WORKSPACES`].
pub const MAX_SCENES_PER_WORKSPACE: usize = 1000;

/// The most components one ENTITY holds, the most composite components one of
/// those components holds, and the most properties and property groups one
/// component — top-level or composite — holds, before `CreateEntity` /
/// `UpdateEntity` answer the `ServiceQuotaExceededException` (HTTP 402) each
/// operation models. See [`entity_contents_over_ceiling`], the one rule both
/// writers apply.
///
/// The same reading as [`MAX_WORKSPACES`]: the real service's quotas are
/// per-account and adjustable, so no fixed number is AWS's; these are this
/// emulator's own declared ceilings, so the contents of one entity cannot be
/// grown without bound by a caller that keeps writing components or properties
/// into it. Each sits ABOVE what a read can report
/// ([`MAX_COMPONENTS_RETURNED`], [`MAX_PROPERTIES_RETURNED`]), so a write is
/// never refused merely for exceeding what one response carries.
pub const MAX_COMPONENTS_PER_ENTITY: usize = 100;
/// See [`MAX_COMPONENTS_PER_ENTITY`].
pub const MAX_COMPOSITE_COMPONENTS_PER_COMPONENT: usize = 100;
/// See [`MAX_COMPONENTS_PER_ENTITY`].
pub const MAX_PROPERTIES_PER_COMPONENT: usize = 1000;
/// See [`MAX_COMPONENTS_PER_ENTITY`].
pub const MAX_PROPERTY_GROUPS_PER_COMPONENT: usize = 100;

/// Which of this emulator's declared ceilings the contents of `entity` exceed,
/// as the message a `ServiceQuotaExceededException` carries — `None` when they
/// exceed none.
///
/// Checked over the WHOLE record a writer is about to store rather than over
/// the entries one call carries, so neither a single huge request nor a long
/// series of small ones can grow one entity past the ceilings
/// ([`MAX_COMPONENTS_PER_ENTITY`] and the three beside it). Shared by
/// `CreateEntity` and `UpdateEntity`, so both writers of an entity's contents
/// are bounded by one rule, and applied BEFORE anything is committed.
pub fn entity_contents_over_ceiling(entity: &Entity) -> Option<String> {
    let scope = format!("entity {}", entity.entity_id);
    over(
        &scope,
        "components",
        entity.components.len(),
        MAX_COMPONENTS_PER_ENTITY,
    )
    .or_else(|| {
        entity
            .components
            .iter()
            .find_map(|(component_name, component)| {
                let scope = format!("{scope} component {component_name}");
                over(
                    &scope,
                    "properties",
                    component.properties.len(),
                    MAX_PROPERTIES_PER_COMPONENT,
                )
                .or_else(|| {
                    over(
                        &scope,
                        "property groups",
                        component.property_groups.len(),
                        MAX_PROPERTY_GROUPS_PER_COMPONENT,
                    )
                })
                .or_else(|| {
                    over(
                        &scope,
                        "composite components",
                        component.composite_components.len(),
                        MAX_COMPOSITE_COMPONENTS_PER_COMPONENT,
                    )
                })
                .or_else(|| {
                    component
                        .composite_components
                        .values()
                        .find_map(|composite| {
                            let scope =
                                format!("{scope} compositeComponent {}", composite.component_path);
                            over(
                                &scope,
                                "properties",
                                composite.properties.len(),
                                MAX_PROPERTIES_PER_COMPONENT,
                            )
                            .or_else(|| {
                                over(
                                    &scope,
                                    "property groups",
                                    composite.property_groups.len(),
                                    MAX_PROPERTY_GROUPS_PER_COMPONENT,
                                )
                            })
                        })
                })
            })
    })
}

/// The message [`entity_contents_over_ceiling`] builds for one ceiling, or
/// `None` while `count` is within it.
fn over(scope: &str, kind: &str, count: usize, ceiling: usize) -> Option<String> {
    (count > ceiling).then(|| {
        format!("{scope} would hold {count} {kind}, past this service's limit of {ceiling}")
    })
}
/// See [`MAX_WORKSPACES`]. A metadata transfer job is an ACCOUNT-level
/// resource (`/metadata-transfer-jobs/{metadataTransferJobId}` names no
/// workspace), so its ceiling is per account rather than per workspace, and
/// `CreateMetadataTransferJob` answers the `ServiceQuotaExceededException` it
/// models once the account holds this many.
pub const MAX_METADATA_TRANSFER_JOBS: usize = 1000;

/// The `MetadataTransferJobState` a job the service has just accepted is in,
/// and the modelled failure state a run that could not be performed reaches.
///
/// `VALIDATING` is the state `CreateMetadataTransferJob` answers with: the job
/// has been accepted and its outcome is not yet due, so it is non-terminal and
/// the clock owes that outcome ([`METADATA_TRANSFER_JOB_SETTLE_SPAN`]). The
/// outcome itself is what the job's RUN established
/// ([`TransferOutcome`]): `COMPLETED` for a run that read its source and wrote
/// its destination, `ERROR` for one that could not —
/// and `CANCELLED`/`CANCELLING` belong to `CancelMetadataTransferJob`, which
/// owns them. The full value list is `METADATA_TRANSFER_JOB_STATE_VALUES` in
/// `ops::types`.
pub const METADATA_TRANSFER_JOB_STATE_VALIDATING: &str = "VALIDATING";
/// See [`METADATA_TRANSFER_JOB_STATE_VALIDATING`].
pub const METADATA_TRANSFER_JOB_STATE_ERROR: &str = "ERROR";

/// How long a metadata transfer job stays in
/// [`METADATA_TRANSFER_JOB_STATE_VALIDATING`] before the clock makes its
/// outcome due.
///
/// AWS documents no duration for a metadata transfer, so this span is CHOSEN by
/// this emulator, the same way (and for the same reasons) as
/// [`COMPONENT_TYPE_SETTLE_SPAN`]: deliberately non-zero, so a caller that
/// reads the job back in the same breath as the create still observes the
/// non-terminal state the create answered with, and deliberately short, so a
/// client that polls reaches the terminal state without a long wait. Nothing
/// sleeps for it: the instant it is due is stored on the record
/// ([`MetadataTransferJob::settles_at`]) and a reader materialises the
/// transition the clock already owed.
pub const METADATA_TRANSFER_JOB_SETTLE_SPAN: Duration = Duration::from_millis(500);

/// The `ErrorCode` a metadata transfer job whose RUN could not be performed
/// carries (`ops::types::ERROR_CODE_VALUES`).
///
/// Transferring metadata means reading the job's source — an Amazon S3 metadata
/// document or another workspace — and writing what it describes into the
/// destination ([`crate::state::perform_transfer`]). A run that could not do
/// that (a source object the store holds nothing at, a destination it could not
/// write, a workspace this service does not hold) ends in the service's own
/// modelled failure state carrying this code and the sentence that says which
/// of those it was, rather than being reported `COMPLETED` for a transfer that
/// did not happen.
pub const METADATA_TRANSFER_JOB_ERROR_CODE: &str = "PROCESSING_ERROR";

/// The `ErrorCode` a run whose source is no metadata document carries: the
/// object was read and what it holds describes no component type and no entity,
/// which is a fault in the document rather than in the transfer.
pub const METADATA_TRANSFER_JOB_VALIDATION_ERROR_CODE: &str = "VALIDATION_ERROR";

/// Why a job whose run never came back to report an outcome failed.
///
/// The run is performed inside the call that accepts the job, so a stored job
/// that carries no outcome ([`MetadataTransferJob::transfer`]) is one whose
/// transfer was never performed — and a transfer that was never performed
/// transferred nothing, so the job settles in the modelled failure state saying
/// exactly that rather than in a `COMPLETED` nothing earned.
pub const TRANSFER_NOT_PERFORMED_MESSAGE: &str =
    "the metadata transfer was not performed: nothing was read from the job's source and nothing \
     was written to its destination";

/// Why a transfer with an IoT SiteWise end at either side transfers nothing.
///
/// An IoT SiteWise source names asset models and assets to read, and an IoT
/// SiteWise destination is where asset models would be written; both live in
/// another service, and this emulator mounts no IoT SiteWise at all — so such a
/// run has nothing to read and nowhere to write, and the job carries the
/// modelled failure that says so rather than a `COMPLETED` nothing earned. The
/// sentence is this emulator's, not AWS's: it states the emulator's limitation,
/// which no AWS rule produces.
pub const IOT_SITE_WISE_END_MESSAGE: &str =
    "this emulator holds no IoT SiteWise asset models: a transfer with an IoT SiteWise end has \
     nothing to read and nowhere to write";

/// The resource portion of the ARN of the service-linked role a metadata
/// transfer job runs under.
///
/// `GetMetadataTransferJob` reports `metadataTransferJobRole` as `@required`,
/// but `CreateMetadataTransferJobRequest` has no role member at all, so the
/// role is the SERVICE's own and never the caller's input: the role IoT
/// TwinMaker assumes to transfer metadata. The ARN is built through
/// [`metadata_transfer_job_role`], so its account segment is the emulator's
/// canonical one.
pub const METADATA_TRANSFER_JOB_ROLE_PATH: &str =
    "role/aws-service-role/iottwinmaker.amazonaws.com/AWSServiceRoleForIoTTwinMaker";

/// The `MetadataTransferJobState` values `CancelMetadataTransferJob` owns, and
/// the third TERMINAL value of the enum.
///
/// `CANCELLING` is the state a cancellation the service has accepted puts the
/// job in: the transfer is being torn down, so it is non-terminal and the clock
/// owes `CANCELLED` ([`METADATA_TRANSFER_JOB_CANCEL_SPAN`]). `CANCELLED` and
/// `COMPLETED` are terminal, as [`METADATA_TRANSFER_JOB_STATE_ERROR`] is:
/// nothing moves a job out of one, so there is nothing left to cancel
/// ([`metadata_transfer_job_state_is_terminal`]). `COMPLETED` is a state no
/// operation of this emulator writes — it is the outcome of a transfer that ran,
/// and none does (see [`METADATA_TRANSFER_JOB_ERROR_CODE`]) — but it is named
/// here because the terminal rule holds for it too. The full value list is
/// `METADATA_TRANSFER_JOB_STATE_VALUES` in `ops::types`.
pub const METADATA_TRANSFER_JOB_STATE_CANCELLING: &str = "CANCELLING";
/// See [`METADATA_TRANSFER_JOB_STATE_CANCELLING`]. TERMINAL.
pub const METADATA_TRANSFER_JOB_STATE_CANCELLED: &str = "CANCELLED";
/// See [`METADATA_TRANSFER_JOB_STATE_CANCELLING`]. TERMINAL.
pub const METADATA_TRANSFER_JOB_STATE_COMPLETED: &str = "COMPLETED";

/// How long a metadata transfer job stays in
/// [`METADATA_TRANSFER_JOB_STATE_CANCELLING`] before the clock makes its
/// transition to [`METADATA_TRANSFER_JOB_STATE_CANCELLED`] due.
///
/// `CancelMetadataTransferJob` answers with the job's status, and the status of
/// a job whose cancellation the service has just accepted is the non-terminal
/// `CANCELLING` — the transfer is being torn down, and the job is `CANCELLED`
/// once that teardown has landed. AWS documents no duration for that window, so
/// this span is CHOSEN by this emulator, exactly as
/// [`METADATA_TRANSFER_JOB_SETTLE_SPAN`] and [`COMPONENT_TYPE_SETTLE_SPAN`] are:
/// deliberately non-zero, so a caller that reads the job back in the same breath
/// as the cancel still observes `CANCELLING`, and deliberately short, so a client
/// that polls reaches `CANCELLED` without a long wait. Nothing sleeps for it: the
/// instant it is due is stored on the record
/// ([`MetadataTransferJob::settles_at`]) and a reader materialises the
/// transition the clock already owed.
pub const METADATA_TRANSFER_JOB_CANCEL_SPAN: Duration = Duration::from_millis(500);

/// The one `SyncSource` this service supports.
///
/// Taken from the model, not chosen here: the `syncSource` member of every sync
/// operation documents "Currently the only supported syncSource is SITEWISE"
/// (`smithy_query(query="operation",operation="CreateSyncJob")`), and the
/// `SyncSource` shape itself is a plain `^[a-zA-Z_0-9]+$` string that cannot say
/// so — so a create naming any other source is refused with the
/// `ValidationException` the operation models rather than stored as a job
/// against a source nothing can sync from.
pub const SYNC_SOURCE_SITEWISE: &str = "SITEWISE";

/// The `SyncJobState` a sync job the service has just accepted is in, and the
/// terminal state it reaches.
///
/// `CREATING` is the state `CreateSyncJob` answers with: the job has been
/// accepted and nothing has been synchronised yet, so it is non-terminal and the
/// clock owes its outcome ([`SYNC_JOB_SETTLE_SPAN`]). `ERROR` is the modelled
/// failure state that outcome is in this emulator — see [`SyncJob::settle`] —
/// while `INITIALIZING` and `ACTIVE` belong to a synchronisation that really
/// ran and `DELETING` to `DeleteSyncJob`, which owns it. The full value list is
/// `SYNC_JOB_STATE_VALUES` in `ops::types`.
pub const SYNC_JOB_STATE_CREATING: &str = "CREATING";
/// See [`SYNC_JOB_STATE_CREATING`].
pub const SYNC_JOB_STATE_ERROR: &str = "ERROR";
/// See [`SYNC_JOB_STATE_CREATING`]. The state `DeleteSyncJob` answers with for
/// the removal it performs: the job is gone from the workspace that held it, and
/// the call reports the removal rather than a job's standing state.
pub const SYNC_JOB_STATE_DELETING: &str = "DELETING";

/// How long a sync job stays in [`SYNC_JOB_STATE_CREATING`] before the clock
/// makes its outcome due.
///
/// AWS documents no duration for bringing a sync job up, so this span is CHOSEN
/// by this emulator, the same way (and for the same reasons) as
/// [`METADATA_TRANSFER_JOB_SETTLE_SPAN`]: deliberately non-zero, so a caller
/// that reads the job back in the same breath as the create still observes the
/// non-terminal state the create answered with, and deliberately short, so a
/// client that polls reaches the terminal state without a long wait. Nothing
/// sleeps for it: the instant it is due is stored on the record
/// ([`SyncJob::settles_at`]) and a reader materialises the transition the clock
/// already owed.
pub const SYNC_JOB_SETTLE_SPAN: Duration = Duration::from_millis(500);

/// The `ErrorCode` a sync job that never synchronised anything carries
/// (`ops::types::ERROR_CODE_VALUES`), and the message that says why.
///
/// Synchronising means reading the sync source — an IoT SiteWise asset model
/// hierarchy — and creating, updating or deleting the corresponding entities
/// and component types in the workspace, which is work this emulator does not
/// perform: it holds no link to the source service. Nothing therefore runs for
/// an accepted job, and a job nothing ran ends in the service's own modelled
/// failure state with the reason that says so, rather than being reported
/// `ACTIVE` for a synchronisation that is not happening. `SYNC_INITIALIZING_ERROR`
/// is the code for a job that failed bringing the sync up, which is where this
/// one stops. The message is this emulator's, not AWS's: it states the
/// emulator's limitation, which no AWS rule produces.
pub const SYNC_JOB_ERROR_CODE: &str = "SYNC_INITIALIZING_ERROR";
/// See [`SYNC_JOB_ERROR_CODE`].
pub const SYNC_JOB_ERROR_MESSAGE: &str =
    "this emulator synchronises nothing: no resource was read from the sync source and none was \
     created, updated or deleted in the workspace";

/// This service's own principal, as an IAM trust policy names it: the principal
/// a role this service has to act as must admit.
///
/// A `syncRole` "is used by the SyncJob to read from the syncSource, and create,
/// update, or delete the corresponding resources"
/// (`smithy_query(query="operation",operation="CreateSyncJob")`), and a
/// workspace's `role` is "the ARN of the execution role associated with the
/// workspace"
/// (`smithy_query(query="doc",shape="CreateWorkspaceRequest",member="role")`),
/// so the service itself assumes each of them — which IAM only lets it do when
/// the role's trust policy grants this principal `sts:AssumeRole`. The rule that
/// reads a role's trust policy against this principal is [`assume_role_refusal`],
/// shared by every operation that accepts such a role.
pub const SERVICE_PRINCIPAL: &str = "iottwinmaker.amazonaws.com";

/// The IAM store a caller-supplied role is resolved against.
///
/// It is the IAM service's OWN state struct, reached through the SAME shared
/// engine this service's store lives in, so the roles read here are the very
/// records `CreateRole` wrote rather than a second copy of them.
pub type IamState =
    ::service_com_amazonaws_iam_awsidentitymanagementv20100508::ComAmazonawsIamAwsidentitymanagementv20100508State;

/// The IAM store in the SAME shared engine, which is where the account's roles
/// live. This is the IAM service's OWN accessor, called rather than copied;
/// `None` has the same meaning as for [`service_state`]: the engine carries no
/// such store, which a handler reports as a server fault.
pub fn iam_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<IamState>> {
    ::service_com_amazonaws_iam_awsidentitymanagementv20100508::state::service_state(engine)
}

/// The sentence this service words the refusal of a role it cannot assume with.
///
/// The model declares the `ValidationException` (HTTP 400) but states no message
/// text for it; the service's own wording is this ONE fixed sentence, the same
/// for every member that names a role. It is a constant, not a template: the
/// service interpolates nothing into it — not the member, not the role ARN, not
/// its own principal, not the action the trust policy withholds — so neither the
/// request nor this emulator's state can reach the text a caller matches on.
pub const UNASSUMABLE_ROLE_MESSAGE: &str =
    "Could not assume the role provided, verify permissions";

/// `None` when this emulator has no reason to think the service could not act as
/// the role `role_arn`; otherwise the message its refusal carries
/// ([`UNASSUMABLE_ROLE_MESSAGE`]).
///
/// Where a request names a role THIS service has to act as — a workspace's
/// execution role (`CreateWorkspaceRequest$role`, `UpdateWorkspaceRequest$role`)
/// or a sync job's (`CreateSyncJobRequest$syncRole`) — the role is resolved
/// against the account's IAM records before anything is persisted, and a role
/// whose trust policy withholds `sts:AssumeRole` from [`SERVICE_PRINCIPAL`] is
/// refused synchronously rather than stored and left to fail later. ONE function
/// for every such member, carrying ONE message: the rule is about the role, not
/// about the operation that named it, so nothing about the caller or the member
/// differs between its callers' refusals.
///
/// NOTHING is parsed here. The role is found by COMPARING the stored ARN string
/// with the one the request carried — no splitting, no indexing, no arithmetic —
/// and the trust document is read by the ONE shared reader every service that
/// resolves a caller-supplied role uses
/// (`service_common::iam_trust_policy::admits_service_principal`, which is total
/// over arbitrary text: it answers `None` for anything it cannot read as a
/// policy), so two services asked about the same document cannot answer
/// differently and no input can panic. Both reads are O(the account's roles)
/// over records only an IAM call can create.
///
/// An ARN the IAM store holds no role for is NOT evidence of a missing trust
/// (the role may belong to another account, or to a client that never told this
/// emulator about it), so such a request goes ahead; nor is a document this
/// emulator cannot read as a policy.
///
/// That last part is why this reads the IAM store directly rather than calling
/// `service_common::iam_role::role_is_unassumable`, which answers the same
/// question for the services that refuse a role IAM reports ABSENT: this service
/// accepts such a role — it has no record saying the role is unusable — so the
/// two readings answer differently where IAM holds no role, and only the trust
/// half of the question is shared (the one reader
/// `service_common::iam_trust_policy::admits_service_principal`).
pub fn assume_role_refusal(role_arn: &str, iam: &IamState) -> Option<&'static str> {
    let role = iam.roles.values().find(|role| role.arn == role_arn)?;
    let document = role.assume_role_policy_document.as_deref()?;
    match ::service_common::iam_trust_policy::admits_service_principal(document, SERVICE_PRINCIPAL) {
        // A document this emulator cannot read as a policy is no evidence either
        // way, and one that grants the principal the action is the grant itself.
        None | Some(true) => None,
        Some(false) => Some(UNASSUMABLE_ROLE_MESSAGE),
    }
}

/// The `PricingMode` an account that has never changed its pricing plan is on,
/// and the `UpdateReason` such a plan carries.
///
/// `BASIC` is the first value of the model's `PricingMode` enum
/// (`ops::types::PRICING_MODE_VALUES`) and the pay-as-you-go mode the service
/// bills an untouched account in; `DEFAULT` is the `UpdateReason` value that
/// names exactly that — a plan nobody has updated
/// (`ops::types::UPDATE_REASON_VALUES`).
pub const PRICING_MODE_BASIC: &str = "BASIC";
/// See [`PRICING_MODE_BASIC`].
pub const UPDATE_REASON_DEFAULT: &str = "DEFAULT";

/// The `UpdateReason` a plan change records when it replaces a plan whose
/// `pricingMode` differs from the one in force
/// (`ops::types::UPDATE_REASON_VALUES`).
pub const UPDATE_REASON_PRICING_MODE_UPDATE: &str = "PRICING_MODE_UPDATE";
/// The `UpdateReason` a plan change records when the mode is unchanged, so the
/// bundles — and with them the tier the bundles place the account in — are all
/// the change can have moved. See [`UPDATE_REASON_PRICING_MODE_UPDATE`].
pub const UPDATE_REASON_PRICING_TIER_UPDATE: &str = "PRICING_TIER_UPDATE";
/// The `UpdateReason` a plan change records when it supersedes a plan change
/// that had been accepted but had not taken effect: that earlier change is
/// overwritten and never becomes the plan in force. See
/// [`UPDATE_REASON_PRICING_MODE_UPDATE`].
pub const UPDATE_REASON_OVERWRITTEN: &str = "OVERWRITTEN";
/// The `UpdateReason` a plan carries once the SERVICE itself has moved its tier,
/// the account's billable entity count having crossed into another band
/// ([`pricing_tier_for`]) — a change no caller asked for. See
/// [`UPDATE_REASON_PRICING_MODE_UPDATE`].
pub const UPDATE_REASON_ENTITY_COUNT_UPDATE: &str = "ENTITY_COUNT_UPDATE";

/// How long a plan change that does NOT take effect at once stays pending
/// before the clock makes it the plan in force.
///
/// AWS's IoT TwinMaker pricing documentation states the rule this stands in for:
/// a switch away from a tiered-bundle plan to pay-as-you-go takes effect at the
/// start of the account's next billing cycle, the bundle already bought covering
/// the cycle the account is in — so the change is accepted now and applies later
/// ([`pricing_change_is_deferred`]). That wait is the remainder of a billing
/// cycle, a calendar quantity rather than a published duration, so the span here
/// is CHOSEN by this emulator, exactly as the settle spans above are: it is
/// deliberately non-zero, so a caller that reads the plan back in the same
/// breath as the change still sees the change PENDING and the old plan still in
/// force, and deliberately short, so a client that polls sees it land without a
/// long wait. Nothing sleeps for it: the instant the change falls due is its own
/// `effectiveDateTime` ([`PricingPlanRecord::effective_date_time`]) and a reader
/// materialises what the clock already owed
/// ([`RegionalState::settle_pricing_plan`]).
pub const PRICING_PLAN_PENDING_SPAN: Duration = Duration::from_millis(500);

/// The billable entity counts the modelled `PricingTier` bands end at: the
/// largest count still in `TIER_1`, then in `TIER_2`, then in `TIER_3` — every
/// count above the last of them being in the final tier
/// (`ops::types::PRICING_TIER_VALUES` names four).
///
/// A bundled plan is billed by TIER, and the tier an account sits in follows how
/// many billable entities it keeps: `TIER_1` is the smallest band and each
/// further tier covers a larger count, the last one unbounded. That ORDERING is
/// the rule; the thresholds themselves are not in this service's model, so the
/// three below are CHOSEN by this emulator. They are spaced by powers of ten and
/// placed inside this store's own ceilings ([`MAX_WORKSPACES`] ×
/// [`MAX_ENTITIES_PER_WORKSPACE`]), so every modelled tier is reachable here
/// rather than only the first.
const PRICING_TIER_BAND_CEILINGS: [i64; 3] = [100, 1_000, 10_000];

/// The `PricingTier` a billable entity count of `billable_entity_count` places
/// the account in: the first band the count fits in
/// ([`PRICING_TIER_BAND_CEILINGS`]), or the last modelled tier when it is above
/// them all.
///
/// Total over every count, including zero (an account with no entity at all is
/// in the smallest band), so a bundled plan always has a tier to report.
pub fn pricing_tier_for(billable_entity_count: i64) -> &'static str {
    let tiers = crate::ops::types::PRICING_TIER_VALUES;
    for (band, ceiling) in PRICING_TIER_BAND_CEILINGS.iter().enumerate() {
        if billable_entity_count <= *ceiling {
            // The ceilings are declared alongside the tiers they end, so there
            // is a tier for each; the fallback keeps this total if that ever
            // stops holding.
            return tiers.get(band).copied().unwrap_or("TIER_1");
        }
    }
    tiers.last().copied().unwrap_or("TIER_1")
}

/// Whether a change from the plan in force (`pricing_mode_in_force`) to
/// `requested_pricing_mode` is one the service accepts now and applies LATER,
/// rather than one that takes effect at once.
///
/// Leaving a bundled plan for the pay-as-you-go mode is the deferred change: the
/// bundle the account bought covers the billing cycle it is in, so the move off
/// it is billed from the start of the next one — until then the bundled plan is
/// still the plan in force and the change is reported as pending
/// ([`RegionalState::pending_pricing_plan`]). Every other change — onto a
/// bundled plan, between two bundled plans, or the pay-as-you-go plan re-set on
/// itself — buys the account more than it had and takes effect at once.
pub fn pricing_change_is_deferred(
    pricing_mode_in_force: &str,
    requested_pricing_mode: &str,
) -> bool {
    pricing_mode_is_bundled(pricing_mode_in_force)
        && !pricing_mode_is_bundled(requested_pricing_mode)
}

/// Whether `pricing_mode` names a BUNDLED pricing plan — one whose billing is
/// described by the bundles it was set with — rather than the pay-as-you-go
/// mode.
///
/// [`PRICING_MODE_BASIC`] is the pay-as-you-go mode, and it is the mode of the
/// plan an untouched account is on, which names no bundle at all
/// ([`PricingPlanRecord::default_plan`]). Every other `PricingMode` value
/// (`ops::types::PRICING_MODE_VALUES`) names a plan bought as bundles, and a
/// bundled plan is reported through `BundleInformation`, whose `bundleNames` is
/// `@required` with `@length(1..10)` — so there is no form of a bundled plan
/// that names no bundle, and no form of the pay-as-you-go plan that names one.
pub fn pricing_mode_is_bundled(pricing_mode: &str) -> bool {
    pricing_mode != PRICING_MODE_BASIC
}

/// Per-service state for com-amazonaws-iottwinmaker-awsiottwinmaker. A handler reaches it through [`service_state`].
///
/// IoT TwinMaker is a REGIONAL service, so this holds ONE store per Region
/// ([`RegionalState`]) rather than one store for the account: a resource exists
/// only in the Region whose endpoint created it, the same id may name a
/// different resource in another Region, and every resource's ARN carries the
/// Region of the store that holds it. A handler picks its store with
/// [`region`](Self::region) (to report) or [`region_mut`](Self::region_mut) (to
/// write), passing the Region of the request's own credential scope
/// ([`request_region`]) — never the Region stored on another record.
#[derive(Debug)]
pub struct ComAmazonawsIottwinmakerAwsiottwinmakerState {
    /// The Regions this account has created something in, keyed by Region.
    ///
    /// Private: a Region's store is reached through
    /// [`region`](Self::region)/[`region_mut`](Self::region_mut), so no caller
    /// can read one Region's resources while serving another's request.
    regions: BTreeMap<String, RegionalState>,
    /// When this account's store came into being.
    ///
    /// Every Region's store dates from here rather than from the first call
    /// that touched that Region, so the pricing plan an untouched Region
    /// reports ([`PricingPlanRecord::default_plan`]) does not move when a
    /// later create materialises the Region's store — a read and the write
    /// after it report one plan.
    created_at: SystemTime,
    /// The read view of a Region nothing has been created in: an empty store,
    /// so a REPORTING operation answers for such a Region (no resources, the
    /// default pricing plan) without materialising anything a later caller
    /// could observe. Never mutated.
    empty_region: RegionalState,
}

impl ComAmazonawsIottwinmakerAwsiottwinmakerState {
    /// The store of `region`, for a caller that REPORTS: the Region's own
    /// store, or an empty one when nothing has been created in that Region.
    pub fn region(&self, region: &str) -> &RegionalState {
        self.regions.get(region).unwrap_or(&self.empty_region)
    }

    /// The store of `region`, for a caller that WRITES: materialised on first
    /// use, since a Region's store comes into being with the first resource
    /// created in it.
    ///
    /// Materialising one is never what GROWS this map: a store holding nothing
    /// a caller put there answers every read exactly as the absence of one does
    /// ([`region`](Self::region)), so the stores left behind by calls that
    /// wrote nothing are dropped here rather than kept — a caller cannot grow
    /// the map by naming Regions it creates nothing in, and what it CAN create
    /// is bounded by [`MAX_REGIONS`] through
    /// [`region_ceiling_reached`](Self::region_ceiling_reached).
    pub fn region_mut(&mut self, region: &str) -> &mut RegionalState {
        let created_at = self.created_at;
        if !self.regions.contains_key(region) {
            let default_plan = PricingPlanRecord::default_plan(created_at);
            self.regions
                .retain(|_, store| !store.is_untouched(&default_plan));
        }
        self.regions
            .entry(region.to_string())
            .or_insert_with(|| RegionalState::new(created_at))
    }

    /// Whether this store has no room to give `region` its FIRST resource: it
    /// already holds resources for [`MAX_REGIONS`] Regions and `region` is not
    /// one of them.
    ///
    /// Checked by every CREATE, which is the only kind of call that gives a
    /// Region its first resource; a Region the store already holds something
    /// for is never refused, and neither is a call that only changes or removes
    /// what is already there.
    pub fn region_ceiling_reached(&self, region: &str) -> bool {
        let default_plan = PricingPlanRecord::default_plan(self.created_at);
        let holds_something = |store: &RegionalState| !store.is_untouched(&default_plan);
        if self.regions.get(region).is_some_and(holds_something) {
            return false;
        }
        self.regions
            .values()
            .filter(|store| holds_something(store))
            .count()
            >= MAX_REGIONS
    }
}

/// One Region's store: everything this service holds for the account in a
/// single Region.
///
/// Keyed by nothing — the Region is the key it is filed under in
/// [`ComAmazonawsIottwinmakerAwsiottwinmakerState::regions`] — so a record in
/// here cannot be reached while serving another Region's request.
#[derive(Debug)]
pub struct RegionalState {
    /// Workspaces by `workspaceId`.
    pub workspaces: BTreeMap<String, Workspace>,
    /// Every ARN this service has minted, so an operation whose input accepts an
    /// id *or* an ARN (`GetWorkspace`, and the tagging operations in a later
    /// slice) resolves one without scanning every workspace.
    pub arn_index: BTreeMap<String, ResourceRef>,
    /// The pricing plan this account is billed under in this Region for the
    /// current billing cycle — the one `GetPricingPlan` reports as
    /// `currentPricingPlan`.
    ///
    /// Region-wide, not per workspace: the pricing operations carry no
    /// workspace in their input, so there is ONE plan per account and Region,
    /// held here beside the workspaces rather than on each of them. Present
    /// rather than optional, because an account always HAS a plan: until
    /// something changes it, that plan is the service default
    /// ([`PricingPlanRecord::default_plan`]), effective from the instant this
    /// store came into being — so a read never has to mint one, and two reads
    /// of an unchanged plan report the same instants.
    pub pricing_plan: PricingPlanRecord,
    /// The plan change that is accepted but not yet in force — what
    /// `GetPricingPlan` reports as `pendingPricingPlan` — and `None` while
    /// nothing is pending, which is what an account whose plan nobody has
    /// changed looks like.
    ///
    /// A change the service applies LATER rather than at once lands here
    /// ([`pricing_change_is_deferred`]), carrying the instant it falls due as
    /// its own `effectiveDateTime`, while [`pricing_plan`](Self::pricing_plan)
    /// keeps reporting the plan still in force. Nothing sleeps for that
    /// instant: a reader materialises the change the clock has already made due
    /// ([`Self::settle_pricing_plan`]), and a further change that arrives first
    /// SUPERSEDES what is here, which is the change
    /// [`UPDATE_REASON_OVERWRITTEN`] names.
    pub pending_pricing_plan: Option<PricingPlanRecord>,
    /// The metadata transfer jobs this account holds in this Region, keyed by
    /// `metadataTransferJobId`.
    ///
    /// Region-wide, beside the workspaces rather than inside one: a job is
    /// addressed as `/metadata-transfer-jobs/{metadataTransferJobId}`, which
    /// names no workspace, and one job's source and destination may name
    /// different workspaces (or none at all, for an Amazon S3 or an IoT
    /// SiteWise end), so no single workspace can own it. A job's ARN is
    /// deliberately NOT in [`arn_index`](Self::arn_index): that index exists to
    /// resolve an id-or-ARN input and to find a taggable resource's tag map,
    /// and a metadata transfer job is addressed only by its id and carries no
    /// `tags` member on any of its shapes.
    pub metadata_transfer_jobs: BTreeMap<String, MetadataTransferJob>,
}

impl Default for ComAmazonawsIottwinmakerAwsiottwinmakerState {
    fn default() -> Self {
        // The account's plan dates from the moment the account's store exists,
        // so the default plan's instants are captured here, ONCE, rather than
        // by whichever call happens to read it first — or by whichever Region
        // is touched first.
        let created_at = SystemTime::now();
        Self {
            regions: BTreeMap::new(),
            created_at,
            empty_region: RegionalState::new(created_at),
        }
    }
}

impl RegionalState {
    /// An empty Region store whose default pricing plan dates from
    /// `created_at`, the instant the account's store came into being.
    fn new(created_at: SystemTime) -> Self {
        Self {
            workspaces: BTreeMap::new(),
            arn_index: BTreeMap::new(),
            pricing_plan: PricingPlanRecord::default_plan(created_at),
            pending_pricing_plan: None,
            metadata_transfer_jobs: BTreeMap::new(),
        }
    }

    /// Whether this store holds nothing a caller put in this Region: no
    /// workspace, no metadata transfer job, nothing pending, and still the
    /// `default_plan` an untouched Region is on.
    ///
    /// Such a store answers every read exactly as the absence of one does, so
    /// it is the store [`ComAmazonawsIottwinmakerAwsiottwinmakerState`] may drop
    /// and the one that counts against no Region ceiling. The ARN index is not
    /// consulted: it only ever holds the ARNs of the resources above.
    fn is_untouched(&self, default_plan: &PricingPlanRecord) -> bool {
        self.workspaces.is_empty()
            && self.metadata_transfer_jobs.is_empty()
            && self.pending_pricing_plan.is_none()
            && &self.pricing_plan == default_plan
    }
}

/// One stored pricing plan: the mode and bundles it was set to, and when.
///
/// `billableEntityCount` is NOT a field here. It is the account's current
/// billable entity count, which the stored entities already say
/// ([`RegionalState::billable_entity_count`]) —
/// keeping a copy beside the plan would let an entity created or deleted after
/// the plan was set leave it stale, the same reason `hasChildEntities` is
/// derived.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PricingPlanRecord {
    /// A `PricingMode` value (`ops::types::PRICING_MODE_VALUES`).
    pub pricing_mode: String,
    /// An `UpdateReason` value (`ops::types::UPDATE_REASON_VALUES`): why this
    /// plan replaced the one before it.
    pub update_reason: String,
    /// The bundles this plan was set with, empty when it names none — a plan
    /// without bundles reports no `bundleInformation` at all, since
    /// `BundleInformation::bundleNames` is `@required` and `@length(1..10)`, so
    /// an empty list is not a value the shape admits.
    pub bundle_names: Vec<String>,
    /// The `PricingTier` the account's billable entity count places this plan in
    /// ([`pricing_tier_for`]), and `None` for a plan that names no bundle: a
    /// tier is what a BUNDLED plan is billed by, and a plan without bundles
    /// reports no `bundleInformation` to carry one.
    ///
    /// Stored rather than derived at report time, because a MOVE between bands
    /// is itself a change to the plan — it is what
    /// [`UPDATE_REASON_ENTITY_COUNT_UPDATE`] describes — so the tier the plan
    /// was last billed at has to be there for the move to be noticed
    /// ([`RegionalState::settle_pricing_plan`]).
    pub pricing_tier: Option<String>,
    /// When this plan takes (or took) effect.
    pub effective_date_time: SystemTime,
    /// When this plan was set.
    pub update_date_time: SystemTime,
}

impl PricingPlanRecord {
    /// The plan an account that has never changed its pricing plan is on:
    /// [`PRICING_MODE_BASIC`] with no bundles, set and effective at `at` and
    /// carrying the [`UPDATE_REASON_DEFAULT`] reason.
    pub fn default_plan(at: SystemTime) -> Self {
        Self {
            pricing_mode: PRICING_MODE_BASIC.to_string(),
            update_reason: UPDATE_REASON_DEFAULT.to_string(),
            bundle_names: Vec::new(),
            pricing_tier: None,
            effective_date_time: at,
            update_date_time: at,
        }
    }

    /// The `PricingTier` this plan is billed at while the account holds
    /// `billable_entity_count` billable entities, and `None` for a plan that
    /// reports no tier at all.
    ///
    /// A tier is reported INSIDE `bundleInformation`, so a plan has one exactly
    /// when it names bundles — the same condition
    /// [`as_pricing_plan`](Self::as_pricing_plan) reports that structure under —
    /// and which tier that is follows the account's billable entity count
    /// ([`pricing_tier_for`]).
    pub fn tier_at(&self, billable_entity_count: i64) -> Option<String> {
        (!self.bundle_names.is_empty())
            .then(|| pricing_tier_for(billable_entity_count).to_string())
    }

    /// This record as the modelled `PricingPlan`, reported with the account's
    /// current `billable_entity_count`.
    ///
    /// Shared by every reader of a stored plan, so the current and the pending
    /// plan — and `GetPricingPlan` and `UpdatePricingPlan` — report one plan the
    /// same way.
    pub fn as_pricing_plan(&self, billable_entity_count: i64) -> crate::ops::types::PricingPlan {
        crate::ops::types::PricingPlan {
            billable_entity_count: Some(billable_entity_count),
            // Omitted while the plan names no bundle: `bundleNames` is required
            // inside `BundleInformation`, so there is no empty form of it.
            bundle_information: (!self.bundle_names.is_empty()).then(|| {
                crate::ops::types::BundleInformation {
                    bundle_names: self.bundle_names.clone(),
                    pricing_tier: self.pricing_tier.clone(),
                }
            }),
            effective_date_time: timestamp(self.effective_date_time),
            pricing_mode: self.pricing_mode.clone(),
            update_date_time: timestamp(self.update_date_time),
            update_reason: self.update_reason.clone(),
        }
    }
}

/// What an ARN in [`RegionalState::arn_index`] points at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResourceRef {
    /// A workspace, by id.
    Workspace { workspace_id: String },
    /// An entity, by the workspace that owns it and its own id.
    Entity {
        workspace_id: String,
        entity_id: String,
    },
    /// A component type, by the workspace that owns it and its own id.
    ComponentType {
        workspace_id: String,
        component_type_id: String,
    },
    /// A scene, by the workspace that owns it and its own id.
    Scene {
        workspace_id: String,
        scene_id: String,
    },
    /// A sync job, by the workspace that owns it and the sync source that
    /// identifies it inside that workspace.
    SyncJob {
        workspace_id: String,
        sync_source: String,
    },
}

/// One stored workspace.
#[derive(Debug, Clone)]
pub struct Workspace {
    pub workspace_id: String,
    pub arn: String,
    /// The workspace's description, the empty string when the create named
    /// none: `Description` is modelled `@length 0..2048`, so the empty string
    /// is a legal value, and the service carries the member on every resource
    /// rather than leaving it absent. See [`Entity::description`].
    pub description: String,
    pub s3_location: Option<String>,
    pub role: Option<String>,
    pub tags: BTreeMap<String, String>,
    pub creation_date_time: SystemTime,
    pub update_date_time: SystemTime,
    /// The entities created in this workspace, keyed by `entityId`.
    pub entities: BTreeMap<String, Entity>,
    /// The component types this workspace holds, keyed by `componentTypeId`.
    /// Owned by the workspace for the same reason entities are: a component
    /// type is addressed as
    /// `/workspaces/{workspaceId}/component-types/{componentTypeId}`, so it
    /// cannot outlive the workspace that contains it, and two operations must
    /// not read two copies of one component type.
    ///
    /// Holds BOTH the component types a caller created here and the service's
    /// own built-in ones, which every workspace is provisioned with when it is
    /// created ([`built_in_component_types`]) — they are addressed through the
    /// same path and read through the same operations, so they live in the same
    /// map rather than in a second one beside it. The two are told apart by the
    /// reserved namespace their ids lie in
    /// ([`built_in_component_type_id`]): see
    /// [`created_component_types`](Workspace::created_component_types) for the
    /// caller's own.
    pub component_types: BTreeMap<String, ComponentType>,
    /// The scenes created in this workspace, keyed by `sceneId`. Owned by the
    /// workspace for the same reason entities and component types are: a scene
    /// is addressed as `/workspaces/{workspaceId}/scenes/{sceneId}`, so it
    /// cannot outlive the workspace that contains it, and two operations must
    /// not read two copies of one scene.
    pub scenes: BTreeMap<String, Scene>,
    /// The sync jobs created in this workspace, keyed by `syncSource`.
    ///
    /// Owned by the workspace for the same reason its scenes are: a sync job is
    /// addressed as `/workspaces/{workspaceId}/sync-jobs/{syncSource}`, so it
    /// cannot outlive the workspace that contains it. The KEY is the sync
    /// source, because that label is the job's whole identity inside its
    /// workspace — the create, the read and the delete all address a job by it
    /// and nothing else — so one workspace holds at most one job per source.
    pub sync_jobs: BTreeMap<String, SyncJob>,
}

/// One stored scene.
///
/// A scene has no lifecycle `state` member anywhere in its API — `CreateScene`
/// answers only the ARN and the creation instant, and `GetScene` reports no
/// status — so a scene is READY as soon as it is created and there is no
/// transition for the clock to owe. Its only reported failure channel is
/// `GetScene`'s `error` (`SceneError`), whose single modelled code
/// (`MATTERPORT_ERROR`) belongs to a Matterport import this emulator never
/// performs, so a scene it stores carries none.
#[derive(Debug, Clone)]
pub struct Scene {
    pub scene_id: String,
    pub arn: String,
    pub workspace_id: String,
    /// The `s3://…` path of the scene's content definition file. Required on
    /// both the create and the read, so it is stored, never derived.
    pub content_location: String,
    /// The scene's description, absent when the create named none:
    /// `description` is optional on `CreateSceneRequest` AND on
    /// `GetSceneResponse`, so unlike a workspace's it is carried as an option
    /// rather than resolved to the empty string.
    pub description: Option<String>,
    /// The capabilities the scene renders itself with, in the order the create
    /// gave them: `SceneCapabilities` is a LIST, so its order is part of the
    /// value.
    pub capabilities: Vec<String>,
    /// The scene metadata the caller supplied (`sceneMetadata`), which
    /// `GetScene` reports back as the response metadata.
    pub scene_metadata: BTreeMap<String, String>,
    pub tags: BTreeMap<String, String>,
    pub creation_date_time: SystemTime,
    pub update_date_time: SystemTime,
}

/// One stored sync job.
///
/// A sync job is a workspace's standing link to another service: its
/// `syncSource` says which service's resources are synchronised into the
/// workspace, and its `syncRole` is the role the synchronisation would run
/// under. The source is the job's identity inside the workspace, so it is the
/// key of [`Workspace::sync_jobs`] and is not repeated here.
///
/// Like a [`MetadataTransferJob`], the job is accepted in a non-terminal state
/// and carries the instant its outcome falls due ([`settles_at`](Self::settles_at)),
/// which a reader materialises rather than sleeps for.
#[derive(Debug, Clone)]
pub struct SyncJob {
    pub sync_source: String,
    pub arn: String,
    pub workspace_id: String,
    /// The ARN of the IAM role the create supplied: "this IAM role is used by
    /// the SyncJob to read from the syncSource, and create, update, or delete
    /// the corresponding resources". Required on both the create and the read,
    /// so it is stored, never derived.
    pub sync_role: String,
    /// The tags the create supplied. `CreateSyncJob` accepts a `tags` map, so a
    /// sync job is a taggable resource of this service: its ARN is indexed in
    /// [`RegionalState::arn_index`] and the
    /// tagging operations reach this map through it, exactly as they reach a
    /// scene's.
    pub tags: BTreeMap<String, String>,
    pub creation_date_time: SystemTime,
    /// When the job last changed — the create, until the clock settles its
    /// outcome, which moves it to the instant that outcome fell due.
    pub update_date_time: SystemTime,
    /// The job's `SyncJobState`, as STORED: [`SYNC_JOB_STATE_CREATING`] from the
    /// create until [`settles_at`](Self::settles_at) passes, then the terminal
    /// state [`settle`](Self::settle) writes.
    pub state: String,
    /// Why the job is in a failed state, and `None` while nothing has failed.
    pub error: Option<crate::ops::types::ErrorDetails>,
    /// The instant the transition out of [`SYNC_JOB_STATE_CREATING`] falls due
    /// — the instant of the create plus [`SYNC_JOB_SETTLE_SPAN`]. Stored, never
    /// slept for, so a reader can materialise a transition the clock already
    /// owed without any caller having had to poll.
    pub settles_at: SystemTime,
    /// The resources this job's synchronisation has acted on, keyed by the
    /// `externalId` that identifies each in the sync SOURCE — see
    /// [`SyncResource`], which is also where what writes one is set out.
    ///
    /// Keyed by the external id, so iterating the map is external-id order:
    /// two listings of one job report its resources in the same order and a
    /// page of them can be resumed at a row rather than at an offset
    /// (`crate::ops::list_sync_resources`).
    pub sync_resources: BTreeMap<String, SyncResource>,
}

impl SyncJob {
    /// The job a `CreateSyncJob` call accepted at `now`: accepted, nothing
    /// synchronised yet, and its outcome due [`SYNC_JOB_SETTLE_SPAN`] later.
    pub fn accepted(
        sync_source: String,
        arn: String,
        workspace_id: String,
        sync_role: String,
        tags: BTreeMap<String, String>,
        now: SystemTime,
    ) -> Self {
        Self {
            sync_source,
            arn,
            workspace_id,
            sync_role,
            tags,
            creation_date_time: now,
            // Nothing has changed the job yet, so it was last updated when it
            // was created.
            update_date_time: now,
            state: SYNC_JOB_STATE_CREATING.to_string(),
            error: None,
            settles_at: now + SYNC_JOB_SETTLE_SPAN,
            // Nothing has been synchronised yet, so the job has acted on no
            // resource of its source: an accepted job holds none.
            sync_resources: BTreeMap::new(),
        }
    }

    /// Materialise the lifecycle transition the CLOCK has already made due.
    ///
    /// Called by a reader before it reports this job, exactly as
    /// [`MetadataTransferJob::settle`] is: the outcome was owed at
    /// [`settles_at`](Self::settles_at) whether or not anybody called, so
    /// writing it here makes the record say what was already true rather than
    /// changing anything a read caused.
    ///
    /// The outcome is the modelled failure state: nothing was synchronised,
    /// because this emulator synchronises nothing (see [`SYNC_JOB_ERROR_CODE`]),
    /// so the job carries the error that says so instead of an `ACTIVE` nothing
    /// earned. Idempotent, a no-op before the instant falls due, and a no-op on
    /// a job some other operation has already moved out of the non-terminal
    /// state.
    pub fn settle(&mut self, now: SystemTime) {
        if self.state != SYNC_JOB_STATE_CREATING || now < self.settles_at {
            return;
        }
        self.state = SYNC_JOB_STATE_ERROR.to_string();
        self.error = Some(crate::ops::types::ErrorDetails {
            code: Some(SYNC_JOB_ERROR_CODE.to_string()),
            message: Some(SYNC_JOB_ERROR_MESSAGE.to_string()),
        });
        // The change fell due at `settles_at`, not at the instant a caller
        // happened to ask, so that is when the job was last updated: two reads
        // of a settled job report the same instant.
        self.update_date_time = self.settles_at;
    }

    /// The job's modelled `SyncJobStatus`: its stored state and, when it has
    /// failed, why.
    ///
    /// The `error` structure is reported WHOLE, the way every other status of
    /// this service is (see [`status`]): a job that has not failed carries an
    /// `ErrorDetails` with neither a code nor a message, so a client that reads
    /// `status.error` needs no null guard.
    pub fn status(&self) -> crate::ops::types::SyncJobStatus {
        crate::ops::types::SyncJobStatus {
            state: Some(self.state.clone()),
            error: Some(self.error.clone().unwrap_or_default()),
        }
    }
}

/// One resource a sync job's synchronisation has acted on.
///
/// A sync job synchronises the resources of its source into the workspace that
/// holds the job, so each resource it has acted on is named at BOTH ends: in
/// the source by the `externalId` that identifies it there — which is why it is
/// the key of [`SyncJob::sync_resources`] — and, once the synchronisation has
/// created the matching resource in the workspace, in the workspace by its
/// `resourceId`, of the `resourceType` that says which kind of resource it is.
/// A resource no workspace resource has been created for yet carries no
/// `resourceId`, which is why that member is optional on the modelled summary;
/// its own `state` is the state of ITS synchronisation, independent of the
/// job's (one resource can be in sync while another is still being processed or
/// has failed).
///
/// NOTHING in this emulator writes one: synchronising means reading the source
/// service and creating, updating or deleting the corresponding resources in
/// the workspace, which this emulator does not perform
/// ([`SYNC_JOB_ERROR_CODE`]), so a job accepted here acts on no resource and
/// holds none. The listing that reports them
/// (`crate::ops::list_sync_resources`) therefore reports what the job HOLDS —
/// an empty page for every job this emulator's create accepted — rather than
/// rows it invents for a synchronisation that did not run.
#[derive(Debug, Clone)]
pub struct SyncResource {
    /// Which kind of workspace resource this is, from the modelled
    /// `SyncResourceType` value list (`ops::types::SYNC_RESOURCE_TYPE_VALUES`).
    pub resource_type: String,
    /// The resource's identity in the sync SOURCE, which is the key it is held
    /// under and is therefore not repeated in the map's values by accident: it
    /// is stored here so a row reports it without the caller having to read the
    /// key.
    pub external_id: String,
    /// The id of the resource in the WORKSPACE, and `None` until the
    /// synchronisation has created one for it.
    pub resource_id: Option<String>,
    /// The resource's `SyncResourceState`
    /// (`ops::types::SYNC_RESOURCE_STATE_VALUES`), as stored.
    pub state: String,
    /// Why this resource's synchronisation failed, and `None` while nothing has
    /// failed.
    pub error: Option<crate::ops::types::ErrorDetails>,
    /// When this resource's synchronisation last changed.
    pub update_date_time: SystemTime,
}

impl SyncResource {
    /// The resource's modelled `SyncResourceStatus`: its stored state and, when
    /// its synchronisation has failed, why.
    ///
    /// The `error` structure is reported WHOLE, the way every other status of
    /// this service is (see [`SyncJob::status`]): a resource that has not failed
    /// carries an `ErrorDetails` with neither a code nor a message, so a client
    /// that reads `status.error` needs no null guard.
    pub fn status(&self) -> crate::ops::types::SyncResourceStatus {
        crate::ops::types::SyncResourceStatus {
            state: Some(self.state.clone()),
            error: Some(self.error.clone().unwrap_or_default()),
        }
    }
}

/// One stored metadata transfer job.
///
/// The job's `sources` and `destination` are kept as the modelled REQUEST
/// shapes the caller supplied — the same choice [`ComponentType`] makes for a
/// component type's definitions — so a read reports back exactly what was
/// persisted rather than a lossy projection of it.
#[derive(Debug, Clone)]
pub struct MetadataTransferJob {
    pub metadata_transfer_job_id: String,
    pub arn: String,
    /// The job's description, absent when the create named none: `description`
    /// is optional on `CreateMetadataTransferJobRequest` AND on
    /// `GetMetadataTransferJobResponse`, so it is carried as an option rather
    /// than resolved to the empty string, as a [`Scene`]'s is.
    pub description: Option<String>,
    /// The job's sources, in the order the create gave them
    /// (`SourceConfigurations` is a `@length(1..1)` LIST).
    pub sources: Vec<crate::ops::types::SourceConfiguration>,
    pub destination: crate::ops::types::DestinationConfiguration,
    /// The ARN of the role the job runs under
    /// ([`metadata_transfer_job_role`]): the service's own, since the create
    /// accepts no role.
    pub metadata_transfer_job_role: String,
    pub creation_date_time: SystemTime,
    /// When the job last changed — the create, until the clock settles its
    /// outcome, which moves it to the instant that outcome fell due.
    pub update_date_time: SystemTime,
    /// The job's `MetadataTransferJobState`, as STORED:
    /// [`METADATA_TRANSFER_JOB_STATE_VALIDATING`] from the create until
    /// [`settles_at`](Self::settles_at) passes, then the terminal state
    /// [`settle`](Self::settle) writes.
    pub state: String,
    /// Why the job is in a failed state, and `None` while nothing has failed.
    pub error: Option<crate::ops::types::ErrorDetails>,
    /// The instant the transition out of the job's current NON-TERMINAL state
    /// falls due — the instant of the create plus
    /// [`METADATA_TRANSFER_JOB_SETTLE_SPAN`] for the outcome a created job is
    /// owed, or the instant of the cancel plus
    /// [`METADATA_TRANSFER_JOB_CANCEL_SPAN`] once a cancellation has been
    /// accepted ([`begin_cancel`](Self::begin_cancel)). Stored, never slept
    /// for, so a reader can materialise a transition the clock already owed
    /// without any caller having had to poll.
    pub settles_at: SystemTime,
    /// The `MetadataTransferJobState` the transition due at
    /// [`settles_at`](Self::settles_at) puts the job in: the state the job's own
    /// RUN established ([`TransferOutcome::state`]) for the outcome a created
    /// job is owed, or [`METADATA_TRANSFER_JOB_STATE_CANCELLED`] once a
    /// cancellation has been accepted. It is read only while the job is
    /// non-terminal: a job that has settled owes nothing more.
    pub settles_to: String,
    /// What the job's RUN established: the state it settles in, its counts and
    /// its report ([`TransferOutcome`]).
    ///
    /// `None` only for a job whose run has not been performed yet — the window
    /// inside the accepting call, between the job being stored and its run
    /// coming back — so a reader that meets one reports the job as the
    /// non-terminal thing it still is.
    pub transfer: Option<TransferOutcome>,
}

impl MetadataTransferJob {
    /// The job a `CreateMetadataTransferJob` call accepted at `now`: accepted,
    /// its run not yet performed, and its outcome due
    /// [`METADATA_TRANSFER_JOB_SETTLE_SPAN`] later.
    pub fn accepted(
        metadata_transfer_job_id: String,
        arn: String,
        description: Option<String>,
        sources: Vec<crate::ops::types::SourceConfiguration>,
        destination: crate::ops::types::DestinationConfiguration,
        now: SystemTime,
    ) -> Self {
        Self {
            metadata_transfer_job_id,
            arn,
            description,
            sources,
            destination,
            metadata_transfer_job_role: metadata_transfer_job_role(),
            creation_date_time: now,
            // Nothing has changed the job yet, so it was last updated when it
            // was created.
            update_date_time: now,
            state: METADATA_TRANSFER_JOB_STATE_VALIDATING.to_string(),
            error: None,
            settles_at: now + METADATA_TRANSFER_JOB_SETTLE_SPAN,
            // Until the run comes back ([`record_transfer`](Self::record_transfer))
            // the outcome the clock owes is the modelled failure state: a job
            // whose transfer was never performed transferred nothing, so it may
            // not settle `COMPLETED`.
            settles_to: METADATA_TRANSFER_JOB_STATE_ERROR.to_string(),
            transfer: None,
        }
    }

    /// Record what the job's RUN established, as the outcome the clock owes it.
    ///
    /// Called by the accepting call once the transfer has been performed: the
    /// run decides which terminal state is due and what the job reports with it
    /// ([`TransferOutcome`]), and the instant it falls due is the one the create
    /// already stored, so recording it moves nothing a caller can observe
    /// early.
    ///
    /// A job that is no longer the one the run belongs to is left exactly as it
    /// is: a cancellation accepted while the run was in flight has already
    /// decided this job's outcome, and so has a settle that materialised one.
    pub fn record_transfer(&mut self, outcome: TransferOutcome) {
        if self.state != METADATA_TRANSFER_JOB_STATE_VALIDATING {
            return;
        }
        self.settles_to = outcome.state.clone();
        self.transfer = Some(outcome);
    }

    /// Whether this job has finished for good: see
    /// [`metadata_transfer_job_state_is_terminal`]. A job that has not is one
    /// the clock still owes the transition at [`settles_at`](Self::settles_at).
    pub fn is_terminal(&self) -> bool {
        metadata_transfer_job_state_is_terminal(&self.state)
    }

    /// Materialise the lifecycle transition the CLOCK has already made due.
    ///
    /// Called by a reader before it reports this job, exactly as
    /// [`ComponentType::settle`] is: the outcome was owed at
    /// [`settles_at`](Self::settles_at) whether or not anybody called, so
    /// writing it here makes the record say what was already true rather than
    /// changing anything a read caused.
    ///
    /// The outcome of a CREATED job is the one its RUN established
    /// ([`record_transfer`](Self::record_transfer)): `COMPLETED` for a run that
    /// read its source and wrote its destination, and the modelled failure
    /// state carrying the reason for one that could not
    /// ([`METADATA_TRANSFER_JOB_ERROR_CODE`]). The outcome of a job whose
    /// CANCELLATION was accepted is
    /// [`METADATA_TRANSFER_JOB_STATE_CANCELLED`], which carries no error:
    /// nothing failed, a caller asked the transfer to stop. Which of them is
    /// due is [`settles_to`](Self::settles_to). Idempotent, a no-op before the
    /// instant falls due, and a no-op on a job that has already settled.
    pub fn settle(&mut self, now: SystemTime) {
        if self.is_terminal() || now < self.settles_at {
            return;
        }
        self.state = self.settles_to.clone();
        if self.state == METADATA_TRANSFER_JOB_STATE_ERROR {
            // The reason is the run's own, and the service's own general one for
            // a job whose run never came back to report one.
            self.error = Some(
                self.transfer
                    .as_ref()
                    .and_then(|outcome| outcome.error.clone())
                    .unwrap_or_else(|| crate::ops::types::ErrorDetails {
                        code: Some(METADATA_TRANSFER_JOB_ERROR_CODE.to_string()),
                        message: Some(TRANSFER_NOT_PERFORMED_MESSAGE.to_string()),
                    }),
            );
        }
        // The change fell due at `settles_at`, not at the instant a caller
        // happened to ask, so that is when the job was last updated: two reads
        // of a settled job report the same instant.
        self.update_date_time = self.settles_at;
    }

    /// Accept a cancellation at `now`: the job leaves whichever non-terminal
    /// state it was in for the non-terminal
    /// [`METADATA_TRANSFER_JOB_STATE_CANCELLING`] — the teardown is in flight —
    /// and the instant the clock owes
    /// [`METADATA_TRANSFER_JOB_STATE_CANCELLED`] is stored beside it, never
    /// slept for. See [`settle`](Self::settle).
    ///
    /// A job that has FINISHED is left exactly as it is, and so is one whose
    /// cancellation the service has ALREADY accepted: the transition the clock
    /// already owed is materialised first, a job that is terminal once it has
    /// has nothing left to cancel, and a job already in
    /// [`METADATA_TRANSFER_JOB_STATE_CANCELLING`] is being torn down under the
    /// instant the clock already owes it — so neither that instant nor
    /// `updateDateTime` moves when the same cancellation arrives twice. Both
    /// rules live here rather than in the caller;
    /// `CancelMetadataTransferJob` tests terminality too, because that is what
    /// it answers the caller's `ConflictException` for.
    pub fn begin_cancel(&mut self, now: SystemTime) {
        self.settle(now);
        if self.is_terminal() || self.state == METADATA_TRANSFER_JOB_STATE_CANCELLING {
            return;
        }
        self.state = METADATA_TRANSFER_JOB_STATE_CANCELLING.to_string();
        self.update_date_time = now;
        self.settles_at = now + METADATA_TRANSFER_JOB_CANCEL_SPAN;
        self.settles_to = METADATA_TRANSFER_JOB_STATE_CANCELLED.to_string();
    }

    /// The job's modelled `MetadataTransferJobStatus`: its stored state and,
    /// when it has failed, why.
    ///
    /// The `error` structure is reported WHOLE, the way every other status of
    /// this service is (see [`status`]): a job that has not failed carries an
    /// `ErrorDetails` with neither a code nor a message, so a client that reads
    /// `status.error` needs no null guard.
    ///
    /// `queuedPosition` is left absent: it is a job's place in the queue of
    /// jobs waiting to run, and this emulator queues no transfer behind
    /// another, so there is no position to report.
    pub fn status(&self) -> crate::ops::types::MetadataTransferJobStatus {
        crate::ops::types::MetadataTransferJobStatus {
            state: Some(self.state.clone()),
            error: Some(self.error.clone().unwrap_or_default()),
            queued_position: None,
        }
    }

    /// The job's modelled `MetadataTransferJobProgress`: how many entries its
    /// run moved, and what became of each.
    ///
    /// Reported once the job has SETTLED in the outcome its own run established
    /// and not before: the counts are part of that outcome, which falls due at
    /// [`settles_at`](Self::settles_at), so a job still in flight reports none —
    /// and neither does one a caller CANCELLED, whose run was abandoned rather
    /// than reported. A run that never got as far as walking an entry (one whose
    /// source could not be read) counts nothing, so it reports nothing either.
    pub fn progress(&self) -> Option<crate::ops::types::MetadataTransferJobProgress> {
        self.settled_transfer()?.progress.clone()
    }

    /// Where the job's run wrote its report, once that outcome has fallen due.
    ///
    /// Reported on the same rule [`progress`](Self::progress) is reported on: a
    /// report is part of the outcome of a run, and `None` where no object store
    /// answered the write, since a URL no caller could fetch is worse than
    /// none.
    pub fn report_url(&self) -> Option<String> {
        self.settled_transfer()?.report_url.clone()
    }

    /// The job's run, once the job is settled in the outcome that run
    /// established — and `None` for a job that is still in flight or that ended
    /// some other way (a cancellation).
    fn settled_transfer(&self) -> Option<&TransferOutcome> {
        let transfer = self.transfer.as_ref()?;
        (self.state == transfer.state).then_some(transfer)
    }
}

/// One stored entity.
#[derive(Debug, Clone)]
pub struct Entity {
    pub entity_id: String,
    pub entity_name: String,
    pub arn: String,
    pub workspace_id: String,
    /// The entity's description, the empty string when the create named none.
    ///
    /// A description is a member every entity carries rather than one it may
    /// lack: `Description` admits the empty string (`@length 0..2048`), and a
    /// read reports the member for an entity whose description is empty instead
    /// of omitting it, so a client that reads it back needs no null guard.
    pub description: String,
    /// [`ROOT_PARENT_ENTITY_ID`] when the entity was created without a parent.
    pub parent_entity_id: String,
    /// The entity's components, keyed by component name.
    pub components: BTreeMap<String, ComponentRecord>,
    pub tags: BTreeMap<String, String>,
    pub creation_date_time: SystemTime,
    pub update_date_time: SystemTime,
    /// The entity's `State`, as the WRITER that last touched the record left
    /// it: [`STATE_ACTIVE`] from a create, which settles the entity inside the
    /// call that asked for it (see that constant), or [`STATE_UPDATING`] from
    /// an update, until [`settles_at`](Self::settles_at) passes.
    pub state: String,
    /// The instant the transition out of [`STATE_UPDATING`] falls due — the
    /// instant of the call that opened the window plus [`ENTITY_SETTLE_SPAN`].
    /// Stored, never slept for, so a read reports the transition the clock
    /// already owed without any caller having had to poll. Irrelevant while the
    /// stored state is terminal, which is how a create leaves it.
    pub settles_at: SystemTime,
}

impl Entity {
    /// The entity's `State` AS OF `now`: the stored state, with the transition
    /// the CLOCK has already made due applied.
    ///
    /// The `UPDATING` -> `ACTIVE` transition was owed at
    /// [`settles_at`](Self::settles_at) whether or not anybody called, so
    /// reporting it is saying what was already true. Computed rather than
    /// written back, so a read of an entity leaves the store exactly as it
    /// found it.
    pub fn state_at(&self, now: SystemTime) -> &str {
        if self.state == STATE_UPDATING && now >= self.settles_at {
            STATE_ACTIVE
        } else {
            &self.state
        }
    }

    /// The entity's modelled `Status` as of `now`: its state
    /// ([`state_at`](Self::state_at)), with the `error` structure every reported
    /// status carries. See [`status`].
    pub fn status(&self, now: SystemTime) -> crate::ops::types::Status {
        status(self.state_at(now))
    }

    /// Open the window an UPDATE owes.
    ///
    /// The update has landed on the record by the time this is called, so its
    /// `updateDateTime` moves to the instant of the call that made the change;
    /// its state leaves [`STATE_ACTIVE`] for the non-terminal
    /// [`STATE_UPDATING`] the update answers with, and the instant the clock
    /// owes the return to [`STATE_ACTIVE`] is stored beside it — never slept
    /// for. See [`state_at`](Self::state_at).
    pub fn begin_update(&mut self, now: SystemTime) {
        self.update_date_time = now;
        self.state = STATE_UPDATING.to_string();
        self.settles_at = now + ENTITY_SETTLE_SPAN;
    }

    /// Every component the entity holds: each top-level component, and every
    /// composite component stored inside one of them, at whatever depth.
    ///
    /// A composite component is a component of the entity too — it instantiates
    /// a component type of its own and carries properties of its own, and
    /// `GetEntity` and `ListComponents` both report it as a component — so any
    /// reader asking "which components does this entity hold" walks all of them
    /// through here rather than stopping at
    /// [`components`](Self::components). Each is handed back as the
    /// [`ComponentAtPath`] a `componentPath` would have resolved to, so the two
    /// kinds answer the same questions ([`ComponentAtPath::component_type_id`],
    /// [`ComponentAtPath::properties`]) while staying distinguishable by a
    /// caller that reports them differently.
    pub fn components_at_every_path(&self) -> impl Iterator<Item = ComponentAtPath<'_>> {
        self.components.values().flat_map(|component| {
            ::std::iter::once(ComponentAtPath::TopLevel(component)).chain(
                component
                    .composite_components
                    .iter()
                    .map(move |(composite_path, composite)| ComponentAtPath::Composite {
                        component,
                        composite_path,
                        composite,
                    }),
            )
        })
    }
}

/// One stored component type.
///
/// A component type's property definitions, functions and composite component
/// types are kept as the modelled REQUEST shapes the caller supplied — the same
/// choice [`ComponentRecord`] makes for an entity's properties — so a read
/// reports back exactly what was persisted rather than a lossy projection of
/// it.
#[derive(Debug, Clone)]
pub struct ComponentType {
    pub component_type_id: String,
    pub arn: String,
    pub workspace_id: String,
    /// The friendly name the create supplied, absent when it named none.
    pub component_type_name: Option<String>,
    /// The description the create supplied, absent when it named none.
    pub description: Option<String>,
    /// "Whether an entity can have more than one component of this type" — the
    /// member's own documentation. Stored resolved: a create that names none
    /// leaves it `false`.
    pub is_singleton: bool,
    /// "Whether the component type is abstract" — a base type an entity holds
    /// no component of, which exists to be extended.
    ///
    /// STORED rather than derived at each reader, so the value
    /// `GetComponentType` reports, the set a `ListComponentTypes` `isAbstract`
    /// filter selects and the component types `CreateEntity` refuses to
    /// instantiate cannot drift apart. `isAbstract` is not a member of
    /// `CreateComponentTypeRequest` — a caller does not declare it — so a
    /// writer DECIDES it from what the request declares
    /// ([`component_type_is_abstract`]) and stores the answer beside the
    /// record; the service's own built-ins carry the abstractness the service
    /// publishes for them, which the workspace is provisioned with rather than
    /// asked for ([`built_in_component_types`]).
    pub is_abstract: bool,
    /// The parent component types this one extends, in the order the create
    /// gave them.
    pub extends_from: Vec<String>,
    pub property_definitions: BTreeMap<String, crate::ops::types::PropertyDefinitionRequest>,
    pub functions: BTreeMap<String, crate::ops::types::FunctionRequest>,
    pub property_groups: BTreeMap<String, PropertyGroupRecord>,
    pub composite_component_types:
        BTreeMap<String, crate::ops::types::CompositeComponentTypeRequest>,
    pub tags: BTreeMap<String, String>,
    pub creation_date_time: SystemTime,
    pub update_date_time: SystemTime,
    /// The component type's `State`, as STORED: [`STATE_CREATING`] from the
    /// create (or [`STATE_UPDATING`] from an update) until
    /// [`settles_at`](Self::settles_at) passes, then [`STATE_ACTIVE`].
    pub state: String,
    /// The instant the transition out of [`STATE_CREATING`] or
    /// [`STATE_UPDATING`] falls due — the instant of the call that opened the
    /// window plus [`COMPONENT_TYPE_SETTLE_SPAN`]. Stored, never slept for, so
    /// a reader can materialise a transition the clock already owed without any
    /// caller having had to poll.
    pub settles_at: SystemTime,
}

impl ComponentType {
    /// Materialise the lifecycle transition the CLOCK has already made due.
    ///
    /// Called by a reader before it reports this component type: the
    /// `CREATING` -> `ACTIVE` (or `UPDATING` -> `ACTIVE`) transition was owed at
    /// [`settles_at`](Self::settles_at) whether or not anybody called, so
    /// writing it here is making the record say what was already true, not a
    /// change a read caused. Idempotent, and a no-op before the instant falls
    /// due.
    pub fn settle(&mut self, now: SystemTime) {
        let in_flight = self.state == STATE_CREATING || self.state == STATE_UPDATING;
        if in_flight && now >= self.settles_at {
            self.state = STATE_ACTIVE.to_string();
        }
    }

    /// Open the provisioning window an UPDATE owes.
    ///
    /// The update has landed in the store by the time this is called, so the
    /// record's `updateDateTime` moves to the instant of the call that made the
    /// change; its state leaves [`STATE_ACTIVE`] for the non-terminal
    /// [`STATE_UPDATING`] the update answers with, and the instant the clock
    /// owes the return to [`STATE_ACTIVE`] is stored beside it — the same span a
    /// create opens, and never slept for. See [`settle`](Self::settle).
    pub fn begin_update(&mut self, now: SystemTime) {
        self.update_date_time = now;
        self.state = STATE_UPDATING.to_string();
        self.settles_at = now + COMPONENT_TYPE_SETTLE_SPAN;
    }

    /// The component type's modelled `Status`: its stored state plus the
    /// `error` structure every reported status carries. See [`status`].
    pub fn status(&self) -> crate::ops::types::Status {
        status(&self.state)
    }

    /// Whether the component type "has a schema initializer and the schema
    /// initializer has run" (the `isSchemaInitialized` member's own
    /// documentation): it has one when the `functions` it EFFECTIVELY has carry
    /// [`SCHEMA_INITIALIZER_FUNCTION`] — its own, or one a component type on its
    /// `extendsFrom` chain declares and it never restated
    /// ([`Workspace::inherited_component_types`], which is what the caller
    /// passes as `inherited`) — and that initializer has run once the component
    /// type has settled, which is exactly when the stored state is
    /// [`STATE_ACTIVE`]: neither a create nor an update still in flight has
    /// finished running it.
    pub fn is_schema_initialized(&self, inherited: &[&ComponentType]) -> bool {
        let has_initializer = std::iter::once(self)
            .chain(inherited.iter().copied())
            .any(|component_type| {
                component_type
                    .functions
                    .contains_key(SCHEMA_INITIALIZER_FUNCTION)
            });
        has_initializer && self.state == STATE_ACTIVE
    }
}

/// Whether a `MetadataTransferJobState` is TERMINAL — a state the job has
/// finished in and never leaves, so there is nothing left for a cancellation to
/// stop.
///
/// The model's `MetadataTransferJobState` enum
/// (`ops::types::METADATA_TRANSFER_JOB_STATE_VALUES`) holds three such values:
/// the transfer ran to the end (`COMPLETED`), it was cancelled (`CANCELLED`),
/// or it failed (`ERROR`). Every other value is a state the job is still
/// moving through.
pub fn metadata_transfer_job_state_is_terminal(state: &str) -> bool {
    matches!(
        state,
        METADATA_TRANSFER_JOB_STATE_COMPLETED
            | METADATA_TRANSFER_JOB_STATE_CANCELLED
            | METADATA_TRANSFER_JOB_STATE_ERROR
    )
}

/// The modelled `Status` of a resource in `state`.
///
/// Both of `Status`'s members are optional in the model, but the service
/// reports the structure WHOLE: `error` is a present `ErrorDetails` — carrying
/// neither a code nor a message while nothing has failed — rather than an
/// absent member, so a client that reads `status.error` without a null guard
/// sees an empty structure instead of nothing. A failed resource is the same
/// structure with its `code`/`message` filled in.
pub fn status(state: &str) -> crate::ops::types::Status {
    crate::ops::types::Status {
        state: Some(state.to_string()),
        error: Some(crate::ops::types::ErrorDetails {
            code: None,
            message: None,
        }),
    }
}

/// The single value the model's `GroupType` enum defines
/// (`ops::types::GROUP_TYPE_VALUES`).
///
/// `groupType` is required on every property-group RESPONSE but optional on the
/// request, so a group stored without one is reported with the only value the
/// model admits rather than with an invented string.
pub const TABULAR_GROUP_TYPE: &str = "TABULAR";

/// One stored property group of a COMPONENT as the modelled
/// `ComponentPropertyGroupResponse`.
///
/// Shared by every reader of a component's property groups (`GetEntity`,
/// `ListComponents`), so one group is reported the same way wherever it is read.
/// `groupType` is required on the response but optional on the request, so a
/// group stored without one reports [`TABULAR_GROUP_TYPE`].
///
/// `is_inherited` is what the caller resolved the group through
/// ([`Workspace::property_group_is_inherited`]): a group that reaches the
/// component through the component type it instantiates is inherited, and one
/// no component type on that chain declares is the component's own. A component
/// TYPE's own groups are a different modelled shape (`PropertyGroupResponse`),
/// which `GetComponentType` builds.
pub fn component_property_group_response(
    group: &PropertyGroupRecord,
    is_inherited: bool,
) -> crate::ops::types::ComponentPropertyGroupResponse {
    crate::ops::types::ComponentPropertyGroupResponse {
        group_type: group
            .group_type
            .clone()
            .unwrap_or_else(|| TABULAR_GROUP_TYPE.to_string()),
        property_names: group.property_names.clone(),
        is_inherited,
    }
}

/// Every property group one component HAS, as the modelled map a reader of a
/// component reports: the groups the component declares itself plus the ones
/// the component type it instantiates — or a type that type extends — declares
/// and it never restated
/// ([`Workspace::component_property_group_names`]), each resolved through
/// [`Workspace::effective_property_group`] and projected by
/// [`component_property_group_response`].
///
/// The ONE projection every reader of a component's property groups shares
/// (`GetEntity` for a component and for a composite component,
/// `ListComponents` for both), so the readers cannot drift on which groups a
/// component has or on which of them is inherited.
pub fn component_property_groups_response(
    workspace: &Workspace,
    component_type_id: Option<&str>,
    property_groups: &BTreeMap<String, PropertyGroupRecord>,
) -> std::collections::HashMap<String, crate::ops::types::ComponentPropertyGroupResponse> {
    workspace
        .component_property_group_names(component_type_id, property_groups)
        .into_iter()
        .filter_map(|group_name| {
            let group =
                workspace.effective_property_group(component_type_id, property_groups, &group_name)?;
            let response = component_property_group_response(
                &group,
                workspace.property_group_is_inherited(component_type_id, &group_name),
            );
            Some((group_name, response))
        })
        .collect()
}

/// How a writer refuses a property definition
/// [`Workspace::stranded_property_definition`] names, so a create and an update
/// word one rule one way — and the same way the component-level writers word
/// theirs.
pub fn property_definition_needs_data_type_message(property_name: &str) -> String {
    format!("propertyDefinitions.{property_name}.dataType is required")
}

/// The names under which a component type's `functions` map carries a DATA
/// CONNECTOR — the function that fetches, or writes, the values of a property
/// whose values live outside the service.
///
/// The map's keys are free-form in the model (`functions` is a
/// `map<String, FunctionRequest>`), so these are the names the AWS IoT
/// TwinMaker user guide publishes for a component type's data connectors:
/// a reader at entity scope, a reader at component-type scope, the plain
/// reader, and the writer. The schema initializer
/// ([`SCHEMA_INITIALIZER_FUNCTION`]) is NOT one of them: it initializes the
/// component type's schema and serves no property's values.
pub const DATA_CONNECTOR_FUNCTION_NAMES: &[&str] = &[
    "dataReader",
    "dataReaderByEntity",
    "dataReaderByComponentType",
    DATA_WRITER_FUNCTION,
];

/// The name under which a component type's `functions` map carries the data
/// connector that WRITES the values of its properties — the one of
/// [`DATA_CONNECTOR_FUNCTION_NAMES`] the time-series write path needs, since the
/// values of a component type's properties are set THROUGH it.
pub const DATA_WRITER_FUNCTION: &str = "dataWriter";

/// Whether the component type `component_type_id` has a WRITE connector — a
/// [`DATA_WRITER_FUNCTION`] entry in its `functions`, its own or one it inherits
/// from a component type it extends ([`Workspace::component_type_chain`]).
///
/// A component type that declares none has no way to set the values of its
/// properties, so a write addressed to one of them is refused rather than
/// recorded. `None` — a component instantiating no component type at all —
/// declares no connector either.
pub fn component_type_has_data_writer(
    workspace: &Workspace,
    component_type_id: Option<&str>,
) -> bool {
    component_type_id.is_some_and(|component_type_id| {
        workspace
            .component_type_chain(component_type_id)
            .iter()
            .any(|component_type| component_type.functions.contains_key(DATA_WRITER_FUNCTION))
    })
}

/// The message a refusal carries for a write to a property whose component type
/// declares no write connector ([`component_type_has_data_writer`]).
///
/// Built from the request and from stored state: the property and the component
/// the write addressed, and the connector that would have to serve it — naming
/// the connector is what tells the caller the component type, not the request,
/// is what has to change.
///
/// Only that NAMING is established for this class: the service's own refusal
/// restates the connector request it could not make, of which the connector's
/// name is the one part that is a property of the request rather than of one
/// invocation. The rest of the sentence is therefore this service's own wording,
/// and a later reading of the refusal can tighten it without changing which
/// requests earn it.
pub fn missing_data_writer_message(
    workspace_id: &str,
    entity_id: &str,
    component: &str,
    property_name: &str,
) -> String {
    format!(
        "No {DATA_WRITER_FUNCTION} connector is defined for component {component} of Entity \
         {entity_id} in workspace {workspace_id}, so the values of property {property_name} \
         cannot be set."
    )
}

/// Whether `definition` declares a property whose values live OUTSIDE this
/// service — `isStoredExternally`, "a Boolean value that specifies whether the
/// property is stored externally" (the member's own documentation), resolved so
/// an absent member reads as `false`.
pub fn property_is_stored_externally(
    definition: &crate::ops::types::PropertyDefinitionRequest,
) -> bool {
    definition.is_stored_externally.unwrap_or(false)
}

/// Whether a component type is ABSTRACT — whether it is a base type that exists
/// to be extended rather than instantiated, which is what `isAbstract`, "a
/// Boolean value that specifies whether the component type is abstract",
/// reports.
///
/// A component type is abstract exactly when it has no way to serve the values
/// of some property it declares: it carries an externally stored property
/// ([`property_is_stored_externally`]) and no data connector
/// ([`DATA_CONNECTOR_FUNCTION_NAMES`]) that could read or write one. Such a
/// type is incomplete on its own — a type extending it supplies the connector —
/// so no component instantiates it.
///
/// Both halves are resolved over the record AND the inheritance it declares
/// (`extendsFrom`, walked by [`Workspace::component_type_chain`]): a property
/// inherited from a parent is as much the type's as one it restates, and a
/// connector inherited from a parent serves it just as well. `record` is passed
/// in rather than looked up, so the rule holds for a component type a create
/// has not stored yet and for one an update has patched on a copy.
pub fn component_type_is_abstract(workspace: &Workspace, record: &ComponentType) -> bool {
    let inherited = record
        .extends_from
        .iter()
        .flat_map(|parent| workspace.component_type_chain(parent));
    let mut stores_externally = false;
    let mut has_connector = false;
    for component_type in std::iter::once(record).chain(inherited) {
        stores_externally |= component_type
            .property_definitions
            .values()
            .any(property_is_stored_externally);
        has_connector |= component_type
            .functions
            .keys()
            .any(|function_name| DATA_CONNECTOR_FUNCTION_NAMES.contains(&function_name.as_str()));
    }
    stores_externally && !has_connector
}

/// Re-decide the stored abstractness of the component type `component_type_id`
/// of `workspace` and of every component type that INHERITS from it
/// ([`component_type_is_abstract`]).
///
/// Abstractness is decided over a record AND the inheritance it declares, so a
/// write that changes one component type's properties, functions or parents
/// changes the answer for every component type whose chain reaches it, not only
/// for the one addressed: a parent that gains a data connector makes each of its
/// descendants servable, and one that loses it makes them abstract again. Those
/// descendants are exactly the ids
/// [`component_type_instance_ids`](Workspace::component_type_instance_ids)
/// resolves, in the one reverse-edge pass that method already makes, so a write
/// re-decides the records it can have moved and leaves the rest of the workspace
/// untouched. Each answer is computed from the OTHER records' definitions,
/// functions and parents rather than from their stored abstractness, so the pass
/// needs no ordering and one sweep settles a chain of any depth.
///
/// A BUILT-IN is skipped: it carries the abstractness the service publishes for
/// it ([`built_in_component_types`]), which is not derived from its definitions,
/// and no caller's write reaches one.
pub fn redecide_component_type_abstractness(workspace: &mut Workspace, component_type_id: &str) {
    let affected: Vec<String> = workspace
        .component_type_instance_ids(component_type_id)
        .into_iter()
        .filter(|id| !built_in_component_type_id(id))
        .map(str::to_string)
        .collect();
    let decided: Vec<(String, bool)> = affected
        .into_iter()
        .filter_map(|id| {
            let record = workspace.component_types.get(&id)?;
            let is_abstract = component_type_is_abstract(workspace, record);
            Some((id, is_abstract))
        })
        .collect();
    for (component_type_id, is_abstract) in decided {
        if let Some(record) = workspace.component_types.get_mut(&component_type_id) {
            record.is_abstract = is_abstract;
        }
    }
}

/// Why a component of the component type `component_type_id` cannot be created,
/// as the message a `ValidationException` carries: the component type is
/// ABSTRACT ([`component_type_is_abstract`]), so it is extended rather than
/// instantiated. Built from the request — it names the component type the call
/// asked for — so a caller is told which one to change.
pub fn abstract_component_type_message(component_type_id: &str) -> String {
    format!("Abstract ComponentType {component_type_id} can not be used for creating a component")
}

/// Why a component instantiating `component_type_id` cannot be written — `None`
/// when it can — as the message a `ValidationException` carries.
///
/// The component type is resolved in the WORKSPACE's own registry, which holds
/// both the component types a caller created and the built-ins the workspace was
/// provisioned with. A component INSTANTIATES the type its `componentTypeId`
/// names, so that type has to be one that can be instantiated at all: an
/// ABSTRACT one cannot, which is what makes it a base type to extend rather than
/// one to hold a component of ([`abstract_component_type_message`]). An id the
/// workspace does not hold is left alone — a real workspace carries more
/// built-in component types than this emulator models, so refusing one would
/// reject writes the real service accepts — and a component that names no
/// component type instantiates nothing and so has nothing to resolve.
///
/// Shared by every writer of an entity's components, top-level and composite
/// alike, so a create and an update hold one path to one rule.
pub fn abstract_component_type_violation(
    workspace: &Workspace,
    component_type_id: Option<&str>,
) -> Option<String> {
    let component_type_id = component_type_id?;
    let component_type = workspace.component_types.get(component_type_id)?;
    component_type
        .is_abstract
        .then(|| abstract_component_type_message(component_type_id))
}

/// The id prefix the service's own BUILT-IN component types carry.
///
/// IoT TwinMaker publishes the component types it ships under the reserved
/// `com.amazon.` namespace — `com.amazon.iottwinmaker.alarm.basic`,
/// `com.amazon.iotsitewise.connector` and the rest of the user guide's
/// "built-in component types" — while a caller's own ids live outside it. The
/// namespace is therefore the SERVICE's: the component types in it are
/// provisioned with every workspace ([`built_in_component_types`]) and no
/// caller creates, changes or deletes one. An id inside it that this emulator
/// does not model still RESOLVES as a reference rather than being refused: the
/// real service ships more built-ins than are modelled here, and refusing one
/// would reject inheritance the real service accepts, which is the opposite
/// error from the dangling reference that rule exists to catch.
pub const BUILT_IN_COMPONENT_TYPE_ID_PREFIX: &str = "com.amazon.";

/// Whether `component_type_id` names one of the service's built-in component
/// types. See [`BUILT_IN_COMPONENT_TYPE_ID_PREFIX`].
pub fn built_in_component_type_id(component_type_id: &str) -> bool {
    component_type_id.starts_with(BUILT_IN_COMPONENT_TYPE_ID_PREFIX)
}

/// The `Type` values ([`crate::ops::types::TYPE_VALUES`]) the built-in
/// catalogue's property definitions carry.
const TYPE_STRING: &str = "STRING";
/// See [`TYPE_STRING`].
const TYPE_MAP: &str = "MAP";

/// One entry of the built-in catalogue: a component type IoT TwinMaker SHIPS,
/// as this emulator models it. See [`built_in_component_types`].
struct BuiltInComponentType {
    /// Its published `componentTypeId`, inside
    /// [`BUILT_IN_COMPONENT_TYPE_ID_PREFIX`].
    component_type_id: &'static str,
    /// Whether it is abstract — see [`ComponentType::is_abstract`]. The
    /// abstractness the SERVICE publishes for the built-in, carried as declared
    /// rather than decided by [`component_type_is_abstract`]: a built-in is
    /// provisioned with the workspace, and no caller can create or update one
    /// in the reserved namespace, so no writer of a component type's
    /// definitions or functions ever reaches a built-in's record.
    is_abstract: bool,
    /// The property definitions it declares itself, which a component type
    /// extending it inherits.
    property_definitions: &'static [BuiltInProperty],
}

/// One property definition of a built-in component type.
struct BuiltInProperty {
    /// The property's name, as the service publishes it.
    name: &'static str,
    /// Its `dataType`'s `type`: a [`crate::ops::types::TYPE_VALUES`] value.
    data_type: &'static str,
    /// The `nestedType`'s `type` for a container `data_type`, `None` for a
    /// scalar one.
    nested_type: Option<&'static str>,
    /// Whether the property "ID comes from an external data store"
    /// (`isExternalId`): what names the resource of the linked service the
    /// built-in connects a component to.
    is_external_id: bool,
    /// Whether the property "consists of time series data" (`isTimeSeries`).
    is_time_series: bool,
}

impl BuiltInProperty {
    /// This declaration as the modelled `PropertyDefinitionRequest` a stored
    /// component type carries, so a built-in's definitions are stored and read
    /// back exactly like a created one's.
    fn definition(&self) -> crate::ops::types::PropertyDefinitionRequest {
        crate::ops::types::PropertyDefinitionRequest {
            data_type: Some(crate::ops::types::DataType {
                r#type: self.data_type.to_string(),
                nested_type: self.nested_type.map(|nested| {
                    Box::new(crate::ops::types::DataType {
                        r#type: nested.to_string(),
                        ..Default::default()
                    })
                }),
                ..Default::default()
            }),
            is_external_id: Some(self.is_external_id),
            is_time_series: Some(self.is_time_series),
            ..Default::default()
        }
    }
}

/// The built-in component types this emulator models, from the AWS IoT
/// TwinMaker user guide's "built-in component types": the IoT SiteWise
/// connector a workspace's own types extend to read an asset's property values,
/// the basic alarm type the service's alarm pattern extends, and the document
/// type an entity carries document links in.
///
/// The service ships these; nothing creates them through the API, which is why
/// [`built_in_component_types`] provisions a workspace with them and
/// `CreateComponentType` refuses the namespace they live in. The set is the one
/// the user guide publishes and this emulator models — an id inside the
/// reserved namespace that is NOT here still resolves as a reference (see
/// [`BUILT_IN_COMPONENT_TYPE_ID_PREFIX`]) rather than being refused.
const BUILT_IN_COMPONENT_TYPES: &[BuiltInComponentType] = &[
    // The IoT SiteWise connector. ABSTRACT: a workspace's own component type
    // extends it and the entity's component then names the SiteWise asset and
    // asset model the component reads its values from, so the connector itself
    // is never instantiated. Its two properties are those external ids.
    BuiltInComponentType {
        component_type_id: "com.amazon.iotsitewise.connector",
        is_abstract: true,
        property_definitions: &[
            BuiltInProperty {
                name: "sitewiseAssetModelId",
                data_type: TYPE_STRING,
                nested_type: None,
                is_external_id: true,
                is_time_series: false,
            },
            BuiltInProperty {
                name: "sitewiseAssetId",
                data_type: TYPE_STRING,
                nested_type: None,
                is_external_id: true,
                is_time_series: false,
            },
        ],
    },
    // The basic alarm. ABSTRACT: it is the parent the service's documented
    // alarm pattern declares in `extendsFrom`, so a workspace's own alarm type
    // inherits the alarm key that names the alarm in the system that raises it
    // and the alarm status that system reports over time.
    BuiltInComponentType {
        component_type_id: "com.amazon.iottwinmaker.alarm.basic",
        is_abstract: true,
        property_definitions: &[
            BuiltInProperty {
                name: "alarm_key",
                data_type: TYPE_STRING,
                nested_type: None,
                is_external_id: true,
                is_time_series: false,
            },
            BuiltInProperty {
                name: "alarm_status",
                data_type: TYPE_STRING,
                nested_type: None,
                is_external_id: false,
                is_time_series: true,
            },
        ],
    },
    // The document type. CONCRETE: an entity holds a component of it directly,
    // whose one property maps each document's name to its location.
    BuiltInComponentType {
        component_type_id: "com.amazon.iottwinmaker.documents",
        is_abstract: false,
        property_definitions: &[BuiltInProperty {
            name: "documents",
            data_type: TYPE_MAP,
            nested_type: Some(TYPE_STRING),
            is_external_id: false,
            is_time_series: false,
        }],
    },
];

/// The built-in component types a workspace created in `region` under
/// `workspace_id` at `at` is PROVISIONED with, keyed by `componentTypeId` — the
/// value `CreateWorkspace` stores in [`Workspace::component_types`].
///
/// The service's own component types are not created through this API: they
/// exist in a workspace from the instant it does, which is why a workspace is
/// seeded with them rather than a reader synthesising them. Each is stored as
/// an ordinary [`ComponentType`] record — same ARN minting, same reported
/// members — so `GetComponentType`, `ListComponentTypes` and every walk of an
/// inheritance chain reach a built-in exactly as they reach a created one, and
/// a component type that extends one really has a parent whose definitions can
/// be read.
///
/// Each record is ACTIVE from that same instant, with no transition owed: a
/// provisioned component type has no create in flight for the clock to settle
/// ([`ComponentType::settle`] is a no-op on it). It carries no tags and no
/// friendly name or description — this emulator does not invent the service's
/// prose — and its ARN is deliberately NOT put in
/// [`RegionalState::arn_index`], for the same
/// reason a metadata transfer job's is not: that index resolves an id-or-ARN
/// input and finds a TAGGABLE resource's tag map, and a built-in is the
/// service's own resource, which carries no caller's tags.
pub fn built_in_component_types(
    region: &str,
    workspace_id: &str,
    at: SystemTime,
) -> BTreeMap<String, ComponentType> {
    BUILT_IN_COMPONENT_TYPES
        .iter()
        .map(|built_in| {
            (
                built_in.component_type_id.to_string(),
                ComponentType {
                    component_type_id: built_in.component_type_id.to_string(),
                    arn: component_type_arn(region, workspace_id, built_in.component_type_id),
                    workspace_id: workspace_id.to_string(),
                    component_type_name: None,
                    description: None,
                    is_singleton: false,
                    is_abstract: built_in.is_abstract,
                    extends_from: Vec::new(),
                    property_definitions: built_in
                        .property_definitions
                        .iter()
                        .map(|property| (property.name.to_string(), property.definition()))
                        .collect(),
                    functions: BTreeMap::new(),
                    property_groups: BTreeMap::new(),
                    composite_component_types: BTreeMap::new(),
                    tags: BTreeMap::new(),
                    creation_date_time: at,
                    update_date_time: at,
                    state: STATE_ACTIVE.to_string(),
                    settles_at: at,
                },
            )
        })
        .collect()
}

/// Why a `componentTypeId` a caller asked to create cannot be minted, as the
/// message a `ValidationException` carries: it lies in the namespace the
/// service reserves for the component types it ships.
///
/// Built from the request — it names the id and the namespace — so a caller is
/// told what to change. See [`BUILT_IN_COMPONENT_TYPE_ID_PREFIX`].
pub fn reserved_component_type_namespace_message(component_type_id: &str) -> String {
    format!(
        "componentTypeId {component_type_id} lies in the \
         {BUILT_IN_COMPONENT_TYPE_ID_PREFIX} namespace, which is reserved for the service's \
         built-in component types"
    )
}

/// Why a write addressed to a stored built-in component type cannot be
/// performed, as the message a `ValidationException` carries: the component
/// type is the service's own, provisioned with the workspace, so no caller's
/// update or delete applies to it. Built from the request, naming what was
/// addressed and where.
pub fn built_in_component_type_not_writable_message(
    workspace_id: &str,
    component_type_id: &str,
) -> String {
    format!(
        "ComponentType {component_type_id} in workspace {workspace_id} is a built-in component \
         type of the service and cannot be changed or deleted."
    )
}

/// Why `extends_from` cannot be stored as the inheritance of
/// `component_type_id`, as the message a `ValidationException` carries — `None`
/// when it can.
///
/// A component type's declared inheritance has to be DESCRIBABLE: a read
/// reports each property definition's required `dataType` and `isInherited`,
/// which is only answerable when the service holds every component type the
/// chain names, and an id that reaches back to the component type itself is an
/// inheritance cycle no chain can be walked up. Shared by every writer of a
/// component type's `extendsFrom`, so one declaration meets one rule wherever
/// it is written.
pub fn extends_from_violation(
    workspace: &Workspace,
    component_type_id: &str,
    extends_from: &[String],
) -> Option<String> {
    for parent in extends_from {
        // The self-reference check comes FIRST, and so gets its own message
        // whether or not the id is stored: a writer that mints the component
        // type (a create) has not stored it yet, so resolving first would tell
        // such a caller its parent "does not exist" rather than that a
        // component type cannot extend itself.
        if parent == component_type_id {
            return Some(format!(
                "extendsFrom names component type {component_type_id} itself; a component type \
                 cannot extend itself"
            ));
        }
        if !workspace.resolves_component_type(parent) {
            return Some(format!(
                "extendsFrom names component type {parent}, which does not exist in workspace {}",
                workspace.workspace_id
            ));
        }
    }
    let (ancestors, _) = workspace.ancestor_component_type_ids(extends_from);
    if ancestors.contains(component_type_id) {
        return Some(format!(
            "extendsFrom names a component type that already extends \
             {component_type_id}; inheritance cannot form a cycle"
        ));
    }
    None
}

/// Why these `compositeComponentTypes` cannot be stored on the component type
/// `component_type_id`, as the message a `ValidationException` carries — `None`
/// when they can.
///
/// A composite component type IS the component type its `componentTypeId`
/// refers to, so the service has to hold that one for the composition to mean
/// anything; an entry that names none declares no reference to resolve.
/// Shared by every writer of a component type's `compositeComponentTypes`.
///
/// An entry naming `component_type_id` itself RESOLVES: the write that carries
/// it is the write that leaves the workspace holding that component type, so
/// the reference is not a dangling one — and this way a create and an update
/// carrying the same entry agree, instead of the create refusing what an
/// update of the stored record would accept.
pub fn composite_component_types_violation(
    workspace: &Workspace,
    component_type_id: &str,
    composite_component_types: &BTreeMap<String, crate::ops::types::CompositeComponentTypeRequest>,
) -> Option<String> {
    for (composite_name, composite) in composite_component_types {
        let Some(referenced) = composite.component_type_id.as_deref() else {
            continue;
        };
        if referenced == component_type_id || workspace.resolves_component_type(referenced) {
            continue;
        }
        return Some(format!(
            "compositeComponentTypes.{composite_name}.componentTypeId names component type \
             {referenced}, which does not exist in workspace {}",
            workspace.workspace_id
        ));
    }
    None
}

/// Why `record`'s `propertyGroups` cannot be stored, as the message a
/// `ValidationException` carries — `None` when they can.
///
/// A property group GROUPS properties of the component type, so each name it
/// carries has to be one the component type defines: one of its own
/// definitions, or one it inherits from the component types it extends. The
/// groups and the names are both read off `record` as the write leaves it, so a
/// group may name a property the same request defines and no caller can check
/// one component type's groups against another's definitions. Where the chain
/// reaches a built-in type this emulator does not model, it does not hold its
/// definitions, so a
/// name it does not otherwise account for cannot be called undefined and the
/// group stands. Shared by every writer of a component type's `propertyGroups`.
pub fn property_groups_violation(workspace: &Workspace, record: &ComponentType) -> Option<String> {
    let (inherited, inherits_unknown_definitions) =
        workspace.inherited_property_names(&record.extends_from);
    if inherits_unknown_definitions {
        return None;
    }
    for (group_name, group) in &record.property_groups {
        for property_name in &group.property_names {
            if record.property_definitions.contains_key(property_name)
                || inherited.contains(property_name)
            {
                continue;
            }
            return Some(format!(
                "propertyGroups.{group_name}.propertyNames names property {property_name}, which \
                 component type {} does not define",
                record.component_type_id
            ));
        }
    }
    None
}

/// One property definition RESOLVED over the declarations that reach it,
/// NEAREST declaration first: every member is the value the nearest declaration
/// that SETS it gives, and the members no declaration sets stay unset. `None`
/// when no declaration reaches the property at all.
///
/// Every member of `PropertyDefinitionRequest` is optional, so a declaration
/// may REFINE an inherited definition — restate only the members it changes,
/// `dataType` included — rather than replace it, which is exactly why a writer
/// accepts a definition without a `dataType` wherever one is inherited
/// ([`Workspace::stranded_property_definition`]). A reader therefore folds the
/// whole set of declarations that reach a property instead of taking the
/// nearest one whole, so the definition it reports carries the refined members
/// and the inherited values of everything the refinement left alone.
///
/// Shared by every resolver of a definition — a component's
/// ([`Workspace::effective_property_definition`]) and a component type's own —
/// so one property is resolved one way wherever it is read.
pub fn refined_property_definition<'a>(
    declarations: impl IntoIterator<Item = &'a crate::ops::types::PropertyDefinitionRequest>,
) -> Option<crate::ops::types::PropertyDefinitionRequest> {
    let mut declarations = declarations.into_iter();
    let mut resolved = declarations.next()?.clone();
    for inherited in declarations {
        resolved.data_type = resolved
            .data_type
            .take()
            .or_else(|| inherited.data_type.clone());
        resolved.is_required_in_entity = resolved
            .is_required_in_entity
            .or(inherited.is_required_in_entity);
        resolved.is_external_id = resolved.is_external_id.or(inherited.is_external_id);
        resolved.is_stored_externally = resolved
            .is_stored_externally
            .or(inherited.is_stored_externally);
        resolved.is_time_series = resolved.is_time_series.or(inherited.is_time_series);
        resolved.default_value = resolved
            .default_value
            .take()
            .or_else(|| inherited.default_value.clone());
        resolved.configuration = resolved
            .configuration
            .take()
            .or_else(|| inherited.configuration.clone());
        resolved.display_name = resolved
            .display_name
            .take()
            .or_else(|| inherited.display_name.clone());
    }
    Some(resolved)
}

/// One property group RESOLVED over the declarations that reach it, NEAREST
/// declaration first: `groupType` is the one the nearest declaration that
/// carries it names, and `propertyNames` the list of the nearest declaration
/// that carries any. `None` when no declaration reaches the group at all.
///
/// Both request shapes a group is written from (`PropertyGroupRequest` for a
/// component type, `ComponentPropertyGroupRequest` for an entity component)
/// make every member optional, so a declaration may REFINE an inherited group —
/// restate only the part it changes — rather than replace it. A record stores an
/// absent list as the empty one ([`PropertyGroupRecord::from_request`]), so a
/// declaration with no names sets nothing and leaves the inherited group's names
/// standing, exactly as a definition's unset member keeps the inherited value
/// ([`refined_property_definition`]).
///
/// Shared by every resolver of a group — a component's
/// ([`Workspace::effective_property_group`]) and a component type's own — so one
/// group is resolved one way wherever it is read.
pub fn refined_property_group<'a>(
    declarations: impl IntoIterator<Item = &'a PropertyGroupRecord>,
) -> Option<PropertyGroupRecord> {
    let mut declarations = declarations.into_iter();
    let mut resolved = declarations.next()?.clone();
    for inherited in declarations {
        resolved.group_type = resolved
            .group_type
            .take()
            .or_else(|| inherited.group_type.clone());
        if resolved.property_names.is_empty() {
            resolved.property_names = inherited.property_names.clone();
        }
    }
    Some(resolved)
}

/// One stored `PropertyDefinitionRequest` as the modelled
/// `PropertyDefinitionResponse` a read reports.
///
/// Shared by every reader of a stored definition — an entity's component
/// properties (`GetEntity`) and a component type's own property definitions
/// (`GetComponentType`) — so both report one definition the same way.
///
/// `definition` is the RESOLVED definition, the one
/// [`refined_property_definition`] folded out of every declaration that reaches
/// the property, so a declaration that refines an inherited one is reported with
/// the inherited values of the members it left alone.
///
/// `None` when even the resolved definition carries no `dataType`, which the
/// response shape requires: a writer refuses a definition without one wherever
/// nothing is inherited, and a declaration that omits it takes its parent's, so
/// what is left is a chain reaching a built-in of the real service this emulator
/// does not model — this stays fallible rather than inventing a `Type` the model
/// does not define.
///
/// `is_inherited` is what the caller resolved this definition through: a
/// definition that reaches the component or component type through its
/// inheritance is inherited whether or not the reader also refines it, and one
/// nothing on that chain declares is its own
/// ([`Workspace::property_definition_is_inherited`]). Nothing
/// imports or finalises a definition yet: `isImported` and `isFinal` describe a
/// definition projected from an external data store and one that cannot be
/// updated, neither of which this service produces.
pub fn property_definition_response(
    definition: &crate::ops::types::PropertyDefinitionRequest,
    is_inherited: bool,
) -> Option<crate::ops::types::PropertyDefinitionResponse> {
    let Some(data_type) = definition.data_type.clone() else {
        // Logged rather than dropped silently so a regression in a writer that
        // is supposed to refuse a definition without a `dataType`, or a
        // resolution that failed to inherit one, is visible instead of quietly
        // costing the member.
        ::tracing::warn!("a resolved property definition carries no dataType; omitting it");
        return None;
    };
    Some(crate::ops::types::PropertyDefinitionResponse {
        data_type,
        is_time_series: definition.is_time_series.unwrap_or(false),
        is_required_in_entity: definition.is_required_in_entity.unwrap_or(false),
        is_external_id: definition.is_external_id.unwrap_or(false),
        is_stored_externally: definition.is_stored_externally.unwrap_or(false),
        is_imported: false,
        is_final: false,
        is_inherited,
        default_value: definition.default_value.clone(),
        configuration: definition.configuration.clone(),
        display_name: definition.display_name.clone(),
    })
}

/// The value a read reports for ONE property: the MOST RECENT value written to
/// the property, else the DEFAULT its definition declares, and nothing at all
/// when neither exists.
///
/// A property is written to through either plane, so the plane that wrote LAST
/// is the one the value comes from ([`PropertyRecord::latest_written_value`]):
/// the time-series data plane writes values FOR an instant
/// ([`PropertyRecord::time_series`], keyed by that instant, so its LAST entry
/// is the value written for the latest instant), and the control plane writes
/// one current value ([`PropertyRecord::value`]) or takes it away again
/// ([`PropertyRecord::reset_value`]). Every write is therefore observable here,
/// whichever plane made it; `defaultValue` is "an object that contains the
/// default value" of the property, so a property nothing has written to — and
/// one whose value a reset took away — reads as the value its definition gave
/// it, and either plane's write replaces it.
///
/// Shared by every reader of a property's current value, so one property reads
/// the same way wherever it is reported; `definition` is the one
/// [`Workspace::effective_property_definition`] resolved, which is what makes a
/// default declared only on the component TYPE apply to a component that
/// restates nothing.
pub fn latest_property_value(
    property: Option<&PropertyRecord>,
    definition: Option<&crate::ops::types::PropertyDefinitionRequest>,
) -> Option<crate::ops::types::DataValue> {
    property
        .and_then(|property| property.latest_written_value())
        .or_else(|| definition.and_then(|definition| definition.default_value.clone()))
}

/// Whether a read of `value` reports every value the property holds —
/// `areAllPropertyValuesReturned`, "whether all values of a list or map type
/// property are returned in the API response".
///
/// Only a LIST or MAP value holds several values, and the flag's documentation
/// caps how many of them a response carries at
/// [`MAX_PROPERTY_VALUES_RETURNED`]; a value of any other data type is one
/// value, and a property that reads as nothing has none, so both report every
/// value they hold. Shared by every reader of a property's value so one
/// property answers the flag the same way wherever it is reported.
pub fn all_property_values_returned(value: Option<&crate::ops::types::DataValue>) -> bool {
    let Some(value) = value else {
        return true;
    };
    let held = value
        .list_value
        .as_ref()
        .map(Vec::len)
        .or_else(|| value.map_value.as_ref().map(::std::collections::HashMap::len));
    held.is_none_or(|held| held <= MAX_PROPERTY_VALUES_RETURNED)
}

/// Whether `definition` declares a property whose values are a TIME SERIES —
/// `isTimeSeries`, "a Boolean value that specifies whether the property
/// consists of time series data", resolved so an absent member reads as
/// `false`, and so a property with no definition at all is not one.
///
/// Which properties the time-series data plane serves is decided here once:
/// `BatchPutPropertyValues` "sets values for multiple time series properties"
/// and `GetPropertyValueHistory` reads "the history of a time series property
/// value", so both resolve the property's EFFECTIVE definition
/// ([`Workspace::effective_property_definition`]) and ask this.
pub fn property_is_time_series(
    definition: Option<&crate::ops::types::PropertyDefinitionRequest>,
) -> bool {
    definition.is_some_and(|definition| definition.is_time_series.unwrap_or(false))
}

/// The message a refusal carries for a property the time-series data plane
/// serves nothing of, because its definition does not MARK it as a time series
/// property ([`property_is_time_series`]).
///
/// Built from the request: the only thing that varies between two of these
/// refusals is which property the caller named, so the wording is fixed and the
/// property name is the one it was asked about. One construction shared by
/// every operation that can be asked for such a property — the data plane's
/// write and its history read — so the same refusal reads the same wherever it
/// is reported.
pub fn property_not_time_series_message(property_name: &str) -> String {
    format!("{property_name} is not marked as a time-series property.")
}

/// Which `Type` the value in a `DataValue` belongs to — the member it travels
/// in, spelled as the `Type` enum spells it
/// (`crate::ops::types::TYPE_VALUES`) — or `None` when it carries no typed
/// value at all.
///
/// A `DataValue` carries ONE value, so the members are tried in a fixed order
/// and the first one present names the type. `expression` is none of them: it
/// carries a parameter reference to be resolved rather than a value of a
/// declared type.
pub fn data_value_type(value: &crate::ops::types::DataValue) -> Option<&'static str> {
    if value.string_value.is_some() {
        return Some("STRING");
    }
    if value.double_value.is_some() {
        return Some("DOUBLE");
    }
    if value.integer_value.is_some() {
        return Some("INTEGER");
    }
    if value.long_value.is_some() {
        return Some("LONG");
    }
    if value.boolean_value.is_some() {
        return Some("BOOLEAN");
    }
    if value.list_value.is_some() {
        return Some("LIST");
    }
    if value.map_value.is_some() {
        return Some("MAP");
    }
    if value.relationship_value.is_some() {
        return Some("RELATIONSHIP");
    }
    None
}

/// Why `value` cannot be stored as a value of the property `definition`
/// defines, as the message that earns — `None` when it can.
///
/// A property's `dataType` is "an object that specifies the data type of a
/// property", so a value written to the property carries a value of THAT type:
/// one carrying another type, or no value at all, is refused rather than
/// stored, which would leave a read reporting a value of a type the definition
/// does not declare. A definition declaring no `dataType` constrains nothing,
/// so only the presence of a value is required there.
///
/// Shared by every writer of a property's value, whichever plane it writes
/// through: the time-series data plane asks it of each value one entry carries,
/// and a writer of an entity's components asks it through
/// [`Workspace::component_property_value_violation`], which resolves the
/// definition the property effectively carries first.
pub fn data_value_type_violation(
    property_name: &str,
    definition: Option<&crate::ops::types::PropertyDefinitionRequest>,
    value: &crate::ops::types::DataValue,
) -> Option<String> {
    let carried = data_value_type(value);
    let Some(carried) = carried else {
        return Some(format!(
            "the value written to property {property_name} carries no value of any data type"
        ));
    };
    let declared = definition
        .and_then(|definition| definition.data_type.as_ref())
        .map(|data_type| data_type.r#type.as_str())?;
    (carried != declared).then(|| {
        format!(
            "the value written to property {property_name} is a {carried} where that \
             property's dataType is {declared}"
        )
    })
}

/// Why the `defaultValue` the resolved `definition` declares cannot stand as a
/// value of the property that definition defines, as the message that earns —
/// `None` when it can.
///
/// `defaultValue` is "an object that contains the default value" of the
/// property, and a read REPORTS it as the property's value whenever nothing has
/// been written to the property ([`latest_property_value`]), so it is a value of
/// the property like any other and carries a value of the `dataType` the
/// definition declares beside it: the rule [`data_value_type_violation`] every
/// writer of a property's value is held to, asked here of the default a
/// definition declares rather than of a value a write carries. A default
/// carrying another type is refused rather than stored, so no read can report a
/// value of a type the definition reported beside it does not declare.
///
/// `definition` is the definition a read REPORTS for the property — the
/// declaration being stored folded over the ones it inherits
/// ([`refined_property_definition`]) — because `dataType` and `defaultValue` are
/// resolved over that same fold: a declaration may refine an inherited
/// `dataType` with its own default, or an inherited default with its own
/// `dataType`, and it is the pair a read puts in one response that has to agree.
/// A definition no `dataType` reaches constrains nothing, and one whose default
/// names no typed value at all contradicts no `dataType`, so neither is refused
/// here.
///
/// Shared by every writer of a property DEFINITION, whichever resource holds it:
/// a component type's own definitions
/// ([`Workspace::property_definition_default_violation`]) and a component's
/// ([`Workspace::component_property_default_violation`]).
pub fn default_value_type_violation(
    property_name: &str,
    definition: Option<&crate::ops::types::PropertyDefinitionRequest>,
) -> Option<String> {
    let definition = definition?;
    let default = definition.default_value.as_ref()?;
    let carried = data_value_type(default)?;
    let declared = definition
        .data_type
        .as_ref()
        .map(|data_type| data_type.r#type.as_str())?;
    (carried != declared).then(|| {
        format!(
            "the defaultValue of property {property_name} is a {carried} where that \
             property's dataType is {declared}"
        )
    })
}

/// The comparison operators a property FILTER may carry.
///
/// `operator` is modelled as a plain string with no value list, so this set is
/// not read off the model: these are the comparisons this service's own query
/// language spells in a `WHERE` clause (`crate::ops::execute_query`), asked of
/// the same stored property values. An operator outside the set is refused by
/// name rather than ignored, which would answer a filtered question with
/// unfiltered rows. Shared by every operation that filters on a property's
/// value, so one filter means one thing wherever it is read.
pub const COMPARISON_OPERATORS: &[&str] = &["=", "!=", "<", "<=", ">", ">="];

/// The operator a filter that names none compares with: a filter carries a
/// value to compare against, and equality is the comparison that value alone
/// describes.
pub const COMPARISON_OPERATOR_EQUALS: &str = "=";

/// Whether a comparison that came out `ordering` satisfies `operator`.
pub fn comparison_satisfies(operator: &str, ordering: ::std::cmp::Ordering) -> bool {
    use ::std::cmp::Ordering;
    match operator {
        "=" => ordering == Ordering::Equal,
        "!=" => ordering != Ordering::Equal,
        "<" => ordering == Ordering::Less,
        "<=" => ordering != Ordering::Greater,
        ">" => ordering == Ordering::Greater,
        ">=" => ordering != Ordering::Less,
        // Unreachable: a filter's operator is checked against
        // [`COMPARISON_OPERATORS`] before it is applied, and every one of them
        // is matched above.
        _ => false,
    }
}

/// Compare two modelled `DataValue`s for a FILTER, or `None` when they carry
/// nothing comparable: two values of different kinds, or a kind with no order
/// (a list, a map, a relationship, an expression).
///
/// `INTEGER`, `LONG` and `DOUBLE` are widths of ONE domain, so a filter
/// comparing a property against a threshold compares numbers whichever member
/// each of them travels in; two integral values compare exactly, so two `LONG`s
/// too far apart for a double to tell apart still compare as the distinct
/// values they are.
pub fn compare_data_values(
    left: &crate::ops::types::DataValue,
    right: &crate::ops::types::DataValue,
) -> Option<::std::cmp::Ordering> {
    if let (Some(left), Some(right)) = (DataNumber::of(left), DataNumber::of(right)) {
        return left.compare(&right);
    }
    if let (Some(left), Some(right)) = (
        left.string_value.as_deref(),
        right.string_value.as_deref(),
    ) {
        return Some(left.cmp(right));
    }
    if let (Some(left), Some(right)) = (left.boolean_value, right.boolean_value) {
        return Some(left.cmp(&right));
    }
    None
}

/// The number a `DataValue` carries placed on the DOUBLE, the one width every
/// number can be placed on — what an ORDERING compares, where a total relation
/// matters more than the last bits of a very large integer — or `None` when it
/// carries no number.
pub fn data_value_as_double(value: &crate::ops::types::DataValue) -> Option<f64> {
    DataNumber::of(value).map(|number| number.as_double())
}

/// A number a `DataValue` carries, at the width it travels in. The integral
/// widths are kept as an integer rather than folded into a double, so two
/// values too far apart for a double to tell apart still compare as the
/// distinct values they are.
enum DataNumber {
    Integer(i128),
    Double(f64),
}

impl DataNumber {
    /// The number this value carries, or `None` when it carries none.
    fn of(value: &crate::ops::types::DataValue) -> Option<Self> {
        if let Some(double) = value.double_value {
            return Some(Self::Double(double));
        }
        if let Some(integer) = value.integer_value {
            return Some(Self::Integer(i128::from(integer)));
        }
        value.long_value.map(|long| Self::Integer(i128::from(long)))
    }

    /// See [`data_value_as_double`].
    fn as_double(&self) -> f64 {
        match self {
            Self::Integer(integer) => *integer as f64,
            Self::Double(double) => *double,
        }
    }

    /// Compare two numbers for a FILTER, which judges one pair at a time: two
    /// integral values compare exactly, and a comparison that involves a double
    /// compares on the double — `None` when a `NaN` makes the pair
    /// incomparable.
    fn compare(&self, other: &Self) -> Option<::std::cmp::Ordering> {
        match (self, other) {
            (Self::Integer(left), Self::Integer(right)) => Some(left.cmp(right)),
            _ => self.as_double().partial_cmp(&other.as_double()),
        }
    }
}

/// Nanoseconds in one second: the unit a stored time-series instant is kept in,
/// so a value written with a fractional second keeps it.
pub const NANOS_PER_SECOND: i128 = 1_000_000_000;

/// The instant an ISO 8601 `Time` names, in nanoseconds since the Unix epoch,
/// or `None` when the text names none.
///
/// `Time` is "the ISO8601 DateTime of a value for a time series property",
/// which is a STRING in the model, so nothing before a handler parses it. The
/// zone it declares is read by the one reader of an RFC 3339 offset
/// (`service_common::rfc3339::split_utc_offset`) and text carrying no zone at
/// all is read as UTC, the zone the format's own designator stands for; the
/// calendar conversion itself is the library's.
pub fn instant_of_time(text: &str) -> Option<i128> {
    let (civil, offset_seconds) = match ::service_common::rfc3339::split_utc_offset(text) {
        Some(split) => split,
        None => (text, 0),
    };
    let parsed = ::aws_smithy_types::DateTime::from_str(
        &format!("{civil}Z"),
        ::aws_smithy_types::date_time::Format::DateTime,
    )
    .ok()?;
    Some(instant_of_timestamp(&parsed) - i128::from(offset_seconds) * NANOS_PER_SECOND)
}

/// The instant a modelled `Timestamp` names, in nanoseconds since the Unix
/// epoch — the same unit [`instant_of_time`] answers in, so a value written
/// with the deprecated `timestamp` member and one written with `time` land on
/// one timeline.
pub fn instant_of_timestamp(at: &::aws_smithy_types::DateTime) -> i128 {
    i128::from(at.secs()) * NANOS_PER_SECOND + i128::from(at.subsec_nanos())
}

/// A stored instant as the ISO 8601 `Time` the wire carries, or `None` for an
/// instant outside the range the library's calendar covers.
pub fn time_of_instant(at: i128) -> Option<String> {
    let secs = i64::try_from(at.div_euclid(NANOS_PER_SECOND)).ok()?;
    let nanos = u32::try_from(at.rem_euclid(NANOS_PER_SECOND)).ok()?;
    ::aws_smithy_types::DateTime::from_secs_and_nanos(secs, nanos)
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
        .ok()
}

/// The most values this emulator keeps for ONE time series property.
///
/// A time series has no end: `BatchPutPropertyValues` may be called forever, so
/// the series a property holds is a bounded window of the most recent values
/// rather than everything ever written. The real service keeps values for a
/// retention period this emulator has no clock to age them out with, so the
/// bound is on the COUNT and is this emulator's own declared ceiling — the
/// same reading as [`MAX_WORKSPACES`] and the ceilings beside it.
pub const MAX_TIME_SERIES_VALUES_PER_PROPERTY: usize = 10_000;

/// One value of a time series property: what was written, and the instant text
/// it was written with.
///
/// The instant itself is the key the value is held under
/// ([`PropertyRecord::time_series`]), so two values written for the same
/// instant are one value — the later write replaces the earlier one — and the
/// series is in time order whatever order it was written in. The TEXT is kept
/// beside the value so a read reports the instant as the writer spelled it
/// rather than as this service would re-render it.
#[derive(Debug, Clone)]
pub struct TimeSeriesValue {
    pub value: crate::ops::types::DataValue,
    /// The ISO 8601 `Time` the write carried, or the rendering of the instant
    /// for a write that carried only the deprecated `timestamp`.
    pub time: String,
}

/// One component of an entity.
///
/// A component's properties and property groups are stored as the modelled
/// request shapes the caller supplied, so a read reports back exactly the
/// definition and value that were persisted rather than a lossy projection of
/// them. Only what the entity DECLARED is stored here: the rest of the
/// component's schema lives on the component type it instantiates and is
/// resolved against the workspace when a component is read
/// ([`Workspace::component_property_names`],
/// [`Workspace::effective_property_definition`]).
#[derive(Debug, Clone, Default)]
pub struct ComponentRecord {
    pub component_name: String,
    pub component_type_id: Option<String>,
    pub description: Option<String>,
    /// The component's properties, keyed by property name.
    pub properties: BTreeMap<String, PropertyRecord>,
    /// The component's property groups, keyed by group name.
    pub property_groups: BTreeMap<String, PropertyGroupRecord>,
    /// The composite components created INSIDE this component, keyed by the
    /// part of their `ComponentPath` below this component (`inner`, or
    /// `inner/deeper` for one nested a level further down) — the path with its
    /// first segment, which is this component's own name, removed. Keyed that
    /// way because the path below the component is what identifies a composite
    /// component within it, whatever depth it sits at. See
    /// [`resolve_composite_component_path`].
    pub composite_components: BTreeMap<String, CompositeComponentRecord>,
}

/// One composite component of an entity's component: an instance of the
/// component type that the holding component's own component type declares
/// under this name in its `compositeComponentTypes`.
///
/// Its properties and property groups are stored as the modelled request shapes
/// the caller supplied, the same choice [`ComponentRecord`] makes.
#[derive(Debug, Clone, Default)]
pub struct CompositeComponentRecord {
    /// The whole `ComponentPath` the create addressed this composite component
    /// by, starting at the entity's top-level component. Reported as
    /// `ComponentSummary::componentPath`.
    pub component_path: String,
    /// This composite component's own name: the LAST segment of its path, which
    /// is what `GetEntity` keys `ComponentResponse::compositeComponents` by
    /// (`CompositeComponentResponse`'s key is a `Name`, whose pattern admits no
    /// `/`).
    pub component_name: String,
    /// The component type this composite component is an instance of — the one
    /// the declaration its path resolved to names.
    pub component_type_id: String,
    pub description: Option<String>,
    /// The composite component's properties, keyed by property name.
    pub properties: BTreeMap<String, PropertyRecord>,
    /// The composite component's property groups, keyed by group name.
    pub property_groups: BTreeMap<String, PropertyGroupRecord>,
}

/// The separator between the segments of a `ComponentPath`.
///
/// A composite component is addressed by the path of names that leads to it:
/// the entity's top-level component, then one name per
/// `compositeComponentTypes` declaration walked through. `ComponentPath`'s
/// pattern (`^[a-zA-Z_\-0-9/]+$`) is a `Name`'s (`^[a-zA-Z_\-0-9]+$`) plus
/// `/`, so `/` is what joins those names.
pub const COMPONENT_PATH_SEPARATOR: &str = "/";

/// A `ComponentPath` resolved against an entity's components and its
/// workspace's component types. See [`resolve_composite_component_path`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedComponentPath {
    /// The entity's top-level component the path starts at: its first segment.
    pub component_name: String,
    /// The path BELOW that component — the remaining segments, still joined by
    /// [`COMPONENT_PATH_SEPARATOR`]. This is the key the composite component is
    /// held under in [`ComponentRecord::composite_components`].
    pub composite_path: String,
    /// The composite component's own name: the path's last segment.
    pub composite_name: String,
    /// The component type the declaration the path ends at names: the type the
    /// composite component is an instance of.
    pub component_type_id: String,
}

/// The transitive `extendsFrom` closure of `seeds` over a stored component-type
/// map, as [`Workspace::component_type_closure`] describes it.
///
/// The walk itself lives here, over the MAP rather than the whole workspace, so
/// a resolution that holds only the component types — a writer of an entity's
/// composite components, which also holds the entity being patched — walks an
/// inheritance chain the same single way every reader of inherited structure
/// does.
fn component_type_closure_in(
    component_types: &BTreeMap<String, ComponentType>,
    seeds: impl IntoIterator<Item = String>,
) -> (Vec<&ComponentType>, BTreeSet<String>, bool) {
    let mut reached: BTreeSet<String> = BTreeSet::new();
    let mut queue: VecDeque<String> = seeds.into_iter().collect();
    let mut chain = Vec::new();
    let mut holds_unstored = false;
    while let Some(ancestor) = queue.pop_front() {
        if !reached.insert(ancestor.clone()) {
            continue;
        }
        match component_types.get(&ancestor) {
            Some(stored) => {
                chain.push(stored);
                queue.extend(stored.extends_from.iter().cloned());
            }
            None => holds_unstored = true,
        }
    }
    (chain, reached, holds_unstored)
}

/// The component type `component_type_id` and every component type it extends,
/// nearest first — [`Workspace::component_type_chain`] over a component-type
/// map. Empty when the map holds no component type under the id.
pub fn component_type_chain_in<'a>(
    component_types: &'a BTreeMap<String, ComponentType>,
    component_type_id: &str,
) -> Vec<&'a ComponentType> {
    let (chain, _, _) =
        component_type_closure_in(component_types, [component_type_id.to_string()]);
    chain
}

/// Resolve a `ComponentPath` that addresses a COMPOSITE component of an entity.
///
/// A composite component path is valid when its first segment names one of the
/// entity's components and each following segment names an entry in the
/// `compositeComponentTypes` of the component type reached by the segment
/// before it — which is why resolution needs both the entity's components and
/// the workspace's component types. A path that resolves yields the component it
/// hangs under and the component type it instantiates; one that does not yields
/// the reason, built from the request so the refusal names the member and the
/// segment that failed to resolve.
///
/// Shared by every writer that BRINGS a composite component into being, so a
/// create and an update hold one path to one rule. `member` is the request
/// member the path arrived in (`compositeComponents` on a create,
/// `compositeComponentUpdates` on an update), so a refusal names the member the
/// caller has to fix rather than another operation's. A path that names a
/// composite component the entity ALREADY HOLDS is answered from the store
/// instead ([`stored_composite_component_path`]): the declarations are what a
/// NEW composite component needs, not what keeps a stored one addressable.
///
/// Two limits are deliberate, both inherited from how this service stores
/// component types. A path of a single segment names a top-level component and
/// no composite component, so it does not resolve; and a segment is resolved
/// against the EFFECTIVE `compositeComponentTypes` of the component type
/// reached so far — its own declarations plus the ones its `extendsFrom` chain
/// contributes, nearest first, which is the schema the component type reader
/// reports — so a path through a component type this workspace does not hold at
/// all cannot be resolved, and is refused rather than guessed at.
pub fn resolve_composite_component_path(
    components: &BTreeMap<String, ComponentRecord>,
    component_types: &BTreeMap<String, ComponentType>,
    member: &str,
    component_path: &str,
) -> Result<ResolvedComponentPath, String> {
    let segments: Vec<&str> = component_path.split(COMPONENT_PATH_SEPARATOR).collect();
    if segments.len() < 2 || segments.iter().any(|segment| segment.is_empty()) {
        return Err(format!(
            "{member}.{component_path} is not the path of a composite component: a \
             component path names one of the entity's components and at least one \
             compositeComponentType declared inside it, separated by \
             '{COMPONENT_PATH_SEPARATOR}'"
        ));
    }
    let component_name = segments[0];
    let Some(component) = components.get(component_name) else {
        return Err(format!(
            "{member}.{component_path} names component {component_name}, which the \
             entity does not have"
        ));
    };
    let Some(mut component_type_id) = component.component_type_id.clone() else {
        return Err(format!(
            "{member}.{component_path} names component {component_name}, which \
             instantiates no componentType and so declares no composite component"
        ));
    };
    for segment in &segments[1..] {
        // Every declaration that reaches the entry, nearest first: the
        // component type's own, then its `extendsFrom` chain's. The chain is
        // empty exactly when this workspace holds no component type under the
        // id.
        let chain = component_type_chain_in(component_types, &component_type_id);
        if chain.is_empty() {
            return Err(format!(
                "{member}.{component_path}: componentType {component_type_id} does \
                 not exist in this workspace, so {segment} cannot be resolved against it"
            ));
        }
        let mut declarations = chain
            .iter()
            .filter_map(|parent| parent.composite_component_types.get(*segment))
            .peekable();
        if declarations.peek().is_none() {
            return Err(format!(
                "{member}.{component_path}: componentType {component_type_id} \
                 declares no compositeComponentTypes.{segment}"
            ));
        }
        // The id instantiated is the one the NEAREST declaration that names any
        // refers to, the fold every other part of an inherited schema gets.
        let Some(declared) =
            declarations.find_map(|declaration| declaration.component_type_id.clone())
        else {
            return Err(format!(
                "{member}.{component_path}: componentType {component_type_id} \
                 declares compositeComponentTypes.{segment} with no componentTypeId, so it names \
                 no component type to instantiate"
            ));
        };
        component_type_id = declared;
    }
    Ok(ResolvedComponentPath {
        component_name: component_name.to_string(),
        composite_path: segments[1..].join(COMPONENT_PATH_SEPARATOR),
        composite_name: segments[segments.len() - 1].to_string(),
        component_type_id,
    })
}

/// The composite component an entity ALREADY STORES at a `ComponentPath`,
/// resolved from the STORE rather than from the component types.
///
/// A stored composite component is a record of its own: it carries the
/// `componentTypeId` it instantiates, and the entity holds it under the path
/// below its component whatever the holding component type declares TODAY. So
/// a path that names a record the entity holds is answerable without the
/// declarations — exactly how every reader of a `componentPath` answers it
/// ([`component_at_path`]) — and a writer addressing an EXISTING composite
/// component resolves it here, while [`resolve_composite_component_path`]
/// stays the rule for a path that has to be resolved against the declarations
/// because no record stands at it yet (a create). Without this, a component
/// type patch that drops a `compositeComponentTypes` declaration would leave
/// the composite components standing on it readable but unwritable: nothing
/// could update or remove a record the store still holds and the reference scan
/// ([`Workspace::component_type_references`]) still counts.
///
/// `None` when the entity holds no composite component at the path, including
/// when the path names no composite component at all (a single segment).
pub fn stored_composite_component_path(
    components: &BTreeMap<String, ComponentRecord>,
    component_path: &str,
) -> Option<ResolvedComponentPath> {
    let (component_name, composite_path) = component_path.split_once(COMPONENT_PATH_SEPARATOR)?;
    let composite = components
        .get(component_name)?
        .composite_components
        .get(composite_path)?;
    Some(ResolvedComponentPath {
        component_name: component_name.to_string(),
        composite_path: composite_path.to_string(),
        composite_name: composite.component_name.clone(),
        component_type_id: composite.component_type_id.clone(),
    })
}

/// The component of an entity a `ComponentPath` addresses, as
/// [`component_at_path`] read it: either one of the entity's top-level
/// components or a composite component held inside one of them.
///
/// The two are kept apart rather than reduced on the spot, because a caller
/// reports them differently — a composite component is named by its own
/// `ComponentPath` — while every caller needs the same few things out of the
/// resolution, which [`component_type_id`](Self::component_type_id),
/// [`properties`](Self::properties), [`component_name`](Self::component_name),
/// [`component_path`](Self::component_path) and
/// [`description`](Self::description) answer.
#[derive(Debug, Clone, Copy)]
pub enum ComponentAtPath<'a> {
    /// The path named one of the entity's top-level components: it had a single
    /// segment.
    TopLevel(&'a ComponentRecord),
    /// The path named a composite component inside one of the entity's
    /// components.
    Composite {
        /// The top-level component the composite component hangs under: the
        /// path's first segment.
        component: &'a ComponentRecord,
        /// The path BELOW that component — the key
        /// [`ComponentRecord::composite_components`] holds the composite
        /// component under.
        composite_path: &'a str,
        /// The addressed composite component itself.
        composite: &'a CompositeComponentRecord,
    },
}

impl<'a> ComponentAtPath<'a> {
    /// The component type the addressed component instantiates. Absent only for
    /// a top-level component created without a `componentTypeId`; a composite
    /// component always instantiates the type its declaration names.
    pub fn component_type_id(&self) -> Option<&'a str> {
        match self {
            Self::TopLevel(component) => component.component_type_id.as_deref(),
            Self::Composite { composite, .. } => Some(&composite.component_type_id),
        }
    }

    /// The addressed component's stored properties, keyed by property name.
    pub fn properties(&self) -> &'a BTreeMap<String, PropertyRecord> {
        match self {
            Self::TopLevel(component) => &component.properties,
            Self::Composite { composite, .. } => &composite.properties,
        }
    }

    /// The component's own name: a top-level component's name, or — for a
    /// composite component — the LAST segment of its path, which is the name
    /// `GetEntity` keys it under.
    pub fn component_name(&self) -> &'a str {
        match self {
            Self::TopLevel(component) => &component.component_name,
            Self::Composite { composite, .. } => &composite.component_name,
        }
    }

    /// The whole `ComponentPath` this component is addressed by, for a COMPOSITE
    /// component; `None` for a top-level one, which is addressed by its name
    /// alone. This is the same split `ComponentSummary` reports, where only a
    /// composite component carries a `componentPath`.
    pub fn component_path(&self) -> Option<&'a str> {
        match self {
            Self::TopLevel(_) => None,
            Self::Composite { composite, .. } => Some(&composite.component_path),
        }
    }

    /// The description stored on the component, absent when it carries none.
    pub fn description(&self) -> Option<&'a str> {
        match self {
            Self::TopLevel(component) => component.description.as_deref(),
            Self::Composite { composite, .. } => composite.description.as_deref(),
        }
    }
}

/// The component of `entity` a `ComponentPath` addresses, or `None` when the
/// entity holds none there.
///
/// A path is "the path to the composite component, STARTING FROM the top-level
/// component", so a path of ONE segment names that top-level component, and a
/// longer one names the composite component stored under the REST of the path
/// inside it — the key [`ComponentRecord::composite_components`] holds it
/// under, whatever depth the composite component sits at.
///
/// This is the READ side of the rule
/// [`resolve_composite_component_path`] writes with: every property reader that
/// accepts a `componentPath` resolves it here, so one change to how a path is
/// read reaches all of them at once and they cannot disagree about which
/// component one path names. A reader needs no component types to resolve a
/// path, unlike a writer: the composite components a path may reach are the
/// ones the entity already holds.
pub fn component_at_path<'a>(
    entity: &'a Entity,
    component_path: &str,
) -> Option<ComponentAtPath<'a>> {
    match component_path.split_once(COMPONENT_PATH_SEPARATOR) {
        None => Some(ComponentAtPath::TopLevel(
            entity.components.get(component_path)?,
        )),
        Some((component_name, composite_path)) => {
            let component = entity.components.get(component_name)?;
            // By key, so the path below the component is borrowed from the
            // STORE and outlives the request's own string.
            let (composite_path, composite) =
                component.composite_components.get_key_value(composite_path)?;
            Some(ComponentAtPath::Composite {
                component,
                composite_path,
                composite,
            })
        }
    }
}

/// ONE LEVEL of an entity's component tree: the composite components the
/// component `component_path` leads to holds DIRECTLY, or `None` when the entity
/// holds no component at that path.
///
/// Existence is the rule [`component_at_path`] reads a path with, widened by the
/// one thing a level walk adds: nothing requires the intermediate steps of a
/// composite component's path to have been created themselves, so a path names a
/// component when one is stored AT it or anywhere BENEATH it.
///
/// The level that path leads to therefore has one entry per DISTINCT next path
/// segment the entity holds a composite component at or beneath — the stored
/// record when one stands at that segment, else the step itself, which carries
/// the component type the declaration its path resolves to names
/// ([`resolve_composite_component_path`]) and no properties or property groups
/// of its own, since nothing was ever stored at it. That is what makes the walk
/// TOTAL: every composite component the entity holds is reachable from its
/// top-level component one segment per call, so a caller walking a level at a
/// time reaches exactly the composite components a whole-entity read
/// ([`Entity::components_at_every_path`]) reports.
///
/// Entries come back in segment order, the order the store is keyed in, so two
/// reads of unchanged state build the same level and a page boundary does not
/// depend on map iteration order.
pub fn component_level_at_path<'a>(
    entity: &'a Entity,
    component_types: &BTreeMap<String, ComponentType>,
    component_path: &str,
) -> Option<Vec<Cow<'a, CompositeComponentRecord>>> {
    let separator = COMPONENT_PATH_SEPARATOR;
    let (component_name, below) = match component_path.split_once(separator) {
        None => (component_path, None),
        Some((component_name, below)) => (component_name, Some(below)),
    };
    let component = entity.components.get(component_name)?;
    // The key prefix a child of the named component carries inside
    // [`ComponentRecord::composite_components`]. Empty for a path of one
    // segment: the children of a top-level component are the composite
    // components keyed by a single segment inside it.
    let prefix = match below {
        None => String::new(),
        Some(below) => {
            let nested = format!("{below}{separator}");
            if component_at_path(entity, component_path).is_none()
                && !component
                    .composite_components
                    .keys()
                    .any(|key| key.starts_with(&nested))
            {
                return None;
            }
            nested
        }
    };
    let mut level: BTreeMap<&str, Cow<'a, CompositeComponentRecord>> = BTreeMap::new();
    for key in component.composite_components.keys() {
        let Some(below_prefix) = key.strip_prefix(prefix.as_str()) else {
            continue;
        };
        let segment = match below_prefix.split_once(separator) {
            None => below_prefix,
            Some((segment, _)) => segment,
        };
        if segment.is_empty() || level.contains_key(segment) {
            continue;
        }
        let child_key = format!("{prefix}{segment}");
        let entry = match component.composite_components.get(&child_key) {
            Some(stored) => Cow::Borrowed(stored),
            // Nothing stands at this step: it is reported as the step it is,
            // addressed by its own whole path and instantiating the component
            // type the declarations resolve it to.
            None => {
                let child_path = format!("{component_name}{separator}{child_key}");
                let component_type_id = resolve_composite_component_path(
                    &entity.components,
                    component_types,
                    "componentPath",
                    &child_path,
                )
                // The declarations normally resolve a step a stored record
                // hangs beneath, since that record's own create resolved
                // through them. When a later component-type patch has dropped
                // the declaration, the step keeps no type to report and reports
                // the empty string — what a component stored with no
                // `componentTypeId` already reports for a required member,
                // rather than hiding a step the stored record is reached
                // through.
                .map(|resolved| resolved.component_type_id)
                .unwrap_or_default();
                Cow::Owned(CompositeComponentRecord {
                    component_path: child_path,
                    component_name: segment.to_string(),
                    component_type_id,
                    ..CompositeComponentRecord::default()
                })
            }
        };
        level.insert(segment, entry);
    }
    Some(level.into_values().collect())
}

/// Every INSTANCE of one component type in a workspace, as the entity each sits
/// in and the component it is: a component whose `componentTypeId` is that type
/// or one that extends it ([`Workspace::component_type_instance_ids`]), at every
/// path an entity holds a component at ([`Entity::components_at_every_path`]) —
/// so a composite component of the type is an instance of it too.
///
/// This is the one scan a query addressed to a component TYPE is scoped by:
/// every reader that answers such a query resolves its scope here, so one
/// change to what counts as an instance — or to the order the instances come
/// back in — reaches all of them at once and they cannot disagree about which
/// components one component type names. The pairs come back in (entityId,
/// componentName) order, the order the store is keyed in, so two reads of
/// unchanged state build the same answer and a page boundary does not depend on
/// map iteration order.
///
/// BOUNDED by `ceiling`: the scope is request-chosen over caller-grown state, so
/// the scan stops as soon as it has collected MORE pairs than `ceiling` — the
/// caller sees a result longer than its own ceiling and refuses the request in
/// its own words rather than paying for the rest of the walk. Which ceiling, and
/// what the refusal says, stay with the caller: each reader materialises
/// something different per instance.
pub fn component_type_instances<'a>(
    workspace: &'a Workspace,
    component_type_id: &str,
    ceiling: usize,
) -> Vec<(&'a str, ComponentAtPath<'a>)> {
    // The component types an instance of the named one may name, resolved ONCE
    // per scan rather than once per component tested against it.
    let instance_type_ids = workspace.component_type_instance_ids(component_type_id);
    let mut instances: Vec<(&str, ComponentAtPath<'_>)> = Vec::new();
    for entity in workspace.entities.values() {
        for component in entity.components_at_every_path() {
            if component
                .component_type_id()
                .is_some_and(|instantiated| instance_type_ids.contains(instantiated))
            {
                instances.push((entity.entity_id.as_str(), component));
                if instances.len() > ceiling {
                    return instances;
                }
            }
        }
    }
    instances
}

/// Which plane wrote to a property's value LAST, which is the plane whose value
/// a read of the property reports ([`PropertyRecord::latest_written_value`]).
///
/// A property's value is writable through both planes — the control plane sets
/// or resets the one current value a property carries (`CreateEntity`,
/// `UpdateEntity`), the time-series data plane writes values for an instant
/// (`BatchPutPropertyValues`) — so which of the two a read reports is decided
/// by which wrote most recently, and a property no write has reached yet
/// carries no writer at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValueWriter {
    /// The control plane wrote the property's value last, so its one current
    /// value ([`PropertyRecord::value`]) is what the property reads as — and
    /// nothing at all where that write RESET the value away.
    ControlPlane,
    /// The time-series data plane wrote last, so the value of the latest
    /// instant in the series ([`PropertyRecord::time_series`]) is what the
    /// property reads as.
    DataPlane,
}

/// One property of a component: the definition and value the request gave it.
///
/// Whether the property is the component's external id is NOT answerable from
/// this record alone: `isExternalId` lives on the property DEFINITION, which a
/// component normally inherits from the component type it instantiates rather
/// than restating, so it is resolved against the workspace
/// ([`Workspace::component_external_ids`]).
#[derive(Debug, Clone, Default)]
pub struct PropertyRecord {
    pub definition: Option<crate::ops::types::PropertyDefinitionRequest>,
    /// The one current value the CONTROL plane set on this property, `None`
    /// where it has set none or where its last write RESET the value away.
    ///
    /// Written only through [`Self::set_value`] and [`Self::reset_value`], so
    /// the plane that wrote the property's value last
    /// ([`Self::value_writer`]) is recorded with every write to it.
    pub value: Option<crate::ops::types::DataValue>,
    /// The values written to this property through the time-series data plane
    /// (`BatchPutPropertyValues`), keyed by the instant each was written FOR in
    /// nanoseconds since the Unix epoch ([`instant_of_time`]).
    ///
    /// A map keyed by the instant, so the series is in time order however it
    /// was written, a value written twice for one instant is stored once, and a
    /// read of a window takes a range of it. It is EMPTY for every property
    /// nothing has written a time-series value to, which is every property of a
    /// definition that is not `isTimeSeries`
    /// ([`property_is_time_series`]) — the data plane writes to no other.
    pub time_series: BTreeMap<i128, TimeSeriesValue>,
    /// Which plane wrote to this property's value last ([`ValueWriter`]), and
    /// `None` for a property no write has reached: it is what decides which of
    /// the two the property reads as, so a write through either plane is the
    /// value every later read reports.
    pub value_writer: Option<ValueWriter>,
}

impl PropertyRecord {
    /// Set the one current value the control plane holds for this property.
    ///
    /// The control plane becomes the property's latest writer, so this value is
    /// what every read of it reports until the next write through either plane.
    pub fn set_value(&mut self, value: crate::ops::types::DataValue) {
        self.value = Some(value);
        self.value_writer = Some(ValueWriter::ControlPlane);
    }

    /// Take this property's value away, leaving its definition — a `RESET_VALUE`
    /// property update.
    ///
    /// A reset is a write of NO value: it makes the control plane the
    /// property's latest writer with nothing in hand, so the property reads as
    /// a property nothing has written to (its definition's default, else
    /// nothing at all) whatever either plane wrote before it. The series itself
    /// is left whole, because it is the history of the values written FOR their
    /// instants and a reset did not unwrite them.
    pub fn reset_value(&mut self) {
        self.value = None;
        self.value_writer = Some(ValueWriter::ControlPlane);
    }

    /// Record one time-series value for this property, at the instant `at`.
    ///
    /// A value written for an instant the series already holds REPLACES it: the
    /// instant is the value's identity, so writing the same instant twice
    /// leaves one value rather than two readings of one moment. Past this
    /// emulator's declared ceiling
    /// ([`MAX_TIME_SERIES_VALUES_PER_PROPERTY`]) the OLDEST values are
    /// dropped, so a caller that keeps writing grows the store no further and
    /// keeps the most recent window.
    ///
    /// The data plane becomes the property's latest writer, so a read reports
    /// the series' latest value again even where the control plane had set or
    /// reset one before.
    pub fn record_time_series_value(&mut self, at: i128, value: TimeSeriesValue) {
        self.time_series.insert(at, value);
        while self.time_series.len() > MAX_TIME_SERIES_VALUES_PER_PROPERTY {
            self.time_series.pop_first();
        }
        self.value_writer = Some(ValueWriter::DataPlane);
    }

    /// The most recent value WRITTEN to this property, through either plane —
    /// `None` for a property nothing has written a value to, and for one whose
    /// latest write RESET its value away.
    ///
    /// The plane that wrote last is the one the value comes from
    /// ([`Self::value_writer`]): the control plane holds the one current value
    /// it set ([`Self::value`]), and the data plane's series is keyed by the
    /// instant each value was written FOR ([`Self::time_series`]), so its last
    /// entry is the value of its latest instant.
    pub fn latest_written_value(&self) -> Option<crate::ops::types::DataValue> {
        match self.value_writer? {
            ValueWriter::ControlPlane => self.value.clone(),
            ValueWriter::DataPlane => self
                .time_series
                .last_key_value()
                .map(|(_, latest)| latest.value.clone()),
        }
    }
}

/// One property group of a component or of a component type.
#[derive(Debug, Clone, Default)]
pub struct PropertyGroupRecord {
    pub group_type: Option<String>,
    pub property_names: Vec<String>,
}

impl PropertyGroupRecord {
    /// One modelled `PropertyGroupRequest` as the record the store holds.
    ///
    /// Shared by every writer of a COMPONENT TYPE's property groups
    /// (`CreateComponentType`, `UpdateComponentType`), so one shape has one
    /// conversion. `propertyNames` is stored RESOLVED, so an absent list is an
    /// empty one rather than a missing collection every reader has to guard
    /// against. An entity component's groups are a different modelled shape
    /// (`ComponentPropertyGroupRequest`, which also carries the `updateType`
    /// that says what to do with the group), so `CreateEntity` builds its
    /// records from that shape instead.
    pub fn from_request(request: crate::ops::types::PropertyGroupRequest) -> Self {
        Self {
            group_type: request.group_type,
            property_names: request.property_names.unwrap_or_default(),
        }
    }
}

impl RegionalState {
    /// The account's billable entity count in this Region: how many entities
    /// this service currently holds there, across every workspace.
    ///
    /// IoT TwinMaker bills the entities an account keeps in its workspaces, so
    /// the count a pricing plan is reported with is a count of the stored
    /// entities — derived on every read, never a counter something bumps, so a
    /// create or a delete cannot leave it stale and reading the plan does not
    /// change it. Saturating rather than panicking on the (unreachable, given
    /// [`MAX_WORKSPACES`] × [`MAX_ENTITIES_PER_WORKSPACE`]) overflow of the
    /// modelled `Long`.
    pub fn billable_entity_count(&self) -> i64 {
        let count: usize = self
            .workspaces
            .values()
            .map(|workspace| workspace.entities.len())
            .sum();
        i64::try_from(count).unwrap_or(i64::MAX)
    }

    /// Set the account's pricing plan to `pricing_mode` with `bundle_names` at
    /// `now`.
    ///
    /// WHEN the change takes effect is not the caller's to choose: a change that
    /// buys the account at least what it already had takes effect at once, and a
    /// change that leaves a bundled plan for the pay-as-you-go mode is billed
    /// from the start of the next cycle ([`pricing_change_is_deferred`]). So the
    /// plan this call records goes either into
    /// [`pricing_plan`](Self::pricing_plan), with its `effectiveDateTime` at the
    /// instant of the call, or into
    /// [`pending_pricing_plan`](Self::pending_pricing_plan), with its
    /// `effectiveDateTime` [`PRICING_PLAN_PENDING_SPAN`] later and the plan in
    /// force left exactly as it was. Either way its `updateDateTime` is the
    /// instant of the call, which is when the service accepted it, and the plan
    /// is recorded WHOLE: it carries the requested mode and bundles, never a
    /// member left over from the plan before it.
    ///
    /// Whatever the clock already owed lands FIRST
    /// ([`settle_pricing_plan`](Self::settle_pricing_plan)), so this change is
    /// read against the plan really in force and a pending change that had
    /// already come due is applied rather than thrown away. A pending change
    /// that had NOT come due is superseded here and never becomes the plan in
    /// force.
    ///
    /// The `UpdateReason` recorded on the new plan says what this change was,
    /// read off the plan it replaces — never off the request alone:
    ///
    /// * [`UPDATE_REASON_OVERWRITTEN`] when it superseded a pending change,
    ///   which is the most specific thing that can be said about it;
    /// * else [`UPDATE_REASON_PRICING_MODE_UPDATE`] when the mode differs from
    ///   the mode in force;
    /// * else [`UPDATE_REASON_PRICING_TIER_UPDATE`], the mode being unchanged,
    ///   so the bundles — and the tier they place the account in — are all the
    ///   change can have moved.
    ///
    /// The remaining two values describe changes no caller asked for, so neither
    /// is recorded here: [`UPDATE_REASON_DEFAULT`] marks the plan of an account
    /// nobody has updated, and [`UPDATE_REASON_ENTITY_COUNT_UPDATE`] the tier
    /// move the service makes on its own
    /// ([`settle_pricing_plan`](Self::settle_pricing_plan)).
    ///
    /// `pricingTier` is recorded on a plan that names bundles, since that is the
    /// plan billed by tier, and the tier is the band the account's billable
    /// entity count falls in ([`PricingPlanRecord::tier_at`]).
    pub fn set_pricing_plan(
        &mut self,
        pricing_mode: String,
        bundle_names: Vec<String>,
        now: SystemTime,
    ) {
        self.settle_pricing_plan(now);
        let superseded_pending = self.pending_pricing_plan.take().is_some();
        let update_reason = if superseded_pending {
            UPDATE_REASON_OVERWRITTEN
        } else if pricing_mode != self.pricing_plan.pricing_mode {
            UPDATE_REASON_PRICING_MODE_UPDATE
        } else {
            UPDATE_REASON_PRICING_TIER_UPDATE
        };
        let deferred = pricing_change_is_deferred(&self.pricing_plan.pricing_mode, &pricing_mode);
        let effective_date_time = if deferred {
            now + PRICING_PLAN_PENDING_SPAN
        } else {
            now
        };
        let mut record = PricingPlanRecord {
            pricing_mode,
            update_reason: update_reason.to_string(),
            bundle_names,
            pricing_tier: None,
            effective_date_time,
            update_date_time: now,
        };
        record.pricing_tier = record.tier_at(self.billable_entity_count());
        if deferred {
            self.pending_pricing_plan = Some(record);
        } else {
            self.pricing_plan = record;
        }
    }

    /// Whether the stored pricing plan has a change owed to it as of `now` that
    /// nothing has applied yet — a pending change that has come due, or a tier
    /// the account's billable entity count has moved out from under.
    ///
    /// A REPORTING caller asks this first and only reaches for the store
    /// mutably when the answer is yes, so reading the plan of an account with
    /// nothing owed materialises nothing at all.
    pub fn pricing_plan_settlement_due(&self, now: SystemTime) -> bool {
        if self
            .pending_pricing_plan
            .as_ref()
            .is_some_and(|pending| now >= pending.effective_date_time)
        {
            return true;
        }
        self.pricing_plan.pricing_tier != self.pricing_plan.tier_at(self.billable_entity_count())
    }

    /// Apply everything that is already owed to the stored pricing plan as of
    /// `now`, so what a caller is told is what was true before it called.
    ///
    /// Two things fall due without anybody asking:
    ///
    /// * a change accepted for a later instant becomes the plan in force once
    ///   that instant has passed — it was due then whether or not anybody
    ///   called, so its `effectiveDateTime` and `updateDateTime` are left as
    ///   they were recorded and two reads of a landed change report the same
    ///   instants;
    /// * a plan billed by tier moves band when the account's billable entity
    ///   count crosses a threshold ([`pricing_tier_for`]). The service makes
    ///   that move itself, which is what [`UPDATE_REASON_ENTITY_COUNT_UPDATE`]
    ///   records, so the plan keeps its mode and bundles and only its tier,
    ///   reason and instants move. Idempotent: once the band is recorded,
    ///   nothing further is owed until the count crosses again.
    pub fn settle_pricing_plan(&mut self, now: SystemTime) {
        if let Some(due) = self
            .pending_pricing_plan
            .take_if(|pending| now >= pending.effective_date_time)
        {
            self.pricing_plan = due;
        }
        let tier = self.pricing_plan.tier_at(self.billable_entity_count());
        if self.pricing_plan.pricing_tier != tier {
            self.pricing_plan.pricing_tier = tier;
            self.pricing_plan.update_reason = UPDATE_REASON_ENTITY_COUNT_UPDATE.to_string();
            self.pricing_plan.effective_date_time = now;
            self.pricing_plan.update_date_time = now;
        }
    }

    /// Resolve an `IdOrArn` input to a stored `workspaceId`.
    ///
    /// `GetWorkspace`'s label accepts the workspace id or its ARN (its
    /// `IdOrArn` pattern is the union of both), and both must resolve to the one
    /// stored record.
    pub fn resolve_workspace_id(&self, id_or_arn: &str) -> Option<String> {
        if self.workspaces.contains_key(id_or_arn) {
            return Some(id_or_arn.to_string());
        }
        match self.arn_index.get(id_or_arn) {
            Some(ResourceRef::Workspace { workspace_id }) => Some(workspace_id.clone()),
            _ => None,
        }
    }

    /// The stored tag map of the resource `resource_arn` names, or `None` when
    /// this service holds no resource under that ARN.
    ///
    /// ONE resolution path for every taggable resource: the tags live on the
    /// resource record itself (never in a second map beside it), and the ARN
    /// index says which record an ARN points at, so a workspace, an entity and
    /// a component type are reached the same way and a reader and a writer see
    /// one tag set.
    pub fn tags_for_resource(&self, resource_arn: &str) -> Option<&BTreeMap<String, String>> {
        match self.arn_index.get(resource_arn)? {
            ResourceRef::Workspace { workspace_id } => {
                Some(&self.workspaces.get(workspace_id)?.tags)
            }
            ResourceRef::Entity {
                workspace_id,
                entity_id,
            } => Some(
                &self
                    .workspaces
                    .get(workspace_id)?
                    .entities
                    .get(entity_id)?
                    .tags,
            ),
            ResourceRef::ComponentType {
                workspace_id,
                component_type_id,
            } => Some(
                &self
                    .workspaces
                    .get(workspace_id)?
                    .component_types
                    .get(component_type_id)?
                    .tags,
            ),
            ResourceRef::Scene {
                workspace_id,
                scene_id,
            } => Some(&self.workspaces.get(workspace_id)?.scenes.get(scene_id)?.tags),
            ResourceRef::SyncJob {
                workspace_id,
                sync_source,
            } => Some(
                &self
                    .workspaces
                    .get(workspace_id)?
                    .sync_jobs
                    .get(sync_source)?
                    .tags,
            ),
        }
    }

    /// [`tags_for_resource`](Self::tags_for_resource), for a caller that writes.
    pub fn tags_for_resource_mut(
        &mut self,
        resource_arn: &str,
    ) -> Option<&mut BTreeMap<String, String>> {
        // Cloned out of the index first: the index and the resource records are
        // both fields of `self`, so holding a borrow of one would forbid the
        // mutable borrow of the other.
        let resource = self.arn_index.get(resource_arn)?.clone();
        match resource {
            ResourceRef::Workspace { workspace_id } => {
                Some(&mut self.workspaces.get_mut(&workspace_id)?.tags)
            }
            ResourceRef::Entity {
                workspace_id,
                entity_id,
            } => Some(
                &mut self
                    .workspaces
                    .get_mut(&workspace_id)?
                    .entities
                    .get_mut(&entity_id)?
                    .tags,
            ),
            ResourceRef::ComponentType {
                workspace_id,
                component_type_id,
            } => Some(
                &mut self
                    .workspaces
                    .get_mut(&workspace_id)?
                    .component_types
                    .get_mut(&component_type_id)?
                    .tags,
            ),
            ResourceRef::Scene {
                workspace_id,
                scene_id,
            } => Some(
                &mut self
                    .workspaces
                    .get_mut(&workspace_id)?
                    .scenes
                    .get_mut(&scene_id)?
                    .tags,
            ),
            ResourceRef::SyncJob {
                workspace_id,
                sync_source,
            } => Some(
                &mut self
                    .workspaces
                    .get_mut(&workspace_id)?
                    .sync_jobs
                    .get_mut(&sync_source)?
                    .tags,
            ),
        }
    }
}

/// A stored instant as the modelled `Timestamp` the wire carries.
pub fn timestamp(at: SystemTime) -> ::aws_smithy_types::DateTime {
    let since_epoch = at
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    ::aws_smithy_types::DateTime::from_secs_and_nanos(
        i64::try_from(since_epoch.as_secs()).unwrap_or(i64::MAX),
        since_epoch.subsec_nanos(),
    )
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsIottwinmakerAwsiottwinmakerState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsIottwinmakerAwsiottwinmakerState>)
}
