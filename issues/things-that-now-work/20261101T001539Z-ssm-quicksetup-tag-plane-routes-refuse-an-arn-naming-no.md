Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ssm-quicksetup.tag-resource-list-tags-roundtrip
Issue-Shape-Id: com.amazonaws.ssmquicksetup#QuickSetup
Issue-Key: ssm-quicksetup.tag-resource-list-tags-roundtrip|run#22|AssertionError|assert tag_code == "ResourceNotFoundException", (
Issue-Commit: dc6c2293f24f05a053eccc773f364dc1d05ad512

# Tag-plane routes refuse an ARN naming no manager with 401 UnauthorizedException instead of the modeled 404 ResourceNotFoundException

> _Note: this issue was identified against the code base at commit `dc6c2293f24f05a053eccc773f364dc1d05ad512`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListTagsForResource

## Description
1 cause; it must be closed before this issue is submitted.

Cause 1 of 1: the locally invented `TargetAuthorization` request-authorization stage in front of the `/tags/{ResourceArn}` route family (and `/configurationDefinition/{ManagerArn}/{Id}`) turns "this ARN names no configuration manager in this Region" into an authorization refusal, so the operation's own modeled not-found answer is never reached.

AWS: `ListTagsForResource` (GET /tags/{ResourceArn}) binds exactly six errors and `UnauthorizedException` is not one of them — Smithy: ListTagsForResource errors = AccessDeniedException (403), ConflictException (409), InternalServerException (500), ResourceNotFoundException (404), ThrottlingException (429), ValidationException (400); ResourceNotFoundException is documented "The resource couldn't be found. Check the ID or name and try again." An ARN that names no resource — including the ARN of a configuration manager that was just deleted — is therefore answered HTTP 404 with error code `ResourceNotFoundException`. The message is the service's manager-not-found sentence, fixed text `Quick setup manager with arn: ` + the ARN the request carried + ` could not be found.` — the only variable part is the ARN, taken from the request path (this crate already states that template in `crate::state::manager_not_found_message` and pins it in `tests/all/manager_not_found_message.rs`; the behavioral check asserts the error CODE, so the sentence is recorded behavior only). `TagResource` and `UntagResource` bind the identical error set on the same URI and behave the same way. Nothing in the model declares a 401 or an `UnauthorizedException` shape for this service.

Emulator: the request never reaches the handler. `src/service.rs` registers `ListTagsForResource`, `TagResource`, `UntagResource` and `UpdateConfigurationDefinition` behind `TargetAuthorization`, whose `serves()` does the store's own `manager_in(&region, arn)` lookup and, on a miss, answers `unauthorized(...)` — HTTP 401 (`UNAUTHORIZED_STATUS`), wire error code `UnauthorizedException` (`UNAUTHORIZED_CODE`, also the `x-amzn-errortype` header), body `{"message": "Unauthorized"}` (`UNAUTHORIZED_MESSAGE`, with a lowercase `message` member, unlike this service's modeled `Message`). The 404 branch in `src/ops/list_tags_for_resource.rs` is consequently dead for every absent ARN. Replay: after the manager is deleted and `GetConfigurationManager` on it correctly reports `ResourceNotFoundException`, `ListTagsForResource` on the same ARN reports `UnauthorizedException` — "expected ListTagsForResource on deleted manager A to raise ResourceNotFoundException, got UnauthorizedException".

Why: a client cannot tell a deleted/absent resource from a credential or permission problem. Botocore/SDK callers branch on `ResourceNotFoundException` to treat a resource as gone (idempotent cleanup, delete-then-verify loops, tag reconciliation); a 401 with an unmodeled code is not in the generated error union, so it surfaces as a generic `ClientError`/`UnauthorizedException` that no retry or not-found branch matches, and 401 is additionally interpreted by SDK credential-refresh paths as an auth failure rather than a resource outcome. The same wrong class is served for TagResource and UntagResource on a non-existent ARN, so a post-delete mutation also reports the wrong refusal, and the lowercase `message` member breaks clients reading `Error.Message`.

Fix: the request-authorization stage in the ssm-quicksetup service crate (`src/service.rs`: `TargetAuthorization::refusal`/`serves`, `unauthorized`, `target_authorization_table`) must not convert resource non-existence into an authorization refusal. Resolution of the target ARN belongs to each operation's handler, which already returns the modeled `ResourceNotFoundException` (`src/ops/list_tags_for_resource.rs`, `src/ops/tag_resource.rs`, `src/ops/untag_resource.rs`, `src/ops/update_configuration_definition.rs`); the stage should pass such requests through so the modeled 404 is what a caller earns.

Traces: ssm-quicksetup.tag-resource-list-tags-roundtrip.

## Evidence
Replay verdict — [mismatch] ssm-quicksetup.tag-resource-list-tags-roundtrip:
```
File "check_ssm_quicksetup_tag_resource_list_tags_roundtrip.py", line 71, in run
  assert tag_code == "ResourceNotFoundException", (
AssertionError: expected ListTagsForResource on deleted manager A to raise ResourceNotFoundException, got UnauthorizedException
```
The check first proves the manager really is gone by polling `GetConfigurationManager` until it returns `ResourceNotFoundException` (that assertion passes), then calls `ListTagsForResource` on the same ARN.

Expected: HTTP 404, `Error.Code` = `ResourceNotFoundException`, `Error.Message` = `Quick setup manager with arn: <the ARN from the request> could not be found.` (fixed text around one request-supplied ARN).
Actual: HTTP 401, `Error.Code` / `x-amzn-errortype` = `UnauthorizedException`, body member lowercase `message` = `Unauthorized` (wholly fixed text, naming no member, value or resource).

Source of the wrong class — services/service-com-amazonaws-ssmquicksetup-quicksetup/src/service.rs:103-112:
```
pub const UNAUTHORIZED_CODE: &str = "UnauthorizedException";
pub const UNAUTHORIZED_STATUS: u16 = 401;
pub const UNAUTHORIZED_MESSAGE: &str = "Unauthorized";
```
service.rs:268-279 (`serves`) decides it purely on resource existence:
```
let region = crate::state::request_region(ctx.parts());
let mut store = state.lock();
store.settle(SystemTime::now());
store.manager_in(&region, arn).is_some()
```
service.rs:240-255 (`refusal`) returns `Some(unauthorized(svc))` for every miss, and service.rs:379-384 enrolls the three tag operations plus `UpdateConfigurationDefinition` in that table.

The modeled answer it preempts — src/ops/list_tags_for_resource.rs:174-183 returns `ResourceNotFoundException` with `crate::state::manager_not_found_message(&input.resource_arn)`; that branch is unreachable for an absent ARN.

The model does not declare the served shape — `smithy_query(query="operation", operation="ListTagsForResource")`: errors = AccessDeniedException(403), ConflictException(409), InternalServerException(500), ResourceNotFoundException(404), ThrottlingException(429), ValidationException(400); no 401 and no `UnauthorizedException`. `TagResource` (PUT /tags/{ResourceArn}) reports the same six.

Not covered by any open issue: 20261029T155211Z-ssm-quicksetup-listtagsforresource-error-response.md concerns eliding an oversized ARN *inside* the 404 message and assumes that 404 is served; 20261029T184830Z-ssm-quicksetup-4-net-new-root-causes-anchored.md cause 4 concerns the stage not enforcing an identity-policy Deny (a missing 403), the opposite direction from this cause's spurious 401.

## Steps to Reproduce
1. Build and boot the emulator from this checkout and sign requests for ssm-quicksetup.
2. Create a configuration manager: POST /configurationManager with a valid Name and one AWSQuickSetupType-SSMHostMgmt definition; keep the returned ManagerArn.
3. Tag it: PUT /tags/{ManagerArn} with {"Tags": {"owner": "manager-a"}}, and confirm GET /tags/{ManagerArn} reports it (both succeed today).
4. Delete it: DELETE /configurationManager/{ManagerArn}, then poll GET /configurationManager/{ManagerArn} until it answers 404 ResourceNotFoundException — the resource is now gone.
5. Call GET /tags/{ManagerArn} on that same, now-deleted ARN. Observe HTTP 401 with x-amzn-errortype: UnauthorizedException and body {"message": "Unauthorized"} instead of HTTP 404 ResourceNotFoundException with Message "Quick setup manager with arn: <that ARN> could not be found."
6. Same wrong refusal for a mutation on the dead id: PUT /tags/{ManagerArn} with {"Tags": {"owner": "post-delete"}} (TagResource) and DELETE /tags/{ManagerArn}?tagKeys=owner (UntagResource) — 401 UnauthorizedException rather than the modeled 404.
7. Also reproducible without any deletion: call GET /tags/arn:aws:ssm-quicksetup:<region>:<account>:configuration-manager/00000000-0000-0000-0000-000000000000 (an ARN never minted) — 401 UnauthorizedException instead of 404 ResourceNotFoundException. Contrast GET /configurationManager/{same ARN}, which is not behind the stage and correctly answers the modeled 404.
