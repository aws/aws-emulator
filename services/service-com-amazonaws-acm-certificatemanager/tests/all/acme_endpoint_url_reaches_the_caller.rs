// SPDX-License-Identifier: CC0-1.0

//! Where the ACME surface of an ACM ACME endpoint is PUBLISHED, end to end over the wire.
//!
//! RULE: a server-managed URL this service hands a client names an address that client
//! can reach, and the address this emulator SERVES that resource at. `AcmeEndpoint$EndpointUrl`
//! is the single handle an ACME client (certbot, lego, acme.sh, the python `acme` library)
//! is configured with — it fetches that URL verbatim and follows the resource map the body
//! carries — so the URL is minted on the Region's published ACME enrollment host for a
//! caller that resolves AWS's own hostnames, and, for a caller that reached this emulator
//! directly, on the loopback socket this emulator serves that surface on
//! (`state::acme_socket`, whose answers carry the field names RFC 8555 spells), with the
//! plane, the Region and the endpoint's id in the PATH. Every URL the ACME surface then
//! answers with — the resource map inside the directory, an account's own URL — is on the
//! address the client fetched it from, so the whole account lifecycle is reachable for a
//! client that rewrites no host.
//!
//! The table spans what the rule generalises over: the shape of the endpoint a caller
//! reached (an IPv4 loopback address with and without a port, a loopback NAME, an IPv6
//! literal), a caller that addressed the Region's published control-plane host, and a
//! caller that named no host at all; and, for each, the Region the endpoint lives in.
//! What the published address serves, field name by field name, is
//! [`crate::acme_nonce_field_name`].
//!
//! Every URL fetched here is one a DIFFERENT operation (`DescribeAcmeEndpoint`,
//! `ListAcmeEndpoints`) advertised, fetched with that URL's own host and path and no
//! credential of any kind — exactly what a client holding it does — so an answer that is
//! not computed from the persisted endpoint record cannot pass.

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::acme_account_registration::{
    external_account_binding, header, jws, post, AccountKey, Identification,
};
use crate::acme_directory_endpoint::{
    activate_endpoints_now, body_json, create_endpoint, endpoint_id, fetch, split_url,
    DIRECTORY_MEMBERS, DNS_SUFFIX, ENROLLMENT_LABEL,
};
use crate::acme_external_account_binding_lifecycle::create_binding;
use crate::certificate_lifecycle::{call, router_with_engine, ACCOUNT, REGION};

/// The reserved first path segment every data plane this emulator serves on its OWN
/// endpoint sits under, and the segment that names THIS plane behind it.
///
/// Written out here rather than read from the service, so this test pins the published
/// form: the prefix begins with `_`, which no modelled path segment of any service does,
/// and the plane is named by the same label the published enrollment host carries.
const LOCAL_PLANE_PREFIX: &str = "/_aws/acm-acme-enroll";

/// The Regions the table spans: the recorded one and one in another geography, so the
/// Region travels in the path on the emulator's own endpoint for every Region-name shape
/// an ACM ARN may carry.
const REGIONS: &[&str] = &["us-west-2", "eu-west-1"];

/// One row: what the caller reached, and the `Host` it addressed (`None` for a caller
/// that names no host at all, which is what an in-process caller sends).
const ADDRESSES: &[(&str, Option<&str>)] = &[
    ("the loopback address and the data port", Some("127.0.0.1:9297")),
    ("the loopback address with no port", Some("127.0.0.1")),
    ("a loopback name", Some("localhost:9297")),
    ("an IPv6 loopback literal", Some("[::1]:9297")),
];

/// The caller-chosen text every row's `CreateAcmeEndpoint` carries in each member that
/// takes any (`crate::acme_directory_endpoint::create_endpoint` puts it in the endpoint's
/// tags), so a URL derived from the request could not escape notice.
const MARKER: &str = "client-supplied-marker";

/// Call one operation signed for `region` and addressed to `host`, returning
/// `(status, body)`.
///
/// The `Host` is the one thing this differs from the sibling suites' control-plane call
/// in: it is what a caller that reached this emulator at its own endpoint sends, and
/// what decides the address a server-managed URL is published at.
async fn call_at(
    router: &MultiServiceRouter,
    host: Option<&str>,
    region: &str,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    let mut request = ::http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("CertificateManager.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTEST/20240101/{region}/acm/aws4_request, \
                 SignedHeaders=host;x-amz-target, Signature=0000"
            ),
        );
    if let Some(host) = host {
        request = request.header("host", host);
    }
    let response = router
        .dispatch(
            request
                .body(Bytes::from(
                    serde_json::to_vec(body).expect("the request body serializes"),
                ))
                .expect("a valid request"),
        )
        .await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let parsed = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice::<Value>(&bytes).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// The `EndpointUrl` `DescribeAcmeEndpoint` reports for `arn` to a caller that addressed
/// `host`.
async fn advertised_url(
    router: &MultiServiceRouter,
    case: &str,
    host: Option<&str>,
    region: &str,
    arn: &str,
) -> String {
    let (status, response) = call_at(
        router,
        host,
        region,
        "DescribeAcmeEndpoint",
        &json!({ "AcmeEndpointArn": arn }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeAcmeEndpoint must succeed, got {status} {response}"
    );
    response["AcmeEndpoint"]["EndpointUrl"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no EndpointUrl in {response}"))
        .to_string()
}

/// The `EndpointUrl` `ListAcmeEndpoints` reports for `arn` to a caller that addressed
/// `host`.
async fn listed_url(
    router: &MultiServiceRouter,
    case: &str,
    host: Option<&str>,
    region: &str,
    arn: &str,
) -> String {
    let (status, response) = call_at(router, host, region, "ListAcmeEndpoints", &json!({})).await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListAcmeEndpoints must succeed, got {status} {response}"
    );
    response["AcmeEndpoints"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: no AcmeEndpoints in {response}"))
        .iter()
        .find(|summary| summary["AcmeEndpointArn"].as_str() == Some(arn))
        .and_then(|summary| summary["EndpointUrl"].as_str())
        .unwrap_or_else(|| panic!("case {case:?}: {arn} was not listed with a URL: {response}"))
        .to_string()
}

/// Assert the directory at `url` is served and names its five resources under the same
/// base, returning the document.
async fn assert_directory_served(router: &MultiServiceRouter, case: &str, url: &str) -> Value {
    let response = fetch(router, case, "GET", url).await;
    let status = response.status().as_u16();
    let document = body_json(case, &response);
    assert_eq!(
        status, 200,
        "case {case:?}: the advertised URL {url} must serve the ACME directory, got {status} \
         {document}"
    );
    let base = url
        .strip_suffix("/directory")
        .unwrap_or_else(|| panic!("case {case:?}: {url} is not a directory URL"));
    let (authority, _path) = split_url(case, url);
    for (member, resource) in DIRECTORY_MEMBERS {
        let named = document[*member].as_str().unwrap_or_default();
        assert_eq!(
            named,
            format!("{base}/{resource}"),
            "case {case:?}: expected {member} under {base}, got {document}"
        );
        assert_eq!(
            split_url(case, named).0,
            authority,
            "case {case:?}: every resource the directory names must be reachable at the \
             address the client fetched it from, got {named:?}"
        );
    }
    document
}

/// RULE: the advertised `EndpointUrl` is the address this emulator SERVES that endpoint's
/// ACME surface at and the caller can reach — and a plain `GET` of it, exactly as returned,
/// serves the RFC 8555 directory whose resources are on that same address.
#[tokio::test]
async fn the_advertised_endpoint_url_is_on_the_endpoint_the_caller_reached() {
    for region in REGIONS {
        let (router, engine) = router_with_engine();
        let arn = create_endpoint(&router, &format!("{region} endpoint"), region, MARKER).await;
        activate_endpoints_now(&engine);
        let id = endpoint_id(&arn).to_string();
        // The address the store says this emulator serves the ACME surface at, read from
        // the state the create opened it on rather than written here.
        let served = served_acme_address(&engine, region);

        for (what, host) in ADDRESSES {
            let case = format!("{region}, a caller that reached {what}");
            let url = advertised_url(&router, &case, *host, region, &arn).await;
            let (authority, path) = split_url(&case, &url);

            // The address is the one this emulator serves that surface at, which is a
            // loopback address this caller reaches — whichever endpoint shape it addressed.
            assert_eq!(
                authority, served,
                "case {case:?}: expected the URL on the address this emulator serves the ACME \
                 surface at ({served}), got {url:?}"
            );
            // Everything that identifies the resource is in the PATH: the plane, the
            // Region and the endpoint's own id, ending at the directory resource.
            assert_eq!(
                path,
                format!("{LOCAL_PLANE_PREFIX}/{region}/{id}/directory"),
                "case {case:?}: expected the ACME plane path, got {url:?}"
            );
            assert!(
                !authority.contains(&id),
                "case {case:?}: the endpoint id must not be a host label, got {authority:?}"
            );
            // Server-managed: nothing the caller sent appears in it.
            for sent in [MARKER, "marker", ACCOUNT] {
                assert!(
                    !url.contains(sent),
                    "case {case:?}: a server-managed URL carries nothing the caller sent, but \
                     {url:?} contains {sent:?}"
                );
            }
            // Identical on every read, and the same URL both readers report.
            assert_eq!(
                advertised_url(&router, &case, *host, region, &arn).await,
                url,
                "case {case:?}: the URL must be identical on every read"
            );
            assert_eq!(
                listed_url(&router, &case, *host, region, &arn).await,
                url,
                "case {case:?}: the listing must advertise the same URL as the read"
            );

            // And it is a URL this service SERVES: the directory, and the fresh nonce the
            // resource map it carries names.
            let document = assert_directory_served(&router, &case, &url).await;
            let nonce_url = document["newNonce"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: no newNonce in {document}"))
                .to_string();
            let response = fetch(&router, &case, "HEAD", &nonce_url).await;
            assert_eq!(
                response.status().as_u16(),
                200,
                "case {case:?}: HEAD {nonce_url} must answer 200"
            );
            assert!(
                !header(&response, "replay-nonce").is_empty(),
                "case {case:?}: HEAD {nonce_url} must answer a Replay-Nonce"
            );
        }

        // A caller that resolves AWS's own hostnames is told the Region's published
        // enrollment host, and that address serves the same endpoint's directory too —
        // both addresses name ONE endpoint record.
        for (what, host) in [
            ("the Region's control-plane host", Some(format!("acm.{region}.{DNS_SUFFIX}"))),
            ("no host at all", None),
        ] {
            let case = format!("{region}, a caller that addressed {what}");
            let url = advertised_url(&router, &case, host.as_deref(), region, &arn).await;
            assert_eq!(
                url,
                format!("https://{ENROLLMENT_LABEL}.{region}.{DNS_SUFFIX}/{id}/directory"),
                "case {case:?}: expected the Region's published enrollment host"
            );
            assert_directory_served(&router, &case, &url).await;
        }
    }
}

/// The `host:port` this emulator opened for the ACME surface, read off the service state —
/// the address a published `EndpointUrl` must name, asserted to be one a caller on this
/// machine can reach.
fn served_acme_address(engine: &std::sync::Arc<engine::InMemoryEngine>, region: &str) -> String {
    let store = engine.state::<service_com_amazonaws_acm_certificatemanager::state::ComAmazonawsAcmCertificatemanagerState>();
    let store = store.lock();
    let authority = store
        .acme_socket
        .as_ref()
        .unwrap_or_else(|| panic!("{region}: the create must have opened the ACME address"))
        .authority()
        .to_string();
    let (host, port) = authority
        .rsplit_once(':')
        .unwrap_or_else(|| panic!("{region}: the served address names no port: {authority}"));
    assert_eq!(
        host, "127.0.0.1",
        "{region}: the served ACME address must be this machine's own, got {authority:?}"
    );
    assert!(
        port.parse::<u16>().is_ok_and(|port| port > 0),
        "{region}: the served ACME address must name a port, got {authority:?}"
    );
    authority
}

/// RULE, to the end of the ACME account lifecycle: a client that is handed the
/// `EndpointUrl` and rewrites no host can register an account through it, and the account
/// URL that registration answers is a URL the control plane resolves — so
/// `DescribeAcmeAccount` and `RevokeAcmeAccount` are reachable for such a client.
#[tokio::test]
async fn an_account_registered_through_the_advertised_url_is_reported_by_the_control_plane() {
    let (router, engine) = router_with_engine();
    let host = Some("127.0.0.1:9297");
    let case = "a client that dials the advertised URL";
    let endpoint_arn = create_endpoint(&router, case, REGION, "registration-marker").await;
    activate_endpoints_now(&engine);

    // Everything the client knows comes from the advertised URL and the document at it.
    let url = advertised_url(&router, case, host, REGION, &endpoint_arn).await;
    let directory = assert_directory_served(&router, case, &url).await;
    let new_account = directory["newAccount"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no newAccount in {directory}"))
        .to_string();
    let new_nonce = directory["newNonce"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no newNonce in {directory}"))
        .to_string();

    // The external account binding this endpoint registers accounts with.
    let binding = create_binding(
        &router,
        case,
        &json!({
            "AcmeEndpointArn": endpoint_arn,
            "RoleArn": format!("arn:aws:iam::{ACCOUNT}:role/AcmeIssuer"),
        }),
    )
    .await;
    let binding_arn = binding["AcmeExternalAccountBindingArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: the binding carried no ARN"))
        .to_string();
    let (status, credentials) = call(
        &router,
        "GetAcmeExternalAccountBindingCredentials",
        &json!({ "AcmeExternalAccountBindingArn": binding_arn }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: the binding's credentials must be reported, got {status} {credentials}"
    );
    let key_id = credentials["KeyId"].as_str().unwrap_or_default().to_string();
    let mac_key = credentials["MacKey"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    // Register, over the wire, at the URL the directory named.
    let nonce = fetch(&router, case, "HEAD", &new_nonce).await;
    let nonce = header(&nonce, "replay-nonce");
    let key = AccountKey::ecdsa("ES256");
    let payload = json!({
        "termsOfServiceAgreed": true,
        "contact": ["mailto:advertised@example.com"],
        "externalAccountBinding":
            external_account_binding(&key_id, &mac_key, &new_account, &key.jwk()),
    });
    let response = post(
        &router,
        case,
        &new_account,
        &jws(
            &key,
            &Identification::Jwk,
            &nonce,
            &new_account,
            Some(&payload),
        ),
    )
    .await;
    let status = response.status().as_u16();
    let document = body_json(case, &response);
    assert_eq!(
        status, 201,
        "case {case:?}: the registration must be accepted, got {status} {document}"
    );
    let account_url = header(&response, "location");
    assert_eq!(
        split_url(case, &account_url).0,
        split_url(case, &new_account).0,
        "case {case:?}: the account URL must be on the address the client is talking to, got \
         {account_url:?}"
    );

    // The control plane resolves that URL: the account it names is the one that was
    // registered, and revoking it is observable through the same handle.
    for (step, expected) in [("after the registration", "VALID"), ("after the revocation", "REVOKED")]
    {
        if step == "after the revocation" {
            let (status, response) = call_at(
                &router,
                host,
                REGION,
                "RevokeAcmeAccount",
                &json!({ "AcmeEndpointArn": endpoint_arn, "AccountUrl": account_url }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: RevokeAcmeAccount must succeed, got {status} {response}"
            );
        }
        let (status, response) = call_at(
            &router,
            host,
            REGION,
            "DescribeAcmeAccount",
            &json!({ "AcmeEndpointArn": endpoint_arn, "AccountUrl": account_url }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?} {step}: DescribeAcmeAccount must resolve the advertised account \
             URL, got {status} {response}"
        );
        assert_eq!(
            response["AcmeAccount"]["AccountUrl"].as_str(),
            Some(account_url.as_str()),
            "case {case:?} {step}: the account must report the URL it was registered at, got \
             {response}"
        );
        assert_eq!(
            response["AcmeAccount"]["Status"].as_str(),
            Some(expected),
            "case {case:?} {step}: expected {expected}, got {response}"
        );
    }
}
