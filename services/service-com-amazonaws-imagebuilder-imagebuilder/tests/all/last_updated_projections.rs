// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the `dateUpdated` member every Image Builder read path
//! projects.
//!
//! Every case drives the real wire path — the production `MultiServiceRouter`, restJson1
//! serialization, the modelled-constraint walk and the handlers — so what is asserted is
//! what a signed AWS SDK request observes.
//!
//! The rule under test: a read path serves the stored resource's projection, so every
//! modelled member the stored record holds is reported — `dateUpdated` carries the instant
//! of the write that last stored that record, and every projection of ONE resource (the
//! single-resource read and each entry of a listing) reports the same value, which a read
//! never moves.
//!
//! `ImagePipeline.dateUpdated` is "the date on which this image pipeline was last updated"
//! and `DistributionConfiguration.dateUpdated` is "the date on which this distribution
//! configuration was last updated"
//! (`smithy_query(query="shape",shape="ImagePipeline,DistributionConfiguration")`), while
//! `ImagePipeline.consecutiveFailures` is the count Image Builder tracks for SCHEDULED
//! executions, so it is reported only once one has been tracked.
//!
//! The rows span what the rule generalises over: which resource kind is read, which read
//! path serves it (one resource or a listing entry), which optional members the stored
//! record carries, how many writes have replaced it, and — for a replacement list — how
//! many entries it holds.

use std::sync::Arc;

use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The Region every call here is signed for.
const REGION: &str = "us-west-2";

/// A build component document: one `build` phase, which is all the recipe these cases
/// name needs.
const BUILD_DOCUMENT: &str = "name: HelloWorld\nschemaVersion: 1.0\nphases:\n  - name: build\n    \
                              steps:\n      - name: Greet\n        action: ExecuteBash\n        \
                              inputs:\n          commands:\n            - echo hello\n";

/// One emulator with an empty store, mounted exactly as the router mounts it.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_imagebuilder_imagebuilder::EmulatedService::new(engine),
    );
    router
}

/// A request signed for `imagebuilder` in [`REGION`].
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

/// Status and parsed body of one call.
async fn call(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<&Value>,
) -> (u16, Value) {
    let response = router.dispatch(request(method, uri, body)).await;
    let status = response.status().as_u16();
    let raw = response.into_body();
    let parsed: Value = if raw.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&raw)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&raw).to_string()))
    };
    (status, parsed)
}

/// Assert a call succeeded, and answer its body.
fn assert_ok(case: &str, status: u16, body: Value) -> Value {
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with {body}"
    );
    body
}

/// The text one member of a response carries.
fn text(case: &str, body: &Value, name: &str) -> String {
    body.get(name)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: expected text in {name} of {body}"))
        .to_string()
}

/// The instant one member of a projection reports, asserted to be an ISO 8601 UTC instant
/// the way this model's string `DateTime` members carry one (`2026-10-24T01:51:48Z`).
fn instant(case: &str, view: &Value, member: &str) -> String {
    let reported = view.get(member).and_then(Value::as_str);
    assert!(
        reported.is_some_and(|stamp| {
            stamp.len() >= 20 && stamp.contains('T') && stamp.ends_with('Z')
        }),
        "case {case:?}: expected {member} to be an ISO 8601 UTC instant, got {reported:?} in \
         {view}"
    );
    reported.unwrap_or_default().to_string()
}

/// Whether `later` is the same instant as `earlier` or a later one.
///
/// The two are compared as INSTANTS and not as text: an ISO 8601 instant renders its
/// fraction without trailing zeros, so `…:55.44Z` and `…:55.442956Z` order one way as
/// chronology and the other way as strings. Parsed through the tree's own reading of this
/// model's `date-time` form (`aws_smithy_types::DateTime`) rather than through a
/// hand-rolled split.
///
/// `pub(crate)` because every suite that orders two instants this model reports takes this
/// one reading of them, rather than keeping a second copy of it.
pub(crate) fn not_before(case: &str, later: &str, earlier: &str) -> bool {
    let parse = |stamp: &str| {
        ::aws_smithy_types::DateTime::from_str(
            stamp,
            ::aws_smithy_types::date_time::Format::DateTime,
        )
        .unwrap_or_else(|error| {
            panic!("case {case:?}: expected an ISO 8601 instant, got {stamp:?} ({error})")
        })
    };
    let (later, earlier) = (parse(later), parse(earlier));
    (later.secs(), later.subsec_nanos()) >= (earlier.secs(), earlier.subsec_nanos())
}

/// Merge the members of `extra` into `body`.
fn merge(body: &mut Map<String, Value>, extra: Value) {
    if let Value::Object(extra) = extra {
        for (member, value) in extra {
            body.insert(member, value);
        }
    }
}

/// `CreateComponent` for one build version of `name`, answering its build version ARN.
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

/// `CreateImageRecipe` for `name`, answering the recipe's ARN.
async fn create_recipe(router: &MultiServiceRouter, name: &str, component_arn: &str) -> String {
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

/// `CreateInfrastructureConfiguration` for `name`, answering its ARN.
async fn create_infrastructure(router: &MultiServiceRouter, name: &str) -> String {
    let body = json!({
        "name": name,
        "clientToken": format!("infrastructure-{name}"),
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

/// `CreateImagePipeline` for `name`, over whatever `extra` names beside the recipe and the
/// infrastructure configuration, answering the pipeline's ARN.
async fn create_pipeline(
    router: &MultiServiceRouter,
    name: &str,
    recipe: &str,
    infrastructure: &str,
    extra: Value,
) -> String {
    let mut body = Map::new();
    body.insert("name".to_string(), json!(name));
    body.insert("clientToken".to_string(), json!(format!("pipeline-{name}")));
    body.insert("imageRecipeArn".to_string(), json!(recipe));
    body.insert(
        "infrastructureConfigurationArn".to_string(),
        json!(infrastructure),
    );
    merge(&mut body, extra);
    let case = format!("create the pipeline {name}");
    let (status, answer) = call(
        router,
        "PUT",
        "/CreateImagePipeline",
        Some(&Value::Object(body)),
    )
    .await;
    let answer = assert_ok(&case, status, answer);
    text(&case, &answer, "imagePipelineArn")
}

/// `UpdateImagePipeline` for `arn`, carrying `extra` beside the members an update must
/// name: the pipeline, the infrastructure configuration and the recipe it keeps.
async fn update_pipeline(
    router: &MultiServiceRouter,
    case: &str,
    arn: &str,
    client_token: &str,
    infrastructure: &str,
    recipe: &str,
    extra: Value,
) {
    let mut body = Map::new();
    body.insert("imagePipelineArn".to_string(), json!(arn));
    body.insert("clientToken".to_string(), json!(client_token));
    body.insert(
        "infrastructureConfigurationArn".to_string(),
        json!(infrastructure),
    );
    body.insert("imageRecipeArn".to_string(), json!(recipe));
    merge(&mut body, extra);
    let (status, answer) = call(
        router,
        "PUT",
        "/UpdateImagePipeline",
        Some(&Value::Object(body)),
    )
    .await;
    assert_ok(case, status, answer);
}

/// The `imagePipeline` view `GetImagePipeline` reports for `arn`.
async fn read_pipeline(router: &MultiServiceRouter, case: &str, arn: &str) -> Value {
    let (status, answer) = call(
        router,
        "GET",
        &format!("/GetImagePipeline?imagePipelineArn={arn}"),
        None,
    )
    .await;
    let answer = assert_ok(case, status, answer);
    answer
        .get("imagePipeline")
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: expected an imagePipeline in {answer}"))
}

/// Every entry `ListImagePipelines` reports, keyed by the pipeline's own ARN.
async fn listed_pipelines(router: &MultiServiceRouter, case: &str) -> Vec<(String, Value)> {
    let (status, answer) = call(router, "POST", "/ListImagePipelines", Some(&json!({}))).await;
    let answer = assert_ok(case, status, answer);
    answer
        .get("imagePipelineList")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: expected an imagePipelineList in {answer}"))
        .iter()
        .map(|entry| (text(case, entry, "arn"), entry.clone()))
        .collect()
}

/// One `distributions` entry naming `region` as its target, whose AMI settings name
/// `ami_name` when it names one.
fn distribution_entry(region: &str, ami_name: Option<&str>) -> Value {
    let mut settings = Map::new();
    if let Some(name) = ami_name {
        settings.insert("name".to_string(), json!(name));
    }
    json!({ "region": region, "amiDistributionConfiguration": Value::Object(settings) })
}

/// `CreateDistributionConfiguration` for `name`, with one target Region, answering its ARN.
async fn create_distribution(router: &MultiServiceRouter, name: &str) -> String {
    let body = json!({
        "name": name,
        "clientToken": format!("distribution-{name}"),
        "distributions": [distribution_entry(REGION, Some("golden-{{ imagebuilder:buildDate }}"))],
    });
    let case = format!("create the distribution configuration {name}");
    let (status, answer) = call(
        router,
        "PUT",
        "/CreateDistributionConfiguration",
        Some(&body),
    )
    .await;
    let answer = assert_ok(&case, status, answer);
    text(&case, &answer, "distributionConfigurationArn")
}

/// `UpdateDistributionConfiguration` for `arn`, replacing its list with `distributions`.
async fn update_distribution(
    router: &MultiServiceRouter,
    case: &str,
    arn: &str,
    client_token: &str,
    distributions: &Value,
) {
    let body = json!({
        "distributionConfigurationArn": arn,
        "clientToken": client_token,
        "distributions": distributions,
    });
    let (status, answer) = call(
        router,
        "PUT",
        "/UpdateDistributionConfiguration",
        Some(&body),
    )
    .await;
    assert_ok(case, status, answer);
}

/// The `distributionConfiguration` view `GetDistributionConfiguration` reports for `arn`.
async fn read_distribution(router: &MultiServiceRouter, case: &str, arn: &str) -> Value {
    let (status, answer) = call(
        router,
        "GET",
        &format!("/GetDistributionConfiguration?distributionConfigurationArn={arn}"),
        None,
    )
    .await;
    let answer = assert_ok(case, status, answer);
    answer
        .get("distributionConfiguration")
        .cloned()
        .unwrap_or_else(|| {
            panic!("case {case:?}: expected a distributionConfiguration in {answer}")
        })
}

/// RULE: a pipeline's stored record holds the instant of its last write from the create
/// onwards, so both read paths — the single-resource read and every listing entry — report
/// it as `dateUpdated`, and report the same projection.
///
/// The rows differ along what the rule generalises over: which optional members the stored
/// pipeline carries.
#[tokio::test]
async fn every_pipeline_read_path_reports_the_stored_last_write_instant() {
    let router = emulator();
    let component = create_component(&router, "last-updated-component").await;
    let recipe = create_recipe(&router, "last-updated-recipe", &component).await;
    let infrastructure = create_infrastructure(&router, "last-updated-infrastructure").await;

    // (case, the create's optional members)
    let rows: Vec<(&str, Value)> = vec![
        ("a pipeline carrying nothing optional", json!({})),
        (
            "a pipeline carrying a description",
            json!({"description": "the pipeline as created"}),
        ),
        (
            "a pipeline carrying a cron schedule",
            json!({"schedule": {"scheduleExpression": "cron(0 3 ? * SUN *)"}}),
        ),
        (
            "a pipeline carrying a rate schedule and a status",
            json!({
                "schedule": {"scheduleExpression": "rate(1 day)"},
                "status": "DISABLED",
            }),
        ),
        (
            "a pipeline carrying tags and image tags",
            json!({"tags": {"owner": "platform"}, "imageTags": {"tier": "golden"}}),
        ),
    ];

    let mut created: Vec<(&str, String)> = Vec::new();
    for (index, (case, extra)) in rows.into_iter().enumerate() {
        let name = format!("last-updated-pipeline-{index}");
        let arn = create_pipeline(&router, &name, &recipe, &infrastructure, extra).await;
        created.push((case, arn));
    }

    let listed = listed_pipelines(&router, "the whole pipeline listing").await;
    assert_eq!(
        listed.len(),
        created.len(),
        "case \"the whole pipeline listing\": expected {} entries, got {}",
        created.len(),
        listed.len()
    );

    for (case, arn) in &created {
        let view = read_pipeline(&router, case, arn).await;

        // Nothing has replaced the record since the create, so the create IS its last
        // write and both date members report that one instant.
        let created_at = instant(case, &view, "dateCreated");
        let updated_at = instant(case, &view, "dateUpdated");
        assert_eq!(
            updated_at, created_at,
            "case {case:?}: expected dateUpdated to be the create's own instant \
             {created_at:?}, got {updated_at:?}"
        );

        // No scheduled execution has been tracked for this pipeline, so there is no
        // consecutive-failure count to report.
        assert!(
            view.get("consecutiveFailures").is_none(),
            "case {case:?}: expected no consecutiveFailures before any scheduled execution, \
             got {view}"
        );

        // A read moves nothing: the next caller sees the same instant.
        let again = read_pipeline(&router, case, arn).await;
        assert_eq!(
            again, view,
            "case {case:?}: expected a second read to report the same projection, got {again} \
             after {view}"
        );

        // The listing serves the same projection, so its entry carries the same instant.
        let entry = listed
            .iter()
            .find(|(listed_arn, _)| listed_arn == arn)
            .map(|(_, entry)| entry.clone())
            .unwrap_or_else(|| panic!("case {case:?}: expected {arn} in the listing {listed:?}"));
        let listed_at = instant(case, &entry, "dateUpdated");
        assert_eq!(
            listed_at, updated_at,
            "case {case:?}: expected the listed dateUpdated {updated_at:?}, got {listed_at:?}"
        );
        assert_eq!(
            entry, view,
            "case {case:?}: expected the listing entry to be the read's view, got {entry} and \
             {view}"
        );
    }
}

/// RULE: a write replaces the stored record, so `dateUpdated` moves to the instant of that
/// write and never goes backwards — on every read path at once.
///
/// The rows differ along which setting the update changes, and how many updates have run.
#[tokio::test]
async fn an_update_restamps_the_pipeline_on_every_read_path() {
    let router = emulator();
    let component = create_component(&router, "restamped-component").await;
    let recipe = create_recipe(&router, "restamped-recipe", &component).await;
    let infrastructure = create_infrastructure(&router, "restamped-infrastructure").await;

    // (case, the update's optional members)
    let rows: Vec<(&str, Value)> = vec![
        (
            "an update naming a description",
            json!({"description": "the pipeline as updated"}),
        ),
        ("an update disabling the pipeline", json!({"status": "DISABLED"})),
        (
            "an update re-settling the test configuration",
            json!({"imageTestsConfiguration": {"timeoutMinutes": 60}}),
        ),
        (
            "an update carrying nothing but the required members",
            json!({}),
        ),
    ];

    for (index, (case, extra)) in rows.into_iter().enumerate() {
        let name = format!("restamped-pipeline-{index}");
        let arn = create_pipeline(
            &router,
            &name,
            &recipe,
            &infrastructure,
            json!({"description": "the pipeline as created"}),
        )
        .await;

        let before = read_pipeline(&router, case, &arn).await;
        let created_at = instant(case, &before, "dateCreated");
        let first_stamp = instant(case, &before, "dateUpdated");

        // Two writes in a row: each one is the record's last write, so each is the instant
        // every read path reports, and the instant never precedes the earlier one.
        let mut stamps = vec![first_stamp];
        for write in 0..2 {
            update_pipeline(
                &router,
                case,
                &arn,
                &format!("restamp-{index}-{write}"),
                &infrastructure,
                &recipe,
                extra.clone(),
            )
            .await;

            let after = read_pipeline(&router, case, &arn).await;
            let updated_at = instant(case, &after, "dateUpdated");
            assert_eq!(
                after.get("dateCreated").and_then(Value::as_str),
                Some(created_at.as_str()),
                "case {case:?}: expected dateCreated to stay {created_at:?}, got {after}"
            );
            let previous = stamps.last().cloned().unwrap_or_default();
            assert!(
                not_before(case, &updated_at, &previous),
                "case {case:?}: expected the later dateUpdated {updated_at:?} not to precede \
                 the earlier {previous:?}"
            );

            let listed = listed_pipelines(&router, case).await;
            let entry = listed
                .iter()
                .find(|(listed_arn, _)| *listed_arn == arn)
                .map(|(_, entry)| entry.clone())
                .unwrap_or_else(|| {
                    panic!("case {case:?}: expected {arn} in the listing {listed:?}")
                });
            assert_eq!(
                entry, after,
                "case {case:?}: expected the listing entry to be the read's view, got {entry} \
                 and {after}"
            );
            stamps.push(updated_at);
        }
    }
}

/// RULE: a distribution configuration's read reports the list and the instant its last
/// update stored — the whole replacement list, and `dateUpdated` as that update's instant —
/// while a configuration no update has replaced reports no such instant.
///
/// The rows differ along how many entries the replacement list holds and whether it adds a
/// target Region, drops one or only re-settles the created one.
#[tokio::test]
async fn a_distribution_configuration_reports_the_instant_of_its_last_update() {
    // (case, the replacement list)
    let rows: Vec<(&str, Value)> = vec![
        (
            "a second target Region added",
            json!([
                distribution_entry(REGION, Some("golden-{{ imagebuilder:buildDate }}")),
                distribution_entry("us-east-1", Some("second-{{ imagebuilder:buildDate }}")),
            ]),
        ),
        (
            "three target Regions",
            json!([
                distribution_entry("eu-west-1", None),
                distribution_entry("ap-south-1", Some("{{ imagebuilder:buildDate }}-app")),
                distribution_entry(REGION, Some("golden-{{ imagebuilder:buildDate }}")),
            ]),
        ),
        (
            "one target Region, somewhere else entirely",
            json!([distribution_entry("me-central-1", None)]),
        ),
        (
            "the created target, re-settled",
            json!([distribution_entry(
                REGION,
                Some("renamed-{{ imagebuilder:buildDate }}")
            )]),
        ),
    ];

    for (index, (case, replacement)) in rows.into_iter().enumerate() {
        let router = emulator();
        let name = format!("last-updated-distribution-{index}");
        let arn = create_distribution(&router, &name).await;

        let before = read_distribution(&router, case, &arn).await;
        let created_at = instant(case, &before, "dateCreated");
        assert!(
            before.get("dateUpdated").is_none(),
            "case {case:?}: expected no dateUpdated before any update, got {before}"
        );

        update_distribution(
            &router,
            case,
            &arn,
            &format!("last-updated-{index}"),
            &replacement,
        )
        .await;

        let after = read_distribution(&router, case, &arn).await;
        assert_eq!(
            after.get("distributions"),
            Some(&replacement),
            "case {case:?}: expected the replacement list {replacement}, got {after}"
        );
        let updated_at = instant(case, &after, "dateUpdated");
        assert!(
            not_before(case, &updated_at, &created_at),
            "case {case:?}: expected dateUpdated {updated_at:?} not to precede dateCreated \
             {created_at:?}"
        );

        // A read moves nothing: the next caller sees the same instant and the same list.
        let again = read_distribution(&router, case, &arn).await;
        assert_eq!(
            again, after,
            "case {case:?}: expected a second read to report the same projection, got {again} \
             after {after}"
        );

        // A second update is the configuration's last write, so its instant is the one
        // reported afterwards, and it never precedes the first update's.
        let second = json!([distribution_entry(REGION, None)]);
        update_distribution(
            &router,
            case,
            &arn,
            &format!("last-updated-{index}-again"),
            &second,
        )
        .await;
        let latest = read_distribution(&router, case, &arn).await;
        assert_eq!(
            latest.get("distributions"),
            Some(&second),
            "case {case:?}: expected the second replacement list {second}, got {latest}"
        );
        let latest_at = instant(case, &latest, "dateUpdated");
        assert!(
            not_before(case, &latest_at, &updated_at),
            "case {case:?}: expected the later dateUpdated {latest_at:?} not to precede the \
             earlier {updated_at:?}"
        );
    }
}
