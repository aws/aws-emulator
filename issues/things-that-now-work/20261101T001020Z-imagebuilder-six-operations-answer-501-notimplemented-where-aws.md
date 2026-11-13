Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: imagebuilder.list-components-pagination-invalid-token-rejected imagebuilder.list-tags-for-resource-not-found imagebuilder.put-component-policy-invalid-json-rejected imagebuilder.put-component-policy-nonexistent-component-not-found imagebuilder.put-image-recipe-policy-nonexistent-recipe-rejected imagebuilder.create-workflow-dryrun-no-mutation imagebuilder.create-image-pipeline-missing-recipe-arn-rejected
Issue-Shape-Id: com.amazonaws.imagebuilder#imagebuilder
Issue-Key: imagebuilder.list-components-pagination-invalid-token-rejected|run#3|AssertionError|assert code == "InvalidParameterValueException", f"expected InvalidParameterValueException, got {code!r}"
Issue-Key: imagebuilder.list-tags-for-resource-not-found|run#7|AssertionError|assert code == "NotFoundException", f"expected NotFoundException, got {code!r}"
Issue-Key: imagebuilder.put-component-policy-invalid-json-rejected|run#8|AssertionError|assert code == "InvalidParameterValueException", f"expected InvalidParameterValueException, got {code!r}"
Issue-Key: imagebuilder.put-component-policy-nonexistent-component-not-found|run#9|AssertionError|assert code == "ResourceNotFoundException", f"expected ResourceNotFoundException, got {code!r}"
Issue-Key: imagebuilder.put-image-recipe-policy-nonexistent-recipe-rejected|run#8|AssertionError|assert code == "ResourceNotFoundException", f"expected ResourceNotFoundException, got {code!r}"
Issue-Key: imagebuilder.create-workflow-dryrun-no-mutation|run#5|AssertionError|assert code == "DryRunOperationException", f"expected DryRunOperationException, got {code!r}"
Issue-Key: imagebuilder.create-image-pipeline-missing-recipe-arn-rejected|run#7|AssertionError|assert code == "MissingParametersException", f"expected MissingParametersException, got {code!r}"
Issue-Commit: e3579db6a1835cfcd1e7033eb2fb9f64dc458cca

# imagebuilder: six operations answer 501 NotImplemented where AWS refuses or answers — ListComponents, ListTagsForResource, PutComponentPolicy, PutImageRecipePolicy, CreateWorkflow dryRun, CreateImagePipeline

> _Note: this issue was identified against the code base at commit `e3579db6a1835cfcd1e7033eb2fb9f64dc458cca`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListComponents

## Description
SIX root causes, one per operation that is still a scaffold stub and answers 501 NotImplemented ("operation <Name> is scaffolded but not implemented in this emulator") to a request AWS answers or refuses with a specific, modeled error. Each cause states the refusal semantics the implemented operation owes, which the scaffold-stub work items do not; all six must be closed before this issue is submitted.
Scope note: separate auto-filed scaffold-stub issues exist for PutComponentPolicy, ListTagsForResource, CreateWorkflow and CreateImagePipeline (e.g. 20261031T221631Z-imagebuilder-createimagepipeline-stub.md); they name the operation and file but not the behavior below, and none of them covers ListComponents or PutImageRecipePolicy, which have no stub issue at all.

Cause 1 of 6: ListComponents does not refuse a malformed pagination token (and does not list at all).
AWS: Smithy: ListComponentsRequest.nextToken is a PaginationToken with length 1..65535 and the operation models InvalidPaginationTokenException (client, @httpError 400) as well as InvalidRequestException; the recorded refusal for a token that is not one the service minted is 400 InvalidParameterValueException (client, @httpError 400, "The value that you provided for the specified parameter is invalid.") with the message "The value supplied for parameter 'nextToken' is not valid. The provided token does not meet the required format." — fixed text "The value supplied for parameter '", "' is not valid. The provided token does not meet the required format."; the only variable part is the member name (nextToken). An unpaginated ListComponents(owner='Self') answers 200 with a componentVersionList.
Emulator: 501 NotImplemented from services/service-com-amazonaws-imagebuilder-imagebuilder/src/ops/list_components.rs line 199 (Scaffolded::not_implemented("ListComponents")), so neither the listing nor the refusal exists.
Why: 501 is not in any SDK's imagebuilder error union, is not retriable and is not catchable as a client error; paginating clients and every effect-absent assertion that lists after a refused create break.
Traces: imagebuilder.list-components-pagination-invalid-token-rejected.

Cause 2 of 6: ListTagsForResource does not report a well-formed-but-absent ARN as not found.
AWS: Smithy: ListTagsForResourceRequest.resourceArn is a required @httpLabel ImageBuilderArn with the multi-resource ARN pattern; the operation models ResourceNotFoundException (client, @httpError 404) and InvalidParameterException (400). The recorded wire error type for an ARN that was never created is NotFoundException (no such shape exists in the model — recorded behavior only) with the fixed message "Resource was not found" (no variable parts). The same call against an existing resource answers 200 with that resource's tags.
Emulator: 501 NotImplemented from the ListTagsForResource stub (services/.../src/ops/list_tags_for_resource.rs).
Why: tag-reading code cannot distinguish "absent resource" from "broken service", and no caller can read tags at all.
Traces: imagebuilder.list-tags-for-resource-not-found.

Cause 3 of 6: PutComponentPolicy neither validates the policy document nor reports an absent component.
AWS: Smithy: PutComponentPolicyRequest.componentArn (required, component build-version ARN pattern) and policy (required ResourcePolicyDocument, length 1..30000); the operation models InvalidParameterValueException (400) and ResourceNotFoundException (404). Recorded behavior: a policy that is not parseable JSON is refused 400 InvalidParameterValueException with "The value supplied for parameter 'policy' is not valid. Policy is malformed and cannot be parsed" (fixed text apart from the member name 'policy'), and the component is left with no attached policy — GetComponentPolicy then answers 200 with the placeholder policy "{}" rather than an error. A componentArn that does not exist is refused 404 ResourceNotFoundException with "The following required resource 'Component' cannot be found: '<componentArn>'." (fixed text "The following required resource '", "' cannot be found: '", "'."; variable parts the resource-type token and the ARN from the request). A well-formed policy against a real component succeeds and is reflected verbatim by GetComponentPolicy.
Emulator: 501 NotImplemented from the PutComponentPolicy stub (services/.../src/ops/put_component_policy.rs) for both the malformed-policy and the absent-component request.
Why: resource sharing cannot be exercised at all, and a client cannot tell a bad policy from a missing component from a dead service.
Traces: imagebuilder.put-component-policy-invalid-json-rejected, imagebuilder.put-component-policy-nonexistent-component-not-found.

Cause 4 of 6: PutImageRecipePolicy does not report an absent image recipe.
AWS: Smithy: PutImageRecipePolicyRequest.imageRecipeArn (required, image-recipe ARN pattern) and policy (required, 1..30000); the operation models ResourceNotFoundException (404) and InvalidParameterValueException (400). Recorded behavior: an imageRecipeArn that does not exist is refused 404 ResourceNotFoundException with "The following required resource 'ImageRecipe' cannot be found: '<imageRecipeArn>'." — same template as above with the resource-type token ImageRecipe and the ARN from the request. Against a real recipe the call succeeds and GetImageRecipePolicy reflects the stored document.
Emulator: 501 NotImplemented from the PutImageRecipePolicy stub (services/.../src/ops/put_image_recipe_policy.rs). No scaffold-stub issue exists for this operation.
Why: as above — recipe sharing is unavailable and the not-found condition is unobservable.
Traces: imagebuilder.put-image-recipe-policy-nonexistent-recipe-rejected.

Cause 5 of 6: CreateWorkflow with dryRun=true does not answer the dry-run refusal.
AWS: Smithy: CreateWorkflowRequest.dryRun is a Boolean defaulting to false and the operation models DryRunOperationException (client, @httpError 412, "The dry run operation of the resource was successful, and no resources or mutations were actually performed due to the dry run flag in the request."). Recorded behavior: a valid request with dryRun=true is answered DryRunOperationException with the fixed message "Request would have succeeded, but dryRun flag is set." (no variable parts) and nothing is persisted — ListWorkflows afterwards contains no workflow of that name.
Emulator: 501 NotImplemented from the CreateWorkflow stub (services/.../src/ops/create_workflow.rs), so the 412 dry-run contract and the no-mutation guarantee are both absent.
Why: dry-run is how callers validate a workflow document before committing; a 501 makes validation impossible and is indistinguishable from an outage.
Traces: imagebuilder.create-workflow-dryrun-no-mutation.

Cause 6 of 6: CreateImagePipeline does not refuse a request that names neither recipe.
AWS: exactly one of imageRecipeArn / containerRecipeArn must be supplied; omitting both is refused with the error code MissingParametersException (no such shape in the Smithy model — recorded behavior only) and the message "Required parameters are missing from the request: 'imageRecipeArn' or 'containerRecipeArn'. Verify that you have entered all of the required parameters in your request and try again." — entirely fixed text apart from the two member names it quotes. Nothing is created: ListImagePipelines filtered by that name is empty, and the same call with exactly one recipe ARN succeeds.
Emulator: 501 NotImplemented from the CreateImagePipeline stub (services/.../src/ops/create_image_pipeline.rs). The open stub issue 20261031T221631Z-imagebuilder-createimagepipeline-stub.md names the operation and file but not this required-parameter refusal.
Why: a caller that omits the recipe gets an un-catchable 501 instead of the modeled client error, so misconfiguration is reported as a service failure.
Fix (all causes): implement the six handlers in services/service-com-amazonaws-imagebuilder-imagebuilder/src/ops/ (list_components.rs, list_tags_for_resource.rs, put_component_policy.rs, put_image_recipe_policy.rs, create_workflow.rs, create_image_pipeline.rs) together with their validation and the state they read/write, so each returns the status, error type and message named above instead of the scaffold's 501.
Traces: imagebuilder.create-image-pipeline-missing-recipe-arn-rejected.

## Evidence
Cause 1: imagebuilder.list-components-pagination-invalid-token-rejected — ListComponents(nextToken='not-a-real-pagination-token==') expected 'InvalidParameterValueException' + "The value supplied for parameter 'nextToken' is not valid. The provided token does not meet the required format.", got 'NotImplemented'.
Cause 2: imagebuilder.list-tags-for-resource-not-found — ListTagsForResource(resourceArn=<well-formed component ARN never created>) expected 'NotFoundException' + 'Resource was not found', got 'NotImplemented'.
Cause 3: imagebuilder.put-component-policy-invalid-json-rejected — PutComponentPolicy(componentArn=<real component>, policy='{this is not valid json') expected 'InvalidParameterValueException' + "The value supplied for parameter 'policy' is not valid. Policy is malformed and cannot be parsed", got 'NotImplemented'. imagebuilder.put-component-policy-nonexistent-component-not-found — PutComponentPolicy(componentArn=<absent ARN>, policy=<valid JSON>) expected 'ResourceNotFoundException' + "The following required resource 'Component' cannot be found: '<arn>'.", got 'NotImplemented'.
Cause 4: imagebuilder.put-image-recipe-policy-nonexistent-recipe-rejected — PutImageRecipePolicy(imageRecipeArn=<absent ARN>, policy=<valid JSON>) expected 'ResourceNotFoundException' + "The following required resource 'ImageRecipe' cannot be found: '<arn>'.", got 'NotImplemented'.
Cause 5: imagebuilder.create-workflow-dryrun-no-mutation — CreateWorkflow(..., dryRun=True) expected 'DryRunOperationException' (412) + 'Request would have succeeded, but dryRun flag is set.', got 'NotImplemented'.
Cause 6: imagebuilder.create-image-pipeline-missing-recipe-arn-rejected — CreateImagePipeline(name=<unique>, infrastructureConfigurationArn=<real infra config>, clientToken=<uuid>) with neither imageRecipeArn nor containerRecipeArn expected 'MissingParametersException' + "Required parameters are missing from the request: 'imageRecipeArn' or 'containerRecipeArn'. Verify that you have entered all of the required parameters in your request and try again.", got 'NotImplemented'.
All six answer the scaffold body's message "operation <Name> is scaffolded but not implemented in this emulator"; list_components.rs line 199 is the representative call site.

## Steps to Reproduce
1. ListComponents(nextToken='not-a-real-pagination-token==') — expect 400 InvalidParameterValueException; then ListComponents(owner='Self') — expect 200 with componentVersionList.
2. ListTagsForResource(resourceArn='arn:aws:imagebuilder:<region>:<account>:component/<never-created-name>/1.0.0/1') — expect NotFoundException 'Resource was not found'; then the same call on a real component — expect its tags.
3. Create a component, then PutComponentPolicy(componentArn=<arn>, policy='{this is not valid json') — expect 400 InvalidParameterValueException; then GetComponentPolicy(componentArn=<arn>) — expect 200 with policy '{}'; then PutComponentPolicy with a valid policy and read it back verbatim. Also PutComponentPolicy against a well-formed but absent componentArn — expect 404 ResourceNotFoundException.
4. PutImageRecipePolicy(imageRecipeArn=<well-formed absent recipe ARN>, policy=<valid JSON>) — expect 404 ResourceNotFoundException; then the same against a real recipe followed by GetImageRecipePolicy.
5. CreateWorkflow(name=<unique>, semanticVersion='1.0.0', type='TEST', data=<valid workflow YAML>, dryRun=True, clientToken=<uuid>) — expect 412 DryRunOperationException and no such workflow in ListWorkflows.
6. CreateImagePipeline(name=<unique>, infrastructureConfigurationArn=<existing infra config>, clientToken=<uuid>) with no recipe ARN — expect MissingParametersException and an empty ListImagePipelines for that name; then repeat with imageRecipeArn supplied — expect success.
