// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-imagebuilder-imagebuilder service.
//!
//! The first resource this store holds is the **infrastructure configuration**: the
//! environment Image Builder builds and tests an image in. Every configuration is
//! stored exactly once, keyed by its own ARN, so `CreateInfrastructureConfiguration`
//! writes the record that `GetInfrastructureConfiguration` reads back and
//! `UpdateInfrastructureConfiguration` REPLACES that record under the same key
//! ([`ComAmazonawsImagebuilderImagebuilderState::replace_infrastructure_configuration`]).
//!
//! Two facts about that resource shape the store:
//!
//! * The ARN is derived from the NAME, not minted independently — the modelled
//!   `name` member documents "Image Builder generates the infrastructure
//!   configuration ARN from a normalized form of the name, so names that differ only
//!   in case, spaces, or underscores count as the same name", and
//!   "Infrastructure configuration names must be unique to your account in each
//!   Amazon Web Services Region". [`normalized_name`] is that one normalization, so
//!   the uniqueness test and the ARN agree by construction.
//! * The service is REGIONAL (its endpoints are `imagebuilder.<region>.<dns-suffix>`),
//!   so a configuration belongs to the Region its ARN names and a regional endpoint
//!   reaches only the configurations of its own Region
//!   ([`ComAmazonawsImagebuilderImagebuilderState::infrastructure_configuration`]).
//!
//! A **distribution configuration** is stored the same way, keyed by its own ARN, and for
//! the same two reasons: `CreateDistributionConfiguration`'s `name` documents "Distribution
//! configuration names must be unique to your account in each Amazon Web Services Region.
//! Image Builder generates the distribution configuration ARN from a normalized form of the
//! name", so [`normalized_name`] serves both the ARN and the uniqueness test, and the
//! configuration belongs to the Region its ARN names
//! ([`ComAmazonawsImagebuilderImagebuilderState::distribution_configuration`]). What it
//! holds is the per-Region settings a create carried, in that order, which is what a read
//! reports back ([`DistributionConfigurationRecord::view`]).
//!
//! Beside it the store holds the two authoring resources an image build starts from.
//! A **component** is the YAML document Image Builder runs on a build or test instance,
//! and an **image recipe** names the parent image a build starts from together with the
//! components it applies. Three facts about them shape how they are stored:
//!
//! * a component is versioned twice over. `CreateComponent`'s `name` member documents
//!   "If a component with the same name and semantic version already exists in your
//!   account in the same Amazon Web Services Region, the request creates a new build
//!   version for it", so a record is keyed by its BUILD version ARN
//!   (`component/<name>/<version>/<build>`) and nothing is ever overwritten.
//! * a reference to a component may name a build version, or a version whose trailing
//!   nodes are `x` wildcards — "Version ARNs resolve to the latest available matching
//!   component build version" (`GetComponent`'s `componentBuildVersionArn`). That one
//!   resolution ([`ComAmazonawsImagebuilderImagebuilderState::resolve_component`])
//!   serves the read, the recipe's component list and the delete's dependency check.
//! * an image recipe is keyed by its own ARN, which carries the [`normalized_name`] and
//!   the semantic version, so one name+version pair is stored at most once.
//!
//! Beside those the store holds the **image build versions** a build produced, keyed by
//! their own build version ARNs, each carrying what its build RECORDED — today the
//! package inventory `ListImagePackages` reports
//! ([`ImageBuildVersionRecord`]). A build version nothing has built is absent, and a read
//! of an absent one reports no packages rather than refusing: the inventory of a build
//! that never ran is empty, not missing.
//!
//! An **image pipeline** is what starts those builds. It references the recipe and the
//! infrastructure configuration a build uses and holds the settings that control the
//! build, and it is stored keyed by its own ARN, which carries the [`normalized_name`]:
//! "Pipeline names must be unique to your account in each Amazon Web Services Region.
//! Image Builder generates the pipeline ARN from a normalized form of the name"
//! (`CreateImagePipeline`'s `name`). Two of the members a pipeline read reports are NOT
//! held on the record: the date of its last run and the status of that run are read from
//! the newest image build version that names the pipeline as its source
//! ([`ComAmazonawsImagebuilderImagebuilderState::image_pipeline_view`]), so what a read
//! reports about a pipeline's runs cannot drift from the runs the store actually holds.
//!
//! A build is not instant: `StartImagePipelineExecution` "returns as soon as Image
//! Builder creates the new image resource and queues the build", so a build version is
//! filed carrying the instant its build is due to finish, and the status it reports is
//! read from the CLOCK against that instant ([`ImageBuildVersionRecord::status_at`]) —
//! nothing has to be polled for a queued build to become available.
//!
//! A **resource policy** is held beside the resource it is applied to, keyed by that
//! resource's own ARN
//! ([`ComAmazonawsImagebuilderImagebuilderState::resource_policies`]): every policy pair
//! in this model names its resource by that ARN and carries nothing but the document, a
//! write REPLACES the document the matching read then reports, and a resource no
//! policy was ever applied to has no entry at all — which a read reports as
//! [`UNSET_RESOURCE_POLICY`] rather than as a missing resource. What is stored is this
//! store's own rendering of the submitted document
//! ([`ComAmazonawsImagebuilderImagebuilderState::put_resource_policy`]), so a read reports
//! the policy as this service holds it.
//!
//! `clientToken` is an idempotency token: a repeat of a request that already
//! succeeded answers with the ORIGINAL response, and the same token carrying
//! different creation parameters is a mismatch. Both need the first request's
//! parameters and its answer, so every successful create files a [`CreateReceipt`]
//! beside the record, under the operation that earned it.

use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime};

/// Where a component document named by `uri` is read from: the object the emulator's own
/// Amazon S3 holds at that address.
mod component_document_object;
pub use component_document_object::*;

/// The component document an IMPORTED script is wrapped in, and how an import's own
/// receipt is filed.
mod imported_component;
pub use imported_component::*;

/// Whether the instance profile a configuration names is one this account's IAM holds.
mod instance_profile;
pub use instance_profile::*;

/// How a workflow step that has paused for a caller to respond is FILED in this store.
mod waiting_workflow_steps;
pub use waiting_workflow_steps::*;

/// The rules both kinds of recipe this service holds follow.
mod recipes;
pub use recipes::*;

/// The container recipes this store holds, and the rules a container recipe follows on its
/// own.
mod container_recipe;
pub use container_recipe::*;

/// The refusal classes this service answers that its own model does not declare.
mod recorded_refusals;
pub use recorded_refusals::*;

/// The workflow resources this store holds, and the versioned reference that reaches one.
mod workflows;
pub use workflows::*;

/// How many workflow references ONE REQUEST resolves, and what they established.
mod workflow;
pub use workflow::*;

/// What a pipeline listing asks for, and which member one of its filters selects on.
mod image_pipeline_listing;
pub use image_pipeline_listing::*;

/// How an UPDATE of an infrastructure configuration replaces the stored record.
mod infrastructure_configuration_update;
pub use infrastructure_configuration_update::*;

use crate::ops::types::{
    AdditionalInstanceConfiguration, AmiDistributionConfiguration, Component,
    ComponentConfiguration, ComponentParameterDetail,
    ComponentState, ComponentSummary, ComponentVersion, CreateComponentInput,
    CreateDistributionConfigurationInput, CreateImageInput,
    CreateImagePipelineInput, CreateImageRecipeInput,
    CreateInfrastructureConfigurationInput, Distribution, DistributionConfiguration, Filter,
    FastLaunchConfiguration, ImageLoggingConfiguration, ImagePipeline,
    ImageRecipe, ImageRecipeSummary,
    ImageScanningConfiguration, ImageState, ImageSummary, ImageTestsConfiguration,
    InfrastructureConfiguration, InfrastructureConfigurationSummary, InstanceBlockDeviceMapping,
    InstanceMetadataOptions,
    LatestVersionReferences, Logging, ParentInstanceConfiguration, PipelineLoggingConfiguration,
    Placement, Schedule, StartImagePipelineExecutionInput, WorkflowConfiguration,
    WorkflowStepExecution,
};

/// This service's ARN namespace and SigV4 signing name (`arnNamespace` / `sigv4Name`
/// of `smithy_query(query="service_info")`).
pub const ARN_SERVICE: &str = "imagebuilder";

/// The region used when a request carries no SigV4 credential scope to read one from;
/// the emulator's documented default region.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The resource portion an infrastructure-configuration ARN carries before its name,
/// from the modelled `InfrastructureConfigurationArn` pattern
/// `^arn:aws[^:]*:imagebuilder:[^:]+:(?:[0-9]{12}|aws):infrastructure-configuration/[a-z0-9-_]+$`.
pub const INFRASTRUCTURE_CONFIGURATION_ARN_PREFIX: &str = "infrastructure-configuration/";

/// The resource portion a distribution-configuration ARN carries before its name, from
/// the modelled `DistributionConfigurationArn` pattern
/// `^arn:aws[^:]*:imagebuilder:[^:]+:(?:[0-9]{12}|aws):distribution-configuration/[a-z0-9-_]+$`.
pub const DISTRIBUTION_CONFIGURATION_ARN_PREFIX: &str = "distribution-configuration/";

/// The resource portion a component ARN carries before its name, from the modelled
/// `ComponentBuildVersionArn` pattern
/// `^arn:aws[^:]*:imagebuilder:[^:]+:(?:[0-9]{12}|aws(?:-[a-z-]+)?|third-party):component/[a-z0-9-_]+/[0-9]+\.[0-9]+\.[0-9]+/[0-9]+$`.
pub const COMPONENT_ARN_PREFIX: &str = "component/";

/// The resource portion an image-recipe ARN carries before its name, from the modelled
/// `ImageRecipeArn` pattern
/// `^arn:aws[^:]*:imagebuilder:[^:]+:(?:[0-9]{12}|aws):image-recipe/[a-z0-9-_]+/(?:[0-9]+|x)\.(?:[0-9]+|x)\.(?:[0-9]+|x)$`.
pub const IMAGE_RECIPE_ARN_PREFIX: &str = "image-recipe/";

/// The resource portion an image build version ARN carries before its name, from the
/// modelled `ImageBuildVersionArn` pattern
/// `^arn:aws[^:]*:imagebuilder:[^:]+:(?:[0-9]{12}|aws(?:-[a-z-]+)?):image/[a-z0-9-_]+/[0-9]+\.[0-9]+\.[0-9]+/[0-9]+$`.
pub const IMAGE_ARN_PREFIX: &str = "image/";

/// The resource portion an image-pipeline ARN carries before its name, from the modelled
/// `ImagePipelineArn` pattern
/// `^arn:aws[^:]*:imagebuilder:[^:]+:(?:[0-9]{12}|aws):image-pipeline/[a-z0-9-_]+$`.
pub const IMAGE_PIPELINE_ARN_PREFIX: &str = "image-pipeline/";

/// The operations whose `clientToken` receipts share one index, each named as the key
/// that index files its receipts under — the operation's own name, so two operations
/// never read each other's.
pub const CREATE_INFRASTRUCTURE_CONFIGURATION_OPERATION: &str =
    "CreateInfrastructureConfiguration";

/// See [`CREATE_INFRASTRUCTURE_CONFIGURATION_OPERATION`]. An update carries a
/// `clientToken` of its own, so its receipts are filed under its own name and never read
/// the create's: the same token may create a configuration and then update it.
pub const UPDATE_INFRASTRUCTURE_CONFIGURATION_OPERATION: &str =
    "UpdateInfrastructureConfiguration";

/// See [`CREATE_INFRASTRUCTURE_CONFIGURATION_OPERATION`].
pub const CREATE_DISTRIBUTION_CONFIGURATION_OPERATION: &str = "CreateDistributionConfiguration";

/// See [`CREATE_INFRASTRUCTURE_CONFIGURATION_OPERATION`].
pub const CREATE_COMPONENT_OPERATION: &str = "CreateComponent";

/// See [`CREATE_INFRASTRUCTURE_CONFIGURATION_OPERATION`].
pub const CREATE_IMAGE_RECIPE_OPERATION: &str = "CreateImageRecipe";

/// See [`CREATE_INFRASTRUCTURE_CONFIGURATION_OPERATION`].
pub const CREATE_IMAGE_PIPELINE_OPERATION: &str = "CreateImagePipeline";

/// See [`CREATE_INFRASTRUCTURE_CONFIGURATION_OPERATION`]. A pipeline execution carries a
/// `clientToken` of its own and mints an image build version, so its receipts are filed
/// under its own name like any other create's.
pub const START_IMAGE_PIPELINE_EXECUTION_OPERATION: &str = "StartImagePipelineExecution";

/// See [`CREATE_INFRASTRUCTURE_CONFIGURATION_OPERATION`]. An image build a caller asks
/// for by hand carries a `clientToken` of its own, so its receipts are filed under its
/// own name and never read a pipeline execution's.
pub const CREATE_IMAGE_OPERATION: &str = "CreateImage";

/// The listing a page token belongs to, so a token issued by one list operation cannot
/// be spent on another ([`encode_page_token`]).
pub const LIST_IMAGE_RECIPES_OPERATION: &str = "ListImageRecipes";

/// See [`LIST_IMAGE_RECIPES_OPERATION`].
pub const LIST_IMAGE_PACKAGES_OPERATION: &str = "ListImagePackages";

/// See [`LIST_IMAGE_RECIPES_OPERATION`].
pub const LIST_IMAGE_BUILD_VERSIONS_OPERATION: &str = "ListImageBuildVersions";

/// See [`LIST_IMAGE_RECIPES_OPERATION`].
pub const LIST_COMPONENTS_OPERATION: &str = "ListComponents";

/// See [`LIST_IMAGE_RECIPES_OPERATION`].
pub const LIST_COMPONENT_BUILD_VERSIONS_OPERATION: &str = "ListComponentBuildVersions";

/// See [`LIST_IMAGE_RECIPES_OPERATION`].
pub const LIST_INFRASTRUCTURE_CONFIGURATIONS_OPERATION: &str = "ListInfrastructureConfigurations";

/// See [`LIST_IMAGE_RECIPES_OPERATION`].
pub const LIST_WAITING_WORKFLOW_STEPS_OPERATION: &str = "ListWaitingWorkflowSteps";

/// How many nodes of a semantic version a caller assigns.
///
/// From `semanticVersion`'s own documentation, which `CreateComponent` and
/// `CreateImageRecipe` repeat word for word: "The semantic version has four nodes:
/// <major>.<minor>.<patch>/<build>. You can assign values for the first three", and
/// "Image Builder automatically assigns the build number to the fourth node".
pub const ASSIGNED_VERSION_NODES: usize = 3;

/// The largest value an assigned semantic-version node may carry: "The upper limit is
/// 2^30-1, or 1073741823, for each node" (`semanticVersion`).
pub const MAX_VERSION_NODE: u64 = 1_073_741_823;

/// The build number the FIRST build version of one component name+version pair carries;
/// Image Builder counts the fourth node from one.
pub const FIRST_BUILD_VERSION: u64 = 1;

/// The status a stored component reports, from the modelled `ComponentStatus` enum
/// (`ACTIVE`, `DEPRECATED`, `DISABLED`): a component this service just stored is the one
/// a new recipe may include, which is what `ACTIVE` means — "Components with a status of
/// DEPRECATED or DISABLED can't be added to new recipes".
pub const COMPONENT_STATUS_ACTIVE: &str = "ACTIVE";

/// The component type of a document that is anything but a lone `test` phase, from
/// `CreateComponent`'s own documentation: "Image Builder determines the component type
/// from the document. If the document contains a single phase named test, the component
/// type is TEST. Otherwise, the component type is BUILD."
pub const COMPONENT_TYPE_BUILD: &str = "BUILD";

/// The component type of a document that is a single phase named `test` — the other
/// value of the modelled `ComponentType` enum. See [`COMPONENT_TYPE_BUILD`].
pub const COMPONENT_TYPE_TEST: &str = "TEST";

/// Whether a stored component's data is encrypted at rest, from the modelled `encrypted`
/// member: "Image Builder encrypts all component data at rest, so this value is always
/// true. This field is retained for backward compatibility."
pub const COMPONENT_DATA_ENCRYPTED: bool = true;

/// The platform of a parent image whose reference names Windows, from the modelled
/// `Platform` enum (`Windows | Linux | macOS`).
pub const PLATFORM_WINDOWS: &str = "Windows";

/// The platform of a parent image whose reference names macOS, from the modelled
/// `Platform` enum (`Windows | Linux | macOS`).
pub const PLATFORM_MACOS: &str = "macOS";

/// The platform of every other parent image: the remaining value of the modelled
/// `Platform` enum (`Windows | Linux | macOS`).
pub const PLATFORM_LINUX: &str = "Linux";

/// The version node that matches any assigned value, from `GetComponent`'s
/// `componentBuildVersionArn`: "The version can use the `x` wildcard in trailing
/// positions".
pub const WILDCARD_VERSION_NODE: &str = "x";

/// The version a NAME ARN carries: three [`WILDCARD_VERSION_NODE`] nodes.
///
/// From the note on `ComponentVersion`'s `arn`: "Versionless ARNs and Name ARNs do not
/// include specific values in any of the nodes. The nodes are either left off entirely, or
/// they are specified as wildcards, for example: x.x.x", beside "Version ARNs have only
/// the first three nodes" — so a listing entry that aggregates every version of one name
/// is reported under this version, and an entry that stands for one version under its own
/// three assigned nodes.
pub const WILDCARD_VERSION: &str = "x.x.x";

/// The ownership a caller's own resources carry, from the modelled `Ownership` enum.
/// `ListImageRecipes`' `owner` documents "By default, this request will only show image
/// recipes owned by your account", so this is also the value an omitted `owner` means.
pub const OWNERSHIP_SELF: &str = "Self";

/// The owner a resource published through the Amazon Web Services Marketplace reports,
/// from the modelled `Ownership` enum and `owner`'s own documentation: "The value is your
/// account ID for components that you own, the sharing account's ID for shared components,
/// or `Amazon`, `ThirdParty`, or `AWSMarketplace`."
pub const OWNERSHIP_AWS_MARKETPLACE: &str = "AWSMarketplace";

/// The filter names `ListImageRecipes` accepts, from the `filters` member's own list:
/// "Use the following filters to streamline results: name, parentImage, platform."
pub const IMAGE_RECIPE_FILTER_NAMES: &[&str] = &["name", "parentImage", "platform"];

/// The most items one `ListImageRecipes` page carries when the request names no
/// `maxResults`: the modelled `maxResults` range's own maximum (1..25).
pub const MAX_IMAGE_RECIPES_PER_PAGE: usize = 25;

/// The filter names `ListInfrastructureConfigurations` accepts, from the `filters`
/// member's own documentation: "You can filter on name to streamline results."
pub const INFRASTRUCTURE_CONFIGURATION_FILTER_NAMES: &[&str] = &["name"];

/// The most items one `ListInfrastructureConfigurations` page carries when the request
/// names no `maxResults`: the modelled `maxResults` range's own maximum (1..25).
pub const MAX_INFRASTRUCTURE_CONFIGURATIONS_PER_PAGE: usize = 25;

/// The most items one `ListWaitingWorkflowSteps` page carries when the request names no
/// `maxResults`: the modelled `maxResults` range's own maximum (1..25).
pub const MAX_WAITING_WORKFLOW_STEPS_PER_PAGE: usize = 25;

/// The most items one `ListImagePackages` page carries when the request names no
/// `maxResults`: the modelled `maxResults` range's own maximum (1..25).
pub const MAX_IMAGE_PACKAGES_PER_PAGE: usize = 25;

/// The most items one `ListImageBuildVersions` page carries when the request names no
/// `maxResults`: the modelled `maxResults` range's own maximum (1..25).
pub const MAX_IMAGE_BUILD_VERSIONS_PER_PAGE: usize = 25;

/// The filter names `ListImageBuildVersions` accepts, from the `filters` member's own
/// list: "Use the following filters to streamline results: name, osVersion, platform,
/// type, version."
pub const IMAGE_BUILD_VERSION_FILTER_NAMES: &[&str] =
    &["name", "osVersion", "platform", "type", "version"];

/// The filter names `ListComponents` accepts, from the `filters` member's own list:
/// "Use the following filters to streamline results: description, name, platform,
/// productCodes, status, supportedOsVersion, type, version."
///
/// Held in the order a refusal STATES them, which is the order AWS's own recorded
/// unsupported-filter refusal of this operation lists them in rather than the alphabetical
/// order the member's documentation happens to use; membership is what every other reader
/// asks of this set, and order does not change it.
pub const COMPONENT_FILTER_NAMES: &[&str] = &[
    "name",
    "version",
    "description",
    "platform",
    "type",
    "supportedOsVersion",
    "productCodes",
    "status",
];

/// The one filter a `ListComponents` request may not carry beside `byName`: "You can't
/// combine this option with the version filter" (the modelled `byName` member).
pub const COMPONENT_VERSION_FILTER_NAME: &str = "version";

/// The most items one `ListComponents` page carries when the request names no
/// `maxResults`: the modelled `maxResults` range's own maximum (1..25).
pub const MAX_COMPONENTS_PER_PAGE: usize = 25;

/// The most tags one resource carries.
///
/// The modelled `TagMap`'s own length bound (`len 1..50`) — and `TagMap` is the target of
/// every `tags` member, the one a resource reports as much as the one a request submits, so
/// it bounds the map a resource ends up holding and not only the map one call carries.
pub const MAX_RESOURCE_TAGS: usize = 50;

/// The most components one image recipe includes.
///
/// The documented quota, from the modelled `components` member of `ImageRecipe`: "A recipe
/// can contain a maximum of 20 build and test components in any combination, by default.
/// This maximum is an adjustable quota." The modelled list carries no maximum of its own,
/// so the quota is read here — which is also what bounds the work ONE create spends
/// resolving the references it names.
pub const MAX_RECIPE_COMPONENTS: usize = 20;

// ---------------------------------------------------------------------------
// Pre-flight bounds on a caller-supplied component document
//
// `data` is caller-supplied text handed to a RECURSIVE YAML loader, so how deep that
// loader may descend has to be refused BEFORE it is called: an `EventReceiver` cannot
// abort `load` (`on_event` returns `()`), and exhausting the stack aborts the process
// rather than unwinding. The modelled `len 1..16000` bounds how BIG a document is, not how
// deeply it nests, so each measure below is a bound on NESTING and a large flat document
// trips none of them.
// ---------------------------------------------------------------------------

/// The most flow collections (`[`, `{`) a component document may have OPEN at once,
/// closers subtracted: the nesting a well-formed document reaches. Chosen: far past the
/// nesting of a document's `phases` → `steps` → `inputs` tree.
pub const MAX_DOCUMENT_FLOW_DEPTH: usize = 64;

/// The most flow openers (`[`, `{`) a component document may carry in total.
///
/// The backstop for [`MAX_DOCUMENT_FLOW_DEPTH`]: this is a byte scan, so a `]` inside a
/// quoted scalar lowers the running depth while the loader's own depth keeps climbing —
/// but no document can nest deeper than its opener COUNT. Chosen as a depth the loader
/// survives with room to spare, and far above the flow collections a real document carries
/// in total: the modelled `data` maximum is 16,000 bytes, and a document carrying this
/// many flow collections inside that is almost all punctuation.
pub const MAX_DOCUMENT_FLOW_OPENERS: usize = 2_048;

/// The most leading spaces one line of a component document may carry, which is what block
/// nesting costs per level. Chosen: far past the indentation of a document written at any
/// sane indent step.
pub const MAX_DOCUMENT_INDENT: usize = 256;

/// The most compact block indicators (`- `, `? `) one line of a component document may
/// carry, which is where block nesting (`- - - x`) descends without costing a column. A
/// hyphen inside a name, a command or a description is not one of these (no space follows
/// it). Chosen.
pub const MAX_DOCUMENT_COMPACT_PER_LINE: usize = 256;

/// Applied when a create or an update of an infrastructure configuration omits
/// `terminateInstanceOnFailure`, whose own documentation says "Defaults to true".
///
/// This is the ONE flag of the pair the service itself writes into the stored
/// configuration: `terminateInstanceOnCompletion` carries no stored value until a request
/// supplies one, so a read of a configuration no request set it on reports nothing for it
/// (see [`InfrastructureConfigurationRecord::terminate_instance_on_completion`]).
pub const DEFAULT_TERMINATE_INSTANCE_ON_FAILURE: bool = true;

/// The status a stored pipeline reports when the create named none, from the modelled
/// `status` member: "The status of the image pipeline. If you don't specify a status, it
/// defaults to ENABLED" — one of the two values of the modelled `PipelineStatus` enum.
pub const PIPELINE_STATUS_ENABLED: &str = "ENABLED";

/// Applied when `CreateImagePipeline` omits `enhancedImageMetadataEnabled`, whose own
/// documentation says "Defaults to true".
pub const DEFAULT_ENHANCED_IMAGE_METADATA_ENABLED: bool = true;

/// Applied when a create omits `imageTestsConfiguration` altogether, or omits its
/// `imageTestsEnabled`: "If you don't provide test settings, Image Builder stores a
/// default configuration with image tests enabled" (`CreateImagePipeline`'s
/// `imageTestsConfiguration`), and `imageTestsEnabled` itself "Defaults to true".
pub const DEFAULT_IMAGE_TESTS_ENABLED: bool = true;

/// Applied when a stored test configuration carries no `timeoutMinutes`, whose own
/// documentation says "If you don't specify a value, Image Builder stores and returns
/// 720".
pub const DEFAULT_IMAGE_TESTS_TIMEOUT_MINUTES: i32 = 720;

/// The status an image build version reports while the build it was filed for is still
/// outstanding, from the modelled `ImageStatus` enum.
///
/// `StartImagePipelineExecution` "returns as soon as Image Builder creates the new image
/// resource and queues the build", so the image a start answers with exists and its build
/// is QUEUED — which is what `PENDING` says.
pub const IMAGE_STATUS_PENDING: &str = "PENDING";

/// The status an image build version reports once its build is no longer outstanding: the
/// end state the modelled `ImageState`'s own documentation names for a build that ran
/// ("A new image moves through build, test, and distribution statuses during creation,
/// and ends in the AVAILABLE, FAILED, or CANCELLED state").
pub const IMAGE_STATUS_AVAILABLE: &str = "AVAILABLE";

/// How a build a caller asked for by hand was initiated, from the modelled `BuildType`
/// enum, whose own documentation names `USER_INITIATED` for a manual build request, as
/// against the `SCHEDULED` build that only a pipeline's own schedule starts.
pub const BUILD_TYPE_USER_INITIATED: &str = "USER_INITIATED";

/// The image type a build from an IMAGE recipe produces, from the modelled `type` member
/// of `ImageRecipe`: "For an image recipe, this is always AMI."
pub const IMAGE_TYPE_AMI: &str = "AMI";

/// The image type a build from a CONTAINER recipe produces, the other value of the
/// modelled `ImageType` enum. See [`IMAGE_TYPE_AMI`].
pub const IMAGE_TYPE_DOCKER: &str = "DOCKER";

/// How long after the call that queued it a build is due to finish, and so how long the
/// image it creates reports [`IMAGE_STATUS_PENDING`] before it reports
/// [`IMAGE_STATUS_AVAILABLE`].
///
/// CHOSEN, not documented: the model says a new image is queued and reaches its end state
/// later, but names no duration — a real build takes many minutes. It is deliberately not
/// zero, so a caller that reads straight after the call still observes the queued state,
/// and short enough that a caller waiting for the image does not wait long.
pub const IMAGE_BUILD_SPAN: Duration = Duration::from_secs(3);

/// The metadata-tag keys a caller may not assign to the build and test instances.
///
/// From the `resourceTags` member's own documentation: "Tag keys can't begin with
/// `aws:` or match one of the following reserved keys: CreatedBy, Ec2ImageBuilderArn,
/// Name, or Tags." The `aws:` prefix is already refused by the modelled `TagKey`
/// pattern before a handler runs; these four are not, so they are refused here.
pub const RESERVED_RESOURCE_TAG_KEYS: &[&str] =
    &["CreatedBy", "Ec2ImageBuilderArn", "Name", "Tags"];

/// The most infrastructure configurations this emulator will hold.
///
/// An emulator resource guard, NOT an AWS quota: Image Builder publishes a per-account
/// limit but the model carries no value for it, so this cap is set well above any
/// realistic use and exists only so a caller-driven store cannot grow without bound.
pub const MAX_INFRASTRUCTURE_CONFIGURATIONS: usize = 1_000;

/// The most distribution configurations this emulator will hold. An emulator resource
/// guard, for the same reason as [`MAX_INFRASTRUCTURE_CONFIGURATIONS`].
pub const MAX_DISTRIBUTION_CONFIGURATIONS: usize = 1_000;

/// The value a read reports for a distribution configuration's `timeoutMinutes`.
///
/// The member is REQUIRED on the modelled `DistributionConfiguration` yet is not a
/// creation parameter: "A property that Image Builder doesn't use. You can't set this
/// property when you create or update a distribution configuration, and it has no effect
/// on distribution behavior." So a read reports one value for every configuration, and it
/// is the maximum of the member's own modelled range
/// (`smithy_query(query="shape",shape="DistributionTimeoutMinutes")`: 30..720) — the value
/// a property that bounds nothing carries.
pub const DISTRIBUTION_TIMEOUT_MINUTES: i32 = 720;

/// The most components this emulator will hold, counting every build version.
///
/// An emulator resource guard like [`MAX_INFRASTRUCTURE_CONFIGURATIONS`], not an AWS
/// quota: the model carries no value for Image Builder's published per-account limit, so
/// this cap sits well above any realistic use and exists only so a caller-driven store
/// cannot grow without bound.
pub const MAX_COMPONENTS: usize = 1_000;

/// The most image recipes this emulator will hold. An emulator resource guard, for the
/// same reason as [`MAX_COMPONENTS`].
pub const MAX_IMAGE_RECIPES: usize = 1_000;

/// The most image pipelines this emulator will hold. An emulator resource guard, for the
/// same reason as [`MAX_COMPONENTS`].
pub const MAX_IMAGE_PIPELINES: usize = 1_000;

/// The most image build versions this emulator will hold. An emulator resource guard, for
/// the same reason as [`MAX_COMPONENTS`]:
/// [`ComAmazonawsImagebuilderImagebuilderState::image_store_is_full`] is what a build
/// checks before it files one.
pub const MAX_IMAGES: usize = 1_000;

/// The most packages this emulator keeps in ONE build version's inventory.
///
/// An emulator resource guard like [`MAX_IMAGES`], and a bound on one RECORD rather than on
/// the store: the modelled `ImagePackageList` carries no maximum, and the inventory of a
/// built image runs to a few thousand packages, so this cap sits above that and exists only
/// so one record cannot grow without bound
/// ([`ComAmazonawsImagebuilderImagebuilderState::insert_image`]).
pub const MAX_IMAGE_PACKAGES: usize = 5_000;

/// The policy document a resource-policy read reports for a resource no policy has been
/// applied to.
///
/// A resource carries a policy only once a `Put*Policy` call applies one, and the absence
/// is not an error: the read answers the empty JSON document. The modelled
/// `ResourcePolicyDocument` has a minimum length of one
/// (`smithy_query(query="shape",shape="ResourcePolicyDocument")`), so an unset policy is
/// reported as this document rather than as empty text.
pub const UNSET_RESOURCE_POLICY: &str = "{}";

/// The modelled member name a policy document arrives in (`PutComponentPolicy`'s
/// `policy`), which is the member an invalid-value refusal names.
pub const POLICY_MEMBER: &str = "policy";

/// The most `clientToken` receipts this emulator keeps.
///
/// The receipt index's OWN bound, stated here rather than borrowed from the resource caps
/// above: a receipt is read only while the caller that earned it is still retrying, and
/// nothing else ties its lifetime to the resource it names — so where a delete removes a
/// resource without removing the receipt that named it, THIS is the constant that still
/// bounds the index. Past it the OLDEST receipt is evicted
/// ([`ComAmazonawsImagebuilderImagebuilderState::file_create_receipt`]), never the create.
pub const MAX_CREATE_RECEIPTS: usize =
    2 * (MAX_INFRASTRUCTURE_CONFIGURATIONS + MAX_COMPONENTS + MAX_IMAGE_RECIPES);

/// The most resource policies this emulator will hold.
///
/// A bound on an index filed into from the request path, like every other store of this
/// module, set to the TOTAL of every resource store's own cap: a policy belongs to one
/// stored resource, so an index holding one per resource this emulator will keep can hold
/// every policy there is to hold. See
/// [`ComAmazonawsImagebuilderImagebuilderState::put_resource_policy`].
pub const MAX_RESOURCE_POLICIES: usize = MAX_INFRASTRUCTURE_CONFIGURATIONS
    + MAX_DISTRIBUTION_CONFIGURATIONS
    + MAX_COMPONENTS
    + MAX_IMAGE_RECIPES
    + MAX_CONTAINER_RECIPES
    + MAX_IMAGE_PIPELINES
    + MAX_IMAGES
    + MAX_WORKFLOWS;

/// One stored infrastructure configuration: what a create wrote, which is what every
/// later read reports.
///
/// The list and map members are held as `Option`, so a read reports exactly what the
/// create stored — an omitted `instanceTypes` is reported as absent rather than as an
/// empty list.
#[derive(Debug, Clone)]
pub struct InfrastructureConfigurationRecord {
    /// This configuration's own ARN, which is also its key in the store.
    pub arn: String,
    /// The Region whose endpoint created it, and the only one that can reach it.
    pub region: String,
    /// The name as the caller submitted it; the ARN carries its [`normalized_name`].
    pub name: String,
    pub description: Option<String>,
    pub instance_types: Option<Vec<String>>,
    pub instance_profile_name: String,
    pub security_group_ids: Option<Vec<String>>,
    pub subnet_id: Option<String>,
    pub logging: Option<Logging>,
    pub key_pair: Option<String>,
    pub terminate_instance_on_failure: bool,
    /// Whether the build and test instances are terminated when image creation ends
    /// without a failure, held only once a request supplied it: the member is nullable and
    /// carries no modelled default, and the service writes no value of its own into the
    /// stored configuration, so a configuration no request set it on holds none and a read
    /// reports none.
    pub terminate_instance_on_completion: Option<bool>,
    pub sns_topic_arn: Option<String>,
    pub resource_tags: Option<BTreeMap<String, String>>,
    pub instance_metadata_options: Option<InstanceMetadataOptions>,
    pub tags: Option<BTreeMap<String, String>>,
    pub placement: Option<Placement>,
    pub parent_instance: Option<ParentInstanceConfiguration>,
    /// When it was created, as the modelled string `DateTime` carries an instant.
    pub date_created: String,
    /// When an update last replaced it — "The date on which the infrastructure
    /// configuration was last updated" — and so `None` until one has: a create is not an
    /// update, and a configuration nothing has updated has no last-updated date to report.
    pub date_updated: Option<String>,
}

impl InfrastructureConfigurationRecord {
    /// This record as the modelled `InfrastructureConfiguration` a read reports.
    #[must_use]
    pub fn view(&self) -> InfrastructureConfiguration {
        InfrastructureConfiguration {
            arn: Some(self.arn.clone()),
            name: Some(self.name.clone()),
            description: self.description.clone(),
            instance_types: self.instance_types.clone(),
            instance_profile_name: Some(self.instance_profile_name.clone()),
            security_group_ids: self.security_group_ids.clone(),
            subnet_id: self.subnet_id.clone(),
            logging: self.logging.clone(),
            key_pair: self.key_pair.clone(),
            terminate_instance_on_failure: Some(self.terminate_instance_on_failure),
            sns_topic_arn: self.sns_topic_arn.clone(),
            date_created: Some(self.date_created.clone()),
            date_updated: self.date_updated.clone(),
            resource_tags: self.resource_tags.clone().map(into_tag_map),
            instance_metadata_options: self.instance_metadata_options.clone(),
            tags: Some(reported_tags(self.tags.as_ref())),
            placement: self.placement.clone(),
            terminate_instance_on_completion: self.terminate_instance_on_completion,
            parent_instance: self.parent_instance.clone(),
        }
    }

    /// This record as the modelled `InfrastructureConfigurationSummary` one listed page
    /// entry carries: the members that shape names, read off the same stored record
    /// [`Self::view`] reports, so a listing and a read never disagree about one
    /// configuration.
    #[must_use]
    pub fn summary(&self) -> InfrastructureConfigurationSummary {
        InfrastructureConfigurationSummary {
            arn: Some(self.arn.clone()),
            name: Some(self.name.clone()),
            description: self.description.clone(),
            date_created: Some(self.date_created.clone()),
            date_updated: self.date_updated.clone(),
            resource_tags: self.resource_tags.clone().map(into_tag_map),
            tags: Some(reported_tags(self.tags.as_ref())),
            instance_types: self.instance_types.clone(),
            instance_profile_name: Some(self.instance_profile_name.clone()),
            placement: self.placement.clone(),
        }
    }

    /// The value of the member `filter_name` filters on, or `None` when this record
    /// reports none. See [`INFRASTRUCTURE_CONFIGURATION_FILTER_NAMES`].
    #[must_use]
    pub fn filterable(&self, filter_name: &str) -> Option<&str> {
        match filter_name {
            "name" => Some(self.name.as_str()),
            _ => None,
        }
    }

    /// Whether the filter `filter_name` carrying `values` admits this record: one of its
    /// values IS the value the record reports for the member the filter names — "Filter
    /// values are case-sensitive" (the modelled `values`).
    #[must_use]
    pub fn admits_filter(&self, filter_name: &str, values: &[String]) -> bool {
        self.filterable(filter_name)
            .is_some_and(|reported| values.iter().any(|value| value == reported))
    }
}

/// One stored distribution configuration: what a create wrote, which is what every later
/// read reports.
///
/// The ARN carries the [`normalized_name`] for the same reason an infrastructure
/// configuration's does — `CreateDistributionConfiguration`'s `name` member documents
/// "Distribution configuration names must be unique to your account in each Amazon Web
/// Services Region. Image Builder generates the distribution configuration ARN from a
/// normalized form of the name" — so the uniqueness test and the ARN agree by
/// construction.
#[derive(Debug, Clone)]
pub struct DistributionConfigurationRecord {
    /// This configuration's own ARN, which is also its key in the store.
    pub arn: String,
    /// The Region whose endpoint created it, and the only one that can reach it.
    pub region: String,
    /// The name as the caller submitted it; the ARN carries its [`normalized_name`].
    pub name: String,
    pub description: Option<String>,
    /// The per-Region distribution settings, in the order the request carried them: "Each
    /// entry defines how output images are distributed in one target Amazon Web Services
    /// Region."
    pub distributions: Vec<Distribution>,
    pub tags: Option<BTreeMap<String, String>>,
    /// When it was created, as the modelled string `DateTime` carries an instant.
    pub date_created: String,
    /// When an update last replaced it, and so `None` until one has — for the same reason
    /// an infrastructure configuration's is
    /// ([`InfrastructureConfigurationRecord::date_updated`]).
    pub date_updated: Option<String>,
}

impl DistributionConfigurationRecord {
    /// This record as the modelled `DistributionConfiguration` a read reports.
    #[must_use]
    pub fn view(&self) -> DistributionConfiguration {
        DistributionConfiguration {
            arn: Some(self.arn.clone()),
            name: Some(self.name.clone()),
            description: self.description.clone(),
            distributions: Some(self.distributions.clone()),
            timeout_minutes: DISTRIBUTION_TIMEOUT_MINUTES,
            date_created: Some(self.date_created.clone()),
            date_updated: self.date_updated.clone(),
            tags: Some(reported_tags(self.tags.as_ref())),
        }
    }
}

/// One stored component build version: what a create wrote, which is what every later
/// read reports.
#[derive(Debug, Clone)]
pub struct ComponentRecord {
    /// This build version's own ARN, which is also its key in the store.
    pub arn: String,
    /// The Region whose endpoint created it, and the only one that can reach it.
    pub region: String,
    /// The name as the caller submitted it; the ARN carries its [`normalized_name`].
    pub name: String,
    /// The three assigned version nodes, as the caller submitted them.
    pub version: String,
    /// The fourth node, which Image Builder assigns: which build of this name+version
    /// pair this is, counted from [`FIRST_BUILD_VERSION`].
    pub build_version: u64,
    pub description: Option<String>,
    pub change_description: Option<String>,
    /// `BUILD` or `TEST`, derived from the document's phases rather than submitted.
    pub component_type: String,
    pub platform: String,
    pub supported_os_versions: Option<Vec<String>>,
    /// The account that owns it — "The value is your account ID for components that you
    /// own" (the modelled `owner` member).
    pub owner: String,
    /// The document this component carries: the inline `data` the create carried, or the
    /// one-time snapshot fetched from the `uri` it named instead.
    pub data: Option<String>,
    /// The document URI the create carried instead of an inline document, when it carried
    /// one. The document itself was fetched from it at create time and lives in `data`.
    pub uri: Option<String>,
    pub kms_key_id: Option<String>,
    /// The parameters the document declared, derived from it at create time.
    pub parameters: Option<Vec<ComponentParameterDetail>>,
    /// When it was created, as the modelled string `DateTime` carries an instant.
    pub date_created: String,
    pub tags: Option<BTreeMap<String, String>>,
}

impl ComponentRecord {
    /// This record as the modelled `Component` a read reports.
    #[must_use]
    pub fn view(&self) -> Component {
        Component {
            arn: Some(self.arn.clone()),
            name: Some(self.name.clone()),
            version: Some(self.version.clone()),
            description: self.description.clone(),
            change_description: self.change_description.clone(),
            r#type: Some(self.component_type.clone()),
            platform: Some(self.platform.clone()),
            supported_os_versions: self.supported_os_versions.clone(),
            state: Some(ComponentState {
                status: Some(COMPONENT_STATUS_ACTIVE.to_string()),
                // `reason` "Describes how or why the component changed state"; this one
                // has never changed state, so there is nothing to describe.
                reason: None,
            }),
            parameters: self.parameters.clone(),
            owner: Some(self.owner.clone()),
            data: self.data.clone(),
            kms_key_id: self.kms_key_id.clone(),
            encrypted: Some(COMPONENT_DATA_ENCRYPTED),
            date_created: Some(self.date_created.clone()),
            tags: Some(reported_tags(self.tags.as_ref())),
            // "Contains the name of the publisher if this is a third-party component.
            // Otherwise, this property is empty" — a component this account created is
            // not third-party.
            publisher: None,
            // "Indicates whether component source is hidden from view" — the stored
            // document is reported, so it is not hidden.
            obfuscate: false,
            // "Contains product codes that are used for billing purposes for Amazon Web
            // Services Marketplace components" — this one came from a caller, not from
            // Marketplace.
            product_codes: None,
        }
    }

    /// Whether this component incurs Amazon Web Services Marketplace charges: the
    /// `billable` member a read of a component and every listing entry that stands for one
    /// of its versions reports.
    ///
    /// Only a component published through the Marketplace is metered — its charges are
    /// billed through the product codes such a component carries ("Contains product codes
    /// that are used for billing purposes for Amazon Web Services Marketplace components",
    /// the modelled `productCodes`), and the owner of one is
    /// [`OWNERSHIP_AWS_MARKETPLACE`]. So the value is read off the record's own owner, and
    /// a component this account created carries no charge of that kind.
    #[must_use]
    pub fn billable(&self) -> bool {
        self.owner == OWNERSHIP_AWS_MARKETPLACE
    }

    /// The four version nodes this build version sorts by: the three assigned ones and
    /// the build number, so "the latest available matching component build version" is
    /// the greatest of them.
    #[must_use]
    pub fn version_order(&self) -> [u64; ASSIGNED_VERSION_NODES + 1] {
        let [major, minor, patch] = parse_semantic_version(&self.version).unwrap_or_default();
        [major, minor, patch, self.build_version]
    }

    /// The version a LISTING entry this record stands for reports: its own three assigned
    /// nodes, or [`WILDCARD_VERSION`] when the entry aggregates every version of one name.
    #[must_use]
    pub fn listed_version(&self, aggregated_by_name: bool) -> &str {
        if aggregated_by_name {
            WILDCARD_VERSION
        } else {
            self.version.as_str()
        }
    }

    /// The ARN a LISTING entry this record stands for reports: the version ARN of its
    /// three assigned nodes, or the name ARN when the entry aggregates every version of
    /// one name. Neither carries the build number — an entry stands for a version or for a
    /// name, not for one build ([`ComponentVersion`]'s `arn`).
    #[must_use]
    pub fn listed_arn(&self, aggregated_by_name: bool) -> String {
        component_version_arn(
            &self.region,
            &self.name,
            self.listed_version(aggregated_by_name),
        )
    }

    /// This record as the modelled `ComponentVersion` a listing reports for the version,
    /// or for the name, its entry stands for.
    #[must_use]
    pub fn version_entry(&self, aggregated_by_name: bool) -> ComponentVersion {
        ComponentVersion {
            arn: Some(self.listed_arn(aggregated_by_name)),
            name: Some(self.name.clone()),
            version: Some(self.listed_version(aggregated_by_name).to_string()),
            description: self.description.clone(),
            platform: Some(self.platform.clone()),
            supported_os_versions: self.supported_os_versions.clone(),
            r#type: Some(self.component_type.clone()),
            owner: Some(self.owner.clone()),
            date_created: Some(self.date_created.clone()),
            status: Some(COMPONENT_STATUS_ACTIVE.to_string()),
            // See [`Self::view`]: a component a caller created here carries no Marketplace
            // product codes.
            product_codes: None,
            // Which is also why it is not billable ([`Self::billable`]).
            billable: Some(self.billable()),
        }
    }

    /// This record as the modelled `ComponentSummary` a build-version listing reports: "A
    /// high-level summary of a component", one per BUILD version, so the ARN it carries is
    /// the build version's own.
    #[must_use]
    pub fn build_summary(&self) -> ComponentSummary {
        ComponentSummary {
            arn: Some(self.arn.clone()),
            name: Some(self.name.clone()),
            version: Some(self.version.clone()),
            platform: Some(self.platform.clone()),
            supported_os_versions: self.supported_os_versions.clone(),
            state: Some(ComponentState {
                status: Some(COMPONENT_STATUS_ACTIVE.to_string()),
                reason: None,
            }),
            r#type: Some(self.component_type.clone()),
            owner: Some(self.owner.clone()),
            description: self.description.clone(),
            change_description: self.change_description.clone(),
            date_created: Some(self.date_created.clone()),
            tags: Some(reported_tags(self.tags.as_ref())),
            // See [`Self::view`]: a component a caller created here is neither third-party
            // nor hidden from view.
            publisher: None,
            obfuscate: false,
        }
    }

    /// The value of the member `filter_name` filters on, or `None` when this record
    /// reports none. See [`COMPONENT_FILTER_NAMES`].
    #[must_use]
    pub fn filterable(&self, filter_name: &str) -> Option<&str> {
        match filter_name {
            "description" => self.description.as_deref(),
            "name" => Some(self.name.as_str()),
            "platform" => Some(self.platform.as_str()),
            "status" => Some(COMPONENT_STATUS_ACTIVE),
            "type" => Some(self.component_type.as_str()),
            "version" => Some(self.version.as_str()),
            // `supportedOsVersion` is a LIST and `version` admits wildcards, so both are
            // read by [`Self::admits_filter`] instead; a stored component reports no
            // product codes (see [`Self::view`]), so a filter on them matches none.
            _ => None,
        }
    }

    /// Whether the filter `filter_name` carrying `values` admits this record.
    ///
    /// A filter admits a record when one of its values IS the value the record reports for
    /// the member the filter names \u2014 "Filter values are case-sensitive" (the modelled
    /// `values`) \u2014 with two members read differently because of what they hold:
    ///
    /// * `supportedOsVersion` names one entry of a LIST, so a value admits the record when
    ///   the record supports that OS version;
    /// * `version` admits wildcard nodes: "You can assign values for the first three, and
    ///   can filter on all of them \u2026 You can use wildcards (x) to specify the most recent
    ///   versions or nodes" (`ListComponents`), so `1.0.x` admits every patch of `1.0`.
    #[must_use]
    pub fn admits_filter(&self, filter_name: &str, values: &[String]) -> bool {
        match filter_name {
            "supportedOsVersion" => self
                .supported_os_versions
                .iter()
                .flatten()
                .any(|supported| values.iter().any(|value| value == supported)),
            COMPONENT_VERSION_FILTER_NAME => values
                .iter()
                .any(|value| version_pattern_admits(value, &self.version)),
            _ => self
                .filterable(filter_name)
                .is_some_and(|reported| values.iter().any(|value| value == reported)),
        }
    }
}

/// One stored image recipe: what a create wrote, which is what every later read reports.
#[derive(Debug, Clone)]
pub struct ImageRecipeRecord {
    /// This recipe's own ARN, which is also its key in the store.
    pub arn: String,
    /// The Region whose endpoint created it, and the only one that can reach it.
    pub region: String,
    /// The name as the caller submitted it; the ARN carries its [`normalized_name`].
    pub name: String,
    pub version: String,
    pub description: Option<String>,
    /// The ONE platform the recipe builds for. Every recipe has one, so every read
    /// reports it: see [`image_recipe_platform`] for where a create reads it from.
    pub platform: String,
    pub owner: String,
    /// The component configurations, in the order the create submitted them — empty for a
    /// recipe created without any.
    pub components: Vec<ComponentConfiguration>,
    pub parent_image: String,
    pub block_device_mappings: Option<Vec<InstanceBlockDeviceMapping>>,
    pub working_directory: Option<String>,
    pub additional_instance_configuration: Option<AdditionalInstanceConfiguration>,
    pub ami_tags: Option<BTreeMap<String, String>>,
    pub ami_watermarks: Option<Vec<String>>,
    pub tags: Option<BTreeMap<String, String>>,
    pub date_created: String,
}

impl ImageRecipeRecord {
    /// This record as the modelled `ImageRecipe` a read reports.
    #[must_use]
    pub fn view(&self) -> ImageRecipe {
        ImageRecipe {
            arn: Some(self.arn.clone()),
            // The modelled `type` member: "For an image recipe, this is always AMI. …
            // This field isn't currently returned in responses."
            r#type: None,
            name: Some(self.name.clone()),
            description: self.description.clone(),
            platform: Some(self.platform.clone()),
            owner: Some(self.owner.clone()),
            version: Some(self.version.clone()),
            components: Some(self.components.clone()),
            parent_image: Some(self.parent_image.clone()),
            block_device_mappings: self.block_device_mappings.clone(),
            date_created: Some(self.date_created.clone()),
            tags: Some(reported_tags(self.tags.as_ref())),
            working_directory: self.working_directory.clone(),
            additional_instance_configuration: self.additional_instance_configuration.clone(),
            ami_tags: self.ami_tags.clone().map(into_tag_map),
            ami_watermarks: self.ami_watermarks.clone(),
        }
    }

    /// This record as the modelled `ImageRecipeSummary` one listed page entry carries.
    #[must_use]
    pub fn summary(&self) -> ImageRecipeSummary {
        ImageRecipeSummary {
            arn: Some(self.arn.clone()),
            name: Some(self.name.clone()),
            platform: Some(self.platform.clone()),
            owner: Some(self.owner.clone()),
            parent_image: Some(self.parent_image.clone()),
            date_created: Some(self.date_created.clone()),
            tags: Some(reported_tags(self.tags.as_ref())),
        }
    }

    /// The value of the member `filter_name` filters on, or `None` when this record
    /// reports none. See [`IMAGE_RECIPE_FILTER_NAMES`].
    #[must_use]
    pub fn filterable(&self, filter_name: &str) -> Option<&str> {
        match filter_name {
            "name" => Some(self.name.as_str()),
            "parentImage" => Some(self.parent_image.as_str()),
            "platform" => Some(self.platform.as_str()),
            _ => None,
        }
    }
}

/// One stored image build version: what the build that produced it recorded, which is
/// what every later read reports.
///
/// The record exists for the sake of what a build OBSERVED, not only for the image's own
/// members: `ListImagePackages` reports "the packages that are associated with an image
/// build version, as determined by Amazon Web Services Systems Manager Inventory at build
/// time", so that inventory is taken on the instance the build ran on and is held here
/// beside the build version it was taken for. Nothing else can derive it — no recipe,
/// component or configuration this store holds says which packages an operating system
/// ended up with — so a build version whose build recorded no inventory holds none, and a
/// read of it reports none.
#[derive(Debug, Clone, Default)]
pub struct ImageBuildVersionRecord {
    /// This build version's own ARN, which is also its key in the store.
    pub arn: String,
    /// The Region whose endpoint built it, and the only one that can reach it.
    pub region: String,
    /// The image name this build version belongs to, as the recipe it was built from
    /// carries it; the ARN carries its [`normalized_name`].
    pub name: String,
    /// The three assigned version nodes this build version belongs to, taken from the
    /// recipe it was built from.
    pub version: String,
    /// The fourth version node: "Image Builder automatically assigns the build number to
    /// the fourth node", counted up per name+version pair from [`FIRST_BUILD_VERSION`].
    pub build_version: u64,
    /// The pipeline whose execution created this build version, when a pipeline did \u2014 the
    /// modelled `sourcePipelineArn` is "only present for images that a pipeline execution
    /// created".
    pub source_pipeline_arn: Option<String>,
    /// The image recipe this build version was built from, as the reference the build
    /// resolved to at the instant it started: "this is the image recipe that Image
    /// Builder used to create the image" (the modelled `imageRecipe` member of `Image`).
    /// A build of a container recipe, which this store holds none of, carries none.
    pub image_recipe_arn: Option<String>,
    /// The infrastructure configuration the build ran under: "the infrastructure that
    /// Image Builder used to create this image" (`Image`'s `infrastructureConfiguration`).
    pub infrastructure_configuration_arn: Option<String>,
    /// Whether the build collected the additional image metadata, as the request or the
    /// pipeline that asked for it named it, with the documented default filled in
    /// ([`DEFAULT_ENHANCED_IMAGE_METADATA_ENABLED`]).
    pub enhanced_image_metadata_enabled: bool,
    /// The test settings the build applied, with the documented defaults already filled in
    /// ([`DEFAULT_IMAGE_TESTS_ENABLED`], [`DEFAULT_IMAGE_TESTS_TIMEOUT_MINUTES`]): "the
    /// image test settings that Image Builder used when it created this image".
    pub image_tests_configuration: ImageTestsConfiguration,
    /// The role the build's workflow actions ran under, when the request or the pipeline
    /// named one: "the name or Amazon Resource Name (ARN) for the IAM role you create that
    /// grants Image Builder access to perform workflow actions".
    pub execution_role: Option<String>,
    /// Where the build's logs were directed: "the logging configuration that's defined for
    /// the image", which for a pipeline's build is that pipeline's `imageLogGroupName`.
    pub logging_configuration: Option<ImageLoggingConfiguration>,
    /// The platform the image was built for, taken from the recipe it was built from.
    pub platform: Option<String>,
    /// Whether the build produced an AMI or a container image, from the kind of recipe it
    /// was built from: the modelled `type` member of `ImageRecipe` documents "For an image
    /// recipe, this is always AMI", and a container recipe's images are `DOCKER`.
    pub image_type: Option<String>,
    /// How the build was initiated, from the modelled `BuildType` enum: a build a caller
    /// asked for by hand is [`BUILD_TYPE_USER_INITIATED`].
    pub build_type: Option<String>,
    /// The instant the build that produced it was queued, as a read reports it.
    pub date_created: String,
    /// The instant the build that produced it is due to finish, when one is outstanding:
    /// the status the record reports is read from the clock against it
    /// ([`Self::status_at`]). A record filed for a build that is not outstanding carries
    /// none and is already available.
    pub available_at: Option<SystemTime>,
    /// The tags applied to the image resource, which for a manually started build are the
    /// ones that request carried: "Builds that you start manually use the tags from the
    /// StartImagePipelineExecution request" (the modelled `imageTags` member).
    pub tags: Option<BTreeMap<String, String>>,
    /// The package inventory recorded for this build version, keyed by package name: one
    /// version per name, held in name order, so two reads of one page report the same
    /// entries and a token's position in the listing is stable. Bounded at
    /// [`MAX_IMAGE_PACKAGES`] where it is filed
    /// ([`ComAmazonawsImagebuilderImagebuilderState::insert_image`]).
    pub packages: BTreeMap<String, String>,
}

impl ImageBuildVersionRecord {
    /// The status this build version reports at the instant `at`.
    ///
    /// A build that was queued is outstanding until the instant it is due to finish
    /// ([`IMAGE_BUILD_SPAN`]), which the clock decides and no call has to materialise: the
    /// same read answers [`IMAGE_STATUS_PENDING`] before that instant and
    /// [`IMAGE_STATUS_AVAILABLE`] from it on.
    #[must_use]
    pub fn status_at(&self, at: SystemTime) -> &'static str {
        match self.available_at {
            Some(due) if at < due => IMAGE_STATUS_PENDING,
            _ => IMAGE_STATUS_AVAILABLE,
        }
    }

    /// This record as the modelled `ImageSummary` one listed page entry carries, read at
    /// the instant `at`, which only its status depends on ([`Self::status_at`]).
    #[must_use]
    pub fn summary(&self, at: SystemTime) -> ImageSummary {
        ImageSummary {
            arn: Some(self.arn.clone()),
            name: Some(self.name.clone()),
            r#type: self.image_type.clone(),
            version: Some(self.version.clone()),
            platform: self.platform.clone(),
            // "The operating system version for instances that launch from this image":
            // nothing this store holds says which OS version a parent image carries (a
            // recipe's `parentImage` is an AMI id, an ARN or a parameter name), so a build
            // version reports none rather than naming one nothing observed.
            os_version: None,
            state: Some(ImageState {
                status: Some(self.status_at(at).to_string()),
                reason: None,
                failure_context: None,
            }),
            owner: Some(::engine::EMULATOR_ACCOUNT_ID.to_string()),
            date_created: Some(self.date_created.clone()),
            // "The output resources that Image Builder produces for this image": this
            // emulator runs no build instance and distributes nothing, so a build version
            // reports no output AMI or container image rather than naming one that exists
            // nowhere.
            output_resources: None,
            tags: Some(reported_tags(self.tags.as_ref())),
            build_type: self.build_type.clone(),
            // "The origin of the base image that Image Builder used to build this image":
            // a recipe's `parentImage` is caller-supplied text that nothing here can place
            // in that enum, so none is reported.
            image_source: None,
            deprecation_time: None,
            lifecycle_execution_id: None,
            logging_configuration: None,
        }
    }

    /// The value of the member `filter_name` filters on, or `None` when this record
    /// reports none. See [`IMAGE_BUILD_VERSION_FILTER_NAMES`].
    #[must_use]
    pub fn filterable(&self, filter_name: &str) -> Option<&str> {
        match filter_name {
            "name" => Some(self.name.as_str()),
            "version" => Some(self.version.as_str()),
            "platform" => self.platform.as_deref(),
            "type" => self.image_type.as_deref(),
            // A build version reports no `osVersion` (see [`Self::summary`]), so a filter
            // on it matches none.
            _ => None,
        }
    }
}

/// One stored image pipeline: what a create wrote, which is what every later read reports
/// \u2014 beside the two members a read derives from the runs the pipeline has started
/// ([`ComAmazonawsImagebuilderImagebuilderState::image_pipeline_view`]).
#[derive(Debug, Clone)]
pub struct ImagePipelineRecord {
    /// This pipeline's own ARN, which is also its key in the store.
    pub arn: String,
    /// The Region whose endpoint created it, and the only one that can reach it.
    pub region: String,
    /// The name as the caller submitted it; the ARN carries its [`normalized_name`].
    pub name: String,
    pub description: Option<String>,
    /// The platform the pipeline builds for: "the platform of the image pipeline,
    /// inherited from the recipe that the pipeline uses" (the modelled `platform`
    /// member), read from that recipe when the pipeline was created.
    pub platform: Option<String>,
    pub enhanced_image_metadata_enabled: bool,
    /// Exactly one of this and `container_recipe_arn` is held: "You must specify exactly
    /// one recipe for the pipeline, using either a containerRecipeArn or an
    /// imageRecipeArn" (`CreateImagePipeline`). The reference is stored as the create
    /// submitted it, wildcard version nodes included, so each run resolves it afresh.
    pub image_recipe_arn: Option<String>,
    /// See [`Self::image_recipe_arn`].
    pub container_recipe_arn: Option<String>,
    pub infrastructure_configuration_arn: String,
    pub distribution_configuration_arn: Option<String>,
    /// The test settings the pipeline's builds apply, with the documented defaults
    /// already filled in ([`DEFAULT_IMAGE_TESTS_ENABLED`],
    /// [`DEFAULT_IMAGE_TESTS_TIMEOUT_MINUTES`]), since Image Builder "stores a default
    /// configuration" rather than storing nothing.
    pub image_tests_configuration: ImageTestsConfiguration,
    pub schedule: Option<Schedule>,
    pub status: String,
    pub tags: Option<BTreeMap<String, String>>,
    pub image_tags: Option<BTreeMap<String, String>>,
    pub image_scanning_configuration: Option<ImageScanningConfiguration>,
    pub execution_role: Option<String>,
    pub workflows: Option<Vec<WorkflowConfiguration>>,
    pub logging_configuration: Option<PipelineLoggingConfiguration>,
    /// When the create stored it.
    pub date_created: String,
    /// When an update last replaced it, and so `None` until one has — for the same reason
    /// an infrastructure configuration's is
    /// ([`InfrastructureConfigurationRecord::date_updated`]).
    pub date_updated: Option<String>,
}

/// What a successful create filed about the request it served, so a retry of that
/// request can be told from a different one carrying the same `clientToken`.
#[derive(Debug, Clone)]
pub struct CreateReceipt {
    /// The creation parameters the original request carried (one of the
    /// `*_request_fingerprint` renderings below).
    pub fingerprint: String,
    /// The `requestId` the original response named.
    pub request_id: String,
    /// The `clientToken` the original response echoed.
    pub client_token: String,
    /// The ARN the original response named.
    pub arn: String,
    /// Which receipt this is, counted from one, so the OLDEST is the one the index
    /// evicts once it is full.
    pub filed: u64,
}

/// Which of this store's resource kinds an ARN names, once the ARN has been found to
/// name a stored resource.
///
/// The modelled `ImageBuilderArn` the tagging operations take admits ten resource kinds;
/// these are the ones this store holds, so they are the ones a tag can be read from or
/// written to. An ARN of any other kind names nothing stored and is reported missing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaggedResource {
    /// An infrastructure configuration ([`INFRASTRUCTURE_CONFIGURATION_ARN_PREFIX`]).
    InfrastructureConfiguration,
    /// A distribution configuration ([`DISTRIBUTION_CONFIGURATION_ARN_PREFIX`]).
    DistributionConfiguration,
    /// One component build version ([`COMPONENT_ARN_PREFIX`]).
    Component,
    /// An image recipe ([`IMAGE_RECIPE_ARN_PREFIX`]).
    ImageRecipe,
    /// A container recipe ([`CONTAINER_RECIPE_ARN_PREFIX`]).
    ContainerRecipe,
    /// An image pipeline ([`IMAGE_PIPELINE_ARN_PREFIX`]).
    ImagePipeline,
    /// One image build version ([`IMAGE_ARN_PREFIX`]).
    Image,
    /// One workflow build version ([`WORKFLOW_ARN_PREFIX`]).
    Workflow,
}

/// Per-service state for com-amazonaws-imagebuilder-imagebuilder. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsImagebuilderImagebuilderState {
    /// Every stored infrastructure configuration, keyed by its own ARN — which
    /// carries both its Region and its normalized name, so one key says where a
    /// configuration lives and which name is taken there.
    pub infrastructure_configurations: BTreeMap<String, InfrastructureConfigurationRecord>,
    /// Every stored distribution configuration, keyed by its own ARN — which carries both
    /// its Region and its normalized name, so one key says where a configuration lives
    /// and which name is taken there.
    pub distribution_configurations: BTreeMap<String, DistributionConfigurationRecord>,
    /// Every stored component build version, keyed by its own build version ARN — which
    /// carries its Region, its normalized name, its version and its build number.
    pub components: BTreeMap<String, ComponentRecord>,
    /// Every stored image recipe, keyed by its own ARN — which carries its Region, its
    /// normalized name and its version, so one key says which name+version pair is
    /// taken where.
    pub image_recipes: BTreeMap<String, ImageRecipeRecord>,
    /// Every stored container recipe, keyed by its own ARN — which carries its Region, its
    /// normalized name and its version, exactly as an image recipe's does, so one key says
    /// which name+version pair is taken where.
    pub container_recipes: BTreeMap<String, ContainerRecipeRecord>,
    /// Every stored image pipeline, keyed by its own ARN — which carries its Region and
    /// its normalized name, so one key says which pipeline name is taken where.
    pub image_pipelines: BTreeMap<String, ImagePipelineRecord>,
    /// Every stored workflow build version, keyed by its own build version ARN — which
    /// carries its Region, its stage, its normalized name, its version and its build
    /// number, so one key says which name+version pair is taken for which stage where.
    pub workflows: BTreeMap<String, WorkflowRecord>,
    /// Every stored image build version, keyed by its own build version ARN — which
    /// carries its Region, its normalized name, its version and its build number. The
    /// image-build path is what files one ([`Self::insert_image`]), together with the
    /// package inventory the build recorded.
    pub images: BTreeMap<String, ImageBuildVersionRecord>,
    /// The answer each `clientToken` already got, keyed by (operation, Region, token):
    /// an idempotency token is spent on ONE operation, so the same token may create a
    /// component and a recipe. Bounded on its own, by age, at [`MAX_CREATE_RECEIPTS`] —
    /// see [`Self::file_create_receipt`].
    pub create_receipts: BTreeMap<(String, String, String), CreateReceipt>,
    /// How many receipts have been filed, which is what orders them by age.
    pub create_receipts_filed: u64,
    /// The resource policy applied to each resource that carries one, keyed by the
    /// resource's OWN ARN.
    ///
    /// A resource policy is attached to a resource rather than being part of it — every
    /// policy pair in this model (`Put*Policy` / `Get*Policy`) names its resource by that
    /// resource's own ARN and carries nothing else — so one index serves every resource
    /// kind, and a resource that was never given a policy simply has no entry here. The
    /// ARN carries the Region, and the resource lookup a policy call makes first is what
    /// keeps a regional endpoint to its own Region's resources.
    pub resource_policies: BTreeMap<String, String>,
    /// The workflow step executions this account has WAITING: the steps of a workflow run
    /// that have paused at a `WaitForAction` step, keyed by their own step execution id —
    /// what `ListWaitingWorkflowSteps` reports and `SendWorkflowStepAction` answers for.
    ///
    /// Filed by whatever RUNS a workflow of an image build, through the one bounded write
    /// path this store has for it ([`Self::insert_waiting_workflow_step`]). No operation of
    /// this service calls that path yet: none of them starts a workflow execution, and an
    /// image build's record holds only what the build itself observed about the image
    /// ([`ImageBuildVersionRecord`]) — so on a running emulator this index is empty, and a
    /// waiting-step listing reports none rather than a step nothing ever paused at.
    pub waiting_workflow_steps: BTreeMap<String, WorkflowStepExecution>,
    /// The secret this store's page tokens are authenticated under, drawn from the system's
    /// randomness when the store is created ([`PageTokenKey`]) — which is what lets a
    /// paginated read tell a `nextToken` it ISSUED from one a caller built, and refuse the
    /// latter with the modelled `InvalidPaginationTokenException`.
    pub page_token_key: PageTokenKey,
}

impl ComAmazonawsImagebuilderImagebuilderState {
    /// The configuration `arn` names, but only when it belongs to `request_region`.
    ///
    /// A regional endpoint serves only its own Region's resources, so a configuration
    /// of another Region answers as if it did not exist.
    #[must_use]
    pub fn infrastructure_configuration(
        &self,
        arn: &str,
        request_region: &str,
    ) -> Option<&InfrastructureConfigurationRecord> {
        self.infrastructure_configurations
            .get(arn)
            .filter(|record| record.region == request_region)
    }

    /// Store a newly created configuration together with the receipt for the request
    /// that created it.
    pub fn insert_infrastructure_configuration(
        &mut self,
        record: InfrastructureConfigurationRecord,
        client_token: &str,
        fingerprint: String,
        request_id: String,
    ) {
        self.file_create_receipt(
            CREATE_INFRASTRUCTURE_CONFIGURATION_OPERATION,
            &record.region,
            client_token,
            fingerprint,
            request_id,
            record.arn.clone(),
        );
        self.infrastructure_configurations
            .insert(record.arn.clone(), record);
    }

    /// The receipt `client_token` already earned from `CreateInfrastructureConfiguration`
    /// in `request_region`, if any.
    #[must_use]
    pub fn infrastructure_configuration_receipt(
        &self,
        request_region: &str,
        client_token: &str,
    ) -> Option<&CreateReceipt> {
        self.create_receipt(
            CREATE_INFRASTRUCTURE_CONFIGURATION_OPERATION,
            request_region,
            client_token,
        )
    }

    /// Whether the store is already holding as many configurations as this emulator
    /// will keep ([`MAX_INFRASTRUCTURE_CONFIGURATIONS`]).
    #[must_use]
    pub fn infrastructure_configuration_store_is_full(&self) -> bool {
        self.infrastructure_configurations.len() >= MAX_INFRASTRUCTURE_CONFIGURATIONS
    }

    /// The distribution configuration `arn` names, but only when it belongs to
    /// `request_region`.
    ///
    /// A regional endpoint serves only its own Region's resources, so a configuration of
    /// another Region answers as if it did not exist.
    #[must_use]
    pub fn distribution_configuration(
        &self,
        arn: &str,
        request_region: &str,
    ) -> Option<&DistributionConfigurationRecord> {
        self.distribution_configurations
            .get(arn)
            .filter(|record| record.region == request_region)
    }

    /// Store a newly created distribution configuration together with the receipt for the
    /// request that created it.
    pub fn insert_distribution_configuration(
        &mut self,
        record: DistributionConfigurationRecord,
        client_token: &str,
        fingerprint: String,
        request_id: String,
    ) {
        self.file_create_receipt(
            CREATE_DISTRIBUTION_CONFIGURATION_OPERATION,
            &record.region,
            client_token,
            fingerprint,
            request_id,
            record.arn.clone(),
        );
        self.distribution_configurations
            .insert(record.arn.clone(), record);
    }

    /// The receipt `client_token` already earned from `CreateDistributionConfiguration`
    /// in `request_region`, if any.
    #[must_use]
    pub fn distribution_configuration_receipt(
        &self,
        request_region: &str,
        client_token: &str,
    ) -> Option<&CreateReceipt> {
        self.create_receipt(
            CREATE_DISTRIBUTION_CONFIGURATION_OPERATION,
            request_region,
            client_token,
        )
    }

    /// Whether the store is already holding as many distribution configurations as this
    /// emulator will keep ([`MAX_DISTRIBUTION_CONFIGURATIONS`]).
    #[must_use]
    pub fn distribution_configuration_store_is_full(&self) -> bool {
        self.distribution_configurations.len() >= MAX_DISTRIBUTION_CONFIGURATIONS
    }

    /// The receipt `client_token` already earned from `operation` in `request_region`,
    /// if any.
    #[must_use]
    pub fn create_receipt(
        &self,
        operation: &str,
        request_region: &str,
        client_token: &str,
    ) -> Option<&CreateReceipt> {
        self.create_receipts.get(&(
            operation.to_string(),
            request_region.to_string(),
            client_token.to_string(),
        ))
    }

    /// File the receipt for a request that just created `arn`.
    ///
    /// The receipt index is bounded by AGE rather than by refusing the create: a
    /// receipt is only ever read by a caller still retrying its own request, so past
    /// [`MAX_CREATE_RECEIPTS`] — the index's own bound — the oldest is dropped. A
    /// retry whose receipt was evicted is then answered as a fresh request — which the
    /// uniqueness rule of the resource it names refuses, the same answer any caller
    /// asking for a name already taken gets — and the index can never wedge a create.
    pub fn file_create_receipt(
        &mut self,
        operation: &str,
        region: &str,
        client_token: &str,
        fingerprint: String,
        request_id: String,
        arn: String,
    ) {
        self.create_receipts_filed = self.create_receipts_filed.saturating_add(1);
        let receipt = CreateReceipt {
            fingerprint,
            request_id,
            client_token: client_token.to_string(),
            arn,
            filed: self.create_receipts_filed,
        };
        while self.create_receipts.len() >= MAX_CREATE_RECEIPTS {
            let oldest = self
                .create_receipts
                .iter()
                .min_by_key(|(_, filed)| filed.filed)
                .map(|(key, _)| key.clone());
            match oldest {
                Some(key) => {
                    self.create_receipts.remove(&key);
                }
                None => break,
            }
        }
        self.create_receipts.insert(
            (
                operation.to_string(),
                region.to_string(),
                client_token.to_string(),
            ),
            receipt,
        );
    }

    /// The component build version the ARN `reference` names, when one is stored in
    /// `request_region`.
    ///
    /// `reference` may be a build version ARN or a version ARN whose trailing nodes are
    /// `x` wildcards: "Version ARNs resolve to the latest available matching component
    /// build version" (`GetComponent`'s `componentBuildVersionArn`), so the greatest
    /// matching build version is the one answered. `None` when `reference` is not a
    /// component ARN of this service, names another Region, or matches nothing stored.
    #[must_use]
    pub fn resolve_component(
        &self,
        reference: &str,
        request_region: &str,
    ) -> Option<&ComponentRecord> {
        let reference = ComponentReference::parse(reference)?;
        if reference.region != request_region {
            return None;
        }
        self.components
            .values()
            .filter(|record| reference.matches(record))
            .max_by_key(|record| record.version_order())
    }

    /// The build number the NEXT build version of this name+version pair carries.
    ///
    /// "If a component with the same name and semantic version already exists in your
    /// account in the same Amazon Web Services Region, the request creates a new build
    /// version for it" (`CreateComponent`'s `name`), so the fourth node counts up from
    /// the greatest build already stored, and starts at [`FIRST_BUILD_VERSION`].
    #[must_use]
    pub fn next_component_build_version(
        &self,
        region: &str,
        normalized: &str,
        version: &str,
    ) -> u64 {
        self.component_builds(region, normalized, version)
            .into_iter()
            .map(|record| record.build_version)
            .max()
            .map_or(FIRST_BUILD_VERSION, |latest| latest.saturating_add(1))
    }

    /// The build version of this name+version pair with the greatest build number, if
    /// any is stored.
    #[must_use]
    pub fn latest_component_build(
        &self,
        region: &str,
        normalized: &str,
        version: &str,
    ) -> Option<&ComponentRecord> {
        self.component_builds(region, normalized, version)
            .into_iter()
            .max_by_key(|record| record.build_version)
    }

    /// Every stored build version of one component name+version pair in one Region.
    fn component_builds<'store>(
        &'store self,
        region: &str,
        normalized: &str,
        version: &str,
    ) -> Vec<&'store ComponentRecord> {
        self.components
            .values()
            .filter(|record| {
                record.region == region
                    && normalized_name(&record.name) == normalized
                    && record.version == version
            })
            .collect()
    }

    /// Store a newly created component build version together with the receipt for the
    /// request that created it.
    pub fn insert_component(
        &mut self,
        record: ComponentRecord,
        client_token: &str,
        fingerprint: String,
        request_id: String,
    ) {
        self.file_create_receipt(
            CREATE_COMPONENT_OPERATION,
            &record.region,
            client_token,
            fingerprint,
            request_id,
            record.arn.clone(),
        );
        self.components.insert(record.arn.clone(), record);
    }

    /// Whether the store is already holding as many components as this emulator will
    /// keep ([`MAX_COMPONENTS`]).
    #[must_use]
    pub fn component_store_is_full(&self) -> bool {
        self.components.len() >= MAX_COMPONENTS
    }

    /// The first stored recipe whose component list reaches the component `arn` names,
    /// if any does.
    ///
    /// A recipe reaches a build version whenever one of its references MATCHES it, not
    /// only when it is the newest match: a wildcard reference answers with whichever
    /// build is newest at the time, so every build it matches is still in use.
    #[must_use]
    pub fn recipe_depending_on_component(&self, arn: &str) -> Option<&ImageRecipeRecord> {
        let component = self.components.get(arn)?;
        self.image_recipes.values().find(|recipe| {
            recipe
                .components
                .iter()
                .filter_map(|configuration| {
                    ComponentReference::parse(&configuration.component_arn)
                })
                .any(|reference| reference.matches(component))
        })
    }

    /// Forget the component `arn` names, when it belongs to `request_region`.
    pub fn remove_component(&mut self, arn: &str, request_region: &str) -> Option<ComponentRecord> {
        self.component(arn, request_region)?;
        self.resource_policies.remove(arn);
        self.components.remove(arn)
    }

    /// The component build version `arn` names exactly, but only when it belongs to
    /// `request_region`.
    #[must_use]
    pub fn component(&self, arn: &str, request_region: &str) -> Option<&ComponentRecord> {
        self.components
            .get(arn)
            .filter(|record| record.region == request_region)
    }

    /// The image recipe `arn` names, but only when it belongs to `request_region`.
    #[must_use]
    pub fn image_recipe(&self, arn: &str, request_region: &str) -> Option<&ImageRecipeRecord> {
        self.image_recipes
            .get(arn)
            .filter(|record| record.region == request_region)
    }

    /// Store a newly created image recipe together with the receipt for the request that
    /// created it.
    pub fn insert_image_recipe(
        &mut self,
        record: ImageRecipeRecord,
        client_token: &str,
        fingerprint: String,
        request_id: String,
    ) {
        self.file_create_receipt(
            CREATE_IMAGE_RECIPE_OPERATION,
            &record.region,
            client_token,
            fingerprint,
            request_id,
            record.arn.clone(),
        );
        self.image_recipes.insert(record.arn.clone(), record);
    }

    /// The image build version `arn` names exactly, but only when it belongs to
    /// `request_region`.
    ///
    /// A regional endpoint serves only its own Region's resources, so a build version of
    /// another Region answers as if it did not exist. The lookup is EXACT: a build version
    /// ARN names one build, never the latest build of the version it belongs to.
    #[must_use]
    pub fn image(&self, arn: &str, request_region: &str) -> Option<&ImageBuildVersionRecord> {
        self.images
            .get(arn)
            .filter(|record| record.region == request_region)
    }

    /// The workflow steps of `request_region` that have paused at a `WaitForAction` step
    /// and are waiting for a response, in the step-execution-id order the index holds them
    /// in — so two reads of one page report the same entries and a token's position in the
    /// listing is stable.
    ///
    /// A step is reported only when the build version its run belongs to is one THIS Region
    /// holds ([`Self::image`]): a regional endpoint serves only its own Region's resources,
    /// and a paused step belongs to the build it paused in. The index whatever runs a
    /// workflow files those steps in is the `waiting_workflow_steps` field, filed into
    /// through [`Self::insert_waiting_workflow_step`], which no operation of this service
    /// calls yet — see that field's documentation.
    #[must_use]
    pub fn paused_workflow_steps(&self, request_region: &str) -> Vec<&WorkflowStepExecution> {
        self.waiting_workflow_steps
            .values()
            .filter(|step| {
                step.image_build_version_arn
                    .as_deref()
                    .is_some_and(|arn| self.image(arn, request_region).is_some())
            })
            .collect()
    }

    /// Store one image build version, with whatever the build recorded about it.
    ///
    /// Answers `false` and stores NOTHING once the store already holds [`MAX_IMAGES`] other
    /// build versions, so a store driven from the request path cannot grow without bound; a
    /// build checks [`Self::image_store_is_full`] first and refuses rather than reaching
    /// here. A build version the store already holds is replaced whatever the count, since
    /// that stores no additional record.
    ///
    /// The inventory ONE record keeps is bounded on its own at [`MAX_IMAGE_PACKAGES`],
    /// counted in the package-name order the record holds it in, so one build cannot grow
    /// one record without bound either.
    pub fn insert_image(&mut self, mut record: ImageBuildVersionRecord) -> bool {
        if self.image_store_is_full() && !self.images.contains_key(&record.arn) {
            return false;
        }
        if record.packages.len() > MAX_IMAGE_PACKAGES {
            record.packages = record
                .packages
                .into_iter()
                .take(MAX_IMAGE_PACKAGES)
                .collect();
        }
        self.images.insert(record.arn.clone(), record);
        true
    }

    /// Whether the store is already holding as many image build versions as this emulator
    /// will keep ([`MAX_IMAGES`]).
    #[must_use]
    pub fn image_store_is_full(&self) -> bool {
        self.images.len() >= MAX_IMAGES
    }

    /// The build number the NEXT build version of this image name+version pair carries.
    ///
    /// "Image Builder automatically assigns the build number to the fourth node"
    /// (`semanticVersion`), so each new build of one name+version pair counts up from the
    /// greatest build already stored, and the first starts at [`FIRST_BUILD_VERSION`].
    #[must_use]
    pub fn next_image_build_version(&self, region: &str, normalized: &str, version: &str) -> u64 {
        self.images
            .values()
            .filter(|record| {
                record.region == region
                    && normalized_name(&record.name) == normalized
                    && record.version == version
            })
            .map(|record| record.build_version)
            .max()
            .map_or(FIRST_BUILD_VERSION, |latest| latest.saturating_add(1))
    }

    /// The image pipeline `arn` names, but only when it belongs to `request_region`.
    ///
    /// A regional endpoint serves only its own Region's resources, so a pipeline of
    /// another Region answers as if it did not exist.
    #[must_use]
    pub fn image_pipeline(&self, arn: &str, request_region: &str) -> Option<&ImagePipelineRecord> {
        self.image_pipelines
            .get(arn)
            .filter(|record| record.region == request_region)
    }

    /// Store a newly created image pipeline together with the receipt for the request that
    /// created it.
    pub fn insert_image_pipeline(
        &mut self,
        record: ImagePipelineRecord,
        client_token: &str,
        fingerprint: String,
        request_id: String,
    ) {
        self.file_create_receipt(
            CREATE_IMAGE_PIPELINE_OPERATION,
            &record.region,
            client_token,
            fingerprint,
            request_id,
            record.arn.clone(),
        );
        self.image_pipelines.insert(record.arn.clone(), record);
    }

    /// Whether the store is already holding as many image pipelines as this emulator will
    /// keep ([`MAX_IMAGE_PIPELINES`]).
    #[must_use]
    pub fn image_pipeline_store_is_full(&self) -> bool {
        self.image_pipelines.len() >= MAX_IMAGE_PIPELINES
    }

    /// The newest build version a run of the pipeline `arn` names created, if it has run.
    ///
    /// The newest is the one whose build was queued last, and the greatest build number
    /// settles two runs queued in the same instant.
    #[must_use]
    pub fn latest_pipeline_run(&self, arn: &str) -> Option<&ImageBuildVersionRecord> {
        self.images
            .values()
            .filter(|record| record.source_pipeline_arn.as_deref() == Some(arn))
            .max_by_key(|record| (record.available_at, record.build_version))
    }

    /// The pipeline `arn` names as the modelled `ImagePipeline` a read reports at the
    /// instant `at`, or `None` when `request_region` holds no pipeline under that ARN.
    ///
    /// Every member but two is what the create stored. The last run's date and status are
    /// read from the runs the store holds ([`Self::latest_pipeline_run`]), so a pipeline
    /// nothing has started reports neither, and the status it reports for the one it did
    /// start moves with the clock rather than with what anything wrote.
    #[must_use]
    pub fn image_pipeline_view(
        &self,
        arn: &str,
        request_region: &str,
        at: SystemTime,
    ) -> Option<ImagePipeline> {
        let record = self.image_pipeline(arn, request_region)?;
        let last_run = self.latest_pipeline_run(&record.arn);
        Some(ImagePipeline {
            arn: Some(record.arn.clone()),
            name: Some(record.name.clone()),
            description: record.description.clone(),
            platform: record.platform.clone(),
            enhanced_image_metadata_enabled: Some(record.enhanced_image_metadata_enabled),
            image_recipe_arn: record.image_recipe_arn.clone(),
            container_recipe_arn: record.container_recipe_arn.clone(),
            infrastructure_configuration_arn: Some(record.infrastructure_configuration_arn.clone()),
            distribution_configuration_arn: record.distribution_configuration_arn.clone(),
            image_tests_configuration: Some(record.image_tests_configuration.clone()),
            schedule: record.schedule.clone(),
            status: Some(record.status.clone()),
            date_created: Some(record.date_created.clone()),
            date_updated: record.date_updated.clone(),
            date_last_run: last_run.map(|run| run.date_created.clone()),
            last_run_status: last_run.map(|run| run.status_at(at).to_string()),
            // "The next date when the pipeline is scheduled to run": nothing in this
            // emulator runs a pipeline on its schedule, so no run is due at any date and
            // a read reports none rather than naming a date nothing will act on.
            date_next_run: None,
            tags: Some(reported_tags(record.tags.as_ref())),
            image_scanning_configuration: record.image_scanning_configuration.clone(),
            image_tags: record.image_tags.clone().map(into_tag_map),
            execution_role: record.execution_role.clone(),
            workflows: record.workflows.clone(),
            logging_configuration: record.logging_configuration.clone(),
            // "Image Builder tracks the number of consecutive failures for scheduled
            // pipeline executions": no scheduled execution has run, so none has failed.
            consecutive_failures: Some(0),
        })
    }

    /// The image recipe the ARN `reference` names, when one is stored in `request_region`.
    ///
    /// `reference` may be a recipe ARN naming an exact version, or one whose trailing
    /// version nodes are `x` wildcards \u2014 "Version ARNs resolve to the latest available
    /// matching" record \u2014 so the greatest matching version is the one answered. That is
    /// what lets a pipeline hold one reference and have each of its runs build the newest
    /// recipe the reference reaches.
    #[must_use]
    pub fn resolve_image_recipe(
        &self,
        reference: &str,
        request_region: &str,
    ) -> Option<&ImageRecipeRecord> {
        let reference = VersionedReference::parse(reference, IMAGE_RECIPE_ARN_PREFIX)?;
        if reference.region != request_region || reference.build_version.is_some() {
            return None;
        }
        self.image_recipes
            .values()
            .filter(|record| {
                // A regional endpoint serves only its own Region's resources, so a recipe of
                // another Region is reached by no reference of this one — whatever name and
                // version it carries.
                record.region == request_region
                    && reference.name == normalized_name(&record.name)
                    && version_admits(&reference.version, &record.version)
            })
            .max_by_key(|record| parse_semantic_version(&record.version).unwrap_or_default())
    }

    /// File one image build version a caller's request queued, under the `operation` whose
    /// `clientToken` earned it, together with the receipt for that request.
    ///
    /// Both paths that queue a build — a pipeline execution and a build the caller asked
    /// for by hand — file one record and one receipt, so they differ only in which
    /// operation's receipt index the token is spent in.
    ///
    /// Answers `false` and files NOTHING when the build-version store is full, for the
    /// reason [`Self::insert_image`] states, so the receipt is filed only for a run that
    /// really was queued.
    pub fn file_image_build(
        &mut self,
        operation: &str,
        record: ImageBuildVersionRecord,
        client_token: &str,
        fingerprint: String,
        request_id: String,
    ) -> bool {
        let (region, arn) = (record.region.clone(), record.arn.clone());
        if !self.insert_image(record) {
            return false;
        }
        self.file_create_receipt(
            operation,
            &region,
            client_token,
            fingerprint,
            request_id,
            arn,
        );
        true
    }

    /// Forget the image recipe `arn` names, when it belongs to `request_region`.
    pub fn remove_image_recipe(
        &mut self,
        arn: &str,
        request_region: &str,
    ) -> Option<ImageRecipeRecord> {
        self.image_recipe(arn, request_region)?;
        self.resource_policies.remove(arn);
        self.image_recipes.remove(arn)
    }

    /// The policy document applied to the resource `arn` names, when one has been applied.
    ///
    /// `None` says no policy was ever applied to that ARN, which a read reports as
    /// [`UNSET_RESOURCE_POLICY`] — whether the ARN names a stored resource is a separate
    /// question, answered by that resource's own lookup.
    #[must_use]
    pub fn resource_policy(&self, arn: &str) -> Option<&str> {
        self.resource_policies.get(arn).map(String::as_str)
    }

    /// Apply `policy` to the resource `arn` names, replacing whatever policy it carried, and
    /// answer whether it was applied.
    ///
    /// A policy write is a REPLACEMENT, not a merge: the document stored here is the
    /// document the matching read reports back. What is stored is this store's own
    /// RENDERING of the submitted document rather than the submitted bytes — the
    /// whitespace between its JSON tokens dropped, and a principal value given as a
    /// one-element array of one string written as that string
    /// (`service_common::iam_policy_rendering`), which is the form a read of this service
    /// reports. Every other element keeps the form the caller wrote, an `Action` list
    /// included.
    ///
    /// The index is BOUNDED at [`MAX_RESOURCE_POLICIES`], like every other store filed into
    /// from the request path: past it a policy for an ARN the index does not already carry
    /// one for is not filed. No caller can observe that, and nothing is reported as applied
    /// that was not — a policy call of this service resolves its resource against this store
    /// and reports an ARN it holds nothing under as missing BEFORE reaching here, and the
    /// bound is the total of every resource store's own cap, so an ARN that got this far
    /// always has room.
    pub fn put_resource_policy(&mut self, arn: &str, policy: &str) {
        if self.resource_policy_store_is_full() && !self.resource_policies.contains_key(arn) {
            return;
        }
        self.resource_policies.insert(
            arn.to_string(),
            ::service_common::iam_policy_rendering::rendered_collapsing(
                policy,
                &::service_common::iam_policy_rendering::PRINCIPAL_ELEMENTS,
            ),
        );
    }

    /// Whether the index is already holding as many resource policies as this emulator will
    /// keep ([`MAX_RESOURCE_POLICIES`]).
    #[must_use]
    pub fn resource_policy_store_is_full(&self) -> bool {
        self.resource_policies.len() >= MAX_RESOURCE_POLICIES
    }

    /// Whether the store is already holding as many image recipes as this emulator will
    /// keep ([`MAX_IMAGE_RECIPES`]).
    #[must_use]
    pub fn image_recipe_store_is_full(&self) -> bool {
        self.image_recipes.len() >= MAX_IMAGE_RECIPES
    }

    /// Which stored resource `arn` names, when one of this Region's resources carries that
    /// ARN exactly.
    ///
    /// A tagging call names its resource by the resource's OWN ARN, so the lookup is exact:
    /// a component ARN answers the build version it names, never the latest build of a
    /// version it resolves to. A regional endpoint reaches only its own Region's
    /// resources, which each lookup below already enforces.
    #[must_use]
    pub fn tagged_resource(&self, arn: &str, request_region: &str) -> Option<TaggedResource> {
        if self.infrastructure_configuration(arn, request_region).is_some() {
            return Some(TaggedResource::InfrastructureConfiguration);
        }
        if self.distribution_configuration(arn, request_region).is_some() {
            return Some(TaggedResource::DistributionConfiguration);
        }
        if self.component(arn, request_region).is_some() {
            return Some(TaggedResource::Component);
        }
        if self.image_recipe(arn, request_region).is_some() {
            return Some(TaggedResource::ImageRecipe);
        }
        if self.container_recipe(arn, request_region).is_some() {
            return Some(TaggedResource::ContainerRecipe);
        }
        if self.image_pipeline(arn, request_region).is_some() {
            return Some(TaggedResource::ImagePipeline);
        }
        if self.image(arn, request_region).is_some() {
            return Some(TaggedResource::Image);
        }
        if self.workflow(arn, request_region).is_some() {
            return Some(TaggedResource::Workflow);
        }
        None
    }

    /// The tags the resource `arn` names currently carries, or `None` when nothing in
    /// this Region carries that ARN.
    ///
    /// Named for the resource's own `tags` member, which is what a tagging call reads and
    /// writes — NOT the `resourceTags` an infrastructure configuration puts on its build
    /// and test instances, which is a different member of one record.
    ///
    /// A stored resource that was never tagged carries NO tags, which is an empty map
    /// rather than an absent resource — the two are different answers.
    #[must_use]
    pub fn tags_of_resource(
        &self,
        arn: &str,
        request_region: &str,
    ) -> Option<BTreeMap<String, String>> {
        let stored = match self.tagged_resource(arn, request_region)? {
            TaggedResource::InfrastructureConfiguration => self
                .infrastructure_configurations
                .get(arn)
                .and_then(|record| record.tags.clone()),
            TaggedResource::DistributionConfiguration => self
                .distribution_configurations
                .get(arn)
                .and_then(|record| record.tags.clone()),
            TaggedResource::Component => {
                self.components.get(arn).and_then(|record| record.tags.clone())
            }
            TaggedResource::ImageRecipe => self
                .image_recipes
                .get(arn)
                .and_then(|record| record.tags.clone()),
            TaggedResource::ContainerRecipe => self
                .container_recipes
                .get(arn)
                .and_then(|record| record.tags.clone()),
            TaggedResource::ImagePipeline => self
                .image_pipelines
                .get(arn)
                .and_then(|record| record.tags.clone()),
            TaggedResource::Image => {
                self.images.get(arn).and_then(|record| record.tags.clone())
            }
            TaggedResource::Workflow => self
                .workflows
                .get(arn)
                .and_then(|record| record.tags.clone()),
        };
        Some(stored.unwrap_or_default())
    }

    /// The tag map of the resource `arn` names, to write through, or `None` when nothing
    /// in this Region carries that ARN.
    ///
    /// Hands back the record's own `tags` member, so a tag written here is the tag every
    /// later read of that resource reports — whether the read is a tag listing or the
    /// resource's own Get.
    pub fn tags_of_resource_mut(
        &mut self,
        arn: &str,
        request_region: &str,
    ) -> Option<&mut Option<BTreeMap<String, String>>> {
        match self.tagged_resource(arn, request_region)? {
            TaggedResource::InfrastructureConfiguration => self
                .infrastructure_configurations
                .get_mut(arn)
                .map(|record| &mut record.tags),
            TaggedResource::DistributionConfiguration => self
                .distribution_configurations
                .get_mut(arn)
                .map(|record| &mut record.tags),
            TaggedResource::Component => {
                self.components.get_mut(arn).map(|record| &mut record.tags)
            }
            TaggedResource::ImageRecipe => {
                self.image_recipes.get_mut(arn).map(|record| &mut record.tags)
            }
            TaggedResource::ContainerRecipe => self
                .container_recipes
                .get_mut(arn)
                .map(|record| &mut record.tags),
            TaggedResource::ImagePipeline => self
                .image_pipelines
                .get_mut(arn)
                .map(|record| &mut record.tags),
            TaggedResource::Image => self.images.get_mut(arn).map(|record| &mut record.tags),
            TaggedResource::Workflow => {
                self.workflows.get_mut(arn).map(|record| &mut record.tags)
            }
        }
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsImagebuilderImagebuilderState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsImagebuilderImagebuilderState>)
}

/// The normalized form of a resource name: the form its ARN carries, and the form two
/// names are compared in.
///
/// The rule is the same for every resource this store holds, and each one's own `name`
/// member states it: "Image Builder generates the infrastructure configuration ARN from a
/// normalized form of the name, so names that differ only in case, spaces, or
/// underscores count as the same name", and `CreateComponent`'s says the same of a
/// component. Case folds down, and a space and an underscore are the same separator as a
/// hyphen — which is what makes those three forms one name. The modelled `ResourceName`
/// pattern admits only `[-_A-Za-z0-9 ]`, so the result is always `[a-z0-9-]+` and
/// therefore inside the `[a-z0-9-_]+` every one of those ARN patterns allows.
#[must_use]
pub fn normalized_name(name: &str) -> String {
    name.chars()
        .map(|c| match c {
            ' ' | '_' => '-',
            other => other.to_ascii_lowercase(),
        })
        .collect()
}

/// The ARN of the infrastructure configuration `name` names in `region`.
#[must_use]
pub fn infrastructure_configuration_arn(region: &str, name: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!(
            "{INFRASTRUCTURE_CONFIGURATION_ARN_PREFIX}{}",
            normalized_name(name)
        ),
    )
}

/// The ARN of the distribution configuration `name` names in `region`.
#[must_use]
pub fn distribution_configuration_arn(region: &str, name: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!(
            "{DISTRIBUTION_CONFIGURATION_ARN_PREFIX}{}",
            normalized_name(name)
        ),
    )
}

/// The ARN of one build version of the component `name` names at `version` in `region`.
#[must_use]
pub fn component_build_version_arn(
    region: &str,
    name: &str,
    version: &str,
    build_version: u64,
) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!(
            "{COMPONENT_ARN_PREFIX}{}/{version}/{build_version}",
            normalized_name(name)
        ),
    )
}

/// The ARN of the component `name` names at `version` in `region`, carrying no build
/// number: the VERSION ARN of three assigned nodes, or the name ARN when `version` is
/// [`WILDCARD_VERSION`].
#[must_use]
pub fn component_version_arn(region: &str, name: &str, version: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("{COMPONENT_ARN_PREFIX}{}/{version}", normalized_name(name)),
    )
}

/// The ARN of the image recipe `name` names at `version` in `region`.
#[must_use]
pub fn image_recipe_arn(region: &str, name: &str, version: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!(
            "{IMAGE_RECIPE_ARN_PREFIX}{}/{version}",
            normalized_name(name)
        ),
    )
}

/// The ARN of one build version of the image `name` names at `version` in `region`.
#[must_use]
pub fn image_build_version_arn(
    region: &str,
    name: &str,
    version: &str,
    build_version: u64,
) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!(
            "{IMAGE_ARN_PREFIX}{}/{version}/{build_version}",
            normalized_name(name)
        ),
    )
}

/// The ARN of the image pipeline `name` names in `region`.
#[must_use]
pub fn image_pipeline_arn(region: &str, name: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("{IMAGE_PIPELINE_ARN_PREFIX}{}", normalized_name(name)),
    )
}

/// The four wildcard-version ARNs a response reports beside the versioned resource `arn`
/// names, or `None` when `arn` is not an ARN of a resource stored under
/// `resource_prefix`.
///
/// `LatestVersionReferences` is "a set of wildcard version ARNs that always reference the
/// latest version of the resource. ARNs are included for the latest version overall, and
/// for the latest versions within the same major, minor, and patch levels", and a wildcard
/// is admitted only in a TRAILING position (`GetComponent`'s
/// `componentBuildVersionArn`: "The version can use the `x` wildcard in trailing
/// positions"). So the four of them are this resource's own ARN with its version replaced
/// by `x.x.x`, `<major>.x.x`, `<major>.<minor>.x` and its exact `<major>.<minor>.<patch>`
/// — each a reference this service RESOLVES
/// ([`ComAmazonawsImagebuilderImagebuilderState::resolve_component`],
/// [`ComAmazonawsImagebuilderImagebuilderState::resolve_image_recipe`]), so what a client
/// reads here reaches the latest matching version at the time it is used. A build number
/// is deliberately not carried: a reference pinned to one build is not a reference to the
/// latest.
#[must_use]
pub fn latest_version_references(
    arn: &str,
    resource_prefix: &str,
) -> Option<LatestVersionReferences> {
    let reference = VersionedReference::parse(arn, resource_prefix)?;
    let assigned = |node: &VersionNode| match node {
        VersionNode::Exact(value) => Some(value.to_string()),
        VersionNode::Any => None,
    };
    let [major, minor, patch] = [
        assigned(&reference.version[0])?,
        assigned(&reference.version[1])?,
        assigned(&reference.version[2])?,
    ];
    let wildcard = WILDCARD_VERSION_NODE;
    let at = |version: String| {
        Some(::engine::emulator_arn(
            ARN_SERVICE,
            &reference.region,
            &format!("{resource_prefix}{}/{version}", reference.name),
        ))
    };
    Some(LatestVersionReferences {
        latest_version_arn: at(format!("{wildcard}.{wildcard}.{wildcard}")),
        latest_major_version_arn: at(format!("{major}.{wildcard}.{wildcard}")),
        latest_minor_version_arn: at(format!("{major}.{minor}.{wildcard}")),
        latest_patch_version_arn: at(format!("{major}.{minor}.{patch}")),
    })
}

/// The ONE platform the image recipe a create describes builds for.
///
/// Every recipe has one, so every read of one reports it. Where it comes from:
///
/// * the components the recipe includes, when it includes any. They are resources this
///   store holds, each carrying the platform its own create named, and a recipe's
///   components "must all target one platform" ([`mixed_component_platforms_message`]),
///   so that one platform is the recipe's.
/// * otherwise the parent image the recipe builds from, read by
///   [`parent_image_platform`].
#[must_use]
pub fn image_recipe_platform(components_target: Option<&str>, parent_image: &str) -> String {
    components_target.map_or_else(|| parent_image_platform(parent_image), str::to_string)
}

/// The platform of the parent image `reference` names.
///
/// `parentImage` is "AMI ID", an "Image Builder image Amazon Resource Name (ARN)", an
/// "Amazon Web Services Systems Manager (SSM) Parameter Store Parameter" or a
/// "Marketplace product ID" (the modelled member's own list). None of those is resolved
/// against EC2 by anything in this emulator, so what is read is the reference itself: the
/// Windows and macOS images a recipe builds from name their operating system in the image
/// or parameter name, and the remaining value of the modelled `Platform` enum is the one
/// every other base image carries.
#[must_use]
pub fn parent_image_platform(reference: &str) -> String {
    let named = reference.to_ascii_lowercase();
    if named.contains("windows") {
        PLATFORM_WINDOWS
    } else if named.contains("macos") {
        PLATFORM_MACOS
    } else {
        PLATFORM_LINUX
    }
    .to_string()
}

/// The three assigned nodes `version` carries, or `None` when it is not three integers
/// within the documented upper limit.
///
/// The modelled pattern admits any digits at all, so the bound
/// `semanticVersion` documents — "you can assign any positive integer value, including
/// zero. The upper limit is 2^30-1, or 1073741823, for each node" — is read here, and a
/// version past it is out of bounds rather than merely long.
#[must_use]
pub fn parse_semantic_version(version: &str) -> Option<[u64; ASSIGNED_VERSION_NODES]> {
    let mut nodes = [0_u64; ASSIGNED_VERSION_NODES];
    let mut submitted = version.split('.');
    for node in &mut nodes {
        let field = submitted.next()?;
        let value: u64 = field.parse().ok()?;
        if value > MAX_VERSION_NODE {
            return None;
        }
        *node = value;
    }
    submitted.next().is_none().then_some(nodes)
}

/// One node of a version a REFERENCE carries: an assigned value, or the `x` wildcard
/// that matches any.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VersionNode {
    /// The `x` wildcard: "The version can use the `x` wildcard in trailing positions"
    /// (`GetComponent`'s `componentBuildVersionArn`).
    Any,
    /// One assigned value.
    Exact(u64),
}

impl VersionNode {
    /// Whether this node admits the assigned `value`.
    #[must_use]
    pub fn admits(self, value: u64) -> bool {
        match self {
            Self::Any => true,
            Self::Exact(exact) => exact == value,
        }
    }
}

/// What a VERSIONED resource ARN names: everything a resolution rule compares a stored
/// record against.
///
/// One reading for every versioned resource kind of this model, since each of their ARNs
/// carries the same resource portion — `<kind>/<name>/<version>[/<build>]` — and the same
/// wildcard rule: "Version ARNs resolve to the latest available matching component build
/// version" (`GetComponent`'s `componentBuildVersionArn`), with a wildcard admitted only
/// in a trailing position.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VersionedReference {
    /// The Region field of the ARN.
    pub region: String,
    /// The normalized name the ARN carries.
    pub name: String,
    /// The three version nodes, each assigned or wildcard.
    pub version: [VersionNode; ASSIGNED_VERSION_NODES],
    /// The build number, when the ARN names one — a build version ARN does, a version
    /// ARN does not, and a resource kind that is not built at all never does.
    pub build_version: Option<u64>,
}

impl VersionedReference {
    /// What `value` names as a reference to a resource stored under `resource_prefix`, or
    /// `None` when it is not an ARN of that kind of resource of this service.
    ///
    /// The ARN's six fields come from this tree's one ARN parse
    /// (`service_common::arn`); what is read here is only the resource portion
    /// `<kind>/<name>/<version>[/<build>]`. A wildcard is admitted only in a TRAILING
    /// position, as `GetComponent`'s `componentBuildVersionArn` documents (`1.0.x`,
    /// `1.x.x`), so `x.0.0` names nothing.
    #[must_use]
    pub fn parse(value: &str, resource_prefix: &str) -> Option<Self> {
        let arn = ::service_common::arn::parse_of(value, ARN_SERVICE)?;
        let resource = arn.resource.strip_prefix(resource_prefix)?;
        let mut fields = resource.split('/');
        let name = fields.next()?;
        let version = fields.next()?;
        let build_version = match fields.next() {
            Some(build) => Some(build.parse().ok()?),
            None => None,
        };
        if fields.next().is_some() || name.is_empty() {
            return None;
        }

        let nodes = parse_version_pattern(version)?;
        // A build version ARN names an exact version: `1.x.x/2` reaches no single build.
        if build_version.is_some() && nodes.contains(&VersionNode::Any) {
            return None;
        }

        Some(Self {
            region: arn.region.to_string(),
            name: name.to_string(),
            version: nodes,
            build_version,
        })
    }
}

/// The three version nodes `value` carries, each assigned or wildcard, or `None` when it
/// is not three such nodes with every wildcard in a TRAILING position.
///
/// The ONE reading of a version pattern, so the version a reference ARN carries
/// ([`VersionedReference::parse`]) and the version a listing's filter names admit the same
/// stored versions. A wildcard is admitted only in a trailing position: "The version can
/// use the `x` wildcard in trailing positions" (`GetComponent`'s
/// `componentBuildVersionArn`), which `ListComponents` states as "When you use a wildcard
/// in any node, all nodes to the right of the first wildcard must also be wildcards".
#[must_use]
pub fn parse_version_pattern(value: &str) -> Option<[VersionNode; ASSIGNED_VERSION_NODES]> {
    let mut nodes = [VersionNode::Any; ASSIGNED_VERSION_NODES];
    let mut submitted = value.split('.');
    for node in &mut nodes {
        *node = match submitted.next()? {
            WILDCARD_VERSION_NODE => VersionNode::Any,
            assigned => VersionNode::Exact(
                assigned
                    .parse()
                    .ok()
                    .filter(|value| *value <= MAX_VERSION_NODE)?,
            ),
        };
    }
    if submitted.next().is_some() {
        return None;
    }
    // Trailing positions only: once a node is a wildcard, every later one is too.
    let wildcard_from = nodes
        .iter()
        .position(|node| *node == VersionNode::Any)
        .unwrap_or(ASSIGNED_VERSION_NODES);
    nodes
        .iter()
        .skip(wildcard_from)
        .all(|node| *node == VersionNode::Any)
        .then_some(nodes)
}

/// Whether the version `pattern` a filter names admits the `assigned` version of a stored
/// record. A value that is not a version pattern at all admits nothing.
#[must_use]
pub fn version_pattern_admits(pattern: &str, assigned: &str) -> bool {
    parse_version_pattern(pattern).is_some_and(|nodes| version_admits(&nodes, assigned))
}

/// Whether the three nodes a reference carries admit the `assigned` version of a stored
/// record. A record whose stored version is not three assigned nodes is reached by no
/// reference at all.
#[must_use]
fn version_admits(nodes: &[VersionNode; ASSIGNED_VERSION_NODES], assigned: &str) -> bool {
    parse_semantic_version(assigned).is_some_and(|values| {
        nodes
            .iter()
            .zip(values)
            .all(|(node, value)| node.admits(value))
    })
}

/// What a component ARN names: the [`VersionedReference`] reading of it, under the
/// component resource prefix.
///
/// The ONE reading of a component reference, so the read, a recipe's component list and
/// the delete's dependency check agree on which build versions one ARN reaches.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComponentReference {
    /// The Region field of the ARN.
    pub region: String,
    /// The normalized name the ARN carries.
    pub name: String,
    /// The three version nodes, each assigned or wildcard.
    pub version: [VersionNode; ASSIGNED_VERSION_NODES],
    /// The build number, when the ARN names one — a build version ARN does, a version
    /// ARN does not.
    pub build_version: Option<u64>,
}

impl From<VersionedReference> for ComponentReference {
    fn from(named: VersionedReference) -> Self {
        Self {
            region: named.region,
            name: named.name,
            version: named.version,
            build_version: named.build_version,
        }
    }
}

impl ComponentReference {
    /// What `value` names, or `None` when it is not a component ARN of this service.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        VersionedReference::parse(value, COMPONENT_ARN_PREFIX).map(Self::from)
    }

    /// Whether this reference reaches the stored build version `record`.
    #[must_use]
    pub fn matches(&self, record: &ComponentRecord) -> bool {
        if self.region != record.region || self.name != normalized_name(&record.name) {
            return false;
        }
        if self
            .build_version
            .is_some_and(|build| build != record.build_version)
        {
            return false;
        }
        version_admits(&self.version, &record.version)
    }
}

// ---------------------------------------------------------------------------
// The component document
//
// "Creates a new component that can be used to build, validate, test, and assess your
// image. The component is based on a YAML document that you specify using exactly one of
// the following methods: Inline, using the data property in the request body. A URL that
// points to a YAML document file stored in Amazon S3, using the uri property in the
// request body. Image Builder determines the component type from the document. If the
// document contains a single phase named test, the component type is TEST. Otherwise,
// the component type is BUILD." (`CreateComponent`'s own documentation.)
//
// So what a stored component derives from the document it carries is read HERE, once, by
// a maintained YAML 1.2 parser.
// ---------------------------------------------------------------------------

/// The document key the phases are listed under, which is what the component type is
/// read from.
const PHASES_KEY: &str = "phases";

/// The key one phase's name is read from.
const PHASE_NAME_KEY: &str = "name";

/// The phase name whose LONE presence makes a document a test component.
const TEST_PHASE_NAME: &str = "test";

/// The document key the component's parameters are listed under.
const PARAMETERS_KEY: &str = "parameters";

/// The keys one declared parameter carries.
const PARAMETER_TYPE_KEY: &str = "type";
const PARAMETER_DEFAULT_KEY: &str = "default";
const PARAMETER_DESCRIPTION_KEY: &str = "description";

/// The requirement clause a document declaring no phases is refused with: fixed text for
/// this class, naming the key the document left out.
const MISSING_PHASES_REQUIREMENT: &str = "Missing phases.";

/// What a component document declares: everything a stored component derives from it.
#[derive(Debug, Clone)]
pub struct ComponentDocument {
    /// `BUILD` or `TEST`, from the document's phases.
    pub component_type: String,
    /// The parameters the document defined, in the order it listed them — the modelled
    /// `parameters` member "Contains parameter details for each of the parameters that
    /// the component document defined for the component".
    pub parameters: Option<Vec<ComponentParameterDetail>>,
}

/// What a submitted component document declares, or why it is not a component document.
///
/// The `Err` is the REQUIREMENT CLAUSE the refusal states ([`component_document_message`]):
/// this service's own statement of what a
/// component document has to be, never the parser's diagnostic text, so the message one
/// malformed document earns does not depend on which reader read it. The document is
/// bounded before it is parsed: the modelled `data` member is `len 1..16000`, which the
/// constraint walk refuses past before a handler runs, and an aliased document is refused
/// here rather than expanded, so what the parser is handed is both small and finite.
pub fn parse_component_document(data: &str) -> Result<ComponentDocument, String> {
    // Before the loader — or the event scan that looks for an alias — sees the document
    // at all: how deep it may make either of them recurse.
    if guard_document_nesting(data).is_none() {
        return Err("The component document nests too deeply.".to_string());
    }
    if document_uses_aliases(data) {
        return Err("The component document must not use YAML aliases.".to_string());
    }
    let mut documents = ::yaml_rust2::YamlLoader::load_from_str(data)
        .map_err(|_| "The component document is not a valid YAML document.".to_string())?;
    if documents.len() != 1 {
        return Err(format!(
            "The component document must be a single YAML document; this request carries {}.",
            documents.len()
        ));
    }
    let document = documents.remove(0);
    let Some(map) = document.as_hash() else {
        return Err(
            "The component document must be a YAML mapping of document keys to values.".to_string(),
        );
    };

    let phases = mapping_get(map, PHASES_KEY)
        .and_then(::yaml_rust2::Yaml::as_vec)
        .map(Vec::as_slice)
        .unwrap_or_default();
    // A component document is the work Image Builder runs, and the phases are that work:
    // a document that declares none describes nothing to build, test or validate, so it is
    // not a document this service can accept. The clause is fixed text for this class.
    if phases.is_empty() {
        return Err(MISSING_PHASES_REQUIREMENT.to_string());
    }
    let only_test_phase = phases.len() == 1
        && phases.iter().all(|phase| {
            phase
                .as_hash()
                .and_then(|phase| mapping_get(phase, PHASE_NAME_KEY))
                .and_then(scalar_string)
                .as_deref()
                == Some(TEST_PHASE_NAME)
        });

    Ok(ComponentDocument {
        component_type: if only_test_phase {
            COMPONENT_TYPE_TEST.to_string()
        } else {
            COMPONENT_TYPE_BUILD.to_string()
        },
        parameters: declared_parameters(map),
    })
}

/// The parameters a component document declares, each as the modelled
/// `ComponentParameterDetail` a read reports.
///
/// A component document lists its parameters as single-key mappings of the parameter's
/// name to its details, so a list entry that is not one declares no parameter and is
/// passed over. `type` is the one detail the modelled structure requires, so an entry
/// without it declares nothing a read could report.
fn declared_parameters(document: &::yaml_rust2::yaml::Hash) -> Option<Vec<ComponentParameterDetail>> {
    let declared = mapping_get(document, PARAMETERS_KEY)?.as_vec()?;
    let parameters: Vec<ComponentParameterDetail> = declared
        .iter()
        .filter_map(|entry| {
            let entry = entry.as_hash()?;
            let (name, details) = entry.iter().next()?;
            let name = scalar_string(name)?;
            let details = details.as_hash()?;
            let parameter_type = mapping_get(details, PARAMETER_TYPE_KEY).and_then(scalar_string)?;
            Some(ComponentParameterDetail {
                name,
                r#type: parameter_type,
                default_value: mapping_get(details, PARAMETER_DEFAULT_KEY).and_then(scalar_values),
                description: mapping_get(details, PARAMETER_DESCRIPTION_KEY).and_then(scalar_string),
            })
        })
        .collect();
    (!parameters.is_empty()).then_some(parameters)
}

/// The value `key` carries in `map`, if any.
fn mapping_get<'document>(
    map: &'document ::yaml_rust2::yaml::Hash,
    key: &str,
) -> Option<&'document ::yaml_rust2::Yaml> {
    map.get(&::yaml_rust2::Yaml::String(key.to_string()))
        .or_else(|| {
            map.iter()
                .find(|(candidate, _)| candidate.as_str() == Some(key))
                .map(|(_, value)| value)
        })
}

/// One scalar node as the text a modelled string member carries.
fn scalar_string(value: &::yaml_rust2::Yaml) -> Option<String> {
    match value {
        ::yaml_rust2::Yaml::String(text) | ::yaml_rust2::Yaml::Real(text) => Some(text.clone()),
        ::yaml_rust2::Yaml::Integer(number) => Some(number.to_string()),
        ::yaml_rust2::Yaml::Boolean(flag) => Some(flag.to_string()),
        _ => None,
    }
}

/// One node as the list of values a modelled string-list member carries: a scalar is the
/// one-value list, a sequence is its scalars.
fn scalar_values(value: &::yaml_rust2::Yaml) -> Option<Vec<String>> {
    match value {
        ::yaml_rust2::Yaml::Array(items) => {
            Some(items.iter().filter_map(scalar_string).collect::<Vec<_>>())
        }
        scalar => scalar_string(scalar).map(|value| vec![value]),
    }
    .filter(|values| !values.is_empty())
}

/// Whether `data` carries a YAML alias.
///
/// An alias is read from the event stream rather than from a loaded tree, so the
/// document is never expanded to find one: an aliased document is refused, so no
/// caller-supplied document can expand into more memory than it occupies on the wire.
fn document_uses_aliases(data: &str) -> bool {
    #[derive(Default)]
    struct AliasDetector {
        found: bool,
    }

    impl ::yaml_rust2::parser::EventReceiver for AliasDetector {
        fn on_event(&mut self, event: ::yaml_rust2::parser::Event) {
            if matches!(event, ::yaml_rust2::parser::Event::Alias(_)) {
                self.found = true;
            }
        }
    }

    let mut detector = AliasDetector::default();
    // A parse failure is not an alias; the load below reports why the document is not
    // readable.
    let _ = ::yaml_rust2::parser::Parser::new_from_str(data).load(&mut detector, true);
    detector.found
}

/// How deeply `data` can make a YAML reader recurse, refused before any reader runs.
///
/// Three measures, each a bound on NESTING rather than a count of how big the document
/// is: [`MAX_DOCUMENT_FLOW_DEPTH`] flow collections open at once with
/// [`MAX_DOCUMENT_FLOW_OPENERS`] as the backstop a byte scan needs (a `]` inside a quoted
/// scalar lowers the running depth while the loader's own depth climbs, and no document
/// nests deeper than its opener COUNT), plus [`MAX_DOCUMENT_INDENT`] columns of leading
/// indentation and [`MAX_DOCUMENT_COMPACT_PER_LINE`] compact `- `/`? ` indicators on one
/// line — the two ways block nesting descends. `None` once a document is past one of them.
fn guard_document_nesting(data: &str) -> Option<()> {
    let mut flow_depth = 0usize;
    let mut flow_openers = 0usize;
    let mut compact_on_line = 0usize;
    let mut indent = 0usize;
    let mut counting_indent = true;
    let mut previous = b'\n';

    for byte in data.bytes() {
        match byte {
            b'\n' | b'\r' => {
                indent = 0;
                compact_on_line = 0;
                counting_indent = true;
            }
            b' ' | b'\t' if counting_indent => {
                indent += 1;
                if indent > MAX_DOCUMENT_INDENT {
                    return None;
                }
            }
            b'[' | b'{' => {
                counting_indent = false;
                flow_depth += 1;
                flow_openers += 1;
                if flow_depth > MAX_DOCUMENT_FLOW_DEPTH
                    || flow_openers > MAX_DOCUMENT_FLOW_OPENERS
                {
                    return None;
                }
            }
            b']' | b'}' => {
                counting_indent = false;
                flow_depth = flow_depth.saturating_sub(1);
            }
            _ => counting_indent = false,
        }
        // A `-` or `?` is a block indicator only where a token may start — at the
        // beginning of a line or after a space — and only when a space follows it, which
        // is what separates `- step` from the hyphen in `my-component` or `a-b-c`.
        if matches!(previous, b'-' | b'?') && matches!(byte, b' ' | b'\t' | b'\n' | b'\r') {
            compact_on_line += 1;
            if compact_on_line > MAX_DOCUMENT_COMPACT_PER_LINE {
                return None;
            }
        }
        previous = byte;
    }
    Some(())
}

// ---------------------------------------------------------------------------
// Page tokens
//
// Every `@paginated` listing of this service hands back a `nextToken`, and the modelled
// `InvalidPaginationTokenException` is how a token this service did not issue is refused.
// A token is OPAQUE: the only thing a caller may do with one is hand it back unchanged, so
// it exposes neither the listing it belongs to nor the query that selected it. The page
// position therefore travels INSIDE it, under an authenticator computed with a secret this
// store draws from the system's randomness and never reveals, and the listing it was
// issued for travels as a digest — so a fabricated, edited or re-aimed token carries no
// position here and is refused instead of being read as a page position. Nothing is stored
// for it: listing is a read, so it writes nothing a later caller could observe.
// ---------------------------------------------------------------------------

/// What one page token continues.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageCursor {
    /// The list operation that issued it ([`LIST_IMAGE_RECIPES_OPERATION`]).
    pub operation: String,
    /// The Region whose endpoint issued it.
    pub region: String,
    /// The query the page belongs to, so a token cannot be spent on a different one
    /// ([`image_recipe_query_fingerprint`]).
    pub query: String,
    /// The key the previous page ended at; the next page starts after it.
    pub after: String,
}

/// Separates the two fields a token's payload carries: the digest of the listing it is a
/// position in, and the key that position names. A unit separator, which no ARN and no
/// digest carries.
const PAGE_TOKEN_FIELD_SEPARATOR: char = '\u{1f}';

/// Separates the parts of the LISTING a token was issued for, inside the text the digest
/// over it is taken of. A record separator, which no operation name, Region or rendered
/// query carries, so two listings never render one text.
const PAGE_TOKEN_LISTING_SEPARATOR: char = '\u{1e}';

/// How many bytes of the listing's digest a token carries. A CHOSEN width: 128 bits is far
/// past the chance of two listings agreeing on one digest, while keeping the token short.
const PAGE_TOKEN_DIGEST_BYTES: usize = 16;

/// The secret this service's page tokens are authenticated under — the shared one every
/// paginated read of this emulator uses, so no two services disagree about what makes a
/// continuation token one the service ISSUED. One key per store
/// ([`ComAmazonawsImagebuilderImagebuilderState::page_token_key`]).
pub use ::service_common::page_tokens::PageTokenKey;

/// The `nextToken` that continues `cursor`, authenticated under `key`.
///
/// Base64 is the spelling an AWS continuation token has, through the standard-alphabet codec
/// this workspace already uses for every Smithy blob, so no second codec lives here. What it
/// renders is the authenticated bytes of the position: the LISTING the token belongs to
/// travels as a digest and the query that selected the page is nowhere in the token, so a
/// caller that stores, logs or round-trips the value sees an opaque handle rather than this
/// service's own query state, and cannot produce the authenticator for a position of its
/// own. The position itself is the key the page it continues ENDED at, which is a value the
/// caller was just handed.
#[must_use]
pub fn encode_page_token(key: &PageTokenKey, cursor: &PageCursor) -> String {
    ::aws_smithy_types::base64::encode(key.mint(&page_token_payload(cursor)))
}

/// Where the page `token` continues starts, or `None` when `token` is not a token THIS
/// store issued for THIS listing: one that is not base64, one whose authenticator does not
/// match the position it travels with (a fabricated or edited value, or one another store
/// minted), one carrying no position at all, and one issued for another operation, another
/// Region's endpoint or another query.
#[must_use]
pub fn decode_page_token(key: &PageTokenKey, token: &str, cursor_of: &PageCursor) -> Option<String> {
    let payload = key.cursor_of(&::aws_smithy_types::base64::decode(token).ok()?)?;
    let (listing, after) = payload.split_once(PAGE_TOKEN_FIELD_SEPARATOR)?;
    (listing == page_token_listing_digest(cursor_of)).then(|| after.to_string())
}

/// The payload a token authenticates: the digest of the listing it is a position in, then
/// the key the previous page ended at.
fn page_token_payload(cursor: &PageCursor) -> String {
    format!(
        "{}{PAGE_TOKEN_FIELD_SEPARATOR}{}",
        page_token_listing_digest(cursor),
        cursor.after
    )
}

/// The digest of the LISTING a token belongs to: its operation, the Region whose endpoint
/// issued it and the query that selected the items it pages over, rendered as hex.
///
/// A digest rather than the listing itself, so the token names which listing it continues
/// without carrying the operation, the Region or the caller's own filter values in a form
/// anything but this service can read.
fn page_token_listing_digest(cursor: &PageCursor) -> String {
    use ::sha2::Digest;

    let listing = [
        cursor.operation.as_str(),
        cursor.region.as_str(),
        cursor.query.as_str(),
    ]
    .join(&PAGE_TOKEN_LISTING_SEPARATOR.to_string());
    let digest = ::sha2::Sha256::digest(listing.as_bytes());
    ::hex::encode(&digest[..PAGE_TOKEN_DIGEST_BYTES])
}

/// A deterministic rendering of which image recipes one `ListImageRecipes` request asked
/// for, so a token issued for one query is refused for another.
///
/// `maxResults` is deliberately NOT part of it: the page size describes how much of the
/// listing to answer with, not which listing it is, so a caller may change it between
/// pages.
#[must_use]
pub fn image_recipe_query_fingerprint(owner: Option<&str>, filters: Option<&[Filter]>) -> String {
    format!(
        "owner={};filters={};",
        text(Some(owner.unwrap_or(OWNERSHIP_SELF))),
        filter_list(filters)
    )
}

/// A deterministic rendering of which configurations one
/// `ListInfrastructureConfigurations` request asked for, so a token issued for one query is
/// refused for another.
///
/// `maxResults` is deliberately NOT part of it, for the reason
/// [`image_recipe_query_fingerprint`] states.
#[must_use]
pub fn infrastructure_configuration_query_fingerprint(filters: Option<&[Filter]>) -> String {
    format!("filters={};", filter_list(filters))
}

/// A deterministic rendering of which components one `ListComponents` request asked for,
/// so a token issued for one query is refused for another.
///
/// `byName` is part of it: it decides whether an entry stands for one version or for a
/// whole name, so the two listings are keyed differently and a token issued for one cannot
/// be spent on the other. `maxResults` is deliberately NOT part of it, for the reason
/// [`image_recipe_query_fingerprint`] states.
#[must_use]
pub fn component_query_fingerprint(
    owner: Option<&str>,
    by_name: bool,
    filters: Option<&[Filter]>,
) -> String {
    format!(
        "owner={};byName={by_name};filters={};",
        text(Some(owner.unwrap_or(OWNERSHIP_SELF))),
        filter_list(filters)
    )
}

/// A deterministic rendering of which build versions one `ListComponentBuildVersions`
/// request asked for, so a token issued for one query is refused for another.
///
/// `maxResults` is deliberately NOT part of it, for the reason
/// [`image_recipe_query_fingerprint`] states.
#[must_use]
pub fn component_build_version_query_fingerprint(component_version_arn: Option<&str>) -> String {
    format!("componentVersionArn={};", text(component_version_arn))
}

/// A deterministic rendering of which build versions one `ListImageBuildVersions`
/// request asked for, so a token issued for one query is refused for another.
///
/// `maxResults` is deliberately NOT part of it, for the reason
/// [`image_recipe_query_fingerprint`] states.
#[must_use]
pub fn image_build_version_query_fingerprint(
    image_version_arn: Option<&str>,
    filters: Option<&[Filter]>,
) -> String {
    format!(
        "imageVersionArn={};filters={};",
        text(image_version_arn),
        filter_list(filters)
    )
}

/// A deterministic rendering of which packages one `ListImagePackages` request asked for,
/// so a token issued for one image build version is refused for another.
///
/// `maxResults` is deliberately NOT part of it, for the reason
/// [`image_recipe_query_fingerprint`] states.
#[must_use]
pub fn image_package_query_fingerprint(image_build_version_arn: &str) -> String {
    format!(
        "imageBuildVersionArn={};",
        text(Some(image_build_version_arn))
    )
}

/// The region this request was signed for, from its SigV4 credential scope; the
/// emulator's default region when it carries none.
///
/// The scope is caller-controlled text and the value lands inside an ARN whose
/// modelled pattern allows only a non-colon field there, so anything carrying a colon
/// or anything implausibly long is refused here and the default is used instead: a
/// generated ARN can never carry text the model does not admit. A request whose scope
/// is unusable is therefore served by the DEFAULT Region, so the configuration it
/// creates lives there and is read back through that Region's ARN — the same place a
/// request carrying no scope at all is served.
#[must_use]
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

/// Render `at` the way this model's string `DateTime` members carry an instant
/// (ISO 8601, e.g. `2026-10-24T01:51:48Z`).
#[must_use]
pub fn iso8601(at: SystemTime) -> String {
    ::aws_smithy_types::DateTime::from(at)
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
        .unwrap_or_default()
}

/// Mint the `requestId` one response names the request it answers.
///
/// 8-4-4-4-12 lowercase hex, the form an AWS request id takes. The VALUE is per-call
/// and is not a requirement: nothing can predict it and no caller may depend on a
/// particular one. What it must do is name ONE response, so it is drawn from a
/// per-process counter run through [`scramble`] — a bijection, so two responses never
/// carry the same id — mixed with the instant the response was answered, so two runs
/// of this emulator do not mint the same sequence.
#[must_use]
pub fn new_request_id() -> String {
    static MINTED: AtomicU64 = AtomicU64::new(0);

    let counter = MINTED.fetch_add(1, Ordering::Relaxed);
    let since_epoch = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    let instant = since_epoch
        .as_secs()
        .wrapping_mul(1_000_000_000)
        .wrapping_add(u64::from(since_epoch.subsec_nanos()));
    // The high half is a bijection of the counter alone, which is what makes two ids
    // minted in the same nanosecond differ; the low half carries the instant.
    let high = scramble(counter);
    let low = scramble(instant ^ scramble(counter.wrapping_add(1)));
    let hex = format!("{high:016x}{low:016x}");
    format!(
        "{}-{}-{}-{}-{}",
        &hex[..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

/// Mix a counter so consecutive values render as unrelated tokens.
///
/// SplitMix64's finaliser applied to the counter times the 64-bit golden ratio. Every
/// step (multiply by an odd constant, xor-shift) is a bijection on `u64`, so the whole
/// function is one: distinct counters never collide.
fn scramble(seq: u64) -> u64 {
    let mut z = seq.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// A caller-submitted tag map, ordered so a read reports it the same way twice.
#[must_use]
pub fn into_ordered_tags(tags: HashMap<String, String>) -> BTreeMap<String, String> {
    tags.into_iter().collect()
}

/// An ordered tag map as the modelled map member carries it.
fn into_tag_map(tags: BTreeMap<String, String>) -> HashMap<String, String> {
    tags.into_iter().collect()
}

/// The tags a read of a stored resource reports: the ones it carries, which is an EMPTY
/// map for a resource that carries none.
///
/// A resource and its tags are not the same question: this service's own tag read already
/// tells an untagged resource ("an empty tag map") apart from an ARN naming no resource at
/// all ([`ComAmazonawsImagebuilderImagebuilderState::tags_of_resource`]), and a resource
/// view answers the same way, so a client that iterates `tags` finds a map on every
/// resource rather than one it has to null-check.
#[must_use]
pub fn reported_tags(tags: Option<&BTreeMap<String, String>>) -> HashMap<String, String> {
    tags.cloned().map(into_tag_map).unwrap_or_default()
}

// ---------------------------------------------------------------------------
// Idempotency
//
// `clientToken` is an @idempotencyToken: "If you retry a request with the same client
// token, Image Builder returns the original response without running the operation
// again", and a token reused with different parameter values is the modelled
// `IdempotentParameterMismatchException`. Telling those two apart needs the ORIGINAL
// request's creation parameters, so a successful create files their rendering and a
// later request carrying the same token is rendered the same way and compared.
//
// The rendering is a total, deterministic text over every creation parameter: each
// member is length-prefixed so no two different inputs can render alike, and every map
// is written in key order so one request cannot render two ways.
// ---------------------------------------------------------------------------

/// How an absent member renders inside a fingerprint.
const ABSENT: &str = "-";

/// A deterministic rendering of the creation parameters one
/// `CreateInfrastructureConfiguration` request carries.
///
/// `clientToken` and `dryRun` are deliberately NOT part of it: the token is the key
/// this rendering is compared under, and a dry run never files a receipt.
#[must_use]
pub fn infrastructure_configuration_request_fingerprint(
    input: &CreateInfrastructureConfigurationInput,
) -> String {
    let mut fields: BTreeMap<&'static str, String> = BTreeMap::new();
    fields.insert("name", text(Some(input.name.as_str())));
    fields.insert("description", text(input.description.as_deref()));
    fields.insert(
        "instanceProfileName",
        text(Some(input.instance_profile_name.as_str())),
    );
    fields.insert("subnetId", text(input.subnet_id.as_deref()));
    fields.insert("keyPair", text(input.key_pair.as_deref()));
    fields.insert("snsTopicArn", text(input.sns_topic_arn.as_deref()));
    fields.insert("instanceTypes", list(input.instance_types.as_deref()));
    fields.insert("securityGroupIds", list(input.security_group_ids.as_deref()));
    fields.insert("resourceTags", tag_map(input.resource_tags.as_ref()));
    fields.insert("tags", tag_map(input.tags.as_ref()));
    fields.insert(
        "terminateInstanceOnFailure",
        flag(input.terminate_instance_on_failure),
    );
    fields.insert(
        "terminateInstanceOnCompletion",
        flag(input.terminate_instance_on_completion),
    );
    fields.insert("logging", logging_fingerprint(input.logging.as_ref()));
    fields.insert(
        "instanceMetadataOptions",
        instance_metadata_options_fingerprint(input.instance_metadata_options.as_ref()),
    );
    fields.insert("placement", placement_fingerprint(input.placement.as_ref()));
    fields.insert(
        "parentInstance",
        parent_instance_fingerprint(input.parent_instance.as_ref()),
    );
    fields
        .iter()
        .map(|(member, rendered)| format!("{member}={rendered};"))
        .collect()
}

/// The `logging` member of an infrastructure-configuration request, as every fingerprint
/// over one renders it. Shared by the create's rendering and the update's
/// ([`infrastructure_configuration_update_fingerprint`]), so one request cannot render two
/// ways depending on which operation carried it.
fn logging_fingerprint(logging: Option<&Logging>) -> String {
    logging.map_or_else(
        || ABSENT.to_string(),
        |logging| {
            logging.s3_logs.as_ref().map_or_else(
                || ABSENT.to_string(),
                |s3| {
                    group(&[
                        text(s3.s3_bucket_name.as_deref()),
                        text(s3.s3_key_prefix.as_deref()),
                    ])
                },
            )
        },
    )
}

/// The `instanceMetadataOptions` member, as every fingerprint over one renders it. See
/// [`logging_fingerprint`].
fn instance_metadata_options_fingerprint(options: Option<&InstanceMetadataOptions>) -> String {
    options.map_or_else(
        || ABSENT.to_string(),
        |options| {
            group(&[
                text(options.http_tokens.as_deref()),
                number(options.http_put_response_hop_limit),
            ])
        },
    )
}

/// The `placement` member, as every fingerprint over one renders it. See
/// [`logging_fingerprint`].
fn placement_fingerprint(placement: Option<&Placement>) -> String {
    placement.map_or_else(
        || ABSENT.to_string(),
        |placement| {
            group(&[
                text(placement.availability_zone.as_deref()),
                text(placement.tenancy.as_deref()),
                text(placement.host_id.as_deref()),
                text(placement.host_resource_group_arn.as_deref()),
            ])
        },
    )
}

/// The `parentInstance` member, as every fingerprint over one renders it. See
/// [`logging_fingerprint`].
fn parent_instance_fingerprint(parent: Option<&ParentInstanceConfiguration>) -> String {
    parent.map_or_else(
        || ABSENT.to_string(),
        |parent| {
            group(&[
                text(Some(parent.auto_scaling_group.name.as_str())),
                number(parent.auto_scaling_group.capacity_wait_timeout_seconds),
            ])
        },
    )
}

/// A deterministic rendering of the creation parameters one
/// `CreateDistributionConfiguration` request carries. See
/// [`infrastructure_configuration_request_fingerprint`] for what it leaves out and why.
#[must_use]
pub fn distribution_configuration_request_fingerprint(
    input: &CreateDistributionConfigurationInput,
) -> String {
    let mut fields: BTreeMap<&'static str, String> = BTreeMap::new();
    fields.insert("name", text(Some(input.name.as_str())));
    fields.insert("description", text(input.description.as_deref()));
    fields.insert("tags", tag_map(input.tags.as_ref()));
    fields.insert(
        "distributions",
        distributions_fingerprint(&input.distributions),
    );
    fields
        .iter()
        .map(|(member, rendered)| format!("{member}={rendered};"))
        .collect()
}

/// A deterministic rendering of the creation parameters one `CreateComponent` request
/// carries. See [`infrastructure_configuration_request_fingerprint`] for what it leaves
/// out and why.
#[must_use]
pub fn component_request_fingerprint(input: &CreateComponentInput) -> String {
    let mut fields: BTreeMap<&'static str, String> = BTreeMap::new();
    fields.insert("name", text(Some(input.name.as_str())));
    fields.insert("semanticVersion", text(Some(input.semantic_version.as_str())));
    fields.insert("platform", text(Some(input.platform.as_str())));
    fields.insert("description", text(input.description.as_deref()));
    fields.insert(
        "changeDescription",
        text(input.change_description.as_deref()),
    );
    fields.insert(
        "supportedOsVersions",
        list(input.supported_os_versions.as_deref()),
    );
    fields.insert("data", text(input.data.as_deref()));
    fields.insert("uri", text(input.uri.as_deref()));
    fields.insert("kmsKeyId", text(input.kms_key_id.as_deref()));
    fields.insert("tags", tag_map(input.tags.as_ref()));
    fields
        .iter()
        .map(|(member, rendered)| format!("{member}={rendered};"))
        .collect()
}

/// A deterministic rendering of the creation parameters one `CreateImageRecipe` request
/// carries. See [`infrastructure_configuration_request_fingerprint`] for what it leaves
/// out and why.
#[must_use]
pub fn image_recipe_request_fingerprint(input: &CreateImageRecipeInput) -> String {
    let mut fields: BTreeMap<&'static str, String> = BTreeMap::new();
    fields.insert("name", text(Some(input.name.as_str())));
    fields.insert("semanticVersion", text(Some(input.semantic_version.as_str())));
    fields.insert("parentImage", text(Some(input.parent_image.as_str())));
    fields.insert("description", text(input.description.as_deref()));
    fields.insert("workingDirectory", text(input.working_directory.as_deref()));
    fields.insert("amiWatermarks", list(input.ami_watermarks.as_deref()));
    fields.insert("tags", tag_map(input.tags.as_ref()));
    fields.insert("amiTags", tag_map(input.ami_tags.as_ref()));
    fields.insert(
        "components",
        component_configurations_fingerprint(input.components.as_deref()),
    );
    fields.insert(
        "blockDeviceMappings",
        block_device_mappings_fingerprint(input.block_device_mappings.as_deref()),
    );
    fields.insert(
        "additionalInstanceConfiguration",
        input.additional_instance_configuration.as_ref().map_or_else(
            || ABSENT.to_string(),
            |configuration| {
                group(&[
                    text(configuration.user_data_override.as_deref()),
                    configuration.systems_manager_agent.as_ref().map_or_else(
                        || ABSENT.to_string(),
                        |agent| flag(agent.uninstall_after_build),
                    ),
                ])
            },
        ),
    );
    fields
        .iter()
        .map(|(member, rendered)| format!("{member}={rendered};"))
        .collect()
}

/// A deterministic rendering of the creation parameters one `CreateImagePipeline`
/// request carries. See [`infrastructure_configuration_request_fingerprint`] for what it
/// leaves out and why.
#[must_use]
pub fn image_pipeline_request_fingerprint(input: &CreateImagePipelineInput) -> String {
    let mut fields: BTreeMap<&'static str, String> = BTreeMap::new();
    fields.insert("name", text(Some(input.name.as_str())));
    fields.insert("description", text(input.description.as_deref()));
    fields.insert("imageRecipeArn", text(input.image_recipe_arn.as_deref()));
    fields.insert(
        "containerRecipeArn",
        text(input.container_recipe_arn.as_deref()),
    );
    fields.insert(
        "infrastructureConfigurationArn",
        text(Some(input.infrastructure_configuration_arn.as_str())),
    );
    fields.insert(
        "distributionConfigurationArn",
        text(input.distribution_configuration_arn.as_deref()),
    );
    fields.insert("status", text(input.status.as_deref()));
    fields.insert("executionRole", text(input.execution_role.as_deref()));
    fields.insert(
        "enhancedImageMetadataEnabled",
        flag(input.enhanced_image_metadata_enabled),
    );
    fields.insert("tags", tag_map(input.tags.as_ref()));
    fields.insert("imageTags", tag_map(input.image_tags.as_ref()));
    fields.insert(
        "imageTestsConfiguration",
        image_tests_fingerprint(input.image_tests_configuration.as_ref()),
    );
    fields.insert(
        "imageScanningConfiguration",
        image_scanning_fingerprint(input.image_scanning_configuration.as_ref()),
    );
    fields.insert(
        "schedule",
        input.schedule.as_ref().map_or_else(
            || ABSENT.to_string(),
            |schedule| {
                group(&[
                    text(schedule.schedule_expression.as_deref()),
                    text(schedule.timezone.as_deref()),
                    text(schedule.pipeline_execution_start_condition.as_deref()),
                    schedule.auto_disable_policy.as_ref().map_or_else(
                        || ABSENT.to_string(),
                        |policy| number(Some(policy.failure_count)),
                    ),
                ])
            },
        ),
    );
    fields.insert(
        "loggingConfiguration",
        input.logging_configuration.as_ref().map_or_else(
            || ABSENT.to_string(),
            |logging| {
                group(&[
                    text(logging.image_log_group_name.as_deref()),
                    text(logging.pipeline_log_group_name.as_deref()),
                ])
            },
        ),
    );
    fields.insert("workflows", workflows_fingerprint(input.workflows.as_deref()));
    fields
        .iter()
        .map(|(member, rendered)| format!("{member}={rendered};"))
        .collect()
}

/// A deterministic rendering of the creation parameters one `CreateImage` request
/// carries: everything the build it queues is defined by. See
/// [`infrastructure_configuration_request_fingerprint`] for what a fingerprint leaves out
/// and why.
#[must_use]
pub fn create_image_request_fingerprint(input: &CreateImageInput) -> String {
    let mut fields: BTreeMap<&'static str, String> = BTreeMap::new();
    fields.insert("imageRecipeArn", text(input.image_recipe_arn.as_deref()));
    fields.insert(
        "containerRecipeArn",
        text(input.container_recipe_arn.as_deref()),
    );
    fields.insert(
        "infrastructureConfigurationArn",
        text(Some(input.infrastructure_configuration_arn.as_str())),
    );
    fields.insert(
        "distributionConfigurationArn",
        text(input.distribution_configuration_arn.as_deref()),
    );
    fields.insert("executionRole", text(input.execution_role.as_deref()));
    fields.insert(
        "enhancedImageMetadataEnabled",
        flag(input.enhanced_image_metadata_enabled),
    );
    fields.insert("tags", tag_map(input.tags.as_ref()));
    fields.insert(
        "imageTestsConfiguration",
        image_tests_fingerprint(input.image_tests_configuration.as_ref()),
    );
    fields.insert(
        "imageScanningConfiguration",
        image_scanning_fingerprint(input.image_scanning_configuration.as_ref()),
    );
    fields.insert(
        "loggingConfiguration",
        input.logging_configuration.as_ref().map_or_else(
            || ABSENT.to_string(),
            |logging| group(&[text(logging.log_group_name.as_deref())]),
        ),
    );
    fields.insert("workflows", workflows_fingerprint(input.workflows.as_deref()));
    fields
        .iter()
        .map(|(member, rendered)| format!("{member}={rendered};"))
        .collect()
}

/// A deterministic rendering of the parameters one `StartImagePipelineExecution` request
/// carries beside its `clientToken`: the pipeline it starts and the tags the image it
/// creates is to carry. See [`infrastructure_configuration_request_fingerprint`].
#[must_use]
pub fn pipeline_execution_request_fingerprint(input: &StartImagePipelineExecutionInput) -> String {
    let mut fields: BTreeMap<&'static str, String> = BTreeMap::new();
    fields.insert(
        "imagePipelineArn",
        text(Some(input.image_pipeline_arn.as_str())),
    );
    fields.insert("tags", tag_map(input.tags.as_ref()));
    fields
        .iter()
        .map(|(member, rendered)| format!("{member}={rendered};"))
        .collect()
}

/// One string member, length-prefixed so no two different values render alike.
fn text(value: Option<&str>) -> String {
    value.map_or_else(
        || ABSENT.to_string(),
        |value| format!("{}:{value}", value.len()),
    )
}

/// One optional boolean member.
fn flag(value: Option<bool>) -> String {
    value.map_or_else(|| ABSENT.to_string(), |value| value.to_string())
}

/// One optional integer member.
fn number(value: Option<i32>) -> String {
    value.map_or_else(|| ABSENT.to_string(), |value| value.to_string())
}

/// One list member, in the order the request carried it — a list's order is part of
/// its value, so two orders are two different requests.
fn list(values: Option<&[String]>) -> String {
    values.map_or_else(
        || ABSENT.to_string(),
        |values| {
            group(
                &values
                    .iter()
                    .map(|value| text(Some(value)))
                    .collect::<Vec<_>>(),
            )
        },
    )
}

/// The `components` member of a recipe create, as every recipe's fingerprint renders it: each
/// configuration's reference beside the parameter settings it carries, in the order the
/// request listed them.
///
/// One rendering for both kinds of recipe, since both carry the same modelled
/// `ComponentConfigurationList`.
fn component_configurations_fingerprint(
    configurations: Option<&[ComponentConfiguration]>,
) -> String {
    configurations.map_or_else(
        || ABSENT.to_string(),
        |configurations| {
            group(
                &configurations
                    .iter()
                    .map(|configuration| {
                        group(&[
                            text(Some(configuration.component_arn.as_str())),
                            configuration.parameters.as_ref().map_or_else(
                                || ABSENT.to_string(),
                                |parameters| {
                                    group(
                                        &parameters
                                            .iter()
                                            .map(|parameter| {
                                                group(&[
                                                    text(Some(parameter.name.as_str())),
                                                    list(Some(parameter.value.as_slice())),
                                                ])
                                            })
                                            .collect::<Vec<_>>(),
                                    )
                                },
                            ),
                        ])
                    })
                    .collect::<Vec<_>>(),
            )
        },
    )
}

/// The block device mappings one request named, as a fingerprint renders them.
///
/// One rendering for every member that carries the modelled `InstanceBlockDeviceMappings`:
/// an image recipe's own `blockDeviceMappings`, and the ones inside a container recipe's
/// `instanceConfiguration`.
fn block_device_mappings_fingerprint(mappings: Option<&[InstanceBlockDeviceMapping]>) -> String {
    mappings.map_or_else(
        || ABSENT.to_string(),
        |mappings| {
            group(
                &mappings
                    .iter()
                    .map(|mapping| {
                        group(&[
                            text(mapping.device_name.as_deref()),
                            text(mapping.virtual_name.as_deref()),
                            text(mapping.no_device.as_deref()),
                            mapping.ebs.as_ref().map_or_else(
                                || ABSENT.to_string(),
                                |ebs| {
                                    group(&[
                                        flag(ebs.encrypted),
                                        flag(ebs.delete_on_termination),
                                        number(ebs.iops),
                                        number(ebs.volume_size),
                                        number(ebs.throughput),
                                        text(ebs.kms_key_id.as_deref()),
                                        text(ebs.snapshot_id.as_deref()),
                                        text(ebs.volume_type.as_deref()),
                                    ])
                                },
                            ),
                        ])
                    })
                    .collect::<Vec<_>>(),
            )
        },
    )
}

/// The `filters` member of a list request, as every listing's fingerprint renders it: a
/// filter's name beside its values, in the order the request carried them.
fn filter_list(filters: Option<&[Filter]>) -> String {
    filters.map_or_else(
        || ABSENT.to_string(),
        |filters| {
            group(
                &filters
                    .iter()
                    .map(|filter| {
                        group(&[
                            text(filter.name.as_deref()),
                            list(filter.values.as_deref()),
                        ])
                    })
                    .collect::<Vec<_>>(),
            )
        },
    )
}

/// One map member, in key order — a map carries no order of its own, so the rendering
/// imposes one and two orderings of the same pairs render alike.
fn tag_map(values: Option<&HashMap<String, String>>) -> String {
    values.map_or_else(
        || ABSENT.to_string(),
        |values| {
            let ordered: BTreeMap<&String, &String> = values.iter().collect();
            group(
                &ordered
                    .into_iter()
                    .map(|(key, value)| {
                        format!("{}={}", text(Some(key)), text(Some(value)))
                    })
                    .collect::<Vec<_>>(),
            )
        },
    )
}

/// The `distributions` member, as a distribution configuration's fingerprint renders it:
/// every entry in the order the request carried it, down to each setting it holds — a
/// list's order is part of its value, so two orders are two different requests.
fn distributions_fingerprint(distributions: &[Distribution]) -> String {
    group(
        &distributions
            .iter()
            .map(|distribution| {
                group(&[
                    text(Some(distribution.region.as_str())),
                    distribution
                        .ami_distribution_configuration
                        .as_ref()
                        .map_or_else(|| ABSENT.to_string(), ami_distribution_fingerprint),
                    distribution
                        .container_distribution_configuration
                        .as_ref()
                        .map_or_else(|| ABSENT.to_string(), |container| {
                            group(&[
                                text(container.description.as_deref()),
                                list(container.container_tags.as_deref()),
                                text(Some(container.target_repository.service.as_str())),
                                text(Some(
                                    container.target_repository.repository_name.as_str(),
                                )),
                            ])
                        }),
                    list(distribution.license_configuration_arns.as_deref()),
                    distribution
                        .launch_template_configurations
                        .as_ref()
                        .map_or_else(
                            || ABSENT.to_string(),
                            |configurations| {
                                group(
                                    &configurations
                                        .iter()
                                        .map(|configuration| {
                                            group(&[
                                                text(Some(
                                                    configuration.launch_template_id.as_str(),
                                                )),
                                                text(configuration.account_id.as_deref()),
                                                flag(Some(configuration.set_default_version)),
                                            ])
                                        })
                                        .collect::<Vec<_>>(),
                                )
                            },
                        ),
                    distribution.s3_export_configuration.as_ref().map_or_else(
                        || ABSENT.to_string(),
                        |export| {
                            group(&[
                                text(Some(export.role_name.as_str())),
                                text(Some(export.disk_image_format.as_str())),
                                text(Some(export.s3_bucket.as_str())),
                                text(export.s3_prefix.as_deref()),
                            ])
                        },
                    ),
                    distribution
                        .fast_launch_configurations
                        .as_ref()
                        .map_or_else(
                            || ABSENT.to_string(),
                            |configurations| {
                                group(
                                    &configurations
                                        .iter()
                                        .map(fast_launch_fingerprint)
                                        .collect::<Vec<_>>(),
                                )
                            },
                        ),
                    distribution
                        .ssm_parameter_configurations
                        .as_ref()
                        .map_or_else(
                            || ABSENT.to_string(),
                            |configurations| {
                                group(
                                    &configurations
                                        .iter()
                                        .map(|configuration| {
                                            group(&[
                                                text(Some(
                                                    configuration.parameter_name.as_str(),
                                                )),
                                                text(configuration.ami_account_id.as_deref()),
                                                text(configuration.data_type.as_deref()),
                                            ])
                                        })
                                        .collect::<Vec<_>>(),
                                )
                            },
                        ),
                ])
            })
            .collect::<Vec<_>>(),
    )
}

/// One entry's `amiDistributionConfiguration`, as [`distributions_fingerprint`] renders
/// it.
fn ami_distribution_fingerprint(ami: &AmiDistributionConfiguration) -> String {
    group(&[
        text(ami.name.as_deref()),
        text(ami.description.as_deref()),
        text(ami.kms_key_id.as_deref()),
        list(ami.target_account_ids.as_deref()),
        tag_map(ami.ami_tags.as_ref()),
        ami.launch_permission.as_ref().map_or_else(
            || ABSENT.to_string(),
            |permission| {
                group(&[
                    list(permission.user_ids.as_deref()),
                    list(permission.user_groups.as_deref()),
                    list(permission.organization_arns.as_deref()),
                    list(permission.organizational_unit_arns.as_deref()),
                ])
            },
        ),
    ])
}

/// One entry's `fastLaunchConfigurations` element, as [`distributions_fingerprint`]
/// renders it.
fn fast_launch_fingerprint(configuration: &FastLaunchConfiguration) -> String {
    group(&[
        flag(Some(configuration.enabled)),
        number(configuration.max_parallel_launches),
        text(configuration.account_id.as_deref()),
        configuration.snapshot_configuration.as_ref().map_or_else(
            || ABSENT.to_string(),
            |snapshot| number(snapshot.target_resource_count),
        ),
        configuration.launch_template.as_ref().map_or_else(
            || ABSENT.to_string(),
            |template| {
                group(&[
                    text(template.launch_template_id.as_deref()),
                    text(template.launch_template_name.as_deref()),
                    text(template.launch_template_version.as_deref()),
                ])
            },
        ),
    ])
}

/// The `imageTestsConfiguration` member, as every fingerprint that carries one renders
/// it: the two settings a build's tests are configured by.
fn image_tests_fingerprint(configuration: Option<&ImageTestsConfiguration>) -> String {
    configuration.map_or_else(
        || ABSENT.to_string(),
        |configuration| {
            group(&[
                flag(configuration.image_tests_enabled),
                number(configuration.timeout_minutes),
            ])
        },
    )
}

/// The `imageScanningConfiguration` member, as every fingerprint that carries one renders
/// it.
fn image_scanning_fingerprint(configuration: Option<&ImageScanningConfiguration>) -> String {
    configuration.map_or_else(
        || ABSENT.to_string(),
        |configuration| {
            group(&[
                flag(configuration.image_scanning_enabled),
                configuration.ecr_configuration.as_ref().map_or_else(
                    || ABSENT.to_string(),
                    |ecr| {
                        group(&[
                            text(ecr.repository_name.as_deref()),
                            list(ecr.container_tags.as_deref()),
                        ])
                    },
                ),
            ])
        },
    )
}

/// The `workflows` member, as every fingerprint that carries one renders it: each
/// configuration in the order the request carried it, down to its parameters.
fn workflows_fingerprint(workflows: Option<&[WorkflowConfiguration]>) -> String {
    workflows.map_or_else(
        || ABSENT.to_string(),
        |workflows| {
            group(
                &workflows
                    .iter()
                    .map(|configuration| {
                        group(&[
                            text(Some(configuration.workflow_arn.as_str())),
                            text(configuration.parallel_group.as_deref()),
                            text(configuration.on_failure.as_deref()),
                            configuration.parameters.as_ref().map_or_else(
                                || ABSENT.to_string(),
                                |parameters| {
                                    group(
                                        &parameters
                                            .iter()
                                            .map(|parameter| {
                                                group(&[
                                                    text(Some(parameter.name.as_str())),
                                                    list(Some(parameter.value.as_slice())),
                                                ])
                                            })
                                            .collect::<Vec<_>>(),
                                    )
                                },
                            ),
                        ])
                    })
                    .collect::<Vec<_>>(),
            )
        },
    )
}

/// Several already-rendered parts as one member's value.
fn group(parts: &[String]) -> String {
    format!("[{}]", parts.join(","))
}

// ---------------------------------------------------------------------------
// How this service words a refusal
//
// Each wording lives here once, so two operations state the same failure the same way.
// ---------------------------------------------------------------------------

/// Why a request naming a subnet but no security group is refused.
///
/// The rule is the `subnetId` member's own: "If you specify `subnetId`, you must also
/// specify one or more security group IDs in `securityGroupIds`. Otherwise, the
/// request fails." The wording names the two members and the requirement, and
/// withholds the values the request carried.
#[must_use]
pub fn subnet_without_security_group_message() -> String {
    "The request must specify one or more securityGroupIds when it specifies a subnetId."
        .to_string()
}

/// Why a request assigning a reserved metadata-tag key is refused.
#[must_use]
pub fn reserved_resource_tag_key_message(key: &str) -> String {
    format!("The resourceTags key {key} is reserved and can't be assigned.")
}

/// Why a create of a resource this Region already holds is refused.
///
/// Names the resource LABEL AWS gives the kind the create would have minted and the ARN of
/// the resource that already carries it, read from this service's own store — the identity a
/// caller needs in order to take the already-exists path, and nothing else about that
/// resource. The label is the kind the ARN itself names ([`already_exists_label`]), so one
/// wording serves every create whose recorded refusal states the ARN.
#[must_use]
pub fn resource_already_exists_message(arn: &str) -> String {
    let resource_label = already_exists_label(arn);
    format!("The following resource '{resource_label}' already exists: '{arn}'.")
}

/// The resource LABEL an already-exists refusal names a resource kind by.
///
/// Mostly the kind the ARN itself names, in the upper camel case this model's shape names use
/// ([`arn_resource_type`]). One kind is labelled differently from its ARN: an
/// `image-pipeline` is labelled `Pipeline`, which is what a recorded refusal of
/// `CreateImagePipeline` carries.
#[must_use]
fn already_exists_label(arn: &str) -> String {
    match ::service_common::arn::resource_type(arn) {
        Some(IMAGE_PIPELINE_RESOURCE_TYPE) => IMAGE_PIPELINE_RESOURCE_LABEL.to_string(),
        _ => arn_resource_type(arn),
    }
}

/// The resource type an image-pipeline ARN carries, as the resource field spells it
/// ([`IMAGE_PIPELINE_ARN_PREFIX`] without its separator).
const IMAGE_PIPELINE_RESOURCE_TYPE: &str = "image-pipeline";

/// The label a refusal gives that kind, which is not the kind its ARN names.
const IMAGE_PIPELINE_RESOURCE_LABEL: &str = "Pipeline";

/// Why a create whose infrastructure-configuration name is already taken in this Region is
/// refused.
///
/// The one create of this service whose recorded already-exists refusal states no ARN and
/// names no resource: the whole sentence is fixed text, so nothing of the request or of the
/// existing configuration appears in it.
#[must_use]
pub fn infrastructure_configuration_exists_message() -> String {
    "The resource already exists.".to_string()
}

/// Why a create whose distributions name one Region twice is refused.
///
/// The rule is the `distributions` member's own: "Each entry defines how output images are
/// distributed in one target Amazon Web Services Region. A Region can appear at most once
/// in the list." The wording names the member, the requirement and the Region the request
/// repeated.
#[must_use]
pub fn duplicate_distribution_region_message(region: &str) -> String {
    format!(
        "The distributions list can name each target Region at most once, and it names \
         {region} more than once."
    )
}

/// Why a create was not performed because the request asked only for validation.
///
/// From the `dryRun` member's own documentation: "Validates the required permissions
/// and request parameters without performing the operation. If validation succeeds,
/// the operation returns a DryRunOperationException error response." The sentence is
/// FIXED text and names the member as the API spells it, `dryRun`.
#[must_use]
pub fn dry_run_message() -> String {
    "Request would have succeeded, but dryRun flag is set.".to_string()
}

/// Why the same `clientToken` carrying different creation parameters is refused.
///
/// The modelled `IdempotentParameterMismatchException` is "You have specified a client token
/// for an operation using parameter values that differ from a previous request that used the
/// same client token", and the sentence this service states it in is FIXED text: it names
/// neither the token nor the parameters that differ, so one wording serves every create of
/// this service.
#[must_use]
pub fn idempotent_parameter_mismatch_message() -> String {
    "The request parameters do not match those in the original idempotent request.".to_string()
}

/// Why a create is refused once the configuration store is full.
#[must_use]
pub fn infrastructure_configuration_store_full_message() -> String {
    format!(
        "This account already holds the maximum of {MAX_INFRASTRUCTURE_CONFIGURATIONS} \
         infrastructure configurations."
    )
}

/// Why a create is refused once the distribution-configuration store is full.
#[must_use]
pub fn distribution_configuration_store_full_message() -> String {
    format!(
        "This account already holds the maximum of {MAX_DISTRIBUTION_CONFIGURATIONS} \
         distribution configurations."
    )
}

/// Why a create is refused once the component store is full.
#[must_use]
pub fn component_store_full_message() -> String {
    format!("This account already holds the maximum of {MAX_COMPONENTS} components.")
}

/// Why a create is refused once the image-recipe store is full.
#[must_use]
pub fn image_recipe_store_full_message() -> String {
    format!("This account already holds the maximum of {MAX_IMAGE_RECIPES} image recipes.")
}

/// Why a create is refused once the image-pipeline store is full.
#[must_use]
pub fn image_pipeline_store_full_message() -> String {
    format!("This account already holds the maximum of {MAX_IMAGE_PIPELINES} image pipelines.")
}

/// Why a build is refused once the image store is full.
#[must_use]
pub fn image_store_full_message() -> String {
    format!("This account already holds the maximum of {MAX_IMAGES} image build versions.")
}

/// Why an operation could not reach this service's store at all.
#[must_use]
pub fn store_unavailable_message() -> String {
    "The Image Builder resource store is unavailable.".to_string()
}

/// The member a semantic version arrives in, as the API addresses it — the parameter a
/// refusal of such a value names.
pub const SEMANTIC_VERSION_MEMBER: &str = "semanticVersion";

/// The requirement clause a semantic version this service cannot accept is refused with.
///
/// The rule is `semanticVersion`'s own: the three assignable nodes take "any positive
/// integer value, including zero. The upper limit is 2^30-1, or 1073741823, for each node",
/// and the fourth node is Image Builder's to assign — a bound no `@pattern` can state, so a
/// value that satisfies the modelled pattern and misses the bound is a value of that
/// parameter this service cannot accept. The clause states the FORM the value had to follow
/// and echoes none of what the request carried.
const SEMANTIC_VERSION_REQUIREMENT: &str =
    "The supplied semantic version does not follow the required format.";

/// The additional clause stated for a member whose own modelled pattern admits the `x`
/// wildcard node, since how many of them one version may carry is part of the format such a
/// value has to follow.
const ONE_WILDCARD_PER_VERSION_REQUIREMENT: &str =
    "Only one 'x' wildcard is allowed per version.";

/// Why a create naming a semantic version outside the documented bound is refused, for a
/// member whose modelled pattern admits assigned nodes only
/// (`CreateComponentRequest.semanticVersion`, `^[0-9]+\.[0-9]+\.[0-9]+$`).
///
/// Stated in the sentence this service states every refused parameter in
/// (`service_common::validation_message::parameter_value_mismatch`), whose only variable
/// part is the member the value arrived in. See [`SEMANTIC_VERSION_REQUIREMENT`].
#[must_use]
pub fn invalid_semantic_version_message(member: &str) -> String {
    ::service_common::validation_message::parameter_value_mismatch(
        member,
        SEMANTIC_VERSION_REQUIREMENT,
    )
}

/// [`invalid_semantic_version_message`] for a member whose modelled pattern also admits the
/// `x` wildcard node (`CreateImageRecipeRequest.semanticVersion`,
/// `^(?:[0-9]+|x)\.(?:[0-9]+|x)\.(?:[0-9]+|x)$`): the requirement states the wildcard rule
/// too.
#[must_use]
pub fn invalid_wildcard_semantic_version_message(member: &str) -> String {
    ::service_common::validation_message::parameter_value_mismatch(
        member,
        &format!("{SEMANTIC_VERSION_REQUIREMENT} {ONE_WILDCARD_PER_VERSION_REQUIREMENT}"),
    )
}

/// The member an instance profile name arrives in, as the API addresses it.
pub const INSTANCE_PROFILE_NAME_MEMBER: &str = "instanceProfileName";

/// Why a create naming an instance profile this account does not hold is refused.
///
/// `instanceProfileName` is "the instance profile to associate with the instance used to
/// customize your Amazon EC2 AMI" — an IAM resource of the caller's account, which the model
/// can only constrain the SHAPE of, so whether it exists is resolved against IAM itself
/// ([`instance_profile_named`]). The wording names the member the value arrived in and
/// states that no such profile exists; nothing of the value is echoed.
#[must_use]
pub fn missing_instance_profile_message(member: &str) -> String {
    format!(
        "Invalid parameter value for {member}: The provided instance profile does not exist. \
         Please specify a different instance profile and try again."
    )
}

/// Why a `CreateComponent` naming neither a document nor a URI, or both, is refused.
///
/// The rule is the operation's own: the component "is based on a YAML document that you
/// specify using exactly one of the following methods: Inline, using the data property in
/// the request body. A URL that points to a YAML document file stored in Amazon S3, using
/// the uri property in the request body", and `data` adds "you cannot specify both
/// properties".
///
/// A requirement over the two members TOGETHER rather than a constraint on either value, so
/// it is stated in the sentence this service states every refused parameter in
/// (`service_common::validation_message::parameters_value_mismatch`), naming both members:
/// neither one's value is wrong on its own.
#[must_use]
pub fn component_data_or_uri_message() -> String {
    ::service_common::validation_message::parameters_value_mismatch(
        &[COMPONENT_DATA_MEMBER, COMPONENT_URI_MEMBER],
        "You must specify either data or a uri.",
    )
}

/// The member a component document rides in inline, as the API spells it.
pub const COMPONENT_DATA_MEMBER: &str = "data";

/// The member a component document's location rides in, as the API spells it.
pub const COMPONENT_URI_MEMBER: &str = "uri";

/// Why a `CreateComponent` naming a URI that is not an S3 URL is refused.
///
/// From the `uri` member's own documentation: "This must be an S3 URL (s3://bucket/key)" —
/// a requirement on that member's VALUE, so it is stated as this service states every
/// parameter whose value it refuses.
#[must_use]
pub fn component_uri_message() -> String {
    ::service_common::validation_message::parameter_value_mismatch(
        COMPONENT_URI_MEMBER,
        "uri must be an S3 URL (s3://bucket/key)",
    )
}

/// Why a `CreateComponent` naming a URI the object store serves no object at is refused.
///
/// Image Builder fetches the document itself at create time, so a `uri` naming no object is
/// a value of that member this service cannot accept, stated in the same sentence as every
/// other refused parameter with the clause this class carries.
#[must_use]
pub fn component_uri_not_found_message() -> String {
    ::service_common::validation_message::parameter_value_mismatch(
        COMPONENT_URI_MEMBER,
        URI_NOT_FOUND_REQUIREMENT,
    )
}

/// The requirement clause a `uri` the object store serves no object at is refused with.
///
/// One clause for every member of this service that names an object the create READS, since
/// what each of them establishes is the same: the store serves nothing at the address the
/// value carried.
const URI_NOT_FOUND_REQUIREMENT: &str = "Not Found";

/// Why a create whose document or template this service could not read at all is answered
/// with a server-side failure.
///
/// Nothing was established about the object the request named — no object store answered the
/// read — so this is not a claim about the value the request carried, and the message is the
/// one the modelled `ServiceException` states for its own class: "An internal server error
/// occurred while Image Builder processed the request. Retrying the request may succeed."
#[must_use]
pub fn internal_server_error_message() -> String {
    "An internal server error occurred while Image Builder processed the request. Retrying \
     the request may succeed."
        .to_string()
}

/// Why a `CreateComponent` whose document this service cannot accept is refused.
///
/// `member` is the member the document ARRIVED in — `data` for an inline document, `uri` for
/// one fetched from the object store — and `why` is the clause [`parse_component_document`]
/// states for it: the document is the VALUE that member carried, so a document this service
/// cannot accept is refused as that parameter's value.
#[must_use]
pub fn component_document_message(member: &str, why: &str) -> String {
    ::service_common::validation_message::parameter_value_mismatch(member, why)
}

/// Why a delete of a resource another stored resource still references is refused.
///
/// Names the ARN the request asked to delete and the fact that something else depends on
/// it, and nothing about the resources that do: what the caller has to act on is that the
/// delete cannot proceed while a reference stands.
#[must_use]
pub fn resource_dependency_message(arn: &str) -> String {
    format!("Resource dependency error: The resource ARN '{arn}' has other resources depended on it.")
}

/// Why a create whose components do not agree on one platform is refused.
///
/// A recipe builds for ONE platform, which is the platform of the components it applies,
/// so a list spanning two of them names no platform the recipe could report. One wording for
/// both kinds of recipe, which share the rule.
#[must_use]
pub fn mixed_component_platforms_message(first: &str, second: &str) -> String {
    format!(
        "The components of a recipe must all target one platform; this request names both \
         {first} and {second}."
    )
}

/// Why a create naming more components than one recipe may include is refused.
///
/// The bound is the quota the modelled `components` member documents: "A recipe can
/// contain a maximum of 20 build and test components in any combination, by default. This
/// maximum is an adjustable quota." The member of a container recipe documents the same
/// quota, so one wording serves both kinds of recipe.
#[must_use]
pub fn too_many_recipe_components_message(submitted: usize) -> String {
    format!(
        "A recipe can contain a maximum of {MAX_RECIPE_COMPONENTS} components; this request \
         names {submitted}."
    )
}

/// Why a create naming one component twice is refused.
///
/// The rule is `ComponentConfiguration`'s own: "You can specify each component only once
/// in a recipe, regardless of version."
#[must_use]
pub fn duplicate_recipe_component_message(name: &str) -> String {
    format!(
        "The component {name} is named more than once; a recipe can specify each component \
         only once, regardless of version."
    )
}


/// Why a create naming neither recipe, or both, is refused.
///
/// The rule is the one `CreateImagePipeline` and `CreateImage` each state for themselves:
/// "You must specify exactly one recipe" "using either a containerRecipeArn or an
/// imageRecipeArn", which each of the two members repeats as "You must specify either this
/// property or … but not both".
#[must_use]
pub fn exactly_one_recipe_message() -> String {
    "The request must specify exactly one of imageRecipeArn and containerRecipeArn.".to_string()
}

/// Why a create naming workflows but no execution role is refused.
///
/// The rule is the `workflows` member's own: "You must also specify executionRole when you
/// provide workflows."
#[must_use]
pub fn workflows_without_execution_role_message() -> String {
    "The request must specify an executionRole when it specifies workflows.".to_string()
}

/// Why a tagging call that would leave a resource holding more tags than one resource may
/// carry is refused.
///
/// The bound is the modelled `TagMap`'s own ([`MAX_RESOURCE_TAGS`]); the wording names it
/// and how many tags the merged map would have held.
#[must_use]
pub fn too_many_resource_tags_message(arn: &str, merged: usize) -> String {
    format!(
        "The resource {arn} can carry a maximum of {MAX_RESOURCE_TAGS} tags; this request would \
         leave it with {merged}."
    )
}

/// Why a list naming a filter it does not filter on is refused.
///
/// A filter names the member it selects on, so a name the operation does not filter on is a
/// value its `filters` parameter does not admit: the refusal is this service's own
/// parameter-value sentence ([`crate::service::parameter_refusal`]) about that member, and
/// its requirement states the filters the operation ALLOWS — never the name the request
/// carried, which is the value and is withheld like every other refused value.
///
/// The `supported` names are the `filters` member's own list, of the operation the request
/// called ([`IMAGE_RECIPE_FILTER_NAMES`], [`COMPONENT_FILTER_NAMES`]) — each operation
/// filters on the members of the resource it lists, so the wording names the ones the
/// caller may ask for rather than any one operation's — and `operation` is that operation,
/// which the sentence names because the set belongs to it.
#[must_use]
pub fn unsupported_filter_message(operation: &str, supported: &[&str]) -> String {
    ::service_common::validation_message::parameter_value_mismatch(
        FILTERS_MEMBER,
        &format!(
            "Allowed filters for {operation} are: {}.",
            supported.join(", ")
        ),
    )
}

/// Why a `ListComponents` request carrying both `byName` and the version filter is
/// refused.
///
/// The rule is the `byName` member's own: "You can't combine this option with the version
/// filter."
#[must_use]
pub fn by_name_with_version_filter_message() -> String {
    format!(
        "The request must not specify byName together with the \
         {COMPONENT_VERSION_FILTER_NAME} filter."
    )
}

/// The member every listing of this service carries its filters in, as the API addresses
/// it — the parameter a refusal about one of them names.
pub const FILTERS_MEMBER: &str = "filters";

/// Why a list naming a filter with no name or no values is refused.
#[must_use]
pub fn incomplete_filter_message() -> String {
    "Each filter must specify a name and one or more values.".to_string()
}

/// Why a call naming a resource this Region holds nothing under reports it missing.
///
/// Names the resource TYPE the request needed and the ARN it named, and nothing about any
/// resource that does exist. The type token is the kind the ARN itself names
/// ([`arn_resource_type`]), so one wording serves every lookup: the resource an operation
/// acts on, and a resource a create has to resolve a reference to.
#[must_use]
pub fn required_resource_not_found_message(arn: &str) -> String {
    let resource_type = arn_resource_type(arn);
    format!("The following required resource '{resource_type}' cannot be found: '{arn}'.")
}

/// The resource-type token a missing-resource report names the resource `arn` identifies
/// by: the name this model gives that kind of resource.
///
/// An Image Builder ARN carries its kind as the resource TYPE of its resource field, read
/// through this tree's one ARN parse (`service_common::arn`), in lower kebab case. The
/// kinds an ARN may name are the alternation of the modelled `ImageBuilderArn` pattern
/// (`smithy_query(query="shape",shape="ImageBuilderArn")`): `component`, `image`,
/// `image-recipe`, `container-recipe`, `image-pipeline`, `infrastructure-configuration`,
/// `distribution-configuration`, `lifecycle-policy` and `workflow`. This model names each
/// of them by a shape whose name is that same text in upper camel case (`component` is
/// `Component`, `image-recipe` is `ImageRecipe`), which is the token derived here — so a
/// kind added to that alternation needs no table kept in step. A value that is not a
/// resource ARN at all names no kind, and is reported by itself.
#[must_use]
pub fn arn_resource_type(arn: &str) -> String {
    let kind = ::service_common::arn::resource_type(arn).unwrap_or(arn);
    kind.split('-')
        .map(|word| {
            let mut characters = word.chars();
            match characters.next() {
                Some(first) => first.to_uppercase().chain(characters).collect::<String>(),
                None => String::new(),
            }
        })
        .collect()
}

/// Why a policy document that is not parseable JSON is refused.
///
/// A resource policy is a JSON document, so a value that cannot be parsed as one is an
/// invalid value for the member it arrived in — which the wording names.
#[must_use]
pub fn malformed_policy_message(member: &str) -> String {
    format!(
        "The value supplied for parameter '{member}' is not valid. Policy is malformed and cannot \
         be parsed"
    )
}

/// The member every paginated listing of this service carries its continuation token in, as
/// the API addresses it — the parameter a refusal about a token names.
pub const NEXT_TOKEN_MEMBER: &str = "nextToken";

/// Why a list carrying a token this service did not issue for it is refused.
///
/// A continuation token is OPAQUE: the only value the parameter admits is one a previous
/// page of that same listing handed back, so anything else is a value `nextToken` does not
/// admit — this service's parameter-value sentence
/// ([`crate::service::parameter_refusal`]) about that member, whose requirement states that
/// the token is not of the form the service issues. The token itself is never echoed: it is
/// the refused value.
#[must_use]
pub fn invalid_pagination_token_message() -> String {
    ::service_common::validation_message::parameter_value_mismatch(
        NEXT_TOKEN_MEMBER,
        "The provided token does not meet the required format.",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_differing_only_in_case_spaces_or_underscores_normalize_alike() {
        let cases: &[(&str, &str)] = &[
            ("Build-Infra", "build-infra"),
            ("build infra", "build-infra"),
            ("build_infra", "build-infra"),
            ("BUILD INFRA", "build-infra"),
            ("build-infra", "build-infra"),
            ("Prod_Linux Build-01", "prod-linux-build-01"),
        ];
        for (name, expected) in cases {
            let actual = normalized_name(name);
            assert_eq!(
                &actual, expected,
                "case {name:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }

    #[test]
    fn a_minted_request_id_is_uuid_shaped_and_fresh() {
        let mut seen: Vec<String> = Vec::new();
        for _ in 0..16 {
            let id = new_request_id();
            let groups: Vec<usize> = id.split('-').map(str::len).collect();
            assert_eq!(
                groups,
                vec![8, 4, 4, 4, 12],
                "case new_request_id: expected 8-4-4-4-12 groups, got {id:?}"
            );
            assert!(
                id.chars().all(|c| c == '-' || c.is_ascii_hexdigit()),
                "case new_request_id: expected lowercase hex, got {id:?}"
            );
            assert!(
                !seen.contains(&id),
                "case new_request_id: expected a fresh id, got the already-minted {id:?}"
            );
            seen.push(id);
        }
    }

    #[test]
    fn a_semantic_version_is_read_only_inside_the_documented_bound() {
        // Each row is a version and the three nodes it assigns, or `None` for one the
        // documented upper limit (2^30-1) or the three-node syntax refuses.
        let cases: &[(&str, Option<[u64; ASSIGNED_VERSION_NODES]>)] = &[
            ("1.0.0", Some([1, 0, 0])),
            ("0.0.0", Some([0, 0, 0])),
            ("2021.1.1", Some([2021, 1, 1])),
            ("1073741823.1073741823.1073741823", Some([
                MAX_VERSION_NODE,
                MAX_VERSION_NODE,
                MAX_VERSION_NODE,
            ])),
            ("1073741824.0.0", None),
            ("1.0", None),
            ("1.0.0.0", None),
            ("1.x.x", None),
        ];
        for (version, expected) in cases {
            let actual = parse_semantic_version(version);
            assert_eq!(
                &actual, expected,
                "case {version:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }

    #[test]
    fn a_component_reference_reads_build_and_wildcard_version_arns() {
        let build = component_build_version_arn("us-west-2", "Demo Component", "1.2.3", 4);
        let reference = ComponentReference::parse(&build)
            .unwrap_or_else(|| panic!("case {build:?}: expected a component reference"));
        assert_eq!(
            reference,
            ComponentReference {
                region: "us-west-2".to_string(),
                name: "demo-component".to_string(),
                version: [
                    VersionNode::Exact(1),
                    VersionNode::Exact(2),
                    VersionNode::Exact(3)
                ],
                build_version: Some(4),
            },
            "case {build:?}: expected the ARN's own fields, got {reference:?}"
        );

        // A wildcard is admitted in TRAILING positions only, and a build version ARN
        // names an exact version.
        let cases: &[(&str, bool)] = &[
            ("arn:aws:imagebuilder:us-west-2:000000000000:component/demo/1.2.3", true),
            ("arn:aws:imagebuilder:us-west-2:000000000000:component/demo/1.2.x", true),
            ("arn:aws:imagebuilder:us-west-2:000000000000:component/demo/1.x.x", true),
            ("arn:aws:imagebuilder:us-west-2:000000000000:component/demo/x.x.x", true),
            ("arn:aws:imagebuilder:us-west-2:000000000000:component/demo/x.2.3", false),
            ("arn:aws:imagebuilder:us-west-2:000000000000:component/demo/1.x.3", false),
            ("arn:aws:imagebuilder:us-west-2:000000000000:component/demo/1.x.x/1", false),
            ("arn:aws:imagebuilder:us-west-2:000000000000:image-recipe/demo/1.2.3", false),
            ("arn:aws:ec2:us-west-2:000000000000:component/demo/1.2.3", false),
            ("not-an-arn", false),
        ];
        for (reference, readable) in cases {
            let actual = ComponentReference::parse(reference).is_some();
            assert_eq!(
                actual, *readable,
                "case {reference:?}: expected readable={readable}, got {actual}"
            );
        }
    }

    #[test]
    fn a_component_document_declares_its_type_and_parameters() {
        // One phase named `test` is the only document that is a TEST component; the rows
        // vary the phase count, the phase name and the order.
        let cases: &[(&str, &str)] = &[
            ("name: t\nphases:\n  - name: test\n    steps: []\n", COMPONENT_TYPE_TEST),
            ("name: b\nphases:\n  - name: build\n    steps: []\n", COMPONENT_TYPE_BUILD),
            (
                "name: bt\nphases:\n  - name: build\n    steps: []\n  - name: test\n    steps: []\n",
                COMPONENT_TYPE_BUILD,
            ),
            (
                "name: tb\nphases:\n  - name: test\n    steps: []\n  - name: build\n    steps: []\n",
                COMPONENT_TYPE_BUILD,
            ),
        ];
        for (data, expected) in cases {
            let document = parse_component_document(data)
                .unwrap_or_else(|why| panic!("case {data:?}: expected a document, got {why}"));
            assert_eq!(
                &document.component_type, expected,
                "case {data:?}: expected type {expected:?}, got {:?}",
                document.component_type
            );
        }

        let declared = parse_component_document(
            "name: p\nparameters:\n  - Size:\n      type: string\n      default: small\n      \
             description: how big\n  - Hosts:\n      type: stringList\n      default:\n        \
             - a\n        - b\n  - NoType:\n      description: declares nothing\nphases:\n  - \
             name: build\n    steps: []\n",
        )
        .unwrap_or_else(|why| panic!("case parameters: expected a document, got {why}"));
        let parameters = declared
            .parameters
            .unwrap_or_else(|| panic!("case parameters: expected the declared parameters"));
        let actual: Vec<String> = parameters
            .iter()
            .map(|parameter| {
                format!(
                    "{}:{}:{:?}:{:?}",
                    parameter.name,
                    parameter.r#type,
                    parameter.default_value,
                    parameter.description
                )
            })
            .collect();
        assert_eq!(
            actual,
            vec![
                "Size:string:Some([\"small\"]):Some(\"how big\")".to_string(),
                "Hosts:stringList:Some([\"a\", \"b\"]):None".to_string(),
            ],
            "case parameters: expected the two typed parameters in document order, got {actual:?}"
        );
    }

    #[test]
    fn a_document_that_is_not_one_yaml_mapping_or_uses_aliases_is_refused() {
        let deep_flow = "phases: ".to_string() + &"[".repeat(8_000);
        let quoted_closers = "phases: ".to_string() + &"[']'".repeat(4_000);
        let cases: Vec<String> = vec![
            // Two documents.
            "name: a\n---\nname: b\n".to_string(),
            // Not a mapping.
            "- name: a\n".to_string(),
            // An alias, which is refused rather than expanded.
            "anchor: &a [1, 2]\nphases: *a\n".to_string(),
            // Not YAML at all.
            "name: [a\n".to_string(),
            // Nesting built to drive the loader's recursion, refused before any reader
            // runs: open flow collections, and the same with every closer quoted so a
            // byte scan's running depth stays low while the loader's climbs.
            deep_flow,
            quoted_closers,
            // Block nesting, by indentation and by compact indicators on one line.
            " ".repeat(1_000) + "name: a\n",
            "phases:\n".to_string() + &"- ".repeat(1_000) + "x\n",
        ];
        for data in cases {
            let actual = parse_component_document(&data);
            assert!(
                actual.is_err(),
                "case {:?}…: expected a refusal, got {actual:?}",
                data.chars().take(32).collect::<String>()
            );
        }

        // A document that is big and FLAT is not deep, so none of those bounds touches
        // it: 150 steps at the document's own nesting, inside the modelled 16,000-byte
        // maximum for `data`, are read.
        let flat = "name: Flat\nphases:\n  - name: build\n    steps:\n".to_string()
            + &"      - {name: s, action: ExecuteBash, inputs: {commands: [echo hi]}}\n"
                .repeat(150);
        assert!(
            flat.len() <= 16_000,
            "case a big flat document: the row must stay inside the modelled data maximum, got {}",
            flat.len()
        );
        let actual = parse_component_document(&flat);
        assert!(
            actual.is_ok(),
            "case a big flat document: expected it to be read, got {actual:?}"
        );
    }

    /// RULE: a document that declares no phases describes no work, and is refused with the
    /// clause this class carries whatever else the document holds.
    #[test]
    fn a_document_declaring_no_phases_is_refused_as_missing_phases() {
        // The rows differ along HOW the phases are missing: the key absent, present with an
        // empty list, present with no value at all, and present carrying something that is
        // not a list of phases.
        let cases: &[(&str, &str)] = &[
            ("no phases key at all", "name: x\nschemaVersion: 1.0\n"),
            ("nothing but the name", "name: x\n"),
            ("an empty phases list", "name: x\nphases: []\n"),
            ("a phases key with no value", "name: x\nphases:\n"),
            ("phases carrying a scalar", "name: x\nphases: build\n"),
            (
                "phases carrying a mapping rather than a list",
                "name: x\nphases:\n  build:\n    steps: []\n",
            ),
        ];
        for (case, data) in cases {
            let actual = parse_component_document(data);
            assert_eq!(
                actual.as_ref().err().map(String::as_str),
                Some(MISSING_PHASES_REQUIREMENT),
                "case {case:?}: expected the missing-phases clause, got {actual:?}"
            );
        }
    }

    /// RULE: an already-exists refusal names the label AWS gives the kind the ARN identifies,
    /// which is the kind its own resource field spells except where a recorded refusal shows
    /// otherwise.
    #[test]
    fn an_already_exists_refusal_labels_the_kind_its_arn_names() {
        // (case, the existing resource's ARN, the label the message carries)
        let rows: &[(&str, &str, &str)] = &[
            (
                "a component build version",
                "arn:aws:imagebuilder:us-west-2:000000000000:component/demo/1.0.0/1",
                "Component",
            ),
            (
                "an image recipe",
                "arn:aws:imagebuilder:us-west-2:000000000000:image-recipe/demo/1.0.0",
                "ImageRecipe",
            ),
            (
                "a distribution configuration",
                "arn:aws:imagebuilder:us-west-2:000000000000:distribution-configuration/demo",
                "DistributionConfiguration",
            ),
            (
                "an infrastructure configuration",
                "arn:aws:imagebuilder:us-west-2:000000000000:infrastructure-configuration/demo",
                "InfrastructureConfiguration",
            ),
            (
                "a container recipe",
                "arn:aws:imagebuilder:us-west-2:000000000000:container-recipe/demo/1.0.0",
                "ContainerRecipe",
            ),
            (
                "an image pipeline, which AWS labels by the shorter word",
                "arn:aws:imagebuilder:us-west-2:000000000000:image-pipeline/demo",
                "Pipeline",
            ),
        ];
        for (case, arn, label) in rows {
            let actual = resource_already_exists_message(arn);
            let expected = format!("The following resource '{label}' already exists: '{arn}'.");
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }

    #[test]
    fn a_page_token_is_read_back_only_for_the_listing_that_issued_it() {
        let key = PageTokenKey::new();
        let cursor = PageCursor {
            operation: LIST_IMAGE_RECIPES_OPERATION.to_string(),
            region: "us-west-2".to_string(),
            query: image_recipe_query_fingerprint(Some(OWNERSHIP_SELF), None),
            after: "arn:aws:imagebuilder:us-west-2:000000000000:image-recipe/demo/1.0.0"
                .to_string(),
        };
        let token = encode_page_token(&key, &cursor);
        let actual = decode_page_token(&key, &token, &cursor);
        assert_eq!(
            actual.as_deref(),
            Some(cursor.after.as_str()),
            "case issued token: expected {:?}, got {actual:?}",
            cursor.after
        );

        // The token is opaque: none of the LISTING it continues is readable out of it,
        // however it is decoded — only the position, which is the key the page it follows
        // ended at and the caller was just handed.
        let decoded = ::aws_smithy_types::base64::decode(&token)
            .map(|bytes| String::from_utf8_lossy(&bytes).to_string())
            .unwrap_or_default();
        for secret in [LIST_IMAGE_RECIPES_OPERATION, cursor.query.as_str()] {
            assert!(
                !token.contains(secret) && !decoded.contains(secret),
                "case opacity: expected {secret:?} to be absent from {token:?}"
            );
        }

        // A token read against another listing, Region or query is not this token, and
        // neither is anything the service did not issue.
        let other_query = PageCursor {
            query: image_recipe_query_fingerprint(Some("Amazon"), None),
            ..cursor.clone()
        };
        let other_region = PageCursor {
            region: "eu-west-1".to_string(),
            ..cursor.clone()
        };
        let other_operation = PageCursor {
            operation: LIST_COMPONENTS_OPERATION.to_string(),
            ..cursor.clone()
        };
        let cases: &[(&str, String)] = &[
            ("another query", token.clone()),
            ("another region", token.clone()),
            ("another operation", token.clone()),
            ("fabricated", "fabricated-token".to_string()),
            ("another store's key", encode_page_token(&PageTokenKey::new(), &cursor)),
            (
                "edited authenticator",
                ::aws_smithy_types::base64::encode(
                    [
                        vec![0_u8; ::service_common::page_tokens::AUTHENTICATOR_BYTES],
                        page_token_payload(&cursor).into_bytes(),
                    ]
                    .concat(),
                ),
            ),
        ];
        for (case, candidate) in cases {
            let cursor_of = match *case {
                "another query" => &other_query,
                "another region" => &other_region,
                "another operation" => &other_operation,
                _ => &cursor,
            };
            let actual = decode_page_token(&key, candidate, cursor_of);
            assert!(
                actual.is_none(),
                "case {case}: expected a refusal, got {actual:?}"
            );
        }
    }

    /// A component version is billable only when it is one the Marketplace publishes: the
    /// `billable` member is read off the stored record's owner, so a component this account
    /// created reports false.
    #[test]
    fn billable_follows_the_component_s_owner() {
        let record = ComponentRecord {
            arn: component_build_version_arn("us-west-2", "demo", "1.0.0", FIRST_BUILD_VERSION),
            region: "us-west-2".to_string(),
            name: "demo".to_string(),
            version: "1.0.0".to_string(),
            build_version: FIRST_BUILD_VERSION,
            description: None,
            change_description: None,
            component_type: COMPONENT_TYPE_BUILD.to_string(),
            platform: PLATFORM_LINUX.to_string(),
            supported_os_versions: None,
            owner: ::engine::EMULATOR_ACCOUNT_ID.to_string(),
            data: None,
            uri: None,
            kms_key_id: None,
            parameters: None,
            date_created: "2026-01-01T00:00:00Z".to_string(),
            tags: None,
        };
        // (case, the owner the record reports, whether the version is billable)
        let rows: &[(&str, String, bool)] = &[
            (
                "this account's own",
                ::engine::EMULATOR_ACCOUNT_ID.to_string(),
                false,
            ),
            (
                "another account's, shared with this one",
                "210987654321".to_string(),
                false,
            ),
            ("Amazon's own", "Amazon".to_string(), false),
            ("a third party's", "ThirdParty".to_string(), false),
            (
                "the Marketplace's",
                OWNERSHIP_AWS_MARKETPLACE.to_string(),
                true,
            ),
        ];
        for (case, owner, expected) in rows {
            let record = ComponentRecord {
                owner: owner.clone(),
                ..record.clone()
            };
            let actual = record.billable();
            assert_eq!(
                actual, *expected,
                "case {case:?}: expected billable {expected}, got {actual}"
            );
            let entry = record.version_entry(false);
            assert_eq!(
                entry.billable,
                Some(*expected),
                "case {case:?}: expected the listing entry to report billable \
                 {expected:?}, got {:?}",
                entry.billable
            );
        }
    }
}
