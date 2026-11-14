// SPDX-License-Identifier: CC0-1.0

//! The patch baselines this service holds, and which one is the DEFAULT for an operating
//! system.
//!
//! `CreatePatchBaseline` mints a baseline the account owns and
//! `RegisterDefaultPatchBaseline` records one as the default for the operating system
//! that baseline applies to; `GetDefaultPatchBaseline` reports what is recorded. All of
//! them reach the one store below
//! ([`super::ComAmazonawsSsmAmazonssmState::patch_baselines`] and
//! [`super::ComAmazonawsSsmAmazonssmState::default_patch_baselines`]), so whatever a
//! write persisted is what a later read reports.
//!
//! A baseline is reached through the endpoint of ONE Region — Systems Manager is a
//! regional service and a baseline's ARN carries that Region — so both maps are keyed by
//! Region: the baselines by `(region, baseline id)` and the defaults by
//! `(region, operating system)`. "Systems Manager supports creating multiple default
//! patch baselines. For example, you can create a default patch baseline for each
//! operating system" (`GetDefaultPatchBaseline`), which is why the default is held per
//! operating system rather than once per Region.
//!
//! Not every baseline is the account's own: Systems Manager PREDEFINES one baseline per
//! supported operating system ([`PREDEFINED_BASELINES`]) that exists in every account
//! without anybody creating it, and which is the default until a register says otherwise.
//! A predefined baseline is owned by Systems Manager rather than by the account reading
//! it, so it is named by its full ARN: "To reset the Amazon Web Services-predefined patch
//! baseline as the default, specify the full patch baseline Amazon Resource Name (ARN) as
//! the baseline ID value" (`RegisterDefaultPatchBaseline`). A baseline the account
//! created is named by its own id, and either spelling of either resolves through
//! [`super::ComAmazonawsSsmAmazonssmState::resolve_patch_baseline`].

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::time::SystemTime;

use sha2::{Digest, Sha256};

use super::{parse_resource_query, ComAmazonawsSsmAmazonssmState, ARN_SERVICE};
use crate::ops::types::{PatchBaselineIdentity, PatchFilterGroup, PatchRuleGroup, PatchSource};

/// The resource portion a patch baseline's ARN carries before the baseline's id, from the
/// ARN `RegisterDefaultPatchBaseline` documents
/// (`arn:aws:ssm:us-east-2:733109147000:patchbaseline/pb-0574b43a65ea646ed`).
pub const ARN_PATCH_BASELINE_RESOURCE_PREFIX: &str = "patchbaseline/";

/// What every patch baseline id starts with, from the same documented spelling.
pub const BASELINE_ID_PREFIX: &str = "pb-";

/// How many lowercase hex digits a baseline id carries after [`BASELINE_ID_PREFIX`], from
/// the same documented spelling — which is also what makes a bare id exactly the floor of
/// the modelled `BaselineId` length (`len 20..128`).
pub const BASELINE_ID_HEX_DIGITS: usize = 17;

/// The operating system a request that names none is about: "If you don't specify an
/// operating system value, the default patch baseline for Windows is returned"
/// (`GetDefaultPatchBaseline`), and "Defines the operating system the patch baseline
/// applies to. The default value is WINDOWS" (`CreatePatchBaseline$OperatingSystem`). One
/// of the modelled `OperatingSystem` values (`crate::ops::types::OPERATING_SYSTEM_VALUES`).
pub const DEFAULT_OPERATING_SYSTEM: &str = "WINDOWS";

/// The compliance level of a baseline whose create named none: "The default value is
/// UNSPECIFIED" (`CreatePatchBaseline$ApprovedPatchesComplianceLevel`). One of the
/// modelled `PatchComplianceLevel` values.
pub const DEFAULT_APPROVED_PATCHES_COMPLIANCE_LEVEL: &str = "UNSPECIFIED";

/// What Patch Manager does with a rejected patch when the create named no action: "This
/// is the default action if no option is specified"
/// (`CreatePatchBaseline$RejectedPatchesAction`, `ALLOW_AS_DEPENDENCY`). One of the
/// modelled `PatchAction` values.
pub const DEFAULT_REJECTED_PATCHES_ACTION: &str = "ALLOW_AS_DEPENDENCY";

/// Whether a baseline whose create named nothing includes non-security updates among its
/// approved patches: "The default value is false"
/// (`CreatePatchBaseline$ApprovedPatchesEnableNonSecurity`).
pub const DEFAULT_APPROVED_PATCHES_ENABLE_NON_SECURITY: bool = false;

/// Whether an approval rule that named nothing applies non-security updates available in
/// the repository it selects from: "The default value is false"
/// (`PatchRule$EnableNonSecurity`).
pub const DEFAULT_PATCH_RULE_ENABLE_NON_SECURITY: bool = false;

/// How many patch baselines this store holds at once, across every Region.
///
/// This is the EMULATOR's own ceiling rather than a quota Systems Manager publishes: a
/// create is the only thing that admits a baseline, callers may run as many as they like,
/// and the store is held in memory for as long as the process runs — so without a ceiling
/// the one map would grow for as long as creates keep arriving. A create past it is
/// refused through the modelled `ResourceLimitExceededException` and writes nothing.
pub const MAX_PATCH_BASELINES: usize = 100;

/// What a create refused by [`MAX_PATCH_BASELINES`] reports: the emulator's own ceiling,
/// named as the emulator's, with the figure it stands at.
pub fn patch_baseline_ceiling_message() -> String {
    format!("This emulator holds at most {MAX_PATCH_BASELINES} patch baselines across every Region")
}

/// How many Regions' default patch baselines this store records.
///
/// Emulator resource guard, NOT a published AWS quota: a default is recorded under the
/// Region of the request's own credential scope ([`super::request_region`]), which the
/// caller chooses, so without a total the one map would be one set of defaults per
/// caller-chosen partition of the key space and could grow for as long as registers kept
/// arriving. More Regions than AWS itself has, so no account reaching a Region that exists
/// can trip it. Mirrors the guard the service settings of this store carry
/// (`super::service_settings::MAX_SERVICE_SETTING_REGIONS`).
pub const MAX_DEFAULT_PATCH_BASELINE_REGIONS: usize = 64;

/// How many default patch baselines this store records in TOTAL: one per operating system
/// Patch Manager supports, once per Region [`MAX_DEFAULT_PATCH_BASELINE_REGIONS`] allows.
pub const MAX_DEFAULT_PATCH_BASELINES: usize =
    MAX_DEFAULT_PATCH_BASELINE_REGIONS * PREDEFINED_BASELINES.len();

/// What a register is refused with when it would record one default past
/// [`MAX_DEFAULT_PATCH_BASELINES`] — the emulator's own guard, named as the emulator's
/// rather than as an AWS quota, and reported the way this crate's other total guards are.
#[must_use]
pub fn defaults_store_full_message() -> String {
    format!(
        "This emulator records at most {MAX_DEFAULT_PATCH_BASELINES} default patch baselines \
         across every Region"
    )
}

/// How this service reports a create that reuses an idempotency token with members that
/// differ from the create that token already ran, through `IdempotentParameterMismatch`.
///
/// The sentence is the modelled error's own account of its class — "an idempotent
/// operation is retried and the parameters don't match the original call to the API with
/// the same idempotency token" (`smithy_query(query="doc",
/// shape="IdempotentParameterMismatch")`) — so it is fixed text and interpolates nothing
/// from the request.
pub const IDEMPOTENT_PARAMETER_MISMATCH: &str = "The parameters don't match the original \
     call to the API with the same idempotency token.";

/// How this service reports a `BaselineId` that names no patch baseline reachable through
/// this endpoint, through `DoesNotExistException`.
///
/// The whole sentence is FIXED text for this error class: it names the resource class and
/// interpolates nothing from the request, so the id a caller submitted does not appear in
/// it. Every operation that resolves a baseline id reports it, so a read and a write of
/// the same absent baseline are worded the same way.
pub const PATCH_BASELINE_NOT_FOUND: &str = "Patch Baseline to be retrieved does not exist.";

/// How Patch Manager words the refusal of a baseline that would approve no patch at all.
///
/// A baseline approves patches either by naming them (`ApprovedPatches`) or by a rule that
/// selects them (`ApprovalRules`); the model makes each member optional on its own, so the
/// rule is that the two cannot BOTH be empty, and the sentence is fixed text naming the
/// two attributes rather than anything of the request.
pub const APPROVAL_COVERAGE_REQUIRED: &str =
    "Patch Baseline attribute: Approved Patches and Approval Rule, at least one must be non empty.";

/// Why a patch baseline carrying `approved_patches` and `approval_rules` cannot be stored,
/// or `None` when at least one of the two approves something.
///
/// An `ApprovalRules` whose own `PatchRules` list is empty selects no patch, so it is as
/// empty as a missing one: what the rule asks for is a NON-EMPTY approved-patch list or a
/// non-empty rule group. Every write of a baseline is held to it, so a baseline that could
/// never approve a patch cannot be created nor updated into existence.
pub fn approval_coverage_rejection(
    approved_patches: Option<&[String]>,
    approval_rules: Option<&PatchRuleGroup>,
) -> Option<&'static str> {
    let approves_by_name = approved_patches.is_some_and(|patches| !patches.is_empty());
    let approves_by_rule = approval_rules.is_some_and(|rules| !rules.patch_rules.is_empty());
    (!approves_by_name && !approves_by_rule).then_some(APPROVAL_COVERAGE_REQUIRED)
}

// ---------------------------------------------------------------------------
// Listing the baselines an endpoint holds
// ---------------------------------------------------------------------------

/// The filter key a baseline listing narrows by name with: "NAME_PREFIX Sample values:
/// AWS- | My-" (`smithy_query(query="doc", shape="DescribePatchBaselinesRequest",
/// member="Filters")`).
pub const FILTER_KEY_NAME_PREFIX: &str = "NAME_PREFIX";

/// The filter key a baseline listing narrows by owner with: "OWNER Sample values: AWS |
/// Self" (same doc).
pub const FILTER_KEY_OWNER: &str = "OWNER";

/// The filter key a baseline listing narrows by operating system with: "OPERATING_SYSTEM
/// Sample values: AMAZON_LINUX | SUSE | WINDOWS" (same doc).
pub const FILTER_KEY_OPERATING_SYSTEM: &str = "OPERATING_SYSTEM";

/// Every key a baseline listing filters on, in the order that doc lists them. The
/// `PatchOrchestratorFilter$Key` member is a plain string in the model, so the vocabulary
/// is enforced here, and this is also what a refusal names back to the caller.
pub const PATCH_BASELINE_FILTER_KEYS: &[&str] = &[
    FILTER_KEY_NAME_PREFIX,
    FILTER_KEY_OWNER,
    FILTER_KEY_OPERATING_SYSTEM,
];

/// The `OWNER` of a baseline Systems Manager predefines ([`PREDEFINED_BASELINES`]), from
/// the sample values of that filter key.
pub const OWNER_AWS: &str = "AWS";

/// The `OWNER` of a baseline the account created itself, from the same sample values.
pub const OWNER_SELF: &str = "Self";

/// What a `Filters` entry naming no filter key at all is refused with. Both members of a
/// `PatchOrchestratorFilter` are optional in the model, so such an entry reaches the
/// handler, and it names no property of a baseline for the listing to be narrowed by.
pub const PATCH_BASELINE_FILTER_KEY_MISSING: &str =
    "A Filters entry carries no filter key to search on";

/// What a `Filters` entry naming a key but no value is refused with, worded as the sibling
/// listings of this service word it ([`crate::ops::list_cloud_connectors`]).
pub fn patch_baseline_filter_value_missing(key: &str) -> String {
    format!("Filter key '{key}' carries no value to search for")
}

/// Whether `key` is a key a baseline listing filters on
/// ([`PATCH_BASELINE_FILTER_KEYS`]).
pub fn is_patch_baseline_filter_key(key: &str) -> bool {
    PATCH_BASELINE_FILTER_KEYS.contains(&key)
}

/// The lower bound the modelled page size of a baseline listing carries
/// (`DescribePatchBaselinesRequest$MaxResults`, range 1..100).
pub const MIN_PATCH_BASELINE_PAGE_SIZE: i32 = 1;

/// The upper bound of the same member, and the page a request naming no `MaxResults` is
/// served up to: "the maximum number of patch baselines to return (per page)" bounds a
/// page the caller asked to be bounded, so a caller asking for no bound is served the
/// whole page the operation allows.
pub const MAX_PATCH_BASELINE_PAGE_SIZE: i32 = 100;

/// The page size a baseline listing serves, or the refusal message for a `MaxResults`
/// outside the modelled range — which the boundary walk already refuses, so this is a
/// backstop rather than the first line of defence (see [`super::page_size`]).
pub fn patch_baseline_page_size(max_results: Option<i32>) -> Result<usize, String> {
    super::page_size(
        max_results,
        MIN_PATCH_BASELINE_PAGE_SIZE,
        MAX_PATCH_BASELINE_PAGE_SIZE,
    )
}

/// The basic information one baseline reachable through an endpoint carries — "defines the
/// basic information about a patch baseline" (`PatchBaselineIdentity`) — projected from
/// whatever holds that baseline: the created-baseline map, or the definition of a baseline
/// Systems Manager predefines.
///
/// Both kinds of baseline are reachable through one endpoint, so a listing reports them
/// through one record: the `reference` is how a read NAMES the baseline (its own id for
/// one the account created, its full ARN for one Systems Manager predefined, the same
/// spelling [`ResolvedBaseline::reference`] carries), while `baseline_id` is the bare id
/// the baseline is held or derived under, which is what a filter and a page token work on.
#[derive(Debug, Clone)]
pub struct PatchBaselineIdentityRecord {
    /// The bare id the baseline is held (or derived) under.
    pub baseline_id: String,
    /// How a read names this baseline ([`ResolvedBaseline::reference`]).
    pub reference: String,
    /// "The name of the patch baseline."
    pub name: String,
    /// "Defines the operating system the patch baseline applies to."
    pub operating_system: String,
    /// "The description of the patch baseline", or `None` for a baseline whose write
    /// carried none — and for one Systems Manager predefines, whose contents are Patch
    /// Manager's own and are held nowhere here.
    pub description: Option<String>,
    /// Who owns the baseline: [`OWNER_AWS`] for one Systems Manager predefines and
    /// [`OWNER_SELF`] for one the account created.
    pub owner: &'static str,
    /// "Indicates whether this is the default baseline" for the operating system it
    /// applies to, which is what a register of a default moves.
    pub default_baseline: bool,
}

impl PatchBaselineIdentityRecord {
    /// Whether this baseline meets the filter `key` searching for any of `values`.
    ///
    /// A filter entry carries a list of values, so a baseline meets the entry when ANY of
    /// them selects it, and a value naming nothing a baseline carries selects none.
    pub fn matches_filter(&self, key: &str, values: &[String]) -> bool {
        values.iter().any(|value| match key {
            // "NAME_PREFIX": the start of the baseline's name rather than the whole of it.
            FILTER_KEY_NAME_PREFIX => self.name.starts_with(value.as_str()),
            FILTER_KEY_OWNER => self.owner == value.as_str(),
            FILTER_KEY_OPERATING_SYSTEM => self.operating_system == value.as_str(),
            // A key outside the vocabulary is refused before anything is matched, so this
            // is unreachable through the wire.
            _ => false,
        })
    }

    /// This baseline as the identity a listing reports.
    pub fn to_identity(&self) -> PatchBaselineIdentity {
        PatchBaselineIdentity {
            baseline_id: Some(self.reference.clone()),
            baseline_name: Some(self.name.clone()),
            operating_system: Some(self.operating_system.clone()),
            baseline_description: self.description.clone(),
            default_baseline: self.default_baseline,
        }
    }
}

/// One patch baseline Systems Manager predefines: it exists in every account and Region
/// without anybody creating it, and is the default for its operating system until a
/// register says otherwise.
#[derive(Debug, Clone, Copy)]
pub struct PredefinedBaseline {
    /// The baseline's name, in the form the modelled `BaselineName` allows
    /// (`^[a-zA-Z0-9_\-.]{3,128}$`).
    pub name: &'static str,
    /// The operating system the baseline applies to: one of the modelled
    /// `OperatingSystem` values (`crate::ops::types::OPERATING_SYSTEM_VALUES`).
    pub operating_system: &'static str,
}

/// The predefined baseline of each operating system Patch Manager supports — the
/// `AWS-…DefaultPatchBaseline` of that operating system, as Patch Manager names them, with
/// one entry per value of the modelled `OperatingSystem` enum so every operating system a
/// request may name has a default without anybody registering one.
pub const PREDEFINED_BASELINES: &[PredefinedBaseline] = &[
    PredefinedBaseline {
        name: "AWS-AlmaLinuxDefaultPatchBaseline",
        operating_system: "ALMA_LINUX",
    },
    PredefinedBaseline {
        name: "AWS-AmazonLinuxDefaultPatchBaseline",
        operating_system: "AMAZON_LINUX",
    },
    PredefinedBaseline {
        name: "AWS-AmazonLinux2DefaultPatchBaseline",
        operating_system: "AMAZON_LINUX_2",
    },
    PredefinedBaseline {
        name: "AWS-AmazonLinux2022DefaultPatchBaseline",
        operating_system: "AMAZON_LINUX_2022",
    },
    PredefinedBaseline {
        name: "AWS-AmazonLinux2023DefaultPatchBaseline",
        operating_system: "AMAZON_LINUX_2023",
    },
    PredefinedBaseline {
        name: "AWS-CentOSDefaultPatchBaseline",
        operating_system: "CENTOS",
    },
    PredefinedBaseline {
        name: "AWS-DebianDefaultPatchBaseline",
        operating_system: "DEBIAN",
    },
    PredefinedBaseline {
        name: "AWS-MacOSDefaultPatchBaseline",
        operating_system: "MACOS",
    },
    PredefinedBaseline {
        name: "AWS-OracleLinuxDefaultPatchBaseline",
        operating_system: "ORACLE_LINUX",
    },
    PredefinedBaseline {
        name: "AWS-RaspbianDefaultPatchBaseline",
        operating_system: "RASPBIAN",
    },
    PredefinedBaseline {
        name: "AWS-RedHatDefaultPatchBaseline",
        operating_system: "REDHAT_ENTERPRISE_LINUX",
    },
    PredefinedBaseline {
        name: "AWS-RockyLinuxDefaultPatchBaseline",
        operating_system: "ROCKY_LINUX",
    },
    PredefinedBaseline {
        name: "AWS-SuseDefaultPatchBaseline",
        operating_system: "SUSE",
    },
    PredefinedBaseline {
        name: "AWS-UbuntuDefaultPatchBaseline",
        operating_system: "UBUNTU",
    },
    PredefinedBaseline {
        name: "AWS-DefaultPatchBaseline",
        operating_system: DEFAULT_OPERATING_SYSTEM,
    },
];

/// The baseline Systems Manager predefines for `operating_system`, or `None` for a value
/// outside the modelled enum — which the boundary walk refuses before a handler runs.
pub fn predefined_baseline(operating_system: &str) -> Option<&'static PredefinedBaseline> {
    PREDEFINED_BASELINES
        .iter()
        .find(|baseline| baseline.operating_system == operating_system)
}

/// The baseline Systems Manager predefines that carries `baseline_id` at `region`'s
/// endpoint ([`predefined_baseline_id`]), or `None` when the id is none of theirs.
///
/// The id of a predefined baseline is derived rather than minted, so this is how a read
/// that holds an id gets back to the baseline the id stands for.
pub fn predefined_baseline_with_id(
    region: &str,
    baseline_id: &str,
) -> Option<&'static PredefinedBaseline> {
    PREDEFINED_BASELINES
        .iter()
        .find(|baseline| predefined_baseline_id(region, baseline.name) == baseline_id)
}

/// The id a predefined baseline carries at `region`'s endpoint.
///
/// A predefined baseline is not created by a call, so there is no mint counter behind its
/// id: it is derived from the one thing that identifies it — its name — and the Region it
/// is reached through, so two reads of one Region's baseline report the same id, two
/// Regions report different ids the way Patch Manager's own predefined baselines differ
/// per Region, and nothing of a request is in it.
pub fn predefined_baseline_id(region: &str, name: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(region.as_bytes());
    hasher.update([0u8]);
    hasher.update(name.as_bytes());
    let mut digits = ::hex::encode(hasher.finalize());
    digits.truncate(BASELINE_ID_HEX_DIGITS);
    format!("{BASELINE_ID_PREFIX}{digits}")
}

/// Mint the `BaselineId` of a baseline being created: [`BASELINE_ID_PREFIX`] followed by
/// [`BASELINE_ID_HEX_DIGITS`] lowercase hex digits.
///
/// Through the crate's one minter of a prefixed hex identifier
/// ([`super::minted_prefixed_hex_id`]): the digits are the leading bytes of a digest of
/// the store's own mint counter and the instant of the create, so two baselines of one
/// store never carry one id and an id carries nothing from the request that created it.
pub fn mint_baseline_id(sequence: u64, at: SystemTime) -> String {
    super::minted_prefixed_hex_id(BASELINE_ID_PREFIX, BASELINE_ID_HEX_DIGITS, sequence, at)
}

/// A patch baseline's ARN: `arn:aws:ssm:<region>:<account>:patchbaseline/<id>`.
pub fn patch_baseline_arn(region: &str, baseline_id: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("{ARN_PATCH_BASELINE_RESOURCE_PREFIX}{baseline_id}"),
    )
}

/// What a `BaselineId` a request carried resolved to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedBaseline {
    /// The baseline's own id, which is the key it is held (or derived) under.
    pub baseline_id: String,
    /// The operating system the baseline applies to, which is the operating system whose
    /// default it becomes when it is registered as one.
    pub operating_system: String,
    /// How a read NAMES this baseline: its own id when the account owns it, and its full
    /// ARN when Systems Manager predefined it, because a predefined baseline is Systems
    /// Manager's rather than the account's.
    pub reference: String,
}

/// One patch baseline the account created, as the store holds it.
///
/// Every member a baseline read reports is here, so a read projects this record rather
/// than recomputing anything: what the create persisted is what a later read reports.
#[derive(Debug, Clone)]
pub struct PatchBaselineRecord {
    /// The server-minted id this record is stored under ([`mint_baseline_id`]).
    pub baseline_id: String,
    /// The Region whose endpoint this baseline was created through, and which its ARN
    /// carries.
    pub region: String,
    /// "The name of the patch baseline" — required on create, so a baseline always has
    /// one.
    pub name: String,
    /// "Defines the operating system the patch baseline applies to", defaulted to
    /// [`DEFAULT_OPERATING_SYSTEM`].
    pub operating_system: String,
    /// "A set of global filters used to include patches in the baseline", or `None` for a
    /// create that carried none.
    pub global_filters: Option<PatchFilterGroup>,
    /// "A set of rules used to include patches in the baseline", or `None` for a create
    /// that carried none.
    pub approval_rules: Option<PatchRuleGroup>,
    /// "A list of explicitly approved patches for the baseline", in the create's order.
    pub approved_patches: Vec<String>,
    /// "Defines the compliance level for approved patches", defaulted to
    /// [`DEFAULT_APPROVED_PATCHES_COMPLIANCE_LEVEL`].
    pub approved_patches_compliance_level: String,
    /// "Indicates whether the list of approved patches includes non-security updates",
    /// defaulted to [`DEFAULT_APPROVED_PATCHES_ENABLE_NON_SECURITY`].
    pub approved_patches_enable_non_security: bool,
    /// "A list of explicitly rejected patches for the baseline", in the create's order.
    pub rejected_patches: Vec<String>,
    /// "The action for Patch Manager to take on patches included in the RejectedPackages
    /// list", defaulted to [`DEFAULT_REJECTED_PATCHES_ACTION`].
    pub rejected_patches_action: String,
    /// "A description of the patch baseline", or `None` for a create that carried none.
    pub description: Option<String>,
    /// "Information about the patches to use to update the managed nodes", in the
    /// create's order.
    pub sources: Vec<PatchSource>,
    /// "Indicates the status you want to assign to security patches that are available
    /// but not approved", or `None` for a create that carried none.
    pub available_security_updates_compliance_status: Option<String>,
    /// The "user-provided idempotency token" the create carried, or `None` when it
    /// carried none: what makes a retry of that create report the baseline it already
    /// minted rather than minting a second one.
    pub client_token: Option<String>,
    /// The baseline's tags, keyed by tag key, as the create supplied them.
    pub tags: BTreeMap<String, String>,
    /// When the create ran.
    pub created_date: SystemTime,
    /// When the baseline was last written. Equal to
    /// [`created_date`](Self::created_date) until something updates the baseline.
    pub modified_date: SystemTime,
}

impl PatchBaselineRecord {
    /// This baseline's ARN ([`patch_baseline_arn`]).
    pub fn arn(&self) -> String {
        patch_baseline_arn(&self.region, &self.baseline_id)
    }

    /// What this baseline resolves to when a request names it.
    pub fn resolved(&self) -> ResolvedBaseline {
        ResolvedBaseline {
            baseline_id: self.baseline_id.clone(),
            operating_system: self.operating_system.clone(),
            // A baseline the account created is named by its own id.
            reference: self.baseline_id.clone(),
        }
    }

    /// The global filter group a read of this baseline reports: the group its write named,
    /// or the group that filters nothing when the write named none.
    ///
    /// A read reports the default of every member a write left out — the way a baseline's
    /// own compliance level, rejected-patch action and operating system are reported at
    /// their documented defaults ([`DEFAULT_APPROVED_PATCHES_COMPLIANCE_LEVEL`] and the
    /// constants beside it). For a "set of filters" the default is the EMPTY set rather
    /// than nothing at all, which is what a baseline no global filter excludes a patch
    /// from carries.
    pub fn reported_global_filters(&self) -> PatchFilterGroup {
        self.global_filters.clone().unwrap_or_default()
    }

    /// The approval rule group a read of this baseline reports: the group its write named,
    /// with every member of every rule the write left out reported at its default, or the
    /// group that selects nothing when the write named none.
    ///
    /// A rule's own optional members default the same way a baseline's do: "the default
    /// value is false" (`PatchRule$EnableNonSecurity`), and a rule that names no compliance
    /// severity level carries the unspecified one
    /// ([`DEFAULT_APPROVED_PATCHES_COMPLIANCE_LEVEL`]), which is the level a baseline that
    /// names none carries too.
    pub fn reported_approval_rules(&self) -> PatchRuleGroup {
        let mut rules = self.approval_rules.clone().unwrap_or_default();
        for rule in &mut rules.patch_rules {
            rule.compliance_level
                .get_or_insert_with(|| DEFAULT_APPROVED_PATCHES_COMPLIANCE_LEVEL.to_string());
            rule.enable_non_security
                .get_or_insert(DEFAULT_PATCH_RULE_ENABLE_NON_SECURITY);
        }
        rules
    }

    /// The members a create SUPPLIED, rendered as one canonical string.
    ///
    /// This is what "the parameters don't match the original call to the API with the same
    /// idempotency token" (`IdempotentParameterMismatch`) is decided on: two creates
    /// carrying one idempotency token are the same call when their supplied members render
    /// the same way. The minted id, the Region, the token itself and the instants are not
    /// supplied members, so they are not in it.
    pub fn creation_fingerprint(&self) -> String {
        let mut rendered = String::new();
        let _ = write!(
            rendered,
            "name={}\nos={}\ndescription={}\napproved={}\nlevel={}\nnon_security={}\n\
             rejected={}\nrejected_action={}\navailable_status={}\n",
            self.name,
            self.operating_system,
            self.description.as_deref().unwrap_or_default(),
            self.approved_patches.join(","),
            self.approved_patches_compliance_level,
            self.approved_patches_enable_non_security,
            self.rejected_patches.join(","),
            self.rejected_patches_action,
            self.available_security_updates_compliance_status
                .as_deref()
                .unwrap_or_default(),
        );
        rendered.push_str("global_filters=");
        render_filter_group(&mut rendered, self.global_filters.as_ref());
        rendered.push_str("\napproval_rules=");
        if let Some(rules) = self.approval_rules.as_ref() {
            for rule in &rules.patch_rules {
                let _ = write!(
                    rendered,
                    "[level={};after={};until={};non_security={};filters=",
                    rule.compliance_level.as_deref().unwrap_or_default(),
                    rule.approve_after_days.unwrap_or_default(),
                    rule.approve_until_date.as_deref().unwrap_or_default(),
                    rule.enable_non_security.unwrap_or_default(),
                );
                render_filter_group(&mut rendered, Some(&rule.patch_filter_group));
                rendered.push(']');
            }
        }
        rendered.push_str("\nsources=");
        for source in &self.sources {
            let _ = write!(
                rendered,
                "[{};{};{}]",
                source.name,
                source.products.join(","),
                source.configuration
            );
        }
        rendered.push_str("\ntags=");
        for (key, value) in &self.tags {
            let _ = write!(rendered, "[{key}={value}]");
        }
        rendered
    }
}

/// Render a patch filter group into `out`, filters in their own order, so two groups
/// render the same way exactly when they carry the same filters in the same order.
fn render_filter_group(out: &mut String, group: Option<&PatchFilterGroup>) {
    let Some(group) = group else {
        return;
    };
    for filter in &group.patch_filters {
        let _ = write!(out, "({}={})", filter.key, filter.values.join(","));
    }
}

impl ComAmazonawsSsmAmazonssmState {
    /// The patch baseline `baseline_id` names in `region`, or `None` when that Region
    /// holds none.
    pub fn patch_baseline(&self, region: &str, baseline_id: &str) -> Option<&PatchBaselineRecord> {
        self.patch_baselines
            .get(&(region.to_string(), baseline_id.to_string()))
    }

    /// Store `record` as a patch baseline of its own Region, keyed by its id.
    pub fn insert_patch_baseline(&mut self, record: PatchBaselineRecord) {
        let key = (record.region.clone(), record.baseline_id.clone());
        self.patch_baselines.insert(key, record);
    }

    /// Whether this store can take another patch baseline ([`MAX_PATCH_BASELINES`]).
    pub fn can_hold_another_patch_baseline(&self) -> bool {
        self.patch_baselines.len() < MAX_PATCH_BASELINES
    }

    /// Mint the id of a patch baseline being created in this store.
    pub fn next_patch_baseline_id(&mut self, at: SystemTime) -> String {
        self.patch_baseline_sequence += 1;
        mint_baseline_id(self.patch_baseline_sequence, at)
    }

    /// The baseline `region` holds under the idempotency token `client_token`, or `None`
    /// when no create carrying that token has run there.
    pub fn patch_baseline_with_client_token(
        &self,
        region: &str,
        client_token: &str,
    ) -> Option<&PatchBaselineRecord> {
        self.patch_baselines
            .iter()
            .filter(|((held, _), _)| held == region)
            .map(|(_, record)| record)
            .find(|record| record.client_token.as_deref() == Some(client_token))
    }

    /// What the `BaselineId` `value` names through `region`'s endpoint: the baseline the
    /// account created under that id, or the one Systems Manager predefined
    /// ([`PREDEFINED_BASELINES`]).
    ///
    /// Either spelling resolves — a bare id, or the full ARN a predefined baseline is
    /// named by — through the one parse every "name or ARN" member of this service reads
    /// ([`parse_resource_query`]). `None` means the value names no baseline reachable
    /// here, which is what a not-found refusal reports; a value that is an ARN of another
    /// service, another account or another Region names nothing this endpoint could ever
    /// hold, which is reported as an invalid id instead ([`Self::patch_baseline_query`]).
    pub fn resolve_patch_baseline(&self, region: &str, value: &str) -> Option<ResolvedBaseline> {
        let baseline_id = self.patch_baseline_query(region, value)?;
        if let Some(record) = self.patch_baseline(region, &baseline_id) {
            return Some(record.resolved());
        }
        predefined_baseline_with_id(region, &baseline_id)
            .map(|baseline| predefined_resolved(region, baseline))
    }

    /// The baseline id `value` names through `region`'s endpoint, or `None` when `value`
    /// could never name a baseline held there: an ARN of another service, of another
    /// account or of another Region, or one whose resource is not a patch baseline at all.
    ///
    /// `region` comes from the signed credential scope (`super::request_region`) and never
    /// from the request body, so an ARN naming another endpoint resolves to nothing here.
    pub fn patch_baseline_query(&self, region: &str, value: &str) -> Option<String> {
        let query = parse_resource_query(value, ARN_PATCH_BASELINE_RESOURCE_PREFIX)?;
        if !query.names_this_endpoint(region) {
            return None;
        }
        Some(query.name.to_string())
    }

    /// The default patch baseline of `operating_system` at `region`'s endpoint: the one a
    /// register recorded there, or the baseline Systems Manager predefines for that
    /// operating system when nothing has registered one — so every operating system a
    /// request may name has a default and this read invents none.
    ///
    /// A recorded default is resolved against the store rather than reported as stored, so
    /// a reference that no longer names a baseline reports the predefined default again
    /// rather than a baseline nothing holds.
    pub fn default_patch_baseline(
        &self,
        region: &str,
        operating_system: &str,
    ) -> Option<ResolvedBaseline> {
        let registered = self
            .default_patch_baselines
            .get(&(region.to_string(), operating_system.to_string()))
            .and_then(|reference| self.resolve_patch_baseline(region, reference))
            .filter(|resolved| resolved.operating_system == operating_system);
        registered.or_else(|| {
            predefined_baseline(operating_system)
                .map(|baseline| predefined_resolved(region, baseline))
        })
    }

    /// Record `resolved` as the default patch baseline of its operating system at
    /// `region`'s endpoint — what `RegisterDefaultPatchBaseline` persists, so a later read
    /// reports it instead of the predefined default — or the reason this emulator cannot
    /// hold it.
    ///
    /// The map is bounded by [`MAX_DEFAULT_PATCH_BASELINES`], because the Region a default
    /// is recorded under comes from the request's own credential scope: a register that
    /// would record a default for a `(region, operating system)` the store does not already
    /// hold is refused past that total, while a register that REPLACES a default already
    /// held is always accepted, so the guard can never wedge an operating system whose
    /// default an account has already set.
    pub fn register_default_patch_baseline(
        &mut self,
        region: &str,
        resolved: &ResolvedBaseline,
    ) -> Result<(), String> {
        let key = (region.to_string(), resolved.operating_system.clone());
        if !self.default_patch_baselines.contains_key(&key)
            && self.default_patch_baselines.len() >= MAX_DEFAULT_PATCH_BASELINES
        {
            return Err(defaults_store_full_message());
        }
        self.default_patch_baselines
            .insert(key, resolved.reference.clone());
        Ok(())
    }

    /// Every patch baseline reachable through `region`'s endpoint, as the basic information
    /// a listing reports about it: the ones the account created there, and the ones Systems
    /// Manager predefines ([`PREDEFINED_BASELINES`]), which exist without anybody creating
    /// them.
    ///
    /// A READ: whatever a create, an update or a register of a default persisted is what
    /// this projects, and nothing is written. The order is total and taken from the
    /// baselines themselves — by name, with the bare id breaking a tie — so it is
    /// independent of the request, two reads report the same sequence, and a page resumes
    /// at the baseline the last one stopped at.
    pub fn patch_baseline_identities(&self, region: &str) -> Vec<PatchBaselineIdentityRecord> {
        let predefined = PREDEFINED_BASELINES.iter().map(|baseline| {
            let resolved = predefined_resolved(region, baseline);
            PatchBaselineIdentityRecord {
                baseline_id: resolved.baseline_id,
                reference: resolved.reference,
                name: baseline.name.to_string(),
                operating_system: baseline.operating_system.to_string(),
                // What Patch Manager approves in its own baseline, its description
                // included, is held nowhere here, so a read of one reports no description
                // rather than inventing a sentence for it — the way `GetPatchBaseline`
                // leaves those members unreported.
                description: None,
                owner: OWNER_AWS,
                default_baseline: false,
            }
        });
        let created = self
            .patch_baselines
            .iter()
            .filter(|((held, _), _)| held == region)
            .map(|(_, record)| PatchBaselineIdentityRecord {
                baseline_id: record.baseline_id.clone(),
                reference: record.baseline_id.clone(),
                name: record.name.clone(),
                operating_system: record.operating_system.clone(),
                description: record.description.clone(),
                owner: OWNER_SELF,
                default_baseline: false,
            });
        let mut identities: Vec<PatchBaselineIdentityRecord> = predefined.chain(created).collect();

        // Which baseline is the default is held once per operating system, so the flag each
        // identity carries is read from that one record rather than stored beside the
        // baseline: the baseline a register recorded for an operating system carries it, and
        // the one Systems Manager predefines for that operating system carries it while
        // nothing has registered one.
        for identity in &mut identities {
            identity.default_baseline = self
                .default_patch_baseline(region, &identity.operating_system)
                .is_some_and(|default| default.baseline_id == identity.baseline_id);
        }
        identities.sort_by(|left, right| {
            (&left.name, &left.baseline_id).cmp(&(&right.name, &right.baseline_id))
        });
        identities
    }
}

/// What a predefined baseline resolves to at `region`'s endpoint: its derived id, its
/// operating system, and the full ARN a read names it by.
fn predefined_resolved(region: &str, baseline: &PredefinedBaseline) -> ResolvedBaseline {
    let baseline_id = predefined_baseline_id(region, baseline.name);
    ResolvedBaseline {
        reference: patch_baseline_arn(region, &baseline_id),
        baseline_id,
        operating_system: baseline.operating_system.to_string(),
    }
}
