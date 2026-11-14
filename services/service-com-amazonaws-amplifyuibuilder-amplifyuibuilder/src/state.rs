// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-amplifyuibuilder-amplifyuibuilder service.
//!
//! The component is the primary resource of the Amplify UI Builder control
//! plane: `CreateComponent` mints one, `GetComponent` / `ListComponents` report
//! it, `UpdateComponent` merges into it and `DeleteComponent` removes it. The
//! record below is the one copy of a component every one of those handlers
//! reads and writes, plus the two projections the model asks for (`Component`
//! and `ComponentSummary`).
//!
//! Beside the components, the store holds one app environment's UI-builder
//! metadata: the feature flags `PutMetadataFlag` writes and `GetMetadata`
//! reports.
//!
//! ## The codegen job, and the clock that moves its status
//!
//! `StartCodegenJob` mints the service's other resource, the CODEGEN JOB,
//! `GetCodegenJob` reports one and `ListCodegenJobs` reports a page of the
//! summaries of an app environment's jobs. The job AWS returns is `in_progress` — a
//! non-terminal status — so the service owes it a terminal status without
//! anybody calling again. That owed transition is held as a DUE INSTANT
//! ([`StoredCodegenJob::next_status_due`]) and materialised by
//! [`ComAmazonawsAmplifyuibuilderAmplifyuibuilderState::settle_codegen_jobs`],
//! which each codegen handler calls first: a read reports what the clock has
//! already made due and never advances a job because it was polled.
//!
//! Every handler that stamps a record — and every one that settles what the
//! clock has made due — reads the clock AFTER taking the store lock, never
//! before it. The instant a record keeps is then the instant observed while its
//! writer owned the store, so two concurrent writers stamp in the order they
//! commit and a record's [`StoredComponent::modified_at`] (or a theme's, or a
//! job's) cannot end up older than the value an already-committed write left.
//!
//! The form and the theme are the control plane's other app-environment
//! resources, held the same way: `CreateForm` mints a form, `UpdateForm` merges
//! into it, `GetForm` reports it from the single [`StoredForm`] record and
//! `DeleteForm` removes that record; `CreateTheme` mints a theme, `GetTheme`
//! reports it from the single [`StoredTheme`] record, `UpdateTheme` merges into
//! that record and `DeleteTheme` removes it, all keyed by the same
//! (appId, environmentName) a component is keyed by and all with the projection
//! the model asks for — `ListForms` reports a page of the `FormSummary`
//! projection and `ListThemes` a page of the `ThemeSummary` projection of those
//! same records.
//!
//! ## The tagging surface
//!
//! Every record of this service that holds a tag set — a component, a form, a
//! theme and a codegen job each carry the one their create or start stored — is
//! reached through the tagging surface, which holds no store of its own:
//! `ListTagsForResource` reports, and `TagResource` / `UntagResource` change, the
//! `tags` of the one record the resource's own reader answers from.
//! [`parse_resource_arn`] is what resolves a `resourceArn` to that record, and
//! [`resource_tags`] / [`with_resource_tags_mut`] are the one read and the one
//! write over it.
//!
//! ## The page walk
//!
//! Every paginated operation of this service — the component, form, theme and
//! codegen-job lists and the three exports — serves its page from [`page_of`],
//! the one walk over an app environment's records: it honours only a token
//! [`page_token`] issued for that app environment over the SAME collection
//! ([`PagedCollection`]), resumes at the cursor the token carries and issues the
//! token of the next page. An operation supplies only what is its own — the
//! records, the page bound and the projection it reports — so the resume
//! semantics and the refusal of an unknown token cannot drift between two of
//! them, and the collection a token belongs to is read off the record kind the
//! walk runs over rather than restated by each handler.
//!
//! ## The third-party token surface
//!
//! `RefreshToken` answers with an ACCESS TOKEN for a third-party provider, so the
//! store also holds one [`ProviderTokenSession`] per refresh token it has been
//! called with: the access token that is currently live for that session, and the
//! instant it stops being live. The access token is secret material, so it is
//! minted by the emulator's credential store ([`mint_provider_token`]) and
//! never constructed here, and the record is released by
//! [`ComAmazonawsAmplifyuibuilderAmplifyuibuilderState::expire_provider_token_sessions`]
//! once the clock has made that expiry due.
//!
//! `ExchangeCodeForToken` STARTS such a session: it mints the refresh token the
//! session is named by ([`mint_provider_token`] mints both) as well as the session's
//! first access token, and records the digest of the access code it was exchanged
//! from ([`ProviderTokenSession::code_digest`]), which is what makes that code
//! single-use while the session lives. There is ONE token store, so the session a
//! refresh reads is the session an exchange started.

use std::collections::{HashMap, VecDeque};
use std::time::{Duration, SystemTime};

use crate::ops::types::{
    CodegenFeatureFlags, CodegenJob, CodegenJobGenericDataSchema, CodegenJobRenderConfig,
    CodegenJobSummary, Component, ComponentBindingPropertiesValue, ComponentChild,
    ComponentDataConfiguration,
    ComponentEvent, ComponentProperty, ComponentSummary, ComponentVariant, FieldConfig, Form,
    FormCTA, FormDataTypeConfig, FormStyle, FormSummary, InvalidParameterException,
    ResourceConflictException, ResourceNotFoundException, SectionalElement, Theme, ThemeSummary,
    ThemeValues,
};

/// The largest page an entity list serves, and the page size it uses when the
/// request names no `maxResults`: that member targets `ListEntityLimit`, which
/// the model constrains `@range(min: 1, max: 100)`, on every operation that
/// pages over an app environment's entities (`ListComponents`, `ListForms`,
/// `ListThemes`).
pub const LIST_ENTITY_PAGE_LIMIT: i32 = 100;

/// How many records one export page carries, for every export this service
/// serves (`ExportComponents`, `ExportForms`, `ExportThemes`).
///
/// An export request models no page-size member (only `nextToken`), so the
/// length of an export page is the service's own choice rather than something a
/// caller asks for. This emulator serves the same largest page its component
/// list does, so a caller walking any of those operations walks pages of one
/// bound.
pub const EXPORT_PAGE_LIMIT: usize = LIST_ENTITY_PAGE_LIMIT as usize;

/// How many components the store holds before `CreateComponent` refuses with the
/// modelled `ServiceQuotaExceededException`.
///
/// Amplify UI Builder publishes no component quota this emulator can quote, so
/// this is the emulator's OWN bound on the store rather than a claim about any
/// account's limit. It is set far above any workflow an emulator serves, and it
/// is what stops the state growing without end: every record it holds is
/// released by `DeleteComponent`, which also forgets that component's client
/// tokens, so the token map can never outgrow this bound either.
pub const COMPONENT_QUOTA: usize = 1_000;

/// The feature names this service's metadata holds — the catalogue
/// `PutMetadataFlag` accepts and `GetMetadata` reports.
///
/// `FeaturesMap` is modelled as an open `String -> String` map and names no
/// feature (`smithy_query(query="shape",shape="FeaturesMap")`), so the catalogue
/// cannot be read off the model: it is the fixed set of UI-builder feature flags
/// `GetMetadata` reports for an app environment, which is what `PutMetadataFlag`
/// validates `featureName` against — any other name is refused rather than
/// stored.
pub const METADATA_FEATURES: [&str; 5] = [
    "autoGenerateForms",
    "autoGenerateViews",
    "isRelationshipSupported",
    "isNonModelSupported",
    "isGraphQLEnabled",
];

/// What `GetMetadata` reports for a feature no `PutMetadataFlag` has set.
///
/// Every feature in [`METADATA_FEATURES`] is a flag, and `PutMetadataFlagBody$newValue`
/// is the free-form string a caller sets it to, so an app environment nothing has
/// written reports every one of them off. This is the emulator's OWN starting
/// value — AWS publishes no default for these flags — and it moves only when a
/// `PutMetadataFlag` writes one.
pub const METADATA_FEATURE_DEFAULT: &str = "false";

/// How many app environments the metadata store holds flags for before
/// `PutMetadataFlag` refuses to start another one.
///
/// Amplify UI Builder publishes no quota on app environments this emulator can
/// quote, so — exactly like [`COMPONENT_QUOTA`] — this is the emulator's OWN
/// bound on the store rather than a claim about any account's limit, and it is
/// what stops the state growing without end: `appId` and `environmentName` are
/// request-supplied `@httpLabel` values, so without a bound every request naming
/// a fresh pair would add an entry nothing ever releases (the service models no
/// delete-metadata operation). It is set far above any workflow an emulator
/// serves. Each entry holds at most [`METADATA_FEATURES`]`.len()` values —
/// `PutMetadataFlag` refuses every other feature name — so the whole store is
/// bounded by this many entries times that many values, each value no longer
/// than the request that wrote it.
pub const METADATA_ENVIRONMENT_QUOTA: usize = 1_000;

/// How many forms the store holds before `CreateForm` refuses with the modelled
/// `ServiceQuotaExceededException`.
///
/// Amplify UI Builder publishes no form quota this emulator can quote, so — like
/// [`COMPONENT_QUOTA`] — this is the emulator's OWN bound on the store rather
/// than a claim about any account's limit, set far above any workflow an
/// emulator serves. Every record it bounds is released by `DeleteForm`, which
/// also forgets that form's client tokens, so neither the form map nor the token
/// map can outgrow this bound.
pub const FORM_QUOTA: usize = 1_000;

/// How many themes the store holds before `CreateTheme` refuses with the
/// modelled `ServiceQuotaExceededException`.
///
/// Amplify UI Builder publishes no theme quota this emulator can quote, so this
/// is the emulator's OWN bound on the store rather than a claim about any
/// account's limit. It is set far above any workflow an emulator serves, and it
/// is what stops the theme state — the records and the client tokens that name
/// them — growing without end.
pub const THEME_QUOTA: usize = 1_000;

/// The map key a component's property maps may not carry.
///
/// `Component$properties`, `$overrides`, `$bindingProperties` and
/// `$collectionProperties` each document "You can't specify `tags` as a valid
/// property" for that map, and so do the maps the payload nests:
/// `ComponentChild$properties` (at every depth of `children`) and
/// `ComponentVariant$variantValues` / `$overrides`. A request naming the key in
/// any of them is refused rather than stored.
pub const RESERVED_PROPERTY_NAME: &str = "tags";

/// The values `SectionalElement$type` admits, in the order the model lists them
/// ("The type of sectional element. Valid values are `Heading`, `Text`, and
/// `Divider`", `smithy_query(query="shape",shape="SectionalElement")`).
///
/// The member targets a plain `String` rather than an enum, so the valid set
/// cannot be read off the shape and the framework's validating walk cannot refuse
/// a value outside it — exactly the situation [`RESERVED_PROPERTY_NAME`] and
/// [`METADATA_FEATURES`] are held here for. A form write naming anything else is
/// refused rather than stored.
pub const SECTIONAL_ELEMENT_TYPES: [&str; 3] = ["Heading", "Text", "Divider"];

/// The values `SectionalElement$orientation` admits, from the same shape
/// ("Specifies the orientation for a `Divider` sectional element. Valid values are
/// `horizontal` or `vertical`"). A plain `String` in the model, so it is checked
/// here for the reason [`SECTIONAL_ELEMENT_TYPES`] is.
pub const SECTIONAL_ELEMENT_ORIENTATIONS: [&str; 2] = ["horizontal", "vertical"];

/// The smallest value `SectionalElement$level` admits ("Specifies the size of the
/// font for a `Heading` sectional element. Valid values are `1 | 2 | 3 | 4 | 5 |
/// 6`"). A plain `Integer` in the model, carrying neither `@range` nor an enum, so
/// it is checked here for the reason [`SECTIONAL_ELEMENT_TYPES`] is.
pub const SECTIONAL_ELEMENT_LEVEL_MIN: i32 = 1;

/// The largest value `SectionalElement$level` admits. See
/// [`SECTIONAL_ELEMENT_LEVEL_MIN`].
pub const SECTIONAL_ELEMENT_LEVEL_MAX: i32 = 6;

/// The `CodegenJobStatus` values the model declares, so no handler spells one as
/// a bare literal. The full set is `ops::types::CODEGEN_JOB_STATUS_VALUES`.
pub const CODEGEN_JOB_IN_PROGRESS: &str = "in_progress";
/// See [`CODEGEN_JOB_IN_PROGRESS`].
pub const CODEGEN_JOB_FAILED: &str = "failed";
/// See [`CODEGEN_JOB_IN_PROGRESS`].
pub const CODEGEN_JOB_SUCCEEDED: &str = "succeeded";

/// The statuses a codegen job never leaves. `in_progress` is the only transient
/// one, so a job in either of these owes the clock nothing.
pub const CODEGEN_JOB_TERMINAL_STATUSES: &[&str] = &[CODEGEN_JOB_FAILED, CODEGEN_JOB_SUCCEEDED];

/// How long after `StartCodegenJob` the clock owes the job its terminal status.
///
/// AWS documents no duration — how long a code generation run takes depends on
/// the app being rendered — so this span is CHOSEN, not modelled: 500 ms, inside
/// the project's 250-750 ms band. It is deliberately non-zero, so a caller
/// reading the job straight after starting it still observes `in_progress`.
/// Nothing sleeps for it: the due instant is stored and compared to the clock.
pub const CODEGEN_JOB_TRANSITION_DELAY: Duration = Duration::from_millis(500);

/// Why a codegen job this emulator accepts ends `failed`.
///
/// `StartCodegenJob` starts a RUN of the Amplify UI code generator, which renders
/// the app's components, forms and themes into React sources and publishes them
/// as the artifact `CodegenJob$asset.downloadUrl` points at. This emulator has no
/// such generator and no artifact store to serve a download from, so the run is
/// not performed: the job reaches the service's modelled failure status carrying
/// this reason, rather than reporting a success nothing computed.
pub const CODEGEN_JOB_NOT_RUN: &str = "this emulator does not run the Amplify UI code generator, \
     so no code was generated and no asset was published for this job";

/// How many codegen jobs the store holds before `StartCodegenJob` refuses.
///
/// A started job is reported for the life of the process — `GetCodegenJob` must
/// still answer what `StartCodegenJob` created, so nothing evicts one — and
/// starting a job is the only thing that adds one, so the store needs a maximum
/// entry count. Amplify UI Builder publishes no codegen-job quota this emulator
/// can quote, so this is the emulator's OWN bound, set far above any workflow an
/// emulator serves.
pub const CODEGEN_JOB_QUOTA: usize = 1_000;

/// The largest page `ListCodegenJobs` serves: its `maxResults` member targets
/// `ListCodegenJobsLimit`, which the model constrains `@range(min: 1, max: 100)`
/// and defaults to 100, so a request that names no page size asks for this many.
pub const CODEGEN_JOB_PAGE_LIMIT: i32 = 100;

// ---------------------------------------------------------------------------
// The access code an exchange is made from: how it is held, and how it is refused.
// ---------------------------------------------------------------------------

/// The address a secret a token request carries is held under: the hex SHA-256
/// digest of its text.
///
/// It is what lets the store recognise an access code it has already been given
/// without retaining the code itself, which the model marks `@sensitive`. `sha2`
/// does the hashing and `hex` the rendering, as for the ids this store mints.
fn token_digest(text: &str) -> String {
    use sha2::{Digest, Sha256};

    hex::encode(Sha256::digest(text.as_bytes()))
}

/// The message of the refusal of an exchange this service will not grant a token
/// for: the access code carries no characters, or it already started a session the
/// store still holds.
///
/// ONE fixed literal for the class, like [`BLANK_REFRESH_TOKEN_MESSAGE`] and the
/// other messages of this service, and it interpolates nothing: the code is
/// `@sensitive`, so a refusal must not quote the value it rejected, and an OAuth
/// 2.0 token endpoint tells a code that is unknown, blank or already spent apart no
/// further either (RFC 6749 §5.2).
pub const ACCESS_CODE_NOT_VALID_MESSAGE: &str = "The access code provided is not valid";

/// The `InvalidParameterException` payload an exchange answers for an access code
/// it will not grant a token for.
pub fn access_code_not_valid() -> InvalidParameterException {
    InvalidParameterException {
        message: Some(ACCESS_CODE_NOT_VALID_MESSAGE.to_string()),
    }
}

/// How a page token of this service is spelled, so a token none of its paginated
/// operations ever issued can be told apart from one that was issued.
const PAGE_TOKEN_PREFIX: &str = "c-";

/// The width of the cursor a page token carries, in hex digits (a `u64`).
const PAGE_TOKEN_HEX_WIDTH: usize = 16;

/// The width of the scope digest a page token carries, in hex digits (a `u32`).
const PAGE_TOKEN_SCOPE_WIDTH: usize = 8;

/// The lowest cursor this service ever hands out, and so the lowest one a page
/// token it issued can carry.
///
/// [`ComAmazonawsAmplifyuibuilderAmplifyuibuilderState::next_cursor`] draws from
/// a counter that starts at zero and pre-increments, so the first cursor any
/// record holds is this one and every smaller value — zero — belongs to no
/// record and to no token the service issued.
pub const FIRST_CURSOR: u64 = 1;

/// Which Amplify app and backend environment a component belongs to: the two
/// `@httpLabel` members that, with the id, name a component.
pub type EnvironmentKey = (String, String);

/// One Amplify UI Builder component, as this emulator persists it.
#[derive(Debug, Clone)]
pub struct StoredComponent {
    pub app_id: String,
    pub environment_name: String,
    /// `Component$id` targets `Uuid`; minted by
    /// [`ComAmazonawsAmplifyuibuilderAmplifyuibuilderState::mint_component_id`].
    pub id: String,
    pub name: String,
    pub component_type: String,
    pub properties: HashMap<String, ComponentProperty>,
    pub children: Option<Vec<ComponentChild>>,
    pub variants: Vec<ComponentVariant>,
    pub overrides: HashMap<String, HashMap<String, String>>,
    pub binding_properties: HashMap<String, ComponentBindingPropertiesValue>,
    pub collection_properties: Option<HashMap<String, ComponentDataConfiguration>>,
    pub events: Option<HashMap<String, ComponentEvent>>,
    pub source_id: Option<String>,
    pub schema_version: Option<String>,
    /// `Component$tags` is reported for every component, so the tags a create
    /// supplied are held as a map rather than as an absence: a component created
    /// with no tags carries an empty one and reports `{}`.
    pub tags: HashMap<String, String>,
    pub created_at: SystemTime,
    /// When the component last changed. Creating it is the change that first set
    /// it, so it starts at [`StoredComponent::created_at`] and is moved by every
    /// `UpdateComponent` that merges into the record.
    pub modified_at: SystemTime,
    /// The position this component holds in its environment's creation order.
    ///
    /// It is what a `ListComponents` page token indexes into: it never changes
    /// and never repeats, so a page resumes at the same place whether or not
    /// components were created or deleted since the previous page.
    pub cursor: u64,
}

impl StoredComponent {
    /// The `Component` the create, get and update operations return.
    pub fn to_component(&self) -> Component {
        Component {
            app_id: self.app_id.clone(),
            environment_name: self.environment_name.clone(),
            source_id: self.source_id.clone(),
            id: self.id.clone(),
            name: self.name.clone(),
            component_type: self.component_type.clone(),
            properties: self.properties.clone(),
            children: self.children.clone(),
            variants: self.variants.clone(),
            overrides: self.overrides.clone(),
            binding_properties: self.binding_properties.clone(),
            collection_properties: self.collection_properties.clone(),
            created_at: ::aws_smithy_types::DateTime::from(self.created_at),
            modified_at: Some(::aws_smithy_types::DateTime::from(self.modified_at)),
            tags: Some(self.tags.clone()),
            events: self.events.clone(),
            schema_version: self.schema_version.clone(),
        }
    }

    /// The `ComponentSummary` projection `ListComponents` returns.
    pub fn to_summary(&self) -> ComponentSummary {
        ComponentSummary {
            app_id: self.app_id.clone(),
            environment_name: self.environment_name.clone(),
            id: self.id.clone(),
            name: self.name.clone(),
            component_type: self.component_type.clone(),
        }
    }
}

/// One Amplify UI Builder code generation job, as this emulator persists it.
///
/// Every member a modelled response carries is read from here; nothing in a
/// response is recomputed from the request.
#[derive(Debug, Clone)]
pub struct StoredCodegenJob {
    pub app_id: String,
    pub environment_name: String,
    /// `CodegenJob$id` targets `Uuid`; minted by
    /// [`ComAmazonawsAmplifyuibuilderAmplifyuibuilderState::mint_codegen_job_id`].
    pub id: String,
    /// The job configuration `StartCodegenJobData` supplied, held as given:
    /// `CodegenJob` reports each of these back as `create-and-read` members.
    pub render_config: CodegenJobRenderConfig,
    pub generic_data_schema: Option<CodegenJobGenericDataSchema>,
    pub auto_generate_forms: Option<bool>,
    pub features: Option<CodegenFeatureFlags>,
    /// `CodegenJob$tags` is reported for every job, so the tags a start supplied
    /// are held as a map rather than as an absence: a job started with no tags
    /// carries an empty one and reports `{}`.
    pub tags: HashMap<String, String>,
    /// One of [`CODEGEN_JOB_IN_PROGRESS`] or [`CODEGEN_JOB_TERMINAL_STATUSES`].
    pub status: String,
    pub status_message: Option<String>,
    pub created_at: SystemTime,
    /// When the job last changed. Starting it is the change that first set it, so
    /// it begins at [`StoredCodegenJob::created_at`] and moves to the instant the
    /// status transition below was due.
    pub modified_at: SystemTime,
    /// The status the clock owes this job, and the instant it is owed from.
    /// `None` for a job in a terminal status, which owes nothing further.
    pub next_status_due: Option<(SystemTime, &'static str)>,
    /// The position this job holds in its environment's start order.
    ///
    /// It is what a `ListCodegenJobs` page token indexes into: drawn from the
    /// same counter [`StoredComponent::cursor`] is drawn from, it never changes
    /// and never repeats, so a page resumes at the same place whatever was
    /// started since the previous page.
    pub cursor: u64,
}

impl StoredCodegenJob {
    /// Whether this job has reached a status it never leaves.
    pub fn is_terminal(&self) -> bool {
        CODEGEN_JOB_TERMINAL_STATUSES.contains(&self.status.as_str())
    }

    /// Apply the owed status transition when the clock has already made it due.
    ///
    /// Only a change that was owed at `now` whether or not anybody called is
    /// applied here, so calling this from a reporting operation does not move a
    /// job the clock has not moved yet.
    fn settle(&mut self, now: SystemTime) {
        let Some((due, next)) = self.next_status_due else {
            return;
        };
        if now < due {
            return;
        }
        self.status = next.to_string();
        // The reason is DERIVED from the status reached, so the two can never
        // disagree: `failed` says why the run did not happen, and a job that
        // reached any other status carries no failure reason.
        self.status_message = (next == CODEGEN_JOB_FAILED).then(|| CODEGEN_JOB_NOT_RUN.to_string());
        self.next_status_due = None;
        // The transition is the change, and it was due at `due` — not at the
        // instant somebody happened to read the job.
        self.modified_at = due;
        debug_assert!(
            self.is_terminal(),
            "a settled codegen job holds a terminal status, not {:?}",
            self.status
        );
    }

    /// The `CodegenJob` `StartCodegenJob` and `GetCodegenJob` return.
    pub fn to_codegen_job(&self) -> CodegenJob {
        CodegenJob {
            id: self.id.clone(),
            app_id: self.app_id.clone(),
            environment_name: self.environment_name.clone(),
            render_config: Some(self.render_config.clone()),
            generic_data_schema: self.generic_data_schema.clone(),
            auto_generate_forms: self.auto_generate_forms,
            features: self.features.clone(),
            status: Some(self.status.clone()),
            status_message: self.status_message.clone(),
            // `asset` carries the download URL of the artifact a code generation
            // RUN publishes, and `dependencies` the packages that run found the
            // generated code needs. This emulator performs no such run (see
            // [`CODEGEN_JOB_NOT_RUN`]), so both members are ABSENT rather than
            // carrying a URL nothing serves or a dependency list nothing computed.
            asset: None,
            tags: Some(self.tags.clone()),
            created_at: Some(::aws_smithy_types::DateTime::from(self.created_at)),
            modified_at: Some(::aws_smithy_types::DateTime::from(self.modified_at)),
            dependencies: None,
        }
    }

    /// The `CodegenJobSummary` projection `ListCodegenJobs` returns.
    ///
    /// `CodegenJobSummary` models appId, environmentName, id, createdAt and
    /// modifiedAt (`smithy_query(query="shape", shape="CodegenJobSummary")`) and
    /// no status, so the summary is the same record's identity and timestamps —
    /// nothing in it is recomputed.
    pub fn to_summary(&self) -> CodegenJobSummary {
        CodegenJobSummary {
            app_id: self.app_id.clone(),
            environment_name: self.environment_name.clone(),
            id: self.id.clone(),
            created_at: Some(::aws_smithy_types::DateTime::from(self.created_at)),
            modified_at: Some(::aws_smithy_types::DateTime::from(self.modified_at)),
        }
    }
}

/// One Amplify UI Builder form, as this emulator persists it.
///
/// `Form` models no timestamps (`smithy_query(query="shape", shape="Form")`
/// lists appId, environmentName, id, name, formActionType, style, dataType,
/// fields, sectionalElements, schemaVersion, tags, cta and labelDecorator), so
/// none is held: every field below is one the model reports.
#[derive(Debug, Clone)]
pub struct StoredForm {
    pub app_id: String,
    pub environment_name: String,
    /// `Form$id` targets `Uuid`; minted by
    /// [`ComAmazonawsAmplifyuibuilderAmplifyuibuilderState::mint_form_id`].
    pub id: String,
    pub name: String,
    pub form_action_type: String,
    pub style: FormStyle,
    pub data_type: FormDataTypeConfig,
    pub fields: HashMap<String, FieldConfig>,
    pub sectional_elements: HashMap<String, SectionalElement>,
    pub schema_version: String,
    /// `Form$tags` is reported for every form, so the tags a create supplied are
    /// held as a map rather than as an absence: a form created with no tags
    /// carries an empty one and reports `{}`, exactly as a component, a theme and
    /// a codegen job do.
    pub tags: HashMap<String, String>,
    pub cta: Option<FormCTA>,
    pub label_decorator: Option<String>,
    /// The position this form holds in its environment's creation order.
    ///
    /// It is what a `ListForms` or an `ExportForms` page token indexes into:
    /// drawn from the same
    /// counter [`StoredComponent::cursor`] is drawn from, it never changes and
    /// never repeats, so a page resumes at the same place whether or not forms
    /// were created or deleted since the previous page.
    pub cursor: u64,
}

impl StoredForm {
    /// The `Form` the create, update and get operations return.
    pub fn to_form(&self) -> Form {
        Form {
            app_id: self.app_id.clone(),
            environment_name: self.environment_name.clone(),
            id: self.id.clone(),
            name: self.name.clone(),
            form_action_type: self.form_action_type.clone(),
            style: self.style.clone(),
            data_type: self.data_type.clone(),
            fields: self.fields.clone(),
            sectional_elements: self.sectional_elements.clone(),
            schema_version: self.schema_version.clone(),
            tags: Some(self.tags.clone()),
            cta: self.cta.clone(),
            label_decorator: self.label_decorator.clone(),
        }
    }

    /// The `FormSummary` projection `ListForms` returns: the six members the
    /// model declares on it, read off the one stored record.
    pub fn to_summary(&self) -> FormSummary {
        FormSummary {
            app_id: self.app_id.clone(),
            data_type: self.data_type.clone(),
            environment_name: self.environment_name.clone(),
            form_action_type: self.form_action_type.clone(),
            id: self.id.clone(),
            name: self.name.clone(),
        }
    }
}

/// One Amplify UI Builder theme, as this emulator persists it.
#[derive(Debug, Clone)]
pub struct StoredTheme {
    pub app_id: String,
    pub environment_name: String,
    /// `Theme$id` targets `Uuid`; minted by
    /// [`ComAmazonawsAmplifyuibuilderAmplifyuibuilderState::mint_theme_id`].
    pub id: String,
    pub name: String,
    /// `Theme$values` is `@required`: the list the create supplied, in the order
    /// it supplied it, nested `ThemeValue$children` and all.
    pub values: Vec<ThemeValues>,
    /// `Theme$overrides` is optional, so an absent one stays absent rather than
    /// being reported as an empty list.
    pub overrides: Option<Vec<ThemeValues>>,
    /// `Theme$tags` is reported for every theme, so the tags a create supplied
    /// are held as a map rather than as an absence: a theme created with no tags
    /// carries an empty one and reports `{}`, exactly as a component does.
    pub tags: HashMap<String, String>,
    pub created_at: SystemTime,
    /// When the theme last changed. Creating it is the change that first set it,
    /// so it starts at [`StoredTheme::created_at`].
    pub modified_at: SystemTime,
    /// The position this theme holds in its environment's creation order, drawn
    /// from the same counter a component's [`StoredComponent::cursor`] is.
    ///
    /// It is what an `ExportThemes` page token indexes into: it never changes
    /// and never repeats, so a page resumes at the same place whether or not
    /// themes were created or deleted since the previous page.
    pub cursor: u64,
}

impl StoredTheme {
    /// The `ThemeSummary` projection `ListThemes` returns: the four members the
    /// model declares on it, read off the one stored record.
    pub fn to_summary(&self) -> ThemeSummary {
        ThemeSummary {
            app_id: self.app_id.clone(),
            environment_name: self.environment_name.clone(),
            id: self.id.clone(),
            name: self.name.clone(),
        }
    }

    /// The `Theme` the create and get operations return.
    pub fn to_theme(&self) -> Theme {
        Theme {
            app_id: self.app_id.clone(),
            environment_name: self.environment_name.clone(),
            id: self.id.clone(),
            name: self.name.clone(),
            created_at: ::aws_smithy_types::DateTime::from(self.created_at),
            modified_at: Some(::aws_smithy_types::DateTime::from(self.modified_at)),
            values: self.values.clone(),
            overrides: self.overrides.clone(),
            tags: Some(self.tags.clone()),
        }
    }
}

/// Per-service state for com-amazonaws-amplifyuibuilder-amplifyuibuilder. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsAmplifyuibuilderAmplifyuibuilderState {
    /// Every component, keyed by the (appId, environmentName) it belongs to and
    /// held in creation order within that environment — the stable total order
    /// `ListComponents` pages over.
    pub components: HashMap<EnvironmentKey, Vec<StoredComponent>>,
    /// A `CreateComponent` `clientToken` to the component that token created,
    /// so a retried create reports the first component instead of minting a
    /// second. The key carries the (appId, environmentName) the create
    /// addressed: a token is scoped to the environment it was used in, so
    /// reusing one against another app environment creates that environment's
    /// own component rather than reporting a component the request does not
    /// address.
    pub component_tokens: HashMap<(EnvironmentKey, String), String>,
    /// The UI-builder metadata flags written for one app environment, keyed by
    /// the (appId, environmentName) the `PutMetadataFlag` addressed: the feature
    /// name to the value it was last set to. A feature nothing has written is
    /// absent here and reported as [`METADATA_FEATURE_DEFAULT`], so the map holds
    /// only what a caller actually stored, and how many app environments it holds
    /// at all is bounded by [`METADATA_ENVIRONMENT_QUOTA`].
    pub metadata_flags: HashMap<EnvironmentKey, HashMap<String, String>>,
    /// Every codegen job, keyed by the (appId, environmentName) it belongs to and
    /// held in start order within that environment.
    pub codegen_jobs: HashMap<EnvironmentKey, Vec<StoredCodegenJob>>,
    /// A `StartCodegenJob` `clientToken` to the job that token started, so a
    /// retried start reports the first job instead of starting a second. Scoped
    /// to the (appId, environmentName) the start addressed, exactly as
    /// [`ComAmazonawsAmplifyuibuilderAmplifyuibuilderState::component_tokens`] is.
    pub codegen_job_tokens: HashMap<(EnvironmentKey, String), String>,
    /// Every form, keyed by the (appId, environmentName) it belongs to and held
    /// in creation order within that environment.
    pub forms: HashMap<EnvironmentKey, Vec<StoredForm>>,
    /// A `CreateForm` `clientToken` to the form that token created, so a retried
    /// create reports the first form instead of minting a second. As for a
    /// component, the key carries the (appId, environmentName) the create
    /// addressed: a token is scoped to the environment it was used in.
    pub form_tokens: HashMap<(EnvironmentKey, String), String>,
    /// The counter [`StoredComponent::cursor`] and [`StoredTheme::cursor`]
    /// values are drawn from.
    pub cursor_seq: u64,
    /// Every theme, keyed by the (appId, environmentName) it belongs to and held
    /// in creation order within that environment.
    pub themes: HashMap<EnvironmentKey, Vec<StoredTheme>>,
    /// A `CreateTheme` `clientToken` to the theme that token created, so a
    /// retried create reports the first theme instead of minting a second. As
    /// for a component, the key carries the (appId, environmentName) the create
    /// addressed: a token is scoped to the environment it was used in.
    pub theme_tokens: HashMap<(EnvironmentKey, String), String>,
    /// The LIVE third-party access token of each token session a token operation
    /// has answered — the session an exchange STARTED as well as the one a
    /// refresh answered — keyed by the [`ProviderTokenKey`] naming it.
    ///
    /// One entry per session, REPLACED rather than added to when the same
    /// session is refreshed again, and dropped once its access token has run out
    /// ([`Self::expire_provider_token_sessions`]), so the map holds only tokens
    /// that are still live and never more than
    /// [`PROVIDER_TOKEN_SESSION_QUOTA`] of them.
    pub provider_token_sessions: HashMap<ProviderTokenKey, ProviderTokenSession>,
    /// The seed this store's resource ids are minted from, drawn from the
    /// operating system the first time one is minted.
    id_seed: Option<u64>,
    /// The counter that keeps two ids of one run apart.
    id_seq: u64,
}

impl ComAmazonawsAmplifyuibuilderAmplifyuibuilderState {
    /// The components stored for one app environment, in creation order.
    pub fn components_in(&self, app_id: &str, environment_name: &str) -> &[StoredComponent] {
        self.components
            .get(&(app_id.to_string(), environment_name.to_string()))
            .map_or(&[], Vec::as_slice)
    }

    /// The component `id` names in one app environment.
    pub fn component(
        &self,
        app_id: &str,
        environment_name: &str,
        id: &str,
    ) -> Option<&StoredComponent> {
        self.components_in(app_id, environment_name)
            .iter()
            .find(|component| component.id == id)
    }

    /// Whether `id` names a component some OTHER app environment owns.
    ///
    /// A component is addressed by (appId, environmentName, id), so an id that
    /// resolves outside the app environment the request named is a request made
    /// in the wrong scope — told apart here from an id that names no component at
    /// all, which is simply absent.
    pub fn component_in_other_scope(
        &self,
        app_id: &str,
        environment_name: &str,
        id: &str,
    ) -> bool {
        id_in_other_scope(&self.components, app_id, environment_name, id, |component| {
            &component.id
        })
    }

    /// The live component called `name` in one app environment, if there is one.
    ///
    /// A component name identifies a component within its app environment, so
    /// this is what a create (or a rename) consults before taking a name.
    pub fn component_named(
        &self,
        app_id: &str,
        environment_name: &str,
        name: &str,
    ) -> Option<&StoredComponent> {
        record_named(
            self.components_in(app_id, environment_name),
            name,
            |component| &component.name,
        )
    }

    /// The component `id` names in one app environment, for a caller that
    /// changes it.
    pub fn component_mut(
        &mut self,
        app_id: &str,
        environment_name: &str,
        id: &str,
    ) -> Option<&mut StoredComponent> {
        self.components
            .get_mut(&(app_id.to_string(), environment_name.to_string()))?
            .iter_mut()
            .find(|component| component.id == id)
    }

    /// Store a component at the end of its environment's creation order.
    pub fn insert_component(&mut self, component: StoredComponent) {
        self.components
            .entry((
                component.app_id.clone(),
                component.environment_name.clone(),
            ))
            .or_default()
            .push(component);
    }

    /// Remove the component `id` names, and forget every client token that named
    /// it so the token map cannot outlive the record.
    pub fn remove_component(
        &mut self,
        app_id: &str,
        environment_name: &str,
        id: &str,
    ) -> Option<StoredComponent> {
        let key = (app_id.to_string(), environment_name.to_string());
        let components = self.components.get_mut(&key)?;
        let index = components
            .iter()
            .position(|component| component.id == id)?;
        let removed = components.remove(index);
        if components.is_empty() {
            self.components.remove(&key);
        }
        self.component_tokens
            .retain(|(token_key, _), token_id| !(*token_key == key && token_id == id));
        Some(removed)
    }

    /// The component one `clientToken` already created in the app environment
    /// the create addressed, if that token has been seen there.
    pub fn replayed_component(&self, key: &EnvironmentKey, token: &str) -> Option<&StoredComponent> {
        let id = self
            .component_tokens
            .get(&(key.clone(), token.to_string()))?;
        self.component(&key.0, &key.1, id)
    }

    /// Remember that `token` created the component `id` in that app environment.
    pub fn remember_token(&mut self, key: EnvironmentKey, token: String, id: String) {
        self.component_tokens.insert((key, token), id);
    }

    /// The metadata features of one app environment: every feature in
    /// [`METADATA_FEATURES`] with the value a `PutMetadataFlag` last wrote for it,
    /// or [`METADATA_FEATURE_DEFAULT`] for one nothing has written.
    pub fn metadata_features(
        &self,
        app_id: &str,
        environment_name: &str,
    ) -> HashMap<String, String> {
        let written = self
            .metadata_flags
            .get(&(app_id.to_string(), environment_name.to_string()));
        METADATA_FEATURES
            .iter()
            .map(|feature| {
                let value = written
                    .and_then(|flags| flags.get(*feature))
                    .cloned()
                    .unwrap_or_else(|| METADATA_FEATURE_DEFAULT.to_string());
                ((*feature).to_string(), value)
            })
            .collect()
    }

    /// Store the value of one metadata feature of an app environment.
    ///
    /// Writing a feature of an app environment the store does not hold yet starts
    /// an entry, which is what [`METADATA_ENVIRONMENT_QUOTA`] bounds: a caller
    /// checks [`Self::can_hold_metadata_for`] first and refuses past the bound
    /// rather than growing the store without end.
    pub fn set_metadata_flag(&mut self, key: EnvironmentKey, feature: String, value: String) {
        self.metadata_flags
            .entry(key)
            .or_default()
            .insert(feature, value);
    }

    /// How many app environments the metadata store holds flags for — what
    /// [`METADATA_ENVIRONMENT_QUOTA`] bounds.
    pub fn metadata_environments(&self) -> usize {
        self.metadata_flags.len()
    }

    /// Whether a metadata write to `key` is within the store's bound: an app
    /// environment the store already holds always is (the write replaces a value
    /// in an existing entry, and an entry holds at most the catalogue), and a new
    /// one only while the store is below [`METADATA_ENVIRONMENT_QUOTA`].
    pub fn can_hold_metadata_for(&self, key: &EnvironmentKey) -> bool {
        self.metadata_flags.contains_key(key)
            || self.metadata_environments() < METADATA_ENVIRONMENT_QUOTA
    }

    /// How many components the whole store holds — what [`COMPONENT_QUOTA`]
    /// bounds.
    pub fn total_components(&self) -> usize {
        self.components.values().map(Vec::len).sum()
    }

    /// The next cursor in this service's one creation order.
    ///
    /// Saturating rather than wrapping so the "never repeats" invariant above
    /// holds for every value it can actually reach: exhausting a `u64` would
    /// take 2^64 creates, which no emulator run performs.
    ///
    /// The counter pre-increments, so the first cursor handed out is
    /// [`FIRST_CURSOR`]: the cursors in existence always run from there up to
    /// `cursor_seq`, which is the span [`parse_page_token`] admits.
    pub fn next_cursor(&mut self) -> u64 {
        self.cursor_seq = self.cursor_seq.saturating_add(1);
        self.cursor_seq
    }

    /// The forms stored for one app environment, in creation order.
    pub fn forms_in(&self, app_id: &str, environment_name: &str) -> &[StoredForm] {
        self.forms
            .get(&(app_id.to_string(), environment_name.to_string()))
            .map_or(&[], Vec::as_slice)
    }

    /// The form `id` names in one app environment.
    pub fn form(&self, app_id: &str, environment_name: &str, id: &str) -> Option<&StoredForm> {
        self.forms_in(app_id, environment_name)
            .iter()
            .find(|form| form.id == id)
    }

    /// The form `id` names in one app environment, for a caller that changes it.
    ///
    /// `UpdateForm` merges into this one record, so the update and the reads
    /// report the same copy of the form rather than two.
    pub fn form_mut(
        &mut self,
        app_id: &str,
        environment_name: &str,
        id: &str,
    ) -> Option<&mut StoredForm> {
        self.forms
            .get_mut(&(app_id.to_string(), environment_name.to_string()))?
            .iter_mut()
            .find(|form| form.id == id)
    }

    /// Store a form at the end of its environment's creation order.
    pub fn insert_form(&mut self, form: StoredForm) {
        self.forms
            .entry((form.app_id.clone(), form.environment_name.clone()))
            .or_default()
            .push(form);
    }

    /// The form one `clientToken` already created in the app environment the
    /// create addressed, if that token has been seen there.
    pub fn replayed_form(&self, key: &EnvironmentKey, token: &str) -> Option<&StoredForm> {
        let id = self.form_tokens.get(&(key.clone(), token.to_string()))?;
        self.form(&key.0, &key.1, id)
    }

    /// Remember that `token` created the form `id` in that app environment.
    pub fn remember_form_token(&mut self, key: EnvironmentKey, token: String, id: String) {
        self.form_tokens.insert((key, token), id);
    }

    /// Whether `id` names a form some OTHER app environment owns.
    ///
    /// A form is addressed by (appId, environmentName, id) exactly as a component
    /// is, so an id that resolves outside the app environment the request named is
    /// a request made in the wrong scope — told apart here from an id that names no
    /// form at all, which is simply absent. The component counterpart is
    /// [`Self::component_in_other_scope`].
    pub fn form_in_other_scope(&self, app_id: &str, environment_name: &str, id: &str) -> bool {
        id_in_other_scope(&self.forms, app_id, environment_name, id, |form| &form.id)
    }

    /// Remove the form `id` names from the app environment the request addressed,
    /// and forget every client token that named it so the token map cannot outlive
    /// the record (the same bookkeeping [`Self::remove_component`] does).
    pub fn remove_form(
        &mut self,
        app_id: &str,
        environment_name: &str,
        id: &str,
    ) -> Option<StoredForm> {
        let key = (app_id.to_string(), environment_name.to_string());
        let forms = self.forms.get_mut(&key)?;
        let index = forms.iter().position(|form| form.id == id)?;
        let removed = forms.remove(index);
        if forms.is_empty() {
            self.forms.remove(&key);
        }
        self.form_tokens
            .retain(|(token_key, _), token_id| !(*token_key == key && token_id == id));
        Some(removed)
    }

    /// The live form called `name` in one app environment, if there is one.
    ///
    /// A form name identifies a form within its app environment exactly as a
    /// component name identifies a component, so this is what a create (or a
    /// rename) consults before taking a name; the component counterpart is
    /// [`Self::component_named`].
    pub fn form_named(
        &self,
        app_id: &str,
        environment_name: &str,
        name: &str,
    ) -> Option<&StoredForm> {
        record_named(self.forms_in(app_id, environment_name), name, |form| {
            &form.name
        })
    }

    /// How many forms the whole store holds — what [`FORM_QUOTA`] bounds.
    pub fn total_forms(&self) -> usize {
        self.forms.values().map(Vec::len).sum()
    }

    /// Mint an unused component id. `Component$id` targets `Uuid`, so the id is
    /// laid out as a version 4 UUID; the loop skips an id already taken in that
    /// environment.
    pub fn mint_component_id(&mut self, key: &EnvironmentKey) -> String {
        loop {
            let id = self.next_uuid();
            if self
                .components
                .get(key)
                .is_none_or(|components| components.iter().all(|c| c.id != id))
            {
                return id;
            }
        }
    }

    /// Materialise every codegen-job status transition the clock has already made
    /// due. Call this FIRST in every codegen handler, including a read: it only
    /// applies a change that was owed at `now` whether or not anybody called.
    pub fn settle_codegen_jobs(&mut self, now: SystemTime) {
        for jobs in self.codegen_jobs.values_mut() {
            for job in jobs.iter_mut() {
                job.settle(now);
            }
        }
    }

    /// The codegen jobs stored for one app environment, in start order.
    pub fn codegen_jobs_in(&self, app_id: &str, environment_name: &str) -> &[StoredCodegenJob] {
        self.codegen_jobs
            .get(&(app_id.to_string(), environment_name.to_string()))
            .map_or(&[], Vec::as_slice)
    }

    /// The codegen job `id` names in one app environment.
    pub fn codegen_job(
        &self,
        app_id: &str,
        environment_name: &str,
        id: &str,
    ) -> Option<&StoredCodegenJob> {
        self.codegen_jobs_in(app_id, environment_name)
            .iter()
            .find(|job| job.id == id)
    }

    /// The codegen job `id` names in one app environment, for a caller that
    /// changes it — the counterpart of [`Self::component_mut`], [`Self::form_mut`]
    /// and [`Self::theme_mut`].
    pub fn codegen_job_mut(
        &mut self,
        app_id: &str,
        environment_name: &str,
        id: &str,
    ) -> Option<&mut StoredCodegenJob> {
        self.codegen_jobs
            .get_mut(&(app_id.to_string(), environment_name.to_string()))?
            .iter_mut()
            .find(|job| job.id == id)
    }

    /// Whether `id` names a codegen job some OTHER app environment owns.
    ///
    /// A codegen job is addressed by (appId, environmentName, id) exactly as a
    /// component and a form are, so an id that resolves outside the app
    /// environment the request named is a request made in the wrong scope — told
    /// apart here from an id that names no job at all, which is simply absent.
    /// The counterparts are [`Self::component_in_other_scope`] and
    /// [`Self::form_in_other_scope`].
    pub fn codegen_job_in_other_scope(
        &self,
        app_id: &str,
        environment_name: &str,
        id: &str,
    ) -> bool {
        id_in_other_scope(&self.codegen_jobs, app_id, environment_name, id, |job| {
            &job.id
        })
    }

    /// Store a codegen job at the end of its environment's start order.
    pub fn insert_codegen_job(&mut self, job: StoredCodegenJob) {
        self.codegen_jobs
            .entry((job.app_id.clone(), job.environment_name.clone()))
            .or_default()
            .push(job);
    }

    /// The codegen job one `clientToken` already started in the app environment
    /// the start addressed, if that token has been seen there.
    pub fn replayed_codegen_job(
        &self,
        key: &EnvironmentKey,
        token: &str,
    ) -> Option<&StoredCodegenJob> {
        let id = self
            .codegen_job_tokens
            .get(&(key.clone(), token.to_string()))?;
        self.codegen_job(&key.0, &key.1, id)
    }

    /// Remember that `token` started the codegen job `id` in that app environment.
    pub fn remember_codegen_job_token(&mut self, key: EnvironmentKey, token: String, id: String) {
        self.codegen_job_tokens.insert((key, token), id);
    }

    /// How many codegen jobs the whole store holds — what [`CODEGEN_JOB_QUOTA`]
    /// bounds.
    pub fn total_codegen_jobs(&self) -> usize {
        self.codegen_jobs.values().map(Vec::len).sum()
    }

    /// Mint an unused codegen job id. `CodegenJob$id` targets `Uuid`, so the id is
    /// laid out as a version 4 UUID; the loop skips an id already taken in that
    /// environment.
    pub fn mint_codegen_job_id(&mut self, key: &EnvironmentKey) -> String {
        loop {
            let id = self.next_uuid();
            if self
                .codegen_jobs
                .get(key)
                .is_none_or(|jobs| jobs.iter().all(|job| job.id != id))
            {
                return id;
            }
        }
    }

    /// Mint an unused form id. `Form$id` targets `Uuid`, so the id is laid out
    /// as a version 4 UUID; the loop skips an id already taken in that
    /// environment.
    pub fn mint_form_id(&mut self, key: &EnvironmentKey) -> String {
        loop {
            let id = self.next_uuid();
            if self
                .forms
                .get(key)
                .is_none_or(|forms| forms.iter().all(|form| form.id != id))
            {
                return id;
            }
        }
    }

    /// The themes stored for one app environment, in creation order.
    pub fn themes_in(&self, app_id: &str, environment_name: &str) -> &[StoredTheme] {
        self.themes
            .get(&(app_id.to_string(), environment_name.to_string()))
            .map_or(&[], Vec::as_slice)
    }

    /// The theme `id` names in one app environment.
    pub fn theme(&self, app_id: &str, environment_name: &str, id: &str) -> Option<&StoredTheme> {
        self.themes_in(app_id, environment_name)
            .iter()
            .find(|theme| theme.id == id)
    }

    /// Whether `id` names a theme some OTHER app environment owns.
    ///
    /// A theme is addressed by (appId, environmentName, id) exactly as a component
    /// and a form are, so an id that resolves outside the app environment the
    /// request named is a request made in the wrong scope — told apart here from an
    /// id that names no theme at all, which is simply absent. The counterparts are
    /// [`Self::component_in_other_scope`] and [`Self::form_in_other_scope`].
    pub fn theme_in_other_scope(&self, app_id: &str, environment_name: &str, id: &str) -> bool {
        id_in_other_scope(&self.themes, app_id, environment_name, id, |theme| {
            &theme.id
        })
    }

    /// The theme `id` names in one app environment, for a caller that changes it.
    pub fn theme_mut(
        &mut self,
        app_id: &str,
        environment_name: &str,
        id: &str,
    ) -> Option<&mut StoredTheme> {
        self.themes
            .get_mut(&(app_id.to_string(), environment_name.to_string()))?
            .iter_mut()
            .find(|theme| theme.id == id)
    }

    /// Store a theme at the end of its environment's creation order.
    pub fn insert_theme(&mut self, theme: StoredTheme) {
        self.themes
            .entry((theme.app_id.clone(), theme.environment_name.clone()))
            .or_default()
            .push(theme);
    }

    /// Remove the theme `id` names, and forget every client token that named it
    /// so the token map cannot outlive the record — the same way
    /// [`ComAmazonawsAmplifyuibuilderAmplifyuibuilderState::remove_component`]
    /// releases a component.
    pub fn remove_theme(
        &mut self,
        app_id: &str,
        environment_name: &str,
        id: &str,
    ) -> Option<StoredTheme> {
        let key = (app_id.to_string(), environment_name.to_string());
        let themes = self.themes.get_mut(&key)?;
        let index = themes.iter().position(|theme| theme.id == id)?;
        let removed = themes.remove(index);
        if themes.is_empty() {
            self.themes.remove(&key);
        }
        self.theme_tokens
            .retain(|(token_key, _), token_id| !(*token_key == key && token_id == id));
        Some(removed)
    }

    /// The theme one `clientToken` already created in the app environment the
    /// create addressed, if that token has been seen there.
    pub fn replayed_theme(&self, key: &EnvironmentKey, token: &str) -> Option<&StoredTheme> {
        let id = self.theme_tokens.get(&(key.clone(), token.to_string()))?;
        self.theme(&key.0, &key.1, id)
    }

    /// Remember that `token` created the theme `id` in that app environment.
    pub fn remember_theme_token(&mut self, key: EnvironmentKey, token: String, id: String) {
        self.theme_tokens.insert((key, token), id);
    }

    /// The live theme called `name` in one app environment, if there is one.
    ///
    /// A theme name identifies a theme within its app environment exactly as a
    /// component or a form name does, so this is what a create (or a rename)
    /// consults before taking a name; the component counterpart is
    /// [`Self::component_named`].
    pub fn theme_named(
        &self,
        app_id: &str,
        environment_name: &str,
        name: &str,
    ) -> Option<&StoredTheme> {
        record_named(self.themes_in(app_id, environment_name), name, |theme| {
            &theme.name
        })
    }

    /// How many themes the whole store holds — what [`THEME_QUOTA`] bounds.
    pub fn total_themes(&self) -> usize {
        self.themes.values().map(Vec::len).sum()
    }

    /// Mint an unused theme id. `Theme$id` targets `Uuid`, like a component's,
    /// so the id is laid out the same way; the loop skips an id already taken in
    /// that environment.
    pub fn mint_theme_id(&mut self, key: &EnvironmentKey) -> String {
        loop {
            let id = self.next_uuid();
            if self
                .themes
                .get(key)
                .is_none_or(|themes| themes.iter().all(|theme| theme.id != id))
            {
                return id;
            }
        }
    }

    /// Materialise the expiry the clock has ALREADY made due: forget every token
    /// session whose access token has run out.
    ///
    /// Owed at each session's `expires_at` whether or not anybody called, so a
    /// token handler runs this first and then reads only live sessions.
    /// Idempotent.
    pub fn expire_provider_token_sessions(&mut self, now: SystemTime) {
        self.provider_token_sessions
            .retain(|_, session| session.is_live(now));
    }

    /// The live token session `key` addresses, or `None` when this store holds
    /// none for it.
    pub fn provider_token_session(&self, key: &ProviderTokenKey) -> Option<&ProviderTokenSession> {
        self.provider_token_sessions.get(key)
    }

    /// Whether the store may hold the session `key` names: one it already holds
    /// always may (nothing grows), a new one only while the store is under
    /// [`PROVIDER_TOKEN_SESSION_QUOTA`]. Call this BEFORE
    /// [`Self::refresh_provider_token_session`].
    pub fn has_room_for_provider_token_session(&self, key: &ProviderTokenKey) -> bool {
        self.provider_token_sessions.contains_key(key)
            || self.has_room_for_another_provider_token_session()
    }

    /// Whether the store may hold ANOTHER session: what
    /// [`PROVIDER_TOKEN_SESSION_QUOTA`] bounds.
    ///
    /// For a caller that is about to start a session the store cannot already
    /// hold, so it has no key to ask about yet: an exchange, whose session is
    /// named by a refresh token it has not minted. A refresh addresses a session
    /// that may already be held, so it asks
    /// [`Self::has_room_for_provider_token_session`] instead.
    pub fn has_room_for_another_provider_token_session(&self) -> bool {
        self.provider_token_sessions.len() < PROVIDER_TOKEN_SESSION_QUOTA
    }

    /// Draw the secret material a token request hands back, and ONLY once the
    /// bound this store is under has admitted the session that request
    /// addresses.
    ///
    /// Both token handlers reach the credential store through here, so the ORDER
    /// of the two steps belongs to the store rather than to each handler: `mint`
    /// is called on the admitted path alone, so a request refused for the bound
    /// has drawn nothing. Drawing a token is work on the shared credential store
    /// — a `create` and a `delete` (see [`mint_provider_token`]) — and a token
    /// request that is going to be refused must do none of it.
    ///
    /// `addressed` names the session when the store may already hold it (a
    /// refresh), and is `None` when the request starts a session this store
    /// cannot already hold (an exchange, whose session is named by a refresh
    /// token it has not drawn yet). A `mint` that reports `None` drew no secret
    /// to build a token from, which is
    /// [`ProviderTokenRefusal::NothingMinted`].
    ///
    /// Call [`Self::expire_provider_token_sessions`] first, so the bound is read
    /// over the sessions that are still live.
    pub fn mint_within_token_session_bound<T>(
        &self,
        addressed: Option<&ProviderTokenKey>,
        mint: impl FnOnce() -> Option<T>,
    ) -> Result<T, ProviderTokenRefusal> {
        let admitted = match addressed {
            Some(key) => self.has_room_for_provider_token_session(key),
            None => self.has_room_for_another_provider_token_session(),
        };
        if !admitted {
            return Err(ProviderTokenRefusal::OverQuota);
        }
        mint().ok_or(ProviderTokenRefusal::NothingMinted)
    }

    /// Make `access_token` the live access token of the session `key` names,
    /// from `now`, and report the STORED record — which is what the response is
    /// read off, so a caller is never told a token the store does not hold.
    ///
    /// A session the store already holds is REPLACED: refreshing hands out a new
    /// access token, and the one it supersedes stops being the session's.
    pub fn refresh_provider_token_session(
        &mut self,
        key: ProviderTokenKey,
        access_token: String,
        now: SystemTime,
    ) -> &ProviderTokenSession {
        self.put_provider_token_session(key, access_token, None, now)
    }

    /// Start the session an EXCHANGE of `code` grants: as
    /// [`Self::refresh_provider_token_session`], and the session also records the
    /// digest of that access code, which is what makes the code single-use while
    /// the session lives.
    pub fn start_exchanged_provider_token_session(
        &mut self,
        key: ProviderTokenKey,
        access_token: String,
        code: &str,
        now: SystemTime,
    ) -> &ProviderTokenSession {
        self.put_provider_token_session(key, access_token, Some(token_digest(code)), now)
    }

    /// The one write over [`Self::provider_token_sessions`], shared by the
    /// refresh and the exchange paths.
    ///
    /// A session the store already holds is REPLACED, but KEEPS the access code
    /// it was exchanged from: a refresh hands out a new access token, and it does
    /// not un-spend the code that started the session.
    fn put_provider_token_session(
        &mut self,
        key: ProviderTokenKey,
        access_token: String,
        code_digest: Option<String>,
        now: SystemTime,
    ) -> &ProviderTokenSession {
        use std::collections::hash_map::Entry;

        let mut session = ProviderTokenSession::new(&key, access_token, now);
        session.code_digest = code_digest;
        match self.provider_token_sessions.entry(key) {
            Entry::Occupied(mut held) => {
                // A refresh carries no access code of its own, so the code the
                // session was exchanged from stays spent for as long as the
                // session lives.
                if session.code_digest.is_none() {
                    session.code_digest = held.get().code_digest.clone();
                }
                held.insert(session);
                held.into_mut()
            }
            Entry::Vacant(empty) => empty.insert(session),
        }
    }

    /// Whether `code` already started a session of `provider` the store still
    /// holds: an authorization code is single-use (RFC 6749 §4.1.2), so a second
    /// exchange of one is refused rather than granting another token pair. Call
    /// [`Self::expire_provider_token_sessions`] first, so a code whose session
    /// has run out is spent no longer.
    pub fn provider_code_already_exchanged(&self, provider: &str, code: &str) -> bool {
        let digest = token_digest(code);
        self.provider_token_sessions.values().any(|session| {
            session.provider == provider && session.code_digest.as_deref() == Some(digest.as_str())
        })
    }

    /// One candidate id: a version 4 UUID whose 122 free bits are the leading
    /// bits of a SHA-256 digest over this store's seed, a counter and the clock.
    ///
    /// Shared by every `Uuid` this service mints — a component id and a codegen
    /// job id are drawn from the same counter, so two resources of one run never
    /// collide. The seed is drawn from the operating system once per store, so ids
    /// are not predictable from one run to the next; the counter and the clock
    /// make two ids of one run differ even on a seed collision. `sha2` does the
    /// hashing and `hex` the rendering — this only composes the input and sets
    /// the two nibbles RFC 4122 reserves. Every resource this service mints an
    /// id for draws from this one counter, so two resources cannot be handed the
    /// same id either.
    fn next_uuid(&mut self) -> String {
        use sha2::{Digest, Sha256};

        let seed = *self.id_seed.get_or_insert_with(os_seed);
        self.id_seq = self.id_seq.saturating_add(1);
        let nanos = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map_or(0, |since| since.as_nanos());

        let mut digest = Sha256::new();
        digest.update(seed.to_le_bytes());
        digest.update(self.id_seq.to_le_bytes());
        digest.update(nanos.to_le_bytes());
        let digest = digest.finalize();

        let mut bytes = [0u8; 16];
        bytes.copy_from_slice(&digest[..16]);
        // RFC 4122 §4.4: the high nibble of octet 6 is the version (4) and the
        // two high bits of octet 8 are the variant (10).
        bytes[6] = (bytes[6] & 0x0f) | 0x40;
        bytes[8] = (bytes[8] & 0x3f) | 0x80;

        let hex = hex::encode(bytes);
        format!(
            "{}-{}-{}-{}-{}",
            &hex[..8],
            &hex[8..12],
            &hex[12..16],
            &hex[16..20],
            &hex[20..]
        )
    }
}

/// Whether `id` names a record some OTHER app environment owns, in one of this
/// store's (appId, environmentName)-keyed collections.
///
/// The one place the wrong-scope test lives: every resource of this service that
/// is addressed by (appId, environmentName, id) classifies a miss the same way,
/// so the per-kind methods above ([`ComAmazonawsAmplifyuibuilderAmplifyuibuilderState::component_in_other_scope`]
/// and its counterparts) differ only in which collection they search and where
/// the record carries its id.
fn id_in_other_scope<T>(
    collections: &HashMap<EnvironmentKey, Vec<T>>,
    app_id: &str,
    environment_name: &str,
    id: &str,
    record_id: impl Fn(&T) -> &str,
) -> bool {
    collections
        .iter()
        .filter(|((app, environment), _)| app != app_id || environment != environment_name)
        .any(|(_, records)| records.iter().any(|record| record_id(record) == id))
}

/// The record called `name` among those one app environment holds, if there is
/// one.
///
/// The one place the name-conflict lookup lives: a resource name identifies a
/// resource of its kind within its app environment, so the per-kind methods above
/// ([`ComAmazonawsAmplifyuibuilderAmplifyuibuilderState::component_named`] and its
/// counterparts) differ only in which collection they search and where the record
/// carries its name.
fn record_named<'a, T>(
    records: &'a [T],
    name: &str,
    record_name: impl Fn(&T) -> &str,
) -> Option<&'a T> {
    records.iter().find(|record| record_name(record) == name)
}

/// A seed from the operating system, for the ids one store mints.
///
/// `RandomState` draws its hasher keys from the OS when it is constructed, so a
/// fresh instance's hash of the empty input is OS-drawn entropy — the standard
/// library's own source, with no further dependency.
fn os_seed() -> u64 {
    use std::hash::{BuildHasher, Hasher};

    std::collections::hash_map::RandomState::new()
        .build_hasher()
        .finish()
}

/// True when a component property map carries the reserved
/// [`RESERVED_PROPERTY_NAME`] key.
///
/// The rule is about the KEY, so this is generic over the map's value type: the
/// four maps a component itself carries, the one on each `ComponentChild` and the
/// two on each `ComponentVariant` are all tested through this one predicate.
pub fn carries_reserved_name<V>(map: &HashMap<String, V>) -> bool {
    map.contains_key(RESERVED_PROPERTY_NAME)
}

/// The path of the map nearest the top of a `children` tree that carries the
/// reserved key — siblings in request order — relative to the payload holding
/// that list, or `None` when no map in the tree carries it.
///
/// `ComponentChild$properties` documents the same rule the component's own maps
/// do, and a child carries its own `children`, so the rule is tested at every
/// depth of the tree. The reported path is the one the request spells, so a
/// refusal names the map the caller wrote rather than the member containing it.
///
/// The tree is walked level by level through an explicit queue rather than by
/// recursion: its depth is whatever the request nested, so no payload can drive
/// this walk into the call stack.
pub fn reserved_name_in_children(children: &[ComponentChild]) -> Option<String> {
    let mut pending: VecDeque<(String, &[ComponentChild])> = VecDeque::new();
    pending.push_back((String::new(), children));
    while let Some((prefix, level)) = pending.pop_front() {
        for (index, child) in level.iter().enumerate() {
            let path = format!("{prefix}children[{index}]");
            if carries_reserved_name(&child.properties) {
                return Some(format!("{path}.properties"));
            }
            if let Some(nested) = child.children.as_deref() {
                pending.push_back((format!("{path}."), nested));
            }
        }
    }
    None
}

/// The path of the first map under a `variants` list that carries the reserved
/// key, in request order, relative to the payload holding that list, or `None`
/// when none does.
///
/// `ComponentVariant$variantValues` and `ComponentVariant$overrides` document the
/// same rule, so a variant naming the key is refused exactly as a component's own
/// maps are.
pub fn reserved_name_in_variants(variants: &[ComponentVariant]) -> Option<String> {
    variants.iter().enumerate().find_map(|(index, variant)| {
        [
            (
                "variantValues",
                variant
                    .variant_values
                    .as_ref()
                    .is_some_and(carries_reserved_name),
            ),
            (
                "overrides",
                variant.overrides.as_ref().is_some_and(carries_reserved_name),
            ),
        ]
        .into_iter()
        .find_map(|(member, carries)| carries.then(|| format!("variants[{index}].{member}")))
    })
}

/// The `InvalidParameterException` payload a write answers for `path` carrying a
/// value outside the set the model documents for that member.
///
/// ONE message form for the class, so every member whose valid values the model
/// spells out in prose is refused the same way: what was supplied, where, and
/// what would have been accepted — the caller has to fix the request from this
/// text alone.
fn value_not_valid(path: &str, value: &str, valid: &[String]) -> InvalidParameterException {
    InvalidParameterException {
        message: Some(format!(
            "Invalid value {value} for {path}; valid values are {}",
            valid.join(", ")
        )),
    }
}

/// The refusal a form write answers for a sectional element whose `type`, `level`
/// or `orientation` lies outside the values the model documents for it, or `None`
/// when every element of `elements` is within them.
///
/// The three members target plain `String`/`Integer` shapes, so the valid sets
/// live in [`SECTIONAL_ELEMENT_TYPES`], [`SECTIONAL_ELEMENT_ORIENTATIONS`] and
/// [`SECTIONAL_ELEMENT_LEVEL_MIN`]..=[`SECTIONAL_ELEMENT_LEVEL_MAX`] rather than
/// in the shape, and the check is shared by every form write path
/// (`CreateForm`, `UpdateForm`) the way `carries_reserved_name` is shared by the
/// component ones. `payload_member` is the input member holding the map
/// (`formToCreate`, `updatedForm`), so the refusal names the member the caller
/// wrote.
///
/// A map has no order, so the elements are examined by key in sorted order and
/// the members of one element in the order the shape declares them: the same
/// payload is always refused for the same value.
pub fn invalid_sectional_element_value(
    payload_member: &str,
    elements: &HashMap<String, SectionalElement>,
) -> Option<InvalidParameterException> {
    let valid_levels = SECTIONAL_ELEMENT_LEVEL_MIN..=SECTIONAL_ELEMENT_LEVEL_MAX;
    let levels: Vec<String> = valid_levels.clone().map(|level| level.to_string()).collect();
    let types: Vec<String> = SECTIONAL_ELEMENT_TYPES
        .iter()
        .map(|value| value.to_string())
        .collect();
    let orientations: Vec<String> = SECTIONAL_ELEMENT_ORIENTATIONS
        .iter()
        .map(|value| value.to_string())
        .collect();

    let mut keys: Vec<&str> = elements.keys().map(String::as_str).collect();
    keys.sort_unstable();
    keys.into_iter().find_map(|key| {
        let element = &elements[key];
        let path = |member: &str| format!("{payload_member}.sectionalElements.{key}.{member}");
        if !SECTIONAL_ELEMENT_TYPES.contains(&element.r#type.as_str()) {
            return Some(value_not_valid(&path("type"), &element.r#type, &types));
        }
        if let Some(level) = element.level.filter(|level| !valid_levels.contains(level)) {
            return Some(value_not_valid(&path("level"), &level.to_string(), &levels));
        }
        if let Some(orientation) = element
            .orientation
            .as_deref()
            .filter(|orientation| !SECTIONAL_ELEMENT_ORIENTATIONS.contains(orientation))
        {
            return Some(value_not_valid(
                &path("orientation"),
                orientation,
                &orientations,
            ));
        }
        None
    })
}

/// Which collection of an app environment a page of records was served over.
///
/// A cursor is a position in ONE collection's creation order, and every record
/// this service stores draws its cursor from the single counter
/// [`ComAmazonawsAmplifyuibuilderAmplifyuibuilderState::next_cursor`] hands out,
/// so the same cursor value names a component in one collection and nothing at
/// all in another. A page token therefore carries which collection it indexes
/// beside the app environment it was issued for, and a token presented to an
/// operation that pages a DIFFERENT collection names no page of it.
///
/// The variants are the four collections this service pages: the components,
/// forms and themes of an app environment, and its codegen jobs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PagedCollection {
    Components,
    Forms,
    Themes,
    CodegenJobs,
}

impl PagedCollection {
    /// How a collection is spelled into the digest a page token carries: the
    /// last segment of the `@http` URI its operations are bound to, so the four
    /// labels are distinct and each names its collection as the model does.
    fn label(self) -> &'static str {
        match self {
            PagedCollection::Components => "components",
            PagedCollection::Forms => "forms",
            PagedCollection::Themes => "themes",
            PagedCollection::CodegenJobs => "codegen-jobs",
        }
    }
}

/// The `nextToken` a page of `collection`'s records in `key` that ends before
/// `cursor` reports.
///
/// The token carries the app environment it was issued for and the collection it
/// was served over as well as the cursor, so it cannot be replayed against
/// another (appId, environmentName) or against another collection of the same
/// app environment: the pages a caller walks are the ones of the list it
/// started.
pub fn page_token(key: &EnvironmentKey, collection: PagedCollection, cursor: u64) -> String {
    format!(
        "{PAGE_TOKEN_PREFIX}{scope:0scope_width$x}{cursor:0cursor_width$x}",
        scope = token_scope(key, collection),
        scope_width = PAGE_TOKEN_SCOPE_WIDTH,
        cursor_width = PAGE_TOKEN_HEX_WIDTH
    )
}

/// The cursor `token` resumes `collection` in `key` at, or `None` when it is not
/// a token this service issued over that collection of that app environment.
///
/// The cursor has to be one the service actually handed out: the cursors in
/// existence run from [`FIRST_CURSOR`] to `issued`, the highest handed out, so a
/// well-formed token carrying anything outside that span — above `issued`, or
/// below the first cursor ever minted — names no page, exactly as a token of the
/// wrong shape, the wrong app environment or another collection does.
pub fn parse_page_token(
    token: &str,
    key: &EnvironmentKey,
    collection: PagedCollection,
    issued: u64,
) -> Option<u64> {
    let digits = token.strip_prefix(PAGE_TOKEN_PREFIX)?;
    // Both halves are hex digits, so refusing anything else here is what makes
    // the split below land on a character boundary: a request-supplied token is
    // arbitrary UTF-8, and splitting one at a byte offset inside a multi-byte
    // scalar would panic.
    if digits.len() != PAGE_TOKEN_SCOPE_WIDTH + PAGE_TOKEN_HEX_WIDTH
        || !digits.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return None;
    }
    let (scope, cursor) = digits.split_at(PAGE_TOKEN_SCOPE_WIDTH);
    if u32::from_str_radix(scope, 16).ok()? != token_scope(key, collection) {
        return None;
    }
    let cursor = u64::from_str_radix(cursor, 16).ok()?;
    (FIRST_CURSOR..=issued).contains(&cursor).then_some(cursor)
}

/// A record one of this service's paginated operations pages over: one held in
/// its app environment's creation (or start) order, carrying the cursor that
/// order assigned it and belonging to one of the collections this service pages.
///
/// It is all [`page_of`] needs of a record, so every stored kind whose list is
/// paginated implements it and none of them needs its own page walk. The
/// collection comes off the record kind rather than from the handler, so the
/// operations that page one collection — its list and its export alike — all
/// honour each other's tokens and no operation can mint a token under another
/// collection's name.
pub trait PagedRecord {
    /// Which collection a walk over these records pages, and so the collection
    /// the tokens that walk issues belong to.
    const COLLECTION: PagedCollection;

    /// The position this record holds in its app environment's order — what a
    /// page token indexes into.
    fn cursor(&self) -> u64;
}

impl PagedRecord for StoredComponent {
    const COLLECTION: PagedCollection = PagedCollection::Components;

    fn cursor(&self) -> u64 {
        self.cursor
    }
}

impl PagedRecord for StoredCodegenJob {
    const COLLECTION: PagedCollection = PagedCollection::CodegenJobs;

    fn cursor(&self) -> u64 {
        self.cursor
    }
}

impl PagedRecord for StoredForm {
    const COLLECTION: PagedCollection = PagedCollection::Forms;

    fn cursor(&self) -> u64 {
        self.cursor
    }
}

impl PagedRecord for StoredTheme {
    const COLLECTION: PagedCollection = PagedCollection::Themes;

    fn cursor(&self) -> u64 {
        self.cursor
    }
}

/// One page of an app environment's records: the projections it serves, and the
/// token that resumes after them — absent when the page ends the list.
#[derive(Debug)]
pub struct Page<T> {
    pub entities: Vec<T>,
    pub next_token: Option<String>,
}

/// A `nextToken` this service never issued for the app environment and the
/// collection it was presented against, and so names no page.
#[derive(Debug)]
pub struct UnknownPageToken(String);

impl UnknownPageToken {
    /// The `message` the refusal carries, naming the operation the token was
    /// presented to and the token it refuses.
    pub fn message(&self, operation: &str) -> String {
        format!(
            "nextToken {:?} is not a token returned by {operation} for this app environment",
            self.0
        )
    }
}

/// One page of `records`, resumed at `next_token` and at most `page_size` long,
/// with each served record put through `project`.
///
/// The one page walk of this service, shared by every paginated operation:
///
/// * a request carrying no token starts at [`FIRST_CURSOR`], the head of the
///   list;
/// * a token is honoured only when [`parse_page_token`] recognises it as one this
///   service issued for `key` over the collection `records` belong to, out of
///   the `issued` cursors it has handed out — anything else, a token of another
///   collection included, yields [`UnknownPageToken`], which the caller turns
///   into its own modelled refusal rather than serving a page;
/// * the page resumes at the first record AT OR AFTER the cursor, so a page is
///   still served when the record the token named was deleted meanwhile;
/// * once `page_size` records are served, the next token is issued from the
///   cursor of the first record left unserved, so the next page resumes exactly
///   there.
pub fn page_of<R: PagedRecord, T>(
    records: &[R],
    key: &EnvironmentKey,
    issued: u64,
    next_token: Option<&str>,
    page_size: usize,
    project: impl Fn(&R) -> T,
) -> Result<Page<T>, UnknownPageToken> {
    let cursor = match next_token {
        None => FIRST_CURSOR,
        Some(token) => parse_page_token(token, key, R::COLLECTION, issued)
            .ok_or_else(|| UnknownPageToken(token.to_string()))?,
    };

    let mut entities = Vec::with_capacity(page_size.min(records.len()));
    let mut resume_at = None;
    for record in records {
        if record.cursor() < cursor {
            continue;
        }
        if entities.len() == page_size {
            resume_at = Some(page_token(key, R::COLLECTION, record.cursor()));
            break;
        }
        entities.push(project(record));
    }

    Ok(Page {
        entities,
        next_token: resume_at,
    })
}

/// The scope half of a page token: a digest of the (appId, environmentName) the
/// page was served for and of the collection it was served over.
///
/// Both, because a cursor is a position in one collection of one app
/// environment: a token whose scope digest does not match the page being asked
/// for names no page of it.
///
/// A token is only meaningful within the run that issued it, so the standard
/// hasher is enough; nothing persists a token across runs.
fn token_scope(key: &EnvironmentKey, collection: PagedCollection) -> u32 {
    use std::hash::{Hash, Hasher};

    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    key.0.hash(&mut hasher);
    key.1.hash(&mut hasher);
    collection.label().hash(&mut hasher);
    (hasher.finish() >> 32) as u32
}

/// True when `name` is a feature this service's metadata holds.
pub fn is_known_metadata_feature(name: &str) -> bool {
    METADATA_FEATURES.contains(&name)
}

// ---------------------------------------------------------------------------
// The third-party token surface: the access token `RefreshToken` hands back.
// ---------------------------------------------------------------------------

/// How long an access token this service hands back stays live, and so the
/// `expiresIn` a freshly minted one reports.
///
/// CHOSEN, not documented: AWS publishes no lifetime for the access token the
/// AmplifyUIBuilder token operations answer with. An hour is long enough for a
/// caller to use the token it was just handed, and short enough that a session
/// nobody refreshes is released from the store again soon.
pub const PROVIDER_ACCESS_TOKEN_LIFETIME: Duration = Duration::from_secs(3600);

/// How many characters of credential-store secret an access token carries.
///
/// CHOSEN: AWS publishes no length for it, and the token is opaque to a caller.
/// 32 characters of the store's 64-symbol alphabet is ~192 bits, so a token
/// cannot be guessed inside [`PROVIDER_ACCESS_TOKEN_LIFETIME`].
pub const PROVIDER_ACCESS_TOKEN_CHARS: usize = 32;

/// How many live token sessions the store holds before a refresh that would
/// start another one is refused.
///
/// A session is keyed by request-supplied values, so without a bound every
/// request naming a fresh refresh token would add an entry. Amplify UI Builder
/// publishes no quota this emulator can quote, so — exactly like
/// [`COMPONENT_QUOTA`] — this is the emulator's OWN bound on the store, set far
/// above any workflow an emulator serves. Each entry is released by
/// [`ComAmazonawsAmplifyuibuilderAmplifyuibuilderState::expire_provider_token_sessions`]
/// as soon as its access token has run out, so the bound holds a count of LIVE
/// tokens rather than of calls.
pub const PROVIDER_TOKEN_SESSION_QUOTA: usize = 1_000;

/// Which token session a request addresses: the `provider` it names, the
/// `clientId` it carries (absent when it names none) and the refresh token
/// itself.
///
/// All three, because a refresh token is issued to ONE client of ONE provider:
/// two clients presenting the same token hold two sessions, each with its own
/// access token, rather than taking each other's.
pub type ProviderTokenKey = (String, Option<String>, String);

/// Why a token request was handed no token at all: the two conditions
/// [`ComAmazonawsAmplifyuibuilderAmplifyuibuilderState::mint_within_token_session_bound`]
/// reports, in the order it settles them. Each token handler answers its own
/// modelled refusal for each.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProviderTokenRefusal {
    /// The store already holds [`PROVIDER_TOKEN_SESSION_QUOTA`] live sessions and
    /// the request addresses none of them, so it would start another: settled
    /// before any secret material is drawn.
    OverQuota,
    /// The credential store drew no secret to build a token from — a condition on
    /// this service's own side rather than a refusal of the request.
    NothingMinted,
}

/// One live third-party token session: the access token this service last handed
/// back for a refresh token, and when that access token stops being live.
///
/// The refresh token itself is the session's NAME rather than part of the record:
/// it is held only as the third element of the [`ProviderTokenKey`] the session
/// is stored under, and this record states nothing further about it. Where that
/// name comes from is the token operation's own: an exchange models a `@required`
/// `refreshToken` output, so it MINTS the one it names the session it starts by
/// ([`mint_provider_token`] draws both tokens that answer carries), while a
/// refresh models no token to hand out and is presented one its caller already
/// holds. Either way, every name in this map resolves a session this store holds.
#[derive(Clone)]
pub struct ProviderTokenSession {
    /// The provider the session's tokens belong to (`TokenProviders`).
    pub provider: String,
    /// The `clientId` the refreshing request named, if it named one.
    pub client_id: Option<String>,
    /// The access token currently live for the session. Minted by the emulator's
    /// credential store (see [`mint_provider_token`]), never constructed
    /// here.
    pub access_token: String,
    /// The digest of the access code the EXCHANGE that started this session was
    /// made from, and `None` for a session a refresh started — a refresh carries
    /// no code.
    ///
    /// A digest, not the code: the code is `@sensitive` and nothing ever needs it
    /// back — recognising a code presented again is all this is for, which is what
    /// makes an authorization code single-use while its session lives
    /// ([`ComAmazonawsAmplifyuibuilderAmplifyuibuilderState::provider_code_already_exchanged`]).
    pub code_digest: Option<String>,
    /// When this access token was handed back.
    pub issued_at: SystemTime,
    /// When it stops being live: [`PROVIDER_ACCESS_TOKEN_LIFETIME`] after
    /// [`Self::issued_at`].
    pub expires_at: SystemTime,
}

/// Prints the session's provider and client but NEITHER token, so a formatted or
/// logged value leaks no secret material — the shape the credential store's own
/// minted-credential type takes for the same reason.
impl std::fmt::Debug for ProviderTokenSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProviderTokenSession")
            .field("provider", &self.provider)
            .field("client_id", &self.client_id)
            .field("issued_at", &self.issued_at)
            .field("expires_at", &self.expires_at)
            .finish_non_exhaustive()
    }
}

impl ProviderTokenSession {
    /// The session `key` names, carrying `access_token` from `now` until
    /// [`PROVIDER_ACCESS_TOKEN_LIFETIME`] later.
    pub fn new(key: &ProviderTokenKey, access_token: String, now: SystemTime) -> Self {
        Self {
            provider: key.0.clone(),
            client_id: key.1.clone(),
            access_token,
            code_digest: None,
            issued_at: now,
            expires_at: now + PROVIDER_ACCESS_TOKEN_LIFETIME,
        }
    }

    /// Whether this access token is still live at `now`.
    pub fn is_live(&self, now: SystemTime) -> bool {
        now < self.expires_at
    }

    /// The `expiresIn` a response reports for this access token: the whole
    /// seconds it has left at `now`, read off the stored expiry instant.
    ///
    /// A SPAN, not an instant. The model's prose for the member reads "The date
    /// and time when the new access token expires", but it targets `Integer`
    /// with no timestamp format and no units
    /// (`smithy_query(query="operation", operation="RefreshToken")`), and the
    /// name both token operations report it under — `expiresIn`, a time to live
    /// — is what a caller reads: how long the token it was just handed remains
    /// usable, the quantity a provider's OAuth token endpoint reports in
    /// seconds. What the service ENFORCES is the expiry INSTANT
    /// ([`Self::expires_at`], applied by
    /// [`ComAmazonawsAmplifyuibuilderAmplifyuibuilderState::expire_provider_token_sessions`]),
    /// and this span is read from it rather than from the request.
    pub fn expires_in(&self, now: SystemTime) -> i32 {
        let left = self
            .expires_at
            .duration_since(now)
            .unwrap_or_default()
            .as_secs();
        i32::try_from(left).unwrap_or(i32::MAX)
    }
}

/// A fresh opaque provider token, or `None` when the credential store minted no
/// secret to build one from.
///
/// One mint for both tokens a token operation hands back: the ACCESS token a
/// refresh and an exchange answer with, and the REFRESH token an exchange issues
/// to name the session it starts (called once per token, so the two an exchange
/// hands back are two different secrets). A refresh issues none — it is presented
/// one the caller already holds.
///
/// A token is a secret handed to a caller, and nothing in a service may construct
/// secret material itself, so it is drawn from the one component that generates
/// secrets: [`PROVIDER_ACCESS_TOKEN_CHARS`] characters of one credential's secret.
/// It authenticates to the THIRD-PARTY provider and not to this emulator, so the
/// credential is removed from the store again straight away: the caller gets the
/// token, the emulator keeps no credential it would accept as SigV4, and nothing
/// accumulates per call.
pub fn mint_provider_token(store: &::emulator_server::CredentialStore) -> Option<String> {
    let minted = store.create(&::emulator_server::aws_credentials::AWS_SESSION);
    let token: Option<String> = minted
        .secret(::emulator_server::aws_credentials::SESSION_TOKEN)
        .map(|secret| secret.chars().take(PROVIDER_ACCESS_TOKEN_CHARS).collect());
    // Reclaimed on EVERY path: the token is not a credential of this emulator
    // (see above), so a mint that yielded no secret must not leave one behind
    // either.
    let _ = store.delete(&minted.id);
    token
}

/// The message of the refusal of a token request whose refresh token carries no
/// characters at all.
///
/// `RefreshTokenRequestBody$token` is a `@required` `SensitiveString` with no
/// `@length` (`smithy_query(query="shape", shape="RefreshTokenRequestBody")`),
/// so the model admits a blank one and the service is what refuses it. The
/// sentence interpolates NOTHING: the member is `@sensitive`, so a refusal must
/// not quote the value it rejected, and — like every message of this service
/// ([`ITEM_NOT_FOUND_MESSAGE`] and the literals beside it) — it is the only
/// machine-readable name this failure class has on an untyped error.
pub const BLANK_REFRESH_TOKEN_MESSAGE: &str = "The refresh token provided is not valid";

/// The `InvalidParameterException` payload for a refresh token that carries no
/// characters.
pub fn blank_refresh_token() -> InvalidParameterException {
    InvalidParameterException {
        message: Some(BLANK_REFRESH_TOKEN_MESSAGE.to_string()),
    }
}

/// The refusal of a request that would grow one of this emulator's OWN stores
/// past the bound it is held under, for an operation whose model declares no
/// quota error.
///
/// The bounds the sentence quotes — [`METADATA_ENVIRONMENT_QUOTA`],
/// [`PROVIDER_TOKEN_SESSION_QUOTA`] and the constants beside them — are the
/// emulator's own, so AWS refuses nothing here and there is no wording of its
/// own to reproduce. The SHAPE is what matters to a client: this service strips
/// every error-type discriminator (see `crate::service::untyped_errors`), so the
/// status is all a client has to classify a refusal by, and it may therefore only
/// be a status the refusing operation's own model declares. Of the shapes
/// `PutMetadataFlag`, `RefreshToken` and `ExchangeCodeForToken` each declare,
/// `InvalidParameterException` (400) is the one whose documented condition — "an
/// invalid or out-of-range value was supplied for the input parameter" — covers a
/// request naming an app environment or a token session beyond the range the
/// store can hold. An operation whose model declares a shape that fits this
/// condition better answers that one instead: `CreateComponent` and the other
/// creates declare `ServiceQuotaExceededException` (402), and `StartCodegenJob`
/// declares `ThrottlingException` (429).
///
/// `held` names what the store already holds, and `bound` the maximum it holds of
/// it; nothing from the request is interpolated, so the sentence stays the one
/// machine-readable name this class has on an untyped refusal.
pub fn store_bound_reached(held: &str, bound: usize) -> InvalidParameterException {
    InvalidParameterException {
        message: Some(format!(
            "this emulator already holds the maximum of {bound} {held}"
        )),
    }
}

// The messages of this service's resource refusals.
//
// AmplifyUIBuilder answers a refused resource call with NO error-type
// discriminator (see `crate::service::untyped_errors`): no `x-amzn-errortype`
// header and no `__type` / `code` / `Code` member, so a client's `Error.Code`
// degrades to the bare status string and the `message` is the ONLY
// machine-readable name the failure class has. The service's own messages for
// these classes are therefore part of its wire contract, and each is a FIXED
// literal that interpolates nothing from the request — no app id, no
// environment name, no resource id or name — and one literal per class, not per
// resource kind. A message built out of the request would carry per-run text no
// client can match, which is why the constants below take no arguments.

/// The message of the not-found refusal of a REPORTING or UPDATING call — an id
/// that names no resource the request could address.
///
/// Recorded behaviour: the model gives these exceptions a `message` member but
/// no text for it, so the literal is the one the service is observed to answer.
pub const ITEM_NOT_FOUND_MESSAGE: &str = "The item specified was not found";

/// The message of the not-found refusal of a DELETE — a deliberately different
/// literal from [`ITEM_NOT_FOUND_MESSAGE`], which is what lets a client tell a
/// failed delete from a failed read apart on an otherwise identical untyped 404
/// (idempotent-delete logic keys on it).
///
/// Recorded behaviour, like the read path's.
pub const ITEM_TO_DELETE_NOT_FOUND_MESSAGE: &str = "Unable to find item to delete";

/// The message of the conflict refusal of a call that would leave two live
/// resources of one app environment sharing a name.
///
/// Recorded behaviour, like the not-found literals.
pub const DUPLICATE_ITEM_NAME_MESSAGE: &str = "Item name already exists";

/// The message of the wrong-scope refusal — an id that resolves, but under
/// another app or backend environment than the one the request addressed.
///
/// Recorded behaviour: ONE fixed sentence, which names no ids and no resource
/// kind — it reads the same whichever of this service's resources the id
/// resolved under — and is what separates a scope violation from an ordinary
/// malformed-parameter rejection, both untyped 400s.
pub const WRONG_SCOPE_MESSAGE: &str =
    "The item requests does not belong to that app or environment";

/// The `ResourceNotFoundException` payload a reporting or updating call answers
/// for an id that names no resource in the app environment the request
/// addressed.
pub fn item_not_found() -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(ITEM_NOT_FOUND_MESSAGE.to_string()),
    }
}

/// The `ResourceNotFoundException` payload a DELETE answers for an id that names
/// no resource in the app environment the request addressed: the delete path has
/// its own literal, so it is not the read path's payload.
pub fn item_to_delete_not_found() -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(ITEM_TO_DELETE_NOT_FOUND_MESSAGE.to_string()),
    }
}

/// The `InvalidParameterException` payload for an id that names a resource
/// another app environment owns: the request is addressed to the wrong scope,
/// which is a different failure from an id that names nothing.
pub fn item_wrong_scope() -> InvalidParameterException {
    InvalidParameterException {
        message: Some(WRONG_SCOPE_MESSAGE.to_string()),
    }
}

/// The `ResourceConflictException` payload for a name another live resource of
/// the same app environment already holds.
pub fn duplicate_item_name() -> ResourceConflictException {
    ResourceConflictException {
        message: Some(DUPLICATE_ITEM_NAME_MESSAGE.to_string()),
    }
}

// ---------------------------------------------------------------------------
// The tagging surface: which resource a `resourceArn` names, and its tag set.
// ---------------------------------------------------------------------------

/// The `service` element of an ARN naming a resource of this service.
pub const ARN_SERVICE: &str = "amplifyuibuilder";

/// A resource kind the tagging surface addresses.
///
/// Amplify UI Builder names each of its app-environment resources with an ARN of
/// the documented form
/// `arn:<partition>:amplifyuibuilder:<region>:<account>:app/<appId>/environment/<environmentName>/<collection>/<id>`,
/// whose `<collection>` is [`TaggedKind::collection`] — the element of that
/// resource's own ARN template in the model
/// (`smithy_query(query="shape", shape="ComponentResource,FormResource,ThemeResource,CodegenJobResource")`).
/// The model declares no ARN shape to read the form off — each tagging input's
/// `resourceArn` targets the bare `String` with neither `@length` nor `@pattern`
/// (`smithy_query(query="operation", operation="ListTagsForResource,TagResource,UntagResource")`)
/// — so the form is the documented one.
///
/// The kinds listed here are EVERY kind of this service whose own record holds a
/// tag set: a component, a form and a theme carry the tags their create stored,
/// and a codegen job carries the tags its start stored (`CodegenJob$tags`, which
/// `GetCodegenJob` reports). A record that holds a tag set is reachable through
/// the operations modelled to read and change one, so the two readers of a
/// resource — its own `Get…` and `ListTagsForResource` — never disagree about
/// whether it exists or about what it carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaggedKind {
    Component,
    Form,
    Theme,
    CodegenJob,
}

impl TaggedKind {
    /// Every kind the tagging surface addresses, in the order the resource's own
    /// collection URIs are listed in this crate's `OPERATIONS.md`.
    pub const ALL: [Self; 4] = [Self::Component, Self::Form, Self::Theme, Self::CodegenJob];

    /// The ARN path element naming the collection that holds this kind — the same
    /// element the resource's own REST URIs use
    /// (`/app/{appId}/environment/{environmentName}/components/{id}`).
    pub const fn collection(self) -> &'static str {
        match self {
            Self::Component => "components",
            Self::Form => "forms",
            Self::Theme => "themes",
            Self::CodegenJob => "codegen-jobs",
        }
    }

    /// The kind an ARN's collection element names, or `None` when it names no
    /// kind this service holds a tag set for.
    pub fn from_collection(collection: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|kind| kind.collection() == collection)
    }
}

/// The resource one `resourceArn` names: its kind, plus the (appId,
/// environmentName, id) every reader of this store is keyed by.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaggedResource {
    pub kind: TaggedKind,
    pub app_id: String,
    pub environment_name: String,
    pub id: String,
}

/// The resource `arn` names, or `None` when `arn` is not an ARN naming a resource
/// kind of this service.
///
/// The partition, region and account elements have to be PRESENT — an ARN is six
/// `:`-separated fields — but none of them is part of the lookup: this store keys
/// every resource by (appId, environmentName, id) alone and the emulator serves a
/// single account ([`::engine::emulator_account_id`]), so neither element tells
/// one resource it holds from another.
pub fn parse_resource_arn(arn: &str) -> Option<TaggedResource> {
    // The resource field is the sixth, and keeps any `:` of its own.
    let mut fields = arn.splitn(6, ':');
    if fields.next()? != "arn" {
        return None;
    }
    let _partition = fields.next()?;
    if fields.next()? != ARN_SERVICE {
        return None;
    }
    let _region = fields.next()?;
    let _account = fields.next()?;
    let segments: Vec<&str> = fields.next()?.split('/').collect();
    let ["app", app_id, "environment", environment_name, collection, id] = segments.as_slice()
    else {
        return None;
    };
    let kind = TaggedKind::from_collection(collection)?;
    if app_id.is_empty() || environment_name.is_empty() || id.is_empty() {
        return None;
    }
    Some(TaggedResource {
        kind,
        app_id: (*app_id).to_string(),
        environment_name: (*environment_name).to_string(),
        id: (*id).to_string(),
    })
}

/// The tags the resource `arn` names carries, or `None` when this store holds no
/// such resource.
///
/// A resource carrying no tag answers the EMPTY map: `tags` is `@required` on
/// `ListTagsForResourceResponse`, so "no tags" is an empty map there and only "no
/// such resource" is an absence. A pure read — the tag set every other reader of
/// that resource reports, cloned out of the one record holding it.
pub fn resource_tags(
    state: &ComAmazonawsAmplifyuibuilderAmplifyuibuilderState,
    arn: &str,
) -> Option<HashMap<String, String>> {
    let resource = parse_resource_arn(arn)?;
    let (app, environment, id) = (
        resource.app_id.as_str(),
        resource.environment_name.as_str(),
        resource.id.as_str(),
    );
    match resource.kind {
        TaggedKind::Component => state
            .component(app, environment, id)
            .map(|component| component.tags.clone()),
        TaggedKind::Form => state.form(app, environment, id).map(|form| form.tags.clone()),
        TaggedKind::Theme => state
            .theme(app, environment, id)
            .map(|theme| theme.tags.clone()),
        TaggedKind::CodegenJob => state
            .codegen_job(app, environment, id)
            .map(|job| job.tags.clone()),
    }
}

/// Hand the tag set of the resource `arn` names to `change`, or answer `None`
/// when this store holds no such resource.
///
/// The one place a tagging WRITE resolves its target, so `TagResource` and
/// `UntagResource` change exactly the record [`resource_tags`] and the resource's
/// own reader report. Only the tag set is touched: tagging a resource is not an
/// update of it, so nothing else about the record moves.
pub fn with_resource_tags_mut<R>(
    state: &mut ComAmazonawsAmplifyuibuilderAmplifyuibuilderState,
    arn: &str,
    change: impl FnOnce(&mut HashMap<String, String>) -> R,
) -> Option<R> {
    let resource = parse_resource_arn(arn)?;
    let (app, environment, id) = (
        resource.app_id.as_str(),
        resource.environment_name.as_str(),
        resource.id.as_str(),
    );
    match resource.kind {
        TaggedKind::Component => state
            .component_mut(app, environment, id)
            .map(|component| change(&mut component.tags)),
        TaggedKind::Form => state
            .form_mut(app, environment, id)
            .map(|form| change(&mut form.tags)),
        TaggedKind::Theme => state
            .theme_mut(app, environment, id)
            .map(|theme| change(&mut theme.tags)),
        TaggedKind::CodegenJob => state
            .codegen_job_mut(app, environment, id)
            .map(|job| change(&mut job.tags)),
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsAmplifyuibuilderAmplifyuibuilderState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsAmplifyuibuilderAmplifyuibuilderState>)
}
