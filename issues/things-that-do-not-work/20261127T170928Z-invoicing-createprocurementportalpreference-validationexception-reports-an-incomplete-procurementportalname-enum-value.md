Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: invoicing.create-procurement-portal-preference-duplicate-conflict-rejected
Issue-Shape-Id: com.amazonaws.invoicing#Invoicing
Issue-Key: invoicing.create-procurement-portal-preference-duplicate-conflict-rejected|run#5|AssertionError|assert bad_portal_err["Message"] == expected_portal_message, (
Issue-Commit: cc8c6d4514b1c20009096c9d1c08086c7dabca5f

# CreateProcurementPortalPreference ValidationException reports an incomplete ProcurementPortalName enum value set (ZIP and JAGGAER missing)

> _Note: this issue was identified against the code base at commit `cc8c6d4514b1c20009096c9d1c08086c7dabca5f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateProcurementPortalPreference

## Description
Cause 1 of 1: the set of accepted ProcurementPortalName values used by CreateProcurementPortalPreference input validation — and therefore the enum value set quoted in the ValidationException message — is missing two portals real AWS supports.

AWS: CreateProcurementPortalPreference.ProcurementPortalName is a required enum member. Real AWS accepts four portals and, when a request carries an unmodeled value, refuses with HTTP 400 ValidationException whose Message is the standard AWS constraint-violation template: fixed text `1 validation error detected: Value at '<memberName>' failed to satisfy constraint: Member must satisfy enum value set: [<comma-separated accepted values>]`, where `<memberName>` is the wire (lowerCamelCase) member name taken from the request — here `procurementPortalName` — and `<...>` is the service's full accepted enum value set, recorded as `[ZIP, SAP_BUSINESS_NETWORK, JAGGAER, COUPA]`. Status 400 and code `ValidationException` are taken from the recorded response; the member-name and template wording are the documented AWS validation-message rule. Smithy (on-disk model services/service-com-amazonaws-invoicing-invoicing/smithy-model/com-amazonaws-invoicing-invoicing.json): CreateProcurementPortalPreference input ProcurementPortalName -> enum ProcurementPortalName with values only [SAP_BUSINESS_NETWORK, COUPA]; error ValidationException @httpError 400. The checked-in model is therefore itself stale relative to the recorded AWS behavior, which lists four values — recorded behavior is authoritative here.

Emulator: status 400 and error code `ValidationException` are correct, but the message is `1 validation error detected: Value at 'procurementPortalName' failed to satisfy constraint: Member must satisfy enum value set: [SAP_BUSINESS_NETWORK, COUPA]` — the enum value set omits `ZIP` and `JAGGAER` and the ordering of the advertised set does not match AWS's. By construction the same narrowed set is what the handler validates against, so a well-formed request naming `ZIP` or `JAGGAER` is refused with ValidationException where AWS accepts it and creates the preference.

Why: a client reading the message to discover which portals are supported (or asserting on it, as integration tests and SDK error-mapping layers do) is told two of the four AWS portals do not exist. Worse, any caller configuring a ZIP or JAGGAER procurement portal cannot onboard at all against the emulator: a request AWS accepts comes back as a client-side validation failure, so the code path that creates and then reads back the preference is untestable.

Fix: the ProcurementPortalName enum accepted-value set behind CreateProcurementPortalPreference's input validation in the invoicing service — extend it to the four AWS values and render the message's `[...]` list from that same set in AWS's order, so the accepted set and the advertised set cannot drift. The on-disk Smithy enum for ProcurementPortalName should be refreshed in the same change. Note the error status, code and message template are otherwise already correct, and the per-field independence of validation (the buyerIdentifier `\S+` pattern message) is a separate assertion in the same check that was never reached — re-replay it after the fix. Two open invoicing issues (ListInvoiceSummaries filtered-query refusal, BatchGetInvoiceProfile cross-account authorization) are unrelated to this cause and do not cover it.

Traces: invoicing.create-procurement-portal-preference-duplicate-conflict-rejected.

## Evidence
Check invoicing.create-procurement-portal-preference-duplicate-conflict-rejected, first assertion.

Request: CreateProcurementPortalPreference with ProcurementPortalName="NOT_A_REAL_PORTAL", BuyerDomain="NetworkID", BuyerIdentifier=<valid \S+ string>, SupplierDomain="NetworkID", SupplierIdentifier=<valid \S+ string>, EinvoiceDeliveryEnabled=false, PurchaseOrderRetrievalEnabled=true, Contacts=[one contact].

Expected (AWS): HTTP 400, Error.Code = "ValidationException", Error.Message = "1 validation error detected: Value at 'procurementPortalName' failed to satisfy constraint: Member must satisfy enum value set: [ZIP, SAP_BUSINESS_NETWORK, JAGGAER, COUPA]"

Actual (emulator): HTTP 400, Error.Code = "ValidationException" (correct), Error.Message = "1 validation error detected: Value at 'procurementPortalName' failed to satisfy constraint: Member must satisfy enum value set: [SAP_BUSINESS_NETWORK, COUPA]"

AssertionError: expected message "... [ZIP, SAP_BUSINESS_NETWORK, JAGGAER, COUPA]", got "... [SAP_BUSINESS_NETWORK, COUPA]".

Message anatomy — fixed text: "1 validation error detected: Value at '", "' failed to satisfy constraint: Member must satisfy enum value set: [", "]". Variable from the request: the member name `procurementPortalName`. Variable from service configuration (not from any one run): the accepted enum value set, which must be the four AWS portals. No per-run value (request id, timestamp, generated id) participates in this message.

## Steps to Reproduce
1. Boot the emulator and point an invoicing client at it.
2. Call CreateProcurementPortalPreference with all required members well-formed (BuyerDomain="NetworkID", BuyerIdentifier and SupplierIdentifier matching ^\S+$, EinvoiceDeliveryEnabled=false, PurchaseOrderRetrievalEnabled=true, exactly one Contacts entry) but ProcurementPortalName="NOT_A_REAL_PORTAL".
3. Observe HTTP 400 ValidationException; read Error.Message. The enum value set it advertises is [SAP_BUSINESS_NETWORK, COUPA] instead of AWS's [ZIP, SAP_BUSINESS_NETWORK, JAGGAER, COUPA].
4. Repeat the same call with ProcurementPortalName="ZIP" (and again with "JAGGAER"): the emulator refuses with ValidationException, where AWS accepts the request and returns a ProcurementPortalPreferenceArn.
