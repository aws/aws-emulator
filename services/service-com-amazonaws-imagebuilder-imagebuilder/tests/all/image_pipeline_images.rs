// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `ListImagePipelineImages`: the images ONE pipeline's runs
//! created.
//!
//! Every case drives the real wire path — the production `MultiServiceRouter`, restJson1
//! serialization, the modelled-constraint walk and the handlers — so what is asserted is what
//! a signed AWS SDK request observes.
//!
//! The rules under test:
//!
//! * the listing reports one entry per stored image build version whose build a run of the
//!   pipeline the request names queued, and none a run of any other pipeline queued, however
//!   many pipelines share the recipe those builds came from; a pipeline nothing has started
//!   reports an empty list;
//! * an entry is the SAME summary the account-wide build-version listing reports for that
//!   build version, so two listings of one build cannot disagree about it;
//! * `filters` selects on the members this operation documents — name and version
//!   (`smithy_query(query="doc",shape="ListImagePipelineImagesRequest",member="filters")`) —
//!   case-sensitively, every filter of the list having to admit an entry for it to be
//!   reported; a filter naming any other member is refused as a value the `filters` parameter
//!   does not admit, and one that left out the member it selects on or the values it admits is
//!   reported as the missing parameter it is;
//! * one page carries at most `maxResults` entries, and at most the documented default when
//!   the request names none; a `nextToken` is answered only while entries remain, and the token is
//!   issued FOR that one listing, so a token of another pipeline's listing is refused;
//! * a pipeline this Region holds none of is reported missing, as the required resource it is.
//!
//! The rows below span what those rules generalise over: how many runs a pipeline has had and
//! how many pipelines share one recipe, which member a filter names and in which letter case,
//! how many filters one request carries, how large a page is asked for, and how a pipeline ARN
//! resolves to nothing (never created, deleted, another Region, another account).
//!
//! The harness is the sibling suite [`super::image_pipeline_schedule`]'s: one emulator, one
//! signed call and one reader of an answer, borrowed rather than kept a second time.

use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::image_pipeline_schedule::{
    assert_ok, call, emulator, error_code, error_message, pipeline_arn, text,
};

/// The Region every case signs for; the ARNs it creates carry it.
const REGION: &str = "us-west-2";

/// The canonical emulator account (`engine::EMULATOR_ACCOUNT_ID`) every ARN carries.
const ACCOUNT: &str = "000000000000";

/// A build component document: one `build` phase, which is all a recipe these cases reference
/// needs.
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

/// The error this service refuses a parameter value with
/// (`smithy_query(query="shape",shape="InvalidParameterValueException")`: a client error,
/// `@httpError` 400).
const INVALID_PARAMETER_VALUE: &str = "InvalidParameterValueException";

/// The class a filter that left out a member every filter has to carry is reported under.
const MISSING_PARAMETERS: &str = "MissingParametersException";

/// The error a call naming a resource this Region holds nothing under is answered with
/// (`smithy_query(query="operation",operation="ListImagePipelineImages")`: `@httpError` 404).
const RESOURCE_NOT_FOUND: &str = "ResourceNotFoundException";

/// The requirement the refusal of a filter naming a member this listing does not filter on
/// states: the names this operation admits, in the order its `filters` member documents them.
const ALLOWED_FILTERS: &str =
    "Allowed filters for ListImagePipelineImages are: name, version.";

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

/// `CreateImageRecipe` at `version`, including `component_arn`, answering the recipe's ARN.
async fn create_image_recipe(
    router: &MultiServiceRouter,
    name: &str,
    version: &str,
    component_arn: &str,
) -> String {
    let body = json!({
        "name": name,
        "semanticVersion": version,
        "parentImage": "ami-0123456789abcdef0",
        "clientToken": format!("recipe-{name}-{version}"),
        "components": [{ "componentArn": component_arn }],
    });
    let case = format!("create the image recipe {name} {version}");
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

/// `CreateImagePipeline` for `name` over `recipe`, answering the pipeline's own ARN.
async fn create_pipeline(
    router: &MultiServiceRouter,
    name: &str,
    recipe: &str,
    configuration: &str,
) -> String {
    let body = json!({
        "name": name,
        "clientToken": format!("pipeline-{name}"),
        "imageRecipeArn": recipe,
        "infrastructureConfigurationArn": configuration,
    });
    let case = format!("create the pipeline {name}");
    let (status, answer) = call(router, "PUT", "/CreateImagePipeline", Some(&body)).await;
    let answer = assert_ok(&case, status, answer);
    text(&case, &answer, "imagePipelineArn")
}

/// `StartImagePipelineExecution` for `arn`, answering the build version ARN it queued.
async fn start_pipeline(router: &MultiServiceRouter, arn: &str, token: &str) -> String {
    let body = json!({ "imagePipelineArn": arn, "clientToken": token });
    let case = format!("start the pipeline {arn} as {token}");
    let (status, answer) = call(
        router,
        "PUT",
        "/StartImagePipelineExecution",
        Some(&body),
    )
    .await;
    let answer = assert_ok(&case, status, answer);
    text(&case, &answer, "imageBuildVersionArn")
}

/// `DeleteImagePipeline` for `arn`.
async fn delete_pipeline(router: &MultiServiceRouter, arn: &str) {
    let case = format!("delete the pipeline {arn}");
    let (status, answer) = call(
        router,
        "DELETE",
        &format!(
            "/DeleteImagePipeline?imagePipelineArn={}",
            percent_encoding::utf8_percent_encode(arn, percent_encoding::NON_ALPHANUMERIC)
        ),
        None,
    )
    .await;
    assert_ok(&case, status, answer);
}

/// One `ListImagePipelineImages` request body: the pipeline, and whatever `extra` adds.
fn listing_body(arn: &str, extra: Value) -> Value {
    let mut body = Map::new();
    body.insert("imagePipelineArn".to_string(), json!(arn));
    if let Value::Object(extra) = extra {
        for (member, value) in extra {
            body.insert(member, value);
        }
    }
    Value::Object(body)
}

/// `ListImagePipelineImages` with `body`.
async fn list_pipeline_images(router: &MultiServiceRouter, body: &Value) -> (u16, Value) {
    call(router, "POST", "/ListImagePipelineImages", Some(body)).await
}

/// `ListImageBuildVersions` over every build version of the account.
async fn list_build_versions(router: &MultiServiceRouter) -> Value {
    let (status, answer) = call(
        router,
        "POST",
        "/ListImageBuildVersions",
        Some(&json!({})),
    )
    .await;
    assert_ok("list every build version", status, answer)
}

/// The `imageSummaryList` of one answer, in the order it carried it.
fn summaries(case: &str, body: &Value) -> Vec<Value> {
    body.get("imageSummaryList")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: expected an imageSummaryList in {body}"))
        .clone()
}

/// The `arn` of every entry of one answer, in the order it carried them.
fn listed_arns(case: &str, body: &Value) -> Vec<String> {
    summaries(case, body)
        .iter()
        .map(|entry| text(case, entry, "arn"))
        .collect()
}

/// One entry with its `state` dropped: the status a build reports is read from the CLOCK
/// against the build's own span, so two answers read at two instants may report two statuses
/// of the same build and everything else they report has to agree.
fn without_state(entry: &Value) -> Value {
    let mut entry = entry.clone();
    if let Value::Object(members) = &mut entry {
        members.remove("state");
    }
    entry
}

/// The ARN an image build version of `name` at `version` carries as its `build`th build in
/// [`REGION`].
fn build_version_arn(name: &str, version: &str, build: u64) -> String {
    format!("arn:aws:imagebuilder:{REGION}:{ACCOUNT}:image/{name}/{version}/{build}")
}

/// The pipelines, the recipes they build and the runs they have had: what every case here
/// reads.
struct Fixture {
    /// The pipeline that has run twice, over [`Fixture::ALPHA`].
    twice_run: String,
    /// A second pipeline over the SAME recipe, which has run once.
    shared_recipe: String,
    /// A pipeline over another recipe, which has run once.
    other_recipe: String,
    /// A pipeline over [`Fixture::ALPHA`] that nothing has started.
    never_run: String,
}

impl Fixture {
    /// The image name the first recipe gives the builds of it, and its version.
    const ALPHA: (&'static str, &'static str) = ("pipeline-images-alpha", "1.0.0");

    /// The image name the second recipe gives the builds of it, and its version.
    const BETA: (&'static str, &'static str) = ("pipeline-images-beta", "2.3.4");

    /// Create the recipes and pipelines, and start the runs, in a fixed order, so the build
    /// number each run earned is the one a case names.
    async fn given(router: &MultiServiceRouter) -> Self {
        let component = create_component(router, "pipeline-images-component").await;
        let configuration = create_configuration(router, "pipeline-images-infra").await;
        let alpha =
            create_image_recipe(router, Self::ALPHA.0, Self::ALPHA.1, &component).await;
        let beta = create_image_recipe(router, Self::BETA.0, Self::BETA.1, &component).await;

        let twice_run =
            create_pipeline(router, "pipeline-images-first", &alpha, &configuration).await;
        let shared_recipe =
            create_pipeline(router, "pipeline-images-second", &alpha, &configuration).await;
        let other_recipe =
            create_pipeline(router, "pipeline-images-third", &beta, &configuration).await;
        let never_run =
            create_pipeline(router, "pipeline-images-idle", &alpha, &configuration).await;

        // The builds of one name+version pair count up across the pipelines that queue them,
        // so the order these runs are started in is the order of their build numbers.
        start_pipeline(router, &twice_run, "run-first-1").await;
        start_pipeline(router, &twice_run, "run-first-2").await;
        start_pipeline(router, &shared_recipe, "run-second-1").await;
        start_pipeline(router, &other_recipe, "run-third-1").await;

        Self {
            twice_run,
            shared_recipe,
            other_recipe,
            never_run,
        }
    }
}

/// RULE: the listing reports the build versions the named pipeline's runs created, and only
/// those, however many pipelines share the recipe those builds came from.
#[tokio::test]
async fn the_listing_reports_the_builds_that_pipeline_started() {
    let router = emulator();
    let fixture = Fixture::given(&router).await;

    let (alpha, alpha_version) = Fixture::ALPHA;
    let (beta, beta_version) = Fixture::BETA;

    // (case, the pipeline asked about, the build versions its own runs created).
    let rows: Vec<(&str, &String, Vec<String>)> = vec![
        (
            "the pipeline that has run twice",
            &fixture.twice_run,
            vec![
                build_version_arn(alpha, alpha_version, 1),
                build_version_arn(alpha, alpha_version, 2),
            ],
        ),
        (
            "a second pipeline over the same recipe",
            &fixture.shared_recipe,
            vec![build_version_arn(alpha, alpha_version, 3)],
        ),
        (
            "a pipeline over another recipe",
            &fixture.other_recipe,
            vec![build_version_arn(beta, beta_version, 1)],
        ),
        (
            "a pipeline nothing has started",
            &fixture.never_run,
            Vec::new(),
        ),
    ];

    for (case, pipeline, expected) in rows {
        let (status, answer) =
            list_pipeline_images(&router, &listing_body(pipeline, json!({}))).await;
        let answer = assert_ok(case, status, answer);
        let actual = listed_arns(case, &answer);
        assert_eq!(
            actual, expected,
            "case {case:?}: expected the build versions {expected:?}, got {actual:?}"
        );
        assert!(
            answer.get("nextToken").is_none_or(Value::is_null),
            "case {case:?}: expected no nextToken on a complete page, got {answer}"
        );
        assert!(
            answer
                .get("requestId")
                .and_then(Value::as_str)
                .is_some_and(|id| !id.is_empty()),
            "case {case:?}: expected a requestId in {answer}"
        );
    }
}

/// RULE: an entry of a pipeline's listing is the same summary the account-wide build-version
/// listing reports for that build version.
#[tokio::test]
async fn an_entry_agrees_with_the_build_version_listing() {
    let router = emulator();
    let fixture = Fixture::given(&router).await;

    let case = "every build version of the account";
    let account_wide = list_build_versions(&router).await;
    let account_entries = summaries(case, &account_wide);

    for pipeline in [
        &fixture.twice_run,
        &fixture.shared_recipe,
        &fixture.other_recipe,
    ] {
        let case = format!("the images of {pipeline}");
        let (status, answer) =
            list_pipeline_images(&router, &listing_body(pipeline, json!({}))).await;
        let answer = assert_ok(&case, status, answer);
        for entry in summaries(&case, &answer) {
            let arn = text(&case, &entry, "arn");
            let expected = account_entries
                .iter()
                .find(|other| other.get("arn") == entry.get("arn"))
                .unwrap_or_else(|| {
                    panic!("case {case:?}: expected {arn} among {account_entries:?}")
                });
            assert_eq!(
                without_state(&entry),
                without_state(expected),
                "case {case:?}: expected the entry for {arn} to agree with the build-version \
                 listing, got {entry}"
            );
            // Both listings report a modelled status for the build, whichever instant each
            // was read at.
            let status = entry
                .get("state")
                .and_then(|state| state.get("status"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            assert!(
                ["PENDING", "BUILDING", "TESTING", "AVAILABLE"].contains(&status.as_str()),
                "case {case:?}: expected a build status for {arn}, got {status:?}"
            );
        }
    }
}

/// RULE: every filter the request carries has to admit an entry for it to be reported, and a
/// filter selects case-sensitively on the member it names.
#[tokio::test]
async fn filters_select_on_the_members_this_listing_documents() {
    let router = emulator();
    let fixture = Fixture::given(&router).await;

    let (alpha, alpha_version) = Fixture::ALPHA;
    let (beta, beta_version) = Fixture::BETA;
    let first = build_version_arn(alpha, alpha_version, 1);
    let second = build_version_arn(alpha, alpha_version, 2);

    // (case, the pipeline asked about, the filters, the build versions they admit).
    let rows: Vec<(&str, &String, Value, Vec<String>)> = vec![
        (
            "the name the recipe gave the builds",
            &fixture.twice_run,
            json!([{ "name": "name", "values": [alpha] }]),
            vec![first.clone(), second.clone()],
        ),
        (
            "another pipeline's image name",
            &fixture.twice_run,
            json!([{ "name": "name", "values": [beta] }]),
            Vec::new(),
        ),
        (
            "two values, one of which names these builds",
            &fixture.twice_run,
            json!([{ "name": "name", "values": [beta, alpha] }]),
            vec![first.clone(), second.clone()],
        ),
        (
            "the name in another letter case",
            &fixture.twice_run,
            json!([{ "name": "name", "values": ["Pipeline-Images-Alpha"] }]),
            Vec::new(),
        ),
        (
            "the version these builds carry",
            &fixture.twice_run,
            json!([{ "name": "version", "values": [alpha_version] }]),
            vec![first.clone(), second.clone()],
        ),
        (
            "a version nothing was built at",
            &fixture.twice_run,
            json!([{ "name": "version", "values": ["9.9.9"] }]),
            Vec::new(),
        ),
        (
            "the name and the version together",
            &fixture.twice_run,
            json!([
                { "name": "name", "values": [alpha] },
                { "name": "version", "values": [alpha_version] },
            ]),
            vec![first, second],
        ),
        (
            "the name together with a version nothing was built at",
            &fixture.twice_run,
            json!([
                { "name": "name", "values": [alpha] },
                { "name": "version", "values": ["9.9.9"] },
            ]),
            Vec::new(),
        ),
        (
            "the other recipe's name and version, on its own pipeline",
            &fixture.other_recipe,
            json!([
                { "name": "name", "values": [beta] },
                { "name": "version", "values": [beta_version] },
            ]),
            vec![build_version_arn(beta, beta_version, 1)],
        ),
        (
            "the first recipe's name, on the other recipe's pipeline",
            &fixture.other_recipe,
            json!([{ "name": "name", "values": [alpha] }]),
            Vec::new(),
        ),
    ];

    for (case, pipeline, filters, expected) in rows {
        let body = listing_body(pipeline, json!({ "filters": filters }));
        let (status, answer) = list_pipeline_images(&router, &body).await;
        let answer = assert_ok(case, status, answer);
        let actual = listed_arns(case, &answer);
        assert_eq!(
            actual, expected,
            "case {case:?}: expected {expected:?}, got {actual:?}"
        );
    }
}

/// RULE: one page carries at most `maxResults` entries, a `nextToken` is answered only while
/// entries remain, and walking the tokens reports every entry exactly once.
#[tokio::test]
async fn a_page_carries_at_most_the_size_the_request_asked_for() {
    let router = emulator();
    let fixture = Fixture::given(&router).await;
    // A third run of the pipeline, so the listing has more entries than the smallest page.
    start_pipeline(&router, &fixture.twice_run, "run-first-3").await;

    let (alpha, alpha_version) = Fixture::ALPHA;
    let complete = vec![
        build_version_arn(alpha, alpha_version, 1),
        build_version_arn(alpha, alpha_version, 2),
        build_version_arn(alpha, alpha_version, 4),
    ];

    // The page sizes a row asks for: below the listing's size, at it, past it, and the
    // modelled maximum (`maxResults` range 1..100).
    for page_size in [1_u32, 2, 3, 4, 100] {
        let case = format!("pages of {page_size}");
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0_usize;
        loop {
            let mut extra = Map::new();
            extra.insert("maxResults".to_string(), json!(page_size));
            if let Some(token) = &token {
                extra.insert("nextToken".to_string(), json!(token));
            }
            let body = listing_body(&fixture.twice_run, Value::Object(extra));
            let (status, answer) = list_pipeline_images(&router, &body).await;
            let answer = assert_ok(&case, status, answer);
            let page = listed_arns(&case, &answer);
            assert!(
                page.len() <= page_size as usize,
                "case {case:?}: expected at most {page_size} entries, got {page:?}"
            );
            walked.extend(page);
            pages += 1;
            assert!(
                pages <= complete.len() + 1,
                "case {case:?}: the walk did not end, having read {walked:?}"
            );
            token = answer
                .get("nextToken")
                .and_then(Value::as_str)
                .map(str::to_string);
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            walked, complete,
            "case {case:?}: expected the whole listing once, got {walked:?}"
        );
    }
}

/// RULE: a page token is issued FOR one listing, so a token another pipeline's listing handed
/// back is refused rather than spent.
#[tokio::test]
async fn a_token_of_another_listing_is_refused() {
    let router = emulator();
    let fixture = Fixture::given(&router).await;

    let case = "the first page of the pipeline that has run twice";
    let body = listing_body(&fixture.twice_run, json!({ "maxResults": 1 }));
    let (status, answer) = list_pipeline_images(&router, &body).await;
    let answer = assert_ok(case, status, answer);
    let token = text(case, &answer, "nextToken");

    // (case, the listing the token is offered to).
    let rows: Vec<(&str, Value)> = vec![
        (
            "another pipeline's listing",
            listing_body(&fixture.shared_recipe, json!({ "nextToken": token.clone() })),
        ),
        (
            "the same listing, now filtered",
            listing_body(
                &fixture.twice_run,
                json!({
                    "nextToken": token.clone(),
                    "filters": [{ "name": "name", "values": [Fixture::ALPHA.0] }],
                }),
            ),
        ),
        (
            "a token this store never issued",
            listing_body(&fixture.twice_run, json!({ "nextToken": "not-a-token" })),
        ),
    ];

    for (case, body) in rows {
        let (status, answer) = list_pipeline_images(&router, &body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {answer}"
        );
        assert_eq!(
            error_code(case, &answer),
            INVALID_PARAMETER_VALUE,
            "case {case:?}: expected {INVALID_PARAMETER_VALUE} in {answer}"
        );
        assert_eq!(
            error_message(case, &answer),
            "The value supplied for parameter 'nextToken' is not valid. The provided token \
             does not meet the required format.",
            "case {case:?}: expected the invalid-token sentence in {answer}"
        );
    }

    // The token is still good for the listing it was issued for.
    let case = "the second page of the listing the token belongs to";
    let body = listing_body(
        &fixture.twice_run,
        json!({ "maxResults": 1, "nextToken": token }),
    );
    let (status, answer) = list_pipeline_images(&router, &body).await;
    let answer = assert_ok(case, status, answer);
    let actual = listed_arns(case, &answer);
    assert_eq!(
        actual,
        vec![build_version_arn(Fixture::ALPHA.0, Fixture::ALPHA.1, 2)],
        "case {case:?}: expected the second entry of the listing, got {actual:?}"
    );
}

/// RULE: a filter naming a member this listing does not filter on is refused as a value its
/// `filters` parameter does not admit, stating the names it admits.
#[tokio::test]
async fn an_unsupported_filter_name_is_refused() {
    let router = emulator();
    let fixture = Fixture::given(&router).await;

    // The names a row sends: three the account-wide image listings admit and this one does
    // not, a member of the resource that is no filter of any listing, a name in the wrong
    // letter case, and a name nothing names.
    for name in ["platform", "osVersion", "type", "owner", "Name", "nme"] {
        let case = format!("the filter {name:?}");
        let body = listing_body(
            &fixture.twice_run,
            json!({ "filters": [{ "name": name, "values": ["anything"] }] }),
        );
        let (status, answer) = list_pipeline_images(&router, &body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {answer}"
        );
        assert_eq!(
            error_code(&case, &answer),
            INVALID_PARAMETER_VALUE,
            "case {case:?}: expected {INVALID_PARAMETER_VALUE} in {answer}"
        );
        assert_eq!(
            error_message(&case, &answer),
            format!("The value supplied for parameter 'filters' is not valid. {ALLOWED_FILTERS}"),
            "case {case:?}: expected the allowed-filters sentence in {answer}"
        );
    }
}

/// RULE: a filter that left out the member it selects on, or the values that member may take,
/// is reported as the missing parameter it is.
#[tokio::test]
async fn a_filter_missing_a_member_is_reported() {
    let router = emulator();
    let fixture = Fixture::given(&router).await;

    // (case, the filters the request carried, the member it is reported for).
    let rows: Vec<(&str, Value, &str)> = vec![
        (
            "a filter naming no member",
            json!([{ "values": ["anything"] }]),
            "name",
        ),
        (
            "a filter carrying no values",
            json!([{ "name": "name" }]),
            "values",
        ),
        (
            "a filter missing both members",
            json!([{}]),
            "name",
        ),
        (
            "a filter missing its values behind one that is whole",
            json!([
                { "name": "name", "values": [Fixture::ALPHA.0] },
                { "name": "version" },
            ]),
            "values",
        ),
    ];

    for (case, filters, member) in rows {
        let body = listing_body(&fixture.twice_run, json!({ "filters": filters }));
        let (status, answer) = list_pipeline_images(&router, &body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {answer}"
        );
        assert_eq!(
            error_code(case, &answer),
            MISSING_PARAMETERS,
            "case {case:?}: expected {MISSING_PARAMETERS} in {answer}"
        );
        assert_eq!(
            error_message(case, &answer),
            format!(
                "Required parameters are missing from the request: '{member}'. Verify that you \
                 have entered all of the required parameters in your request and try again."
            ),
            "case {case:?}: expected the missing-parameter sentence for {member} in {answer}"
        );
    }
}

/// RULE: a pipeline this Region holds none of is reported missing, naming the kind of resource
/// the ARN identifies and the ARN the request carried.
#[tokio::test]
async fn a_pipeline_this_region_holds_none_of_is_reported_missing() {
    let router = emulator();
    let fixture = Fixture::given(&router).await;

    // A pipeline that existed and was deleted holds no images to list either.
    delete_pipeline(&router, &fixture.never_run).await;

    // (case, the ARN the request named).
    let rows: Vec<(&str, String)> = vec![
        ("a pipeline nothing created", pipeline_arn("never-created")),
        ("a pipeline that was deleted", fixture.never_run.clone()),
        (
            "a pipeline of another Region",
            format!("arn:aws:imagebuilder:eu-west-1:{ACCOUNT}:image-pipeline/pipeline-images-first"),
        ),
        (
            "a pipeline of another account",
            format!(
                "arn:aws:imagebuilder:{REGION}:210987654321:image-pipeline/pipeline-images-first"
            ),
        ),
    ];

    for (case, arn) in rows {
        let (status, answer) = list_pipeline_images(&router, &listing_body(&arn, json!({}))).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected HTTP 404, got {status} with {answer}"
        );
        assert_eq!(
            error_code(case, &answer),
            RESOURCE_NOT_FOUND,
            "case {case:?}: expected {RESOURCE_NOT_FOUND} in {answer}"
        );
        assert_eq!(
            error_message(case, &answer),
            format!(
                "The following required resource 'ImagePipeline' cannot be found: '{arn}'."
            ),
            "case {case:?}: expected the missing-resource sentence in {answer}"
        );
    }
}
