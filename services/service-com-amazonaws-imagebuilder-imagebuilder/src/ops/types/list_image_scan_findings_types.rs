// SPDX-License-Identifier: CC0-1.0

//! Type definitions reached only by the `list_image_scan_findings` operation.
//!
//! This listing's `maxResults` is held to the page-size maximum Image Builder enforces,
//! which is smaller than the published snapshot's `RestrictedInteger` range, so the input
//! is declared from this crate's overlay model instead (see
//! `smithy-model/LOCAL-ADDITIONS.md`).
//!
//! Each structure/union is declared exactly once across this service's `types/`
//! modules, and `types/mod.rs` re-exports them all.
#![allow(unused_imports)]

use super::*;

smithy_macros::smithy_model!("smithy-model/com-amazonaws-imagebuilder-imagebuilder.json");

#[smithy_macros::smithy(
    shape = "com.amazonaws.imagebuilder#ListImageScanFindingsRequest",
    model = "smithy-model/local-additions.json"
)]
#[derive(Debug, Clone, Default)]
pub struct ListImageScanFindingsInput {
    pub filters: Option<Vec<ImageScanFindingsFilter>>,
    pub max_results: Option<i32>,
    pub next_token: Option<String>,
}

#[smithy_macros::smithy(shape = "com.amazonaws.imagebuilder#ImageScanFindingsFilter")]
#[derive(Debug, Clone, Default)]
pub struct ImageScanFindingsFilter {
    pub name: Option<String>,
    pub values: Option<Vec<String>>,
}

#[smithy_macros::smithy(shape = "com.amazonaws.imagebuilder#ListImageScanFindingsResponse")]
#[derive(Debug, Clone, Default)]
pub struct ListImageScanFindingsOutput {
    pub request_id: Option<String>,
    pub findings: Option<Vec<ImageScanFinding>>,
    pub next_token: Option<String>,
}

#[smithy_macros::smithy(shape = "com.amazonaws.imagebuilder#ImageScanFinding")]
#[derive(Debug, Clone, Default)]
pub struct ImageScanFinding {
    pub aws_account_id: Option<String>,
    pub image_build_version_arn: Option<String>,
    pub image_pipeline_arn: Option<String>,
    pub r#type: Option<String>,
    pub description: Option<String>,
    pub title: Option<String>,
    pub remediation: Option<Remediation>,
    pub severity: Option<String>,
    pub first_observed_at: Option<::aws_smithy_types::DateTime>,
    pub updated_at: Option<::aws_smithy_types::DateTime>,
    pub inspector_score: Option<f64>,
    pub inspector_score_details: Option<InspectorScoreDetails>,
    pub package_vulnerability_details: Option<PackageVulnerabilityDetails>,
    pub fix_available: Option<String>,
}

#[smithy_macros::smithy(shape = "com.amazonaws.imagebuilder#Remediation")]
#[derive(Debug, Clone, Default)]
pub struct Remediation {
    pub recommendation: Option<RemediationRecommendation>,
}

#[smithy_macros::smithy(shape = "com.amazonaws.imagebuilder#RemediationRecommendation")]
#[derive(Debug, Clone, Default)]
pub struct RemediationRecommendation {
    pub text: Option<String>,
    pub url: Option<String>,
}

#[smithy_macros::smithy(shape = "com.amazonaws.imagebuilder#InspectorScoreDetails")]
#[derive(Debug, Clone, Default)]
pub struct InspectorScoreDetails {
    pub adjusted_cvss: Option<CvssScoreDetails>,
}

#[smithy_macros::smithy(shape = "com.amazonaws.imagebuilder#CvssScoreDetails")]
#[derive(Debug, Clone, Default)]
pub struct CvssScoreDetails {
    pub score_source: Option<String>,
    pub cvss_source: Option<String>,
    pub version: Option<String>,
    pub score: Option<f64>,
    pub scoring_vector: Option<String>,
    pub adjustments: Option<Vec<CvssScoreAdjustment>>,
}

#[smithy_macros::smithy(shape = "com.amazonaws.imagebuilder#CvssScoreAdjustment")]
#[derive(Debug, Clone, Default)]
pub struct CvssScoreAdjustment {
    pub metric: Option<String>,
    pub reason: Option<String>,
}

#[smithy_macros::smithy(shape = "com.amazonaws.imagebuilder#PackageVulnerabilityDetails")]
#[derive(Debug, Clone, Default)]
pub struct PackageVulnerabilityDetails {
    pub vulnerability_id: String,
    pub vulnerable_packages: Option<Vec<VulnerablePackage>>,
    pub source: Option<String>,
    pub cvss: Option<Vec<CvssScore>>,
    pub related_vulnerabilities: Option<Vec<String>>,
    pub source_url: Option<String>,
    pub vendor_severity: Option<String>,
    pub vendor_created_at: Option<::aws_smithy_types::DateTime>,
    pub vendor_updated_at: Option<::aws_smithy_types::DateTime>,
    pub reference_urls: Option<Vec<String>>,
}

#[smithy_macros::smithy(shape = "com.amazonaws.imagebuilder#VulnerablePackage")]
#[derive(Debug, Clone, Default)]
pub struct VulnerablePackage {
    pub name: Option<String>,
    pub version: Option<String>,
    pub source_layer_hash: Option<String>,
    pub epoch: Option<i32>,
    pub release: Option<String>,
    pub arch: Option<String>,
    pub package_manager: Option<String>,
    pub file_path: Option<String>,
    pub fixed_in_version: Option<String>,
    pub remediation: Option<String>,
}

#[smithy_macros::smithy(shape = "com.amazonaws.imagebuilder#CvssScore")]
#[derive(Debug, Clone, Default)]
pub struct CvssScore {
    pub base_score: Option<f64>,
    pub scoring_vector: Option<String>,
    pub version: Option<String>,
    pub source: Option<String>,
}

