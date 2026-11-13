// SPDX-License-Identifier: CC0-1.0

//! The invoke address this emulator ANNOUNCES for a deployed stage: the address an artifact
//! a caller downloads — an exported OpenAPI definition, a generated client SDK — names as the
//! endpoint to call the stage at.
//!
//! AWS serves a deployed stage at `{rest-api-id}.execute-api.{region}.amazonaws.com`
//! ("clients can invoke your API with the default
//! https://{api_id}.execute-api.{region}.amazonaws.com endpoint",
//! `RestApi$disableExecuteApiEndpoint`), and that is the address to announce a client that
//! resolves AWS's own hosts. A client on the machine this emulator runs on resolves that name
//! nowhere: it reached the emulator's own endpoint, and what it can call the stage at is that
//! same endpoint — which serves every service's surface at one address, so the api travels in
//! the PATH, behind the reserved prefix this data plane claims
//! (`/_aws/execute-api/{api-id}/{stage}/{resource-path}`, the prefixed path-style addressing
//! `super::resolve` reads). An announced address a caller cannot reach is an artifact that
//! points every client built from it away from the emulated API.
//!
//! ONE reading for every artifact that names it, so a definition and a generated SDK of one
//! stage cannot hand two unrelated addresses out: the exported document's `host` (Swagger
//! 2.0) and `servers[].url` (OpenAPI 3.0) are built on it, and so is the endpoint `GetSdk`
//! binds its client to.

use super::{EMULATOR_PATH_NAMESPACE, EXECUTE_API_LABEL};

/// The address a stage of an api is announced at: the HOST a client dials, and the path
/// prefix that names the api on that host ahead of the stage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnnouncedInvokeAddress {
    /// The host a client dials — "the host (name or ip) serving the API … It MAY include a
    /// port", which is what a 2.0 document's `host` states.
    pub host: String,
    /// The path that names the api ahead of the stage segment, with a leading `/` and no
    /// trailing one. Empty on the api's own `execute-api` host, whose whole path space is one
    /// api's invoke path, and `/_aws/execute-api/{api-id}` on the emulator's own endpoint,
    /// which serves every service's surface at one address and so carries the api in the
    /// path.
    pub path_prefix: String,
}

impl AnnouncedInvokeAddress {
    /// The invoke URL of `stage_name`: the host, the api's own prefix and the stage, which is
    /// the URL a generated client calls.
    #[must_use]
    pub fn invoke_url(&self, scheme: &str, stage_name: &str) -> String {
        self.templated_invoke_url(scheme, stage_name)
    }

    /// The invoke URL of this address with `template` standing in for the stage segment —
    /// the form a 3.0 Server Object takes, whose `{basePath}` variable carries the stage.
    ///
    /// The api is named ahead of the variable, so the ONE URL a 3.0 document carries
    /// addresses this api's data plane whatever its stages are called.
    #[must_use]
    pub fn templated_invoke_url(&self, scheme: &str, template: &str) -> String {
        format!("{scheme}://{}{}/{template}", self.host, self.path_prefix)
    }
}

/// The address this emulator announces for `api_id`, for the request these `parts` came
/// from:
///
/// * a request that addressed the EMULATOR'S OWN endpoint comes from a client that resolves
///   nothing else, so the stage is announced on that same endpoint with the api behind the
///   reserved data-plane prefix;
/// * a request on an emulator that IS served on a port, but addressed to some service's own
///   host, is announced that port — the one address this process answers on — so an artifact
///   produced through an in-process call of another service names an address its caller can
///   reach too;
/// * a request that addressed a service's host on an emulator served on no port at all comes
///   from a client that resolves AWS's own hosts, so the stage is announced on the api's own
///   `execute-api` host ([`execute_api_host`]), which this data plane claims host-style.
///
/// Which host is the emulator's own and which names a service is the tree's one reading of
/// that question (`service_common::data_plane_addressing`), so nothing here is a second rule
/// for it.
#[must_use]
pub fn announced_invoke_address(
    api_id: &str,
    region: &str,
    parts: &::http::request::Parts,
    served_base: &str,
) -> AnnouncedInvokeAddress {
    let emulator_authority = reached_emulator_endpoint(parts).or_else(|| {
        ::service_common::data_plane_addressing::served_endpoint_authority(served_base)
    });
    match emulator_authority {
        Some(host) => AnnouncedInvokeAddress {
            host,
            path_prefix: format!("/{EMULATOR_PATH_NAMESPACE}/{EXECUTE_API_LABEL}/{api_id}"),
        },
        None => AnnouncedInvokeAddress {
            host: execute_api_host(api_id, region),
            path_prefix: String::new(),
        },
    }
}

/// The default invoke host of `api_id` in `region`:
/// `{api-id}.execute-api.{region}.amazonaws.com`, from
/// `RestApi$disableExecuteApiEndpoint`. Built from the same label the data plane claims a
/// request on ([`EXECUTE_API_LABEL`]), so the host announced to a client that resolves AWS's
/// own hosts is the host this service answers for.
#[must_use]
pub fn execute_api_host(api_id: &str, region: &str) -> String {
    format!("{api_id}.{EXECUTE_API_LABEL}.{region}.amazonaws.com")
}

/// The EMULATOR'S OWN endpoint this request reached, or `None` for a request whose host
/// names a service — which is a client that resolves AWS's own hosts.
///
/// The host travels in the `Host` header or in the authority of an absolute-form URI, and
/// both are read by the tree's one reading of the question
/// (`service_common::data_plane_addressing::reached_emulator_endpoint`).
fn reached_emulator_endpoint(parts: &::http::request::Parts) -> Option<String> {
    ::service_common::data_plane_addressing::reached_emulator_endpoint(
        &parts.uri.to_string(),
        parts
            .headers
            .get(::http::header::HOST)
            .and_then(|value| value.to_str().ok()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The scheme a stage is invoked over; the handlers pass their own constant.
    const SCHEME: &str = "https";

    fn parts(host: &str, uri: &str) -> ::http::request::Parts {
        let mut builder = ::http::Request::builder().method("GET").uri(uri);
        if !host.is_empty() {
            builder = builder.header(::http::header::HOST, host);
        }
        builder
            .body(())
            .expect("the test request is well formed")
            .into_parts()
            .0
    }

    #[test]
    fn a_stage_is_announced_at_the_address_the_request_reached() {
        // The rows span what the reading generalises over: the emulator's own endpoint as a
        // loopback literal with a port, as a loopback NAME with another port and as a
        // bracketed IPv6 literal, a request that carries its endpoint on an absolute-form
        // URI rather than in a header, and a client that resolves AWS's own hosts (this
        // service's control-plane host) on an emulator served on a port and on one served on
        // no port at all.
        let unserved = ::smithy_server_runtime::UNSERVED_URL_BASE;
        let relative = "/restapis/abc1230000/stages/live/exports/swagger";
        let cases: &[(&str, &str, &str, &str, &str, &str)] = &[
            (
                "the emulator's own loopback endpoint",
                "127.0.0.1:39461",
                relative,
                unserved,
                "127.0.0.1:39461",
                "/_aws/execute-api/abc1230000",
            ),
            (
                "the same endpoint by its loopback name on another port",
                "localhost:9297",
                relative,
                unserved,
                "localhost:9297",
                "/_aws/execute-api/abc1230000",
            ),
            (
                "a bracketed IPv6 endpoint",
                "[::1]:9297",
                relative,
                unserved,
                "[::1]:9297",
                "/_aws/execute-api/abc1230000",
            ),
            (
                "an endpoint carried on an absolute-form URI rather than in a header",
                "",
                "https://127.0.0.1:39461/restapis/abc1230000/stages/live/exports/swagger",
                unserved,
                "127.0.0.1:39461",
                "/_aws/execute-api/abc1230000",
            ),
            (
                "a client that resolves AWS's own hosts, on an emulator served on a port",
                "apigateway.us-west-2.amazonaws.com",
                relative,
                "https://127.0.0.1:39461",
                "127.0.0.1:39461",
                "/_aws/execute-api/abc1230000",
            ),
            (
                "a client that resolves AWS's own hosts, on an emulator served on no port",
                "apigateway.us-west-2.amazonaws.com",
                relative,
                unserved,
                "abc1230000.execute-api.us-west-2.amazonaws.com",
                "",
            ),
        ];
        for (case, host, uri, served_base, expected_host, expected_prefix) in cases {
            let address = announced_invoke_address(
                "abc1230000",
                "us-west-2",
                &parts(host, uri),
                served_base,
            );
            assert_eq!(
                address.host, *expected_host,
                "case {case:?}: expected the host {expected_host:?}, got {:?}",
                address.host
            );
            assert_eq!(
                address.path_prefix, *expected_prefix,
                "case {case:?}: expected the api's path prefix {expected_prefix:?}, got {:?}",
                address.path_prefix
            );
            let invoke_url = address.invoke_url(SCHEME, "live");
            assert_eq!(
                invoke_url,
                format!("{SCHEME}://{expected_host}{expected_prefix}/live"),
                "case {case:?}: expected the stage's invoke URL, got {invoke_url:?}"
            );
            let templated = address.templated_invoke_url(SCHEME, "{basePath}");
            assert_eq!(
                templated,
                format!("{SCHEME}://{expected_host}{expected_prefix}/{{basePath}}"),
                "case {case:?}: expected the templated server URL, got {templated:?}"
            );
        }
    }
}
