// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-agentregistry-agentregistry service.
//!
//! The Agent Registry model this crate serves binds only the three *discovery read*
//! operations (`ListDiscoverableRegistryRecords`,
//! `BatchGetDiscoverableRegistryRecord`, `SearchDiscoverableRegistryRecords`);
//! registry and record authoring lives in the Agent Registry CONTROL plane's model.
//! So this service holds no registry of its own: a discovery read resolves the
//! registry it was given in the store that OWNS it ([`owner`]) and projects it into
//! the shapes this model publishes. The types below are that projection, plus the
//! refusals and identifier readings every discovery read shares.

/// Whether IAM lets a request perform the action it asks for: the decision every
/// discovery read takes before it resolves the registry the request names.
pub mod authorization;

/// The store a discovery read reports: the one the Agent Registry control plane owns.
pub mod owner;

use std::collections::BTreeMap;

use crate::ops::types::{
    Descriptors, DiscoverableRegistryRecordSummary, RegistryRecordSummary,
    ResourceNotFoundException, ValidationException, ValidationExceptionField,
};
use ::aws_smithy_types::{DateTime, Document};

/// The region every ARN this service mints is scoped to.
///
/// Registry records are region-scoped resources, and nothing in a discovery read
/// names a region, so the store records the one it was seeded in.
pub const DEFAULT_REGION: &str = "us-east-1";

/// This service's ARN namespace (`arnNamespace` / sigv4 signing name in the model).
pub const ARN_SERVICE: &str = "agent-registry";

/// The only `RegistryRecordStatus` that is discoverable.
///
/// From the model's own `RegistryRecordStatus` doc: a record is "`APPROVED` once it
/// is approved and discoverable", while "`REJECTED` and `DEPRECATED` records are not
/// discoverable" and `DRAFT` / `PENDING_APPROVAL` / `CREATING` / `UPDATING` /
/// `CREATE_FAILED` / `UPDATE_FAILED` describe a record that has not become
/// discoverable yet.
pub const STATUS_APPROVED: &str = "APPROVED";

/// Every descriptor entry key this model has, in the order the `Descriptors`
/// structure declares its members.
///
/// The keys are that structure's member names — the values
/// `DiscoverableRegistryRecordSummary$descriptorTypes` carries ("Each value
/// corresponds to a descriptor entry key on the approved record") — so this is both
/// what a record's own key list is drawn from ([`descriptor_types`]) and the set of
/// values a `descriptorType` filter may name.
pub const DESCRIPTOR_TYPE_KEYS: [&str; 6] = [
    "mcpServer",
    "a2aAgentCard",
    "agentSkillsDefinition",
    "custom",
    "http",
    "agui",
];

/// The descriptor entry keys present on `descriptors`, derived from the structure
/// itself rather than stored a second time.
///
/// `Descriptors` is destructured exhaustively and the presence flags are paired
/// positionally with [`DESCRIPTOR_TYPE_KEYS`], so a member added to the model fails
/// to compile here until its entry key is named in that one list.
pub fn descriptor_types(descriptors: &Descriptors) -> Vec<String> {
    let Descriptors {
        mcp_server,
        a2a_agent_card,
        agent_skills_definition,
        custom,
        http,
        agui,
    } = descriptors;
    let present: [bool; DESCRIPTOR_TYPE_KEYS.len()] = [
        mcp_server.is_some(),
        a2a_agent_card.is_some(),
        agent_skills_definition.is_some(),
        custom.is_some(),
        http.is_some(),
        agui.is_some(),
    ];
    DESCRIPTOR_TYPE_KEYS
        .iter()
        .zip(present)
        .filter(|(_, present)| *present)
        .map(|(key, _)| (*key).to_string())
        .collect()
}

/// The registry id inside a `RegistryIdentifier`, which the model lets a caller
/// supply either as a bare id or as the registry's full ARN
/// (`arn:aws:agent-registry:<region>:<account>:registry/<id>`).
pub fn registry_id_from_identifier(identifier: &str) -> &str {
    match identifier.rsplit_once(":registry/") {
        Some((_, registry_id)) => registry_id,
        None => identifier,
    }
}

/// `ValidationExceptionReason$FieldValidationFailed`, one of the model's own
/// `VALIDATION_EXCEPTION_REASON_VALUES`.
pub const REASON_FIELD_VALIDATION_FAILED: &str = "FieldValidationFailed";

/// `ValidationExceptionReason$CannotParse`, one of the model's own
/// `VALIDATION_EXCEPTION_REASON_VALUES`.
pub const REASON_CANNOT_PARSE: &str = "CannotParse";

/// The modelled `ValidationException` naming the member that failed and why.
///
/// Shared by every operation so one refusal is worded one way; each operation
/// lifts it into its own error channel with `.into()`.
pub fn field_validation(member: &str, message: &str) -> ValidationException {
    ValidationException {
        message: format!("{member}: {message}"),
        reason: REASON_FIELD_VALIDATION_FAILED.to_string(),
        field_list: Some(vec![ValidationExceptionField {
            name: member.to_string(),
            message: message.to_string(),
        }]),
    }
}

/// The request member a paginated read's page size arrives in, named by the refusal
/// [`page_size`] answers for a value outside the documented range.
const MAX_RESULTS_MEMBER: &str = "maxResults";

/// The documented bounds ONE operation puts on its page size: the `@range` its model
/// declares, and the size it serves a request that names none.
///
/// The three numbers stay where they are documented — beside each operation, against
/// that operation's own model — and this is only the shape they are handed to
/// [`page_size`] in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PageSizeBounds {
    /// The smallest page the operation admits (`@range{min}`).
    pub min: usize,
    /// The largest page the operation admits (`@range{max}`).
    pub max: usize,
    /// The page served for a request that names no page size.
    pub default: usize,
}

/// The number of records a paginated read serves for the page size it was asked for:
/// the documented default when the member is absent, the requested value when the
/// documented range admits it, and otherwise the modelled `ValidationException`
/// naming `maxResults` and stating that range.
///
/// One rule for every paginated read of this service, so two reads cannot drift in
/// what they default to, in what they refuse or in how they word the refusal; each
/// read passes its own [`PageSizeBounds`], so each states its own numbers in that one
/// sentence. The requested value is narrowed to a record count here as well: a page
/// size that is no count at all is outside the range for the same reason a value past
/// the maximum is, so there is one branch rather than a second, unreachable one that
/// could answer differently.
pub fn page_size(
    requested: Option<i32>,
    bounds: PageSizeBounds,
) -> Result<usize, ValidationException> {
    let PageSizeBounds { min, max, default } = bounds;
    let Some(requested) = requested else {
        return Ok(default);
    };
    match usize::try_from(requested) {
        Ok(page) if (min..=max).contains(&page) => Ok(page),
        _ => Err(field_validation(
            MAX_RESULTS_MEMBER,
            &format!("{requested} is outside the valid range {min} through {max}."),
        )),
    }
}

/// The modelled `ValidationException` whose sentence the service words itself,
/// rather than the member-and-constraint sentence [`field_validation`] builds: a
/// refusal about a request value the service alone decides, stated verbatim.
///
/// The `reason` is `FieldValidationFailed`, the member the model defaults it to.
pub fn validation_failure(message: &str) -> ValidationException {
    ValidationException {
        message: message.to_string(),
        reason: REASON_FIELD_VALIDATION_FAILED.to_string(),
        field_list: None,
    }
}

/// The fixed text of the refusal a discovery read answers for a registry this
/// service does not hold. The identifier follows it and is the only variable part.
const REGISTRY_NOT_FOUND: &str = "Registry not found: ";

/// The modelled `ResourceNotFoundException` a discovery read answers when the
/// `RegistryIdentifier` it was given names no registry this store holds.
///
/// Shared by every operation that resolves a registry, so one refusal is worded
/// one way. The identifier is stated exactly as the request supplied it — a bare
/// registry id or the registry's full ARN — never the id parsed back out of it,
/// so a caller reads its own value back.
pub fn registry_not_found(identifier: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(format!("{REGISTRY_NOT_FOUND}{identifier}")),
    }
}

/// The modelled `ValidationException` for a request value this service cannot
/// parse at all (an opaque token it did not mint, say).
pub fn cannot_parse(message: &str) -> ValidationException {
    ValidationException {
        message: message.to_string(),
        reason: REASON_CANNOT_PARSE.to_string(),
        field_list: None,
    }
}

/// The registry and record a `RecordIdentifier` names: a record ARN carries its
/// own `registry/<id>` segment, while a bare record id carries none.
pub fn record_identifier_parts(identifier: &str) -> (Option<&str>, &str) {
    match identifier.split_once("/record/") {
        Some((prefix, record_id)) => (Some(registry_id_from_identifier(prefix)), record_id),
        None => (None, identifier),
    }
}

/// The record id inside a `RecordIdentifier`, which the model lets a caller supply
/// either as a bare id or as the record's full ARN
/// (`arn:aws:agent-registry:<region>:<account>:registry/<id>/record/<recordId>`).
pub fn record_id_from_identifier(identifier: &str) -> &str {
    record_identifier_parts(identifier).1
}

/// One registry record, as a discovery read sees it: the members this model publishes,
/// projected from the record the control plane holds ([`owner`]).
#[derive(Debug, Clone)]
pub struct RegistryRecord {
    pub record_id: String,
    /// Minted by the control plane when the record was created, so every operation of
    /// either plane reports the same ARN for the same record.
    pub record_arn: String,
    /// The owning registry's ARN, minted the same way.
    pub registry_arn: String,
    pub name: String,
    pub display_name: Option<String>,
    pub description: Option<String>,
    pub record_type: String,
    pub record_version: String,
    pub status: String,
    pub descriptors: Descriptors,
    pub custom_metadata: Option<Document>,
    pub created_at: DateTime,
    pub updated_at: DateTime,
}

impl RegistryRecord {
    /// Whether a discovery read may return this record at all.
    pub fn is_discoverable(&self) -> bool {
        self.status == STATUS_APPROVED
    }

    /// The descriptor entry keys present on this record.
    pub fn descriptor_types(&self) -> Vec<String> {
        descriptor_types(&self.descriptors)
    }

    /// The full `RegistryRecordSummary` — what `BatchGetDiscoverableRegistryRecord`
    /// and `SearchDiscoverableRegistryRecords` return. Both go through this one
    /// conversion, so the same record renders identically in either response.
    pub fn summary(&self) -> RegistryRecordSummary {
        RegistryRecordSummary {
            registry_arn: self.registry_arn.clone(),
            record_arn: self.record_arn.clone(),
            record_id: self.record_id.clone(),
            name: self.name.clone(),
            description: self.description.clone(),
            display_name: self.display_name.clone(),
            record_type: self.record_type.clone(),
            descriptors: self.descriptors.clone(),
            record_version: self.record_version.clone(),
            status: self.status.clone(),
            created_at: self.created_at,
            updated_at: self.updated_at,
            custom_metadata: self.custom_metadata.clone(),
        }
    }

    /// The list summary — `DiscoverableRegistryRecordSummary` carries no
    /// `descriptors` (the model's doc: "This summary does not include
    /// descriptors") but does carry their entry keys.
    pub fn discoverable_summary(&self) -> DiscoverableRegistryRecordSummary {
        DiscoverableRegistryRecordSummary {
            registry_arn: self.registry_arn.clone(),
            record_arn: self.record_arn.clone(),
            record_id: self.record_id.clone(),
            name: self.name.clone(),
            description: self.description.clone(),
            display_name: self.display_name.clone(),
            record_type: self.record_type.clone(),
            record_version: self.record_version.clone(),
            status: self.status.clone(),
            created_at: self.created_at,
            updated_at: self.updated_at,
            descriptor_types: Some(self.descriptor_types()),
        }
    }
}

/// One registry and the records it owns, keyed by record id so iteration order is
/// stable and a pagination token can resume from a record id.
#[derive(Debug, Clone)]
pub struct Registry {
    pub registry_id: String,
    pub registry_arn: String,
    pub records: BTreeMap<String, RegistryRecord>,
}

impl Registry {
    /// This registry's discoverable records, in record-id order.
    pub fn discoverable_records(&self) -> impl Iterator<Item = &RegistryRecord> {
        self.records
            .values()
            .filter(|record| record.is_discoverable())
    }

    /// A discoverable record by id; `None` for an unknown id and for one whose
    /// status keeps it out of the discovery surface.
    pub fn discoverable_record(&self, record_id: &str) -> Option<&RegistryRecord> {
        self.records
            .get(record_id)
            .filter(|record| record.is_discoverable())
    }
}

/// Per-service state for com-amazonaws-agentregistry-agentregistry: NOTHING.
///
/// This service's model binds three discovery READS and no operation that creates,
/// updates or deletes anything, so it owns no resource and keeps no store. The
/// registries and records its reads report belong to the Agent Registry control plane
/// and are read out of that service's own store, through [`owner::registry`] — the one
/// place this crate resolves a registry, so no read of this service can see a registry
/// the control plane does not hold, and nothing it reports is a second copy of one.
///
/// The type is kept because the generated crate root re-exports it: it is the engine's
/// per-service state slot for this service, and this service's slot is empty.
#[derive(Debug, Default)]
pub struct ComAmazonawsAgentregistryAgentregistryState;

impl ComAmazonawsAgentregistryAgentregistryState {
    /// The account every ARN of this service carries. There is exactly one source of
    /// truth for it, in the vendored runtime.
    pub fn account_id(&self) -> &'static str {
        ::engine::emulator_account_id()
    }
}

/// This service's (empty) state slot in the shared engine: call it with a handler's
/// `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine. A discovery
/// read does not go through here — it resolves the registry it was given in the store
/// that owns it, [`owner::registry`].
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsAgentregistryAgentregistryState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsAgentregistryAgentregistryState>)
}
