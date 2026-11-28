// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-greengrassv2-greengrassv2` service: how it claims requests and how it words the
//! failures the framework hands back.
//!
//! EDIT THIS FILE. It is emitted once with working defaults and is never
//! regenerated, so a choice made here survives adding another service or
//! re-running the generator.
//!
//! `EmulatedService` is what the router mounts (`EmulatedService::new(engine)`).
//! It wraps the generated [`ServiceDefinition`], which carries the model-derived half
//! of this service — its identity, its wire protocol, and its operation routing
//! table — built from the Smithy model at compile time. You do not edit the
//! entry; you edit the behavioral seams below.
//!
//! Five opt-in seams are yours to change; every one defaults to the framework's
//! own behaviour, so an unedited service still routes and answers correctly:
//!
//! - `can_handle` — which requests this service claims. The default matches the
//!   request's resolved route key against this service's signing name. Edit it
//!   for anything the model does not express: a signing name shared by several
//!   services, or routing that keys on the access-key id.
//! - `override_operation` — serve a request the model's routing key cannot name
//!   correctly. The default returns `None`, so every request resolves through the
//!   generated routing table. Return `Some(op)` to route one yourself.
//! - `override_refusal` — how this service words a request the framework refused
//!   before any handler ran. The default returns `None`, keeping the framework's
//!   wording. See the inline note on that method for how to reword.
//! - `override_routing_error` — how this service words a request that reached it
//!   but named no modeled operation. The default returns `None`, so the entry
//!   renders the protocol-correct default (query -> 400 InvalidAction, awsJson ->
//!   404, …). Return `Some(resp)` to answer in your own envelope if desired.
//! - `override_framework_error` — how this service words a transport-level
//!   failure the orchestrator detects up front (e.g. a malformed `aws-chunked`
//!   body). The default returns `None`, keeping the entry's protocol-correct
//!   rendering. Return `Some(resp)` to reword it if desired.
//!
//! The model-derived half of this service — its identity, wire protocol, routing
//! table, and the default error renderers — lives in the generated `ServiceDefinition`
//! and is reached, not edited, through the one `definition` method below. You
//! do not write it; editing the entry would let this file drift from the model.
//!
//! A sixth seam, `override_protocol`, is one this service DOES take, for two framings the
//! model's protocol cannot state. An operation whose output structure declares no member
//! (`TagResource`, `UntagResource`) answers `204 No Content`, and a response at that
//! status carries no body at all rather than the empty JSON object the serializer renders
//! for a members-less output; the correction is not written here — it is
//! [`service_common::no_content_responses::without_content`], which owns that framing for
//! every service that needs it. And a refusal of the LEGACY Greengrass routes this service
//! co-hosts names no error shape at all, which
//! [`crate::state::without_error_shape`] states once for both of those refusal classes.
//! Both are applied at the one place every response of this service passes through rather
//! than per operation.
//!
//! `override_operation` is taken too, for the requests the model's routing key cannot name:
//! the framework's route lookup withholds a route whose `@required` header or query bindings
//! the request does not carry, and a request that leaves such a member out addresses an
//! operation this service HAS. See [`EmulatedService::addressed_operation`], and
//! [`untag_without_keys`] for the one of those requests that is SERVED rather than refused.
//! It is also where a request NAMING a parameter this service could not read a value for is
//! refused, because that is the last point at which an unreadable value is still
//! distinguishable from an absent one ([`unparsable_parameters`]).
//!
//! `override_refusal` is taken for the refusals of the tagging route this service words
//! itself — a `resourceArn` that is no ARN at all, and one that is an ARN of another vendor
//! — which are [`tagging_resource_arn`]; every other boundary refusal keeps the AWS envelope
//! the shared renderer states.

mod tagging_resource_arn;
mod tagging_vendor;
mod unparsable_parameters;
mod untag_without_keys;

use aws_smithy_runtime_api::http::{Request as WireRequest, Response as WireResponse};
use aws_smithy_schema::Schema;
use aws_smithy_schema_server::{ProtocolError, ServerProtocol};
use service_common::no_content_responses::without_content;
use smithy_server_runtime::{
    http, ErasedOperation, Operation, Refusal, Refused, Service,
};
use std::sync::Arc;

/// The `com-amazonaws-greengrassv2-greengrassv2` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// The route each modelled operation binds, read off that operation's own input schema:
    /// what [`EmulatedService::addressed_operation`] resolves a request the framework's own
    /// route lookup named NO operation for.
    modelled_routes: Vec<ModelledRoute>,
    /// `UntagResource` as the one route of this service whose `@required` query parameter
    /// carries a LIST, and so travels as nothing at all when the client's list is empty:
    /// [`untag_without_keys`] says why that request is served rather than refused.
    untag_without_keys: untag_without_keys::UntagWithoutKeys<Eng>,
}

/// One modelled operation and the `@http` route its input schema binds.
///
/// Neither the method nor the URI pattern is written here: both are the schema's own and are
/// read through it, so this table cannot disagree with the model.
struct ModelledRoute {
    /// The operation's modelled name — the routing key the generated table files it under.
    operation: &'static str,
    /// Its input schema, which carries the `@http` method and URI pattern.
    input_schema: &'static Schema,
}

/// Record one [`ModelledRoute`] per `crate::ops::<module>::<Operation>` named, taking each
/// one's name and route from its own `impl Operation` rather than spelling either here.
macro_rules! modelled_routes {
    ($($module:ident :: $operation:ident),+ $(,)?) => {
        vec![$(ModelledRoute {
            operation: <crate::ops::$module::$operation<Eng> as Operation>::NAME,
            input_schema: <crate::ops::$module::$operation<Eng> as Operation>::INPUT_SCHEMA,
        }),+]
    };
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self {
            untag_without_keys: untag_without_keys::UntagWithoutKeys::new(Arc::clone(&engine)),
            entry: crate::ServiceDefinition::new(engine),
            // EVERY operation this model binds a route for, so the rule below is stated
            // once over the whole surface rather than for the one operation that needs it
            // today: an operation whose input declares no `@required` header or query
            // member simply never has its route withheld, and the framework's own lookup
            // resolves it before this table is consulted.
            // `every_modelled_operation_is_listed_as_a_route` pins the list against the
            // model.
            modelled_routes: modelled_routes!(
                associate_service_role_to_account::AssociateServiceRoleToAccount,
                batch_associate_client_device_with_core_device::BatchAssociateClientDeviceWithCoreDevice,
                batch_disassociate_client_device_from_core_device::BatchDisassociateClientDeviceFromCoreDevice,
                cancel_deployment::CancelDeployment,
                create_component_version::CreateComponentVersion,
                create_deployment::CreateDeployment,
                delete_component::DeleteComponent,
                delete_core_device::DeleteCoreDevice,
                delete_deployment::DeleteDeployment,
                describe_component::DescribeComponent,
                disassociate_service_role_from_account::DisassociateServiceRoleFromAccount,
                get_component::GetComponent,
                get_component_version_artifact::GetComponentVersionArtifact,
                get_connectivity_info::GetConnectivityInfo,
                get_core_device::GetCoreDevice,
                get_deployment::GetDeployment,
                get_service_role_for_account::GetServiceRoleForAccount,
                list_client_devices_associated_with_core_device::ListClientDevicesAssociatedWithCoreDevice,
                list_component_versions::ListComponentVersions,
                list_components::ListComponents,
                list_core_devices::ListCoreDevices,
                list_deployments::ListDeployments,
                list_effective_deployments::ListEffectiveDeployments,
                list_installed_components::ListInstalledComponents,
                list_tags_for_resource::ListTagsForResource,
                resolve_component_candidates::ResolveComponentCandidates,
                tag_resource::TagResource,
                untag_resource::UntagResource,
                update_connectivity_info::UpdateConnectivityInfo,
            ),
        }
    }

    /// The operation whose modelled route `req` addresses, for a request the framework's own
    /// route lookup named NO operation for.
    ///
    /// WHY a second lookup: that lookup REJECTS a route whose `@required` header or query
    /// bindings the request does not carry, which is how a model that binds several
    /// operations to one method and URI template tells them apart by the shape of the
    /// request. This model binds each of its routes to exactly ONE operation
    /// (`smithy_query(query="digest")`), so for this service the rejection turns an
    /// INCOMPLETE request into a request for an operation that does not exist — a 501 for a
    /// route the service serves. The operation the request addressed is resolved here
    /// instead, and answers the request itself: it refuses the member the request left out
    /// through that member's own modelled constraints, or — where the absence IS the value,
    /// as it is for a repeated query parameter carrying an empty list — serves it
    /// ([`untag_without_keys`]).
    ///
    /// Each candidate's method and URI pattern are the ones its own input schema carries,
    /// matched by `service_common::addresses_modeled_http_route`; a request TWO of them could
    /// claim resolves to neither, leaving the framework's own answer in place, since choosing
    /// between them is what the bindings the request omitted are for. An operation whose
    /// input declares no member at all carries no `@http` binding on its input schema and so
    /// is never resolved here — nor does it need to be, since it has no `@required` binding
    /// for a request to leave out.
    fn addressed_operation(
        &self,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<&'static str> {
        let mut matching = self
            .modelled_routes
            .iter()
            .filter(|route| service_common::addresses_modeled_http_route(req, route.input_schema));
        let addressed = matching.next()?;
        matching.next().is_none().then_some(addressed.operation)
    }
}

/// The protocol this service answers modelled requests in: the model's own, with the two
/// framing corrections the module docs state.
///
/// Implemented on the service itself because the model-derived protocol it delegates to is
/// owned by the generated entry and reached by reference (`self.entry.protocol()`), and
/// [`Service::override_protocol`] hands back a borrow of `&self` — so there is one protocol
/// instance, the generated one, and no second routing table to drift from the model.
impl<Eng: Send + Sync + 'static> ServerProtocol for EmulatedService<Eng> {
    type Request = WireRequest;
    type Response = WireResponse;

    fn protocol_id(&self) -> &aws_smithy_schema::ShapeId {
        self.entry.protocol().protocol_id()
    }

    fn routing_key<'a>(&self, request: &'a WireRequest) -> Result<&'a str, ProtocolError> {
        self.entry.protocol().routing_key(request)
    }

    fn deserialize_input<'a>(
        &self,
        request: &'a WireRequest,
        input_schema: &Schema,
    ) -> Result<Box<dyn aws_smithy_schema::serde::ShapeDeserializer + 'a>, ProtocolError> {
        self.entry
            .protocol()
            .deserialize_input(request, input_schema)
    }

    fn serialize_output(
        &self,
        output: &dyn aws_smithy_schema::serde::SerializableStruct,
        output_schema: &Schema,
        operation_name: &str,
        success_status: u16,
    ) -> Result<WireResponse, ProtocolError> {
        let response = self.entry.protocol().serialize_output(
            output,
            output_schema,
            operation_name,
            success_status,
        )?;
        // The status and every other header are the protocol's: only an answer that
        // carries no content is emptied of the body rendered for it.
        Ok(without_content(response))
    }

    fn serialize_error(
        &self,
        error: &dyn aws_smithy_schema::serde::SerializableStruct,
        error_schema: &Schema,
        error_code: &str,
        http_status: u16,
    ) -> Result<WireResponse, ProtocolError> {
        let response = self
            .entry
            .protocol()
            .serialize_error(error, error_schema, error_code, http_status)?;
        // A refusal the LEGACY Greengrass surface words itself names no error shape, so the
        // shared renderer's two namings of one are dropped from it; every modelled failure
        // is returned exactly as that renderer wrote it. See
        // [`crate::state::without_error_shape`] — the one place that envelope is stated,
        // reached here because this is the one seam every response of this service passes
        // through.
        let response = crate::state::without_error_shape(error_schema, response);
        // Every modelled failure of this service carries its error envelope at a 4xx or
        // 5xx, so nothing here is emptied; the same call is made on both paths rather
        // than leaving one of them to answer differently.
        Ok(without_content(response))
    }

    fn serialize_event_stream_message(
        &self,
        event_type: &str,
        is_exception: bool,
        event: &dyn aws_smithy_schema::serde::SerializableStruct,
        event_schema: &Schema,
    ) -> Result<WireResponse, ProtocolError> {
        self.entry.protocol().serialize_event_stream_message(
            event_type,
            is_exception,
            event,
            event_schema,
        )
    }
}

/// `ServerProtocol` requires `Debug`; the generated entry is not `Debug`, so this names
/// the protocol being served rather than the service's internals.
impl<Eng: Send + Sync + 'static> std::fmt::Debug for EmulatedService<Eng> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EmulatedService")
            .field("protocol", &self.entry.protocol().protocol_id())
            .finish()
    }
}

impl<Eng: Send + Sync + 'static> Service for EmulatedService<Eng> {
    fn definition(&self) -> &dyn smithy_server_runtime::ErasedServiceDefinition {
        // The model-derived half of this service — identity, protocol, routing
        // table, and the default error renderers — lives in the generated entry.
        // The router reads it through this one accessor. Do not edit the entry.
        &self.entry
    }

    fn can_handle(&self, parts: &http::request::Parts) -> bool {
        // This service signs as `greengrass` — the signing name it SHARES with the
        // 2017-06-07 Greengrass API (`services/…-greengrass-greengrass`), whose model
        // declares the same endpoint prefix, the same ARN namespace and the same sigv4
        // name, because AWS serves both API versions from ONE endpoint. The router
        // resolves a request's routing key from the SigV4 credential scope first, so both
        // services see the key `greengrass`: claiming on the signing name alone makes
        // each claim the other's traffic, and the router — which refuses rather than
        // guesses — answers `500 AmbiguousService` for every Greengrass request of either
        // version, a code no AWS model carries.
        //
        // Nor can a first path segment separate them: both models publish `greengrass`
        // and `tags` and nothing else.
        //
        // What separates them is what AWS itself dispatches on: the `restJson1` route
        // each request addresses. So this service claims a request whose routing key
        // names this endpoint and whose method + URI its OWN model binds a route for, by
        // the one rule `service_common::greengrass` states for the family — which also
        // names the single claimant of the few routes the two models bind identically.
        service_common::greengrass::claims_request(
            parts,
            self.definition(),
            crate::ROUTE_PATH_SEGMENTS,
        )
    }

    fn override_operation(
        &self,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<&dyn ErasedOperation> {
        // Mostly NOT a re-route: a request the protocol resolves a routing key for is left
        // to the generated routing table, which is what `None` here means.
        //
        // First, though, the one decision about a request that cannot be made later: a
        // parameter the body NAMES with a value the modelled member cannot be read from
        // never became a value at all, and this service's codec reads it as the member
        // being absent — so no handler can still tell the two apart. Such a request is
        // answered here, naming that parameter ([`unparsable_parameters`]).
        if unparsable_parameters::refuses(req) {
            return Some(&unparsable_parameters::UNPARSABLE_PARAMETERS);
        }

        // A request the protocol names NO key for is given one more chance, on the route its
        // method and URI address ([`Self::addressed_operation`]): a `@required` header or
        // query member a request leaves out is a request for an operation this service HAS,
        // not one it does not have. The operation served is the one the generated table
        // holds for that name — so it reads and writes the one store and answers in the one
        // envelope — and it refuses the member the request left out through that member's
        // own modelled constraints.
        //
        // `UntagResource` is the one exception, because `tagKeys` is the one `@required`
        // query member of this model that carries a LIST, and a list of no entries is sent
        // as no parameter at all: that request named every key it meant to and is served.
        // See [`untag_without_keys`].
        if self.entry.protocol().routing_key(req).is_ok() {
            return None;
        }
        let addressed = self.addressed_operation(req)?;
        if addressed == untag_without_keys::OPERATION {
            return Some(&self.untag_without_keys);
        }
        self.entry.routing_table().get(addressed)
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // The tagging route PARSES its `resourceArn` before it looks for any resource: a
        // value that is no ARN at all, and an ARN of another service, are each refused as a
        // bad request about that value rather than as a constraint the model states
        // ([`tagging_resource_arn`]). Every other refusal keeps the envelope below.
        if let Some(refused) = tagging_resource_arn::resource_arn_refusal(refusal) {
            return Some(refused);
        }
        // AWS answers a value its own model does not admit as `400 ValidationException`
        // with the envelope `N validation error(s) detected: Value '<value>' at
        // '<member>' failed to satisfy constraint: <requirement>` — the envelope the
        // framework already renders, except that a `@pattern` requirement is quoted
        // WITHOUT the anchors a whole-value pattern wraps it in. That rendering is stated
        // once for every service on a JSON protocol in
        // `service_common::validation_message`, so the sentence this service answers and
        // the one the framework checks against cannot drift. The CHECK still uses the
        // model's pattern exactly as written; only the quoting changes.
        ::service_common::validation_message::aws_validation_refusal(refusal)
    }

    fn override_protocol(&self) -> Option<&dyn smithy_server_runtime::ServerHttpProtocol> {
        // Every response a modelled operation of this service produces is serialized by
        // the protocol returned here. See the module docs: it is the model's own
        // protocol, with one correction — an answer that carries no content carries no
        // body and states `content-length: 0`, instead of the empty JSON object the
        // serializer renders for a members-less output.
        Some(self)
    }

    fn override_routing_error(
        &self,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<aws_smithy_runtime_api::http::Response> {
        // The router calls this when a request reached this service but named no
        // modeled operation — neither `override_operation` nor the generated
        // routing table resolved it. Returning `None` renders the protocol-correct
        // default for this service's envelope (query -> 400 InvalidAction text/xml,
        // awsJson -> 404, rpcv2Cbor -> 404, rest -> 501). Return `Some(resp)` to
        // word that routing failure in your own envelope instead.
        let _ = req;
        None
    }

    fn override_framework_error(
        &self,
        error: &aws_smithy_schema_server::ProtocolError,
    ) -> Option<aws_smithy_runtime_api::http::Response> {
        // The router calls this on a transport-level failure it detects up front —
        // before (or instead of) naming an operation — such as a malformed
        // `aws-chunked` body. Returning `None` keeps the entry's protocol-correct
        // rendering. Return `Some(resp)` to reword that failure in your own
        // envelope. The error is borrowed, so the default renderer can still
        // consume it on `None`.
        let _ = error;
        None
    }
}

#[cfg(test)]
mod tests {
    //! The half of the rule above that needs no request: that the route table this file
    //! holds spans the model's whole surface, so the fallback lookup can resolve any
    //! operation whose route the framework withholds — not only the one that needs it today.

    use super::*;

    /// This service's Smithy model, as it ships beside this crate. Read at test time rather
    /// than embedded: it is JSON that no non-test build needs.
    const MODEL_PATH: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/smithy-model/com-amazonaws-greengrassv2-greengrassv2.json"
    );

    /// How many operations this model declares, as a second, independent statement of this
    /// service's surface: a model that grows or shrinks fails here rather than quietly
    /// leaving the new operation out of the table.
    const MODELLED_OPERATIONS: usize = 29;

    /// RULE applied to the whole surface: every operation the model declares is listed as a
    /// [`ModelledRoute`], under the same key the generated routing table is keyed by — and
    /// every operation whose route that lookup can WITHHOLD (one declaring a `@required`
    /// member bound to the query string or to a header) has the `@http` method and URI on its
    /// input schema that [`EmulatedService::addressed_operation`] matches a request against.
    ///
    /// The names come from the MODEL rather than from a second hand-written list, so an
    /// operation added to the model and not to `modelled_routes!` fails here instead of
    /// silently answering `501` for an incomplete request.
    #[test]
    fn every_modelled_operation_is_listed_as_a_route() {
        let model = std::fs::read_to_string(MODEL_PATH)
            .expect("the model that generated this crate ships beside it");
        let model: serde_json::Value =
            serde_json::from_str(&model).expect("the model that generated this crate is JSON");
        let shapes = model["shapes"]
            .as_object()
            .expect("a Smithy model declares shapes");
        let operations: Vec<(&str, &serde_json::Value)> = shapes
            .iter()
            .filter(|(_, shape)| shape["type"] == "operation")
            .filter_map(|(id, shape)| Some((id.rsplit('#').next()?, shape)))
            .collect();
        assert_eq!(
            operations.len(),
            MODELLED_OPERATIONS,
            "case \"the model's surface\": expected this service's {MODELLED_OPERATIONS} \
             operations, got {}",
            operations.len()
        );

        /// Whether `operation`'s input declares a `@required` member bound to the query
        /// string or to a header — the members whose absence makes the framework's route
        /// lookup withhold this operation's route.
        fn binds_a_required_parameter(
            shapes: &serde_json::Map<String, serde_json::Value>,
            operation: &serde_json::Value,
        ) -> bool {
            let input = operation["input"]["target"].as_str().unwrap_or_default();
            let Some(members) = shapes
                .get(input)
                .and_then(|shape| shape["members"].as_object())
            else {
                return false;
            };
            members.values().any(|member| {
                let traits = &member["traits"];
                traits.get("smithy.api#required").is_some()
                    && (traits.get("smithy.api#httpQuery").is_some()
                        || traits.get("smithy.api#httpHeader").is_some())
            })
        }

        let service = EmulatedService::new(Arc::new(engine::InMemoryEngine::new()));
        assert_eq!(
            service.modelled_routes.len(),
            MODELLED_OPERATIONS,
            "case \"the route table's size\": expected one route per modelled operation, got {}",
            service.modelled_routes.len()
        );
        for (operation, shape) in operations {
            let route = service
                .modelled_routes
                .iter()
                .find(|route| route.operation == operation);
            assert!(
                route.is_some(),
                "case {operation:?}: every modelled operation is listed in modelled_routes!; \
                 this one is missing, so an incomplete request for it is answered as a route \
                 this service does not have"
            );
            if binds_a_required_parameter(shapes, shape) {
                assert!(
                    route.is_some_and(|route| route.input_schema.http().is_some()),
                    "case {operation:?}: its route can be withheld for a parameter the request \
                     left out, so its input schema must carry the `@http` method and URI the \
                     fallback lookup matches on, and this one carries none"
                );
            }
            assert!(
                service.entry.routing_table().get(operation).is_some(),
                "case {operation:?}: the generated table keys an operation by its shape name; \
                 this one resolved to nothing, so the table's key spelling is wrong"
            );
        }
    }
}
