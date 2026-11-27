// SPDX-License-Identifier: CC0-1.0

//! This service's `restJson1` codec, with the SHARED rule for an unset string member
//! applied to every response it renders — and the one place a request's input shape is in
//! hand, which the refusal path reads.
//!
//! # The rule
//!
//! A response never carries a value a member's own constraints forbid. A string member
//! whose `@length` minimum is 1 or more cannot legally hold the empty string, so "no value"
//! for such a member is expressed by omitting it. The rule itself, and the walk that
//! applies it at every nesting level, live once for every service of the class in
//! [`service_common::unset_string_members`]; this module is where THIS service takes them.
//!
//! # Why this service is one of that class
//!
//! Every description this model declares targets one `Description` shape
//! (`smithy_query(query="shape", shape="Description")` — `@length` min 1, max 2048, pattern
//! `^[^\u0000-\u001F\u007F]+$`) and is modelled `@required` on the read side even though the
//! member that writes it is optional on every create and update — an asset model's, an
//! asset's, a dataset's, a composite model's, and the same member on each summary. So a
//! resource created without one has no description to report.
//!
//! `ListActionsResponse$nextToken` is the same shape of problem on a pagination token:
//! modelled `@required` while documented "The token for the next set of results, or null if
//! there are no additional results", and targeting `NextToken`, whose `@length` minimum of 1
//! and `^[A-Za-z0-9+/=]+$` leave it no value that could stand for "no next page". The
//! generated response therefore carries the empty token for the last page, and this codec is
//! what leaves the member out instead — so that shape keeps the derive the model drives
//! rather than a hand-written copy of it.
//!
//! The REQUEST side is the framework's own: a request carrying a value outside a member's
//! `@length` is refused by the validating walk rather than quietly dropped.

use aws_smithy_runtime_api::http::{Request, Response};
use aws_smithy_schema::serde::{SerializableStruct, ShapeDeserializer};
use aws_smithy_schema::{Schema, ShapeId};
use aws_smithy_schema_server::protocol::{ProtocolError, ServerProtocol};
use aws_smithy_schema_server::RestJsonServerProtocol;
use service_common::unset_string_members::OmittingUnsetStrings;

// The shared predicate this service's own cases ask of its model's member schemas; the
// codec reaches it through `OmittingUnsetStrings` rather than calling it here.
#[cfg(test)]
use service_common::unset_string_members::forbids_the_empty_string;

/// This service's `restJson1` codec, with an unset string member omitted from
/// every response rather than reported as the empty string.
#[derive(Debug)]
pub struct UnsetStringMembersRestJson {
    inner: RestJsonServerProtocol,
}

impl UnsetStringMembersRestJson {
    /// Build the protocol once, at service construction:
    /// [`smithy_server_runtime::Service::override_protocol`] hands back a borrow of
    /// it and must never build one per request.
    ///
    /// The route table is EMPTY by design. A protocol's routes are read by
    /// `ServerProtocol::routing_key` alone, and the router resolves a request's
    /// operation through the generated service definition's own protocol (whose
    /// table is the model's); an author-overridden protocol drives only the
    /// modelled sequence — `deserialize_input`, `serialize_output`,
    /// `serialize_error`.
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: RestJsonServerProtocol::new(&[]),
        }
    }
}

impl Default for UnsetStringMembersRestJson {
    fn default() -> Self {
        Self::new()
    }
}

impl ServerProtocol for UnsetStringMembersRestJson {
    type Request = Request;
    type Response = Response;

    fn protocol_id(&self) -> &ShapeId {
        self.inner.protocol_id()
    }

    fn routing_key<'a>(&self, request: &'a Self::Request) -> Result<&'a str, ProtocolError> {
        self.inner.routing_key(request)
    }

    fn deserialize_input<'a>(
        &self,
        request: &'a Self::Request,
        input_schema: &Schema,
    ) -> Result<Box<dyn ShapeDeserializer + 'a>, ProtocolError> {
        // The REQUEST side is the framework's own: the rule is about what a read
        // reports, and a request carrying a value outside a member's `@length` is
        // refused by the validating walk rather than quietly dropped here.
        //
        // This is also the one place a request's INPUT SHAPE is in hand, so what that
        // shape declares is recorded here for the refusal the walk may produce: the
        // walk cannot apply an expression written with look-around, and a clause
        // stated at a collection member lists the element shape's whole constraint
        // set, which no violation carries (`super::input_shape`).
        super::input_shape::reading_input(input_schema);
        self.inner.deserialize_input(request, input_schema)
    }

    fn serialize_output(
        &self,
        output: &dyn SerializableStruct,
        output_schema: &Schema,
        operation_name: &str,
        success_status: u16,
    ) -> Result<Self::Response, ProtocolError> {
        self.inner.serialize_output(
            &OmittingUnsetStrings(output),
            output_schema,
            operation_name,
            success_status,
        )
    }

    fn serialize_error(
        &self,
        error: &dyn SerializableStruct,
        error_schema: &Schema,
        error_code: &str,
        http_status: u16,
    ) -> Result<Self::Response, ProtocolError> {
        // Wrapped for the same reason the success body is: the seam is uniform, so
        // no shape of this model can report a string its own constraints forbid.
        // Every error shape of this service carries a message it always has, so the
        // wrapper is a no-op for them.
        self.inner.serialize_error(
            &OmittingUnsetStrings(error),
            error_schema,
            error_code,
            http_status,
        )
    }

    fn serialize_event_stream_message(
        &self,
        event_type: &str,
        is_exception: bool,
        event: &dyn SerializableStruct,
        event_schema: &Schema,
    ) -> Result<Self::Response, ProtocolError> {
        self.inner.serialize_event_stream_message(
            event_type,
            is_exception,
            &OmittingUnsetStrings(event),
            event_schema,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::forbids_the_empty_string;

    /// RULE: a member forbids the empty string exactly when its own `@length`
    /// minimum is 1 or more. Asked of the REAL member schemas of one of this
    /// model's read responses, so the predicate is pinned against the model rather
    /// than against a schema written here: source
    /// `smithy_query(query="shape", shape="DescribeAssetModelResponse")` — the
    /// description targets `Description` (`@length` min 1), the external id
    /// `ExternalId` (min 2), the ARN `ARN` (min 1), while the type is an enum and
    /// the creation date a timestamp, neither carrying a `@length` at all.
    #[test]
    fn only_a_minimum_of_one_or_more_forbids_the_empty_string() {
        let rows: [(&str, bool); 5] = [
            ("assetModelDescription", true),
            ("assetModelExternalId", true),
            ("assetModelArn", true),
            ("assetModelType", false),
            ("assetModelCreationDate", false),
        ];
        let schema = crate::ops::types::DESCRIBE_ASSET_MODEL_OUTPUT_SCHEMA;
        for (case, expected) in rows {
            let member = schema
                .members()
                .iter()
                .find(|member| member.member_name() == Some(case))
                .unwrap_or_else(|| panic!("case {case:?}: the response declares this member"));
            let forbids = forbids_the_empty_string(member);
            assert_eq!(
                forbids, expected,
                "case {case:?}: expected forbids_the_empty_string {expected}, got {forbids}"
            );
        }
    }

    /// RULE, over the one member this service's PAGINATION depends on: the token of a
    /// `@required` output token member is omittable, because `NextToken`'s own `@length`
    /// minimum of 1 leaves it no value standing for "no next page". Asked of the real
    /// member schema the derive builds for `ListActionsResponse`, beside the list member
    /// that always has a value. Source
    /// `smithy_query(query="operation", operation="ListActions")`.
    #[test]
    fn the_required_pagination_token_of_a_listing_is_omittable() {
        let rows: [(&str, bool); 2] = [("nextToken", true), ("actionSummaries", false)];
        let schema = crate::ops::types::LIST_ACTIONS_OUTPUT_SCHEMA;
        for (case, expected) in rows {
            let member = schema
                .members()
                .iter()
                .find(|member| member.member_name() == Some(case))
                .unwrap_or_else(|| panic!("case {case:?}: the response declares this member"));
            let forbids = forbids_the_empty_string(member);
            assert_eq!(
                forbids, expected,
                "case {case:?}: expected forbids_the_empty_string {expected}, got {forbids}"
            );
        }
    }
}
