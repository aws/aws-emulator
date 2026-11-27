Issue-Source: trace-sweep-divergence
Issue-Trace: codecommit-comment-client-request-token-required
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: codecommit-comment-client-request-token-required codecommit-compared-commit-comment-token-and-position-refusals codecommit-pull-request-comment-token-and-line-refusals codecommit-compared-commit-comment-location-refusals codecommit-comment-reply-over-content-limit-rejected codecommit-comment-reply-required-member-and-token-refusals codecommit-pull-request-comment-token-and-reply-target-refusals codecommit-compared-commit-comment-absent-commit codecommit-compared-commit-comment-size-and-commit-id-refusals
Issue-Shape-Id: com.amazonaws.codecommit#CodeCommit_20150413
Issue-Key: codecommit|PostCommentForComparedCommit|4xx->2xx|$.__type
Issue-Key: codecommit|PostCommentForComparedCommit|4xx->2xx|$.message
Issue-Key: codecommit|PostCommentForComparedCommit|4xx->2xx|status
Issue-Key: codecommit|PostCommentReply|4xx->2xx|$.__type
Issue-Key: codecommit|PostCommentReply|4xx->2xx|$.message
Issue-Key: codecommit|PostCommentReply|4xx->2xx|status
Issue-Key: codecommit|PostCommentForPullRequest|4xx->2xx|$.__type
Issue-Key: codecommit|PostCommentForPullRequest|4xx->2xx|$.message
Issue-Key: codecommit|PostCommentForPullRequest|4xx->2xx|status
Issue-Key: codecommit|PostCommentForComparedCommit|2xx->4xx|$.afterBlobId
Issue-Key: codecommit|PostCommentForComparedCommit|2xx->4xx|$.afterCommitId
Issue-Key: codecommit|PostCommentForComparedCommit|2xx->4xx|$.beforeBlobId
Issue-Key: codecommit|PostCommentForComparedCommit|2xx->4xx|$.beforeCommitId
Issue-Key: codecommit|PostCommentForComparedCommit|2xx->4xx|$.comment
Issue-Key: codecommit|PostCommentForComparedCommit|2xx->4xx|$.location
Issue-Key: codecommit|PostCommentForComparedCommit|2xx->4xx|$.repositoryName
Issue-Key: codecommit|PostCommentForComparedCommit|2xx->4xx|status
Issue-Key: codecommit|PostCommentForPullRequest|2xx->4xx|$.afterBlobId
Issue-Key: codecommit|PostCommentForPullRequest|2xx->4xx|$.afterCommitId
Issue-Key: codecommit|PostCommentForPullRequest|2xx->4xx|$.beforeBlobId
Issue-Key: codecommit|PostCommentForPullRequest|2xx->4xx|$.beforeCommitId
Issue-Key: codecommit|PostCommentForPullRequest|2xx->4xx|$.comment
Issue-Key: codecommit|PostCommentForPullRequest|2xx->4xx|$.location
Issue-Key: codecommit|PostCommentForPullRequest|2xx->4xx|$.pullRequestId
Issue-Key: codecommit|PostCommentForPullRequest|2xx->4xx|$.repositoryName
Issue-Key: codecommit|PostCommentForPullRequest|2xx->4xx|status
Issue-Key: codecommit|PostCommentForComparedCommit|4xx->4xx|$.__type
Issue-Key: codecommit|PostCommentForComparedCommit|4xx->4xx|$.message
Issue-Key: codecommit|PostCommentReply|4xx->4xx|$.message
Issue-Commit: e76aa5e7b75dc83d136a841bb7500c1a87dd58d0

# CodeCommit comment write validation: required clientRequestToken, file position and refusal templates diverge

> _Note: this issue was identified against the code base at commit `e76aa5e7b75dc83d136a841bb7500c1a87dd58d0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PostCommentForComparedCommit

## Description
Eight replayed divergence clusters, all in the CodeCommit comment write path (PostCommentForComparedCommit / PostCommentReply / PostCommentForPullRequest). The shared fault is input validation: a request AWS refuses is accepted, a request AWS accepts is refused, and three refusal classes answer with the wrong error identity or the wrong message text. 8 causes.

Cause 1 of 8 — PostCommentForComparedCommit accepts a request with no clientRequestToken (fixture codecommit-comment-client-request-token-required, step task#4).
AWS: Smithy: PostCommentForComparedCommit.clientRequestToken targets ClientRequestToken and is marked @idempotencyToken; the operation models error ClientRequestTokenRequiredException (client, HTTP 400), documented "A client request token is required. A client request token is an unique, client-generated idempotency token that, when provided in a request, ensures the request cannot be repeated with a changed parameter." The recorded response is HTTP 400 with __type ClientRequestTokenRequiredException and message "A client request token is required." — that whole sentence is fixed text, it has no variable part.
Emulator: HTTP 200 with a fully populated PostCommentForComparedCommitOutput; no __type and no message are emitted, and the comment is persisted.
Why: a client that omits the idempotency token gets a stored comment instead of the modeled refusal, so its error path is never exercised and the comment it believes was rejected is in fact present in every later GetCommentsForComparedCommit page (this is what corrupts the listings reported separately).
Fix: PostCommentForComparedCommit's input validation — reject a request whose clientRequestToken is absent before any state mutation.
Traces: codecommit-comment-client-request-token-required#task#4 (also codecommit-compared-commit-comment-token-and-position-refusals#task#4).

Cause 2 of 8 — PostCommentReply accepts a request with no clientRequestToken (fixture codecommit-comment-client-request-token-required, step task#6).
AWS: Smithy: PostCommentReply.clientRequestToken is @idempotencyToken and the operation models ClientRequestTokenRequiredException (client, HTTP 400). Recorded: HTTP 400, __type ClientRequestTokenRequiredException, message "A client request token is required." (fixed text).
Emulator: HTTP 200, the reply is created, no __type/message.
Why: same as cause 1 for replies — the refusal never happens and the reply pollutes comment listings.
Fix: PostCommentReply's input validation.
Traces: codecommit-comment-client-request-token-required#task#6.

Cause 3 of 8 — PostCommentForPullRequest accepts a request with no clientRequestToken (fixture codecommit-pull-request-comment-token-and-line-refusals, step task#6).
AWS: Smithy: PostCommentForPullRequest.clientRequestToken is @idempotencyToken; ClientRequestTokenRequiredException (client, HTTP 400) is modeled. Recorded: HTTP 400, __type ClientRequestTokenRequiredException, message "A client request token is required." (fixed text).
Emulator: HTTP 200 and the comment is stored.
Why: same as cause 1 on the pull-request comment path.
Fix: PostCommentForPullRequest's input validation.
Traces: codecommit-pull-request-comment-token-and-line-refusals#task#6.

Cause 4 of 8 — PostCommentForComparedCommit refuses a Location whose filePosition is past the last line of the file (fixture codecommit-compared-commit-comment-token-and-position-refusals, step task#7).
AWS: recorded behavior only — the recording shows HTTP 200 for a comment on notes.txt with location {filePath: notes.txt, filePosition: <a line number beyond the file's last line>, relativeFileVersion: AFTER}: the response echoes repositoryName, beforeCommitId/afterCommitId, beforeBlobId/afterBlobId, the requested location unchanged, and the created comment. The model does have InvalidFilePositionException, but the recording shows it is not raised for a position beyond end-of-file.
Emulator: HTTP 400 and an empty body for the modeled output members (afterBlobId, afterCommitId, beforeBlobId, beforeCommitId, comment, location, repositoryName all absent).
Why: a client annotating a diff at or past the end of a file (a common review gesture) loses the comment entirely and sees a 400 where AWS returns the stored comment.
Fix: PostCommentForComparedCommit's Location validation — a filePosition greater than the file's line count must not be a refusal.
Traces: codecommit-compared-commit-comment-token-and-position-refusals#task#7.

Cause 5 of 8 — PostCommentForPullRequest refuses a Location whose filePosition is past the last line (fixture codecommit-pull-request-comment-token-and-line-refusals, step task#9).
AWS: recorded behavior only — HTTP 200, echoing pullRequestId, repositoryName, the before/after commit and blob ids, the requested location (filePath notes.txt, relativeFileVersion AFTER, the out-of-range filePosition) and the created comment.
Emulator: HTTP 400, none of those members present.
Why: same as cause 4 on the pull-request comment path.
Fix: PostCommentForPullRequest's Location validation.
Traces: codecommit-pull-request-comment-token-and-line-refusals#task#9.

Cause 6 of 8 — commenting on a path absent from the named relativeFileVersion answers the wrong error identity and message (fixture codecommit-compared-commit-comment-location-refusals, step task#4).
AWS: Smithy: PostCommentForComparedCommit models both InvalidRelativeFileVersionEnumException ("Either the enum is not in a valid format, or the specified file version enum is not valid in respect to the current file version") and PathDoesNotExistException, both client/HTTP 400. The recording shows AWS picks InvalidRelativeFileVersionEnumException for this case, with message "You can only comment on existing files. You attempted to comment on a file that does not exist in this portion of the comparison between commits: <RELATIVE_FILE_VERSION>. Either the file has been deleted, or has not been added to the repository at the time of this commit. Try commenting on the file in the other commit portion of the comparison." Everything in that message is fixed text except the one variable part, the relativeFileVersion from the request's Location (BEFORE or AFTER); the file path is NOT part of the message.
Emulator: HTTP 400 with __type PathDoesNotExistException and message "the path <path> does not exist in the <RELATIVE_FILE_VERSION> version of the comparison" — a different exception shape and an invented template that names the path.
Why: a client branching on the error code takes the PathDoesNotExistException branch (retry with a different path) instead of the relative-file-version branch (retry against the other side of the comparison), and any message match fails.
Fix: PostCommentForComparedCommit's Location validation / error mapping.
Traces: codecommit-compared-commit-comment-location-refusals#task#4 (also codecommit-compared-commit-comment-location-refusals#task#7).

Cause 7 of 8 — CommentContentSizeLimitExceededException message text diverges on PostCommentReply (fixture codecommit-comment-reply-over-content-limit-rejected, step task#5).
AWS: Smithy: CommentContentSizeLimitExceededException (client, HTTP 400), documented "The comment is too large. Comments are limited to 10,240 characters." The recorded message is "The size limit for comments is 10240": fixed text "The size limit for comments is" plus the limit value as the only variable part.
Emulator: correct status 400 and error code, but message "the comment is too large: comments are limited to 10240 characters".
Why: a client surfacing or asserting on the service message shows wording AWS never sends.
Fix: the PostCommentReply content-length validation's error message template.
Traces: codecommit-comment-reply-over-content-limit-rejected#task#5 (also codecommit-comment-reply-required-member-and-token-refusals#task#6, codecommit-pull-request-comment-token-and-reply-target-refusals#task#8).

Cause 8 of 8 — the same message divergence on PostCommentForComparedCommit (fixture codecommit-comment-reply-over-content-limit-rejected, step task#6).
AWS: as cause 7; recorded message "The size limit for comments is 10240".
Emulator: "the comment is too large: comments are limited to 10240 characters".
Why: as cause 7.
Fix: the PostCommentForComparedCommit content-length validation's error message template (share one template with PostCommentReply and PostCommentForPullRequest).
Traces: codecommit-comment-reply-over-content-limit-rejected#task#6 (also codecommit-compared-commit-comment-absent-commit#task#4, codecommit-compared-commit-comment-location-refusals#task#5 and #task#6, codecommit-compared-commit-comment-size-and-commit-id-refusals#task#5, codecommit-compared-commit-comment-token-and-position-refusals#task#6).

Related open issues that do NOT cover these: 20261022T213415Z-codecommit-putcommentreaction-is-a-501-stub-cmk-disablement.md and 20261127T103949Z-codecommit-10-net-new-behavioral-causes-error-message.md mention diverging CodeCommit error-message templates, but neither carries the replay keys above and the replay reports these clusters as unowned.

## Evidence
Replay of trace set simparity-wire-aws-v1 (score 88.7%, 64 mismatched steps):

1) codecommit-comment-client-request-token-required#task#4 — PostCommentForComparedCommit: status expected 400, got 200; $.__type missing (expected 'ClientRequestTokenRequiredException', got None); $.message missing (expected 'A client request token is required.', got None). Cluster also covers codecommit-compared-commit-comment-token-and-position-refusals#task#4.
2) codecommit-comment-client-request-token-required#task#6 — PostCommentReply: identical expected-400/got-200 and missing __type/message.
3) codecommit-pull-request-comment-token-and-line-refusals#task#6 — PostCommentForPullRequest: identical expected-400/got-200 and missing __type/message.
4) codecommit-compared-commit-comment-token-and-position-refusals#task#7 — PostCommentForComparedCommit: status expected 200, got 400; afterBlobId, afterCommitId, beforeBlobId, beforeCommitId, comment, location (filePath notes.txt / filePosition past end / AFTER) and repositoryName all missing from the response.
5) codecommit-pull-request-comment-token-and-line-refusals#task#9 — PostCommentForPullRequest: status expected 200, got 400; pullRequestId, repositoryName, before/after commit+blob ids, location and comment all missing.
6) codecommit-compared-commit-comment-location-refusals#task#4 — PostCommentForComparedCommit: $.__type expected 'InvalidRelativeFileVersionEnumException', got 'PathDoesNotExistException'; $.message expected 'You can only comment on existing files. You attempted to comment on a file that does not exist in this portion of the comparison between commits: BEFORE. Either the file has been deleted, or has not been added to the repository at the time of this commit. Try commenting on the file in the other commit portion of the comparison.', got 'the path docs/guide.txt does not exist in the BEFORE version of the comparison'. Cluster also covers #task#7.
7) codecommit-comment-reply-over-content-limit-rejected#task#5 — PostCommentReply: $.message expected 'The size limit for comments is 10240', got 'the comment is too large: comments are limited to 10240 characters' (3 affected steps).
8) codecommit-comment-reply-over-content-limit-rejected#task#6 — PostCommentForComparedCommit: same message mismatch (6 affected steps).

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay trace set simparity-wire-aws-v1 (replay_wire_corpus).
3. Representative: fixture codecommit-comment-client-request-token-required, step task#4 — PostCommentForComparedCommit with repositoryName/afterCommitId/content and NO clientRequestToken; expect HTTP 400 ClientRequestTokenRequiredException 'A client request token is required.', observe HTTP 200 with a created comment.
4. Same fixture step task#6 (PostCommentReply) and codecommit-pull-request-comment-token-and-line-refusals#task#6 (PostCommentForPullRequest) for causes 2-3.
5. codecommit-compared-commit-comment-token-and-position-refusals#task#7 and codecommit-pull-request-comment-token-and-line-refusals#task#9: post a comment whose location.filePosition is past the file's last line; expect 200, observe 400.
6. codecommit-compared-commit-comment-location-refusals#task#4: post a comment on a path missing from the BEFORE side; expect InvalidRelativeFileVersionEnumException with AWS's 'You can only comment on existing files...' text, observe PathDoesNotExistException.
7. codecommit-comment-reply-over-content-limit-rejected#task#5 and #task#6: post content over 10240 characters and compare the message against 'The size limit for comments is 10240'.
