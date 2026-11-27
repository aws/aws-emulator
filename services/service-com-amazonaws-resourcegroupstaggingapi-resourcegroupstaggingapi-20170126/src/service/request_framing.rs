// SPDX-License-Identifier: CC0-1.0

//! The FRAMING one request of this endpoint arrives in, and therefore the one it is read
//! and answered in.
//!
//! This endpoint serves its nine operations in the two Smithy RPC framings its MODEL
//! declares
//! (`smithy_query(query="service_info",model_path="services/service-com-amazonaws-resourcegroupstaggingapi-resourcegroupstaggingapi-20170126/smithy-model/com-amazonaws-resourcegroupstaggingapi-resourcegroupstaggingapi-20170126.json")`
//! -> protocols `["awsJson1_1", "rpcv2Cbor"]`, `sigv4Name` `tagging`). `awsJson1_1` posts
//! every operation to `/` and names it in `X-Amz-Target`. Smithy RPC v2 CBOR names the
//! operation in the request PATH (`/service/{Service}/operation/{Operation}`), declares
//! itself with the `smithy-protocol` header, carries its body as CBOR, and deliberately
//! sends no `X-Amz-Target` at all.
//!
//! RULE: a request is read, dispatched and answered in the framing THE REQUEST names, so
//! the `X-Amz-Target` requirement belongs to the `awsJson` framing alone. A framing is
//! named by the request's headers and path and by nothing else, so which one it is is
//! settled before any byte of the body is read, and both framings decode to the same
//! modelled input and are served by the same operation over the same state: no operation
//! can behave differently in the two, and a read in either cannot perturb what a later
//! caller sees.
//!
//! Nothing of either framing is implemented or selected here. The framework holds one
//! codec per protocol the MODEL declares and picks the one a request names, falling back to
//! the refusal a request naming none of them earns
//! (`vendor/smithy-server-runtime/src/wire_protocol.rs`), and every later step of the
//! request reads that one choice (operation lookup, the modelled auth schemes, deserialize,
//! serialize, and every error envelope), so one request cannot be routed in one framing and
//! answered in another. This module therefore states only the one question this service
//! itself asks about a framing: whether a request names the CBOR one, which is what
//! [`super::contradictory_framing`] refuses a self-contradicting request by.

use aws_smithy_runtime_api::http::Request as RtRequest;
use smithy_server_runtime::WireProtocolKind;

/// Whether `req` names the Smithy RPC v2 CBOR framing.
///
/// The question is asked of the framework's own detection — the one rule the whole tree
/// recognises a framing by, reading the `smithy-protocol` header and the
/// `/service/{Service}/operation/{Operation}` path and nothing else, and the one the
/// generated entry selects this request's protocol with — so this service cannot recognise
/// a request the framework would not, or miss one it would.
pub(super) fn names_rpc_v2_cbor(req: &RtRequest) -> bool {
    matches!(
        WireProtocolKind::detect(req),
        Some(WireProtocolKind::RpcV2Cbor)
    )
}

#[cfg(test)]
mod tests {
    use super::names_rpc_v2_cbor;
    use aws_smithy_runtime_api::http::Request as RtRequest;
    use aws_smithy_types::body::SdkBody;

    /// A request to `uri` carrying `headers`.
    fn asked(uri: &str, headers: &[(&str, &str)]) -> RtRequest {
        let mut req = RtRequest::new(SdkBody::empty());
        req.set_uri(uri.to_string()).expect("a valid uri");
        for (name, value) in headers {
            req.headers_mut().insert(name.to_string(), value.to_string());
        }
        req
    }

    /// One row of the table below: the case, the request's uri, the headers it
    /// carries, and whether it names the CBOR framing.
    type FramingCase = (
        &'static str,
        &'static str,
        &'static [(&'static str, &'static str)],
        bool,
    );

    /// RULE: the CBOR framing is named by the `smithy-protocol` header or by the path that
    /// framing routes on; the `awsJson` framing of the same endpoint is named by neither,
    /// whatever else it carries.
    #[test]
    fn a_request_names_the_cbor_framing_by_that_framings_own_markers() {
        // (case, request uri, headers, whether the request names the CBOR framing)
        let rows: &[FramingCase] = &[
            (
                "the framing's own header and path",
                "/service/ResourceGroupsTaggingAPI_20170126/operation/GetResources",
                &[
                    ("smithy-protocol", "rpc-v2-cbor"),
                    ("content-type", "application/cbor"),
                ],
                true,
            ),
            (
                "that header spelled in upper case",
                "/",
                &[("smithy-protocol", "RPC-V2-CBOR")],
                true,
            ),
            (
                "the framing's path with no header",
                "/service/ResourceGroupsTaggingAPI_20170126/operation/GetTagKeys",
                &[],
                true,
            ),
            (
                "a protocol header naming something else",
                "/",
                &[("smithy-protocol", "rpc-v2-json")],
                false,
            ),
            (
                "the awsJson framing of the same operation",
                "/",
                &[
                    ("content-type", "application/x-amz-json-1.1"),
                    (
                        "x-amz-target",
                        "ResourceGroupsTaggingAPI_20170126.GetResources",
                    ),
                ],
                false,
            ),
            (
                "a CBOR body with neither marker",
                "/",
                &[("content-type", "application/cbor")],
                false,
            ),
            ("no header at all", "/", &[], false),
        ];
        for (case, uri, headers, expected) in rows.iter().copied() {
            let actual = names_rpc_v2_cbor(&asked(uri, headers));
            assert_eq!(
                actual, expected,
                "case {case:?}: expected the request to name the CBOR framing: {expected}, \
                 got {actual}"
            );
        }
    }
}
