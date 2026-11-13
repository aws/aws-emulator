// SPDX-License-Identifier: CC0-1.0

//! This service's request-DESERIALIZATION layer: how a body value whose JSON CLASS the
//! member it is bound to cannot be read from is refused.
//!
//! A value of a class a member's modelled type admits no reading of never becomes a value
//! at all — a policy document sent as a JSON OBJECT where the member is modelled a string,
//! a cluster sent as a string where the member is modelled a structure. AWS refuses such a
//! request at the deserialization layer, before any operation runs: HTTP 400 under the
//! protocol's own [`SERIALIZATION_EXCEPTION`], whose message states the CLASS of value the
//! caller sent and the modelled type it could not be read into. Nothing is read, so nothing
//! is mutated.
//!
//! The framework's codec refuses the same requests, but it renders its own parser
//! diagnostic (`expected string, got object start`) as the service's message, under the
//! constraint-violation error class — a sentence that names neither the class sent nor the
//! type expected, and a class that says the value was read and then broke a rule. The
//! vendored tree is read-only from a service crate, so the refusal is worded here, at the
//! one seam a service has before dispatch
//! ([`crate::service::EmulatedService::override_operation`]), exactly as
//! `services/service-com-amazonaws-directoryservicedata-directoryservicedata`,
//! `services/service-com-amazonaws-kinesisanalyticsv2-kinesisanalytics-20180523` and
//! `services/service-com-amazonaws-xray-awsxray` do for their own request layers.
//!
//! Nothing about the model is written down here beyond the one table of modelled
//! operations ([`input_schemas`]), and that table names no member and no type: each entry
//! is an operation TYPE, which carries its own wire name and its own input schema. WHICH
//! members a body may carry, under which wire name each arrives and what each is modelled
//! as is read off those schemas, through `service_common::json_wire_values` — this tree's
//! one naming of a JSON value's class, of the member a key names and of the sentence
//! refusing a value that could not be read.
//!
//! Two SCOPE boundaries, both deliberate:
//!
//! * the classes this layer decides are the ones a member's modelled KIND admits no
//!   reading of ([`class_fits`]), so a request it refuses is one the codec was going to
//!   refuse anyway, with the class and the type named;
//! * the TOP-LEVEL members of an operation's input, plus the body document itself. A
//!   member `Schema` in this framework carries its target's KIND but not its target's
//!   schema, so a value nested inside a structure, a list or a map is not reachable from
//!   here and is left exactly as the codec reads it — the same boundary the three sibling
//!   layers state.

use std::collections::HashMap;

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use aws_smithy_schema::{Schema, ShapeType};
use aws_smithy_schema_server::ProtocolError;
use serde_json::Value;
use service_common::json_wire_values::{coercion_message, json_value_class, member_by_wire_name};
use smithy_server_runtime::{
    BoxFuture, ErasedOperation, Operation, OperationContext, Refused, Service,
};

/// The error code a request body that never became an input is refused with.
///
/// It is the PROTOCOL's error, not one of this service's modelled shapes: no operation ran,
/// so none of them answered with an error of its own. A value that WAS read and then broke
/// a rule the model declares keeps the constraint-violation class
/// ([`crate::service::validation`]); a client branches on the difference.
pub(crate) const SERIALIZATION_EXCEPTION: &str = "SerializationException";

/// What this layer states when the request that reached it no longer shows WHICH value
/// could not be read: the fact the refusal rests on, with no class claimed.
const UNREADABLE_BODY: &str = "The request body can not be deserialized.";

/// The operation this service serves for a request carrying a body value the modelled
/// member it is bound to cannot be read from.
///
/// An [`ErasedOperation`] rather than a response built in
/// [`Service::override_operation`], because that seam hands the router a BORROWED
/// operation. It holds the service's input schemas and no request state: the refusal is
/// re-derived from the same request the router passes back into [`ErasedOperation::run`],
/// so one value serves every such request.
pub(crate) struct UnreadableBody {
    /// The INPUT schema of every modelled operation, keyed by the wire name the protocol
    /// resolves a request's routing key to — which is the operation's own name
    /// ([`input_schemas`]).
    schemas: HashMap<&'static str, &'static Schema>,
}

impl UnreadableBody {
    /// Build the layer for a service mounted on engine type `Eng`.
    pub(crate) fn new<Eng: Send + Sync + 'static>() -> Self {
        Self {
            schemas: input_schemas::<Eng>(),
        }
    }

    /// The sentence refusing `body` as the input of `operation`, or `None` when there is
    /// nothing here to refuse.
    ///
    /// `None` deliberately covers every other outcome: a routing key naming no modelled
    /// operation, a body that is not JSON (an empty one included), and a body whose every
    /// value is of a class the member it is bound to can be read from — each of which is
    /// left exactly as the framework answers it today.
    pub(crate) fn refusal(&self, operation: &str, body: &[u8]) -> Option<String> {
        unreadable_value(*self.schemas.get(operation)?, body)
    }
}

impl ErasedOperation for UnreadableBody {
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r RtRequest,
        _ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        // The same decision the seam made, re-derived from the same request: this
        // operation is reached only when it answered `Some`, so the refusal is found
        // again. The fallback states only what IS known on that path — that the body
        // could not be read — and never names a class the request may not have carried.
        let message = svc
            .definition()
            .wire_protocol(req)
            .protocol()
            .routing_key(req)
            .ok()
            .and_then(|operation| self.refusal(operation, request_body(req)))
            .unwrap_or_else(|| UNREADABLE_BODY.to_string());
        // The status and the envelope stay with the protocol, which owns them; the code
        // and the sentence are this layer's. `Malformed` is the framework's own class for
        // a body it could not process, so the protocol's default answer for it is already
        // the status this refusal carries.
        let answer = svc.definition().refusal_response(
            ProtocolError::Malformed(message.clone()),
            Some(Refused {
                code: SERIALIZATION_EXCEPTION.to_owned(),
                message,
            }),
            req,
        );
        Box::pin(std::future::ready(answer))
    }
}

/// The bytes of a request's body, empty for a request that carries none.
pub(crate) fn request_body(req: &RtRequest) -> &[u8] {
    req.body().bytes().unwrap_or_default()
}

/// The first value in `body` whose JSON class the modelled target type of the member it is
/// bound to admits no reading of, as the message naming it.
///
/// Reads the shape, not the request: a member's wire name and its modelled target type
/// both come off `schema`. Two levels are reachable and both are checked — the body
/// document itself, which must be the input structure, and each of the input's own
/// members, in the order a JSON object reports them.
///
/// An operation whose input binds a member to the raw body (`@httpPayload`) is left
/// alone: the body is then that member's value rather than a document of members, so
/// nothing here could name which member a value belongs to.
fn unreadable_value(schema: &Schema, body: &[u8]) -> Option<String> {
    if schema
        .members()
        .iter()
        .any(|member| member.http_payload().is_some())
    {
        return None;
    }
    let Ok(value) = serde_json::from_slice::<Value>(body) else {
        return None;
    };
    if !class_fits(schema.shape_type(), &value) {
        return Some(coercion_message(json_value_class(&value), schema.shape_type()));
    }
    let Value::Object(members) = &value else {
        return None;
    };
    members
        .iter()
        // A key naming no member is ignored, exactly as the codec ignores it, and a member
        // read from the URI or the headers carries no value in the body.
        .filter_map(|(key, held)| Some((member_by_wire_name(schema, key)?, held)))
        .filter(|(member, _)| !bound_to_the_request(member))
        .find_map(|(member, held)| {
            (!class_fits(member.shape_type(), held))
                .then(|| coercion_message(json_value_class(held), member.shape_type()))
        })
}

/// Whether a member is read from the request rather than from the body, so the body
/// carries no value for it.
fn bound_to_the_request(member: &Schema) -> bool {
    member.http_label().is_some()
        || member.http_query().is_some()
        || member.http_query_params().is_some()
        || member.http_header().is_some()
        || member.http_prefix_headers().is_some()
}

/// Whether a JSON value's class is one the modelled `target` type admits a reading of.
///
/// The classes are the Smithy ones: a string reads into a string-shaped target (an
/// enumerated type IS a string shape here, and a blob's base64 travels as a string), a
/// number into a numeric one, a boolean into a boolean, an array into a list, an object
/// into a map, a structure or a union, and anything at all into a `document`. The two
/// arbitrary-precision numerics and a timestamp read from a number OR a string, because
/// each of those is a form the value travels in — a timestamp's form is what
/// `@timestampFormat` decides.
///
/// JSON `null` is absence, not a class: an absent member the shape requires is the
/// validation layer's refusal, not this one's.
///
/// A target this layer states no reading for admits every class, so a model that grows a
/// type is left to the codec rather than refused by a rule nothing here wrote down.
fn class_fits(target: ShapeType, value: &Value) -> bool {
    if matches!(value, Value::Null) || matches!(target, ShapeType::Document) {
        return true;
    }
    match target {
        ShapeType::String | ShapeType::Blob => matches!(value, Value::String(_)),
        ShapeType::Boolean => matches!(value, Value::Bool(_)),
        ShapeType::Byte
        | ShapeType::Short
        | ShapeType::Integer
        | ShapeType::Long
        | ShapeType::Float
        | ShapeType::Double => matches!(value, Value::Number(_)),
        ShapeType::BigInteger | ShapeType::BigDecimal | ShapeType::Timestamp => {
            matches!(value, Value::Number(_) | Value::String(_))
        }
        ShapeType::List => matches!(value, Value::Array(_)),
        ShapeType::Map | ShapeType::Structure | ShapeType::Union => {
            matches!(value, Value::Object(_))
        }
        _ => true,
    }
}

/// Every modelled operation of this service, as the pair of facts this layer needs about
/// it: the wire name the protocol resolves a request's routing key to, and the schema of
/// its input shape.
///
/// Read off the operation TYPE, so neither fact is written down here and neither can drift
/// from the model: `NAME` and `INPUT_SCHEMA` are the constants codegen filled for that
/// operation.
fn entry<O: Operation>() -> (&'static str, &'static Schema) {
    (O::NAME, O::INPUT_SCHEMA)
}

/// The INPUT schema of every operation this service serves, keyed by its wire name: the
/// one place this module enumerates the service's surface.
///
/// Every modelled operation is listed, so an operation whose input gains a member is
/// covered without a change here, and a routing key naming none answers `None` and is left
/// to the routing table's own refusal. Regenerate with
/// `ls src/ops/*.rs`, which names exactly these modules; each entry is compiled against
/// the operation type, so a name this list gets wrong does not build.
fn input_schemas<Eng: Send + Sync + 'static>() -> HashMap<&'static str, &'static Schema> {
    use crate::ops;
    [
        entry::<ops::accept_inbound_connection::AcceptInboundConnection<Eng>>(),
        entry::<ops::add_data_source::AddDataSource<Eng>>(),
        entry::<ops::add_direct_query_data_source::AddDirectQueryDataSource<Eng>>(),
        entry::<ops::add_tags::AddTags<Eng>>(),
        entry::<ops::associate_package::AssociatePackage<Eng>>(),
        entry::<ops::associate_packages::AssociatePackages<Eng>>(),
        entry::<ops::attach_data_source::AttachDataSource<Eng>>(),
        entry::<ops::authorize_vpc_endpoint_access::AuthorizeVpcEndpointAccess<Eng>>(),
        entry::<ops::cancel_domain_config_change::CancelDomainConfigChange<Eng>>(),
        entry::<ops::cancel_service_software_update::CancelServiceSoftwareUpdate<Eng>>(),
        entry::<ops::create_application::CreateApplication<Eng>>(),
        entry::<ops::create_domain::CreateDomain<Eng>>(),
        entry::<ops::create_index::CreateIndex<Eng>>(),
        entry::<ops::create_outbound_connection::CreateOutboundConnection<Eng>>(),
        entry::<ops::create_package::CreatePackage<Eng>>(),
        entry::<ops::create_vpc_endpoint::CreateVpcEndpoint<Eng>>(),
        entry::<ops::delete_application::DeleteApplication<Eng>>(),
        entry::<ops::delete_data_source::DeleteDataSource<Eng>>(),
        entry::<ops::delete_direct_query_data_source::DeleteDirectQueryDataSource<Eng>>(),
        entry::<ops::delete_domain::DeleteDomain<Eng>>(),
        entry::<ops::delete_inbound_connection::DeleteInboundConnection<Eng>>(),
        entry::<ops::delete_index::DeleteIndex<Eng>>(),
        entry::<ops::delete_outbound_connection::DeleteOutboundConnection<Eng>>(),
        entry::<ops::delete_package::DeletePackage<Eng>>(),
        entry::<ops::delete_vpc_endpoint::DeleteVpcEndpoint<Eng>>(),
        entry::<ops::deregister_capability::DeregisterCapability<Eng>>(),
        entry::<ops::describe_data_source_attachment::DescribeDataSourceAttachment<Eng>>(),
        entry::<ops::describe_domain::DescribeDomain<Eng>>(),
        entry::<ops::describe_domain_auto_tunes::DescribeDomainAutoTunes<Eng>>(),
        entry::<ops::describe_domain_change_progress::DescribeDomainChangeProgress<Eng>>(),
        entry::<ops::describe_domain_config::DescribeDomainConfig<Eng>>(),
        entry::<ops::describe_domain_health::DescribeDomainHealth<Eng>>(),
        entry::<ops::describe_domain_nodes::DescribeDomainNodes<Eng>>(),
        entry::<ops::describe_domains::DescribeDomains<Eng>>(),
        entry::<ops::describe_dry_run_progress::DescribeDryRunProgress<Eng>>(),
        entry::<ops::describe_inbound_connections::DescribeInboundConnections<Eng>>(),
        entry::<ops::describe_insight_details::DescribeInsightDetails<Eng>>(),
        entry::<ops::describe_instance_type_limits::DescribeInstanceTypeLimits<Eng>>(),
        entry::<ops::describe_outbound_connections::DescribeOutboundConnections<Eng>>(),
        entry::<ops::describe_packages::DescribePackages<Eng>>(),
        entry::<ops::describe_reserved_instance_offerings::DescribeReservedInstanceOfferings<Eng>>(),
        entry::<ops::describe_reserved_instances::DescribeReservedInstances<Eng>>(),
        entry::<ops::describe_vpc_endpoints::DescribeVpcEndpoints<Eng>>(),
        entry::<ops::detach_data_source::DetachDataSource<Eng>>(),
        entry::<ops::dissociate_package::DissociatePackage<Eng>>(),
        entry::<ops::dissociate_packages::DissociatePackages<Eng>>(),
        entry::<ops::get_application::GetApplication<Eng>>(),
        entry::<ops::get_capability::GetCapability<Eng>>(),
        entry::<ops::get_compatible_versions::GetCompatibleVersions<Eng>>(),
        entry::<ops::get_data_source::GetDataSource<Eng>>(),
        entry::<ops::get_default_application_setting::GetDefaultApplicationSetting<Eng>>(),
        entry::<ops::get_direct_query_data_source::GetDirectQueryDataSource<Eng>>(),
        entry::<ops::get_domain_maintenance_status::GetDomainMaintenanceStatus<Eng>>(),
        entry::<ops::get_index::GetIndex<Eng>>(),
        entry::<ops::get_migration::GetMigration<Eng>>(),
        entry::<ops::get_package_version_history::GetPackageVersionHistory<Eng>>(),
        entry::<ops::get_upgrade_history::GetUpgradeHistory<Eng>>(),
        entry::<ops::get_upgrade_status::GetUpgradeStatus<Eng>>(),
        entry::<ops::insight_feedback::InsightFeedback<Eng>>(),
        entry::<ops::list_applications::ListApplications<Eng>>(),
        entry::<ops::list_data_source_attachments::ListDataSourceAttachments<Eng>>(),
        entry::<ops::list_data_sources::ListDataSources<Eng>>(),
        entry::<ops::list_direct_query_data_sources::ListDirectQueryDataSources<Eng>>(),
        entry::<ops::list_domain_maintenances::ListDomainMaintenances<Eng>>(),
        entry::<ops::list_domain_names::ListDomainNames<Eng>>(),
        entry::<ops::list_domains_for_package::ListDomainsForPackage<Eng>>(),
        entry::<ops::list_insights::ListInsights<Eng>>(),
        entry::<ops::list_instance_type_details::ListInstanceTypeDetails<Eng>>(),
        entry::<ops::list_migrations::ListMigrations<Eng>>(),
        entry::<ops::list_packages_for_domain::ListPackagesForDomain<Eng>>(),
        entry::<ops::list_scheduled_actions::ListScheduledActions<Eng>>(),
        entry::<ops::list_tags::ListTags<Eng>>(),
        entry::<ops::list_versions::ListVersions<Eng>>(),
        entry::<ops::list_vpc_endpoint_access::ListVpcEndpointAccess<Eng>>(),
        entry::<ops::list_vpc_endpoints::ListVpcEndpoints<Eng>>(),
        entry::<ops::list_vpc_endpoints_for_domain::ListVpcEndpointsForDomain<Eng>>(),
        entry::<ops::purchase_reserved_instance_offering::PurchaseReservedInstanceOffering<Eng>>(),
        entry::<ops::put_default_application_setting::PutDefaultApplicationSetting<Eng>>(),
        entry::<ops::register_capability::RegisterCapability<Eng>>(),
        entry::<ops::reject_inbound_connection::RejectInboundConnection<Eng>>(),
        entry::<ops::remove_tags::RemoveTags<Eng>>(),
        entry::<ops::revoke_vpc_endpoint_access::RevokeVpcEndpointAccess<Eng>>(),
        entry::<ops::rollback_service_software_update::RollbackServiceSoftwareUpdate<Eng>>(),
        entry::<ops::start_domain_maintenance::StartDomainMaintenance<Eng>>(),
        entry::<ops::start_migration::StartMigration<Eng>>(),
        entry::<ops::start_service_software_update::StartServiceSoftwareUpdate<Eng>>(),
        entry::<ops::update_application::UpdateApplication<Eng>>(),
        entry::<ops::update_data_source::UpdateDataSource<Eng>>(),
        entry::<ops::update_direct_query_data_source::UpdateDirectQueryDataSource<Eng>>(),
        entry::<ops::update_domain_config::UpdateDomainConfig<Eng>>(),
        entry::<ops::update_index::UpdateIndex<Eng>>(),
        entry::<ops::update_package::UpdatePackage<Eng>>(),
        entry::<ops::update_package_scope::UpdatePackageScope<Eng>>(),
        entry::<ops::update_scheduled_action::UpdateScheduledAction<Eng>>(),
        entry::<ops::update_vpc_endpoint::UpdateVpcEndpoint<Eng>>(),
        entry::<ops::upgrade_domain::UpgradeDomain<Eng>>(),
    ]
    .into_iter()
    .collect()
}
