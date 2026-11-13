Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: cd1ff1c075c03af4b96f1d8fe21bfc329bc355d4

# CreateMetadataTransferJob builds its refusals from unbounded caller text without the clamp the crate says every refusal uses, so ValidationException/ResourceNotFoundException.message leaves the modelled @length(0..2048)

_Filed: 2026-11-01T20:34:27Z_
_Operation: CreateMetadataTransferJob | Class: error-response_

> _Note: this issue was identified against the code base at commit `cd1ff1c075c03af4b96f1d8fe21bfc329bc355d4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every modelled refusal of this service states its reason on the `ErrorMessage` shape, which is `@length(min: 0, max: 2048)`. `CreateMetadataTransferJob` builds two of its refusal messages by interpolating caller-supplied text that the model bounds AS WIDELY AS — or more widely than — that member, and it is the one refusal path in the crate that does not run the result through `crate::state::clamped_message`:

1. The workspace-not-found refusal interpolates `sources[].iotTwinMakerConfiguration.workspace` / `destination.iotTwinMakerConfiguration.workspace`, whose target `TwinMakerArn` is `@length(min: 20, max: 2048)`. `workspace_not_found_message` prefixes 33 fixed characters ("Unable to find workspace for id: "), so a request naming a 2048-character workspace ARN that the store does not hold answers a `ResourceNotFoundException` whose `message` is 2081 characters — 33 past the maximum the member admits.

2. The out-of-enum type refusals interpolate `sources[i].type` / `destination.type` verbatim. `SourceType` and `DestinationType` are plain enum strings with NO `@length`, and this crate states that the boundary validator does not enforce enum value lists (which is exactly why this handler checks them itself), so the interpolated text is unbounded and the message can be arbitrarily long.

The crate already owns the fix and states the rule: `clamped_message` exists for precisely "an identifier the model bounds as widely as — or more widely than — the member the message travels on", and the sibling `DeleteComponentType` says its clamped refusal is clamped "as every other refusal of this service is". This handler is the counter-example. The boundary validator does not police output, so the over-long message reaches the wire.

## Evidence

THE MODEL — `smithy_query(query="shape", shape="ErrorMessage,ResourceNotFoundException,ValidationException")`:
```
"ErrorMessage": {"shape_id": "com.amazonaws.iottwinmaker#ErrorMessage", "type": "string", "length": {"min": 0, "max": 2048}}
"ResourceNotFoundException": {... "members": {"message": {"target": "ErrorMessage", "length": {"min": 0, "max": 2048}}}}
"ValidationException": {... "members": {"message": {"target": "ErrorMessage", "length": {"min": 0, "max": 2048}}}}
```
THE MEMBER THE MESSAGE INTERPOLATES — `smithy_query(query="shape", shape="IotTwinMakerSourceConfiguration")`:
```
"workspace": {"target": "TwinMakerArn", "required": true, "length": {"min": 20, "max": 2048}, "pattern": "^arn:((aws)|(aws-cn)|(aws-us-gov)):iottwinmaker:[a-z0-9-]+:[0-9]{12}:[\\/a-zA-Z0-9_\\-\\.:]+$"}
```
and `smithy_query(query="shape", shape="SourceConfiguration")` shows the type member carries no length at all:
```
"type": {"target": "SourceType", "required": true, "doc": "<p>The source configuration type.</p>"}
```

THE UNCLAMPED REFUSALS — services/service-com-amazonaws-iottwinmaker-awsiottwinmaker/src/ops/create_metadata_transfer_job.rs:255-266:
```
        for workspace in referenced_workspaces(&input) {
            if state.resolve_workspace_id(workspace).is_none() {
                return Err(
                    CreateMetadataTransferJobErrorModeled::ResourceNotFoundException(
                        ResourceNotFoundException {
                            message: Some(crate::state::workspace_not_found_message(workspace)),
                        },
                    )
                    .into(),
                );
```
and src/ops/create_metadata_transfer_job.rs:376-381:
```
        other => {
            return Some(format!(
                "sources[{index}] has type {other}, which is not one of {}",
                SOURCE_TYPE_VALUES.join(", ")
            ));
        }
```
(the same construction for the destination at src/ops/create_metadata_transfer_job.rs:410-415, and `configuration_error` interpolates `kind` at src/ops/create_metadata_transfer_job.rs:433 and :437).

THE MESSAGE BUILDER THAT ADDS THE FIXED PREFIX — src/state.rs:4920-4922:
```
pub fn workspace_not_found_message(workspace_id: &str) -> String {
    format!("Unable to find workspace for id: {workspace_id}")
}
```

THE CRATE'S OWN RULE, WHICH THIS HANDLER BREAKS — src/ops/delete_component_type.rs:236-249:
```
        // What is built on the component type is deleted before the component
        // type itself, so nothing stored is left instantiating or extending a
        // type the store no longer holds. The refusal names as many ids as
        // reference it, so it is stated within the bound the member it travels
        // on declares ([`crate::state::clamped_message`]), as every other
        // refusal of this service is.
...
                    message: Some(crate::state::clamped_message(
                        message,
                        crate::state::ERROR_MESSAGE_MAX_CHARS,
                    )),
```
and src/state.rs:4742-4746 + 4754-4760:
```
/// The greatest number of characters the modelled `ErrorMessage` shape — the
/// member every one of this service's modelled refusals states its reason on —
/// admits: `@length(0..2048)`, from
/// `smithy_query(query="shape", shape="ErrorMessage")`.
pub const ERROR_MESSAGE_MAX_CHARS: usize = 2048;
...
/// A message names what the request named, and an identifier the model bounds
/// as widely as — or more widely than — the member the message travels on
/// renders text past that member's own bound: a `ComponentPath` of 2048
/// characters, or a `Name` of 256, stated on a member capped at 256.
```

THAT THE VALIDATOR DOES NOT CATCH THE ENUM TEXT — src/ops/list_metadata_transfer_jobs.rs:226-231:
```
        // The ends the listing is taken over. Both are modelled enumerations,
        // whose value lists the boundary validator does not enforce (the same
        // reason `UpdatePricingPlan` checks its `pricingMode` here), so a value
        // outside them is refused rather than quietly selecting no job.
```

TEST COVERAGE: `tests/all/error_message_bounds.rs:45-48` states the bound (`pub(crate) const ERROR_MESSAGE_MAX: usize = 2048;`) and its header at :33-34 says the rows cover "WHICH operation reports it (the five operations that accept a `componentPath`)" — `CreateMetadataTransferJob` is not one of them. `tests/all/not_found_constructions.rs:87-98` pins the "Unable to find workspace for id: " construction for ten operations, all of which take a `workspaceId` bounded at 128 characters; `CreateMetadataTransferJob` is absent from that table, so nothing pins its message against the member's maximum.

## Steps to Reproduce

1. CreateWorkspace(workspaceId="ws") so the Region's store exists.
2. Build a syntactically legal but unheld workspace ARN of exactly 2048 characters: `arn:aws:iottwinmaker:us-east-1:123456789012:` followed by enough `a` characters to reach 2048 (the `TwinMakerArn` pattern admits `[\/a-zA-Z0-9_\-\.:]+` and `@length` admits up to 2048).
3. CreateMetadataTransferJob(sources=[{"type": "iottwinmaker", "iotTwinMakerConfiguration": {"workspace": "<that 2048-char ARN>"}}], destination={"type": "s3", "s3Configuration": {"location": "arn:aws:s3:::bucket"}}).
4. Observe HTTP 404 `ResourceNotFoundException` whose `message` is 2081 characters — past the `@length(0..2048)` of `ErrorMessage`, the member it travels on — because src/ops/create_metadata_transfer_job.rs:260 does not pass it through `crate::state::clamped_message`, unlike src/ops/delete_component_type.rs:246.
5. Second symptom: CreateMetadataTransferJob(sources=[{"type": "<4000 arbitrary characters>"}], destination={"type": "s3", "s3Configuration": {...}}) — `SourceType` carries no `@length`, so the 400 `ValidationException` message interpolates all 4000 characters at src/ops/create_metadata_transfer_job.rs:378.
