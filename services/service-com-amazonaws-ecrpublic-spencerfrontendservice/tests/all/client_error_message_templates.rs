// SPDX-License-Identifier: CC0-1.0

//! The `$.message` each ECR Public client error carries, over a SET of requests.
//!
//! The rule under test: each modelled client error reports the AWS sentence for that
//! exception, interpolating the identifiers the request itself carried (the repository
//! name, the whole requested image identifier, the claimed digest) plus the registry
//! the call resolved to, and nothing the caller did not ask about — no digest this
//! registry computed, no count of what the store happens to hold.
//!
//! So each table below spans the class the template generalises over — the forms the
//! modelled `repositoryName` pattern admits, the ways a claimed digest can differ from
//! the manifest's, the ways an `ImageIdentifier` can name nothing, how the request
//! names the registry (not at all, by account id, by alias), and how many images a
//! repository holds — and every row's expectation is BUILT from that row's own request
//! rather than copied from a recording.
//!
//! The wire plumbing is the crate's shared harness (`tests/all/harness.rs`).

use serde_json::{json, Value};

use crate::repository_image_lifecycle::{digest_of, ok, refused, service, REGISTRY_ID};

/// This registry's alias (`state::DEFAULT_REGISTRY_ALIAS`), which `registryId` accepts
/// in place of the account id: a request naming it must still see the account id in the
/// message, because the message reports the registry the call RESOLVED to.
const REGISTRY_ALIAS: &str = "emulator";

/// The `$.message` of a refused call.
fn message(body: &Value, case: &str) -> String {
    body["message"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a string $.message, got {body}"))
        .to_string()
}

/// A manifest document of the given name — distinct names hash to distinct digests.
fn manifest(name: &str) -> String {
    json!({"schemaVersion": 2, "name": name}).to_string()
}

#[tokio::test]
async fn put_image_names_the_requested_digest_repository_and_registry_on_a_mismatch() {
    // (case, repositoryName, the manifest pushed, the digest claimed for it, the
    // `registryId` the request names)
    let mismatch_digest = digest_of(&manifest("some other manifest"));
    let rows: Vec<(&str, &str, String, String, Option<&str>)> = vec![
        (
            "single-segment name, all-zero digest, no registryId",
            "probe",
            manifest("probe"),
            format!("sha256:{}", "0".repeat(64)),
            None,
        ),
        (
            "nested name, another manifest's digest, registryId by account id",
            "team/app",
            manifest("nested"),
            mismatch_digest.clone(),
            Some(REGISTRY_ID),
        ),
        (
            "separator-rich name, digest with no algorithm, registryId by alias",
            "a_b.c-d",
            manifest("separators"),
            "deadbeef".to_string(),
            Some(REGISTRY_ALIAS),
        ),
        (
            "deeply nested name, uppercase hex digest",
            "org/team/sub/app",
            manifest("deep"),
            format!("sha256:{}", "AB".repeat(32)),
            None,
        ),
        (
            "large manifest, empty-hex digest",
            "bulk-manifest",
            json!({"schemaVersion": 2, "name": "bulk", "pad": "x".repeat(40_000)}).to_string(),
            "sha256:".to_string(),
            Some(REGISTRY_ID),
        ),
    ];

    let router = service();
    for (case, repository, pushed, claimed, registry) in rows {
        ok(
            &router,
            "CreateRepository",
            json!({"repositoryName": repository}),
            case,
        )
        .await;
        let computed = digest_of(&pushed);
        assert_ne!(
            claimed, computed,
            "case {case:?}: the row's claimed digest must differ from the manifest's"
        );

        let mut request = json!({
            "repositoryName": repository,
            "imageManifest": pushed,
            "imageDigest": claimed,
            "imageTag": "mismatch",
        });
        if let Some(registry) = registry {
            request["registryId"] = json!(registry);
        }
        let body = refused(
            &router,
            "PutImage",
            request,
            "ImageDigestDoesNotMatchException",
            case,
        )
        .await;

        let actual = message(&body, case);
        let expected = format!(
            "The image with digest '{claimed}' in repository '{repository}' and registry ID \
             '{REGISTRY_ID}' does not match the image digest that Amazon ECR has calculated."
        );
        assert_eq!(
            actual, expected,
            "case {case:?}: expected {expected:?}, got {actual:?}"
        );
        assert!(
            !actual.contains(&computed),
            "case {case:?}: the message must not leak the digest this registry computed \
             ({computed:?}), got {actual:?}"
        );
    }
}

#[tokio::test]
async fn describe_images_renders_the_whole_requested_image_identifier_when_it_resolves_to_nothing()
{
    // Two repositories, so the name in the message is the one the request named.
    let repositories = ["probe-repo", "team/app"];
    let stored = manifest("stored");
    let stored_digest = digest_of(&stored);
    let absent_digest = format!("sha256:{}", "1".repeat(64));

    let router = service();
    for repository in repositories {
        ok(
            &router,
            "CreateRepository",
            json!({"repositoryName": repository}),
            repository,
        )
        .await;
        ok(
            &router,
            "PutImage",
            json!({
                "repositoryName": repository,
                "imageManifest": stored,
                "imageTag": "live",
            }),
            repository,
        )
        .await;
    }

    // (case, the `imageIds` entry, the imageDigest the message must render, the
    // imageTag it must render) — an absent member renders as `null`.
    let rows: Vec<(&str, Value, String, String)> = vec![
        (
            "digest only, nothing stored under it",
            json!({"imageDigest": absent_digest}),
            absent_digest.clone(),
            "null".to_string(),
        ),
        (
            "tag only, no such tag",
            json!({"imageTag": "never-pushed"}),
            "null".to_string(),
            "never-pushed".to_string(),
        ),
        (
            "both members, neither known",
            json!({"imageDigest": absent_digest, "imageTag": "never-pushed"}),
            absent_digest.clone(),
            "never-pushed".to_string(),
        ),
        (
            "stored tag with a digest it does not name",
            json!({"imageDigest": absent_digest, "imageTag": "live"}),
            absent_digest.clone(),
            "live".to_string(),
        ),
        (
            "stored digest with a tag it does not carry",
            json!({"imageDigest": stored_digest, "imageTag": "never-pushed"}),
            stored_digest.clone(),
            "never-pushed".to_string(),
        ),
    ];

    for repository in repositories {
        for (case, entry, rendered_digest, rendered_tag) in &rows {
            let case = format!("{repository} / {case}");
            let body = refused(
                &router,
                "DescribeImages",
                json!({"repositoryName": repository, "imageIds": [entry]}),
                "ImageNotFoundException",
                &case,
            )
            .await;

            let actual = message(&body, &case);
            let expected = format!(
                "The image with imageId {{imageDigest:'{rendered_digest}', \
                 imageTag:'{rendered_tag}'}} does not exist within the repository with name \
                 '{repository}' in the registry with id '{REGISTRY_ID}'"
            );
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }
}

#[tokio::test]
async fn create_repository_names_the_repository_and_registry_of_a_name_already_taken() {
    // Names spanning the forms the modelled `repositoryName` pattern admits, plus the
    // ways the second request can name the registry.
    let rows: [(&str, &str, Option<&str>); 4] = [
        ("single segment, no registryId", "probe", None),
        ("nested name, registryId by account id", "team/app", Some(REGISTRY_ID)),
        (
            "separator-rich name, registryId by alias",
            "a_b.c-d2",
            Some(REGISTRY_ALIAS),
        ),
        ("deeply nested name", "org/team/sub/app", None),
    ];

    let router = service();
    for (case, repository, registry) in rows {
        ok(
            &router,
            "CreateRepository",
            json!({"repositoryName": repository}),
            case,
        )
        .await;

        let mut request = json!({"repositoryName": repository});
        if let Some(registry) = registry {
            request["registryId"] = json!(registry);
        }
        let body = refused(
            &router,
            "CreateRepository",
            request,
            "RepositoryAlreadyExistsException",
            case,
        )
        .await;

        let actual = message(&body, case);
        let expected = format!(
            "The repository with name '{repository}' already exists in the registry with id \
             '{REGISTRY_ID}'"
        );
        assert_eq!(
            actual, expected,
            "case {case:?}: expected {expected:?}, got {actual:?}"
        );
    }
}

#[tokio::test]
async fn delete_repository_reports_a_non_empty_repository_without_counting_its_images() {
    // (case, repositoryName, how many images it holds, whether `force` is sent as false
    // or left out) — the message must be the same sentence for every row, so what the
    // store holds cannot show through it.
    let rows: [(&str, &str, usize, bool); 4] = [
        ("one image, force omitted", "probe", 1, false),
        ("one image, force sent false", "team/app", 1, true),
        ("three images, force omitted", "many-images", 3, false),
        ("twelve images, force sent false", "org/team/sub/app", 12, true),
    ];

    let router = service();
    for (case, repository, image_count, send_force) in rows {
        ok(
            &router,
            "CreateRepository",
            json!({"repositoryName": repository}),
            case,
        )
        .await;
        for index in 0..image_count {
            ok(
                &router,
                "PutImage",
                json!({
                    "repositoryName": repository,
                    "imageManifest": manifest(&format!("{repository}-{index}")),
                    "imageTag": format!("v{index}"),
                }),
                case,
            )
            .await;
        }

        let mut request = json!({"repositoryName": repository});
        if send_force {
            request["force"] = json!(false);
        }
        let body = refused(
            &router,
            "DeleteRepository",
            request,
            "RepositoryNotEmptyException",
            case,
        )
        .await;

        let actual = message(&body, case);
        let expected = format!(
            "The repository with name '{repository}' in registry with id '{REGISTRY_ID}' \
             cannot be deleted because it still contains images"
        );
        assert_eq!(
            actual, expected,
            "case {case:?}: expected {expected:?}, got {actual:?}"
        );
        // Beyond the exact sentence: nothing state-derived may appear at all, so once
        // the repository name and the registry id are removed no digit is left to
        // report a count with.
        let without_identifiers = actual.replace(repository, "").replace(REGISTRY_ID, "");
        assert!(
            !without_identifiers.chars().any(char::is_numeric),
            "case {case:?}: the message must not report how many images remain, got {actual:?}"
        );
    }
}

#[tokio::test]
async fn set_repository_policy_reports_the_lockout_refusal_rather_than_a_constraint() {
    // A document that would deny this account the policy-setting action is refused, when
    // `force` is absent or false, with the SERVICE's lockout refusal: the fixed sentence
    // AWS documents the `force` parameter in — what the document would do, and the
    // parameter to re-send with. `force` is a legal input value rather than a constrained
    // one, so the message names no member and no constraint.
    //
    // (case, the document that would lock the policy, whether `force` is sent as false or
    // left out) — the rows span the ways a `Deny` reaches this account (every principal,
    // this account by ARN, a wildcard action, a second statement) and both ways the
    // request can decline to force it, because the refusal is one sentence for all of
    // them.
    let rows: [(&str, &str, bool); 4] = [
        (
            "every principal, this action, force omitted",
            r#"{"Version":"2012-10-17","Statement":[{"Effect":"Deny","Principal":"*","Action":"ecr-public:SetRepositoryPolicy"}]}"#,
            false,
        ),
        (
            "every principal, every action, force sent false",
            r#"{"Version":"2012-10-17","Statement":[{"Effect":"Deny","Principal":"*","Action":"*"}]}"#,
            true,
        ),
        (
            "this account by ARN, an action prefix, force omitted",
            r#"{"Version":"2012-10-17","Statement":[{"Effect":"Deny","Principal":{"AWS":"arn:aws:iam::000000000000:root"},"Action":"ecr-public:Set*"}]}"#,
            false,
        ),
        (
            "a second statement that denies it, force sent false",
            r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":"*","Action":"ecr-public:BatchGetImage"},{"Effect":"Deny","Principal":"*","Action":"ecr-public:SetRepositoryPolicy"}]}"#,
            true,
        ),
    ];

    // The sentence AWS documents the `force` parameter in; the same one for every row.
    let expected = "You are about to set a repository policy that will prevent you from \
                    setting another one in the future. Use the force parameter to override \
                    this exception.";

    let router = service();
    for (index, (case, document, send_force)) in rows.into_iter().enumerate() {
        let repository = format!("policy/lockout-refusal-{index}");
        ok(
            &router,
            "CreateRepository",
            json!({"repositoryName": repository}),
            case,
        )
        .await;

        let mut request = json!({"repositoryName": repository, "policyText": document});
        if send_force {
            request["force"] = json!(false);
        }
        let body = refused(
            &router,
            "SetRepositoryPolicy",
            request,
            "InvalidParameterException",
            case,
        )
        .await;

        let actual = message(&body, case);
        assert_eq!(
            actual, expected,
            "case {case:?}: expected {expected:?}, got {actual:?}"
        );

        // With `force` the same document is stored, so the refusal is about the lockout
        // and not about the document being unacceptable.
        ok(
            &router,
            "SetRepositoryPolicy",
            json!({"repositoryName": repository, "policyText": document, "force": true}),
            case,
        )
        .await;
    }
}
