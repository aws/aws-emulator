Issue-Source: trace-sweep-divergence
Issue-Trace: ecr-repository-pagination-and-scoping-conflict
Issue-Step: task#6
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ecr-repository-pagination-and-scoping-conflict ecr-image-describe-pagination-and-params
Issue-Shape-Id: com.amazonaws.ecr#AmazonEC2ContainerRegistry_V20150921
Issue-Key: ecr|DescribeRepositories|4xx->2xx|$.__type
Issue-Key: ecr|DescribeRepositories|4xx->2xx|$.message
Issue-Key: ecr|DescribeRepositories|4xx->2xx|$.repositories[*].repositoryUri
Issue-Key: ecr|DescribeRepositories|4xx->2xx|status
Issue-Key: ecr|DescribeImages|2xx->2xx|$.imageDetails[*]
Issue-Related: 20261109T150104Z-ecr-publishes-real-aws-registry-hostnames-instead-of.md
Issue-Commit: d115796fe5713602f1adc3e09d553e6de30aa6e5

# ECR DescribeRepositories and DescribeImages reads diverge in ordering, continuation window and MaxResults/RepositoryNames validation

> _Note: this issue was identified against the code base at commit `d115796fe5713602f1adc3e09d553e6de30aa6e5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeRepositories

## Description
Three net-new causes on ECR's paginated read path: a missing input-conflict validation, a continuation page that does not line up with AWS's, and an unordered image list.

Cause 1 of 3 — DescribeRepositories accepts maxResults together with repositoryNames (fixture ecr-repository-pagination-and-scoping-conflict, step task#6).
AWS: maxResults and repositoryNames are mutually exclusive. Smithy (ECR): DescribeRepositoriesRequest$maxResults documentation — 'This option cannot be used when you specify repositories with repositoryNames.'; DescribeRepositories models InvalidParameterException (client error, HTTP 400). A request carrying both is refused 400 InvalidParameterException with message "Invalid parameter at 'MaxResults' failed to satisfy constraint: 'Cannot provide MaxResults if providing RepositoryNames'" — that message is entirely fixed text (the quoted member names 'MaxResults' and 'RepositoryNames' are part of the template, nothing in it comes from the request).
Emulator: answers HTTP 200 with a populated repositories list and no __type/message at all, i.e. the conflict is not validated.
Why: a client that relies on AWS refusing the combination gets a silently different contract — it believes a scoped-by-name read was also page-limited, and AWS-targeted code that handles InvalidParameterException never sees it, so the bug it is guarding against is invisible until the code runs against real ECR.
Fix: DescribeRepositories input validation (reject maxResults together with repositoryNames before the state read, raising InvalidParameterException with the message above).

Cause 2 of 3 — DescribeRepositories continuation page returns a different window than AWS (fixture ecr-repository-pagination-and-scoping-conflict, step task#4).
AWS: a nextToken returned by a page resumes strictly after the last entry of that page, so paging a stable repository set visits every repository exactly once, in one consistent order. Smithy (ECR): DescribeRepositories is @paginated {inputToken: nextToken, outputToken: nextToken, items: repositories, pageSize: maxResults}; the member docs state the remaining results are seen by sending another request with the returned nextToken. Recorded behavior: the recorded continuation page holds the two repositories that follow the first page.
Emulator: for the same token the page is shifted by one entry — its first element is a repository the previous page already returned, and the last repository AWS returns on this page is pushed out of it. Hypothesis: the token's offset is applied one entry early, or the ordering used to build a page differs from the ordering used to mint the token.
Why: a client paging through repositories sees a duplicate and, at the end of the walk, either loses the final repository or must make an extra request; code that de-duplicates by name silently processes fewer repositories than exist.
Fix: DescribeRepositories pagination in the handler/state read (mint and consume nextToken against one total order, resuming after the last emitted entry).

Cause 3 of 3 — DescribeImages returns imageDetails in an order that does not match AWS (fixture ecr-image-describe-pagination-and-params, step task#12).
AWS: recorded behavior only — the model documents no ordering for DescribeImages; the recording returns imageDetails in ascending imagePushedAt order (the image pushed first appears first, so a paged walk is stable).
Emulator: returns the same images in a different order — the second-pushed image is emitted before the first-pushed one, even though the emulator's own imagePushedAt values for those entries are in the opposite (push) order. Per-run values in the entries (imagePushedAt timestamps, generated repository names and digests) are not part of this finding; only the position of the entries is.
Why: a client that reads imageDetails[0] as the oldest image, or that pages DescribeImages with maxResults/nextToken, gets a different image than on AWS, and an unstable order makes a paged walk able to repeat or skip entries.
Fix: DescribeImages handler/state read (emit imageDetails in a deterministic order matching AWS's recorded ascending push order, and page over that same order).

Traces: ecr-repository-pagination-and-scoping-conflict#task#6, ecr-repository-pagination-and-scoping-conflict#task#4, ecr-image-describe-pagination-and-params#task#12.

## Evidence
Cause 1 (DescribeRepositories, ecr-repository-pagination-and-scoping-conflict#task#6): expected status 400, got 200. detail: "$.__type: missing (expected 'InvalidParameterException', got None); $.message: missing (expected \"Invalid parameter at 'MaxResults' failed to satisfy constraint: 'Cannot provide MaxResults if providing RepositoryNames'\", got None)", plus the 200 body's $.repositories[0].repositoryUri where AWS returns no repositories at all.

Cause 2 (DescribeRepositories, ecr-repository-pagination-and-scoping-conflict#task#4): status 200 -> 200; detail "$.repositories[0].repositoryUri: expected '...<repo-A>', got '...<repo-prev>'; $.repositories[1].repositoryUri: expected '...<repo-B>', got '...<repo-A>'" — the emulator's page begins one entry earlier than AWS's and drops AWS's last entry (recorded values: expected [sim2real-probe-ecr-pag-a2e2ee22, sim2real-probe-ecr-pag-ae2e30d8], got [sim2real-probe-ecr-pag-0f27820e, sim2real-probe-ecr-pag-a2e2ee22]; the names are per-run, the one-entry shift is the finding).

Cause 3 (DescribeImages, ecr-image-describe-pagination-and-params#task#12): status 200 -> 200; detail reports $.imageDetails[0]/[1]/[2] all differing by position — expected[0] is the image tagged v1 and expected[1] the image tagged v2 (recorded imagePushedAt ascending), while the emulator returned the v2 entry at index 0 and the v1 entry at index 1, with its own imagePushedAt values showing v1 pushed before v2.

Expected: the both-parameters request is refused 400 InvalidParameterException with AWS's fixed message; a continuation page resumes after the previous page's last entry; imageDetails is ordered by ascending push time. Actual: 200 with results, a page shifted by one entry, and an unordered image list.

## Steps to Reproduce
1. Build and boot the emulator from this checkout.
2. Replay the wire corpus (trace set simparity-wire-aws-v1).
3. Re-replay ecr-repository-pagination-and-scoping-conflict steps task#6 and task#4, and ecr-image-describe-pagination-and-params step task#12.
4. Or by hand: (a) call DescribeRepositories with both repositoryNames and maxResults set — the emulator answers 200 instead of 400 InvalidParameterException; (b) create three repositories, call DescribeRepositories with maxResults smaller than the set, then follow the returned nextToken — the second page repeats the previous page's last repository; (c) push two images to one repository and call DescribeImages — imageDetails is not ordered by imagePushedAt.
