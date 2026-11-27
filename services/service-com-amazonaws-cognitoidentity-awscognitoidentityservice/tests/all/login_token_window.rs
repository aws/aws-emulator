// SPDX-License-Identifier: CC0-1.0

//! The validity WINDOW of an OpenID Connect token this service minted, at the exchange
//! that spends it.
//!
//! RULE: a token this service mints is a time-bounded credential whose NOMINAL window is
//! the one the issuance recorded (`TokenDuration` seconds, or the documented 15 minutes for
//! a request naming none), and the window a PRESENTED token is read against is that instant
//! plus the clock-skew grace the model asks a caller to allow for: "Please provide for a
//! small grace period, usually no more than 5 minutes, to account for clock skew"
//! (`smithy_query(query="doc",shape="GetOpenIdTokenForDeveloperIdentityInput",member="TokenDuration")`).
//! So presenting a token to `GetCredentialsForIdentity` under this service's own provider
//! name buys a credential while that extended window is open — a token minted for the
//! modelled minimum of one second included, and a token presented by a caller whose clock
//! runs a little ahead of this service's with it — and once the clock has closed it the
//! exchange is refused with the modelled `NotAuthorizedException` and no credential. A
//! token this service cannot resolve at all buys nothing either. The refusal is answered
//! before any role is resolved and before any session is minted or handed back, so a
//! session the identity already holds does not outlive the token it was vended for.
//!
//! Every case asserts that rule over a TABLE of windows — the modelled minimum, the window
//! of a request that names none, and the modelled maximum among them — rather than one
//! recorded call, and each row is read at three instants: inside its nominal window, past
//! that window but inside the grace, and past the grace. Each of those readings is made with
//! an issuance of its own, of the row's window, so one row's reading cannot be what decides
//! the next: how many exchanges ONE issuance buys is `login_token_single_use`'s rule, and
//! this suite's rule is the window alone.
//!
//! Those instants are reached by AGEING the issuance each row is read against — the record
//! this service keeps of the token it minted, held in the store of this suite's own engine —
//! and never by waiting for wall-clock time to pass: a grace of minutes cannot be waited
//! out, and an outcome that depends on how fast the host runs is no assertion at all.
//! Holding that engine is why this suite stands its emulator up itself instead of borrowing
//! `identity_pool_flow`'s, which keeps its engine to itself; everything else — the role a
//! vend assumes, the request bodies — is the sibling suites' own. A sibling suite whose rule
//! is read against that same record drives it through THIS harness rather than a second copy
//! of it (`login_token_reuse_then_expiry`).

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_cognitoidentity_awscognitoidentityservice as cognito;
use smithy_server_runtime::MultiServiceRouter;

use crate::developer_identity_tokens::token_request;
use crate::identity_pool_flow::{
    cognito_trust_policy, error_code, seed_role, seeded_role_arn, DEFAULT_REGION,
};

/// The operation the window is enforced at.
const OPERATION: &str = "GetCredentialsForIdentity";

/// The operation that mints the token, and so records its window.
const MINT: &str = "GetOpenIdTokenForDeveloperIdentity";

/// The `Logins` provider name a token minted by THIS service is presented under: "If the
/// token is for cognito-identity.amazonaws.com, it will be passed through to Security Token
/// Service with the appropriate role for the token"
/// (`smithy_query(query="doc",shape="GetCredentialsForIdentityInput")`). Spelled as a
/// literal here so the wire-visible name is pinned by the test, not by the code under test.
const COGNITO_IDENTITY_PROVIDER: &str = "cognito-identity.amazonaws.com";

/// The message the refusal of a token whose window has closed carries
/// (`crate::state::EXPIRED_LOGIN_TOKEN_MESSAGE`) — pinned here as a literal for the same
/// reason. Fixed text: it carries no part taken from the request or from stored state.
const EXPIRED_MESSAGE: &str = "Invalid login token. Token is expired.";

/// The message the refusal of a value of this service's OWN token form that it cannot
/// resolve carries (`crate::state::UNVERIFIABLE_LOGIN_TOKEN_MESSAGE`).
const UNVERIFIABLE_MESSAGE: &str = "Invalid login token. Couldn't verify signed token.";

/// The message the refusal of a token this service cannot resolve at all carries, when the
/// value does not even claim this service as its issuer
/// (`crate::state::NOT_A_COGNITO_TOKEN_MESSAGE`).
const NOT_A_COGNITO_TOKEN_MESSAGE: &str = "Invalid login token. Not a Cognito token.";

/// The modelled bounds of `TokenDuration` (`long range 1..86400`,
/// `smithy_query(query="operation",operation="GetOpenIdTokenForDeveloperIdentity")`): the
/// shortest and the longest window a mint can ask for.
const MIN_TOKEN_SECONDS: i64 = 1;
const MAX_TOKEN_SECONDS: i64 = 86_400;

/// The window of a mint that names no `TokenDuration`: "If you don't provide an expiration
/// time, the token is valid for 15 minutes." (that member's own documentation).
const DEFAULT_TOKEN_SECONDS: i64 = 900;

/// How far past the instant its issuance recorded a presented token is still read as valid:
/// the longest grace period the `TokenDuration` documentation asks a caller to allow for,
/// "usually no more than 5 minutes, to account for clock skew". Spelled as a literal so the
/// behaviour the wire shows is pinned by the test rather than read out of the code under
/// test.
const CLOCK_SKEW_GRACE: Duration = Duration::from_secs(300);

/// One emulator holding this service, reached exactly as the production router reaches it,
/// beside the engine it is mounted on: this suite reads and AGES the issuance records that
/// engine holds, which is how it drives the clock the window is read against. `pub(crate)`
/// so a sibling suite whose rule is read against that same record drives the service through
/// this harness rather than a second copy of it.
pub(crate) struct Emulator {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

impl Emulator {
    pub(crate) fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.register_service(cognito::EmulatedService::new(Arc::clone(&engine)));
        Self { router, engine }
    }

    /// Call one operation and answer its HTTP status with the parsed JSON body (`Null` for
    /// an empty body), as a client calls it: `POST /` with `X-Amz-Target` on `awsJson1_1`.
    async fn call(&self, operation: &str, body: &Value) -> (u16, Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri(format!(
                "https://cognito-identity.{DEFAULT_REGION}.amazonaws.com/"
            ))
            .header("content-type", "application/x-amz-json-1.1")
            .header(
                "x-amz-target",
                format!("AWSCognitoIdentityService.{operation}"),
            )
            // The SigV4 credential scope is what routes the request to this service and
            // names the region its ids are minted in. Signatures are not verified unless
            // the emulator is started with authentication on.
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 \
                     Credential=EMULATORTESTKEY12345/20260101/{DEFAULT_REGION}/cognito-identity/aws4_request, \
                     SignedHeaders=host;x-amz-target, Signature=0"
                ),
            )
            .body(bytes::Bytes::from(
                serde_json::to_vec(body).expect("the request body serializes"),
            ))
            .expect("the request builds");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let bytes = response.into_body();
        let parsed = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or_else(|error| {
                panic!(
                    "operation {operation} answered a body that is not JSON ({error}): {}",
                    String::from_utf8_lossy(&bytes)
                )
            })
        };
        (status, parsed)
    }

    /// A call that must succeed, answering its output object.
    async fn ok(&self, operation: &str, body: &Value) -> Value {
        let (status, parsed) = self.call(operation, body).await;
        assert_eq!(
            status, 200,
            "{operation} on {body} expected HTTP 200, got {status} with {parsed}"
        );
        parsed
    }

    /// A pool that names a developer provider AND can vend credentials: its `authenticated`
    /// role is seeded into the IAM store of the same engine and configured on the pool,
    /// which is the shape a pool has to be in for an exchange to be served at all. Answers
    /// its `IdentityPoolId`.
    pub(crate) async fn vending_pool(&self, name: &str, developer_provider: &str) -> String {
        let created = self
            .ok(
                "CreateIdentityPool",
                &json!({
                    "IdentityPoolName": name,
                    "AllowUnauthenticatedIdentities": false,
                    "DeveloperProviderName": developer_provider,
                }),
            )
            .await;
        let pool_id = created["IdentityPoolId"]
            .as_str()
            .unwrap_or_else(|| panic!("CreateIdentityPool answers an IdentityPoolId: {created}"))
            .to_string();
        let authenticated = seed_role(
            &self.engine,
            &seeded_role_arn(name, "auth"),
            cognito_trust_policy(&pool_id),
        );
        self.ok(
            "SetIdentityPoolRoles",
            &json!({
                "IdentityPoolId": &pool_id,
                "Roles": {"authenticated": authenticated},
            }),
        )
        .await;
        pool_id
    }

    /// Mint a token for `user` in `pool_id` with the window `token_duration` asks for (none
    /// when it is `None`), and answer the identity it resolved to with the token itself.
    pub(crate) async fn mint(
        &self,
        pool_id: &str,
        developer_provider: &str,
        user: &str,
        token_duration: Option<i64>,
    ) -> (String, String) {
        let answered = self
            .ok(
                MINT,
                &token_request(
                    pool_id,
                    &[(developer_provider, user)],
                    None,
                    token_duration,
                ),
            )
            .await;
        let identity_id = answered["IdentityId"]
            .as_str()
            .unwrap_or_else(|| panic!("{MINT} answers an IdentityId: {answered}"))
            .to_string();
        let token = answered["Token"]
            .as_str()
            .unwrap_or_else(|| panic!("{MINT} answers a Token: {answered}"))
            .to_string();
        (identity_id, token)
    }

    /// The exchange itself: `GetCredentialsForIdentity` presenting `token` under this
    /// service's own provider name, as a client spends a minted token.
    pub(crate) async fn exchange(&self, identity_id: &str, token: &str) -> (u16, Value) {
        self.call(
            OPERATION,
            &json!({
                "IdentityId": identity_id,
                "Logins": {COGNITO_IDENTITY_PROVIDER: token},
            }),
        )
        .await
    }

    /// Move the window the recorded issuance of `token` carries BACK by `elapsed`: the
    /// record is left as it would be had it been minted that much earlier, which is the
    /// same thing as reading it that much later and needs no wall clock to pass.
    pub(crate) fn age_issuance(&self, case: &str, token: &str, elapsed: Duration) {
        let state = cognito::state::service_state(&*self.engine)
            .expect("this service's store is on this engine");
        let mut store = state.lock();
        let issued = store
            .open_id_tokens
            .get_mut(token)
            .unwrap_or_else(|| panic!("case {case:?}: the issuance of a minted token is recorded"));
        issued.issued_at = issued
            .issued_at
            .checked_sub(elapsed)
            .unwrap_or_else(|| panic!("case {case:?}: the aged issuance instant is representable"));
        issued.expiration = issued
            .expiration
            .checked_sub(elapsed)
            .unwrap_or_else(|| panic!("case {case:?}: the aged expiration is representable"));
    }

    /// Record an issuance whose window is the one given, minted the way this service mints
    /// one, and answer the token itself — a token already past its own window at the instant
    /// it is recorded, which no mint over the wire can produce (`TokenDuration` is at least
    /// one second).
    fn record_issuance(
        &self,
        pool_id: &str,
        identity_id: &str,
        authenticated_by: &[String],
        issued_at: SystemTime,
        expiration: SystemTime,
    ) -> String {
        let token = cognito::state::open_id_token(
            pool_id,
            identity_id,
            authenticated_by,
            issued_at,
            expiration,
        )
        .expect("this service signs the token it mints");
        let state = cognito::state::service_state(&*self.engine)
            .expect("this service's store is on this engine");
        state.lock().open_id_tokens.insert(
            token.clone(),
            cognito::state::IssuedOpenIdToken {
                identity_id: identity_id.to_string(),
                identity_pool_id: pool_id.to_string(),
                issued_at,
                expiration,
                authenticated_by: authenticated_by.to_vec(),
                principal_tags: None,
            },
        );
        token
    }

    /// Whether the store still holds an issuance for `token`.
    fn holds_issuance(&self, token: &str) -> bool {
        let state = cognito::state::service_state(&*self.engine)
            .expect("this service's store is on this engine");
        let store = state.lock();
        store.open_id_tokens.contains_key(token)
    }
}

/// The `message` an error response carries.
fn error_message(case: &str, body: &Value) -> String {
    body["message"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: an error response carries a message: {body}"))
        .to_string()
}

/// Assert that `body` is a served exchange: a complete credential set for `identity_id`.
pub(crate) fn assert_served_credentials(case: &str, identity_id: &str, body: &Value) {
    assert_eq!(
        body["IdentityId"].as_str(),
        Some(identity_id),
        "case {case:?}: a served exchange answers the identity it was made for, got {body}"
    );
    for member in ["AccessKeyId", "SecretKey", "SessionToken", "Expiration"] {
        assert!(
            !body["Credentials"][member].is_null(),
            "case {case:?}: a served exchange carries Credentials.{member}, got {body}"
        );
    }
}

/// Assert that `body` is the refusal `message` words, with no credential in it.
pub(crate) fn assert_refused(case: &str, status: u16, body: &Value, message: &str) {
    assert_eq!(
        status, 400,
        "case {case:?}: expected the HTTP 400 of a refused exchange, got {status} with {body}"
    );
    let code = error_code(body);
    assert!(
        code.ends_with("NotAuthorizedException"),
        "case {case:?}: expected NotAuthorizedException, got {code:?} in {body}"
    );
    let actual = error_message(case, body);
    assert_eq!(
        actual, message,
        "case {case:?}: expected the refusal message {message:?}, got {actual:?}"
    );
    assert!(
        body.get("Credentials").is_none(),
        "case {case:?}: a refused exchange carries no credential, got {body}"
    );
}

/// The seconds between the `iat` and the `exp` of a token this service minted, read from
/// the token's own claims: the window the ISSUANCE recorded, which is what the mint answers
/// for.
fn claimed_window_seconds(case: &str, token: &str) -> i64 {
    let read = ::service_common::jwt::read_unsecured(token)
        .unwrap_or_else(|error| panic!("case {case:?}: a minted token is a compact JWS: {error:?}"));
    let issued_at = read
        .numeric_date("iat")
        .unwrap_or_else(|| panic!("case {case:?}: a minted token carries an iat claim"));
    let expiration = read
        .numeric_date("exp")
        .unwrap_or_else(|| panic!("case {case:?}: a minted token carries an exp claim"));
    expiration - issued_at
}

/// RULE: the window a mint records is the NOMINAL window, and the window the exchange reads
/// a presented token against is that instant plus the documented clock-skew grace — so a
/// token is spent inside its nominal window, still spent for the grace past it, and refused
/// only once the grace has closed too.
#[tokio::test]
async fn a_minted_token_is_spent_until_the_clock_skew_grace_past_its_window_has_closed() {
    let emulator = Emulator::new();
    let developer_provider = "login.example.com";
    let pool_id = emulator
        .vending_pool("token-window", developer_provider)
        .await;
    // One developer user identifier for every row, so re-minting also has to keep answering
    // the SAME identity.
    let user = "backend-user-1";

    // The rows differ along the window the mint asks for: each modelled bound, the window of
    // a request that names none, and windows in between. The second field is what the row
    // sends as `TokenDuration`; the third is the nominal window it is owed in seconds.
    let rows: Vec<(&str, Option<i64>, i64)> = vec![
        (
            "the modelled minimum window of one second",
            Some(MIN_TOKEN_SECONDS),
            MIN_TOKEN_SECONDS,
        ),
        ("a window of two seconds", Some(2), 2),
        (
            "no TokenDuration at all: the documented 15-minute default",
            None,
            DEFAULT_TOKEN_SECONDS,
        ),
        ("a window of one hour", Some(3_600), 3_600),
        (
            "the modelled maximum window of 86400 seconds",
            Some(MAX_TOKEN_SECONDS),
            MAX_TOKEN_SECONDS,
        ),
    ];

    let mut identity: Option<String> = None;
    for (case, token_duration, nominal) in rows {
        // One issuance per phase of the row, all of the same window, so what one phase read
        // its record at cannot be what the next phase reads: each phase ages an issuance of
        // its own.
        let mut minted = Vec::new();
        while minted.len() < 3 {
            let (identity_id, token) = emulator
                .mint(&pool_id, developer_provider, user, token_duration)
                .await;
            match &identity {
                Some(first) => assert_eq!(
                    &identity_id, first,
                    "case {case:?}: re-minting for one developer user identifier answers the \
                     same identity, got {identity_id} after {first}"
                ),
                None => identity = Some(identity_id.clone()),
            }
            minted.push((identity_id, token));
        }
        let (identity_id, token) = minted[0].clone();
        let (_, inside_the_grace) = minted[1].clone();
        let (_, past_the_grace) = minted[2].clone();

        // The mint honours the duration its caller asked for EXACTLY: the grace is a
        // tolerance the reading adds, never a window the issuance hands out.
        let claimed = claimed_window_seconds(case, &token);
        assert_eq!(
            claimed, nominal,
            "case {case:?}: the minted token's own window must be {nominal} seconds, got \
             {claimed}"
        );

        // Inside its nominal window, whatever window was asked for, the token is spent —
        // the one-second minimum included, which is the shortest window the model admits.
        let (status, body) = emulator.exchange(&identity_id, &token).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a token inside its window is exchanged, got {status} with {body}"
        );
        assert_served_credentials(case, &identity_id, &body);

        // One second PAST the nominal window, and so inside the grace: still spendable.
        let nominal_window = Duration::from_secs(u64::try_from(nominal).unwrap_or(0));
        emulator.age_issuance(
            case,
            &inside_the_grace,
            nominal_window + Duration::from_secs(1),
        );
        let (status, body) = emulator.exchange(&identity_id, &inside_the_grace).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a token inside the clock-skew grace past its window is still \
             exchanged, got {status} with {body}"
        );
        assert_served_credentials(case, &identity_id, &body);

        // A further grace on top of that puts a token of the same window past the grace as
        // well, and nothing but the window the record is read against differs between the
        // two calls.
        emulator.age_issuance(
            case,
            &past_the_grace,
            nominal_window + Duration::from_secs(1) + CLOCK_SKEW_GRACE,
        );
        let (status, body) = emulator.exchange(&identity_id, &past_the_grace).await;
        assert_refused(case, status, &body, EXPIRED_MESSAGE);
    }
}

/// RULE, the same window from the other side: the answer does not depend on whether the
/// issuance store still HOLDS the record — a token past its window and the grace is refused
/// as the expired token it is even after a sweep has forgotten that record, and a token the
/// grace still covers is neither swept nor refused.
#[tokio::test]
async fn the_sweep_gives_up_what_the_grace_no_longer_covers_and_nothing_else() {
    let emulator = Emulator::new();
    let developer_provider = "login.example.com";
    let pool_id = emulator
        .vending_pool("token-sweep", developer_provider)
        .await;
    let user = "backend-user-2";
    let (identity_id, live) = emulator
        .mint(&pool_id, developer_provider, user, Some(MAX_TOKEN_SECONDS))
        .await;
    let (status, body) = emulator.exchange(&identity_id, &live).await;
    assert_eq!(
        status, 200,
        "a token inside its window is exchanged, got {status} with {body}"
    );

    // What the mint records for a developer-authenticated issuance: the provider the login
    // was made under, and the user that provider authenticated.
    let authenticated_by = vec![
        developer_provider.to_string(),
        format!("{developer_provider}:{pool_id}:{user}"),
    ];
    let now = SystemTime::now();
    // Two issuances the clock has already taken past their nominal window, differing in
    // whether the grace still covers them: one a second past it, one a minute past the whole
    // grace.
    let inside_the_grace = emulator.record_issuance(
        &pool_id,
        &identity_id,
        &authenticated_by,
        now - Duration::from_secs(901),
        now - Duration::from_secs(1),
    );
    // A second issuance of that same window, for the reading made AFTER the sweep, so the two
    // readings are made with two tokens the grace covers alike and neither reading is the
    // other's.
    let inside_the_grace_again = emulator.record_issuance(
        &pool_id,
        &identity_id,
        &authenticated_by,
        now - Duration::from_secs(901),
        now - Duration::from_secs(1),
    );
    let past_the_grace = emulator.record_issuance(
        &pool_id,
        &identity_id,
        &authenticated_by,
        now - Duration::from_secs(900) - CLOCK_SKEW_GRACE - Duration::from_secs(60),
        now - CLOCK_SKEW_GRACE - Duration::from_secs(60),
    );

    let (status, body) = emulator.exchange(&identity_id, &inside_the_grace).await;
    assert_eq!(
        status, 200,
        "a token the grace still covers is exchanged, got {status} with {body}"
    );
    assert_served_credentials("inside the grace", &identity_id, &body);
    let (status, body) = emulator.exchange(&identity_id, &past_the_grace).await;
    assert_refused("past the grace", status, &body, EXPIRED_MESSAGE);

    // A mint is what sweeps the issuances the clock has run out on, so this is the call that
    // decides which records are given up.
    let (_, fresh) = emulator
        .mint(&pool_id, developer_provider, user, Some(MAX_TOKEN_SECONDS))
        .await;
    assert!(
        emulator.holds_issuance(&inside_the_grace),
        "the sweep must keep a record the grace still covers"
    );
    assert!(
        !emulator.holds_issuance(&past_the_grace),
        "the sweep must give up a record past its window and the grace"
    );

    // And the answers are the same ones, the swept record included: what decides the
    // exchange is the window, not whether the store still holds the issuance.
    let (status, body) = emulator
        .exchange(&identity_id, &inside_the_grace_again)
        .await;
    assert_eq!(
        status, 200,
        "a token the grace still covers is exchanged after the sweep too, got {status} with \
         {body}"
    );
    let (status, body) = emulator.exchange(&identity_id, &past_the_grace).await;
    assert_refused("the swept token", status, &body, EXPIRED_MESSAGE);

    // The window of the token minted afterwards is its own, so the sweep costs it nothing.
    let (status, body) = emulator.exchange(&identity_id, &fresh).await;
    assert_eq!(
        status, 200,
        "the freshly minted token is inside its own window, got {status} with {body}"
    );
    assert_served_credentials("the freshly minted token", &identity_id, &body);
}

/// A compact JWS carrying `claims`, with the empty signature segment RFC 7515 §6 defines
/// for an unsecured one — the form this service's tokens take, so a row below can present a
/// value of that form that no mint ever produced.
fn unsecured_jws(claims: &Value) -> String {
    let segment = |bytes: &[u8]| {
        aws_smithy_types::base64::encode(bytes)
            .replace('+', "-")
            .replace('/', "_")
            .trim_end_matches('=')
            .to_string()
    };
    format!(
        "{}.{}.",
        segment(json!({"alg": "none", "typ": "JWT"}).to_string().as_bytes()),
        segment(claims.to_string().as_bytes())
    )
}

/// RULE, the resolution half: a value presented under this service's own provider name has
/// to be a token this service issued, so one it never issued vouches for nothing and buys
/// no credential — and WHICH refusal it carries follows from what the value itself claims to
/// be: a value of this service's own token form is the unverifiable token it claims to be,
/// while a value that claims this service as its issuer in no way is no Cognito token at
/// all.
#[tokio::test]
async fn a_token_this_service_never_issued_buys_no_credential() {
    let emulator = Emulator::new();
    let developer_provider = "login.example.com";
    let pool_id = emulator
        .vending_pool("token-unknown", developer_provider)
        .await;
    // The identity exists and holds a credential from a token that WAS issued, so each row
    // below is refused on the token it presents and not on anything else.
    let (identity_id, issued) = emulator
        .mint(
            &pool_id,
            developer_provider,
            "backend-user-3",
            Some(MAX_TOKEN_SECONDS),
        )
        .await;
    let (status, body) = emulator.exchange(&identity_id, &issued).await;
    assert_eq!(
        status, 200,
        "the issued token is exchanged, got {status} with {body}"
    );
    // A second issuance of the same identity, for the reading made after the rows, so that
    // reading is its own.
    let (_, issued_again) = emulator
        .mint(
            &pool_id,
            developer_provider,
            "backend-user-3",
            Some(MAX_TOKEN_SECONDS),
        )
        .await;

    // An instant far ahead of any run of this suite, so no row below is refused for its
    // window rather than for being unknown.
    let far_future = 4_102_444_800u64;
    // The rows differ along the FORM of the value presented: none of them is a token this
    // service minted, and the third field is the sentence that form is owed.
    let rows: Vec<(&str, String, &str)> = vec![
        (
            "a value that is no token at all",
            "not-a-token".to_string(),
            NOT_A_COGNITO_TOKEN_MESSAGE,
        ),
        (
            "a JWS whose payload is not a claim set",
            unsecured_jws(&json!("a string, not an object")),
            NOT_A_COGNITO_TOKEN_MESSAGE,
        ),
        (
            "a JWS naming another issuer",
            unsecured_jws(&json!({
                "iss": "https://accounts.google.com",
                "aud": &pool_id,
                "sub": &identity_id,
                "exp": far_future,
            })),
            NOT_A_COGNITO_TOKEN_MESSAGE,
        ),
        (
            "a JWS of this service's own form that no mint produced",
            unsecured_jws(&json!({
                "iss": "https://cognito-identity.amazonaws.com",
                "aud": &pool_id,
                "sub": &identity_id,
                "exp": far_future,
            })),
            UNVERIFIABLE_MESSAGE,
        ),
    ];

    for (case, token, owed) in rows {
        let (status, body) = emulator.exchange(&identity_id, &token).await;
        assert_refused(case, status, &body, owed);
    }

    // The token that WAS issued is untouched by any of it: the refusals decided on the
    // value presented, nothing else.
    let (status, body) = emulator.exchange(&identity_id, &issued_again).await;
    assert_eq!(
        status, 200,
        "the issued token is still exchanged, got {status} with {body}"
    );
    assert_served_credentials("the issued token", &identity_id, &body);
}
