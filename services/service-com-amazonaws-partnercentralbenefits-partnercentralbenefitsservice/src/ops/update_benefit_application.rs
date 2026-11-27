// SPDX-License-Identifier: CC0-1.0

//! @op UpdateBenefitApplication
//! Applies the supplied members to one stored benefit application in [`crate::state`],
//! bumps its revision and `UpdatedAt` where that changed the record, and reports the
//! revision the record then holds.
//!
//! The revision is the record's optimistic-concurrency token, so it moves only when the
//! write actually CHANGED the stored record ([`crate::state::BenefitApplication::revise`]):
//! a request that names no member to apply, or names only values the record already
//! holds, is accepted and answers with the revision and `UpdatedAt` it already had —
//! exactly as `crate::ops::associate_benefit_application_resource` answers a request
//! naming a resource the application already carries.
//!
//! The operation answers in this order, which is the order its sibling write paths answer
//! in (`crate::ops::amend_benefit_application`, and the resource writes beside it):
//!
//! 1. request-shape validation against the MODELLED constraints — the centralized walk,
//!    before this handler runs, so a malformed request is a `ValidationException` whatever
//!    the store holds;
//! 2. the `Catalog` the request names, and then the partner-entitlement gate — one
//!    construction of that order for every writing operation
//!    ([`crate::state::write_request_refusal`]), which READS the stored catalog record: a
//!    catalog this service does not hold is `ResourceNotFoundException` whatever the
//!    calling account holds, and a caller holding no active AWS partner benefit whose
//!    catalog DID resolve is `AccessDeniedException`;
//! 3. the bounds this service puts on the members the model leaves unconstrained — the
//!    `BenefitApplicationDetails` document, and the `Phone` of a supplied contact and the
//!    `BusinessUseCase` of a supplied attachment. The model constrains none of them, so
//!    AWS refuses such a value on nothing of its own: these bounds exist to keep a stored
//!    record finite, and are therefore answered only once every refusal the MODELLED
//!    service owns — the walk above, and the resource and entitlement answers of step 2 —
//!    has passed, so this service's own bound can never stand in for one of those;
//! 4. the `ClientToken` replay check;
//! 5. the application itself — one that does not exist is `ResourceNotFoundException`;
//! 6. the application's STATUS, which refuses the write whatever the request carries;
//! 7. optimistic concurrency — the request's `Revision` must be the current one;
//! 8. the rules on the CONTENT this write would store, which are the application's own
//!    benefit's.
//!
//! The state of the record is therefore answered before anything about the content: a
//! request aimed at an application that admits no content change is refused with the
//! modelled `ConflictException` (HTTP 409) even when the content it carries would also be
//! refused, so one logical request gets ONE answer whichever write path it arrives on.
//!
//! An update edits the partner's own DRAFT: it is accepted only while the application is
//! `PENDING_SUBMISSION` ([`crate::state::UPDATABLE_STATUSES`]), and from any other status
//! — a submitted application AWS is reviewing, or a terminal one — it is refused with the
//! modelled `ConflictException` and nothing is persisted. A submitted application is
//! changed through `RecallBenefitApplication` (back to draft) or
//! `AmendBenefitApplication` instead.
//!
//! Optimistic concurrency: the request's `Revision` must be the application's current
//! one, so an update built on a stale read is refused with the modelled
//! `ConflictException` rather than silently overwriting a newer write.
//!
//! A `BenefitApplicationDetails` document the update would store is judged against the
//! request schema the application's benefit declares, exactly as the create's is: a
//! document that contradicts the schema is refused with the modelled
//! `ValidationException` (HTTP 400) and nothing is persisted — see
//! [`crate::state::benefit_schema_refusal`].
//!
//! A caller string this update PERSISTS that the model constrains not at all — the `Phone`
//! of a supplied contact, the `BusinessUseCase` of a supplied attachment — carries this
//! service's own bound on such a value, so one past it is refused with the modelled
//! `ValidationException` (HTTP 400) and nothing is persisted, exactly as the create and the
//! amendment refuse it: see [`crate::state::unconstrained_members_refusal`].
//!
//! An attachment the application already carries keeps the `CreatedBy`/`CreatedAt` the call
//! that attached it recorded: those report WHO uploaded that file and WHEN, so a write that
//! adds, removes, reorders or re-describes attachments leaves them alone and records the
//! calling principal and the current instant only for a file not already attached — see
//! [`crate::state::merged_file_details`].
//!
//! Idempotency: `ClientToken` identifies the REQUEST, so a retry carrying the token of an
//! update that already applied changes nothing a second time and answers with the
//! `Id`/`Arn`/`Revision` that update reported, while an update carrying that token with
//! any different member is a different request and is refused with the modelled
//! `ConflictException` (HTTP 409) — see
//! [`crate::state::ComAmazonawsPartnercentralbenefitsPartnercentralbenefitsserviceState::replayed_application_write`]
//! and [`crate::state::IDEMPOTENT_PARAMETER_MISMATCH`]. The members compared are the
//! records the request NAMES rather than the spellings it used, and the token is looked up
//! wherever in the store it was spent — so a retry naming the same application by its ARN
//! instead of its id is that same request, and an update of ANOTHER application carrying
//! the token is not. The token is checked BEFORE the status and the revision, because the
//! first call's own write is what moved the revision the retry still carries.
//!
//! ## Shape of an operation
//!
//! Each operation is a struct that owns the shared engine and implements
//! [`smithy_server_runtime::Operation`]. The framework constructs one per service via
//! `UpdateBenefitApplication::new(engine)` and drives it through the centralized request walk:
//! deserialize → validate → `handle` → serialize. You write ONLY the `handle` body.
//!
//! ## Reaching this service's state
//!
//! The engine is held on `self` (constructor-injected), NOT passed per request — so
//! engine access is guaranteed by the time any request runs. Reach the store through
//! [`crate::state::service_state`] and read and write it through the guard it hands back:
//!
//! ```ignore
//!     let state = crate::state::service_state(&*self.engine)
//!         .ok_or(/* a modelled server error */)?;
//!     state.lock().your_field = /* ... */;
//! ```
//!
//! `ctx` ([`smithy_server_runtime::OperationContext`]) carries request metadata only —
//! headers, URI, method, and anything middleware placed in `ctx.parts().extensions`.
//! The engine is NOT reached through `ctx`.
//!
//! @generated by the emulator core scaffold op-stub generator.
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `UpdateBenefitApplication`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-partnercentralbenefits-partnercentralbenefitsservice/smithy-model/com-amazonaws-partnercentralbenefits-partnercentralbenefitsservice.json")
//! ```
//!
//! Updates an existing benefit application with new information while maintaining revision
//! control.
//!
//! ## Input
//!
//! * `BenefitApplicationDetails` (Document) — Updated detailed information and requirements
//!   specific to the benefit being requested.
//! * `Catalog` (CatalogName, pattern ^[A-Za-z0-9_-]+$) **required** — The catalog identifier
//!   that specifies which benefit catalog the application belongs to.
//! * `ClientToken` (String) **required** — A unique, case-sensitive identifier to ensure
//!   idempotent processing of the update request.
//! * `Description` (BenefitApplicationDescription, len 1..1000) — The updated detailed
//!   description of the benefit application.
//! * `FileDetails` (FileInputDetails) — Updated supporting documents and files attached to
//!   the benefit application.
//! * `Identifier` (BenefitApplicationIdentifier, pattern ^(arn:.+|benappl-[0-9a-z]{14})$)
//!   **required** — The unique identifier of the benefit application to update.
//! * `Name` (BenefitApplicationName, len 1..255) — The updated human-readable name for the
//!   benefit application.
//! * `PartnerContacts` (Contacts) — Updated contact information for partner representatives
//!   responsible for this benefit application.
//! * `Revision` (String) **required** — The current revision number of the benefit
//!   application to ensure optimistic concurrency control.
//!
//! ## Output
//!
//! * `Arn` (String) — The Amazon Resource Name (ARN) of the updated benefit application.
//! * `Id` (BenefitApplicationId, pattern ^benappl-[0-9a-z]{14}$) — The unique identifier of
//!   the updated benefit application.
//! * `Revision` (String) — The new revision number of the benefit application after the
//!   update.
//!
//! ## Errors
//!
//! * `AccessDeniedException` (HTTP 403) — Thrown when the caller does not have sufficient
//!   permissions to perform the requested operation.
//! * `ConflictException` (HTTP 409) — Thrown when the request conflicts with the current
//!   state of the resource, such as attempting to modify a resource that has been changed by
//!   another process.
//! * `InternalServerException` (HTTP 500) — Thrown when an unexpected error occurs on the
//!   server side during request processing.
//! * `ResourceNotFoundException` (HTTP 404) — Thrown when the requested resource cannot be
//!   found or does not exist.
//! * `ThrottlingException` (HTTP 429) — Thrown when the request rate exceeds the allowed
//!   limits and the request is being throttled.
//! * `ValidationException` (HTTP 400) — Thrown when the request contains invalid parameters
//!   or fails input validation requirements.

#[allow(unused_imports)]
use super::types::*;

/// The input members an update may apply, as the model spells them on the wire. Each is
/// recorded on the applied-write record of the application it changed, so a stored write
/// says which members that request wrote.
const MEMBER_NAME: &str = "Name";
const MEMBER_DESCRIPTION: &str = "Description";
const MEMBER_DETAILS: &str = "BenefitApplicationDetails";
const MEMBER_PARTNER_CONTACTS: &str = "PartnerContacts";
const MEMBER_FILE_DETAILS: &str = "FileDetails";

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `UpdateBenefitApplicationError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-partnercentralbenefits-partnercentralbenefitsservice.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `UpdateBenefitApplicationError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.partnercentralbenefits#UpdateBenefitApplication")]
#[derive(Debug)]
pub enum UpdateBenefitApplicationErrorModeled {
    AccessDeniedException(AccessDeniedException),
    ConflictException(ConflictException),
    InternalServerException(InternalServerException),
    ResourceNotFoundException(ResourceNotFoundException),
    ThrottlingException(ThrottlingException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::update_benefit_application::UpdateBenefitApplicationError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type UpdateBenefitApplicationError = crate::scaffolded::Scaffolded<UpdateBenefitApplicationErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<UpdateBenefitApplicationErrorModeled> for UpdateBenefitApplicationError {
    fn from(modeled: UpdateBenefitApplicationErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `UpdateBenefitApplication` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct UpdateBenefitApplication<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> UpdateBenefitApplication<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for UpdateBenefitApplication<Eng>
{
    type Input = UpdateBenefitApplicationInput;
    type Output = UpdateBenefitApplicationOutput;
    type Error = UpdateBenefitApplicationError;

    const NAME: &'static str = "UpdateBenefitApplication";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::UPDATE_BENEFIT_APPLICATION_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::UPDATE_BENEFIT_APPLICATION_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        // The catalog this write would land in is resolved FIRST, and the
        // partner-entitlement gate applies to a request whose catalog resolved — see
        // [`crate::state::write_request_refusal`]. The modelled constraints were refused
        // earlier still, by the centralized walk; the bounds this service puts on the
        // members the model leaves unconstrained are answered BELOW, once these two have
        // passed.
        match crate::state::write_request_refusal(&*self.engine, &input.catalog) {
            Some(crate::state::WriteRequestRefusal::MissingCatalog(missing)) => {
                return Err(
                    UpdateBenefitApplicationErrorModeled::ResourceNotFoundException(missing).into(),
                )
            }
            Some(crate::state::WriteRequestRefusal::NotEntitled(denied)) => {
                return Err(
                    UpdateBenefitApplicationErrorModeled::AccessDeniedException(denied).into(),
                )
            }
            None => {}
        }

        let Some(state) = crate::state::service_state(&*self.engine) else {
            return Err(UpdateBenefitApplicationErrorModeled::InternalServerException(
                InternalServerException {
                    message: crate::state::STORE_UNAVAILABLE.to_string(),
                },
            )
            .into());
        };

        // `BenefitApplicationDetails` is an unconstrained `Document`, so the bounds on a
        // document this update would store are this service's own, refused exactly as the
        // create refuses them — see [`crate::state::details_refusal`]. The `Phone` of a
        // supplied contact and the `BusinessUseCase` of a supplied attachment are
        // unconstrained strings this update also persists, so they carry this service's
        // own bound on the same rule, wherever the member is written — see
        // [`crate::state::unconstrained_members_refusal`]. The model states none of these
        // bounds, so AWS refuses such a value on nothing of its own: they are enforced
        // here, after the gate above, so a request the modelled service would answer with
        // its own refusal still gets that answer.
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

        let principal = crate::state::caller_principal(ctx);
        let now = crate::state::now_millis();

        let mut store = state.lock();

        // The catalog the request names is resolved FIRST: an application lives inside a
        // catalog, so a catalog that does not exist is the missing resource and is what
        // the answer names — see [`crate::state::catalog_refusal`].
        if let Some(missing) = store.catalog_refusal(&input.catalog) {
            return Err(
                UpdateBenefitApplicationErrorModeled::ResourceNotFoundException(missing).into(),
            );
        }

        // The application's own benefit governs the document this update would store, the
        // way it governs the one the create stores. The schema is read — and cloned —
        // BEFORE the application is borrowed for writing, and judged after the retry check
        // below and after the gates on the state of the record, so a replay of a write that
        // already landed still answers with what it reported and a record that admits no
        // content change is answered about its state.
        let benefit_schema =
            store.benefit_schema_for_application(&input.catalog, &input.identifier);

        // The identity of this REQUEST, which is what the `ClientToken` below is judged
        // against: every member the request carries other than the token itself. The
        // `Identifier` member is covered by the APPLICATION it names
        // ([`crate::state::ComAmazonawsPartnercentralbenefitsPartnercentralbenefitsserviceState::application_identity`]),
        // not by the spelling it was written with, so a retry naming the same application
        // the other way the modelled pattern admits is the same request.
        let fingerprint = crate::state::request_fingerprint(vec![
            (
                "Catalog",
                Some(crate::state::fingerprint_value(&input.catalog)),
            ),
            (
                "Identifier",
                Some(crate::state::fingerprint_value(
                    &store.application_identity(&input.catalog, &input.identifier),
                )),
            ),
            (
                "Revision",
                Some(crate::state::fingerprint_value(&input.revision)),
            ),
            (
                MEMBER_NAME,
                input.name.as_ref().map(crate::state::fingerprint_value),
            ),
            (
                MEMBER_DESCRIPTION,
                input
                    .description
                    .as_ref()
                    .map(crate::state::fingerprint_value),
            ),
            (
                MEMBER_DETAILS,
                input
                    .benefit_application_details
                    .as_ref()
                    .map(crate::state::fingerprint_value),
            ),
            (
                MEMBER_PARTNER_CONTACTS,
                input
                    .partner_contacts
                    .as_ref()
                    .map(crate::state::fingerprint_value),
            ),
            (
                MEMBER_FILE_DETAILS,
                input
                    .file_details
                    .as_ref()
                    .map(crate::state::fingerprint_value),
            ),
        ]);

        // A retry of an update that already applied applies nothing a second time and
        // answers with what that call reported: the `ClientToken` is the identity of the
        // REQUEST, so a request repeating one is the same request rather than a new
        // write — and a request repeating one with different members is a DIFFERENT
        // request, refused rather than answered with that write's result. The token is
        // looked up wherever in the store it was spent, so the application the request
        // names is a member it can differ on. It is checked BEFORE the status and the
        // revision, because the first call's own write moved the revision the retry still
        // carries (and the application may since have been submitted, which the status
        // gate below refuses an update of) — so without this a lost response turns into a
        // refusal of a write that already succeeded.
        match store.replayed_application_write(Self::NAME, &input.client_token, &fingerprint) {
            Err(conflict) => {
                return Err(UpdateBenefitApplicationErrorModeled::ConflictException(conflict).into())
            }
            Ok(Some(replayed)) => {
                return Ok(UpdateBenefitApplicationOutput {
                    id: Some(replayed.id),
                    arn: Some(replayed.arn),
                    revision: Some(replayed.revision.to_string()),
                })
            }
            Ok(None) => {}
        }

        // The application and the refusal a missing one is answered with are ONE
        // resolution — see [`crate::state::require_application_mut`].
        let application = store
            .require_application_mut(&input.catalog, &input.identifier)
            .map_err(UpdateBenefitApplicationErrorModeled::ResourceNotFoundException)?;

        // An update is the partner editing its own DRAFT: once the application has been
        // submitted the record is the one AWS is reviewing, and a terminal application is
        // finished with, so an update of anything but a `PENDING_SUBMISSION` application
        // is refused and NOTHING below runs — see [`crate::state::UPDATABLE_STATUSES`].
        // Checked before the revision and before the rules on the content this write would
        // store, because the status refuses the request whatever the request carries — the
        // order `crate::ops::amend_benefit_application` answers the same two rules in.
        if !application.is_updatable() {
            return Err(UpdateBenefitApplicationErrorModeled::ConflictException(
                ConflictException {
                    message: crate::state::status_conflict_message(
                        &application.id,
                        &application.status,
                        "updated",
                    ),
                },
            )
            .into());
        }

        if application.revision_text() != input.revision {
            return Err(UpdateBenefitApplicationErrorModeled::ConflictException(
                ConflictException {
                    message: crate::state::revision_conflict_message(
                        &application.id,
                        &application.revision_text(),
                        &input.revision,
                    ),
                },
            )
            .into());
        }

        // A document this update would store has to satisfy the benefit's request schema,
        // exactly as the create's does: one rule, applied wherever the member is written.
        // It judges the CONTENT of the write, so it runs after the gates on the state of
        // the record above — the place `crate::ops::amend_benefit_application` applies the
        // same rule to the document its amendments produced.
        if let Some(refusal) = crate::state::benefit_schema_refusal(
            benefit_schema.as_ref(),
            input.benefit_application_details.as_ref(),
        ) {
            return Err(UpdateBenefitApplicationErrorModeled::ValidationException(refusal).into());
        }

        // Only the members the request carried are applied, and only where the value the
        // request names is not the one the record already holds: an absent member leaves
        // the stored value alone, which is what lets a caller update one field without
        // having to resend the rest, and a member carrying the stored value writes
        // nothing. Each member this request CHANGED is named on the write record below.
        let mut applied_members: Vec<String> = Vec::new();
        if let Some(name) = input.name {
            if application.name.as_deref() != Some(name.as_str()) {
                application.name = Some(name);
                applied_members.push(MEMBER_NAME.to_string());
            }
        }
        if let Some(description) = input.description {
            if application.description.as_deref() != Some(description.as_str()) {
                application.description = Some(description);
                applied_members.push(MEMBER_DESCRIPTION.to_string());
            }
        }
        if let Some(details) = input.benefit_application_details {
            if application.benefit_application_details.as_ref() != Some(&details) {
                application.benefit_application_details = Some(details);
                applied_members.push(MEMBER_DETAILS.to_string());
            }
        }
        if let Some(contacts) = input.partner_contacts {
            if !crate::state::same_value(&application.partner_contacts, &contacts) {
                application.partner_contacts = contacts;
                applied_members.push(MEMBER_PARTNER_CONTACTS.to_string());
            }
        }
        if let Some(files) = input.file_details {
            // An attachment is the file the caller named, so a request re-sending the
            // files the application already carries attaches nothing a second time and
            // leaves the `CreatedBy`/`CreatedAt` the first attachment recorded — see
            // [`crate::state::same_file_details`].
            // A request that DID change the list stores the attachments it names, and
            // each file the application already carries keeps the `CreatedBy`/`CreatedAt`
            // the call that attached it recorded: those report who uploaded that file and
            // when, so adding, removing or reordering attachments leaves them alone and
            // only a file not already attached records this caller and this instant — see
            // [`crate::state::merged_file_details`].
            if !crate::state::same_file_details(&application.file_details, &files) {
                application.file_details = crate::state::merged_file_details(
                    &application.file_details,
                    &files,
                    &principal,
                    now,
                );
                applied_members.push(MEMBER_FILE_DETAILS.to_string());
            }
        }

        // The revision and `UpdatedAt` move only when the record actually changed, so an
        // update that applied nothing — one naming no member, or naming only values the
        // record already holds — reports the revision it already had and leaves the
        // idempotency window a preceding recall opened open (see
        // [`crate::state::BenefitApplication::revise`]).
        if !applied_members.is_empty() {
            application.revise(now);
        }

        // What this call reported, kept so a retry carrying the same `ClientToken`
        // answers it again. The token is at most
        // [`crate::state::MAX_CLIENT_TOKEN_LEN`] bytes — the modelled `@length(min: 1,
        // max: 64)` the centralized walk refuses a longer one against — so the record
        // the store keeps for it stays within the bound its own accounting cites.
        let revision = application.revision;
        let applied_at_millis = application.updated_at_millis;
        application.record_write(crate::state::AppliedWrite {
            operation: Self::NAME,
            client_token: input.client_token,
            request_fingerprint: fingerprint,
            // This operation carries no reason member, so the record states none.
            reason: None,
            fields: applied_members,
            applied_at_millis,
            revision,
        });

        Ok(UpdateBenefitApplicationOutput {
            id: Some(application.id.clone()),
            arn: Some(application.arn.clone()),
            revision: Some(revision.to_string()),
        })
    }
}
