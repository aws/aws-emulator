Issue-Source: trace-sweep-divergence
Issue-Trace: codecommit-merge-by-three-way-nonexistent-repository
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: codecommit-merge-by-three-way-nonexistent-repository codecommit-file-folder-blob-content-operations
Issue-Shape-Id: com.amazonaws.codecommit#CodeCommit_20150413
Issue-Key: codecommit|MergePullRequestByThreeWay|4xx->4xx|$.__type
Issue-Key: codecommit|MergePullRequestByThreeWay|4xx->4xx|$.message
Issue-Key: codecommit|ListFileCommitHistory|2xx->2xx|$.revisionDag[*]
Issue-Commit: ac9e642ebb6d7cd67632ea3e08d8c560fc4b2f43

# codecommit wrong error for missing repository and unstable commit dates

> _Note: this issue was identified against the code base at commit `ac9e642ebb6d7cd67632ea3e08d8c560fc4b2f43`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
MergePullRequestByThreeWay

## Description
Two net-new codecommit wire divergences, both cases of a response that does not agree with the resource the request names: a merge refusal that reports the wrong missing resource, and a commit projection that reports a date other than the commit's own. BOTH must be fixed before this issue is closed.

Cause 1 of 2 - MergePullRequestByThreeWay against a repository name the account holds no repository for reports PullRequestDoesNotExistException instead of RepositoryDoesNotExistException (fixture codecommit-merge-by-three-way-nonexistent-repository, step task#1).
AWS: Smithy: MergePullRequestByThreeWay input has pullRequestId @required and repositoryName @required (target RepositoryName, @length min 1 max 100, pattern ^[\w\.-]+$); its modeled errors include RepositoryDoesNotExistException ("The specified repository does not exist.", client error, HTTP 400) and PullRequestDoesNotExistException (client error, HTTP 400). The recorded response shows AWS resolving the REPOSITORY the request names before the pull request id: HTTP 400 with __type RepositoryDoesNotExistException and a message whose template is "<repositoryName> for <accountId>" - the fixed text is the single " for " separator, the first variable part is the repositoryName copied from the request and the second is the account id the lookup ran in (this run's recording shows the probe repository name and account 000000000000; neither literal is part of the requirement).
Emulator: HTTP 400 with __type PullRequestDoesNotExistException and message "The pull request ID does not exist. Verify that you provided a valid pull request ID, and then try your operation again. To get a list of pull request IDs, use ListPullRequests." - i.e. it answers the pull-request question first and never reports the missing repository.
Why: a client that branches on the error code (create or re-name the repository on RepositoryDoesNotExistException, re-list pull requests on PullRequestDoesNotExistException) takes the wrong branch whenever the repository name is wrong or the repository has been deleted; it will loop re-listing pull request ids for a repository that does not exist. The message also no longer carries the repository name, so a caller cannot tell WHICH name failed.
Fix: the operation's handler, in its precondition ordering - services/service-com-amazonaws-codecommit-codecommit-20150413/src/ops/merge_pull_request_by_three_way.rs performs the store-wide pull-request lookup ("the question this operation answers FIRST") before the repositories lookup; the repository named in the request must be resolved first, raising RepositoryDoesNotExistException worded by the existing crate::state::repository_does_not_exist_message helper (which already produces AWS's "<name> for <account>" template). The sibling pull-request-keyed merge handlers that carry the same ordering comment should be checked for the same inversion.
Traces: codecommit-merge-by-three-way-nonexistent-repository#task#1.

Cause 2 of 2 - ListFileCommitHistory reports a commit's author/committer date that is not the date of the commit it is reporting (fixture codecommit-file-folder-blob-content-operations, step task#9).
AWS: Smithy: ListFileCommitHistory output revisionDag @required is a list of FileVersion {commit, blobId, path, revisionChildren}; Commit.author/committer are UserInfo, whose date member is documented "The date when the specified commit was commited, in timestamp format with GMT offset." The date is therefore a property OF THE COMMIT: every read of the same commitId - GetCommit, ListFileCommitHistory, and the commit object the git transport serves, whose SHA-1 name is computed over those identity lines - reports the same date. Recorded behavior only for the concrete value: the fixture asserts the date against a value bound from the same run's earlier read of that commit (the corpus marks values it genuinely cannot assert as not-asserted, and this one is asserted), so the requirement is agreement within the run, not a literal timestamp.
Emulator: HTTP 200, and every other member of the revisionDag element matches exactly (commitId, treeId, parents, message, additionalData, author/committer name and email, blobId, path, revisionChildren). Only commit.author.date and commit.committer.date diverge: the emulator returns the wall-clock second at which the ListFileCommitHistory call was replayed ("<unix-seconds> +0000", 1794611843 +0000 in this run - a per-run value, not the requirement) rather than the date bound for that commit id.
Why: a client correlating file history with a commit read, ordering revisions by date, or verifying the commit id it was handed against the commit object's identity lines sees two different dates for one commit id; the reported commitId then does not correspond to the author/committer lines reported beside it, since a git commit name is hashed over exactly those lines.
Fix: the commit projection this operation serves - Commit::info()/identity_info in services/service-com-amazonaws-codecommit-codecommit-20150413/src/state.rs must report the STORED author/committer UserInfo, and the write path that stamps a commit date (state::commit_identity, which formats git_date(now)) must store the same date that state::commit_id / git_object::encode_commit hashed into the commit object. Hypothesis: the date reported here is re-stamped from a later clock read instead of being carried from the stored commit, which is why it tracks replay time.
Traces: codecommit-file-folder-blob-content-operations#task#9.

## Evidence
Cause 1 - codecommit-merge-by-three-way-nonexistent-repository#task#1, MergePullRequestByThreeWay, expected status 400 / actual status 400:
  $.__type: expected 'RepositoryDoesNotExistException', got 'PullRequestDoesNotExistException'
  $.message: expected '<repositoryName> for <accountId>' (recorded: 'sim2real-probe-codecommit-cc-merge-threeway-repo-5d503b9e for 000000000000'), got 'The pull request ID does not exist. Verify that you provided a valid pull request ID, and then try your operation again. To get a list of pull request IDs, use ListPullRequests.'
  Fixed text of the expected message: the ' for ' separator only. Variable parts: repositoryName from the request, account id from the caller's account.
  Source confirmation: merge_pull_request_by_three_way.rs checks store.pull_request(pull_request_id).is_none() -> PullRequestDoesNotExistException BEFORE store.repositories.contains_key(&input.repository_name) -> RepositoryDoesNotExistException.

Cause 2 - codecommit-file-folder-blob-content-operations#task#9, ListFileCommitHistory, expected status 200 / actual status 200:
  $.revisionDag[0]: element does not match. Expected commit.author.date / commit.committer.date = the date bound for commit 4cf9ef7f1201a3066749d2bb8f182757ebc5b1d8 earlier in the same fixture; got '1794611843 +0000' for both, which is the replay-time wall clock.
  All other members of the element are byte-identical between expected and actual (commitId, treeId, parents ['5d43186c004aae997df470f3b1545ae5e39307ac'], message 'add docs/sub/notes.txt', additionalData '', author/committer name and email, blobId a068def323c4ff9b39c29215807e7e754cbb2a8a, path 'docs/sub/notes.txt', revisionChildren []).
  Format is correct ('<unix-seconds> +0000', matching UserInfo.date's "timestamp format with GMT offset"); the VALUE disagrees with the date of the commit being reported.

## Steps to Reproduce
Cause 1: with the emulator serving codecommit, call MergePullRequestByThreeWay with a repositoryName the account holds no repository for and any pullRequestId. Observe HTTP 400 __type PullRequestDoesNotExistException; AWS answers HTTP 400 __type RepositoryDoesNotExistException with message '<repositoryName> for <accountId>'. Replay fixture codecommit-merge-by-three-way-nonexistent-repository step task#1.

Cause 2: create a repository, write a file twice so a second commit exists (PutFile/CreateCommit), read that commit (GetCommit) and note commit.author.date, then call ListFileCommitHistory for that file path and compare the author/committer date reported for the same commitId. The two reads must report the same date, and it must be the date hashed into the commit object named by commitId. Replay fixture codecommit-file-folder-blob-content-operations step task#9.
