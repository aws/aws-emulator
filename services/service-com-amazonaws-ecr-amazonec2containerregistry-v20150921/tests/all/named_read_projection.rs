// SPDX-License-Identifier: CC0-1.0

//! What a read that ENUMERATES the repositories to report answers, end to end over the
//! wire, and what the address it reports for each of them is.
//!
//! Two rules, both read off one response.
//!
//! **A read whose input enumerates the resources to report PROJECTS that list onto the
//! response: one output element per requested ELEMENT, in the order the request listed
//! them, and the whole call refused on the first element that names nothing.** The list
//! `DescribeRepositories` takes is `RepositoryNameList`, `@length(min: 1, max: 100)` and NOT
//! `@uniqueItems`, and the output `repositories` is a plain `RepositoryList`, read with:
//!
//! ```text
//! smithy_query(query="operation",operation="DescribeRepositories",model_path="services/service-com-amazonaws-ecr-amazonec2containerregistry-v20150921/smithy-model/com-amazonaws-ecr-amazonec2containerregistry-v20150921.json")
//! ```
//!
//! So the answer's length and order are functions of the REQUEST: a client that zips its own
//! name list against `repositories` pairs each name with its repository, and a client that
//! counts entries sees every name it asked about answered. A read that enumerates nothing is
//! the other case — it reports the registry's own repositories, in the one order it holds
//! them in, one page at a time. The sibling
//! `BatchGetRepositoryScanningConfiguration` answers the same way for the same member, one
//! answer per requested name in request order, which is why the rows below vary the SHAPE of
//! the requested list — its length, its order against the store's, and how often one name
//! appears in it — rather than the names themselves.
//!
//! **`Repository$repositoryUri` is the ADDRESS a client reaches the repository at: the
//! endpoint this emulator answers that caller's registry requests at, followed by `/` and the
//! repository's own name.** The member says what the value is for ("The URI for the
//! repository. You can use this URI for container image `push` and `pull` operations"), and a
//! client DIALS it, splitting it on the first `/` — so the authority is an address a
//! connection opened to it reaches rather than a name to resolve. This suite asserts that out
//! of the very response that carries the value: the authority is the endpoint this emulator
//! answers on, the path is the repository's own name, the registry HOLDING it is reported
//! beside the address (`registryId` is the account its ARN names, and that ARN names the
//! Region), and the address is never a name in a zone AWS publishes its own endpoints in — a
//! name there resolves at the real Amazon ECR rather than at the machine this emulator runs
//! on.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::published_registry_address::AWS_ENDPOINT_DNS_SUFFIXES;
use crate::wire::{call, call_in_region, emulator, ok, ok_in_region, REGION};

/// The repositories every projection case reads, in the order the store holds them in —
/// ascending by name, which is what a read enumerating nothing reports.
const STORED: [&str; 3] = ["alpha", "bravo", "charlie"];

/// The most names one `DescribeRepositories` call may enumerate: `RepositoryNameList`'s
/// `@length(min: 1, max: 100)`.
const MAX_REQUESTED_NAMES: usize = 100;

/// One projection case: the list the read enumerates, and the repository names its answer
/// must carry, in order.
struct Projection {
    case: &'static str,
    requested: Vec<&'static str>,
    reported: Vec<&'static str>,
}

/// Create every repository of [`STORED`] in `region`.
async fn given_stored_repositories(router: &MultiServiceRouter, case: &str, region: &str) {
    for name in STORED {
        ok_in_region(
            router,
            case,
            region,
            "CreateRepository",
            &json!({"repositoryName": name}),
        )
        .await;
    }
}

/// The repository names `body` reports at `pointer`, in the order they were answered.
fn reported_names(case: &str, body: &Value, pointer: &str) -> Vec<String> {
    body.pointer(pointer)
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: expected a list at {pointer}, got {body}"))
        .iter()
        .map(|repository| {
            repository["repositoryName"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case {case:?}: every reported repository names itself, got {body}")
                })
                .to_string()
        })
        .collect()
}

/// The string member `name` of `repository`, or a panic naming the case.
fn member(case: &str, repository: &Value, name: &str) -> String {
    repository[name]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a {name}, got {repository}"))
        .to_string()
}

/// Assert that `repository` reports the address a client reaches it at: the endpoint this
/// emulator answers the caller's registry requests at, followed by the repository's own name.
///
/// Read exactly as a container client reads the value — split on the first `/` — and the
/// identity the SAME response reports beside it held against itself, so the assertion is
/// about the rule rather than about one composed string.
fn assert_published_uri(case: &str, repository: &Value, name: &str) {
    let uri = member(case, repository, "repositoryUri");
    let registry_id = member(case, repository, "registryId");
    let arn = member(case, repository, "repositoryArn");
    // `arn:<partition>:ecr:<region>:<account>:repository/<name>` — the identity the stored
    // record carries, which is what a client reads the registry out of.
    let arn_fields: Vec<&str> = arn.split(':').collect();
    let arn_region = *arn_fields
        .get(3)
        .unwrap_or_else(|| panic!("case {case:?}: {arn:?} names no Region"));
    let arn_account = *arn_fields
        .get(4)
        .unwrap_or_else(|| panic!("case {case:?}: {arn:?} names no account"));
    assert_eq!(
        registry_id, arn_account,
        "case {case:?}: expected the reported registry to own the repository its ARN names, \
         got {registry_id:?} beside {arn:?}"
    );
    assert!(
        !arn_region.is_empty(),
        "case {case:?}: expected {arn:?} to name the Region holding the repository"
    );

    let (authority, path) = uri
        .split_once('/')
        .unwrap_or_else(|| panic!("case {case:?}: {uri:?} names no repository path"));
    assert_eq!(
        path, name,
        "case {case:?}: expected the path of {uri:?} to be the repository's own name"
    );
    // The authority is the endpoint this emulator answers this caller's registry requests at,
    // host and port whole, which is the one address a client of these calls can dial.
    assert_eq!(
        authority,
        crate::wire::published_registry_authority(),
        "case {case:?}: expected the authority of {uri:?} to be the endpoint this emulator \
         answers registry requests at, got {authority:?}"
    );
    // And never a name in a zone AWS publishes its own endpoints in: a name there resolves at
    // the real Amazon ECR rather than at this process.
    let host = authority.rsplit_once(':').map_or(authority, |(host, _)| host);
    assert!(
        !AWS_ENDPOINT_DNS_SUFFIXES
            .iter()
            .any(|suffix| host.ends_with(suffix)),
        "case {case:?}: expected {authority:?} to be an address this emulator answers on, got \
         a name in a zone AWS publishes its own endpoints in"
    );
}

#[tokio::test]
async fn a_read_that_names_the_repositories_answers_one_entry_per_requested_name() {
    let router = emulator();
    given_stored_repositories(&router, "projection", REGION).await;

    // The rows differ along what the rule generalises over: how many elements the request
    // enumerates, how often one name appears among them, and how their order stands against
    // the order the store holds them in.
    let repeated_to_the_modelled_maximum = vec!["bravo"; MAX_REQUESTED_NAMES];
    let rows = vec![
        Projection {
            case: "one name",
            requested: vec!["bravo"],
            reported: vec!["bravo"],
        },
        Projection {
            case: "the same name twice",
            requested: vec!["bravo", "bravo"],
            reported: vec!["bravo", "bravo"],
        },
        Projection {
            case: "the same name three times",
            requested: vec!["alpha", "alpha", "alpha"],
            reported: vec!["alpha", "alpha", "alpha"],
        },
        Projection {
            case: "two names against the store's order",
            requested: vec!["charlie", "alpha"],
            reported: vec!["charlie", "alpha"],
        },
        Projection {
            case: "a repeat among distinct names",
            requested: vec!["bravo", "alpha", "bravo"],
            reported: vec!["bravo", "alpha", "bravo"],
        },
        Projection {
            case: "every stored name, reversed",
            requested: vec!["charlie", "bravo", "alpha"],
            reported: vec!["charlie", "bravo", "alpha"],
        },
        Projection {
            case: "one name repeated to the modelled maximum",
            requested: repeated_to_the_modelled_maximum.clone(),
            reported: repeated_to_the_modelled_maximum,
        },
    ];

    for row in rows {
        let described = ok(
            &router,
            row.case,
            "DescribeRepositories",
            &json!({"repositoryNames": row.requested}),
        )
        .await;
        let reported = reported_names(row.case, &described, "/repositories");
        assert_eq!(
            reported, row.reported,
            "case {:?}: expected one entry per requested name, in request order, for {:?}",
            row.case, row.requested
        );
        assert_eq!(
            described["nextToken"],
            Value::Null,
            "case {:?}: a read that enumerates the repositories is answered whole, so no \
             token follows it, got {described}",
            row.case
        );
        for (position, name) in row.reported.iter().enumerate() {
            let repository = &described["repositories"][position];
            assert_published_uri(row.case, repository, name);
        }
    }

    // The other case of the same rule: a read that enumerates nothing reports the
    // registry's own repositories, in the one order it holds them in.
    let all = ok(&router, "no names", "DescribeRepositories", &json!({})).await;
    let reported = reported_names("no names", &all, "/repositories");
    assert_eq!(
        reported,
        STORED.to_vec(),
        "case \"no names\": expected the registry's own repositories in name order"
    );
}

#[tokio::test]
async fn a_read_that_names_an_absent_repository_is_refused_whole() {
    let router = emulator();
    given_stored_repositories(&router, "refusal", REGION).await;

    // One name no repository carries, wherever it sits among names that resolve and however
    // often the list repeats either of them: the call reports nothing it could find.
    let rows: Vec<(&str, Vec<&str>)> = vec![
        ("the only name", vec!["never-created"]),
        ("the first of two", vec!["never-created", "alpha"]),
        ("the last of two", vec!["alpha", "never-created"]),
        (
            "between names that resolve",
            vec!["alpha", "never-created", "charlie"],
        ),
        (
            "repeated beside a repeated name that resolves",
            vec!["bravo", "never-created", "bravo", "never-created"],
        ),
    ];

    for (case, requested) in rows {
        let answer = call(
            &router,
            "DescribeRepositories",
            &json!({"repositoryNames": requested}),
        )
        .await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected HTTP 400 for {requested:?}, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "RepositoryNotFoundException",
            "case {case:?}: expected RepositoryNotFoundException for {requested:?}, got {:?}",
            answer.body
        );
        assert_eq!(
            answer.message(),
            format!(
                "The repository with name 'never-created' does not exist in the registry \
                 with id '{}'",
                account_of(&router, case).await
            ),
            "case {case:?}: expected the refusal to name the repository and the registry, \
             got {:?}",
            answer.body
        );
    }
}

/// The account this registry serves, read off a repository the registry itself reports —
/// never spelled out here, so the refusal's wording is checked against the registry's own
/// id rather than against a literal.
async fn account_of(router: &MultiServiceRouter, case: &str) -> String {
    let described = ok(
        router,
        case,
        "DescribeRepositories",
        &json!({"repositoryNames": [STORED[0]]}),
    )
    .await;
    member(case, &described["repositories"][0], "registryId")
}

#[tokio::test]
async fn every_reported_repository_uri_is_the_endpoint_this_emulator_answers_at() {
    // The class the address rule generalises over: the Region the repository is held in,
    // which its ARN names, crossed with the name forms `RepositoryName`'s pattern allows,
    // which become the path. One Region of four partitions, so no case is the harness's own
    // Region alone.
    //
    // Source for the Region of each partition: the `dnsSuffix` each partition publishes in
    // the AWS SDK partition metadata (`endpoints.json`) — the suffix a registry of that
    // partition is served under ON AWS, which is exactly what a published address may not
    // carry.
    let regions = [REGION, "us-west-2", "cn-north-1", "us-gov-west-1"];
    let names = ["app", "ab", "team/app", "org/team/app", "a.b_c--d"];

    for region in regions {
        for name in names {
            let router = emulator();
            let case = format!("{region} / {name}");

            // Every operation that reports a `Repository` composes the member the same way,
            // so the create, the read and the delete are all asserted.
            let created = ok_in_region(
                &router,
                &case,
                region,
                "CreateRepository",
                &json!({"repositoryName": name}),
            )
            .await;
            assert_published_uri(&case, &created["repository"], name);

            let described = ok_in_region(
                &router,
                &case,
                region,
                "DescribeRepositories",
                &json!({"repositoryNames": [name]}),
            )
            .await;
            assert_published_uri(&case, &described["repositories"][0], name);

            // A repository of ANOTHER Region's registry is not in this one, so a read made
            // against a different Region reports nothing for the name at all.
            let elsewhere = if region == REGION { "us-east-2" } else { REGION };
            let foreign = call_in_region(
                &router,
                elsewhere,
                "DescribeRepositories",
                &json!({"repositoryNames": [name]}),
            )
            .await;
            assert_eq!(
                foreign.error_code(),
                "RepositoryNotFoundException",
                "case {case:?}: expected {elsewhere}'s registry to hold no such repository, \
                 got {:?}",
                foreign.body
            );

            let deleted = ok_in_region(
                &router,
                &case,
                region,
                "DeleteRepository",
                &json!({"repositoryName": name}),
            )
            .await;
            assert_published_uri(&case, &deleted["repository"], name);
        }
    }
}
