// SPDX-License-Identifier: CC0-1.0

//! The workflow resources this store holds: what a workflow create writes, and what a
//! workflow read reports.
//!
//! A workflow is one of this service's VERSIONED, immutable resources, exactly like a
//! component build version: "Workflow resources are immutable. To make a change, you can
//! clone a workflow or create a new version" (the modelled `Workflow`'s `version` member).
//! So the same three rules every versioned resource of this service follows hold here, and
//! each is implemented by the same reading the component path uses rather than a second one:
//!
//! * the ARN carries the Region, the workflow's STAGE (`build`, `test` or `distribution`),
//!   the normalized name, the three assigned version nodes and the build number this
//!   emulator assigned — [`workflow_build_version_arn`];
//! * a repeat of one name+version pair in one Region creates the NEXT build version
//!   ([`ComAmazonawsImagebuilderImagebuilderState::next_workflow_build_version`]), counted
//!   from [`super::FIRST_BUILD_VERSION`];
//! * a reference whose trailing version nodes are `x` wildcards reaches the newest matching
//!   build ([`ComAmazonawsImagebuilderImagebuilderState::resolve_workflow`]), through the
//!   one reading of a versioned reference this crate has
//!   ([`super::VersionedReference`]).
//!
//! The stage is part of the ARN rather than of the name, so one name may carry a `build` and
//! a `test` workflow at once — which is why the stage travels with every lookup here.

use std::collections::BTreeMap;

use crate::ops::types::{CreateWorkflowInput, Filter, Workflow, WorkflowParameterDetail};

use super::{
    normalized_name, reported_tags, version_admits, ComAmazonawsImagebuilderImagebuilderState,
    VersionNode, VersionedReference, ARN_SERVICE, ASSIGNED_VERSION_NODES, FIRST_BUILD_VERSION,
};

/// The resource field prefix every workflow ARN carries, ahead of its stage.
pub const WORKFLOW_ARN_PREFIX: &str = "workflow/";

/// The operation a workflow create's idempotency receipt is filed under. See
/// [`super::CREATE_INFRASTRUCTURE_CONFIGURATION_OPERATION`].
pub const CREATE_WORKFLOW_OPERATION: &str = "CreateWorkflow";

/// The most workflow build versions this emulator will hold. An emulator resource guard,
/// for the same reason as [`super::MAX_COMPONENTS`].
pub const MAX_WORKFLOWS: usize = 1_000;

/// The member a workflow document rides in inline, as the API spells it.
pub const WORKFLOW_DATA_MEMBER: &str = "data";

/// The member a workflow document's location rides in, as the API spells it.
pub const WORKFLOW_URI_MEMBER: &str = "uri";

/// The document key a workflow document lists its input parameters under.
const WORKFLOW_PARAMETERS_KEY: &str = "parameters";

/// The keys one declared workflow parameter carries. A workflow document names each
/// parameter under its own `name` key — unlike a component document, which lists a
/// parameter as a single-key mapping of its name to its details.
const WORKFLOW_PARAMETER_NAME_KEY: &str = "name";
const WORKFLOW_PARAMETER_TYPE_KEY: &str = "type";
const WORKFLOW_PARAMETER_DEFAULT_KEY: &str = "default";
const WORKFLOW_PARAMETER_DESCRIPTION_KEY: &str = "description";

/// The stage segment a workflow ARN carries for the workflow `workflow_type` names.
///
/// The modelled `WorkflowType` is `BUILD` / `TEST` / `DISTRIBUTION`
/// (`smithy_query(query="shape",shape="WorkflowType")`) and the modelled
/// `WorkflowBuildVersionArn` pattern carries `(build|test|distribution)` in that position,
/// so the segment is the enum value in lower case. A value the enum does not carry is
/// refused by the constraint walk before any handler runs.
#[must_use]
pub fn workflow_stage(workflow_type: &str) -> String {
    workflow_type.to_ascii_lowercase()
}

/// The resource prefix a workflow of this stage is stored under, which is what a versioned
/// reference to one is read against.
#[must_use]
pub fn workflow_arn_prefix(workflow_type: &str) -> String {
    format!("{WORKFLOW_ARN_PREFIX}{}/", workflow_stage(workflow_type))
}

/// The ARN of one build version of the `workflow_type` workflow `name` names at `version`
/// in `region`.
#[must_use]
pub fn workflow_build_version_arn(
    region: &str,
    workflow_type: &str,
    name: &str,
    version: &str,
    build_version: u64,
) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!(
            "{}{}/{version}/{build_version}",
            workflow_arn_prefix(workflow_type),
            normalized_name(name)
        ),
    )
}

/// What a workflow ARN names: the [`VersionedReference`] reading of it, plus the stage the
/// ARN carries — which is part of a workflow's identity rather than of its name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkflowReference {
    /// The Region field of the ARN.
    pub region: String,
    /// The stage segment the ARN carries, in the lower case it carries it in.
    pub stage: String,
    /// The normalized name the ARN carries.
    pub name: String,
    /// The three version nodes, each assigned or wildcard.
    pub version: [VersionNode; ASSIGNED_VERSION_NODES],
    /// The build number, when the ARN names one — a build version ARN does, a version ARN
    /// does not.
    pub build_version: Option<u64>,
}

/// The stages a workflow ARN may carry, as the modelled `WorkflowBuildVersionArn` pattern's
/// own alternation `(build|test|distribution)` spells them.
const WORKFLOW_STAGES: [&str; 3] = ["build", "test", "distribution"];

impl WorkflowReference {
    /// What `value` names, or `None` when it is not a workflow ARN of this service.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        WORKFLOW_STAGES.iter().find_map(|stage| {
            let prefix = format!("{WORKFLOW_ARN_PREFIX}{stage}/");
            let reference = VersionedReference::parse(value, &prefix)?;
            Some(Self {
                region: reference.region,
                stage: (*stage).to_string(),
                name: reference.name,
                version: reference.version,
                build_version: reference.build_version,
            })
        })
    }

    /// Whether this reference reaches the stored build version `record`.
    #[must_use]
    pub fn matches(&self, record: &WorkflowRecord) -> bool {
        if self.region != record.region
            || self.stage != workflow_stage(&record.workflow_type)
            || self.name != normalized_name(&record.name)
        {
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

/// One stored workflow build version: what a create wrote, which is what every later read
/// reports.
#[derive(Debug, Clone)]
pub struct WorkflowRecord {
    /// This build version's own ARN, which is also its key in the store.
    pub arn: String,
    /// The Region whose endpoint created it, and the only one that can reach it.
    pub region: String,
    /// The name as the caller submitted it; the ARN carries its [`normalized_name`].
    pub name: String,
    /// The three assigned version nodes, as the caller submitted them.
    pub version: String,
    /// The fourth node, which Image Builder assigns: which build of this name+version pair
    /// this is, counted from [`FIRST_BUILD_VERSION`].
    pub build_version: u64,
    /// `BUILD`, `TEST` or `DISTRIBUTION`, as the request named it — "The image creation
    /// stage that the workflow applies to" (the modelled `type` member).
    pub workflow_type: String,
    pub description: Option<String>,
    pub change_description: Option<String>,
    /// The account that owns it.
    pub owner: String,
    /// The document this workflow carries: the inline `data` the create carried, or the
    /// one-time snapshot fetched from the `uri` it named instead.
    pub data: Option<String>,
    /// The document URI the create carried instead of an inline document, when it carried
    /// one. The document itself was fetched from it at create time and lives in `data`.
    pub uri: Option<String>,
    pub kms_key_id: Option<String>,
    /// The parameters the document declared, derived from it at create time.
    pub parameters: Option<Vec<WorkflowParameterDetail>>,
    /// When it was created, as the modelled string `DateTime` carries an instant.
    pub date_created: String,
    pub tags: Option<BTreeMap<String, String>>,
}

impl WorkflowRecord {
    /// This record as the modelled `Workflow` a read reports.
    #[must_use]
    pub fn view(&self) -> Workflow {
        Workflow {
            arn: Some(self.arn.clone()),
            name: Some(self.name.clone()),
            version: Some(self.version.clone()),
            description: self.description.clone(),
            change_description: self.change_description.clone(),
            r#type: Some(self.workflow_type.clone()),
            // The modelled `WorkflowStatus` carries exactly one value, `DEPRECATED`
            // (`smithy_query(query="shape",shape="WorkflowStatus")`), and this workflow is
            // not deprecated — so there is no state to report for it.
            state: None,
            owner: Some(self.owner.clone()),
            data: self.data.clone(),
            kms_key_id: self.kms_key_id.clone(),
            date_created: Some(self.date_created.clone()),
            tags: Some(reported_tags(self.tags.as_ref())),
            parameters: self.parameters.clone(),
        }
    }
}

impl ComAmazonawsImagebuilderImagebuilderState {
    /// The workflow build version `arn` names exactly, but only when it belongs to
    /// `request_region`.
    #[must_use]
    pub fn workflow(&self, arn: &str, request_region: &str) -> Option<&WorkflowRecord> {
        self.workflows
            .get(arn)
            .filter(|record| record.region == request_region)
    }

    /// The workflow build version the ARN `reference` names, when one is stored in
    /// `request_region`.
    ///
    /// `reference` may be a build version ARN or a version ARN whose trailing nodes are `x`
    /// wildcards (the modelled `WorkflowVersionArnOrBuildVersionArn` admits both), and a
    /// version ARN reaches the newest matching build — the rule every versioned reference of
    /// this service follows.
    #[must_use]
    pub fn resolve_workflow(
        &self,
        reference: &str,
        request_region: &str,
    ) -> Option<&WorkflowRecord> {
        let reference = WorkflowReference::parse(reference)?;
        if reference.region != request_region {
            return None;
        }
        self.workflows
            .values()
            .filter(|record| reference.matches(record))
            .max_by_key(|record| record.build_version)
    }

    /// The build number the NEXT build version of this stage+name+version triple carries.
    #[must_use]
    pub fn next_workflow_build_version(
        &self,
        region: &str,
        workflow_type: &str,
        normalized: &str,
        version: &str,
    ) -> u64 {
        self.workflow_builds(region, workflow_type, normalized, version)
            .into_iter()
            .map(|record| record.build_version)
            .max()
            .map_or(FIRST_BUILD_VERSION, |latest| latest.saturating_add(1))
    }

    /// The build version of this stage+name+version triple with the greatest build number,
    /// if any is stored.
    #[must_use]
    pub fn latest_workflow_build(
        &self,
        region: &str,
        workflow_type: &str,
        normalized: &str,
        version: &str,
    ) -> Option<&WorkflowRecord> {
        self.workflow_builds(region, workflow_type, normalized, version)
            .into_iter()
            .max_by_key(|record| record.build_version)
    }

    /// Every stored build version of one workflow stage+name+version triple in one Region.
    fn workflow_builds<'store>(
        &'store self,
        region: &str,
        workflow_type: &str,
        normalized: &str,
        version: &str,
    ) -> Vec<&'store WorkflowRecord> {
        let stage = workflow_stage(workflow_type);
        self.workflows
            .values()
            .filter(|record| {
                record.region == region
                    && workflow_stage(&record.workflow_type) == stage
                    && normalized_name(&record.name) == normalized
                    && record.version == version
            })
            .collect()
    }

    /// Store a newly created workflow build version together with the receipt for the
    /// request that created it.
    pub fn insert_workflow(
        &mut self,
        record: WorkflowRecord,
        client_token: &str,
        fingerprint: String,
        request_id: String,
    ) {
        self.file_create_receipt(
            CREATE_WORKFLOW_OPERATION,
            &record.region,
            client_token,
            fingerprint,
            request_id,
            record.arn.clone(),
        );
        self.workflows.insert(record.arn.clone(), record);
    }

    /// Whether the store is already holding as many workflows as this emulator will keep
    /// ([`MAX_WORKFLOWS`]).
    #[must_use]
    pub fn workflow_store_is_full(&self) -> bool {
        self.workflows.len() >= MAX_WORKFLOWS
    }
}

/// What a submitted workflow document declares, or why it is not a workflow document.
///
/// The `Err` is the REQUIREMENT CLAUSE the refusal states
/// ([`workflow_document_message`]): this service's own statement of what a workflow
/// document has to be, never the parser's diagnostic text. The document is bounded before
/// it is parsed — the modelled `data` member is `len 1..16000`, which the constraint walk
/// refuses past before a handler runs — and an aliased or deeply nested document is refused
/// rather than expanded, through the same two guards the component document passes
/// ([`super::guard_document_nesting`]).
///
/// What is READ from it is the parameters it declares, which is what a read of the workflow
/// reports. Nothing else is required of its body: a workflow document's steps are the work
/// a workflow RUN performs, and this service stores the document whole, so the document is
/// accepted as long as it is one YAML mapping.
pub fn parse_workflow_document(data: &str) -> Result<Option<Vec<WorkflowParameterDetail>>, String> {
    if super::guard_document_nesting(data).is_none() {
        return Err("The workflow document nests too deeply.".to_string());
    }
    if super::document_uses_aliases(data) {
        return Err("The workflow document must not use YAML aliases.".to_string());
    }
    let mut documents = ::yaml_rust2::YamlLoader::load_from_str(data)
        .map_err(|_| "The workflow document is not a valid YAML document.".to_string())?;
    if documents.len() != 1 {
        return Err(format!(
            "The workflow document must be a single YAML document; this request carries {}.",
            documents.len()
        ));
    }
    let document = documents.remove(0);
    let Some(map) = document.as_hash() else {
        return Err(
            "The workflow document must be a YAML mapping of document keys to values.".to_string(),
        );
    };
    Ok(declared_workflow_parameters(map))
}

/// The parameters a workflow document declares, each as the modelled
/// `WorkflowParameterDetail` a read reports.
///
/// A workflow document lists its parameters as mappings carrying the parameter's own `name`
/// key, so an entry that is not one declares no parameter and is passed over. `name` and
/// `type` are the two details the modelled structure requires, so an entry without either
/// declares nothing a read could report.
fn declared_workflow_parameters(
    document: &::yaml_rust2::yaml::Hash,
) -> Option<Vec<WorkflowParameterDetail>> {
    let declared = super::mapping_get(document, WORKFLOW_PARAMETERS_KEY)?.as_vec()?;
    let parameters: Vec<WorkflowParameterDetail> = declared
        .iter()
        .filter_map(|entry| {
            let entry = entry.as_hash()?;
            Some(WorkflowParameterDetail {
                name: super::mapping_get(entry, WORKFLOW_PARAMETER_NAME_KEY)
                    .and_then(super::scalar_string)?,
                r#type: super::mapping_get(entry, WORKFLOW_PARAMETER_TYPE_KEY)
                    .and_then(super::scalar_string)?,
                default_value: super::mapping_get(entry, WORKFLOW_PARAMETER_DEFAULT_KEY)
                    .and_then(super::scalar_values),
                description: super::mapping_get(entry, WORKFLOW_PARAMETER_DESCRIPTION_KEY)
                    .and_then(super::scalar_string),
            })
        })
        .collect();
    (!parameters.is_empty()).then_some(parameters)
}

/// A deterministic rendering of the creation parameters one `CreateWorkflow` request
/// carries. See [`super::infrastructure_configuration_request_fingerprint`] for what it
/// leaves out and why.
#[must_use]
pub fn workflow_request_fingerprint(input: &CreateWorkflowInput) -> String {
    let mut fields: BTreeMap<&'static str, String> = BTreeMap::new();
    fields.insert("name", super::text(Some(input.name.as_str())));
    fields.insert(
        "semanticVersion",
        super::text(Some(input.semantic_version.as_str())),
    );
    fields.insert("type", super::text(Some(input.r#type.as_str())));
    fields.insert("description", super::text(input.description.as_deref()));
    fields.insert(
        "changeDescription",
        super::text(input.change_description.as_deref()),
    );
    fields.insert("data", super::text(input.data.as_deref()));
    fields.insert("uri", super::text(input.uri.as_deref()));
    fields.insert("kmsKeyId", super::text(input.kms_key_id.as_deref()));
    fields.insert("tags", super::tag_map(input.tags.as_ref()));
    fields
        .iter()
        .map(|(member, rendered)| format!("{member}={rendered};"))
        .collect()
}

/// Why a `CreateWorkflow` that names neither a document nor a document location, or both,
/// is refused.
///
/// The rule is the `data` member's own: "Contains the UTF-8 encoded YAML document content
/// for the workflow. Alternatively, you can specify the `uri` of a YAML document file
/// stored in Amazon S3" — a requirement over the two members TOGETHER rather than a
/// constraint on either value, so it is stated in the sentence this service states every
/// refused parameter in, naming both members.
#[must_use]
pub fn workflow_data_or_uri_message() -> String {
    ::service_common::validation_message::parameters_value_mismatch(
        &[WORKFLOW_DATA_MEMBER, WORKFLOW_URI_MEMBER],
        "You must specify either data or a uri.",
    )
}

/// Why a `CreateWorkflow` naming a URI that is not an S3 URL is refused.
#[must_use]
pub fn workflow_uri_message() -> String {
    ::service_common::validation_message::parameter_value_mismatch(
        WORKFLOW_URI_MEMBER,
        "uri must be an S3 URL (s3://bucket/key)",
    )
}

/// Why a `CreateWorkflow` naming a URI the object store serves no object at is refused.
#[must_use]
pub fn workflow_uri_not_found_message() -> String {
    ::service_common::validation_message::parameter_value_mismatch(WORKFLOW_URI_MEMBER, "Not Found")
}

/// Why a `CreateWorkflow` whose document this service cannot accept is refused.
///
/// `member` is the member the document ARRIVED in — `data` for an inline document, `uri`
/// for one fetched from the object store — and `why` is the clause
/// [`parse_workflow_document`] states for it: the document is the VALUE that member
/// carried, so a document this service cannot accept is refused as that parameter's value.
#[must_use]
pub fn workflow_document_message(member: &str, why: &str) -> String {
    ::service_common::validation_message::parameter_value_mismatch(member, why)
}

/// Why a create is refused once the workflow store is full.
#[must_use]
pub fn workflow_store_full_message() -> String {
    format!("This account already holds the maximum of {MAX_WORKFLOWS} workflows.")
}

/// The operation a workflow listing's page token is issued for. See
/// [`super::LIST_IMAGE_RECIPES_OPERATION`].
pub const LIST_WORKFLOWS_OPERATION: &str = "ListWorkflows";

/// The most workflow entries one page of a listing carries: the modelled `maxResults`
/// maximum (`smithy_query(query="operation",operation="ListWorkflows")` reports
/// `range 1..25`), which is also what a request naming no page size gets.
pub const MAX_WORKFLOWS_PER_PAGE: usize = 25;

/// The members `ListWorkflows` filters on, as its own `filters` member documents them
/// (`smithy_query(query="doc",shape="ListWorkflowsRequest",member="filters")`: "You can
/// filter on name, version, description, and type").
pub const WORKFLOW_FILTER_NAMES: &[&str] = &["name", "version", "description", "type"];

impl WorkflowRecord {
    /// The four version nodes this build version sorts by: the three assigned ones and the
    /// build number, so the newest matching build is the greatest of them.
    #[must_use]
    pub fn version_order(&self) -> [u64; ASSIGNED_VERSION_NODES + 1] {
        let [major, minor, patch] =
            super::parse_semantic_version(&self.version).unwrap_or_default();
        [major, minor, patch, self.build_version]
    }

    /// The version a LISTING entry this record stands for reports: its own three assigned
    /// nodes, or the wildcard version when the entry aggregates every version of one name.
    #[must_use]
    pub fn listed_version(&self, aggregated_by_name: bool) -> &str {
        if aggregated_by_name {
            super::WILDCARD_VERSION
        } else {
            self.version.as_str()
        }
    }

    /// The ARN a LISTING entry this record stands for reports: the version ARN of its three
    /// assigned nodes, or the name ARN when the entry aggregates every version of one name.
    /// Neither carries the build number — an entry stands for a version or for a name, not
    /// for one build (the modelled `WorkflowVersion`'s `arn`).
    #[must_use]
    pub fn listed_arn(&self, aggregated_by_name: bool) -> String {
        ::engine::emulator_arn(
            ARN_SERVICE,
            &self.region,
            &format!(
                "{}{}/{}",
                workflow_arn_prefix(&self.workflow_type),
                normalized_name(&self.name),
                self.listed_version(aggregated_by_name)
            ),
        )
    }

    /// This record as the modelled `WorkflowVersion` a listing reports for the version, or
    /// for the name, its entry stands for.
    #[must_use]
    pub fn version_entry(&self, aggregated_by_name: bool) -> crate::ops::types::WorkflowVersion {
        crate::ops::types::WorkflowVersion {
            arn: Some(self.listed_arn(aggregated_by_name)),
            name: Some(self.name.clone()),
            version: Some(self.listed_version(aggregated_by_name).to_string()),
            description: self.description.clone(),
            r#type: Some(self.workflow_type.clone()),
            owner: Some(self.owner.clone()),
            date_created: Some(self.date_created.clone()),
        }
    }

    /// The value of the member `filter_name` filters on, or `None` when this record reports
    /// none. See [`WORKFLOW_FILTER_NAMES`].
    #[must_use]
    pub fn filterable(&self, filter_name: &str) -> Option<&str> {
        match filter_name {
            "name" => Some(self.name.as_str()),
            "version" => Some(self.version.as_str()),
            "description" => self.description.as_deref(),
            "type" => Some(self.workflow_type.as_str()),
            _ => None,
        }
    }

    /// Whether the filter `filter_name` carrying `values` admits this record.
    ///
    /// A filter admits a record when one of its values IS the value the record reports for
    /// the member the filter names — "Filter values are case-sensitive" (the modelled
    /// `values`) — with `version` read differently, since a version pattern admits wildcard
    /// nodes and so `1.0.x` admits every patch of `1.0`.
    #[must_use]
    pub fn admits_filter(&self, filter_name: &str, values: &[String]) -> bool {
        if filter_name == super::COMPONENT_VERSION_FILTER_NAME {
            return values
                .iter()
                .any(|value| super::version_pattern_admits(value, &self.version));
        }
        self.filterable(filter_name)
            .is_some_and(|reported| values.iter().any(|value| value == reported))
    }
}

/// A deterministic rendering of which workflows one `ListWorkflows` request asked for, so a
/// token issued for one query is refused for another.
///
/// `byName` is part of it: it decides whether an entry stands for one version or for a whole
/// name, so the two listings are keyed differently. `maxResults` is deliberately NOT part of
/// it, for the reason [`super::image_recipe_query_fingerprint`] states.
#[must_use]
pub fn workflow_query_fingerprint(
    owner: Option<&str>,
    by_name: bool,
    filters: Option<&[Filter]>,
) -> String {
    format!(
        "owner={};byName={by_name};filters={};",
        super::text(Some(owner.unwrap_or(super::OWNERSHIP_SELF))),
        super::filter_list(filters)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One stored workflow, for the reference rows below.
    fn record(name: &str, workflow_type: &str, version: &str, build: u64) -> WorkflowRecord {
        WorkflowRecord {
            arn: workflow_build_version_arn("us-west-2", workflow_type, name, version, build),
            region: "us-west-2".to_string(),
            name: name.to_string(),
            version: version.to_string(),
            build_version: build,
            workflow_type: workflow_type.to_string(),
            description: None,
            change_description: None,
            owner: ::engine::EMULATOR_ACCOUNT_ID.to_string(),
            data: Some("name: w\n".to_string()),
            uri: None,
            kms_key_id: None,
            parameters: None,
            date_created: "2024-01-01T00:00:00.000Z".to_string(),
            tags: None,
        }
    }

    /// RULE: a workflow reference reaches a stored build version only when the Region, the
    /// stage, the normalized name, the version pattern and any build number it pins all
    /// admit it.
    #[test]
    fn a_workflow_reference_reaches_the_builds_its_fields_admit() {
        let stored = record("Nightly Test", "TEST", "1.2.3", 2);
        // (case, the reference, whether it reaches the stored build)
        let rows: &[(&str, String, bool)] = &[
            ("its own build version ARN", stored.arn.clone(), true),
            (
                "a fully wildcarded version ARN",
                workflow_version_reference("us-west-2", "TEST", "nightly-test", "x.x.x"),
                true,
            ),
            (
                "a minor-pinned wildcard",
                workflow_version_reference("us-west-2", "TEST", "nightly-test", "1.2.x"),
                true,
            ),
            (
                "another stage, same name and version",
                workflow_version_reference("us-west-2", "BUILD", "nightly-test", "1.2.3"),
                false,
            ),
            (
                "another Region",
                workflow_version_reference("eu-west-1", "TEST", "nightly-test", "x.x.x"),
                false,
            ),
            (
                "another version",
                workflow_version_reference("us-west-2", "TEST", "nightly-test", "1.2.4"),
                false,
            ),
            (
                "another build number",
                workflow_build_version_arn("us-west-2", "TEST", "nightly-test", "1.2.3", 1),
                false,
            ),
            (
                "a component ARN",
                ::engine::emulator_arn(ARN_SERVICE, "us-west-2", "component/nightly-test/1.2.3/2"),
                false,
            ),
        ];
        for (case, reference, expected) in rows {
            let parsed = WorkflowReference::parse(reference);
            let actual = parsed
                .as_ref()
                .is_some_and(|parsed| parsed.matches(&stored));
            assert_eq!(
                actual, *expected,
                "case {case:?}: expected {expected:?} for {reference:?}, got {actual:?}"
            );
        }
    }

    /// A version ARN (no build number) of one stage, name and version pattern.
    fn workflow_version_reference(
        region: &str,
        workflow_type: &str,
        name: &str,
        version: &str,
    ) -> String {
        ::engine::emulator_arn(
            ARN_SERVICE,
            region,
            &format!("{}{name}/{version}", workflow_arn_prefix(workflow_type)),
        )
    }

    /// RULE: the parameters a read reports are the ones the document declared, in the order
    /// it listed them; an entry missing a required detail declares nothing.
    #[test]
    fn a_workflow_documents_parameters_are_read_from_it() {
        let document = "name: t\nschemaVersion: 1.0\nparameters:\n  - name: waitFor\n    \
                        type: boolean\n    default: false\n    description: whether to wait\n  \
                        - name: message\n    type: string\n  - type: string\n  - name: noType\n\
                        steps:\n  - name: s\n    action: ExecuteBash\n";
        let parameters = parse_workflow_document(document)
            .expect("a workflow document")
            .expect("the declared parameters");
        let named: Vec<(&str, &str)> = parameters
            .iter()
            .map(|parameter| (parameter.name.as_str(), parameter.r#type.as_str()))
            .collect();
        assert_eq!(
            named,
            vec![("waitFor", "boolean"), ("message", "string")],
            "case \"a document declaring two complete parameters\": got {named:?}"
        );
        assert_eq!(
            parameters[0].default_value,
            Some(vec!["false".to_string()]),
            "case \"a declared default\": got {:?}",
            parameters[0].default_value
        );

        // (case, the document, whether it is accepted)
        let rows: &[(&str, &str, bool)] = &[
            ("a mapping with no parameters", "name: t\nsteps: []\n", true),
            ("a sequence", "- name: t\n", false),
            ("a scalar", "workflow\n", false),
            ("two documents", "name: a\n---\nname: b\n", false),
            ("an alias", "a: &x 1\nb: *x\n", false),
        ];
        for (case, document, expected) in rows {
            let actual = parse_workflow_document(document).is_ok();
            assert_eq!(
                actual, *expected,
                "case {case:?}: expected accepted={expected:?}, got {actual:?}"
            );
        }
    }
}
