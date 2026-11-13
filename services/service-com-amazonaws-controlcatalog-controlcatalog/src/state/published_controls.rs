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
//!   control Control Tower publishes (a row carrying a `control_tower_identifier`), and
//!   the implementing AWS Config managed rule's own title and description for a control
//!   the catalog publishes under a `CONFIG.…` designator. They are catalog CONTENT, not a
//!   summary of the row written here: `Description` is "[a] description of the control,
//!   as it may appear in the console" (`smithy_query(query="shape",
//!   shape="ControlSummary")`), and the catalog is account- and Region-independent, so
//!   one ARN carries one `Name` and one `Description` for every caller and every run.
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
//! * `aliases` are DERIVED for the two designators that are functions of an identifier
//!   (see [`aliases`]); only a designator that is neither — a `CONFIG.…` or `CT.…`
//!   catalog designator — is written in the row.
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
//! under that identifier and with that release time; for a control it does not name —
//! and for the ontology in [`super::published_ontology`] — this emulator mints an
//! identifier deterministically from the row's `slug` (see `super::catalog_id`) and
//! derives the timestamp the same deterministic way (see `super::seed_catalog`). The
//! slug never appears on the wire.

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

/// `ControlBehavior`: the control reports on resources that already exist.
const DETECTIVE: &str = "DETECTIVE";
/// `ControlBehavior`: the control blocks a non-compliant action.
const PREVENTIVE: &str = "PREVENTIVE";
/// `ControlBehavior`: the control rejects a non-compliant deployment before it runs.
const PROACTIVE: &str = "PROACTIVE";

/// The `ControlSeverity` values this table's controls are published with, as the modeled
/// enum spells them. The enum also declares `LOW`, which no control published here
/// carries: a control's severity is the one the catalog publishes for it, not a value
/// chosen to cover the enum.
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
/// designator", and `S3.1` is that control's own Security Hub id.
pub(super) const SECURITY_HUB_ALIAS_PREFIX: &str = "SH.";

/// `GovernedProviders` values: "a value of AWS indicates that the control governs
/// Amazon Web Services resources".
const AWS: &[&str] = &["AWS"];
const GCP: &[&str] = &["GCP"];

/// Region sets a REGIONAL control in this catalog deploys to. A control is available
/// in the Regions its implementing service is, which differs between controls, so the
/// table picks the set per row.
const BROAD_REGIONS: &[&str] = &[
    "ap-southeast-2",
    "eu-central-1",
    "eu-west-1",
    "us-east-1",
    "us-east-2",
    "us-west-2",
];
const CORE_REGIONS: &[&str] = &["eu-west-1", "us-east-1", "us-west-2"];
const US_REGIONS: &[&str] = &["us-east-1", "us-west-2"];
/// A GLOBAL control deploys to no Region of its own.
const NO_REGIONS: &[&str] = &[];

/// A control that supports no control parameter: nothing is supplied when it is
/// enabled, so its `ParameterRequirementSummary` is `NONE`.
const NO_PARAMETERS: &[(&str, &str)] = &[];

/// A control that is published under no catalog designator of its own (its aliases,
/// if any, are the derived ones).
const NO_DESIGNATORS: &[&str] = &[];

/// A resource set that no available resource type can represent: "If
/// GovernedResources cannot be represented by available resource types, it's returned
/// as an empty list."
const NO_GOVERNED_RESOURCES: &[&str] = &[];

// The compliance frameworks this catalog cross-walks its controls into.
//
// A FRAMEWORK mapping names the framework by the compact, version-bearing designator
// the catalog publishes it under — the framework's abbreviation, the revision or
// version it is read at, and nothing else (`NIST-CSF-v1.1`, `PCI-DSS-v4.0`) — because
// the designator is what identifies a framework ACROSS the surfaces that cross-walk
// into it, while a prose title is one surface's wording of it. The item beside it in a
// mapping row is that framework's own identifier for the requirement (`PR.AC-4`). The
// modeled `FrameworkName` admits 3..250 characters, so either form would serialize;
// the designator is the one a client can join on.
pub(super) const NIST_SP_800_53_R5: &str = "NIST-SP-800-53-r5";
pub(super) const NIST_SP_800_171_R2: &str = "NIST-SP-800-171-r2";
pub(super) const NIST_CSF_V1_1: &str = "NIST-CSF-v1.1";
pub(super) const PCI_DSS_V4_0: &str = "PCI-DSS-v4.0";
pub(super) const CIS_AWS_BENCHMARK_V3_0: &str = "CIS-AWS-Benchmark-v3.0";
pub(super) const CIS_CONTROLS_V8_0: &str = "CIS-v8.0";
pub(super) const CIS_GCP_BENCHMARK_V2_0: &str = "CIS-GCP-Benchmark-v2.0";
pub(super) const FEDRAMP_R5: &str = "FedRAMP-r5";
pub(super) const ISO_IEC_27001: &str = "ISO-IEC-27001";
pub(super) const CRI_PROFILE_V2_2: &str = "CRI-Profile-v2.2";
pub(super) const CMMC_LEVEL_2: &str = "CMMC-Level2";
pub(super) const SSAE_18_SOC_2: &str = "SSAE-18-SOC-2";
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
    FEDRAMP_R5,
    ISO_IEC_27001,
    CRI_PROFILE_V2_2,
    CMMC_LEVEL_2,
    SSAE_18_SOC_2,
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
    /// Catalog designators that are not a function of an identifier (`CONFIG.…`,
    /// `CT.…`); the `SH.` and Control Tower designators are derived, not written here.
    pub(super) designators: &'static [&'static str],
    /// `ControlScope`.
    pub(super) scope: &'static str,
    /// Empty for a GLOBAL control: `DeployableRegions` does not apply to it.
    pub(super) deployable_regions: &'static [&'static str],
    /// `(name, ControlParameterRequirement)` for each EXEMPTION parameter the control
    /// supports, named from the modeled vocabulary ([`ALLOWED_REGIONS`] and the names
    /// beside it). The parameters of the surface that implements the control are not
    /// written here: they belong to that surface, so a control an AWS Config managed rule
    /// implements takes its parameters from [`CONFIG_RULE_PARAMETERS`]. Read a control's
    /// whole parameter list through [`parameters`].
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
        name: "Checks whether an Amazon Web Services account is part of an organization",
        description: "Reports an account that is not a member of an Amazon Web Services \
                      organization, so organization-wide governance cannot reach it.",
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
        slug: "cloud-trail-enabled",
        name: "Checks whether CloudTrail is enabled in an Amazon Web Services account",
        description: "Reports an account that records no management events through a \
                      CloudTrail trail.",
        behavior: DETECTIVE,
        severity: HIGH,
        implementation_type: CONFIG_RULE,
        implementation_identifier: Some("CLOUD_TRAIL_ENABLED"),
        control_tower_identifier: None,
        designators: NO_DESIGNATORS,
        scope: REGIONAL,
        deployable_regions: CORE_REGIONS,
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
        name: "Detect whether CloudTrail is enabled in a member account",
        description: "Reports a member account of the organization that records no \
                      management events through a CloudTrail trail.",
        behavior: DETECTIVE,
        severity: HIGH,
        // The same Config rule backs this Control Tower control and the Config-rule
        // control above: two published controls, one implementation.
        implementation_type: CONFIG_RULE,
        implementation_identifier: Some("CLOUD_TRAIL_ENABLED"),
        control_tower_identifier: Some("AWS-GR_DETECT_CLOUDTRAIL_ENABLED_ON_MEMBER_ACCOUNTS"),
        designators: NO_DESIGNATORS,
        scope: REGIONAL,
        deployable_regions: BROAD_REGIONS,
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
        deployable_regions: BROAD_REGIONS,
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
        name: "Checks whether CloudTrail log file validation is enabled",
        description: "Reports a CloudTrail trail that publishes no digest files, so log \
                      tampering cannot be detected.",
        behavior: DETECTIVE,
        severity: MEDIUM,
        implementation_type: CONFIG_RULE,
        implementation_identifier: Some("CLOUD_TRAIL_LOG_FILE_VALIDATION_ENABLED"),
        control_tower_identifier: None,
        designators: NO_DESIGNATORS,
        scope: REGIONAL,
        deployable_regions: CORE_REGIONS,
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
        deployable_regions: CORE_REGIONS,
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
        deployable_regions: BROAD_REGIONS,
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
        deployable_regions: BROAD_REGIONS,
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
        deployable_regions: BROAD_REGIONS,
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
        deployable_regions: BROAD_REGIONS,
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
        deployable_regions: CORE_REGIONS,
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
        name: "Checks whether the root user has an access key",
        description: "Reports an account whose root user holds an access key.",
        behavior: DETECTIVE,
        severity: CRITICAL,
        implementation_type: CONFIG_RULE,
        implementation_identifier: Some("IAM_ROOT_ACCESS_KEY_CHECK"),
        control_tower_identifier: None,
        designators: NO_DESIGNATORS,
        scope: GLOBAL,
        deployable_regions: NO_REGIONS,
        parameters: NO_PARAMETERS,
        governed_resources: &["AWS::IAM::AccessKey"],
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
        name: "Checks whether every IAM user has multi-factor authentication enabled",
        description: "Reports an IAM user with no multi-factor authentication device.",
        behavior: DETECTIVE,
        severity: MEDIUM,
        implementation_type: CONFIG_RULE,
        implementation_identifier: Some("IAM_USER_MFA_ENABLED"),
        control_tower_identifier: None,
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
        name: "Checks whether the account password policy meets the required strength",
        description: "Reports an account whose IAM password policy is weaker than the \
                      implementing rule requires.",
        behavior: DETECTIVE,
        severity: MEDIUM,
        implementation_type: CONFIG_RULE,
        implementation_identifier: Some("IAM_PASSWORD_POLICY"),
        control_tower_identifier: None,
        designators: NO_DESIGNATORS,
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
        name: "Checks whether an attached Amazon EBS volume is encrypted",
        description: "Reports an Amazon EBS volume that is attached to an instance and not \
                      encrypted.",
        behavior: DETECTIVE,
        severity: MEDIUM,
        implementation_type: CONFIG_RULE,
        implementation_identifier: Some("ENCRYPTED_VOLUMES"),
        control_tower_identifier: Some("AWS-GR_ENCRYPTED_VOLUMES"),
        designators: NO_DESIGNATORS,
        scope: REGIONAL,
        deployable_regions: BROAD_REGIONS,
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
        deployable_regions: CORE_REGIONS,
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
        slug: "incoming-ssh-disabled",
        name: "Checks whether a security group allows unrestricted SSH ingress",
        description: "Reports a security group that allows inbound traffic to port 22 from \
                      any address.",
        behavior: DETECTIVE,
        severity: HIGH,
        implementation_type: CONFIG_RULE,
        implementation_identifier: Some("INCOMING_SSH_DISABLED"),
        control_tower_identifier: Some("AWS-GR_RESTRICTED_SSH"),
        designators: NO_DESIGNATORS,
        scope: REGIONAL,
        deployable_regions: BROAD_REGIONS,
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
        deployable_regions: CORE_REGIONS,
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
        deployable_regions: CORE_REGIONS,
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
        name: "Checks whether Amazon GuardDuty is enabled and centrally managed",
        description: "Reports an account where Amazon GuardDuty is not enabled or is not \
                      administered from the organization's delegated account.",
        behavior: DETECTIVE,
        severity: HIGH,
        implementation_type: CONFIG_RULE,
        implementation_identifier: Some("GUARDDUTY_ENABLED_CENTRALIZED"),
        control_tower_identifier: None,
        designators: NO_DESIGNATORS,
        scope: REGIONAL,
        deployable_regions: BROAD_REGIONS,
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
        name: "Checks whether a CloudWatch log group is encrypted with a KMS key",
        description: "Reports a CloudWatch Logs log group that is not encrypted with a KMS \
                      key.",
        behavior: DETECTIVE,
        severity: MEDIUM,
        implementation_type: CONFIG_RULE,
        implementation_identifier: Some("CLOUDWATCH_LOG_GROUP_ENCRYPTED"),
        control_tower_identifier: None,
        designators: NO_DESIGNATORS,
        scope: REGIONAL,
        deployable_regions: CORE_REGIONS,
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
        designators: NO_DESIGNATORS,
        scope: REGIONAL,
        deployable_regions: CORE_REGIONS,
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
        designators: NO_DESIGNATORS,
        scope: REGIONAL,
        deployable_regions: BROAD_REGIONS,
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
        description: "Checks whether the root user holds an access key, and reports the \
                      finding in Security Hub.",
        behavior: DETECTIVE,
        severity: CRITICAL,
        implementation_type: SECURITY_HUB_CONTROL,
        implementation_identifier: Some("IAM.4"),
        control_tower_identifier: None,
        designators: NO_DESIGNATORS,
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
        description: "Checks whether every identity with a console password also has \
                      multi-factor authentication enabled.",
        behavior: DETECTIVE,
        severity: MEDIUM,
        implementation_type: SECURITY_HUB_CONTROL,
        implementation_identifier: Some("IAM.5"),
        control_tower_identifier: None,
        designators: NO_DESIGNATORS,
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
        designators: NO_DESIGNATORS,
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
        deployable_regions: CORE_REGIONS,
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
    /// the Unix epoch: `CreateTime` "notes the time when the control was released (start
    /// of its life) as a governance capability in Amazon Web Services"
    /// (`smithy_query(query="shape", shape="ControlSummary")`).
    pub(super) release_time: i64,
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
        release_time: 1_539_993_600,
    },
    PublishedCatalogRecord {
        slug: "s3-bucket-server-side-encryption-enabled",
        identifier: "d4wgeffz6izrb627c2yb8nq8d",
        release_time: 1_515_110_400,
    },
    // The account-wide and the log-archive control over public S3 reads were released
    // together, so the catalog dates both with one instant.
    PublishedCatalogRecord {
        slug: "s3-bucket-public-read-prohibited",
        identifier: "8sw3pbid15t9cbww8d2w2qwgf",
        release_time: 1_542_240_000,
    },
    PublishedCatalogRecord {
        slug: "audit-bucket-public-read-prohibited",
        identifier: "9jzlkwys54gzgsx3pmkqnx8l1",
        release_time: 1_542_240_000,
    },
    PublishedCatalogRecord {
        slug: "s3-bucket-public-write-prohibited",
        identifier: "9j9nwxj789d82sypnukhyyowy",
        release_time: 1_542_240_000,
    },
    PublishedCatalogRecord {
        slug: "audit-bucket-public-write-prohibited",
        identifier: "ci9ov6vzjoi6vzkajgademvvo",
        release_time: 1_542_240_000,
    },
    PublishedCatalogRecord {
        slug: "s3-bucket-ssl-requests-only",
        identifier: "e5efjudq0b347dgxn86o8fg2o",
        release_time: 1_479_772_800,
    },
    PublishedCatalogRecord {
        slug: "vpc-flow-logs-enabled",
        identifier: "22484xhzccf9twis5gmdt7uic",
        release_time: 1_536_710_400,
    },
    // Both RDS controls were released together, so they too share one instant.
    PublishedCatalogRecord {
        slug: "rds-snapshots-public-prohibited",
        identifier: "1h4eyqyyonp19dlrreqf1i3w0",
        release_time: 1_564_444_800,
    },
    PublishedCatalogRecord {
        slug: "rds-storage-encrypted",
        identifier: "e34kieahgkm0lggs5g0s412jt",
        release_time: 1_564_444_800,
    },
    // The Security Hub-implemented controls the catalog publishes under their `SH.`
    // designators: the instant recorded here is the one the catalog publishes as the
    // release time of the `SH.CloudTrail.1` control. This catalog holds no separate
    // record of the other two controls' release instants, and dates them with the same
    // one because the catalog publishes Security Hub's control identifiers as one
    // release and dates controls it released together with a single instant (see the
    // pairs above) — not because a record of each was read.
    PublishedCatalogRecord {
        slug: "securityhub-cloudtrail-1",
        identifier: "435u8u4w5bsf02tkxga5jisbc",
        release_time: 1_669_593_600,
    },
    PublishedCatalogRecord {
        slug: "securityhub-iam-4",
        identifier: "ezfflnh8xgshxb78grtt5sjc5",
        release_time: 1_669_593_600,
    },
    PublishedCatalogRecord {
        slug: "securityhub-iam-5",
        identifier: "6g3gbim493yofkc9eg6vvnahm",
        release_time: 1_669_593_600,
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
/// `None` when this catalog records no published record for it.
pub(super) fn published_release_time(slug: &str) -> Option<i64> {
    published_catalog_record(slug).map(|record| record.release_time)
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
        "cloud-trail-enabled",
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
            (ISO_IEC_27001, "A.5.9"),
            // "Asset inventories are maintained".
            (CRI_PROFILE_V2_2, "ID.AM-01.01"),
            // "Logical access security software, infrastructure and architectures".
            (SSAE_18_SOC_2, "CC6.1"),
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
            (ISO_IEC_27001, "A.8.15"),
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
            (ISO_IEC_27001, "A.8.15"),
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
            (ISO_IEC_27001, "A.5.33"),
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
            (ISO_IEC_27001, "A.8.24"),
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
            (ISO_IEC_27001, "A.8.24"),
            // "Data in transit is protected by encryption".
            (CRI_PROFILE_V2_2, "PR.DS-02.01"),
            // "Enforce encryption in transit".
            (AWS_WAF_V10, "SEC09-BP02"),
            // "Transmission, movement and removal of information".
            (SSAE_18_SOC_2, "CC6.7"),
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
            (ISO_IEC_27001, "A.5.15"),
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
            (ISO_IEC_27001, "A.8.2"),
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
            (ISO_IEC_27001, "A.5.17"),
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
            (ISO_IEC_27001, "A.5.17"),
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
            (ISO_IEC_27001, "A.5.17"),
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
            (ISO_IEC_27001, "A.8.16"),
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
            (ISO_IEC_27001, "A.8.20"),
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
            (ISO_IEC_27001, "A.8.16"),
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
/// Two frameworks here publish another framework's requirements rather than
/// requirements of their own, so their rows are DERIVED from the rows they restate
/// instead of being written out: a `FedRAMP-r5` baseline is composed of
/// `NIST-SP-800-53-r5` controls, under the same control identifier, and a
/// `CMMC-Level2` practice is an `NIST-SP-800-171-r2` requirement (see
/// [`cmmc_practice`]). Each `(framework, item)` pair appears once, however many of the
/// three sources name it.
pub(super) fn framework_mappings(seed: &ControlSeed) -> Vec<(&'static str, String)> {
    let objective = OBJECTIVE_FRAMEWORKS
        .iter()
        .find(|(slug, _)| *slug == seed.implements)
        .map(|(_, items)| *items)
        .unwrap_or(&[]);

    let mut mappings: Vec<(&'static str, String)> = Vec::new();
    for &(framework, item) in objective.iter().chain(seed.frameworks) {
        let restatements = match framework {
            NIST_SP_800_53_R5 => vec![(FEDRAMP_R5, item.to_string())],
            NIST_SP_800_171_R2 => cmmc_practice(item)
                .map(|practice| vec![(CMMC_LEVEL_2, practice)])
                .unwrap_or_default(),
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
/// issues it: the Security Hub designator of a Security Hub control is `SH.` followed
/// by that control's own Security Hub id, and a control Control Tower publishes is
/// designated by its Control Tower identifier. Both are DERIVED here from the
/// identifier they are a function of, which is what keeps one surface's designator off
/// another surface's control: a control implemented by a Config rule cannot wear a
/// Security Hub control's `SH.…` designator, because the Security Hub control is its
/// own row in [`PUBLISHED_CONTROLS`].
pub(super) fn aliases(seed: &ControlSeed) -> Vec<String> {
    let mut aliases: Vec<String> = seed
        .designators
        .iter()
        .map(|designator| (*designator).to_string())
        .collect();
    if seed.implementation_type == SECURITY_HUB_CONTROL {
        if let Some(identifier) = seed.implementation_identifier {
            aliases.push(format!("{SECURITY_HUB_ALIAS_PREFIX}{identifier}"));
        }
    }
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
