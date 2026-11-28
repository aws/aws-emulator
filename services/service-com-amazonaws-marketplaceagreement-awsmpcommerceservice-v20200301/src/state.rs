// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-marketplaceagreement-awsmpcommerceservice-v20200301 service.
//!
//! Three resources carry the agreement lifecycle this crate serves:
//!
//! * an **agreement request** — "a quote for the terms you want to accept"
//!   (`CreateAgreementRequest`), which captures the requested terms and the
//!   charges they imply;
//! * an **agreement** — what `AcceptAgreementRequest` finalises from one of
//!   those requests, or, where the request amends an existing agreement
//!   ("`AMEND` – Modifies an existing agreement with terms that are accepted in
//!   the request"), the agreement that accept re-states ([`Agreement::amend`]),
//!   and what the clock creates again at the end date of one
//!   whose accepted terms renew it there
//!   ([`ComAmazonawsMarketplaceagreementAwsmpcommerceserviceV20200301State::settle_agreement`]),
//!   and what `DescribeAgreement`, `GetAgreementTerms`,
//!   `SearchAgreements` and `CancelAgreement` report on and mutate. Accepting it
//!   also provisions the **entitlements** it grants, which belong to the
//!   agreement rather than standing on their own and which
//!   `GetAgreementEntitlements` reports;
//! * a **payment request** — what `SendAgreementPaymentRequest` submits under an
//!   agreement's `VariablePaymentTerm`, and what
//!   `ListAgreementPaymentRequests` reports;
//! * a **billing adjustment request** — what
//!   `BatchCreateBillingAdjustmentRequest` submits against one invoice of an
//!   agreement, and what `GetBillingAdjustmentRequest` and
//!   `ListBillingAdjustmentRequests` report on;
//! * an **invoice** — what the acceptance of an agreement issues for each
//!   charge it bills ([`Invoice`]), what `ListAgreementInvoiceLineItems`
//!   reports grouped by invoice, and what an adjustment entry is resolved
//!   against.
//!
//! Every projection the model asks for (`DescribeAgreementOutput`,
//! `AgreementViewSummary`, `ChargeSummary`, `AcceptedTerm`,
//! `BillingAdjustmentSummary`) is computed from these records, so the
//! implemented operations share exactly one copy of each resource.
//!
//! ## Where the money comes from
//!
//! An agreement request names terms that live in an *agreement proposal* (an
//! offer), which is a resource of a different AWS Marketplace API: its
//! EXISTENCE is resolved there, against the catalog that publishes it
//! ([`agreement_proposals`]), while its term document and rate card are not in
//! this service's model or state — so there is no per-proposal pricing to read
//! here. What this emulator holds instead is an offer of its
//! OWN, as DATA: [`RATE_CARD`] publishes the per-unit price of each product
//! dimension it carries, at each contract length, [`TERM_PRICE_LIST`] the single
//! price of each kind of term that carries one instead of a rate card,
//! [`OFFER_RESOURCES`] the products the offer is for (the resources an agreement
//! accepted from it is involved with, and the ones its entitlements are
//! provisioned to), and
//! [`OFFER_DOCUMENT`] what each kind of term proposes beyond those prices (its
//! documents, policy text, durations, grants and ceilings). A quote prices the
//! dimensions and terms those lists publish — quantity times the published rate
//! — and reports NO amount for anything they do not, which
//! `ExpectedCharge$amount` ("Only available when the charge amount is known")
//! states, and the accepted terms an acceptance stores are the terms of that
//! same document ([`accepted_term`]). Nothing here derives a price: a rate card
//! is data.
//!
//! Each amount is computed ONCE, by the quote
//! ([`ComAmazonawsMarketplaceagreementAwsmpcommerceserviceV20200301State::charge_summary`]),
//! so the agreement that accepting the quote mints and the charges that
//! agreement holds read that one copy rather than pricing again.
//!
//! Two charged kinds are never priced at quote time whatever the lists carry: a
//! usage-based charge is "based on product usage", and a variable payment is an
//! amount no seller has requested yet. The agreement value of terms that commit
//! nothing at all is `EstimatedCharges$agreementValue`'s documented zero
//! ([`ZERO_AGREEMENT_VALUE`]), and it is absent when a term commits an amount
//! these lists do not price. `ChargeSummary`'s tax members and its
//! `invoicingEntity` stay absent: AWS estimates tax from the acceptor's tax
//! registration and address and invoices through the entity that address
//! belongs to, none of which this service models, and the model returns those
//! members "on a best-effort basis".

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime};

use sha2::{Digest, Sha256};

use crate::ops::types::{
    AcceptedTerm, Acceptor, AcceptorContact, AgreementApprovalPurchaseOrder,
    AgreementCancellationRequestSummary,
    AgreementEntitlement, AgreementRequestSummary, AgreementRequestTerm,
    AgreementInvoiceLineItemGroupSummary, AgreementViewSummary,
    BatchCreateBillingAdjustmentRequestEntry, BillingAdjustmentSummary,
    ByolPricingTerm, Charge, ChargeSummary, ConfigurableUpfrontPricingTerm,
    ConfigurableUpfrontPricingTermConfiguration, ConfigurableUpfrontRateCardItem,
    ConflictException, Constraints, DescribeAgreementOutput, Dimension, DocumentItem,
    EndTimeBehavior, Entitlement, EstimatedCharges, ExpectedCharge, Filter,
    FixedUpfrontPricingTerm, FreeTrialPricingTerm, FreeTrialSummary,
    GetAgreementPaymentRequestOutput,
    GetBillingAdjustmentRequestOutput, GrantItem, InternalServerException, InvoiceBillingPeriod,
    ItemizedCharge, LegalTerm,
    NetPaymentTerm, PaymentRequestSummary, PaymentScheduleTerm, PricingCurrencyAmount,
    ProposalSummary, Proposer, RateCardItem, ScheduleItem, Selector, UsageBasedRateCardItem,
    CreateAgreementRequestInput, PurchaseOrder, RecurringPaymentTerm, RenewalTerm, RequestedTerm,
    RequestedTermConfiguration, Resource, ResourceNotFoundException,
    SendAgreementCancellationRequestInput, SendAgreementPaymentRequestInput, SupportTerm,
    TaxConfiguration, ThrottlingException, UsageBasedPricingTerm, ValidationException,
    ValidationExceptionField, ValidityTerm, VariablePaymentTerm, AGREEMENT_STATUS_VALUES,
    END_TIME_BEHAVIOR_REASON_CODE_VALUES, END_TIME_BEHAVIOR_TYPE_VALUES,
};
mod failures;
mod filters;
mod purchase_orders;
pub use failures::*;
pub use filters::*;
pub use purchase_orders::*;

/// The store's own behaviour — minting, resolving, settling and refusing — kept
/// beside the shapes it holds rather than in this module root (`state/store.rs`).
mod store;

/// Publishing the entitlements an accepted agreement grants into the services
/// that OWN them — the Marketplace Entitlement Service and License Manager.
pub mod granted_entitlements;

/// Resolving the agreement proposal a quote names against the service that OWNS
/// offers — the Marketplace Discovery catalog.
pub mod agreement_proposals;

/// The approvals a buyer's organization requires before an agreement request can
/// be accepted: the published approval policy and the approvals started from it.
pub mod agreement_approvals;
pub use agreement_approvals::{
    AgreementApproval, ApprovalLevel, ApproverIdentity, ApproverOutcomeRecord, DecisionRefusal,
};

/// The `agreementType` every agreement this slice mints carries.
///
/// `DescribeAgreementOutput$agreementType` documents two values,
/// "PurchaseAgreement or VendorInsightsAgreement", and `ChargeSummary` — the
/// output `CreateAgreementRequest` computes — says it "is applicable only when a
/// request is created for a `PurchaseAgreement`". The agreement-request flow
/// this crate serves is therefore the purchase one.
pub const AGREEMENT_TYPE: &str = "PurchaseAgreement";

/// The catalog an agreement is created in, and the one
/// `SearchAgreementsInput$catalog` selects.
///
/// AWS Marketplace exposes a single catalog, `AWSMarketplace`; the agreement
/// operations carry no catalog member, so every agreement is created in it.
pub const CATALOG: &str = "AWSMarketplace";

/// `Intent`'s three values (`INTENT_VALUES` in `types`), named where the
/// handlers branch on them.
pub const INTENT_NEW: &str = "NEW";
/// See [`INTENT_NEW`].
pub const INTENT_AMEND: &str = "AMEND";
/// See [`INTENT_NEW`].
pub const INTENT_REPLACE: &str = "REPLACE";

/// The `AgreementRequestStatus` a created agreement request carries: "the
/// agreement request has been created but not yet submitted for approval".
pub const REQUEST_STATUS_DRAFT: &str = "DRAFT";
/// The `AgreementRequestStatus` starting its approvals moves a request to: "the
/// agreement request is waiting for one or more approvers to decide".
pub const REQUEST_STATUS_PENDING_APPROVAL: &str = "PENDING_APPROVAL";
/// The `AgreementRequestStatus` a request reaches once its approvals are in:
/// "all required approvals have been granted, but the agreement request wasn't
/// accepted automatically. Call `AcceptAgreementRequest` to accept it."
pub const REQUEST_STATUS_APPROVED: &str = "APPROVED";
/// The `AgreementRequestStatus` accepting a request moves it to: "the agreement
/// request has been accepted and the resulting agreement has been created".
pub const REQUEST_STATUS_ACCEPTED: &str = "ACCEPTED";
/// The `AgreementRequestStatus` a rejected approval moves its request to: "at
/// least one approver rejected the agreement request. It won't proceed."
pub const REQUEST_STATUS_REJECTED: &str = "REJECTED";
/// The `AgreementRequestStatus` `CancelAgreementRequest` moves a request to:
/// "the agreement request was cancelled before completion".
pub const REQUEST_STATUS_CANCELLED: &str = "CANCELLED";

/// `ApprovalRequirement`'s two values: whether "the agreement request requires
/// approval before it can be accepted".
pub const APPROVAL_REQUIRED: &str = "REQUIRED";
/// See [`APPROVAL_REQUIRED`].
pub const APPROVAL_NOT_REQUIRED: &str = "NOT_REQUIRED";

/// `ContactType`'s only value: the kind of contact an agreement has on file when
/// its quote carried the acceptor's contact information.
pub const CONTACT_TYPE_ACCEPTOR: &str = "ACCEPTOR";

/// The `AgreementStatus` an accepted agreement carries, and the one
/// `CancelAgreement` moves it out of.
pub const STATUS_ACTIVE: &str = "ACTIVE";
/// The `AgreementStatus` `CancelAgreement` moves an agreement to.
pub const STATUS_CANCELLED: &str = "CANCELLED";
/// The `AgreementStatus` the agreement a `REPLACE` request acts upon carries
/// from then on: "REPLACED – The agreement was replaced using an agreement
/// replacement offer."
pub const STATUS_REPLACED: &str = "REPLACED";
/// The `AgreementStatus` an agreement carries once its own end date has passed
/// and its terms do not renew it at that date: "EXPIRED – The agreement ended on
/// the defined end date" — reached by the clock rather than by any call, and
/// materialised by
/// [`ComAmazonawsMarketplaceagreementAwsmpcommerceserviceV20200301State::settle_agreement`].
pub const STATUS_EXPIRED: &str = "EXPIRED";
/// The `AgreementStatus` an agreement carries once its own end date has passed
/// and its terms renewed it at that date: "RENEWED – The agreement was renewed
/// into a new agreement (for example, an auto-renewal)" — the other end the
/// clock owes at an end date, materialised by the same
/// [`ComAmazonawsMarketplaceagreementAwsmpcommerceserviceV20200301State::settle_agreement`]
/// that creates the renewal agreement.
pub const STATUS_RENEWED: &str = "RENEWED";

/// How many renewals of one chain a single call materialises.
///
/// A guard on the walk of a chain rather than a rule of the service: what
/// bounds the renewals of an agreement is its own renewal term
/// (`RenewalTerm$maxRenewals`), and a term that publishes no limit — "the field
/// is null when the number of renewals is unlimited" — would otherwise renew
/// once per contract length for however long ago the chain's end date passed.
const RENEWALS_PER_CALL: usize = 64;

// ---------------------------------------------------------------------------
// Entitlements
// ---------------------------------------------------------------------------

/// The `AgreementEntitlementStatus` values an entitlement of this emulator
/// reports (`AGREEMENT_ENTITLEMENT_STATUS_VALUES` in `types`), each the state
/// the agreement that provisioned it and the clock put it in — see
/// [`Agreement::entitlement_state`].
///
/// `FAILED` is the one modelled value none reports: every condition the model
/// gives a reason code for a failure (`INVALID_PAYMENT_INSTRUMENT`,
/// `ACCOUNT_SUSPENDED`, `INCOMPATIBLE_CURRENCY`, `PRODUCT_RESTRICTED`,
/// `UNSUPPORTED_OPERATION`) belongs to a payment instrument, an account state or
/// a product catalog this service neither models nor stores, so the emulator
/// never reports a provisioning failure for work nothing ran.
pub const ENTITLEMENT_PENDING: &str = "PENDING";
/// See [`ENTITLEMENT_PENDING`].
pub const ENTITLEMENT_SCHEDULED: &str = "SCHEDULED";
/// See [`ENTITLEMENT_PENDING`].
pub const ENTITLEMENT_PROVISIONED: &str = "PROVISIONED";
/// See [`ENTITLEMENT_PENDING`].
pub const ENTITLEMENT_DEPROVISIONED: &str = "DEPROVISIONED";

/// The `AgreementEntitlementStatusReasonCode` reported beside each of those
/// statuses — "provides more information about the status of an entitlement", so
/// it names WHY the entitlement is in that state
/// (`AGREEMENT_ENTITLEMENT_STATUS_REASON_CODE_VALUES` in `types`).
pub const ENTITLEMENT_REASON_PROVISIONING: &str = "PROVISIONING_IN_PROGRESS";
/// See [`ENTITLEMENT_REASON_PROVISIONING`].
pub const ENTITLEMENT_REASON_FUTURE_START_DATE: &str = "FUTURE_START_DATE";
/// See [`ENTITLEMENT_REASON_PROVISIONING`].
pub const ENTITLEMENT_REASON_AGREEMENT_ACTIVE: &str = "AGREEMENT_ACTIVE";
/// See [`ENTITLEMENT_REASON_PROVISIONING`].
pub const ENTITLEMENT_REASON_AGREEMENT_INACTIVE: &str = "AGREEMENT_INACTIVE";

/// How long an entitlement an acceptance provisioned stays `PENDING` before the
/// clock owes it `PROVISIONED`.
///
/// AWS publishes no provisioning time for an agreement's entitlements, so this
/// span is the emulator's OWN — the same kind of span as
/// [`BILLING_ADJUSTMENT_SETTLEMENT`]: non-zero, so a caller reading straight
/// after the accept still sees the `PROVISIONING_IN_PROGRESS` every entitlement
/// starts in, and short enough that a test can wait it out.
pub const ENTITLEMENT_PROVISIONING: Duration = Duration::from_millis(500);

/// The service segment of the ARN an entitlement's licence carries:
/// `AgreementEntitlement$licenseArn` is "the Amazon Resource Name (ARN) of the
/// AWS License Manager license associated with the entitlement".
pub const LICENSE_ARN_SERVICE: &str = "license-manager";

/// The `EntitlementType` every entitlement this service provisions reports.
///
/// `AgreementEntitlement$type` is "the type of entitlement", and
/// `EntitlementType` is a plain string (`@length(1, 64)`, `^[A-Za-z:]+$`) for
/// which the model declares no value list. What an entitlement of this service
/// IS, though, is stated by the member beside it: `licenseArn` is "the Amazon
/// Resource Name (ARN) of the AWS License Manager license associated with the
/// entitlement", so every entitlement an agreement here grants is a licence of
/// that kind and that is the kind each reports — one published value, like
/// [`AGREEMENT_TYPE`], rather than a per-request choice.
pub const ENTITLEMENT_TYPE: &str = "License";

/// How long the registration token an entitlement was provisioned with stays
/// valid.
///
/// `AgreementEntitlement$registrationToken` is "a short-lived token required by
/// acceptors to register their account with the product provider. The token is
/// only valid for 30 minutes after creation": the duration is the model's own,
/// not a span this emulator chose, so it keeps that value.
pub const REGISTRATION_TOKEN_VALIDITY: Duration = Duration::from_secs(30 * 60);

/// Mint the registration token one entitlement is provisioned with.
///
/// The token is a capability an acceptor presents to the product provider, so
/// none of it is derived from the request, from the agreement or from anything
/// else a caller can see. It is the SHA-256 digest (`sha2`) of three
/// independent inputs, hex encoded (`hex`): a process-wide counter, so two
/// tokens minted in the same instant differ; the wall clock in nanoseconds, so
/// two processes differ; and the hash keys of a fresh `RandomState`, which the
/// standard library seeds from the operating system, so the bytes are not
/// derivable from the clock. Its 64 characters satisfy `RegistrationToken`'s
/// `@length(1, 512)` and `^[A-Za-z0-9+/=.:_-]+$`.
///
/// The same mixing the `iotsecuretunneling` slice mints its own opaque tokens
/// with (`minted_bytes` in that crate's `state.rs`): adequate for an emulator's
/// opaque values, and drawn from the crates this service already carries.
fn mint_registration_token() -> String {
    use std::hash::{BuildHasher, Hasher};

    static MINTED: AtomicU64 = AtomicU64::new(0);

    let sequence = MINTED.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|since| since.as_nanos())
        .unwrap_or_default();
    let mut keyed = ::std::collections::hash_map::RandomState::new().build_hasher();
    keyed.write_u64(sequence);
    keyed.write_u128(nanos);

    let mut digest = Sha256::new();
    digest.update(sequence.to_be_bytes());
    digest.update(nanos.to_be_bytes());
    digest.update(keyed.finish().to_be_bytes());
    ::hex::encode(digest.finalize())
}

/// The region an ARN this service mints is in when the caller signed for none,
/// or for text that is no Region at all — `::engine::emulator_arn`'s own
/// documented default ("emulators default to `us-east-1`").
pub const DEFAULT_REGION: &str = "us-east-1";

/// The region to mint a licence ARN in: the one the caller signed for when that
/// is a well-formed AWS Region LABEL, else [`DEFAULT_REGION`].
///
/// Read from the SigV4 credential scope, the same signal the router routes on,
/// so a licence is minted in the region the client addressed. The scope's region
/// field is arbitrary text — the parser that produces it only rejects an empty
/// field — while this value is INTERPOLATED into a minted ARN that is then
/// persisted and reported under `AgreementEntitlement$licenseArn`, whose
/// `AwsArn` pattern bounds that segment. So the field is read through the shared
/// Region grammar (`service_common::arn::region_label`, non-empty, bounded,
/// `[a-z0-9-]` only), which every model's ARN pattern admits in that position:
/// text outside it names no Region any AWS endpoint has, and minting from it
/// verbatim would store a value the member's own pattern cannot hold and that
/// `SearchAgreements`' `LicenseArn` filter could never carry.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .filter(|scope| ::service_common::arn::region_label(&scope.region).is_some())
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// `ResourceType` values this service reports on a `ResourceNotFoundException`
/// or a `ConflictException` (`RESOURCE_TYPE_VALUES` in `types`).
pub const RESOURCE_TYPE_AGREEMENT: &str = "Agreement";
/// See [`RESOURCE_TYPE_AGREEMENT`].
pub const RESOURCE_TYPE_AGREEMENT_REQUEST: &str = "AgreementRequest";
/// See [`RESOURCE_TYPE_AGREEMENT`].
pub const RESOURCE_TYPE_BILLING_ADJUSTMENT_REQUEST: &str = "BillingAdjustmentRequest";
/// See [`RESOURCE_TYPE_AGREEMENT`].
pub const RESOURCE_TYPE_PAYMENT_REQUEST: &str = "PaymentRequest";

/// The largest page the paginated operations serve, and the page they serve
/// when the request names no `maxResults`: `MaxResults` is modelled
/// `@range(min: 1, max: 50)`.
pub const MAX_RESULTS: i32 = 50;

/// How many agreement requests the store holds before `CreateAgreementRequest`
/// answers its modelled `ServiceQuotaExceededException`.
///
/// AWS Marketplace publishes no public number for this, so the bound is the
/// emulator's OWN bound on the store rather than a claim about an account's
/// quota: it is what stops state growing without end, and it is far above any
/// workflow an emulator serves.
pub const AGREEMENT_REQUEST_QUOTA: usize = 1_000;

/// How many billing adjustment requests the store holds before
/// `BatchCreateBillingAdjustmentRequest` stops accepting entries.
///
/// The same kind of bound as [`AGREEMENT_REQUEST_QUOTA`], and for the same
/// reason: AWS publishes no number, so this is the emulator's OWN bound on the
/// store — what stops the records growing without end across calls, however
/// many calls are made — rather than a claim about an account's quota. It is far
/// above any workflow an emulator serves. [`BILLING_ADJUSTMENT_BATCH_LIMIT`]
/// bounds ONE call; this bounds the store. Past it an entry is refused on
/// `BatchCreateBillingAdjustmentRequest`'s own per-entry `errors` channel, so a
/// call whose other entries still fit is answered rather than refused.
pub const BILLING_ADJUSTMENT_REQUEST_QUOTA: usize = 1_000;

/// The `quotaCode` reported with that refusal. Named for what it bounds: a real
/// quota code is an identifier Service Quotas issues, which this emulator does
/// not model and so must not invent one of.
pub const AGREEMENT_REQUEST_QUOTA_CODE: &str = "AgreementRequestsPerAccount";

/// How many agreements the store holds before `AcceptAgreementRequest` refuses
/// to finalise another.
///
/// The same kind of bound as [`AGREEMENT_REQUEST_QUOTA`] and for the same
/// reason: AWS publishes no number, so this is the emulator's OWN bound on the
/// store rather than a claim about an account's quota, and it is far above any
/// workflow an emulator serves. `AcceptAgreementRequest` models a
/// `ServiceQuotaExceededException`, so reaching the bound is answered with it —
/// the acceptance mints nothing and the quote stays acceptable.
pub const AGREEMENT_QUOTA: usize = 1_000;

/// The `quotaCode` that refusal reports; see [`AGREEMENT_REQUEST_QUOTA_CODE`]
/// for why it is named for what it bounds.
pub const AGREEMENT_QUOTA_CODE: &str = "AgreementsPerAccount";

/// How many payment requests the store holds before
/// `SendAgreementPaymentRequest` refuses to store another.
///
/// Like [`AGREEMENT_REQUEST_QUOTA`], this is the emulator's OWN bound on the
/// store rather than a claim about an account's quota (AWS Marketplace
/// publishes no number for it): it is what stops a caller growing state without
/// end, and it is far above any workflow an emulator serves. It also bounds the
/// idempotency lookup, which scans the stored requests for the `clientToken`
/// that minted one rather than holding a second, separately growing map.
///
/// `SendAgreementPaymentRequest` models no `ServiceQuotaExceededException`, so
/// reaching the bound is answered with the modelled `ThrottlingException`
/// ("Request was denied due to request throttling") — a retryable refusal that
/// stores nothing.
pub const PAYMENT_REQUEST_QUOTA: usize = 1_000;

/// How many agreement cancellation requests the store holds before
/// `SendAgreementCancellationRequest` refuses another.
///
/// Like [`AGREEMENT_REQUEST_QUOTA`] this is the emulator's OWN ceiling on the
/// store rather than a claim about an account's quota: it is what stops state
/// growing without end under repeated sends, and it is far above any workflow an
/// emulator serves. `SendAgreementCancellationRequest` models no
/// `ServiceQuotaExceededException`, so a send past the ceiling is refused with
/// its modelled `ThrottlingException` ([`store_full`]) — the service declining
/// to take more work — and nothing is stored.
///
/// This also bounds the [`ComAmazonawsMarketplaceagreementAwsmpcommerceserviceV20200301State::idempotency`]
/// map, whose entries are only ever written beside a record a create stored: the
/// map holds at most one key per stored resource, so every create's own ceiling
/// bounds it too.
pub const AGREEMENT_CANCELLATION_REQUEST_QUOTA: usize = 1_000;

/// The sigv4 signing name of this service, reported as the `serviceCode` of a
/// quota refusal.
pub const SERVICE_CODE: &str = "aws-marketplace";

/// `EstimatedCharges$agreementValue` / `ChargeSummary$newAgreementValue` for an
/// agreement whose accepted terms commit the acceptor to nothing.
///
/// `agreementValue` states this case for metered pricing: "This is `0` if the
/// accepted terms contain `UsageBasedPricingTerm` without
/// `ConfigurableUpfrontPricingTerm` or `RecurringPaymentTerm`" — i.e. zero when
/// no accepted term commits an amount. This emulator applies it to every such
/// terms set, which includes one carrying no pricing term at all (a legal term
/// only): there too the total known amount the acceptor has to pay is zero.
pub const ZERO_AGREEMENT_VALUE: &str = "0";

// ---------------------------------------------------------------------------
// Prices
// ---------------------------------------------------------------------------

/// The currency every price this service quotes is in, as `CurrencyCode` spells
/// one (`@length(3, 3)`, `^[A-Z]+$`): "The three-letter currency code for all
/// charges (e.g., USD)".
///
/// One code for every charge of an agreement is what that member states ("for
/// ALL charges"), and the listing catalog this service serves ([`CATALOG`])
/// prices in US dollars.
pub const CURRENCY_CODE: &str = "USD";

/// How many minor units one unit of [`CURRENCY_CODE`] divides into, which is
/// the precision every amount this service reports carries.
const MINOR_UNITS: u128 = 100;

/// How many fraction digits a currency amount of this service is reported to —
/// the digits of its [`MINOR_UNITS`].
const MINOR_DIGITS: usize = 2;

/// How many fraction digits an amount of this API may carry at most:
/// `PositiveAmountUpto8Decimals` is named for its eight
/// (`^[0-9]*(\.[0-9]{0,8})?$`).
const AMOUNT_FRACTION_DIGITS: u32 = 8;

/// How many of the smallest unit [`amount_units`] counts make one unit of
/// [`CURRENCY_CODE`]: ten to the power of [`AMOUNT_FRACTION_DIGITS`].
const AMOUNT_UNITS_PER_CURRENCY_UNIT: u128 = 100_000_000;

/// One published per-unit rate of the rate card this emulator carries: what one
/// unit of a product dimension costs, per contract length.
///
/// `RateCardItem` is "the per unit rates for each individual product dimension":
/// its `dimensionKey` is the "dimension for which the given entitlement
/// applies… Dimensions represent categories of capacity in a product and are
/// specified when the product is listed in AWS Marketplace", and its `price` is
/// the "per unit price for the product dimension that's used for calculating the
/// amount to be charged". Which rate card of a term applies is the acceptor's
/// choice: `ConfigurableUpfrontRateCardItem$selector` "differentiates between
/// the mutually exclusive rate cards in the same pricing term to be selected by
/// the buyer", and the configuration's `selectorValue` "defines the length of
/// time for which the particular pricing/dimension is being purchased" — so a
/// rate is keyed by the dimension and the contract length together.
#[derive(Debug, Clone, Copy)]
pub struct PublishedRate {
    /// The product dimension this row prices.
    pub dimension_key: &'static str,
    /// `(selectorValue, the price of one unit in minor units of
    /// [`CURRENCY_CODE`])`.
    pub per_selector: &'static [(&'static str, u64)],
}

/// The rate card this emulator quotes configured prepaid dimensions from.
///
/// SOURCE: this is the emulator's OWN catalog. A rate card belongs to the
/// agreement proposal — the seller's offer — which is a resource of a different
/// AWS Marketplace API and is in neither this service's model nor its state, and
/// AWS publishes no list price for another seller's listing, so there is no
/// price list to carry here and nothing to compute one from either. The rows
/// below are therefore DATA this emulator ships, the way a price list is data:
/// the dimension names are the ones AWS Marketplace contract pricing documents
/// as examples of product dimensions (users, admins, hosts, nodes, data
/// storage), and the selectors are the ISO-8601 contract lengths contract
/// pricing is bought in (1, 12, 24 and 36 months), each priced per unit for the
/// whole length.
///
/// It is deliberately a SLICE: a dimension these rows do not publish, or a
/// contract length they do not publish for that dimension, is quoted with NO
/// amount at all — `ExpectedCharge$amount` is "only available when the charge
/// amount is known" — never with a made-up number.
pub const RATE_CARD: &[PublishedRate] = &[
    PublishedRate {
        dimension_key: "Users",
        per_selector: &[("P1M", 2_000), ("P12M", 21_600), ("P24M", 40_800), ("P36M", 57_600)],
    },
    PublishedRate {
        dimension_key: "Admins",
        per_selector: &[("P1M", 5_000), ("P12M", 54_000), ("P24M", 102_000), ("P36M", 144_000)],
    },
    PublishedRate {
        dimension_key: "Hosts",
        per_selector: &[("P1M", 1_200), ("P12M", 12_960), ("P24M", 24_480), ("P36M", 34_560)],
    },
    PublishedRate {
        dimension_key: "Nodes",
        per_selector: &[("P1M", 3_500), ("P12M", 37_800), ("P24M", 71_400), ("P36M", 100_800)],
    },
    PublishedRate {
        dimension_key: "Storage",
        per_selector: &[("P1M", 10), ("P12M", 108), ("P24M", 204), ("P36M", 288)],
    },
    PublishedRate {
        dimension_key: "DataTransfer",
        per_selector: &[("P1M", 5), ("P12M", 54), ("P24M", 102), ("P36M", 144)],
    },
];

/// The price of a term that carries ONE price instead of a rate card, in minor
/// units of [`CURRENCY_CODE`], keyed by the kind of term:
/// `FixedUpfrontPricingTerm$price` is the "fixed amount to be charged to the
/// customer when this term is accepted", `RecurringPaymentTerm$price` the
/// "amount charged to the buyer every billing period", and a
/// `PaymentScheduleTerm`'s is the total of the installments its "schedule"
/// lists ("the sum of all invoice charges in the payment schedule",
/// `EstimatedCharges$agreementValue`).
///
/// Data on the same footing as [`RATE_CARD`], and keyed by the kind rather than
/// by a term id because the price of such a term is stated by the proposal's
/// term document and a term id is the acceptor's own naming of it. A kind this
/// list does not publish is quoted with no amount.
pub const TERM_PRICE_LIST: &[(&str, u64)] = &[
    ("FixedUpfrontPricingTerm", 120_000),
    ("RecurringPaymentTerm", 10_000),
    ("PaymentScheduleTerm", 360_000),
];

/// The published per-unit rate of `dimension_key` at `selector`, or `None` when
/// [`RATE_CARD`] publishes none — the case no amount is reported for.
fn published_unit_rate(dimension_key: &str, selector: &str) -> Option<u64> {
    RATE_CARD
        .iter()
        .find(|rate| rate.dimension_key == dimension_key)?
        .per_selector
        .iter()
        .find(|(published, _)| *published == selector)
        .map(|(_, price)| *price)
}

/// The published price of a term of `kind` that carries one price rather than a
/// rate card, or `None` when [`TERM_PRICE_LIST`] publishes none.
fn published_term_price(kind: &str) -> Option<u64> {
    TERM_PRICE_LIST
        .iter()
        .find(|(published, _)| *published == kind)
        .map(|(_, price)| *price)
}

/// `minor` minor units, as every money member of this service spells an amount:
/// the units, a decimal point, and the two minor digits.
pub fn amount(minor: u128) -> String {
    format!(
        "{units}.{minor_digits:02}",
        units = minor / MINOR_UNITS,
        minor_digits = minor % MINOR_UNITS
    )
}

/// The quantity a dimension value buys, in the arithmetic amounts are computed
/// in. `ZeroValueInteger` is `@range(min: 0)`, so a negative value is one the
/// boundary already refused and buys nothing.
fn quantity(value: i32) -> u128 {
    u128::try_from(value).unwrap_or(0)
}

/// What one priced dimension of a configured prepaid term costs.
#[derive(Debug)]
struct DimensionPrice {
    /// "The dimension key as specified in the accepted term."
    key: String,
    /// "The requested quantity for this dimension" — the total of every
    /// occurrence of the key in the configuration ([`requested_quantities`]).
    new_quantity: i32,
    /// "The existing quantity for this dimension from the source agreement",
    /// as far as it is not already accounted for by an earlier term of the same
    /// request ([`term_price`]).
    old_quantity: i32,
    /// `ItemizedCharge$incrementalChargeAmount`, "the total incremental charge
    /// amount for this dimension": the units this request ADDS at the
    /// dimension's published rate, and `None` for a dimension [`RATE_CARD`]
    /// publishes no rate for.
    incremental: Option<u128>,
}

/// What one charged term of a quote costs.
#[derive(Debug, Default)]
struct TermPrice {
    /// What this request is charged for the term — `ExpectedCharge$amount`,
    /// "the tax-exclusive amount of the charge" — which for an amendment covers
    /// only the units the amendment adds. `None` for a charge the published
    /// rates do not price.
    charged: Option<u128>,
    /// What the term commits over the agreement's lifecycle at the quantities
    /// this request asks for, which is what `ChargeSummary$newAgreementValue`
    /// totals. `None` for a term the published rates do not price.
    committed: Option<u128>,
    /// One entry per dimension the acceptor configured, in the order the
    /// configuration lists them.
    dimensions: Vec<DimensionPrice>,
}

/// What the term of kind `kind` costs, given the configuration the acceptor
/// supplied and the quantities the agreement being amended already holds.
///
/// The kinds priced at all are the ones [`commits_amount`] names: a configured
/// prepaid term at the published per-unit rate of every dimension it buys
/// ([`published_unit_rate`]), and a fixed upfront, recurring or payment-schedule
/// term at the one published price of its kind ([`published_term_price`]). Every
/// other kind is unpriced at quote time whatever is published — a usage-based
/// charge follows "product usage" and a variable payment is an amount no seller
/// has requested yet — and so is anything the published rates do not carry.
///
/// A dimension key names ONE dimension of the agreement, so a configured prepaid
/// term is priced per KEY and not per occurrence of it
/// ([`requested_quantities`]), and the existing quantity a key is measured
/// against is subtracted from the total asked for exactly once: what this term
/// measures against is taken OFF `old_quantities`, so a key a later term of the
/// same request names again is not measured against the same existing units a
/// second time.
fn term_price(
    kind: &str,
    configuration: Option<&ConfigurableUpfrontPricingTermConfiguration>,
    old_quantities: &mut HashMap<String, i32>,
) -> TermPrice {
    if !commits_amount(kind) {
        return TermPrice::default();
    }
    match kind {
        "ConfigurableUpfrontPricingTerm" => {
            // The dimensions and the contract length are the acceptor's own
            // choice, so a term carrying no configuration buys nothing there is
            // a rate for.
            let Some(configuration) = configuration else {
                return TermPrice::default();
            };
            // One line item per dimension KEY the acceptor configured, at the
            // total quantity its occurrences ask for.
            let requested = requested_quantities(&configuration.dimensions);
            let mut dimensions = Vec::with_capacity(requested.len());
            // The term's own totals, which stay unknown as soon as ONE of its
            // dimensions is one the rate card does not price: a total missing a
            // line item is not the amount of the charge.
            let mut committed: Option<u128> = Some(0);
            let mut charged: Option<u128> = Some(0);
            for (key, new_quantity) in requested {
                let rate =
                    published_unit_rate(&key, &configuration.selector_value).map(u128::from);
                let old_quantity = old_quantities.get(&key).copied().unwrap_or_default();
                let new_units = quantity(new_quantity);
                let old_units = quantity(old_quantity);
                let added = new_units.saturating_sub(old_units);
                // The existing units this term measured against are spent: what
                // is left is what a later term naming the same key is measured
                // against.
                old_quantities.insert(
                    key.clone(),
                    i32::try_from(old_units.saturating_sub(new_units)).unwrap_or_default(),
                );
                let incremental = rate.map(|rate| rate.saturating_mul(added));
                committed = committed
                    .zip(rate)
                    .map(|(total, rate)| total.saturating_add(rate.saturating_mul(new_units)));
                charged = charged
                    .zip(incremental)
                    .map(|(total, incremental)| total.saturating_add(incremental));
                dimensions.push(DimensionPrice {
                    key,
                    new_quantity,
                    old_quantity,
                    incremental,
                });
            }
            TermPrice {
                charged,
                committed,
                dimensions,
            }
        }
        _ => {
            let price = published_term_price(kind).map(u128::from);
            TermPrice {
                charged: price,
                committed: price,
                dimensions: Vec::new(),
            }
        }
    }
}

/// The configured prepaid configuration a requested term carries, if any: the
/// one union member of `RequestedTermConfiguration` a rate card prices against.
fn requested_upfront_configuration(
    requested: &RequestedTerm,
) -> Option<&ConfigurableUpfrontPricingTermConfiguration> {
    match requested.configuration.as_ref() {
        Some(RequestedTermConfiguration::configurableUpfrontPricingTermConfiguration(
            configuration,
        )) => Some(configuration),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Billing adjustment requests
// ---------------------------------------------------------------------------

/// `BillingAdjustmentStatus`'s values (`BILLING_ADJUSTMENT_STATUS_VALUES` in
/// `types`), named where the billing-adjustment handlers branch on them.
///
/// `PENDING` is what a submitted request carries while AWS Marketplace is still
/// processing it, `COMPLETED` once the adjustment has been applied, and
/// `VALIDATION_FAILED` when the submitted adjustment did not pass validation —
/// the one status `GetBillingAdjustmentRequestOutput$statusMessage` is
/// "populated only when" ([`BILLING_ADJUSTMENT_VALIDATION_FAILED`]).
pub const BILLING_ADJUSTMENT_PENDING: &str = "PENDING";
/// See [`BILLING_ADJUSTMENT_PENDING`].
pub const BILLING_ADJUSTMENT_COMPLETED: &str = "COMPLETED";
/// See [`BILLING_ADJUSTMENT_PENDING`]. No stored request reaches this status:
/// `BatchCreateBillingAdjustmentRequest` documents an `errors` list "for entries
/// that failed validation", so an entry this emulator cannot accept — including
/// one whose invoice details do not hold up against the invoice being adjusted
/// ([`Invoice`]) — is reported on that per-entry channel at submit time and no
/// record is created for it.
pub const BILLING_ADJUSTMENT_VALIDATION_FAILED: &str = "VALIDATION_FAILED";

/// How long a submitted billing adjustment request stays `PENDING` before the
/// clock owes it its terminal `COMPLETED`.
///
/// AWS publishes no processing time for a billing adjustment, so this span is
/// the emulator's OWN: it is non-zero, so a caller reading straight after the
/// submit still sees the `PENDING` the submit reported, and short enough that a
/// test can wait it out.
pub const BILLING_ADJUSTMENT_SETTLEMENT: Duration = Duration::from_millis(500);

/// How many entries one `BatchCreateBillingAdjustmentRequest` call accepts
/// before it answers its modelled `ValidationException` with reason
/// `BILLING_ADJUSTMENTS_LIMIT_EXCEEDED`.
///
/// `BatchCreateBillingAdjustmentRequestEntryList` carries no `@length`, and AWS
/// publishes no public number, so the bound is the emulator's OWN bound on one
/// batch rather than a claim about an account's quota: it is what stops one call
/// growing state without end.
pub const BILLING_ADJUSTMENT_BATCH_LIMIT: usize = 50;

/// `BillingAdjustmentErrorCode`'s values (`BILLING_ADJUSTMENT_ERROR_CODE_VALUES`
/// in `types`), as `BatchCreateBillingAdjustmentError$code` reports them for the
/// entry that failed.
pub const BILLING_ADJUSTMENT_ERROR_VALIDATION: &str = "VALIDATION_EXCEPTION";
/// See [`BILLING_ADJUSTMENT_ERROR_VALIDATION`].
pub const BILLING_ADJUSTMENT_ERROR_NOT_FOUND: &str = "RESOURCE_NOT_FOUND_EXCEPTION";
/// See [`BILLING_ADJUSTMENT_ERROR_VALIDATION`].
pub const BILLING_ADJUSTMENT_ERROR_CONFLICT: &str = "CONFLICT_EXCEPTION";

/// The message `BatchCreateBillingAdjustmentError$message` carries for an entry
/// whose `agreementId` names no agreement.
///
/// A per-entry error carries only its code, its message and the entry's
/// `clientToken`, so the message is the service's own sentence for the code —
/// the same one whatever entry hit it — and the entry it is about is the one its
/// `clientToken` names.
pub const BILLING_ADJUSTMENT_AGREEMENT_NOT_FOUND_MESSAGE: &str =
    "Provided agreement does not exist.";

/// The message `BatchCreateBillingAdjustmentError$message` carries for an entry
/// whose `originalInvoiceId` names no invoice of the agreement it adjusts.
///
/// The sibling of [`BILLING_ADJUSTMENT_AGREEMENT_NOT_FOUND_MESSAGE`], for the
/// other resource an entry names: `originalInvoiceId` is "the identifier of the
/// original invoice to adjust", so an id that is not one of that agreement's
/// invoices ([`Invoice`]) names no invoice this account was ever issued.
pub const BILLING_ADJUSTMENT_INVOICE_NOT_FOUND_MESSAGE: &str =
    "Provided invoice does not exist.";

/// The `ValidationExceptionReason` for an entry whose `currencyCode` is not the
/// currency the invoice it adjusts is denominated in
/// (`smithy_query(query="shape", shape="ValidationExceptionReason")` lists
/// `INVALID_CURRENCY_CODE`).
pub const REASON_INVALID_CURRENCY_CODE: &str = "INVALID_CURRENCY_CODE";

/// The `ValidationExceptionReason` for an entry asking to adjust more of an
/// invoice than that invoice has left to adjust (the modelled
/// `EXCEEDED_MAXIMUM_ADJUSTMENT_AMOUNT`).
pub const REASON_EXCEEDED_MAXIMUM_ADJUSTMENT_AMOUNT: &str =
    "EXCEEDED_MAXIMUM_ADJUSTMENT_AMOUNT";

/// The `ValidationExceptionReason` for an entry whose `adjustmentAmount` is not
/// an amount at all (the modelled `INVALID_ADJUSTMENT_AMOUNT`).
pub const REASON_INVALID_ADJUSTMENT_AMOUNT: &str = "INVALID_ADJUSTMENT_AMOUNT";

/// The `ValidationExceptionReason` for a submit that asks for more billing
/// adjustment requests than are allowed — one call carrying more entries than
/// [`BILLING_ADJUSTMENT_BATCH_LIMIT`], and an entry that would take the store
/// past [`BILLING_ADJUSTMENT_REQUEST_QUOTA`].
///
/// The one value `ValidationExceptionReason` publishes about a bound on billing
/// adjustments at all (`smithy_query(query="shape",
/// shape="ValidationExceptionReason")` lists
/// `BILLING_ADJUSTMENTS_LIMIT_EXCEEDED`), so it is the value a client branches
/// on for either bound.
pub const REASON_BILLING_ADJUSTMENTS_LIMIT_EXCEEDED: &str = "BILLING_ADJUSTMENTS_LIMIT_EXCEEDED";

/// How many characters of an amount a request names this service accepts, for
/// EVERY member that carries one (`adjustmentAmount`, `chargeAmount` — both
/// target `PositiveAmountUpto8Decimals`).
///
/// `PositiveAmountUpto8Decimals` carries a pattern but no `@length`, so the only
/// bound on how long a submitted amount may be is this service's own, and one is
/// needed: the amount is parsed as a number and compared, which is work the
/// caller would otherwise choose the size of. The bound leaves room for the
/// eight fraction digits the shape is named for, the decimal point, and more
/// integer digits than any amount this service can bill — so no amount of a
/// real invoice or of a real charge is near it — and makes parsing one constant
/// work.
pub const AMOUNT_MAX_LEN: usize = 32;

/// Whether `amount` is an amount this service accepts on a request at all:
/// short enough to be an amount ([`AMOUNT_MAX_LEN`]) and positive
/// ([`is_positive_amount`]).
///
/// The length is checked FIRST, so nothing reads or parses a string past the
/// bound.
pub fn is_acceptable_amount(amount: &str) -> bool {
    amount.len() <= AMOUNT_MAX_LEN && is_positive_amount(amount)
}

/// How a per-entry refusal of `BatchCreateBillingAdjustmentRequest` states the
/// rule the entry broke: the modelled `ValidationExceptionReason` a client
/// branches on, then what about the entry broke it.
///
/// `BatchCreateBillingAdjustmentError` carries only a `code`, a `message` and
/// the entry's `clientToken` — no `reason` member — so the reason is named in
/// the message, which is the only place left for it.
pub fn billing_adjustment_entry_message(reason: &str, detail: impl AsRef<str>) -> String {
    format!("{reason}: {}", detail.as_ref())
}

/// Whether `amount` is the positive amount `PositiveAmountUpto8Decimals` is
/// named for.
///
/// That shape's `^[0-9]*(\.[0-9]{0,8})?$` — which the boundary already enforced —
/// also admits amounts that are not positive (`""`, `"0"`, `"0.00"`), so
/// positivity is the one part of the constraint left for a handler: the value
/// has to carry a non-zero digit.
pub fn is_positive_amount(amount: &str) -> bool {
    amount.chars().any(|digit| digit.is_ascii_digit() && digit != '0')
}

/// `value` as a count of the smallest unit an amount of this API can carry, or
/// `None` when it is not an amount this service can hold at all.
///
/// Amounts cross this API as decimal STRINGS of at most eight fraction digits
/// (`PositiveAmountUpto8Decimals`'s `^[0-9]*(\.[0-9]{0,8})?$`, which the
/// boundary already enforced) whose length a handler bounds before reading one
/// ([`AMOUNT_MAX_LEN`]). The SCALE is therefore fixed by the
/// model: such a value is an exact whole number of 10^-8 currency units, so
/// comparing and subtracting two of them is integer arithmetic and no money is
/// lost to a binary float. `None` for a value outside that shape, and for one
/// too large to count in those units — astronomically past any amount this
/// service bills, and refused rather than wrapped.
pub fn amount_units(value: &str) -> Option<u128> {
    let (whole, fraction) = value.split_once('.').unwrap_or((value, ""));
    if fraction.len() > AMOUNT_FRACTION_DIGITS as usize {
        return None;
    }
    let digits = |part: &str| -> Option<u128> {
        if part.is_empty() {
            return Some(0);
        }
        part.bytes()
            .try_fold(0u128, |total, digit| {
                let digit = u128::from(digit.checked_sub(b'0').filter(|d| *d < 10)?);
                total.checked_mul(10)?.checked_add(digit)
            })
    };
    let scaled_fraction = digits(fraction)?.checked_mul(
        10u128.pow(AMOUNT_FRACTION_DIGITS - u32::try_from(fraction.len()).ok()?),
    )?;
    digits(whole)?
        .checked_mul(AMOUNT_UNITS_PER_CURRENCY_UNIT)?
        .checked_add(scaled_fraction)
}

/// `units` of the smallest unit [`amount_units`] counts, spelled the way every
/// money member of this service spells an amount: the units, a decimal point,
/// and the fraction digits — at least the two a currency amount is reported to
/// ([`amount`]) and no trailing zero past them, so an amount nothing has
/// adjusted reads exactly as the quote that priced it spelled it.
pub fn amount_of_units(units: u128) -> String {
    let mut fraction = format!(
        "{:0width$}",
        units % AMOUNT_UNITS_PER_CURRENCY_UNIT,
        width = AMOUNT_FRACTION_DIGITS as usize
    );
    while fraction.len() > MINOR_DIGITS && fraction.ends_with('0') {
        fraction.pop();
    }
    format!("{whole}.{fraction}", whole = units / AMOUNT_UNITS_PER_CURRENCY_UNIT)
}

/// The `ValidationExceptionReason` for a `chargeAmount` a payment request
/// cannot be sent for: "A `ValidationException` is returned if the
/// `chargeAmount` exceeds the available balance" — the member at fault is the
/// amount, and `INVALID_CHARGE_AMOUNT` is the one value
/// `ValidationExceptionReason` publishes about it
/// (`smithy_query(query="shape", shape="ValidationExceptionReason")`).
pub const REASON_INVALID_CHARGE_AMOUNT: &str = "INVALID_CHARGE_AMOUNT";

/// The ceiling `term` publishes, in the units [`amount_units`] counts, or `None`
/// for a term that publishes no ceiling an amount can be measured against.
///
/// `VariablePaymentTerm$maxTotalChargeAmount` is "the maximum total amount that
/// can be charged to the customer through variable payment requests under this
/// term". It targets `BoundedString` rather than an amount shape, so a term may
/// carry a value that is not an amount this service can count — and a ceiling
/// that cannot be counted bounds nothing, rather than refusing every charge.
pub fn term_ceiling_units(term: &VariablePaymentTerm) -> Option<u128> {
    let published = term.max_total_charge_amount.as_deref()?;
    if published.len() > AMOUNT_MAX_LEN {
        return None;
    }
    amount_units(published)
}

// ---------------------------------------------------------------------------
// Invoices
// ---------------------------------------------------------------------------

/// `InvoiceType`'s values (`INVOICE_TYPE_VALUES` in `types`): "Valid values are
/// `INVOICE` and `CREDIT_MEMO`."
///
/// Every invoice this service issues is an `INVOICE` — it bills a charge of an
/// agreement. A `CREDIT_MEMO` is the document a credit is issued as, which AWS
/// raises in the billing system that applies an approved adjustment; no
/// operation of this service creates or reports one, so this store holds none.
pub const INVOICE_TYPE_INVOICE: &str = "INVOICE";

/// `InvoiceBillingPeriod`'s modelled bounds: "The billing period month. Valid
/// range: 1-12" and `@range(min: 2023, max: 2050)` on its `year`
/// (`smithy_query(query="shape", shape="InvoiceBillingPeriod")`). A period
/// outside them is not a value the member admits, so it is left unreported
/// rather than reported out of range.
const BILLING_PERIOD_MONTHS: ::std::ops::RangeInclusive<i32> = 1..=12;
/// See [`BILLING_PERIOD_MONTHS`].
const BILLING_PERIOD_YEARS: ::std::ops::RangeInclusive<i32> = 2023..=2050;

/// One invoice of an agreement, as this emulator persists it.
///
/// An invoice BILLS one charge of an agreement, and the charges this service can
/// invoice are the ones whose invoice date it knows: `ExpectedCharge$time` is
/// "the date and time when the charge is due to be invoiced… available only when
/// the charge date is known", and the charges with a known date are the ones
/// [`charge_timing`] times [`TIMING_ON_ACCEPTANCE`] — "charged immediately when
/// the agreement request is accepted" — so accepting the quote is what issues
/// them. A charge timed on a billing period or on the proposal's payment
/// schedule is due on a date this service does not model, and a charge the
/// published rates do not price carries no amount to bill, so neither is
/// invoiced at all.
///
/// `BatchCreateBillingAdjustmentRequest` adjusts one of these, and
/// `ListAgreementInvoiceLineItems` reports them grouped by invoice.
#[derive(Debug, Clone)]
pub struct Invoice {
    /// `ResourceId`'s `^[A-Za-z0-9_/-]+$`, minted by
    /// [`ComAmazonawsMarketplaceagreementAwsmpcommerceserviceV20200301State::mint_invoice_id`].
    pub id: String,
    /// The agreement whose charge this invoice bills; the agreement itself stays
    /// the single copy of its own type, catalog and parties.
    pub agreement_id: String,
    /// The charge this invoice bills — the line item of its group. Held for
    /// provenance (which charge of the quote this invoice came from); no member
    /// of `AgreementInvoiceLineItemGroupSummary` reports a charge, so nothing
    /// projects it.
    pub charge_id: String,
    /// What the invoice bills, read from the quote that priced that charge (see
    /// the module docs) rather than priced a second time.
    pub amount: String,
    /// The part of [`Self::amount`] still open to adjustment, which is what
    /// `PricingCurrencyAmount$maxAdjustmentAmount` reports.
    ///
    /// Held ON the invoice and decremented once, by the submit that stores an
    /// adjustment request against it
    /// ([`ComAmazonawsMarketplaceagreementAwsmpcommerceserviceV20200301State::hold_against_invoice`]),
    /// so reporting it and holding an entry to it are constant work rather than
    /// a walk over every stored adjustment request. Spelled the way every money
    /// member of this service spells an amount, and never negative: an entry
    /// asking for more than is open is refused before it is stored.
    pub open_amount: String,
    /// "The three-letter currency code for all charges": the currency the quote
    /// priced the agreement in, which is also the currency an adjustment of this
    /// invoice has to be in.
    pub currency_code: String,
    /// One of `InvoiceType`'s values — [`INVOICE_TYPE_INVOICE`] for every
    /// invoice this service issues.
    pub invoice_type: String,
    /// When the invoice was issued: the instant the charge it bills was
    /// incurred.
    pub issued_at: SystemTime,
}

impl Invoice {
    /// The most of this invoice an adjustment may still ask to credit, as a
    /// count of the smallest unit an amount can carry ([`amount_units`]) — the
    /// same amount [`Self::open_amount`] spells and
    /// `PricingCurrencyAmount$maxAdjustmentAmount` reports.
    pub fn open_units(&self) -> u128 {
        amount_units(&self.open_amount).unwrap_or_default()
    }

    /// The billing period this invoice belongs to: the calendar month its issue
    /// date falls in, as `InvoiceBillingPeriod` spells one.
    ///
    /// The month and year are read off the issue instant rendered the way the
    /// protocol renders a `Timestamp` (`YYYY-MM-DD…`), so the period is the one
    /// a client reading `issuedTime` would compute. Absent when the period falls
    /// outside the member's own bounds ([`BILLING_PERIOD_MONTHS`]).
    pub fn billing_period(&self) -> Option<InvoiceBillingPeriod> {
        let issued = ::aws_smithy_types::DateTime::from(self.issued_at)
            .fmt(::aws_smithy_types::date_time::Format::DateTime)
            .ok()?;
        let (year, rest) = issued.split_once('-')?;
        let month = rest.split('-').next()?;
        let period = InvoiceBillingPeriod {
            month: month.parse().ok()?,
            year: year.parse().ok()?,
        };
        (BILLING_PERIOD_MONTHS.contains(&period.month)
            && BILLING_PERIOD_YEARS.contains(&period.year))
        .then_some(period)
    }

    /// The `ListAgreementInvoiceLineItems` projection of this invoice: the group
    /// its line items are grouped into.
    ///
    /// `invoicingEntity` — "the entity that issues the invoice" — stays absent
    /// for the reason the module docs give for `ChargeSummary$invoicingEntity`:
    /// AWS invoices through the entity the acceptor's address belongs to, which
    /// this service does not model.
    pub fn to_group_summary(&self) -> AgreementInvoiceLineItemGroupSummary {
        AgreementInvoiceLineItemGroupSummary {
            agreement_id: Some(self.agreement_id.clone()),
            invoice_id: Some(self.id.clone()),
            pricing_currency_amount: Some(PricingCurrencyAmount {
                amount: Some(self.amount.clone()),
                max_adjustment_amount: Some(self.open_amount.clone()),
                currency_code: Some(self.currency_code.clone()),
            }),
            invoice_billing_period: self.billing_period(),
            issued_time: Some(::aws_smithy_types::DateTime::from(self.issued_at)),
            invoice_type: Some(self.invoice_type.clone()),
            invoicing_entity: None,
        }
    }
}

/// The scope a `ListAgreementInvoiceLineItems` page token belongs to: the whole
/// query, so a token cannot resume a different listing.
///
/// Every part is length-prefixed by [`query_scope`], and the minting operation
/// is a part too ([`listing_scope`]), so no value a caller supplies can imitate
/// the delimiters and make two different listings hash alike.
///
/// `issued` is the ONE window the request's two issued-time bounds name, as the
/// handler reads them: after, then before. Each is rendered by
/// [`scope_instant`], the one rendering every windowed scope here shares.
pub fn invoice_line_item_scope(
    operation: &str,
    agreement_id: &str,
    group_by: &str,
    invoice_id: Option<&str>,
    invoice_type: Option<&str>,
    billing_period: Option<&InvoiceBillingPeriod>,
    issued: (Option<f64>, Option<f64>),
) -> String {
    let period = billing_period
        .map(|period| format!("{}/{}", period.month, period.year))
        .unwrap_or_default();
    let (issued_after, issued_before) = issued;
    listing_scope(
        operation,
        &[
            agreement_id,
            group_by,
            invoice_id.unwrap_or_default(),
            invoice_type.unwrap_or_default(),
            &period,
            &scope_instant(issued_after),
            &scope_instant(issued_before),
        ],
    )
}

/// `Timing`'s values (`TIMING_VALUES` in `types`), with the meaning
/// `ExpectedCharge$timing` documents: `ON_ACCEPTANCE` is "charged immediately
/// when the agreement request is accepted", `BILLING_PERIOD` is "charged on each
/// billing period" and `SCHEDULED` is "charged at a predetermined future date".
pub const TIMING_ON_ACCEPTANCE: &str = "ON_ACCEPTANCE";
/// See [`TIMING_ON_ACCEPTANCE`].
pub const TIMING_BILLING_PERIOD: &str = "BILLING_PERIOD";
/// See [`TIMING_ON_ACCEPTANCE`].
pub const TIMING_SCHEDULED: &str = "SCHEDULED";

/// The revision every charge this store holds reports as `Charge$revision`.
///
/// `ChargeRevision` is modelled `@range(min: 1)`
/// (`smithy_query(query="shape", shape="ChargeRevision")`), so a charge's first
/// revision is 1. A charge is minted once — by the quote whose requested terms
/// imply it ([`ComAmazonawsMarketplaceagreementAwsmpcommerceserviceV20200301State::charge_summary`])
/// — and no operation of this service revises one: what changes afterwards is
/// the purchase-order association recorded ON the charge, which is a member of
/// the charge's report rather than a new revision of the charge. So every charge
/// this store holds is at its first revision.
pub const INITIAL_CHARGE_REVISION: i64 = 1;

/// Every term kind `AcceptedTerm` models, spelled as the term's own `type`
/// member spells it (`GetAgreementTerms` documents "`TermType` – The type of
/// term, such as `LegalTerm`, `RenewalTerm`, or `ConfigurableUpfrontPricingTerm`").
///
/// This is also the id each term of [`OFFER_DOCUMENT`] is published under, and
/// so the catalog [`resolved_term_kind`] resolves a requested term id against.
pub const TERM_KINDS: &[&str] = &[
    "LegalTerm",
    "SupportTerm",
    "RenewalTerm",
    "UsageBasedPricingTerm",
    "ConfigurableUpfrontPricingTerm",
    "ByolPricingTerm",
    "RecurringPaymentTerm",
    "ValidityTerm",
    "PaymentScheduleTerm",
    "FreeTrialPricingTerm",
    "FixedUpfrontPricingTerm",
    "VariablePaymentTerm",
    "NetPaymentTerm",
];

/// The term kind a requested term none of the resolvable signals names a kind
/// for is accepted as.
///
/// A `RequestedTerm` names a term of an agreement proposal, which this service
/// neither models nor stores, so the signals a kind can be read from are the
/// ones [`resolved_term_kind`] lists: the request's own `configuration`, the
/// term of the same id on the agreement being amended or replaced, and an id
/// the published document carries a term under. A term none of them names is a
/// term of a proposal this emulator does not publish; it is accepted as a legal
/// term, the kind that takes no configuration, incurs no charge and sets no
/// date, and it carries NO payload from the published document — reporting the
/// document's own legal documents for it would assert a EULA no proposal
/// proposed, and leaving them absent is what tells such a term apart from the
/// legal term the document really proposes.
pub const DEFAULT_TERM_KIND: &str = "LegalTerm";

/// The attributes `Sort$sortBy` documents: "which can be `EndTime`, `StartTime`,
/// or `LastUpdateTime`".
pub const SORT_BY_VALUES: &[&str] = &["EndTime", "StartTime", "LastUpdateTime"];

/// `Sort$sortBy`'s documented default ("The default value is `EndTime`").
pub const DEFAULT_SORT_BY: &str = "EndTime";

/// The sort attributes `Sort$sortBy` restricts to one party type: "`StartTime`
/// and `LastUpdateTime` are supported only when `PartyType` is `Proposer`"
/// (repeated on `SearchAgreementsInput$sort`). `EndTime` carries no such
/// restriction, so it is not listed here.
pub const PROPOSER_ONLY_SORT_BY: &[&str] = &["StartTime", "LastUpdateTime"];

/// `Sort$sortOrder`'s documented default ("The default value is `ASCENDING`").
pub const SORT_ORDER_ASCENDING: &str = "ASCENDING";

/// The filter names `SearchAgreementsInput$filters` documents, in the order the
/// model lists them. A filter naming anything else is refused.
pub const SEARCH_FILTER_NAMES: &[&str] = &[
    "ResourceIdentifier",
    "ResourceType",
    "PartyType",
    "AcceptorAccountId",
    "OfferId",
    "Status",
    "BeforeEndTime",
    "AfterEndTime",
    "BeforeStartTime",
    "AfterStartTime",
    "BeforeLastUpdateTime",
    "AfterLastUpdateTime",
    "AgreementType",
    "OfferSetId",
    "EndTimeBehaviorType",
    "EndTimeBehaviorReasonCode",
    "InitialAgreementId",
    "LicenseArn",
];

/// The one filter name `SearchAgreementsInput$filters` requires every search to
/// carry: "A proposer can use any combination of the preceding filters along
/// with `AgreementType`, which is required" — and every combination the same
/// member lists for `PartyType` `Acceptor` begins with it too, so it is required
/// whichever party type a search is scoped to.
pub const REQUIRED_SEARCH_FILTER_NAME: &str = "AgreementType";

/// The filter name a search is scoped to a party type by.
pub const PARTY_TYPE_FILTER_NAME: &str = "PartyType";

/// The filter names that select on an agreement's end time, which the
/// combination list of `SearchAgreementsInput$filters` writes `EndTime`: "To
/// filter by `EndTime`, you can use `BeforeEndTime`, `AfterEndTime`, or both."
/// Every combination that list names is supported with and without them, so they
/// are left out of [`ACCEPTOR_FILTER_COMBINATIONS`] rather than doubling it.
pub const END_TIME_FILTER_NAMES: &[&str] = &["BeforeEndTime", "AfterEndTime"];

/// The filter-name pairs of `SearchAgreementsInput$filters` that bound ONE
/// stored instant from both sides, each entry `(lower bound, upper bound)`.
///
/// The member documents each name as "a date used to filter agreements with a
/// date before/after the `endTime`/`startTime`/`lastUpdateTime` of an
/// agreement", so the two names of a pair bound the same stored instant, and it
/// expects callers to send both at once ("To filter by `EndTime`, you can use
/// `BeforeEndTime`, `AfterEndTime`, or both"). Every such pair is one window, so
/// [`filter_time_window_refusal`] reads them all.
pub const FILTER_TIME_WINDOW_NAMES: &[(&str, &str)] = &[
    ("AfterEndTime", "BeforeEndTime"),
    ("AfterStartTime", "BeforeStartTime"),
    ("AfterLastUpdateTime", "BeforeLastUpdateTime"),
];

/// Every filter-name combination `SearchAgreementsInput$filters` lists as
/// supported "when the `PartyType` is `Acceptor`", each as the names it carries
/// BESIDE the three that say nothing about which combination it is: the required
/// [`REQUIRED_SEARCH_FILTER_NAME`], the [`PARTY_TYPE_FILTER_NAME`] that scoped
/// the search, and the optional [`END_TIME_FILTER_NAMES`]. Each entry is sorted,
/// so a request's own names compare against it once sorted and deduplicated.
///
/// An acceptor searches on at most one of the four identifying attributes
/// (`ResourceIdentifier`, `ResourceType`, `OfferId`, `OfferSetId`), and may add
/// `Status` to all but `ResourceType` — the list does not pair that one with it.
pub const ACCEPTOR_FILTER_COMBINATIONS: &[&[&str]] = &[
    &[],
    &["Status"],
    &["ResourceIdentifier"],
    &["ResourceIdentifier", "Status"],
    &["ResourceType"],
    &["OfferId"],
    &["OfferId", "Status"],
    &["OfferSetId"],
    &["OfferSetId", "Status"],
];

/// `PartyType`'s two values, as `SearchAgreementsInput$filters` documents them:
/// "The party type of the caller. Use `Proposer` or `Acceptor`."
///
/// `ListAgreementPaymentRequestsInput$partyType` names the same two with the
/// roles spelled out: "Use `Proposer` to list payment requests where you are the
/// seller, or `Acceptor` to list payment requests where you are the buyer."
pub const PARTY_TYPE_ACCEPTOR: &str = "Acceptor";
/// See [`PARTY_TYPE_ACCEPTOR`].
pub const PARTY_TYPE_PROPOSER: &str = "Proposer";

/// The `PaymentRequestStatus` a payment request is created in:
/// `SendAgreementPaymentRequest` documents "The payment request is created in
/// `PENDING_APPROVAL` status, at which point the buyer can accept or reject
/// it."
pub const PAYMENT_REQUEST_STATUS_PENDING_APPROVAL: &str = "PENDING_APPROVAL";

/// The `PaymentRequestStatus` an approved payment request carries, and the only
/// one `PaymentRequestSummary$chargeId` is reported for ("This field is only
/// present for approved payment requests").
pub const PAYMENT_REQUEST_STATUS_APPROVED: &str = "APPROVED";

/// The `PaymentRequestStatus` a cancelled payment request carries: "Once
/// cancelled, the payment request transitions to CANCELLED status and can no
/// longer be accepted or rejected by the buyer"
/// (`CancelAgreementPaymentRequest`).
///
/// Terminal: nothing moves a request out of it, so a second decision on the
/// same request is a conflict rather than a second transition.
pub const PAYMENT_REQUEST_STATUS_CANCELLED: &str = "CANCELLED";

/// The `PaymentRequestStatus` a rejected payment request carries: "Once
/// rejected, the payment request transitions to REJECTED status and cannot be
/// accepted" (`RejectAgreementPaymentRequest`).
///
/// Terminal in the same way [`PAYMENT_REQUEST_STATUS_CANCELLED`] is: nothing
/// moves a request out of it, so a second decision on the same request is a
/// conflict rather than a second transition.
pub const PAYMENT_REQUEST_STATUS_REJECTED: &str = "REJECTED";

/// The `PaymentRequestStatus` a payment request that did not pass validation
/// carries. No stored request of this service reaches it: a send is validated
/// before anything is stored, so a request that fails validation is answered
/// with the operation's modelled `ValidationException` and no record is created
/// for it. It is named here because it is the third status whose request can
/// never be charged ([`PaymentRequest::holds_term_balance`]).
pub const PAYMENT_REQUEST_STATUS_VALIDATION_FAILED: &str = "VALIDATION_FAILED";

/// The `PaymentRequestApprovalStrategy` value that leaves the approval of a
/// payment request to the CLOCK rather than to the buyer's own decision.
///
/// `VariablePaymentTermConfiguration$paymentRequestApprovalStrategy` "defines
/// the strategy for approving payment requests. Values include
/// `AUTO_APPROVE_ON_EXPIRATION` and `WAIT_FOR_APPROVAL`", and the duration the
/// expiration is measured by is that configuration's own `expirationDuration`:
/// "the duration after which a payment request is automatically approved if no
/// further action is taken. This only applies when the payment request approval
/// strategy is set to `AUTO_APPROVE_ON_EXPIRATION`."
pub const PAYMENT_REQUEST_APPROVAL_ON_EXPIRATION: &str = "AUTO_APPROVE_ON_EXPIRATION";

/// The instant the automatic approval of a payment request sent under `term` at
/// `sent_at` is owed at, or `None` when that term owes none.
///
/// The acceptor's configuration of the term is what decides: only a term
/// configured [`PAYMENT_REQUEST_APPROVAL_ON_EXPIRATION`] has its requests
/// approved on expiration, and the expiration is that configuration's
/// `expirationDuration` after the send — the acceptor's OWN value, read as the
/// span its ISO 8601 duration names (`service_common::iso8601_span`, the one
/// reading of such a member this project makes) rather than as a span this
/// service chose. Under the other strategy, and for a term carrying no
/// configuration at all, nothing but the buyer's decision moves the request.
///
/// `None` as well for a value naming no span that can be placed after the send:
/// `ISO8601Duration`'s pattern admits a signed duration, and a negative one
/// names no instant the clock will reach. Such a request waits for the buyer,
/// which is what a configuration naming no expiration does.
pub fn payment_request_auto_approval(
    term: &VariablePaymentTerm,
    sent_at: SystemTime,
) -> Option<SystemTime> {
    let configuration = term.configuration.as_ref()?;
    if configuration.payment_request_approval_strategy != PAYMENT_REQUEST_APPROVAL_ON_EXPIRATION {
        return None;
    }
    let expiration = configuration.expiration_duration.as_deref()?;
    sent_at.checked_add(::service_common::iso8601_span::iso8601_span(expiration)?)
}

/// Every value a `partyType` admits. `PartyType` is modelled a plain string
/// (`@length(1, 32)`, `^[A-Za-z]+$`), not an enum, so the boundary validator
/// cannot refuse a party type that is neither of these — the handler does, with
/// [`validation_exception`] and the modelled `INVALID_PARTY_TYPE` reason.
pub const PARTY_TYPE_VALUES: &[&str] = &[PARTY_TYPE_PROPOSER, PARTY_TYPE_ACCEPTOR];

/// The other `agreementType` the model documents beside [`AGREEMENT_TYPE`]: "The
/// type of agreement. Values are `PurchaseAgreement` or
/// `VendorInsightsAgreement`" (`smithy_query(query="doc",
/// shape="DescribeAgreementOutput", member="agreementType")`). No operation of
/// this crate mints one, so it is a value an `agreementType` selector admits and
/// selects nothing with.
pub const AGREEMENT_TYPE_VENDOR_INSIGHTS: &str = "VendorInsightsAgreement";

/// Every value an `agreementType` admits, from the two the model documents.
///
/// `AgreementType` is modelled a plain string (`@length(1, 64)`, `^[A-Za-z]+$`)
/// rather than an enum, so the boundary validator cannot refuse a word outside
/// the documented pair — the handler does, with [`selector_value_refusal`].
pub const AGREEMENT_TYPE_VALUES: &[&str] = &[AGREEMENT_TYPE, AGREEMENT_TYPE_VENDOR_INSIGHTS];

/// The `AgreementCancellationRequestStatus` a cancellation request is created
/// in: "The cancellation request is created in PENDING_APPROVAL status, at which
/// point the buyer can review it"
/// (`SendAgreementCancellationRequest`).
///
/// What moves it on is the buyer's own call
/// (`AcceptAgreementCancellationRequest`, `RejectAgreementCancellationRequest`)
/// or the seller's `CancelAgreementCancellationRequest` — never the clock, so a
/// stored request stays `PENDING_APPROVAL` until one of those runs.
pub const CANCELLATION_STATUS_PENDING_APPROVAL: &str = "PENDING_APPROVAL";

/// The `AgreementCancellationRequestStatus` an accepted cancellation request
/// carries: "Once accepted, the cancellation request transitions to APPROVED
/// status and the agreement cancellation will be processed"
/// (`AcceptAgreementCancellationRequest`).
pub const CANCELLATION_STATUS_APPROVED: &str = "APPROVED";

/// The `AgreementCancellationRequestStatus` a rejected cancellation request
/// carries: "Once rejected, the cancellation request transitions to REJECTED
/// status and the agreement remains active"
/// (`RejectAgreementCancellationRequest`).
pub const CANCELLATION_STATUS_REJECTED: &str = "REJECTED";

/// The `AgreementCancellationRequestStatus` a withdrawn cancellation request
/// carries: "Once cancelled, the cancellation request transitions to CANCELLED
/// status and can no longer be approved or rejected by the buyer"
/// (`CancelAgreementCancellationRequest`).
///
/// Terminal in the same way [`CANCELLATION_STATUS_REJECTED`] is: nothing moves a
/// request out of it, so a second decision on the same request is a conflict
/// rather than a second transition.
pub const CANCELLATION_STATUS_CANCELLED: &str = "CANCELLED";

/// `ResourceType` for a cancellation request, reported on a
/// `ResourceNotFoundException` or a `ConflictException` about one.
pub const RESOURCE_TYPE_AGREEMENT_CANCELLATION_REQUEST: &str = "AgreementCancellationRequest";

// ---------------------------------------------------------------------------
// Pagination tokens
// ---------------------------------------------------------------------------

/// Characters in a `nextToken` this service mints: a 16-hex-digit digest of the
/// query the page belongs to, then the 8-hex-digit offset the next page starts
/// at.
const PAGE_TOKEN_DIGEST_LEN: usize = 16;
/// See [`PAGE_TOKEN_DIGEST_LEN`].
const PAGE_TOKEN_OFFSET_LEN: usize = 8;
/// See [`PAGE_TOKEN_DIGEST_LEN`].
const PAGE_TOKEN_LEN: usize = PAGE_TOKEN_DIGEST_LEN + PAGE_TOKEN_OFFSET_LEN;

/// The digest of the query a page belongs to — the agreement id for
/// `GetAgreementTerms`, the whole search for `SearchAgreements`.
///
/// Hashing is what makes the token opaque rather than secret: it carries no
/// resource name, and it BINDS the token to the query it was minted for, so a
/// token cannot resume a different agreement's terms or a different search. It
/// is not a capability and guards nothing — the same caller could issue the
/// query it belongs to directly.
fn scope_digest(scope: &str) -> String {
    let digest = Sha256::digest(scope.as_bytes());
    hex::encode(digest)[..PAGE_TOKEN_DIGEST_LEN].to_string()
}

/// The `nextToken` that resumes `scope` at `offset`.
///
/// Every character is a hex digit, so the token satisfies `NextToken`'s
/// `^[a-zA-Z0-9+/=_-]+$` and its 8192-character bound.
pub fn page_token(scope: &str, offset: usize) -> String {
    format!(
        "{digest}{offset:0width$x}",
        digest = scope_digest(scope),
        width = PAGE_TOKEN_OFFSET_LEN
    )
}

/// The offset `token` resumes `scope` at, or `None` when it is not a token this
/// service minted for that scope.
///
/// Reading hexadecimal digits out of the offset half is not enough to accept a
/// token: `usize::from_str_radix` also reads spellings [`page_token`] never
/// writes — upper and mixed case digits, a leading `+` — and `NextToken`'s
/// modelled `^[a-zA-Z0-9+/=_-]+$` admits all of them, so they do reach here. A
/// token names a page only when it is, character for character, the token this
/// service mints for the scope and offset it carries; every other spelling names
/// no page, exactly like a non-hexadecimal one.
pub fn page_offset(scope: &str, token: &str) -> Option<usize> {
    if !token.is_ascii() || token.len() != PAGE_TOKEN_LEN {
        return None;
    }
    let (digest, offset) = token.split_at(PAGE_TOKEN_DIGEST_LEN);
    if digest != scope_digest(scope) {
        return None;
    }
    let offset = usize::from_str_radix(offset, 16).ok()?;
    (page_token(scope, offset) == token).then_some(offset)
}

/// The page of `items` starting at `offset`, and the token that resumes after
/// it — absent on the last page.
///
/// Shared by `GetAgreementTerms` and `SearchAgreements`: both paginate a list
/// they have already ordered, which is the only thing the token indexes into.
///
/// `None` when `offset` is past the end of the list: a token is minted only
/// while items remain, so such an offset names a position this list never had
/// and is a bad token rather than an empty page.
pub fn paginate<T: Clone>(
    items: &[T],
    scope: &str,
    offset: usize,
    page_size: usize,
) -> Option<(Vec<T>, Option<String>)> {
    if offset > 0 && offset >= items.len() {
        return None;
    }
    let end = offset.saturating_add(page_size).min(items.len());
    let page = items.get(offset..end).unwrap_or_default().to_vec();
    let next = (end < items.len()).then(|| page_token(scope, end));
    Some((page, next))
}

/// The page size a request asks for: its `maxResults`, or the largest page the
/// model admits when it names none.
pub fn page_size(max_results: Option<i32>) -> usize {
    let requested = max_results.unwrap_or(MAX_RESULTS);
    debug_assert!(
        (1..=MAX_RESULTS).contains(&requested),
        "maxResults {requested} should have been refused at the boundary"
    );
    usize::try_from(requested.clamp(1, MAX_RESULTS)).expect("a page size in 1..=50 fits in usize")
}

// ---------------------------------------------------------------------------
// The offer document
// ---------------------------------------------------------------------------

/// The term document the offer this emulator serves publishes: what each kind
/// of term proposes beyond the acceptor's own selection.
///
/// SOURCE: the emulator's OWN document, on exactly the same footing as
/// [`RATE_CARD`] and [`TERM_PRICE_LIST`] and for the same reason — a term
/// document belongs to the agreement proposal, which is a resource of a
/// different AWS Marketplace API and is in neither this service's model nor its
/// state, so there is nothing to read a seller's document from and nothing to
/// compute one from either. These fields are therefore DATA this emulator
/// ships, the way a price list is data, and the agreement an acceptance mints
/// reports the terms of THIS document.
///
/// Every amount it publishes is read from the two lists above, so a term
/// reports the price the quote charged for it rather than a second number.
#[derive(Debug, Clone, Copy)]
pub struct OfferDocument {
    /// `SupportTerm$refundPolicy`, a "free-text field about the refund policy
    /// description that will be shown to customers as is".
    pub refund_policy: &'static str,
    /// `NetPaymentTerm$paymentDuePeriod`, "the duration after an invoice is
    /// issued within which the payment is due … in the ISO 8601 format (for
    /// example, `P30D` for 30 days)".
    pub payment_due_period: &'static str,
    /// `RecurringPaymentTerm$billingPeriod`, which "defines the recurrence at
    /// which buyers are charged" — the period [`TERM_PRICE_LIST`]'s recurring
    /// price is the price of, and the one a usage-based rate is metered over.
    pub billing_period: &'static str,
    /// `FreeTrialPricingTerm$duration`, the "duration of the free trial period
    /// (5–31 days)".
    pub free_trial_duration: &'static str,
    /// `RenewalTerm$lockoutPeriod`, "the last day either party can opt in to or
    /// opt out of the renewal … in whole days (for example, `P30D`)".
    pub lockout_period: &'static str,
    /// `RenewalTerm$maxRenewals`, "the maximum number of times the agreement
    /// can be renewed" (`@range(min: 1)`).
    pub max_renewals: i32,
    /// `GrantItem$maxQuantity`, the "maximum amount of capacity that the buyer
    /// can be entitled to the given dimension of the product"
    /// (`@range(min: 1)`), for each dimension [`RATE_CARD`] publishes.
    pub grant_max_quantity: i32,
    /// `VariablePaymentTerm$maxTotalChargeAmount`, "the maximum total amount
    /// that can be charged to the customer through variable payment requests
    /// under this term", in minor units of [`CURRENCY_CODE`].
    ///
    /// It is the ceiling every send under an accepted term of this offer is
    /// measured against ([`term_ceiling_units`]), so it is published here
    /// rather than being left for a handler to invent per request.
    pub max_total_charge_amount: u64,
    /// `DocumentItem$type` of the one legal document the offer proposes. A
    /// standard contract "is managed by AWS Marketplace", so the document
    /// carries a `version` and no `url`.
    pub eula_type: &'static str,
    /// `DocumentItem$version`, "version of standard contracts" — the version of
    /// THIS document.
    pub eula_version: &'static str,
    /// How many installments a published payment schedule bills its total in:
    /// `ScheduleItem` is "an individual installment of the payment".
    pub installments: u32,
    /// The legal name of the seller that published this offer, as
    /// `AgreementApprovalProposer$legalName` reports it (`@required`, "the legal
    /// name of the proposer").
    ///
    /// The parties of an agreement carry an ACCOUNT in this model and no name
    /// anywhere else, so the one name of the seller is published with its offer.
    pub proposer_legal_name: &'static str,
    /// `OfferVisibilityScope` for this offer, one of `PUBLIC` and `PRIVATE`, as
    /// `AgreementApprovalProposalSummary$visibilityScope` reports it
    /// (`@required`).
    ///
    /// The offer this emulator publishes is signed by the proposer for the
    /// acceptor that quotes it (`CreateAgreementRequest$agreementProposalIdentifier`
    /// is "the agreement proposal signed by the proposer"), which is a private
    /// offer rather than a public listing.
    pub visibility_scope: &'static str,
}

/// See [`OfferDocument`].
pub const OFFER_DOCUMENT: OfferDocument = OfferDocument {
    refund_policy: "Refunds are available within 14 days of the agreement start date.",
    payment_due_period: "P30D",
    billing_period: "P1M",
    free_trial_duration: "P14D",
    lockout_period: "P30D",
    max_renewals: 3,
    grant_max_quantity: 10,
    max_total_charge_amount: 1_000_000_000,
    eula_type: "StandardEula",
    eula_version: "1.0",
    installments: 4,
    proposer_legal_name: "Emulated Marketplace Seller, Inc.",
    visibility_scope: "PRIVATE",
};

/// The products the offer this emulator publishes is for, each as
/// `(id, type, name)`.
///
/// SOURCE: the emulator's OWN catalog, on exactly the same footing as
/// [`RATE_CARD`] and [`OFFER_DOCUMENT`] and for the same reason — the resources
/// of an offer belong to the agreement proposal, which is a resource of a
/// different AWS Marketplace API and is in neither this service's model nor its
/// state, so there is nothing to read a seller's product list from and nothing
/// to compute one from either. The ids have the `prod-` form a marketplace
/// product id takes (`Resource$id`: "we mention the term resource, which is
/// most commonly a product, so a `resourceId` is also a `productId`"), and the
/// types are the ones `Resource$type` publishes ("the product (for example,
/// `SaaSProduct`, `AmiProduct`, `ContainerProduct`)").
///
/// This is the list an agreement names in `ProposalSummary$resources` ("the
/// list of resources involved in the agreement") when it inherits none, and the
/// list its acceptance provisions one entitlement per — so it is never empty.
/// The names are what `AgreementApprovalResource$name` reports for a product of
/// the proposal an approval is about ([`published_product_name`]).
pub const OFFER_RESOURCES: &[(&str, &str, &str)] = &[
    ("prod-emulatorsaas", "SaaSProduct", "Emulator SaaS Platform"),
    ("prod-emulatorami", "AmiProduct", "Emulator Machine Image"),
    (
        "prod-emulatorcontainer",
        "ContainerProduct",
        "Emulator Container Suite",
    ),
];

/// The name the published catalog carries for the product `id`, or `None` for a
/// product it does not publish.
pub fn published_product_name(id: &str) -> Option<&'static str> {
    OFFER_RESOURCES
        .iter()
        .find(|(published, _, _)| *published == id)
        .map(|(_, _, name)| *name)
}

/// [`OFFER_RESOURCES`] as the model reports a resource.
pub fn offer_resources() -> Vec<Resource> {
    OFFER_RESOURCES
        .iter()
        .map(|(id, r#type, _)| Resource {
            id: Some((*id).to_string()),
            r#type: Some((*r#type).to_string()),
        })
        .collect()
}

/// The resources one agreement is involved with: the ones the agreement it
/// derives from named, where it derives from an agreement that named any, and
/// otherwise the products the published offer is for ([`OFFER_RESOURCES`]).
///
/// The single place that resolution is made, so the resources an agreement
/// reports in `ProposalSummary$resources` and the resources its entitlements
/// were provisioned to are the same list rather than two. An amendment or a
/// replacement keeps the products of the chain it continues: the agreement it
/// acts upon entitles its acceptor to those, and the amendment re-states the
/// terms over them.
pub fn agreement_resources(inherited: Option<&[Resource]>) -> Vec<Resource> {
    match inherited {
        Some(resources) if !resources.is_empty() => resources.to_vec(),
        _ => offer_resources(),
    }
}

/// The proposal one agreement request — and the agreement accepting it mints —
/// is for: the proposal the request named, the resources it is involved with
/// ([`agreement_resources`]) and the offer set of the agreement it derives from.
///
/// The ONE place that summary is built, so the proposal an agreement request
/// reports and the proposal the agreement its acceptance mints reports are the
/// same thing rather than two. The only identity of the proposal this emulator
/// holds is the identifier the request named; its offer set lives in the
/// agreement proposal, which is not this service's resource, so it is carried
/// over from the agreement the request acts upon where there is one.
pub fn proposal_summary_of(
    agreement_proposal_identifier: Option<&str>,
    source: Option<&Agreement>,
) -> ProposalSummary {
    ProposalSummary {
        offer_id: agreement_proposal_identifier
            .map(str::to_string)
            .or_else(|| source.and_then(|source| source.proposal_summary.offer_id.clone())),
        resources: Some(agreement_resources(
            source.and_then(|source| source.proposal_summary.resources.as_deref()),
        )),
        offer_set_id: source.and_then(|source| source.proposal_summary.offer_set_id.clone()),
    }
}

/// The contract lengths a configured prepaid purchase is bought in, each with
/// how long it lasts in days.
///
/// The lengths are the selectors [`RATE_CARD`] publishes a rate for, in
/// ascending order — `ConfigurableUpfrontRateCardItem$selector`
/// "differentiates between the mutually exclusive rate cards in the same
/// pricing term to be selected by the buyer" — and the day counts are this
/// emulator's own reading of them (a month of 30 days, a year of 365), the same
/// kind of DATA as the rates themselves. They are what an agreement's validity
/// is measured with: `ValidityTerm$agreementDuration` "defines the duration
/// that the agreement remains active", "relative to the agreement signature
/// time" when the terms name no start date.
pub const CONTRACT_LENGTHS: &[(&str, u64)] = &[
    ("P1M", 30),
    DEFAULT_CONTRACT_LENGTH,
    ("P24M", 730),
    ("P36M", 1095),
];

/// The contract length the offer document proposes for an agreement whose
/// acceptor selected none: the one year [`CONTRACT_LENGTHS`] publishes.
pub const DEFAULT_CONTRACT_LENGTH: (&str, u64) = ("P12M", 365);

/// How many seconds a day of a contract length lasts.
const SECONDS_PER_DAY: u64 = 24 * 60 * 60;

/// The published contract length `selector` names, or `None` when
/// [`CONTRACT_LENGTHS`] publishes none — the case an agreement runs for
/// [`DEFAULT_CONTRACT_LENGTH`] instead.
fn published_contract_length(selector: &str) -> Option<(&'static str, u64)> {
    CONTRACT_LENGTHS
        .iter()
        .copied()
        .find(|(published, _)| *published == selector)
}

/// `days` of a contract length, as a duration.
fn contract_duration(days: u64) -> Duration {
    Duration::from_secs(days.saturating_mul(SECONDS_PER_DAY))
}

/// The legal documents the offer document proposes, as `LegalTerm$documents`
/// reports them: "references to legal resources proposed to the buyers".
fn published_documents() -> Vec<DocumentItem> {
    vec![DocumentItem {
        r#type: Some(OFFER_DOCUMENT.eula_type.to_string()),
        // "You don't provide a URL for this type because it's managed by AWS
        // Marketplace."
        url: None,
        version: Some(OFFER_DOCUMENT.eula_version.to_string()),
    }]
}

/// The entitlements the offer document grants with a prepaid or free-trial
/// term: one per product dimension [`RATE_CARD`] publishes, each up to the
/// capacity the document allows.
fn published_grants() -> Vec<GrantItem> {
    RATE_CARD
        .iter()
        .map(|rate| GrantItem {
            dimension_key: Some(rate.dimension_key.to_string()),
            max_quantity: OFFER_DOCUMENT.grant_max_quantity,
        })
        .collect()
}

/// The per-unit rates [`RATE_CARD`] publishes at one contract length, as
/// `RateCardItem` reports them ("the per unit rates for each individual product
/// dimension"). A dimension the card does not price at that length is not on
/// it.
fn published_rate_card(length: &str) -> Vec<RateCardItem> {
    RATE_CARD
        .iter()
        .filter_map(|rate| {
            published_unit_rate(rate.dimension_key, length).map(|price| RateCardItem {
                dimension_key: Some(rate.dimension_key.to_string()),
                price: Some(amount(u128::from(price))),
            })
        })
        .collect()
}

/// The rate cards a configured prepaid term proposes: one per contract length
/// [`CONTRACT_LENGTHS`] publishes, which are "the mutually exclusive rate cards
/// in the same pricing term to be selected by the buyer".
fn published_configurable_rate_cards() -> Vec<ConfigurableUpfrontRateCardItem> {
    CONTRACT_LENGTHS
        .iter()
        .map(|(length, _)| ConfigurableUpfrontRateCardItem {
            selector: Some(Selector {
                // The category the mutually exclusive cards differ by is the
                // contract length: `…TermConfiguration$selectorValue` "defines
                // the length of time for which the particular pricing/dimension
                // is being purchased".
                r#type: Some(SELECTOR_TYPE_DURATION.to_string()),
                value: Some((*length).to_string()),
            }),
            // Both constraints carry `Constraints`' own documented default,
            // "The default value is `Allowed`": the acceptor may select
            // several dimensions of a card and configure a quantity for each,
            // which is what this service accepts on a requested term.
            constraints: Some(Constraints {
                multiple_dimension_selection: Some(CONSTRAINT_ALLOWED.to_string()),
                quantity_configuration: Some(CONSTRAINT_ALLOWED.to_string()),
            }),
            rate_card: Some(published_rate_card(length)),
        })
        .collect()
}

/// The rate cards a usage-based term proposes: the per-unit rates of one
/// billing period, since such a term charges "based on product usage" over each
/// period ([`OfferDocument::billing_period`]).
fn published_usage_rate_cards() -> Vec<UsageBasedRateCardItem> {
    vec![UsageBasedRateCardItem {
        rate_card: Some(published_rate_card(OFFER_DOCUMENT.billing_period)),
    }]
}

/// The installments a payment-schedule term bills, as `PaymentScheduleTerm`
/// reports them: "each element defines one installment of payment".
///
/// The total is the one [`TERM_PRICE_LIST`] publishes for the kind — the same
/// number the quote charged, which `EstimatedCharges$agreementValue` calls "the
/// sum of all invoice charges in the payment schedule" — split into
/// [`OfferDocument::installments`] equal charges, the last carrying any
/// remainder so the schedule sums to exactly that total. The dates divide the
/// agreement's own contract length, starting on its start date: "invoices are
/// generated on the date provided".
fn published_schedule(context: &AcceptedTermContext<'_>) -> Vec<ScheduleItem> {
    let installments = OFFER_DOCUMENT.installments.max(1);
    let Some(total) = published_term_price("PaymentScheduleTerm").map(u128::from) else {
        return Vec::new();
    };
    let each = total / u128::from(installments);
    let interval = contract_duration(context.contract_days) / installments;
    (0..installments)
        .map(|index| ScheduleItem {
            charge_date: Some(::aws_smithy_types::DateTime::from(
                context.start + interval * index,
            )),
            charge_amount: Some(amount(if index + 1 == installments {
                total - each * u128::from(installments - 1)
            } else {
                each
            })),
        })
        .collect()
}

/// `Selector$type` — "category of selector" — of the cards a configured prepaid
/// term proposes. See [`published_configurable_rate_cards`].
const SELECTOR_TYPE_DURATION: &str = "Duration";

/// `Constraints`' documented default for both of its members: "The possible
/// values are `Allowed` and `Disallowed`. The default value is `Allowed`."
const CONSTRAINT_ALLOWED: &str = "Allowed";

// ---------------------------------------------------------------------------
// Terms
// ---------------------------------------------------------------------------

/// The kind of term `requested` names, resolved from what this service holds.
///
/// Three signals name a kind, and a more specific one wins:
///
/// 1. the acceptor's own `configuration` — each `RequestedTermConfiguration`
///    union member belongs to exactly one term kind;
/// 2. the term of the SAME id on the agreement this request amends or replaces:
///    `GetAgreementTerms` documents the `TermID` as "common between offer and
///    agreement", so one id across a chain is one term of one proposal, and the
///    kind the agreement already holds for it is the kind it keeps;
/// 3. the term id naming one of the kinds the published offer document carries
///    ([`TERM_KINDS`]).
///
/// `None` when none of them names a kind — a term of a proposal this emulator
/// does not publish, which [`DEFAULT_TERM_KIND`] covers.
pub fn resolved_term_kind(
    requested: &RequestedTerm,
    source: Option<&Agreement>,
) -> Option<&'static str> {
    configured_term_kind(requested)
        .or_else(|| {
            source
                .and_then(|source| source.accepted_term(&requested.id))
                .map(accepted_term_kind)
                .and_then(published_term_kind)
        })
        .or_else(|| published_term_kind(&requested.id))
}

/// The kind of term `requested` is accepted as: the one its request resolves to
/// ([`resolved_term_kind`]), else [`DEFAULT_TERM_KIND`].
pub fn term_kind(requested: &RequestedTerm, source: Option<&Agreement>) -> &'static str {
    resolved_term_kind(requested, source).unwrap_or(DEFAULT_TERM_KIND)
}

/// The kind the acceptor's own configuration names, when it carries one.
fn configured_term_kind(requested: &RequestedTerm) -> Option<&'static str> {
    match requested.configuration {
        Some(RequestedTermConfiguration::configurableUpfrontPricingTermConfiguration(_)) => {
            Some("ConfigurableUpfrontPricingTerm")
        }
        Some(RequestedTermConfiguration::renewalTermConfiguration(_)) => Some("RenewalTerm"),
        Some(RequestedTermConfiguration::variablePaymentTermConfiguration(_)) => {
            Some("VariablePaymentTerm")
        }
        None => None,
    }
}

/// The kind `name` names, when it is one of the kinds the published offer
/// document carries ([`TERM_KINDS`]).
fn published_term_kind(name: &str) -> Option<&'static str> {
    TERM_KINDS.iter().copied().find(|kind| *kind == name)
}

/// What the agreement being created contributes to each of its accepted terms:
/// the instants and the contract length its own record fixes, and the agreement
/// its terms are inherited from.
///
/// One context per agreement, built once from the terms its request asked for,
/// so every term of one agreement reports the same window and the same length.
#[derive(Debug, Clone, Copy)]
pub struct AcceptedTermContext<'a> {
    /// When the agreement starts, which is its signature time:
    /// `ValidityTerm$agreementStartDate` is "determined based on agreement
    /// signature time" when the terms provide no date of their own.
    pub start: SystemTime,
    /// The contract length the agreement runs for, as
    /// `ConfigurableUpfrontPricingTermConfiguration$selectorValue` spells one:
    /// the length the acceptor selected on a configured prepaid term where they
    /// selected one the rate card publishes, else the published default.
    pub contract_length: &'static str,
    /// How long [`Self::contract_length`] lasts, in days.
    pub contract_days: u64,
    /// The agreement this request amends or replaces, whose accepted terms name
    /// the kind of a term carried over by id ([`resolved_term_kind`]).
    pub source: Option<&'a Agreement>,
}

impl<'a> AcceptedTermContext<'a> {
    /// The context of an agreement starting at `start` over `requested`,
    /// derived from `source` where it amends or replaces one.
    pub fn new(
        requested: &[RequestedTerm],
        start: SystemTime,
        source: Option<&'a Agreement>,
    ) -> Self {
        let (contract_length, contract_days) = requested
            .iter()
            .filter_map(requested_upfront_configuration)
            .find_map(|configuration| published_contract_length(&configuration.selector_value))
            .unwrap_or(DEFAULT_CONTRACT_LENGTH);
        Self {
            start,
            contract_length,
            contract_days,
            source,
        }
    }

    /// When the agreement's validity ends: its start plus its contract length.
    pub fn validity_end(&self) -> SystemTime {
        self.start + contract_duration(self.contract_days)
    }
}

/// The accepted terms an agreement holds for the terms its request asked for,
/// in the order the request asked for them.
///
/// `AcceptedTerm` is "a subset of terms proposed by the proposer, which have
/// been accepted by the acceptor as part of agreement creation", so the terms
/// are exactly the requested ones: nothing adds a term no acceptor accepted.
pub fn accepted_terms(
    requested: &[RequestedTerm],
    context: &AcceptedTermContext<'_>,
) -> Vec<AcceptedTerm> {
    requested
        .iter()
        .map(|term| accepted_term(term, context))
        .collect()
}

/// The terms PROPOSED for an agreement request, in the order the request asked
/// for them.
///
/// `GetAgreementRequestTerms` "retrieves the terms proposed for an agreement
/// request, such as pricing, legal, and renewal terms", and `AgreementRequestTerm`
/// models exactly the term shapes `AcceptedTerm` does: the terms proposed for a
/// request are the terms the agreement accepting it would hold, so they are built
/// by the ONE builder that resolves a requested term into the proposal's own term
/// ([`accepted_terms`]) and re-stated as the request's own union. The window they
/// are quoted over is anchored at the instant the request was created, so every
/// read of a request reports the same dates.
pub fn proposed_terms(
    request: &AgreementRequest,
    source: Option<&Agreement>,
) -> Vec<AgreementRequestTerm> {
    let context = AcceptedTermContext::new(&request.requested_terms, request.created_at, source);
    accepted_terms(&request.requested_terms, &context)
        .into_iter()
        .map(proposed_term)
        .collect()
}

/// One accepted term as the term PROPOSED for a request: the same payload under
/// the union `AgreementRequestTerm` names it by.
fn proposed_term(term: AcceptedTerm) -> AgreementRequestTerm {
    match term {
        AcceptedTerm::legalTerm(term) => AgreementRequestTerm::legalTerm(term),
        AcceptedTerm::supportTerm(term) => AgreementRequestTerm::supportTerm(term),
        AcceptedTerm::renewalTerm(term) => AgreementRequestTerm::renewalTerm(term),
        AcceptedTerm::usageBasedPricingTerm(term) => {
            AgreementRequestTerm::usageBasedPricingTerm(term)
        }
        AcceptedTerm::configurableUpfrontPricingTerm(term) => {
            AgreementRequestTerm::configurableUpfrontPricingTerm(term)
        }
        AcceptedTerm::byolPricingTerm(term) => AgreementRequestTerm::byolPricingTerm(term),
        AcceptedTerm::recurringPaymentTerm(term) => {
            AgreementRequestTerm::recurringPaymentTerm(term)
        }
        AcceptedTerm::validityTerm(term) => AgreementRequestTerm::validityTerm(term),
        AcceptedTerm::paymentScheduleTerm(term) => {
            AgreementRequestTerm::paymentScheduleTerm(term)
        }
        AcceptedTerm::freeTrialPricingTerm(term) => {
            AgreementRequestTerm::freeTrialPricingTerm(term)
        }
        AcceptedTerm::fixedUpfrontPricingTerm(term) => {
            AgreementRequestTerm::fixedUpfrontPricingTerm(term)
        }
        AcceptedTerm::variablePaymentTerm(term) => {
            AgreementRequestTerm::variablePaymentTerm(term)
        }
        AcceptedTerm::netPaymentTerm(term) => AgreementRequestTerm::netPaymentTerm(term),
    }
}

/// The accepted term a requested term becomes.
///
/// `GetAgreementTerms` documents the three members that come from the request
/// and the agreement: the `TermType` ([`resolved_term_kind`]), the `TermID`
/// "which is common between offer and agreement", and the `Configuration` —
/// "the buyer/acceptor's selection at the time of agreement creation". Every
/// OTHER payload member belongs to the proposal's own term document, which is
/// what [`OFFER_DOCUMENT`] publishes: the term's currency and rate cards, its
/// price, duration, schedule, grants and policy text, and — for a validity term
/// — the window the agreement itself runs for (`context`). A term whose kind
/// the request does not resolve carries no payload at all; see
/// [`DEFAULT_TERM_KIND`].
pub fn accepted_term(requested: &RequestedTerm, context: &AcceptedTermContext<'_>) -> AcceptedTerm {
    let resolved = resolved_term_kind(requested, context.source);
    let kind = resolved.unwrap_or(DEFAULT_TERM_KIND);
    let r#type = Some(kind.to_string());
    let id = Some(requested.id.clone());
    let configuration = requested.configuration.clone();
    let currency_code = Some(CURRENCY_CODE.to_string());
    match kind {
        "SupportTerm" => AcceptedTerm::supportTerm(SupportTerm {
            r#type,
            id,
            refund_policy: Some(OFFER_DOCUMENT.refund_policy.to_string()),
        }),
        "RenewalTerm" => AcceptedTerm::renewalTerm(RenewalTerm {
            r#type,
            id,
            configuration: match configuration {
                Some(RequestedTermConfiguration::renewalTermConfiguration(cfg)) => Some(cfg),
                _ => None,
            },
            lockout_period: Some(OFFER_DOCUMENT.lockout_period.to_string()),
            max_renewals: Some(OFFER_DOCUMENT.max_renewals),
            // "This field applies only when `PriceIncrease` is a
            // `PercentageRange`", and the published offer raises no price at
            // renewal, so neither member carries a value: `priceIncrease` "is
            // null when the price doesn't change at renewal" and
            // `termTemplates` "is null when no terms change at renewal".
            adjustment_deadline: None,
            price_increase: None,
            term_templates: None,
        }),
        "UsageBasedPricingTerm" => AcceptedTerm::usageBasedPricingTerm(UsageBasedPricingTerm {
            r#type,
            id,
            currency_code,
            rate_cards: Some(published_usage_rate_cards()),
        }),
        "ConfigurableUpfrontPricingTerm" => {
            AcceptedTerm::configurableUpfrontPricingTerm(ConfigurableUpfrontPricingTerm {
                r#type,
                id,
                currency_code,
                // "The `RateCard` section will define the prices and dimensions
                // defined by the seller (proposer), whereas the `Configuration`
                // section will define the actual dimensions, prices, and units
                // the buyer has chosen to accept" (`Dimension$dimensionValue`).
                rate_cards: Some(published_configurable_rate_cards()),
                configuration: match configuration {
                    Some(
                        RequestedTermConfiguration::configurableUpfrontPricingTermConfiguration(
                            cfg,
                        ),
                    ) => Some(cfg),
                    _ => None,
                },
            })
        }
        "ByolPricingTerm" => AcceptedTerm::byolPricingTerm(ByolPricingTerm { r#type, id }),
        "RecurringPaymentTerm" => AcceptedTerm::recurringPaymentTerm(RecurringPaymentTerm {
            r#type,
            id,
            currency_code,
            billing_period: Some(OFFER_DOCUMENT.billing_period.to_string()),
            price: published_term_price(kind).map(|price| amount(u128::from(price))),
        }),
        "ValidityTerm" => AcceptedTerm::validityTerm(ValidityTerm {
            r#type,
            id,
            agreement_duration: Some(context.contract_length.to_string()),
            agreement_start_date: Some(::aws_smithy_types::DateTime::from(context.start)),
            agreement_end_date: Some(::aws_smithy_types::DateTime::from(context.validity_end())),
        }),
        "PaymentScheduleTerm" => AcceptedTerm::paymentScheduleTerm(PaymentScheduleTerm {
            r#type,
            id,
            currency_code,
            schedule: Some(published_schedule(context)),
        }),
        "FreeTrialPricingTerm" => AcceptedTerm::freeTrialPricingTerm(FreeTrialPricingTerm {
            r#type,
            id,
            duration: Some(OFFER_DOCUMENT.free_trial_duration.to_string()),
            grants: Some(published_grants()),
        }),
        "FixedUpfrontPricingTerm" => {
            AcceptedTerm::fixedUpfrontPricingTerm(FixedUpfrontPricingTerm {
                r#type,
                id,
                currency_code,
                duration: Some(context.contract_length.to_string()),
                price: published_term_price(kind).map(|price| amount(u128::from(price))),
                grants: Some(published_grants()),
            })
        }
        "VariablePaymentTerm" => AcceptedTerm::variablePaymentTerm(VariablePaymentTerm {
            r#type,
            id,
            currency_code,
            max_total_charge_amount: Some(amount(u128::from(
                OFFER_DOCUMENT.max_total_charge_amount,
            ))),
            configuration: match configuration {
                Some(RequestedTermConfiguration::variablePaymentTermConfiguration(cfg)) => {
                    Some(cfg)
                }
                _ => None,
            },
        }),
        "NetPaymentTerm" => AcceptedTerm::netPaymentTerm(NetPaymentTerm {
            r#type,
            id,
            payment_due_period: Some(OFFER_DOCUMENT.payment_due_period.to_string()),
        }),
        // `DEFAULT_TERM_KIND`, and the kind a `LegalTerm` id names. The
        // documents are the published offer document's own, and a term whose
        // kind nothing resolved carries NONE of them: it is the one case a term
        // reaches this arm without naming its kind, and reporting the published
        // EULA for it would assert a legal document no proposal proposed.
        _ => AcceptedTerm::legalTerm(LegalTerm {
            r#type,
            id,
            documents: resolved.is_some().then(published_documents),
        }),
    }
}

/// When the agreement that holds `terms` ends, or `None` for an agreement whose
/// terms commit it for no duration at all.
///
/// `ValidityTerm$agreementEndDate` states the rule for terms that name no end
/// date of their own: "If `AgreementEndDate` isn't provided, the agreement end
/// date is determined by the validity of individual terms." So an agreement ends
/// when the duration its accepted terms committed it for runs out — the one
/// window every term of it reports ([`AcceptedTermContext::validity_end`]) — and
/// it has no end date only when NONE of those terms commits a duration, which is
/// the case `DescribeAgreementOutput$endTime` leaves absent: "the field is null
/// for pay-as-you-go agreements, which don't have end dates".
pub fn agreement_end_time(
    terms: &[AcceptedTerm],
    context: &AcceptedTermContext<'_>,
) -> Option<SystemTime> {
    terms
        .iter()
        .any(commits_agreement_duration)
        .then(|| context.validity_end())
}

/// Whether one accepted term commits the agreement that holds it for a duration,
/// as that term's own duration member states it.
///
/// Three kinds carry one: a validity term, whose `agreementDuration` "defines
/// the duration that the agreement remains active"; a configured prepaid term,
/// whose `ConfigurableUpfrontPricingTermConfiguration$selectorValue` "defines the
/// length of time for which the particular pricing/dimension is being purchased
/// by the acceptor"; and a fixed prepaid term, whose `duration` is the "contract
/// duration for the terms".
///
/// No other kind bounds the agreement: metered usage is charged "based on product
/// usage", a recurring, variable or net payment term states a billing or payment
/// period rather than a length the agreement is bought for, a legal, support or
/// BYOL term states no window at all, a renewal term decides what happens AT an
/// end date rather than setting one, and `FreeTrialPricingTerm$duration` is the
/// length of the free trial inside the agreement, not of the agreement.
fn commits_agreement_duration(term: &AcceptedTerm) -> bool {
    match term {
        AcceptedTerm::validityTerm(validity) => validity.agreement_duration.is_some(),
        AcceptedTerm::configurableUpfrontPricingTerm(pricing) => pricing.configuration.is_some(),
        AcceptedTerm::fixedUpfrontPricingTerm(fixed) => fixed.duration.is_some(),
        _ => false,
    }
}

/// When a term of `kind` is charged, or `None` for a kind that carries no
/// charge at all (a legal, support, validity, renewal or BYOL term).
pub fn charge_timing(kind: &str) -> Option<&'static str> {
    match kind {
        // Prepaid: "charged immediately when the agreement request is accepted".
        "ConfigurableUpfrontPricingTerm" | "FixedUpfrontPricingTerm" => {
            Some(TIMING_ON_ACCEPTANCE)
        }
        // Metered and recurring pricing: "charged on each billing period".
        "UsageBasedPricingTerm" | "RecurringPaymentTerm" => Some(TIMING_BILLING_PERIOD),
        // An installment schedule: "charged at a predetermined future date".
        "PaymentScheduleTerm" | "VariablePaymentTerm" => Some(TIMING_SCHEDULED),
        _ => None,
    }
}

/// True for a term kind that commits the acceptor to an amount the proposal's
/// rate card prices.
///
/// `EstimatedCharges$agreementValue` names these: the total is the contract
/// value when the terms carry a `ConfigurableUpfrontPricingTerm` or
/// `FixedUpfrontPricingTerm`, the payment-schedule total when they carry a
/// `PaymentScheduleTerm`, and zero for usage-based pricing without a
/// `ConfigurableUpfrontPricingTerm` or `RecurringPaymentTerm`.
pub fn commits_amount(kind: &str) -> bool {
    matches!(
        kind,
        "ConfigurableUpfrontPricingTerm"
            | "FixedUpfrontPricingTerm"
            | "RecurringPaymentTerm"
            | "PaymentScheduleTerm"
    )
}

/// The quantity each dimension KEY of one configuration is asked for, summed
/// over its occurrences, in the order the configuration first names each key.
///
/// `DimensionList` carries no uniqueness rule (`smithy_query(query="shape",
/// shape="DimensionList")`), so one configuration may name a `dimensionKey` more
/// than once. A dimension key names ONE dimension of the agreement — the
/// dimension `ItemizedCharge$dimensionKey` reports "the total incremental charge
/// amount for" — so what that dimension is bought at is the TOTAL of its
/// occurrences, not one purchase per occurrence. The sum is bounded rather than
/// left to overflow `i32`: a quantity cannot be larger than the largest the
/// member can carry.
fn requested_quantities(dimensions: &[Dimension]) -> Vec<(String, i32)> {
    let mut totals: Vec<(String, i32)> = Vec::with_capacity(dimensions.len());
    // Where the total of each key already sits in `totals`, so one occurrence
    // costs one hash lookup rather than a walk of every key named before it:
    // the list the caller sends carries as many entries as it likes, so the
    // work this does grows with it and no faster.
    let mut placed: HashMap<&str, usize> = HashMap::with_capacity(dimensions.len());
    for dimension in dimensions {
        match placed.get(dimension.dimension_key.as_str()) {
            Some(&index) => {
                let total = &mut totals[index].1;
                *total = total.saturating_add(dimension.dimension_value);
            }
            None => {
                placed.insert(dimension.dimension_key.as_str(), totals.len());
                totals.push((dimension.dimension_key.clone(), dimension.dimension_value));
            }
        }
    }
    totals
}

/// The quantity each dimension of `terms` is already bought at, summed over
/// every accepted configurable-upfront term.
///
/// This is what `ItemizedCharge$oldQuantity` reports for an amendment: "The
/// existing quantity for this dimension from the source agreement."
fn dimension_quantities(terms: &[AcceptedTerm]) -> HashMap<String, i32> {
    let mut quantities: HashMap<String, i32> = HashMap::new();
    for term in terms {
        if let AcceptedTerm::configurableUpfrontPricingTerm(pricing) = term {
            if let Some(configuration) = pricing.configuration.as_ref() {
                // A key the configuration names more than once is bought at the
                // total of its occurrences, which is the one rule the quote
                // prices by too ([`requested_quantities`]).
                for (key, quantity) in requested_quantities(&configuration.dimensions) {
                    let total = quantities.entry(key).or_default();
                    *total = total.saturating_add(quantity);
                }
            }
        }
    }
    quantities
}

// ---------------------------------------------------------------------------
// Records
// ---------------------------------------------------------------------------

/// A value this model marks `@sensitive`, as a stored record holds it.
///
/// The value goes on the wire wherever the model puts it — an acceptor's contact
/// details, an approver's note, an approver's email address and name are all
/// members of some response — and NEVER into a log: this wrapper's `Debug` prints
/// the FACT that a value is held and not the value, so a record that reaches a
/// formatter cannot carry it into one. The same rule
/// [`AgreementEntitlementRecord`]'s own `Debug` follows for a registration token.
#[derive(Clone, Default)]
pub struct Sensitive<T>(T);

impl<T> Sensitive<T> {
    /// Hold `value` as the sensitive value it is.
    pub fn new(value: T) -> Self {
        Self(value)
    }

    /// The value, for the response member the model puts it in.
    pub fn get(&self) -> &T {
        &self.0
    }
}

/// Prints that a value is held, never the value itself.
impl<T> ::std::fmt::Debug for Sensitive<T> {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        f.write_str("<sensitive>")
    }
}

/// One agreement request — "a quote for the terms you want to accept" — as this
/// emulator persists it.
#[derive(Debug, Clone)]
pub struct AgreementRequest {
    /// `^ar-[A-Za-z0-9]+$`, minted by
    /// [`ComAmazonawsMarketplaceagreementAwsmpcommerceserviceV20200301State::mint_agreement_request_id`].
    pub id: String,
    /// One of `Intent`'s values.
    pub intent: String,
    /// The terms the request asked for, verbatim: the accepted terms of the
    /// agreement are derived from these.
    pub requested_terms: Vec<RequestedTerm>,
    pub source_agreement_identifier: Option<String>,
    pub agreement_proposal_identifier: Option<String>,
    pub tax_configuration: Option<TaxConfiguration>,
    /// "The contact information of the acceptor", as the create carried it —
    /// `None` for a request that named none. What `GetAgreementContacts` reports
    /// for the agreement accepting this request mints.
    pub acceptor_contact: Option<Sensitive<AcceptorContact>>,
    /// The charges the requested terms imply, computed once at create time so a
    /// replayed `clientToken` answers with the same quote.
    pub charge_summary: ChargeSummary,
    /// The proposal this request is for, built once at create time
    /// ([`proposal_summary_of`]).
    pub proposal_summary: ProposalSummary,
    /// The `agreementType` of this request, and of the agreement accepting it
    /// mints ([`AGREEMENT_TYPE`]).
    pub agreement_type: String,
    /// The catalog this request was created in ([`CATALOG`]), which
    /// `ListAgreementRequests` filters on.
    pub catalog: String,
    /// One of `AgreementRequestStatus`'s values: where this request stands in
    /// the lifecycle `GetAgreementRequestOutput$status` documents.
    pub status: String,
    /// One of `ApprovalRequirement`'s values, decided once at create time from
    /// the buyer organization's published approval policy
    /// ([`agreement_approvals::approval_requirement`]) so every later report of
    /// this request states the requirement the quote was made under.
    pub approval_requirement: String,
    /// The purchase order references collected for this request's charges while
    /// its approvals ran, each naming one charge of its quote. The acceptance
    /// associates them with the agreement's charges, beside any the accept call
    /// states itself.
    pub purchase_orders: Vec<AgreementApprovalPurchaseOrder>,
    pub client_token: Option<String>,
    /// The agreement `AcceptAgreementRequest` finalised from this request; a
    /// second accept of the same request is a conflict.
    pub accepted_agreement_id: Option<String>,
    /// When that acceptance happened, and `None` while this quote has not been
    /// accepted at all.
    ///
    /// What dates the charges this quote minted: a prepaid charge is "charged
    /// immediately when the agreement request is accepted", so the instant THIS
    /// request was accepted is the date its own charges are incurred — which an
    /// amendment's charges are reported under rather than the acceptance of the
    /// agreement it amended ([`…State::charges_of`](ComAmazonawsMarketplaceagreementAwsmpcommerceserviceV20200301State::charges_of)).
    pub accepted_at: Option<SystemTime>,
    pub created_at: SystemTime,
    /// When this request's own record last moved — a status transition, or a
    /// purchase order reference collected for one of its charges.
    pub updated_at: SystemTime,
}

impl AgreementRequest {
    /// Whether this request is the one `input` asks to create — every member the
    /// create carries beside its `clientToken` is the same.
    ///
    /// A create replaying the same token with the same content reports this
    /// request; one reusing it for different content is a conflict, as the
    /// siblings [`PaymentRequest::matches_send`] and
    /// [`BillingAdjustmentRequest::matches_entry`] treat the same case.
    pub fn matches_create(&self, input: &CreateAgreementRequestInput) -> bool {
        self.intent == input.intent
            && requested_terms_match(&self.requested_terms, &input.requested_terms)
            && self.source_agreement_identifier == input.source_agreement_identifier
            && self.agreement_proposal_identifier == input.agreement_proposal_identifier
            && tax_estimation(self.tax_configuration.as_ref())
                == tax_estimation(input.tax_configuration.as_ref())
    }

    /// The charges of this request's quote, with the purchase order reference
    /// recorded for each of them.
    ///
    /// `ExpectedCharge$purchaseOrderReference` is "the purchase order reference
    /// associated with the charge, if any": the references this request collected
    /// are the one copy of them, so every report of its quote reads them from
    /// here rather than keeping a second copy inside the stored summary.
    pub fn quote(&self) -> ChargeSummary {
        let mut summary = self.charge_summary.clone();
        for charge in summary.expected_charges.iter_mut().flatten() {
            charge.purchase_order_reference = charge
                .id
                .as_deref()
                .and_then(|id| self.purchase_order_reference_of(id));
        }
        summary
    }

    /// The purchase order reference recorded for the charge `charge_id` of this
    /// request, if one was collected.
    pub fn purchase_order_reference_of(&self, charge_id: &str) -> Option<String> {
        self.purchase_orders
            .iter()
            .find(|order| order.charge_id == charge_id)
            .and_then(|order| order.purchase_order_reference.clone())
    }

    /// Record `collected` as the purchase order references of the charges they
    /// name, at `now`.
    ///
    /// A charge carries ONE reference, so a charge that already carries one has
    /// it REPLACED rather than standing beside it — the same single-writer rule
    /// [`Agreement::associate_purchase_order`] follows for an agreement's own
    /// associations.
    pub fn collect_purchase_orders(
        &mut self,
        collected: Vec<AgreementApprovalPurchaseOrder>,
        now: SystemTime,
    ) {
        for order in collected {
            match self
                .purchase_orders
                .iter_mut()
                .find(|held| held.charge_id == order.charge_id)
            {
                Some(held) => *held = order,
                None => self.purchase_orders.push(order),
            }
        }
        self.updated_at = now;
    }

    /// This request as `ListAgreementRequests` reports it.
    pub fn to_summary(&self) -> AgreementRequestSummary {
        AgreementRequestSummary {
            agreement_request_id: Some(self.id.clone()),
            agreement_type: Some(self.agreement_type.clone()),
            status: Some(self.status.clone()),
            agreement_id: self.accepted_agreement_id.clone(),
            proposal_summary: Some(self.proposal_summary.clone()),
        }
    }

    /// Whether this request is for the resource `resource_id` — one of the
    /// resources of the proposal it is for.
    ///
    /// `ListAgreementRequestsInput$resourceId` is "an optional parameter to list
    /// agreement requests for a specific resource, such as a product", and the
    /// resources a request is for are the ones its proposal summary names.
    pub fn involves_resource(&self, resource_id: &str) -> bool {
        self.proposal_summary
            .resources
            .iter()
            .flatten()
            .any(|resource| resource.id.as_deref() == Some(resource_id))
    }
}

/// Whether two `requestedTerms` lists ask for the same thing: the same terms, in
/// the same order, each configured the same way.
///
/// Spelled out member by member because `RequestedTerm` and the configurations it
/// carries are generated from the model, which derives no equality for them: the
/// comparison a replayed `clientToken` turns on is the content of the request, so
/// it is stated here over every member that content is made of.
fn requested_terms_match(held: &[RequestedTerm], asked: &[RequestedTerm]) -> bool {
    held.len() == asked.len()
        && held.iter().zip(asked).all(|(held, asked)| {
            held.id == asked.id
                && match (held.configuration.as_ref(), asked.configuration.as_ref()) {
                    (None, None) => true,
                    (Some(held), Some(asked)) => term_configurations_match(held, asked),
                    _ => false,
                }
        })
}

/// Whether two requested-term configurations ask for the same thing: the same
/// union member, carrying the same values.
fn term_configurations_match(
    held: &RequestedTermConfiguration,
    asked: &RequestedTermConfiguration,
) -> bool {
    use RequestedTermConfiguration as Configuration;
    match (held, asked) {
        (
            Configuration::configurableUpfrontPricingTermConfiguration(held),
            Configuration::configurableUpfrontPricingTermConfiguration(asked),
        ) => {
            held.selector_value == asked.selector_value
                && held.dimensions.len() == asked.dimensions.len()
                && held.dimensions.iter().zip(&asked.dimensions).all(
                    |(held, asked)| {
                        held.dimension_key == asked.dimension_key
                            && held.dimension_value == asked.dimension_value
                    },
                )
        }
        (
            Configuration::renewalTermConfiguration(held),
            Configuration::renewalTermConfiguration(asked),
        ) => held.enable_auto_renew == asked.enable_auto_renew,
        (
            Configuration::variablePaymentTermConfiguration(held),
            Configuration::variablePaymentTermConfiguration(asked),
        ) => {
            held.payment_request_approval_strategy == asked.payment_request_approval_strategy
                && held.expiration_duration == asked.expiration_duration
        }
        _ => false,
    }
}

/// What tax estimation `configuration` asks for, as the request effectively
/// carries it.
///
/// `TaxConfiguration$taxEstimation` is modelled with the default
/// [`TAX_ESTIMATION_DEFAULT`], so a request that names no configuration at all
/// asks for exactly what one naming that default asks for: the two are the same
/// content, which is what makes a replay of one under the other's `clientToken`
/// a replay rather than a conflict.
fn tax_estimation(configuration: Option<&TaxConfiguration>) -> &str {
    configuration.map_or(TAX_ESTIMATION_DEFAULT, |configuration| {
        configuration.tax_estimation.as_str()
    })
}

/// `TaxConfiguration$taxEstimation`'s modelled default, which is also what a
/// request naming no `taxConfiguration` asks for.
pub const TAX_ESTIMATION_DEFAULT: &str = "DISABLED";

/// The `ValidationExceptionReason` for a request whose approvers have not
/// decided yet.
pub const REASON_PENDING_APPROVAL_AGREEMENT_REQUEST: &str = "PENDING_APPROVAL_AGREEMENT_REQUEST";
/// The `ValidationExceptionReason` for a request that has been closed without
/// being accepted.
pub const REASON_CLOSED_AGREEMENT_REQUEST: &str = "CLOSED_AGREEMENT_REQUEST";
/// The `ValidationExceptionReason` for an acceptance the buyer organization's
/// approval policy does not allow: of a request an approver rejected, or of one
/// the policy requires approval of that has not been approved.
pub const REASON_REJECTED_BY_APPROVAL_POLICY: &str = "REJECTED_BY_AGREEMENT_APPROVAL_POLICY";

/// Why `request` cannot be accepted in the state it is in, or `None` when it
/// can.
///
/// `AgreementRequestStatus` states the lifecycle per value: `APPROVED` means
/// "all required approvals have been granted… Call `AcceptAgreementRequest` to
/// accept it", `PENDING_APPROVAL` that the request "is waiting for one or more
/// approvers to decide", `REJECTED` that "at least one approver rejected the
/// agreement request. It won't proceed", and `CANCELLED` that it "was cancelled
/// before completion". A `DRAFT` request "has been created but not yet submitted
/// for approval", so it can be accepted exactly while the buyer organization's
/// policy does not require an approval of it — `ApprovalRequirement` "indicates
/// whether the agreement request requires approval before it can be accepted" —
/// and an acceptance the policy will not have is refused by that policy.
///
/// The ONE place that rule lives, so the acceptance door and any later reader of
/// a request agree on which quotes are acceptable.
pub fn acceptance_refusal_reason(request: &AgreementRequest) -> Option<&'static str> {
    match request.status.as_str() {
        REQUEST_STATUS_APPROVED => None,
        REQUEST_STATUS_DRAFT if request.approval_requirement == APPROVAL_NOT_REQUIRED => None,
        REQUEST_STATUS_DRAFT | REQUEST_STATUS_REJECTED => {
            Some(REASON_REJECTED_BY_APPROVAL_POLICY)
        }
        REQUEST_STATUS_PENDING_APPROVAL => Some(REASON_PENDING_APPROVAL_AGREEMENT_REQUEST),
        _ => Some(REASON_CLOSED_AGREEMENT_REQUEST),
    }
}

/// One entitlement an acceptance provisioned, as this emulator persists it.
///
/// What is STORED is the entitlement's IDENTITY — the resource it was
/// provisioned to, the licence it granted, the registration token it was minted
/// with and the instant provisioning is due to finish. Its `status` and
/// `statusReasonCode` are COMPUTED from the agreement it belongs to and the
/// clock ([`Agreement::entitlement_state`]), so cancelling or replacing the
/// agreement moves every entitlement it granted without a second copy of the
/// agreement's own state to keep in step.
#[derive(Clone)]
pub struct AgreementEntitlementRecord {
    /// "The resource that the entitlement is provisioned to, such as a product"
    /// — one of the resources the agreement is involved with
    /// ([`agreement_resources`]), which is what the acceptance provisioned one
    /// entitlement per.
    pub resource: Resource,
    /// The AWS License Manager licence this entitlement granted, minted once
    /// when the entitlement was provisioned so every later report — this
    /// agreement's entitlements, `AgreementViewSummary$entitlements` and the
    /// `LicenseArn` search filter — names the same licence.
    pub license_arn: String,
    /// "A short-lived token required by acceptors to register their account
    /// with the product provider", minted once when the entitlement was
    /// provisioned ([`mint_registration_token`]) — so two reads inside its
    /// validity report the same token and no read mints one.
    pub registration_token: String,
    /// When that token stops being valid: the instant it was minted plus
    /// [`REGISTRATION_TOKEN_VALIDITY`]. Past it the member is reported absent
    /// rather than offering an acceptor a token that registers nothing.
    pub registration_token_expires_at: SystemTime,
    /// When the `PENDING` this entitlement was provisioned in is due to become
    /// `PROVISIONED`: the acceptance instant plus [`ENTITLEMENT_PROVISIONING`].
    /// Stored rather than slept for.
    pub provisioned_at: SystemTime,
}

/// Prints every member but the registration token, which is a capability: a
/// record that reaches a formatter must not carry it into a log.
impl ::std::fmt::Debug for AgreementEntitlementRecord {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        f.debug_struct("AgreementEntitlementRecord")
            .field("resource", &self.resource)
            .field("license_arn", &self.license_arn)
            .field(
                "registration_token_expires_at",
                &self.registration_token_expires_at,
            )
            .field("provisioned_at", &self.provisioned_at)
            .finish_non_exhaustive()
    }
}

/// One agreement, as this emulator persists it.
#[derive(Debug, Clone)]
pub struct Agreement {
    /// `^[A-Za-z0-9_/-]+$`, minted by
    /// [`ComAmazonawsMarketplaceagreementAwsmpcommerceserviceV20200301State::mint_agreement_id`].
    pub id: String,
    /// "When an agreement isn't derived from another agreement, its
    /// `InitialAgreementId` is its own `AgreementId`" — otherwise the source
    /// agreement's own initial id, so a chain traces back to its first link.
    pub initial_agreement_id: String,
    pub agreement_request_id: String,
    /// The quotes of the amendments applied to this agreement, in the order
    /// they were accepted.
    ///
    /// `Intent` `AMEND` "modifies an existing agreement with terms that are
    /// accepted in the request", so an accepted amendment's own quote is a
    /// quote OF this agreement beside the one it was accepted from: its
    /// charges are charges of this agreement, and the last one priced the
    /// agreement as it now stands
    /// ([`…State::quotes_of`](ComAmazonawsMarketplaceagreementAwsmpcommerceserviceV20200301State::quotes_of)).
    pub amendment_request_ids: Vec<String>,
    pub agreement_type: String,
    pub catalog: String,
    pub acceptor: Acceptor,
    pub proposer: Proposer,
    /// The region the caller who accepted this agreement signed for
    /// ([`request_region`]), which is the region its entitlements' licences were
    /// minted in. Held on the record so a renewal the clock performs — which no
    /// caller signs — mints its own licences in the same region as the agreement
    /// it continues rather than in a region nothing asked for.
    pub region: String,
    /// How many times the chain this agreement belongs to has been renewed: zero
    /// for one an acceptance created, and one more than its source for each
    /// renewal the clock performs ([`Agreement::renew`]).
    ///
    /// This is what `RenewalTerm$maxRenewals` — "the maximum number of times the
    /// agreement can be renewed" — is measured against, so the count lives on
    /// the record rather than being re-derived per read.
    pub renewal_count: u32,
    /// One of `AgreementStatus`'s values.
    pub status: String,
    pub start_time: SystemTime,
    /// "The field is null for pay-as-you-go agreements, which don't have end
    /// dates": an agreement ends when the duration its accepted terms commit it
    /// for runs out ([`agreement_end_time`]), and one whose accepted terms
    /// commit no duration at all has no end date.
    pub end_time: Option<SystemTime>,
    pub acceptance_time: SystemTime,
    pub proposal_summary: ProposalSummary,
    pub accepted_terms: Vec<AcceptedTerm>,
    /// The purchase orders the accept call associated with this agreement's
    /// charges.
    pub purchase_orders: Vec<PurchaseOrder>,
    /// "The contact information of the acceptor", as the quote this agreement
    /// was accepted from carried it — `None` for an agreement whose quote named
    /// none. What `GetAgreementContacts` reports, and what
    /// `AgreementViewSummary$includesContactOfType` says is on file.
    pub acceptor_contact: Option<Sensitive<AcceptorContact>>,
    /// The entitlements accepting this agreement provisioned — one per resource
    /// the agreement is involved with (see [`agreement_resources`] and
    /// [`ComAmazonawsMarketplaceagreementAwsmpcommerceserviceV20200301State::provision_entitlements`]).
    pub entitlements: Vec<AgreementEntitlementRecord>,
    pub updated_at: SystemTime,
}

impl Agreement {
    /// The purchase order reference recorded for the charge `charge_id` of this
    /// agreement, if an association records one.
    ///
    /// "The purchase order reference associated with the charge, if any"
    /// (`Charge$purchaseOrderReference`): the associations this agreement holds
    /// are the one copy of them, so every report of a charge reads the reference
    /// from here.
    pub fn purchase_order_reference_of(&self, charge_id: &str) -> Option<String> {
        self.purchase_orders
            .iter()
            .find(|order| order.charge_id == charge_id)
            .and_then(|order| order.purchase_order_reference.clone())
    }

    /// Record `association` as the purchase order of the charge it names, at
    /// `now`.
    ///
    /// A charge carries ONE purchase order reference, so associating a charge
    /// that already carries one REPLACES it rather than standing beside it. The
    /// single writer of this agreement's associations, which both
    /// `UpdatePurchaseOrders` ("associate purchase orders with agreement charges
    /// after an agreement is created") and the approval of a payment request go
    /// through, so a charge cannot end up carrying two. The agreement's own
    /// record changed, which is what `AgreementViewSummary$lastUpdateTime`
    /// reports.
    pub fn associate_purchase_order(&mut self, association: PurchaseOrder, now: SystemTime) {
        match self
            .purchase_orders
            .iter_mut()
            .find(|held| held.charge_id == association.charge_id)
        {
            Some(held) => *held = association,
            None => self.purchase_orders.push(association),
        }
        self.updated_at = now;
    }

    /// Apply to this agreement the amendment the quote `request_id` priced,
    /// whose accepted terms are `accepted_terms` and which commits the
    /// agreement until `end_time`, at `now`.
    ///
    /// `Intent` `AMEND` "modifies an existing agreement with terms that are
    /// accepted in the request", so the terms accepted in that request become
    /// this agreement's accepted terms and the window they commit it for
    /// becomes its end date ([`agreement_end_time`]); the amendment's own quote
    /// joins the quotes of this agreement, so what it priced is this
    /// agreement's value and the charges it minted are this agreement's
    /// charges. The record changed, which is what
    /// `AgreementViewSummary$lastUpdateTime` reports: "an agreement is updated
    /// when any of its attributes or accepted terms change. Amendments,
    /// renewals, and a party changing whether the agreement renews are all
    /// examples."
    ///
    /// Nothing else about the agreement moves. An amendment is none of the
    /// status transitions this service performs — [`Agreement::leave_active`]
    /// names them — and the entitlements the agreement granted entitle its
    /// acceptor to the same resources under the same licences, so neither is
    /// re-stated here.
    pub fn amend(
        &mut self,
        request_id: String,
        accepted_terms: Vec<AcceptedTerm>,
        end_time: Option<SystemTime>,
        now: SystemTime,
    ) {
        self.accepted_terms = accepted_terms;
        self.end_time = end_time;
        self.amendment_request_ids.push(request_id);
        self.updated_at = now;
    }

    /// Move this agreement out of [`STATUS_ACTIVE`] to the terminal `status` at
    /// `now`: the ONE writer of an agreement's status, so every transition this
    /// service performs is bounded the same way and the `updated_at` that goes
    /// with it is stamped in one place.
    ///
    /// Only an [`STATUS_ACTIVE`] agreement is moved; one that has already
    /// reached a terminal status (cancelled, replaced, expired) keeps it rather
    /// than being moved backwards into or across a status it has already
    /// passed, whatever later call asks. The answer says whether this call is
    /// what moved it, so a caller that owes an audit record emits one only for
    /// the transition it actually made.
    ///
    /// `updated_at` is "when the agreement was last updated"
    /// (`AgreementViewSummary$lastUpdateTime`), so it reports the LATEST change
    /// the record carries and never moves backwards: a transition stamped at the
    /// instant it was DUE — which is what the clock-driven ones are
    /// ([`Agreement::expire`], [`Agreement::renew`]) — leaves a change already
    /// recorded later in place rather than re-dating the record before it. That
    /// is what keeps repeated reads reporting one instant while the key
    /// `SearchAgreements` sorts by, and the value its `BeforeLastUpdateTime` and
    /// `AfterLastUpdateTime` filters select on, stay the newest change each
    /// agreement has.
    #[must_use]
    fn leave_active(&mut self, status: &str, now: SystemTime) -> bool {
        if self.status != STATUS_ACTIVE {
            return false;
        }
        self.status = status.to_string();
        self.updated_at = self.updated_at.max(now);
        true
    }

    /// Cancel this agreement at `now`: the one transition to
    /// [`STATUS_CANCELLED`], which both the acceptor's direct `CancelAgreement`
    /// and the approval of a cancellation request
    /// (`AcceptAgreementCancellationRequest`) perform, so the status and the
    /// `updated_at` that goes with it have exactly one writer.
    ///
    /// Guarded by [`Agreement::leave_active`]: an agreement that has already
    /// reached a terminal status keeps it.
    #[must_use]
    pub fn cancel(&mut self, now: SystemTime) -> bool {
        self.leave_active(STATUS_CANCELLED, now)
    }

    /// Record this agreement as superseded at `now`: the one transition to
    /// [`STATUS_REPLACED`], which accepting a `REPLACE` request naming it as its
    /// source performs.
    ///
    /// Guarded by [`Agreement::leave_active`] exactly as [`Agreement::cancel`]
    /// is: an agreement that has already reached a terminal status — one the
    /// acceptor cancelled, one an earlier replacement already superseded — keeps
    /// it, so the status it reached is never overwritten by a later
    /// replacement.
    #[must_use]
    pub fn replace(&mut self, now: SystemTime) -> bool {
        self.leave_active(STATUS_REPLACED, now)
    }

    /// Record this agreement as renewed into a new agreement at `now`: the one
    /// transition to [`STATUS_RENEWED`], which the clock performs at the end
    /// date of an agreement whose terms renew it there
    /// ([`ComAmazonawsMarketplaceagreementAwsmpcommerceserviceV20200301State::settle_agreement`],
    /// which is also what creates the renewal agreement itself).
    ///
    /// Guarded by [`Agreement::leave_active`] exactly as [`Agreement::cancel`]
    /// is, which is also what makes the renewal happen ONCE: the agreement this
    /// moved is no longer `ACTIVE`, so a later call finds nothing due and mints
    /// no second renewal.
    #[must_use]
    pub fn renew(&mut self, now: SystemTime) -> bool {
        self.leave_active(STATUS_RENEWED, now)
    }

    /// Materialise the expiry this agreement's own stored end date has already
    /// made due by `now`.
    ///
    /// `endTime` is "the date and time when the agreement ends", and the status
    /// the model gives for having reached it is "EXPIRED – The agreement ended
    /// on the defined end date": the transition is owed at that instant whether
    /// or not anybody calls, so every operation that reports or guards on an
    /// agreement's status materialises it before it answers — the same way the
    /// billing readers settle a pending adjustment at its own due instant
    /// ([`ComAmazonawsMarketplaceagreementAwsmpcommerceserviceV20200301State::settle_billing_adjustments`]).
    /// An agreement with no end date ("the field is null for pay-as-you-go
    /// agreements, which don't have end dates") never reaches one, so nothing is
    /// ever due for it.
    ///
    /// Only an agreement its end date ENDS is moved, which is what the behaviour
    /// at that date decides ([`Agreement::end_time_behavior_at_end_date`]):
    /// `EXPIRE` is "the agreement ends and isn't renewed or replaced", while an
    /// agreement that `RENEW`s there is renewed into a new agreement instead and
    /// carries [`STATUS_RENEWED`] — the transition
    /// [`ComAmazonawsMarketplaceagreementAwsmpcommerceserviceV20200301State::settle_agreement`]
    /// performs beside the renewal agreement it creates, so neither end is
    /// reported without the other.
    ///
    /// `updated_at` becomes the instant the change was DUE, never the instant it
    /// was observed, so repeated reads report the same `lastUpdateTime` — and
    /// never an instant earlier than a change the record already carries, which
    /// [`Agreement::leave_active`] is where that floor is applied. Guarded
    /// by [`Agreement::leave_active`] exactly as [`Agreement::cancel`] is, so an
    /// agreement the acceptor cancelled or a replacement superseded before its
    /// end date keeps the status it reached.
    fn expire(&mut self, now: SystemTime) {
        if !self.reached_end_date(now) || self.renews_at_end_date() {
            // Either the end date is still ahead (or there is none at all, so
            // nothing is ever due), or it renews this agreement rather than
            // ending it.
            return;
        }
        let Some(end) = self.end_time else {
            return;
        };
        // Whether this call is what moved it matters to nobody: no audit record
        // is owed for a transition no caller made.
        let _ = self.leave_active(STATUS_EXPIRED, end);
    }

    /// Whether this agreement's own stored end date has passed by `now`.
    ///
    /// An agreement with no end date never reaches one — "the field is null for
    /// pay-as-you-go agreements, which don't have end dates" — so nothing its
    /// end date would make due is ever due for it.
    fn reached_end_date(&self, now: SystemTime) -> bool {
        self.end_time.is_some_and(|end| now >= end)
    }

    /// Whether the acceptor asked for auto-renewal, as the accepted renewal
    /// term's `enableAutoRenew` records it.
    fn auto_renew(&self) -> Option<bool> {
        self.accepted_terms.iter().find_map(|term| match term {
            AcceptedTerm::renewalTerm(renewal) => Some(
                renewal
                    .configuration
                    .as_ref()
                    .is_some_and(|configuration| configuration.enable_auto_renew),
            ),
            _ => None,
        })
    }

    /// How many times the accepted renewal term allows the chain this agreement
    /// belongs to to be renewed, or `None` for a term that sets no limit.
    ///
    /// `RenewalTerm$maxRenewals` is "the maximum number of times the agreement
    /// can be renewed. The field is null when the number of renewals is
    /// unlimited."
    fn renewal_limit(&self) -> Option<i32> {
        self.accepted_terms.iter().find_map(|term| match term {
            AcceptedTerm::renewalTerm(renewal) => renewal.max_renewals,
            _ => None,
        })
    }

    /// Whether the chain this agreement belongs to has used up the renewals its
    /// accepted renewal term allows: "after the agreement reaches this limit, it
    /// expires on its end date instead of renewing"
    /// (`RenewalTerm$maxRenewals`).
    fn renewal_limit_reached(&self) -> bool {
        self.renewal_limit()
            .is_some_and(|max| self.renewal_count >= u32::try_from(max).unwrap_or(0))
    }

    /// Whether this agreement is renewed into a new agreement at its end date
    /// rather than ended by it: the `RENEW` case of
    /// [`Agreement::end_time_behavior_at_end_date`].
    pub fn renews_at_end_date(&self) -> bool {
        self.end_time.is_some() && self.auto_renew() == Some(true) && !self.renewal_limit_reached()
    }

    /// What happens to this agreement at its end date, computed from the end
    /// date and the accepted terms it stores: `None` only for an agreement that
    /// has no end date, which "never reach[es] an end time", as
    /// `AgreementViewSummary$endTimeBehaviorType` states its single null case.
    ///
    /// This is the behaviour itself, so it is the value every projection whose
    /// member documents no further null case reports, whatever status the
    /// agreement has reached: a cancellation, a replacement or the renewal of
    /// the agreement leaves the stored end date and accepted terms untouched, so
    /// what the agreement does at that date stays computable from them. The
    /// projection that withholds it from an inactive agreement is
    /// [`Agreement::end_time_behavior`], which states that rule from its own
    /// member.
    ///
    /// The behaviour follows `EndTimeBehavior`: `RENEW` when the accepted
    /// renewal term enables auto-renewal and the chain has renewals left under
    /// it, and otherwise `EXPIRE` with the reason the terms give —
    /// `NO_RENEWAL_TERM` when they carry no renewal term at all,
    /// `ACCEPTOR_RENEW_OPTED_OUT` when they carry one the acceptor turned off,
    /// and `RENEWAL_LIMIT_EXHAUSTED` once "the agreement reached the maximum
    /// number of renewals allowed by its renewal term", after which "it expires
    /// on its end date instead of renewing".
    pub fn end_time_behavior_at_end_date(&self) -> Option<EndTimeBehavior> {
        self.end_time?;
        let expires = |reason_code: &str| EndTimeBehavior {
            r#type: "EXPIRE".to_string(),
            reason_code: Some(reason_code.to_string()),
            renewal_summary: None,
        };
        Some(match self.auto_renew() {
            Some(true) if !self.renewal_limit_reached() => EndTimeBehavior {
                r#type: "RENEW".to_string(),
                // "The field is null when the agreement renews."
                reason_code: None,
                // The offer the agreement would renew from belongs to the
                // proposal this emulator does not hold.
                renewal_summary: None,
            },
            Some(true) => expires("RENEWAL_LIMIT_EXHAUSTED"),
            Some(false) => expires("ACCEPTOR_RENEW_OPTED_OUT"),
            None => expires("NO_RENEWAL_TERM"),
        })
    }

    /// The `DescribeAgreementOutput$endTimeBehavior` projection of
    /// [`Agreement::end_time_behavior_at_end_date`], which that member reports
    /// under a NARROWER rule than the behaviour itself has: "This field is
    /// present for every active agreement that has an end date. It is not
    /// present for an agreement that has no end date… It is also not present for
    /// an agreement that is no longer active." The second sentence belongs to
    /// this member alone, so it is applied here rather than inside the
    /// behaviour every other projection reads.
    pub fn end_time_behavior(&self) -> Option<EndTimeBehavior> {
        if self.status != STATUS_ACTIVE {
            return None;
        }
        self.end_time_behavior_at_end_date()
    }

    /// The provisioning state of one entitlement of this agreement at `now`:
    /// its `AgreementEntitlementStatus` and the `…StatusReasonCode` that says
    /// why it is in that state.
    ///
    /// An entitlement is granted BY the agreement, so its state follows the
    /// agreement's own and the clock: an agreement that is no longer `ACTIVE`
    /// entitles its acceptor to nothing (`DEPROVISIONED`, `AGREEMENT_INACTIVE`),
    /// one that has not started yet has its entitlement waiting for that start
    /// date (`SCHEDULED`, `FUTURE_START_DATE`), and one that has started is
    /// provisioning until the instant the clock owes the entitlement its licence
    /// (`PENDING`, `PROVISIONING_IN_PROGRESS`), and provisioned from then on
    /// (`PROVISIONED`, `AGREEMENT_ACTIVE`).
    pub fn entitlement_state(
        &self,
        record: &AgreementEntitlementRecord,
        now: SystemTime,
    ) -> (&'static str, &'static str) {
        if self.status != STATUS_ACTIVE {
            (
                ENTITLEMENT_DEPROVISIONED,
                ENTITLEMENT_REASON_AGREEMENT_INACTIVE,
            )
        } else if now < self.start_time {
            (ENTITLEMENT_SCHEDULED, ENTITLEMENT_REASON_FUTURE_START_DATE)
        } else if now < record.provisioned_at {
            (ENTITLEMENT_PENDING, ENTITLEMENT_REASON_PROVISIONING)
        } else {
            (
                ENTITLEMENT_PROVISIONED,
                ENTITLEMENT_REASON_AGREEMENT_ACTIVE,
            )
        }
    }

    /// The `GetAgreementEntitlements` projection of this agreement's
    /// entitlements at `now`, in the order they were provisioned in.
    ///
    /// Every member is read from the record the acceptance stored, except the
    /// two the agreement and the clock decide (`status` and `statusReasonCode`)
    /// and `registrationToken`, which is reported only while the token the
    /// entitlement was minted with is still valid — it "is only valid for 30
    /// minutes after creation", so past that instant it is absent rather than
    /// offered to an acceptor it would no longer register with the provider.
    /// Nothing here mints anything: a read reports what provisioning stored.
    pub fn to_entitlements(&self, now: SystemTime) -> Vec<AgreementEntitlement> {
        self.entitlements
            .iter()
            .map(|record| {
                let (status, reason_code) = self.entitlement_state(record, now);
                AgreementEntitlement {
                    resource: Some(record.resource.clone()),
                    r#type: Some(ENTITLEMENT_TYPE.to_string()),
                    registration_token: (now < record.registration_token_expires_at)
                        .then(|| record.registration_token.clone()),
                    status: Some(status.to_string()),
                    status_reason_code: Some(reason_code.to_string()),
                    license_arn: Some(record.license_arn.clone()),
                }
            })
            .collect()
    }

    /// The `DescribeAgreement` projection of this agreement.
    ///
    /// `estimated_charges` is the agreement's cost, which the quote it was
    /// accepted from already priced
    /// ([`ComAmazonawsMarketplaceagreementAwsmpcommerceserviceV20200301State::estimated_charges`]):
    /// it is passed in rather than recomputed here so the amounts the agreement
    /// reports are the ones its own quote reported.
    pub fn to_description(&self, estimated_charges: EstimatedCharges) -> DescribeAgreementOutput {
        DescribeAgreementOutput {
            agreement_id: Some(self.id.clone()),
            acceptor: Some(self.acceptor.clone()),
            proposer: Some(self.proposer.clone()),
            start_time: Some(::aws_smithy_types::DateTime::from(self.start_time)),
            end_time: self.end_time.map(::aws_smithy_types::DateTime::from),
            acceptance_time: Some(::aws_smithy_types::DateTime::from(self.acceptance_time)),
            agreement_type: Some(self.agreement_type.clone()),
            estimated_charges: Some(estimated_charges),
            proposal_summary: Some(self.proposal_summary.clone()),
            status: Some(self.status.clone()),
            initial_agreement_id: Some(self.initial_agreement_id.clone()),
            end_time_behavior: self.end_time_behavior(),
            // "The free trial details of the agreement. This field is present
            // only when the agreement includes a free trial."
            free_trial_summary: self.free_trial_end_time().map(|trial_end_time| {
                FreeTrialSummary {
                    trial_end_time: Some(::aws_smithy_types::DateTime::from(trial_end_time)),
                }
            }),
        }
    }

    /// When the free trial of this agreement ends, and `None` for an agreement
    /// whose accepted terms include no free trial.
    ///
    /// `FreeTrialSummary$trialEndTime` is "the date and time when the free trial
    /// ends": the trial runs from the agreement's start date for the duration its
    /// accepted `FreeTrialPricingTerm` states ("duration of the free trial
    /// period"), which is the ONE place this emulator reports a trial end from —
    /// so the summary of the agreement and its search view cannot disagree.
    /// A term whose duration is not an ISO 8601 span names no instant, so no
    /// trial end is reported for it.
    pub fn free_trial_end_time(&self) -> Option<SystemTime> {
        let duration = self.accepted_terms.iter().find_map(|term| match term {
            AcceptedTerm::freeTrialPricingTerm(trial) => trial.duration.as_deref(),
            _ => None,
        })?;
        self.start_time
            .checked_add(::service_common::iso8601_span::iso8601_span(duration)?)
    }

    /// The kinds of contact information this agreement has on file, as
    /// `AgreementViewSummary$includesContactOfType` lists them.
    ///
    /// "For example, `ACCEPTOR` indicates that the acceptor's contact information
    /// is on file, which you can retrieve with `GetAgreementContacts`": the one
    /// kind `ContactType` publishes, listed exactly when this agreement holds
    /// that contact — so the member agrees with what `GetAgreementContacts`
    /// answers. Absent, not empty, for an agreement with no contact on file.
    pub fn contact_types(&self) -> Option<Vec<String>> {
        self.acceptor_contact
            .as_ref()
            .map(|_| vec![CONTACT_TYPE_ACCEPTOR.to_string()])
    }

    /// The `SearchAgreements` projection of this agreement.
    ///
    /// `endTimeBehaviorType`/`endTimeBehaviorReasonCode` carry the behaviour at
    /// the end date itself ([`Agreement::end_time_behavior_at_end_date`]): the
    /// summary member documents ONE null case, "agreements that have no end
    /// date", so an agreement that has one reports what it does at that date
    /// whatever status it has since reached.
    pub fn to_view_summary(&self) -> AgreementViewSummary {
        let behavior = self.end_time_behavior_at_end_date();
        AgreementViewSummary {
            agreement_id: Some(self.id.clone()),
            acceptance_time: Some(::aws_smithy_types::DateTime::from(self.acceptance_time)),
            start_time: Some(::aws_smithy_types::DateTime::from(self.start_time)),
            end_time: self.end_time.map(::aws_smithy_types::DateTime::from),
            last_update_time: Some(::aws_smithy_types::DateTime::from(self.updated_at)),
            agreement_type: Some(self.agreement_type.clone()),
            acceptor: Some(self.acceptor.clone()),
            proposer: Some(self.proposer.clone()),
            proposal_summary: Some(self.proposal_summary.clone()),
            status: Some(self.status.clone()),
            // "A list of entitlements associated with the agreement": an
            // `Entitlement` carries only the licence of each entitlement the
            // acceptance provisioned; `GetAgreementEntitlements` reports the
            // same entitlements in full. Absent, not empty, for an agreement
            // that granted none.
            entitlements: (!self.entitlements.is_empty()).then(|| {
                self.entitlements
                    .iter()
                    .map(|record| Entitlement {
                        license_arn: Some(record.license_arn.clone()),
                    })
                    .collect()
            }),
            initial_agreement_id: Some(self.initial_agreement_id.clone()),
            end_time_behavior_type: behavior.as_ref().map(|b| b.r#type.clone()),
            end_time_behavior_reason_code: behavior.and_then(|b| b.reason_code),
            // "The date and time when the free trial of the agreement ends. This
            // field is present only when the agreement includes a free trial" —
            // the same instant `DescribeAgreement` reports in its
            // `freeTrialSummary` ([`Agreement::free_trial_end_time`]).
            free_trial_end_time: self
                .free_trial_end_time()
                .map(::aws_smithy_types::DateTime::from),
            includes_contact_of_type: self.contact_types(),
        }
    }

    /// The accepted term of this agreement that `id` names, or `None` when it
    /// holds none with that id.
    ///
    /// The `TermID` is "common between offer and agreement", so a term of this
    /// id on an agreement derived from this one is the same term of the same
    /// proposal — which is what [`resolved_term_kind`] reads a carried-over
    /// term's kind from.
    pub fn accepted_term(&self, id: &str) -> Option<&AcceptedTerm> {
        self.accepted_terms
            .iter()
            .find(|term| accepted_term_id(term) == Some(id))
    }

    /// The accepted `VariablePaymentTerm` of this agreement that `term_id`
    /// names, or `None` when no accepted term of that kind carries that id.
    ///
    /// `SendAgreementPaymentRequest` is defined over exactly this term — "an
    /// agreement that includes a `VariablePaymentTerm`" — and
    /// `SendAgreementPaymentRequestInput$termId` names "the unique identifier of
    /// the `VariablePaymentTerm` for the agreement".
    pub fn variable_payment_term(&self, term_id: &str) -> Option<&VariablePaymentTerm> {
        self.accepted_terms.iter().find_map(|term| match term {
            AcceptedTerm::variablePaymentTerm(variable)
                if variable.id.as_deref() == Some(term_id) =>
            {
                Some(variable)
            }
            _ => None,
        })
    }

    /// The instant `sort_by` orders this agreement by, absent when the
    /// agreement has no value for that attribute.
    pub fn sort_key(&self, sort_by: &str) -> Option<SystemTime> {
        match sort_by {
            "StartTime" => Some(self.start_time),
            "LastUpdateTime" => Some(self.updated_at),
            // `DEFAULT_SORT_BY`.
            _ => self.end_time,
        }
    }
}

/// One payment request — what a seller submits to a buyer "for a specific
/// charge amount for an agreement that includes a `VariablePaymentTerm`" — as
/// this emulator persists it.
///
/// `ListAgreementPaymentRequests` reports these, and every member it projects is
/// read from this record or from the agreement it names; nothing is echoed from
/// the listing call's own input.
#[derive(Debug, Clone)]
pub struct PaymentRequest {
    /// `^pr-[a-zA-Z0-9]+$`, minted by
    /// [`ComAmazonawsMarketplaceagreementAwsmpcommerceserviceV20200301State::mint_payment_request_id`].
    pub id: String,
    /// The agreement the request is for; the agreement itself stays the single
    /// copy of its own type, catalog and parties.
    pub agreement_id: String,
    /// The `VariablePaymentTerm` of that agreement the request was sent under.
    pub term_id: String,
    pub name: String,
    pub description: Option<String>,
    pub charge_amount: String,
    /// "The currency code for the charge amount": the currency of the accepted
    /// term, which is `VariablePaymentTerm$currencyCode` — read from that term
    /// when the request is sent, so a request reports the currency the term it
    /// was sent under is denominated in and nothing re-derives it later.
    pub currency_code: Option<String>,
    /// One of `PaymentRequestStatus`'s values.
    pub status: String,
    /// "An optional message providing additional context about the payment
    /// request status, such as a rejection reason or validation failure
    /// details": the value of whichever call that moved the request supplied
    /// one, which is the buyer's `rejectionReason`
    /// (`RejectAgreementPaymentRequest`). A status no call carries a message
    /// member for leaves it absent rather than inventing one.
    pub status_message: Option<String>,
    /// "The unique identifier of the charge created after the payment request is
    /// approved. This field is only present for approved payment requests" —
    /// approving one is `AcceptAgreementPaymentRequest`, which mints the charge
    /// its approval creates ([`PaymentRequest::approve`]), so a request no
    /// approval has moved carries none. Read through
    /// [`PaymentRequest::created_charge_id`], which is what decides whether a
    /// stored charge is reportable.
    pub charge_id: Option<String>,
    /// The account that submitted the request, which is its proposer: "Allows
    /// sellers (proposers) to submit a payment request to buyers (acceptors)".
    pub proposer_account_id: String,
    /// The `clientToken` the send carried, if any.
    pub client_token: Option<String>,
    /// The instant this request's automatic approval is owed at, for a request
    /// sent under a term whose acceptor configured one
    /// ([`payment_request_auto_approval`]), and `None` for a request only the
    /// buyer's own decision moves.
    ///
    /// Held on the record rather than re-derived from the term, the way the
    /// sibling [`BillingAdjustmentRequest::completes_at`] holds the instant its
    /// own transition is owed at: the configuration the send read is what
    /// governs that send, and the instant is materialised rather than slept for
    /// ([`ComAmazonawsMarketplaceagreementAwsmpcommerceserviceV20200301State::settle_payment_requests`]).
    pub auto_approves_at: Option<SystemTime>,
    pub created_at: SystemTime,
    pub updated_at: SystemTime,
}

impl PaymentRequest {
    /// Whether this request is the one `input` asks to send — every member the
    /// request carries beside its `clientToken` is the same.
    ///
    /// A send replaying the same token with the same details reports this
    /// request; one reusing it for different details is a conflict, as the
    /// sibling `BillingAdjustmentRequest::matches_entry` treats the same case.
    pub fn matches_send(&self, input: &SendAgreementPaymentRequestInput) -> bool {
        self.agreement_id == input.agreement_id
            && self.term_id == input.term_id
            && self.name == input.name
            && self.charge_amount == input.charge_amount
            && self.description == input.description
    }

    /// Whether a decision on this payment request is still open — the one
    /// precondition every operation that moves it shares.
    ///
    /// A request is decidable only in [`PAYMENT_REQUEST_STATUS_PENDING_APPROVAL`]:
    /// "Only payment requests in PENDING_APPROVAL status can be cancelled"
    /// (`CancelAgreementPaymentRequest`), and the buyer's accept and reject state
    /// the same bound. Every other `PaymentRequestStatus` value is one a decision
    /// has already moved it to, or one the validation of the send left it in, so
    /// deciding now would move it a second time.
    pub fn is_pending_approval(&self) -> bool {
        self.status == PAYMENT_REQUEST_STATUS_PENDING_APPROVAL
    }

    /// The instant this request's automatic approval was owed at, once `now` has
    /// reached it — `None` for a request that is owed none, one whose instant is
    /// still ahead, and one a decision has already moved.
    ///
    /// The instant itself is the answer rather than a yes/no, because the
    /// approval is dated at the instant it was DUE and not at the instant
    /// somebody looked.
    pub fn auto_approval_due(&self, now: SystemTime) -> Option<SystemTime> {
        let due = self.auto_approves_at?;
        (self.is_pending_approval() && now >= due).then_some(due)
    }

    /// Cancel this payment request at `now`: the one transition to
    /// [`PAYMENT_REQUEST_STATUS_CANCELLED`], so the status and the `updated_at`
    /// that goes with it have exactly one writer.
    ///
    /// `updatedAt` is "the date and time when the payment request was
    /// cancelled", so it becomes this instant while `created_at` keeps the one
    /// the send recorded. Callers check [`PaymentRequest::is_pending_approval`]
    /// first and refuse anything else as a conflict.
    pub fn cancel(&mut self, now: SystemTime) {
        self.status = PAYMENT_REQUEST_STATUS_CANCELLED.to_string();
        self.updated_at = now;
    }

    /// Reject this payment request at `now`: the one transition to
    /// [`PAYMENT_REQUEST_STATUS_REJECTED`], so the status, the status message
    /// and the `updated_at` that go with it have exactly one writer.
    ///
    /// `updatedAt` is "the date and time when the payment request was
    /// rejected", so it becomes this instant while `created_at` keeps the one
    /// the send recorded. `reason` is the buyer's optional rejection reason —
    /// "this message is visible to the seller" — and it becomes the request's
    /// `statusMessage`, absent when the call supplied none. Callers check
    /// [`PaymentRequest::is_pending_approval`] first and refuse anything else
    /// as a conflict.
    pub fn reject(&mut self, now: SystemTime, reason: Option<String>) {
        self.status = PAYMENT_REQUEST_STATUS_REJECTED.to_string();
        self.status_message = reason;
        self.updated_at = now;
    }

    /// Approve this payment request at `now`, the charge its approval created
    /// being `charge_id`: the one transition to
    /// [`PAYMENT_REQUEST_STATUS_APPROVED`], so the status, the charge and the
    /// `updated_at` that go with them have exactly one writer.
    ///
    /// "Once accepted, the payment request transitions to APPROVED status and
    /// the charge will be processed", and `chargeId` is "the unique identifier
    /// of the charge created after the payment request is approved" — so the
    /// charge is recorded here and nowhere else. `updatedAt` is "the date and
    /// time when the payment request was accepted", so it becomes this instant
    /// while `created_at` keeps the one the send recorded. Callers check
    /// [`PaymentRequest::is_pending_approval`] first and refuse anything else
    /// as a conflict.
    pub fn approve(&mut self, now: SystemTime, charge_id: String) {
        self.status = PAYMENT_REQUEST_STATUS_APPROVED.to_string();
        self.charge_id = Some(charge_id);
        self.updated_at = now;
    }

    /// The charge this request's approval created, if an approval created one.
    ///
    /// `chargeId` is "only present for approved payment requests", so this is
    /// the one place that decides whether a stored charge is reportable, and
    /// every projection of this record — and every report of the charge itself
    /// — reads it through here.
    pub fn created_charge_id(&self) -> Option<&str> {
        self.charge_id
            .as_deref()
            .filter(|_| self.status == PAYMENT_REQUEST_STATUS_APPROVED)
    }

    /// This request's `chargeAmount`, in the units [`amount_units`] counts, and
    /// zero for an amount this service cannot count at all — which no stored
    /// request carries, because a send naming one is refused before anything is
    /// stored.
    pub fn charge_units(&self) -> u128 {
        amount_units(&self.charge_amount).unwrap_or_default()
    }

    /// Whether this request still stands against the ceiling of the term it was
    /// sent under — whether its amount is part of what has been or may yet be
    /// charged under that term.
    ///
    /// A request awaiting the buyer's decision may still be charged, and an
    /// approved one already has been ("the charge will be processed"). A request
    /// in a status no charge can follow — cancelled, rejected, or one whose
    /// validation failed — will never be charged, so its amount is available to
    /// the term again.
    pub fn holds_term_balance(&self) -> bool {
        !matches!(
            self.status.as_str(),
            PAYMENT_REQUEST_STATUS_CANCELLED
                | PAYMENT_REQUEST_STATUS_REJECTED
                | PAYMENT_REQUEST_STATUS_VALIDATION_FAILED
        )
    }

    /// The `ListAgreementPaymentRequests` projection of this payment request.
    pub fn to_summary(&self) -> PaymentRequestSummary {
        PaymentRequestSummary {
            payment_request_id: Some(self.id.clone()),
            agreement_id: Some(self.agreement_id.clone()),
            status: Some(self.status.clone()),
            name: Some(self.name.clone()),
            // Only an approved request has a charge; see `charge_id`.
            charge_id: self.created_charge_id().map(str::to_string),
            charge_amount: Some(self.charge_amount.clone()),
            currency_code: self.currency_code.clone(),
            created_at: Some(::aws_smithy_types::DateTime::from(self.created_at)),
            updated_at: Some(::aws_smithy_types::DateTime::from(self.updated_at)),
        }
    }

    /// The whole payment request, as `GetAgreementPaymentRequest` reports it:
    /// "the current status, charge details, timestamps, and the charge ID if the
    /// request has been approved".
    ///
    /// Every member is read from this one stored record, so the detail and the
    /// [`PaymentRequest::to_summary`] a list reports can never disagree about
    /// the same request.
    pub fn to_detail(&self) -> GetAgreementPaymentRequestOutput {
        GetAgreementPaymentRequestOutput {
            payment_request_id: Some(self.id.clone()),
            agreement_id: Some(self.agreement_id.clone()),
            status: Some(self.status.clone()),
            // `statusMessage` is "an optional message providing additional
            // context about the payment request status, such as a rejection
            // reason or validation failure details", which only a call that
            // supplies one gives a value to: the buyer's `rejectionReason`
            // (`RejectAgreementPaymentRequest`) is the one this service holds.
            // The seller's own `CancelAgreementPaymentRequest` models no reason
            // member at all, so a cancelled request has no message to report
            // and this member stays absent rather than being invented.
            status_message: self.status_message.clone(),
            name: Some(self.name.clone()),
            description: self.description.clone(),
            // Only an approved request has a charge; see `charge_id`.
            charge_id: self.created_charge_id().map(str::to_string),
            charge_amount: Some(self.charge_amount.clone()),
            currency_code: self.currency_code.clone(),
            created_at: Some(::aws_smithy_types::DateTime::from(self.created_at)),
            updated_at: Some(::aws_smithy_types::DateTime::from(self.updated_at)),
        }
    }
}

/// Whether the caller is the `party_type` party of a request a SELLER submitted
/// under an agreement — a payment request ([`PaymentRequest`]) or a cancellation
/// request ([`AgreementCancellationRequest`]).
///
/// A `partyType` states the two roles the same way for both listings: "Use
/// `Proposer` to list … where you are the seller, or `Acceptor` to list … where
/// you are the buyer." The seller is the account that SUBMITTED the request,
/// which this service records for every seller-submitted request
/// (`SendAgreementPaymentRequest` and `SendAgreementCancellationRequest` are
/// both documented as the seller's own call) — so `proposer_account_id` is the
/// account the submitting call recorded on the request itself, rather than the
/// proposer of the agreement it was sent under. The buyer is the acceptor of
/// that agreement, which [`caller_is_party`] decides — so `agreement` is the
/// stored agreement the request's `agreement_id` names, and a request whose
/// agreement is gone has no buyer to match.
///
/// A `party_type` that is neither value matches nothing; the handler refuses it
/// before any request is scanned.
pub fn submitted_request_party_matches(
    proposer_account_id: &str,
    agreement: Option<&Agreement>,
    party_type: &str,
) -> bool {
    match party_type {
        PARTY_TYPE_PROPOSER => proposer_account_id == ::engine::emulator_account_id(),
        PARTY_TYPE_ACCEPTOR => {
            agreement.is_some_and(|agreement| caller_is_party(agreement, PARTY_TYPE_ACCEPTOR))
        }
        _ => false,
    }
}

/// The accounts of the two parties of an agreement this emulator mints.
///
/// An agreement names an account for BOTH of its sides: `Acceptor` is "the
/// details of the party accepting the agreement terms" ("commonly the buyer")
/// and `Proposer` "details of the party proposing the agreement terms"
/// ("commonly the seller"), each carrying "the AWS account ID" of that party —
/// so neither side of a stored agreement is reported empty.
///
/// Both are the single account this emulator serves
/// ([`::engine::emulator_account_id`], which every account-valued member it
/// reports carries), because that one account makes the calls of BOTH
/// documented roles of this API: it accepts an agreement request — the buyer's
/// call, whose acceptor "can optionally provide purchase orders" — and it
/// submits the payment and cancellation requests the model documents as the
/// seller's ("Allows sellers (proposers) to …"). So it is the acceptor and the
/// proposer of every agreement the store holds, which is what makes both
/// members reportable and either `PartyType` selectable ([`caller_is_party`]).
pub fn agreement_parties() -> (Acceptor, Proposer) {
    let account = ::engine::emulator_account_id().to_string();
    (
        Acceptor {
            account_id: Some(account.clone()),
        },
        Proposer {
            account_id: Some(account),
        },
    )
}

/// Whether the caller is the party `party_type` names on `agreement`.
///
/// A party type is "the party type of the caller", so it selects by the account
/// the agreement records for that side ([`agreement_parties`]). A party type
/// that is neither value selects nothing; a handler refuses it where it is a
/// request member, with [`validation_exception`] and the modelled
/// `INVALID_PARTY_TYPE` reason.
///
/// Shared by `SearchAgreements`' `PartyType` filter and by the acceptor side of
/// [`submitted_request_party_matches`], which is how every listing of
/// seller-submitted requests resolves its buyer.
pub fn caller_is_party(agreement: &Agreement, party_type: &str) -> bool {
    let caller = Some(::engine::emulator_account_id());
    match party_type {
        PARTY_TYPE_ACCEPTOR => agreement.acceptor.account_id.as_deref() == caller,
        PARTY_TYPE_PROPOSER => agreement.proposer.account_id.as_deref() == caller,
        _ => false,
    }
}

/// One agreement cancellation request, as this emulator persists it.
///
/// `SendAgreementCancellationRequest` creates it over an active agreement and
/// every later report of it (`ListAgreementCancellationRequests`,
/// `GetAgreementCancellationRequest`) is computed from this one record.
#[derive(Debug, Clone)]
pub struct AgreementCancellationRequest {
    /// `^acr-[a-zA-Z0-9]+$`, minted by
    /// [`ComAmazonawsMarketplaceagreementAwsmpcommerceserviceV20200301State::mint_agreement_cancellation_request_id`].
    pub id: String,
    /// The agreement the cancellation was asked for.
    pub agreement_id: String,
    /// The agreement's own `agreementType` and `catalog`, copied at create time:
    /// a summary reports "the type of agreement" and "the catalog in which the
    /// agreement was created", which are properties of that agreement and not of
    /// the cancelling call.
    pub agreement_type: String,
    /// See [`AgreementCancellationRequest::agreement_type`].
    pub catalog: String,
    /// One of `AgreementCancellationRequestStatus`'s values; see
    /// [`CANCELLATION_STATUS_PENDING_APPROVAL`].
    pub status: String,
    /// "A message providing additional context about the cancellation request
    /// status": the value of whichever call that moved the request supplied one,
    /// which is the buyer's `rejectionReason`
    /// (`RejectAgreementCancellationRequest`) or the seller's
    /// `cancellationReason` (`CancelAgreementCancellationRequest`). A status no call carries a
    /// message member for leaves it absent rather than inventing one, exactly as
    /// the sibling [`PaymentRequest::status_message`] does.
    pub status_message: Option<String>,
    /// One of `AgreementCancellationRequestReasonCode`'s values, as the modelled
    /// enum member the create call carried.
    pub reason_code: String,
    /// The optional free-text reason the create call carried.
    pub description: Option<String>,
    /// The account that submitted the request, which is its proposer: "Allows
    /// sellers (proposers) to submit a cancellation request for an active
    /// agreement". It is what [`submitted_request_party_matches`] resolves the
    /// seller side of a `partyType` against, exactly as the sibling
    /// [`PaymentRequest::proposer_account_id`] does.
    pub proposer_account_id: String,
    /// The `clientToken` the create call carried, if any.
    pub client_token: Option<String>,
    pub created_at: SystemTime,
    pub updated_at: SystemTime,
}

impl AgreementCancellationRequest {
    /// Whether this request is the one `input` asks to send — every member the
    /// send carries beside its `clientToken` is the same.
    ///
    /// A send replaying the same token with the same content reports this
    /// request; one reusing it for different content is a conflict, as the
    /// siblings [`PaymentRequest::matches_send`] and
    /// [`BillingAdjustmentRequest::matches_entry`] treat the same case.
    pub fn matches_send(&self, input: &SendAgreementCancellationRequestInput) -> bool {
        self.agreement_id == input.agreement_id
            && self.reason_code == input.reason_code
            && self.description == input.description
    }

    /// Whether a decision on this cancellation request is still open — the one
    /// precondition every operation that moves it shares.
    ///
    /// A request is decidable only in
    /// [`CANCELLATION_STATUS_PENDING_APPROVAL`]: "Only cancellation requests in
    /// PENDING_APPROVAL status can be accepted"
    /// (`AcceptAgreementCancellationRequest`), and the buyer's reject states the
    /// same bound. Every other `AgreementCancellationRequestStatus` value is one
    /// a decision has already moved it to, or one the validation of the send
    /// left it in, so deciding now would move it a second time.
    pub fn is_pending_approval(&self) -> bool {
        self.status == CANCELLATION_STATUS_PENDING_APPROVAL
    }

    /// Reject this cancellation request at `now`: the one transition to
    /// [`CANCELLATION_STATUS_REJECTED`], so the status, the status message and
    /// the `updated_at` that go with it have exactly one writer.
    ///
    /// `updatedAt` is "the date and time when the cancellation request was
    /// rejected", so it becomes this instant while `created_at` keeps the one
    /// the send recorded. `reason` is the buyer's required rejection reason —
    /// "this message is visible to the seller" — and it becomes the request's
    /// `statusMessage`. Callers check
    /// [`AgreementCancellationRequest::is_pending_approval`] first and refuse
    /// anything else as a conflict, the way the sibling
    /// [`PaymentRequest::reject`] is used.
    pub fn reject(&mut self, now: SystemTime, reason: String) {
        self.status = CANCELLATION_STATUS_REJECTED.to_string();
        self.status_message = Some(reason);
        self.updated_at = now;
    }

    /// Withdraw this cancellation request at `now`: the one transition to
    /// [`CANCELLATION_STATUS_CANCELLED`], so the status, the status message and
    /// the `updated_at` that go with it have exactly one writer.
    ///
    /// `updatedAt` is "the date and time when the cancellation request was
    /// cancelled", so it becomes this instant while `created_at` keeps the one
    /// the send recorded. `reason` is the seller's required cancellation reason —
    /// "a required message explaining why the cancellation request is being
    /// withdrawn" — and it becomes the request's `statusMessage`, the same way
    /// the buyer's reason becomes it in
    /// [`AgreementCancellationRequest::reject`]. Callers check
    /// [`AgreementCancellationRequest::is_pending_approval`] first and refuse
    /// anything else as a conflict.
    pub fn cancel(&mut self, now: SystemTime, reason: String) {
        self.status = CANCELLATION_STATUS_CANCELLED.to_string();
        self.status_message = Some(reason);
        self.updated_at = now;
    }

    /// The `ListAgreementCancellationRequests` projection of this request.
    pub fn to_summary(&self) -> AgreementCancellationRequestSummary {
        AgreementCancellationRequestSummary {
            agreement_cancellation_request_id: Some(self.id.clone()),
            agreement_id: Some(self.agreement_id.clone()),
            status: Some(self.status.clone()),
            reason_code: Some(self.reason_code.clone()),
            agreement_type: Some(self.agreement_type.clone()),
            catalog: Some(self.catalog.clone()),
            created_at: Some(::aws_smithy_types::DateTime::from(self.created_at)),
            updated_at: Some(::aws_smithy_types::DateTime::from(self.updated_at)),
        }
    }
}

/// The kind of an accepted term, as its own `type` member spells it.
pub fn accepted_term_kind(term: &AcceptedTerm) -> &str {
    let r#type = match term {
        AcceptedTerm::legalTerm(t) => t.r#type.as_deref(),
        AcceptedTerm::supportTerm(t) => t.r#type.as_deref(),
        AcceptedTerm::renewalTerm(t) => t.r#type.as_deref(),
        AcceptedTerm::usageBasedPricingTerm(t) => t.r#type.as_deref(),
        AcceptedTerm::configurableUpfrontPricingTerm(t) => t.r#type.as_deref(),
        AcceptedTerm::byolPricingTerm(t) => t.r#type.as_deref(),
        AcceptedTerm::recurringPaymentTerm(t) => t.r#type.as_deref(),
        AcceptedTerm::validityTerm(t) => t.r#type.as_deref(),
        AcceptedTerm::paymentScheduleTerm(t) => t.r#type.as_deref(),
        AcceptedTerm::freeTrialPricingTerm(t) => t.r#type.as_deref(),
        AcceptedTerm::fixedUpfrontPricingTerm(t) => t.r#type.as_deref(),
        AcceptedTerm::variablePaymentTerm(t) => t.r#type.as_deref(),
        AcceptedTerm::netPaymentTerm(t) => t.r#type.as_deref(),
    };
    r#type.unwrap_or(DEFAULT_TERM_KIND)
}

/// The id of an accepted term — the `TermID` "which is common between offer and
/// agreement", so it is what names the same term across a chain of agreements.
pub fn accepted_term_id(term: &AcceptedTerm) -> Option<&str> {
    match term {
        AcceptedTerm::legalTerm(t) => t.id.as_deref(),
        AcceptedTerm::supportTerm(t) => t.id.as_deref(),
        AcceptedTerm::renewalTerm(t) => t.id.as_deref(),
        AcceptedTerm::usageBasedPricingTerm(t) => t.id.as_deref(),
        AcceptedTerm::configurableUpfrontPricingTerm(t) => t.id.as_deref(),
        AcceptedTerm::byolPricingTerm(t) => t.id.as_deref(),
        AcceptedTerm::recurringPaymentTerm(t) => t.id.as_deref(),
        AcceptedTerm::validityTerm(t) => t.id.as_deref(),
        AcceptedTerm::paymentScheduleTerm(t) => t.id.as_deref(),
        AcceptedTerm::freeTrialPricingTerm(t) => t.id.as_deref(),
        AcceptedTerm::fixedUpfrontPricingTerm(t) => t.id.as_deref(),
        AcceptedTerm::variablePaymentTerm(t) => t.id.as_deref(),
        AcceptedTerm::netPaymentTerm(t) => t.id.as_deref(),
    }
}

/// One billing adjustment request: what `BatchCreateBillingAdjustmentRequest`
/// stored for one entry, and the one record `GetBillingAdjustmentRequest` and
/// `ListBillingAdjustmentRequests` report on.
///
/// `agreement_type` and `catalog` are read from the agreement the request
/// adjusts rather than from the submitting call, which carries neither: a
/// summary reports the agreement's own.
#[derive(Debug, Clone)]
pub struct BillingAdjustmentRequest {
    /// `BillingAdjustmentRequestId`'s `^ba-[a-zA-Z0-9]+$`.
    pub id: String,
    pub agreement_id: String,
    pub agreement_type: String,
    pub catalog: String,
    pub original_invoice_id: String,
    pub adjustment_amount: String,
    pub currency_code: String,
    pub adjustment_reason_code: String,
    pub description: Option<String>,
    /// The entry's `clientToken`, which is what makes a replayed submit report
    /// the request it already created.
    pub client_token: String,
    pub status: String,
    /// "This field is populated only when the status is `VALIDATION_FAILED`."
    pub status_message: Option<String>,
    pub created_at: SystemTime,
    pub updated_at: SystemTime,
    /// When the `PENDING` this request was submitted in is due to become
    /// `COMPLETED`: `created_at` plus [`BILLING_ADJUSTMENT_SETTLEMENT`]. Stored
    /// rather than slept for, and read by [`ComAmazonawsMarketplaceagreementAwsmpcommerceserviceV20200301State::settle_billing_adjustments`].
    pub completes_at: SystemTime,
}

impl BillingAdjustmentRequest {
    /// Whether this request is the one `entry` asks to create — every member the
    /// entry carries beside its `clientToken` is the same.
    ///
    /// A submit replaying the same token with the same details reports this
    /// request; one reusing it for different details is a conflict.
    pub fn matches_entry(&self, entry: &BatchCreateBillingAdjustmentRequestEntry) -> bool {
        self.agreement_id == entry.agreement_id
            && self.original_invoice_id == entry.original_invoice_id
            && self.adjustment_amount == entry.adjustment_amount
            && self.currency_code == entry.currency_code
            && self.adjustment_reason_code == entry.adjustment_reason_code
            && self.description == entry.description
    }

    /// `BillingAdjustmentSummary`, as `ListBillingAdjustmentRequests` reports
    /// this request.
    pub fn to_summary(&self) -> BillingAdjustmentSummary {
        BillingAdjustmentSummary {
            billing_adjustment_request_id: self.id.clone(),
            original_invoice_id: self.original_invoice_id.clone(),
            adjustment_amount: self.adjustment_amount.clone(),
            currency_code: self.currency_code.clone(),
            status: self.status.clone(),
            agreement_id: self.agreement_id.clone(),
            created_at: ::aws_smithy_types::DateTime::from(self.created_at),
            updated_at: ::aws_smithy_types::DateTime::from(self.updated_at),
            agreement_type: self.agreement_type.clone(),
            catalog: self.catalog.clone(),
        }
    }

    /// The whole request, as `GetBillingAdjustmentRequest` reports it.
    pub fn to_detail(&self) -> GetBillingAdjustmentRequestOutput {
        GetBillingAdjustmentRequestOutput {
            billing_adjustment_request_id: self.id.clone(),
            agreement_id: self.agreement_id.clone(),
            adjustment_reason_code: self.adjustment_reason_code.clone(),
            description: self.description.clone(),
            original_invoice_id: self.original_invoice_id.clone(),
            adjustment_amount: self.adjustment_amount.clone(),
            currency_code: self.currency_code.clone(),
            status: self.status.clone(),
            status_message: self.status_message.clone(),
            created_at: ::aws_smithy_types::DateTime::from(self.created_at),
            updated_at: ::aws_smithy_types::DateTime::from(self.updated_at),
        }
    }
}

/// The scope a `ListBillingAdjustmentRequests` page token belongs to: the whole
/// query, so a token cannot resume a different listing.
///
/// Every part is length-prefixed by [`query_scope`], and the minting operation
/// is a part too ([`listing_scope`]), so no value a caller supplies can imitate
/// the delimiters and make two different listings hash alike.
///
/// The two creation-time bounds are rendered by [`scope_instant`], the one
/// rendering every windowed scope here shares.
pub fn billing_adjustment_scope(
    operation: &str,
    agreement_id: Option<&str>,
    status: Option<&str>,
    catalog: Option<&str>,
    agreement_type: Option<&str>,
    created_after: Option<f64>,
    created_before: Option<f64>,
) -> String {
    listing_scope(
        operation,
        &[
            agreement_id.unwrap_or_default(),
            status.unwrap_or_default(),
            catalog.unwrap_or_default(),
            agreement_type.unwrap_or_default(),
            &scope_instant(created_after),
            &scope_instant(created_before),
        ],
    )
}

// ---------------------------------------------------------------------------
// Search filters
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Purchase orders
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Modelled failures
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// The store
// ---------------------------------------------------------------------------

/// The key [`ComAmazonawsMarketplaceagreementAwsmpcommerceserviceV20200301State::idempotency`]
/// holds an operation's `clientToken` under.
///
/// A `clientToken` is unique within the create that minted it, not across the
/// service, so the operation's own name is part of the key. The separator is a
/// character `ClientToken`'s `^[a-zA-Z0-9-]+$` forbids, so no token a caller
/// supplies can forge another operation's key.
pub fn idempotency_key(operation: &str, client_token: &str) -> String {
    format!("{operation}#{client_token}")
}

/// Per-service state for com-amazonaws-marketplaceagreement-awsmpcommerceservice-v20200301. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsMarketplaceagreementAwsmpcommerceserviceV20200301State {
    /// Every agreement request, keyed by its `^ar-[A-Za-z0-9]+$` id.
    pub agreement_requests: BTreeMap<String, AgreementRequest>,
    /// Every agreement approval, keyed by its `^apr-[A-Za-z0-9]+$` id. A
    /// `BTreeMap` is what gives `ListAgreementApprovals` a stable total order to
    /// page over: ids are minted from a zero-padded counter, so they sort in the
    /// order the approvals were started. Bounded by
    /// [`agreement_approvals::AGREEMENT_APPROVAL_QUOTA`], which
    /// `StartAgreementApprovals` checks before it starts one.
    pub agreement_approvals: BTreeMap<String, AgreementApproval>,
    /// Every agreement, keyed by its id. A `BTreeMap` is what gives
    /// `SearchAgreements` a stable total order to page over: ids are minted from
    /// a zero-padded counter, so they sort in creation order.
    pub agreements: BTreeMap<String, Agreement>,
    /// Every payment request, keyed by its `^pr-[a-zA-Z0-9]+$` id. A `BTreeMap`
    /// gives `ListAgreementPaymentRequests` the same stable total order to page
    /// over that `agreements` gives `SearchAgreements`. Bounded by
    /// [`PAYMENT_REQUEST_QUOTA`], which `SendAgreementPaymentRequest` checks
    /// before it stores one.
    pub payment_requests: BTreeMap<String, PaymentRequest>,
    /// Every agreement cancellation request, keyed by its
    /// `^acr-[a-zA-Z0-9]+$` id. A `BTreeMap` is what gives
    /// `ListAgreementCancellationRequests` a stable total order to page over:
    /// ids are minted from a zero-padded counter, so they sort in creation
    /// order.
    pub agreement_cancellation_requests: BTreeMap<String, AgreementCancellationRequest>,
    /// Every billing adjustment request, keyed by its `^ba-[a-zA-Z0-9]+$` id.
    /// A `BTreeMap` is what gives `ListBillingAdjustmentRequests` a stable total
    /// order to page over: ids are minted from a zero-padded counter, so they
    /// sort in creation order.
    pub billing_adjustment_requests: BTreeMap<String, BillingAdjustmentRequest>,
    /// Every invoice this service has issued, keyed by its `inv-…` id. A
    /// `BTreeMap` is what gives `ListAgreementInvoiceLineItems` a stable total
    /// order to page over: ids are minted from a zero-padded counter, so they
    /// sort in the order the invoices were issued.
    pub invoices: BTreeMap<String, Invoice>,
    /// An `@idempotencyToken` `clientToken` to the resource it created, so a
    /// replayed create answers with the first resource instead of minting a
    /// second one. Every entry is keyed by [`idempotency_key`] — the operation
    /// that minted the token and the token itself — because a `clientToken` is
    /// unique within its own create rather than across the service, so one
    /// create's token can never name another create's resource.
    pub idempotency: HashMap<String, String>,
    /// The counter every id this service mints is drawn from.
    pub id_sequence: u64,
    /// The instant the last STORE-WIDE settling walk
    /// ([`Self::settle_agreements`]) brought every chain this store holds up
    /// to, and `None` while no walk has run.
    ///
    /// What bounds how often that walk runs: nothing can become due at an
    /// instant a walk has already covered, so a second walk at the same instant
    /// would read every agreement again to change nothing. One is therefore run
    /// at most once per instant of the clock, however many calls ask for one —
    /// which is what keeps resolving an id this store holds nothing under from
    /// paying for the same walk again and again at one instant.
    pub settled_through: Option<SystemTime>,
    /// What the last publication of this store's grants was made from
    /// ([`granted_entitlements::publish`]), and `None` while none has been made.
    ///
    /// What bounds how often that publication does any work: the entitlements
    /// the agreements of this store grant are a value DERIVED from those
    /// agreements and the clock, so a publication made from a store that has not
    /// moved since the last one would rebuild the same records and write them
    /// again. One is therefore made only when this summary of the store differs
    /// from the one the last publication was made from.
    pub published_grants: Option<granted_entitlements::GrantSummary>,
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsMarketplaceagreementAwsmpcommerceserviceV20200301State>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsMarketplaceagreementAwsmpcommerceserviceV20200301State>)
}
