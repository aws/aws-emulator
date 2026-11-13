Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: amplify__amplifyuibuilder__cloudformation.deploy-amplifyuibuilder-form-invalid-name-rejected-and-rolled-back amplify__amplifyuibuilder__cloudformation.deploy-amplifyuibuilder-theme-form-component-round-trip-and-delete
Issue-Key: amplify__amplifyuibuilder__cloudformation|CreateStack|ValidationError
Issue-Commit: 86c25e3173c48b1c4d4a9173324e183693bfe876

# amplify__amplifyuibuilder__cloudformation: AWS::AmplifyUIBuilder::{Component,Form,Theme} are not registered resource types

> _Note: this issue was identified against the code base at commit `86c25e3173c48b1c4d4a9173324e183693bfe876`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Cause 1 of 1: AWS::AmplifyUIBuilder::Component, ::Form and ::Theme are missing from the CloudFormation template resource-type registry.

AWS: recorded behavior only — CreateStack accepts all three types, provisions them through Amplify UI Builder against the app/environment the template names, and reaches CREATE_COMPLETE; a Form whose Name violates the documented pattern instead rolls the stack back with Amplify UI Builder's own refusal as the resource failure reason.
Emulator: CreateStack answers 400 ValidationError `Template format error: Unrecognized resource types: [AWS::AmplifyUIBuilder::Form]` / `[AWS::AmplifyUIBuilder::Theme, AWS::AmplifyUIBuilder::Form, AWS::AmplifyUIBuilder::Component]`, so nothing is created and the invalid-Name refusal is never reached.
Why: no Amplify UI Builder component, form or theme can be deployed from a template; the user is told these AWS resource types do not exist.
Fix: register the three types in the CloudFormation template resource-type registry and add a resource handler per type over the amplifyuibuilder crate, including the Name pattern check on create.
Traces: the 2 checks in trace_ids.

## Evidence
Expected: CreateStack 200 + StackId and CREATE_COMPLETE (ROLLBACK_COMPLETE citing the Name refusal for the invalid-name leg).
Actual: botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the CreateStack operation: Template format error: Unrecognized resource types: [AWS::AmplifyUIBuilder::Form] / [AWS::AmplifyUIBuilder::Theme, AWS::AmplifyUIBuilder::Form, AWS::AmplifyUIBuilder::Component]

## Steps to Reproduce
1. CreateStack with a template declaring AWS::AmplifyUIBuilder::Component, ::Form or ::Theme.
2. Observe the 400 ValidationError "Unrecognized resource types".
3. Re-replay the 2 checks in trace_ids.
