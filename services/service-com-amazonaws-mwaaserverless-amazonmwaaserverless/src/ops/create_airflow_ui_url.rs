// SPDX-License-Identifier: CC0-1.0

//! @op CreateAirflowUiUrl
//!
//! ---
//!
//! # Modeled contract for `CreateAirflowUiUrl`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-mwaaserverless-amazonmwaaserverless/smithy-model/com-amazonaws-mwaaserverless-amazonmwaaserverless.json")
//! ```
//!
//! Creates a launch URL that opens the Apache Airflow UI for your workflows in Amazon
//! Managed Workflows for Apache Airflow Serverless. The UI shows the workflows in your
//! account in the current AWS Region. To start a UI session, open the URL in a browser
//! before it expires. The UI runs with the permissions of the IAM identity that you use to
//! call this operation. You see only the workflows, workflow runs, and logs that the
//! identity can read, and you can start a workflow run only if the identity has permission
//! to start one. The UI session uses the caller's permissions. The session works only from
//! the network address that opened the URL. It ends at SessionExpiresAt or when you sign
//! out from the UI.
//!
//! ## Input
//!
//! * `ExpiresInSeconds` (ExpiresInSecondsInteger, range 5..300) — Specifies the number of
//!   seconds that the launch URL stays valid. This value limits only the time to open the
//!   URL, not the length of the UI session. The default is 300.
//! * `SessionDurationInSeconds` (SessionDurationInSecondsInteger, range 1..43200) — Specifies
//!   the number of seconds that the UI session lasts, starting from when you call this
//!   operation. The default is 43200 (12 hours).
//!
//! ## Output
//!
//! * `AuthorizedUrl` (AuthorizedUrlString, len 1..2048) **required** — The launch URL that
//!   opens the Airflow UI. Open it in a browser before UrlExpiresAt. The UI session uses the
//!   permissions of the identity that called this operation.
//! * `SessionExpiresAt` (TimestampValue) **required** — The UI session ends at this time. The
//!   value is in ISO 8601 date-time format. After this time, create a new URL to open the UI
//!   again.
//! * `SessionId` (SessionIdString, len 32..32, pattern ^[a-z0-9]{32}$) **required** — The URL
//!   starts a UI session that has this identifier.
//! * `UrlExpiresAt` (TimestampValue) **required** — The launch URL stops being valid at this
//!   time. The value is in ISO 8601 date-time format.
//!
//! ## Errors
//!
//! * `AccessDeniedException` (HTTP 403) — You do not have sufficient permission to perform
//!   this action.
//! * `ConflictException` (HTTP 409) — You cannot create a resource that already exists, or
//!   the resource is in a state that prevents the requested operation.
//! * `InternalServerException` (HTTP 500) — An unexpected server-side error occurred during
//!   request processing.
//! * `OperationTimeoutException` (HTTP 504) — The operation timed out.
//! * `ThrottlingException` (HTTP 429) — The request was denied because too many requests were
//!   made in a short period, exceeding the service rate limits. Amazon Managed Workflows for
//!   Apache Airflow Serverless implements throttling controls to ensure fair resource
//!   allocation across all customers in the multi-tenant environment. This helps maintain
//!   service stability and performance. If you encounter throttling, implement exponential
//!   backoff and retry logic in your applications, or consider distributing your API calls
//!   over a longer time period.
//! * `ValidationException` (HTTP 400) — The specified request parameters are invalid,
//!   missing, or inconsistent with Amazon Managed Workflows for Apache Airflow Serverless
//!   service requirements. This can occur when workflow definitions contain unsupported
//!   operators, when required IAM permissions are missing, when S3 locations are
//!   inaccessible, or when network configurations are invalid. The service validates workflow
//!   definitions, execution roles, and resource configurations to ensure compatibility with
//!   the managed Airflow environment and security requirements.

#[allow(unused_imports)]
use super::types::*;

use ::std::time::Duration;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `CreateAirflowUiUrlError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-mwaaserverless-amazonmwaaserverless.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `CreateAirflowUiUrlError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.mwaaserverless#CreateAirflowUiUrl")]
#[derive(Debug)]
pub enum CreateAirflowUiUrlErrorModeled {
    AccessDeniedException(AccessDeniedException),
    ConflictException(ConflictException),
    InternalServerException(InternalServerException),
    OperationTimeoutException(OperationTimeoutException),
    ThrottlingException(ThrottlingException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::create_airflow_ui_url::CreateAirflowUiUrlError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type CreateAirflowUiUrlError = crate::scaffolded::Scaffolded<CreateAirflowUiUrlErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<CreateAirflowUiUrlErrorModeled> for CreateAirflowUiUrlError {
    fn from(modeled: CreateAirflowUiUrlErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `CreateAirflowUiUrl` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct CreateAirflowUiUrl<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> CreateAirflowUiUrl<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for CreateAirflowUiUrl<Eng>
{
    type Input = CreateAirflowUiUrlInput;
    type Output = CreateAirflowUiUrlOutput;
    type Error = CreateAirflowUiUrlError;

    const NAME: &'static str = "CreateAirflowUiUrl";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::CREATE_AIRFLOW_UI_URL_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::CREATE_AIRFLOW_UI_URL_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        // "The UI shows the workflows in your account in the current AWS Region", so the
        // session this call opens belongs to the endpoint the request was signed for, and
        // the launch URL addresses that same regional endpoint.
        let region = crate::state::request_region(ctx.parts());

        // How long the URL may be opened in, and how long the session it starts lasts.
        // Each member documents its own default for a request that names none
        // (`crate::state::airflow_ui_session`); the modelled `@range` of a supplied value
        // is refused at the request boundary, and it is read as an unsigned count of
        // seconds regardless, so no request-supplied integer can reach the signer or the
        // store as a nonsensical span.
        let url_validity = input.expires_in_seconds.map_or(
            crate::state::airflow_ui_session::DEFAULT_URL_VALIDITY,
            |seconds| Duration::from_secs(u64::from(seconds.unsigned_abs())),
        );
        let session_duration = input.session_duration_in_seconds.map_or(
            crate::state::airflow_ui_session::DEFAULT_SESSION_DURATION,
            |seconds| Duration::from_secs(u64::from(seconds.unsigned_abs())),
        );

        let state = crate::state::service_state(&*self.engine).ok_or_else(unavailable)?;

        // The session is MINTED here: this operation creates the session the URL starts,
        // so the identifier it reports names a record this store holds until that
        // session's own end instant. The instant is sampled under the guard, with every
        // change the clock already owed this store materialised first — including the
        // sessions whose end it has already passed.
        let session = {
            let mut store = state.lock();
            let now = store.advance_clock();
            let nonce = store.next_nonce(now);
            let session_id = crate::state::airflow_ui_session::mint_session_id(&region, &nonce);
            store.open_airflow_ui_session(session_id, &region, now, session_duration)
        };

        // The URL a caller opens later has to lead back to THIS emulator, so it is
        // query-signed by the emulator's own signer for exactly as long as it may be
        // opened: the signature is what stops being valid at `UrlExpiresAt`. The path
        // names the session the URL starts, so each session has its own address.
        let path = crate::state::airflow_ui_session::session_path(&session.session_id);
        let signed = ::smithy_server_runtime::SignedUrlRequest::new(
            ::http::Method::GET,
            crate::state::ARN_NAMESPACE,
            &path,
        )
        .region(&region);
        let url = ctx.url_signer().sign(signed, url_validity).map_err(|error| {
            crate::scaffolded::Scaffolded::validation(format!(
                "no launch URL can be issued for this request: {error}"
            ))
        })?;

        Ok(CreateAirflowUiUrlOutput {
            authorized_url: url.as_str().to_string(),
            session_id: session.session_id.clone(),
            // The instant the signature itself stops verifying, read off the signed URL
            // rather than recomputed, so what this reports is when the URL really stops
            // being openable.
            url_expires_at: crate::state::timestamp(url.expires_at()),
            // "The UI session ends at this time": the end instant the stored session
            // carries, so that session and this answer cannot come apart.
            session_expires_at: crate::state::timestamp(session.expires_at),
        })
    }
}

/// The modelled server error for a store this handler could not reach.
fn unavailable() -> CreateAirflowUiUrlError {
    CreateAirflowUiUrlErrorModeled::InternalServerException(InternalServerException {
        message: "the com.amazonaws.mwaaserverless workflow store is unavailable".to_string(),
        retry_after_seconds: None,
    })
    .into()
}
