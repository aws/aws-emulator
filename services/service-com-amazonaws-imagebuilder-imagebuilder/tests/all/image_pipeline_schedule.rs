// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the SCHEDULE an image pipeline fires on: which
//! `schedule.scheduleExpression` values `CreateImagePipeline` and `UpdateImagePipeline`
//! accept, how one they cannot read is refused, and what a refused request leaves behind.
//!
//! Every case drives the real wire path — the production `MultiServiceRouter`, restJson1
//! serialization, the modelled-constraint walk and the handlers — so what is asserted is
//! what a signed AWS SDK request observes.
//!
//! The rules under test:
//!
//! * `scheduleExpression` is an EXPRESSION in a grammar — "You can specify a cron expression,
//!   or a rate expression such as `rate(1 day)`"
//!   (`smithy_query(query="doc",shape="Schedule",member="scheduleExpression")`) — and its
//!   target carries a length bound and no pattern, so the SERVICE reads the syntax: an
//!   expression it cannot read is refused as that parameter's value, with HTTP 400,
//!   `InvalidParameterValueException` and the one sentence this service states every
//!   parameter refusal in, whose requirement is the parser's own statement of what stopped it
//!   — the offset into the expression body it stopped at and the token it stopped on, in
//!   upper case.
//! * The refusal is decided BEFORE anything is written, so a refused create stores no
//!   pipeline and leaves the name it asked for free: the retry that corrects the expression
//!   creates the pipeline under that same name, and a refused update replaces nothing.
//! * An expression the parser reads is stored as submitted, whichever form it takes, and a
//!   read of the pipeline reports it back — beside the start condition the pipeline will run
//!   under: "If no value is set Image Builder defaults to
//!   `EXPRESSION_MATCH_AND_DEPENDENCY_UPDATES_AVAILABLE`"
//!   (`smithy_query(query="doc",shape="Schedule",member="pipelineExecutionStartCondition")`),
//!   so a request naming no condition reads back carrying that one, exactly as the test
//!   settings a create stores read back carrying their own defaults.
//! * A pipeline reports the instant of its last WRITE: the create is that write, so
//!   `dateUpdated` carries the create's own instant until an update replaces the record.
//! * A pipeline that is deleted is gone: a read of its ARN reports it missing, and its name
//!   is free again.
//!
//! The rows below span what the rules generalise over: which of the six cron fields carries
//! the token that cannot be read, the letter case it was written in, how many fields the body
//! carries, the shape of a rate expression and its unit, a value that is no expression at
//! all, and both operations that take the member.
//!
//! The harness and the refusal wording below are `pub(crate)`: the sibling suite
//! [`super::image_pipeline_schedule_listing`] reads the same rule from the account's listing
//! and borrows them from here rather than keeping a second copy of them, so one wording
//! cannot drift from the other.

use std::sync::Arc;

use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

// Two instants this model reports are ordered through the one reading of its string
// `DateTime` members the sibling suite states, so neither suite keeps a second copy of it.
use super::last_updated_projections::not_before;

/// The Region every case signs for; the ARNs it creates carry it.
const REGION: &str = "us-west-2";

/// The canonical emulator account (`engine::EMULATOR_ACCOUNT_ID`) every ARN carries.
const ACCOUNT: &str = "000000000000";

/// A build component document: one `build` phase, which is all a recipe these cases
/// reference needs.
const BUILD_DOCUMENT: &str = concat!(
    "name: HelloWorld\n",
    "schemaVersion: 1.0\n",
    "phases:\n",
    "  - name: build\n",
    "    steps:\n",
    "      - name: Greet\n",
    "        action: ExecuteBash\n",
    "        inputs:\n",
    "          commands:\n",
    "            - echo hello\n",
);

/// One emulator with an empty store, mounted exactly as the router mounts it.
pub(crate) fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_imagebuilder_imagebuilder::EmulatedService::new(engine),
    );
    router
}

/// A request signed for `imagebuilder` in [`REGION`] — the signal the router routes on and
/// the handlers read the Region from.
fn request(method: &str, uri: &str, body: Option<&Value>) -> http::Request<bytes::Bytes> {
    let mut builder = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("imagebuilder.{REGION}.amazonaws.com"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDIMAGEBUILDER/20260101/{REGION}/imagebuilder/\
                 aws4_request, SignedHeaders=host, Signature=0000"
            ),
        );
    let payload = match body {
        Some(value) => {
            builder = builder.header("content-type", "application/json");
            bytes::Bytes::from(serde_json::to_vec(value).expect("serialize the request body"))
        }
        None => bytes::Bytes::new(),
    };
    builder.body(payload).expect("a well-formed test request")
}

/// Status and parsed body of one call. An empty body reads as `Value::Null`.
pub(crate) async fn call(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<&Value>,
) -> (u16, Value) {
    let response = router.dispatch(request(method, uri, body)).await;
    let status = response.status().as_u16();
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let raw = response.into_body();
    let mut parsed: Value = if raw.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&raw)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&raw).to_string()))
    };
    // restJson1 carries the error shape in `__type`, and the framework also sets the
    // `x-amzn-errortype` header; fold the header in so one accessor reads either.
    if let (Some(error_type), Value::Object(map)) = (error_type, &mut parsed) {
        map.entry("__type")
            .or_insert_with(|| Value::String(error_type));
    }
    (status, parsed)
}

/// Percent-encode one path segment, as an AWS SDK encodes a NON-greedy `@httpLabel`, so the
/// `:` and `/` of an ARN travel as `%3A` and `%2F` and the whole ARN stays ONE segment.
fn encode_label(value: &str) -> String {
    percent_encoding::utf8_percent_encode(value, percent_encoding::NON_ALPHANUMERIC).to_string()
}

/// Read `name` out of an answer, panicking with the case when it is absent.
pub(crate) fn text(case: &str, body: &Value, name: &str) -> String {
    body.get(name)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: expected text in {name} of {body}"))
        .to_string()
}

/// Assert the call succeeded and answer its body.
pub(crate) fn assert_ok(case: &str, status: u16, body: Value) -> Value {
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with {body}"
    );
    body
}

/// The `__type` of an error answer, with any shape-id prefix dropped.
pub(crate) fn error_code(case: &str, body: &Value) -> String {
    let rendered = body
        .get("__type")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: expected a __type member in {body}"));
    rendered
        .rsplit(['#', '/'])
        .next()
        .unwrap_or(rendered)
        .to_string()
}

/// The `message` of an error answer.
pub(crate) fn error_message(case: &str, body: &Value) -> String {
    body.get("message")
        .or_else(|| body.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: expected a message member in {body}"))
        .to_string()
}

/// The whole sentence a refused schedule expression is answered with: this service's
/// parameter-value sentence about the member the value sat at, carrying `detail`.
pub(crate) fn refusal_message(detail: &str) -> String {
    format!(
        "The value supplied for parameter 'schedule.scheduleExpression' is not valid. Invalid \
         schedule expression. Details: {detail}"
    )
}

/// The parser's statement about a cron body it stopped reading at `position` on `token`.
pub(crate) fn illegal_characters(position: usize, token: &str) -> String {
    format!(
        "Invalid CRON expression: error at position {position}, details: Illegal characters for \
         this position: '{token}'"
    )
}

/// The parser's statement about a cron body that ends before the field `missing` would have
/// been, at `position`.
pub(crate) fn unexpected_end(position: usize, missing: &str) -> String {
    format!(
        "Invalid CRON expression: error at position {position}, details: Unexpected end of \
         expression, the {missing} field is missing, the CRON format consists of 6 fields \
         ([Minutes] [Hours] [Day_of_Month] [Month] [Day_of_Week] [Year])."
    )
}

/// The statement a value that is no schedule expression at all is refused with: the grammar
/// an expression has to be written in.
pub(crate) const SCHEDULE_SYNTAX: &str = "Schedule expressions must have the following syntax: \
     rate(<number>\\s?(minutes?|hours?|days?)), cron(<cron_expression>) or \
     at(yyyy-MM-dd'T'HH:mm:ss).";

/// `CreateComponent` for one build version, answering its build version ARN.
async fn create_component(router: &MultiServiceRouter, name: &str) -> String {
    let body = json!({
        "name": name,
        "semanticVersion": "1.0.0",
        "platform": "Linux",
        "clientToken": format!("component-{name}"),
        "data": BUILD_DOCUMENT,
    });
    let case = format!("create the component {name}");
    let (status, answer) = call(router, "PUT", "/CreateComponent", Some(&body)).await;
    let answer = assert_ok(&case, status, answer);
    text(&case, &answer, "componentBuildVersionArn")
}

/// `CreateImageRecipe` including `component_arn`, answering the recipe's own ARN.
async fn create_image_recipe(
    router: &MultiServiceRouter,
    name: &str,
    component_arn: &str,
) -> String {
    let body = json!({
        "name": name,
        "semanticVersion": "1.0.0",
        "parentImage": "ami-0123456789abcdef0",
        "clientToken": format!("recipe-{name}"),
        "components": [{ "componentArn": component_arn }],
    });
    let case = format!("create the image recipe {name}");
    let (status, answer) = call(router, "PUT", "/CreateImageRecipe", Some(&body)).await;
    let answer = assert_ok(&case, status, answer);
    text(&case, &answer, "imageRecipeArn")
}

/// `CreateInfrastructureConfiguration`, answering its ARN.
async fn create_configuration(router: &MultiServiceRouter, name: &str) -> String {
    let body = json!({
        "name": name,
        "clientToken": format!("configuration-{name}"),
        "instanceProfileName": "EC2InstanceProfileForImageBuilder",
    });
    let case = format!("create the infrastructure configuration {name}");
    let (status, answer) = call(
        router,
        "PUT",
        "/CreateInfrastructureConfiguration",
        Some(&body),
    )
    .await;
    let answer = assert_ok(&case, status, answer);
    text(&case, &answer, "infrastructureConfigurationArn")
}

/// The resources every pipeline these cases create references.
pub(crate) struct Fixture {
    /// The image recipe the pipeline builds.
    recipe: String,
    /// The infrastructure configuration it builds in.
    configuration: String,
}

impl Fixture {
    /// Create the recipe and the configuration one emulator's cases share.
    pub(crate) async fn given(router: &MultiServiceRouter) -> Self {
        let component = create_component(router, "schedule-component").await;
        Self {
            recipe: create_image_recipe(router, "schedule-recipe", &component).await,
            configuration: create_configuration(router, "schedule-infra").await,
        }
    }

    /// A `CreateImagePipeline` body for `name` carrying `extra`.
    pub(crate) fn create_body(&self, name: &str, client_token: &str, extra: Value) -> Value {
        let mut body = Map::new();
        body.insert("name".to_string(), json!(name));
        body.insert("clientToken".to_string(), json!(client_token));
        body.insert("imageRecipeArn".to_string(), json!(self.recipe));
        body.insert(
            "infrastructureConfigurationArn".to_string(),
            json!(self.configuration),
        );
        if let Value::Object(extra) = extra {
            for (member, value) in extra {
                body.insert(member, value);
            }
        }
        Value::Object(body)
    }

    /// An `UpdateImagePipeline` body for `arn` carrying `extra`.
    fn update_body(&self, arn: &str, client_token: &str, extra: Value) -> Value {
        let mut body = Map::new();
        body.insert("imagePipelineArn".to_string(), json!(arn));
        body.insert("clientToken".to_string(), json!(client_token));
        body.insert("imageRecipeArn".to_string(), json!(self.recipe));
        body.insert(
            "infrastructureConfigurationArn".to_string(),
            json!(self.configuration),
        );
        if let Value::Object(extra) = extra {
            for (member, value) in extra {
                body.insert(member, value);
            }
        }
        Value::Object(body)
    }
}

/// `CreateImagePipeline` with `body`.
pub(crate) async fn create_pipeline(router: &MultiServiceRouter, body: &Value) -> (u16, Value) {
    call(router, "PUT", "/CreateImagePipeline", Some(body)).await
}

/// `UpdateImagePipeline` with `body`.
async fn update_pipeline(router: &MultiServiceRouter, body: &Value) -> (u16, Value) {
    call(router, "PUT", "/UpdateImagePipeline", Some(body)).await
}

/// `GetImagePipeline` on `arn`.
///
/// `pub(crate)`: the sibling suite [`super::image_pipeline_create_call`] reads the same
/// pipeline back and borrows this one rather than writing a second GetImagePipeline URI.
pub(crate) async fn get_pipeline(router: &MultiServiceRouter, arn: &str) -> (u16, Value) {
    call(
        router,
        "GET",
        &format!("/GetImagePipeline?imagePipelineArn={}", encode_label(arn)),
        None,
    )
    .await
}

/// The ARN a pipeline named `normalized` carries in [`REGION`].
pub(crate) fn pipeline_arn(normalized: &str) -> String {
    format!("arn:aws:imagebuilder:{REGION}:{ACCOUNT}:image-pipeline/{normalized}")
}

/// One expression the parser cannot read: where in the body it stopped, stated as the parser
/// states it.
struct Refused {
    /// What the row is about.
    case: &'static str,
    /// The expression the request carried.
    expression: &'static str,
    /// The requirement clause the refusal states.
    detail: String,
}

/// The catalog of unreadable expressions: a token the parser cannot read in each of the six
/// fields the cron format declares, written in either letter case, bodies of the wrong field
/// count, and values that are no expression at all.
fn refused_expressions() -> Vec<Refused> {
    vec![
        Refused {
            case: "a minutes field that is not a minute",
            expression: "cron(not valid)",
            detail: illegal_characters(0, "NOT"),
        },
        Refused {
            case: "a named minutes field",
            expression: "cron(sixty 10 * * ? *)",
            detail: illegal_characters(0, "SIXTY"),
        },
        Refused {
            case: "an hours field out of range",
            expression: "cron(0 99 * * ? *)",
            detail: illegal_characters(2, "99"),
        },
        Refused {
            case: "a day of month that is not a day",
            expression: "cron(0 10 someday * ? *)",
            detail: illegal_characters(5, "SOMEDAY"),
        },
        Refused {
            case: "a month that is not a month",
            expression: "cron(0 10 * SMARCH ? *)",
            detail: illegal_characters(7, "SMARCH"),
        },
        Refused {
            case: "a day of week that is not a day, in lower case",
            expression: "cron(0 10 ? * funday *)",
            detail: illegal_characters(9, "FUNDAY"),
        },
        Refused {
            case: "a year that is not a year",
            expression: "cron(0 10 * * ? someday)",
            detail: illegal_characters(11, "SOMEDAY"),
        },
        Refused {
            case: "seven fields, so the year field reads a day-of-week token",
            expression: "cron(0 0 10 * * ? *)",
            detail: illegal_characters(11, "?"),
        },
        Refused {
            case: "five fields, so no year",
            expression: "cron(0 10 * * ?)",
            detail: unexpected_end(10, "Year"),
        },
        Refused {
            case: "no fields at all",
            expression: "cron()",
            detail: unexpected_end(0, "Minutes"),
        },
        Refused {
            case: "a value that is no expression at all",
            expression: "nightly",
            detail: SCHEDULE_SYNTAX.to_string(),
        },
        Refused {
            case: "a rate naming no unit",
            expression: "rate(5)",
            detail: SCHEDULE_SYNTAX.to_string(),
        },
        Refused {
            case: "a rate of a unit that is not one",
            expression: "rate(2 fortnights)",
            detail: SCHEDULE_SYNTAX.to_string(),
        },
        Refused {
            case: "a rate of zero",
            expression: "rate(0 hours)",
            detail: SCHEDULE_SYNTAX.to_string(),
        },
    ]
}

/// The expressions the parser reads, one per form the documentation names: the cron fields
/// with each of the operators they admit, and a rate for every unit the grammar accepts
/// (`service_common::schedule_expression::RATE_UNITS`, which is what reads them), beside the
/// one the member's own documentation gives ("a rate expression such as `rate(1 day)`").
const ACCEPTED_EXPRESSIONS: &[(&str, &str)] = &[
    ("a weekly cron", "cron(0 3 ? * SUN *)"),
    ("a daily cron", "cron(0 0 * * ? *)"),
    ("a cron with a step", "cron(0/15 * * * ? *)"),
    ("a cron with a list and a named month", "cron(0 8,20 1 JAN ? *)"),
    ("a cron with a day-of-week range", "cron(30 9 ? * MON-FRI *)"),
    ("a cron naming a year", "cron(15 2 1 * ? 2030)"),
    ("a rate of one day", "rate(1 day)"),
    ("a rate of several days", "rate(3 days)"),
    ("a rate of one hour", "rate(1 hour)"),
    ("a rate of several hours", "rate(6 hours)"),
    ("a rate of one minute", "rate(1 minute)"),
    ("a rate of several minutes", "rate(15 minutes)"),
];

#[tokio::test]
async fn an_expression_the_parser_cannot_read_is_refused_and_stores_no_pipeline() {
    let router = emulator();
    let fixture = Fixture::given(&router).await;

    for (index, row) in refused_expressions().iter().enumerate() {
        let case = row.case;
        let name = format!("refused-pipeline-{index}");
        let body = fixture.create_body(
            &name,
            &format!("create-refused-{index}"),
            json!({"schedule": {"scheduleExpression": row.expression}}),
        );
        let (status, refused) = create_pipeline(&router, &body).await;
        assert_eq!(
            status, 400,
            "case {case:?} ({:?}): expected HTTP 400, got {status} with {refused}",
            row.expression
        );
        let code = error_code(case, &refused);
        assert_eq!(
            code, "InvalidParameterValueException",
            "case {case:?}: expected InvalidParameterValueException, got {code:?} with {refused}"
        );
        let expected = refusal_message(&row.detail);
        let message = error_message(case, &refused);
        assert_eq!(
            message, expected,
            "case {case:?} ({:?}): expected {expected:?}, got {message:?}",
            row.expression
        );
        for member in ["imagePipelineArn", "clientToken"] {
            assert!(
                refused.get(member).is_none(),
                "case {case:?}: expected no {member} on the refusal, got {refused}"
            );
        }

        // Nothing was written: the ARN the create would have minted resolves to no
        // pipeline, so the name it asked for is still free.
        let (status, missing) = get_pipeline(&router, &pipeline_arn(&name)).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected the refused create to store nothing, got {status} with \
             {missing}"
        );
    }
}

#[tokio::test]
async fn an_expression_the_parser_reads_is_stored_as_submitted() {
    let router = emulator();
    let fixture = Fixture::given(&router).await;

    for (index, (case, expression)) in ACCEPTED_EXPRESSIONS.iter().enumerate() {
        let name = format!("accepted-pipeline-{index}");
        let body = fixture.create_body(
            &name,
            &format!("create-accepted-{index}"),
            json!({"schedule": {"scheduleExpression": expression, "timezone": "Etc/UTC"}}),
        );
        let (status, created) = create_pipeline(&router, &body).await;
        let created = assert_ok(case, status, created);
        let arn = text(case, &created, "imagePipelineArn");
        assert_eq!(
            arn,
            pipeline_arn(&name),
            "case {case:?}: expected the pipeline ARN of {name}, got {arn:?}"
        );
        assert!(
            created.get("requestId").is_some() && created.get("clientToken").is_some(),
            "case {case:?}: expected a requestId and a clientToken, got {created}"
        );

        // The round trip: a read reports the expression back as submitted.
        let (status, read) = get_pipeline(&router, &arn).await;
        let read = assert_ok(case, status, read);
        let stored = read
            .get("imagePipeline")
            .and_then(|pipeline| pipeline.get("schedule"))
            .cloned()
            .unwrap_or_else(|| panic!("case {case:?}: expected a stored schedule in {read}"));
        assert_eq!(
            stored.get("scheduleExpression"),
            Some(&json!(expression)),
            "case {case:?}: expected the schedule {expression:?} to be reported back, got \
             {stored}"
        );
    }
}

#[tokio::test]
async fn the_retry_that_corrects_the_expression_creates_the_pipeline_under_that_name() {
    let router = emulator();
    let fixture = Fixture::given(&router).await;

    // The rows differ in whether the retry reuses the refused request's own client token:
    // the refused attempt filed no receipt either, so both reach the create.
    let rows = [
        ("the same client token", "retry-token", "retry-token"),
        ("a fresh client token", "first-token", "second-token"),
    ];

    for (index, (case, refused_token, retry_token)) in rows.into_iter().enumerate() {
        let name = format!("retried-pipeline-{index}");
        let (status, refused) = create_pipeline(
            &router,
            &fixture.create_body(
                &name,
                refused_token,
                json!({"schedule": {"scheduleExpression": "cron(not valid)"}}),
            ),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the invalid expression to be refused, got {status} with \
             {refused}"
        );

        let (status, created) = create_pipeline(
            &router,
            &fixture.create_body(
                &name,
                retry_token,
                json!({"schedule": {"scheduleExpression": "cron(0 3 ? * SUN *)"}}),
            ),
        )
        .await;
        let created = assert_ok(case, status, created);
        let arn = text(case, &created, "imagePipelineArn");
        assert_eq!(
            arn,
            pipeline_arn(&name),
            "case {case:?}: expected the retry to create {name}, got {arn:?}"
        );
        let echoed = text(case, &created, "clientToken");
        assert_eq!(
            echoed, retry_token,
            "case {case:?}: expected the retry's own client token, got {echoed:?}"
        );

        let (status, read) = get_pipeline(&router, &arn).await;
        let read = assert_ok(case, status, read);
        let expression = read
            .get("imagePipeline")
            .and_then(|pipeline| pipeline.get("schedule"))
            .and_then(|schedule| schedule.get("scheduleExpression"))
            .cloned();
        assert_eq!(
            expression,
            Some(json!("cron(0 3 ? * SUN *)")),
            "case {case:?}: expected the corrected expression to be stored, got {expression:?}"
        );
    }
}

#[tokio::test]
async fn an_update_carrying_an_unreadable_expression_replaces_nothing() {
    let router = emulator();
    let fixture = Fixture::given(&router).await;

    let case = "create the pipeline an update then fails to replace";
    let (status, created) = create_pipeline(
        &router,
        &fixture.create_body(
            "updated-pipeline",
            "create-updated",
            json!({
                "description": "as created",
                "schedule": {"scheduleExpression": "cron(0 0 * * ? *)"},
            }),
        ),
    )
    .await;
    let created = assert_ok(case, status, created);
    let arn = text(case, &created, "imagePipelineArn");

    for (index, row) in refused_expressions().iter().enumerate() {
        let case = row.case;
        let (status, refused) = update_pipeline(
            &router,
            &fixture.update_body(
                &arn,
                &format!("update-refused-{index}"),
                json!({
                    "description": "the replacement that was refused",
                    "schedule": {"scheduleExpression": row.expression},
                }),
            ),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?} ({:?}): expected HTTP 400 from the update, got {status} with \
             {refused}",
            row.expression
        );
        let code = error_code(case, &refused);
        assert_eq!(
            code, "InvalidParameterValueException",
            "case {case:?}: expected InvalidParameterValueException, got {code:?} with {refused}"
        );
        let expected = refusal_message(&row.detail);
        let message = error_message(case, &refused);
        assert_eq!(
            message, expected,
            "case {case:?} ({:?}): expected {expected:?}, got {message:?}",
            row.expression
        );

        // The stored pipeline is untouched: the configuration it was created with is what a
        // read still reports.
        let (status, read) = get_pipeline(&router, &arn).await;
        let read = assert_ok(case, status, read);
        let pipeline = read
            .get("imagePipeline")
            .cloned()
            .unwrap_or_else(|| panic!("case {case:?}: expected an imagePipeline in {read}"));
        assert_eq!(
            pipeline.get("description"),
            Some(&json!("as created")),
            "case {case:?}: expected the refused update to replace nothing, got {pipeline}"
        );
        assert_eq!(
            pipeline
                .get("schedule")
                .and_then(|schedule| schedule.get("scheduleExpression")),
            Some(&json!("cron(0 0 * * ? *)")),
            "case {case:?}: expected the created schedule to stand, got {pipeline}"
        );
    }
}

#[tokio::test]
async fn the_stored_schedule_carries_the_start_condition_the_pipeline_runs_under() {
    let router = emulator();
    let fixture = Fixture::given(&router).await;

    // (case, the schedule the create carried, the start condition the read reports) — the
    // value set of `PipelineExecutionStartCondition`
    // (`smithy_query(query="shape",shape="PipelineExecutionStartCondition")`) plus the
    // request that names none.
    let rows = [
        (
            "a schedule naming no start condition",
            json!({"scheduleExpression": "cron(0 3 ? * SUN *)"}),
            "EXPRESSION_MATCH_AND_DEPENDENCY_UPDATES_AVAILABLE",
        ),
        (
            "a schedule naming the expression-match condition",
            json!({
                "scheduleExpression": "cron(0 3 ? * SUN *)",
                "pipelineExecutionStartCondition": "EXPRESSION_MATCH_ONLY",
            }),
            "EXPRESSION_MATCH_ONLY",
        ),
        (
            "a schedule naming the dependency-updates condition",
            json!({
                "scheduleExpression": "rate(1 day)",
                "pipelineExecutionStartCondition":
                    "EXPRESSION_MATCH_AND_DEPENDENCY_UPDATES_AVAILABLE",
            }),
            "EXPRESSION_MATCH_AND_DEPENDENCY_UPDATES_AVAILABLE",
        ),
    ];

    for (index, (case, schedule, expected)) in rows.into_iter().enumerate() {
        let name = format!("conditioned-pipeline-{index}");
        let (status, created) = create_pipeline(
            &router,
            &fixture.create_body(
                &name,
                &format!("create-conditioned-{index}"),
                json!({"schedule": schedule}),
            ),
        )
        .await;
        let created = assert_ok(case, status, created);
        let arn = text(case, &created, "imagePipelineArn");

        let (status, read) = get_pipeline(&router, &arn).await;
        let read = assert_ok(case, status, read);
        let stored = read
            .get("imagePipeline")
            .and_then(|pipeline| pipeline.get("schedule"))
            .cloned()
            .unwrap_or_else(|| panic!("case {case:?}: expected a stored schedule in {read}"));
        assert_eq!(
            stored.get("pipelineExecutionStartCondition"),
            Some(&json!(expected)),
            "case {case:?}: expected the start condition {expected:?}, got {stored}"
        );

        // And the update stores it the same way: the replacement's schedule is read the same
        // way the create's is.
        let (status, updated) = update_pipeline(
            &router,
            &fixture.update_body(
                &arn,
                &format!("update-conditioned-{index}"),
                json!({"schedule": {"scheduleExpression": "cron(0 4 ? * MON *)"}}),
            ),
        )
        .await;
        let _ = assert_ok(case, status, updated);
        let (status, read) = get_pipeline(&router, &arn).await;
        let read = assert_ok(case, status, read);
        let replaced = read
            .get("imagePipeline")
            .and_then(|pipeline| pipeline.get("schedule"))
            .cloned()
            .unwrap_or_else(|| panic!("case {case:?}: expected a replaced schedule in {read}"));
        assert_eq!(
            replaced.get("pipelineExecutionStartCondition"),
            Some(&json!("EXPRESSION_MATCH_AND_DEPENDENCY_UPDATES_AVAILABLE")),
            "case {case:?}: expected the replacement to carry the default condition, got \
             {replaced}"
        );
    }
}

#[tokio::test]
async fn a_pipeline_reports_the_instant_of_its_last_write() {
    let router = emulator();
    let fixture = Fixture::given(&router).await;

    // The rows differ in whether the create named a schedule at all, so what is pinned is
    // the instant the record was written rather than one request's shape.
    let rows = [
        ("a pipeline created with a schedule", true),
        ("a pipeline created without one", false),
    ];

    for (index, (case, scheduled)) in rows.into_iter().enumerate() {
        let name = format!("written-pipeline-{index}");
        let extra = if scheduled {
            json!({"schedule": {"scheduleExpression": "cron(0 3 ? * SUN *)"}})
        } else {
            json!({})
        };
        let (status, created) = create_pipeline(
            &router,
            &fixture.create_body(&name, &format!("create-written-{index}"), extra),
        )
        .await;
        let created = assert_ok(case, status, created);
        let arn = text(case, &created, "imagePipelineArn");

        let (status, read) = get_pipeline(&router, &arn).await;
        let read = assert_ok(case, status, read);
        let pipeline = read
            .get("imagePipeline")
            .cloned()
            .unwrap_or_else(|| panic!("case {case:?}: expected an imagePipeline in {read}"));
        let date_created = pipeline.get("dateCreated").and_then(Value::as_str);
        assert!(
            date_created.is_some_and(|stamp| stamp.contains('T') && stamp.ends_with('Z')),
            "case {case:?}: expected an ISO 8601 dateCreated, got {date_created:?}"
        );
        assert_eq!(
            pipeline.get("dateUpdated").and_then(Value::as_str),
            date_created,
            "case {case:?}: expected the create's own instant as dateUpdated, got {pipeline}"
        );

        // An update is the next write, so the instant it reports is its own — at or after
        // the create's.
        let (status, updated) = update_pipeline(
            &router,
            &fixture.update_body(
                &arn,
                &format!("update-written-{index}"),
                json!({"description": "replaced"}),
            ),
        )
        .await;
        let _ = assert_ok(case, status, updated);
        let (status, read) = get_pipeline(&router, &arn).await;
        let read = assert_ok(case, status, read);
        let pipeline = read
            .get("imagePipeline")
            .cloned()
            .unwrap_or_else(|| panic!("case {case:?}: expected an imagePipeline in {read}"));
        let replaced_at = pipeline
            .get("dateUpdated")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("case {case:?}: expected a dateUpdated in {pipeline}"))
            .to_string();
        let created_at = date_created.unwrap_or_default().to_string();
        // Compared as INSTANTS, not as text, through the one reading of this model's
        // string `DateTime` members the sibling suite already states
        // ([`super::last_updated_projections::not_before`]): the renderer drops a
        // fraction's trailing zeros, so `…:50.88Z` and `…:50.88293Z` order one way as
        // chronology and the other way as strings.
        assert!(
            not_before(case, &replaced_at, &created_at),
            "case {case:?}: expected the update's instant {replaced_at:?} not to precede the \
             create's {created_at:?}"
        );
        assert_eq!(
            pipeline.get("dateCreated").and_then(Value::as_str),
            Some(created_at.as_str()),
            "case {case:?}: expected the create's instant to survive the update, got {pipeline}"
        );
    }
}

#[tokio::test]
async fn a_deleted_pipeline_is_reported_missing_and_its_name_is_free_again() {
    let router = emulator();
    let fixture = Fixture::given(&router).await;

    // The rows differ in the expression the pipeline was created with, so what is shown is
    // the lifecycle rather than one pipeline.
    for (index, (case, expression)) in [
        ("a cron schedule", "cron(0 3 ? * SUN *)"),
        ("a rate schedule", "rate(1 day)"),
    ]
    .into_iter()
    .enumerate()
    {
        let name = format!("recreated-pipeline-{index}");
        let (status, created) = create_pipeline(
            &router,
            &fixture.create_body(
                &name,
                &format!("create-recreated-{index}"),
                json!({"schedule": {"scheduleExpression": expression}}),
            ),
        )
        .await;
        let created = assert_ok(case, status, created);
        let arn = text(case, &created, "imagePipelineArn");

        let (status, deleted) = call(
            &router,
            "DELETE",
            &format!("/DeleteImagePipeline?imagePipelineArn={}", encode_label(&arn)),
            None,
        )
        .await;
        let deleted = assert_ok(case, status, deleted);
        assert_eq!(
            deleted.get("imagePipelineArn"),
            Some(&json!(arn)),
            "case {case:?}: expected the delete to echo the ARN it removed, got {deleted}"
        );

        let (status, missing) = get_pipeline(&router, &arn).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected a deleted pipeline to be reported missing, got {status} \
             with {missing}"
        );
        let message = error_message(case, &missing);
        let expected =
            format!("The following required resource 'ImagePipeline' cannot be found: '{arn}'.");
        assert_eq!(
            message, expected,
            "case {case:?}: expected {expected:?}, got {message:?}"
        );

        // And the name is free again: the same name creates a pipeline once more.
        let (status, recreated) = create_pipeline(
            &router,
            &fixture.create_body(
                &name,
                &format!("recreate-{index}"),
                json!({"schedule": {"scheduleExpression": expression}}),
            ),
        )
        .await;
        let recreated = assert_ok(case, status, recreated);
        assert_eq!(
            recreated.get("imagePipelineArn"),
            Some(&json!(arn)),
            "case {case:?}: expected the name to be free after the delete, got {recreated}"
        );
    }
}
