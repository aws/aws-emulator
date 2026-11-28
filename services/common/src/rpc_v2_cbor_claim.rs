// SPDX-License-Identifier: CC0-1.0

//! The claim an `rpcv2Cbor` service makes on a request that CARRIES a SigV4-family
//! credential the framework's routing-key reader cannot read, and addresses that protocol's
//! own URI.
//!
//! # What this rule is not
//!
//! It is not authentication, and it does not stand in for any. `Service::can_handle` decides
//! WHICH service's model, protocol and envelope answer a request; the router then
//! authenticates the request it selected — `MultiServiceRouter::dispatch` runs its
//! `authenticate` step after `select_service` and before the operation, so with request
//! authentication on (the default) a request this rule claims is still refused unless its
//! signature verifies against the emulator's minted keys. A claim therefore cannot open a
//! surface; it can only change which service's envelope the refusal is rendered in. The
//! service suite that drives this rule asserts exactly that, with authentication ON.
//!
//! For the same reason the rule requires the caller to have PRESENTED a SigV4-family
//! credential ([`carries_sigv4_family_credential`]): a request carrying none is not this
//! rule's business, so it keeps reaching whatever the framework's own routing decides for it
//! rather than being claimed on the strength of a path a caller chose.
//!
//! # Why a presented credential can still name no service
//!
//! The framework resolves a request's routing key from a SigV4 credential scope, then an
//! `X-Amz-Target`, then a service-naming endpoint host. A signed request to an `rpcv2Cbor`
//! service can match none of the three:
//!
//! * that protocol has no `X-Amz-Target` — it names its operation in the URI;
//! * a client pointed at this emulator's own address reaches it on a loopback host, which
//!   names no service; and
//! * SigV4A (`AWS4-ECDSA-P256-SHA256`, the scheme several models list FIRST) signs with a
//!   credential scope of `<access key id>/<date>/<service>/aws4_request` — no Region field —
//!   which the framework's scope reader, written for SigV4's five-field scope, does not
//!   parse.
//!
//! Such a request resolves to no key at all, every service's default claim (the key is my
//! signing name) is false, and the router answers `404 UnknownService` in its own awsJson
//! envelope — a body no client of a service whose single declared protocol is `rpcv2Cbor`
//! can decode, so neither the status nor an error code nor a message reaches it.
//!
//! # The claim
//!
//! [`claims_unattributed_rpc_v2_cbor_route`] admits exactly those requests: the caller
//! presented a SigV4-family credential, nothing in the request names a service, AND the
//! `rpcv2Cbor` URI (`POST /service/{Service}/operation/{Operation}`) names THIS service's
//! shape. That is how the protocol itself dispatches — the target service is a path segment
//! — and it is as narrow as that: a request naming ANOTHER service's shape is that service's,
//! a path that is not the protocol's own form is still nobody's, and an unsigned request is
//! left exactly where it was.
//!
//! The `{Service}` label is read through the same reader [`crate::claims_request`] uses, so
//! the two claims cannot drift into two readings of one path, and is compared
//! case-insensitively against the shape's local name as the framework compares a routing key.
//!
//! A service takes it beside its own routing-key claim, which keeps every request that DOES
//! name a service exactly where it was:
//!
//! ```ignore
//! fn can_handle(&self, parts: &http::request::Parts) -> bool {
//!     match smithy_server_runtime::resolve_route_key(parts) {
//!         Some(key) => key == self.entry.identity().signing_name(),
//!         None => ::service_common::rpc_v2_cbor_claim::claims_unattributed_rpc_v2_cbor_route(
//!             parts,
//!             self.entry.identity().shape_id(),
//!         ),
//!     }
//! }
//! ```

/// Whether the caller PRESENTED a SigV4-family credential: an `Authorization` header signed
/// with an `AWS4-` algorithm, or a presigned URL's own query presentation.
///
/// Whether that credential VERIFIES is the framework's question, asked after the request is
/// routed; this only asks whether one was presented at all, which is what makes the claim
/// below about a credential the reader could not parse rather than about an anonymous caller.
///
/// The reading itself is this crate's one answer to "did the caller present a SigV4
/// credential" ([`crate::sigv4_presentation::presents_a_credential`]), over the `Parts` a
/// `Service::can_handle` seam is handed rather than the two strings that reader takes.
#[must_use]
pub fn carries_sigv4_family_credential(parts: &http::request::Parts) -> bool {
    crate::sigv4_presentation::presents_a_credential(
        parts
            .headers
            .get(::http::header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok()),
        parts.uri.query(),
    )
}

/// Whether the request `parts` presents a SigV4-family credential that names no service, and
/// its `rpcv2Cbor` URI names the service whose Smithy shape id is `service_shape_id`.
///
/// See the [module docs](self) for the rule, for what it is not (it is not authentication,
/// which the router performs on the request it routes), and for why it is the narrowest one
/// that keeps an `rpcv2Cbor` endpoint answerable to a SigV4A-signing client.
#[must_use]
pub fn claims_unattributed_rpc_v2_cbor_route(
    parts: &http::request::Parts,
    service_shape_id: &str,
) -> bool {
    if !carries_sigv4_family_credential(parts) {
        // The caller presented no credential of the family this rule is about, so this rule
        // declines: such a request keeps reaching whatever the framework's own routing
        // decides for it.
        return false;
    }
    if ::emulator_server::resolve_route_key(parts).is_some() {
        // The request names a service. Whichever service that is, this rule does not
        // decide: a request that names one is claimed by its routing key, exactly as
        // before.
        return false;
    }
    crate::rpcv2_cbor_service_named(parts.uri.path())
        .is_some_and(|named| named.eq_ignore_ascii_case(crate::shape_local_name(service_shape_id)))
}

#[cfg(test)]
mod tests {
    //! The rule over the shapes a request takes: the protocol's own URI naming this
    //! service, another service's shape, a path of another form, a request that DOES name a
    //! service, and a request that presents no credential at all.

    use super::*;

    /// The shape id of the service under test, as a model spells it.
    const SHAPE_ID: &str =
        "com.amazonaws.partnercentralrevenuemeasurement#PartnerCentralRevenueMeasurement";

    /// This service's own `rpcv2Cbor` URI.
    const PROTOCOL_URI: &str =
        "/service/PartnerCentralRevenueMeasurement/operation/GetRevenueAttribution";

    /// A SigV4A `Authorization` header: the algorithm the framework's scope reader does not
    /// parse, over a credential scope with no Region field.
    const SIGV4A: &str = "AWS4-ECDSA-P256-SHA256 \
                          Credential=AKIDEXAMPLE/20260101/partnercentral/aws4_request, \
                          SignedHeaders=host, Signature=abc";

    /// One request: its URI, its `Authorization` header if any, and a loopback host (which
    /// names no service), as the parts a claim predicate reads.
    fn parts(uri: &str, authorization: Option<&str>) -> http::request::Parts {
        let mut builder = http::Request::builder().method("POST").uri(uri);
        if let Some(authorization) = authorization {
            builder = builder.header("authorization", authorization);
        }
        builder
            .header("host", "127.0.0.1:9297")
            .body(())
            .expect("a well-formed request")
            .into_parts()
            .0
    }

    #[test]
    fn the_protocol_s_own_uri_naming_this_service_is_claimed() {
        let rows = [
            (PROTOCOL_URI, true),
            // The label is compared case-insensitively, as a routing key is.
            (
                "/service/partnercentralrevenuemeasurement/operation/ListRevenueAttributions",
                true,
            ),
            // Another service's shape, the same protocol.
            (
                "/service/PartnerCentralChannel/operation/ListChannels",
                false,
            ),
            // Not the protocol's own form at all.
            ("/", false),
            ("/GetRevenueAttribution", false),
            ("/service/PartnerCentralRevenueMeasurement", false),
            ("/service//operation/GetRevenueAttribution", false),
        ];
        for (uri, claimed) in rows {
            assert_eq!(
                claims_unattributed_rpc_v2_cbor_route(&parts(uri, Some(SIGV4A)), SHAPE_ID),
                claimed,
                "case {uri:?}: expected the claim to be {claimed}"
            );
        }
    }

    #[test]
    fn a_request_presenting_no_sigv4_family_credential_is_not_claimed() {
        // Nothing presented at all, and a scheme that is not the family's: neither is this
        // rule's business, so the request keeps reaching whatever the framework routes it
        // to — which for an unsigned one is the router's own `404 UnknownService`.
        let rows = [
            ("no authorization header", None),
            ("a bearer token", Some("Bearer not-a-sigv4-credential")),
            ("an empty header", Some("")),
        ];
        for (case, authorization) in rows {
            assert!(
                !claims_unattributed_rpc_v2_cbor_route(
                    &parts(PROTOCOL_URI, authorization),
                    SHAPE_ID
                ),
                "case {case:?}: a request presenting no SigV4-family credential is not claimed"
            );
        }
        // A presigned URL carries its scope in the query string instead of a header, which
        // IS presented credential material.
        assert!(
            claims_unattributed_rpc_v2_cbor_route(
                &parts(
                    &format!("{PROTOCOL_URI}?X-Amz-Algorithm=AWS4-ECDSA-P256-SHA256&X-Amz-Credential=AKID%2F20260101%2Fpartnercentral%2Faws4_request"),
                    None,
                ),
                SHAPE_ID,
            ),
            "case \"a presigned scope\": a presented credential the reader cannot parse is claimed"
        );
    }

    #[test]
    fn a_request_that_names_a_service_is_left_to_its_routing_key() {
        // A readable credential scope names a service, so this rule declines — whether the
        // name is this service's own or a peer's.
        for signing_name in ["partnercentral", "partnercentral-channel"] {
            let authorization = format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/us-east-1/{signing_name}/\
                 aws4_request, SignedHeaders=host, Signature=abc"
            );
            assert!(
                !claims_unattributed_rpc_v2_cbor_route(
                    &parts(PROTOCOL_URI, Some(&authorization)),
                    SHAPE_ID,
                ),
                "case {signing_name:?}: a request naming a service is claimed by its routing key"
            );
        }
    }
}
