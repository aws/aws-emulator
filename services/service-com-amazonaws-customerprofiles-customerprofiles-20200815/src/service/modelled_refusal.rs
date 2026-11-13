// SPDX-License-Identifier: CC0-1.0

//! The MODELLED refusal this service answers for a request it refuses before any handler
//! runs.
//!
//! RULE: every error shape this model declares carries its prose in a member spelled
//! `Message` (`smithy_query(query="shape", shape="BadRequestException")`), and
//! `BadRequestException` (HTTP 400, "The input you provided is invalid.") is the one
//! client-input error every operation of this model lists — the model declares no
//! `ValidationException` at all. So a request refused before a handler runs is answered as
//! that shape, serialized through its own schema: the same path every refusal a HANDLER
//! returns takes, so one refused request answers one shape whichever layer refused it.
//!
//! WHY IT IS DONE HERE. The framework's pre-dispatch renderer reaches the right status and
//! then writes the prose into a framework-chosen lowercase `message` key, the same for
//! every restJson1 service, and names `ValidationException` for a body it could not read
//! (`vendor/smithy-server-runtime/src/pre_dispatch.rs`, read-only from a service crate).
//! The seam a service has for that refusal (`Service::override_refusal`) chooses an error
//! code and a sentence inside that envelope and nothing else, so the body reached the
//! caller with no `Message` member at all: an SDK caller reading
//! `e.response['Error']['Message']` learns nothing about which member or which constraint
//! failed, and `except client.exceptions.BadRequestException` never matches a refusal
//! named after a shape the model does not declare.
//!
//! WHAT THE WRAPPER ANSWERS. Every modelled operation is served behind
//! [`ModelledRefusal`] ([`table`], returned from `Service::override_operation`), which
//! answers the two refusals that precede a handler, in the order the request walk reaches
//! them:
//!
//! 1. a value whose JSON type the member it is bound to cannot be read from, which never
//!    became an input ([`super::unreadable_values`]) — deserialization precedes validation,
//!    so this is asked first;
//! 2. a value the model's own constraints refuse — an absent `@required` member, a
//!    `@length`/`@range`/`@pattern`/enum violation — stated in AWS's validation envelope:
//!    `<n> validation error(s) detected: Value '<value>' at '<member path>' failed to
//!    satisfy constraint: <requirement>`.
//!
//! and, as a backstop, restates any OTHER 400 the framework rendered for this request (a
//! codec failure this layer does not word) as the same modelled shape, carrying the
//! sentence the framework wrote. So no 400 this service answers names an error type its
//! model does not declare, and none carries its prose anywhere but `Message`.
//!
//! Nothing else is re-implemented and no constraint is re-checked: the violations come from
//! the framework's own machinery — the protocol's deserializer for an absent member and
//! `constraints::validate` over the built input — through the tree's shared collecting walk
//! (`service_common::validation::refusing_violations`), so this layer cannot find a
//! different set from the walk that follows it. A request it does not refuse is handed to
//! the generated `ErasedOperation::run`, which owns deserialize -> validate -> handle ->
//! serialize, telemetry and every modelled error.
//!
//! THE TWO RENDERINGS THIS SERVICE CHOOSES, which the envelope and the clauses are
//! otherwise entirely the framework's and the shared renderer's:
//!
//! * **an enumerated type's value set is listed in the order the layer in front of this
//!   service walks its own hash table of the admissible values**, not in the order the
//!   model declares them (`service_common::validation_message::in_hashed_enum_order`). It
//!   is a property of the TYPE — computed from the declared values themselves — so every
//!   member targeting one enum is stated alike and a type the model gains needs no entry
//!   anywhere;
//! * **a `@pattern` requirement is quoted as the model declares it for a value bound to the
//!   request URI, and without the whole-value anchors for a BODY member**
//!   (`service_common::validation::unanchored`). The two halves of a request are read by
//!   two layers — the body against the operation's input schema, a path segment or a query
//!   parameter by the service itself — and the body layer quotes the expression a value has
//!   to satisfy while the other quotes the expression the model publishes. WHICH half a
//!   violated member belongs to is read off that operation's own input schema
//!   (`service_common::front_door::binds_to_body`), which is why this is decided per
//!   operation here rather than at the service-level refusal seam, where two operations
//!   binding one member name differently are indistinguishable. The CHECK always runs the
//!   model's pattern as written; only the quoting differs.
//!
//! THE ONE HALF OF A CLAUSE THAT IS NEITHER: a member the model marks `@sensitive` is one
//! the service never discloses, so its clause names the member and the requirement and
//! states NO value — `Value at '<member path>' failed to satisfy constraint: …`. That is
//! the clause the framework's own walk writes for such a member, decided from that member's
//! own generated schema; the shared collecting walk hands the submitted value back BESIDE
//! the violation for the services whose refusals quote it, and this one quotes it for every
//! member but those.

use std::sync::Arc;

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse, StatusCode};
use aws_smithy_schema::Schema;
use aws_smithy_types::body::SdkBody;
use serde_json::Value;
use service_common::front_door::binds_to_body;
use service_common::validation::{refusing_violations_with_echoes, unanchored};
use service_common::validation_message::{
    aws_validation_refusal_with, in_hashed_enum_order, quoting_the_submitted_collection,
    quoting_the_whole_submitted_value,
};
use smithy_server_runtime::{
    BoxFuture, ErasedOperation, Operation, OperationContext, OperationRoutingTable, Refusal,
    ServerHttpProtocol, Service, Violation, ViolationKind,
};

use crate::ops::types::{BadRequestException, SCHEMA_BAD_REQUEST_EXCEPTION};

/// The error shape a refused input is answered as: the one client-input error every
/// operation of this model lists.
pub const BAD_REQUEST_EXCEPTION: &str = "BadRequestException";

/// The HTTP status that shape's `@httpError` declares.
const BAD_REQUEST_STATUS: u16 = 400;

/// The member the framework's pre-dispatch renderer writes its prose into. No error shape
/// of this model declares it, so a body carrying it is one the framework rendered outside
/// the modelled error path.
const FRAMEWORK_MESSAGE_MEMBER: &str = "message";

/// Wire one `crate::ops::<module>::<Operation>` per line into the table, each wrapped in
/// [`ModelledRefusal`] and keyed by its operation shape name — the routing key
/// `ServerProtocol::routing_key` resolves and the generated table is keyed by — and record
/// the same names for [`REFUSED_AS_MODELLED`], so the surface is written down once.
macro_rules! modelled_refusal_table {
    ($($module:ident :: $operation:ident),+ $(,)?) => {
        /// Every operation served behind [`ModelledRefusal`], by the routing key it is
        /// served under.
        ///
        /// The one place this crate enumerates the service's surface; the suite pins it
        /// against the operations the model binds, so an operation the model gains cannot
        /// be left answering the framework's envelope unnoticed.
        pub const REFUSED_AS_MODELLED: &[&str] = &[$(stringify!($operation)),+];

        /// The routing table this service answers `Service::override_operation` from: the
        /// same modelled operations the generated table holds, each wrapped in
        /// [`ModelledRefusal`] and built from the SAME shared engine, so a wrapped
        /// operation reads and writes the one store.
        pub fn table<Eng: ::std::marker::Send + ::std::marker::Sync + 'static>(
            engine: &Arc<Eng>,
        ) -> OperationRoutingTable {
            let mut table = OperationRoutingTable::new();
            $(
                table.add_operation(
                    stringify!($operation),
                    ::std::boxed::Box::new(ModelledRefusal::new(
                        crate::ops::$module::$operation::new(Arc::clone(engine)),
                    )) as ::std::boxed::Box<dyn ErasedOperation>,
                );
            )+
            table
        }
    };
}

modelled_refusal_table!(
    add_profile_key::AddProfileKey,
    associate_stream_for_segments::AssociateStreamForSegments,
    batch_get_calculated_attribute_for_profile::BatchGetCalculatedAttributeForProfile,
    batch_get_profile::BatchGetProfile,
    batch_put_profile_object::BatchPutProfileObject,
    create_calculated_attribute_definition::CreateCalculatedAttributeDefinition,
    create_domain::CreateDomain,
    create_domain_layout::CreateDomainLayout,
    create_event_stream::CreateEventStream,
    create_event_trigger::CreateEventTrigger,
    create_integration_workflow::CreateIntegrationWorkflow,
    create_profile::CreateProfile,
    create_recommender::CreateRecommender,
    create_recommender_filter::CreateRecommenderFilter,
    create_recommender_schema::CreateRecommenderSchema,
    create_segment_definition::CreateSegmentDefinition,
    create_segment_estimate::CreateSegmentEstimate,
    create_segment_snapshot::CreateSegmentSnapshot,
    create_upload_job::CreateUploadJob,
    delete_calculated_attribute_definition::DeleteCalculatedAttributeDefinition,
    delete_domain::DeleteDomain,
    delete_domain_layout::DeleteDomainLayout,
    delete_domain_object_type::DeleteDomainObjectType,
    delete_event_stream::DeleteEventStream,
    delete_event_trigger::DeleteEventTrigger,
    delete_integration::DeleteIntegration,
    delete_profile::DeleteProfile,
    delete_profile_key::DeleteProfileKey,
    delete_profile_object::DeleteProfileObject,
    delete_profile_object_type::DeleteProfileObjectType,
    delete_recommender::DeleteRecommender,
    delete_recommender_filter::DeleteRecommenderFilter,
    delete_recommender_schema::DeleteRecommenderSchema,
    delete_segment_definition::DeleteSegmentDefinition,
    delete_segment_subscription::DeleteSegmentSubscription,
    delete_workflow::DeleteWorkflow,
    detect_profile_object_type::DetectProfileObjectType,
    disassociate_stream_for_segments::DisassociateStreamForSegments,
    get_auto_merging_preview::GetAutoMergingPreview,
    get_calculated_attribute_definition::GetCalculatedAttributeDefinition,
    get_calculated_attribute_for_profile::GetCalculatedAttributeForProfile,
    get_domain::GetDomain,
    get_domain_layout::GetDomainLayout,
    get_domain_object_type::GetDomainObjectType,
    get_event_stream::GetEventStream,
    get_event_trigger::GetEventTrigger,
    get_identity_resolution_job::GetIdentityResolutionJob,
    get_integration::GetIntegration,
    get_matches::GetMatches,
    get_object_type_attribute_statistics::GetObjectTypeAttributeStatistics,
    get_profile_history_record::GetProfileHistoryRecord,
    get_profile_object_type::GetProfileObjectType,
    get_profile_object_type_template::GetProfileObjectTypeTemplate,
    get_profile_recommendations::GetProfileRecommendations,
    get_recommender::GetRecommender,
    get_recommender_filter::GetRecommenderFilter,
    get_recommender_schema::GetRecommenderSchema,
    get_segment_definition::GetSegmentDefinition,
    get_segment_estimate::GetSegmentEstimate,
    get_segment_membership::GetSegmentMembership,
    get_segment_snapshot::GetSegmentSnapshot,
    get_segment_subscription::GetSegmentSubscription,
    get_similar_profiles::GetSimilarProfiles,
    get_stream_for_segments::GetStreamForSegments,
    get_upload_job::GetUploadJob,
    get_upload_job_path::GetUploadJobPath,
    get_workflow::GetWorkflow,
    get_workflow_steps::GetWorkflowSteps,
    list_account_integrations::ListAccountIntegrations,
    list_calculated_attribute_definitions::ListCalculatedAttributeDefinitions,
    list_calculated_attributes_for_profile::ListCalculatedAttributesForProfile,
    list_domain_layouts::ListDomainLayouts,
    list_domain_object_types::ListDomainObjectTypes,
    list_domains::ListDomains,
    list_event_streams::ListEventStreams,
    list_event_triggers::ListEventTriggers,
    list_identity_resolution_jobs::ListIdentityResolutionJobs,
    list_integrations::ListIntegrations,
    list_object_type_attribute_values::ListObjectTypeAttributeValues,
    list_object_type_attributes::ListObjectTypeAttributes,
    list_profile_attribute_values::ListProfileAttributeValues,
    list_profile_history_records::ListProfileHistoryRecords,
    list_profile_object_type_templates::ListProfileObjectTypeTemplates,
    list_profile_object_types::ListProfileObjectTypes,
    list_profile_objects::ListProfileObjects,
    list_recommender_filters::ListRecommenderFilters,
    list_recommender_recipes::ListRecommenderRecipes,
    list_recommender_schemas::ListRecommenderSchemas,
    list_recommenders::ListRecommenders,
    list_rule_based_matches::ListRuleBasedMatches,
    list_segment_definitions::ListSegmentDefinitions,
    list_segment_subscription_events::ListSegmentSubscriptionEvents,
    list_tags_for_resource::ListTagsForResource,
    list_upload_jobs::ListUploadJobs,
    list_workflows::ListWorkflows,
    merge_profiles::MergeProfiles,
    put_domain_object_type::PutDomainObjectType,
    put_integration::PutIntegration,
    put_profile_object::PutProfileObject,
    put_profile_object_type::PutProfileObjectType,
    put_segment_subscription::PutSegmentSubscription,
    search_profiles::SearchProfiles,
    search_recommendations::SearchRecommendations,
    start_recommender::StartRecommender,
    start_upload_job::StartUploadJob,
    stop_recommender::StopRecommender,
    stop_upload_job::StopUploadJob,
    tag_resource::TagResource,
    untag_resource::UntagResource,
    update_calculated_attribute_definition::UpdateCalculatedAttributeDefinition,
    update_domain::UpdateDomain,
    update_domain_layout::UpdateDomainLayout,
    update_event_trigger::UpdateEventTrigger,
    update_profile::UpdateProfile,
    update_recommender::UpdateRecommender,
);

/// One modelled operation, served with the refusals that precede it stated in this
/// service's own `BadRequestException` (see the module docs).
pub struct ModelledRefusal<O> {
    operation: O,
}

impl<O> ModelledRefusal<O> {
    /// Wrap one operation, already constructed from the shared engine.
    fn new(operation: O) -> Self {
        Self { operation }
    }
}

impl<O> ErasedOperation for ModelledRefusal<O>
where
    O: Operation + ErasedOperation,
{
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r RtRequest,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        Box::pin(async move {
            // The protocol is read fresh at each use rather than held across the await
            // below: a `&dyn ErasedServiceDefinition` is not `Sync`, so holding one across
            // it would make this future non-`Send`.
            if let Some(message) = refusal::<O>(svc.definition().protocol(), req) {
                return bad_request(svc.definition().protocol(), message);
            }
            let answered = self.operation.run(svc, req, ctx).await;
            restated(svc.definition().protocol(), answered)
        })
    }

    /// The wrapped operation's modelled auth schemes: wrapping an operation must not change
    /// which schemes sign it.
    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        self.operation.auth_schemes()
    }
}

/// The sentence this service refuses `req` with before `O`'s handler runs, or `None` when
/// it refuses nothing — in which case the operation behind the wrapper is served.
///
/// Deserialization comes first, because a value that never became a value has broken no
/// bound: the constraint layer is reached only by a body the operation's input can be read
/// from.
fn refusal<O: Operation>(protocol: &dyn ServerHttpProtocol, req: &RtRequest) -> Option<String> {
    let body = req.body().bytes().unwrap_or_default();
    super::unreadable_values::unreadable_value(O::INPUT_SCHEMA, body)
        .or_else(|| constraint_refusal::<O>(protocol, req))
}

/// AWS's validation envelope over every modelled constraint `req` breaks, in the two
/// renderings this service chooses (see the module docs), or `None` when it breaks none.
fn constraint_refusal<O: Operation>(
    protocol: &dyn ServerHttpProtocol,
    req: &RtRequest,
) -> Option<String> {
    let stated: Vec<Violation> = refusing_violations_with_echoes::<O>(protocol, req)
        .into_iter()
        .map(|(violation, echo)| {
            // A member the model marks `@sensitive` keeps the clause the walk wrote, which
            // names the member and the requirement and states NO value: the value is the
            // one thing such a clause must not disclose, so neither restoring rule below is
            // asked about it and the request's bytes never leave the walk.
            if violation.sensitive {
                return violation;
            }
            // A collection's clause states the collection and a scalar's the bytes
            // submitted: the framework's walk stores a size summary for the one and its own
            // elision of a long value for the other, while an AWS clause quotes what the
            // request carried. Each rule touches only its own clause, so the two compose.
            let quoted = quoting_the_submitted_collection(violation, echo.clone());
            quoting_the_whole_submitted_value(quoted, &echo)
        })
        .map(|violation| as_this_service_states(O::INPUT_SCHEMA, &in_hashed_enum_order(&violation)))
        .collect();
    // The envelope, the member paths, the clause and every requirement are the shared
    // renderer's and the framework's; the `@pattern` quoting is already applied above, so
    // the expression is passed through here, and the value half of a clause about a
    // `@sensitive` member is already absent from the violation. Only the MESSAGE is taken:
    // the code that renderer travels with is `ValidationException`, a shape this model does
    // not declare, and the code this service answers with is decided in `bad_request` for
    // every layer alike.
    aws_validation_refusal_with(&Refusal::Invalid(stated), str::to_string)
        .map(|refused| refused.message)
}

/// `violation` as this service states it: a `@pattern` broken by a BODY member quoted
/// without the whole-value anchors the model writes around it, and every other violation —
/// a `@pattern` broken by a value bound to the request URI included — left exactly as the
/// framework's walk reported it.
fn as_this_service_states(input_schema: &Schema, violation: &Violation) -> Violation {
    let ViolationKind::Pattern(pattern) = &violation.kind else {
        return violation.clone();
    };
    if !binds_to_body(input_schema, &violation.path) {
        return violation.clone();
    }
    Violation {
        kind: ViolationKind::Pattern(unanchored(pattern)),
        ..violation.clone()
    }
}

/// This service's refusal of an input carrying `message`: HTTP 400 with the prose in the
/// `Message` member the model declares, serialized through that shape's own schema.
///
/// The ONE place this service renders a refused input, so the deserialization layer, the
/// constraint layer and the backstop below cannot drift apart in anything but the prose
/// each owns. A decision to refuse never degrades into success: where the shape cannot be
/// serialized through its own schema, the same status, error code and member are written
/// directly rather than letting the request fall through.
fn bad_request(protocol: &dyn ServerHttpProtocol, message: String) -> RtResponse {
    let refused = BadRequestException {
        message: Some(message),
    };
    protocol
        .serialize_error(
            &refused,
            &SCHEMA_BAD_REQUEST_EXCEPTION,
            BAD_REQUEST_EXCEPTION,
            BAD_REQUEST_STATUS,
        )
        .unwrap_or_else(|_| written_directly(refused.message.as_deref().unwrap_or_default()))
}

/// `answered` as this service sends it: a 400 the framework rendered OUTSIDE the modelled
/// error path — recognised by the [`FRAMEWORK_MESSAGE_MEMBER`] no error shape of this model
/// declares — restated as [`BAD_REQUEST_EXCEPTION`] carrying that same sentence in
/// `Message`.
///
/// Every other response is returned untouched, byte for byte: a success, a refusal a
/// handler built (which already carries `Message`), and every status but 400 — an
/// authentication refusal and the scaffold's 501 among them, neither of which is this
/// service's `BadRequestException`.
fn restated(protocol: &dyn ServerHttpProtocol, answered: RtResponse) -> RtResponse {
    if answered.status().as_u16() != BAD_REQUEST_STATUS {
        return answered;
    }
    let message = {
        // An error body is built in memory by its renderer, so the bytes are readable here;
        // a streamed body is not an error body and is left alone.
        let Some(bytes) = answered.body().bytes() else {
            return answered;
        };
        match framework_message(bytes) {
            Some(message) => message,
            None => return answered,
        }
    };
    bad_request(protocol, message)
}

/// The sentence a body the FRAMEWORK rendered carries, and `None` for a body that is not
/// one: a refusal a handler built (whose prose is in `Message`), a body that is not a JSON
/// object, and one that is not JSON at all.
fn framework_message(body: &[u8]) -> Option<String> {
    let document: Value = serde_json::from_slice(body).ok()?;
    let stated = document.as_object()?.get(FRAMEWORK_MESSAGE_MEMBER)?;
    Some(stated.as_str().unwrap_or_default().to_string())
}

/// The same refusal written without the protocol's serializer, for the case where the
/// modelled shape cannot be rendered through its own schema: the status, the error code and
/// the `Message` member, so a refusal is never answered as a success.
fn written_directly(message: &str) -> RtResponse {
    let body = serde_json::json!({
        "__type": BAD_REQUEST_EXCEPTION,
        "Message": message,
    })
    .to_string();
    let mut response = RtResponse::new(
        StatusCode::try_from(BAD_REQUEST_STATUS).expect("400 is a valid status"),
        SdkBody::from(body),
    );
    response
        .headers_mut()
        .insert("content-type", "application/json");
    response
        .headers_mut()
        .insert("x-amzn-errortype", BAD_REQUEST_EXCEPTION);
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RULE: a 400 the framework rendered is recognised by the lowercase `message` member
    /// no error shape of this model declares; a refusal a handler built, and anything that
    /// is not a JSON object, is left alone.
    #[test]
    fn a_framework_rendered_body_is_the_one_restated() {
        // (case, the body, the sentence restated)
        let restatable = [
            (
                "a codec failure",
                r#"{"__type":"ValidationException","message":"expected string, got number"}"#,
                "expected string, got number",
            ),
            (
                "a reworded constraint refusal",
                r#"{"__type":"BadRequestException","message":"1 validation error detected"}"#,
                "1 validation error detected",
            ),
            (
                "a sentence the renderer left out",
                r#"{"__type":"ValidationException","message":null}"#,
                "",
            ),
        ];
        for (case, body, expected) in restatable {
            assert_eq!(
                framework_message(body.as_bytes()).as_deref(),
                Some(expected),
                "case {case:?}: expected the sentence {expected:?} to be restated"
            );
        }

        for (case, body) in [
            (
                "a refusal a handler built",
                r#"{"__type":"BadRequestException","Message":"Domain d is not found"}"#,
            ),
            ("no prose at all", r#"{"__type":"ThrottlingException"}"#),
            ("not an object", r#"["BadRequestException"]"#),
            ("not JSON", "<Error/>"),
        ] {
            assert!(
                framework_message(body.as_bytes()).is_none(),
                "case {case:?}: expected the body to be left alone"
            );
        }
    }
}
