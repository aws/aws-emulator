// SPDX-License-Identifier: CC0-1.0

//! The project POLICIES attached to a project of this store: what `PutProjectPolicy`
//! records, what `ListProjectPolicies` reports, and the two ways a policy DOCUMENT is
//! refused.
//!
//! A project policy is a resource policy on a project ("A project policy specifies that a
//! trusted AWS account can copy a model version from a trusting AWS account to a project
//! in the trusted AWS account", `PutProjectPolicy`), so the resource a statement of it
//! grants on is a MODEL VERSION of the project the policy is attached to. That is the
//! whole of what makes a document acceptable here, and it is one rule for every operation
//! that takes a document, so it lives in this module once rather than in each handler:
//!
//! * the document has to BE a policy — parseable JSON whose `Statement` is a statement, or
//!   a list of statements, each naming the resources it grants on ([`policy_resources`]);
//! * every resource it names has to resolve to a model version the named project holds
//!   ([`validate_document_for`]).
//!
//! Either refusal is the modelled `MalformedPolicyDocumentException` ("The format of the
//! project policy document that you supplied to `PutProjectPolicy` is incorrect",
//! `smithy_query(query="shape",shape="MalformedPolicyDocumentException")`), with the two
//! classes worded differently on the wire — see
//! [`crate::state::modeled_error::malformed_policy_document`] and
//! [`crate::state::modeled_error::invalid_policy_resource`].
//!
//! Nothing here interprets the policy: no statement of it is ever evaluated, and the
//! document is stored and reported back exactly as it was supplied. What IS computed is
//! which resources it names and whether they exist.

use std::collections::BTreeMap;
use std::time::SystemTime;

use serde_json::Value;

use crate::ops::types::ProjectPolicy;

use super::Project;

/// How many policies one project of this store holds before a further attach is refused
/// with the modelled `LimitExceededException`.
///
/// The same reasoning as [`super::MAX_PROJECTS`]: the store is this process's memory, so a
/// caller repeating `PutProjectPolicy` under fresh names is refused rather than allowed to
/// grow it without limit. This is the EMULATOR's bound, not an AWS quota.
pub const MAX_POLICIES_PER_PROJECT: usize = 100;

/// The largest page `ListProjectPolicies` serves, and the one it serves when the caller
/// sends no `MaxResults`: the top of the modelled `ListProjectPoliciesPageSize` range
/// (`range 1..5`), which that member also documents as the default ("The largest value you
/// can specify is 5. … The default value is 5.",
/// `smithy_query(query="operation",operation="ListProjectPolicies")`).
pub const POLICIES_PAGE_SIZE_MAX: usize = 5;

/// The cursor scope of `ListProjectPolicies`, so its `NextToken` names a position in that
/// list and in no other of this service.
pub const POLICIES_CURSOR_SCOPE: &str = "projectPolicies";

/// The page size a `MaxResults` of `requested` asks `ListProjectPolicies` for, held inside
/// the modelled range — which the boundary validator has already enforced, so this only
/// keeps the page size a page size for a value that reached here anyway.
pub fn policies_page_size(requested: Option<i32>) -> usize {
    requested.map_or(POLICIES_PAGE_SIZE_MAX, |max| {
        usize::try_from(max)
            .unwrap_or(1)
            .clamp(1, POLICIES_PAGE_SIZE_MAX)
    })
}

/// One project policy, as the `PutProjectPolicy` call that attached it recorded it.
///
/// The document is kept verbatim: it is what the caller supplied, and `ListProjectPolicies`
/// reports it back unchanged.
#[derive(Debug, Clone)]
pub struct ProjectPolicyRecord {
    /// `PutProjectPolicy.PolicyName` — the project's identity for the policy, and the
    /// project's key for it.
    pub policy_name: String,
    /// The `PolicyDocument` the attach supplied, byte for byte.
    pub policy_document: String,
    /// The revision this service assigned the policy when it was last written. A new
    /// revision is minted on every write, which is what makes a stale one refusable
    /// ("Each time you modify a policy, Amazon Rekognition Custom Labels generates and
    /// assigns a new PolicyRevisionId and then deletes the previous version of the
    /// policy", `PutProjectPolicy.PolicyRevisionId`).
    pub policy_revision_id: String,
    /// When the policy was first attached, which an update does not change.
    pub creation_timestamp: SystemTime,
    /// When the policy was last written: the creation instant until an update replaces it.
    pub last_updated_timestamp: SystemTime,
}

impl ProjectPolicyRecord {
    /// A policy as `PutProjectPolicy` records a fresh attach of it.
    pub fn new(
        policy_name: String,
        policy_document: String,
        policy_revision_id: String,
        created_at: SystemTime,
    ) -> Self {
        Self {
            policy_name,
            policy_document,
            policy_revision_id,
            creation_timestamp: created_at,
            last_updated_timestamp: created_at,
        }
    }

    /// Record the policy as rewritten at `updated_at` with `policy_document` under
    /// `policy_revision_id`: the creation instant stays the one of the first attach, since
    /// an update does not create the policy again.
    pub fn update(
        &mut self,
        policy_document: String,
        policy_revision_id: String,
        updated_at: SystemTime,
    ) {
        self.policy_document = policy_document;
        self.policy_revision_id = policy_revision_id;
        self.last_updated_timestamp = updated_at;
    }

    /// This policy as the modelled `ProjectPolicy`, attached to the project whose ARN is
    /// `project_arn`.
    pub fn modeled(&self, project_arn: &str) -> ProjectPolicy {
        ProjectPolicy {
            project_arn: Some(project_arn.to_string()),
            policy_name: Some(self.policy_name.clone()),
            policy_revision_id: Some(self.policy_revision_id.clone()),
            policy_document: Some(self.policy_document.clone()),
            creation_timestamp: Some(::aws_smithy_types::DateTime::from(self.creation_timestamp)),
            last_updated_timestamp: Some(::aws_smithy_types::DateTime::from(
                self.last_updated_timestamp,
            )),
        }
    }
}

/// Why a project policy document is not acceptable. The two classes are one modelled
/// exception with two wordings; see this module's docs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DocumentDefect {
    /// The document is not a policy: it does not parse as JSON, or what it parses to is
    /// not a statement naming the resources it grants on.
    NotAPolicy,
    /// The document is a policy, but a resource it names is not a model version of the
    /// project the policy is being attached to.
    ResourceDoesNotExist,
}

/// The resources `document`'s statements name, or [`DocumentDefect::NotAPolicy`] when the
/// document is not a policy at all.
///
/// A resource policy's statements each name the resources they apply to, so the resources
/// of the whole document are the resources of its statements. `Statement` is accepted as
/// one statement or as a list of them, which is how IAM's JSON policy grammar writes it,
/// and nothing else about a statement is read: this reports which resources a document
/// names, not what it permits.
pub fn policy_resources(document: &str) -> Result<Vec<String>, DocumentDefect> {
    let parsed = ::serde_json::from_str::<Value>(document).map_err(|_| DocumentDefect::NotAPolicy)?;
    let statements = match parsed.get("Statement") {
        Some(Value::Array(statements)) if !statements.is_empty() => statements.clone(),
        Some(statement @ Value::Object(_)) => vec![statement.clone()],
        _ => return Err(DocumentDefect::NotAPolicy),
    };

    let mut resources = Vec::new();
    for statement in &statements {
        let named = statement
            .as_object()
            .and_then(|statement| statement.get("Resource"))
            .ok_or(DocumentDefect::NotAPolicy)?;
        match named {
            Value::String(resource) => resources.push(resource.clone()),
            Value::Array(named) => {
                for resource in named {
                    let resource = resource.as_str().ok_or(DocumentDefect::NotAPolicy)?;
                    resources.push(resource.to_string());
                }
            }
            _ => return Err(DocumentDefect::NotAPolicy),
        }
    }

    if resources.is_empty() {
        return Err(DocumentDefect::NotAPolicy);
    }
    Ok(resources)
}

/// Hold `document` to what makes a project policy acceptable for `project`: it has to be a
/// policy, and every resource it names has to be a model version `project` holds, because a
/// project policy grants copying a model version OF the project it is attached to.
pub fn validate_document_for(document: &str, project: &Project) -> Result<(), DocumentDefect> {
    let resources = policy_resources(document)?;
    if resources
        .iter()
        .any(|resource| !project.has_version_arn(resource))
    {
        return Err(DocumentDefect::ResourceDoesNotExist);
    }
    Ok(())
}

/// Whether `revision_id` is the revision a write is allowed to replace `policies`' entry
/// for `policy_name` at: the revision that entry currently carries.
///
/// A revision id is hexadecimal (`ProjectPolicyRevisionId`, `pattern ^[0-9A-Fa-f]+$`), and
/// hexadecimal is case-insensitive, so the comparison is too: the same revision written in
/// either case names the same revision.
pub fn names_current_revision(
    policies: &BTreeMap<String, ProjectPolicyRecord>,
    policy_name: &str,
    revision_id: &str,
) -> bool {
    policies
        .get(policy_name)
        .is_some_and(|policy| policy.policy_revision_id.eq_ignore_ascii_case(revision_id))
}
