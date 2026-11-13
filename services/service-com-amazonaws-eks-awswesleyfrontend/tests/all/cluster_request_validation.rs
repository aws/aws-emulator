// SPDX-License-Identifier: CC0-1.0

//! What this service answers a cluster-scope request whose PARAMETERS it refuses:
//! `CreateCluster`, `DescribeCluster`, `ListClusters` and `DescribeClusterVersions`,
//! driven over the wire through the same `MultiServiceRouter` the production binary
//! mounts.
//!
//! RULE: a parameter Amazon EKS validates itself is judged before any state is read or
//! written, and a violation is refused with the sentence THIS API states for that
//! parameter — never with the validation framework's constrained-leaf report — in a
//! modelled error channel of the operation, carrying the `clusterName` the request
//! named for the refusals that report one.
//!
//! Each suite asserts that rule over a TABLE whose rows differ along what the rule
//! generalises over — which parameter broke, HOW it broke it (absent, outside a
//! grammar, past a bound, outside a value set, naming no resource), the operation it
//! was sent to, and the accepted side of the same rule — rather than over the one
//! request an issue cited. The sentences themselves are written out here, as the
//! contract a client reads; the value sets, bounds and version rows the rows are drawn
//! from are read out of the model's own constants and the catalogs the emulator serves,
//! so no row repeats a table the service also holds.
//!
//! The wire harness (`Answer`, `emulator`, `call`, `encode_label`) is the shared one in
//! `tests/all/wire.rs`; the cluster request builders are `cluster_lifecycle`'s.

use service_com_amazonaws_eks_awswesleyfrontend::state::{
    MAX_CLUSTER_NAME_LEN, MAX_LIST_MAX_RESULTS, MIN_LIST_MAX_RESULTS,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::cluster_lifecycle::{create, describe, list, minimal_request, names_of, REGION};
use crate::wire::{call, emulator, error_code, Answer};

/// The error this service states a bad parameter with — "The specified parameter is
/// invalid. Review the available parameters for the API request."
/// (`smithy_query(query="shape", shape="InvalidParameterException")`), which nearly
/// every operation that takes input declares.
const INVALID_PARAMETER: &str = "InvalidParameterException";

/// The error a second create of a name a Region already holds is refused with
/// (`smithy_query(query="operation", operation="CreateCluster")` reports
/// `ResourceInUseException`, `httpError` 409).
const RESOURCE_IN_USE: &str = "ResourceInUseException";

/// The sentence a cluster-name member outside the cluster-name grammar earns, as the
/// service prints it: the member's name, then fixed text stating the grammar
/// `ClusterName` is `@pattern`-ed with
/// (`smithy_query(query="operation", operation="CreateCluster")`).
fn invalid_characters(member: &str) -> String {
    format!(
        "The parameter {member} contains invalid characters. It should conform to the \
         regular expression \"^[0-9A-Za-z][A-Za-z0-9-_]*\"In other words, it should start \
         with a number or letter, and can contain numbers, letters, dashes, and \
         underscores."
    )
}

/// The sentence a parameter past its published maximum length earns.
fn too_long(member: &str, maximum: usize) -> String {
    format!("The parameter {member} should not be greater than {maximum} characters.")
}

/// The sentence a `maxResults` outside its modelled `@range` earns, whose two numbers
/// are the bounds the model declares for every `maxResults` of this API.
fn page_size_out_of_range() -> String {
    format!("maxResults needs to be between {MIN_LIST_MAX_RESULTS} and {MAX_LIST_MAX_RESULTS}")
}

/// A request with `members` merged into the smallest body `CreateCluster` accepts.
fn request(name: &str, members: serde_json::Value) -> serde_json::Value {
    let mut body = minimal_request(name);
    let (Some(body_members), Some(extra)) = (body.as_object_mut(), members.as_object()) else {
        panic!("both the request and the members merged into it must be objects");
    };
    for (key, value) in extra {
        body_members.insert(key.clone(), value.clone());
    }
    body
}

/// The `clusterName` member a refusal carries, or `None` when it names no cluster.
fn cluster_name_of(answer: &Answer) -> Option<&str> {
    answer.body["clusterName"].as_str()
}

/// `GET /cluster-versions` with `query`.
async fn describe_versions(router: &MultiServiceRouter, query: &str) -> Answer {
    let uri = if query.is_empty() {
        "/cluster-versions".to_string()
    } else {
        format!("/cluster-versions?{query}")
    };
    call(router, REGION, "GET", &uri, "").await
}

/// The Kubernetes versions the catalog OFFERS a control plane for — the published
/// rows whose support has not ended, which is the set a caller picks a `version`
/// from. The listing publishes the whole calendar, out-of-support rows included,
/// so the `versionStatus` on each row is what narrows it to the offered ones.
async fn offered_versions(router: &MultiServiceRouter) -> Vec<String> {
    let answer = describe_versions(router, "maxResults=100").await;
    assert_eq!(
        answer.status, 200,
        "setup: expected HTTP 200 from the version catalog, got {answer:?}"
    );
    let rows = answer.body["clusterVersions"]
        .as_array()
        .unwrap_or_else(|| panic!("setup: expected a clusterVersions list, got {:?}", answer.body))
        .iter()
        .filter(|row| row["versionStatus"].as_str() != Some("UNSUPPORTED"))
        .filter_map(|row| row["clusterVersion"].as_str().map(str::to_owned))
        .collect::<Vec<String>>();
    assert!(
        !rows.is_empty(),
        "setup: the catalog must offer at least one Kubernetes version"
    );
    rows
}

// ---------------------------------------------------------------------------
// RULE: a cluster name outside the grammar every cluster of this API obeys is refused
// with the sentence this service states for that parameter, naming the cluster the
// request asked for, and nothing is written.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_cluster_name_outside_the_grammar_is_refused_in_the_services_own_sentence() {
    let router = emulator();
    let past_the_bound = "c".repeat(MAX_CLUSTER_NAME_LEN + 1);

    // Rows differ along HOW the name misses the grammar: a separator the grammar does
    // not admit, a first character it does not admit, punctuation, a path separator, a
    // whole ARN where a name belongs, and a name past the published maximum — which is
    // the other sentence of the same parameter.
    let rows: Vec<(&str, String, String)> = vec![
        (
            "a space inside the name",
            "probe cluster".to_string(),
            invalid_characters("name"),
        ),
        (
            "a leading dash",
            "-probe".to_string(),
            invalid_characters("name"),
        ),
        (
            "a leading underscore",
            "_probe".to_string(),
            invalid_characters("name"),
        ),
        (
            "punctuation",
            "probe!cluster".to_string(),
            invalid_characters("name"),
        ),
        (
            "a path separator",
            "probe/cluster".to_string(),
            invalid_characters("name"),
        ),
        (
            "a cluster ARN where a name belongs",
            "arn:aws:eks:us-west-2:000000000000:cluster/probe".to_string(),
            invalid_characters("name"),
        ),
        (
            "one character past the published maximum",
            past_the_bound.clone(),
            too_long("name", MAX_CLUSTER_NAME_LEN),
        ),
    ];

    for (case, name, expected) in &rows {
        let answer = create(&router, &minimal_request(name)).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected HTTP 400, got {answer:?}"
        );
        assert_eq!(
            error_code(&answer),
            INVALID_PARAMETER,
            "case {case:?}: expected {INVALID_PARAMETER}, got {answer:?}"
        );
        assert_eq!(
            answer.message(),
            *expected,
            "case {case:?}: expected {expected:?}, got {:?}",
            answer.message()
        );
        assert_eq!(
            cluster_name_of(&answer),
            Some(name.as_str()),
            "case {case:?}: expected the refusal to name the cluster asked for, got {:?}",
            cluster_name_of(&answer)
        );
        assert!(
            names_of(&list(&router, "").await).is_empty(),
            "case {case:?}: a refused create must write nothing"
        );
    }

    // The accepted side of the same grammar, on rows the issue did not cite: a name
    // starting with a digit, one carrying both separators, a single character, and one
    // AT the published maximum.
    let at_the_bound = "c".repeat(MAX_CLUSTER_NAME_LEN);
    for name in ["9probe", "probe-cluster_9", "c", at_the_bound.as_str()] {
        let answer = create(&router, &minimal_request(name)).await;
        assert_eq!(
            answer.status, 200,
            "case {name:?}: a name inside the grammar is accepted, got {answer:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// RULE: a parameter this API requires and the request omits is refused with the
// sentence this service states for that parameter.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_omitted_required_parameter_is_refused_in_the_services_own_sentence() {
    let router = emulator();

    // (case, the body, the sentence, the cluster the refusal names). The rows differ
    // along WHICH required parameter is missing, and along whether the refusal reports
    // a cluster at all: a member the request never carried is reported by the reader
    // that could not bind it, which names no cluster, while the rules the handler
    // judges name the cluster the request asked for.
    let rows: Vec<(&str, serde_json::Value, String, Option<&str>)> = vec![
        (
            "no name at all",
            serde_json::json!({
                "roleArn": "arn:aws:iam::000000000000:role/eksClusterRole",
                "resourcesVpcConfig": { "subnetIds": ["subnet-11111111", "subnet-22222222"] },
            }),
            "The parameter name is required.".to_string(),
            None,
        ),
        (
            "no roleArn",
            serde_json::json!({
                "name": "required-role",
                "resourcesVpcConfig": { "subnetIds": ["subnet-11111111", "subnet-22222222"] },
            }),
            "The roleArn parameter is required".to_string(),
            None,
        ),
        (
            "no resourcesVpcConfig",
            serde_json::json!({
                "name": "required-vpc",
                "roleArn": "arn:aws:iam::000000000000:role/eksClusterRole",
            }),
            "Subnet Id is required".to_string(),
            None,
        ),
        (
            "a resourcesVpcConfig naming no subnet",
            serde_json::json!({
                "name": "required-subnets",
                "roleArn": "arn:aws:iam::000000000000:role/eksClusterRole",
                "resourcesVpcConfig": { "securityGroupIds": ["sg-11111111"] },
            }),
            "Subnet Id is required".to_string(),
            Some("required-subnets"),
        ),
        (
            "an empty subnet list",
            serde_json::json!({
                "name": "required-empty-subnets",
                "roleArn": "arn:aws:iam::000000000000:role/eksClusterRole",
                "resourcesVpcConfig": { "subnetIds": [] },
            }),
            "Subnet Id is required".to_string(),
            Some("required-empty-subnets"),
        ),
        (
            "a single subnet, which cannot sit in two zones",
            serde_json::json!({
                "name": "required-two-zones",
                "roleArn": "arn:aws:iam::000000000000:role/eksClusterRole",
                "resourcesVpcConfig": { "subnetIds": ["subnet-11111111"] },
            }),
            "Subnets specified must be in at least two different AZs".to_string(),
            Some("required-two-zones"),
        ),
    ];

    for (case, body, expected, names) in &rows {
        let answer = create(&router, body).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected HTTP 400, got {answer:?}"
        );
        assert_eq!(
            error_code(&answer),
            INVALID_PARAMETER,
            "case {case:?}: expected {INVALID_PARAMETER}, got {answer:?}"
        );
        assert_eq!(
            answer.message(),
            *expected,
            "case {case:?}: expected {expected:?}, got {:?}",
            answer.message()
        );
        assert_eq!(
            cluster_name_of(&answer),
            *names,
            "case {case:?}: expected the refusal to name {names:?}, got {:?}",
            cluster_name_of(&answer)
        );
        assert!(
            names_of(&list(&router, "").await).is_empty(),
            "case {case:?}: a refused create must write nothing"
        );
    }
}

// ---------------------------------------------------------------------------
// RULE: a page size outside the bounds the model declares for it is refused with the
// sentence this service states for `maxResults`, whatever listing it was sent to.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_page_size_outside_the_published_bounds_is_refused_in_the_services_own_sentence() {
    let router = emulator();
    create(&router, &minimal_request("paged")).await;

    // The listings of this slice, each bounding `maxResults` 1..100, and the values
    // that miss those bounds: each published bound's own far side, far past it, and a
    // negative page.
    let listings: [(&str, &str); 2] = [
        ("ListClusters", "/clusters"),
        ("DescribeClusterVersions", "/cluster-versions"),
    ];
    let outside: [(&str, i64); 4] = [
        ("one below the minimum", MIN_LIST_MAX_RESULTS - 1),
        ("one past the maximum", MAX_LIST_MAX_RESULTS + 1),
        ("far past the maximum", 1_000),
        ("a negative page", -1),
    ];

    for (operation, path) in listings {
        for (case, size) in outside {
            let answer = call(
                &router,
                REGION,
                "GET",
                &format!("{path}?maxResults={size}"),
                "",
            )
            .await;
            assert_eq!(
                answer.status, 400,
                "case {operation}/{case}: expected HTTP 400, got {answer:?}"
            );
            assert_eq!(
                error_code(&answer),
                INVALID_PARAMETER,
                "case {operation}/{case}: expected {INVALID_PARAMETER}, got {answer:?}"
            );
            assert_eq!(
                answer.message(),
                page_size_out_of_range(),
                "case {operation}/{case}: expected {:?}, got {:?}",
                page_size_out_of_range(),
                answer.message()
            );
        }

        // Each published bound itself is a page this service serves.
        for (case, size) in [
            ("the minimum", MIN_LIST_MAX_RESULTS),
            ("the maximum", MAX_LIST_MAX_RESULTS),
        ] {
            let answer = call(
                &router,
                REGION,
                "GET",
                &format!("{path}?maxResults={size}"),
                "",
            )
            .await;
            assert_eq!(
                answer.status, 200,
                "case {operation}/{case}: a page size inside the bounds is served, got {answer:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// RULE: a `nextToken` a listing of the CLUSTER-MANAGEMENT plane did not issue is
// refused with the one sentence Amazon EKS states for a page token there, naming
// neither the listing nor the token.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_next_token_this_service_did_not_issue_is_refused_in_the_services_own_sentence() {
    let router = emulator();
    create(&router, &minimal_request("tokens")).await;

    // Rows differ along HOW the token misses: a word, hexadecimal bytes nothing
    // minted, an empty token, and a token one listing minted carried to another.
    let minted = describe_versions(&router, "maxResults=1")
        .await
        .body["nextToken"]
        .as_str()
        .unwrap_or_else(|| panic!("setup: a one-row page must leave a nextToken"))
        .to_owned();
    let tokens: Vec<(&str, String)> = vec![
        ("a word", "not-a-token".to_string()),
        ("bytes nothing minted", "deadbeefdeadbeef".to_string()),
        ("an empty token", String::new()),
        ("another listing's token", minted),
    ];

    for (operation, path) in [
        ("ListClusters", "/clusters"),
        ("DescribeClusterVersions", "/cluster-versions"),
    ] {
        for (case, token) in &tokens {
            // The token one listing minted is a token of THAT listing: skip the
            // listing that issued it.
            if *case == "another listing's token" && path == "/cluster-versions" {
                continue;
            }
            let answer = call(
                &router,
                REGION,
                "GET",
                &format!("{path}?nextToken={token}"),
                "",
            )
            .await;
            assert_eq!(
                answer.status, 400,
                "case {operation}/{case}: expected HTTP 400, got {answer:?}"
            );
            assert_eq!(
                answer.message(),
                "nextToken is not valid or expired.",
                "case {operation}/{case}: expected the page-token sentence, got {:?}",
                answer.message()
            );
        }
    }
}

// ---------------------------------------------------------------------------
// RULE: the cluster-version catalog's two filters are refused with the sentences this
// service states for them — the support status naming the rejected value, the version
// list stating the form it takes.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_cluster_version_filters_are_refused_in_the_services_own_sentences() {
    let router = emulator();

    // Rows differ along HOW the status misses the value set the model declares for the
    // filter (`ClusterVersionStatus` is `unsupported | standard-support |
    // extended-support`, `smithy_query(query="shape", shape="ClusterVersionStatus")`):
    // a separator swapped, the case changed, a value of the OTHER spelling of the same
    // filter, and a word.
    for status in [
        "standard_support",
        "STANDARD-SUPPORT",
        "STANDARD_SUPPORT",
        "bogus",
    ] {
        let answer = describe_versions(&router, &format!("status={status}")).await;
        assert_eq!(
            answer.status, 400,
            "case {status:?}: expected HTTP 400, got {answer:?}"
        );
        assert_eq!(
            error_code(&answer),
            INVALID_PARAMETER,
            "case {status:?}: expected {INVALID_PARAMETER}, got {answer:?}"
        );
        assert_eq!(
            answer.message(),
            format!("Provided Cluster Status is invalid: {status}"),
            "case {status:?}: expected the status sentence, got {:?}",
            answer.message()
        );
    }

    // Every value the model DOES declare for that filter is served.
    for status in ["unsupported", "standard-support", "extended-support"] {
        let answer = describe_versions(&router, &format!("status={status}")).await;
        assert_eq!(
            answer.status, 200,
            "case {status:?}: a declared status filter is served, got {answer:?}"
        );
    }

    // Rows differ along HOW the version list misses the form its own refusal states:
    // a word, a major version alone, a patch version of an offered one, a prefixed
    // version, an empty entry, and a well-formed version beside a word.
    let offered = offered_versions(&router).await;
    let newest = offered.first().expect("the catalog offers a version").clone();
    let malformed: Vec<(&str, String)> = vec![
        ("a word", "not-a-version".to_string()),
        ("a major version alone", "1".to_string()),
        ("a patch version", format!("{newest}.0")),
        ("a prefixed version", format!("v{newest}")),
        ("an empty entry", String::new()),
        ("a word beside a version", format!("{newest},latest")),
    ];
    for (case, filter) in &malformed {
        let answer = describe_versions(&router, &format!("clusterVersions={filter}")).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected HTTP 400, got {answer:?}"
        );
        assert_eq!(
            error_code(&answer),
            INVALID_PARAMETER,
            "case {case:?}: expected {INVALID_PARAMETER}, got {answer:?}"
        );
        assert_eq!(
            answer.message(),
            "Provided Cluster Versions are invalid, it must be comma separated versions ie \
             1.27,1.28",
            "case {case:?}: expected the version-filter sentence, got {:?}",
            answer.message()
        );
    }

    // The accepted side: one version, and several comma-separated ones, each filtering
    // the catalog to exactly the rows asked for.
    for (case, filter, expected) in [
        ("one version", newest.clone(), vec![newest.clone()]),
        (
            "two comma-separated versions",
            offered[..2.min(offered.len())].join(","),
            offered[..2.min(offered.len())].to_vec(),
        ),
    ] {
        let answer = describe_versions(&router, &format!("clusterVersions={filter}")).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: a filter of versions is served, got {answer:?}"
        );
        let reported: Vec<String> = answer.body["clusterVersions"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .filter_map(|row| row["clusterVersion"].as_str().map(str::to_owned))
            .collect();
        assert_eq!(
            reported, expected,
            "case {case:?}: expected the versions the filter names, got {reported:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// RULE: a `version` Amazon EKS offers no control plane for is refused with the
// sentence this service states for it, naming the cluster asked for, and nothing is
// written.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_version_the_service_does_not_offer_is_refused_in_the_services_own_sentence() {
    let router = emulator();
    let offered = offered_versions(&router).await;
    let newest = offered.first().expect("the catalog offers a version").clone();

    // Rows differ along HOW the version misses the catalog: a patch version of an
    // offered one, a prefixed one, a minor version nothing publishes, a major version
    // alone, a word, and the empty string.
    let refused: Vec<(&str, String)> = vec![
        ("a patch version of an offered one", format!("{newest}.0")),
        ("a prefixed version", format!("v{newest}")),
        ("a minor version nothing publishes", "9.99".to_string()),
        ("a major version alone", "1".to_string()),
        ("a word", "latest".to_string()),
        ("the empty string", String::new()),
    ];

    for (index, (case, version)) in refused.iter().enumerate() {
        let name = format!("version-refused-{index}");
        let body = request(&name, serde_json::json!({ "version": version }));
        let answer = create(&router, &body).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected HTTP 400, got {answer:?}"
        );
        assert_eq!(
            error_code(&answer),
            INVALID_PARAMETER,
            "case {case:?}: expected {INVALID_PARAMETER}, got {answer:?}"
        );
        assert_eq!(
            answer.message(),
            format!("unsupported Kubernetes version {version}"),
            "case {case:?}: expected the unsupported-version sentence, got {:?}",
            answer.message()
        );
        assert_eq!(
            cluster_name_of(&answer),
            Some(name.as_str()),
            "case {case:?}: expected the refusal to name the cluster asked for, got {:?}",
            cluster_name_of(&answer)
        );
        assert_eq!(
            describe(&router, &name).await.status,
            404,
            "case {case:?}: a refused create must write nothing"
        );
    }

    // The accepted side, over the WHOLE catalog rather than one cited row: every
    // version the catalog offers creates a cluster that reports it.
    for (index, version) in offered.iter().enumerate() {
        let name = format!("version-offered-{index}");
        let body = request(&name, serde_json::json!({ "version": version }));
        let answer = create(&router, &body).await;
        assert_eq!(
            answer.status, 200,
            "case {version:?}: an offered version is accepted, got {answer:?}"
        );
        assert_eq!(
            answer.cluster()["version"].as_str(),
            Some(version.as_str()),
            "case {version:?}: the cluster reports the version it was created on, got {:?}",
            answer.cluster()["version"]
        );
    }
}

// ---------------------------------------------------------------------------
// RULE: a VPC configuration this service cannot provision a control plane in is
// refused with the sentence it states for the parameter at fault — and a refusal that
// reports another service's resource id names that id and no cluster.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_vpc_configuration_the_service_cannot_provision_is_refused() {
    let router = emulator();

    // (case, the resourcesVpcConfig, the sentence, the cluster the refusal names).
    // Rows differ along WHICH member is at fault and how: a subnet id and a security
    // group id outside Amazon EC2's own identifier form (no prefix, the prefix alone, a
    // width between the two published ones, another kind's prefix), both endpoint-access
    // paths turned off, and a public-access CIDR that is not a CIDR block.
    let rows: Vec<(&str, serde_json::Value, String, bool)> = vec![
        (
            "a subnet id with no prefix at all",
            serde_json::json!({ "subnetIds": ["not-a-subnet-id", "subnet-22222222"] }),
            "The request contains an invalid subnet Id: not-a-subnet-id".to_string(),
            false,
        ),
        (
            "a subnet id of a width EC2 does not publish",
            serde_json::json!({ "subnetIds": ["subnet-11111111", "subnet-123456789"] }),
            "The request contains an invalid subnet Id: subnet-123456789".to_string(),
            false,
        ),
        (
            "a security group id with no prefix at all",
            serde_json::json!({
                "subnetIds": ["subnet-11111111", "subnet-22222222"],
                "securityGroupIds": ["not-a-security-group-id"],
            }),
            "The request contains an invalid security group Id: not-a-security-group-id"
                .to_string(),
            false,
        ),
        (
            "a security group id that is only the prefix",
            serde_json::json!({
                "subnetIds": ["subnet-11111111", "subnet-22222222"],
                "securityGroupIds": ["sg-11111111", "sg-"],
            }),
            "The request contains an invalid security group Id: sg-".to_string(),
            false,
        ),
        (
            "a subnet id in the security group list",
            serde_json::json!({
                "subnetIds": ["subnet-11111111", "subnet-22222222"],
                "securityGroupIds": ["subnet-11111111"],
            }),
            "The request contains an invalid security group Id: subnet-11111111".to_string(),
            false,
        ),
        (
            "public access off with private access left at its default",
            serde_json::json!({
                "subnetIds": ["subnet-11111111", "subnet-22222222"],
                "endpointPublicAccess": false,
            }),
            "Private and public endpoint access cannot be false".to_string(),
            true,
        ),
        (
            "both endpoint access paths off",
            serde_json::json!({
                "subnetIds": ["subnet-11111111", "subnet-22222222"],
                "endpointPublicAccess": false,
                "endpointPrivateAccess": false,
            }),
            "Private and public endpoint access cannot be false".to_string(),
            true,
        ),
        (
            "a public access CIDR whose prefix is past its family's width",
            serde_json::json!({
                "subnetIds": ["subnet-11111111", "subnet-22222222"],
                "publicAccessCidrs": ["0.0.0.0/33"],
            }),
            "The following CIDRs are invalid in publicAccessCidrs: [0.0.0.0/33]".to_string(),
            true,
        ),
        (
            "two public access CIDRs that are not CIDR blocks",
            serde_json::json!({
                "subnetIds": ["subnet-11111111", "subnet-22222222"],
                "publicAccessCidrs": ["10.0.0.0/8", "10.0.0.0", "2001:db8::/129"],
            }),
            "The following CIDRs are invalid in publicAccessCidrs: [10.0.0.0, 2001:db8::/129]"
                .to_string(),
            true,
        ),
    ];

    for (index, (case, vpc, expected, names_cluster)) in rows.iter().enumerate() {
        let name = format!("vpc-refused-{index}");
        let mut body = minimal_request(&name);
        body["resourcesVpcConfig"] = vpc.clone();
        let answer = create(&router, &body).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected HTTP 400, got {answer:?}"
        );
        assert_eq!(
            error_code(&answer),
            INVALID_PARAMETER,
            "case {case:?}: expected {INVALID_PARAMETER}, got {answer:?}"
        );
        assert_eq!(
            answer.message(),
            *expected,
            "case {case:?}: expected {expected:?}, got {:?}",
            answer.message()
        );
        let expected_cluster = names_cluster.then_some(name.as_str());
        assert_eq!(
            cluster_name_of(&answer),
            expected_cluster,
            "case {case:?}: expected the refusal to name {expected_cluster:?}, got {:?}",
            cluster_name_of(&answer)
        );
        assert_eq!(
            describe(&router, &name).await.status,
            404,
            "case {case:?}: a refused create must write nothing"
        );
    }

    // The accepted side of each of those rules, on rows the issue did not cite: EC2
    // ids in both published widths, private access carrying the endpoint public access
    // gives up, and CIDR blocks of both address families.
    let accepted: Vec<(&str, serde_json::Value)> = vec![
        (
            "ids in EC2's short form",
            serde_json::json!({
                "subnetIds": ["subnet-11111111", "subnet-22222222"],
                "securityGroupIds": ["sg-11111111"],
            }),
        ),
        (
            "ids in EC2's long form",
            serde_json::json!({
                "subnetIds": ["subnet-0123456789abcdef0", "subnet-0123456789abcdef1"],
                "securityGroupIds": ["sg-0123456789abcdef0"],
            }),
        ),
        (
            "private access only",
            serde_json::json!({
                "subnetIds": ["subnet-11111111", "subnet-22222222"],
                "endpointPublicAccess": false,
                "endpointPrivateAccess": true,
            }),
        ),
        (
            "CIDR blocks of both families",
            serde_json::json!({
                "subnetIds": ["subnet-11111111", "subnet-22222222"],
                "publicAccessCidrs": ["10.0.0.0/8", "0.0.0.0/0", "::/0", "2001:db8::1/128"],
            }),
        ),
    ];
    for (index, (case, vpc)) in accepted.iter().enumerate() {
        let name = format!("vpc-accepted-{index}");
        let mut body = minimal_request(&name);
        body["resourcesVpcConfig"] = vpc.clone();
        let answer = create(&router, &body).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: a configuration this service can provision is accepted, got {answer:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// RULE: a value outside the set an enum member is judged by is refused with the
// sentence this service states for that member, wherever the model puts it.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_value_outside_an_enum_member_is_refused_in_the_services_own_sentence() {
    let router = emulator();

    // (case, the members merged into the request, the sentence). The rows differ along
    // the member, along the DEPTH it sits at (a nested structure, and a list inside a
    // list), and along how the value misses the set the model declares.
    let rows: Vec<(&str, serde_json::Value, &str)> = vec![
        (
            "an authenticationMode in the wrong case",
            serde_json::json!({ "accessConfig": { "authenticationMode": "api" } }),
            "authenticationMode must be one of [API, API_AND_CONFIG_MAP, CONFIG_MAP]",
        ),
        (
            "an authenticationMode nothing declares",
            serde_json::json!({ "accessConfig": { "authenticationMode": "NONE" } }),
            "authenticationMode must be one of [API, API_AND_CONFIG_MAP, CONFIG_MAP]",
        ),
        (
            "a log type in the wrong case",
            serde_json::json!({
                "logging": { "clusterLogging": [{ "types": ["API"], "enabled": true }] },
            }),
            "API is not a supported log type",
        ),
        (
            "a log type nothing declares, beside one that is declared",
            serde_json::json!({
                "logging": {
                    "clusterLogging": [{ "types": ["api", "kubelet"], "enabled": true }],
                },
            }),
            "kubelet is not a supported log type",
        ),
    ];

    for (index, (case, members, expected)) in rows.iter().enumerate() {
        let name = format!("enum-refused-{index}");
        let answer = create(&router, &request(&name, members.clone())).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected HTTP 400, got {answer:?}"
        );
        assert_eq!(
            error_code(&answer),
            INVALID_PARAMETER,
            "case {case:?}: expected {INVALID_PARAMETER}, got {answer:?}"
        );
        assert_eq!(
            answer.message(),
            *expected,
            "case {case:?}: expected {expected:?}, got {:?}",
            answer.message()
        );
        // A value-set refusal is about the member it names, so it reports no cluster.
        assert_eq!(
            cluster_name_of(&answer),
            None,
            "case {case:?}: expected the refusal to name no cluster, got {:?}",
            cluster_name_of(&answer)
        );
        assert_eq!(
            describe(&router, &name).await.status,
            404,
            "case {case:?}: a refused create must write nothing"
        );
    }
}

// ---------------------------------------------------------------------------
// RULE: `DescribeCluster` judges the name it is addressed by BEFORE it reads anything:
// a value outside the cluster-name grammar names no cluster that could exist, so it is
// refused as the invalid parameter it is rather than reported as a cluster this Region
// does not hold.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_malformed_cluster_name_is_refused_before_the_cluster_is_looked_up() {
    let router = emulator();
    create(&router, &minimal_request("read-me")).await;
    let past_the_bound = "c".repeat(MAX_CLUSTER_NAME_LEN + 1);

    // Rows differ along HOW the name misses the grammar, including the value a client
    // most often sends by mistake: the cluster's own ARN.
    let rows: Vec<(&str, String, String)> = vec![
        (
            "the cluster's own ARN",
            "arn:aws:eks:us-west-2:000000000000:cluster/read-me".to_string(),
            invalid_characters("name"),
        ),
        (
            "a space inside the name",
            "read me".to_string(),
            invalid_characters("name"),
        ),
        (
            "a leading dash",
            "-read-me".to_string(),
            invalid_characters("name"),
        ),
        (
            "one character past the published maximum",
            past_the_bound.clone(),
            too_long("name", MAX_CLUSTER_NAME_LEN),
        ),
    ];

    for (case, name, expected) in &rows {
        let answer = describe(&router, name).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected HTTP 400, got {answer:?}"
        );
        assert_eq!(
            error_code(&answer),
            INVALID_PARAMETER,
            "case {case:?}: expected {INVALID_PARAMETER}, got {answer:?}"
        );
        assert_eq!(
            answer.message(),
            *expected,
            "case {case:?}: expected {expected:?}, got {:?}",
            answer.message()
        );
        assert_eq!(
            cluster_name_of(&answer),
            Some(name.as_str()),
            "case {case:?}: expected the refusal to name the value asked about, got {:?}",
            cluster_name_of(&answer)
        );
    }

    // The other side of the rule: a WELL-FORMED name is resolved, so the cluster that
    // exists is read and the one that does not is absence.
    let found = describe(&router, "read-me").await;
    assert_eq!(
        found.status, 200,
        "a well-formed name of a stored cluster is read, got {found:?}"
    );
    let missing = describe(&router, "read-me-not").await;
    assert_eq!(
        missing.status, 404,
        "a well-formed name nothing holds is absence, got {missing:?}"
    );
    assert_eq!(
        error_code(&missing),
        "ResourceNotFoundException",
        "a well-formed name nothing holds is absence, got {missing:?}"
    );
}

// ---------------------------------------------------------------------------
// RULE: a cluster NAME identifies a cluster within a Region, so a second create of a
// name that Region already holds is the modelled conflict — and exactly one cluster
// carries the name afterwards.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_second_create_of_a_name_the_region_holds_is_a_conflict() {
    let router = emulator();

    // Rows differ along what the SECOND request carries beside the name: the same body,
    // the same idempotency token, a different token, and a different configuration —
    // the name is the whole conflict key, so each is the same conflict.
    let rows: Vec<(&str, serde_json::Value)> = vec![
        ("the identical body", serde_json::json!({})),
        (
            "the same idempotency token",
            serde_json::json!({ "clientRequestToken": "token-conflict" }),
        ),
        (
            "a different idempotency token",
            serde_json::json!({ "clientRequestToken": "token-other" }),
        ),
        (
            "a different configuration",
            serde_json::json!({ "deletionProtection": true }),
        ),
    ];

    for (index, (case, extra)) in rows.iter().enumerate() {
        let name = format!("conflict-{index}");
        let first = request(&name, serde_json::json!({ "clientRequestToken": "token-conflict" }));
        let created = create(&router, &first).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: setup expected 200 from the first create, got {created:?}"
        );

        let second = request(&name, extra.clone());
        let answer = create(&router, &second).await;
        assert_eq!(
            answer.status, 409,
            "case {case:?}: expected HTTP 409, got {answer:?}"
        );
        assert_eq!(
            error_code(&answer),
            RESOURCE_IN_USE,
            "case {case:?}: expected {RESOURCE_IN_USE}, got {answer:?}"
        );
        assert_eq!(
            answer.message(),
            format!("Cluster already exists with name: {name}"),
            "case {case:?}: expected the conflict sentence, got {:?}",
            answer.message()
        );
        assert_eq!(
            cluster_name_of(&answer),
            Some(name.as_str()),
            "case {case:?}: expected the refusal to name the cluster, got {:?}",
            cluster_name_of(&answer)
        );
        let listed = names_of(&list(&router, "maxResults=100").await);
        assert_eq!(
            listed.iter().filter(|listed| **listed == name).count(),
            1,
            "case {case:?}: exactly one cluster must carry the name, got {listed:?}"
        );
    }
}
