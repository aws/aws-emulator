// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the ISOLATION of an execution environment that serves more than one
//! invocation.
//!
//! Lambda keeps the execution environment of an earlier invocation and serves a later
//! invocation of the same function version in it (`invocation_environment_reuse`), which is
//! what makes the objects a handler declares outside its own method stand between calls. This
//! suite pins the other half of that rule: WHOSE invocations one environment may serve, and
//! what an invocation served in a standing one is handed.
//!
//! * An environment serves the invocations of ONE tenant: an invocation naming a `TenantId`
//!   (`X-Amz-Tenant-Id`, which the runtime reports as `context.tenant_id`) is served only in
//!   an environment built for that tenant, and an invocation naming none only in an
//!   environment built for none — so nothing a handler cached in its interpreter for one
//!   tenant is reachable from another tenant's invocation.
//! * The ENVIRONMENT VARIABLES an invocation reads are its own: the variables of the
//!   invocation before it are replaced rather than left behind, including the X-Ray tracing
//!   header AWS documents as changing with each invocation, and a variable the guest of an
//!   earlier invocation wrote itself is not readable by the next invocation served in that
//!   same environment.
//!
//! The rows differ along what the rules generalise over: which tenant an invocation names
//! (one, another, none at all), and what the invocation before it left in the environment (a
//! tracing header, a variable its own code wrote, nothing).

use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use serde_json::{json, Value};
use service_com_amazonaws_lambda_awsgirapiservice::state::ACTIVATION_DELAY;
use service_com_amazonaws_lambda_awsgirapiservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

use crate::invoke_request_id::one_invocation_stream_at_a_time;
use crate::synchronous_invoke_run::package;

/// Where `CreateFunction` is bound.
const FUNCTIONS_URI: &str = "/2015-03-31/functions";

/// A SigV4 credential scope naming this service's signing name (`lambda`) in `us-east-1`.
/// Nothing in it is verified: the harness turns request authentication off.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=emulator-test-key/20260101/us-east-1/lambda/aws4_request, \
     SignedHeaders=host, Signature=not-verified";

/// A well-formed execution role ARN, satisfying the model's `RoleArn` pattern.
const ROLE: &str = "arn:aws:iam::000000000000:role/service-role/lambda-exec";

/// The module every package below holds its handler in.
const HANDLER: &str = "lambda_function.lambda_handler";

/// How long past a due instant this suite waits before calling. The transition is owed by the
/// CLOCK, so nothing here sleeps on the emulator's behalf.
const SLACK: Duration = Duration::from_millis(150);

/// What a dispatched request answered.
struct Answer {
    status: u16,
    body: bytes::Bytes,
}

impl Answer {
    fn json(&self) -> Value {
        if self.body.is_empty() {
            return Value::Null;
        }
        serde_json::from_slice(&self.body).unwrap_or(Value::Null)
    }

    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).to_string()
    }

    /// A member of the document the handler returned, as text, and the empty string where the
    /// handler answered `None` for it.
    fn member(&self, name: &str) -> String {
        self.json()
            .get(name)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }

    /// How many invocations the environment that served this one has served.
    fn served(&self) -> i64 {
        self.json()
            .get("served")
            .and_then(Value::as_i64)
            .unwrap_or_default()
    }
}

/// One service mounted on its own engine, reached the way the production router reaches it.
fn router_on(engine: &Arc<engine::InMemoryEngine>) -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(engine)));
    router
}

/// Dispatch one request, with the headers `headers` names.
async fn call(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: &[u8],
    headers: &[(&str, &str)],
) -> Answer {
    let mut builder = http::Request::builder()
        .method(method)
        .uri(format!("http://lambda.us-east-1.amazonaws.com{uri}"))
        .header("host", "lambda.us-east-1.amazonaws.com")
        .header("authorization", AUTHORIZATION)
        .header("content-type", "application/json");
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    let request = builder
        .body(bytes::Bytes::from(body.to_vec()))
        .expect("the test request must build");
    let response = router.dispatch(request).await;
    let (parts, body) = response.into_parts();
    Answer {
        status: parts.status.as_u16(),
        body,
    }
}

/// Create a function whose code is `source` and wait out the provisioning window the create
/// documents.
async fn create_function(router: &MultiServiceRouter, name: &str, source: &str) {
    let created = call(
        router,
        "POST",
        FUNCTIONS_URI,
        &serde_json::to_vec(&json!({
            "FunctionName": name,
            "Role": ROLE,
            "Runtime": "python3.12",
            "Handler": HANDLER,
            "Timeout": 10,
            "Code": {
                "ZipFile": base64::engine::general_purpose::STANDARD.encode(package(source)),
            },
        }))
        .expect("the create body serializes"),
        &[],
    )
    .await;
    assert_eq!(
        created.status,
        201,
        "CreateFunction {name:?} must succeed, got {} {}",
        created.status,
        created.text()
    );
    tokio::time::sleep(ACTIVATION_DELAY + SLACK).await;
}

/// `Invoke` the function `name` with the headers `headers` names.
async fn invoke(router: &MultiServiceRouter, name: &str, headers: &[(&str, &str)]) -> Answer {
    call(
        router,
        "POST",
        &format!("{FUNCTIONS_URI}/{name}/invocations"),
        b"{}",
        headers,
    )
    .await
}

/// A handler that reports which environment served the invocation, how many invocations that
/// environment has served, and the tenant the runtime told it this invocation is for.
const REPORTS_TENANT: &str = "import uuid\n\
     \n\
     ENVIRONMENT = uuid.uuid4().hex\n\
     CALLS = []\n\
     \n\
     \n\
     def lambda_handler(event, context):\n\
     \x20   CALLS.append(1)\n\
     \x20   return {\n\
     \x20       'environment': ENVIRONMENT,\n\
     \x20       'served': len(CALLS),\n\
     \x20       'tenant': context.tenant_id,\n\
     \x20   }\n";

/// A handler that reports what the ENVIRONMENT handed it — the tracing header of its own
/// invocation, and whatever an earlier invocation left behind — and then writes a variable of
/// its own, so the next invocation served in that same environment can say whether it stood.
const REPORTS_ENVIRONMENT: &str = "import os\n\
     import uuid\n\
     \n\
     ENVIRONMENT = uuid.uuid4().hex\n\
     CALLS = []\n\
     \n\
     \n\
     def lambda_handler(event, context):\n\
     \x20   CALLS.append(1)\n\
     \x20   seen = {\n\
     \x20       'environment': ENVIRONMENT,\n\
     \x20       'served': len(CALLS),\n\
     \x20       'left': os.environ.get('LEFT_BEHIND'),\n\
     \x20       'trace': os.environ.get('_X_AMZN_TRACE_ID'),\n\
     \x20   }\n\
     \x20   os.environ['LEFT_BEHIND'] = 'invocation %d' % len(CALLS)\n\
     \x20   return seen\n";

/// An invocation is served only in an execution environment of its OWN tenant.
///
/// The rows are one per tenant an invocation can name, including naming none, and each states
/// whether the environment serving it is one that has served before.
#[tokio::test]
async fn an_invocation_is_served_only_in_an_environment_of_its_own_tenant() {
    let _stream = one_invocation_stream_at_a_time().await;
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    let name = "probe-environment-tenant";
    create_function(&router, name, REPORTS_TENANT).await;

    let first = invoke(&router, name, &[("x-amz-tenant-id", "tenant-a")]).await;
    assert_eq!(
        first.status,
        200,
        "case the first invocation of a tenant: it must be served, got {} {}",
        first.status,
        first.text()
    );
    let tenant_a = first.member("environment");
    assert_eq!(
        (first.served(), first.member("tenant")),
        (1, "tenant-a".to_string()),
        "case the first invocation of a tenant: expected a new environment serving that \
         tenant, got {}",
        first.text()
    );

    // The same tenant again: the environment that served it is still the one serving it.
    let again = invoke(&router, name, &[("x-amz-tenant-id", "tenant-a")]).await;
    assert_eq!(
        (again.member("environment"), again.served()),
        (tenant_a.clone(), 2),
        "case the same tenant again: expected its own environment having served two, got {}",
        again.text()
    );

    // Another tenant, and no tenant at all: each is served in an environment of its own, and
    // neither is the one that served the invocations above.
    let cases: &[(&str, Option<&str>)] = &[
        ("another tenant of the same function", Some("tenant-b")),
        ("an invocation naming no tenant at all", None),
    ];
    let mut elsewhere = vec![tenant_a.clone()];
    for (case, tenant) in cases {
        let headers: Vec<(&str, &str)> = tenant
            .map(|tenant| vec![("x-amz-tenant-id", tenant)])
            .unwrap_or_default();
        let answered = invoke(&router, name, &headers).await;
        assert_eq!(
            answered.status,
            200,
            "case {case}: the invocation must be served, got {} {}",
            answered.status,
            answered.text()
        );
        assert_eq!(
            answered.served(),
            1,
            "case {case}: expected an environment of its own, got {}",
            answered.text()
        );
        assert_eq!(
            answered.member("tenant"),
            tenant.unwrap_or_default(),
            "case {case}: expected the tenant the call named, got {}",
            answered.text()
        );
        let reported = answered.member("environment");
        assert!(
            !elsewhere.contains(&reported),
            "case {case}: expected an environment none of {elsewhere:?} served in, got {}",
            answered.text()
        );
        elsewhere.push(reported);
    }

    // And the first tenant's own environment is still standing for it, with everything it has
    // served being that tenant's.
    let returning = invoke(&router, name, &[("x-amz-tenant-id", "tenant-a")]).await;
    assert_eq!(
        (returning.member("environment"), returning.served()),
        (tenant_a, 3),
        "case the first tenant returning: expected its own environment having served three, \
         got {}",
        returning.text()
    );
}

/// What an invocation served in a standing environment reads out of `os.environ` is its OWN
/// environment: the tracing header of its own invocation, and nothing the invocation before it
/// had or wrote.
///
/// The rows differ along what the invocation before left behind: a tracing header the caller
/// supplied, a variable the guest wrote itself, and then a different header again.
#[tokio::test]
async fn a_reused_environment_hands_an_invocation_no_variable_of_the_one_before_it() {
    let _stream = one_invocation_stream_at_a_time().await;
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    let name = "probe-environment-variables";
    create_function(&router, name, REPORTS_ENVIRONMENT).await;

    // One row per invocation, in order: the tracing header the caller supplies (if any), and
    // the header the handler must then read for itself.
    let traced = "Root=1-67891233-abcdef012345678912345678;Sampled=1";
    let other = "Root=1-67891234-abcdef012345678912345679;Sampled=1";
    let cases: &[(&str, Option<&str>, &str)] = &[
        (
            "the invocation that built the environment, with a tracing header",
            Some(traced),
            traced,
        ),
        (
            "the next invocation, with no tracing header of its own",
            None,
            "",
        ),
        (
            "an invocation with a tracing header of a different chain",
            Some(other),
            other,
        ),
    ];
    let mut standing = String::new();
    for (index, (case, supplied, expected)) in cases.iter().enumerate() {
        let headers: Vec<(&str, &str)> = supplied
            .map(|header| vec![("x-amzn-trace-id", header)])
            .unwrap_or_default();
        let answered = invoke(&router, name, &headers).await;
        assert_eq!(
            answered.status,
            200,
            "case {case}: the invocation must be served, got {} {}",
            answered.status,
            answered.text()
        );
        // Every row after the first has to be served in the SAME environment, or it says
        // nothing about what a reused one hands an invocation.
        let environment = answered.member("environment");
        if index == 0 {
            standing = environment.clone();
        } else {
            assert_eq!(
                environment, standing,
                "case {case}: expected the standing environment {standing:?}, got {}",
                answered.text()
            );
        }
        assert_eq!(
            answered.served(),
            i64::try_from(index).expect("the row index fits") + 1,
            "case {case}: expected the environment to have served {} invocations, got {}",
            index + 1,
            answered.text()
        );
        // The tracing header is this invocation's own: the chain it named, or none at all
        // where it named none — never the chain of the invocation before it.
        assert!(
            answered.member("trace").starts_with(*expected)
                && (!expected.is_empty() || answered.member("trace").is_empty()),
            "case {case}: expected a tracing header continuing {expected:?}, got {}",
            answered.text()
        );
        // And nothing the invocation before it wrote into its own environment is here.
        assert_eq!(
            answered.member("left"),
            "",
            "case {case}: expected no variable of an earlier invocation, got {}",
            answered.text()
        );
    }
}
