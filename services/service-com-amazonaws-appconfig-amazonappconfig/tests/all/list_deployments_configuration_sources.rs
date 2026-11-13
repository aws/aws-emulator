// SPDX-License-Identifier: CC0-1.0

//! `ListDeployments` over the configuration SOURCES a profile can name.
//!
//! The rule under test: `ListDeployments` reports one summary per deployment the addressed
//! environment holds, newest deployment number first and paged by `MaxResults`/`NextToken`,
//! read off the stored deployment record and therefore INDEPENDENT of where the deployed
//! configuration lives — AppConfig's own hosted store or any external store a profile's
//! `LocationUri` names — and an environment that holds no deployment is a 200 with an empty
//! `Items` list rather than a refusal.
//!
//! The companion suite `list_deployments` asserts the ordering, paging and refusal rules
//! over hosted profiles; every row here deploys a DIFFERENT configuration source, the
//! dimension that suite holds fixed, and cross-checks each summary against `GetDeployment`
//! of the same deployment — which records the external source in `ConfigurationLocationUri`,
//! a member `DeploymentSummary` does not model.
//!
//! Every case drives real HTTP requests through the router the emulator's front door uses.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::appconfig_workflow::{
    create_application, create_environment, create_profile_from, json_call, router, send,
    with_required_retrieval_role, Answer,
};

/// One configuration source a profile can name, and the `ConfigurationVersion` a deployment
/// of it names.
struct Source {
    /// What a failing assertion calls the row.
    case: &'static str,
    /// The profile's `LocationUri`.
    location_uri: &'static str,
    /// The version the deployment names. "If deploying an AppConfig hosted configuration
    /// version, you can specify either the version number or version label. For all other
    /// configurations, you must specify the version number"
    /// (`smithy_query(query="operation",operation="StartDeployment")`, `ConfigurationVersion`).
    version: &'static str,
    /// Whether the row's bytes live in AppConfig's hosted store, so the suite hosts a
    /// version for it before deploying.
    hosted: bool,
}

/// Every store the model names on `CreateConfigurationProfile`'s `LocationUri` member:
/// "For the AppConfig hosted configuration store and for feature flags, specify hosted. For
/// an Amazon Web Services Systems Manager Parameter Store parameter, specify either the
/// parameter name in the format ssm-parameter:// or the ARN. For an Amazon Web Services
/// CodePipeline pipeline, specify the URI in the following format: codepipeline://. For an
/// Secrets Manager secret, specify the URI in the following format: secretsmanager://. For
/// an Amazon S3 object, specify the URI in the following format: s3:/// … For an SSM
/// document, specify either the document name or ARN"
/// (`smithy_query(query="doc",shape="CreateConfigurationProfileRequest",member="LocationUri")`).
///
/// The rows are deployed in this order into one environment, so the listing's descending
/// order is asserted across sources, not within one.
const SOURCES: [Source; 7] = [
    Source {
        case: "the hosted store",
        location_uri: "hosted",
        version: "1",
        hosted: true,
    },
    Source {
        case: "a Parameter Store parameter by name",
        location_uri: "ssm-parameter://app-config-parameter",
        version: "3",
        hosted: false,
    },
    Source {
        case: "a Parameter Store parameter by ARN",
        location_uri: "arn:aws:ssm:us-east-1:000000000000:parameter/app-config-parameter",
        version: "12",
        hosted: false,
    },
    Source {
        case: "an SSM document",
        location_uri: "ssm-document://app-config-document",
        version: "7",
        hosted: false,
    },
    Source {
        case: "a Secrets Manager secret",
        location_uri: "secretsmanager://app-config-secret",
        version: "AWSCURRENT",
        hosted: false,
    },
    Source {
        case: "an S3 object",
        location_uri: "s3://amzn-s3-demo-bucket/my-app/us-east-1/my-config.json",
        version: "2",
        hosted: false,
    },
    Source {
        case: "a CodePipeline pipeline",
        location_uri: "codepipeline://app-config-pipeline",
        version: "1",
        hosted: false,
    },
];

/// The member `GetDeployment` reports and `DeploymentSummary` does not model, so a summary
/// must NOT carry it while the reader of the same record must
/// (`smithy_query(query="shape",shape="DeploymentSummary")`).
const SOURCE_MEMBER_A_SUMMARY_OMITS: &str = "ConfigurationLocationUri";

/// Where the deployments of one environment live, from this operation's modelled URI
/// (`smithy_query(query="operation",operation="ListDeployments")`).
fn deployments_uri(application: &str, environment: &str) -> String {
    format!("/applications/{application}/environments/{environment}/deployments")
}

/// A deployment strategy with no rollout window and no bake time, so a deployment it paces
/// is over one growth interval after it started and the next row can then start its own
/// (`StartDeployment` refuses a second deployment while one is still active).
async fn instant_strategy(router: &MultiServiceRouter, name: &str) -> String {
    let answer = json_call(
        router,
        "POST",
        "/deploymentstrategies",
        &json!({
            "Name": name,
            "DeploymentDurationInMinutes": 0,
            "GrowthFactor": 100.0,
            "GrowthType": "LINEAR",
            "FinalBakeTimeInMinutes": 0,
            "ReplicateTo": "NONE",
        }),
    )
    .await;
    assert_eq!(
        answer.status,
        201,
        "case {name:?}: CreateDeploymentStrategy must answer 201, got {} / {}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
    answer
        .string("Id")
        .unwrap_or_else(|| panic!("case {name:?}: CreateDeploymentStrategy answered no Id"))
}

/// `GET` one page of the listing, with the query members in `query`.
async fn list(
    router: &MultiServiceRouter,
    application: &str,
    environment: &str,
    query: &[(&str, String)],
) -> Answer {
    let mut uri = deployments_uri(application, environment);
    if !query.is_empty() {
        let rendered: Vec<String> = query
            .iter()
            .map(|(name, value)| format!("{name}={value}"))
            .collect();
        uri.push('?');
        uri.push_str(&rendered.join("&"));
    }
    send(router, "GET", &uri, "application/json", Vec::new(), &[]).await
}

/// The `Items` of a successful listing.
fn items(answer: &Answer, case: &str) -> Vec<Value> {
    assert_eq!(
        answer.status,
        200,
        "case {case:?}: ListDeployments expected 200, got {} / {}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
    answer
        .json()
        .get("Items")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: no Items array in {}", answer.json()))
}

/// `GetDeployment` of one deployment: the other report of the same stored record.
async fn get_deployment(
    router: &MultiServiceRouter,
    application: &str,
    environment: &str,
    number: i64,
    case: &str,
) -> Value {
    let answer = json_call(
        router,
        "GET",
        &format!("{}/{number}", deployments_uri(application, environment)),
        &Value::Null,
    )
    .await;
    assert_eq!(
        answer.status,
        200,
        "case {case:?}: GetDeployment must answer 200, got {} / {}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
    answer.json()
}

/// What one row left behind: the profile it deployed and the deployment number it got.
struct Deployed {
    case: &'static str,
    location_uri: &'static str,
    version: &'static str,
    profile_id: String,
    profile_name: String,
    deployment_number: i64,
}

/// Deploy every row of [`SOURCES`] into one environment, in table order.
async fn deploy_every_source(
    router: &MultiServiceRouter,
    application: &str,
    environment: &str,
    strategy: &str,
) -> Vec<Deployed> {
    let mut deployed = Vec::new();
    for (index, source) in SOURCES.iter().enumerate() {
        let profile_name = format!("source-{index}");
        let profile = create_profile_from(
            router,
            application,
            source.case,
            &with_required_retrieval_role(json!({
                "Name": profile_name,
                "LocationUri": source.location_uri,
            })),
        )
        .await;
        let profile_id = profile
            .string("Id")
            .unwrap_or_else(|| panic!("case {:?}: the profile answered no Id", source.case));
        assert_eq!(
            profile.string("LocationUri").as_deref(),
            Some(source.location_uri),
            "case {:?}: the profile must keep the LocationUri it was created with, got {}",
            source.case,
            profile.json()
        );
        if source.hosted {
            let hosted = send(
                router,
                "POST",
                &format!(
                    "/applications/{application}/configurationprofiles/{profile_id}\
                     /hostedconfigurationversions"
                ),
                "application/json",
                br#"{"enabled":true}"#.to_vec(),
                &[],
            )
            .await;
            assert_eq!(
                hosted.status,
                201,
                "case {:?}: CreateHostedConfigurationVersion must answer 201, got {} / {}",
                source.case,
                hosted.status,
                String::from_utf8_lossy(&hosted.body)
            );
        }
        let answer = json_call(
            router,
            "POST",
            &deployments_uri(application, environment),
            &json!({
                "ConfigurationProfileId": profile_id,
                "ConfigurationVersion": source.version,
                "DeploymentStrategyId": strategy,
            }),
        )
        .await;
        assert_eq!(
            answer.status,
            201,
            "case {:?}: StartDeployment must answer 201, got {} / {}",
            source.case,
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        let deployment_number = answer
            .json()
            .get("DeploymentNumber")
            .and_then(Value::as_i64)
            .unwrap_or_else(|| {
                panic!(
                    "case {:?}: StartDeployment answered no DeploymentNumber: {}",
                    source.case,
                    answer.json()
                )
            });
        // The rollout is a transition AppConfig performs after the call that asked for it,
        // and an environment holds one deployment at a time, so the next row waits for
        // this deployment to be over — on the CONDITION rather than on a span.
        crate::appconfig_workflow::deployment_in_state(
            router,
            source.case,
            application,
            environment,
            deployment_number,
            &["COMPLETE"],
        )
        .await;
        deployed.push(Deployed {
            case: source.case,
            location_uri: source.location_uri,
            version: source.version,
            profile_id,
            profile_name,
            deployment_number,
        });
    }
    deployed
}

/// A deployment of ANY configuration source is reported by the listing, newest first, with
/// every member of its summary agreeing with `GetDeployment` of the same deployment.
#[tokio::test]
async fn every_configuration_source_is_listed_newest_first_and_agrees_with_the_reader() {
    let router = router();
    let application = create_application(&router, "sources").await;
    let environment = create_environment(&router, &application, "sources").await;
    let strategy = instant_strategy(&router, "sources-instant").await;
    let deployed = deploy_every_source(&router, &application, &environment, &strategy).await;

    let answer = list(&router, &application, &environment, &[]).await;
    let page = items(&answer, "every source");
    assert_eq!(
        page.len(),
        deployed.len(),
        "case \"every source\": expected one summary per deployment ({}), got {}: {}",
        deployed.len(),
        page.len(),
        answer.json()
    );
    assert!(
        answer.json().get("NextToken").is_none(),
        "case \"every source\": a listing that reported every deployment must carry no \
         NextToken, got {}",
        answer.json()
    );

    let reported: Vec<i64> = page
        .iter()
        .map(|summary| {
            summary
                .get("DeploymentNumber")
                .and_then(Value::as_i64)
                .unwrap_or_else(|| {
                    panic!("case \"every source\": a summary carries no DeploymentNumber: {summary}")
                })
        })
        .collect();
    let mut newest_first: Vec<i64> = deployed.iter().map(|row| row.deployment_number).collect();
    newest_first.reverse();
    assert_eq!(
        reported, newest_first,
        "case \"every source\": expected the deployment numbers newest first ({newest_first:?}), \
         got {reported:?}"
    );

    for row in &deployed {
        let summary = page
            .iter()
            .find(|summary| {
                summary.get("DeploymentNumber").and_then(Value::as_i64) == Some(row.deployment_number)
            })
            .unwrap_or_else(|| {
                panic!(
                    "case {:?}: deployment {} is missing from the listing: {}",
                    row.case,
                    row.deployment_number,
                    answer.json()
                )
            });
        assert_eq!(
            summary.get("ConfigurationProfileId").and_then(Value::as_str),
            Some(row.profile_id.as_str()),
            "case {:?}: expected the deployed profile {:?}, got {summary}",
            row.case,
            row.profile_id
        );
        assert_eq!(
            summary.get("ConfigurationName").and_then(Value::as_str),
            Some(row.profile_name.as_str()),
            "case {:?}: expected the deployed configuration's name {:?}, got {summary}",
            row.case,
            row.profile_name
        );
        assert_eq!(
            summary.get("ConfigurationVersion").and_then(Value::as_str),
            Some(row.version),
            "case {:?}: expected the version the deployment named ({:?}), got {summary}",
            row.case,
            row.version
        );
        assert!(
            summary.get(SOURCE_MEMBER_A_SUMMARY_OMITS).is_none(),
            "case {:?}: a summary does not model {SOURCE_MEMBER_A_SUMMARY_OMITS}, got {summary}",
            row.case
        );

        // The two reports of one record: every member the summary carries must be the
        // member `GetDeployment` reports, and the reader must record the external source
        // the summary has no member for.
        let document = get_deployment(
            &router,
            &application,
            &environment,
            row.deployment_number,
            row.case,
        )
        .await;
        let summary_members = summary
            .as_object()
            .unwrap_or_else(|| panic!("case {:?}: a summary must be a document: {summary}", row.case));
        for (name, value) in summary_members {
            assert_eq!(
                document.get(name),
                Some(value),
                "case {:?}: the summary's {name} must be what GetDeployment reports ({:?}), got \
                 {value:?}",
                row.case,
                document.get(name)
            );
        }
        assert_eq!(
            document
                .get(SOURCE_MEMBER_A_SUMMARY_OMITS)
                .and_then(Value::as_str),
            Some(row.location_uri),
            "case {:?}: GetDeployment must record the configuration's source location, got \
             {document}",
            row.case
        );
    }
}

/// `MaxResults` bounds a page and `NextToken` resumes after it, so a walk over deployments
/// of mixed configuration sources reports every one exactly once, still newest first, at
/// any page size the model's range allows.
#[tokio::test]
async fn a_walk_over_mixed_sources_reports_every_deployment_exactly_once() {
    let router = router();
    let application = create_application(&router, "source-walks").await;
    let environment = create_environment(&router, &application, "source-walks").await;
    let strategy = instant_strategy(&router, "walk-instant").await;
    let deployed = deploy_every_source(&router, &application, &environment, &strategy).await;
    let mut newest_first: Vec<i64> = deployed.iter().map(|row| row.deployment_number).collect();
    newest_first.reverse();

    // The published bounds of `MaxResults` on this operation are `range 1..50`
    // (`smithy_query(query="operation",operation="ListDeployments")`): the smallest page,
    // a page that splits the rows, a page that holds them all and the published maximum.
    for max_results in [1_i64, 3, newest_first.len() as i64, 50] {
        let case = format!("max_results={max_results}");
        let mut walked: Vec<i64> = Vec::new();
        let mut next_token: Option<String> = None;
        loop {
            let mut query = vec![("max_results", max_results.to_string())];
            if let Some(token) = &next_token {
                query.push(("next_token", token.clone()));
            }
            let answer = list(&router, &application, &environment, &query).await;
            let page = items(&answer, &case);
            assert!(
                page.len() as i64 <= max_results,
                "case {case:?}: a page must hold at most MaxResults items, got {}",
                page.len()
            );
            for summary in &page {
                walked.push(
                    summary
                        .get("DeploymentNumber")
                        .and_then(Value::as_i64)
                        .unwrap_or_else(|| {
                            panic!("case {case:?}: a summary carries no DeploymentNumber: {summary}")
                        }),
                );
            }
            next_token = answer.string("NextToken");
            if next_token.is_none() {
                break;
            }
            assert!(
                !page.is_empty(),
                "case {case:?}: a page that resumes must carry items, got {}",
                answer.json()
            );
        }
        assert_eq!(
            walked, newest_first,
            "case {case:?}: expected the walk to report every deployment exactly once, newest \
             first ({newest_first:?}), got {walked:?}"
        );
    }
}

/// An environment that holds no deployment is an ANSWER about an empty history: 200 with an
/// empty `Items` list and no `NextToken` — not a refusal — whether or not the application
/// holds configuration profiles or deployments in a sibling environment.
#[tokio::test]
async fn an_environment_with_no_deployment_is_an_empty_listing() {
    let router = router();
    let application = create_application(&router, "empties").await;
    let strategy = instant_strategy(&router, "empty-instant").await;

    for (index, (case, deploy_into_a_sibling)) in [
        ("an application with nothing deployed anywhere", false),
        ("an environment beside one that holds deployments", true),
    ]
    .into_iter()
    .enumerate()
    {
        let environment =
            create_environment(&router, &application, &format!("empty-{index}")).await;
        if deploy_into_a_sibling {
            let sibling =
                create_environment(&router, &application, &format!("sibling-{index}")).await;
            deploy_every_source(&router, &application, &sibling, &strategy).await;
        }
        let answer = list(&router, &application, &environment, &[]).await;
        let page = items(&answer, case);
        assert!(
            page.is_empty(),
            "case {case:?}: expected an empty Items list, got {}",
            answer.json()
        );
        assert!(
            answer.json().get("NextToken").is_none(),
            "case {case:?}: an empty listing has no further page, so it carries no NextToken, \
             got {}",
            answer.json()
        );
    }
}
