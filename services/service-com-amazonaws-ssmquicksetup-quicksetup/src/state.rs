// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-ssmquicksetup-quicksetup service.
//!
//! The service's primary resource is the **configuration manager**: a collection of
//! configuration definitions, each of which the service deploys as one
//! **configuration**. Both live here exactly once — the manager keyed by its ARN,
//! the configuration by its service-generated id — so every operation reads and
//! writes the one record, and a configuration is reclaimed with the manager that
//! owns it.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::time::{Duration, SystemTime};

/// Whether the role a LOCAL configuration deployment is administered with can be assumed
/// by the service that deploys it: its own concern, so its own module.
pub mod local_administration_role;

/// Whether the CALLER's own identity-based IAM policies let a request perform the action
/// it asks for: its own concern, so its own module.
pub mod identity_authorization;

use crate::ops::types::{
    ConfigurationDefinition, ConfigurationDefinitionSummary, ConfigurationManagerSummary,
    ConfigurationSummary, Filter, ServiceSettings, StatusSummary,
};

/// The signing name, endpoint prefix and ARN namespace of this service, all three
/// `ssm-quicksetup` (the model's `aws.auth#sigv4` / `aws.api#service` traits).
pub const ARN_SERVICE: &str = "ssm-quicksetup";

/// The resource portion a configuration-manager ARN carries before its uuid, from
/// the modelled `ManagerArn` pattern
/// `^arn:aws:ssm-quicksetup:([^:]+):(\d{12}):configuration-manager/<uuid>$`.
pub const MANAGER_ARN_RESOURCE_PREFIX: &str = "configuration-manager/";

/// Region used when a request carries no SigV4 credential scope to read one from.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The `StatusType` of the summary a configuration manager's deployment carries,
/// from the modelled `StatusType` enum (`crate::ops::types::STATUS_TYPE_VALUES`).
pub const STATUS_TYPE_DEPLOYMENT: &str = "Deployment";

/// The status a just-created deployment is in, from the modelled `Status` enum
/// (`crate::ops::types::STATUS_VALUES`).
pub const STATUS_INITIALIZING: &str = "INITIALIZING";

/// The status a deployment settles into, from the same enum.
pub const STATUS_SUCCEEDED: &str = "SUCCEEDED";

/// The status the work on a manager (and on each configuration it owns) is in while
/// the service tears that manager's deployed resources down, from the same enum.
pub const STATUS_DELETING: &str = "DELETING";

/// The `StatusType` of the summary that reports the asynchronous executions a
/// deployment runs on the accounts and Regions it targets — the second value the
/// modelled `StatusType` enum publishes (`crate::ops::types::STATUS_TYPE_VALUES`).
pub const STATUS_TYPE_ASYNC_EXECUTIONS: &str = "AsyncExecutions";

/// The status a deployment is in while it is rolling its resources out, after it has
/// initialized and before it reaches an outcome, from the modelled `Status` enum.
pub const STATUS_DEPLOYING: &str = "DEPLOYING";

/// The status a kind of work that has not run reports, from the same enum.
pub const STATUS_NONE: &str = "NONE";

/// The phases the deployment of a configuration manager (or of one of its
/// configurations) is reported in, in the order it walks them: it initializes, rolls
/// out, and then reports its outcome. Every value is from the modelled `Status` enum.
pub const DEPLOYMENT_PHASES: [&str; 3] = [STATUS_INITIALIZING, STATUS_DEPLOYING, STATUS_SUCCEEDED];

/// The phases the asynchronous executions a deployment runs are reported in, in the
/// order they walk them: nothing has run while the deployment is still rolling out, and
/// once it has rolled out the executions it ran report their outcome. Both values are
/// from the modelled `Status` enum.
pub const ASYNC_EXECUTION_PHASES: [&str; 2] = [STATUS_NONE, STATUS_SUCCEEDED];

/// How long a deployment or a teardown is in flight before the clock owes it its
/// outcome.
///
/// `CreateConfigurationManager` starts a deployment and
/// `DeleteConfigurationManager` starts a teardown, so the manager and its
/// configurations are reported in a non-terminal `Status` first: that is a
/// transition the clock owes. The span is this emulator's own choice (AWS documents
/// no duration for a Quick Setup deployment or teardown) and is deliberately
/// non-zero, so a caller reading straight after the call still observes the
/// transient status, as it does against the real service. Work that walks more than
/// one transient phase shares this span out over them
/// ([`StatusSummaryRecord::walking`]), so it reaches its terminal status this long
/// after the call that started it however many phases it passes through. The instant
/// each phase comes due is stored on the summary ([`StatusSummaryRecord::settles`]) —
/// or, for a teardown, on the manager
/// ([`ConfigurationManagerRecord::deletes_at`]) — and materialised by
/// [`ComAmazonawsSsmquicksetupQuicksetupState::settle`]; nothing sleeps.
pub const TRANSITION_SPAN: Duration = Duration::from_millis(500);

/// Why a mutation of a configuration manager whose teardown is in flight is refused.
///
/// The wording is what the model documents for `ConflictException`: a teardown the
/// service accepted IS another request being processed, and it is the one the caller
/// has to wait out before the manager takes any further mutation. The one place the
/// message lives, so every operation that mutates a manager refuses the same way.
pub const TEARDOWN_IN_FLIGHT_MESSAGE: &str =
    "Another request is being processed. Wait a few minutes and try again.";

/// The sentence this service refuses with when an ARN a request supplied names no
/// stored configuration manager.
///
/// The model declares `ResourceNotFoundException` with a single `Message` member and
/// states no text for it, so the wording is what the service itself emits: the fixed
/// text `Quick setup manager with arn: ` and ` could not be found.` around the ARN the
/// REQUEST carried. Nothing else varies — no account id, Region or uuid is part of the
/// template, and the ARN is never re-derived from stored state.
/// The one place the message lives, so every operation that addresses a manager by ARN
/// refuses an absent one the same way.
///
/// The ARN is rendered through [`echoed_value`], the way every caller-supplied value
/// this service echoes into a refusal is, because no modelled constraint bounds the
/// length of the one this template renders: the tag plane's `ResourceArn` carries
/// neither a `@pattern` nor a `@length`, and the `ManagerArn` pattern of the manager
/// routes leaves the Region segment it admits (`[^:]+`) unbounded, so text of any
/// length the request can carry reaches here. Every ARN this service mints is far
/// inside that bound, so the sentence for one is the ARN itself, unchanged.
pub fn manager_not_found_message(manager_arn: &str) -> String {
    format!(
        "Quick setup manager with arn: {} could not be found.",
        echoed_value(manager_arn)
    )
}

/// The sentence `ListTagsForResource` refuses with when the ARN it was given names no
/// resource this endpoint serves.
///
/// Wholly fixed text, and NOT the manager-route sentence
/// [`manager_not_found_message`] states: AWS words its not-found refusal per operation,
/// and the one the tag plane's read states names no id, ARN, member or value, so there is
/// nothing in it taken from the request or from stored state — and nothing
/// caller-supplied to bound.
pub const LIST_TAGS_NOT_FOUND_MESSAGE: &str = "Configuration with requested id does not exist.";

/// The sentence a tag MUTATION — `TagResource` and `UntagResource`, the two operations
/// bound to `PUT`/`DELETE /tags/{ResourceArn}` — refuses with when the ARN it was given
/// names no resource this endpoint serves.
///
/// Wholly fixed text, for the same reason as [`LIST_TAGS_NOT_FOUND_MESSAGE`] and
/// different from it: the sentence belongs to the error class of the operation stating it,
/// and assigning or removing tags is one class where reading them is another. The one
/// place it lives, so both mutations of the tag plane refuse an absent resource alike.
pub const TAG_MUTATION_NOT_FOUND_MESSAGE: &str = "Resource arn not found.";

/// The sentence this service refuses with when a configuration-definition id a request
/// supplied names no stored definition in the scope that request addressed it in.
///
/// `manager_arn` is the manager the same request named, when it named one: a definition
/// id means something only against a manager, so a request that scoped its lookup to one
/// is told which manager holds no such definition, while one that scoped it to nothing
/// but the Region's endpoint is told only that the id names no definition. The model
/// declares `ResourceNotFoundException` with a single `Message` member and states no text
/// for it, so both wordings are this service's own, and both render only the id the
/// REQUEST carried — nothing is re-derived from stored state. The one place the message
/// lives, so every operation that addresses a configuration definition by id refuses an
/// absent one the same way.
///
/// The ARN is bounded by [`echoed_value`] for the reason
/// [`manager_not_found_message`] gives: it arrives on a member whose modelled pattern
/// leaves its Region segment unbounded. The id needs no bound — every member that
/// carries one into this template is modelled `^[a-z0-9-]{1,20}$`, far inside
/// [`MAX_ECHOED_VALUE`] — so it is rendered as the request spelled it.
pub fn definition_not_found_message(manager_arn: Option<&str>, id: &str) -> String {
    match manager_arn {
        Some(manager_arn) => format!(
            "configuration manager {} holds no configuration definition with ID {id}",
            echoed_value(manager_arn)
        ),
        None => format!("no configuration definition with ID {id} exists"),
    }
}

/// Largest page either list operation will build, and the page size applied when the
/// request names no `MaxItems`: the maximum of that member's modelled `@range(1,
/// 100)`.
pub const MAX_ITEMS_LIMIT: i32 = 100;

/// How many configuration managers this emulator will hold.
///
/// Emulator resource guard, NOT a published AWS quota: a create that would take the
/// store past it is refused rather than growing this process without bound.
pub const MAX_CONFIGURATION_MANAGERS: usize = 500;

/// How many configuration definitions one create may carry.
///
/// `ConfigurationDefinitionsInputList` carries no modelled length, so the bound
/// lives here. Emulator resource guard, NOT a published AWS quota.
pub const MAX_CONFIGURATION_DEFINITIONS: usize = 50;

/// How many parameters one configuration definition may carry.
///
/// `ConfigurationParametersMap` bounds each key and value but not the map's size, so
/// the bound lives here. Emulator resource guard, NOT a published AWS quota.
pub const MAX_PARAMETERS: usize = 100;

/// How many tags one configuration manager may carry.
///
/// `TagsMap` bounds each key and value but not the map's size, so the bound lives
/// here. Emulator resource guard, NOT a published AWS quota.
pub const MAX_TAGS: usize = 50;

/// How many `Filters` entries one listing request may carry, and how many `Values`
/// each of them may carry.
///
/// `FiltersList` and `FilterValues` are caller-sized and the model publishes no length
/// for either, while the matching work one request does is records x filters x values.
/// Emulator resource guards, NOT published AWS quotas: a request past them is refused
/// before the store lock is taken.
pub const MAX_FILTERS: usize = 10;

/// See [`MAX_FILTERS`].
pub const MAX_FILTER_VALUES: usize = 20;

/// How many stored records ONE listing request may examine.
///
/// The page is bounded by `MaxItems`, but a filter that admits little would otherwise
/// walk the whole store to fill it, holding the store lock for records x filters x
/// values work. A request that reaches this many examined records ends its page early
/// and hands back the token to resume from, so paging still visits every record exactly
/// once. Emulator resource guard, NOT a published AWS quota.
pub const MAX_SCAN_ENTRIES: usize = 1_000;

/// `Filter` keys `ListConfigurationManagers` accepts.
///
/// The model documents no key catalog, so the keys are the listed shape's own
/// string-valued members — `ConfigurationManagerSummary` — whose values the modelled
/// `FilterValues` pattern (`^[A-Za-z0-9+=@_\/\s-]*$`) can express. `ManagerArn` is
/// not among them: an ARN carries `:`, which that pattern refuses, so no filter value
/// can ever name one.
pub const MANAGER_FILTER_KEYS: &[&str] = &["Name", "Description"];

/// `Filter` keys `ListConfigurations` accepts: the string-valued members of
/// `ConfigurationSummary`, for the same reason as [`MANAGER_FILTER_KEYS`]. The
/// manager is named by this operation's own `ManagerArn` member instead.
///
/// `TypeVersion` IS among them even though the stored spelling of a version carries a
/// `.`, which that pattern refuses: a configuration version has several spellings (a
/// major alone numbers the same version as `<major>.0`, and the modelled
/// [`TYPE_VERSION_LATEST`] alias names the newest), so a filter value the pattern
/// admits can still name a stored version. Which version it names is resolved the way
/// every other member naming a version is ([`FilterField::TypeVersion`]) rather than
/// compared as text.
pub const CONFIGURATION_FILTER_KEYS: &[&str] = &[
    "Id",
    "ConfigurationDefinitionId",
    "Type",
    "TypeVersion",
    "Account",
    "Region",
];

/// How many regions' service settings this store will hold.
///
/// The settings record is keyed by the region the request was signed for, which is
/// caller-controlled text (bounded by [`request_region`] but not enumerated), so an
/// update naming an unseen region would otherwise grow this store without bound.
/// Emulator resource guard, NOT a published AWS quota: an update past it is refused
/// rather than stored. Far above the number of regions AWS publishes.
pub const MAX_SETTINGS_REGIONS: usize = 100;

/// When the parameter table documented for a configuration type requires one of the
/// parameters it marks `(Required)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequiredWhen {
    /// The table states the requirement on its own, so a definition of that type must
    /// always provide the parameter.
    Always,
    /// The table marks the parameter `(Required)` and its own description states the
    /// condition: a definition must provide it only while the parameter `key` carries
    /// one of `values`, and may leave it out otherwise.
    ParameterIsOneOf {
        /// The `Parameters` key the condition reads.
        key: &'static str,
        /// The values of that key the condition holds for.
        values: &'static [&'static str],
    },
}

/// One parameter the table documented for a configuration type requires of a
/// configuration definition deploying that type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RequiredParameter {
    /// The `Parameters` key, spelled as the documented table writes it.
    pub key: &'static str,
    /// When that table requires it.
    pub when: RequiredWhen,
}

impl RequiredParameter {
    /// Whether a definition holding `parameters` must provide this parameter.
    ///
    /// A conditional requirement is read off the parameter set the definition HOLDS,
    /// because the condition the documented table states is about another parameter of
    /// that same set; a value is matched trimmed, the way every other parameter value
    /// this service reads is.
    pub fn applies<P: ConfigurationParameters + ?Sized>(&self, parameters: &P) -> bool {
        match self.when {
            RequiredWhen::Always => true,
            RequiredWhen::ParameterIsOneOf { key, values } => parameters
                .parameter(key)
                .is_some_and(|value| values.contains(&value.trim())),
        }
    }
}

/// One entry of the Quick Setup type catalog `ListQuickSetupTypes` publishes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QuickSetupTypeEntry {
    /// A `QuickSetupTypeOutput$Type`: the type a `ConfigurationDefinitionInput`
    /// names. Satisfies that member's modelled `^[a-zA-Z0-9_\-.:/]{3,200}$` pattern.
    pub type_: &'static str,
    /// Every configuration version this service publishes for `type_`, oldest first:
    /// the version history of the service-owned template that type deploys.
    ///
    /// Each entry takes the form the modelled `TypeVersion` pattern
    /// `^\d{1,3}(\.\d{1,3})?$|^LATEST$` gives (`smithy_query(query="operation",
    /// operation="UpdateConfigurationDefinition")`) — a major and an optional minor
    /// of up to three digits each. Each type's template is versioned independently,
    /// so one type's history advances without the others', which is why the version
    /// lives per entry rather than once for the whole catalog. A history is not
    /// contiguous: a revision is published out of band, so this carries the versions
    /// the service offers rather than every number between the first and the newest.
    /// Never empty; the newest entry is what [`Self::latest_version`] reports.
    pub versions: &'static [&'static str],
    /// Every parameter a configuration definition of this type must provide, in the
    /// order the documented table states them.
    ///
    /// Source: the parameter table this service's model documents for this type
    /// (`smithy_query(query="doc", shape="ConfigurationDefinitionInput",
    /// member="Parameters")`) — every entry that table marks `(Required)`, with the
    /// condition the entry's own description states where it states one. A definition
    /// deploys this type's service-owned template, and the template takes those
    /// parameters, so a parameter set leaving one out cannot be deployed: that is what
    /// [`Self::missing_required_parameter`] refuses.
    ///
    /// Two markers that table carries live elsewhere rather than being repeated per
    /// entry, because they hold for a definition of EVERY type:
    /// [`PARAMETER_TARGET_REGIONS`], which every table marks `(Required)`
    /// ([`NO_DEPLOYMENT_REGION_MESSAGE`]), and the `(Optional)` either/or between
    /// [`PARAMETER_TARGET_ACCOUNTS`] and [`PARAMETER_TARGET_ORGANIZATIONAL_UNITS`]
    /// ([`NO_DEPLOYMENT_TARGET_MESSAGE`]) — which is why the tables that instead mark
    /// [`PARAMETER_TARGET_ORGANIZATIONAL_UNITS`] itself `(Required)`, and name no
    /// `TargetAccounts` at all, carry it here: for those types the either/or is not
    /// what their own table says.
    pub required_parameters: &'static [RequiredParameter],
}

/// The configuration version a published version history starts at: the first
/// version of a service-owned template, and what an entry publishing nothing at all
/// would report.
pub const FIRST_TYPE_VERSION: &str = "1.0";

/// The alias a request uses to name whichever version a type publishes as its newest,
/// instead of naming that version itself.
///
/// Modelled: `UpdateConfigurationDefinitionInput$TypeVersion`'s pattern
/// `^\d{1,3}(\.\d{1,3})?$|^LATEST$` admits exactly this one non-numeric value, and it
/// means something only against the published catalog.
pub const TYPE_VERSION_LATEST: &str = "LATEST";

/// The `(major, minor)` a configuration version numbers — so two versions can be
/// ordered and the newest of a history found — or `None` when the text is not a
/// version in the form the modelled `TypeVersion` pattern gives.
///
/// An omitted minor numbers `0`, so `1` and `1.0` are two spellings of ONE version:
/// that pattern admits both.
pub fn type_version_order(value: &str) -> Option<(u32, u32)> {
    fn component(text: &str) -> Option<u32> {
        if text.is_empty() || text.len() > 3 || !text.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        text.parse().ok()
    }
    let (major, minor) = value.split_once('.').unwrap_or((value, "0"));
    Some((component(major)?, component(minor)?))
}

impl QuickSetupTypeEntry {
    /// The newest configuration version this type publishes: its
    /// `QuickSetupTypeOutput$LatestVersion`.
    ///
    /// Computed from the published history rather than written down beside it, so a
    /// type's versions live in one place and the latest the catalog reports cannot
    /// drift from the versions it accepts.
    pub fn latest_version(&self) -> &'static str {
        self.versions
            .iter()
            .copied()
            .max_by_key(|version| type_version_order(version).unwrap_or((0, 0)))
            .unwrap_or(FIRST_TYPE_VERSION)
    }

    /// Whether this type's documented table requires `key` of a definition holding
    /// `parameters`: the table marks it `(Required)` and whatever condition it states
    /// for it holds for that set.
    pub fn requires<P: ConfigurationParameters + ?Sized>(&self, key: &str, parameters: &P) -> bool {
        self.required_parameters
            .iter()
            .any(|required| required.key == key && required.applies(parameters))
    }

    /// The first parameter this type's documented table requires of a definition
    /// holding `parameters` that the set does not provide, or `None` when it provides
    /// every one of them.
    ///
    /// The requirements are read in the order the table states them, so a set leaving
    /// out several is refused for the first one that table names. A key written with no
    /// value, or with nothing but blanks, provides nothing: the same reading the
    /// deployment-target keys get ([`comma_separated`]).
    pub fn missing_required_parameter<P: ConfigurationParameters + ?Sized>(
        &self,
        parameters: &P,
    ) -> Option<&'static str> {
        self.required_parameters
            .iter()
            .filter(|required| required.applies(parameters))
            .map(|required| required.key)
            .find(|key| {
                parameters
                    .parameter(key)
                    .is_none_or(|value| value.trim().is_empty())
            })
    }

    /// The version of this type a request naming `requested` means, in the spelling
    /// the catalog publishes it under, or `None` when this type publishes no such
    /// version.
    ///
    /// Versions are matched by what they NUMBER rather than by their text, so a
    /// request naming `2` means the version published as `2.0`; the modelled
    /// [`TYPE_VERSION_LATEST`] alias means the newest.
    pub fn published_version(&self, requested: &str) -> Option<&'static str> {
        if requested == TYPE_VERSION_LATEST {
            return Some(self.latest_version());
        }
        let wanted = type_version_order(requested)?;
        self.versions
            .iter()
            .copied()
            .find(|version| type_version_order(version) == Some(wanted))
    }
}

/// The Quick Setup configuration types this service publishes, ordered by `Type` —
/// which is the order [`QUICK_SETUP_TYPE_CATALOG`] is published in, so two reads of the
/// catalog report the same list in that order.
///
/// Source: the configuration types this service's own model documents, from
/// `smithy_query(query="doc", shape="ConfigurationDefinitionInput",
/// member="Parameters")` — its parameter tables are titled
/// `<name> (Type: AWSQuickSetupType-<type>)`, one per supported type (the doc text
/// renders `AWS` as `Amazon Web Services`). `AWSQuickSetupType-SSM` is the Quick
/// Setup default configuration type, which that member's tables do not itemise; it
/// is published too because configuration definitions name it. A type the service
/// offers that this model revision's tables do not itemise at all is published the same
/// way — the catalog is versioned out of band from the model, so the model's tables
/// bound neither the membership nor the versions (which is also why `Type` is modelled
/// as a patterned string rather than an enum).
///
/// This list is both the DISCOVERY catalog `ListQuickSetupTypes` publishes and the
/// set of configuration types a configuration definition may name: Quick Setup
/// deploys a service-owned template per type, so a type the service does not offer
/// resolves to no template and cannot be deployed. `Type` is modelled as a patterned
/// string rather than an enum only because the set is versioned out of band, so the
/// membership check lives in [`quick_setup_type_rejection`] instead of at the
/// modelled boundary.
///
/// It is the authority for VERSIONS too, not only for types: each entry carries the
/// version history of that type's template ([`QuickSetupTypeEntry::versions`]), the
/// newest of which it publishes as that type's `LatestVersion` and against which
/// [`resolve_type_version`] resolves every member that names a configuration version.
///
/// It is the authority for the PARAMETERS a definition of a type must provide as well
/// ([`QuickSetupTypeEntry::required_parameters`]): each entry carries the entries its
/// own documented table marks `(Required)`, which is what
/// [`configuration_parameters_rejection`] holds a definition's parameter map to.
pub const QUICK_SETUP_TYPE_CATALOG: &[QuickSetupTypeEntry] = &[
    // Conformance Packs
    QuickSetupTypeEntry {
        type_: "AWSQuickSetupType-CFGCPacks",
        versions: &["1.0", "1.1", "3.0"],
        required_parameters: &[RequiredParameter { key: "CPackNames", when: RequiredWhen::Always }],
    },
    // Config Recording
    QuickSetupTypeEntry {
        type_: "AWSQuickSetupType-CFGRecording",
        versions: &["1.0", "1.1", "1.2", "3.1"],
        // Its table marks every entry of its own `(Optional)`.
        required_parameters: &[],
    },
    // CloudWatch agent configuration
    QuickSetupTypeEntry {
        type_: "AWSQuickSetupType-CWASetup",
        versions: &["1.0", "1.3"],
        // A type this model revision's parameter tables do not itemise, so nothing but
        // the parameters common to every definition is documented for it.
        required_parameters: &[],
    },
    // Default Host Management Configuration
    QuickSetupTypeEntry {
        type_: "AWSQuickSetupType-DHMC",
        versions: &["1.0", "1.1", "2.1"],
        required_parameters: &[RequiredParameter {
            key: PARAMETER_TARGET_ORGANIZATIONAL_UNITS,
            when: RequiredWhen::Always,
        }],
    },
    // DevOps Guru
    QuickSetupTypeEntry {
        type_: "AWSQuickSetupType-DevOpsGuru",
        versions: &["1.0", "3.0"],
        // Its table marks every entry of its own `(Optional)`.
        required_parameters: &[],
    },
    // Distributor
    QuickSetupTypeEntry {
        type_: "AWSQuickSetupType-Distributor",
        versions: &["1.0", "1.1", "4.1"],
        required_parameters: &[
            RequiredParameter { key: "PackagesToInstall", when: RequiredWhen::Always },
            RequiredParameter { key: "TargetTagKey", when: TARGET_TYPE_IS_TAGS },
            RequiredParameter { key: "TargetTagValue", when: TARGET_TYPE_IS_TAGS },
            RequiredParameter { key: "ResourceGroupName", when: TARGET_TYPE_IS_RESOURCE_GROUPS },
        ],
    },
    // Just-in-time node access
    QuickSetupTypeEntry {
        type_: "AWSQuickSetupType-JITNA",
        versions: &["1.0", "1.1"],
        // A type this model revision's parameter tables do not itemise, so nothing but
        // the parameters common to every definition is documented for it.
        required_parameters: &[],
    },
    // Patch Policy
    QuickSetupTypeEntry {
        type_: "AWSQuickSetupType-PatchPolicy",
        versions: &["1.0", "1.1", "2.0", "2.6"],
        required_parameters: &[
            RequiredParameter { key: "PatchPolicyName", when: RequiredWhen::Always },
            RequiredParameter { key: "SelectedPatchBaselines", when: RequiredWhen::Always },
            RequiredParameter { key: "TargetTagKey", when: TARGET_TYPE_IS_TAGS },
            RequiredParameter { key: "TargetTagValue", when: TARGET_TYPE_IS_TAGS },
            RequiredParameter { key: "ResourceGroupName", when: TARGET_TYPE_IS_RESOURCE_GROUPS },
        ],
    },
    // Resource Explorer
    QuickSetupTypeEntry {
        type_: "AWSQuickSetupType-ResourceExplorer",
        versions: &["1.0", "1.1", "2.2"],
        required_parameters: &[
            RequiredParameter { key: "SelectedAggregatorRegion", when: RequiredWhen::Always },
            RequiredParameter { key: "ReplaceExistingAggregator", when: RequiredWhen::Always },
            RequiredParameter {
                key: PARAMETER_TARGET_ORGANIZATIONAL_UNITS,
                when: RequiredWhen::Always,
            },
        ],
    },
    // Quick Setup default configuration
    QuickSetupTypeEntry {
        type_: "AWSQuickSetupType-SSM",
        versions: &["1.0", "3.0"],
        // The default configuration type, whose own table the model does not itemise:
        // nothing but the parameters common to every definition is documented for it.
        required_parameters: &[],
    },
    // Change Manager
    QuickSetupTypeEntry {
        type_: "AWSQuickSetupType-SSMChangeMgr",
        versions: &["1.0", "1.1", "2.2"],
        required_parameters: &[
            RequiredParameter { key: "DelegatedAccountId", when: RequiredWhen::Always },
            RequiredParameter { key: "JobFunction", when: RequiredWhen::Always },
            RequiredParameter {
                key: PARAMETER_TARGET_ORGANIZATIONAL_UNITS,
                when: RequiredWhen::Always,
            },
        ],
    },
    // Host Management
    QuickSetupTypeEntry {
        type_: "AWSQuickSetupType-SSMHostMgmt",
        versions: &["1.0", "1.1", "2.0", "5.1"],
        // Its table marks every entry of its own `(Optional)` — including the three
        // targeting keys the Distributor and Patch Policy tables mark `(Required)`.
        required_parameters: &[],
    },
    // OpsCenter
    QuickSetupTypeEntry {
        type_: "AWSQuickSetupType-SSMOpsCenter",
        versions: &["1.0", "1.1", "1.6"],
        required_parameters: &[
            RequiredParameter { key: "DelegatedAccountId", when: RequiredWhen::Always },
            RequiredParameter {
                key: PARAMETER_TARGET_ORGANIZATIONAL_UNITS,
                when: RequiredWhen::Always,
            },
        ],
    },
    // Resource Scheduler
    QuickSetupTypeEntry {
        type_: "AWSQuickSetupType-Scheduler",
        versions: &["1.0", "2.0", "4.0"],
        required_parameters: &[
            RequiredParameter { key: "TargetTagKey", when: RequiredWhen::Always },
            RequiredParameter { key: "TargetTagValue", when: RequiredWhen::Always },
            RequiredParameter { key: "ICalendarString", when: RequiredWhen::Always },
        ],
    },
];

/// The condition the Distributor and Patch Policy tables state on the two tag-targeting
/// parameters they mark `(Required)`: a value must be provided for either of them only
/// when the definition targets instances by tag, which its `TargetType` parameter is
/// what says ("You must provide a value for this parameter if you specify `Tags` for
/// the `TargetType` parameter").
const TARGET_TYPE_IS_TAGS: RequiredWhen = RequiredWhen::ParameterIsOneOf {
    key: "TargetType",
    values: &["Tags"],
};

/// The same condition on the resource-group parameter those two tables mark
/// `(Required)`: it is required only when the definition targets instances by resource
/// group.
const TARGET_TYPE_IS_RESOURCE_GROUPS: RequiredWhen = RequiredWhen::ParameterIsOneOf {
    key: "TargetType",
    values: &["ResourceGroups"],
};

/// The catalog entry for a configuration type, or `None` when this service does not
/// offer it. Membership is exact — a type name differing only in case is not a
/// published type. The one place the catalog is looked up.
pub fn quick_setup_type_entry(type_: &str) -> Option<&'static QuickSetupTypeEntry> {
    QUICK_SETUP_TYPE_CATALOG
        .iter()
        .find(|entry| entry.type_ == type_)
}

/// Why the value of a configuration-type member is unacceptable, or `None` when this
/// service offers that Quick Setup configuration type.
///
/// A configuration definition's `Type` selects the service-owned template Quick Setup
/// deploys, so the set of acceptable values is exactly the catalog the service
/// publishes ([`QUICK_SETUP_TYPE_CATALOG`], what `ListQuickSetupTypes` answers): a
/// pattern-valid string the catalog does not carry names no template, so it is
/// refused at validation rather than minting a manager that could never deploy.
///
/// The one place the rule lives, so every member that names a configuration type
/// refuses the same values. The message names the member and the requirement, the way
/// this service's other refusals do, and renders the offending value only where
/// [`echoed_clause`] allows.
pub fn quick_setup_type_rejection(member: &str, value: &str) -> Option<String> {
    if quick_setup_type_entry(value).is_some() {
        return None;
    }
    Some(format!(
        "{member} must name a Quick Setup configuration type this service offers \
         (one of the types ListQuickSetupTypes publishes){}",
        echoed_clause(member, value)
    ))
}

/// The published configuration version a definition of `type_` is deployed at when it
/// names `requested`, or why that version cannot be served.
///
/// A configuration version names one shipped revision of the service-owned template
/// `type_` deploys, so the published catalog is its authority exactly as it is for the
/// type itself:
///
/// * a request naming a version the type publishes is served that version, in the
///   spelling the catalog publishes it under;
/// * a request naming the modelled [`TYPE_VERSION_LATEST`] alias, and one naming no
///   version at all, are both served the type's newest published version — so what is
///   stored and reported always names the version that was deployed, which is what
///   lets a caller learn it;
/// * anything else names no revision of that template and is refused, naming the
///   versions the type does publish.
///
/// The one place the rule lives, so every member that names a configuration version
/// resolves and refuses alike: `ConfigurationDefinitionInput$TypeVersion`, whose only
/// modelled constraint is a `@length(1, 128)`, and
/// `UpdateConfigurationDefinitionInput$TypeVersion`, whose modelled pattern gives the
/// form but not the published set.
pub fn resolve_type_version(
    member: &str,
    type_: &str,
    requested: Option<&str>,
) -> Result<String, String> {
    let Some(entry) = quick_setup_type_entry(type_) else {
        // The value rendered here is the definition's `Type`, which the model marks
        // neither `@sensitive` itself nor through any shape on the path to it
        // (`SENSITIVE_MEMBERS`), so it stays echoable — bounded like any other echo.
        return Err(format!(
            "{member} cannot be resolved because '{}' is not a Quick Setup configuration \
             type this service offers",
            echoed_value(type_)
        ));
    };
    let Some(requested) = requested else {
        return Ok(entry.latest_version().to_string());
    };
    entry
        .published_version(requested)
        .map(str::to_string)
        .ok_or_else(|| {
            format!(
                "{member} must name a configuration version this service publishes for \
                 {} (one of [{}]) or the {TYPE_VERSION_LATEST} alias{}",
                entry.type_,
                entry.versions.join(", "),
                echoed_clause(member, requested)
            )
        })
}

/// The `Parameters` key naming the Regions a configuration definition is deployed to.
///
/// Documented on `ConfigurationDefinitionInput$Parameters` for every configuration
/// type the service offers as a comma separated list of the Regions to deploy the
/// configuration to (`smithy_query(query="doc",
/// shape="ConfigurationDefinitionInput", member="Parameters")`).
pub const PARAMETER_TARGET_REGIONS: &str = "TargetRegions";

/// The `Parameters` key naming the accounts a configuration definition is deployed
/// to, documented on the same member as the ID of the account to deploy the
/// configuration to.
pub const PARAMETER_TARGET_ACCOUNTS: &str = "TargetAccounts";

/// The `Parameters` key naming the organizational units a configuration definition is
/// deployed to, documented on the same member as a comma separated list of the
/// organizational units to deploy the configuration to.
pub const PARAMETER_TARGET_ORGANIZATIONAL_UNITS: &str = "TargetOrganizationalUnits";

/// The `Parameters` keys that are COMMON to a configuration definition whatever Quick
/// Setup configuration type it deploys: the keys that say where the definition is
/// deployed.
///
/// Source: the per-type parameter tables documented on
/// `ConfigurationDefinitionInput$Parameters` (`smithy_query(query="doc",
/// shape="ConfigurationDefinitionInput", member="Parameters")`). Every type's table
/// names where the configuration goes with these three keys and describes them in the
/// same words; every OTHER key in those tables belongs to one type's own
/// service-owned template (a conformance-pack list, a recorder's delivery bucket, a
/// schedule, an agent toggle) and so is specific to that type rather than common to
/// definitions.
///
/// This is the set a SUMMARY reports ([`first_class_parameters`]), as opposed to the
/// full parameter map a definition carries, so the one place the distinction lives is
/// here.
pub const COMMON_PARAMETER_KEYS: &[&str] = &[
    PARAMETER_TARGET_ACCOUNTS,
    PARAMETER_TARGET_ORGANIZATIONAL_UNITS,
    PARAMETER_TARGET_REGIONS,
];

/// The common parameters and values of a definition holding `parameters`: the entries
/// whose keys are in [`COMMON_PARAMETER_KEYS`], and no others.
///
/// This is what both summary shapes report as their `FirstClassParameters`
/// (`ConfigurationSummary` and `ConfigurationDefinitionSummary`, both documented as
/// the common parameters and values for the configuration definition) — a listing
/// reports the common parameters of a definition, never the type-specific options its
/// full `Parameters` map also holds, which is why the model carries the two members
/// separately and why a value may run to 40960 characters on the full one.
///
/// A key matches exactly, as it does everywhere else a parameter is read
/// ([`ConfigurationParameters::parameter`]): a definition that spells a key
/// differently names a parameter of its own type's template, not a common one. A
/// definition carrying none of them reports an empty map rather than nothing, because
/// the member reports what the definition holds.
pub fn first_class_parameters(parameters: &BTreeMap<String, String>) -> HashMap<String, String> {
    COMMON_PARAMETER_KEYS
        .iter()
        .filter_map(|key| parameters.get_key_value(*key))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect()
}

/// Why a configuration definition whose parameters name no deployment target is
/// refused.
///
/// Documented on `ConfigurationDefinitionInput$Parameters` for every configuration type
/// that leaves both target keys optional: a value must be provided for either
/// [`PARAMETER_TARGET_ACCOUNTS`] or [`PARAMETER_TARGET_ORGANIZATIONAL_UNITS`]
/// (`smithy_query(query="doc", shape="ConfigurationDefinitionInput",
/// member="Parameters")`). The sentence carries no value of the request, so it names no
/// member: one definition either names somewhere to deploy to or it does not.
pub const NO_DEPLOYMENT_TARGET_MESSAGE: &str =
    "Either TargetAccounts or TargetOrganizationalUnits must be defined in parameters.";

/// Why a configuration definition whose parameters name no Region to deploy into is
/// refused.
///
/// `ConfigurationDefinitionInput$Parameters` documents one parameter table per
/// configuration type, and those tables mark [`PARAMETER_TARGET_REGIONS`]
/// `(Required)` — in deliberate contrast to the two
/// target keys beside it, which the same tables mark optional and require only one of
/// (`smithy_query(query="doc", shape="ConfigurationDefinitionInput",
/// member="Parameters")`). A definition is deployed into the Regions it names and into
/// no others, so a parameter set naming none names nowhere to deploy.
///
/// Worded the way this service words [`NO_DEPLOYMENT_TARGET_MESSAGE`], the other
/// refusal of a parameter map that leaves out what it must provide, and naming the
/// parameter that is missing. It carries no value of the request: one definition either
/// names the Regions to deploy into or it does not. The same sentence
/// [`missing_parameter_message`] builds for every other parameter a definition must
/// provide, this one being the parameter every type's table requires.
pub const NO_DEPLOYMENT_REGION_MESSAGE: &str = "TargetRegions must be defined in parameters.";

/// Why a create whose `ConfigurationDefinitions` list carries no definition at all is
/// refused.
///
/// `CreateConfigurationManagerInput$ConfigurationDefinitions` is `@required` and its
/// target `ConfigurationDefinitionsInputList` carries no `@length`, so the EMPTY list
/// reaches the handler and the non-empty rule is the service's rather than the model's: a
/// configuration manager is the deployment of its definitions, so one with none to deploy
/// is refused. Recorded behavior supplies the sentence, which is wholly fixed text —
/// nothing in it is interpolated from the request or the environment.
pub const NO_CONFIGURATION_DEFINITIONS_MESSAGE: &str =
    "At least 1 configuration definition must be provided.";

/// Why a configuration definition whose parameters leave out one the documented table
/// for its own configuration type requires is refused.
///
/// `ConfigurationDefinitionInput$Parameters` documents one parameter table per
/// configuration type and marks entries of it `(Required)`
/// ([`QuickSetupTypeEntry::required_parameters`]): a definition deploys that type's
/// service-owned template with the parameters it provides, so one that provides none
/// for a parameter the template requires cannot be deployed.
///
/// Worded the way this service words the two refusals of a parameter map that leaves
/// out a parameter every type requires ([`NO_DEPLOYMENT_TARGET_MESSAGE`] and
/// [`NO_DEPLOYMENT_REGION_MESSAGE`], which is this sentence for
/// [`PARAMETER_TARGET_REGIONS`]), naming the parameter that is missing. The key is one
/// the catalog publishes rather than anything the request carried, so the sentence
/// renders no value of the request.
pub fn missing_parameter_message(key: &str) -> String {
    format!("{key} must be defined in parameters.")
}

/// The `ConfigurationDefinitionInput` member naming the role a LOCAL configuration
/// deployment is administered with: "The ARN of the IAM role used to administrate local
/// configuration deployments" (`smithy_query(query="shape",
/// shape="ConfigurationDefinitionInput")`). `UpdateConfigurationDefinition` binds the
/// same member.
pub const LOCAL_DEPLOYMENT_ADMINISTRATION_ROLE_ARN: &str = "LocalDeploymentAdministrationRoleArn";

/// Why a request leaving out a MEMBER this service requires of it is refused.
///
/// A different sentence from [`missing_parameter_message`] because it is about a
/// different thing: that one names an entry of a definition's `Parameters` map, this one
/// names a member of the request's own shape. Both are requirements the model cannot
/// state, because both hold only under a condition about another value of the same
/// request — which is why the member is modelled optional and refused here instead.
/// Carries nothing of the request but the member's own modelled name.
pub fn missing_required_member_message(member: &str) -> String {
    format!("The required parameter {member} is missing.")
}

/// Whether a configuration definition holding `parameters` deploys LOCALLY: whether its
/// parameters name [`PARAMETER_TARGET_ACCOUNTS`], documented as the ID of the account
/// initiating the configuration deployment, which a request provides only if it wants to
/// deploy the configuration locally (`smithy_query(query="doc",
/// shape="ConfigurationDefinitionInput", member="Parameters")`). A definition naming
/// [`PARAMETER_TARGET_ORGANIZATIONAL_UNITS`] alone is deployed through the organization
/// instead, and needs nothing a local deployment needs.
///
/// Read through the same [`comma_separated`] reading every other rule about a target list
/// applies: a key written with no entries names no account, so such a definition is no
/// local deployment.
pub fn deploys_locally<P: ConfigurationParameters + ?Sized>(parameters: &P) -> bool {
    !comma_separated(parameters.parameter(PARAMETER_TARGET_ACCOUNTS)).is_empty()
}

/// Why a configuration definition that deploys LOCALLY cannot be served without the role
/// such a deployment is administered with, or `None` when it can.
///
/// A definition deploys locally exactly when [`deploys_locally`] says so. A local
/// deployment is administered with the role
/// [`LOCAL_DEPLOYMENT_ADMINISTRATION_ROLE_ARN`] names, so one that names none cannot be
/// administered at all and is refused before anything is stored — which is why that
/// member is modelled optional and required here: a definition deployed through an
/// organization needs no local administration role, so the requirement is conditional on
/// another value of the same request and no per-member constraint can express it.
///
/// The member beside it, `LocalDeploymentExecutionRoleName`, is a role NAME rather than
/// an ARN and this service requires none: the recorded refusal of a local deployment
/// names only the administration role, and the model states no requirement for the other.
///
/// Whether the role a definition DOES name can administer the deployment is the other
/// half of the rule, and lives in [`local_administration_role`].
///
/// The one place this half lives, so a create and an update refuse the same definitions.
pub fn local_deployment_rejection<P: ConfigurationParameters + ?Sized>(
    parameters: &P,
    administration_role_arn: Option<&str>,
) -> Option<String> {
    if !deploys_locally(parameters) {
        return None;
    }
    administration_role_arn
        .is_none_or(|arn| arn.trim().is_empty())
        .then(|| missing_required_member_message(LOCAL_DEPLOYMENT_ADMINISTRATION_ROLE_ARN))
}

/// How many account-and-Region deployments ONE configuration definition may fan out
/// to.
///
/// Both target lists are caller-sized text inside a parameter value, and each pair
/// they name becomes a stored configuration, so the product is bounded here rather
/// than letting one create grow the store without bound. Emulator resource guard, NOT
/// a published AWS quota; far above the number of Regions AWS publishes.
pub const MAX_DEPLOYMENT_TARGETS: usize = 100;

/// How many entries this service reads out of ONE comma-separated parameter value.
///
/// `ConfigurationParametersMap` lets a value run to 40960 characters, which is tens of
/// thousands of comma-separated entries, while the deployments one definition fans out
/// to are bounded by [`MAX_DEPLOYMENT_TARGETS`]: a target list naming more entries than
/// that already names more deployments than this service serves, so it is refused
/// whatever its further entries say. Reading stops one entry past that bound, so the
/// work one request costs is bounded by the bound rather than by the length of the
/// value it sent. Emulator resource guard, NOT a published AWS quota.
pub const MAX_PARAMETER_LIST_ENTRIES: usize = MAX_DEPLOYMENT_TARGETS + 1;

/// How many configurations this emulator will hold at once.
///
/// One create deploys one configuration per targeted account and Region of EVERY
/// definition it carries, so its fan-out is the product of
/// [`MAX_CONFIGURATION_DEFINITIONS`] and [`MAX_DEPLOYMENT_TARGETS`]; this bounds what
/// one request may add to the store instead of letting that product stand. Emulator
/// resource guard, NOT a published AWS quota, and far above what a manager of a
/// realistic size deploys.
pub const MAX_CONFIGURATIONS: usize = 2_000;

/// Digits in an Amazon Web Services account id, the form a `TargetAccounts` entry
/// takes.
const ACCOUNT_ID_DIGITS: usize = 12;

/// One place a configuration definition is deployed: the account and the Region the
/// deployed configuration reports as `ConfigurationSummary$Account` / `$Region`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct DeploymentTarget {
    pub account: String,
    pub region: String,
}

/// The entries of one comma-separated parameter value: each trimmed, empty entries
/// dropped, repeats dropped, and the order the caller wrote them kept.
///
/// Repeats are recognised through a set rather than by re-scanning what has been kept,
/// so one caller-sized value costs work proportional to its entries and not to their
/// square, and reading stops at [`MAX_PARAMETER_LIST_ENTRIES`] — one entry past what
/// may be deployed, which is enough for [`configuration_parameters_rejection`] to
/// refuse the value.
pub fn comma_separated(value: Option<&str>) -> Vec<String> {
    let mut entries: Vec<String> = Vec::new();
    let mut seen: ::std::collections::HashSet<&str> = ::std::collections::HashSet::new();
    for entry in value.unwrap_or_default().split(',') {
        let entry = entry.trim();
        if entry.is_empty() || !seen.insert(entry) {
            continue;
        }
        entries.push(entry.to_string());
        if entries.len() == MAX_PARAMETER_LIST_ENTRIES {
            break;
        }
    }
    entries
}

/// A configuration definition's `Parameters`, whichever map holds them.
///
/// `ConfigurationParametersMap` reaches a handler as a `HashMap` on the modelled input
/// and is stored as a [`BTreeMap`] on the record, and what a parameter MEANS is the
/// same either way, so the rules below read a definition's parameters through this one
/// accessor rather than through whichever map the caller happens to hold.
pub trait ConfigurationParameters {
    /// The value the parameter `key` carries, or `None` when the definition names no
    /// such parameter.
    fn parameter(&self, key: &str) -> Option<&str>;
}

impl ConfigurationParameters for HashMap<String, String> {
    fn parameter(&self, key: &str) -> Option<&str> {
        self.get(key).map(String::as_str)
    }
}

impl ConfigurationParameters for BTreeMap<String, String> {
    fn parameter(&self, key: &str) -> Option<&str> {
        self.get(key).map(String::as_str)
    }
}

/// The deployment-target parameters one configuration definition names, each `None`
/// when the definition carries no such parameter.
struct DeploymentTargetParameters<'a> {
    /// The [`PARAMETER_TARGET_REGIONS`] value.
    regions: Option<&'a str>,
    /// The [`PARAMETER_TARGET_ACCOUNTS`] value.
    accounts: Option<&'a str>,
    /// The [`PARAMETER_TARGET_ORGANIZATIONAL_UNITS`] value.
    organizational_units: Option<&'a str>,
}

/// The deployment-target parameters `parameters` names.
///
/// The one place the extraction lives, so the refusal and the fan-out below take a
/// definition's targets from the same parameter keys: which keys name a target is a
/// change here, not in each handler that writes a definition.
fn deployment_target_parameters<P: ConfigurationParameters + ?Sized>(
    parameters: &P,
) -> DeploymentTargetParameters<'_> {
    DeploymentTargetParameters {
        regions: parameters.parameter(PARAMETER_TARGET_REGIONS),
        accounts: parameters.parameter(PARAMETER_TARGET_ACCOUNTS),
        organizational_units: parameters.parameter(PARAMETER_TARGET_ORGANIZATIONAL_UNITS),
    }
}

/// Where a configuration definition carrying `parameters` is deployed: ONE target per
/// targeted account and Region pair.
///
/// A Quick Setup configuration definition names where it is deployed through its
/// parameters, and the service deploys one configuration per targeted account and
/// Region, which is what that configuration reports as the account and Region it was
/// deployed in. The Regions are exactly the ones the definition's
/// [`PARAMETER_TARGET_REGIONS`] names — nothing stands in for a Region a definition
/// never asked for, so a parameter set naming none is deployed nowhere at all, which
/// is what [`configuration_parameters_rejection`] refuses before any of it is stored.
/// One naming no account is deployed in `local_account` — the single account this
/// emulator serves, which is also the only account the organizational units a
/// definition may target instead could resolve to here.
pub fn deployment_targets<P: ConfigurationParameters + ?Sized>(
    parameters: &P,
    local_account: &str,
) -> Vec<DeploymentTarget> {
    let targets = deployment_target_parameters(parameters);
    let regions = comma_separated(targets.regions);
    let mut accounts = comma_separated(targets.accounts);
    if accounts.is_empty() {
        accounts.push(local_account.to_string());
    }
    accounts
        .iter()
        .flat_map(|account| {
            regions.iter().map(move |region| DeploymentTarget {
                account: account.clone(),
                region: region.clone(),
            })
        })
        .collect()
}

/// How many account-and-Region deployments the parameter set `parameters` names — what
/// [`deployment_targets`] would build from it, counted rather than built.
///
/// The fan-out is the PRODUCT of the accounts and the Regions a definition names, so the
/// count is that product: `accounts.len().max(1)` (a definition naming no account is
/// deployed in the one account this emulator serves, exactly as [`deployment_targets`]
/// builds it) times `regions.len()`. Arithmetic rather than a built list because both
/// factors come from ONE request's parameter values, whose length is all the model bounds:
/// building the product to measure it would allocate the very fan-out
/// [`MAX_DEPLOYMENT_TARGETS`] exists to refuse. `saturating_mul`, so a product past
/// `usize` reports the largest count rather than wrapping to a small one.
///
/// The one place the count lives, so every bound read against it — the per-definition
/// [`MAX_DEPLOYMENT_TARGETS`] and the store's own
/// [`configuration_capacity_rejection`] — counts the same deployments the service then
/// performs.
pub fn deployment_target_count<P: ConfigurationParameters + ?Sized>(parameters: &P) -> usize {
    let targets = deployment_target_parameters(parameters);
    let regions = comma_separated(targets.regions).len();
    let accounts = comma_separated(targets.accounts).len().max(1);
    accounts.saturating_mul(regions)
}

/// Why the parameter set a configuration definition of `type_` holds cannot be served,
/// or `None` when it can.
///
/// `ConfigurationParametersMap` bounds each key and value but says nothing about what
/// a value means, so these rules live here: a parameter set must provide every
/// parameter the documented parameter tables require of it, which is
/// `type_`-dependent because those tables are documented one per configuration type
/// ([`QuickSetupTypeEntry::required_parameters`]):
///
/// * somewhere to deploy to — either of the two target keys for a type whose table
///   marks them `(Optional)` and requires one of the two
///   ([`NO_DEPLOYMENT_TARGET_MESSAGE`]), and [`PARAMETER_TARGET_ORGANIZATIONAL_UNITS`]
///   itself for a type whose own table marks that key `(Required)` and names no
///   `TargetAccounts` at all;
/// * the Regions to deploy into, which every type's table marks `(Required)`
///   ([`NO_DEPLOYMENT_REGION_MESSAGE`]);
/// * every parameter of `type_`'s own service-owned template that its table marks
///   `(Required)`, under the condition that table states for it
///   ([`missing_parameter_message`]).
///
/// Beside those: a targeted account is named by its account id, and the number of
/// account-and-Region deployments one definition fans out to is bounded by
/// [`MAX_DEPLOYMENT_TARGETS`]. The one place the rule lives, so a create and an update
/// refuse the same parameter sets — and, counting the deployments
/// [`deployment_targets`] would build from the same parameters
/// ([`deployment_target_count`], which counts the single account a definition naming
/// none is deployed in the same way that fan-out does), refuse exactly
/// the sets that cannot be deployed. `member` labels the request member the parameters
/// arrived on, so a refusal stating one of this service's own bounds names what the
/// caller sent.
///
/// A `type_` the catalog does not publish carries no documented table, and so no
/// per-type requirement: it is refused for the type itself
/// ([`quick_setup_type_rejection`]) rather than for the parameters of a template this
/// service does not deploy.
pub fn configuration_parameters_rejection<P: ConfigurationParameters + ?Sized>(
    member: &str,
    parameters: &P,
    type_: &str,
) -> Option<String> {
    let entry = quick_setup_type_entry(type_);
    let target_parameters = deployment_target_parameters(parameters);
    let accounts = comma_separated(target_parameters.accounts);
    let organizational_units = comma_separated(target_parameters.organizational_units);
    // A definition is deployed where its parameters say, so a parameter set that names
    // nowhere to deploy to is refused. WHICH of the two target keys says so is the
    // type's own table's to state: the tables that mark the organizational units
    // `(Required)` name no accounts to stand in for them, while the rest leave both
    // keys optional and require a value for either. A key written with no entries
    // provides none, whichever rule reads it.
    //
    // The organizational units are also among the parameters the per-type requirement
    // below reads, so this branch adds no requirement: it decides WHICH of the two
    // refusals a set naming nowhere to deploy is answered with, and answers it here
    // rather than after the Regions and the account form.
    let organizational_units_required = entry
        .is_some_and(|entry| entry.requires(PARAMETER_TARGET_ORGANIZATIONAL_UNITS, parameters));
    if organizational_units_required {
        if organizational_units.is_empty() {
            return Some(missing_parameter_message(
                PARAMETER_TARGET_ORGANIZATIONAL_UNITS,
            ));
        }
    } else if accounts.is_empty() && organizational_units.is_empty() {
        return Some(NO_DEPLOYMENT_TARGET_MESSAGE.to_string());
    }
    // The Regions a definition deploys into are required of every configuration type
    // the service offers, and a key written with no entries provides none, so a
    // parameter set that leaves them out is refused rather than deployed into a Region
    // the caller never named.
    if comma_separated(target_parameters.regions).is_empty() {
        return Some(NO_DEPLOYMENT_REGION_MESSAGE.to_string());
    }
    if let Some(malformed) = accounts.iter().find(|account| {
        account.len() != ACCOUNT_ID_DIGITS || !account.bytes().all(|b| b.is_ascii_digit())
    }) {
        return Some(format!(
            "{member}.{PARAMETER_TARGET_ACCOUNTS} must be a comma separated list of \
             {ACCOUNT_ID_DIGITS}-digit Amazon Web Services account IDs{}",
            echoed_clause(member, malformed)
        ));
    }
    // A definition deploys the service-owned template its type names, with the
    // parameters that template takes, so the parameters that type's documented table
    // marks `(Required)` are required of it: a set leaving one out is refused rather
    // than minting a manager and deploying a configuration from a parameter set that
    // type cannot be deployed from.
    if let Some(missing) = entry.and_then(|entry| entry.missing_required_parameter(parameters)) {
        return Some(missing_parameter_message(missing));
    }
    // The deployments are COUNTED rather than built ([`deployment_target_count`]): the
    // count is the same one the definition fans out to, so whatever a parameter set fans
    // out to is what is bounded here, and a set naming more deployments than this
    // service performs is refused without that fan-out ever being allocated.
    let targets = deployment_target_count(parameters);
    (targets > MAX_DEPLOYMENT_TARGETS).then(|| {
        format!(
            "{member} names {targets} account and Region deployments, more than the \
             {MAX_DEPLOYMENT_TARGETS} this service deploys for one configuration definition"
        )
    })
}

/// The settings Quick Setup holds for ONE account and region: what the last
/// successful `UpdateServiceSettings` wrote there.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceSettingsRecord {
    /// `ServiceSettings$ExplorerEnablingRoleArn`, or `None` while no update has
    /// named one.
    pub explorer_enabling_role_arn: Option<String>,
    /// When the record was last written. Not on the wire — the modelled
    /// `ServiceSettings` carries no timestamp — it is what distinguishes one write
    /// of the record from the next.
    pub last_modified_at: SystemTime,
}

impl ServiceSettingsRecord {
    /// The modelled `ServiceSettings` for this record.
    pub fn to_service_settings(&self) -> ServiceSettings {
        ServiceSettings {
            explorer_enabling_role_arn: self.explorer_enabling_role_arn.clone(),
        }
    }
}

/// A status change the clock owes a status summary: where it lands, and when.
#[derive(Debug, Clone)]
pub struct PendingStatus {
    /// The `Status` the summary moves into.
    pub to: String,
    /// The instant the change comes due.
    pub due_at: SystemTime,
}

/// One stored `StatusSummary`: the state of one kind of work on a manager or a
/// configuration.
#[derive(Debug, Clone)]
pub struct StatusSummaryRecord {
    /// A `StatusType` value.
    pub status_type: String,
    /// A `Status` value.
    pub status: String,
    /// The informational message this status and status type call for, from
    /// [`status_message`], or `None` where the status is the whole of what there is to
    /// report.
    pub status_message: Option<String>,
    pub last_updated_at: SystemTime,
    /// Free-form details about the status, reported through the modelled
    /// `StatusDetails` map whenever there are any.
    ///
    /// The model documents the member as "Details about the status" and publishes no
    /// key for it, so nothing goes here that this service does not actually hold: a
    /// summary with no details to report carries the member absent rather than keys
    /// nothing produced.
    pub status_details: BTreeMap<String, String>,
    /// The phases the clock still owes this summary, in the order they come due, and
    /// empty once the work it reports has reached its terminal status.
    pub settles: VecDeque<PendingStatus>,
}

impl StatusSummaryRecord {
    /// The summary of ONE kind of work that walks `phases`: reported in the first of
    /// them from `now`, with every later phase coming due a share of
    /// [`TRANSITION_SPAN`] after the one before, so the work reaches its terminal
    /// phase one whole span after the call that started it and each phase in between
    /// is observable to a caller that polls. Work with a single phase has nothing the
    /// clock owes it.
    ///
    /// The one place a phased summary is built, so the deployment and the asynchronous
    /// executions of one manager (or of one of its configurations) are reported, moved
    /// and messaged the same way.
    fn walking(status_type: &str, phases: &[&str], now: SystemTime) -> Self {
        // Total in both directions: a kind of work named with no phase at all reports
        // that nothing has run, and one named with a single phase has nothing the clock
        // owes it.
        let first = phases.first().copied().unwrap_or(STATUS_NONE);
        let later = phases.get(1..).unwrap_or_default();
        Self {
            status_type: status_type.to_string(),
            status: first.to_string(),
            status_message: status_message(status_type, first),
            last_updated_at: now,
            status_details: BTreeMap::new(),
            settles: later
                .iter()
                .enumerate()
                .map(|(step, status)| PendingStatus {
                    to: (*status).to_string(),
                    due_at: now + TRANSITION_SPAN * (step as u32 + 1) / later.len() as u32,
                })
                .collect(),
        }
    }

    /// The deployment summary a configuration manager (or one of its configurations)
    /// carries from the moment its deployment is submitted: the first of
    /// [`DEPLOYMENT_PHASES`] now, each later one as the clock makes it due.
    pub fn deployment(now: SystemTime) -> Self {
        Self::walking(STATUS_TYPE_DEPLOYMENT, &DEPLOYMENT_PHASES, now)
    }

    /// The summary of the asynchronous executions that same deployment runs, walking
    /// [`ASYNC_EXECUTION_PHASES`]: none has run while the deployment is still rolling
    /// out, and the outcome is reported once it has.
    pub fn async_executions(now: SystemTime) -> Self {
        Self::walking(STATUS_TYPE_ASYNC_EXECUTIONS, &ASYNC_EXECUTION_PHASES, now)
    }

    /// The summary the work on a configuration manager (or on one of its
    /// configurations) carries from the moment a delete is accepted: `DELETING`,
    /// with nothing to settle into — the record it belongs to is removed when the
    /// teardown completes, so no later status is ever reported for it.
    pub fn teardown(now: SystemTime) -> Self {
        Self::walking(STATUS_TYPE_DEPLOYMENT, &[STATUS_DELETING], now)
    }

    /// Materialise every phase change the clock already owes this summary.
    ///
    /// This is the ONLY write a reporting operation makes: each change was owed at
    /// this instant whether or not anybody called, so a summary whose whole span has
    /// passed is reported in its terminal phase even if nobody looked while it was in
    /// an earlier one. Nothing is cloned on the way: a summary whose next phase is not
    /// yet due is left exactly as it was.
    fn settle(&mut self, now: SystemTime) {
        while self
            .settles
            .front()
            .is_some_and(|pending| pending.due_at <= now)
        {
            let Some(pending) = self.settles.pop_front() else {
                return;
            };
            self.status = pending.to;
            // The change happened when the clock made it due, not when someone looked.
            self.last_updated_at = pending.due_at.max(self.last_updated_at);
            // The message belongs to the phase now reported, not to the one left.
            self.status_message = status_message(&self.status_type, &self.status);
        }
    }

    /// The modelled `StatusSummary` for this record.
    pub fn to_status_summary(&self) -> StatusSummary {
        StatusSummary {
            status_type: self.status_type.clone(),
            status: Some(self.status.clone()),
            status_message: self.status_message.clone(),
            last_updated_at: ::aws_smithy_types::DateTime::from(self.last_updated_at),
            status_details: (!self.status_details.is_empty())
                .then(|| to_hash_map(&self.status_details)),
        }
    }
}

/// The informational message a summary of `status_type` carries while it reports
/// `status`, or `None` where the status is the whole of what there is to report.
///
/// `StatusSummary$StatusMessage` is documented as the message relevant to the current
/// status AND status type of the summary, carried when applicable — so the pair decides
/// it, and this is the one place it is decided: every summary this service reports takes
/// its message from here, and a summary the clock moves gets the message of the phase it
/// moved into rather than keeping the one it left.
pub fn status_message(status_type: &str, status: &str) -> Option<String> {
    let message = match (status_type, status) {
        (STATUS_TYPE_DEPLOYMENT, STATUS_INITIALIZING) => "The deployment is initializing.",
        (STATUS_TYPE_DEPLOYMENT, STATUS_DEPLOYING) => "The deployment is in progress.",
        (STATUS_TYPE_DEPLOYMENT, STATUS_DELETING) => {
            "The configuration and the resources deployed for it are being deleted."
        }
        (STATUS_TYPE_ASYNC_EXECUTIONS, STATUS_NONE) => {
            "No asynchronous executions have run for this configuration yet."
        }
        (STATUS_TYPE_ASYNC_EXECUTIONS, STATUS_SUCCEEDED) => {
            "All asynchronous executions have succeeded."
        }
        // A terminal deployment status is the outcome itself: there is no further
        // information to add to it, and the member is carried only when applicable.
        _ => return None,
    };
    Some(message.to_string())
}

/// The status summaries a manager (or one of its configurations) carries from the
/// moment its deployment is submitted: ONE per modelled `StatusType` — the deployment
/// itself, and the asynchronous executions that deployment runs on the accounts and
/// Regions it targets — each walking its own phases on the clock.
///
/// The one place the set is built, so a manager and every configuration deployed from
/// one of its definitions report the same kinds of work.
pub fn fresh_summaries(now: SystemTime) -> Vec<StatusSummaryRecord> {
    vec![
        StatusSummaryRecord::deployment(now),
        StatusSummaryRecord::async_executions(now),
    ]
}

/// Submit `summaries` for deployment again: every kind of work starts over from its
/// first phase, due to walk the rest as the clock makes them due.
///
/// A mutation that changes what the service deploys (`UpdateConfigurationDefinition`)
/// starts a deployment exactly as `CreateConfigurationManager` does — and with it the
/// asynchronous executions that deployment runs — so the same records and the same
/// clock-owed phases serve both.
pub fn restart_deployment(summaries: &mut Vec<StatusSummaryRecord>, now: SystemTime) {
    *summaries = fresh_summaries(now);
}

/// Write `requested` over `stored` when the two differ, answering whether the stored
/// value actually changed.
///
/// Every mutation of this service is modelled `@idempotent`, so what a request leaves
/// behind is decided by the values it stores and not by the fact that it was sent: a
/// request writing the value a record already holds has stored nothing, and nothing
/// derived from a write — a `LastModifiedAt` stamp, a re-submitted deployment — is owed
/// for it. The one place that comparison lives, so every member of every record a
/// mutation may replace is guarded the same way.
pub fn store_if_changed<T: PartialEq>(stored: &mut T, requested: T) -> bool {
    if *stored == requested {
        return false;
    }
    *stored = requested;
    true
}

/// Report `summaries` as being torn down: the `Deployment` summary becomes the
/// transient teardown one, and every other kind of summary keeps the status it had
/// reached.
///
/// A manager and each configuration it owns are torn down together, so the same
/// record serves both; this is the one place that construction lives. Accepting a
/// delete ends the work that was in flight, so no phase of any other kind of work
/// comes due after this instant.
pub fn start_teardown(summaries: &mut Vec<StatusSummaryRecord>, now: SystemTime) {
    for summary in summaries.iter_mut() {
        summary.settles.clear();
    }
    let teardown = StatusSummaryRecord::teardown(now);
    match summaries
        .iter_mut()
        .find(|summary| summary.status_type == STATUS_TYPE_DEPLOYMENT)
    {
        Some(existing) => *existing = teardown,
        None => summaries.push(teardown),
    }
}

/// A configuration definition's stored parameter map, SHARED with the configurations
/// deployed from that definition.
///
/// One definition fans out to up to [`MAX_DEPLOYMENT_TARGETS`] configurations, each
/// carrying the parameters it was deployed with, and a parameter map may hold
/// [`MAX_PARAMETERS`] values of up to the 40960 characters
/// `ConfigurationParametersMap` allows: sharing one map keeps what a request stores
/// proportional to what it sent instead of multiplying it by the fan-out. The map is
/// never edited in place — an update stores a NEW map
/// ([`merged_parameters`]) over the shared one — so no record observes another's write.
pub type SharedParameters = ::std::sync::Arc<BTreeMap<String, String>>;

/// A wire parameter map as the store shares it. The one place this conversion lives.
pub fn to_shared_parameters(map: Option<HashMap<String, String>>) -> SharedParameters {
    ::std::sync::Arc::new(to_btree_map(map))
}

/// The parameter set a configuration definition HOLDS once the entries an update
/// carries are applied to the set it already holds: every entry `update` names written
/// over the stored one of the same key, and every other stored entry kept.
///
/// `ConfigurationParametersMap` is a map of INDEPENDENTLY named parameters, and an
/// update that names one of them states that parameter and says nothing about the ones
/// it leaves out, so a parameter the request does not name keeps the value the
/// definition holds for it. What a request names with an EMPTY value the definition goes
/// on holding as an empty entry, and that entry provides nothing — no deployment target,
/// no Region — exactly as an absent key provides nothing, which is how a stored target
/// is dropped. The one place that application lives, so the rules a stored
/// parameter set must satisfy and the set actually written are read off the same map.
pub fn merged_parameters(
    stored: &BTreeMap<String, String>,
    update: &HashMap<String, String>,
) -> BTreeMap<String, String> {
    let mut merged = stored.clone();
    merged.extend(
        update
            .iter()
            .map(|(key, value)| (key.clone(), value.clone())),
    );
    merged
}

/// One configuration definition of a stored manager, with the `Id` the service
/// assigned it.
#[derive(Debug, Clone)]
pub struct ConfigurationDefinitionRecord {
    /// Service-assigned, matching the modelled `^[a-z0-9-]{1,20}$` id pattern that
    /// `UpdateConfigurationDefinition` binds on its `{Id}` label.
    pub id: String,
    pub type_: String,
    /// The published configuration version this definition is deployed at, resolved
    /// against the catalog by [`resolve_type_version`]: a stored definition always
    /// carries one — the version a request named, or the type's newest when it named
    /// none — so a caller can read which version was deployed. The member it is
    /// reported through is modelled optional, so the record keeps it optional too.
    pub type_version: Option<String>,
    pub parameters: SharedParameters,
    pub local_deployment_execution_role_name: Option<String>,
    pub local_deployment_administration_role_arn: Option<String>,
}

impl ConfigurationDefinitionRecord {
    /// The modelled `ConfigurationDefinition` for this record.
    pub fn to_configuration_definition(&self) -> ConfigurationDefinition {
        ConfigurationDefinition {
            r#type: self.type_.clone(),
            parameters: to_hash_map(&self.parameters),
            type_version: self.type_version.clone(),
            local_deployment_execution_role_name: self
                .local_deployment_execution_role_name
                .clone(),
            local_deployment_administration_role_arn: self
                .local_deployment_administration_role_arn
                .clone(),
            id: Some(self.id.clone()),
        }
    }

    /// The modelled `ConfigurationDefinitionSummary` for this record.
    ///
    /// `FirstClassParameters` are this definition's COMMON parameters
    /// ([`first_class_parameters`]), a subset of the full `Parameters` map the record
    /// stores and `to_configuration_definition` reports: a summary reports the
    /// parameters common to a configuration definition, not the options of its type.
    pub fn to_summary(&self) -> ConfigurationDefinitionSummary {
        ConfigurationDefinitionSummary {
            id: Some(self.id.clone()),
            r#type: Some(self.type_.clone()),
            type_version: self.type_version.clone(),
            first_class_parameters: Some(first_class_parameters(&self.parameters)),
        }
    }
}

/// One stored configuration manager: everything `CreateConfigurationManager` was
/// given, plus what the service minted for it.
#[derive(Debug, Clone)]
pub struct ConfigurationManagerRecord {
    pub manager_arn: String,
    /// The Region this manager was created in, the one its ARN carries: the single
    /// Region whose endpoint reports, mutates and tears this manager down, which
    /// [`ComAmazonawsSsmquicksetupQuicksetupState::manager_in`] is the one place to
    /// enforce.
    ///
    /// It is never a Region this manager is deployed INTO: a configuration definition
    /// is deployed into exactly the Regions its own parameters name
    /// ([`deployment_targets`], which is not even given this field), and a parameter
    /// set naming none is refused with [`NO_DEPLOYMENT_REGION_MESSAGE`]
    /// ([`configuration_parameters_rejection`]) rather than deployed here.
    pub region: String,
    pub name: Option<String>,
    pub description: Option<String>,
    pub created_at: SystemTime,
    pub last_modified_at: SystemTime,
    pub tags: BTreeMap<String, String>,
    pub definitions: Vec<ConfigurationDefinitionRecord>,
    pub status_summaries: Vec<StatusSummaryRecord>,
    /// When the teardown a `DeleteConfigurationManager` started comes due, or `None`
    /// while no delete has been accepted for this manager.
    ///
    /// Removing a manager's deployed resources is work the service does after it has
    /// answered the delete, so the record stays readable in its `DELETING` status
    /// until this instant, at which point
    /// [`ComAmazonawsSsmquicksetupQuicksetupState::settle`] removes it and the
    /// configurations it owns. Nothing sleeps: the removal is owed from this instant
    /// on whether or not anybody calls.
    pub deletes_at: Option<SystemTime>,
}

impl ConfigurationManagerRecord {
    /// Whether the teardown of this manager is in flight: a delete has been accepted
    /// for it, so it takes no further mutation and is on its way out of the store.
    pub fn is_deleting(&self) -> bool {
        self.deletes_at.is_some()
    }

    /// The modelled `ConfigurationManagerSummary` for this record.
    pub fn to_summary(&self) -> ConfigurationManagerSummary {
        ConfigurationManagerSummary {
            manager_arn: self.manager_arn.clone(),
            description: self.description.clone(),
            name: self.name.clone(),
            status_summaries: Some(
                self.status_summaries
                    .iter()
                    .map(StatusSummaryRecord::to_status_summary)
                    .collect(),
            ),
            configuration_definition_summaries: Some(
                self.definitions
                    .iter()
                    .map(ConfigurationDefinitionRecord::to_summary)
                    .collect(),
            ),
        }
    }

    /// The value of the `ConfigurationManagerSummary` member a `Filter` key names,
    /// or `None` when this record carries none. Every filterable member of that shape
    /// is caller-supplied text, named by exactly what it holds
    /// ([`FilterField::Text`]). Borrowed, so matching allocates nothing.
    pub fn filter_field(&self, key: &str) -> Option<FilterField<'_>> {
        match key {
            "Name" => self.name.as_deref().map(FilterField::Text),
            "Description" => self.description.as_deref().map(FilterField::Text),
            _ => None,
        }
    }
}

/// Why a store already holding `held` configurations cannot take `planned` further
/// ones, or `None` when it can.
///
/// A create deploys one configuration per targeted account and Region of every
/// definition it carries, so the fan-out of ONE request is the product of the
/// per-definition bounds; what the store holds is bounded by [`MAX_CONFIGURATIONS`]
/// instead, and a request that would take it past that is refused before anything is
/// deployed. The one place the bound is read, so a create and an update refuse alike.
/// `held` is the store as the request found it, so a request that also replaces
/// deployments of its own is measured against no less than it adds.
pub fn configuration_capacity_rejection(held: usize, planned: usize) -> Option<String> {
    (held.saturating_add(planned) > MAX_CONFIGURATIONS).then(|| {
        format!(
            "this request deploys {planned} configurations, more than this service can hold \
             beside the {held} it already deploys (the maximum is {MAX_CONFIGURATIONS})"
        )
    })
}

/// One stored configuration: the deployment of one configuration definition, created
/// with the manager that owns it and reclaimed when that manager goes.
#[derive(Debug, Clone)]
pub struct ConfigurationRecord {
    /// Service-generated, and this record's store key.
    pub id: String,
    pub manager_arn: String,
    pub configuration_definition_id: String,
    pub type_: String,
    pub type_version: Option<String>,
    pub account: String,
    pub region: String,
    pub created_at: SystemTime,
    pub last_modified_at: SystemTime,
    /// The parameters this configuration was deployed with: the SAME map the definition
    /// it deploys holds ([`SharedParameters`]), not a copy of it.
    pub parameters: SharedParameters,
    pub status_summaries: Vec<StatusSummaryRecord>,
}

impl ConfigurationRecord {
    /// The configuration ONE deployment of `definition` is: the definition's own
    /// values, in the account and Region `target` names, submitted for deployment at
    /// `now`. The one place a deployed configuration is built, so a create and an
    /// update that changes a definition's targets deploy the same record. The
    /// definition's parameter map is SHARED with the record rather than copied into it
    /// ([`SharedParameters`]), so a definition deployed to many targets is stored once.
    pub fn deployed(
        id: String,
        manager_arn: &str,
        definition: &ConfigurationDefinitionRecord,
        target: &DeploymentTarget,
        now: SystemTime,
    ) -> Self {
        Self {
            id,
            manager_arn: manager_arn.to_string(),
            configuration_definition_id: definition.id.clone(),
            type_: definition.type_.clone(),
            type_version: definition.type_version.clone(),
            account: target.account.clone(),
            region: target.region.clone(),
            created_at: now,
            last_modified_at: now,
            parameters: SharedParameters::clone(&definition.parameters),
            status_summaries: fresh_summaries(now),
        }
    }

    /// The account and Region this configuration is deployed in.
    pub fn target(&self) -> DeploymentTarget {
        DeploymentTarget {
            account: self.account.clone(),
            region: self.region.clone(),
        }
    }

    /// The modelled `ConfigurationSummary` for this record.
    ///
    /// `FirstClassParameters` are this configuration's COMMON parameters
    /// ([`first_class_parameters`]), the same subset of the same map the definition's
    /// own summary reports; the full map is what `GetConfiguration` answers.
    pub fn to_summary(&self) -> ConfigurationSummary {
        ConfigurationSummary {
            id: Some(self.id.clone()),
            manager_arn: Some(self.manager_arn.clone()),
            configuration_definition_id: Some(self.configuration_definition_id.clone()),
            r#type: Some(self.type_.clone()),
            type_version: self.type_version.clone(),
            region: Some(self.region.clone()),
            account: Some(self.account.clone()),
            created_at: Some(::aws_smithy_types::DateTime::from(self.created_at)),
            first_class_parameters: Some(first_class_parameters(&self.parameters)),
            status_summaries: Some(
                self.status_summaries
                    .iter()
                    .map(StatusSummaryRecord::to_status_summary)
                    .collect(),
            ),
        }
    }

    /// The value of the `ConfigurationSummary` member a `Filter` key names, and how a
    /// filter value names it. Every member but the version holds text that is its own
    /// value; the version this configuration is deployed at has several spellings, so
    /// it is reported with the type whose published history resolves them
    /// ([`FilterField::TypeVersion`]). Borrowed, so matching allocates nothing.
    pub fn filter_field(&self, key: &str) -> Option<FilterField<'_>> {
        match key {
            "Id" => Some(FilterField::Text(&self.id)),
            "ConfigurationDefinitionId" => {
                Some(FilterField::Text(&self.configuration_definition_id))
            }
            "Type" => Some(FilterField::Text(&self.type_)),
            "TypeVersion" => self.type_version.as_deref().map(|version| {
                FilterField::TypeVersion {
                    type_: &self.type_,
                    version,
                }
            }),
            "Account" => Some(FilterField::Text(&self.account)),
            "Region" => Some(FilterField::Text(&self.region)),
            _ => None,
        }
    }
}

/// Per-service state for com-amazonaws-ssmquicksetup-quicksetup. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsSsmquicksetupQuicksetupState {
    /// Every configuration manager, keyed by its ARN. A `BTreeMap` so a list walks a
    /// stable, total order and a page token can name the next ARN.
    ///
    /// The ARN is unique across Regions, so one map holds them all; which Region's
    /// endpoint serves a given manager is its [`ConfigurationManagerRecord::region`],
    /// and an operation reaches it through [`Self::manager_in`] rather than through
    /// this map, so no Region addresses another Region's resource.
    pub managers: BTreeMap<String, ConfigurationManagerRecord>,
    /// Every configuration, keyed by its service-generated id, for the same reason —
    /// and reached, for the same reason, through [`Self::configuration_in`].
    pub configurations: BTreeMap<String, ConfigurationRecord>,
    /// The service settings record of each region one has been written for.
    ///
    /// Quick Setup's settings are per-account and per-Region, and this emulator
    /// serves one account, so the region the request was signed for
    /// ([`request_region`]) is the key. A region with no entry has never been
    /// configured; what a read answers for such a region is
    /// [`Self::service_settings_in`], the one place that rule lives. Bounded by
    /// [`MAX_SETTINGS_REGIONS`].
    pub service_settings: BTreeMap<String, ServiceSettingsRecord>,
    /// Monotonic counter backing configuration-definition id assignment.
    next_definition_id: u64,
    /// Monotonic counter backing uuid assignment (see [`Self::fresh_uuid`]).
    next_uuid: u64,
}

impl ComAmazonawsSsmquicksetupQuicksetupState {
    /// A fresh uuid for a resource this service mints.
    ///
    /// The modelled `ManagerArn` pattern ends in an `8-4-4-4-12` hex uuid and a
    /// configuration's `Id` takes the same shape, so that is what this builds: the
    /// version (`4`) and variant (`8`) nibbles are set, the first three groups come
    /// from the host clock mixed through SplitMix64 (the construction
    /// `service-com-amazonaws-ram-amazonresourcesharing` uses for the same job), and
    /// the last group is this store's own monotonic counter, so two ids one store
    /// hands out can never collide however the clock behaves.
    pub fn fresh_uuid(&mut self) -> String {
        self.next_uuid = self.next_uuid.saturating_add(1);
        let mut seed = now_nanos() ^ self.next_uuid.rotate_left(17);
        let groups: Vec<u64> = (0..3).map(|_| splitmix64(&mut seed)).collect();
        format!(
            "{:08x}-{:04x}-4{:03x}-8{:03x}-{:012x}",
            groups[0] as u32,
            groups[1] as u16,
            groups[2] & 0x0fff,
            (groups[2] >> 16) & 0x0fff,
            self.next_uuid & 0xffff_ffff_ffff,
        )
    }
    /// The service-generated id a freshly deployed configuration is handed out under:
    /// the digits of a fresh uuid ([`Self::fresh_uuid`]) with no separators, which is
    /// the form [`is_configuration_id`] admits.
    ///
    /// Minted through that one generator, so two ids one store hands out can never
    /// collide, and read back through that one form check, so every id this service
    /// reports is an id a later request may name.
    pub fn fresh_configuration_id(&mut self) -> String {
        self.fresh_uuid().replace('-', "")
    }

    /// The next configuration-definition id: a decimal, which satisfies the modelled
    /// `^[a-z0-9-]{1,20}$` id pattern and is unique for every id this store can hand
    /// out (the counter saturates at `u64::MAX`, which no store this emulator can hold
    /// could ever reach — `MAX_CONFIGURATION_MANAGERS` x
    /// `MAX_CONFIGURATION_DEFINITIONS` is its whole capacity).
    pub fn fresh_definition_id(&mut self) -> String {
        self.next_definition_id = self.next_definition_id.saturating_add(1);
        self.next_definition_id.to_string()
    }

    /// The configuration manager `manager_arn` names as the endpoint of `region`
    /// serves it: `None` both when no manager has that ARN at all and when the manager
    /// that does was created through another Region's endpoint.
    ///
    /// Quick Setup is a regional control plane — one endpoint per Region, and a
    /// manager's ARN carries the Region it was created in — so a manager belongs to
    /// exactly that one Region and is reported, mutated and torn down only there. The
    /// one place that scoping lives, so every ARN-addressed operation treats an ARN of
    /// another Region exactly as it treats one that was never minted.
    pub fn manager_in(
        &self,
        region: &str,
        manager_arn: &str,
    ) -> Option<&ConfigurationManagerRecord> {
        self.managers
            .get(manager_arn)
            .filter(|manager| manager.region == region)
    }

    /// [`Self::manager_in`] for a caller that goes on to mutate the manager it finds.
    pub fn manager_in_mut(
        &mut self,
        region: &str,
        manager_arn: &str,
    ) -> Option<&mut ConfigurationManagerRecord> {
        self.managers
            .get_mut(manager_arn)
            .filter(|manager| manager.region == region)
    }

    /// Whether the endpoint of `region` serves `configuration`.
    ///
    /// A configuration is the deployment of one definition of one manager, so the
    /// endpoint that serves it is the one that created that manager — NOT the Region
    /// the deployment went INTO, which is what the definition's target parameters name
    /// and what the configuration's own `Region` member reports.
    pub fn serves_configuration(&self, region: &str, configuration: &ConfigurationRecord) -> bool {
        self.manager_in(region, &configuration.manager_arn)
            .is_some()
    }

    /// The configuration `id` names as the endpoint of `region` serves it, and `None`
    /// when that endpoint serves no configuration with that id (see
    /// [`Self::serves_configuration`]).
    pub fn configuration_in(&self, region: &str, id: &str) -> Option<&ConfigurationRecord> {
        self.configurations
            .get(id)
            .filter(|configuration| self.serves_configuration(region, configuration))
    }

    /// The configuration definition `id` names as the endpoint of `region` serves it,
    /// restricted to the manager `manager_arn` names when a request named one, and
    /// `None` when that endpoint holds no such definition in that scope.
    ///
    /// A definition lives on the manager that declares it, so the manager is part of the
    /// lookup: an id held by another manager than the one a request named is not the
    /// definition that request addressed, exactly as an id held by a manager of another
    /// Region is not one this endpoint holds. A request naming a manager resolves it by
    /// key and looks no further; one naming none walks the managers this endpoint serves,
    /// which [`MAX_CONFIGURATION_MANAGERS`] and [`MAX_CONFIGURATION_DEFINITIONS`] already
    /// bound. The one place that resolution lives, so every operation addressing a
    /// definition by id resolves it the same way.
    pub fn definition_in(
        &self,
        region: &str,
        manager_arn: Option<&str>,
        id: &str,
    ) -> Option<&ConfigurationDefinitionRecord> {
        match manager_arn {
            Some(manager_arn) => self.manager_in(region, manager_arn).and_then(|manager| {
                manager
                    .definitions
                    .iter()
                    .find(|definition| definition.id == id)
            }),
            None => self
                .managers
                .values()
                .filter(|manager| manager.region == region)
                .find_map(|manager| {
                    manager
                        .definitions
                        .iter()
                        .find(|definition| definition.id == id)
                }),
        }
    }

    /// The modelled `ServiceSettings` the endpoint of `region` reports.
    ///
    /// Settings are not a creatable resource — the read binds no not-found and no
    /// validation error, and its only output member is the structure itself — so every
    /// region HAS settings and what a region no update has written answers is that
    /// structure with every member nothing was written for left unset, never an absent
    /// structure. The one place that rule lives, so the absence of an entry in
    /// [`Self::service_settings`] means the same thing to every operation that reports
    /// a region's settings.
    pub fn service_settings_in(&self, region: &str) -> ServiceSettings {
        self.service_settings
            .get(region)
            .map(ServiceSettingsRecord::to_service_settings)
            .unwrap_or_default()
    }

    /// Deploy `definition` of the manager `manager_arn` to exactly `targets`: one
    /// configuration per targeted account and Region, each carrying the definition's
    /// current values.
    ///
    /// A target the definition is already deployed to keeps its configuration, and
    /// with it the service-generated id a caller may already hold, so only what the
    /// definition now says changes and the deployment is submitted again; a target it
    /// is no longer deployed to keeps no configuration; a newly targeted one gains a
    /// fresh configuration. The one place the fan-out lives, so a create and an update
    /// that changes a definition's targets leave the same deployed set.
    pub fn deploy_definition(
        &mut self,
        manager_arn: &str,
        definition: &ConfigurationDefinitionRecord,
        targets: &[DeploymentTarget],
        now: SystemTime,
    ) {
        let owns = |configuration: &ConfigurationRecord| {
            configuration.manager_arn == manager_arn
                && configuration.configuration_definition_id == definition.id
        };
        self.configurations.retain(|_, configuration| {
            !owns(configuration) || targets.contains(&configuration.target())
        });
        let mut deployed: Vec<DeploymentTarget> = Vec::new();
        for configuration in self.configurations.values_mut() {
            if !owns(configuration) {
                continue;
            }
            configuration.type_ = definition.type_.clone();
            configuration.type_version = definition.type_version.clone();
            configuration.parameters = SharedParameters::clone(&definition.parameters);
            // Monotonic: the stamp never moves backwards, whatever the host clock does.
            configuration.last_modified_at = now.max(configuration.last_modified_at);
            restart_deployment(&mut configuration.status_summaries, now);
            deployed.push(configuration.target());
        }
        for target in targets {
            if deployed.contains(target) {
                continue;
            }
            let id = self.fresh_configuration_id();
            let configuration =
                ConfigurationRecord::deployed(id, manager_arn, definition, target, now);
            self.configurations
                .insert(configuration.id.clone(), configuration);
        }
    }

    /// Materialise every status change the clock already owes, so a reporting
    /// operation answers with what is due at `now` rather than with what the last
    /// mutating call left behind.
    pub fn settle(&mut self, now: SystemTime) {
        // A teardown the clock has made due is the removal of the manager and of the
        // configurations it owns: owed at this instant whether or not anybody called.
        let torn_down: Vec<String> = self
            .managers
            .iter()
            .filter(|(_, manager)| manager.deletes_at.is_some_and(|due_at| due_at <= now))
            .map(|(manager_arn, _)| manager_arn.clone())
            .collect();
        for manager_arn in &torn_down {
            self.remove_manager(manager_arn);
        }
        for manager in self.managers.values_mut() {
            for summary in &mut manager.status_summaries {
                summary.settle(now);
            }
        }
        for configuration in self.configurations.values_mut() {
            for summary in &mut configuration.status_summaries {
                summary.settle(now);
            }
        }
    }

    /// Accept the teardown of the manager `manager_arn` names: it and every
    /// configuration it owns report the transient teardown status from now on, and
    /// their removal comes due [`TRANSITION_SPAN`] later. Nothing to do when no
    /// manager has that ARN.
    ///
    /// A configuration is the deployment of one of its manager's definitions, so the
    /// two are torn down as one: what a caller polling after a delete observes is
    /// every record of that manager winding down together.
    pub fn begin_teardown(&mut self, manager_arn: &str, now: SystemTime) {
        let Some(manager) = self.managers.get_mut(manager_arn) else {
            return;
        };
        start_teardown(&mut manager.status_summaries, now);
        // Monotonic: the stamp never moves backwards, whatever the host clock does.
        manager.last_modified_at = now.max(manager.last_modified_at);
        manager.deletes_at = Some(now + TRANSITION_SPAN);
        for configuration in self.configurations.values_mut() {
            if configuration.manager_arn != manager_arn {
                continue;
            }
            start_teardown(&mut configuration.status_summaries, now);
            configuration.last_modified_at = now.max(configuration.last_modified_at);
        }
    }

    /// Remove the manager `manager_arn` names together with every configuration it
    /// owns. `None` when no manager has that ARN.
    pub fn remove_manager(&mut self, manager_arn: &str) -> Option<ConfigurationManagerRecord> {
        let manager = self.managers.remove(manager_arn)?;
        self.configurations
            .retain(|_, configuration| configuration.manager_arn != manager_arn);
        Some(manager)
    }
}

/// The expression the value of an `IAMRoleArn` member has to match, written the way a
/// whole-value `@pattern` is: anchored at both ends.
///
/// `IAMRoleArn` is modelled as a plain string carrying
/// `@aws.api#arnReference(type: "AWS::IAM::Role")` and no `@pattern` or `@length`
/// (`smithy_query(query="shape", shape="IAMRoleArn")`), so the boundary validator admits
/// any text and the form that reference names has to be stated here. It is the ARN form
/// of an IAM role, from the AWS General Reference's IAM ARN formats: the `iam` service
/// with no Region of its own, a twelve-digit account, the `role` resource and the role's
/// path and name — in any partition whose name that reference publishes (`aws` and the
/// `-cn`, `-us-gov`, `-iso*` and `-eusc` partitions).
pub const IAM_ROLE_ARN_PATTERN: &str =
    r"^arn:aws(-cn|-us-gov|-iso(-[a-z])?|-eusc)?:iam::\d{12}:role/?[a-zA-Z_0-9+=,.@\-_/]+.*$";

/// [`IAM_ROLE_ARN_PATTERN`] compiled once, by the same regular-expression engine the
/// boundary validator runs a modelled `@pattern` through, so a value is judged by the
/// expression the refusal advertises rather than by a restatement of it.
static IAM_ROLE_ARN: ::std::sync::LazyLock<::regex::Regex> = ::std::sync::LazyLock::new(|| {
    ::regex::Regex::new(IAM_ROLE_ARN_PATTERN).expect("the IAM role ARN form is a valid expression")
});

/// Why the value of an `IAMRoleArn` member is unacceptable, or `None` when it matches
/// [`IAM_ROLE_ARN_PATTERN`].
///
/// The one place the rule lives, so every `IAMRoleArn` member of this service refuses
/// the same values: `UpdateServiceSettings$ExplorerEnablingRoleArn` and
/// `ConfigurationDefinitionInput$LocalDeploymentAdministrationRoleArn`.
///
/// `path` is the member's VALIDATION PATH, and the message is
/// [`value_constraint_message`]'s: a value outside the form a member's own shape names
/// is refused in the words every other constraint refusal of this endpoint carries.
pub fn iam_role_arn_rejection(path: &str, value: &str) -> Option<String> {
    (!IAM_ROLE_ARN.is_match(value))
        .then(|| value_constraint_message(path, value, IAM_ROLE_ARN_PATTERN))
}

/// The message the boundary layer states for `value` at `path` being outside `pattern`:
/// the counted envelope over one clause naming the member path, the value submitted and
/// the expression it had to match.
///
/// Nothing about the sentence is written here. The clause and the requirement half are
/// the framework's own ([`::aws_smithy_schema_server::Violation::message`]), the
/// envelope around it and the quoting of the expression — without the anchor pair a
/// whole-value pattern is wrapped in — are the cross-service validation renderings' (the
/// same two `crate::service` answers `override_refusal` on). So a constraint this service
/// applies ITSELF, because its model states the form as a reference rather than as a
/// `@pattern`, is refused in exactly the words a constraint the centralized walk could
/// apply is.
///
/// The value is bounded by [`echoed_value`] before it reaches the clause, which is the
/// contract the framework's own violations carry.
pub fn value_constraint_message(path: &str, value: &str, pattern: &str) -> String {
    let clause = ::aws_smithy_schema_server::Violation {
        path: path.to_string(),
        kind: ::aws_smithy_schema_server::ViolationKind::Pattern(
            ::service_common::validation::unanchored(pattern),
        ),
        value: Some(echoed_value(value)),
        // No `IAMRoleArn` member of this model takes its data from a `@sensitive` shape
        // ([`SENSITIVE_MEMBERS`]), so the clause states the value the request sent.
        sensitive: false,
    }
    .message();
    ::service_common::validation_message::envelope_over(::std::slice::from_ref(&clause))
        .unwrap_or(clause)
}

/// How many hex digits a configuration's service-generated id carries.
pub const CONFIGURATION_ID_DIGITS: usize = 32;

/// Whether `id` is in the form this service's configuration ids take.
///
/// A configuration's `Id` is "a service generated identifier for the configuration"
/// (`smithy_query(query="shape", shape="ConfigurationSummary")`) and the model gives it
/// no `@pattern` anywhere — not on the output members that report one and not on the
/// `@httpLabel` that takes one back, whose own
/// `^[a-zA-Z0-9-_/:]{1,100}$` is the generic form of a path segment rather than the form
/// of this id. So the form is the one this service MINTS
/// ([`ComAmazonawsSsmquicksetupQuicksetupState::fresh_configuration_id`]): the
/// [`CONFIGURATION_ID_DIGITS`] hex digits of a uuid, written with no separators. Hex is
/// case-insensitive, so an id differing from a minted one only in the case of its digits
/// is still in the form (it simply names no configuration).
///
/// The one place the form lives, so what this service hands out and what it accepts back
/// cannot drift apart.
pub fn is_configuration_id(id: &str) -> bool {
    id.len() == CONFIGURATION_ID_DIGITS && id.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// Why a configuration id a request carried cannot name a configuration at all, or
/// `None` when it is in the form [`is_configuration_id`] describes.
///
/// A service-generated id that is not in the service's own id form names no
/// configuration this service could ever have minted, so it is a fault in the request's
/// input rather than a fact about what the store holds: it is refused BEFORE any lookup,
/// with the modelled `ValidationException`, and never reaches the not-found path.
///
/// The model declares `ValidationException` with a single `Message` member and states no
/// text for it, so the wording is the service's own: the fixed text `Configuration id `
/// and ` has invalid format.` around the id the REQUEST carried, bounded by
/// [`echoed_value`] the way every caller-supplied value this service echoes is. Nothing
/// else varies.
pub fn configuration_id_rejection(id: &str) -> Option<String> {
    (!is_configuration_id(id)).then(|| {
        format!(
            "Configuration id {} has invalid format.",
            echoed_value(id)
        )
    })
}

/// Longest caller-supplied value echoed in a refusal message.
///
/// The value is caller-controlled, so an unelided echo would amplify one request into
/// an arbitrarily long response. The same bound the framework's own constraint walk
/// applies.
const MAX_ECHOED_VALUE: usize = 256;

/// `value` as it may appear in a refusal message: itself when short, otherwise a
/// character-boundary-safe prefix plus the elision the framework's renderer appends.
///
/// Bounding only — whether a value may be rendered AT ALL is
/// [`is_sensitive_member`]'s question, which [`echoed_clause`] asks for every refusal
/// that ends in one.
fn echoed_value(value: &str) -> String {
    if value.len() <= MAX_ECHOED_VALUE {
        return value.to_string();
    }
    let mut end = MAX_ECHOED_VALUE;
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}... [{} bytes]", &value[..end], value.len())
}

/// The request members of this service whose data the model marks `@sensitive`, so no
/// value taken from one of them may be rendered on the wire outside the member it
/// arrived on.
///
/// Source: the sensitive shapes this model declares are `TagsMap` (what both write
/// paths' `Tags` member targets) and the `Tags`/`TagEntry` pair the tagging reader
/// reports — `smithy_query(query="shape", shape="TagsMap,TagKeys")`, which also shows
/// `TagKeys` carrying no sensitive trait anywhere in its path, so its values stay
/// echoable. Every member of this service that takes a sensitive shape from the caller
/// is named `Tags`.
const SENSITIVE_MEMBERS: &[&str] = &["Tags"];

/// Whether `member` names data the model marks `@sensitive`.
///
/// `member` is the request member path a refusal labels — a top-level member (`Tags`) or
/// a nested one (`ConfigurationDefinitions.Parameters`). A path is sensitive when ANY of
/// its segments names a sensitive member, because a value nested inside a sensitive
/// shape is sensitive too.
fn is_sensitive_member(member: &str) -> bool {
    member
        .split('.')
        .any(|segment| SENSITIVE_MEMBERS.contains(&segment))
}

/// The trailing `, got '<value>'` clause a refusal about `member` carries, or the empty
/// string when that member's data is sensitive.
///
/// A refusal always states the member and the requirement, both request-independent. It
/// also echoes the offending value, so a caller can tell which of the values it sent was
/// refused — EXCEPT when the value comes from a member the model marks `@sensitive`
/// ([`is_sensitive_member`]): rendering it would carry sensitive data into the response
/// body, and into anything that logs it, so such a refusal states the member and the
/// requirement alone. The echoed value is bounded for the reason [`MAX_ECHOED_VALUE`]
/// gives.
fn echoed_clause(member: &str, value: &str) -> String {
    if is_sensitive_member(member) {
        return String::new();
    }
    format!(", got '{}'", echoed_value(value))
}

/// The tag-key prefix Amazon Web Services reserves for its own tags.
///
/// Nothing in the model keeps such a key out: `TagsMap`'s key `@pattern`
/// (`^[A-Za-z0-9 _=@:.+-/]+$`) admits `:`, and `UntagResource$TagKeys` is a bare
/// `list<String>` with no pattern at all, so the boundary walk admits a key under this
/// prefix and the rule has to live here. The AWS General Reference's tag naming limits
/// reserve the prefix for Amazon Web Services' own tags, in any combination of upper and
/// lower case, and a tag under it is not the caller's to assign, edit or delete.
pub const RESERVED_TAG_PREFIX: &str = "aws:";

/// Whether `key` falls under [`RESERVED_TAG_PREFIX`].
///
/// Compared case-INSENSITIVELY, because the prefix is reserved however it is spelled,
/// even though a tag key is otherwise a case-sensitive string. A key whose first bytes
/// are the middle of a multi-byte character cannot spell the ASCII prefix, so such a key
/// is the caller's own.
pub fn is_reserved_tag_key(key: &str) -> bool {
    key.get(..RESERVED_TAG_PREFIX.len())
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case(RESERVED_TAG_PREFIX))
}

/// Why a set of caller-supplied tag keys cannot be served, or `None` when every key is
/// the caller's own to write.
///
/// The one place this rule lives, so every member that takes tag keys from the caller
/// refuses the same ones: `CreateConfigurationManager$Tags`, `TagResource$Tags` and
/// `UntagResource$TagKeys`. Applied BEFORE the named resource is resolved and before the
/// store lock is taken, so a refused request neither stores nor removes anything.
///
/// The named key is the lexically first reserved one, so one request always answers the
/// same way however the collection it came from is iterated. The message names the
/// member and the requirement, the way this service's other refusals do, and renders the
/// offending key only where [`echoed_clause`] allows — so a refusal about a `Tags` map,
/// whose shape the model marks `@sensitive`, carries no key out of it, while one about
/// the non-sensitive `UntagResource$TagKeys` list names the key it refused.
pub fn reserved_tag_key_rejection<'k>(
    member: &str,
    keys: impl IntoIterator<Item = &'k str>,
) -> Option<String> {
    keys.into_iter()
        .filter(|key| is_reserved_tag_key(key))
        .min()
        .map(|key| {
            format!(
                "{member} must not name a tag key under the '{RESERVED_TAG_PREFIX}' prefix, \
                 which is reserved for Amazon Web Services use and cannot be edited or \
                 deleted{}",
                echoed_clause(member, key)
            )
        })
}

/// Why a `Filters` list is unacceptable, or `None` when this operation can serve it.
///
/// Two rules, both applied BEFORE the store lock is taken, so a refused request never
/// occupies it:
///
/// * a filter key that names no field of the listed shape cannot be honoured, and
///   silently ignoring it would answer a narrowing request with unnarrowed results, so
///   it is refused — naming the keys that are accepted;
/// * the list and each filter's `Values` are caller-sized and the model publishes no
///   length for either, while the matching work one request does is records x filters
///   x values, so anything past [`MAX_FILTERS`] / [`MAX_FILTER_VALUES`] is refused
///   rather than evaluated.
pub fn filter_key_rejection(filters: Option<&[Filter]>, supported: &[&str]) -> Option<String> {
    let filters = filters?;
    if filters.len() > MAX_FILTERS {
        return Some(format!(
            "Filters carries {} entries, more than the {MAX_FILTERS} this operation accepts",
            filters.len()
        ));
    }
    if let Some(oversized) = filters
        .iter()
        .find(|filter| filter.values.len() > MAX_FILTER_VALUES)
    {
        return Some(format!(
            "Filters key '{}' carries {} values, more than the {MAX_FILTER_VALUES} this operation accepts",
            oversized.key,
            oversized.values.len()
        ));
    }
    let unsupported = filters
        .iter()
        .find(|filter| !supported.contains(&filter.key.as_str()))?;
    Some(format!(
        "Filters carries key '{}', which this operation does not filter on; accepted keys are [{}]",
        unsupported.key,
        supported.join(", ")
    ))
}

/// The value one record holds for the field a `Filter` key names, and HOW a filter
/// value is compared to it.
///
/// A filter selects the records whose value for its key is one the filter NAMES, so
/// what counts as naming that value is the field's own rule rather than the text the
/// record reports: a field whose value has one spelling is named by that text, while a
/// configuration version has several, so every spelling of a version names it. Carrying
/// the rule beside the value is what keeps the two listings' matching in one place
/// ([`matches_filters`]) while each field keeps its own notion of equality. Borrowed out
/// of the record, so matching allocates nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilterField<'r> {
    /// A field whose stored text IS its value, named by exactly that text: every
    /// filterable member of both summary shapes except the configuration version — an
    /// id, a name, a description, a configuration type, an account, a Region.
    Text(&'r str),
    /// The configuration version of a record deploying `type_`: a filter value names
    /// `version` when it resolves to it against the versions `type_` publishes
    /// ([`QuickSetupTypeEntry::published_version`]) — the one place the version rule
    /// lives, so a filter reads a version the way the members that deploy one do, and
    /// `<major>` and the [`TYPE_VERSION_LATEST`] alias name a published version here too.
    TypeVersion {
        type_: &'r str,
        version: &'r str,
    },
}

impl FilterField<'_> {
    /// Whether a filter value of `wanted` names the value this field holds.
    ///
    /// Costs one comparison for a text field and, for a version, a walk of the
    /// published history of ONE type — a fixed list this service compiles in, so the
    /// work one listing does stays bounded by the bounds
    /// [`filter_key_rejection`] and [`paginate`] already apply.
    pub fn named_by(self, wanted: &str) -> bool {
        match self {
            Self::Text(value) => wanted == value,
            // A stored configuration always names a type the catalog publishes
            // ([`quick_setup_type_rejection`] refuses any other), so the catalog is
            // what resolves the requested spelling — the ONE implementation of the
            // version rule. A type it does not publish has no version history to
            // resolve against, so no value names a version of it.
            Self::TypeVersion { type_, version } => quick_setup_type_entry(type_)
                .and_then(|entry| entry.published_version(wanted))
                .is_some_and(|published| published == version),
        }
    }
}

/// Whether a record whose fields `field` reports satisfies every supplied filter: a
/// filter holds when one of its `Values` names the record's value for its key
/// ([`FilterField::named_by`]).
///
/// Both factors of the per-record cost are bounded before this runs: the filter list
/// and each `Values` list by [`filter_key_rejection`], and the records examined by
/// [`paginate`]. `field` borrows out of the record rather than cloning, so matching a
/// record allocates nothing.
pub fn matches_filters<'r>(
    filters: Option<&[Filter]>,
    field: impl Fn(&str) -> Option<FilterField<'r>>,
) -> bool {
    filters.is_none_or(|filters| {
        filters.iter().all(|filter| {
            field(&filter.key)
                .is_some_and(|value| filter.values.iter().any(|wanted| value.named_by(wanted)))
        })
    })
}

/// One page of a `BTreeMap`-ordered listing: the records `keep` admits, plus the token
/// the next page resumes from — the EMPTY string once the listing is exhausted.
///
/// Every listing of this service documents its output token as a string that is empty
/// when there are no additional items to return, so the token is a VALUE on every page
/// and never a missing member: a caller decides whether to ask for another page by
/// whether the token it was handed is empty, not by whether the member is there. See
/// [`resume_token`] for the other half of that contract on the way in.
///
/// `kind` names the listing the token belongs to and `region` the endpoint serving
/// this page, so a token one listing issued is not accepted by the other and a token
/// one Region's endpoint issued is not accepted by another's (see
/// [`encode_page_token`]).
///
/// Both bounds here are the server's own: `page_size` (see [`page_size`]) sizes the
/// page, so nothing a caller sends sizes an allocation, and [`MAX_SCAN_ENTRIES`]
/// bounds how many stored records ONE request examines — a narrow filter must not walk
/// the whole store under the store lock. Either way the token names where to resume,
/// so paging still visits every record exactly once.
pub fn paginate<'a, T>(
    kind: &str,
    region: &str,
    entries: impl Iterator<Item = (&'a String, &'a T)>,
    page_size: usize,
    keep: impl Fn(&T) -> bool,
) -> (Vec<&'a T>, String) {
    let mut page = Vec::with_capacity(page_size);
    let mut next_token = String::new();
    for (examined, (key, record)) in entries.enumerate() {
        // The first record that does not fit becomes the next page's start, and only
        // then does the token name a record — an exhausted listing reports the empty
        // string it started as.
        if page.len() == page_size || examined == MAX_SCAN_ENTRIES {
            next_token = encode_page_token(kind, region, key);
            break;
        }
        if keep(record) {
            page.push(record);
        }
    }
    (page, next_token)
}

/// The sentence this service refuses an invalid `StartingToken` with.
///
/// Wholly fixed text: the refusal reports neither the token nor the listing it was sent
/// to, so there is nothing in it derived from the request or from this environment. The
/// one place it lives, so every paginated listing of this service refuses a token it did
/// not issue with the same sentence.
pub const INVALID_STARTING_TOKEN_MESSAGE: &str = "Provided starting token is not valid";

/// The record a request means to resume at, or `None` when it names none: an absent
/// `StartingToken`, or the empty one.
///
/// The empty string is what an exhausted listing reports as its own token (see
/// [`paginate`]), so it says "there is no next page" rather than naming a record, and a
/// request that hands it straight back means the same as one that carries no token at
/// all. Every other token is still resolved through [`decode_page_token`], so text this
/// service never issued is refused rather than read as "start from the beginning".
pub fn resume_token(starting_token: Option<&str>) -> Option<&str> {
    starting_token.filter(|token| !token.is_empty())
}

/// The page size to serve for a requested `MaxItems`: clamped into
/// `1..=MAX_ITEMS_LIMIT` whatever the request asked for, and that limit when it asked
/// for nothing.
pub fn page_size(max_items: Option<i32>) -> usize {
    max_items.map_or(MAX_ITEMS_LIMIT, |requested| {
        requested.clamp(1, MAX_ITEMS_LIMIT)
    }) as usize
}

/// The listing a page token belongs to: `ListConfigurationManagers`.
pub const MANAGER_PAGE_TOKEN_KIND: &str = "configuration-manager";

/// The listing a page token belongs to: `ListConfigurations`.
pub const CONFIGURATION_PAGE_TOKEN_KIND: &str = "configuration";

/// A page token naming the record the NEXT page starts at, tagged with the listing
/// that issued it and with the Region whose endpoint issued it.
///
/// A listing resumes a walk of records scoped to ONE listing and ONE Region, so a
/// token is only meaningful to the endpoint and the listing that minted it: both are
/// part of the token, which is what lets [`decode_page_token`] refuse a token issued
/// elsewhere instead of resuming this Region's walk at a record belonging to another
/// one and skipping everything that sorts before it. Every Region's records share one
/// store key order ([`ComAmazonawsSsmquicksetupQuicksetupState::managers`]), so a
/// foreign token's key is a position in that shared order rather than in the
/// requesting Region's own.
///
/// Hex-encoded rather than handed back in the clear, which makes the token opaque to
/// a caller rather than secret: it carries nothing but the listing's name, the
/// Region's name and a store key. Hex is inside both `StartingToken` patterns
/// (`^[A-Za-z0-9+=@_\/\s-]*$` and the `ListConfigurations` variant) and well inside
/// their 1024-character bound — a tag, a Region and an ARN encode to about 270
/// characters.
pub fn encode_page_token(kind: &str, region: &str, key: &str) -> String {
    ::hex::encode(format!("{kind}\u{1f}{region}\u{1f}{key}"))
}

/// The store key a page token names, or `None` when the token is not one this
/// listing (`kind`) issued through the endpoint of `region` — a token of the other
/// listing, a token of another Region's endpoint, or caller-crafted text.
///
/// `region` carries no `\u{1f}` ([`request_region`] admits only `[A-Za-z0-9-]`), so
/// the tagged prefix a token is accepted under names exactly one listing and one
/// Region.
pub fn decode_page_token(kind: &str, region: &str, token: &str) -> Option<String> {
    let decoded = ::hex::decode(token).ok()?;
    let decoded = String::from_utf8(decoded).ok()?;
    decoded
        .strip_prefix(&format!("{kind}\u{1f}{region}\u{1f}"))
        .map(str::to_string)
}

/// The ARN of a configuration manager with `uuid` in `region`.
pub fn manager_arn(region: &str, uuid: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("{MANAGER_ARN_RESOURCE_PREFIX}{uuid}"),
    )
}

/// The region this request was signed for, from its SigV4 credential scope; the
/// emulator's default region when it carries none.
///
/// The scope is caller-controlled text and the value lands inside an ARN whose
/// modelled pattern allows no `:` in that field and whose length the store keys on,
/// so anything outside `[A-Za-z0-9-]` is refused here and the default used instead: a
/// generated ARN can never carry text the model does not admit.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .filter(|region| {
            !region.is_empty()
                && region.len() <= 64
                && region
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        })
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// A `BTreeMap` as the wire map shapes want it. The one place this conversion lives.
pub fn to_hash_map(map: &BTreeMap<String, String>) -> HashMap<String, String> {
    map.iter()
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect()
}

/// Now, in nanoseconds since the Unix epoch — only a uuid-generator seed.
fn now_nanos() -> u64 {
    SystemTime::now()
        .duration_since(::std::time::UNIX_EPOCH)
        .map_or(0, |since_epoch| since_epoch.as_nanos() as u64)
}

/// SplitMix64: the uuid generator's mixing step, so consecutive seeds do not produce
/// near-identical ids.
fn splitmix64(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// A wire map as the store holds it: ordered, so two reads of one record report its
/// entries in the same order.
pub fn to_btree_map(map: Option<HashMap<String, String>>) -> BTreeMap<String, String> {
    map.unwrap_or_default().into_iter().collect()
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsSsmquicksetupQuicksetupState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsSsmquicksetupQuicksetupState>)
}
