Issue-Source: trace-sweep-divergence
Issue-Trace: codecommit-comment-and-reply-same-token-retry-returns-stored-comment
Issue-Step: task#4
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: codecommit-comment-and-reply-same-token-retry-returns-stored-comment codecommit-comment-client-request-token-required codecommit-comment-reply-over-content-limit-rejected codecommit-compared-commit-comment-location-refusals codecommit-compared-commit-comment-size-and-commit-id-refusals codecommit-compared-commit-comment-token-and-position-refusals codecommit-pull-request-comment-token-and-reply-target-refusals codecommit-pull-request-comment-malformed-commit-id codecommit-pull-request-comment-missing-commit-id-refusals codecommit-pull-request-comment-on-unknown-commit-is-refused codecommit-pull-request-comment-token-and-line-refusals codecommit-compared-commit-comment-absent-commit codecommit-compared-commit-comment-malformed-commit-id codecommit-comment-reply-required-member-and-token-refusals
Issue-Shape-Id: com.amazonaws.codecommit#CodeCommit_20150413
Issue-Key: codecommit|PostCommentForComparedCommit|2xx->2xx|$.comment.creationDate
Issue-Key: codecommit|PostCommentForComparedCommit|2xx->2xx|$.comment.lastModifiedDate
Issue-Key: codecommit|PostCommentReply|2xx->2xx|$.comment.creationDate
Issue-Key: codecommit|PostCommentReply|2xx->2xx|$.comment.lastModifiedDate
Issue-Key: codecommit|CreatePullRequest|2xx->2xx|$.pullRequest.creationDate
Issue-Key: codecommit|CreatePullRequest|2xx->2xx|$.pullRequest.lastActivityDate
Issue-Key: codecommit|PostCommentForPullRequest|2xx->2xx|$.comment.creationDate
Issue-Key: codecommit|PostCommentForPullRequest|2xx->2xx|$.comment.lastModifiedDate
Issue-Key: codecommit|GetCommentsForComparedCommit|2xx->2xx|$.commentsForComparedCommitData[*].comments[*].callerReactions
Issue-Key: codecommit|GetCommentsForComparedCommit|2xx->2xx|$.commentsForComparedCommitData[*].comments[*].creationDate
Issue-Key: codecommit|GetCommentsForComparedCommit|2xx->2xx|$.commentsForComparedCommitData[*].comments[*].lastModifiedDate
Issue-Key: codecommit|GetCommentsForPullRequest|2xx->2xx|$.commentsForPullRequestData[*].comments[*].callerReactions
Issue-Key: codecommit|GetCommentsForPullRequest|2xx->2xx|$.commentsForPullRequestData[*].comments[*].creationDate
Issue-Key: codecommit|GetCommentsForPullRequest|2xx->2xx|$.commentsForPullRequestData[*].comments[*].lastModifiedDate
Issue-Key: codecommit|GetCommentsForComparedCommit|2xx->2xx|$.commentsForComparedCommitData[*].comments[*]
Issue-Commit: e76aa5e7b75dc83d136a841bb7500c1a87dd58d0

# CodeCommit comment serializer omits callerReactions and emits sub-millisecond timestamps

> _Note: this issue was identified against the code base at commit `e76aa5e7b75dc83d136a841bb7500c1a87dd58d0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PostCommentForComparedCommit

## Description
Nine replayed divergence clusters that share one serialization fault: every CodeCommit structure carrying a Comment (or a timestamp member) is serialized with the wrong shape — the Comment's callerReactions list is dropped instead of being emitted empty, and timestamp members are emitted as full-precision floats rather than the millisecond-precision epoch seconds AWS's awsJson1_1 responses carry. Status codes and all other members are correct, so this is purely the response serializer. 9 causes.

Cause 1 of 9 — PostCommentForComparedCommit comment timestamps have the wrong shape (fixture codecommit-comment-and-reply-same-token-retry-returns-stored-comment, step task#4).
AWS: Smithy: Comment.creationDate targets CreationDate (timestamp) and Comment.lastModifiedDate targets LastModifiedDate (timestamp); CodeCommit is awsJson1_1, so timestamps go on the wire as epoch seconds. Recorded behavior only: the fixture asserts the SHAPE of these two members, not their value, and the emulator's value fails that shape assertion. Hypothesis: AWS serializes these epoch seconds with at most millisecond precision (three fractional digits), while the emulator emits the raw float.
Emulator: HTTP 200 with $.comment.creationDate and $.comment.lastModifiedDate as floats carrying seven fractional digits (sub-millisecond precision).
Why: a client that parses or round-trips the timestamp with a millisecond-resolution date type (every AWS SDK date type is millisecond-resolution) loses or rounds information the emulator invented, and anything that re-sends the value back fails a shape check AWS responses satisfy.
Fix: the codecommit response serializer's timestamp encoding, shared by every operation that returns a Comment or a date member.
Traces: codecommit-comment-and-reply-same-token-retry-returns-stored-comment#task#4 (7 affected steps across the comment fixtures).

Cause 2 of 9 — PostCommentReply comment timestamps, same shape fault (fixture codecommit-comment-and-reply-same-token-retry-returns-stored-comment, step task#6).
AWS: as cause 1 (Comment.creationDate / Comment.lastModifiedDate, shape asserted only).
Emulator: $.comment.creationDate and $.comment.lastModifiedDate emitted with sub-millisecond float precision.
Why: as cause 1.
Fix: same serializer.
Traces: codecommit-comment-and-reply-same-token-retry-returns-stored-comment#task#6 (5 affected steps).

Cause 3 of 9 — CreatePullRequest pullRequest timestamps, same shape fault (fixture codecommit-pull-request-comment-malformed-commit-id, step setup#5).
AWS: Smithy: PullRequest.creationDate and PullRequest.lastActivityDate are timestamps; recorded behavior only — shape asserted, value not.
Emulator: $.pullRequest.creationDate and $.pullRequest.lastActivityDate emitted with sub-millisecond float precision.
Why: as cause 1; shows the fault is in the shared timestamp encoder, not in the comment handlers.
Fix: same serializer.
Traces: codecommit-pull-request-comment-malformed-commit-id#setup#5 (4 affected steps).

Cause 4 of 9 — PostCommentForPullRequest comment timestamps, same shape fault (fixture codecommit-pull-request-comment-missing-commit-id-refusals, step task#8).
AWS: as cause 1.
Emulator: $.comment.creationDate / $.comment.lastModifiedDate with sub-millisecond float precision.
Why: as cause 1.
Fix: same serializer.
Traces: codecommit-pull-request-comment-missing-commit-id-refusals#task#8 (3 affected steps).

Cause 5 of 9 — GetCommentsForComparedCommit drops Comment.callerReactions (fixture codecommit-compared-commit-comment-absent-commit, step task#6).
AWS: Smithy: Comment.callerReactions targets CallerReactions (list), documented "The emoji reactions to a comment, if any, submitted by the user whose credentials are associated with the call to the API." The recording shows AWS emits it as an empty list for a comment the caller has not reacted to, alongside reactionCounts {}.
Emulator: the member is absent from each comment object ($.commentsForComparedCommitData[0].comments[0].callerReactions missing, expected []).
Why: a client that reads comment.callerReactions gets null instead of an empty list and must special-case it; code that iterates the list without a null guard breaks on the emulator and not on AWS.
Fix: the Comment serializer — always emit callerReactions, empty when the caller has no reactions.
Traces: codecommit-compared-commit-comment-absent-commit#task#6 (also codecommit-compared-commit-comment-malformed-commit-id#task#7).

Cause 6 of 9 — GetCommentsForComparedCommit drops callerReactions AND emits sub-millisecond comment timestamps (fixture codecommit-comment-reply-over-content-limit-rejected, step task#7).
AWS: as causes 1 and 5 — callerReactions [] present, creationDate/lastModifiedDate shape-asserted.
Emulator: callerReactions missing on comments[0]; creationDate and lastModifiedDate with sub-millisecond float precision.
Why: as causes 1 and 5.
Fix: the Comment serializer (both members).
Traces: codecommit-comment-reply-over-content-limit-rejected#task#7 (3 affected steps).

Cause 7 of 9 — GetCommentsForPullRequest drops callerReactions (fixture codecommit-pull-request-comment-malformed-commit-id, step task#9).
AWS: as cause 5, on commentsForPullRequestData[*].comments[*].
Emulator: $.commentsForPullRequestData[0].comments[0].callerReactions missing (expected []).
Why: as cause 5.
Fix: the Comment serializer, reached from GetCommentsForPullRequest.
Traces: codecommit-pull-request-comment-malformed-commit-id#task#9.

Cause 8 of 9 — GetCommentsForPullRequest drops callerReactions and emits sub-millisecond timestamps (fixture codecommit-pull-request-comment-missing-commit-id-refusals, step task#9).
AWS: as causes 1 and 5.
Emulator: callerReactions missing; creationDate/lastModifiedDate with sub-millisecond float precision.
Why: as causes 1 and 5.
Fix: the Comment serializer.
Traces: codecommit-pull-request-comment-missing-commit-id-refusals#task#9 (also codecommit-pull-request-comment-on-unknown-commit-is-refused#task#9).

Cause 9 of 9 — whole comment objects fail to match in GetCommentsForComparedCommit because the serialized Comment is missing callerReactions (fixture codecommit-comment-and-reply-same-token-retry-returns-stored-comment, step task#8).
AWS: as causes 1 and 5 — each comment in commentsForComparedCommitData[0].comments carries commentId, content, optional inReplyTo, creationDate, lastModifiedDate, authorArn, deleted, clientRequestToken, callerReactions [] and reactionCounts {}.
Emulator: the comment and its reply are emitted without callerReactions and with sub-millisecond timestamps, so the whole element diverges (reported as missing/extra for comments[0] and comments[1]).
Why: a client comparing or caching the comment structure sees a shape AWS never emits; this is the listing-level consequence of causes 1 and 5.
Fix: the Comment serializer.
Traces: codecommit-comment-and-reply-same-token-retry-returns-stored-comment#task#8 (3 affected steps).

## Evidence
Replay of trace set simparity-wire-aws-v1 (score 88.7%):

- codecommit-comment-and-reply-same-token-retry-returns-stored-comment#task#4 (PostCommentForComparedCommit, 200->200): $.comment.creationDate value_mismatch (expected '<shaped-not-asserted>', got 1795795105.3184052); $.comment.lastModifiedDate likewise. 7 affected steps.
- codecommit-comment-and-reply-same-token-retry-returns-stored-comment#task#6 (PostCommentReply): same two members, got 1795795105.3403494. 5 affected steps.
- codecommit-pull-request-comment-malformed-commit-id#setup#5 (CreatePullRequest): $.pullRequest.creationDate and $.pullRequest.lastActivityDate value_mismatch (expected '<shaped-not-asserted>', got 1795795058.9822893). 4 affected steps.
- codecommit-pull-request-comment-missing-commit-id-refusals#task#8 (PostCommentForPullRequest): $.comment.creationDate / lastModifiedDate, got 1795795066.8361092. 3 affected steps.
- codecommit-compared-commit-comment-absent-commit#task#6 (GetCommentsForComparedCommit): $.commentsForComparedCommitData[0].comments[0].callerReactions missing (expected [], got None). 2 affected steps.
- codecommit-comment-reply-over-content-limit-rejected#task#7 (GetCommentsForComparedCommit): callerReactions missing (expected []) plus creationDate/lastModifiedDate got 1795795073.950133. 3 affected steps.
- codecommit-pull-request-comment-malformed-commit-id#task#9 (GetCommentsForPullRequest): $.commentsForPullRequestData[0].comments[0].callerReactions missing (expected []).
- codecommit-pull-request-comment-missing-commit-id-refusals#task#9 (GetCommentsForPullRequest): callerReactions missing plus creationDate/lastModifiedDate got 1795795066.8361092. 2 affected steps.
- codecommit-comment-and-reply-same-token-retry-returns-stored-comment#task#8 (GetCommentsForComparedCommit): comments[0] and comments[1] reported missing/extra — expected objects carry 'callerReactions': [], the emulator's otherwise identical objects do not. 3 affected steps.

In every case the HTTP status matched (200) and all other members matched; only the Comment/date encoding diverged.

## Steps to Reproduce
1. Clone the repo and boot the emulator built from it.
2. Replay trace set simparity-wire-aws-v1 (replay_wire_corpus).
3. Representative: fixture codecommit-comment-and-reply-same-token-retry-returns-stored-comment, step task#4 — PostCommentForComparedCommit; inspect $.comment.creationDate / $.comment.lastModifiedDate: the emulator emits epoch seconds with seven fractional digits where the recording's shape assertion requires AWS's millisecond-precision encoding.
4. Same fixture step task#6, codecommit-pull-request-comment-malformed-commit-id#setup#5 and codecommit-pull-request-comment-missing-commit-id-refusals#task#8 for the other timestamp-carrying responses.
5. codecommit-compared-commit-comment-absent-commit#task#6 and codecommit-pull-request-comment-malformed-commit-id#task#9 — call GetCommentsForComparedCommit / GetCommentsForPullRequest and observe that comments[*].callerReactions is absent where AWS returns [].
6. codecommit-comment-reply-over-content-limit-rejected#task#7, codecommit-pull-request-comment-missing-commit-id-refusals#task#9 and codecommit-comment-and-reply-same-token-retry-returns-stored-comment#task#8 show both faults together.
