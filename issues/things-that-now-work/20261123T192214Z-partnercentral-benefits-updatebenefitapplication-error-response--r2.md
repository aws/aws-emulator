Issue-Severity: LOW
Issue-Gap-Class: error-response
Issue-Commit: a8012ac7679e3d1383f84f202c2fe4311ebc42ff

# The update handler's documented answer order puts its own unconstrained-member bounds before any stored record is read, while the handler reads the catalog and applies the entitlement gate first

_Filed: 2026-11-23T19:22:14Z_
_Operation: UpdateBenefitApplication | Class: error-response_

> _Note: this issue was identified against the code base at commit `a8012ac7679e3d1383f84f202c2fe4311ebc42ff`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`src/ops/update_benefit_application.rs` opens with a numbered list that claims to be the order in which this operation answers refusals, and claims it is also the order its sibling write paths answer in. Step 1 is "request-shape validation — the centralized walk AND the bounds this service puts on the members the model leaves unconstrained, before any stored record is read"; step 2 is "the catalog and the partner-entitlement gate".

The handler under that header does the opposite for the half of step 1 that is the handler's own: the first thing `handle` does is call `crate::state::write_request_refusal`, which READS the catalog record out of the store (and takes the store lock to do it) and answers `ResourceNotFoundException` for a catalog this service does not hold, then `AccessDeniedException` for an un-entitled caller. Only after that gate has passed does the handler apply the bounds it enforces itself — `crate::state::details_refusal` on the unconstrained `BenefitApplicationDetails` document and `crate::state::unconstrained_members_refusal` on `PartnerContacts[].Phone` / `FileDetails[].BusinessUseCase`. The handler's own body comment states the contrary order out loud: "The catalog this write would land in is resolved FIRST".

So one logical request that fails both rules — an update naming a catalog this service does not hold (or coming from an un-entitled account) and carrying a `Phone` past this service's bound — is answered `ResourceNotFoundException` / `AccessDeniedException`, where the header promises the `ValidationException` of step 1. The header also mis-describes the siblings it cites: `crate::ops::amend_benefit_application` likewise runs `write_request_refusal` before its own `amendments`/`amendmentReason`/`clientToken` length bounds. Either the list or the handler is wrong; as written, the file that a reader would copy an answer order from contradicts the code beneath it.

(The already-open issue 20261119T095241Z is about the amend header's catalog-vs-gate order and its dangling `ENTITLEMENT_REQUIRED` link — a different claim, in a different file; it even asserts "Every other write handler's header matches its body on this point", which is true of the catalog/gate order this header states correctly as step 2. The contradiction reported here is step 1 versus step 2.)

## Evidence

The header's ordered list — src/ops/update_benefit_application.rs:15-24:
    //! The operation answers in this order, which is the order its sibling write paths answer
    //! in (`crate::ops::amend_benefit_application`, and the resource writes beside it):
    //!
    //! 1. request-shape validation — the centralized walk and the bounds this service puts on
    //!    the members the model leaves unconstrained, before any stored record is read;
    //! 2. the catalog and the partner-entitlement gate;
    //! 3. the `ClientToken` replay check;

The body of the same file, which answers step 2 FIRST and says so — src/ops/update_benefit_application.rs:246-253:
        // The catalog this write would land in is resolved FIRST, and the
        // partner-entitlement gate applies to a request whose catalog resolved — see
        // [`crate::state::write_request_refusal`].
        match crate::state::write_request_refusal(&*self.engine, &input.catalog) {
            Some(crate::state::WriteRequestRefusal::MissingCatalog(missing)) => {
                return Err(
                    UpdateBenefitApplicationErrorModeled::ResourceNotFoundException(missing).into(),
                )

The step-1 bounds the handler enforces itself, reached only after that gate — src/ops/update_benefit_application.rs:279-290:
        if let Some(refusal) = crate::state::details_refusal(
            crate::state::DETAILS_MEMBER,
            input.benefit_application_details.as_ref(),
        ) {
            return Err(UpdateBenefitApplicationErrorModeled::ValidationException(refusal).into());
        }
        if let Some(refusal) = crate::state::unconstrained_members_refusal(
            input.partner_contacts.as_deref(),
            input.file_details.as_deref(),
        ) {
            return Err(UpdateBenefitApplicationErrorModeled::ValidationException(refusal).into());
        }

That the gate it runs first DOES read a stored record (so "before any stored record is read" cannot hold) — src/state.rs:3376-3385:
    pub fn write_request_refusal<Eng: Send + Sync + 'static>(
        engine: &Eng,
        catalog: &str,
    ) -> Option<WriteRequestRefusal> {
        let state = service_state(engine)?;
        let store = state.lock();
        if let Some(missing) = store.catalog_refusal(catalog) {
            return Some(WriteRequestRefusal::MissingCatalog(missing));
        }
        store.entitlement_refusal().map(WriteRequestRefusal::NotEntitled)
    }

The sibling the header cites, which also puts its own bounds after the same gate — src/ops/amend_benefit_application.rs:836 (`match crate::state::write_request_refusal(&*self.engine, &input.catalog) {`) versus src/ops/amend_benefit_application.rs:863-865:
        if let Some(clause) = amendments_length_clause(input.amendments.len()) {
            return Err(refused(AMENDMENTS_MEMBER, &clause));
        }

Test coverage: tests/all/write_gate_order.rs pins catalog-before-gate for all eight writing operations, but only with well-formed bodies (`catalog_bound_requests()` + `with_catalog`), so it says nothing about this ordering; tests/all/unconstrained_member_bounds.rs:12-25 pins the step-1 refusals themselves ("accepted up to this service's own bound … and refused 400 ValidationException past it") on create, update and amend, always with a catalog the service holds and an entitled caller. No test asserts the relative order of the two, so correcting the header changes no response and no test.

## Steps to Reproduce

1. Read src/ops/update_benefit_application.rs:15-24: step 1 of the documented answer order is the centralized walk AND "the bounds this service puts on the members the model leaves unconstrained, before any stored record is read"; step 2 is the catalog and the entitlement gate.
2. Read src/ops/update_benefit_application.rs:246-261: the first statement of `handle` is `crate::state::write_request_refusal(&*self.engine, &input.catalog)`, whose `MissingCatalog` arm returns `ResourceNotFoundException` and whose `NotEntitled` arm returns `AccessDeniedException`.
3. Read src/state.rs:3376-3385 and confirm that helper locks the store and reads the catalog record before either refusal — i.e. a stored record IS read at step 2.
4. Read src/ops/update_benefit_application.rs:279-290 and note that `details_refusal` and `unconstrained_members_refusal` (the step-1 bounds this service owns) are only reached after that gate returned `None`.
5. Derive the observable difference: an `UpdateBenefitApplication` naming a catalog the service does not hold (or sent from an un-entitled account) while also carrying a `PartnerContacts[0].Phone` longer than `crate::state::MAX_DETAILS_VALUE_LEN` is answered the 404/denial, not the `ValidationException` the header's step 1 promises.
6. Confirm the header's claim about its siblings fails too: src/ops/amend_benefit_application.rs:836 runs `write_request_refusal` before its own `amendments_length_clause` at :863.
