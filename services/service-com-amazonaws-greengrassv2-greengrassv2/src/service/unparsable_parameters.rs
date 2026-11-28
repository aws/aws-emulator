// SPDX-License-Identifier: CC0-1.0

//! The parameters of the LEGACY Greengrass surface a request NAMED and this service could
//! not read a value for.
//!
//! Two refusals of that surface happen before any endpoint list is stored, and they are not
//! one class:
//!
//! * a parameter the body names with a value the modelled type of the member it is bound to
//!   cannot be READ from — a string where the model declares a list, a number where it
//!   declares a string, a fraction where it declares an integer — never became a value at
//!   all. The service reports it by NAMING that parameter
//!   ([`crate::state::unparsable_parameter_message`]), because the caller has to be told
//!   which parameter of its own request it must correct; and
//! * a parameter that WAS read and carries something the operation does not take — an entry
//!   of the endpoint list naming no endpoint — is refused about the request body
//!   ([`crate::state::UNREADABLE_REQUEST_BODY_MESSAGE`]), naming nothing of it. That one is
//!   the operation's own decision and stays in its handler, which is where the stored list
//!   is decided.
//!
//! The framework cannot tell the two apart for this service: its JSON codec reads an entry
//! whose member carries the wrong class as an entry with that member absent, so by the time
//! the handler is reached a port sent as `"8883"` is indistinguishable from a request that
//! named no port, and both would be answered as the second class. The separation is
//! therefore made here, at the one seam a service has to the request before dispatch
//! ([`smithy_server_runtime::Service::override_operation`]) — the same seam
//! `services/service-com-amazonaws-codebuild-codebuild-20161006` makes its own
//! deserialization decision at.
//!
//! Nothing about the model is written down here. Which key names which member, and what
//! each member is read from, come off the operation's own input schema and off the schema of
//! the entry its list carries; WHETHER a value can be read into a member is the workspace's
//! one statement of that question
//! ([`service_common::json_wire_values::reads_as`]), so this layer cannot disagree with the
//! request layers of the other services that answer it. Only the NAME the service publishes
//! a parameter under, and the words of the sentence, are this surface's own.
//!
//! Being before dispatch also decides the ORDER of the two refusals a request can earn at
//! once: a value that never became a value is refused before the values that DID parse are
//! judged against their modelled constraints — the same order the framework's own walk takes
//! (deserialize, then validate), and the only one available to a request that has no input
//! for a constraint to be applied to. `connectivity_info_request_contract` pins it.

use aws_smithy_runtime_api::http::{Request as WireRequest, Response as WireResponse};
use aws_smithy_schema::Schema;
use serde_json::Value;
use service_common::json_wire_values::{member_by_wire_name, reads_as};
use smithy_server_runtime::{
    BoxFuture, ErasedOperation, OperationContext, OperationError, Service,
};

/// One parameter of this surface a refusal names, and how a value for it is read.
struct NamedParameter {
    /// The route the parameter belongs to, as the operation's own input schema — which
    /// carries the `@http` method and URI a request is matched against, and the members the
    /// body's keys name.
    input: &'static Schema,
    /// The member of that input, as the MODEL names it (not as the wire spells it: the wire
    /// key is resolved through the member's own `@jsonName`).
    member: &'static str,
    /// The schema one ENTRY of the parameter's value is read against, for a parameter whose
    /// value is a list of structures; `None` for one whose value the member's own type
    /// decides on its own.
    ///
    /// A member `Schema` in this framework carries its target's KIND but not its target's
    /// schema, so the entry's shape is named here to read one level further than the member
    /// alone allows — which is the level the entries of this surface's one list parameter
    /// live at.
    entry: Option<&'static Schema>,
    /// The name the SERVICE publishes the parameter under, which the refusal states.
    published: &'static str,
}

/// Every parameter of this surface whose unreadable value is reported by naming it.
///
/// One entry: the endpoint list `UpdateConnectivityInfo` publishes. The sibling legacy route
/// (`PUT /greengrass/servicerole`) carries one required body parameter too, and the value it
/// cannot take is reported by its own recorded sentence about the role ARN
/// ([`crate::state::INVALID_ROLE_ARN_MESSAGE`]) rather than by naming the parameter, so it is
/// not listed here.
const NAMED_PARAMETERS: &[NamedParameter] = &[NamedParameter {
    input: crate::ops::types::UPDATE_CONNECTIVITY_INFO_INPUT_SCHEMA,
    member: "connectivityInfo",
    entry: Some(&crate::ops::types::ConnectivityInfo::SCHEMA),
    published: crate::state::CONNECTIVITY_INFO_PARAMETER,
}];

/// Whether this layer refuses `req`: its body names a parameter of [`NAMED_PARAMETERS`] with
/// a value the modelled type cannot be read from.
pub(super) fn refuses(req: &WireRequest) -> bool {
    !unparsable_parameters(req).is_empty()
}

/// The parameters of `req` nothing readable arrived under, in the order the route declares
/// them, as the sentence names them.
///
/// Empty is "nothing for THIS layer to refuse", which covers several outcomes deliberately:
/// every value the body carries reads; the body is not a JSON object at all, so no parameter
/// of it can be named and the framework's own rendering of a body it cannot read stands; the
/// body names the parameter as `null` or leaves it out, which is absence rather than an
/// unreadable value and is the operation's own refusal to word; or the request addresses no
/// route this layer speaks for.
///
/// Asked once per decision rather than memoised: the route match is the cheap guard that
/// keeps every request for another operation of this service from being read at all, and the
/// body of a request this route DOES address is the small document the codec is about to read
/// anyway.
fn unparsable_parameters(req: &WireRequest) -> Vec<&'static str> {
    // The route is matched FIRST, off each parameter's own input schema, so a request for any
    // other operation of this service — whose body this layer has nothing to say about — is
    // never read at all.
    let addressed: Vec<&NamedParameter> = NAMED_PARAMETERS
        .iter()
        .filter(|parameter| service_common::addresses_modeled_http_route(req, parameter.input))
        .collect();
    if addressed.is_empty() {
        return Vec::new();
    }
    let body = req.body().bytes().unwrap_or_default();
    let Ok(Value::Object(carried)) = serde_json::from_slice::<Value>(body) else {
        return Vec::new();
    };
    addressed
        .into_iter()
        .filter(|parameter| names_an_unreadable_value(parameter, &carried))
        .map(|parameter| parameter.published)
        .collect()
}

/// Whether the body carries `parameter` under the key its member is spelled as on the wire,
/// with a value that member cannot be read from.
fn names_an_unreadable_value(
    parameter: &NamedParameter,
    carried: &serde_json::Map<String, Value>,
) -> bool {
    carried.iter().any(|(key, held)| {
        member_by_wire_name(parameter.input, key)
            .filter(|member| member.member_name() == Some(parameter.member))
            .is_some_and(|member| !reads_into(member, parameter.entry, held))
    })
}

/// Whether `held` reads into the member `schema` declares, and — for a list whose entries
/// have a schema of their own — whether every entry of it reads too.
fn reads_into(schema: &Schema, entry: Option<&Schema>, held: &Value) -> bool {
    if !reads_as(schema.shape_type(), held) {
        return false;
    }
    match (entry, held) {
        (Some(entry), Value::Array(entries)) => {
            entries.iter().all(|held| reads_into_entry(entry, held))
        }
        _ => true,
    }
}

/// Whether one entry reads into the structure `schema` declares: the entry itself is of the
/// class that structure is read from, and every key of it that names a member carries a value
/// that member can be read from.
///
/// A key naming no member is ignored, exactly as the codec ignores it.
fn reads_into_entry(schema: &Schema, held: &Value) -> bool {
    if !reads_as(schema.shape_type(), held) {
        return false;
    }
    let Value::Object(carried) = held else {
        return true;
    };
    carried.iter().all(|(key, held)| {
        member_by_wire_name(schema, key).is_none_or(|member| reads_as(member.shape_type(), held))
    })
}

/// The operation this service serves for a request naming a parameter it could not read a
/// value for.
///
/// An [`ErasedOperation`] rather than a response built in
/// [`smithy_server_runtime::Service::override_operation`], because that seam hands the router
/// a BORROWED operation. It holds no state: the refusal is re-derived from the same request
/// the router passes back into [`ErasedOperation::run`], so one value serves every such
/// request.
pub(super) struct UnparsableParameters;

/// The one value of [`UnparsableParameters`] this service hands back.
pub(super) static UNPARSABLE_PARAMETERS: UnparsableParameters = UnparsableParameters;

impl ErasedOperation for UnparsableParameters {
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r WireRequest,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, WireResponse> {
        Box::pin(async move {
            let definition = svc.definition();
            // The same decision `override_operation` made, re-derived from the same request:
            // this operation is reached only when it answered `Some`, so the refusal is found
            // again. Were it somehow not, the request is served exactly as the generated
            // routing table serves it rather than refused in words nothing decided — and
            // that table is the whole of the fallback, because every route this layer matches
            // resolves its own routing key, so no sibling override of
            // [`super::EmulatedService::override_operation`] is bypassed by it.
            let named = unparsable_parameters(req);
            if named.is_empty() {
                return match definition
                    .protocol()
                    .routing_key(req)
                    .ok()
                    .and_then(|key| definition.routing_table().get(key))
                {
                    Some(op) => op.run(svc, req, ctx).await,
                    None => definition.routing_error_response(req),
                };
            }

            // The refusal is the legacy surface's own class, carried on the operation's own
            // error channel and rendered through the protocol this service serves — so it
            // answers with the status, the `Message` member and the absence of any error
            // shape that every other refusal of this surface answers with, and cannot drift
            // from the one the handler words for a list it DID read.
            let refusal: crate::ops::update_connectivity_info::UpdateConnectivityInfoError =
                crate::state::LegacyRefusal::bad_request(
                    crate::state::unparsable_parameter_message(&named),
                )
                .into();
            let protocol = svc
                .override_protocol()
                .unwrap_or_else(|| definition.wire_protocol(req).protocol());
            protocol
                .serialize_error(
                    refusal.as_serializable(),
                    refusal.variant_schema(),
                    refusal.shape_name(),
                    refusal.http_status(),
                )
                // The protocol writes one string member into its own envelope, so this cannot
                // fail; a codec that somehow could not is answered in the envelope the
                // framework renders a malformed request in.
                .unwrap_or_else(|error| definition.framework_error_response(error, req))
        })
    }

    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        // `None` is "defer to the modelled operation the routing key names", so a request
        // this layer refuses is authenticated exactly as the operation it addressed is: a
        // value the service could not read is no reason to answer an unsigned caller.
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RULE: a parameter of the table is named exactly when the body carries it with a value
    /// the modelled member — or, for its entries, the modelled entry — cannot be read from.
    ///
    /// Asked of the body walk directly, so the rows can span the classes a value can arrive
    /// as without building a request for each; the suites under `tests/all/` drive the same
    /// rows through the front door.
    #[test]
    fn a_value_is_unreadable_only_where_the_model_cannot_read_it() {
        let parameter = &NAMED_PARAMETERS[0];
        // (case, the request document, whether the parameter is named as unparsable)
        let rows: [(&str, &str, bool); 12] = [
            ("the list as a list of entries", r#"{"ConnectivityInfo":[{"Id":"a"}]}"#, false),
            ("the list as a string", r#"{"ConnectivityInfo":"a"}"#, true),
            ("the list as a number", r#"{"ConnectivityInfo":1}"#, true),
            ("the list as a flag", r#"{"ConnectivityInfo":true}"#, true),
            ("the list as one entry", r#"{"ConnectivityInfo":{"Id":"a"}}"#, true),
            ("the list as null", r#"{"ConnectivityInfo":null}"#, false),
            ("the list left out", r#"{}"#, false),
            ("an entry as a string", r#"{"ConnectivityInfo":["a"]}"#, true),
            ("an entry whose id is a number", r#"{"ConnectivityInfo":[{"Id":1}]}"#, true),
            (
                "an entry whose port is a string",
                r#"{"ConnectivityInfo":[{"PortNumber":"8883"}]}"#,
                true,
            ),
            (
                "an entry whose port is a whole number",
                r#"{"ConnectivityInfo":[{"PortNumber":8883}]}"#,
                false,
            ),
            (
                "an entry carrying a key the model declares no member for",
                r#"{"ConnectivityInfo":[{"Unmodelled":1}]}"#,
                false,
            ),
        ];
        for (case, body, unparsable) in rows {
            let Ok(Value::Object(carried)) = serde_json::from_str::<Value>(body) else {
                panic!("case {case:?}: the row's document is a JSON object");
            };
            assert_eq!(
                names_an_unreadable_value(parameter, &carried),
                unparsable,
                "case {case:?}: expected unparsable = {unparsable}, got {}",
                names_an_unreadable_value(parameter, &carried)
            );
        }
    }

    /// Every parameter of the table names a member its route's input really declares, so a
    /// model whose member is renamed fails here rather than quietly refusing nothing.
    #[test]
    fn every_named_parameter_is_a_member_of_its_route() {
        for parameter in NAMED_PARAMETERS {
            let declared = parameter
                .input
                .members()
                .iter()
                .any(|member| member.member_name() == Some(parameter.member));
            assert!(
                declared,
                "case {:?}: the input of this route declares no such member",
                parameter.member
            );
            assert!(
                parameter.input.http().is_some(),
                "case {:?}: its route is matched on the `@http` method and URI its input \
                 schema carries, and this one carries none",
                parameter.member
            );
        }
    }
}
