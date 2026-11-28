// SPDX-License-Identifier: CC0-1.0

//! The ADDRESS an OpenSearch UI application publishes, and what a client that follows it is
//! answered.
//!
//! "The endpoint URL of the OpenSearch application"
//! (`smithy_query(query="doc", shape="GetApplicationResponse", member="endpoint")`, and the
//! same member on `ApplicationSummary`) is not a label a caller reads back: it is the URL a
//! client FOLLOWS to reach the application's own user interface. AWS serves it on a host of
//! its own, which this emulator neither owns nor answers on, so a caller handed such a URL
//! leaves the emulator altogether — reaching nothing inside a sandbox, and infrastructure
//! that knows nothing of the emulated application anywhere else.
//!
//! So the URL is built over the endpoint THIS emulator answers on, with the application's own
//! id as the segment that says which application it is, and the two halves of that address
//! live here: [`application_endpoint`] mints it and [`application_ui_route`] reads it back, so
//! what a response advertises and what this service serves cannot drift apart.
//!
//! Which address the emulator is reached at is the tree's one reading of that question
//! (`service_common::data_plane_addressing`): the emulator's own endpoint THIS request
//! arrived on, else the port this process is served on — so an application created through an
//! in-process call of another service publishes an address that process answers on too. On an
//! emulator served on no port at all, what is left is the base that names no port
//! (`smithy_server_runtime::UNSERVED_URL_BASE`, a host reserved so nothing resolves it),
//! which is still an address no client is sent out of the emulator by.

use super::Application;

/// The segment that names this service's application plane, behind the reserved namespace
/// segment every emulator-served plane is addressed under:
/// `/_aws/opensearch-application/{id}`.
///
/// Shared by the URL an application reports and by the route that serves it, so the address
/// this service publishes is the address it answers on. It is the APPLICATION plane's own
/// segment, distinct from the one a domain's REST data plane is addressed under
/// (`crate::service::domain_rest`), so neither plane can claim the other's traffic however a
/// resource is named.
pub const APPLICATION_UI_PLANE: &str = "opensearch-application";

/// The scheme an application's endpoint carries: the member is an "endpoint URL", and this
/// emulator serves its whole surface over TLS.
const APPLICATION_UI_SCHEME: &str = "https";

/// The request path the application `id` is answered at.
///
/// The namespace segment and the split into segments are
/// `service_common::data_plane_addressing`'s, so this plane is addressed the way every other
/// emulator-served plane is. An `Id` is admitted only by `^[a-z0-9]{3,30}$`
/// (`smithy_query(query="shape",shape="GetApplicationResponse")`), so every character it can
/// carry is already a path character and the id needs no escaping.
#[must_use]
pub fn application_ui_path(id: &str) -> String {
    format!(
        "/{}/{APPLICATION_UI_PLANE}/{id}",
        ::service_common::data_plane_addressing::EMULATOR_PATH_NAMESPACE
    )
}

/// The URL the application `id` is reached on: the endpoint this emulator is reached at,
/// followed by that application's own path.
///
/// `parts` are the request's — the endpoint it arrived on is the address its caller can reach
/// — and `served_base` is the base URL this process is served at
/// (`smithy_server_runtime::OperationContext::endpoint`), which is what stands for a caller
/// that addressed one of AWS's own hosts. The host is therefore never one of AWS's names: a
/// URL a caller is told to follow has to lead back to where that caller is already talking.
#[must_use]
pub fn application_endpoint(
    parts: &::http::request::Parts,
    served_base: &str,
    id: &str,
) -> String {
    format!(
        "{APPLICATION_UI_SCHEME}://{}{}",
        announced_authority(parts, served_base),
        application_ui_path(id)
    )
}

/// The `host[:port]` this emulator is reached at for the caller that made this request: the
/// emulator's own endpoint the request arrived on, else the port this process is served on,
/// else the base that names no port at all — which is what an emulator serving no port has to
/// report, and resolves nowhere rather than naming a host of AWS's.
fn announced_authority(parts: &::http::request::Parts, served_base: &str) -> String {
    use ::service_common::data_plane_addressing::{
        reached_emulator_endpoint_of_parts, served_endpoint_authority, url_authority,
    };
    reached_emulator_endpoint_of_parts(parts)
        .or_else(|| served_endpoint_authority(served_base))
        .unwrap_or_else(|| url_authority(served_base).trim_end_matches('/').to_string())
}

/// The application a request `uri` addresses, for a path of the form
/// [`application_ui_path`] mints, and `None` for every other path.
///
/// The inverse of the minting above, read through the shared addressing module, so a URL this
/// service published resolves back to the application behind it. A trailing `/` is the same
/// address (that is how a client normalises a bare path), and nothing below the application's
/// own segment is this route: the address names one application, not a path space.
#[must_use]
pub fn application_ui_route(uri: &str) -> Option<&str> {
    use ::service_common::data_plane_addressing::{first_segment, EMULATOR_PATH_NAMESPACE};
    let (namespace, rest) = first_segment(::service_common::request_path(uri))?;
    if namespace != EMULATOR_PATH_NAMESPACE {
        return None;
    }
    let (plane, rest) = first_segment(rest)?;
    if plane != APPLICATION_UI_PLANE {
        return None;
    }
    let (id, below) = first_segment(rest)?;
    (!id.is_empty() && below == "/").then_some(id)
}

/// The media type an application's endpoint answers with: the JSON every other plane of this
/// service answers in ([`crate::service::domain_rest`]), so a client that followed the URL can
/// read the answer.
pub const APPLICATION_UI_MEDIA_TYPE: &str = "application/json";

/// What a fetch of an application's endpoint answers: the application that endpoint belongs
/// to, as the store holds it right now — its id, its name, the status it currently reports and
/// the data sources it reads from.
///
/// Every value comes from the stored record, so what a caller that followed the URL is told
/// cannot disagree with what `GetApplication` reports about the same application. The members
/// are named as the model names them on `GetApplicationResponse`, and they are rendered by the
/// JSON serializer rather than by text assembled here, so no stored string can break the
/// document it is written into.
#[must_use]
pub fn application_ui_document(application: &Application) -> String {
    let data_sources: Vec<::serde_json::Value> = application
        .data_sources
        .iter()
        .map(|source| {
            let members = [
                ("dataSourceArn", source.data_source_arn.as_deref()),
                (
                    "dataSourceDescription",
                    source.data_source_description.as_deref(),
                ),
                (
                    "iamRoleForDataSourceArn",
                    source.iam_role_for_data_source_arn.as_deref(),
                ),
            ];
            // A member the stored data source holds nothing for is left out, the way the
            // control plane leaves it out of its own answer.
            members
                .into_iter()
                .filter_map(|(member, value)| {
                    value.map(|value| (member.to_string(), ::serde_json::json!(value)))
                })
                .collect::<::serde_json::Map<String, ::serde_json::Value>>()
                .into()
        })
        .collect();
    ::serde_json::json!({
        "id": application.id,
        "name": application.name,
        "status": application.status,
        "dataSources": data_sources,
    })
    .to_string()
}

/// What a fetch of an application endpoint this emulator holds no application for answers: an
/// id that was never created, or one whose deletion has completed.
///
/// The sentence is the one the control plane states about such an id
/// ([`super::APPLICATION_DOES_NOT_EXIST`]), so both halves of this service say the same thing
/// about an application the account does not hold.
#[must_use]
pub fn application_ui_not_found_document() -> String {
    ::serde_json::json!({ "message": super::APPLICATION_DOES_NOT_EXIST }).to_string()
}

/// Whether a fetch of an application's endpoint is admitted to READ that application: one
/// presenting an AWS credential is, and one presenting none is not.
///
/// An application's interface is reached through a SIGN-IN — `IamIdentityCenterOptions`
/// "configuration settings for IAM Identity Center in an OpenSearch application", whose
/// `enabled` member "indicates whether IAM Identity Center is enabled for the OpenSearch
/// application" — so the application is not something an anonymous caller reads. This
/// emulator does not evaluate an Identity Center session; what it has of a caller's identity
/// is the AWS credential the request carries, which the front door verifies for every other
/// surface this service serves ([`crate::service::application_ui`] names the scheme so the
/// router verifies a presented one before this plane runs). A fetch carrying none is
/// therefore refused, whatever the application configures, and reads nothing of it.
///
/// The ONE reading of that rule, so the claim this plane makes and the answer it gives cannot
/// disagree about who is admitted.
#[must_use]
pub fn application_ui_admits(sigv4: bool) -> bool {
    sigv4
}

/// What a fetch this plane does not admit answers: the request carried no credential to
/// resolve a caller from.
///
/// Nothing of any application is stated — not whether one exists under the address, not its
/// name, status or data sources — because the fetch was not admitted to read it; what it is
/// told is the condition it did not meet, in the sentence this emulator states for a request
/// that has to be signed and is not
/// ([`::emulator_server::request_authentication::ACCESS_DENIED`], the framework's own).
#[must_use]
pub fn application_ui_unauthenticated_document() -> String {
    ::serde_json::json!({
        "message": ::emulator_server::request_authentication::ACCESS_DENIED.message,
    })
    .to_string()
}

/// The error code that answer travels under, which is the one the framework states for the
/// same condition.
pub const APPLICATION_UI_UNAUTHENTICATED_CODE: &str =
    ::emulator_server::request_authentication::ACCESS_DENIED.code;

/// The status it is served on: the status AWS answers a request it will not serve the caller
/// of, and the one every unauthenticated caller of this emulator's front door is refused on.
pub const APPLICATION_UI_UNAUTHENTICATED_STATUS: u16 = 403;

#[cfg(test)]
mod tests {
    use super::*;

    /// The request parts of a call that addressed `host`.
    fn addressed(host: &str) -> ::http::request::Parts {
        ::http::Request::builder()
            .uri("/")
            .header(::http::header::HOST, host)
            .body(())
            .expect("a well-formed request")
            .into_parts()
            .0
    }

    /// RULE: the URL an application publishes is built over the endpoint this emulator is
    /// reached at — the one the request arrived on, else the port this process is served on —
    /// with the application's own id in the path, and it resolves back to that application.
    #[test]
    fn an_application_publishes_a_url_on_the_endpoint_this_emulator_is_reached_at() {
        /// One case: who called, where this emulator is served, and the authority the
        /// application's URL has to carry.
        struct Row {
            case: &'static str,
            host: &'static str,
            served_base: &'static str,
            authority: &'static str,
        }
        let id = "ab3xk9qp17vn42zd0c8m";
        let rows = [
            Row {
                case: "a client on the emulator's own loopback address",
                host: "127.0.0.1:9297",
                served_base: "https://127.0.0.1:39355",
                authority: "127.0.0.1:9297",
            },
            Row {
                case: "a client on a loopback name, this emulator served on no port",
                host: "localhost:9297",
                served_base: ::smithy_server_runtime::UNSERVED_URL_BASE,
                authority: "localhost:9297",
            },
            Row {
                case: "a client that resolves AWS's own hosts, this emulator served",
                host: "es.us-west-2.amazonaws.com",
                served_base: "https://127.0.0.1:39355",
                authority: "127.0.0.1:39355",
            },
            Row {
                case: "a client that resolves AWS's own hosts, served on no port at all",
                host: "es.us-west-2.amazonaws.com",
                served_base: ::smithy_server_runtime::UNSERVED_URL_BASE,
                authority: "emulator.invalid",
            },
        ];
        for Row {
            case,
            host,
            served_base,
            authority,
        } in &rows
        {
            let published = application_endpoint(&addressed(host), served_base, id);
            let expected = format!("https://{authority}/_aws/{APPLICATION_UI_PLANE}/{id}");
            assert_eq!(
                published, expected,
                "case {case:?}: expected the URL {expected:?}, got {published:?}"
            );
            // Never a host of AWS's: a URL a caller follows has to lead back here.
            assert!(
                !published.contains("amazonaws.com"),
                "case {case:?}: expected no AWS host in the published URL, got {published:?}"
            );
            // And the URL resolves back to the application it was minted for, so what is
            // advertised is what this service serves.
            assert_eq!(
                application_ui_route(&published),
                Some(id),
                "case {case:?}: expected the published URL to address {id:?}, got {published:?}"
            );
        }
    }

    /// RULE: only the path this service mints addresses one application — a path under
    /// another plane, one naming no application and one reaching BELOW the application's own
    /// segment address nothing here.
    #[test]
    fn only_an_application_path_of_this_plane_addresses_an_application() {
        let cases = [
            ("the minted path", "/_aws/opensearch-application/abc123", Some("abc123")),
            (
                "the minted path with the trailing slash a client may add",
                "/_aws/opensearch-application/abc123/",
                Some("abc123"),
            ),
            (
                "the same path in the absolute form a client may send",
                "https://127.0.0.1:9297/_aws/opensearch-application/abc123",
                Some("abc123"),
            ),
            ("a path below the application's own segment", "/_aws/opensearch-application/abc123/app", None),
            ("another plane of this emulator", "/_aws/es/a-domain/_cat/indices", None),
            ("the plane segment with no application named", "/_aws/opensearch-application", None),
            ("a modelled control-plane route", "/2021-01-01/opensearch/application/abc123", None),
        ];
        for (case, uri, expected) in cases {
            let resolved = application_ui_route(uri);
            assert_eq!(
                resolved, expected,
                "case {case:?}: expected {expected:?} for {uri:?}, got {resolved:?}"
            );
        }
    }
}
