// SPDX-License-Identifier: CC0-1.0

//! What CodeCommit SAYS when a git object or a commit specifier resolves to
//! nothing, over the wire.
//!
//! One rule, three codes: the text of a not-found refusal is decided by WHAT the
//! service was asked to resolve, and it carries the value the request supplied for
//! that thing and nothing else of the request.
//!
//! * A value that does not name a git object at all is refused as
//!   `InvalidBlobIdException` naming the rejected id — `ObjectId` is unconstrained
//!   in the model (`smithy_query(query="shape",shape="ObjectId")`), so the width
//!   and alphabet of a git object name (40 hexadecimal characters) are checked by
//!   the operation, and one request may carry two blob ids, so the id is the only
//!   thing that tells a caller which side was refused.
//! * A member the model types `CommitName` is a commit SPECIFIER — a branch, a
//!   tag, `HEAD`, or a full commit id — so one that resolves to nothing is refused
//!   as `CommitDoesNotExistException` naming the specifier, plus the sentence that
//!   says the reference may exist and simply not be pushed yet.
//! * A member the model types `CommitId` is a raw object name looked up rather
//!   than resolved, so the same code is worded as the git object lookup's own
//!   miss, naming that id.
//! * `ParentCommitDoesNotExistException` names NOTHING: the parent commit id a
//!   write was rejected for, and the repository it was looked for in, stay out of
//!   the text. `BlobIdDoesNotExistException` — a well-formed object name that no
//!   object carries — likewise names nothing.
//! * A reference `CreateBranch` cannot write because the name is taken is refused
//!   naming that reference in its fully qualified form, and what the name
//!   collides with.
//!
//! Each table varies what the rule generalises over rather than one request: the
//! form of the rejected blob id (too short, too long, right width but not
//! hexadecimal, not hexadecimal at all, a reference name), which blob member
//! carries it, the form of the commit specifier (a branch name, a fully qualified
//! reference, a full object id, `HEAD`, absent), the operation that resolves it,
//! and, for the parent commit, which write path raises the code and whether the
//! repository holds a branch at all.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::error_with_message;
use crate::repository_lifecycle::{base64, emulator, git_blob_id, ok, string_at};

/// The HTTP status each error asserted here models (`errorKind: client`,
/// `httpError: 400` on every one of these error shapes).
const CLIENT_ERROR_STATUS: u16 = 400;

/// A syntactically valid git object id (40 hexadecimal characters) that no
/// repository here holds, so a request carrying it gets past the form check and
/// reaches the lookup under test.
const ABSENT_OBJECT_ID: &str = "0123456789abcdef0123456789abcdef01234567";

/// `ParentCommitDoesNotExistException`'s whole text. Asserted as a literal rather
/// than read from the crate, so a change to the crate's constant cannot change
/// what this suite expects.
const PARENT_COMMIT_DOES_NOT_EXIST: &str =
    "The parent commit ID is not valid. The specified parent commit ID does not exist.";

/// `BlobIdDoesNotExistException`'s text on the single-object READ: a well-formed
/// object name that no object carries names which blob could not be found — the
/// counterpart of the form check, which names the value it rejected as malformed.
fn blob_id_does_not_exist(blob_id: &str) -> String {
    format!("AWS CodeCommit could not find the blob {blob_id} in the repository.")
}

/// `BlobIdDoesNotExistException`'s text on the COMPARISON path, where CodeCommit
/// states the condition alone and interpolates no value — spelled out here rather
/// than read from the crate, for the reason the constants above are.
const BLOB_NOT_IN_REPOSITORY: &str = "The specified blob does not exist in the repository.";

/// `InvalidBlobIdException`'s text for a value that does not name a git object.
fn invalid_blob_id(blob_id: &str) -> String {
    format!("{blob_id} is not a valid git object ID")
}

/// `CommitDoesNotExistException`'s text for a commit SPECIFIER that resolved to no
/// commit.
fn commit_specifier_not_found(specifier: &str) -> String {
    format!(
        "AWS CodeCommit could not find the commit specifier {specifier}. Either the specifier \
         does not exist, or the commit has not yet been pushed to the remote repository."
    )
}

/// `CommitDoesNotExistException`'s text for a raw commit object name the
/// repository does not hold.
fn commit_object_missing(commit_id: &str) -> String {
    format!("Object missing: {commit_id}")
}

/// A repository with one commit on `branch` holding `path`, answering that
/// commit's id. The first write of an empty repository is what creates the
/// branch, so it carries no `parentCommitId`.
async fn seeded(
    router: &MultiServiceRouter,
    repository: &str,
    branch: &str,
    path: &str,
    content: &[u8],
) -> String {
    ok(
        router,
        "CreateRepository",
        &json!({ "repositoryName": repository }),
    )
    .await;
    let written = ok(
        router,
        "PutFile",
        &json!({
            "repositoryName": repository,
            "branchName": branch,
            "filePath": path,
            "fileContent": base64(content),
        }),
    )
    .await;
    string_at(&written, "/commitId")
}

/// The values that do not name a git object, spanning the ways a value can fail
/// the object-name rule: the width (40) and the alphabet (hexadecimal) of a git
/// object name.
fn rejected_blob_ids() -> Vec<(&'static str, String)> {
    vec![
        (
            "not hexadecimal and not the right width",
            "not-a-valid-blob-id".to_string(),
        ),
        (
            "one character short of an object name",
            "a".repeat(39),
        ),
        ("one character past an object name", "a".repeat(41)),
        (
            "the right width but not hexadecimal",
            "z".repeat(40),
        ),
        (
            "the right width with one non-hexadecimal character",
            format!("{}-", &ABSENT_OBJECT_ID[..39]),
        ),
        ("a reference name rather than an object name", "HEAD".to_string()),
        (
            "a fully qualified reference rather than an object name",
            "refs/heads/main".to_string(),
        ),
        ("a single hexadecimal character", "a".to_string()),
    ]
}

/// `InvalidBlobIdException` names the id it rejected, whichever operation and
/// whichever blob member carried it — and, where the request carries two, it names
/// the one at fault and not the other.
#[tokio::test]
async fn invalid_blob_id_message_names_the_rejected_id() {
    let router = emulator();
    let repository = "blob-id-wording";
    let content = b"# demo\n".to_vec();
    seeded(&router, repository, "main", "README.md", &content).await;
    let held = git_blob_id(&content);

    for (label, rejected) in rejected_blob_ids() {
        // Each row is asked of every member that carries a blob id: `GetBlob`'s
        // `blobId`, and both sides of `GetBlobDifferences`.
        let requests: Vec<(&str, Value)> = vec![
            (
                "GetBlob",
                json!({ "repositoryName": repository, "blobId": rejected }),
            ),
            (
                "GetBlobDifferences",
                json!({ "repositoryName": repository, "afterBlobId": rejected }),
            ),
            (
                "GetBlobDifferences",
                json!({
                    "repositoryName": repository,
                    "beforeBlobId": rejected,
                    "afterBlobId": held,
                }),
            ),
            (
                "GetBlobDifferences",
                json!({
                    "repositoryName": repository,
                    "beforeBlobId": held,
                    "afterBlobId": rejected,
                }),
            ),
        ];
        for (operation, body) in requests {
            let (code, message) =
                error_with_message(&router, operation, &body, CLIENT_ERROR_STATUS).await;
            assert_eq!(
                code, "InvalidBlobIdException",
                "case ({operation}, {label}, {rejected:?}): expected code \
                 InvalidBlobIdException, got {code:?}"
            );
            let expected = invalid_blob_id(&rejected);
            assert_eq!(
                message, expected,
                "case ({operation}, {label}, {rejected:?}): expected message {expected:?}, got \
                 {message:?}"
            );
            // The id the request got RIGHT stays out of the text: the message's job
            // is to say which side was refused.
            assert!(
                !message.contains(&held),
                "case ({operation}, {label}, {rejected:?}): the message must not name the \
                 accepted blob id {held:?}, got {message:?}"
            );
        }
    }
}

/// A blob id that IS an object name but names no object this repository holds is a
/// different refusal: the lookup's own code, worded as the OPERATION words it. The
/// single-object read names the object the request asked for; the comparison states only
/// the condition, interpolating nothing. Neither names the repository. Pinned beside the
/// form check so the two cannot collapse into one.
#[tokio::test]
async fn an_object_name_the_repository_does_not_hold_is_a_different_refusal() {
    let router = emulator();
    let repository = "blob-id-lookup";
    let content = b"# demo\n".to_vec();
    seeded(&router, repository, "main", "README.md", &content).await;
    let held = git_blob_id(&content);

    // The rows span the FORM a well-formed object name no object carries can take: a
    // content-addressed id of content never written, each end of the object-name range, a
    // fixed id, and the same id in upper case (an object name is hexadecimal, so its case
    // does not change which object it names).
    let absent: Vec<(&str, String)> = vec![
        ("a content id never written", git_blob_id(b"never written\n")),
        ("the lowest object name", "0".repeat(40)),
        ("the highest object name", "f".repeat(40)),
        ("a fixed full object id", ABSENT_OBJECT_ID.to_string()),
        (
            "the same id in upper case",
            ABSENT_OBJECT_ID.to_ascii_uppercase(),
        ),
    ];
    for (label, missing) in absent {
        // Each row is asked of every member that carries a blob id: `GetBlob`'s `blobId`,
        // and both sides of `GetBlobDifferences` — each beside the sentence that operation
        // words this code with.
        let requests: Vec<(&str, Value, String)> = vec![
            (
                "GetBlob",
                json!({ "repositoryName": repository, "blobId": missing }),
                blob_id_does_not_exist(&missing),
            ),
            (
                "GetBlobDifferences",
                json!({ "repositoryName": repository, "afterBlobId": missing }),
                BLOB_NOT_IN_REPOSITORY.to_string(),
            ),
            (
                "GetBlobDifferences",
                json!({
                    "repositoryName": repository,
                    "beforeBlobId": missing,
                    "afterBlobId": held,
                }),
                BLOB_NOT_IN_REPOSITORY.to_string(),
            ),
        ];
        for (operation, body, expected) in requests {
            let (code, message) =
                error_with_message(&router, operation, &body, CLIENT_ERROR_STATUS).await;
            assert_eq!(
                code, "BlobIdDoesNotExistException",
                "case ({operation}, {label}): expected code BlobIdDoesNotExistException, got \
                 {code:?}"
            );
            assert_eq!(
                message, expected,
                "case ({operation}, {label}): expected message {expected:?}, got {message:?}"
            );
            assert!(
                !message.contains(repository),
                "case ({operation}, {label}): the message must not name the repository, got \
                 {message:?}"
            );
        }
    }
}

/// The specifier forms `CommitName` documents — a branch name, a fully qualified
/// reference, a full commit id — each paired with the specifier the refusal must
/// name.
fn rejected_specifiers() -> Vec<(&'static str, &'static str)> {
    vec![
        ("a branch name", "ghost-branch"),
        ("a fully qualified reference", "refs/heads/ghost-branch"),
        ("a full object id", ABSENT_OBJECT_ID),
        ("a tag-shaped reference", "refs/tags/v9"),
    ]
}

/// `CommitDoesNotExistException` raised on a member the model types `CommitName`
/// names the SPECIFIER, and says the reference may simply not be pushed yet —
/// whichever operation resolved it and whatever form the specifier took.
#[tokio::test]
async fn a_commit_specifier_that_resolves_to_nothing_names_the_specifier() {
    let router = emulator();
    let repository = "specifier-wording";
    let branch = "main";
    seeded(&router, repository, branch, "README.md", b"# demo\n").await;

    for (label, specifier) in rejected_specifiers() {
        let requests: Vec<(&str, Value)> = vec![
            (
                "GetFile",
                json!({
                    "repositoryName": repository,
                    "filePath": "README.md",
                    "commitSpecifier": specifier,
                }),
            ),
            (
                "GetFolder",
                json!({
                    "repositoryName": repository,
                    "folderPath": "/",
                    "commitSpecifier": specifier,
                }),
            ),
            (
                "ListFileCommitHistory",
                json!({
                    "repositoryName": repository,
                    "filePath": "README.md",
                    "commitSpecifier": specifier,
                }),
            ),
            (
                "GetDifferences",
                json!({
                    "repositoryName": repository,
                    "afterCommitSpecifier": specifier,
                }),
            ),
            (
                "GetMergeOptions",
                json!({
                    "repositoryName": repository,
                    "sourceCommitSpecifier": specifier,
                    "destinationCommitSpecifier": branch,
                }),
            ),
            (
                "GetMergeOptions",
                json!({
                    "repositoryName": repository,
                    "sourceCommitSpecifier": branch,
                    "destinationCommitSpecifier": specifier,
                }),
            ),
        ];
        for (operation, body) in requests {
            let (code, message) =
                error_with_message(&router, operation, &body, CLIENT_ERROR_STATUS).await;
            assert_eq!(
                code, "CommitDoesNotExistException",
                "case ({operation}, {label}, {specifier:?}): expected code \
                 CommitDoesNotExistException, got {code:?}"
            );
            let expected = commit_specifier_not_found(specifier);
            assert_eq!(
                message, expected,
                "case ({operation}, {label}, {specifier:?}): expected message {expected:?}, got \
                 {message:?}"
            );
            assert!(
                !message.contains(repository),
                "case ({operation}, {label}, {specifier:?}): the message must not name the \
                 repository, got {message:?}"
            );
        }
    }
}

/// A request that names NO specifier resolves `HEAD`, so a repository with no
/// default branch to follow is refused naming `HEAD` rather than an empty value.
#[tokio::test]
async fn an_absent_commit_specifier_is_refused_naming_head() {
    let router = emulator();
    let repository = "head-default-wording";
    ok(
        &router,
        "CreateRepository",
        &json!({ "repositoryName": repository }),
    )
    .await;

    let requests: Vec<(&str, Value)> = vec![
        (
            "GetFile",
            json!({ "repositoryName": repository, "filePath": "README.md" }),
        ),
        (
            "GetFolder",
            json!({ "repositoryName": repository, "folderPath": "/" }),
        ),
        (
            "ListFileCommitHistory",
            json!({ "repositoryName": repository, "filePath": "README.md" }),
        ),
    ];
    let expected = commit_specifier_not_found("HEAD");
    for (operation, body) in requests {
        let (code, message) =
            error_with_message(&router, operation, &body, CLIENT_ERROR_STATUS).await;
        assert_eq!(
            code, "CommitDoesNotExistException",
            "case {operation}: expected code CommitDoesNotExistException, got {code:?}"
        );
        assert_eq!(
            message, expected,
            "case {operation}: expected message {expected:?}, got {message:?}"
        );
    }
}

/// `CommitDoesNotExistException` raised on a member the model types `CommitId` is
/// the git object lookup's own miss: the fixed prefix plus the id that named no
/// object, whichever operation looked it up.
#[tokio::test]
async fn a_raw_commit_id_the_repository_does_not_hold_is_an_object_miss() {
    let router = emulator();
    let repository = "commit-id-wording";
    let branch = "main";
    let tip = seeded(&router, repository, branch, "README.md", b"# demo\n").await;

    // Object names no commit of this repository carries. Each is a valid object
    // name, so every row gets past the form check and reaches the lookup.
    let absent: Vec<(&str, &str)> = vec![
        ("an all-zero object name", "0000000000000000000000000000000000000000"),
        ("a mixed-digit object name", ABSENT_OBJECT_ID),
        ("an all-f object name", "ffffffffffffffffffffffffffffffffffffffff"),
    ];
    for (label, commit_id) in absent {
        assert_ne!(
            commit_id, tip,
            "case {label}: the row must name a commit the repository does not hold"
        );
        let requests: Vec<(&str, Value)> = vec![
            (
                "CreateBranch",
                json!({
                    "repositoryName": repository,
                    "branchName": format!("from-{commit_id}"),
                    "commitId": commit_id,
                }),
            ),
            (
                "GetCommentsForComparedCommit",
                json!({ "repositoryName": repository, "afterCommitId": commit_id }),
            ),
            (
                "GetCommentsForComparedCommit",
                json!({
                    "repositoryName": repository,
                    "beforeCommitId": commit_id,
                    "afterCommitId": tip,
                }),
            ),
            (
                "PostCommentForComparedCommit",
                json!({
                    "repositoryName": repository,
                    "afterCommitId": commit_id,
                    "content": "a comment",
                }),
            ),
        ];
        for (operation, body) in requests {
            let (code, message) =
                error_with_message(&router, operation, &body, CLIENT_ERROR_STATUS).await;
            assert_eq!(
                code, "CommitDoesNotExistException",
                "case ({operation}, {label}): expected code CommitDoesNotExistException, got \
                 {code:?}"
            );
            let expected = commit_object_missing(commit_id);
            assert_eq!(
                message, expected,
                "case ({operation}, {label}): expected message {expected:?}, got {message:?}"
            );
        }
    }
}

/// `ParentCommitDoesNotExistException` carries the same sentence pair whichever
/// write path raises it, and names neither the parent commit id nor the
/// repository.
#[tokio::test]
async fn parent_commit_does_not_exist_message_names_nothing() {
    let router = emulator();
    let branch = "main";

    // A repository with a tip, so the named parent is refused for naming no commit
    // rather than for the branch not existing; and an EMPTY one, whose write path
    // reaches the same code by the other route (no branch at all, but a parent
    // named).
    let seeded_repository = "parent-wording";
    seeded(
        &router,
        seeded_repository,
        branch,
        "README.md",
        b"# demo\n",
    )
    .await;
    let empty_repository = "parent-wording-empty";
    ok(
        &router,
        "CreateRepository",
        &json!({ "repositoryName": empty_repository }),
    )
    .await;

    let absent: Vec<(&str, &str)> = vec![
        ("an all-zero object name", "0000000000000000000000000000000000000000"),
        ("a mixed-digit object name", ABSENT_OBJECT_ID),
    ];
    for (label, parent) in absent {
        let requests: Vec<(&str, &str, Value)> = vec![
            (
                "PutFile",
                "a branch with a tip",
                json!({
                    "repositoryName": seeded_repository,
                    "branchName": branch,
                    "filePath": "added.txt",
                    "fileContent": base64(b"added"),
                    "parentCommitId": parent,
                }),
            ),
            (
                "CreateCommit",
                "a branch with a tip",
                json!({
                    "repositoryName": seeded_repository,
                    "branchName": branch,
                    "parentCommitId": parent,
                    "putFiles": [
                        { "filePath": "created.txt", "fileContent": base64(b"created") },
                    ],
                }),
            ),
            (
                "DeleteFile",
                "a branch with a tip",
                json!({
                    "repositoryName": seeded_repository,
                    "branchName": branch,
                    "filePath": "README.md",
                    "parentCommitId": parent,
                }),
            ),
            (
                "PutFile",
                "a repository holding no branch",
                json!({
                    "repositoryName": empty_repository,
                    "branchName": branch,
                    "filePath": "added.txt",
                    "fileContent": base64(b"added"),
                    "parentCommitId": parent,
                }),
            ),
            (
                "CreateCommit",
                "a repository holding no branch",
                json!({
                    "repositoryName": empty_repository,
                    "branchName": branch,
                    "parentCommitId": parent,
                    "putFiles": [
                        { "filePath": "created.txt", "fileContent": base64(b"created") },
                    ],
                }),
            ),
        ];
        for (operation, shape, body) in requests {
            let (code, message) =
                error_with_message(&router, operation, &body, CLIENT_ERROR_STATUS).await;
            assert_eq!(
                code, "ParentCommitDoesNotExistException",
                "case ({operation}, {shape}, {label}): expected code \
                 ParentCommitDoesNotExistException, got {code:?}"
            );
            assert_eq!(
                message, PARENT_COMMIT_DOES_NOT_EXIST,
                "case ({operation}, {shape}, {label}): expected message \
                 {PARENT_COMMIT_DOES_NOT_EXIST:?}, got {message:?}"
            );
            assert!(
                !message.contains(parent),
                "case ({operation}, {shape}, {label}): the message must not name the parent \
                 commit id, got {message:?}"
            );
            for named in [seeded_repository, empty_repository] {
                assert!(
                    !message.contains(named),
                    "case ({operation}, {shape}, {label}): the message must not name a \
                     repository, got {message:?}"
                );
            }
        }
    }
}

/// A reference `CreateBranch` cannot write because the name is already a branch of
/// the repository is refused naming that reference in its FULLY QUALIFIED form,
/// whatever form the short name took — and naming neither the repository nor the
/// commit the caller was pointing it at.
#[tokio::test]
async fn a_branch_name_already_taken_names_the_qualified_reference() {
    let router = emulator();
    let repository = "branch-name-taken";
    let tip = seeded(&router, repository, "main", "README.md", b"# demo\n").await;

    // Short names spanning what `BranchName` allows (len 1..256): the seeded
    // branch, a single character, a nested reference, dots and underscores, a
    // non-ASCII name, and the 256-character maximum.
    let names: Vec<(&str, String)> = vec![
        ("the branch the first write created", "main".to_string()),
        ("a single-character name", "b".to_string()),
        (
            "a nested reference name",
            "feature/deeply/nested-ref".to_string(),
        ),
        ("dots and underscores", "release_1.2.3".to_string()),
        ("a non-ASCII name", "rama-española".to_string()),
        ("the longest name", "z".repeat(256)),
    ];
    for (label, name) in names {
        // Every row but the seeded branch has to be created first, so each one is
        // refused for the name being TAKEN rather than for anything else.
        if name != "main" {
            ok(
                &router,
                "CreateBranch",
                &json!({
                    "repositoryName": repository,
                    "branchName": name,
                    "commitId": tip,
                }),
            )
            .await;
        }
        let (code, message) = error_with_message(
            &router,
            "CreateBranch",
            &json!({
                "repositoryName": repository,
                "branchName": name,
                "commitId": tip,
            }),
            CLIENT_ERROR_STATUS,
        )
        .await;
        assert_eq!(
            code, "BranchNameExistsException",
            "case ({label}, {name:?}): expected code BranchNameExistsException, got {code:?}"
        );
        let expected = format!(
            "A branch cannot be created with the following name: \"refs/heads/{name}\". The \
             commit conflicts with an existing branch with the same name. Branch names must be \
             unique."
        );
        assert_eq!(
            message, expected,
            "case ({label}, {name:?}): expected message {expected:?}, got {message:?}"
        );
        assert!(
            !message.contains(repository),
            "case ({label}, {name:?}): the message must not name the repository, got {message:?}"
        );
        assert!(
            !message.contains(&tip),
            "case ({label}, {name:?}): the message must not name the commit id, got {message:?}"
        );
    }
}
