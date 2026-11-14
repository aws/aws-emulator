// SPDX-License-Identifier: CC0-1.0

//! `UpdateConfigurationProfile` refusing a request that asks a configuration profile to
//! become a different `Type`.
//!
//! A profile's `Type` is fixed when the profile is created: it is what the profile's
//! content MEANS (`ConfigurationProfile::served_content`), so a stored configuration
//! cannot be reinterpreted by renaming its kind. `UpdateConfigurationProfileRequest`
//! declares no `Type` member at all
//! (`smithy_query(query="operation",operation="UpdateConfigurationProfile")`), so an SDK
//! cannot send one — but a hand-built request can put the key in the body anyway, and
//! AppConfig INSPECTS it: a body naming a type the profile is not is refused WHOLE, with
//! the modelled `BadRequestException` (HTTP 400), and none of the members the same
//! request carried is applied.
//!
//! This is the one rule in this crate that cannot live in the operation's handler: the
//! handler is given the DESERIALIZED input, from which a member the model does not
//! declare has already been dropped, so the only layer that can still see the key is one
//! holding the request itself. That is this wrapper, in front of the same
//! [`super::InServiceDialect`] every other operation is served through, so an
//! unrefused request is handled and worded exactly as before.
//!
//! A body naming the type the profile ALREADY is asks for no change, so it is not
//! refused: the request is served as a request that omitted the key, which is what the
//! refusal's own sentence says it is about. `LocationUri` — the profile's other member
//! fixed at creation — is not refused here: the operation models no member for it either,
//! and AppConfig is not recorded refusing a body that names it, so it keeps being ignored
//! the way every other undeclared key is.

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use serde_json::Value;
use smithy_server_runtime::{
    BoxFuture, DeserializableStruct, ErasedOperation, Operation, OperationContext,
    OperationRoutingTable, Refused, Service,
};
use std::sync::Arc;

/// The key a body carries the profile's kind under. A restJson1 member's wire name is its
/// Smithy member name, and `ConfigurationProfile$Type` declares no `jsonName`, so this is
/// the spelling both the response and such a hand-built request use.
const TYPE_MEMBER: &str = "Type";

/// The sentence AppConfig refuses the request with. It states the rule and names no value
/// from the request: what is refused is that the profile's kind would change at all, not
/// which kind was named.
const IMMUTABLE_TYPE_MESSAGE: &str =
    "Changing the Type of this configuration profile is not supported.";

/// The operation this wrapper guards, for the one engine the service was built from.
type Update<Eng> = crate::ops::update_configuration_profile::UpdateConfigurationProfile<Eng>;

/// File `UpdateConfigurationProfile` in `table` behind this guard, under the operation's
/// own modelled name — the routing key the protocol resolves, exactly as
/// [`super::served`] files every other operation, recording the same operation's own
/// `@http` route in `routes` as that function does.
pub(super) fn served<Eng: Send + Sync + 'static>(
    table: &mut OperationRoutingTable,
    routes: &mut Vec<super::ModelledRoute>,
    engine: Arc<Eng>,
) {
    let operation = Update::new(Arc::clone(&engine));
    routes.push(super::ModelledRoute {
        operation: <Update<Eng> as Operation>::NAME,
        input_schema: <Update<Eng> as Operation>::INPUT_SCHEMA,
    });
    table.add_operation(
        <Update<Eng> as Operation>::NAME,
        Box::new(ImmutableProfileType {
            inner: super::InServiceDialect::new(operation),
            engine,
        }),
    );
}

/// `UpdateConfigurationProfile`, served only for a request that does not ask the profile
/// to change its `Type`.
struct ImmutableProfileType<Eng> {
    /// The operation as every other one is served: this service's dialect around the
    /// generated request walk.
    inner: super::InServiceDialect<Update<Eng>>,
    /// The shared engine, so the stored profile's kind can be read — the refusal is about
    /// the difference between what the request names and what the profile IS.
    engine: Arc<Eng>,
}

impl<Eng: Send + Sync + 'static> ImmutableProfileType<Eng> {
    /// The sentence this request is refused with, or `None` when it asks for no change of
    /// the profile's kind: it carries no `Type` at all, it names the kind the profile
    /// already is, or it addresses no profile this account holds — a miss the operation
    /// itself reports, as a `ResourceNotFoundException`, rather than one reported here as
    /// a refusal.
    fn type_change_refusal(&self, svc: &dyn Service, req: &RtRequest) -> Option<String> {
        let requested = requested_type(req)?;
        let (application_id, configuration_profile_id) = self.addressed_profile(svc, req)?;
        let state = crate::state::service_state(&*self.engine)?;
        let store = state.lock();
        let profile = store.configuration_profile(&application_id, &configuration_profile_id)?;
        (profile.profile_type != requested).then(|| IMMUTABLE_TYPE_MESSAGE.to_string())
    }

    /// The application and configuration profile ids this request addresses, read through
    /// the SAME codec and input schema the generated walk reads them with, so this
    /// wrapper cannot resolve a different profile from the one the handler would.
    ///
    /// `None` for a request the codec cannot read at all; such a request is handed on and
    /// refused by the walk that reads it next.
    fn addressed_profile(&self, svc: &dyn Service, req: &RtRequest) -> Option<(String, String)> {
        // The protocol is read, used and dropped inside this one function, so nothing
        // borrowed from the service definition is held across the caller's await:
        // `&dyn ErasedServiceDefinition` is not `Sync`.
        let protocol = svc.definition().protocol();
        let mut deserializer = protocol
            .deserialize_input(req, <Update<Eng> as Operation>::INPUT_SCHEMA)
            .ok()?;
        let input = <crate::ops::types::UpdateConfigurationProfileInput as DeserializableStruct>::deserialize(
            &mut *deserializer,
        )
        .ok()?;
        Some((input.application_id, input.configuration_profile_id))
    }
}

/// The kind the body asks the profile to be, or `None` when it asks for none: a body that
/// is not a JSON object, one carrying no `Type` key, or one carrying it as `null` — which
/// a client writes for a member it is not sending.
///
/// A value that is not a string is reported as the JSON text it arrived as, which no
/// stored kind equals, so a body naming the profile's kind in some other JSON type is
/// refused rather than read as the kind it resembles.
fn requested_type(req: &RtRequest) -> Option<String> {
    let body = req.body().bytes()?;
    let Ok(Value::Object(members)) = serde_json::from_slice::<Value>(body) else {
        return None;
    };
    match members.get(TYPE_MEMBER)? {
        Value::Null => None,
        Value::String(requested) => Some(requested.clone()),
        other => Some(other.to_string()),
    }
}

impl<Eng: Send + Sync + 'static> ErasedOperation for ImmutableProfileType<Eng> {
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r RtRequest,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        Box::pin(async move {
            if let Some(message) = self.type_change_refusal(svc, req) {
                // The code is the one this service names for every request it refuses as
                // a client error: `BadRequestException`, which this operation models
                // (HTTP 400). The status and the envelope stay the framework's, through
                // the shared renderer, and the detail text is moved into the `Message`
                // member this service's model declares — the same two steps
                // [`super::constraint_refusal`] answers a refused constraint through.
                return super::with_service_detail_member(
                    service_common::front_door::worded_rejection(
                        svc,
                        req,
                        Vec::new(),
                        Refused {
                            code: super::CONSTRAINT_REFUSAL_CODE.to_string(),
                            message,
                        },
                    ),
                );
            }
            self.inner.run(svc, req, ctx).await
        })
    }

    /// The wrapped operation's modelled auth schemes: guarding an operation must not
    /// change which schemes sign it.
    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        self.inner.auth_schemes()
    }
}
