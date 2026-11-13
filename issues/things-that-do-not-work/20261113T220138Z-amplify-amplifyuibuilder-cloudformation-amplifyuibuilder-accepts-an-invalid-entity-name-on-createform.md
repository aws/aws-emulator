Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: amplify__amplifyuibuilder__cloudformation.deploy-amplifyuibuilder-form-invalid-name-rejected-and-rolled-back amplify__amplifyuibuilder__cloudformation.deploy-amplifyuibuilder-theme-form-component-round-trip-and-delete
Issue-Key: amplify__amplifyuibuilder__cloudformation.deploy-amplifyuibuilder-form-invalid-name-rejected-and-rolled-back|run#22|AssertionError|assert status == "ROLLBACK_COMPLETE", f"rejected stack ended {status!r}"
Issue-Key: amplify__amplifyuibuilder__cloudformation.deploy-amplifyuibuilder-theme-form-component-round-trip-and-delete|run#33|AssertionError|assert theme["values"] == [
Issue-Commit: fa605e3beb5560d80c09dff54081711248679782

# AmplifyUIBuilder accepts an invalid entity name on CreateForm, and GetTheme omits the empty children list on a leaf theme value

> _Note: this issue was identified against the code base at commit `fa605e3beb5560d80c09dff54081711248679782`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateForm

## Description
Two net-new root causes found by the CloudFormation+Amplify+AmplifyUIBuilder deploy checks. Both are in the amplifyuibuilder surface of the emulator; the CloudFormation stack outcome is only how the first one becomes visible.

Cause 1 of 2: CreateForm does not refuse an entity name AmplifyUIBuilder considers invalid, so a CloudFormation stack that should fail its Form resource and roll back reaches CREATE_COMPLETE instead.
  AWS: Smithy (services/service-com-amazonaws-amplifyuibuilder-amplifyuibuilder/smithy-model/com-amazonaws-amplifyuibuilder-amplifyuibuilder.json) models CreateForm with errors InternalServerException (500), InvalidParameterException (400, "An invalid or out-of-range value was supplied for the input parameter"), ResourceConflictException (409), ServiceQuotaExceededException (402); CreateFormData$name targets FormName, a string with @length min 1 max 255 and NO pattern. The name rule beyond that length is recorded behavior only: the recording shows that a Form whose Name is the same identifier with a lowercase initial letter (the check builds good_form = bad_form with its first character upper-cased) is REFUSED, while the capitalised one is created and reads back. The refusal is HTTP 400 with the fixed message text "Entity name is not a valid name" — no part of that message is taken from the request. CloudFormation then surfaces it on the AWS::AmplifyUIBuilder::Form resource as a CREATE_FAILED row whose ResourceStatusReason is the registry-handler envelope `Resource handler returned message: "Entity name is not a valid name (Service: AmplifyUiBuilder, Status Code: 400, Request ID: <id>) (SDK Attempt Count: <n>)" (RequestToken: <token>, HandlerErrorCode: InternalFailure)` — fixed text is everything but the request id, attempt count and request token, which are per-run values — and the stack ends ROLLBACK_COMPLETE with no form of that name listed by ListForms under the app.
  Emulator: the create is accepted. CreateForm's handler validates only sectional-element type/level/orientation, the form-name conflict and the store quota (services/service-com-amazonaws-amplifyuibuilder-amplifyuibuilder/src/ops/create_form.rs), so a name AmplifyUIBuilder rejects is stored; the stack reaches CREATE_COMPLETE and the check fails with "rejected stack ended 'CREATE_COMPLETE'".
  Why: a client that relies on AWS refusing a malformed entity name gets a resource AWS would never have created, and a deployment that must roll back is reported as a successful one — negative-path tests and template validation pass against the emulator and fail against AWS, and the rolled-back-absence contract (ListForms returns no such form) is never exercised.
  Fix: CreateForm's validation (the name check belongs beside the existing sectional-element validation, in shared state so UpdateForm — and the same rule for CreateTheme/CreateComponent names — uses one implementation), answering the modelled InvalidParameterException (400) with the message AWS words. The AWS::AmplifyUIBuilder::Form resource path in the CloudFormation emulator must carry that refusal into a CREATE_FAILED row with the registry-handler reason above and roll the stack back.
  Traces: check amplify__amplifyuibuilder__cloudformation.deploy-amplifyuibuilder-form-invalid-name-rejected-and-rolled-back (assert at line 284 of the check: status == "ROLLBACK_COMPLETE"). Handler: services/service-com-amazonaws-amplifyuibuilder-amplifyuibuilder/src/ops/create_form.rs lines 160-224.

Cause 2 of 2: GetTheme drops the `children` member of a theme value that has none, where AWS reports it as an empty list.
  AWS: Smithy: Theme$values targets ThemeValuesList of ThemeValues{key, value: ThemeValue}; ThemeValue has members `value` (String) and `children` (ThemeValuesList), neither required, so the model does not state the rule — recorded behavior only. The recording shows that a theme created with values [{Key: font, Value: {Children: [{Key: primary, Value: {Value: "Amazon Ember"}}]}}] reads back from GetTheme as [{"key": "font", "value": {"children": [{"key": "primary", "value": {"value": "Amazon Ember", "children": []}}]}}]: a ThemeValue stored without children is reported WITH `children` present and empty, at every nesting depth, while a ThemeValue stored without `value` is not given one.
  Emulator: the member is omitted — GetTheme returned [{'key': 'font', 'value': {'children': [{'key': 'primary', 'value': {'value': 'Amazon Ember'}}]}}], so the leaf theme value carries no `children` key at all.
  Why: a client that walks a theme's value tree by reading `children` unconditionally (the natural recursion over AWS's own responses) raises a KeyError/absent-field error against the emulator while the same code works against AWS, and a client that compares a theme it just wrote against the theme AWS reports sees a shape mismatch.
  Fix: the Theme projection in crate::state (StoredTheme's to_theme, shared by GetTheme, CreateTheme, UpdateTheme and ListThemes) must materialise ThemeValue$children as an empty list rather than an absence when the stored value has no children, recursively.
  Traces: check amplify__amplifyuibuilder__cloudformation.deploy-amplifyuibuilder-theme-form-component-round-trip-and-delete (assert at line 644 of the check). Handler: services/service-com-amazonaws-amplifyuibuilder-amplifyuibuilder/src/ops/get_theme.rs line 176 (theme.to_theme()).

## Evidence
Cause 1 — check deploy-amplifyuibuilder-form-invalid-name-rejected-and-rolled-back:
  Expected (AWS): the stack containing only the Form whose Name has a lowercase initial letter ends 'ROLLBACK_COMPLETE'; the Form's CREATE_FAILED ResourceStatusReason masks to ("Entity name is not a valid name (Service: AmplifyUiBuilder, Status Code: 400, Request ID: <id>) (SDK Attempt Count: <n>)", "InternalFailure"); ListForms lists no form with that name.
  Actual (emulator): AssertionError: rejected stack ended 'CREATE_COMPLETE' (check line 284) — CreateForm returned 200 and the form was stored. The positive control (same Form, capitalised Name) passed, so nothing but the name differs between the accepted and the should-be-refused call.

Cause 2 — check deploy-amplifyuibuilder-theme-form-component-round-trip-and-delete:
  Expected (AWS): GetTheme theme['values'] == [{'key': 'font', 'value': {'children': [{'key': 'primary', 'value': {'value': 'Amazon Ember', 'children': []}}]}}]
  Actual (emulator): [{'key': 'font', 'value': {'children': [{'key': 'primary', 'value': {'value': 'Amazon Ember'}}]}}]
  AssertionError: GetTheme returned values [...], expected the declared nested Values (check line 644). Only the innermost ThemeValue's `children: []` is missing; every other asserted member of the theme, form and component round-tripped.

## Steps to Reproduce
Cause 1:
1. CreateStack with a template holding AWS::Amplify::App plus AWS::AmplifyUIBuilder::Form (AppId = GetAtt App.AppId, EnvironmentName staging, Name = an identifier with an UPPERCASE initial letter, DataType DataStore/Post, FormActionType create, SchemaVersion 1.0, one field, Style gaps, one Heading sectional element). Wait for CREATE_COMPLETE — this is the control and it passes.
2. CreateStack a second stack with only that same Form under the same AppId, with Name = the identical identifier but with a LOWERCASE initial letter.
3. DescribeStacks the second stack: AWS ends ROLLBACK_COMPLETE with the Form's CREATE_FAILED reason quoting "Entity name is not a valid name ... Status Code: 400 ... HandlerErrorCode: InternalFailure"; the emulator ends CREATE_COMPLETE.
4. Equivalently, call amplifyuibuilder CreateForm directly with formToCreate.name starting with a lowercase letter: AWS answers 400 InvalidParameterException "Entity name is not a valid name"; the emulator answers 200 with the created form.

Cause 2:
1. CreateStack with AWS::Amplify::App plus AWS::AmplifyUIBuilder::Theme whose Values = [{Key: font, Value: {Children: [{Key: primary, Value: {Value: "Amazon Ember"}}]}}] (EnvironmentName staging).
2. After CREATE_COMPLETE, ListThemes to get the theme id, then GetTheme(appId, environmentName='staging', id).
3. Inspect theme['values'][0]['value']['children'][0]['value']: AWS reports {'value': 'Amazon Ember', 'children': []}; the emulator reports {'value': 'Amazon Ember'} with no children member.
