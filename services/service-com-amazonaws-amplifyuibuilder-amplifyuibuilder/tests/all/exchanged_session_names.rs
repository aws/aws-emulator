// SPDX-License-Identifier: CC0-1.0

//! Which refresh tokens the token-session store can resolve: the names a token
//! operation puts into it.
//!
//! The rule under test: a token this service hands back is drawn from the emulator's
//! credential store and is no value the request carried, and the session a call stores
//! is named by exactly the refresh token of that call — the one an exchange MINTED (its
//! model declares a `@required` `refreshToken` output,
//! `smithy_query(query="operation",operation="ExchangeCodeForToken")`), or the one a
//! refresh was presented, whose model declares no token output at all
//! (`smithy_query(query="operation",operation="RefreshToken")`) and which therefore
//! issues none.
//!
//! So, over a span of exchange requests:
//!
//! 1. The `refreshToken` an exchange answers is minted: it matches neither the access
//!    code, the redirect URI, the `clientId` nor the provider the request carried, it
//!    differs from the `accessToken` of the same answer, and no two exchanges answer the
//!    same token.
//! 2. The session the exchange started is resolvable under THAT name and under no other
//!    name the call put within reach — not the access token it answered, and not any
//!    value the request carried.
//! 3. A `RefreshToken` answer carries no refresh token of its own, and the session it
//!    writes is named by the token its caller presented.

use std::collections::BTreeSet;
use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_amplifyuibuilder_amplifyuibuilder::state::{
    service_state, ProviderTokenKey, ProviderTokenSession, PROVIDER_ACCESS_TOKEN_CHARS,
};
use service_com_amazonaws_amplifyuibuilder_amplifyuibuilder::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 credential scope naming this service: it is what routes a request here.
const SIGNED_FOR_AMPLIFYUIBUILDER: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/eu-west-1/amplifyuibuilder/aws4_request, \
     SignedHeaders=host, Signature=abc";

/// The only `TokenProviders` value the model declares
/// (`smithy_query(query="shape",shape="TokenProviders")`).
const PROVIDER: &str = "figma";

/// One emulator, plus the engine its state is read back through.
struct Emulator {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        Self { router, engine }
    }

    async fn call(&self, uri: &str, body: Value) -> (u16, Value) {
        let request = ::http::Request::builder()
            .method("POST")
            .uri(uri)
            .header("host", "amplifyuibuilder.eu-west-1.amazonaws.com")
            .header("authorization", SIGNED_FOR_AMPLIFYUIBUILDER)
            .header("content-type", "application/json")
            .body(Bytes::from(body.to_string()))
            .expect("the test request is well formed");

        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let bytes = response.into_body();
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or_else(|error| {
                panic!(
                    "{uri} answered a body that is not JSON ({error}): {:?}",
                    String::from_utf8_lossy(&bytes)
                )
            })
        };
        (status, body)
    }

    /// `POST /tokens/{provider}` with the `ExchangeCodeForTokenRequestBody` payload.
    async fn exchange(
        &self,
        code: &str,
        redirect_uri: &str,
        client_id: Option<&str>,
    ) -> (u16, Value) {
        let mut body = json!({"code": code, "redirectUri": redirect_uri});
        if let Some(client_id) = client_id {
            body["clientId"] = json!(client_id);
        }
        self.call(&format!("/tokens/{PROVIDER}"), body).await
    }

    /// `POST /tokens/{provider}/refresh` with the `RefreshTokenRequestBody` payload.
    async fn refresh(&self, token: &str, client_id: Option<&str>) -> (u16, Value) {
        let mut body = json!({"token": token});
        if let Some(client_id) = client_id {
            body["clientId"] = json!(client_id);
        }
        self.call(&format!("/tokens/{PROVIDER}/refresh"), body)
            .await
    }

    /// The session the store holds for one (provider, clientId, token), if any.
    fn session(&self, key: &ProviderTokenKey) -> Option<ProviderTokenSession> {
        service_state(&*self.engine)
            .expect("the shared engine holds this service's state")
            .lock()
            .provider_token_session(key)
            .cloned()
    }

    /// How many token sessions the store holds at all.
    fn session_count(&self) -> usize {
        service_state(&*self.engine)
            .expect("the shared engine holds this service's state")
            .lock()
            .provider_token_sessions
            .len()
    }
}

/// The key a request addresses, as the store names it.
fn key(token: &str, client_id: Option<&str>) -> ProviderTokenKey {
    (
        PROVIDER.to_string(),
        client_id.map(str::to_string),
        token.to_string(),
    )
}

/// The member of the answer, or the empty string when it carries none.
fn member(body: &Value, name: &str) -> String {
    body[name].as_str().unwrap_or_default().to_string()
}

/// One exchange request, differing from its neighbours along what the rule generalises
/// over: the form and size of the access code, the form of the redirect URI, and
/// whether — and which — `clientId` the request names.
struct Case {
    label: &'static str,
    code: String,
    redirect_uri: &'static str,
    client_id: Option<&'static str>,
}

fn cases() -> Vec<Case> {
    vec![
        Case {
            label: "a short code, no client named",
            code: "zq".to_string(),
            redirect_uri: "https://studio.example.com/cb",
            client_id: None,
        },
        Case {
            label: "an opaque code of the length a provider hands out",
            code: "4Hn8tQ1wZ7mS3kR9bV6pL0xD2cY5fJa".to_string(),
            redirect_uri: "https://studio.example.com/cb?state=42",
            client_id: Some("amplify-studio"),
        },
        Case {
            label: "a long code for a client on loopback",
            code: "k".repeat(400),
            redirect_uri: "http://127.0.0.1:8080/oauth/return",
            client_id: Some("loopback-client"),
        },
        Case {
            label: "a code outside ASCII, client named the empty string",
            code: "autorisations-códe-✓".to_string(),
            redirect_uri: "https://studio.example.com/a/deeper/path",
            client_id: Some(""),
        },
        Case {
            label: "a code of URL-safe punctuation",
            code: "-._~code_42-".to_string(),
            redirect_uri: "https://studio.example.com/cb#fragment",
            client_id: Some("another-client"),
        },
    ]
}

/// Rules 1 and 2: the refresh token an exchange answers is minted rather than taken
/// from the request, and it is the ONLY name the session it started resolves under.
#[tokio::test]
async fn an_exchange_names_its_session_by_the_token_it_minted() {
    let emulator = Emulator::new();
    // Every token any row was handed: a repeat is a token the handler did not mint per
    // request.
    let mut minted: BTreeSet<String> = BTreeSet::new();
    let mut expected_sessions = 0usize;

    for case in cases() {
        let label = case.label;
        let (status, body) = emulator
            .exchange(&case.code, case.redirect_uri, case.client_id)
            .await;
        assert_eq!(
            status, 200,
            "case {label:?}: an exchange must answer HTTP 200, got {status} with {body:?}"
        );
        let access_token = member(&body, "accessToken");
        let refresh_token = member(&body, "refreshToken");

        // Rule 1: minted, and no value of the request.
        assert_eq!(
            refresh_token.chars().count(),
            PROVIDER_ACCESS_TOKEN_CHARS,
            "case {label:?}: the refresh token must carry {PROVIDER_ACCESS_TOKEN_CHARS} \
             characters of minted secret, got {:?}",
            refresh_token.chars().count()
        );
        for (what, carried) in [
            ("the access code", case.code.as_str()),
            ("the redirect URI", case.redirect_uri),
            ("the clientId", case.client_id.unwrap_or_default()),
            ("the provider", PROVIDER),
            ("the access token it answered", access_token.as_str()),
        ] {
            assert_ne!(
                refresh_token.as_str(), carried,
                "case {label:?}: the refresh token must be minted, not {what}, got \
                 {refresh_token:?}"
            );
        }
        for (what, token) in [("accessToken", &access_token), ("refreshToken", &refresh_token)] {
            assert!(
                minted.insert(token.clone()),
                "case {label:?}: every token handed back must be its own, got {what} \
                 {token:?} again"
            );
        }

        // Rule 2: the session resolves under the minted refresh token.
        expected_sessions += 1;
        let named = key(&refresh_token, case.client_id);
        let session = emulator.session(&named).unwrap_or_else(|| {
            panic!(
                "case {label:?}: the store must hold the exchange's session under the \
                 refresh token it answered"
            )
        });
        assert_eq!(
            session.access_token, access_token,
            "case {label:?}: that session must hold the access token the caller was handed, \
             got {:?}",
            session.access_token
        );
        assert!(
            session.code_digest.is_some(),
            "case {label:?}: an exchanged session must record the code it was exchanged \
             from, got {session:?}"
        );
        assert_eq!(
            emulator.session_count(),
            expected_sessions,
            "case {label:?}: an exchange must start exactly one session, got {} for \
             {expected_sessions} exchanges",
            emulator.session_count()
        );

        // Rule 2, the other half: no OTHER name the call put within reach resolves it.
        for (what, token) in [
            ("the access token it answered", access_token.as_str()),
            ("the access code", case.code.as_str()),
            ("the redirect URI", case.redirect_uri),
            ("the clientId", case.client_id.unwrap_or_default()),
        ] {
            let other = key(token, case.client_id);
            if other == named {
                continue;
            }
            assert!(
                emulator.session(&other).is_none(),
                "case {label:?}: the session must resolve under the minted refresh token \
                 alone, but {what} names one too: {:?}",
                emulator.session(&other)
            );
        }
    }
}

/// Rule 3: a refresh answers no refresh token of its own, and the session it writes is
/// named by the token its caller presented — the same store, named the same way.
#[tokio::test]
async fn a_refresh_issues_no_token_and_names_its_session_by_the_one_presented() {
    let emulator = Emulator::new();
    let long_token = "p".repeat(400);

    // The rows differ along the presented token's size and alphabet and along the client
    // named, the parts a session's name is made of.
    let rows: [(&str, &str, Option<&str>); 5] = [
        ("a single-character token", "t", None),
        ("an opaque provider token", "figd_7Yh-21_qQ", Some("amplify-studio")),
        ("the same token for another client", "figd_7Yh-21_qQ", Some("")),
        ("a long token", long_token.as_str(), Some("loopback-client")),
        ("a token outside ASCII", "jeton-de-rafraîchissement-✓", None),
    ];

    let mut expected_sessions = 0usize;
    for (label, presented, client_id) in rows {
        let (status, body) = emulator.refresh(presented, client_id).await;
        assert_eq!(
            status, 200,
            "case {label:?}: a refresh must answer HTTP 200, got {status} with {body:?}"
        );
        assert!(
            body.get("refreshToken").is_none(),
            "case {label:?}: RefreshToken models no refresh token to hand out, so its \
             answer must carry none, got {body:?}"
        );

        expected_sessions += 1;
        let named = key(presented, client_id);
        let session = emulator.session(&named).unwrap_or_else(|| {
            panic!(
                "case {label:?}: the store must hold the session under the refresh token \
                 the caller presented"
            )
        });
        assert_eq!(
            session.access_token,
            member(&body, "accessToken"),
            "case {label:?}: that session must hold the access token the caller was handed, \
             got {:?}",
            session.access_token
        );
        assert!(
            session.code_digest.is_none(),
            "case {label:?}: a refresh carries no access code, so its session records \
             none, got {session:?}"
        );
        assert_eq!(
            emulator.session_count(),
            expected_sessions,
            "case {label:?}: a refresh of a token the store did not hold must write one \
             session, got {} for {expected_sessions} refreshes",
            emulator.session_count()
        );
        // The token the caller presented is the session's name, so the access token the
        // refresh answered names nothing.
        let by_access_token = key(&member(&body, "accessToken"), client_id);
        assert!(
            emulator.session(&by_access_token).is_none(),
            "case {label:?}: the access token must name no session, got {:?}",
            emulator.session(&by_access_token)
        );
    }
}
