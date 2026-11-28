Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codecommit.get-blob-over-six-mib-pushed-by-git-rejected codecommit.get-file-on-git-pushed-file-over-six-mib-rejected
Issue-Shape-Id: com.amazonaws.codecommit#CodeCommit_20150413
Issue-Key: codecommit.get-blob-over-six-mib-pushed-by-git-rejected|run#25|AssertionError|assert push.returncode == 0, f"git push exited {push.returncode}; stderr={push.stderr[-300:]!r}"
Issue-Key: codecommit.get-file-on-git-pushed-file-over-six-mib-rejected|_run_git#23|AssertionError|assert proc.returncode == 0, f"git {args[0]!r} failed ({proc.returncode}): {proc.stderr[-300:]!r}"
Issue-Commit: ed3ed06301643834f5ccb593614cfd988c817634

# CodeCommit announces repositoryMetadata.cloneUrlHttp with the plaintext http:// scheme, so the documented `aws codecommit credential-helper` git push path gets no credentials and every credential-helper push to the announced URL fails

> _Note: this issue was identified against the code base at commit `ed3ed06301643834f5ccb593614cfd988c817634`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetRepository

## Description
Cause 1 of 1 — the git smart-HTTP address the emulator announces in CreateRepository/GetRepository `repositoryMetadata.cloneUrlHttp` is a plaintext `http://<host>:<port>/v1/repos/<repositoryName>` URL, and the endpoint behind it challenges an unauthenticated git request for HTTP Basic credentials. The credential helper AWS documents for this transport (`git -c credential.helper='!aws codecommit credential-helper $@' -c credential.UseHttpPath=true`) therefore supplies nothing, git falls through to an interactive prompt, and with prompts disabled the push dies before any git protocol exchange. Two independent data-plane checks that must first push content with git are blocked by this one seam, so the GetBlob/GetFile 6 MiB boundary behaviour they exist to pin is never reached. Failing checks: codecommit.get-blob-over-six-mib-pushed-by-git-rejected, codecommit.get-file-on-git-pushed-file-over-six-mib-rejected.

AWS: Smithy: GetRepository output shape `RepositoryMetadata`, member `cloneUrlHttp` (String) — documented as "The URL to use for cloning the repository over HTTPS." The model says nothing further about the transport, so beyond the scheme this is documented/recorded behavior only (https://docs.aws.amazon.com/codecommit/latest/userguide/temporary-access.html). Real CodeCommit announces an `https://` URL and serves git smart-HTTP there, authenticating the request with a SigV4-derived user/password that `aws codecommit credential-helper get` emits; that helper only emits credentials for a request whose `protocol` is https. Consequently a hermetic `git push <cloneUrlHttp> <local>:<remote>` configured with that helper exits 0, and GetBranch(repositoryName, branchName) then reports the commitId git pushed.

Emulator: the announced cloneUrlHttp uses scheme `http`, and the push exits 128 with git's own `fatal: could not read Username for '<cloneUrlHttp>': terminal prompts disabled`. The fixed text is `fatal: could not read Username for '<url>': terminal prompts disabled`; the url (host, port and repository name) is a per-run value taken from the announced metadata and is not part of the requirement. No HTTP git exchange, no credential verification and no reference update occur. Hypothesis: because the announced scheme is not https, the documented credential helper declines to answer the credential query, so the Basic-auth challenge the endpoint issues can never be satisfied by the documented workflow.

Why: a client that reads cloneUrlHttp from CreateRepository/GetRepository and hands it to an unmodified git client with the credential helper AWS documents cannot clone, fetch or push at all — it sees a username prompt (or a 401) instead of a successful transfer. Every workflow that seeds repository content with git rather than PutFile is unusable, and anything downstream of such a push (GetBlob, GetFile, GetFolder, GetBranch over git-pushed objects) cannot be exercised.

Fix: the seam that composes the announced clone URLs in the CodeCommit repository metadata (services/service-com-amazonaws-codecommit-codecommit-20150413/src/state.rs together with src/ops/create_repository.rs and src/ops/get_repository.rs) and the git smart-HTTP listener it names — the announced address must be reachable, over the scheme it announces, by an unmodified git client using the documented credential helper, and the transport's authentication must accept the credentials that helper produces.

Related: open issue 20261124T051640Z-codecommit-credential-helper-git-transport-the-announced-cloneurlhttp.md and open issue 20261025T040000Z-codecommit-announced-clone-url-is-served-with-an-untrusted-certificate.md record an EARLIER, different symptom of this transport (an https URL fronted by a self-signed certificate, failing at the TLS handshake) and carry only their own check keys; neither carries the two check keys filed here, and the failure observed now is a plaintext http:// announcement with a credential prompt, not a certificate rejection.

Traces: codecommit.get-blob-over-six-mib-pushed-by-git-rejected (check line 114: `git push exited 128; stderr="fatal: could not read Username for 'http://<host>:<port>/v1/repos/<repo>': terminal prompts disabled"`); codecommit.get-file-on-git-pushed-file-over-six-mib-rejected (check line 102 via _run_git line 54: `git '-c' failed (128): "fatal: could not read Username for 'http://<host>:<port>/v1/repos/<repo>': terminal prompts disabled"`).

## Evidence
Expected (real AWS):
- `repositoryMetadata.cloneUrlHttp` is an `https://` git smart-HTTP URL for the repository (Smithy: "The URL to use for cloning the repository over HTTPS").
- With GIT_TERMINAL_PROMPT=0, GIT_CONFIG_GLOBAL=/dev/null, GIT_CONFIG_NOSYSTEM=1 and `-c credential.helper='!aws codecommit credential-helper $@' -c credential.UseHttpPath=true`, `git push <cloneUrlHttp> <branch>:<branch>` exits 0; GetBranch(branchName).branch.commitId then equals the local `git rev-parse HEAD`, and GetBlob/GetFile serve the pushed objects (a 6 MiB object is served; 6 MiB + 1 is refused with FileTooLargeException).

Actual (emulator):
- cloneUrlHttp is announced as `http://<host>:<port>/v1/repos/<repositoryName>` (scheme http, not https).
- The push exits 128 with git's `fatal: could not read Username for '<cloneUrlHttp>': terminal prompts disabled` — fixed text `fatal: could not read Username for '<url>': terminal prompts disabled`, with `<url>` taken verbatim from the announced metadata. The credential helper supplied no credentials and the endpoint's Basic-auth challenge went unanswered; nothing was pushed, so neither the 6 MiB success nor the FileTooLargeException refusal could be observed.

## Steps to Reproduce
1. `meta = cc.create_repository(repositoryName=R)['repositoryMetadata']` and note `meta['cloneUrlHttp']` — observe the `http://` scheme where AWS announces `https://`.
2. Locally: `git init -b main`, write a small file plus a 6 MiB file and a 6 MiB + 1 file, `git add -A`, `git commit -m seed`, record `git rev-parse HEAD`.
3. With a hermetic git environment (GIT_TERMINAL_PROMPT=0, GIT_CONFIG_GLOBAL=/dev/null, GIT_CONFIG_NOSYSTEM=1) run `git -c credential.helper='!aws codecommit credential-helper $@' -c credential.UseHttpPath=true push --quiet <meta['cloneUrlHttp']> main:main` -> expect exit 0; observe exit 128 with `fatal: could not read Username for '<cloneUrlHttp>': terminal prompts disabled`.
4. `cc.get_branch(repositoryName=R, branchName='main')` -> expect the commitId from step 2; nothing was pushed.
5. `cc.get_blob(repositoryName=R, blobId=<6 MiB blob>)` -> expect the 6 MiB bytes; `cc.get_blob(... blobId=<6 MiB + 1 blob>)` -> expect FileTooLargeException. Likewise `cc.get_file(repositoryName=R, commitSpecifier='trunk', filePath=...)`.
6. Re-replay codecommit.get-blob-over-six-mib-pushed-by-git-rejected and codecommit.get-file-on-git-pushed-file-over-six-mib-rejected.
