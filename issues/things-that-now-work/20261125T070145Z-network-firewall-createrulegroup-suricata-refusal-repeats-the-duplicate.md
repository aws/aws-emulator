Issue-Source: trace-sweep-divergence
Issue-Trace: network-firewall-nfwr-traj-suricata-rule-syntax-refusals
Issue-Step: task#4
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: network-firewall-nfwr-traj-suricata-rule-syntax-refusals
Issue-Shape-Id: com.amazonaws.networkfirewall#NetworkFirewall_20201112
Issue-Key: network-firewall|CreateRuleGroup|4xx->4xx|$.Message
Issue-Commit: 8d45c17cb7efd125284ce0c8433eba65cdb760bf

# network-firewall CreateRuleGroup Suricata refusal repeats the duplicate-signature diagnostic instead of emitting the distinct parse-error and "Loading signatures failed." lines

> _Note: this issue was identified against the code base at commit `8d45c17cb7efd125284ce0c8433eba65cdb760bf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateRuleGroup

## Description
One cause (Cause 1 of 1).

Cause 1 of 1: CreateRuleGroup's Suricata rule-validation diagnostics are wrong for a STATEFUL rule group whose Rules string fails to load — the emulator returns the right status and error shape but the wrong message body.

AWS: Smithy: CreateRuleGroup (input RuleGroup / Rules: RulesString @length 0..2000000, Type: RuleGroupType required), modeled error InvalidRequestException (client, httpError 400) — documented as "The operation failed because of a problem with your request... You specified an unsupported parameter name or value." The model does not specify the diagnostic text, so for the message this is recorded behavior only. The recording shows AWS refusing with HTTP 400 / InvalidRequestException and a Message that is a newline-joined list of DISTINCT per-stage Suricata loader diagnostics, each line prefixed with the fixed text `stateful rule is invalid, reason: ` and then one distinct reason:
  - `Duplicate signature "<rule>"` — reported once for the signature that collides,
  - `error parsing signature "<rule>"` — reported for the signature the loader then could not parse,
  - `Loading signatures failed.` — a single trailing summary line that terminates the list.
Fixed text in that message: the `stateful rule is invalid, reason: ` prefix, the reason words `Duplicate signature`, `error parsing signature`, `Loading signatures failed.`, the surrounding double quotes, and the newline separators. Variable parts: the quoted `<rule>` text, which is copied verbatim from the offending rule line in the request's Rules string (in this recording a `pass tcp ... (msg:...; sid:...; rev:1; gid:...;)` line, but that value is request data, not part of the template).

Emulator: returns HTTP 400 (status matches) but Message = `stateful rule is invalid, reason: Duplicate signature "<rule>"\nstateful rule is invalid, reason: Duplicate signature "<rule>"` — i.e. it emits the SAME duplicate-signature diagnostic twice (apparently one per offending rule occurrence), never emits the `error parsing signature "<rule>"` line, and never emits the trailing `Loading signatures failed.` line. Replay verdict: mismatch on `$.Message` (value_mismatch), expected 400 / actual 400.

Why: clients and IaC tooling parse this message to tell the operator which rule is wrong and why. Because the emulator repeats one reason class, a caller whose rules fail for a parse reason is told the rules are duplicated instead, and because the terminating `Loading signatures failed.` line is absent, any consumer that keys on that sentinel to detect "the whole rule bundle was rejected by the Suricata loader" (as opposed to a single advisory diagnostic) concludes the load succeeded. A rule-authoring loop that fixes only the reported duplication keeps resubmitting and keeps being refused with no indication of the real parse fault.

Fix: the CreateRuleGroup handler's Suricata rules validation for Type=STATEFUL — the stage that walks the parsed Rules string and accumulates loader diagnostics. It must accumulate one diagnostic per DISTINCT loader failure (duplicate signature, signature parse error) rather than re-emitting the first reason for each offending line, and must append the single trailing `stateful rule is invalid, reason: Loading signatures failed.` summary line whenever any diagnostic was accumulated, before serializing the joined text into the InvalidRequestException Message.

Traces: fixture network-firewall-nfwr-traj-suricata-rule-syntax-refusals, step task#4 (CreateRuleGroup).

Note: the open issues `network-firewall: the stateful DROP rule is never enforced on the firewall endpoint's data plane` and `Network Firewall CreateFirewall skips subnet-placement validation...` are about data-plane enforcement and CreateFirewall validation respectively; neither covers CreateRuleGroup's rule-load diagnostics, and no open issue carries this replay key.

## Evidence
Replay divergence (verdict=mismatch, divergence_key `service=network-firewall|CreateRuleGroup|status=400->400|keys=$.Message`), fixture `network-firewall-nfwr-traj-suricata-rule-syntax-refusals`, step `task#4`.

Expected status: 400. Actual status: 400.

Expected $.Message (3 newline-separated lines):
  stateful rule is invalid, reason: Duplicate signature "<offending rule line from the request's Rules>"
  stateful rule is invalid, reason: error parsing signature "<offending rule line from the request's Rules>"
  stateful rule is invalid, reason: Loading signatures failed.

Actual $.Message (2 newline-separated lines):
  stateful rule is invalid, reason: Duplicate signature "<offending rule line>"
  stateful rule is invalid, reason: Duplicate signature "<offending rule line>"

Delta: the `error parsing signature` diagnostic is replaced by a second copy of the `Duplicate signature` diagnostic, and the terminating `Loading signatures failed.` line is missing entirely. Status (400) and error shape (InvalidRequestException, modeled httpError 400) are correct.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus (`replay_wire_corpus`).
2. Fixture `network-firewall-nfwr-traj-suricata-rule-syntax-refusals`, step `task#4`, issues CreateRuleGroup with Type=STATEFUL and a Rules string containing a Suricata signature that both duplicates an earlier signature and fails to parse.
3. Observe the 400 InvalidRequestException body: $.Message contains the `Duplicate signature "..."` diagnostic twice and lacks both the `error parsing signature "..."` diagnostic and the trailing `stateful rule is invalid, reason: Loading signatures failed.` line.
4. Expected: one diagnostic line per distinct loader failure, followed by the single `Loading signatures failed.` summary line, all newline-joined and each prefixed `stateful rule is invalid, reason: `.
