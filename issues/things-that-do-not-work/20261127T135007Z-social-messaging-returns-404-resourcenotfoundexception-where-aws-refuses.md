Issue-Source: trace-sweep-divergence
Issue-Trace: socialmessaging-conversion-event-business-account-id-refusals
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: socialmessaging-conversion-event-business-account-id-refusals socialmessaging-send-message-parameter-refusals
Issue-Shape-Id: com.amazonaws.socialmessaging#SocialMessaging
Issue-Key: social-messaging|SendWhatsAppConversionEvent|4xx->4xx|$.message
Issue-Key: social-messaging|SendWhatsAppConversionEvent|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: social-messaging|SendWhatsAppConversionEvent|4xx->4xx|status
Issue-Key: social-messaging|SendWhatsAppMessage|4xx->4xx|$.message
Issue-Key: social-messaging|SendWhatsAppMessage|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: social-messaging|SendWhatsAppMessage|4xx->4xx|status
Issue-Commit: 388bf0aeef5b87cdb8b8e5fe5948d049b57af517

# social-messaging returns 404 ResourceNotFoundException where AWS refuses with 400 InvalidParametersException

> _Note: this issue was identified against the code base at commit `388bf0aeef5b87cdb8b8e5fe5948d049b57af517`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SendWhatsAppConversionEvent

## Description
Two WhatsApp send operations resolve a request identifier against the store as the FIRST thing they do and answer 404 ResourceNotFoundException when the lookup misses. AWS refuses these requests earlier, as a parameter-level refusal: HTTP 400 InvalidParametersException ("One or more parameters provided to the action are not valid.", httpError 400). The fault the two share is the error classification/ordering: a request whose identifier parameters are not acceptable must be refused 400 InvalidParametersException, not reported as a missing resource. 2 causes.

Cause 1 of 2: SendWhatsAppConversionEvent - unrecognized WABA id reported as not-found.
AWS: Smithy SendWhatsAppConversionEvent.id -> LinkedWhatsAppBusinessAccountId, @required, @length 1..115, @pattern (^waba-.*$)|(^arn:.*:waba/[0-9a-zA-Z]+$); datasetId @required, @length 10..20, @pattern ^[0-9]+$; modeled errors include InvalidParametersException (400) and ResourceNotFoundException (404). The recorded refusal is HTTP 400 with message "Invalid Waba Id" (the recording does not assert x-amzn-errortype, so the class is pinned by the 400 status plus the modeled 400 client error). "Invalid Waba Id" is entirely fixed text - it carries no value from the request or from state. So: an id that is syntactically accepted by the pattern but does not name a WhatsApp Business Account this caller can use is a 400 InvalidParametersException, not a 404.
Emulator: src/ops/send_whats_app_conversion_event.rs resolves store.linked_account_id(&input.id) filtered on the account's dataset_id and, on a miss, returns ResourceNotFoundException with crate::state::ID_NOT_FOUND_FOR_ACCOUNT_MESSAGE - observed HTTP 404, x-amzn-errortype: ResourceNotFoundException, message "Id not found for account.".
Why: the status class is wrong (404 vs 400) and the error code is wrong, so an SDK client maps the failure to ResourceNotFoundException rather than InvalidParametersException; retry/branch logic keyed on the modeled error and any 4xx-class handling both take the wrong path, and the message a client surfaces is not the one AWS words.
Fix: the SendWhatsAppConversionEvent handler's refusal path (and the validation that should precede the store lookup) - refuse an unusable id with InvalidParametersException/400 and the message "Invalid Waba Id", reserving ResourceNotFoundException for the cases AWS actually reports as missing.
Traces: socialmessaging-conversion-event-business-account-id-refusals#task#1.

Cause 2 of 2: SendWhatsAppMessage - parameter refusal pre-empted by the origination-number lookup.
AWS: Smithy SendWhatsAppMessage.originationPhoneNumberId -> WhatsAppPhoneNumberId, @required, @pattern (^phone-number-id-.*$)|(^arn:.*:phone-number-id/[0-9a-zA-Z]+$); message @required blob; metaApiVersion @required; modeled errors include InvalidParametersException (400) and ResourceNotFoundException (404). The recorded refusal is HTTP 400 with message "Provided parameters are not valid" - entirely fixed text, with no request or state value interpolated. AWS evaluates the acceptability of the send parameters and refuses 400 InvalidParametersException; it does not report the request as a missing resource.
Emulator: src/ops/send_whats_app_message.rs:166-178 looks up store.phone_number_id(&input.origination_phone_number_id) before anything else and, on a miss, returns ResourceNotFoundException with crate::state::ORIGINATION_PHONE_NUMBER_NOT_FOUND_MESSAGE - observed HTTP 404, x-amzn-errortype: ResourceNotFoundException, message "Origination phone number id not found for account.". Both steps of the fixture (task#1, task#2) take this path.
Why: every malformed-parameter send is reported to clients as a nonexistent phone number, so a caller debugging a bad message payload or metaApiVersion is pointed at the wrong input, and code that distinguishes InvalidParametersException (fix the request) from ResourceNotFoundException (provision the resource) branches wrongly.
Fix: SendWhatsAppMessage's validation stage/handler - perform the parameter-acceptability refusal (400 InvalidParametersException, message "Provided parameters are not valid") before resolving originationPhoneNumberId against the store.
Traces: socialmessaging-send-message-parameter-refusals#task#1 and #task#2.

No existing open issue covers either cause (list_existing_issues finds no social-messaging issue).

## Evidence
Cause 1 (fixture socialmessaging-conversion-event-business-account-id-refusals, step task#1, SendWhatsAppConversionEvent): status expected 400, got 404. $.message expected "Invalid Waba Id", got "Id not found for account.". $headers.x-amzn-errortype expected a 400-class client error (recording asserts shape only), got "ResourceNotFoundException".

Cause 2 (fixture socialmessaging-send-message-parameter-refusals, steps task#1 and task#2, SendWhatsAppMessage): status expected 400, got 404. $.message expected "Provided parameters are not valid", got "Origination phone number id not found for account.". $headers.x-amzn-errortype expected a 400-class client error, got "ResourceNotFoundException".

Emulator source: services/service-com-amazonaws-socialmessaging-socialmessaging/src/ops/send_whats_app_message.rs lines 166-178 (ResourceNotFoundException / ORIGINATION_PHONE_NUMBER_NOT_FOUND_MESSAGE); .../src/ops/send_whats_app_conversion_event.rs lines 168-188 (ResourceNotFoundException / ID_NOT_FOUND_FOR_ACCOUNT_MESSAGE).

Model: InvalidParametersException {errorKind client, httpError 400, doc "One or more parameters provided to the action are not valid."}; ResourceNotFoundException {errorKind client, httpError 404}. Both are modeled on both operations.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus (replay_wire_corpus).
2. Cause 1: POST /v1/whatsapp/waba/dataset/events (SendWhatsAppConversionEvent) with a pattern-valid id such as waba-<something> that names no linked account (or a datasetId that is not that account's dataset). Observe 404 / ResourceNotFoundException / "Id not found for account." instead of 400 / InvalidParametersException / "Invalid Waba Id".
3. Cause 2: POST /v1/whatsapp/send (SendWhatsAppMessage) with parameters AWS refuses outright (pattern-valid originationPhoneNumberId that is not registered, together with the fixture's message/metaApiVersion). Observe 404 / ResourceNotFoundException / "Origination phone number id not found for account." instead of 400 / InvalidParametersException / "Provided parameters are not valid".
4. Replay fixtures socialmessaging-conversion-event-business-account-id-refusals (task#1) and socialmessaging-send-message-parameter-refusals (task#1, task#2) to confirm both causes are closed.
