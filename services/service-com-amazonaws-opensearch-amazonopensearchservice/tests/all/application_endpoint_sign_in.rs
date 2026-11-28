// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for WHO is admitted to the endpoint an OpenSearch UI application
//! publishes — the address `GetApplication` reports for a client to FOLLOW, which this
//! emulator serves on its own front door.
//!
//! Every case drives the real wire path — an HTTP request through the multi-service router
//! into the mounted service, sent to the address the control plane PUBLISHED — so what is
//! asserted is what a client sees, not what a handler returns. The harness comes from
//! [`super::domain_lifecycle`] and the create from [`super::application_updates`]; this
//! suite adds no second copy of either.
//!
//! The RULE under test: **an application's interface is reached through a SIGN-IN
//! (`IamIdentityCenterOptions` is "configuration settings for IAM Identity Center in an
//! OpenSearch application"), so a fetch of the endpoint it publishes reads the application
//! only when it PRESENTS an AWS credential — the identity this emulator has of a caller, and
//! the one its front door verifies for every other surface it serves. A fetch carrying none
//! is refused and told nothing of any application: not its name, not its status, not a data
//! source, and not even whether the address names one.**
//!
//! The rows differ along what the rule generalises over: whether the application carries IAM
//! Identity Center options at all and whether the member that enables that sign-in is `true`,
//! `false` or absent — none of which makes an application readable by a caller that presents
//! no credential — and, for the addresses that name no application, whether the fetch
//! presented one.

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::http;

use super::application_updates::{create_application, get_application};
use super::domain_lifecycle::{string_member, Answer, Emulator, DEFAULT_ACCESS_KEY_ID, REGION};

/// The IAM Identity Center instance an application names, in the form `ARN`'s own constraint
/// admits (`@length(20, 2048)`).
const SSO_INSTANCE: &str = "arn:aws:sso:::instance/ssoins-72239f2e1d8a1f85";

/// The IAM role an Identity Center application is reached through.
const SSO_ROLE: &str = "arn:aws:iam::123456789012:role/identity-center";

/// The status this emulator refuses an unauthenticated caller on.
const UNAUTHENTICATED_STATUS: u16 = 403;

/// The error code that refusal travels under, and the sentence it carries — the ones this
/// emulator states for a request that has to be signed and is not. Restated here rather than
/// imported from the service, so changing it in the service fails this suite instead of being
/// asserted against itself.
const UNAUTHENTICATED_CODE: &str = "AccessDenied";
/// See [`UNAUTHENTICATED_CODE`].
const UNAUTHENTICATED_MESSAGE: &str = "Sign the request with the emulator credentials";

/// The authority and the request path of `url`, split the way a client that FOLLOWS the URL
/// splits it: whatever stands before the first `/` of the address is the host it dials, and
/// the rest is the path it sends.
fn dialled(url: &str) -> (String, String) {
    let address = url
        .strip_prefix("https://")
        .expect("a published endpoint is an absolute https URL");
    match address.find('/') {
        Some(end) => (address[..end].to_string(), address[end..].to_string()),
        None => (address.to_string(), "/".to_string()),
    }
}

/// A fetch of `url` as a client that holds the address makes it, presenting an AWS credential
/// where `credentialed` says so and none at all otherwise.
async fn followed(emulator: &Emulator, url: &str, credentialed: bool) -> Answer {
    let (authority, path) = dialled(url);
    let mut request = http::Request::builder()
        .method("GET")
        .uri(path)
        .header("host", authority);
    if credentialed {
        request = request.header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 \
                 Credential={DEFAULT_ACCESS_KEY_ID}/20260101/{REGION}/es/aws4_request, \
                 SignedHeaders=host, Signature=0"
            ),
        );
    }
    let request = request
        .body(Bytes::new())
        .expect("the test request is well formed");
    emulator.dispatch(request).await
}

/// Create an application with `options` as its `iamIdentityCenterOptions` — absent where the
/// row names none — and hand back the URL it publishes.
async fn given_application(emulator: &Emulator, name: &str, options: Option<Value>) -> String {
    let mut body = json!({
        "name": name,
        "dataSources": [{
            "dataSourceArn": format!("arn:aws:es:{REGION}:123456789012:domain/logs"),
            "dataSourceDescription": "the logs domain",
        }],
    });
    if let Some(options) = options {
        body["iamIdentityCenterOptions"] = options;
    }
    let created = create_application(emulator, body).await;
    assert_eq!(
        created.status, 200,
        "case CreateApplication({name:?}): expected 200, got {} with {:?}",
        created.status, created.body
    );
    let id = string_member(&created.body, "id", name);
    let read = get_application(emulator, &id).await;
    assert_eq!(
        read.status, 200,
        "case GetApplication({name:?}): expected 200, got {} with {:?}",
        read.status, read.body
    );
    string_member(&read.body, "endpoint", name)
}

/// One row: how the application's sign-in is configured, which is what the rule generalises
/// over.
struct Row {
    case: &'static str,
    name: &'static str,
    options: Option<Value>,
}

/// The table every case below is made over.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "an application the create configured no sign-in for",
            name: "signin-absent-ui",
            options: None,
        },
        Row {
            case: "options that carry the sign-in switched off",
            name: "signin-disabled-ui",
            options: Some(json!({ "enabled": false })),
        },
        Row {
            case: "options that name an instance without enabling the sign-in",
            name: "signin-unset-ui",
            options: Some(json!({ "iamIdentityCenterInstanceArn": SSO_INSTANCE })),
        },
        Row {
            case: "an application reached through IAM Identity Center",
            name: "signin-enabled-ui",
            options: Some(json!({
                "enabled": true,
                "iamIdentityCenterInstanceArn": SSO_INSTANCE,
                "iamRoleForIdentityCenterApplicationArn": SSO_ROLE,
            })),
        },
    ]
}

/// RULE: see the module docs — the endpoint an application publishes reads that application
/// only for a fetch presenting an AWS credential, and a fetch presenting none is told nothing
/// of it, whatever the application's sign-in is configured as.
#[tokio::test]
async fn an_application_endpoint_reads_its_application_only_for_a_credentialed_fetch() {
    let emulator = Emulator::new();

    for row in rows() {
        let case = row.case;
        let url = given_application(&emulator, row.name, row.options.clone()).await;

        // The fetch presenting NO credential: refused, and told nothing of the application.
        let anonymous = followed(&emulator, &url, false).await;
        assert_eq!(
            (
                anonymous.status,
                anonymous.error.as_deref(),
                anonymous.body["message"].as_str(),
            ),
            (
                UNAUTHENTICATED_STATUS,
                Some(UNAUTHENTICATED_CODE),
                Some(UNAUTHENTICATED_MESSAGE),
            ),
            "case {case}: expected the unauthenticated refusal, got {} with {:?}",
            anonymous.status,
            anonymous.body
        );
        for member in ["id", "name", "status", "dataSources"] {
            assert!(
                anonymous.body.get(member).is_none(),
                "case {case}: expected no {member:?} in the refusal, got {:?}",
                anonymous.body
            );
        }

        // The fetch presenting one: served the application the address names.
        let credentialed = followed(&emulator, &url, true).await;
        assert_eq!(
            (credentialed.status, credentialed.body["name"].as_str()),
            (200, Some(row.name)),
            "case {case}: expected a credentialed fetch to be served the application, got {} \
             with {:?}",
            credentialed.status,
            credentialed.body
        );
    }
}

/// RULE: an address of that shape naming an application this account does not hold states
/// that there is none — to a fetch that presented a credential. A fetch that presented none is
/// refused before any address is resolved, so it learns nothing about which addresses name an
/// application and which do not.
#[tokio::test]
async fn an_endpoint_naming_no_application_states_that_there_is_none() {
    let emulator = Emulator::new();
    // One application that exists, so the address every row below dials is the one this
    // emulator serves, with only the id in the path changed.
    let held = given_application(&emulator, "signin-held-ui", None).await;
    let (authority, path) = dialled(&held);
    let (prefix, _) = path
        .rsplit_once('/')
        .expect("a published endpoint names the application in its path");

    for (case, id) in [
        ("an id nothing was ever created under", "abc"),
        (
            "a 20-character id of the width this service mints",
            "ab3xk9qp17vn42zd0c8m",
        ),
    ] {
        let url = format!("https://{authority}{prefix}/{id}");

        let credentialed = followed(&emulator, &url, true).await;
        assert_eq!(
            credentialed.status, 404,
            "case {case}: expected 404 for a credentialed fetch, got {} with {:?}",
            credentialed.status, credentialed.body
        );
        assert_eq!(
            credentialed.body["message"].as_str(),
            Some("Application does not exist"),
            "case {case}: expected the no-such-application statement, got {:?}",
            credentialed.body
        );

        // And the uncredentialed fetch is refused exactly as it is for an address that DOES
        // name an application: what it is told cannot tell the two apart.
        let anonymous = followed(&emulator, &url, false).await;
        assert_eq!(
            (anonymous.status, anonymous.body["message"].as_str()),
            (UNAUTHENTICATED_STATUS, Some(UNAUTHENTICATED_MESSAGE)),
            "case {case}: expected the unauthenticated refusal, got {} with {:?}",
            anonymous.status,
            anonymous.body
        );
    }
}
