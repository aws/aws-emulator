Issue-Severity: LOW
Issue-Gap-Class: wrong-behavior
Issue-Commit: 72f48f42de416ff1071fa90a732a0bc13a8c145e

# The stated rule for which operations keep their own success status omits this service's three bodyless 200s, and calls their modelled 200 "the protocol default"

_Filed: 2026-11-28T15:53:11Z_
_Operation: TagResource | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `72f48f42de416ff1071fa90a732a0bc13a8c145e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`TagResource`/`UntagResource` answer 204 and every other operation keeps its own status; the RESPONSES are correct and must not change. What is defective is the rule stated beside them, in two places in this crate, which a future fixer would act on:

1. `src/ops/tag_resource.rs:147-153` justifies the 204 with "`TagResourceResponse` carries no members, so the answer has nothing for a caller to parse" and then enumerates the operations that keep their own status as "the operations whose output DOES carry members, and the create whose `@http` code the model states". Three handlers in this same crate belong to NEITHER group and keep 200: `DeleteEmailContact` (src/ops/delete_email_contact.rs:155), `ActivateEmailContact` (src/ops/activate_email_contact.rs:150) and `SendActivationCode` (src/ops/send_activation_code.rs:141) — all three have member-less output structures, exactly the property the comment gives as the REASON for 204. Applied as written, the stated rule turns three operations' statuses into 204.

2. `tests/all/tag_mutation_success_status.rs:10-12` accounts for "Every other operation" as only "the create" plus "the readers, whose output carries members", and its own test body at lines 253-256 calls those three bodyless successes the ones "this service answers with the protocol default (200)". The model contradicts that: it declares an explicit `@http` code 200 for all three, and leaves the code DEFAULTED only for the tag mutations — which is the real discriminator and the one the handler comment's first half gets right ("the model states no explicit `@http` code here").

The fix is prose only: name the actual discriminator (whether the model states an `@http` code) in both places and account for the three bodyless 200s. Every response and every existing assertion stays exactly as it is.

## Evidence

Stated rule, handler side — services/service-com-amazonaws-notificationscontacts-notificationscontacts/src/ops/tag_resource.rs:147-154:
```
    // A successful tag write answers 204 No Content: `TagResourceResponse` carries no
    // members, so the answer has nothing for a caller to parse and the service reports
    // that with the status that says so, rather than the 200 the protocol leaves
    // defaulted (the model states no explicit `@http` code here, so the success status
    // is the service's own behaviour and not the model's). `UntagResource`, the other
    // tag mutation, answers the same way; the operations whose output DOES carry
    // members, and the create whose `@http` code the model states, keep their own.
    const SUCCESS_STATUS: u16 = 204u16;
```

Contradicted by three sibling handlers of this crate, each with a member-less output structure and a 200:
- src/ops/delete_email_contact.rs:155 `const SUCCESS_STATUS: u16 = 200u16;`
- src/ops/activate_email_contact.rs:150 `const SUCCESS_STATUS: u16 = 200u16;`
- src/ops/send_activation_code.rs:141 `const SUCCESS_STATUS: u16 = 200u16;`
(their outputs carry no members: `smithy_query(query="operation",operation="DeleteEmailContact,ActivateEmailContact,SendActivationCode")` reports `"output": {}` for all three.)

Stated rule, test side — tests/all/tag_mutation_success_status.rs:7-12:
```
//! RULE: a successful tag mutation answers 204 No Content with zero body bytes, stating
//! the length it has — the response structure carries no members, so the
//! service reports "nothing to parse" with the status that says so rather than with the
//! 200 the protocol leaves defaulted. Every other operation keeps its own success
//! status: the create answers the code the model states for it, and the readers, whose
//! output carries members, answer 200.
```
and the same file's inline comment, tests/all/tag_mutation_success_status.rs:253-257:
```
    // Rows differ in the kind of answer: a create whose `@http` code the model states
    // (201), the readers whose output carries members (200), and the bodyless successes
    // this service answers with the protocol default (200) — `SendActivationCode`,
    // `ActivateEmailContact` and `DeleteEmailContact`, whose wire form is pinned by
    // `email_contact_timestamp_wire_form.rs` and `email_contacts.rs`.
```

The model says those three 200s are NOT the protocol default — `smithy_query(query="operation",operation="TagResource,DeleteEmailContact,SendActivationCode,ActivateEmailContact")`:
```
"TagResource": ... "http": {"method": "POST", "uri": "/tags/{arn}", "code": 200, "codeDefaulted": true}
"DeleteEmailContact": ... "http": {"code": 200, "method": "DELETE", "uri": "/emailcontacts/{arn}"}
"SendActivationCode": ... "http": {"code": 200, "method": "POST", "uri": "/2022-10-31/emailcontacts/{arn}/activate/send"}
"ActivateEmailContact": ... "http": {"code": 200, "method": "PUT", "uri": "/emailcontacts/{arn}/activate/{code}"}
```
Only `TagResource` (and `UntagResource`) carry `codeDefaulted`. OPERATIONS.md shows the same split: "## TagResource — **Route:** `POST /tags/{arn}`" (OPERATIONS.md:36) with no code, against "**Route:** `DELETE /emailcontacts/{arn} -> 200`" (OPERATIONS.md:67), "`PUT /emailcontacts/{arn}/activate/{code}` -> 200" (OPERATIONS.md:83) and "`POST /2022-10-31/emailcontacts/{arn}/activate/send` -> 200" (OPERATIONS.md:91).

Test coverage that pins the responses (so only the prose may move): tests/all/tag_mutation_success_status.rs:244-305 `every_other_operation_keeps_its_own_success_status` asserts 204 for the tag mutations, 201 for the create, 200 for the three readers and 200 for `SendActivationCode`, `ActivateEmailContact` and `DeleteEmailContact` (lines 287-305).

## Steps to Reproduce

1. Read src/ops/tag_resource.rs:147-154: the 204 is justified by "`TagResourceResponse` carries no members", and the operations that "keep their own" status are enumerated as those "whose output DOES carry members" plus "the create".
2. Read src/ops/delete_email_contact.rs:155, src/ops/activate_email_contact.rs:150 and src/ops/send_activation_code.rs:141: all three set `SUCCESS_STATUS = 200u16` while their output structures carry no members (`smithy_query(query="operation",operation="DeleteEmailContact,ActivateEmailContact,SendActivationCode")` reports `"output": {}`), so they satisfy the comment's stated reason for 204 yet are in neither group it names.
3. Read tests/all/tag_mutation_success_status.rs:10-12 and 253-257: "Every other operation" is accounted for as the create plus the member-carrying readers, and the three bodyless successes are described as answering "the protocol default (200)".
4. Run `smithy_query(query="operation",operation="TagResource,DeleteEmailContact,SendActivationCode,ActivateEmailContact")`: `TagResource`'s http binding carries `"codeDefaulted": true`, while the other three declare `"code": 200` explicitly — so their 200 is the model's, not the protocol default, and the model's explicit code (not the member-less output) is the property that separates them from the tag mutations.
5. Conclude the two rule statements are the stale side: tests/all/tag_mutation_success_status.rs:244-305 pins every status as it is, so the responses must not change and only the prose needs correcting.
