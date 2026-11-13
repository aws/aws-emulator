// SPDX-License-Identifier: CC0-1.0

//! The `AWS/Cognito` CloudWatch series a user pool's own traffic publishes, asserted where a
//! client reads it: through CloudWatch's own `GetMetricStatistics` and `ListMetrics`, with
//! both services mounted on ONE engine exactly as the assembled emulator mounts them.
//!
//! The rule under test: every call that TERMINATES an authentication or registration attempt
//! against an app client publishes exactly one datapoint into the `AWS/Cognito` series its
//! attempt belongs to — `SignInSuccesses` for a sign-in, `TokenRefreshSuccesses` for a
//! refresh-token exchange, `SignUpSuccesses` for a registration — keyed on the dimension
//! pair `UserPool` + `UserPoolClient`, valued 1 where the attempt completed and 0 where it
//! was refused, so `Sum` counts completions while `SampleCount` counts attempts. A call that
//! answers with a CHALLENGE terminates nothing and publishes nothing yet. Where the pool
//! evaluates the events it serves (threat protection switched on), a completed attempt also
//! publishes the risk outcome `NoRisk`, keyed on `Operation` + `UserPoolId` — a different
//! dimension name for the pool, and CloudWatch dimension matching is exact.
//!
//! The rows span what the rule generalises over rather than one recorded request: WHICH kind
//! of attempt (sign-in, refresh, registration), WHICH operation terminated it (the
//! self-service and the administrative surface, and the refresh-token exchange operation),
//! HOW the attempt ended (completed, refused on the credential, refused on a value the pool
//! does not hold, refused by request validation before any credential is checked — including
//! the app client's own flow gate, before and after the client is reconfigured), HOW MANY
//! attempts were made (none, one, several), and whether the pool evaluates risk.

use bytes::Bytes;
use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use service_com_amazonaws_cloudwatch_graniteserviceversion20100801 as cloudwatch;
use service_com_amazonaws_cognitoidentityprovider_awscognitoidentityproviderservice as cognito_idp;

/// The `X-Amz-Target` prefix of this service, which is its Smithy shape's local name.
const COGNITO_TARGET_PREFIX: &str = "AWSCognitoIdentityProviderService";

/// The `X-Amz-Target` prefix CloudWatch serves its `awsJson1_0` wire under (that crate's
/// OPERATIONS.md "Wire protocol").
const CLOUDWATCH_TARGET_PREFIX: &str = "GraniteServiceVersion20100801";

/// The namespace a user pool's own metrics live in.
const NAMESPACE: &str = "AWS/Cognito";

/// The metric names this suite reads.
const SIGN_IN_SUCCESSES: &str = "SignInSuccesses";
const TOKEN_REFRESH_SUCCESSES: &str = "TokenRefreshSuccesses";
const SIGN_UP_SUCCESSES: &str = "SignUpSuccesses";
const NO_RISK: &str = "NoRisk";

/// The dimension names an `AWS/Cognito` series is keyed on: the counters carry the pool as
/// `UserPool` beside the app client, and a risk outcome carries it as `UserPoolId` beside the
/// operation it was evaluated for.
const USER_POOL: &str = "UserPool";
const USER_POOL_CLIENT: &str = "UserPoolClient";
const OPERATION: &str = "Operation";
const USER_POOL_ID: &str = "UserPoolId";

/// A password satisfying a default pool's password policy, and one no user holds.
const GOOD_PASSWORD: &str = "Passw0rd!2024";
const WRONG_PASSWORD: &str = "N0tTheP4ssword!";

/// A second password satisfying the same policy, which an administrator sets as temporary —
/// so the user it belongs to owes a password change before it can sign in.
const TEMPORARY_PASSWORD: &str = "Temp0rary!2024";

/// The user every sign-in row signs in, and a name no pool holds.
const USERNAME: &str = "metrics-user";
const UNKNOWN_USERNAME: &str = "nobody-at-all";

/// The period the datapoints are aggregated into, and how far either side of the calls the
/// read window reaches, so the window holds every datapoint whatever minute the calls land
/// in.
const PERIOD_SECS: i64 = 3600;
const WINDOW_SLACK_SECS: f64 = 1_200.0;

/// One emulator with this service and CloudWatch mounted on ONE engine, which is what makes
/// the metric store the pool writes the same one CloudWatch reads.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.register_service(cognito_idp::EmulatedService::new(Arc::clone(&engine)));
    router.register_service(cloudwatch::EmulatedService::new(engine));
    router.set_request_authentication(false);
    router
}

/// Wall-clock now as epoch seconds, the clock a metric window is measured against.
fn now_secs() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0.0, |since| since.as_secs_f64())
}

/// Dispatch one `awsJson` call and return its status and JSON body.
async fn json_rpc(
    router: &MultiServiceRouter,
    host: &str,
    signing_name: &str,
    content_type: &str,
    target: &str,
    body: &Value,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", host)
        .header("content-type", content_type)
        .header("x-amz-target", target)
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 \
                 Credential=AKIDEXAMPLE/20260101/us-east-1/{signing_name}/aws4_request, \
                 SignedHeaders=host, Signature=0000"
            ),
        )
        .body(Bytes::from(body.to_string()))
        .expect("the request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, payload)
}

/// One cognito-idp call, as an SDK drives it.
async fn cognito(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    json_rpc(
        router,
        "cognito-idp.us-east-1.amazonaws.com",
        "cognito-idp",
        "application/x-amz-json-1.1",
        &format!("{COGNITO_TARGET_PREFIX}.{operation}"),
        body,
    )
    .await
}

/// The same call, requiring success.
async fn cognito_ok(router: &MultiServiceRouter, operation: &str, body: &Value) -> Value {
    let (status, payload) = cognito(router, operation, body).await;
    assert_eq!(
        status, 200,
        "{operation} must succeed, got {status} with {payload}"
    );
    payload
}

/// One CloudWatch call, requiring success.
async fn cloudwatch_ok(router: &MultiServiceRouter, operation: &str, body: &Value) -> Value {
    let (status, payload) = json_rpc(
        router,
        "monitoring.us-east-1.amazonaws.com",
        "monitoring",
        "application/x-amz-json-1.0",
        &format!("{CLOUDWATCH_TARGET_PREFIX}.{operation}"),
        body,
    )
    .await;
    assert_eq!(
        status, 200,
        "{operation} must succeed, got {status} with {payload}"
    );
    payload
}

/// What one series holds over the window around the calls: the `Sum` and the `SampleCount`
/// of every datapoint in it, or `None` where the read answers no datapoint at all — the
/// reading a client that polls such a series gets while nothing has published into it.
async fn observed(
    router: &MultiServiceRouter,
    metric: &str,
    dimensions: &[(&str, &str)],
    opened: f64,
) -> Option<(f64, f64)> {
    let dimensions: Vec<Value> = dimensions
        .iter()
        .map(|(name, value)| json!({ "Name": name, "Value": value }))
        .collect();
    let answered = cloudwatch_ok(
        router,
        "GetMetricStatistics",
        &json!({
            "Namespace": NAMESPACE,
            "MetricName": metric,
            "Dimensions": dimensions,
            "StartTime": opened - WINDOW_SLACK_SECS,
            "EndTime": now_secs() + WINDOW_SLACK_SECS,
            "Period": PERIOD_SECS,
            "Statistics": ["Sum", "SampleCount"],
        }),
    )
    .await;
    let points = answered
        .get("Datapoints")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if points.is_empty() {
        return None;
    }
    let total = |member: &str| -> f64 {
        points
            .iter()
            .filter_map(|point| point.get(member).and_then(Value::as_f64))
            .sum()
    };
    Some((total("Sum"), total("SampleCount")))
}

/// Every `(MetricName, dimension names)` pair `ListMetrics` catalogues under the filter
/// `dimension`, as a client discovering the namespace reads it.
async fn catalogued(
    router: &MultiServiceRouter,
    dimension: (&str, &str),
) -> Vec<(String, Vec<String>)> {
    let answered = cloudwatch_ok(
        router,
        "ListMetrics",
        &json!({
            "Namespace": NAMESPACE,
            "Dimensions": [{ "Name": dimension.0, "Value": dimension.1 }],
        }),
    )
    .await;
    answered
        .get("Metrics")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|metric| {
            let name = metric
                .get("MetricName")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let mut names: Vec<String> = metric
                .get("Dimensions")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default()
                .iter()
                .filter_map(|entry| entry.get("Name").and_then(Value::as_str))
                .map(str::to_string)
                .collect();
            names.sort();
            (name, names)
        })
        .collect()
}

/// A created pool, one app client that may be signed in to, and a second app client that
/// nothing is ever attempted against.
struct Fixture {
    user_pool_id: String,
    client_id: String,
    unused_client_id: String,
}

/// The `ExplicitAuthFlows` the fixture's app client admits: the two client-side flows and the
/// administrative password flow, so one client serves every row.
const FLOWS: [&str; 3] = [
    "ALLOW_USER_PASSWORD_AUTH",
    "ALLOW_REFRESH_TOKEN_AUTH",
    "ALLOW_ADMIN_USER_PASSWORD_AUTH",
];

/// Create a pool with `add_ons` (its `UserPoolAddOns`, when it has any), two app clients and
/// one `CONFIRMED` user.
async fn given_pool(
    router: &MultiServiceRouter,
    tier: Option<&str>,
    mode: Option<&str>,
) -> Fixture {
    let mut pool = Map::new();
    pool.insert("PoolName".to_string(), json!("metrics-pool"));
    if let Some(tier) = tier {
        pool.insert("UserPoolTier".to_string(), json!(tier));
    }
    if let Some(mode) = mode {
        pool.insert(
            "UserPoolAddOns".to_string(),
            json!({ "AdvancedSecurityMode": mode }),
        );
    }
    let created = cognito_ok(router, "CreateUserPool", &Value::Object(pool)).await;
    let user_pool_id = created["UserPool"]["Id"]
        .as_str()
        .expect("a minted pool id")
        .to_string();
    let client_id = create_client(router, &user_pool_id, "used", &FLOWS).await;
    let unused_client_id = create_client(router, &user_pool_id, "never-used", &FLOWS).await;
    cognito_ok(
        router,
        "AdminCreateUser",
        &json!({ "UserPoolId": user_pool_id, "Username": USERNAME, "MessageAction": "SUPPRESS" }),
    )
    .await;
    cognito_ok(
        router,
        "AdminSetUserPassword",
        &json!({
            "UserPoolId": user_pool_id,
            "Username": USERNAME,
            "Password": GOOD_PASSWORD,
            "Permanent": true,
        }),
    )
    .await;
    Fixture {
        user_pool_id,
        client_id,
        unused_client_id,
    }
}

/// Create an app client named `client_name` in `user_pool_id`, admitting exactly `flows`, and
/// return its minted id.
async fn create_client(
    router: &MultiServiceRouter,
    user_pool_id: &str,
    client_name: &str,
    flows: &[&str],
) -> String {
    let created = cognito_ok(
        router,
        "CreateUserPoolClient",
        &json!({
            "UserPoolId": user_pool_id,
            "ClientName": client_name,
            "GenerateSecret": false,
            "ExplicitAuthFlows": flows,
        }),
    )
    .await;
    created["UserPoolClient"]["ClientId"]
        .as_str()
        .expect("a minted client id")
        .to_string()
}

/// One attempt a row drives against the fixture's app client.
#[derive(Clone, Copy, Debug)]
enum Step {
    /// `InitiateAuth` `USER_PASSWORD_AUTH` with the stored password: a sign-in that
    /// completes.
    SignIn,
    /// The same with a password the user does not hold: a sign-in refused on the credential.
    WrongPassword,
    /// The same for a username the pool does not hold.
    UnknownUser,
    /// `InitiateAuth` with an `AuthFlow` that is not valid for it: a refusal raised by
    /// request validation, before any credential is checked.
    InvalidFlow,
    /// `AdminInitiateAuth` `ADMIN_USER_PASSWORD_AUTH`, with the stored password and with a
    /// wrong one: the administrative surface of the same attempt.
    AdminSignIn,
    AdminWrongPassword,
    /// `InitiateAuth` with the named refresh-token `AuthFlow`, presenting the token the last
    /// completed sign-in issued.
    Refresh(&'static str),
    /// The same, presenting a value nothing issued.
    RefreshUnknownToken,
    /// `GetTokensFromRefreshToken` with the token the last completed sign-in issued.
    ExchangeRefreshToken,
    /// `SignUp` of a user nothing registered yet, and of the username the last `SignUp`
    /// registered.
    SignUp,
    DuplicateSignUp,
}

/// Drive `steps` against `fixture`, asserting each attempt was answered the way its own step
/// says — so a row can only pass while the traffic it claims to have driven was really
/// served or really refused.
async fn drive(router: &MultiServiceRouter, fixture: &Fixture, case: &str, steps: &[Step]) {
    let mut refresh_token: Option<String> = None;
    let mut registered: Option<String> = None;
    let mut minted = 0_usize;
    for step in steps {
        match step {
            Step::SignIn => {
                let answered = cognito_ok(
                    router,
                    "InitiateAuth",
                    &initiate_auth(
                        &fixture.client_id,
                        "USER_PASSWORD_AUTH",
                        json!({ "USERNAME": USERNAME, "PASSWORD": GOOD_PASSWORD }),
                    ),
                )
                .await;
                refresh_token = answered["AuthenticationResult"]["RefreshToken"]
                    .as_str()
                    .map(str::to_string);
                assert!(
                    refresh_token.is_some(),
                    "case {case:?}: a completed sign-in must issue a refresh token, got \
                     {answered}"
                );
            }
            Step::WrongPassword => {
                refused(
                    router,
                    case,
                    "InitiateAuth",
                    &initiate_auth(
                        &fixture.client_id,
                        "USER_PASSWORD_AUTH",
                        json!({ "USERNAME": USERNAME, "PASSWORD": WRONG_PASSWORD }),
                    ),
                )
                .await;
            }
            Step::UnknownUser => {
                refused(
                    router,
                    case,
                    "InitiateAuth",
                    &initiate_auth(
                        &fixture.client_id,
                        "USER_PASSWORD_AUTH",
                        json!({ "USERNAME": UNKNOWN_USERNAME, "PASSWORD": GOOD_PASSWORD }),
                    ),
                )
                .await;
            }
            Step::InvalidFlow => {
                refused(
                    router,
                    case,
                    "InitiateAuth",
                    &initiate_auth(
                        &fixture.client_id,
                        "ADMIN_USER_PASSWORD_AUTH",
                        json!({ "USERNAME": USERNAME, "PASSWORD": GOOD_PASSWORD }),
                    ),
                )
                .await;
            }
            Step::AdminSignIn => {
                cognito_ok(
                    router,
                    "AdminInitiateAuth",
                    &admin_initiate_auth(fixture, GOOD_PASSWORD),
                )
                .await;
            }
            Step::AdminWrongPassword => {
                refused(
                    router,
                    case,
                    "AdminInitiateAuth",
                    &admin_initiate_auth(fixture, WRONG_PASSWORD),
                )
                .await;
            }
            Step::Refresh(auth_flow) => {
                let token = refresh_token
                    .clone()
                    .unwrap_or_else(|| panic!("case {case:?}: no sign-in issued a token yet"));
                cognito_ok(
                    router,
                    "InitiateAuth",
                    &initiate_auth(
                        &fixture.client_id,
                        auth_flow,
                        json!({ "REFRESH_TOKEN": token }),
                    ),
                )
                .await;
            }
            Step::RefreshUnknownToken => {
                refused(
                    router,
                    case,
                    "InitiateAuth",
                    &initiate_auth(
                        &fixture.client_id,
                        "REFRESH_TOKEN_AUTH",
                        json!({ "REFRESH_TOKEN": "a-token-nothing-issued" }),
                    ),
                )
                .await;
            }
            Step::ExchangeRefreshToken => {
                let token = refresh_token
                    .clone()
                    .unwrap_or_else(|| panic!("case {case:?}: no sign-in issued a token yet"));
                cognito_ok(
                    router,
                    "GetTokensFromRefreshToken",
                    &json!({ "ClientId": fixture.client_id, "RefreshToken": token }),
                )
                .await;
            }
            Step::SignUp => {
                minted += 1;
                let username = format!("registered-{minted}");
                cognito_ok(
                    router,
                    "SignUp",
                    &json!({
                        "ClientId": fixture.client_id,
                        "Username": username,
                        "Password": GOOD_PASSWORD,
                    }),
                )
                .await;
                registered = Some(username);
            }
            Step::DuplicateSignUp => {
                let username = registered
                    .clone()
                    .unwrap_or_else(|| panic!("case {case:?}: nothing was registered yet"));
                refused(
                    router,
                    case,
                    "SignUp",
                    &json!({
                        "ClientId": fixture.client_id,
                        "Username": username,
                        "Password": GOOD_PASSWORD,
                    }),
                )
                .await;
            }
        }
    }
}

/// An `InitiateAuth` request body.
fn initiate_auth(client_id: &str, auth_flow: &str, auth_parameters: Value) -> Value {
    json!({
        "ClientId": client_id,
        "AuthFlow": auth_flow,
        "AuthParameters": auth_parameters,
    })
}

/// An `AdminInitiateAuth` request body presenting `password`.
fn admin_initiate_auth(fixture: &Fixture, password: &str) -> Value {
    json!({
        "UserPoolId": fixture.user_pool_id,
        "ClientId": fixture.client_id,
        "AuthFlow": "ADMIN_USER_PASSWORD_AUTH",
        "AuthParameters": { "USERNAME": USERNAME, "PASSWORD": password },
    })
}

/// Call `operation`, requiring that it was REFUSED — the attempt ended, and the series owes
/// it a zero-valued sample either way.
async fn refused(router: &MultiServiceRouter, case: &str, operation: &str, body: &Value) {
    let (status, payload) = cognito(router, operation, body).await;
    assert!(
        (400..500).contains(&status),
        "case {case:?}: {operation} must be refused with a client error, got {status} with \
         {payload}"
    );
}

/// The `__type` of an error body, which is the wire error code.
fn error_code(payload: &Value) -> &str {
    payload
        .get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
}

/// The `message` member of an error body.
fn error_message(payload: &Value) -> &str {
    payload
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("<no message>")
}

/// One row of the counter table.
struct Row {
    case: &'static str,
    steps: &'static [Step],
    /// The `(Sum, SampleCount)` each counter must read back as, or `None` where the series
    /// must hold no datapoint at all.
    sign_in: Option<(f64, f64)>,
    token_refresh: Option<(f64, f64)>,
    sign_up: Option<(f64, f64)>,
}

/// The traffic classes the rule generalises over.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "no attempt at all",
            steps: &[],
            sign_in: None,
            token_refresh: None,
            sign_up: None,
        },
        Row {
            case: "one completed sign-in",
            steps: &[Step::SignIn],
            sign_in: Some((1.0, 1.0)),
            token_refresh: None,
            sign_up: None,
        },
        Row {
            case: "two completed sign-ins and three refused on the credential",
            steps: &[
                Step::SignIn,
                Step::WrongPassword,
                Step::SignIn,
                Step::WrongPassword,
                Step::WrongPassword,
            ],
            sign_in: Some((2.0, 5.0)),
            token_refresh: None,
            sign_up: None,
        },
        Row {
            case: "a sign-in for a user the pool does not hold",
            steps: &[Step::UnknownUser],
            sign_in: Some((0.0, 1.0)),
            token_refresh: None,
            sign_up: None,
        },
        Row {
            case: "a sign-in refused by request validation",
            steps: &[Step::InvalidFlow],
            sign_in: Some((0.0, 1.0)),
            token_refresh: None,
            sign_up: None,
        },
        Row {
            case: "the administrative sign-in surface, completed and refused",
            steps: &[Step::AdminSignIn, Step::AdminWrongPassword],
            sign_in: Some((1.0, 2.0)),
            token_refresh: None,
            sign_up: None,
        },
        Row {
            case: "a refresh through each of the flow's two names",
            steps: &[
                Step::SignIn,
                Step::Refresh("REFRESH_TOKEN_AUTH"),
                Step::Refresh("REFRESH_TOKEN"),
            ],
            sign_in: Some((1.0, 1.0)),
            token_refresh: Some((2.0, 2.0)),
            sign_up: None,
        },
        Row {
            case: "a refresh of a token nothing issued",
            steps: &[Step::RefreshUnknownToken],
            sign_in: None,
            token_refresh: Some((0.0, 1.0)),
            sign_up: None,
        },
        Row {
            case: "the refresh-token exchange operation",
            steps: &[Step::SignIn, Step::ExchangeRefreshToken],
            sign_in: Some((1.0, 1.0)),
            token_refresh: Some((1.0, 1.0)),
            sign_up: None,
        },
        Row {
            case: "a registration, and a second one of the same username",
            steps: &[Step::SignUp, Step::DuplicateSignUp],
            sign_in: None,
            token_refresh: None,
            sign_up: Some((1.0, 2.0)),
        },
        Row {
            case: "all three kinds of attempt on one app client",
            steps: &[
                Step::SignIn,
                Step::WrongPassword,
                Step::Refresh("REFRESH_TOKEN_AUTH"),
                Step::SignUp,
                Step::SignUp,
            ],
            sign_in: Some((1.0, 2.0)),
            token_refresh: Some((1.0, 1.0)),
            sign_up: Some((2.0, 2.0)),
        },
    ]
}

/// RULE: one terminated attempt is one datapoint in the series of its kind, valued 1 when it
/// completed and 0 when it was refused, so `Sum` counts the completions and `SampleCount`
/// counts the attempts.
#[tokio::test]
async fn each_terminated_attempt_publishes_one_outcome_sample() {
    for row in rows() {
        let router = emulator();
        let opened = now_secs();
        let fixture = given_pool(&router, None, None).await;
        drive(&router, &fixture, row.case, row.steps).await;

        let keyed = [
            (USER_POOL, fixture.user_pool_id.as_str()),
            (USER_POOL_CLIENT, fixture.client_id.as_str()),
        ];
        for (metric, expected) in [
            (SIGN_IN_SUCCESSES, row.sign_in),
            (TOKEN_REFRESH_SUCCESSES, row.token_refresh),
            (SIGN_UP_SUCCESSES, row.sign_up),
        ] {
            let found = observed(&router, metric, &keyed, opened).await;
            assert_eq!(
                found, expected,
                "case {:?}: expected {metric} to read back {expected:?}, got {found:?}",
                row.case
            );
            // The completion rate a client computes off the same reading is the quotient of
            // the two, which is what makes a refusal observable at all.
            if let (Some((sum, sample_count)), Some((expected_sum, expected_count))) =
                (found, expected)
            {
                assert!(
                    (sum / sample_count - expected_sum / expected_count).abs() < f64::EPSILON,
                    "case {:?}: expected {metric} to average {}, got {}",
                    row.case,
                    expected_sum / expected_count,
                    sum / sample_count
                );
            }
        }
    }
}

/// RULE: a dimension set is part of a metric's identity, so an app client nothing was
/// attempted against has no datapoints in any of the three counters — and `ListMetrics`
/// catalogues the series that DO exist under the dimension pair they are keyed on.
#[tokio::test]
async fn an_app_client_nothing_was_attempted_against_has_no_datapoints() {
    let router = emulator();
    let opened = now_secs();
    let fixture = given_pool(&router, None, None).await;
    drive(
        &router,
        &fixture,
        "traffic on one client only",
        &[Step::SignIn, Step::WrongPassword, Step::SignUp],
    )
    .await;

    for metric in [SIGN_IN_SUCCESSES, TOKEN_REFRESH_SUCCESSES, SIGN_UP_SUCCESSES] {
        let found = observed(
            &router,
            metric,
            &[
                (USER_POOL, fixture.user_pool_id.as_str()),
                (USER_POOL_CLIENT, fixture.unused_client_id.as_str()),
            ],
            opened,
        )
        .await;
        assert_eq!(
            found, None,
            "the never-used app client must have no {metric} datapoint, got {found:?}"
        );
    }

    let catalogue = catalogued(&router, (USER_POOL, fixture.user_pool_id.as_str())).await;
    let mut expected_names = vec![USER_POOL.to_string(), USER_POOL_CLIENT.to_string()];
    expected_names.sort();
    for metric in [SIGN_IN_SUCCESSES, SIGN_UP_SUCCESSES] {
        assert!(
            catalogue.contains(&(metric.to_string(), expected_names.clone())),
            "ListMetrics under {USER_POOL} must catalogue {metric} keyed on \
             {expected_names:?}, got {catalogue:?}"
        );
    }
    assert!(
        !catalogue
            .iter()
            .any(|(metric_name, _)| metric_name == NO_RISK),
        "no risk outcome is keyed on {USER_POOL}, got {catalogue:?}"
    );
}

/// One row of the risk table: the pool's configuration, and what the attempt it serves owes
/// the risk series.
struct RiskRow {
    case: &'static str,
    tier: Option<&'static str>,
    mode: Option<&'static str>,
    /// The `Operation` dimension value and the `(Sum, SampleCount)` the `NoRisk` series must
    /// read back as, or `None` where no evaluation happened at all.
    expected: Option<(&'static str, (f64, f64))>,
    /// Whether the attempt is driven as a completed sign-in, a refused one, or a
    /// registration.
    steps: &'static [Step],
}

/// RULE: a pool that EVALUATES the events it serves publishes the outcome of that evaluation
/// as its own series, keyed on `Operation` + `UserPoolId`; a pool with threat protection off
/// evaluates nothing, and an attempt that never completed is no evaluated event.
#[tokio::test]
async fn a_pool_that_evaluates_its_events_publishes_the_risk_outcome() {
    for row in [
        RiskRow {
            case: "audit-only threat protection, a completed sign-in",
            tier: Some("PLUS"),
            mode: Some("AUDIT"),
            expected: Some(("SignIn", (1.0, 1.0))),
            steps: &[Step::SignIn],
        },
        RiskRow {
            case: "threat protection acting on traffic, two completed sign-ins",
            tier: Some("PLUS"),
            mode: Some("ENFORCED"),
            expected: Some(("SignIn", (2.0, 2.0))),
            steps: &[Step::SignIn, Step::SignIn],
        },
        RiskRow {
            case: "audit-only threat protection, a registration",
            tier: Some("PLUS"),
            mode: Some("AUDIT"),
            expected: Some(("SignUp", (1.0, 1.0))),
            steps: &[Step::SignUp],
        },
        RiskRow {
            case: "audit-only threat protection, a sign-in that was refused",
            tier: Some("PLUS"),
            mode: Some("AUDIT"),
            expected: None,
            steps: &[Step::WrongPassword],
        },
        RiskRow {
            case: "threat protection switched off explicitly",
            tier: Some("PLUS"),
            mode: Some("OFF"),
            expected: None,
            steps: &[Step::SignIn],
        },
        RiskRow {
            case: "a pool that configures no add-ons at all",
            tier: None,
            mode: None,
            expected: None,
            steps: &[Step::SignIn],
        },
    ] {
        let router = emulator();
        let opened = now_secs();
        let fixture = given_pool(&router, row.tier, row.mode).await;
        drive(&router, &fixture, row.case, row.steps).await;

        for operation in ["SignIn", "SignUp"] {
            let expected = row
                .expected
                .filter(|(evaluated, _)| *evaluated == operation)
                .map(|(_, reading)| reading);
            let found = observed(
                &router,
                NO_RISK,
                &[
                    (OPERATION, operation),
                    (USER_POOL_ID, fixture.user_pool_id.as_str()),
                ],
                opened,
            )
            .await;
            assert_eq!(
                found, expected,
                "case {:?}: expected {NO_RISK} for {operation} to read back {expected:?}, got \
                 {found:?}",
                row.case
            );
        }

        // The pool is named `UserPoolId` in a risk outcome and `UserPool` in a counter, and
        // dimension matching is exact: the same query keyed on the counters' name resolves
        // no series at all.
        let keyed_on_the_counter_dimension = observed(
            &router,
            NO_RISK,
            &[
                (OPERATION, "SignIn"),
                (USER_POOL, fixture.user_pool_id.as_str()),
            ],
            opened,
        )
        .await;
        assert_eq!(
            keyed_on_the_counter_dimension, None,
            "case {:?}: {NO_RISK} is keyed on {USER_POOL_ID}, so a read keyed on {USER_POOL} \
             must answer nothing, got {keyed_on_the_counter_dimension:?}",
            row.case
        );

        if let Some((operation, _)) = row.expected {
            let catalogue = catalogued(&router, (USER_POOL_ID, fixture.user_pool_id.as_str())).await;
            let mut expected_names = vec![OPERATION.to_string(), USER_POOL_ID.to_string()];
            expected_names.sort();
            assert!(
                catalogue.contains(&(NO_RISK.to_string(), expected_names.clone())),
                "case {:?}: ListMetrics under {USER_POOL_ID} must catalogue {NO_RISK} keyed on \
                 {expected_names:?} for {operation}, got {catalogue:?}",
                row.case
            );
        }
    }
}

/// RULE: a call answered with a CHALLENGE terminates no attempt, so it publishes nothing
/// yet; the call that ANSWERS that challenge is the one that observes the attempt's outcome,
/// so the whole exchange is one sample and not two.
#[tokio::test]
async fn a_challenge_publishes_nothing_until_its_response_terminates_the_attempt() {
    /// The user an administrator creates with a temporary password, which is what makes its
    /// sign-in answer with the password-change challenge instead of tokens.
    const OWES_A_PASSWORD_CHANGE: &str = "owes-a-password-change";
    /// The challenge such a user is answered with (`ChallengeNameType`).
    const NEW_PASSWORD_REQUIRED: &str = "NEW_PASSWORD_REQUIRED";

    let router = emulator();
    let opened = now_secs();
    let fixture = given_pool(&router, Some("PLUS"), Some("AUDIT")).await;
    cognito_ok(
        &router,
        "AdminCreateUser",
        &json!({
            "UserPoolId": fixture.user_pool_id,
            "Username": OWES_A_PASSWORD_CHANGE,
            "TemporaryPassword": TEMPORARY_PASSWORD,
            "MessageAction": "SUPPRESS",
        }),
    )
    .await;

    let challenged = cognito_ok(
        &router,
        "AdminInitiateAuth",
        &json!({
            "UserPoolId": fixture.user_pool_id,
            "ClientId": fixture.client_id,
            "AuthFlow": "ADMIN_USER_PASSWORD_AUTH",
            "AuthParameters": {
                "USERNAME": OWES_A_PASSWORD_CHANGE,
                "PASSWORD": TEMPORARY_PASSWORD,
            },
        }),
    )
    .await;
    assert_eq!(
        challenged["ChallengeName"], NEW_PASSWORD_REQUIRED,
        "a user that owes a password change must be answered with the challenge, got \
         {challenged}"
    );
    let session = challenged["Session"]
        .as_str()
        .expect("a persisted challenge session")
        .to_string();

    let keyed = [
        (USER_POOL, fixture.user_pool_id.as_str()),
        (USER_POOL_CLIENT, fixture.client_id.as_str()),
    ];
    let risk_keyed = [
        (OPERATION, "SignIn"),
        (USER_POOL_ID, fixture.user_pool_id.as_str()),
    ];
    let during = observed(&router, SIGN_IN_SUCCESSES, &keyed, opened).await;
    assert_eq!(
        during, None,
        "the challenge terminated no attempt, so nothing is published yet, got {during:?}"
    );
    let risk_during = observed(&router, NO_RISK, &risk_keyed, opened).await;
    assert_eq!(
        risk_during, None,
        "no event has been served yet, so there is no evaluated outcome, got {risk_during:?}"
    );

    let completed = cognito_ok(
        &router,
        "AdminRespondToAuthChallenge",
        &json!({
            "UserPoolId": fixture.user_pool_id,
            "ClientId": fixture.client_id,
            "ChallengeName": NEW_PASSWORD_REQUIRED,
            "Session": session,
            "ChallengeResponses": {
                "USERNAME": OWES_A_PASSWORD_CHANGE,
                "NEW_PASSWORD": GOOD_PASSWORD,
            },
        }),
    )
    .await;
    assert!(
        completed["AuthenticationResult"]["AccessToken"].is_string(),
        "answering the challenge must complete the sign-in, got {completed}"
    );

    let after = observed(&router, SIGN_IN_SUCCESSES, &keyed, opened).await;
    assert_eq!(
        after,
        Some((1.0, 1.0)),
        "the response that completed the sign-in is the ONE sample the attempt owes, got \
         {after:?}"
    );
    let risk_after = observed(&router, NO_RISK, &risk_keyed, opened).await;
    assert_eq!(
        risk_after,
        Some((1.0, 1.0)),
        "the completed sign-in is the one evaluated event, got {risk_after:?}"
    );
}

/// RULE: reading the series does not change it — `GetMetricStatistics` reports, so a second
/// identical read answers exactly what the first did, and only new traffic moves it.
#[tokio::test]
async fn reading_the_series_leaves_it_as_it_was() {
    let router = emulator();
    let opened = now_secs();
    let fixture = given_pool(&router, Some("PLUS"), Some("AUDIT")).await;
    drive(
        &router,
        &fixture,
        "one completed and one refused sign-in",
        &[Step::SignIn, Step::WrongPassword],
    )
    .await;

    let keyed = [
        (USER_POOL, fixture.user_pool_id.as_str()),
        (USER_POOL_CLIENT, fixture.client_id.as_str()),
    ];
    let first = observed(&router, SIGN_IN_SUCCESSES, &keyed, opened).await;
    let again = observed(&router, SIGN_IN_SUCCESSES, &keyed, opened).await;
    assert_eq!(
        first,
        Some((1.0, 2.0)),
        "expected one completion in two attempts, got {first:?}"
    );
    assert_eq!(
        again, first,
        "a second read must answer what the first did, got {again:?} after {first:?}"
    );

    let risk = [
        (OPERATION, "SignIn"),
        (USER_POOL_ID, fixture.user_pool_id.as_str()),
    ];
    let first_risk = observed(&router, NO_RISK, &risk, opened).await;
    let again_risk = observed(&router, NO_RISK, &risk, opened).await;
    assert_eq!(
        again_risk, first_risk,
        "a second read of the risk series must answer what the first did, got {again_risk:?} \
         after {first_risk:?}"
    );
}

/// Which app-client flow gate a row exercises: the `AuthFlow` the row asks for, the API it
/// asks it through, and the `ExplicitAuthFlows` value that enables it
/// (`CreateUserPoolClient.ExplicitAuthFlows`, "the authentication flows that you want your
/// user pool client to support").
#[derive(Clone, Copy, Debug)]
enum Gate {
    /// `InitiateAuth` `USER_PASSWORD_AUTH`, which `ALLOW_USER_PASSWORD_AUTH` enables.
    UserPassword,
    /// `AdminInitiateAuth` `ADMIN_USER_PASSWORD_AUTH`, which
    /// `ALLOW_ADMIN_USER_PASSWORD_AUTH` enables: the administrative surface of the same
    /// attempt, gated on its own value.
    AdminPassword,
    /// `InitiateAuth` `REFRESH_TOKEN_AUTH`, which `ALLOW_REFRESH_TOKEN_AUTH` enables — a
    /// flow that presents a token a completed sign-in issued, so the row signs in first.
    Refresh,
}

impl Gate {
    /// The `ExplicitAuthFlows` the app client holds BEFORE it is repaired: everything the
    /// row needs except the flow its gated attempt asks for.
    fn blocked_flows(self) -> &'static [&'static str] {
        match self {
            Self::UserPassword => &["ALLOW_REFRESH_TOKEN_AUTH", "ALLOW_USER_SRP_AUTH"],
            Self::AdminPassword | Self::Refresh => &["ALLOW_USER_PASSWORD_AUTH"],
        }
    }

    /// The `ExplicitAuthFlows` the repair stores: the same list with the flow the gated
    /// attempt asks for added, so the next attempt of that flow is served.
    fn repaired_flows(self) -> &'static [&'static str] {
        match self {
            Self::UserPassword => &[
                "ALLOW_REFRESH_TOKEN_AUTH",
                "ALLOW_USER_SRP_AUTH",
                "ALLOW_USER_PASSWORD_AUTH",
            ],
            Self::AdminPassword => &[
                "ALLOW_USER_PASSWORD_AUTH",
                "ALLOW_ADMIN_USER_PASSWORD_AUTH",
            ],
            Self::Refresh => &["ALLOW_USER_PASSWORD_AUTH", "ALLOW_REFRESH_TOKEN_AUTH"],
        }
    }

    /// The counter the gated attempt is observed in: a token exchange is a refresh, and both
    /// password surfaces sign a user in.
    fn metric(self) -> &'static str {
        match self {
            Self::UserPassword | Self::AdminPassword => SIGN_IN_SUCCESSES,
            Self::Refresh => TOKEN_REFRESH_SUCCESSES,
        }
    }

    /// The sentence this gate refuses with — the wording the `auth_flow_not_enabled` suite
    /// pins: the client-side APIs name the flow asked for, the administrative one words the
    /// same condition in one fixed sentence.
    fn refusal_message(self) -> &'static str {
        match self {
            Self::UserPassword => "USER_PASSWORD_AUTH flow not enabled for this client",
            Self::AdminPassword => "Auth flow not enabled for this client",
            Self::Refresh => "REFRESH_TOKEN_AUTH flow not enabled for this client",
        }
    }

    /// Whether the row has to complete a sign-in before its gated attempt, to hold the
    /// refresh token that attempt presents.
    fn needs_a_sign_in_first(self) -> bool {
        matches!(self, Self::Refresh)
    }

    /// The operation and request body of one attempt of this kind, presenting a credential
    /// the pool really holds — so what ends the attempt is the app client's configuration and
    /// nothing else.
    fn request(self, fixture: &Fixture, refresh_token: &str) -> (&'static str, Value) {
        match self {
            Self::UserPassword => (
                "InitiateAuth",
                initiate_auth(
                    &fixture.client_id,
                    "USER_PASSWORD_AUTH",
                    json!({ "USERNAME": USERNAME, "PASSWORD": GOOD_PASSWORD }),
                ),
            ),
            Self::AdminPassword => (
                "AdminInitiateAuth",
                admin_initiate_auth(fixture, GOOD_PASSWORD),
            ),
            Self::Refresh => (
                "InitiateAuth",
                initiate_auth(
                    &fixture.client_id,
                    "REFRESH_TOKEN_AUTH",
                    json!({ "REFRESH_TOKEN": refresh_token }),
                ),
            ),
        }
    }
}

/// One row of the flow-gate table: the gate, how many attempts it refuses before the app
/// client is repaired, and how many are made after.
struct GateRow {
    case: &'static str,
    gate: Gate,
    refused: usize,
    completed: usize,
}

/// RULE: a refusal the app client's own flow configuration raises — before any credential is
/// checked — still TERMINATES the attempt, so it publishes its zero-valued sample like any
/// other refusal; the series is keyed on the pool and the app client alone, so reconfiguring
/// that client does not start a new one, and the catalogue names the series while every
/// sample in it is still zero.
#[tokio::test]
async fn a_flow_the_app_client_does_not_enable_is_a_zero_valued_sample() {
    for row in [
        GateRow {
            case: "one client-side password sign-in the client does not enable, then repaired",
            gate: Gate::UserPassword,
            refused: 1,
            completed: 1,
        },
        GateRow {
            case: "three gate-refused password sign-ins, then one repaired sign-in",
            gate: Gate::UserPassword,
            refused: 3,
            completed: 1,
        },
        GateRow {
            case: "gate-refused attempts and no attempt after the repair",
            gate: Gate::UserPassword,
            refused: 2,
            completed: 0,
        },
        GateRow {
            case: "the administrative surface, gate-refused twice and then completed twice",
            gate: Gate::AdminPassword,
            refused: 2,
            completed: 2,
        },
        GateRow {
            case: "a refresh-token exchange the client does not enable, then repaired",
            gate: Gate::Refresh,
            refused: 3,
            completed: 1,
        },
    ] {
        let router = emulator();
        let opened = now_secs();
        let base = given_pool(&router, None, None).await;
        let client_id = create_client(
            &router,
            &base.user_pool_id,
            "flow-gated",
            row.gate.blocked_flows(),
        )
        .await;
        let fixture = Fixture { client_id, ..base };
        let keyed = [
            (USER_POOL, fixture.user_pool_id.as_str()),
            (USER_POOL_CLIENT, fixture.client_id.as_str()),
        ];

        // A refresh row holds the token a completed sign-in issued; that sign-in is itself
        // one sample, in the sign-in counter.
        let mut refresh_token = String::new();
        if row.gate.needs_a_sign_in_first() {
            let signed_in = cognito_ok(
                &router,
                "InitiateAuth",
                &initiate_auth(
                    &fixture.client_id,
                    "USER_PASSWORD_AUTH",
                    json!({ "USERNAME": USERNAME, "PASSWORD": GOOD_PASSWORD }),
                ),
            )
            .await;
            refresh_token = signed_in["AuthenticationResult"]["RefreshToken"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!(
                        "case {:?}: a completed sign-in must issue a refresh token, got \
                         {signed_in}",
                        row.case
                    )
                })
                .to_string();
        }

        for attempt in 0..row.refused {
            let (operation, body) = row.gate.request(&fixture, &refresh_token);
            let (status, payload) = cognito(&router, operation, &body).await;
            assert_eq!(
                (status, error_code(&payload)),
                (400, "InvalidParameterException"),
                "case {:?}: attempt {attempt} must be refused by the flow gate, got {status} \
                 with {payload}",
                row.case
            );
            let found = error_message(&payload);
            assert_eq!(
                found,
                row.gate.refusal_message(),
                "case {:?}: attempt {attempt} expected {:?}, got {found:?}",
                row.case,
                row.gate.refusal_message()
            );
        }

        let gated = row.gate.metric();
        let before = observed(&router, gated, &keyed, opened).await;
        let expected_before = Some((0.0, row.refused as f64));
        assert_eq!(
            before, expected_before,
            "case {:?}: expected {gated} to read back {expected_before:?} while every attempt \
             was gate-refused, got {before:?}",
            row.case
        );
        // A series every sample of which is zero still EXISTS, so a client discovering the
        // namespace finds it under the dimension pair the counters are keyed on.
        let catalogue = catalogued(&router, (USER_POOL, fixture.user_pool_id.as_str())).await;
        let mut expected_names = vec![USER_POOL.to_string(), USER_POOL_CLIENT.to_string()];
        expected_names.sort();
        assert!(
            catalogue.contains(&(gated.to_string(), expected_names.clone())),
            "case {:?}: ListMetrics must catalogue {gated} keyed on {expected_names:?}, got \
             {catalogue:?}",
            row.case
        );

        // The repair changes the app client's configuration and attempts nothing, so it owes
        // the series no sample of its own.
        cognito_ok(
            &router,
            "UpdateUserPoolClient",
            &json!({
                "UserPoolId": fixture.user_pool_id,
                "ClientId": fixture.client_id,
                "ExplicitAuthFlows": row.gate.repaired_flows(),
            }),
        )
        .await;
        for attempt in 0..row.completed {
            let (operation, body) = row.gate.request(&fixture, &refresh_token);
            let answered = cognito_ok(&router, operation, &body).await;
            assert!(
                answered["AuthenticationResult"]["AccessToken"].is_string(),
                "case {:?}: attempt {attempt} must complete once the client enables the flow, \
                 got {answered}",
                row.case
            );
        }

        let expected_gated =
            Some((row.completed as f64, (row.refused + row.completed) as f64));
        for metric in [SIGN_IN_SUCCESSES, TOKEN_REFRESH_SUCCESSES, SIGN_UP_SUCCESSES] {
            let expected = if metric == gated {
                expected_gated
            } else if metric == SIGN_IN_SUCCESSES && row.gate.needs_a_sign_in_first() {
                // The sign-in that minted the refresh token, and nothing else.
                Some((1.0, 1.0))
            } else {
                None
            };
            let found = observed(&router, metric, &keyed, opened).await;
            assert_eq!(
                found, expected,
                "case {:?}: expected {metric} to read back {expected:?} across the repair, got \
                 {found:?}",
                row.case
            );
        }
    }
}
