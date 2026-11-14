// SPDX-License-Identifier: CC0-1.0

//! Which of two lookups decides the refusal, over the wire: an operation resolves the
//! resource it ACTS ON first, and reports that one's absence.
//!
//! The rule under test is about precedence rather than about one request. An operation that
//! acts on the PULL REQUEST — its comments — resolves the pull request id first (on
//! `GetCommentsForPullRequest` the repository name is not even required), so a request
//! carrying an id no pull request answers to is told that, with
//! `PullRequestDoesNotExistException` and the fixed three-sentence text CodeCommit words that
//! code with, even when the `repositoryName` beside it names no repository either. An
//! operation that acts on the REPOSITORY — the three merges, each of which writes into its
//! object store and moves its destination reference — resolves the repository name first, and
//! a name the account holds no repository for is `RepositoryDoesNotExistException` with the
//! `<repositoryName> for <accountId>` text, whatever the pull request id names.
//!
//! The tables therefore vary what each rule generalises over — the operation, the shape of
//! the repository name that names no repository, and the shape of the id that names no pull
//! request — and assert the same code and message on every row.
//!
//! The other half of each precedence is asserted here too: once the pull request DOES exist, a
//! repository name the account holds no repository for is `RepositoryDoesNotExistException` on
//! every one of the five operations.
//!
//! The complementary halves are pinned next door: an id belonging to another
//! repository is `RepositoryNotAssociatedWithPullRequestException`
//! (`crate::pull_request_comments`), and `crate::client_error_messages` pins the
//! repository code's template.

use serde_json::{json, Value};

use crate::harness::error_with_message;
use crate::pull_request_comments::pull_request_on;
use crate::repository_lifecycle::emulator;

/// The HTTP status both codes model (`errorKind: client`, `httpError: 400`).
const CLIENT_ERROR_STATUS: u16 = 400;

/// A syntactically valid git object id (40 hex characters), so an operation taking
/// a commit id gets past its own `InvalidCommitIdException` check and reaches the
/// lookups under test.
const AFTER_COMMIT_ID: &str = "0123456789abcdef0123456789abcdef01234567";
const BEFORE_COMMIT_ID: &str = "76543210fedcba9876543210fedcba9876543210";

/// `PullRequestDoesNotExistException`'s message: entirely fixed text, spelled out
/// here rather than read off the crate so the WORDING is what these rows are held
/// to.
const PULL_REQUEST_DOES_NOT_EXIST: &str =
    "The pull request ID does not exist. Verify that you provided a valid pull request ID, and \
     then try your operation again. To get a list of pull request IDs, use ListPullRequests.";

/// Repository names that are valid `RepositoryName`s (1..100 of `[\w\.-]+`:
/// `smithy_query(query="shape",shape="RepositoryName")`) and name no repository on
/// an emulator that holds none. Rows differ along length and spelling.
fn absent_repositories() -> Vec<String> {
    vec![
        "q7".to_string(),
        "no-such-repository".to_string(),
        "no.such_repository-in-this-much-longer-name".to_string(),
        "r".repeat(100),
    ]
}

/// Pull request ids that are ids (`PullRequestId` is a decimal string: see
/// `crate::state::normalize_pull_request_id`) and name no pull request on an
/// emulator that has minted none.
fn absent_pull_request_ids() -> Vec<&'static str> {
    vec!["1", "999999", "18446744073709551615"]
}

/// The message CodeCommit words `RepositoryDoesNotExistException` with: the
/// requested name, the fixed `" for "`, then the account the lookup ran in.
fn expected_message(repository: &str) -> String {
    format!("{repository} for {}", engine::emulator_account_id())
}

/// Every operation that takes both a `pullRequestId` and a `repositoryName`, with
/// the rest of its required members filled in so each reaches the two lookups.
fn requests_naming(repository: &str, pull_request_id: &str) -> Vec<(&'static str, Value)> {
    let mut requests = comment_requests_naming(repository, pull_request_id);
    requests.extend(merge_requests_naming(repository, pull_request_id));
    requests
}

/// The operations that act on the PULL REQUEST: a comment is attached to the pull request, so
/// the pull request is the resource they address.
fn comment_requests_naming(repository: &str, pull_request_id: &str) -> Vec<(&'static str, Value)> {
    vec![
        (
            "PostCommentForPullRequest",
            json!({
                "pullRequestId": pull_request_id,
                "repositoryName": repository,
                "beforeCommitId": BEFORE_COMMIT_ID,
                "afterCommitId": AFTER_COMMIT_ID,
                "content": "a comment",
            }),
        ),
        (
            "GetCommentsForPullRequest",
            json!({
                "pullRequestId": pull_request_id,
                "repositoryName": repository,
                // Naming the repository makes both sides of the comparison
                // required on this operation, so both are filled in.
                "beforeCommitId": BEFORE_COMMIT_ID,
                "afterCommitId": AFTER_COMMIT_ID,
            }),
        ),
    ]
}

/// The operations that act on the REPOSITORY: a merge writes a commit into it and moves its
/// destination reference, so the repository is the resource they address.
fn merge_requests_naming(repository: &str, pull_request_id: &str) -> Vec<(&'static str, Value)> {
    vec![
        (
            "MergePullRequestByFastForward",
            json!({
                "pullRequestId": pull_request_id,
                "repositoryName": repository,
            }),
        ),
        (
            "MergePullRequestBySquash",
            json!({
                "pullRequestId": pull_request_id,
                "repositoryName": repository,
            }),
        ),
        (
            "MergePullRequestByThreeWay",
            json!({
                "pullRequestId": pull_request_id,
                "repositoryName": repository,
            }),
        ),
    ]
}

#[tokio::test]
async fn a_comment_operation_resolves_the_pull_request_before_the_repository() {
    let router = emulator();

    for repository in absent_repositories() {
        for pull_request_id in absent_pull_request_ids() {
            for (operation, body) in comment_requests_naming(&repository, pull_request_id) {
                let (code, message) =
                    error_with_message(&router, operation, &body, CLIENT_ERROR_STATUS).await;
                assert_eq!(
                    code, "PullRequestDoesNotExistException",
                    "case ({operation}, repository {repository:?}, id {pull_request_id}): the \
                     pull request is the resource addressed, so expected code \
                     PullRequestDoesNotExistException, got {code:?}"
                );
                assert_eq!(
                    message, PULL_REQUEST_DOES_NOT_EXIST,
                    "case ({operation}, repository {repository:?}, id {pull_request_id}): \
                     expected message {PULL_REQUEST_DOES_NOT_EXIST:?}, got {message:?}"
                );
                assert!(
                    !message.contains(repository.as_str()),
                    "case ({operation}, repository {repository:?}, id {pull_request_id}): the \
                     refusal must not name a member the request's repository carried, got \
                     {message:?}"
                );
            }
        }
    }
}

/// The mirror rule for the operations that act on the repository: a merge writes into the
/// repository the request names, so a name the account holds no repository for is reported
/// first — whatever the pull request id beside it names.
#[tokio::test]
async fn a_merge_resolves_the_repository_before_the_pull_request() {
    let router = emulator();

    for repository in absent_repositories() {
        for pull_request_id in absent_pull_request_ids() {
            for (operation, body) in merge_requests_naming(&repository, pull_request_id) {
                let (code, message) =
                    error_with_message(&router, operation, &body, CLIENT_ERROR_STATUS).await;
                assert_eq!(
                    code, "RepositoryDoesNotExistException",
                    "case ({operation}, repository {repository:?}, id {pull_request_id}): the \
                     repository is the resource the merge acts on, so expected code \
                     RepositoryDoesNotExistException, got {code:?}"
                );
                let expected = expected_message(&repository);
                assert_eq!(
                    message, expected,
                    "case ({operation}, repository {repository:?}, id {pull_request_id}): \
                     expected message {expected:?}, got {message:?}"
                );
            }
        }
    }
}

/// The other half of each precedence: a `repositoryName` the account holds no repository for
/// is the repository refusal on every one of the five operations once the `pullRequestId` DOES
/// name a pull request — under the template CodeCommit words that code with.
#[tokio::test]
async fn a_repository_that_does_not_exist_is_still_reported_once_the_pull_request_resolves() {
    let router = emulator();

    // One real repository with a pull request between two branches, minted through the
    // implemented write path by the fixture the comment suite already builds, so the id
    // below names a pull request this account really holds.
    let pr = pull_request_on(&router, "pull-request-precedence").await;
    let pull_request_id = pr.pull_request_id.clone();
    assert!(
        !pull_request_id.is_empty(),
        "the pull request must be minted for these rows to be about the repository"
    );

    for absent in absent_repositories() {
        for (operation, body) in requests_naming(&absent, &pull_request_id) {
            let (code, message) =
                error_with_message(&router, operation, &body, CLIENT_ERROR_STATUS).await;
            assert_eq!(
                code, "RepositoryDoesNotExistException",
                "case ({operation}, repository {absent:?}, id {pull_request_id}): the pull \
                 request resolved, so expected code RepositoryDoesNotExistException, got \
                 {code:?}"
            );
            let expected = expected_message(&absent);
            assert_eq!(
                message, expected,
                "case ({operation}, repository {absent:?}, id {pull_request_id}): expected \
                 message {expected:?}, got {message:?}"
            );
        }
    }
}
