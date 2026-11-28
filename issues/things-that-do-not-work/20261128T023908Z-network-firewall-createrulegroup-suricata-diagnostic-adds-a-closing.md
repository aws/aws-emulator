Issue-Source: trace-sweep-divergence
Issue-Trace: network-firewall-nfwr-traj-suricata-rule-syntax-refusals
Issue-Step: task#4
Issue-Severity: LOW
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: network-firewall-nfwr-traj-suricata-rule-syntax-refusals
Issue-Shape-Id: com.amazonaws.networkfirewall#NetworkFirewall_20201112
Issue-Key: network-firewall|CreateRuleGroup|4xx->4xx|$.Message
Issue-Commit: a3da817e95a0773e82c99c0ce1cb35022d14d67e

# network-firewall CreateRuleGroup Suricata diagnostic adds a closing quote to the "error parsing signature" line

> _Note: this issue was identified against the code base at commit `a3da817e95a0773e82c99c0ce1cb35022d14d67e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateRuleGroup

## Description
One cause.

Cause 1 of 1: the multi-line Suricata rule-validation diagnostic returned by CreateRuleGroup quotes the offending rule asymmetrically on its "error parsing signature" line, and the emulator closes that quote.

AWS: Smithy (services/service-com-amazonaws-networkfirewall-networkfirewall-20201112): CreateRuleGroup takes `Rules` (RulesString, @length 0..2000000) and models InvalidRequestException (errorKind client, httpError 400) whose documentation covers "You specified an unsupported parameter name or value". The model does not specify the exception's Message text, so for the text this is recorded behavior only. The recording shows AWS refusing a STATEFUL rule group whose `Rules` string contains a duplicated/malformed Suricata signature with HTTP 400 InvalidRequestException and a `Message` that is the Suricata loader's own report: a sequence of `stateful rule is invalid, reason: <reason>` lines joined by newlines, here three of them —
  1. `stateful rule is invalid, reason: Duplicate signature "<rule>"` — the rule text wrapped in a leading AND a trailing double quote;
  2. `stateful rule is invalid, reason: error parsing signature "<rule>` — the rule text with a LEADING double quote and NO trailing double quote;
  3. `stateful rule is invalid, reason: Loading signatures failed.`
Fixed text of the template: the per-line prefix `stateful rule is invalid, reason: `, the reason stems `Duplicate signature `, `error parsing signature ` and `Loading signatures failed.`, the newline joins, and the quoting shape of each line (balanced quotes on the Duplicate-signature line, opening quote only on the error-parsing-signature line). Variable parts: the offending signature text itself, copied verbatim out of the submitted `Rules` string (in this recording a `pass tcp <cidr> any -> any 443 (msg:"..."; sid:...; rev:...; gid:...;)` line), and which reason lines appear at all, which depends on which loader checks the submitted rules fail. No value from this one recording (the CIDR, the msg text, sid/rev/gid, the port) is part of the requirement.

Emulator: it produces the same status (400), the same error class and the same three reason lines, but it emits the second line as `error parsing signature "<rule>"` — it appends a closing double quote after the signature text that AWS leaves unterminated. Replay diff on $.Message: expected `...reason: error parsing signature "pass tcp 10.1.0.0/16 any -> any 443 (msg:"egress 443"; sid:1001; rev:1; gid:1234567890;)\nstateful rule is invalid, reason: Loading signatures failed.`, actual `...reason: error parsing signature "pass tcp 10.1.0.0/16 any -> any 443 (msg:"egress 443"; sid:1001; rev:1; gid:1234567890;)"\nstateful rule is invalid, reason: Loading signatures failed.` (the extra `"` before the newline is the entire difference; the Duplicate-signature line and the Loading-signatures-failed line match).

Why: the rule-validation Message is the only machine-readable report a client gets for which submitted signature was rejected and why, and callers (rule-authoring CI, Terraform/CDK wrappers, console-style error renderers) split the reason lines and extract the signature between the quotes. AWS's own output terminates the quote on the Duplicate-signature line but not on the error-parsing-signature line, so a parser written against AWS either keys on that asymmetry or takes the rest of the line as the rule; against the emulator it recovers a rule string with a stray trailing `"`, and any golden-text or exact-message assertion against AWS fails. It also hides the fact that the emulator is re-formatting the loader report rather than reproducing it, so other reason lines are likely re-quoted the same way.

Fix: the network-firewall service's CreateRuleGroup stateful rule-string validation — specifically the diagnostic formatter that assembles the `stateful rule is invalid, reason: ...` lines for the InvalidRequestException Message. The `error parsing signature` reason must render the offending signature with a leading double quote only, leaving it unterminated, while `Duplicate signature` keeps both quotes; the formatter should reproduce each loader reason's quoting verbatim instead of applying one quote-wrapping helper to every reason.

Traces: fixture network-firewall-nfwr-traj-suricata-rule-syntax-refusals, step task#4 (CreateRuleGroup, expected 400 / actual 400, key $.Message).

No already-open network-firewall issue covers this: the open CreateFirewall subnet-placement/duplicate-name bundle, the stateful DROP data-plane issue and the ec2__network-firewall CreateRoute/domain-list bundle are all other operations and other faults.

## Evidence
Replay of fixture network-firewall-nfwr-traj-suricata-rule-syntax-refusals, step task#4, operation CreateRuleGroup (service network-firewall), verdict mismatch, divergence key `service=network-firewall|CreateRuleGroup|status=400->400|keys=$.Message`.

Status: expected 400, actual 400 — matches. Error class InvalidRequestException — matches. Diverging member: $.Message (value_mismatch).

Expected (recorded AWS Message, newlines shown as \n):
  stateful rule is invalid, reason: Duplicate signature "pass tcp 10.1.0.0/16 any -> any 443 (msg:"egress 443"; sid:1001; rev:1; gid:1234567890;)"\nstateful rule is invalid, reason: error parsing signature "pass tcp 10.1.0.0/16 any -> any 443 (msg:"egress 443"; sid:1001; rev:1; gid:1234567890;)\nstateful rule is invalid, reason: Loading signatures failed.

Actual (emulator Message):
  stateful rule is invalid, reason: Duplicate signature "pass tcp 10.1.0.0/16 any -> any 443 (msg:"egress 443"; sid:1001; rev:1; gid:1234567890;)"\nstateful rule is invalid, reason: error parsing signature "pass tcp 10.1.0.0/16 any -> any 443 (msg:"egress 443"; sid:1001; rev:1; gid:1234567890;)"\nstateful rule is invalid, reason: Loading signatures failed.

Delta: the emulator appends a closing double quote after the signature on the `error parsing signature` line. The signature text, the CIDR, the msg/sid/rev/gid values and the line ordering are request-derived values that fill the template; the requirement being broken is the template's quoting of that reason line.

Model basis: smithy_query operation=CreateRuleGroup on com-amazonaws-networkfirewall-networkfirewall-20201112 — errors include InvalidRequestException (client, httpError 400); input member `Rules` target RulesString @length 0..2000000. Message text is recorded behavior only (not modeled).

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire fixture corpus (trace set simparity-wire-aws-v1).
2. Replay fixture `network-firewall-nfwr-traj-suricata-rule-syntax-refusals`, step `task#4`: a CreateRuleGroup call for a STATEFUL rule group whose `Rules` string contains the same Suricata signature twice, with the duplicate also failing the loader's parse check (the recorded step submits a `pass tcp <cidr> any -> any 443 (msg:"..."; sid:1001; rev:1; gid:1234567890;)` line twice).
3. Observe the 400 InvalidRequestException and read `$.Message`.
4. Split `$.Message` on newlines. The second line is `stateful rule is invalid, reason: error parsing signature "<rule>"` — note the trailing double quote after the signature.
5. Compare with the recorded AWS response for the same step: its second line ends immediately after the signature text, with no closing double quote, while the first line (`Duplicate signature "<rule>"`) is quoted on both sides.
6. The step is reported as a mismatch on key `$.Message` with equal expected/actual status 400.
