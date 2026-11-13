// SPDX-License-Identifier: CC0-1.0

//! How this service's REFUSAL ENVELOPES are shaped: the status, the error shape and the
//! structured members (`resourceId`, `resourceType`, `message`) a client branches on.
//!
//! Every case drives real `restJson1` requests through the `MultiServiceRouter` the
//! production emulator serves with, so what is asserted here is the wire envelope a
//! CodeArtifact client reads — not a handler's return value.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than on one recorded
//! example:
//!
//! 1. A `generic` publish that names no `namespace` is refused `ValidationException`
//!    (400), naming the member and the condition that requires it, and stores nothing: the
//!    namespace is a COMPONENT of a generic version's identity
//!    (`<namespace>/<package>/<version>`), which is why the format requires a member the
//!    model leaves optional for the formats that derive it.
//! 2. A refusal that names a PACKAGE VERSION reports that whole coordinate in
//!    `resourceId` — never the bare version string, which identifies nothing.
//! 3. `DescribePackageVersion` resolves a version by its whole (format, namespace,
//!    package, version) coordinate, and a package exists in a repository only through its
//!    versions — so a coordinate the repository holds no version for is a 404 naming the
//!    PACKAGE at that coordinate: `resourceType` `package`, `resourceId` the package name.
//! 4. `AssociateExternalConnection` classifies its refusals by what is wrong with the
//!    request: a second connection is the one-per-repository limit, so
//!    `ValidationException` (400) — a permanent input error, not the transient quota an
//!    SDK retries; a well-formed name that is no supported registry is a connection that
//!    does not exist, so `ResourceNotFoundException` (404); the connection already held is
//!    `ConflictException` (409).
//! 5. An ARN-addressed resource that is not held reports the ID of the resource its ARN
//!    named — a domain or repository name, a package group's pattern — never the input
//!    ARN, and states the sentence that resource's own kind is refused with.
//! 6. A `packageGroup` that is not a path from the domain root fails a constraint on the
//!    member, so it is refused in the AWS constraint-violation envelope naming the member
//!    and the requirement, at EVERY operation that takes one.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::domain_repository_lifecycle::create_repository;
use crate::external_connection::{associate, urlencode};
use crate::harness::{
    call, create_domain, domain_not_found, emulator, external_connection_not_found,
    repository_not_found, Answer, ACCOUNT, REGION,
};
use crate::package_group_hierarchy::{create_package_group, describe_package_group};
use crate::package_version_publish::{
    binary_content, describe, emulator_with_repository, publish, Publish, DOMAIN, NAMESPACE,
    REPOSITORY,
};
use crate::resource_tags::{encode_query, list_tags, pairs, tag};

/// RULE 1: the refusal a `generic` publish with no `namespace` gets. Built from the
/// request's own members — the member that is missing and the `format` that requires it.
const NAMESPACE_REQUIRED: &str = "'namespace' is required when 'format' is 'generic'.";

/// RULE 4: the limit `AssociateExternalConnection` states. One connection per repository
/// (the operation's own documentation: "One external connection is allowed per
/// repository").
const CONNECTION_LIMIT: &str = "ExternalConnections for repositories are currently limited to 1";

/// RULE 6: the requirement a `packageGroup` that is not a path from the domain root failed.
const PATTERN_REQUIREMENT: &str = "Pattern must start with '/'.";

/// RULE 6: the AWS constraint-violation envelope over ONE member, as
/// `tests/all/error_message_composition.rs` pins it for the boundary's own refusals: the
/// value echoed, the member path, and the requirement.
fn one_constraint_error(value: &str, member: &str, requirement: &str) -> String {
    format!(
        "1 validation error detected: Value '{value}' at '{member}' failed to satisfy \
         constraint: {requirement}"
    )
}

/// RULE 3: the sentence a package the repository holds no version of is refused with.
fn package_not_found(namespace: &str, package: &str, repository: &str) -> String {
    format!(
        "The generic package '{package}' with namespace '{namespace}' does not exist in \
         repository '{repository}'."
    )
}

/// RULE 2 and 3: a package version's whole coordinate inside its repository.
fn coordinate(namespace: &str, package: &str, version: &str) -> String {
    format!("{namespace}/{package}/{version}")
}

// RULE 5: the sentences a domain and a repository that are not held are refused with —
// which the tagging operations state for an ARN-addressed resource too — come from the
// shared harness, so this suite and `tests/all/error_message_composition.rs` assert one
// wording. The package-group sentence below is this suite's own.

fn package_group_not_found(domain: &str, owner: &str, pattern: &str) -> String {
    format!(
        "Package Group not found not found. Package Group '{pattern}' in domain {domain} \
         owned by account {owner} does not exist"
    )
}

/// The `resourceId`/`resourceType`/`message` of a refusal, as the wire reports them.
fn envelope(answer: &Answer) -> (Option<&str>, Option<&str>, String) {
    (
        answer.body.get("resourceId").and_then(Value::as_str),
        answer.body.get("resourceType").and_then(Value::as_str),
        answer.message(),
    )
}

/// One RULE 3 row: the coordinate a read addresses, and the envelope its 404 must carry.
#[derive(Debug)]
struct MissingLevel {
    label: &'static str,
    namespace: Option<&'static str>,
    package: &'static str,
    version: &'static str,
    /// The resource the refusal must name, and its id.
    resource_type: &'static str,
    resource_id: String,
    message: String,
}

/// One RULE 4 row: the connection a repository already holds, the one asked for next, and
/// the refusal that pair must get.
#[derive(Debug)]
struct ConnectionLimit {
    label: &'static str,
    first: &'static str,
    second: &'static str,
    status: u16,
    code: &'static str,
    /// The message the rule fixes, or `None` where only the classification is the rule.
    message: Option<&'static str>,
}

/// `PutPackageOriginConfiguration`: creates the package record (with no versions) when the
/// repository does not hold it, which is one of RULE 3's rows.
async fn put_package_origin_configuration(
    router: &MultiServiceRouter,
    namespace: &str,
    package: &str,
) -> Answer {
    call(
        router,
        "POST",
        &format!(
            "/v1/package?domain={DOMAIN}&repository={REPOSITORY}&format=generic\
             &namespace={namespace}&package={package}"
        ),
        Some(json!({ "restrictions": { "publish": "ALLOW", "upstream": "ALLOW" } })),
    )
    .await
}

/// `DescribePackageVersion` for a coordinate given member by member, so a row can address
/// a package or a version that no [`Publish`] in this suite created.
async fn describe_version(
    router: &MultiServiceRouter,
    namespace: Option<&str>,
    package: &str,
    version: &str,
) -> Answer {
    let mut uri = format!(
        "/v1/package/version?domain={DOMAIN}&repository={REPOSITORY}&format=generic\
         &package={package}&version={version}"
    );
    if let Some(namespace) = namespace {
        uri.push_str(&format!("&namespace={namespace}"));
    }
    call(router, "GET", &uri, None).await
}

// ---------------------------------------------------------------------------
// RULE 1: `namespace` is required of a generic publish.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_generic_publish_without_a_namespace_is_refused_and_stores_nothing() {
    let router = emulator_with_repository().await;

    // Rows differ along what the rule generalises over: the package name, the shape of the
    // version string, the asset (name and size, one byte through 4 KiB), and whether the
    // publish would have left the version open (`unfinished`) or closed it.
    let cases = vec![
        (
            "one byte, closing the version",
            Publish::new("ns-less-a", "1.0.0", "payload.bin", vec![0x00]).without_namespace(),
        ),
        (
            "text asset, pre-release version",
            Publish::new("ns-less-b", "2.0.0-rc.1", "notes.txt", b"hello".to_vec())
                .without_namespace(),
        ),
        (
            "4 KiB binary, date-shaped version",
            Publish::new("ns-less-c", "20240101", "bundle.tar.gz", binary_content(4096, 7))
                .without_namespace(),
        ),
        (
            "left open with `unfinished`",
            Publish::new("ns-less-d", "0.0.1", "part-0.bin", binary_content(17, 3))
                .without_namespace()
                .unfinished(true),
        ),
    ];

    for (label, case) in &cases {
        let refused = publish(&router, case).await;
        assert_eq!(
            (refused.status, refused.code(), refused.message()),
            (
                400,
                "ValidationException".to_string(),
                NAMESPACE_REQUIRED.to_string()
            ),
            "case {label:?}: expected a 400 ValidationException naming the member and the \
             condition, got {refused:?}"
        );

        // Nothing was stored under the namespace-less identity the request named...
        let absent = describe(&router, case).await;
        assert_eq!(
            (absent.status, absent.code()),
            (404, "ResourceNotFoundException".to_string()),
            "case {label:?}: a refused publish must store nothing, got {absent:?}"
        );
        // ...and nothing under the namespace a later caller would address it by either.
        let under_namespace =
            describe_version(&router, Some(NAMESPACE), &case.package, &case.version).await;
        assert_eq!(
            (under_namespace.status, under_namespace.code()),
            (404, "ResourceNotFoundException".to_string()),
            "case {label:?}: a refused publish must store nothing under any namespace, got \
             {under_namespace:?}"
        );
    }

    // The same requests WITH a namespace are accepted, so the refusals above are about the
    // absent member and nothing else.
    for (label, case) in &cases {
        let accepted = publish(&router, &case.clone().namespace(NAMESPACE)).await;
        assert_eq!(
            (accepted.status, accepted.code()),
            (200, String::new()),
            "case {label:?}: the same publish carrying a namespace must succeed, got \
             {accepted:?}"
        );
        assert_eq!(
            accepted.body["namespace"].as_str(),
            Some(NAMESPACE),
            "case {label:?}: the stored version carries the namespace it was published \
             under, got {accepted:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// RULE 2: a package-version refusal reports the whole coordinate.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_refusal_naming_a_package_version_reports_its_whole_coordinate() {
    let router = emulator_with_repository().await;

    // Rows differ along every component of the coordinate: the namespace (short, dotted,
    // scoped), the package name, and the shape of the version string. Two rows share a
    // version string under different packages, which is exactly the pair a bare version
    // string could not tell apart.
    let rows = [
        ("short namespace", "team-a", "conflict-alpha", "1.0.0"),
        ("dotted namespace", "com.example.tools", "conflict-beta", "1.0.0"),
        ("scoped namespace", "@scope", "conflict-gamma", "2.1.3-rc.4"),
        ("date-shaped version", "reports", "conflict-delta", "20240101"),
    ];

    for (label, namespace, package, version) in rows {
        let published = Publish::new(package, version, "payload.bin", binary_content(64, 3))
            .namespace(namespace);
        assert_eq!(
            publish(&router, &published).await.status,
            200,
            "case {label:?}: the first publish must succeed"
        );

        // A closed version is immutable, so republishing into it conflicts — and the
        // conflict names WHICH version it is about.
        let refused = publish(&router, &published).await;
        let expected = coordinate(namespace, package, version);
        assert_eq!(
            (refused.status, refused.code()),
            (409, "ConflictException".to_string()),
            "case {label:?}: expected a 409 ConflictException, got {refused:?}"
        );
        assert_eq!(
            (
                refused.body["resourceId"].as_str(),
                refused.body["resourceType"].as_str()
            ),
            (Some(expected.as_str()), Some("package-version")),
            "case {label:?}: expected resourceId {expected:?} of type package-version, got \
             {refused:?}"
        );
        assert_ne!(
            refused.body["resourceId"].as_str(),
            Some(version),
            "case {label:?}: the bare version string identifies no package, got {refused:?}"
        );

        // The READ path answers for a coordinate it holds no version for by naming the
        // PACKAGE at that coordinate (RULE 3), so the two refusals are about different
        // resources: this one is not the version's id.
        let missing = describe_version(&router, Some(namespace), package, "9.9.9").await;
        assert_eq!(
            (
                missing.status,
                missing.code(),
                missing.body["resourceId"].as_str(),
                missing.body["resourceType"].as_str(),
            ),
            (
                404,
                "ResourceNotFoundException".to_string(),
                Some(package),
                Some("package"),
            ),
            "case {label:?}: expected a 404 naming the package {package:?}, got {missing:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// RULE 3: the 404 names the package at the addressed coordinate.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn describe_package_version_404_names_the_package_at_the_coordinate() {
    let router = emulator_with_repository().await;

    // A package holding a version, so the rows that address another version of it are
    // about that version and not about an empty repository.
    let held = Publish::new("level-held", "1.0.0", "payload.bin", binary_content(32, 1))
        .namespace(NAMESPACE);
    assert_eq!(
        publish(&router, &held).await.status,
        200,
        "the control publish must succeed"
    );

    // A package record carrying NO version: `PutPackageOriginConfiguration` creates one for
    // a package the repository does not hold.
    assert_eq!(
        put_package_origin_configuration(&router, NAMESPACE, "level-empty")
            .await
            .status,
        200,
        "PutPackageOriginConfiguration must succeed"
    );

    // Every row addresses a coordinate the repository holds no version for, and each gets
    // the SAME envelope: the package at that coordinate. Rows differ in WHY no version is
    // there — a package nothing ever created, a package record with no versions, a package
    // whose versions are stored under another namespace (the namespace is part of the key,
    // so that addresses another package entirely), and a version string the package does
    // not hold.
    let rows: Vec<MissingLevel> = vec![
        MissingLevel {
            label: "a package nothing created",
            namespace: Some(NAMESPACE),
            package: "level-never",
            version: "1.0.0",
            resource_type: "package",
            resource_id: "level-never".to_string(),
            message: package_not_found(NAMESPACE, "level-never", REPOSITORY),
        },
        MissingLevel {
            label: "a package record holding no version",
            namespace: Some(NAMESPACE),
            package: "level-empty",
            version: "1.0.0",
            resource_type: "package",
            resource_id: "level-empty".to_string(),
            message: package_not_found(NAMESPACE, "level-empty", REPOSITORY),
        },
        MissingLevel {
            label: "another namespace's package",
            namespace: Some("other-namespace"),
            package: "level-held",
            version: "1.0.0",
            resource_type: "package",
            resource_id: "level-held".to_string(),
            message: package_not_found("other-namespace", "level-held", REPOSITORY),
        },
        MissingLevel {
            label: "a version the package does not hold",
            namespace: Some(NAMESPACE),
            package: "level-held",
            version: "9.9.9",
            resource_type: "package",
            resource_id: "level-held".to_string(),
            message: package_not_found(NAMESPACE, "level-held", REPOSITORY),
        },
    ];

    for row in &rows {
        let label = row.label;
        let answer = describe_version(&router, row.namespace, row.package, row.version).await;
        assert_eq!(
            (answer.status, answer.code()),
            (404, "ResourceNotFoundException".to_string()),
            "case {label:?}: expected a 404 ResourceNotFoundException, got {answer:?}"
        );
        let (resource_id, resource_type, message) = envelope(&answer);
        assert_eq!(
            (resource_type, resource_id, message.as_str()),
            (
                Some(row.resource_type),
                Some(row.resource_id.as_str()),
                row.message.as_str()
            ),
            "case {label:?}: expected the {:?} {:?} reported as missing, got {answer:?}",
            row.resource_type,
            row.resource_id
        );
    }

    // The control version is still readable: the rows above refuse because what they
    // address is missing, not because every read refuses.
    assert_eq!(
        describe(&router, &held).await.status,
        200,
        "the held version must still be readable"
    );
}

// ---------------------------------------------------------------------------
// RULE 4: the external-connection refusals are classified by what is wrong.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn external_connection_refusals_are_classified_by_what_is_wrong() {
    let router = emulator();
    let domain = "envelope-ec-domain";
    assert_eq!(
        create_domain(&router, domain, None).await.status,
        200,
        "domain setup must succeed"
    );

    // A well-formed name that is no supported registry: each row satisfies
    // `ExternalConnectionName` (len 2..100, pattern `^[A-Za-z0-9][A-Za-z0-9._\-:]{1,99}$`),
    // so the boundary admits it and what is wrong is that the registry does not exist.
    // Rows differ in how they miss: an unknown registry, a bare registry name with no
    // scope, a prefix of a published name, a case variant, and a published name with a
    // suffix.
    for (index, connection) in [
        "public:does-not-exist",
        "npmjs",
        "public:maven",
        "PUBLIC:NPMJS",
        "public:npmjs.org",
    ]
    .iter()
    .enumerate()
    {
        let repository = format!("envelope-ec-missing-{index}");
        assert_eq!(
            create_repository(&router, domain, &repository, None, &[])
                .await
                .status,
            200,
            "case {connection:?}: repository setup must succeed"
        );
        let answer = associate(&router, domain, &repository, connection).await;
        assert_eq!(
            (answer.status, answer.code()),
            (404, "ResourceNotFoundException".to_string()),
            "case {connection:?}: a well-formed name that names no registry does not \
             exist, got {answer:?}"
        );
        assert_eq!(
            (
                answer.body["resourceId"].as_str(),
                answer.message().as_str()
            ),
            (
                Some(*connection),
                external_connection_not_found(connection).as_str()
            ),
            "case {connection:?}: the refusal names the connection asked for, got {answer:?}"
        );
    }

    // The one-per-repository limit, over rows differing in which registry is held first
    // and which is asked for second (two Maven registries share a package format, the
    // other pair does not), and in whether the second name is the one already held.
    let rows: &[ConnectionLimit] = &[
        ConnectionLimit {
            label: "a second connection of another format",
            first: "public:npmjs",
            second: "public:pypi",
            status: 400,
            code: "ValidationException",
            message: Some(CONNECTION_LIMIT),
        },
        ConnectionLimit {
            label: "a second connection of the same format",
            first: "public:maven-central",
            second: "public:maven-clojars",
            status: 400,
            code: "ValidationException",
            message: Some(CONNECTION_LIMIT),
        },
        ConnectionLimit {
            label: "the connection already held",
            first: "public:crates-io",
            second: "public:crates-io",
            status: 409,
            code: "ConflictException",
            message: None,
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let label = row.label;
        let repository = format!("envelope-ec-limit-{index}");
        assert_eq!(
            create_repository(&router, domain, &repository, None, &[])
                .await
                .status,
            200,
            "case {label:?}: repository setup must succeed"
        );
        let held = associate(&router, domain, &repository, row.first).await;
        assert_eq!(
            held.status, 200,
            "case {label:?}: the first associate must succeed, got {held:?}"
        );

        let refused = associate(&router, domain, &repository, row.second).await;
        assert_eq!(
            (refused.status, refused.code()),
            (row.status, row.code.to_string()),
            "case {label:?}: expected {} {}, got {refused:?}",
            row.status,
            row.code
        );
        if let Some(message) = row.message {
            assert_eq!(
                refused.message(),
                message,
                "case {label:?}: expected the limit stated as {message:?}, got {refused:?}"
            );
        }

        // Nothing was written: the repository still holds exactly the first connection.
        let read = call(
            &router,
            "GET",
            &format!("/v1/repository?domain={domain}&repository={repository}"),
            None,
        )
        .await;
        assert_eq!(
            read.body["repository"], held.body["repository"],
            "case {label:?}: a refused associate must change nothing, got {read:?}"
        );
    }

    // And a supported registry on a repository holding none is still accepted, so the
    // refusals above are about the request and not about this operation.
    let repository = "envelope-ec-ok";
    assert_eq!(
        create_repository(&router, domain, repository, None, &[])
            .await
            .status,
        200
    );
    let accepted = associate(&router, domain, repository, "public:npmjs").await;
    assert_eq!(
        accepted.status, 200,
        "a supported registry must be accepted, got {accepted:?}"
    );
    // The URI encoder the rows share is exercised on the accepted name too.
    assert!(
        urlencode("public:npmjs").contains("%3A"),
        "the suite's connection names travel percent-encoded"
    );
}

// ---------------------------------------------------------------------------
// RULE 5: an ARN-addressed resource reports its ID, not the ARN.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_arn_addressed_resource_that_is_not_held_reports_its_id() {
    let router = emulator();
    let domain = "envelope-arn";
    assert_eq!(create_domain(&router, domain, None).await.status, 200);
    assert_eq!(
        create_repository(&router, domain, "held-repo", None, &[])
            .await
            .status,
        200
    );

    // Rows differ along the resource KIND the ARN names (domain, repository, package
    // group), WHICH level of it is absent (the resource itself, or the domain containing
    // it), and whether the ARN pins the resource to another account.
    let rows: Vec<(&str, String, Option<&str>, String, String)> = vec![
        (
            "a domain this store does not hold",
            format!("arn:aws:codeartifact:{REGION}:{ACCOUNT}:domain/absent-domain"),
            Some("domain"),
            "absent-domain".to_string(),
            domain_not_found("absent-domain", ACCOUNT),
        ),
        (
            "a held domain pinned to another account",
            format!("arn:aws:codeartifact:{REGION}:999999999999:domain/{domain}"),
            Some("domain"),
            domain.to_string(),
            domain_not_found(domain, "999999999999"),
        ),
        (
            "a repository the held domain does not hold",
            format!("arn:aws:codeartifact:{REGION}:{ACCOUNT}:repository/{domain}/absent-repo"),
            Some("repository"),
            "absent-repo".to_string(),
            repository_not_found(domain, ACCOUNT, "absent-repo"),
        ),
        (
            "a repository in a domain this store does not hold",
            format!("arn:aws:codeartifact:{REGION}:{ACCOUNT}:repository/absent-domain/held-repo"),
            Some("repository"),
            "held-repo".to_string(),
            repository_not_found("absent-domain", ACCOUNT, "held-repo"),
        ),
        (
            // `ResourceType` declares no value for a package group, so the kind is left
            // unset — the ID is still the group's own identifier, its pattern.
            "a package-group pattern the domain does not hold",
            format!("arn:aws:codeartifact:{REGION}:{ACCOUNT}:package-group/{domain}/npm/*"),
            None,
            "/npm/*".to_string(),
            package_group_not_found(domain, ACCOUNT, "/npm/*"),
        ),
    ];

    // Both tagging operations read the same ARN, so every row runs against both.
    for (label, arn, resource_type, resource_id, message) in &rows {
        for (operation, answer) in [
            ("TagResource", tag(&router, arn, &pairs(&[("probe", "1")])).await),
            ("ListTagsForResource", list_tags(&router, arn).await),
        ] {
            assert_eq!(
                (answer.status, answer.code()),
                (404, "ResourceNotFoundException".to_string()),
                "case {label:?}/{operation}: expected a 404 ResourceNotFoundException, got \
                 {answer:?}"
            );
            let (got_id, got_type, got_message) = envelope(&answer);
            assert_eq!(
                (got_id, got_type, got_message.as_str()),
                (
                    Some(resource_id.as_str()),
                    *resource_type,
                    message.as_str()
                ),
                "case {label:?}/{operation}: expected resourceId {resource_id:?} of kind \
                 {resource_type:?}, got {answer:?}"
            );
            assert_ne!(
                got_id,
                Some(arn.as_str()),
                "case {label:?}/{operation}: the ARN is not the ID of a resource, got \
                 {answer:?}"
            );
        }
    }

    // The resources that DO exist are still readable through the same ARNs, so the rows
    // above refuse because what they address is absent.
    let held = format!("arn:aws:codeartifact:{REGION}:{ACCOUNT}:domain/{domain}");
    assert_eq!(
        list_tags(&router, &held).await.status,
        200,
        "the held domain must still be readable by ARN"
    );
    assert!(
        encode_query("arn:aws:codeartifact").contains("%3A"),
        "the suite's ARNs travel percent-encoded"
    );
}

// ---------------------------------------------------------------------------
// RULE 6: a packageGroup pattern is a path from the domain root.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_package_group_pattern_that_is_not_a_path_is_a_constraint_violation() {
    let router = emulator();
    let domain = "envelope-pg";
    assert_eq!(create_domain(&router, domain, None).await.status, 200);

    // Every row satisfies `PackageGroupPattern` (len 2..520, pattern
    // `^[^\p{C}\p{IsWhitespace}]+$`), so the boundary admits it and the leading `/` is the
    // constraint this service enforces. Rows differ in how they miss it: a relative prefix
    // pattern, a relative exact pattern, a bare word, a wildcard pair, and a value whose
    // slash is not leading.
    for pattern in [
        "npm/*",
        "npm/pkg$",
        "sim2real-probe-not-a-pattern",
        "**",
        "a/npm/*",
    ] {
        let expected = one_constraint_error(pattern, "packageGroup", PATTERN_REQUIREMENT);

        // The rule holds at every operation that takes a `packageGroup`: the create and
        // the read refuse the same value the same way.
        let created = create_package_group(&router, domain, pattern, None, None).await;
        let described = describe_package_group(&router, domain, pattern).await;
        for (operation, answer) in [
            ("CreatePackageGroup", created),
            ("DescribePackageGroup", described),
        ] {
            assert_eq!(
                (answer.status, answer.code(), answer.message()),
                (400, "ValidationException".to_string(), expected.clone()),
                "case {pattern:?}/{operation}: expected the constraint-violation envelope \
                 {expected:?}, got {answer:?}"
            );
        }
    }

    // The same patterns WITH the leading `/` are accepted and readable back, so the
    // refusals above are about that one constraint.
    for pattern in ["/npm/*", "/npm/pkg$", "/sim2real-probe-not-a-pattern"] {
        let created = create_package_group(&router, domain, pattern, None, None).await;
        assert_eq!(
            (created.status, created.code()),
            (200, String::new()),
            "case {pattern:?}: a path from the domain root must be accepted, got {created:?}"
        );
        let described = describe_package_group(&router, domain, pattern).await;
        assert_eq!(
            described.group().get("pattern").and_then(Value::as_str),
            Some(pattern),
            "case {pattern:?}: the created group must be readable back, got {described:?}"
        );
    }
}
