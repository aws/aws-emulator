Issue-Source: trace-sweep-divergence
Issue-Trace: cloudwatchomni-view-definition-statement-and-name-rules
Issue-Step: task#3
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cloudwatchomni-view-definition-statement-and-name-rules
Issue-Shape-Id: com.amazonaws.cloudwatchomni#CloudWatchOmniFrontend
Issue-Key: cloudwatchomni|CreateView|4xx->4xx|$.message
Issue-Commit: 7300602cd0fc37394a6328fdd869591f4a36c031

# cloudwatchomni ViewName pattern omits the period, so dotted view names are refused and the advertised regex is wrong

> _Note: this issue was identified against the code base at commit `7300602cd0fc37394a6328fdd869591f4a36c031`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateView

## Description
One cause.

Cause 1 of 1: the `ViewName` constraint the CreateView boundary enforces — and quotes back in its ValidationException message — excludes the period from the trailing character class, so it is narrower than the constraint AWS enforces.

AWS: CreateView refuses a malformed `name` with HTTP 400 / ValidationException (Smithy: CreateView errors include ValidationException, httpError 400, "A parameter is specified incorrectly"; member CreateViewRequest$name targets ViewName, required, @length min 6 max 256, @pattern). The recorded refusal advertises the enforced expression as `view\.[a-z0-9][a-z0-9._-]{0,250}` — the trailing character class admits the period, so a dotted name such as `view.<segment>.<segment>` IS a legal view name. Note the on-disk model disagrees with the wire here: it carries @pattern `^view\.[a-z0-9][a-z0-9_-]{0,250}$` and ViewName's prose says only "lowercase alphanumeric, hyphens, and underscores". The recording is the authority for the wire, and the operation's own module doc already states this (services/service-com-amazonaws-cloudwatchomni-cloudwatchomnifrontend/src/ops/create_view.rs lines 18-23: "Its trailing character class admits the period, so a dotted name (view.<a>.<b>) is a view name ... the wire is the authority where it and the member's prose disagree"), so the model's pattern — not the recorded expectation — is what is stale.

The message for this error class is AWS's standard constraint-violation template: the fixed text is "<n> validation error detected: Value at '<member>' failed to satisfy constraint: Member must satisfy regular expression pattern: <pattern>". Its variable parts are the violation count, the member path taken from the request (`name`), and the pattern taken from the member's own constraint. Nothing in it is specific to one recording — no id, name or timestamp from the run appears in it.

Emulator: the status and error class are right (400, ValidationException) but the message renders the pattern as `view\.[a-z0-9][a-z0-9_-]{0,250}`, i.e. the period is missing from the character class. Replay detail: `$.message: value_mismatch (expected "1 validation error detected: Value at 'name' failed to satisfy constraint: Member must satisfy regular expression pattern: view\\.[a-z0-9][a-z0-9._-]{0,250}", got "1 validation error detected: Value at 'name' failed to satisfy constraint: Member must satisfy regular expression pattern: view\\.[a-z0-9][a-z0-9_-]{0,250}")"`. Because the boundary enforces the same expression it advertises, the emulator does not merely misreport the rule: it also REJECTS dotted names that AWS accepts, and the same member is carried by GetView, UpdateView and DeleteView, so those reject them too.

Why: a client that names its views with dotted segments (a common namespacing convention, e.g. `view.billing.monthly`) creates them successfully against AWS and gets a 400 ValidationException from the emulator, so the whole view lifecycle is unreachable for those names. A client that surfaces or asserts on the advertised regex also learns a rule narrower than the service's, and tooling that pre-validates names client-side from that expression rejects names the service would take.

Fix: the ViewName constraint used by CreateView's input validation — the member's @pattern in the service's Smithy model, which both the generated boundary check and the constraint-violation message render from. Widening the trailing character class to admit the period fixes the message and the accepted-name set together, and keeping the one pattern on the member keeps GetView/UpdateView/DeleteView in step, as that module doc intends.

Traces: cloudwatchomni-view-definition-statement-and-name-rules#task#3 (CreateView, expected 400, actual 400, `$.message` mismatch).

## Evidence
Fixture cloudwatchomni-view-definition-statement-and-name-rules, step task#3, operation CreateView.
Expected status 400, actual status 400 — the diverging field is $.message.

Expected (AWS, recorded):
  1 validation error detected: Value at 'name' failed to satisfy constraint: Member must satisfy regular expression pattern: view\.[a-z0-9][a-z0-9._-]{0,250}

Actual (emulator):
  1 validation error detected: Value at 'name' failed to satisfy constraint: Member must satisfy regular expression pattern: view\.[a-z0-9][a-z0-9_-]{0,250}

Message anatomy — fixed text: "<n> validation error detected: Value at '<member>' failed to satisfy constraint: Member must satisfy regular expression pattern: <pattern>". Variable parts: the violation count, the member path from the request (`name`), and the member's own pattern. The only difference is the pattern: the period is absent from the emulator's trailing character class.

Model evidence (smithy_query, services/service-com-amazonaws-cloudwatchomni-cloudwatchomnifrontend/smithy-model/com-amazonaws-cloudwatchomni-cloudwatchomnifrontend.json):
  CreateView.input.name -> ViewName, required, length 6..256, pattern ^view\.[a-z0-9][a-z0-9_-]{0,250}$
  CreateView.errors -> ValidationException (client, httpError 400)
  doc ViewName -> "User view name: must start with \"view.\" prefix followed by lowercase alphanumeric, hyphens, and underscores."
The emulator's rendered pattern matches the on-disk model exactly, which locates the defect in that constraint rather than in message formatting.

Corroborating source note: services/service-com-amazonaws-cloudwatchomni-cloudwatchomnifrontend/src/ops/create_view.rs lines 18-23 state that ViewName's trailing character class admits the period and that the wire is authoritative over the member's prose — the enforced constraint does not match that statement.

## Steps to Reproduce
1. git_clone() and boot the emulator built from the checkout.
2. replay_wire_corpus().
3. Inspect fixture cloudwatchomni-view-definition-statement-and-name-rules, step task#3 — the CreateView call with a `name` that violates ViewName's pattern.
4. Compare $.message in the 400 ValidationException body: the advertised regex is `view\.[a-z0-9][a-z0-9_-]{0,250}` instead of `view\.[a-z0-9][a-z0-9._-]{0,250}`.
5. Independently: call CreateView with a dotted-but-otherwise-legal name (e.g. name="view.billing.monthly", definition="SELECT 1"). AWS accepts it; the emulator answers 400 ValidationException with the same constraint message, and GetView/UpdateView/DeleteView reject that name too.
