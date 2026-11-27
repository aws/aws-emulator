// SPDX-License-Identifier: CC0-1.0

//! The controls the Control Catalog publishes, as data: one row per published control.
//!
//! ## One row per published control, not per check
//!
//! The catalog publishes a control for every surface that governs a resource, and
//! several surfaces can govern the SAME thing: `ListControls` "[r]eturns a paginated
//! list of all available controls in the Control Catalog library", each as a
//! `ControlSummary` carrying its own `Arn`, `Aliases` and `Implementation`
//! (`smithy_query(query="operation", operation="ListControls",
//! model_path="services/service-com-amazonaws-controlcatalog-controlcatalog/smithy-model/com-amazonaws-controlcatalog-controlcatalog.json")`).
//! So the AWS Config rule that detects a misconfiguration, the Security Hub control
//! that reports it, the service control policy that prevents it and the CloudFormation
//! hook that rejects it are FOUR published controls, each with its own identity, not one
//! record wearing four designators — and two controls may legitimately share one
//! `Implementation` (a Control Tower detective control and the Config-rule control
//! behind it both report `AWS::Config::ConfigRule` / `CLOUD_TRAIL_ENABLED`), which is
//! why `Implementation` is not this table's key and the `Identifiers` filter can match
//! more than one row.
//!
//! ## Where the values come from
//!
//! * `implementation_type` / `implementation_identifier` are the implementing service's
//!   own published identifiers: an AWS Config managed rule identifier
//!   (`MULTI_REGION_CLOUD_TRAIL_ENABLED`), a Security Hub control id (`CloudTrail.1`),
//!   or — for a service control policy, which has no per-control identifier — `None`.
//!   Both members' character classes come from the model: `ImplementationType` is
//!   `^[A-Za-z0-9]+(::[A-Za-z0-9_]+){2,3}$` and `ImplementationIdentifier`
//!   `^[a-zA-Z0-9_\.-]+$` (`smithy_query(query="shape", shape="ImplementationType")`).
//! * `control_tower_identifier` is the `AWS-GR_…` identifier Amazon Web Services Control
//!   Tower publishes for the control, where it publishes one.
//! * `name` and `description` are the control's PUBLISHED text, worded by the surface
//!   that publishes the control: Control Tower's own control name and description for a
//!   control Control Tower publishes (a row carrying a `control_tower_identifier`), the
//!   implementing AWS Config managed rule's own title and description for a control
//!   the catalog publishes under a `CONFIG.…` designator, and the sentence the catalog
//!   publishes for a Security Hub-implemented control under its `SH.…` designator. They
//!   are catalog CONTENT reproduced VERBATIM, never re-worded, re-ordered, abridged or
//!   extended here — not a paraphrase of the implementing service's current
//!   documentation prose, and not a summary of the row written here: `Description` is
//!   "[a] description of the control, as it may appear in the console"
//!   (`smithy_query(query="shape", shape="ControlSummary")`), and the catalog is account-
//!   and Region-independent, so one ARN carries one `Name` and one `Description` for
//!   every caller and every run.
//! * the instant the catalog publishes as a control's release time (`CreateTime`) is
//!   recorded with its published identity, in [`PUBLISHED_CATALOG_RECORDS`], because like
//!   the opaque identifier it is a fact the catalog assigns and nothing in the row or the
//!   request derives — and two controls the catalog released together carry one instant.
//! * `parameters` are the control parameters a caller supplies when it ENABLES the
//!   control, and they are the parameters of the SURFACE that implements it, so they are
//!   read through [`parameters`] rather than off the row alone: a control an AWS Config
//!   managed rule implements supports that rule's own input parameters
//!   ([`CONFIG_RULE_PARAMETERS`], which is why two controls sharing one rule publish one
//!   parameter set), while a control deployed as a policy or a hook supports the
//!   exemption parameters the modeled `ControlParameter` documents (see
//!   [`ALLOWED_REGIONS`] and the names beside it), which the row carries itself. The
//!   modeled `ParameterRequirementSummary` is DERIVED from that list (see
//!   [`parameter_requirement_summary`]) rather than written per row, because it is
//!   defined as a summary of exactly that list.
//! * `aliases` are the designators the catalog PUBLISHES the control under, written in
//!   the row (see [`aliases`]). A designator is catalog CONTENT, not a function of the
//!   control's implementation identifier: the catalog publishes most Security Hub
//!   controls under `SH.` followed by the control's own Security Hub id, but it
//!   publishes others under a designator of its own (`SH.{SERVICE}.DT.{N}`, the same
//!   shape as the `CONFIG.…` designators), so the designators cannot be computed from
//!   the id. Only the Control Tower designator is read off the row's
//!   `control_tower_identifier`, because the designator IS that identifier.
//! * the compliance-framework items a control satisfies are mostly a property of the
//!   control OBJECTIVE it implements, not of its own row, so they are recorded once per
//!   objective in [`OBJECTIVE_FRAMEWORKS`]; a row carries only the items that name its
//!   own check (see [`framework_mappings`]).
//!
//! The opaque 25-character identifier inside a catalog ARN is assigned by Amazon Web
//! Services and is derivable from nothing in the request, yet it is part of the
//! control's published identity: the catalog is account-independent, so one ARN names
//! the control for every caller, in every partition and on every run. A control whose
//! published record is recorded in [`PUBLISHED_CATALOG_RECORDS`] is therefore served
//! under that identifier and, where the record holds one, with that release time; for a
//! control it does not name — and for the ontology in [`super::published_ontology`] —
//! this emulator mints an identifier deterministically from the row's `slug` (see
//! `super::catalog_id`) and derives the release instant from that same slug (see
//! `super::minted_release_time`). The slug never appears on the wire.

/// `ControlParameterRequirement`: a parameter the caller must supply.
pub(super) const REQUIRED: &str = "REQUIRED";
/// `ControlParameterRequirement`: a parameter the caller may supply.
pub(super) const OPTIONAL: &str = "OPTIONAL";

// The EXEMPTION parameters a control deployed as a policy or a hook can support,
// spelled as the modeled `ControlParameter` spells them: "Five types of control
// parameters are supported" — `AllowedRegions`, `ExemptedActions`,
// `ExemptedPrincipalArns`, `ExemptedResourceArns` and `ExemptAssumeRoot`, each a value
// supplied when the control is enabled
// (`smithy_query(query="doc", shape="ControlParameter",
// model_path="services/service-com-amazonaws-controlcatalog-controlcatalog/smithy-model/com-amazonaws-controlcatalog-controlcatalog.json")`).
// They are the vocabulary of the exemptions a deployed control takes, not the whole
// parameter space: a control an implementing surface parameterises publishes that
// surface's own input parameters as well (see [`CONFIG_RULE_PARAMETERS`]). Only the
// names the controls in this table support are declared here; of the rest,
// `ExemptAssumeRoot` "applies only to the `AWS-GR_RESTRICT_ROOT_USER` control".

/// Regions exempted from the control.
const ALLOWED_REGIONS: &str = "AllowedRegions";
/// IAM principal ARNs exempted from the control.
const EXEMPTED_PRINCIPAL_ARNS: &str = "ExemptedPrincipalArns";
/// Resource ARNs exempted from the control.
const EXEMPTED_RESOURCE_ARNS: &str = "ExemptedResourceArns";

// The parameters Security Hub publishes for the control that checks AWS Config resource
// recording, under Security Hub's own names: whether the recorder must cover every
// resource type, and the resource types it must cover when it does not. Source: Security
// Hub's documented parameters for that control, which the catalog publishes as the
// control's `Parameters` — a control the implementing service parameterises is published
// as accepting them, which is what its `ParameterRequirementSummary` then summarises.

/// Whether every resource type has to be recorded.
const RECORD_ALL_RESOURCES: &str = "recordAllResources";
/// The resource types that have to be recorded when not every type is.
const RESOURCE_TYPES: &str = "resourceTypes";

/// `ControlBehavior`: the control reports on resources that already exist.
const DETECTIVE: &str = "DETECTIVE";
/// `ControlBehavior`: the control blocks a non-compliant action.
const PREVENTIVE: &str = "PREVENTIVE";
/// `ControlBehavior`: the control rejects a non-compliant deployment before it runs.
const PROACTIVE: &str = "PROACTIVE";

/// The `ControlSeverity` values this table's controls are published with, as the modeled
/// enum spells them. A control's severity is the one the catalog publishes for it, not a
/// value chosen to cover the enum.
const LOW: &str = "LOW";
const MEDIUM: &str = "MEDIUM";
const HIGH: &str = "HIGH";
const CRITICAL: &str = "CRITICAL";

/// `ControlScope`: effective in every Region, so it reports no deployable Regions.
const GLOBAL: &str = "GLOBAL";
/// `ControlScope`: deployed per Region, so it reports the Regions it deploys to.
const REGIONAL: &str = "REGIONAL";

/// Implementation types this catalog publishes controls for.
pub(super) const CONFIG_RULE: &str = "AWS::Config::ConfigRule";
pub(super) const SECURITY_HUB_CONTROL: &str = "AWS::SecurityHub::SecurityControl";
pub(super) const SERVICE_CONTROL_POLICY: &str =
    "AWS::Organizations::Policy::SERVICE_CONTROL_POLICY";
pub(super) const CLOUDFORMATION_HOOK: &str = "AWS::CloudFormation::Type::HOOK";

/// The `SH.` designator prefix a Security Hub control is published under. The
/// `Aliases` documentation gives `SH.S3.1` as its example of a "human-readable
/// designator", and `S3.1` is that control's own Security Hub id; what follows the
/// prefix is published content, so a control's own designator is recorded on its row.
pub(super) const SECURITY_HUB_ALIAS_PREFIX: &str = "SH.";

/// `GovernedProviders` values: "a value of AWS indicates that the control governs
/// Amazon Web Services resources".
const AWS: &[&str] = &["AWS"];
const GCP: &[&str] = &["GCP"];

/// Region sets a REGIONAL control in this catalog deploys to. A control is available
/// in the Regions its implementing service is, which differs between controls, so the
/// table picks the set per row.
const US_REGIONS: &[&str] = &["us-east-1", "us-west-2"];
/// A GLOBAL control deploys to no Region of its own.
const NO_REGIONS: &[&str] = &[];

/// Every commercial Region of the `aws` partition, which is the deployable-Region set of
/// a control whose implementing surface is generally available: `DeployableRegions`
/// reports "the Regions in which the control is available for deployment"
/// (`smithy_query(query="doc", shape="RegionConfiguration", member="DeployableRegions")`),
/// and a control implemented by a generally available service — an AWS Config managed
/// rule or a Security Hub control over a service offered partition-wide — is available
/// for deployment wherever that partition offers the service. Ordered by Region code, the
/// order the catalog reports them in.
///
/// It is the partition-wide set, so for a control whose implementing service is offered
/// in fewer Regions it is an upper bound rather than a read fact; a control the catalog
/// publishes a narrower availability for carries its own set on its row.
const GENERALLY_AVAILABLE_REGIONS: &[&str] = &[
    "af-south-1",
    "ap-east-1",
    "ap-east-2",
    "ap-northeast-1",
    "ap-northeast-2",
    "ap-northeast-3",
    "ap-south-1",
    "ap-south-2",
    "ap-southeast-1",
    "ap-southeast-2",
    "ap-southeast-3",
    "ap-southeast-4",
    "ap-southeast-5",
    "ap-southeast-6",
    "ap-southeast-7",
    "ca-central-1",
    "ca-west-1",
    "eu-central-1",
    "eu-central-2",
    "eu-north-1",
    "eu-south-1",
    "eu-south-2",
    "eu-west-1",
    "eu-west-2",
    "eu-west-3",
    "il-central-1",
    "me-central-1",
    "me-south-1",
    "mx-central-1",
    "sa-east-1",
    "us-east-1",
    "us-east-2",
    "us-west-1",
    "us-west-2",
];

/// The Regions a control over Amazon Cognito user pool advanced security deploys to.
///
/// Source: the availability the catalog publishes for this control — the Regions a user
/// pool can have advanced security in, which is where the implementing AWS Config managed
/// rule can evaluate one. `DeployableRegions` reports "the Regions in which the control is
/// available for deployment" (`smithy_query(query="doc", shape="RegionConfiguration",
/// member="DeployableRegions")`), so a control whose implementing surface is offered in
/// fewer Regions than the partition publishes its own, narrower list rather than the
/// partition-wide one ([`GENERALLY_AVAILABLE_REGIONS`]). It is narrower than the partition
/// but not the Regions enabled by default either: the catalog publishes the control as
/// deployable in the opt-in Regions `eu-south-1`, `il-central-1` and `me-south-1` as well.
/// Ordered by Region code, the order the catalog reports them in.
const COGNITO_ADVANCED_SECURITY_REGIONS: &[&str] = &[
    "ap-northeast-1",
    "ap-northeast-2",
    "ap-northeast-3",
    "ap-south-1",
    "ap-southeast-1",
    "ap-southeast-2",
    "ca-central-1",
    "eu-central-1",
    "eu-north-1",
    "eu-south-1",
    "eu-west-1",
    "eu-west-2",
    "eu-west-3",
    "il-central-1",
    "me-south-1",
    "sa-east-1",
    "us-east-1",
    "us-east-2",
    "us-west-1",
    "us-west-2",
];

/// The Regions a control over an Amazon Redshift Serverless workgroup deploys to.
///
/// Source: the availability the catalog publishes for this control — the Regions a
/// Redshift Serverless workgroup can exist in for the control to govern one, which is
/// narrower than the partition ([`GENERALLY_AVAILABLE_REGIONS`]) and is not the set the
/// control over Amazon Cognito user pool advanced security carries either: it covers the
/// opt-in Regions the service is offered in (`ap-east-1`, `ap-southeast-3`, `eu-central-2`,
/// `eu-south-2`, `il-central-1`, `me-central-1`) and leaves out Regions the service is not
/// offered in, `af-south-1` and `ap-northeast-3` among them. Ordered by Region code, the
/// order the catalog reports them in.
const REDSHIFT_SERVERLESS_REGIONS: &[&str] = &[
    "ap-east-1",
    "ap-northeast-1",
    "ap-northeast-2",
    "ap-south-1",
    "ap-southeast-1",
    "ap-southeast-2",
    "ap-southeast-3",
    "ca-central-1",
    "eu-central-1",
    "eu-central-2",
    "eu-north-1",
    "eu-south-2",
    "eu-west-1",
    "eu-west-2",
    "eu-west-3",
    "il-central-1",
    "me-central-1",
    "sa-east-1",
    "us-east-1",
    "us-east-2",
    "us-west-1",
    "us-west-2",
];

/// A control that supports no control parameter: nothing is supplied when it is
/// enabled, so its `ParameterRequirementSummary` is `NONE`.
const NO_PARAMETERS: &[(&str, &str)] = &[];

/// A control that is published under no catalog designator of its own (its aliases,
/// if any, are the derived ones).
const NO_DESIGNATORS: &[&str] = &[];

/// A control whose own check no published compliance-framework item names: the framework
/// requirements it satisfies are the ones its control OBJECTIVE is cross-walked into (see
/// [`OBJECTIVE_FRAMEWORKS`]), and nothing beyond them is recorded on the row.
const NO_FRAMEWORK_ITEMS: &[FrameworkItem] = &[];

/// A resource set that no available resource type can represent: "If
/// GovernedResources cannot be represented by available resource types, it's returned
/// as an empty list."
const NO_GOVERNED_RESOURCES: &[&str] = &[];

// The compliance frameworks this catalog cross-walks its controls into.
//
// A FRAMEWORK mapping names the framework by the compact designator the catalog
// publishes it under — the framework's abbreviation, followed by the EDITION it is read
// at where the catalog reads it at a particular one, and nothing else — because the
// designator is what identifies a framework ACROSS the surfaces that cross-walk into it,
// while a prose title is one surface's wording of it. The edition is whichever form that
// framework issues its editions in: a version (`NIST-CSF-v1.1`, `PCI-DSS-v4.0`), a
// revision (`FedRAMP-r4`), a level (`CMMC-Level2`), the part of a wider standard the
// edition is read at (`ISO-IEC-27001:2022-Annex-A`) or the publication date of the
// edition, for a framework that issues dated editions rather than numbered ones
// (`SSAE-18-SOC-2-Oct-2023`).
//
// The catalog cross-walks a requirement into EVERY edition of a framework it publishes,
// each under its own designator, because a client reads its compliance obligation at the
// edition it is audited against: so the frameworks below carry more than one designator
// for one framework where the catalog publishes more than one of its editions, and an
// edition that restates another edition's requirements is DERIVED from it (see
// [`framework_mappings`]) rather than transcribed. The item beside a designator is always
// that EDITION's own identifier for the requirement — the 2013 edition of ISO/IEC 27001
// numbers its Annex A controls `A.9.4.1` and the 2022 edition numbers its Annex A clauses
// `8.22` — so an item is never renumbered from one edition into another's designator.
//
// The modeled `FrameworkName` admits 3..250 characters and no pattern, so a prose title
// would serialize too; the designator is the one a client can join on.
pub(super) const NIST_SP_800_53_R5: &str = "NIST-SP-800-53-r5";
pub(super) const NIST_SP_800_171_R2: &str = "NIST-SP-800-171-r2";
pub(super) const NIST_CSF_V1_1: &str = "NIST-CSF-v1.1";
pub(super) const PCI_DSS_V4_0: &str = "PCI-DSS-v4.0";
pub(super) const CIS_AWS_BENCHMARK_V3_0: &str = "CIS-AWS-Benchmark-v3.0";
pub(super) const CIS_CONTROLS_V8_0: &str = "CIS-v8.0";
pub(super) const CIS_GCP_BENCHMARK_V2_0: &str = "CIS-GCP-Benchmark-v2.0";
pub(super) const FEDRAMP_R4: &str = "FedRAMP-r4";
pub(super) const FEDRAMP_R5: &str = "FedRAMP-r5";
pub(super) const ISO_IEC_27001: &str = "ISO-IEC-27001";
pub(super) const ISO_IEC_27001_2022_ANNEX_A: &str = "ISO-IEC-27001:2022-Annex-A";
pub(super) const CRI_PROFILE_V2_2: &str = "CRI-Profile-v2.2";
pub(super) const CMMC_LEVEL_2: &str = "CMMC-Level2";
pub(super) const SSAE_18_SOC_2: &str = "SSAE-18-SOC-2";
pub(super) const SSAE_18_SOC_2_OCT_2023: &str = "SSAE-18-SOC-2-Oct-2023";
pub(super) const AWS_WAF_V10: &str = "AWS-WAF-v10";

/// Every framework designator the catalog publishes, which is also the set a
/// FRAMEWORK mapping's `Name` is drawn from.
pub(super) const PUBLISHED_FRAMEWORKS: &[&str] = &[
    NIST_SP_800_53_R5,
    NIST_SP_800_171_R2,
    NIST_CSF_V1_1,
    PCI_DSS_V4_0,
    CIS_AWS_BENCHMARK_V3_0,
    CIS_CONTROLS_V8_0,
    CIS_GCP_BENCHMARK_V2_0,
    FEDRAMP_R4,
    FEDRAMP_R5,
    ISO_IEC_27001,
    ISO_IEC_27001_2022_ANNEX_A,
    CRI_PROFILE_V2_2,
    CMMC_LEVEL_2,
    SSAE_18_SOC_2,
    SSAE_18_SOC_2_OCT_2023,
    AWS_WAF_V10,
];

/// One framework requirement: `(the framework's designator, that framework's own
/// identifier for the requirement)`.
pub(super) type FrameworkItem = (&'static str, &'static str);

/// One control the catalog publishes.
#[derive(Debug)]
pub(super) struct ControlSeed {
    /// Internal, never on the wire: the hash input `super::catalog_id` mints this
    /// control's opaque catalog identifier from, and the key the mapping tables use.
    pub(super) slug: &'static str,
    pub(super) name: &'static str,
    pub(super) description: &'static str,
    /// `ControlBehavior`.
    pub(super) behavior: &'static str,
    /// `ControlSeverity`.
    pub(super) severity: &'static str,
    /// `ImplementationType`.
    pub(super) implementation_type: &'static str,
    /// `ImplementationIdentifier`, where the implementing surface publishes one.
    pub(super) implementation_identifier: Option<&'static str>,
    /// The Control Tower `AWS-GR_…` identifier, for a control Control Tower publishes.
    pub(super) control_tower_identifier: Option<&'static str>,
    /// Every catalog designator the control is published under (`CONFIG.…`, `CT.…`,
    /// `SH.…`) — published content, so it is written here rather than computed. Only the
    /// Control Tower designator is left out, because it is the `AWS-GR_…` identifier
    /// `control_tower_identifier` already records (see [`aliases`]).
    pub(super) designators: &'static [&'static str],
    /// `ControlScope`.
    pub(super) scope: &'static str,
    /// Empty for a GLOBAL control: `DeployableRegions` does not apply to it.
    pub(super) deployable_regions: &'static [&'static str],
    /// `(name, ControlParameterRequirement)` for each parameter the control publishes
    /// itself: the EXEMPTION parameters of a control Control Tower deploys, named from the
    /// modeled vocabulary ([`ALLOWED_REGIONS`] and the names beside it), and — for a
    /// Security Hub control the implementing service parameterises — that control's own
    /// Security Hub parameters, under Security Hub's own names. An AWS Config managed
    /// rule's parameters are NOT written here: they belong to the rule, so a control that
    /// rule implements takes them from [`CONFIG_RULE_PARAMETERS`]. Read a control's whole
    /// parameter list through [`parameters`].
    pub(super) parameters: &'static [(&'static str, &'static str)],
    pub(super) governed_resources: &'static [&'static str],
    pub(super) governed_providers: &'static [&'static str],
    /// The slug of the common control this control implements, in
    /// [`super::published_ontology`]: the control's COMMON_CONTROL mapping.
    pub(super) implements: &'static str,
    /// `(framework designator, that framework's identifier for the requirement)` for
    /// each framework item that names THIS control's own check — a benchmark item
    /// written against one service's setting. The items every control implementing the
    /// same objective satisfies are recorded once, in [`OBJECTIVE_FRAMEWORKS`]; a
    /// control's FRAMEWORK mappings are the two sets together (see
    /// [`framework_mappings`]).
    pub(super) frameworks: &'static [FrameworkItem],
}

/// The catalog's controls, in one table.
pub(super) const PUBLISHED_CONTROLS: &[ControlSeed] = &[
    // --- AWS Config managed rules: detective controls -----------------------
    ControlSeed {
        slug: "account-part-of-organizations",
        name: "Rule checks whether AWS account is part of AWS Organizations",
        // The implementing AWS Config managed rule's own published description, which is
        // what the catalog publishes for a control it publishes under a `CONFIG.…`
        // designator: its first sentence is the control's published name, followed by the
        // rule's own compliance criterion.
        description: "Rule checks whether AWS account is part of AWS Organizations. The \
                      rule is NON_COMPLIANT if the AWS account is not part of AWS \
                      Organizations or AWS Organizations master account ID does not match \
                      rule parameter MasterAccountId.",
        behavior: DETECTIVE,
        severity: HIGH,
        implementation_type: CONFIG_RULE,
        implementation_identifier: Some("ACCOUNT_PART_OF_ORGANIZATIONS"),
        control_tower_identifier: None,
        designators: &["CONFIG.ACCOUNT.DT.1"],
        scope: GLOBAL,
        deployable_regions: NO_REGIONS,
        parameters: NO_PARAMETERS,
        // An account's membership of an organization has no resource type of its own.
        governed_resources: NO_GOVERNED_RESOURCES,
        governed_providers: AWS,
        implements: "maintain-an-asset-inventory",
        frameworks: &[
            // "System component inventory".
            (NIST_SP_800_53_R5, "CM-8"),
            // "Establish and maintain detailed enterprise asset inventory".
            (CIS_CONTROLS_V8_0, "1.1"),
        ],
    },
    ControlSeed {
        slug: "detect-cloud-trail-enabled-on-shared-accounts",
        name: "Detect whether a shared AWS account in the Security organizational unit has \
               at least one AWS CloudTrail trail enabled for a governed AWS Region.",
        description: "This control detects whether a shared AWS account in the Security \
                      organizational unit has at least one AWS CloudTrail trail enabled \
                      for a governed AWS Region. This rule is NON_COMPLIANT if no \
                      CloudTrail trails are enabled in an AWS account for a governed AWS \
                      Region.",
        behavior: DETECTIVE,
        severity: HIGH,
        // One AWS Config managed rule backs this Control Tower control and the
        // member-account one below: two published controls over one implementation, each
        // governing a different scope.
        implementation_type: CONFIG_RULE,
        implementation_identifier: Some("CLOUD_TRAIL_ENABLED"),
        control_tower_identifier: Some("AWS-GR_DETECT_CLOUDTRAIL_ENABLED_ON_SHARED_ACCOUNTS"),
        designators: NO_DESIGNATORS,
        scope: REGIONAL,
        deployable_regions: GENERALLY_AVAILABLE_REGIONS,
        parameters: NO_PARAMETERS,
        governed_resources: &["AWS::CloudTrail::Trail"],
        governed_providers: AWS,
        implements: "enable-centralized-logging",
        frameworks: &[
            // "Event logging".
            (NIST_SP_800_53_R5, "AU-2"),
            // "Ensure CloudTrail is enabled in all Regions".
            (CIS_AWS_BENCHMARK_V3_0, "3.1"),
        ],
    },
    ControlSeed {
        slug: "detect-cloud-trail-enabled-on-member-accounts",
        name: "Detect whether an AWS account has at least one AWS CloudTrail trail enabled \
               for a governed AWS Region.",
        description: "Detects whether an AWS account has AWS CloudTrail enabled has at \
                      least one AWS CloudTrail trail enabled for a governed AWS Region. \
                      This rule is NON_COMPLIANT if no CloudTrail trails are enabled in an \
                      AWS account for a governed AWS Region.",
        behavior: DETECTIVE,
        severity: HIGH,
        // The same Config rule backs this Control Tower control and the shared-account
        // one above: two published controls, one implementation.
        implementation_type: CONFIG_RULE,
        implementation_identifier: Some("CLOUD_TRAIL_ENABLED"),
        control_tower_identifier: Some("AWS-GR_DETECT_CLOUDTRAIL_ENABLED_ON_MEMBER_ACCOUNTS"),
        designators: NO_DESIGNATORS,
        scope: REGIONAL,
        deployable_regions: GENERALLY_AVAILABLE_REGIONS,
        parameters: NO_PARAMETERS,
        governed_resources: &["AWS::CloudTrail::Trail"],
        governed_providers: AWS,
        implements: "enable-centralized-logging",
        frameworks: &[
            // "Event data are collected and correlated".
            (NIST_CSF_V1_1, "DE.AE-3"),
            // "Event logging".
            (NIST_SP_800_53_R5, "AU-2"),
        ],
    },
    ControlSeed {
        slug: "multi-region-cloud-trail-enabled",
        name: "Checks that there is at least one multi-region AWS CloudTrail",
        // The implementing AWS Config managed rule's own published description, which is
        // what the catalog publishes for a control it publishes under a `CONFIG.…`
        // designator.
        description: "Checks that there is at least one multi-region AWS CloudTrail. The \
                      rule is non-compliant if the trails do not match input parameters",
        behavior: DETECTIVE,
        severity: HIGH,
        implementation_type: CONFIG_RULE,
        implementation_identifier: Some("MULTI_REGION_CLOUD_TRAIL_ENABLED"),
        control_tower_identifier: None,
        designators: &["CONFIG.CLOUDTRAIL.DT.10"],
        scope: REGIONAL,
        deployable_regions: GENERALLY_AVAILABLE_REGIONS,
        // Parameterised by the implementing rule; see `CONFIG_RULE_PARAMETERS`.
        parameters: NO_PARAMETERS,
        governed_resources: &["AWS::CloudTrail::Trail"],
        governed_providers: AWS,
        implements: "enable-centralized-logging",
        frameworks: &[
            // "Audit logs are enabled and active for all system components".
            (PCI_DSS_V4_0, "10.2.1"),
            // "Ensure CloudTrail is enabled in all Regions".
            (CIS_AWS_BENCHMARK_V3_0, "3.1"),
        ],
    },
    ControlSeed {
        slug: "cloud-trail-log-file-validation-enabled",
        name: "Checks if AWS CloudTrail creates a signed digest file with logs",
        description: "Checks if AWS CloudTrail creates a signed digest file with logs. AWS \
                      recommends that the file validation must be enabled on all trails. \
                      The rule is NON_COMPLIANT if the validation is not enabled.",
        behavior: DETECTIVE,
        severity: HIGH,
        implementation_type: CONFIG_RULE,
        implementation_identifier: Some("CLOUD_TRAIL_LOG_FILE_VALIDATION_ENABLED"),
        control_tower_identifier: None,
        designators: &["CONFIG.CLOUDTRAIL.DT.9"],
        scope: REGIONAL,
        deployable_regions: GENERALLY_AVAILABLE_REGIONS,
        parameters: NO_PARAMETERS,
        governed_resources: &["AWS::CloudTrail::Trail"],
        governed_providers: AWS,
        implements: "log-integrity-protection",
        frameworks: &[
            // "Protection of audit information".
            (NIST_SP_800_53_R5, "AU-9"),
            // "Ensure CloudTrail log file validation is enabled".
            (CIS_AWS_BENCHMARK_V3_0, "3.2"),
        ],
    },
    ControlSeed {
        slug: "s3-bucket-server-side-encryption-enabled",
        name: "Checks if S3 buckets have default encryption enabled or have bucket \
               policies that explicitly deny put-object requests without server side \
               encryption using AES-256 or AWS KMS",
        // The implementing AWS Config managed rule's own published title and
        // description, which the catalog publishes for this control.
        description: "Checks if S3 buckets have default encryption enabled or have bucket \
                      policies that explicitly deny put-object requests without server \
                      side encryption using AES-256 or AWS KMS. The rule is \
                      NON_COMPLIANT if an S3 bucket is not encrypted by default.",
        behavior: DETECTIVE,
        severity: MEDIUM,
        implementation_type: CONFIG_RULE,
        implementation_identifier: Some("S3_BUCKET_SERVER_SIDE_ENCRYPTION_ENABLED"),
        control_tower_identifier: None,
        designators: &["CONFIG.S3.DT.8"],
        scope: REGIONAL,
        deployable_regions: GENERALLY_AVAILABLE_REGIONS,
        parameters: NO_PARAMETERS,
        governed_resources: &["AWS::S3::Bucket"],
        governed_providers: AWS,
        implements: "encrypt-data-at-rest-in-storage-services",
        frameworks: &[
            // "Protection of information at rest".
            (NIST_SP_800_53_R5, "SC-28"),
            // "Data-at-rest is protected".
            (NIST_CSF_V1_1, "PR.DS-1"),
            // "Primary account number is rendered unreadable wherever it is stored".
            (PCI_DSS_V4_0, "3.5.1"),
            // "Encrypt sensitive data at rest".
            (CIS_CONTROLS_V8_0, "3.11"),
        ],
    },
    ControlSeed {
        slug: "s3-bucket-public-read-prohibited",
        name: "Disallow public read access to S3 buckets",
        // Control Tower's own published description of the control it publishes, which
        // is what the catalog publishes for a Control Tower control.
        description: "Secure access to data stored in Amazon S3 buckets by disallowing \
                      public reads.",
        behavior: DETECTIVE,
        severity: CRITICAL,
        implementation_type: CONFIG_RULE,
        implementation_identifier: Some("S3_BUCKET_PUBLIC_READ_PROHIBITED"),
        control_tower_identifier: Some("AWS-GR_S3_BUCKET_PUBLIC_READ_PROHIBITED"),
        designators: NO_DESIGNATORS,
        scope: REGIONAL,
        deployable_regions: GENERALLY_AVAILABLE_REGIONS,
        parameters: NO_PARAMETERS,
        governed_resources: &["AWS::S3::Bucket"],
        governed_providers: AWS,
        implements: "restrict-public-access",
        frameworks: &[
            // "Access enforcement".
            (NIST_SP_800_53_R5, "AC-3"),
            // "Ensure that S3 buckets are configured with 'Block public access'".
            (CIS_AWS_BENCHMARK_V3_0, "2.1.4"),
        ],
    },
    // The log-archive control over the SAME Config rule as the account-wide one above:
    // the catalog publishes the two separately, because one governs every bucket in the
    // account and this one the bucket the organization archives its audit logs in.
    ControlSeed {
        slug: "audit-bucket-public-read-prohibited",
        name: "Disallow public read access to log archive",
        description: "Control access to your log archive's Amazon S3 bucket by \
                      disallowing public read access.",
        behavior: DETECTIVE,
        severity: CRITICAL,
        implementation_type: CONFIG_RULE,
        implementation_identifier: Some("S3_BUCKET_PUBLIC_READ_PROHIBITED"),
        control_tower_identifier: Some("AWS-GR_AUDIT_BUCKET_PUBLIC_READ_PROHIBITED"),
        designators: NO_DESIGNATORS,
        scope: REGIONAL,
        deployable_regions: GENERALLY_AVAILABLE_REGIONS,
        parameters: NO_PARAMETERS,
        governed_resources: &["AWS::S3::Bucket"],
        governed_providers: AWS,
        implements: "restrict-public-access",
        frameworks: &[
            // "Access enforcement".
            (NIST_SP_800_53_R5, "AC-3"),
            // "Protection of audit information".
            (NIST_SP_800_53_R5, "AU-9"),
        ],
    },
    ControlSeed {
        slug: "s3-bucket-public-write-prohibited",
        name: "Disallow public write access to S3 buckets",
        description: "Secure access to data stored in Amazon S3 buckets by disallowing \
                      public writes.",
        behavior: DETECTIVE,
        severity: CRITICAL,
        implementation_type: CONFIG_RULE,
        implementation_identifier: Some("S3_BUCKET_PUBLIC_WRITE_PROHIBITED"),
        control_tower_identifier: Some("AWS-GR_S3_BUCKET_PUBLIC_WRITE_PROHIBITED"),
        designators: NO_DESIGNATORS,
        scope: REGIONAL,
        deployable_regions: GENERALLY_AVAILABLE_REGIONS,
        parameters: NO_PARAMETERS,
        governed_resources: &["AWS::S3::Bucket"],
        governed_providers: AWS,
        implements: "restrict-public-access",
        frameworks: &[
            // "Protections against data leaks are implemented".
            (NIST_CSF_V1_1, "PR.DS-5"),
            // "Access enforcement".
            (NIST_SP_800_53_R5, "AC-3"),
        ],
    },
    // The log-archive control over the same Config rule as the account-wide one above.
    ControlSeed {
        slug: "audit-bucket-public-write-prohibited",
        name: "Disallow public write access to log archive",
        description: "Control access to your log archive's Amazon S3 bucket by \
                      disallowing public write access.",
        behavior: DETECTIVE,
        severity: CRITICAL,
        implementation_type: CONFIG_RULE,
        implementation_identifier: Some("S3_BUCKET_PUBLIC_WRITE_PROHIBITED"),
        control_tower_identifier: Some("AWS-GR_AUDIT_BUCKET_PUBLIC_WRITE_PROHIBITED"),
        designators: NO_DESIGNATORS,
        scope: REGIONAL,
        deployable_regions: GENERALLY_AVAILABLE_REGIONS,
        parameters: NO_PARAMETERS,
        governed_resources: &["AWS::S3::Bucket"],
        governed_providers: AWS,
        implements: "restrict-public-access",
        frameworks: &[
            // "Protections against data leaks are implemented".
            (NIST_CSF_V1_1, "PR.DS-5"),
            // "Protection of audit information".
            (NIST_SP_800_53_R5, "AU-9"),
        ],
    },
    ControlSeed {
        slug: "s3-bucket-ssl-requests-only",
        name: "Checks if S3 buckets have policies that require requests to use SSL/TLS",
        description: "Checks if S3 buckets have policies that require requests to use \
                      SSL/TLS. The rule is NON_COMPLIANT if any S3 bucket has policies \
                      allowing HTTP requests.",
        behavior: DETECTIVE,
        severity: MEDIUM,
        implementation_type: CONFIG_RULE,
        implementation_identifier: Some("S3_BUCKET_SSL_REQUESTS_ONLY"),
        control_tower_identifier: None,
        designators: &["CONFIG.S3.DT.15"],
        scope: REGIONAL,
        deployable_regions: GENERALLY_AVAILABLE_REGIONS,
        parameters: NO_PARAMETERS,
        governed_resources: &["AWS::S3::Bucket"],
        governed_providers: AWS,
        implements: "enforce-encryption-in-transit",
        frameworks: &[
            // "Transmission confidentiality and integrity".
            (NIST_SP_800_53_R5, "SC-8"),
            // "Strong cryptography is implemented to safeguard data in transit".
            (PCI_DSS_V4_0, "4.2.1"),
        ],
    },
    ControlSeed {
        slug: "iam-root-access-key-check",
        name: "Checks if the root user access key is available",
        description: "Checks if the root user access key is available. The rule is \
                      COMPLIANT if the user access key does not exist. Otherwise, \
                      NON_COMPLIANT.",
        behavior: DETECTIVE,
        severity: CRITICAL,
        implementation_type: CONFIG_RULE,
        implementation_identifier: Some("IAM_ROOT_ACCESS_KEY_CHECK"),
        control_tower_identifier: None,
        designators: &["CONFIG.IAM.DT.13"],
        scope: GLOBAL,
        deployable_regions: NO_REGIONS,
        parameters: NO_PARAMETERS,
        // The account's own root-user credentials, which no available resource type
        // represents, so the catalog publishes the list empty.
        governed_resources: NO_GOVERNED_RESOURCES,
        governed_providers: AWS,
        implements: "privileged-access-approval",
        frameworks: &[
            // "Ensure no 'root' user account access key exists".
            (CIS_AWS_BENCHMARK_V3_0, "1.4"),
            // "Least privilege".
            (NIST_SP_800_53_R5, "AC-6"),
        ],
    },
    ControlSeed {
        slug: "iam-user-mfa-enabled",
        name: "Disallow access to IAM users without MFA",
        // Control Tower's own published description for the control it publishes, which
        // is what the catalog publishes for a control carrying an `AWS-GR_…` designator.
        description: "Protect your account by requiring multi-factor authentication (MFA) \
                      for any AWS Identity and Access Management (IAM) user using the \
                      account. MFA adds an extra authentication code on top of a user name \
                      and password.",
        behavior: DETECTIVE,
        severity: MEDIUM,
        implementation_type: CONFIG_RULE,
        implementation_identifier: Some("IAM_USER_MFA_ENABLED"),
        control_tower_identifier: Some("AWS-GR_IAM_USER_MFA_ENABLED"),
        designators: NO_DESIGNATORS,
        scope: GLOBAL,
        deployable_regions: NO_REGIONS,
        parameters: NO_PARAMETERS,
        governed_resources: &["AWS::IAM::User"],
        governed_providers: AWS,
        implements: "require-multi-factor-authentication",
        frameworks: &[
            // "Ensure MFA is enabled for all IAM users that have a console password".
            (CIS_AWS_BENCHMARK_V3_0, "1.10"),
            // "Identification and authentication (organizational users)".
            (NIST_SP_800_53_R5, "IA-2"),
        ],
    },
    ControlSeed {
        slug: "access-keys-rotated",
        name: "Checks whether an IAM access key is rotated within the required number of days",
        description: "Reports an active IAM access key older than the maximum age the \
                      implementing rule allows.",
        behavior: DETECTIVE,
        severity: MEDIUM,
        implementation_type: CONFIG_RULE,
        implementation_identifier: Some("ACCESS_KEYS_ROTATED"),
        control_tower_identifier: None,
        designators: NO_DESIGNATORS,
        scope: GLOBAL,
        deployable_regions: NO_REGIONS,
        // The key age this control checks against is the implementing rule's input
        // parameter, which it publishes as a control parameter; see
        // `CONFIG_RULE_PARAMETERS`.
        parameters: NO_PARAMETERS,
        governed_resources: &["AWS::IAM::AccessKey"],
        governed_providers: AWS,
        implements: "credential-rotation",
        frameworks: &[
            // "Ensure access keys are rotated every 90 days or less".
            (CIS_AWS_BENCHMARK_V3_0, "1.14"),
            // "Authenticator management".
            (NIST_SP_800_53_R5, "IA-5"),
        ],
    },
    ControlSeed {
        slug: "iam-password-policy",
        name: "Checks if the account password policy for AWS Identity and Access \
               Management (IAM) users meets the specified requirements indicated in the \
               parameters",
        description: "Checks if the account password policy for AWS Identity and Access \
                      Management (IAM) users meets the specified requirements indicated in \
                      the parameters. The rule is NON_COMPLIANT if the account password \
                      policy does not meet the specified requirements.",
        behavior: DETECTIVE,
        severity: MEDIUM,
        implementation_type: CONFIG_RULE,
        implementation_identifier: Some("IAM_PASSWORD_POLICY"),
        control_tower_identifier: None,
        designators: &["CONFIG.IAM.DT.1"],
        scope: GLOBAL,
        deployable_regions: NO_REGIONS,
        // The strength thresholds are the implementing rule's input parameters; see
        // `CONFIG_RULE_PARAMETERS`.
        parameters: NO_PARAMETERS,
        // The account's password policy has no resource type of its own.
        governed_resources: NO_GOVERNED_RESOURCES,
        governed_providers: AWS,
        implements: "password-strength-enforcement",
        frameworks: &[
            // "Ensure IAM password policy requires a minimum length of 14 or greater".
            (CIS_AWS_BENCHMARK_V3_0, "1.8"),
            // "Authenticator management".
            (NIST_SP_800_53_R5, "IA-5"),
        ],
    },
    ControlSeed {
        slug: "encrypted-volumes",
        name: "Enable encryption for EBS volumes attached to EC2 instances",
        description: "Secure EBS volumes attached to EC2 instances through encryption.",
        behavior: DETECTIVE,
        severity: MEDIUM,
        implementation_type: CONFIG_RULE,
        implementation_identifier: Some("ENCRYPTED_VOLUMES"),
        control_tower_identifier: Some("AWS-GR_ENCRYPTED_VOLUMES"),
        designators: NO_DESIGNATORS,
        scope: REGIONAL,
        deployable_regions: GENERALLY_AVAILABLE_REGIONS,
        // The KMS key the rule checks volumes against is its input parameter; see
        // `CONFIG_RULE_PARAMETERS`.
        parameters: NO_PARAMETERS,
        governed_resources: &["AWS::EC2::Volume"],
        governed_providers: AWS,
        implements: "encrypt-data-at-rest-in-storage-services",
        frameworks: &[
            // "Ensure EBS volume encryption is enabled".
            (CIS_AWS_BENCHMARK_V3_0, "2.2.1"),
            // "Protection of information at rest".
            (NIST_SP_800_53_R5, "SC-28"),
        ],
    },
    ControlSeed {
        slug: "vpc-flow-logs-enabled",
        name: "Checks if Amazon Virtual Private Cloud (Amazon VPC) flow logs are found \
               and enabled for all Amazon VPCs",
        description: "Checks if Amazon Virtual Private Cloud (Amazon VPC) flow logs are \
                      found and enabled for all Amazon VPCs. The rule is NON_COMPLIANT \
                      if flow logs are not enabled for at least one Amazon VPC.",
        behavior: DETECTIVE,
        severity: MEDIUM,
        implementation_type: CONFIG_RULE,
        implementation_identifier: Some("VPC_FLOW_LOGS_ENABLED"),
        control_tower_identifier: None,
        designators: &["CONFIG.EC2.DT.48"],
        scope: REGIONAL,
        deployable_regions: GENERALLY_AVAILABLE_REGIONS,
        // Parameterised by the implementing rule; see `CONFIG_RULE_PARAMETERS`.
        parameters: NO_PARAMETERS,
        governed_resources: &["AWS::EC2::VPC"],
        governed_providers: AWS,
        implements: "data-egress-monitoring",
        frameworks: &[
            // "Ensure VPC flow logging is enabled in all VPCs".
            (CIS_AWS_BENCHMARK_V3_0, "3.9"),
            // "Audit record generation".
            (NIST_SP_800_53_R5, "AU-12"),
        ],
    },
    ControlSeed {
        slug: "ec2-instance-detailed-monitoring-enabled",
        name: "Checks whether detailed monitoring is enabled for EC2 instances",
        // The implementing AWS Config managed rule's own published description, which is
        // what the catalog publishes for a control it publishes under a `CONFIG.…`
        // designator.
        description: "Checks whether detailed monitoring is enabled for EC2 instances.",
        behavior: DETECTIVE,
        severity: MEDIUM,
        implementation_type: CONFIG_RULE,
        implementation_identifier: Some("EC2_INSTANCE_DETAILED_MONITORING_ENABLED"),
        control_tower_identifier: None,
        designators: &["CONFIG.EC2.DT.1"],
        scope: REGIONAL,
        deployable_regions: GENERALLY_AVAILABLE_REGIONS,
        parameters: NO_PARAMETERS,
        governed_resources: &["AWS::EC2::Instance"],
        governed_providers: AWS,
        implements: "asset-performance-management",
        frameworks: NO_FRAMEWORK_ITEMS,
    },
    ControlSeed {
        slug: "incoming-ssh-disabled",
        name: "Disallow Internet connection via SSH",
        // Control Tower publishes this control and the one over remote console ports it
        // sits beside with one description, naming the service each removes connectivity
        // to.
        description: "Reduces a server's exposure to risk by removing unfettered \
                      connectivity to remote console services such as SSH.",
        behavior: DETECTIVE,
        severity: HIGH,
        implementation_type: CONFIG_RULE,
        implementation_identifier: Some("INCOMING_SSH_DISABLED"),
        control_tower_identifier: Some("AWS-GR_RESTRICTED_SSH"),
        designators: NO_DESIGNATORS,
        scope: REGIONAL,
        deployable_regions: GENERALLY_AVAILABLE_REGIONS,
        parameters: NO_PARAMETERS,
        governed_resources: &["AWS::EC2::SecurityGroup"],
        governed_providers: AWS,
        implements: "restrict-administrative-port-exposure",
        frameworks: &[
            // "Ensure no security group allows ingress from 0.0.0.0/0 to port 22".
            (CIS_AWS_BENCHMARK_V3_0, "5.2"),
            // "Boundary protection".
            (NIST_SP_800_53_R5, "SC-7"),
        ],
    },
    ControlSeed {
        slug: "rds-snapshots-public-prohibited",
        name: "Disallow public access to RDS database snapshots",
        description: "Secure your Amazon RDS database snapshots by disallowing public \
                      access.",
        behavior: DETECTIVE,
        severity: CRITICAL,
        implementation_type: CONFIG_RULE,
        implementation_identifier: Some("RDS_SNAPSHOTS_PUBLIC_PROHIBITED"),
        control_tower_identifier: Some("AWS-GR_RDS_SNAPSHOTS_PUBLIC_PROHIBITED"),
        designators: NO_DESIGNATORS,
        scope: REGIONAL,
        deployable_regions: GENERALLY_AVAILABLE_REGIONS,
        parameters: NO_PARAMETERS,
        // The catalog publishes no available resource type for the snapshot sharing this
        // control governs.
        governed_resources: NO_GOVERNED_RESOURCES,
        governed_providers: AWS,
        implements: "restrict-public-access",
        frameworks: &[
            // "Access enforcement".
            (NIST_SP_800_53_R5, "AC-3"),
            // "Protections against data leaks are implemented".
            (NIST_CSF_V1_1, "PR.DS-5"),
        ],
    },
    ControlSeed {
        slug: "rds-storage-encrypted",
        name: "Disallow RDS database instances without storage encryption",
        description: "Secure your Amazon RDS database instances at rest by encrypting the \
                      underlying storage for database instances and their automated \
                      backups, Read Replicas, and snapshots.",
        behavior: DETECTIVE,
        severity: MEDIUM,
        implementation_type: CONFIG_RULE,
        implementation_identifier: Some("RDS_STORAGE_ENCRYPTED"),
        control_tower_identifier: Some("AWS-GR_RDS_STORAGE_ENCRYPTED"),
        designators: NO_DESIGNATORS,
        scope: REGIONAL,
        deployable_regions: GENERALLY_AVAILABLE_REGIONS,
        parameters: NO_PARAMETERS,
        governed_resources: &["AWS::RDS::DBInstance"],
        governed_providers: AWS,
        implements: "encrypt-data-at-rest-in-storage-services",
        frameworks: &[
            // "Data-at-rest is protected".
            (NIST_CSF_V1_1, "PR.DS-1"),
            // "Protection of information at rest".
            (NIST_SP_800_53_R5, "SC-28"),
        ],
    },
    ControlSeed {
        slug: "guardduty-enabled-centralized",
        name: "Checks if Amazon GuardDuty is enabled in your AWS account and AWS Region",
        description: "Checks if Amazon GuardDuty is enabled in your AWS account and AWS \
                      Region. If you provide an AWS account for centralization, the rule \
                      evaluates the GuardDuty results in the centralized account. The rule \
                      is COMPLIANT when GuardDuty is enabled.",
        behavior: DETECTIVE,
        severity: HIGH,
        implementation_type: CONFIG_RULE,
        implementation_identifier: Some("GUARDDUTY_ENABLED_CENTRALIZED"),
        control_tower_identifier: None,
        designators: &["CONFIG.GUARDDUTY.DT.9"],
        scope: REGIONAL,
        deployable_regions: GENERALLY_AVAILABLE_REGIONS,
        parameters: NO_PARAMETERS,
        governed_resources: &["AWS::GuardDuty::Detector"],
        governed_providers: AWS,
        implements: "intrusion-detection-deployment",
        frameworks: &[
            // "System monitoring".
            (NIST_SP_800_53_R5, "SI-4"),
            // "Ensure GuardDuty is enabled".
            (CIS_AWS_BENCHMARK_V3_0, "3.8"),
        ],
    },
    ControlSeed {
        slug: "cloudwatch-log-group-encrypted",
        // The implementing rule's own published title and description, reproduced as the
        // rule publishes them (its wording, not this crate's).
        name: "Checks if Amazon CloudWatch Log Groups are encrypted with any AWS KMS key \
               or a specified AWS KMS key Id",
        description: "Checks if Amazon CloudWatch Log Groups are encrypted with any AWS \
                      KMS key or a specified AWS KMS key Id. The rule is NON_COMPLIANT if \
                      a CloudWatch Log Group is not encrypted with a KMS key or is \
                      encrypted with a KMS key not supplied in the rule parameter.",
        behavior: DETECTIVE,
        severity: MEDIUM,
        implementation_type: CONFIG_RULE,
        implementation_identifier: Some("CLOUDWATCH_LOG_GROUP_ENCRYPTED"),
        control_tower_identifier: None,
        designators: &["CONFIG.LOGS.DT.1"],
        scope: REGIONAL,
        deployable_regions: GENERALLY_AVAILABLE_REGIONS,
        parameters: NO_PARAMETERS,
        governed_resources: &["AWS::Logs::LogGroup"],
        governed_providers: AWS,
        implements: "log-integrity-protection",
        frameworks: &[
            // "Protection of audit information".
            (NIST_SP_800_53_R5, "AU-9"),
            // "Data-at-rest is protected".
            (NIST_CSF_V1_1, "PR.DS-1"),
        ],
    },
    // --- Security Hub controls ----------------------------------------------
    ControlSeed {
        slug: "securityhub-s3-4",
        name: "S3 general purpose buckets should have server-side encryption enabled",
        description: "Checks whether an Amazon S3 bucket has default server-side encryption \
                      enabled, and reports the finding in Security Hub.",
        behavior: DETECTIVE,
        severity: MEDIUM,
        implementation_type: SECURITY_HUB_CONTROL,
        implementation_identifier: Some("S3.4"),
        control_tower_identifier: None,
        designators: &["SH.S3.4"],
        scope: REGIONAL,
        deployable_regions: GENERALLY_AVAILABLE_REGIONS,
        parameters: NO_PARAMETERS,
        governed_resources: &["AWS::S3::Bucket"],
        governed_providers: AWS,
        implements: "encrypt-data-at-rest-in-storage-services",
        frameworks: &[
            // "Protection of information at rest".
            (NIST_SP_800_53_R5, "SC-28"),
            // "Primary account number is rendered unreadable wherever it is stored".
            (PCI_DSS_V4_0, "3.5.1"),
        ],
    },
    ControlSeed {
        slug: "securityhub-cloudtrail-1",
        name: "CloudTrail should be enabled and configured with at least one multi-Region \
               trail that includes read and write management events",
        description: "This AWS control checks that there is at least one multi-region AWS \
                      CloudTrail trail includes read and write management events.",
        behavior: DETECTIVE,
        severity: HIGH,
        implementation_type: SECURITY_HUB_CONTROL,
        implementation_identifier: Some("CloudTrail.1"),
        control_tower_identifier: None,
        designators: &["SH.CloudTrail.1"],
        scope: REGIONAL,
        deployable_regions: GENERALLY_AVAILABLE_REGIONS,
        parameters: NO_PARAMETERS,
        governed_resources: &["AWS::CloudTrail::Trail"],
        governed_providers: AWS,
        implements: "enable-centralized-logging",
        frameworks: &[
            // "Ensure CloudTrail is enabled in all Regions".
            (CIS_AWS_BENCHMARK_V3_0, "3.1"),
            // "Event logging".
            (NIST_SP_800_53_R5, "AU-2"),
        ],
    },
    ControlSeed {
        slug: "securityhub-iam-4",
        name: "IAM root user access key should not exist",
        // The published sentence the catalog serves for this control, reproduced as the
        // catalog publishes it rather than summarised here.
        description: "This AWS control checks whether the root user access key is \
                      available.",
        behavior: DETECTIVE,
        severity: CRITICAL,
        implementation_type: SECURITY_HUB_CONTROL,
        implementation_identifier: Some("IAM.4"),
        control_tower_identifier: None,
        designators: &["SH.IAM.4"],
        scope: GLOBAL,
        deployable_regions: NO_REGIONS,
        parameters: NO_PARAMETERS,
        // What this control governs is the Amazon Web Services account's own root user
        // credentials, which no available resource type represents, so the catalog
        // publishes the list empty: "If GovernedResources cannot be represented by
        // available resource types, it’s returned as an empty list"
        // (`smithy_query(query="shape", shape="ControlSummary")`).
        governed_resources: NO_GOVERNED_RESOURCES,
        governed_providers: AWS,
        implements: "privileged-access-approval",
        frameworks: &[
            // "Ensure no 'root' user account access key exists".
            (CIS_AWS_BENCHMARK_V3_0, "1.4"),
            // "Least privilege".
            (NIST_SP_800_53_R5, "AC-6"),
        ],
    },
    ControlSeed {
        slug: "securityhub-iam-5",
        name: "MFA should be enabled for all IAM users that have a console password",
        description: "This AWS control checks whether AWS Multi-Factor Authentication \
                      (MFA) is enabled for all AWS Identity and Access Management (IAM) \
                      users that use a console password.",
        behavior: DETECTIVE,
        severity: MEDIUM,
        implementation_type: SECURITY_HUB_CONTROL,
        implementation_identifier: Some("IAM.5"),
        control_tower_identifier: None,
        designators: &["SH.IAM.5"],
        scope: REGIONAL,
        deployable_regions: US_REGIONS,
        parameters: NO_PARAMETERS,
        governed_resources: &["AWS::IAM::User"],
        governed_providers: AWS,
        implements: "require-multi-factor-authentication",
        frameworks: &[
            // "Identification and authentication (organizational users)".
            (NIST_SP_800_53_R5, "IA-2"),
            // "Multi-factor authentication is implemented for all access".
            (PCI_DSS_V4_0, "8.4.2"),
        ],
    },
    ControlSeed {
        slug: "gcp-storage-bucket-uniform-access",
        name: "Require uniform bucket-level access on Cloud Storage buckets",
        description: "Checks whether a Google Cloud Storage bucket enforces uniform \
                      bucket-level access.",
        behavior: DETECTIVE,
        severity: MEDIUM,
        implementation_type: SECURITY_HUB_CONTROL,
        implementation_identifier: Some("GCP.Storage.1"),
        control_tower_identifier: None,
        designators: &["SH.GCP.Storage.1"],
        scope: REGIONAL,
        deployable_regions: US_REGIONS,
        parameters: NO_PARAMETERS,
        // A non-Amazon Web Services control's resources are "represented in a
        // provider-specific format".
        governed_resources: &["storage.googleapis.com/Bucket"],
        governed_providers: GCP,
        implements: "restrict-public-access",
        frameworks: &[
            // "Ensure that uniform bucket-level access is enabled on Cloud Storage
            // buckets".
            (CIS_GCP_BENCHMARK_V2_0, "5.2"),
            // "Access enforcement".
            (NIST_SP_800_53_R5, "AC-3"),
        ],
    },
    // --- service control policies: preventive controls ----------------------
    ControlSeed {
        slug: "restrict-root-user-access-keys",
        name: "Disallow the creation of access keys for the root user",
        description: "Prevents the creation of access keys for the Amazon Web Services \
                      account root user.",
        behavior: PREVENTIVE,
        severity: CRITICAL,
        implementation_type: SERVICE_CONTROL_POLICY,
        // A service control policy is the control; it carries no identifier of its own.
        implementation_identifier: None,
        control_tower_identifier: Some("AWS-GR_RESTRICT_ROOT_USER_ACCESS_KEYS"),
        designators: &["CT.IAM.PV.1"],
        // A control implemented by a service control policy governs every Region.
        scope: GLOBAL,
        deployable_regions: NO_REGIONS,
        parameters: &[(EXEMPTED_PRINCIPAL_ARNS, OPTIONAL)],
        // The root user's own credentials, which no available resource type represents;
        // see the Security Hub control over the same subject above.
        governed_resources: NO_GOVERNED_RESOURCES,
        governed_providers: AWS,
        implements: "privileged-access-approval",
        frameworks: &[
            // "Ensure no 'root' user account access key exists".
            (CIS_AWS_BENCHMARK_V3_0, "1.4"),
            // "Least privilege".
            (NIST_SP_800_53_R5, "AC-6"),
            // "Group, shared and generic accounts are managed".
            (PCI_DSS_V4_0, "8.2.2"),
        ],
    },
    ControlSeed {
        slug: "deny-public-rds-snapshots",
        name: "Disallow public Amazon RDS database snapshots",
        description: "Prevents an Amazon RDS database snapshot from being shared publicly.",
        behavior: PREVENTIVE,
        severity: HIGH,
        implementation_type: SERVICE_CONTROL_POLICY,
        implementation_identifier: None,
        control_tower_identifier: None,
        designators: &["CT.RDS.PV.4"],
        scope: GLOBAL,
        deployable_regions: NO_REGIONS,
        parameters: NO_PARAMETERS,
        governed_resources: &["AWS::RDS::DBSnapshot"],
        governed_providers: AWS,
        implements: "restrict-public-access",
        frameworks: &[
            // "Access enforcement".
            (NIST_SP_800_53_R5, "AC-3"),
            // "Configure data access control lists".
            (CIS_CONTROLS_V8_0, "3.3"),
        ],
    },
    ControlSeed {
        slug: "disallow-cloudtrail-changes",
        name: "Disallow changes to CloudTrail configuration",
        description: "Prevents an account from stopping, deleting or reconfiguring the \
                      CloudTrail trail the organization set up.",
        behavior: PREVENTIVE,
        severity: HIGH,
        implementation_type: SERVICE_CONTROL_POLICY,
        implementation_identifier: None,
        control_tower_identifier: Some("AWS-GR_CLOUDTRAIL_CHANGE_PROHIBITED"),
        designators: NO_DESIGNATORS,
        scope: GLOBAL,
        deployable_regions: NO_REGIONS,
        parameters: NO_PARAMETERS,
        governed_resources: &["AWS::CloudTrail::Trail"],
        governed_providers: AWS,
        implements: "log-integrity-protection",
        frameworks: &[
            // "Protection of audit information".
            (NIST_SP_800_53_R5, "AU-9"),
            // "Audit/log records are determined, documented, implemented and
            // reviewed".
            (NIST_CSF_V1_1, "PR.PT-1"),
        ],
    },
    ControlSeed {
        slug: "disallow-log-archive-deletion",
        name: "Disallow deletion of the log archive bucket",
        description: "Prevents deletion of the Amazon S3 bucket the organization archives \
                      its audit logs in.",
        behavior: PREVENTIVE,
        severity: HIGH,
        implementation_type: SERVICE_CONTROL_POLICY,
        implementation_identifier: None,
        control_tower_identifier: Some("AWS-GR_AUDIT_BUCKET_DELETION_PROHIBITED"),
        designators: NO_DESIGNATORS,
        scope: GLOBAL,
        deployable_regions: NO_REGIONS,
        parameters: NO_PARAMETERS,
        governed_resources: &["AWS::S3::Bucket"],
        governed_providers: AWS,
        implements: "log-retention-enforcement",
        frameworks: &[
            // "Audit record retention".
            (NIST_SP_800_53_R5, "AU-11"),
            // "Audit log history is retained for at least 12 months".
            (PCI_DSS_V4_0, "10.5.1"),
        ],
    },
    // --- CloudFormation hooks: proactive controls ---------------------------
    ControlSeed {
        slug: "require-encrypted-ebs-volumes",
        name: "Require encryption on Amazon EBS volumes at creation",
        description: "Rejects a CloudFormation deployment that would create an unencrypted \
                      Amazon EBS volume.",
        behavior: PROACTIVE,
        severity: MEDIUM,
        implementation_type: CLOUDFORMATION_HOOK,
        implementation_identifier: Some("AWS.EBS.EncryptionHook"),
        control_tower_identifier: None,
        designators: NO_DESIGNATORS,
        scope: REGIONAL,
        deployable_regions: &["eu-central-1", "us-east-1"],
        parameters: &[
            (ALLOWED_REGIONS, REQUIRED),
            (EXEMPTED_RESOURCE_ARNS, OPTIONAL),
        ],
        governed_resources: &["AWS::EC2::Volume", "AWS::EC2::Instance"],
        governed_providers: AWS,
        implements: "encrypt-data-at-rest-in-storage-services",
        frameworks: &[
            // "Protection of information at rest".
            (NIST_SP_800_53_R5, "SC-28"),
            // "Ensure EBS volume encryption is enabled".
            (CIS_AWS_BENCHMARK_V3_0, "2.2.1"),
        ],
    },
    ControlSeed {
        slug: "require-rds-storage-encryption-at-creation",
        name: "Require storage encryption on Amazon RDS instances at creation",
        description: "Rejects a CloudFormation deployment that would create an Amazon RDS \
                      database instance with unencrypted storage.",
        behavior: PROACTIVE,
        severity: MEDIUM,
        implementation_type: CLOUDFORMATION_HOOK,
        implementation_identifier: Some("AWS.RDS.StorageEncryptionHook"),
        control_tower_identifier: None,
        designators: NO_DESIGNATORS,
        scope: REGIONAL,
        deployable_regions: GENERALLY_AVAILABLE_REGIONS,
        parameters: NO_PARAMETERS,
        governed_resources: &["AWS::RDS::DBInstance"],
        governed_providers: AWS,
        implements: "encrypt-data-at-rest-in-storage-services",
        frameworks: &[
            // "Data-at-rest is protected".
            (NIST_CSF_V1_1, "PR.DS-1"),
            // "Protection of information at rest".
            (NIST_SP_800_53_R5, "SC-28"),
        ],
    },
    // --- AWS Config managed rules: the rest of the Config-implemented catalog ----
    //
    // The catalog publishes a control for EVERY managed rule the implementing surface
    // governs a resource with, so the rows below carry the rest of the AWS Config managed
    // rules this catalog publishes controls over, each with that rule's own published
    // identifier, title and description and the resource type it evaluates. A row carries
    // only the designators the catalog records for it: a `CONFIG.…` designator is a
    // lookup key a client joins on, so one the catalog publishes but this crate does not
    // record is left absent rather than invented.
    ControlSeed {
        slug: "acm-certificate-expiration-check",
        name: "Checks whether ACM Certificates in your account are marked for expiration \
               within the specified number of days",
        // The implementing AWS Config managed rule's own published description, reproduced
        // as the catalog publishes it for a control under a `CONFIG.…` designator: the
        // rule's documented text, whose first sentence is the control's published name.
        description: "Checks whether ACM Certificates in your account are marked for \
                      expiration within the specified number of days. Certificates \
                      provided by ACM are automatically renewed. ACM does not \
                      automatically renew certificates that you import.",
        behavior: DETECTIVE,
        severity: MEDIUM,
        implementation_type: CONFIG_RULE,
        implementation_identifier: Some("ACM_CERTIFICATE_EXPIRATION_CHECK"),
        control_tower_identifier: None,
        designators: &["CONFIG.ACM.DT.3"],
        scope: REGIONAL,
        deployable_regions: GENERALLY_AVAILABLE_REGIONS,
        // Parameterised by the implementing rule; see `CONFIG_RULE_PARAMETERS`.
        parameters: NO_PARAMETERS,
        governed_resources: &["AWS::CertificateManager::Certificate"],
        governed_providers: AWS,
        implements: "certificate-lifecycle-management",
        frameworks: NO_FRAMEWORK_ITEMS,
    },
    ControlSeed {
        slug: "acm-pca-certificate-authority-tagged",
        name: "Checks if AWS Private CA certificate authorities have tags",
        description: "Checks if AWS Private CA certificate authorities have tags. \
                      Optionally, you can specify tag keys. The rule is NON_COMPLIANT if \
                      there are no tags or if the specified tag keys are not present. The \
                      rule does not check for tags starting with 'aws:'.",
        behavior: DETECTIVE,
        severity: LOW,
        implementation_type: CONFIG_RULE,
        implementation_identifier: Some("ACMPCA_CERTIFICATE_AUTHORITY_TAGGED"),
        control_tower_identifier: None,
        designators: &["CONFIG.ACM-PCA.DT.2"],
        scope: REGIONAL,
        deployable_regions: GENERALLY_AVAILABLE_REGIONS,
        // Parameterised by the implementing rule; see `CONFIG_RULE_PARAMETERS`.
        parameters: NO_PARAMETERS,
        governed_resources: &["AWS::ACMPCA::CertificateAuthority"],
        governed_providers: AWS,
        implements: "maintain-an-asset-inventory",
        frameworks: NO_FRAMEWORK_ITEMS,
    },
    ControlSeed {
        slug: "cognito-user-pool-advanced-security-enabled",
        name: "Checks if an Amazon Cognito user pool has advanced security enabled",
        description: "Checks if an Amazon Cognito user pool has advanced security \
                      enabled. The rule is NON_COMPLIANT if advanced security is not \
                      enabled. Optionally, you can specify an advanced security mode for \
                      the rule to check.",
        behavior: DETECTIVE,
        severity: MEDIUM,
        implementation_type: CONFIG_RULE,
        implementation_identifier: Some("COGNITO_USER_POOL_ADVANCED_SECURITY_ENABLED"),
        control_tower_identifier: None,
        designators: &["CONFIG.COGNITO-IDP.DT.1"],
        scope: REGIONAL,
        // Advanced security on a user pool is offered in fewer Regions than the
        // partition, so the catalog publishes this control's own availability rather
        // than the partition-wide set.
        deployable_regions: COGNITO_ADVANCED_SECURITY_REGIONS,
        parameters: NO_PARAMETERS,
        governed_resources: &["AWS::Cognito::UserPool"],
        governed_providers: AWS,
        implements: "intrusion-detection-deployment",
        frameworks: NO_FRAMEWORK_ITEMS,
    },
    ControlSeed {
        slug: "efs-encrypted-check",
        name: "Checks if Amazon Elastic File System (Amazon EFS) file systems are \
               encrypted",
        description: "Checks if Amazon Elastic File System (Amazon EFS) file systems are \
                      encrypted. The rule is NON_COMPLIANT if the file system is not \
                      encrypted, or if it is encrypted with a key other than the one the \
                      rule's input parameter names.",
        behavior: DETECTIVE,
        severity: MEDIUM,
        implementation_type: CONFIG_RULE,
        implementation_identifier: Some("EFS_ENCRYPTED_CHECK"),
        control_tower_identifier: None,
        designators: &["CONFIG.ELASTICFILESYSTEM.DT.8"],
        scope: REGIONAL,
        deployable_regions: GENERALLY_AVAILABLE_REGIONS,
        parameters: NO_PARAMETERS,
        governed_resources: &["AWS::EFS::FileSystem"],
        governed_providers: AWS,
        implements: "encrypt-data-at-rest-in-storage-services",
        frameworks: NO_FRAMEWORK_ITEMS,
    },
    ControlSeed {
        slug: "dynamodb-table-encrypted-kms",
        name: "Checks if the Amazon DynamoDB table is encrypted with AWS Key Management \
               Service (KMS)",
        description: "Checks if the Amazon DynamoDB table is encrypted with AWS Key \
                      Management Service (KMS). The rule is NON_COMPLIANT if the Amazon \
                      DynamoDB table is not encrypted with AWS KMS. The rule is also \
                      NON_COMPLIANT if the encrypted AWS KMS key is not present in \
                      kmsKeyArns input parameter.",
        behavior: DETECTIVE,
        severity: MEDIUM,
        implementation_type: CONFIG_RULE,
        implementation_identifier: Some("DYNAMODB_TABLE_ENCRYPTED_KMS"),
        control_tower_identifier: None,
        designators: &["CONFIG.DYNAMODB.DT.4"],
        scope: REGIONAL,
        deployable_regions: GENERALLY_AVAILABLE_REGIONS,
        parameters: NO_PARAMETERS,
        governed_resources: &["AWS::DynamoDB::Table"],
        governed_providers: AWS,
        implements: "encrypt-data-at-rest-in-storage-services",
        frameworks: NO_FRAMEWORK_ITEMS,
    },
    ControlSeed {
        slug: "sns-encrypted-kms",
        name: "Checks if SNS topics are encrypted with AWS Key Management Service (AWS \
               KMS)",
        description: "Checks if SNS topics are encrypted with AWS Key Management Service \
                      (AWS KMS). The rule is NON_COMPLIANT if an SNS topic is not \
                      encrypted with AWS KMS. Optionally, specify the key ARNs, the alias \
                      ARNs, the alias name, or the key IDs for the rule to check.",
        behavior: DETECTIVE,
        severity: MEDIUM,
        implementation_type: CONFIG_RULE,
        implementation_identifier: Some("SNS_ENCRYPTED_KMS"),
        control_tower_identifier: None,
        designators: &["CONFIG.SNS.DT.2"],
        scope: REGIONAL,
        deployable_regions: GENERALLY_AVAILABLE_REGIONS,
        parameters: NO_PARAMETERS,
        governed_resources: &["AWS::SNS::Topic"],
        governed_providers: AWS,
        implements: "encrypt-data-at-rest-in-storage-services",
        frameworks: NO_FRAMEWORK_ITEMS,
    },
    ControlSeed {
        slug: "rds-snapshot-encrypted",
        name: "Checks if Amazon Relational Database Service (Amazon RDS) DB snapshots are \
               encrypted",
        description: "Checks if Amazon Relational Database Service (Amazon RDS) DB \
                      snapshots are encrypted. The rule is NON_COMPLIANT if the Amazon \
                      RDS DB snapshots are not encrypted.",
        behavior: DETECTIVE,
        severity: MEDIUM,
        implementation_type: CONFIG_RULE,
        implementation_identifier: Some("RDS_SNAPSHOT_ENCRYPTED"),
        control_tower_identifier: None,
        designators: &["CONFIG.RDS.DT.39"],
        scope: REGIONAL,
        deployable_regions: GENERALLY_AVAILABLE_REGIONS,
        parameters: NO_PARAMETERS,
        // The catalog publishes no governed resource type for this control: its recorded
        // response carries the empty list.
        governed_resources: NO_GOVERNED_RESOURCES,
        governed_providers: AWS,
        implements: "encrypt-data-at-rest-in-storage-services",
        frameworks: NO_FRAMEWORK_ITEMS,
    },
    ControlSeed {
        slug: "opensearch-encrypted-at-rest",
        name: "Checks if Amazon OpenSearch Service domains have encryption at rest \
               enabled",
        description: "Checks if Amazon OpenSearch Service domains have encryption at rest \
                      enabled. The rule is NON_COMPLIANT if the domain does not have \
                      encryption at rest enabled.",
        behavior: DETECTIVE,
        severity: MEDIUM,
        implementation_type: CONFIG_RULE,
        implementation_identifier: Some("OPENSEARCH_ENCRYPTED_AT_REST"),
        control_tower_identifier: None,
        designators: &["CONFIG.OPENSEARCH.DT.9"],
        scope: REGIONAL,
        deployable_regions: GENERALLY_AVAILABLE_REGIONS,
        parameters: NO_PARAMETERS,
        governed_resources: &["AWS::OpenSearchService::Domain"],
        governed_providers: AWS,
        implements: "encrypt-data-at-rest-in-storage-services",
        frameworks: NO_FRAMEWORK_ITEMS,
    },
    ControlSeed {
        slug: "redshift-cluster-configuration-check",
        name: "Checks whether Amazon Redshift clusters have the specified settings",
        description: "Checks whether Amazon Redshift clusters have the specified \
                      settings. The rule is NON_COMPLIANT if the cluster is not \
                      encrypted or is not logging audit information, or if its node type \
                      is not one the rule's input parameters name.",
        behavior: DETECTIVE,
        severity: MEDIUM,
        implementation_type: CONFIG_RULE,
        implementation_identifier: Some("REDSHIFT_CLUSTER_CONFIGURATION_CHECK"),
        control_tower_identifier: None,
        designators: NO_DESIGNATORS,
        scope: REGIONAL,
        deployable_regions: GENERALLY_AVAILABLE_REGIONS,
        parameters: NO_PARAMETERS,
        governed_resources: &["AWS::Redshift::Cluster"],
        governed_providers: AWS,
        implements: "encrypt-data-at-rest-in-storage-services",
        frameworks: NO_FRAMEWORK_ITEMS,
    },
    ControlSeed {
        slug: "redshift-require-tls-ssl",
        name: "Checks if Amazon Redshift clusters require TLS/SSL encryption to connect \
               to SQL clients",
        description: "Checks if Amazon Redshift clusters require TLS/SSL encryption to \
                      connect to SQL clients. The rule is NON_COMPLIANT if any Amazon \
                      Redshift cluster has parameter require_SSL not set to true.",
        behavior: DETECTIVE,
        severity: MEDIUM,
        implementation_type: CONFIG_RULE,
        implementation_identifier: Some("REDSHIFT_REQUIRE_TLS_SSL"),
        control_tower_identifier: None,
        designators: &["CONFIG.REDSHIFT.DT.9"],
        scope: REGIONAL,
        deployable_regions: GENERALLY_AVAILABLE_REGIONS,
        parameters: NO_PARAMETERS,
        // The rule reads the cluster's parameter group to decide the cluster's
        // compliance, so the catalog publishes the control as governing both resource
        // types.
        governed_resources: &[
            "AWS::Redshift::Cluster",
            "AWS::Redshift::ClusterParameterGroup",
        ],
        governed_providers: AWS,
        implements: "enforce-encryption-in-transit",
        frameworks: NO_FRAMEWORK_ITEMS,
    },
    ControlSeed {
        slug: "elb-tls-https-listeners-only",
        name: "Checks if your Classic Load Balancer is configured with SSL or HTTPS \
               listeners",
        description: "Checks if your Classic Load Balancer is configured with SSL or \
                      HTTPS listeners. The rule is NON_COMPLIANT if a listener is not \
                      configured with SSL or HTTPS.",
        behavior: DETECTIVE,
        severity: MEDIUM,
        implementation_type: CONFIG_RULE,
        implementation_identifier: Some("ELB_TLS_HTTPS_LISTENERS_ONLY"),
        control_tower_identifier: None,
        designators: &["CONFIG.ELASTICLOADBALANCING.DT.16"],
        scope: REGIONAL,
        deployable_regions: GENERALLY_AVAILABLE_REGIONS,
        parameters: NO_PARAMETERS,
        governed_resources: &["AWS::ElasticLoadBalancing::LoadBalancer"],
        governed_providers: AWS,
        implements: "enforce-encryption-in-transit",
        frameworks: NO_FRAMEWORK_ITEMS,
    },
    ControlSeed {
        slug: "elbv2-acm-certificate-required",
        name: "Checks if Application Load Balancers and Network Load Balancers are \
               configured to use certificates from Amazon Certificate Manager (ACM)",
        description: "Checks if Application Load Balancers and Network Load Balancers \
                      are configured to use certificates from Amazon Certificate Manager \
                      (ACM). This rule is NON_COMPLIANT if at least 1 load balancer is \
                      configured without a certificate from ACM.",
        behavior: DETECTIVE,
        severity: MEDIUM,
        implementation_type: CONFIG_RULE,
        implementation_identifier: Some("ELBV2_ACM_CERTIFICATE_REQUIRED"),
        control_tower_identifier: None,
        designators: &["CONFIG.ELASTICLOADBALANCING.DT.3"],
        scope: REGIONAL,
        deployable_regions: GENERALLY_AVAILABLE_REGIONS,
        parameters: NO_PARAMETERS,
        governed_resources: &["AWS::ElasticLoadBalancingV2::LoadBalancer"],
        governed_providers: AWS,
        implements: "enforce-encryption-in-transit",
        frameworks: NO_FRAMEWORK_ITEMS,
    },
    ControlSeed {
        slug: "s3-bucket-logging-enabled",
        name: "Checks if logging is enabled for your S3 buckets",
        description: "Checks if logging is enabled for your S3 buckets. The rule is \
                      NON_COMPLIANT if logging is not enabled.",
        behavior: DETECTIVE,
        severity: MEDIUM,
        implementation_type: CONFIG_RULE,
        implementation_identifier: Some("S3_BUCKET_LOGGING_ENABLED"),
        control_tower_identifier: None,
        designators: &["CONFIG.S3.DT.14"],
        scope: REGIONAL,
        deployable_regions: GENERALLY_AVAILABLE_REGIONS,
        parameters: NO_PARAMETERS,
        governed_resources: &["AWS::S3::Bucket"],
        governed_providers: AWS,
        implements: "enable-centralized-logging",
        frameworks: NO_FRAMEWORK_ITEMS,
    },
    ControlSeed {
        slug: "cloud-trail-cloud-watch-logs-enabled",
        name: "Checks if AWS CloudTrail trails are configured to send logs to CloudWatch \
               logs",
        description: "Checks if AWS CloudTrail trails are configured to send logs to \
                      CloudWatch logs. The trail is NON_COMPLIANT if the \
                      CloudWatchLogsLogGroupArn property of the trail is empty.",
        behavior: DETECTIVE,
        severity: LOW,
        implementation_type: CONFIG_RULE,
        implementation_identifier: Some("CLOUD_TRAIL_CLOUD_WATCH_LOGS_ENABLED"),
        control_tower_identifier: None,
        designators: &["CONFIG.CLOUDTRAIL.DT.8"],
        scope: REGIONAL,
        deployable_regions: GENERALLY_AVAILABLE_REGIONS,
        parameters: NO_PARAMETERS,
        governed_resources: &["AWS::CloudTrail::Trail"],
        governed_providers: AWS,
        implements: "enable-centralized-logging",
        frameworks: NO_FRAMEWORK_ITEMS,
    },
    ControlSeed {
        slug: "elb-logging-enabled",
        name: "Checks whether the Application Load Balancers and the Classic Load \
               Balancers have logging enabled",
        description: "Checks whether the Application Load Balancers and the Classic Load \
                      Balancers have logging enabled.",
        behavior: DETECTIVE,
        severity: MEDIUM,
        implementation_type: CONFIG_RULE,
        implementation_identifier: Some("ELB_LOGGING_ENABLED"),
        control_tower_identifier: None,
        designators: &["CONFIG.ELASTICLOADBALANCING.DT.12"],
        scope: REGIONAL,
        deployable_regions: GENERALLY_AVAILABLE_REGIONS,
        parameters: NO_PARAMETERS,
        governed_resources: &[
            "AWS::ElasticLoadBalancing::LoadBalancer",
            "AWS::ElasticLoadBalancingV2::LoadBalancer",
        ],
        governed_providers: AWS,
        implements: "enable-centralized-logging",
        frameworks: NO_FRAMEWORK_ITEMS,
    },
    ControlSeed {
        slug: "api-gw-execution-logging-enabled",
        name: "Checks if all methods in Amazon API Gateway stages have logging enabled",
        description: "Checks if all methods in Amazon API Gateway stages have logging \
                      enabled. The rule is NON_COMPLIANT if logging is not enabled, or if \
                      loggingLevel is neither ERROR nor INFO.",
        behavior: DETECTIVE,
        severity: MEDIUM,
        implementation_type: CONFIG_RULE,
        implementation_identifier: Some("API_GW_EXECUTION_LOGGING_ENABLED"),
        control_tower_identifier: None,
        designators: &["CONFIG.APIGATEWAY.DT.10"],
        scope: REGIONAL,
        deployable_regions: GENERALLY_AVAILABLE_REGIONS,
        parameters: NO_PARAMETERS,
        governed_resources: &["AWS::ApiGateway::Stage", "AWS::ApiGatewayV2::Stage"],
        governed_providers: AWS,
        implements: "enable-centralized-logging",
        frameworks: NO_FRAMEWORK_ITEMS,
    },
    ControlSeed {
        slug: "ec2-instance-no-public-ip",
        name: "Detect whether any Amazon EC2 instance has an associated public IPv4 \
               address",
        description: "Detects whether any Amazon Elastic Compute Cloud (Amazon EC2) \
                      instance has an associated public IPv4 address. The rule is \
                      NON_COMPLIANT if the publicIp field is present in the Amazon EC2 \
                      instance configuration item. This rule applies only to IPv4 \
                      addresses.",
        behavior: DETECTIVE,
        severity: HIGH,
        implementation_type: CONFIG_RULE,
        implementation_identifier: Some("EC2_INSTANCE_NO_PUBLIC_IP"),
        control_tower_identifier: Some("AWS-GR_EC2_INSTANCE_NO_PUBLIC_IP"),
        designators: NO_DESIGNATORS,
        scope: REGIONAL,
        deployable_regions: GENERALLY_AVAILABLE_REGIONS,
        parameters: NO_PARAMETERS,
        governed_resources: &["AWS::EC2::Instance"],
        governed_providers: AWS,
        implements: "restrict-public-access",
        frameworks: NO_FRAMEWORK_ITEMS,
    },
    ControlSeed {
        slug: "rds-instance-public-access-check",
        name: "Disallow public access to RDS database instances",
        // Control Tower's own published description for the control it publishes, which
        // is what the catalog publishes for a control carrying an `AWS-GR_…` designator.
        description: "Secure your Amazon RDS database instances by disallowing public \
                      access.",
        behavior: DETECTIVE,
        severity: CRITICAL,
        implementation_type: CONFIG_RULE,
        implementation_identifier: Some("RDS_INSTANCE_PUBLIC_ACCESS_CHECK"),
        control_tower_identifier: Some("AWS-GR_RDS_INSTANCE_PUBLIC_ACCESS_CHECK"),
        designators: NO_DESIGNATORS,
        scope: REGIONAL,
        deployable_regions: GENERALLY_AVAILABLE_REGIONS,
        parameters: NO_PARAMETERS,
        governed_resources: &["AWS::RDS::DBInstance"],
        governed_providers: AWS,
        implements: "restrict-public-access",
        frameworks: NO_FRAMEWORK_ITEMS,
    },
    ControlSeed {
        slug: "lambda-function-public-access-prohibited",
        name: "Detect whether the AWS Lambda function policy attached to the Lambda \
               resource blocks public access",
        description: "Detects whether the AWS Lambda function policy attached to the \
                      Lambda resource blocks public access. The rule is NON_COMPLIANT if \
                      the AWS Lambda function policy allows public access.",
        behavior: DETECTIVE,
        severity: CRITICAL,
        implementation_type: CONFIG_RULE,
        implementation_identifier: Some("LAMBDA_FUNCTION_PUBLIC_ACCESS_PROHIBITED"),
        control_tower_identifier: Some("AWS-GR_LAMBDA_FUNCTION_PUBLIC_ACCESS_PROHIBITED"),
        designators: NO_DESIGNATORS,
        scope: REGIONAL,
        deployable_regions: GENERALLY_AVAILABLE_REGIONS,
        parameters: NO_PARAMETERS,
        governed_resources: &["AWS::Lambda::Function"],
        governed_providers: AWS,
        implements: "restrict-public-access",
        frameworks: NO_FRAMEWORK_ITEMS,
    },
    ControlSeed {
        slug: "eks-endpoint-no-public-access",
        name: "Detect whether an Amazon EKS endpoint is blocked from public access",
        description: "Detects whether an Amazon Elastic Kubernetes Service (Amazon EKS) \
                      endpoint is blocked from public access. The rule is NON_COMPLIANT \
                      if the Amazon EKS endpoint is publicly accessible.",
        behavior: DETECTIVE,
        severity: HIGH,
        implementation_type: CONFIG_RULE,
        implementation_identifier: Some("EKS_ENDPOINT_NO_PUBLIC_ACCESS"),
        control_tower_identifier: Some("AWS-GR_EKS_ENDPOINT_NO_PUBLIC_ACCESS"),
        designators: NO_DESIGNATORS,
        scope: REGIONAL,
        deployable_regions: GENERALLY_AVAILABLE_REGIONS,
        parameters: NO_PARAMETERS,
        governed_resources: &["AWS::EKS::Cluster"],
        governed_providers: AWS,
        implements: "restrict-public-access",
        frameworks: NO_FRAMEWORK_ITEMS,
    },
    ControlSeed {
        slug: "redshift-cluster-public-access-check",
        name: "Detect whether Amazon Redshift clusters are blocked from public access",
        description: "Detects whether Amazon Redshift clusters are blocked from public \
                      access. The rule is NON_COMPLIANT if the publiclyAccessible field \
                      is true in the cluster configuration item.",
        behavior: DETECTIVE,
        severity: CRITICAL,
        implementation_type: CONFIG_RULE,
        implementation_identifier: Some("REDSHIFT_CLUSTER_PUBLIC_ACCESS_CHECK"),
        control_tower_identifier: Some("AWS-GR_REDSHIFT_CLUSTER_PUBLIC_ACCESS_CHECK"),
        designators: NO_DESIGNATORS,
        scope: REGIONAL,
        deployable_regions: GENERALLY_AVAILABLE_REGIONS,
        parameters: NO_PARAMETERS,
        governed_resources: &["AWS::Redshift::Cluster"],
        governed_providers: AWS,
        implements: "restrict-public-access",
        frameworks: NO_FRAMEWORK_ITEMS,
    },
    ControlSeed {
        slug: "sagemaker-notebook-no-direct-internet-access",
        name: "Detect whether an Amazon SageMaker notebook instance allows direct \
               internet access",
        description: "Detects whether direct internet access is allowed for an Amazon \
                      SageMaker notebook instance. The rule is NON_COMPLIANT if Amazon \
                      SageMaker notebook instances allow direct internet access.",
        behavior: DETECTIVE,
        severity: HIGH,
        implementation_type: CONFIG_RULE,
        implementation_identifier: Some("SAGEMAKER_NOTEBOOK_NO_DIRECT_INTERNET_ACCESS"),
        control_tower_identifier: Some("AWS-GR_SAGEMAKER_NOTEBOOK_NO_DIRECT_INTERNET_ACCESS"),
        designators: NO_DESIGNATORS,
        scope: REGIONAL,
        deployable_regions: US_REGIONS,
        parameters: NO_PARAMETERS,
        governed_resources: &["AWS::SageMaker::NotebookInstance"],
        governed_providers: AWS,
        implements: "restrict-public-access",
        frameworks: NO_FRAMEWORK_ITEMS,
    },
    ControlSeed {
        slug: "ebs-snapshot-public-restorable-check",
        name: "Detect whether Amazon EBS snapshots are restorable by all AWS accounts",
        description: "Detects whether all AWS accounts have access to restore Amazon EBS \
                      snapshots. The rule is NON_COMPLIANT if any snapshots have the \
                      RestorableByUserIds field set to the value All. In that case, the \
                      Amazon EBS snapshots are public.",
        behavior: DETECTIVE,
        severity: CRITICAL,
        implementation_type: CONFIG_RULE,
        implementation_identifier: Some("EBS_SNAPSHOT_PUBLIC_RESTORABLE_CHECK"),
        control_tower_identifier: Some("AWS-GR_EBS_SNAPSHOT_PUBLIC_RESTORABLE_CHECK"),
        designators: NO_DESIGNATORS,
        scope: REGIONAL,
        deployable_regions: GENERALLY_AVAILABLE_REGIONS,
        parameters: NO_PARAMETERS,
        // The catalog publishes the resource type its recorded response names for this
        // control, which is the volume the snapshot was taken from.
        governed_resources: &["AWS::EC2::Volume"],
        governed_providers: AWS,
        implements: "restrict-public-access",
        frameworks: NO_FRAMEWORK_ITEMS,
    },
    ControlSeed {
        slug: "iam-user-unused-credentials-check",
        name: "Checks whether your Amazon Web Services Identity and Access Management \
               (IAM) users have passwords or active access keys that have not been used \
               within the specified number of days",
        description: "Checks whether your Amazon Web Services Identity and Access \
                      Management (IAM) users have passwords or active access keys that \
                      have not been used within the specified number of days. The rule is \
                      NON_COMPLIANT if a credential has gone unused for longer than the \
                      age the rule's input parameter allows.",
        behavior: DETECTIVE,
        severity: MEDIUM,
        implementation_type: CONFIG_RULE,
        implementation_identifier: Some("IAM_USER_UNUSED_CREDENTIALS_CHECK"),
        control_tower_identifier: None,
        designators: NO_DESIGNATORS,
        scope: GLOBAL,
        deployable_regions: NO_REGIONS,
        parameters: NO_PARAMETERS,
        governed_resources: &["AWS::IAM::User"],
        governed_providers: AWS,
        implements: "credential-rotation",
        frameworks: &[
            // "Ensure credentials unused for 45 days or greater are disabled".
            (CIS_AWS_BENCHMARK_V3_0, "1.12"),
        ],
    },
    ControlSeed {
        slug: "secretsmanager-rotation-enabled-check",
        name: "Checks whether AWS Secret Manager secret has rotation enabled",
        description: "Checks whether AWS Secret Manager secret has rotation enabled. If \
                      the maximumAllowedRotationFrequency or the \
                      maximumAllowedRotationFrequencyInHours parameter is specified, the \
                      rotation frequency of the secret is compared with the maximum \
                      allowed frequency.",
        behavior: DETECTIVE,
        severity: MEDIUM,
        implementation_type: CONFIG_RULE,
        implementation_identifier: Some("SECRETSMANAGER_ROTATION_ENABLED_CHECK"),
        control_tower_identifier: None,
        designators: &["CONFIG.SECRETSMANAGER.DT.5"],
        scope: REGIONAL,
        deployable_regions: GENERALLY_AVAILABLE_REGIONS,
        parameters: NO_PARAMETERS,
        governed_resources: &["AWS::SecretsManager::Secret"],
        governed_providers: AWS,
        implements: "credential-rotation",
        frameworks: NO_FRAMEWORK_ITEMS,
    },
    ControlSeed {
        slug: "cmk-backing-key-rotation-enabled",
        name: "Checks if automatic key rotation is enabled for each key and matches to \
               the key ID of the customer created AWS KMS key",
        description: "Checks if automatic key rotation is enabled for each key and \
                      matches to the key ID of the customer created AWS KMS key. The rule \
                      is NON_COMPLIANT if the AWS Config recorder role for a resource \
                      does not have the kms:DescribeKey permission.",
        behavior: DETECTIVE,
        severity: MEDIUM,
        implementation_type: CONFIG_RULE,
        implementation_identifier: Some("CMK_BACKING_KEY_ROTATION_ENABLED"),
        control_tower_identifier: None,
        designators: &["CONFIG.KMS.DT.3"],
        scope: REGIONAL,
        deployable_regions: GENERALLY_AVAILABLE_REGIONS,
        parameters: NO_PARAMETERS,
        governed_resources: &["AWS::KMS::Key"],
        governed_providers: AWS,
        implements: "credential-rotation",
        frameworks: NO_FRAMEWORK_ITEMS,
    },
    ControlSeed {
        slug: "mfa-enabled-for-iam-console-access",
        name: "Disallow console access to IAM users without MFA",
        // Control Tower's own published description for the control it publishes, as for
        // the control over every IAM user above (`iam-user-mfa-enabled`).
        description: "Protect your account by requiring multi-factor authentication (MFA) \
                      for any AWS Identity and Access Management (IAM) user with console \
                      access. MFA adds an extra authentication code on top of a user name \
                      and password.",
        behavior: DETECTIVE,
        severity: MEDIUM,
        implementation_type: CONFIG_RULE,
        implementation_identifier: Some("MFA_ENABLED_FOR_IAM_CONSOLE_ACCESS"),
        control_tower_identifier: Some("AWS-GR_MFA_ENABLED_FOR_IAM_CONSOLE_ACCESS"),
        designators: NO_DESIGNATORS,
        scope: GLOBAL,
        deployable_regions: NO_REGIONS,
        parameters: NO_PARAMETERS,
        governed_resources: &["AWS::IAM::User"],
        governed_providers: AWS,
        implements: "require-multi-factor-authentication",
        frameworks: &[
            // "Ensure MFA is enabled for all IAM users that have a console password".
            (CIS_AWS_BENCHMARK_V3_0, "1.10"),
        ],
    },
    ControlSeed {
        slug: "root-account-hardware-mfa-enabled",
        name: "Checks if your AWS account is enabled to use multi-factor authentication \
               (MFA) hardware device to sign in with root credentials",
        description: "Checks if your AWS account is enabled to use multi-factor \
                      authentication (MFA) hardware device to sign in with root \
                      credentials. The rule is NON_COMPLIANT if any virtual MFA devices \
                      are permitted for signing in with root credentials.",
        behavior: DETECTIVE,
        severity: CRITICAL,
        implementation_type: CONFIG_RULE,
        implementation_identifier: Some("ROOT_ACCOUNT_HARDWARE_MFA_ENABLED"),
        control_tower_identifier: None,
        designators: &["CONFIG.ACCOUNT.DT.2"],
        scope: GLOBAL,
        deployable_regions: NO_REGIONS,
        parameters: NO_PARAMETERS,
        // The root user's own credentials, which no available resource type represents.
        governed_resources: NO_GOVERNED_RESOURCES,
        governed_providers: AWS,
        implements: "require-multi-factor-authentication",
        frameworks: &[
            // "Ensure hardware MFA is enabled for the root user account".
            (CIS_AWS_BENCHMARK_V3_0, "1.6"),
        ],
    },
    ControlSeed {
        slug: "iam-policy-no-statements-with-admin-access",
        name: "Checks whether the default version of AWS Identity and Access Management \
               (IAM) policies do not have administrator access",
        description: "Checks whether the default version of AWS Identity and Access \
                      Management (IAM) policies do not have administrator access. If any \
                      statement has \"Effect\": \"Allow\" with \"Action\": \"*\" over \
                      \"Resource\": \"*\", the rule is non-compliant.",
        behavior: DETECTIVE,
        severity: HIGH,
        implementation_type: CONFIG_RULE,
        implementation_identifier: Some("IAM_POLICY_NO_STATEMENTS_WITH_ADMIN_ACCESS"),
        control_tower_identifier: None,
        designators: &["CONFIG.IAM.DT.7"],
        scope: GLOBAL,
        deployable_regions: NO_REGIONS,
        parameters: NO_PARAMETERS,
        governed_resources: &["AWS::IAM::Policy"],
        governed_providers: AWS,
        implements: "privileged-access-approval",
        frameworks: NO_FRAMEWORK_ITEMS,
    },
    ControlSeed {
        slug: "restricted-common-ports",
        name: "Disallow Internet connection via RDP",
        description: "Reduces a server's exposure to risk by removing unfettered \
                      connectivity to remote console services such as RDP.",
        behavior: DETECTIVE,
        severity: CRITICAL,
        implementation_type: CONFIG_RULE,
        implementation_identifier: Some("RESTRICTED_INCOMING_TRAFFIC"),
        control_tower_identifier: Some("AWS-GR_RESTRICTED_COMMON_PORTS"),
        designators: NO_DESIGNATORS,
        scope: REGIONAL,
        deployable_regions: GENERALLY_AVAILABLE_REGIONS,
        parameters: NO_PARAMETERS,
        governed_resources: &["AWS::EC2::SecurityGroup"],
        governed_providers: AWS,
        implements: "restrict-administrative-port-exposure",
        frameworks: NO_FRAMEWORK_ITEMS,
    },
    ControlSeed {
        slug: "vpc-default-security-group-closed",
        name: "Checks if the default security group of any Amazon Virtual Private Cloud \
               (Amazon VPC) does not allow inbound or outbound traffic",
        description: "Checks if the default security group of any Amazon Virtual Private \
                      Cloud (Amazon VPC) does not allow inbound or outbound traffic. The \
                      rule is NON_COMPLIANT if the default security group has one or more \
                      inbound or outbound traffic rules.",
        behavior: DETECTIVE,
        severity: HIGH,
        implementation_type: CONFIG_RULE,
        implementation_identifier: Some("VPC_DEFAULT_SECURITY_GROUP_CLOSED"),
        control_tower_identifier: None,
        designators: &["CONFIG.EC2.DT.49"],
        scope: REGIONAL,
        deployable_regions: GENERALLY_AVAILABLE_REGIONS,
        parameters: NO_PARAMETERS,
        governed_resources: &["AWS::EC2::SecurityGroup"],
        governed_providers: AWS,
        implements: "restrict-administrative-port-exposure",
        frameworks: &[
            // "Ensure the default security group of every VPC restricts all traffic".
            (CIS_AWS_BENCHMARK_V3_0, "5.4"),
        ],
    },
    // --- Security Hub controls: the rest of the Security Hub-implemented catalog -
    //
    // One row per Security Hub control the catalog publishes, each carrying the
    // designator the catalog publishes it under (see `aliases`).
    ControlSeed {
        slug: "securityhub-s3-1",
        name: "S3 general purpose buckets should have block public access settings enabled",
        // The implementing Security Hub control's own published text: what the control
        // checks, followed by the condition under which it fails. The check is on the
        // ACCOUNT-level block public access settings, which is also why the control
        // governs every S3 resource type those settings restrict access to.
        description: "This control checks whether the preceding Amazon S3 block public \
                      access settings are configured at the account level for an S3 \
                      general purpose bucket. The control fails if one or more of the \
                      block public access settings are set to false.",
        behavior: DETECTIVE,
        severity: MEDIUM,
        implementation_type: SECURITY_HUB_CONTROL,
        implementation_identifier: Some("S3.1"),
        control_tower_identifier: None,
        designators: &["SH.S3.1"],
        scope: REGIONAL,
        deployable_regions: GENERALLY_AVAILABLE_REGIONS,
        parameters: NO_PARAMETERS,
        // The account-level block public access settings restrict public access to every
        // S3 surface that can grant it, so the catalog publishes each of those resource
        // types as governed, in resource-type order.
        governed_resources: &[
            "AWS::S3::AccessPoint",
            "AWS::S3::Bucket",
            "AWS::S3::BucketPolicy",
            "AWS::S3::MultiRegionAccessPoint",
            "AWS::S3::MultiRegionAccessPointPolicy",
        ],
        governed_providers: AWS,
        implements: "restrict-public-access",
        frameworks: NO_FRAMEWORK_ITEMS,
    },
    ControlSeed {
        slug: "securityhub-s3-5",
        name: "S3 general purpose buckets should require requests to use SSL",
        // The implementing Security Hub control's own published text: what the control
        // checks, followed by the condition under which it fails.
        description: "This control checks whether an Amazon S3 general purpose bucket has \
                      a policy that requires requests to use SSL. The control fails if \
                      the bucket policy doesn't require requests to use SSL.",
        behavior: DETECTIVE,
        severity: MEDIUM,
        implementation_type: SECURITY_HUB_CONTROL,
        implementation_identifier: Some("S3.5"),
        control_tower_identifier: None,
        designators: &["SH.S3.5"],
        scope: REGIONAL,
        deployable_regions: GENERALLY_AVAILABLE_REGIONS,
        parameters: NO_PARAMETERS,
        governed_resources: &["AWS::S3::Bucket"],
        governed_providers: AWS,
        implements: "enforce-encryption-in-transit",
        frameworks: NO_FRAMEWORK_ITEMS,
    },
    ControlSeed {
        slug: "securityhub-ec2-2",
        name: "VPC default security groups should not allow inbound or outbound traffic",
        // The catalog's published sentence for this control, which states the condition
        // the control requires ("does not allow"), not its negation.
        description: "This AWS control checks that the default security group of a VPC \
                      does not allow inbound or outbound traffic.",
        behavior: DETECTIVE,
        severity: HIGH,
        implementation_type: SECURITY_HUB_CONTROL,
        implementation_identifier: Some("EC2.2"),
        control_tower_identifier: None,
        designators: &["SH.EC2.2"],
        scope: REGIONAL,
        deployable_regions: GENERALLY_AVAILABLE_REGIONS,
        parameters: NO_PARAMETERS,
        governed_resources: &["AWS::EC2::SecurityGroup"],
        governed_providers: AWS,
        implements: "restrict-administrative-port-exposure",
        frameworks: NO_FRAMEWORK_ITEMS,
    },
    ControlSeed {
        slug: "securityhub-rds-2",
        name: "RDS DB Instances should prohibit public access, as determined by the \
               PubliclyAccessible configuration",
        description: "This AWS control checks whether RDS instances are publicly \
                      accessible by evaluating the publiclyAccessible field in the \
                      instance configuration item.",
        behavior: DETECTIVE,
        severity: CRITICAL,
        implementation_type: SECURITY_HUB_CONTROL,
        implementation_identifier: Some("RDS.2"),
        control_tower_identifier: None,
        designators: &["SH.RDS.2"],
        scope: REGIONAL,
        deployable_regions: GENERALLY_AVAILABLE_REGIONS,
        parameters: NO_PARAMETERS,
        governed_resources: &["AWS::RDS::DBInstance"],
        governed_providers: AWS,
        implements: "restrict-public-access",
        frameworks: NO_FRAMEWORK_ITEMS,
    },
    ControlSeed {
        slug: "securityhub-redshift-serverless-3",
        name: "Redshift Serverless workgroups should prohibit public access",
        // The implementing Security Hub control's own published text: what the control
        // checks, the configuration property it evaluates, and the value of that
        // property under which it fails.
        description: "This control checks whether public access is disabled for an \
                      Amazon Redshift Serverless workgroup. It evaluates the \
                      publiclyAccessible property of a Redshift Serverless workgroup. \
                      The control fails if public access is enabled (true) for the \
                      workgroup.",
        behavior: DETECTIVE,
        severity: HIGH,
        implementation_type: SECURITY_HUB_CONTROL,
        implementation_identifier: Some("RedshiftServerless.3"),
        control_tower_identifier: None,
        // The catalog publishes this control under a designator of its own rather than
        // under its Security Hub id.
        designators: &["SH.REDSHIFT-SERVERLESS.DT.3"],
        scope: REGIONAL,
        // Redshift Serverless is offered in fewer Regions than the partition, so the
        // control deploys to the Regions a workgroup can exist in.
        deployable_regions: REDSHIFT_SERVERLESS_REGIONS,
        parameters: NO_PARAMETERS,
        governed_resources: &["AWS::RedshiftServerless::Workgroup"],
        governed_providers: AWS,
        implements: "restrict-public-access",
        frameworks: NO_FRAMEWORK_ITEMS,
    },
    ControlSeed {
        slug: "securityhub-rds-3",
        name: "RDS DB instances should have encryption at-rest enabled",
        description: "This AWS control checks whether storage encryption is enabled for \
                      your RDS DB instances.",
        behavior: DETECTIVE,
        severity: MEDIUM,
        implementation_type: SECURITY_HUB_CONTROL,
        implementation_identifier: Some("RDS.3"),
        control_tower_identifier: None,
        designators: &["SH.RDS.3"],
        scope: REGIONAL,
        deployable_regions: GENERALLY_AVAILABLE_REGIONS,
        parameters: NO_PARAMETERS,
        governed_resources: &["AWS::RDS::DBInstance"],
        governed_providers: AWS,
        implements: "encrypt-data-at-rest-in-storage-services",
        frameworks: NO_FRAMEWORK_ITEMS,
    },
    ControlSeed {
        slug: "securityhub-efs-1",
        name: "Elastic File System should be configured to encrypt file data at-rest \
               using AWS KMS",
        description: "This AWS control checks whether Amazon Elastic File System (Amazon \
                      EFS) is configured to encrypt the file data using AWS Key \
                      Management Service (AWS KMS). The check will fail if the encrypted \
                      key is set to false on DescribeFileSystems or if the KmsKeyId key \
                      on DescribeFileSystems does not match the KmsKeyId parameter.",
        behavior: DETECTIVE,
        severity: MEDIUM,
        implementation_type: SECURITY_HUB_CONTROL,
        implementation_identifier: Some("EFS.1"),
        control_tower_identifier: None,
        designators: &["SH.EFS.1"],
        scope: REGIONAL,
        deployable_regions: GENERALLY_AVAILABLE_REGIONS,
        parameters: NO_PARAMETERS,
        governed_resources: &["AWS::EFS::FileSystem"],
        governed_providers: AWS,
        implements: "encrypt-data-at-rest-in-storage-services",
        frameworks: NO_FRAMEWORK_ITEMS,
    },
    ControlSeed {
        slug: "securityhub-cloudtrail-4",
        name: "CloudTrail log file validation should be enabled",
        description: "This AWS control checks whether CloudTrail log file validation is \
                      enabled.",
        behavior: DETECTIVE,
        severity: LOW,
        implementation_type: SECURITY_HUB_CONTROL,
        implementation_identifier: Some("CloudTrail.4"),
        control_tower_identifier: None,
        designators: &["SH.CloudTrail.4"],
        scope: REGIONAL,
        deployable_regions: GENERALLY_AVAILABLE_REGIONS,
        parameters: NO_PARAMETERS,
        governed_resources: &["AWS::CloudTrail::Trail"],
        governed_providers: AWS,
        implements: "log-integrity-protection",
        frameworks: &[
            // "Ensure CloudTrail log file validation is enabled".
            (CIS_AWS_BENCHMARK_V3_0, "3.2"),
        ],
    },
    ControlSeed {
        slug: "securityhub-iam-1",
        name: "IAM policies should not allow full \"*\" administrative privileges",
        description: "This AWS control checks whether the default version of AWS \
                      Identity and Access Management (IAM) policies (also known as \
                      customer managed policies) do not have administrator access with a \
                      statement that has \"Effect\": \"Allow\" with \"Action\": \"*\" \
                      over \"Resource\": \"*\". It only checks for the Customer Managed \
                      Policies that you created, but not inline and AWS Managed \
                      Policies.",
        behavior: DETECTIVE,
        severity: HIGH,
        implementation_type: SECURITY_HUB_CONTROL,
        implementation_identifier: Some("IAM.1"),
        control_tower_identifier: None,
        designators: &["SH.IAM.1"],
        scope: GLOBAL,
        deployable_regions: NO_REGIONS,
        parameters: NO_PARAMETERS,
        governed_resources: &["AWS::IAM::Policy"],
        governed_providers: AWS,
        implements: "privileged-access-approval",
        frameworks: NO_FRAMEWORK_ITEMS,
    },
    ControlSeed {
        slug: "securityhub-iam-3",
        name: "IAM users' access keys should be rotated every 90 days or less",
        description: "This AWS control checks whether the active access keys are rotated \
                      within 90 days.",
        behavior: DETECTIVE,
        severity: MEDIUM,
        implementation_type: SECURITY_HUB_CONTROL,
        implementation_identifier: Some("IAM.3"),
        control_tower_identifier: None,
        designators: &["SH.IAM.3"],
        scope: GLOBAL,
        deployable_regions: NO_REGIONS,
        parameters: NO_PARAMETERS,
        // The catalog publishes the identity whose keys are checked as the resource type
        // this control governs, not the key resource itself.
        governed_resources: &["AWS::IAM::User"],
        governed_providers: AWS,
        implements: "credential-rotation",
        frameworks: &[
            // "Ensure access keys are rotated every 90 days or less".
            (CIS_AWS_BENCHMARK_V3_0, "1.14"),
        ],
    },
    ControlSeed {
        slug: "securityhub-kms-4",
        name: "AWS KMS key rotation should be enabled",
        description: "This AWS control checks that key rotation is enabled for each AWS \
                      KMS key. It does not check KMS keys that have imported key \
                      material.",
        behavior: DETECTIVE,
        severity: MEDIUM,
        implementation_type: SECURITY_HUB_CONTROL,
        implementation_identifier: Some("KMS.4"),
        control_tower_identifier: None,
        designators: &["SH.KMS.4"],
        scope: REGIONAL,
        deployable_regions: GENERALLY_AVAILABLE_REGIONS,
        parameters: NO_PARAMETERS,
        governed_resources: &["AWS::KMS::Key"],
        governed_providers: AWS,
        implements: "credential-rotation",
        frameworks: NO_FRAMEWORK_ITEMS,
    },
    ControlSeed {
        slug: "securityhub-guardduty-1",
        name: "GuardDuty should be enabled",
        description: "This AWS control checks whether Amazon GuardDuty is enabled in \
                      your AWS account and region.",
        behavior: DETECTIVE,
        severity: HIGH,
        implementation_type: SECURITY_HUB_CONTROL,
        implementation_identifier: Some("GuardDuty.1"),
        control_tower_identifier: None,
        designators: &["SH.GuardDuty.1"],
        scope: REGIONAL,
        deployable_regions: GENERALLY_AVAILABLE_REGIONS,
        parameters: NO_PARAMETERS,
        governed_resources: &["AWS::GuardDuty::Detector"],
        governed_providers: AWS,
        implements: "intrusion-detection-deployment",
        frameworks: &[
            // "Ensure GuardDuty is enabled".
            (CIS_AWS_BENCHMARK_V3_0, "3.8"),
        ],
    },
    ControlSeed {
        slug: "securityhub-config-1",
        name: "AWS Config should be enabled and use the service-linked role for resource \
               recording",
        description: "This control checks whether AWS Config is enabled in your account \
                      in the current AWS Region, records all resources that correspond \
                      to controls that are enabled in the current Region, and uses the \
                      service-linked AWS Config role.",
        behavior: DETECTIVE,
        severity: CRITICAL,
        implementation_type: SECURITY_HUB_CONTROL,
        implementation_identifier: Some("Config.1"),
        control_tower_identifier: None,
        // The catalog publishes this control under a designator of its own rather than
        // under its Security Hub id.
        designators: &["SH.CONFIG.DT.1"],
        scope: REGIONAL,
        deployable_regions: GENERALLY_AVAILABLE_REGIONS,
        // Security Hub parameterises this control, so the catalog publishes it as
        // accepting those parameters, none of which the caller has to supply.
        parameters: &[(RECORD_ALL_RESOURCES, OPTIONAL), (RESOURCE_TYPES, OPTIONAL)],
        // The account's own configuration recorder, which no available resource type
        // represents.
        governed_resources: NO_GOVERNED_RESOURCES,
        governed_providers: AWS,
        implements: "maintain-an-asset-inventory",
        frameworks: NO_FRAMEWORK_ITEMS,
    },
];

/// What the Control Catalog itself assigns to one published control: the facts that are
/// neither derivable from the request nor from the row describing the control.
#[derive(Debug)]
pub(super) struct PublishedCatalogRecord {
    /// The `slug` of the [`PUBLISHED_CONTROLS`] row this record belongs to.
    pub(super) slug: &'static str,
    /// The opaque identifier the control's published ARN carries.
    pub(super) identifier: &'static str,
    /// The instant the catalog publishes as the control's release time, in seconds since
    /// the Unix epoch, where this crate records it: `CreateTime` "notes the time when the
    /// control was released (start of its life) as a governance capability in Amazon Web
    /// Services" (`smithy_query(query="shape", shape="ControlSummary")`). `None` for a
    /// control whose published identifier is recorded but whose release instant is not —
    /// the two are separate published facts, so recording the identifier does not wait on
    /// the instant.
    pub(super) release_time: Option<i64>,
}

/// The published catalog record of every control this catalog records one for.
///
/// Source: the Control Catalog's own published control list, which names every control
/// by the ARN `arn:{PARTITION}:controlcatalog:::control/{identifier}` and publishes the
/// instant it was released. Neither value is derivable from the request, yet both are
/// part of the control's published content rather than a per-run value: the catalog is
/// account-independent and read-only, so one identifier names the control and one
/// release instant dates it for every caller and across runs — a client holds the
/// identifier to read the control back with `GetControl` and to enable it elsewhere, and
/// reads the release instant to decide which controls are new since its last sync. Two
/// controls the catalog released together therefore carry ONE release instant, not two
/// near ones. A row this table does not name is served under an identifier minted from
/// its slug (see `super::catalog_id`) and a release instant derived the same
/// deterministic way (see `super::seed_catalog`), which is all a control whose published
/// record is not recorded here can be served under.
pub(super) const PUBLISHED_CATALOG_RECORDS: &[PublishedCatalogRecord] = &[
    PublishedCatalogRecord {
        slug: "multi-region-cloud-trail-enabled",
        identifier: "d5kdbugbadk3r481vtwnvoynv",
        release_time: Some(1_539_993_600),
    },
    PublishedCatalogRecord {
        slug: "s3-bucket-server-side-encryption-enabled",
        identifier: "d4wgeffz6izrb627c2yb8nq8d",
        release_time: Some(1_515_110_400),
    },
    // The account-wide and the log-archive control over public S3 reads were released
    // together, so the catalog dates both with one instant.
    PublishedCatalogRecord {
        slug: "s3-bucket-public-read-prohibited",
        identifier: "8sw3pbid15t9cbww8d2w2qwgf",
        release_time: Some(1_542_240_000),
    },
    PublishedCatalogRecord {
        slug: "audit-bucket-public-read-prohibited",
        identifier: "9jzlkwys54gzgsx3pmkqnx8l1",
        release_time: Some(1_542_240_000),
    },
    PublishedCatalogRecord {
        slug: "s3-bucket-public-write-prohibited",
        identifier: "9j9nwxj789d82sypnukhyyowy",
        release_time: Some(1_542_240_000),
    },
    PublishedCatalogRecord {
        slug: "audit-bucket-public-write-prohibited",
        identifier: "ci9ov6vzjoi6vzkajgademvvo",
        release_time: Some(1_542_240_000),
    },
    PublishedCatalogRecord {
        slug: "s3-bucket-ssl-requests-only",
        identifier: "e5efjudq0b347dgxn86o8fg2o",
        release_time: Some(1_479_772_800),
    },
    PublishedCatalogRecord {
        slug: "vpc-flow-logs-enabled",
        identifier: "22484xhzccf9twis5gmdt7uic",
        release_time: Some(1_536_710_400),
    },
    // Both RDS controls were released together, so they too share one instant.
    PublishedCatalogRecord {
        slug: "rds-snapshots-public-prohibited",
        identifier: "1h4eyqyyonp19dlrreqf1i3w0",
        release_time: Some(1_564_444_800),
    },
    PublishedCatalogRecord {
        slug: "rds-storage-encrypted",
        identifier: "e34kieahgkm0lggs5g0s412jt",
        release_time: Some(1_564_444_800),
    },
    // The Security Hub-implemented controls the catalog publishes under their `SH.`
    // designators. The catalog released most of them as one body of governance
    // capability, so they carry one release instant; the two that were released on their
    // own carry the instant recorded for each (see `securityhub-kms-4` and
    // `securityhub-config-1` at the end of this group).
    PublishedCatalogRecord {
        slug: "securityhub-cloudtrail-1",
        identifier: "435u8u4w5bsf02tkxga5jisbc",
        release_time: Some(1_669_593_600),
    },
    PublishedCatalogRecord {
        slug: "securityhub-iam-4",
        identifier: "ezfflnh8xgshxb78grtt5sjc5",
        release_time: Some(1_669_593_600),
    },
    PublishedCatalogRecord {
        slug: "securityhub-iam-5",
        identifier: "6g3gbim493yofkc9eg6vvnahm",
        release_time: Some(1_669_593_600),
    },
    PublishedCatalogRecord {
        slug: "securityhub-cloudtrail-4",
        identifier: "624yg0j9d8swiglwfc1m4kvnm",
        release_time: Some(1_669_593_600),
    },
    PublishedCatalogRecord {
        slug: "securityhub-ec2-2",
        identifier: "dowsaumj3n874siszx3j94m9n",
        release_time: Some(1_669_593_600),
    },
    PublishedCatalogRecord {
        slug: "securityhub-efs-1",
        identifier: "5cksj6jdycadjupdbviikn1w7",
        release_time: Some(1_669_593_600),
    },
    PublishedCatalogRecord {
        slug: "securityhub-guardduty-1",
        identifier: "62smpoz33dsy0oa7u1iwa58lz",
        release_time: Some(1_669_593_600),
    },
    PublishedCatalogRecord {
        slug: "securityhub-iam-1",
        identifier: "a7ertnvgba7449o25l6s75nrj",
        release_time: Some(1_669_593_600),
    },
    PublishedCatalogRecord {
        slug: "securityhub-iam-3",
        identifier: "bxh5ltpzbcr59zexx3llhg3l9",
        release_time: Some(1_669_593_600),
    },
    PublishedCatalogRecord {
        slug: "securityhub-rds-2",
        identifier: "bbns1svmi60sz11i658ot4amf",
        release_time: Some(1_669_593_600),
    },
    PublishedCatalogRecord {
        slug: "securityhub-rds-3",
        identifier: "dsrtfqf4g9gvlguj5hqahozla",
        release_time: Some(1_669_593_600),
    },
    // Released well after the body above — the catalog publishes a control once the
    // surface it governs exists — so it carries its own instant.
    PublishedCatalogRecord {
        slug: "securityhub-redshift-serverless-3",
        identifier: "1ks6hwrtiiia8t2djn1iys62x",
        release_time: Some(1_765_929_600),
    },
    // The two controls over Amazon S3 belong to that same body of governance
    // capability, so the catalog dates them with the one instant recorded above rather
    // than with an instant of their own.
    PublishedCatalogRecord {
        slug: "securityhub-s3-1",
        identifier: "alm61aw5oxpgfgh3bhmish87u",
        release_time: Some(1_669_593_600),
    },
    PublishedCatalogRecord {
        slug: "securityhub-s3-5",
        identifier: "35hoe56okxxh2ujnsub92wkym",
        release_time: Some(1_669_593_600),
    },
    // Released after the body above, so the catalog dates it with its own instant.
    PublishedCatalogRecord {
        slug: "securityhub-kms-4",
        identifier: "esv514s4zvnxdunuijbgerpn3",
        release_time: Some(1_701_043_200),
    },
    // The most recently released of this group, and the one the catalog publishes under
    // its own `SH.CONFIG.DT.1` designator rather than under its Security Hub id.
    PublishedCatalogRecord {
        slug: "securityhub-config-1",
        identifier: "9r3or4f2x3aq1jrnma6ddfp2y",
        release_time: Some(1_779_840_000),
    },
    // Two controls of the wider AWS Config-implemented surface: the catalog publishes
    // each of them under the identifier recorded here. Their release instants are a
    // decade apart because the catalog released them a decade apart, not because anything
    // in this crate dates them.
    PublishedCatalogRecord {
        slug: "acm-pca-certificate-authority-tagged",
        identifier: "6yl9xxuky13vzaxi3n6lm73k5",
        release_time: Some(1_779_840_000),
    },
    PublishedCatalogRecord {
        slug: "cognito-user-pool-advanced-security-enabled",
        identifier: "1038h7d21e6g8i10g6v3xsssi",
        release_time: Some(1_715_212_800),
    },
    // The rest of the AWS Config-implemented surface whose published identifier this
    // catalog records: the controls the catalog publishes under a `CONFIG.…` designator
    // and the Control Tower controls an AWS Config managed rule implements. A release
    // instant is recorded only where this crate holds the one the catalog publishes; the
    // identifier and the instant are separate published facts, so a row records the
    // identifier whether or not the instant is known.
    PublishedCatalogRecord {
        slug: "ebs-snapshot-public-restorable-check",
        identifier: "87qo8rsoettjrxjevmjqcw1tu",
        release_time: Some(1_638_316_800),
    },
    PublishedCatalogRecord {
        slug: "cloud-trail-cloud-watch-logs-enabled",
        identifier: "9vnj728czsv4m6pflfqt21bw2",
        release_time: Some(1_540_166_400),
    },
    PublishedCatalogRecord {
        slug: "elbv2-acm-certificate-required",
        identifier: "aazzef7lptl5umta6690p6u5c",
        release_time: Some(1_594_771_200),
    },
    PublishedCatalogRecord {
        slug: "iam-policy-no-statements-with-admin-access",
        identifier: "byrg7iwyl4azhecyhqtq8ocw1",
        release_time: Some(1_537_228_800),
    },
    PublishedCatalogRecord {
        slug: "rds-snapshot-encrypted",
        identifier: "84hd48gyrb6e5ribkmsoub44c",
        release_time: Some(1_590_364_800),
    },
    PublishedCatalogRecord {
        slug: "redshift-cluster-public-access-check",
        identifier: "1oxkwnc4hwhi2ndv6ekwy7np7",
        // Released together with `sagemaker-notebook-no-direct-internet-access` below, so
        // the catalog dates both controls with this one instant.
        release_time: Some(1_638_230_400),
    },
    PublishedCatalogRecord {
        slug: "restricted-common-ports",
        identifier: "df2ta5ytg2zatj1q7y5e09u32",
        release_time: Some(1_542_240_000),
    },
    PublishedCatalogRecord {
        slug: "root-account-hardware-mfa-enabled",
        identifier: "227ph3vim64tdwv5mfagjd22r",
        release_time: Some(1_535_587_200),
    },
    PublishedCatalogRecord {
        slug: "sagemaker-notebook-no-direct-internet-access",
        identifier: "66gfl06uj1v999z53szvu0exa",
        // The instant it shares with `redshift-cluster-public-access-check` above.
        release_time: Some(1_638_230_400),
    },
    PublishedCatalogRecord {
        slug: "secretsmanager-rotation-enabled-check",
        identifier: "4tf01viblttjxqhu35yz90vw1",
        // The catalog dates this control with the same instant as the other most recently
        // released controls it records one for (see `acm-pca-certificate-authority-tagged`
        // and `securityhub-config-1`): one instant for one release, later than the
        // instants above because the catalog released these controls later.
        release_time: Some(1_779_840_000),
    },
    // The AWS Config-implemented controls the catalog publishes under a `CONFIG.…`
    // designator and the Control Tower controls an AWS Config managed rule implements,
    // including the TWO controls published over one rule (`CLOUD_TRAIL_ENABLED`), each
    // under its own identifier because each governs a different scope.
    PublishedCatalogRecord {
        slug: "account-part-of-organizations",
        identifier: "4x11tie7h25sws4g7ilakhcb7",
        release_time: Some(1_594_166_400),
    },
    PublishedCatalogRecord {
        slug: "cloudwatch-log-group-encrypted",
        identifier: "497wrm2xnk1wxlf4obrdo7mej",
        release_time: Some(1_563_408_000),
    },
    // One AWS Config managed rule implements both CloudTrail-enablement controls and the
    // catalog released the two together, so they carry ONE release instant.
    PublishedCatalogRecord {
        slug: "detect-cloud-trail-enabled-on-member-accounts",
        identifier: "8sdhbqfe2fsmh3b1i2b24xaic",
        release_time: Some(1_657_584_000),
    },
    PublishedCatalogRecord {
        slug: "detect-cloud-trail-enabled-on-shared-accounts",
        identifier: "1ln29qglgkigm9ajfka6u2p2u",
        release_time: Some(1_657_584_000),
    },
    PublishedCatalogRecord {
        slug: "cloud-trail-log-file-validation-enabled",
        identifier: "85z4kpyxbo5nepzfwbfwwxjx7",
        release_time: Some(1_534_982_400),
    },
    // Released together with the controls over public S3 access above and the two over
    // remote console ports, so the catalog dates them all with that one instant.
    PublishedCatalogRecord {
        slug: "encrypted-volumes",
        identifier: "503uicglhjkokaajywfpt6ros",
        release_time: Some(1_542_240_000),
    },
    PublishedCatalogRecord {
        slug: "guardduty-enabled-centralized",
        identifier: "9xv17a25orapqa36e19o5b8lp",
        release_time: Some(1_534_809_600),
    },
    PublishedCatalogRecord {
        slug: "iam-password-policy",
        identifier: "dmb52g7tw004w9iede1pgb1fj",
        release_time: Some(1_474_416_000),
    },
    PublishedCatalogRecord {
        slug: "iam-root-access-key-check",
        identifier: "9utgo6tl86bg3e9jaq073vmqw",
        release_time: Some(1_534_377_600),
    },
    // The instant it shares with the two RDS controls above.
    PublishedCatalogRecord {
        slug: "iam-user-mfa-enabled",
        identifier: "7z1uzm6s8qk7ym7m0sm56cq0u",
        release_time: Some(1_564_444_800),
    },
    // The control over SSH and the one over RDP (`restricted-common-ports` above) were
    // released together, so they carry one instant.
    PublishedCatalogRecord {
        slug: "incoming-ssh-disabled",
        identifier: "6rilu41n0gb9w6mxrkyewoer4",
        release_time: Some(1_542_240_000),
    },
    // The control over certificate expiry belongs to the same release as the other most
    // recently released controls recorded above (see
    // `acm-pca-certificate-authority-tagged` and `securityhub-config-1`), so the catalog
    // dates it with that one instant.
    PublishedCatalogRecord {
        slug: "acm-certificate-expiration-check",
        identifier: "a6kwsgw65ayxiqg7dhzbz2n7w",
        release_time: Some(1_779_840_000),
    },
    PublishedCatalogRecord {
        slug: "elb-logging-enabled",
        identifier: "8abhdcvu10syqrdyfgr2300px",
        release_time: Some(1_534_896_000),
    },
    // A published identifier is opaque content the catalog assigns, so it is recorded as
    // the catalog publishes it: this one is 24 characters where most are 25.
    PublishedCatalogRecord {
        slug: "elb-tls-https-listeners-only",
        identifier: "7laz29268m0wkplaj7z2fxxl",
        release_time: Some(1_594_166_400),
    },
    // The control over Amazon Redshift connections and the one over Amazon SNS encryption
    // were released together, so the catalog dates both with one instant — the instant it
    // also dates `rds-snapshot-encrypted` with.
    PublishedCatalogRecord {
        slug: "redshift-require-tls-ssl",
        identifier: "dfbp3c8jerraflh2g68427p4k",
        release_time: Some(1_590_364_800),
    },
    PublishedCatalogRecord {
        slug: "sns-encrypted-kms",
        identifier: "bn8ygh3zsy33drdeojmu09a4z",
        release_time: Some(1_590_364_800),
    },
    // The earliest release instant this catalog records: S3 server access logging predates
    // every other control recorded here.
    PublishedCatalogRecord {
        slug: "s3-bucket-logging-enabled",
        identifier: "bd9d97ndoue5lss1zjjiecle4",
        release_time: Some(1_475_280_000),
    },
    PublishedCatalogRecord {
        slug: "vpc-default-security-group-closed",
        identifier: "3ucly6t3j33t5zhbzy45l619n",
        release_time: Some(1_534_291_200),
    },
    // The AWS Config-implemented controls the catalog publishes under a `CONFIG.…`
    // designator whose published identifier and release instant this catalog records
    // alongside the group above.
    PublishedCatalogRecord {
        slug: "api-gw-execution-logging-enabled",
        identifier: "aayte0p9vah5rp97z6yfbytl2",
        // The catalog's own release instant for this control, which coincides with the
        // instant recorded above for the controls released in the same release
        // (`acm-pca-certificate-authority-tagged`, `securityhub-config-1`) because the
        // catalog released them together, not because it is inferred from them.
        release_time: Some(1_779_840_000),
    },
    PublishedCatalogRecord {
        slug: "cmk-backing-key-rotation-enabled",
        identifier: "7305ygy72ra2amwqjl1bqrmrw",
        release_time: Some(1_540_339_200),
    },
    PublishedCatalogRecord {
        slug: "dynamodb-table-encrypted-kms",
        identifier: "a0ufrfbomg90qbskltuop262u",
        // The catalog's own release instant for this control; it is the instant recorded
        // above for `rds-snapshot-encrypted` and `redshift-require-tls-ssl` because the
        // catalog released those controls over encryption at rest together.
        release_time: Some(1_590_364_800),
    },
    PublishedCatalogRecord {
        slug: "efs-encrypted-check",
        identifier: "8qmzz7l2841u6bvwpkiug38ta",
        // The catalog's own release instant for this control, the same instant it dates
        // `cloudwatch-log-group-encrypted` above with.
        release_time: Some(1_563_408_000),
    },
    PublishedCatalogRecord {
        slug: "opensearch-encrypted-at-rest",
        identifier: "70u9gks728rcr33237rckvzbo",
        release_time: Some(1_637_193_600),
    },
    // The AWS Config-implemented controls the catalog publishes under a `CONFIG.…`
    // designator whose published identifier this catalog records while the instant the
    // catalog released them is not recorded here: the identifier and the instant are
    // separate published facts, so a record holds the one this crate holds and the other is
    // derived from the row's slug (see `super::minted_release_time`).
    PublishedCatalogRecord {
        slug: "ec2-instance-detailed-monitoring-enabled",
        identifier: "27lgd3oknluio5kzcl324kr0d",
        release_time: None,
    },
    // The Control Tower controls an AWS Config managed rule implements whose published
    // identifier this catalog records while the instant the catalog released them is not
    // recorded here: the identifier and the instant are separate published facts, so a
    // record holds the one this crate holds and the other is derived from the row's slug
    // (see `super::minted_release_time`).
    PublishedCatalogRecord {
        slug: "ec2-instance-no-public-ip",
        identifier: "4v7xtm83uvvyulk1wwpm4qm3s",
        release_time: None,
    },
    PublishedCatalogRecord {
        slug: "eks-endpoint-no-public-access",
        identifier: "aeellyghb27pbehyzua1nyena",
        release_time: None,
    },
    PublishedCatalogRecord {
        slug: "lambda-function-public-access-prohibited",
        identifier: "b2gzofz99eb7nsuj5g8wcimse",
        release_time: None,
    },
    PublishedCatalogRecord {
        slug: "mfa-enabled-for-iam-console-access",
        identifier: "1fvktjhpo9wdpt3pjb2wsz7nt",
        release_time: None,
    },
    PublishedCatalogRecord {
        slug: "rds-instance-public-access-check",
        identifier: "4jc77cq1lcr7g64xywwypykv8",
        release_time: None,
    },
];

/// The published catalog record of the control `slug` names, or `None` when this catalog
/// records none for it.
fn published_catalog_record(slug: &str) -> Option<&'static PublishedCatalogRecord> {
    PUBLISHED_CATALOG_RECORDS
        .iter()
        .find(|record| record.slug == slug)
}

/// The opaque catalog identifier published for the control `slug` names, or `None` when
/// this catalog records no published record for it.
pub(super) fn published_catalog_identifier(slug: &str) -> Option<&'static str> {
    published_catalog_record(slug).map(|record| record.identifier)
}

/// The instant the catalog publishes as the release time of the control `slug` names, or
/// `None` when this catalog records no published release instant for it.
pub(super) fn published_release_time(slug: &str) -> Option<i64> {
    published_catalog_record(slug).and_then(|record| record.release_time)
}

/// The input parameters an AWS Config managed rule takes: `(rule identifier, [(parameter
/// name, ControlParameterRequirement)])`.
///
/// A published control's `Parameters` are the parameters of the surface that implements
/// it — the catalog publishes, for a control an AWS Config managed rule implements, that
/// rule's own input parameters under the rule's own names, which is why such a control
/// summarises its parameter requirement as `OPTIONAL` while a control over a rule that
/// takes no input summarises it as `NONE`. Source: the implementing rule's documented
/// input parameters; a parameter the rule gives a default is `OPTIONAL` and one it has no
/// default for is `REQUIRED`. A rule this table does not name takes no input parameter,
/// so a control it implements supports none.
const CONFIG_RULE_PARAMETERS: &[(&str, &[(&str, &str)])] = &[
    ("ACCOUNT_PART_OF_ORGANIZATIONS", &[("MasterAccountId", OPTIONAL)]),
    (
        "CLOUD_TRAIL_ENABLED",
        &[
            ("s3BucketName", OPTIONAL),
            ("s3KeyPrefix", OPTIONAL),
            ("snsTopicArn", OPTIONAL),
        ],
    ),
    (
        "MULTI_REGION_CLOUD_TRAIL_ENABLED",
        &[
            ("s3BucketName", OPTIONAL),
            ("s3KeyPrefix", OPTIONAL),
            ("snsTopicArn", OPTIONAL),
            ("cloudWatchLogsLogGroupArn", OPTIONAL),
            ("includeManagementEvents", OPTIONAL),
            ("readWriteType", OPTIONAL),
        ],
    ),
    ("ACCESS_KEYS_ROTATED", &[("maxAccessKeyAge", OPTIONAL)]),
    (
        "IAM_PASSWORD_POLICY",
        &[
            ("RequireUppercaseCharacters", OPTIONAL),
            ("RequireLowercaseCharacters", OPTIONAL),
            ("RequireSymbols", OPTIONAL),
            ("RequireNumbers", OPTIONAL),
            ("MinimumPasswordLength", OPTIONAL),
            ("PasswordReusePrevention", OPTIONAL),
            ("MaxPasswordAge", OPTIONAL),
        ],
    ),
    ("ENCRYPTED_VOLUMES", &[("kmsId", OPTIONAL)]),
    ("VPC_FLOW_LOGS_ENABLED", &[("trafficType", OPTIONAL)]),
    ("RDS_STORAGE_ENCRYPTED", &[("kmsKeyId", OPTIONAL)]),
    (
        "GUARDDUTY_ENABLED_CENTRALIZED",
        &[("centralMonitoringAccount", OPTIONAL)],
    ),
    ("CLOUDWATCH_LOG_GROUP_ENCRYPTED", &[("KmsKeyId", OPTIONAL)]),
    // The number of days before expiry at which a certificate is reported, which the rule
    // defaults, so the caller may supply it.
    (
        "ACM_CERTIFICATE_EXPIRATION_CHECK",
        &[("daysToExpiration", OPTIONAL)],
    ),
    ("ACMPCA_CERTIFICATE_AUTHORITY_TAGGED", &[("tags", OPTIONAL)]),
    (
        "COGNITO_USER_POOL_ADVANCED_SECURITY_ENABLED",
        &[("SecurityMode", OPTIONAL)],
    ),
    ("EFS_ENCRYPTED_CHECK", &[("KmsKeyId", OPTIONAL)]),
    ("DYNAMODB_TABLE_ENCRYPTED_KMS", &[("kmsKeyArns", OPTIONAL)]),
    ("SNS_ENCRYPTED_KMS", &[("kmsKeyIds", OPTIONAL)]),
    (
        // The two settings this rule checks have no default, so the caller must supply
        // them; the node types it narrows to do have one.
        "REDSHIFT_CLUSTER_CONFIGURATION_CHECK",
        &[
            ("clusterDbEncrypted", REQUIRED),
            ("loggingEnabled", REQUIRED),
            ("nodeTypes", OPTIONAL),
        ],
    ),
    (
        "S3_BUCKET_LOGGING_ENABLED",
        &[("targetBucket", OPTIONAL), ("targetPrefix", OPTIONAL)],
    ),
    (
        "CLOUD_TRAIL_CLOUD_WATCH_LOGS_ENABLED",
        &[("expectedDeliveryWindowAge", OPTIONAL)],
    ),
    ("ELB_LOGGING_ENABLED", &[("s3BucketNames", OPTIONAL)]),
    (
        "API_GW_EXECUTION_LOGGING_ENABLED",
        &[("loggingLevel", OPTIONAL)],
    ),
    (
        "IAM_USER_UNUSED_CREDENTIALS_CHECK",
        &[("maxCredentialUsageAge", OPTIONAL)],
    ),
    (
        "RESTRICTED_INCOMING_TRAFFIC",
        &[
            ("blockedPort1", OPTIONAL),
            ("blockedPort2", OPTIONAL),
            ("blockedPort3", OPTIONAL),
            ("blockedPort4", OPTIONAL),
            ("blockedPort5", OPTIONAL),
        ],
    ),
    // The certificate authorities a listener's certificate may come from.
    (
        "ELBV2_ACM_CERTIFICATE_REQUIRED",
        &[("AcmCertificateArns", OPTIONAL)],
    ),
    // Whether a policy used only as a permissions boundary is exempt from the check.
    (
        "IAM_POLICY_NO_STATEMENTS_WITH_ADMIN_ACCESS",
        &[("excludePermissionBoundaryPolicy", OPTIONAL)],
    ),
    // The longest rotation interval a secret may be scheduled on, in days and in hours.
    (
        "SECRETSMANAGER_ROTATION_ENABLED_CHECK",
        &[
            ("maximumAllowedRotationFrequency", OPTIONAL),
            ("maximumAllowedRotationFrequencyInHours", OPTIONAL),
        ],
    ),
];

/// The control parameters a published control supports.
///
/// A parameter is a value the caller supplies when it ENABLES the control, and which
/// values those are is a property of the surface that implements the control, not of the
/// catalog row: a control an AWS Config managed rule implements publishes that rule's
/// input parameters ([`CONFIG_RULE_PARAMETERS`]), so two controls sharing one rule
/// publish one parameter set and cannot drift apart, while a control deployed as a
/// policy or a hook publishes the exemption parameters its row carries.
pub(super) fn parameters(seed: &ControlSeed) -> &'static [(&'static str, &'static str)] {
    if seed.implementation_type == CONFIG_RULE {
        // A Config-rule row leaves the field empty, because the implementing rule's
        // parameters are what such a control publishes; a row that declared both would
        // otherwise lose the ones written on it.
        debug_assert!(
            seed.parameters.is_empty(),
            "{}: a control an AWS Config managed rule implements takes its parameters \
             from CONFIG_RULE_PARAMETERS, so its row declares none",
            seed.slug
        );
        if let Some(identifier) = seed.implementation_identifier {
            return CONFIG_RULE_PARAMETERS
                .iter()
                .find(|(rule, _)| *rule == identifier)
                .map_or(NO_PARAMETERS, |(_, parameters)| *parameters);
        }
    }
    seed.parameters
}

/// How two published controls relate: `(control slug, related control slug,
/// ControlRelationType)`.
///
/// ALTERNATIVE where either control satisfies the common control the pair implements —
/// the typical case for the same check published on two surfaces — and COMPLEMENTARY
/// where both can be enabled and reinforce one another, as a preventive control does
/// with the detective control that reports what slipped past it. So a pair the catalog
/// publishes on ONE implementation (both rows carrying the same `implementation_type`
/// and `implementation_identifier`) is always ALTERNATIVE, never COMPLEMENTARY: there
/// is a single check behind the two controls, so enabling the second one adds no
/// coverage the first does not already give. This catalog publishes
/// no pair that conflicts, so it records no MUTUALLY_EXCLUSIVE relation. The seeder
/// records each row in BOTH directions, because a query filters on one control at a
/// time and each side has to report the relation when it is the one queried.
pub(super) const RELATED_CONTROLS: &[(&str, &str, &str)] = &[
    (
        "s3-bucket-server-side-encryption-enabled",
        "securityhub-s3-4",
        "ALTERNATIVE",
    ),
    (
        "s3-bucket-server-side-encryption-enabled",
        "require-encrypted-ebs-volumes",
        "COMPLEMENTARY",
    ),
    (
        "multi-region-cloud-trail-enabled",
        "securityhub-cloudtrail-1",
        "ALTERNATIVE",
    ),
    // One Config rule (`CLOUD_TRAIL_ENABLED`) backs both of these published controls,
    // and both implement `enable-centralized-logging`: the same check on two surfaces,
    // so either one satisfies the objective on its own.
    (
        "detect-cloud-trail-enabled-on-shared-accounts",
        "detect-cloud-trail-enabled-on-member-accounts",
        "ALTERNATIVE",
    ),
    (
        "multi-region-cloud-trail-enabled",
        "disallow-cloudtrail-changes",
        "COMPLEMENTARY",
    ),
    (
        "iam-root-access-key-check",
        "securityhub-iam-4",
        "ALTERNATIVE",
    ),
    (
        "iam-root-access-key-check",
        "restrict-root-user-access-keys",
        "COMPLEMENTARY",
    ),
    ("iam-user-mfa-enabled", "securityhub-iam-5", "ALTERNATIVE"),
    (
        "encrypted-volumes",
        "require-encrypted-ebs-volumes",
        "COMPLEMENTARY",
    ),
    (
        "rds-storage-encrypted",
        "require-rds-storage-encryption-at-creation",
        "COMPLEMENTARY",
    ),
    (
        "rds-snapshots-public-prohibited",
        "deny-public-rds-snapshots",
        "COMPLEMENTARY",
    ),
    (
        "deny-public-rds-snapshots",
        "gcp-storage-bucket-uniform-access",
        "COMPLEMENTARY",
    ),
    (
        "s3-bucket-public-read-prohibited",
        "s3-bucket-public-write-prohibited",
        "COMPLEMENTARY",
    ),
    // Each of these pairs is one Config rule behind two published controls — the
    // account-wide control and the log-archive one — so either satisfies the common
    // control they both implement.
    (
        "s3-bucket-public-read-prohibited",
        "audit-bucket-public-read-prohibited",
        "ALTERNATIVE",
    ),
    (
        "s3-bucket-public-write-prohibited",
        "audit-bucket-public-write-prohibited",
        "ALTERNATIVE",
    ),
];

/// The framework requirements each control OBJECTIVE satisfies, keyed by the slug of
/// the common control that states the objective (a control's `implements`).
///
/// A compliance framework states its requirements at the level of the objective —
/// "protect information at rest", "log system events" — not at the level of one
/// service's setting, and the frameworks a catalog cross-walks into restate the same
/// objective each in its own words. So every control implementing an objective
/// satisfies that objective's requirement in EVERY framework that states it, whichever
/// surface the control is implemented on, which is why a control's mapping set spans
/// the frameworks the catalog cross-walks rather than one or two of them. Recording the
/// cross-walk once per objective is also what keeps two controls that implement one
/// objective from reporting different framework coverage.
///
/// Each row's comment is the requirement's own title in that framework.
pub(super) const OBJECTIVE_FRAMEWORKS: &[(&str, &[FrameworkItem])] = &[
    (
        "maintain-an-asset-inventory",
        &[
            // "System component inventory".
            (NIST_SP_800_53_R5, "CM-8"),
            // "Physical devices and systems within the organization are inventoried".
            (NIST_CSF_V1_1, "ID.AM-1"),
            // "Software platforms and applications within the organization are
            // inventoried".
            (NIST_CSF_V1_1, "ID.AM-2"),
            // "Establish and maintain baseline configurations and inventories".
            (NIST_SP_800_171_R2, "3.4.1"),
            // "Establish and maintain detailed enterprise asset inventory".
            (CIS_CONTROLS_V8_0, "1.1"),
            // "An inventory of system components that are in scope is maintained".
            (PCI_DSS_V4_0, "12.5.1"),
            // "Inventory of information and other associated assets".
            (ISO_IEC_27001_2022_ANNEX_A, "5.9"),
            // "Asset inventories are maintained".
            (CRI_PROFILE_V2_2, "ID.AM-01.01"),
            // "Logical access security software, infrastructure and architectures".
            (SSAE_18_SOC_2, "CC6.1"),
        ],
    ),
    (
        "asset-performance-management",
        &[
            // "System monitoring".
            (NIST_SP_800_53_R5, "SI-4"),
            // "Monitoring for unauthorized personnel, connections, devices, and software
            // is performed".
            (NIST_CSF_V1_1, "DE.CM-7"),
            // "Monitor organizational systems, including inbound and outbound
            // communications traffic".
            (NIST_SP_800_171_R2, "3.14.6"),
            // "Failures of critical security control systems are detected, alerted and
            // addressed promptly".
            (PCI_DSS_V4_0, "10.7.2"),
            // "Monitoring activities".
            (ISO_IEC_27001_2022_ANNEX_A, "8.16"),
            // "Computing hardware and software are monitored to find potentially adverse
            // events".
            (CRI_PROFILE_V2_2, "DE.CM-09.01"),
            // "Analyze logs, findings and metrics centrally".
            (AWS_WAF_V10, "SEC04-BP02"),
            // "Monitoring of system components for anomalies".
            (SSAE_18_SOC_2, "CC7.2"),
        ],
    ),
    (
        "enable-centralized-logging",
        &[
            // "Event logging".
            (NIST_SP_800_53_R5, "AU-2"),
            // "Audit record review, analysis and reporting".
            (NIST_SP_800_53_R5, "AU-6"),
            // "Event data are collected and correlated from multiple sources and
            // sensors".
            (NIST_CSF_V1_1, "DE.AE-3"),
            // "Create, protect and retain system audit records".
            (NIST_SP_800_171_R2, "3.3.1"),
            // "Audit logs are enabled and active for all system components".
            (PCI_DSS_V4_0, "10.2.1"),
            // "Collect audit logs".
            (CIS_CONTROLS_V8_0, "8.2"),
            // "Logging".
            (ISO_IEC_27001_2022_ANNEX_A, "8.15"),
            // "Event logs are generated and made available for monitoring".
            (CRI_PROFILE_V2_2, "DE.CM-09.01"),
            // "Configure service and application logging".
            (AWS_WAF_V10, "SEC04-BP01"),
            // "Monitoring of system components for anomalies".
            (SSAE_18_SOC_2, "CC7.2"),
        ],
    ),
    (
        "log-integrity-protection",
        &[
            // "Protection of audit information".
            (NIST_SP_800_53_R5, "AU-9"),
            // "Audit/log records are determined, documented, implemented and reviewed".
            (NIST_CSF_V1_1, "PR.PT-1"),
            // "Protect audit information and audit logging tools from unauthorized
            // access, modification and deletion".
            (NIST_SP_800_171_R2, "3.3.8"),
            // "Audit log files are protected to prevent modifications".
            (PCI_DSS_V4_0, "10.3.2"),
            // "Ensure adequate audit log storage".
            (CIS_CONTROLS_V8_0, "8.3"),
            // "Logging".
            (ISO_IEC_27001_2022_ANNEX_A, "8.15"),
            // "The integrity of logged data is protected".
            (CRI_PROFILE_V2_2, "PR.DS-06.01"),
            // "Monitoring of system components for anomalies".
            (SSAE_18_SOC_2, "CC7.2"),
        ],
    ),
    (
        "log-retention-enforcement",
        &[
            // "Audit record retention".
            (NIST_SP_800_53_R5, "AU-11"),
            // "Create, protect and retain system audit records".
            (NIST_SP_800_171_R2, "3.3.1"),
            // "Audit/log records are determined, documented, implemented and reviewed".
            (NIST_CSF_V1_1, "PR.PT-1"),
            // "Audit log history is retained for at least 12 months".
            (PCI_DSS_V4_0, "10.5.1"),
            // "Retain audit logs".
            (CIS_CONTROLS_V8_0, "8.10"),
            // "Protection of records".
            (ISO_IEC_27001_2022_ANNEX_A, "5.33"),
            // "Records are retained for the defined retention period".
            (CRI_PROFILE_V2_2, "PR.IP-04.01"),
            // "Monitoring of system components for anomalies".
            (SSAE_18_SOC_2, "CC7.2"),
        ],
    ),
    (
        "encrypt-data-at-rest-in-storage-services",
        &[
            // "Protection of information at rest".
            (NIST_SP_800_53_R5, "SC-28"),
            // "Data-at-rest is protected".
            (NIST_CSF_V1_1, "PR.DS-1"),
            // "Protect the confidentiality of CUI at rest".
            (NIST_SP_800_171_R2, "3.13.16"),
            // "Primary account number is rendered unreadable wherever it is stored".
            (PCI_DSS_V4_0, "3.5.1"),
            // "Encrypt sensitive data at rest".
            (CIS_CONTROLS_V8_0, "3.11"),
            // "Use of cryptography".
            (ISO_IEC_27001_2022_ANNEX_A, "8.24"),
            // "Data at rest is protected by encryption".
            (CRI_PROFILE_V2_2, "PR.DS-01.01"),
            // "Enforce encryption at rest".
            (AWS_WAF_V10, "SEC08-BP02"),
            // "Logical access security software, infrastructure and architectures".
            (SSAE_18_SOC_2, "CC6.1"),
        ],
    ),
    (
        "enforce-encryption-in-transit",
        &[
            // "Transmission confidentiality and integrity".
            (NIST_SP_800_53_R5, "SC-8"),
            // "Data-in-transit is protected".
            (NIST_CSF_V1_1, "PR.DS-2"),
            // "Implement cryptographic mechanisms to prevent unauthorized disclosure of
            // CUI during transmission".
            (NIST_SP_800_171_R2, "3.13.8"),
            // "Strong cryptography is implemented to safeguard data in transit".
            (PCI_DSS_V4_0, "4.2.1"),
            // "Encrypt sensitive data in transit".
            (CIS_CONTROLS_V8_0, "3.10"),
            // "Use of cryptography".
            (ISO_IEC_27001_2022_ANNEX_A, "8.24"),
            // "Data in transit is protected by encryption".
            (CRI_PROFILE_V2_2, "PR.DS-02.01"),
            // "Enforce encryption in transit".
            (AWS_WAF_V10, "SEC09-BP02"),
            // "Transmission, movement and removal of information".
            (SSAE_18_SOC_2, "CC6.7"),
        ],
    ),
    (
        "certificate-lifecycle-management",
        &[
            // "Public key infrastructure certificates".
            (NIST_SP_800_53_R5, "SC-17"),
            // "Identities and credentials are issued, managed, verified, revoked and
            // audited for authorized devices, users and processes".
            (NIST_CSF_V1_1, "PR.AC-1"),
            // "Establish and manage cryptographic keys for cryptography employed in
            // organizational systems".
            (NIST_SP_800_171_R2, "3.13.10"),
            // "An inventory of the entity's trusted keys and certificates is
            // maintained".
            (PCI_DSS_V4_0, "4.2.1.1"),
            // "Use of cryptography".
            (ISO_IEC_27001_2022_ANNEX_A, "8.24"),
            // "Logical access security software, infrastructure and architectures".
            (SSAE_18_SOC_2, "CC6.1"),
        ],
    ),
    (
        "restrict-public-access",
        &[
            // "Access enforcement".
            (NIST_SP_800_53_R5, "AC-3"),
            // "Access permissions and authorizations are managed, incorporating the
            // principles of least privilege".
            (NIST_CSF_V1_1, "PR.AC-4"),
            // "Protections against data leaks are implemented".
            (NIST_CSF_V1_1, "PR.DS-5"),
            // "Control the flow of CUI in accordance with approved authorizations".
            (NIST_SP_800_171_R2, "3.1.3"),
            // "Inbound traffic to the cardholder data environment is restricted".
            (PCI_DSS_V4_0, "1.3.1"),
            // "Configure data access control lists".
            (CIS_CONTROLS_V8_0, "3.3"),
            // "Access control".
            (ISO_IEC_27001_2022_ANNEX_A, "5.15"),
            // "Access permissions are granted on a least-privilege basis".
            (CRI_PROFILE_V2_2, "PR.AC-04.01"),
            // "Logical access security software, infrastructure and architectures".
            (SSAE_18_SOC_2, "CC6.1"),
        ],
    ),
    (
        "privileged-access-approval",
        &[
            // "Least privilege".
            (NIST_SP_800_53_R5, "AC-6"),
            // "Access permissions and authorizations are managed, incorporating the
            // principles of least privilege".
            (NIST_CSF_V1_1, "PR.AC-4"),
            // "Employ the principle of least privilege".
            (NIST_SP_800_171_R2, "3.1.5"),
            // "Group, shared and generic accounts are managed".
            (PCI_DSS_V4_0, "8.2.2"),
            // "Restrict administrator privileges to dedicated administrator accounts".
            (CIS_CONTROLS_V8_0, "5.4"),
            // "Privileged access rights".
            (ISO_IEC_27001_2022_ANNEX_A, "8.2"),
            // "Privileged access is approved and recorded".
            (CRI_PROFILE_V2_2, "PR.AC-04.03"),
            // "Grant least privilege access".
            (AWS_WAF_V10, "SEC03-BP02"),
            // "Registration and authorization of new internal and external users".
            (SSAE_18_SOC_2, "CC6.3"),
        ],
    ),
    (
        "require-multi-factor-authentication",
        &[
            // "Identification and authentication (organizational users)".
            (NIST_SP_800_53_R5, "IA-2"),
            // "Users, devices and other assets are authenticated commensurate with the
            // risk of the transaction".
            (NIST_CSF_V1_1, "PR.AC-7"),
            // "Use multifactor authentication for local and network access".
            (NIST_SP_800_171_R2, "3.5.3"),
            // "Multi-factor authentication is implemented for all access".
            (PCI_DSS_V4_0, "8.4.2"),
            // "Require MFA for administrative access".
            (CIS_CONTROLS_V8_0, "6.5"),
            // "Authentication information".
            (ISO_IEC_27001_2022_ANNEX_A, "5.17"),
            // "Multi-factor authentication is required for privileged access".
            (CRI_PROFILE_V2_2, "PR.AC-01.02"),
            // "Use strong sign-in mechanisms".
            (AWS_WAF_V10, "SEC02-BP01"),
            // "Logical access security software, infrastructure and architectures".
            (SSAE_18_SOC_2, "CC6.1"),
        ],
    ),
    (
        "credential-rotation",
        &[
            // "Authenticator management".
            (NIST_SP_800_53_R5, "IA-5"),
            // "Identities and credentials are issued, managed, verified, revoked and
            // audited".
            (NIST_CSF_V1_1, "PR.AC-1"),
            // "Prohibit password reuse for a specified number of generations".
            (NIST_SP_800_171_R2, "3.5.8"),
            // "Passwords are changed at least once every 90 days".
            (PCI_DSS_V4_0, "8.3.9"),
            // "Use unique passwords".
            (CIS_CONTROLS_V8_0, "5.2"),
            // "Authentication information".
            (ISO_IEC_27001_2022_ANNEX_A, "5.17"),
            // "Credentials are rotated at a defined frequency".
            (CRI_PROFILE_V2_2, "PR.AC-01.03"),
            // "Use temporary credentials".
            (AWS_WAF_V10, "SEC02-BP02"),
            // "Logical access security software, infrastructure and architectures".
            (SSAE_18_SOC_2, "CC6.1"),
        ],
    ),
    (
        "password-strength-enforcement",
        &[
            // "Authenticator management".
            (NIST_SP_800_53_R5, "IA-5"),
            // "Identities and credentials are issued, managed, verified, revoked and
            // audited".
            (NIST_CSF_V1_1, "PR.AC-1"),
            // "Enforce a minimum password complexity and change of characters when new
            // passwords are created".
            (NIST_SP_800_171_R2, "3.5.7"),
            // "Passwords have a minimum length and contain both numeric and alphabetic
            // characters".
            (PCI_DSS_V4_0, "8.3.6"),
            // "Use unique passwords".
            (CIS_CONTROLS_V8_0, "5.2"),
            // "Authentication information".
            (ISO_IEC_27001_2022_ANNEX_A, "5.17"),
            // "Authentication credentials meet defined strength requirements".
            (CRI_PROFILE_V2_2, "PR.AC-01.01"),
            // "Logical access security software, infrastructure and architectures".
            (SSAE_18_SOC_2, "CC6.1"),
        ],
    ),
    (
        "data-egress-monitoring",
        &[
            // "Audit record generation".
            (NIST_SP_800_53_R5, "AU-12"),
            // "System monitoring".
            (NIST_SP_800_53_R5, "SI-4"),
            // "The network is monitored to detect potential cybersecurity events".
            (NIST_CSF_V1_1, "DE.CM-1"),
            // "Monitor organizational systems, including inbound and outbound
            // communications traffic".
            (NIST_SP_800_171_R2, "3.14.6"),
            // "Audit logs are enabled and active for all system components".
            (PCI_DSS_V4_0, "10.2.1"),
            // "Collect network traffic flow logs".
            (CIS_CONTROLS_V8_0, "13.6"),
            // "Monitoring activities".
            (ISO_IEC_27001_2022_ANNEX_A, "8.16"),
            // "Network traffic is monitored for anomalous activity".
            (CRI_PROFILE_V2_2, "DE.CM-01.01"),
            // "Monitoring of system components for anomalies".
            (SSAE_18_SOC_2, "CC7.2"),
        ],
    ),
    (
        "restrict-administrative-port-exposure",
        &[
            // "Boundary protection".
            (NIST_SP_800_53_R5, "SC-7"),
            // "Network integrity is protected".
            (NIST_CSF_V1_1, "PR.AC-5"),
            // "Monitor, control and protect communications at the external boundaries
            // of organizational systems".
            (NIST_SP_800_171_R2, "3.13.1"),
            // "Inbound traffic to the cardholder data environment is restricted".
            (PCI_DSS_V4_0, "1.3.1"),
            // "Implement and manage a firewall on servers".
            (CIS_CONTROLS_V8_0, "4.4"),
            // "Networks security".
            (ISO_IEC_27001_2022_ANNEX_A, "8.20"),
            // "Network boundaries restrict administrative access".
            (CRI_PROFILE_V2_2, "PR.IR-01.03"),
            // "Create network layers".
            (AWS_WAF_V10, "SEC05-BP01"),
            // "Protection against threats from outside system boundaries".
            (SSAE_18_SOC_2, "CC6.6"),
        ],
    ),
    (
        "intrusion-detection-deployment",
        &[
            // "System monitoring".
            (NIST_SP_800_53_R5, "SI-4"),
            // "The network is monitored to detect potential cybersecurity events".
            (NIST_CSF_V1_1, "DE.CM-1"),
            // "Monitor organizational systems, including inbound and outbound
            // communications traffic".
            (NIST_SP_800_171_R2, "3.14.6"),
            // "Intrusion-detection and intrusion-prevention techniques are used".
            (PCI_DSS_V4_0, "11.5.1"),
            // "Centralize security event alerting".
            (CIS_CONTROLS_V8_0, "13.1"),
            // "Monitoring activities".
            (ISO_IEC_27001_2022_ANNEX_A, "8.16"),
            // "Malicious activity is detected and analyzed".
            (CRI_PROFILE_V2_2, "DE.CM-01.02"),
            // "Analyze logs, findings and metrics centrally".
            (AWS_WAF_V10, "SEC04-BP02"),
            // "Monitoring of system components for anomalies".
            (SSAE_18_SOC_2, "CC7.2"),
        ],
    ),
];

/// The requirement family each `NIST-SP-800-171-r2` section belongs to:
/// `(section, that family's abbreviation)`.
///
/// A requirement is numbered `3.<section>.<n>` and its section IS its family, in the
/// order the publication lists them.
const NIST_800_171_FAMILIES: &[(&str, &str)] = &[
    // Access control.
    ("3.1", "AC"),
    // Awareness and training.
    ("3.2", "AT"),
    // Audit and accountability.
    ("3.3", "AU"),
    // Configuration management.
    ("3.4", "CM"),
    // Identification and authentication.
    ("3.5", "IA"),
    // Incident response.
    ("3.6", "IR"),
    // Maintenance.
    ("3.7", "MA"),
    // Media protection.
    ("3.8", "MP"),
    // Personnel security.
    ("3.9", "PS"),
    // Physical protection.
    ("3.10", "PE"),
    // Risk assessment.
    ("3.11", "RA"),
    // Security assessment.
    ("3.12", "CA"),
    // System and communications protection.
    ("3.13", "SC"),
    // System and information integrity.
    ("3.14", "SI"),
];

/// The ISO/IEC 27001:2013 Annex A controls an ISO/IEC 27001:2022 Annex A clause
/// restates: `(the 2022 clause, the 2013 controls it corresponds to)`.
///
/// The 2022 edition consolidated and renumbered the 2013 Annex A controls, and the
/// correspondence between the two numberings is published WITH the edition — so the
/// 2013 edition's rows are derived from it rather than cross-walked a second time by
/// hand, which is also what keeps the two editions from disagreeing about which
/// requirement a control satisfies. A clause the 2022 edition introduced corresponds to
/// no 2013 control and so restates nothing. Each row's comment is the 2022 clause's own
/// title, then the titles of the 2013 controls it consolidated.
const ANNEX_A_2013_CORRESPONDENCE: &[(&str, &[&str])] = &[
    // "Inventory of information and other associated assets" <- "inventory of assets",
    // "ownership of assets".
    ("5.9", &["A.8.1.1", "A.8.1.2"]),
    // "Access control" <- "access control policy", "information access restriction".
    ("5.15", &["A.9.1.1", "A.9.4.1"]),
    // "Authentication information" <- "management of secret authentication information
    // of users", "use of secret authentication information", "password management
    // system".
    ("5.17", &["A.9.2.4", "A.9.3.1", "A.9.4.3"]),
    // "Protection of records" <- "protection of records".
    ("5.33", &["A.18.1.3"]),
    // "Privileged access rights" <- "management of privileged access rights".
    ("8.2", &["A.9.2.3"]),
    // "Logging" <- "event logging", "protection of log information", "administrator and
    // operator logs".
    ("8.15", &["A.12.4.1", "A.12.4.2", "A.12.4.3"]),
    // "Monitoring activities": introduced by the 2022 edition, so no 2013 control
    // states it.
    ("8.16", &[]),
    // "Networks security" <- "network controls".
    ("8.20", &["A.13.1.1"]),
    // "Use of cryptography" <- "policy on the use of cryptographic controls", "key
    // management".
    ("8.24", &["A.10.1.1", "A.10.1.2"]),
];

/// The ISO/IEC 27001:2013 Annex A controls the 2022 Annex A `clause` restates, in the
/// 2013 edition's own numbering.
///
/// A clause this catalog cross-walks into but does not record a correspondence for would
/// otherwise be served at the 2022 edition only, with no signal — hence the assertion.
fn annex_a_2013_controls(clause: &str) -> &'static [&'static str] {
    let recorded = ANNEX_A_2013_CORRESPONDENCE
        .iter()
        .find(|(candidate, _)| *candidate == clause);
    debug_assert!(
        recorded.is_some(),
        "{clause}: every Annex A clause the catalog cross-walks into records the 2013 \
         controls it restates, or none where the 2022 edition introduced it"
    );
    recorded.map_or(&[], |(_, controls)| *controls)
}

/// The NIST SP 800-53 control identifiers this catalog cross-walks into that revision 4
/// of the publication also carries under the same designation.
///
/// A FedRAMP baseline is composed of the controls of the 800-53 revision it is read at,
/// under the identifier that publication gives them. Revision 5 is not a renaming of
/// revision 4: it introduced control families revision 4 has none of and withdrew
/// controls revision 4 carried, so a revision-4 baseline can only name a control both
/// revisions publish — which is why the revision-4 rows are restricted to this list
/// rather than derived from every revision-5 identifier. A control only revision 5
/// publishes is served at the revision-5 baseline alone.
const CONTROLS_IN_800_53_R4: &[&str] = &[
    // Access control.
    "AC-3", "AC-6",
    // Audit and accountability.
    "AU-2", "AU-6", "AU-9", "AU-11", "AU-12",
    // Configuration management.
    "CM-8",
    // Identification and authentication.
    "IA-2", "IA-5",
    // System and communications protection.
    "SC-7", "SC-8", "SC-17", "SC-28",
    // System and information integrity.
    "SI-4",
];

/// Both FedRAMP baselines the catalog publishes, for a control each of their revisions
/// carries.
const BOTH_FEDRAMP_BASELINES: &[&str] = &[FEDRAMP_R4, FEDRAMP_R5];

/// The revision-5 baseline alone, for a control only revision 5 publishes.
const FEDRAMP_R5_BASELINE: &[&str] = &[FEDRAMP_R5];

/// The FedRAMP baselines a NIST SP 800-53 `control` is part of, as the catalog publishes
/// them: the revision-5 baseline for every control revision 5 publishes, and the
/// revision-4 baseline for the controls revision 4 publishes too (see
/// [`CONTROLS_IN_800_53_R4`]).
fn fedramp_baselines(control: &str) -> &'static [&'static str] {
    if CONTROLS_IN_800_53_R4.contains(&control) {
        BOTH_FEDRAMP_BASELINES
    } else {
        FEDRAMP_R5_BASELINE
    }
}

/// The `CMMC-Level2` practice that restates an `NIST-SP-800-171-r2` requirement.
///
/// A Level 2 practice IS an 800-171 requirement, designated `<family>.L2-<requirement>`
/// — so the practice is DERIVED from the requirement rather than cross-walked by hand,
/// and a requirement whose section names no family (none, as the table is the whole
/// list) yields no practice.
fn cmmc_practice(requirement: &str) -> Option<String> {
    let mut parts = requirement.split('.');
    let section = format!("{}.{}", parts.next()?, parts.next()?);
    let family = NIST_800_171_FAMILIES
        .iter()
        .find(|(candidate, _)| *candidate == section)
        .map(|(_, family)| *family)?;
    Some(format!("{family}.L2-{requirement}"))
}

/// Every framework requirement a control satisfies: its objective's cross-walk, the
/// items naming its own check, and the restatements those imply.
///
/// An edition that publishes another edition's or another framework's requirements
/// rather than requirements of its own is DERIVED from the rows it restates instead of
/// being written out, each in the numbering of the edition it is served under:
///
/// * a FedRAMP baseline is composed of NIST SP 800-53 controls, under the control
///   identifier the publication gives them; every control this catalog cross-walks into
///   is in a family both published revisions carry (`AC`, `AU`, `CM`, `IA`, `SC`, `SI`),
///   and a revision-4 baseline names only the controls revision 4 publishes too (see
///   [`fedramp_baselines`]);
/// * a `CMMC-Level2` practice is an `NIST-SP-800-171-r2` requirement (see
///   [`cmmc_practice`]);
/// * an ISO/IEC 27001:2013 Annex A control is the 2013 numbering of the 2022 Annex A
///   clause that consolidated it (see [`annex_a_2013_controls`]);
/// * the trust services criteria are numbered the same in each edition of SOC 2, so the
///   dated edition restates the criterion under its own identifier.
///
/// Each `(framework, item)` pair appears once, however many of the sources name it.
pub(super) fn framework_mappings(seed: &ControlSeed) -> Vec<(&'static str, String)> {
    let objective = OBJECTIVE_FRAMEWORKS
        .iter()
        .find(|(slug, _)| *slug == seed.implements)
        .map(|(_, items)| *items)
        .unwrap_or(&[]);

    let mut mappings: Vec<(&'static str, String)> = Vec::new();
    for &(framework, item) in objective.iter().chain(seed.frameworks) {
        let restatements = match framework {
            NIST_SP_800_53_R5 => fedramp_baselines(item)
                .iter()
                .map(|baseline| (*baseline, item.to_string()))
                .collect(),
            NIST_SP_800_171_R2 => cmmc_practice(item)
                .map(|practice| vec![(CMMC_LEVEL_2, practice)])
                .unwrap_or_default(),
            ISO_IEC_27001_2022_ANNEX_A => annex_a_2013_controls(item)
                .iter()
                .map(|control| (ISO_IEC_27001, (*control).to_string()))
                .collect(),
            SSAE_18_SOC_2 => vec![(SSAE_18_SOC_2_OCT_2023, item.to_string())],
            _ => Vec::new(),
        };
        for mapping in std::iter::once((framework, item.to_string())).chain(restatements) {
            if !mappings.contains(&mapping) {
                mappings.push(mapping);
            }
        }
    }
    mappings
}

/// The aliases a control is published under.
///
/// An alias is a "human-readable designator, such as SH.S3.1", and "[s]everal aliases
/// can refer to the same control across different Amazon Web Services services or
/// compliance frameworks" (`smithy_query(query="doc", shape="ControlSummary",
/// member="Aliases")`). A designator therefore names the control ON the surface that
/// issues it, and WHICH designator that surface issues is published content: the catalog
/// publishes most Security Hub controls under `SH.` followed by the control's own
/// Security Hub id, and others under a designator of its own (`SH.{SERVICE}.DT.{N}`), so
/// a designator cannot be computed from the identifier and is read off the control's row
/// instead. The one exception is the Control Tower designator, which IS the `AWS-GR_…`
/// identifier the row already records.
pub(super) fn aliases(seed: &ControlSeed) -> Vec<String> {
    // A Security Hub control is published under a Security Hub designator, so a row that
    // records none would be served without the designator a client joins it to its
    // Security Hub control by.
    debug_assert!(
        seed.implementation_type != SECURITY_HUB_CONTROL
            || seed
                .designators
                .iter()
                .any(|designator| designator.starts_with(SECURITY_HUB_ALIAS_PREFIX)),
        "{}: a Security Hub control records the {SECURITY_HUB_ALIAS_PREFIX} designator \
         the catalog publishes it under",
        seed.slug
    );
    let mut aliases: Vec<String> = seed
        .designators
        .iter()
        .map(|designator| (*designator).to_string())
        .collect();
    if let Some(identifier) = seed.control_tower_identifier {
        aliases.push(identifier.to_string());
    }
    aliases
}

/// The modeled `ParameterRequirementSummary` for a control that supports `parameters`.
///
/// It is "[a] summary that indicates whether the control requires parameters, accepts
/// optional parameters, or does not support parameters" (`smithy_query(query="doc",
/// shape="GetControlResponse", member="ParameterRequirementSummary")`), and the modeled
/// enum admits exactly `REQUIRED`, `OPTIONAL` and `NONE`. It is therefore a function of
/// the control's `Parameters` list, not an independent fact about the control: NONE when
/// the control supports no parameter, REQUIRED when at least one must be supplied, and
/// OPTIONAL when it accepts parameters and none is required.
pub(super) fn parameter_requirement_summary(parameters: &[(&str, &str)]) -> &'static str {
    if parameters.is_empty() {
        "NONE"
    } else if parameters
        .iter()
        .any(|(_, requirement)| *requirement == REQUIRED)
    {
        "REQUIRED"
    } else {
        "OPTIONAL"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every ISO/IEC 27001:2022 Annex A clause this catalog cross-walks into records
    /// what it restates in the 2013 edition's numbering.
    ///
    /// The lookup guards this with a `debug_assert!`, which a release build compiles
    /// out — so the invariant is checked here as well, where it holds for every build
    /// profile. A clause the 2022 edition introduced records an empty list, which is a
    /// recorded answer, not a missing one.
    #[test]
    fn every_cross_walked_annex_a_clause_records_its_2013_correspondence() {
        let cross_walked = OBJECTIVE_FRAMEWORKS
            .iter()
            .flat_map(|(_, items)| items.iter())
            .chain(
                PUBLISHED_CONTROLS
                    .iter()
                    .flat_map(|seed| seed.frameworks.iter()),
            )
            .filter(|(framework, _)| *framework == ISO_IEC_27001_2022_ANNEX_A);
        let mut covered = 0usize;
        for (_, clause) in cross_walked {
            assert!(
                ANNEX_A_2013_CORRESPONDENCE
                    .iter()
                    .any(|(candidate, _)| candidate == clause),
                "case clause {clause}: the catalog cross-walks into it, so it records \
                 the 2013 controls it restates (or none, where the 2022 edition \
                 introduced it)"
            );
            covered += 1;
        }
        assert!(
            covered > 0,
            "case catalog: the cross-walk names {ISO_IEC_27001_2022_ANNEX_A} clauses"
        );
    }

    /// A FedRAMP baseline names only the NIST SP 800-53 controls the revision it is read
    /// at publishes.
    ///
    /// The rows are a control both revisions carry and controls only revision 5 carries:
    /// revision 5 introduced the supply chain risk management (`SR`) and personally
    /// identifiable information processing and transparency (`PT`) families, which
    /// revision 4 has none of, so a revision-4 baseline cannot name one of their
    /// controls.
    #[test]
    fn a_fedramp_baseline_names_only_the_controls_its_revision_publishes() {
        let both: &[&str] = &[FEDRAMP_R4, FEDRAMP_R5];
        let revision_5_only: &[&str] = &[FEDRAMP_R5];
        for (case, control, expected) in [
            ("an access enforcement control both revisions carry", "AC-3", both),
            (
                "a supply chain control only revision 5 carries",
                "SR-3",
                revision_5_only,
            ),
            (
                "a PII processing control only revision 5 carries",
                "PT-2",
                revision_5_only,
            ),
        ] {
            let baselines = fedramp_baselines(control);
            assert_eq!(
                baselines, expected,
                "case {case} ({control}): expected {expected:?}, got {baselines:?}"
            );
        }
    }
}
