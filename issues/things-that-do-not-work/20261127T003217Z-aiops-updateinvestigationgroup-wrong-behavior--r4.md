Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 91a813c65f84129772cec88e1a94bd21614e47f6

# UpdateInvestigationGroup that omits isCloudTrailEventHistoryEnabled keeps the stored value, though the member documents its omission default

_Filed: 2026-11-27T00:32:17Z_
_Operation: UpdateInvestigationGroup | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `91a813c65f84129772cec88e1a94bd21614e47f6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`UpdateInvestigationGroupRequest$isCloudTrailEventHistoryEnabled` documents what the service does when the request omits the member: "The default is `true`." The handler does the opposite — it writes the member only when the request carries it, so a group previously updated (or created) with `false` keeps reporting `false` after an update that names no `isCloudTrailEventHistoryEnabled` at all.

The crate contradicts itself three ways here.

1. The handler's own comment asserts, as the rule it follows, that "a member the request omits keeps the stored value UNLESS the member's own documentation states what the service does when the parameter is omitted, in which case the write stores that documented value" — and then states, as fact, that `isCloudTrailEventHistoryEnabled` is NOT such a member ("`roleArn`, `tagKeyBoundaries`, `chatbotNotificationChannel`, `isCloudTrailEventHistoryEnabled` and `crossAccountConfigurations` each document only what naming them does"). The model says otherwise: this member's doc on THIS operation's input carries "The default is `true`." `roleArn`, `tagKeyBoundaries` and `crossAccountConfigurations` genuinely document no omission default; this one does, so the comment's own rule selects it and the code under the comment skips it.

2. `crate::state` already OWNS that documented default as a named constant whose doc quotes the very sentence (`DEFAULT_CLOUD_TRAIL_EVENT_HISTORY_ENABLED`, "From that member's own documentation: 'The default is true.'"), and the create path applies it. The update path never reads the constant, so the two writers of one member disagree about what an omitted member means — the same split the crate deliberately removed for `encryptionConfiguration`, which the SAME handler now applies unconditionally through the single-homed `stored_encryption_configuration` rule a few lines above.

3. The handler's own doc block reproduces the sentence it then ignores ("`isCloudTrailEventHistoryEnabled` (Boolean) — ... The default is `true`.").

Note for the fixer: `tests/all/investigation_groups.rs:716-724` currently asserts the opposite behaviour, lumping this member together with `roleArn` under the reason "was not patched, so it must survive". That reason holds for `roleArn` (whose doc states no omission default) and is contradicted by the model for `isCloudTrailEventHistoryEnabled`, so that row has to move with the fix — exactly as the `encryptionConfiguration` omission row did.

## Evidence

Smithy, quoted verbatim — `smithy_query(query="doc",shape="UpdateInvestigationGroupRequest",member="isCloudTrailEventHistoryEnabled")`:
"Specify <code>true</code> to enable CloudWatch investigations to have access to change events that are recorded by CloudTrail. The default is <code>true</code>."

The same sentence is carried in the handler file itself — services/service-com-amazonaws-aiops-aiops/src/ops/update_investigation_group.rs (modelled-contract block, `## Input`):
"//! * `isCloudTrailEventHistoryEnabled` (Boolean) — Specify true to enable CloudWatch
//!   investigations to have access to change events that are recorded by CloudTrail. The
//!   default is true."

The handler's rule, and the FALSE claim about this member — services/service-com-amazonaws-aiops-aiops/src/ops/update_investigation_group.rs:293-303:
```
        // Each member the request carries replaces the stored value; a member the
        // request omits keeps the stored value UNLESS the member's own documentation
        // states what the service does when the parameter is omitted, in which case the
        // write stores that documented value — the member says what an omission means,
        // so an omission is an instruction like any other. `roleArn`,
        // `tagKeyBoundaries`, `chatbotNotificationChannel`,
        // `isCloudTrailEventHistoryEnabled` and `crossAccountConfigurations` each
        // document only what naming them does, so omitting them changes nothing; the
        // one member of this request that documents its omission is
        // `encryptionConfiguration`, handled below.
```

The code that breaks it — services/service-com-amazonaws-aiops-aiops/src/ops/update_investigation_group.rs:327-329:
```
        if let Some(enabled) = input.is_cloud_trail_event_history_enabled {
            group.is_cloud_trail_event_history_enabled = Some(enabled);
        }
```

The default the crate already owns, with the quoted sentence — services/service-com-amazonaws-aiops-aiops/src/state.rs:86-89:
```
/// Applied when `CreateInvestigationGroup` omits `isCloudTrailEventHistoryEnabled`.
///
/// From that member's own documentation: "The default is true."
pub const DEFAULT_CLOUD_TRAIL_EVENT_HISTORY_ENABLED: bool = true;
```

The sibling writer that applies it — services/service-com-amazonaws-aiops-aiops/src/ops/create_investigation_group.rs:388-406:
```
            // `retentionInDays` and `isCloudTrailEventHistoryEnabled` each document the
            // value the service applies when the request omits them, so the stored
            // group — and every later read of it — carries that value, not nothing.
...
            is_cloud_trail_event_history_enabled: Some(
                input
                    .is_cloud_trail_event_history_enabled
                    .unwrap_or(crate::state::DEFAULT_CLOUD_TRAIL_EVENT_HISTORY_ENABLED),
            ),
```

The member of the SAME request that IS handled by the documented-omission rule, three lines earlier — services/service-com-amazonaws-aiops-aiops/src/ops/update_investigation_group.rs:317-319:
```
        group.encryption_configuration = Some(crate::state::stored_encryption_configuration(
            input.encryption_configuration,
        ));
```

Test coverage that must move with the fix — services/service-com-amazonaws-aiops-aiops/tests/all/investigation_groups.rs:716-724:
```
        for member in ["roleArn", "isCloudTrailEventHistoryEnabled"] {
            if !patched.contains_key(member) {
                assert_eq!(
                    after.get(member),
                    before.get(member),
                    "case {patch}: {member} was not patched, so it must survive"
                );
```
The create-side default IS pinned, with the same quoted sentence — services/service-com-amazonaws-aiops-aiops/tests/all/investigation_groups.rs:232: `// used." \`isCloudTrailEventHistoryEnabled\`: "The default is true."`

## Steps to Reproduce

1. POST /investigationGroups signed for us-west-2 with {"name":"ctrail","roleArn":"arn:aws:iam::000000000000:role/AIOpsAssistantRole","isCloudTrailEventHistoryEnabled":false} -> 201, note the returned arn
2. GET /investigationGroups/<url-encoded arn> -> isCloudTrailEventHistoryEnabled is false, as created
3. PATCH /investigationGroups/<url-encoded arn> with a body that names no isCloudTrailEventHistoryEnabled at all, e.g. {"tagKeyBoundaries":["Team"]} -> 200
4. GET /investigationGroups/<url-encoded arn> -> isCloudTrailEventHistoryEnabled is still false, although the member's own documentation for THIS operation says "The default is true." and the handler's own comment at update_investigation_group.rs:293-303 states that a member documenting its omission must be written with the documented value
5. Compare with CreateInvestigationGroup, which routes the same omitted member through crate::state::DEFAULT_CLOUD_TRAIL_EVENT_HISTORY_ENABLED (create_investigation_group.rs:402-406), and with the encryptionConfiguration member of the same PATCH, which is written unconditionally through the single-homed rule (update_investigation_group.rs:317-319)
