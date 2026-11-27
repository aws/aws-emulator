// SPDX-License-Identifier: CC0-1.0

//! Amazon ECS task definitions and Amazon EventBridge rules are REGIONAL: each exists only
//! in the Region whose endpoint created it, and a lookup from another Region finds nothing.
//!
//! The rule under test spans both services, which is why it lives here rather than in
//! either crate: a client that builds a rule and a task definition in one Region and then
//! points a client at another Region must find NEITHER there.
//!
//! * Amazon ECS: a task-definition revision belongs to the one Region it was registered in,
//!   and every ARN that Region's endpoint reports carries that Region. So another Region's
//!   endpoint lists none of it (`ListTaskDefinitions` is "a list of task definitions that
//!   are registered to your account" — of the account AND Region it is asked of), reports
//!   none of its families (`ListTaskDefinitionFamilies`), and resolves none of its
//!   references (`DescribeTaskDefinition` and `RunTask` answer for an identifier they know
//!   nothing about, and refuse one whose ARN names another Region as the invalid parameter
//!   it is). Revision numbering is per family per Region too: "when you register a
//!   task definition for the first time, the revision is 1" holds of the family in THAT
//!   Region, so one family name registered in two Regions is revision 1 in each.
//! * Amazon EventBridge: a rule lives on an event bus of one Region, so `DescribeRule` in
//!   another Region answers `ResourceNotFoundException` and `ListRules` names it nowhere.
//!
//! The rows span what the rule generalises over: three Regions across two geographic
//! prefixes, how many revisions each Region holds, which operation does the looking, and
//! which form the reference takes (a family alone, a `family:revision`, and the full ARN
//! another Region's endpoint reported).

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The Region every resource below is created in.
const HOME: &str = "us-west-2";

/// The Regions that hold none of them: one of the same geographic prefix and one of
/// another, so nothing here depends on a Region name.
const FAR_REGIONS: [&str; 2] = ["us-east-2", "eu-west-1"];

/// The account every ARN below belongs to — the emulator's own.
fn account() -> String {
    engine::emulator_account_id().to_string()
}

/// The family the revisions below belong to.
const FAMILY: &str = "s2r-regional-td";

/// The rule the EventBridge half creates.
const RULE: &str = "s2r-regional-rule";

/// The access key id every request presents. The router runs with request authentication
/// off, so only the credential scope is read: its service field routes the request and its
/// Region field is the Region the service serves it in.
const ACCESS_KEY: &str = "AKIDECSEVENTSREGIONS";

/// The emulator with Amazon ECS and EventBridge mounted on ONE engine, as
/// `core/router/src/lib.rs` mounts them.
fn build_router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_ecs_amazonec2containerservicev20141113::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_eventbridge_awsevents::EmulatedService::new(engine),
    );
    router
}

/// One `awsJson1_1` call through `region`'s endpoint of the service whose signing name is
/// `signing_name`, answered with its status and body.
async fn rpc(
    router: &MultiServiceRouter,
    region: &str,
    signing_name: &str,
    target_prefix: &str,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("{signing_name}.{region}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("{target_prefix}.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential={ACCESS_KEY}/20260101/{region}/{signing_name}\
                 /aws4_request, SignedHeaders=host, Signature=0000"
            ),
        )
        .body(bytes::Bytes::from(body.to_string()))
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = response.into_body();
    let parsed = if payload.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice::<Value>(&payload).unwrap_or_else(|error| {
            panic!(
                "{operation} answered a body that is not JSON ({error}): {:?}",
                String::from_utf8_lossy(&payload)
            )
        })
    };
    (status, parsed)
}

/// One Amazon ECS call through `region`'s endpoint.
async fn ecs(
    router: &MultiServiceRouter,
    region: &str,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    rpc(
        router,
        region,
        "ecs",
        "AmazonEC2ContainerServiceV20141113",
        operation,
        body,
    )
    .await
}

/// One Amazon ECS call that must succeed.
async fn ecs_ok(
    router: &MultiServiceRouter,
    region: &str,
    operation: &str,
    body: &Value,
    case: &str,
) -> Value {
    let (status, answer) = ecs(router, region, operation, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: ecs {operation} in {region} answered {status} {answer}"
    );
    answer
}

/// One EventBridge call through `region`'s endpoint.
async fn events(
    router: &MultiServiceRouter,
    region: &str,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    rpc(router, region, "events", "AWSEvents", operation, body).await
}

/// Register one revision of [`FAMILY`] through `region`'s endpoint; answers the ARN it was
/// registered under.
async fn register_in(router: &MultiServiceRouter, region: &str, case: &str) -> String {
    let registered = ecs_ok(
        router,
        region,
        "RegisterTaskDefinition",
        &json!({
            "family": FAMILY,
            "containerDefinitions": [{
                "name": "app",
                "image": "public.ecr.aws/nginx/nginx:latest",
                "essential": true,
                "memory": 256,
            }],
        }),
        case,
    )
    .await;
    registered["taskDefinition"]["taskDefinitionArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no taskDefinitionArn in {registered}"))
        .to_string()
}

/// The task-definition ARNs `region`'s endpoint lists.
async fn listed_in(router: &MultiServiceRouter, region: &str, case: &str) -> Vec<String> {
    let listed = ecs_ok(router, region, "ListTaskDefinitions", &json!({}), case).await;
    listed["taskDefinitionArns"]
        .as_array()
        .map(|arns| {
            arns.iter()
                .filter_map(|arn| arn.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// The families `region`'s endpoint lists.
async fn families_in(router: &MultiServiceRouter, region: &str, case: &str) -> Vec<String> {
    let listed = ecs_ok(router, region, "ListTaskDefinitionFamilies", &json!({}), case).await;
    listed["families"]
        .as_array()
        .map(|families| {
            families
                .iter()
                .filter_map(|family| family.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// A revision registered in one Region is listed by THAT Region's endpoint and by no other,
/// and the ARN each endpoint reports carries its own Region — over a table whose rows differ
/// along how many revisions each Region holds.
#[tokio::test]
async fn one_regions_task_definitions_are_listed_by_that_region_alone() {
    // (case, how many revisions are registered in HOME, how many in each far Region)
    let rows: [(&str, usize, usize); 4] = [
        ("one revision at home and none far away", 1, 0),
        ("three revisions at home and none far away", 3, 0),
        ("none at home and one in each far Region", 0, 1),
        ("two at home and two in each far Region", 2, 2),
    ];

    for (case, home_revisions, far_revisions) in rows {
        let router = build_router();
        for _ in 0..home_revisions {
            register_in(&router, HOME, case).await;
        }
        for far in FAR_REGIONS {
            for _ in 0..far_revisions {
                register_in(&router, far, case).await;
            }
        }

        let expected_per_region = [(HOME, home_revisions)]
            .into_iter()
            .chain(FAR_REGIONS.map(|far| (far, far_revisions)));
        for (region, expected) in expected_per_region {
            let listed = listed_in(&router, region, case).await;
            assert_eq!(
                listed.len(),
                expected,
                "case {case:?}: expected {region} to list {expected} revision(s), got {listed:?}"
            );
            let prefix = format!("arn:aws:ecs:{region}:{}:task-definition/", account());
            for arn in &listed {
                assert!(
                    arn.starts_with(&prefix),
                    "case {case:?}: an ARN {region} serves carries {region}, got {arn}"
                );
            }
            // A family exists in a Region exactly while that Region holds a revision of it.
            let families = families_in(&router, region, case).await;
            let wanted: Vec<String> = if expected == 0 {
                Vec::new()
            } else {
                vec![FAMILY.to_string()]
            };
            assert_eq!(
                families, wanted,
                "case {case:?}: expected {region} to report {wanted:?}, got {families:?}"
            );
        }
    }
}

/// Revision numbering is per family PER REGION: the first revision of a family registered
/// against a Region's endpoint is 1 there, however many revisions the same family name has
/// elsewhere — and each Region's counter goes on increasing by one on its own.
#[tokio::test]
async fn each_regions_task_definition_revisions_are_numbered_from_one() {
    let case = "one family name registered in three Regions";
    let router = build_router();

    let mut home = Vec::new();
    for _ in 0..3 {
        home.push(register_in(&router, HOME, case).await);
    }
    assert_eq!(
        home,
        (1..=3)
            .map(|revision| task_definition_arn(HOME, revision))
            .collect::<Vec<String>>(),
        "case {case:?}: the home Region numbers its own revisions from one, got {home:?}"
    );

    for far in FAR_REGIONS {
        let mut registered = Vec::new();
        for _ in 0..2 {
            registered.push(register_in(&router, far, case).await);
        }
        assert_eq!(
            registered,
            (1..=2)
                .map(|revision| task_definition_arn(far, revision))
                .collect::<Vec<String>>(),
            "case {case:?}: {far} numbers its own revisions from one too, got {registered:?}"
        );
    }

    // And a later registration at home carries on from where THAT Region was.
    let next = register_in(&router, HOME, case).await;
    assert_eq!(
        next,
        task_definition_arn(HOME, 4),
        "case {case:?}: the home Region's counter is its own, got {next}"
    );
}

/// The ARN `region`'s endpoint reports for revision `revision` of [`FAMILY`].
fn task_definition_arn(region: &str, revision: i32) -> String {
    format!(
        "arn:aws:ecs:{region}:{}:task-definition/{FAMILY}:{revision}",
        account()
    )
}

/// What a LOOKUP from another Region answers: a reference to a revision of one Region names
/// nothing in another, whichever form it takes and whichever operation resolves it — while
/// the Region that holds it resolves every form, which is the control.
#[tokio::test]
async fn a_task_definition_lookup_from_another_region_resolves_nothing() {
    let case = "a revision of the home Region, looked up far away";
    let router = build_router();
    let registered = register_in(&router, HOME, case).await;

    // Each form, with the refusal it earns away from home. A family or a `family:revision`
    // carries no Region of its own, so away from home it names a revision that endpoint holds
    // none of — the client fault about an identifier it knows nothing about. A full ARN
    // carries the home Region IN the identifier, so it is the parameter that is wrong rather
    // than a revision that is absent, and it is refused as such before anything is resolved.
    let references = [
        // The family alone, which names "the latest ACTIVE revision in that family".
        (FAMILY.to_string(), "ClientException"),
        // The family and the revision.
        (format!("{FAMILY}:1"), "ClientException"),
        // The full ARN the home Region reported, Region label and all.
        (registered.clone(), "InvalidParameterException"),
    ];

    for (reference, expected_code) in references {
        // The home Region resolves every one of them.
        let described = ecs_ok(
            &router,
            HOME,
            "DescribeTaskDefinition",
            &json!({ "taskDefinition": reference }),
            case,
        )
        .await;
        assert_eq!(
            described["taskDefinition"]["taskDefinitionArn"].as_str(),
            Some(registered.as_str()),
            "case {case:?}: the home Region resolves {reference:?}, got {described}"
        );

        for far in FAR_REGIONS {
            let (status, answer) = ecs(
                &router,
                far,
                "DescribeTaskDefinition",
                &json!({ "taskDefinition": reference.clone() }),
            )
            .await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected {far} to resolve nothing for {reference:?}, got \
                 {status} with {answer}"
            );
            assert_refused_as(&answer, expected_code, &reference, far, case);

            // And no task of it can be run there either: there is no revision to run.
            let cluster = "s2r-regional-cluster";
            ecs_ok(
                &router,
                far,
                "CreateCluster",
                &json!({ "clusterName": cluster }),
                case,
            )
            .await;
            let (status, answer) = ecs(
                &router,
                far,
                "RunTask",
                &json!({ "cluster": cluster, "taskDefinition": reference.clone() }),
            )
            .await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected {far} to run nothing for {reference:?}, got {status} \
                 with {answer}"
            );
            assert_refused_as(&answer, expected_code, &reference, far, case);
        }
    }
}

/// The refusal a Region that resolves nothing for `reference` answers: the error code that
/// reference's FORM earns there — a client fault for an identifier the endpoint knows nothing
/// about, and the invalid parameter for one whose ARN names another Region.
fn assert_refused_as(
    answer: &Value,
    expected_code: &str,
    reference: &str,
    region: &str,
    case: &str,
) {
    let wire_type = answer["__type"].as_str().unwrap_or_default();
    assert!(
        wire_type.contains(expected_code),
        "case {case:?}: expected a {expected_code} from {region} for {reference:?}, got {answer}"
    );
}

/// The EventBridge half of the same rule, over the same engine: a rule created in one Region
/// is described and listed by that Region alone, so a client pointed at another Region finds
/// neither the rule nor the task definition it would have run.
#[tokio::test]
async fn a_rule_of_one_region_is_not_found_in_another() {
    let case = "a rule created at home, looked up far away";
    let router = build_router();
    let registered = register_in(&router, HOME, case).await;

    let (status, created) = events(
        &router,
        HOME,
        "PutRule",
        &json!({
            "Name": RULE,
            "EventPattern": json!({ "source": ["com.demo.regional"] }).to_string(),
            "State": "ENABLED",
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: PutRule in {HOME} answered {status} {created}"
    );
    let rule_arn = created["RuleArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no RuleArn in {created}"))
        .to_string();
    assert!(
        rule_arn.starts_with(&format!("arn:aws:events:{HOME}:{}:rule/", account())),
        "case {case:?}: the rule's ARN carries the Region that created it, got {rule_arn}"
    );

    // The home Region reports both resources.
    let (status, described) = events(&router, HOME, "DescribeRule", &json!({ "Name": RULE })).await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeRule in {HOME} answered {status} {described}"
    );
    assert_eq!(
        listed_in(&router, HOME, case).await,
        vec![registered],
        "case {case:?}: the home Region lists its own revision"
    );

    for far in FAR_REGIONS {
        let (status, answer) = events(&router, far, "DescribeRule", &json!({ "Name": RULE })).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected {far} to hold no such rule, got {status} with {answer}"
        );
        let wire_type = answer["__type"].as_str().unwrap_or_default();
        assert!(
            wire_type.contains("ResourceNotFoundException"),
            "case {case:?}: expected a ResourceNotFoundException from {far}, got {answer}"
        );
        let (status, listed) = events(&router, far, "ListRules", &json!({})).await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListRules in {far} answered {status} {listed}"
        );
        assert_eq!(
            listed["Rules"].as_array().map(Vec::len).unwrap_or_default(),
            0,
            "case {case:?}: expected {far} to list no rule, got {listed}"
        );
        assert!(
            listed_in(&router, far, case).await.is_empty(),
            "case {case:?}: expected {far} to list no task definition either"
        );
    }
}
