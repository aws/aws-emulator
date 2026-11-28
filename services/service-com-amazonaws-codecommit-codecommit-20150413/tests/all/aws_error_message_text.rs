// SPDX-License-Identifier: CC0-1.0

//! The TEXT CodeCommit puts on the client errors whose message rule is known, over
//! the wire.
//!
//! The rule under test: each of these error codes carries ONE message — a CONSTANT
//! for the class where the service's text names nothing from the request, and a
//! TEMPLATE whose slot is a named request value where it does — whichever operation
//! raises the code and whatever else the request said. Nothing the emulator's own
//! layers know is added to it: not the member a validator rejected, and not the
//! diagnostic of whichever JSON reader refused a document.
//!
//! So each table below varies exactly what its rule generalises over — the template
//! name (the 1- and 100-character bounds of `ApprovalRuleTemplateName`, digits,
//! dots, hyphens, underscores, non-ASCII), the way a document fails to be an
//! approval rule document, the `maxResults` value (each published bound and one past
//! it), the operation that raises the code, the malformed ARN, the unrecognized
//! reaction value — and asserts the same constant, or the same template with the
//! request's value in its slot, on every row.
//!
//! Sibling suites own the neighbouring rules: `client_error_messages.rs` pins the
//! repository-scoped codes' templates, and `comment_deleted_refusals.rs` pins
//! `CommentDeletedException` across comments, operations and principals. The bounds
//! and member names here come from the model (`smithy_query(query="shape",
//! shape="ApprovalRuleTemplateName")` — `len 1..100`, no pattern; `BranchName` —
//! `len 1..256`; `Content` and `ResourceArn` — unconstrained, so their emptiness is
//! a handler rule).
//!
//! One of these codes carries NO message at all — `IdempotencyParameterMismatchException`
//! — and the absence is as much part of that refusal as the code is, so it is asserted
//! here beside the texts.

use serde_json::{json, Value};

use crate::comment_lifecycle::{fixture, Fixture, MAX_COMMENT_CHARACTERS, PATH};
use crate::harness::error_with_message;
use crate::pull_request_comments::{pull_request_on, PullRequest};
use crate::repository_lifecycle::{base64, emulator, ok, string_at, REGION};
use crate::wire::fresh_client_request_token;

/// The HTTP status every error asserted here models (`errorKind: client`,
/// `httpError: 400` on each of these error shapes).
const CLIENT_ERROR_STATUS: u16 = 400;

/// A syntactically valid git object id (40 hex characters), so an operation that
/// takes a commit id gets past its own `InvalidCommitIdException` check and reaches
/// the rule under test.
const FULL_OBJECT_ID: &str = "0123456789abcdef0123456789abcdef01234567";

/// Approval rule template names spanning what `ApprovalRuleTemplateName` allows:
/// the 1-character minimum, the 100-character maximum, and names mixing digits,
/// dots, hyphens, underscores and non-ASCII letters (the shape carries a length
/// bound and no pattern) — none of them a name the issue cited.
fn template_names() -> Vec<String> {
    vec![
        "t".to_string(),
        "x".repeat(100),
        "template-with-hyphens".to_string(),
        "template_with_underscores".to_string(),
        "Template.With.Dots".to_string(),
        "MiXeD-Case_9.9".to_string(),
        "revisión-obligatoria".to_string(),
    ]
}

/// A valid approval rule template document, as compact JSON text. The structure
/// (`Version`, `Statements` of `Type` `Approvers` with `NumberOfApprovalsNeeded`) is
/// the one the CodeCommit User Guide documents for approval rule template content.
fn document(approvals: u32) -> String {
    serde_json::to_string(&json!({
        "Version": "2018-11-08",
        "Statements": [{
            "Type": "Approvers",
            "NumberOfApprovalsNeeded": approvals,
            "ApprovalPoolMembers": ["arn:aws:sts::123456789012:assumed-role/CodeCommitReview/*"],
        }],
    }))
    .expect("the document serializes")
}

// ---------------------------------------------------------------------------
// Cause 1 — ApprovalRuleTemplateNameAlreadyExistsException
// ---------------------------------------------------------------------------

/// The message is a CONSTANT: it names neither the template the caller asked for
/// nor a Region, because CodeCommit scopes template names per ACCOUNT.
///
/// `UpdateApprovalRuleTemplateName` raises the same code and is still a 501 stub
/// (`src/ops/update_approval_rule_template_name.rs` carries the scaffold header), so
/// the only operation that can be asserted here is the create path.
#[tokio::test]
async fn approval_rule_template_name_already_exists_message_is_a_constant() {
    let router = emulator();
    const EXPECTED: &str = "The approval rule template cannot be created because a template with \
                            that name already exists in your AWS account.";

    for name in template_names() {
        ok(
            &router,
            "CreateApprovalRuleTemplate",
            &json!({
                "approvalRuleTemplateName": name,
                "approvalRuleTemplateContent": document(1),
            }),
        )
        .await;

        // A second create of the same name, carrying DIFFERENT content, so the
        // rejection cannot be read as byte-identical-request idempotence.
        let (code, message) = error_with_message(
            &router,
            "CreateApprovalRuleTemplate",
            &json!({
                "approvalRuleTemplateName": name,
                "approvalRuleTemplateContent": document(2),
                "approvalRuleTemplateDescription": "a second attempt",
            }),
            CLIENT_ERROR_STATUS,
        )
        .await;
        assert_eq!(
            code, "ApprovalRuleTemplateNameAlreadyExistsException",
            "case {name:?}: expected code ApprovalRuleTemplateNameAlreadyExistsException, \
             got {code:?}"
        );
        assert_eq!(
            message, EXPECTED,
            "case {name:?}: expected message {EXPECTED:?}, got {message:?}"
        );

        // The refusal left the stored template as the first create made it: the
        // content the rejected request carried was NOT written.
        let read_back = ok(
            &router,
            "GetApprovalRuleTemplate",
            &json!({"approvalRuleTemplateName": name}),
        )
        .await;
        assert_eq!(
            string_at(&read_back, "/approvalRuleTemplate/approvalRuleTemplateContent"),
            document(1),
            "case {name:?}: the rejected create must not touch the stored template, \
             got {read_back}"
        );
    }
}

// ---------------------------------------------------------------------------
// Cause 2 — ApprovalRuleTemplateDoesNotExistException
// ---------------------------------------------------------------------------

/// The message is a TEMPLATE whose one slot is the template name the request asked
/// for, followed by the sentence naming the likeliest cause — the same text from
/// every operation that resolves a template name.
#[tokio::test]
async fn approval_rule_template_does_not_exist_message_names_the_requested_template() {
    let router = emulator();
    let repository = "repo-for-template-lookups";
    ok(
        &router,
        "CreateRepository",
        &json!({"repositoryName": repository}),
    )
    .await;

    for name in template_names() {
        // Every row is a name the store has never held, so each operation has to
        // answer with the not-found error rather than resolve a template.
        let requests: Vec<(&str, Value)> = vec![
            (
                "GetApprovalRuleTemplate",
                json!({"approvalRuleTemplateName": name}),
            ),
            (
                "AssociateApprovalRuleTemplateWithRepository",
                json!({
                    "approvalRuleTemplateName": name,
                    "repositoryName": repository,
                }),
            ),
            (
                "ListRepositoriesForApprovalRuleTemplate",
                json!({"approvalRuleTemplateName": name}),
            ),
            (
                "UpdateApprovalRuleTemplateContent",
                json!({
                    "approvalRuleTemplateName": name,
                    "newRuleContent": document(3),
                }),
            ),
        ];
        let expected = format!(
            "No approval rule template exists in this AWS Region with the following name: \
             {name}. Make sure that you are signed in to the AWS Region where you created the \
             rule, and then try again."
        );
        for (operation, body) in requests {
            let (code, message) =
                error_with_message(&router, operation, &body, CLIENT_ERROR_STATUS).await;
            assert_eq!(
                code, "ApprovalRuleTemplateDoesNotExistException",
                "case ({operation}, {name:?}): expected code \
                 ApprovalRuleTemplateDoesNotExistException, got {code:?}"
            );
            assert_eq!(
                message, expected,
                "case ({operation}, {name:?}): expected message {expected:?}, got {message:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Cause 3 — InvalidApprovalRuleTemplateContentException
// ---------------------------------------------------------------------------

/// Documents that are not an approval rule document, each failing in a different
/// way: the rows differ along WHERE the document stops being one, including two
/// truncations whose parser diagnostics would report different offsets.
fn invalid_documents() -> Vec<(&'static str, String)> {
    vec![
        ("truncated after the opening brace", "{bad json".to_string()),
        (
            "truncated one character later",
            "{\"bad json".to_string(),
        ),
        ("a JSON array, not an object", "[]".to_string()),
        ("a bare JSON string", "\"not a document\"".to_string()),
        ("an object with no Version", "{}".to_string()),
        (
            "a Version the schema does not define",
            serde_json::to_string(&json!({
                "Version": "1999-01-01",
                "Statements": [{"Type": "Approvers", "NumberOfApprovalsNeeded": 1}],
            }))
            .expect("the document serializes"),
        ),
        (
            "no Statements array",
            serde_json::to_string(&json!({"Version": "2018-11-08"}))
                .expect("the document serializes"),
        ),
        (
            "an empty Statements array",
            serde_json::to_string(&json!({"Version": "2018-11-08", "Statements": []}))
                .expect("the document serializes"),
        ),
        (
            "a statement Type the schema does not define",
            serde_json::to_string(&json!({
                "Version": "2018-11-08",
                "Statements": [{"Type": "Rejecters", "NumberOfApprovalsNeeded": 1}],
            }))
            .expect("the document serializes"),
        ),
        (
            "NumberOfApprovalsNeeded below one",
            serde_json::to_string(&json!({
                "Version": "2018-11-08",
                "Statements": [{"Type": "Approvers", "NumberOfApprovalsNeeded": 0}],
            }))
            .expect("the document serializes"),
        ),
        (
            "an ApprovalPoolMembers entry that is not a string",
            serde_json::to_string(&json!({
                "Version": "2018-11-08",
                "Statements": [{
                    "Type": "Approvers",
                    "NumberOfApprovalsNeeded": 1,
                    "ApprovalPoolMembers": [7],
                }],
            }))
            .expect("the document serializes"),
        ),
        (
            "a DestinationReferences entry that is not a string",
            serde_json::to_string(&json!({
                "Version": "2018-11-08",
                "Statements": [{"Type": "Approvers", "NumberOfApprovalsNeeded": 1}],
                "DestinationReferences": [{"ref": "main"}],
            }))
            .expect("the document serializes"),
        ),
    ]
}

/// The message is a CONSTANT, the same for every way a document fails to be an
/// approval rule document — and it carries no parser diagnostic, which is why no
/// row's message may differ from another's even though their offsets do.
#[tokio::test]
async fn invalid_approval_rule_template_content_message_is_a_constant() {
    let router = emulator();
    const EXPECTED: &str = "The JSON in the approval rule template is not valid. Review the JSON \
                            and make sure it is valid, and then try again.";
    // A template the update path can address, so both operations' refusals are the
    // content rule's rather than a lookup's.
    let existing = "template-for-content-updates";
    ok(
        &router,
        "CreateApprovalRuleTemplate",
        &json!({
            "approvalRuleTemplateName": existing,
            "approvalRuleTemplateContent": document(1),
        }),
    )
    .await;

    for (label, content) in invalid_documents() {
        let requests: Vec<(&str, &str, Value)> = vec![
            (
                "CreateApprovalRuleTemplate",
                "InvalidApprovalRuleTemplateContentException",
                json!({
                    "approvalRuleTemplateName": format!("t-{}", content.len()),
                    "approvalRuleTemplateContent": content,
                }),
            ),
            (
                "UpdateApprovalRuleTemplateContent",
                "InvalidApprovalRuleTemplateContentException",
                json!({
                    "approvalRuleTemplateName": existing,
                    "newRuleContent": content,
                }),
            ),
        ];
        for (operation, expected_code, body) in requests {
            let (code, message) =
                error_with_message(&router, operation, &body, CLIENT_ERROR_STATUS).await;
            assert_eq!(
                code, expected_code,
                "case ({operation}, {label}): expected code {expected_code}, got {code:?}"
            );
            assert_eq!(
                message, EXPECTED,
                "case ({operation}, {label}): expected message {EXPECTED:?}, got {message:?}"
            );
            // The one diagnostic that must never reach a caller is the JSON reader's
            // own, which reports a position: the constant carries no digit at all, so
            // an offset leaking back in fails here.
            assert!(
                !message.bytes().any(|byte| byte.is_ascii_digit()),
                "case ({operation}, {label}): the message must carry no parser offset, \
                 got {message:?}"
            );
        }
    }

    // The template the update path addressed still holds the content it was created
    // with: a refused document is not stored.
    let read_back = ok(
        &router,
        "GetApprovalRuleTemplate",
        &json!({"approvalRuleTemplateName": existing}),
    )
    .await;
    assert_eq!(
        string_at(&read_back, "/approvalRuleTemplate/approvalRuleTemplateContent"),
        document(1),
        "a refused content update must leave the stored document alone, got {read_back}"
    );
}

/// The same document rule refused on a pull request's OWN approval rule carries the
/// sentence pair for an approval RULE, not for a template: each error class states
/// the thing the caller submitted, and `InvalidApprovalRuleContentException` is the
/// class for a rule.
#[tokio::test]
async fn invalid_approval_rule_content_message_names_the_rule_not_the_template() {
    let router = emulator();
    const EXPECTED: &str = "The JSON in the approval rule is not valid. Review the JSON and make \
                            sure it is valid, and then try again.";
    let pr = crate::pull_request_comments::pull_request_on(&router, "rule-content-messages").await;

    for (label, content) in invalid_documents() {
        let (code, message) = error_with_message(
            &router,
            "CreatePullRequestApprovalRule",
            &json!({
                "pullRequestId": pr.pull_request_id,
                "approvalRuleName": format!("rule-{}", content.len()),
                "approvalRuleContent": content,
            }),
            CLIENT_ERROR_STATUS,
        )
        .await;
        assert_eq!(
            code, "InvalidApprovalRuleContentException",
            "case {label:?}: expected InvalidApprovalRuleContentException, got {code:?}"
        );
        assert_eq!(
            message, EXPECTED,
            "case {label:?}: expected message {EXPECTED:?}, got {message:?}"
        );
        // The parser's own diagnostic reports a position, so a message carrying a
        // digit is one that leaked an offset.
        assert!(
            !message.bytes().any(|byte| byte.is_ascii_digit()),
            "case {label:?}: the message must carry no parser offset, got {message:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Causes 4 and 5 — the pagination refusals
// ---------------------------------------------------------------------------

/// The listings whose model declares `InvalidMaxResultsException` and
/// `InvalidContinuationTokenException`, each with the request that reaches this
/// service's one pagination rule, and the fixture those requests need.
///
/// `ListTagsForResource` pages through the same rule but its model declares neither
/// error and no `maxResults` member at all, so it answers a token it never issued as
/// the bad request it is rather than with one of these codes; it is not a row here.
async fn paginated_requests(
    router: &smithy_server_runtime::MultiServiceRouter,
) -> Vec<(&'static str, Value)> {
    let repository = "repo-for-pagination";
    let template = "template-for-pagination";
    ok(
        router,
        "CreateRepository",
        &json!({"repositoryName": repository}),
    )
    .await;
    ok(
        router,
        "CreateApprovalRuleTemplate",
        &json!({
            "approvalRuleTemplateName": template,
            "approvalRuleTemplateContent": document(1),
        }),
    )
    .await;
    vec![
        ("ListApprovalRuleTemplates", json!({})),
        (
            "ListRepositoriesForApprovalRuleTemplate",
            json!({"approvalRuleTemplateName": template}),
        ),
        (
            "ListAssociatedApprovalRuleTemplatesForRepository",
            json!({"repositoryName": repository}),
        ),
    ]
}

/// `InvalidContinuationTokenException`'s message is a TEMPLATE: the literal
/// `Error parsing next token: ` and then the token the request sent.
#[tokio::test]
async fn invalid_continuation_token_message_names_the_token() {
    let router = emulator();
    let requests = paginated_requests(&router).await;

    let tokens = vec![
        "not-a-real-continuation-token".to_string(),
        // Decimal: a position a caller may guess at, which is not a token.
        "0".to_string(),
        // Valid hex, so it decodes, but it is not a token this service issued.
        "deadbeef".to_string(),
        "Zażółć gęślą jaźń".to_string(),
        "t".repeat(512),
    ];

    for token in tokens {
        let expected = format!("Error parsing next token: {token}");
        for (operation, body) in &requests {
            let mut body = body.clone();
            body["nextToken"] = json!(token);
            let (code, message) =
                error_with_message(&router, operation, &body, CLIENT_ERROR_STATUS).await;
            assert_eq!(
                code, "InvalidContinuationTokenException",
                "case ({operation}, {token:?}): expected code \
                 InvalidContinuationTokenException, got {code:?}"
            );
            assert_eq!(
                message, expected,
                "case ({operation}, {token:?}): expected message {expected:?}, got {message:?}"
            );
        }
    }

    // A token this service really issued, for ANOTHER listing: it is not a token for
    // the listing it is replayed against, and the slot carries it exactly as sent —
    // so the template holds for a token-shaped value too, not only for prose.
    ok(
        &router,
        "CreateApprovalRuleTemplate",
        &json!({
            "approvalRuleTemplateName": "another-template-for-pagination",
            "approvalRuleTemplateContent": document(1),
        }),
    )
    .await;
    let first_page = ok(
        &router,
        "ListApprovalRuleTemplates",
        &json!({"maxResults": 1}),
    )
    .await;
    let issued_elsewhere = string_at(&first_page, "/nextToken");
    let expected = format!("Error parsing next token: {issued_elsewhere}");
    for (operation, body) in &requests {
        // Except on the listing that issued it, where it is the valid resumption
        // point the service handed out.
        if *operation == "ListApprovalRuleTemplates" {
            continue;
        }
        let mut body = body.clone();
        body["nextToken"] = json!(issued_elsewhere);
        let (code, message) =
            error_with_message(&router, operation, &body, CLIENT_ERROR_STATUS).await;
        assert_eq!(
            code, "InvalidContinuationTokenException",
            "case ({operation}, a token issued for another listing): expected code \
             InvalidContinuationTokenException, got {code:?}"
        );
        assert_eq!(
            message, expected,
            "case ({operation}, a token issued for another listing): expected message \
             {expected:?}, got {message:?}"
        );
    }
    // And on that listing it still resumes, so the row above refuses it for the
    // listing it belongs to being another one, not for the token being unreadable.
    let resumed = ok(
        &router,
        "ListApprovalRuleTemplates",
        &json!({"maxResults": 1, "nextToken": issued_elsewhere}),
    )
    .await;
    assert!(
        resumed["approvalRuleTemplateNames"]
            .as_array()
            .is_some_and(|names| names.len() == 1),
        "the issuing listing must resume from its own token, got {resumed}"
    );
}

/// The same template on the operations that carry their OWN cursor rather than
/// paging through the shared rule: one code, one message, however the operation
/// tracks its position.
#[tokio::test]
async fn invalid_continuation_token_message_names_the_token_on_cursor_operations() {
    let f = fixture("repo-for-cursor-tokens").await;
    let requests: Vec<(&str, &str, Value)> = vec![
        ("ListRepositories", "nextToken", json!({})),
        (
            "ListBranches",
            "nextToken",
            json!({"repositoryName": f.repository}),
        ),
        (
            "ListFileCommitHistory",
            "nextToken",
            json!({"repositoryName": f.repository, "filePath": PATH}),
        ),
        (
            // `GetDifferences` spells the member `NextToken`
            // (`smithy_query(query="shape",shape="GetDifferencesInput")`).
            "GetDifferences",
            "NextToken",
            json!({
                "repositoryName": f.repository,
                "beforeCommitSpecifier": f.before_commit,
                "afterCommitSpecifier": f.after_commit,
            }),
        ),
        (
            "GetCommentsForComparedCommit",
            "nextToken",
            json!({
                "repositoryName": f.repository,
                "beforeCommitId": f.before_commit,
                "afterCommitId": f.after_commit,
            }),
        ),
    ];

    // These cursors are decimal offsets, so a value that IS an offset into the
    // current answer is a token they issued; the rows below are values that are not
    // — prose, hex, non-ASCII text, a very long value, and a position past the end.
    let tokens = [
        "not-a-real-continuation-token".to_string(),
        "deadbeef".to_string(),
        "Zażółć gęślą jaźń".to_string(),
        "t".repeat(512),
        "4294967296".to_string(),
    ];
    for token in tokens {
        let expected = format!("Error parsing next token: {token}");
        for (operation, member, body) in &requests {
            let mut body = body.clone();
            body[*member] = json!(token);
            let (code, message) =
                error_with_message(&f.router, operation, &body, CLIENT_ERROR_STATUS).await;
            assert_eq!(
                code, "InvalidContinuationTokenException",
                "case ({operation}, {token:?}): expected code \
                 InvalidContinuationTokenException, got {code:?}"
            );
            assert_eq!(
                message, expected,
                "case ({operation}, {token:?}): expected message {expected:?}, got {message:?}"
            );
        }
    }
}

/// `InvalidMaxResultsException`'s message is a CONSTANT stating the accepted range:
/// it does not echo the rejected value. Each published bound is accepted and the
/// value one past each is refused.
#[tokio::test]
async fn invalid_max_results_message_is_the_accepted_range() {
    let router = emulator();
    let requests = paginated_requests(&router).await;
    // The published range: `MIN_MAX_RESULTS`..=`MAX_MAX_RESULTS` in `crate::state`.
    const EXPECTED: &str = "A valid limit is between 1 and 1000";

    for value in [0i64, -1, 1001, i64::from(i32::MIN), i64::from(i32::MAX)] {
        for (operation, body) in &requests {
            let mut body = body.clone();
            body["maxResults"] = json!(value);
            let (code, message) =
                error_with_message(&router, operation, &body, CLIENT_ERROR_STATUS).await;
            assert_eq!(
                code, "InvalidMaxResultsException",
                "case ({operation}, maxResults {value}): expected code \
                 InvalidMaxResultsException, got {code:?}"
            );
            assert_eq!(
                message, EXPECTED,
                "case ({operation}, maxResults {value}): expected message {EXPECTED:?}, \
                 got {message:?}"
            );
        }
    }

    // Both published bounds are inside the range, so they are served.
    for value in [1i64, 1000] {
        for (operation, body) in &requests {
            let mut body = body.clone();
            body["maxResults"] = json!(value);
            let served = ok(&router, operation, &body).await;
            assert!(
                served.is_object(),
                "case ({operation}, maxResults {value}): a published bound must be served, \
                 got {served}"
            );
        }
    }
}

/// `InvalidMaxResultsException`'s message states the accepted `maxResults` on EVERY
/// operation that takes one, not only the paginated listings — and it states that
/// operation's OWN rule, which is what the shared wording generalises over: the
/// range for an operation that publishes a maximum, and the member's documented
/// "non-zero, non-negative integer" rule for one that publishes none and so serves
/// every positive value. Neither form echoes the rejected value.
#[tokio::test]
async fn invalid_max_results_message_states_each_operations_own_range() {
    let f = fixture("repo-for-max-results-ranges").await;
    // (operation, the member it spells `maxResults` with, the request that reaches
    // its `maxResults` rule, the rule it states, a value that rule accepts).
    // `GetCommentsForComparedCommit`'s maximum is `crate::state::COMMENTS_PAGE_MAX`;
    // `ListFileCommitHistory` and `GetDifferences` publish no maximum and refuse
    // only a non-positive value. `GetDifferences` spells the member `MaxResults`
    // (`smithy_query(query="shape",shape="GetDifferencesInput")`).
    let cases: Vec<(&str, &str, Value, &str, i64)> = vec![
        (
            "GetCommentsForComparedCommit",
            "maxResults",
            json!({
                "repositoryName": f.repository,
                "beforeCommitId": f.before_commit,
                "afterCommitId": f.after_commit,
            }),
            "A valid limit is between 1 and 500",
            500,
        ),
        (
            "ListFileCommitHistory",
            "maxResults",
            json!({"repositoryName": f.repository, "filePath": PATH}),
            "A valid limit is a non-zero, non-negative integer",
            i64::from(i32::MAX),
        ),
        (
            "GetDifferences",
            "MaxResults",
            json!({
                "repositoryName": f.repository,
                "beforeCommitSpecifier": f.before_commit,
                "afterCommitSpecifier": f.after_commit,
            }),
            "A valid limit is a non-zero, non-negative integer",
            i64::from(i32::MAX),
        ),
    ];

    for (operation, member, body, expected, inside) in cases {
        for value in [0i64, -1, i64::from(i32::MIN)] {
            let mut refused = body.clone();
            refused[member] = json!(value);
            let (code, message) =
                error_with_message(&f.router, operation, &refused, CLIENT_ERROR_STATUS).await;
            assert_eq!(
                code, "InvalidMaxResultsException",
                "case ({operation}, maxResults {value}): expected code \
                 InvalidMaxResultsException, got {code:?}"
            );
            assert_eq!(
                message, expected,
                "case ({operation}, maxResults {value}): expected message {expected:?}, \
                 got {message:?}"
            );
        }
        // Each value the rule the message states accepts is served.
        for value in [1i64, inside] {
            let mut served_body = body.clone();
            served_body[member] = json!(value);
            let served = ok(&f.router, operation, &served_body).await;
            assert!(
                served.is_object(),
                "case ({operation}, maxResults {value}): a value inside the published range \
                 must be served, got {served}"
            );
        }
    }

    // And the value one past the bound `GetCommentsForComparedCommit` publishes is
    // refused, so its range is the one its message names rather than the listings'.
    let (code, message) = error_with_message(
        &f.router,
        "GetCommentsForComparedCommit",
        &json!({
            "repositoryName": f.repository,
            "beforeCommitId": f.before_commit,
            "afterCommitId": f.after_commit,
            "maxResults": 501,
        }),
        CLIENT_ERROR_STATUS,
    )
    .await;
    assert_eq!(
        code, "InvalidMaxResultsException",
        "case (GetCommentsForComparedCommit, maxResults 501): expected code \
         InvalidMaxResultsException, got {code:?}"
    );
    assert_eq!(
        message, "A valid limit is between 1 and 500",
        "case (GetCommentsForComparedCommit, maxResults 501): expected the operation's own \
         range, got {message:?}"
    );
}

// ---------------------------------------------------------------------------
// Cause 6 — InvalidBranchNameException for an empty branchName
// ---------------------------------------------------------------------------

/// A request naming a branch, per operation, with the branch member left for the
/// caller to fill in.
fn branch_requests() -> Vec<(&'static str, &'static str, Value)> {
    vec![
        (
            "CreateCommit",
            "branchName",
            json!({
                "repositoryName": "repo-for-branch-names",
                "putFiles": [{"filePath": "a.txt", "fileContent": "eA=="}],
            }),
        ),
        (
            "DeleteFile",
            "branchName",
            json!({
                "repositoryName": "repo-for-branch-names",
                "filePath": "README.md",
                "parentCommitId": FULL_OBJECT_ID,
            }),
        ),
        (
            "PutFile",
            "branchName",
            json!({
                "repositoryName": "repo-for-branch-names",
                "filePath": "a.txt",
                "fileContent": "eA==",
            }),
        ),
        (
            "CreateBranch",
            "branchName",
            json!({
                "repositoryName": "repo-for-branch-names",
                "commitId": FULL_OBJECT_ID,
            }),
        ),
        (
            "DeleteBranch",
            "branchName",
            json!({"repositoryName": "repo-for-branch-names"}),
        ),
        (
            "GetBranch",
            "branchName",
            json!({"repositoryName": "repo-for-branch-names"}),
        ),
        (
            "UpdateDefaultBranch",
            "defaultBranchName",
            json!({"repositoryName": "repo-for-branch-names"}),
        ),
    ]
}

/// An EMPTY branch name is refused with the sentence CodeCommit words that case
/// with, on every operation that names a branch — and a branch name that is present
/// and merely not a valid reference name keeps the other sentence, so the empty case
/// is not a generic constraint rejection in disguise.
#[tokio::test]
async fn empty_branch_name_message_is_the_constant() {
    let router = emulator();
    ok(
        &router,
        "CreateRepository",
        &json!({"repositoryName": "repo-for-branch-names"}),
    )
    .await;
    const EXPECTED_EMPTY: &str = "Branch name cannot be empty";
    // `BranchName` is `len 1..256`, so a 257-character name is the other way a
    // branch member fails its modelled constraint.
    const EXPECTED_INVALID: &str = "The specified reference name is not valid.";

    for (operation, member, body) in branch_requests() {
        for (label, sent, expected) in [
            ("an empty branch name", String::new(), EXPECTED_EMPTY),
            (
                "a branch name one character past the maximum",
                "z".repeat(257),
                EXPECTED_INVALID,
            ),
        ] {
            let mut body = body.clone();
            body[member] = json!(sent);
            let (code, message) =
                error_with_message(&router, operation, &body, CLIENT_ERROR_STATUS).await;
            assert_eq!(
                code, "InvalidBranchNameException",
                "case ({operation}, {label}): expected code InvalidBranchNameException, \
                 got {code:?}"
            );
            assert_eq!(
                message, expected,
                "case ({operation}, {label}): expected message {expected:?}, got {message:?}"
            );
            assert!(
                !message.contains("member"),
                "case ({operation}, {label}): the message must not name the emulator's \
                 validator or the member it rejected, got {message:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Cause 7 — InvalidReactionValueException
// ---------------------------------------------------------------------------

/// The message is a TEMPLATE: the literal `Unrecognized Reaction Value: ` and then
/// the value the request sent, as it sent it.
#[tokio::test]
async fn unrecognized_reaction_value_message_names_the_value() {
    let f = fixture("repo-for-reaction-messages").await;
    let posted = ok(
        &f.router,
        "PostCommentForComparedCommit",
        &json!({
            "repositoryName": f.repository,
            "beforeCommitId": f.before_commit,
            "afterCommitId": f.after_commit,
            "content": "a comment to react to",
        }),
    )
    .await;
    let comment_id = string_at(&posted, "/comment/commentId");

    // Values no row of the service's reaction table holds: a shortcode that is not
    // one, a shortcode missing a delimiter, a bare word, a name the table does not
    // hold, a codepoint outside it, emoji outside the table, and a repeated emoji.
    // `""` and `"none"` are NOT here: those are the documented
    // removal values, not unrecognized ones.
    let values = [
        "not-a-real-emoji-shortcode",
        ":shipit:",
        ":THUMBSUP",
        "thumbs-up",
        "SHRUG",
        "U+1F44F",
        "\u{1f44f}",
        "\u{1f680}\u{1f680}",
    ];
    for value in values {
        let (code, message) = error_with_message(
            &f.router,
            "PutCommentReaction",
            &json!({"commentId": comment_id, "reactionValue": value}),
            CLIENT_ERROR_STATUS,
        )
        .await;
        assert_eq!(
            code, "InvalidReactionValueException",
            "case {value:?}: expected code InvalidReactionValueException, got {code:?}"
        );
        let expected = format!("Unrecognized Reaction Value: {value}");
        assert_eq!(
            message, expected,
            "case {value:?}: expected message {expected:?}, got {message:?}"
        );
    }

    // The refusals left the comment without reactions: nothing was recorded for a
    // value the service does not hold, so the count map a comment always carries is
    // empty.
    let read_back = ok(&f.router, "GetComment", &json!({"commentId": comment_id})).await;
    let counts = &read_back["comment"]["reactionCounts"];
    assert!(
        counts.as_object().is_some_and(serde_json::Map::is_empty),
        "a refused reaction must leave the comment's reactions empty, got {read_back}"
    );
}

// ---------------------------------------------------------------------------
// Cause 8 — InvalidResourceArnException
// ---------------------------------------------------------------------------

/// The message is a CONSTANT: the malformed value the caller sent is not echoed
/// back into it, on every tagging operation.
#[tokio::test]
async fn invalid_resource_arn_message_is_a_constant() {
    let router = emulator();
    const EXPECTED: &str = "Inputted resource arn is not formatted correctly.";

    // Rows differ along WHERE the value stops being a CodeCommit resource ARN: not
    // an ARN at all, too few fields, another vendor, the vendor in the wrong case,
    // an account that is not 12 digits, an empty region, an empty resource.
    let arns = [
        "not-a-valid-arn",
        "arn",
        "arn:aws:s3:::my-bucket",
        "arn:aws:CodeCommit:us-east-1:123456789012:a-repository",
        "arn:aws:codecommit:us-east-1:12345:a-repository",
        "arn:aws:codecommit::123456789012:a-repository",
        "arn:aws:codecommit:us-east-1:123456789012:",
        "https://example.com/a-repository",
    ];
    for arn in arns {
        let requests: Vec<(&str, Value)> = vec![
            (
                "TagResource",
                json!({"resourceArn": arn, "tags": {"owner": "team"}}),
            ),
            (
                "UntagResource",
                json!({"resourceArn": arn, "tagKeys": ["owner"]}),
            ),
            ("ListTagsForResource", json!({"resourceArn": arn})),
        ];
        for (operation, body) in requests {
            let (code, message) =
                error_with_message(&router, operation, &body, CLIENT_ERROR_STATUS).await;
            assert_eq!(
                code, "InvalidResourceArnException",
                "case ({operation}, {arn:?}): expected code InvalidResourceArnException, \
                 got {code:?}"
            );
            assert_eq!(
                message, EXPECTED,
                "case ({operation}, {arn:?}): expected message {EXPECTED:?}, got {message:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Cause 9 — CommentDeletedException's sentence separation
// ---------------------------------------------------------------------------

/// The message's sentences are separated by TWO spaces, which is part of the
/// recorded constant: the read paths carry the two sentences, and the calls that
/// ask to edit or delete the dead comment lead them with a third.
///
/// `comment_deleted_refusals.rs` spans the comments, operations and principals this
/// refusal generalises over; what is asserted here is the whitespace itself, which a
/// text comparison alone can hide.
#[tokio::test]
async fn comment_deleted_message_separates_its_sentences_with_two_spaces() {
    let f = fixture("repo-for-deleted-comment-text").await;
    let posted = ok(
        &f.router,
        "PostCommentForComparedCommit",
        &json!({
            "repositoryName": f.repository,
            "beforeCommitId": f.before_commit,
            "afterCommitId": f.after_commit,
            "content": "a comment whose content goes",
        }),
    )
    .await;
    let comment_id = string_at(&posted, "/comment/commentId");
    ok(
        &f.router,
        "DeleteCommentContent",
        &json!({"commentId": comment_id}),
    )
    .await;

    let already_deleted = "The specified comment has already been marked as deleted.";
    let verify = "Verify that you specified a valid comment ID, and then try again.";
    let requests: Vec<(&str, Value, Vec<&str>)> = vec![
        (
            "GetComment",
            json!({"commentId": comment_id}),
            vec![already_deleted, verify],
        ),
        (
            "UpdateComment",
            json!({"commentId": comment_id, "content": "an edit"}),
            vec!["You cannot delete this comment.", already_deleted, verify],
        ),
        (
            "PutCommentReaction",
            json!({"commentId": comment_id, "reactionValue": ":thumbsup:"}),
            vec![already_deleted, verify],
        ),
        (
            "DeleteCommentContent",
            json!({"commentId": comment_id}),
            vec!["You cannot delete this comment.", already_deleted, verify],
        ),
    ];
    for (operation, body, sentences) in requests {
        let (code, message) =
            error_with_message(&f.router, operation, &body, CLIENT_ERROR_STATUS).await;
        assert_eq!(
            code, "CommentDeletedException",
            "case {operation}: expected code CommentDeletedException, got {code:?}"
        );
        let expected = sentences.join("  ");
        assert_eq!(
            message, expected,
            "case {operation}: expected message {expected:?}, got {message:?}"
        );
        // Stated again as the property, so a single space between the sentences
        // fails here with the reason rather than as a byte mismatch.
        assert_eq!(
            message.split("  ").collect::<Vec<_>>(),
            sentences,
            "case {operation}: the sentences must be separated by two spaces, got {message:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Cause 10 — CommentContentRequiredException
// ---------------------------------------------------------------------------

/// The message is a CONSTANT stating the failure and then the rule, whether the
/// content member arrived empty or did not arrive at all, and on both of the
/// operations that save comment content today.
///
/// `PostCommentForPullRequest` and `PostCommentReply` raise the same code and are
/// still 501 stubs (both carry the scaffold header), so they cannot be asserted
/// here; `crate::service` words the code the same way for them when the request
/// boundary refuses a missing `content`.
#[tokio::test]
async fn comment_content_required_message_is_a_constant() {
    let f = fixture("repo-for-empty-comment-content").await;
    const EXPECTED: &str = "The comment could not be saved because no content was provided. \
                            Comments cannot be empty.";
    let posted = ok(
        &f.router,
        "PostCommentForComparedCommit",
        &json!({
            "repositoryName": f.repository,
            "beforeCommitId": f.before_commit,
            "afterCommitId": f.after_commit,
            "content": "the comment an edit will try to empty",
        }),
    )
    .await;
    let comment_id = string_at(&posted, "/comment/commentId");

    // The rows differ along how the content is absent — sent empty, or not sent at
    // all — and along the operation that refuses it.
    let requests: Vec<(&str, Value)> = vec![
        (
            "PostCommentForComparedCommit (empty content)",
            json!({
                "repositoryName": f.repository,
                "beforeCommitId": f.before_commit,
                "afterCommitId": f.after_commit,
                "content": "",
            }),
        ),
        (
            "PostCommentForComparedCommit (no content member)",
            json!({
                "repositoryName": f.repository,
                "beforeCommitId": f.before_commit,
                "afterCommitId": f.after_commit,
            }),
        ),
        (
            "UpdateComment (empty content)",
            json!({"commentId": comment_id, "content": ""}),
        ),
        (
            "UpdateComment (no content member)",
            json!({"commentId": comment_id}),
        ),
    ];
    for (label, body) in requests {
        let operation = label
            .split_once(' ')
            .map_or(label, |(operation, _)| operation);
        let (code, message) =
            error_with_message(&f.router, operation, &body, CLIENT_ERROR_STATUS).await;
        assert_eq!(
            code, "CommentContentRequiredException",
            "case {label}: expected code CommentContentRequiredException, got {code:?}"
        );
        assert_eq!(
            message, EXPECTED,
            "case {label}: expected message {EXPECTED:?}, got {message:?}"
        );
    }

    // The refusals left the posted comment's content as it was.
    let read_back = ok(&f.router, "GetComment", &json!({"commentId": comment_id})).await;
    assert_eq!(
        string_at(&read_back, "/comment/content"),
        "the comment an edit will try to empty",
        "a refused edit must leave the stored comment alone, got {read_back}"
    );
}

// ---------------------------------------------------------------------------
// InvalidRepositoryTriggerDestinationArnException
// ---------------------------------------------------------------------------

/// The message is a CONSTANT: it names neither the malformed value the caller sent,
/// nor the trigger it belongs to, nor the repository the configuration was for.
///
/// The rows differ along WHERE the destination stops being an ARN a trigger can
/// target — not an ARN at all, too few fields, another vendor's ARN, the vendor in
/// the wrong case, an account that is not 12 digits, an empty region, an empty
/// resource, a URL — and each one is refused with the same text. `crate::state`'s
/// trigger validator is shared with `TestRepositoryTriggers`, which is still a 501
/// stub (`src/ops/test_repository_triggers.rs` carries the scaffold header), so the
/// put path is the only operation that can be asserted here.
#[tokio::test]
async fn invalid_repository_trigger_destination_arn_message_is_a_constant() {
    let router = emulator();
    const EXPECTED: &str = "Repository trigger destination arn is incorrectly formatted";
    let repository = "repo-for-trigger-destinations";
    ok(
        &router,
        "CreateRepository",
        &json!({ "repositoryName": repository }),
    )
    .await;

    let account = engine::emulator_account_id();
    let destinations = [
        "not-a-valid-sns-arn".to_string(),
        "arn".to_string(),
        "arn:aws:sns".to_string(),
        format!("arn:aws:s3:::{account}-bucket"),
        format!("arn:aws:SNS:{REGION}:{account}:releases"),
        format!("arn:aws:sns:{REGION}:12345:releases"),
        format!("arn:aws:sns::{account}:releases"),
        format!("arn:aws:sns:{REGION}:{account}:"),
        format!("https://sns.{REGION}.amazonaws.com/releases"),
    ];
    for destination in &destinations {
        let (code, message) = error_with_message(
            &router,
            "PutRepositoryTriggers",
            &json!({
                "repositoryName": repository,
                "triggers": [{
                    "name": "a-trigger",
                    "destinationArn": destination,
                    "events": ["all"],
                    "branches": [],
                }],
            }),
            CLIENT_ERROR_STATUS,
        )
        .await;
        assert_eq!(
            code, "InvalidRepositoryTriggerDestinationArnException",
            "case {destination:?}: expected code \
             InvalidRepositoryTriggerDestinationArnException, got {code:?}"
        );
        assert_eq!(
            message, EXPECTED,
            "case {destination:?}: expected message {EXPECTED:?}, got {message:?}"
        );

        // Nothing was stored for the refused configuration.
        let held = ok(
            &router,
            "GetRepositoryTriggers",
            &json!({ "repositoryName": repository }),
        )
        .await;
        assert_eq!(
            held.get("triggers"),
            Some(&json!([])),
            "case {destination:?}: a refused put must leave the configuration empty, got {held}"
        );
    }

    // And a trigger of the same shape whose destination IS an ARN of a service a
    // trigger can target still applies afterwards.
    let good = json!([{
        "name": "a-trigger",
        "destinationArn": format!("arn:aws:sns:{REGION}:{account}:releases"),
        "events": ["all"],
        "branches": [],
    }]);
    let written = ok(
        &router,
        "PutRepositoryTriggers",
        &json!({ "repositoryName": repository, "triggers": good }),
    )
    .await;
    assert!(
        !string_at(&written, "/configurationId").is_empty(),
        "a put that applies must mint a configurationId, got {written}"
    );
    let held = ok(
        &router,
        "GetRepositoryTriggers",
        &json!({ "repositoryName": repository }),
    )
    .await;
    assert_eq!(
        held.get("triggers"),
        Some(&good),
        "expected the applied configuration to be read back, got {held}"
    );
}

// ---------------------------------------------------------------------------
// The comment refusals: IdempotencyParameterMismatchException,
// InvalidFilePositionException, InvalidPathException,
// InvalidFileLocationException, InvalidRelativeFileVersionEnumException and
// CommentContentSizeLimitExceededException
// ---------------------------------------------------------------------------

/// `body` with every member `overrides` names replaced, and every member it maps to
/// `null` removed — how a row states the ONE thing it changes about a request.
fn with(mut body: Value, overrides: Value) -> Value {
    let Value::Object(overrides) = overrides else {
        return body;
    };
    let members = body
        .as_object_mut()
        .expect("a request body is a JSON object");
    for (member, value) in overrides {
        if value.is_null() {
            members.remove(&member);
        } else {
            members.insert(member, value);
        }
    }
    body
}

/// A `PostCommentForComparedCommit` body on the fixture's comparison.
fn compared_comment(f: &Fixture, overrides: Value) -> Value {
    with(
        json!({
            "repositoryName": f.repository,
            "beforeCommitId": f.before_commit,
            "afterCommitId": f.after_commit,
            "content": "a comment",
        }),
        overrides,
    )
}

/// A `PostCommentForPullRequest` body on an open pull request's comparison.
fn pull_request_comment(pr: &PullRequest, overrides: Value) -> Value {
    with(
        json!({
            "pullRequestId": pr.pull_request_id,
            "repositoryName": pr.repository,
            "beforeCommitId": pr.before_commit,
            "afterCommitId": pr.after_commit,
            "content": "a comment",
        }),
        overrides,
    )
}

/// Reusing a client request token with changed parameters is refused with the code
/// and the status alone: CodeCommit serializes NO message member for it, so the
/// ABSENCE of a message is part of that refusal.
///
/// The rows differ along what the rule generalises over: which operation carries the
/// `@idempotencyToken` (every one of the four that does) and WHICH parameter of the
/// first request the second one changed (its content, its location, its title).
#[tokio::test]
async fn an_idempotency_parameter_mismatch_carries_no_message() {
    let f = fixture("token-mismatch-messages").await;
    let pr = pull_request_on(&f.router, "token-mismatch-pull-request").await;

    // A comment to reply to, posted outside the rows so a reply row's own token is
    // the only token in play for it.
    let posted = ok(
        &f.router,
        "PostCommentForComparedCommit",
        &compared_comment(&f, json!({ "content": "the comment the replies answer" })),
    )
    .await;
    let parent = string_at(&posted, "/comment/commentId");

    let rows: Vec<(&str, &str, Value, Value)> = vec![
        (
            "a compared-commit comment with changed content",
            "PostCommentForComparedCommit",
            compared_comment(&f, json!({})),
            json!({ "content": "a second comment" }),
        ),
        (
            "a compared-commit comment with a changed location",
            "PostCommentForComparedCommit",
            compared_comment(&f, json!({})),
            json!({
                "location": {
                    "filePath": PATH,
                    "filePosition": 1,
                    "relativeFileVersion": "AFTER",
                },
            }),
        ),
        (
            "a pull request comment with changed content",
            "PostCommentForPullRequest",
            pull_request_comment(&pr, json!({})),
            json!({ "content": "a second comment" }),
        ),
        (
            "a reply with changed content",
            "PostCommentReply",
            json!({ "inReplyTo": parent, "content": "the first reply" }),
            json!({ "content": "a second reply" }),
        ),
        (
            "a pull request with a changed title",
            "CreatePullRequest",
            json!({
                "title": "a change to review",
                "targets": [{
                    "repositoryName": pr.repository,
                    "sourceReference": "feature",
                    "destinationReference": "main",
                }],
            }),
            json!({ "title": "a different title" }),
        ),
    ];

    for (case, operation, first, change) in rows {
        let first = with(first, json!({ "clientRequestToken": fresh_client_request_token() }));
        ok(&f.router, operation, &first).await;
        let (code, message) = error_with_message(
            &f.router,
            operation,
            &with(first.clone(), change),
            CLIENT_ERROR_STATUS,
        )
        .await;
        assert_eq!(
            code, "IdempotencyParameterMismatchException",
            "case {case:?}: expected IdempotencyParameterMismatchException, got {code:?}"
        );
        assert_eq!(
            message, "",
            "case {case:?}: this refusal carries no message, got {message:?}"
        );
        // The refusal consumed nothing: the token still answers the request it was
        // accepted for.
        ok(&f.router, operation, &first).await;
    }
}

/// A `filePosition` below the first line is refused with the FIXED line-number text:
/// `Location.filePosition` is a 1-based line number
/// (`smithy_query(query="shape",shape="Location")` — member `filePosition`, "The
/// position of a change in a compared file, in line number format"), and the message
/// states that rule without naming anything from the request.
///
/// The rows differ along the position (the line before the first, zero, a negative
/// line far from it, and the smallest value a `long` holds), the side of the
/// comparison the location names, and the operation that places the comment.
#[tokio::test]
async fn an_invalid_file_position_message_is_the_fixed_line_number_text() {
    const EXPECTED: &str = "The line number is not valid. Either the line number was 0 or a \
                            negative number. Check to make sure you've included the positive \
                            value of the line number in the file for the comment, and try again.";
    let f = fixture("file-position-messages").await;
    let pr = pull_request_on(&f.router, "file-position-pull-request").await;

    for position in [0_i64, -1, -4096, i64::MIN] {
        for side in ["AFTER", "BEFORE"] {
            let location = json!({
                "filePath": PATH,
                "filePosition": position,
                "relativeFileVersion": side,
            });
            let requests = [
                (
                    "PostCommentForComparedCommit",
                    compared_comment(&f, json!({ "location": location })),
                ),
                (
                    "PostCommentForPullRequest",
                    pull_request_comment(&pr, json!({ "location": location })),
                ),
            ];
            for (operation, body) in requests {
                let case = format!("{operation} at position {position} of the {side} version");
                let (code, message) =
                    error_with_message(&f.router, operation, &body, CLIENT_ERROR_STATUS).await;
                assert_eq!(
                    code, "InvalidFilePositionException",
                    "case {case:?}: expected InvalidFilePositionException, got {code:?}"
                );
                assert_eq!(
                    message, EXPECTED,
                    "case {case:?}: expected message {EXPECTED:?}, got {message:?}"
                );
            }
        }
    }
}

/// `InvalidPathException`'s message NAMES THE DEFECT: a fixed opening sentence, a
/// fixed sentence saying which path rule was broken, and a fixed pointer at the
/// service's published path limits — with no part of the offending path in it.
///
/// The rows differ along the way a path fails to address a file in a tree: it climbs
/// out of the tree, it walks the tree from inside it, it names the current
/// directory, or it carries an empty directory name (in the middle and at the end).
#[tokio::test]
async fn an_invalid_path_message_names_the_defect_and_the_limits() {
    const OPENING: &str = "The path is not valid.";
    const LIMITS: &str = "For more information about limitations for file paths, see Limits in \
                          the AWS CodeCommit User Guide.";
    const RELATIVE: &str = "The path is a relative path.";
    const EMPTY_DIRECTORY: &str = "The path contains an empty directory name.";
    let f = fixture("invalid-path-messages").await;
    let pr = pull_request_on(&f.router, "invalid-path-pull-request").await;

    let rows: [(&str, &str); 6] = [
        ("../docs/guide/intro.txt", RELATIVE),
        ("docs/../intro.txt", RELATIVE),
        ("./intro.txt", RELATIVE),
        ("docs/./guide/intro.txt", RELATIVE),
        ("docs//guide/intro.txt", EMPTY_DIRECTORY),
        ("docs/guide//", EMPTY_DIRECTORY),
    ];
    for (file_path, defect) in rows {
        let expected = format!("{OPENING} {defect} {LIMITS}");
        let location = json!({
            "filePath": file_path,
            "filePosition": 1,
            "relativeFileVersion": "AFTER",
        });
        let requests = [
            (
                "PostCommentForComparedCommit",
                compared_comment(&f, json!({ "location": location })),
            ),
            (
                "PostCommentForPullRequest",
                pull_request_comment(&pr, json!({ "location": location })),
            ),
        ];
        for (operation, body) in requests {
            let case = format!("{operation} at {file_path:?}");
            let (code, message) =
                error_with_message(&f.router, operation, &body, CLIENT_ERROR_STATUS).await;
            assert_eq!(
                code, "InvalidPathException",
                "case {case:?}: expected InvalidPathException, got {code:?}"
            );
            assert_eq!(
                message, expected,
                "case {case:?}: expected message {expected:?}, got {message:?}"
            );
        }
    }
}

/// A comment anchors either to a whole FILE (a path alone) or to a LINE, and a line
/// is named by BOTH halves of the anchor — the position and the side of the
/// comparison it is a line of. Either half on its own is refused with
/// `InvalidFileLocationException` naming the member that was not provided, and no
/// comment is created.
///
/// The rows differ along which half is missing, whether the half that IS there would
/// have been valid on its own (a position inside the file and one past its last
/// line, each side of the comparison), and the operation that places the comment.
#[tokio::test]
async fn a_half_specified_comment_location_is_refused_and_stores_nothing() {
    const NO_SIDE: &str = "RelativeFileVersion is not provided";
    const NO_POSITION: &str = "FilePosition is not provided";
    let f = fixture("half-anchored-locations").await;
    let pr = pull_request_on(&f.router, "half-anchored-pull-request").await;

    let rows: [(&str, Value, &str); 5] = [
        (
            "a position with no side",
            json!({ "filePath": PATH, "filePosition": 1 }),
            NO_SIDE,
        ),
        (
            "a position past the last line with no side",
            json!({ "filePath": PATH, "filePosition": 9_999 }),
            NO_SIDE,
        ),
        (
            "the after side with no position",
            json!({ "filePath": PATH, "relativeFileVersion": "AFTER" }),
            NO_POSITION,
        ),
        (
            "the before side with no position",
            json!({ "filePath": PATH, "relativeFileVersion": "BEFORE" }),
            NO_POSITION,
        ),
        (
            "a leading separator, a position and no side",
            json!({ "filePath": format!("/{PATH}"), "filePosition": 2 }),
            NO_SIDE,
        ),
    ];
    for (label, location, expected) in rows {
        let requests = [
            (
                "PostCommentForComparedCommit",
                compared_comment(&f, json!({ "location": location })),
            ),
            (
                "PostCommentForPullRequest",
                pull_request_comment(&pr, json!({ "location": location })),
            ),
        ];
        for (operation, body) in requests {
            let case = format!("{operation} with {label}");
            let (code, message) =
                error_with_message(&f.router, operation, &body, CLIENT_ERROR_STATUS).await;
            assert_eq!(
                code, "InvalidFileLocationException",
                "case {case:?}: expected InvalidFileLocationException, got {code:?}"
            );
            assert_eq!(
                message, expected,
                "case {case:?}: expected message {expected:?}, got {message:?}"
            );
        }
    }

    // Nothing any of those requests asked for was written: the comparison carries no
    // comment at all.
    let held = ok(
        &f.router,
        "GetCommentsForComparedCommit",
        &json!({
            "repositoryName": f.repository,
            "beforeCommitId": f.before_commit,
            "afterCommitId": f.after_commit,
        }),
    )
    .await;
    assert_eq!(
        held.get("commentsForComparedCommitData"),
        Some(&json!([])),
        "a refused location must leave the comparison with no comment, got {held}"
    );

    // And the two locations that ARE anchors are accepted on the same comparison: a
    // whole file, and a line named by both halves.
    for (case, location) in [
        ("a whole file", json!({ "filePath": PATH })),
        (
            "a line named by both halves",
            json!({
                "filePath": PATH,
                "filePosition": 1,
                "relativeFileVersion": "AFTER",
            }),
        ),
    ] {
        let posted = ok(
            &f.router,
            "PostCommentForComparedCommit",
            &compared_comment(&f, json!({ "location": location })),
        )
        .await;
        assert_eq!(
            posted.pointer("/location"),
            Some(&location),
            "case {case:?}: the accepted location must be reported back, got {posted}"
        );
    }
}

/// A comment on a file the named PORTION of the comparison does not hold, while the
/// other portion does, is refused with `InvalidRelativeFileVersionEnumException`:
/// the file exists in the comparison, just not on the side the caller asked to
/// comment on, so the remedy is the other side — which is what the message says, its
/// one variable part being that side's name. A path NEITHER portion holds is
/// `PathDoesNotExistException` instead.
///
/// The rows differ along which side lacks the file and why (a file added between the
/// two commits, and one deleted between them), and the control rows post the same
/// locations on the side that does hold the file.
#[tokio::test]
async fn a_comment_on_a_file_missing_from_the_named_side_is_refused_for_that_side() {
    let router = emulator();
    let repository = "comments-across-one-sided-files";
    ok(
        &router,
        "CreateRepository",
        &json!({ "repositoryName": repository }),
    )
    .await;
    let put = |path: &'static str, parent: Option<String>| {
        let router = &router;
        let mut body = json!({
            "repositoryName": repository,
            "branchName": "main",
            "filePath": path,
            "fileContent": base64(b"one\ntwo\n"),
            "commitMessage": format!("write {path}"),
        });
        if let Some(parent) = parent {
            body["parentCommitId"] = json!(parent);
        }
        async move { string_at(&ok(router, "PutFile", &body).await, "/commitId") }
    };
    // The before side holds `kept.txt` and `legacy.css`; the after side holds
    // `kept.txt` and `style.css`, with `legacy.css` deleted between the two.
    let first = put("kept.txt", None).await;
    let before = put("legacy.css", Some(first)).await;
    let added = put("web/style.css", Some(before.clone())).await;
    let deleted = ok(
        &router,
        "DeleteFile",
        &json!({
            "repositoryName": repository,
            "branchName": "main",
            "filePath": "legacy.css",
            "parentCommitId": added,
        }),
    )
    .await;
    let after = string_at(&deleted, "/commitId");

    let post = |file_path: &str, side: &str| {
        json!({
            "repositoryName": repository,
            "beforeCommitId": before,
            "afterCommitId": after,
            "content": "a comment",
            "location": {
                "filePath": file_path,
                "filePosition": 1,
                "relativeFileVersion": side,
            },
        })
    };
    let wrong_side = |side: &str| {
        format!(
            "You can only comment on existing files. You attempted to comment on a file that \
             does not exist in this portion of the comparison between commits: {side}. Either \
             the file has been deleted, or has not been added to the repository at the time of \
             this commit. Try commenting on the file in the other commit portion of the \
             comparison."
        )
    };

    let rows: [(&str, &str, &str, &str, String); 3] = [
        (
            "a file added between the commits, commented on the before side",
            "web/style.css",
            "BEFORE",
            "InvalidRelativeFileVersionEnumException",
            wrong_side("BEFORE"),
        ),
        (
            "a file deleted between the commits, commented on the after side",
            "legacy.css",
            "AFTER",
            "InvalidRelativeFileVersionEnumException",
            wrong_side("AFTER"),
        ),
        (
            "a path neither side of the comparison holds",
            "docs/absent.md",
            "AFTER",
            "PathDoesNotExistException",
            "the path docs/absent.md does not exist in the AFTER version of the comparison"
                .to_string(),
        ),
    ];
    for (case, file_path, side, expected_code, expected_message) in rows {
        let (code, message) = error_with_message(
            &router,
            "PostCommentForComparedCommit",
            &post(file_path, side),
            CLIENT_ERROR_STATUS,
        )
        .await;
        assert_eq!(
            code, expected_code,
            "case {case:?}: expected {expected_code}, got {code:?} with {message:?}"
        );
        assert_eq!(
            message, expected_message,
            "case {case:?}: expected message {expected_message:?}, got {message:?}"
        );
    }

    // THE CONTROLS: the same file on the side that does hold it is accepted, so each
    // refusal above is attributable to the side it named.
    for (case, file_path, side) in [
        ("the added file on the after side", "web/style.css", "AFTER"),
        ("the deleted file on the before side", "legacy.css", "BEFORE"),
        ("a file both sides hold", "kept.txt", "BEFORE"),
    ] {
        let posted = ok(
            &router,
            "PostCommentForComparedCommit",
            &post(file_path, side),
        )
        .await;
        assert_eq!(
            posted.pointer("/location/filePath"),
            Some(&json!(file_path)),
            "case {case:?}: the comment must be posted at the file it named, got {posted}"
        );
    }
}

/// `CommentContentSizeLimitExceededException` states the LIMIT: the fixed text the
/// service words that code with, followed by the number of characters a comment may
/// carry — which is what a caller parses the limit out of.
///
/// The rows differ along how far past the limit the content is and what it is made
/// of (one character over, far over, and one character over in characters that are
/// several bytes each — the limit counts characters), and along the operation that
/// saves comment content.
#[tokio::test]
async fn the_comment_size_limit_message_states_the_limit() {
    const EXPECTED: &str = "The size limit for comments is 10240";
    let f = fixture("comment-size-messages").await;
    let pr = pull_request_on(&f.router, "comment-size-pull-request").await;
    let posted = ok(
        &f.router,
        "PostCommentForComparedCommit",
        &compared_comment(&f, json!({ "content": "the comment an edit will try to grow" })),
    )
    .await;
    let comment_id = string_at(&posted, "/comment/commentId");

    let contents: [(&str, String); 3] = [
        (
            "one character past the limit",
            "x".repeat(MAX_COMMENT_CHARACTERS + 1),
        ),
        (
            "far past the limit",
            "x".repeat(MAX_COMMENT_CHARACTERS * 2),
        ),
        (
            "one multi-byte character past the limit",
            "é".repeat(MAX_COMMENT_CHARACTERS + 1),
        ),
    ];
    for (label, content) in contents {
        let requests = [
            (
                "PostCommentForComparedCommit",
                compared_comment(&f, json!({ "content": content })),
            ),
            (
                "PostCommentForPullRequest",
                pull_request_comment(&pr, json!({ "content": content })),
            ),
            (
                "PostCommentReply",
                json!({ "inReplyTo": comment_id, "content": content }),
            ),
            (
                "UpdateComment",
                json!({ "commentId": comment_id, "content": content }),
            ),
        ];
        for (operation, body) in requests {
            let case = format!("{operation} with content {label}");
            let (code, message) =
                error_with_message(&f.router, operation, &body, CLIENT_ERROR_STATUS).await;
            assert_eq!(
                code, "CommentContentSizeLimitExceededException",
                "case {case:?}: expected CommentContentSizeLimitExceededException, got {code:?}"
            );
            assert_eq!(
                message, EXPECTED,
                "case {case:?}: expected message {EXPECTED:?}, got {message:?}"
            );
        }
    }

    // Content AT the limit is saved: the refusals above are the bound's far side.
    let at_limit = "x".repeat(MAX_COMMENT_CHARACTERS);
    let accepted = ok(
        &f.router,
        "UpdateComment",
        &json!({ "commentId": comment_id, "content": at_limit }),
    )
    .await;
    assert_eq!(
        accepted.pointer("/comment/content"),
        Some(&json!(at_limit)),
        "content at the limit must be saved, got {accepted}"
    );
}

/// `FileContentSizeLimitExceededException` carries the GUIDANCE text: the limit, the
/// two faces it applies to, and the git client as the way to add content past it —
/// which is the only place the service tells a caller either of those.
///
/// The rows differ along the operation that carries the `fileContent` the model
/// bounds (`smithy_query(query="shape",shape="FileContent")` — `@length max
/// 6291456`) and how far past the bound the content is; the control writes content
/// AT the bound, which is served back as the file it wrote.
#[tokio::test]
async fn the_file_content_size_limit_message_names_the_git_client() {
    const EXPECTED: &str = "The maximum file size for adding a file from the AWS CodeCommit \
                            console or using the PutFile API is 6 MB. For files larger than 6 \
                            MB but smaller than 2 GB, use a Git client.";
    /// `FileContent`'s modelled maximum, in bytes.
    const LIMIT: usize = 6 * 1024 * 1024;
    let router = emulator();
    let repository = "file-content-size-messages";
    ok(
        &router,
        "CreateRepository",
        &json!({ "repositoryName": repository }),
    )
    .await;

    let rows: [(&str, &str, Value); 2] = [
        (
            "PutFile one byte past the limit",
            "PutFile",
            json!({
                "repositoryName": repository,
                "branchName": "main",
                "filePath": "big.bin",
                "fileContent": base64(&vec![b'x'; LIMIT + 1]),
            }),
        ),
        (
            "CreateCommit with a file one byte past the limit",
            "CreateCommit",
            json!({
                "repositoryName": repository,
                "branchName": "main",
                "putFiles": [{
                    "filePath": "big.bin",
                    "fileContent": base64(&vec![b'x'; LIMIT + 1]),
                }],
            }),
        ),
    ];
    for (case, operation, body) in rows {
        let (code, message) =
            error_with_message(&router, operation, &body, CLIENT_ERROR_STATUS).await;
        assert_eq!(
            code, "FileContentSizeLimitExceededException",
            "case {case:?}: expected FileContentSizeLimitExceededException, got {code:?}"
        );
        assert_eq!(
            message, EXPECTED,
            "case {case:?}: expected message {EXPECTED:?}, got {message:?}"
        );
    }

    // THE CONTROL: content AT the limit is written, and a read serves it back — the
    // bound is where the refusal starts, not where reads stop.
    let written = ok(
        &router,
        "PutFile",
        &json!({
            "repositoryName": repository,
            "branchName": "main",
            "filePath": "at-the-limit.bin",
            "fileContent": base64(&vec![b'x'; LIMIT]),
        }),
    )
    .await;
    assert!(
        !string_at(&written, "/commitId").is_empty(),
        "content at the limit must be written, got {written}"
    );
    let read_back = ok(
        &router,
        "GetFile",
        &json!({ "repositoryName": repository, "filePath": "at-the-limit.bin" }),
    )
    .await;
    assert_eq!(
        read_back.get("fileSize").and_then(Value::as_i64),
        Some(LIMIT as i64),
        "a file at the limit must be served back whole, got fileSize {:?}",
        read_back.get("fileSize")
    );
}
