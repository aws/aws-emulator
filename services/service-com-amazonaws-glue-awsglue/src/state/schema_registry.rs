// SPDX-License-Identifier: CC0-1.0

//! The schema registry's records — registries, schemas and schema versions — and the ARNs,
//! references and version spans every registry operation resolves one through.

use super::*;

/// The registry a schema-registry call that names no registry is served from.
///
/// `CreateSchema` documents the name: "When this API is called without a `RegistryId`,
/// this will create an entry for a "default-registry" in the registry database tables, if
/// it is not already present." It is the one registry name a create may bring into
/// existence, so it resolves to the same create-if-absent whether a request defaults to it
/// or names it outright; any other name must already have been created.
pub const DEFAULT_REGISTRY_NAME: &str = "default-registry";

/// Ceiling on the schema registries this emulator will hold, for the same reason as
/// [`MAX_DATABASES_PER_CATALOG`]: resident memory bounded against a create any caller can
/// reach. A create past it is refused with the operation's own modelled
/// `ResourceNumberLimitExceededException`.
pub const MAX_REGISTRIES: usize = 100;

/// Ceiling on the schema sets one registry may hold, for the same reason as
/// [`MAX_REGISTRIES`].
pub const MAX_SCHEMAS_PER_REGISTRY: usize = 1_000;

/// Ceiling on the schema sets the whole store may hold, across every registry.
///
/// Bounds the product of the two ceilings above, which on its own would still let a caller
/// grow the store far past what one process should hold: a schema set keeps every
/// `SchemaDefinition` registered against it verbatim, and the modelled
/// `SchemaDefinitionString` admits 170000 characters apiece, so the bound that matters is
/// the one on how many of those the store holds at once.
pub const MAX_STORED_SCHEMAS: usize = 2_000;

/// `RegistryStatus` for a registry this store holds. The enum carries exactly two values
/// and the other one (`DELETING`) belongs to a registry a delete is draining, so a
/// registry that is simply present is this one.
pub const REGISTRY_STATUS_AVAILABLE: &str = "AVAILABLE";

/// `RegistryStatus` for a registry a delete is draining, the other of the two values the
/// enum carries. A listing leaves a registry in this status out of its results, so the
/// value is named here rather than spelled in each reader.
pub const REGISTRY_STATUS_DELETING: &str = "DELETING";

/// `SchemaStatus` for a schema set this store holds. Nothing here is provisioned
/// asynchronously, so a schema set a create returned is readable at once rather than
/// passing through `PENDING`.
pub const SCHEMA_STATUS_AVAILABLE: &str = "AVAILABLE";

/// `SchemaVersionStatus` for a stored schema version that may be used, for the same reason
/// as [`SCHEMA_STATUS_AVAILABLE`]: a version is stored by the time its registration
/// answers, so it is readable at once rather than passing through `PENDING`.
///
/// A version whose compatibility check did not hold is stored as
/// [`SCHEMA_VERSION_STATUS_FAILURE`] instead.
pub const SCHEMA_VERSION_STATUS_AVAILABLE: &str = "AVAILABLE";

/// The `Compatibility` mode recorded for a schema created without one.
///
/// Source: AWS's schema-registry documentation, which names BACKWARD the default
/// compatibility mode of a new schema and recommends it ("This compatibility choice is
/// recommended as it allows data receivers to read both the current and one previous
/// schema version" — `CreateSchemaInput$Compatibility`). A schema always has a mode: it is
/// what every later version registration is checked against, and what
/// [`SchemaEntry::checkpoint`] records a change of.
pub const DEFAULT_COMPATIBILITY: &str = "BACKWARD";

/// The `Compatibility` mode that admits no version beyond the first.
///
/// Stated by `CreateSchema`: "Compatibility mode 'DISABLED' restricts any additional
/// schema versions from being added after the first schema version. For all other
/// compatibility modes, validation of compatibility settings will be applied only from the
/// second version onwards when the RegisterSchemaVersion API is used." It is named here
/// because the mode is recorded on the schema set and read back by every registration.
pub const COMPATIBILITY_DISABLED: &str = "DISABLED";

/// Ceiling on the versions one schema set may hold, for the same reason as
/// [`MAX_STORED_SCHEMAS`]: every registered `SchemaDefinition` is kept verbatim and the
/// modelled `SchemaDefinitionString` admits 170000 characters apiece, so the number of
/// them one schema set accumulates is bounded against a registration any caller can reach.
/// A registration past it is refused with the operation's own modelled
/// `ResourceNumberLimitExceededException`.
pub const MAX_VERSIONS_PER_SCHEMA: usize = 1_000;

/// The highest version number a schema set can report, from the modelled
/// `VersionLongNumber` every member carrying one targets (range 1..100000).
pub const MAX_SCHEMA_VERSION_NUMBER: i64 = 100_000;

/// The number of the first version registered against a schema set, which is also the
/// minimum the modelled `VersionLongNumber` admits (range 1..100000).
pub const FIRST_SCHEMA_VERSION: i64 = 1;

/// The version number reported for a schema set that holds no version yet — the one before
/// [`FIRST_SCHEMA_VERSION`], i.e. the version a first registration advances FROM.
///
/// `CreateSchema$SchemaDefinition` is optional, so a schema set can exist with no version;
/// its latest version and its checkpoint are then this, and its next version is
/// [`FIRST_SCHEMA_VERSION`].
pub const NO_SCHEMA_VERSION: i64 = 0;

/// The ARN resource-type segment of a schema registry (`registry/<name>`), the way
/// `CreateSchema$RegistryId` spells it ("The ARN format for the same will be:
/// arn:aws:glue:us-east-2::registry/default-registry…").
pub const REGISTRY_ARN_SEGMENT: &str = "registry";

/// The ARN resource-type segment of a schema set. A schema lives inside a registry, so its
/// resource portion carries BOTH names: `schema/<registry>/<schema>`.
pub const SCHEMA_ARN_SEGMENT: &str = "schema";

/// The ARN of registry `registry_name` as served in `region`.
pub fn registry_arn(region: &str, registry_name: &str) -> String {
    ::engine::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("{REGISTRY_ARN_SEGMENT}/{registry_name}"),
    )
}

/// The ARN of schema set `schema_name` of registry `registry_name`, as served in `region`.
pub fn schema_arn(region: &str, registry_name: &str, schema_name: &str) -> String {
    ::engine::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("{SCHEMA_ARN_SEGMENT}/{registry_name}/{schema_name}"),
    )
}

/// The identity fields and resource portion of a Glue resource ARN.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GlueArn {
    /// Field 4 of the ARN: the Region whose estate the resource belongs to.
    pub region: String,
    /// Field 5: the account that owns it.
    pub account_id: String,
    /// The sixth field and everything after it, colons included.
    pub resource: String,
}

impl GlueArn {
    /// Whether this ARN names the estate this endpoint serves: its own account, and the
    /// Region the request asking is served in. An ARN of any other account or Region names
    /// a resource this process does not hold, whatever it holds under that name.
    pub fn is_local(&self, region: &str) -> bool {
        is_served_account(&self.account_id) && self.region == region
    }

    /// This ARN's resource portion with the resource-type segment `segment` and its `/`
    /// stripped — the `default-registry` of `registry/default-registry`, the
    /// `default-registry/payments` of `schema/default-registry/payments`.
    ///
    /// `None` when the resource names another type, or carries no name after the segment.
    pub fn resource_of_type(&self, segment: &str) -> Option<&str> {
        let (kind, name) = self.resource.split_once('/')?;
        (kind == segment && !name.is_empty()).then_some(name)
    }
}

/// `arn`'s identity fields and resource portion when it is a well-formed Glue resource
/// ARN, or `None` when it is not one — a value in another service's namespace, or an ARN
/// missing a field.
///
/// The ONE split this crate does of a Glue ARN, so no two operations can disagree about
/// what one is. Only the fields are read here; which resource TYPE the ARN names
/// ([`GlueArn::resource_of_type`]), whether it names this endpoint's estate
/// ([`GlueArn::is_local`]) and whether this store holds the resource are separate answers.
pub fn parse_glue_arn(arn: &str) -> Option<GlueArn> {
    // `arn:<partition>:glue:<region>:<account>:<resource>`. The resource is the LAST
    // field, so it keeps any colon and any `/` of its own.
    let fields: Vec<&str> = arn.splitn(6, ':').collect();
    if fields.len() != 6 || fields[0] != "arn" || fields[2] != ARN_NAMESPACE {
        return None;
    }
    Some(GlueArn {
        region: fields[3].to_string(),
        account_id: fields[4].to_string(),
        resource: fields[5].to_string(),
    })
}

/// What a `RegistryId` or `SchemaId` resolves to.
///
/// Both wrappers may address their resource by name or by ARN, and an ARN carries an
/// account and a Region of its own — so a well-formed reference can name an estate this
/// endpoint does not serve. That is not a bad parameter and not a local resource either:
/// it is a reference this endpoint holds nothing for, whatever it holds under that name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reference<T> {
    /// A reference into this endpoint's own estate, resolved to its names.
    Local(T),
    /// A well-formed ARN of another account or another Region, resolved to the names it
    /// carries so a refusal can say what was looked for.
    Elsewhere(T),
}

/// The sentence a refusal carries for a `RegistryId` that names neither of the two members
/// one has to carry. Stated by the model: "One of `RegistryArn` or `RegistryName` has to
/// be provided" (`RegistryId$RegistryName`).
pub const REGISTRY_ID_REQUIRES_ONE: &str =
    "One of RegistryArn or RegistryName has to be provided.";

/// The same for a `SchemaId`, from `SchemaId$SchemaArn`: "One of `SchemaArn` or
/// `SchemaName` has to be provided."
pub const SCHEMA_ID_REQUIRES_ONE: &str = "One of SchemaArn or SchemaName has to be provided.";

/// The sentence the schema READ refuses a `SchemaId` that addresses no schema set
/// COMPLETELY with ([`complete_schema_reference`]).
///
/// RECORDED constant, and the whole message: the read states the two ways a schema set is
/// addressed — the registry and the schema together, or the schema's own ARN — and
/// interpolates nothing from the request. It is what the read answers for a reference
/// naming the schema alone, so the model's own prose for the member ("One of SchemaArn or
/// SchemaName has to be provided", [`SCHEMA_ID_REQUIRES_ONE`]) is not the whole rule: a
/// `SchemaName` carrying no `RegistryName` addresses no schema set, and is a bad parameter
/// rather than a miss of the registry a create would have defaulted to.
pub const SCHEMA_ID_REQUIRES_PAIR_OR_ARN: &str =
    "At least one of (registryName and schemaName) or schemaArn has to be provided.";

/// The sentence a refusal carries for a version READ that addresses no version at all.
/// Stated by the model: "This field is required for fetching by schema ID. Either this or
/// the `SchemaId` wrapper has to be provided" (`GetSchemaVersionInput$SchemaVersionId`) —
/// so a version is addressed either by its own id or by a schema set plus which of its
/// versions is meant.
pub const SCHEMA_VERSION_REQUIRES_ID_OR_NUMBER: &str =
    "One of SchemaVersionId or SchemaId and SchemaVersionNumber has to be provided.";

/// The sentence a refusal carries for a `SchemaVersionNumber` that names neither of its
/// two members, or both of them: one says "the latest version available for the schema"
/// and the other "the version number of the schema" (`SchemaVersionNumber`), so a value
/// carrying neither names no version and one carrying both names two.
pub const SCHEMA_VERSION_NUMBER_REQUIRES_ONE: &str =
    "One of LatestVersion or VersionNumber has to be provided.";

/// The sentence a refusal carries for an `UpdateSchema` that asks for neither of the two
/// things it can change about a schema set. Stated by the operation: "If the value for the
/// VersionNumber (checkpoint) is provided, Compatibility is optional and this can be used
/// to set/reset a checkpoint for the schema", so one of the two has to say what the update
/// is of.
pub const SCHEMA_UPDATE_REQUIRES_ONE: &str =
    "One of VersionNumber or Compatibility has to be provided.";

/// How a member the request did not carry is spelled where a refusal renders the whole
/// `SchemaId` wrapper back ([`schema_id_description`]).
const ABSENT_MEMBER: &str = "null";

/// A `SchemaId` as a refusal renders it: the wrapper's own name and its three members, in
/// the lower-camel spelling the service uses inside a message, each carrying the value the
/// request sent for it or [`ABSENT_MEMBER`] for one it did not carry.
///
/// The ONE rendering of the wrapper, so two refusals cannot disagree about it. Every part
/// is read off the request, which is what makes the sentence reproducible: the caller sees
/// the reference it sent, member for member.
pub fn schema_id_description(schema_id: &SchemaId) -> String {
    fn member(value: Option<&str>) -> &str {
        non_empty(value).unwrap_or(ABSENT_MEMBER)
    }
    format!(
        "SchemaId(schemaArn={}, schemaName={}, registryName={})",
        member(schema_id.schema_arn.as_deref()),
        member(schema_id.schema_name.as_deref()),
        member(schema_id.registry_name.as_deref())
    )
}

/// The registry a `RegistryId` names, as served in `region`.
///
/// `Err` is a bad parameter — neither member carried, an `RegistryArn` that is not a Glue
/// registry ARN, or two members naming different registries — and its string is the
/// refusal's message, derived from the member that is wrong.
pub fn registry_reference(
    registry_id: &RegistryId,
    region: &str,
) -> Result<Reference<String>, String> {
    let from_arn = match non_empty(registry_id.registry_arn.as_deref()) {
        Some(arn) => {
            let not_a_registry =
                || format!("RegistryArn {arn:?} is not the ARN of a Glue schema registry");
            let Some(parsed) = parse_glue_arn(arn) else {
                return Err(not_a_registry());
            };
            // A registry's resource portion is its name alone; anything deeper names
            // something else living under the segment, not a registry.
            let name = parsed
                .resource_of_type(REGISTRY_ARN_SEGMENT)
                .filter(|name| !name.contains('/'))
                .map(str::to_string);
            let Some(name) = name else {
                return Err(not_a_registry());
            };
            if !parsed.is_local(region) {
                return Ok(Reference::Elsewhere(name));
            }
            Some(name)
        }
        None => None,
    };
    match (from_arn, non_empty(registry_id.registry_name.as_deref())) {
        (None, None) => Err(REGISTRY_ID_REQUIRES_ONE.to_string()),
        (Some(name), None) => Ok(Reference::Local(name)),
        (None, Some(name)) => Ok(Reference::Local(name.to_string())),
        (Some(from_arn), Some(name)) if from_arn == name => Ok(Reference::Local(from_arn)),
        (Some(from_arn), Some(name)) => Err(format!(
            "RegistryArn names registry {from_arn} but RegistryName is {name}; both members \
             must name the same registry"
        )),
    }
}

/// The registry and schema names a `SchemaId` names for an operation that requires the
/// reference to address a schema set COMPLETELY: the schema's own `SchemaArn`, or its
/// `RegistryName` and `SchemaName` together.
///
/// This is the schema READ's rule ([`crate::ops::get_schema`]): the read resolves a
/// reference rather than writing one, so there is no registry for it to default to, and a
/// reference naming only half of the name pair is refused as a bad parameter
/// ([`SCHEMA_ID_REQUIRES_PAIR_OR_ARN`]) instead of being resolved against
/// [`DEFAULT_REGISTRY_NAME`]. A complete reference is then resolved exactly as
/// [`schema_reference`] resolves one, so the two cannot disagree about what a `SchemaId`
/// names.
pub fn complete_schema_reference(
    schema_id: &SchemaId,
    region: &str,
) -> Result<Reference<(String, String)>, String> {
    let by_arn = non_empty(schema_id.schema_arn.as_deref()).is_some();
    let by_pair = non_empty(schema_id.schema_name.as_deref()).is_some()
        && non_empty(schema_id.registry_name.as_deref()).is_some();
    if !by_arn && !by_pair {
        return Err(SCHEMA_ID_REQUIRES_PAIR_OR_ARN.to_string());
    }
    schema_reference(schema_id, region)
}

/// The registry and schema names a `SchemaId` names, as served in `region`.
///
/// A `SchemaId` that names a schema by name and no registry addresses the registry a
/// schema-registry call defaults to ([`DEFAULT_REGISTRY_NAME`]), which is the registry
/// `CreateSchema` writes into when the create named none.
///
/// `Err` carries the refusal's message, like [`registry_reference`]'s.
pub fn schema_reference(
    schema_id: &SchemaId,
    region: &str,
) -> Result<Reference<(String, String)>, String> {
    let from_arn = match non_empty(schema_id.schema_arn.as_deref()) {
        Some(arn) => {
            let not_a_schema = || format!("SchemaArn {arn:?} is not the ARN of a Glue schema");
            let Some(parsed) = parse_glue_arn(arn) else {
                return Err(not_a_schema());
            };
            // `schema/<registry>/<schema>`: exactly two names, so a schema name carrying a
            // further `/` is not this ARN's shape.
            let names = parsed
                .resource_of_type(SCHEMA_ARN_SEGMENT)
                .and_then(|resource| resource.split_once('/'))
                .filter(|(registry, schema)| !registry.is_empty() && !schema.contains('/'))
                .map(|(registry, schema)| (registry.to_string(), schema.to_string()));
            let Some(names) = names else {
                return Err(not_a_schema());
            };
            if !parsed.is_local(region) {
                return Ok(Reference::Elsewhere(names));
            }
            Some(names)
        }
        None => None,
    };
    let by_name = non_empty(schema_id.schema_name.as_deref()).map(|schema| {
        let registry = non_empty(schema_id.registry_name.as_deref())
            .unwrap_or(DEFAULT_REGISTRY_NAME)
            .to_string();
        (registry, schema.to_string())
    });
    match (from_arn, by_name) {
        (None, None) => Err(SCHEMA_ID_REQUIRES_ONE.to_string()),
        (Some(named), None) => Ok(Reference::Local(named)),
        (None, Some(named)) => Ok(Reference::Local(named)),
        (Some(from_arn), Some(by_name)) if from_arn == by_name => Ok(Reference::Local(from_arn)),
        (Some(from_arn), Some(by_name)) => Err(format!(
            "SchemaArn names schema {}/{} but SchemaName names {}/{}; both must name the \
             same schema",
            from_arn.0, from_arn.1, by_name.0, by_name.1
        )),
    }
}

/// `value` when it carries a non-empty string. An absent member and one sent as `""` say
/// the same thing about which member a wrapper carried, and `""` satisfies no name's
/// modelled pattern.
pub(super) fn non_empty(value: Option<&str>) -> Option<&str> {
    value.filter(|text| !text.is_empty())
}

/// One schema registry: the schema sets stored under a single `RegistryName`.
#[derive(Debug, Clone)]
pub struct RegistryEntry {
    /// `RegistryName`, which is also this entry's key in
    /// [`ComAmazonawsGlueAwsglueState::registries`].
    pub name: String,
    /// `CreateRegistry$Description`, absent for a registry created without one and for the
    /// default registry a schema create materialises.
    pub description: Option<String>,
    /// `RegistryStatus`: [`REGISTRY_STATUS_AVAILABLE`] for every registry this store holds.
    pub status: String,
    /// When the registry was created, reported as `CreatedTime`.
    pub created_time: SystemTime,
    /// When it last changed, reported as `UpdatedTime`. Equal to [`Self::created_time`]
    /// until something updates the registry itself — a schema created inside it does not
    /// change the registry.
    pub updated_time: SystemTime,
    /// `CreateRegistry$Tags`.
    pub tags: HashMap<String, String>,
    /// The schema sets this registry holds, keyed by `SchemaName`, in name order.
    pub schemas: BTreeMap<String, SchemaEntry>,
}

impl RegistryEntry {
    /// A registry created at `now`, with the description and tags its create carried.
    pub fn new(
        name: &str,
        description: Option<String>,
        tags: Option<&HashMap<String, String>>,
        now: SystemTime,
    ) -> Self {
        Self {
            name: name.to_string(),
            description,
            status: REGISTRY_STATUS_AVAILABLE.to_string(),
            created_time: now,
            updated_time: now,
            tags: tags.cloned().unwrap_or_default(),
            schemas: BTreeMap::new(),
        }
    }

    /// The [`DEFAULT_REGISTRY_NAME`] entry a schema create materialises when it is not
    /// already present. It carries no description and no tags: nothing asked for it, so
    /// there is nothing to record on it.
    pub fn default_registry(now: SystemTime) -> Self {
        Self::new(DEFAULT_REGISTRY_NAME, None, None, now)
    }

    /// Whether this registry can hold another schema set, i.e. is below
    /// [`MAX_SCHEMAS_PER_REGISTRY`].
    pub fn can_store_schema(&self) -> bool {
        self.schemas.len() < MAX_SCHEMAS_PER_REGISTRY
    }
}

/// One schema set: the compatibility settings a `CreateSchema` recorded, plus the versions
/// registered against it.
#[derive(Debug, Clone)]
pub struct SchemaEntry {
    /// `SchemaName`, which is also this entry's key in [`RegistryEntry::schemas`].
    pub name: String,
    /// The name of the registry holding it, so a read of the schema alone can report the
    /// registry it belongs to.
    pub registry_name: String,
    /// `CreateSchema$Description`: "If description is not provided, there will not be any
    /// automatic default value for this", hence `Option` rather than an empty string.
    pub description: Option<String>,
    /// `DataFormat`, the format every version's definition is written in.
    pub data_format: String,
    /// `Compatibility`, the mode later version registrations are checked against
    /// ([`DEFAULT_COMPATIBILITY`] for a create that named none).
    pub compatibility: String,
    /// `SchemaStatus`: [`SCHEMA_STATUS_AVAILABLE`] for every schema set this store holds.
    pub status: String,
    /// `SchemaCheckpoint`, "the version number of the checkpoint (the last time the
    /// compatibility mode was changed)": the version in force when the mode was last set,
    /// which at create is the version the create registered — or [`NO_SCHEMA_VERSION`] when
    /// it registered none.
    pub checkpoint: i64,
    /// When the schema set was created, reported as `CreatedTime`.
    pub created_time: SystemTime,
    /// When it last changed, reported as `UpdatedTime`.
    pub updated_time: SystemTime,
    /// `CreateSchema$Tags`.
    pub tags: HashMap<String, String>,
    /// The versions registered against it, in registration order, so the last is the
    /// latest.
    pub versions: Vec<SchemaVersionEntry>,
    /// The highest `VersionNumber` ever registered against this schema set, and
    /// [`NO_SCHEMA_VERSION`] for a set nothing has ever been registered against.
    ///
    /// A version NUMBER is assigned once and never reused: it counts the registrations a
    /// schema set has taken, not the versions it still holds, so deleting a version frees
    /// neither its number nor the numbers above it ([`Self::next_schema_version`]). That is
    /// why the counter lives on the set rather than being read off [`Self::versions`],
    /// which a delete shortens ([`crate::ops::delete_schema_versions`]).
    pub highest_version_number: i64,
}

impl SchemaEntry {
    /// `LatestSchemaVersion`, "the latest version of the schema associated with the
    /// returned schema definition": the number of the latest version registered, or
    /// [`NO_SCHEMA_VERSION`] for a schema set nothing has been registered against.
    ///
    /// Read off [`Self::highest_version_number`], so a set whose latest version has been
    /// deleted still reports the number that registration took: version numbers are
    /// assigned once and never reused.
    pub fn latest_schema_version(&self) -> i64 {
        self.highest_version_number
    }

    /// `NextSchemaVersion`: the number the next registration would take, which is one past
    /// [`Self::latest_schema_version`] — so a schema set holding no version reports
    /// [`FIRST_SCHEMA_VERSION`].
    pub fn next_schema_version(&self) -> i64 {
        self.latest_schema_version().saturating_add(1)
    }

    /// The last version registered against this schema set.
    pub fn latest_version(&self) -> Option<&SchemaVersionEntry> {
        self.versions.last()
    }

    /// The version this schema set holds under `version_number`, or `None` when it holds
    /// no such version — one never registered, or one a delete removed.
    pub fn version(&self, version_number: i64) -> Option<&SchemaVersionEntry> {
        self.versions
            .iter()
            .find(|version| version.version_number == version_number)
    }

    /// The version whose stored `SchemaDefinition` is exactly `definition`, or `None` when
    /// this schema set holds no version of it.
    ///
    /// Compared byte for byte, as stored: a definition is the artefact a registration
    /// submitted, and the registry documents no normalisation of it.
    pub fn version_with_definition(&self, definition: &str) -> Option<&SchemaVersionEntry> {
        self.versions
            .iter()
            .find(|version| version.definition == definition)
    }

    /// Whether this schema set's recorded `Compatibility` admits a FURTHER version:
    /// [`COMPATIBILITY_DISABLED`] "restricts any additional schema versions from being
    /// added after the first schema version", so a set already holding one admits none —
    /// while a set holding none still takes its first, which is the version every other
    /// mode's compatibility validation is applied "only from the second version onwards"
    /// against.
    pub fn allows_another_version(&self) -> bool {
        self.compatibility != COMPATIBILITY_DISABLED || self.versions.is_empty()
    }

    /// Whether this schema set can hold another version: below
    /// [`MAX_VERSIONS_PER_SCHEMA`], and with a next version number the modelled
    /// `VersionLongNumber` still admits — version numbers only ever climb
    /// ([`Self::next_schema_version`]), so a set whose versions have been deleted and
    /// re-registered many times over can reach the top of that range while holding few.
    pub fn can_store_version(&self) -> bool {
        self.versions.len() < MAX_VERSIONS_PER_SCHEMA
            && self.next_schema_version() <= MAX_SCHEMA_VERSION_NUMBER
    }

    /// Register `definition` as this schema set's next version, at `now`, with
    /// `status` as its `SchemaVersionStatus`, and report the version stored.
    ///
    /// The ONE write that adds a version, so a registration and a create cannot disagree
    /// about what one is: the number is [`Self::next_schema_version`] and the version is
    /// appended, which keeps [`Self::versions`] in registration order. The schema set's
    /// `UpdatedTime` moves with it, because the set now holds something it did not. Its
    /// `SchemaCheckpoint` does NOT: that records "the last time the compatibility mode was
    /// changed", which a registration does not change.
    ///
    /// Which status a registration carries is decided by its compatibility check, so the
    /// caller is [`Self::register_checked_version`] rather than a handler.
    pub fn register_version(
        &mut self,
        definition: String,
        status: &str,
        now: SystemTime,
    ) -> SchemaVersionEntry {
        let version_number = self.next_schema_version();
        let registered = SchemaVersionEntry::new(version_number, definition, status, now);
        self.versions.push(registered.clone());
        // The number this registration took is now the highest the set has ever assigned,
        // which is what the next one counts from however many versions are later deleted.
        self.highest_version_number = version_number;
        self.updated_time = now;
        registered
    }
}

impl ComAmazonawsGlueAwsglueState {
    /// The schema set and the version a `SchemaVersionId` names, or `None` when this store
    /// holds no version under that id.
    ///
    /// A `SchemaVersionId` is minted per version and addresses it on its own
    /// (`GetSchemaVersion$SchemaVersionId`), so the search is over every schema set of
    /// every registry — bounded by [`MAX_STORED_SCHEMAS`] sets of
    /// [`MAX_VERSIONS_PER_SCHEMA`] versions. The schema set is reported beside the version
    /// because a read of a version reports its schema's ARN and `DataFormat` too.
    pub fn schema_version_by_id(
        &self,
        version_id: &str,
    ) -> Option<(&SchemaEntry, &SchemaVersionEntry)> {
        self.registries.values().find_map(|registry| {
            registry.schemas.values().find_map(|schema| {
                schema
                    .versions
                    .iter()
                    .find(|version| version.version_id == version_id)
                    .map(|version| (schema, version))
            })
        })
    }
}

/// One version registered against a schema set.
#[derive(Debug, Clone)]
pub struct SchemaVersionEntry {
    /// `SchemaVersionId`, the version's own identifier: a version-4 UUID, which is the
    /// shape the modelled `SchemaVersionIdString` pins (len 36, pattern
    /// `^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$`). Minted once, at
    /// registration, so every later read of this version reports the same id.
    pub version_id: String,
    /// `VersionNumber`, counting from [`FIRST_SCHEMA_VERSION`].
    pub version_number: i64,
    /// The `SchemaDefinition` registered, stored verbatim: it is the artefact later reads
    /// return and later registrations are compared against.
    pub definition: String,
    /// `SchemaVersionStatus`.
    pub status: String,
    /// When the version was registered, reported as `CreatedTime`.
    pub created_time: SystemTime,
}

impl SchemaVersionEntry {
    /// The version `version_number` of a schema set, registered at `now` with `definition`
    /// and the `SchemaVersionStatus` `status`.
    pub fn new(
        version_number: i64,
        definition: String,
        status: &str,
        now: SystemTime,
    ) -> Self {
        Self {
            version_id: new_schema_version_id(),
            version_number,
            definition,
            status: status.to_string(),
            created_time: now,
        }
    }
}

/// A fresh `SchemaVersionId`: a version-4 UUID, the shape the modelled
/// `SchemaVersionIdString` pins.
///
/// The randomness is the `uuid` crate's, reached through `emulator_telemetry`'s
/// uuid-backed id newtype (`BatchId::random()` is `uuid::Uuid::new_v4()` rendered
/// canonically), which this crate already depends on: nothing here is a hand-rolled
/// generator. This names a RESOURCE, not a credential.
pub fn new_schema_version_id() -> String {
    ::emulator_telemetry::BatchId::random().as_str().to_string()
}

/// A wall-clock instant as the schema registry reports it.
///
/// The registry's own timestamps (`CreatedTimestamp`, `UpdatedTimestamp`) are modelled as
/// STRINGS rather than as the timestamp type the catalog's shapes use, so the instant is
/// rendered as a date-time string here, once, and every schema-registry read formats it the
/// same way.
pub fn to_timestamp_string(at: SystemTime) -> String {
    to_date_time(at)
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
        .unwrap_or_default()
}

/// The inclusive span of schema version numbers a `Versions` string names, lowest first,
/// or `None` when the value names no version at all.
///
/// Stated by the member: "A version range may be supplied which may be of the format: a
/// single version number, 5 a range, 5-8 : deletes versions 5, 6, 7, 8"
/// (`DeleteSchemaVersionsInput$Versions`), whose modelled form is a decimal counter with no
/// leading zero, alone or as the two ends of a `<low>-<high>` range. A value outside that
/// form names no version, and so does one whose ends do not spell a number this service can
/// count with.
///
/// The span is NOT narrowed to the versions a schema set holds: which of the versions a
/// request named exist is the caller's own schema set to answer, not this reading's.
pub fn parse_version_span(versions: &str) -> Option<(i64, i64)> {
    /// One end of the span: the modelled `[1-9][0-9]*`, i.e. a positive decimal counter
    /// carrying no sign, no separator and no leading zero.
    fn counter(text: &str) -> Option<i64> {
        let mut characters = text.chars();
        if !matches!(characters.next(), Some('1'..='9')) {
            return None;
        }
        if !characters.all(|character| character.is_ascii_digit()) {
            return None;
        }
        text.parse::<i64>().ok()
    }

    match versions.split_once('-') {
        Some((low, high)) => Some((counter(low)?, counter(high)?)),
        None => {
            let one = counter(versions)?;
            Some((one, one))
        }
    }
}

/// The counter a stored or requested `VersionId` spells, or `None` when it spells no
/// version counter.
///
/// "A `VersionId` is a string representation of an integer. Each version is incremented
/// by 1" (`GetTableVersionRequest$VersionId`), and only the server ever writes one — as
/// a decimal counter — so a value that spells no such integer names no version of any
/// table.
pub fn parse_version_counter(version_id: &str) -> Option<u64> {
    version_id.parse::<u64>().ok()
}

/// The `VersionId` that follows `current`: the stored counter plus one.
///
/// Only the server ever writes a `VersionId`, and it writes a decimal counter, so a
/// value that does not parse is treated as the version before the first one
/// ([`FIRST_TABLE_VERSION_ID`] minus one, i.e. the id a fresh table advances FROM)
/// rather than carried forward unchanged — a version must always move when the table
/// does.
pub fn next_version_id(current: &str) -> String {
    parse_version_counter(current)
        // Explicitly the version before the first one, not a general-purpose default.
        .unwrap_or(0)
        .saturating_add(1)
        .to_string()
}
