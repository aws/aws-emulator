// SPDX-License-Identifier: CC0-1.0

//! `UpdateRepositoryName` over the wire, driven as awsJson1_1 requests through the
//! router that serves the real emulator.
//!
//! The rule under test: a rename re-keys the STORED repository, so the same
//! resource — its id, creation date, description, default branch and whole history
//! — answers to the new name and no longer to the old one, while every field
//! CodeCommit derives from the name (the ARN, and both clone URLs, which name the
//! repository under the endpoint the asking request arrived at) is rebuilt for it
//! and the last-modified instant moves to the rename; and the new name has to be
//! one the account does not already hold, which is also why a rename frees the old
//! name for another repository.
//!
//! The tables below span the class the rule covers — name lengths at both modeled
//! bounds, the punctuation `RepositoryName`'s pattern allows, repositories with and
//! without a description, with and without history, and renames chained and
//! round-tripped — rather than one recorded request. The harness (`emulator`,
//! `ok`, `error`, `string_at`, `is_uuid`) is the shared one in
//! [`crate::repository_lifecycle`].

use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::repository_lifecycle::{
    emulator, error, is_uuid, ok, string_at, ENDPOINT_AUTHORITY, REGION,
};

/// The `repositoryMetadata` a `GetRepository` read reports for a name.
async fn metadata_of(router: &MultiServiceRouter, name: &str) -> Value {
    let got = ok(router, "GetRepository", &json!({ "repositoryName": name })).await;
    got.get("repositoryMetadata")
        .unwrap_or_else(|| panic!("no repositoryMetadata for {name:?} in {got}"))
        .clone()
}

fn as_f64(metadata: &Value, member: &str) -> f64 {
    metadata
        .get(member)
        .and_then(Value::as_f64)
        .unwrap_or_else(|| panic!("no {member} in {metadata}"))
}

/// The names `ListRepositories` reports, in the order it reports them.
async fn listed_names(router: &MultiServiceRouter) -> Vec<String> {
    let listed = ok(router, "ListRepositories", &json!({})).await;
    listed
        .get("repositories")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("no repositories in {listed}"))
        .iter()
        .map(|entry| string_at(entry, "/repositoryName"))
        .collect()
}

/// Write one file so the renamed repository has a branch and a history to carry
/// over.
async fn put_file(router: &MultiServiceRouter, repository: &str, branch: &str, path: &str) {
    ok(
        router,
        "PutFile",
        &json!({
            "repositoryName": repository,
            "branchName": branch,
            "filePath": path,
            "fileContent": aws_smithy_types::base64::encode(path.as_bytes()),
            "commitMessage": "init",
            "name": "Test Author",
            "email": "author@example.com",
        }),
    )
    .await;
}

/// One row of the rename table.
#[derive(Debug, Clone)]
struct Case {
    old_name: String,
    new_name: String,
    description: Option<String>,
    /// The branch and file path written before the rename, or `None` for a
    /// repository with no commit at all (whose `defaultBranch` stays absent).
    content: Option<(&'static str, &'static str)>,
}

/// The table: rows differ along what the rule generalises over — the direction
/// across `RepositoryName`'s modeled `length` 1..100 (ordinary to ordinary, the
/// maximum down to the minimum, the minimum up to the maximum), the characters its
/// modeled `pattern` `^[\w\.-]+$` allows (dots, dashes, underscores, digits), a
/// description present, empty or absent, and a repository with history against one
/// with none.
fn rename_cases() -> Vec<Case> {
    vec![
        Case {
            old_name: "demo-repo".to_string(),
            new_name: "demo-repo-renamed".to_string(),
            description: Some("bootstrap demo".to_string()),
            content: Some(("main", "README.md")),
        },
        Case {
            // The modeled maximum length down to the modeled minimum.
            old_name: "x".repeat(100),
            new_name: "a".to_string(),
            description: None,
            content: None,
        },
        Case {
            old_name: "dots.and_dashes-1".to_string(),
            new_name: "renamed.v2_final-9".to_string(),
            description: Some(String::new()),
            content: Some(("release-1.0", "/src/main/app.sh")),
        },
        Case {
            // The modeled minimum length up to the modeled maximum.
            old_name: "b".to_string(),
            new_name: "y".repeat(100),
            // The modeled maximum of `RepositoryDescription`'s length 0..1000.
            description: Some("z".repeat(1000)),
            content: Some(("feature/ünïcode", "docs/guía.md")),
        },
    ]
}

#[tokio::test]
async fn a_renamed_repository_keeps_its_identity_and_answers_to_the_new_name() {
    let router = emulator();

    for case in rename_cases() {
        let label = format!("{} -> {}", case.old_name, case.new_name);

        let mut create = json!({ "repositoryName": case.old_name });
        if let Some(description) = &case.description {
            create["repositoryDescription"] = json!(description);
        }
        ok(&router, "CreateRepository", &create).await;
        if let Some((branch, path)) = case.content {
            put_file(&router, &case.old_name, branch, path).await;
        }

        let before = metadata_of(&router, &case.old_name).await;
        let before_file = match case.content {
            Some((_, path)) => Some(
                ok(
                    &router,
                    "GetFile",
                    &json!({ "repositoryName": case.old_name, "filePath": path }),
                )
                .await,
            ),
            None => None,
        };

        // Separate the modification instants so lastModifiedDate strictly orders.
        std::thread::sleep(Duration::from_millis(5));

        let renamed = ok(
            &router,
            "UpdateRepositoryName",
            &json!({ "oldName": case.old_name, "newName": case.new_name }),
        )
        .await;
        assert!(
            renamed.is_null() || renamed == json!({}),
            "case {label:?}: UpdateRepositoryName models no output members, got {renamed}"
        );

        // The new name addresses the same resource.
        let after = metadata_of(&router, &case.new_name).await;
        assert_eq!(
            string_at(&after, "/repositoryName"),
            case.new_name,
            "case {label:?}: repositoryName after the rename"
        );
        let repository_id = string_at(&after, "/repositoryId");
        assert!(
            is_uuid(&repository_id),
            "case {label:?}: repositoryId must stay a uuid, got {repository_id:?}"
        );
        assert_eq!(
            repository_id,
            string_at(&before, "/repositoryId"),
            "case {label:?}: a rename keeps the repository id"
        );
        assert_eq!(
            after.get("creationDate"),
            before.get("creationDate"),
            "case {label:?}: a rename keeps the creationDate, got {after}"
        );
        assert_eq!(
            after.get("repositoryDescription").and_then(Value::as_str),
            case.description.as_deref(),
            "case {label:?}: a rename keeps the description, got {after}"
        );
        assert_eq!(
            after.get("defaultBranch"),
            before.get("defaultBranch"),
            "case {label:?}: a rename keeps the defaultBranch, got {after}"
        );
        assert_eq!(
            after.get("accountId"),
            before.get("accountId"),
            "case {label:?}: a rename keeps the accountId, got {after}"
        );

        // Every field derived from the name is rebuilt for the new name.
        assert_eq!(
            string_at(&after, "/Arn"),
            format!(
                "arn:aws:codecommit:{REGION}:{}:{}",
                engine::emulator_account_id(),
                case.new_name
            ),
            "case {label:?}: Arn must address the new name"
        );
        // The clone URL names the address this emulator serves git at — its own git
        // listener's, which a stock git client dials in the clear — with the new
        // repository name on its path.
        assert_eq!(
            string_at(&after, "/cloneUrlHttp"),
            format!(
                "{}/v1/repos/{}",
                string_at(&before, "/cloneUrlHttp")
                    .rsplit_once("/v1/repos/")
                    .map(|(origin, _)| origin.to_string())
                    .unwrap_or_default(),
                case.new_name
            ),
            "case {label:?}: cloneUrlHttp must address the new name"
        );
        assert_eq!(
            string_at(&after, "/cloneUrlSsh"),
            format!("ssh://{ENDPOINT_AUTHORITY}/v1/repos/{}", case.new_name),
            "case {label:?}: cloneUrlSsh must address the new name"
        );

        // The rename is a modification of the repository.
        let creation = as_f64(&after, "creationDate");
        let modified = as_f64(&after, "lastModifiedDate");
        assert!(
            modified > creation,
            "case {label:?}: lastModifiedDate {modified} must be after creationDate {creation}"
        );
        assert!(
            modified > as_f64(&before, "lastModifiedDate"),
            "case {label:?}: the rename must move lastModifiedDate forward, got {modified}"
        );

        // The old name addresses nothing, for metadata and for content alike.
        let code = error(
            &router,
            "GetRepository",
            &json!({ "repositoryName": case.old_name }),
            400,
        )
        .await;
        assert_eq!(
            code, "RepositoryDoesNotExistException",
            "case {label:?}: the old name must no longer address a repository"
        );

        // The history carries over untouched under the new name.
        if let (Some((_, path)), Some(before_file)) = (case.content, before_file) {
            let after_file = ok(
                &router,
                "GetFile",
                &json!({ "repositoryName": case.new_name, "filePath": path }),
            )
            .await;
            assert_eq!(
                after_file, before_file,
                "case {label:?}: the stored file must read back identically after the rename"
            );
            let code = error(
                &router,
                "GetFile",
                &json!({ "repositoryName": case.old_name, "filePath": path }),
                400,
            )
            .await;
            assert_eq!(
                code, "RepositoryDoesNotExistException",
                "case {label:?}: the old name must not address the repository's files"
            );
        }

        // The listing is the same re-keying: the new name, never the old one.
        let names = listed_names(&router).await;
        assert!(
            names.contains(&case.new_name),
            "case {label:?}: ListRepositories must report the new name, got {names:?}"
        );
        assert!(
            !names.contains(&case.old_name),
            "case {label:?}: ListRepositories must not report the old name, got {names:?}"
        );
    }
}

#[tokio::test]
async fn a_rename_the_rules_forbid_is_refused_and_changes_nothing() {
    let router = emulator();
    ok(
        &router,
        "CreateRepository",
        &json!({ "repositoryName": "rename-source", "repositoryDescription": "source" }),
    )
    .await;
    ok(
        &router,
        "CreateRepository",
        &json!({ "repositoryName": "rename-target" }),
    )
    .await;
    put_file(&router, "rename-source", "main", "f.txt").await;
    let source_before = metadata_of(&router, "rename-source").await;
    let target_before = metadata_of(&router, "rename-target").await;

    // Each row: why the rename is refused, the names sent, and the modeled error
    // the service answers with. The `.git` rows are the documented rule
    // "The suffix .git is prohibited"; the rest are the two existence rules.
    let refusals: Vec<(&str, String, String, &str)> = vec![
        (
            "a new name the account already holds",
            "rename-source".to_string(),
            "rename-target".to_string(),
            "RepositoryNameExistsException",
        ),
        (
            "the repository's own name, which the account holds like any other",
            "rename-source".to_string(),
            "rename-source".to_string(),
            "RepositoryNameExistsException",
        ),
        (
            "an old name no repository has",
            "rename-absent".to_string(),
            "rename-fresh".to_string(),
            "RepositoryDoesNotExistException",
        ),
        (
            "a .git suffix on the new name",
            "rename-source".to_string(),
            "rename-source.git".to_string(),
            "InvalidRepositoryNameException",
        ),
        (
            "a .git suffix the account would hold under any case",
            "rename-source".to_string(),
            "renamed.GIT".to_string(),
            "InvalidRepositoryNameException",
        ),
        (
            "a .git suffix on the old name",
            "rename-source.git".to_string(),
            "rename-fresh".to_string(),
            "InvalidRepositoryNameException",
        ),
    ];

    for (why, old_name, new_name, expected) in refusals {
        let code = error(
            &router,
            "UpdateRepositoryName",
            &json!({ "oldName": old_name, "newName": new_name }),
            400,
        )
        .await;
        assert_eq!(
            code, expected,
            "case {why:?}: oldName {old_name:?} newName {new_name:?} must be refused with {expected}"
        );
        assert_eq!(
            metadata_of(&router, "rename-source").await,
            source_before,
            "case {why:?}: a refused rename must leave rename-source exactly as it was"
        );
        assert_eq!(
            metadata_of(&router, "rename-target").await,
            target_before,
            "case {why:?}: a refused rename must leave rename-target exactly as it was"
        );
    }

    // No refused rename minted a repository under the name it was refused for.
    for absent in ["rename-fresh", "rename-source.git", "renamed.GIT"] {
        let code = error(
            &router,
            "GetRepository",
            &json!({ "repositoryName": absent }),
            400,
        )
        .await;
        assert_eq!(
            code, "RepositoryDoesNotExistException",
            "case {absent:?}: a refused rename must not create a repository"
        );
    }
    assert_eq!(
        listed_names(&router).await,
        vec!["rename-source".to_string(), "rename-target".to_string()],
        "a refused rename must not change which repositories the account holds"
    );
}

#[tokio::test]
async fn renames_chain_round_trip_and_free_the_name_they_leave() {
    let router = emulator();
    ok(
        &router,
        "CreateRepository",
        &json!({ "repositoryName": "chain-0", "repositoryDescription": "chained" }),
    )
    .await;
    put_file(&router, "chain-0", "main", "f.txt").await;
    let original = metadata_of(&router, "chain-0").await;
    let original_file = ok(
        &router,
        "GetFile",
        &json!({ "repositoryName": "chain-0", "filePath": "f.txt" }),
    )
    .await;

    // Chained renames, ending back at the name the repository started with: the
    // rename is a re-keying, so round-tripping it restores the original state up to
    // the last-modified instant.
    let mut current = "chain-0".to_string();
    for next in ["chain-1", "chain-2", "chain-0"] {
        std::thread::sleep(Duration::from_millis(5));
        ok(
            &router,
            "UpdateRepositoryName",
            &json!({ "oldName": current, "newName": next }),
        )
        .await;
        let after = metadata_of(&router, next).await;
        assert_eq!(
            string_at(&after, "/repositoryId"),
            string_at(&original, "/repositoryId"),
            "step {next:?}: a chain of renames keeps one repository id"
        );
        assert_eq!(
            string_at(&after, "/Arn"),
            format!(
                "arn:aws:codecommit:{REGION}:{}:{next}",
                engine::emulator_account_id()
            ),
            "step {next:?}: Arn"
        );
        // The name just left is free again: nothing in the account holds it.
        let code = error(
            &router,
            "GetRepository",
            &json!({ "repositoryName": current }),
            400,
        )
        .await;
        assert_eq!(
            code, "RepositoryDoesNotExistException",
            "step {next:?}: the name {current:?} the rename left must be free"
        );
        current = next.to_string();
    }

    let round_tripped = metadata_of(&router, "chain-0").await;
    for member in [
        "repositoryId",
        "repositoryName",
        "repositoryDescription",
        "defaultBranch",
        "accountId",
        "Arn",
        "cloneUrlHttp",
        "cloneUrlSsh",
        "creationDate",
    ] {
        assert_eq!(
            round_tripped.get(member),
            original.get(member),
            "member {member}: a round-tripped rename restores the repository, got {round_tripped}"
        );
    }
    assert!(
        as_f64(&round_tripped, "lastModifiedDate") > as_f64(&original, "lastModifiedDate"),
        "a round-tripped rename is still a modification, got {round_tripped}"
    );
    assert_eq!(
        ok(
            &router,
            "GetFile",
            &json!({ "repositoryName": "chain-0", "filePath": "f.txt" })
        )
        .await,
        original_file,
        "a round-tripped rename keeps the repository's content"
    );

    // A name a rename freed can be taken by a new repository, which is a different
    // resource: same name, its own id.
    let created = ok(
        &router,
        "CreateRepository",
        &json!({ "repositoryName": "chain-2" }),
    )
    .await;
    let fresh = created
        .get("repositoryMetadata")
        .unwrap_or_else(|| panic!("no repositoryMetadata in {created}"))
        .clone();
    assert_ne!(
        string_at(&fresh, "/repositoryId"),
        string_at(&original, "/repositoryId"),
        "a repository created under a freed name is a new resource"
    );
    assert_eq!(
        listed_names(&router).await,
        vec!["chain-0".to_string(), "chain-2".to_string()],
        "the account holds the round-tripped repository and the new one"
    );
}
