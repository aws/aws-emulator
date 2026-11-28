// SPDX-License-Identifier: CC0-1.0

//! The refusal payloads, split out of [`crate::state`] unchanged: one builder per
//! condition this service refuses a request for, so every operation that meets a
//! condition states it the same way.

use super::*;

/// The region this request was signed for — what a domain's ARN and endpoint name.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// A `SystemTime` as the modelled `UpdateTimestamp` type.
pub fn datetime(at: SystemTime) -> ::aws_smithy_types::DateTime {
    ::aws_smithy_types::DateTime::from(at)
}

/// A `SystemTime` as the TEXT a member modelled a string carries an instant as:
/// date-time, which is how this service's string-typed timestamps
/// (`DryRunProgressStatus$CreationDate`, `$UpdateDate`) are written.
pub fn datetime_text(at: SystemTime) -> String {
    datetime(at)
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
        .unwrap_or_default()
}

/// What this service says of a name that names no stored domain, in whichever
/// envelope the operation refusing it answers with: the ONE wording, so two
/// operations cannot word the same miss differently.
pub fn domain_not_found_message(domain_name: &str) -> String {
    format!("Domain not found: {domain_name}")
}

/// The `ResourceNotFoundException` payload for a name that names no stored domain.
pub fn domain_not_found(domain_name: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(domain_not_found_message(domain_name)),
    }
}

/// The `BaseException` payload for a name that names no stored domain — the generic
/// envelope (HTTP 400), for an operation that refuses an unknown domain with it
/// rather than with the not-found one.
pub fn domain_not_found_base(domain_name: &str) -> BaseException {
    BaseException {
        message: Some(domain_not_found_message(domain_name)),
    }
}

/// The `ValidationException` payload for the same miss, for an operation that refuses a
/// domain it cannot resolve as a bad request rather than as a missing resource — the class
/// `AddReplicationPeer` refuses both the domain it is addressed to and the peer it is
/// handed under.
pub fn domain_not_found_validation(domain_name: &str) -> ValidationException {
    ValidationException {
        message: Some(domain_not_found_message(domain_name)),
    }
}

/// How this service states that there is no configuration-change progress to report:
/// one fixed sentence with no variable part — it names neither the domain nor a change
/// id, because what it reports is that the service holds no progress information for
/// the request at all.
pub const NO_PROGRESS_INFORMATION: &str = "No progress information found";

/// The `BaseException` payload for a progress read that resolves no domain to report on
/// — the generic envelope (HTTP 400) this operation models beside its not-found error,
/// carrying [`NO_PROGRESS_INFORMATION`].
pub fn no_progress_information() -> BaseException {
    BaseException {
        message: Some(NO_PROGRESS_INFORMATION.to_string()),
    }
}

/// The `ResourceNotFoundException` payload for an id that addresses no cross-cluster
/// connection the caller has a view of — one this service never minted, one since
/// released, or one the caller's side of the connection has nothing to see yet.
///
/// The id the request carried is what fills the message, so a caller is told which of
/// several connections it named could not be resolved.
pub fn connection_not_found(connection_id: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(format!("Connection ID {connection_id} does not exist")),
    }
}

/// The `DisabledOperationException` payload for a decision on a connection that is
/// not waiting for one.
///
/// Accepting or rejecting an inbound connection request is available only while the
/// request waits for the destination domain owner's decision; past that the
/// connection is somewhere its lifecycle does not come back from, so the call is not
/// an operation that connection offers. The refusal names the status the connection
/// is actually in, so a caller can tell a decision already taken from one the
/// connection never waited for.
pub fn connection_decision_unavailable(
    connection_id: &str,
    status: &str,
) -> DisabledOperationException {
    DisabledOperationException {
        message: Some(format!(
            "Connection {connection_id} is in {status} state, not {CONNECTION_STATUS_PENDING_ACCEPTANCE}"
        )),
    }
}

/// The `ValidationException` payload for a change asked of a cross-cluster connection
/// whose current status does not allow it.
///
/// A connection the destination owner refused is that owner's record OF the refusal,
/// and a record of a decision is not something a later call takes back: the refusal
/// names the status the connection is in, because that status is the whole reason the
/// change cannot be made.
pub fn connection_change_unavailable(status: &str) -> ValidationException {
    ValidationException {
        message: Some(format!(
            "existing status is {status}, unable to make necessary change"
        )),
    }
}

/// The `ValidationException` payload for a name that names no data source of a
/// domain the store DOES hold.
///
/// Resolving a data source name against an existing domain is input validation, not
/// a missing-resource lookup: the refusal names the value that could not be
/// resolved, so a caller tells it apart from [`domain_not_found`].
pub fn data_source_absent(name: &str) -> ValidationException {
    ValidationException {
        message: Some(format!("DataSource with name {name} doesn't exist.")),
    }
}

/// The `ValidationException` payload for a create naming a data source the domain
/// already holds — the mirror of [`data_source_absent`], so one refusal reads like
/// the other.
pub fn data_source_exists(name: &str) -> ValidationException {
    ValidationException {
        message: Some(format!("DataSource with name {name} already exists.")),
    }
}

/// The `ResourceNotFoundException` payload for a name that names no index of a
/// domain the store DOES hold.
///
/// An index is a resource of its own — `CreateIndex` models
/// `ResourceAlreadyExistsException` for one the domain already has — so a call
/// naming one that does not exist is the modelled missing-resource refusal. The
/// wording the service uses for a missing index is fixed and names the `IndexName`
/// from the request only, so every index operation reports it identically.
pub fn index_not_found(index_name: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(format!("Resource index {index_name} does not exist")),
    }
}

/// The `ResourceAlreadyExistsException` payload for a create naming an index the
/// domain already holds, naming both the domain and the index so a caller can place
/// the collision.
pub fn index_exists(domain_name: &str, index_name: &str) -> ResourceAlreadyExistsException {
    ResourceAlreadyExistsException {
        message: Some(format!(
            "Index {index_name} already exists in domain {domain_name}."
        )),
    }
}

/// The `ValidationException` payload for a member that must be an `IndexName` and
/// is not ([`is_index_name`]).
///
/// Stated as the constraint report every other value outside its member's declared
/// expression is refused with ([`crate::service::validation::constraint_report`]): the
/// request boundary enforces `IndexName`'s `@length` but not its `@pattern` — the
/// negative lookahead is not a construct its regex engine compiles — so this is the
/// SAME constraint reported one step later, and a caller cannot tell the two apart.
/// `member` is the member's name as the model spells it.
pub fn malformed_index_name(member: &str, value: &str) -> ValidationException {
    ValidationException {
        message: Some(crate::service::validation::constraint_report(
            member,
            value,
            ::aws_smithy_schema_server::ViolationKind::Pattern(INDEX_NAME_PATTERN.to_owned()),
        )),
    }
}

/// The `ValidationException` payload for a MAP of log-publishing options whose KEY is not
/// a log type this surface admits.
///
/// The map's key shape is an enumerated type, so this is a constraint the MODEL declares —
/// but the validating walk treats a map's key as a path SEGMENT rather than as a
/// constrained leaf, so no walk ahead of a handler reports it and the handler states it.
/// Being an INPUT constraint it is applied before the domain the call names is resolved: a
/// key outside the set is refused by naming the member, never reported as a domain that
/// does not exist.
///
/// MEMBERSHIP is [`admitted_log_types`] — the generated `LOG_TYPE_VALUES` plus the one
/// value the live surface takes that this model has not caught up with — and the ORDER the
/// set is stated in is the order the front door's own hash table over those values is
/// walked ([`::service_common::validation_message::hashed_enum_order`]), which is the order
/// a recorded refusal of this map's keys lists them in. ONE validation error for the member
/// however many of its keys are outside the set: the clause is about the member's value,
/// and that value is the one map.
pub fn log_type_key_refusal(
    options: Option<&std::collections::HashMap<String, LogPublishingOption>>,
) -> Option<ValidationException> {
    let options = options?;
    let admitted = admitted_log_types();
    if options
        .keys()
        .all(|log_type| admitted.contains(&log_type.as_str()))
    {
        return None;
    }
    let stated: Vec<String> = ::service_common::validation_message::hashed_enum_order(&admitted)
        .into_iter()
        .map(str::to_string)
        .collect();
    Some(ValidationException {
        message: Some(crate::service::validation::map_key_constraint_report(
            "LogPublishingOptions",
            rendered_log_publishing_options(options).as_deref(),
            ::aws_smithy_schema_server::ViolationKind::EnumValue(stated),
        )),
    })
}

/// The log type the LIVE surface admits that this model's `LogType` enum does not declare.
///
/// EVIDENCE: the refusal this service is recorded answering an out-of-enum log type with
/// lists this value beside the four the model declares, while the legacy 2015-01-01
/// surface's recorded refusal lists only those four — so the two surfaces publish
/// different sets, and this is the one value the model here has not caught up with. The
/// same reading `crate::service::validation`'s own table of stated requirements applies to
/// a member whose validation surface the model lags.
///
/// It decides BOTH halves deliberately: the set the refusal STATES, and what a caller may
/// SEND — a service that publishes a value as admissible accepts it, so refusing it while
/// advertising it would state one set and enforce another. DROP this constant once the
/// model declares the value: `admitted_log_types` then adds nothing, which is exactly what
/// it checks before pushing.
const LIVE_LOG_TYPE: &str = "TASK_DETAILS_LOGS";

/// Every log type this surface admits: the ones the model declares, plus
/// [`LIVE_LOG_TYPE`] where the model does not declare it yet.
///
/// Read off the generated constant rather than listed, so a value the model gains is
/// admitted by the model alone; the extra value is added once and only while it is missing.
fn admitted_log_types() -> Vec<&'static str> {
    let mut admitted: Vec<&'static str> = crate::ops::LOG_TYPE_VALUES.to_vec();
    if !admitted.contains(&LIVE_LOG_TYPE) {
        admitted.push(LIVE_LOG_TYPE);
    }
    admitted
}

/// `options` as a refusal quotes it back: the map form AWS echoes a map-valued member in
/// ([`::service_common::validation::rendered_map_in_key_order`], the fleet's one renderer of
/// that form, which answers `None` for a map larger than a clause quotes), each value the
/// printed form of the `LogPublishingOption` it carried.
fn rendered_log_publishing_options(
    options: &std::collections::HashMap<String, LogPublishingOption>,
) -> Option<String> {
    let entries: Vec<(&str, String)> = options
        .iter()
        .map(|(log_type, option)| (log_type.as_str(), rendered_log_publishing_option(option)))
        .collect();
    ::service_common::validation::rendered_map_in_key_order(&entries)
}

/// One `LogPublishingOption` as the validation layer in front of the service prints a
/// modelled structure ([`::service_common::validation::rendered_record`]): the shape's name
/// over EVERY member the shape declares, in declaration order, with one the request left
/// unset stated as the fleet's `UNSET_MEMBER`.
fn rendered_log_publishing_option(option: &LogPublishingOption) -> String {
    let arn = option
        .cloud_watch_logs_log_group_arn
        .as_deref()
        .unwrap_or(::service_common::validation::UNSET_MEMBER);
    let enabled = option.enabled.map_or_else(
        || ::service_common::validation::UNSET_MEMBER.to_string(),
        |enabled| enabled.to_string(),
    );
    let arn_member =
        ::aws_smithy_schema_server::constraints::path::member("CloudWatchLogsLogGroupArn");
    let enabled_member = ::aws_smithy_schema_server::constraints::path::member("Enabled");
    ::service_common::validation::rendered_record(
        "LogPublishingOption",
        &[
            (arn_member.as_str(), arn),
            (enabled_member.as_str(), enabled.as_str()),
        ],
    )
}

/// The `ValidationException` payload for an `IndexSchema` that is not a JSON
/// object.
///
/// `IndexSchema` is a document "defining index mappings, settings, and semantic
/// enrichment configuration" — a set of named sections — so a document that carries
/// a scalar or an array instead is a malformed request rather than a schema with no
/// sections.
pub fn malformed_index_schema(member: &str) -> ValidationException {
    ValidationException {
        message: Some(format!(
            "{member} must be a JSON object of index schema sections."
        )),
    }
}

/// The `ValidationException` payload for an update that would re-type a field the
/// index already maps ([`mapping_type_conflict`]).
///
/// It names the field and both types, the way the search engine whose mapping the
/// update would contradict reports a mapping it cannot merge, so a caller can tell
/// which field of a many-field update was refused and why.
pub fn mapping_type_immutable(conflict: &MappingTypeConflict) -> ValidationException {
    ValidationException {
        message: Some(format!(
            "mapper [{}] cannot be changed from type [{}] to [{}]",
            conflict.field, conflict.from, conflict.to
        )),
    }
}

/// The widths of a `GUID`'s five hexadecimal groups.
pub(super) const GUID_GROUP_WIDTHS: [usize; 5] = [8, 4, 4, 4, 12];

/// `GUID`'s own `@pattern`, as the model declares it
/// (`smithy_query(query="shape",shape="GUID")`): the five hexadecimal groups
/// [`GUID_GROUP_WIDTHS`] states, as the expression a refusal of a malformed one quotes.
pub(super) const GUID_PATTERN: &str =
    r"^\p{XDigit}{8}-\p{XDigit}{4}-\p{XDigit}{4}-\p{XDigit}{4}-\p{XDigit}{12}$";

/// Whether `value` is a `GUID`: `@length(min: 36, max: 36)` with pattern
/// `^\p{XDigit}{8}-\p{XDigit}{4}-\p{XDigit}{4}-\p{XDigit}{4}-\p{XDigit}{12}$`.
///
/// The request boundary enforces the length bound but not the pattern, so an
/// operation that takes a `GUID` checks the shape itself rather than treating a
/// malformed id as an id that merely names nothing. It is a shape test rather than a
/// parse: `Uuid::parse_str` would accept the simple and URN forms this pattern
/// forbids.
pub fn is_guid(value: &str) -> bool {
    let groups: Vec<&str> = value.split('-').collect();
    groups.len() == GUID_GROUP_WIDTHS.len()
        && groups.iter().zip(GUID_GROUP_WIDTHS).all(|(group, width)| {
            group.len() == width && group.bytes().all(|byte| byte.is_ascii_hexdigit())
        })
}

/// The `ResourceNotFoundException` payload for a change id a domain's history holds
/// no change for, and for a domain that has no configuration change to report.
pub fn change_not_found(domain_name: &str, change_id: Option<&str>) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(match change_id {
            Some(change_id) => {
                format!("Change not found for domain {domain_name}: {change_id}")
            }
            None => format!("No configuration change found for domain {domain_name}"),
        }),
    }
}

/// The `ResourceNotFoundException` payload for a `DryRunId` a domain's history holds
/// no dry run for, and for a domain on which no dry run was ever started.
///
/// A request that NAMES an id is refused with the service's fixed sentence for an id
/// that addresses no dry run, carrying the id the request asked for and nothing else —
/// which is also what a caller sees for a run a later change to the domain superseded.
/// A request that names none asks about the domain's latest run, so a domain that has
/// no run at all is worded the way this service words every other miss of a record held
/// ON a domain (a configuration change, a maintenance action).
pub fn dry_run_not_found(domain_name: &str, dry_run_id: Option<&str>) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(match dry_run_id {
            Some(dry_run_id) => {
                format!("No Dry Run found for the provided DryRunId: {dry_run_id}")
            }
            None => format!("No dry run found for domain {domain_name}"),
        }),
    }
}

/// The `ValidationException` payload for a member that must be a `GUID` and is not.
///
/// Stated as the constraint report every other value outside its member's declared
/// expression is refused with ([`crate::service::validation::constraint_report`]): the
/// request boundary enforces `GUID`'s `@length` but not its `@pattern`, so this is the
/// SAME constraint reported one step later, and a caller cannot tell the two apart.
/// `member` is the member's name as the model spells it.
pub fn malformed_guid(member: &str, value: &str) -> ValidationException {
    ValidationException {
        message: Some(crate::service::validation::constraint_report(
            member,
            value,
            ::aws_smithy_schema_server::ViolationKind::Pattern(GUID_PATTERN.to_owned()),
        )),
    }
}

/// The `ValidationException` payload for an integer member below the minimum its
/// modelled `@range` sets, in the envelope AWS states a constraint violation in: the
/// value the request carried, the member it was carried at as the wire names it, and
/// the bound it broke.
///
/// Stated through [`crate::service::validation::constraint_report`], the one builder of
/// that envelope, so a bound a handler checks reads like one the boundary checked.
pub fn below_range_minimum(member: &str, value: i32, minimum: i32) -> ValidationException {
    ValidationException {
        message: Some(crate::service::validation::constraint_report(
            member,
            &value.to_string(),
            ::aws_smithy_schema_server::ViolationKind::MinValue(minimum.to_string()),
        )),
    }
}

/// The `ResourceNotFoundException` payload for a domain that has no pending
/// configuration change for a cancel to cancel.
pub fn no_pending_change(domain_name: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(format!(
            "No pending configuration change to cancel for domain {domain_name}"
        )),
    }
}

/// The `ResourceNotFoundException` payload for a `NodeId` that names no data node of
/// the domain a maintenance action was asked for on, or one whose role cannot lose a
/// node ([`MAINTAINABLE_DATA_NODES`]) — the node is then not one this service can
/// maintain, which is what makes the id invalid.
pub fn invalid_node_id() -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some("The requested node ID is invalid".to_string()),
    }
}

/// The `ValidationException` payload for a maintenance request that names no node:
/// the action is "the node maintenance process on the data node", so there is no
/// action to start without one.
pub fn node_id_required() -> ValidationException {
    ValidationException {
        message: Some("NodeId is required to start domain maintenance".to_string()),
    }
}

/// The `ResourceNotFoundException` payload for a `MaintenanceId` a domain's history
/// holds no maintenance action for.
pub fn maintenance_not_found(domain_name: &str, maintenance_id: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(format!(
            "Maintenance not found for domain {domain_name}: {maintenance_id}"
        )),
    }
}

/// The `ResourceNotFoundException` payload for an offering id the catalog publishes
/// no Reserved Instance offering for.
///
/// The sentence names the KIND of identifier that resolved to nothing and no value:
/// the id is the one the request itself carried, so echoing it back tells a caller
/// nothing it does not hold, and that is how the recorded wire refusal of such an id
/// words it — the same shape as the sibling refusal for a reservation id that
/// resolves to nothing ([`crate::state::reserved_instance_not_found`]). Every
/// operation that resolves an offering id answers through here, so the catalog
/// listing and a purchase cannot word the same miss differently.
pub fn offering_not_found() -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some("Offering id not found".to_string()),
    }
}

/// `entity`'s `Value`, or `None` where the request named none — an absent value and
/// a blank one name the same nothing.
pub(super) fn insight_entity_value(entity: &InsightEntity) -> Option<&str> {
    entity
        .value
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

/// The `ValidationException` payload for an insight entity that names a domain but
/// no domain name.
pub fn insight_value_required() -> ValidationException {
    ValidationException {
        message: Some(format!(
            "Entity value is required when Entity type is {INSIGHT_ENTITY_DOMAIN_NAME}."
        )),
    }
}

/// The `ValidationException` payload for an `Account` insight entity naming an
/// account other than the one the request was made for.
pub fn foreign_account_entity(account_id: &str) -> ValidationException {
    ValidationException {
        message: Some(format!(
            "Entity value {account_id} is not the Amazon Web Services account ID this request \
             was made for."
        )),
    }
}

/// The `ValidationException` payload for an insight entity whose type is outside
/// `InsightEntityType`.
pub fn unknown_insight_entity(entity_type: &str) -> ValidationException {
    ValidationException {
        message: Some(format!(
            "Entity type {entity_type} is not supported. Supported values are \
             {INSIGHT_ENTITY_ACCOUNT} and {INSIGHT_ENTITY_DOMAIN_NAME}."
        )),
    }
}

/// The `ValidationException` payload for a feedback entity whose type is outside
/// `InsightFeedbackEntityType` — which admits `DomainName` alone, so feedback accepts
/// strictly fewer entity types than a listing does.
pub fn unknown_insight_feedback_entity(entity_type: &str) -> ValidationException {
    ValidationException {
        message: Some(format!(
            "Entity type {entity_type} is not supported. Supported values are \
             {INSIGHT_ENTITY_DOMAIN_NAME}."
        )),
    }
}

/// The `ValidationException` payload for a feedback rating outside
/// `InsightFeedbackThumbs`.
pub fn unknown_insight_feedback_thumbs(thumbs: &str) -> ValidationException {
    ValidationException {
        message: Some(format!(
            "Thumbs {thumbs} is not supported. Supported values are {} and {}.",
            INSIGHT_FEEDBACK_THUMBS_UP, INSIGHT_FEEDBACK_THUMBS_DOWN
        )),
    }
}

/// The `ValidationException` payload for feedback text past the longest the modelled
/// member admits: it names the bound and the length the submission carried, so the
/// caller can fix the request from the refusal alone.
pub fn feedback_text_too_long(length: usize, maximum: usize) -> ValidationException {
    ValidationException {
        message: Some(format!(
            "FeedbackText must be at most {maximum} characters: {length}"
        )),
    }
}

/// What a feedback submission says of a resource it cannot resolve: the submission
/// named an insight of a domain, and neither the domain nor the insight resolved to
/// something this account holds.
///
/// The wording is the generic one this operation refuses with, naming no resource:
/// one refusal for both halves of the address, so a caller cannot tell a domain it
/// does not hold from an insight that domain does not hold — which is what the
/// service answers.
pub fn insight_resource_not_found() -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some("Resource not found".to_string()),
    }
}

/// The refusal payload for an upgrade whose `TargetVersion` is not a version the
/// domain can be upgraded to ([`compatible_target_versions`]).
///
/// The service answers an ineligible target on its GENERIC envelope — "an error
/// occurred while processing the request" — rather than on the request-validation
/// one, and states the two versions the eligibility was decided from: the version the
/// domain runs, read from its stored record, and the version the request asked for.
/// Nothing else of the request is named and nothing of the domain's state moves.
///
/// The domain's version is stated as its record declares it, and the target the caller
/// asked for in the service's legacy engine-version form ([`legacy_engine_version`]): an
/// Elasticsearch target as its bare release, an OpenSearch target with the engine name
/// its `VersionString` carries.
pub fn incompatible_target_version(source_version: &str, target_version: &str) -> BaseException {
    BaseException {
        message: Some(format!(
            "Failed to submit upgrade. Upgrade from {source_version} to {} \
             not supported.",
            legacy_engine_version(target_version)
        )),
    }
}

/// The `ResourceAlreadyExistsException` payload for an upgrade asked for while the
/// domain is still performing one: that run already exists, so the refusal names it
/// rather than the call starting a second.
pub fn upgrade_already_in_progress(
    domain_name: &str,
    upgrade_id: &str,
) -> ResourceAlreadyExistsException {
    ResourceAlreadyExistsException {
        message: Some(format!(
            "Domain {domain_name} is already performing upgrade {upgrade_id}."
        )),
    }
}

/// The most domain names ONE batch domain-describe may name.
///
/// The model declares no maximum on `DomainNameList`, so the bound is the service's
/// own: a batch describe reports one `DomainStatus` per name it is given, and the
/// service caps the batch at five rather than letting one request choose the size of
/// its answer. Recorded behaviour of the batch describe on both this surface and the
/// legacy one, which word the refusal in their own terms but cap at the same number.
pub const DESCRIBE_DOMAINS_BATCH_LIMIT: usize = 5;

/// The `ValidationException` payload for a batch domain-describe naming more domains
/// than one call may describe.
///
/// A FIXED sentence apart from the limit itself: it states the cap the request has to
/// stay within and quotes nothing of the request back — neither how many names it
/// carried nor which they were — so a caller is told the rule rather than its own
/// input. The surfaces word the subject in their own terms (this one describes
/// domains), so each states its own sentence.
pub fn too_many_domain_names() -> ValidationException {
    ValidationException {
        message: Some(format!(
            "Please provide a maximum of {DESCRIBE_DOMAINS_BATCH_LIMIT} domain names to describe."
        )),
    }
}

/// The `ValidationException` payload for a pagination token that does not name a
/// place in the listing it was sent to.
///
/// A token is only ever a value the service itself handed out, so one it cannot
/// resolve is a malformed request rather than an empty page.
pub fn invalid_next_token() -> ValidationException {
    ValidationException {
        message: Some(
            "NextToken is not a pagination token this operation issued. Send the NextToken \
             returned by the previous page, or omit it to start from the first page."
                .to_string(),
        ),
    }
}

/// The `ValidationException` payload for a page size outside the range the listing's
/// `MaxResults` publishes.
///
/// `MaxResults` is `@range(max: 100)` with `@default(0)`: a page size past the maximum
/// is a malformed request, and a NEGATIVE one is malformed for the same reason — it is
/// not a page size at all — while the default zero is the request that asks for no
/// particular page size. `limit` is the maximum the ASKING operation publishes, because
/// a listing may bound its page below what the shape allows.
pub fn max_results_out_of_range(max_results: i32, limit: i32) -> ValidationException {
    ValidationException {
        message: Some(format!(
            "MaxResults must be between 0 and {limit}: {max_results}"
        )),
    }
}

/// The `ValidationException` payload for a NEGATIVE page size, for a listing whose
/// `maxResults` publishes no maximum at all.
///
/// A negative value is not a page size, so it is refused rather than reinterpreted,
/// while the default zero stays the request that asks for no particular page size.
/// There is no upper bound to report because the shape declares none: a page size past
/// the size of the set is simply the whole set.
pub fn negative_max_results(max_results: i32) -> ValidationException {
    ValidationException {
        message: Some(format!("MaxResults must not be negative: {max_results}")),
    }
}

/// The number of colon-separated fields an ARN carries:
/// `arn:<partition>:<service>:<region>:<account-id>:<resource>`. The resource field
/// may itself contain colons, so it is the LAST field and is not split further.
pub(super) const ARN_FIELDS: usize = 6;

/// Whether `value` has ARN syntax: the `arn` literal, a partition, a service and a
/// non-empty resource, with the Region and account-id fields allowed to be empty
/// (service-wide and account-less ARNs leave them so).
///
/// The `ARN` shape is `@length(min: 20, max: 2048)` with pattern `.*`, so the request
/// boundary enforces only the length: an operation that takes an ARN checks the
/// syntax itself rather than storing a value no ARN parser would accept — the same
/// reasoning as [`is_guid`]. It is a SYNTAX test only: resolving an ARN to a stored
/// resource is the job of whichever registry owns that resource.
pub fn is_arn(value: &str) -> bool {
    let fields: Vec<&str> = value.splitn(ARN_FIELDS, ':').collect();
    fields.len() == ARN_FIELDS
        && fields[0] == "arn"
        && !fields[1].is_empty()
        && !fields[2].is_empty()
        && !fields[5].is_empty()
}

/// The `ValidationException` payload for a member that must carry an ARN and does
/// not. It names the member and the value that could not be read as one, so the
/// caller can tell which input it was.
pub fn malformed_arn(member: &str, value: &str) -> ValidationException {
    ValidationException {
        message: Some(format!("{member} is not a valid ARN: {value}")),
    }
}

/// The `ResourceNotFoundException` payload for a read of the default-application
/// setting made while no default application is set — the state the setting is in
/// before any `PutDefaultApplicationSetting`, and after one that cleared it.
pub fn default_application_not_set() -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some("No default application is set for this account.".to_string()),
    }
}

/// The `ResourceAlreadyExistsException` payload for a create naming a connection
/// alias the source domain already has a connection under — the mirror of
/// [`data_source_exists`], so one refusal reads like the other.
pub fn connection_alias_exists(alias: &str, local_domain: &str) -> ResourceAlreadyExistsException {
    ResourceAlreadyExistsException {
        message: Some(format!(
            "Domain {local_domain} already has a cross-cluster connection with the alias {alias}."
        )),
    }
}

/// The `InvalidPaginationTokenException` payload for a `NextToken` that names no
/// place in the set being paged: one this service did not mint, or one whose record
/// has since been released.
///
/// The same refusal as [`invalid_next_token`] in a different modelled shape: an
/// operation that LISTS `InvalidPaginationTokenException` refuses an unresolvable
/// token with that error, and one that does not falls back to its
/// `ValidationException`.
///
/// The text is FIXED and carries no variable part — in particular it does not quote
/// the token back, which is the caller's own input and tells it nothing it did not
/// send — so every operation that refuses a token with this error words the refusal
/// identically.
pub fn invalid_pagination_token() -> InvalidPaginationTokenException {
    InvalidPaginationTokenException {
        message: Some("Pagination token provided is invalid".to_string()),
    }
}
