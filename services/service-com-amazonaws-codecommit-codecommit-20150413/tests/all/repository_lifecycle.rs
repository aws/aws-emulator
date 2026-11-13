// SPDX-License-Identifier: CC0-1.0

//! The repository lifecycle over the wire: `CreateRepository`, `PutFile`, `GetFile`,
//! `GetBranch`, `GetRepository`, `ListRepositories` and `DeleteRepository`, driven as
//! awsJson1_1 requests through the router that serves the real emulator.
//!
//! The rule under test is that every response is computed from the service's stored
//! state: what a write persists is what a later read returns (metadata, bytes, mode,
//! size, branch tip), the git object ids are the object names of what was written, and
//! a read never changes what the next caller sees. The tables below span the class the
//! rule covers — name lengths, descriptions, path depths, content kinds and sizes, file
//! modes, sort keys and directions — rather than one recorded request.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use sha1::Digest;
use smithy_server_runtime::MultiServiceRouter;

/// The region the test's requests are signed for; every ARN the service reports
/// must be built for it.
///
/// This module's harness (`emulator`, `call`, `ok`, `error` and the small value
/// helpers) is shared with the sibling suites in `tests/all/`, so every suite
/// drives the same emulator over the same wire protocol. A suite that needs to
/// call as a NAMED principal uses `crate::wire`, which signs with a given access
/// key id over this same protocol.
pub(crate) const REGION: &str = "us-west-2";

/// The authority the test's requests are addressed to — the endpoint the
/// emulator is reached on, which is what every clone URL it announces must name.
pub(crate) const ENDPOINT_AUTHORITY: &str = "codecommit.us-west-2.amazonaws.com";

/// Build the emulator the way the router does: one shared engine, this service
/// mounted on it, request authentication off so the test signs nothing. The
/// engine is returned too, so a test can read the store the handlers write.
///
/// The account holds the default caller's long-term access key for an IAM user of
/// its own ([`crate::wire::hold_long_term_key`]), which is the position a long-term
/// key is in on AWS: it belongs to a user, and that user is the principal every ARN
/// this service records for the request names.
///
/// `pub(crate)` here and below: this module is the wire harness every suite in
/// this test target shares, so a sibling suite drives the emulator exactly the
/// same way rather than building a second copy of it.
pub(crate) fn emulator_with_engine() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_codecommit_codecommit_20150413::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    crate::wire::hold_long_term_key(&engine, crate::wire::DEFAULT_ACCESS_KEY_ID);
    (router, engine)
}

pub(crate) fn emulator() -> MultiServiceRouter {
    emulator_with_engine().0
}

/// Send one awsJson1_1 request: `POST /` with `X-Amz-Target:
/// CodeCommit_20150413.<Operation>` (the crate's OPERATIONS.md "Wire protocol"),
/// signed with the default access key id.
pub(crate) async fn call(
    router: &MultiServiceRouter,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    crate::wire::call_as(router, crate::wire::DEFAULT_ACCESS_KEY_ID, operation, body).await
}

/// A successful call's payload, with the status asserted.
pub(crate) async fn ok(router: &MultiServiceRouter, operation: &str, body: &Value) -> Value {
    crate::wire::ok_as(router, crate::wire::DEFAULT_ACCESS_KEY_ID, operation, body).await
}

/// The modeled error code of a failed call, with the status asserted. awsJson
/// carries it in `__type`, optionally namespace-qualified.
pub(crate) async fn error(
    router: &MultiServiceRouter,
    operation: &str,
    body: &Value,
    expected_status: u16,
) -> String {
    crate::wire::error_as(
        router,
        crate::wire::DEFAULT_ACCESS_KEY_ID,
        operation,
        body,
        expected_status,
    )
    .await
}

/// The base64 form the wire carries file content in.
pub(crate) fn base64(content: &[u8]) -> String {
    aws_smithy_types::base64::encode(content)
}

pub(crate) fn from_base64(encoded: &str) -> Vec<u8> {
    aws_smithy_types::base64::decode(encoded).expect("the service returns base64 blobs")
}

pub(crate) fn string_at(payload: &Value, pointer: &str) -> String {
    payload
        .pointer(pointer)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("expected a string at {pointer} in {payload}"))
        .to_string()
}

/// The object name of a blob — git's own object name function over the object
/// encoding `"blob <len>\0<content>"` — computed here independently of the
/// service, so `blobId` is checked against the rule rather than against itself.
pub(crate) fn git_blob_id(content: &[u8]) -> String {
    let mut hasher = sha1::Sha1::new();
    hasher.update(format!("blob {}\0", content.len()).as_bytes());
    hasher.update(content);
    hex::encode(hasher.finalize())
}

pub(crate) fn is_object_id(value: &str) -> bool {
    value.len() == 40 && value.bytes().all(|b| b.is_ascii_hexdigit())
}

/// A UUID in its canonical textual form: 8-4-4-4-12 hex characters.
pub(crate) fn is_uuid(value: &str) -> bool {
    let groups: Vec<&str> = value.split('-').collect();
    groups.len() == 5
        && [8usize, 4, 4, 4, 12]
            .iter()
            .zip(&groups)
            .all(|(len, group)| group.len() == *len)
        && groups
            .iter()
            .all(|group| group.bytes().all(|b| b.is_ascii_hexdigit()))
}

/// The textual form CodeCommit mints a `commentId` as: a pair of UUIDs joined by
/// a colon, so a single UUID is only HALF a comment id.
pub(crate) fn is_comment_id(value: &str) -> bool {
    value
        .split_once(':')
        .is_some_and(|(first, second)| is_uuid(first) && is_uuid(second))
}

/// One row of the lifecycle table: a repository and the single file written into it.
#[derive(Debug, Clone)]
struct Case {
    name: String,
    description: Option<String>,
    branch: String,
    /// The `filePath` as sent.
    sent_path: String,
    /// The path the service must address the file by (leading separators removed).
    stored_path: String,
    content: Vec<u8>,
    /// The `fileMode` as sent; `None` exercises the NORMAL default.
    sent_mode: Option<String>,
    stored_mode: String,
}

/// The table: rows differ along what the rule generalises over — name length
/// (1, ordinary, the modeled maximum of 100), description (absent, empty, text),
/// branch naming, path depth and leading separator, content kind and size (text,
/// every byte value, unicode, 4 KiB) and `fileMode` (defaulted, EXECUTABLE,
/// SYMLINK). `FileModeTypeEnum` values are from
/// `smithy_query(query="shape",shape="FileModeTypeEnum")`; the name bounds are
/// `RepositoryName`'s modeled `length` 1..100.
fn lifecycle_cases() -> Vec<Case> {
    vec![
        Case {
            name: "demo-repo".to_string(),
            description: Some("bootstrap demo".to_string()),
            branch: "main".to_string(),
            sent_path: "README.md".to_string(),
            stored_path: "README.md".to_string(),
            content: b"# demo\n".to_vec(),
            sent_mode: None,
            stored_mode: "NORMAL".to_string(),
        },
        Case {
            name: "nested.repo_1".to_string(),
            description: None,
            branch: "release-1.0".to_string(),
            // A leading separator addresses the same file as none at all.
            sent_path: "/src/main/app.sh".to_string(),
            stored_path: "src/main/app.sh".to_string(),
            content: (0..=255u8).cycle().take(4096).collect(),
            sent_mode: Some("EXECUTABLE".to_string()),
            stored_mode: "EXECUTABLE".to_string(),
        },
        Case {
            name: "a".to_string(),
            description: Some(String::new()),
            branch: "feature/ünïcode".to_string(),
            sent_path: "docs/guía.md".to_string(),
            stored_path: "docs/guía.md".to_string(),
            content: "⚙ configuración\n".as_bytes().to_vec(),
            sent_mode: Some("SYMLINK".to_string()),
            stored_mode: "SYMLINK".to_string(),
        },
        Case {
            // The modeled maximum name length.
            name: "x".repeat(100),
            description: Some("y".repeat(1000)),
            branch: "b".to_string(),
            sent_path: "a/b/c/d/e/f/g/h/i/j/file.txt".to_string(),
            stored_path: "a/b/c/d/e/f/g/h/i/j/file.txt".to_string(),
            content: b"deep".to_vec(),
            sent_mode: Some("NORMAL".to_string()),
            stored_mode: "NORMAL".to_string(),
        },
    ]
}

#[tokio::test]
async fn a_written_repository_and_file_are_read_back_from_state() {
    let router = emulator();
    let mut ids: BTreeSet<String> = BTreeSet::new();

    for case in lifecycle_cases() {
        let label = case.name.clone();

        // CreateRepository persists the repository and reports its generated fields.
        let mut create_input = json!({ "repositoryName": case.name });
        if let Some(description) = &case.description {
            create_input["repositoryDescription"] = json!(description);
        }
        let created = ok(&router, "CreateRepository", &create_input).await;
        let metadata = created
            .get("repositoryMetadata")
            .unwrap_or_else(|| panic!("case {label:?}: no repositoryMetadata in {created}"))
            .clone();

        let repository_id = string_at(&metadata, "/repositoryId");
        assert!(
            is_uuid(&repository_id),
            "case {label:?}: repositoryId must be a uuid, got {repository_id:?}"
        );
        assert!(
            ids.insert(repository_id.clone()),
            "case {label:?}: repositoryId {repository_id} was already issued to another repository"
        );
        assert_eq!(
            string_at(&metadata, "/repositoryName"),
            case.name,
            "case {label:?}: repositoryName"
        );
        assert_eq!(
            metadata.get("repositoryDescription").and_then(Value::as_str),
            case.description.as_deref(),
            "case {label:?}: repositoryDescription must be what was given, got {metadata}"
        );
        assert_eq!(
            string_at(&metadata, "/Arn"),
            format!(
                "arn:aws:codecommit:{REGION}:{}:{}",
                engine::emulator_account_id(),
                case.name
            ),
            "case {label:?}: Arn"
        );
        assert_eq!(
            string_at(&metadata, "/accountId"),
            engine::emulator_account_id(),
            "case {label:?}: accountId"
        );
        // The clone URL names an address of THIS emulator — the one its git transport
        // answers on, which a stock git client dials in the clear — not the public AWS
        // git host, which is a different server. This repository was created through
        // `CreateRepository`, so that address is open: the URL is the git listener's
        // own loopback authority with this repository on its path.
        let clone_url_http = string_at(&metadata, "/cloneUrlHttp");
        let (origin, path) = clone_url_http
            .split_once("/v1/repos/")
            .unwrap_or_else(|| panic!("case {label:?}: cloneUrlHttp {clone_url_http:?}"));
        assert_eq!(
            path, case.name,
            "case {label:?}: cloneUrlHttp must carry the repository on its path, got \
             {clone_url_http:?}"
        );
        let port = origin.strip_prefix("http://127.0.0.1:").unwrap_or_else(|| {
            panic!(
                "case {label:?}: cloneUrlHttp must name this emulator's own git address in \
                 the clear, got {clone_url_http:?}"
            )
        });
        assert!(
            port.parse::<u16>().is_ok_and(|port| port > 0),
            "case {label:?}: cloneUrlHttp must name the port the git listener bound, got \
             {clone_url_http:?}"
        );
        assert_eq!(
            string_at(&metadata, "/cloneUrlSsh"),
            format!("ssh://{ENDPOINT_AUTHORITY}/v1/repos/{}", case.name),
            "case {label:?}: cloneUrlSsh"
        );
        assert_eq!(
            metadata.get("defaultBranch"),
            None,
            "case {label:?}: a repository with no commit has no defaultBranch, got {metadata}"
        );
        let creation_date = metadata
            .get("creationDate")
            .and_then(Value::as_f64)
            .unwrap_or_else(|| panic!("case {label:?}: no creationDate in {metadata}"));
        assert_eq!(
            metadata.get("lastModifiedDate").and_then(Value::as_f64),
            Some(creation_date),
            "case {label:?}: a fresh repository was last modified when it was created"
        );

        // A second create of the same name is refused.
        let code = error(&router, "CreateRepository", &create_input, 400).await;
        assert_eq!(
            code, "RepositoryNameExistsException",
            "case {label:?}: a duplicate repository name must be refused"
        );

        // PutFile writes the file, creating the branch of an empty repository.
        let put = ok(
            &router,
            "PutFile",
            &{
                let mut input = json!({
                    "repositoryName": case.name,
                    "branchName": case.branch,
                    "filePath": case.sent_path,
                    "fileContent": base64(&case.content),
                    "commitMessage": "init",
                    "name": "Test Author",
                    "email": "author@example.com",
                });
                if let Some(mode) = &case.sent_mode {
                    input["fileMode"] = json!(mode);
                }
                input
            },
        )
        .await;
        let commit_id = string_at(&put, "/commitId");
        let blob_id = string_at(&put, "/blobId");
        let tree_id = string_at(&put, "/treeId");
        for (field, value) in [
            ("commitId", &commit_id),
            ("blobId", &blob_id),
            ("treeId", &tree_id),
        ] {
            assert!(
                is_object_id(value),
                "case {label:?}: {field} must be a 40-character hex object id, got {value:?}"
            );
        }
        assert_eq!(
            blob_id,
            git_blob_id(&case.content),
            "case {label:?}: blobId must be the object name of the content written"
        );

        // GetFile returns exactly what PutFile stored.
        let get_input = json!({ "repositoryName": case.name, "filePath": case.sent_path });
        let got = ok(&router, "GetFile", &get_input).await;
        assert_eq!(
            from_base64(&string_at(&got, "/fileContent")),
            case.content,
            "case {label:?}: fileContent must be byte-identical to what was put"
        );
        assert_eq!(
            string_at(&got, "/blobId"),
            blob_id,
            "case {label:?}: blobId from GetFile"
        );
        assert_eq!(
            string_at(&got, "/commitId"),
            commit_id,
            "case {label:?}: commitId from GetFile"
        );
        assert_eq!(
            string_at(&got, "/filePath"),
            case.stored_path,
            "case {label:?}: filePath"
        );
        assert_eq!(
            string_at(&got, "/fileMode"),
            case.stored_mode,
            "case {label:?}: fileMode"
        );
        assert_eq!(
            got.get("fileSize").and_then(Value::as_i64),
            Some(case.content.len() as i64),
            "case {label:?}: fileSize, got {got}"
        );
        // A read does not change what the next caller sees.
        let again = ok(&router, "GetFile", &get_input).await;
        assert_eq!(again, got, "case {label:?}: GetFile must be repeatable");

        // GetBranch reports the tip PutFile wrote.
        let branch = ok(
            &router,
            "GetBranch",
            &json!({ "repositoryName": case.name, "branchName": case.branch }),
        )
        .await;
        assert_eq!(
            string_at(&branch, "/branch/commitId"),
            commit_id,
            "case {label:?}: branch tip"
        );
        assert_eq!(
            string_at(&branch, "/branch/branchName"),
            case.branch,
            "case {label:?}: branchName"
        );

        // GetRepository reports the same stored repository, now with a default branch.
        let fetched = ok(
            &router,
            "GetRepository",
            &json!({ "repositoryName": case.name }),
        )
        .await;
        let fetched_metadata = fetched
            .get("repositoryMetadata")
            .unwrap_or_else(|| panic!("case {label:?}: no repositoryMetadata in {fetched}"))
            .clone();
        assert_eq!(
            string_at(&fetched_metadata, "/repositoryId"),
            repository_id,
            "case {label:?}: repositoryId must not change"
        );
        assert_eq!(
            string_at(&fetched_metadata, "/Arn"),
            string_at(&metadata, "/Arn"),
            "case {label:?}: Arn must not change"
        );
        assert_eq!(
            fetched_metadata.get("creationDate").and_then(Value::as_f64),
            Some(creation_date),
            "case {label:?}: creationDate must not change"
        );
        assert_eq!(
            fetched_metadata.get("defaultBranch").and_then(Value::as_str),
            Some(case.branch.as_str()),
            "case {label:?}: the first branch written becomes the default branch"
        );
        let modified = fetched_metadata
            .get("lastModifiedDate")
            .and_then(Value::as_f64)
            .unwrap_or_else(|| panic!("case {label:?}: no lastModifiedDate"));
        assert!(
            modified >= creation_date,
            "case {label:?}: lastModifiedDate {modified} must be at or after creationDate {creation_date}"
        );
    }

    // ListRepositories reports every live repository with its stored id.
    let listed = ok(&router, "ListRepositories", &json!({})).await;
    let pairs = listed
        .get("repositories")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("no repositories in {listed}"))
        .clone();
    assert_eq!(
        pairs.len(),
        lifecycle_cases().len(),
        "every created repository must be listed, got {listed}"
    );
    let listed_ids: BTreeSet<String> = pairs
        .iter()
        .map(|pair| string_at(pair, "/repositoryId"))
        .collect();
    assert_eq!(
        listed_ids, ids,
        "the listed repository ids must be the ids CreateRepository issued"
    );
    assert_eq!(
        listed.get("nextToken"),
        None,
        "a single page of repositories carries no nextToken, got {listed}"
    );

    // DeleteRepository removes the repository and everything under it.
    for case in lifecycle_cases() {
        let label = case.name.clone();
        let deleted = ok(
            &router,
            "DeleteRepository",
            &json!({ "repositoryName": case.name }),
        )
        .await;
        let deleted_id = string_at(&deleted, "/repositoryId");
        assert!(
            ids.contains(&deleted_id),
            "case {label:?}: DeleteRepository must return the stored repositoryId, got {deleted_id}"
        );
        let code = error(
            &router,
            "GetRepository",
            &json!({ "repositoryName": case.name }),
            400,
        )
        .await;
        assert_eq!(
            code, "RepositoryDoesNotExistException",
            "case {label:?}: a deleted repository must not be readable"
        );
        let code = error(
            &router,
            "GetFile",
            &json!({ "repositoryName": case.name, "filePath": case.sent_path }),
            400,
        )
        .await;
        assert_eq!(
            code, "RepositoryDoesNotExistException",
            "case {label:?}: a deleted repository's files must be gone"
        );
    }

    let emptied = ok(&router, "ListRepositories", &json!({})).await;
    assert_eq!(
        emptied.get("repositories").and_then(Value::as_array),
        Some(&vec![]),
        "no repository may be listed after all were deleted, got {emptied}"
    );

    // Deleting what is not there is idempotent: no error, and no id.
    let absent = ok(
        &router,
        "DeleteRepository",
        &json!({ "repositoryName": "never-existed" }),
    )
    .await;
    assert_eq!(
        absent.get("repositoryId"),
        None,
        "deleting a repository that does not exist returns no repositoryId, got {absent}"
    );
}

#[tokio::test]
async fn a_commit_specifier_reads_the_tree_of_that_commit() {
    let router = emulator();
    ok(
        &router,
        "CreateRepository",
        &json!({ "repositoryName": "history" }),
    )
    .await;

    // Three commits: one file, then that file changed, then a second file added.
    let first = ok(
        &router,
        "PutFile",
        &json!({
            "repositoryName": "history",
            "branchName": "main",
            "filePath": "f.txt",
            "fileContent": base64(b"one"),
        }),
    )
    .await;
    let commit_one = string_at(&first, "/commitId");
    let second = ok(
        &router,
        "PutFile",
        &json!({
            "repositoryName": "history",
            "branchName": "main",
            "filePath": "f.txt",
            "fileContent": base64(b"two"),
            "parentCommitId": commit_one,
        }),
    )
    .await;
    let commit_two = string_at(&second, "/commitId");
    let third = ok(
        &router,
        "PutFile",
        &json!({
            "repositoryName": "history",
            "branchName": "main",
            "filePath": "dir/g.txt",
            "fileContent": base64(b"gee"),
            "parentCommitId": commit_two,
        }),
    )
    .await;
    let commit_three = string_at(&third, "/commitId");

    assert!(
        commit_one != commit_two && commit_two != commit_three,
        "each commit must have its own id: {commit_one}, {commit_two}, {commit_three}"
    );
    assert_ne!(
        string_at(&second, "/treeId"),
        string_at(&third, "/treeId"),
        "adding a file changes the tree"
    );

    // Each specifier form resolves to the commit whose snapshot it names.
    // (commitSpecifier: absent -> the default branch, a branch name, a commit id.)
    let reads: Vec<(Option<&str>, &str, &[u8])> = vec![
        (None, commit_three.as_str(), b"two"),
        (Some("main"), commit_three.as_str(), b"two"),
        (Some(commit_three.as_str()), commit_three.as_str(), b"two"),
        (Some(commit_two.as_str()), commit_two.as_str(), b"two"),
        (Some(commit_one.as_str()), commit_one.as_str(), b"one"),
    ];
    for (specifier, expected_commit, expected_content) in reads {
        let mut input = json!({ "repositoryName": "history", "filePath": "f.txt" });
        if let Some(specifier) = specifier {
            input["commitSpecifier"] = json!(specifier);
        }
        let got = ok(&router, "GetFile", &input).await;
        assert_eq!(
            string_at(&got, "/commitId"),
            expected_commit,
            "case {specifier:?}: commitId"
        );
        assert_eq!(
            from_base64(&string_at(&got, "/fileContent")),
            expected_content,
            "case {specifier:?}: fileContent"
        );
        assert_eq!(
            string_at(&got, "/blobId"),
            git_blob_id(expected_content),
            "case {specifier:?}: blobId must be the object name of the content at that commit"
        );
    }

    // A file added later is absent from an earlier commit's snapshot.
    let code = error(
        &router,
        "GetFile",
        &json!({
            "repositoryName": "history",
            "filePath": "dir/g.txt",
            "commitSpecifier": commit_one,
        }),
        400,
    )
    .await;
    assert_eq!(
        code, "FileDoesNotExistException",
        "a file is absent from a commit written before it"
    );

    // The branch tip tracks the newest commit.
    let branch = ok(
        &router,
        "GetBranch",
        &json!({ "repositoryName": "history", "branchName": "main" }),
    )
    .await;
    assert_eq!(
        string_at(&branch, "/branch/commitId"),
        commit_three,
        "the branch tip is the newest commit"
    );
}

#[tokio::test]
async fn list_repositories_honours_sort_by_order_and_paging() {
    let router = emulator();
    // Modify the repositories in an order that differs from their name order, so
    // the two sort keys cannot be satisfied by one ordering.
    for name in ["r-alpha", "r-beta", "r-gamma"] {
        ok(
            &router,
            "CreateRepository",
            &json!({ "repositoryName": name }),
        )
        .await;
    }
    for name in ["r-gamma", "r-alpha", "r-beta"] {
        ok(
            &router,
            "PutFile",
            &json!({
                "repositoryName": name,
                "branchName": "main",
                "filePath": "f.txt",
                "fileContent": base64(name.as_bytes()),
            }),
        )
        .await;
        // Separate the modification instants so lastModifiedDate strictly orders.
        std::thread::sleep(Duration::from_millis(5));
    }

    // `SortByEnum` = repositoryName | lastModifiedDate and `OrderEnum` =
    // ascending | descending, with their defaults when the member is absent
    // (`smithy_query(query="shape",shape="SortByEnum,OrderEnum")`).
    let expectations: Vec<(Option<&str>, Option<&str>, [&str; 3])> = vec![
        (None, None, ["r-alpha", "r-beta", "r-gamma"]),
        (
            Some("repositoryName"),
            Some("ascending"),
            ["r-alpha", "r-beta", "r-gamma"],
        ),
        (
            Some("repositoryName"),
            Some("descending"),
            ["r-gamma", "r-beta", "r-alpha"],
        ),
        (
            Some("lastModifiedDate"),
            Some("ascending"),
            ["r-gamma", "r-alpha", "r-beta"],
        ),
        (
            Some("lastModifiedDate"),
            Some("descending"),
            ["r-beta", "r-alpha", "r-gamma"],
        ),
        (Some("lastModifiedDate"), None, ["r-gamma", "r-alpha", "r-beta"]),
    ];
    for (sort_by, order, expected) in expectations {
        let mut input = json!({});
        if let Some(sort_by) = sort_by {
            input["sortBy"] = json!(sort_by);
        }
        if let Some(order) = order {
            input["order"] = json!(order);
        }
        let listed = ok(&router, "ListRepositories", &input).await;
        let names: Vec<String> = listed
            .get("repositories")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("case {sort_by:?}/{order:?}: no repositories in {listed}"))
            .iter()
            .map(|pair| string_at(pair, "/repositoryName"))
            .collect();
        assert_eq!(
            names,
            expected.to_vec(),
            "case {sort_by:?}/{order:?}: expected {expected:?}, got {names:?}"
        );
    }

    // A token past the end of the list, and a token that is not one at all, are
    // both refused; a token at the end is an empty final page.
    for token in ["not-a-token", "99", "-1"] {
        let code = error(
            &router,
            "ListRepositories",
            &json!({ "nextToken": token }),
            400,
        )
        .await;
        assert_eq!(
            code, "InvalidContinuationTokenException",
            "case {token:?}: an unusable continuation token must be refused"
        );
    }
    let final_page = ok(&router, "ListRepositories", &json!({ "nextToken": "3" })).await;
    assert_eq!(
        final_page.get("repositories").and_then(Value::as_array),
        Some(&vec![]),
        "a token at the end of the list is an empty page, got {final_page}"
    );
    assert_eq!(
        final_page.get("nextToken"),
        None,
        "the last page carries no nextToken, got {final_page}"
    );

    // Values outside the two enums are refused with the error modeled for each.
    let code = error(
        &router,
        "ListRepositories",
        &json!({ "sortBy": "creationDate" }),
        400,
    )
    .await;
    assert_eq!(code, "InvalidSortByException", "sortBy outside the enum");
    let code = error(&router, "ListRepositories", &json!({ "order": "ASC" }), 400).await;
    assert_eq!(code, "InvalidOrderException", "order outside the enum");
}

#[tokio::test]
async fn writes_and_reads_are_refused_with_the_modeled_error() {
    let router = emulator();
    ok(
        &router,
        "CreateRepository",
        &json!({ "repositoryName": "guarded" }),
    )
    .await;
    let first = ok(
        &router,
        "PutFile",
        &json!({
            "repositoryName": "guarded",
            "branchName": "main",
            "filePath": "README.md",
            "fileContent": base64(b"one"),
        }),
    )
    .await;
    let tip = string_at(&first, "/commitId");
    ok(
        &router,
        "PutFile",
        &json!({
            "repositoryName": "guarded",
            "branchName": "main",
            "filePath": "docs/guide.md",
            "fileContent": base64(b"guide"),
            "parentCommitId": tip,
        }),
    )
    .await;
    let tip = string_at(
        &ok(
            &router,
            "GetBranch",
            &json!({ "repositoryName": "guarded", "branchName": "main" }),
        )
        .await,
        "/branch/commitId",
    );

    // Each row is a request that cannot be served, and the error the model names
    // for it. They differ along which part of the request is at fault: the
    // repository, the branch, the parent commit, the path, the content.
    let cases: Vec<(&str, Value, &str)> = vec![
        (
            "PutFile",
            json!({
                "repositoryName": "missing-repo",
                "branchName": "main",
                "filePath": "f.txt",
                "fileContent": base64(b"x"),
            }),
            "RepositoryDoesNotExistException",
        ),
        (
            "PutFile",
            json!({
                "repositoryName": "guarded",
                "branchName": "main",
                "filePath": "f.txt",
                "fileContent": base64(b"x"),
            }),
            "ParentCommitIdRequiredException",
        ),
        (
            "PutFile",
            json!({
                "repositoryName": "guarded",
                "branchName": "main",
                "filePath": "f.txt",
                "fileContent": base64(b"x"),
                "parentCommitId": first.get("commitId").cloned().unwrap_or(Value::Null),
            }),
            "ParentCommitIdOutdatedException",
        ),
        (
            "PutFile",
            json!({
                "repositoryName": "guarded",
                "branchName": "main",
                "filePath": "f.txt",
                "fileContent": base64(b"x"),
                "parentCommitId": "0".repeat(40),
            }),
            "ParentCommitDoesNotExistException",
        ),
        (
            "PutFile",
            json!({
                "repositoryName": "guarded",
                "branchName": "main",
                "filePath": "f.txt",
                "fileContent": base64(b"x"),
                "parentCommitId": "not-a-commit-id",
            }),
            "InvalidParentCommitIdException",
        ),
        (
            "PutFile",
            json!({
                "repositoryName": "guarded",
                "branchName": "does-not-exist",
                "filePath": "f.txt",
                "fileContent": base64(b"x"),
            }),
            "BranchDoesNotExistException",
        ),
        (
            "PutFile",
            json!({
                "repositoryName": "guarded",
                "branchName": "main",
                "filePath": "README.md",
                "fileContent": base64(b"one"),
                "parentCommitId": tip,
            }),
            "SameFileContentException",
        ),
        (
            "PutFile",
            json!({
                "repositoryName": "guarded",
                "branchName": "main",
                "filePath": "empty.txt",
                "fileContent": base64(b""),
                "parentCommitId": tip,
            }),
            "FileContentRequiredException",
        ),
        (
            "PutFile",
            json!({
                "repositoryName": "guarded",
                "branchName": "main",
                "filePath": "/",
                "fileContent": base64(b"x"),
                "parentCommitId": tip,
            }),
            "PathRequiredException",
        ),
        (
            "PutFile",
            json!({
                "repositoryName": "guarded",
                "branchName": "main",
                "filePath": "a/../b",
                "fileContent": base64(b"x"),
                "parentCommitId": tip,
            }),
            "InvalidPathException",
        ),
        (
            "PutFile",
            json!({
                "repositoryName": "guarded",
                "branchName": "main",
                "filePath": "x",
                "fileContent": base64(b"x"),
                "fileMode": "DIRECTORY",
                "parentCommitId": tip,
            }),
            "InvalidFileModeException",
        ),
        (
            // "README.md" is a file, so it cannot also be a directory.
            "PutFile",
            json!({
                "repositoryName": "guarded",
                "branchName": "main",
                "filePath": "README.md/child",
                "fileContent": base64(b"x"),
                "parentCommitId": tip,
            }),
            "DirectoryNameConflictsWithFileNameException",
        ),
        (
            // "docs" is a directory, so it cannot also be a file.
            "PutFile",
            json!({
                "repositoryName": "guarded",
                "branchName": "main",
                "filePath": "docs",
                "fileContent": base64(b"x"),
                "parentCommitId": tip,
            }),
            "FileNameConflictsWithDirectoryNameException",
        ),
        (
            "GetFile",
            json!({ "repositoryName": "guarded", "filePath": "nowhere.txt" }),
            "FileDoesNotExistException",
        ),
        (
            "GetFile",
            json!({
                "repositoryName": "guarded",
                "filePath": "README.md",
                "commitSpecifier": "0".repeat(40),
            }),
            "CommitDoesNotExistException",
        ),
        (
            "GetFile",
            json!({ "repositoryName": "missing-repo", "filePath": "README.md" }),
            "RepositoryDoesNotExistException",
        ),
        (
            "GetBranch",
            json!({ "repositoryName": "guarded", "branchName": "nope" }),
            "BranchDoesNotExistException",
        ),
        (
            "GetBranch",
            json!({ "repositoryName": "missing-repo", "branchName": "main" }),
            "RepositoryDoesNotExistException",
        ),
        (
            "GetBranch",
            json!({ "repositoryName": "guarded" }),
            "BranchNameRequiredException",
        ),
        (
            "GetBranch",
            json!({ "branchName": "main" }),
            "RepositoryNameRequiredException",
        ),
        (
            "GetRepository",
            json!({ "repositoryName": "missing-repo" }),
            "RepositoryDoesNotExistException",
        ),
        (
            // "The suffix .git is prohibited."
            "CreateRepository",
            json!({ "repositoryName": "mirror.git" }),
            "InvalidRepositoryNameException",
        ),
        (
            // "Key names cannot be prefixed with aws:."
            "CreateRepository",
            json!({ "repositoryName": "tagged", "tags": { "aws:internal": "no" } }),
            "InvalidSystemTagUsageException",
        ),
    ];

    for (operation, input, expected) in cases {
        let code = error(&router, operation, &input, 400).await;
        assert_eq!(
            code, expected,
            "case {operation} {input}: expected {expected}, got {code}"
        );
    }

    // None of the refusals wrote anything: the branch tip is still the commit the
    // last successful PutFile produced, and the refused paths are absent.
    let branch = ok(
        &router,
        "GetBranch",
        &json!({ "repositoryName": "guarded", "branchName": "main" }),
    )
    .await;
    assert_eq!(
        string_at(&branch, "/branch/commitId"),
        tip,
        "a refused write must not move the branch tip"
    );
    let code = error(
        &router,
        "GetFile",
        &json!({ "repositoryName": "guarded", "filePath": "f.txt" }),
        400,
    )
    .await;
    assert_eq!(
        code, "FileDoesNotExistException",
        "a refused write must not store the file"
    );
}

#[tokio::test]
async fn a_recreated_repository_starts_empty_with_a_new_id() {
    let router = emulator();
    let created = ok(
        &router,
        "CreateRepository",
        &json!({ "repositoryName": "recycled", "repositoryDescription": "first" }),
    )
    .await;
    let first_id = string_at(&created, "/repositoryMetadata/repositoryId");
    ok(
        &router,
        "PutFile",
        &json!({
            "repositoryName": "recycled",
            "branchName": "main",
            "filePath": "f.txt",
            "fileContent": base64(b"content"),
        }),
    )
    .await;
    let deleted = ok(
        &router,
        "DeleteRepository",
        &json!({ "repositoryName": "recycled" }),
    )
    .await;
    assert_eq!(
        string_at(&deleted, "/repositoryId"),
        first_id,
        "DeleteRepository returns the id of the repository it removed"
    );

    let recreated = ok(
        &router,
        "CreateRepository",
        &json!({ "repositoryName": "recycled", "repositoryDescription": "second" }),
    )
    .await;
    let metadata = recreated
        .get("repositoryMetadata")
        .expect("repositoryMetadata")
        .clone();
    assert_ne!(
        string_at(&metadata, "/repositoryId"),
        first_id,
        "a repository created again under the same name is a new repository"
    );
    assert_eq!(
        metadata.get("defaultBranch"),
        None,
        "the new repository has no branch yet, got {metadata}"
    );
    assert_eq!(
        metadata.get("repositoryDescription").and_then(Value::as_str),
        Some("second"),
        "the new repository carries its own description, got {metadata}"
    );
    let code = error(
        &router,
        "GetFile",
        &json!({ "repositoryName": "recycled", "filePath": "f.txt" }),
        400,
    )
    .await;
    assert_eq!(
        code, "CommitDoesNotExistException",
        "the deleted repository's commits must not survive it"
    );
}

#[tokio::test]
async fn the_store_is_bounded_on_every_axis_a_caller_drives() {
    use service_com_amazonaws_codecommit_codecommit_20150413::state::{
        ComAmazonawsCodecommitCodecommit20150413State, MAX_COMMITS_RETAINED, REPOSITORY_QUOTA,
    };

    let (router, engine) = emulator_with_engine();

    // Repository count: one per request, bounded by the store's own quota, and
    // refused with the error the model declares for a repository resource limit.
    for index in 0..REPOSITORY_QUOTA {
        ok(
            &router,
            "CreateRepository",
            &json!({ "repositoryName": format!("repo-{index:05}") }),
        )
        .await;
    }
    let code = error(
        &router,
        "CreateRepository",
        &json!({ "repositoryName": "one-too-many" }),
        400,
    )
    .await;
    assert_eq!(
        code, "RepositoryLimitExceededException",
        "the store must refuse a repository beyond its quota of {REPOSITORY_QUOTA}"
    );
    assert_eq!(
        engine
            .state::<ComAmazonawsCodecommitCodecommit20150413State>()
            .lock()
            .repositories
            .len(),
        REPOSITORY_QUOTA,
        "the refused create must not have been stored"
    );

    // Commit and blob count: repeated writes to one branch retain at most
    // MAX_COMMITS_RETAINED commits, and a blob no retained commit references is
    // released with it.
    let writes = MAX_COMMITS_RETAINED + 8;
    let mut tip = String::new();
    let mut first_commit = String::new();
    for index in 0..writes {
        let mut input = json!({
            "repositoryName": "repo-00000",
            "branchName": "main",
            "filePath": "f.txt",
            "fileContent": base64(format!("revision {index}").as_bytes()),
        });
        if index > 0 {
            input["parentCommitId"] = json!(tip);
        }
        tip = string_at(&ok(&router, "PutFile", &input).await, "/commitId");
        if index == 0 {
            first_commit = tip.clone();
        }
    }
    {
        let state = engine.state::<ComAmazonawsCodecommitCodecommit20150413State>();
        let store = state.lock();
        let repository = store
            .repositories
            .get("repo-00000")
            .expect("the repository written above");
        assert!(
            repository.commits.len() <= MAX_COMMITS_RETAINED,
            "after {writes} writes the repository must retain at most {MAX_COMMITS_RETAINED} commits, got {}",
            repository.commits.len()
        );
        // Every object the store holds is one a ref reaches: an object nothing
        // points at any more is released, and an object the history still needs
        // is kept (a clone has to be able to serve it).
        let roots: Vec<String> = repository.refs().into_iter().map(|(_, id)| id).collect();
        let reachable = repository.reachable_from(&roots);
        assert_eq!(
            repository.objects.len(),
            reachable.len(),
            "the object store must be exactly what the refs reach: {} stored, {} reachable",
            repository.objects.len(),
            reachable.len()
        );
    }

    // The branch tip is never dropped: the newest content is still readable.
    let got = ok(
        &router,
        "GetFile",
        &json!({ "repositoryName": "repo-00000", "filePath": "f.txt" }),
    )
    .await;
    assert_eq!(
        string_at(&got, "/commitId"),
        tip,
        "the tip commit must survive pruning"
    );
    assert_eq!(
        from_base64(&string_at(&got, "/fileContent")),
        format!("revision {}", writes - 1).as_bytes(),
        "the tip's content must survive pruning"
    );
    // A commit the repository no longer holds reads like one it never held.
    let code = error(
        &router,
        "GetFile",
        &json!({
            "repositoryName": "repo-00000",
            "filePath": "f.txt",
            "commitSpecifier": first_commit,
        }),
        400,
    )
    .await;
    assert_eq!(
        code, "CommitDoesNotExistException",
        "a pruned commit must answer like an unknown commit"
    );
}
