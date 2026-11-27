Issue-Source: trace-sweep-divergence
Issue-Trace: codecommit-comment-client-request-token-required
Issue-Step: task#9
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: codecommit-comment-client-request-token-required codecommit-compared-commit-comment-token-and-position-refusals codecommit-pull-request-comment-token-and-line-refusals codecommit-pull-request-comment-token-and-reply-target-refusals
Issue-Shape-Id: com.amazonaws.codecommit#CodeCommit_20150413
Issue-Key: codecommit|GetCommentsForComparedCommit|2xx->2xx|$.commentsForComparedCommitData[*]
Issue-Key: codecommit|GetCommentsForPullRequest|2xx->2xx|$.commentsForPullRequestData[*]
Issue-Key: codecommit|GetCommentsForPullRequest|2xx->2xx|$.commentsForPullRequestData[*].comments[*]
Issue-Related: 20261127T160338Z-codecommit-comment-serializer-omits-callerreactions-and-emits-sub.md
Issue-Commit: e76aa5e7b75dc83d136a841bb7500c1a87dd58d0

# CodeCommit comment listings return extra location groups and comments AWS refused

> _Note: this issue was identified against the code base at commit `e76aa5e7b75dc83d136a841bb7500c1a87dd58d0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetCommentsForComparedCommit

## Description
Four replayed divergence clusters where GetCommentsForComparedCommit / GetCommentsForPullRequest answer HTTP 200 with the wrong set of comment groups: comments that AWS refused at write time are present, comments AWS stored are absent, and the comments are split across more commentsForComparedCommitData / commentsForPullRequestData entries than AWS returns for the same commit pair and file location. 4 causes.

Cause 1 of 4 — GetCommentsForComparedCommit returns a comment AWS never stored and splits the thread into an extra location group (fixture codecommit-comment-client-request-token-required, step task#9).
AWS: recorded behavior only — for one before/after commit pair and one file location AWS returns a SINGLE commentsForComparedCommitData entry whose comments list holds the stored comment and its reply (the reply carrying inReplyTo), and it holds only comments whose write succeeded; the token-less posts earlier in the fixture were refused with HTTP 400 ClientRequestTokenRequiredException (Smithy: ClientRequestTokenRequiredException, client, HTTP 400) and therefore appear nowhere in the listing.
Emulator: HTTP 200 but commentsForComparedCommitData[0].comments holds a different comment than the one recorded, the recorded reply is absent (comments[1] is None), and a second commentsForComparedCommitData[1] entry is emitted for the same repository and commit pair carrying three further comments — the ones the write path accepted instead of refusing.
Why: a client paging comments on a comparison sees phantom comments (ones the service told it nothing about, because the refusal never happened) and loses the thread structure, so review tooling renders a thread that does not exist on AWS and misses the real one.
Fix: GetCommentsForComparedCommit's state/grouping — one commentsForComparedCommitData entry per (beforeCommitId, afterCommitId, location) and only successfully stored comments; the stored-phantom half is the write-path validation reported in the companion write-validation issue.
Traces: codecommit-comment-client-request-token-required#task#9.

Cause 2 of 4 — GetCommentsForComparedCommit groups by the wrong file location and reports a filePosition the caller did not use (fixture codecommit-compared-commit-comment-token-and-position-refusals, step task#9).
AWS: recorded behavior only — the listing returns the group whose location is the one the stored comment was posted at (filePath notes.txt, relativeFileVersion AFTER, the requested filePosition), with that comment's long clientRequestToken echoed, and no second group.
Emulator: the first group's location.filePosition is a different line from the one the stored comment was posted at, its comments list holds a comment that should have been refused for a missing client request token, and further groups diverge.
Why: a client that keys annotations by (filePath, relativeFileVersion, filePosition) attaches comments to the wrong line of the diff.
Fix: GetCommentsForComparedCommit's state/grouping.
Traces: codecommit-compared-commit-comment-token-and-position-refusals#task#9.

Cause 3 of 4 — GetCommentsForPullRequest returns the refused comment instead of the stored one and omits the second group (fixture codecommit-pull-request-comment-token-and-line-refusals, step task#10).
AWS: recorded behavior only — two commentsForPullRequestData entries for pullRequestId 1, each with the before/after commit and blob ids, its own location, and the comment stored at that location (including the comment posted past the file's last line, which AWS accepts).
Emulator: a single group whose comments list holds the token-less comment the write path should have refused, with no beforeBlobId/afterBlobId and no second group for the past-end-of-file comment the emulator refused with HTTP 400.
Why: a pull-request review client sees neither the comment it successfully posted nor the correct per-location grouping, and sees one it was never told was created.
Fix: GetCommentsForPullRequest's state/grouping (plus the write-path validation filed separately).
Traces: codecommit-pull-request-comment-token-and-line-refusals#task#10.

Cause 4 of 4 — GetCommentsForPullRequest comment objects do not match the stored comment thread (fixture codecommit-pull-request-comment-token-and-reply-target-refusals, step task#11).
AWS: recorded behavior only — commentsForPullRequestData[0].comments holds the general remark and its reply (inReplyTo set to the first comment's id), each comment carrying its clientRequestToken, authorArn, deleted false, callerReactions [] and reactionCounts {}.
Emulator: HTTP 200 with the same two comments, but each object diverges from the recorded shape (reported as missing/extra for comments[0] and comments[1]) because callerReactions is absent and the timestamps are emitted with sub-millisecond precision.
Why: a client comparing the returned thread against the shape AWS emits never matches; this is the listing-level consequence of the Comment serializer fault.
Fix: the Comment serializer reached from GetCommentsForPullRequest (same component as the companion serializer issue) — fixing that one must also re-run this step.
Traces: codecommit-pull-request-comment-token-and-reply-target-refusals#task#11.

## Evidence
Replay of trace set simparity-wire-aws-v1 (score 88.7%):

- codecommit-comment-client-request-token-required#task#9 (GetCommentsForComparedCommit, 200->200): $.commentsForComparedCommitData[0].comments[0] missing (expected the comment with commentId 'c65a50d9-...:4e23ba79-...', got a different commentId '7d158688-...:e95ca181-...'); $.commentsForComparedCommitData[0].comments[1] missing (expected the reply with inReplyTo set, got None); $.commentsForComparedCommitData[1] extra (expected None, got a second group for the same repositoryName/beforeCommitId/afterCommitId carrying three comments).
- codecommit-compared-commit-comment-token-and-position-refusals#task#9 (GetCommentsForComparedCommit): $.commentsForComparedCommitData[0] missing — expected location {filePath notes.txt, filePosition 1, relativeFileVersion AFTER} with the long-clientRequestToken comment 'first line, longest token'; got location filePosition 2 with a comment 'needs a token' and no clientRequestToken. $.commentsForComparedCommitData[1] also missing.
- codecommit-pull-request-comment-token-and-line-refusals#task#10 (GetCommentsForPullRequest): $.commentsForPullRequestData[0] missing — expected before/after blob ids, location {notes.txt, 1, AFTER} and comment 'remark on the first line'; got a group with no blob ids and comment 'general remark without a token'. $.commentsForPullRequestData[1] missing (the past-end-of-file comment group).
- codecommit-pull-request-comment-token-and-reply-target-refusals#task#11 (GetCommentsForPullRequest): comments[0] and comments[1] reported missing — the expected objects carry 'callerReactions': [] and shape-asserted timestamps, the emulator's carry neither.

All four returned HTTP 200, so the divergence is entirely in the body's comment set and grouping.

## Steps to Reproduce
1. Clone the repo and boot the emulator built from it.
2. Replay trace set simparity-wire-aws-v1 (replay_wire_corpus).
3. Representative: fixture codecommit-comment-client-request-token-required, step task#9 — after the fixture's earlier token-less PostCommentForComparedCommit/PostCommentReply calls, call GetCommentsForComparedCommit for the commit pair: AWS returns one commentsForComparedCommitData group holding the stored comment and its reply; the emulator returns a different first comment, no reply, and an extra second group holding the comments it should have refused.
4. codecommit-compared-commit-comment-token-and-position-refusals#task#9 — same call, observe the group's location.filePosition does not match the position the stored comment was posted at.
5. codecommit-pull-request-comment-token-and-line-refusals#task#10 — GetCommentsForPullRequest for pull request 1: expect two location groups with before/after blob ids, observe one group holding the token-less comment.
6. codecommit-pull-request-comment-token-and-reply-target-refusals#task#11 — GetCommentsForPullRequest and compare each comment object against the recorded shape.
