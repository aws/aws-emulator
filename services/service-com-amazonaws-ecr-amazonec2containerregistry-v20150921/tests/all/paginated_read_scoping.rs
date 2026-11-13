// SPDX-License-Identifier: CC0-1.0

//! What a paginated ECR read answers when the request scopes it, and the order such a
//! read reports its entries in. Two rules, each over a table of inputs:
//!
//! 1. A read whose request ENUMERATES the items to report is refused when it also carries
//!    a pagination parameter: of `maxResults` and `nextToken`, "This option cannot be used
//!    when you specify ..." (`DescribeRepositoriesRequest$maxResults`,
//!    `DescribeImagesRequest$maxResults`, `GetLifecyclePolicyPreviewRequest$maxResults`,
//!    and each of those requests' `nextToken`). The refusal is
//!    `InvalidParameterException` (HTTP 400) and names the pagination parameter the
//!    request may not carry together with the identifier list that excludes it.
//! 2. `DescribeImages` reports a repository's images in ascending push order — the image
//!    pushed first comes first — and pages over that same order, so a walk at any page
//!    size visits every image exactly once in that order.
//! 3. Every instant an ECR response states is expressed to a resolution of one
//!    millisecond: "the date and time, expressed in standard JavaScript date format"
//!    (`ImageDetail$imagePushedAt`), "the Unix time in seconds and milliseconds"
//!    (`AuthorizationData$expiresAt`) — never with the sub-millisecond digits a host clock
//!    carries.
//!
//! Every case goes through the router, so it covers routing, deserialization, the handler,
//! the persisted state and serialization — the same path a client takes. The one thing
//! arranged in the store directly is each image's push instant, so a case controls the
//! push order independently of the digests the pushes produced (the same seam
//! `tests/all/image_scan.rs` uses for a record a push cannot produce).

use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_ecr_amazonec2containerregistry_v20150921::state;
use smithy_server_runtime::MultiServiceRouter;

use crate::repository_image_lifecycle::{given_repository, manifest_digest};
use crate::wire::{at, call, emulator, emulator_with_engine, ok};

/// One row of the scoping table: what the row is about, the operation, the API-documented
/// label of the identifier list that excludes paging, the request that enumerates the
/// items to report, and the wire error code that same request answers once the pagination
/// parameters are gone (`None` where it is served).
struct ScopedRead {
    case: &'static str,
    operation: &'static str,
    identifiers: &'static str,
    scoped: Value,
    without_paging: Option<&'static str>,
}

/// ECR's refusal of a pagination parameter presented together with the identifiers to
/// report, as the API states it: the parameter-and-constraint template, naming the
/// pagination parameter and the identifier list that excludes it. Written out here rather
/// than taken from the implementation, so a change to the wording fails this suite.
fn excluded_message(parameter: &str, identifiers: &str) -> String {
    format!(
        "Invalid parameter at '{parameter}' failed to satisfy constraint: \
         'Cannot provide {parameter} if providing {identifiers}'"
    )
}

/// `request` with every member of `extra` added to it.
fn merged(request: &Value, extra: &Value) -> Value {
    let mut merged = request.clone();
    for (member, value) in extra.as_object().expect("the extra members are an object") {
        merged[member.as_str()] = value.clone();
    }
    merged
}

/// The digests `DescribeImages` reported, in the order it reported them.
fn reported_digests(body: &Value) -> Vec<String> {
    body["imageDetails"]
        .as_array()
        .unwrap_or_else(|| panic!("DescribeImages must return an imageDetails list: {body:?}"))
        .iter()
        .map(|detail| {
            detail["imageDigest"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// The `imagePushedAt` values `DescribeImages` reported, in the order it reported them.
fn reported_push_instants(body: &Value) -> Vec<f64> {
    body["imageDetails"]
        .as_array()
        .unwrap_or_else(|| panic!("DescribeImages must return an imageDetails list: {body:?}"))
        .iter()
        .map(|detail| {
            detail["imagePushedAt"].as_f64().unwrap_or_else(|| {
                panic!("every reported image carries an imagePushedAt: {detail:?}")
            })
        })
        .collect()
}

/// Stamp the stored push instant of one image, so a case decides the push order.
fn given_push_instant(
    engine: &engine::InMemoryEngine,
    repository: &str,
    digest: &str,
    pushed_at: SystemTime,
) {
    let state = state::service_state(engine).expect("the ecr store is reachable");
    let mut store = state.lock();
    store
        .repositories
        .get_mut(repository)
        .expect("the repository was created over the wire")
        .images
        .get_mut(digest)
        .expect("the image was pushed over the wire")
        .pushed_at = pushed_at;
}

/// Push one image carrying `tag` into `repository`, and answer the digest it is stored
/// under.
async fn given_image(router: &MultiServiceRouter, repository: &str, tag: &str) -> String {
    let manifest = format!("{{\"schemaVersion\":2,\"image\":\"{tag}\"}}");
    ok(
        router,
        tag,
        "PutImage",
        &json!({
            "repositoryName": repository,
            "imageManifest": manifest,
            "imageTag": tag,
        }),
    )
    .await;
    manifest_digest(&manifest)
}

/// A read that enumerates the items to report may not also be paged, whichever pagination
/// parameter it carries — and the same request without one is not refused for it.
#[tokio::test]
async fn a_read_that_names_the_items_to_report_refuses_a_pagination_parameter() {
    let router = emulator();
    given_repository(&router, "alpha", json!({})).await;
    given_repository(&router, "bravo", json!({})).await;
    given_image(&router, "alpha", "v1").await;

    let rows = vec![
        ScopedRead {
            case: "DescribeRepositories scoped by name",
            operation: "DescribeRepositories",
            identifiers: "RepositoryNames",
            scoped: json!({"repositoryNames": ["alpha", "bravo"]}),
            without_paging: None,
        },
        ScopedRead {
            case: "DescribeImages scoped by image identifier",
            operation: "DescribeImages",
            identifiers: "ImageIds",
            scoped: json!({"repositoryName": "alpha", "imageIds": [{"imageTag": "v1"}]}),
            without_paging: None,
        },
        ScopedRead {
            case: "GetLifecyclePolicyPreview scoped by image identifier",
            operation: "GetLifecyclePolicyPreview",
            identifiers: "ImageIds",
            scoped: json!({"repositoryName": "alpha", "imageIds": [{"imageTag": "v1"}]}),
            // No preview has been started on this repository, so the read answers the
            // preview's own absence once the pagination parameters are gone. What the
            // preview itself reports is the lifecycle-preview suite's.
            without_paging: Some("LifecyclePolicyPreviewNotFoundException"),
        },
    ];

    // Each pagination variant: what it adds to the scoped request, and the parameter the
    // refusal must name. A request carrying both is refused for `MaxResults`, so one
    // request is refused for one parameter.
    let variants: Vec<(&str, Value, &str)> = vec![
        ("maxResults", json!({"maxResults": 1}), "MaxResults"),
        // A hex-rendered token, so the refusal cannot be a token that failed to decode.
        ("nextToken", json!({"nextToken": "616c706861"}), "NextToken"),
        (
            "both",
            json!({"maxResults": 1, "nextToken": "616c706861"}),
            "MaxResults",
        ),
    ];

    for row in rows {
        for (variant, extra, parameter) in &variants {
            let case = format!("{} with {variant}", row.case);
            let request = merged(&row.scoped, extra);
            let answer = call(&router, row.operation, &request).await;
            assert_eq!(
                answer.status, 400,
                "case {case:?}: expected HTTP 400 from {}, got {} with {:?}",
                row.operation, answer.status, answer.body
            );
            assert_eq!(
                answer.error_code(),
                "InvalidParameterException",
                "case {case:?}: expected InvalidParameterException, got {:?}",
                answer.body
            );
            assert_eq!(
                answer.message(),
                excluded_message(parameter, row.identifiers),
                "case {case:?}: expected the refusal to name {parameter} and {}, got {:?}",
                row.identifiers,
                answer.body
            );
        }

        // The same request without a pagination parameter is not refused for this reason.
        let case = format!("{} without a pagination parameter", row.case);
        let answer = call(&router, row.operation, &row.scoped).await;
        match row.without_paging {
            None => assert_eq!(
                answer.status, 200,
                "case {case:?}: expected HTTP 200 from {}, got {} with {:?}",
                row.operation, answer.status, answer.body
            ),
            Some(code) => assert_eq!(
                answer.error_code(),
                code,
                "case {case:?}: expected {code} from {}, got {:?}",
                row.operation,
                answer.body
            ),
        }
    }

    // A read that enumerates nothing is paged as usual: the refusal is about the
    // combination, not about the pagination parameters themselves.
    let paged: Vec<(&str, Value)> = vec![
        ("DescribeRepositories", json!({"maxResults": 1})),
        (
            "DescribeImages",
            json!({"repositoryName": "alpha", "maxResults": 1}),
        ),
    ];
    for (operation, request) in paged {
        let case = format!("{operation} paged over the whole set");
        ok(&router, &case, operation, &request).await;
    }
}

/// `DescribeImages` reports a repository's images in ascending push order, and pages over
/// that same order.
///
/// The store holds an image under its digest, so a read ordered by its storage key would
/// report the images in digest order: the push instants here are stamped in the REVERSE of
/// digest order, so only a read ordered by push time answers the expected sequence. The
/// rows differ along what the rule generalises over — the whole set, the page size a walk
/// takes it at, and a read scoped to a subset named in the opposite order.
#[tokio::test]
async fn describe_images_reports_the_images_in_ascending_push_order() {
    let (router, engine) = emulator_with_engine();
    let repository = "push-order";
    given_repository(&router, repository, json!({})).await;

    let tags = ["v1", "v2", "v3", "v4"];
    let mut digests = Vec::new();
    for tag in tags {
        digests.push(given_image(&router, repository, tag).await);
    }
    digests.sort();
    // The push order: the reverse of the digest order the store keys them by.
    let expected: Vec<String> = digests.iter().rev().cloned().collect();
    // A fixed instant, so the stamped order owes nothing to how fast the pushes ran.
    let first_push = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    for (position, digest) in expected.iter().enumerate() {
        let position = u64::try_from(position).expect("a handful of images");
        given_push_instant(
            &engine,
            repository,
            digest,
            first_push + Duration::from_secs(60 * position),
        );
    }

    let every = ok(
        &router,
        "every image",
        "DescribeImages",
        &json!({"repositoryName": repository}),
    )
    .await;
    assert_eq!(
        reported_digests(&every),
        expected,
        "case \"every image\": expected the images in push order {expected:?}, got {:?}",
        reported_digests(&every)
    );
    let instants = reported_push_instants(&every);
    assert!(
        instants.windows(2).all(|pair| pair[0] <= pair[1]),
        "case \"every image\": imagePushedAt must not go backwards, got {instants:?}"
    );

    // A walk at every page size visits every image exactly once, in that same order.
    for page_size in 1..=tags.len() + 1 {
        let case = format!("maxResults {page_size}");
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let mut request = json!({"repositoryName": repository, "maxResults": page_size});
            if let Some(token) = token.as_ref() {
                request["nextToken"] = json!(token);
            }
            let page = ok(&router, &case, "DescribeImages", &request).await;
            let reported = reported_digests(&page);
            assert!(
                reported.len() <= page_size,
                "case {case:?}: a page holds at most {page_size} entries, got {reported:?}"
            );
            walked.extend(reported);
            token = page["nextToken"].as_str().map(ToString::to_string);
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            walked, expected,
            "case {case:?}: paging must walk every image once in push order, got {walked:?}"
        );
    }

    // A read scoped to a subset reports it in the same order, whatever order the request
    // named the identifiers in.
    let subset: Vec<String> = vec![expected[0].clone(), expected[2].clone()];
    let scoped = ok(
        &router,
        "a subset named in reverse",
        "DescribeImages",
        &json!({
            "repositoryName": repository,
            "imageIds": [
                {"imageDigest": subset[1].clone()},
                {"imageDigest": subset[0].clone()},
            ],
        }),
    )
    .await;
    assert_eq!(
        reported_digests(&scoped),
        subset,
        "case \"a subset named in reverse\": expected push order {subset:?}, got {:?}",
        reported_digests(&scoped)
    );
}

/// The digits after the decimal point in the JSON rendering of `value`, or `None` when it
/// is not a number at all.
fn fractional_digits(value: &Value) -> Option<usize> {
    let rendered = value.as_number()?.to_string();
    Some(
        rendered
            .split_once('.')
            .map_or(0, |(_, fraction)| fraction.len()),
    )
}

/// Every instant a response states carries at most milliseconds — whichever operation
/// reports it, and whether it is the instant of a create, of a push, of an expiry or of a
/// transition.
///
/// The rows differ along what the rule generalises over: the operation, and which kind of
/// instant the member is.
#[tokio::test]
async fn every_reported_instant_is_stated_to_the_millisecond() {
    let router = emulator();
    let repository = "instants";
    given_repository(&router, repository, json!({})).await;
    let digest = given_image(&router, repository, "v1").await;

    // A lifecycle policy, so `GetLifecyclePolicy` has an evaluation instant to report. The
    // document is padded past the modelled minimum length of `lifecyclePolicyText`.
    let mut policy = "{\"rules\":[{\"rulePriority\":1,\"description\":\"keep ten\",\
         \"action\":{\"type\":\"expire\"},\"selection\":{\"tagStatus\":\"any\",\
         \"countType\":\"imageCountMoreThan\",\"countNumber\":10}}]}"
        .to_string();
    while policy.len() < 100 {
        policy.push(' ');
    }
    ok(
        &router,
        "a policy to evaluate",
        "PutLifecyclePolicy",
        &json!({"repositoryName": repository, "lifecyclePolicyText": policy}),
    )
    .await;
    // An archived image, so an `ImageDetail` carries the instant of a transition too.
    ok(
        &router,
        "an archived image",
        "UpdateImageStorageClass",
        &json!({
            "repositoryName": repository,
            "imageId": {"imageDigest": digest.clone()},
            "targetStorageClass": "ARCHIVE",
        }),
    )
    .await;

    // Each row: the case, the operation, its request, and the pointer to the instant.
    let rows: Vec<(&str, &str, Value, &str)> = vec![
        (
            "the instant a repository was created",
            "DescribeRepositories",
            json!({"repositoryNames": [repository]}),
            "/repositories/0/createdAt",
        ),
        (
            "the instant an image was pushed",
            "DescribeImages",
            json!({"repositoryName": repository}),
            "/imageDetails/0/imagePushedAt",
        ),
        (
            "the instant an image was archived",
            "DescribeImages",
            json!({"repositoryName": repository}),
            "/imageDetails/0/lastArchivedAt",
        ),
        (
            "the instant a registry credential expires",
            "GetAuthorizationToken",
            json!({}),
            "/authorizationData/0/expiresAt",
        ),
        (
            "the instant a lifecycle policy was evaluated",
            "GetLifecyclePolicy",
            json!({"repositoryName": repository}),
            "/lastEvaluatedAt",
        ),
    ];

    for (case, operation, request, pointer) in rows {
        let answer = ok(&router, case, operation, &request).await;
        let instant = at(&answer, pointer);
        let digits = fractional_digits(&instant).unwrap_or_else(|| {
            panic!("case {case:?}: {operation} must state {pointer} as an epoch number, got {instant:?}")
        });
        assert!(
            digits <= 3,
            "case {case:?}: an instant is stated in seconds and milliseconds, \
             so at most 3 fractional digits; {pointer} has {digits} ({instant:?})"
        );
        assert!(
            instant.as_f64().is_some_and(|seconds| seconds > 0.0),
            "case {case:?}: {pointer} must be an instant after the epoch, got {instant:?}"
        );
    }
}
