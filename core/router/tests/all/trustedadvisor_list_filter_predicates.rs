// SPDX-License-Identifier: CC0-1.0

//! The ONE pair of list-filter predicates every Trusted Advisor list selects
//! with, driven over HTTP through the same `MultiServiceRouter` the emulator
//! serves with.
//!
//! RULE under test: **a (recommendation, flagged resource) row of
//! `ListRecommendationsForResource` is kept by the SAME two predicates its
//! sibling lists select with — the resource predicate the resource lists apply
//! to a stored resource (`status`) and the recommendation predicate the
//! recommendation listings apply to a stored recommendation (`pillar`
//! membership, check equality) — so for one filter value the three reads cannot
//! select differently.** Each row is therefore reported exactly when
//! `ListRecommendationResources` keeps that resource under the same `status`
//! and `ListRecommendations` keeps that recommendation under the same `pillar`
//! and check, and the responses themselves are unchanged by the predicates
//! being stated once.
//!
//! Every case is a row of a TABLE that spans what the rule generalises over:
//! which filter is in force, the value it names (every `ResourceStatus` and
//! every `RecommendationPillar` the model declares, each seeded check, and a
//! check ARN naming no check), how many filters combine, and how many rows
//! survive (all, one, none). Each expectation is stated from the seeded store
//! (its source is the `SEED` table in the service crate's `src/state.rs`, and
//! the per-resource-type ARN forms AWS publishes), and is then ALSO required to
//! agree with what the two sibling reads answer — which is the invariant a
//! second copy of any of the three tests would break.
//!
//! Why it lives beside the router's suites rather than in the service crate:
//! this suite needs no declaration here, while that crate's integration-test
//! target declares each of its suites by hand and this change adds none. The
//! service is mounted exactly as the router mounts it, so the three reads are
//! observed over the wire either way.
//!
//! Every call is made as an account whose support plan includes the Trusted
//! Advisor API; the gate ahead of each read is pinned by the service crate's
//! own suites.

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::Value;
use service_com_amazonaws_trustedadvisor_trustedadvisor::ops::types::{
    RECOMMENDATION_PILLAR_VALUES, RESOURCE_STATUS_VALUES,
};
use service_com_amazonaws_trustedadvisor_trustedadvisor::state::{service_state, SupportPlan};
use service_com_amazonaws_trustedadvisor_trustedadvisor::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 `Authorization` header whose credential scope names this service, so
/// the router claims the request for it. Request authentication is off in this
/// suite, so the signature itself is never verified.
const SIGV4: &str = "AWS4-HMAC-SHA256 Credential=AKIAEMULATOR0000000/20260101/us-east-1/\
                     trustedadvisor/aws4_request, SignedHeaders=host, Signature=00";

/// `arn:aws:trustedadvisor:::check/<id>` — the ARN pattern `CheckArn` declares
/// (no account segment).
fn check_arn(check_id: &str) -> String {
    format!("arn:aws:trustedadvisor:::check/{check_id}")
}

/// `arn:aws:trustedadvisor::<account>:recommendation/<id>` — the ARN pattern
/// `AccountRecommendationIdentifier` declares. Each seeded check produced the
/// recommendation `<check id>-recommendation`.
fn recommendation_arn(check_id: &str) -> String {
    format!(
        "arn:aws:trustedadvisor::{}:recommendation/{check_id}-recommendation",
        engine::EMULATOR_ACCOUNT_ID
    )
}

/// `arn:aws:trustedadvisor::<account>:recommendation-resource/<rec>/<resource>`
/// — the ARN pattern `RecommendationResourceArn` declares.
fn resource_arn(check_id: &str, resource_id: &str) -> String {
    format!(
        "arn:aws:trustedadvisor::{}:recommendation-resource/{check_id}-recommendation/{resource_id}",
        engine::EMULATOR_ACCOUNT_ID
    )
}

/// One stored (recommendation, flagged resource) pair this suite drives: the
/// check behind it, the stored resource, the ARN of the AWS resource that
/// resource flags, the resource's stored `status` and the recommendation's
/// pillars.
///
/// Source: the `SEED` table in the service crate's `src/state.rs`, plus the ARN
/// form AWS publishes for each flagged resource type
/// (`AWS::EC2::Instance` → `arn:aws:ec2:<region>:<account>:instance/<id>`,
/// `AWS::EC2::Volume` → `arn:aws:ec2:<region>:<account>:volume/<id>`,
/// `AWS::RDS::DBInstance` → `arn:aws:rds:<region>:<account>:db:<name>`).
struct Flagged {
    check_id: &'static str,
    resource_id: &'static str,
    /// The AWS resource ARN, with `{account}` standing for the calling account.
    aws_resource_arn: &'static str,
    /// The `ResourceStatus` this suite stores on the resource.
    status: &'static str,
    pillars: &'static [&'static str],
}

impl Flagged {
    /// The ARN of the AWS resource this pair is addressed by.
    fn aws_resource_arn(&self) -> String {
        self.aws_resource_arn
            .replace("{account}", engine::EMULATOR_ACCOUNT_ID)
    }

    /// The stored recommendation resource's own ARN, which the resource lists
    /// report it as.
    fn resource_arn(&self) -> String {
        resource_arn(self.check_id, self.resource_id)
    }

    /// The recommendation that flagged it.
    fn recommendation_arn(&self) -> String {
        recommendation_arn(self.check_id)
    }
}

/// Every seeded pair an AWS resource ARN addresses, spanning three resource
/// types with three different ARN forms, four Regions, all three
/// `ResourceStatus` values, a single-pillar and a two-pillar recommendation, and
/// a recommendation with one and with three flagged resources. The
/// `iam-access-key-rotation` check is deliberately absent: it is not
/// `resourceArnQueryable`, so no ARN addresses its finding.
const FLAGGED: &[Flagged] = &[
    Flagged {
        check_id: "low-utilization-ec2-instances",
        resource_id: "res-ec2-0001",
        aws_resource_arn: "arn:aws:ec2:us-east-1:{account}:instance/i-0abc1de23f4567890",
        status: "warning",
        pillars: &["cost_optimizing"],
    },
    Flagged {
        check_id: "low-utilization-ec2-instances",
        resource_id: "res-ec2-0002",
        aws_resource_arn: "arn:aws:ec2:us-west-2:{account}:instance/i-0abc2de23f4567891",
        status: "ok",
        pillars: &["cost_optimizing"],
    },
    Flagged {
        check_id: "low-utilization-ec2-instances",
        resource_id: "res-ec2-0003",
        aws_resource_arn: "arn:aws:ec2:eu-west-1:{account}:instance/i-0abc3de23f4567892",
        // Stored by this suite, so the predicate is shown to follow the STORE
        // rather than the seed (the seed holds `warning` here).
        status: "error",
        pillars: &["cost_optimizing"],
    },
    Flagged {
        check_id: "amazon-ebs-snapshots",
        resource_id: "res-ebs-0001",
        aws_resource_arn: "arn:aws:ec2:us-east-1:{account}:volume/vol-0ea1b2c3d4e5f6789",
        status: "error",
        pillars: &["fault_tolerance"],
    },
    Flagged {
        check_id: "amazon-ebs-snapshots",
        resource_id: "res-ebs-0002",
        aws_resource_arn: "arn:aws:ec2:ap-southeast-1:{account}:volume/vol-0ea2b2c3d4e5f6789",
        status: "ok",
        pillars: &["fault_tolerance"],
    },
    Flagged {
        check_id: "rds-idle-db-instances",
        resource_id: "res-rds-0001",
        // An RDS DB instance ARN separates its name with `:`, not `/`.
        aws_resource_arn: "arn:aws:rds:us-east-1:{account}:db:reporting-prod",
        status: "warning",
        pillars: &["cost_optimizing", "performance"],
    },
    Flagged {
        check_id: "rds-idle-db-instances",
        resource_id: "res-rds-0002",
        aws_resource_arn: "arn:aws:rds:us-west-2:{account}:db:analytics-staging",
        status: "ok",
        pillars: &["cost_optimizing", "performance"],
    },
];

/// A check ARN naming no stored check, so every filter value has a row that
/// keeps nothing. Satisfies `CheckArn`'s pattern and both its length bounds.
const UNKNOWN_CHECK_ID: &str = "no-such-check";

/// One row of the table: which of the three filters are in force.
struct Case {
    name: &'static str,
    status: Option<&'static str>,
    pillar: Option<&'static str>,
    /// The check this row filters on, as a check id, or an ARN naming no check.
    check_id: Option<&'static str>,
}

impl Case {
    /// Whether this row's filters keep `pair`, by the rule stated from the
    /// table's own values: `status` equals the resource's stored status,
    /// `pillar` is a member of the recommendation's pillars, and the check is
    /// the one behind the recommendation.
    fn keeps(&self, pair: &Flagged) -> bool {
        self.status.is_none_or(|status| pair.status == status)
            && self
                .pillar
                .is_none_or(|pillar| pair.pillars.contains(&pillar))
            && self.check_id.is_none_or(|check| pair.check_id == check)
    }

    /// The query `ListRecommendationsForResource` is called with, under the
    /// member names that operation models.
    fn for_resource_query(&self) -> String {
        let mut parameters = Vec::new();
        if let Some(status) = self.status {
            parameters.push(format!("status={status}"));
        }
        if let Some(pillar) = self.pillar {
            parameters.push(format!("pillar={pillar}"));
        }
        if let Some(check) = self.check_id {
            parameters.push(format!("checkArn={}", label(&check_arn(check))));
        }
        query(&parameters)
    }

    /// The query `ListRecommendationResources` is called with: the one filter of
    /// this row that selects over a stored RESOURCE.
    fn resource_list_query(&self) -> String {
        let parameters: Vec<String> = self
            .status
            .map(|status| format!("status={status}"))
            .into_iter()
            .collect();
        query(&parameters)
    }

    /// The query `ListRecommendations` is called with: the two filters of this
    /// row that select over a stored RECOMMENDATION, under the member names
    /// THAT operation models (`checkIdentifier` for the check).
    fn recommendation_list_query(&self) -> String {
        let mut parameters = Vec::new();
        if let Some(pillar) = self.pillar {
            parameters.push(format!("pillar={pillar}"));
        }
        if let Some(check) = self.check_id {
            parameters.push(format!("checkIdentifier={}", label(&check_arn(check))));
        }
        query(&parameters)
    }
}

/// `?a=1&b=2`, or the empty string when no filter is in force.
fn query(parameters: &[String]) -> String {
    if parameters.is_empty() {
        String::new()
    } else {
        format!("?{}", parameters.join("&"))
    }
}

/// One ARN as it goes into a path label or a query value: the characters that
/// would otherwise split the segment or be decoded as something else.
fn label(value: &str) -> String {
    value
        .replace('%', "%25")
        .replace('/', "%2F")
        .replace('+', "%2B")
        .replace('=', "%3D")
}

/// A mounted emulator holding this suite's statuses, with the account on a
/// support plan that includes the Trusted Advisor API.
///
/// Both are written in process, the way the part of Trusted Advisor that runs a
/// check — and the subscription outside this API — would: no modelled operation
/// creates a recommendation resource, moves its status or changes a plan.
fn emulator() -> MultiServiceRouter {
    let engine = Arc::new(engine::InMemoryEngine::new());
    {
        let state =
            service_state(&*engine).expect("the in-memory engine carries this service's state");
        let mut state = state.lock();
        state.set_support_plan(SupportPlan::Business);
        for pair in FLAGGED {
            let arn = pair.resource_arn();
            let resource = state
                .recommendation_resources
                .get_mut(&arn)
                .unwrap_or_else(|| panic!("no seeded resource {arn}"));
            resource.status = pair.status.to_string();
        }
    }
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// Drive one GET through the router and read its status and JSON body.
async fn get(router: &MultiServiceRouter, path_and_query: &str) -> (u16, Value) {
    let request = http::Request::builder()
        .method("GET")
        .uri(format!(
            "https://trustedadvisor.us-east-1.amazonaws.com{path_and_query}"
        ))
        .header("authorization", SIGV4)
        .header("content-type", "application/json")
        .body(bytes::Bytes::new())
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let body = response.into_body();
    let json = if body.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&body).expect("a JSON response body")
    };
    (status, json)
}

/// The `field` of every member of the `list` in a 200 answer to `path`.
async fn listed(
    router: &MultiServiceRouter,
    case: &str,
    path_and_query: &str,
    list: &str,
    field: &str,
) -> Vec<String> {
    let (status, body) = get(router, path_and_query).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 from GET {path_and_query}, got {status} with {body}"
    );
    body[list]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected a {list} array, got {body}"))
        .iter()
        .map(|item| {
            item[field]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: a {list} item with no {field}: {item}"))
                .to_string()
        })
        .collect()
}

/// The rows of the table, differing along which filter is in force, the value
/// it names, how many filters combine and how many pairs survive.
fn cases() -> Vec<Case> {
    let mut cases = vec![Case {
        name: "no filter",
        status: None,
        pillar: None,
        check_id: None,
    }];
    // Every value the two filtered enums declare, so a value added to either is
    // not silently left untested.
    for status in RESOURCE_STATUS_VALUES {
        cases.push(Case {
            name: "one resource status",
            status: Some(status),
            pillar: None,
            check_id: None,
        });
    }
    for pillar in RECOMMENDATION_PILLAR_VALUES {
        cases.push(Case {
            name: "one pillar",
            status: None,
            pillar: Some(pillar),
            check_id: None,
        });
    }
    // Each seeded check, including the one no ARN addresses, and a check ARN
    // naming nothing at all.
    for check in [
        "low-utilization-ec2-instances",
        "amazon-ebs-snapshots",
        "rds-idle-db-instances",
        "iam-access-key-rotation",
        UNKNOWN_CHECK_ID,
    ] {
        cases.push(Case {
            name: "one check",
            status: None,
            pillar: None,
            check_id: Some(check),
        });
    }
    // Combinations: two filters that hold together, two that cannot, and all
    // three at once.
    cases.extend([
        Case {
            name: "a status and a pillar that hold together",
            status: Some("warning"),
            pillar: Some("cost_optimizing"),
            check_id: None,
        },
        Case {
            name: "a status and a pillar that cannot hold together",
            status: Some("ok"),
            pillar: Some("service_limits"),
            check_id: None,
        },
        Case {
            name: "a pillar and a check of two different recommendations",
            status: None,
            pillar: Some("fault_tolerance"),
            check_id: Some("rds-idle-db-instances"),
        },
        Case {
            name: "a pillar a two-pillar recommendation carries, and its check",
            status: None,
            pillar: Some("performance"),
            check_id: Some("rds-idle-db-instances"),
        },
        Case {
            name: "all three filters on one pair",
            status: Some("error"),
            pillar: Some("fault_tolerance"),
            check_id: Some("amazon-ebs-snapshots"),
        },
        Case {
            name: "all three filters, with a status no row of that check holds",
            status: Some("ok"),
            pillar: Some("fault_tolerance"),
            check_id: Some("low-utilization-ec2-instances"),
        },
    ]);
    cases
}

#[tokio::test]
async fn the_three_reads_select_one_resource_and_one_recommendation_alike() {
    let router = emulator();
    let cases = cases();

    // The table must span both filtered enums, so a value added to either is
    // not silently left untested.
    for status in RESOURCE_STATUS_VALUES {
        assert!(
            cases.iter().any(|case| case.status == Some(*status)),
            "the table must cover the resource status {status}"
        );
    }
    for pillar in RECOMMENDATION_PILLAR_VALUES {
        assert!(
            cases.iter().any(|case| case.pillar == Some(*pillar)),
            "the table must cover the pillar {pillar}"
        );
    }

    // How many pairs the table kept and cut in total, so an answer that was
    // empty — or complete — throughout could not satisfy it.
    let (mut kept_total, mut cut_total) = (0_usize, 0_usize);

    for case in &cases {
        let for_resource_query = case.for_resource_query();
        let row = format!("{} {for_resource_query:?}", case.name);

        // What the SIBLING reads select under this row's filters: the resource
        // lists of each flagged recommendation under its resource filter, and
        // the recommendation listing under its recommendation filters.
        let recommendations: BTreeSet<String> = listed(
            &router,
            &row,
            &format!("/v1/recommendations{}", case.recommendation_list_query()),
            "recommendationSummaries",
            "arn",
        )
        .await
        .into_iter()
        .collect();

        for pair in FLAGGED {
            let case_label = format!("{row} / {}", pair.aws_resource_arn());
            let resources: BTreeSet<String> = listed(
                &router,
                &case_label,
                &format!(
                    "/v1/recommendations/{}/resources{}",
                    label(&pair.recommendation_arn()),
                    case.resource_list_query()
                ),
                "recommendationResourceSummaries",
                "arn",
            )
            .await
            .into_iter()
            .collect();

            // The rule, read off the two sibling reads: this pair survives
            // exactly when the resource list keeps its resource AND the
            // recommendation listing keeps its recommendation.
            let kept_by_siblings = resources.contains(&pair.resource_arn())
                && recommendations.contains(&pair.recommendation_arn());
            let kept = case.keeps(pair);
            assert_eq!(
                kept_by_siblings, kept,
                "case {case_label:?}: the sibling reads must keep this pair exactly when the \
                 stated filters do, expected {kept}, got {kept_by_siblings}"
            );

            // And the read under test selects the same way, reporting the one
            // recommendation that flagged this resource, or nothing.
            let reported = listed(
                &router,
                &case_label,
                &format!(
                    "/v1/recommendations-for-resource/{}{for_resource_query}",
                    label(&pair.aws_resource_arn())
                ),
                "recommendationForResourceSummaries",
                "recommendationArn",
            )
            .await;
            let expected: Vec<String> = if kept {
                vec![pair.recommendation_arn()]
            } else {
                Vec::new()
            };
            assert_eq!(
                reported, expected,
                "case {case_label:?}: expected {expected:?}, got {reported:?}"
            );

            if kept {
                kept_total += 1;
            } else {
                cut_total += 1;
            }
        }
    }

    assert!(
        kept_total > 0 && cut_total > 0,
        "the table must both keep and cut pairs, kept {kept_total} and cut {cut_total}"
    );
}
