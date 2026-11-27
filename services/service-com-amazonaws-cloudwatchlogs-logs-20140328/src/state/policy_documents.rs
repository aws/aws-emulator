// SPDX-License-Identifier: CC0-1.0

//! The POLICY DOCUMENTS this service stores, and the rules each document is held to: the
//! account-level policy shapes, the per-kind document checks (subscription filter, field
//! index, transformer, metric extraction, data protection), and the selection criteria that
//! say which log groups an account-level policy applies to.

use super::*;

/// Whether an ARN names an IAM role — what `OpenSearchResourceConfig$dataSourceRoleArn`
/// must be ("Specify the ARN of an IAM role that CloudWatch Logs will use to create the
/// integration"). An IAM ARN's resource part is `role/<path><name>`.
pub fn is_iam_role_arn(arn: &str) -> bool {
    arn_service_and_resource(arn)
        .is_some_and(|(service, resource)| service == IAM_SERVICE && resource.starts_with("role/"))
}

/// Whether an ARN names a principal `OpenSearchResourceConfig$dashboardViewerPrincipals`
/// admits: "the ARNs of IAM roles and IAM users who you want to grant permission to for
/// viewing the dashboards".
pub fn is_dashboard_viewer_principal_arn(arn: &str) -> bool {
    arn_service_and_resource(arn).is_some_and(|(service, resource)| {
        service == IAM_SERVICE && (resource.starts_with("role/") || resource.starts_with("user/"))
    })
}

/// Whether an ARN names an OpenSearch Service application — what
/// `OpenSearchResourceConfig$applicationArn` must be ("If you want to use an existing
/// OpenSearch Service application for your integration [...] specify it here").
pub fn is_opensearch_application_arn(arn: &str) -> bool {
    arn_service_and_resource(arn).is_some_and(|(service, resource)| {
        service == OPENSEARCH_SERVICE && resource.starts_with("application/")
    })
}

/// Whether an ARN names a KMS key — what `OpenSearchResourceConfig$kmsKeyArn` must be
/// ("specify the ARN of the KMS key that you want to use"). Unlike a log group's
/// `kmsKeyId`, this member is modelled as an `Arn`, so a bare key id is not one of its
/// values.
pub fn is_kms_key_arn(arn: &str) -> bool {
    arn_service_and_resource(arn)
        .is_some_and(|(service, resource)| service == KMS_SERVICE && !resource.is_empty())
}

/// How many account-level policies of `policy_type` the account may hold IN TOTAL once a
/// policy whose scoped-ness is `scoped` is added: each quota sentence is an either/or —
/// "either one account-level metric extraction policy that applies to all log groups, or up
/// to 5 policies that are each scoped to a subset of log groups with the selectionCriteria
/// parameter" — so the number the account is held to is the one for the policy being put,
/// and every policy of that type counts against it. Every number is one
/// `PutAccountPolicy`'s own documentation states — see the constants it returns.
pub fn max_account_policies(policy_type: &str, scoped: bool) -> usize {
    if !scoped {
        return MAX_UNSCOPED_ACCOUNT_POLICIES;
    }
    match policy_type {
        POLICY_TYPE_TRANSFORMER | POLICY_TYPE_FIELD_INDEX => MAX_SCOPED_ACCOUNT_POLICIES,
        POLICY_TYPE_METRIC_EXTRACTION => MAX_SCOPED_METRIC_EXTRACTION_ACCOUNT_POLICIES,
        // A data protection policy and a subscription filter policy have no scoped half:
        // the account holds one of each, whatever it is scoped to.
        _ => MAX_UNSCOPED_ACCOUNT_POLICIES,
    }
}

/// What an accepted account-level policy document says: the parts a handler acts on after
/// [`check_account_policy_document`] accepts it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AccountPolicyShape {
    /// The log groups a data protection policy's Audit action sends findings to, as
    /// [`DataProtectionPolicyShape::findings_destination_log_groups`] reports them. Empty
    /// for every other policy type, none of which names a destination log group.
    pub findings_destination_log_groups: Vec<String>,
}

/// Judge a `PutAccountPolicy$policyDocument` against the rules that member's own
/// documentation states for the `policyType` it is put with, answering the message to
/// refuse it with, or what the accepted document says.
///
/// Every type's document is "the policy, in JSON" and can be "up to 30,720 characters
/// long". Beyond that each type has its own documented shape, and only the rules the
/// documentation states are enforced: which data identifiers, processors or fields exist
/// is a catalogue that lives outside this service, and the grammar of `selectionCriteria`
/// is judged by the caller of this function rather than here.
pub fn check_account_policy_document(
    policy_type: &str,
    region: &str,
    document: &str,
) -> Result<AccountPolicyShape, String> {
    if document.chars().count() > MAX_ACCOUNT_POLICY_DOCUMENT_CHARS {
        return Err(format!(
            "policyDocument can be up to {MAX_ACCOUNT_POLICY_DOCUMENT_CHARS} characters"
        ));
    }
    match policy_type {
        // "A data protection policy must include two JSON blocks" — the same document the
        // log-group-level `PutDataProtectionPolicy` takes, so it is held to the same rule.
        POLICY_TYPE_DATA_PROTECTION => {
            let shape = check_data_protection_policy_document(document)?;
            Ok(AccountPolicyShape {
                findings_destination_log_groups: shape.findings_destination_log_groups,
            })
        }
        POLICY_TYPE_SUBSCRIPTION_FILTER => {
            check_subscription_filter_policy_document(region, document)
                .map(|()| AccountPolicyShape::default())
        }
        POLICY_TYPE_FIELD_INDEX => {
            check_field_index_policy_document(document).map(|_| AccountPolicyShape::default())
        }
        POLICY_TYPE_TRANSFORMER => {
            check_transformer_policy_document(document).map(|()| AccountPolicyShape::default())
        }
        // A metric extraction policy "controls whether CloudWatch Metrics can be created
        // through the Embedded Metrics Format (EMF)", and the one thing it can say is that
        // EMF metric creation is off.
        _ => check_metric_extraction_policy_document(document).map(|()| AccountPolicyShape::default()),
    }
}

/// The document as the JSON object every policy type but a transformer policy is
/// expressed as ("Specify the policy, in JSON").
pub(super) fn policy_object(document: &str) -> Result<serde_json::Map<String, serde_json::Value>, String> {
    serde_json::from_str::<serde_json::Value>(document)
        .ok()
        .and_then(|parsed| match parsed {
            serde_json::Value::Object(members) => Some(members),
            _ => None,
        })
        .filter(|members| !members.is_empty())
        .ok_or_else(|| "policyDocument must be the policy in JSON (a non-empty JSON object)".to_string())
}

/// Judge a `SUBSCRIPTION_FILTER_POLICY` document. The attributes are the ones
/// `PutAccountPolicy$policyDocument` lists for one: "DestinationArn The ARN of the
/// destination to deliver log events to", "RoleArn The ARN of an IAM role that grants
/// CloudWatch Logs permissions to deliver ingested log events to the destination stream",
/// "FilterPattern A filter pattern for subscribing to a filtered stream of log events" and
/// "Distribution The method used to distribute log data to the destination". Each is held
/// to the rule the log-group-level `PutSubscriptionFilter` holds the member of the same
/// name to, so one destination is as acceptable account-wide as it is per group — including
/// the Region it has to be in, which for an account-level policy is the Region the policy,
/// and so every log group it applies to, is in.
pub(super) fn check_subscription_filter_policy_document(
    region: &str,
    document: &str,
) -> Result<(), String> {
    let members = policy_object(document)?;
    let string = |name: &str| members.get(name).and_then(serde_json::Value::as_str);
    let Some(destination_arn) = string("DestinationArn") else {
        return Err(
            "policyDocument for a subscription filter policy must include a DestinationArn"
                .to_string(),
        );
    };
    if let Some(message) = unparsable_arn_message("DestinationArn", destination_arn) {
        return Err(message);
    }
    // The same ordering as `PutSubscriptionFilter`: a destination that needs a role is
    // refused for the missing role before the kind of resource its ARN names is judged.
    if string("RoleArn").is_none() {
        if let Some(vendor) = subscription_role_required_vendor(destination_arn) {
            return Err(format!(
                "DestinationArn for vendor {vendor} cannot be used without RoleArn"
            ));
        }
    }
    if !is_subscription_destination_arn(destination_arn) {
        return Err(format!(
            "DestinationArn must be the ARN of a {} resource",
            SUBSCRIPTION_DESTINATIONS
                .iter()
                .map(|(service, _)| *service)
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    // The same rule again: the policy subscribes the log groups of the Region it is put in,
    // so a destination in another Region is a delivery none of them could make
    // ([`cross_region_destination_message`]).
    if let Some(message) = cross_region_destination_message(region, destination_arn) {
        return Err(message.to_string());
    }
    if let Some(filter_pattern) = string("FilterPattern") {
        // The same reading a subscription filter's own put holds the member to.
        super::filter_pattern::validate_configured_pattern(filter_pattern)?;
    }
    // "the grouping can be set to Random for a more even distribution": the two values of
    // the model's `Distribution` enum, which no boundary validator holds a JSON document
    // to, listed in the one order this service states that enum's values in.
    if let Some(distribution) = string("Distribution") {
        if !crate::ops::types::DISTRIBUTION_VALUES.contains(&distribution) {
            return Err(invalid_value_message(
                "Distribution",
                crate::service::enum_value_set(
                    crate::ops::types::DISTRIBUTION_VALUES.iter().copied(),
                ),
            ));
        }
    }
    Ok(())
}

/// What a `SUBSCRIPTION_FILTER_POLICY` document CONFIGURES, read from an accepted document:
/// the parts the fan-out of an ingested batch acts on ([`super::subscription_delivery`]).
///
/// The members are the attributes `PutAccountPolicy$policyDocument` lists for such a policy,
/// each standing for the `PutSubscriptionFilter` member of the same name
/// ([`check_subscription_filter_policy_document`]), so one document configures account-wide
/// exactly what one put configures for one log group.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubscriptionFilterPolicy {
    /// "DestinationArn The ARN of the destination to deliver log events to".
    pub destination_arn: String,
    /// "RoleArn The ARN of an IAM role that grants CloudWatch Logs permissions to deliver
    /// ingested log events to the destination stream"; `None` for a document naming none,
    /// which is the Lambda destination that needs no role.
    pub role_arn: Option<String>,
    /// "FilterPattern A filter pattern for subscribing to a filtered stream of log events".
    /// A document naming none subscribes to the whole stream, which is what the empty
    /// pattern selects.
    pub filter_pattern: String,
    /// "Distribution The method used to distribute log data to the destination", with the
    /// documented default of a put that names none ([`super::DEFAULT_DISTRIBUTION`]).
    pub distribution: String,
}

/// What `document` configures, or `None` when it is no subscription filter policy document at
/// all — one that names no destination configures no delivery.
///
/// A stored document went through [`check_subscription_filter_policy_document`] when it was
/// put, so this reads the accepted document rather than judging it again.
pub fn subscription_filter_policy(document: &str) -> Option<SubscriptionFilterPolicy> {
    let members = policy_object(document).ok()?;
    let string = |name: &str| {
        members
            .get(name)
            .and_then(serde_json::Value::as_str)
            .map(str::to_string)
    };
    Some(SubscriptionFilterPolicy {
        destination_arn: string("DestinationArn")?,
        role_arn: string("RoleArn"),
        filter_pattern: string("FilterPattern").unwrap_or_default(),
        distribution: string("Distribution")
            .unwrap_or_else(|| super::DEFAULT_DISTRIBUTION.to_string()),
    })
}

/// One field index a field index policy document names: the field whose log event values
/// are indexed, and the type the document gives it.
///
/// From `PutIndexPolicy$policyDocument`: "You can use FieldsV2 to specify the type for each
/// field. Supported types are FIELD_INDEX and FACET." A name listed in the plain `Fields`
/// array carries no type of its own, and that array is "The array of field indexes to
/// create", so such a name is a field index ([`FIELDS_ARRAY_INDEX_TYPE`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexedField {
    /// The field whose values are indexed — what `FieldIndex$fieldIndexName` reports as
    /// "the string that this field index matches".
    pub name: String,
    /// The field index's type, one of [`crate::ops::types::INDEX_TYPE_VALUES`].
    pub index_type: String,
}

/// The `IndexType` of a field index named in a document's plain `Fields` array, which
/// gives no type: that array is "The array of field indexes to create", so the index it
/// creates is a field index rather than a facet. Spelled as the model's `IndexType` spells
/// it ([`crate::ops::types::INDEX_TYPE_VALUES`]).
pub const FIELDS_ARRAY_INDEX_TYPE: &str = "FIELD_INDEX";

/// Judge a field index policy document, answering the message to refuse it with, or the
/// field indexes it creates — `Fields` in the order the array lists them, then the
/// `FieldsV2` names.
///
/// From `PutAccountPolicy$policyDocument`, which states the same document's rules as
/// `PutIndexPolicy$policyDocument`: the attributes are "Fields The array of field indexes
/// to create" and "FieldsV2 The object of field indexes to create along with it's type";
/// "It must contain at least one field index"; "Supported types are FIELD_INDEX and FACET"
/// (the model's `IndexType` enum); and "Field names within Fields and FieldsV2 must be
/// mutually exclusive."
///
/// The per-policy field quotas are NOT judged here, because they are a log-group-level
/// policy's own ([`check_index_policy_quotas`]).
pub fn check_field_index_policy_document(document: &str) -> Result<Vec<IndexedField>, String> {
    // The document's FORM is refused with the service's fixed text for a policy document it
    // cannot read ([`MALFORMED_POLICY_DOCUMENT_MESSAGE`]); what a readable document SAYS is
    // refused below with the sentence of the rule it breaks.
    let malformed = || MALFORMED_POLICY_DOCUMENT_MESSAGE.to_string();
    let members = policy_object(document).map_err(|_| malformed())?;
    let fields = match members.get("Fields") {
        Some(fields) => fields
            .as_array()
            .ok_or_else(malformed)?
            .iter()
            .map(|field| field.as_str().map(str::to_string).ok_or_else(malformed))
            .collect::<Result<Vec<String>, String>>()?,
        None => Vec::new(),
    };
    let fields_v2 = match members.get("FieldsV2") {
        Some(fields_v2) => fields_v2.as_object().ok_or_else(malformed)?.clone(),
        None => serde_json::Map::new(),
    };
    let mut typed: Vec<IndexedField> = Vec::new();
    for (name, configuration) in &fields_v2 {
        let index_type = configuration
            .get("type")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| format!("the FieldsV2 entry '{name}' in policyDocument must carry a type"))?;
        if !crate::ops::types::INDEX_TYPE_VALUES.contains(&index_type) {
            return Err(invalid_value_message(
                &format!("FieldsV2 '{name}' type"),
                crate::ops::types::INDEX_TYPE_VALUES.iter().copied(),
            ));
        }
        typed.push(IndexedField {
            name: name.clone(),
            index_type: index_type.to_string(),
        });
    }
    if fields.is_empty() && fields_v2.is_empty() {
        return Err(EMPTY_INDEX_POLICY_DOCUMENT_MESSAGE.to_string());
    }
    if let Some(name) = fields.iter().find(|field| fields_v2.contains_key(*field)) {
        return Err(format!(
            "the field '{name}' is listed in both Fields and FieldsV2 in policyDocument; field names within Fields and FieldsV2 must be mutually exclusive"
        ));
    }
    let mut indexed: Vec<IndexedField> = fields
        .into_iter()
        .map(|name| IndexedField {
            name,
            index_type: FIELDS_ARRAY_INDEX_TYPE.to_string(),
        })
        .collect();
    indexed.extend(typed);
    Ok(indexed)
}

/// Judge the field indexes of a log-group-level field index policy against the quotas
/// `PutIndexPolicy` states for "each index policy": [`MAX_INDEX_POLICY_FIELDS`] fields in
/// the policy, and [`MAX_INDEX_FIELD_NAME_CHARS`] characters in each field name. The names
/// are the ones [`check_field_index_policy_document`] read out of the document.
///
/// Both breaches are a fault of the DOCUMENT this request carries — a value of a parameter,
/// which no retry of the same request can turn into an accepted one — so the `Err` carries
/// the sentence stating the rule and the caller refuses the request as the incorrectly
/// specified parameter it is. Neither is a count of what the account HOLDS, which is what a
/// resource-limit refusal reports.
pub fn check_index_policy_quotas(fields: &[IndexedField]) -> Result<(), String> {
    if let Some(length) = fields
        .iter()
        .map(|field| field.name.chars().count())
        .find(|length| *length > MAX_INDEX_FIELD_NAME_CHARS)
    {
        return Err(format!(
            "a field name in policyDocument is {length} characters; each field name can include as many as {MAX_INDEX_FIELD_NAME_CHARS} characters"
        ));
    }
    if fields.len() > MAX_INDEX_POLICY_FIELDS {
        return Err(format!(
            "Policy document contains more than {MAX_INDEX_POLICY_FIELDS} fields."
        ));
    }
    Ok(())
}

/// The PROCESSOR BLOCKS a `TRANSFORMER_POLICY` document carries: "a transformer policy must
/// include one JSON block with the array of processors and their configurations", written either
/// as that array itself or as an object holding it. `None` for a document that is not JSON, or
/// that carries no such array at all.
///
/// ONE reading of that shape, shared by the check below — which judges the blocks it answers —
/// and by the ingest path that RUNS them
/// ([`super::active_transformer`]), so the two cannot drift apart.
pub(super) fn transformer_policy_processor_blocks(
    document: &str,
) -> Option<Vec<serde_json::Value>> {
    match serde_json::from_str::<serde_json::Value>(document).ok()? {
        serde_json::Value::Array(processors) => Some(processors),
        serde_json::Value::Object(members) => members
            .values()
            .find_map(|value| value.as_array().cloned()),
        _ => None,
    }
}

/// Judge a `TRANSFORMER_POLICY` document: "A transformer policy must include one JSON
/// block with the array of processors and their configurations." Which processors exist,
/// and how each is configured, is the catalogue `PutTransformer` owns, so what is judged
/// here is that the document carries a non-empty array of processor blocks
/// ([`transformer_policy_processor_blocks`]) whose FIRST block names a
/// parse-type processor, and in which a parser for an AWS-vended log format appears nowhere
/// but first: the two rules a transformer's pipeline has to satisfy wherever it is configured
/// ([`PARSE_TYPE_PROCESSORS`], [`LEADING_ONLY_PROCESSORS`]).
pub(super) fn check_transformer_policy_document(document: &str) -> Result<(), String> {
    let expected =
        "policyDocument for a transformer policy must include an array of processors".to_string();
    let processors =
        transformer_policy_processor_blocks(document).ok_or_else(|| expected.clone())?;
    if processors.is_empty() || !processors.iter().all(serde_json::Value::is_object) {
        return Err(expected);
    }
    let leading = processors
        .first()
        .and_then(serde_json::Value::as_object)
        .and_then(|block| block.keys().next())
        .ok_or_else(|| expected.clone())?;
    if !is_parse_type_processor(leading) {
        return Err(transformer_leading_parser_message(leading));
    }
    // A parser for an AWS-vended log format may appear NOWHERE but first ("If you use this
    // processor, it must be the first processor in your transformer"), which is the same rule
    // wherever a processor list is configured: an account-level transformer policy carries one
    // too, so a block naming such a parser at any later position is refused here as well
    // ([`LEADING_ONLY_PROCESSORS`]).
    if let Some(misplaced) = processors
        .iter()
        .skip(1)
        .filter_map(serde_json::Value::as_object)
        .flat_map(|block| block.keys())
        .find(|named| LEADING_ONLY_PROCESSORS.contains(&named.as_str()))
    {
        return Err(transformer_leading_only_message(misplaced));
    }
    Ok(())
}

/// Judge a `METRIC_EXTRACTION_POLICY` document.
///
/// Such a policy exists only to turn the Embedded Metrics Format off: "By default, EMF
/// metric creation is enabled for all log groups. You can use metric extraction policies to
/// disable EMF metric creation for your entire account or specific log groups." The
/// document says so through [`METRIC_EXTRACTION_EMF_MEMBER`]'s
/// [`METRIC_EXTRACTION_STATUS_MEMBER`], and the real service "accepts only the exact
/// string" [`METRIC_EXTRACTION_STATUS_DISABLED`] there — a document that says anything
/// else, including that EMF is enabled, is refused with
/// [`INVALID_JSON_POLICY_DOCUMENT_MESSAGE`]. Any other member of the document is left
/// alone: nothing documents one, so nothing is judged about it.
pub(super) fn check_metric_extraction_policy_document(document: &str) -> Result<(), String> {
    let members = policy_object(document).map_err(|_| INVALID_JSON_POLICY_DOCUMENT_MESSAGE.to_string())?;
    let status = members
        .get(METRIC_EXTRACTION_EMF_MEMBER)
        .and_then(serde_json::Value::as_object)
        .and_then(|embedded_metric_format| embedded_metric_format.get(METRIC_EXTRACTION_STATUS_MEMBER))
        .and_then(serde_json::Value::as_str);
    if status != Some(METRIC_EXTRACTION_STATUS_DISABLED) {
        return Err(INVALID_JSON_POLICY_DOCUMENT_MESSAGE.to_string());
    }
    Ok(())
}

/// One filter of a `selectionCriteria` string: the field it selects on, whether it EXCLUDES
/// the values it names rather than selecting them, and those values.
///
/// The forms are the ones `PutAccountPolicy` spells out — `LogGroupName IN ["log-group-1",
/// "log-group-2"]`, `LogGroupNamePrefix NOT IN ["/aws/prefix1", "/aws/prefix2"]`,
/// `LogGroupNamePrefix = "my-prefix"` — and the filters are the ones
/// `PutAccountPolicy$selectionCriteria` names per policy type
/// ([`SELECTION_FIELD_LOG_GROUP_NAME`], [`SELECTION_FIELD_LOG_GROUP_NAME_PREFIX`],
/// [`SELECTION_FIELD_DATA_SOURCE_NAME`], [`SELECTION_FIELD_DATA_SOURCE_TYPE`]). A clause
/// selects the same log groups whichever of those forms carries its values, so everything
/// downstream reads the clause rather than the string it was written as.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectionClause {
    /// The filter's field, as the criteria string spells it.
    pub field: String,
    /// `true` for a `NOT IN` list, `false` for an `IN` list or an equality.
    pub negated: bool,
    /// The values the filter names, in the order written: the one value of an equality, or
    /// the list's. Empty for the documented `LogGroupName NOT IN []`.
    pub values: Vec<String>,
}

impl SelectionClause {
    /// One clause, from the text before its list and the values of that list; `None` when
    /// the text is not `Field IN` or `Field NOT IN`.
    pub(super) fn parse(head: &str, values: Vec<String>) -> Option<Self> {
        let mut tokens = head.split_whitespace();
        let field = tokens.next()?.to_string();
        let negated = match (tokens.next()?, tokens.next()) {
            ("IN", None) => false,
            ("NOT", Some("IN")) => true,
            _ => return None,
        };
        if tokens.next().is_some() {
            return None;
        }
        Some(Self {
            field,
            negated,
            values,
        })
    }
}

/// A `selectionCriteria` string, parsed into its filters.
///
/// More than one clause is only the field index policy's `DataSourceName` AND
/// `DataSourceType` pair — "you can use either LogGroupNamePrefix by itself or
/// DataSourceName and DataSourceType together" — which is the one documented form that
/// joins two filters.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SelectionCriteria {
    /// The criteria's clauses, in the order written.
    pub clauses: Vec<SelectionClause>,
}

impl SelectionCriteria {
    /// This criteria's clause on `field`, when it carries one.
    pub fn clause(&self, field: &str) -> Option<&SelectionClause> {
        self.clauses.iter().find(|clause| clause.field == field)
    }

    /// Whether this criteria is exactly one clause on `field`, optionally with the
    /// operator `negated` names.
    pub(super) fn is_single(&self, field: &str, negated: Option<bool>) -> bool {
        match self.clauses.as_slice() {
            [clause] => {
                clause.field == field && negated.is_none_or(|negated| clause.negated == negated)
            }
            _ => false,
        }
    }

    /// Whether this criteria is the `DataSourceName` AND `DataSourceType` pair.
    pub(super) fn is_data_source_pair(&self) -> bool {
        self.clauses.len() == 2
            && self.clause(SELECTION_FIELD_DATA_SOURCE_NAME).is_some()
            && self.clause(SELECTION_FIELD_DATA_SOURCE_TYPE).is_some()
    }
}

/// One set of log groups a `selectionCriteria` value names.
///
/// A `LogGroupName` value names the one group called that; a `LogGroupNamePrefix` value
/// names every group whose name starts with it. Both comparisons below follow from those
/// two sets, which is what `PutAccountPolicy`'s overlap sentences are stated in terms of
/// ("the set of log groups matching these prefixes would be a subset of the log groups
/// matching the first policy's prefix, creating an overlap").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum GroupSelector<'a> {
    /// The group whose name is exactly this.
    Name(&'a str),
    /// Every group whose name starts with this.
    Prefix(&'a str),
}

impl GroupSelector<'_> {
    /// Whether the two sets of log groups share a member.
    pub(super) fn intersects(self, other: Self) -> bool {
        match (self, other) {
            (Self::Name(one), Self::Name(two)) => one == two,
            (Self::Name(name), Self::Prefix(prefix)) | (Self::Prefix(prefix), Self::Name(name)) => {
                name.starts_with(prefix)
            }
            (Self::Prefix(one), Self::Prefix(two)) => {
                one.starts_with(two) || two.starts_with(one)
            }
        }
    }

    /// Whether this set of log groups is wholly inside `other` — "completely contained
    /// within", the test `PutAccountPolicy` states for an `IN` value against a `NOT IN`
    /// one. A prefix names groups a single name does not, so it is contained in no name.
    pub(super) fn contained_in(self, other: Self) -> bool {
        match (self, other) {
            (Self::Name(one), Self::Name(two)) => one == two,
            (Self::Name(name), Self::Prefix(prefix)) => name.starts_with(prefix),
            (Self::Prefix(one), Self::Prefix(two)) => one.starts_with(two),
            (Self::Prefix(_), Self::Name(_)) => false,
        }
    }
}

/// What one criteria selects: a set of log groups, or a data source name and type
/// combination. `None` for a criteria that selects neither, which nothing can overlap.
pub(super) enum CriteriaSelection<'a> {
    /// The log groups the criteria's `LogGroupName` or `LogGroupNamePrefix` clause names,
    /// and whether that clause excludes them (`NOT IN`) rather than selects them.
    Groups {
        negated: bool,
        selectors: Vec<GroupSelector<'a>>,
    },
    /// The data source names and types of the criteria's `DataSourceName` AND
    /// `DataSourceType` pair.
    DataSource {
        names: &'a [String],
        types: &'a [String],
    },
}

impl<'a> CriteriaSelection<'a> {
    /// What `criteria` selects.
    pub(super) fn of(criteria: &'a SelectionCriteria) -> Option<Self> {
        if let (Some(names), Some(types)) = (
            criteria.clause(SELECTION_FIELD_DATA_SOURCE_NAME),
            criteria.clause(SELECTION_FIELD_DATA_SOURCE_TYPE),
        ) {
            return Some(Self::DataSource {
                names: &names.values,
                types: &types.values,
            });
        }
        let clause = criteria
            .clause(SELECTION_FIELD_LOG_GROUP_NAME)
            .or_else(|| criteria.clause(SELECTION_FIELD_LOG_GROUP_NAME_PREFIX))?;
        let selectors = clause
            .values
            .iter()
            .map(|value| {
                if clause.field == SELECTION_FIELD_LOG_GROUP_NAME {
                    GroupSelector::Name(value)
                } else {
                    GroupSelector::Prefix(value)
                }
            })
            .collect();
        Some(Self::Groups {
            negated: clause.negated,
            selectors,
        })
    }
}

/// Judge a `PutAccountPolicy$selectionCriteria` string against the filters that member
/// documents for the `policyType` it is put with, answering the message to refuse it with,
/// or the parsed criteria.
///
/// The filters are exactly the documented ones:
///
/// * `SUBSCRIPTION_FILTER_POLICY` — "the only supported selectionCriteria filter is
///   `LogGroupName NOT IN []`";
/// * `TRANSFORMER_POLICY` — "the only supported selectionCriteria filter is
///   `LogGroupNamePrefix`";
/// * `FIELD_INDEX_POLICY` — "you can use either `LogGroupNamePrefix` by itself or
///   `DataSourceName` and `DataSourceType` together";
/// * `METRIC_EXTRACTION_POLICY` — "The selection criteria supports filtering by
///   `LogGroupName` and `LogGroupNamePrefix` using the operators `IN` and `NOT IN`. You can
///   specify up to 50 values in each `IN` or `NOT IN` list."
///
/// Which operator a `LogGroupNamePrefix` filter is written with is not documented for a
/// transformer or a field index policy, so every form of it is accepted there — the set
/// operators over a list and equality on a single value alike ([`parse_selection_clauses`]).
pub fn check_selection_criteria(policy_type: &str, criteria: &str) -> Result<SelectionCriteria, String> {
    let parsed = SelectionCriteria {
        clauses: parse_selection_clauses(criteria)?,
    };
    let supported = match policy_type {
        POLICY_TYPE_SUBSCRIPTION_FILTER => {
            parsed.is_single(SELECTION_FIELD_LOG_GROUP_NAME, Some(true))
        }
        POLICY_TYPE_TRANSFORMER => parsed.is_single(SELECTION_FIELD_LOG_GROUP_NAME_PREFIX, None),
        POLICY_TYPE_FIELD_INDEX => {
            parsed.is_single(SELECTION_FIELD_LOG_GROUP_NAME_PREFIX, None)
                || parsed.is_data_source_pair()
        }
        POLICY_TYPE_METRIC_EXTRACTION => {
            parsed.is_single(SELECTION_FIELD_LOG_GROUP_NAME, None)
                || parsed.is_single(SELECTION_FIELD_LOG_GROUP_NAME_PREFIX, None)
        }
        // A data protection policy admits no criteria at all, and its caller refuses one
        // before it gets here.
        _ => false,
    };
    if !supported {
        return Err(format!(
            "selectionCriteria for a {policy_type} must be {}",
            selection_criteria_forms(policy_type)
        ));
    }
    if policy_type == POLICY_TYPE_METRIC_EXTRACTION
        && parsed
            .clauses
            .iter()
            .any(|clause| clause.values.len() > MAX_SELECTION_CRITERIA_VALUES)
    {
        return Err(format!(
            "selectionCriteria can specify up to {MAX_SELECTION_CRITERIA_VALUES} values in each IN or NOT IN list"
        ));
    }
    Ok(parsed)
}

/// The `selectionCriteria` filters one policy type admits, as a refusal names them. See
/// [`check_selection_criteria`] for where each comes from.
pub(super) fn selection_criteria_forms(policy_type: &str) -> &'static str {
    match policy_type {
        POLICY_TYPE_SUBSCRIPTION_FILTER => "the filter LogGroupName NOT IN [...]",
        POLICY_TYPE_TRANSFORMER => "a LogGroupNamePrefix filter",
        POLICY_TYPE_FIELD_INDEX => {
            "a LogGroupNamePrefix filter, or DataSourceName IN [...] AND DataSourceType IN [...] together"
        }
        POLICY_TYPE_METRIC_EXTRACTION => {
            "a LogGroupName or LogGroupNamePrefix filter with the IN or NOT IN operator"
        }
        _ => "omitted: this policy type applies to all log groups in the account",
    }
}

/// Split a `selectionCriteria` string into its clauses — each a filter written over a LIST
/// (`Field IN [...]`, `Field NOT IN [...]`) or over a SINGLE value (`Field = "value"`),
/// joined by `AND` — refusing a string that is not written that way.
pub(super) fn parse_selection_clauses(criteria: &str) -> Result<Vec<SelectionClause>, String> {
    let malformed = || MALFORMED_SELECTION_CRITERIA_MESSAGE.to_string();
    let mut clauses = Vec::new();
    let mut rest = criteria.trim();
    loop {
        let (clause, after) = parse_selection_clause(rest).ok_or_else(malformed)?;
        clauses.push(clause);
        rest = after.trim_start();
        if rest.is_empty() {
            break;
        }
        let joined = rest.strip_prefix("AND").ok_or_else(malformed)?;
        if !joined.starts_with(char::is_whitespace) {
            return Err(malformed());
        }
        rest = joined.trim_start();
        if rest.is_empty() {
            return Err(malformed());
        }
    }
    if clauses.is_empty() {
        return Err(malformed());
    }
    Ok(clauses)
}

/// How CloudWatch Logs refuses a `selectionCriteria` string that is not written as filters at
/// all; see [`parse_selection_clauses`] for the forms one is written in.
pub(super) const MALFORMED_SELECTION_CRITERIA_MESSAGE: &str =
    "selectionCriteria must be one or more filters of the form 'Field IN [\"value\", ...]', \
     'Field NOT IN [\"value\", ...]' or 'Field = \"value\"', joined by AND";

/// ONE clause off the front of `text`, and whatever follows it; `None` when the front of
/// `text` is not a filter.
///
/// A filter states its field, an operator and the values it selects on. The operator is
/// either a SET operator over a list of values (`IN`, `NOT IN` — the form
/// `PutAccountPolicy$selectionCriteria` spells out for the lists it bounds, "up to 50 values
/// in each IN or NOT IN list") or EQUALITY on a single value (`LogGroupNamePrefix =
/// "my-prefix"`, the form the member's prefix filters are written in), and the two select the
/// same log groups when they carry the same value: equality is the one-value list.
///
/// Which form a clause is written in is decided by its own operator — whichever of the list's
/// `[` and equality's `=` opens first — so a value containing one of those characters inside
/// the other form's quotes does not change the form.
fn parse_selection_clause(text: &str) -> Option<(SelectionClause, &str)> {
    let list_at = text.find('[');
    let equals_at = text.find('=');
    if let Some(open) = list_at.filter(|open| equals_at.is_none_or(|equals| *open < equals)) {
        let (head, list) = text.split_at(open);
        let close = selection_list_end(list)?;
        // The list is a JSON array of strings in every documented form, so it is read as
        // one rather than scanned by hand.
        let values = serde_json::from_str::<Vec<String>>(&list[..=close]).ok()?;
        return Some((SelectionClause::parse(head, values)?, &list[close + 1..]));
    }
    let equals = equals_at?;
    let field = single_token(&text[..equals])?;
    // The value is a JSON string, as every value of the list form is.
    let value_text = text[equals + 1..].trim_start();
    let close = selection_string_end(value_text)?;
    let value = serde_json::from_str::<String>(&value_text[..=close]).ok()?;
    Some((
        SelectionClause {
            field,
            negated: false,
            values: vec![value],
        },
        &value_text[close + 1..],
    ))
}

/// The one whitespace-free word `text` holds, or `None` when it holds none or several — what
/// a filter's field is written as.
fn single_token(text: &str) -> Option<String> {
    let mut tokens = text.split_whitespace();
    let token = tokens.next()?;
    tokens.next().is_none().then(|| token.to_string())
}

/// The byte index of the `"` closing the JSON string that starts at the beginning of `text`,
/// or `None` when `text` does not start with a string or never closes it. An escaped `"` does
/// not close it.
fn selection_string_end(text: &str) -> Option<usize> {
    let mut characters = text.char_indices();
    if characters.next()?.1 != '"' {
        return None;
    }
    let mut escaped = false;
    for (index, character) in characters {
        if escaped {
            escaped = false;
        } else if character == '\\' {
            escaped = true;
        } else if character == '"' {
            return Some(index);
        }
    }
    None
}

/// The byte index of the `]` closing the list that starts at the beginning of `list`, or
/// `None` for a list that is never closed. A `]` inside a quoted value does not close it.
pub(super) fn selection_list_end(list: &str) -> Option<usize> {
    let mut in_string = false;
    let mut escaped = false;
    for (index, character) in list.char_indices() {
        if in_string {
            if escaped {
                escaped = false;
            } else if character == '\\' {
                escaped = true;
            } else if character == '"' {
                in_string = false;
            }
            continue;
        }
        match character {
            '"' => in_string = true,
            ']' => return Some(index),
            _ => {}
        }
    }
    None
}

/// Refuse a `selectionCriteria` that conflicts with the criteria of another account-level
/// policy of the same type, answering the message to refuse it with.
///
/// `PutAccountPolicy` states the rule per type:
///
/// * a transformer policy — "If you have multiple account-level transformer policies with
///   selection criteria, no two of them can use the same or overlapping log group name
///   prefixes" — and a field index policy, with the same sentence for its
///   `LogGroupNamePrefix` criteria plus "no two of them can use the same data source name
///   and type combination" for its data source criteria;
/// * a metric extraction policy — "no two of them can have overlapping criteria", "When
///   using NOT IN, only one policy with this operator is allowed per account", and two
///   policies with `IN` and `NOT IN` "do not overlap if and only if every value in the IN
///   policy is completely contained within some value in the NOT IN policy".
///
/// A policy is identified by its type and name, so the policy being updated is not
/// compared with itself.
pub fn check_account_policy_criteria_overlap(
    policies: &BTreeMap<(String, String), AccountPolicyRecord>,
    policy_type: &str,
    policy_name: &str,
    criteria: &SelectionCriteria,
) -> Result<(), String> {
    let Some(putting) = CriteriaSelection::of(criteria) else {
        return Ok(());
    };
    for held in policies.values() {
        if held.policy_type != policy_type || held.policy_name == policy_name {
            continue;
        }
        let Some(held_criteria) = held.selection_criteria.as_deref() else {
            continue;
        };
        // A stored criteria string went through the same grammar when it was put, so one
        // that does not parse selects nothing there is to conflict with.
        let Ok(held_criteria) = check_selection_criteria(policy_type, held_criteria) else {
            continue;
        };
        let Some(held_selection) = CriteriaSelection::of(&held_criteria) else {
            continue;
        };
        if criteria_conflict(policy_type, &putting, &held_selection) {
            return Err(overlap_message(policy_type, &putting, &held.policy_name));
        }
    }
    Ok(())
}

/// How CloudWatch Logs refuses a `selectionCriteria` that overlaps the criteria of an
/// account-level policy the account already holds: the sentence for the RULE that was broken.
///
/// The rules are the ones [`check_account_policy_criteria_overlap`] quotes, and each is
/// refused in its own words:
///
/// * a policy whose LOG GROUP NAME PREFIXES overlap another's breaks "no two of them can use
///   the same or overlapping log group name prefixes", the rule stated for the transformer
///   and field index policies, and is refused with the fixed sentence about that prefix
///   ([`SELECTION_CRITERIA_PREFIX_TAKEN_MESSAGE`]);
/// * an overlap of any other criteria — the metric extraction policy's, and the field index
///   policy's data source name and type combination — is refused by naming the policy it
///   overlaps ([`selection_criteria_overlap_message`]), which is what tells a caller holding
///   several policies which one to narrow.
fn overlap_message(
    policy_type: &str,
    putting: &CriteriaSelection<'_>,
    held_policy_name: &str,
) -> String {
    let by_prefix = matches!(policy_type, POLICY_TYPE_TRANSFORMER | POLICY_TYPE_FIELD_INDEX)
        && matches!(putting, CriteriaSelection::Groups { .. });
    if by_prefix {
        return SELECTION_CRITERIA_PREFIX_TAKEN_MESSAGE.to_string();
    }
    selection_criteria_overlap_message(held_policy_name)
}

/// The sentence CloudWatch Logs refuses a put whose log group name prefix another
/// account-level policy of the same type already covers with: fixed text for the class,
/// carrying nothing of the request — the prefix is the caller's own value and the policy that
/// holds it is not named.
pub const SELECTION_CRITERIA_PREFIX_TAKEN_MESSAGE: &str =
    "SelectionCriteria prefix is already supported by an existing policy";

/// How CloudWatch Logs refuses a `selectionCriteria` that overlaps the criteria of an
/// account-level policy the account already holds.
///
/// Built FROM the request and the conflicting resource: the refusal names the policy whose
/// criteria the put would overlap, so a caller holding several policies knows which one to
/// narrow without describing them all.
pub fn selection_criteria_overlap_message(held_policy_name: &str) -> String {
    format!(
        "SelectionCriteria is overlapping with an existing policy with policy name: \
         {held_policy_name}"
    )
}

/// Whether two criteria of one policy type conflict. See
/// [`check_account_policy_criteria_overlap`] for the sentence behind each answer.
pub(super) fn criteria_conflict(
    policy_type: &str,
    putting: &CriteriaSelection<'_>,
    held: &CriteriaSelection<'_>,
) -> bool {
    match (putting, held) {
        (
            CriteriaSelection::DataSource { names, types },
            CriteriaSelection::DataSource {
                names: held_names,
                types: held_types,
            },
        ) => {
            // "no two of them can use the same data source name and type combination"
            let same_name = names.iter().any(|name| held_names.contains(name));
            let same_type = types.iter().any(|source| held_types.contains(source));
            same_name && same_type
        }
        (
            CriteriaSelection::Groups { negated, selectors },
            CriteriaSelection::Groups {
                negated: held_negated,
                selectors: held_selectors,
            },
        ) => {
            if policy_type != POLICY_TYPE_METRIC_EXTRACTION {
                // Every other type's sentence is about the prefixes themselves: "no two of
                // them can use the same or overlapping log group name prefixes".
                return selectors
                    .iter()
                    .any(|one| held_selectors.iter().any(|two| one.intersects(*two)));
            }
            match (negated, held_negated) {
                // "When using NOT IN, only one policy with this operator is allowed per
                // account".
                (true, true) => true,
                // "no two of them can have overlapping criteria"
                (false, false) => selectors
                    .iter()
                    .any(|one| held_selectors.iter().any(|two| one.intersects(*two))),
                // One selects the log groups it lists and the other excludes the ones it
                // lists: they do not overlap only when every selected value is completely
                // contained within some excluded value.
                (false, true) => !selectors
                    .iter()
                    .all(|one| held_selectors.iter().any(|two| one.contained_in(*two))),
                (true, false) => !held_selectors
                    .iter()
                    .all(|one| selectors.iter().any(|two| one.contained_in(*two))),
            }
        }
        // One criteria selects log groups and the other a data source name and type: they
        // select along different dimensions, which the field index policy's quota sentences
        // count separately, so neither constrains the other.
        _ => false,
    }
}

/// Whether an account-level policy applies to one log group.
///
/// `PutAccountPolicy$selectionCriteria` is what narrows an account-level policy: "Use this
/// parameter to apply the new policy to a subset of log groups in the account", and a policy
/// put without one is the policy "that applies to all log groups in the account". So a
/// policy holding no criteria selects every group, and one holding criteria selects the
/// groups its filters name: a `LogGroupName` value names the group called that, a
/// `LogGroupNamePrefix` value every group whose name starts with it, and the
/// `DataSourceName` AND `DataSourceType` pair every group with a data source of that name
/// and type. A `NOT IN` list selects every group its values do NOT name, and criteria
/// written with more than one filter are joined by `AND`, so each of them must hold.
///
/// A stored criteria string went through [`check_selection_criteria`] when it was put, so
/// one that no longer parses names no log groups rather than all of them.
pub fn account_policy_selects_log_group(
    policy: &AccountPolicyRecord,
    group: &LogGroupRecord,
) -> bool {
    let Some(criteria) = policy.selection_criteria.as_deref() else {
        return true;
    };
    let Ok(criteria) = check_selection_criteria(&policy.policy_type, criteria) else {
        return false;
    };
    // The one documented form that joins two filters is the `DataSourceName` AND
    // `DataSourceType` pair, and it names ONE data source: a group satisfies that pair only
    // when a single data source of its own satisfies both of its filters. So the pair is ONE
    // conjunct of the criteria rather than two, and any further filter written beside it is
    // still a conjunct of its own.
    let pair = match (
        criteria.clause(SELECTION_FIELD_DATA_SOURCE_NAME),
        criteria.clause(SELECTION_FIELD_DATA_SOURCE_TYPE),
    ) {
        (Some(names), Some(types)) => Some((names, types)),
        _ => None,
    };
    if let Some((names, types)) = pair {
        if !data_source_pair_selects_log_group(names, types, group) {
            return false;
        }
    }
    criteria
        .clauses
        .iter()
        // The two filters of the pair are judged together above, so they are not judged
        // again one at a time here.
        .filter(|clause| pair.is_none() || !is_data_source_selection_field(&clause.field))
        .all(|clause| selection_clause_selects_log_group(clause, group))
}

/// The account-level policy of one type that applies to one log group, or `None` when none
/// of the account's does.
///
/// An account-level policy applies to a log group when its `selectionCriteria` select that
/// group, and a policy put without criteria is the one "that applies to all log groups in
/// the account" ([`account_policy_selects_log_group`]). A group can therefore be selected by
/// both a scoped policy and that account-wide one — `PutAccountPolicy`'s quota sentence
/// admits "either one account-level policy that applies to all log groups in the account, or
/// up to 20 policies that are each scoped to a subset", and its overlap rule ("no two of them
/// can use the same or overlapping log group name prefixes") leaves at most one SCOPED policy
/// selecting any one group. So the policy that applies to the group is the scoped one when it
/// has one, and the account-wide one otherwise.
pub fn applicable_account_policy<'a>(
    account_policies: &'a BTreeMap<(String, String), AccountPolicyRecord>,
    policy_type: &str,
    group: &LogGroupRecord,
) -> Option<&'a AccountPolicyRecord> {
    let applying: Vec<&AccountPolicyRecord> = account_policies
        .values()
        .filter(|policy| policy.policy_type == policy_type)
        .filter(|policy| account_policy_selects_log_group(policy, group))
        .collect();
    applying
        .iter()
        .find(|policy| policy.selection_criteria.is_some())
        .or_else(|| applying.first())
        .copied()
}

/// Whether a log group carries one data source satisfying BOTH filters of a
/// `DataSourceName` AND `DataSourceType` pair; see [`account_policy_selects_log_group`].
pub(super) fn data_source_pair_selects_log_group(
    names: &SelectionClause,
    types: &SelectionClause,
    group: &LogGroupRecord,
) -> bool {
    group.data_sources.iter().any(|source| {
        let named = names.values.contains(&source.name) != names.negated;
        let typed = source
            .source_type
            .as_ref()
            .is_some_and(|source_type| types.values.contains(source_type))
            != types.negated;
        named && typed
    })
}

/// Whether a `selectionCriteria` filter is one of the two that name a data source
/// ([`SELECTION_FIELD_DATA_SOURCE_NAME`], [`SELECTION_FIELD_DATA_SOURCE_TYPE`]).
pub(super) fn is_data_source_selection_field(field: &str) -> bool {
    matches!(
        field,
        SELECTION_FIELD_DATA_SOURCE_NAME | SELECTION_FIELD_DATA_SOURCE_TYPE
    )
}

/// Whether one filter of a `selectionCriteria` string selects a log group; see
/// [`account_policy_selects_log_group`] for what each field's values name. A filter on a
/// field that names no property of a log group selects none of them.
pub(super) fn selection_clause_selects_log_group(clause: &SelectionClause, group: &LogGroupRecord) -> bool {
    let named = match clause.field.as_str() {
        SELECTION_FIELD_LOG_GROUP_NAME => clause.values.contains(&group.name),
        SELECTION_FIELD_LOG_GROUP_NAME_PREFIX => clause
            .values
            .iter()
            .any(|value| group.name.starts_with(value)),
        SELECTION_FIELD_DATA_SOURCE_NAME => group
            .data_sources
            .iter()
            .any(|source| clause.values.contains(&source.name)),
        SELECTION_FIELD_DATA_SOURCE_TYPE => group.data_sources.iter().any(|source| {
            source
                .source_type
                .as_ref()
                .is_some_and(|source_type| clause.values.contains(source_type))
        }),
        _ => return false,
    };
    named != clause.negated
}

/// One log group's data protection policy — what `PutDataProtectionPolicy` stores — as
/// it is stored.
///
/// The document is kept VERBATIM, exactly the bytes the request carried: AWS reports it
/// back through `GetDataProtectionPolicy` unchanged, so re-serializing the parsed JSON
/// (which would reorder members and drop whitespace) would not be the same document.
#[derive(Debug, Clone)]
pub struct DataProtectionPolicyRecord {
    /// `policyDocument`, byte-for-byte as the put carried it.
    pub policy_document: String,
    /// "The date and time that this policy was most recently updated."
    pub last_updated_time_ms: i64,
}

/// One log group's own field index policy — the log-group level policy `PutIndexPolicy`
/// stores and `DeleteIndexPolicy` removes — as it is stored.
///
/// A log-group level policy is identified by nothing but the group it applies to ("Responses
/// about log group-level field index policies don't have this field, because those policies
/// don't have names"), so it lives ON the group and a later put replaces this record rather
/// than adding another. The document is kept VERBATIM, exactly the bytes the request
/// carried, the way [`DataProtectionPolicyRecord::policy_document`] is: the service reports
/// a stored policy document back unchanged.
#[derive(Debug, Clone)]
pub struct IndexPolicyRecord {
    /// `policyDocument`, byte-for-byte as the put carried it.
    pub policy_document: String,
    /// "The date and time that this index policy was most recently updated."
    pub last_update_time_ms: i64,
}

/// The account's storage tier policy — what `PutStorageTierPolicy` stores and
/// `GetStorageTierPolicy` reports — as it is stored. One per account: the policy is named
/// by nothing, so a later put replaces this record rather than adding another.
#[derive(Debug, Clone)]
pub struct StorageTierPolicyRecord {
    /// `storageTier`, one of [`crate::ops::types::STORAGE_TIER_VALUES`].
    pub storage_tier: String,
    /// "The time when the storage tier policy was last updated, expressed as the number of
    /// milliseconds after January 1, 1970 00:00:00 UTC" — the instant of the put that
    /// stored this tier.
    pub last_updated_time_ms: i64,
}

/// What a data protection policy document says, once it has been judged: the parts a
/// handler acts on after [`check_data_protection_policy_document`] accepts it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DataProtectionPolicyShape {
    /// The data identifiers both of the document's blocks list, in the order the Audit
    /// block lists them.
    pub data_identifiers: Vec<String>,
    /// The log groups the Audit action's `FindingsDestination` sends audit findings to
    /// (`FindingsDestination.CloudWatchLogs.LogGroup`). A destination this service does
    /// not own (a Firehose stream, an S3 bucket) is not here: only a log group lives in
    /// this service's state.
    pub findings_destination_log_groups: Vec<String>,
}

/// Judge a `PutDataProtectionPolicy$policyDocument`, answering the message to refuse it
/// with, or what the accepted document says.
///
/// Every rule here is one that member's own documentation states:
///
/// * "Specify the data protection policy, in JSON" — and "This policy must include two
///   JSON blocks", so the document is a JSON object carrying a `Statement` array;
/// * "The first block must include both a `DataIdentifer` array and an `Operation`
///   property with an `Audit` action", and "This `Audit` action must contain a
///   `FindingsDestination` object";
/// * "The second block must include both a `DataIdentifer` array and an `Operation`
///   property with an `Deidentify` action", which "must contain the `"MaskConfig": {}`
///   object. The `"MaskConfig": {}` object must be empty";
/// * "The contents of the two `DataIdentifer` arrays must match exactly";
/// * "The JSON specified in `policyDocument` can be up to 30,720 characters."
///
/// The documentation spells the member `DataIdentifer` while the API's own policy
/// documents spell it `DataIdentifier`, so both spellings are read.
///
/// Which data identifiers exist, and whether a Firehose stream or S3 bucket named as a
/// findings destination exists, are NOT judged here: that catalogue and those resources
/// live outside this service.
pub fn check_data_protection_policy_document(
    document: &str,
) -> Result<DataProtectionPolicyShape, String> {
    if document.chars().count() > MAX_DATA_PROTECTION_POLICY_DOCUMENT_CHARS {
        return Err(format!(
            "policyDocument can be up to {MAX_DATA_PROTECTION_POLICY_DOCUMENT_CHARS} characters"
        ));
    }
    let parsed = serde_json::from_str::<serde_json::Value>(document)
        .ok()
        .filter(serde_json::Value::is_object)
        .ok_or_else(|| {
            // A document that is not JSON at all never reaches the rules below: the service
            // answers its own JSON parser's failure, naming the token it could not read and
            // where that token is ([`unreadable_policy_json_message`]).
            unreadable_policy_json_message(document).unwrap_or_else(|| {
                "policyDocument must be a data protection policy in JSON (a JSON object)"
                    .to_string()
            })
        })?;
    let statements = parsed
        .get("Statement")
        .and_then(serde_json::Value::as_array)
        .filter(|statements| !statements.is_empty())
        .ok_or_else(|| {
            "policyDocument must include a Statement array of the policy's two blocks".to_string()
        })?;

    // One block per action: the Audit block that finds the sensitive data terms, and the
    // Deidentify block that masks them.
    let block = |action: &str| {
        statements.iter().find(|statement| {
            statement
                .get("Operation")
                .and_then(|operation| operation.get(action))
                .is_some_and(serde_json::Value::is_object)
        })
    };
    let audit = block("Audit").ok_or_else(|| {
        "policyDocument must include a Statement block whose Operation has an Audit action"
            .to_string()
    })?;
    let deidentify = block("Deidentify").ok_or_else(|| {
        "policyDocument must include a Statement block whose Operation has a Deidentify action"
            .to_string()
    })?;

    let findings_destination = audit
        .get("Operation")
        .and_then(|operation| operation.get("Audit"))
        .and_then(|action| action.get("FindingsDestination"))
        .filter(|destination| destination.is_object())
        .ok_or_else(|| {
            "the Audit action in policyDocument must contain a FindingsDestination object"
                .to_string()
        })?;
    let mask_config = deidentify
        .get("Operation")
        .and_then(|operation| operation.get("Deidentify"))
        .and_then(|action| action.get("MaskConfig"))
        .and_then(serde_json::Value::as_object)
        .ok_or_else(|| {
            "the Deidentify action in policyDocument must contain a MaskConfig object".to_string()
        })?;
    if !mask_config.is_empty() {
        return Err("the MaskConfig object in policyDocument must be empty".to_string());
    }

    let identifiers = |statement: &serde_json::Value| -> Option<Vec<String>> {
        let listed = statement
            .get("DataIdentifier")
            .or_else(|| statement.get("DataIdentifer"))?
            .as_array()?;
        if listed.is_empty() {
            return None;
        }
        listed
            .iter()
            .map(|identifier| identifier.as_str().map(str::to_string))
            .collect()
    };
    let audit_identifiers = identifiers(audit).ok_or_else(|| {
        "the Audit block in policyDocument must include a DataIdentifier array of data identifiers"
            .to_string()
    })?;
    let deidentify_identifiers = identifiers(deidentify).ok_or_else(|| {
        "the Deidentify block in policyDocument must include a DataIdentifier array of data identifiers"
            .to_string()
    })?;
    // "must match exactly" is about the CONTENTS of the two arrays, so the order they are
    // listed in is not part of the match.
    let sorted = |mut identifiers: Vec<String>| {
        identifiers.sort();
        identifiers
    };
    if sorted(audit_identifiers.clone()) != sorted(deidentify_identifiers) {
        return Err(
            "the contents of the two DataIdentifier arrays in policyDocument must match exactly"
                .to_string(),
        );
    }

    Ok(DataProtectionPolicyShape {
        data_identifiers: audit_identifiers,
        findings_destination_log_groups: findings_destination
            .get("CloudWatchLogs")
            .and_then(|destination| destination.get("LogGroup"))
            .and_then(serde_json::Value::as_str)
            .map(|log_group| vec![log_group.to_string()])
            .unwrap_or_default(),
    })
}
