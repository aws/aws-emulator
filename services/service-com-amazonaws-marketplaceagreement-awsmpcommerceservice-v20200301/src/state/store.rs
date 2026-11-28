// SPDX-License-Identifier: CC0-1.0

//! What the one store this service keeps does: the ids it mints, the records it
//! resolves, the transitions the clock owes them and the refusals a request
//! about a record this store does not hold earns.
//!
//! Moved out of `state.rs` unchanged, so the module root keeps the shapes and
//! the refusal constructors while the store's own behaviour lives here; every
//! name stays reachable through `crate::state`.

use super::*;

impl ComAmazonawsMarketplaceagreementAwsmpcommerceserviceV20200301State {
    /// The next value of the shared id counter.
    fn next_sequence(&mut self) -> u64 {
        self.id_sequence = self.id_sequence.wrapping_add(1);
        self.id_sequence
    }

    /// Mint an unused agreement request id matching `AgreementRequestId`'s
    /// `^ar-[A-Za-z0-9]+$`.
    pub fn mint_agreement_request_id(&mut self) -> String {
        loop {
            let id = format!("ar-{:016x}", self.next_sequence());
            if !self.agreement_requests.contains_key(&id) {
                return id;
            }
        }
    }

    /// Mint an unused cancellation request id matching
    /// `AgreementCancellationRequestId`'s `^acr-[a-zA-Z0-9]+$`.
    pub fn mint_agreement_cancellation_request_id(&mut self) -> String {
        loop {
            let id = format!("acr-{:016x}", self.next_sequence());
            if !self.agreement_cancellation_requests.contains_key(&id) {
                return id;
            }
        }
    }

    /// Mint an unused agreement approval id matching `AgreementApprovalId`'s
    /// `^apr-[A-Za-z0-9]+$`.
    pub fn mint_agreement_approval_id(&mut self) -> String {
        loop {
            let id = format!("apr-{:016x}", self.next_sequence());
            if !self.agreement_approvals.contains_key(&id) {
                return id;
            }
        }
    }

    /// Mint an approver outcome id matching `ApproverOutcomeId`'s
    /// `^aprout-[A-Za-z0-9-]+$`.
    ///
    /// Drawn from the same counter every id of this store is drawn from, so an
    /// outcome id is unique across the store and names exactly one approver of
    /// one approval — which is what `approverOutcomeIdToOverride` resolves
    /// against.
    pub fn mint_approver_outcome_id(&mut self) -> String {
        format!("aprout-{:016x}", self.next_sequence())
    }

    /// Mint an unused agreement id matching `ResourceId`'s
    /// `^[A-Za-z0-9_/-]+$`.
    pub fn mint_agreement_id(&mut self) -> String {
        loop {
            let id = format!("agmt-{:016x}", self.next_sequence());
            if !self.agreements.contains_key(&id) {
                return id;
            }
        }
    }

    /// Mint an unused payment request id matching `PaymentRequestId`'s
    /// `^pr-[a-zA-Z0-9]+$`.
    pub fn mint_payment_request_id(&mut self) -> String {
        loop {
            let id = format!("pr-{:016x}", self.next_sequence());
            if !self.payment_requests.contains_key(&id) {
                return id;
            }
        }
    }

    /// Mint an unused billing adjustment request id matching
    /// `BillingAdjustmentRequestId`'s `^ba-[a-zA-Z0-9]+$`.
    pub fn mint_billing_adjustment_request_id(&mut self) -> String {
        loop {
            let id = format!("ba-{:016x}", self.next_sequence());
            if !self.billing_adjustment_requests.contains_key(&id) {
                return id;
            }
        }
    }

    /// Mint an unused invoice id matching `ResourceId`'s `^[A-Za-z0-9_/-]+$`,
    /// which is how `AgreementInvoiceLineItemGroupSummary$invoiceId` is
    /// modelled.
    pub fn mint_invoice_id(&mut self) -> String {
        loop {
            let id = format!("inv-{:016x}", self.next_sequence());
            if !self.invoices.contains_key(&id) {
                return id;
            }
        }
    }

    /// Issue the invoices accepting `agreement_id` bills, answering their ids in
    /// the order they were issued.
    ///
    /// One invoice per charge of the quote the acceptance applied that is
    /// charged on acceptance and carries an amount — see [`Invoice`] for why
    /// those are the charges this service can invoice. That quote is the LAST
    /// one of the agreement ([`Self::quotes_of`]): the one it was accepted from
    /// for an agreement an acceptance created, and the amendment's own for an
    /// acceptance that amended one, whose charges nothing has billed before.
    /// The amount and currency are READ from that quote, which priced the charge
    /// once, so an invoice never states a second amount for a charge the
    /// agreement already reports.
    pub fn issue_acceptance_invoices(
        &mut self,
        agreement_id: &str,
        now: SystemTime,
    ) -> Vec<String> {
        let Some(summary) = self
            .agreements
            .get(agreement_id)
            .and_then(|agreement| self.quotes_of(agreement).last())
            .map(|request| request.charge_summary.clone())
        else {
            return Vec::new();
        };
        let Some(currency_code) = summary.currency_code else {
            return Vec::new();
        };
        // One pair per charge this acceptance bills: the charge, and the amount
        // the quote priced it at.
        let billed: Vec<(String, String)> = summary
            .expected_charges
            .iter()
            .flatten()
            .filter(|charge| charge.timing.as_deref() == Some(TIMING_ON_ACCEPTANCE))
            .filter_map(|charge| {
                let id = charge.id.clone()?;
                let amount = charge
                    .amount
                    .clone()
                    .filter(|amount| is_positive_amount(amount))?;
                Some((id, amount))
            })
            .collect();

        let mut issued = Vec::with_capacity(billed.len());
        for (charge_id, amount) in billed {
            let id = self.mint_invoice_id();
            self.invoices.insert(
                id.clone(),
                Invoice {
                    id: id.clone(),
                    agreement_id: agreement_id.to_string(),
                    charge_id,
                    // Nothing has been adjusted yet, so the whole of what the
                    // invoice bills is open to adjustment.
                    open_amount: amount.clone(),
                    amount,
                    currency_code: currency_code.clone(),
                    invoice_type: INVOICE_TYPE_INVOICE.to_string(),
                    issued_at: now,
                },
            );
            issued.push(id);
        }
        issued
    }

    /// The invoice of `agreement_id` that `invoice_id` names, or `None` when
    /// that id names no invoice of THAT agreement.
    ///
    /// An invoice belongs to one agreement, so an id naming an invoice of
    /// another agreement resolves to nothing here: an adjustment is made to an
    /// invoice OF the agreement its entry names.
    pub fn invoice_of(&self, agreement_id: &str, invoice_id: &str) -> Option<&Invoice> {
        self.invoices
            .get(invoice_id)
            .filter(|invoice| invoice.agreement_id == agreement_id)
    }

    /// The invoices of one agreement, in the order they were issued.
    pub fn invoices_of(&self, agreement_id: &str) -> Vec<&Invoice> {
        self.invoices
            .values()
            .filter(|invoice| invoice.agreement_id == agreement_id)
            .collect()
    }

    /// Hold `units` of the invoice `invoice_id` names against that invoice:
    /// what one stored adjustment request asks to credit is no longer open to a
    /// later one.
    ///
    /// Called once, by the submit that stores the request, so the invoice and
    /// the requests against it never disagree. An amount larger than what is
    /// open is refused before it is stored, so this never drives the open amount
    /// below zero; if it somehow were, nothing is left open.
    pub fn hold_against_invoice(&mut self, invoice_id: &str, units: u128) {
        if let Some(invoice) = self.invoices.get_mut(invoice_id) {
            invoice.open_amount = amount_of_units(invoice.open_units().saturating_sub(units));
        }
    }

    /// Mint the ARN of the AWS License Manager licence one entitlement grants.
    ///
    /// The licence id carries License Manager's own `l-` prefix, and the ARN is
    /// built through [`::engine::emulator_arn`], so its account segment is the
    /// emulator's canonical one and its region is the one the accepting caller
    /// signed for, read through [`request_region`] so that segment is a Region
    /// label rather than whatever text the credential scope carried. Every ARN
    /// this mints therefore satisfies `AwsArn`'s pattern, whose resource part
    /// admits the `license:<id>` form License Manager uses — and, because that
    /// Region grammar is a subset of it, satisfies `FilterValue`'s pattern too,
    /// so a minted licence can always be passed to `SearchAgreements`'
    /// `LicenseArn` filter.
    fn mint_license_arn(&mut self, region: &str) -> String {
        let license_id = format!("l-{:016x}", self.next_sequence());
        ::engine::emulator_arn(
            LICENSE_ARN_SERVICE,
            region,
            &format!("license:{license_id}"),
        )
    }

    /// The entitlements accepting an agreement provisions, each with its own
    /// freshly minted licence, its own registration token and the instant its
    /// provisioning is due to finish.
    ///
    /// `AgreementEntitlement` "represents an entitlement associated with an
    /// agreement, including the provisioning status, resource, and type", and
    /// `Resource` is "the resource that the entitlement is provisioned to, such
    /// as a product" — so an agreement grants one entitlement per resource it
    /// is involved with, which is the list [`agreement_resources`] resolves for
    /// the agreement being accepted (never empty, so every agreement grants at
    /// least one entitlement).
    pub fn provision_entitlements(
        &mut self,
        resources: &[Resource],
        region: &str,
        now: SystemTime,
    ) -> Vec<AgreementEntitlementRecord> {
        let provisioned_at = now + ENTITLEMENT_PROVISIONING;
        let registration_token_expires_at = now + REGISTRATION_TOKEN_VALIDITY;
        resources
            .iter()
            .map(|resource| AgreementEntitlementRecord {
                resource: resource.clone(),
                license_arn: self.mint_license_arn(region),
                registration_token: mint_registration_token(),
                registration_token_expires_at,
                provisioned_at,
            })
            .collect()
    }

    /// Refuse an `agreementId` that names no agreement this store holds, for
    /// every operation that takes the member as INPUT rather than as the
    /// resource it acts on.
    ///
    /// The same resolution [`selector_value_refusal`] applies to the members
    /// whose value set is fixed, applied to the member that names a RESOURCE of
    /// the account: the model gives this service its own `AgreementId` shape
    /// beside the generic `ResourceId`, and binds it to the listings that
    /// RESOLVE the agreement they name — `ListAgreementPaymentRequests`,
    /// `ListAgreementCancellationRequests` and `ListBillingAdjustmentRequests`
    /// (`smithy_query(query="operation",
    /// operation="ListBillingAdjustmentRequests")`). The behavioural corpus
    /// records that resolution: a call of one of them naming an agreement the
    /// account does not hold is refused rather than answered with an empty page,
    /// so a caller can tell "this agreement has no such records" from "this
    /// agreement does not exist". They model no `ResourceNotFoundException`, so
    /// the refusal is their modelled `ValidationException` carrying the
    /// `INVALID_AGREEMENT_ID` reason.
    ///
    /// The SAME refusal is what every operation on a PAYMENT REQUEST answers
    /// for that member, even though each of them does model
    /// `ResourceNotFoundException`: a payment request is the resource those
    /// calls act on and `agreementId` only says which agreement it was sent
    /// under, so a value that resolves to no agreement is a fault in the input
    /// and not a reference to a resource that was looked for. The recorded AWS
    /// responses for this service draw exactly that line — a well-formed
    /// `agreementId` the account does not hold is answered with
    /// `ValidationException` / `INVALID_AGREEMENT_ID` by `SendAgreementPaymentRequest`,
    /// `GetAgreementPaymentRequest`, `AcceptAgreementPaymentRequest`,
    /// `RejectAgreementPaymentRequest` and `CancelAgreementPaymentRequest`,
    /// while the operations whose own resource IS the agreement
    /// (`DescribeAgreement`, `CancelAgreement`, `GetAgreementTerms`,
    /// `GetAgreementEntitlements`, `ListAgreementInvoiceLineItems`,
    /// `CreateAgreementRequest`'s source) and the reads and decisions on the
    /// other child families (the agreement cancellation request and the billing
    /// adjustment request) answer the modelled not-found naming `Agreement`. So
    /// `ResourceNotFoundException` stays the answer of the payment-request calls
    /// for the one resource they look for, the payment request itself
    /// ([`payment_request_not_found`]).
    ///
    /// `ListAgreementCharges` is NOT one of them: its selector targets the
    /// generic `ResourceId`, and the corpus records that operation serving the
    /// empty page for the same class of value — so it filters where these three
    /// resolve.
    ///
    /// The member failed by naming nothing rather than by carrying a value this
    /// service does not admit, so the `fields` entry is the
    /// unresolved-reference one ([`unresolved_reference`]).
    pub fn unknown_agreement_refusal(
        &self,
        agreement_id: Option<&str>,
    ) -> Option<ValidationException> {
        match agreement_id {
            Some(id) if !self.agreements.contains_key(id) => {
                Some(unresolved_reference("INVALID_AGREEMENT_ID", "agreementId"))
            }
            _ => None,
        }
    }

    /// The payment request `token` already sent, if any.
    ///
    /// `SendAgreementPaymentRequestInput$clientToken` is modelled
    /// `@idempotencyToken`, so a replayed token names the request it first
    /// minted. The token lives on the record itself rather than in a second map,
    /// so the two can never diverge and the lookup is bounded by the same
    /// [`PAYMENT_REQUEST_QUOTA`] that bounds the store.
    pub fn payment_request_for_token(&self, token: &str) -> Option<&PaymentRequest> {
        self.payment_requests
            .values()
            .find(|request| request.client_token.as_deref() == Some(token))
    }

    /// What the payment requests already sent under one `VariablePaymentTerm` of
    /// one agreement stand to charge, in the units [`amount_units`] counts.
    ///
    /// The ceiling is "the maximum total amount that can be charged to the
    /// customer through variable payment requests under this term", so what
    /// consumes it is the amount of every request of that term that has been or
    /// may yet be charged ([`PaymentRequest::holds_term_balance`]) — read off
    /// the stored requests themselves, so no running total can drift from them.
    pub fn charged_payment_units(&self, agreement_id: &str, term_id: &str) -> u128 {
        self.payment_requests
            .values()
            .filter(|request| request.agreement_id == agreement_id && request.term_id == term_id)
            .filter(|request| request.holds_term_balance())
            .fold(0u128, |total, request| {
                total.saturating_add(request.charge_units())
            })
    }

    /// Refuse a `chargeAmount` that cannot be charged under the
    /// `VariablePaymentTerm` `term` of the agreement `agreement_id`.
    ///
    /// `SendAgreementPaymentRequest` states both bounds on the member and the
    /// one refusal they share: the amount is a "positive decimal value in the
    /// currency of the accepted term" and it "must not exceed the remaining
    /// available balance under the `VariablePaymentTerm`
    /// `maxTotalChargeAmount`", and "a `ValidationException` is returned if the
    /// `chargeAmount` exceeds the available balance". So an amount that is not
    /// positive or is not one this service can count ([`is_acceptable_amount`]),
    /// and one that would take the requests of that term past its ceiling
    /// ([`term_ceiling_units`], [`Self::charged_payment_units`]), are the same
    /// input fault on the same member.
    ///
    /// The remaining balance, not the single amount, is what bounds a send: a
    /// request within the ceiling on its own is still refused once the term's
    /// earlier requests have consumed it. `None` — nothing to refuse — when the
    /// term publishes no ceiling that can be measured against.
    pub fn payment_charge_refusal(
        &self,
        agreement_id: &str,
        term: &VariablePaymentTerm,
        charge_amount: &str,
    ) -> Option<ValidationException> {
        let refused =
            || Some(validation_exception(REASON_INVALID_CHARGE_AMOUNT, "chargeAmount"));
        if !is_acceptable_amount(charge_amount) {
            return refused();
        }
        let Some(requested) = amount_units(charge_amount) else {
            return refused();
        };
        let (Some(ceiling), Some(term_id)) = (term_ceiling_units(term), term.id.as_deref()) else {
            return None;
        };
        let charged = self.charged_payment_units(agreement_id, term_id);
        if charged.saturating_add(requested) > ceiling {
            return refused();
        }
        None
    }

    /// Approve the payment request `payment_request_id` names at `now`,
    /// answering the approved record.
    ///
    /// Approving one is what CREATES its charge: `chargeId` is "the unique
    /// identifier of the charge created after the payment request is approved",
    /// so the charge is minted here from the store's single id counter
    /// ([`Self::mint_charge_id`]) and is from then on a charge of the agreement
    /// the request was sent under ([`Self::charges_of`]). The buyer's optional
    /// purchase order reference — what they "can provide to associate the
    /// payment request with their internal purchase order system" — is recorded
    /// as that charge's own association
    /// ([`Agreement::associate_purchase_order`]), which is the one place this
    /// service holds a charge's purchase order, so `ListAgreementCharges`
    /// reports it and `UpdatePurchaseOrders` replaces it later rather than
    /// there being a second copy to keep in step.
    ///
    /// Answers `None` when no stored request carries that id: callers resolve
    /// the request and check [`PaymentRequest::is_pending_approval`] first.
    pub fn approve_payment_request(
        &mut self,
        payment_request_id: &str,
        purchase_order_reference: Option<String>,
        now: SystemTime,
    ) -> Option<PaymentRequest> {
        if !self.payment_requests.contains_key(payment_request_id) {
            return None;
        }
        let charge_id = self.mint_charge_id();
        let request = self.payment_requests.get_mut(payment_request_id)?;
        request.approve(now, charge_id.clone());
        let approved = request.clone();
        let Some(reference) = purchase_order_reference else {
            return Some(approved);
        };
        if let Some(agreement) = self.agreements.get_mut(&approved.agreement_id) {
            agreement.associate_purchase_order(
                PurchaseOrder {
                    charge_id,
                    // The charge was just minted, so it is at its first
                    // revision ([`INITIAL_CHARGE_REVISION`]).
                    charge_revision: Some(INITIAL_CHARGE_REVISION),
                    // "The unique identifier of the agreement associated with
                    // this charge": the agreement the request was sent under.
                    agreement_id: Some(approved.agreement_id.clone()),
                    purchase_order_reference: Some(reference),
                },
                now,
            );
        }
        Some(approved)
    }

    /// The stored billing adjustment request submitted under `token`, if any.
    ///
    /// The token is held on the request itself rather than in a second index, so
    /// there is exactly one copy of each request to read.
    pub fn billing_adjustment_by_client_token(
        &self,
        token: &str,
    ) -> Option<&BillingAdjustmentRequest> {
        self.billing_adjustment_requests
            .values()
            .find(|request| request.client_token == token)
    }

    /// Materialise what the end date of the ONE agreement `agreement_id` names
    /// has already made due by `now`, and the same for each renewal that
    /// creates — or nothing when this store holds no agreement under that id.
    ///
    /// An end date makes exactly one of two things due, which is what the
    /// behaviour at it says ([`Agreement::end_time_behavior_at_end_date`]): the
    /// agreement expires there ([`Agreement::expire`]), or it is renewed into a
    /// new agreement there ([`Self::renew_agreement`]). Both are owed at that
    /// instant whether or not anybody calls, so every operation that reports or
    /// guards on an agreement's status materialises them before it answers.
    ///
    /// A renewal starts where its source ended, so its OWN end date can be past
    /// too: the chain is walked until an agreement whose end date is still ahead
    /// or that renews no further, which `RenewalTerm$maxRenewals` bounds. The
    /// loop is additionally capped by [`RENEWALS_PER_CALL`] for a renewal term
    /// that publishes no limit at all.
    ///
    /// What settles the chain of an agreement this store HOLDS: the record an
    /// answer is about is the only one that answer depends on, so it and the
    /// chain it starts are the only ones to bring up to date. An operation
    /// RESOLVING an id calls [`Self::settle_before_lookup`], which adds what an
    /// id this store holds nothing under needs.
    pub fn settle_agreement(&mut self, agreement_id: &str, now: SystemTime) {
        let mut id = agreement_id.to_string();
        for _ in 0..=RENEWALS_PER_CALL {
            if let Some(agreement) = self.agreements.get_mut(&id) {
                agreement.expire(now);
            }
            match self.renew_agreement(&id, now) {
                Some(renewal) => id = renewal,
                None => return,
            }
        }
    }

    /// Materialise every end-date transition the clock has already made due by
    /// `now`, for every agreement this store holds ([`Self::settle_agreement`]).
    ///
    /// What an operation that SCANS the store calls, where the answer depends on
    /// every agreement rather than on one: the status an agreement past its own
    /// end date is searched by, the summary status reported beside it, and the
    /// renewal agreement its end date created are then the ones the clock
    /// already owed. An operation answering about a single agreement calls
    /// [`Self::settle_before_lookup`] instead.
    ///
    /// The ids are taken before the walk, because a renewal inserts a new
    /// agreement: each chain is settled to its end by the walk of its first
    /// link, so the ones it inserted need no second pass.
    pub fn settle_agreements(&mut self, now: SystemTime) {
        let ids: Vec<String> = self.agreements.keys().cloned().collect();
        for id in ids {
            self.settle_agreement(&id, now);
        }
        // Every chain this store holds is up to date as at `now`, which is what
        // lets a walk asked for again at the same instant be skipped
        // ([`Self::settled_through`]).
        self.settled_through = Some(now);
    }

    /// Materialise what the clock has already made due BEFORE the id
    /// `agreement_id` is resolved: the chain that id starts
    /// ([`Self::settle_agreement`]) and, only when this store holds no
    /// agreement under it, every chain this store holds
    /// ([`Self::settle_agreements`]).
    ///
    /// What an operation answering about a SINGLE agreement calls. The second
    /// half is what a renewal makes necessary: the agreement a `RENEW` end date
    /// creates is owed at that date whether or not anybody calls, and until the
    /// chain that owes it is walked this store holds nothing under its id — so
    /// an id that resolves to nothing is not yet an id that NAMES nothing, and
    /// answering the not-found refusal for it would make the answer depend on
    /// whether some other call had walked that chain first. Once every chain is
    /// settled the id either names the agreement the clock created or names
    /// nothing at all, and the operation's own refusal is what the latter
    /// earns. The store-wide walk runs only on that miss, and only when no walk
    /// has already brought every chain up to THIS instant
    /// ([`Self::settled_through`]) — so resolving an agreement this store
    /// already holds stays the settling of its own chain, and repeated misses at
    /// one instant do not pay for the same walk again.
    pub fn settle_before_lookup(&mut self, agreement_id: &str, now: SystemTime) {
        self.settle_agreement(agreement_id, now);
        let settled = self.settled_through.is_some_and(|through| through >= now);
        if !self.agreements.contains_key(agreement_id) && !settled {
            self.settle_agreements(now);
        }
    }

    /// Materialise the renewal the end date of the agreement `agreement_id`
    /// names has already made due by `now`, answering the id of the renewal
    /// agreement it created — or `None` when no renewal is due.
    ///
    /// `AgreementStatus` documents both halves of this transition: the source
    /// agreement becomes "RENEWED – The agreement was renewed into a new
    /// agreement (for example, an auto-renewal)", and `EndTimeBehaviorType`
    /// `RENEW` says what the new agreement is — "a new agreement is created from
    /// the accepted terms of this agreement". So the renewal re-states the terms
    /// the source's own quote asked for, over the window that starts where the
    /// source ended:
    ///
    /// * it continues the same chain, so it carries the source's
    ///   `initialAgreementId` ("it stays the same across all agreements in that
    ///   chain, which lets you trace an agreement back to the original") and one
    ///   more than its [`Agreement::renewal_count`];
    /// * it is created AT the source's end date, so that instant is its
    ///   `startTime` and its `acceptanceTime`, and its own end date is a further
    ///   contract length on ([`agreement_end_time`]) — nothing here is dated at
    ///   the instant somebody looked, so repeated reads report the same record;
    /// * it is a new purchase of those terms rather than an amendment of the
    ///   source's, so it gets its OWN quote ([`Self::charge_summary`], at
    ///   [`INTENT_NEW`], whose charges nothing else holds), the invoices that
    ///   quote bills on acceptance ([`Self::issue_acceptance_invoices`]) and its
    ///   own freshly provisioned entitlements
    ///   ([`Self::provision_entitlements`]) — the purchase orders the acceptor
    ///   associated with the SOURCE's charges stay with those charges, since the
    ///   renewal's charges are not the ones they were associated with.
    ///
    /// Nothing is due unless the source is still `ACTIVE`, its stored end date
    /// has passed and its terms renew it there
    /// ([`Agreement::renews_at_end_date`]), and the transition itself is what
    /// makes this happen once: the source leaves `ACTIVE` for [`STATUS_RENEWED`]
    /// ([`Agreement::renew`]), so a later call finds nothing due for it and
    /// mints no second renewal. An agreement whose own quote this store no
    /// longer holds is left as it stands: the terms the renewal would re-state
    /// are read from that quote.
    ///
    /// Nothing is due either once the store has reached the ceiling a create
    /// declines at ([`AGREEMENT_REQUEST_QUOTA`]): a renewal inserts a quote and
    /// an agreement and issues that agreement's invoices, exactly what the
    /// create path checks that ceiling before doing, so past it the chain is
    /// left where it stands rather than taking the store further. That bound is
    /// what keeps the records a clock-driven renewal creates finite however many
    /// calls settle however many chains — [`RENEWALS_PER_CALL`] bounds ONE
    /// walk's steps, this bounds the store.
    fn renew_agreement(&mut self, agreement_id: &str, now: SystemTime) -> Option<String> {
        if self.agreement_requests.len() >= AGREEMENT_REQUEST_QUOTA
            || self.agreements.len() >= AGREEMENT_REQUEST_QUOTA
        {
            return None;
        }
        let source = self.agreements.get(agreement_id)?;
        if source.status != STATUS_ACTIVE
            || !source.reached_end_date(now)
            || !source.renews_at_end_date()
        {
            return None;
        }
        // The instant the renewal is due at, which is the source's end date.
        let end = source.end_time?;
        let source = source.clone();
        // The quote the source STANDS AT ([`Self::quotes_of`]): "a new agreement
        // is created from the accepted terms of this agreement", so an
        // agreement an amendment modified is renewed over the terms that
        // amendment accepted rather than the ones it held before.
        let quote = self.quotes_of(&source).last()?.clone();
        let requested_terms = quote.requested_terms;

        // The renewal's own quote, which prices the same requested terms afresh
        // for the new window: its charge ids are minted here, so the renewal
        // reports charges of its own rather than the source's. It is priced in
        // the Region the source agreement was accepted in, since a renewal is
        // the same purchase continued rather than one made somewhere else.
        let charge_summary =
            self.charge_summary(&requested_terms, INTENT_NEW, None, &source.region);
        let request_id = self.mint_agreement_request_id();
        let id = self.mint_agreement_id();
        let context = AcceptedTermContext::new(&requested_terms, end, Some(&source));
        let terms = accepted_terms(&requested_terms, &context);
        let end_time = agreement_end_time(&terms, &context);
        // The renewal entitles its acceptor to the same resources the source's
        // chain is involved with, each with its own newly provisioned licence.
        let resources = agreement_resources(source.proposal_summary.resources.as_deref());
        let entitlements = self.provision_entitlements(&resources, &source.region, end);
        self.agreement_requests.insert(
            request_id.clone(),
            AgreementRequest {
                id: request_id.clone(),
                // A renewal buys the accepted terms for a new window rather
                // than changing what the source bought, so its quote is quoted
                // as a new purchase: nothing of the source's quantities is
                // carried into it.
                intent: INTENT_NEW.to_string(),
                requested_terms,
                source_agreement_identifier: None,
                // The proposal and the tax estimation the source's own quote
                // named: the renewal re-states that quote's terms.
                agreement_proposal_identifier: quote.agreement_proposal_identifier,
                tax_configuration: quote.tax_configuration,
                // The contact the source's own quote carried: a renewal is the
                // same acceptor continuing the same purchase.
                acceptor_contact: quote.acceptor_contact,
                charge_summary,
                proposal_summary: source.proposal_summary.clone(),
                agreement_type: source.agreement_type.clone(),
                catalog: source.catalog.clone(),
                // The renewal's quote was accepted at the instant the clock
                // created it, and the organization's approval policy has nothing
                // to say about a renewal no caller asked for.
                status: REQUEST_STATUS_ACCEPTED.to_string(),
                approval_requirement: APPROVAL_NOT_REQUIRED.to_string(),
                purchase_orders: Vec::new(),
                // No caller sent this quote, so it carries no client token, and
                // the renewal below is the agreement it was accepted as.
                client_token: None,
                accepted_agreement_id: Some(id.clone()),
                accepted_at: Some(end),
                created_at: end,
                updated_at: end,
            },
        );
        self.agreements.insert(
            id.clone(),
            Agreement {
                id: id.clone(),
                initial_agreement_id: source.initial_agreement_id.clone(),
                agreement_request_id: request_id,
                // A renewal is a new agreement over its own quote: the
                // amendments applied to the agreement it continues were applied
                // to THAT record, and its terms are already in the quote above.
                amendment_request_ids: Vec::new(),
                agreement_type: source.agreement_type.clone(),
                catalog: source.catalog.clone(),
                acceptor: source.acceptor.clone(),
                proposer: source.proposer.clone(),
                region: source.region.clone(),
                renewal_count: source.renewal_count.saturating_add(1),
                status: STATUS_ACTIVE.to_string(),
                start_time: end,
                end_time,
                acceptance_time: end,
                proposal_summary: source.proposal_summary.clone(),
                accepted_terms: terms,
                purchase_orders: Vec::new(),
                // The renewal is for the same acceptor, whose contact the source
                // agreement has on file.
                acceptor_contact: source.acceptor_contact.clone(),
                entitlements,
                updated_at: end,
            },
        );
        // The renewal's quote bills its prepaid charges on acceptance exactly as
        // an acceptor's own would, dated at the instant the renewal was created.
        let _ = self.issue_acceptance_invoices(&id, end);
        if let Some(renewed) = self.agreements.get_mut(agreement_id) {
            let _ = renewed.renew(end);
        }
        Some(id)
    }

    /// Materialise every automatic payment-request approval the clock has
    /// already made due by `now`.
    ///
    /// A payment request sent under a term whose acceptor configured approval on
    /// expiration is owed its `APPROVED` status at the instant the send recorded
    /// ([`payment_request_auto_approval`]) whether or not anybody calls — "the
    /// duration after which a payment request is automatically approved if no
    /// further action is taken" — so an operation that reports a payment
    /// request, reports the charges of an agreement, or guards on a request's
    /// status materialises it before it answers, exactly as the sibling
    /// [`Self::settle_billing_adjustments`] does for a submitted adjustment.
    ///
    /// The transition is the one the buyer's own accept performs
    /// ([`Self::approve_payment_request`]), so the charge the approval creates
    /// is minted by that single writer; no purchase order reference is recorded,
    /// because no call supplied one. `updated_at` becomes the instant the
    /// approval was DUE, never the instant it was observed, so repeated reads
    /// report the same value and the charge it created carries that same date.
    pub fn settle_payment_requests(&mut self, now: SystemTime) {
        let due: Vec<(String, SystemTime)> = self
            .payment_requests
            .values()
            .filter_map(|request| {
                request
                    .auto_approval_due(now)
                    .map(|at| (request.id.clone(), at))
            })
            .collect();
        for (id, at) in due {
            let _ = self.approve_payment_request(&id, None, at);
        }
    }

    /// Materialise every billing adjustment status change the clock has already
    /// made due by `now`.
    ///
    /// A request is submitted `PENDING` and AWS Marketplace processes it; the
    /// processing is owed at `completes_at` whether or not anybody calls, so a
    /// reporting operation materialises it before it reports. `updated_at`
    /// becomes the instant the change was DUE, never the instant it was
    /// observed, so repeated reads report the same value.
    pub fn settle_billing_adjustments(&mut self, now: SystemTime) {
        for request in self.billing_adjustment_requests.values_mut() {
            if request.status == BILLING_ADJUSTMENT_PENDING && now >= request.completes_at {
                request.status = BILLING_ADJUSTMENT_COMPLETED.to_string();
                request.updated_at = request.completes_at;
            }
        }
    }

    /// The payment requests of `agreement` whose approval created a charge, in
    /// the order they were sent.
    ///
    /// A payment request carries a charge only once it has been approved
    /// ([`PaymentRequest::created_charge_id`]), and that charge is a charge of
    /// the agreement the request was sent under: `chargeId` is "the unique
    /// identifier of the charge created after the payment request is approved"
    /// and `Charge$id` is the "unique identifier of the charge for a given
    /// agreement". The order is the store's own over payment request ids, which
    /// are minted from one counter, so it is the order the requests were sent
    /// in.
    fn approved_payment_requests_of<'a>(
        &'a self,
        agreement: &'a Agreement,
    ) -> impl Iterator<Item = &'a PaymentRequest> + 'a {
        self.payment_requests
            .values()
            .filter(|request| request.agreement_id == agreement.id)
            .filter(|request| request.created_charge_id().is_some())
    }

    /// The quotes of `agreement`, in the order they were accepted: the one it
    /// was accepted from, followed by the quote of each amendment applied to it
    /// ([`Agreement::amend`]).
    ///
    /// An amendment "modifies an existing agreement with terms that are
    /// accepted in the request", so its quote prices THIS agreement: the
    /// charges of the agreement are the charges of all of its quotes, and the
    /// LAST one is the quote the agreement stands at. A quote this store no
    /// longer holds is simply not among them.
    pub fn quotes_of<'a>(
        &'a self,
        agreement: &'a Agreement,
    ) -> impl Iterator<Item = &'a AgreementRequest> + 'a {
        ::std::iter::once(&agreement.agreement_request_id)
            .chain(agreement.amendment_request_ids.iter())
            .filter_map(|id| self.agreement_requests.get(id))
    }

    /// The charges of `agreement`, in the order its quotes minted them followed
    /// by the order its payment requests were approved in, as
    /// `ListAgreementCharges` reports them.
    ///
    /// Every member is read from the one copy of the resource it belongs to:
    /// the charge's identity and timing from the quote's own `ExpectedCharge`,
    /// the agreement's id and type from the agreement, and the purchase order
    /// reference from the association recorded on that agreement's charge —
    /// absent while the charge carries none, which is what "The purchase order
    /// reference associated with the charge, if any" states.
    ///
    /// Every quote of the agreement is reported, which for an amended agreement
    /// is the one it was accepted from and each amendment's own
    /// ([`Self::quotes_of`]): an amendment modifies the agreement rather than
    /// creating one, so the charges it added are charges of the same agreement
    /// and the charges it was already reporting stay reported.
    ///
    /// The approval of a payment request creates a charge of the same agreement
    /// ([`Self::approved_payment_requests_of`]), so it is reported here too,
    /// read from the one payment request record that holds it.
    pub fn charges_of(&self, agreement: &Agreement) -> Vec<Charge> {
        let mut charges: Vec<Charge> = Vec::new();
        for request in self.quotes_of(agreement) {
            // When the quote that minted these charges was accepted, which for
            // the quote the agreement was accepted from is its own acceptance.
            let accepted_at = request.accepted_at.unwrap_or(agreement.acceptance_time);
            charges.extend(
                request
                    .charge_summary
                    .expected_charges
                    .iter()
                    .flatten()
                    .filter_map(|expected| {
                        let id = expected.id.as_deref()?;
                        Some(Charge {
                            id: Some(id.to_string()),
                            revision: Some(INITIAL_CHARGE_REVISION),
                            agreement_id: Some(agreement.id.clone()),
                            agreement_type: Some(agreement.agreement_type.clone()),
                            purchase_order_reference: agreement.purchase_order_reference_of(id),
                            // "The currency code for the charge amount" and "the
                            // amount of the charge": both are read from the
                            // quote's own charge, which priced it once (see the
                            // module docs), so a charge is never re-priced into a
                            // second amount.
                            currency_code: request.charge_summary.currency_code.clone(),
                            amount: expected.amount.clone(),
                            // "The date and time when the charge will be
                            // incurred. This is available only when the charge
                            // date is known." A charge timed `ON_ACCEPTANCE` is
                            // "charged immediately when the agreement request is
                            // accepted", so the acceptance of the request that
                            // minted it is its date. One timed on a billing
                            // period or on the proposal's payment schedule is
                            // dated by a billing cycle or a schedule this service
                            // does not model, so its date stays unknown.
                            time: expected.time.or_else(|| {
                                (expected.timing.as_deref() == Some(TIMING_ON_ACCEPTANCE))
                                    .then(|| ::aws_smithy_types::DateTime::from(accepted_at))
                            }),
                        })
                    }),
            );
        }
        charges.extend(
            self.approved_payment_requests_of(agreement)
                .filter_map(|request| {
                    let id = request.created_charge_id()?;
                    Some(Charge {
                        id: Some(id.to_string()),
                        revision: Some(INITIAL_CHARGE_REVISION),
                        agreement_id: Some(agreement.id.clone()),
                        agreement_type: Some(agreement.agreement_type.clone()),
                        purchase_order_reference: agreement.purchase_order_reference_of(id),
                        // "The amount of the charge" and "the currency code for
                        // the charge amount": the amount the seller requested and
                        // the currency of the term it was requested under, read
                        // from the payment request the approval moved rather than
                        // restated here.
                        currency_code: request.currency_code.clone(),
                        amount: Some(request.charge_amount.clone()),
                        // The approval is what created this charge — "once
                        // accepted… the charge will be processed" — so the instant
                        // it moved the request is the date the charge is incurred,
                        // and `updated_at` holds it: APPROVED is the request's
                        // last transition.
                        time: Some(::aws_smithy_types::DateTime::from(request.updated_at)),
                    })
                }),
        );
        charges
    }

    /// The agreement's estimated cost, as `DescribeAgreement` reports it.
    ///
    /// Read from the quote the agreement STANDS AT ([`Self::quotes_of`]), which
    /// priced the terms it holds once ([`Self::charge_summary`]): the one it was
    /// accepted from, or the last amendment applied to it.
    /// `ChargeSummary$newAgreementValue` is "the total value of the agreement,
    /// which includes any amendments", the same total
    /// `EstimatedCharges$agreementValue` reports ("the total known amount
    /// customer has to pay across the lifecycle of the agreement", which "in
    /// case a customer has amended an agreement, by purchasing more units of any
    /// dimension … will include both the original cost as well as the added cost
    /// incurred due to addition of new units"). Reading it rather than pricing
    /// the accepted terms a second time is what keeps the quote, the agreement
    /// and the agreement's charges reporting one set of amounts.
    pub fn estimated_charges(&self, agreement: &Agreement) -> EstimatedCharges {
        let summary = self
            .quotes_of(agreement)
            .last()
            .map(|request| &request.charge_summary);
        EstimatedCharges {
            currency_code: summary.and_then(|summary| summary.currency_code.clone()),
            agreement_value: summary.and_then(|summary| summary.new_agreement_value.clone()),
        }
    }

    /// Mint a charge id for an expected charge of a quote. `ExpectedCharge$id`
    /// is a `ResourceId`; AWS Marketplace charge ids carry the `ch-` prefix the
    /// payment-request operations model (`^ch-[a-zA-Z0-9]+$`).
    fn mint_charge_id(&mut self) -> String {
        format!("ch-{:016x}", self.next_sequence())
    }

    /// The charges `requested_terms` imply for a purchase made in `region`, as
    /// `CreateAgreementRequest` returns them.
    ///
    /// Every member is computed from the request, the store and the published
    /// rates: one expected charge per requested term that is charged at all
    /// (with the timing its kind implies, and the amount the published rates
    /// price it at in `region`), one itemized charge per dimension KEY the
    /// acceptor configured (with the total quantity it asked for, the quantity
    /// the source agreement already holds for an amendment, and the incremental
    /// amount between them), and an agreement value totalling what the requested
    /// terms commit. See the module docs for where the prices come from and what
    /// is left unpriced.
    pub fn charge_summary(
        &mut self,
        requested_terms: &[RequestedTerm],
        intent: &str,
        source_agreement_id: Option<&str>,
        region: &str,
    ) -> ChargeSummary {
        // "This value is 0 for NEW intent"; an amendment or replacement reads
        // the quantities the source agreement already holds.
        let source = source_agreement_id
            .filter(|_| intent != INTENT_NEW)
            .and_then(|id| self.agreements.get(id));
        // The existing quantity of each dimension, which the quote subtracts
        // from what the request asks for ONCE: every term priced below takes
        // off what it measured against, so two terms naming one dimension key
        // are together charged for the units the request adds above the source.
        let mut remaining_quantities = source
            .map(|source| dimension_quantities(&source.accepted_terms))
            .unwrap_or_default();
        // The kind of each requested term, resolved the ONE way the acceptance
        // of this quote will resolve it ([`term_kind`]) — including from the
        // agreement an amendment carries a term over from — so the charge a
        // term is quoted for and the term the agreement reports are about the
        // same kind of term. Resolved before anything mints a charge id, since
        // that borrows the store mutably.
        let kinds: Vec<&'static str> = requested_terms
            .iter()
            .map(|requested| term_kind(requested, source))
            .collect();

        let mut expected_charges: Vec<ExpectedCharge> = Vec::new();
        let mut itemized_charges: Vec<ItemizedCharge> = Vec::new();
        // The agreement value: the total of what the requested terms commit,
        // whether any term commits an amount at all, and whether one of them
        // commits an amount the published rates do not price — in which case the
        // total is NOT known and the member is left absent rather than reported
        // short.
        let mut committed_total: u128 = 0;
        let mut commits = false;
        let mut unpriced = false;
        for (requested, kind) in requested_terms.iter().zip(kinds) {
            let Some(timing) = charge_timing(kind) else {
                continue;
            };
            let price = term_price(
                kind,
                requested_upfront_configuration(requested),
                &mut remaining_quantities,
                region,
            );
            if commits_amount(kind) {
                commits = true;
                match price.committed {
                    Some(committed) => {
                        committed_total = committed_total.saturating_add(committed);
                    }
                    None => unpriced = true,
                }
            }
            let charge_id = self.mint_charge_id();
            expected_charges.push(ExpectedCharge {
                id: Some(charge_id.clone()),
                // "The date and time when the charge is due to be invoiced.
                // This is available only when the charge date is known" — a
                // quote has not been accepted yet, so no charge date is.
                time: None,
                amount: price.charged.map(amount),
                // The tax members stay absent whatever `taxConfiguration` asked
                // for: see the module docs.
                amount_after_tax: None,
                timing: Some(timing.to_string()),
                estimated_taxes: None,
                // "The purchase order reference associated with the charge, if
                // any": this call MINTS the charge, so nothing has associated a
                // reference with it yet. A reference collected later is reported
                // from the request that holds it ([`AgreementRequest::quote`]).
                purchase_order_reference: None,
            });
            for dimension in price.dimensions {
                itemized_charges.push(ItemizedCharge {
                    dimension_key: Some(dimension.key),
                    new_quantity: Some(dimension.new_quantity),
                    old_quantity: Some(dimension.old_quantity),
                    charge_reference: Some(charge_id.clone()),
                    incremental_charge_amount: dimension.incremental.map(amount),
                });
            }
        }

        ChargeSummary {
            currency_code: Some(CURRENCY_CODE.to_string()),
            new_agreement_value: if !commits {
                Some(ZERO_AGREEMENT_VALUE.to_string())
            } else if unpriced {
                None
            } else {
                Some(amount(committed_total))
            },
            new_agreement_value_after_tax: None,
            expected_charges: (!expected_charges.is_empty()).then_some(expected_charges),
            estimated_taxes: None,
            itemized_charges: (!itemized_charges.is_empty()).then_some(itemized_charges),
            // The entity that issues the acceptor's invoice follows the
            // acceptor's address, which this service does not model, so the
            // member stays absent — one of the tax and invoicing members
            // `ChargeSummary` returns "on a best-effort basis".
            invoicing_entity: None,
        }
    }
}
