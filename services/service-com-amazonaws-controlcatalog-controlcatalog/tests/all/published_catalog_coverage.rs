// SPDX-License-Identifier: CC0-1.0

//! What the catalog has to hold for a client to read a control it discovered elsewhere.
//!
//! The rule under test: the Control Catalog is an Amazon Web Services-published,
//! account- and Region-independent read-only dataset (its ARNs carry neither account nor
//! Region), so a control the catalog publishes is served under the identity the catalog
//! publishes for it — one record per published control, reachable BOTH by its published
//! ARN through `GetControl` and in every filtered `ListControls` page that selects it,
//! with the page sequence walking the WHOLE filtered set whatever page size drives it.
//! `GetControl` "[r]eturns details about a specific control" and `ListControls`
//! "[r]eturns a paginated list of all available controls in the Control Catalog library"
//! (`smithy_query(query="operation", operation="GetControl,ListControls",
//! model_path="services/service-com-amazonaws-controlcatalog-controlcatalog/smithy-model/com-amazonaws-controlcatalog-controlcatalog.json")`).
//! The refusal the other side of the rule — an ARN the catalog publishes nothing for —
//! is pinned by `all/refusal_wording.rs`, and the member-for-member projection of a
//! served control by `all/control_catalog.rs`; this suite is about what the catalog must
//! HOLD and how far a filtered walk reaches.
//!
//! Each test is a TABLE rather than one control picked by hand: the implementation table
//! below spans both implementing surfaces the catalog publishes controls over and the
//! services they govern, the walk test takes EVERY control the service serves, and the
//! page sizes span one control per page, a partial page and the modeled maximum of
//! `MaxListControlsResults` (`@range 1..100`).

use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;

use crate::control_catalog::{by_arn, drain, strings, Emulator};

/// The AWS Config implementation type, as the catalog spells it.
const CONFIG_RULE: &str = "AWS::Config::ConfigRule";

/// The Security Hub implementation type, as the catalog spells it.
const SECURITY_HUB_CONTROL: &str = "AWS::SecurityHub::SecurityControl";

/// Implementations the catalog publishes a control over: `(ImplementationType,
/// ImplementationIdentifier)`.
///
/// Source: the implementing surfaces' own published identifiers — an AWS Config managed
/// rule identifier and a Security Hub control id — each of which the catalog publishes a
/// control for. The rows span both surfaces and the services they govern (certificate
/// authorities, user pools, file systems, tables, data warehouses, functions, load
/// balancers, security groups, identities and keys), and they are deliberately NOT the
/// implementations any one recorded page named.
const PUBLISHED_IMPLEMENTATIONS: &[(&str, &str)] = &[
    (CONFIG_RULE, "ACMPCA_CERTIFICATE_AUTHORITY_TAGGED"),
    (CONFIG_RULE, "COGNITO_USER_POOL_ADVANCED_SECURITY_ENABLED"),
    (CONFIG_RULE, "EC2_INSTANCE_DETAILED_MONITORING_ENABLED"),
    (CONFIG_RULE, "EFS_ENCRYPTED_CHECK"),
    (CONFIG_RULE, "DYNAMODB_TABLE_ENCRYPTED_KMS"),
    (CONFIG_RULE, "SNS_ENCRYPTED_KMS"),
    (CONFIG_RULE, "OPENSEARCH_ENCRYPTED_AT_REST"),
    (CONFIG_RULE, "REDSHIFT_CLUSTER_CONFIGURATION_CHECK"),
    (CONFIG_RULE, "REDSHIFT_REQUIRE_TLS_SSL"),
    (CONFIG_RULE, "ELBV2_ACM_CERTIFICATE_REQUIRED"),
    (CONFIG_RULE, "S3_BUCKET_LOGGING_ENABLED"),
    (CONFIG_RULE, "API_GW_EXECUTION_LOGGING_ENABLED"),
    (CONFIG_RULE, "LAMBDA_FUNCTION_PUBLIC_ACCESS_PROHIBITED"),
    (CONFIG_RULE, "EBS_SNAPSHOT_PUBLIC_RESTORABLE_CHECK"),
    (CONFIG_RULE, "IAM_USER_UNUSED_CREDENTIALS_CHECK"),
    (CONFIG_RULE, "RESTRICTED_INCOMING_TRAFFIC"),
    (CONFIG_RULE, "CMK_BACKING_KEY_ROTATION_ENABLED"),
    (CONFIG_RULE, "ROOT_ACCOUNT_HARDWARE_MFA_ENABLED"),
    (SECURITY_HUB_CONTROL, "S3.1"),
    (SECURITY_HUB_CONTROL, "EC2.2"),
    (SECURITY_HUB_CONTROL, "RDS.2"),
    (SECURITY_HUB_CONTROL, "CloudTrail.4"),
    (SECURITY_HUB_CONTROL, "KMS.4"),
    (SECURITY_HUB_CONTROL, "GuardDuty.1"),
];

/// Page sizes the walks below are driven with: the service's own default, one control per
/// page, a partial page, and the modeled maximum of `MaxListControlsResults`
/// (`@range 1..100`).
const PAGE_SIZES: [Option<u32>; 4] = [None, Some(1), Some(7), Some(100)];

/// Every control the catalog publishes, keyed by its ARN.
async fn catalog(emulator: &Emulator) -> BTreeMap<String, Value> {
    let controls = drain(emulator, "/list-controls", "Controls", None, "{}").await;
    assert!(
        !controls.is_empty(),
        "case catalog: ListControls must publish the catalog's controls"
    );
    controls
        .into_iter()
        .map(|control| {
            let arn = control["Arn"]
                .as_str()
                .unwrap_or_else(|| panic!("case {control:?}: a control carries its ARN"))
                .to_string();
            (arn, control)
        })
        .collect()
}

/// The request body that narrows `ListControls` to one implementation identifier.
fn identifier_filter(identifier: &str) -> String {
    format!(r#"{{"Filter":{{"Implementations":{{"Identifiers":["{identifier}"]}}}}}}"#)
}

/// The request body that narrows `ListControls` to one implementation type.
fn type_filter(implementation_type: &str) -> String {
    format!(r#"{{"Filter":{{"Implementations":{{"Types":["{implementation_type}"]}}}}}}"#)
}

/// Every implementation the catalog records a published control for is served as one: the
/// `Identifiers` filter selects it at every page size, and each control it selects reads
/// back by its own published ARN.
#[tokio::test]
async fn every_published_implementation_is_served_as_a_catalog_control() {
    let emulator = Emulator::new();
    let published = catalog(&emulator).await;

    for (implementation_type, identifier) in PUBLISHED_IMPLEMENTATIONS {
        let body = identifier_filter(identifier);
        let mut per_page_size: Vec<(Option<u32>, Vec<Value>)> = Vec::new();

        for page_size in PAGE_SIZES {
            let case = format!("{identifier} maxResults={page_size:?}");
            let selected = drain(&emulator, "/list-controls", "Controls", page_size, &body).await;
            assert!(
                !selected.is_empty(),
                "case {case}: the catalog must publish a control over this implementation"
            );

            for control in &selected {
                let arn = control["Arn"]
                    .as_str()
                    .unwrap_or_else(|| panic!("case {case}: a control carries its ARN"));
                assert_eq!(
                    (
                        control["Implementation"]["Type"].as_str(),
                        control["Implementation"]["Identifier"].as_str(),
                    ),
                    (Some(*implementation_type), Some(*identifier)),
                    "case {case}: expected the implementing surface \
                     {implementation_type}/{identifier}, got {:?}",
                    control["Implementation"]
                );
                assert!(
                    published.contains_key(arn),
                    "case {case}: the unfiltered listing must publish {arn} too"
                );

                // A client that harvested the ARN reads the control back by it.
                let read_back = emulator
                    .ok("/get-control", &format!(r#"{{"ControlArn":"{arn}"}}"#))
                    .await;
                assert_eq!(
                    read_back["Arn"].as_str(),
                    Some(arn),
                    "case {case}: GetControl must resolve the published ARN, got {read_back:?}"
                );
                assert_eq!(
                    read_back["Name"].as_str(),
                    control["Name"].as_str(),
                    "case {case}: GetControl and ListControls must report one name, got \
                     {read_back:?}"
                );
            }
            per_page_size.push((page_size, by_arn(selected)));
        }

        // The page size decides how the filtered set is walked, never how much of it the
        // walk reaches.
        let (first_size, first) = &per_page_size[0];
        for (page_size, selected) in &per_page_size[1..] {
            assert_eq!(
                selected, first,
                "case {identifier} maxResults={page_size:?}: expected the same controls \
                 the walk at maxResults={first_size:?} reached, got {selected:?}"
            );
        }
    }
}

/// Every filtered walk reaches the whole filtered set: for each implementation type the
/// catalog serves, the walk returns exactly the controls of that type the unfiltered
/// catalog holds, at every page size — no page is short and no walk stops early.
#[tokio::test]
async fn a_filtered_walk_reaches_every_control_the_filter_selects() {
    let emulator = Emulator::new();
    let published = catalog(&emulator).await;

    let mut types: BTreeSet<String> = BTreeSet::new();
    for control in published.values() {
        if let Some(implementation_type) = control["Implementation"]["Type"].as_str() {
            types.insert(implementation_type.to_string());
        }
    }
    assert!(
        types.len() >= 2,
        "case catalog: the catalog must publish more than one implementation type for a \
         type filter to narrow anything, got {types:?}"
    );

    for implementation_type in &types {
        let expected = by_arn(
            published
                .values()
                .filter(|control| {
                    control["Implementation"]["Type"].as_str() == Some(implementation_type.as_str())
                })
                .cloned()
                .collect(),
        );
        let body = type_filter(implementation_type);
        for page_size in PAGE_SIZES {
            let selected =
                by_arn(drain(&emulator, "/list-controls", "Controls", page_size, &body).await);
            assert_eq!(
                selected.len(),
                expected.len(),
                "case type {implementation_type} maxResults={page_size:?}: expected the \
                 {} control(s) the catalog publishes over this surface, got {}",
                expected.len(),
                selected.len()
            );
            assert_eq!(
                selected, expected,
                "case type {implementation_type} maxResults={page_size:?}: expected the \
                 catalog's own entries for this surface"
            );
        }
    }

    // The walk is not asserted vacuously over a handful of entries: the catalog's
    // Config-implemented surface alone publishes a control for every implementation the
    // table above records for it, and more besides.
    let config_rules = PUBLISHED_IMPLEMENTATIONS
        .iter()
        .filter(|(implementation_type, _)| *implementation_type == CONFIG_RULE)
        .count();
    let served = published
        .values()
        .filter(|control| control["Implementation"]["Type"].as_str() == Some(CONFIG_RULE))
        .count();
    assert!(
        served >= config_rules,
        "case type {CONFIG_RULE}: the catalog must publish at least the {config_rules} \
         recorded Config-implemented control(s), got {served}"
    );
}

/// One control the catalog publishes over an AWS Config managed rule, as the catalog
/// publishes it.
///
/// Source: the Control Catalog's own published control list — the opaque identifier its
/// ARN `arn:{PARTITION}:controlcatalog:::control/{identifier}` carries, the `CONFIG.…`
/// designators it is published under, the resource types it governs and the instant it
/// records as the control's release time — together with the implementing AWS Config
/// managed rule's own published title and description, which is the text the catalog
/// publishes for such a control.
struct PublishedConfigControl {
    /// The implementing rule's identifier, which is what an `Implementations.Identifiers`
    /// filter names.
    rule: &'static str,
    identifier: &'static str,
    designators: &'static [&'static str],
    release_time: i64,
    governed_resources: &'static [&'static str],
    parameter_requirement_summary: &'static str,
    /// The catalog's published description, where this row pins the whole sentence;
    /// `None` leaves it to the first-sentence rule every row is held to.
    description: Option<&'static str>,
}

/// Controls the catalog publishes over an AWS Config managed rule, spanning what the rule
/// below generalises over: the designator family and number (`CONFIG.ACM.…`,
/// `CONFIG.ACM-PCA.…`, `CONFIG.COGNITO-IDP.…`, `CONFIG.ELASTICLOADBALANCING.…`,
/// `CONFIG.REDSHIFT.…`, `CONFIG.S3.…`, `CONFIG.SNS.…`, `CONFIG.EC2.…`,
/// `CONFIG.CLOUDTRAIL.…`, `CONFIG.RDS.…`), the length of the opaque identifier (24 and 25
/// characters), release instants from 2016 to 2026 including two controls released
/// together on one instant, governed-resource lists of none, one and two types, both
/// parameter summaries a Config-implemented control can carry, and a description that
/// names optional parameters against one that does not.
///
/// The last two rows are controls no page this suite was written for named, so the rule is
/// asserted beyond the cited ones.
const PUBLISHED_CONFIG_CONTROLS: &[PublishedConfigControl] = &[
    PublishedConfigControl {
        rule: "ACM_CERTIFICATE_EXPIRATION_CHECK",
        identifier: "a6kwsgw65ayxiqg7dhzbz2n7w",
        designators: &["CONFIG.ACM.DT.3"],
        release_time: 1_779_840_000,
        governed_resources: &["AWS::CertificateManager::Certificate"],
        parameter_requirement_summary: "OPTIONAL",
        description: None,
    },
    PublishedConfigControl {
        rule: "ACM_CERTIFICATE_RSA_CHECK",
        identifier: "1ysrvaplv6a00p99mim170vqy",
        designators: &["CONFIG.ACM.DT.1"],
        release_time: 1_678_060_800,
        governed_resources: &["AWS::CertificateManager::Certificate"],
        parameter_requirement_summary: "NONE",
        description: Some(
            "Checks if RSA certificates managed by AWS Certificate Manager (ACM) have a \
             key length of at least '2048' bits. The rule is NON_COMPLIANT if the minimum \
             key length is less than 2048 bits.",
        ),
    },
    PublishedConfigControl {
        rule: "ACMPCA_CERTIFICATE_AUTHORITY_TAGGED",
        identifier: "6yl9xxuky13vzaxi3n6lm73k5",
        designators: &["CONFIG.ACM-PCA.DT.2"],
        release_time: 1_779_840_000,
        governed_resources: &["AWS::ACMPCA::CertificateAuthority"],
        parameter_requirement_summary: "OPTIONAL",
        description: Some(
            "Checks if AWS Private CA certificate authorities have tags. Optionally, you \
             can specify tag keys. The rule is NON_COMPLIANT if there are no tags or if \
             the specified tag keys are not present. The rule does not check for tags \
             starting with 'aws:'.",
        ),
    },
    PublishedConfigControl {
        rule: "COGNITO_USER_POOL_ADVANCED_SECURITY_ENABLED",
        identifier: "1038h7d21e6g8i10g6v3xsssi",
        designators: &["CONFIG.COGNITO-IDP.DT.1"],
        release_time: 1_715_212_800,
        governed_resources: &["AWS::Cognito::UserPool"],
        parameter_requirement_summary: "OPTIONAL",
        description: Some(
            "Checks if an Amazon Cognito user pool has advanced security enabled. The \
             rule is NON_COMPLIANT if advanced security is not enabled. Optionally, you \
             can specify an advanced security mode for the rule to check.",
        ),
    },
    PublishedConfigControl {
        rule: "ELB_LOGGING_ENABLED",
        identifier: "8abhdcvu10syqrdyfgr2300px",
        designators: &["CONFIG.ELASTICLOADBALANCING.DT.12"],
        release_time: 1_534_896_000,
        governed_resources: &[
            "AWS::ElasticLoadBalancing::LoadBalancer",
            "AWS::ElasticLoadBalancingV2::LoadBalancer",
        ],
        parameter_requirement_summary: "OPTIONAL",
        description: None,
    },
    PublishedConfigControl {
        rule: "ELB_TLS_HTTPS_LISTENERS_ONLY",
        identifier: "7laz29268m0wkplaj7z2fxxl",
        designators: &["CONFIG.ELASTICLOADBALANCING.DT.16"],
        release_time: 1_594_166_400,
        governed_resources: &["AWS::ElasticLoadBalancing::LoadBalancer"],
        parameter_requirement_summary: "NONE",
        description: None,
    },
    PublishedConfigControl {
        rule: "REDSHIFT_REQUIRE_TLS_SSL",
        identifier: "dfbp3c8jerraflh2g68427p4k",
        designators: &["CONFIG.REDSHIFT.DT.9"],
        release_time: 1_590_364_800,
        governed_resources: &[
            "AWS::Redshift::Cluster",
            "AWS::Redshift::ClusterParameterGroup",
        ],
        parameter_requirement_summary: "NONE",
        description: None,
    },
    PublishedConfigControl {
        rule: "S3_BUCKET_LOGGING_ENABLED",
        identifier: "bd9d97ndoue5lss1zjjiecle4",
        designators: &["CONFIG.S3.DT.14"],
        release_time: 1_475_280_000,
        governed_resources: &["AWS::S3::Bucket"],
        parameter_requirement_summary: "OPTIONAL",
        description: None,
    },
    PublishedConfigControl {
        rule: "SNS_ENCRYPTED_KMS",
        identifier: "bn8ygh3zsy33drdeojmu09a4z",
        designators: &["CONFIG.SNS.DT.2"],
        release_time: 1_590_364_800,
        governed_resources: &["AWS::SNS::Topic"],
        parameter_requirement_summary: "OPTIONAL",
        description: None,
    },
    PublishedConfigControl {
        rule: "VPC_DEFAULT_SECURITY_GROUP_CLOSED",
        identifier: "3ucly6t3j33t5zhbzy45l619n",
        designators: &["CONFIG.EC2.DT.49"],
        release_time: 1_534_291_200,
        governed_resources: &["AWS::EC2::SecurityGroup"],
        parameter_requirement_summary: "NONE",
        description: None,
    },
    PublishedConfigControl {
        rule: "MULTI_REGION_CLOUD_TRAIL_ENABLED",
        identifier: "d5kdbugbadk3r481vtwnvoynv",
        designators: &["CONFIG.CLOUDTRAIL.DT.10"],
        release_time: 1_539_993_600,
        governed_resources: &["AWS::CloudTrail::Trail"],
        parameter_requirement_summary: "OPTIONAL",
        description: None,
    },
    PublishedConfigControl {
        rule: "RDS_SNAPSHOT_ENCRYPTED",
        identifier: "84hd48gyrb6e5ribkmsoub44c",
        designators: &["CONFIG.RDS.DT.39"],
        release_time: 1_590_364_800,
        // "If GovernedResources cannot be represented by available resource types, it's
        // returned as an empty list."
        governed_resources: &[],
        parameter_requirement_summary: "NONE",
        description: None,
    },
];

/// A page is the catalog's own records: the control an `Implementations.Identifiers`
/// filter selects is served under the catalog's published ARN identifier, with the
/// designators, release instant and governed resource types the catalog publishes and the
/// implementing rule's own published description — nothing minted and nothing re-worded.
#[tokio::test]
async fn a_filtered_page_serves_the_catalogs_own_published_config_records() {
    let emulator = Emulator::new();
    let published = catalog(&emulator).await;
    let mut identifier_lengths: BTreeSet<usize> = BTreeSet::new();
    let mut summaries_seen: BTreeSet<&str> = BTreeSet::new();

    for row in PUBLISHED_CONFIG_CONTROLS {
        let case = row.rule;
        let expected_arn = format!("arn:aws:controlcatalog:::control/{}", row.identifier);
        let selected = drain(
            &emulator,
            "/list-controls",
            "Controls",
            None,
            &identifier_filter(row.rule),
        )
        .await;
        let arns: Vec<&str> = selected
            .iter()
            .filter_map(|control| control["Arn"].as_str())
            .collect();
        assert_eq!(
            arns,
            vec![expected_arn.as_str()],
            "case {case}: expected the catalog's own control {expected_arn}, got {arns:?}"
        );

        let control = &selected[0];
        assert_eq!(
            strings(control, "Aliases"),
            row.designators,
            "case {case}: expected the published designators {:?}, got {:?}",
            row.designators,
            strings(control, "Aliases")
        );
        assert_eq!(
            control["CreateTime"].as_f64(),
            Some(row.release_time as f64),
            "case {case}: expected the published release instant {}, got {:?}",
            row.release_time,
            control["CreateTime"]
        );
        assert_eq!(
            strings(control, "GovernedResources"),
            row.governed_resources,
            "case {case}: expected the published governed resource types {:?}, got {:?}",
            row.governed_resources,
            strings(control, "GovernedResources")
        );
        assert_eq!(
            control["ParameterRequirementSummary"].as_str(),
            Some(row.parameter_requirement_summary),
            "case {case}: expected ParameterRequirementSummary {}, got {:?}",
            row.parameter_requirement_summary,
            control["ParameterRequirementSummary"]
        );

        // The published description is the implementing rule's own text, whose first
        // sentence is the control's published name — a paraphrase of the row would drop
        // or re-word it.
        let name = control["Name"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case}: a control carries its name"));
        let description = control["Description"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case}: a control carries its description"));
        assert!(
            description.starts_with(name) && description.len() > name.len(),
            "case {case}: expected a description opening with the published name {name:?}, \
             got {description:?}"
        );
        if let Some(expected) = row.description {
            assert_eq!(
                description, expected,
                "case {case}: expected the catalog's published description {expected:?}, \
                 got {description:?}"
            );
        }

        // The same record, read back by the ARN a client harvested from the page.
        let read_back = emulator
            .ok(
                "/get-control",
                &format!(r#"{{"ControlArn":"{expected_arn}"}}"#),
            )
            .await;
        for member in ["Arn", "Name", "Description", "CreateTime"] {
            assert_eq!(
                read_back[member], control[member],
                "case {case}: GetControl and ListControls must report one {member}, got \
                 {:?} and {:?}",
                read_back[member], control[member]
            );
        }
        assert!(
            published.contains_key(&expected_arn),
            "case {case}: the unfiltered listing must publish {expected_arn} too"
        );

        identifier_lengths.insert(row.identifier.chars().count());
        summaries_seen.insert(row.parameter_requirement_summary);
    }

    // The table spans what the rule generalises over rather than one shape of row: the
    // identifier is opaque published content of the catalog's own length, and the
    // parameter summary follows the implementing rule's parameters.
    assert!(
        identifier_lengths.len() > 1,
        "case table: the rows must span more than one published identifier length, saw \
         {identifier_lengths:?}"
    );
    for summary in ["NONE", "OPTIONAL"] {
        assert!(
            summaries_seen.contains(summary),
            "case table: the rows must span the {summary} parameter summary, saw \
             {summaries_seen:?}"
        );
    }
}

/// The published display text of one control the catalog publishes over an AWS Config
/// managed rule.
///
/// Source: the implementing AWS Config managed rule's own published title and description,
/// which is the text the catalog publishes for such a control, with the severity the
/// catalog classifies it at. `name` is the published display name and `description` the
/// published description, each reproduced whole: neither is a summary of the row nor a
/// paraphrase of the implementing service's current documentation prose. The published
/// IDENTITY of these controls — the opaque identifier, the designators, the release
/// instant — is pinned by [`PUBLISHED_CONFIG_CONTROLS`] above rather than re-listed here,
/// so a row is located in the page by the rule that implements it.
struct PublishedConfigText {
    /// The implementing rule's identifier, which is what `Implementation.Identifier`
    /// reports and an `Implementations.Identifiers` filter names.
    rule: &'static str,
    name: &'static str,
    description: &'static str,
    severity: &'static str,
}

/// Published Config-implemented controls, spanning what the rule below generalises over:
/// the two leading clauses the published catalog text uses ("Checks if …" and "Checks
/// whether …"), a description of one sentence against ones of two and three, a description
/// that names the rule's optional parameters against ones that do not, a description that
/// quotes a numeric threshold, and both severities these rows carry.
const PUBLISHED_CONFIG_TEXT: &[PublishedConfigText] = &[
    PublishedConfigText {
        rule: "ACM_CERTIFICATE_RSA_CHECK",
        name: "Checks if RSA certificates managed by AWS Certificate Manager (ACM) have a \
               key length of at least '2048' bits",
        description: "Checks if RSA certificates managed by AWS Certificate Manager (ACM) \
                      have a key length of at least '2048' bits. The rule is NON_COMPLIANT \
                      if the minimum key length is less than 2048 bits.",
        severity: "HIGH",
    },
    PublishedConfigText {
        rule: "ACM_CERTIFICATE_EXPIRATION_CHECK",
        name: "Checks whether ACM Certificates in your account are marked for expiration \
               within the specified number of days",
        description: "Checks whether ACM Certificates in your account are marked for \
                      expiration within the specified number of days. Certificates \
                      provided by ACM are automatically renewed. ACM does not \
                      automatically renew certificates that you import.",
        severity: "MEDIUM",
    },
    PublishedConfigText {
        rule: "ELB_LOGGING_ENABLED",
        name: "Checks whether the Application Load Balancers and the Classic Load \
               Balancers have logging enabled",
        description: "Checks whether the Application Load Balancers and the Classic Load \
                      Balancers have logging enabled.",
        severity: "MEDIUM",
    },
    PublishedConfigText {
        rule: "SNS_ENCRYPTED_KMS",
        name: "Checks if SNS topics are encrypted with AWS Key Management Service (AWS \
               KMS)",
        description: "Checks if SNS topics are encrypted with AWS Key Management Service \
                      (AWS KMS). The rule is NON_COMPLIANT if an SNS topic is not \
                      encrypted with AWS KMS. Optionally, specify the key ARNs, the alias \
                      ARNs, the alias name, or the key IDs for the rule to check.",
        severity: "MEDIUM",
    },
    PublishedConfigText {
        rule: "REDSHIFT_REQUIRE_TLS_SSL",
        name: "Checks if Amazon Redshift clusters require TLS/SSL encryption to connect \
               to SQL clients",
        description: "Checks if Amazon Redshift clusters require TLS/SSL encryption to \
                      connect to SQL clients. The rule is NON_COMPLIANT if any Amazon \
                      Redshift cluster has parameter require_SSL not set to true.",
        severity: "MEDIUM",
    },
    PublishedConfigText {
        rule: "VPC_DEFAULT_SECURITY_GROUP_CLOSED",
        name: "Checks if the default security group of any Amazon Virtual Private Cloud \
               (Amazon VPC) does not allow inbound or outbound traffic",
        description: "Checks if the default security group of any Amazon Virtual Private \
                      Cloud (Amazon VPC) does not allow inbound or outbound traffic. The \
                      rule is NON_COMPLIANT if the default security group has one or more \
                      inbound or outbound traffic rules.",
        severity: "HIGH",
    },
];

/// A control the catalog publishes is CARRIED by the page its implementation type selects,
/// under its published identity and with its published display text: the walk of the
/// `AWS::Config::ConfigRule` page holds every row below, each reporting the published
/// `Name` and `Description` whole — no row is absent from the page and no row's text is
/// re-worded, abridged or extended.
#[tokio::test]
async fn the_config_rule_page_carries_every_published_row_with_its_published_text() {
    let emulator = Emulator::new();
    let page = drain(
        &emulator,
        "/list-controls",
        "Controls",
        None,
        &type_filter(CONFIG_RULE),
    )
    .await;
    let mut leading_clauses: BTreeSet<&str> = BTreeSet::new();
    let mut severities_seen: BTreeSet<&str> = BTreeSet::new();
    let mut sentence_counts: BTreeSet<usize> = BTreeSet::new();

    for row in PUBLISHED_CONFIG_TEXT {
        let case = row.rule;
        // The row is located by the rule that implements it, so this test pins the
        // published TEXT the page carries and leaves the published identity to the table
        // above.
        let carried: Vec<&Value> = page
            .iter()
            .filter(|control| {
                control["Implementation"]["Identifier"].as_str() == Some(row.rule)
            })
            .collect();
        assert_eq!(
            carried.len(),
            1,
            "case {case}: expected the {CONFIG_RULE} page to carry the one control the \
             catalog publishes over this rule, got {carried:?} among its {} control(s)",
            page.len()
        );
        let control = carried[0];

        assert_eq!(
            control["Name"].as_str(),
            Some(row.name),
            "case {case}: expected the published name {:?}, got {:?}",
            row.name,
            control["Name"]
        );
        assert_eq!(
            control["Description"].as_str(),
            Some(row.description),
            "case {case}: expected the published description {:?}, got {:?}",
            row.description,
            control["Description"]
        );
        assert_eq!(
            control["Severity"].as_str(),
            Some(row.severity),
            "case {case}: expected the published severity {}, got {:?}",
            row.severity,
            control["Severity"]
        );

        leading_clauses.insert(if row.name.starts_with("Checks if") {
            "Checks if"
        } else {
            "Checks whether"
        });
        severities_seen.insert(row.severity);
        sentence_counts.insert(row.description.split(". ").count());
    }

    // The rows span what the rule generalises over rather than one shape of published
    // text: both leading clauses, both severities, and descriptions of one, two and three
    // sentences.
    assert_eq!(
        leading_clauses.len(),
        2,
        "case table: the rows must span both published leading clauses, saw \
         {leading_clauses:?}"
    );
    assert_eq!(
        severities_seen.len(),
        2,
        "case table: the rows must span more than one published severity, saw \
         {severities_seen:?}"
    );
    assert!(
        sentence_counts.len() >= 3,
        "case table: the rows must span descriptions of differing sentence counts, saw \
         {sentence_counts:?}"
    );
}
