Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ses.send-email-through-paused-configuration-set-rejected ses.send-templated-email-through-paused-configuration-set-rejected ses.send-raw-email-paused-set-named-in-mime-header-rejected
Issue-Shape-Id: com.amazonaws.ses#SimpleEmailService
Issue-Key: ses.send-email-through-paused-configuration-set-rejected|run#14|AssertionError|assert got == paused, f"expected {paused!r} for the disabled set, got {got!r}"
Issue-Key: ses.send-templated-email-through-paused-configuration-set-rejected|run#14|AssertionError|assert got == paused, f"expected {paused!r} for the disabled set, got {got!r}"
Issue-Key: ses.send-raw-email-paused-set-named-in-mime-header-rejected|run#12|AssertionError|assert ghost == ("ConfigurationSetDoesNotExist", f"Configuration set <{unknown_set}> does not exist."), (
Issue-Commit: fe76b837b9c6a96309bb6d3401999300c9d3921a

# ses: 2 net-new causes — the ConfigurationSetSendingPausedException message is emulator-authored, and SendRawEmail never resolves the configuration set named by the X-SES-CONFIGURATION-SET header

> _Note: this issue was identified against the code base at commit `fe76b837b9c6a96309bb6d3401999300c9d3921a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SendEmail

## Description
Two net-new root causes in the SES v1 send family's configuration-set gate. Both are grounded in failing behavioral checks and are independent of each other; a fixer must close BOTH before submitting.

Cause 1 of 2: the ConfigurationSetSendingPausedException message text is the emulator's own sentence, not AWS's.
  AWS:      Smithy (services/service-com-amazonaws-ses-simpleemailservice): SendEmail / SendTemplatedEmail / SendRawEmail all model error ConfigurationSetSendingPausedException, errorKind client, httpError 400, awsQueryError code "ConfigurationSetSendingPausedException" (shape doc: "Indicates that email sending is disabled for the configuration set. You can enable or disable email sending for a configuration set using UpdateConfigurationSetSendingEnabled."). The model does not state the message text, so for the text this is recorded behavior only: when the named configuration set exists but its sending is disabled, the recorded refusal is HTTP 400, Code "ConfigurationSetSendingPausedException", Message "Sending is paused for configuration set <set name>". The FIXED text of that message is "Sending is paused for configuration set "; the only variable part is the configuration-set name taken verbatim from the request's ConfigurationSetName (no angle brackets around it, and no trailing period).
  Emulator: answers the same code but the message "Email sending is disabled for configuration set <<set name>>." — i.e. a different sentence, the name wrapped in angle brackets, and a trailing full stop. Replay: expected ('ConfigurationSetSendingPausedException', 'Sending is paused for configuration set <set>') ; got ('ConfigurationSetSendingPausedException', 'Email sending is disabled for configuration set <<set>>.') for SendEmail and identically for SendTemplatedEmail. SendRawEmail carries the same wording in its own copy of the block, so the same divergence is latent there.
  Why:      a client that surfaces, logs or matches on the refusal text (operators grep SES logs for "Sending is paused for configuration set", and test suites assert the sentence) sees a message real SES never emits; the angle brackets also make naive parsers extract "<name>" instead of the set name, so automated remediation that re-enables the named set is fed a wrong identifier.
  Fix:      the configuration-set pause branch of each v1 send handler's refusal construction — the message it fills ConfigurationSetSendingPausedException with must be AWS's fixed sentence with the request's set name interpolated bare. One sentence shared by all three sends rather than three copies.
  Traces:   ses.send-email-through-paused-configuration-set-rejected, ses.send-templated-email-through-paused-configuration-set-rejected (file/line: services/service-com-amazonaws-ses-simpleemailservice/src/ops/send_email.rs:327-340, src/ops/send_templated_email.rs:342-355, src/ops/send_raw_email.rs:578-591).

Cause 2 of 2: SendRawEmail ignores a configuration set named only by the X-SES-CONFIGURATION-SET MIME header, so the set is never looked up and never gates the send.
  AWS:      recorded behavior only (the Smithy model binds the set only through SendRawEmailRequest$ConfigurationSetName; the header route is documented in the SES Developer Guide, "Specifying a configuration set when you send email" — a raw message may select the set with the X-SES-CONFIGURATION-SET header). Recorded: a SendRawEmail whose RawMessage carries X-SES-CONFIGURATION-SET and whose request carries NO ConfigurationSetName is resolved against that header name, and the set-level gates run before the sender check: a header-named set that was never created (or has been deleted) is refused HTTP 400 Code "ConfigurationSetDoesNotExist" Message "Configuration set <<set name>> does not exist." (fixed text "Configuration set <" ... "> does not exist.", variable part the header's value), and a header-named set whose sending is disabled is refused HTTP 400 "ConfigurationSetSendingPausedException" with Cause 1's sentence. Only once the header set resolves and is enabled does the send reach the sender gate.
  Emulator: the handler reads only input.configuration_set_name; the parsed MIME headers are read for From/To/Cc/Bcc/Return-Path/Subject and never for X-SES-CONFIGURATION-SET. A message naming a set that does not exist therefore skips the set lookup entirely and falls through to the identity gate, answering ('MessageRejected', 'Email address is not verified. The following identities failed the check in region <REGION>: <sender>') where AWS answers ('ConfigurationSetDoesNotExist', 'Configuration set <<set>> does not exist.'). For the same reason a header-named but PAUSED set is not honoured at all.
  Why:      a client that selects its configuration set per message through the MIME header (the only way to do it when composing raw MIME, e.g. when relaying an already-assembled message) gets no feedback that the set is unknown, and — worse — a paused set stops nothing: a message a verified sender submits through a paused header-named set is accepted and sent where AWS refuses it, so an operator's "pause this configuration set" kill switch is silently ineffective on the raw-send path. The wrong error code also misroutes client retry logic (an unverified-identity refusal is permanent and actionable by a different operator than a missing configuration set).
  Fix:      SendRawEmail's handler: after the message is parsed, resolve the configuration set from the request's ConfigurationSetName when present and otherwise from the message's X-SES-CONFIGURATION-SET header, and run the existing exists/paused gates on the resolved name BEFORE the sender-verification gate; the resolved name is also what the send log should record as the message's configuration set.
  Traces:   ses.send-raw-email-paused-set-named-in-mime-header-rejected (file/line: services/service-com-amazonaws-ses-simpleemailservice/src/ops/send_raw_email.rs:424-592 — headers parsed at 424, set gate at 563-592 keyed only on the input member).

## Evidence
Cause 1 — ses.send-email-through-paused-configuration-set-rejected:
  expected ('ConfigurationSetSendingPausedException', 'Sending is paused for configuration set <set>')
  got      ('ConfigurationSetSendingPausedException', 'Email sending is disabled for configuration set <<set>>.')
ses.send-templated-email-through-paused-configuration-set-rejected:
  expected ('ConfigurationSetSendingPausedException', 'Sending is paused for configuration set <set>')
  got      ('ConfigurationSetSendingPausedException', 'Email sending is disabled for configuration set <<set>>.')
(<set> is the configuration-set name from the request; it is a per-run value, not part of the requirement.)
Source of the emulator text: src/ops/send_email.rs:332-335, src/ops/send_templated_email.rs:347-350, src/ops/send_raw_email.rs:583-586 all format "Email sending is disabled for configuration set <{configuration_set_name}>.".

Cause 2 — ses.send-raw-email-paused-set-named-in-mime-header-rejected:
  the raw message carries only `X-SES-CONFIGURATION-SET: <never-created-set>` (no ConfigurationSetName in the request)
  expected ('ConfigurationSetDoesNotExist', 'Configuration set <<never-created-set>> does not exist.')
  got      ('MessageRejected', 'Email address is not verified. The following identities failed the check in region US-WEST-2: <sender>')
The same check then expects a DISABLED header-named set to answer ConfigurationSetSendingPausedException with Cause 1's sentence, and a deleted header-named set to answer ConfigurationSetDoesNotExist again. src/ops/send_raw_email.rs:563 shows the gate entered only `if let Some(configuration_set_name) = input.configuration_set_name.as_deref()`, so the header is never consulted.
Model for both causes (smithy_query operation=SendEmail,SendRawEmail,SendTemplatedEmail): each models ConfigurationSetDoesNotExistException (awsQueryError code "ConfigurationSetDoesNotExist", 400) and ConfigurationSetSendingPausedException (awsQueryError code "ConfigurationSetSendingPausedException", 400).

## Steps to Reproduce
Cause 1:
1. ses.create_configuration_set(ConfigurationSet={'Name': S}).
2. ses.update_configuration_set_sending_enabled(ConfigurationSetName=S, Enabled=False).
3. ses.send_email(Source=..., Destination=..., Message=..., ConfigurationSetName=S) — and likewise ses.send_templated_email(..., ConfigurationSetName=S, Template=..., TemplateData='{}').
4. Observe the ClientError: Code is ConfigurationSetSendingPausedException (correct) but Message is 'Email sending is disabled for configuration set <S>.' instead of AWS's 'Sending is paused for configuration set S'.

Cause 2:
1. ses.create_configuration_set(ConfigurationSet={'Name': S}).
2. Build a raw RFC 5322 message whose ONLY configuration-set reference is the header line 'X-SES-CONFIGURATION-SET: G', where G is a name that was never created, and call ses.send_raw_email(RawMessage={'Data': msg}) with no Source and no ConfigurationSetName.
3. Observe ('MessageRejected', 'Email address is not verified...') instead of ('ConfigurationSetDoesNotExist', 'Configuration set <G> does not exist.').
4. Repeat with the header naming S after update_configuration_set_sending_enabled(ConfigurationSetName=S, Enabled=False): the send is not refused as paused either, so the header-named pause does not gate the send at all.
