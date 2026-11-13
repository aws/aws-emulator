// SPDX-License-Identifier: CC0-1.0

//! The identity and metadata a published control is served with.
//!
//! The rules under test, read off the modeled `ControlSummary` / `GetControlResponse`
//! (`smithy_query(query="operation", operation="ListControls,GetControl")`) and the
//! documentation of their members:
//!
//! 1. The catalog is an Amazon Web Services-published, account-independent read-only
//!    dataset — its ARNs carry neither account nor Region — so every control is served
//!    with the catalog's OWN published identity and metadata: the opaque identifier its
//!    published ARN carries, the designators it is published under, its name, its
//!    severity and the resource types it governs ("If GovernedResources cannot be
//!    represented by available resource types, it's returned as an empty list").
//! 2. `Implementation` does not identify a control: the catalog publishes a SEPARATE
//!    control for every surface and scope it governs, so two controls may report one
//!    implementation type and identifier — an account-wide control and a log-archive
//!    one over the same AWS Config managed rule — each under its own ARN and its own
//!    designator, and neither may be dropped or merged into the other.
//! 3. `ParameterRequirementSummary` summarises the parameters a caller supplies when it
//!    enables the control, which are the parameters of the surface implementing it: a
//!    control over an AWS Config managed rule that takes input parameters accepts them
//!    (`OPTIONAL`), one over a rule that takes none supports none (`NONE`), and a
//!    control deployed as a policy or a hook summarises its exemption parameters.
//!
//! Each test is a TABLE spanning the class rather than one control picked by hand: the
//! rows differ in designator family (`CONFIG.…`, `AWS-GR_…`, `CT.…`, `SH.…`),
//! implementation surface, severity, parameter summary, governed-resource shape and
//! whether the implementation is shared with another control.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;

use crate::control_catalog::{catalog_arn, drain, Emulator};

/// One published control, as the catalog publishes it.
///
/// Source: the Control Catalog's own published control list — each control's ARN
/// (`arn:{PARTITION}:controlcatalog:::control/{identifier}`), the designators it is
/// published under, its name, its severity and its governed resource types — together
/// with the input parameters of the AWS Config managed rule named in `implementation`,
/// which is what its `ParameterRequirementSummary` summarises.
struct PublishedControl {
    /// A designator the control is published under, which identifies it catalog-wide.
    alias: &'static str,
    /// The opaque identifier the control's published catalog ARN carries.
    identifier: &'static str,
    name: &'static str,
    severity: &'static str,
    parameter_requirement_summary: &'static str,
    governed_resources: &'static [&'static str],
    /// `(ImplementationType, ImplementationIdentifier)`.
    implementation: (&'static str, &'static str),
}

/// The AWS Config implementation type, as the catalog spells it.
const CONFIG_RULE: &str = "AWS::Config::ConfigRule";

/// The Security Hub implementation type, as the catalog spells it.
const SECURITY_HUB_CONTROL: &str = "AWS::SecurityHub::SecurityControl";

/// Controls whose published catalog identity the catalog records, spanning the
/// designator families the catalog publishes controls under (`CONFIG.…`, `AWS-GR_…` and
/// `SH.…`), both implementation surfaces behind them, every severity the rows carry,
/// both parameter summaries an AWS Config-implemented control can have, an empty and a
/// non-empty governed-resource list, and both members of each pair of controls that
/// share one Config rule.
const PUBLISHED_CONTROLS: &[PublishedControl] = &[
    PublishedControl {
        alias: "CONFIG.CLOUDTRAIL.DT.10",
        identifier: "d5kdbugbadk3r481vtwnvoynv",
        name: "Checks that there is at least one multi-region AWS CloudTrail",
        severity: "HIGH",
        parameter_requirement_summary: "OPTIONAL",
        governed_resources: &["AWS::CloudTrail::Trail"],
        implementation: (CONFIG_RULE, "MULTI_REGION_CLOUD_TRAIL_ENABLED"),
    },
    PublishedControl {
        alias: "CONFIG.S3.DT.8",
        identifier: "d4wgeffz6izrb627c2yb8nq8d",
        name: "Checks if S3 buckets have default encryption enabled or have bucket \
               policies that explicitly deny put-object requests without server side \
               encryption using AES-256 or AWS KMS",
        severity: "MEDIUM",
        parameter_requirement_summary: "NONE",
        governed_resources: &["AWS::S3::Bucket"],
        implementation: (CONFIG_RULE, "S3_BUCKET_SERVER_SIDE_ENCRYPTION_ENABLED"),
    },
    PublishedControl {
        alias: "CONFIG.S3.DT.15",
        identifier: "e5efjudq0b347dgxn86o8fg2o",
        name: "Checks if S3 buckets have policies that require requests to use SSL/TLS",
        severity: "MEDIUM",
        parameter_requirement_summary: "NONE",
        governed_resources: &["AWS::S3::Bucket"],
        implementation: (CONFIG_RULE, "S3_BUCKET_SSL_REQUESTS_ONLY"),
    },
    PublishedControl {
        alias: "CONFIG.EC2.DT.48",
        identifier: "22484xhzccf9twis5gmdt7uic",
        name: "Checks if Amazon Virtual Private Cloud (Amazon VPC) flow logs are found \
               and enabled for all Amazon VPCs",
        severity: "MEDIUM",
        parameter_requirement_summary: "OPTIONAL",
        governed_resources: &["AWS::EC2::VPC"],
        implementation: (CONFIG_RULE, "VPC_FLOW_LOGS_ENABLED"),
    },
    PublishedControl {
        alias: "AWS-GR_S3_BUCKET_PUBLIC_READ_PROHIBITED",
        identifier: "8sw3pbid15t9cbww8d2w2qwgf",
        name: "Disallow public read access to S3 buckets",
        severity: "CRITICAL",
        parameter_requirement_summary: "NONE",
        governed_resources: &["AWS::S3::Bucket"],
        implementation: (CONFIG_RULE, "S3_BUCKET_PUBLIC_READ_PROHIBITED"),
    },
    PublishedControl {
        alias: "AWS-GR_AUDIT_BUCKET_PUBLIC_READ_PROHIBITED",
        identifier: "9jzlkwys54gzgsx3pmkqnx8l1",
        name: "Disallow public read access to log archive",
        severity: "CRITICAL",
        parameter_requirement_summary: "NONE",
        governed_resources: &["AWS::S3::Bucket"],
        implementation: (CONFIG_RULE, "S3_BUCKET_PUBLIC_READ_PROHIBITED"),
    },
    PublishedControl {
        alias: "AWS-GR_S3_BUCKET_PUBLIC_WRITE_PROHIBITED",
        identifier: "9j9nwxj789d82sypnukhyyowy",
        name: "Disallow public write access to S3 buckets",
        severity: "CRITICAL",
        parameter_requirement_summary: "NONE",
        governed_resources: &["AWS::S3::Bucket"],
        implementation: (CONFIG_RULE, "S3_BUCKET_PUBLIC_WRITE_PROHIBITED"),
    },
    PublishedControl {
        alias: "AWS-GR_AUDIT_BUCKET_PUBLIC_WRITE_PROHIBITED",
        identifier: "ci9ov6vzjoi6vzkajgademvvo",
        name: "Disallow public write access to log archive",
        severity: "CRITICAL",
        parameter_requirement_summary: "NONE",
        governed_resources: &["AWS::S3::Bucket"],
        implementation: (CONFIG_RULE, "S3_BUCKET_PUBLIC_WRITE_PROHIBITED"),
    },
    PublishedControl {
        alias: "AWS-GR_RDS_SNAPSHOTS_PUBLIC_PROHIBITED",
        identifier: "1h4eyqyyonp19dlrreqf1i3w0",
        name: "Disallow public access to RDS database snapshots",
        severity: "CRITICAL",
        parameter_requirement_summary: "NONE",
        // The catalog publishes no available resource type for what this control
        // governs, so the list is served empty.
        governed_resources: &[],
        implementation: (CONFIG_RULE, "RDS_SNAPSHOTS_PUBLIC_PROHIBITED"),
    },
    PublishedControl {
        alias: "AWS-GR_RDS_STORAGE_ENCRYPTED",
        identifier: "e34kieahgkm0lggs5g0s412jt",
        name: "Disallow RDS database instances without storage encryption",
        severity: "MEDIUM",
        parameter_requirement_summary: "OPTIONAL",
        governed_resources: &["AWS::RDS::DBInstance"],
        implementation: (CONFIG_RULE, "RDS_STORAGE_ENCRYPTED"),
    },
    PublishedControl {
        alias: "SH.CloudTrail.1",
        identifier: "435u8u4w5bsf02tkxga5jisbc",
        name: "CloudTrail should be enabled and configured with at least one multi-Region \
               trail that includes read and write management events",
        severity: "HIGH",
        parameter_requirement_summary: "NONE",
        governed_resources: &["AWS::CloudTrail::Trail"],
        implementation: (SECURITY_HUB_CONTROL, "CloudTrail.1"),
    },
    PublishedControl {
        alias: "SH.IAM.4",
        identifier: "ezfflnh8xgshxb78grtt5sjc5",
        name: "IAM root user access key should not exist",
        severity: "CRITICAL",
        parameter_requirement_summary: "NONE",
        // What this control governs — the account's own root user credentials — no
        // available resource type represents, so the catalog publishes the list empty.
        governed_resources: &[],
        implementation: (SECURITY_HUB_CONTROL, "IAM.4"),
    },
    PublishedControl {
        alias: "SH.IAM.5",
        identifier: "6g3gbim493yofkc9eg6vvnahm",
        name: "MFA should be enabled for all IAM users that have a console password",
        severity: "MEDIUM",
        parameter_requirement_summary: "NONE",
        governed_resources: &["AWS::IAM::User"],
        implementation: (SECURITY_HUB_CONTROL, "IAM.5"),
    },
];

/// What a control's parameter summary must be, for controls beyond the identity table
/// above: `(designator, ParameterRequirementSummary)`.
///
/// Source: the input parameters of the surface that implements each control — the named
/// AWS Config managed rule's own input parameters, which it gives defaults for, and the
/// exemption parameters of a control deployed as a policy or a hook.
const PUBLISHED_PARAMETER_SUMMARIES: &[(&str, &str)] = &[
    // ACCOUNT_PART_OF_ORGANIZATIONS takes an input parameter.
    ("CONFIG.ACCOUNT.DT.1", "OPTIONAL"),
    // ENCRYPTED_VOLUMES takes the KMS key to check volumes against.
    ("AWS-GR_ENCRYPTED_VOLUMES", "OPTIONAL"),
    // CLOUD_TRAIL_ENABLED, behind the Control Tower member-account control, takes the
    // trail destination to check against.
    (
        "AWS-GR_DETECT_CLOUDTRAIL_ENABLED_ON_MEMBER_ACCOUNTS",
        "OPTIONAL",
    ),
    // INCOMING_SSH_DISABLED takes no input parameter.
    ("AWS-GR_RESTRICTED_SSH", "NONE"),
    // A Security Hub control is not parameterised through the catalog.
    ("SH.S3.4", "NONE"),
    // A control deployed as a service control policy, exempting principals.
    ("CT.IAM.PV.1", "OPTIONAL"),
];

/// The `Aliases` of a served control summary.
fn aliases(control: &Value) -> Vec<String> {
    control["Aliases"]
        .as_array()
        .map(|list| {
            list.iter()
                .filter_map(|alias| alias.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// Every control the catalog serves, keyed by each designator it is published under.
async fn by_alias(emulator: &Emulator) -> BTreeMap<String, Value> {
    let controls = drain(emulator, "/list-controls", "Controls", None, "{}").await;
    assert!(
        !controls.is_empty(),
        "case catalog: ListControls must publish the catalog's controls"
    );
    let mut indexed: BTreeMap<String, Value> = BTreeMap::new();
    for control in controls {
        for alias in aliases(&control) {
            assert!(
                indexed.insert(alias.clone(), control.clone()).is_none(),
                "case alias {alias}: expected one control to carry it, got a second: \
                 {control:?}"
            );
        }
    }
    indexed
}

/// The list of strings a summary member carries.
fn strings(control: &Value, member: &str) -> Vec<String> {
    control[member]
        .as_array()
        .map(|list| {
            list.iter()
                .filter_map(|value| value.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// Every published control is served under its own catalog ARN, with the catalog's own
/// metadata, and reads back the same way through `GetControl`.
#[tokio::test]
async fn every_published_control_is_served_with_the_catalogs_own_identity() {
    let emulator = Emulator::new();
    let indexed = by_alias(&emulator).await;

    for row in PUBLISHED_CONTROLS {
        let case = row.alias;
        let summary = indexed
            .get(row.alias)
            .unwrap_or_else(|| panic!("case {case}: the catalog must publish this control"));
        let expected_arn = catalog_arn(row.identifier);

        assert_eq!(
            summary["Arn"].as_str(),
            Some(expected_arn.as_str()),
            "case {case}: expected the catalog's ARN {expected_arn}, got {:?}",
            summary["Arn"]
        );
        assert_eq!(
            summary["Name"].as_str(),
            Some(row.name),
            "case {case}: expected the catalog's name, got {:?}",
            summary["Name"]
        );
        assert_eq!(
            summary["Severity"].as_str(),
            Some(row.severity),
            "case {case}: expected severity {}, got {:?}",
            row.severity,
            summary["Severity"]
        );
        assert_eq!(
            summary["ParameterRequirementSummary"].as_str(),
            Some(row.parameter_requirement_summary),
            "case {case}: expected ParameterRequirementSummary {}, got {:?}",
            row.parameter_requirement_summary,
            summary["ParameterRequirementSummary"]
        );
        assert_eq!(
            strings(summary, "GovernedResources"),
            row.governed_resources,
            "case {case}: expected the governed resource types the catalog publishes, \
             got {:?}",
            summary["GovernedResources"]
        );
        let (implementation_type, implementation_identifier) = row.implementation;
        assert_eq!(
            (
                summary["Implementation"]["Type"].as_str(),
                summary["Implementation"]["Identifier"].as_str(),
            ),
            (Some(implementation_type), Some(implementation_identifier)),
            "case {case}: expected the implementing surface \
             {implementation_type}/{implementation_identifier}, got {:?}",
            summary["Implementation"]
        );
        // A client that harvests the ARN reads the same control back by it.
        let read_back = emulator
            .ok(
                "/get-control",
                &format!(r#"{{"ControlArn":"{expected_arn}"}}"#),
            )
            .await;
        assert_eq!(
            read_back["Arn"].as_str(),
            Some(expected_arn.as_str()),
            "case {case}: GetControl must resolve the catalog's ARN, got {read_back:?}"
        );
        assert_eq!(
            read_back["Name"].as_str(),
            summary["Name"].as_str(),
            "case {case}: GetControl and ListControls must report one name, got \
             {read_back:?}"
        );
        assert!(
            aliases(&read_back).contains(&row.alias.to_string()),
            "case {case}: GetControl must report the designator the control is published \
             under, got {:?}",
            aliases(&read_back)
        );
    }
}

/// A shared implementation publishes every control over it: both controls are served,
/// each under its own ARN and designator, and the `Identifiers` filter answers with all
/// of them whatever the page size.
#[tokio::test]
async fn a_shared_implementation_publishes_each_control_over_it() {
    let emulator = Emulator::new();
    let indexed = by_alias(&emulator).await;

    // The implementations this table publishes more than one control over, each with
    // the designators of the controls that share it.
    let mut shared: BTreeMap<&str, Vec<&PublishedControl>> = BTreeMap::new();
    for row in PUBLISHED_CONTROLS {
        shared.entry(row.implementation.1).or_default().push(row);
    }
    let shared: Vec<(&str, Vec<&PublishedControl>)> = shared
        .into_iter()
        .filter(|(_, rows)| rows.len() > 1)
        .collect();
    assert!(
        shared.len() >= 2,
        "case catalog: the table must span more than one shared implementation for the \
         rule to be tested, got {}",
        shared.len()
    );

    for (identifier, rows) in shared {
        let expected_arns: BTreeSet<String> = rows
            .iter()
            .map(|row| catalog_arn(row.identifier))
            .collect();
        assert_eq!(
            expected_arns.len(),
            rows.len(),
            "case identifier {identifier}: each control over one implementation has its \
             own ARN, got {expected_arns:?}"
        );

        let body = format!(r#"{{"Filter":{{"Implementations":{{"Identifiers":["{identifier}"]}}}}}}"#);
        // Page size 1 as well, so a shared implementation is walked across pages.
        for page_size in [None, Some(1u32)] {
            let filtered = drain(&emulator, "/list-controls", "Controls", page_size, &body).await;
            let served: BTreeSet<String> = filtered
                .iter()
                .filter_map(|control| control["Arn"].as_str().map(str::to_string))
                .collect();
            assert_eq!(
                served, expected_arns,
                "case identifier {identifier} maxResults={page_size:?}: expected every \
                 control published over this implementation, got {served:?}"
            );
        }

        // Each side keeps its own designator and its own name, so a client can tell the
        // account-wide control from the log-archive one.
        let mut names: BTreeSet<&str> = BTreeSet::new();
        for row in &rows {
            let summary = indexed.get(row.alias).unwrap_or_else(|| {
                panic!("case identifier {identifier}: {} must be published", row.alias)
            });
            assert_eq!(
                summary["Arn"].as_str(),
                Some(catalog_arn(row.identifier).as_str()),
                "case {}: expected its own catalog ARN, got {:?}",
                row.alias,
                summary["Arn"]
            );
            assert!(
                names.insert(row.name),
                "case identifier {identifier}: the controls over it must be named \
                 distinctly, got {} twice",
                row.name
            );
        }
    }
}

/// The parameter summary follows the surface that implements the control, across the
/// designator families and implementation surfaces the catalog publishes.
#[tokio::test]
async fn the_parameter_summary_follows_the_implementing_surface() {
    let emulator = Emulator::new();
    let indexed = by_alias(&emulator).await;
    let mut summaries_seen: BTreeSet<&str> = BTreeSet::new();

    let rows = PUBLISHED_PARAMETER_SUMMARIES.iter().copied().chain(
        PUBLISHED_CONTROLS
            .iter()
            .map(|row| (row.alias, row.parameter_requirement_summary)),
    );
    for (alias, expected) in rows {
        let summary = indexed
            .get(alias)
            .unwrap_or_else(|| panic!("case {alias}: the catalog must publish this control"));
        assert_eq!(
            summary["ParameterRequirementSummary"].as_str(),
            Some(expected),
            "case {alias}: expected ParameterRequirementSummary {expected}, got {:?}",
            summary["ParameterRequirementSummary"]
        );
        summaries_seen.insert(expected);
    }

    for value in ["NONE", "OPTIONAL"] {
        assert!(
            summaries_seen.contains(value),
            "case {value}: the table must span this summary for the rule to be tested, \
             saw {summaries_seen:?}"
        );
    }
}

/// Every control the catalog serves — not only the rows above — is served under a
/// well-formed catalog ARN of its own.
#[tokio::test]
async fn every_served_control_carries_its_own_catalog_identifier() {
    let emulator = Emulator::new();
    let controls = drain(&emulator, "/list-controls", "Controls", None, "{}").await;
    assert!(
        !controls.is_empty(),
        "case catalog: ListControls must publish the catalog's controls"
    );

    // The global `controlcatalog` form the modeled `ControlArn` pattern admits, which
    // carries neither account nor Region: `arn:{PARTITION}:controlcatalog:::control/
    // {CONTROL_CATALOG_OPAQUE_ID}` (`smithy_query(query="shape", shape="ControlArn")`).
    const PREFIX: &str = "arn:aws:controlcatalog:::control/";
    /// The length of the opaque identifier the catalog's published ARNs carry.
    const IDENTIFIER_LEN: usize = 25;

    let mut identifiers: BTreeSet<String> = BTreeSet::new();
    for control in &controls {
        let arn = control["Arn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {control:?}: a control carries its ARN"));
        let identifier = arn
            .strip_prefix(PREFIX)
            .unwrap_or_else(|| panic!("case {arn}: expected the global form {PREFIX}…"));
        assert_eq!(
            identifier.len(),
            IDENTIFIER_LEN,
            "case {arn}: expected a {IDENTIFIER_LEN}-character catalog identifier, got \
             {identifier}"
        );
        assert!(
            identifier
                .chars()
                .all(|character| character.is_ascii_lowercase() || character.is_ascii_digit()),
            "case {arn}: a catalog identifier is lower-case alphanumeric, got {identifier}"
        );
        assert!(
            identifiers.insert(identifier.to_string()),
            "case {arn}: two controls must not share one catalog identifier"
        );
    }
}
