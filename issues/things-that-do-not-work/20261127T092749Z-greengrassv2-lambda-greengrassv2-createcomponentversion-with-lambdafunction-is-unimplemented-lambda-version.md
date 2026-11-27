Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: greengrassv2__lambda.lambda-import-component-takes-function-name-and-version greengrassv2__lambda.lambda-import-unpublished-version-rejected
Issue-Key: greengrassv2|CreateComponentVersion|NotImplemented
Issue-Commit: 47d3968c23878270cbb97f1f35a0ca51e5d34ce1

# greengrassv2 CreateComponentVersion with lambdaFunction is unimplemented: Lambda-version import returns NotImplemented instead of synthesizing the aws.greengrass.lambda component

> _Note: this issue was identified against the code base at commit `47d3968c23878270cbb97f1f35a0ca51e5d34ce1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateComponentVersion

## Description
2 causes to close. Both are reached through CreateComponentVersion's `lambdaFunction` input branch, which the emulator does not implement.

Cause 1 of 2: importing a published Lambda function version as a Greengrass component is refused as unimplemented. Failing checks: greengrassv2__lambda.lambda-import-component-takes-function-name-and-version, greengrassv2__lambda.lambda-import-unpublished-version-rejected.
AWS: Smithy: CreateComponentVersion POST /greengrass/v2/createComponentVersion, success code 201; input is the union of `inlineRecipe` (blob) and `lambdaFunction` (LambdaFunctionRecipeSource, whose only required member is `lambdaArn`, NonEmptyString @length min 1, documented as "The ARN of the Lambda function. The ARN must include the version of the function to import. You can't use version aliases like $LATEST"); LambdaFunctionRecipeSource also carries optional componentName ("Defaults to the name of the Lambda function"), componentVersion ("Defaults to the version of the Lambda function as a semantic version. For example, if your function version is 3, the component version becomes 3.0.0"), componentPlatforms, componentDependencies and componentLambdaParameters. Output members arn/componentName/componentVersion/creationTimestamp/status are required. Recorded behavior: given a published version ARN, the call returns 201 with componentName = the Lambda function's name and componentVersion = "<function version>.0.0"; the component reaches componentState DEPLOYABLE, and GetComponent(recipeOutputFormat='JSON') returns a recipe whose ComponentName/ComponentVersion are those two values, ComponentType is `aws.greengrass.lambda`, ComponentSource is the imported version ARN, ComponentDescription is the published version's Description, Lifecycle.setenv carries AWS_GREENGRASS_LAMBDA_FUNCTION_HANDLER = the function's Handler and AWS_GREENGRASS_LAMBDA_RUNTIME = its Runtime, and the manifests' Artifacts carry one entry with Algorithm "SHA-256" and Digest = the function's CodeSha256. The names, versions and digests are values taken from the Lambda function and the request; the member names, the ComponentType literal, the `<version>.0.0` rule and the SHA-256 algorithm are the requirement.
Emulator: HTTP error with code `NotImplemented` and message "operation CreateComponentVersion with lambdaFunction is scaffolded but not implemented in this emulator" — the inlineRecipe branch works, the lambdaFunction branch does not, and no component is created.
Why: the documented Lambda-import path of Greengrass V2 is unusable. A caller migrating Lambda functions to Greengrass components gets an unmodeled `NotImplemented` code that is in no error list, so even its error handling cannot be written, and nothing downstream (ListComponentVersions, GetComponent, deployment of the imported component) can be exercised.
Fix: CreateComponentVersion's handler in the greengrassv2 service crate — the `lambdaFunction` branch that currently returns the scaffold NotImplemented: resolve the lambdaArn against Lambda state, derive componentName/componentVersion (request overrides, else function name and `<version>.0.0`), synthesize and store the aws.greengrass.lambda recipe (ComponentSource, ComponentDescription, Lifecycle.setenv, SHA-256 artifact digest) in component state, and return the 201 output with the component ARN and status.

Cause 2 of 2: importing a Lambda version that was never published must be refused, and must leave no component version behind. Failing check: greengrassv2__lambda.lambda-import-unpublished-version-rejected.
AWS: Smithy: ValidationException (httpError 400, required `message`, doc "The request isn't valid.") is a modeled error of CreateComponentVersion. Recorded behavior: create_component_version(lambdaFunction={'lambdaArn': '<unqualified function ARN>:<a version that was never published>'}) is refused with error code `ValidationException` and message `Invalid Input: Validation Error: {<lambdaArn>=No corresponding Lambda function found for specified ARN}` — fixed text `Invalid Input: Validation Error: {`, then the lambdaArn copied verbatim from the request, then `=No corresponding Lambda function found for specified ARN}`; the only variable part is the ARN from the request. After the refusal the function's component lists only the versions that were actually imported (e.g. just the one published version), so the refused import stores nothing.
Emulator: the same `NotImplemented` scaffold error as Cause 1, so this refusal class does not exist and nothing distinguishes an unresolvable Lambda ARN from a valid one.
Why: a caller that typo'd or pointed at an unpublished version sees an unmodeled NotImplemented instead of ValidationException with the ARN it passed, and cannot branch on the documented validation failure.
Fix: the lambdaArn resolution step of CreateComponentVersion's lambdaFunction branch (validation, before any state mutation) must raise ValidationException with that sentence when the ARN resolves to no Lambda function version, and must not create a component version.

Traces: the two failing checks named above.

## Evidence
greengrassv2__lambda.lambda-import-component-takes-function-name-and-version:
  botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateComponentVersion operation: operation CreateComponentVersion with lambdaFunction is scaffolded but not implemented in this emulator
  raised by gg.create_component_version(lambdaFunction={'lambdaArn': <published version ARN>}, tags={...})
  expected: HTTP 201 with componentName == the Lambda function name and componentVersion == '<published version>.0.0'; then componentState DEPLOYABLE and GetComponent(recipeOutputFormat='JSON') yielding ComponentType 'aws.greengrass.lambda', ComponentSource == the version ARN, ComponentDescription == the version's Description, Lifecycle.setenv.AWS_GREENGRASS_LAMBDA_FUNCTION_HANDLER == Handler, Lifecycle.setenv.AWS_GREENGRASS_LAMBDA_RUNTIME == Runtime, Artifacts == [{'Algorithm': 'SHA-256', 'Digest': <CodeSha256>}]

greengrassv2__lambda.lambda-import-unpublished-version-rejected:
  botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateComponentVersion operation: operation CreateComponentVersion with lambdaFunction is scaffolded but not implemented in this emulator
  expected for the published version: 201, componentName == function name, componentVersion == '1.0.0'
  expected for lambdaArn '<unqualified ARN>:2' (never published): error code 'ValidationException', message 'Invalid Input: Validation Error: {<unqualified ARN>:2=No corresponding Lambda function found for specified ARN}' (the ARN is the value from the request; the rest is fixed text)
  expected afterwards: ListComponentVersions(arn='arn:aws:greengrass:<region>:<account>:components:<function name>') lists exactly ['1.0.0']
Actual in both cases: the unmodeled error code `NotImplemented` with the scaffold message; no component version is created and no modeled error is ever returned.

## Steps to Reproduce
Boot the emulator and use iam, lambda and greengrassv2 clients in one region/account.
1. iam.create_role(<role>, trust policy for lambda.amazonaws.com).
2. lambda.create_function(FunctionName=<fn>, Runtime='python3.11', Role=<role arn>, Handler='app.handler', Code={'ZipFile': <zip with app.py>}); wait function_active_v2.
3. v1 = lambda.publish_version(FunctionName=<fn>); update_function_configuration(Description='second published version'); wait function_updated_v2; v2 = lambda.publish_version(FunctionName=<fn>).
4. Cause 1: greengrassv2.create_component_version(lambdaFunction={'lambdaArn': v2['FunctionArn']}, tags={'sim2real': 'x'}).
   Expect HTTP 201 with componentName == <fn> and componentVersion == f"{v2['Version']}.0.0"; then describe_component(arn=...) reaching DEPLOYABLE and get_component(arn=..., recipeOutputFormat='JSON') returning the aws.greengrass.lambda recipe described above.
   Observe: ClientError NotImplemented "operation CreateComponentVersion with lambdaFunction is scaffolded but not implemented in this emulator".
5. Cause 2: greengrassv2.create_component_version(lambdaFunction={'lambdaArn': f"{<unqualified function ARN>}:2"}) where version 2 was never published.
   Expect error code ValidationException with message 'Invalid Input: Validation Error: {<that ARN>=No corresponding Lambda function found for specified ARN}', and list_component_versions(arn='arn:aws:greengrass:<region>:<account>:components:<fn>') still listing only the versions actually imported.
   Observe: the same NotImplemented scaffold error.
