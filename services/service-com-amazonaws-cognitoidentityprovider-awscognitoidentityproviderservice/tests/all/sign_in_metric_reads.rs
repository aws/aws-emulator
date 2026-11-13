// SPDX-License-Identifier: CC0-1.0

//! How a client READS the `AWS/Cognito` sign-in series a user pool's own traffic publishes.
//!
//! The rule under test: a call that terminates a sign-in against an app client publishes its
//! outcome into that pool's `SignInSuccesses` series, identified by the dimension PAIR
//! `UserPool` (the pool id) + `UserPoolClient` (the app-client id) — so a client reading that
//! series through CloudWatch's own `GetMetricStatistics` finds the sign-in whatever statistics
//! the read asks for, finds it under that pair in either order and under NO other dimension
//! set (a dimension set is part of a metric's identity and matching is exact), and discovers
//! the series through `ListMetrics`; and the series a sign-in lands in follows the attempt,
//! not how the user it signs in came to exist.
//!
//! The rows span what the rule generalises over rather than one recorded read: WHICH
//! statistics the read names (each member of the `Statistic` enumeration on its own, a pair of
//! them, and the whole enumeration), WHICH dimension set it names (the pair, the pair
//! reversed, either half alone, none, the pair plus a third dimension, the pair with the
//! pool's other app client, with another pool, and with the pool named under the dimension
//! name the risk series uses), and HOW the signed-in user was registered (created by an
//! administrator, or self-registered and then confirmed).
//!
//! Both services are mounted on ONE engine exactly as the assembled emulator mounts them,
//! which is what makes the store the pool writes the same one CloudWatch reads.

use bytes::Bytes;
use serde_json::{json, Value};
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

/// The namespace a user pool's own metrics live in, and the counter a sign-in is observed in.
const NAMESPACE: &str = "AWS/Cognito";
const SIGN_IN_SUCCESSES: &str = "SignInSuccesses";
const SIGN_UP_SUCCESSES: &str = "SignUpSuccesses";

/// The dimension names the counters are keyed on, and the name the risk series carries the
/// pool under instead.
const USER_POOL: &str = "UserPool";
const USER_POOL_CLIENT: &str = "UserPoolClient";
const USER_POOL_ID: &str = "UserPoolId";

/// Every statistic a `GetMetricStatistics` read can name, which is the `Statistic`
/// enumeration (`smithy_query(query="shape",shape="Statistic")` on the CloudWatch model) —
/// also the maximum length of the `Statistics` member it travels in (len 1..5).
const STATISTIC_CATALOG: [&str; 5] = ["SampleCount", "Average", "Sum", "Minimum", "Maximum"];

/// A password satisfying a default pool's password policy, and one no user holds.
const GOOD_PASSWORD: &str = "Passw0rd!2024";
const WRONG_PASSWORD: &str = "N0tTheP4ssword!";

/// The user each row signs in.
const USERNAME: &str = "reader-user";

/// The period the read aggregates into, and how far either side of the calls its window
/// reaches, so the window holds every datapoint whatever minute the calls land in.
const PERIOD_SECS: i64 = 3600;
const WINDOW_SLACK_SECS: f64 = 1_200.0;

/// The `ExplicitAuthFlows` the app clients here admit.
const FLOWS: [&str; 2] = ["ALLOW_USER_PASSWORD_AUTH", "ALLOW_REFRESH_TOKEN_AUTH"];

/// One emulator with this service and CloudWatch mounted on ONE engine.
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

/// The datapoints one `GetMetricStatistics` read answers with, over the window around the
/// calls — the read a client watching the series makes.
async fn read_series(
    router: &MultiServiceRouter,
    metric: &str,
    dimensions: &[(&str, &str)],
    statistics: &[&str],
    opened: f64,
) -> Vec<Value> {
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
            "Statistics": statistics,
        }),
    )
    .await;
    answered
        .get("Datapoints")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// The `Sum` and `SampleCount` the series reads back as, or `None` where the read answers no
/// datapoint at all.
async fn observed(
    router: &MultiServiceRouter,
    metric: &str,
    dimensions: &[(&str, &str)],
    opened: f64,
) -> Option<(f64, f64)> {
    let points = read_series(router, metric, dimensions, &["Sum", "SampleCount"], opened).await;
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

/// Create a user pool and return its id.
async fn given_pool(router: &MultiServiceRouter, pool_name: &str) -> String {
    let created = cognito_ok(router, "CreateUserPool", &json!({ "PoolName": pool_name })).await;
    created["UserPool"]["Id"]
        .as_str()
        .expect("a minted pool id")
        .to_string()
}

/// Create an app client in `user_pool_id` and return its minted id.
async fn given_client(
    router: &MultiServiceRouter,
    user_pool_id: &str,
    client_name: &str,
) -> String {
    let created = cognito_ok(
        router,
        "CreateUserPoolClient",
        &json!({
            "UserPoolId": user_pool_id,
            "ClientName": client_name,
            "GenerateSecret": false,
            "ExplicitAuthFlows": FLOWS,
        }),
    )
    .await;
    created["UserPoolClient"]["ClientId"]
        .as_str()
        .expect("a minted client id")
        .to_string()
}

/// A `CONFIRMED` user an administrator created, holding [`GOOD_PASSWORD`] permanently.
async fn given_admin_created_user(router: &MultiServiceRouter, user_pool_id: &str) {
    cognito_ok(
        router,
        "AdminCreateUser",
        &json!({
            "UserPoolId": user_pool_id,
            "Username": USERNAME,
            "MessageAction": "SUPPRESS",
        }),
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
}

/// A `CONFIRMED` user that registered ITSELF through the app client and was then confirmed.
async fn given_self_registered_user(
    router: &MultiServiceRouter,
    user_pool_id: &str,
    client_id: &str,
) {
    cognito_ok(
        router,
        "SignUp",
        &json!({
            "ClientId": client_id,
            "Username": USERNAME,
            "Password": GOOD_PASSWORD,
        }),
    )
    .await;
    cognito_ok(
        router,
        "AdminConfirmSignUp",
        &json!({ "UserPoolId": user_pool_id, "Username": USERNAME }),
    )
    .await;
}

/// Sign in with `password`, returning what the call answered.
async fn sign_in(router: &MultiServiceRouter, client_id: &str, password: &str) -> (u16, Value) {
    cognito(
        router,
        "InitiateAuth",
        &json!({
            "ClientId": client_id,
            "AuthFlow": "USER_PASSWORD_AUTH",
            "AuthParameters": { "USERNAME": USERNAME, "PASSWORD": password },
        }),
    )
    .await
}

/// RULE: the sign-ins a window aggregated are reported through whichever statistics the read
/// names — a completed attempt is a sample valued 1 and a refused one a sample valued 0, so
/// `Sum` is the completions, `SampleCount` the attempts and `Average` the completion rate —
/// and a statistic the read did NOT name is not reported at all.
#[tokio::test]
async fn every_statistic_a_read_names_reports_the_sign_ins_the_window_holds() {
    let router = emulator();
    let opened = now_secs();
    let user_pool_id = given_pool(&router, "sign-in-reads").await;
    let client_id = given_client(&router, &user_pool_id, "reader").await;
    given_admin_created_user(&router, &user_pool_id).await;

    let (status, payload) = sign_in(&router, &client_id, GOOD_PASSWORD).await;
    assert_eq!(
        status, 200,
        "the sign-in the series is read for must succeed, got {status} with {payload}"
    );
    let (status, payload) = sign_in(&router, &client_id, WRONG_PASSWORD).await;
    assert!(
        (400..500).contains(&status),
        "a sign-in with a password the user does not hold must be refused, got {status} with \
         {payload}"
    );

    // One completion in two attempts, which is what each statistic of the window reports.
    let expected = |statistic: &str| -> f64 {
        match statistic {
            "SampleCount" => 2.0,
            "Average" => 0.5,
            "Sum" | "Maximum" => 1.0,
            _ => 0.0,
        }
    };
    let keyed = [
        (USER_POOL, user_pool_id.as_str()),
        (USER_POOL_CLIENT, client_id.as_str()),
    ];
    for (case, statistics) in [
        ("the sum alone", &["Sum"][..]),
        ("the sample count alone", &["SampleCount"][..]),
        ("the average alone", &["Average"][..]),
        ("the minimum alone", &["Minimum"][..]),
        ("the maximum alone", &["Maximum"][..]),
        ("the sum beside the sample count", &["Sum", "SampleCount"][..]),
        ("every statistic the member admits", &STATISTIC_CATALOG[..]),
    ] {
        let points = read_series(&router, SIGN_IN_SUCCESSES, &keyed, statistics, opened).await;
        assert_eq!(
            points.len(),
            1,
            "case {case:?}: the two attempts fall in one period, so the read answers one \
             datapoint, got {points:?}"
        );
        let point = &points[0];
        for statistic in STATISTIC_CATALOG {
            let found = point.get(statistic).and_then(Value::as_f64);
            if statistics.contains(&statistic) {
                let wanted = expected(statistic);
                assert!(
                    found.is_some_and(|value| (value - wanted).abs() < f64::EPSILON),
                    "case {case:?}: expected {statistic} to report {wanted}, got {found:?} in \
                     {point}"
                );
            } else {
                assert!(
                    found.is_none(),
                    "case {case:?}: {statistic} was not asked for, so it is not reported, got \
                     {found:?} in {point}"
                );
            }
        }
    }
}

/// RULE: the dimension set is part of a metric's identity and matching is exact, so the
/// sign-in series answers a read naming the PAIR the pool published it under — in either
/// order, a dimension set being unordered — and no read naming any other set; `ListMetrics`
/// is how a client discovers which pair that is.
#[tokio::test]
async fn the_series_answers_only_the_dimension_pair_it_was_published_under() {
    let router = emulator();
    let opened = now_secs();
    let user_pool_id = given_pool(&router, "sign-in-dimensions").await;
    let client_id = given_client(&router, &user_pool_id, "signed-in").await;
    let other_client_id = given_client(&router, &user_pool_id, "never-used").await;
    let other_pool_id = given_pool(&router, "another-pool").await;
    given_admin_created_user(&router, &user_pool_id).await;

    let (status, payload) = sign_in(&router, &client_id, GOOD_PASSWORD).await;
    assert_eq!(
        status, 200,
        "the sign-in the series is read for must succeed, got {status} with {payload}"
    );

    // The one completed attempt, as the series keyed on the pair reads back.
    let published = Some((1.0, 1.0));
    for (case, dimensions, expected) in [
        (
            "the pair the attempt published",
            vec![
                (USER_POOL, user_pool_id.as_str()),
                (USER_POOL_CLIENT, client_id.as_str()),
            ],
            published,
        ),
        (
            "the same pair in the other order",
            vec![
                (USER_POOL_CLIENT, client_id.as_str()),
                (USER_POOL, user_pool_id.as_str()),
            ],
            published,
        ),
        (
            "the pool alone",
            vec![(USER_POOL, user_pool_id.as_str())],
            None,
        ),
        (
            "the app client alone",
            vec![(USER_POOL_CLIENT, client_id.as_str())],
            None,
        ),
        ("no dimension at all", Vec::new(), None),
        (
            "the pair beside a third dimension",
            vec![
                (USER_POOL, user_pool_id.as_str()),
                (USER_POOL_CLIENT, client_id.as_str()),
                ("Operation", "SignIn"),
            ],
            None,
        ),
        (
            "the pool's other app client",
            vec![
                (USER_POOL, user_pool_id.as_str()),
                (USER_POOL_CLIENT, other_client_id.as_str()),
            ],
            None,
        ),
        (
            "another pool holding this app client",
            vec![
                (USER_POOL, other_pool_id.as_str()),
                (USER_POOL_CLIENT, client_id.as_str()),
            ],
            None,
        ),
        (
            "the pool under the name the risk series carries it as",
            vec![
                (USER_POOL_ID, user_pool_id.as_str()),
                (USER_POOL_CLIENT, client_id.as_str()),
            ],
            None,
        ),
    ] {
        let found = observed(&router, SIGN_IN_SUCCESSES, &dimensions, opened).await;
        assert_eq!(
            found, expected,
            "case {case:?}: expected {SIGN_IN_SUCCESSES} to read back {expected:?} for \
             {dimensions:?}, got {found:?}"
        );
    }

    let mut pair = vec![USER_POOL.to_string(), USER_POOL_CLIENT.to_string()];
    pair.sort();
    for filter in [
        (USER_POOL, user_pool_id.as_str()),
        (USER_POOL_CLIENT, client_id.as_str()),
    ] {
        let catalogue = catalogued(&router, filter).await;
        assert!(
            catalogue.contains(&(SIGN_IN_SUCCESSES.to_string(), pair.clone())),
            "ListMetrics filtered on {filter:?} must catalogue {SIGN_IN_SUCCESSES} keyed on \
             {pair:?}, got {catalogue:?}"
        );
    }
    let catalogue = catalogued(&router, (USER_POOL_CLIENT, other_client_id.as_str())).await;
    assert!(
        catalogue.is_empty(),
        "nothing was attempted against the other app client, so it catalogues no series, got \
         {catalogue:?}"
    );
}

/// RULE: which series an attempt is observed in follows what the call attempted, not how the
/// user it signs in came to exist — so a sign-in lands in the sign-in counter whether an
/// administrator created that user or the user registered itself, and the registration is the
/// only thing the registration counter holds.
#[tokio::test]
async fn a_sign_in_is_observed_in_the_sign_in_counter_however_the_user_was_registered() {
    for (case, self_registered) in [
        ("a user an administrator created", false),
        ("a user that registered itself", true),
    ] {
        let router = emulator();
        let opened = now_secs();
        let user_pool_id = given_pool(&router, "sign-in-registration").await;
        let client_id = given_client(&router, &user_pool_id, "reader").await;
        if self_registered {
            given_self_registered_user(&router, &user_pool_id, &client_id).await;
        } else {
            given_admin_created_user(&router, &user_pool_id).await;
        }

        let (status, payload) = sign_in(&router, &client_id, GOOD_PASSWORD).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the sign-in must succeed, got {status} with {payload}"
        );

        let keyed = [
            (USER_POOL, user_pool_id.as_str()),
            (USER_POOL_CLIENT, client_id.as_str()),
        ];
        let found = observed(&router, SIGN_IN_SUCCESSES, &keyed, opened).await;
        assert_eq!(
            found,
            Some((1.0, 1.0)),
            "case {case:?}: expected the completed sign-in to be the one sample \
             {SIGN_IN_SUCCESSES} holds, got {found:?}"
        );
        // The registration counter holds the registration and nothing else: a sign-in is not
        // a registration, however the user was registered.
        let expected_sign_ups = self_registered.then_some((1.0, 1.0));
        let found = observed(&router, SIGN_UP_SUCCESSES, &keyed, opened).await;
        assert_eq!(
            found, expected_sign_ups,
            "case {case:?}: expected {SIGN_UP_SUCCESSES} to read back {expected_sign_ups:?}, \
             got {found:?}"
        );
    }
}
