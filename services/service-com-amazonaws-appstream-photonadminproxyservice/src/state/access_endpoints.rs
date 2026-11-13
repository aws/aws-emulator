// SPDX-License-Identifier: CC0-1.0

//! What a write of an `AccessEndpoints` list can settle from the REQUEST alone.
//!
//! An `AccessEndpoints` entry references an interface VPC endpoint by id, and whether the
//! account has that endpoint is the owning service's answer, asked over the hop in
//! [`super::vpc_access_endpoints`]. Two things about such a list need no hop at all,
//! because they are properties of the request: an entry that carries no `VpceId`
//! references no endpoint for anything to resolve, and a list naming one `EndpointType`
//! twice asks for two endpoints of a type a stack is reached through once. Both are
//! answered here, before the hop is made, so a request that cannot name a valid reference
//! is refused without asking another service about it.
//!
//! The readings are stated once and shared by every write of the member, so two operations
//! cannot answer one condition two ways.

use crate::ops::types::AccessEndpoint;

/// The input member an `AccessEndpoints` list arrives under, as the model spells it; a
/// refusal's member path states it in lowerCamel
/// ([`::aws_smithy_schema_server::constraints::path::member`]).
const ACCESS_ENDPOINTS_MEMBER: &str = "AccessEndpoints";

/// The member of one entry that names the endpoint, as the model spells it.
const VPCE_ID_MEMBER: &str = "VpceId";

/// AWS's `InvalidParameterCombinationException` sentence for an `AccessEndpoints` list
/// carrying two entries of one `EndpointType`: each type may appear at most once, and the
/// repeat is refused rather than de-duplicated. The only variable part is the repeated
/// `EndpointType` the request asked for.
pub fn duplicate_access_endpoint_type_message(endpoint_type: &str) -> String {
    format!(
        "You specified duplicate VPC endpoint types in the request. \
         Specify {endpoint_type} once, and try again."
    )
}

/// The `EndpointType` an `AccessEndpoints` list names twice, if any: the first value a
/// later entry repeats, in the order the request listed them.
pub fn duplicate_access_endpoint_type(endpoints: &[AccessEndpoint]) -> Option<String> {
    let mut seen: Vec<&str> = Vec::with_capacity(endpoints.len());
    for endpoint in endpoints {
        let endpoint_type = endpoint.endpoint_type.as_str();
        if seen.contains(&endpoint_type) {
            return Some(endpoint_type.to_string());
        }
        seen.push(endpoint_type);
    }
    None
}

/// The `ValidationException` message an `AccessEndpoints` list is refused with when an
/// entry carries no `VpceId` — one clause per such entry, naming its 1-based position, in
/// the framework's own envelope. `None` when every entry names an endpoint.
///
/// An entry without an id REFERENCES no endpoint, so there is nothing to resolve and
/// nothing a stack scoped to it could be reached through: the member is one the service
/// requires even though the model leaves it optional, which is a request-shape finding and
/// so is reported the way every other one is — the framework's clause for an absent member
/// (`Member must not be null`) at the member's own path, rendered by the framework's
/// [`::aws_smithy_schema_server::envelope`] so this refusal cannot word the envelope
/// differently from the boundary walk's.
pub fn missing_vpce_id_message(endpoints: &[AccessEndpoint]) -> Option<String> {
    let violations: Vec<::aws_smithy_schema_server::Violation> = endpoints
        .iter()
        .enumerate()
        .filter(|(_, endpoint)| endpoint.vpce_id.is_none())
        .map(
            |(index, _)| ::aws_smithy_schema_server::Violation {
                path: ::aws_smithy_schema_server::constraints::path::join(&[
                    ::aws_smithy_schema_server::constraints::path::member(ACCESS_ENDPOINTS_MEMBER),
                    ::aws_smithy_schema_server::constraints::path::element(index + 1),
                    ::aws_smithy_schema_server::constraints::path::member(VPCE_ID_MEMBER),
                ]),
                kind: ::aws_smithy_schema_server::ViolationKind::MissingRequiredMember,
                value: None,
                sensitive: false,
            },
        )
        .collect();
    ::aws_smithy_schema_server::envelope(&violations)
}
