// SPDX-License-Identifier: CC0-1.0

//! `ListServicesByNamespace`: the services of ONE Cloud Map namespace, across clusters.
//!
//! The rules under test, each asserted over a table that spans its class rather than over
//! one request:
//!
//! * "This operation lists all of the services that are associated with a Cloud Map
//!   namespace. This list might include services in different clusters" — what selects a
//!   service is its Service Connect namespace and not the cluster it was created in, and a
//!   service that uses no Service Connect, one in another namespace and one the caller
//!   deleted are none of the namespace's services;
//! * `ServiceConnectConfiguration$namespace`: "The namespace name or full Amazon Resource
//!   Name (ARN) of the Cloud Map namespace" — naming one namespace by name or by ARN names
//!   the SAME namespace, so a service that stored the ARN is listed by the name in it;
//! * a `namespace` outside the form a namespace reference takes is refused for its FORM,
//!   with the pattern it has to match, before anything is resolved — and the published
//!   length bound is itself inside the form;
//! * `maxResults`: "This value can be between 1 and 100. If this parameter isn't used,
//!   then ListServicesByNamespace returns up to 10 results" — a page is at most
//!   `maxResults` entries and its `nextToken` resumes the walk where the page stopped.
//!
//! Every case drives the emulator through the shared wire harness in [`crate::ecs_wire`],
//! the same front door a client uses, so nothing reaches into the state struct and a
//! response that is not computed from persisted state cannot pass.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::ecs_wire::{
    cluster_with_capacity, list, ok, refusal, register_definition, service, string, ACCOUNT,
    SIGNED_REGION,
};

/// The namespace most rows are about.
const NAMESPACE: &str = "shared-namespace";

/// A second namespace, so a service in another one is a service this listing must not
/// report.
const OTHER_NAMESPACE: &str = "other-namespace";

/// The namespace a CLUSTER holds as its Service Connect default, for the service whose own
/// configuration names none.
const DEFAULT_NAMESPACE: &str = "cluster-default-namespace";

/// The Cloud Map id of the namespace a service stored as a full ARN, so the listing is
/// asked for it by NAME.
const ARN_NAMESPACE_ID: &str = "ns-0123456789abcdef";

/// The two clusters the namespace's services are spread across, and a third holding the
/// cluster-default case.
const ALPHA: &str = "nsalpha";
const BETA: &str = "nsbeta";
const GAMMA: &str = "nsgamma";

/// The task-definition family every service here runs.
const FAMILY: &str = "nsfamily";

/// The longest a namespace reference may be, from this crate's statement of the pattern's
/// bound.
const MAX_NAMESPACE_LENGTH: usize =
    service_com_amazonaws_ecs_amazonec2containerservicev20141113::state::MAX_NAMESPACE_LENGTH;

/// The fixed half of the refusal a namespace this account has nothing in earns; the
/// variable half is the `namespace` the request carried.
const NOT_FOUND_PREFIX: &str =
    service_com_amazonaws_ecs_amazonec2containerservicev20141113::state::NAMESPACE_NOT_FOUND_PREFIX;

/// The ARN a service of `name` in `cluster` carries.
fn service_arn(cluster: &str, name: &str) -> String {
    format!("arn:aws:ecs:{SIGNED_REGION}:{ACCOUNT}:service/{cluster}/{name}")
}

/// Create one service in `cluster`, with the `serviceConnectConfiguration` given (none
/// when `Value::Null`), and return its ARN.
async fn create_service(
    router: &MultiServiceRouter,
    cluster: &str,
    name: &str,
    service_connect: Value,
    case: &str,
) -> String {
    let mut request = json!({
        "cluster": cluster,
        "serviceName": name,
        "taskDefinition": FAMILY,
        "desiredCount": 0,
        "launchType": "EC2",
    });
    if !service_connect.is_null() {
        request["serviceConnectConfiguration"] = service_connect;
    }
    let body = ok(router, "CreateService", request, case).await;
    string(&body["service"], "serviceArn", case)
}

/// The ARNs one `ListServicesByNamespace` page reports, and the token it hands back.
async fn page(
    router: &MultiServiceRouter,
    request: Value,
    case: &str,
) -> (Vec<String>, Option<String>) {
    let body = ok(router, "ListServicesByNamespace", request, case).await;
    let arns = list(&body, "serviceArns", case)
        .into_iter()
        .map(|arn| {
            arn.as_str()
                .unwrap_or_else(|| panic!("case {case:?}: expected a service ARN, got {arn}"))
                .to_string()
        })
        .collect();
    let token = body["nextToken"].as_str().map(str::to_string);
    (arns, token)
}

/// Three clusters with capacity, one task-definition revision, and the services the rules
/// select between.
async fn fixture() -> MultiServiceRouter {
    let router = service();
    for cluster in [ALPHA, BETA, GAMMA] {
        cluster_with_capacity(&router, cluster, "fixture").await;
    }
    register_definition(&router, FAMILY, "fixture").await;
    // The namespace's own services, in two DIFFERENT clusters.
    create_service(
        &router,
        ALPHA,
        "shared-one",
        json!({ "enabled": true, "namespace": NAMESPACE }),
        "fixture",
    )
    .await;
    create_service(
        &router,
        BETA,
        "shared-two",
        json!({ "enabled": true, "namespace": NAMESPACE }),
        "fixture",
    )
    .await;
    // A service of another namespace, one that stored the namespace as an ARN, one that
    // uses Service Connect but has it turned off, and one that does not use it at all.
    create_service(
        &router,
        ALPHA,
        "other-namespace-service",
        json!({ "enabled": true, "namespace": OTHER_NAMESPACE }),
        "fixture",
    )
    .await;
    create_service(
        &router,
        ALPHA,
        "arn-namespace-service",
        json!({
            "enabled": true,
            "namespace": format!(
                "arn:aws:servicediscovery:{SIGNED_REGION}:{ACCOUNT}:namespace/{ARN_NAMESPACE_ID}"
            ),
        }),
        "fixture",
    )
    .await;
    create_service(
        &router,
        ALPHA,
        "disabled-connect-service",
        json!({ "enabled": false, "namespace": NAMESPACE }),
        "fixture",
    )
    .await;
    create_service(&router, ALPHA, "no-connect-service", Value::Null, "fixture").await;
    // The cluster-default case: the cluster holds the namespace, the service's own
    // configuration names none.
    ok(
        &router,
        "UpdateCluster",
        json!({
            "cluster": GAMMA,
            "serviceConnectDefaults": { "namespace": DEFAULT_NAMESPACE },
        }),
        "fixture",
    )
    .await;
    create_service(
        &router,
        GAMMA,
        "cluster-default-service",
        json!({ "enabled": true }),
        "fixture",
    )
    .await;
    router
}

#[tokio::test]
async fn the_namespace_selects_the_services_whichever_cluster_they_are_in() {
    let router = fixture().await;

    // Every row: the namespace asked for, and the services that are in it.
    let rows: [(&str, String, Vec<String>); 3] = [
        (
            "the namespace two clusters share",
            NAMESPACE.to_string(),
            vec![
                service_arn(ALPHA, "shared-one"),
                service_arn(BETA, "shared-two"),
            ],
        ),
        (
            "a namespace one service is in",
            OTHER_NAMESPACE.to_string(),
            vec![service_arn(ALPHA, "other-namespace-service")],
        ),
        (
            "the name of a namespace a service stored as an ARN",
            ARN_NAMESPACE_ID.to_string(),
            vec![service_arn(ALPHA, "arn-namespace-service")],
        ),
    ];
    for (case, namespace, expected) in rows {
        let (arns, token) = page(&router, json!({ "namespace": namespace }), case).await;
        assert_eq!(
            arns, expected,
            "case {case:?}: expected the namespace's services, got {arns:?}"
        );
        assert_eq!(
            token, None,
            "case {case:?}: a page that holds every service reports no token, got {token:?}"
        );
    }

    // The cluster's default namespace is the namespace of a service whose own
    // configuration names none.
    let case = "the cluster's default namespace";
    let (arns, _) = page(&router, json!({ "namespace": DEFAULT_NAMESPACE }), case).await;
    assert_eq!(
        arns,
        vec![service_arn(GAMMA, "cluster-default-service")],
        "case {case:?}: expected the service that took the cluster's default, got {arns:?}"
    );

    // A service the caller DELETED is no longer one of the namespace's services.
    ok(
        &router,
        "DeleteService",
        json!({ "cluster": BETA, "service": "shared-two" }),
        "the deleted service",
    )
    .await;
    let case = "after one of the namespace's services is deleted";
    let (arns, _) = page(&router, json!({ "namespace": NAMESPACE }), case).await;
    assert_eq!(
        arns,
        vec![service_arn(ALPHA, "shared-one")],
        "case {case:?}: expected the deleted service to drop out, got {arns:?}"
    );
}

#[tokio::test]
async fn a_page_is_at_most_max_results_and_its_token_resumes_the_walk() {
    let router = fixture().await;

    // One-entry pages walk the namespace's two services exactly once, in the order the
    // full listing reports them.
    let case = "a walk of one-service pages";
    let (full, _) = page(&router, json!({ "namespace": NAMESPACE }), case).await;
    let mut walked: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    for _ in 0..full.len() {
        let mut request = json!({ "namespace": NAMESPACE, "maxResults": 1 });
        if let Some(token) = &token {
            request["nextToken"] = json!(token);
        }
        let (arns, next) = page(&router, request, case).await;
        assert_eq!(
            arns.len(),
            1,
            "case {case:?}: a page may not exceed maxResults, got {arns:?}"
        );
        walked.extend(arns);
        token = next;
    }
    assert_eq!(
        walked, full,
        "case {case:?}: the walk must report every service exactly once, got {walked:?}"
    );
    assert_eq!(
        token, None,
        "case {case:?}: the last page reports no token, got {token:?}"
    );

    // A page size outside the published range is refused: a page below it states the
    // smallest page the member takes, one above it states the range.
    for (case, max_results, expected) in [
        (
            "a page below the range",
            0,
            "Max results can not be lesser than 1",
        ),
        (
            "a page far below the range",
            -5,
            "Max results can not be lesser than 1",
        ),
        (
            "a page one above the range",
            101,
            "MaxResults must be a positive number between 1-100.",
        ),
    ] {
        let (code, message) = refusal(
            &router,
            "ListServicesByNamespace",
            json!({ "namespace": NAMESPACE, "maxResults": max_results }),
            case,
        )
        .await;
        assert_eq!(
            (code.as_str(), message.as_str()),
            ("InvalidParameterException", expected),
            "case {case:?}: expected the page-size refusal, got {code} with {message:?}"
        );
    }
    // Both published bounds are inside the range, so both are answered.
    for (case, max_results) in [("the smallest page", 1), ("the largest page", 100)] {
        let (arns, _) = page(
            &router,
            json!({ "namespace": NAMESPACE, "maxResults": max_results }),
            case,
        )
        .await;
        assert!(
            !arns.is_empty(),
            "case {case:?}: expected a page at a published bound to be answered, got {arns:?}"
        );
    }
}

#[tokio::test]
async fn a_namespace_outside_the_pattern_is_refused_for_its_form() {
    let router = fixture().await;

    // The rows span the ways a value can leave the form a namespace reference takes: each
    // character the class excludes, a character below the printable range, no character at
    // all, and one character past the published length.
    let rows: [(&str, String); 8] = [
        ("a value with a space in it", "not a valid @ namespace".to_string()),
        ("a value carrying a left angle bracket", "name<space".to_string()),
        ("a value carrying a right angle bracket", "name>space".to_string()),
        ("a value carrying a slash", "name/space".to_string()),
        ("a value carrying a quote", "name\"space".to_string()),
        ("a value carrying a tab", "name\tspace".to_string()),
        ("no namespace at all", String::new()),
        (
            "one character past the published length",
            "n".repeat(MAX_NAMESPACE_LENGTH + 1),
        ),
    ];
    for (case, namespace) in rows {
        let (code, message) = refusal(
            &router,
            "ListServicesByNamespace",
            json!({ "namespace": namespace.clone() }),
            case,
        )
        .await;
        let expected = format!(
            "Namespace ({namespace}) must match ^[!-~&&[^<>/\"]]{{1,1024}}$."
        );
        assert_eq!(
            (code.as_str(), message.as_str()),
            ("InvalidParameterException", expected.as_str()),
            "case {case:?}: expected the form refusal, got {code} with {message:?}"
        );
    }

    // A value INSIDE the form, including one of exactly the published length, is read as a
    // namespace: this account has nothing in it, which is the other refusal.
    for (case, namespace) in [
        ("a namespace nothing is in", "never-used-namespace".to_string()),
        (
            "a namespace of exactly the published length",
            "n".repeat(MAX_NAMESPACE_LENGTH),
        ),
        ("a namespace of punctuation the class allows", "ns!#$%&".to_string()),
    ] {
        let (code, message) = refusal(
            &router,
            "ListServicesByNamespace",
            json!({ "namespace": namespace.clone() }),
            case,
        )
        .await;
        // The refusal names the namespace the REQUEST asked about, so a client is told
        // which lookup failed.
        let expected = format!("{NOT_FOUND_PREFIX}{namespace}");
        assert_eq!(
            (code.as_str(), message.as_str()),
            ("NamespaceNotFoundException", expected.as_str()),
            "case {case:?}: expected the namespace-not-found refusal, got {code} with \
             {message:?}"
        );
    }
}
