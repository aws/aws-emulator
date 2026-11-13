Issue-Source: trace-sweep-divergence
Issue-Trace: sagemaker-studio-domain-app-lifecycle
Issue-Step: teardown#11
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: sagemaker-studio-domain-app-lifecycle
Issue-Shape-Id: com.amazonaws.sagemaker#SageMaker
Issue-Key: sagemaker|DescribeDomain|4xx->2xx|$
Issue-Key: sagemaker|DescribeDomain|4xx->2xx|status
Issue-Commit: a1ec1c78d82f4b31ace8b5486cc3f03e8393d01c

# sagemaker DeleteDomain never removes the Studio domain: DescribeDomain keeps answering 200 InService instead of 400 ResourceNotFound

> _Note: this issue was identified against the code base at commit `a1ec1c78d82f4b31ace8b5486cc3f03e8393d01c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeDomain

## Description
ONE net-new cause in the SageMaker AI Studio domain lifecycle: a deleted domain is never actually deleted, so the post-delete DescribeDomain poll never converges.

Cause 1 of 1: after a successful DeleteDomain, the domain remains readable and InService forever.
AWS: DeleteDomain takes the required DomainId (@length 0..63, pattern ^d-(-*[a-z0-9]){1,61}$) plus an optional RetentionPolicy, returns an empty body, and starts an asynchronous teardown: the domain's Status moves through the DomainStatus enum to 'Deleting' and the domain record is then removed from the account. Once it is removed, DescribeDomain on that DomainId no longer succeeds — it raises the one error it models, ResourceNotFound (client error, HTTP 400, member Message of FailureReason, doc 'Resource being access is not found.'). The recorded teardown step is exactly that convergence: poll DescribeDomain until it fails with error code 'ResourceNotFound' and HTTP 400. (Smithy: DeleteDomain errors ResourceInUse, ResourceNotFound; DescribeDomain errors ResourceNotFound only; DomainStatus enum values Deleting/Failed/InService/Pending/Updating/Update_Failed/Delete_Failed.) The requirement is the terminal behavior — 400 with error code ResourceNotFound for a DomainId that no longer exists — not any particular domain id, ARN, timestamp or EFS/VPC value from the recording; the error Message, when present, is the resource-not-found sentence naming the requested DomainId, with the id taken from the request.
Emulator: DeleteDomain is accepted, but DescribeDomain keeps answering HTTP 200 with the complete DescribeDomainResponse — Status still 'InService', unchanged CreationTime/LastModifiedTime, DomainArn, DomainName, AuthMode, DefaultUserSettings, AppNetworkAccessType, SubnetIds, VpcId, Url, HomeEfsFileSystemCreation, TagPropagation, DefaultSpaceSettings all still populated. The replay polled 19 times and gave up at the 180 s cap: the status never becomes 'Deleting', the record is never withdrawn, and no error code is ever returned. (Hypothesis: the DeleteDomain handler returns the empty success body without mutating or removing the domain entry in the sagemaker state store.)
Why: a client cannot observe deletion at all. Any teardown that waits for a domain to disappear (the documented way to confirm DeleteDomain finished) hangs until its own timeout, and a caller re-creating a domain with the same configuration sees the stale record instead of a clean account. The ResourceNotFound path on DescribeDomain — the only error the operation models — is unreachable, so client code handling a deleted/mistyped domain is never exercised, and anything that reads Status to decide whether teardown is in flight permanently reads 'InService'.
Fix: the DeleteDomain handler and the domain record in the sagemaker service crate's state — DeleteDomain must transition the domain to the 'Deleting' DomainStatus and then remove it, and DescribeDomain must raise ResourceNotFound/400 for a DomainId that is absent instead of serving the retained record.
Traces: sagemaker-studio-domain-app-lifecycle#teardown#11.

Related open issue that does NOT cover this cause: '20261113T083839Z-sagemaker-studio-domain-and-app-paths-diverge-missing.md' covers CreateDomain's missing VpcId/SubnetIds validation, the non-served Studio Url, DescribeDomain's omitted DefaultUserSettings members and CreateApp's missing user-profile check — all on a live domain. None of its causes concerns DeleteDomain's effect or the post-delete ResourceNotFound path, and its replay keys do not include this step.

## Evidence
Fixture sagemaker-studio-domain-app-lifecycle, step teardown#11, operation DescribeDomain (service sagemaker): expected status 400 with error code 'ResourceNotFound'; got status 200.
Replay detail: "$: poll capped at 180s" and "$: poll: error code None not in ['ResourceNotFound'] after 19 attempt(s)" — the response body returned on every attempt was the full domain record with 'Status': 'InService' (plus DomainArn/DomainId/DomainName/AuthMode/DefaultUserSettings/AppNetworkAccessType/SubnetIds/Url/VpcId/HomeEfsFileSystemCreation/TagPropagation/DefaultSpaceSettings), i.e. no 'Deleting' transition and no removal after DeleteDomain succeeded.
Divergence key: service=sagemaker|DescribeDomain|status=400->200|keys=$,status. No stub_path and no owning open issue.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus (replay_wire_corpus).
2. Replay fixture sagemaker-studio-domain-app-lifecycle through its teardown; the preceding steps create a Studio domain and then call DeleteDomain on it.
3. At step teardown#11 the harness polls DescribeDomain with the deleted DomainId, waiting for HTTP 400 / error code ResourceNotFound.
4. Observe that every poll answers HTTP 200 with the full domain record and Status 'InService' until the 180 s poll cap is hit (19 attempts), instead of the domain transitioning to 'Deleting' and then failing with ResourceNotFound/400.
