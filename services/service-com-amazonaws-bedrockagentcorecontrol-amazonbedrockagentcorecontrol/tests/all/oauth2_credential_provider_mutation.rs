// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `UpdateOauth2CredentialProvider`.
//!
//! The rule under test: an update REPLACES the whole configuration of a provider this
//! account already holds — the vendor, the `Oauth2ProviderConfigInput` member that vendor
//! selects and the secret the client secret it authenticates with lives in are all
//! `@required` on the request (`smithy_query(query="operation",
//! operation="UpdateOauth2CredentialProvider")`) — while everything that IDENTIFIES the
//! provider rather than configures it survives: the name it is addressed by, the ARN
//! minted from that name and the instant it was created. The update's own answer is what
//! a later read reports, and `lastUpdatedTime` never goes backwards.
//!
//! Its two refusal classes follow from the same rule: a `name` resolving to no provider of
//! this account is the modelled HTTP 404 (an update configures an existing provider, it
//! never mints one), and a vendor and a configuration member that disagree are the modelled
//! HTTP 400 — in both cases the account is left exactly as it was.
//!
//! Secrets Manager is mounted beside this service on ONE engine, the shape
//! `core/router/src/lib.rs` mounts them in, because a provider's client secret is stored in
//! a real secret there.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_bedrockagentcorecontrol_amazonbedrockagentcorecontrol::EmulatedService;

/// The region every request in this suite signs for.
const REGION: &str = "us-east-2";

/// `SecretSourceType`'s two values: a secret the SERVICE manages for the provider, and one
/// the CALLER manages in Secrets Manager (`smithy_query shape=SecretSourceType`).
const SOURCE_MANAGED: &str = "MANAGED";
const SOURCE_EXTERNAL: &str = "EXTERNAL";

/// The two documents an authorization server's metadata is published as — OpenID Connect
/// Discovery 1.0 and RFC 8414 — which are the only endings `Oauth2Discovery$discoveryUrl`'s
/// modelled pattern admits (`smithy_query shape=Oauth2Discovery`).
const OPENID_CONFIGURATION: &str = "/.well-known/openid-configuration";
const OAUTH_AUTHORIZATION_SERVER: &str = "/.well-known/oauth-authorization-server";

/// `Status`'s value for a provider whose secret is stored or resolved, which is what an
/// update that answered has already done (`smithy_query shape=Status`).
const STATUS_READY: &str = "READY";

struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        router.register_service(
            service_com_amazonaws_secretsmanager_secretsmanager::EmulatedService::new(Arc::clone(
                &engine,
            )),
        );
        Self { router }
    }

    /// POST one `restJson1` operation of this service, signed for [`REGION`].
    async fn control_plane(&self, uri: &str, body: &Value) -> (u16, Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri(uri)
            .header("content-type", "application/json")
            .header("host", "bedrock-agentcore-control.amazonaws.com")
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/bedrock-agentcore/aws4_request, SignedHeaders=host, Signature=deadbeef"
                ),
            )
            .body(Bytes::from(
                serde_json::to_vec(body).expect("the request body is serializable"),
            ))
            .expect("a valid request");
        answer(self.router.dispatch(request).await)
    }

    /// POST one `awsJson1_1` Secrets Manager operation, signed for [`REGION`].
    async fn secrets_manager(&self, operation: &str, body: &Value) -> (u16, Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri(format!("https://secretsmanager.{REGION}.amazonaws.com/"))
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", format!("secretsmanager.{operation}"))
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/secretsmanager/aws4_request, SignedHeaders=host;x-amz-target, Signature=deadbeef"
                ),
            )
            .body(Bytes::from(
                serde_json::to_vec(body).expect("the request body is serializable"),
            ))
            .expect("a valid request");
        answer(self.router.dispatch(request).await)
    }

    /// Create a secret of the CALLER's holding `value`, answered as the ARN Secrets
    /// Manager minted for it.
    async fn create_secret(&self, name: &str, value: &str) -> String {
        let (status, created) = self
            .secrets_manager(
                "CreateSecret",
                &json!({
                    "Name": name,
                    "SecretString": value,
                    // Secrets Manager stakes the version a create stores on a
                    // `ClientRequestToken` the caller names ("We recommend a UUID-type
                    // value"), so this suite's one secret carries one.
                    "ClientRequestToken": "4f1c0a3e-9b7d-4c21-8a55-0d6e2f7b1c94",
                }),
            )
            .await;
        assert_eq!(
            status, 200,
            "expected HTTP 200 from Secrets Manager CreateSecret for {name:?}, got {status} \
             ({created})"
        );
        created["ARN"]
            .as_str()
            .unwrap_or_else(|| panic!("CreateSecret reported no ARN for {name:?} in {created}"))
            .to_string()
    }

    async fn create(&self, body: &Value) -> (u16, Value) {
        self.control_plane("/identities/CreateOauth2CredentialProvider", body)
            .await
    }

    async fn update(&self, body: &Value) -> (u16, Value) {
        self.control_plane("/identities/UpdateOauth2CredentialProvider", body)
            .await
    }

    async fn get(&self, name: &str) -> (u16, Value) {
        self.control_plane(
            "/identities/GetOauth2CredentialProvider",
            &json!({"name": name}),
        )
        .await
    }

    async fn list(&self) -> (u16, Value) {
        self.control_plane("/identities/ListOauth2CredentialProviders", &json!({}))
            .await
    }

    /// Create a provider that must be served, answered as the create's envelope.
    async fn create_ok(&self, body: &Value, case: &str) -> Value {
        let (status, created) = self.create(body).await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected HTTP 201 from the create, got {status} ({created})"
        );
        created
    }
}

fn answer(response: http::Response<Bytes>) -> (u16, Value) {
    let status = response.status().as_u16();
    let parsed = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// The error code `restJson1` puts in `__type` / `x-amzn-errortype`.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string()
}

/// A configuration carried in `member`, with the three members every built-in vendor's
/// configuration declares.
fn vendor_config(member: &str, client_id: &str, client_secret: &str) -> Value {
    json!({member: {"clientId": client_id, "clientSecret": client_secret}})
}

/// The single member an `Oauth2ProviderConfigOutput` union carries, and its name.
fn reported_config(envelope: &Value) -> (String, Value) {
    envelope["oauth2ProviderConfigOutput"]
        .as_object()
        .and_then(|members| {
            members
                .iter()
                .next()
                .map(|(member, config)| (member.clone(), config.clone()))
        })
        .unwrap_or_default()
}

/// The timestamp `member` carries, as the epoch seconds `restJson1` renders a `Timestamp`
/// as.
fn instant(envelope: &Value, member: &str) -> f64 {
    envelope[member].as_f64().unwrap_or(f64::NAN)
}

/// The names a listing reports.
fn listed_names(body: &Value) -> Vec<String> {
    body["credentialProviders"]
        .as_array()
        .map(|providers| {
            providers
                .iter()
                .map(|provider| provider["name"].as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// Successive updates of ONE provider each replace its whole configuration, keep the
/// identity it was created with, and are what the read reports afterwards.
///
/// The rows differ along the shape of the configuration the update supplies: another
/// built-in vendor's member, the custom member with an authorization server of the
/// caller's, the same vendor again with a different value in it (the directory a Microsoft
/// provider is scoped to), the included member of a vendor the union declares no member of
/// its own for, and a configuration whose client secret is one the CALLER manages — so the
/// rule under test is "the configuration the update carried is the provider's
/// configuration", not any one vendor's.
#[tokio::test]
async fn an_update_replaces_the_configuration_and_keeps_the_providers_identity() {
    let emulator = Emulator::new();
    let provider = "lifecycle-provider";

    let created = emulator
        .create_ok(
            &json!({
                "name": provider,
                "credentialProviderVendor": "GoogleOauth2",
                "oauth2ProviderConfigInput": vendor_config(
                    "googleOauth2ProviderConfig", "the-first-client", "the-first-secret",
                ),
                "tags": {"owner": "identity"},
            }),
            "the provider under test",
        )
        .await;
    let arn = created["credentialProviderArn"].clone();
    let managed_secret_arn = created["clientSecretArn"]["secretArn"].clone();
    let (_, before_any_update) = emulator.get(provider).await;
    let created_time = instant(&before_any_update, "createdTime");

    // A secret of the caller's for the EXTERNAL row, created through Secrets Manager's own
    // operation so the provider is updated against a secret that really exists.
    let external_secret_arn = emulator
        .create_secret(
            "a-partner-oauth2-client",
            &json!({"clientSecret": "the-partners-secret"}).to_string(),
        )
        .await;

    // (case, vendor, the configuration supplied, the member expected back, the discovery
    //  expected back, the secret source expected back)
    let rows: [(&str, &str, Value, &str, String, &str); 5] = [
        (
            "another built-in vendor, whose own metadata document is reported",
            "GithubOauth2",
            vendor_config(
                "githubOauth2ProviderConfig",
                "a-github-client",
                "a-github-secret",
            ),
            "githubOauth2ProviderConfig",
            // GitHub serves OAuth2 without OpenID Connect, so RFC 8414's document.
            format!("https://github.com{OAUTH_AUTHORIZATION_SERVER}"),
            SOURCE_MANAGED,
        ),
        (
            "the custom member, with an authorization server of the caller's",
            "CustomOauth2",
            json!({"customOauth2ProviderConfig": {
                "oauthDiscovery": {"discoveryUrl": format!("https://auth.example.com{OPENID_CONFIGURATION}")},
                "clientId": "a-custom-client",
                "clientSecret": "a-custom-secret",
            }}),
            "customOauth2ProviderConfig",
            format!("https://auth.example.com{OPENID_CONFIGURATION}"),
            SOURCE_MANAGED,
        ),
        (
            "a vendor whose server is scoped to the directory the request names",
            "MicrosoftOauth2",
            json!({"microsoftOauth2ProviderConfig": {
                "clientId": "a-microsoft-client",
                "clientSecret": "a-microsoft-secret",
                "tenantId": "00000000-0000-0000-0000-00000000000a",
            }}),
            "microsoftOauth2ProviderConfig",
            format!(
                "https://login.microsoftonline.com/00000000-0000-0000-0000-00000000000a/v2.0{OPENID_CONFIGURATION}"
            ),
            SOURCE_MANAGED,
        ),
        (
            "the same vendor again with another directory, which is another server",
            "MicrosoftOauth2",
            json!({"microsoftOauth2ProviderConfig": {
                "clientId": "a-microsoft-client",
                "clientSecret": "a-microsoft-secret",
                "tenantId": "00000000-0000-0000-0000-00000000000b",
            }}),
            "microsoftOauth2ProviderConfig",
            format!(
                "https://login.microsoftonline.com/00000000-0000-0000-0000-00000000000b/v2.0{OPENID_CONFIGURATION}"
            ),
            SOURCE_MANAGED,
        ),
        (
            "the included member of a vendor the union declares no member for, over a \
             secret the caller manages",
            "OktaOauth2",
            json!({"includedOauth2ProviderConfig": {
                "clientId": "an-okta-client",
                "issuer": "https://dev-7.okta.example.com/",
                "clientSecretSource": SOURCE_EXTERNAL,
                "clientSecretConfig": {
                    "secretId": "a-partner-oauth2-client",
                    "jsonKey": "clientSecret",
                },
            }}),
            "includedOauth2ProviderConfig",
            format!("https://dev-7.okta.example.com{OPENID_CONFIGURATION}"),
            SOURCE_EXTERNAL,
        ),
    ];

    let mut previous_update = created_time;
    for (case, vendor, configuration, expected_member, expected_discovery, expected_source) in rows
    {
        let (status, updated) = emulator
            .update(&json!({
                "name": provider,
                "credentialProviderVendor": vendor,
                "oauth2ProviderConfigInput": configuration,
            }))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from the update, got {status} ({updated})"
        );

        // The configuration the update carried is the provider's configuration: the member
        // it supplied, the authorization server that configuration names, and nothing of
        // the configuration it replaced.
        let (member, config) = reported_config(&updated);
        assert_eq!(
            (member.as_str(), config["oauthDiscovery"]["discoveryUrl"].clone()),
            (expected_member, json!(expected_discovery)),
            "case {case:?}: the update reports the configuration it was given, got {updated}"
        );
        assert_eq!(
            (
                updated["credentialProviderVendor"].clone(),
                updated["clientSecretSource"].clone(),
                updated["status"].clone()
            ),
            (json!(vendor), json!(expected_source), json!(STATUS_READY)),
            "case {case:?}: the update reports the vendor and the secret source it was \
             given, got {updated}"
        );
        // The client secret the caller handed over is held in a secret rather than echoed.
        assert!(
            !updated.to_string().contains("-secret\""),
            "case {case:?}: the update echoed a client secret, in {updated}"
        );
        let expected_secret_arn = if expected_source == SOURCE_EXTERNAL {
            json!(external_secret_arn)
        } else {
            // A managed secret is named for the provider, so every managed update of one
            // provider puts its value into the same secret.
            managed_secret_arn.clone()
        };
        assert_eq!(
            updated["clientSecretArn"]["secretArn"], expected_secret_arn,
            "case {case:?}: the update reports the secret the client secret lives in, got \
             {updated}"
        );

        // What identifies the provider survives the update, and the clock only moves
        // forward.
        assert_eq!(
            (
                updated["name"].clone(),
                updated["credentialProviderArn"].clone(),
                instant(&updated, "createdTime")
            ),
            (json!(provider), arn.clone(), created_time),
            "case {case:?}: the update changed what identifies the provider, got {updated}"
        );
        let last_updated = instant(&updated, "lastUpdatedTime");
        assert!(
            last_updated >= previous_update,
            "case {case:?}: lastUpdatedTime went backwards, expected at least \
             {previous_update}, got {last_updated}"
        );
        previous_update = last_updated;

        // The update WROTE what it reported: the read answers with the same provider, and
        // the account still holds exactly one of it.
        let (status, fetched) = emulator.get(provider).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from the read, got {status} ({fetched})"
        );
        for member in [
            "credentialProviderVendor",
            "oauth2ProviderConfigOutput",
            "clientSecretArn",
            "clientSecretSource",
            "clientSecretJsonKey",
            "credentialProviderArn",
            "name",
            "createdTime",
            "lastUpdatedTime",
            "status",
        ] {
            assert_eq!(
                fetched[member], updated[member],
                "case {case:?}: the read disagrees with the update about {member:?}, got \
                 {fetched}"
            );
        }
        assert_eq!(
            listed_names(&emulator.list().await.1),
            vec![provider.to_string()],
            "case {case:?}: the account holds exactly the one updated provider"
        );
    }
}

/// An update of a `name` this account holds no provider under is the modelled 404, and
/// mints nothing: an update configures an existing provider.
///
/// The rows differ along why the name resolves to nothing — never used at all, the name of
/// a provider in another letter case, and a name whose only create was refused.
#[tokio::test]
async fn an_update_of_a_name_no_provider_resolves_to_is_refused() {
    let emulator = Emulator::new();
    emulator
        .create_ok(
            &json!({
                "name": "held-provider",
                "credentialProviderVendor": "SlackOauth2",
                "oauth2ProviderConfigInput": vendor_config(
                    "slackOauth2ProviderConfig", "a-client", "a-secret",
                ),
            }),
            "the one provider this account holds",
        )
        .await;

    // A create whose vendor and configuration disagree leaves no provider behind, so this
    // name is one no provider stands under either.
    let (status, _) = emulator
        .create(&json!({
            "name": "refused-provider",
            "credentialProviderVendor": "GoogleOauth2",
            "oauth2ProviderConfigInput": vendor_config(
                "githubOauth2ProviderConfig", "a-client", "a-secret",
            ),
        }))
        .await;
    assert_eq!(
        status, 400,
        "the setup create is refused, so its name holds no provider"
    );

    // (case, the name the update addresses)
    let rows: [(&str, &str); 3] = [
        ("a name never used", "no-such-provider"),
        (
            "the held provider's name in another letter case",
            "Held-Provider",
        ),
        ("a name whose only create was refused", "refused-provider"),
    ];

    for (case, name) in rows {
        let (status, refused) = emulator
            .update(&json!({
                "name": name,
                "credentialProviderVendor": "GoogleOauth2",
                "oauth2ProviderConfigInput": vendor_config(
                    "googleOauth2ProviderConfig", "a-client", "a-secret",
                ),
            }))
            .await;
        assert_eq!(
            (status, error_code(&refused).as_str()),
            (404, "ResourceNotFoundException"),
            "case {case:?}: expected HTTP 404 ResourceNotFoundException, got {status} \
             ({refused})"
        );
        assert_eq!(
            refused["message"].as_str().unwrap_or_default(),
            format!("Oauth2CredentialProvider not found for {name}"),
            "case {case:?}: unexpected refusal message, got {refused}"
        );

        let (status, fetched) = emulator.get(name).await;
        assert_eq!(
            (status, error_code(&fetched).as_str()),
            (404, "ResourceNotFoundException"),
            "case {case:?}: the refused update minted a provider, got {status} ({fetched})"
        );
        assert_eq!(
            listed_names(&emulator.list().await.1),
            vec!["held-provider".to_string()],
            "case {case:?}: the refused update changed what the account holds"
        );
    }
}

/// An update whose vendor and configuration member disagree is the modelled 400, and the
/// provider is left exactly as it was.
///
/// The rows differ along which pair disagrees: a vendor with a member of its own given
/// another vendor's, a vendor with one given the custom member, the custom vendor given a
/// built-in member, and a vendor the union declares no member of its own for given one.
#[tokio::test]
async fn an_update_whose_vendor_and_configuration_disagree_leaves_the_provider_unchanged() {
    let emulator = Emulator::new();
    let provider = "unchanged-provider";
    emulator
        .create_ok(
            &json!({
                "name": provider,
                "credentialProviderVendor": "AtlassianOauth2",
                "oauth2ProviderConfigInput": vendor_config(
                    "atlassianOauth2ProviderConfig", "a-client", "a-secret",
                ),
            }),
            "the provider under test",
        )
        .await;
    let (_, before) = emulator.get(provider).await;

    // (case, the vendor declared, the configuration supplied)
    let rows: [(&str, &str, Value); 4] = [
        (
            "a vendor given another vendor's member",
            "GoogleOauth2",
            vendor_config("linkedinOauth2ProviderConfig", "a-client", "a-secret"),
        ),
        (
            "a vendor given the custom member",
            "SalesforceOauth2",
            json!({"customOauth2ProviderConfig": {
                "oauthDiscovery": {"discoveryUrl": format!("https://auth.example.com{OPENID_CONFIGURATION}")},
                "clientId": "a-client",
                "clientSecret": "a-secret",
            }}),
        ),
        (
            "the custom vendor given a built-in member",
            "CustomOauth2",
            vendor_config("slackOauth2ProviderConfig", "a-client", "a-secret"),
        ),
        (
            "a vendor the union declares no member for, given a named vendor's member",
            "NotionOauth2",
            vendor_config("googleOauth2ProviderConfig", "a-client", "a-secret"),
        ),
    ];

    for (case, vendor, configuration) in rows {
        let (status, refused) = emulator
            .update(&json!({
                "name": provider,
                "credentialProviderVendor": vendor,
                "oauth2ProviderConfigInput": configuration,
            }))
            .await;
        assert_eq!(
            (status, error_code(&refused).as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected HTTP 400 ValidationException, got {status} ({refused})"
        );
        assert_eq!(
            refused["message"].as_str().unwrap_or_default(),
            format!("Provided configuration does not match selected type: {vendor}"),
            "case {case:?}: unexpected refusal message, got {refused}"
        );

        let (status, after) = emulator.get(provider).await;
        assert_eq!(
            (status, &after),
            (200, &before),
            "case {case:?}: the refused update changed the provider, got {status} ({after})"
        );
    }
}

/// A `discoveryUrl` outside its modelled pattern refuses the update in this service's
/// constraint-violation wording, and the provider keeps the server it had.
#[tokio::test]
async fn an_update_with_a_discovery_url_outside_its_pattern_is_refused() {
    let emulator = Emulator::new();
    let provider = "pattern-provider";
    let served = format!("https://first.example.com{OPENID_CONFIGURATION}");
    emulator
        .create_ok(
            &json!({
                "name": provider,
                "credentialProviderVendor": "CustomOauth2",
                "oauth2ProviderConfigInput": {"customOauth2ProviderConfig": {
                    "oauthDiscovery": {"discoveryUrl": served},
                    "clientId": "a-client",
                    "clientSecret": "a-secret",
                }},
            }),
            "the provider under test",
        )
        .await;

    // Each way the pattern can fail: no well-known path, a document it does not admit, no
    // issuer before the document, and a segment after it (the pattern is anchored).
    let rows: [(&str, String); 4] = [
        (
            "no well-known path at all",
            "https://second.example.com/oauth".to_string(),
        ),
        (
            "a well-known document the pattern does not admit",
            "https://second.example.com/.well-known/jwks.json".to_string(),
        ),
        (
            "the metadata document with no issuer before it",
            OPENID_CONFIGURATION.to_string(),
        ),
        (
            "the metadata document followed by another segment",
            format!("https://second.example.com{OPENID_CONFIGURATION}/v2"),
        ),
    ];

    for (case, url) in rows {
        let (status, refused) = emulator
            .update(&json!({
                "name": provider,
                "credentialProviderVendor": "CustomOauth2",
                "oauth2ProviderConfigInput": {"customOauth2ProviderConfig": {
                    "oauthDiscovery": {"discoveryUrl": url},
                    "clientId": "another-client",
                    "clientSecret": "another-secret",
                }},
            }))
            .await;
        assert_eq!(
            (status, error_code(&refused).as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected HTTP 400 ValidationException, got {status} ({refused})"
        );

        let (_, fetched) = emulator.get(provider).await;
        assert_eq!(
            reported_config(&fetched).1["oauthDiscovery"]["discoveryUrl"],
            json!(served),
            "case {case:?}: the refused update changed the provider's authorization \
             server, got {fetched}"
        );
    }
}
