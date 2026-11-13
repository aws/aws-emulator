// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-controlcatalog-controlcatalog service.
//!
//! ControlCatalog is a read-only catalog: every one of its six operations reports
//! what Amazon Web Services publishes, and none of them creates, updates or deletes
//! anything. The catalog therefore lives in this service's state, seeded once (the
//! engine builds the state through `Default` on first touch, atomically) with the
//! ontology the catalog publishes — every domain, every objective under it and every
//! common control under those objectives, in [`published_ontology`] — together with a
//! deterministic set of controls and control mappings. Every handler projects its
//! response out of these records, so two
//! processes talking to one emulator — and two runs of the same emulator — see the
//! same catalog.
//!
//! The collections are `BTreeMap`s keyed by ARN: that key order is the stable total
//! order the paginated list operations walk, and it is what a `NextToken` resumes
//! after (see [`page`]).

use std::collections::BTreeMap;

use sha2::{Digest, Sha256};
use smithy_server_runtime::{Violation, ViolationKind};

/// The minimum every one of this service's `MaxList*Results` shapes declares with
/// `@range` (`min: 1`); only the maximum differs between operations.
const MIN_MAX_RESULTS: i32 = 1;

/// The validation path of the page-size member, as every list operation's input
/// spells it (`MaxResults`, lowerCamel in a message path).
const MAX_RESULTS_MEMBER: &str = "maxResults";

/// The controls the catalog publishes, as data.
mod published_controls;
/// The domain/objective/common-control ontology the catalog publishes, as data.
mod published_ontology;

/// Every compliance framework the catalog cross-walks its controls into, named by the
/// designator a FRAMEWORK control mapping carries for it.
pub const PUBLISHED_FRAMEWORKS: &[&str] = published_controls::PUBLISHED_FRAMEWORKS;

/// A catalog domain — the top of the catalog hierarchy.
#[derive(Debug, Clone)]
pub struct Domain {
    pub arn: String,
    pub name: String,
    pub description: String,
    /// Seconds since the Unix epoch; rendered as the modeled `Timestamp`.
    pub create_time: i64,
    pub last_update_time: i64,
}

/// A catalog objective, which always belongs to exactly one [`Domain`].
#[derive(Debug, Clone)]
pub struct Objective {
    pub arn: String,
    pub name: String,
    pub description: String,
    pub domain_arn: String,
    pub create_time: i64,
    pub last_update_time: i64,
}

/// A common control, which belongs to one [`Objective`] (and so to that
/// objective's domain).
#[derive(Debug, Clone)]
pub struct CommonControl {
    pub arn: String,
    pub name: String,
    pub description: String,
    pub objective_arn: String,
    pub domain_arn: String,
    pub create_time: i64,
    pub last_update_time: i64,
}

/// Where a control can be deployed: the modeled `RegionConfiguration`.
#[derive(Debug, Clone)]
pub struct RegionConfig {
    /// `ControlScope`: `GLOBAL` or `REGIONAL`.
    pub scope: String,
    /// Regions the control can be deployed in. Empty for a `GLOBAL` control: the
    /// GetControl documentation says a control whose `Scope` is `GLOBAL` does not
    /// show `DeployableRegions`, because the field does not apply.
    pub deployable_regions: Vec<String>,
}

/// How a control is implemented: the modeled `ImplementationDetails` /
/// `ImplementationSummary` (the two shapes carry the same two members).
#[derive(Debug, Clone)]
pub struct Implementation {
    /// `ImplementationType`, e.g. `AWS::Config::ConfigRule`.
    pub implementation_type: String,
    /// `ImplementationIdentifier`, the implementing service's own id for the control.
    pub identifier: Option<String>,
}

/// One parameter a control accepts: the modeled `ControlParameter`.
#[derive(Debug, Clone)]
pub struct ControlParam {
    pub name: String,
    /// `ControlParameterRequirement`: `REQUIRED` or `OPTIONAL`.
    pub requirement: Option<String>,
}

/// A catalog control, as both GetControl and ListControls report it — the list
/// summary is a projection of this same record.
#[derive(Debug, Clone)]
pub struct Control {
    /// The `controlcatalog` ARN. GetControl always answers with this form, whichever
    /// accepted form the caller asked with.
    pub arn: String,
    /// The Control Tower control identifier (the segment after `control/` in an
    /// `arn:aws:controltower:{REGION}::control/{ID}` ARN), when this control is also
    /// a Control Tower control. GetControl accepts that ARN form as input.
    pub control_tower_identifier: Option<String>,
    pub aliases: Vec<String>,
    pub name: String,
    pub description: String,
    /// `ControlBehavior`: `PREVENTIVE`, `PROACTIVE` or `DETECTIVE`.
    pub behavior: String,
    /// `ControlSeverity`: `LOW`, `MEDIUM`, `HIGH` or `CRITICAL`.
    pub severity: Option<String>,
    pub region_configuration: RegionConfig,
    pub implementation: Option<Implementation>,
    /// `ParameterRequirementSummary`: `REQUIRED`, `OPTIONAL` or `NONE`.
    pub parameter_requirement_summary: Option<String>,
    pub parameters: Vec<ControlParam>,
    pub create_time: i64,
    pub governed_resources: Vec<String>,
    pub governed_providers: Vec<String>,
}

/// The target of a control mapping: the modeled `Mapping` union.
#[derive(Debug, Clone)]
pub enum MappingDetails {
    Framework { name: String, item: String },
    CommonControl { common_control_arn: String },
    RelatedControl {
        control_arn: Option<String>,
        /// `ControlRelationType`: `COMPLEMENTARY`, `ALTERNATIVE` or `MUTUALLY_EXCLUSIVE`.
        relation_type: String,
    },
}

impl MappingDetails {
    /// The `MappingType` of a mapping with these details.
    ///
    /// A mapping's kind is not an independent fact about it: the modeled `Mapping` is a
    /// union, which carries exactly one member, and `ControlMapping.MappingType` is
    /// documented as "[t]he type of mapping relationship between the control and other
    /// entities" — so the type names the member the union carries, one `MappingType`
    /// value per union member. Reading it off the details here is what keeps a mapping
    /// from ever being reported under a type its details do not match.
    pub fn mapping_type(&self) -> &'static str {
        match self {
            Self::Framework { .. } => "FRAMEWORK",
            Self::CommonControl { .. } => "COMMON_CONTROL",
            Self::RelatedControl { .. } => "RELATED_CONTROL",
        }
    }

    /// What the mapping maps to, as the text that distinguishes two mappings of one
    /// control and kind from each other — the framework item, the common control's ARN,
    /// or the related control's ARN with the relation. Part of the key the catalog
    /// stores a mapping under (see [`ComAmazonawsControlcatalogControlcatalogState::control_mappings`]).
    fn target(&self) -> String {
        match self {
            Self::Framework { name, item } => format!("{name}/{item}"),
            Self::CommonControl { common_control_arn } => common_control_arn.clone(),
            Self::RelatedControl {
                control_arn,
                relation_type,
            } => format!(
                "{}/{relation_type}",
                control_arn.clone().unwrap_or_default()
            ),
        }
    }
}

/// A control mapping: a control, and what it maps to. The kind of mapping is a
/// function of the latter (see [`MappingDetails::mapping_type`]), so it is not stored
/// beside it.
#[derive(Debug, Clone)]
pub struct ControlMappingRecord {
    pub control_arn: String,
    pub mapping: MappingDetails,
}

impl ControlMappingRecord {
    /// This mapping's `MappingType`: `FRAMEWORK`, `COMMON_CONTROL` or
    /// `RELATED_CONTROL`, read off the details it carries.
    pub fn mapping_type(&self) -> &'static str {
        self.mapping.mapping_type()
    }
}

/// Per-service state for com-amazonaws-controlcatalog-controlcatalog. A handler reaches it through [`service_state`].
#[derive(Debug)]
pub struct ComAmazonawsControlcatalogControlcatalogState {
    /// Keyed by domain ARN.
    pub domains: BTreeMap<String, Domain>,
    /// Keyed by objective ARN.
    pub objectives: BTreeMap<String, Objective>,
    /// Keyed by common control ARN.
    pub common_controls: BTreeMap<String, CommonControl>,
    /// Keyed by the control's `controlcatalog` ARN.
    pub controls: BTreeMap<String, Control>,
    /// Keyed by a synthesized mapping key (`<control arn>|<mapping type>|<target>`),
    /// because a mapping has no ARN of its own yet still needs the stable total
    /// order ListControlMappings' `NextToken` resumes in.
    pub control_mappings: BTreeMap<String, ControlMappingRecord>,
}

impl Default for ComAmazonawsControlcatalogControlcatalogState {
    fn default() -> Self {
        seed_catalog()
    }
}

impl ComAmazonawsControlcatalogControlcatalogState {
    /// Resolve an accepted control ARN to the stored control.
    ///
    /// GetControl's documentation: it "accepts controltower or controlcatalog control
    /// ARNs as input" and "returns a controlcatalog ARN format", and gives the two
    /// accepted forms as `arn:{PARTITION}:controlcatalog:::control/
    /// {CONTROL_CATALOG_OPAQUE_ID}` and `arn:{PARTITION}:controltower:{REGION}::control/
    /// {CONTROL_TOWER_OPAQUE_ID}`. What names the control is therefore the identifier
    /// after `control/`, read in the namespace the service segment selects; `{PARTITION}`
    /// says which partition the caller writes the ARN in and `{REGION}` where the control
    /// was enabled, so neither segment is part of a control's identity. An ARN of either
    /// form whose identifier the catalog publishes resolves, whatever partition it is
    /// written in — the catalog is global, and publishes one control per identifier.
    pub fn resolve_control(&self, requested_arn: &str) -> Option<&Control> {
        match ControlArnRef::parse(requested_arn)? {
            ControlArnRef::Catalog(identifier) => {
                self.controls.get(&catalog_arn("control", identifier))
            }
            ControlArnRef::ControlTower(identifier) => self.controls.values().find(|control| {
                control
                    .control_tower_identifier
                    .as_deref()
                    .is_some_and(|stored| stored == identifier)
            }),
        }
    }

    /// The ARN this catalog stores for the control that `requested_arn` names, read by
    /// the crate's one resolve-or-keep rule (`canonical_arn`).
    ///
    /// The control kind is the one kind whose resolution is more than reading a global
    /// catalog ARN, because a control may also be named by its Control Tower ARN form
    /// (see [`Self::resolve_control`]); which of the two forms the caller wrote is all
    /// that is per-kind here, so a filter reading a control ARN names only the dimension
    /// it is reading. The counterpart for every other kind is
    /// [`canonical_catalog_arn`].
    pub fn canonical_control_arn(&self, requested_arn: &str) -> String {
        canonical_arn(
            requested_arn,
            self.resolve_control(requested_arn)
                .map(|control| control.arn.clone()),
        )
    }
}

/// What an accepted control ARN names: the identifier it carries, in the namespace its
/// service segment selects. Any other ARN names no control at all.
#[derive(Debug)]
enum ControlArnRef<'a> {
    /// A `controlcatalog` ARN's `{CONTROL_CATALOG_OPAQUE_ID}` — the opaque identifier
    /// this catalog publishes a control under.
    Catalog(&'a str),
    /// A `controltower` ARN's `{CONTROL_TOWER_OPAQUE_ID}` — the Control Tower identifier
    /// of a control that is also a Control Tower control.
    ControlTower(&'a str),
}

impl<'a> ControlArnRef<'a> {
    /// Read an ARN as the control it names, or `None` when it names none.
    ///
    /// The global catalog form is read by [`catalog_identifier`] — the one helper every
    /// caller of a catalog ARN goes through, [`canonical_catalog_arn`] included — so what
    /// a global `controlcatalog` ARN names is decided in a single place for this
    /// resolution and for every list filter alike. What is left here is the namespace
    /// this type genuinely owns: `controltower`, whose ARNs do carry the Region the
    /// control was enabled in.
    fn parse(arn: &'a str) -> Option<Self> {
        if let Some(identifier) = catalog_identifier("control", arn) {
            return Some(Self::Catalog(identifier));
        }
        let parsed = AccountlessArn::parse(arn)?;
        if parsed.service != "controltower" {
            return None;
        }
        kind_identifier("control", parsed.resource).map(Self::ControlTower)
    }
}

/// The segments that carry meaning in an ARN this service accepts: none of the forms
/// its modeled ARN patterns admit carries an account id, so each is
/// `arn:{PARTITION}:{SERVICE}:{REGION}::{RESOURCE}`.
///
/// Parsing drops the partition because no modeled ARN pattern here distinguishes one:
/// every pattern restricts that segment to the `aws` family (`aws(?:[-a-z]*)?`) and
/// says nothing more about it, so the segment is checked for membership of the family
/// ([`is_aws_partition`]) and then carries no information about which resource the ARN
/// names. The Region segment is read the same way: it is checked against the class the
/// patterns admit there ([`is_modeled_region`]), and what it then decides is only
/// whether the ARN is the global catalog form (an EMPTY Region) or a regional
/// `controltower` one.
///
/// A segment is therefore accepted only when the WHOLE of it is a spelling the modeled
/// patterns admit. Both of these segments are dropped once read, so a spelling outside
/// the modeled class would otherwise name a published resource as though the caller had
/// written a modeled ARN; the patterns refuse such an ARN before a handler runs, so it
/// is unreachable from the wire, and refusing it here too means an in-process caller
/// gets the same answer as a client, in every build profile.
#[derive(Debug)]
struct AccountlessArn<'a> {
    service: &'a str,
    region: &'a str,
    resource: &'a str,
}

impl<'a> AccountlessArn<'a> {
    /// Split an ARN into its meaningful segments, or `None` when it is not an ARN of
    /// this shape at all.
    fn parse(arn: &'a str) -> Option<Self> {
        let mut fields = arn.splitn(6, ':');
        let (
            Some("arn"),
            Some(partition),
            Some(service),
            Some(region),
            // No accepted form carries an account id.
            Some(""),
            Some(resource),
        ) = (
            fields.next(),
            fields.next(),
            fields.next(),
            fields.next(),
            fields.next(),
            fields.next(),
        )
        else {
            return None;
        };
        if !is_aws_partition(partition) || !is_modeled_region(region) {
            return None;
        }
        Some(Self {
            service,
            region,
            resource,
        })
    }
}

/// Is `partition` a member of the partition family every modeled ARN pattern here
/// restricts that segment to — `aws(?:[-a-z]*)?`, the letters `aws` followed by any
/// number of lowercase letters and hyphens and nothing else
/// (`smithy_query(query="shape", shape="ControlArn,DomainArn,ObjectiveArn,CommonControlArn")`)?
///
/// So `aws` itself and every partition spelled in that class (`aws-cn`, `aws-us-gov`,
/// `aws-iso-e`) belong to it, while a segment carrying anything else — a digit
/// (`aws1`), an uppercase letter (`awsX`, `aws-CN`), an underscore (`aws_cn`) — or one
/// that does not open with `aws` at all does not, whatever prefix it shares with the
/// family.
fn is_aws_partition(partition: &str) -> bool {
    partition
        .strip_prefix("aws")
        .is_some_and(|rest| rest.chars().all(|c| c == '-' || c.is_ascii_lowercase()))
}

/// Is `region` a Region segment the modeled ARN patterns admit? `ControlArn` allows
/// `[a-zA-Z0-9-]*` there — letters of either case, digits and hyphens — and the three
/// ontology patterns pin `controlcatalog:::`, so the empty segment, which that class
/// contains, is the only one they admit
/// (`smithy_query(query="shape", shape="ControlArn,DomainArn,ObjectiveArn,CommonControlArn")`).
fn is_modeled_region(region: &str) -> bool {
    region.chars().all(|c| c == '-' || c.is_ascii_alphanumeric())
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsControlcatalogControlcatalogState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsControlcatalogControlcatalogState>)
}

// ---------------------------------------------------------------------------
// ARNs and identifiers
// ---------------------------------------------------------------------------

/// Build a ControlCatalog ARN for `kind`/`id`.
///
/// ControlCatalog ARNs carry neither a Region nor an account id — the modeled
/// `DomainArn` pattern is `^arn:(aws(?:[-a-z]*)?):controlcatalog:::domain/[0-9a-z]+$`,
/// and `ControlArn`, `ObjectiveArn` and `CommonControlArn` have the same empty
/// Region and account segments — so `engine::emulator_arn`, which always writes the
/// emulator's account id into the ARN, cannot produce one. The format below is the
/// model's own.
fn catalog_arn(kind: &str, id: &str) -> String {
    format!("arn:aws:controlcatalog:::{kind}/{id}")
}

/// The crate's one rule for reading a caller-supplied ARN before it is compared with a
/// stored one: the ARN the catalog stores for the resource the caller's ARN names, as
/// `resolved` carries it, and otherwise the ARN exactly as sent.
///
/// An ARN that names nothing the catalog publishes is therefore returned unchanged, so
/// it goes on matching nothing: the modeled pattern on each member that accepts one
/// refuses such an ARN at the boundary anyway. Every kind reaches this rule through one
/// of the two resolutions above it —
/// [`ComAmazonawsControlcatalogControlcatalogState::canonical_control_arn`] for a
/// control, [`canonical_catalog_arn`] for every other kind — so the rule is stated once
/// and cannot drift between them.
fn canonical_arn(requested_arn: &str, resolved: Option<String>) -> String {
    resolved.unwrap_or_else(|| requested_arn.to_string())
}

/// The ARN this catalog stores for the resource of `kind` that `requested_arn` names,
/// read by the crate's one resolve-or-keep rule (`canonical_arn`).
///
/// What names a catalog resource is its kind and the opaque identifier after
/// `{kind}/`, exactly as it is what names a control (see
/// [`ComAmazonawsControlcatalogControlcatalogState::resolve_control`]): the
/// `{PARTITION}` segment says which partition the caller writes the ARN in, and the
/// catalog is global, publishing one resource per kind and identifier. So every ARN
/// of that kind the modeled patterns admit — any partition of the `aws` family —
/// names the same resource, and a caller's ARN is read through here before it is
/// compared with a stored one.
pub fn canonical_catalog_arn(kind: &str, requested_arn: &str) -> String {
    canonical_arn(
        requested_arn,
        catalog_identifier(kind, requested_arn).map(|identifier| catalog_arn(kind, identifier)),
    )
}

/// The opaque identifier a `controlcatalog` ARN of `kind` carries, or `None` when the
/// ARN names no resource of that kind.
fn catalog_identifier<'a>(kind: &str, arn: &'a str) -> Option<&'a str> {
    let parsed = AccountlessArn::parse(arn)?;
    // The catalog is global, so its own ARNs carry no Region segment.
    if parsed.service != "controlcatalog" || !parsed.region.is_empty() {
        return None;
    }
    kind_identifier(kind, parsed.resource)
}

/// The identifier an ARN's resource segment carries for `kind` — what follows `{kind}/`
/// — or `None` when the segment names no resource of that kind, an empty identifier
/// included.
///
/// The resource segment is read the same way in either namespace an accepted ARN can
/// select, so both [`catalog_identifier`] and [`ControlArnRef::parse`]'s `controltower`
/// branch read it through here.
fn kind_identifier<'a>(kind: &str, resource: &'a str) -> Option<&'a str> {
    let identifier = resource
        .strip_prefix(kind)
        .and_then(|rest| rest.strip_prefix('/'))?;
    (!identifier.is_empty()).then_some(identifier)
}

/// Length of the opaque identifier at the end of a catalog ARN. The modeled ARN
/// patterns admit `[0-9a-z]+`; AWS's published catalog ARNs (e.g.
/// `arn:aws:controlcatalog:::control/5kvme4m5d2b4d7if2fs5yg2ui`, quoted in the
/// `ControlParameter` documentation) use 25 characters.
const CATALOG_ID_LEN: usize = 25;

/// A deterministic opaque catalog identifier for `slug`.
///
/// Derived by hashing, so the identifier — and therefore every ARN in the catalog —
/// is identical in every process and on every run, which is what lets a client hold
/// an ARN across sessions. Hex digits are a subset of the `[0-9a-z]` the ARN
/// patterns allow.
fn catalog_id(slug: &str) -> String {
    let digest = Sha256::digest(slug.as_bytes());
    hex::encode(digest)[..CATALOG_ID_LEN].to_string()
}

/// The opaque identifier the catalog's ARN carries for one published entry: the
/// catalog's own published identifier, where this crate records one for the entry, and
/// otherwise one minted from the entry's slug ([`catalog_id`]).
///
/// The catalog is account-independent and read-only, so a published identifier names the
/// entry for every caller, in every partition and on every run — which is what lets a
/// client hold an ARN across sessions and feed it to a filter or to `GetControl`. The
/// rule is the same for every kind the catalog publishes, so it is stated once here and
/// read through the two functions below.
fn published_or_minted_identifier(published: Option<&'static str>, slug: &str) -> String {
    published.map_or_else(|| catalog_id(slug), str::to_string)
}

/// The opaque identifier the catalog's control ARN carries for the published control
/// `slug` names (`published_controls::published_catalog_identifier`).
fn control_identifier(slug: &str) -> String {
    published_or_minted_identifier(published_controls::published_catalog_identifier(slug), slug)
}

/// The opaque identifier the catalog's ARN carries for the ontology entry of `kind` that
/// `slug` names — a domain, an objective or a common control
/// (`published_ontology::published_catalog_identifier`).
fn ontology_identifier(kind: &str, slug: &str) -> String {
    published_or_minted_identifier(
        published_ontology::published_catalog_identifier(kind, slug),
        slug,
    )
}

// ---------------------------------------------------------------------------
// List filters
// ---------------------------------------------------------------------------

/// One entry of a single-ARN resource filter: the structure a filtered list operation
/// declares for the resource kind it narrows to, carrying that resource's ARN.
///
/// The service models one such structure per filtered kind, each with a single
/// optional `Arn` member
/// (`smithy_query(query="shape",shape="DomainResourceFilter,ObjectiveResourceFilter")`),
/// so this trait is what lets the one rule below read either of them.
pub trait ResourceFilterEntry {
    /// The ARN the entry names, or `None` when the caller set no `Arn` on it.
    fn filter_arn(&self) -> Option<&str>;
}

impl ResourceFilterEntry for crate::ops::types::DomainResourceFilter {
    fn filter_arn(&self) -> Option<&str> {
        self.arn.as_deref()
    }
}

impl ResourceFilterEntry for crate::ops::types::ObjectiveResourceFilter {
    fn filter_arn(&self) -> Option<&str> {
        self.arn.as_deref()
    }
}

/// The one resource ARN a single-ARN resource filter narrows to, or the sentence a
/// filter this service does not accept is refused with.
///
/// Every list operation that takes a resource filter takes the same kind of filter,
/// so the rule is this one function rather than one copy per operation: the filter
/// list carries at most ONE entry (each filter member's documentation says the
/// operation accepts one ARN at a time and that passing several is not supported) and
/// that entry must name the resource to narrow to. Neither filter list declares a
/// `@length` and `Arn` is optional on the entry, so the boundary validator admits
/// both a longer list and an entry naming nothing, and this is where each is refused.
///
/// `Ok(None)` — narrow nothing — is the answer for a caller that sent no filter list
/// at all and for one that sent an empty list, neither of which names a resource to
/// narrow by. An accepted ARN is read through [`canonical_catalog_arn`] for `kind`
/// before it is returned, so what it names is compared with the catalog by the
/// resource's identity rather than by the partition the caller wrote it in.
///
/// The refusal is the service's one fixed invalid-filter sentence
/// ([`crate::service::INVALID_FILTER_MESSAGE`]): it names neither the filter member
/// at fault nor the number of entries sent, so there is nothing to word per cause or
/// per operation and the sentence itself is the whole error. A handler lifts it into
/// its own modeled `ValidationException` — the only part of the refusal that is
/// per-operation.
pub fn single_resource_filter_arn<Entry: ResourceFilterEntry>(
    kind: &str,
    entries: Option<&[Entry]>,
) -> Result<Option<String>, String> {
    let Some(entries) = entries else {
        return Ok(None);
    };
    if entries.len() > 1 {
        return Err(crate::service::INVALID_FILTER_MESSAGE.to_string());
    }
    let Some(entry) = entries.first() else {
        return Ok(None);
    };
    let Some(arn) = entry.filter_arn() else {
        return Err(crate::service::INVALID_FILTER_MESSAGE.to_string());
    };
    Ok(Some(canonical_catalog_arn(kind, arn)))
}

/// A single-ARN resource filter a list operation received, read: the zero-or-one
/// canonical parent ARN it narrows to, under the name of the page-fingerprint dimension
/// it contributes.
///
/// Every list operation that takes such a filter narrows and fingerprints by the same
/// rule, so the rule is this one type rather than one copy per operation: the filter
/// contributes exactly ONE `(dimension, values)` pair to the page fingerprint — the
/// resolved ARN, or no value at all for a filter that narrows nothing — and it admits
/// exactly those children whose stored parent ARN is that ARN, every child when it
/// narrows nothing. The dimension carries the resolved ARN rather than the caller's
/// spelling of it, so a `NextToken` stays valid for every spelling of the same resource
/// (see [`canonical_catalog_arn`]) and is refused for a different one.
///
/// What stays per-operation is only what the model makes per-operation: the filter
/// member the entries came from and which stored member of the listed record holds the
/// parent ARN.
#[derive(Debug, Clone)]
pub struct SingleResourceFilter {
    /// The fingerprint dimension this filter contributes, named after the filter
    /// structure's list member (lowerCamel, as [`filter_fingerprint`]'s other callers
    /// name theirs).
    dimension: &'static str,
    /// The resource ARN to narrow to, or `None` for a filter that narrows nothing.
    parent_arn: Option<String>,
}

impl SingleResourceFilter {
    /// Read the filter `entries` a list operation received, whose entries name a
    /// resource of `kind` and which contributes the fingerprint dimension `dimension`.
    ///
    /// Which entry counts, which entries are acceptable, the sentence an unacceptable
    /// filter is refused with and reading the accepted ARN by the resource's identity
    /// are [`single_resource_filter_arn`]'s rule, unchanged; the caller lifts the
    /// refusal into its own modeled `ValidationException`.
    pub fn read<Entry: ResourceFilterEntry>(
        kind: &str,
        dimension: &'static str,
        entries: Option<&[Entry]>,
    ) -> Result<Self, String> {
        Ok(Self {
            dimension,
            parent_arn: single_resource_filter_arn(kind, entries)?,
        })
    }

    /// The page fingerprint of this filter: its one dimension, carrying the one ARN it
    /// narrows to or no value at all.
    pub fn fingerprint(&self) -> String {
        filter_fingerprint(&[(
            self.dimension,
            self.parent_arn.clone().into_iter().collect(),
        )])
    }

    /// Does a listed record whose stored parent ARN is `parent_arn` survive this
    /// filter? A filter that narrows nothing admits every record.
    pub fn admits(&self, parent_arn: &str) -> bool {
        self.parent_arn
            .as_deref()
            .is_none_or(|narrowed_to| parent_arn == narrowed_to)
    }
}

// ---------------------------------------------------------------------------
// Parent summaries
// ---------------------------------------------------------------------------

/// A catalog record another record names as the parent it belongs to: the [`Domain`] an
/// objective or a common control belongs to, and the [`Objective`] a common control
/// supports.
///
/// The only thing a parent summary reports out of the parent record is its name, so
/// that is all this trait exposes.
pub trait CatalogParent {
    /// The parent's stored name.
    fn parent_name(&self) -> &str;
}

impl CatalogParent for Domain {
    fn parent_name(&self) -> &str {
        &self.name
    }
}

impl CatalogParent for Objective {
    fn parent_name(&self) -> &str {
        &self.name
    }
}

/// One of the modeled `Associated*Summary` structures: the summary of a parent record a
/// listed child carries.
///
/// The service models one such structure per parent kind, each with the same two
/// optional members, `Arn` and `Name`
/// (`smithy_query(query="shape",shape="AssociatedDomainSummary,AssociatedObjectiveSummary")`),
/// so this trait is what lets the one rule below build either of them.
pub trait AssociatedParentSummary {
    /// The summary reporting `arn` and `name`.
    fn from_parts(arn: String, name: Option<String>) -> Self;
}

impl AssociatedParentSummary for crate::ops::types::AssociatedDomainSummary {
    fn from_parts(arn: String, name: Option<String>) -> Self {
        Self {
            arn: Some(arn),
            name,
        }
    }
}

impl AssociatedParentSummary for crate::ops::types::AssociatedObjectiveSummary {
    fn from_parts(arn: String, name: Option<String>) -> Self {
        Self {
            arn: Some(arn),
            name,
        }
    }
}

/// The summary of the parent `parent_arn` names, resolved out of `parents` — the same
/// catalog collection the parent's own list operation reports.
///
/// Every listing that carries a parent summary carries the same kind of summary, so the
/// rule is this one function rather than one copy per operation and member: the `Arn` is
/// the ARN the CHILD stores, reported exactly as stored so every listing describes one
/// parent the same way, and the `Name` is the stored parent's, reported only when that
/// ARN resolves in the collection — both members are optional on every modeled
/// `Associated*Summary`, so a parent the catalog does not hold leaves the name out
/// instead of failing the listing.
pub fn associated_parent_summary<Parent, Summary>(
    parents: &BTreeMap<String, Parent>,
    parent_arn: &str,
) -> Summary
where
    Parent: CatalogParent,
    Summary: AssociatedParentSummary,
{
    Summary::from_parts(
        parent_arn.to_string(),
        parents
            .get(parent_arn)
            .map(|parent| parent.parent_name().to_string()),
    )
}

// ---------------------------------------------------------------------------
// Pagination
// ---------------------------------------------------------------------------

/// One page of a list operation's results.
#[derive(Debug)]
pub struct Page<T> {
    pub items: Vec<T>,
    /// Set only when items remain after this page.
    pub next_token: Option<String>,
}

/// Why a page could not be taken. Both variants are client-input failures a handler
/// answers with the service's modeled `ValidationException`.
#[derive(Debug)]
pub enum PageError {
    /// A `NextToken` this collection and filter cannot resume from.
    InvalidToken,
    /// A `MaxResults` outside the operation's modeled range. The modeled `@range`
    /// refuses such a value before a handler runs, so it is unreachable from the
    /// wire; refusing it here too means an in-process caller gets the same answer as
    /// a client, in every build profile, rather than a page quietly sized differently
    /// from the one it asked for.
    MaxResultsOutOfRange { requested: i32, modeled_max: i32 },
}

/// A short, stable fingerprint of the filter a page was produced under.
///
/// A `NextToken` is only meaningful for the collection and filter that produced it:
/// replaying it against a different filter would resume in a sequence the token's key
/// may not even belong to. The fingerprint travels inside the token so such a replay
/// is refused instead of silently answering from the wrong sequence.
///
/// Every list operation builds it the same way — one `(dimension, values)` pair per
/// dimension its filter declares, in the order the model declares them, with the
/// values the caller set (none for a dimension the caller omitted) — so the input to
/// the hash is canonical across the service rather than hand-rolled per operation.
pub fn filter_fingerprint(dimensions: &[(&str, Vec<String>)]) -> String {
    let canonical = dimensions
        .iter()
        .map(|(dimension, values)| format!("{dimension}=[{}]", values.join(",")))
        .collect::<Vec<_>>()
        .join(";");
    let digest = Sha256::digest(canonical.as_bytes());
    hex::encode(digest)[..16].to_string()
}

/// Field separator inside a decoded token. A unit separator cannot appear in an ARN,
/// a collection name or a fingerprint, so the three fields always split cleanly.
const TOKEN_SEPARATOR: char = '\u{1f}';

/// Encode an opaque `NextToken` for the given collection, filter and last-returned key.
fn encode_page_token(collection: &str, fingerprint: &str, last_key: &str) -> String {
    hex::encode(format!(
        "{collection}{TOKEN_SEPARATOR}{fingerprint}{TOKEN_SEPARATOR}{last_key}"
    ))
}

/// Decode a `NextToken` back to the key it resumes after, refusing one that was not
/// issued for this collection and filter.
///
/// A token is this service's only when it is spelled exactly as
/// [`encode_page_token`] mints it. The hex reader accepts several spellings of the
/// same bytes — upper-, lower- and mixed-case digits — while the writer emits only
/// lowercase ones, so a token is compared against the one spelling the encoder can
/// produce for the bytes it carries: every other spelling is a token no previous
/// call handed out, and is refused like any other made-up token rather than served
/// as a cursor.
fn decode_page_token(
    token: &str,
    collection: &str,
    fingerprint: &str,
) -> Result<String, PageError> {
    let decoded = hex::decode(token).map_err(|_| PageError::InvalidToken)?;
    if hex::encode(&decoded) != token {
        return Err(PageError::InvalidToken);
    }
    let decoded = String::from_utf8(decoded).map_err(|_| PageError::InvalidToken)?;
    let mut fields = decoded.split(TOKEN_SEPARATOR);
    let (Some(token_collection), Some(token_fingerprint), Some(last_key), None) = (
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
    ) else {
        return Err(PageError::InvalidToken);
    };
    if token_collection != collection || token_fingerprint != fingerprint {
        return Err(PageError::InvalidToken);
    }
    Ok(last_key.to_string())
}

/// What Control Catalog answers a `NextToken` it did not issue, or one that has expired,
/// with: ONE fixed sentence for the whole class.
///
/// The whole message is fixed text, with no part taken from the request: it names neither
/// the submitted token, nor the operation, nor the parameters the token was issued for — so
/// every one of this service's five paginated listings refuses an unusable cursor with
/// byte-identical text.
pub const INVALID_NEXT_TOKEN_MESSAGE: &str = "The NextToken parameter is not valid or expired.";

/// How a list operation words a [`PageError`] in the service's modeled
/// `ValidationException`, built the same way for all five list operations.
///
/// A `MaxResults` outside the modeled range is a violation of that `@range`, so it is
/// worded by the one renderer this service words every constraint violation with
/// ([`crate::service::constraint_envelope`]) rather than by a sentence of its own —
/// member and constraint, with no rendering of the rejected value. An unusable
/// `NextToken` is the one fixed sentence [`INVALID_NEXT_TOKEN_MESSAGE`] states.
pub fn page_refusal(error: PageError) -> String {
    match error {
        PageError::InvalidToken => INVALID_NEXT_TOKEN_MESSAGE.to_string(),
        PageError::MaxResultsOutOfRange {
            requested,
            modeled_max,
        } => {
            // The modeled range is `MIN_MAX_RESULTS..=modeled_max`, so a refused value
            // is either below the minimum or above the maximum.
            let kind = if requested < MIN_MAX_RESULTS {
                ViolationKind::MinValue(MIN_MAX_RESULTS.to_string())
            } else {
                ViolationKind::MaxValue(modeled_max.to_string())
            };
            // `value` is `None` because this service's renderer never states the
            // rejected value: the member WAS present, it is simply not rendered.
            let violation = Violation {
                path: MAX_RESULTS_MEMBER.to_string(),
                kind,
                value: None,
                sensitive: false,
            };
            crate::service::constraint_envelope(std::slice::from_ref(&violation)).unwrap_or_else(
                || {
                    // The renderer words every numeric bound, so this is unreachable;
                    // wording it here too keeps a refusal from ever being answered with
                    // no message at all.
                    format!(
                        "1 validation error detected: Value at '{MAX_RESULTS_MEMBER}' failed to \
                         satisfy constraint: Member must have value between \
                         {MIN_MAX_RESULTS} and {modeled_max}"
                    )
                },
            )
        }
    }
}

/// Take one page out of an ordered `(key, item)` sequence.
///
/// `max_results` is the caller's page size, refused when it falls outside the
/// operation's own modeled `MaxResults` range; with none sent, a page holds up to
/// `modeled_max`, the maximum that range declares. A `NextToken` is returned only when
/// items remain, and resuming with it starts at the item after the last one returned.
pub fn page<T>(
    collection: &str,
    fingerprint: &str,
    ordered: impl IntoIterator<Item = (String, T)>,
    next_token: Option<&str>,
    max_results: Option<i32>,
    modeled_max: i32,
) -> Result<Page<T>, PageError> {
    if let Some(requested) = max_results {
        if !(MIN_MAX_RESULTS..=modeled_max).contains(&requested) {
            return Err(PageError::MaxResultsOutOfRange {
                requested,
                modeled_max,
            });
        }
    }
    let page_size = max_results.unwrap_or(modeled_max) as usize;
    let resume_after = match next_token {
        Some(token) => Some(decode_page_token(token, collection, fingerprint)?),
        None => None,
    };

    let mut skipping = resume_after.is_some();
    let mut items = Vec::new();
    let mut last_key = String::new();
    let mut more_remain = false;

    for (key, item) in ordered {
        if skipping {
            if resume_after.as_deref() == Some(key.as_str()) {
                skipping = false;
            }
            continue;
        }
        if items.len() == page_size {
            more_remain = true;
            break;
        }
        last_key = key;
        items.push(item);
    }

    if skipping {
        // The key the token names is not in this sequence at all, so there is no
        // position to resume from.
        return Err(PageError::InvalidToken);
    }

    let next_token = more_remain.then(|| encode_page_token(collection, fingerprint, &last_key));
    Ok(Page { items, next_token })
}

// ---------------------------------------------------------------------------
// The seeded catalog
// ---------------------------------------------------------------------------

/// Base instant for the seeded catalog's timestamps: 2023-11-27T00:00:00Z, expressed
/// in seconds since the Unix epoch. Fixed, so timestamps are reproducible across runs.
const CATALOG_EPOCH: i64 = 1_701_043_200;

/// One day in seconds, the step between seeded catalog timestamps.
const DAY: i64 = 86_400;

/// Build the catalog this emulator serves.
///
/// The ontology (domains, objectives, common controls) is the whole published table in
/// [`published_ontology`], not a sample of it, because a client walks domain ->
/// objective -> common control for any domain the catalog publishes.
///
/// Referentially closed: every objective names a domain that is in `domains`, every
/// common control names an objective and a domain that are in `objectives` /
/// `domains`, and every mapping names a control (and, for a common-control or
/// related-control mapping, a common control or control) that is in `controls` /
/// `common_controls`. Each list/get operation therefore returns entries the others
/// can resolve.
fn seed_catalog() -> ComAmazonawsControlcatalogControlcatalogState {
    let mut domains = BTreeMap::new();
    let mut objectives = BTreeMap::new();
    let mut common_controls = BTreeMap::new();
    let mut controls = BTreeMap::new();
    let mut control_mappings = BTreeMap::new();

    // --- the published ontology: domains, objectives, common controls ------
    //
    // ONE pass over the published ontology, which carries the parentage with it: an
    // objective is inserted under the domain whose entry holds it, and a common
    // control under the objective whose entry holds it. The catalog therefore cannot
    // be seeded with an entry whose parent is missing, and every objective and every
    // common control the ontology publishes is reachable through the filter that
    // narrows to its parent.
    //
    // Each entry's ARN carries the catalog's own published identifier for it, where this
    // crate records one, and otherwise one minted from its slug (see
    // `ontology_identifier`), and every entry is dated with the instant the catalog
    // published the library it belongs to (`published_ontology::LIBRARY_PUBLICATION_TIME`
    // — the library is published as a whole, so its entries are not dated by their
    // position in it). Both are fixed, so the catalog a client reads is identical in
    // every process and on every run.
    // Every recorded published identifier names an entry of the kind it is recorded for:
    // a renamed slug would otherwise leave its entry served under a minted identifier
    // with no signal, which is the defect this record exists to close.
    debug_assert!(
        published_ontology::every_published_identifier_names_an_entry(),
        "a published catalog identifier must name a published ontology entry of its kind"
    );
    let create_time = published_ontology::LIBRARY_PUBLICATION_TIME;
    for (domain_slug, domain_name, domain_description, domain_objectives) in
        published_ontology::PUBLISHED_ONTOLOGY
    {
        let domain_arn = catalog_arn(
            published_ontology::DOMAIN,
            &ontology_identifier(published_ontology::DOMAIN, domain_slug),
        );
        domains.insert(
            domain_arn.clone(),
            Domain {
                arn: domain_arn.clone(),
                name: (*domain_name).to_string(),
                description: (*domain_description).to_string(),
                create_time,
                // Equal to `CreateTime` until a revision moves it; see the domain seed.
                last_update_time: create_time,
            },
        );
        for (objective_slug, objective_name, objective_description, objective_common_controls) in
            *domain_objectives
        {
            let objective_arn = catalog_arn(
                published_ontology::OBJECTIVE,
                &ontology_identifier(published_ontology::OBJECTIVE, objective_slug),
            );
            objectives.insert(
                objective_arn.clone(),
                Objective {
                    arn: objective_arn.clone(),
                    name: (*objective_name).to_string(),
                    description: (*objective_description).to_string(),
                    domain_arn: domain_arn.clone(),
                    create_time,
                    // Equal to `CreateTime` until a revision moves it; see the domain.
                    last_update_time: create_time,
                },
            );
            for (slug, name, description) in *objective_common_controls {
                let arn = catalog_arn(
                    published_ontology::COMMON_CONTROL,
                    &ontology_identifier(published_ontology::COMMON_CONTROL, slug),
                );
                common_controls.insert(
                    arn.clone(),
                    CommonControl {
                        arn,
                        name: (*name).to_string(),
                        description: (*description).to_string(),
                        objective_arn: objective_arn.clone(),
                        domain_arn: domain_arn.clone(),
                        create_time,
                        // Equal to `CreateTime` until a revision moves it; see the domain.
                        last_update_time: create_time,
                    },
                );
            }
        }
    }
    let common_control_arn = |slug: &str| {
        catalog_arn(
            published_ontology::COMMON_CONTROL,
            &ontology_identifier(published_ontology::COMMON_CONTROL, slug),
        )
    };
    let control_arn = |slug: &str| catalog_arn("control", &control_identifier(slug));

    // --- controls ----------------------------------------------------------
    //
    // ONE pass over the published control table, which carries one row per published
    // control: its identity, the published text that describes it, the surface that
    // implements it, the control parameters a caller supplies when enabling it, and the
    // common control and framework items it is published against. The ARN carries the
    // catalog's own published identifier for the control, where the table records it,
    // and otherwise one minted from the row's slug, exactly as the ontology's are (see
    // `control_arn`).
    //
    // `CreateTime` is the instant the catalog publishes as the control's release time,
    // read from that same published record, so controls the catalog released together
    // carry one instant. Only a control whose published record this catalog does not
    // hold — one already served under a minted identifier — is dated positionally;
    // either way the instant is fixed (see CATALOG_EPOCH), so the catalog a client reads
    // is identical in every process and on every run.
    // Every recorded published record names a row of the table below: a stale entry
    // would otherwise leave its control served under a minted identifier with no signal.
    debug_assert!(
        published_controls::PUBLISHED_CATALOG_RECORDS
            .iter()
            .all(|record| published_controls::PUBLISHED_CONTROLS
                .iter()
                .any(|seed| seed.slug == record.slug)),
        "a published catalog record must name a published control"
    );
    for (position, seed) in published_controls::PUBLISHED_CONTROLS.iter().enumerate() {
        let arn = control_arn(seed.slug);
        // The parameters of the surface that implements the control, so two controls
        // over one implementation report one parameter set.
        let parameters = published_controls::parameters(seed);
        controls.insert(
            arn.clone(),
            Control {
                arn,
                control_tower_identifier: seed.control_tower_identifier.map(str::to_string),
                // Derived from the identifiers the designators are a function of, so
                // one surface's designator cannot land on another surface's control.
                aliases: published_controls::aliases(seed),
                name: seed.name.to_string(),
                description: seed.description.to_string(),
                behavior: seed.behavior.to_string(),
                severity: Some(seed.severity.to_string()),
                region_configuration: RegionConfig {
                    scope: seed.scope.to_string(),
                    deployable_regions: seed
                        .deployable_regions
                        .iter()
                        .map(|region| (*region).to_string())
                        .collect(),
                },
                implementation: Some(Implementation {
                    implementation_type: seed.implementation_type.to_string(),
                    identifier: seed.implementation_identifier.map(str::to_string),
                }),
                // A summary OF the parameter list below, never an independent fact
                // about the control.
                parameter_requirement_summary: Some(
                    published_controls::parameter_requirement_summary(parameters).to_string(),
                ),
                parameters: parameters
                    .iter()
                    .map(|(name, requirement)| ControlParam {
                        name: (*name).to_string(),
                        requirement: Some((*requirement).to_string()),
                    })
                    .collect(),
                // The instant the catalog publishes as this control's release time,
                // where its published record holds one; a control served under a minted
                // identifier is dated from its position in the table instead.
                create_time: published_controls::published_release_time(seed.slug)
                    .unwrap_or(CATALOG_EPOCH + (position as i64) * DAY),
                governed_resources: seed
                    .governed_resources
                    .iter()
                    .map(|resource| (*resource).to_string())
                    .collect(),
                governed_providers: seed
                    .governed_providers
                    .iter()
                    .map(|provider| (*provider).to_string())
                    .collect(),
            },
        );
    }

    // --- control mappings --------------------------------------------------
    // The catalog records, for every control it publishes, that control's WHOLE
    // mapping set, which is the only wire source for "what does this control
    // satisfy, implement and relate to":
    //
    //  * COMMON_CONTROL — the common control the control implements (its row's
    //    `implements`);
    //  * FRAMEWORK — one entry per published compliance-framework item the control
    //    satisfies (`published_controls::framework_mappings`: the cross-walk of the
    //    objective it implements, the items naming its own check, and the
    //    restatements those imply, each naming a published framework by its
    //    designator and that framework's own identifier for the requirement). A
    //    control satisfies the same objective in every framework that states it, so
    //    one or two entries per control is not the catalog's shape;
    //  * RELATED_CONTROL — one entry per control it is published alongside, with how
    //    the two relate (`published_controls::RELATED_CONTROLS`). Relations are
    //    recorded in both directions, because a query filters on one control at a time
    //    and each side of a pair has to report the relation when it is the one queried.
    let mut mapping_seed: Vec<(String, MappingDetails)> = Vec::new();
    for seed in published_controls::PUBLISHED_CONTROLS {
        let arn = control_arn(seed.slug);
        mapping_seed.push((
            arn.clone(),
            MappingDetails::CommonControl {
                common_control_arn: common_control_arn(seed.implements),
            },
        ));
        for (framework_name, item) in published_controls::framework_mappings(seed) {
            mapping_seed.push((
                arn.clone(),
                MappingDetails::Framework {
                    name: framework_name.to_string(),
                    item,
                },
            ));
        }
    }
    for &(one, other, relation_type) in published_controls::RELATED_CONTROLS {
        for (from, to) in [(one, other), (other, one)] {
            mapping_seed.push((
                control_arn(from),
                MappingDetails::RelatedControl {
                    control_arn: Some(control_arn(to)),
                    relation_type: relation_type.to_string(),
                },
            ));
        }
    }
    for (control_arn, mapping) in mapping_seed {
        // The key is the control, the kind of mapping and what it maps to — the kind
        // read off the mapping's own details, so a record cannot be stored under a type
        // its details do not match.
        let key = format!(
            "{control_arn}|{}|{}",
            mapping.mapping_type(),
            mapping.target()
        );
        control_mappings.insert(key, ControlMappingRecord { control_arn, mapping });
    }

    ComAmazonawsControlcatalogControlcatalogState {
        domains,
        objectives,
        common_controls,
        controls,
        control_mappings,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every reference in the seeded catalog resolves to an entry the matching list
    /// or get operation also returns.
    #[test]
    fn the_seeded_catalog_is_referentially_closed() {
        let state = ComAmazonawsControlcatalogControlcatalogState::default();
        for (arn, objective) in &state.objectives {
            assert!(
                state.domains.contains_key(&objective.domain_arn),
                "objective {arn}: domain {} is not in the catalog",
                objective.domain_arn
            );
        }
        for (arn, common_control) in &state.common_controls {
            assert!(
                state.objectives.contains_key(&common_control.objective_arn),
                "common control {arn}: objective {} is not in the catalog",
                common_control.objective_arn
            );
            assert!(
                state.domains.contains_key(&common_control.domain_arn),
                "common control {arn}: domain {} is not in the catalog",
                common_control.domain_arn
            );
        }
        for (key, mapping) in &state.control_mappings {
            assert!(
                state.controls.contains_key(&mapping.control_arn),
                "mapping {key}: control {} is not in the catalog",
                mapping.control_arn
            );
            match &mapping.mapping {
                MappingDetails::CommonControl { common_control_arn } => assert!(
                    state.common_controls.contains_key(common_control_arn),
                    "mapping {key}: common control {common_control_arn} is not in the catalog"
                ),
                MappingDetails::RelatedControl { control_arn, .. } => {
                    let related = control_arn.as_deref().unwrap_or_default();
                    assert!(
                        state.controls.contains_key(related),
                        "mapping {key}: related control {related} is not in the catalog"
                    );
                }
                MappingDetails::Framework { .. } => {}
            }
        }
    }

    /// Every entry the published ontology declares is seeded exactly once.
    ///
    /// A slug duplicated anywhere in the ontology would hash to the ARN that keys the
    /// catalog, so one of the two entries would silently replace the other and the
    /// served population would shrink without a diagnostic. Comparing the seeded
    /// counts with the declared ones is what makes such an editing mistake fail loudly.
    #[test]
    fn every_published_entry_is_seeded_exactly_once() {
        let state = ComAmazonawsControlcatalogControlcatalogState::default();
        let declared_domains = published_ontology::PUBLISHED_ONTOLOGY.len();
        let declared_objectives: usize = published_ontology::PUBLISHED_ONTOLOGY
            .iter()
            .map(|(_, _, _, objectives)| objectives.len())
            .sum();
        let declared_common_controls: usize = published_ontology::PUBLISHED_ONTOLOGY
            .iter()
            .flat_map(|(_, _, _, objectives)| objectives.iter())
            .map(|(_, _, _, common_controls)| common_controls.len())
            .sum();

        assert_eq!(
            state.domains.len(),
            declared_domains,
            "case domains: the ontology declares {declared_domains} domains, the catalog \
             serves {}; a duplicated domain slug would do this",
            state.domains.len()
        );
        assert_eq!(
            state.objectives.len(),
            declared_objectives,
            "case objectives: the ontology declares {declared_objectives} objectives, the \
             catalog serves {}; a duplicated objective slug would do this",
            state.objectives.len()
        );
        assert_eq!(
            state.common_controls.len(),
            declared_common_controls,
            "case common controls: the ontology declares {declared_common_controls} common \
             controls, the catalog serves {}; a duplicated common-control slug would do this",
            state.common_controls.len()
        );
    }

    /// A token resumes only in the collection and filter that issued it.
    #[test]
    fn a_token_from_another_collection_is_refused() {
        let items: Vec<(String, u8)> = vec![("a".to_string(), 1), ("b".to_string(), 2)];
        let first = page("domains", "", items.clone(), None, Some(1), 100)
            .expect("the first page has no token to decode");
        let token = first.next_token.expect("two items, page size 1: more remain");
        assert!(
            page("objectives", "", items.clone(), Some(&token), Some(1), 100).is_err(),
            "case replay-across-collections: a domains token must not resume objectives"
        );
        assert!(
            page("domains", "other-filter", items, Some(&token), Some(1), 100).is_err(),
            "case replay-across-filters: a token must not resume a different filter"
        );
    }
}
