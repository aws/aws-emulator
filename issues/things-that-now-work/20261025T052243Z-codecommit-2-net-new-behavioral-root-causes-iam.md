Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codecommit.branch-protection-policy-denies-a-real-git-push-to-main codecommit.git-push-deny-without-the-null-reference-guard-blocks-every-branch codecommit.fast-forward-merge-refused-on-diverged-branches
Issue-Shape-Id: com.amazonaws.codecommit#CodeCommit_20150413
Issue-Key: codecommit.branch-protection-policy-denies-a-real-git-push-to-main|_await#35|Timeout|time.sleep(interval_s)
Issue-Key: codecommit.git-push-deny-without-the-null-reference-guard-blocks-every-branch|_await#35|Timeout|time.sleep(interval_s)
Issue-Key: codecommit|MergeBranchesByFastForward|ManualMergeRequiredException
Issue-Commit: 35f0669fbed72946ad7349f4657cbc8fac9f7c2b

# CodeCommit: 2 net-new behavioral root causes — IAM-conditioned git-push refusals are unobservable because the announced clone URL is still unusable, and ManualMergeRequiredException carries emulator prose instead of AWS's fixed text

## Resolution

Both causes are implemented in the emulator. Cause 2 was already fixed by a later change
to the merge-legality validator; cause 1 — the transport AND the receive-pack
authorization path the "Fix:" line below names — landed here.

* Cause 1, the announced address (FIXED) — `cloneUrlHttp` is now
  `http://<host>:<port>/v1/repos/<repositoryName>` on a listener of this emulator's own,
  opened by `CreateRepository` through the core's plaintext socket
  (`src/state/git_endpoint.rs`, `Listeners::tcp_plaintext`, so the core keeps the bind
  interface — loopback by default — the port range, the connection caps and the
  teardown). It speaks HTTP/1.1, negotiates TLS in band for a client that dials the same
  address with `https://`, and hands every request to the router that serves this
  emulator, so the announced origin is one an unmodified git client dials with no extra
  trust material and no certificate to accept. `tests/all/git_push_authorization.rs`
  opens a real socket to the announced authority and completes the exchange there.
* Cause 1, the authorization path (FIXED) — every request to the git transport is now
  authenticated and authorized (`src/service/git_authorization.rs`): a request presenting
  no credential is answered `401` with a `WWW-Authenticate: Basic` challenge, the git
  credentials it then presents (the access key id the CodeCommit credential helper writes,
  with its session token after a `%`, or an IAM service-specific credential's
  `ServiceUserName`) are resolved to their principal through the STS and IAM stores that
  minted them, and the identity-based policies in force on that principal decide
  `codecommit:GitPull` and `codecommit:GitPush`. The push is decided ONCE PER
  AUTHORIZATION STEP: the ref advertisement carries no value for
  `codecommit:References`, and each ref update command carries its own — so the
  documented `Null: {"codecommit:References": "false"}` guard behaves here as it does on
  AWS (dropping it widens the deny from one ref to the keyless step, which is refused
  `403` before any pack data is read), and a refused ref is refused on AUTHORIZATION
  ("You don't have permission to push changes to this branch."), leaves that ref where it
  stood, and does not stop the other refs of the same push.
* Cause 2 (FIXED, before this change) — `ManualMergeRequiredException` carries the fixed
  AWS sentence "Current branches cannot be merged, please try to merge using a Git client
  locally." (`src/state.rs`), shared by the branch and pull-request fast-forward paths,
  and the check `codecommit.fast-forward-merge-refused-on-diverged-branches` passes.

What REMAINS, and it is not this crate's to serve: the two git checks still spend their
whole window in their first poll, which is a `git clone` driven with
`-c credential.helper='!aws codecommit credential-helper $@'`. That helper is documented
for CodeCommit's own `https` origins (it signs a canonical request for the
`git-codecommit.<region>.amazonaws.com` host family), and the evidence here is consistent
with it printing nothing for an address an emulator serves on the machine it runs on: git
then has no credentials to present to the `401`, so no principal reaches the authorization
path those two checks grade, and every retry inside their 120 s window fails the same way
(the harness cuts the check at 60 s, before the poll's own assertion is reached, so the
verdict carries no git stderr to confirm it from). A client that CAN present git
credentials — an IAM service-specific credential pair, or any `Authorization: Basic`
header — completes the clone, the fetch and the push on the announced address today.
Closing the credential-helper path needs a transport whose certificate a stock git client
already trusts for an address this emulator answers on, which the core's listeners cannot
present (`vendor/server/src/tls_certificate_authority.rs` generates one self-signed
certificate per process and `vendor/server/src/listeners.rs` serves it on every listener).
The open
`issues/things-that-do-not-work/20261107T223907Z-codecommit-the-announced-cloneurlhttp-is-served-over-tls.md`
carries that gap and both check ids. Everything behind the transport is now exercised by
`tests/all/git_push_authorization.rs` instead: the announced address over a socket, the
challenge, the per-reference decision, the keyless step, the `Null` guard, and both
documented kinds of git credentials.

> _Note: this issue was identified against the code base at commit `35f0669fbed72946ad7349f4657cbc8fac9f7c2b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GitPush

## Description
Two net-new root causes in the codecommit crate, each grounded in a failing behavioral check that the replay report left unmarked (not ALREADY FILED, not OWNED by any open issue's replay key). A fixer must close BOTH before submitting.

Cause 1 of 2: the git smart-HTTP transport at the announced cloneUrlHttp cannot be driven by a standard git client, so CodeCommit's ONLY branch-protection mechanism — IAM authorization of codecommit:GitPush conditioned on codecommit:References, evaluated inside git-receive-pack — is not observable at all. Both checks that grade it never reach an assertion; they exhaust their clone/push poll window and the harness cuts them off.
AWS: recorded behavior only (no API operation models this; the decision is made during the git receive-pack negotiation, so the Smithy model says nothing about it). GetRepository.repositoryMetadata.cloneUrlHttp is an address the service serves git smart-HTTP on over TLS with a certificate a stock client trusts: with the `aws codecommit credential-helper`, a principal granted codecommit:GitPull/GitPush can `git clone <cloneUrlHttp>` and push. Authorization is then per-ref: an inline Deny on codecommit:GitPush with Condition {StringEqualsIfExists: {codecommit:References: [refs/heads/main]}, Null: {codecommit:References: "false"}} makes `git push origin main` exit non-zero from inside receive-pack (measured stderr: "! [remote rejected] HEAD -> main (You don't have permission to push changes to this branch.)" — an authorization refusal, with neither "fetch first" nor "non-fast-forward" present), leaves main's tip untouched, still admits the identical objects on refs/heads/feature, and admits the byte-identical push to main once the Deny is removed. Dropping ONLY the Null clause widens the same Deny from one ref to every ref — the CodeCommit docs make the guard mandatory ("The Null statement is required in any policy whose effect is Deny and where GitPush is one of the actions"), because at least one receive-pack authorization step carries no codecommit:References key at all, so StringEqualsIfExists evaluates TRUE against it; the refusal then surfaces as a 403 on the keyless discovery call before any pack data is sent, and restoring the clause narrows it back.
Emulator: neither check completes a single git exchange. Both poll their git legs inside a 120 s window (clone until rc=0, then push until rc=0 / until rc!=0 for the deny legs) and the harness reports "Check timed out after 60.0s" with no assertion reached — so the emulator neither demonstrated nor denied per-ref enforcement; the transport simply never served the client. Hypothesis (two possibilities consistent with this one symptom, both in the same component): (a) the poll is retrying the same hard refusal that open issue 20261023T185908Z-codecommit-8-net-new-behavioral-root-causes-the.md Cause 1 records verbatim for a third check — git rc=128, "fatal: unable to access '<cloneUrlHttp>': server verification failed: certificate signer not trusted" — until the budget is gone; or (b) the transport answers but authorizes every push regardless of the attached IAM policy, so the leg that polls for a refusal (ok = returncode != 0) never observes one and runs to its deadline. Note: that open issue describes this family and owns the ids codecommit.delete-file-on-tag-named-branch-blocked, codecommit.git-push-non-fast-forward-refused-by-the-server and codecommit.git-push-over-https-is-readable-through-the-api; NEITHER of the two ids below is on its replay list, so they are not re-replayed by its fix.
Why: cloning cloneUrlHttp is the documented way to use a CodeCommit repository, and an IAM Deny on GitPush conditioned on codecommit:References is the only branch protection the service offers. A client that reads the metadata and clones it cannot reach the repository; and an emulator that stores the policy, returns it from GetRolePolicy and then authorizes (or never serves) every push is indistinguishable from one that enforces it — a user who protects main in the emulator and verifies it there gets no protection, and the well-known Null-guard footgun (a Deny that silently locks the whole repository) is unreproducible.
Fix: the git smart-HTTP transport of the codecommit crate — its TLS termination and the cloneUrlHttp/cloneUrlSsh construction in the repository-metadata serializer (the announced URL must be an address, scheme and certificate an unmodified git client accepts), and the receive-pack authorization path, which must evaluate the caller's IAM policy once per authorization step including a keyless step that carries no codecommit:References key.
Traces: codecommit.branch-protection-policy-denies-a-real-git-push-to-main, codecommit.git-push-deny-without-the-null-reference-guard-blocks-every-branch.

Cause 2 of 2: MergeBranchesByFastForward's ManualMergeRequiredException carries an emulator-authored, request-interpolated message instead of AWS's fixed text.
AWS: Smithy (com.amazonaws.codecommit#CodeCommit_20150413) MergeBranchesByFastForward models ManualMergeRequiredException as a client error with httpError 400, documented "The pull request cannot be merged automatically into the destination branch. You must manually merge the branches and resolve any conflicts." Recorded behavior: when the two branches have diverged (neither tip an ancestor of the other) the refusal's Error.Message is the FIXED string "Current branches cannot be merged, please try to merge using a Git client locally." — it names neither branch nor commit, i.e. it is a constant for this error class, with the remediation sentence that tells the operator what to do instead.
Emulator: the status (400) and the code (ManualMergeRequiredException) are already correct, and so is everything the check asserts before the message — GetMergeOptions drops FAST_FORWARD_MERGE once the branches diverge while keeping SQUASH_MERGE/THREE_WAY_MERGE, and the base/source/destination commit ids are right. The residual divergence is the message, which the emulator builds from the request: "the merge of <sourceBranch> into <destinationBranch> cannot be performed by fast-forward because neither commit is an ancestor of the other. You must manually merge the branches and resolve any conflicts."
Why: this message is a constant AWS emits for the class, not formatting latitude; clients that surface or match the documented text (and recorded-fixture comparisons) diverge on every refused fast-forward, and the emulator's variant echoes branch names AWS never puts in this message.
Fix: the ManualMergeRequiredException message construction in the merge-legality validator shared by services/service-com-amazonaws-codecommit-codecommit-20150413/src/ops/merge_branches_by_fast_forward.rs (and the same exception raised from the pull-request fast-forward merge path) — emit the fixed AWS text rather than an interpolated one.
Traces: codecommit.fast-forward-merge-refused-on-diverged-branches.

## Evidence
All quoted strings are verbatim from the simparity-behavioral-ga-prod-v1 replay report (90 checks, 42 passed). Generated repository/branch names, commit ids, hosts and ports vary per run and are examples; the rule stated in each cause is the requirement.

Cause 1 — codecommit.branch-protection-policy-denies-a-real-git-push-to-main [mismatch]: "Error: Check timed out after 60.0s" — no assertion reached. codecommit.git-push-deny-without-the-null-reference-guard-blocks-every-branch [mismatch]: "Error: Check timed out after 60.0s" — no assertion reached. Both checks require only `git` and `aws` on the runner (require_tools("git", "aws")) and both drive the emulator through ctx.endpoint_route(meta["cloneUrlHttp"]), i.e. the address the emulator itself announced; their first git leg is a poll of `git clone <cloneUrlHttp>` whose documented failure mode is "a persistent failure means the git endpoint is unreachable for this principal". Against real AWS these checks were measured passing at ~84 s, where the wall clock is dominated by IAM propagation windows that do not exist in an emulator (a correct, propagation-free target satisfies each poll on its first attempt), so the expiry is not a budget artifact of a correct target. The related refusal is already recorded for a sibling check in open issue 20261023T185908Z-codecommit-8-net-new-behavioral-root-causes-the.md: git rc=128, stderr "fatal: unable to access 'https://127.0.0.1:34793/v1/repos/<repo>/': server verification failed: certificate signer not trusted. (CAfile: /etc/ssl/certs/ca-certificates.crt CRLfile: none)" — that issue's replay list does not contain either id above.

Cause 2 — codecommit.fast-forward-merge-refused-on-diverged-branches [mismatch]: the check asserts the code first (`assert code == "ManualMergeRequiredException"`, which PASSED) and then fails at `assert message == ("Current branches cannot be merged, please try to merge using a Git client locally.")` with AssertionError: "the merge of sim2real-behav-branch-d9a5f06c1759 into main cannot be performed by fast-forward because neither commit is an ancestor of the other. You must manually merge the branches and resolve any conflicts." The preceding assertions in the same check passed, including GetMergeOptions omitting FAST_FORWARD_MERGE for the diverged pair while still offering SQUASH_MERGE and THREE_WAY_MERGE and reporting the branch point as baseCommitId.

## Steps to Reproduce
Boot the emulator and point botocore at it; cc = client('codecommit'), iam = client('iam'), sts = client('sts'). `git` and the `aws` CLI must be on PATH for Cause 1.

Cause 1:
1. meta = cc.create_repository(repositoryName=R)['repositoryMetadata']; seed = cc.put_file(repositoryName=R, branchName='main', filePath='README.md', fileContent=b'seed', commitMessage='seed')['commitId']; cc.create_branch(repositoryName=R, branchName='feature', commitId=seed).
2. Create an IAM role assumable by the account root, attach an inline policy granting Allow codecommit:GitPull+GitPush on meta['Arn'] plus a Deny on codecommit:GitPush with Condition {"StringEqualsIfExists": {"codecommit:References": ["refs/heads/main"]}, "Null": {"codecommit:References": "false"}}, and assume it.
3. With those credentials and `-c credential.helper='!aws codecommit credential-helper $@' -c credential.UseHttpPath=true`, run `git clone <meta['cloneUrlHttp']>` -> expect the clone to succeed. Observe it failing (or the poll never succeeding) instead.
4. Commit a new file on 'feature' and `git push origin feature` -> expect exit 0 and cc.get_branch(branchName='feature') to report the pushed commit. Commit on 'main' and `git push origin main` -> expect a non-zero exit whose stderr is an authorization refusal (no "fetch first", no "non-fast-forward") and cc.get_branch(branchName='main') still at the seeded tip.
5. Re-put the policy with the Null clause REMOVED -> expect `git push --dry-run origin feature` to be refused too (the unguarded Deny covers every ref); restore the clause -> expect the byte-identical push to feature to succeed again.

Cause 2:
6. c_base = cc.put_file(repositoryName=R2, branchName='main', filePath='base.txt', fileContent=b'base')['commitId']; cc.create_branch(repositoryName=R2, branchName=F, commitId=c_base); cc.put_file(repositoryName=R2, branchName=F, filePath='feat.txt', fileContent=b'feat', parentCommitId=c_base); cc.put_file(repositoryName=R2, branchName='main', filePath='main.txt', fileContent=b'main', parentCommitId=c_base) so the branches diverge.
7. cc.merge_branches_by_fast_forward(repositoryName=R2, sourceCommitSpecifier=F, destinationCommitSpecifier='main', targetBranch='main') -> expect HTTP 400 ManualMergeRequiredException with Error.Message exactly "Current branches cannot be merged, please try to merge using a Git client locally."; observe the emulator's interpolated "the merge of <F> into main cannot be performed by fast-forward because neither commit is an ancestor of the other. You must manually merge the branches and resolve any conflicts."
8. Re-replay every check id in trace_ids.
