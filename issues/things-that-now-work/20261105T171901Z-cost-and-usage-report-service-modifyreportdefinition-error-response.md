Issue-Severity: LOW
Issue-Gap-Class: error-response
Issue-Commit: 6ba61591396ce6cba86cdf0a33b2c5ad334488a5

# Rename-collision refusal puts the DuplicateReportNameException class doc on the wire with the requested name spliced in, which the sibling create path's own comment says must not happen

_Filed: 2026-11-05T17:19:01Z_
_Operation: ModifyReportDefinition | Class: error-response_

> _Note: this issue was identified against the code base at commit `6ba61591396ce6cba86cdf0a33b2c5ad334488a5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Two handlers in this crate answer the SAME resource-state condition — "the account already holds a report under this name" — in contradictory ways, and each states a rule the other breaks.

`PutReportDefinition` detects the condition with `store.position(..).is_some()` and deliberately refuses with NO message, and the comment above it states the rule: the modelled exception's only member is an optional `Message`, the error shape's documentation "describes the error class rather than being text the service sends", and "the requested name is not echoed back". `crate::state::report_not_found` carries the same rule for the tag plane, and the crate's `refusal_envelope.rs` suite pins it as the service-wide rule for a resource-state refusal (code + status alone, nothing from the request echoed).

`ModifyReportDefinition` detects the same condition with the same `store.position(..)` lookup and refuses with a `ValidationException` whose message is the `DuplicateReportNameException` shape documentation reproduced verbatim ("A report with the specified name already exists in the account. Specify a different report name.") with the requested name substituted for "the specified name". So the one sentence this crate's create path declares is model prose and not wire text is exactly what the update path sends, together with the report name the create path refuses to echo.

One of the two paths has to be wrong: they cannot both be right about the same sentence and the same name. The update path does have a documented need for a message (`src/state.rs:61-78` explains the update `ValidationException` multiplexes conditions, so the message is the discriminator), but that reason says nothing about using the model's error-class doc or about echoing the request, which is the part the create path's comment forbids. Nothing pins the current text: the only test over this branch asserts the 400 and the error code and never looks at the message.

## Evidence

The rule, stated in this crate at services/service-com-amazonaws-costandusagereportservice-awsorigamiservicegatewayservice/src/ops/put_report_definition.rs:165-174:
```
        if store.position(&report_name).is_some() {
            // No `Message`: the modelled exception's only member is an optional
            // `Message`, and the error shape's documentation describes the error class
            // rather than being text the service sends, so this refusal is its code and
            // status alone and the requested name is not echoed back.
            return Err(PutReportDefinitionErrorModeled::DuplicateReportNameException(
                DuplicateReportNameException { message: None },
            )
            .into());
        }
```

The same rule for the tag plane, src/state.rs:38-50:
```
/// Built with NO `Message`. The modelled exception's only member is an OPTIONAL
/// `Message`, documented as "A message to show the detail of the exception"
/// ... so the refusal carries its error code and HTTP status alone, and nothing from the
/// request (least of all the report name) is echoed back in it.
```

And as the service-wide rule in tests/all/refusal_envelope.rs:5-10:
```
//! The rule under test: this service's modelled exceptions declare a single OPTIONAL
//! `Message` ... so a refusal that reports a resource-state condition is serialized with
//! its error code and HTTP status alone, no `Message` member at all, and nothing from the
//! request echoed back into it.
```

What the update path does with the identical condition, src/ops/modify_report_definition.rs:166-180:
```
        // A rename may not collide with another report: report names are the resource's
        // identity, so two reports cannot share one.
        let new_name = input.report_definition.report_name.clone();
        let renames_onto_another = store.position(&new_name).is_some_and(|other| other != at);
        if renames_onto_another {
            return Err(ModifyReportDefinitionErrorModeled::ValidationException(
                ValidationException {
                    message: Some(format!(
                        "A report with the name {new_name} already exists in the account. \
                         Specify a different report name."
                    )),
                },
            )
            .into());
        }
```

That sentence is the model's error-class documentation — smithy_query(query="doc", shape="DuplicateReportNameException"): "A report with the specified name already exists in the account. Specify a different report name." — i.e. the very text the create path's comment calls "the error shape's documentation ... rather than being text the service sends".

No test pins it: tests/all/report_definition_lifecycle.rs:693-706 (`modify_refuses_a_name_the_account_does_not_hold_or_a_colliding_rename`) asserts only `status == 400` and `error_code(&body) == "ValidationException"` for the row "rename onto a name another report holds" (line 687-690); tests/all/refusal_envelope.rs:244-293 (`no_refusal_echoes_the_requested_report_name`) probes PutReportDefinition and the three tag operations only, never ModifyReportDefinition; tests/all/refusal_message_text.rs pins the update path's not-found message (line 534-543) and the boundary envelope, not this branch.

## Steps to Reproduce

1. Call PutReportDefinition twice with ReportDefinition.ReportName = "held-report" (second call under a held name). Observe HTTP 400 DuplicateReportNameException with NO Message member — the create path's rule (src/ops/put_report_definition.rs:166-172).
2. Call PutReportDefinition for "other-report" so the account holds both names.
3. Call ModifyReportDefinition with {"ReportName": "held-report", "ReportDefinition": <a valid definition whose ReportName is "other-report">}.
4. Observe HTTP 400 ValidationException whose message is "A report with the name other-report already exists in the account. Specify a different report name." — the DuplicateReportNameException class documentation with the requested name spliced in, i.e. both of the things the create path's comment at src/ops/put_report_definition.rs:166-169 says this service does not put on the wire for the same condition.
