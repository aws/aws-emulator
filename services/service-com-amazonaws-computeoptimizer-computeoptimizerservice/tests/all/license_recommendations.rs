// SPDX-License-Identifier: CC0-1.0

//! `GetLicenseRecommendations` over the wire: the recommendation this service derives from
//! each observed commercial software license, and how a request narrows, filters, pages and
//! refuses.
//!
//! Every assertion here is on the RULE, checked over a SET of licenses that differ in
//! edition, license model, core count, how long they have been observed, and what their
//! CloudWatch Application Insights metric source reports — the response itself carries the
//! license configuration a recommendation was derived from, so an option can be checked
//! against the license it would replace without this suite restating the handler's
//! arithmetic.

use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_computeoptimizer_computeoptimizerservice::state::ANALYSIS_DELAY;
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{call, call_in_region, emulator, error_code, opt_in};

/// The model's `LicenseFinding` values
/// (`crate::ops::types::LICENSE_FINDING_VALUES`).
const FINDINGS: &[&str] = &["InsufficientMetrics", "NotOptimized", "Optimized"];

/// The model's `LicenseFindingReasonCode` values
/// (`crate::ops::types::LICENSE_FINDING_REASON_CODE_VALUES`).
const FINDING_REASON_CODES: &[&str] = &[
    "InvalidCloudWatchApplicationInsightsSetup",
    "CloudWatchApplicationInsightsError",
    "LicenseOverprovisioned",
    "Optimized",
];

/// The model's `LicenseEdition` values (`crate::ops::types::LICENSE_EDITION_VALUES`), in the
/// order they license one another's feature sets — `Free` ⊂ `Standard` ⊂ `Enterprise`, the
/// rank `crate::state::license_edition_rank` states, which is what makes a license
/// "over-provisioned" when it "can be downgraded while still meeting the performance
/// requirements of your workload" (`LicenseRecommendation$findingReasonCodes`).
/// `NoLicenseEditionFound` names no edition at all, so it carries no rank.
const EDITIONS_BY_RANK: &[&str] = &["Free", "Standard", "Enterprise"];

/// The model's `LicenseModel` values (`crate::ops::types::LICENSE_MODEL_VALUES`).
const LICENSE_MODELS: &[&str] = &["LicenseIncluded", "BringYourOwnLicense"];

/// The model's `LicenseName` values (`crate::ops::types::LICENSE_NAME_VALUES`).
const LICENSE_NAMES: &[&str] = &["SQLServer"];

/// The model's `MetricSourceProvider` values
/// (`crate::ops::types::METRIC_SOURCE_PROVIDER_VALUES`).
const METRIC_SOURCE_PROVIDERS: &[&str] = &["CloudWatchApplicationInsights"];

/// The look-back period in force when no preference sets one, from the model's own
/// `PutRecommendationPreferences$lookBackPeriod` documentation: "When this preference isn't
/// specified, we use the default value `DAYS_14`".
const DEFAULT_LOOK_BACK_DAYS: f64 = 14.0;

/// The refusal every reader of this service answers a request about ANOTHER account with
/// (`crate::state::CROSS_ACCOUNT_MESSAGE`, the one wording for the whole crate, which
/// `tests/all/cross_account_requests.rs` pins for every reader that takes `accountIds`): it
/// names neither the member nor the account — it states the permission that is missing and
/// how an organization account gets it.
const CROSS_ACCOUNT_MESSAGE: &str = "Invalid permission to access another account. If this \
     is an organization account, request the management account to enable service access.";

/// The seeded licensed instances, by instance id — one per case a license recommendation
/// distinguishes.
const OVER_PROVISIONED_LICENSE_INCLUDED: &str = "i-0a1b2c3d4e5f70001";
const ALREADY_OPTIMIZED: &str = "i-0a1b2c3d4e5f70002";
const OVER_PROVISIONED_BRING_YOUR_OWN: &str = "i-0a1b2c3d4e5f70003";
const INSIGHTS_NOT_SET_UP: &str = "i-0a1b2c3d4e5f70004";
const INSIGHTS_FAILING: &str = "i-0a1b2c3d4e5f70005";

/// Opt the account in and let the analysis its enrollment starts come due — the gate every
/// recommendation reader of this service applies. The span is the service's own
/// ([`ANALYSIS_DELAY`]); the extra margin covers scheduler jitter.
async fn analysed(router: &MultiServiceRouter) {
    opt_in(router).await;
    tokio::time::sleep(ANALYSIS_DELAY + Duration::from_millis(100)).await;
}

/// The ARN of one seeded licensed instance, in `region`.
fn instance_arn_in(region: &str, instance_id: &str) -> String {
    format!(
        "arn:aws:ec2:{region}:{}:instance/{instance_id}",
        ::engine::emulator_account_id()
    )
}

/// The ARN of one seeded licensed instance at the endpoint these tests address.
fn instance_arn(instance_id: &str) -> String {
    instance_arn_in("us-east-1", instance_id)
}

/// Read license recommendations, asserting the call was served.
async fn recommendations(router: &MultiServiceRouter, request: Value) -> Value {
    let (status, body) = call(router, "GetLicenseRecommendations", request.clone()).await;
    assert_eq!(
        status, 200,
        "case {request}: expected 200, got {status}: {body}"
    );
    body
}

/// The recommendations of one response, in the order it reports them.
fn reported(body: &Value) -> Vec<Value> {
    body["licenseRecommendations"]
        .as_array()
        .unwrap_or_else(|| panic!("licenseRecommendations must be a list, got {body}"))
        .clone()
}

/// The resource ARN of every recommendation in a response, in order.
fn reported_arns(body: &Value) -> Vec<String> {
    reported(body)
        .iter()
        .map(|recommendation| {
            recommendation["resourceArn"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// The recommendation a response carries for one seeded instance.
fn recommendation_for(body: &Value, instance_id: &str) -> Value {
    reported(body)
        .into_iter()
        .find(|recommendation| {
            recommendation["resourceArn"]
                .as_str()
                .unwrap_or_default()
                .ends_with(&format!("instance/{instance_id}"))
        })
        .unwrap_or_else(|| panic!("case {instance_id:?}: no recommendation for it: {body}"))
}

/// A number the wire carries, or `0` for one the protocol omitted because it equals the
/// member's modelled default.
fn number(value: &Value, member: &str) -> f64 {
    value[member].as_f64().unwrap_or(0.0)
}

/// A string member of a response object.
fn text(value: &Value, member: &str) -> String {
    value[member].as_str().unwrap_or_default().to_string()
}

/// The finding reason codes a recommendation reports.
fn reason_codes(recommendation: &Value) -> Vec<String> {
    recommendation["findingReasonCodes"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|code| code.as_str().unwrap_or_default().to_string())
        .collect()
}

/// The recommendation options a recommendation reports (none at all is an empty list here).
fn options(recommendation: &Value) -> Vec<Value> {
    recommendation["licenseRecommendationOptions"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// What the one option of a recommendation would save per month, and `0.0` for one that
/// carries no savings figure at all (a license AWS charges nothing for).
fn option_savings(option: &Value) -> f64 {
    number(&option["savingsOpportunity"]["estimatedMonthlySavings"], "value")
}

/// Where an edition sits in [`EDITIONS_BY_RANK`], or `None` for one that is no edition.
fn edition_rank(edition: &str) -> Option<usize> {
    EDITIONS_BY_RANK.iter().position(|known| *known == edition)
}

/// Every invariant a license recommendation satisfies, whatever license it is for.
///
/// The rule: a recommendation is the observed license read three ways — the configuration is
/// the license in force, the finding and its reason code say what the metric source reported
/// about that license and whether it can be downgraded, and an option is reported exactly
/// where there is a lower edition to move to, at the same operating system and license model,
/// with the saving that move is worth.
fn assert_recommendation_invariants(recommendation: &Value) {
    let case = text(recommendation, "resourceArn");
    assert!(
        case.starts_with(&format!(
            "arn:aws:ec2:us-east-1:{}:instance/i-",
            ::engine::emulator_account_id()
        )),
        "case {case:?}: a license recommendation names the EC2 instance it is for, as \
         arn:aws:ec2:region:aws_account_id:instance/instance-id, got {case:?}"
    );
    assert_eq!(
        text(recommendation, "accountId"),
        ::engine::emulator_account_id(),
        "case {case:?}: a recommendation reports the account that owns the license, got \
         {recommendation}"
    );

    let configuration = &recommendation["currentLicenseConfiguration"];
    let current_edition = text(configuration, "licenseEdition");
    let license_model = text(configuration, "licenseModel");
    let operating_system = text(configuration, "operatingSystem");
    assert!(
        LICENSE_NAMES.contains(&text(configuration, "licenseName").as_str()),
        "case {case:?}: licenseName must be a LicenseName, got {configuration}"
    );
    assert!(
        EDITIONS_BY_RANK.contains(&current_edition.as_str())
            || current_edition == "NoLicenseEditionFound",
        "case {case:?}: licenseEdition must be a LicenseEdition, got {configuration}"
    );
    assert!(
        LICENSE_MODELS.contains(&license_model.as_str()),
        "case {case:?}: licenseModel must be a LicenseModel, got {configuration}"
    );
    assert!(
        !operating_system.is_empty() && !text(configuration, "instanceType").is_empty(),
        "case {case:?}: a configuration names the instance the license runs on, got \
         {configuration}"
    );
    let cores = number(configuration, "numberOfCores");
    assert!(
        cores > 0.0 && cores.fract() == 0.0,
        "case {case:?}: numberOfCores is a whole number of cores, got {cores}"
    );
    for source in configuration["metricsSource"]
        .as_array()
        .cloned()
        .unwrap_or_default()
    {
        assert!(
            METRIC_SOURCE_PROVIDERS.contains(&text(&source, "provider").as_str()),
            "case {case:?}: provider must be a MetricSourceProvider, got {source}"
        );
        let expected_prefix = format!(
            "arn:aws:applicationinsights:us-east-1:{}:application/resource-group/",
            ::engine::emulator_account_id()
        );
        assert!(
            text(&source, "providerArn").starts_with(&expected_prefix),
            "case {case:?}: a metric source names the Application Insights application that \
             watches the instance, expected an ARN starting {expected_prefix:?}, got {source}"
        );
    }

    let look_back = number(recommendation, "lookbackPeriodInDays");
    assert!(
        look_back > 0.0 && look_back <= DEFAULT_LOOK_BACK_DAYS,
        "case {case:?}: with no look-back preference stored the analysed period is at most \
         the default {DEFAULT_LOOK_BACK_DAYS} days, got {look_back}"
    );
    assert!(
        number(recommendation, "lastRefreshTimestamp") > 0.0,
        "case {case:?}: a recommendation says when it was generated, got {recommendation}"
    );

    let finding = text(recommendation, "finding");
    assert!(
        FINDINGS.contains(&finding.as_str()),
        "case {case:?}: finding must be a LicenseFinding, got {finding:?}"
    );
    let codes = reason_codes(recommendation);
    assert!(
        !codes.is_empty(),
        "case {case:?}: a finding is reported with the reason behind it, got {recommendation}"
    );
    for code in &codes {
        assert!(
            FINDING_REASON_CODES.contains(&code.as_str()),
            "case {case:?}: {code:?} is not a LicenseFindingReasonCode, got {recommendation}"
        );
    }

    let reported_options = options(recommendation);
    match finding.as_str() {
        // "When Compute Optimizer detects that your CloudWatch Application Insights isn't
        // enabled or is enabled with insufficient permissions": nothing was read about the
        // license, so there is nothing to move it to.
        "InsufficientMetrics" => {
            assert!(
                codes.iter().all(|code| code
                    == "InvalidCloudWatchApplicationInsightsSetup"
                    || code == "CloudWatchApplicationInsightsError"),
                "case {case:?}: InsufficientMetrics is explained by the metric source, got \
                 {codes:?}"
            );
            assert!(
                reported_options.is_empty(),
                "case {case:?}: a license the analysis could not read has no recommendation \
                 option, got {reported_options:?}"
            );
        }
        // "A license is considered over-provisioned when your license can be downgraded
        // while still meeting the performance requirements of your workload."
        "NotOptimized" => {
            assert_eq!(
                codes,
                vec!["LicenseOverprovisioned".to_string()],
                "case {case:?}: NotOptimized is explained by LicenseOverprovisioned, got \
                 {codes:?}"
            );
            assert_eq!(
                reported_options.len(),
                1,
                "case {case:?}: expected the one option this analysis has, got \
                 {reported_options:?}"
            );
            let option = &reported_options[0];
            assert_eq!(
                number(option, "rank"),
                1.0,
                "case {case:?}: the top recommendation option is ranked as 1, got {option}"
            );
            let recommended_edition = text(option, "licenseEdition");
            let (recommended_rank, current_rank) = (
                edition_rank(&recommended_edition),
                edition_rank(&current_edition),
            );
            assert!(
                recommended_rank.is_some_and(|recommended| current_rank
                    .is_some_and(|current| recommended < current)),
                "case {case:?}: an over-provisioned license is moved DOWN a tier, expected an \
                 edition ranking below {current_edition:?} in {EDITIONS_BY_RANK:?}, got \
                 {recommended_edition:?}"
            );
            assert_eq!(
                text(option, "operatingSystem"),
                operating_system,
                "case {case:?}: the option keeps the instance's operating system, got {option}"
            );
            assert_eq!(
                text(option, "licenseModel"),
                license_model,
                "case {case:?}: the option keeps the license model, got {option}"
            );
            if let Some(opportunity) = option.get("savingsOpportunity").filter(|v| !v.is_null()) {
                let value = number(&opportunity["estimatedMonthlySavings"], "value");
                let percentage = number(opportunity, "savingsOpportunityPercentage");
                assert!(
                    value > 0.0,
                    "case {case:?}: a downgrade of a license AWS charges for saves money, got \
                     {opportunity}"
                );
                assert_eq!(
                    text(&opportunity["estimatedMonthlySavings"], "currency"),
                    "USD",
                    "case {case:?}: a saving names its currency, got {opportunity}"
                );
                assert!(
                    (0.0..=100.0).contains(&percentage) && percentage > 0.0,
                    "case {case:?}: a saving is a percentage of monthly cost, got {percentage}"
                );
            }
        }
        // "When Compute Optimizer detects that all specifications of your license meet the
        // performance requirements of your workload."
        "Optimized" => {
            assert_eq!(
                codes,
                vec!["Optimized".to_string()],
                "case {case:?}: Optimized is explained by Optimized, got {codes:?}"
            );
            assert!(
                reported_options.is_empty(),
                "case {case:?}: a license that meets its workload's requirements has nothing to \
                 move to, got {reported_options:?}"
            );
        }
        other => panic!("case {case:?}: unexpected finding {other:?}"),
    }
}

/// Before the account opts in there is no analysis to report, and the service says so with
/// the modelled `OptInRequiredException` (HTTP 403) rather than an empty answer; once it
/// opts in, the analysis its enrollment starts has to come due before any recommendation
/// exists; and opting back out takes them away again.
#[tokio::test]
async fn license_recommendations_need_an_opted_in_account_and_a_due_analysis() {
    let router = emulator();

    let (status, body) = call(&router, "GetLicenseRecommendations", json!({})).await;
    assert_eq!(
        status, 403,
        "before opting in: expected 403, got {status}: {body}"
    );
    assert_eq!(
        error_code(&body),
        "OptInRequiredException",
        "before opting in: expected OptInRequiredException, got {body}"
    );

    opt_in(&router).await;
    let body = recommendations(&router, json!({})).await;
    assert_eq!(
        reported(&body),
        Vec::<Value>::new(),
        "straight after opting in the analysis is not due yet, so there is no recommendation \
         to report, got {body}"
    );

    tokio::time::sleep(ANALYSIS_DELAY + Duration::from_millis(100)).await;
    let body = recommendations(&router, json!({})).await;
    assert!(
        reported(&body).len() >= 5,
        "once the analysis is due every observed license is reported, got {body}"
    );

    let (status, body) = call(
        &router,
        "UpdateEnrollmentStatus",
        json!({"status": "Inactive"}),
    )
    .await;
    assert_eq!(status, 200, "opting out: expected 200, got {status}: {body}");
    let (status, body) = call(&router, "GetLicenseRecommendations", json!({})).await;
    assert_eq!(
        status, 403,
        "after opting out: expected 403, got {status}: {body}"
    );
    assert_eq!(
        error_code(&body),
        "OptInRequiredException",
        "after opting out: expected OptInRequiredException, got {body}"
    );
}

/// Every reported recommendation is the observed license derived, and the inventory spans
/// the cases that derivation distinguishes: a license-included license that can be
/// downgraded (so the move saves money), one already at the edition its workload needs, a
/// license the customer brings (which AWS charges nothing for, so its downgrade carries no
/// savings figure), and two whose metric source the analysis could not read.
#[tokio::test]
async fn every_recommendation_is_the_observed_license_derived() {
    let router = emulator();
    analysed(&router).await;

    let body = recommendations(&router, json!({})).await;
    let reported = reported(&body);
    assert!(
        reported.len() >= 5,
        "this suite needs the whole observed inventory to prove anything, got {body}"
    );
    for recommendation in &reported {
        assert_recommendation_invariants(recommendation);
    }

    // Rows: the instance, the finding its observation yields, and whether its option (if
    // any) carries a savings figure — a license AWS charges nothing for cannot report one.
    let cases: [(&str, &str, bool); 5] = [
        (OVER_PROVISIONED_LICENSE_INCLUDED, "NotOptimized", true),
        (ALREADY_OPTIMIZED, "Optimized", false),
        (OVER_PROVISIONED_BRING_YOUR_OWN, "NotOptimized", false),
        (INSIGHTS_NOT_SET_UP, "InsufficientMetrics", false),
        (INSIGHTS_FAILING, "InsufficientMetrics", false),
    ];
    for (instance_id, expected_finding, saves) in cases {
        let recommendation = recommendation_for(&body, instance_id);
        assert_eq!(
            text(&recommendation, "finding"),
            expected_finding,
            "case {instance_id:?}: expected finding {expected_finding:?}, got {recommendation}"
        );
        let saved: f64 = options(&recommendation).iter().map(option_savings).sum();
        assert_eq!(
            saved > 0.0, saves,
            "case {instance_id:?}: expected a saving to be reported: {saves}, got {saved}"
        );
    }

    // The two metric-source failures are distinguished by their reason code, which is the
    // only thing that separates them.
    assert_eq!(
        reason_codes(&recommendation_for(&body, INSIGHTS_NOT_SET_UP)),
        vec!["InvalidCloudWatchApplicationInsightsSetup".to_string()],
        "a metric source that is not set up is reported as an invalid setup, got {body}"
    );
    assert_eq!(
        reason_codes(&recommendation_for(&body, INSIGHTS_FAILING)),
        vec!["CloudWatchApplicationInsightsError".to_string()],
        "a metric source answering with an error is reported as an error, got {body}"
    );

    // An instance observed for less than the look-back period is analysed over the period it
    // has been observed, not over one the service never watched.
    let shortest = number(
        &recommendation_for(&body, ALREADY_OPTIMIZED),
        "lookbackPeriodInDays",
    );
    let longest = number(
        &recommendation_for(&body, OVER_PROVISIONED_LICENSE_INCLUDED),
        "lookbackPeriodInDays",
    );
    assert!(
        shortest < longest && longest == DEFAULT_LOOK_BACK_DAYS,
        "an instance observed for less than the look-back period reports the shorter period: \
         got {shortest} against {longest}"
    );
}

/// A license recommendation is REGIONAL: the ARNs it reports carry the region of the
/// endpoint the request addressed, and a request that names another region's instance holds
/// a recommendation for nothing.
#[tokio::test]
async fn recommendations_are_reported_for_the_region_the_request_addressed() {
    let router = emulator();
    analysed(&router).await;

    // Rows: two regional endpoints, each of which reports its own region's ARNs.
    for region in ["us-east-1", "eu-west-2"] {
        let (status, body) = call_in_region(&router, region, "GetLicenseRecommendations", json!({}))
            .await;
        assert_eq!(
            status, 200,
            "case {region:?}: expected 200, got {status}: {body}"
        );
        for arn in reported_arns(&body) {
            assert!(
                arn.starts_with(&format!("arn:aws:ec2:{region}:")),
                "case {region:?}: a regional endpoint reports its own region's ARNs, got {arn:?}"
            );
        }
        // An ARN of ANOTHER region names an instance this endpoint does not observe, so it
        // is reported as a resource with no recommendation rather than matched.
        let other_region = if region == "us-east-1" {
            "eu-west-2"
        } else {
            "us-east-1"
        };
        let foreign = instance_arn_in(other_region, OVER_PROVISIONED_LICENSE_INCLUDED);
        let (status, body) = call_in_region(
            &router,
            region,
            "GetLicenseRecommendations",
            json!({"resourceArns": [foreign.clone()]}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {region:?}: expected 200, got {status}: {body}"
        );
        assert_eq!(
            reported(&body),
            Vec::<Value>::new(),
            "case {region:?}: {foreign} is not an instance this endpoint observes, got {body}"
        );
        assert_eq!(
            body["errors"],
            json!([]),
            "case {region:?}: a resource with no recommendation is absent from the answer, not \
             an error of the request, got {body}"
        );
    }
}

/// `resourceArns` and `accountIds` select WHAT is reported: the named instances of the named
/// account, a well-formed ARN the service holds no license recommendation for is absent from
/// the answer rather than refused or reported as an error, and a value that is not an ARN —
/// or more than one account id — is refused.
#[tokio::test]
async fn resource_arns_and_account_ids_select_what_is_reported() {
    let router = emulator();
    analysed(&router).await;

    let all = reported_arns(&recommendations(&router, json!({})).await);
    let known = instance_arn(OVER_PROVISIONED_LICENSE_INCLUDED);
    let also_known = instance_arn(ALREADY_OPTIMIZED);
    // An EC2 instance ARN this service observes no license on: well-formed, so it selects
    // nothing rather than being refused.
    let unknown_instance = instance_arn("i-0a1b2c3d4e5f7ffff");
    // A well-formed ARN of another kind of resource entirely.
    let not_an_instance = format!(
        "arn:aws:rds:us-east-1:{}:db:orders-primary",
        ::engine::emulator_account_id()
    );

    // Rows: the member absent, empty, one ARN, two ARNs, an unobserved instance, and an ARN
    // of another resource kind.
    let cases: [(&str, Value, Vec<String>); 6] = [
        ("absent", json!({}), all.clone()),
        ("empty", json!({"resourceArns": []}), all.clone()),
        (
            "one ARN",
            json!({"resourceArns": [known.clone()]}),
            vec![known.clone()],
        ),
        (
            "two ARNs",
            json!({"resourceArns": [known.clone(), also_known.clone()]}),
            vec![known.clone(), also_known.clone()],
        ),
        (
            "an unobserved instance",
            json!({"resourceArns": [unknown_instance.clone()]}),
            Vec::new(),
        ),
        (
            "another resource kind",
            json!({"resourceArns": [not_an_instance.clone()]}),
            Vec::new(),
        ),
    ];
    for (case, request, expected_arns) in cases {
        let body = recommendations(&router, request).await;
        let mut reported = reported_arns(&body);
        let mut expected = expected_arns.clone();
        reported.sort();
        expected.sort();
        assert_eq!(
            reported, expected,
            "case {case:?}: expected exactly the named instances' recommendations, got {body}"
        );
        // A named resource with no license recommendation is one the analysis has not
        // reached: absent from the answer, and no per-resource failure of the request.
        assert_eq!(
            body["errors"],
            json!([]),
            "case {case:?}: expected no errors entry, got {body}"
        );
    }

    // `accountIds` is how a MANAGEMENT account names a member account: absent or the
    // caller's own reads the caller's licenses, and this emulator presents one standalone
    // account, so any OTHER account is REFUSED rather than answered with an empty list a
    // client would read as "that account has no licenses" — the rule
    // `crate::state::requested_account_id` states for every reader of this service.
    let own = ::engine::emulator_account_id().to_string();
    for (case, request) in [
        ("own account", json!({"accountIds": [own.clone()]})),
        ("empty", json!({"accountIds": []})),
    ] {
        let body = recommendations(&router, request).await;
        assert_eq!(
            reported_arns(&body),
            all,
            "case {case:?}: expected the caller's own licenses, got {body}"
        );
    }

    // Rows: a value that is not an ARN at all, two account ids ("Only one account ID can be
    // specified per request"), an id that is not an account id, and the two ways a request
    // can ask about ANOTHER account — in `accountIds` and through a `resourceArns` entry.
    let refusals: [(&str, Value); 6] = [
        ("not an ARN", json!({"resourceArns": ["i-0a1b2c3d4e5f70001"]})),
        ("empty ARN", json!({"resourceArns": [""]})),
        (
            "two accounts",
            json!({"accountIds": [own.clone(), "111122223333"]}),
        ),
        ("not an account id", json!({"accountIds": ["12345"]})),
        (
            "another account",
            json!({"accountIds": ["111122223333"]}),
        ),
        (
            "another account's instance",
            json!({"resourceArns": [format!(
                "arn:aws:ec2:us-east-1:111122223333:instance/{OVER_PROVISIONED_LICENSE_INCLUDED}"
            )]}),
        ),
    ];
    for (case, request) in refusals {
        let (status, body) = call(&router, "GetLicenseRecommendations", request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidParameterValueException",
            "case {case:?}: expected InvalidParameterValueException, got {body}"
        );
    }

    // The two cross-account refusals carry the crate's one wording for that refusal, so no
    // reader of this service words it differently.
    for (case, request) in [
        ("another account", json!({"accountIds": ["111122223333"]})),
        (
            "another account's instance",
            json!({"resourceArns": [format!(
                "arn:aws:ec2:us-east-1:111122223333:instance/{OVER_PROVISIONED_LICENSE_INCLUDED}"
            )]}),
        ),
    ] {
        let (_, body) = call(&router, "GetLicenseRecommendations", request).await;
        assert_eq!(
            body["message"].as_str(),
            Some(CROSS_ACCOUNT_MESSAGE),
            "case {case:?}: expected the crate's cross-account refusal, got {body}"
        );
    }
}

/// `filters` select on the recommendation this service DERIVED: a filter keeps exactly the
/// recommendations whose member its name selects on carries one of its values, several
/// filters are ANDed, and a filter with no values constrains nothing. A name or a value
/// outside the model's vocabulary is refused rather than silently matching nothing.
#[tokio::test]
async fn filters_select_on_the_derived_recommendation() {
    let router = emulator();
    analysed(&router).await;

    let unfiltered = reported(&recommendations(&router, json!({})).await);

    // Rows: each filter name over each of its documented values, a multi-value filter, two
    // filters ANDed, and a filter carrying no values.
    let mut cases: Vec<(String, Value)> = Vec::new();
    for finding in FINDINGS {
        cases.push((
            format!("Finding={finding}"),
            json!({"filters": [{"name": "Finding", "values": [finding]}]}),
        ));
    }
    for code in FINDING_REASON_CODES {
        cases.push((
            format!("FindingReasonCode={code}"),
            json!({"filters": [{"name": "FindingReasonCode", "values": [code]}]}),
        ));
    }
    for license_name in LICENSE_NAMES {
        cases.push((
            format!("LicenseName={license_name}"),
            json!({"filters": [{"name": "LicenseName", "values": [license_name]}]}),
        ));
    }
    cases.push((
        "Finding in {Optimized, NotOptimized}".to_string(),
        json!({"filters": [{"name": "Finding", "values": ["Optimized", "NotOptimized"]}]}),
    ));
    cases.push((
        "Finding=NotOptimized AND FindingReasonCode=LicenseOverprovisioned".to_string(),
        json!({"filters": [
            {"name": "Finding", "values": ["NotOptimized"]},
            {"name": "FindingReasonCode", "values": ["LicenseOverprovisioned"]},
        ]}),
    ));
    cases.push((
        "Finding with no values".to_string(),
        json!({"filters": [{"name": "Finding", "values": []}]}),
    ));

    for (case, request) in cases {
        let body = recommendations(&router, request.clone()).await;
        let selected = reported(&body);
        let expected: Vec<Value> = unfiltered
            .iter()
            .filter(|recommendation| matches_filters(recommendation, &request["filters"]))
            .cloned()
            .collect();
        assert_eq!(
            selected, expected,
            "case {case:?}: a filter keeps exactly the recommendations that satisfy it, got \
             {body}"
        );
    }

    // A value outside the vocabulary of the member its name selects on is refused.
    let refusals: [(&str, Value); 4] = [
        (
            "unmodelled Finding value",
            json!({"filters": [{"name": "Finding", "values": ["Overprovisioned"]}]}),
        ),
        (
            "unmodelled FindingReasonCode value",
            json!({"filters": [{"name": "FindingReasonCode", "values": ["MemoryOverprovisioned"]}]}),
        ),
        (
            "unmodelled LicenseName value",
            json!({"filters": [{"name": "LicenseName", "values": ["PostgreSQL"]}]}),
        ),
        (
            "no filter name",
            json!({"filters": [{"values": ["Optimized"]}]}),
        ),
    ];
    for (case, request) in refusals {
        let (status, body) = call(&router, "GetLicenseRecommendations", request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidParameterValueException",
            "case {case:?}: expected InvalidParameterValueException, got {body}"
        );
    }

    // A filter name the model does not declare is refused at the request boundary.
    let (status, body) = call(
        &router,
        "GetLicenseRecommendations",
        json!({"filters": [{"name": "LicenseEdition", "values": ["Enterprise"]}]}),
    )
    .await;
    assert_eq!(
        status, 400,
        "an unmodelled filter name names no member to select on, got {status}: {body}"
    );
}

/// Does one reported recommendation satisfy the filters of a request? The rule the handler
/// applies, stated independently here: the member a filter's name selects on carries one of
/// its values, several filters are ANDed, and a filter with no values constrains nothing.
fn matches_filters(recommendation: &Value, filters: &Value) -> bool {
    filters
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .all(|filter| {
            let values: Vec<String> = filter["values"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .iter()
                .map(|value| value.as_str().unwrap_or_default().to_string())
                .collect();
            if values.is_empty() {
                return true;
            }
            match filter["name"].as_str().unwrap_or_default() {
                "Finding" => values.contains(&text(recommendation, "finding")),
                "FindingReasonCode" => reason_codes(recommendation)
                    .iter()
                    .any(|code| values.contains(code)),
                "LicenseName" => values.contains(&text(
                    &recommendation["currentLicenseConfiguration"],
                    "licenseName",
                )),
                _ => false,
            }
        })
}

/// Paging never changes WHAT is reported: walking the recommendations `maxResults` at a time
/// yields exactly the unpaged list, in the same order, with a `nextToken` on every page but
/// the last and none on it. A token this service did not issue — including one issued for a
/// different selection — is refused.
#[tokio::test]
async fn paging_walks_the_same_recommendations_in_the_same_order() {
    let router = emulator();
    analysed(&router).await;

    let unpaged = reported(&recommendations(&router, json!({})).await);
    assert!(
        unpaged.len() > 2,
        "this test needs several recommendations to page over, got {}",
        unpaged.len()
    );

    // Rows: the smallest page, one that divides the set unevenly, one that holds it all, the
    // modelled upper bound of `maxResults`, its lower bound `0` (no size asked for) and the
    // member left out entirely.
    let page_sizes: [Option<i64>; 6] = [
        Some(1),
        Some(2),
        Some(unpaged.len() as i64),
        Some(1000),
        Some(0),
        None,
    ];
    for page_size in page_sizes {
        let case = format!("maxResults={page_size:?}");
        let mut walked: Vec<Value> = Vec::new();
        let mut next_token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut request = json!({});
            if let Some(size) = page_size {
                request["maxResults"] = json!(size);
            }
            if let Some(token) = &next_token {
                request["nextToken"] = json!(token);
            }
            let body = recommendations(&router, request).await;
            let page = reported(&body);
            pages += 1;
            assert!(
                pages <= unpaged.len() + 1,
                "case {case:?}: paging must terminate, got {pages} pages"
            );
            if let Some(size) = page_size.filter(|size| *size > 0) {
                assert!(
                    page.len() as i64 <= size,
                    "case {case:?}: a page holds at most {size} recommendations, got {}",
                    page.len()
                );
            }
            walked.extend(page);
            next_token = body["nextToken"].as_str().map(ToString::to_string);
            if next_token.is_none() {
                break;
            }
        }
        assert_eq!(
            walked, unpaged,
            "case {case:?}: paging must walk exactly the unpaged recommendations, in order"
        );
        let expected_pages = match page_size.filter(|size| *size > 0) {
            Some(size) => unpaged.len().div_ceil(size as usize),
            None => 1,
        };
        assert_eq!(
            pages, expected_pages,
            "case {case:?}: expected {expected_pages} page(s) for {} recommendations, got {pages}",
            unpaged.len()
        );
    }

    // A token taken with one selection names no page of another: resuming the walk with
    // different filters is refused rather than answered with a page of a different set.
    let first = recommendations(&router, json!({"maxResults": 1})).await;
    let token = first["nextToken"]
        .as_str()
        .unwrap_or_else(|| panic!("the first page of many must carry a token, got {first}"))
        .to_string();
    // Rows: a token that is not hex, one that is hex but not a token this service issued, a
    // value a client might build from a resource it can see, and this service's own token
    // presented with a different selection.
    for (case, request) in [
        ("not hex", json!({"nextToken": "not-a-token"})),
        ("hex, not a token", json!({"nextToken": "6e6f742d612d746f6b656e"})),
        (
            "an ARN as a token",
            json!({"nextToken": instance_arn(OVER_PROVISIONED_LICENSE_INCLUDED)}),
        ),
        (
            "a token of another selection",
            json!({
                "maxResults": 1,
                "nextToken": token.clone(),
                "filters": [{"name": "Finding", "values": ["Optimized"]}],
            }),
        ),
    ] {
        let (status, body) = call(&router, "GetLicenseRecommendations", request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a token this service did not issue for this selection names no \
             page, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidParameterValueException",
            "case {case:?}: expected InvalidParameterValueException, got {body}"
        );
    }
}

/// The two readers of one license agree: `GetRecommendationSummaries` counts exactly the
/// licenses this reader reports at each `LicenseFinding` — `InsufficientMetrics`, the
/// classification of a license whose metrics the analysis could not read, included — and a
/// license summary states those counts alone, because a license's analysis reaches a
/// classification and no recommended configuration of its own to price or rate.
#[tokio::test]
async fn the_summary_of_the_account_s_licenses_agrees_with_this_reader() {
    let router = emulator();
    analysed(&router).await;

    let reported = reported(&recommendations(&router, json!({})).await);
    let (status, summaries) = call(&router, "GetRecommendationSummaries", json!({})).await;
    assert_eq!(
        status, 200,
        "reading the summaries: expected 200, got {status}: {summaries}"
    );
    let license_summary = summaries["recommendationSummaries"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .find(|summary| summary["recommendationResourceType"] == json!("License"))
        .unwrap_or_else(|| panic!("the account's licenses must be summarised, got {summaries}"));

    // Rows: each `LicenseFinding` a license summary reports a count at.
    for finding in FINDINGS {
        let counted = license_summary["summaries"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .find(|entry| entry["name"] == json!(finding))
            .map(|entry| number(&entry, "value"))
            .unwrap_or_else(|| panic!("case {finding:?}: no count in {license_summary}"));
        let expected = reported
            .iter()
            .filter(|recommendation| text(recommendation, "finding") == *finding)
            .count() as f64;
        assert!(
            expected > 0.0,
            "case {finding:?}: this fixture must hold a license at that finding for the count \
             to be observable, got {expected}"
        );
        assert_eq!(
            counted, expected,
            "case {finding:?}: the summary counts the licenses this reader reports at that \
             finding, expected {expected}, got {counted}"
        );
    }

    // What a license recommendation would save belongs to the OPTION it recommends
    // (`LicenseRecommendationOption$savingsOpportunity`), and a license carries no
    // performance risk at all, so the summary prices and rates nothing beside its counts.
    for block in ["savingsOpportunity", "currentPerformanceRiskRatings"] {
        assert!(
            license_summary[block].is_null(),
            "a license summary reports no {block}, got {license_summary}"
        );
    }
}

/// Reading license recommendations is a READ: two identical calls answer identically, and
/// the read moves neither the enrollment record nor the stored preferences.
#[tokio::test]
async fn reading_license_recommendations_changes_nothing_a_later_caller_sees() {
    let router = emulator();
    analysed(&router).await;

    let before = call(&router, "GetEnrollmentStatus", json!({})).await.1;
    let first = recommendations(&router, json!({})).await;
    let second = recommendations(&router, json!({})).await;
    assert_eq!(
        first, second,
        "two reads of the same observation must answer identically"
    );

    let after = call(&router, "GetEnrollmentStatus", json!({})).await.1;
    assert_eq!(
        before, after,
        "reading license recommendations must not move the enrollment record"
    );
    let preferences = call(
        &router,
        "GetRecommendationPreferences",
        json!({"resourceType": "License"}),
    )
    .await
    .1;
    assert_eq!(
        preferences["recommendationPreferencesDetails"],
        json!([]),
        "reading license recommendations must not write a preference, got {preferences}"
    );
}
