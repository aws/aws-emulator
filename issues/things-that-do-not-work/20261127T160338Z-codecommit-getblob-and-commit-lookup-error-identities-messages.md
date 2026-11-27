Issue-Source: trace-sweep-divergence
Issue-Trace: codecommit-get-blob-missing-blob-id-rejected
Issue-Step: task#4
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: codecommit-get-blob-missing-blob-id-rejected codecommit-get-blob-repository-name-refusals codecommit-pull-request-comment-on-unknown-commit-is-refused codecommit-pull-request-comment-token-and-line-refusals codecommit-file-folder-blob-content-operations
Issue-Shape-Id: com.amazonaws.codecommit#CodeCommit_20150413
Issue-Key: codecommit|GetBlob|4xx->4xx|$.message
Issue-Key: codecommit|GetBlob|4xx->4xx|$.__type
Issue-Key: codecommit|PostCommentForPullRequest|4xx->4xx|$.message
Issue-Key: codecommit|ListFileCommitHistory|2xx->2xx|$.revisionDag[*]
Issue-Commit: e76aa5e7b75dc83d136a841bb7500c1a87dd58d0

# CodeCommit GetBlob and commit-lookup error identities, messages and commit dates diverge

> _Note: this issue was identified against the code base at commit `e76aa5e7b75dc83d136a841bb7500c1a87dd58d0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetBlob

## Description
Four replayed divergence clusters on the object/commit lookup path: GetBlob attaches a message AWS does not send, misclassifies a present-but-invalid blob id as a missing one, the commit-not-found refusal uses an invented message template, and ListFileCommitHistory reports a commit date that disagrees with the date the same commit reported earlier in the same fixture. 4 causes.

Cause 1 of 4 — GetBlob's BlobIdRequiredException carries a message AWS does not emit (fixture codecommit-get-blob-missing-blob-id-rejected, step task#4).
AWS: Smithy: GetBlob models BlobIdRequiredException (client, HTTP 400, shape doc "A blob ID is required, but was not specified."); its message member is optional ("Any message associated with the exception"). The recording shows AWS answering HTTP 400 with the error type and NO message member at all for a GetBlob whose blobId is absent.
Emulator: HTTP 400 with the right error type but an extra $.message 'A blob ID is required, but was not specified.' — the shape's documentation text promoted to a wire message.
Why: a client that renders or asserts on the presence of a service message sees text AWS never sends, and message-equality assertions against recorded AWS traffic fail.
Fix: the GetBlob input-validation error construction — do not populate message for this refusal.
Traces: codecommit-get-blob-missing-blob-id-rejected#task#4 (also codecommit-get-blob-repository-name-refusals#task#4).

Cause 2 of 4 — GetBlob classifies a present-but-invalid blob id as a missing one (fixture codecommit-get-blob-missing-blob-id-rejected, step task#5).
AWS: Smithy: GetBlob.blobId targets ObjectId and is @required; the operation models both BlobIdRequiredException and InvalidBlobIdException (client, HTTP 400, doc "The specified blob is not valid"). The recording shows that a blobId which is PRESENT but not a git object id is refused as InvalidBlobIdException with message '<blobId> is not a valid git object ID': fixed text " is not a valid git object ID" preceded by the rejected blobId value from the request (empty in this recording, hence the leading space).
Emulator: HTTP 400 with __type BlobIdRequiredException and message 'A blob ID is required, but was not specified.' — it treats the present value as absent.
Why: a client branching on the error code cannot distinguish "you forgot the blob id" from "the blob id you sent is malformed", so it retries the wrong way; the message also names the wrong problem.
Fix: the GetBlob input validation — validate a present blobId against the git object-id form and raise InvalidBlobIdException, reserving BlobIdRequiredException for an absent member.
Traces: codecommit-get-blob-missing-blob-id-rejected#task#5.

Cause 3 of 4 — PostCommentForPullRequest's CommitDoesNotExistException uses an invented message template (fixture codecommit-pull-request-comment-on-unknown-commit-is-refused, step task#6).
AWS: Smithy: PostCommentForPullRequest models CommitDoesNotExistException (client, HTTP 400, doc "The specified commit does not exist or no commit was specified, and the specified repository has no default branch"). The recorded message is 'Could not find commit <commitId>': fixed text "Could not find commit " plus the unresolvable commit id taken from the request.
Emulator: correct status 400 and error code, but message 'Object missing: <commitId>' — a git-layer phrase rather than the CodeCommit template.
Why: a client surfacing the service message shows wording AWS never sends, and anything matching on the message text fails.
Fix: the PostCommentForPullRequest commit-resolution error message template (the git-object-layer phrase must not leak into the API message).
Traces: codecommit-pull-request-comment-on-unknown-commit-is-refused#task#6 (also codecommit-pull-request-comment-token-and-line-refusals#task#7).

Cause 4 of 4 — ListFileCommitHistory reports a commit author/committer date that disagrees with the date the same commit reported earlier (fixture codecommit-file-folder-blob-content-operations, step task#9).
AWS: Smithy: UserInfo.date is documented "The date when the specified commit was commited, in timestamp format with GMT offset", so the author and committer dates are properties OF the commit — the same commit id must report the same date from every operation. The fixture binds the expected date to the value the same commit reported in an earlier step of the same fixture ({{commit2_date}}).
Emulator: HTTP 200 and revisionDag[0] matches on blobId, path, revisionChildren, commitId, treeId, parents, message and additionalData, but commit.author.date and commit.committer.date carry a different value from the one that commit reported earlier (emitted as '<epoch> +0000'), so the whole revisionDag entry diverges.
Why: a client correlating file history with commit metadata (for ordering, caching, or display) sees the same commit dated differently depending on which operation it asked, which makes history ordering and idempotent comparisons unreliable.
Fix: ListFileCommitHistory's commit projection — read the author/committer date from the stored commit object instead of re-deriving it at response time.
Traces: codecommit-file-folder-blob-content-operations#task#9.

## Evidence
Replay of trace set simparity-wire-aws-v1 (score 88.7%):

- codecommit-get-blob-missing-blob-id-rejected#task#4 (GetBlob, 400->400): $.message unexpected_present (expected None, got 'A blob ID is required, but was not specified.'). 2 affected steps (also codecommit-get-blob-repository-name-refusals#task#4).
- codecommit-get-blob-missing-blob-id-rejected#task#5 (GetBlob, 400->400): $.__type value_mismatch (expected 'InvalidBlobIdException', got 'BlobIdRequiredException'); $.message value_mismatch (expected ' is not a valid git object ID', got 'A blob ID is required, but was not specified.').
- codecommit-pull-request-comment-on-unknown-commit-is-refused#task#6 (PostCommentForPullRequest, 400->400): $.message value_mismatch (expected 'Could not find commit fedcba9876543210fedcba9876543210fedcba98', got 'Object missing: fedcba9876543210fedcba9876543210fedcba98'). 2 affected steps.
- codecommit-file-folder-blob-content-operations#task#9 (ListFileCommitHistory, 200->200): $.revisionDag[0] missing — expected commit.author.date and commit.committer.date to equal the date the same commit (commitId a4f0a034707193e6abed72e5cd8a7a9898a0b072) reported earlier in the fixture; got '1795795114 +0000' for both. All other members of the entry matched.

## Steps to Reproduce
1. Clone the repo and boot the emulator built from it.
2. Replay trace set simparity-wire-aws-v1 (replay_wire_corpus).
3. Representative: fixture codecommit-get-blob-missing-blob-id-rejected, step task#4 — GetBlob with repositoryName set and blobId absent; expect HTTP 400 BlobIdRequiredException with no message member, observe a populated message.
4. Same fixture step task#5 — GetBlob with a present but non-git-object blobId; expect InvalidBlobIdException '<blobId> is not a valid git object ID', observe BlobIdRequiredException.
5. codecommit-pull-request-comment-on-unknown-commit-is-refused#task#6 — PostCommentForPullRequest naming a commit id that does not exist; compare the message against 'Could not find commit <commitId>'.
6. codecommit-file-folder-blob-content-operations#task#9 — ListFileCommitHistory for docs/sub/notes.txt and compare commit.author.date / commit.committer.date with the date the same commit reports from the earlier commit-reading step of the fixture.
